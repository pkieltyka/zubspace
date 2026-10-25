use super::*;
/// Decoding every packet the transport accumulated in one frame lets a join
/// backlog arrive as a single 12k-event batch: it spikes frame time and lands
/// the client's authority staging queue within a few hundred events of its
/// hard horizon, so whether the client survives join is a race against
/// `MAX_DEFERRED_AUTHORITY_ACTIONS`. Stop at a bounded slice instead; the
/// remaining packets stay queued in the transport and are decoded by the next
/// poll, in the same order, and reliable chunk assembly already survives a poll
/// boundary.
pub(crate) const MAX_POLLED_EVENTS_PER_POLL: usize = 2_048;
/// Work ceiling for a poll that yields no events (reliable ACKs, duplicates),
/// matching the complete admission horizon the browser bridge grants one lane.
pub(crate) const MAX_POLLED_PACKETS_PER_POLL: usize = 4_096;

pub(crate) struct NetworkClient<T: PacketTransport> {
    pub(crate) session: ClientProtocolSession,
    pub(crate) transport: T,
    packets_sent: u32,
    packets_received: u32,
    // A later packet/send failure cannot undo earlier decoder admissions:
    // their replay IDs have already been consumed. Retain that bounded
    // prefix for the next successful poll instead of silently losing it.
    pending_poll_actions: ClientProtocolActions,
}

#[allow(dead_code)]
impl<T: PacketTransport> NetworkClient<T> {
    pub(crate) fn new(session: ClientProtocolSession, transport: T) -> Self {
        Self {
            session,
            transport,
            packets_sent: 0,
            packets_received: 0,
            pending_poll_actions: ClientProtocolActions::default(),
        }
    }

    pub(crate) fn transport(&self) -> &T {
        &self.transport
    }

    pub(crate) fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    pub(crate) fn session(&self) -> &ClientProtocolSession {
        &self.session
    }

    /// Re-arm the bounded file-request retry cycle. Called when an authority wait expired with no delivery to retry,
    /// so the client asks the server again instead of staying unready forever.
    pub(crate) fn rearm_file_request_retry(&mut self) {
        self.session.rearm_file_request_retry();
    }

    pub(crate) fn set_outbound_protocol_tick(&mut self, tick: u32) {
        self.session.set_outbound_protocol_tick(tick);
    }

    pub(crate) fn set_local_monotonic_micros(&mut self, now_micros: u64) {
        self.session.set_local_monotonic_micros(now_micros);
    }

    pub(crate) fn packet_totals(&self) -> (u32, u32) {
        (self.packets_sent, self.packets_received)
    }

    fn send_packet(&mut self, packet: Vec<u8>) -> Result<(), String> {
        let packet = self.session.encode_transport_packet(&packet);
        self.transport.send_packet(&packet)?;
        self.packets_sent = self.packets_sent.wrapping_add(1);
        Ok(())
    }

    fn send_raw_packet(&mut self, packet: &[u8]) -> Result<(), String> {
        self.transport.send_packet(packet)?;
        self.packets_sent = self.packets_sent.wrapping_add(1);
        Ok(())
    }

    pub(crate) fn resend_timed_out_reliable(&mut self, timestamp: u32) -> Result<usize, String> {
        let packets = self.session.resend_timed_out_reliable_at_tick(timestamp);
        let count = packets.len();
        for packet in packets {
            self.send_packet(packet)?;
        }
        Ok(count)
    }

    pub(crate) fn send_projectile_refresh(
        &mut self,
        server_session_id: u64,
        arena_stream_id: u64,
    ) -> Result<(), String> {
        let packet = self
            .session
            .send_projectile_refresh(server_session_id, arena_stream_id);
        self.send_packet(packet)
    }

    pub(crate) fn send_password_login(
        &mut self,
        login: &PasswordLoginRequest,
    ) -> Result<(), String> {
        let packet = self.session.send_password_login(login);
        self.send_packet(packet)
    }

