use super::super::super::{arena_settings, state, zone};
use std::collections::BTreeSet;

/// Choose a clear server-owned destination using the same spawn policy and
/// hostile-brick exclusions as random warp, without its voluntary costs.
pub(crate) fn brick_overlap_destination(
    state: &state::ServerState,
    frequency: u16,
    ship: u8,
    tick: u32,
) -> Option<(u16, u16)> {
    let blocked = state
        .active_bricks
        .iter()
        .filter(|brick| {
            !crate::continuum::continuum_tick_reached(tick, brick.expires_at_tick)
                && brick
                    .packet
                    .get(9..11)
                    .is_some_and(|bytes| u16::from_le_bytes(bytes.try_into().unwrap()) != frequency)
        })
        .flat_map(|brick| brick.tiles.iter().copied())
        .collect::<BTreeSet<_>>();
    let mut entropy = tick.wrapping_add(u32::from(frequency)).to_le_bytes();
    if let Err(error) = getrandom::fill(&mut entropy) {
        tracing::warn!(event = "brick_warp_entropy_unavailable", %error);
    }
    zone::server_random_warp_coordinates(
        state.zone_map(),
        frequency,
        state.clients.len() + state.bots.len(),
        arena_settings::arena_ship_radius_pixels(ship),
        if state.config.free_for_all {
            zone::RandomWarpArea::CentralCombat
        } else {
            zone::RandomWarpArea::ArenaSettings
        },
        &mut i32::from_le_bytes(entropy),
        &blocked,
    )
}

fn overlaps_hostile_brick(
    state: &state::ServerState,
    x: f32,
    y: f32,
    ship: u8,
    frequency: u16,
    tick: u32,
) -> bool {
    let radius = arena_settings::arena_ship_radius_pixels(ship);
    state.active_bricks.iter().any(|brick| {
        !crate::continuum::continuum_tick_reached(tick, brick.expires_at_tick)
            && brick
                .packet
                .get(9..11)
                .is_some_and(|bytes| u16::from_le_bytes(bytes.try_into().unwrap()) != frequency)
            && brick.tiles.iter().any(|&(tx, ty)| {
                let left = f32::from(tx) * zone::TILE_SIZE_PX;
                let top = f32::from(ty) * zone::TILE_SIZE_PX;
                crate::gameplay_rules::reference_game_player_hull_overlaps_tile(
                    x - radius,
                    y - radius,
                    x + radius,
                    y + radius,
                    left,
                    top,
                    left + zone::TILE_SIZE_PX,
                    top + zone::TILE_SIZE_PX,
                )
            })
    })
}

/// Retry after other movement/lifecycle transactions finish. Merely offering
/// the correction never moves a human: its exact paused-tail receipt commits
/// the server-selected relocation. Bots are simulated entirely by the server.
pub(crate) fn queue_enemy_brick_overlap_corrections(state: &mut state::ServerState, tick: u32) {
    if state.active_bricks.is_empty() {
        return;
    }
    let humans = state
        .clients
        .iter()
        .filter_map(|(&id, client)| {
            if !client.joined
                || !client.alive
                || client.player.ship >= 8
                || client.pending_movement_correction.is_some()
                || !client.pending_warp_prize_relocations.is_empty()
            {
                return None;
            }
            let witness = client.canonical_movement_witness?;
            (witness.life_seq == client.life_seq
                && Some(witness.sequence) == client.last_position_sequence
                && overlaps_hostile_brick(
                    state,
                    witness.state.position_x,
                    witness.state.position_y,
                    client.player.ship,
                    client.player.frequency,
                    tick,
                ))
            .then_some((id, client.player.frequency, client.player.ship, witness))
        })
        .collect::<Vec<_>>();
    for (id, frequency, ship, mut witness) in humans {
        let Some((x, y)) = brick_overlap_destination(state, frequency, ship, tick) else {
            continue;
        };
        let client = state.clients.get_mut(&id).expect("retained current client");
        let detected_state = witness.state;
        witness.state.position_x = f32::from(x);
        witness.state.position_y = f32::from(y);
        witness.state.velocity_x = 0.0;
        witness.state.velocity_y = 0.0;
        client.next_movement_correction_generation = client
            .next_movement_correction_generation
            .wrapping_add(1)
            .max(1);
        let proposal = crate::movement_correction::MovementCorrection {
            reason: crate::movement_correction::MovementCorrection::REASON_BRICK_OVERLAP,
            repel_window: None,
            server_session_id: crate::wire::server_session_id(),
            player_id: id,
            life_seq: client.life_seq,
            correction_generation: client.next_movement_correction_generation,
            position_sequence: witness.sequence,
            server_tick: tick,
            position_x: witness.state.position_x,
            position_y: witness.state.position_y,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: witness.state.orientation,
        };
        client.pending_movement_correction = Some(state::ServerMovementCorrection {
            proposal,
            detected_state,
            replay_witness: Some(witness),
            replay_server_tick: tick,
            predicted_repel_window: None,
            offer_reliable_id: None,
            application_sequence: None,
            terminal_state: None,
            terminal_server_tick: None,
            cancelled: false,
            receipt_reliable_id: None,
        });
        tracing::info!(
            event = "brick_overlap_correction_offered",
            player_id = id,
            life_seq = client.life_seq,
            server_tick = tick,
            destination_x = x,
            destination_y = y
        );
    }
    let bots = state
        .bots
        .iter()
        .filter(|bot| {
            bot.alive()
                && overlaps_hostile_brick(
                    state,
                    bot.x,
                    bot.y,
                    bot.player.ship,
                    bot.player.frequency,
                    tick,
                )
        })
        .map(|bot| (bot.player.id, bot.player.frequency, bot.player.ship))
        .collect::<Vec<_>>();
    for (id, frequency, ship) in bots {
        let Some((x, y)) = brick_overlap_destination(state, frequency, ship, tick) else {
            continue;
        };
        let bot = state.bot_mut(id).expect("retained bot");
        bot.x = f32::from(x);
        bot.y = f32::from(y);
        bot.velocity_x = 0.0;
        bot.velocity_y = 0.0;
        bot.safe_egress_path.clear();
        bot.begin_actor_motion_segment();
        let safe = zone::world_position_in_safe_tile_for_map(state.zone_map(), x, y);
        let bot = state.bot_mut(id).expect("retained bot");
        let appended = bot.append_actor_motion_sample(tick, safe);
        debug_assert_eq!(
            appended,
            crate::combat_timeline::HistoryAppendDisposition::Applied
        );
        tracing::info!(
            event = "brick_overlap_bot_relocated",
            player_id = id,
            life_seq = bot.life_seq,
            server_tick = tick,
            destination_x = x,
            destination_y = y
        );
    }
}
