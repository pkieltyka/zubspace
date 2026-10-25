use super::*;

/// The step gate with its inputs lifted out, so the transport-terminal term is
/// testable on native builds, where `browser_rtc_transport_is_terminal()` is
/// always false. A transport that reached open and then died is a terminal
/// session: the client must stop stepping rather than keep predicting against
/// a peer connection that can no longer send or receive, and readiness metrics
/// must stop claiming a healthy gameplay step while that is true.
pub(crate) fn gameplay_step_authority_ready(
    networked_play: bool,
    transport_terminal: bool,
    scope_present: bool,
    arena_ready: bool,
) -> bool {
    !networked_play || (!transport_terminal && scope_present && arena_ready)
}

#[cfg(test)]
mod transport_terminal_gate_tests {
    use super::gameplay_step_authority_ready;

    #[test]
    fn a_terminal_transport_closes_the_step_gate_even_with_a_live_scope() {
        // Before recovery lands this is exactly the stalled client's state: a
        // scope and arena readiness still held from a session whose peer
        // connection is gone. The gate must read false, or readiness metrics
        // keep claiming a healthy step on a dead socket.
        assert!(!gameplay_step_authority_ready(true, true, true, true));
        assert!(!gameplay_step_authority_ready(true, true, false, true));
    }

    #[test]
    fn a_live_or_offline_session_is_unaffected_by_the_terminal_term() {
        assert!(gameplay_step_authority_ready(true, false, true, true));
        assert!(!gameplay_step_authority_ready(true, false, false, true));
        assert!(!gameplay_step_authority_ready(true, false, true, false));
        // Non-networked (offline) play never consults a transport at all.
        assert!(gameplay_step_authority_ready(false, true, false, false));
    }
}

fn movement_correction_barrier_retains_action(kind: SemanticInputActionKind) -> bool {
    matches!(
        kind,
        SemanticInputActionKind::ChatAutocompleteOrBomb
            | SemanticInputActionKind::PrimaryFireOrSpectate
            | SemanticInputActionKind::BombOrSpectate
            | SemanticInputActionKind::Mine
            | SemanticInputActionKind::Repel
            | SemanticInputActionKind::RepelOrMenuQuit
            | SemanticInputActionKind::Burst
            | SemanticInputActionKind::ToggleMultifire
            | SemanticInputActionKind::ToggleCloak
            | SemanticInputActionKind::ToggleStealth
            | SemanticInputActionKind::ToggleAntiwarp
            | SemanticInputActionKind::ToggleXradar
            | SemanticInputActionKind::PlacePortal
            | SemanticInputActionKind::Warp
            | SemanticInputActionKind::RandomWarp
            | SemanticInputActionKind::Rocket
            | SemanticInputActionKind::Brick
            | SemanticInputActionKind::Decoy
            | SemanticInputActionKind::Thor
            | SemanticInputActionKind::Attach
    )
}

fn combat_view_barrier_retains_batch(actions: &[SemanticInputAction]) -> bool {
    actions
        .iter()
        .any(|action| semantic_action_requires_weapon_submission(action.kind))
}

#[cfg(test)]
mod correction_barrier_tests {
    use super::*;
    use crate::game::lifecycle_sequencer::{
        ClientAuthorityDisposition, ClientAuthorityPhase, ClientAuthorityReadiness,
        ClientAuthoritySource,
    };
    use crate::protocol::AuthorityScopeIdentity;

    #[test]
    fn correction_barrier_retains_only_gameplay_edges() {
        assert!(movement_correction_barrier_retains_action(
            SemanticInputActionKind::PrimaryFireOrSpectate
        ));
        assert!(movement_correction_barrier_retains_action(
            SemanticInputActionKind::Rocket
        ));
        assert!(!movement_correction_barrier_retains_action(
            SemanticInputActionKind::ResetShip
        ));
        assert!(!movement_correction_barrier_retains_action(
            SemanticInputActionKind::SelectShip(1)
        ));
    }

    #[test]
    fn combat_view_barrier_keeps_ordered_weapon_chords_atomic() {
        let action = |kind| SemanticInputAction {
            kind,
            modifiers: crate::input::InputModifiers::default(),
        };
        assert!(combat_view_barrier_retains_batch(&[
            action(SemanticInputActionKind::Rocket),
            action(SemanticInputActionKind::BombOrSpectate),
        ]));
        assert!(!combat_view_barrier_retains_batch(&[action(
            SemanticInputActionKind::Rocket,
        )]));
    }

    #[test]
    fn observer_presentation_ages_weapon_trails_only_once_per_rendered_frame() {
        let mut game = Game::new_network_authority_test();
        game.bullet_trails.push(BulletTrail {
            pos: Vec2::ZERO,
            age: 0.0,
            level: 1,
            bouncing: false,
            remote_source: None,
        });
        game.update_observer_presentation(0.05);
        game.update_remote_presentation(0.05);
        assert!((game.bullet_trails[0].age - 0.05).abs() < 0.00001);
    }

    #[test]
    fn dead_network_life_advances_respawn_eligibility_while_gameplay_is_closed() {
        let mut game = Game::new_network_authority_test();
        let live = AuthorityScopeIdentity {
            server_session_id: Some(7),
            arena_stream_id: Some(11),
            player_id: Some(23),
            life_seq: Some(41),
            alive: Some(true),
        };
        game.client_authority.install_test_scope(
            1,
            live,
            ClientAuthorityReadiness {
                joined: true,
                arena_epoch: Some(13),
                settings: true,
                security: true,
                map: true,
                files: true,
            },
        );
        game.networked_play = true;
        game.network_prize_baseline_pending = false;

        let dead = AuthorityScopeIdentity {
            life_seq: Some(42),
            alive: Some(false),
            ..live
        };
        assert_eq!(
            game.client_authority.observe_private_lifecycle(
                ClientAuthoritySource::Reliable,
                dead,
                Some(500),
                0.05,
            ),
            ClientAuthorityDisposition::Replacement,
        );
        game.start_player_respawn_with_delay(vec2(8_200.0, 8_200.0), 0.05);
        assert!(!game.local_gameplay_ready());

        game.advance_respawn_while_gameplay_unready(0.05);

        assert_eq!(game.player_respawn_timer, 0.0);
        assert_eq!(
            game.client_authority.view().phase,
            ClientAuthorityPhase::AwaitingDurableSubmission,
        );
        assert!(
            game.client_authority
                .view()
                .lifecycle_baseline_ready_to_submit
        );
    }
}

impl Game {
    /// Freeze the receipt for a press-time weapon edge at the render/input
    /// boundary. A high-refresh frame may execute no 60 Hz step; preparing
    /// here prevents that queued one-shot from being reclassified against a
    /// different drawn view on the following frame.
    pub(crate) fn prepare_pending_semantic_weapon_receipt(
        &mut self,
        render_input: &RenderInputContext,
        logical_steps: u8,
    ) {
        if self.networked_play && render_input.has_pending_weapon_submission_action() {
            // One-shot actions drain on the final catch-up step so their
            // weapon field and position publish atomically. Zero-step frames
            // retain the current sequence; N-step frames reserve N-1 ahead.
            let action_input_sequence = self
                .next_input_seq
                .wrapping_add(u16::from(logical_steps.saturating_sub(1)));
            let receipt = self.prepare_combat_view_receipt_for_input(action_input_sequence);
            let server_tick = self.browser_server_tick();
            let applied = self.observer_combat_timeline.latest_game_applied_snapshot();
            publish_weapon_admission_event(&format!(
                "{{\"phase\":\"prepared\",\"inputSequence\":{},\"logicalSteps\":{},\"ready\":{},\"observerLifeSeq\":{},\"timelineGeneration\":{},\"snapshotId\":{},\"holdOrStall\":{},\"browserServerTick\":{},\"scheduledRenderTick\":{},\"presentationRenderTick\":{},\"gameAppliedServerTick\":{}}}",
                action_input_sequence,
                logical_steps,
                receipt.weapon_submission_ready(),
                receipt.observer_life_seq,
                receipt.timeline_generation,
                receipt.latest_game_applied_snapshot_id,
                receipt.hold_or_stall(),
                server_tick.map_or_else(|| "null".to_owned(), |value| value.to_string()),
                self.observer_combat_timeline
                    .presentation_scheduled_tick()
                    .map_or_else(|| "null".to_owned(), |value| value.to_string()),
                self.observer_combat_timeline
                    .presentation_render_tick()
                    .map_or_else(|| "null".to_owned(), |value| value.to_string()),
                applied
                    .map(|(_, server_tick)| server_tick)
                    .map_or_else(|| "null".to_owned(), |value| value.to_string()),
            ));
        }
    }