    pub(crate) fn send_time_sync_request(&mut self, nonce: u32) -> Result<(), String> {
        let packet = self.session.send_time_sync_request_with_totals(
            nonce,
            self.packets_sent,
            self.packets_received,
        );
        self.send_packet(packet)
    }

    pub(crate) fn send_position_update(
        &mut self,
        update: &ClientPositionUpdate,
    ) -> Result<(), String> {
        let packet = self.session.send_position_update(update);
        self.send_packet(packet)
    }

    pub(crate) fn tracked_reliable_capacity_available(&self) -> bool {
        self.session.tracked_reliable_capacity_available()
    }

    pub(crate) fn try_send_reliable_position_update(
        &mut self,
        update: &ClientPositionUpdate,
    ) -> Result<Option<u32>, String> {
        let Some((reliable_id, packet)) = self.session.try_send_reliable_position_update(update)
        else {
            return Ok(None);
        };
        if let Err(err) = self.send_packet(packet) {
            let rolled_back = self.session.rollback_tracked_reliable(reliable_id);
            debug_assert!(rolled_back);
            return Err(err);
        }
        Ok(Some(reliable_id))
    }

    pub(crate) fn reliable_prefix_acked_through(&self, inclusive_id: u32) -> bool {
        self.session.reliable_prefix_acked_through(inclusive_id)
    }

    /// Transmit reliable acknowledgements that are queued but not yet sent.
    ///
    /// Acks otherwise leave the client only while an inbound server packet is being handled. That is circular for a client
    /// whose zone-file transfer window is frozen: the frames it must acknowledge are exactly the ones that would grant it a
    /// gameplay scope and keep snapshots arriving, so once inbound traffic stopped, its queued acks were never transmitted,
    /// the server's in-flight window never freed, and the client stayed invisible to every peer indefinitely. The frame
    /// pump calls this every rendered frame so acknowledgement delivery cannot depend on the traffic it exists to unblock.
    pub(crate) fn flush_reliable_acks(&mut self) {
        for packet in self.session.take_outbound_ack_packets() {
            let retry_copy = packet.clone();
            if self.send_packet(packet).is_err() {
                // A channel whose send buffer is full rejects the write, and an acknowledgement is one-shot: dropping it
                // here would recreate silently the very stall the acknowledgement exists to prevent, so the batch goes back
                // for the next frame. The error is not escalated because a congested lane must not abort the rest of the
                // frame's network work.
                self.session.requeue_failed_ack_packet(retry_copy);
            }
        }
    }

    /// Re-send the zone-file request the queue is waiting on, once per outstanding request.
    ///
    /// Mirrors `flush_reliable_acks` for the same circularity in the other direction: a file whose completed transfer the
    /// client could not use is put back on the queue, and the only thing that used to send a request was another inbound
    /// packet. While the transfer is stuck no inbound file arrives, so the request never left the queue, the server saw
    /// neither requests nor refusals, and the client sat outside gameplay scope invisible to every peer.
    pub(crate) fn pump_zone_file_requests(&mut self) {
        let Some((reliable_id, packet)) = self.session.take_file_request_retry() else {
            return;
        };
        if self.send_packet(packet).is_err() {
            // The sequencer already recorded this assignment, so hand it back exactly as the other tracked reliable sends do;
            // otherwise the retry consumes a slot for a packet that never left the page.
            let rolled_back = self.session.rollback_tracked_reliable(reliable_id);
            debug_assert!(rolled_back);
            self.session.cancel_file_request_retry();
        }
    }

    pub(crate) fn send_input_record(
        &mut self,
        packet: &ClientInputRecordPacket,
    ) -> Result<(), String> {
        let bytes = self.session.send_input_record(packet);
        self.send_packet(bytes)
    }

    pub(crate) fn try_send_movement_correction_applied(
        &mut self,
        application: MovementCorrectionApplied,
    ) -> Result<Option<u32>, String> {
        let Some((reliable_id, packet)) = self
            .session
            .try_send_movement_correction_applied(application)
        else {
            return Ok(None);
        };
        if let Err(err) = self.send_packet(packet) {
            let rolled_back = self.session.rollback_tracked_reliable(reliable_id);
            debug_assert!(rolled_back);
            return Err(err);
        }
        Ok(Some(reliable_id))
    }

