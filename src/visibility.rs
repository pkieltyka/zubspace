use crate::{NetworkPlayer, SPECTATOR_SHIP, STATUS_CLOAK, STATUS_STEALTH};

pub(crate) fn flags_visible_to_viewer(
    spectator_mode: bool,
    hide_flags_from_spectators: bool,
    arena_elapsed_seconds: f32,
    enter_game_flagging_delay_seconds: f32,
) -> bool {
    if spectator_mode && hide_flags_from_spectators {
        return false;
    }

    let delay = enter_game_flagging_delay_seconds.max(0.0);
    delay <= 0.0 || arena_elapsed_seconds > delay
}

pub(crate) fn spectator_xradar_toggle_after(current: bool, no_xradar: bool) -> Option<bool> {
    (!no_xradar).then_some(!current)
}

pub(crate) fn viewer_frequency_after_spectate(
    player_frequency: u16,
    spectator_mode: bool,
    spectated_frequency: Option<u16>,
) -> u16 {
    if spectator_mode {
        spectated_frequency.unwrap_or(player_frequency)
    } else {
        player_frequency
    }
}

pub(crate) fn visibility_ship_type_after_spectate(
    player_ship_type: usize,
    spectator_mode: bool,
    spectated_ship: Option<u8>,
) -> usize {
    if spectator_mode {
        spectated_ship
            .filter(|ship| *ship < SPECTATOR_SHIP)
            .map(|ship| ship as usize)
            .unwrap_or(player_ship_type)
    } else {
        player_ship_type
    }
}

pub(crate) fn network_player_world_visible(
    player_togglables: u8,
    player_frequency: u16,
    viewer_frequency: u16,
    viewer_xradar: bool,
) -> bool {
    player_togglables & STATUS_CLOAK == 0 || viewer_xradar || player_frequency == viewer_frequency
}

pub(crate) fn network_player_radar_visible(
    player_togglables: u8,
    player_frequency: u16,
    viewer_frequency: u16,
    viewer_xradar: bool,
) -> bool {
    player_togglables & STATUS_STEALTH == 0 || viewer_xradar || player_frequency == viewer_frequency
}

pub(crate) fn network_child_reveals_parent_on_radar(child: &NetworkPlayer) -> bool {
    child.togglables & STATUS_STEALTH == 0
}
