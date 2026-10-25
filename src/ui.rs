use macroquad::prelude::*;

use crate::{
    AddKothTime, ArenaDirectoryEntry, ArenaDirectoryListing, ClientFileRequest,
    DEFAULT_BANNER_PALETTE, FlagReward, FlagVictory, FrequencyChange, KothGameReset,
    PLAYER_BANNER_BYTES, PlayerRosterEntry, PlayerVoice, PrizeKind, ServerDamageReport,
    ServerFileTransfer, ServerVersionPacket, SetPersonalKothTimer, ShipFrequencyChange, SoccerGoal,
    SpeedGameOver, ToggleUfoPacket,
};
use std::fmt::Write;

pub(crate) const NOTIFICATION_LIMIT: usize = 7;
pub(crate) const NOTIFICATION_LIFETIME: f32 = 5.0;
pub(crate) const NOTIFICATION_TEXT_SCALE: f32 = 0.875;
pub(crate) const NOTIFICATION_ANCHOR_X_FRACTION: f32 = 0.2;
pub(crate) const NOTIFICATION_ANCHOR_Y_FRACTION: f32 = 0.6;

pub(crate) struct Notification {
    pub(crate) message: String,
    pub(crate) ttl: f32,
    pub(crate) color: Color,
}

pub(crate) fn ship_label(
    name: &str,
    bounty: u32,
    carrying_flag: bool,
    carrying_ball: bool,
) -> String {
    let mut label = if carrying_flag {
        format!("{name}({bounty}:1)")
    } else {
        format!("{name}({bounty})")
    };

    if carrying_ball {
        label.push_str(" (Ball)");
    }
    label
}

pub(crate) fn energy_label(energy: f32, max_energy: f32) -> Option<(String, Color)> {
    if energy >= max_energy * 0.5 {
        return None;
    }

    let color = if energy < max_energy * 0.25 {
        Color::from_rgba(255, 90, 70, 255)
    } else {
        Color::from_rgba(255, 225, 95, 255)
    };
    Some((format!("{:03}", energy.max(0.0) as i32), color))
}

pub(crate) fn energy_alarm_active(
    energy: f32,
    max_energy: f32,
    spectator: bool,
    respawn_timer: f32,
) -> bool {
    !spectator && respawn_timer <= 0.0 && max_energy > 0.0 && energy < max_energy * 0.25
}

pub(crate) fn draw_player_banner(x: f32, y: f32, scale: f32, banner: &[u8; PLAYER_BANNER_BYTES]) {
    for row in 0..8 {
        for column in 0..12 {
            let color = banner_palette_color(banner[row * 12 + column]);
            draw_rectangle(
                x + column as f32 * scale,
                y + row as f32 * scale,
                scale,
                scale,
                color,
            );
        }
    }
}

pub(crate) fn banner_palette_color(index: u8) -> Color {
    let rgb = DEFAULT_BANNER_PALETTE[index as usize];
    Color::from_rgba(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
        255,
    )
}

