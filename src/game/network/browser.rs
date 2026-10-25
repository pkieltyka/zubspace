use super::*;

impl Game {
    /// Bounded transport recovery. A peer connection that reached open and then died is terminal, and the page reports
    /// that with `zub_rtc_terminal`; nothing else in the client notices, so a dead socket otherwise freezes the session
    /// forever. This retires the dead session, triggers a fresh offer through the shared construction path, and lets the
    /// normal login handshake re-enter scope. It is deliberately bounded: one attempt per terminal episode, and the latch
    /// clears only once the transport reaches open again, so a retry that also dies does not become a reconnect loop.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn maybe_recover_terminal_browser_transport(&mut self) {
        if browser_disconnect_requested() {
            return;
        }
        if self.quality_return_recovery.is_some() {
            // The explicit return attempt owns its one fresh connection.
            return;
        }
        let Some(browser) = self.browser_network.as_ref() else {
            return;
        };
        if browser.disconnect_sent {
            return;
        }
        if browser.client.transport().status() == BrowserRtcStatus::Open {
            // A live transport ends the episode; a later failure may recover again.
            if let Some(browser) = self.browser_network.as_mut() {
                browser.rejoin_attempted = false;
            }
            return;
        }
        if !crate::transport::browser_rtc_transport_is_terminal() || browser.rejoin_attempted {
            return;
        }
        // Built before any Game state is reset: a construction failure must leave the dead session in place for
        // diagnostics rather than discarding it and landing in an unrecoverable empty state.
        match self.begin_browser_network_rejoin(None) {
            Ok(()) => {
                self.set_network_notice("Network lost; rejoining.", TextColor::Yellow);
                self.push_notification("Transport lost; bounded rejoin started.", ORANGE);
            }
            Err(err) => {
                if let Some(browser) = self.browser_network.as_mut() {
                    browser.rejoin_attempted = true;
                }
                self.set_network_notice("Network rejoin failed.", TextColor::DarkRed);
                self.push_notification(&err, RED);
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn begin_browser_network_rejoin(&mut self, selected_ship: Option<u8>) -> Result<(), String> {
        let previous_rejoins = self.browser_network.as_ref().map_or(0, |b| b.rejoin_count);
        let mut client =
            crate::game::init::browser_network_client_from_config(&self.maps[self.map_index])?;
        if let Some(ship) = selected_ship {
            client.client.session.arena_login.ship = ship;
        }
        client.rejoin_attempted = true;
        client.rejoin_count = previous_rejoins.saturating_add(1);
        self.reset_browser_network_state_for_rejoin();
        self.browser_network = Some(client);
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    fn advance_quality_return_recovery(&mut self) {
        use quality_return::RecoveryAction;
        if browser_disconnect_requested() {
            self.quality_return_recovery = None;
            return;
        }
        let Some(mut attempt) = self.quality_return_recovery.take() else {
            return;
        };
        if attempt.is_ready() && crate::transport::browser_rtc_transport_is_terminal() {
            attempt.stop();
        }
        let view = self.client_authority.view();
        let action = attempt.poll(
            macroquad::miniquad::date::now(),
            view.phase == ClientAuthorityPhase::Live
                && self.ship.ship_type == usize::from(attempt.ship)
                && self
                    .client_authority
                    .quality_hold_remaining(macroquad::miniquad::date::now())
                    .is_none(),
            view.observer_scope().is_some() || view.live_scope().is_some(),
            view.transition_pending,
        );
        match action {
            RecoveryAction::Reconnect => {
                if let Err(error) = self.begin_browser_network_rejoin(Some(8)) {
                    attempt.stop();
                    self.quality_return_recovery = Some(attempt);
                    self.push_notification(&error, RED);
                    return;
                }
                // The old playable life cannot continue during the new handshake.
                self.spectator_mode = true;
                self.ship.vel = Vec2::ZERO;
                self.clear_local_weapons();
                self.quality_return_recovery = Some(attempt);
            }
            RecoveryAction::Ready => {
                self.quality_return_recovery = Some(attempt);
                self.push_notification(
                    "Reconnected. Staying in spectator; ESC and select a ship to return.",
                    GREEN,
                );
            }
            RecoveryAction::Wait => self.quality_return_recovery = Some(attempt),
            RecoveryAction::Complete => {
                self.push_notification("Connection restored.", GREEN);
            }
            RecoveryAction::TimedOut => {
                // A late rejection cannot start another reconnect. Only a new
                // user action retries this stopped attempt.
                self.quality_return_recovery = Some(attempt);
                self.push_notification("Connection recovery timed out. Check your connection, then use ESC and select a ship to retry.", ORANGE);
            }
        }
    }

    /// Return the network-scoped half of `Game` to its just-joined shape so the fresh session's own handshake rebuilds
    /// authority. Only state the server re-authors belongs here; local assets, the loaded zone, and user settings survive.
    #[cfg(target_arch = "wasm32")]
    fn reset_browser_network_state_for_rejoin(&mut self) {
        self.client_authority = Default::default();
        self.observer_combat_timeline = Default::default();
        self.prepared_combat_view_receipt = None;
        self.network_world = Default::default();
        self.network_settings_received = false;
        self.network_zone_files_ready = false;
        self.network_zone_map_ready = false;
        self.zone_files_batch_open = false;
        self.zone_files_resolve_pending = false;
        self.network_security_challenge = None;
        self.network_security_challenge_generation = None;
        self.network_security_generation = None;
        self.network_security_synchronized = false;
        self.network_prize_baseline_pending = true;
        self.door_clock_server_tick = None;
        self.door_clock_observed_server_tick = None;
        self.door_clock_anchor_seconds = 0.0;
        self.door_clock_last_observed_seconds = 0.0;
        self.door_clock_fresh = false;
        self.last_observed_browser_server_tick = None;
        self.projectile_refresh_needed = false;
        self.last_projectile_refresh_request_tick = None;
        self.network_notice = None;
        self.player_respawn_timer = 0.0;
        self.player_respawn_seq = 0;
        self.suppress_gameplay_step_after_network_rebase = false;
        self.suppress_lifecycle_tail_position = false;
        self.last_input = Default::default();
        self.next_input_seq = 0;
        self.outbound_input_ring = Default::default();
        self.server_input_applied_through = None;
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn poll_browser_network_before_gameplay(&mut self) {
        // Recovery runs before the transport is borrowed out for polling: it replaces the whole browser network client,
        // so doing it first means the same frame polls the fresh transport instead of the dead one.
        self.advance_quality_return_recovery();
        self.maybe_recover_terminal_browser_transport();
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if browser.disconnect_sent {
            self.browser_network = Some(browser);
            return;
        }

        // Snapshot recovery is transport pacing, not gameplay state. Feed it
        // Macroquad's unwrapped local monotonic clock before draining inbound
        // packets; the session clamps this value forward as an extra guard.
        let local_monotonic_micros = (get_time().max(0.0) * 1_000_000.0) as u64;
        browser
            .client
            .set_local_monotonic_micros(local_monotonic_micros);
        // Acknowledgements are queued by inbound processing but must not wait for the next inbound packet to leave: a
        // client frozen mid zone-file transfer receives nothing further to ack with. See Client::flush_reliable_acks.
        browser.client.flush_reliable_acks();
        browser.client.pump_zone_file_requests();
        browser
            .server_clock
            .advance_local_monotonic_micros(local_monotonic_micros);

        let status = browser.client.transport().status();
        if matches!(status, BrowserRtcStatus::Closed | BrowserRtcStatus::Error) {
            if self.set_network_notice("Network disconnected.", TextColor::DarkRed) {
                self.push_notification("Browser network disconnected.", ORANGE);
            }
            self.browser_network = Some(browser);
            return;
        }
        if status == BrowserRtcStatus::Open && browser_disconnect_requested() {
            if let Err(err) = browser.client.send_disconnect() {
                self.set_network_notice("Network disconnect failed.", TextColor::DarkRed);
                self.push_notification(&err, RED);
            } else {
                self.set_network_notice("Network disconnecting.", TextColor::Yellow);
                mark_browser_disconnect_sent();
                browser.disconnect_sent = true;
            }
            self.browser_network = Some(browser);
            return;
        }

        let protocol_tick = browser.server_clock.current_server_tick().unwrap_or(0);
        self.last_observed_browser_server_tick = browser.server_clock.current_server_tick();
        browser.client.set_outbound_protocol_tick(protocol_tick);
        if self.projectile_refresh_needed
            && self
                .last_projectile_refresh_request_tick
                .is_none_or(|last| {
                    crate::continuum::continuum_tick_diff(protocol_tick, last) >= 200
                })
        {
            if let Some((server_session_id, arena_stream_id)) = browser.server_clock.weapon_scope()
            {
                if browser
                    .client
                    .send_projectile_refresh(server_session_id, arena_stream_id)
                    .is_ok()
                {
                    self.projectile_refresh_needed = false;
                    self.last_projectile_refresh_request_tick = Some(protocol_tick);
                }
            }
        }
        if status == BrowserRtcStatus::Open && !browser.login_sent {
            if let Err(err) = self.send_browser_login_if_ready(&mut browser) {
                self.set_network_notice("Network login failed.", TextColor::DarkRed);
                self.push_notification(&err, RED);
                self.browser_network = Some(browser);
                return;
            }
        }

        let actions = match browser.client.poll_actions_at_tick(protocol_tick) {
            Ok(actions) => actions,
            Err(err) => {
                self.set_network_notice("Network poll failed.", TextColor::DarkRed);
                self.push_notification(&err, RED);
                // A malformed datagram is consumed by the transport. Keep the
                // live client owned by Game so the next frame can continue
                // polling subsequent valid packets.
                self.browser_network = Some(browser);
                return;
            }
        };
        let authority_view = self.client_authority.view();
        let respawn_baseline_pending = authority_view.initial_baseline_pending
            || authority_view.transition_kind == Some(ClientAuthorityTransitionKind::Respawn);
        let correction_pending =
            authority_view.transition_kind == Some(ClientAuthorityTransitionKind::Correction);
        let lifecycle_cutover_pending = self.prize_reconciliation_cutover_pending();
        let polled_events = actions.events.len();
        let (correction_actions, deferred_actions) = partition_browser_network_actions_at_segment(
            actions,
            authority_view.scope.server_session_id,
            authority_view.scope.arena_stream_id,
            authority_view.scope.player_id,
            authority_view.scope.life_seq,
            authority_view.scope.alive,
            authority_view.readiness.joined,
            authority_view.readiness.arena_epoch,
            authority_view.readiness.settings,
            authority_view.readiness.security,
            authority_view.readiness.map,
            authority_view.segment,
            correction_pending,
            respawn_baseline_pending,
            lifecycle_cutover_pending,
        );
        self.poll_peak_events = self.poll_peak_events.max(polled_events);
        self.poll_peak_deferred_events = self
            .poll_peak_deferred_events
            .max(deferred_actions.events.len());
        let _ = self.apply_network_actions(&correction_actions);
        if self
            .client_authority
            .defer_authority_actions(deferred_actions)
            .is_err()
        {
            self.set_network_notice(
                "Network authority staging capacity exceeded.",
                TextColor::DarkRed,
            );
            self.push_notification("Network authority staging failed closed.", RED);
            self.browser_network = Some(browser);
            return;
        }
        if let Some(snapshot_tick) = browser
            .client
            .session()
            .latest_received_snapshot_server_tick
        {
            browser.server_clock.observe_server_tick(snapshot_tick);
        }
        let protocol_tick = browser
            .server_clock
            .current_server_tick()
            .unwrap_or(protocol_tick);
        self.last_observed_browser_server_tick = Some(protocol_tick);
        browser.client.set_outbound_protocol_tick(protocol_tick);
        if let Some(application) = self.client_authority.correction_application_to_submit() {
            match browser
                .client
                .try_send_movement_correction_applied(application)
            {
                Ok(Some(reliable_id)) => {
                    let disposition = self
                        .client_authority
                        .record_correction_application_submitted(reliable_id);
                    debug_assert_eq!(disposition, ClientAuthorityDisposition::Applied);
                }
                Ok(None) => {}
                Err(err) => {
                    self.set_network_notice(
                        "Movement correction application failed.",
                        TextColor::DarkRed,
                    );
                    self.push_notification(&err, RED);
                }
            }
        }
        if let Some(pending) = self.client_authority.pending_prize_effect_application()
            && !pending.submitted()
        {
            match browser
                .client
                .try_send_prize_effect_applied(pending.application)
            {
                Ok(Some(reliable_id)) => {
                    let disposition = self
                        .client_authority
                        .record_prize_effect_application_submitted(
                            pending.application,
                            reliable_id,
                        );
                    debug_assert_eq!(disposition, ClientAuthorityDisposition::Applied);
                }
                Ok(None) => {}
                Err(err) => {
                    self.set_network_notice("Prize effect application failed.", TextColor::DarkRed);
                    self.push_notification(&err, RED);
                }
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn update_browser_network(&mut self, dt: f32, emit_position: bool) {
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if browser.disconnect_sent {
            self.browser_network = Some(browser);
            return;
        }
        let protocol_tick = browser.server_clock.current_server_tick().unwrap_or(0);
        self.last_observed_browser_server_tick = browser.server_clock.current_server_tick();
        browser.client.set_outbound_protocol_tick(protocol_tick);
        let deferred_actions = self.client_authority.take_deferred_authority_actions();
        let authority_view = self.client_authority.view();
        let (ready_actions, boundary_actions) =
            partition_network_actions_behind_movement_correction_at_segment(
                deferred_actions,
                movement_correction_blocks_local_discontinuity(
                    authority_view.correction_application,
                ),
                authority_view.segment,
            );
        // A Warp, explicit coordinate relocation, or hostile Repel is an
        // exact same-life movement boundary. Publish its witness in this
        // post-gameplay frame instead of waiting for the ordinary position
        // cadence: another gameplay step could add velocity or cross a door
        // and make the one-shot server authorization impossible to prove.
        let same_life_boundary_applied = ready_actions
            .events
            .iter()
            .any(network_event_waits_for_movement_correction_terminal);
        self.pending_same_life_movement_boundary_position |= same_life_boundary_applied;
        if self
            .client_authority
            .restore_deferred_authority_actions(boundary_actions)
            .is_err()
        {
            self.set_network_notice(
                "Network authority staging capacity exceeded.",
                TextColor::DarkRed,
            );
            self.push_notification("Network authority staging failed closed.", RED);
            self.browser_network = Some(browser);
            return;
        }
        let admitted_actions = self.apply_network_actions(&ready_actions);
        if let Err(err) =
            self.handle_browser_login_actions(&mut browser, &ready_actions, &admitted_actions)
        {
            self.set_network_notice("Network login failed.", TextColor::DarkRed);
            self.push_notification(&err, RED);
            self.browser_network = Some(browser);
            return;
        }
        let protocol_tick = browser
            .server_clock
            .current_server_tick()
            .unwrap_or(protocol_tick);
        self.last_observed_browser_server_tick = Some(protocol_tick);
        browser.client.set_outbound_protocol_tick(protocol_tick);
        self.expire_pending_repel_application_at_tick(protocol_tick);
        if browser.login_sent {
            // A core reliable ACK proves only that the wrapper reached the
            // server receiver. Position sequencing/admission happens later on
            // the server tick, so only LifecycleBaselineAccepted may open the
            // realtime barrier below.

            // Inbound ACKs were just applied by `poll_game_at_tick`. Retry an
            // unsent lifecycle intent before any other frame work can reserve
            // the newly available tracked-reliable slot.
            if self.client_authority.view().lifecycle_intent.is_some()
                && self
                    .client_authority
                    .view()
                    .lifecycle_request_sequence
                    .is_none()
                && let Err(err) = self.try_send_pending_lifecycle_request(&mut browser)
            {
                self.set_network_notice("Network lifecycle send failed.", TextColor::DarkRed);
                self.push_notification(&err, RED);
                self.browser_network = Some(browser);
                return;
            }

            if let Err(err) = self.try_send_pending_objective_actions(&mut browser) {
                self.set_network_notice("Network objective send failed.", TextColor::DarkRed);
                self.push_notification(&err, RED);
                self.browser_network = Some(browser);
                return;
            }

            if let Err(err) = self.update_browser_time_sync(&mut browser, dt) {
                self.set_network_notice("Network time sync failed.", TextColor::DarkRed);
                self.push_notification(&err, RED);
                self.browser_network = Some(browser);
                return;
            }

            let timed_out_kind = self.client_authority.view().transition_kind;
            let initial_join_disposition = self
                .client_authority
                .advance_initial_deadline(dt, SHIP_CHANGE_ACK_TIMEOUT_SECONDS);
            let initial_join_timed_out =
                initial_join_disposition == ClientAuthorityDisposition::TimedOut;
            if initial_join_disposition == ClientAuthorityDisposition::Reattempt {
                // The join has been waiting on arena data the server has not delivered, and its wait is now bounded
                // by the sequencer instead of lasting forever. This is the re-ask: re-arm the bounded file-request
                // retry cycle, which otherwise stops after its own budget and leaves the client unready with nothing
                // asking again. It deliberately does not fail the session - a slow arena install is legitimate, and
                // a rejoin would restart a download the server may already be part-way through.
                browser.client.rearm_file_request_retry();
                self.set_network_notice(
                    "Still waiting for the initial server baseline.",
                    TextColor::Yellow,
                );
            }
            let lifecycle_cutover_timed_out = self
                .client_authority
                .advance_lifecycle_deadline(dt, SHIP_CHANGE_ACK_TIMEOUT_SECONDS)
                == ClientAuthorityDisposition::TimedOut;
            let application_barrier_timed_out = self
                .client_authority
                .advance_application_barrier_deadline(dt, SHIP_CHANGE_ACK_TIMEOUT_SECONDS);
            if initial_join_timed_out
                || lifecycle_cutover_timed_out
                || application_barrier_timed_out.is_some()
            {
                let timeout_message = match timed_out_kind {
                    _ if application_barrier_timed_out
                        == Some(ClientAuthorityApplicationBarrierKind::PrizeEffect) =>
                    {
                        "Prize effect application timed out"
                    }
                    _ if application_barrier_timed_out
                        == Some(ClientAuthorityApplicationBarrierKind::PortalWarp) =>
                    {
                        "Portal Warp application timed out"
                    }
                    Some(ClientAuthorityTransitionKind::InitialJoin) => {
                        "Initial authority baseline timed out"
                    }
                    Some(ClientAuthorityTransitionKind::Respawn) => {
                        "Respawn baseline acknowledgement timed out"
                    }
                    Some(ClientAuthorityTransitionKind::Correction) => {
                        "Movement correction application timed out"
                    }
                    Some(
                        ClientAuthorityTransitionKind::Ship
                        | ClientAuthorityTransitionKind::Frequency,
                    ) => "Ship/frequency change acknowledgement timed out",
                    None => "Authority transition timed out",
                };
                self.set_network_notice(&format!("{timeout_message}."), TextColor::DarkRed);
                self.push_notification(&format!("{timeout_message}; disconnecting safely."), RED);
                let _ = browser.client.send_disconnect();
                self.browser_network = Some(browser);
                return;
            }

            let authority_view = self.client_authority.view();
            let initial_baseline_to_submit = authority_view.initial_baseline_ready_to_submit;
            let lifecycle_baseline_to_submit = authority_view.lifecycle_baseline_ready_to_submit;
            let emit_ship_cutover_baseline = authority_view.neutral_ship_baseline_ready();
            let ship_loadout_ready =
                !emit_ship_cutover_baseline || self.current_life_loadout_baseline_ready();
            let sequencer_live = authority_view.live_scope().is_some();
            if arena_gameplay_ready(
                authority_view.readiness.joined,
                authority_view.readiness.security,
                self.door_clock_fresh,
                authority_view.readiness.map,
                authority_view.readiness.files,
            ) && ship_loadout_ready
                && (!self.network_prize_baseline_pending || emit_ship_cutover_baseline)
                && !self.suppress_gameplay_step_after_network_rebase
                && authority_view.correction_application.is_none()
                && authority_view.application_barriers == 0
                && (initial_baseline_to_submit || lifecycle_baseline_to_submit || sequencer_live)
            {
                let emit_initial_join_baseline = initial_baseline_to_submit;
                let emit_respawn_baseline = lifecycle_baseline_to_submit
                    && authority_view.transition_kind
                        == Some(ClientAuthorityTransitionKind::Respawn);
                let emit_ship_reset_position = emit_initial_join_baseline
                    || emit_ship_cutover_baseline
                    || emit_respawn_baseline;
                if emit_ship_reset_position {
                    // SetCoordinates is intentionally half-pixel based for
                    // ordinary Continuum warps. The reliable life boundary is
                    // stricter and names the exact server/map spawn.
                    self.normalize_ship_life_reset_pose();
                }
                let frequency_baseline = self.client_authority.lifecycle_baseline_to_submit();
                let emit_frequency_baseline = frequency_baseline.is_some();
                // The lifecycle baseline must be retained before we consume
                // its exact input sequence. A saturated reliable table keeps
                // the intent and barrier pending until ordinary ACKs free a
                // slot; it never emits an untracked reset/baseline.
                if (emit_ship_reset_position || emit_frequency_baseline)
                    && !browser.client.tracked_reliable_capacity_available()
                {
                    self.browser_network = Some(browser);
                    return;
                }
                if let Some(update) = frequency_baseline {
                    match browser.client.try_send_reliable_position_update(&update) {
                        Ok(Some(reliable_id)) => {
                            browser.stats.record_position_sent();
                            let sequence = update.extra.and_then(|extra| {
                                crate::continuum_position::zubspace_position_sequence_from_items(
                                    extra.items,
                                )
                            });
                            if let Some(sequence) = sequence {
                                let disposition =
                                    self.client_authority.record_lifecycle_baseline_submitted(
                                        reliable_id,
                                        sequence,
                                        Some(update.clone()),
                                    );
                                debug_assert_eq!(disposition, ClientAuthorityDisposition::Applied);
                            }
                            self.suppress_lifecycle_tail_position = true;
                            self.pending_weapon = 0;
                            self.cancel_pending_weapon_prediction();
                            self.ship.flash = false;
                        }
                        Ok(None) => {
                            // Capacity was checked immediately above. Keep the
                            // exact stored baseline and fail closed if a future
                            // refactor introduces an intervening reservation.
                        }
                        Err(err) => {
                            self.set_network_notice(
                                "Network position send failed.",
                                TextColor::DarkRed,
                            );
                            self.push_notification(&err, RED);
                        }
                    }
                    self.browser_network = Some(browser);
                    return;
                }
                let position_due = advance_network_position_timer(
                    &mut browser.position_timer,
                    dt,
                    NETWORK_RTC_POSITION_INTERVAL,
                );
                let input_sequence = self.next_input_seq;
                let combat_view_receipt = self.take_combat_view_receipt_for_input(input_sequence);
                let input_packet = if emit_ship_reset_position {
                    // The first position of a new ship/respawn life is exactly
                    // one neutral boundary record. Clearing inside the ring
                    // operation prevents old-life door phases from leaking
                    // even if lifecycle delivery lands between render steps.
                    self.outbound_input_ring.record_neutral_life_boundary(
                        input_sequence,
                        self.map().door_seed,
                        combat_view_receipt,
                    )
                } else {
                    self.outbound_input_ring
                        .record(
                            input_sequence,
                            PlayerInputRecord {
                                bits: self.last_input.bits(),
                                direction: protocol_direction(self.ship.orientation),
                                // Door state advances from render/wall time, not
                                // from this logical sequence. Carry the exact seed
                                // so the server can validate and replay this step.
                                door_seed: self.map().door_seed,
                                combat_view_receipt,
                            },
                        )
                        .without_applied_inputs(self.server_input_applied_through)
                };
                self.next_input_seq = self.next_input_seq.wrapping_add(1);

                let position_emission_due = network_position_emission_due(
                    position_due,
                    emit_position,
                    emit_ship_reset_position,
                    self.suppress_lifecycle_tail_position,
                    self.pending_same_life_movement_boundary_position,
                    self.pending_weapon != 0,
                );

                // Every current-protocol position embeds this exact rolling
                // input packet. Keep standalone publication only for logical
                // catch-up/timer steps that do not emit a position; sending
                // both copies on the usual 60 Hz path needlessly doubled the
                // disposable game-lane ingress rate.
                if standalone_input_emission_due(position_emission_due)
                    && let Err(err) = browser.client.send_input_record(&input_packet)
                {
                    self.push_notification(&format!("Input relay failed: {err}"), RED);
                }

                // A slow render can advance several exact logical inputs at
                // once. Retain and transmit every input above, but publish
                // only the batch's final position. The position references
                // this tail sequence, so server replay consumes every step
                // without an unordered burst of intermediate positions.
                if position_emission_due {
                    let weapon = if emit_ship_reset_position {
                        0
                    } else {
                        self.pending_weapon
                    };
                    let repel_application = if emit_ship_reset_position {
                        None
                    } else {
                        self.pending_repel_application
                    };
                    let mut update = self.client_position_update(
                        input_sequence,
                        weapon,
                        input_packet.clone(),
                        repel_application,
                    );
                    if emit_ship_reset_position {
                        // SetCoordinates intentionally flashes an ordinary
                        // correction. The ship-life boundary is stricter: its
                        // reliable baseline must contain no transient toggles
                        // or motion, regardless of local render state.
                        update.direction = 0;
                        update.timestamp = 0;
                        update.velocity_x = 0;
                        update.velocity_y = 0;
                        update.togglables = 0;
                        update.weapon = 0;
                        update.canonical_state =
                            crate::continuum_position::ClientPositionCanonicalState::from_f32(
                                update.x as f32,
                                update.y as f32,
                                0.0,
                                0.0,
                                0.0,
                            );
                    }
                    let send_result = if emit_ship_reset_position {
                        browser.client.try_send_reliable_position_update(&update)
                    } else {
                        browser.client.send_position_update(&update).map(|()| None)
                    };
                    let reset_delivery_id = match send_result {
                        Ok(reset_delivery_id) => reset_delivery_id,
                        Err(err) => {
                            // Preserve the old fallback evidence on the game
                            // lane if the richer position could not be queued.
                            // A later rolling packet can then recover this
                            // logical input without treating a failed position
                            // send as an admitted movement witness.
                            if let Err(input_err) = browser.client.send_input_record(&input_packet)
                            {
                                self.push_notification(
                                    &format!("Input relay failed: {input_err}"),
                                    RED,
                                );
                            }
                            self.set_network_notice(
                                "Network position send failed.",
                                TextColor::DarkRed,
                            );
                            self.push_notification(&err, RED);
                            self.browser_network = Some(browser);
                            return;
                        }
                    };
                    if weapon != 0 {
                        self.bind_pending_weapon_prediction(input_sequence);
                    }
                    browser.stats.record_position_sent();
                    if repel_application.is_some() {
                        self.repel_application_positions_sent =
                            self.repel_application_positions_sent.saturating_add(1);
                    }
                    if update.warp_collectible_id.is_some()
                        && !self
                            .pending_warp_relocation_sequences
                            .contains(&input_sequence)
                    {
                        self.pending_warp_relocation_sequences
                            .push_back(input_sequence);
                        while self.pending_warp_relocation_sequences.len()
                            > MAX_PENDING_WARP_MARKER_SEQUENCES
                        {
                            self.pending_warp_relocation_sequences.pop_front();
                        }
                    }
                    if emit_ship_reset_position {
                        if emit_initial_join_baseline {
                            if let Some(delivery_id) = reset_delivery_id {
                                let disposition = self
                                    .client_authority
                                    .record_initial_baseline_submitted(delivery_id, input_sequence);
                                debug_assert_eq!(disposition, ClientAuthorityDisposition::Applied);
                            }
                        } else if emit_ship_cutover_baseline || emit_respawn_baseline {
                            if let Some(delivery_id) = reset_delivery_id {
                                let disposition =
                                    self.client_authority.record_lifecycle_baseline_submitted(
                                        delivery_id,
                                        input_sequence,
                                        None,
                                    );
                                debug_assert_eq!(disposition, ClientAuthorityDisposition::Applied);
                            }
                        }
                        self.suppress_lifecycle_tail_position = true;
                    }
                    self.pending_weapon = 0;
                    self.cancel_pending_weapon_prediction();
                    self.ship.flash = false;
                    self.pending_same_life_movement_boundary_position = false;
                }
            }
        }

        let (packets_sent, packets_received) = browser.client.packet_totals();
        let dropped_realtime_packets = browser_dropped_realtime_packets();
        browser.stats.update_rates(
            dt,
            packets_sent,
            packets_received,
            self.network_world.stats.remote_position_updates,
            dropped_realtime_packets,
        );
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    fn update_browser_time_sync(
        &mut self,
        browser: &mut BrowserNetworkClient,
        dt: f32,
    ) -> Result<(), String> {
        browser.time_sync_timer -= dt;
        if browser.time_sync_timer > 0.0 {
            return Ok(());
        }

        browser.time_sync_timer = NETWORK_TIME_SYNC_INTERVAL;
        browser.client.send_time_sync_request(
            browser
                .server_clock
                .next_sync_nonce(self.client_authority.view().segment),
        )
    }

    #[cfg(target_arch = "wasm32")]
    fn handle_browser_login_actions(
        &mut self,
        browser: &mut BrowserNetworkClient,
        actions: &ClientProtocolActions,
        admitted_actions: &[bool],
    ) -> Result<(), String> {
        for (event, admitted) in actions.events.iter().zip(admitted_actions) {
            if !*admitted {
                continue;
            }
            match event {
                ClientProtocolEvent::Disconnected => {
                    self.set_network_notice("Server disconnected.", TextColor::DarkRed);
                    self.push_notification("Server disconnected.", ORANGE);
                }
                ClientProtocolEvent::TimeSyncReceived(response) => {
                    if browser
                        .server_clock
                        .observe_time_sync(*response, self.client_authority.view().segment)
                    {
                        let sample = browser_time_sync_sample(response.response);
                        browser.stats.record_time_sync(sample.rtt_ticks);
                    }
                }
                ClientProtocolEvent::ServerClockBootstrapped(bootstrap) => {
                    let authority_replaced = browser
                        .server_clock
                        .observe_bootstrap(*bootstrap, self.client_authority.view().segment);
                    if authority_replaced {
                        self.next_input_seq = bootstrap.input_sequence_base;
                        self.outbound_input_ring.clear();
                        self.pending_repel_application = None;
                        self.pending_repel_application_expiry_tick = None;
                        self.expired_repel_applications.clear();
                        self.consumed_repel_delivery_generations.clear();
                        self.client_authority.clear_deferred_warp_pickup_acks();
                        self.repel_movement_sequence_window = None;
                        self.applied_collectible_acks.clear();
                        self.pending_warp_relocation_collectible_id = None;
                        self.pending_warp_relocation_life_seq = None;
                        self.pending_warp_relocation_sequences.clear();
                    }
                }
                ClientProtocolEvent::QaTraceContextReceived(context) => {
                    let authority = self.client_authority.view();
                    if authority.scope.server_session_id == Some(context.server_session_id)
                        && authority.scope.arena_stream_id == Some(context.arena_stream_id)
                        && authority.scope.player_id == Some(context.player_id)
                    {
                        publish_qa_trace_context(&format!(
                            "{{\"version\":2,\"serverSessionId\":\"{}\",\"arenaStreamId\":\"{}\",\"playerId\":{},\"bundleId\":\"{}\",\"uploadToken\":\"{}\",\"serverBinarySha256\":\"{}\",\"serverBinaryBytes\":{}}}",
                            context.server_session_id,
                            context.arena_stream_id,
                            context.player_id,
                            context.bundle_id_hex(),
                            context.upload_token_hex(),
                            context.server_binary_sha256_hex(),
                            context.server_binary_bytes,
                        ));
                    }
                }
                ClientProtocolEvent::PasswordAccepted => {
                    browser.identity_login_retry.accepted();
                    crate::assets::set_loading_status("Joining arena…");
                    self.set_network_notice("Login accepted.", TextColor::Green);
                    if crate::browser_metrics::debug_mode() {
                        self.push_notification("Login accepted.", GREEN);
                    }
                }
                ClientProtocolEvent::RegistrationRequired if !browser.registration_requested => {
                    let login = browser_password_login(browser, true);
                    browser.client.send_password_login(&login)?;
                    browser.registration_requested = true;
                    self.set_network_notice("Registering player.", TextColor::Yellow);
                    self.push_notification("Requesting registration form.", ORANGE);
                }
                ClientProtocolEvent::RegistrationFormRequested if !browser.registration_sent => {
                    let response = RegistrationFormResponse::minimal();
                    browser.client.send_registration_response(&response)?;
                    browser.registration_sent = true;
                    self.set_network_notice("Sending registration.", TextColor::Yellow);
                    self.push_notification("Sent registration form.", ORANGE);
                }
                ClientProtocolEvent::LoginFailed(code) => {
                    browser.identity_login_retry.accepted();
                    let message = login_failure_notification(*code);
                    self.set_network_notice(
                        format!("Login rejected: {message}"),
                        TextColor::DarkRed,
                    );
                    self.push_notification(&message, RED);
                    request_browser_join_retry(3)?;
                }
                ClientProtocolEvent::LoginFailureMessage(packet) => {
                    if packet.reason != "ZUBSPACE_ID_IN_USE" {
                        browser.identity_login_retry.accepted();
                    }
                    if packet.reason == "ZUBSPACE_NAME_INVALID" {
                        request_browser_join_retry(5)?;
                    } else if packet.reason == "ZUBSPACE_NAME_IN_USE"
                        || packet.reason == "ZUBSPACE_SERVER_FULL"
                    {
                        request_browser_join_retry(if packet.reason == "ZUBSPACE_NAME_IN_USE" {
                            1
                        } else {
                            2
                        })?;
                    } else if packet.reason == "ZUBSPACE_ID_IN_USE" {
                        browser
                            .identity_login_retry
                            .rejected((get_time().max(0.0) * 1_000_000.0) as u64);
                        self.set_network_notice(
                            "Waiting for your previous session; retrying.",
                            TextColor::Yellow,
                        );
                        crate::assets::set_loading_status(
                            "Waiting for your previous session. Retrying…",
                        );
                    } else if packet.reason.starts_with("ZUBSPACE_ID_INVALID") {
                        self.set_network_notice(
                            "Invalid player id; rejoining.",
                            TextColor::DarkRed,
                        );
                        reset_browser_player_identity("Player id was invalid")?;
                    } else {
                        self.set_network_notice(
                            format!("Login rejected: {}", packet.reason),
                            TextColor::DarkRed,
                        );
                        request_browser_join_retry(3)?;
                    }
                    if packet.reason != "ZUBSPACE_ID_IN_USE" {
                        self.push_notification(&format!("Login failed: {}", packet.reason), RED);
                    }
                }
                ClientProtocolEvent::JoinedArena(_) => {
                    // Reliable replay of the join fact must not hide the
                    // longer-lived explanation for an initial authority wait.
                    if !self.network_notice.as_ref().is_some_and(|notice| {
                        notice.message == "Still waiting for the initial server baseline."
                    }) {
                        self.network_notice = None;
                    }
                    if crate::browser_metrics::debug_mode() {
                        self.push_notification("Joined arena.", GREEN);
                    }
                }
                ClientProtocolEvent::ClientFileRequested(request) => {
                    let response = ClientFileTransferRequest::empty_response_to(request);
                    browser.client.send_client_file_transfer(&response)?;
                }
                _ => {}
            }
        }

        if browser.client.transport().status() == BrowserRtcStatus::Open && !browser.disconnect_sent
        {
            match browser
                .identity_login_retry
                .poll((get_time().max(0.0) * 1_000_000.0) as u64)
            {
                IdentityLoginRetryAction::Wait => {}
                IdentityLoginRetryAction::Send => {
                    let login = browser_password_login(browser, false);
                    // Reliable queue pressure must not reset the overall deadline.
                    if let Err(error) = browser.client.send_password_login(&login) {
                        tracing::debug!(%error, "identity login retry deferred by transport");
                    }
                }
                IdentityLoginRetryAction::ReturnToMenu => {
                    request_browser_join_retry(4)?;
                }
            }
        }
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn set_network_notice(
        &mut self,
        message: impl Into<String>,
        color: TextColor,
    ) -> bool {
        let message = message.into();
        let changed = self
            .network_notice
            .as_ref()
            .is_none_or(|notice| notice.message != message || notice.color != color);
        self.network_notice = Some(NetworkNotice { message, color });
        changed
    }

    pub(in crate::game) fn clear_initial_join_wait_notice(&mut self) {
        if self.network_notice.as_ref().is_some_and(|notice| {
            notice.message == "Still waiting for the initial server baseline."
        }) {
            self.network_notice = None;
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn send_browser_login_if_ready(
        &mut self,
        browser: &mut BrowserNetworkClient,
    ) -> Result<(), String> {
        if browser.login_sent {
            return Ok(());
        }

        let login = browser_password_login(browser, false);
        browser.client.send_password_login(&login)?;
        browser.client.send_time_sync_request(
            browser
                .server_clock
                .next_sync_nonce(self.client_authority.view().segment),
        )?;
        browser.time_sync_timer = NETWORK_TIME_SYNC_INTERVAL;
        browser.login_sent = true;
        if crate::browser_metrics::debug_mode() {
            self.push_notification("Browser network connected.", GREEN);
        }
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
fn browser_password_login(
    browser: &BrowserNetworkClient,
    registration: bool,
) -> PasswordLoginRequest {
    let (machine_id, remote_ip) =
        browser_player_identity_parts(&browser.player_identity).unwrap_or((0, 0));
    PasswordLoginRequest::new(
        LoginProtocol::Continuum,
        registration,
        browser.login_name.clone(),
        browser.password.clone(),
        machine_id,
        0,
        remote_ip,
        0,
    )
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BrowserTimeSyncSample {
    server_tick: u32,
    rtt_ticks: i32,
}

#[cfg(target_arch = "wasm32")]
fn browser_time_sync_sample(response: TimeSyncResponse) -> BrowserTimeSyncSample {
    BrowserTimeSyncSample {
        server_tick: response.local_timestamp,
        rtt_ticks: 0,
    }
}

#[cfg(target_arch = "wasm32")]
fn login_failure_notification(code: u8) -> String {
    match login_response_message(code) {
        Some(message) => format!("Login failed: {message}."),
        None => format!("Login failed: response code {code:#04x}."),
    }
}