    pub(in crate::game) fn drain_actions_for_gameplay_step(
        render_input: &mut RenderInputContext,
        final_catch_up_step: bool,
    ) -> Vec<SemanticInputAction> {
        if final_catch_up_step {
            render_input.drain_actions()
        } else {
            Vec::new()
        }
    }

    fn capture_chat_characters(&mut self) {
        while let Some(ch) = get_char_pressed() {
            // Miniquad queues printable keypresses independently of key-down
            // actions. Always drain that queue so gameplay hotkeys cannot be
            // replayed into a later chat session.
            if self.chat.active {
                self.chat.push_char(ch);
            }
        }
    }

    fn submit_chat_input(&mut self) {
        self.submit_chat_input_with_gameplay_ready(true);
    }

    fn submit_chat_input_with_gameplay_ready(&mut self, gameplay_ready: bool) {
        let Some(message) = self.chat.submit_input() else {
            return;
        };
        match parse_audio_volume_command(&message) {
            Ok(Some(command)) => {
                let notification = self.apply_audio_volume_command(command.category, command.value);
                self.push_notification(&notification, GREEN);
            }
            Ok(None) => {
                if let Some(command) = crate::chat::parse_network_info_command(&message) {
                    let stats = crate::browser_metrics::connection_ping_stats();
                    for line in crate::chat::network_info_messages(command, stats) {
                        self.chat.push_message(line, TextColor::Green);
                    }
                } else if parse_status_command(&message) {
                    let status = self.local_status_message();
                    self.chat.push_message(status, TextColor::Green);
                } else {
                    match parse_frequency_change_input(&message) {
                        Ok(Some(frequency)) if gameplay_ready => self.change_frequency(frequency),
                        Ok(Some(_)) => self.push_notification("Gameplay is not ready.", ORANGE),
                        Ok(None) => {
                            match parse_local_chat_input_with_resolver(&message, |target| {
                                self.resolve_private_chat_target(target)
                            }) {
                                Ok(chat) => {
                                    // Online messages are displayed once, on the server echo.
                                    // Equal consecutive texts are independent messages, not deduplicated.
                                    if !self.networked_play {
                                        self.chat.push_message(
                                            format_network_chat_message(
                                                chat.request.chat_type,
                                                Some(&self.local_player_name),
                                                0,
                                                &chat.request.message,
                                            ),
                                            TextColor::Blue,
                                        );
                                    }
                                    self.send_local_chat_message(&chat.request);
                                }
                                Err(err) => self.push_notification(err, RED),
                            }
                        }
                        Err(err) => self.push_notification(err, RED),
                    }
                }
            }
            Err(err) => self.push_notification(err, RED),
        }
    }

    /// Apply queued press-time actions in chronological order. Lifecycle
    /// boundaries discard already-buffered ship actions and suppress later
    /// ones from the same render batch so an action captured for one
    /// life/session cannot leak into its replacement.
    fn request_main_menu(&mut self) {
        if self.main_menu_requested {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        if let Err(err) = request_browser_main_menu() {
            self.push_notification(&err, RED);
            return;
        }
        self.main_menu_requested = true;
        self.push_notification("Leaving arena.", ORANGE);
    }

    fn process_semantic_actions(
        &mut self,
        actions: Vec<SemanticInputAction>,
    ) -> Vec<SemanticInputAction> {
        if self.scoreboard_visible {
            return Vec::new();
        }
        let mut gameplay_actions = Vec::new();
        let mut crossed_session_boundary = false;
        for mut action in actions {
            if self.chat.active {
                match action.kind {
                    SemanticInputActionKind::ChatBackspace => self.chat.backspace(),
                    SemanticInputActionKind::ChatAutocompleteOrBomb => {
                        match self.autocomplete_private_chat_target() {
                            Ok(Some(input)) => self.chat.input = input,
                            Ok(None) => {}
                            Err(err) => self.push_notification(err, ORANGE),
                        }
                    }
                    SemanticInputActionKind::ChatHistoryPrevious => self.chat.recall_previous(),
                    SemanticInputActionKind::ChatHistoryNext => self.chat.recall_next(),
                    SemanticInputActionKind::CancelChatOrToggleMenu => self.chat.cancel_input(),
                    SemanticInputActionKind::ChatEnter => self.submit_chat_input(),
                    _ => {}
                }
                continue;
            }

            if let SemanticInputActionKind::NumberShortcut(number) = action.kind {
                if self.menu_visible {
                    if !(1..=8).contains(&number) {
                        continue;
                    }
                    action.kind = SemanticInputActionKind::SelectShip(number - 1);
                } else {
                    action.kind = crate::input::number_gameplay_action(number, action.modifiers);
                    if action.kind == SemanticInputActionKind::UseOrPlacePortal {
                        if !self.pending_portal_placements.is_empty() {
                            continue;
                        }
                        action.kind = if self.active_portal().is_some() {
                            SemanticInputActionKind::Warp
                        } else {
                            SemanticInputActionKind::PlacePortal
                        };
                    }
                }
            }
            if self.main_menu_requested {
                continue;
            }
            if self.menu_visible {
                gameplay_actions.clear();
                crossed_session_boundary = true;
                if action.kind == SemanticInputActionKind::RepelOrMenuQuit {
                    self.request_main_menu();
                    continue;
                }
                if !matches!(
                    action.kind,
                    SemanticInputActionKind::CancelChatOrToggleMenu
                        | SemanticInputActionKind::ChatEnter
                        | SemanticInputActionKind::ToggleSpectator
                        | SemanticInputActionKind::ToggleArenaDirectory
                        | SemanticInputActionKind::ToggleDeveloperOverlay
                        | SemanticInputActionKind::SelectShip(_)
                        | SemanticInputActionKind::MenuControls
                        | SemanticInputActionKind::MenuFullscreen
                        | SemanticInputActionKind::MenuEnterSpectator
                        | SemanticInputActionKind::MenuToggleArenaDirectory
                        | SemanticInputActionKind::MenuFrequencyStep(_)
                ) {
                    continue;
                }
            } else if action.kind == SemanticInputActionKind::RepelOrMenuQuit {
                action.kind = SemanticInputActionKind::Repel;
            }
            let mut boundary = false;
            let handled = match action.kind {
                SemanticInputActionKind::ChatEnter => {
                    self.menu_visible = false;
                    self.chat.begin_input();
                    true
                }
                SemanticInputActionKind::ChatBackspace
                | SemanticInputActionKind::ChatHistoryPrevious
                | SemanticInputActionKind::ChatHistoryNext => true,
                SemanticInputActionKind::CancelChatOrToggleMenu => {
                    self.menu_visible = !self.menu_visible;
                    if self.menu_visible {
                        self.client_authority.discard_gameplay_actions_for_menu();
                    }
                    boundary = true;
                    true
                }
                SemanticInputActionKind::ResetShip => {
                    self.reset_ship();
                    boundary = true;
                    true
                }
                SemanticInputActionKind::ToggleSpectator if self.menu_visible => {
                    self.toggle_spectator_mode();
                    self.menu_visible = false;
                    boundary = true;
                    true
                }
                SemanticInputActionKind::ToggleArenaDirectory => true,
                SemanticInputActionKind::ToggleDeveloperOverlay => {
                    if crate::browser_metrics::debug_mode() {
                        self.developer_overlay = !self.developer_overlay;
                    }
                    true
                }
                SemanticInputActionKind::MenuControls if self.menu_visible => {
                    crate::browser_metrics::show_controls();
                    true
                }
                SemanticInputActionKind::MenuFullscreen if self.menu_visible => {
                    self.menu_visible = false;
                    if !crate::browser_metrics::toggle_fullscreen() {
                        self.push_notification(
                            "Fullscreen is unavailable in this browser.",
                            ORANGE,
                        );
                    }
                    boundary = true;
                    true
                }
                SemanticInputActionKind::SelectShip(ship_type) if self.menu_visible => {
                    self.change_ship(ship_type as usize);
                    self.menu_visible = false;
                    boundary = true;
                    true
                }
                SemanticInputActionKind::MenuEnterSpectator if self.menu_visible => {
                    self.enter_spectator_mode();
                    self.menu_visible = false;
                    boundary = true;
                    true
                }
                SemanticInputActionKind::MenuToggleArenaDirectory if self.menu_visible => {
                    self.arena_directory_visible = !self.arena_directory_visible;
                    self.menu_visible = false;
                    true
                }
                SemanticInputActionKind::MenuFrequencyStep(step) if self.menu_visible => {
                    let _ = step;
                    self.push_notification(
                        "Free-for-all: frequencies are assigned by the server.",
                        ORANGE,
                    );
                    self.menu_visible = false;
                    boundary = true;
                    true
                }
                SemanticInputActionKind::ChangeMap(step) => {
                    self.change_map(i32::from(step));
                    boundary = true;
                    true
                }
                SemanticInputActionKind::ChangeZone(step) => {
                    self.change_zone(i32::from(step));
                    boundary = true;
                    true
                }
                SemanticInputActionKind::ToggleSpectator
                | SemanticInputActionKind::SelectShip(_)
                | SemanticInputActionKind::MenuControls
                | SemanticInputActionKind::MenuFullscreen
                | SemanticInputActionKind::MenuEnterSpectator
                | SemanticInputActionKind::MenuToggleArenaDirectory
                | SemanticInputActionKind::MenuFrequencyStep(_) => true,
                _ => false,
            };
            if boundary {
                gameplay_actions.clear();
                crossed_session_boundary = true;
            } else if !handled && !crossed_session_boundary {
                gameplay_actions.push(action);
            }
        }
        gameplay_actions
    }

    fn resolve_private_chat_target(&self, target: &str) -> Result<u16, &'static str> {
        let needle = target.trim();
        if needle.is_empty() {
            return Err("private chat needs a target");
        }

        let needle_lower = needle.to_ascii_lowercase();
        let mut prefix_match = None;
        for player in self.network_world.players.values() {
            let name_lower = player.name.to_ascii_lowercase();
            if name_lower == needle_lower {
                return Ok(player.player_id);
            }
            if name_lower.starts_with(&needle_lower) {
                if prefix_match.is_some() {
                    return Err("private chat player name is ambiguous");
                }
                prefix_match = Some(player.player_id);
            }
        }

        prefix_match.ok_or("private chat player not found")
    }