    pub(crate) fn try_send_prize_effect_applied(
        &mut self,
        application: PrizeEffectApplied,
    ) -> Result<Option<u32>, String> {
        let Some((reliable_id, packet)) = self.session.try_send_prize_effect_applied(application)
        else {
            return Ok(None);
        };
        if let Err(err) = self.send_packet(packet) {
            let rolled_back = self.session.rollback_tracked_reliable(reliable_id);
            debug_assert!(rolled_back);
            return Err(err);
        }
        Ok(Some(reliable_id))
    }

    pub(crate) fn try_send_portal_placement(
        &mut self,
        request: PortalPlacementRequest,
    ) -> Result<Option<u32>, String> {
        let Some((reliable_id, packet)) = self.session.try_send_portal_placement(request) else {
            return Ok(None);
        };
        if let Err(err) = self.send_packet(packet) {
            let rolled_back = self.session.rollback_tracked_reliable(reliable_id);
            debug_assert!(rolled_back);
            return Err(err);
        }
        Ok(Some(reliable_id))
    }

    pub(crate) fn try_send_portal_warp(
        &mut self,
        request: PortalWarpRequest,
    ) -> Result<Option<u32>, String> {
        let Some((reliable_id, packet)) = self.session.try_send_portal_warp(request) else {
            return Ok(None);
        };
        if let Err(err) = self.send_packet(packet) {
            let rolled_back = self.session.rollback_tracked_reliable(reliable_id);
            debug_assert!(rolled_back);
            return Err(err);
        }
        Ok(Some(reliable_id))
    }

    pub(crate) fn send_chat_message(&mut self, request: &ChatMessageRequest) -> Result<(), String> {
        let packet = self.session.send_chat_message(request);
        self.send_packet(packet)
    }

    pub(crate) fn try_send_frequency_change(
        &mut self,
        frequency: u16,
        position_sequence_base: u16,
        baseline: &ClientPositionUpdate,
    ) -> Result<Option<u32>, String> {
        let Some((reliable_id, packet)) =
            self.session
                .try_send_frequency_change(frequency, position_sequence_base, baseline)
        else {
            return Ok(None);
        };
        if let Err(err) = self.send_packet(packet) {
            let rolled_back = self.session.rollback_tracked_reliable(reliable_id);
            debug_assert!(rolled_back);
            return Err(err);
        }
        Ok(Some(reliable_id))
    }

    pub(crate) fn send_spectator_view(
        &mut self,
        view: crate::spectator_view::SpectatorViewRequest,
    ) -> Result<bool, String> {
        let Some((id, packet)) = self.session.try_send_spectator_view(view) else {
            return Ok(false);
        };
        if let Err(error) = self.send_packet(packet) {
            self.session.rollback_tracked_reliable(id);
            return Err(error);
        }
        Ok(true)
    }

    pub(crate) fn send_spectate_request(&mut self, player_id: u16) -> Result<(), String> {
        let packet = self.session.send_spectate_request(player_id);
        self.send_packet(packet)
    }

    pub(crate) fn send_attach_request(&mut self, player_id: u16) -> Result<(), String> {
        let packet = self.session.send_attach_request(player_id);
        self.send_packet(packet)
    }

    pub(crate) fn send_flag_request(&mut self, flag_id: u16) -> Result<(), String> {
        let packet = self.session.send_flag_request(flag_id);
        self.send_packet(packet)
    }

    pub(crate) fn send_death(&mut self, request: &DeathRequest) -> Result<(), String> {
        let packet = self.session.send_death(request);
        self.send_packet(packet)
    }

    pub(crate) fn send_leave_arena(&mut self) -> Result<(), String> {
        let packet = self.session.send_leave_arena();
        self.send_packet(packet)
    }

    pub(crate) fn send_disconnect(&mut self) -> Result<(), String> {
        let packet = self.session.send_disconnect();
        self.send_packet(packet)
    }

