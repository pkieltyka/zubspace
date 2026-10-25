use super::*;

impl NetworkWorld {
    pub(super) fn apply_damage_watched_lifecycle_event(&mut self, report: &ServerDamageReport) {
        // WATCH_DAMAGE is only a lifecycle repair hint for a remote victim.
        // Self lifecycle remains owned by SnapshotLifecycleState/PLAYER_DEATH.
        if self.viewer_player_id == Some(report.player_id) {
            return;
        }
        let Some(player) = self.players.get_mut(&report.player_id) else {
            return;
        };
        for entry in &report.entries {
            if entry.explicit_lethal != Some(true) {
                continue;
            }
            let Some(source_snapshot_id) = entry.source_snapshot_id else {
                continue;
            };
            if source_snapshot_id == 0 || entry.source_snapshot_epoch != self.snapshot_epoch {
                continue;
            }
            if self
                .last_received_snapshot_id
                .is_some_and(|previous| !snapshot_id_is_forward(source_snapshot_id, previous))
            {
                continue;
            }
            let Some(victim_life_seq) = entry.victim_life_seq else {
                continue;
            };
            let next_life_seq = victim_life_seq.wrapping_add(1).max(1);
            let admissible = player.life_seq.map_or(true, |previous| {
                next_life_seq == previous || next_life_seq.wrapping_sub(previous) < (1_u32 << 31)
            });
            if admissible {
                player.set_life_seq(Some(next_life_seq));
            }
        }
    }

    pub(super) fn apply_player_entered_event(&mut self, entry: &PlayerRosterEntry) {
        self.players
            .insert(entry.player_id, NetworkPlayer::from_roster(entry));
    }

    pub(super) fn apply_player_left_event(&mut self, player_id: u16) {
        self.detach_player_and_children(player_id);
        self.players.remove(&player_id);
    }

    pub(super) fn apply_score_updated_event(&mut self, score: &ScoreUpdate) {
        if let Some(player) = self.players.get_mut(&score.player_id) {
            player.kill_points = score.kill_points;
            player.assists = score.assists;
            player.flag_points = score.flag_points;
            player.wins = score.wins;
            player.losses = score.losses;
        }
    }

    pub(super) fn apply_scores_reset_event(&mut self, reset: &ResetScore) {
        for player in self.players.values_mut() {
            if reset.applies_to(player.player_id) {
                player.kill_points = 0;
                player.assists = 0;
                player.flag_points = 0;
                player.wins = 0;
                player.losses = 0;
            }
        }
    }

    pub(super) fn apply_frequency_changed_event(&mut self, change: &FrequencyChange) {
        self.detach_player_and_children(change.player_id);
        if let Some(player) = self.players.get_mut(&change.player_id) {
            player.frequency = change.frequency;
            player.velocity = Vec2::ZERO;
            player.flags = 0;
        }
    }

    pub(super) fn apply_ship_frequency_changed_event(&mut self, change: &ShipFrequencyChange) {
        self.detach_player_and_children(change.player_id);
        if let Some(player) = self.players.get_mut(&change.player_id) {
            player.ship = change.ship;
            player.frequency = change.frequency;
            if change.life_seq.is_some_and(|life_seq| life_seq != 0) {
                player.set_life_seq(change.life_seq);
            }
            player.velocity = Vec2::ZERO;
            player.flags = 0;
        }
    }

    pub(super) fn apply_turret_link_created_event(&mut self, link: &CreateTurretLink) {
        if let Some(destination_id) = link.attached_destination() {
            let parent_state = self
                .players
                .get(&destination_id)
                .map(|parent| (parent.position, parent.velocity));
            if let Some(player) = self.players.get_mut(&link.requester_id) {
                player.attach_parent = destination_id;
                if let Some((position, velocity)) = parent_state {
                    player.position = position;
                    player.render_position = position;
                    player.velocity = velocity;
                }
            }
        } else if let Some(player_id) = link.detach_target(self.viewer_player_id) {
            if let Some(player) = self.players.get_mut(&player_id) {
                player.attach_parent = INVALID_PLAYER_ID;
                player.clear_position();
                player.velocity = Vec2::ZERO;
            }
        }
    }

    pub(super) fn apply_turret_link_destroyed_event(&mut self, link: &DestroyTurretLink) {
        for player in self.players.values_mut() {
            if player.attach_parent == link.player_id {
                player.attach_parent = INVALID_PLAYER_ID;
                player.clear_position();
                player.velocity = Vec2::ZERO;
            }
        }
    }

