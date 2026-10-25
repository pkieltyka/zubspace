use super::*;

#[cfg(any(test, target_arch = "wasm32"))]
const OBJECTIVE_ACTION_RETRY_TICKS: u32 = 10;

#[cfg(any(test, target_arch = "wasm32"))]
fn objective_action_position_sequence(next_input_sequence: u16) -> u16 {
    next_input_sequence
}

#[cfg(any(test, target_arch = "wasm32"))]
fn objective_action_attempt_blocks(
    attempt: ClientObjectiveActionAttempt,
    candidate: ObjectiveActionRequest,
    identity: NetworkObjectiveIdentity,
    current_server_tick: Option<u32>,
) -> bool {
    if attempt.request.kind != candidate.kind
        || attempt.request.server_session_id != candidate.server_session_id
        || attempt.request.arena_stream_id != candidate.arena_stream_id
        || attempt.request.player_id != candidate.player_id
        || attempt.request.life_seq != candidate.life_seq
        || attempt.request.objective_id != candidate.objective_id
    {
        return false;
    }
    if attempt.request.objective_generation != identity.generation
        || attempt.request.expected_version != identity.version
    {
        return false;
    }
    if identity.last_action_id != attempt.request.action_id
        || identity.last_outcome != ObjectiveStateUpdate::OUTCOME_REJECTED
    {
        return true;
    }
    attempt.sent_at_tick.is_none_or(|sent_at_tick| {
        current_server_tick.is_none_or(|tick| {
            crate::continuum::continuum_tick_elapsed(tick, sent_at_tick)
                < OBJECTIVE_ACTION_RETRY_TICKS
        })
    })
}

impl Game {
    #[allow(dead_code)]
    pub(in crate::game) fn client_position_update(
        &self,
        input_sequence: u16,
        weapon: u16,
        input_history: ClientInputRecordPacket,
        repel_application: Option<crate::continuum_position::ClientRepelApplicationMarker>,
    ) -> ClientPositionUpdate {
        ClientPositionUpdate {
            direction: protocol_direction(self.ship.orientation),
            // The legacy Continuum position wire shape still has this
            // timestamp slot, but zubspace authority never lets browser time
            // drive movement or weapons. Use the extended tail below for the
            // logical sequence and leave this field inert.
            timestamp: 0,
            x: protocol_position_coord(self.ship.pos.x),
            y: protocol_position_coord(self.ship.pos.y),
            velocity_x: protocol_velocity_component(self.ship.vel.x),
            velocity_y: protocol_velocity_component(self.ship.vel.y),
            togglables: ship_togglables(&self.ship, self.in_safe_zone()),
            bounty: self.player_bounty.min(u16::MAX as u32) as u16,
            energy: self.ship.energy.round().clamp(0.0, u16::MAX as f32) as u16,
            weapon,
            extra: Some(ClientPositionExtra {
                s2c_latency: repel_application
                    .map(|marker| marker.snapshot_id)
                    .unwrap_or_default(),
                flag_timer: repel_application
                    .map(|_| crate::continuum_position::CLIENT_POSITION_REPEL_MARKER_MAGIC)
                    .unwrap_or_default(),
                items: crate::continuum_position::zubspace_position_sequence_items(input_sequence),
            }),
            canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_f32(
                self.ship.pos.x,
                self.ship.pos.y,
                self.ship.vel.x,
                self.ship.vel.y,
                self.ship.orientation,
            ),
            repel_application,
            input_history: Some(input_history),
            warp_collectible_id: self.pending_warp_relocation_collectible_id,
        }
    }

