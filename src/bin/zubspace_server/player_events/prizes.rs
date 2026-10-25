use super::super::arena_settings::{
    arena_prize_claim_retention_ticks, arena_ship_prize_share_limit, arena_ship_radius_pixels,
};
use super::super::hidden_prizes::{active_collectible_claim, remove_active_collectible};
use super::super::reliable::{broadcast_reliable_except, send_reliable_to};
use super::super::state::{
    ClientHandle, PendingPrizeEffectApplication, PendingWarpPrizeRelocation, ServerPrizeClaim,
    ServerPrizeReceipt, ServerState,
};
use super::super::zone::TILE_SIZE_PX;
use std::sync::{Arc, Mutex};

pub(crate) const TAKE_PRIZE_REQUEST: u8 = 0x07;
const PLAYER_PRIZE: u8 = 0x08;
const WARP_PRIZE_PROTOCOL_ID: i16 = 7;

fn movement_tuning_bits(client: &ClientHandle) -> [u32; 4] {
    [
        client.server_thrust_acceleration.to_bits(),
        client.server_top_speed.to_bits(),
        client.server_rotation_per_second.to_bits(),
        client.server_recharge_per_second.to_bits(),
    ]
}

fn prize_may_change_movement_tuning(prize_id: i16) -> bool {
    matches!(prize_id.checked_abs(), Some(1 | 3 | 11 | 12)) || prize_id == 25
}

fn remember_prize_effect_terminal(
    client: &mut ClientHandle,
    terminal: crate::prize_application::PrizeEffectAccepted,
) -> crate::prize_application::PrizeEffectAccepted {
    client.remember_prize_effect_application_terminal(terminal);
    terminal
}