    pub(crate) fn send_voice_upload(&mut self, request: &VoiceUploadRequest) -> Result<(), String> {
        for packet in self.session.send_voice_upload_packets(request) {
            self.send_packet(packet)?;
        }
        Ok(())
    }

    pub(crate) fn send_client_file_transfer(
        &mut self,
        request: &ClientFileTransferRequest,
    ) -> Result<(), String> {
        for packet in self.session.send_client_file_transfer_packets(request) {
            self.send_packet(packet)?;
        }
        Ok(())
    }

    pub(crate) fn send_registration_response(
        &mut self,
        response: &RegistrationFormResponse,
    ) -> Result<(), String> {
        for packet in self.session.send_registration_response_packets(response) {
            self.send_packet(packet)?;
        }
        Ok(())
    }

    pub(crate) fn send_lvz_toggle(&mut self, request: &LvzTogglePacket) -> Result<(), String> {
        let packet = self.session.send_lvz_toggle(request);
        self.send_packet(packet)
    }

    pub(crate) fn send_lvz_modify(&mut self, request: &LvzModifyPacket) -> Result<(), String> {
        let packet = self.session.send_lvz_modify(request);
        self.send_packet(packet)
    }

    pub(crate) fn send_exe_request(&mut self) -> Result<(), String> {
        let packet = self.session.send_exe_request();
        self.send_packet(packet)
    }

    pub(crate) fn send_news_request(&mut self) -> Result<(), String> {
        let packet = self.session.send_news_request();
        self.send_packet(packet)
    }

    pub(crate) fn send_security_violation(
        &mut self,
        report: &SecurityViolationReport,
    ) -> Result<(), String> {
        let packet = self.session.send_security_violation(report);
        self.send_packet(packet)
    }

    pub(crate) fn send_security_violation_checksum(
        &mut self,
        report: &SecurityViolationChecksumReport,
    ) -> Result<(), String> {
        let packet = self.session.send_security_violation_checksum(report);
        self.send_packet(packet)
    }

    pub(crate) fn send_arena_settings_change(
        &mut self,
        request: &ArenaSettingsChangeRequest,
    ) -> Result<(), String> {
        let packet = self.session.send_arena_settings_change(request);
        self.send_packet(packet)
    }

    pub(crate) fn send_drop_attached(&mut self) -> Result<(), String> {
        let packet = self.session.send_drop_attached();
        self.send_packet(packet)
    }

    pub(crate) fn send_drop_flags(&mut self) -> Result<(), String> {
        let packet = self.session.send_drop_flags();
        self.send_packet(packet)
    }

    pub(crate) fn send_end_koth_timer(&mut self) -> Result<(), String> {
        let packet = self.session.send_end_koth_timer();
        self.send_packet(packet)
    }

    pub(crate) fn try_send_ship_change(
        &mut self,
        ship: u8,
        position_sequence_base: Option<u16>,
    ) -> Result<Option<u32>, String> {
        let Some((reliable_id, packet)) = self
            .session
            .try_send_ship_change(ship, position_sequence_base)
        else {
            return Ok(None);
        };
        if let Err(err) = self.send_packet(packet) {
            let rolled_back = self.session.rollback_tracked_reliable(reliable_id);
            debug_assert!(rolled_back);
            return Err(err);
        }
        Ok(Some(reliable_id))
    }

    pub(crate) fn send_set_banner(&mut self, request: &SetBannerRequest) -> Result<(), String> {
        let packet = self.session.send_set_banner(request);
        self.send_packet(packet)
    }

    pub(crate) fn send_take_prize(
        &mut self,
        request: &TakePrizeRequest,
        reliable: bool,
    ) -> Result<(), String> {
        let packet = self.session.send_take_prize(request, reliable);
        self.send_packet(packet)
    }

    pub(crate) fn send_drop_brick(&mut self, request: &DropBrickRequest) -> Result<(), String> {
        let packet = self.session.send_drop_brick(request);
        self.send_packet(packet)
    }

    pub(crate) fn send_ball_pickup(&mut self, request: &BallPickupRequest) -> Result<(), String> {
        let packet = self.session.send_ball_pickup(request);
        self.send_packet(packet)
    }