    pub(super) fn apply_player_death_event(&mut self, death: &PlayerDeath) {
        if self.viewer_player_id == Some(death.killer_id) && death.killer_id != death.killed_id {
            self.lvz.trigger_kill();
        }
        if self.viewer_player_id == Some(death.killed_id) {
            self.lvz.trigger_death();
        }

        // PLAYER_DEATH carries the generation entered by the death. Reliable
        // delivery can be delayed behind a large file-transfer gap while
        // forward snapshots already prove that exact generation (or a newer
        // one) has respawned. Keep the score event, but never let the delayed
        // lifecycle packet clear a live same-life render proxy.
        let death_lifecycle_superseded = self.authoritative_snapshot_id.is_some()
            && self
                .authoritative_snapshot_actors
                .iter()
                .find(|actor| actor.player_id == death.killed_id)
                .and_then(|actor| actor.life_seq)
                .is_some_and(|live_life_seq| {
                    live_life_seq == death.life_seq
                        || (live_life_seq != 0
                            && live_life_seq.wrapping_sub(death.life_seq) < (1_u32 << 31))
                });
        if !death_lifecycle_superseded {
            self.detach_player_and_children(death.killed_id);
        }
        if let Some(killed) = self.players.get_mut(&death.killed_id) {
            if !death_lifecycle_superseded {
                killed.set_life_seq(Some(death.life_seq));
                killed.clear_position();
                killed.velocity = Vec2::ZERO;
                killed.bounty = death.bounty;
                killed.flags = 0;
                killed.flag_timer = None;
            }
            killed.losses = killed.losses.saturating_add(1);
        }

        if let Some(killer) = self.players.get_mut(&death.killer_id) {
            killer.wins = killer.wins.saturating_add(1);
            if death.killer_id != death.killed_id {
                killer.flags = killer.flags.saturating_add(death.flag_transfer);
            }
        }
    }

    fn detach_player_and_children(&mut self, player_id: u16) {
        let was_attached = self
            .players
            .get(&player_id)
            .is_some_and(|player| player.attach_parent != INVALID_PLAYER_ID);
        if let Some(player) = self.players.get_mut(&player_id) {
            player.attach_parent = INVALID_PLAYER_ID;
            if was_attached {
                player.clear_position();
                player.velocity = Vec2::ZERO;
            }
        }
        for player in self.players.values_mut() {
            if player.attach_parent == player_id {
                player.attach_parent = INVALID_PLAYER_ID;
                player.clear_position();
                player.velocity = Vec2::ZERO;
            }
        }
    }

    pub(crate) fn visible_players(&self) -> impl Iterator<Item = &NetworkPlayer> {
        self.players
            .values()
            .filter(|player| Some(player.player_id) != self.viewer_player_id)
            .filter(|player| player.ship < 8)
            .filter(|player| player.attach_parent == INVALID_PLAYER_ID)
            .filter(|player| player.position.is_some())
    }

    pub(crate) fn visible_attached_players(
        &self,
        parent_id: u16,
    ) -> impl Iterator<Item = &NetworkPlayer> {
        self.players
            .values()
            .filter(move |player| player.attach_parent == parent_id)
            .filter(|player| Some(player.player_id) != self.viewer_player_id)
            .filter(|player| player.ship < 8)
    }

    /// Groups every drawable attached child under its parent in one world pass.
    ///
    /// The per-frame ship, radar and HUD-metrics paths each iterate every
    /// visible player, so querying `visible_attached_players` from inside those
    /// loops rescanned the whole world once per player. Grouping once keeps each
    /// path linear in the number of players and yields exactly the same children
    /// in the same ascending player-id order.
    pub(crate) fn attached_children_by_parent(&self) -> BTreeMap<u16, Vec<&NetworkPlayer>> {
        let mut grouped: BTreeMap<u16, Vec<&NetworkPlayer>> = BTreeMap::new();
        for player in self.players.values() {
            if player.attach_parent == INVALID_PLAYER_ID
                || Some(player.player_id) == self.viewer_player_id
                || player.ship >= 8
            {
                continue;
            }
            grouped
                .entry(player.attach_parent)
                .or_default()
                .push(player);
        }
        grouped
    }
}