pub(crate) fn prize_notification(kind: PrizeKind, negative: bool) -> &'static str {
    match (kind, negative) {
        (PrizeKind::Energy, false) => "Maximum energy level increased.",
        (PrizeKind::Energy, true) => "Maximum energy level decreased.",
        (PrizeKind::FullCharge, false) => "Full charge.",
        (PrizeKind::FullCharge, true) => "Energy depleted.",
        (PrizeKind::Rocket, false) => "Rocket Increased.",
        (PrizeKind::Rocket, true) => "Rocket lost.",
        (PrizeKind::Repel, false) => "Repeller increased.",
        (PrizeKind::Repel, true) => "Repeller lost.",
        (PrizeKind::Burst, false) => "Burst increased.",
        (PrizeKind::Burst, true) => "Burst lost.",
        (PrizeKind::Brick, false) => "Brick increased.",
        (PrizeKind::Brick, true) => "Brick lost.",
        (PrizeKind::Decoy, false) => "Decoy increased.",
        (PrizeKind::Decoy, true) => "Decoy lost.",
        (PrizeKind::Thor, false) => "Thor's hammer increased.",
        (PrizeKind::Thor, true) => "Thor's hammer lost.",
        (PrizeKind::Portal, false) => "Portal increased.",
        (PrizeKind::Portal, true) => "Portal lost.",
        (PrizeKind::Super, false) => "Temporary SuperPower!",
        (PrizeKind::Super, true) => "",
        (PrizeKind::Shield, false) => "Temporary Shields.",
        (PrizeKind::Shield, true) => "",
        (PrizeKind::EngineShutdown, false) => "Engines shut-down.",
        (PrizeKind::EngineShutdown, true) => "Engines shut-down (severe).",
        (PrizeKind::BouncingBullets, false) => "Bouncing bullets.",
        (PrizeKind::BouncingBullets, true) => "Bouncing bullets lost.",
        (PrizeKind::Stealth, false) => "Stealth available.",
        (PrizeKind::Stealth, true) => "Stealth lost.",
        (PrizeKind::Cloak, false) => "Cloak available.",
        (PrizeKind::Cloak, true) => "Cloak lost.",
        (PrizeKind::XRadar, false) => "X-Radar available.",
        (PrizeKind::XRadar, true) => "X-Radar lost.",
        (PrizeKind::Warp, false) => "Warp!",
        (PrizeKind::Warp, true) => "Energy depleted.",
        (PrizeKind::Rotation, false) => "Rotation speed increased.",
        (PrizeKind::Rotation, true) => "Rotation speed decreased.",
        (PrizeKind::Thruster, false) => "Thrusters upgraded.",
        (PrizeKind::Thruster, true) => "Thrusters downgraded.",
        (PrizeKind::TopSpeed, false) => "Top speed increased.",
        (PrizeKind::TopSpeed, true) => "Top speed reduced.",
        (PrizeKind::Recharge, false) => "Charge rate increased.",
        (PrizeKind::Recharge, true) => "Charge rate decreased.",
        (PrizeKind::Shrapnel, false) => "Shrapnel increased.",
        (PrizeKind::Shrapnel, true) => "Shrapnel reduced.",
        (PrizeKind::Guns, false) => "Guns upgraded.",
        (PrizeKind::Guns, true) => "Guns downgraded.",
        (PrizeKind::Bombs, false) => "Bombs upgraded.",
        (PrizeKind::Bombs, true) => "Bombs downgraded.",
        (PrizeKind::Multifire, false) => "MultiFire bullets.",
        (PrizeKind::Multifire, true) => "MultiFire lost.",
        (PrizeKind::Proximity, false) => "Proximity bombs.",
        (PrizeKind::Proximity, true) => "Proximity bombs lost.",
        (PrizeKind::Antiwarp, false) => "AntiWarp available.",
        (PrizeKind::Antiwarp, true) => "AntiWarp lost.",
        (PrizeKind::Multiprize, false) => "MultiPrize!",
        (PrizeKind::Multiprize, true) => "",
    }
}

pub(crate) fn prize_notification_text(kind: PrizeKind, negative: bool, damage: bool) -> String {
    let message = prize_notification(kind, negative);
    if message.is_empty() {
        String::new()
    } else if damage {
        format!("DAMAGE: {message}")
    } else {
        message.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_prize_notifications_match_reference_game_prefix() {
        assert_eq!(
            prize_notification_text(PrizeKind::Guns, true, true),
            "DAMAGE: Guns downgraded."
        );
        assert_eq!(
            prize_notification_text(PrizeKind::Guns, true, false),
            "Guns downgraded."
        );
        assert_eq!(prize_notification_text(PrizeKind::Shield, true, true), "");
    }
}

pub(crate) fn player_destroyed_notification(
    player_name: &str,
    bounty: u32,
    killer_name: Option<&str>,
) -> String {
    match killer_name {
        Some(name) => player_killed_by_notification(player_name, bounty, name),
        None => format!("{player_name}({bounty}) destroyed."),
    }
}

pub(crate) fn bot_destroyed_notification(id: usize, bounty: u32, killer_name: &str) -> String {
    player_killed_by_notification(&format!("Bot {}", id + 1), bounty, killer_name)
}

pub(crate) fn player_killed_by_notification(
    killed_name: &str,
    bounty: u32,
    killer_name: &str,
) -> String {
    format!("{killed_name}({bounty}) killed by: {killer_name}")
}

pub(crate) fn server_version_notification(packet: &ServerVersionPacket) -> String {
    format!(
        "Server version {} checksum {:#010x}.",
        packet.version, packet.checksum
    )
}

pub(crate) fn arena_directory_notification(listing: &ArenaDirectoryListing) -> String {
    let population: u32 = listing
        .arenas
        .iter()
        .map(|arena| u32::from(arena.population))
        .sum();
    match listing.arenas.len() {
        0 => "Arena directory is empty.".to_string(),
        1 => format!(
            "Arena directory: {} ({} players).",
            listing.arenas[0].name, listing.arenas[0].population
        ),
        count => format!("Arena directory: {count} arenas, {population} players."),
    }
}

pub(crate) fn arena_directory_rows(arenas: &[ArenaDirectoryEntry], max_rows: usize) -> Vec<String> {
    let mut rows: Vec<String> = arenas
        .iter()
        .take(max_rows)
        .map(|arena| {
            format!(
                "{:<20} {:>4}",
                fixed_width_label(&arena.name, 20),
                arena.population
            )
        })
        .collect();
    if arenas.len() > max_rows {
        rows.push(format!("+{} more arenas", arenas.len() - max_rows));
    }
    rows
}

fn fixed_width_label(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }

    let mut label: String = value.chars().take(max_chars - 3).collect();
    label.push_str("...");
    label
}

