use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::SET_COORDINATES;
use crate::arena_settings::{
    arena_repel_lifetime_ticks, arena_repel_radius_pixels, arena_repel_speed_pixels_per_second,
    arena_ship_afterburner_energy_per_second, arena_ship_antiwarp_energy_per_second,
    arena_ship_bomb_recoil_speed, arena_ship_cloak_energy_per_second,
    arena_ship_max_speed_pixels_per_second, arena_ship_max_thrust_pixels_per_second_squared,
    arena_ship_radius_pixels, arena_ship_stealth_energy_per_second,
    arena_ship_xradar_energy_per_second,
};
use crate::authority::WeaponFireRejectReason;
use crate::authority::{
    EnergyPool, POSITION_STATUS_CLOAK, POSITION_STATUS_FLASH, POSITION_STATUS_SAFETY,
    StatusEnergyCosts,
};
use crate::authority_identity::{LifeIdentity, PlayerRuntimeIdentity};
use crate::continuum_position::{
    ClientPositionSnapshot, parse_client_position_snapshot, reported_energy_within_limit,
    reported_position_delta_within_limit, reported_position_hull_overlaps_blocked_tile,
    reported_position_path_hull_overlaps_blocked_tile, reported_velocity_delta_within_limit,
    reported_velocity_within_limit, world_coordinates_in_bounds,
};
use crate::continuum_weapon::{
    continuum_weapon_alternate, continuum_weapon_type, valid_relayed_weapon_data,
};
use crate::security::log_security_reject;
use crate::state::{
    ClientHandle, ClientPacketSender, ServerProjectile, ServerRepelObservation, ServerState,
};
#[cfg(test)]
use crate::zone::position_blocking_tile;
use crate::zone::{
    MAP_TILES, TILE_SIZE_PX, ZoneMap, position_blocking_tile_with_door_seed,
    server_spawn_coordinates, tile_at_world, world_position_in_safe_tile_for_map,
};
use tracing::{debug, info};

// Per-event lines log under this module; see `log_targets`.
const _: () = assert!(crate::log_targets::is_child_of(
    crate::log_targets::POSITIONS_COMBAT,
    module_path!()
));

#[path = "positions/batching.rs"]
mod batching;
#[path = "positions/repel_prediction.rs"]
mod repel_prediction;
#[path = "positions/weapons.rs"]
mod weapons;

pub(crate) use batching::send_position_relays_batched;
use weapons::{
    WeaponBlock, accepted_position_weapon, mine_blocked_by_mine_limits,
    preview_accepted_position_weapon, preview_position_weapon,
    proximity_bomb_blocked_by_bomb_safety,
};

const POSITION_RELAY_RATE_WINDOW: Duration = Duration::from_millis(50);
const POSITION_RELAY_BURST_LIMIT: u8 = 3;
const POSITION_RELAY_RATE_PER_SECOND: f32 =
    POSITION_RELAY_BURST_LIMIT as f32 / POSITION_RELAY_RATE_WINDOW.as_secs_f32();
const POSITION_VELOCITY_ALLOWANCE_PX_PER_SECOND: f32 = 1_200.0;
const POSITION_VELOCITY_DELTA_GRACE_PX_PER_SECOND: f32 = 650.0;
const POSITION_DELTA_ALLOWANCE_PX_PER_SECOND: f32 = 1_200.0;
const POSITION_DELTA_GRACE_PX: f32 = 512.0;
const POSITION_COLLISION_RADIUS_GRACE_PX: f32 = 1.0;

pub(crate) fn accepted_position_weapon_applies_recoil(weapon: u16) -> bool {
    weapon != 0
        && matches!(continuum_weapon_type(weapon), 3 | 4)
        && !continuum_weapon_alternate(weapon)
}

fn prediction_correction_transaction(
    sender: &mut ClientHandle,
    player_id: u16,
    client_sequence: u16,
    server_tick: u32,
    mut authoritative_witness: crate::state::CanonicalPositionWitness,
    detected_state: crate::state::ServerSimMovement,
) -> crate::state::ServerMovementCorrection {
    authoritative_witness.state.energy = sender.server_energy as f32;
    authoritative_witness.state.max_energy = sender.server_max_energy as f32;
    sender.next_movement_correction_generation =
        sender.next_movement_correction_generation.wrapping_add(1);
    if sender.next_movement_correction_generation == 0 {
        sender.next_movement_correction_generation = 1;
    }
    let proposal = crate::movement_correction::MovementCorrection {
        reason: crate::movement_correction::MovementCorrection::REASON_REJECTED_WEAPON_RECOIL,
        repel_window: None,
        server_session_id: crate::wire::server_session_id(),
        player_id,
        life_seq: sender.life_seq,
        correction_generation: sender.next_movement_correction_generation,
        position_sequence: client_sequence,
        server_tick,
        position_x: authoritative_witness.state.position_x,
        position_y: authoritative_witness.state.position_y,
        velocity_x: authoritative_witness.state.velocity_x,
        velocity_y: authoritative_witness.state.velocity_y,
        orientation: authoritative_witness.state.orientation,
    };
    crate::state::ServerMovementCorrection {
        proposal,
        detected_state,
        replay_witness: Some(authoritative_witness),
        replay_server_tick: server_tick,
        predicted_repel_window: None,
        offer_reliable_id: None,
        application_sequence: None,
        terminal_state: None,
        terminal_server_tick: None,
        cancelled: false,
        receipt_reliable_id: None,
    }
}

/// Open one exact correction barrier when the server's accepted position
/// anchor has fallen behind the bounded input-recovery window. The proposal
/// carries only the last server-proven canonical state; no missing client
/// movement or weapon is promoted into authority. The application handler
/// commits that state at the browser's paused tail and clears the obsolete
/// rolling histories atomically.
pub(crate) fn begin_input_history_gap_correction(
    state: &mut ServerState,
    identity: PlayerRuntimeIdentity,
    detected_sequence: u16,
    detected_state: crate::state::ServerSimMovement,
    server_tick: u32,
) -> Option<crate::movement_correction::MovementCorrection> {
    if !state.player_runtime_matches(identity) {
        return None;
    }
    let sender = state.clients.get_mut(&identity.player_id())?;
    if sender.pending_movement_correction.is_some() {
        return None;
    }
    let last_position_sequence = sender.last_position_sequence?;
    let mut authoritative_witness = sender.canonical_movement_witness.filter(|witness| {
        witness.life_seq == identity.life.life_seq && witness.sequence == last_position_sequence
    })?;
    authoritative_witness.state.energy = sender.server_energy as f32;
    authoritative_witness.state.max_energy = sender.server_max_energy as f32;

    sender.next_movement_correction_generation =
        sender.next_movement_correction_generation.wrapping_add(1);
    if sender.next_movement_correction_generation == 0 {
        sender.next_movement_correction_generation = 1;
    }
    let proposal = crate::movement_correction::MovementCorrection {
        reason: crate::movement_correction::MovementCorrection::REASON_INPUT_HISTORY_GAP,
        repel_window: None,
        server_session_id: crate::wire::server_session_id(),
        player_id: identity.player_id(),
        life_seq: identity.life.life_seq,
        correction_generation: sender.next_movement_correction_generation,
        position_sequence: detected_sequence,
        server_tick,
        position_x: authoritative_witness.state.position_x,
        position_y: authoritative_witness.state.position_y,
        velocity_x: authoritative_witness.state.velocity_x,
        velocity_y: authoritative_witness.state.velocity_y,
        orientation: authoritative_witness.state.orientation,
    };
    sender.pending_movement_correction = Some(crate::state::ServerMovementCorrection {
        proposal,
        detected_state,
        replay_witness: Some(authoritative_witness),
        replay_server_tick: server_tick,
        predicted_repel_window: None,
        offer_reliable_id: None,
        application_sequence: None,
        terminal_state: None,
        terminal_server_tick: None,
        cancelled: false,
        receipt_reliable_id: None,
    });
    Some(proposal)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AcceptedPositionRelay {
    pub(crate) position: Vec<u8>,
    pub(crate) recipient_ids: Vec<u16>,
    /// The obsolete public LARGE_POSITION fanout token bucket remains
    /// observable to legacy tests/metrics, but it must never turn a committed
    /// authoritative admission into `None`: private lifecycle/Warp receipts
    /// are produced from the admitted result independently of public fanout.
    pub(crate) public_relay_suppressed: bool,
    pub(crate) warp_relocation_receipt: Option<(u32, u32, u16)>,
    pub(crate) repel_application_receipt: Option<(u64, u32, u32, u16, u16, u16, u16)>,
    /// Exact owner-private correction for this admitted tail. The server tick
    /// sends it through the reliable runtime-targeted path; it is never part
    /// of public position fanout.
    pub(crate) movement_correction: Option<crate::movement_correction::MovementCorrection>,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn broadcast_position(state: &Arc<Mutex<ServerState>>, player_id: u16, packet: &[u8]) {
    #[cfg(test)]
    let normalized_packet = normalize_legacy_mutated_position_fixture(packet);
    #[cfg(test)]
    let packet = normalized_packet.as_deref().unwrap_or(packet);
    #[cfg(test)]
    seed_position_input_witness_for_test(state, player_id, packet);
    let Some(relay) = accept_position_relay(state, player_id, packet) else {
        return;
    };
    let senders = position_relay_senders(state, &relay.recipient_ids);
    for tx in senders {
        let _ = tx.send(relay.position.clone());
    }
}

/// Test-only variant of `broadcast_position` that bypasses the
/// `snapshots_enabled()` gate and always uses the legacy
/// `visible_position_recipient_ids` fan-out. Lets tests keep
/// observing recipient delivery through their `mpsc::Receiver`s
/// after the flag flip that turned snapshots on by default —
/// the server-state side-effects (security checks, energy recharge,
/// projectile recording, etc.) are identical to production
/// `broadcast_position`, only the recipient list is forced to the
/// pre-flip shape.
///
/// Tests that explicitly assert the snapshot/AOI delivery path
/// should keep calling `broadcast_position` directly (or the
/// snapshot emission helpers) — this exists for the position-relay
/// regression tests whose assertions are the rx-channel fan-out.
#[cfg(test)]
pub(crate) fn broadcast_position_observable(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    packet: &[u8],
) {
    let normalized_packet = normalize_legacy_mutated_position_fixture(packet);
    let packet = normalized_packet.as_deref().unwrap_or(packet);
    seed_position_input_witness_for_test(state, player_id, packet);
    let Some(relay) = accept_position_relay(state, player_id, packet) else {
        return;
    };
    if relay.public_relay_suppressed {
        return;
    }
    let recipient_ids = {
        let state = state.lock().expect("server state should not be poisoned");
        state.visible_position_recipient_ids(player_id, &relay.position)
    };
    let senders = position_relay_senders(state, &recipient_ids);
    for tx in senders {
        let _ = tx.send(relay.position.clone());
    }
}

/// Older fixtures mutate one of the leading Continuum fields after using the
/// common packet constructor. Keep those unrelated relay/AOI fixtures valid
/// by rebuilding their test-only exact block. Direct authority tests never
/// pass through this adapter and therefore exercise fail-closed mismatches.
#[cfg(test)]
fn normalize_legacy_mutated_position_fixture(packet: &[u8]) -> Option<Vec<u8>> {
    let offset = crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_OFFSET;
    let end = crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET;
    if packet.len() < end
        || packet.get(offset).copied()
            != Some(crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_TYPE)
        || packet.get(offset + 1).copied()
            != Some(crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_VERSION)
        || packet.get(offset + 2).copied()
            != Some(crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_PAYLOAD_LEN)
    {
        return None;
    }
    let mut normalized = packet.to_vec();
    let state = crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
        packet[1],
        u16::from_le_bytes(packet[12..14].try_into().ok()?),
        u16::from_le_bytes(packet[8..10].try_into().ok()?),
        i16::from_le_bytes(packet[6..8].try_into().ok()?),
        i16::from_le_bytes(packet[14..16].try_into().ok()?),
    );
    let mut encoded = Vec::with_capacity(end - offset);
    state.encode(&mut encoded);
    normalized[offset..end].copy_from_slice(&encoded);
    Some(normalized)
}

/// Low-level relay tests historically called the admission function without
/// first sending the input record that the real connection path always emits.
/// Keep those fixtures honest without weakening production admission. Focused
/// witness/security tests call `accept_position_relay*` directly and therefore
/// exercise the fail-closed behavior.
#[cfg(test)]
fn seed_position_input_witness_for_test(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    packet: &[u8],
) {
    let Some(snapshot) = parse_client_position_snapshot(packet) else {
        return;
    };
    let Some(sequence) = snapshot.client_sequence else {
        return;
    };
    let mut state = state.lock().expect("server state should not be poisoned");
    let server_tick = crate::wire::current_tick();
    // These legacy relay/AOI fixtures deliberately authorize arbitrary test
    // boundaries and are not snap-back tests. Preserve their pre-rollout
    // isolation without mutating the client's private observer before an
    // admission whose atomicity the fixture may be asserting.
    state.config.movement_snapback = false;
    let Some(client) = state.clients.get_mut(&player_id) else {
        return;
    };
    if client.last_position_sequence.is_none() && client.canonical_movement_witness.is_none() {
        client.life_position_sequence_base = Some(sequence);
    }
    // These legacy relay/AOI fixtures exercise checks other than canonical
    // movement (energy, weapon ownership, path geometry, fanout). Mark their
    // exact current packet as a test-only boundary without inventing a prior
    // float or perturbing sequence/history. Direct authority tests and every
    // production packet bypass this adapter.
    client.pending_frequency_baseline = Some(snapshot);
    let first = client
        .last_position_sequence
        .map_or(sequence, |previous| previous.wrapping_add(1));
    let steps = sequence.wrapping_sub(first).saturating_add(1);
    if steps == 0 || steps > crate::state::MAX_RECENT_INPUT_OBSERVATIONS as u16 {
        return;
    }
    for offset in 0..steps {
        let witness_sequence = first.wrapping_add(offset);
        if !client
            .recent_input_records
            .iter()
            .any(|record| record.sequence == witness_sequence)
        {
            let input = crate::state::ClientInputObservation {
                bits: 0,
                direction: snapshot.direction,
            };
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: witness_sequence,
                    input,
                    door_seed: 0,
                    door_phase_tick: 0,
                });
            if client.last_input_seq.is_none_or(|last| {
                witness_sequence != last && witness_sequence.wrapping_sub(last) < (1 << 15)
            }) {
                client.last_input_seq = Some(witness_sequence);
                client.last_input_record = Some(input);
            }
        }
        let _ = client
            .observer_combat_timeline
            .seed_input_decision_for_test(witness_sequence, server_tick);
    }
}

pub(crate) fn accept_position_relay(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    packet: &[u8],
) -> Option<AcceptedPositionRelay> {
    let identity = state
        .lock()
        .expect("server state should not be poisoned")
        .active_life_identity(player_id)?;
    accept_position_relay_for_life(state, identity, packet)
}

pub(crate) fn accept_position_relay_for_life(
    state: &Arc<Mutex<ServerState>>,
    identity: LifeIdentity,
    packet: &[u8],
) -> Option<AcceptedPositionRelay> {
    accept_position_relay_for_life_at_tick(state, identity, packet, crate::wire::current_tick())
}

pub(crate) fn accept_position_relay_for_life_at_tick(
    state: &Arc<Mutex<ServerState>>,
    identity: LifeIdentity,
    packet: &[u8],
    server_tick_at_accept: u32,
) -> Option<AcceptedPositionRelay> {
    let mut state = state.lock().expect("server state should not be poisoned");
    state.weapon_admission_deferred.remove(&identity);
    let relay = accept_position_relay_for_life_in_state_at_tick(
        &mut state,
        identity,
        packet,
        server_tick_at_accept,
    );
    state.weapon_admission_deferred.remove(&identity);
    relay
}

/// Production admission boundary for queued client work. The never-reused
/// session id is validated under the same state lock as the life/position
/// transaction and is returned with the relay so any later application
/// receipt can target only this exact runtime incarnation.
#[cfg(test)]
pub(crate) fn accept_position_relay_for_runtime_at_tick(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    packet: &[u8],
    server_tick_at_accept: u32,
) -> Option<(PlayerRuntimeIdentity, AcceptedPositionRelay)> {
    match accept_position_relay_for_runtime_at_tick_outcome(
        state,
        identity,
        packet,
        server_tick_at_accept,
    ) {
        RuntimePositionAdmission::Accepted(accepted) => Some(accepted),
        RuntimePositionAdmission::DeferredWeapon
        | RuntimePositionAdmission::DeferredSpawn
        | RuntimePositionAdmission::Rejected => None,
    }
}

#[derive(Debug)]
pub(crate) enum RuntimePositionAdmission {
    Accepted((PlayerRuntimeIdentity, AcceptedPositionRelay)),
    DeferredWeapon,
    DeferredSpawn,
    Rejected,
}

/// Scheduler-facing result. Capacity pressure or temporarily blocked spawn
/// geometry is not an authority reject: the tick loop durably requeues the
/// original one-send browser packet without an application receipt.
pub(crate) fn accept_position_relay_for_runtime_at_tick_outcome(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    packet: &[u8],
    server_tick_at_accept: u32,
) -> RuntimePositionAdmission {
    let mut state = state.lock().expect("server state should not be poisoned");
    state.weapon_admission_deferred.remove(&identity.life);
    if !state.player_runtime_matches(identity) {
        return RuntimePositionAdmission::Rejected;
    }
    let mut spawn_deferred = false;
    let relay = accept_position_relay_with_spawn_deferral(
        &mut state,
        identity.life,
        packet,
        server_tick_at_accept,
        &mut spawn_deferred,
    );
    match relay {
        Some(relay) => RuntimePositionAdmission::Accepted((identity, relay)),
        None if state.weapon_admission_deferred.remove(&identity.life) => {
            RuntimePositionAdmission::DeferredWeapon
        }
        None if spawn_deferred => RuntimePositionAdmission::DeferredSpawn,
        None => RuntimePositionAdmission::Rejected,
    }
}

