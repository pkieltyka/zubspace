use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::arena_settings::{arena_max_frequency, arena_ship_change_cost};
use crate::authority_identity::PlayerRuntimeIdentity;
use crate::rate_limit::allow_short_burst;
use crate::reliable::broadcast_reliable_to_joined;
use crate::roster::{frequency_cutover_ack, ship_frequency_change};
use crate::state::{ReliableLifecycleMutationOutcome, ServerState};

const ROSTER_CHANGE_RATE_WINDOW: Duration = Duration::from_millis(250);
const ROSTER_CHANGE_BURST_LIMIT: u8 = 3;

#[cfg(test)]
pub(crate) fn update_frequency(state: &Arc<Mutex<ServerState>>, player_id: u16, payload: &[u8]) {
    update_frequency_at_tick(state, player_id, payload, crate::wire::current_tick());
}

#[cfg(test)]
pub(crate) fn update_frequency_at_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    payload: &[u8],
    tick: u32,
) {
    update_frequency_instant_at_tick(state, player_id, payload, tick, Instant::now());
}

#[cfg(test)]
pub(crate) fn update_frequency_instant_at_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    payload: &[u8],
    tick: u32,
    now: Instant,
) {
    update_frequency_for_runtime_instant_at_tick(state, None, player_id, payload, tick, now);
}