pub(crate) fn arena_directory_panel_position(
    screen_width: f32,
    screen_height: f32,
    panel_width: f32,
    panel_height: f32,
    scoreboard_right: f32,
    scoreboard_bottom: f32,
) -> (f32, f32) {
    let preferred_x = (screen_width - panel_width - 24.0).max(24.0);
    let preferred_y = 110.0_f32.min((screen_height - panel_height - 24.0).max(24.0));
    if preferred_x >= scoreboard_right + 16.0 {
        return (preferred_x, preferred_y);
    }

    let stacked_y = (scoreboard_bottom + 16.0).min(screen_height - panel_height - 24.0);
    (24.0, stacked_y.max(24.0))
}

pub(crate) fn server_file_transfer_notification(file: &ServerFileTransfer) -> String {
    format!("Received file: {}.", file.filename)
}

pub(crate) fn client_file_request_notification(request: &ClientFileRequest) -> String {
    format!("Server requested file: {}.", request.local_filename)
}

pub(crate) fn player_voice_notification(voice: &PlayerVoice) -> String {
    format!("Voice message from #{}.", voice.player_id)
}

pub(crate) fn player_entered_notification(entry: &PlayerRosterEntry) -> String {
    format!("Player {} joined.", entry.name)
}

pub(crate) fn player_left_notification(name: Option<&str>) -> String {
    match name.filter(|name| !name.trim().is_empty()) {
        Some(name) => format!("Player {name} left."),
        None => "A player left.".to_string(),
    }
}

pub(crate) fn frequency_change_notification(
    change: &FrequencyChange,
    name: Option<&str>,
) -> String {
    match name {
        Some(name) => format!("{name} changed to freq {}.", change.frequency),
        None => format!(
            "Player #{} changed to freq {}.",
            change.player_id, change.frequency
        ),
    }
}

pub(crate) fn ship_frequency_change_notification(
    change: &ShipFrequencyChange,
    name: Option<&str>,
) -> String {
    if change.ship == crate::SPECTATOR_SHIP {
        return match name {
            Some(name) => format!("{name} entered spectator mode."),
            None => format!("Player #{} entered spectator mode.", change.player_id),
        };
    }
    match name {
        Some(name) => format!(
            "{name} changed to ship {}, freq {}.",
            display_ship_number(change.ship),
            change.frequency
        ),
        None => format!(
            "Player #{} changed to ship {}, freq {}.",
            change.player_id,
            display_ship_number(change.ship),
            change.frequency
        ),
    }
}

pub(crate) fn soccer_goal_notification(goal: &SoccerGoal) -> String {
    format!(
        "Soccer goal: freq {} now has {} points.",
        goal.frequency, goal.team_points
    )
}

pub(crate) fn ufo_toggle_notification(packet: &ToggleUfoPacket) -> &'static str {
    if packet.available {
        "UFO mode available."
    } else {
        "UFO mode disabled."
    }
}