pub(crate) fn apply_prize_effect_application(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    application: crate::prize_application::PrizeEffectApplied,
) -> Option<crate::prize_application::PrizeEffectAccepted> {
    let mut state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get_mut(&player_id)?;
    let terminal = |outcome, application_sequence| crate::prize_application::PrizeEffectAccepted {
        server_session_id: application.server_session_id,
        player_id: application.player_id,
        life_seq: application.life_seq,
        collectible_id: application.collectible_id,
        prize_id: application.prize_id,
        prize_state_revision: application.prize_state_revision,
        requested_application_sequence: application.application_sequence,
        application_sequence,
        outcome,
    };
    if let Some(known) = client
        .prize_effect_application_terminals
        .iter()
        .find(|known| {
            known.server_session_id == application.server_session_id
                && known.player_id == application.player_id
                && known.life_seq == application.life_seq
                && known.collectible_id == application.collectible_id
                && known.prize_id == application.prize_id
                && known.prize_state_revision == application.prize_state_revision
                && known.requested_application_sequence == application.application_sequence
        })
        .copied()
    {
        return Some(known);
    }
    if application.server_session_id != crate::wire::server_session_id()
        || application.player_id != player_id
        || !client.joined
        || !client.alive
        || client.life_seq != application.life_seq
    {
        let cancelled = terminal(
            crate::prize_application::PrizeEffectAccepted::OUTCOME_CANCELLED,
            application.application_sequence,
        );
        return Some(remember_prize_effect_terminal(client, cancelled));
    }
    let Some(pending_index) = client
        .pending_prize_effect_applications
        .iter()
        .position(|pending| {
            pending.collectible_id == application.collectible_id
                && pending.prize_id == application.prize_id
                && pending.life_seq == application.life_seq
                && pending.prize_state_revision == application.prize_state_revision
        })
    else {
        let cancelled = terminal(
            crate::prize_application::PrizeEffectAccepted::OUTCOME_CANCELLED,
            application.application_sequence,
        );
        return Some(remember_prize_effect_terminal(client, cancelled));
    };
    client.pending_prize_effect_applications[pending_index].latest_application_sequence =
        Some(application.application_sequence);
    if pending_index != 0 {
        let head_boundary = client.pending_prize_effect_applications[0]
            .maximum_application_sequence
            .or(client.pending_prize_effect_applications[0].minimum_application_sequence)
            .unwrap_or(application.application_sequence);
        let forward = head_boundary.wrapping_sub(application.application_sequence);
        if forward != 0 && forward < (1 << 15) {
            let rebased = terminal(
                crate::prize_application::PrizeEffectAccepted::OUTCOME_REBASE,
                head_boundary,
            );
            return Some(remember_prize_effect_terminal(client, rebased));
        }
        client
            .pending_prize_effect_applications
            .remove(pending_index);
        let cancelled = terminal(
            crate::prize_application::PrizeEffectAccepted::OUTCOME_CANCELLED,
            application.application_sequence,
        );
        return Some(remember_prize_effect_terminal(client, cancelled));
    }
    let pending = client.pending_prize_effect_applications[pending_index];
    let exact_boundary = pending
        .maximum_application_sequence
        .or(pending.minimum_application_sequence)
        .unwrap_or(application.application_sequence);
    let advance_from_exact = application
        .application_sequence
        .wrapping_sub(exact_boundary);
    if application.application_sequence != exact_boundary
        && advance_from_exact != 0
        && advance_from_exact < (1 << 15)
    {
        if advance_from_exact as usize <= crate::state::MAX_RECENT_INPUT_OBSERVATIONS
            && pending
                .deferred_application
                .is_none_or(|known| known == application)
        {
            // The reliable marker overtook one or more already-generated
            // unordered old-semantics inputs. Hold the declaration until
            // input ingress observes its exact predecessor; never rebase it
            // backward to the merely-observed boundary.
            client.pending_prize_effect_applications[pending_index].deferred_application =
                Some(application);
            return None;
        }
        client
            .pending_prize_effect_applications
            .remove(pending_index);
        let cancelled = terminal(
            crate::prize_application::PrizeEffectAccepted::OUTCOME_CANCELLED,
            application.application_sequence,
        );
        return Some(remember_prize_effect_terminal(client, cancelled));
    }
    if application.application_sequence != exact_boundary {
        // The exact server successor is ahead of the marker. This is the one
        // safe REBASE direction: it drains observed old-semantics lineage and
        // can never reinterpret an omitted tail under Shutdown.
        let rebased = terminal(
            crate::prize_application::PrizeEffectAccepted::OUTCOME_REBASE,
            exact_boundary,
        );
        return Some(remember_prize_effect_terminal(client, rebased));
    }
    let accepted = terminal(
        crate::prize_application::PrizeEffectAccepted::OUTCOME_APPLIED,
        application.application_sequence,
    );
    client
        .pending_prize_effect_applications
        .remove(pending_index);
    // The first accepted Shutdown application owns the start of the active
    // interval. Later stacked claims already run under the same semantics and
    // must not reinterpret the sequences between their two receipts.
    client
        .server_engine_shutdown_start_sequence
        .get_or_insert(application.application_sequence);
    remember_prize_effect_terminal(client, accepted);
    tracing::debug!(
        player_id,
        collectible_id = application.collectible_id,
        prize_id = application.prize_id,
        life_seq = application.life_seq,
        prize_state_revision = application.prize_state_revision,
        application_sequence = application.application_sequence,
        "server prize effect application accepted"
    );
    Some(accepted)
}

pub(crate) fn relay_prize_pickup(state: &Arc<Mutex<ServerState>>, player_id: u16, request: &[u8]) {
    relay_prize_pickup_at_tick(state, player_id, request, crate::wire::current_tick());
}

pub(crate) fn relay_prize_pickup_at_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    request: &[u8],
    tick: u32,
) {
    let _ = relay_prize_pickup_for_runtime_at_tick(state, None, player_id, request, tick);
}