    fn autocomplete_private_chat_target(&self) -> Result<Option<String>, &'static str> {
        autocomplete_private_chat_target(
            &self.chat.input,
            self.network_world
                .players
                .values()
                .map(|player| player.name.as_str()),
        )
    }

    pub(crate) fn update(
        &mut self,
        dt: f32,
        emit_network_position: bool,
        render_input: &mut RenderInputContext,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = emit_network_position;
        self.capture_chat_characters();
        // Browser input is sampled once per rendered frame, while a slow
        // frame can execute several fixed gameplay steps. Network position
        // publication is intentionally reserved for the final catch-up step.
        // Drain one-shot actions on that same step so a weapon created by N
        // cannot be held until N+1 and accidentally share the Rocket pulse
        // that is defined to begin after the weapon/action phase.
        let mut actions =
            Self::drain_actions_for_gameplay_step(render_input, emit_network_position);
        if emit_network_position {
            let mut retained = self.client_authority.take_released_correction_actions(true);
            retained.append(&mut actions);
            actions = retained;
        }
        let mut actions = self.process_semantic_actions(actions);
        let weapon_submission_receipt = if self.networked_play {
            self.prepare_combat_view_receipt_for_input(self.next_input_seq)
        } else {
            crate::combat_timeline::CombatViewReceiptV1::default()
        };
        let weapon_submission_ready =
            !self.networked_play || weapon_submission_receipt.weapon_submission_ready();
        if emit_network_position {
            let mut released = self
                .client_authority
                .take_combat_view_actions(weapon_submission_ready);
            if weapon_submission_ready {
                released.append(&mut actions);
                actions = released;
            } else if combat_view_barrier_retains_batch(&actions) {
                // The canonical action order is one physical gameplay batch:
                // e.g. F3 must activate Rocket immediately before X fires its
                // bomb. If any member needs the drawn combat-view receipt,
                // retain the whole ordered batch instead of allowing its
                // non-projectile prefix to run on an earlier input sequence.
                let _ = self.client_authority.retain_combat_view_actions(actions);
                actions = Vec::new();
            }
        }
        if actions
            .iter()
            .any(|action| semantic_action_requires_weapon_submission(action.kind))
        {
            let kinds = actions
                .iter()
                .filter(|action| semantic_action_requires_weapon_submission(action.kind))
                .map(|action| format!("{:?}", action.kind))
                .collect::<Vec<_>>();
            publish_weapon_admission_event(&format!(
                "{{\"phase\":\"consumed\",\"inputSequence\":{},\"actions\":{:?},\"ready\":{},\"observerLifeSeq\":{},\"timelineGeneration\":{},\"snapshotId\":{},\"holdOrStall\":{},\"browserServerTick\":{},\"scheduledRenderTick\":{},\"presentationRenderTick\":{},\"gameAppliedServerTick\":{}}}",
                self.next_input_seq,
                kinds,
                weapon_submission_ready,
                weapon_submission_receipt.observer_life_seq,
                weapon_submission_receipt.timeline_generation,
                weapon_submission_receipt.latest_game_applied_snapshot_id,
                weapon_submission_receipt.hold_or_stall(),
                self.browser_server_tick()
                    .map_or_else(|| "null".to_owned(), |value| value.to_string()),
                self.observer_combat_timeline
                    .presentation_scheduled_tick()
                    .map_or_else(|| "null".to_owned(), |value| value.to_string()),
                self.observer_combat_timeline
                    .presentation_render_tick()
                    .map_or_else(|| "null".to_owned(), |value| value.to_string()),
                self.observer_combat_timeline
                    .latest_game_applied_snapshot()
                    .map(|(_, server_tick)| server_tick)
                    .map_or_else(|| "null".to_owned(), |value| value.to_string()),
            ));
        }
        self.update_notifications(dt);
        #[cfg(target_arch = "wasm32")]
        self.update_voice_audio(dt);
        self.update_energy_alarm(dt);
        let personal_koth_ended = self.network_world.update_koth_timers(dt);
        self.network_world.update_powerballs(dt);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = personal_koth_ended;
        #[cfg(target_arch = "wasm32")]
        if personal_koth_ended {
            self.send_network_end_koth_timer();
        }

        let gameplay_input_enabled = !self.chat.active
            && !self.menu_visible
            && !self.scoreboard_visible
            && !self.main_menu_requested;

        if self.update_player_respawn(dt) {
            // Respawning ships keep world effects ticking but cannot steer or fire.
        } else if self.spectator_mode {
            if gameplay_input_enabled {
                self.update_spectator(dt, render_input, &actions);
            } else {
                self.thrust_sound_started = false;
                self.exhaust_timer = 0.0;
            }
        } else {
            self.update_ship(
                dt,
                render_input,
                &actions,
                emit_network_position,
                weapon_submission_ready,
            );
        }

        self.network_world.lvz.update(dt, &self.lvz_render_cache);
        self.remove_safe_zone_owner_weapons();
        self.update_bullets(dt);
        self.update_bullet_trails(dt);
        self.update_bullet_impacts(dt);
        self.update_bomb_flashes(dt);
        self.update_bombs(dt);
        self.update_bomb_trails(dt);
        self.update_mines(dt);
        self.update_shrapnel(dt);
        self.update_thors(dt);
        self.update_remote_projectiles(dt);
        self.update_bricks(dt);
        self.update_decoys(dt);
        self.update_repels(dt);
        self.update_warps(dt);
        self.update_portal(dt);
        self.update_timed_prize_statuses(dt);
        self.update_explosions(dt);
        self.update_burst_effects(dt);
        self.update_exhausts(dt);
        self.update_rocket_particles(dt);
        self.update_bots(dt);
        self.update_bot_bullets(dt);
        self.update_prizes(dt);
        self.update_powerball(dt);
        self.update_powerball_trails(dt);
        self.update_flags(dt);
        #[cfg(target_arch = "wasm32")]
        self.update_browser_network(dt, emit_network_position);

        self.world_effects_advanced_before_presentation = true;
        self.sync_player_camera();
    }

    /// Keep transport/security and explicitly non-gameplay UI alive while the
    /// local authority gate is closed. A movement-correction barrier retains
    /// bounded press-time gameplay edges until its terminal outcome; all
    /// other readiness gates continue to discard them fail-closed.
    pub(crate) fn update_while_gameplay_unready(
        &mut self,
        render_dt: f32,
        render_input: &mut RenderInputContext,
    ) {
        self.advance_respawn_while_gameplay_unready(render_dt);
        self.capture_chat_characters();
        let movement_correction_barrier = self.client_authority.view().transition_kind
            == Some(ClientAuthorityTransitionKind::Correction);
        let observer_ready = self.observer_controls_ready();
        let pending_actions = render_input.drain_actions();
        if observer_ready {
            let observer_dt = bounded_presentation_step_seconds(render_dt);
            let observer_actions = self.process_semantic_actions(pending_actions);
            if self.spectator_mode
                && !self.menu_visible
                && !self.chat.active
                && !self.scoreboard_visible
                && !self.main_menu_requested
            {
                self.update_spectator(observer_dt, render_input, &observer_actions);
            } else if self.spectator_mode && self.spectating_player_id.is_some() {
                // Menus/chat own the keys while the followed scene stays live.
                self.update_spectator(observer_dt, &RenderInputContext::default(), &[]);
            }
            if self.spectator_mode {
                self.update_observer_presentation(observer_dt);
            }
            #[cfg(target_arch = "wasm32")]
            if self.spectator_mode {
                self.send_browser_observer_view(render_dt, None);
            }
        } else {
            for mut action in pending_actions {
                if self.chat.active {
                    match action.kind {
                        SemanticInputActionKind::ChatBackspace => self.chat.backspace(),
                        SemanticInputActionKind::ChatAutocompleteOrBomb => {
                            match self.autocomplete_private_chat_target() {
                                Ok(Some(input)) => self.chat.input = input,
                                Ok(None) => {}
                                Err(err) => self.push_notification(err, ORANGE),
                            }
                        }
                        SemanticInputActionKind::ChatHistoryPrevious => self.chat.recall_previous(),
                        SemanticInputActionKind::ChatHistoryNext => self.chat.recall_next(),
                        SemanticInputActionKind::CancelChatOrToggleMenu => self.chat.cancel_input(),
                        SemanticInputActionKind::ChatEnter => {
                            self.submit_chat_input_with_gameplay_ready(false)
                        }
                        _ => {}
                    }
                    continue;
                }
                #[cfg(any(test, target_arch = "wasm32"))]
                if self.retain_unready_ship_return_action(action.kind) {
                    continue;
                }
                if let SemanticInputActionKind::NumberShortcut(number) = action.kind {
                    if self.menu_visible {
                        continue;
                    }
                    action.kind = crate::input::number_gameplay_action(number, action.modifiers);
                    if action.kind == SemanticInputActionKind::UseOrPlacePortal {
                        if !self.pending_portal_placements.is_empty() {
                            continue;
                        }
                        action.kind = if self.active_portal().is_some() {
                            SemanticInputActionKind::Warp
                        } else {
                            SemanticInputActionKind::PlacePortal
                        };
                    }
                }
                if self.main_menu_requested {
                    continue;
                }
                if self.menu_visible {
                    if action.kind == SemanticInputActionKind::RepelOrMenuQuit {
                        self.request_main_menu();
                        continue;
                    }
                    if !matches!(
                        action.kind,
                        SemanticInputActionKind::CancelChatOrToggleMenu
                            | SemanticInputActionKind::ChatEnter
                            | SemanticInputActionKind::ToggleArenaDirectory
                            | SemanticInputActionKind::ToggleDeveloperOverlay
                            | SemanticInputActionKind::MenuControls
                            | SemanticInputActionKind::MenuFullscreen
                    ) {
                        continue;
                    }
                } else if action.kind == SemanticInputActionKind::RepelOrMenuQuit {
                    action.kind = SemanticInputActionKind::Repel;
                }
                match action.kind {
                    SemanticInputActionKind::ChatEnter => {
                        self.menu_visible = false;
                        self.chat.begin_input();
                    }
                    SemanticInputActionKind::CancelChatOrToggleMenu => {
                        self.menu_visible = !self.menu_visible;
                        if self.menu_visible {
                            self.client_authority.discard_gameplay_actions_for_menu();
                        }
                    }
                    SemanticInputActionKind::MenuControls if self.menu_visible => {
                        crate::browser_metrics::show_controls();
                    }
                    SemanticInputActionKind::MenuFullscreen if self.menu_visible => {
                        self.menu_visible = false;
                        if !crate::browser_metrics::toggle_fullscreen() {
                            self.push_notification(
                                "Fullscreen is unavailable in this browser.",
                                ORANGE,
                            );
                        }
                    }
                    SemanticInputActionKind::ToggleArenaDirectory => {
                        self.arena_directory_visible = !self.arena_directory_visible;
                    }
                    SemanticInputActionKind::ToggleDeveloperOverlay => {
                        if crate::browser_metrics::debug_mode() {
                            self.developer_overlay = !self.developer_overlay;
                        }
                    }
                    _ if movement_correction_barrier
                        && movement_correction_barrier_retains_action(action.kind) =>
                    {
                        let _ = self.client_authority.retain_correction_action(action);
                    }
                    _ => {}
                }
            }
        }
        render_input.finish_gameplay_batch();
        // Every semantic edge handled on an unready frame was either retained
        // by its typed lifecycle/correction owner or deliberately discarded.
        // Its press-time combat-view receipt must not survive to a later edge
        // or become eligible after a successor scope opens.
        self.prepared_combat_view_receipt = None;
        self.update_notifications(render_dt);
        #[cfg(target_arch = "wasm32")]
        {
            self.update_voice_audio(render_dt);
            self.update_browser_network(render_dt, false);
            self.reconcile_authoritative_portal();
            self.suppress_gameplay_step_after_network_rebase = false;
        }
        // Rendering is intentionally live while the authoritative gameplay
        // gate is closed. Keep the same pixel-space camera contract as the
        // ready simulation path so the map and embedded starfield do not
        // disappear during RTC setup, zone bootstrap, or recovery barriers.
        self.sync_player_camera();
    }

    /// A dead authoritative life deliberately closes the gameplay gate, so
    /// its server-owned respawn wait must advance on the render/network path.
    /// Once eligible, the sequencer exposes only the neutral successor-life
    /// baseline; gameplay remains closed until its application receipt.
    fn advance_respawn_while_gameplay_unready(&mut self, render_dt: f32) {
        if self.networked_play && self.player_respawn_timer > 0.0 {
            let _ = self.update_player_respawn(render_dt);
        }
    }

    fn sync_player_camera(&mut self) {
        self.camera.target = self.local_render_camera_target();
        self.camera.zoom = player_camera_zoom(screen_width(), screen_height());
    }

    /// Canonical steps require tuning for this exact life. A wait within an
    /// already installed live loadout can keep motion running, but a lifecycle
    /// receipt cannot make arena reset defaults authoritative while the new
    /// private loadout is still in flight. Network polling continues while
    /// this gate is closed, so that private prerequisite can always arrive.
    pub(crate) fn step_authority_ready(&self) -> bool {
        let scope = self.client_authority.view().scope;
        let loadout_installed = !self.network_prize_baseline_pending
            || (scope.life_seq.is_some() && self.last_self_server_prize_life_seq == scope.life_seq);
        gameplay_step_authority_ready(
            self.networked_play,
            crate::transport::browser_rtc_transport_is_terminal(),
            self.client_authority.view().gameplay_scope().is_some(),
            crate::game::network::arena_gameplay_ready(
                self.client_authority.view().readiness.joined,
                self.client_authority.view().readiness.security,
                self.door_clock_fresh,
                self.client_authority.view().readiness.map,
                self.client_authority.view().readiness.files,
            ),
        ) && loadout_installed
    }

    /// The full gameplay gate, prize baseline included. No production path consults
    /// it after this item: the frame step uses `gameplay_step_ready`, and the prize
    /// wait itself is enforced at the network boundary. It stays because the cutover
    /// tests assert that whole predicate, and because a production caller should have
    /// to think about which half it means rather than reach for this one.
    #[cfg(test)]
    pub(crate) fn local_gameplay_ready(&self) -> bool {
        !self.network_prize_baseline_pending && self.step_authority_ready()
    }

    pub(crate) fn pre_step_network_rebase_suppressed(&self) -> bool {
        self.suppress_gameplay_step_after_network_rebase
    }

    /// The single predicate the frame loop consults to decide whether the
    /// logical step runs, and the value the UI metrics publish as
    /// `localGameplayState.gameplayStepReady`. Both must remain one expression:
    /// a retained Chrome/Extreme capture showed 64 published frames and one
    /// input sequence across 1.1 s of wall time, and only a field that reports
    /// exactly what the scheduler consulted can attribute a freeze like that
    /// after the fact.
    pub(crate) fn gameplay_step_ready(&self) -> bool {
        self.step_authority_ready() && !self.pre_step_network_rebase_suppressed()
    }

    pub(crate) fn observer_controls_ready(&self) -> bool {
        self.spectator_mode
            && self.client_authority.view().observer_scope().is_some()
            && !crate::transport::browser_rtc_transport_is_terminal()
            && self.door_clock_fresh
    }

    /// Reveal only a scene whose actual input path is ready. An admitted
    /// spectator has camera controls without a live ship baseline.
    pub(crate) fn initial_presentation_ready(&self) -> bool {
        if self.networked_play && self.spectator_mode {
            self.observer_controls_ready()
        } else {
            self.gameplay_step_ready()
        }
    }

    /// Advance render-only remote proxies once per browser frame. Keeping
    /// this outside the authoritative fixed-step loop prevents an N-step
    /// scheduler catch-up from becoming one N-step visible lurch.
    pub(crate) fn update_remote_presentation(&mut self, render_dt: f32) {
        // Dead/cutover lives cannot run ship steps, but received weapons still
        // emit trails. Their cosmetic clocks must keep expiring. A fixed step
        // or ready observer may already have advanced them in this frame.
        if !self.gameplay_step_ready() && !self.world_effects_advanced_before_presentation {
            self.advance_waiting_visual_effects(render_dt);
        }
        self.world_effects_advanced_before_presentation = false;
        self.update_remote_weapons_at_server_time();
        self.update_decoys_at_server_time();
        let directed_render_tick = self.browser_server_tick().and_then(|server_tick| {
            self.observer_combat_timeline
                .advance_presentation_render_tick(server_tick, render_dt)
        });
        self.network_world
            .update_remote_players(render_dt, directed_render_tick);
        if let Some(tick) = directed_render_tick {
            self.remote_weapon_view_history.prune(tick);
        }
        // The reference game updates the spectator view after player positions. The
        // camera must consume the same presentation sample as ship/name draw,
        // rather than the sample copied during the preceding fixed update.
        self.refresh_spectator_follow_position();
        self.prepare_remote_weapon_views(directed_render_tick);
    }

    fn local_status_message(&self) -> String {
        let tuning = self.ship_tuning();
        status_command_message(
            ship_status_percent(
                tuning.recharge_upgrade_step,
                tuning.energy_recharge_max,
                self.effective_energy_recharge(),
            ),
            ship_status_percent(
                tuning.thrust_upgrade_step,
                tuning.thrust_max_acceleration,
                self.effective_thrust_acceleration(),
            ),
            ship_status_percent(
                tuning.top_speed_upgrade_step,
                tuning.top_speed_max,
                self.effective_top_speed(),
            ),
            ship_status_percent(
                tuning.rotation_upgrade_step,
                tuning.rotation_max_per_second,
                self.effective_rotation_per_second(),
            ),
            shrapnel_count(self.ship.shrapnel_upgrades, tuning.shrapnel_max),
        )
    }

    pub(super) fn ship_change_energy_ready(&mut self) -> bool {
        if !self.spectator_mode
            && (self.player_respawn_timer > 0.0 || self.ship.energy < self.ship.max_energy)
        {
            self.push_notification("Must have full energy to change ships or spectate.", ORANGE);
            return false;
        }
        true
    }

    #[cfg(any(test, target_arch = "wasm32"))]
    fn retain_unready_ship_return_action(&mut self, kind: SemanticInputActionKind) -> bool {
        if !self.networked_play
            || !self.menu_visible
            || self.main_menu_requested
            || !matches!(
                self.client_authority.view().phase,
                ClientAuthorityPhase::Disconnected | ClientAuthorityPhase::Bootstrapping
            )
        {
            return false;
        }
        let ship = match kind {
            SemanticInputActionKind::NumberShortcut(number) if (1..=8).contains(&number) => {
                number - 1
            }
            SemanticInputActionKind::SelectShip(ship) if ship < 8 => ship,
            _ => return false,
        };
        self.change_ship(usize::from(ship));
        self.menu_visible = false;
        true
    }

    pub(super) fn change_ship(&mut self, ship_type: usize) {
        let ship_type = ship_type.min(7);
        #[cfg(any(test, target_arch = "wasm32"))]
        if self.networked_play
            && matches!(
                self.client_authority.view().phase,
                ClientAuthorityPhase::Disconnected | ClientAuthorityPhase::Bootstrapping
            )
        {
            // A hidden tab can return after the server retired its application
            // session while the browser still reports an open RTC transport.
            // No request/rejection can start recovery in that retired scope.
            // Retain this genuine menu intent before the same-ship shortcut or
            // stale energy checks. Fresh normal admission still enforces any
            // server-owned hold; this grants no playable state locally.
            if let Some(attempt) = self
                .quality_return_recovery
                .as_mut()
                .filter(|attempt| !attempt.is_stopped() && !attempt.is_ready())
            {
                attempt.ship = ship_type as u8;
            } else {
                let follow_existing =
                    self.client_authority.view().phase == ClientAuthorityPhase::Bootstrapping;
                #[cfg(target_arch = "wasm32")]
                let follow_existing = follow_existing
                    || self.browser_network.as_ref().is_some_and(|browser| {
                        browser.client.transport().status() == BrowserRtcStatus::Connecting
                    });
                let now = macroquad::miniquad::date::now();
                self.quality_return_recovery = if follow_existing {
                    quality_return::QualityReturnRecovery::awaiting_current_connection(
                        ship_type as u8,
                        now,
                    )
                } else {
                    quality_return::QualityReturnRecovery::new(ship_type as u8, now)
                };
            }
            return;
        }
        if self.spectator_mode {
            self.ship.ship_type = ship_type;
            self.leave_spectator_mode();
            if !self.networked_play {
                self.reset_ship();
            }
            return;
        }
        if self.ship.ship_type == ship_type
            || self.client_authority.view().lifecycle_intent
                == Some(PendingLifecycleRequest::Ship(ship_type as u8))
        {
            return;
        }

        if !self.ship_change_energy_ready() {
            return;
        }
        if self.networked_play {
            // Keep the old server-confirmed ship and pause its canonical
            // simulation together with realtime publication. The reliable
            // roster acknowledgement is the lifecycle cutover: only then do
            // we reset and emit at the request's sequence boundary. A rejected
            // request resumes the unchanged old life at that same boundary.
            self.send_network_ship_change(ship_type as u8);
            return;
        }

        self.ship.ship_type = ship_type;
        // A server-accepted ship change advances the life scope and resets the
        // complete loadout, including InitialBounty-derived movement stats.
        // Mirror that lifecycle locally before emitting the request; retaining
        // the previous ship's upgraded rotation here makes the honest input
        // direction witness physically impossible under the new ship tuning.
        self.reset_ship();
        self.send_network_ship_change(ship_type as u8);
    }

    pub(super) fn change_frequency(&mut self, frequency: u16) {
        if !crate::browser_metrics::frequency_changes_allowed() {
            self.push_notification(
                "Free-for-all: frequencies are assigned by the server.",
                ORANGE,
            );
            return;
        }
        if frequency >= self.tuning.max_frequency {
            self.push_notification("Frequency is out of arena range.", ORANGE);
            return;
        }
        if frequency == self.player_frequency()
            || self.client_authority.view().lifecycle_intent
                == Some(PendingLifecycleRequest::Frequency(frequency))
        {
            return;
        }
        self.send_network_frequency_change(frequency);
    }

    fn change_frequency_by(&mut self, step: i16) {
        let frequency = self.player_frequency();
        let max_frequency = self.tuning.max_frequency.saturating_sub(1);
        let next = if step < 0 {
            frequency.saturating_sub(step.unsigned_abs())
        } else {
            frequency.saturating_add(step as u16)
        };
        self.change_frequency(next.min(max_frequency));
    }

    pub(super) fn update_player_respawn(&mut self, dt: f32) -> bool {
        if self.player_respawn_timer <= 0.0 {
            return false;
        }

        if self.networked_play {
            let disposition = self.client_authority.advance_respawn_eligibility(dt);
            self.player_respawn_timer = self.client_authority.view().respawn_remaining_seconds;
            debug_assert!(matches!(
                disposition,
                ClientAuthorityDisposition::Observed | ClientAuthorityDisposition::Applied
            ));
        } else {
            self.player_respawn_timer -= dt;
        }
        self.ship.vel = Vec2::ZERO;
        self.thrust_sound_started = false;
        self.exhaust_timer = 0.0;

        if self.player_respawn_timer <= 0.0 {
            self.player_respawn_timer = 0.0;
            self.reset_ship_after_death();
            self.arm_respawn_input_boundary();
            false
        } else {
            true
        }
    }
}