    pub(crate) fn send_ball_fire(&mut self, request: &BallFireRequest) -> Result<(), String> {
        let packet = self.session.send_ball_fire(request);
        self.send_packet(packet)
    }

    pub(crate) fn send_ball_goal(&mut self, request: &BallGoalRequest) -> Result<(), String> {
        let packet = self.session.send_ball_goal(request);
        self.send_packet(packet)
    }

    pub(crate) fn try_send_objective_action(
        &mut self,
        request: ObjectiveActionRequest,
    ) -> Result<Option<u32>, String> {
        let Some((reliable_id, packet)) = self.session.try_send_objective_action(request) else {
            return Ok(None);
        };
        if let Err(err) = self.send_packet(packet) {
            let rolled_back = self.session.rollback_tracked_reliable(reliable_id);
            debug_assert!(rolled_back);
            return Err(err);
        }
        Ok(Some(reliable_id))
    }

    pub(crate) fn poll_actions(&mut self) -> Result<ClientProtocolActions, String> {
        self.poll_actions_at_tick(self.session.outbound_protocol_tick)
    }

    pub(crate) fn poll_actions_at_tick(
        &mut self,
        protocol_tick: u32,
    ) -> Result<ClientProtocolActions, String> {
        self.session.set_outbound_protocol_tick(protocol_tick);
        self.resend_timed_out_reliable(protocol_tick)?;
        let mut merged = std::mem::take(&mut self.pending_poll_actions);
        let outcome = (|| -> Result<(), String> {
            let mut polled_packets = 0usize;
            while polled_packets < MAX_POLLED_PACKETS_PER_POLL
                && merged.events.len() < MAX_POLLED_EVENTS_PER_POLL
            {
                let Some(packet) = self.transport.recv_packet()? else {
                    break;
                };
                polled_packets += 1;
                self.packets_received = self.packets_received.wrapping_add(1);
                let lane = self.transport.last_received_lane();
                let actions = self.session.handle_transport_packet_from_lane_at_tick(
                    &packet,
                    protocol_tick,
                    lane,
                )?;
                let outbound_start = merged.outbound_packets.len();
                // The current packet is decoded before its ACK/response is
                // sent. Its events need the same protection as the prefix.
                merged.append(actions);
                for outbound in &merged.outbound_packets[outbound_start..] {
                    self.send_packet(outbound.to_vec())?;
                }
            }
            // An ACK can free capacity without another file packet arriving.
            // Advance cached fronts before the next frame's retry pump can ask
            // for already-held bytes instead.
            let mut file_actions = ClientProtocolActions::default();
            self.session
                .advance_zone_file_requests(&mut file_actions, protocol_tick)?;
            let outbound_start = merged.outbound_packets.len();
            merged.append(file_actions);
            for outbound in &merged.outbound_packets[outbound_start..] {
                self.send_packet(outbound.to_vec())?;
            }
            Ok(())
        })();
        if let Err(error) = outcome {
            // Sending/retransmission remains owned by the reliable session;
            // these bytes are only the poll's send-history metadata. Do not
            // accumulate or blindly resend it across repeated failures.
            merged.outbound_packets.clear();
            self.pending_poll_actions = merged;
            return Err(error);
        }
        Ok(merged)
    }
    pub(crate) fn poll_world(
        &mut self,
        world: &mut NetworkWorld,
    ) -> Result<ClientProtocolActions, String> {
        let actions = self.poll_actions()?;
        world.apply_actions(&actions);
        Ok(actions)
    }

    pub(crate) fn poll_game(&mut self, game: &mut Game) -> Result<ClientProtocolActions, String> {
        let actions = self.poll_actions()?;
        game.apply_network_actions(&actions);
        Ok(actions)
    }

    pub(crate) fn poll_game_at_tick(
        &mut self,
        game: &mut Game,
        protocol_tick: u32,
    ) -> Result<ClientProtocolActions, String> {
        let actions = self.poll_actions_at_tick(protocol_tick)?;
        game.apply_network_actions(&actions);
        Ok(actions)
    }
}