pub(crate) fn relay_prize_pickup_for_runtime_at_tick(
    state: &Arc<Mutex<ServerState>>,
    expected_identity: Option<crate::authority_identity::PlayerRuntimeIdentity>,
    player_id: u16,
    request: &[u8],
    tick: u32,
) -> crate::state::ReliableLifecycleMutationOutcome {
    let Some(collectible_id) = prize_claim_collectible_id(request) else {
        return crate::state::ReliableLifecycleMutationOutcome::TerminalNoop;
    };
    let Some(result) = claim_prize(state, expected_identity, player_id, collectible_id, tick)
    else {
        return crate::state::ReliableLifecycleMutationOutcome::TerminalNoop;
    };
    for recipient in result.recipient_acks {
        send_reliable_to(
            state,
            recipient.player_id,
            prize_pickup_ack_from_claim(result.claim, recipient),
        );
    }
    if result.newly_collected {
        let packet = player_prize_from_claim(player_id, result.claim);
        broadcast_reliable_except(state, player_id, packet);
    }
    crate::state::ReliableLifecycleMutationOutcome::Applied
}

pub(crate) fn player_prize_from_claim(player_id: u16, claim: ServerPrizeClaim) -> Vec<u8> {
    let mut payload = Vec::with_capacity(13);
    payload.push(PLAYER_PRIZE);
    payload.extend(claim.collectible_id.to_le_bytes());
    payload.extend(claim.x.to_le_bytes());
    payload.extend(claim.y.to_le_bytes());
    payload.extend(claim.prize_id.to_le_bytes());
    payload.extend(player_id.to_le_bytes());
    payload
}

pub(crate) fn prize_pickup_ack_from_claim(
    claim: ServerPrizeClaim,
    recipient: PrizeRecipientAck,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(44);
    payload.push(0x61);
    payload.extend(claim.collectible_id.to_le_bytes());
    payload.extend(claim.x.to_le_bytes());
    payload.extend(claim.y.to_le_bytes());
    payload.extend(claim.prize_id.to_le_bytes());
    payload.extend(recipient.authoritative_energy.to_le_bytes());
    payload.extend(recipient.life_seq.to_le_bytes());
    payload.extend(recipient.prize_state_revision.to_le_bytes());
    payload.extend(
        recipient
            .engine_shutdown_until_tick
            .unwrap_or(0)
            .to_le_bytes(),
    );
    payload.push(
        u8::from(recipient.engine_shutdown_application_required)
            | (u8::from(recipient.engine_shutdown_until_tick.is_some()) << 1),
    );
    payload.extend(crate::wire::server_session_id().to_le_bytes());
    payload.extend(recipient.arena_stream_id.to_le_bytes());
    payload.extend(recipient.player_id.to_le_bytes());
    payload
}

