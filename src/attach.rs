use crate::{ArenaTuning, INVALID_PLAYER_ID, NetworkWorld, ShipTuning};
use macroquad::prelude::Vec2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachAction {
    Detach,
    DropAttached,
    Attach(u16),
}

#[allow(dead_code)]
pub(crate) fn attach_action_for_target(
    world: &NetworkWorld,
    target_id: Option<u16>,
    player_bounty: u32,
    player_energy: f32,
    source_tuning: ShipTuning,
    arena_tuning: &ArenaTuning,
) -> Result<AttachAction, &'static str> {
    let Some(self_id) = world.viewer_player_id() else {
        return Err("No local player.");
    };
    let Some(self_player) = world.players.get(&self_id) else {
        return Err("No local player.");
    };

    if self_player.attach_parent != INVALID_PLAYER_ID {
        return Ok(AttachAction::Detach);
    }

    if attached_player_count(world, self_id) > 0 {
        return Ok(AttachAction::DropAttached);
    }

    if player_energy < source_tuning.energy_max {
        return Err("Not enough energy to attach.");
    }

    if player_bounty < source_tuning.attach_bounty {
        return Err("Bounty not high enough to attach.");
    }

    let Some(target_id) = target_id else {
        return Err("No attach target.");
    };
    let Some(target) = world.players.get(&target_id) else {
        return Err("No attach target.");
    };

    if self_id == target.player_id {
        return Err("Cannot attach to self.");
    }

    if self_player.frequency != target.frequency {
        return Err("Must attach to somebody of same frequency.");
    }

    if target.ship >= 8 {
        return Err("Cannot attach to spectator.");
    }

    let destination_tuning = arena_tuning.ship(target.ship as usize);
    if destination_tuning.turret_limit == 0 {
        return Err("Target ship is not attachable.");
    }

    if attached_player_count(world, target.player_id) >= destination_tuning.turret_limit as usize {
        return Err("Too many turrets already attached.");
    }

    Ok(AttachAction::Attach(target.player_id))
}

fn attached_player_count(world: &NetworkWorld, parent_id: u16) -> usize {
    world
        .players
        .values()
        .filter(|player| player.attach_parent == parent_id)
        .count()
}

pub(crate) fn nearest_attach_target(
    world: &NetworkWorld,
    pos: Vec2,
    player_bounty: u32,
    player_energy: f32,
    source_tuning: ShipTuning,
    arena_tuning: &ArenaTuning,
) -> Option<u16> {
    world
        .players
        .values()
        .filter(|player| Some(player.player_id) != world.viewer_player_id())
        .filter(|player| player.attach_parent == INVALID_PLAYER_ID)
        .filter(|player| player.position.is_some())
        .filter_map(|player| {
            attach_action_for_target(
                world,
                Some(player.player_id),
                player_bounty,
                player_energy,
                source_tuning,
                arena_tuning,
            )
            .ok()?;
            let distance_squared = player.position?.distance_squared(pos);
            Some((player.player_id, distance_squared))
        })
        .min_by(|(left_id, left_distance), (right_id, right_distance)| {
            left_distance
                .total_cmp(right_distance)
                .then_with(|| left_id.cmp(right_id))
        })
        .map(|(player_id, _)| player_id)
}
