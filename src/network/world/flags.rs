use super::*;

impl NetworkWorld {
    pub(super) fn apply_flag_position_event(&mut self, position: &FlagPosition) {
        self.flag_legacy_position_events += 1;
        // The legacy 0x12 packet carries no generation and no version, and
        // replacing the entry wholesale used to wipe the carrier: the join
        // baseline's position leg holds the carrier's own flight coordinates, so
        // every networked carry collapsed into "on the map, unowned" seconds
        // after the server granted it. While somebody is carrying the flag only a
        // versioned authoritative objective-state update may re-present it.
        if self
            .flags
            .get(&position.flag_id)
            .is_some_and(|flag| flag.carried_by.is_some())
        {
            self.flag_legacy_position_guard_rejections += 1;
            return;
        }
        self.flags
            .insert(position.flag_id, NetworkFlag::from_position(*position));
    }

    pub(super) fn apply_flag_claim_event(&mut self, claim: &FlagClaim) {
        let mut carried_flag_claimed = false;
        if let Some(flag) = self.flags.get_mut(&claim.flag_id) {
            if flag.turf {
                if let Some(player) = self.players.get(&claim.player_id) {
                    flag.owner = player.frequency;
                }
            } else {
                flag.dropped = false;
                flag.carried_by = Some(claim.player_id);
                carried_flag_claimed = true;
            }
        } else {
            // The baseline sends position and claim as a pair and a peer can land
            // the claim first, or alone. Used to be a silent no-op, which is how a
            // client that joined mid-carry never showed the carrier at all.
            let claimant = self.players.get(&claim.player_id);
            self.flags.insert(
                claim.flag_id,
                NetworkFlag {
                    flag_id: claim.flag_id,
                    owner: claimant.map(|player| player.frequency).unwrap_or(u16::MAX),
                    position: claimant
                        .and_then(|player| player.position)
                        .unwrap_or(Vec2::ZERO),
                    dropped: false,
                    turf: false,
                    carried_by: Some(claim.player_id),
                },
            );
            carried_flag_claimed = true;
        }
        if carried_flag_claimed {
            if let Some(player) = self.players.get_mut(&claim.player_id) {
                player.flags = player.flags.saturating_add(1);
            }
        }
    }

    pub(super) fn apply_flag_victory_event(&mut self, victory: &FlagVictory) {
        self.last_flag_victory = Some(*victory);
    }

    pub(super) fn apply_drop_flag_event(&mut self, drop: &DropFlag) {
        if let Some(player) = self.players.get_mut(&drop.player_id) {
            player.flags = 0;
            player.flag_timer = None;
        }
        for flag in self.flags.values_mut() {
            if flag.carried_by == Some(drop.player_id) {
                flag.carried_by = None;
                // The server put it back on the map. Leaving `dropped` false
                // would keep `pickable_flag_id_near` filtering it out forever,
                // so the arena's flag could only ever be claimed once per client
                // session even though it is sitting on it.
                flag.dropped = true;
            }
        }
    }

    pub(super) fn apply_turf_flag_update_event(&mut self, update: &TurfFlagUpdate) {
        for (index, owner) in update.owners.iter().copied().enumerate() {
            let flag_id = index as u16;
            self.flags
                .entry(flag_id)
                .and_modify(|flag| {
                    flag.owner = owner;
                    flag.dropped = true;
                    flag.turf = true;
                    flag.carried_by = None;
                })
                .or_insert_with(|| NetworkFlag {
                    flag_id,
                    owner,
                    position: Vec2::ZERO,
                    dropped: true,
                    turf: true,
                    carried_by: None,
                });
        }
    }

    pub(super) fn apply_flag_reward_event(&mut self, reward: &FlagReward) {
        self.last_flag_rewards = reward.grants.clone();
    }

    pub(crate) fn apply_turf_flag_positions(&mut self, positions: &[Vec2]) {
        for (index, position) in positions.iter().copied().enumerate() {
            if let Some(flag) = self.flags.get_mut(&(index as u16)) {
                if flag.turf {
                    flag.position = position;
                }
            }
        }
    }

    pub(crate) fn pickable_flag_id_near(
        &self,
        pos: Vec2,
        radius: f32,
        viewer_frequency: u16,
    ) -> Option<u16> {
        self.flags
            .values()
            .filter(|flag| flag.dropped && !flag.turf && flag.carried_by.is_none())
            .filter(|flag| flag.owner != viewer_frequency)
            .filter(|flag| {
                (flag.position.x - pos.x).abs() <= radius
                    && (flag.position.y - pos.y).abs() <= radius
            })
            .min_by(|a, b| {
                a.position
                    .distance_squared(pos)
                    .total_cmp(&b.position.distance_squared(pos))
                    .then_with(|| a.flag_id.cmp(&b.flag_id))
            })
            .map(|flag| flag.flag_id)
    }
}