fn prize_claim_collectible_id(request: &[u8]) -> Option<u32> {
    if request.len() != 11 || request[0] != TAKE_PRIZE_REQUEST {
        return None;
    }
    let collectible_id = u32::from_le_bytes(request[1..5].try_into().ok()?);
    (collectible_id != 0).then_some(collectible_id)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PrizeClaimResult {
    claim: ServerPrizeClaim,
    newly_collected: bool,
    recipient_acks: Vec<PrizeRecipientAck>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrizeRecipientAck {
    pub(crate) player_id: u16,
    pub(crate) arena_stream_id: u64,
    pub(crate) authoritative_energy: u16,
    pub(crate) life_seq: u32,
    pub(crate) prize_state_revision: u32,
    pub(crate) engine_shutdown_until_tick: Option<u32>,
    pub(crate) engine_shutdown_application_required: bool,
}

fn claim_prize(
    state: &Arc<Mutex<ServerState>>,
    expected_identity: Option<crate::authority_identity::PlayerRuntimeIdentity>,
    player_id: u16,
    collectible_id: u32,
    tick: u32,
) -> Option<PrizeClaimResult> {
    let mut state = state.lock().expect("server state should not be poisoned");
    let winner = state.active_player_runtime_identity(player_id)?;
    if expected_identity.is_some_and(|expected| expected != winner) {
        return None;
    }
    let client = state.clients.get(&player_id)?;
    if !client.joined || !client.alive || client.player.ship >= 8 {
        return None;
    }
    state.expire_prize_claims_older_than(tick, arena_prize_claim_retention_ticks());
    if let Some(receipt) = state.claimed_prizes.get(&collectible_id).copied() {
        let pickup_ack_ready = receipt.claim.prize_id != WARP_PRIZE_PROTOCOL_ID
            || state.clients[&player_id]
                .pending_warp_prize_relocations
                .iter()
                .find(|pending| pending.collectible_id == collectible_id)
                .is_none_or(|pending| pending.pickup_ack_sent);
        return (receipt.winner == winner).then_some(PrizeClaimResult {
            claim: receipt.claim,
            newly_collected: false,
            recipient_acks: pickup_ack_ready
                .then_some(PrizeRecipientAck {
                    player_id,
                    arena_stream_id: state.clients[&player_id].arena_stream_id,
                    authoritative_energy: receipt.authoritative_energy,
                    life_seq: receipt.winner.life.life_seq,
                    prize_state_revision: receipt.prize_state_revision,
                    engine_shutdown_until_tick: receipt.engine_shutdown_until_tick,
                    engine_shutdown_application_required: receipt
                        .engine_shutdown_application_required,
                })
                .into_iter()
                .collect(),
        });
    }
    let claim = active_collectible_claim(&mut state, collectible_id, tick)?;
    let client = state.clients.get(&player_id)?;
    if !prize_claim_overlaps_last_position(
        client.player.ship,
        client.last_position.as_deref(),
        claim,
    ) {
        return None;
    }
    if !prize_claim_id_valid(claim.prize_id) {
        return None;
    }
    if claim.prize_id == WARP_PRIZE_PROTOCOL_ID
        && state.clients[&player_id]
            .pending_warp_prize_relocations
            .len()
            >= crate::state::MAX_PENDING_WARP_PRIZE_RELOCATIONS
    {
        tracing::warn!(
            player_id,
            collectible_id,
            "rejecting Warp prize claim while reliable relocation queue is full"
        );
        return None;
    }
    let collector_frequency = state.clients[&player_id].player.frequency;
    let mut authoritative_recipients = vec![player_id];
    if claim.prize_id != WARP_PRIZE_PROTOCOL_ID {
        authoritative_recipients.extend(state.clients.iter().filter_map(
            |(&candidate_id, client)| {
                (candidate_id != player_id
                    && client.joined
                    && client.alive
                    && client.player.ship < 8
                    && client.player.frequency == collector_frequency
                    && client.server_bounty < arena_ship_prize_share_limit(client.player.ship))
                .then_some(candidate_id)
            },
        ));
    }
    authoritative_recipients.sort_unstable();
    if (claim.prize_id.checked_abs() == Some(14)
        || prize_may_change_movement_tuning(claim.prize_id))
        && authoritative_recipients.iter().any(|recipient_id| {
            state.clients[recipient_id]
                .pending_prize_effect_applications
                .len()
                >= crate::state::MAX_PENDING_PRIZE_EFFECT_APPLICATIONS
        })
    {
        return None;
    }
    if !remove_active_collectible(&mut state, collectible_id) {
        return None;
    }
    let mut movement_tuning_before = -1.0_f32;
    let mut movement_tuning_after = -1.0_f32;
    let mut pickup_ack_ready = true;
    let mut recipient_acks = Vec::with_capacity(authoritative_recipients.len());
    for recipient_id in authoritative_recipients.iter().copied() {
        let Some(client) = state.clients.get_mut(&recipient_id) else {
            return None;
        };
        if recipient_id == player_id {
            movement_tuning_before =
                server_movement_tuning_for_prize(client, claim.prize_id).unwrap_or(-1.0);
        }
        if let Some(bounty_gain) = prize_claim_bounty_gain(claim) {
            client.server_bounty = client.server_bounty.saturating_add(bounty_gain);
        }
        let is_shutdown = claim.prize_id.checked_abs() == Some(14);
        let shutdown_semantics_were_active =
            is_shutdown && client.server_prize_engine_shutdown_active(tick);
        if is_shutdown && !shutdown_semantics_were_active {
            // An expired or never-applied predecessor is terminal before a
            // new claim opens its own exact marker identity. It cannot remain
            // at the FIFO head and consume the new claim's declaration.
            client.cancel_pending_prize_effect_applications();
        }
        let movement_tuning_before_bits = movement_tuning_bits(client);
        client.apply_server_prize_effect_at_tick(claim.prize_id, tick);
        let movement_application_required = prize_may_change_movement_tuning(claim.prize_id)
            && movement_tuning_bits(client) != movement_tuning_before_bits;
        let shutdown_application_required = is_shutdown
            && client.server_engine_shutdown_until_tick.is_some()
            && !shutdown_semantics_were_active;
        let effect_application_required =
            shutdown_application_required || movement_application_required;
        if effect_application_required {
            client
                .pending_prize_effect_applications
                .push_back(PendingPrizeEffectApplication {
                    collectible_id: claim.collectible_id,
                    prize_id: claim.prize_id,
                    life_seq: client.life_seq,
                    prize_state_revision: client.server_prize_state_revision,
                    minimum_application_sequence: client
                        .last_input_seq
                        .map(|sequence| sequence.wrapping_add(1)),
                    maximum_application_sequence: client
                        .last_input_seq
                        .map(|sequence| sequence.wrapping_add(1)),
                    latest_application_sequence: None,
                    deferred_application: None,
                    movement_tuning_before_bits: movement_application_required
                        .then_some(movement_tuning_before_bits),
                });
        }
        if is_shutdown {
            if shutdown_application_required {
            } else if client.server_engine_shutdown_until_tick.is_none() {
                // EngineShutdownTime=0 is an exact authoritative no-op. It
                // cannot leave an older application FIFO paused.
                client.cancel_pending_prize_effect_applications();
                client.server_engine_shutdown_start_sequence = None;
            } else if client.server_engine_shutdown_start_sequence.is_none() {
                // EMP already owns the same movement semantics. Continue the
                // prize source from that exact established sequence so EMP
                // expiry cannot open a one-frame movement/recharge gap.
                client.server_engine_shutdown_start_sequence =
                    client.server_emp_start_sequence.or_else(|| {
                        client
                            .last_input_seq
                            .map(|sequence| sequence.wrapping_add(1))
                    });
            }
        }
        if recipient_id == player_id && claim.prize_id == WARP_PRIZE_PROTOCOL_ID {
            pickup_ack_ready = client.pending_repel_observations.is_empty();
            // The reliable pickup acknowledgement can be overtaken by old
            // realtime positions. Keep ordinary tracking alive until the
            // exact neutral-velocity spawn report arrives, then the position
            // admission path consumes this one-shot authorization and rebases
            // before measuring movement deviation.
            client
                .pending_warp_prize_relocations
                .push_back(PendingWarpPrizeRelocation {
                    collectible_id: claim.collectible_id,
                    life_seq: client.life_seq,
                    expected_energy: client.server_energy,
                    expected_bounty: client.server_bounty,
                    prize_state_revision: client.server_prize_state_revision,
                    claim,
                    pickup_ack_sent: pickup_ack_ready,
                });
        }
        if recipient_id == player_id {
            movement_tuning_after =
                server_movement_tuning_for_prize(client, claim.prize_id).unwrap_or(-1.0);
        }
        if claim.prize_id != WARP_PRIZE_PROTOCOL_ID || recipient_id == player_id {
            recipient_acks.push(PrizeRecipientAck {
                player_id: recipient_id,
                arena_stream_id: client.arena_stream_id,
                authoritative_energy: client.server_energy,
                life_seq: client.life_seq,
                prize_state_revision: client.server_prize_state_revision,
                engine_shutdown_until_tick: is_shutdown
                    .then(|| client.server_engine_shutdown_deadline_for_delivery(tick))
                    .flatten(),
                engine_shutdown_application_required: effect_application_required,
            });
        }
    }
    let authoritative_energy = state
        .clients
        .get(&player_id)
        .map_or(0, |client| client.server_energy);
    let engine_shutdown_until_tick = state
        .clients
        .get(&player_id)
        .and_then(|client| client.server_engine_shutdown_deadline_for_delivery(tick))
        .unwrap_or(0);
    let winner_prize_state_revision = state.clients[&player_id].server_prize_state_revision;
    let winner_engine_shutdown_until_tick = (claim.prize_id.checked_abs() == Some(14))
        .then(|| state.clients[&player_id].server_engine_shutdown_deadline_for_delivery(tick))
        .flatten();
    let winner_engine_shutdown_application_required = recipient_acks
        .iter()
        .find(|recipient| recipient.player_id == player_id)
        .is_some_and(|recipient| recipient.engine_shutdown_application_required);
    state.record_prize_claim_receipt(
        collectible_id,
        ServerPrizeReceipt {
            claim,
            winner,
            claimed_at_tick: tick,
            authoritative_energy,
            prize_state_revision: winner_prize_state_revision,
            engine_shutdown_until_tick: winner_engine_shutdown_until_tick,
            engine_shutdown_application_required: winner_engine_shutdown_application_required,
        },
        tick,
    );
    tracing::debug!(
        player_id,
        collectible_id = claim.collectible_id,
        x = claim.x,
        y = claim.y,
        prize_id = claim.prize_id,
        authoritative_recipient_count = authoritative_recipients.len(),
        engine_shutdown_until_tick,
        movement_tuning_before,
        movement_tuning_after,
        "server prize claimed"
    );
    Some(PrizeClaimResult {
        claim,
        newly_collected: true,
        recipient_acks: if pickup_ack_ready {
            recipient_acks
        } else {
            recipient_acks
                .into_iter()
                .filter(|recipient| recipient.player_id != player_id)
                .collect()
        },
    })
}

fn server_movement_tuning_for_prize(client: &ClientHandle, prize_id: i16) -> Option<f32> {
    match prize_id.checked_abs()? {
        3 => Some(client.server_rotation_per_second),
        11 => Some(client.server_thrust_acceleration),
        12 => Some(client.server_top_speed),
        1 => Some(client.server_recharge_per_second),
        _ => None,
    }
}

fn prize_claim_id_valid(prize_id: i16) -> bool {
    matches!(prize_id.checked_abs(), Some(1..=28))
}

fn prize_claim_bounty_gain(claim: ServerPrizeClaim) -> Option<u16> {
    (claim.prize_id >= 0).then_some(1)
}

fn prize_claim_overlaps_last_position(
    ship: u8,
    last_position: Option<&[u8]>,
    claim: ServerPrizeClaim,
) -> bool {
    let Some((ship_x, ship_y)) = large_position_world_position(last_position) else {
        return false;
    };
    let radius = arena_ship_radius_pixels(ship).max(0.0);
    let ship_min_x = ship_x - radius;
    let ship_max_x = ship_x + radius;
    let ship_min_y = ship_y - radius;
    let ship_max_y = ship_y + radius;
    let half_prize = TILE_SIZE_PX * 0.5;
    let prize_x = claim.x as f32;
    let prize_y = claim.y as f32;
    let prize_min_x = prize_x - half_prize;
    let prize_max_x = prize_x + half_prize;
    let prize_min_y = prize_y - half_prize;
    let prize_max_y = prize_y + half_prize;

    ship_max_x >= prize_min_x
        && ship_min_x < prize_max_x
        && ship_max_y >= prize_min_y
        && ship_min_y < prize_max_y
}

fn large_position_world_position(packet: Option<&[u8]>) -> Option<(f32, f32)> {
    let packet = packet?;
    if packet.len() < 17 || packet[0] != crate::arena::LARGE_POSITION_PACKET {
        return None;
    }
    let x = u16::from_le_bytes(packet[4..6].try_into().ok()?);
    let y = u16::from_le_bytes(packet[15..17].try_into().ok()?);
    Some((x as f32, y as f32))
}
