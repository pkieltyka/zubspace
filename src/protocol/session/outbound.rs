use super::*;

#[allow(dead_code)]
impl ClientProtocolSession {
    pub(crate) fn encode_transport_packet(&self, packet: &[u8]) -> Vec<u8> {
        packet.to_vec()
    }

    pub(super) fn decode_transport_packet(&self, packet: &[u8]) -> Result<Vec<u8>, String> {
        Ok(packet.to_vec())
    }

    pub(crate) fn send_projectile_refresh(
        &mut self,
        server_session_id: u64,
        arena_stream_id: u64,
    ) -> Vec<u8> {
        self.send_reliable_payload(
            &ProjectileRefreshRequestPacket {
                server_session_id,
                arena_stream_id,
            }
            .encode(),
        )
    }

    pub(crate) fn send_password_login(&mut self, login: &PasswordLoginRequest) -> Vec<u8> {
        self.state = ClientProtocolState::LoginSent;
        self.send_reliable_payload(&login.encode_payload())
    }

    pub(crate) fn send_time_sync_request(&self, nonce: u32) -> Vec<u8> {
        self.send_time_sync_request_with_totals(nonce, 0, 0)
    }

    pub(crate) fn send_time_sync_request_with_totals(
        &self,
        nonce: u32,
        total_sent: u32,
        total_received: u32,
    ) -> Vec<u8> {
        TimeSyncRequest {
            nonce,
            total_sent,
            total_received,
        }
        .encode()
    }

    pub(crate) fn send_position_update(&mut self, update: &ClientPositionUpdate) -> Vec<u8> {
        if self.extra_position_info && update.extra.is_none() {
            let mut update = update.clone();
            update.extra = Some(ClientPositionExtra::default());
            return update.encode();
        }

        update.encode()
    }

    /// Ship/life cutover has one exact position-sequence boundary. Carry that
    /// canonical reset through the application reliable sequencer so loss on
    /// the timed-unreliable channel cannot strand the new life forever.
    pub(crate) fn try_send_reliable_position_update(
        &mut self,
        update: &ClientPositionUpdate,
    ) -> Option<(u32, Vec<u8>)> {
        let payload = self.send_position_update(update);
        self.try_send_tracked_reliable_payload(&payload)
    }

    /// Standalone client → server input record, for steps that publish
    /// no position (positions embed the same history). The authoritative
    /// server uses it as the movement intent for its simulation step. See
    /// `docs/authoritative_server.md` §4.
    pub(crate) fn send_input_record(&mut self, packet: &ClientInputRecordPacket) -> Vec<u8> {
        packet.encode()
    }

    pub(crate) fn try_send_movement_correction_applied(
        &mut self,
        application: MovementCorrectionApplied,
    ) -> Option<(u32, Vec<u8>)> {
        self.try_send_tracked_reliable_payload(&application.encode())
    }

    pub(crate) fn try_send_prize_effect_applied(
        &mut self,
        application: PrizeEffectApplied,
    ) -> Option<(u32, Vec<u8>)> {
        self.try_send_tracked_reliable_payload(&application.encode())
    }

    pub(crate) fn try_send_portal_placement(
        &mut self,
        request: PortalPlacementRequest,
    ) -> Option<(u32, Vec<u8>)> {
        self.try_send_tracked_reliable_payload(&request.encode())
    }

    pub(crate) fn try_send_portal_warp(
        &mut self,
        request: PortalWarpRequest,
    ) -> Option<(u32, Vec<u8>)> {
        self.try_send_tracked_reliable_payload(&request.encode())
    }

    pub(crate) fn send_chat_message(&mut self, request: &ChatMessageRequest) -> Vec<u8> {
        self.send_reliable_payload(&request.encode_payload())
    }

    pub(crate) fn try_send_frequency_change(
        &mut self,
        frequency: u16,
        position_sequence_base: u16,
        baseline: &ClientPositionUpdate,
    ) -> Option<(u32, Vec<u8>)> {
        let baseline = self.send_position_update(baseline);
        self.try_send_tracked_reliable_payload(
            &FrequencyChangeRequest {
                frequency,
                position_sequence_base,
                baseline,
            }
            .encode_payload(),
        )
    }

    pub(crate) fn try_send_spectator_view(
        &mut self,
        view: crate::spectator_view::SpectatorViewRequest,
    ) -> Option<(u32, Vec<u8>)> {
        self.try_send_tracked_reliable_payload(&view.encode())
    }

    pub(crate) fn send_spectate_request(&mut self, player_id: u16) -> Vec<u8> {
        self.send_reliable_payload(&SpectateRequest { player_id }.encode_payload())
    }

    pub(crate) fn send_attach_request(&mut self, player_id: u16) -> Vec<u8> {
        self.send_reliable_payload(&AttachRequest { player_id }.encode_payload())
    }

    pub(crate) fn send_flag_request(&mut self, flag_id: u16) -> Vec<u8> {
        self.send_reliable_payload(&FlagRequest { flag_id }.encode_payload())
    }