fn player_camera_zoom(viewport_width: f32, viewport_height: f32) -> Vec2 {
    vec2(
        2.0 / viewport_width.max(1.0),
        2.0 / viewport_height.max(1.0),
    )
}

#[cfg(test)]
mod menu_shortcut_tests {
    use super::*;

    fn action(kind: SemanticInputActionKind) -> SemanticInputAction {
        SemanticInputAction {
            kind,
            modifiers: crate::input::InputModifiers::default(),
        }
    }

    #[test]
    fn fullscreen_only_closes_the_menu_and_cannot_release_same_batch_weapons() {
        use SemanticInputActionKind as A;
        let mut game = Game::new_network_authority_test();
        let phase = game.client_authority.view().phase;
        assert!(
            game.process_semantic_actions(vec![action(A::MenuFullscreen)])
                .is_empty()
        );
        assert!(!game.menu_visible);
        game.menu_visible = true;
        assert!(
            game.process_semantic_actions(vec![
                action(A::PrimaryFireOrSpectate),
                action(A::MenuFullscreen),
                action(A::BombOrSpectate),
            ])
            .is_empty()
        );
        assert!(!game.menu_visible);
        assert_eq!(game.client_authority.view().phase, phase);
        game.chat.begin_input();
        game.menu_visible = true;
        assert!(
            game.process_semantic_actions(vec![action(A::MenuFullscreen)])
                .is_empty()
        );
        assert!(game.chat.active);
        assert!(
            game.menu_visible,
            "chat owns F even if a menu was previously open"
        );
    }

