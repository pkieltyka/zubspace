use std::sync::{Arc, Mutex};

use super::objectives::{
    clear_bricks_packet, create_turret_link, drop_flag_packet, initialize_objectives,
    objective_action_rejection_packets_for_registry, objective_state_replay_packets,
    powerball_removal_packet,
};
use super::reliable::{broadcast_reliable_to_joined, retry_pending_movement_correction_delivery};
use super::state::{
    PendingReliableGameplayMutationKind, ReliableLifecycleMutationOutcome,
    ResetApplicationTerminal, ServerState,
};
use super::wire::current_tick;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AdminCommand {
    Help,
    Status,
    Players,
    Reset,
    Shutdown,
    Unknown(String),
}

pub(crate) fn parse_admin_command(line: &str) -> Option<AdminCommand> {
    let command = line.trim();
    if command.is_empty() {
        return None;
    }
    Some(match command.to_ascii_lowercase().as_str() {
        "help" | "?" => AdminCommand::Help,
        "status" | "stats" => AdminCommand::Status,
        "players" | "roster" => AdminCommand::Players,
        "reset" | "reset-arena" | "arena-reset" => AdminCommand::Reset,
        "quit" | "exit" | "shutdown" => AdminCommand::Shutdown,
        _ => AdminCommand::Unknown(command.to_string()),
    })
}

pub(crate) fn admin_command_response(
    state: &Arc<Mutex<ServerState>>,
    command: &AdminCommand,
) -> String {
    match command {
        AdminCommand::Help => "commands: status, players, reset, shutdown".to_string(),
        AdminCommand::Status => server_status(state),
        AdminCommand::Players => player_listing(state),
        AdminCommand::Reset => reset_arena(state),
        AdminCommand::Shutdown => "shutdown requested".to_string(),
        AdminCommand::Unknown(command) => {
            format!("unknown command: {command}; try help")
        }
    }
}

