use super::*;

impl NetworkWorld {
    pub(crate) fn viewer_player_id(&self) -> Option<u16> {
        self.viewer_player_id
    }

    pub(crate) fn snapshot_epoch(&self) -> Option<u32> {
        self.snapshot_epoch
    }

    #[cfg(test)]
    pub(crate) fn install_test_viewer_player_id(&mut self, player_id: Option<u16>) {
        self.viewer_player_id = player_id;
    }

    pub(crate) fn reset_presentation_scope(&mut self, viewer_player_id: Option<u16>) {
        self.viewer_player_id = viewer_player_id;
        self.ufo_available = false;
        self.personal_koth_timer = None;
        self.koth_timer = None;
        self.koth_timer_adding = false;
        self.koth_player_id = None;
        self.personal_koth_remainder = 0.0;
        self.koth_remainder = 0.0;
        self.players.clear();
        self.personal_banner = None;
        self.voice_messages.clear();
        self.transferred_files.clear();
        self.powerballs.clear();
        self.soccer_scores.clear();
        self.arena_directory.clear();
        self.last_zone_banner = None;
        self.client_file_requests.clear();
        self.flags.clear();
        self.brick_lines.clear();
        if !self.objective_versions.is_empty() {
            self.flag_version_resets += 1;
        }
        self.objective_versions.clear();
        self.objective_score_versions.clear();
        self.last_flag_victory = None;
        self.last_flag_rewards.clear();
        self.last_speed_game_over = None;
        self.pending_warp_effects.clear();
        self.lvz = NetworkLvzState::default();
        self.stats = NetworkWorldStats::default();
        self.last_received_snapshot_id = None;
        self.authoritative_snapshot_id = None;
        self.authoritative_snapshot_server_tick = None;
        self.authoritative_snapshot_stream_id = None;
        self.pending_snapshot_position_provenance.clear();
        self.snapshot_epoch = None;
        self.authoritative_snapshot_actors.clear();
        self.authoritative_snapshot_actor_trace.clear();
        self.authoritative_snapshot_actor_serial = 0;
        self.authoritative_snapshot_actor_overflow = 0;
        self.authoritative_snapshot_actor_metrics_cursor.set(None);
        self.snapshot_entity_state.clear();
    }

    pub(super) fn apply_disconnected_event(&mut self) {
        self.reset_presentation_scope(None);
    }

    pub(super) fn apply_player_id_assigned_event(&mut self, player_id: u16) {
        self.reset_presentation_scope(Some(player_id));
    }

    pub(super) fn apply_arena_presentation_joined_event(&mut self, snapshot_epoch: u32) {
        self.brick_lines.clear();
        self.last_received_snapshot_id = None;
        self.authoritative_snapshot_id = None;
        self.authoritative_snapshot_server_tick = None;
        self.authoritative_snapshot_stream_id = None;
        self.pending_snapshot_position_provenance.clear();
        self.snapshot_epoch = Some(snapshot_epoch);
        self.authoritative_snapshot_actors.clear();
        self.authoritative_snapshot_actor_trace.clear();
        self.authoritative_snapshot_actor_serial = 0;
        self.authoritative_snapshot_actor_overflow = 0;
        self.authoritative_snapshot_actor_metrics_cursor.set(None);
        self.snapshot_entity_state.clear();
        self.lvz.reset_arena_entry_age();
    }

    pub(super) fn apply_lvz_toggled_event(&mut self, packet: &LvzTogglePacket) {
        self.lvz.apply_toggle(packet);
    }

    pub(super) fn apply_lvz_modified_event(&mut self, packet: &LvzModifyPacket) {
        self.lvz.apply_modify(*packet);
    }

    /// Consume the immutable action plan produced from the sequencer entry
    /// view. NetworkWorld does not keep or advance an authority cursor of its
    /// own; it only resets presentation state when the admitted command says
    /// a hard replacement occurred.
    pub(crate) fn apply_planned_action(
        &mut self,
        event: &ClientProtocolEvent,
        admission: Option<&AuthorityEventAdmission>,
    ) -> bool {
        if let Some(admission) = admission {
            if matches!(event, ClientProtocolEvent::PreGameplayAuthorityReset) {
                return true;
            }
            if let AuthorityBoundaryAdmission::Hard { replacement, .. } = admission.boundary {
                self.reset_presentation_scope(replacement.and_then(|scope| scope.player_id));
            }
            let rejected_authority_fact = admission.private_fact
                == PrivateAuthorityAdmission::Reject
                || admission.action == AuthorityActionAdmission::Reject;
            if rejected_authority_fact {
                return false;
            }
        }
        self.apply_event(event);
        true
    }

    pub(crate) fn apply_actions(&mut self, actions: &ClientProtocolActions) {
        for (index, event) in actions.events.iter().enumerate() {
            self.apply_planned_action(event, actions.authority_admission.get(index));
        }
    }

    pub(crate) fn update_remote_players(&mut self, dt: f32, directed_render_tick: Option<u32>) {
        let self_id = self.viewer_player_id;
        for player in self.players.values_mut() {
            if Some(player.player_id) != self_id
                && let Some(render_tick) = directed_render_tick
            {
                player.update_directed_render_position(render_tick, dt);
                continue;
            }
            let extrapolated = player.update_render_position(dt);
            if extrapolated && Some(player.player_id) != self_id {
                self.stats.remote_extrapolated_frames =
                    self.stats.remote_extrapolated_frames.wrapping_add(1);
            }
        }
    }

    pub(crate) fn drain_pending_warp_effects(&mut self) -> impl Iterator<Item = Vec2> + '_ {
        self.pending_warp_effects.drain(..)
    }
}