    #[test]
    fn disconnected_menu_ship_intent_survives_same_ship_and_stale_energy_without_admitting_play() {
        for (spectator, bootstrapping) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let mut game = Game::new_network_authority_test();
            game.networked_play = true;
            game.client_authority = Default::default();
            if bootstrapping {
                game.client_authority.observe_hard_replacement(
                    ClientAuthoritySource::Reliable,
                    1,
                    Some(AuthorityScopeIdentity::default()),
                );
            }
            game.spectator_mode = spectator;
            game.ship.ship_type = 0;
            game.ship.energy = 1.0;
            let phase = game.client_authority.view().phase;
            let mut previous_attempt: Option<quality_return::QualityReturnRecovery> = None;
            for number in [1, 4] {
                game.menu_visible = true;
                assert!(game.retain_unready_ship_return_action(
                    SemanticInputActionKind::NumberShortcut(number)
                ));
                assert_eq!(game.quality_return_recovery.unwrap().ship, number - 1);
                if let Some(mut expected) = previous_attempt {
                    expected.ship = number - 1;
                    assert_eq!(
                        game.quality_return_recovery,
                        Some(expected),
                        "another selection must not reset the attempt budget"
                    );
                }
                previous_attempt = game.quality_return_recovery;
                let mut attempt = previous_attempt.unwrap();
                assert_eq!(
                    attempt.poll(macroquad::miniquad::date::now(), false, false, false),
                    if bootstrapping {
                        quality_return::RecoveryAction::Wait
                    } else {
                        quality_return::RecoveryAction::Reconnect
                    }
                );
                assert_eq!(game.client_authority.view().phase, phase);
                assert_eq!(game.client_authority.view().lifecycle_intent, None);
                assert_eq!(game.ship.ship_type, 0);
                assert_eq!(game.ship.energy, 1.0);
                assert_eq!(game.spectator_mode, spectator);
            }
        }
    }

    #[test]
    fn unready_recovery_retains_only_explicit_menu_playable_ship_requests() {
        let mut game = Game::new_network_authority_test();
        game.client_authority = Default::default();
        game.networked_play = true;
        game.menu_visible = true;
        for kind in [
            SemanticInputActionKind::NumberShortcut(0),
            SemanticInputActionKind::NumberShortcut(9),
            SemanticInputActionKind::SelectShip(8),
            SemanticInputActionKind::Repel,
            SemanticInputActionKind::RandomWarp,
        ] {
            assert!(!game.retain_unready_ship_return_action(kind));
            assert!(game.quality_return_recovery.is_none());
        }
        let choose = SemanticInputActionKind::SelectShip(0);
        game.menu_visible = false;
        assert!(!game.retain_unready_ship_return_action(choose));
        game.menu_visible = true;
        game.networked_play = false;
        assert!(!game.retain_unready_ship_return_action(choose));
        game.networked_play = true;
        game.main_menu_requested = true;
        assert!(!game.retain_unready_ship_return_action(choose));
        assert!(game.quality_return_recovery.is_none());
    }

    #[test]
    fn number_row_respects_gameplay_menu_and_chat_layers() {
        use SemanticInputActionKind as A;
        let mut game = Game::new_network_authority_test();
        let expected = [
            A::PlacePortal,
            A::Burst,
            A::Rocket,
            A::Decoy,
            A::Brick,
            A::Thor,
            A::ToggleStealth,
            A::ToggleCloak,
            A::ToggleXradar,
            A::ToggleAntiwarp,
        ];
        for (number, expected) in expected.into_iter().enumerate() {
            let result =
                game.process_semantic_actions(vec![action(A::NumberShortcut(number as u8))]);
            assert_eq!(result[0].kind, expected);
        }
        let mut portal = action(A::NumberShortcut(0));
        portal.modifiers.shift = true;
        assert_eq!(
            game.process_semantic_actions(vec![portal])[0].kind,
            A::RandomWarp
        );
        game.chat.begin_input();
        assert!(
            game.process_semantic_actions((0..=9).map(|n| action(A::NumberShortcut(n))).collect())
                .is_empty()
        );
        assert!(game.chat.active);
        game.chat.cancel_input();
        game.networked_play = false;
        game.spectator_mode = false;
        for number in 1..=8 {
            game.ship.energy = game.ship.max_energy;
            game.menu_visible = true;
            assert!(
                game.process_semantic_actions(vec![action(A::NumberShortcut(number))])
                    .is_empty()
            );
            assert_eq!(game.ship.ship_type, number as usize - 1);
        }
        game.menu_visible = true;
        assert!(
            game.process_semantic_actions(vec![
                action(A::NumberShortcut(9)),
                action(A::NumberShortcut(0))
            ])
            .is_empty()
        );
        assert!(game.menu_visible);
    }

    #[test]
    fn menu_consumes_weapons_and_q_quits_instead_of_repelling() {
        let mut game = Game::new_network_authority_test();
        let gameplay =
            game.process_semantic_actions(vec![action(SemanticInputActionKind::RepelOrMenuQuit)]);
        assert_eq!(gameplay[0].kind, SemanticInputActionKind::Repel);
        assert!(!game.main_menu_requested);
        assert!(
            game.process_semantic_actions(vec![
                action(SemanticInputActionKind::PrimaryFireOrSpectate),
                action(SemanticInputActionKind::CancelChatOrToggleMenu),
                action(SemanticInputActionKind::Rocket),
                action(SemanticInputActionKind::Repel),
            ])
            .is_empty()
        );
        assert!(game.menu_visible);
        assert!(!game.main_menu_requested);
        assert!(
            game.process_semantic_actions(vec![action(SemanticInputActionKind::RepelOrMenuQuit)])
                .is_empty()
        );
        assert!(game.main_menu_requested);
        assert!(
            game.process_semantic_actions(vec![action(SemanticInputActionKind::BombOrSpectate)])
                .is_empty()
        );
    }

    #[test]
    fn opening_and_closing_menu_does_not_replay_same_batch_weapons() {
        let mut game = Game::new_network_authority_test();
        assert!(
            game.process_semantic_actions(vec![
                action(SemanticInputActionKind::CancelChatOrToggleMenu),
                action(SemanticInputActionKind::Rocket),
                action(SemanticInputActionKind::CancelChatOrToggleMenu),
                action(SemanticInputActionKind::BombOrSpectate),
            ])
            .is_empty()
        );
        assert!(!game.menu_visible);
    }

    #[test]
    fn menu_ship_and_spectator_changes_require_exactly_full_energy() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = false;
        game.spectator_mode = false;
        game.player_respawn_timer = 0.0;
        game.ship.ship_type = 0;
        game.ship.max_energy = 2000.0;
        game.ship.energy = 1999.9;
        for kind in [
            SemanticInputActionKind::SelectShip(1),
            SemanticInputActionKind::ToggleSpectator,
        ] {
            game.menu_visible = true;
            game.process_semantic_actions(vec![action(kind)]);
            assert_eq!(game.ship.ship_type, 0);
            assert!(!game.spectator_mode);
            assert_eq!(game.ship.energy, 1999.9);
            assert_eq!(game.client_authority.view().lifecycle_intent, None);
        }
        game.ship.energy = game.ship.max_energy;
        game.menu_visible = true;
        game.process_semantic_actions(vec![action(SemanticInputActionKind::SelectShip(1))]);
        assert_eq!(game.ship.ship_type, 1);
        game.ship.energy = game.ship.max_energy;
        game.menu_visible = true;
        game.process_semantic_actions(vec![action(SemanticInputActionKind::ToggleSpectator)]);
        assert!(game.spectator_mode);
        game.ship.energy = 0.0;
        game.menu_visible = true;
        game.process_semantic_actions(vec![action(SemanticInputActionKind::SelectShip(2))]);
        assert!(!game.spectator_mode);
        assert_eq!(game.ship.ship_type, 2);
    }

    #[test]
    fn lifecycle_shortcuts_are_inert_without_the_menu() {
        let mut game = Game::new_network_authority_test();
        let ship = game.ship.ship_type;
        let spectator = game.spectator_mode;
        let pending = game.client_authority.view().lifecycle_intent;
        assert!(
            game.process_semantic_actions(vec![
                action(SemanticInputActionKind::SelectShip(1)),
                action(SemanticInputActionKind::ToggleSpectator),
            ])
            .is_empty()
        );
        assert_eq!(game.ship.ship_type, ship);
        assert_eq!(game.spectator_mode, spectator);
        assert_eq!(game.client_authority.view().lifecycle_intent, pending);
        assert!(!game.menu_visible);
    }

    #[test]
    fn escape_then_spectator_performs_one_transition_and_closes_the_menu() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = false;
        game.spectator_mode = false;
        assert!(
            game.process_semantic_actions(vec![
                action(SemanticInputActionKind::CancelChatOrToggleMenu),
                action(SemanticInputActionKind::ToggleSpectator),
                action(SemanticInputActionKind::ToggleSpectator),
            ])
            .is_empty()
        );
        assert!(game.spectator_mode);
        assert!(!game.menu_visible);
    }

    #[test]
    fn escape_then_ship_selection_leaves_spectator_and_closes_the_menu() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = false;
        game.spectator_mode = true;
        game.process_semantic_actions(vec![
            action(SemanticInputActionKind::CancelChatOrToggleMenu),
            action(SemanticInputActionKind::SelectShip(1)),
        ]);
        assert_eq!(game.ship.ship_type, 1);
        assert!(!game.spectator_mode);
        assert!(!game.menu_visible);
    }
}