fn reset_arena(state: &Arc<Mutex<ServerState>>) -> String {
    reset_arena_at_tick(state, current_tick())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn reset_arena_at_tick(state: &Arc<Mutex<ServerState>>, tick: u32) -> String {
    let arena_gate = state
        .lock()
        .expect("server state should not be poisoned")
        .arena_transaction_gate
        .clone();
    let _arena_transaction = arena_gate
        .write()
        .expect("arena transaction gate should not be poisoned");
    let ingress_gates = {
        let state = state.lock().expect("server state should not be poisoned");
        let mut gates = state
            .clients
            .iter()
            .map(|(&player_id, client)| (player_id, client.ingress_gate.clone()))
            .collect::<Vec<_>>();
        gates.sort_unstable_by_key(|(player_id, _)| *player_id);
        gates
    };
    let _ingress_transactions = ingress_gates
        .iter()
        .map(|(_, gate)| {
            gate.lock()
                .expect("client ingress gate should not be poisoned")
        })
        .collect::<Vec<_>>();

    // Reserve one generation for every retained Leave's possible quarantine
    // recovery plus the final reset. The staged registry is therefore newer
    // than any cleanup publication, even when an exhausted object version
    // forces every Leave through recovery. No live mutation occurs until the
    // complete placement and reset-terminal closure set has been staged.
    let (staged_objectives, staged_terminals) = {
        let state = state.lock().expect("server state should not be poisoned");
        let pending_leave_count = state.pending_leave_identities().len();
        let staged = match stage_objective_reset(&state, tick, pending_leave_count + 1) {
            Ok(staged) => staged,
            Err(error) => return format!("arena reset failed: {error}"),
        };
        let mut terminals = Vec::new();
        for (&player_id, client) in &state.clients {
            let Some(identity) = state.active_player_runtime_identity(player_id) else {
                continue;
            };
            let mut runtime_terminals = Vec::new();
            for pending in state.reliable_gameplay_items_for_runtime(identity) {
                match pending.kind {
                    PendingReliableGameplayMutationKind::Objective(objective) => {
                        for packet in objective_action_rejection_packets_for_registry(
                            &staged,
                            &[objective],
                            tick,
                        ) {
                            let Ok(packet) = packet.try_into() else {
                                return "arena reset failed: invalid objective terminal framing"
                                    .to_string();
                            };
                            runtime_terminals.push(ResetApplicationTerminal::Objective(packet));
                        }
                    }
                    PendingReliableGameplayMutationKind::MovementCorrection(application) => {
                        if let Some((receipt, _)) =
                            super::arena::cancelled_movement_correction_receipt_for_state(
                                &state,
                                identity,
                                application,
                            )
                        {
                            runtime_terminals
                                .push(ResetApplicationTerminal::MovementCorrection(receipt));
                        }
                    }
                    _ => {}
                }
            }
            if client.pending_reset_application_terminals.len() + runtime_terminals.len()
                > super::state::MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME
            {
                return "arena reset deferred: application terminal capacity unavailable"
                    .to_string();
            }
            terminals.push((identity.session, runtime_terminals));
        }
        (staged, terminals)
    };
    if !commit_pending_leaves_before_reset(state, tick) {
        return "arena reset deferred: pending leave cleanup unavailable".to_string();
    }
    let (
        flag_carriers,
        turret_children,
        powerball_ids,
        brick_count,
        joined_count,
        objective_replay_packets,
    ) = {
        let mut state = state.lock().expect("server state should not be poisoned");
        debug_assert!(
            staged_objectives.replay().generation > state.objectives.replay().generation,
            "staged reset generation must follow every cleanup publication"
        );
        for (session, terminals) in staged_terminals {
            if !state.player_session_matches(session) {
                continue;
            }
            let client = state
                .clients
                .get_mut(&session.player_id)
                .expect("matching reset session remains registered");
            client.pending_reset_application_terminals.extend(terminals);
        }
        let objective_replay = state.objectives.replay();
        let flag_carriers = objective_replay
            .flags
            .iter()
            .filter_map(|flag| match flag.state {
                crate::objectives::FlagState::Carried { carrier, .. } => Some(carrier.player_id()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let turret_children = state.turret_links.keys().copied().collect::<Vec<_>>();
        let powerball_ids = objective_replay
            .powerballs
            .iter()
            .filter_map(|ball| {
                (!matches!(
                    ball.state,
                    crate::objectives::PowerballState::Waiting { .. }
                ))
                .then_some(ball.identity.id)
            })
            .collect::<Vec<_>>();
        let brick_count = state.active_bricks.len();

        state.turret_links.clear();
        state.active_bricks.clear();
        state.active_projectiles.clear();
        state.active_decoys.clear();
        state.pending_owner_weapon_cancellations.clear();
        state.owner_weapon_cancellation_watermarks.clear();
        state.latest_owner_weapon_groups.clear();
        state.pending_damage_transactions.clear();
        state.retired_projectile_owners.clear();
        state.active_hidden_prizes.clear();
        state.next_hidden_prize_spawn_at_tick = None;
        state.claimed_prizes.clear();
        state.visibility_index = Default::default();
        state.pending_position_inputs.clear();
        state.input_recovery_deferred_at.clear();
        state.arena_tick = 0;
        state.last_tick_at = None;
        state.objectives = staged_objectives;

        let joined_runtimes = state
            .clients
            .iter()
            .filter_map(|(&player_id, client)| {
                client
                    .joined
                    .then(|| state.active_player_runtime_identity(player_id))
                    .flatten()
            })
            .collect::<Vec<_>>();
        for identity in joined_runtimes {
            let player_id = identity.player_id();
            state.clear_pending_position_inputs_for_player(player_id);
            state.retire_pending_applications_for_runtime(identity);
            let _ = state.advance_client_life_at_tick(player_id, tick);
        }
        let mut joined_count = 0;
        for client in state.clients.values_mut().filter(|client| client.joined) {
            joined_count += 1;
            client.player.flags = 0;
            client.alive = client.player.ship < 8;
            client.respawn_at_tick = None;
            client.last_position = None;
            client.aoi_observer_position = None;
            client.objective_last_safe = false;
            client.visibility_cell = None;
            client.last_position_server_tick = None;
            client.last_position_sequence = None;
            client.last_position_relay_at = None;
            client.position_relay_tokens = 3.0;
            client.position_relay_burst_count = 0;
            client.reset_server_runtime_for_ship_at_tick(tick);
            client.last_reported_energy = 0;
        }

        let objective_replay_packets = objective_state_replay_packets(&state, tick);
        (
            flag_carriers,
            turret_children,
            powerball_ids,
            brick_count,
            joined_count,
            objective_replay_packets,
        )
    };

    // Reserve reliable ids for ordered application terminals before reset
    // replay/fanout. If the sender is saturated they remain in the bounded
    // session queue and the ordinary tick retry path eventually closes them.
    retry_pending_movement_correction_delivery(state);

    for carrier_id in flag_carriers {
        broadcast_reliable_to_joined(state, drop_flag_packet(carrier_id));
    }
    for child_id in turret_children {
        broadcast_reliable_to_joined(state, create_turret_link(child_id, u16::MAX));
    }
    broadcast_reliable_to_joined(state, clear_bricks_packet());
    for ball_id in powerball_ids {
        broadcast_reliable_to_joined(state, powerball_removal_packet(ball_id));
    }
    for packet in objective_replay_packets {
        broadcast_reliable_to_joined(state, packet);
    }

    format!("arena reset: joined={joined_count} bricks_cleared={brick_count}")
}

fn stage_objective_reset(
    state: &ServerState,
    tick: u32,
    generation_advances: usize,
) -> Result<crate::objectives::ObjectiveRegistry, crate::objectives::ObjectiveRegistryError> {
    let mut staged = state.objectives.clone();
    let profile = staged.profile().clone();
    for _ in 0..generation_advances {
        staged.reset(profile.clone())?;
    }
    let map_tiles = state.zone_map().tiles.clone();
    let no_flag_drop_tiles = state.zone_map().no_flag_drop_tiles.clone();
    initialize_objectives(
        &mut staged,
        tick,
        |x, y| {
            map_tiles
                .get(usize::from(y) * crate::zone::MAP_TILES + usize::from(x))
                .copied()
                .is_none_or(|tile| tile != 0)
        },
        |x, y| {
            crate::objectives::FlagDropTile::from_map_tile(
                map_tiles
                    .get(usize::from(y) * crate::zone::MAP_TILES + usize::from(x))
                    .copied()
                    .unwrap_or(20),
                no_flag_drop_tiles.contains(x, y),
            )
        },
    )?;
    Ok(staged)
}

/// A connection closes its RTC-local joined gate as soon as Leave is
/// admitted, while authoritative `ClientHandle::joined` changes only when the
/// ordered mutation commits. An admin reset must not erase that retained
/// Leave and leave the two gates permanently split. Commit it (including
/// objective cleanup and leave fanout) before advancing/resetting joined
/// runtimes. If mandatory cleanup cannot commit, defer the reset and keep the
/// resident queue intact for retry.
fn commit_pending_leaves_before_reset(state: &Arc<Mutex<ServerState>>, tick: u32) -> bool {
    let identities = state
        .lock()
        .expect("server state should not be poisoned")
        .pending_leave_identities();
    for identity in identities {
        if !state
            .lock()
            .expect("server state should not be poisoned")
            .player_runtime_matches(identity)
        {
            state
                .lock()
                .expect("server state should not be poisoned")
                .retire_pending_applications_for_runtime(identity);
            continue;
        }
        let outcome = super::arena::leave_arena_for_runtime(
            state,
            Some(identity),
            identity.player_id(),
            tick,
        );
        if outcome == ReliableLifecycleMutationOutcome::Deferred {
            return false;
        }
        state
            .lock()
            .expect("server state should not be poisoned")
            .retire_pending_applications_for_runtime(identity);
    }
    true
}

fn server_status(state: &Arc<Mutex<ServerState>>) -> String {
    let state = state.lock().expect("server state should not be poisoned");
    let joined = state
        .clients
        .values()
        .filter(|client| client.joined)
        .count();
    format!(
        "zone={} clients={} joined={} max_players={} configured_bots={} active_bots={} tick_rate={}Hz tick={}",
        state.config.default_zone,
        state.clients.len(),
        joined,
        state.config.max_players,
        state.config.bot_count,
        state.bots.len(),
        state.config.tick_rate_hz,
        state.arena_tick
    )
}

fn player_listing(state: &Arc<Mutex<ServerState>>) -> String {
    let state = state.lock().expect("server state should not be poisoned");
    let mut players: Vec<_> = state
        .clients
        .values()
        .filter(|client| client.joined)
        .map(|client| client.player.clone())
        .collect();
    players.extend(state.bots.iter().map(|bot| bot.player.clone()));
    players.sort_by_key(|player| player.id);
    if players.is_empty() {
        return "players: none".to_string();
    }

    let rows = players
        .into_iter()
        .map(|player| {
            format!(
                "#{} {} ship={} freq={} score={}/{}/{}",
                player.id,
                player.name,
                player.ship,
                player.frequency,
                player.wins,
                player.assists,
                player.losses
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("players: {rows}")
}