    pub(in crate::game) fn send_local_chat_message(&mut self, request: &ChatMessageRequest) {
        #[cfg(target_arch = "wasm32")]
        {
            let send_result = self.with_restored_browser_network(|browser| {
                if browser.login_sent {
                    browser.client.send_chat_message(request)
                } else {
                    Ok(())
                }
            });
            if let Some(Err(err)) = send_result {
                self.push_notification(&err, RED);
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = request;
        }
    }

    /// Temporarily loan the browser client to an operation while keeping
    /// `Game` as its owner before the operation's result is observed. In
    /// particular, transport errors must not turn a recoverable send failure
    /// into a permanent stop of polling and gameplay emission.
    #[cfg(target_arch = "wasm32")]
    fn with_restored_browser_network<T>(
        &mut self,
        operation: impl FnOnce(&mut BrowserNetworkClient) -> T,
    ) -> Option<T> {
        let mut browser = self.browser_network.take()?;
        let result = operation(&mut browser);
        self.browser_network = Some(browser);
        Some(result)
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn can_send_browser_gameplay_action(
        &self,
        browser: &BrowserNetworkClient,
    ) -> bool {
        browser.login_sent
            && !self.network_prize_baseline_pending
            && self.client_authority.view().live_scope().is_some()
            && arena_gameplay_ready(
                self.client_authority.view().readiness.joined,
                self.client_authority.view().readiness.security,
                self.door_clock_fresh,
                self.client_authority.view().readiness.map,
                self.client_authority.view().readiness.files,
            )
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_take_prize(
        &mut self,
        collectible_id: u32,
        pos: Vec2,
        kind: PrizeKind,
        negative: bool,
    ) {
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) {
            let request = take_prize_request(collectible_id, pos, kind, negative);
            let reliable = browser.client.session.take_prize_reliable;
            if let Err(err) = browser.client.send_take_prize(&request, reliable) {
                self.push_notification(&err, RED);
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_drop_brick(&mut self, pos: Vec2) {
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) {
            let request = drop_brick_request(pos);
            if let Err(err) = browser.client.send_drop_brick(&request) {
                self.push_notification(&err, RED);
            }
        }
        self.browser_network = Some(browser);
    }

    /// Reserve the application-reliable request before mutating local
    /// inventory. A saturated reliable table or failed transport leaves the
    /// semantic edge unconsumed so prediction can never outrun authority.
    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn try_send_network_portal_placement(
        &mut self,
        position_sequence: u16,
    ) -> bool {
        if self.pending_portal_placements.len() >= MAX_PENDING_PORTAL_PLACEMENTS {
            return false;
        }
        let Some(mut browser) = self.browser_network.take() else {
            return false;
        };
        let scope = self.client_authority.view().live_scope().map(|scope| {
            (
                scope.server_session_id.expect("complete live scope"),
                scope.arena_stream_id.expect("complete live scope"),
                scope.player_id.expect("complete live scope"),
                scope.life_seq.expect("complete live scope"),
            )
        });
        let ready = self.can_send_browser_gameplay_action(&browser);
        let Some((server_session_id, arena_stream_id, player_id, life_seq)) = scope else {
            self.browser_network = Some(browser);
            return false;
        };
        if !ready || !browser.client.tracked_reliable_capacity_available() {
            self.browser_network = Some(browser);
            return false;
        }
        let action_id = self.next_portal_placement_action_id.max(1);
        let request = PortalPlacementRequest {
            server_session_id,
            arena_stream_id,
            player_id,
            life_seq,
            action_id,
            position_sequence,
        };
        let sent = match browser.client.try_send_portal_placement(request) {
            Ok(Some(_reliable_delivery_id)) => {
                self.pending_portal_placements
                    .push_back(ClientPortalPlacement { request });
                self.next_portal_placement_action_id = action_id.wrapping_add(1).max(1);
                true
            }
            Ok(None) => false,
            Err(err) => {
                self.push_notification(&format!("Portal placement send failed: {err}"), RED);
                false
            }
        };
        self.browser_network = Some(browser);
        sent
    }

    /// Freeze local gameplay behind one exact manual-Warp request. Unlike a
    /// prize Warp, the server chooses whether the current active Portal or
    /// spawn endpoint is legal and returns the canonical rebase state.
    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn try_send_network_portal_warp(
        &mut self,
        position_sequence: u16,
        force_random: bool,
    ) -> bool {
        if self.client_authority.view().portal_warp_barrier {
            return false;
        }
        let Some(mut browser) = self.browser_network.take() else {
            return false;
        };
        let scope = self.client_authority.view().live_scope().map(|scope| {
            (
                scope.server_session_id.expect("complete live scope"),
                scope.arena_stream_id.expect("complete live scope"),
                scope.player_id.expect("complete live scope"),
                scope.life_seq.expect("complete live scope"),
            )
        });
        let ready = self.can_send_browser_gameplay_action(&browser);
        let Some((server_session_id, arena_stream_id, player_id, life_seq)) = scope else {
            self.browser_network = Some(browser);
            return false;
        };
        if !ready || !browser.client.tracked_reliable_capacity_available() {
            self.browser_network = Some(browser);
            return false;
        }
        if position_sequence != self.next_input_seq {
            self.browser_network = Some(browser);
            return false;
        }

        // Unlike placement, a successful Warp freezes further local gameplay
        // until its terminal arrives. Publish the named post-movement witness
        // before installing that barrier; otherwise the reliable request can
        // name a sequence that the normal realtime loop is no longer allowed
        // to emit.
        let mut next_input_ring = self.outbound_input_ring.clone();
        let combat_view_receipt = self.take_combat_view_receipt_for_input(position_sequence);
        let input_packet = next_input_ring.record(
            position_sequence,
            PlayerInputRecord {
                bits: self.last_input.bits(),
                direction: protocol_direction(self.ship.orientation),
                door_seed: self.map().door_seed,
                combat_view_receipt,
            },
        );
        let position = self.client_position_update(
            position_sequence,
            self.pending_weapon,
            input_packet.clone(),
            None,
        );
        if let Err(err) = browser.client.send_input_record(&input_packet) {
            self.push_notification(&format!("Portal Warp input relay failed: {err}"), RED);
            self.browser_network = Some(browser);
            return false;
        }
        if let Err(err) = browser.client.send_position_update(&position) {
            self.push_notification(&format!("Portal Warp position relay failed: {err}"), RED);
            self.browser_network = Some(browser);
            return false;
        }
        self.outbound_input_ring = next_input_ring;
        self.next_input_seq = position_sequence.wrapping_add(1);
        browser.stats.record_position_sent();
        self.pending_weapon = 0;
        self.cancel_pending_weapon_prediction();
        self.ship.flash = false;
        self.pending_same_life_movement_boundary_position = false;

        let action_id = self.next_portal_placement_action_id.max(1);
        let request = PortalWarpRequest {
            force_random,
            server_session_id,
            arena_stream_id,
            player_id,
            life_seq,
            action_id,
            position_sequence,
        };
        let sent = match browser.client.try_send_portal_warp(request) {
            Ok(Some(reliable_delivery_id)) => {
                let disposition = self
                    .client_authority
                    .arm_portal_warp(request, reliable_delivery_id);
                if disposition != ClientAuthorityDisposition::Applied {
                    self.set_network_notice(
                        "Portal Warp authority barrier rejected.",
                        TextColor::DarkRed,
                    );
                    self.browser_network = Some(browser);
                    return false;
                }
                self.next_portal_placement_action_id = action_id.wrapping_add(1).max(1);
                true
            }
            Ok(None) => false,
            Err(err) => {
                self.push_notification(&format!("Portal Warp send failed: {err}"), RED);
                false
            }
        };
        self.browser_network = Some(browser);
        sent
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_ball_pickup(&mut self, ball_id: u8, ball_timestamp: u32) {
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) {
            let _ = ball_timestamp;
            if let Err(err) = self.try_send_objective_action(
                &mut browser,
                ObjectiveActionKind::BallPickup,
                u16::from(ball_id),
            ) {
                self.push_notification(&err, RED);
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_ball_fire(
        &mut self,
        ball_id: u8,
        pos: Vec2,
        velocity: Vec2,
    ) {
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) {
            let _ = (pos, velocity);
            if let Err(err) = self.try_send_objective_action(
                &mut browser,
                ObjectiveActionKind::BallFire,
                u16::from(ball_id),
            ) {
                self.push_notification(&err, RED);
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_ball_goal(&mut self, ball_id: u8, pos: Vec2) {
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) {
            let _ = pos;
            if let Err(err) = self.try_send_objective_action(
                &mut browser,
                ObjectiveActionKind::BallGoal,
                u16::from(ball_id),
            ) {
                self.push_notification(&err, RED);
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_ship_change(&mut self, ship: u8) {
        if ship >= 8 {
            self.quality_return_recovery = None;
        } else if self
            .quality_return_recovery
            .as_ref()
            .is_some_and(|attempt| attempt.is_stopped())
        {
            // Retry must work even if the last transport is now closed and
            // cannot deliver a request/rejection to start another attempt.
            self.quality_return_recovery =
                quality_return::QualityReturnRecovery::new(ship, macroquad::miniquad::date::now());
            return;
        }
        if let Some(attempt) = self.quality_return_recovery.as_mut() {
            attempt.ship = ship;
        }
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_lifecycle_action(&browser) && self.lifecycle_request_can_queue() {
            if self
                .client_authority
                .queue_lifecycle_intent(PendingLifecycleRequest::Ship(ship))
                == ClientAuthorityDisposition::Applied
                && let Err(err) = self.try_send_pending_lifecycle_request(&mut browser)
            {
                self.push_notification(&err, RED);
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_frequency_change(&mut self, frequency: u16) {
        let Some(browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) && self.lifecycle_request_can_queue() {
            let _ = self
                .client_authority
                .queue_lifecycle_intent(PendingLifecycleRequest::Frequency(frequency));
        }
        // `update_browser_network` runs after `update_ship` in this same
        // logical step and owns the first send attempt.  Sending here would
        // reserve N before N's movement and attach the prior exact float tail
        // to the new sequence.
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    fn can_send_browser_lifecycle_action(&self, browser: &BrowserNetworkClient) -> bool {
        self.can_send_browser_gameplay_action(browser)
            || (browser.login_sent
                && self.client_authority.view().observer_scope().is_some()
                && self.door_clock_fresh
                && !crate::transport::browser_rtc_transport_is_terminal())
    }

    #[cfg(target_arch = "wasm32")]
    fn lifecycle_request_can_queue(&self) -> bool {
        (self.client_authority.view().live_scope().is_some()
            || self.client_authority.view().observer_scope().is_some())
            && !self.client_authority.view().transition_pending
    }

    /// Try to assign and emit the exact queued lifecycle request. A full
    /// reliable table returns success-without-progress and leaves every game
    /// boundary field untouched for the next ACK-driven retry.
    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn try_send_pending_lifecycle_request(
        &mut self,
        browser: &mut BrowserNetworkClient,
    ) -> Result<bool, String> {
        let Some(intent) = self.client_authority.view().lifecycle_intent else {
            return Ok(false);
        };
        if !lifecycle_request_emission_ready(self.network_zone_files_ready) {
            // `ZoneMapPublished` is intentionally enough for ordinary
            // gameplay, but not for an application-reliable lifecycle
            // transaction: its server terminal cannot overtake the remaining
            // ordered LVZ/file bootstrap. Keep the immutable user intent and
            // assign its exact sequence only after `ZoneFilesPublished`.
            return Ok(false);
        }
        match intent {
            PendingLifecycleRequest::Ship(ship) => {
                let position_sequence_base = self.next_input_seq;
                let Some(_) = browser
                    .client
                    .try_send_ship_change(ship, Some(position_sequence_base))?
                else {
                    return Ok(false);
                };
                let disposition = self
                    .client_authority
                    .record_ship_request_submitted(ship, position_sequence_base);
                debug_assert_eq!(disposition, ClientAuthorityDisposition::Applied);
            }
            PendingLifecycleRequest::Frequency(frequency) => {
                let position_sequence_base = self.next_input_seq;
                let mut next_input_ring = self.outbound_input_ring.clone();
                next_input_ring.clear();
                let combat_view_receipt =
                    self.take_combat_view_receipt_for_input(position_sequence_base);
                let input_packet = next_input_ring.record(
                    position_sequence_base,
                    PlayerInputRecord {
                        bits: self.last_input.bits() & !PlayerInput::BIT_ROCKET_ACTIVATION_PULSE,
                        direction: protocol_direction(self.ship.orientation),
                        door_seed: self.map().door_seed,
                        combat_view_receipt,
                    },
                );
                let baseline =
                    self.client_position_update(position_sequence_base, 0, input_packet, None);
                let Some(_) = browser.client.try_send_frequency_change(
                    frequency,
                    position_sequence_base,
                    &baseline,
                )?
                else {
                    return Ok(false);
                };
                let disposition = self.client_authority.record_frequency_request_submitted(
                    frequency,
                    position_sequence_base,
                    baseline.clone(),
                );
                debug_assert_eq!(disposition, ClientAuthorityDisposition::Applied);
                self.outbound_input_ring = next_input_ring;
                self.next_input_seq = position_sequence_base.wrapping_add(1);
            }
        }
        Ok(true)
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_flag_request(&mut self, flag_id: u16) {
        let Some(mut browser) = self.browser_network.take() else {
            self.flag_claim_send_gated = self.flag_claim_send_gated.wrapping_add(1);
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) {
            if let Err(err) = self.try_send_objective_action(
                &mut browser,
                ObjectiveActionKind::FlagClaim,
                flag_id,
            ) {
                self.push_notification(&err, RED);
            }
        } else {
            self.flag_claim_send_gated = self.flag_claim_send_gated.wrapping_add(1);
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_drop_flags(&mut self) {
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) {
            let Some(player_id) = self.client_authority.view().scope.player_id else {
                self.browser_network = Some(browser);
                return;
            };
            let anchor = self
                .network_world
                .flags
                .values()
                .find(|flag| flag.carried_by == Some(player_id))
                .map(|flag| flag.flag_id);
            let Some(flag_id) = anchor else {
                self.browser_network = Some(browser);
                return;
            };
            if let Err(err) =
                self.try_send_objective_action(&mut browser, ObjectiveActionKind::FlagDrop, flag_id)
            {
                self.push_notification(&err, RED);
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    fn try_send_objective_action(
        &mut self,
        browser: &mut BrowserNetworkClient,
        kind: ObjectiveActionKind,
        objective_id: u16,
    ) -> Result<bool, String> {
        let objective_kind = match kind {
            ObjectiveActionKind::FlagClaim | ObjectiveActionKind::FlagDrop => {
                ObjectiveStateUpdate::KIND_FLAG
            }
            ObjectiveActionKind::BallPickup
            | ObjectiveActionKind::BallFire
            | ObjectiveActionKind::BallGoal => ObjectiveStateUpdate::KIND_POWERBALL,
        };
        let Some(identity) = self
            .network_world
            .objective_versions
            .get(&(objective_kind, objective_id))
            .copied()
        else {
            return Ok(false);
        };
        let Some(scope) = self.client_authority.view().live_scope() else {
            return Ok(false);
        };
        let server_session_id = scope.server_session_id.expect("complete live scope");
        let arena_stream_id = scope.arena_stream_id.expect("complete live scope");
        let player_id = scope.player_id.expect("complete live scope");
        let life_seq = scope.life_seq.expect("complete live scope");
        let action_id = self.next_objective_action_id.max(1);
        let attempt_key = (objective_kind, objective_id);
        let current_server_tick = self.browser_server_tick();
        let request = ObjectiveActionRequest {
            kind,
            server_session_id,
            arena_stream_id,
            player_id,
            life_seq,
            action_id,
            position_sequence: objective_action_position_sequence(self.next_input_seq),
            objective_id,
            objective_generation: identity.generation,
            expected_version: identity.version,
        };
        if self
            .objective_action_attempts
            .get(&attempt_key)
            .copied()
            .is_some_and(|attempt| {
                objective_action_attempt_blocks(attempt, request, identity, current_server_tick)
            })
        {
            return Ok(false);
        }
        // Reserve the application identity when the local intent is accepted,
        // not when the core reliable table happens to have capacity. This
        // keeps distinct per-object intents unique while retaining the exact
        // position/objective witness across a saturated transport.
        self.next_objective_action_id = action_id.wrapping_add(1).max(1);
        self.objective_action_attempts.insert(
            attempt_key,
            ClientObjectiveActionAttempt {
                request,
                sent_at_tick: None,
            },
        );
        match browser.client.try_send_objective_action(request) {
            Ok(Some(_)) => {
                if let Some(attempt) = self.objective_action_attempts.get_mut(&attempt_key) {
                    attempt.sent_at_tick = current_server_tick;
                }
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(err) => Err(err),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn try_send_pending_objective_actions(
        &mut self,
        browser: &mut BrowserNetworkClient,
    ) -> Result<(), String> {
        if !self.can_send_browser_gameplay_action(browser) {
            return Ok(());
        }
        let current_scope = self.client_authority.view().live_scope().map(|scope| {
            (
                scope.server_session_id.expect("complete live scope"),
                scope.arena_stream_id.expect("complete live scope"),
                scope.player_id.expect("complete live scope"),
                scope.life_seq.expect("complete live scope"),
            )
        });
        let current_server_tick = self.browser_server_tick();
        let pending_keys = self
            .objective_action_attempts
            .iter()
            .filter_map(|(&key, attempt)| attempt.sent_at_tick.is_none().then_some(key))
            .collect::<Vec<_>>();
        for key in pending_keys {
            let Some(attempt) = self.objective_action_attempts.get(&key).copied() else {
                continue;
            };
            let request = attempt.request;
            let current_identity = self.network_world.objective_versions.get(&key).copied();
            let scope_matches = current_scope.is_some_and(
                |(server_session_id, arena_stream_id, player_id, life_seq)| {
                    request.server_session_id == server_session_id
                        && request.arena_stream_id == arena_stream_id
                        && request.player_id == player_id
                        && request.life_seq == life_seq
                },
            );
            let objective_matches = current_identity.is_some_and(|identity| {
                request.objective_generation == identity.generation
                    && request.expected_version == identity.version
            });
            if !scope_matches || !objective_matches {
                self.objective_action_attempts.remove(&key);
                continue;
            }
            match browser.client.try_send_objective_action(request) {
                Ok(Some(_)) => {
                    if let Some(attempt) = self.objective_action_attempts.get_mut(&key) {
                        attempt.sent_at_tick = current_server_tick;
                    }
                }
                Ok(None) => break,
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_end_koth_timer(&mut self) {
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) {
            if let Err(err) = browser.client.send_end_koth_timer() {
                self.push_notification(&err, RED);
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_death(&mut self, killer_id: u16, bounty: u16) {
        let _ = (killer_id, bounty);
        // Server-authoritative multiplayer never lets a browser report
        // death/health state. The server computes lethal damage, score, and
        // respawn from authoritative projectile hits.
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_spectate_request(&mut self, player_id: u16) {
        self.send_browser_observer_view(0.0, Some(player_id));
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_browser_observer_view(&mut self, dt: f32, target: Option<u16>) {
        let Some(scope) = self.client_authority.view().observer_scope() else {
            return;
        };
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        browser.position_timer += dt;
        if browser.login_sent
            && self.network_zone_files_ready
            && self.door_clock_fresh
            && (target.is_some() || browser.position_timer >= 0.1)
        {
            let view = crate::spectator_view::SpectatorViewRequest {
                target: target.unwrap_or(self.spectating_player_id.unwrap_or(INVALID_PLAYER_ID)),
                server_session_id: scope.server_session_id.unwrap(),
                arena_stream_id: scope.arena_stream_id.unwrap(),
                life_seq: scope.life_seq.unwrap(),
                sequence: self.spectator_view_sequence,
                next_input_sequence: Some(self.next_input_seq),
                x: self.ship.pos.x,
                y: self.ship.pos.y,
            };
            match browser.client.send_spectator_view(view) {
                Ok(true) => {
                    browser.position_timer = 0.0;
                    self.spectator_view_sequence = self.spectator_view_sequence.wrapping_add(1);
                }
                Ok(false) => {}
                Err(error) => self.push_notification(&error, RED),
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_attach_request(&mut self, player_id: u16) {
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) {
            if let Err(err) = browser.client.send_attach_request(player_id) {
                self.push_notification(&err, RED);
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(target_arch = "wasm32")]
    pub(in crate::game) fn send_network_drop_attached(&mut self) {
        let Some(mut browser) = self.browser_network.take() else {
            return;
        };
        if self.can_send_browser_gameplay_action(&browser) {
            if let Err(err) = browser.client.send_drop_attached() {
                self.push_notification(&err, RED);
            }
        }
        self.browser_network = Some(browser);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_take_prize(
        &mut self,
        _collectible_id: u32,
        _pos: Vec2,
        _kind: PrizeKind,
        _negative: bool,
    ) {
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_drop_brick(&mut self, _pos: Vec2) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_ball_pickup(&mut self, _ball_id: u8, _ball_timestamp: u32) {
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_ball_fire(
        &mut self,
        _ball_id: u8,
        _pos: Vec2,
        _velocity: Vec2,
    ) {
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_ball_goal(&mut self, _ball_id: u8, _pos: Vec2) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_ship_change(&mut self, _ship: u8) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_frequency_change(&mut self, _frequency: u16) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_flag_request(&mut self, _flag_id: u16) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_drop_flags(&mut self) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_death(&mut self, _killer_id: u16, _bounty: u16) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_spectate_request(&mut self, _player_id: u16) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_attach_request(&mut self, _player_id: u16) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::game) fn send_network_drop_attached(&mut self) {}
}

#[cfg(test)]
mod objective_action_tests {
    use super::*;

    fn request(action_id: u32) -> ObjectiveActionRequest {
        ObjectiveActionRequest {
            kind: ObjectiveActionKind::BallPickup,
            server_session_id: 1,
            arena_stream_id: 2,
            player_id: 3,
            life_seq: 4,
            action_id,
            position_sequence: 5,
            objective_id: 6,
            objective_generation: 7,
            expected_version: 8,
        }
    }

    fn identity(last_action_id: u32, last_outcome: u8) -> NetworkObjectiveIdentity {
        NetworkObjectiveIdentity {
            generation: 7,
            version: 8,
            last_update_tick: 10,
            last_action_id,
            last_outcome,
        }
    }

    #[test]
    fn objective_actions_bind_the_current_forward_position_witness() {
        assert_eq!(objective_action_position_sequence(41), 41);
        assert_eq!(objective_action_position_sequence(u16::MAX), u16::MAX);
    }

    #[test]
    fn objective_attempts_remain_pending_and_rejections_are_rate_limited_per_object() {
        let attempt = ClientObjectiveActionAttempt {
            request: request(9),
            sent_at_tick: Some(crate::continuum::CONTINUUM_TICK_MASK - 4),
        };
        let mut attempts = BTreeMap::new();
        attempts.insert((ObjectiveStateUpdate::KIND_POWERBALL, 6), attempt);
        attempts.insert((ObjectiveStateUpdate::KIND_POWERBALL, 7), attempt);
        assert_eq!(attempts.len(), 2, "one objective cannot overwrite another");
        assert!(objective_action_attempt_blocks(
            attempt,
            request(10),
            identity(8, ObjectiveStateUpdate::OUTCOME_REJECTED),
            Some(3),
        ));
        assert!(objective_action_attempt_blocks(
            attempt,
            request(10),
            identity(9, ObjectiveStateUpdate::OUTCOME_REJECTED),
            Some(3),
        ));
        assert!(!objective_action_attempt_blocks(
            attempt,
            request(10),
            identity(9, ObjectiveStateUpdate::OUTCOME_REJECTED),
            Some(5),
        ));

        let mut new_generation = identity(0, ObjectiveStateUpdate::OUTCOME_REPLAY);
        new_generation.generation = 8;
        assert!(!objective_action_attempt_blocks(
            attempt,
            request(10),
            new_generation,
            None,
        ));

        let mut new_life = request(10);
        new_life.life_seq += 1;
        assert!(!objective_action_attempt_blocks(
            attempt,
            new_life,
            identity(8, ObjectiveStateUpdate::OUTCOME_REPLAY),
            None,
        ));
    }

    #[test]
    fn unsent_objective_intent_keeps_its_exact_witness_until_capacity_returns() {
        let pending = ClientObjectiveActionAttempt {
            request: request(9),
            sent_at_tick: None,
        };

        assert!(objective_action_attempt_blocks(
            pending,
            request(10),
            identity(0, ObjectiveStateUpdate::OUTCOME_REPLAY),
            Some(500),
        ));
        let mut newer = identity(0, ObjectiveStateUpdate::OUTCOME_REPLAY);
        newer.version += 1;
        assert!(!objective_action_attempt_blocks(
            pending,
            request(10),
            newer,
            Some(500),
        ));
    }
}