#[cfg(test)]
mod camera_tests {
    use super::*;

    #[test]
    fn gameplay_ready_and_unready_camera_zoom_uses_the_same_pixel_contract() {
        assert_eq!(
            player_camera_zoom(1440.0, 1000.0),
            vec2(2.0 / 1440.0, 0.002)
        );
        assert_eq!(player_camera_zoom(0.0, 0.0), vec2(2.0, 2.0));
    }
}

#[cfg(test)]
mod logical_step_gate_tests {
    use super::*;
    use crate::fixed_step::FixedGameplayClock;
    use crate::game::lifecycle_sequencer::ClientAuthorityReadiness;
    use crate::protocol::AuthorityScopeIdentity;

    #[test]
    fn a_pending_prize_baseline_does_not_close_an_installed_live_scope_step() {
        let mut game = Game::new_network_authority_test();
        game.client_authority.install_test_scope(
            1,
            AuthorityScopeIdentity {
                server_session_id: Some(7),
                arena_stream_id: Some(11),
                player_id: Some(23),
                life_seq: Some(41),
                alive: Some(true),
            },
            ClientAuthorityReadiness {
                joined: true,
                arena_epoch: Some(13),
                settings: true,
                security: true,
                map: true,
                files: true,
            },
        );
        game.networked_play = true;
        game.door_clock_fresh = true;
        game.network_prize_baseline_pending = false;
        assert!(game.gameplay_step_ready());

        // A respawn or scope cutover wipes the local prize runtime and waits for the
        // server's private baseline. That wait used to stop the frame step, which on an
        // impaired reliable lane costs a retransmit round trip and is the freeze the
        // retained Chrome/Extreme capture published: 64 frames, snapshots and ACKs
        // advancing, one input sequence in 1.1 s. It no longer does, because the wait
        // is enforced where it binds - baseline submission (`network/browser.rs:345`)
        // and `can_send_browser_gameplay_action` (`network/outbound.rs:127`) both test
        // it independently - so a ship mid-flight keeps flying.
        game.network_prize_baseline_pending = true;
        // This models a wait within an already installed live loadout. A
        // successor life with only the predecessor's tuning must stay closed.
        game.last_self_server_prize_life_seq = Some(41);
        assert!(game.gameplay_step_ready());
        // The full gate still reports the wait, which is what the network boundary and
        // the cutover tests consult.
        assert!(!game.local_gameplay_ready());

        // A real authority wait still closes the step, and the shared clock still
        // discards buffered wall time while it is closed so the gap cannot return as a
        // physics/packet burst.
        game.network_prize_baseline_pending = false;
        game.suppress_gameplay_step_after_network_rebase = true;
        assert!(!game.gameplay_step_ready());
        let mut clock = FixedGameplayClock::default();
        assert_eq!(
            clock.advance_when_ready(1.07, game.gameplay_step_ready()),
            0
        );
        assert_eq!(clock.remainder_seconds(), 0.0);
        game.suppress_gameplay_step_after_network_rebase = false;
        assert!(game.gameplay_step_ready());

        // Losing the authority scope is the other half, and it stays closed too.
        game.client_authority.install_test_scope(
            1,
            AuthorityScopeIdentity {
                server_session_id: Some(7),
                arena_stream_id: Some(11),
                player_id: Some(23),
                life_seq: None,
                alive: None,
            },
            ClientAuthorityReadiness {
                joined: true,
                arena_epoch: Some(13),
                settings: true,
                security: true,
                map: true,
                files: true,
            },
        );
        assert!(!game.gameplay_step_ready());
        assert_eq!(
            clock.advance_when_ready(1.07, game.gameplay_step_ready()),
            0
        );
        assert_eq!(clock.remainder_seconds(), 0.0);
    }

