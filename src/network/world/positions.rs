use super::*;

impl NetworkWorld {
    pub(super) fn apply_player_position_event(&mut self, update: &PlayerPositionUpdate) {
        self.apply_position_event(update, false, |player, update| {
            player.apply_position(update)
        });
    }

    pub(super) fn apply_snapshot_player_position_event(
        &mut self,
        update: &PlayerPositionUpdate,
        snapshot_id: u16,
        server_tick: u32,
        arena_stream_id: u64,
    ) {
        if self.authoritative_snapshot_id != Some(snapshot_id)
            || self.authoritative_snapshot_server_tick != Some(server_tick)
            || self.authoritative_snapshot_stream_id != Some(arena_stream_id)
            || update.timestamp != server_tick as u16
            || !self
                .pending_snapshot_position_provenance
                .get(&update.player_id)
                .is_some_and(|(id, tick, _, _, _)| *id == snapshot_id && *tick == server_tick)
        {
            return;
        }
        let (motion_segment, relocation_generation) = self
            .pending_snapshot_position_provenance
            .get(&update.player_id)
            .map(|(_, _, _, segment, relocation)| (*segment, *relocation))
            .unwrap_or_default();
        self.apply_position_event(update, true, |player, update| {
            player.snapshot_position_owned = true;
            let mut presented = update.clone();
            if player.accept_snapshot_motion_segment(motion_segment, relocation_generation) {
                presented.togglables |= STATUS_FLASH;
            }
            let snap_reason = player.apply_position_core(&presented);
            player.togglables = presented.togglables;
            NetworkPositionApply::Applied {
                flash_warp: player.take_flash_warp_effect(),
                snap_reason,
            }
        });
    }

    pub(super) fn apply_batched_small_player_position_event(
        &mut self,
        update: &PlayerPositionUpdate,
    ) {
        self.apply_position_event(update, false, |player, update| {
            player.apply_position_without_togglables(update)
        });
    }

    pub(super) fn apply_batched_large_player_position_event(
        &mut self,
        update: &PlayerPositionUpdate,
    ) {
        self.apply_position_event(update, false, |player, update| {
            player.apply_position_preserving_high_togglables(update)
        });
    }

    fn apply_position_event(
        &mut self,
        update: &PlayerPositionUpdate,
        consume_snapshot_provenance: bool,
        apply: impl FnOnce(&mut NetworkPlayer, &PlayerPositionUpdate) -> NetworkPositionApply,
    ) {
        let is_self = self.viewer_player_id == Some(update.player_id);
        if !is_self {
            self.stats.remote_position_updates = self.stats.remote_position_updates.wrapping_add(1);
        }

        let snapshot_provenance = consume_snapshot_provenance
            .then(|| {
                self.pending_snapshot_position_provenance
                    .remove(&update.player_id)
            })
            .flatten();
        let Some(player) = self.players.get_mut(&update.player_id) else {
            return;
        };
        // After snapshot ownership starts, a delayed legacy bootstrap/relay
        // cannot compete with it, even if 16-bit wrap makes that packet look
        // newer. AOI eviction and life changes do not restore legacy ownership.
        let result = if !consume_snapshot_provenance && player.snapshot_position_owned {
            NetworkPositionApply::Stale
        } else {
            apply(player, update)
        };

        match result {
            NetworkPositionApply::Applied {
                flash_warp,
                snap_reason,
            } => {
                if !is_self && snap_reason != RemoteSnapReason::None {
                    player.record_presentation_snap(snap_reason);
                    self.stats.remote_snap_count = self.stats.remote_snap_count.wrapping_add(1);
                    self.stats.last_remote_snap_reason = snap_reason.code();
                }
                if !is_self
                    && let Some((snapshot_id, server_tick, Some(sample_time), _, _)) =
                        snapshot_provenance
                    && update.timestamp == server_tick as u16
                {
                    player.record_directed_source_snapshot(
                        snapshot_id,
                        server_tick,
                        sample_time,
                        snap_reason,
                    );
                }
                if let Some(position) = flash_warp {
                    self.pending_warp_effects.push(position);
                }
            }
            NetworkPositionApply::Stale => {
                if !is_self {
                    self.stats.remote_position_stale_drops =
                        self.stats.remote_position_stale_drops.wrapping_add(1);
                }
            }
        }
    }
}