pub(crate) fn update_frequency_for_runtime_instant_at_tick(
    state: &Arc<Mutex<ServerState>>,
    expected_identity: Option<PlayerRuntimeIdentity>,
    player_id: u16,
    payload: &[u8],
    tick: u32,
    now: Instant,
) -> ReliableLifecycleMutationOutcome {
    if payload.len() <= 5 {
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    }
    let frequency = u16::from_le_bytes([payload[1], payload[2]]);
    let position_sequence_base = u16::from_le_bytes([payload[3], payload[4]]);
    if frequency >= arena_max_frequency() {
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    }
    let mut state_guard = state.lock().expect("server state should not be poisoned");
    if state_guard.config.free_for_all {
        crate::security::log_security_reject(player_id, "frequency_change", "free_for_all");
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    }
    let identity = {
        let state = &*state_guard;
        let client = state.clients.get(&player_id);
        if client.is_none_or(|client| !client.joined || client.player.frequency == frequency) {
            return ReliableLifecycleMutationOutcome::TerminalNoop;
        }
        if client.is_some_and(|client| {
            !roster_change_burst_available(
                client.last_frequency_change_at,
                client.frequency_change_burst_count,
                now,
            )
        }) {
            return ReliableLifecycleMutationOutcome::TerminalNoop;
        }
        let identity = state.active_player_runtime_identity(player_id);
        if expected_identity.is_some() && identity != expected_identity {
            return ReliableLifecycleMutationOutcome::TerminalNoop;
        }
        identity
    };
    let Some(identity) = identity else {
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    };
    let baseline_packet = &payload[5..];
    let Some(snapshot) = crate::continuum_position::parse_client_position_snapshot(baseline_packet)
    else {
        crate::security::log_security_reject(player_id, "frequency_change", "invalid_baseline");
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    };
    let Some(input_packet) =
        crate::continuum_position::embedded_client_input_record(baseline_packet)
    else {
        crate::security::log_security_reject(player_id, "frequency_change", "missing_input");
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    };
    let exact_input = input_packet.len()
        == crate::continuum_position::CLIENT_POSITION_SINGLE_INPUT_RECORD_LEN
        && input_packet[0] == crate::connection::CLIENT_INPUT_RECORD_TYPE
        && u16::from_le_bytes([input_packet[1], input_packet[2]]) == position_sequence_base
        && input_packet[3] == 1
        && u16::from_le_bytes([input_packet[4], input_packet[5]]) & (1 << 5) == 0
        && input_packet[6] == snapshot.direction
        && crate::combat_timeline::CombatViewReceiptV1::decode(
            &input_packet[8..crate::continuum_position::CLIENT_POSITION_SINGLE_INPUT_RECORD_LEN],
        )
        .is_ok_and(|receipt| receipt.input_sequence == position_sequence_base);
    if snapshot.client_sequence != Some(position_sequence_base)
        || snapshot.timestamp != 0
        || snapshot.weapon != 0
        || snapshot.repel_snapshot_id.is_some()
        || !exact_input
    {
        crate::security::log_security_reject(
            player_id,
            "frequency_change",
            "noncanonical_baseline",
        );
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    }
    // Input observation and position admission mutate a wide, life-scoped
    // authority surface. Keep the ServerState lock for the whole request so
    // tick/ACK work cannot interleave, then restore the exact pre-admission
    // authority if any later validation rejects the baseline.
    let admission_snapshot = FrequencyAdmissionSnapshot::capture(&state_guard, player_id);
    if !crate::connection::record_client_input_observation_in_state_at_tick(
        &mut state_guard,
        identity,
        input_packet,
        tick,
    ) {
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    }
    // A reliable frequency request is an explicit server-owned lifecycle
    // boundary. The reliable channel can overtake a long/coalesced run of
    // already-authenticated realtime positions, leaving the previous
    // position anchor outside the bounded input window even though request N
    // itself is canonical. In that one missing-history case, preserve
    // authority by rebasing the cutover to the server's last accepted large
    // position. Never accept the client's unprovable coordinates, and never
    // use this fallback for an impossible direction, collision, rewrite, or
    // any other forged tail.
    let server_owned_missing_history_baseline =
        state_guard.clients.get(&player_id).and_then(|client| {
            matches!(
                crate::sim_observation::validate_position_input_witness(
                    client,
                    position_sequence_base,
                    snapshot.direction,
                    tick,
                ),
                Err(crate::sim_observation::PositionInputWitnessError::Missing)
            )
            .then(|| {
                client.last_position.as_deref().and_then(|position| {
                    let witness = client.canonical_movement_witness?;
                    crate::arena::frequency_baseline_from_large_position(
                        position,
                        position_sequence_base,
                        crate::continuum_position::ClientPositionCanonicalState::from_f32(
                            witness.state.position_x,
                            witness.state.position_y,
                            witness.state.velocity_x,
                            witness.state.velocity_y,
                            witness.state.orientation,
                        ),
                    )
                })
            })
            .flatten()
        });
    let prior_position_sequence = state_guard
        .clients
        .get(&player_id)
        .and_then(|client| client.last_position_sequence);
    let relay = server_owned_missing_history_baseline
        .is_none()
        .then(|| {
            crate::arena::accept_position_relay_for_life_in_state_at_tick(
                &mut state_guard,
                identity.life,
                baseline_packet,
                tick,
            )
        })
        .flatten();
    // Movement-only relays may be admitted authoritatively while their public
    // fanout is token-bucket suppressed. Frequency cutover depends on that
    // admission, not on whether observers needed another copy of the same
    // position, so verify the accepted sequence when no relay was returned.
    let baseline_admitted = server_owned_missing_history_baseline.is_some()
        || relay.is_some()
        || (prior_position_sequence != Some(position_sequence_base)
            && state_guard.clients.get(&player_id).is_some_and(|client| {
                client.last_position_sequence == Some(position_sequence_base)
            }));
    if !baseline_admitted {
        admission_snapshot.restore(&mut state_guard, player_id);
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    }
    let canonical_baseline = server_owned_missing_history_baseline.or_else(|| {
        state_guard.clients.get(&player_id).and_then(|client| {
            client.last_position.as_deref().and_then(|position| {
                let witness = client.canonical_movement_witness?;
                crate::arena::frequency_baseline_from_large_position(
                    position,
                    position_sequence_base,
                    crate::continuum_position::ClientPositionCanonicalState::from_f32(
                        witness.state.position_x,
                        witness.state.position_y,
                        witness.state.velocity_x,
                        witness.state.velocity_y,
                        witness.state.orientation,
                    ),
                )
            })
        })
    });
    let Some(canonical_baseline) = canonical_baseline else {
        admission_snapshot.restore(&mut state_guard, player_id);
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    };
    match update_player_roster_in_state(
        &mut state_guard,
        RosterChangeOrigin::Player,
        expected_identity,
        player_id,
        None,
        Some(frequency),
        Some(position_sequence_base),
        Some(canonical_baseline),
        tick,
        now,
    ) {
        RosterChangeOutcome::Applied(packet) => {
            let cutover = {
                let client = state_guard
                    .clients
                    .get(&player_id)
                    .expect("active frequency client");
                frequency_cutover_ack(frequency, client.life_seq, canonical_baseline)
            };
            if let Some(client) = state_guard.clients.get_mut(&player_id) {
                crate::reliable::send_reliable_payload(client, &cutover);
            }
            drop(state_guard);
            broadcast_reliable_to_joined(state, packet);
            ReliableLifecycleMutationOutcome::Applied
        }
        RosterChangeOutcome::TerminalNoop => {
            admission_snapshot.restore(&mut state_guard, player_id);
            ReliableLifecycleMutationOutcome::TerminalNoop
        }
        RosterChangeOutcome::Deferred => {
            admission_snapshot.restore(&mut state_guard, player_id);
            ReliableLifecycleMutationOutcome::Deferred
        }
    }
}

#[cfg(test)]
pub(crate) fn update_ship(state: &Arc<Mutex<ServerState>>, player_id: u16, payload: &[u8]) {
    update_ship_at_tick(state, player_id, payload, crate::wire::current_tick());
}

#[cfg(test)]
pub(crate) fn update_ship_at_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    payload: &[u8],
    tick: u32,
) {
    update_ship_for_runtime_at_tick(state, None, player_id, payload, tick);
}

pub(crate) fn update_ship_for_runtime_at_tick(
    state: &Arc<Mutex<ServerState>>,
    expected_identity: Option<PlayerRuntimeIdentity>,
    player_id: u16,
    payload: &[u8],
    tick: u32,
) -> ReliableLifecycleMutationOutcome {
    let Some(ship) = payload.get(1).copied().map(normalized_ship) else {
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    };
    let Some(position_sequence_base) = payload
        .get(2..4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u16::from_le_bytes)
    else {
        crate::security::log_security_reject(
            player_id,
            "ship_change",
            "missing_position_sequence_base",
        );
        return ReliableLifecycleMutationOutcome::TerminalNoop;
    };
    match update_player_roster(
        state,
        expected_identity,
        player_id,
        Some(ship),
        None,
        Some(position_sequence_base),
        None,
        tick,
        Instant::now(),
    ) {
        RosterChangeOutcome::Applied(packet) => {
            broadcast_reliable_to_joined(state, packet);
            ReliableLifecycleMutationOutcome::Applied
        }
        RosterChangeOutcome::TerminalNoop => ReliableLifecycleMutationOutcome::TerminalNoop,
        RosterChangeOutcome::Deferred => ReliableLifecycleMutationOutcome::Deferred,
    }
}