    #[test]
    fn a_published_map_with_the_lvz_tail_still_streaming_keeps_the_step_closed() {
        // The truth-table test in `network/tests.rs` proves the predicate; this one
        // proves the wiring: which readiness bit the step actually consults. The
        // shipped client used to start flying on `ZoneMapPublished` alone and then
        // hitch ~1.1 s into play for a ~158 ms `LvzRenderCache::merge_archive` of
        // the arena's remaining authored archives (ordinary-url Chrome/Chaos,
        // retained capture).
        let scope = AuthorityScopeIdentity {
            server_session_id: Some(7),
            arena_stream_id: Some(11),
            player_id: Some(23),
            life_seq: Some(41),
            alive: Some(true),
        };
        let mut game = Game::new_network_authority_test();
        game.client_authority.install_test_scope(
            1,
            scope,
            ClientAuthorityReadiness {
                joined: true,
                arena_epoch: Some(13),
                settings: true,
                security: true,
                map: true,
                files: false,
            },
        );
        game.networked_play = true;
        game.door_clock_fresh = true;
        game.network_prize_baseline_pending = false;
        assert!(
            !game.gameplay_step_ready(),
            "the map alone is not an arena: the authored archives must be merged too",
        );
        // The hold is now roughly a second long on a real join, so the discard is
        // load-bearing: wall time spent waiting for files must not return as a
        // physics and packet burst when the gate opens.
        let mut clock = FixedGameplayClock::default();
        assert_eq!(
            clock.advance_when_ready(1.07, game.gameplay_step_ready()),
            0
        );
        assert_eq!(clock.remainder_seconds(), 0.0);

        game.client_authority.install_test_scope(
            1,
            scope,
            ClientAuthorityReadiness {
                joined: true,
                arena_epoch: Some(13),
                settings: true,
                security: true,
                map: true,
                files: true,
            },
        );
        assert!(game.gameplay_step_ready());
        assert_eq!(
            clock.advance_when_ready(0.02, game.gameplay_step_ready()),
            1
        );
    }