pub(crate) fn personal_koth_timer_notification(packet: &SetPersonalKothTimer) -> String {
    format!(
        "Personal KOTH timer set to {}.",
        koth_timer_label(packet.timer)
    )
}

pub(crate) fn koth_reset_notification(packet: &KothGameReset) -> String {
    let action = if packet.adding_timer {
        "adding"
    } else {
        "countdown"
    };
    format!(
        "KOTH reset: player #{} timer {} ({action}).",
        packet.player_id,
        koth_timer_label(packet.timer)
    )
}

pub(crate) fn koth_time_added_notification(packet: &AddKothTime) -> String {
    format!(
        "KOTH time added: {}.",
        koth_timer_label(packet.additional_time)
    )
}

pub(crate) fn flag_victory_notification(victory: &FlagVictory) -> String {
    format!(
        "Flag victory: freq {} wins {} jackpot points.",
        victory.team, victory.jackpot_points
    )
}

pub(crate) fn flag_reward_notification(reward: &FlagReward) -> String {
    match reward.grants.as_slice() {
        [] => "Flag reward received.".to_string(),
        [grant] => format!("Flag reward: freq {} +{}.", grant.team, grant.points),
        grants => format!("Flag rewards received for {} teams.", grants.len()),
    }
}

pub(crate) fn speed_game_over_notification(result: &SpeedGameOver) -> String {
    let suffix = if result.best { " best" } else { "" };
    format!(
        "Speed game over: rank {}, score {}{suffix}.",
        result.rank, result.score
    )
}

pub(crate) fn watched_damage_notification(report: &ServerDamageReport) -> String {
    let total_damage: u32 = report
        .entries
        .iter()
        .map(|entry| u32::from(entry.damage))
        .sum();
    match report.entries.len() {
        0 => format!("Watched damage for #{}: none.", report.player_id),
        1 => format!(
            "Watched damage for #{}: {} from #{}.",
            report.player_id, total_damage, report.entries[0].attacker_id
        ),
        count => format!(
            "Watched damage for #{}: {} across {} hits.",
            report.player_id, total_damage, count
        ),
    }
}

pub(crate) fn network_objective_status(
    soccer_scores: &std::collections::BTreeMap<u16, u32>,
    personal_koth_timer: Option<u32>,
    koth_timer: Option<u32>,
    ufo_available: bool,
) -> Option<String> {
    if soccer_scores.is_empty()
        && personal_koth_timer.is_none()
        && koth_timer.is_none()
        && !ufo_available
    {
        return None;
    }

    let mut status = String::from("Server");
    if !soccer_scores.is_empty() {
        status.push_str("  Ball");
        for (frequency, points) in soccer_scores.iter().take(4) {
            let _ = write!(status, " F{frequency}:{points}");
        }
        if soccer_scores.len() > 4 {
            let _ = write!(status, " +{}", soccer_scores.len() - 4);
        }
    }
    if let Some(timer) = koth_timer {
        let _ = write!(status, "  KOTH {}", koth_timer_label(timer));
    }
    if let Some(timer) = personal_koth_timer {
        let _ = write!(status, "  Personal {}", koth_timer_label(timer));
    }
    if ufo_available {
        status.push_str("  UFO");
    }
    Some(status)
}

pub(crate) fn koth_timer_label(centiseconds: u32) -> String {
    let minutes = centiseconds / 6_000;
    let seconds = (centiseconds / 100) % 60;
    let tenths = (centiseconds % 100) / 10;
    if minutes > 0 {
        format!("{minutes}:{seconds:02}.{tenths}")
    } else {
        format!("{seconds}.{tenths}")
    }
}

fn display_ship_number(ship: u8) -> u8 {
    ship.saturating_add(1)
}

pub(crate) fn push_notification_entry(
    notifications: &mut Vec<Notification>,
    message: String,
    color: Color,
    limit: usize,
    ttl: f32,
) {
    if limit == 0 {
        return;
    }

    while notifications.len() >= limit {
        notifications.remove(0);
    }

    notifications.push(Notification {
        message,
        ttl,
        color,
    });
}

pub(crate) fn update_notification_entries(notifications: &mut Vec<Notification>, dt: f32) {
    for notification in notifications.iter_mut() {
        notification.ttl -= dt;
    }
    notifications.retain(|notification| notification.ttl > 0.0);
}
