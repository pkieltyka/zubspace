use super::gauges::{
    REFERENCE_GAME_HEALTHBAR_WIDTH, energy_digit_count, energy_digits_start_x,
    reference_game_energy_half_width,
};
use super::indicators::{left_item_panel_bounds, right_status_panel_bounds};
use super::*;

// The always-on lifecycle channel publishes only when this changes, so a steady frame pays nothing for it.
// Held here rather than on `Game` because the publish site receives `&Game`, and none of this exists natively.
thread_local! {
    static PUBLISHED_LIFECYCLE_STATE: std::cell::Cell<Option<(bool, bool, bool, bool, bool, bool)>> =
        const { std::cell::Cell::new(None) };
    static LAST_PLAYER_REPORT: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

const MAX_REMOTE_PROJECTILE_SOURCE_METRIC_ENTRIES: usize = 64;
const MAX_REMOTE_DECOY_METRIC_ENTRIES: usize = 64;

impl Game {
    pub(super) fn publish_hud_ui_metrics(
        &self,
        local: &LocalRenderPrediction,
        energy_hud_x: f32,
        hud_lines: &[HudTextLine],
        radar_x: f32,
        radar_y: f32,
        radar_dim: f32,
    ) {
        // The diagnostics payload walks remote-player state, measures several
        // text layouts, and allocates JSON. Browser diagnostics request it
        // every frame only for `?trace=1`, or at a deliberately low cadence
        // for `?metrics=1`; ordinary gameplay leaves it disabled.
        // The lifecycle channel is deliberately separate from the gate below. Automated drivers must be
        // able to read readiness on the shipped configuration, where nobody asked for diagnostics, and
        // `tests/rtc_config.test.mjs` rightly refuses to let ordinary play build the full payload. A
        // change-only publish satisfies both: no measurement, no allocation on a steady frame.
        #[cfg(target_arch = "wasm32")]
        {
            crate::browser_metrics::publish_player_frame(
                self.self_correction_count,
                self.step_authority_ready(),
            );
            let scope = self.client_authority.view().scope;
            crate::browser_metrics::publish_connection_scope(
                scope.server_session_id.unwrap_or(0),
                scope.arena_stream_id.unwrap_or(0),
            );
            let now = get_time();
            if LAST_PLAYER_REPORT.with(|last| now - last.get() >= 5.0) {
                LAST_PLAYER_REPORT.with(|last| last.set(now));
                if let (Some(server), Some(arena), Some(life)) = (
                    scope.server_session_id,
                    scope.arena_stream_id,
                    scope.life_seq,
                ) {
                    crate::browser_metrics::publish_player_report(server, arena, life);
                }
            }
        }
        let zone_map_ready = self.network_zone_map_ready;
        // Assignment is distinct from actual controls readiness. Baseline
        // recovery tests deliberately observe the former while holding the
        // latter closed; the ordinary splash must wait for the latter.
        let authority_scope = self.client_authority.view();
        let arena_assigned =
            authority_scope.scope.life_seq.is_some() && self.last_self_server_energy.is_some();
        let in_gameplay = self.initial_presentation_ready();
        let gameplay_ready = self.gameplay_step_ready();
        let lifecycle = (
            zone_map_ready,
            self.network_zone_files_ready,
            arena_assigned,
            in_gameplay,
            gameplay_ready,
            self.spectator_mode,
        );
        if PUBLISHED_LIFECYCLE_STATE.with(|cell| cell.get()) != Some(lifecycle) {
            PUBLISHED_LIFECYCLE_STATE.with(|cell| cell.set(Some(lifecycle)));
            crate::browser_metrics::publish_lifecycle_state(&format!(
                "{{\"zoneMapReady\":{},\"zoneFilesReady\":{},\"arenaAssigned\":{},\"inGameplay\":{},\"gameplayStepReady\":{},\"spectator\":{},\"channel\":\"lifecycle\"}}",
                zone_map_ready,
                self.network_zone_files_ready,
                arena_assigned,
                in_gameplay,
                gameplay_ready,
                self.spectator_mode
            ));
        }
        if !ui_metrics_requested() {
            return;
        }
        let measure =
            self.measure_hud_metrics(hud_lines, radar_x, radar_y, radar_dim, energy_hud_x);
        publish_ui_metrics(
            &self.hud_ui_metrics_payload(local, hud_lines, radar_x, radar_y, measure),
        );
    }

    // The JSON is assembled here, apart from the `?trace=1` request gate and away
    // from any font or sprite-sheet read, so a native test can build the exact
    // payload the browser JSON.parses. Headless games have no `Assets` at all - the
    // test stub panics on touch by design - so every measured number arrives as an
    // argument instead.
    pub(crate) fn hud_ui_metrics_payload(
        &self,
        local: &LocalRenderPrediction,
        hud_lines: &[HudTextLine],
        radar_x: f32,
        radar_y: f32,
        measure: HudMetricsMeasure,
    ) -> String {
        let notification_messages = format!(
            "[{}]",
            self.notifications
                .iter()
                .map(|notice| json_metric_string(Some(&notice.message)))
                .collect::<Vec<_>>()
                .join(",")
        );
        let HudMetricsMeasure {
            screen_width,
            screen_height,
            energy_inside,
            energy_center_delta,
            energy_digits_inside,
            energy_digits_right_aligned,
            hud_text_inside,
            hud_radar_overlap,
            radar_bottom_right,
            radar_label_inside,
            right_status_panel_inside,
            right_status_panel_clear_of_radar,
            left_item_panel_inside,
            label_inside,
            label_scale,
            label_width,
            notification_inside,
            notification_scale,
            notification_width,
            text_positive_scale,
        } = measure;
        let local_render_prediction = format!(
            "{{\"x\":{},\"y\":{},\"velocityX\":{},\"velocityY\":{},\"orientation\":{},\"inputBits\":{},\"partialSeconds\":{},\"spawnPosePending\":{},\"shipPresentable\":{}}}",
            json_metric_number_with_precision(Some(local.position.x), 6),
            json_metric_number_with_precision(Some(local.position.y), 6),
            json_metric_number_with_precision(Some(local.velocity.x), 6),
            json_metric_number_with_precision(Some(local.velocity.y), 6),
            json_metric_number_with_precision(Some(local.orientation), 8),
            local.input_bits,
            json_metric_number_with_precision(Some(local.partial_seconds), 8),
            self.local_spawn_pose_pending(),
            self.local_ship_pose_presentable(),
        );
        let viewer_frequency = self.viewer_frequency();
        let remote_visible_players = self
            .network_world
            .visible_players()
            .filter(|player| {
                player.position.is_some()
                    && network_player_world_visible(
                        player.togglables,
                        player.frequency,
                        viewer_frequency,
                        self.ship.xradar,
                    )
            })
            .count();
        let remote_render_sample = self
            .network_world
            .visible_players()
            .find(|player| {
                player.position.is_some()
                    && network_player_world_visible(
                        player.togglables,
                        player.frequency,
                        viewer_frequency,
                        self.ship.xradar,
                    )
            })
            .and_then(|player| {
                Some((
                    player.display_position()?,
                    player.position?,
                    player.display_orientation(),
                    player.display_energy(),
                    player.frequency,
                ))
            });
        let remote_display_x =
            json_metric_number(remote_render_sample.map(|(display, _, _, _, _)| display.x));
        let remote_display_y =
            json_metric_number(remote_render_sample.map(|(display, _, _, _, _)| display.y));
        let remote_position_x =
            json_metric_number(remote_render_sample.map(|(_, position, _, _, _)| position.x));
        let remote_position_y =
            json_metric_number(remote_render_sample.map(|(_, position, _, _, _)| position.y));
        let remote_display_orientation =
            json_metric_number(remote_render_sample.map(|(_, _, orientation, _, _)| orientation));
        let remote_energy =
            json_metric_number(remote_render_sample.and_then(|(_, _, _, energy, _)| energy));
        let remote_frequency =
            json_metric_u16(remote_render_sample.map(|(_, _, _, _, frequency)| frequency));
        let remote_players = remote_players_metrics_json(self, viewer_frequency);
        let scoreboard_player_ids = scoreboard_player_ids_metrics_json(self);
        let scoreboard_player_stats = scoreboard_player_stats_metrics_json(self);
        let scoreboard_rows = self.player_list_rows();
        let (scoreboard_bounds, scoreboard_visible_rows) =
            Self::player_list_geometry(screen_width, screen_height, scoreboard_rows.len());
        let scoreboard_view = format!(
            "{{\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"visibleRows\":{},\"selectedRow\":{},\"selectedPlayerId\":{}}}",
            scoreboard_bounds.x,
            scoreboard_bounds.y,
            scoreboard_bounds.w,
            scoreboard_bounds.h,
            scoreboard_visible_rows,
            self.scoreboard_scroll.selected,
            json_metric_u16(
                scoreboard_rows
                    .get(self.scoreboard_scroll.selected)
                    .and_then(|r| r.player_id)
            ),
        );
        let radar_player_ids =
            radar_player_ids_metrics_json(self, viewer_frequency, screen_width, screen_height);
        let authoritative_snapshot_actors = authoritative_snapshot_actors_metrics_json(self);
        let authoritative_snapshot_actor_trace =
            authoritative_snapshot_actor_trace_metrics_json(self);
        let authoritative_snapshot_actor_stream = authoritative_snapshot_actor_stream_metrics_json(
            self,
            &authoritative_snapshot_actor_trace,
        );
        let remote_actor_presentation = remote_actor_presentation_metrics_json(self);
        let authority = self.client_authority.view();
        let self_player_id = json_metric_u16(authority.scope.player_id);
        // Keep the local ship and admitted roster witness separate: agreement
        // must be observed, never inferred from shared maximum energy.
        let player_ship = self.ship.ship_type;
        let self_roster_ship = json_metric_u16(
            authority
                .scope
                .player_id
                .and_then(|id| self.network_world.players.get(&id))
                .map(|player| u16::from(player.ship)),
        );
        // Lifecycle identity is sequencer authority, not presentation-roster
        // state. A snapshot may render the self actor before NetworkWorld's
        // compatibility player row learns its life; exposing that alias here
        // previously made a completed initial baseline look permanently
        // unready to browser acceptance and diagnostics.
        let player_life_seq = json_metric_u32(authority.scope.life_seq);
        let player_frequency = json_metric_u16(Some(viewer_frequency));
        let last_network_damage_player_id = json_metric_u16(self.last_network_damage_player_id);
        let last_network_damage_attacker_id = json_metric_u16(self.last_network_damage_attacker_id);
        let last_network_damage_energy = json_metric_number(self.last_network_damage_energy);
        let last_network_damage_energy_before =
            json_metric_number(self.last_network_damage_energy_before);
        let last_network_damage_amount = json_metric_u16(self.last_network_damage_amount);
        let last_network_damage_weapon = json_metric_u16(self.last_network_damage_weapon);
        let last_network_damage_server_tick = json_metric_u32(self.last_network_damage_server_tick);
        let last_network_damage_server_seq = json_metric_u32(self.last_network_damage_server_seq);
        let last_network_damage_victim_life_seq =
            json_metric_u32(self.last_network_damage_victim_life_seq);
        let last_network_damage_shooter_life_seq =
            json_metric_u32(self.last_network_damage_shooter_life_seq);
        let last_self_damage_attacker_id = json_metric_u16(self.last_self_damage_attacker_id);
        let last_self_damage_energy = json_metric_number(self.last_self_damage_energy);
        let last_self_damage_energy_before =
            json_metric_number(self.last_self_damage_energy_before);
        let last_self_damage_amount = json_metric_u16(self.last_self_damage_amount);
        let last_self_damage_weapon = json_metric_u16(self.last_self_damage_weapon);
        let last_self_damage_server_tick = json_metric_u32(self.last_self_damage_server_tick);
        let last_self_damage_server_seq = json_metric_u32(self.last_self_damage_server_seq);
        let last_self_damage_victim_life_seq =
            json_metric_u32(self.last_self_damage_victim_life_seq);
        let last_self_damage_shooter_life_seq =
            json_metric_u32(self.last_self_damage_shooter_life_seq);
        let self_score = authority
            .scope
            .player_id
            .and_then(|player_id| self.network_world.players.get(&player_id))
            .map(|player| (player.wins, player.losses));
        let self_wins = json_metric_u16(self_score.map(|(wins, _)| wins));
        let self_losses = json_metric_u16(self_score.map(|(_, losses)| losses));
        let client_input_seq = self.next_input_seq;
        #[cfg(target_arch = "wasm32")]
        let (map_checksum, arena_settings_checksum) = self
            .browser_network
            .as_ref()
            .map(|browser| {
                let session = browser.client.session();
                let readiness = self.client_authority.view().readiness;
                (
                    readiness.map.then_some(session.continuum_map_crc32),
                    readiness
                        .settings
                        .then_some(session.arena_settings_crc32)
                        .flatten(),
                )
            })
            .unwrap_or((None, None));
        #[cfg(not(target_arch = "wasm32"))]
        let (map_checksum, arena_settings_checksum) = (None, None);
        let map_checksum = json_metric_u32(map_checksum);
        let arena_settings_checksum = json_metric_u32(arena_settings_checksum);
        #[cfg(target_arch = "wasm32")]
        let browser_network_owned = self.browser_network.is_some();
        #[cfg(not(target_arch = "wasm32"))]
        let browser_network_owned = false;
        #[cfg(target_arch = "wasm32")]
        let latest_server_tick = json_metric_u32(
            self.browser_network
                .as_ref()
                .and_then(|browser| browser.server_clock.current_server_tick()),
        );
        #[cfg(not(target_arch = "wasm32"))]
        let latest_server_tick = json_metric_u32(None);
        #[cfg(target_arch = "wasm32")]
        let latest_snapshot_id = json_metric_u16(
            self.browser_network
                .as_ref()
                .and_then(|browser| browser.client.session().latest_received_snapshot_id),
        );
        #[cfg(not(target_arch = "wasm32"))]
        let latest_snapshot_id = json_metric_u16(None);
        // The client's own outbound counters. They already exist and drive the HUD; publishing them is what lets a
        // retained artifact distinguish "this client stopped reporting its position" from "the server stopped delivering
        // one", which is the question the blind-observer item has been unable to answer from artifacts. These move every
        // frame, so they belong in the opt-in block and never on the always-on lifecycle channel.
        #[cfg(target_arch = "wasm32")]
        let (positions_sent, positions_sent_per_second) = self
            .browser_network
            .as_ref()
            .map(|browser| {
                (
                    browser.stats.positions_sent,
                    browser.stats.positions_sent_per_second,
                )
            })
            .unwrap_or((0, 0.0));
        #[cfg(not(target_arch = "wasm32"))]
        let (positions_sent, positions_sent_per_second) = (0u32, 0.0f32);
        // Read from the SAME object as positions_sent, on purpose. `browser_network` is moved out of and back into an
        // Option across the frame, so a lone zero in positionsSent cannot distinguish "this client never sent a
        // position" from "metrics were read while that option was temporarily empty". packetsSent comes from the same
        // borrow: packets non-zero with positions zero is a real emission gap, both zero is this read artifact.
        #[cfg(target_arch = "wasm32")]
        let (packets_sent, packets_received) = self
            .browser_network
            .as_ref()
            .map(|browser| browser.client.packet_totals())
            .unwrap_or((0, 0));
        #[cfg(not(target_arch = "wasm32"))]
        let (packets_sent, packets_received) = (0u32, 0u32);
        // The decisive flag for the blindness class of bug. A frame that is not gameplay-step-ready takes the unready
        // path in src/game/player/lifecycle.rs, which calls update_browser_network(render_dt, false): the position timer
        // advances but every fire lands on final_catch_up_step = false, so no position is ever emitted while standalone
        // input packets keep flowing every frame. That is exactly the measured profile - positionsSent 0 with packetsSent
        // 4050 - and it means a client can look like it is playing (it receives, renders, has life) while it has never
        // told the server where it is, so no peer can ever be shown it.
        #[cfg(target_arch = "wasm32")]
        let gameplay_step_ready = self.gameplay_step_ready();
        #[cfg(not(target_arch = "wasm32"))]
        let gameplay_step_ready = false;
        // The two halves of that predicate, published separately on purpose. gameplayStepReady=false on a stranded client
        // means either the step authority is not ready or a network rebase is still suppressing the step, and those have
        // different causes and different fixes; a single boolean cannot tell them apart after the fact.
        #[cfg(target_arch = "wasm32")]
        let step_authority_ready = self.step_authority_ready();
        #[cfg(not(target_arch = "wasm32"))]
        let step_authority_ready = false;
        #[cfg(target_arch = "wasm32")]
        let rebase_suppressed = self.pre_step_network_rebase_suppressed();
        #[cfg(not(target_arch = "wasm32"))]
        let rebase_suppressed = false;
        // The page's own transport verdict, kept as the raw code: 3 and 4 mean different things to a reader who knows
        // which is which, and collapsing them would hide whether a grace window is still running.
        //
        // The boolean beside it is the recovery precondition, computed by the page because the code alone cannot answer
        // it: 3 covers both a grace window that a rejoin must not interrupt and a close that is final, and 4 also covers
        // a signaling failure that never reached open, where the existing connect-failure UI is already the right answer.
        // The page can state the conjunction - it opened once and it is gone now - which is what a bounded rejoin waits for.
        #[cfg(target_arch = "wasm32")]
        let rtc_transport_status = crate::transport::browser_rtc_status_code();
        // One source for the terminal predicate: the raw code above cannot distinguish a disconnect grace window from a
        // final close, so the boolean is what readiness gating and the recovery trigger consult.
        let rtc_transport_terminal_active = crate::transport::browser_rtc_transport_is_terminal();
        let rtc_transport_terminal = i32::from(rtc_transport_terminal_active);
        // The rejoin count makes a recovery attributable from the client's own numbers, so a lane can assert that it
        // happened instead of inferring it from a peer cycling.
        #[cfg(target_arch = "wasm32")]
        let rtc_rejoin_count = self
            .browser_network
            .as_ref()
            .map_or(0, |browser| browser.rejoin_count);
        #[cfg(not(target_arch = "wasm32"))]
        let rtc_rejoin_count = 0_u32;
        #[cfg(not(target_arch = "wasm32"))]
        let rtc_transport_status = 0_i32;
        #[cfg(target_arch = "wasm32")]
        let latest_snapshot_server_tick =
            json_metric_u32(self.browser_network.as_ref().and_then(|browser| {
                browser
                    .client
                    .session()
                    .latest_received_snapshot_server_tick
            }));
        #[cfg(not(target_arch = "wasm32"))]
        let latest_snapshot_server_tick = json_metric_u32(None);
        #[cfg(target_arch = "wasm32")]
        let latest_snapshot_base_id = json_metric_u16(
            self.browser_network
                .as_ref()
                .and_then(|browser| browser.client.session().latest_received_snapshot_base_id),
        );
        #[cfg(not(target_arch = "wasm32"))]
        let latest_snapshot_base_id = json_metric_u16(None);
        #[cfg(target_arch = "wasm32")]
        let last_acked_snapshot_id = json_metric_u16(
            self.browser_network
                .as_ref()
                .and_then(|browser| browser.client.session().latest_acked_snapshot_id),
        );
        #[cfg(not(target_arch = "wasm32"))]
        let last_acked_snapshot_id = json_metric_u16(None);
        #[cfg(target_arch = "wasm32")]
        let last_acked_snapshot_server_tick = json_metric_u32(
            self.browser_network
                .as_ref()
                .and_then(|browser| browser.client.session().latest_acked_snapshot_server_tick),
        );
        #[cfg(not(target_arch = "wasm32"))]
        let last_acked_snapshot_server_tick = json_metric_u32(None);
        let in_safe_zone = self.in_safe_zone();
        let bomb_cooldown = json_metric_number(Some(self.ship.bomb_cooldown.max(0.0)));
        let pending_weapon = json_metric_u16(Some(self.pending_weapon));
        let visible_energy = self.visible_hud_energy();
        let player_energy = json_metric_number(Some(visible_energy));
        let player_predicted_energy = json_metric_number(Some(self.ship.energy));
        let local_ship_label_energy = json_metric_number(Some(visible_energy));
        let visible_energy_digits = visible_energy.max(0.0) as u32;
        let player_max_energy = json_metric_number(Some(self.ship.max_energy));
        let server_player_energy = json_metric_number(self.last_self_server_energy);
        let player_alive = !self.spectator_mode && self.player_respawn_timer <= 0.0;
        let server_player_alive = json_metric_bool(self.client_authority.view().scope.alive);
        let player_respawn_timer = json_metric_number(Some(self.player_respawn_timer.max(0.0)));
        let player_respawn_seq = self.player_respawn_seq;
        let player_x = json_metric_number(Some(self.ship.pos.x));
        let player_y = json_metric_number(Some(self.ship.pos.y));
        let player_velocity_x = json_metric_number(Some(self.ship.vel.x));
        let player_velocity_y = json_metric_number(Some(self.ship.vel.y));
        let player_orientation = json_metric_number_with_precision(Some(self.ship.orientation), 5);
        let player_protocol_direction =
            crate::continuum_position::protocol_direction(self.ship.orientation);
        let remote_projectile_count = self.remote_projectiles.len();
        let remote_bomb_count = self
            .remote_projectiles
            .iter()
            .filter(|projectile| matches!(projectile.kind, RemoteProjectileKind::Bomb { .. }))
            .count();
        let remote_bullet_count = self
            .remote_projectiles
            .iter()
            .filter(|projectile| matches!(projectile.kind, RemoteProjectileKind::Bullet { .. }))
            .count();
        let remote_mine_count = self
            .remote_projectiles
            .iter()
            .filter(|projectile| matches!(projectile.kind, RemoteProjectileKind::Mine { .. }))
            .count();
        let remote_projectile_source_metric_overflow =
            remote_projectile_count.saturating_sub(MAX_REMOTE_PROJECTILE_SOURCE_METRIC_ENTRIES);
        let remote_projectile_sources =
            remote_projectile_sources_metrics_json(self, &self.remote_projectiles);
        // Installed runtime state can intentionally precede the delayed birth draw.
        // Keep that clock separate from the immutable view submitted this frame.
        let presented_remote_projectile_sources =
            remote_projectile_sources_metrics_json(self, &self.prepared_remote_projectiles);
        let presented_remote_projectile_source_overflow = self
            .prepared_remote_projectiles
            .len()
            .saturating_sub(MAX_REMOTE_PROJECTILE_SOURCE_METRIC_ENTRIES);
        let remote_decoy_count = self
            .decoys
            .iter()
            .filter(|decoy| decoy.remote_source.is_some())
            .count();
        let remote_decoy_metric_overflow =
            remote_decoy_count.saturating_sub(MAX_REMOTE_DECOY_METRIC_ENTRIES);
        let remote_decoys = remote_decoys_metrics_json(self);
        let semantic_input_counts = format!(
            "{{\"primary\":{},\"repel\":{},\"rocket\":{},\"brick\":{},\"decoy\":{},\"thor\":{},\"burst\":{},\"cloak\":{},\"antiwarp\":{},\"portal\":{},\"warp\":{},\"bomb\":{}}}",
            self.semantic_input_counts[0],
            self.semantic_input_counts[1],
            self.semantic_input_counts[2],
            self.semantic_input_counts[3],
            self.semantic_input_counts[4],
            self.semantic_input_counts[5],
            self.semantic_input_counts[6],
            self.semantic_input_counts[7],
            self.semantic_input_counts[8],
            self.semantic_input_counts[9],
            self.semantic_input_counts[10],
            self.semantic_input_counts[11],
        );
        let (door_seed, door_seed_next, door_seed_after_next, door_seed_third_next) = {
            let map = self.map();
            let mut rng_seed = map.door_rng_seed;
            let next =
                crate::door::next_door_seed(map.door_seed, &mut rng_seed, self.tuning.door_mode);
            let after_next =
                crate::door::next_door_seed(next, &mut rng_seed, self.tuning.door_mode);
            let third_next =
                crate::door::next_door_seed(after_next, &mut rng_seed, self.tuning.door_mode);
            (map.door_seed, next, after_next, third_next)
        };
        #[cfg(target_arch = "wasm32")]
        let (
            reliable_next_process_id,
            reliable_queued_packets,
            pending_file_requests,
            pending_file_front_index,
            file_request_in_flight,
            file_request_reissues,
            file_request_frames_since_send,
            pending_zone_downloads,
            pending_zone_publication,
            file_chunk_index_mismatches,
            file_chunks_accepted,
            file_chunk_assembler_rejects,
            small_chunk_bytes,
            small_chunk_fragments,
            reliable_duplicate_drops,
            reliable_queue_full_drops,
            acks_emitted,
            acks_emitted_max_id,
        ) = self
            .browser_network
            .as_ref()
            .map(|browser| {
                (
                    browser.client.session().sequencer.next_process_id,
                    browser.client.session().sequencer.queued.len(),
                    browser.client.session().pending_file_requests.len(),
                    browser
                        .client
                        .session()
                        .pending_file_requests
                        .front()
                        .map(|request| request.index),
                    browser.client.session().file_request_in_flight,
                    browser.client.session().file_request_reissues,
                    browser.client.session().file_request_frames_since_send,
                    browser.client.session().pending_zone_downloads.len(),
                    browser.client.session().pending_zone_publication,
                    browser.client.session().file_chunk_index_mismatches,
                    browser.client.session().file_chunks_accepted,
                    browser.client.session().file_chunk_assembler_rejects,
                    browser.client.session().chunker.small.len,
                    browser.client.session().chunker.small.fragments,
                    browser.client.session().sequencer.duplicate_drops,
                    browser.client.session().sequencer.queue_full_drops,
                    browser.client.session().emitted_ack_stats().0,
                    browser.client.session().emitted_ack_stats().1,
                )
            })
            // An explicit default: std only implements Default for tuples up to twelve elements, and this tuple grew past
            // that with the two reliable-drop counters.
            .unwrap_or((
                0_u32, 0_usize, 0_usize, None, None, 0_u8, 0_u32, 0_usize, false, 0_u32, 0_u32,
                0_u32, 0_usize, 0_usize, 0_u32, 0_u32, 0_u32, 0_u32,
            ));
        #[cfg(not(target_arch = "wasm32"))]
        let (
            reliable_next_process_id,
            reliable_queued_packets,
            pending_file_requests,
            pending_file_front_index,
            file_request_in_flight,
            file_request_reissues,
            file_request_frames_since_send,
            pending_zone_downloads,
            pending_zone_publication,
            file_chunk_index_mismatches,
            file_chunks_accepted,
            file_chunk_assembler_rejects,
            small_chunk_bytes,
            small_chunk_fragments,
            reliable_duplicate_drops,
            reliable_queue_full_drops,
            acks_emitted,
            acks_emitted_max_id,
        ) = (
            0_u32, 0_usize, 0_usize, None, None, 0_u8, 0_u32, 0_usize, false, 0_u32, 0_u32, 0_u32,
            0_usize, 0_usize, 0_u32, 0_u32, 0_u32, 0_u32,
        );
        // HUD metrics are published after every objective draw call in
        // `Game::draw`, so these fields are retained evidence for the render
        // policy that was exercised in this exact frame.
        let local_flag_fallback_presented = !self.networked_play
            && (self.carrying_flag
                || self.dropped_flag.is_some()
                || self
                    .map()
                    .objects
                    .iter()
                    .any(|object| object.id == FLAG_TILE_ID));
        let local_powerball_fallback_presented = !self.networked_play
            && self.network_world.powerballs.is_empty()
            && self.tuning.soccer_mode != 0;
        let active_prize_collectible_ids = format!(
            "[{}]",
            self.prizes
                .iter()
                .map(|prize| prize.collectible_id.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let pending_initial_position =
            authority.initial_baseline_pending && authority.initial_baseline_delivery_id.is_none();
        let lifecycle_receipts_seen = authority.lifecycle_receipts_seen;
        let lifecycle_receipts_matched = authority.lifecycle_receipts_matched;
        let lifecycle_receipts_unmatched = authority.lifecycle_receipts_unmatched;
        let authority_phase = format!("\"{:?}\"", authority.phase);
        let authority_transition_kind = authority
            .transition_kind
            .map_or_else(|| "null".to_string(), |kind| format!("\"{kind:?}\""));
        let authority_application_barrier_kind = authority
            .application_barrier_kind
            .map_or_else(|| "null".to_string(), |kind| format!("\"{kind:?}\""));
        let authority_last_transition_source = authority.last_transition.map_or_else(
            || "null".to_string(),
            |entry| format!("\"{:?}\"", entry.source),
        );
        let authority_last_transition_fact = authority.last_transition.map_or_else(
            || "null".to_string(),
            |entry| format!("\"{:?}\"", entry.fact),
        );
        let authority_last_transition_prior = authority.last_transition.map_or_else(
            || "null".to_string(),
            |entry| format!("\"{:?}\"", entry.prior),
        );
        let authority_last_transition_disposition = authority.last_transition.map_or_else(
            || "null".to_string(),
            |entry| format!("\"{:?}\"", entry.disposition),
        );
        let authority_last_transition_resulting = authority.last_transition.map_or_else(
            || "null".to_string(),
            |entry| format!("\"{:?}\"", entry.resulting),
        );
        let authority_decisive_fact = authority.last_decisive_transition.map_or_else(
            || "null".to_string(),
            |entry| format!("\"{:?}\"", entry.fact),
        );
        let authority_decisive_disposition = authority.last_decisive_transition.map_or_else(
            || "null".to_string(),
            |entry| format!("\"{:?}\"", entry.disposition),
        );
        let authority_decisive_prior = authority.last_decisive_transition.map_or_else(
            || "null".to_string(),
            |entry| format!("\"{:?}\"", entry.prior),
        );
        let authority_decisive_deadline = authority.last_decisive_transition.map_or_else(
            || "null".to_string(),
            |entry| json_metric_number_with_precision(entry.deadline_elapsed_seconds, 2),
        );
        let deferred_authority_actions = authority.deferred_authority_actions;
        let deferred_authority_packets = authority.deferred_authority_packets;
        let deferred_authority_packet_bytes = authority.deferred_authority_packet_bytes;
        let deferred_peak_events = authority.deferred_action_peak_events;
        let deferred_peak_packets = authority.deferred_action_peak_packets;
        let deferred_peak_bytes = authority.deferred_action_peak_bytes;
        let poll_peak_events = self.poll_peak_events;
        let poll_peak_deferred_events = self.poll_peak_deferred_events;
        let lvz_merge_total_ms =
            json_metric_number_with_precision(Some(self.lvz_merge_total_ms), 2);
        let lvz_merge_max_ms = json_metric_number_with_precision(Some(self.lvz_merge_max_ms), 2);
        let flag_claim_eligible_frames = self.flag_claim_eligible_frames;
        let flag_claim_attempts = self.flag_claim_attempts;
        let flag_claim_send_gated = self.flag_claim_send_gated;
        let flag_view = self
            .network_world
            .flags
            .values()
            .find(|flag| !flag.turf)
            .map(|flag| {
                format!(
                    "id:{},x:{},y:{},owner:{},dropped:{},carriedBy:{}",
                    flag.flag_id,
                    flag.position.x as i32,
                    flag.position.y as i32,
                    flag.owner,
                    flag.dropped,
                    flag.carried_by.map_or(-1i32, |carrier| carrier as i32)
                )
            })
            .unwrap_or_default();
        let chat_messages = format!(
            "[{}]",
            self.chat
                .messages
                .iter()
                .map(|message| json_metric_string(Some(message)))
                .collect::<Vec<_>>()
                .join(",")
        );
        let chat_colors = format!(
            "[{}]",
            self.chat
                .message_colors
                .iter()
                .map(|color| format!("\"{color:?}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        let menu_visible = self.menu_visible;
        let debug_mode = crate::browser_metrics::debug_mode();
        let owned_weapon_counts = self.owned_weapon_counts();
        let brick_tiles = active_brick_tiles_metrics_json(self);
        let brick_tiles_overflow = self
            .bricks
            .len()
            .saturating_sub(MAX_BRICK_TILE_METRIC_ENTRIES);
        let local_gameplay_state = format!(
            "{{\"menuVisible\":{menu_visible},\"debugMode\":{debug_mode},\"chatMessages\":{chat_messages},\"chatColors\":{chat_colors},\"objectiveRenderFrameObserved\":true,\"objectivePresentationAuthoritativeOnly\":{},\"objectivePresentedFlagCount\":{},\"objectivePresentedPowerballCount\":{},\"localFlagFallbackPresented\":{},\"localPowerballFallbackPresented\":{},\"networkedPlay\":{},\"localObjectiveFallbackAllowed\":{},\"authoritativeFlagCount\":{},\"authoritativePowerballCount\":{},\"localCarryingFlag\":{},\"localDroppedFlag\":{},\"localPowerballCarried\":{},\"activePrizeCount\":{},\"activePrizeCollectibleIds\":{},\"doorSeed\":{},\"doorSeedNext\":{},\"doorSeedAfterNext\":{},\"doorSeedThirdNext\":{},\"doorTimer\":{},\"doorClockFresh\":{},\"zoneFilesReady\":{},\"zoneMapReady\":{},\"networkPrizeBaselinePending\":{},\"gameplayStepReady\":{},\"gameplayScopeReady\":{},\"preStepNetworkRebaseSuppressed\":{},\"lastSelfServerPrizeLifeSeq\":{},\"lastSelfServerPrizeRevision\":{},\"lastSelfServerEnergy\":{},\"selfEnergyPredictionTolerance\":{},\"inputRingLen\":{},\"inputBits\":{},\"afterburnerActive\":{},\"movementThrustAcceleration\":{},\"movementTopSpeed\":{},\"baseThrustAcceleration\":{},\"baseTopSpeed\":{},\"afterburnerThrustAcceleration\":{},\"afterburnerTopSpeed\":{},\"authorityPhase\":{},\"authorityTransitionKind\":{},\"authorityApplicationBarrierKind\":{},\"authorityApplicationBarrierCount\":{},\"authorityApplicationBarrierDeliveryId\":{},\"authorityApplicationBarrierIdentity\":{},\"authorityApplicationBarrierSequence\":{},\"authorityTransitionLedgerEntries\":{},\"authorityLastTransitionSource\":{},\"authorityLastTransitionFact\":{},\"authorityLastTransitionPrior\":{},\"authorityLastTransitionDisposition\":{},\"authorityLastTransitionResulting\":{},\"authorityLastTransitionDeliveryId\":{},\"authorityLastTransitionApplicationIdentity\":{},\"authorityLastTransitionSequence\":{},\"authorityLastTransitionDeadline\":{},\"authoritySegment\":{},\"authorityScopeServerSessionId\":{},\"authorityScopeArenaStreamId\":{},\"authorityScopePlayerId\":{},\"authorityScopeLifeSeq\":{},\"authorityScopeAlive\":{},\"authorityReadinessJoined\":{},\"authorityReadinessArenaEpoch\":{},\"authorityReadinessSettings\":{},\"authorityReadinessSecurity\":{},\"authorityReadinessMap\":{},\"authorityReadinessFiles\":{},\"authorityInitialBaselineReadyToSubmit\":{},\"pendingShipResetPosition\":{},\"pendingShipResetDeliveryId\":{},\"pendingShipResetLifeSeq\":{},\"pendingShipResetSequence\":{},\"pendingFrequencyBaselineDeliveryId\":{},\"lifecycleBarrierElapsed\":{},\"lifecycleBaselineReceiptsSeen\":{},\"lifecycleBaselineReceiptsMatched\":{},\"lifecycleBaselineReceiptsUnmatched\":{},\"pendingRepelApplicationGeneration\":{},\"pendingRepelApplicationSnapshotId\":{},\"pendingRepelApplicationSequence\":{},\"consumedRepelDeliveryGenerations\":{},\"deferredWarpPickupAcks\":{},\"reliableNextProcessId\":{},\"reliableQueuedPackets\":{},\"pendingFileRequests\":{},\"pendingFileFrontIndex\":{},\"fileRequestInFlight\":{},\"fileRequestReissues\":{},\"fileRequestFramesSinceSend\":{},\"pendingZoneDownloads\":{},\"pendingZonePublication\":{},\"fileChunkIndexMismatches\":{},\"fileChunksAccepted\":{},\"fileChunkAssemblerRejects\":{},\"smallChunkBytes\":{},\"smallChunkFragments\":{},\"positionX\":{},\"positionY\":{},\"velocityX\":{},\"velocityY\":{},\"orientation\":{},\"energy\":{},\"rechargePerSecond\":{},\"bulletCooldown\":{},\"burstCooldown\":{},\"bombCooldown\":{},\"repelCooldown\":{},\"wormholeCooldown\":{},\"fakeAntiwarpTimer\":{},\"rocketTimer\":{},\"rocketCooldown\":{},\"superTimer\":{},\"shieldTimer\":{},\"engineShutdownTimer\":{},\"repelTimer\":{},\"repelSequenceStart\":{},\"repelSequenceEnd\":{},\"repels\":{},\"bursts\":{},\"bricks\":{},\"decoys\":{},\"thors\":{},\"rockets\":{},\"portals\":{},\"gunLevel\":{},\"bombLevel\":{},\"bullets\":{},\"bombs\":{},\"mines\":{},\"shrapnel\":{},\"thorsActive\":{},\"bricksActive\":{},\"brickTiles\":{brick_tiles},\"brickTilesOverflow\":{brick_tiles_overflow},\"decoysActive\":{},\"repelsActive\":{},\"warpsActive\":{},\"portalActive\":{},\"authorityDecisiveFact\":{},\"authorityDecisiveDisposition\":{},\"authorityDecisivePrior\":{},\"authorityDecisiveDeadline\":{},\"deferredAuthorityActions\":{},\"deferredAuthorityPackets\":{},\"deferredAuthorityPacketBytes\":{},\"deferredPeakEvents\":{},\"deferredPeakPackets\":{},\"deferredPeakBytes\":{},\"pollPeakEvents\":{},\"pollPeakDeferredEvents\":{},\"lvzMergeCount\":{},\"lvzMergeTotalMs\":{},\"lvzMergeMaxMs\":{},\"zoneLvzObjects\":{},\"zoneLvzTextures\":{},\"flagClaimEligibleFrames\":{},\"flagClaimAttempts\":{},\"flagClaimSendGated\":{},\"flagView\":\"{}\",\"reliableDuplicateDrops\":{},\"reliableQueueFullDrops\":{},\"acksEmitted\":{},\"acksEmittedMaxId\":{},\"stepAuthorityReady\":{},\"rebaseSuppressed\":{},\"rtcTransportStatus\":{},\"rtcTransportTerminal\":{},\"rtcRejoinCount\":{}}}",
            self.networked_play,
            self.network_world.flags.len(),
            self.network_world.powerballs.len(),
            local_flag_fallback_presented,
            local_powerball_fallback_presented,
            self.networked_play,
            !self.networked_play,
            self.network_world.flags.len(),
            self.network_world.powerballs.len(),
            self.carrying_flag,
            self.dropped_flag.is_some(),
            self.powerball.carried,
            self.prizes.len(),
            active_prize_collectible_ids,
            door_seed,
            door_seed_next,
            door_seed_after_next,
            door_seed_third_next,
            json_metric_number_with_precision(Some(self.door_timer), 4),
            self.door_clock_fresh,
            self.network_zone_files_ready,
            self.network_zone_map_ready,
            self.network_prize_baseline_pending,
            self.gameplay_step_ready(),
            self.client_authority.view().gameplay_scope().is_some()
                && !rtc_transport_terminal_active,
            self.pre_step_network_rebase_suppressed(),
            json_metric_u32(self.last_self_server_prize_life_seq),
            json_metric_u32(self.last_self_server_prize_revision),
            json_metric_number_with_precision(self.last_self_server_energy, 3),
            json_metric_number_with_precision(
                Some(crate::game::network::SELF_ENERGY_PREDICTION_TOLERANCE),
                3,
            ),
            self.outbound_input_ring.len(),
            self.last_input.bits(),
            self.last_afterburning,
            json_metric_number_with_precision(Some(self.last_movement_thrust_acceleration), 3),
            json_metric_number_with_precision(Some(self.last_movement_top_speed), 3),
            json_metric_number_with_precision(Some(self.effective_thrust_acceleration()), 3),
            json_metric_number_with_precision(Some(self.effective_top_speed()), 3),
            json_metric_number_with_precision(
                Some(self.ship_tuning().afterburner_thrust_acceleration),
                3,
            ),
            json_metric_number_with_precision(Some(self.ship_tuning().afterburner_top_speed), 3),
            authority_phase,
            authority_transition_kind,
            authority_application_barrier_kind,
            authority.application_barriers,
            json_metric_u32(authority.application_barrier_delivery_id),
            json_metric_u32(authority.application_barrier_identity),
            json_metric_u16(authority.application_barrier_sequence),
            authority.transition_ledger_entries,
            authority_last_transition_source,
            authority_last_transition_fact,
            authority_last_transition_prior,
            authority_last_transition_disposition,
            authority_last_transition_resulting,
            json_metric_u32(
                authority
                    .last_transition
                    .and_then(|entry| entry.delivery_id)
            ),
            json_metric_u32(
                authority
                    .last_transition
                    .and_then(|entry| entry.application_identity),
            ),
            json_metric_u16(
                authority
                    .last_transition
                    .and_then(|entry| entry.operation_sequence),
            ),
            json_metric_number_with_precision(
                authority
                    .last_transition
                    .and_then(|entry| entry.deadline_elapsed_seconds),
                4,
            ),
            authority.segment,
            json_metric_u64(authority.scope.server_session_id),
            json_metric_u64(authority.scope.arena_stream_id),
            json_metric_u16(authority.scope.player_id),
            json_metric_u32(authority.scope.life_seq),
            json_metric_bool(authority.scope.alive),
            authority.readiness.joined,
            json_metric_u32(authority.readiness.arena_epoch),
            authority.readiness.settings,
            authority.readiness.security,
            authority.readiness.map,
            authority.readiness.files,
            authority.initial_baseline_ready_to_submit,
            pending_initial_position
                || matches!(
                    authority.transition_kind,
                    Some(
                        ClientAuthorityTransitionKind::Ship
                            | ClientAuthorityTransitionKind::Respawn
                    )
                ),
            json_metric_u32(authority.initial_baseline_delivery_id.or_else(|| {
                matches!(
                    authority.transition_kind,
                    Some(
                        ClientAuthorityTransitionKind::Ship
                            | ClientAuthorityTransitionKind::Respawn
                    )
                )
                .then_some(authority.lifecycle_baseline_delivery_id)
                .flatten()
            }),),
            json_metric_u32(authority.initial_baseline_life_seq.or_else(|| {
                matches!(
                    authority.transition_kind,
                    Some(
                        ClientAuthorityTransitionKind::Ship
                            | ClientAuthorityTransitionKind::Respawn
                    )
                )
                .then_some(authority.lifecycle_baseline_life_seq)
                .flatten()
            }),),
            json_metric_u16(authority.initial_baseline_sequence.or_else(|| {
                matches!(
                    authority.transition_kind,
                    Some(
                        ClientAuthorityTransitionKind::Ship
                            | ClientAuthorityTransitionKind::Respawn
                    )
                )
                .then_some(authority.lifecycle_baseline_sequence)
                .flatten()
            }),),
            json_metric_u32(
                (authority.transition_kind == Some(ClientAuthorityTransitionKind::Frequency))
                    .then_some(authority.lifecycle_baseline_delivery_id)
                    .flatten(),
            ),
            json_metric_number_with_precision(authority.deadline_elapsed_seconds, 4),
            lifecycle_receipts_seen,
            lifecycle_receipts_matched,
            lifecycle_receipts_unmatched,
            json_metric_u32(
                self.pending_repel_application
                    .map(|marker| marker.delivery_generation),
            ),
            json_metric_u16(
                self.pending_repel_application
                    .map(|marker| marker.snapshot_id),
            ),
            json_metric_u16(
                self.pending_repel_application
                    .map(|marker| marker.application_sequence),
            ),
            self.consumed_repel_delivery_generations.len(),
            self.client_authority.view().deferred_warp_pickup_acks,
            reliable_next_process_id,
            reliable_queued_packets,
            pending_file_requests,
            json_metric_u16(pending_file_front_index),
            json_metric_u16(file_request_in_flight),
            json_metric_u16(Some(u16::from(file_request_reissues))),
            json_metric_u32(Some(file_request_frames_since_send)),
            pending_zone_downloads,
            pending_zone_publication,
            file_chunk_index_mismatches,
            file_chunks_accepted,
            file_chunk_assembler_rejects,
            small_chunk_bytes,
            small_chunk_fragments,
            json_metric_number_with_precision(Some(self.ship.pos.x), 3),
            json_metric_number_with_precision(Some(self.ship.pos.y), 3),
            json_metric_number_with_precision(Some(self.ship.vel.x), 3),
            json_metric_number_with_precision(Some(self.ship.vel.y), 3),
            json_metric_number_with_precision(Some(self.ship.orientation), 5),
            json_metric_number_with_precision(Some(self.ship.energy), 3),
            json_metric_number_with_precision(Some(self.ship.recharge_per_second), 3),
            json_metric_number_with_precision(Some(self.ship.bullet_cooldown), 4),
            json_metric_number_with_precision(Some(self.ship.burst_cooldown), 4),
            json_metric_number_with_precision(Some(self.ship.bomb_cooldown), 4),
            json_metric_number_with_precision(Some(self.ship.repel_cooldown), 4),
            json_metric_number_with_precision(Some(self.ship.wormhole_cooldown), 4),
            json_metric_number_with_precision(Some(self.ship.fake_antiwarp_timer), 4),
            json_metric_number_with_precision(Some(self.ship.rocket_timer), 4),
            json_metric_number_with_precision(Some(self.ship.rocket_cooldown), 4),
            json_metric_number_with_precision(Some(self.ship.super_timer), 4),
            json_metric_number_with_precision(Some(self.ship.shield_timer), 4),
            json_metric_number_with_precision(Some(self.ship.engine_shutdown_timer), 4),
            json_metric_number_with_precision(Some(self.ship.repel_timer), 4),
            json_metric_u16(self.repel_movement_sequence_window.map(|window| window.0)),
            json_metric_u16(self.repel_movement_sequence_window.map(|window| window.1)),
            self.ship.repels,
            self.ship.bursts,
            self.ship.bricks,
            self.ship.decoys,
            self.ship.thors,
            self.ship.rockets,
            self.ship.portals,
            self.ship.gun_level,
            self.ship.bomb_level,
            owned_weapon_counts[0],
            owned_weapon_counts[1],
            owned_weapon_counts[2],
            owned_weapon_counts[3],
            owned_weapon_counts[4],
            self.bricks.len(),
            self.decoys.len(),
            self.repels.len(),
            self.warps.len(),
            self.active_portal().is_some(),
            authority_decisive_fact,
            authority_decisive_disposition,
            authority_decisive_prior,
            authority_decisive_deadline,
            deferred_authority_actions,
            deferred_authority_packets,
            deferred_authority_packet_bytes,
            deferred_peak_events,
            deferred_peak_packets,
            deferred_peak_bytes,
            poll_peak_events,
            poll_peak_deferred_events,
            self.lvz_merge_count,
            lvz_merge_total_ms,
            lvz_merge_max_ms,
            self.lvz_render_cache.objects.len(),
            self.lvz_render_cache.textures.len(),
            flag_claim_eligible_frames,
            flag_claim_attempts,
            flag_claim_send_gated,
            flag_view,
            reliable_duplicate_drops,
            reliable_queue_full_drops,
            acks_emitted,
            acks_emitted_max_id,
            step_authority_ready,
            rebase_suppressed,
            rtc_transport_status,
            rtc_transport_terminal,
            rtc_rejoin_count,
        );
        let quality_remaining_seconds = self
            .client_authority
            .quality_hold_remaining(macroquad::miniquad::date::now());
        let quality_remaining = quality_remaining_seconds.map_or_else(
            || "null".to_owned(),
            |remaining| format!("{:.0}", (remaining * 1000.0).ceil()),
        );
        let quality_countdown =
            json_metric_string(connection_cooldown_text(quality_remaining_seconds).as_deref());
        #[cfg(any(test, target_arch = "wasm32"))]
        let quality_recovery =
            json_metric_string(self.quality_return_recovery.as_ref().map(|attempt| {
                if attempt.is_stopped() {
                    "stopped"
                } else if attempt.is_ready() {
                    "ready"
                } else {
                    "recovering"
                }
            }));
        #[cfg(not(any(test, target_arch = "wasm32")))]
        let quality_recovery = "null";
        #[cfg(any(test, target_arch = "wasm32"))]
        let quality_recovery_ship = self
            .quality_return_recovery
            .as_ref()
            .map_or_else(|| "null".to_string(), |attempt| attempt.ship.to_string());
        #[cfg(not(any(test, target_arch = "wasm32")))]
        let quality_recovery_ship = "null";
        let local_gameplay_state = append_metric_fields(
            &local_gameplay_state,
            &format!(
                ",\"ownedProjectileReconciliations\":{}",
                self.owned_projectile_reconciliations
            ),
        );
        let local_gameplay_state = append_metric_fields(
            &local_gameplay_state,
            &format!(
                ",\"abilities\":{{\"multifireCapable\":{},\"multifire\":{},\"stealthCapable\":{},\"stealth\":{},\"cloakCapable\":{},\"cloak\":{},\"xradarCapable\":{},\"xradar\":{},\"antiwarpCapable\":{},\"antiwarp\":{}}}",
                self.ship.multifire_capable,
                self.ship.multifire,
                self.ship.stealth_capable,
                self.ship.stealth,
                self.ship.cloak_capable,
                self.ship.cloak,
                self.ship.xradar_capable,
                self.ship.xradar,
                self.ship.antiwarp_capable,
                self.ship.antiwarp,
            ),
        );
        let security_generation_matches_authority_scope =
            self.network_security_generation.is_some_and(|generation| {
                Some(generation.server_session_id) == authority.scope.server_session_id
                    && Some(generation.arena_epoch) == authority.readiness.arena_epoch
            });
        let current_security_generation = self
            .network_security_generation
            .filter(|_| security_generation_matches_authority_scope);
        let security_challenge_generation_matches_current = current_security_generation
            .is_some_and(|generation| {
                self.network_security_challenge_generation == Some(generation)
            });
        let local_gameplay_state = append_metric_fields(
            &local_gameplay_state,
            &format!(
                ",\"networkSecurityGeneration\":{},\"networkSecurityChallengeGeneration\":{},\"networkSecurityChallengePresent\":{},\"networkSecurityGenerationMatchesAuthorityScope\":{},\"networkSecurityChallengeGenerationMatchesCurrent\":{}",
                json_network_security_generation(self.network_security_generation),
                json_network_security_generation(self.network_security_challenge_generation),
                self.network_security_challenge.is_some(),
                security_generation_matches_authority_scope,
                security_challenge_generation_matches_current,
            ),
        );
        // What a viewer is actually shown for a carried flag. The flag sprite is attached to the carrier's
        // interpolated ship position (`src/game/render/ships/network.rs`) and `draw_network_flags` skips
        // carried flags, so the stamped `flagView` position is not what a peer sees while the flag rides.
        let presented_carrier = self
            .network_world
            .players
            .iter()
            .find(|(_, player)| player.flags > 0)
            .and_then(|(player_id, player)| {
                player
                    .display_position()
                    .map(|position| (*player_id, position))
            });
        let local_gameplay_state = append_metric_fields(
            &local_gameplay_state,
            &format!(
                ",\"spectatorMode\":{},\"spectatingPlayerId\":{},\"observerReady\":{},\"observerTargetIds\":{},\"connectionQualityHoldRemainingMs\":{quality_remaining},\"connectionQualityCountdown\":{quality_countdown},\"connectionQualityRecovery\":{quality_recovery},\"connectionQualityRecoveryShip\":{quality_recovery_ship}",
                self.spectator_mode,
                json_metric_u16(self.spectating_player_id),
                self.client_authority.view().observer_scope().is_some(),
                format!(
                    "[{}]",
                    self.network_world
                        .players
                        .values()
                        .filter(|p| p.ship < 8
                            && Some(p.player_id) != self.client_authority.view().scope.player_id)
                        .map(|p| p.player_id.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            ),
        );
        let network_indicator = self.network_notice_text();
        let local_gameplay_state = append_metric_fields(
            &local_gameplay_state,
            &format!(
                ",\"networkNotice\":{},\"networkIndicator\":{}",
                json_metric_string(
                    self.network_notice
                        .as_ref()
                        .map(|notice| notice.message.as_str())
                ),
                json_metric_string(
                    network_indicator
                        .as_ref()
                        .map(|(message, _)| message.as_str())
                ),
            ),
        );
        let local_gameplay_state = append_metric_fields(
            &local_gameplay_state,
            &format!(
                ",\"presentedCarrierId\":{},\"presentedCarrierX\":{},\"presentedCarrierY\":{}",
                json_metric_u16(presented_carrier.map(|(player_id, _)| player_id)),
                json_metric_number_with_precision(
                    presented_carrier.map(|(_, position)| position.x),
                    2,
                ),
                json_metric_number_with_precision(
                    presented_carrier.map(|(_, position)| position.y),
                    2,
                ),
            ),
        );
        let local_gameplay_state = append_metric_fields(
            &local_gameplay_state,
            &format!(
                ",\"repelDeliveryFramesSeen\":{},\"repelDeliveryFramesApplied\":{},\"repelApplicationPositionsSent\":{},\"repelDeliveryLifeRejects\":{},\"repelDeliveryTimingRejects\":{},\"repelDeliveryApplicationRejects\":{},\"repelDeliveryExpiredApplicationCancellations\":{},\"repelDeliveryExpiredReceiptsRecovered\":{},\"expiredRepelApplications\":{},\"repelDeliveryPendingRetries\":{},\"repelDeliveryConsumedRetries\":{},\"repelDeliveryLastSnapshotTick\":{},\"repelDeliveryLastExpiryTick\":{},\"repelDeliveryLastObservedTick\":{},\"repelDeliveryLastOfferedSteps\":{},\"repelDeliveryLastAcceptedEndSequence\":{},\"repelDeliveryLastAcceptedSteps\":{},\"pendingMovementCorrectionGeneration\":{},\"pendingMovementCorrectionDetectedSequence\":{},\"pendingMovementCorrectionApplicationSequence\":{},\"pendingMovementCorrectionDeliveryId\":{},\"pendingMovementCorrectionActions\":{},\"movementCorrectionActionsDropped\":{},\"movementCorrectionTerminalOutcomes\":{},\"movementCorrectionReceiptsSeen\":{},\"movementCorrectionReceiptsMatched\":{},\"movementCorrectionReceiptsUnmatched\":{},\"pendingCombatViewActions\":{},\"combatViewActionsDropped\":{},\"pendingPrizeEffectApplications\":{},\"pendingPrizeEffectApplicationSequence\":{},\"pendingPortalPlacements\":{},\"pendingPortalWarp\":{},\"pendingPortalWarpActionId\":{},\"portalX\":{},\"portalY\":{},\"portalTtl\":{},\"portalAge\":{},\"decoyOrdinaryShipPresentationCount\":{},\"decoyNameLabelPresentationCount\":{},\"decoyRadarPresentationCount\":{},\"prizeEffectApplicationReceiptsSeen\":{},\"prizeEffectApplicationReceiptsMatched\":{},\"prizeEffectApplicationReceiptsUnmatched\":{},\"prizeAuthorityLifeSeq\":{},\"prizeAuthorityRevision\":{},\"damagePrizeLossSeq\":{},\"damagePrizeLossId\":{},\"damagePrizeNotificationCount\":{},\"lastDamagePrizeNotification\":{},\"prizePickupNotificationCount\":{},\"lastPrizePickupNotification\":{},\"prizePickupSoundPlayCount\":{}",
                self.repel_delivery_frames_seen,
                self.repel_delivery_frames_applied,
                self.repel_application_positions_sent,
                self.repel_delivery_life_rejects,
                self.repel_delivery_timing_rejects,
                self.repel_delivery_application_rejects,
                self.repel_delivery_expired_application_cancellations,
                self.repel_delivery_expired_receipts_recovered,
                self.expired_repel_applications.len(),
                self.repel_delivery_pending_retries,
                self.repel_delivery_consumed_retries,
                json_metric_u32(self.repel_delivery_last_snapshot_tick),
                json_metric_u32(self.repel_delivery_last_expiry_tick),
                json_metric_u32(self.repel_delivery_last_observed_tick),
                json_metric_u16(self.repel_delivery_last_offered_steps),
                json_metric_u16(self.repel_delivery_last_accepted_end_sequence),
                json_metric_u16(self.repel_delivery_last_accepted_steps),
                json_metric_u32(
                    authority
                        .correction_application
                        .map(|application| application.correction_generation),
                ),
                json_metric_u16(
                    authority
                        .correction_application
                        .map(|application| application.detected_sequence),
                ),
                json_metric_u16(
                    authority
                        .correction_application
                        .map(|application| application.application_sequence),
                ),
                json_metric_u32(authority.correction_delivery_id),
                authority.correction_retained_actions,
                authority.correction_actions_dropped,
                authority.correction_terminal_outcomes,
                authority.correction_receipts_seen,
                authority.correction_receipts_matched,
                authority.correction_receipts_unmatched,
                authority.combat_view_retained_actions,
                authority.combat_view_actions_dropped,
                authority.prize_effect_application_barriers,
                json_metric_u16(
                    self.client_authority
                        .pending_prize_effect_application()
                        .map(|pending| pending.application.application_sequence),
                ),
                self.pending_portal_placements.len(),
                authority.portal_warp_barrier,
                json_metric_u32(
                    self.client_authority
                        .pending_portal_warp_request()
                        .map(|request| request.action_id),
                ),
                json_metric_number_with_precision(
                    self.active_portal().map(|portal| portal.pos.x),
                    3
                ),
                json_metric_number_with_precision(
                    self.active_portal().map(|portal| portal.pos.y),
                    3
                ),
                json_metric_number_with_precision(self.active_portal_ttl(), 4),
                json_metric_number_with_precision(self.active_portal().map(|portal| portal.age), 4),
                self.decoy_ordinary_ship_presentations.get(),
                self.decoy_name_label_presentations.get(),
                self.decoy_radar_presentations.get(),
                authority.prize_effect_application_receipts_seen,
                authority.prize_effect_application_receipts_matched,
                authority.prize_effect_application_receipts_unmatched,
                json_metric_u32(self.last_self_server_prize_life_seq),
                json_metric_u32(self.last_self_server_prize_revision),
                json_metric_u32(self.last_self_server_damage_prize_loss_seq),
                self.last_self_server_damage_prize_loss_id
                    .map_or_else(|| "null".to_string(), |id| id.to_string(),),
                self.damage_prize_notification_count,
                json_metric_string(self.last_damage_prize_notification.as_deref()),
                self.prize_pickup_notification_count,
                json_metric_string(self.last_prize_pickup_notification.as_deref()),
                self.prize_pickup_sound_play_count,
            ),
        );
        let flag_last_authoritative = format!(
            "v={} state={} x={} y={} tick={}",
            self.network_world.flag_last_authoritative_version,
            self.network_world.flag_last_authoritative_state,
            self.network_world.flag_last_authoritative_x,
            self.network_world.flag_last_authoritative_y,
            self.network_world.flag_last_authoritative_tick,
        );
        let local_gameplay_state = append_metric_fields(
            &local_gameplay_state,
            &format!(
                ",\"flagLegacyPositionEvents\":{},\"flagLegacyPositionGuardRejections\":{},\
                 \"flagAuthoritativeStateEvents\":{},\"flagAuthoritativeCarriedEvents\":{},\
                 \"flagVersionResets\":{},\"flagAuthoritativeStaleRejects\":{},\
                 \"flagLastAuthoritativeState\":{:?}",
                self.network_world.flag_legacy_position_events,
                self.network_world.flag_legacy_position_guard_rejections,
                self.network_world.flag_authoritative_state_events,
                self.network_world.flag_authoritative_carried_events,
                self.network_world.flag_version_resets,
                self.network_world.flag_authoritative_stale_rejects,
                flag_last_authoritative,
            ),
        );
        let presentability = self.network_world.snapshot_presentability();
        let remote_weapon_suppression_reasons =
            remote_weapon_presentation_suppression_metrics_json(self);
        let snapshot_receipt_tiers = format!(
            "{{\"protocolConsumed\":{{\"snapshotId\":{},\"serverTick\":{}}},\"codecAckQueued\":{{\"snapshotId\":{},\"serverTick\":{}}},\"gameApplied\":{{\"snapshotId\":{},\"serverTick\":{}}},\"presentability\":{{\"snapshotId\":{},\"remoteActorCount\":{},\"presentableRemoteActorCount\":{},\"unresolvedLifeCount\":{},\"missingPositionCount\":{}}},\"actorDependency\":{{\"pending\":{},\"queuedTotal\":{},\"releasedTotal\":{},\"droppedTotal\":{},\"supersededTotal\":{},\"capacityTotal\":{},\"expiredTotal\":{},\"cancelledTotal\":{},\"invalidTotal\":{},\"suppressionReasons\":{}}},\"actualDrawEvidence\":\"remoteSpriteTrace\"}}",
            latest_snapshot_id,
            latest_snapshot_server_tick,
            last_acked_snapshot_id,
            last_acked_snapshot_server_tick,
            json_metric_u16(self.network_world.authoritative_snapshot_id),
            json_metric_u32(self.network_world.authoritative_snapshot_server_tick),
            if presentability.snapshot_is_presentable() {
                json_metric_u16(presentability.snapshot_id)
            } else {
                "null".to_owned()
            },
            presentability.remote_actor_count,
            presentability.presentable_remote_actor_count,
            presentability.unresolved_life_count,
            presentability.missing_position_count,
            self.remote_weapon_presentations.len(),
            self.remote_weapon_presentations.queued_total,
            self.remote_weapon_presentations.released_total,
            self.remote_weapon_presentations.dropped_total(),
            self.remote_weapon_presentations.superseded_total(),
            self.remote_weapon_presentations
                .terminal_for(RemoteWeaponPresentationSuppressionReason::CapacityRejected,),
            self.remote_weapon_presentations
                .terminal_for(RemoteWeaponPresentationSuppressionReason::EventWindowExpired,),
            self.remote_weapon_presentations
                .terminal_for(RemoteWeaponPresentationSuppressionReason::ScopeCancelled,),
            self.remote_weapon_presentations
                .terminal_for(RemoteWeaponPresentationSuppressionReason::InvalidIdentity,),
            remote_weapon_suppression_reasons,
        );
        let combat_view_server_tick = self.browser_server_tick();
        let combat_view_directive = self.observer_combat_timeline.active_directive();
        let combat_view_scheduled_render_tick = combat_view_server_tick
            .and_then(|server_tick| self.observer_combat_timeline.render_tick_at(server_tick));
        let combat_view_hold_or_stall = combat_view_server_tick
            .is_none_or(|server_tick| self.observer_combat_timeline.hold_or_stall_at(server_tick));
        let combat_view_weapon_ready = combat_view_server_tick.is_some_and(|server_tick| {
            !self
                .observer_combat_timeline
                .input_hold_or_stall_at(server_tick)
        });
        let combat_view_applied_snapshot =
            self.observer_combat_timeline.latest_game_applied_snapshot();
        let weapon_view = self.remote_weapon_view_history.stats();
        let combat_view_timeline = format!(
            "{{\"generation\":{},\"serverTick\":{},\"delayTicks\":{},\"targetDelayTicks\":{},\"scheduledRenderTick\":{},\"presentationRenderTick\":{},\"gameAppliedSnapshotId\":{},\"gameAppliedServerTick\":{},\"holdOrStall\":{},\"weaponReady\":{},\"weaponView\":{{\"waitingFrames\":{},\"historicalFrames\":{},\"currentFrames\":{},\"missingHistoryFrames\":{},\"effectDrops\":{}}}}}",
            json_metric_u32(combat_view_directive.map(|directive| directive.generation)),
            json_metric_u32(combat_view_server_tick),
            json_metric_u16(
                combat_view_server_tick
                    .and_then(|tick| self.observer_combat_timeline.delay_ticks_at(tick))
                    .map(u16::from)
            ),
            json_metric_u16(
                combat_view_directive.map(|directive| u16::from(directive.target_delay_ticks))
            ),
            json_metric_u32(combat_view_scheduled_render_tick),
            json_metric_u32(self.observer_combat_timeline.presentation_render_tick()),
            json_metric_u16(combat_view_applied_snapshot.map(|(snapshot_id, _)| snapshot_id)),
            json_metric_u32(combat_view_applied_snapshot.map(|(_, server_tick)| server_tick)),
            combat_view_hold_or_stall,
            combat_view_weapon_ready,
            weapon_view.waiting_frames,
            weapon_view.historical_frames,
            weapon_view.current_frames,
            weapon_view.missing_history_frames,
            self.remote_weapon_view_effect_drops,
        );

        let remote_ship_explosion_witnesses = format!(
            "[{}]",
            self.remote_ship_explosion_witnesses
                .iter()
                .map(|witness| format!(
                    "{{\"ordinal\":{},\"authoritySegment\":\"{}\",\"playerId\":{},\"nextLifeSeq\":{},\"respawnAtTick\":{},\"greenCollectibleId\":{},\"x\":{},\"y\":{}}}",
                    witness.ordinal,
                    witness.death.authority_segment,
                    witness.death.killed_id,
                    witness.death.life_seq,
                    witness.death.respawn_at_tick,
                    witness.death.green_collectible_id,
                    json_metric_number_with_precision(Some(witness.position.x), 3),
                    json_metric_number_with_precision(Some(witness.position.y), 3),
                ))
                .collect::<Vec<_>>()
                .join(",")
        );

        let payload = format!(
            "{{\"localRenderPrediction\":{local_render_prediction},\"mapChecksum\":{},\"arenaSettingsChecksum\":{},\"screenWidth\":{:.0},\"screenHeight\":{:.0},\"energyInside\":{},\"energyCenterDelta\":{:.2},\"energyDigitsInside\":{},\"energyDigitsRightAligned\":{},\"hudTextInside\":{},\"hudRadarOverlap\":{},\"radarX\":{:.2},\"radarY\":{:.2},\"radarBottomRight\":{},\"radarLabelInside\":{},\"radarPlayerIds\":{},\"rightStatusPanelInside\":{},\"rightStatusPanelClearOfRadar\":{},\"leftItemPanelInside\":{},\"hudLineCount\":{},\"hudTextScale\":{:.2},\"shipLabelInside\":{},\"shipLabelScale\":{:.2},\"shipLabelWidth\":{:.2},\"notificationMessages\":{notification_messages},\"notificationCount\":{},\"notificationInside\":{},\"notificationScale\":{:.2},\"notificationWidth\":{:.2},\"textPositiveScale\":{},\"selfPlayerId\":{},\"playerShip\":{player_ship},\"selfRosterShip\":{self_roster_ship},\"browserNetworkOwned\":{},\"chatActive\":{},\"scoreboardVisible\":{},\"scoreboardScrollRow\":{},\"scoreboardMaxScrollRow\":{},\"scoreboardPlayerIds\":{},\"scoreboardPlayers\":{scoreboard_player_stats},\"scoreboardView\":{scoreboard_view},\"playerFrequency\":{},\"playerLifeSeq\":{},\"selfWins\":{},\"selfLosses\":{},\"clientInputSeq\":{},\"latestServerTick\":{},\"latestSnapshotId\":{},\"authoritativeSnapshotId\":{},\"authoritativeSnapshotActors\":{},\"authoritativeSnapshotActorTrace\":{},\"authoritativeSnapshotActorStream\":{},\"remoteActorPresentation\":{},\"latestSnapshotServerTick\":{},\"latestSnapshotBaseId\":{},\"lastAckedSnapshotId\":{},\"lastAckedSnapshotServerTick\":{},\"selfCorrectionCount\":{},\"lastSelfCorrectionReason\":{},\"remoteSnapCount\":{},\"positionsSent\":{},\"positionsSentPerSecond\":{:.2},\"packetsSent\":{},\"packetsReceived\":{},\"gameplayStepReady\":{},\"remoteListingNearFrames\":{},\"remoteListingMidFrames\":{},\"remoteListingAbsentFrames\":{},\"remoteLastListingTier\":{},\"lastRemoteSnapReason\":{},\"remoteExtrapolatedFrames\":{},\"remoteFrequency\":{},\"lastNetworkDamageSeq\":{},\"lastNetworkDamageServerSeq\":{},\"lastNetworkDamageServerTick\":{},\"lastNetworkDamageVictimLifeSeq\":{},\"lastNetworkDamageShooterLifeSeq\":{},\"lastNetworkDamagePlayerId\":{},\"lastNetworkDamageAttackerId\":{},\"lastNetworkDamageEnergyBefore\":{},\"lastNetworkDamageEnergy\":{},\"lastNetworkDamageAmount\":{},\"lastNetworkDamageWeapon\":{},\"lastNetworkDamageForSelf\":{},\"lastSelfDamageSeq\":{},\"lastSelfDamageServerSeq\":{},\"lastSelfDamageServerTick\":{},\"lastSelfDamageVictimLifeSeq\":{},\"lastSelfDamageShooterLifeSeq\":{},\"lastSelfDamageAttackerId\":{},\"lastSelfDamageEnergyBefore\":{},\"lastSelfDamageEnergy\":{},\"lastSelfDamageAmount\":{},\"lastSelfDamageWeapon\":{},\"inSafeZone\":{},\"bombCooldown\":{},\"pendingWeapon\":{},\"playerEnergy\":{},\"playerPredictedEnergy\":{},\"localShipLabelEnergy\":{},\"visibleEnergyDigits\":{},\"playerMaxEnergy\":{},\"serverPlayerEnergy\":{},\"playerAlive\":{},\"serverPlayerAlive\":{},\"playerRespawnTimer\":{},\"playerRespawnSeq\":{},\"playerX\":{},\"playerY\":{},\"playerVelocityX\":{},\"playerVelocityY\":{},\"playerOrientation\":{},\"playerProtocolDirection\":{},\"remoteVisiblePlayers\":{},\"remoteDisplayX\":{},\"remoteDisplayY\":{},\"remotePositionX\":{},\"remotePositionY\":{},\"remoteDisplayOrientation\":{},\"remoteEnergy\":{},\"remotePlayers\":{},\"localBotCount\":{},\"remoteProjectileCount\":{},\"remoteBombCount\":{},\"remoteBulletCount\":{},\"remoteMineCount\":{},\"pendingRemoteWeaponPresentations\":{},\"pendingRemoteWeaponPresentationDrops\":{},\"remoteProjectileSources\":{},\"remoteProjectileSourcesOverflow\":{},\"presentedRemoteProjectileSources\":{},\"presentedRemoteProjectileSourcesOverflow\":{},\"remoteDecoyCount\":{},\"remoteDecoys\":{},\"remoteDecoysOverflow\":{},\"remoteProjectileSpawns\":{},\"remoteBombSpawns\":{},\"remoteBulletSpawns\":{},\"remoteMineSpawns\":{},\"remoteRepelSpawns\":{},\"localBombCount\":{},\"semanticInputCounts\":{},\"playerRockets\":{},\"playerRocketTimer\":{},\"lastSelfServerRocketLifeSeq\":{},\"lastSelfServerRocketActivationSequence\":{},\"lastSelfServerRocketUntilTick\":{},\"pendingRocketActivationSequence\":{},\"dynamicLvzChatSoundSources\":{},\"dynamicLvzChatSoundsLoaded\":{},\"localGameplayState\":{},\"combatKillNotificationCount\":{},\"lastCombatKillNotification\":{},\"remoteShipExplosionCount\":{},\"remoteShipExplosionWitnesses\":{remote_ship_explosion_witnesses},\"lastRemoteShipExplosionX\":{},\"lastRemoteShipExplosionY\":{},\"afterburnerSoundPlayCount\":{},\"snapshotReceiptTiers\":{},\"combatViewTimeline\":{}}}",
            map_checksum,
            arena_settings_checksum,
            screen_width,
            screen_height,
            energy_inside,
            energy_center_delta,
            energy_digits_inside,
            energy_digits_right_aligned,
            hud_text_inside,
            hud_radar_overlap,
            radar_x,
            radar_y,
            radar_bottom_right,
            radar_label_inside,
            radar_player_ids,
            right_status_panel_inside,
            right_status_panel_clear_of_radar,
            left_item_panel_inside,
            hud_lines.len(),
            HUD_TEXT_SCALE,
            label_inside,
            label_scale,
            label_width,
            self.notifications.len(),
            notification_inside,
            notification_scale,
            notification_width,
            text_positive_scale,
            self_player_id,
            browser_network_owned,
            self.chat.active,
            self.scoreboard_visible,
            self.scoreboard_scroll.offset,
            self.player_list_max_scroll(screen_width, screen_height),
            scoreboard_player_ids,
            player_frequency,
            player_life_seq,
            self_wins,
            self_losses,
            client_input_seq,
            latest_server_tick,
            latest_snapshot_id,
            json_metric_u16(self.network_world.authoritative_snapshot_id),
            authoritative_snapshot_actors,
            authoritative_snapshot_actor_trace,
            authoritative_snapshot_actor_stream,
            remote_actor_presentation,
            latest_snapshot_server_tick,
            latest_snapshot_base_id,
            last_acked_snapshot_id,
            last_acked_snapshot_server_tick,
            self.self_correction_count,
            self.last_self_correction_reason,
            self.network_world.stats.remote_snap_count,
            positions_sent,
            positions_sent_per_second,
            packets_sent,
            packets_received,
            u8::from(gameplay_step_ready),
            self.network_world.stats.remote_listing_near_frames,
            self.network_world.stats.remote_listing_mid_frames,
            self.network_world.stats.remote_listing_absent_frames,
            self.network_world.stats.remote_last_listing_tier,
            self.network_world.stats.last_remote_snap_reason,
            self.network_world.stats.remote_extrapolated_frames,
            remote_frequency,
            self.network_damage_event_seq,
            last_network_damage_server_seq,
            last_network_damage_server_tick,
            last_network_damage_victim_life_seq,
            last_network_damage_shooter_life_seq,
            last_network_damage_player_id,
            last_network_damage_attacker_id,
            last_network_damage_energy_before,
            last_network_damage_energy,
            last_network_damage_amount,
            last_network_damage_weapon,
            self.last_network_damage_for_self,
            self.self_damage_event_seq,
            last_self_damage_server_seq,
            last_self_damage_server_tick,
            last_self_damage_victim_life_seq,
            last_self_damage_shooter_life_seq,
            last_self_damage_attacker_id,
            last_self_damage_energy_before,
            last_self_damage_energy,
            last_self_damage_amount,
            last_self_damage_weapon,
            in_safe_zone,
            bomb_cooldown,
            pending_weapon,
            player_energy,
            player_predicted_energy,
            local_ship_label_energy,
            visible_energy_digits,
            player_max_energy,
            server_player_energy,
            player_alive,
            server_player_alive,
            player_respawn_timer,
            player_respawn_seq,
            player_x,
            player_y,
            player_velocity_x,
            player_velocity_y,
            player_orientation,
            player_protocol_direction,
            remote_visible_players,
            remote_display_x,
            remote_display_y,
            remote_position_x,
            remote_position_y,
            remote_display_orientation,
            remote_energy,
            remote_players,
            self.bots.len(),
            remote_projectile_count,
            remote_bomb_count,
            remote_bullet_count,
            remote_mine_count,
            self.remote_weapon_presentations.len(),
            self.remote_weapon_presentations.dropped_total(),
            remote_projectile_sources,
            remote_projectile_source_metric_overflow,
            presented_remote_projectile_sources,
            presented_remote_projectile_source_overflow,
            remote_decoy_count,
            remote_decoys,
            remote_decoy_metric_overflow,
            self.remote_projectile_spawns,
            self.remote_bomb_spawns,
            self.remote_bullet_spawns,
            self.remote_mine_spawns,
            self.remote_repel_spawns,
            owned_weapon_counts[1],
            semantic_input_counts,
            self.ship.rockets,
            json_metric_number(Some(self.ship.rocket_timer.max(0.0))),
            json_metric_u32(self.last_self_server_rocket_life_seq),
            json_metric_u16(self.last_self_server_rocket_activation_sequence),
            json_metric_u32(self.last_self_server_rocket_until_tick),
            json_metric_u16(self.pending_rocket_activation_sequence),
            self.dynamic_lvz_chat_sound_sources.len(),
            self.dynamic_lvz_chat_sound_loaded_ids.len(),
            local_gameplay_state,
            self.combat_kill_notification_count,
            json_metric_string(self.last_combat_kill_notification.as_deref()),
            self.remote_ship_explosion_count,
            json_metric_number_with_precision(
                self.last_remote_ship_explosion_position
                    .map(|position| position.x),
                3,
            ),
            json_metric_number_with_precision(
                self.last_remote_ship_explosion_position
                    .map(|position| position.y),
                3,
            ),
            self.afterburner_sound_play_count,
            snapshot_receipt_tiers,
            combat_view_timeline,
        );
        payload
    }

    fn measure_hud_metrics(
        &self,
        hud_lines: &[HudTextLine],
        radar_x: f32,
        radar_y: f32,
        radar_dim: f32,
        energy_hud_x: f32,
    ) -> HudMetricsMeasure {
        let max_hud_width = hud_lines
            .iter()
            .map(|line| {
                self.assets
                    .tall_font
                    .measure_width_scaled(&line.text, HUD_TEXT_SCALE)
            })
            .fold(0.0_f32, f32::max);
        let hud_right = HUD_TEXT_X + max_hud_width;
        let hud_bottom = HUD_TEXT_Y
            + hud_lines.len().saturating_sub(1) as f32 * HUD_LINE_SPACING
            + self.assets.tall_font.line_height_scaled(HUD_TEXT_SCALE);
        let radar_right = radar_x + radar_dim;
        let radar_bottom = radar_y + radar_dim;
        let hud_overlaps_radar_y = HUD_TEXT_Y < radar_bottom && hud_bottom > radar_y;
        let hud_radar_overlap = hud_overlaps_radar_y && hud_right > radar_x - HUD_RADAR_MARGIN;
        let radar_bottom_right = (screen_width() - radar_right - 6.0).abs() <= 1.0
            && (screen_height() - radar_bottom - 6.0).abs() <= 1.0;
        let radar_label = radar_time_sector_label(local_minutes_since_midnight(), self.ship.pos);
        let radar_label_scale = 1.0;
        let radar_label_width = self
            .assets
            .tall_font
            .measure_width_scaled(&radar_label, radar_label_scale);
        let radar_label_y = radar_y - 16.0;
        let radar_label_inside = screen_width() - 5.0 - radar_label_width >= 0.0
            && radar_label_y >= 0.0
            && radar_label_y + self.assets.tall_font.line_height_scaled(radar_label_scale)
                <= radar_y;
        let (right_status_x, right_status_y, right_status_width, right_status_height) =
            right_status_panel_bounds(
                screen_width(),
                screen_height(),
                self.assets.icons.frame_size.x,
                self.assets.icons.frame_size.y,
            );
        let right_status_panel_inside = right_status_x >= 0.0
            && right_status_x + right_status_width <= screen_width()
            && right_status_y >= 0.0
            && right_status_y + right_status_height <= screen_height();
        let right_status_panel_clear_of_radar =
            right_status_y + right_status_height < radar_y || right_status_x > radar_right;
        let (left_item_x, left_item_y, left_item_width, left_item_height) = left_item_panel_bounds(
            screen_height(),
            self.assets.icons.frame_size.x,
            self.assets.icon_counts.frame_size.x,
            self.assets.icons.frame_size.y,
        );
        let left_item_panel_inside = left_item_x >= 0.0
            && left_item_x + left_item_width <= screen_width()
            && left_item_y >= 0.0
            && left_item_y + left_item_height <= screen_height();
        let label = ship_label(
            self.local_player_name(),
            self.player_bounty,
            self.carrying_flag,
            self.powerball.carried,
        );
        let label_scale = SHIP_LABEL_FONT_SIZE as f32 / self.assets.tall_font.line_height();
        let label_pos = self
            .camera
            .world_to_screen(self.ship.pos + vec2(0.0, -34.0));
        let label_width = self
            .assets
            .tall_font
            .measure_width_scaled(&label, label_scale);
        let label_height = self.assets.tall_font.line_height_scaled(label_scale);
        let label_inside = label_pos.x - label_width * 0.5 >= 0.0
            && label_pos.x + label_width * 0.5 <= screen_width()
            && label_pos.y - label_height * 0.5 >= 0.0
            && label_pos.y + label_height * 0.5 <= screen_height();
        let notification_scale = NOTIFICATION_TEXT_SCALE;
        let notification_width = self
            .notifications
            .iter()
            .map(|notification| {
                self.assets
                    .tall_font
                    .measure_width_scaled(&notification.message, notification_scale)
            })
            .fold(0.0_f32, f32::max);
        let notification_line_height = self.assets.tall_font.line_height_scaled(notification_scale);
        let notification_origin = notification_stack_position(
            screen_width(),
            screen_height(),
            notification_width,
            self.notifications.len(),
            notification_line_height,
        );
        let notification_inside = notification_stack_inside(
            notification_origin,
            notification_width,
            self.notifications.len(),
            notification_line_height,
            screen_width(),
            screen_height(),
        );

        let energy_center_x = energy_hud_x + REFERENCE_GAME_HEALTHBAR_WIDTH * 0.5;
        let energy_visual_half_width = reference_game_energy_half_width(screen_width());
        let energy_digits = energy_digit_count(self.ship.energy.max(0.0) as u32) as f32;
        let energy_digits_width = energy_digits * self.assets.energy_font.frame_size.x;
        let energy_digits_x = energy_digits_start_x(
            screen_width(),
            self.assets.energy_font.frame_size.x,
            self.ship.energy.max(0.0) as u32,
        );
        let energy_digits_inside = energy_digits_x >= 0.0
            && energy_digits_x + energy_digits_width <= screen_width()
            && self.assets.energy_font.frame_size.y <= screen_height();
        let energy_digits_right_aligned =
            (energy_digits_x + energy_digits_width - screen_width()).abs() <= 1.0;
        let energy_inside = energy_hud_x >= 0.0
            && energy_hud_x + REFERENCE_GAME_HEALTHBAR_WIDTH <= screen_width()
            && energy_center_x - energy_visual_half_width >= 0.0
            && energy_center_x + energy_visual_half_width <= screen_width();
        HudMetricsMeasure {
            energy_inside,
            energy_center_delta: (energy_center_x - screen_width() * 0.5).abs(),
            energy_digits_inside,
            energy_digits_right_aligned,
            hud_text_inside: HUD_TEXT_X >= 0.0
                && hud_right <= screen_width()
                && hud_bottom <= screen_height(),
            hud_radar_overlap,
            radar_bottom_right,
            radar_label_inside,
            right_status_panel_inside,
            right_status_panel_clear_of_radar,
            left_item_panel_inside,
            screen_width: screen_width(),
            screen_height: screen_height(),
            label_inside,
            label_scale,
            label_width,
            notification_inside,
            notification_scale,
            notification_width,
            text_positive_scale: HUD_TEXT_SCALE > 0.0
                && label_scale > 0.0
                && notification_scale > 0.0,
        }
    }
}

fn json_metric_number(value: Option<f32>) -> String {
    json_metric_number_with_precision(value, 2)
}

fn json_metric_number_with_precision(value: Option<f32>, precision: usize) -> String {
    match value {
        Some(value) if value.is_finite() => format!("{value:.precision$}"),
        _ => "null".to_string(),
    }
}

#[cfg(test)]
mod metric_number_tests {
    use super::{
        Game, HudMetricsMeasure, RemoteWeaponPresentationSuppressionReason, append_metric_fields,
        json_metric_number, json_metric_number_with_precision, json_metric_string,
        json_network_security_generation, remote_weapon_presentation_suppression_metrics_json,
    };

    fn metrics_test_payload(game: &Game) -> String {
        use crate::game::LocalRenderPrediction;
        game.hud_ui_metrics_payload(
            &LocalRenderPrediction {
                position: super::Vec2::ZERO,
                velocity: super::Vec2::ZERO,
                orientation: 0.0,
                input_bits: 0,
                partial_seconds: 0.0,
            },
            &[],
            64.0,
            64.0,
            HudMetricsMeasure {
                screen_width: 1440.0,
                screen_height: 900.0,
                energy_inside: true,
                energy_center_delta: 1.5,
                energy_digits_inside: true,
                energy_digits_right_aligned: false,
                hud_text_inside: true,
                hud_radar_overlap: false,
                radar_bottom_right: true,
                radar_label_inside: true,
                right_status_panel_inside: true,
                right_status_panel_clear_of_radar: true,
                left_item_panel_inside: true,
                label_inside: true,
                label_scale: 0.42,
                label_width: 128.5,
                notification_inside: true,
                notification_scale: 0.35,
                notification_width: 200.25,
                text_positive_scale: true,
            },
        )
    }

    #[test]
    fn projectile_diagnostics_distinguish_installed_from_prepared_render_view() {
        use crate::remote_weapon::{
            RemoteWeaponArenaScope, RemoteWeaponSource, RemoteWeaponSourceIdentity,
            remote_weapon_projectiles,
        };
        let mut game = Game::new_network_authority_test();
        let tuning = game.ship_tuning();
        let source = RemoteWeaponSource::capture(
            RemoteWeaponSourceIdentity::authenticated(768, 7, 1).unwrap(),
            RemoteWeaponArenaScope::authenticated(1, 1),
            super::vec2(100.0, 200.0),
            super::Vec2::ZERO,
            0.25,
            768,
            0,
            tuning,
            false,
        )
        .unwrap()
        .with_realtime_order(4, 1120, 2)
        .unwrap();
        game.remote_projectiles = remote_weapon_projectiles(source, 1, tuning, &game.tuning);
        let payload: serde_json::Value =
            serde_json::from_str(&metrics_test_payload(&game)).unwrap();
        assert_eq!(
            payload["remoteProjectileSources"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            payload["presentedRemoteProjectileSources"],
            serde_json::json!([]),
            "an installed projectile waiting for its delayed birth is not a draw claim"
        );
        let mut presented = game.remote_projectiles[0];
        game.remote_projectiles[0].pos.x = 150.0;
        presented.pos.x = 110.0;
        game.prepared_remote_projectiles.push(presented);
        let payload: serde_json::Value =
            serde_json::from_str(&metrics_test_payload(&game)).unwrap();
        assert_eq!(payload["remoteProjectileSources"][0]["x"], 150.0);
        assert_eq!(payload["presentedRemoteProjectileSources"][0]["x"], 110.0);
        assert_eq!(
            payload["presentedRemoteProjectileSources"][0]["shotGroupId"],
            "2"
        );
        assert_eq!(payload["presentedRemoteProjectileSources"][0]["eventId"], 4);
        assert_eq!(payload["presentedRemoteProjectileSourcesOverflow"], 0);
        game.prepared_remote_projectiles.resize(65, presented);
        let payload: serde_json::Value =
            serde_json::from_str(&metrics_test_payload(&game)).unwrap();
        assert_eq!(
            payload["presentedRemoteProjectileSources"]
                .as_array()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(payload["presentedRemoteProjectileSourcesOverflow"], 1);
    }

    #[test]
    fn heading_diagnostics_preserve_precision_and_distinguish_source_from_display() {
        use crate::network::NetworkPlayer;
        use crate::protocol::PlayerRosterEntry;

        let mut game = Game::new_network_authority_test();
        game.ship.orientation = 0.625;
        let mut player = NetworkPlayer::from_roster(&PlayerRosterEntry {
            ship: 0,
            audio: 0,
            name: "turning".into(),
            squad: String::new(),
            kill_points: 0,
            flag_points: 0,
            player_id: 12,
            frequency: 7,
            wins: 0,
            losses: 0,
            attach_parent: u16::MAX,
            flags: 0,
            koth: 0,
        });
        player.position = Some(super::vec2(100.0, 200.0));
        player.render_position = player.position;
        player.orientation = 0.625;
        player.directed_presentation = true;
        player.directed_render_orientation = Some(0.31416667);
        game.network_world.players.insert(12, player);

        let payload: serde_json::Value =
            serde_json::from_str(&metrics_test_payload(&game)).unwrap();
        assert_eq!(payload["playerOrientation"], 0.625);
        let remote = &payload["remotePlayers"][0];
        assert_eq!(remote["orientation"], 0.625);
        assert!((remote["displayOrientation"].as_f64().unwrap() - 0.31416667).abs() < 0.00001);
        assert_ne!(remote["orientation"], remote["displayOrientation"]);
    }

    #[test]
    fn remote_ship_explosion_witnesses_preserve_identity_position_and_wrapped_ordinal() {
        use crate::game::NetworkDeathIdentity;
        let mut game = Game::new_network_authority_test();
        game.remote_ship_explosion_count = u32::MAX;
        let first = NetworkDeathIdentity {
            authority_segment: u64::MAX,
            killed_id: 768,
            life_seq: 2,
            green_collectible_id: 19,
            respawn_at_tick: 3_023,
        };
        game.add_remote_ship_explosion(super::vec2(7_892.0, 8_596.0), first);
        game.add_remote_ship_explosion(
            super::vec2(7_834.0, 8_572.0),
            NetworkDeathIdentity {
                killed_id: 769,
                green_collectible_id: 20,
                ..first
            },
        );
        let decoded: serde_json::Value =
            serde_json::from_str(&metrics_test_payload(&game)).unwrap();
        let entries = decoded["remoteShipExplosionWitnesses"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["ordinal"], 0);
        assert_eq!(entries[0]["authoritySegment"], u64::MAX.to_string());
        assert_eq!(entries[0]["playerId"], 768);
        assert_eq!(entries[0]["nextLifeSeq"], 2);
        assert_eq!(entries[0]["greenCollectibleId"], 19);
        assert_eq!(entries[0]["respawnAtTick"], 3_023);
        assert_eq!(entries[0]["x"], 7_892.0);
        assert_eq!(entries[0]["y"], 8_596.0);
        assert_eq!(entries[1]["playerId"], 769);
        assert_eq!(entries[1]["ordinal"], 1);
        assert_eq!(decoded["lastRemoteShipExplosionX"], 7_834.0);
    }

    #[test]
    fn remote_ship_explosion_witness_history_is_bounded_without_dropping_effects() {
        use crate::game::{MAX_REMOTE_SHIP_EXPLOSION_WITNESSES, NetworkDeathIdentity};
        let mut game = Game::new_network_authority_test();
        for life in 1..=70 {
            game.add_remote_ship_explosion(
                super::vec2(life as f32, 0.0),
                NetworkDeathIdentity {
                    authority_segment: 1,
                    killed_id: 768,
                    life_seq: life,
                    green_collectible_id: life,
                    respawn_at_tick: life + 100,
                },
            );
        }
        assert_eq!(
            game.remote_ship_explosion_witnesses.len(),
            MAX_REMOTE_SHIP_EXPLOSION_WITNESSES
        );
        assert_eq!(
            game.remote_ship_explosion_witnesses
                .front()
                .unwrap()
                .ordinal,
            7
        );
        assert_eq!(
            game.remote_ship_explosion_witnesses.back().unwrap().ordinal,
            70
        );
        assert_eq!(game.ship_explosions.len(), 70);
        assert_eq!(game.remote_ship_explosion_count, 70);
    }

    #[test]
    fn self_ship_metrics_keep_local_and_authoritative_roster_witnesses_separate() {
        use crate::game::{AuthorityScopeIdentity, ClientAuthorityReadiness};
        use crate::network::NetworkPlayer;
        use crate::protocol::PlayerRosterEntry;

        let mut game = Game::new_network_authority_test();
        game.ship.ship_type = 6;
        let read = |game: &Game| -> serde_json::Value {
            serde_json::from_str(&metrics_test_payload(game)).unwrap()
        };
        assert_eq!(read(&game)["playerShip"], 6);
        assert!(read(&game)["selfRosterShip"].is_null());
        game.client_authority.install_test_scope(
            1,
            AuthorityScopeIdentity {
                player_id: Some(91),
                ..Default::default()
            },
            ClientAuthorityReadiness::default(),
        );
        let entry = PlayerRosterEntry {
            ship: 0,
            audio: 0,
            name: "Ace".into(),
            squad: String::new(),
            kill_points: 0,
            flag_points: 0,
            player_id: 91,
            frequency: 1,
            wins: 0,
            losses: 0,
            attach_parent: u16::MAX,
            flags: 0,
            koth: 0,
        };
        game.network_world
            .players
            .insert(91, NetworkPlayer::from_roster(&entry));
        let disagree = read(&game);
        assert_eq!(disagree["playerShip"], 6);
        assert_eq!(disagree["selfRosterShip"], 0);
        game.network_world.players.get_mut(&91).unwrap().ship = 6;
        assert_eq!(read(&game)["selfRosterShip"], 6);
        // A remaining unrelated roster row cannot stand in for our identity.
        game.network_world
            .players
            .insert(92, NetworkPlayer::from_roster(&entry));
        game.network_world.players.remove(&91);
        assert!(read(&game)["selfRosterShip"].is_null());
    }

    #[test]
    fn brick_diagnostics_preserve_tiles_team_and_age_with_explicit_truncation() {
        let mut game = Game::new_network_authority_test();
        let team = game.player_frequency();
        for x in 0..=super::MAX_BRICK_TILE_METRIC_ENTRIES {
            game.bricks.push(crate::Brick {
                tile: super::ivec2(x as i32, 8),
                previous_tile: 0,
                team,
                age: 1.25,
                ttl: 118.75,
                server_timestamp: None,
            });
        }
        let payload: serde_json::Value =
            serde_json::from_str(&metrics_test_payload(&game)).unwrap();
        let state = &payload["localGameplayState"];
        assert_eq!(state["bricksActive"], 257);
        assert_eq!(state["brickTilesOverflow"], 1);
        assert_eq!(state["brickTiles"].as_array().unwrap().len(), 256);
        assert_eq!(
            state["brickTiles"][0],
            serde_json::json!({
                "x": 0, "y": 8, "team": team, "friendly": true, "age": 1.25, "ttl": 118.75
            })
        );
    }

    #[test]
    fn non_finite_diagnostic_numbers_are_valid_json_nulls() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(json_metric_number(Some(value)), "null");
            assert_eq!(json_metric_number_with_precision(Some(value), 4), "null");
        }
        assert_eq!(json_metric_number_with_precision(Some(-1.25), 4), "-1.2500");
    }

    #[test]
    fn diagnostic_strings_are_valid_json_without_wasm_only_dependencies() {
        assert_eq!(json_metric_string(None), "null");
        assert_eq!(
            json_metric_string(Some("DAMAGE: \\\"Rocket\\\" lost.\n")),
            "\"DAMAGE: \\\\\\\"Rocket\\\\\\\" lost.\\n\""
        );
    }

    #[test]
    fn decoy_metrics_preserve_launch_geometry_turn_and_escaped_owner_name() {
        use super::{
            Decoy, RemoteWeaponArenaScope, RemoteWeaponSource, RemoteWeaponSourceIdentity,
            remote_decoys_metrics_json, vec2,
        };
        let mut game = Game::new_network_authority_test();
        let mut source = RemoteWeaponSource::capture(
            RemoteWeaponSourceIdentity::Authenticated {
                player_id: 12,
                session_id: 34,
                life_seq: 5,
            },
            RemoteWeaponArenaScope::authenticated(1, 2),
            vec2(100.0, 200.0),
            vec2(30.0, -40.0),
            0.25,
            7,
            4,
            game.tuning.ship(4),
            game.tuning.gravity_bombs,
        )
        .unwrap();
        source.shot_group_id = Some(u64::MAX - 1);
        game.decoys.push(Decoy {
            authority_binding: None,
            presentation_clock: None,
            prediction: None,
            pos: vec2(115.0, 180.0),
            vel: vec2(30.0, -40.0),
            frequency: 7,
            orientation: 0.15,
            initial_orientation: 0.25,
            remote_source: Some(source),
            last_repel_mutation_event_id: None,
            authoritative_repel_state: None,
            ship_type: 4,
            owner_name: "ace\"\n".to_owned(),
            owner_bounty: 30,
            ttl: 11.5,
            age: 0.5,
        });
        let data: serde_json::Value =
            serde_json::from_str(&remote_decoys_metrics_json(&game)).unwrap();
        let decoy = &data[0];
        assert_eq!(decoy["sessionId"], "34");
        assert_eq!(decoy["lifeSeq"], 5);
        assert_eq!(decoy["serverSessionId"], "1");
        assert_eq!(decoy["arenaStreamId"], "2");
        assert_eq!(decoy["shotGroupId"], (u64::MAX - 1).to_string());
        assert_eq!(decoy["ownerName"], "ace\"\n");
        assert_eq!(decoy["ship"], 4);
        assert_eq!(decoy["fireX"], 100.0);
        assert_eq!(decoy["fireY"], 200.0);
        assert_eq!(decoy["fireVelocityX"], 30.0);
        assert_eq!(decoy["fireVelocityY"], -40.0);
        assert_eq!(decoy["initialOrientation"], 0.25);
        assert_eq!(decoy["orientation"], 0.15);
        assert_eq!(decoy["x"], 115.0);
        assert_eq!(decoy["y"], 180.0);
        game.decoys[0].orientation = f32::NAN;
        let data: serde_json::Value =
            serde_json::from_str(&remote_decoys_metrics_json(&game)).unwrap();
        assert!(data[0]["orientation"].is_null());
    }

    #[test]
    fn security_generation_metrics_distinguish_current_and_stale_bindings() {
        use crate::game::{
            AuthorityScopeIdentity, ClientAuthorityReadiness, NetworkSecurityGeneration,
        };

        let mut game = Game::new_network_authority_test();
        let scope = AuthorityScopeIdentity {
            server_session_id: Some(u64::MAX - 2),
            arena_stream_id: Some(81),
            player_id: Some(91),
            life_seq: Some(101),
            alive: Some(true),
        };
        let readiness = ClientAuthorityReadiness {
            joined: true,
            arena_epoch: Some(111),
            settings: true,
            security: true,
            map: true,
            files: true,
        };
        game.client_authority
            .install_test_scope(4, scope, readiness);
        let current = NetworkSecurityGeneration {
            server_session_id: u64::MAX - 2,
            arena_epoch: 111,
        };
        game.network_security_generation = Some(current);
        game.network_security_challenge_generation = Some(current);
        game.network_security_challenge = Some(crate::protocol::SecurityChallenge::default());

        let current_payload: serde_json::Value =
            serde_json::from_str(&metrics_test_payload(&game)).unwrap();
        let current_state = &current_payload["localGameplayState"];
        assert_eq!(
            current_state["networkSecurityGeneration"],
            serde_json::json!({"serverSessionId": (u64::MAX - 2).to_string(), "arenaEpoch": 111})
        );
        assert_eq!(current_state["networkSecurityChallengePresent"], true);
        assert_eq!(
            current_state["networkSecurityGenerationMatchesAuthorityScope"],
            true
        );
        assert_eq!(
            current_state["networkSecurityChallengeGenerationMatchesCurrent"],
            true
        );

        game.network_security_challenge_generation = Some(NetworkSecurityGeneration {
            server_session_id: u64::MAX - 3,
            arena_epoch: 110,
        });
        let stale_payload: serde_json::Value =
            serde_json::from_str(&metrics_test_payload(&game)).unwrap();
        let stale_state = &stale_payload["localGameplayState"];
        assert_eq!(
            stale_state["networkSecurityGenerationMatchesAuthorityScope"],
            true
        );
        assert_eq!(
            stale_state["networkSecurityChallengeGenerationMatchesCurrent"],
            false
        );
        assert_eq!(
            stale_state["networkSecurityChallengeGeneration"]["arenaEpoch"],
            110
        );

        game.network_security_generation = Some(NetworkSecurityGeneration {
            server_session_id: u64::MAX - 2,
            arena_epoch: 109,
        });
        let scope_mismatch_payload: serde_json::Value =
            serde_json::from_str(&metrics_test_payload(&game)).unwrap();
        let scope_mismatch_state = &scope_mismatch_payload["localGameplayState"];
        assert_eq!(
            scope_mismatch_state["networkSecurityGenerationMatchesAuthorityScope"],
            false
        );
        assert_eq!(
            scope_mismatch_state["networkSecurityChallengeGenerationMatchesCurrent"],
            false
        );
        assert_eq!(json_network_security_generation(None), "null");
    }

    #[test]
    fn remote_weapon_suppression_metrics_keep_all_typed_reasons_valid_json() {
        let mut game = Game::new_network_authority_test();
        game.remote_weapon_presentations
            .mark_terminal(RemoteWeaponPresentationSuppressionReason::CapacityRejected);
        game.remote_weapon_presentations
            .mark_terminal(RemoteWeaponPresentationSuppressionReason::ScopeCancelled);

        let encoded = remote_weapon_presentation_suppression_metrics_json(&game);
        let decoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        for reason in RemoteWeaponPresentationSuppressionReason::ALL {
            assert_eq!(decoded["pending"][reason.label()], 0);
        }
        assert_eq!(decoded["terminal"]["capacityRejected"], 1);
        assert_eq!(decoded["terminal"]["scopeCancelled"], 1);
        assert_eq!(decoded["terminal"]["invalidIdentity"], 0);
    }

    #[test]
    fn metric_field_splice_merges_into_a_well_formed_object() {
        let merged = append_metric_fields("{\"a\":1}", ",\"b\":2");
        let decoded: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(decoded["a"], 1);
        assert_eq!(decoded["b"], 2);
    }

    #[test]
    fn a_malformed_metric_block_publishes_a_placeholder_instead_of_panicking() {
        // The boot-bricking shape: a value left the block without its closing
        // brace, and the old `strip_suffix('}').expect(..)` turned that into a
        // wasm panic that took the whole client down before it drew a frame.
        let broken = append_metric_fields("{\"a\":1,\"flagView\":{\"id\":0},", ",\"b\":2");
        let decoded: serde_json::Value = serde_json::from_str(&broken)
            .expect("a malformed metrics block must still publish parseable JSON");
        assert_eq!(decoded["metricsSpliceFailed"], true);
        assert!(decoded["metricsBlockPrefix"].is_string());
    }

    #[test]
    fn the_published_hud_payload_parses_and_one_bare_word_value_breaks_it() {
        // The browser hands this string straight to JSON.parse, so a single value
        // emitted as a bare word (a raw `NaN`, an unquoted name) kills the HUD
        // consumer for the whole session. This is the only place the complete
        // payload is validated in one piece, and it is why the builder sits away
        // from the `?trace=1` gate and takes every measured number as an argument.
        use super::HudMetricsMeasure;
        use crate::game::LocalRenderPrediction;
        let game = Game::new_network_authority_test();
        let payload = game.hud_ui_metrics_payload(
            &LocalRenderPrediction {
                position: super::Vec2::ZERO,
                velocity: super::Vec2::ZERO,
                orientation: 0.0,
                input_bits: 0,
                partial_seconds: 0.0,
            },
            &[],
            64.0,
            64.0,
            HudMetricsMeasure {
                screen_width: 1440.0,
                screen_height: 900.0,
                energy_inside: true,
                energy_center_delta: 1.5,
                energy_digits_inside: true,
                energy_digits_right_aligned: false,
                hud_text_inside: true,
                hud_radar_overlap: false,
                radar_bottom_right: true,
                radar_label_inside: true,
                right_status_panel_inside: true,
                right_status_panel_clear_of_radar: true,
                left_item_panel_inside: true,
                label_inside: true,
                label_scale: 0.42,
                label_width: 128.5,
                notification_inside: true,
                notification_scale: 0.35,
                notification_width: 200.25,
                text_positive_scale: true,
            },
        );
        let decoded: serde_json::Value = serde_json::from_str(&payload).unwrap_or_else(|error| {
            panic!("the published HUD payload must parse: {error}\n{payload}")
        });
        assert!(
            decoded["localGameplayState"].is_object(),
            "the nested gameplay block must arrive as an object, not a splice of text"
        );
        // A browser gate reads the switch's merge cost off these fields, so they
        // must arrive as numbers even on a game that merged nothing.
        for key in [
            "lvzMergeCount",
            "lvzMergeTotalMs",
            "lvzMergeMaxMs",
            "zoneLvzObjects",
            "zoneLvzTextures",
        ] {
            assert!(
                decoded["localGameplayState"][key].is_number(),
                "{key} must publish a number, got {}",
                decoded["localGameplayState"][key]
            );
        }

        let marker = "\"playerEnergy\":";
        let at = payload
            .find(marker)
            .expect("the payload carries playerEnergy");
        let value_start = at + marker.len();
        let value_end = value_start
            + payload[value_start..]
                .find([',', '}'])
                .expect("playerEnergy carries a value");
        let bare_word = format!("{}NaN{}", &payload[..value_start], &payload[value_end..]);
        serde_json::from_str::<serde_json::Value>(&bare_word)
            .expect_err("one bare-word value must break the payload the browser parses");
    }

    #[test]
    fn the_zone_lvz_merge_counters_publish_the_accounted_switch_cost() {
        // The switch-frame measurement reads these off a live browser, so an
        // accounted value that never reaches the payload would silently report a
        // zero-cost arena switch.
        use super::HudMetricsMeasure;
        use crate::game::LocalRenderPrediction;
        let mut game = Game::new_network_authority_test();
        game.lvz_merge_count = 3;
        game.lvz_merge_total_ms = 12.5;
        game.lvz_merge_max_ms = 7.25;
        let payload = game.hud_ui_metrics_payload(
            &LocalRenderPrediction {
                position: super::Vec2::ZERO,
                velocity: super::Vec2::ZERO,
                orientation: 0.0,
                input_bits: 0,
                partial_seconds: 0.0,
            },
            &[],
            64.0,
            64.0,
            HudMetricsMeasure {
                screen_width: 1440.0,
                screen_height: 900.0,
                energy_inside: true,
                energy_center_delta: 1.5,
                energy_digits_inside: true,
                energy_digits_right_aligned: false,
                hud_text_inside: true,
                hud_radar_overlap: false,
                radar_bottom_right: true,
                radar_label_inside: true,
                right_status_panel_inside: true,
                right_status_panel_clear_of_radar: true,
                left_item_panel_inside: true,
                label_inside: true,
                label_scale: 0.42,
                label_width: 128.5,
                notification_inside: true,
                notification_scale: 0.35,
                notification_width: 200.25,
                text_positive_scale: true,
            },
        );
        let decoded: serde_json::Value = serde_json::from_str(&payload).unwrap();
        let local = &decoded["localGameplayState"];
        assert_eq!(local["lvzMergeCount"], 3);
        assert_eq!(local["lvzMergeTotalMs"], 12.5);
        assert_eq!(local["lvzMergeMaxMs"], 7.25);
        assert!(
            local["zoneLvzObjects"].is_number() && local["zoneLvzTextures"].is_number(),
            "the merged zone volume must publish alongside its cost"
        );
    }
}

fn json_metric_u16(value: Option<u16>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    }
}

fn json_metric_u32(value: Option<u32>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    }
}

fn json_metric_u64(value: Option<u64>) -> String {
    match value {
        // JavaScript cannot exactly represent every u64 as a Number. Keep
        // authority generations decimal-string encoded so retained browser
        // diagnostics never alias two distinct server or arena identities.
        Some(value) => format!("\"{value}\""),
        None => "null".to_string(),
    }
}

fn json_network_security_generation(
    value: Option<crate::game::NetworkSecurityGeneration>,
) -> String {
    let Some(generation) = value else {
        return "null".to_string();
    };
    format!(
        "{{\"serverSessionId\":{},\"arenaEpoch\":{}}}",
        json_metric_u64(Some(generation.server_session_id)),
        generation.arena_epoch,
    )
}

fn json_metric_bool(value: Option<bool>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    }
}

fn json_metric_string(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "null".to_string();
    };
    let mut encoded = String::with_capacity(value.len() + 2);
    encoded.push('"');
    for character in value.chars() {
        match character {
            '"' => encoded.push_str("\\\""),
            '\\' => encoded.push_str("\\\\"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                write!(&mut encoded, "\\u{:04x}", character as u32)
                    .expect("writing to String cannot fail");
            }
            character => encoded.push(character),
        }
    }
    encoded.push('"');
    encoded
}

/// Append extra fields to a metric object without trusting the block's shape.
///
/// The HUD payload is assembled with `format!`, so one value that changes a
/// block's last character - an object-valued key, for example - used to reach
/// a `strip_suffix('}').expect(..)` in the middle of assembly and panic the
/// browser client during boot, before the first frame. A diagnostic payload is
/// never worth bricking the page for: a block that is not a JSON object is
/// replaced by a small valid object naming the failure, so the consumer still
/// parses it and the producer stays visible in the browser console.
fn append_metric_fields(object: &str, extra: &str) -> String {
    match object.strip_suffix('}') {
        Some(head) => format!("{head}{extra}}}"),
        None => {
            let prefix: String = object.chars().take(160).collect();
            format!(
                "{{\"metricsSpliceFailed\":true,\"metricsBlockPrefix\":{}}}",
                json_metric_string(Some(&prefix)),
            )
        }
    }
}

fn remote_weapon_presentation_suppression_metrics_json(game: &Game) -> String {
    let pending = RemoteWeaponPresentationSuppressionReason::ALL
        .into_iter()
        .map(|reason| {
            format!(
                "\"{}\":{}",
                reason.label(),
                game.remote_weapon_presentations.pending_for(reason)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let terminal = RemoteWeaponPresentationSuppressionReason::ALL
        .into_iter()
        .map(|reason| {
            format!(
                "\"{}\":{}",
                reason.label(),
                game.remote_weapon_presentations.terminal_for(reason)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"pending\":{{{pending}}},\"terminal\":{{{terminal}}}}}")
}

const MAX_BRICK_TILE_METRIC_ENTRIES: usize = 256;

fn active_brick_tiles_metrics_json(game: &Game) -> String {
    let viewer_frequency = game.viewer_frequency();
    let entries = game
        .bricks
        .iter()
        .take(MAX_BRICK_TILE_METRIC_ENTRIES)
        .map(|brick| {
            format!(
                "{{\"x\":{},\"y\":{},\"team\":{},\"friendly\":{},\"age\":{},\"ttl\":{}}}",
                brick.tile.x,
                brick.tile.y,
                brick.team,
                brick.team == viewer_frequency,
                json_metric_number_with_precision(Some(brick.age), 2),
                json_metric_number_with_precision(Some(brick.ttl), 2)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{entries}]")
}

fn remote_projectile_sources_metrics_json(game: &Game, source: &[RemoteProjectile]) -> String {
    let mut projectiles = source.iter().collect::<Vec<_>>();
    let ship_position = game.ship.pos;
    if projectiles.len() > MAX_REMOTE_PROJECTILE_SOURCE_METRIC_ENTRIES {
        projectiles.select_nth_unstable_by(
            MAX_REMOTE_PROJECTILE_SOURCE_METRIC_ENTRIES,
            |left, right| remote_projectile_metric_order(ship_position, left, right),
        );
        projectiles.truncate(MAX_REMOTE_PROJECTILE_SOURCE_METRIC_ENTRIES);
    }
    projectiles.sort_by(|left, right| remote_projectile_metric_order(ship_position, left, right));
    let entries = projectiles
        .into_iter()
        .take(MAX_REMOTE_PROJECTILE_SOURCE_METRIC_ENTRIES)
        .map(|projectile| {
            let kind = match projectile.kind {
                RemoteProjectileKind::Bullet { .. } => "bullet",
                RemoteProjectileKind::Burst { .. } => "burst",
                RemoteProjectileKind::Shrapnel { .. } => "shrapnel",
                RemoteProjectileKind::Bomb { .. } => "bomb",
                RemoteProjectileKind::Mine { .. } => "mine",
                RemoteProjectileKind::Thor => "thor",
            };
            let active = match projectile.kind {
                RemoteProjectileKind::Burst { active } => {
                    if active { "true" } else { "false" }
                }
                _ => "null",
            };
            let (authenticated, session_id, life_seq) = match projectile.source.identity {
                RemoteWeaponSourceIdentity::Authenticated {
                    session_id,
                    life_seq,
                    ..
                } => (true, session_id.to_string(), life_seq.to_string()),
                RemoteWeaponSourceIdentity::Legacy { life_seq, .. } => (
                    false,
                    String::new(),
                    life_seq.map_or_else(|| "null".to_string(), |value| value.to_string()),
                ),
            };
            let (server_session_id, arena_stream_id) = projectile
                .source
                .arena_scope
                .map_or_else(
                    || (String::new(), String::new()),
                    |scope| {
                        (
                            scope.server_session_id.to_string(),
                            scope.arena_stream_id.to_string(),
                        )
                    },
                );
            let (authority_projectile_id, authority_state_tick) = projectile
                .authority_binding
                .map_or((None, None), |binding| {
                    (Some(binding.projectile_id), Some(binding.state_tick))
                });
            format!(
                "{{\"playerId\":{},\"authenticated\":{},\"sessionId\":\"{}\",\"lifeSeq\":{},\"serverSessionId\":\"{}\",\"arenaStreamId\":\"{}\",\"eventId\":{},\"serverTick\":{},\"shotGroupId\":{},\"sourceProjectileIndex\":{},\"authorityProjectileId\":{},\"authorityStateTick\":{},\"motionFrozen\":{},\"kind\":\"{}\",\"active\":{},\"fireX\":{},\"fireY\":{},\"fireVelocityX\":{},\"fireVelocityY\":{},\"fireOrientation\":{},\"frequency\":{},\"ship\":{},\"bombGravity\":{},\"bombBounces\":{},\"x\":{},\"y\":{},\"velocityX\":{},\"velocityY\":{},\"ttl\":{},\"repelMutationEventId\":{},\"authoritativeRepel\":{}}}",
                projectile.source.identity.player_id(),
                authenticated,
                session_id,
                life_seq,
                server_session_id,
                arena_stream_id,
                json_metric_u32(projectile.source.realtime_event_id),
                json_metric_u32(projectile.source.realtime_server_tick),
                json_metric_u64(projectile.source.shot_group_id),
                projectile.source_projectile_index,
                json_metric_u32(authority_projectile_id),
                json_metric_u32(authority_state_tick),
                projectile.presentation_clock.is_some_and(|clock| clock.motion_frozen),
                kind,
                active,
                json_metric_number_with_precision(Some(projectile.source.fire_position.x), 3),
                json_metric_number_with_precision(Some(projectile.source.fire_position.y), 3),
                json_metric_number_with_precision(Some(projectile.source.fire_velocity.x), 3),
                json_metric_number_with_precision(Some(projectile.source.fire_velocity.y), 3),
                json_metric_number_with_precision(Some(projectile.source.fire_orientation), 5),
                projectile.source.frequency,
                projectile.source.ship,
                projectile
                    .source
                    .bomb_gravity
                    .map_or_else(|| "null".to_string(), |value| value.to_string()),
                projectile.source.bomb_bounces,
                json_metric_number_with_precision(Some(projectile.pos.x), 3),
                json_metric_number_with_precision(Some(projectile.pos.y), 3),
                json_metric_number_with_precision(Some(projectile.vel.x), 3),
                json_metric_number_with_precision(Some(projectile.vel.y), 3),
                json_metric_number_with_precision(Some(projectile.ttl), 4),
                json_metric_u32(projectile.last_repel_mutation_event_id),
                authoritative_repel_state_metrics_json(projectile.authoritative_repel_state),
            )
        })
        .collect::<Vec<_>>();
    format!("[{}]", entries.join(","))
}

fn remote_projectile_metric_order(
    ship_position: Vec2,
    left: &RemoteProjectile,
    right: &RemoteProjectile,
) -> std::cmp::Ordering {
    left.pos
        .distance_squared(ship_position)
        .total_cmp(&right.pos.distance_squared(ship_position))
        .then_with(|| {
            left.authority_binding
                .map(|binding| binding.projectile_id)
                .cmp(&right.authority_binding.map(|binding| binding.projectile_id))
        })
        .then_with(|| {
            left.source
                .realtime_event_id
                .cmp(&right.source.realtime_event_id)
        })
        .then_with(|| {
            left.source_projectile_index
                .cmp(&right.source_projectile_index)
        })
}

fn remote_decoys_metrics_json(game: &Game) -> String {
    let entries = game
        .decoys
        .iter()
        .filter_map(|decoy| {
            let source = decoy.remote_source?;
            let (session_id, life_seq) = match source.identity {
                RemoteWeaponSourceIdentity::Authenticated { session_id, life_seq, .. } =>
                    (Some(session_id), Some(life_seq)),
                RemoteWeaponSourceIdentity::Legacy { life_seq, .. } => (None, life_seq),
            };
            Some(format!(
                "{{\"playerId\":{},\"sessionId\":{},\"lifeSeq\":{},\"serverSessionId\":{},\"arenaStreamId\":{},\"shotGroupId\":{},\"eventId\":{},\"serverTick\":{},\"frequency\":{},\"ship\":{},\"ownerName\":{},\"fireX\":{},\"fireY\":{},\"fireVelocityX\":{},\"fireVelocityY\":{},\"initialOrientation\":{},\"orientation\":{},\"x\":{},\"y\":{},\"velocityX\":{},\"velocityY\":{},\"ttl\":{},\"repelMutationEventId\":{},\"authoritativeRepel\":{}}}",
                source.identity.player_id(),
                json_metric_u64(session_id),
                json_metric_u32(life_seq),
                json_metric_u64(source.arena_scope.map(|scope| scope.server_session_id)),
                json_metric_u64(source.arena_scope.map(|scope| scope.arena_stream_id)),
                json_metric_u64(source.shot_group_id),
                json_metric_u32(source.realtime_event_id),
                json_metric_u32(source.realtime_server_tick),
                decoy.frequency,
                decoy.ship_type,
                json_metric_string(Some(&decoy.owner_name)),
                json_metric_number_with_precision(Some(source.fire_position.x), 3),
                json_metric_number_with_precision(Some(source.fire_position.y), 3),
                json_metric_number_with_precision(Some(source.fire_velocity.x), 3),
                json_metric_number_with_precision(Some(source.fire_velocity.y), 3),
                json_metric_number_with_precision(Some(decoy.initial_orientation), 5),
                json_metric_number_with_precision(Some(decoy.orientation), 5),
                json_metric_number_with_precision(Some(decoy.pos.x), 3),
                json_metric_number_with_precision(Some(decoy.pos.y), 3),
                json_metric_number_with_precision(Some(decoy.vel.x), 3),
                json_metric_number_with_precision(Some(decoy.vel.y), 3),
                json_metric_number_with_precision(Some(decoy.ttl), 4),
                json_metric_u32(decoy.last_repel_mutation_event_id),
                authoritative_repel_state_metrics_json(decoy.authoritative_repel_state),
            ))
        })
        .take(MAX_REMOTE_DECOY_METRIC_ENTRIES)
        .collect::<Vec<_>>();
    format!("[{}]", entries.join(","))
}

fn authoritative_repel_state_metrics_json(
    state: Option<crate::realtime_event::RealtimeRepelMutationState>,
) -> String {
    let Some(state) = state else {
        return "null".to_owned();
    };
    format!(
        "{{\"entityKind\":{},\"sourcePlayerId\":{},\"sourceSessionId\":\"{}\",\"sourceLifeSeq\":{},\"sourceServerTick\":{},\"sourceWeaponData\":{},\"sourceProjectileIndex\":{},\"resultWeaponData\":{},\"positionX\":{},\"positionY\":{},\"velocityX\":{},\"velocityY\":{},\"expiresAtTick\":{},\"bouncesRemaining\":{}}}",
        state.entity_kind,
        state.source_player_id,
        state.source_session_id,
        state.source_life_seq,
        state.source_server_tick,
        state.source_weapon_data,
        state.source_projectile_index,
        state.result_weapon_data,
        json_metric_number_with_precision(Some(state.position_x), 3),
        json_metric_number_with_precision(Some(state.position_y), 3),
        json_metric_number_with_precision(Some(state.velocity_x), 3),
        json_metric_number_with_precision(Some(state.velocity_y), 3),
        state.expires_at_tick,
        state.bounces_remaining,
    )
}

fn remote_players_metrics_json(game: &Game, viewer_frequency: u16) -> String {
    let mut players = game
        .network_world
        .visible_players()
        .filter(|player| {
            player.position.is_some()
                && network_player_world_visible(
                    player.togglables,
                    player.frequency,
                    viewer_frequency,
                    game.ship.xradar,
                )
        })
        .collect::<Vec<_>>();
    players.sort_by_key(|player| player.player_id);

    let entries = players
        .into_iter()
        .filter_map(|player| {
            let display = player.display_position()?;
            let position = player.position?;
            Some(format!(
                "{{\"id\":{},\"lifeSeq\":{},\"frequency\":{},\"displayX\":{},\"displayY\":{},\"positionX\":{},\"positionY\":{},\"velocityX\":{},\"velocityY\":{},\"orientation\":{},\"displayOrientation\":{},\"energy\":{},\"serverEnergy\":{},\"lastServerPositionTick\":{}}}",
                player.player_id,
                json_metric_u32(player.life_seq),
                player.frequency,
                json_metric_number(Some(display.x)),
                json_metric_number(Some(display.y)),
                json_metric_number(Some(position.x)),
                json_metric_number(Some(position.y)),
                json_metric_number(Some(player.velocity.x)),
                json_metric_number(Some(player.velocity.y)),
                json_metric_number_with_precision(Some(player.orientation), 5),
                json_metric_number_with_precision(Some(player.display_orientation()), 5),
                json_metric_number(player.display_energy()),
                json_metric_number(player.energy),
                json_metric_u16(player.last_server_position_tick),
            ))
        })
        .collect::<Vec<_>>();

    format!("[{}]", entries.join(","))
}

fn scoreboard_player_ids_metrics_json(game: &Game) -> String {
    let ids = scoreboard_entries(
        game.local_player_name(),
        game.bot_score,
        game.player_deaths,
        game.player_bounty,
        game.flag_score,
        game.ball_score,
        game.ship.ship_type,
        game.ship.energy,
        &game.bots,
        &game.network_world,
    )
    .into_iter()
    .filter_map(|entry| entry.player_id)
    .map(|id| id.to_string())
    .collect::<Vec<_>>();
    format!("[{}]", ids.join(","))
}

// Use the same rows and derived values as the drawn expanded table, including
// the local row's authoritative network stats. This remains opt-in diagnostics.
fn scoreboard_player_stats_metrics_json(game: &Game) -> String {
    let rows = game.player_list_rows().into_iter().map(|row| {
        format!(
            "{{\"id\":{},\"name\":{},\"score\":{},\"killPoints\":{},\"kills\":{},\"deaths\":{},\"averageBounty\":{:.1},\"rating\":{}}}",
            json_metric_u16(row.player_id),
            json_metric_string(Some(&row.name)),
            row.total_score(),
            row.kill_points,
            row.kills,
            row.deaths,
            row.average_bounty(),
            row.rating(),
        )
    }).collect::<Vec<_>>();
    format!("[{}]", rows.join(","))
}

fn radar_player_ids_metrics_json(
    game: &Game,
    viewer_frequency: u16,
    screen_width: f32,
    screen_height: f32,
) -> String {
    let map = game.map();
    let view = small_radar_view(
        map.width,
        map.height,
        game.ship.pos,
        screen_width,
        screen_height,
        game.tuning.map_zoom_factor,
    );
    let max = view.max_tile();
    let attached_children = game.network_world.attached_children_by_parent();
    let mut ids = game
        .network_world
        .visible_players()
        .filter(|player| {
            network_player_radar_visible(
                player.togglables,
                player.frequency,
                viewer_frequency,
                game.ship.xradar,
            ) || attached_children
                .get(&player.player_id)
                .is_some_and(|children| {
                    children
                        .iter()
                        .copied()
                        .any(network_child_reveals_parent_on_radar)
                })
        })
        .filter_map(|player| {
            let tile = player.display_position()? / TILE_SIZE;
            (tile.x >= view.min_tile.x
                && tile.y >= view.min_tile.y
                && tile.x <= max.x
                && tile.y <= max.y)
                .then_some(player.player_id)
        })
        .collect::<Vec<_>>();
    ids.sort_unstable();
    format!(
        "[{}]",
        ids.into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn authoritative_snapshot_actors_metrics_json(game: &Game) -> String {
    let snapshot_id = game.network_world.authoritative_snapshot_id;
    let entries = game
        .network_world
        .authoritative_snapshot_actors
        .iter()
        .map(|actor| {
            format!(
                "{{\"latestSnapshotId\":{},\"id\":{},\"lifeSeq\":{}}}",
                json_metric_u16(snapshot_id),
                actor.player_id,
                json_metric_u32(actor.life_seq),
            )
        })
        .collect::<Vec<_>>();
    format!("[{}]", entries.join(","))
}

fn authoritative_snapshot_actor_trace_metrics_json(game: &Game) -> String {
    let cursor = game
        .network_world
        .authoritative_snapshot_actor_metrics_cursor
        .get();
    let mut snapshots = game
        .network_world
        .authoritative_snapshot_actor_trace
        .iter()
        .filter(|membership| {
            cursor.is_none_or(|(epoch, serial)| {
                membership.epoch != epoch || {
                    let distance = membership.serial.wrapping_sub(serial);
                    distance > 0 && distance < (1_u32 << 31)
                }
            })
        })
        .map(|membership| {
            let actors = membership
                .actors
                .iter()
                .map(|actor| {
                    format!(
                        "{{\"id\":{},\"lifeSeq\":{}}}",
                        actor.player_id,
                        json_metric_u32(actor.life_seq),
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{{\"epoch\":{},\"serial\":{},\"latestSnapshotId\":{},\"actors\":[{}]}}",
                membership.epoch, membership.serial, membership.snapshot_id, actors,
            )
        })
        .collect::<Vec<_>>();
    // Keep the latest authoritative membership visible in every metrics
    // sample even when no newer snapshot arrived. The browser cursor treats
    // the repeated serial as a current observation, while retaining each
    // forward frame only once in its complete trace.
    if snapshots.is_empty()
        && let Some(membership) = game.network_world.authoritative_snapshot_actor_trace.back()
    {
        let actors = membership
            .actors
            .iter()
            .map(|actor| {
                format!(
                    "{{\"id\":{},\"lifeSeq\":{}}}",
                    actor.player_id,
                    json_metric_u32(actor.life_seq),
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        snapshots.push(format!(
            "{{\"epoch\":{},\"serial\":{},\"latestSnapshotId\":{},\"actors\":[{}]}}",
            membership.epoch, membership.serial, membership.snapshot_id, actors,
        ));
    }
    if let Some(latest) = game.network_world.authoritative_snapshot_actor_trace.back() {
        game.network_world
            .authoritative_snapshot_actor_metrics_cursor
            .set(Some((latest.epoch, latest.serial)));
    }
    format!("[{}]", snapshots.join(","))
}

fn authoritative_snapshot_actor_stream_metrics_json(game: &Game, frames: &str) -> String {
    format!(
        "{{\"epoch\":{},\"latestSerial\":{},\"overflow\":{},\"frames\":{}}}",
        json_metric_u32(game.network_world.snapshot_epoch()),
        game.network_world.authoritative_snapshot_actor_serial,
        game.network_world.authoritative_snapshot_actor_overflow,
        frames,
    )
}

fn remote_actor_presentation_metrics_json(game: &Game) -> String {
    let entries = game
        .network_world
        .players
        .values()
        .filter(|player| {
            Some(player.player_id) != game.client_authority.view().scope.player_id
        })
        .map(|player| {
            format!(
                "{{\"id\":{},\"lifeSeq\":{},\"snapCount\":{},\"initialCount\":{},\"lastSnapReason\":{},\"positionPresent\":{},\"renderPositionPresent\":{},\"lastSeenSnapshotId\":{},\"ship\":{},\"attachParent\":{},\"togglables\":{}}}",
                player.player_id,
                json_metric_u32(player.life_seq),
                player.presentation_snap_count,
                player.presentation_initial_count,
                player.last_presentation_snap_reason,
                player.position.is_some(),
                player.render_position.is_some(),
                json_metric_u16(player.last_seen_snapshot_id),
                player.ship,
                player.attach_parent,
                player.togglables,
            )
        })
        .collect::<Vec<_>>();
    format!("[{}]", entries.join(","))
}

/// Every HUD number that comes out of a font or sprite-sheet measurement. The
/// payload builder takes these as arguments instead of reading `Assets`, so a
/// native test - where `Assets` deliberately does not exist - can assemble the
/// exact payload the browser hands to `JSON.parse`.
pub(crate) struct HudMetricsMeasure {
    screen_width: f32,
    screen_height: f32,
    energy_inside: bool,
    energy_center_delta: f32,
    energy_digits_inside: bool,
    energy_digits_right_aligned: bool,
    hud_text_inside: bool,
    hud_radar_overlap: bool,
    radar_bottom_right: bool,
    radar_label_inside: bool,
    right_status_panel_inside: bool,
    right_status_panel_clear_of_radar: bool,
    left_item_panel_inside: bool,
    label_inside: bool,
    label_scale: f32,
    label_width: f32,
    notification_inside: bool,
    notification_scale: f32,
    notification_width: f32,
    text_positive_scale: bool,
}
