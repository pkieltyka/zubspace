//! Prove an unreceipted offer's predicted lineage without admitting it.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn matching_offered_window(
    sender: &ClientHandle,
    map: &ZoneMap,
    snapshot: ClientPositionSnapshot,
    is_flagger: bool,
    attached_riders: bool,
    existing_repel_active: bool,
    movement_upgrades: crate::sim_observation::PlayerMovementUpgrades,
    enemy_brick_tiles: &[(usize, usize)],
    client_sequence: u16,
    server_tick: u32,
    spawn_x: u16,
    spawn_y: u16,
    tail_bomb_recoil_speed: Option<f32>,
) -> Option<(u16, u16)> {
    // Only an admitted application whose receipt is still outstanding owns
    // this uncertainty. A newly queued or already retired event cannot excuse
    // a mismatching report, even if its source id or vector happen to match.
    let event = sender.pending_repel_observations.front()?;
    let application = event.accepted_application?;
    let sealed = crate::gameplay_rules::repel_sequence_window_from_active_steps(
        application.application_sequence,
        u32::from(application.active_steps),
    );
    if sender.server_repel_sequence_window != Some(sealed) {
        return None;
    }
    let offer = event
        .delivered_snapshots
        .iter()
        .flatten()
        .find(|offer| offer.snapshot_id == application.snapshot_id)?;
    let previous = sender.canonical_movement_witness?;
    let prior_offset = previous
        .sequence
        .wrapping_sub(application.application_sequence);
    let tail_offset = client_sequence.wrapping_sub(application.application_sequence);
    if previous.life_seq != sender.life_seq
        || sender.last_position_sequence != Some(previous.sequence)
        || prior_offset >= (1 << 15)
        || tail_offset >= (1 << 15)
        || tail_offset <= prior_offset
        || usize::from(tail_offset - prior_offset) > crate::state::MAX_RECENT_INPUT_OBSERVATIONS
    {
        return None;
    }
    // Ends before the prior witness are equivalent throughout this gap;
    // ends after the tail are equivalent too. Thus at most64+1 candidates,
    // regardless of the configured lifetime, and every replay is read-only.
    let low = prior_offset.min(offer.active_steps);
    let high = tail_offset.min(offer.active_steps);
    for steps in low..=high {
        let candidate = crate::gameplay_rules::repel_sequence_window_from_active_steps(
            application.application_sequence,
            u32::from(steps),
        );
        if candidate == sealed {
            continue;
        }
        let Some(replay) =
            crate::sim_observation::replay_canonical_position_gap_from_witness_with_repel_window(
                sender,
                previous,
                sender.last_position_server_tick?,
                map,
                is_flagger,
                attached_riders,
                existing_repel_active,
                movement_upgrades,
                enemy_brick_tiles,
                client_sequence,
                server_tick,
                spawn_x,
                spawn_y,
                tail_bomb_recoil_speed,
                None,
                Some(candidate),
            )
        else {
            continue;
        };
        if !replay
            .matching_exact_branch_witnesses(
                snapshot.canonical_state,
                snapshot.direction,
                snapshot.x,
                snapshot.y,
                snapshot.velocity_x,
                snapshot.velocity_y,
            )
            .is_empty()
        {
            return Some(candidate);
        }
    }
    None
}