    pub(crate) fn send_death(&mut self, request: &DeathRequest) -> Vec<u8> {
        self.send_reliable_payload(&request.encode_payload())
    }

    pub(crate) fn send_leave_arena(&mut self) -> Vec<u8> {
        self.send_reliable_payload(&LeaveArenaRequest.encode_payload())
    }

    pub(crate) fn send_disconnect(&mut self) -> Vec<u8> {
        self.state = ClientProtocolState::Disconnected;
        CoreDisconnectRequest.encode()
    }

    pub(crate) fn send_voice_upload_packets(
        &mut self,
        request: &VoiceUploadRequest,
    ) -> Vec<Vec<u8>> {
        self.send_reliable_chunked_payload(&request.encode_payload())
    }

    pub(crate) fn send_client_file_transfer_packets(
        &mut self,
        request: &ClientFileTransferRequest,
    ) -> Vec<Vec<u8>> {
        self.send_reliable_chunked_payload(&request.encode_payload())
    }

    pub(crate) fn send_registration_response_packets(
        &mut self,
        response: &RegistrationFormResponse,
    ) -> Vec<Vec<u8>> {
        self.send_reliable_chunked_payload(&response.encode_payload())
    }

    pub(crate) fn send_lvz_toggle(&mut self, request: &LvzTogglePacket) -> Vec<u8> {
        self.send_reliable_payload(&request.encode_payload())
    }

    pub(crate) fn send_lvz_modify(&mut self, request: &LvzModifyPacket) -> Vec<u8> {
        self.send_reliable_payload(&request.encode_payload())
    }

    pub(crate) fn send_exe_request(&mut self) -> Vec<u8> {
        self.send_reliable_payload(&ExeRequest.encode_payload())
    }

    pub(crate) fn send_news_request(&mut self) -> Vec<u8> {
        self.send_reliable_payload(&NewsRequest.encode_payload())
    }

    pub(crate) fn send_security_violation(&mut self, report: &SecurityViolationReport) -> Vec<u8> {
        self.send_reliable_payload(&report.encode_payload())
    }

    pub(crate) fn send_security_violation_checksum(
        &mut self,
        report: &SecurityViolationChecksumReport,
    ) -> Vec<u8> {
        self.send_reliable_payload(&report.encode_payload())
    }

    pub(crate) fn send_arena_settings_change(
        &mut self,
        request: &ArenaSettingsChangeRequest,
    ) -> Vec<u8> {
        self.send_reliable_payload(&request.encode_payload())
    }

    pub(crate) fn send_drop_attached(&mut self) -> Vec<u8> {
        self.send_reliable_payload(&DropAttachedRequest.encode_payload())
    }

    pub(crate) fn send_drop_flags(&mut self) -> Vec<u8> {
        self.send_reliable_payload(&DropFlagsRequest.encode_payload())
    }

    pub(crate) fn send_end_koth_timer(&mut self) -> Vec<u8> {
        self.send_reliable_payload(&EndKothTimerRequest.encode_payload())
    }

    pub(crate) fn try_send_ship_change(
        &mut self,
        ship: u8,
        position_sequence_base: Option<u16>,
    ) -> Option<(u32, Vec<u8>)> {
        self.try_send_tracked_reliable_payload(
            &ShipChangeRequest {
                ship,
                position_sequence_base,
            }
            .encode_payload(),
        )
    }

    pub(crate) fn send_set_banner(&mut self, request: &SetBannerRequest) -> Vec<u8> {
        self.send_reliable_payload(&request.encode_payload())
    }

    pub(crate) fn send_take_prize(
        &mut self,
        request: &TakePrizeRequest,
        reliable: bool,
    ) -> Vec<u8> {
        let payload = request.encode_payload();
        if reliable {
            self.send_reliable_payload(&payload)
        } else {
            payload
        }
    }

    pub(crate) fn send_drop_brick(&mut self, request: &DropBrickRequest) -> Vec<u8> {
        self.send_reliable_payload(&request.encode_payload())
    }

    pub(crate) fn send_ball_pickup(&mut self, request: &BallPickupRequest) -> Vec<u8> {
        self.send_reliable_payload(&request.encode_payload())
    }

    pub(crate) fn send_ball_fire(&mut self, request: &BallFireRequest) -> Vec<u8> {
        self.send_reliable_payload(&request.encode_payload())
    }

    pub(crate) fn send_ball_goal(&mut self, request: &BallGoalRequest) -> Vec<u8> {
        self.send_reliable_payload(&request.encode_payload())
    }

    pub(crate) fn try_send_objective_action(
        &mut self,
        request: ObjectiveActionRequest,
    ) -> Option<(u32, Vec<u8>)> {
        self.try_send_tracked_reliable_payload(&request.encode())
    }

    pub(crate) fn resend_timed_out_reliable_at_tick(&mut self, timestamp: u32) -> Vec<Vec<u8>> {
        self.sequencer.encode_timed_out_reliable_at_tick(timestamp)
    }
}