pub(crate) enum RosterChangeOutcome {
    Applied(Vec<u8>),
    TerminalNoop,
    Deferred,
}

fn update_player_roster(
    state: &Arc<Mutex<ServerState>>,
    expected_identity: Option<PlayerRuntimeIdentity>,
    player_id: u16,
    ship: Option<u8>,
    frequency: Option<u16>,
    position_sequence_base: Option<u16>,
    frequency_baseline: Option<crate::continuum_position::ClientPositionSnapshot>,
    tick: u32,
    now: Instant,
) -> RosterChangeOutcome {
    let mut state = state.lock().expect("server state should not be poisoned");
    update_player_roster_in_state(
        &mut state,
        RosterChangeOrigin::Player,
        expected_identity,
        player_id,
        ship,
        frequency,
        position_sequence_base,
        frequency_baseline,
        tick,
        now,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RosterChangeOrigin {
    Player,
    ConnectionQuality,
    SpawnIdle,
}

/// Server-only origin: never exposed by the ship-change request parser.
/// The caller sends the scoped observer directive before publishing this roster
/// packet. A Deferred result must not start or extend a quality cooldown.
pub(crate) fn force_quality_spectator_in_state(
    state: &mut ServerState,
    identity: PlayerRuntimeIdentity,
    tick: u32,
    now: Instant,
) -> RosterChangeOutcome {
    update_player_roster_in_state(
        state,
        RosterChangeOrigin::ConnectionQuality,
        Some(identity),
        identity.player_id(),
        Some(8),
        None,
        None,
        None,
        tick,
        now,
    )
}

pub(crate) fn force_spawn_idle_spectator_in_state(
    state: &mut ServerState,
    identity: PlayerRuntimeIdentity,
    tick: u32,
    now: Instant,
) -> RosterChangeOutcome {
    update_player_roster_in_state(
        state,
        RosterChangeOrigin::SpawnIdle,
        Some(identity),
        identity.player_id(),
        Some(8),
        None,
        None,
        None,
        tick,
        now,
    )
}

fn update_player_roster_in_state(
    state: &mut ServerState,
    origin: RosterChangeOrigin,
    expected_identity: Option<PlayerRuntimeIdentity>,
    player_id: u16,
    ship: Option<u8>,
    frequency: Option<u16>,
    position_sequence_base: Option<u16>,
    frequency_baseline: Option<crate::continuum_position::ClientPositionSnapshot>,
    tick: u32,
    now: Instant,
) -> RosterChangeOutcome {
    if expected_identity.is_some()
        && state.active_player_runtime_identity(player_id) != expected_identity
    {
        return RosterChangeOutcome::TerminalNoop;
    }
    let forced_observer = matches!(
        origin,
        RosterChangeOrigin::ConnectionQuality | RosterChangeOrigin::SpawnIdle
    );
    if forced_observer
        && (ship != Some(8)
            || frequency.is_some()
            || position_sequence_base.is_some()
            || frequency_baseline.is_some())
    {
        return RosterChangeOutcome::TerminalNoop;
    }
    let clear_turret_links = ship.is_some() || frequency.is_some();
    let mut clear_position = false;
    let objective_context = crate::objectives::objective_player(state, player_id);
    let rollback_client = state.clients.get(&player_id).cloned();
    let rollback_objectives = state.objectives.clone();
    let return_hold = if origin == RosterChangeOrigin::Player
        && ship.is_some_and(|ship| ship < 8)
        && state
            .clients
            .get(&player_id)
            .is_some_and(|client| client.joined && Some(client.player.ship) != ship)
    {
        crate::connection_quality::hold_for_client(state, player_id)
    } else {
        None
    };
    if let Some(hold) = return_hold
        && !hold.may_return(
            now,
            crate::connection_quality::sustained_health_now(state, player_id, now),
        )
    {
        state.quality_holds.denied = state.quality_holds.denied.saturating_add(1);
        let client = state
            .clients
            .get_mut(&player_id)
            .expect("held client exists");
        let packet = crate::connection_quality::hold_packet(
            client,
            hold,
            now,
            Some((
                ship.expect("ship return"),
                position_sequence_base.expect("ship request sequence"),
            )),
        );
        crate::reliable::send_reliable_payload(client, &packet);
        tracing::info!(
            player_id,
            session_id = client.session_id.get(),
            hold_id = hold.id,
            remaining_ms = hold.remaining(now).as_millis() as u64,
            decision = "return_denied",
            "connection quality decision"
        );
        return RosterChangeOutcome::TerminalNoop;
    }
    let player = {
        let Some(client) = state.clients.get_mut(&player_id) else {
            return RosterChangeOutcome::TerminalNoop;
        };
        if !client.joined {
            return RosterChangeOutcome::TerminalNoop;
        }
        let same_ship = ship.is_none_or(|ship| client.player.ship == ship);
        let same_frequency = frequency.is_none_or(|frequency| client.player.frequency == frequency);
        if same_ship && same_frequency {
            return RosterChangeOutcome::TerminalNoop;
        }
        // Full energy is required for every transition out of a playable ship,
        // including spectator mode, which would otherwise bypass this gate.
        // Spectators can select a ship without an energy pool.
        if let Some(target_ship) = ship
            && !forced_observer
            && client.player.ship < 8
            && (!client.alive || client.server_energy < client.server_max_energy)
        {
            let mut rejection = vec![0x7f, 1, target_ship];
            rejection.extend(crate::wire::server_session_id().to_le_bytes());
            rejection.extend(client.arena_stream_id.to_le_bytes());
            rejection.extend(player_id.to_le_bytes());
            rejection.extend(client.life_seq.to_le_bytes());
            rejection.extend(
                position_sequence_base
                    .expect("ship request has a sequence")
                    .to_le_bytes(),
            );
            crate::reliable::send_reliable_payload(client, &rejection);
            return RosterChangeOutcome::TerminalNoop;
        }
        if ship.is_some()
            && !forced_observer
            && !allow_roster_change_burst(
                &mut client.last_ship_change_at,
                &mut client.ship_change_burst_count,
                now,
            )
        {
            return RosterChangeOutcome::TerminalNoop;
        }
        // ASSS-style ShipChangeCost gate: refuse the change when the
        // requesting ship is alive (in-arena) and lacks the configured
        // energy. `arena_ship_change_cost()` returns 0 unless an arena
        // override is set, so the default Continuum behaviour is
        // unchanged. Spectators/dead players (ship >= 8 or !alive) bypass
        // the gate so they can pick a ship without paying energy.
        if let Some(target_ship) = ship {
            let cost = arena_ship_change_cost();
            if cost > 0
                && client.alive
                && client.player.ship < 8
                && target_ship < 8
                && client.server_energy < cost
            {
                return RosterChangeOutcome::TerminalNoop;
            }
        }
        if frequency.is_some()
            && !allow_roster_change_burst(
                &mut client.last_frequency_change_at,
                &mut client.frequency_change_burst_count,
                now,
            )
        {
            return RosterChangeOutcome::TerminalNoop;
        }
        if origin == RosterChangeOrigin::Player && !same_ship {
            // Only a committed menu selection starts fresh grace. Rejected
            // requests must not erase the idle episode; unattended death/
            // respawn loops retain it across their life boundaries.
            client.spawn_idle = Default::default();
        }
        client.player.flags = 0;
        if let Some(ship) = ship {
            // Charge ShipChangeCost (currently always 0) before swapping
            // ships so the resulting ship state starts at the configured
            // initial energy rather than carrying the pre-change pool.
            let cost = arena_ship_change_cost();
            if cost > 0 && client.alive && client.player.ship < 8 && ship < 8 {
                client.server_energy = client.server_energy.saturating_sub(cost);
            }
            client.player.ship = ship;
            client.alive = ship < 8;
            client.respawn_at_tick = None;
            client.reset_server_runtime_for_ship_at_tick(tick);
            // A ship request is an intentional position discontinuity even
            // when both ships are live. Remove the old public/validation
            // anchor; the first packet at the request's sequence boundary
            // establishes the new ship life at spawn.
            clear_position = true;
        }
        if let Some(frequency) = frequency {
            // A frequency cutover advances life and retires every old-life
            // application barrier before the new baseline ACK. Emit durable
            // cancellation terminals so a lost/late application cannot keep
            // either endpoint paused or block a new-life Shutdown claim.
            client.cancel_pending_prize_effect_applications();
            client.server_engine_shutdown_start_sequence = None;
            client.server_engine_shutdown_until_tick = None;
            client.player.frequency = frequency;
            // This server's committed cutover contract deliberately preserves
            // the already-spent loadout while starting a new Rocket authority
            // life. The reference game's original self-frequency path calls Spawn(true),
            // but changing this established receipt contract would refund a
            // consumed Rocket and break ordered lifecycle reconciliation.
            client.clear_server_rocket_lifecycle();
        }
        client.player.clone()
    };
    if clear_turret_links && let Some(objective_context) = objective_context {
        let packets = match crate::objectives::player_lifecycle_packets_for_context(
            state,
            objective_context,
            None,
            crate::objectives::ObjectiveLifecycleReason::ShipOrFrequency,
            tick,
        ) {
            Ok(packets) => packets,
            Err(error) => {
                tracing::error!(
                    player_id,
                    ?error,
                    "roster change rolled back: objective cleanup could not commit"
                );
                state.objectives = rollback_objectives;
                if let Some(client) = rollback_client {
                    state.clients.insert(player_id, client);
                }
                return RosterChangeOutcome::Deferred;
            }
        };
        for packet in packets {
            for client in state.clients.values_mut().filter(|client| client.joined) {
                crate::reliable::send_reliable_payload(client, &packet);
            }
        }
    }
    let Some(life_seq) = state
        .advance_client_life_at_tick(player_id, tick)
        .map(|identity| identity.life_seq)
    else {
        state.objectives = rollback_objectives;
        if let Some(client) = rollback_client {
            state.clients.insert(player_id, client);
        }
        return RosterChangeOutcome::Deferred;
    };
    if clear_turret_links {
        let owner = state
            .active_player_runtime_identity(player_id)
            .expect("committed roster transition retains its joined session");
        crate::server_tick::cancel_owner_weapons_for_lifecycle_change(state, tick, owner);
        state.remove_projectiles_for_player(player_id);
        state.clear_pending_position_inputs_for_player(player_id);
    }
    if clear_position {
        state.clear_client_position(player_id);
        if ship.is_some() {
            // A ship request advances authority life and invalidates the old
            // position, but the replacement exact spawn baseline travels on
            // the reliable lifecycle path. Retain nearby actor presentation
            // across that bounded gap without installing self authority.
            state.set_client_spawn_aoi_observer(player_id);
        }
    }
    if (ship.is_some() || frequency.is_some())
        && let Some(client) = state.clients.get_mut(&player_id)
    {
        client.life_position_sequence_base = position_sequence_base;
        client.pending_frequency_baseline = frequency_baseline;
    }
    if clear_turret_links {
        state.turret_links.remove(&player_id);
        state
            .turret_links
            .retain(|_, parent_id| *parent_id != player_id);
    }
    if let Some(hold) = return_hold {
        let old_client = rollback_client.as_ref().expect("held client exists");
        let identity = old_client
            .player_identity
            .as_deref()
            .expect("held identity exists");
        // Removal is part of this committed roster transaction, never a probe
        // callback or a speculative request. Clear precedes the public ship ACK.
        assert!(state.quality_holds.finish(identity, hold.id, now, true));
        state.quality_holds.returned = state.quality_holds.returned.saturating_add(1);
        use crate::connection_quality_protocol::{
            ConnectionQualityDirective, ConnectionQualityDirectiveKind,
        };
        let clear = ConnectionQualityDirective {
            kind: ConnectionQualityDirectiveKind::Clear,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: old_client.arena_stream_id,
            player_id,
            predecessor_life_seq: old_client.life_seq,
            life_seq: old_client.life_seq,
            hold_id: hold.id,
            remaining_ms: 0,
            rejected_request: None,
        }
        .encode();
        let client = state
            .clients
            .get_mut(&player_id)
            .expect("returned client exists");
        crate::reliable::send_reliable_payload(client, &clear);
        tracing::info!(
            player_id,
            session_id = client.session_id.get(),
            hold_id = hold.id,
            decision = "return_accepted",
            "connection quality decision"
        );
    }
    RosterChangeOutcome::Applied(ship_frequency_change(&player, life_seq))
}

fn roster_change_burst_available(
    last_change_at: Option<Instant>,
    burst_count: u8,
    now: Instant,
) -> bool {
    !last_change_at.is_some_and(|last| now.duration_since(last) < ROSTER_CHANGE_RATE_WINDOW)
        || burst_count < ROSTER_CHANGE_BURST_LIMIT
}

struct FrequencyAdmissionSnapshot {
    player: crate::state::PlayerSummary,
    door_seed_cache: Option<crate::doors::DoorSeedCache>,
    server_bounty: u16,
    server_energy: u16,
    server_max_energy: u16,
    server_energy_remainder: f32,
    last_energy_recharge_tick: Option<u32>,
    last_continuous_energy_drain_tick: Option<u32>,
    server_super_until_tick: Option<u32>,
    server_shield_until_tick: Option<u32>,
    server_engine_shutdown_start_sequence: Option<u16>,
    server_engine_shutdown_until_tick: Option<u32>,
    pending_prize_effect_applications:
        std::collections::VecDeque<crate::state::PendingPrizeEffectApplication>,
    prize_effect_application_terminals:
        std::collections::VecDeque<crate::prize_application::PrizeEffectAccepted>,
    server_emp_start_sequence: Option<u16>,
    server_emp_until_tick: Option<u32>,
    server_prize_effect_rng_seed: i32,
    server_prize_state_revision: u32,
    last_damage_prize_loss_seq: Option<u32>,
    last_damage_prize_loss_id: Option<i16>,
    server_rocket_until_tick: Option<u32>,
    server_rocket_input_asserted: bool,
    server_rocket_authorizations:
        std::collections::VecDeque<crate::state::ServerRocketAuthorization>,
    server_status: crate::authority::StatusInventory,
    server_weapons: crate::authority::WeaponInventory,
    last_reported_energy: u16,
    last_position: Option<Vec<u8>>,
    objective_last_safe: bool,
    visibility_cell: Option<crate::visibility::VisibilityCell>,
    last_position_server_tick: Option<u32>,
    movement_time_budget: crate::state::MovementTimeBudget,
    last_position_relay_at: Option<Instant>,
    position_relay_tokens: f32,
    position_relay_burst_count: u8,
    last_weapon_fire_tick: Option<u32>,
    last_gun_fire_tick: Option<u32>,
    last_bomb_fire_tick: Option<u32>,
    objective_weapon_lock_until_tick: Option<u32>,
    last_accepted_weapon_data: Option<u16>,
    last_accepted_weapon_tick: Option<u32>,
    accepted_weapon_witnesses: crate::state::CanonicalWitnessConsumptionLedger,
    objective_ball_fire_witnesses: crate::state::CanonicalWitnessConsumptionLedger,
    last_special_weapon_request: Option<u8>,
    last_brick_drop_tick: Option<u32>,
    last_frequency_change_at: Option<Instant>,
    frequency_change_burst_count: u8,
    alive: bool,
    respawn_at_tick: Option<u32>,
    life_started_tick: Option<u32>,
    last_input_seq: Option<u16>,
    last_input_record: Option<crate::state::ClientInputObservation>,
    recent_input_records: std::collections::VecDeque<crate::state::SequencedInputObservation>,
    recent_input_rocket_assertions: std::collections::VecDeque<(u16, bool)>,
    last_position_sequence: Option<u16>,
    life_position_sequence_base: Option<u16>,
    pending_frequency_baseline: Option<crate::continuum_position::ClientPositionSnapshot>,
    pending_warp_prize_relocations:
        std::collections::VecDeque<crate::state::PendingWarpPrizeRelocation>,
    consumed_warp_prize_relocations: std::collections::VecDeque<u32>,
    latest_weapon_seq: Option<u16>,
    weapon_seq_mask: u64,
    latest_portal_action_id: Option<u32>,
    portal_action_mask: u64,
    portal_placement_terminals:
        std::collections::VecDeque<crate::portal_placement::PortalPlacementAccepted>,
    portal_warp_terminals: std::collections::VecDeque<crate::portal_warp::PortalWarpAccepted>,
    active_portal: Option<crate::state::ServerPortal>,
    server_warp_settle_until_tick: Option<u32>,
    server_warp_bomb_lock_until_tick: Option<u32>,
    canonical_position_witnesses:
        std::collections::VecDeque<crate::state::CanonicalPositionWitness>,
    canonical_movement_witness: Option<crate::state::CanonicalPositionWitness>,
    server_sim_state: Option<crate::state::ServerSimMovement>,
    server_sim_anchor_tick: Option<u32>,
    server_sim_input_bits: Option<u16>,
    movement_drift_x: f32,
    movement_drift_y: f32,
    server_thrust_acceleration: f32,
    server_top_speed: f32,
    server_rotation_per_second: f32,
    server_recharge_per_second: f32,
    server_repel_until_tick: Option<u32>,
    server_repel_sequence_window: Option<(u16, u16)>,
    server_repel_observer_until_tick: Option<u32>,
    pending_repel_observations: std::collections::VecDeque<crate::state::ServerRepelObservation>,
    retired_repel_applications:
        std::collections::VecDeque<crate::state::ServerRepelRetiredApplication>,
    position_history: std::collections::VecDeque<crate::state::RecentPositionSample>,
    actor_motion_history: crate::combat_timeline::ActorMotionHistory,
    aoi_tier_memory: std::collections::HashMap<u16, crate::visibility::AoiTier>,
    pending_snapshot_events: crate::snapshot::PendingSnapshotEvents,
    pending_realtime_events: crate::snapshot::PendingRealtimeEvents,
    pending_snapshot_weapon: u16,
    recent_damagers: Vec<(crate::authority_identity::LifeIdentity, u32)>,
}

impl FrequencyAdmissionSnapshot {
    fn capture(state: &ServerState, player_id: u16) -> Self {
        let client = &state.clients[&player_id];
        Self {
            player: client.player.clone(),
            door_seed_cache: client.door_seed_cache,
            server_bounty: client.server_bounty,
            server_energy: client.server_energy,
            server_max_energy: client.server_max_energy,
            server_energy_remainder: client.server_energy_remainder,
            last_energy_recharge_tick: client.last_energy_recharge_tick,
            last_continuous_energy_drain_tick: client.last_continuous_energy_drain_tick,
            server_super_until_tick: client.server_super_until_tick,
            server_shield_until_tick: client.server_shield_until_tick,
            server_engine_shutdown_start_sequence: client.server_engine_shutdown_start_sequence,
            server_engine_shutdown_until_tick: client.server_engine_shutdown_until_tick,
            pending_prize_effect_applications: client.pending_prize_effect_applications.clone(),
            prize_effect_application_terminals: client.prize_effect_application_terminals.clone(),
            server_emp_start_sequence: client.server_emp_start_sequence,
            server_emp_until_tick: client.server_emp_until_tick,
            server_prize_effect_rng_seed: client.server_prize_effect_rng_seed,
            server_prize_state_revision: client.server_prize_state_revision,
            last_damage_prize_loss_seq: client.last_damage_prize_loss_seq,
            last_damage_prize_loss_id: client.last_damage_prize_loss_id,
            server_rocket_until_tick: client.server_rocket_until_tick,
            server_rocket_input_asserted: client.server_rocket_input_asserted,
            server_rocket_authorizations: client.server_rocket_authorizations.clone(),
            server_status: client.server_status,
            server_weapons: client.server_weapons,
            last_reported_energy: client.last_reported_energy,
            last_position: client.last_position.clone(),
            objective_last_safe: client.objective_last_safe,
            visibility_cell: client.visibility_cell,
            last_position_server_tick: client.last_position_server_tick,
            movement_time_budget: client.movement_time_budget,
            last_position_relay_at: client.last_position_relay_at,
            position_relay_tokens: client.position_relay_tokens,
            position_relay_burst_count: client.position_relay_burst_count,
            last_weapon_fire_tick: client.last_weapon_fire_tick,
            last_gun_fire_tick: client.last_gun_fire_tick,
            last_bomb_fire_tick: client.last_bomb_fire_tick,
            objective_weapon_lock_until_tick: client.objective_weapon_lock_until_tick,
            last_accepted_weapon_data: client.last_accepted_weapon_data,
            last_accepted_weapon_tick: client.last_accepted_weapon_tick,
            accepted_weapon_witnesses: client.accepted_weapon_witnesses,
            objective_ball_fire_witnesses: client.objective_ball_fire_witnesses,
            last_special_weapon_request: client.last_special_weapon_request,
            last_brick_drop_tick: client.last_brick_drop_tick,
            last_frequency_change_at: client.last_frequency_change_at,
            frequency_change_burst_count: client.frequency_change_burst_count,
            alive: client.alive,
            respawn_at_tick: client.respawn_at_tick,
            life_started_tick: client.life_started_tick,
            last_input_seq: client.last_input_seq,
            last_input_record: client.last_input_record,
            recent_input_records: client.recent_input_records.clone(),
            recent_input_rocket_assertions: client.recent_input_rocket_assertions.clone(),
            last_position_sequence: client.last_position_sequence,
            life_position_sequence_base: client.life_position_sequence_base,
            pending_frequency_baseline: client.pending_frequency_baseline,
            pending_warp_prize_relocations: client.pending_warp_prize_relocations.clone(),
            consumed_warp_prize_relocations: client.consumed_warp_prize_relocations.clone(),
            latest_weapon_seq: client.latest_weapon_seq,
            weapon_seq_mask: client.weapon_seq_mask,
            latest_portal_action_id: client.latest_portal_action_id,
            portal_action_mask: client.portal_action_mask,
            portal_placement_terminals: client.portal_placement_terminals.clone(),
            portal_warp_terminals: client.portal_warp_terminals.clone(),
            active_portal: client.active_portal,
            server_warp_settle_until_tick: client.server_warp_settle_until_tick,
            server_warp_bomb_lock_until_tick: client.server_warp_bomb_lock_until_tick,
            canonical_position_witnesses: client.canonical_position_witnesses.clone(),
            canonical_movement_witness: client.canonical_movement_witness,
            server_sim_state: client.server_sim_state,
            server_sim_anchor_tick: client.server_sim_anchor_tick,
            server_sim_input_bits: client.server_sim_input_bits,
            movement_drift_x: client.movement_drift_x,
            movement_drift_y: client.movement_drift_y,
            server_thrust_acceleration: client.server_thrust_acceleration,
            server_top_speed: client.server_top_speed,
            server_rotation_per_second: client.server_rotation_per_second,
            server_recharge_per_second: client.server_recharge_per_second,
            server_repel_until_tick: client.server_repel_until_tick,
            server_repel_sequence_window: client.server_repel_sequence_window,
            server_repel_observer_until_tick: client.server_repel_observer_until_tick,
            pending_repel_observations: client.pending_repel_observations.clone(),
            retired_repel_applications: client.retired_repel_applications.clone(),
            position_history: client.position_history.clone(),
            actor_motion_history: client.actor_motion_history.clone(),
            aoi_tier_memory: client.aoi_tier_memory.clone(),
            pending_snapshot_events: client.pending_snapshot_events.clone(),
            pending_realtime_events: client.pending_realtime_events.clone(),
            pending_snapshot_weapon: client.pending_snapshot_weapon,
            recent_damagers: client.recent_damagers.clone(),
        }
    }

    fn restore(self, state: &mut ServerState, player_id: u16) {
        let Some(current_visibility_cell) = state
            .clients
            .get(&player_id)
            .map(|client| client.visibility_cell)
        else {
            return;
        };
        state.visibility_index.set_player_cell(
            player_id,
            current_visibility_cell,
            self.visibility_cell,
        );
        let Some(client) = state.clients.get_mut(&player_id) else {
            return;
        };
        client.player = self.player;
        client.door_seed_cache = self.door_seed_cache;
        client.server_bounty = self.server_bounty;
        client.server_energy = self.server_energy;
        client.server_max_energy = self.server_max_energy;
        client.server_energy_remainder = self.server_energy_remainder;
        client.last_energy_recharge_tick = self.last_energy_recharge_tick;
        client.last_continuous_energy_drain_tick = self.last_continuous_energy_drain_tick;
        client.server_super_until_tick = self.server_super_until_tick;
        client.server_shield_until_tick = self.server_shield_until_tick;
        client.server_engine_shutdown_start_sequence = self.server_engine_shutdown_start_sequence;
        client.server_engine_shutdown_until_tick = self.server_engine_shutdown_until_tick;
        client.pending_prize_effect_applications = self.pending_prize_effect_applications;
        client.prize_effect_application_terminals = self.prize_effect_application_terminals;
        client.server_emp_start_sequence = self.server_emp_start_sequence;
        client.server_emp_until_tick = self.server_emp_until_tick;
        client.server_prize_effect_rng_seed = self.server_prize_effect_rng_seed;
        client.server_prize_state_revision = self.server_prize_state_revision;
        client.last_damage_prize_loss_seq = self.last_damage_prize_loss_seq;
        client.last_damage_prize_loss_id = self.last_damage_prize_loss_id;
        client.server_rocket_until_tick = self.server_rocket_until_tick;
        client.server_rocket_input_asserted = self.server_rocket_input_asserted;
        client.server_rocket_authorizations = self.server_rocket_authorizations;
        client.server_status = self.server_status;
        client.server_weapons = self.server_weapons;
        client.last_reported_energy = self.last_reported_energy;
        client.last_position = self.last_position;
        client.objective_last_safe = self.objective_last_safe;
        client.visibility_cell = self.visibility_cell;
        client.last_position_server_tick = self.last_position_server_tick;
        client.movement_time_budget = self.movement_time_budget;
        client.last_position_relay_at = self.last_position_relay_at;
        client.position_relay_tokens = self.position_relay_tokens;
        client.position_relay_burst_count = self.position_relay_burst_count;
        client.last_weapon_fire_tick = self.last_weapon_fire_tick;
        client.last_gun_fire_tick = self.last_gun_fire_tick;
        client.last_bomb_fire_tick = self.last_bomb_fire_tick;
        client.objective_weapon_lock_until_tick = self.objective_weapon_lock_until_tick;
        client.last_accepted_weapon_data = self.last_accepted_weapon_data;
        client.last_accepted_weapon_tick = self.last_accepted_weapon_tick;
        client.accepted_weapon_witnesses = self.accepted_weapon_witnesses;
        client.objective_ball_fire_witnesses = self.objective_ball_fire_witnesses;
        client.last_special_weapon_request = self.last_special_weapon_request;
        client.last_brick_drop_tick = self.last_brick_drop_tick;
        client.last_frequency_change_at = self.last_frequency_change_at;
        client.frequency_change_burst_count = self.frequency_change_burst_count;
        client.alive = self.alive;
        client.respawn_at_tick = self.respawn_at_tick;
        client.life_started_tick = self.life_started_tick;
        client.last_input_seq = self.last_input_seq;
        client.last_input_record = self.last_input_record;
        client.recent_input_records = self.recent_input_records;
        client.recent_input_rocket_assertions = self.recent_input_rocket_assertions;
        client.last_position_sequence = self.last_position_sequence;
        client.life_position_sequence_base = self.life_position_sequence_base;
        client.pending_frequency_baseline = self.pending_frequency_baseline;
        client.pending_warp_prize_relocations = self.pending_warp_prize_relocations;
        client.consumed_warp_prize_relocations = self.consumed_warp_prize_relocations;
        client.latest_weapon_seq = self.latest_weapon_seq;
        client.weapon_seq_mask = self.weapon_seq_mask;
        client.latest_portal_action_id = self.latest_portal_action_id;
        client.portal_action_mask = self.portal_action_mask;
        client.portal_placement_terminals = self.portal_placement_terminals;
        client.portal_warp_terminals = self.portal_warp_terminals;
        client.active_portal = self.active_portal;
        client.server_warp_settle_until_tick = self.server_warp_settle_until_tick;
        client.server_warp_bomb_lock_until_tick = self.server_warp_bomb_lock_until_tick;
        client.canonical_position_witnesses = self.canonical_position_witnesses;
        client.canonical_movement_witness = self.canonical_movement_witness;
        client.server_sim_state = self.server_sim_state;
        client.server_sim_anchor_tick = self.server_sim_anchor_tick;
        client.server_sim_input_bits = self.server_sim_input_bits;
        client.movement_drift_x = self.movement_drift_x;
        client.movement_drift_y = self.movement_drift_y;
        client.server_thrust_acceleration = self.server_thrust_acceleration;
        client.server_top_speed = self.server_top_speed;
        client.server_rotation_per_second = self.server_rotation_per_second;
        client.server_recharge_per_second = self.server_recharge_per_second;
        client.server_repel_until_tick = self.server_repel_until_tick;
        client.server_repel_sequence_window = self.server_repel_sequence_window;
        client.server_repel_observer_until_tick = self.server_repel_observer_until_tick;
        client.pending_repel_observations = self.pending_repel_observations;
        client.retired_repel_applications = self.retired_repel_applications;
        client.position_history = self.position_history;
        client.actor_motion_history = self.actor_motion_history;
        client.aoi_tier_memory = self.aoi_tier_memory;
        client.pending_snapshot_events = self.pending_snapshot_events;
        client.pending_realtime_events = self.pending_realtime_events;
        client.pending_snapshot_weapon = self.pending_snapshot_weapon;
        client.recent_damagers = self.recent_damagers;
    }
}

fn allow_roster_change_burst(
    last_change_at: &mut Option<Instant>,
    burst_count: &mut u8,
    now: Instant,
) -> bool {
    allow_short_burst(
        last_change_at,
        burst_count,
        now,
        ROSTER_CHANGE_RATE_WINDOW,
        ROSTER_CHANGE_BURST_LIMIT,
    )
}

pub(crate) fn normalized_ship(ship: u8) -> u8 {
    ship.min(8)
}