/// Commit a previously proposed movement correction at the client's exact
/// application tail. Detection intentionally leaves canonical movement on the
/// old lineage; this is the sole transaction that establishes the new one.
pub(crate) fn apply_movement_correction_application_at_tick(
    state: &mut ServerState,
    identity: PlayerRuntimeIdentity,
    application: crate::movement_correction::MovementCorrectionApplied,
    server_tick: u32,
) -> Option<crate::movement_correction::MovementCorrectionAccepted> {
    if !state.player_runtime_matches(identity)
        || application.server_session_id != crate::wire::server_session_id()
        || application.player_id != identity.player_id()
        || application.life_seq != identity.life.life_seq
    {
        return None;
    }
    let transaction = state
        .clients
        .get(&identity.player_id())?
        .pending_movement_correction
        .as_ref()?;
    if !transaction.application_matches(application) {
        return None;
    }
    if transaction.cancelled {
        return transaction.terminal_receipt();
    }
    if let Some(committed_sequence) = transaction.application_sequence {
        return (committed_sequence == application.application_sequence)
            .then(|| transaction.terminal_receipt())
            .flatten();
    }
    let brick_relocation = transaction.proposal.reason
        == crate::movement_correction::MovementCorrection::REASON_BRICK_OVERLAP;
    let replaces_old_tail = matches!(
        transaction.proposal.reason,
        crate::movement_correction::MovementCorrection::REASON_INPUT_HISTORY_GAP
            | crate::movement_correction::MovementCorrection::REASON_BRICK_OVERLAP
    );

    if application.intent
        == crate::movement_correction::MovementCorrectionApplied::INTENT_CANCEL_NEWER_BOUNDARY
    {
        let transaction = state
            .clients
            .get_mut(&identity.player_id())?
            .pending_movement_correction
            .as_mut()?;
        transaction.cancel_at_application_sequence(application.application_sequence);
        return transaction.terminal_receipt();
    }
    let forward = application
        .application_sequence
        .wrapping_sub(application.detected_sequence);
    if forward >= (1 << 15) {
        let transaction = state
            .clients
            .get_mut(&identity.player_id())?
            .pending_movement_correction
            .as_mut()?;
        transaction.cancel_before_application();
        return transaction.terminal_receipt();
    }

    let player_id = identity.player_id();
    let attached_riders = state
        .turret_links
        .values()
        .any(|host_id| *host_id == player_id);
    let is_flagger = player_flagger_active(state, player_id);
    let player_frequency = state.clients.get(&player_id)?.player.frequency;
    let enemy_brick_tiles = state
        .active_bricks
        .iter()
        .filter_map(|brick| {
            let team = u16::from_le_bytes(brick.packet.get(9..11)?.try_into().ok()?);
            (team != player_frequency).then_some(
                brick
                    .tiles
                    .iter()
                    .map(|&(tile_x, tile_y)| (tile_x as usize, tile_y as usize))
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect::<Vec<_>>();
    let map = state.zone_map();
    let (spawn_x, spawn_y) = server_spawn_coordinates(map);
    if !replaces_old_tail {
        let current_tail = {
            let client = state.clients.get(&player_id)?;
            let direct = client.canonical_movement_witness.filter(|witness| {
                witness.life_seq == client.life_seq
                    && witness.sequence == application.application_sequence
            });
            direct.or_else(|| {
                let upgrades = crate::sim_observation::movement_upgrades_for_client(client);
                let repel_active = client.server_repel_until_tick.is_some_and(|until| {
                    !crate::continuum::continuum_tick_reached(server_tick, until)
                });
                crate::sim_observation::replay_canonical_position_gap(
                    client,
                    map,
                    is_flagger,
                    attached_riders,
                    repel_active,
                    upgrades,
                    &enemy_brick_tiles,
                    application.application_sequence,
                    server_tick,
                    spawn_x,
                    spawn_y,
                )
                .and_then(|replay| replay.witnesses.last().copied())
            })
        };
        let Some(_current_tail) = current_tail else {
            // The marker overtook too much unordered old-lineage history to prove
            // its exact tail. Cancel durably instead of leaving the browser paused
            // or guessing at a replacement state.
            let transaction = state
                .clients
                .get_mut(&player_id)?
                .pending_movement_correction
                .as_mut()?;
            transaction.cancel_before_application();
            return transaction.terminal_receipt();
        };
    }

    // Anything already received before the reliable marker was drained on
    // this tick. Remove a cross-channel late suffix now; its sequence is at or
    // before the application boundary and therefore belongs to the old
    // lineage. The browser emits nothing newer until the receipt arrives.
    state.pending_position_inputs.clear_player(player_id);
    state
        .input_recovery_deferred_at
        .retain(|life, _| *life != identity.life);

    // Re-select against the current map/brick topology at application. A
    // second wall may have covered the offered destination while RTC waited.
    let brick_destination = if brick_relocation {
        let ship = state.clients.get(&player_id)?.player.ship;
        match crate::objectives::brick_overlap_destination(
            state,
            player_frequency,
            ship,
            server_tick,
        ) {
            Some(destination) => Some(destination),
            None => {
                let transaction = state
                    .clients
                    .get_mut(&player_id)?
                    .pending_movement_correction
                    .as_mut()?;
                transaction.cancel_before_application();
                return transaction.terminal_receipt();
            }
        }
    } else {
        None
    };
    let correction_movement_budget = {
        let client = state.clients.get(&player_id)?;
        let steps = if replaces_old_tail {
            // A history-gap/brick correction discards unproven movement;
            // skipped sequence numbers do not simulate additional flight.
            0
        } else {
            client.last_position_sequence.map_or(0, |previous| {
                let forward = application.application_sequence.wrapping_sub(previous);
                if forward < (1 << 15) { forward } else { 0 }
            })
        };
        let Some(budget) =
            client
                .movement_time_budget
                .admit(server_tick, client.last_position_server_tick, steps)
        else {
            log_security_reject(player_id, "movement_correction", "movement_time_exceeded");
            return None;
        };
        budget
    };
    let (position, witness) = {
        let client = state.clients.get_mut(&player_id)?;
        if let Some((x, y)) = brick_destination {
            let witness = client
                .pending_movement_correction
                .as_mut()?
                .replay_witness
                .as_mut()?;
            witness.state.position_x = f32::from(x);
            witness.state.position_y = f32::from(y);
            witness.state.velocity_x = 0.0;
            witness.state.velocity_y = 0.0;
        }
        let (shadow_witness, shadow_tick) = {
            let transaction = client.pending_movement_correction.as_ref()?;
            let Some(witness) = transaction.replay_witness else {
                let transaction = client.pending_movement_correction.as_mut()?;
                transaction.cancel_before_application();
                return transaction.terminal_receipt();
            };
            (witness, transaction.replay_server_tick)
        };
        let shadow_tail = if replaces_old_tail {
            // A history gap discards an unverifiable suffix; a forced brick
            // warp deliberately replaces it with the clear server spawn.
            // Both bind the paused tail without replaying old-lineage motion.
            let mut witness = shadow_witness;
            witness.sequence = application.application_sequence;
            Some(witness)
        } else if shadow_witness.sequence == application.application_sequence {
            Some(shadow_witness)
        } else {
            let upgrades = crate::sim_observation::movement_upgrades_for_client(client);
            let repel_active = client
                .server_repel_until_tick
                .is_some_and(|until| !crate::continuum::continuum_tick_reached(server_tick, until));
            crate::sim_observation::replay_canonical_position_gap_from_witness(
                client,
                shadow_witness,
                shadow_tick,
                map,
                is_flagger,
                attached_riders,
                repel_active,
                upgrades,
                &enemy_brick_tiles,
                application.application_sequence,
                server_tick,
                spawn_x,
                spawn_y,
                None,
                None,
            )
            .and_then(|replay| replay.witnesses.last().copied())
        };
        let Some(mut witness) = shadow_tail else {
            let transaction = client.pending_movement_correction.as_mut()?;
            transaction.cancel_before_application();
            return transaction.terminal_receipt();
        };
        let application_record_door_phase = client
            .recent_input_records
            .iter()
            .rev()
            .find(|record| record.sequence == application.application_sequence)
            .or_else(|| client.recent_input_records.back())
            .map(|record| (record.door_seed, record.door_phase_tick));
        // A correction application always replaces the paused predecessor
        // lineage at the server's current transaction boundary. Its retained
        // application-sequence input can be arbitrarily old: a browser may
        // pause behind the reliable offer while server-authored door phases
        // keep advancing. Copying that historical phase into the corrected
        // witness makes the first honest post-receipt input impossible to
        // replay once the bounded phase reconstruction horizon is crossed.
        // Re-anchor every applied correction to the latest server-authenticated
        // phase that has actually begun at the application tick. The corrected
        // hull must also fit that topology; phase metadata alone cannot carry
        // an old open-door pose through a door that is now closed.
        let current_authoritative_door_phase =
            crate::doors::client_door_phase_candidates_at_tick(client.door_state, server_tick)
                .into_iter()
                .filter(|phase| {
                    crate::continuum::continuum_tick_diff(server_tick, phase.started_tick) >= 0
                })
                .min_by_key(|phase| {
                    crate::continuum::continuum_tick_elapsed(server_tick, phase.started_tick)
                })
                .map(|phase| (phase.seed, phase.started_tick));
        let correction_door_phase =
            current_authoritative_door_phase.or(application_record_door_phase);
        let (door_seed, door_phase_tick) = correction_door_phase
            .unwrap_or((shadow_witness.door_seed, shadow_witness.door_phase_tick));
        let mut corrected_state = witness.state;
        // A correction is an absolute current-topology bind, potentially after
        // a pause longer than retained phase history. Query every currently
        // closed dynamic door with the shared reference-game overlap geometry.
        // Zero is the all-open query mask, not a claimed historical phase.
        // Ordinary walls are excluded by that shared door-only predicate.
        if crate::gameplay_rules::newly_closed_door_overlap(
            corrected_state.position_x,
            corrected_state.position_y,
            arena_ship_radius_pixels(client.player.ship),
            MAP_TILES,
            TILE_SIZE_PX,
            0,
            door_seed,
            |x, y| map.tiles.get(y as usize * MAP_TILES + x as usize).copied(),
            crate::zone::position_blocking_tile_with_door_seed,
        ) {
            debug!(
                player_id,
                life_seq = client.life_seq,
                server_tick,
                door_phase_tick,
                door_seed,
                previous_x = corrected_state.position_x,
                previous_y = corrected_state.position_y,
                spawn_x,
                spawn_y,
                "movement correction relocated hull out of current closed door"
            );
            corrected_state.position_x = spawn_x as f32;
            corrected_state.position_y = spawn_y as f32;
            corrected_state.velocity_x = 0.0;
            corrected_state.velocity_y = 0.0;
        }
        corrected_state.energy = client.server_energy as f32;
        corrected_state.max_energy = client.server_max_energy as f32;
        let wire_x = corrected_state
            .position_x
            .round()
            .clamp(0.0, u16::MAX as f32) as u16;
        let wire_y = corrected_state
            .position_y
            .round()
            .clamp(0.0, u16::MAX as f32) as u16;
        let wire_velocity_x =
            crate::continuum_position::protocol_velocity_component(corrected_state.velocity_x);
        let wire_velocity_y =
            crate::continuum_position::protocol_velocity_component(corrected_state.velocity_y);
        let direction = crate::continuum_position::protocol_direction(corrected_state.orientation);
        witness.sequence = application.application_sequence;
        witness.life_seq = client.life_seq;
        witness.state = corrected_state;
        witness.wire_x = wire_x;
        witness.wire_y = wire_y;
        witness.wire_velocity_x = wire_velocity_x;
        witness.wire_velocity_y = wire_velocity_y;
        witness.direction = direction;
        witness.door_seed = door_seed;
        witness.door_phase_tick = door_phase_tick;
        let mut position = client.last_position.clone()?;
        if position.len() < 23 {
            return None;
        }
        position[1] = direction;
        position[2..4].copy_from_slice(&((server_tick & 0xffff) as u16).to_le_bytes());
        position[4..6].copy_from_slice(&wire_x.to_le_bytes());
        position[6..8].copy_from_slice(&wire_velocity_y.to_le_bytes());
        position[10..12].copy_from_slice(&wire_velocity_x.to_le_bytes());
        position[15..17].copy_from_slice(&wire_y.to_le_bytes());
        position[19..21].copy_from_slice(&0_u16.to_le_bytes());
        position[21..23].copy_from_slice(&client.server_energy.to_le_bytes());

        client.last_input_seq = Some(application.application_sequence);
        client.last_input_record = None;
        client.recent_input_records.clear();
        client.recent_input_rocket_assertions.clear();
        client.last_position_sequence = Some(application.application_sequence);
        client.movement_time_budget = correction_movement_budget;
        client.canonical_position_witnesses.clear();
        client.canonical_position_witnesses.push_back(witness);
        client.canonical_movement_witness = Some(witness);
        client.server_sim_state = Some(corrected_state);
        client.server_sim_anchor_tick = Some(server_tick);
        client.server_sim_input_bits = None;
        client.movement_drift_x = 0.0;
        client.movement_drift_y = 0.0;
        client.last_position_server_tick = Some(server_tick);
        client
            .position_history
            .push_back(crate::state::RecentPositionSample {
                tick: server_tick,
                x: corrected_state.position_x,
                y: corrected_state.position_y,
            });
        while client.position_history.len() > crate::state::MAX_POSITION_HISTORY {
            client.position_history.pop_front();
        }
        client.begin_actor_motion_segment();
        let motion_append = client.append_actor_motion_witness(server_tick, witness);
        debug_assert_eq!(
            motion_append,
            Ok(()),
            "a committed correction must establish its exact motion segment"
        );
        let transaction = client
            .pending_movement_correction
            .as_mut()
            .expect("validated correction transaction");
        transaction.application_sequence = Some(application.application_sequence);
        transaction.terminal_state = Some(corrected_state);
        transaction.terminal_server_tick = Some(server_tick);
        (position, witness)
    };
    state.set_client_position(player_id, position);
    // A forced brick spawn is gameplay, not network prediction drift.
    if !brick_relocation {
        state.movement_snapbacks_total = state.movement_snapbacks_total.saturating_add(1);
        state.movement_snapbacks_since_summary =
            state.movement_snapbacks_since_summary.saturating_add(1);
        if let Some(client) = state.clients.get_mut(&player_id) {
            client.session_summary.snapbacks += 1;
        }
        tracing::warn!(
            player_id,
            life_seq = identity.life.life_seq,
            correction_generation = application.correction_generation,
            detected_sequence = application.detected_sequence,
            application_sequence = application.application_sequence,
            corrected_x = witness.state.position_x,
            corrected_y = witness.state.position_y,
            "movement snapback application committed"
        );
    }
    tracing::info!(
        "zubspace_event {{\"type\":\"{}\",\"player_id\":{},\"life_seq\":{},\"correction_generation\":{},\"detected_sequence\":{},\"application_sequence\":{},\"server_tick\":{},\"position_x\":{},\"position_y\":{}}}",
        if brick_relocation {
            "brick_overlap_relocation_committed"
        } else {
            "movement_snapback_committed"
        },
        player_id,
        identity.life.life_seq,
        application.correction_generation,
        application.detected_sequence,
        application.application_sequence,
        server_tick,
        witness.state.position_x,
        witness.state.position_y,
    );
    state.clients[&player_id]
        .pending_movement_correction
        .as_ref()
        .and_then(crate::state::ServerMovementCorrection::terminal_receipt)
}

/// Deterministic terminal for a valid current-life application whose proposal
/// was already retired. This closes the browser's pause barrier without
/// granting a rebase: the receipt carries the server's current canonical
/// state and the client's exact marker identity as a cancelled outcome.
pub(crate) fn cancelled_movement_correction_receipt(
    state: &mut ServerState,
    identity: PlayerRuntimeIdentity,
    application: crate::movement_correction::MovementCorrectionApplied,
    server_tick: u32,
) -> Option<crate::movement_correction::MovementCorrectionAccepted> {
    let (receipt, canonical) =
        cancelled_movement_correction_receipt_for_state(state, identity, application)?;
    state
        .clients
        .get_mut(&identity.player_id())?
        .pending_movement_correction = Some(crate::state::ServerMovementCorrection {
        proposal: crate::movement_correction::MovementCorrection {
            reason: crate::movement_correction::MovementCorrection::REASON_SUSTAINED_DRIFT,
            repel_window: None,
            server_session_id: application.server_session_id,
            player_id: application.player_id,
            life_seq: application.life_seq,
            correction_generation: application.correction_generation,
            position_sequence: application.detected_sequence,
            server_tick,
            position_x: canonical.position_x,
            position_y: canonical.position_y,
            velocity_x: canonical.velocity_x,
            velocity_y: canonical.velocity_y,
            orientation: canonical.orientation,
        },
        detected_state: canonical,
        replay_witness: None,
        replay_server_tick: server_tick,
        predicted_repel_window: None,
        offer_reliable_id: None,
        application_sequence: Some(application.application_sequence),
        terminal_state: None,
        terminal_server_tick: None,
        cancelled: true,
        receipt_reliable_id: None,
    });
    Some(receipt)
}

pub(crate) fn cancelled_movement_correction_receipt_for_state(
    state: &ServerState,
    identity: PlayerRuntimeIdentity,
    application: crate::movement_correction::MovementCorrectionApplied,
) -> Option<(
    crate::movement_correction::MovementCorrectionAccepted,
    crate::state::ServerSimMovement,
)> {
    if !state.player_runtime_matches(identity)
        || application.server_session_id != crate::wire::server_session_id()
        || application.player_id != identity.player_id()
        || application.life_seq != identity.life.life_seq
    {
        return None;
    }
    let canonical = {
        let client = state.clients.get(&identity.player_id())?;
        client
            .canonical_movement_witness
            .filter(|witness| witness.life_seq == identity.life.life_seq)
            .map(|witness| witness.state)
            .or_else(|| {
                crate::continuum_position::parse_client_position_snapshot(
                    client.last_position.as_ref()?,
                )
                .map(|snapshot| snapshot.canonical_state.values())
                .map(
                    |(position_x, position_y, velocity_x, velocity_y, orientation)| {
                        crate::state::ServerSimMovement {
                            position_x,
                            position_y,
                            velocity_x,
                            velocity_y,
                            orientation,
                            energy: f32::from(client.server_energy),
                            max_energy: f32::from(client.server_max_energy),
                        }
                    },
                )
            })?
    };
    let receipt = crate::movement_correction::MovementCorrectionAccepted {
        server_tick: 0, // No rebase occurs for a cancelled transaction.
        outcome: crate::movement_correction::MovementCorrectionAccepted::OUTCOME_CANCELLED,
        server_session_id: application.server_session_id,
        player_id: application.player_id,
        life_seq: application.life_seq,
        correction_generation: application.correction_generation,
        detected_sequence: application.detected_sequence,
        application_sequence: application.application_sequence,
        position_x: canonical.position_x,
        position_y: canonical.position_y,
        velocity_x: canonical.velocity_x,
        velocity_y: canonical.velocity_y,
        orientation: canonical.orientation,
    };
    Some((receipt, canonical))
}

pub(crate) fn accept_position_relay_for_life_in_state_at_tick(
    state: &mut ServerState,
    identity: LifeIdentity,
    packet: &[u8],
    server_tick_at_accept: u32,
) -> Option<AcceptedPositionRelay> {
    accept_position_relay_with_spawn_deferral(
        state,
        identity,
        packet,
        server_tick_at_accept,
        &mut false,
    )
}

fn accept_position_relay_with_spawn_deferral(
    state: &mut ServerState,
    identity: LifeIdentity,
    packet: &[u8],
    server_tick_at_accept: u32,
    spawn_deferred: &mut bool,
) -> Option<AcceptedPositionRelay> {
    let player_id = identity.player_id;
    let Some(snapshot) = parse_client_position_snapshot(packet) else {
        log_security_reject(player_id, "position", "malformed");
        return None;
    };
    if snapshot.timestamp != 0 {
        log_security_reject(player_id, "position", "client_timestamp_nonzero");
        return None;
    }
    let Some(mut position) = large_position_from_client_snapshot(player_id, snapshot) else {
        log_security_reject(player_id, "position", "invalid_packet");
        return None;
    };
    let Some(client_sequence) = snapshot.client_sequence else {
        log_security_reject(player_id, "position", "missing_sequence");
        return None;
    };
    let mut warp_relocation_receipt: Option<(u32, u32, u16)> = None;
    let mut repel_application_receipt: Option<(u64, u32, u32, u16, u16, u16, u16)> = None;
    let mut movement_correction = None;
    let mut prediction_correction = None;
    let admission = 'admission: {
        if !state.identity_matches_active_life(identity) {
            return None;
        }
        if !state
            .clients
            .get(&player_id)
            .is_some_and(|sender| sender.position_sequence_belongs_to_active_life(client_sequence))
        {
            debug!(
                player_id,
                client_sequence,
                "dropping previous-life position at authoritative admission boundary"
            );
            return None;
        }
        if let Some(dead_sender) = state.clients.get(&player_id).filter(|sender| !sender.alive) {
            let due = dead_sender.respawn_at_tick.is_some_and(|respawn_at_tick| {
                crate::continuum::continuum_tick_reached(server_tick_at_accept, respawn_at_tick)
            });
            if !due {
                debug!(
                    player_id,
                    client_sequence,
                    "dropping stale position while authoritative lifecycle is dead"
                );
                return None;
            }
            let mut expected = dead_sender.clone();
            expected.alive = true;
            expected.respawn_at_tick = None;
            expected.reset_server_runtime_for_ship_at_tick(server_tick_at_accept);
            if !respawn_position_is_canonical(&expected, state.zone_map(), snapshot) {
                let (spawn_x, spawn_y) = server_spawn_coordinates(state.zone_map());
                debug!(
                    player_id,
                    client_sequence,
                    life_seq = expected.life_seq,
                    reported_bounty = snapshot.bounty,
                    expected_bounty = expected.server_bounty,
                    reported_energy = snapshot.energy,
                    expected_energy = expected.server_energy,
                    expected_spawn_x = spawn_x,
                    expected_spawn_y = spawn_y,
                    reported = ?snapshot,
                    "dropping non-respawn position while authoritative lifecycle is dead"
                );
                return None;
            }
        }
        let pending_ship_life_reset =
            match pending_ship_life_reset_position_is_canonical(&state, player_id, snapshot) {
                Some(false) => {
                    log_security_reject(player_id, "position", "noncanonical_ship_life_reset");
                    return None;
                }
                Some(true) => true,
                None => false,
            };
        let pending_frequency_baseline =
            match pending_frequency_baseline_is_canonical(&state, player_id, snapshot) {
                Some(false) => {
                    log_security_reject(player_id, "position", "noncanonical_frequency_baseline");
                    return None;
                }
                Some(true) => true,
                None => false,
            };
        // The realtime movement lane is deliberately unordered. Once a newer
        // position has committed, an older movement-only datagram has no
        // authority or side effect left to validate. Drop it before looking
        // up an input record that may legitimately have been omitted or
        // retired. Weapon mirrors and every lifecycle/application marker stay
        // on the fail-closed witness path below.
        if snapshot.weapon == 0
            && snapshot.repel_snapshot_id.is_none()
            && snapshot.warp_collectible_id.is_none()
            && !pending_ship_life_reset
            && !pending_frequency_baseline
            && state.clients.get(&player_id).is_some_and(|sender| {
                sender.alive
                    && !client_position_sequence_advances(
                        sender.last_position_sequence,
                        client_sequence,
                    )
            })
        {
            debug!(
                player_id,
                client_sequence,
                last_sequence = ?state.clients
                    .get(&player_id)
                    .and_then(|sender| sender.last_position_sequence),
                "dropping duplicate or reordered side-effect-free position"
            );
            return None;
        }
        // Security-critical admission boundary. Bind the position and any
        // weapon it carries to the exact input sequence and canonical heading
        // before stale weapon-mirror handling, respawn, energy, weapon-
        // sequence, simulation-baseline, drift, or public-position mutation.
        let (canonical_input, mut canonical_door_seed, mut canonical_door_phase_tick) = {
            let Some(sender) = state.clients.get(&player_id) else {
                return None;
            };
            match crate::sim_observation::validate_position_input_witness(
                sender,
                client_sequence,
                snapshot.direction,
                server_tick_at_accept,
            ) {
                Ok(input) => {
                    let Some(record) = sender
                        .recent_input_records
                        .iter()
                        .rev()
                        .find(|record| record.sequence == client_sequence)
                    else {
                        log_security_reject(player_id, "position", "missing_input_witness");
                        return None;
                    };
                    (input, record.door_seed, record.door_phase_tick)
                }
                Err(error) => {
                    if error == crate::sim_observation::PositionInputWitnessError::Missing {
                        crate::sim_observation::trace_missing_position_input_witness(
                            sender,
                            player_id,
                            client_sequence,
                            snapshot.direction,
                            snapshot.weapon,
                            server_tick_at_accept,
                            "position_admission",
                        );
                    }
                    log_security_reject(player_id, "position", error.security_reason());
                    return None;
                }
            }
        };
        if state.clients.get(&player_id).is_some_and(|sender| {
            sender.pending_frequency_baseline.is_none()
                && sender.life_position_sequence_base.is_some()
                && sender.last_position_sequence.is_none()
        }) && (canonical_input.bits != 0 || canonical_input.direction != 0)
        {
            log_security_reject(player_id, "position", "noncanonical_ship_life_reset");
            return None;
        }
        let canonical_direction = canonical_input.direction;
        let map = state.zone_map();
        let now = Instant::now();
        let sim_observe = state.config.sim_observe;
        let movement_snapback = state.config.movement_snapback;
        let movement_snapback_test_force_once = state.config.movement_snapback_test_force_once;
        position[1] = canonical_direction;
        position[2..4].copy_from_slice(&((server_tick_at_accept & 0xffff) as u16).to_le_bytes());
        let bomb_safety_blocked =
            proximity_bomb_blocked_by_bomb_safety(&state, player_id, snapshot);
        let mine_limit_blocked = mine_blocked_by_mine_limits(&state, player_id, snapshot);
        // Read turret host count BEFORE the mutable borrow on
        // `state.clients` so the sim observer can use the right tuning.
        let attached_riders = state
            .turret_links
            .values()
            .any(|host_id| *host_id == player_id);
        let is_flagger = player_flagger_active(&state, player_id);
        // Also collect enemy-team bricks so the sim's collision sweep
        // can treat friendly bricks as passable. Brick wire format
        // stores the team frequency at packet bytes 9..11; tile
        // coordinates live on `ServerBrick.tile`. Read frequency once
        // here.
        let player_frequency = state
            .clients
            .get(&player_id)
            .map(|client| client.player.frequency)
            .unwrap_or(u16::MAX);
        let enemy_brick_tiles: Vec<(usize, usize)> = state
            .active_bricks
            .iter()
            .filter_map(|brick| {
                if brick.packet.len() < 11 {
                    return None;
                }
                let team = u16::from_le_bytes(brick.packet[9..11].try_into().ok()?);
                if team == player_frequency {
                    return None;
                }
                Some(
                    brick
                        .tiles
                        .iter()
                        .map(|&(tx, ty)| (tx as usize, ty as usize))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect();
        let sequence_advances = state.clients.get(&player_id).is_some_and(|sender| {
            client_position_sequence_advances(sender.last_position_sequence, client_sequence)
        });
        if !sequence_advances
            && snapshot.weapon != 0
            && consume_stale_weapon_mirror(
                state,
                player_id,
                snapshot,
                client_sequence,
                canonical_direction,
                server_tick_at_accept,
            )
        {
            return None;
        }
        let Some(current_sender) = state.clients.get(&player_id) else {
            return None;
        };
        if !current_sender.joined || current_sender.player.ship >= 8 {
            return None;
        }
        let mut combat_view_rejection = None;
        let projectile_combat_timeline = if snapshot.weapon != 0 {
            match bind_human_projectile_timeline(
                state,
                player_id,
                client_sequence,
                server_tick_at_accept,
            ) {
                Ok(binding) => Some(binding),
                Err(rejection) => {
                    debug!(
                        player_id,
                        client_sequence,
                        server_tick = server_tick_at_accept,
                        reason = rejection.reason(),
                        "weapon edge has no admissible server combat-view binding"
                    );
                    combat_view_rejection = Some(combat_view_block_cause(
                        state,
                        player_id,
                        client_sequence,
                        rejection,
                    ));
                    None
                }
            }
        } else {
            None
        };
        let combat_view_weapon_blocked =
            snapshot.weapon != 0 && projectile_combat_timeline.is_none();
        let objective_weapon_blocked = objective_weapon_substitutes_ball_fire(
            state,
            player_id,
            client_sequence,
            snapshot.weapon,
        );
        let weapon_block = WeaponBlock {
            bomb_safety: bomb_safety_blocked,
            mine_limit: mine_limit_blocked,
            objective_ball: objective_weapon_blocked,
            combat_view: combat_view_rejection,
        };
        let respawn_transition = !current_sender.alive;
        if respawn_transition && (canonical_input.bits != 0 || canonical_input.direction != 0) {
            debug!(
                player_id,
                client_sequence,
                input_bits = canonical_input.bits,
                input_direction = canonical_input.direction,
                "dropping non-neutral input at due respawn boundary"
            );
            return None;
        }
        if respawn_transition
            && current_sender
                .respawn_at_tick
                .is_some_and(|respawn_at_tick| {
                    !crate::continuum::continuum_tick_reached(
                        server_tick_at_accept,
                        respawn_at_tick,
                    )
                })
        {
            return None;
        }
        let respawn_input_records = respawn_transition.then(|| {
            crate::connection::rebase_input_door_phases_for_life_boundary(
                current_sender,
                client_sequence,
                server_tick_at_accept,
            )
            .expect("validated respawn inputs must resolve without dead-life history")
        });
        // A due respawn is a prospective life transition. Work on a complete
        // clone until every position, movement, weapon, and witness check has
        // passed; no rejected first report may make the dead player visible or
        // clear its timer/runtime/history.
        // Stage the complete runtime for every position, not only respawns.
        // This keeps weapon queue admission, energy/inventory/cooldown spend,
        // position publication, and projectile creation in one transaction:
        // a full dedicated-event queue can reject before any mutation reaches
        // the live client.
        let mut staged_sender = current_sender.clone();
        let sender = &mut staged_sender;
        if respawn_transition {
            sender.alive = true;
            sender.respawn_at_tick = None;
            sender.last_reported_energy = 0;
            let retained_rocket_assertions = respawn_input_records.as_ref().map(|records| {
                records
                    .iter()
                    .map(|record| {
                        let asserted = sender
                            .recent_input_rocket_assertions
                            .iter()
                            .find_map(|&(sequence, asserted)| {
                                (sequence == record.sequence).then_some(asserted)
                            })
                            .unwrap_or(false);
                        (record.sequence, asserted)
                    })
                    .collect::<Vec<_>>()
            });
            sender.reset_server_runtime_for_ship_at_tick(server_tick_at_accept);
            if let Some(input_records) = respawn_input_records {
                // Input ingress is an independent transaction and ran before
                // position admission. Establish the explicit new-life base
                // without erasing the neutral record or already-admitted
                // forward realtime records. Older records remain behind at
                // the committed life boundary.
                sender.life_position_sequence_base = Some(client_sequence);
                let newest = input_records
                    .last()
                    .copied()
                    .expect("respawn input records include boundary");
                let boundary = input_records
                    .iter()
                    .find(|record| record.sequence == client_sequence)
                    .copied()
                    .expect("respawn input records include exact boundary sequence");
                canonical_door_seed = boundary.door_seed;
                canonical_door_phase_tick = boundary.door_phase_tick;
                sender.last_input_seq = Some(newest.sequence);
                sender.last_input_record = Some(newest.input);
                sender.recent_input_records.extend(input_records);
                sender
                    .recent_input_rocket_assertions
                    .extend(retained_rocket_assertions.unwrap_or_default());
            }
            if !respawn_position_is_canonical(sender, map, snapshot) {
                debug!(
                    player_id,
                    "dropping lifecycle-desynchronized respawn position"
                );
                return None;
            }
            // The exact neutral spawn is safe provisional geometry for the
            // staged validation clone. It does not reach public visibility or
            // projectile hull state unless the complete admission commits.
            sender.last_position = Some(position.clone());
            sender.last_position_server_tick = None;
        }
        if !client_position_sequence_advances(sender.last_position_sequence, client_sequence) {
            if snapshot.weapon != 0 {
                log_security_reject(player_id, "position", "replayed_sequence");
            } else {
                // Movement uses the intentionally unordered, lifetime-bounded
                // DataChannel. A newer datagram can legitimately overtake an
                // older one; drop it without classifying normal transport
                // reordering as an attack. Weapon-bearing duplicates remain
                // security rejects because replaying them has side effects.
                debug!(
                    player_id,
                    client_sequence,
                    last_sequence = ?sender.last_position_sequence,
                    "dropping duplicate or reordered position"
                );
            }
            return None;
        }
        let movement_steps = sender
            .last_position_sequence
            .map_or(0, |previous| client_sequence.wrapping_sub(previous));
        let Some(movement_time_budget) = sender.movement_time_budget.admit(
            server_tick_at_accept,
            sender.last_position_server_tick,
            movement_steps,
        ) else {
            log_security_reject(player_id, "position", "movement_time_exceeded");
            return None;
        };
        // This remains prospective with every other position/weapon field.
        // Soft life changes never reset the session's accumulated time debt.
        sender.movement_time_budget = movement_time_budget;
        let repel_observation_match =
            matching_pending_repel_observations(sender, snapshot, server_tick_at_accept);
        let pending_repel_delivery = match repel_observation_match {
            Some(RepelObservationMatch::Pending(observation, application, delivery)) => {
                Some((observation, application, delivery))
            }
            _ => None,
        };
        let duplicate_repel_application = match repel_observation_match {
            Some(RepelObservationMatch::Duplicate(generation, application)) => {
                Some((generation, application))
            }
            _ => None,
        };
        if snapshot.repel_snapshot_id.is_some()
            && pending_repel_delivery.is_none()
            && duplicate_repel_application.is_none()
        {
            log_security_reject(player_id, "position", "invalid_repel_application_marker");
            return None;
        }
        if snapshot.repel_snapshot_id.is_some() && snapshot.warp_collectible_id.is_some() {
            // Current clients serialize discontinuities: an already-applied
            // Repel tail is admitted/receipted before a reliable Warp ACK is
            // applied. Accepting both markers would make their replay order
            // attacker-selected.
            log_security_reject(player_id, "position", "conflicting_application_markers");
            return None;
        }
        let repel_application = pending_repel_delivery
            .map(|(observation, application, delivery)| {
                Some(crate::sim_observation::CanonicalRepelApplication {
                    sequence: application.application_sequence,
                    origin_x: observation.origin_x,
                    origin_y: observation.origin_y,
                    speed: arena_repel_speed_pixels_per_second(),
                    radius: arena_repel_radius_pixels(),
                    expected_velocity_x: observation.expected_velocity_x,
                    expected_velocity_y: observation.expected_velocity_y,
                    delivered_snapshot_tick: delivery.snapshot_tick,
                    active_steps: application.active_steps,
                })
            })
            .flatten();
        let (spawn_x, spawn_y) = server_spawn_coordinates(map);
        let matching_warp_prize_authorization_index =
            snapshot.warp_collectible_id.and_then(|collectible_id| {
                sender
                    .pending_warp_prize_relocations
                    .iter()
                    .position(|authorization| {
                        authorization.life_seq == sender.life_seq
                            && authorization.collectible_id == collectible_id
                    })
            });
        let duplicate_warp_prize_marker =
            snapshot.warp_collectible_id.is_some_and(|collectible_id| {
                sender
                    .consumed_warp_prize_relocations
                    .contains(&collectible_id)
            });
        if snapshot.warp_collectible_id.is_some()
            && matching_warp_prize_authorization_index.is_none()
            && !duplicate_warp_prize_marker
        {
            log_security_reject(player_id, "position", "invalid_warp_prize_marker");
            return None;
        }
        // A negative Energy prize can authorize an over-cap value in its
        // exact private ACK, then the server's next natural recharge tick can
        // converge before the browser has received that newer state. Retain
        // the last admitted wire value as the bounded delivery-tail ceiling;
        // the first naturally converged report lowers it again. This never
        // lets a client grow energy beyond a previously authoritative value.
        let authorized_reported_energy = sender.server_energy.max(sender.last_reported_energy);
        if !reported_energy_within_limit(
            snapshot,
            sender.server_max_energy,
            authorized_reported_energy,
        ) {
            log_security_reject(player_id, "position", "impossible_energy");
            return None;
        }
        let door_seeds = [canonical_door_seed];
        let tile_blocks_position =
            |tile| position_blocking_tile_with_door_seed(tile, canonical_door_seed);
        if tile_blocks_position(tile_at_world(map, snapshot.x, snapshot.y)) {
            log_security_reject(player_id, "position", "blocked_position_center");
            return None;
        }
        if reported_position_hull_overlaps_blocked_tile(
            snapshot.x,
            snapshot.y,
            validated_position_collision_radius_pixels(sender.player.ship),
            MAP_TILES,
            TILE_SIZE_PX,
            |tile_x, tile_y| {
                map.tiles
                    .get(tile_y * MAP_TILES + tile_x)
                    .is_none_or(|&tile| tile_blocks_position(tile))
            },
        ) {
            log_security_reject(player_id, "position", "blocked_position_hull");
            return None;
        }
        let admission_speed_limit = if pending_repel_delivery.is_some() {
            arena_ship_max_speed_pixels_per_second(sender.player.ship)
                .max(arena_repel_speed_pixels_per_second())
        } else {
            arena_ship_max_speed_pixels_per_second(sender.player.ship)
        };
        if !reported_velocity_within_limit(
            snapshot,
            admission_speed_limit,
            POSITION_VELOCITY_ALLOWANCE_PX_PER_SECOND,
        ) {
            log_security_reject(player_id, "position", "impossible_velocity");
            return None;
        }
        let mut movement_upgrades = crate::sim_observation::movement_upgrades_for_client(sender);
        let existing_repel_active = sender.server_repel_until_tick.is_some_and(|until| {
            !crate::continuum::continuum_tick_reached(server_tick_at_accept, until)
        });
        // Pure prospective replay: each missing 60 Hz record is integrated
        // under its own canonical door phase. The resulting intermediate
        // witnesses are committed only after every admission check succeeds.
        let in_safe_tile = world_position_in_safe_tile_for_map(map, snapshot.x, snapshot.y);
        let (previewed_weapon, weapon_reject_reason) = if snapshot.weapon != 0 {
            let preview = preview_position_weapon(
                sender,
                snapshot,
                in_safe_tile,
                weapon_block,
                server_tick_at_accept,
                is_flagger,
            );
            (preview.accepted_weapon_data, preview.reject_reason)
        } else {
            (0, None)
        };
        let tail_bomb_recoil_speed = accepted_position_weapon_applies_recoil(previewed_weapon)
            .then(|| arena_ship_bomb_recoil_speed(sender.player.ship));
        let replay_with = |upgrades| {
            crate::sim_observation::replay_canonical_position_gap_with_repel_application(
                sender,
                map,
                is_flagger,
                attached_riders,
                existing_repel_active,
                upgrades,
                &enemy_brick_tiles,
                client_sequence,
                server_tick_at_accept,
                spawn_x,
                spawn_y,
                tail_bomb_recoil_speed,
                repel_application,
            )
        };
        let replay_matches = |replay: &crate::sim_observation::CanonicalPositionReplay| {
            let report = (
                snapshot.canonical_state,
                canonical_direction,
                snapshot.x,
                snapshot.y,
                snapshot.velocity_x,
                snapshot.velocity_y,
            );
            !replay
                .matching_exact_branch_witnesses(
                    report.0, report.1, report.2, report.3, report.4, report.5,
                )
                .is_empty()
                || replay
                    .matching_exact_relocated_branch_witnesses(
                        report.0, report.1, report.2, report.3, report.4, report.5,
                    )
                    .is_some()
                || replay
                    .matching_exact_wormhole_branch_witnesses(
                        report.0, report.1, report.2, report.3, report.4, report.5,
                    )
                    .is_some()
        };
        let mut canonical_replay = replay_with(movement_upgrades);
        // Input generated before the browser learned of a damage prize change
        // replays with the replaced tuning (`DamageMovementGrace`).
        if !canonical_replay.as_ref().is_some_and(replay_matches)
            && let Some(tuning) = sender.damage_movement_grace_tuning(client_sequence)
        {
            let grace_upgrades = crate::sim_observation::movement_upgrades_from_tuning(tuning);
            let grace_replay = replay_with(grace_upgrades);
            if grace_replay.as_ref().is_some_and(replay_matches) {
                canonical_replay = grace_replay;
                movement_upgrades = grace_upgrades;
            }
        }
        // Safe-zone weapon presses emit no live weapon. Their exact logical
        // record now carries a dedicated post-step stop bit, so gap replay can
        // reproduce the action even when that intermediate position is lost.
        let canonical_replay_matching_branches = canonical_replay
            .as_ref()
            .map(|replay| {
                replay.matching_exact_branch_witnesses(
                    snapshot.canonical_state,
                    canonical_direction,
                    snapshot.x,
                    snapshot.y,
                    snapshot.velocity_x,
                    snapshot.velocity_y,
                )
            })
            .unwrap_or_default();
        let canonical_replay_matching_relocated_witnesses =
            canonical_replay.as_ref().and_then(|replay| {
                replay.matching_exact_relocated_branch_witnesses(
                    snapshot.canonical_state,
                    canonical_direction,
                    snapshot.x,
                    snapshot.y,
                    snapshot.velocity_x,
                    snapshot.velocity_y,
                )
            });
        let canonical_replay_matching_wormhole_witnesses =
            canonical_replay.as_ref().and_then(|replay| {
                replay.matching_exact_wormhole_branch_witnesses(
                    snapshot.canonical_state,
                    canonical_direction,
                    snapshot.x,
                    snapshot.y,
                    snapshot.velocity_x,
                    snapshot.velocity_y,
                )
            });
        // Prefer an exact relocation branch over an ordinary branch when both
        // happen to quantize to the same tail. A wormhole terminal is the
        // strongest branch because it also owns the server energy drain and
        // projectile cleanup; a door relocation is next because it alone can
        // re-anchor a repeated door seed for the following position.
        let canonical_replay_matching_witnesses = canonical_replay_matching_wormhole_witnesses
            .or(canonical_replay_matching_relocated_witnesses)
            .or_else(|| canonical_replay_matching_branches.first().copied());
        let canonical_replay_matches_tail = canonical_replay_matching_witnesses.is_some();
        let unreceipted_repel_window = (!canonical_replay_matches_tail
            && repel_application.is_none()
            && sender.pending_movement_correction.is_none())
        .then(|| {
            repel_prediction::matching_offered_window(
                sender,
                map,
                snapshot,
                is_flagger,
                attached_riders,
                existing_repel_active,
                movement_upgrades,
                &enemy_brick_tiles,
                client_sequence,
                server_tick_at_accept,
                spawn_x,
                spawn_y,
                tail_bomb_recoil_speed,
            )
        })
        .flatten();
        // Authoritative damage, proximity safety or a cooldown race can
        // overtake a locally-predicted bomb. The browser already applied its
        // recoil while the server must reject the spend. Prove that *only*
        // the rejected recoil explains the raw-f32 report. The weapon stays rejected; the
        // ordinary reliable correction barrier later rebases movement to the
        // exact no-recoil witness.
        let rejected_weapon_recoil_matches_tail = (!canonical_replay_matches_tail
            && (matches!(
                weapon_reject_reason,
                Some(WeaponFireRejectReason::InsufficientEnergy | WeaponFireRejectReason::Cooldown)
            ) || (bomb_safety_blocked
                && weapon_reject_reason == Some(WeaponFireRejectReason::BlockedByArenaRule))
                || combat_view_weapon_blocked)
            && accepted_position_weapon_applies_recoil(snapshot.weapon))
        .then(|| {
            crate::sim_observation::replay_canonical_position_gap_with_repel_application(
                sender,
                map,
                is_flagger,
                attached_riders,
                existing_repel_active,
                movement_upgrades,
                &enemy_brick_tiles,
                client_sequence,
                server_tick_at_accept,
                spawn_x,
                spawn_y,
                Some(arena_ship_bomb_recoil_speed(sender.player.ship)),
                repel_application,
            )
        })
        .flatten()
        .is_some_and(|replay| {
            !replay
                .matching_exact_branch_witnesses(
                    snapshot.canonical_state,
                    canonical_direction,
                    snapshot.x,
                    snapshot.y,
                    snapshot.velocity_x,
                    snapshot.velocity_y,
                )
                .is_empty()
        });
        // A small unordered suffix can already be in flight when the first
        // rejected-recoil packet opens the reliable correction barrier.
        // Recognize only the exact continuation of that client lineage so it
        // is dropped as causally overtaken work rather than mislabeled as a
        // security violation. No suffix position or weapon is committed.
        let pending_prediction_lineage_matches_tail = (!canonical_replay_matches_tail)
            .then(|| {
                let transaction = sender
                    .pending_movement_correction
                    .as_ref()
                    .filter(|transaction| {
                        transaction.awaiting_application()
                            && matches!(transaction.proposal.reason,
                                crate::movement_correction::MovementCorrection::REASON_REJECTED_WEAPON_RECOIL
                                    | crate::movement_correction::MovementCorrection::REASON_REPEL_WINDOW)
                    })?;
                let proposal = transaction.proposal;
                let detected = transaction.detected_state;
                if client_sequence == proposal.position_sequence {
                    return (snapshot.canonical_state
                        == crate::continuum_position::ClientPositionCanonicalState::from_f32(
                            detected.position_x,
                            detected.position_y,
                            detected.velocity_x,
                            detected.velocity_y,
                            detected.orientation,
                        )
                        && snapshot.x
                            == detected.position_x.round().clamp(0.0, u16::MAX as f32) as u16
                        && snapshot.y
                            == detected.position_y.round().clamp(0.0, u16::MAX as f32) as u16
                        && snapshot.velocity_x
                            == crate::continuum_position::protocol_velocity_component(
                                detected.velocity_x,
                            )
                        && snapshot.velocity_y
                            == crate::continuum_position::protocol_velocity_component(
                                detected.velocity_y,
                            ))
                    .then_some(());
                }
                let record = sender
                    .recent_input_records
                    .iter()
                    .find(|record| record.sequence == proposal.position_sequence)?;
                let detected_witness = crate::state::CanonicalPositionWitness {
                    sequence: proposal.position_sequence,
                    life_seq: proposal.life_seq,
                    state: detected,
                    wire_x: detected.position_x.round().clamp(0.0, u16::MAX as f32) as u16,
                    wire_y: detected.position_y.round().clamp(0.0, u16::MAX as f32) as u16,
                    wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                        detected.velocity_x,
                    ),
                    wire_velocity_y: crate::continuum_position::protocol_velocity_component(
                        detected.velocity_y,
                    ),
                    direction: crate::continuum_position::protocol_direction(
                        detected.orientation,
                    ),
                    door_seed: record.door_seed,
                    door_phase_tick: record.door_phase_tick,
                };
                crate::sim_observation::replay_canonical_position_gap_from_witness_with_repel_window(
                    sender,
                    detected_witness,
                    transaction.replay_server_tick,
                    map,
                    is_flagger,
                    attached_riders,
                    existing_repel_active,
                    movement_upgrades,
                    &enemy_brick_tiles,
                    client_sequence,
                    server_tick_at_accept,
                    spawn_x,
                    spawn_y,
                    tail_bomb_recoil_speed,
                    repel_application,
                    transaction.predicted_repel_window.or(sender.server_repel_sequence_window),
                )
                .filter(|replay| {
                    !replay
                        .matching_exact_branch_witnesses(
                            snapshot.canonical_state,
                            canonical_direction,
                            snapshot.x,
                            snapshot.y,
                            snapshot.velocity_x,
                            snapshot.velocity_y,
                        )
                        .is_empty()
                })
                .map(|_| ())
            })
            .flatten()
            .is_some();
        let matching_repel_observation = pending_repel_delivery
            .map(|(observation, _, _)| observation)
            .filter(|_| canonical_replay_matches_tail);
        let canonical_replay_tail = canonical_replay_matching_witnesses
            .and_then(|witnesses| witnesses.last())
            .copied()
            .or_else(|| {
                canonical_replay
                    .as_ref()
                    .and_then(|replay| replay.witnesses.last())
                    .copied()
            });
        if let Some(replay) = canonical_replay
            .as_ref()
            .filter(|replay| replay.any_relocated() && !canonical_replay_matches_tail)
        {
            let previous_witness = sender.canonical_movement_witness.filter(|witness| {
                Some(witness.sequence) == sender.last_position_sequence
                    && witness.life_seq == sender.life_seq
            });
            debug!(
                player_id,
                life_seq = sender.life_seq,
                client_sequence,
                previous_witness = ?previous_witness,
                replay_tail = ?replay.witnesses.last(),
                reported_direction = canonical_direction,
                reported_x = snapshot.x,
                reported_y = snapshot.y,
                reported_velocity_x = snapshot.velocity_x,
                reported_velocity_y = snapshot.velocity_y,
                "canonical authenticated door-close replay missed the reported tail"
            );
        }
        // This exact replay is the sole door-relocation authority. The private
        // observer is diagnostics/correction state and cannot independently
        // waive a blocked path or consume the one-shot discontinuity reset.
        let matching_door_close_relocation =
            canonical_replay_matching_relocated_witnesses.is_some();
        let matching_wormhole_relocation = canonical_replay_matching_wormhole_witnesses.is_some();
        if matching_wormhole_relocation
            && (snapshot.weapon != 0
                || snapshot.repel_snapshot_id.is_some()
                || snapshot.warp_collectible_id.is_some())
        {
            // Browser wormhole handling returns before the action phase and
            // clears local weapons. A packet attempting to combine this exact
            // world boundary with another side effect is not an honest tail.
            log_security_reject(player_id, "position", "wormhole_with_side_effect");
            return None;
        }
        let reported_status_is_owned = sender
            .server_status
            .position_status_bits(snapshot.togglables, in_safe_tile)
            .bits()
            == snapshot.togglables;
        let (
            reported_exact_x,
            reported_exact_y,
            reported_exact_velocity_x,
            reported_exact_velocity_y,
            reported_exact_orientation,
        ) = snapshot.canonical_state.values();
        // Energy is independently stepped server authority and is stamped
        // onto the accepted relay below. In particular, a deferred Warp ACK
        // can be applied between different client/server recharge phases, so
        // its browser position need not repeat the server's current integer
        // energy. The global max-energy check above still rejects impossible
        // values; never make this non-authoritative wire mirror part of the
        // one-shot relocation proof.
        let matching_warp_prize_details = snapshot.warp_collectible_id.is_some()
            && snapshot.timestamp == 0
            && snapshot.direction == canonical_direction
            && snapshot.x == spawn_x
            && snapshot.y == spawn_y
            && snapshot.velocity_x == 0
            && snapshot.velocity_y == 0
            && snapshot.weapon == 0
            && snapshot.repel_snapshot_id.is_none()
            && snapshot.bounty == sender.server_bounty
            && reported_status_is_owned
            && reported_exact_x.to_bits() == (spawn_x as f32).to_bits()
            && reported_exact_y.to_bits() == (spawn_y as f32).to_bits()
            && reported_exact_velocity_x.to_bits() == 0.0f32.to_bits()
            && reported_exact_velocity_y.to_bits() == 0.0f32.to_bits()
            && canonical_replay_tail.is_some_and(|witness| {
                witness.state.orientation.to_bits() == reported_exact_orientation.to_bits()
            });
        let matching_warp_prize_discontinuity =
            matching_warp_prize_authorization_index.is_some() && matching_warp_prize_details;
        if matching_warp_prize_authorization_index.is_some() && !matching_warp_prize_details {
            debug!(
                player_id,
                snapshot_timestamp = snapshot.timestamp,
                snapshot_direction = snapshot.direction,
                canonical_direction,
                snapshot_velocity_x = snapshot.velocity_x,
                snapshot_velocity_y = snapshot.velocity_y,
                snapshot_weapon = snapshot.weapon,
                snapshot_repel_id = ?snapshot.repel_snapshot_id,
                snapshot_warp_collectible_id = ?snapshot.warp_collectible_id,
                snapshot_bounty = snapshot.bounty,
                server_bounty = sender.server_bounty,
                snapshot_energy = snapshot.energy,
                server_energy = sender.server_energy,
                pending_authorizations = ?sender.pending_warp_prize_relocations,
                snapshot_togglables = snapshot.togglables,
                reported_status_is_owned,
                "rejected noncanonical Warp-prize relocation details"
            );
            log_security_reject(player_id, "position", "noncanonical_warp_prize_relocation");
            return None;
        }
        // Warp and hostile repel replace server-authorized position/vector
        // fields, not orientation provenance. The ordinary authenticated
        // input replay still proves the exact raw-f32 orientation point
        // through the gap. Fail
        // closed if that proof is unavailable instead of reseeding from the
        // private observer or the center of the reported 40-bin direction.
        let authorized_discontinuity_orientation = (matching_warp_prize_discontinuity
            || matching_repel_observation.is_some())
        .then_some(canonical_replay_tail)
        .flatten()
        .filter(|witness| {
            witness.life_seq == sender.life_seq
                && witness.sequence == client_sequence
                && witness.direction == canonical_direction
                && witness.state.orientation.to_bits() == reported_exact_orientation.to_bits()
        });
        if (matching_warp_prize_discontinuity || matching_repel_observation.is_some())
            && authorized_discontinuity_orientation.is_none()
        {
            log_security_reject(
                player_id,
                "position",
                "missing_discontinuity_orientation_provenance",
            );
            return None;
        }
        let matching_movement_discontinuity = matching_warp_prize_discontinuity
            || matching_door_close_relocation
            || matching_wormhole_relocation
            || pending_ship_life_reset
            || respawn_transition;
        if !canonical_replay_matches_tail
            && !matching_warp_prize_discontinuity
            && matching_repel_observation.is_none()
            && !pending_ship_life_reset
            && !pending_frequency_baseline
            && !respawn_transition
        {
            if pending_prediction_lineage_matches_tail {
                debug!(
                    player_id,
                    life_seq = sender.life_seq,
                    client_sequence,
                    correction_generation = sender
                        .pending_movement_correction
                        .as_ref()
                        .map(|transaction| transaction.proposal.correction_generation),
                    "dropping exact in-flight prediction lineage behind correction barrier"
                );
                return None;
            }
            if (rejected_weapon_recoil_matches_tail || unreceipted_repel_window.is_some())
                && sender.pending_movement_correction.is_none()
                && let Some(authoritative_witness) = canonical_replay_tail
            {
                let detected_state = crate::state::ServerSimMovement {
                    position_x: reported_exact_x,
                    position_y: reported_exact_y,
                    velocity_x: reported_exact_velocity_x,
                    velocity_y: reported_exact_velocity_y,
                    orientation: reported_exact_orientation,
                    energy: sender.server_energy as f32,
                    max_energy: sender.server_max_energy as f32,
                };
                let mut transaction = prediction_correction_transaction(
                    sender,
                    player_id,
                    client_sequence,
                    server_tick_at_accept,
                    authoritative_witness,
                    detected_state,
                );
                if let Some(window) = unreceipted_repel_window {
                    transaction.proposal.reason =
                        crate::movement_correction::MovementCorrection::REASON_REPEL_WINDOW;
                    transaction.proposal.repel_window =
                        sender.server_repel_sequence_window.map(|(start, end)| {
                            (
                                start,
                                end,
                                sender
                                    .pending_repel_observations
                                    .front()
                                    .expect("proved pending Repel")
                                    .delivery_generation,
                            )
                        });
                    transaction.predicted_repel_window = Some(window);
                }
                tracing::warn!(
                    player_id,
                    life_seq = sender.life_seq,
                    client_sequence,
                    server_tick = server_tick_at_accept,
                    correction_generation = transaction.proposal.correction_generation,
                    reported_velocity_x = reported_exact_velocity_x,
                    reported_velocity_y = reported_exact_velocity_y,
                    authoritative_velocity_x = transaction.proposal.velocity_x,
                    authoritative_velocity_y = transaction.proposal.velocity_y,
                    reported_weapon = snapshot.weapon,
                    weapon_reject_reason = ?weapon_reject_reason,
                    bomb_safety_blocked,
                    combat_view_weapon_blocked,
                    reason = transaction.proposal.reason,
                    "server-owned prediction difference proved exactly; awaiting movement correction barrier"
                );
                prediction_correction = Some(transaction);
                break 'admission None;
            }
            debug!(
                player_id,
                client_sequence,
                life_seq = sender.life_seq,
                reported_direction = canonical_direction,
                reported_x_bits = reported_exact_x.to_bits(),
                reported_y_bits = reported_exact_y.to_bits(),
                reported_velocity_x_bits = reported_exact_velocity_x.to_bits(),
                reported_velocity_y_bits = reported_exact_velocity_y.to_bits(),
                reported_orientation_bits = reported_exact_orientation.to_bits(),
                canonical_replay_tail = ?canonical_replay_tail,
                prior_witness = ?sender.canonical_position_witnesses.back(),
                replay_records = ?sender.recent_input_records
                    .iter()
                    .filter(|record| {
                        let previous = sender
                            .last_position_sequence
                            .unwrap_or(client_sequence);
                        let offset = record.sequence.wrapping_sub(previous);
                        offset != 0
                            && offset <= client_sequence.wrapping_sub(previous)
                    })
                    .map(|record| (
                        record.sequence,
                        record.input.bits,
                        record.input.direction,
                        record.door_seed,
                        record.door_phase_tick,
                    ))
                    .collect::<Vec<_>>(),
                accepted_server_tick = server_tick_at_accept,
                "exact canonical position tail mismatch"
            );
            // The gameplay sender is staged and rolls back on rejection.
            // Diagnostic pacing must survive that rollback in the live handle.
            let detail_due = tracing::enabled!(tracing::Level::WARN)
                && state.clients.get_mut(&player_id).is_some_and(|current| {
                    current.session_id == sender.session_id
                        && current.life_seq == sender.life_seq
                        && current.canonical_mismatch_log_gate.admit(
                            sender.life_seq,
                            Instant::now(),
                            Duration::from_secs(10),
                        )
                });
            if detail_due {
                let reported = [
                    reported_exact_x.to_bits(),
                    reported_exact_y.to_bits(),
                    reported_exact_velocity_x.to_bits(),
                    reported_exact_velocity_y.to_bits(),
                    reported_exact_orientation.to_bits(),
                ];
                let expected = canonical_replay_tail.map(|tail| {
                    [
                        tail.state.position_x.to_bits(),
                        tail.state.position_y.to_bits(),
                        tail.state.velocity_x.to_bits(),
                        tail.state.velocity_y.to_bits(),
                        tail.state.orientation.to_bits(),
                    ]
                });
                let first_field = expected.map_or("missing_replay_tail", |expected| {
                    [
                        "position_x",
                        "position_y",
                        "velocity_x",
                        "velocity_y",
                        "orientation",
                    ]
                    .into_iter()
                    .zip(reported.into_iter().zip(expected))
                    .find_map(|(name, (reported, expected))| (reported != expected).then_some(name))
                    .unwrap_or("wire_or_identity")
                });
                tracing::warn!(
                    event = "canonical_mismatch_detail",
                    player_id,
                    session_id = sender.session_id.get(),
                    arena_stream_id = sender.arena_stream_id,
                    life_seq = sender.life_seq,
                    client_sequence,
                    server_tick = server_tick_at_accept,
                    first_differing_tail_field = first_field,
                    reported_weapon = snapshot.weapon,
                    previewed_weapon,
                    weapon_reject_reason = ?weapon_reject_reason,
                    bomb_safety_blocked,
                    combat_view_weapon_blocked,
                    reported_x_bits = reported[0],
                    reported_y_bits = reported[1],
                    reported_velocity_x_bits = reported[2],
                    reported_velocity_y_bits = reported[3],
                    reported_orientation_bits = reported[4],
                    reported_direction = canonical_direction,
                    expected_x_bits = expected.map(|v| v[0]),
                    expected_y_bits = expected.map(|v| v[1]),
                    expected_velocity_x_bits = expected.map(|v| v[2]),
                    expected_velocity_y_bits = expected.map(|v| v[3]),
                    expected_orientation_bits = expected.map(|v| v[4]),
                    expected_sequence = canonical_replay_tail.map(|tail| tail.sequence),
                    expected_life_seq = canonical_replay_tail.map(|tail| tail.life_seq),
                    expected_direction = canonical_replay_tail.map(|tail| tail.direction),
                    expected_door_seed = canonical_replay_tail.map(|tail| tail.door_seed),
                    expected_door_phase_tick =
                        canonical_replay_tail.map(|tail| tail.door_phase_tick),
                    last_admitted_sequence = sender.last_position_sequence,
                    last_input_sequence = sender.last_input_seq,
                    "first differing field in the rejected canonical tail; origin remains unclassified"
                );
            }
            // Count on the live handle; the staged sender is discarded.
            if let Some(current) = state.clients.get_mut(&player_id)
                && current.session_id == sender.session_id
            {
                current.session_summary.position_rejects += 1;
            }
            log_security_reject(player_id, "position", "canonical_state_mismatch");
            return None;
        }
        if let Some(last_server_tick) = sender.last_position_server_tick {
            if let Some((previous_x, previous_y)) = sender
                .last_position
                .as_deref()
                .and_then(large_position_coordinates)
            {
                let elapsed_ticks = crate::continuum::continuum_tick_elapsed(
                    server_tick_at_accept,
                    last_server_tick,
                );
                let replay_would_bounce = matching_repel_observation.is_none()
                    && canonical_replay.as_ref().is_some_and(|replay| {
                        replay.proves_exact_f32_bounce_report(
                            snapshot.canonical_state,
                            canonical_direction,
                            snapshot.x,
                            snapshot.y,
                            snapshot.velocity_x,
                            snapshot.velocity_y,
                        )
                    });
                let replay_proves_safe_zone_weapon_stop =
                    canonical_replay.as_ref().is_some_and(|replay| {
                        replay.proves_exact_f32_safe_zone_weapon_stop_report(
                            snapshot.canonical_state,
                            canonical_direction,
                            snapshot.x,
                            snapshot.y,
                            snapshot.velocity_x,
                            snapshot.velocity_y,
                        )
                    });
                if let Some((previous_velocity_x, previous_velocity_y)) = sender
                    .last_position
                    .as_deref()
                    .and_then(large_position_velocity)
                {
                    if matching_repel_observation.is_none()
                        && !matching_movement_discontinuity
                        && !replay_would_bounce
                        && !replay_proves_safe_zone_weapon_stop
                        && !reported_velocity_delta_within_limit(
                            previous_velocity_x,
                            previous_velocity_y,
                            snapshot,
                            elapsed_ticks,
                            arena_ship_max_thrust_pixels_per_second_squared(sender.player.ship),
                            POSITION_VELOCITY_DELTA_GRACE_PX_PER_SECOND,
                        )
                    {
                        // A velocity discontinuity is legal only when the exact
                        // authenticated replay above predicted its collision.
                        debug!(
                            player_id,
                            life_seq = sender.life_seq,
                            client_sequence,
                            previous_sequence = ?sender.last_position_sequence,
                            previous_witness = ?sender.canonical_movement_witness,
                            replay_bounced = canonical_replay.as_ref().is_some_and(|replay| replay.any_bounced()),
                            replay_tail = ?canonical_replay_tail,
                            reported_direction = canonical_direction,
                            reported_x = snapshot.x,
                            reported_y = snapshot.y,
                            reported_velocity_x = snapshot.velocity_x,
                            reported_velocity_y = snapshot.velocity_y,
                            elapsed_ticks,
                            "exact canonical replay did not authorize acceleration discontinuity"
                        );
                        log_security_reject(player_id, "position", "impossible_acceleration");
                        return None;
                    }
                }
                if !matching_movement_discontinuity
                    && !reported_position_delta_within_limit(
                        previous_x,
                        previous_y,
                        snapshot,
                        elapsed_ticks,
                        arena_ship_max_speed_pixels_per_second(sender.player.ship),
                        POSITION_DELTA_ALLOWANCE_PX_PER_SECOND,
                        POSITION_DELTA_GRACE_PX,
                    )
                {
                    log_security_reject(player_id, "position", "teleport");
                    return None;
                }
                if !matching_movement_discontinuity
                    && reported_position_path_hull_overlaps_blocked_tile(
                        previous_x,
                        previous_y,
                        snapshot.x,
                        snapshot.y,
                        validated_position_collision_radius_pixels(sender.player.ship),
                        MAP_TILES,
                        TILE_SIZE_PX,
                        |tile_x, tile_y| {
                            map.tiles
                                .get(tile_y * MAP_TILES + tile_x)
                                .is_none_or(|&tile| tile_blocks_position(tile))
                        },
                    )
                    && !canonical_replay_matches_tail
                {
                    log_security_reject(player_id, "position", "blocked_position_path");
                    return None;
                }
            }
        }
        // From this point onward the position cannot be rejected by movement,
        // hull, path, or witness admission. Consume discontinuity authority
        // transactionally now; prospective replay above remained read-only.
        if client_position_sequence_advances(sender.last_position_sequence, client_sequence) {
            sender.keep_weapon_window_near(client_sequence);
        }
        if snapshot.weapon != 0 && sender.weapon_sequence_seen(client_sequence) {
            debug!(
                player_id,
                client_sequence,
                latest_weapon_sequence = ?sender.latest_weapon_seq,
                "dropping position whose weapon sequence was already seen"
            );
            return None;
        }
        if matching_warp_prize_discontinuity {
            let matching_index = matching_warp_prize_authorization_index
                .expect("matching Warp relocation has an authorization");
            // ACK delivery is ordered. A report matching authorization N
            // proves the browser already applied every earlier ACK, even if
            // intermediate relocation datagrams were coalesced or lost.
            let mut authorization = None;
            for _ in 0..=matching_index {
                let consumed = sender.pending_warp_prize_relocations.pop_front();
                if let Some(consumed) = consumed {
                    sender
                        .consumed_warp_prize_relocations
                        .push_back(consumed.collectible_id);
                    while sender.consumed_warp_prize_relocations.len()
                        > crate::state::MAX_CONSUMED_WARP_PRIZE_RELOCATIONS
                    {
                        sender.consumed_warp_prize_relocations.pop_front();
                    }
                    authorization = Some(consumed);
                }
            }
            let authorization =
                authorization.expect("matching Warp authorization index must be present");
            sender.reset_server_movement_tracking_for_discontinuity();
            sender.position_history.clear();
            debug!(
                player_id,
                collectible_id = authorization.collectible_id,
                life_seq = authorization.life_seq,
                client_sequence,
                server_tick = server_tick_at_accept,
                x = snapshot.x,
                y = snapshot.y,
                "authoritative Warp prize relocation observed and movement simulation rebased"
            );
            warp_relocation_receipt = Some((
                authorization.collectible_id,
                authorization.life_seq,
                client_sequence,
            ));
        } else {
            sender
                .pending_warp_prize_relocations
                .retain(|authorization| authorization.life_seq == sender.life_seq);
            if matching_wormhole_relocation {
                let mut energy = EnergyPool::new(
                    sender.server_energy,
                    sender.server_max_energy,
                    sender.server_energy_remainder,
                );
                energy.retain_fraction(0.2, 1.0);
                sender.server_energy = energy.current();
                sender.server_energy_remainder = energy.recharge_remainder();
                // Publish it: the relocation is a server-side change to the client's energy that the client did not
                // cause and has no event for, so bumping the prize/state revision makes the next snapshot carry the
                // new current energy. Without this the client's prediction kept the pre-warp value until some
                // unrelated damage/prize/life event happened to resync it (measured: server 330,
                // client 911.25).
                sender.advance_server_prize_state_revision();
                sender.reset_server_movement_tracking_for_discontinuity();
                sender.position_history.clear();
                debug!(
                    player_id,
                    life_seq = sender.life_seq,
                    client_sequence,
                    server_tick = server_tick_at_accept,
                    x = snapshot.x,
                    y = snapshot.y,
                    energy = sender.server_energy,
                    "authoritative wormhole relocation observed and movement simulation rebased"
                );
                tracing::info!(
                    "zubspace_event {{\"type\":\"server_wormhole_relocation\",\"player_id\":{},\"life_seq\":{},\"client_sequence\":{},\"server_tick\":{},\"position_x\":{},\"position_y\":{},\"energy\":{}}}",
                    player_id,
                    sender.life_seq,
                    client_sequence,
                    server_tick_at_accept,
                    snapshot.x,
                    snapshot.y,
                    sender.server_energy,
                );
            } else if matching_door_close_relocation {
                sender.reset_server_movement_tracking_for_discontinuity();
                sender.position_history.clear();
                debug!(
                    player_id,
                    life_seq = sender.life_seq,
                    client_sequence,
                    server_tick = server_tick_at_accept,
                    x = snapshot.x,
                    y = snapshot.y,
                    "canonical door-close relocation observed and movement simulation rebased"
                );
            }
            if duplicate_warp_prize_marker {
                warp_relocation_receipt = snapshot
                    .warp_collectible_id
                    .map(|collectible_id| (collectible_id, sender.life_seq, client_sequence));
            }
        }
        // Continuous costs are advanced once from the prior accepted state at
        // the beginning of the server tick. Packet acceptance only chooses
        // the state that will be charged on the next tick; otherwise a
        // same-tick off/on packet pair can move the drain anchor for free.
        let mut status_bits =
            accepted_position_status_bits(sender, snapshot.togglables, in_safe_tile, 0);
        if snapshot.weapon != 0 {
            sender.remember_weapon_sequence(client_sequence);
        }
        let weapon_recipients = (previewed_weapon != 0).then(|| {
            crate::snapshot::watch_damage_recipient_identities_for_position(
                state,
                player_id,
                &[player_id],
                Some(&position),
            )
        });
        let weapon_event_reservation = if let Some(recipients) = weapon_recipients.as_deref() {
            let delivery_count = realtime_weapon_delivery_count(
                state,
                previewed_weapon,
                sender.player.frequency,
                snapshot.x,
                snapshot.y,
            );
            let recipient_groups =
                std::iter::repeat_n(recipients, delivery_count).collect::<Vec<_>>();
            let Some(reservation) = crate::snapshot::reserve_realtime_event_deliveries(
                state,
                &recipient_groups,
                crate::snapshot::RealtimeEventPressureDisposition::Deferred,
            ) else {
                debug!(
                    player_id,
                    client_sequence,
                    "deferring weapon position because dedicated realtime event capacity is full"
                );
                state
                    .weapon_admission_deferred
                    .insert(LifeIdentity::new(player_id, sender.life_seq));
                return None;
            };
            Some(reservation)
        } else {
            None
        };
        let accepted_weapon = accepted_position_weapon(
            sender,
            snapshot,
            in_safe_tile,
            weapon_block,
            server_tick_at_accept,
            is_flagger,
        );
        // The reference game's ShipController clears cloak on weapon use outside
        // safe tiles. Enforce this on the accepted shot even when an untrusted
        // client submits an owned cloak bit alongside it; stealth is separate.
        if accepted_weapon != 0 && !in_safe_tile && status_bits & POSITION_STATUS_CLOAK != 0 {
            status_bits = (status_bits & !POSITION_STATUS_CLOAK) | POSITION_STATUS_FLASH;
        }
        debug_assert!(
            previewed_weapon == 0 || accepted_weapon == previewed_weapon,
            "prospective and committed bomb admission must agree"
        );
        if snapshot.weapon != 0
            && accepted_weapon == 0
            && std::env::var_os("ZUBSPACE_SERVER_DEBUG_WEAPONS").is_some()
        {
            debug!(
                player_id,
                weapon = snapshot.weapon,
                server_tick = server_tick_at_accept,
                energy = sender.server_energy,
                max_energy = sender.server_max_energy,
                in_safe = in_safe_tile,
                bomb_safety = bomb_safety_blocked,
                mine_limit = mine_limit_blocked,
                last_weapon = ?sender.last_weapon_fire_tick,
                last_gun = ?sender.last_gun_fire_tick,
                last_bomb = ?sender.last_bomb_fire_tick,
                "server weapon rejected"
            );
        }
        let suppress_position_relay =
            accepted_weapon == 0 && !allow_position_relay_burst(sender, now);
        position[17..19].copy_from_slice(&sender.server_bounty.to_le_bytes());
        position[13] = status_bits;
        position[19..21].copy_from_slice(&accepted_weapon.to_le_bytes());
        position[21..23].copy_from_slice(&sender.server_energy.to_le_bytes());
        // Phase 1c-2/3: observe how far an authoritative simulation
        // step would land from the client's reported position. Reads
        // previous (px, py) before we overwrite `last_position` below.
        // Phase 1c-3a layered axis-swept wall collision on top so wall
        // bounces no longer dominate the deviation distribution.
        // Flagger/turret modifiers ensure a ship carrying flags or hosting
        // riders is simulated with its exact live tuning.
        let (prev_x, prev_y) = sender
            .last_position
            .as_deref()
            .and_then(large_position_coordinates)
            .map_or((None, None), |(x, y)| (Some(x), Some(y)));
        if let Some(repel) = matching_repel_observation {
            // A hostile repel is delivered independently of this victim's
            // unordered position stream. Consume exactly one event only when
            // exact replay reaches the marker's application sequence, applies
            // the velocity replacement after that movement tail, and matches
            // every raw reported bit. This cannot hide unrelated drift: no
            // event, wrong life/snapshot/sequence, expiry, or forged vector
            // follows the ordinary validation path.
            let accepted_application = pending_repel_delivery
                .map(|(_, application, _)| application)
                .expect("matching Repel has server-derived application");
            if let Some(observation) = sender.pending_repel_observations.front_mut() {
                observation.accepted_application = Some(accepted_application);
            }
            // Exact application now arms this server-owned window. Keep the
            // original event deadline: observing the replacement vector
            // later must not extend RepelTime or make loss/reorder timing
            // client-controlled.
            sender.server_repel_until_tick = Some(repel.expires_at_tick);
            sender.server_repel_sequence_window = repel_application.map(|application| {
                crate::sim_observation::canonical_repel_sequence_window(application)
            });
            sender.server_repel_observer_until_tick = Some(repel.expires_at_tick);
            sender.reset_server_movement_tracking_for_impulse();
            debug!(
                player_id,
                source_player_id = repel.source.player_id(),
                source_life_seq = repel.source.life.life_seq,
                event_tick = repel.event_tick,
                server_tick = server_tick_at_accept,
                application_sequence = accepted_application.application_sequence,
                accepted_end_sequence = accepted_application.accepted_end_sequence,
                delivered_snapshot_tick = ?repel_application
                    .map(|application| application.delivered_snapshot_tick),
                repel_sequence_window = ?sender.server_repel_sequence_window,
                repel_sequence_span = ?sender.server_repel_sequence_window
                    .map(|window| window.1.wrapping_sub(window.0)),
                origin_x = repel.origin_x,
                origin_y = repel.origin_y,
                expected_velocity_x = repel.expected_velocity_x,
                expected_velocity_y = repel.expected_velocity_y,
                reported_velocity_x = snapshot.velocity_x as f32 / 10.0,
                reported_velocity_y = snapshot.velocity_y as f32 / 10.0,
                "authoritative hostile repel observed and movement simulation rebased"
            );
            repel_application_receipt = Some((
                crate::wire::server_session_id(),
                sender.life_seq,
                repel.delivery_generation,
                snapshot.repel_snapshot_id.expect("matching Repel snapshot"),
                accepted_application.application_sequence,
                accepted_application.accepted_end_sequence,
                accepted_application.active_steps,
            ));
        } else if let Some((generation, application)) = duplicate_repel_application {
            repel_application_receipt = Some((
                crate::wire::server_session_id(),
                sender.life_seq,
                generation,
                application.snapshot_id,
                application.application_sequence,
                application.accepted_end_sequence,
                application.active_steps,
            ));
        }
        let previous_position_sequence = sender.last_position_sequence;
        let is_repel_active = sender
            .server_repel_observer_until_tick
            .is_some_and(|until| {
                !crate::continuum::continuum_tick_reached(server_tick_at_accept, until)
            });
        // Keep an independently validated corrected lineage current while
        // the reliable offer/application handshake is in flight. Each
        // accepted old-lineage tail supplies the same authenticated records
        // and live nonlinear environment used by ordinary admission. A gap
        // that cannot be replayed exactly converts the transaction to a
        // durable cancellation; application never guesses a translated tail.
        let pending_shadow = sender
            .pending_movement_correction
            .as_ref()
            .filter(|transaction| transaction.awaiting_application())
            .and_then(|transaction| {
                transaction
                    .replay_witness
                    .map(|witness| (witness, transaction.replay_server_tick))
            });
        if let Some((shadow_witness, shadow_tick)) = pending_shadow
            // An unordered position from at or before the correction witness
            // can still arrive after the packet that opened the transaction.
            // It belongs to the already-authenticated predecessor lineage and
            // cannot advance or cancel the corrected shadow. Only a strictly
            // forward, half-range-safe suffix has new inputs to replay.
            && client_sequence != shadow_witness.sequence
            && client_sequence.wrapping_sub(shadow_witness.sequence) < (1 << 15)
        {
            let replay = crate::sim_observation::replay_canonical_position_gap_from_witness(
                sender,
                shadow_witness,
                shadow_tick,
                map,
                is_flagger,
                attached_riders,
                is_repel_active,
                movement_upgrades,
                &enemy_brick_tiles,
                client_sequence,
                server_tick_at_accept,
                spawn_x,
                spawn_y,
                tail_bomb_recoil_speed,
                None,
            )
            .and_then(|replay| replay.witnesses.last().copied());
            let transaction = sender
                .pending_movement_correction
                .as_mut()
                .expect("validated pending correction");
            if let Some(witness) = replay {
                transaction.replay_witness = Some(witness);
                transaction.replay_server_tick = server_tick_at_accept;
            } else {
                transaction.cancel_before_application();
            }
        }
        if movement_snapback
            && movement_snapback_test_force_once
            && !sender.movement_correction_test_forced
            && sender.pending_movement_correction.is_none()
            && previous_position_sequence.is_some()
        {
            // Debug E2E fault injection: exercise the real threshold and
            // correction transaction exactly once without mutating browser
            // gameplay or raw packets. Release builds never read the env flag.
            sender.movement_drift_x = crate::sim_observation::MOVEMENT_DRIFT_SNAP_PX + 2.0;
            sender.movement_correction_test_forced = true;
        }
        let observation = crate::sim_observation::observe_simulation_for_accepted_position(
            sender,
            map,
            &door_seeds,
            is_flagger,
            attached_riders,
            is_repel_active,
            movement_upgrades,
            &enemy_brick_tiles,
            prev_x,
            prev_y,
            canonical_direction,
            snapshot.x,
            snapshot.y,
            snapshot.velocity_x,
            snapshot.velocity_y,
            Some(client_sequence),
            server_tick_at_accept,
            tail_bomb_recoil_speed,
            sim_observe,
        );
        // The private observer replays the same authenticated gap as canonical
        // admission. Keep the completed window through that replay so records
        // before its exclusive end retain RepelSpeed, then retire it before a
        // later accepted gap can reuse recurring u16 sequence values.
        retire_completed_repel_sequence_window(sender, previous_position_sequence, client_sequence);
        // Detection proposes one exact raw-f32 rebase but deliberately leaves
        // this accepted tail on the client's existing lineage. The client may
        // receive the reliable proposal many steps later; only its exact
        // application marker commits the proposal at that later tail.
        let proposed_movement_correction = observation
            .snap_back
            .filter(|correction| {
                movement_snapback
                    && sender.pending_movement_correction.is_none()
                    && correction.position_x.is_finite()
                    && correction.position_y.is_finite()
                    && correction.velocity_x.is_finite()
                    && correction.velocity_y.is_finite()
                    && correction.orientation.is_finite()
                    && (0.0..=u16::MAX as f32).contains(&correction.position_x)
                    && (0.0..=u16::MAX as f32).contains(&correction.position_y)
                    && crate::continuum_position::protocol_direction(correction.orientation)
                        == canonical_direction
            })
            .map(|mut correction| {
                // Movement correction can never promote the private
                // observer's energy integration into canonical authority.
                correction.energy = sender.server_energy as f32;
                correction.max_energy = sender.server_max_energy as f32;
                correction
            });
        if let Some(correction) = proposed_movement_correction {
            sender.next_movement_correction_generation =
                sender.next_movement_correction_generation.wrapping_add(1);
            if sender.next_movement_correction_generation == 0 {
                sender.next_movement_correction_generation = 1;
            }
            let proposal = crate::movement_correction::MovementCorrection {
                reason: crate::movement_correction::MovementCorrection::REASON_SUSTAINED_DRIFT,
                repel_window: None,
                server_session_id: crate::wire::server_session_id(),
                player_id,
                life_seq: sender.life_seq,
                correction_generation: sender.next_movement_correction_generation,
                position_sequence: client_sequence,
                server_tick: server_tick_at_accept,
                position_x: correction.position_x,
                position_y: correction.position_y,
                velocity_x: correction.velocity_x,
                velocity_y: correction.velocity_y,
                orientation: correction.orientation,
            };
            sender.pending_movement_correction = Some(crate::state::ServerMovementCorrection {
                proposal,
                detected_state: crate::state::ServerSimMovement {
                    position_x: reported_exact_x,
                    position_y: reported_exact_y,
                    velocity_x: reported_exact_velocity_x,
                    velocity_y: reported_exact_velocity_y,
                    orientation: reported_exact_orientation,
                    energy: sender.server_energy as f32,
                    max_energy: sender.server_max_energy as f32,
                },
                replay_witness: Some(crate::state::CanonicalPositionWitness {
                    sequence: client_sequence,
                    life_seq: sender.life_seq,
                    state: crate::state::ServerSimMovement {
                        position_x: proposal.position_x,
                        position_y: proposal.position_y,
                        velocity_x: proposal.velocity_x,
                        velocity_y: proposal.velocity_y,
                        orientation: proposal.orientation,
                        energy: sender.server_energy as f32,
                        max_energy: sender.server_max_energy as f32,
                    },
                    wire_x: proposal.position_x.round().clamp(0.0, u16::MAX as f32) as u16,
                    wire_y: proposal.position_y.round().clamp(0.0, u16::MAX as f32) as u16,
                    wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                        proposal.velocity_x,
                    ),
                    wire_velocity_y: crate::continuum_position::protocol_velocity_component(
                        proposal.velocity_y,
                    ),
                    direction: crate::continuum_position::protocol_direction(proposal.orientation),
                    door_seed: canonical_door_seed,
                    door_phase_tick: canonical_door_phase_tick,
                }),
                replay_server_tick: server_tick_at_accept,
                predicted_repel_window: None,
                offer_reliable_id: None,
                application_sequence: None,
                terminal_state: None,
                terminal_server_tick: None,
                cancelled: false,
                receipt_reliable_id: None,
            });
            movement_correction = Some(proposal);
            // Continue observing the old accepted lineage while the proposal
            // is in flight. Installing the proposed sim state here would make
            // every later honest report look incompatible before the client
            // had any opportunity to apply it.
            sender.reset_private_movement_observer();
            tracing::warn!(
                player_id,
                reported_x = snapshot.x,
                reported_y = snapshot.y,
                proposed_x = correction.position_x,
                proposed_y = correction.position_y,
                correction_generation = proposal.correction_generation,
                server_tick = server_tick_at_accept,
                "movement snapback proposed; awaiting exact client application barrier"
            );
        }
        // A neutral lifecycle request proves only readiness, never a
        // client-selected destination. Choose the actual spawn after all
        // admission checks, in this same staged transaction. The private
        // application receipt carries this committed position before the
        // browser can resume rolling input.
        let lifecycle_spawn = if respawn_transition || pending_ship_life_reset {
            let blocked_tiles = state
                .active_bricks
                .iter()
                .filter(|brick| {
                    !crate::continuum::continuum_tick_reached(
                        server_tick_at_accept,
                        brick.expires_at_tick,
                    ) && brick.packet.get(9..11).is_some_and(|bytes| {
                        u16::from_le_bytes(bytes.try_into().unwrap()) != sender.player.frequency
                    })
                })
                .flat_map(|brick| brick.tiles.iter().copied())
                .collect();
            let mut entropy = server_tick_at_accept
                .wrapping_add(u32::from(player_id))
                .wrapping_add(sender.life_seq)
                .to_le_bytes();
            if let Err(error) = getrandom::fill(&mut entropy) {
                tracing::warn!(event = "spawn_entropy_unavailable", player_id, %error);
            }
            let mut seed = i32::from_le_bytes(entropy);
            let settings_packet = map.settings_packet.as_ref();
            #[cfg(test)]
            let settings_packet = state
                .fixture_spawn_settings_packet
                .as_deref()
                .unwrap_or(settings_packet);
            let Some(point) = crate::zone::server_random_spawn_coordinates_from_settings(
                map,
                settings_packet,
                sender.player.frequency,
                state
                    .clients
                    .values()
                    .filter(|client| client.joined)
                    .count()
                    + state.bots.len(),
                arena_ship_radius_pixels(sender.player.ship),
                if state.config.free_for_all {
                    crate::zone::RandomWarpArea::AuthoredOrCentral
                } else {
                    crate::zone::RandomWarpArea::ArenaSettings
                },
                &mut seed,
                &blocked_tiles,
            ) else {
                *spawn_deferred = true;
                tracing::debug!(
                    event = "spawn_destination_unavailable",
                    player_id,
                    life_seq = sender.life_seq
                );
                return None;
            };
            position[4..6].copy_from_slice(&point.0.to_le_bytes());
            position[15..17].copy_from_slice(&point.1.to_le_bytes());
            position[13] = accepted_position_status_bits(
                sender,
                0,
                world_position_in_safe_tile_for_map(map, point.0, point.1),
                0,
            );
            Some(point)
        } else {
            None
        };
        let snapshot = if let Some((x, y)) = lifecycle_spawn {
            ClientPositionSnapshot {
                x,
                y,
                canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_f32(
                    f32::from(x),
                    f32::from(y),
                    0.0,
                    0.0,
                    0.0,
                ),
                ..snapshot
            }
        } else {
            snapshot
        };
        // The reset's neutral request was validated at the static map
        // anchor. Its committed motion history must contain only the new
        // server-selected point, never a path across the intervening map.
        let canonical_replay_matches_tail =
            canonical_replay_matches_tail && lifecycle_spawn.is_none();
        let canonical_replay_matching_witnesses =
            canonical_replay_matching_witnesses.filter(|_| lifecycle_spawn.is_none());
        // The server owns acceptance and stamps the packet with server-owned
        // bounty/status/weapon/energy/tick above, but it no longer mixes the
        // private simulation position into remote snapshots. Publishing one
        // validated motion source avoids small visible jumps on other clients.
        let (accepted_x, accepted_y) =
            large_position_coordinates(&position).unwrap_or((snapshot.x, snapshot.y));
        let (accepted_velocity_x, accepted_velocity_y) = large_position_velocity(&position)
            .unwrap_or((snapshot.velocity_x, snapshot.velocity_y));
        let shooter_ship = sender.player.ship;
        // Retire or monotonically narrow the exceptional negative-Energy
        // delivery-tail ceiling. Ordinary below-cap position reports never
        // manufacture a future allowance.
        if snapshot.energy <= sender.server_max_energy {
            sender.last_reported_energy = 0;
        } else if sender.last_reported_energy > 0 {
            sender.last_reported_energy = sender.last_reported_energy.min(snapshot.energy);
        }
        sender.last_position_sequence = Some(client_sequence);
        if pending_frequency_baseline {
            sender.pending_frequency_baseline = None;
        }
        sender.last_position_server_tick = Some(server_tick_at_accept);
        if let Some(witnesses) = canonical_replay_matching_witnesses {
            if matching_door_close_relocation {
                // A bounded receipt-time relocation branch can disambiguate a
                // repeated one-byte door seed that the unordered input lane
                // conservatively resolved to its earlier phase. Once the exact
                // spawn/raw-f32 report matches, advance those retained records
                // to the proven challenge phase as part of the same atomic
                // admission. Otherwise the next ordinary tail would see a
                // false phase backtrack and reopen the completed boundary.
                for witness in witnesses.iter().copied() {
                    if let Some(record) = sender
                        .recent_input_records
                        .iter_mut()
                        .find(|record| record.sequence == witness.sequence)
                    {
                        record.door_seed = witness.door_seed;
                        record.door_phase_tick = witness.door_phase_tick;
                    }
                }
            }
            for witness in witnesses
                .iter()
                .copied()
                .take_while(|witness| witness.sequence != client_sequence)
            {
                sender.canonical_position_witnesses.push_back(witness);
            }
        }
        let accepted_tail_witness = if canonical_replay_matches_tail {
            let mut witness = canonical_replay_tail
                .expect("an exact canonical replay match must have a tail witness");
            // Preserve the complete subpixel replay state. Energy is server
            // authority and may have changed independently of the movement
            // integration; wire coordinates/vectors remain exact, separate
            // evidence on the witness.
            witness.state.energy = sender.server_energy as f32;
            witness.state.max_energy = sender.server_max_energy as f32;
            witness
        } else {
            // Every non-replay tail admitted above is an exact server-owned
            // boundary or discontinuity (life/frequency reset, Warp, or
            // hostile repel). The versioned extension supplies the raw bits;
            // the corresponding admission branch proved those bits against
            // the server-known transformation. Never reconstruct or widen a
            // hidden state from the legacy integer wire cell.
            let (position_x, position_y, velocity_x, velocity_y, orientation) =
                snapshot.canonical_state.values();
            let mut witness = authorized_discontinuity_orientation.unwrap_or(
                crate::state::CanonicalPositionWitness {
                    sequence: client_sequence,
                    life_seq: sender.life_seq,
                    state: crate::state::ServerSimMovement {
                        position_x,
                        position_y,
                        orientation,
                        velocity_x,
                        velocity_y,
                        energy: sender.server_energy as f32,
                        max_energy: sender.server_max_energy as f32,
                    },
                    wire_x: accepted_x,
                    wire_y: accepted_y,
                    wire_velocity_x: accepted_velocity_x,
                    wire_velocity_y: accepted_velocity_y,
                    direction: canonical_direction,
                    door_seed: canonical_door_seed,
                    door_phase_tick: canonical_door_phase_tick,
                },
            );
            witness.state.position_x = position_x;
            witness.state.position_y = position_y;
            witness.state.velocity_x = velocity_x;
            witness.state.velocity_y = velocity_y;
            witness.state.orientation = orientation;
            witness.state.energy = sender.server_energy as f32;
            witness.state.max_energy = sender.server_max_energy as f32;
            witness.wire_x = accepted_x;
            witness.wire_y = accepted_y;
            witness.wire_velocity_x = accepted_velocity_x;
            witness.wire_velocity_y = accepted_velocity_y;
            witness.door_seed = canonical_door_seed;
            witness.door_phase_tick = canonical_door_phase_tick;
            witness
        };
        if lifecycle_spawn.is_some() {
            sender.reset_private_movement_observer();
            sender.server_sim_state = Some(accepted_tail_witness.state);
            sender.server_sim_anchor_tick = Some(server_tick_at_accept);
            sender.server_sim_input_bits = Some(0);
        }
        sender
            .canonical_position_witnesses
            .push_back(accepted_tail_witness);
        sender.canonical_movement_witness = Some(accepted_tail_witness);
        while sender.canonical_position_witnesses.len()
            > crate::state::MAX_RECENT_INPUT_OBSERVATIONS
        {
            sender.canonical_position_witnesses.pop_front();
        }
        // Phase 2e: append this accepted position to the
        // lag-compensation history. The server tick is the only
        // timestamp used for movement/weapon authority.
        sender
            .position_history
            .push_back(crate::state::RecentPositionSample {
                tick: server_tick_at_accept,
                x: accepted_x as f32,
                y: accepted_y as f32,
            });
        if sender.position_history.len() > crate::state::MAX_POSITION_HISTORY {
            sender.position_history.pop_front();
        }
        let motion_append = if respawn_transition
            || pending_ship_life_reset
            || pending_frequency_baseline
            || matching_warp_prize_discontinuity
            || matching_wormhole_relocation
            || matching_door_close_relocation
            || matching_repel_observation.is_some()
            || !canonical_replay_matches_tail
        {
            sender.append_actor_motion_witness(server_tick_at_accept, accepted_tail_witness)
        } else {
            let mut steps: Vec<_> = canonical_replay_matching_witnesses
                .expect("matched replay has witnesses")
                .iter()
                .copied()
                .take_while(|w| w.sequence != client_sequence)
                .collect();
            steps.push(accepted_tail_witness);
            sender.append_actor_motion_replay(server_tick_at_accept, &steps)
        };
        if let Err(reason) = motion_append {
            tracing::warn!(
                player_id,
                client_sequence,
                ?reason,
                "validated motion history transaction rejected"
            );
            return None;
        }
        // A validated batch may enter and leave safety between server ticks.
        // Select only newly admitted samples from this exact life, not an old
        // retained baseline or a straight line across a teleport discontinuity.
        let safe_entry_sequence = canonical_replay_matching_witnesses
            .into_iter()
            .flat_map(|witnesses| witnesses.iter())
            .chain(std::iter::once(&accepted_tail_witness))
            .filter(|witness| {
                witness.life_seq == sender.life_seq
                    && previous_position_sequence.is_none_or(|previous| {
                        let delta = witness.sequence.wrapping_sub(previous);
                        delta > 0 && delta <= client_sequence.wrapping_sub(previous)
                    })
                    && crate::zone::world_position_in_safe_tile_for_map(
                        map,
                        witness.wire_x,
                        witness.wire_y,
                    )
            })
            .map(|witness| witness.sequence)
            .last();
        let ordinary_input = !respawn_transition
            && !pending_ship_life_reset
            && !pending_frequency_baseline
            && previous_position_sequence.is_some();
        // Movement intent and accepted firing count; drift, status toggles,
        // automatic network updates and holding Shift alone do not.
        let activity = canonical_input.bits & 0x0f != 0
            || accepted_weapon != 0
            || previous_position_sequence.is_some_and(|_| {
                state
                    .clients
                    .get(&player_id)
                    .and_then(|client| client.canonical_movement_witness)
                    .is_some_and(|witness| witness.direction != canonical_direction)
            });
        staged_sender.spawn_idle.accepted_input(
            staged_sender.arena_stream_id,
            staged_sender.life_seq,
            now,
            ordinary_input,
            activity,
        );
        state.clients.insert(player_id, staged_sender);
        if let Some(sequence) = safe_entry_sequence {
            let owner = state
                .active_player_runtime_identity(player_id)
                .expect("committed position runtime");
            crate::server_tick::cancel_owner_weapons_for_admitted_safe_entry(
                state,
                server_tick_at_accept,
                owner,
                Some(sequence),
            );
        }
        state.set_client_position(player_id, position.clone());
        if matching_wormhole_relocation {
            // The reference game clears the complete visible weapon set at the local
            // wormhole terminal. Retire this authoritative actor's projectiles
            // and any now-obsolete collision transactions atomically with the
            // accepted relocation so no pre-warp hit crosses the boundary.
            state.remove_projectiles_for_player(player_id);
        }
        if respawn_transition {
            let committed = state
                .clients
                .get(&player_id)
                .expect("committed respawn sender must remain present");
            info!(
                server_tick = server_tick_at_accept,
                player_id,
                name = %committed.player.name,
                life_seq = committed.life_seq,
                ship = committed.player.ship,
                frequency = committed.player.frequency,
                x = snapshot.x,
                y = snapshot.y,
                energy = committed.server_energy,
                max_energy = committed.server_max_energy,
                input_sequence = client_sequence,
                input_bits = committed.last_input_record.map(|input| input.bits),
                input_direction = committed.last_input_record.map(|input| input.direction),
                input_history_len = committed.recent_input_records.len(),
                "server player respawned"
            );
            debug!(
                "zubspace_event {{\"type\":\"server_player_respawned\",\"server_tick\":{},\"player_id\":{},\"life_seq\":{},\"ship\":{},\"frequency\":{},\"x\":{},\"y\":{},\"energy\":{},\"max_energy\":{},\"input_sequence\":{},\"input_bits\":{},\"input_direction\":{},\"input_history_len\":{}}}",
                server_tick_at_accept,
                player_id,
                committed.life_seq,
                committed.player.ship,
                committed.player.frequency,
                snapshot.x,
                snapshot.y,
                committed.server_energy,
                committed.server_max_energy,
                client_sequence,
                committed
                    .last_input_record
                    .map_or(u16::MAX, |input| input.bits),
                committed
                    .last_input_record
                    .map_or(u8::MAX, |input| input.direction),
                committed.recent_input_records.len(),
            );
        }
        if sim_observe {
            let committed = state
                .clients
                .get(&player_id)
                .expect("committed sender must remain present");
            tracing::info!(
                server_tick = server_tick_at_accept,
                player_id,
                life_seq = identity.life_seq,
                input_sequence = client_sequence,
                input_bits = canonical_input.bits,
                input_direction = canonical_input.direction,
                reported_weapon = snapshot.weapon,
                accepted_weapon,
                x = accepted_x,
                y = accepted_y,
                velocity_x = accepted_velocity_x,
                velocity_y = accepted_velocity_y,
                energy = committed.server_energy,
                max_energy = committed.server_max_energy,
                recharge_per_second = committed.server_recharge_per_second,
                reliable_lifecycle_baseline =
                    respawn_transition || pending_ship_life_reset || pending_frequency_baseline,
                "human authority input trace"
            );
        }
        if accepted_weapon != 0 {
            record_accepted_projectile(
                state,
                player_id,
                projectile_combat_timeline
                    .expect("accepted human weapon must own an exact combat timeline"),
                accepted_weapon,
                shooter_ship,
                canonical_direction,
                accepted_x,
                accepted_y,
                accepted_velocity_x,
                accepted_velocity_y,
                accepted_tail_witness.state,
                server_tick_at_accept,
                weapon_event_reservation
                    .expect("accepted weapon must own dedicated event reservation"),
                weapon_recipients.expect("accepted weapon must retain exact recipients"),
            );
        }
        // Phase 4 legacy retirement: the snapshot stream carries
        // every per-entity field that LARGE_POSITION used to broadcast,
        // so the legacy fan-out is permanently no-op. The server's
        // `last_position` + projectile bookkeeping above continues
        // to run; `send_position_relays_batched` becomes a no-op.
        // Tests that need rx-channel observability call
        // `broadcast_position_observable` which explicitly recomputes
        // recipients via `visible_position_recipient_ids`.
        Some((Vec::new(), suppress_position_relay))
    };
    if let Some(transaction) = prediction_correction {
        if let Some(sender) = state.clients.get_mut(&player_id)
            && sender.life_seq == identity.life_seq
            && sender.pending_movement_correction.is_none()
        {
            sender.next_movement_correction_generation = transaction.proposal.correction_generation;
            sender.pending_movement_correction = Some(transaction);
        }
        return None;
    }
    let (recipient_ids, public_relay_suppressed) = admission?;
    Some(AcceptedPositionRelay {
        position,
        recipient_ids,
        public_relay_suppressed,
        warp_relocation_receipt,
        repel_application_receipt,
        movement_correction,
    })
}

/// `None` means this is not the first position of a boundary-scoped ship
/// life. `Some(true)` is the exact neutral spawn/reset state emitted after
/// the reliable roster acknowledgement; `Some(false)` is never a legal
/// baseline and must fail before input or gameplay mutation.
pub(crate) fn pending_ship_life_reset_position_is_canonical(
    state: &ServerState,
    player_id: u16,
    snapshot: ClientPositionSnapshot,
) -> Option<bool> {
    let client = state.clients.get(&player_id)?;
    if client.pending_frequency_baseline.is_some()
        || client.life_position_sequence_base.is_none()
        || client.last_position_sequence.is_some()
    {
        return None;
    }
    let (spawn_x, spawn_y) = server_spawn_coordinates(state.zone_map());
    let matches = snapshot.timestamp == 0
        && snapshot.direction == 0
        && snapshot.x == spawn_x
        && snapshot.y == spawn_y
        && snapshot.velocity_x == 0
        && snapshot.velocity_y == 0
        && snapshot.togglables == 0
        && snapshot.weapon == 0
        && snapshot.bounty == client.server_bounty
        && snapshot.energy == client.server_energy
        && snapshot.repel_snapshot_id.is_none()
        && canonical_state_matches_exact(snapshot, spawn_x as f32, spawn_y as f32, 0.0, 0.0, 0.0);
    Some(matches)
}

/// A dead player's first due position is the respawn transaction itself, not
/// ordinary client-owned movement. Require the exact neutral canonical spawn
/// produced by the browser after its reliable lifecycle state catches up.
fn respawn_position_is_canonical(
    sender: &ClientHandle,
    map: &ZoneMap,
    snapshot: ClientPositionSnapshot,
) -> bool {
    let (spawn_x, spawn_y) = server_spawn_coordinates(map);
    snapshot.timestamp == 0
        && snapshot.direction == 0
        && snapshot.x == spawn_x
        && snapshot.y == spawn_y
        && snapshot.velocity_x == 0
        && snapshot.velocity_y == 0
        && snapshot.togglables == 0
        && snapshot.weapon == 0
        && snapshot.bounty == sender.server_bounty
        && snapshot.energy == sender.server_energy
        && snapshot.repel_snapshot_id.is_none()
        && canonical_state_matches_exact(snapshot, spawn_x as f32, spawn_y as f32, 0.0, 0.0, 0.0)
}

fn canonical_state_matches_exact(
    snapshot: ClientPositionSnapshot,
    position_x: f32,
    position_y: f32,
    velocity_x: f32,
    velocity_y: f32,
    orientation: f32,
) -> bool {
    snapshot.canonical_state
        == crate::continuum_position::ClientPositionCanonicalState::from_f32(
            position_x,
            position_y,
            velocity_x,
            velocity_y,
            orientation,
        )
}

/// `None` means no frequency cutover is awaiting its exact baseline. The
/// expected state was captured from a fully validated pre-cutover position;
/// every physical/authority field must match before new-frequency input or a
/// weapon can mutate server state.
pub(crate) fn pending_frequency_baseline_is_canonical(
    state: &ServerState,
    player_id: u16,
    snapshot: ClientPositionSnapshot,
) -> Option<bool> {
    let client = state.clients.get(&player_id)?;
    let expected = client.pending_frequency_baseline?;
    if client.last_position_sequence.is_some() {
        #[cfg(test)]
        return Some(snapshot == expected);
        #[cfg(not(test))]
        return None;
    }
    Some(snapshot == expected)
}

fn validated_position_collision_radius_pixels(ship: u8) -> f32 {
    (arena_ship_radius_pixels(ship) - POSITION_COLLISION_RADIUS_GRACE_PX).max(0.0)
}

pub(crate) fn client_position_sequence_advances(previous: Option<u16>, sequence: u16) -> bool {
    previous.map_or(true, |last| {
        sequence != last && sequence.wrapping_sub(last) < (1 << 15)
    })
}

/// Retire exact Repel movement authority only after a successfully admitted
/// position transaction has replayed across its exclusive logical end. Clear
/// the old wall-clock fallbacks with it so the next gap cannot regain
/// RepelSpeed, including when the u16 sequence values recur one epoch later.
pub(crate) fn retire_completed_repel_sequence_window(
    sender: &mut ClientHandle,
    previous_sequence: Option<u16>,
    accepted_sequence: u16,
) -> bool {
    let Some((window, previous_sequence)) =
        sender.server_repel_sequence_window.zip(previous_sequence)
    else {
        return false;
    };
    if !crate::gameplay_rules::repel_sequence_window_end_crossed(
        window,
        previous_sequence,
        accepted_sequence,
    ) {
        return false;
    }
    sender.server_repel_sequence_window = None;
    sender.server_repel_until_tick = None;
    sender.server_repel_observer_until_tick = None;
    true
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RepelObservationMatch {
    Pending(
        ServerRepelObservation,
        crate::state::ServerRepelAcceptedApplication,
        crate::state::ServerRepelDelivery,
    ),
    Duplicate(u32, crate::state::ServerRepelAcceptedApplication),
}

fn matching_pending_repel_observations(
    sender: &mut ClientHandle,
    snapshot: ClientPositionSnapshot,
    server_tick: u32,
) -> Option<RepelObservationMatch> {
    while sender
        .pending_repel_observations
        .front()
        .is_some_and(|event| {
            event.awaiting_application()
                && crate::continuum::continuum_tick_reached(server_tick, event.expires_at_tick)
        })
    {
        sender.pending_repel_observations.pop_front();
    }
    crate::reliable::release_deferred_warp_prize_acks_if_ready(sender);
    let Some(application_sequence) = snapshot.repel_application_sequence else {
        if let Some(observation) = sender.pending_repel_observations.front() {
            debug!(
                player_id = sender.player.id,
                tail_sequence = ?snapshot.client_sequence,
                delivered_snapshot_id = ?observation.delivered_snapshot_id,
                delivery_generation = observation.delivery_generation,
                "pending hostile Repel position omitted exact application marker"
            );
        }
        return None;
    };
    let delivery_generation = snapshot.repel_delivery_generation?;
    if let Some(retired) = sender
        .retired_repel_applications
        .iter()
        .find(|retired| retired.delivery_generation == delivery_generation)
        .copied()
    {
        let application = retired.application;
        return (snapshot.repel_snapshot_id == Some(application.snapshot_id)
            && application_sequence == application.application_sequence)
            .then_some(RepelObservationMatch::Duplicate(
                retired.delivery_generation,
                application,
            ));
    }
    let observation = sender.pending_repel_observations.front().copied()?;
    if delivery_generation != observation.delivery_generation {
        return None;
    }
    if let Some(application) = observation.accepted_application {
        let duplicate_snapshot_id = snapshot.repel_snapshot_id?;
        let was_delivered = observation
            .delivered_snapshots
            .iter()
            .flatten()
            .any(|delivery| delivery.snapshot_id == duplicate_snapshot_id);
        return (was_delivered && application_sequence == application.application_sequence)
            .then_some(RepelObservationMatch::Duplicate(
                observation.delivery_generation,
                crate::state::ServerRepelAcceptedApplication {
                    snapshot_id: duplicate_snapshot_id,
                    application_sequence,
                    accepted_end_sequence: application.accepted_end_sequence,
                    active_steps: application.active_steps,
                    receipt_reliable_ids: application.receipt_reliable_ids,
                    receipt_reliable_cursor: application.receipt_reliable_cursor,
                },
            ));
    }
    let delivery = observation
        .delivered_snapshots
        .iter()
        .flatten()
        .find(|delivery| Some(delivery.snapshot_id) == snapshot.repel_snapshot_id)
        .copied()?;
    let Some(previous_sequence) = sender.last_position_sequence else {
        return None;
    };
    let application_distance = application_sequence.wrapping_sub(previous_sequence);
    let Some(tail_sequence) = snapshot.client_sequence else {
        return None;
    };
    let tail_distance = tail_sequence.wrapping_sub(application_sequence);
    if application_distance == 0 || application_distance >= (1 << 15) || tail_distance >= (1 << 15)
    {
        debug!(
            player_id = sender.player.id,
            previous_sequence,
            tail_sequence,
            application_sequence,
            application_distance,
            tail_distance,
            "pending hostile Repel position carried an out-of-window application sequence"
        );
        return None;
    }
    let tail_distance = u32::from(tail_distance);
    let offered_steps = u32::from(delivery.active_steps);
    if tail_distance > offered_steps {
        debug!(
            player_id = sender.player.id,
            application_sequence,
            tail_sequence,
            tail_distance,
            offered_steps,
            "pending hostile Repel marker tail exceeded its immutable offer"
        );
        return None;
    }
    // Client time is never an authority input. The marker must arrive while
    // the event is still live, then the server narrows the logical suffix to
    // the smaller of (a) the immutable delivery offer and (b) already-observed
    // tail steps plus the server-owned wall-clock remainder. This preserves
    // honest in-flight steps without allowing a claimed old tick to restart
    // the full interval.
    let remaining_ticks =
        crate::continuum::continuum_tick_diff(observation.expires_at_tick, server_tick);
    if remaining_ticks <= 0 {
        return None;
    }
    let future_steps =
        crate::gameplay_rules::repel_active_steps_for_server_ticks(remaining_ticks as u32);
    let accepted_steps_u32 = offered_steps.min(tail_distance.saturating_add(future_steps));
    let active_steps = u16::try_from(accepted_steps_u32).ok()?;
    if active_steps == 0 || tail_distance > accepted_steps_u32 {
        return None;
    }
    let application = crate::state::ServerRepelAcceptedApplication {
        snapshot_id: delivery.snapshot_id,
        application_sequence,
        accepted_end_sequence: application_sequence.wrapping_add(active_steps),
        active_steps,
        receipt_reliable_ids: [None; crate::state::MAX_REPEL_RECEIPT_COPIES],
        receipt_reliable_cursor: 0,
    };
    Some(RepelObservationMatch::Pending(
        observation,
        application,
        delivery,
    ))
}

fn allocate_repel_delivery_generation(
    next_generation: &mut u32,
    occupied: &mut std::collections::HashSet<u32>,
) -> u32 {
    loop {
        *next_generation = next_generation.wrapping_add(1).max(1);
        if occupied.insert(*next_generation) {
            return *next_generation;
        }
    }
}

fn repel_projectile_owner_state(
    state: &ServerState,
) -> std::collections::HashMap<crate::authority_identity::LifeIdentity, (u16, u8)> {
    let mut projectile_owners = state
        .clients
        .iter()
        .filter_map(|(&player_id, client)| {
            (client.joined && client.alive && client.player.ship < 8).then_some((
                crate::authority_identity::LifeIdentity::new(player_id, client.life_seq),
                (
                    client.player.frequency,
                    crate::arena_settings::arena_ship_bomb_bounce_count(client.player.ship),
                ),
            ))
        })
        .collect::<std::collections::HashMap<_, _>>();
    projectile_owners.extend(state.bots.iter().filter(|bot| bot.alive()).map(|bot| {
        (
            bot.life_identity(),
            (
                bot.player.frequency,
                crate::arena_settings::arena_ship_bomb_bounce_count(bot.player.ship),
            ),
        )
    }));
    // Committed flight outlives owner death, disconnect and slot reuse. The
    // immutable carrier supplies the firing ship and frequency for that life.
    for projectile in state.active_projectiles.iter() {
        if let Some(source) = projectile.captured_source() {
            projectile_owners.insert(
                projectile.owner_identity(),
                (
                    source.frequency,
                    crate::arena_settings::arena_ship_bomb_bounce_count(source.ship),
                ),
            );
        }
    }
    projectile_owners
}

fn preview_repel_public_mutation_count(
    state: &ServerState,
    source_frequency: u16,
    origin_x: u16,
    origin_y: u16,
) -> usize {
    let origin = crate::gameplay_rules::RulesVector::new(origin_x as f32, origin_y as f32);
    let repel_radius = arena_repel_radius_pixels();
    let projectile_owners = repel_projectile_owner_state(state);
    let projectiles = state.active_projectiles.preview_repel_mutation_count(
        origin,
        source_frequency,
        repel_radius,
        &projectile_owners,
    );
    let decoys = state
        .active_decoys
        .iter()
        .filter(|decoy| {
            decoy.frequency != source_frequency
                && crate::gameplay_rules::repel_velocity_after(
                    origin,
                    crate::gameplay_rules::RulesVector::new(decoy.position_x, decoy.position_y),
                    1.0,
                    repel_radius,
                )
                .is_some()
        })
        .count();
    projectiles.saturating_add(decoys)
}

fn realtime_weapon_delivery_count(
    state: &ServerState,
    weapon_data: u16,
    source_frequency: u16,
    origin_x: u16,
    origin_y: u16,
) -> usize {
    if crate::continuum_weapon::continuum_weapon_type(weapon_data) != 5 {
        return 1;
    }
    1 + preview_repel_public_mutation_count(state, source_frequency, origin_x, origin_y)
        .div_ceil(crate::realtime_event::MAX_REPEL_MUTATIONS_PER_EVENT)
}

fn register_authoritative_repel_observations(
    state: &mut ServerState,
    source_player_id: u16,
    weapon_data: u16,
    origin_x: u16,
    origin_y: u16,
    server_tick: u32,
) -> Vec<crate::realtime_event::RealtimeRepelMutationState> {
    let Some(source) = state.active_player_runtime_identity(source_player_id) else {
        return Vec::new();
    };
    let Some((source_frequency, source_ship)) =
        state.clients.get(&source_player_id).and_then(|client| {
            crate::snapshot::extract_near_entity_state(client.last_position.as_deref()?)?;
            Some((client.player.frequency, client.player.ship))
        })
    else {
        return Vec::new();
    };
    let map = state.zone_map();
    let origin = crate::gameplay_rules::RulesVector::new(origin_x as f32, origin_y as f32);
    let repel_speed = arena_repel_speed_pixels_per_second();
    let repel_radius = arena_repel_radius_pixels();
    let expires_at_tick =
        crate::continuum::continuum_tick_add(server_tick, arena_repel_lifetime_ticks());
    let active_window =
        crate::gameplay_rules::repel_sequence_window(0, arena_repel_lifetime_ticks());
    let active_steps = active_window
        .1
        .wrapping_sub(active_window.0)
        .saturating_sub(1);
    let projectile_owners = repel_projectile_owner_state(state);
    let projectile_mutation = state.active_projectiles.apply_repel(
        origin,
        source_frequency,
        repel_speed,
        repel_radius,
        server_tick,
        &projectile_owners,
        crate::arena_settings::arena_weapon_lifetime_ticks,
    );
    let mut public_mutations = projectile_mutation
        .states
        .iter()
        .filter_map(|mutation| {
            let source_session_id = if mutation.source_session_id != 0 {
                mutation.source_session_id
            } else {
                // No production carrier is rebound through a live roster slot.
                // This fallback supports isolated pre-carrier native fixtures.
                state
                    .active_actor_runtime_identity(mutation.owner_id)
                    .filter(|owner| owner.life.life_seq == mutation.owner_life_seq)?
                    .session
                    .session_id
                    .get()
            };
            Some(crate::realtime_event::RealtimeRepelMutationState {
                motion_frozen: mutation.motion_frozen,
                entity_kind: crate::realtime_event::REPEL_MUTATION_PROJECTILE,
                source_player_id: mutation.owner_id,
                source_session_id,
                source_life_seq: mutation.owner_life_seq,
                source_server_tick: mutation.created_tick,
                source_weapon_data: mutation.source_weapon_data,
                source_projectile_index: mutation.source_projectile_index,
                result_weapon_data: mutation.result_weapon_data,
                position_x: mutation.position_x,
                position_y: mutation.position_y,
                velocity_x: mutation.velocity_x,
                velocity_y: mutation.velocity_y,
                expires_at_tick: mutation.expires_at_tick,
                bounces_remaining: mutation.bounces_remaining,
            })
        })
        .collect::<Vec<_>>();
    let decoy_lifetime = crate::arena_settings::arena_decoy_lifetime_ticks();
    let mut decoys_repelled = 0_u32;
    for decoy in &mut state.active_decoys {
        if decoy.frequency == source_frequency {
            continue;
        }
        let Some(velocity) = crate::gameplay_rules::repel_velocity_after(
            origin,
            crate::gameplay_rules::RulesVector::new(decoy.position_x, decoy.position_y),
            repel_speed,
            repel_radius,
        ) else {
            continue;
        };
        decoy.velocity_x = velocity.x;
        decoy.velocity_y = velocity.y;
        decoy.expires_at_tick = crate::continuum::continuum_tick_add(server_tick, decoy_lifetime);
        public_mutations.push(crate::realtime_event::RealtimeRepelMutationState {
            motion_frozen: false,
            entity_kind: crate::realtime_event::REPEL_MUTATION_DECOY,
            source_player_id: decoy.owner.player_id(),
            source_session_id: decoy.owner.session.session_id.get(),
            source_life_seq: decoy.owner.life.life_seq,
            source_server_tick: decoy.created_tick,
            source_weapon_data: decoy.source_weapon_data,
            source_projectile_index: 0,
            result_weapon_data: decoy.source_weapon_data,
            position_x: decoy.position_x,
            position_y: decoy.position_y,
            velocity_x: decoy.velocity_x,
            velocity_y: decoy.velocity_y,
            expires_at_tick: decoy.expires_at_tick,
            bounces_remaining: 0,
        });
        decoys_repelled = decoys_repelled.saturating_add(1);
    }
    for mutation in &public_mutations {
        let entity_kind =
            if mutation.entity_kind == crate::realtime_event::REPEL_MUTATION_PROJECTILE {
                "projectile"
            } else {
                "decoy"
            };
        tracing::info!(
            "zubspace_event {{\"type\":\"server_repel_entity_state\",\"entity_kind\":\"{}\",\"source_player_id\":{},\"source_session_id\":\"{}\",\"source_life_seq\":{},\"source_server_tick\":{},\"source_weapon_data\":{},\"source_projectile_index\":{},\"result_weapon_data\":{},\"position_x\":{},\"position_y\":{},\"velocity_x\":{},\"velocity_y\":{},\"expires_at_tick\":{},\"bounces_remaining\":{},\"motion_frozen\":{}}}",
            entity_kind,
            mutation.source_player_id,
            mutation.source_session_id,
            mutation.source_life_seq,
            mutation.source_server_tick,
            mutation.source_weapon_data,
            mutation.source_projectile_index,
            mutation.result_weapon_data,
            mutation.position_x,
            mutation.position_y,
            mutation.velocity_x,
            mutation.velocity_y,
            mutation.expires_at_tick,
            mutation.bounces_remaining,
            mutation.motion_frozen,
        );
    }
    if projectile_mutation.projectiles_repelled != 0 || decoys_repelled != 0 {
        tracing::info!(
            "zubspace_event {{\"type\":\"server_repel_projectiles_applied\",\"source_player_id\":{},\"source_life_seq\":{},\"server_tick\":{},\"projectiles_repelled\":{},\"mines_converted\":{},\"decoys_repelled\":{}}}",
            source_player_id,
            source.life.life_seq,
            server_tick,
            projectile_mutation.projectiles_repelled,
            projectile_mutation.mines_converted,
            decoys_repelled,
        );
    }
    let mut bots_repelled = 0_u32;
    for bot in &mut state.bots {
        if !bot.alive() || bot.player.frequency == source_frequency {
            continue;
        }
        let bot_x = bot.x.round().clamp(0.0, f32::from(u16::MAX)) as u16;
        let bot_y = bot.y.round().clamp(0.0, f32::from(u16::MAX)) as u16;
        if world_position_in_safe_tile_for_map(map, bot_x, bot_y) {
            continue;
        }
        let Some(expected_velocity) = crate::gameplay_rules::repel_velocity_after(
            origin,
            crate::gameplay_rules::RulesVector::new(bot.x, bot.y),
            repel_speed,
            repel_radius,
        ) else {
            continue;
        };
        bot.velocity_x = expected_velocity.x;
        bot.velocity_y = expected_velocity.y;
        bot.repel_until_tick = Some(expires_at_tick);
        // Repel is an authoritative external motion discontinuity. A wall-
        // following direction belongs to the predecessor navigation segment;
        // retaining it can reverse the freshly applied radial impulse before
        // the next snapshot. A later real collision may establish a new
        // wall-follow segment from the repelled trajectory.
        bot.wall_follow_until_tick = None;
        bot.wall_follow_x = 0;
        bot.wall_follow_y = 0;
        bot.begin_actor_impulse_segment();
        let motion_append = bot.append_actor_motion_sample(server_tick, false);
        debug_assert_eq!(
            motion_append,
            crate::combat_timeline::HistoryAppendDisposition::Applied,
            "a bot Repel must establish its new motion segment"
        );
        bots_repelled = bots_repelled.saturating_add(1);
        tracing::info!(
            "zubspace_event {{\"type\":\"server_repel_bot_applied\",\"source_player_id\":{},\"source_life_seq\":{},\"target_player_id\":{},\"target_life_seq\":{},\"server_tick\":{},\"origin_x\":{},\"origin_y\":{},\"target_x\":{},\"target_y\":{},\"velocity_x\":{},\"velocity_y\":{},\"expires_at_tick\":{}}}",
            source_player_id,
            source.life.life_seq,
            bot.player.id,
            bot.life_seq,
            server_tick,
            origin.x,
            origin.y,
            bot.x,
            bot.y,
            bot.velocity_x,
            bot.velocity_y,
            expires_at_tick,
        );
    }
    if bots_repelled != 0 {
        tracing::info!(
            "zubspace_event {{\"type\":\"server_repel_players_applied\",\"source_player_id\":{},\"source_life_seq\":{},\"server_tick\":{},\"bots_repelled\":{}}}",
            source_player_id,
            source.life.life_seq,
            server_tick,
            bots_repelled,
        );
    }
    let mut next_generation = state.next_repel_delivery_generation;
    let mut occupied_generations = state
        .clients
        .values()
        .flat_map(|client| {
            client
                .pending_repel_observations
                .iter()
                .map(|event| event.delivery_generation)
                .chain(
                    client
                        .retired_repel_applications
                        .iter()
                        .map(|event| event.delivery_generation),
                )
        })
        .collect::<std::collections::HashSet<_>>();
    let mut queue_overflows = 0_u64;

    for (&target_player_id, target) in &mut state.clients {
        if target_player_id == source_player_id
            || !target.joined
            || !target.alive
            || target.player.ship >= 8
            || target.player.frequency == source_frequency
        {
            continue;
        }
        let Some(target_witness) = target.canonical_movement_witness.filter(|witness| {
            witness.life_seq == target.life_seq
                && Some(witness.sequence) == target.last_position_sequence
        }) else {
            continue;
        };
        let target_x = target_witness.wire_x;
        let target_y = target_witness.wire_y;
        if world_position_in_safe_tile_for_map(map, target_x, target_y) {
            continue;
        }
        let Some(expected_velocity) = crate::gameplay_rules::repel_velocity_after(
            origin,
            crate::gameplay_rules::RulesVector::new(
                target_witness.state.position_x,
                target_witness.state.position_y,
            ),
            repel_speed,
            repel_radius,
        ) else {
            continue;
        };
        if target.pending_repel_observations.len() >= crate::state::MAX_PENDING_REPEL_EVENTS {
            queue_overflows = queue_overflows.saturating_add(1);
            tracing::error!(
                target_player_id,
                target_life_seq = target.life_seq,
                source_player_id,
                source_life_seq = source.life.life_seq,
                queue_len = target.pending_repel_observations.len(),
                "zubspace_event {{\"type\":\"repel_queue_overflow\"}}"
            );
            continue;
        }
        let delivery_generation =
            allocate_repel_delivery_generation(&mut next_generation, &mut occupied_generations);
        let event = ServerRepelObservation {
            source,
            event_tick: server_tick,
            expires_at_tick,
            origin_x: origin.x,
            origin_y: origin.y,
            expected_velocity_x: expected_velocity.x,
            expected_velocity_y: expected_velocity.y,
            weapon_data,
            source_frequency,
            source_ship,
            delivery_generation,
            active_steps,
            delivered_snapshots: [None; crate::state::MAX_REPEL_DELIVERY_SNAPSHOTS],
            delivered_snapshot_cursor: 0,
            accepted_application: None,
            delivered_snapshot_id: None,
            delivered_snapshot_tick: None,
            delivered_active_steps: None,
            delivery_acked: false,
        };
        let enqueued = target.try_enqueue_repel(event);
        debug_assert!(enqueued, "prechecked Repel queue must retain the event");
        debug!(
            source_player_id,
            source_life_seq = source.life.life_seq,
            target_player_id,
            target_life_seq = target.life_seq,
            server_tick,
            origin_x,
            origin_y,
            target_x,
            target_y,
            expected_velocity_x = expected_velocity.x,
            expected_velocity_y = expected_velocity.y,
            expires_at_tick,
            "authoritative hostile repel awaiting target observation"
        );
    }
    state.next_repel_delivery_generation = next_generation;
    state.repel_queue_overflows = state.repel_queue_overflows.saturating_add(queue_overflows);
    public_mutations
}

fn consume_stale_weapon_mirror(
    state: &mut ServerState,
    player_id: u16,
    snapshot: ClientPositionSnapshot,
    client_sequence: u16,
    canonical_direction: u8,
    server_tick_at_accept: u32,
) -> bool {
    if state
        .clients
        .get(&player_id)
        .is_some_and(|sender| sender.weapon_sequence_seen(client_sequence))
    {
        return true;
    }

    let map = state.zone_map();
    let Some((witness, shooter_ship)) = state.clients.get(&player_id).and_then(|sender| {
        let witness = sender
            .canonical_position_witnesses
            .iter()
            .rev()
            .find(|witness| {
                witness.sequence == client_sequence && witness.life_seq == sender.life_seq
            })
            .copied()?;
        let record = sender
            .recent_input_records
            .iter()
            .rev()
            .find(|record| record.sequence == client_sequence)?;
        (record.door_seed == witness.door_seed
            && record.door_phase_tick == witness.door_phase_tick
            && canonical_direction == witness.direction
            && snapshot.direction == witness.direction
            && snapshot.x == witness.wire_x
            && snapshot.y == witness.wire_y
            && snapshot.velocity_x == witness.wire_velocity_x
            && snapshot.velocity_y == witness.wire_velocity_y
            && canonical_state_matches_exact(
                snapshot,
                witness.state.position_x,
                witness.state.position_y,
                witness.state.velocity_x,
                witness.state.velocity_y,
                witness.state.orientation,
            ))
        .then_some((witness, sender.player.ship))
    }) else {
        log_security_reject(player_id, "weapon", "mirror_witness_mismatch");
        return false;
    };
    if !world_coordinates_in_bounds(witness.wire_x, witness.wire_y, MAP_TILES, TILE_SIZE_PX)
        || position_blocking_tile_with_door_seed(
            tile_at_world(map, witness.wire_x, witness.wire_y),
            witness.door_seed,
        )
        || reported_position_hull_overlaps_blocked_tile(
            witness.wire_x,
            witness.wire_y,
            validated_position_collision_radius_pixels(shooter_ship),
            MAP_TILES,
            TILE_SIZE_PX,
            |tile_x, tile_y| {
                map.tiles
                    .get(tile_y * MAP_TILES + tile_x)
                    .is_none_or(|&tile| {
                        position_blocking_tile_with_door_seed(tile, witness.door_seed)
                    })
            },
        )
    {
        log_security_reject(player_id, "weapon", "mirror_blocked_origin");
        return false;
    }
    let mut canonical_snapshot = snapshot;
    canonical_snapshot.x = witness.wire_x;
    canonical_snapshot.y = witness.wire_y;
    canonical_snapshot.velocity_x = witness.wire_velocity_x;
    canonical_snapshot.velocity_y = witness.wire_velocity_y;
    canonical_snapshot.direction = witness.direction;
    let in_safe_tile =
        world_position_in_safe_tile_for_map(map, canonical_snapshot.x, canonical_snapshot.y);
    let mut weapon_block = WeaponBlock {
        bomb_safety: proximity_bomb_blocked_by_bomb_safety(state, player_id, canonical_snapshot),
        mine_limit: mine_blocked_by_mine_limits(state, player_id, canonical_snapshot),
        objective_ball: objective_weapon_substitutes_ball_fire(
            state,
            player_id,
            client_sequence,
            canonical_snapshot.weapon,
        ),
        combat_view: None,
    };
    let projectile_combat_timeline = match bind_human_projectile_timeline(
        state,
        player_id,
        client_sequence,
        server_tick_at_accept,
    ) {
        Ok(binding) => Some(binding),
        Err(rejection) => {
            debug!(
                player_id,
                client_sequence,
                server_tick = server_tick_at_accept,
                reason = rejection.reason(),
                "mirrored weapon edge has no admissible server combat-view binding"
            );
            weapon_block.combat_view = Some(combat_view_block_cause(
                state,
                player_id,
                client_sequence,
                rejection,
            ));
            None
        }
    };
    let is_flagger = player_flagger_active(state, player_id);
    let previewed_weapon = state.clients.get(&player_id).map_or(0, |sender| {
        preview_accepted_position_weapon(
            sender,
            canonical_snapshot,
            in_safe_tile,
            weapon_block,
            server_tick_at_accept,
            is_flagger,
        )
    });
    let realtime_event_recipients = (previewed_weapon != 0).then(|| {
        crate::snapshot::watch_damage_recipient_identities(state, player_id, &[player_id])
    });
    let realtime_event_reservation = if let Some(recipients) = realtime_event_recipients.as_deref()
    {
        let source_frequency = state
            .clients
            .get(&player_id)
            .map_or(0, |client| client.player.frequency);
        let delivery_count = realtime_weapon_delivery_count(
            state,
            previewed_weapon,
            source_frequency,
            witness.wire_x,
            witness.wire_y,
        );
        let recipient_groups = std::iter::repeat_n(recipients, delivery_count).collect::<Vec<_>>();
        let Some(reservation) = crate::snapshot::reserve_realtime_event_deliveries(
            state,
            &recipient_groups,
            crate::snapshot::RealtimeEventPressureDisposition::Deferred,
        ) else {
            debug!(
                player_id,
                client_sequence,
                "deferring mirrored weapon because dedicated realtime event capacity is full"
            );
            // Leave the weapon sequence unconsumed and tell the scheduler to
            // retain this exact packet. Cross-channel duplicates are only an
            // optimization, never the retry mechanism.
            if let Some(life_seq) = state.clients.get(&player_id).map(|client| client.life_seq) {
                state
                    .weapon_admission_deferred
                    .insert(LifeIdentity::new(player_id, life_seq));
            }
            return true;
        };
        Some(reservation)
    } else {
        None
    };
    let (accepted_weapon, shooter_ship, direction, x, y, velocity_x, velocity_y) = {
        let Some(sender) = state.clients.get_mut(&player_id) else {
            return false;
        };
        if !sender.joined || !sender.alive || sender.player.ship >= 8 {
            return false;
        }
        sender.remember_weapon_sequence(client_sequence);
        let accepted_weapon = accepted_position_weapon(
            sender,
            canonical_snapshot,
            in_safe_tile,
            weapon_block,
            server_tick_at_accept,
            is_flagger,
        );
        (
            accepted_weapon,
            sender.player.ship,
            witness.direction,
            witness.wire_x,
            witness.wire_y,
            witness.wire_velocity_x,
            witness.wire_velocity_y,
        )
    };
    debug_assert_eq!(
        accepted_weapon, previewed_weapon,
        "mirrored weapon preview and commit must agree"
    );

    if accepted_weapon != 0 {
        record_accepted_projectile(
            state,
            player_id,
            projectile_combat_timeline
                .expect("accepted mirrored human weapon must own an exact combat timeline"),
            accepted_weapon,
            shooter_ship,
            direction,
            x,
            y,
            velocity_x,
            velocity_y,
            witness.state,
            server_tick_at_accept,
            realtime_event_reservation
                .expect("accepted mirrored weapon must own dedicated event reservation"),
            realtime_event_recipients
                .expect("accepted mirrored weapon must retain exact recipients"),
        );
    }
    true
}

/// The most specific reason a weapon has no verified combat view. An
/// unverified input names its frozen receipt rejection (for example
/// `combat_view_application_too_far_behind`) when the server recorded one.
pub(crate) fn combat_view_block_cause(
    state: &ServerState,
    player_id: u16,
    input_sequence: u16,
    rejection: crate::combat_timeline::ProjectileTimelineBindingRejection,
) -> &'static str {
    if rejection != crate::combat_timeline::ProjectileTimelineBindingRejection::UnverifiedInput {
        return rejection.reason();
    }
    state
        .clients
        .get(&player_id)
        .and_then(|client| {
            client
                .observer_combat_timeline
                .receipt_rejection_for_sequence(input_sequence)
        })
        .map_or(rejection.reason(), |receipt| receipt.reason())
}

fn bind_human_projectile_timeline(
    state: &ServerState,
    player_id: u16,
    input_sequence: u16,
    accepted_server_tick: u32,
) -> Result<
    crate::combat_timeline::ProjectileCombatTimelineV1,
    crate::combat_timeline::ProjectileTimelineBindingRejection,
> {
    let runtime = state
        .active_player_runtime_identity(player_id)
        .ok_or(crate::combat_timeline::ProjectileTimelineBindingRejection::WrongRuntime)?;
    let client = state
        .clients
        .get(&player_id)
        .ok_or(crate::combat_timeline::ProjectileTimelineBindingRejection::WrongRuntime)?;
    let timeline = &client.observer_combat_timeline;
    let result = timeline.bind_projectile(runtime, input_sequence, accepted_server_tick);
    if let Err(rejection) = result
        && state.config.sim_observe
    {
        let decision = timeline.decision_for_sequence(input_sequence);
        info!(
            player_id,
            session_id = client.session_id.get(),
            arena_stream_id = client.arena_stream_id,
            life_seq = client.life_seq,
            input_sequence,
            server_tick = accepted_server_tick,
            reason = rejection.reason(),
            receipt_reason = timeline
                .receipt_rejection_for_sequence(input_sequence)
                .map_or("none", |reason| reason.reason()),
            verified_view = decision.is_some(),
            held_or_stalled = decision.is_some_and(|view| view.hold_or_stall),
            delay_ticks = timeline.delay_ticks_at(accepted_server_tick),
            "weapon combat-view binding unavailable"
        );
    }
    result
}

fn player_flagger_active(state: &ServerState, player_id: u16) -> bool {
    player_flagger_active_with_ball_policy(
        state,
        player_id,
        crate::arena_settings::arena_use_flagger_for_ball(),
    )
}

fn objective_weapon_substitutes_ball_fire(
    state: &ServerState,
    player_id: u16,
    position_sequence: u16,
    weapon: u16,
) -> bool {
    objective_weapon_substitutes_ball_fire_with_policy(
        state,
        player_id,
        position_sequence,
        weapon,
        crate::arena_settings::arena_ball_carrier_allows_guns(),
        crate::arena_settings::arena_ball_carrier_allows_bombs(),
    )
}

fn objective_weapon_substitutes_ball_fire_with_policy(
    state: &ServerState,
    player_id: u16,
    position_sequence: u16,
    weapon: u16,
    allow_guns: bool,
    allow_bombs: bool,
) -> bool {
    let Some(client) = state.clients.get(&player_id) else {
        return false;
    };
    if client.objective_ball_fire_witness_consumed(position_sequence) {
        return true;
    }
    let Some(runtime) = state.active_player_runtime_identity(player_id) else {
        return false;
    };
    if !crate::objectives::player_carries_powerball(state, runtime) {
        return false;
    }
    match continuum_weapon_type(weapon) {
        1 | 2 => !allow_guns,
        3 | 4 => !allow_bombs,
        _ => false,
    }
}

#[cfg(test)]
pub(crate) fn preview_and_commit_position_weapon_with_ball_policy_for_test(
    state: &mut ServerState,
    player_id: u16,
    snapshot: ClientPositionSnapshot,
    server_fire_tick: u32,
    allow_guns: bool,
    allow_bombs: bool,
) -> Option<(u16, u16)> {
    let sequence = snapshot.client_sequence?;
    let blocked = objective_weapon_substitutes_ball_fire_with_policy(
        state,
        player_id,
        sequence,
        snapshot.weapon,
        allow_guns,
        allow_bombs,
    );
    let flagger_active = player_flagger_active(state, player_id);
    let previewed = preview_accepted_position_weapon(
        state.clients.get(&player_id)?,
        snapshot,
        false,
        WeaponBlock::objective_ball(blocked),
        server_fire_tick,
        flagger_active,
    );
    let committed = accepted_position_weapon(
        state.clients.get_mut(&player_id)?,
        snapshot,
        false,
        WeaponBlock::objective_ball(blocked),
        server_fire_tick,
        flagger_active,
    );
    Some((previewed, committed))
}

fn player_flagger_active_with_ball_policy(
    state: &ServerState,
    player_id: u16,
    use_flagger_for_ball: bool,
) -> bool {
    let Some(runtime) = state.active_player_runtime_identity(player_id) else {
        return false;
    };
    state.objectives.flag_carrier_count(runtime) > 0
        || (use_flagger_for_ball && crate::objectives::player_carries_powerball(state, runtime))
}

#[cfg(test)]
pub(crate) fn preview_and_commit_position_weapon_for_test(
    state: &mut ServerState,
    player_id: u16,
    snapshot: ClientPositionSnapshot,
    server_fire_tick: u32,
    use_flagger_for_ball: bool,
) -> Option<(bool, u16, u16)> {
    let flagger_active =
        player_flagger_active_with_ball_policy(state, player_id, use_flagger_for_ball);
    let objective_weapon_blocked = snapshot.client_sequence.is_some_and(|sequence| {
        objective_weapon_substitutes_ball_fire(state, player_id, sequence, snapshot.weapon)
    });
    let previewed = preview_accepted_position_weapon(
        state.clients.get(&player_id)?,
        snapshot,
        objective_weapon_blocked,
        WeaponBlock::objective_ball(objective_weapon_blocked),
        server_fire_tick,
        flagger_active,
    );
    let committed = accepted_position_weapon(
        state.clients.get_mut(&player_id)?,
        snapshot,
        false,
        WeaponBlock::default(),
        server_fire_tick,
        flagger_active,
    );
    Some((flagger_active, previewed, committed))
}

fn record_accepted_projectile(
    state: &mut ServerState,
    player_id: u16,
    projectile_combat_timeline: crate::combat_timeline::ProjectileCombatTimelineV1,
    accepted_weapon: u16,
    shooter_ship: u8,
    direction: u8,
    accepted_x: u16,
    accepted_y: u16,
    _accepted_velocity_x: i16,
    _accepted_velocity_y: i16,
    exact_source: crate::state::ServerSimMovement,
    server_tick_at_accept: u32,
    realtime_event_reservation: crate::snapshot::RealtimeEventReservation,
    realtime_event_recipients: Vec<crate::authority_identity::PlayerRuntimeIdentity>,
) {
    let (owner_life_seq, source_session_id, source_frequency) = {
        let Some(sender) = state.clients.get(&player_id) else {
            return;
        };
        (
            sender.life_seq,
            sender.session_id.get(),
            sender.player.frequency,
        )
    };
    let Some(shot_group_id) = state.allocate_projectile_group_id() else {
        tracing::error!(
            owner_id = player_id,
            owner_life_seq,
            server_tick = server_tick_at_accept,
            weapon = accepted_weapon,
            "projectile shot-group id space exhausted"
        );
        return;
    };
    let weapon_event = crate::realtime_event::RealtimeWeaponEvent {
        source_player_id: player_id,
        source_session_id,
        source_life_seq: owner_life_seq,
        shot_group_id,
        x: exact_source.position_x,
        y: exact_source.position_y,
        velocity_x: exact_source.velocity_x,
        velocity_y: exact_source.velocity_y,
        orientation: exact_source.orientation,
        frequency: source_frequency,
        ship: shooter_ship,
        weapon_data: accepted_weapon,
    };
    let projectiles = crate::gameplay_rules::initialize_projectiles_for_protocol_direction(
        accepted_weapon,
        crate::gameplay_rules::RulesVector::new(exact_source.position_x, exact_source.position_y),
        crate::gameplay_rules::RulesVector::new(exact_source.velocity_x, exact_source.velocity_y),
        direction,
        crate::arena_settings::arena_projectile_initialization_tuning(shooter_ship),
    );
    let lifetime_ticks = crate::arena_settings::arena_weapon_lifetime_ticks(accepted_weapon);
    // Phase 2b: anchor projectile timing to the server's Continuum tick.
    // Client-reported timestamps are not trusted for authoritative weapon
    // cadence, lifetime, or hit resolution.
    let expiry_tick = lifetime_ticks.map(|lifetime_ticks| {
        crate::continuum::continuum_tick_add(server_tick_at_accept, lifetime_ticks)
    });
    let weapon_type = crate::continuum_weapon::continuum_weapon_type(accepted_weapon);
    let alternate = crate::continuum_weapon::continuum_weapon_alternate(accepted_weapon);
    let public_repel_mutations = if weapon_type == 5 {
        register_authoritative_repel_observations(
            state,
            player_id,
            accepted_weapon,
            accepted_x,
            accepted_y,
            server_tick_at_accept,
        )
    } else {
        Vec::new()
    };
    let mut realtime_deliveries = vec![crate::snapshot::RealtimeEventDelivery {
        recipients: realtime_event_recipients.clone(),
        server_tick: server_tick_at_accept,
        payload: crate::realtime_event::RealtimeEventPayload::Weapon(weapon_event),
    }];
    for mutations in
        public_repel_mutations.chunks(crate::realtime_event::MAX_REPEL_MUTATIONS_PER_EVENT)
    {
        let batch = crate::realtime_event::RealtimeRepelMutationBatch::from_states(mutations)
            .expect("nonempty bounded Repel mutation chunk");
        realtime_deliveries.push(crate::snapshot::RealtimeEventDelivery {
            recipients: realtime_event_recipients.clone(),
            server_tick: server_tick_at_accept,
            payload: crate::realtime_event::RealtimeEventPayload::RepelMutations(batch),
        });
    }
    realtime_event_reservation
        .commit(state, realtime_deliveries)
        .expect("exclusive weapon event reservation must remain valid");
    state.note_owner_weapon_group(weapon_event);
    if state.qa_traces.is_recording() {
        state.qa_traces.record_for_all(serde_json::json!({
            "type": "server_weapon_event_committed",
            "serverTick": server_tick_at_accept,
            "ownerId": player_id,
            "ownerSessionId": source_session_id.to_string(),
            "ownerLifeSeq": owner_life_seq,
            "ownerInputSequence": projectile_combat_timeline.owner_input_sequence,
            "observerTimelineGeneration": projectile_combat_timeline.accepted_timeline_generation,
            "acceptedAppliedSnapshotId": projectile_combat_timeline.accepted_game_applied_snapshot_id,
            "maximumRewindTicks": projectile_combat_timeline.maximum_rewind_ticks,
            "weapon": accepted_weapon,
            "weaponType": weapon_type,
            "alternate": alternate,
            "source": {
                "x": exact_source.position_x,
                "y": exact_source.position_y,
                "velocityX": exact_source.velocity_x,
                "velocityY": exact_source.velocity_y,
                "orientation": exact_source.orientation,
            },
            "recipientCount": realtime_event_recipients.len(),
            "projectileCount": projectiles.len(),
        }));
    }
    if weapon_type == 6 {
        let owner = crate::authority_identity::PlayerRuntimeIdentity::new(
            crate::authority_identity::PlayerSessionIdentity::new(
                player_id,
                crate::authority_identity::PlayerSessionId::new(source_session_id),
            ),
            crate::authority_identity::LifeIdentity::new(player_id, owner_life_seq),
        );
        let presentation_id = state.allocate_projectile_id();
        state.active_decoys.push(crate::state::ServerDecoy {
            presentation_id,
            presentation_source: Some(weapon_event),
            source_input_sequence: Some(projectile_combat_timeline.owner_input_sequence),
            owner,
            source_weapon_data: accepted_weapon,
            frequency: source_frequency,
            ship: shooter_ship,
            position_x: exact_source.position_x,
            position_y: exact_source.position_y,
            velocity_x: exact_source.velocity_x,
            velocity_y: exact_source.velocity_y,
            orientation: exact_source.orientation,
            created_tick: server_tick_at_accept,
            last_step_tick: server_tick_at_accept,
            expires_at_tick: crate::continuum::continuum_tick_add(
                server_tick_at_accept,
                crate::arena_settings::arena_decoy_lifetime_ticks(),
            ),
        });
        while state
            .active_decoys
            .iter()
            .filter(|decoy| decoy.owner == owner)
            .count()
            > crate::authority_projectiles::MAX_PENDING_HITS_PER_OWNER
        {
            if let Some(index) = state
                .active_decoys
                .iter()
                .position(|decoy| decoy.owner == owner)
            {
                state.active_decoys.remove(index);
            }
        }
        while state.active_decoys.len() > crate::authority_projectiles::MAX_PENDING_HITS_TOTAL {
            state.active_decoys.remove(0);
        }
    }
    if projectiles.is_empty() {
        return;
    }
    let mut authoritative_group = Vec::with_capacity(projectiles.len());
    for (shot_index, projectile) in projectiles.into_iter().enumerate() {
        let spawn_x = projectile.position.x.round().clamp(0.0, u16::MAX as f32) as u16;
        let spawn_y = projectile.position.y.round().clamp(0.0, u16::MAX as f32) as u16;
        let spawn_velocity_x = (projectile.velocity.x * 10.0)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        let spawn_velocity_y = (projectile.velocity.y * 10.0)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        let projectile_id = state.allocate_projectile_id();
        authoritative_group.push(ServerProjectile {
            id: projectile_id,
            shot_group_id,
            shot_index: shot_index as u8,
            presentation_source: Some(weapon_event),
            owner_id: player_id,
            owner_life_seq,
            combat_timeline: Some(projectile_combat_timeline),
            weapon_data: accepted_weapon,
            x: spawn_x,
            y: spawn_y,
            timestamp: server_tick_at_accept,
            created_tick: server_tick_at_accept,
            velocity_x: spawn_velocity_x,
            velocity_y: spawn_velocity_y,
            expiry_tick,
            pos_x: projectile.position.x,
            pos_y: projectile.position.y,
            source_server_tick: Some(server_tick_at_accept),
            precise_velocity: Some(projectile.velocity),
            bounces_remaining: projectile.bounces_remaining,
            behavior: projectile.behavior,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        });
        info!(
            target: crate::log_targets::POSITIONS_COMBAT,
            projectile_id,
            shot_group_id,
            owner_id = player_id,
            owner_life_seq,
            owner_session_id = source_session_id,
            owner_input_sequence = projectile_combat_timeline.owner_input_sequence,
            server_tick = server_tick_at_accept,
            weapon = accepted_weapon,
            weapon_type,
            weapon_level = projectile.level,
            weapon_raw_level = projectile.raw_level,
            alternate,
            x = spawn_x,
            y = spawn_y,
            velocity_x = spawn_velocity_x,
            velocity_y = spawn_velocity_y,
            expiry_tick = ?expiry_tick,
            bounces_remaining = projectile.bounces_remaining,
            "server projectile spawned"
        );
        let expiry_tick_json =
            expiry_tick.map_or_else(|| "null".to_owned(), |tick| tick.to_string());
        debug!(
            "zubspace_event {{\"type\":\"server_projectile_spawned\",\"projectile_id\":{},\"shot_group_id\":{},\"owner_id\":{},\"owner_life_seq\":{},\"server_tick\":{},\"weapon\":{},\"x\":{},\"y\":{},\"velocity_x\":{},\"velocity_y\":{},\"expiry_tick\":{},\"bounces_remaining\":{}}}",
            projectile_id,
            shot_group_id,
            player_id,
            owner_life_seq,
            server_tick_at_accept,
            accepted_weapon,
            spawn_x,
            spawn_y,
            spawn_velocity_x,
            spawn_velocity_y,
            expiry_tick_json,
            projectile.bounces_remaining,
        );
        if state.qa_traces.is_recording() {
            state.qa_traces.record_for_all(serde_json::json!({
                "type": "server_projectile_spawned",
                "projectileId": projectile_id,
                "shotGroupId": shot_group_id,
                "shotIndex": shot_index,
                "ownerId": player_id,
                "ownerSessionId": source_session_id.to_string(),
                "ownerLifeSeq": owner_life_seq,
                "ownerInputSequence": projectile_combat_timeline.owner_input_sequence,
                "observerTimelineGeneration": projectile_combat_timeline.accepted_timeline_generation,
                "acceptedAppliedSnapshotId": projectile_combat_timeline.accepted_game_applied_snapshot_id,
                "maximumRewindTicks": projectile_combat_timeline.maximum_rewind_ticks,
                "serverTick": server_tick_at_accept,
                "weapon": accepted_weapon,
                "x": spawn_x,
                "y": spawn_y,
                "velocityX": spawn_velocity_x,
                "velocityY": spawn_velocity_y,
                "expiryTick": expiry_tick,
                "bouncesRemaining": projectile.bounces_remaining,
            }));
        }
    }
    state.record_projectile_group(authoritative_group);
}

pub(crate) fn advance_server_recharge(sender: &mut ClientHandle, server_tick: u32) -> u32 {
    let Some(last_tick) = sender.last_energy_recharge_tick else {
        sender.last_energy_recharge_tick = Some(server_tick);
        return 0;
    };
    let elapsed_ticks = crate::continuum::continuum_tick_elapsed(server_tick, last_tick);
    if elapsed_ticks == 0 {
        return 0;
    }
    let mut energy = EnergyPool::new(
        sender.server_energy,
        sender.server_max_energy,
        sender.server_energy_remainder,
    );
    // The reference game's `ShipController:244-259` skips recharge only while the
    // player is EMP-shutdown. The EngineShutdown *prize* zeroes thrust but
    // still recharges, so EMP and prize movement Shutdown remain separate.
    // When a server step crosses the exact EMP expiry boundary, charge only the portion
    // after that boundary; otherwise a coarse or replayed tick schedule would
    // grant recharge for EMP-frozen time.
    let recharge_ticks = if sender.server_emp_active(server_tick) {
        0
    } else {
        sender
            .server_emp_until_tick
            .map(|until| crate::continuum::continuum_tick_elapsed(server_tick, until))
            .unwrap_or(elapsed_ticks)
            .min(elapsed_ticks)
    };
    if recharge_ticks > 0 {
        energy.recharge_for_position_ticks(recharge_ticks, sender.server_recharge_per_second);
    }
    sender.server_energy = energy.current();
    sender.server_energy_remainder = energy.recharge_remainder();
    sender.last_energy_recharge_tick = Some(server_tick);
    elapsed_ticks
}

fn advance_server_continuous_energy_drain_anchor(
    sender: &mut ClientHandle,
    server_tick: u32,
) -> u32 {
    let Some(last_tick) = sender.last_continuous_energy_drain_tick else {
        sender.last_continuous_energy_drain_tick = Some(server_tick);
        return 0;
    };
    let elapsed_ticks = crate::continuum::continuum_tick_elapsed(server_tick, last_tick);
    if elapsed_ticks == 0 {
        return 0;
    }
    sender.last_continuous_energy_drain_tick = Some(server_tick);
    elapsed_ticks
}

fn drain_authenticated_afterburner_energy(
    sender: &mut ClientHandle,
    elapsed_ticks: u32,
    server_tick: u32,
) {
    const THRUST_OR_REVERSE_BITS: u16 = (1 << 0) | (1 << 1);
    const AFTERBURNER_HELD_BIT: u16 = 1 << 4;

    // `server_sim_input_bits` is the exact input bound to the prior accepted
    // position. `last_input_record` may already contain a newer unordered
    // input and therefore cannot own the continuous interval ending at this
    // server tick.
    let authenticated_input = sender.server_sim_input_bits.unwrap_or(0);
    let afterburner_held = authenticated_input & AFTERBURNER_HELD_BIT != 0;
    let thrusting = authenticated_input & THRUST_OR_REVERSE_BITS != 0;
    // The reference game disables Afterburner while Rocket movement owns thrust. This
    // function runs before the tick-maintenance sweep clears an expired Rocket
    // deadline, so `is_some()` correctly covers the complete preceding
    // interval, including the exact expiry tick.
    let rocket_owned_interval = sender.server_rocket_until_tick.is_some();
    if elapsed_ticks == 0
        || !afterburner_held
        || !thrusting
        || rocket_owned_interval
        || sender.server_engine_shutdown_active(server_tick)
    {
        return;
    }
    let mut energy = EnergyPool::new(
        sender.server_energy,
        sender.server_max_energy,
        sender.server_energy_remainder,
    );
    // The reference game's `ShipController:244-264` enforces the order
    // Afterburner → recharge → status costs. Never infer the held modifier from
    // speed: a legitimate Top Speed prize can exceed the ship's initial cap,
    // and the old velocity heuristic silently charged such coasting players.
    let elapsed_seconds = crate::continuum::ticks_to_seconds_u32(elapsed_ticks);
    let drain = arena_ship_afterburner_energy_per_second(sender.player.ship) * elapsed_seconds;
    if drain > 0.0 {
        energy.spend_continuous_energy(drain);
    }
    sender.server_energy = energy.current();
    sender.server_energy_remainder = energy.recharge_remainder();
}

/// Advance all continuous energy transitions exactly once for this player
/// batch, using only the prior accepted authoritative position. Recharge and
/// drain keep independent anchors because damage/EMP transitions may reset
/// recharge without forgiving status/afterburner costs.
pub(crate) fn advance_server_gameplay_energy(sender: &mut ClientHandle, server_tick: u32) {
    let prior_state = sender.last_position.as_deref().and_then(|position| {
        let (velocity_x, velocity_y) = large_position_velocity(position)?;
        let status = *position.get(13)?;
        Some((velocity_x, velocity_y, status))
    });
    let drain_ticks = advance_server_continuous_energy_drain_anchor(sender, server_tick);

    drain_authenticated_afterburner_energy(sender, drain_ticks, server_tick);
    advance_server_recharge(sender, server_tick);
    if let Some((_, _, prior_status)) = prior_state {
        let in_safe_tile = prior_status & POSITION_STATUS_SAFETY != 0;
        let status = accepted_position_status_bits(sender, prior_status, in_safe_tile, drain_ticks);
        if let Some(position) = sender.last_position.as_mut() {
            if position.len() >= 23 {
                position[13] = status;
            }
        }
    }
    if let Some(position) = sender.last_position.as_mut() {
        if position.len() >= 23 {
            position[21..23].copy_from_slice(&sender.server_energy.to_le_bytes());
        }
    }
}

fn accepted_position_status_bits(
    sender: &mut ClientHandle,
    reported_status: u8,
    in_safe_tile: bool,
    elapsed_ticks: u32,
) -> u8 {
    let mut energy = EnergyPool::new(
        sender.server_energy,
        sender.server_max_energy,
        sender.server_energy_remainder,
    );
    let status_bits = sender
        .server_status
        .position_status_bits(reported_status, in_safe_tile)
        .drain_energy_for_statuses(
            &mut energy,
            StatusEnergyCosts {
                stealth_per_second: arena_ship_stealth_energy_per_second(sender.player.ship),
                cloak_per_second: arena_ship_cloak_energy_per_second(sender.player.ship),
                xradar_per_second: arena_ship_xradar_energy_per_second(sender.player.ship),
                antiwarp_per_second: arena_ship_antiwarp_energy_per_second(sender.player.ship),
            },
            elapsed_ticks,
        );
    sender.server_energy = energy.current();
    sender.server_energy_remainder = energy.recharge_remainder();
    status_bits.bits()
}

pub(crate) fn set_coordinates_packet(map: &ZoneMap) -> Vec<u8> {
    let (x, y) = server_spawn_coordinates(map);
    let mut packet = Vec::with_capacity(5);
    packet.push(SET_COORDINATES);
    packet.extend(x.to_le_bytes());
    packet.extend(y.to_le_bytes());
    packet
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn large_position_from_client(player_id: u16, packet: &[u8]) -> Option<Vec<u8>> {
    #[cfg(test)]
    let normalized_packet = normalize_legacy_mutated_position_fixture(packet);
    #[cfg(test)]
    let packet = normalized_packet.as_deref().unwrap_or(packet);
    let snapshot = parse_client_position_snapshot(packet)?;
    large_position_from_client_snapshot(player_id, snapshot)
}

fn large_position_from_client_snapshot(
    player_id: u16,
    snapshot: ClientPositionSnapshot,
) -> Option<Vec<u8>> {
    if !world_coordinates_in_bounds(snapshot.x, snapshot.y, MAP_TILES, TILE_SIZE_PX) {
        return None;
    }
    if !valid_relayed_weapon_data(snapshot.weapon) {
        return None;
    }

    let mut payload = vec![0; 23];
    payload[0] = crate::arena::LARGE_POSITION_PACKET;
    payload[1] = snapshot.direction;
    // The client-position wire format still contains a timestamp field for
    // Continuum compatibility, but zubspace authority assigns the outgoing
    // position tick from `wire::current_tick()` in `accept_position_relay`.
    // Do not copy client-provided time into server state, even temporarily.
    payload[2..4].copy_from_slice(&0_u16.to_le_bytes());
    payload[4..6].copy_from_slice(&snapshot.x.to_le_bytes());
    payload[6..8].copy_from_slice(&snapshot.velocity_y.to_le_bytes());
    payload[8..10].copy_from_slice(&player_id.to_le_bytes());
    payload[10..12].copy_from_slice(&snapshot.velocity_x.to_le_bytes());
    payload[13] = snapshot.togglables;
    payload[15..17].copy_from_slice(&snapshot.y.to_le_bytes());
    payload[17..19].copy_from_slice(&snapshot.bounty.to_le_bytes());
    payload[19..21].copy_from_slice(&snapshot.weapon.to_le_bytes());
    payload[21..23].copy_from_slice(&snapshot.energy.to_le_bytes());
    Some(payload)
}

pub(crate) fn large_position_coordinates(packet: &[u8]) -> Option<(u16, u16)> {
    if packet.len() < 17 {
        return None;
    }
    Some((
        u16::from_le_bytes(packet[4..6].try_into().ok()?),
        u16::from_le_bytes(packet[15..17].try_into().ok()?),
    ))
}

pub(crate) fn frequency_baseline_from_large_position(
    packet: &[u8],
    sequence: u16,
    canonical_state: crate::continuum_position::ClientPositionCanonicalState,
) -> Option<ClientPositionSnapshot> {
    if packet.len() < 23 {
        return None;
    }
    Some(ClientPositionSnapshot {
        direction: packet[1],
        timestamp: 0,
        client_sequence: Some(sequence),
        x: u16::from_le_bytes(packet[4..6].try_into().ok()?),
        y: u16::from_le_bytes(packet[15..17].try_into().ok()?),
        velocity_x: i16::from_le_bytes(packet[10..12].try_into().ok()?),
        velocity_y: i16::from_le_bytes(packet[6..8].try_into().ok()?),
        togglables: packet[13],
        bounty: u16::from_le_bytes(packet[17..19].try_into().ok()?),
        energy: u16::from_le_bytes(packet[21..23].try_into().ok()?),
        weapon: 0,
        canonical_state,
        repel_snapshot_id: None,
        repel_application_sequence: None,
        repel_delivery_generation: None,
        warp_collectible_id: None,
    })
}

/// Decode the canonical position and velocity from the same accepted relay.
/// Death-green eligibility must not combine coordinates from one observation
/// with movement state from another.
pub(crate) fn large_position_coordinates_and_velocity(
    packet: &[u8],
) -> Option<((u16, u16), (i16, i16))> {
    Some((
        large_position_coordinates(packet)?,
        large_position_velocity(packet)?,
    ))
}

fn large_position_velocity(packet: &[u8]) -> Option<(i16, i16)> {
    if packet.len() < 12 {
        return None;
    }
    Some((
        i16::from_le_bytes(packet[10..12].try_into().ok()?),
        i16::from_le_bytes(packet[6..8].try_into().ok()?),
    ))
}

#[cfg_attr(not(test), allow(dead_code))]
fn position_relay_senders(
    state: &Arc<Mutex<ServerState>>,
    recipient_ids: &[u16],
) -> Vec<ClientPacketSender> {
    let state = state.lock().expect("server state should not be poisoned");
    recipient_ids
        .iter()
        .filter_map(|id| state.clients.get(id).map(|client| client.tx.clone()))
        .collect()
}

fn allow_position_relay_burst(client: &mut ClientHandle, now: Instant) -> bool {
    let max_tokens = POSITION_RELAY_BURST_LIMIT as f32;
    let tokens = if let Some(last) = client.last_position_relay_at {
        let elapsed = now.duration_since(last).as_secs_f32();
        (client.position_relay_tokens + elapsed * POSITION_RELAY_RATE_PER_SECOND).min(max_tokens)
    } else {
        max_tokens
    };
    client.last_position_relay_at = Some(now);

    if tokens < 1.0 {
        client.position_relay_tokens = tokens;
        return false;
    }

    client.position_relay_tokens = tokens - 1.0;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::authority::{StatusInventory, WeaponInventory};
    use crate::authority_identity::{
        LifeIdentity, PlayerRuntimeIdentity, PlayerSessionId, PlayerSessionIdentity,
    };
    use crate::reliable::{ServerReliableReceiver, ServerReliableSender};
    use crate::state::{PLAYER_BANNER_BYTES, PlayerSummary};
    use std::sync::mpsc;

    fn test_client_handle() -> ClientHandle {
        let (tx, _rx) = mpsc::channel();
        ClientHandle {
            protocol_revision: 0,
            snapshot_holds: Default::default(),
            maxed_spawn_loadout: false, // This fixture tests the authored base movement model.
            spawn_idle: Default::default(),
            session_summary: Default::default(),
            session_id: PlayerSessionId::new(1),
            arena_stream_id: 1,
            inputs_received: 0,
            positions_received: 0,
            ack_ids_logged: 0,
            next_realtime_event_id: 0,
            qa_trace_bundle_id: None,
            threat_presentation: Default::default(),
            ingress_gate: Arc::new(Mutex::new(())),
            tx: tx.into(),
            simulated_tx_loss: crate::simulated_loss::SimulatedTxLoss::disabled(),
            simulated_tx_loss_after_bootstrap: false,
            player: PlayerSummary {
                id: 7,
                name: "pilot".to_string(),
                ship: 0,
                frequency: 0,
                assists: 0,
                kill_points: 0,
                flag_points: 0,
                wins: 0,
                losses: 0,
                flags: 0,
                banner: [0; PLAYER_BANNER_BYTES],
            },
            player_identity: None,
            arena_admission_reserved: false,
            joined: true,
            snapshot_ready: true,
            connection_quality: None,
            bootstrap_reliable_barrier: None,
            map_transfer: None,
            outgoing_reliable: ServerReliableSender::default(),
            incoming_reliable: ServerReliableReceiver::default(),
            last_security_report: None,
            last_security_valid: None,
            door_state: None,
            door_seed_cache: None,
            server_bounty: 0,
            server_energy: 0,
            server_max_energy: 0,
            server_energy_remainder: 0.0,
            last_energy_recharge_tick: None,
            last_continuous_energy_drain_tick: None,
            server_super_until_tick: None,
            server_shield_until_tick: None,
            server_engine_shutdown_start_sequence: None,
            server_engine_shutdown_until_tick: None,
            pending_prize_effect_applications: std::collections::VecDeque::new(),
            damage_movement_grace: None,
            prize_effect_application_terminals: std::collections::VecDeque::new(),
            server_emp_start_sequence: None,
            server_emp_until_tick: None,
            server_prize_effect_rng_seed: crate::authority_prizes::SECURITY_PRIZE_SEED,
            server_prize_state_revision: 1,
            last_damage_prize_loss_seq: None,
            last_damage_prize_loss_id: None,
            server_rocket_until_tick: None,
            server_rocket_input_asserted: false,
            server_rocket_authorizations: std::collections::VecDeque::new(),
            server_status: StatusInventory::cleared(),
            server_weapons: WeaponInventory::cleared(),
            last_reported_energy: 0,
            last_position: None,
            aoi_observer_position: None,
            spectator_view_sequence: None,
            spectator_next_input_sequence: None,
            objective_last_safe: false,
            visibility_cell: None,
            last_position_server_tick: None,
            movement_time_budget: Default::default(),
            last_position_relay_at: None,
            position_relay_tokens: POSITION_RELAY_BURST_LIMIT as f32,
            position_relay_burst_count: 0,
            last_weapon_fire_tick: None,
            last_gun_fire_tick: None,
            last_bomb_fire_tick: None,
            weapon_cooldown_schedule: Default::default(),
            objective_weapon_lock_until_tick: None,
            last_accepted_weapon_data: None,
            last_accepted_weapon_tick: None,
            accepted_weapon_witnesses: Default::default(),
            objective_ball_fire_witnesses: Default::default(),
            last_special_weapon_request: None,
            last_brick_drop_tick: None,
            chat_rate_debt_until: None,
            last_ship_change_at: None,
            ship_change_burst_count: 0,
            last_frequency_change_at: None,
            frequency_change_burst_count: 0,
            last_attach_request_at: None,
            attach_request_burst_count: 0,
            alive: true,
            respawn_at_tick: None,
            life_seq: 0,
            life_started_tick: Some(0),
            last_input_seq: None,
            last_input_record: None,
            recent_input_records: std::collections::VecDeque::with_capacity(
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS,
            ),
            recent_input_rocket_assertions: std::collections::VecDeque::with_capacity(
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS,
            ),
            last_position_sequence: None,
            life_position_sequence_base: None,
            pending_frequency_baseline: None,
            pending_warp_prize_relocations: std::collections::VecDeque::new(),
            consumed_warp_prize_relocations: std::collections::VecDeque::new(),
            latest_weapon_seq: None,
            weapon_seq_mask: 0,
            latest_portal_action_id: None,
            portal_action_mask: 0,
            portal_placement_terminals: std::collections::VecDeque::new(),
            portal_warp_terminals: std::collections::VecDeque::new(),
            active_portal: None,
            server_warp_settle_until_tick: None,
            server_warp_bomb_lock_until_tick: None,
            canonical_position_witnesses: std::collections::VecDeque::with_capacity(
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS,
            ),
            canonical_movement_witness: None,
            canonical_mismatch_log_gate: Default::default(),
            server_sim_state: None,
            movement_drift_x: 0.0,
            movement_drift_y: 0.0,
            pending_movement_correction: None,
            retired_movement_corrections: std::collections::VecDeque::new(),
            pending_reset_application_terminals: std::collections::VecDeque::new(),
            next_movement_correction_generation: 0,
            movement_correction_test_forced: false,
            aoi_tier_memory: std::collections::HashMap::new(),
            server_sim_anchor_tick: None,
            server_sim_input_bits: None,
            server_thrust_acceleration: 0.0,
            server_top_speed: 0.0,
            server_rotation_per_second: 0.0,
            server_recharge_per_second: 0.0,
            server_repel_until_tick: None,
            server_repel_sequence_window: None,
            server_repel_observer_until_tick: None,
            pending_repel_observations: std::collections::VecDeque::new(),
            retired_repel_applications: std::collections::VecDeque::new(),
            position_history: std::collections::VecDeque::with_capacity(
                crate::state::MAX_POSITION_HISTORY + 1,
            ),
            actor_motion_history: crate::combat_timeline::ActorMotionHistory::new(),
            observer_presentation_history: crate::combat_timeline::ObserverPresentationHistory::new(
            ),
            observer_combat_timeline: Default::default(),
            adaptive_presentation: Default::default(),
            retired_projectile_observers: Default::default(),
            snapshot_seq: 0,
            #[cfg(debug_assertions)]
            qa_prize_baseline_delay_start_snapshot: None,
            last_acked_snapshot_id: None,
            projectile_refresh_pending: Default::default(),
            projectile_motion_frozen_published: Default::default(),
            last_projectile_refresh: None,
            snapshot_history: std::collections::VecDeque::new(),
            pending_snapshot_events: crate::snapshot::PendingSnapshotEvents::default(),
            pending_realtime_events: crate::snapshot::PendingRealtimeEvents::default(),
            pending_snapshot_weapon: 0,
            recent_damagers: Vec::new(),
            measured_rtt: None,
            lag_compensation_ticks: crate::state::DEFAULT_LAG_COMPENSATION_TICKS,
            send_pacing: crate::state::SendPacing::default(),
        }
    }

    fn repel_snapshot(velocity_x: i16, repel_snapshot_id: Option<u16>) -> ClientPositionSnapshot {
        ClientPositionSnapshot {
            direction: 0,
            timestamp: 0,
            client_sequence: Some(2),
            x: 600,
            y: 700,
            velocity_x,
            velocity_y: 0,
            togglables: 0,
            bounty: 0,
            energy: 0,
            weapon: 0,
            canonical_state:
                crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
                    0, 600, 700, velocity_x, 0,
                ),
            repel_snapshot_id,
            repel_application_sequence: repel_snapshot_id.map(|_| 2),
            repel_delivery_generation: repel_snapshot_id.map(|_| 1),
            warp_collectible_id: None,
        }
    }

    // Prod: a pilot fired at sequence 4440, sat in a safe zone for 9.5
    // minutes and then bombed at 38285, 33845 sequences later. Past the u16
    // half range the bomb compared as an ancient replay, the server dropped
    // its position and the client's recoil forced repeated snap-backs.
    #[test]
    fn weapon_after_a_long_ceasefire_is_fresh_while_old_weapons_stay_seen() {
        let last_shot = 4_440_u16;
        let bomb = 38_285_u16;

        let mut stale = test_client_handle();
        stale.remember_weapon_sequence(last_shot);
        assert!(
            stale.weapon_sequence_seen(bomb),
            "without the window following positions the bomb reads as a replay"
        );

        let mut client = test_client_handle();
        client.remember_weapon_sequence(last_shot);
        let mut sequence = last_shot;
        while sequence != bomb.wrapping_sub(1) {
            sequence = sequence.wrapping_add(1);
            client.keep_weapon_window_near(sequence);
        }
        client.keep_weapon_window_near(bomb);
        assert!(!client.weapon_sequence_seen(bomb));
        assert!(
            client.weapon_sequence_seen(last_shot),
            "consumed weapons stay seen"
        );
        client.remember_weapon_sequence(bomb);
        assert!(
            client.weapon_sequence_seen(bomb),
            "the bomb is consumed once"
        );

        // Through a full u16 epoch and beyond, a held trigger never ages a
        // fresh shot into a replay.
        let mut client = test_client_handle();
        client.remember_weapon_sequence(0);
        for step in 1..=200_000_u32 {
            let sequence = step as u16;
            client.keep_weapon_window_near(sequence);
            if step % 997 == 0 {
                assert!(!client.weapon_sequence_seen(sequence.wrapping_add(1)));
            }
        }
    }

    #[test]
    fn keeping_the_weapon_window_near_positions_preserves_recent_consumption() {
        let mut client = test_client_handle();
        client.remember_weapon_sequence(100);
        client.remember_weapon_sequence(98);
        // Within the allowed lag nothing moves.
        client.keep_weapon_window_near(100 + 0x4000);
        assert_eq!(client.latest_weapon_seq, Some(100));
        assert!(client.weapon_sequence_seen(98));
        assert!(
            !client.weapon_sequence_seen(99),
            "an unconsumed recent mirror stays usable"
        );
        // A small slide keeps the consumed bits at their sequences.
        client.keep_weapon_window_near(100 + 0x4000 + 3);
        assert_eq!(client.latest_weapon_seq, Some(103));
        assert!(client.weapon_sequence_seen(100));
        assert!(client.weapon_sequence_seen(98));
        assert!(!client.weapon_sequence_seen(101));
        // A sequence behind the window (a reordered or forged old report)
        // never pulls it backwards.
        client.keep_weapon_window_near(50);
        assert_eq!(client.latest_weapon_seq, Some(103));
    }

    #[test]
    fn held_repel_prediction_proof_matches_exact_caps_and_rejects_forged_or_unowned_tails() {
        use crate::state::{
            CanonicalPositionWitness, ClientInputObservation, SequencedInputObservation,
            ServerSimMovement,
        };
        let map = crate::zone::zone_map_for_id("chaos-zone").unwrap();
        for start in [24720_u16, 65500] {
            for (sealed_steps, predicted_steps, vx, vy) in [
                (134, 135, -328.79254_f32, -14.420603_f32),
                (135, 134, -324.68787_f32, -14.240574_f32),
            ] {
                let mut client = test_client_handle();
                client.player.ship = 0;
                client.life_seq = 1;
                client.server_thrust_acceleration = 190.0;
                client.server_top_speed = 325.0;
                client.server_rotation_per_second = 0.725;
                client.server_recharge_per_second = 115.0;
                let previous = CanonicalPositionWitness {
                    sequence: start.wrapping_add(133),
                    life_seq: 1,
                    state: ServerSimMovement {
                        position_x: 7230.1016,
                        position_y: 8165.2393,
                        orientation: 0.26583338,
                        velocity_x: -335.12585,
                        velocity_y: -14.420603,
                        energy: 1700.0,
                        max_energy: 1700.0,
                    },
                    wire_x: 7230,
                    wire_y: 8165,
                    wire_velocity_x: -3351,
                    wire_velocity_y: -144,
                    direction: 10,
                    door_seed: 0,
                    door_phase_tick: 0,
                };
                client.canonical_movement_witness = Some(previous);
                client.last_position_sequence = Some(previous.sequence);
                client.last_position_server_tick = Some(2552);
                for offset in [134, 135] {
                    client
                        .recent_input_records
                        .push_back(SequencedInputObservation {
                            sequence: start.wrapping_add(offset),
                            input: ClientInputObservation {
                                bits: 1,
                                direction: 10,
                            },
                            door_seed: 0,
                            door_phase_tick: 0,
                        });
                }
                let sealed = crate::gameplay_rules::repel_sequence_window_from_active_steps(
                    start,
                    sealed_steps,
                );
                client.server_repel_sequence_window = Some(sealed);
                let mut deliveries = [None; crate::state::MAX_REPEL_DELIVERY_SNAPSHOTS];
                deliveries[0] = Some(crate::state::ServerRepelDelivery {
                    snapshot_id: 933,
                    snapshot_tick: 2330,
                    active_steps: 135,
                    acknowledged_at_tick: None,
                });
                client
                    .pending_repel_observations
                    .push_back(ServerRepelObservation {
                        source: PlayerRuntimeIdentity::new(
                            PlayerSessionIdentity::new(9, PlayerSessionId::new(2)),
                            LifeIdentity::new(9, 3),
                        ),
                        event_tick: 2330,
                        expires_at_tick: 2555,
                        origin_x: 8313.0,
                        origin_y: 8200.0,
                        expected_velocity_x: -499.79202,
                        expected_velocity_y: -14.420603,
                        weapon_data: 5,
                        source_frequency: 2,
                        source_ship: 0,
                        delivery_generation: 1,
                        active_steps: 135,
                        delivered_snapshots: deliveries,
                        delivered_snapshot_cursor: 1,
                        accepted_application: Some(crate::state::ServerRepelAcceptedApplication {
                            snapshot_id: 933,
                            application_sequence: start,
                            accepted_end_sequence: start.wrapping_add(sealed_steps as u16),
                            active_steps: sealed_steps as u16,
                            receipt_reliable_ids: [None; crate::state::MAX_REPEL_RECEIPT_COPIES],
                            receipt_reliable_cursor: 0,
                        }),
                        delivered_snapshot_id: Some(933),
                        delivered_snapshot_tick: Some(2330),
                        delivered_active_steps: Some(135),
                        delivery_acked: true,
                    });
                let mut snapshot = repel_snapshot(0, None);
                snapshot.client_sequence = Some(start.wrapping_add(135));
                snapshot.direction = 10;
                snapshot.x = 7219;
                snapshot.y = 8165;
                snapshot.velocity_x = crate::continuum_position::protocol_velocity_component(vx);
                snapshot.velocity_y = crate::continuum_position::protocol_velocity_component(vy);
                snapshot.canonical_state =
                    crate::continuum_position::ClientPositionCanonicalState::from_f32(
                        7218.9834, 8164.759, vx, vy, 0.26583338,
                    );
                let prove = |client: &ClientHandle, snapshot: ClientPositionSnapshot| {
                    repel_prediction::matching_offered_window(
                        client,
                        map,
                        snapshot,
                        false,
                        false,
                        false,
                        crate::sim_observation::movement_upgrades_for_client(client),
                        &[],
                        snapshot.client_sequence.unwrap(),
                        2556,
                        8200,
                        8200,
                        None,
                    )
                };
                assert_eq!(
                    prove(&client, snapshot),
                    Some(
                        crate::gameplay_rules::repel_sequence_window_from_active_steps(
                            start,
                            predicted_steps
                        )
                    )
                );
                assert_eq!(client.server_repel_sequence_window, Some(sealed));
                assert_eq!(client.pending_repel_observations[0].expires_at_tick, 2555);
                for field in 0..5 {
                    let mut forged = snapshot;
                    let bits = &mut forged.canonical_state;
                    match field {
                        0 => bits.position_x_bits ^= 1,
                        1 => bits.position_y_bits ^= 1,
                        2 => bits.velocity_x_bits ^= 1,
                        3 => bits.velocity_y_bits ^= 1,
                        _ => bits.orientation_bits ^= 1,
                    }
                    assert_eq!(prove(&client, forged), None, "raw-f32 field{field}");
                }
                let mut invalid = client.clone();
                invalid.life_seq += 1;
                assert_eq!(prove(&invalid, snapshot), None);
                invalid = client.clone();
                invalid.recent_input_records.pop_back();
                assert_eq!(prove(&invalid, snapshot), None);
                invalid = client.clone();
                invalid.pending_repel_observations[0].delivered_snapshots[0] = None;
                assert_eq!(prove(&invalid, snapshot), None);
                invalid = client.clone();
                invalid.pending_repel_observations[0].accepted_application = None;
                assert_eq!(prove(&invalid, snapshot), None);
                invalid = client.clone();
                invalid.pending_repel_observations.clear();
                assert_eq!(prove(&invalid, snapshot), None);
            }
        }
    }

    #[test]
    fn pending_repel_match_requires_exact_delivery_witness_before_exact_replay() {
        let mut client = test_client_handle();
        client.last_position_sequence = Some(1);
        client.movement_drift_x = 19.0;
        client.movement_drift_y = -7.0;
        client
            .pending_repel_observations
            .push_back(ServerRepelObservation {
                source: PlayerRuntimeIdentity::new(
                    PlayerSessionIdentity::new(9, PlayerSessionId::new(2)),
                    LifeIdentity::new(9, 3),
                ),
                event_tick: 100,
                expires_at_tick: 327,
                origin_x: 500.0,
                origin_y: 700.0,
                expected_velocity_x: 500.0,
                expected_velocity_y: 0.0,
                weapon_data: 5,
                source_frequency: 2,
                source_ship: 0,
                delivery_generation: 1,
                active_steps: 135,
                delivered_snapshots: [None; crate::state::MAX_REPEL_DELIVERY_SNAPSHOTS],
                delivered_snapshot_cursor: 0,
                accepted_application: None,
                delivered_snapshot_id: None,
                delivered_snapshot_tick: None,
                delivered_active_steps: None,
                delivery_acked: false,
            });

        assert!(
            matching_pending_repel_observations(&mut client, repel_snapshot(5_000, None), 101)
                .is_none()
        );
        assert_eq!(
            (client.movement_drift_x, client.movement_drift_y),
            (19.0, -7.0)
        );

        client
            .pending_repel_observations
            .front_mut()
            .unwrap()
            .delivered_snapshot_id = Some(u16::MAX - 1);
        client
            .pending_repel_observations
            .front_mut()
            .unwrap()
            .delivered_snapshots[0] = Some(crate::state::ServerRepelDelivery {
            snapshot_id: u16::MAX - 1,
            snapshot_tick: 102,
            active_steps: 135,
            acknowledged_at_tick: Some(102),
        });
        client.last_acked_snapshot_id = Some(u16::MAX - 2);
        assert!(
            matching_pending_repel_observations(&mut client, repel_snapshot(5_000, None), 102)
                .is_none()
        );
        assert!(
            matching_pending_repel_observations(
                &mut client,
                repel_snapshot(5_000, Some(u16::MAX)),
                102
            )
            .is_none()
        );

        assert!(
            matching_pending_repel_observations(
                &mut client,
                repel_snapshot(4_400, Some(u16::MAX - 1)),
                103
            )
            .is_some()
        );
        // This helper proves only that the client observed the exact delivered
        // snapshot. The caller derives the impulse from the retained exact
        // witness and requires a bit-exact canonical replay tail; an
        // attacker-selected vector can never be authorized here.
        assert_eq!(
            (client.movement_drift_x, client.movement_drift_y),
            (19.0, -7.0)
        );

        assert!(
            matching_pending_repel_observations(
                &mut client,
                repel_snapshot(5_000, Some(u16::MAX - 1)),
                104
            )
            .is_some()
        );
        assert_eq!(
            (client.movement_drift_x, client.movement_drift_y),
            (19.0, -7.0)
        );

        client.pending_repel_observations.clear();
        client
            .pending_repel_observations
            .push_back(ServerRepelObservation {
                source: PlayerRuntimeIdentity::new(
                    PlayerSessionIdentity::new(9, PlayerSessionId::new(2)),
                    LifeIdentity::new(9, 3),
                ),
                event_tick: 105,
                expires_at_tick: 330,
                origin_x: 500.0,
                origin_y: 700.0,
                expected_velocity_x: 500.0,
                expected_velocity_y: 0.0,
                weapon_data: 5,
                source_frequency: 2,
                source_ship: 0,
                delivery_generation: 1,
                active_steps: 135,
                delivered_snapshots: {
                    let mut delivered = [None; crate::state::MAX_REPEL_DELIVERY_SNAPSHOTS];
                    delivered[0] = Some(crate::state::ServerRepelDelivery {
                        snapshot_id: 0,
                        snapshot_tick: 105,
                        active_steps: 135,
                        acknowledged_at_tick: Some(105),
                    });
                    delivered
                },
                delivered_snapshot_cursor: 1,
                accepted_application: None,
                delivered_snapshot_id: Some(0),
                delivered_snapshot_tick: Some(105),
                delivered_active_steps: Some(135),
                delivery_acked: true,
            });
        assert!(
            matching_pending_repel_observations(&mut client, repel_snapshot(5_000, None), 106)
                .is_none()
        );
        assert!(
            matching_pending_repel_observations(&mut client, repel_snapshot(5_000, Some(0)), 106)
                .is_some()
        );
        let mut forged_full_suffix = repel_snapshot(5_000, Some(0));
        forged_full_suffix.client_sequence = Some(138);
        assert!(
            matching_pending_repel_observations(&mut client, forged_full_suffix, 329).is_none(),
            "an observed tail cannot claim more already-applied records than the immutable delivery offered"
        );
        assert!(
            client
                .pending_repel_observations
                .front()
                .is_some_and(|event| event.accepted_application.is_none()),
            "an overlong tail rejects atomically"
        );
        let narrowed =
            matching_pending_repel_observations(&mut client, repel_snapshot(5_000, Some(0)), 329)
                .expect("a first marker one server tick before expiry remains admissible");
        assert_eq!(
            narrowed,
            RepelObservationMatch::Pending(
                client
                    .pending_repel_observations
                    .front()
                    .copied()
                    .expect("matching is prospective"),
                crate::state::ServerRepelAcceptedApplication {
                    snapshot_id: 0,
                    application_sequence: 2,
                    accepted_end_sequence: 3,
                    active_steps: 1,
                    receipt_reliable_ids: [None; crate::state::MAX_REPEL_RECEIPT_COPIES],
                    receipt_reliable_cursor: 0,
                },
                crate::state::ServerRepelDelivery {
                    snapshot_id: 0,
                    snapshot_tick: 105,
                    active_steps: 135,
                    acknowledged_at_tick: Some(105),
                },
            ),
            "server receipt time narrows a stale full-duration offer to the one honest remaining step"
        );
        assert!(
            client
                .pending_repel_observations
                .front()
                .is_some_and(|event| event.accepted_application.is_none()),
            "a prospective marker match must not mutate authority state before exact replay succeeds"
        );
        assert!(
            matching_pending_repel_observations(&mut client, repel_snapshot(5_000, Some(0)), 331)
                .is_none(),
            "the first application marker must reach the server before the immutable deadline"
        );
        assert!(client.pending_repel_observations.is_empty());
    }

    #[test]
    fn pending_repel_server_deadline_narrowing_is_tick_and_sequence_wrap_safe() {
        let mut client = test_client_handle();
        client.last_position_sequence = Some(u16::MAX);
        let event_tick = crate::continuum::CONTINUUM_TICK_MASK - 2;
        let expires_at_tick = crate::continuum::continuum_tick_add(event_tick, 7);
        client
            .pending_repel_observations
            .push_back(ServerRepelObservation {
                source: PlayerRuntimeIdentity::new(
                    PlayerSessionIdentity::new(9, PlayerSessionId::new(2)),
                    LifeIdentity::new(9, 3),
                ),
                event_tick,
                expires_at_tick,
                origin_x: 500.0,
                origin_y: 700.0,
                expected_velocity_x: 500.0,
                expected_velocity_y: 0.0,
                weapon_data: 5,
                source_frequency: 2,
                source_ship: 0,
                delivery_generation: 1,
                active_steps: 5,
                delivered_snapshots: {
                    let mut delivered = [None; crate::state::MAX_REPEL_DELIVERY_SNAPSHOTS];
                    delivered[0] = Some(crate::state::ServerRepelDelivery {
                        snapshot_id: u16::MAX,
                        snapshot_tick: event_tick,
                        active_steps: 5,
                        acknowledged_at_tick: Some(event_tick),
                    });
                    delivered
                },
                delivered_snapshot_cursor: 1,
                accepted_application: None,
                delivered_snapshot_id: Some(u16::MAX),
                delivered_snapshot_tick: Some(event_tick),
                delivered_active_steps: Some(5),
                delivery_acked: true,
            });
        let mut snapshot = repel_snapshot(5_000, Some(u16::MAX));
        snapshot.client_sequence = Some(0);
        snapshot.repel_application_sequence = Some(0);
        let receipt_tick = crate::continuum::continuum_tick_sub(expires_at_tick, 1);
        let matched = matching_pending_repel_observations(&mut client, snapshot, receipt_tick)
            .expect("wrapped marker reaches the server one tick before expiry");
        let RepelObservationMatch::Pending(_, application, _) = matched else {
            panic!("new wrapped application must not be classified as a duplicate");
        };
        assert_eq!(application.application_sequence, 0);
        assert_eq!(application.accepted_end_sequence, 1);
        assert_eq!(application.active_steps, 1);

        assert!(
            matching_pending_repel_observations(&mut client, snapshot, expires_at_tick).is_none(),
            "the exact wrapped deadline is already too late for a first marker"
        );
        assert!(client.pending_repel_observations.is_empty());
    }

    #[test]
    fn position_relay_limit_allows_steady_sixty_hz_without_gap() {
        let start = Instant::now();
        let mut client = test_client_handle();
        let mut accepted = 0;

        for index in 0..12 {
            let now = start + Duration::from_millis(index * 16);
            if allow_position_relay_burst(&mut client, now) {
                accepted += 1;
            }
        }

        assert_eq!(accepted, 12);
    }

    #[test]
    fn position_relay_limit_rejects_more_than_three_inside_window() {
        let start = Instant::now();
        let mut client = test_client_handle();

        assert!(allow_position_relay_burst(&mut client, start));
        assert!(allow_position_relay_burst(
            &mut client,
            start + Duration::from_millis(5)
        ));
        assert!(allow_position_relay_burst(
            &mut client,
            start + Duration::from_millis(10)
        ));
        assert!(!allow_position_relay_burst(
            &mut client,
            start + Duration::from_millis(15)
        ));
        assert!(allow_position_relay_burst(
            &mut client,
            start + POSITION_RELAY_RATE_WINDOW
        ));
    }

    #[test]
    fn repel_delivery_generation_wrap_skips_every_live_or_retired_identity() {
        let mut next = u32::MAX - 1;
        let mut occupied = [u32::MAX, 1, 2].into_iter().collect();
        assert_eq!(
            allocate_repel_delivery_generation(&mut next, &mut occupied),
            3
        );
        assert_eq!(next, 3);
        assert_eq!(
            allocate_repel_delivery_generation(&mut next, &mut occupied),
            4
        );
    }
}
