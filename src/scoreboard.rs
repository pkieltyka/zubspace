use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::{BotShip, NetworkWorld, PLAYER_BANNER_BYTES};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ScoreboardRow {
    Frequency(ScoreboardFrequencyRow),
    Player(ScoreboardEntry),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScoreboardFrequencyRow {
    pub(crate) frequency: Option<u16>,
    pub(crate) players: usize,
    pub(crate) kills: u32,
    pub(crate) assists: u32,
    pub(crate) deaths: u32,
    pub(crate) flag_points: u32,
    pub(crate) ball_goals: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScoreboardEntry {
    /// Authoritative network roster identity. Offline/local-only rows have no
    /// network player id.
    pub(crate) player_id: Option<u16>,
    pub(crate) name: String,
    pub(crate) frequency: Option<u16>,
    pub(crate) ship_type: usize,
    pub(crate) kills: u32,
    pub(crate) kill_points: u32,
    pub(crate) assists: u32,
    pub(crate) deaths: u32,
    pub(crate) bounty: u32,
    pub(crate) flag_goals: u32,
    pub(crate) ball_goals: u32,
    pub(crate) energy: f32,
    pub(crate) banner: Option<[u8; PLAYER_BANNER_BYTES]>,
    pub(crate) active: bool,
    pub(crate) local: bool,
}

impl ScoreboardEntry {
    pub(crate) fn total_score(&self) -> u64 {
        u64::from(self.kill_points) + u64::from(self.flag_goals)
    }

    pub(crate) fn average_bounty(&self) -> f64 {
        if self.kills == 0 {
            0.0
        } else {
            f64::from(self.kill_points) / f64::from(self.kills)
        }
    }

    pub(crate) fn rating(&self) -> u32 {
        let points = i64::from(self.kill_points);
        let balance = i64::from(self.kills) - i64::from(self.deaths);
        (((points + balance * 10) * 10) / (i64::from(self.kills) + 100))
            .clamp(0, i64::from(u32::MAX)) as u32
    }

    fn compare(left: &Self, right: &Self) -> Ordering {
        right
            .kills
            .cmp(&left.kills)
            .then_with(|| left.deaths.cmp(&right.deaths))
            .then_with(|| right.local.cmp(&left.local))
            .then_with(|| left.name.cmp(&right.name))
    }
}

pub(crate) fn scoreboard_entries(
    local_player_name: &str,
    player_kills: u32,
    player_deaths: u32,
    player_bounty: u32,
    player_flag_goals: u32,
    player_ball_goals: u32,
    player_ship_type: usize,
    player_energy: f32,
    bots: &[BotShip],
    network_world: &NetworkWorld,
) -> Vec<ScoreboardEntry> {
    let mut rows = Vec::with_capacity(bots.len() + network_world.players.len() + 1);
    let local_player_name = network_world
        .viewer_player_id()
        .and_then(|player_id| network_world.players.get(&player_id))
        .map(|player| player.name.as_str())
        .unwrap_or(local_player_name);
    let local_network_stats = network_world
        .viewer_player_id()
        .and_then(|player_id| network_world.players.get(&player_id));
    rows.push(ScoreboardEntry {
        player_id: network_world.viewer_player_id(),
        name: local_player_name.to_string(),
        frequency: local_network_stats
            .map(|player| player.frequency)
            .or(Some(0)),
        ship_type: player_ship_type,
        kills: local_network_stats
            .map(|player| player.wins as u32)
            .unwrap_or(player_kills),
        kill_points: local_network_stats
            .map(|player| player.kill_points)
            .unwrap_or(0),
        assists: local_network_stats
            .map(|player| player.assists)
            .unwrap_or(0),
        deaths: local_network_stats
            .map(|player| player.losses as u32)
            .unwrap_or(player_deaths),
        bounty: player_bounty,
        flag_goals: local_network_stats
            .map(|player| player.flag_points)
            .unwrap_or(player_flag_goals),
        ball_goals: player_ball_goals,
        energy: player_energy,
        banner: network_world.personal_banner,
        active: true,
        local: true,
    });

    for bot in bots {
        rows.push(ScoreboardEntry {
            player_id: None,
            name: format!("Bot {}", bot.id + 1),
            frequency: None,
            ship_type: bot.ship_type,
            kills: bot.kills,
            kill_points: 0,
            assists: 0,
            deaths: bot.deaths,
            bounty: bot.bounty,
            flag_goals: bot.flag_goals,
            ball_goals: bot.ball_goals,
            energy: bot.energy,
            banner: None,
            active: bot.respawn_timer <= 0.0,
            local: false,
        });
    }

    for player in network_world.players.values() {
        if Some(player.player_id) == network_world.viewer_player_id() {
            continue;
        }
        rows.push(ScoreboardEntry {
            player_id: Some(player.player_id),
            name: player.name.clone(),
            frequency: Some(player.frequency),
            ship_type: player.ship.min(7) as usize,
            kills: player.wins as u32,
            kill_points: player.kill_points,
            assists: player.assists,
            deaths: player.losses as u32,
            bounty: player.bounty as u32,
            flag_goals: player.flag_points,
            ball_goals: 0,
            energy: player.energy.unwrap_or(0.0),
            banner: player.banner,
            active: player.ship < 8 && player.position.is_some(),
            local: false,
        });
    }

    rows.sort_by(ScoreboardEntry::compare);
    rows
}

pub(crate) fn scoreboard_rows(
    local_player_name: &str,
    player_kills: u32,
    player_deaths: u32,
    player_bounty: u32,
    player_flag_goals: u32,
    player_ball_goals: u32,
    player_ship_type: usize,
    player_energy: f32,
    bots: &[BotShip],
    network_world: &NetworkWorld,
) -> Vec<ScoreboardRow> {
    let entries = scoreboard_entries(
        local_player_name,
        player_kills,
        player_deaths,
        player_bounty,
        player_flag_goals,
        player_ball_goals,
        player_ship_type,
        player_energy,
        bots,
        network_world,
    );
    grouped_scoreboard_rows(entries, &network_world.soccer_scores)
}

fn grouped_scoreboard_rows(
    entries: Vec<ScoreboardEntry>,
    soccer_scores: &BTreeMap<u16, u32>,
) -> Vec<ScoreboardRow> {
    let mut groups: BTreeMap<Option<u16>, Vec<ScoreboardEntry>> = BTreeMap::new();
    for entry in entries {
        groups.entry(entry.frequency).or_default().push(entry);
    }

    let mut rows = Vec::new();
    for (frequency, mut entries) in groups {
        entries.sort_by(ScoreboardEntry::compare);
        rows.push(ScoreboardRow::Frequency(ScoreboardFrequencyRow {
            frequency,
            players: entries.len(),
            kills: entries.iter().map(|entry| entry.kills).sum(),
            assists: entries.iter().map(|entry| entry.assists).sum(),
            deaths: entries.iter().map(|entry| entry.deaths).sum(),
            flag_points: entries.iter().map(|entry| entry.flag_goals).sum(),
            ball_goals: frequency
                .and_then(|frequency| soccer_scores.get(&frequency).copied())
                .unwrap_or_else(|| entries.iter().map(|entry| entry.ball_goals).sum()),
        }));
        rows.extend(entries.into_iter().map(ScoreboardRow::Player));
    }
    rows
}

/// Current playing roster only; score ties resolve to the lowest player ID.
/// Death/respawn does not remove a player from contention, but spectating or
/// leaving the arena does. Sum in u64 to preserve the full wire score range.
pub(crate) fn arena_score_leader(
    players: impl IntoIterator<Item = (u16, u8, u32, u32)>,
) -> Option<u16> {
    players
        .into_iter()
        .filter(|(_, ship, _, _)| *ship < 8)
        .max_by(|a, b| {
            (u64::from(a.2) + u64::from(a.3))
                .cmp(&(u64::from(b.2) + u64::from(b.3)))
                .then_with(|| b.0.cmp(&a.0))
        })
        .map(|player| player.0)
}

#[cfg(test)]
mod leader_tests {
    use super::arena_score_leader;

    #[test]
    fn leader_uses_current_playing_roster_scores_including_bots() {
        let roster = [(1, 0, 100, 0), (768, 4, 90, 30), (2, 8, 9999, 9999)];
        assert_eq!(arena_score_leader(roster), Some(768));
        assert_eq!(
            arena_score_leader([roster[0], roster[2]]),
            Some(1),
            "leaving promotes the next player"
        );
        assert_eq!(
            arena_score_leader([(1, 0, 130, 0), roster[1]]),
            Some(1),
            "score updates transfer the badge"
        );
        assert_eq!(
            arena_score_leader([(1, 8, 130, 0), roster[1]]),
            Some(768),
            "spectators cannot lead"
        );
        assert_eq!(arena_score_leader([]), None);
        assert_eq!(arena_score_leader([roster[2]]), None);
    }

    #[test]
    fn leader_ties_ignore_roster_order_and_handle_full_score_range() {
        assert_eq!(arena_score_leader([(8, 0, 0, 0), (2, 7, 0, 0)]), Some(2));
        assert_eq!(arena_score_leader([(2, 7, 0, 0), (8, 0, 0, 0)]), Some(2));
        assert_eq!(
            arena_score_leader([(2, 0, u32::MAX, 0), (8, 0, u32::MAX, 1)]),
            Some(8)
        );
    }
}
