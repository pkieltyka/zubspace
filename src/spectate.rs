use crate::NetworkWorld;
use macroquad::prelude::Vec2;

pub(crate) fn nearest_spectate_target(
    world: &NetworkWorld,
    pos: Vec2,
    viewer_frequency: u16,
    viewer_xradar: bool,
) -> Option<u16> {
    world
        .visible_players()
        .filter(|player| {
            crate::visibility::network_player_world_visible(
                player.togglables,
                player.frequency,
                viewer_frequency,
                viewer_xradar,
            )
        })
        .filter_map(|player| {
            let player_pos = player.display_position()?;
            Some((player.player_id, player_pos.distance_squared(pos)))
        })
        .min_by(|(left_id, left_distance), (right_id, right_distance)| {
            left_distance
                .total_cmp(right_distance)
                .then_with(|| left_id.cmp(right_id))
        })
        .map(|(player_id, _)| player_id)
}

/// Cycle the whole roster, including actors currently outside the camera AOI.
/// A scoped follow request makes the server publish the selected actor's area.
pub(crate) fn next_spectate_target(
    world: &NetworkWorld,
    current: Option<u16>,
    pos: Vec2,
    backwards: bool,
    viewer_frequency: u16,
    viewer_xradar: bool,
) -> Option<u16> {
    let mut ids: Vec<_> = world
        .players
        .values()
        .filter(|player| Some(player.player_id) != world.viewer_player_id() && player.ship < 8)
        .map(|player| player.player_id)
        .collect();
    ids.sort_unstable();
    let Some(index) = current.and_then(|current| ids.iter().position(|id| *id == current)) else {
        return nearest_spectate_target(world, pos, viewer_frequency, viewer_xradar)
            .or_else(|| ids.first().copied());
    };
    let next = if backwards {
        (index + ids.len() - 1) % ids.len()
    } else {
        (index + 1) % ids.len()
    };
    ids.get(next).copied()
}