    #[test]
    fn an_unready_gate_discards_the_buffered_wall_time() {
        // The discard is the protection that must survive any change to the gate: a
        // frozen simulation that kept accumulating wall time would replay it as a
        // physics and packet burst the moment the gate reopened.
        let mut clock = FixedGameplayClock::default();
        assert_eq!(clock.advance_when_ready(0.9, false), 0);
        assert_eq!(clock.remainder_seconds(), 0.0);
        // Reopening on the next frame steps that frame alone - one step at the fixed
        // 60 Hz tick - and not the discarded nine-tenths of a second.
        assert_eq!(clock.advance_when_ready(0.02, true), 1);
    }
}

#[cfg(test)]
mod initial_presentation_tests {
    use super::*;
    use crate::game::lifecycle_sequencer::{ClientAuthorityReadiness, ClientAuthoritySource};
    use crate::protocol::AuthorityScopeIdentity;

    #[test]
    fn assigned_player_waits_for_files_current_loadout_and_the_actual_step_gate() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.spectator_mode = false;
        game.door_clock_fresh = true;
        game.network_prize_baseline_pending = true;
        game.last_self_server_energy = Some(1700.0);
        let scope = AuthorityScopeIdentity {
            server_session_id: Some(7),
            arena_stream_id: Some(11),
            player_id: Some(23),
            life_seq: Some(41),
            alive: Some(true),
        };
        let mut readiness = ClientAuthorityReadiness {
            joined: true,
            arena_epoch: Some(13),
            settings: true,
            security: true,
            map: true,
            files: false,
        };
        game.client_authority
            .install_test_scope(1, scope, readiness);
        assert!(
            !game.initial_presentation_ready(),
            "assigned life/energy alone must not reveal the arena"
        );
        readiness.files = true;
        game.client_authority
            .install_test_scope(1, scope, readiness);
        game.last_self_server_prize_life_seq = Some(40);
        assert!(
            !game.initial_presentation_ready(),
            "a predecessor's loadout cannot reveal a new life"
        );
        game.last_self_server_prize_life_seq = Some(41);
        assert!(game.initial_presentation_ready());
        game.suppress_gameplay_step_after_network_rebase = true;
        assert!(
            !game.initial_presentation_ready(),
            "initial reveal shares the real movement barrier"
        );
    }

    #[test]
    fn admitted_initial_spectator_reveals_only_when_camera_controls_are_authorized() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = true;
        game.spectator_mode = true;
        game.network_prize_baseline_pending = true;
        let partial = AuthorityScopeIdentity {
            server_session_id: Some(7),
            arena_stream_id: Some(11),
            player_id: Some(23),
            life_seq: None,
            alive: None,
        };
        game.client_authority.observe_hard_replacement(
            ClientAuthoritySource::Reliable,
            1,
            Some(partial),
        );
        game.client_authority.observe_joined_arena(9);
        game.client_authority.observe_initial_ship(8);
        let scope = AuthorityScopeIdentity {
            life_seq: Some(41),
            alive: Some(false),
            ..partial
        };
        game.client_authority.observe_private_lifecycle(
            ClientAuthoritySource::Event,
            scope,
            None,
            0.0,
        );
        assert!(!game.initial_presentation_ready());
        game.client_authority.observe_settings_ready();
        game.client_authority.observe_security_ready();
        game.client_authority.observe_map_ready();
        assert!(
            !game.initial_presentation_ready(),
            "a stale door clock still closes the camera controls"
        );
        game.door_clock_fresh = true;
        assert!(
            !game.gameplay_step_ready(),
            "spectators never acquire ship authority"
        );
        assert!(
            game.initial_presentation_ready(),
            "an authorized camera needs no ship baseline"
        );
    }

    #[test]
    fn offline_spectator_uses_the_existing_offline_step_path() {
        let mut game = Game::new_network_authority_test();
        game.networked_play = false;
        game.spectator_mode = true;
        game.network_prize_baseline_pending = false;
        assert!(game.initial_presentation_ready());
    }
}
