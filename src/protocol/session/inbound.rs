use super::*;

fn exact_priority_application_receipt(payload: &[u8]) -> Option<ClientProtocolEvent> {
    match payload.first().copied() {
        Some(LifecycleBaselineAccepted::TYPE) => LifecycleBaselineAccepted::parse(payload)
            .ok()
            .map(ClientProtocolEvent::LifecycleBaselineAccepted),
        Some(WarpRelocationAccepted::TYPE) => WarpRelocationAccepted::parse(payload)
            .ok()
            .map(ClientProtocolEvent::WarpRelocationAccepted),
        Some(RepelApplicationAccepted::TYPE) => RepelApplicationAccepted::parse(payload)
            .ok()
            .map(ClientProtocolEvent::RepelApplicationAccepted),
        Some(MovementCorrection::TYPE) => MovementCorrection::parse(payload)
            .ok()
            .map(ClientProtocolEvent::MovementCorrected),
        Some(MovementCorrectionAccepted::TYPE) => MovementCorrectionAccepted::parse(payload)
            .ok()
            .map(ClientProtocolEvent::MovementCorrectionAccepted),
        Some(PrizeEffectAccepted::TYPE) => PrizeEffectAccepted::parse(payload)
            .ok()
            .map(ClientProtocolEvent::PrizeEffectAccepted),
        Some(PortalPlacementAccepted::TYPE) => PortalPlacementAccepted::parse(payload)
            .ok()
            .map(ClientProtocolEvent::PortalPlacementAccepted),
        Some(PortalWarpAccepted::TYPE) => PortalWarpAccepted::parse(payload)
            .ok()
            .map(ClientProtocolEvent::PortalWarpAccepted),
        _ => None,
    }
}

fn malformed_application_receipt_prefix(payload: &[u8]) -> bool {
    matches!(
        payload.first().copied(),
        Some(LifecycleBaselineAccepted::TYPE)
            | Some(WarpRelocationAccepted::TYPE)
            | Some(RepelApplicationAccepted::TYPE)
            | Some(MovementCorrection::TYPE)
            | Some(MovementCorrectionAccepted::TYPE)
            | Some(PrizeEffectAccepted::TYPE)
            | Some(PortalPlacementAccepted::TYPE)
            | Some(PortalWarpAccepted::TYPE)
    ) && exact_priority_application_receipt(payload).is_none()
}

#[allow(dead_code)]
impl ClientProtocolSession {
    /// Only an exactly-once ordered reliable carrier can deliver a remote
    /// cleanup older than the realtime replay horizon. Do not reopen old
    /// weapon/state traffic or local prediction cutoffs (whose input sequence
    /// may have wrapped while this prefix was blocked).
    fn dispatch_ordered_stale_remote_weapon_cutoff(
        &mut self,
        payload: &[u8],
        actions: &mut ClientProtocolActions,
    ) -> bool {
        use crate::realtime_event::{AuthenticatedRealtimeEvent, RealtimeEventPayload};
        if payload.first() != Some(&crate::realtime_event::REALTIME_EVENT_PACKET)
            || payload.get(2)
                != Some(&crate::realtime_event::REALTIME_EVENT_OWNER_WEAPONS_CANCELLED)
        {
            return false;
        }
        let Ok(event) = AuthenticatedRealtimeEvent::parse(payload) else {
            return false;
        };
        let RealtimeEventPayload::OwnerWeaponsCancelled(cancellation) = event.payload else {
            return false;
        };
        let Some(bootstrap) = self.latest_server_clock_bootstrap else {
            return false;
        };
        let Some(self_id) = self.self_player_id else {
            return false;
        };
        if event.server_session_id != bootstrap.server_session_id
            || event.arena_stream_id != bootstrap.arena_stream_id
            || self_id == cancellation.source_player_id
            || !self
                .realtime_event_window
                .is_older_than_window(event.event_id)
        {
            return false;
        }
        actions
            .events
            .push(ClientProtocolEvent::RealtimeOwnerWeaponsCancelled {
                server_session_id: event.server_session_id,
                arena_stream_id: event.arena_stream_id,
                event_id: event.event_id,
                server_tick: event.server_tick,
                cancellation,
            });
        true
    }

    pub(crate) fn handle_server_packet(
        &mut self,
        packet: &[u8],
    ) -> Result<ClientProtocolActions, String> {
        self.handle_server_packet_at_tick(packet, self.outbound_protocol_tick)
    }

    fn handle_server_packet_at_tick(
        &mut self,
        packet: &[u8],
        local_tick: u32,
    ) -> Result<ClientProtocolActions, String> {
        let parsed = ServerPacket::parse(packet, self.continuum_packet_format)?;
        let mut actions = ClientProtocolActions::default();

        match parsed {
            ServerPacket::SpawnIdleSpectator(fact) => {
                actions
                    .events
                    .push(ClientProtocolEvent::SpawnIdleSpectator(fact));
            }
            ServerPacket::ConnectionQualityDirective(fact) => {
                actions
                    .events
                    .push(ClientProtocolEvent::ConnectionQualityDirective(fact));
            }
            ServerPacket::ConnectionQualityProbe(probe) => {
                if self.latest_server_clock_bootstrap.is_some_and(|scope| {
                    scope.server_session_id == probe.server_session_id
                        && scope.arena_stream_id == probe.arena_stream_id
                }) {
                    actions.push_unreliable(probe.encode_echo());
                }
            }
            ServerPacket::PlayerId(assignment) => {
                self.self_player_id = Some(assignment.player_id);
                self.roster.clear();
                self.turf_flag_ids.clear();
                self.reset_snapshot_state();
                actions
                    .events
                    .push(ClientProtocolEvent::PlayerIdAssigned(assignment.player_id));
            }
            ServerPacket::JoinGame => {
                self.weapons_received = 0;
                self.snapshot_stream_epoch = self.snapshot_stream_epoch.wrapping_add(1).max(1);
                self.reset_snapshot_state();
                actions
                    .events
                    .push(ClientProtocolEvent::JoinedArena(self.snapshot_stream_epoch));
            }
            ServerPacket::QaTraceContext(context) => {
                actions
                    .events
                    .push(ClientProtocolEvent::QaTraceContextReceived(context));
            }
            ServerPacket::PlayerEntering(entry) => {
                self.roster.insert(entry.player_id, entry.clone());
                actions
                    .events
                    .push(ClientProtocolEvent::PlayerEntered(entry));
                if packet.len() > PlayerRosterEntry::PACKET_LEN {
                    actions.append(self.handle_plain_transport_packet_at_tick(
                        &packet[PlayerRosterEntry::PACKET_LEN..],
                        local_tick,
                    )?);
                }
            }
            ServerPacket::PlayerLeaving(leaving) => {
                self.detach_roster_player_and_children(leaving.player_id);
                self.roster.remove(&leaving.player_id);
                if self.self_player_id == Some(leaving.player_id) {
                    self.self_player_id = None;
                    self.snapshot_stream_epoch = 0;
                    self.reset_snapshot_state();
                }
                actions
                    .events
                    .push(ClientProtocolEvent::PlayerLeft(leaving.player_id));
            }
            ServerPacket::PasswordResponse(response) => match response.action() {
                PasswordResponseAction::SendArenaLogin => {
                    self.server_held_zone_files = packet.get(PasswordResponse::MIN_PACKET_LEN..)
                        == Some(crate::zone_wire::HELD_ZONE_FILES_LOGIN_TRAILER.as_slice());
                    self.state = ClientProtocolState::ArenaLoginSent;
                    actions.events.push(ClientProtocolEvent::PasswordAccepted);
                    actions.push_reliable_at_tick(
                        &mut self.sequencer,
                        &self.arena_login.encode_payload(),
                        local_tick,
                    );
                }
                PasswordResponseAction::RequestRegistration => {
                    self.state = ClientProtocolState::RegistrationRequested;
                    actions
                        .events
                        .push(ClientProtocolEvent::RegistrationRequired);
                }
                PasswordResponseAction::WaitForRegistrationForm => {
                    self.state = ClientProtocolState::RegistrationForm;
                    actions
                        .events
                        .push(ClientProtocolEvent::RegistrationFormRequested);
                }
                PasswordResponseAction::Failed(code) => {
                    self.state = ClientProtocolState::Failed(code);
                    actions.events.push(ClientProtocolEvent::LoginFailed(code));
                }
            },
            ServerPacket::Chat(chat) => {
                actions.events.push(ClientProtocolEvent::Chat(chat));
            }
            ServerPacket::ScoreUpdate(score) => {
                if let Some(entry) = self.roster.get_mut(&score.player_id) {
                    entry.kill_points = score.kill_points;
                    entry.flag_points = score.flag_points;
                    entry.wins = score.wins;
                    entry.losses = score.losses;
                }
                actions
                    .events
                    .push(ClientProtocolEvent::ScoreUpdated(score));
            }
            ServerPacket::ResetScore(reset) => {
                for entry in self.roster.values_mut() {
                    if reset.applies_to(entry.player_id) {
                        entry.kill_points = 0;
                        entry.flag_points = 0;
                        entry.wins = 0;
                        entry.losses = 0;
                    }
                }
                actions.events.push(ClientProtocolEvent::ScoresReset(reset));
            }
            ServerPacket::SoccerGoal(goal) => {
                actions.events.push(ClientProtocolEvent::SoccerGoal(goal));
            }
            ServerPacket::PlayerVoice(voice) => {
                actions.events.push(ClientProtocolEvent::PlayerVoice(voice));
            }
            ServerPacket::FrequencyChange(change) => {
                self.detach_roster_player_and_children(change.player_id);
                if let Some(entry) = self.roster.get_mut(&change.player_id) {
                    entry.frequency = change.frequency;
                    entry.flags = 0;
                }
                actions
                    .events
                    .push(ClientProtocolEvent::FrequencyChanged(change));
            }
            ServerPacket::ArenaSettings(settings) => {
                self.take_prize_reliable = settings.take_prize_reliable;
                self.arena_settings_crc32 = Some(continuum_file_crc32(packet));
                self.last_settings_packet = Some(packet.to_vec());
                actions
                    .events
                    .push(ClientProtocolEvent::ArenaSettingsUpdated(settings));
            }
            ServerPacket::FileTransfer(file) => {
                self.file_transfer_assembler.clear();
                actions
                    .events
                    .push(ClientProtocolEvent::FileTransferred(file));
            }
            ServerPacket::FileTransferChunk(chunk) => {
                let expected_index = self
                    .pending_file_requests
                    .front()
                    .map(|request| request.index)
                    .ok_or_else(|| {
                        self.file_transfer_assembler.clear();
                        self.file_chunk_index_mismatches =
                            self.file_chunk_index_mismatches.saturating_add(1);
                        "received server file chunk without pending request".to_string()
                    })?;
                if chunk.file_index != expected_index {
                    self.file_transfer_assembler.clear();
                    self.file_chunk_index_mismatches =
                        self.file_chunk_index_mismatches.saturating_add(1);
                    return Err(format!(
                        "server file chunk index mismatch: {} != {expected_index}",
                        chunk.file_index
                    ));
                }
                self.file_chunks_accepted = self.file_chunks_accepted.saturating_add(1);
                // The assembler rejects a chunk whose offset is not the next expected one and clears itself, which makes
                // every later chunk for that file an orphan. The error is propagated by `?` and counted nowhere, so a
                // single lost chunk is invisible in every counter that existed before this line.
                let assembled = self.file_transfer_assembler.push(chunk);
                if assembled.is_err() {
                    self.file_chunk_assembler_rejects =
                        self.file_chunk_assembler_rejects.saturating_add(1);
                }
                if let Some(packet) = assembled? {
                    actions
                        .append(self.handle_plain_transport_packet_at_tick(&packet, local_tick)?);
                }
            }
            ServerPacket::NoOp(_) => {}
            ServerPacket::SecurityChallenge(challenge) => {
                actions
                    .events
                    .push(ClientProtocolEvent::SecurityChallengeReceived {
                        challenge,
                        server_session_id: self
                            .latest_server_clock_bootstrap
                            .map(|bootstrap| bootstrap.server_session_id),
                        arena_epoch: self.snapshot_stream_epoch,
                    });

                if let Some(settings_packet) = &self.last_settings_packet {
                    self.settings_checksum =
                        continuum_settings_checksum(challenge.checksum_key, settings_packet)?;
                }

                let map_security_checksum =
                    self.map_security_checksum_for_key(challenge.checksum_key);
                if challenge.should_send_security(map_security_checksum) {
                    let security = if self.continuum_packet_format {
                        SecurityPacket::continuum(
                            self.weapons_received,
                            self.settings_checksum,
                            self.exe_checksum,
                            map_security_checksum,
                            self.stats,
                            self.continuum_map_crc32,
                        )
                    } else {
                        SecurityPacket::vie(
                            self.weapons_received,
                            self.settings_checksum,
                            self.exe_checksum,
                            map_security_checksum,
                            self.stats,
                        )
                    };
                    actions.push_unreliable(security.encode());
                    actions
                        .events
                        .push(ClientProtocolEvent::SecurityResponseQueued);
                }
            }
            ServerPacket::ClientFileRequest(request) => {
                actions
                    .events
                    .push(ClientProtocolEvent::ClientFileRequested(request));
            }
            ServerPacket::ShipFrequencyChange(change) => {
                self.detach_roster_player_and_children(change.player_id);
                if let Some(entry) = self.roster.get_mut(&change.player_id) {
                    entry.ship = change.ship;
                    entry.frequency = change.frequency;
                    entry.flags = 0;
                }
                actions
                    .events
                    .push(ClientProtocolEvent::ShipFrequencyChanged(change));
            }
            ServerPacket::FrequencyCutoverAck(ack) => {
                actions
                    .events
                    .push(ClientProtocolEvent::FrequencyCutoverAcknowledged(ack));
            }
            ServerPacket::ShipChangeRejected(receipt) => {
                actions
                    .events
                    .push(ClientProtocolEvent::ShipChangeRejected(receipt));
            }
            ServerPacket::LifecycleBaselineAccepted(receipt) => {
                actions
                    .events
                    .push(ClientProtocolEvent::LifecycleBaselineAccepted(receipt));
            }
            ServerPacket::WarpRelocationAccepted(receipt) => {
                actions
                    .events
                    .push(ClientProtocolEvent::WarpRelocationAccepted(receipt));
            }
            ServerPacket::RepelApplicationAccepted(receipt) => {
                actions
                    .events
                    .push(ClientProtocolEvent::RepelApplicationAccepted(receipt));
            }
            ServerPacket::MovementCorrection(correction) => {
                actions
                    .events
                    .push(ClientProtocolEvent::MovementCorrected(correction));
            }
            ServerPacket::MovementCorrectionAccepted(receipt) => {
                actions
                    .events
                    .push(ClientProtocolEvent::MovementCorrectionAccepted(receipt));
            }
            ServerPacket::PrizeEffectAccepted(receipt) => {
                actions
                    .events
                    .push(ClientProtocolEvent::PrizeEffectAccepted(receipt));
            }
            ServerPacket::PortalPlacementAccepted(receipt) => {
                actions
                    .events
                    .push(ClientProtocolEvent::PortalPlacementAccepted(receipt));
            }
            ServerPacket::PortalWarpAccepted(receipt) => {
                actions
                    .events
                    .push(ClientProtocolEvent::PortalWarpAccepted(receipt));
            }
            ServerPacket::CreateTurretLink(link) => {
                if let Some(destination_id) = link.attached_destination() {
                    if let Some(entry) = self.roster.get_mut(&link.requester_id) {
                        entry.attach_parent = destination_id;
                    }
                } else if let Some(player_id) = link.detach_target(self.self_player_id) {
                    self.push_local_detach_request_if_attached(&mut actions, player_id, local_tick);
                    if let Some(entry) = self.roster.get_mut(&player_id) {
                        entry.attach_parent = INVALID_PLAYER_ID;
                    }
                }
                actions
                    .events
                    .push(ClientProtocolEvent::TurretLinkCreated(link));
            }
            ServerPacket::DestroyTurretLink(link) => {
                if self
                    .self_player_id
                    .and_then(|player_id| self.roster.get(&player_id))
                    .is_some_and(|entry| entry.attach_parent == link.player_id)
                {
                    actions.push_reliable_at_tick(
                        &mut self.sequencer,
                        &AttachRequest {
                            player_id: INVALID_PLAYER_ID,
                        }
                        .encode_payload(),
                        local_tick,
                    );
                }
                for entry in self.roster.values_mut() {
                    if entry.attach_parent == link.player_id {
                        entry.attach_parent = INVALID_PLAYER_ID;
                    }
                }
                actions
                    .events
                    .push(ClientProtocolEvent::TurretLinkDestroyed(link));
            }
            ServerPacket::Spectate(packet) => {
                self.extra_position_info = packet.extra_position_info;
                actions
                    .events
                    .push(ClientProtocolEvent::SpectateUpdated(packet));
            }
            ServerPacket::LoginFailure(packet) => {
                actions
                    .events
                    .push(ClientProtocolEvent::LoginFailureMessage(packet));
            }
            ServerPacket::Version(packet) => {
                actions
                    .events
                    .push(ClientProtocolEvent::ServerVersion(packet));
            }
            ServerPacket::PersonalBannerChange(change) => {
                actions
                    .events
                    .push(ClientProtocolEvent::PersonalBannerChanged(change));
            }
            ServerPacket::PlayerBannerChange(change) => {
                actions
                    .events
                    .push(ClientProtocolEvent::PlayerBannerChanged(change));
            }
            ServerPacket::ToggleSendDamage(packet) => {
                actions
                    .events
                    .push(ClientProtocolEvent::SendDamageToggled(packet));
            }
            ServerPacket::ToggleUfo(packet) => {
                actions.events.push(ClientProtocolEvent::UfoToggled(packet));
            }
            ServerPacket::SetPersonalKothTimer(packet) => {
                actions
                    .events
                    .push(ClientProtocolEvent::PersonalKothTimerSet(packet));
            }
            ServerPacket::KothGameReset(packet) => {
                actions
                    .events
                    .push(ClientProtocolEvent::KothGameReset(packet));
            }
            ServerPacket::AddKothTime(packet) => {
                actions
                    .events
                    .push(ClientProtocolEvent::KothTimeAdded(packet));
            }
            ServerPacket::WatchDamage(report) => {
                let mut report = report;
                report.entries.retain(|entry| {
                    self.remember_damage_identity(
                        DamageEventCarrier::Reliable,
                        report.player_id,
                        entry.victim_life_seq,
                        entry.damage_seq,
                    )
                });
                if !report.entries.is_empty() {
                    actions
                        .events
                        .push(ClientProtocolEvent::DamageWatched(report));
                }
            }
            ServerPacket::MapInformation(info) => {
                self.clear_pending_zone_transfer_state();
                self.pending_zone_publication = true;
                actions.events.push(ClientProtocolEvent::ZoneFilesPending);
                self.pending_zone_map_sha256 = info.sha256;
                self.pending_file_requests.extend(info.file_requests());
                self.pending_zone_map_cache_identity = info.file_request().cache_identity();
                self.queue_next_file_request(&mut actions, local_tick)?;
                self.publish_zone_files_if_complete(&mut actions);
            }
            ServerPacket::CompressedMap(packet) => {
                let request = self
                    .pending_file_requests
                    .pop_front()
                    .ok_or_else(|| "received compressed map without pending request".to_string())?;
                self.file_request_in_flight = None;
                // Every rejection below used to consume the request and abort, which is how a session ended up holding a zone
                // file it had never received and would never ask for again. Put the request back before returning: the error
                // still reaches the player as text, and the frame pump re-requests the file.
                let decoded = match packet.decode_for_request(&request) {
                    Ok(decoded) => decoded,
                    Err(err) => {
                        self.reject_file_response(request);
                        return Err(err);
                    }
                };
                self.accept_zone_file_bytes(request, decoded, &mut actions)?;
                self.queue_next_file_request(&mut actions, local_tick)?;
            }
            ServerPacket::LargePosition(update) | ServerPacket::SmallPosition(update) => {
                if update.weapon != 0 {
                    self.weapons_received = self.weapons_received.wrapping_add(1);
                }
                actions
                    .events
                    .push(ClientProtocolEvent::PlayerPosition(update));
            }
            ServerPacket::PlayerDeath(death) => {
                self.apply_player_death(death);
                actions.events.push(ClientProtocolEvent::PlayerDeath(death));
            }
            ServerPacket::PlayerPrize(prize) => {
                actions.events.push(ClientProtocolEvent::PlayerPrize(prize));
            }
            ServerPacket::PrizePickupAck(ack) => {
                actions
                    .events
                    .push(ClientProtocolEvent::PrizePickupAck(ScopedPrizePickupAck {
                        ack,
                        server_session_id: self
                            .latest_server_clock_bootstrap
                            .map(|bootstrap| bootstrap.server_session_id),
                        arena_stream_id: self
                            .latest_server_clock_bootstrap
                            .map(|bootstrap| bootstrap.arena_stream_id),
                        player_id: self.self_player_id,
                    }));
            }
            ServerPacket::HiddenPrizeSpawn(spawn) => {
                actions
                    .events
                    .push(ClientProtocolEvent::HiddenPrizeSpawn(spawn));
            }
            ServerPacket::FlagPosition(position) => {
                actions
                    .events
                    .push(ClientProtocolEvent::FlagPosition(position));
            }
            ServerPacket::FlagClaim(claim) => {
                self.apply_flag_claim(claim);
                actions.events.push(ClientProtocolEvent::FlagClaim(claim));
            }
            ServerPacket::FlagVictory(victory) => {
                actions
                    .events
                    .push(ClientProtocolEvent::FlagVictory(victory));
            }
            ServerPacket::DropFlag(drop) => {
                if let Some(entry) = self.roster.get_mut(&drop.player_id) {
                    entry.flags = 0;
                }
                actions.events.push(ClientProtocolEvent::DropFlag(drop));
            }
            ServerPacket::ShipReset(_) => {
                actions.events.push(ClientProtocolEvent::ShipReset);
            }
            ServerPacket::CollectedPrize(prize) => {
                actions
                    .events
                    .push(ClientProtocolEvent::CollectedPrize(prize));
            }
            ServerPacket::BrickDropped(brick) => {
                actions
                    .events
                    .push(ClientProtocolEvent::BrickDropped(brick));
            }
            ServerPacket::TurfFlagUpdate(update) => {
                self.turf_flag_ids = (0..update.owners.len() as u16).collect();
                actions
                    .events
                    .push(ClientProtocolEvent::TurfFlagUpdate(update));
            }
            ServerPacket::FlagReward(reward) => {
                actions.events.push(ClientProtocolEvent::FlagReward(reward));
            }
            ServerPacket::SpeedGameOver(result) => {
                actions
                    .events
                    .push(ClientProtocolEvent::SpeedGameOver(result));
            }
            ServerPacket::KeepAlive(_) => {
                actions.events.push(ClientProtocolEvent::KeepAlive);
            }
            ServerPacket::BatchedSmallPositions(updates) => {
                for update in updates {
                    actions
                        .events
                        .push(ClientProtocolEvent::BatchedSmallPlayerPosition(update));
                }
            }
            ServerPacket::BatchedLargePositions(updates) => {
                for update in updates {
                    actions
                        .events
                        .push(ClientProtocolEvent::BatchedLargePlayerPosition(update));
                }
            }
            ServerPacket::ToggleLvz(packet) => {
                actions.events.push(ClientProtocolEvent::LvzToggled(packet));
            }
            ServerPacket::ModifyLvz(packet) => {
                actions
                    .events
                    .push(ClientProtocolEvent::LvzModified(packet));
            }
            ServerPacket::PowerballPosition(update) => {
                actions
                    .events
                    .push(ClientProtocolEvent::PowerballPosition(update));
            }
            ServerPacket::ObjectiveState(update) => {
                actions
                    .events
                    .push(ClientProtocolEvent::ObjectiveState(update));
            }
            ServerPacket::ObjectiveScore(update) => {
                actions
                    .events
                    .push(ClientProtocolEvent::ObjectiveScore(update));
            }
            ServerPacket::ArenaDirectory(listing) => {
                actions
                    .events
                    .push(ClientProtocolEvent::ArenaDirectory(listing));
                // Spectators never receive SetCoordinates during the real
                // server bootstrap. ArenaDirectory is the first ordered,
                // authenticated lifecycle boundary common to both ships and
                // spectators after MapInformation (and after coordinates for
                // live ships), so only spectators release a cache hit here.
                // The server can override the requested ship (for example a
                // reconnect-persistent quality hold). The authenticated self
                // roster determines whether coordinates will follow; retaining
                // the login preference here strands a cached playable->spec
                // join forever. Keep the request fallback for legacy bootstraps
                // which have not supplied a self roster entry.
                let assigned_ship = self
                    .self_player_id
                    .and_then(|id| self.roster.get(&id))
                    .map_or(self.arena_login.ship, |entry| entry.ship);
                if assigned_ship >= 8 {
                    self.release_pending_cached_map_install(&mut actions);
                }
            }
            ServerPacket::ZoneBannerAdvertisement(advertisement) => {
                actions
                    .events
                    .push(ClientProtocolEvent::ZoneBannerAdvertised(advertisement));
            }
            ServerPacket::PostLogin(_) => {
                actions.events.push(ClientProtocolEvent::PostLogin);
            }
            ServerPacket::SetCoordinates(coordinates) => {
                // The coordinate packet is the authoritative live-ship
                // boundary that releases a cache hit. Install the level
                // first so switch_to_map cannot overwrite the server spawn.
                self.release_pending_cached_map_install(&mut actions);
                actions
                    .events
                    .push(ClientProtocolEvent::SetCoordinates(coordinates));
            }
            ServerPacket::Redirect(packet) => {
                actions.events.push(ClientProtocolEvent::Redirect(packet));
            }
            ServerPacket::SelectBox(packet) => {
                actions.events.push(ClientProtocolEvent::SelectBox(packet));
            }
            ServerPacket::Snapshot(frame) => {
                // Authenticate the arena generation before delta lookup or
                // any state/event/private-lifecycle application. A packet
                // held on either unordered channel from a previous join is
                // inert even when its snapshot/player ids are reused.
                let stream_matches = self.latest_server_clock_bootstrap.map_or(
                    frame.header.arena_stream_id == 0,
                    |bootstrap| {
                        frame.header.arena_stream_id != 0
                            && frame.header.arena_stream_id == bootstrap.arena_stream_id
                    },
                );
                if !stream_matches {
                    return Ok(actions);
                }
                let frame = self.expand_prize_references(frame);
                let snapshot_is_new = snapshot_id_advances(
                    self.latest_received_snapshot_id,
                    frame.header.snapshot_id,
                );
                let frame = if snapshot_is_new {
                    let base = if frame.header.is_full() {
                        None
                    } else {
                        self.snapshot_history
                            .iter()
                            .find(|entry| entry.snapshot_id == frame.header.base_id)
                            .map(|entry| &entry.entities)
                    };
                    let Ok(applied) = delta_apply(base, &frame) else {
                        if self.snapshot_full_request_is_due() {
                            actions.push_reliable_at_tick(
                                &mut self.sequencer,
                                &SnapshotFullRequestPacket.encode(),
                                local_tick,
                            );
                        }
                        // Entity state from unknown or invalid bases is never
                        // applied or acked. Snapshot-scoped events are
                        // independent, self-contained facts with their own
                        // sequence ids, so retain them across the resync path.
                        self.emit_snapshot_events(&frame, &mut actions);
                        return Ok(actions);
                    };
                    self.snapshot_history.push_back(ClientSnapshotHistoryEntry {
                        snapshot_id: frame.header.snapshot_id,
                        entities: applied.entities,
                    });
                    // FULL frames replace current render/carry state below,
                    // but cannot erase older retained bases. Delivery on the
                    // game channel is unordered: after ACKing FULL N, another
                    // already-in-flight FULL N+1 may arrive while the server
                    // has begun sending deltas against N.
                    let retained_base_id = if frame.header.is_full() {
                        self.latest_acked_snapshot_id
                    } else {
                        Some(frame.header.base_id)
                    };
                    self.trim_snapshot_history(retained_base_id);
                    applied.frame
                } else {
                    frame
                };
                let contains_repel_weapon = frame.near_entities.iter().any(|entity| {
                    entity.weapon.is_some_and(|weapon| {
                        crate::continuum_weapon::continuum_weapon_type(weapon) == 5
                    })
                }) || frame.events.iter().any(|event| {
                    matches!(event, SnapshotEvent::LifecycleState(lifecycle) if lifecycle.repel_delivery.is_some())
                });
                // Phase 3e-4a: piggyback a periodic ack on the reliable
                // channel so the server learns the latest snapshot the
                // client has consumed. The server uses this to choose
                // a delta base in 3e-4b.
                if snapshot_is_new {
                    self.latest_received_snapshot_id = Some(frame.header.snapshot_id);
                    self.latest_received_snapshot_server_tick = Some(frame.header.tick);
                    self.latest_received_snapshot_base_id = Some(frame.header.base_id);
                    self.snapshots_since_last_ack = self.snapshots_since_last_ack.saturating_add(1);
                    if contains_repel_weapon {
                        self.latest_repel_snapshot_id = Some(frame.header.snapshot_id);
                    }
                    // Weapon events ride the unordered/unreliable snapshot
                    // stream. ACK a repel-bearing frame immediately and by
                    // exact id so the server never treats a later received
                    // frame as proof that this one arrived.
                    if contains_repel_weapon
                        || self.snapshots_since_last_ack >= SNAPSHOT_ACK_INTERVAL
                    {
                        let ack = SnapshotAckPacket {
                            snapshot_id: frame.header.snapshot_id,
                        };
                        actions.push_reliable_at_tick(
                            &mut self.sequencer,
                            &ack.encode(),
                            local_tick,
                        );
                        self.latest_acked_snapshot_id = Some(frame.header.snapshot_id);
                        self.latest_acked_snapshot_server_tick = Some(frame.header.tick);
                        self.snapshots_since_last_ack = 0;
                    }

                    // Full-state snapshots reset the
                    // carry-forward map so previously-applied state for
                    // entities that have left AOI doesn't bleed into
                    // future deltas.
                    if frame.header.is_full() {
                        self.snapshot_entity_carry.clear();
                    }

                    // Snapshot-scoped combat events compare their source id
                    // with the previously applied snapshot. Emit them before
                    // advancing NetworkWorld membership below; their own
                    // damage sequence still provides exactly-once delivery.
                    self.emit_snapshot_events(&frame, &mut actions);

                    // Publish exact authoritative membership before any
                    // render-grade position synthesized from this frame.
                    // NetworkWorld uses the same-frame wire life to scope
                    // Initial/snap counters, so emitting this after position
                    // would attribute the first presentation to a stale or
                    // unknown generation and then erase it during rebase.
                    actions
                        .events
                        .push(ClientProtocolEvent::SnapshotReceived(frame.clone()));

                    // Publish typed snapshot positions with full frame
                    // provenance. Reuse the position payload for energy,
                    // heading and presentation, but never send these through
                    // legacy datagram timestamp admission.
                    // Unmasked fields fall back to the carry-forward
                    // state; masked fields update both the carry-forward
                    // and the synthesized update. Mid-tier radar positions
                    // below carry the same exact frame identity.
                    let snapshot_timestamp = frame.header.tick as u16;
                    for entity in &frame.near_entities {
                        // Authenticated zubspace sessions receive immutable
                        // weapon facts via 0x7d. Keep snapshot weapon display
                        // only for the unauthenticated legacy/unit stream.
                        let publish_weapon = frame.header.arena_stream_id == 0
                            && entity.weapon.is_some_and(|weapon| weapon != 0);
                        let carry = self
                            .snapshot_entity_carry
                            .entry(entity.entity_id)
                            .or_default();
                        if let Some(v) = entity.x {
                            carry.x = v;
                        }
                        if let Some(v) = entity.y {
                            carry.y = v;
                        }
                        if let Some(v) = entity.velocity_x {
                            carry.velocity_x = v;
                        }
                        if let Some(v) = entity.velocity_y {
                            carry.velocity_y = v;
                        }
                        if let Some(v) = entity.direction {
                            carry.direction = v;
                        }
                        if let Some(v) = entity.energy {
                            carry.energy = v;
                        }
                        if let Some(v) = entity.weapon {
                            carry.weapon = v;
                        }
                        if let Some(v) = entity.status {
                            carry.status = v;
                        }
                        if let Some(v) = entity.bounty {
                            carry.bounty = v;
                        }
                        if let Some(v) = entity.ping {
                            carry.ping = v;
                        }
                        if let Some(v) = entity.flag_timer {
                            carry.flag_timer = v;
                        }
                        // The weapon byte is a one-shot "this tick I fired X"
                        // signal, not a persistent state. Carry-forwarding it
                        // (as we do with energy/bounty/etc.) re-triggers
                        // `apply_remote_weapon_visual` on every subsequent
                        // snapshot frame whose delta omits the weapon field,
                        // multiplying rendered projectiles 3-4x on the
                        // receiving client. Drive the visual ONLY from
                        // `entity.weapon` so a no-op delta passes 0 and the
                        // visual stays quiet. `carry.weapon` is still
                        // updated above for any future consumer that wants
                        // the last-known fire value.
                        //
                        // `STATUS_FLASH` is also a one-shot visual signal. The
                        // codec reconstructs the exact persistent status value
                        // against the referenced base, while the field mask
                        // records whether this wire frame actually carried a
                        // status update. Suppress only a base-carried flash;
                        // safety/cloak/etc. must still be allowed to return to
                        // the referenced base's value.
                        let update_status = if entity.field_mask & SNAPSHOT_FIELD_STATUS != 0 {
                            carry.status
                        } else {
                            carry.status & !crate::STATUS_FLASH
                        };
                        carry.status &= !crate::STATUS_FLASH;
                        let update = PlayerPositionUpdate {
                            player_id: entity.entity_id,
                            direction: carry.direction,
                            timestamp: snapshot_timestamp,
                            x: carry.x,
                            y: carry.y,
                            velocity_x: carry.velocity_x,
                            velocity_y: carry.velocity_y,
                            togglables: update_status,
                            ping: carry.ping,
                            bounty: carry.bounty,
                            // A missing-base copy can publish the independent
                            // one-shot before another copy later becomes
                            // state-applicable. The shared idempotence key must
                            // suppress the weapon on that later normal position
                            // event as well as on an ordinary stale duplicate.
                            weapon: entity.weapon.filter(|_| publish_weapon).unwrap_or(0),
                            checksum: None,
                            energy: Some(carry.energy),
                            s2c_latency: None,
                            flag_timer: Some(carry.flag_timer),
                            items: None,
                        };
                        actions
                            .events
                            .push(ClientProtocolEvent::SnapshotPlayerPosition {
                                update,
                                snapshot_id: frame.header.snapshot_id,
                                server_tick: frame.header.tick,
                                arena_stream_id: frame.header.arena_stream_id,
                            });
                    }
                    // Mid-tier: synthesize a radar-grade position update
                    // ([REVIEW H4]) so never-near players still draw radar
                    // dots — previously mid only refreshed the carry map and
                    // a pure-mid player had no render position at all. The
                    // mid wire carries position only: velocity is sent as
                    // zero (radar fixes interpolate as eased holds between
                    // 30 Hz frames), `energy: None` so labels never
                    // fabricate a value, and `weapon: 0` so no remote fire
                    // visual can trigger from radar range.
                    for entity in &frame.mid_entities {
                        let carry = self
                            .snapshot_entity_carry
                            .entry(entity.entity_id)
                            .or_default();
                        if let Some(v) = entity.x {
                            carry.x = v;
                        }
                        if let Some(v) = entity.y {
                            carry.y = v;
                        }
                        carry.status &= !crate::STATUS_FLASH;
                        let update = PlayerPositionUpdate {
                            player_id: entity.entity_id,
                            direction: carry.direction,
                            timestamp: snapshot_timestamp,
                            x: carry.x,
                            y: carry.y,
                            velocity_x: 0,
                            velocity_y: 0,
                            togglables: carry.status,
                            ping: carry.ping,
                            bounty: carry.bounty,
                            weapon: 0,
                            checksum: None,
                            energy: None,
                            s2c_latency: None,
                            flag_timer: Some(carry.flag_timer),
                            items: None,
                        };
                        actions
                            .events
                            .push(ClientProtocolEvent::SnapshotPlayerPosition {
                                update,
                                snapshot_id: frame.header.snapshot_id,
                                server_tick: frame.header.tick,
                                arena_stream_id: frame.header.arena_stream_id,
                            });
                    }
                }

                // Phase 3e-6 cutover: synthesize a
                // `ClientProtocolEvent::DamageWatched` for each
                // `SnapshotEvent::WatchDamage` in the frame so the
                // existing game/network.rs notification + energy
                // update pathway works unchanged.
                if !snapshot_is_new {
                    self.emit_snapshot_events(&frame, &mut actions);
                }
            }
            ServerPacket::RealtimeEvent(event) => {
                let Some(bootstrap) = self.latest_server_clock_bootstrap else {
                    return Ok(actions);
                };
                if event.server_session_id != bootstrap.server_session_id
                    || event.arena_stream_id != bootstrap.arena_stream_id
                    || !self.realtime_event_window.admit(event.event_id)
                {
                    return Ok(actions);
                }
                match event.payload {
                    crate::realtime_event::RealtimeEventPayload::Weapon(source) => {
                        actions
                            .events
                            .push(ClientProtocolEvent::RealtimeWeaponFired {
                                server_session_id: event.server_session_id,
                                arena_stream_id: event.arena_stream_id,
                                event_id: event.event_id,
                                server_tick: event.server_tick,
                                source,
                            });
                    }
                    crate::realtime_event::RealtimeEventPayload::WatchDamage(source) => {
                        if !self.remember_damage_identity(
                            DamageEventCarrier::Realtime,
                            source.victim_player_id,
                            Some(source.victim_life_seq),
                            Some(source.damage_seq),
                        ) {
                            return Ok(actions);
                        }
                        actions.events.push(ClientProtocolEvent::DamageWatched(
                            ServerDamageReport {
                                player_id: source.victim_player_id,
                                timestamp: event.server_tick,
                                entries: vec![ServerDamageReportEntry {
                                    attacker_id: source.shooter_player_id,
                                    weapon_data: source.weapon_data,
                                    energy: source.energy_after,
                                    damage: source.damage,
                                    timestamp_offset: 0,
                                    energy_before: Some(source.energy_before),
                                    damage_seq: Some(source.damage_seq),
                                    projectile_id: Some(source.projectile_id),
                                    victim_life_seq: Some(source.victim_life_seq),
                                    shooter_life_seq: Some(source.shooter_life_seq),
                                    explicit_lethal: Some(source.lethal),
                                    // Dedicated events never mutate snapshot
                                    // membership/lifecycle. Their arena and
                                    // runtime identities were validated above.
                                    source_snapshot_id: None,
                                    source_snapshot_epoch: None,
                                }],
                            },
                        ));
                    }
                    crate::realtime_event::RealtimeEventPayload::RepelMutations(batch) => {
                        actions
                            .events
                            .push(ClientProtocolEvent::RealtimeRepelMutations {
                                server_session_id: event.server_session_id,
                                arena_stream_id: event.arena_stream_id,
                                event_id: event.event_id,
                                server_tick: event.server_tick,
                                batch,
                            });
                    }
                    crate::realtime_event::RealtimeEventPayload::OwnerWeaponsCancelled(
                        cancellation,
                    ) => {
                        actions
                            .events
                            .push(ClientProtocolEvent::RealtimeOwnerWeaponsCancelled {
                                server_session_id: event.server_session_id,
                                arena_stream_id: event.arena_stream_id,
                                event_id: event.event_id,
                                server_tick: event.server_tick,
                                cancellation,
                            });
                    }
                    crate::realtime_event::RealtimeEventPayload::ProjectileState(state) => {
                        actions
                            .events
                            .push(ClientProtocolEvent::RealtimeProjectileState {
                                server_session_id: event.server_session_id,
                                arena_stream_id: event.arena_stream_id,
                                event_id: event.event_id,
                                server_tick: event.server_tick,
                                state,
                            });
                    }
                }
            }
            ServerPacket::ServerClockBootstrap(bootstrap) => {
                let replaces_authority =
                    self.latest_server_clock_bootstrap.is_none_or(|previous| {
                        previous.server_session_id != bootstrap.server_session_id
                            || previous.arena_stream_id != bootstrap.arena_stream_id
                    });
                if replaces_authority {
                    self.realtime_event_window = RealtimeEventWindow::default();
                    self.self_player_id = None;
                    self.roster.clear();
                    self.turf_flag_ids.clear();
                    self.snapshot_stream_epoch = 0;
                    self.reset_snapshot_state();
                }
                self.latest_server_clock_bootstrap = Some(bootstrap);
                self.latest_received_snapshot_server_tick = Some(bootstrap.server_tick);
                actions
                    .events
                    .push(ClientProtocolEvent::ServerClockBootstrapped(bootstrap));
            }
            ServerPacket::Unknown { packet_type, .. } => {
                actions
                    .events
                    .push(ClientProtocolEvent::UnknownPacket(packet_type));
            }
        }

        Ok(actions)
    }

    fn snapshot_full_request_is_due(&mut self) -> bool {
        let due = self
            .last_snapshot_full_request_micros
            .is_none_or(|last_request| {
                self.local_monotonic_micros.saturating_sub(last_request)
                    >= SNAPSHOT_FULL_REQUEST_COOLDOWN_MICROS
            });
        if due {
            self.last_snapshot_full_request_micros = Some(self.local_monotonic_micros);
        }
        due
    }

    /// Remembers each full prize body by its snapshot id and replaces each
    /// `PrizeReference` with the full `PrizeState` it names, carrying the new
    /// energy, so everything after this sees a complete prize state as before.
    /// The server names only bodies from frames the client acknowledged; one
    /// this session no longer holds is dropped, and the server's next full
    /// body repairs it.
    fn expand_prize_references(&mut self, mut frame: SnapshotFrame) -> SnapshotFrame {
        // More than a second of 60 Hz full bodies, sent while a change awaits
        // acknowledgement, so the acknowledged one the server names is still
        // here.
        const RETAINED_PRIZE_BODIES: usize = 64;
        for event in &mut frame.events {
            match event {
                SnapshotEvent::PrizeState(prize) => {
                    self.prize_bodies
                        .retain(|(snapshot_id, _)| *snapshot_id != frame.header.snapshot_id);
                    self.prize_bodies
                        .push_back((frame.header.snapshot_id, *prize));
                    while self.prize_bodies.len() > RETAINED_PRIZE_BODIES {
                        self.prize_bodies.pop_front();
                    }
                }
                SnapshotEvent::PrizeReference(reference) => {
                    if let Some((_, body)) = self
                        .prize_bodies
                        .iter()
                        .find(|(snapshot_id, _)| *snapshot_id == reference.snapshot_id)
                    {
                        *event = SnapshotEvent::PrizeState(SnapshotPrizeState {
                            energy: Some(reference.energy),
                            ..*body
                        });
                    }
                }
                _ => {}
            }
        }
        frame
            .events
            .retain(|event| !matches!(event, SnapshotEvent::PrizeReference(_)));
        frame.header.event_count = frame.events.len() as u8;
        frame
    }

    fn reset_snapshot_state(&mut self) {
        self.snapshots_since_last_ack = 0;
        self.latest_received_snapshot_id = None;
        self.latest_received_snapshot_server_tick = None;
        self.latest_received_snapshot_base_id = None;
        self.latest_acked_snapshot_id = None;
        self.latest_acked_snapshot_server_tick = None;
        self.latest_repel_snapshot_id = None;
        self.snapshot_history.clear();
        self.prize_bodies.clear();
        self.last_snapshot_full_request_micros = None;
        self.snapshot_entity_carry.clear();
        self.recent_damage_identities.clear();
        self.recent_damage_identity_set.clear();
        self.latest_damage_sequence = None;
    }

    fn trim_snapshot_history(&mut self, server_active_base_id: Option<u16>) {
        while self.snapshot_history.len() > SNAPSHOT_HISTORY_CAPACITY {
            let removable = self
                .snapshot_history
                .iter()
                .position(|entry| Some(entry.snapshot_id) != server_active_base_id)
                .expect("client history over capacity must have a non-active entry");
            self.snapshot_history.remove(removable);
        }
    }

    fn emit_snapshot_events(&mut self, frame: &SnapshotFrame, actions: &mut ClientProtocolActions) {
        for event in &frame.events {
            if let SnapshotEvent::WatchDamage(wd) = event {
                if !self.remember_damage_identity(
                    DamageEventCarrier::Snapshot,
                    wd.victim_id,
                    wd.victim_life_seq,
                    wd.damage_seq,
                ) {
                    continue;
                }
                let report = ServerDamageReport {
                    player_id: wd.victim_id,
                    timestamp: wd.server_tick.unwrap_or(frame.header.tick),
                    entries: vec![ServerDamageReportEntry {
                        attacker_id: wd.shooter_id,
                        weapon_data: wd.weapon_data,
                        energy: wd.energy_after,
                        damage: wd.damage,
                        timestamp_offset: 0,
                        energy_before: wd.energy_before,
                        damage_seq: wd.damage_seq,
                        projectile_id: wd.projectile_id,
                        victim_life_seq: wd.victim_life_seq,
                        shooter_life_seq: wd.shooter_life_seq,
                        explicit_lethal: Some(wd.lethal),
                        source_snapshot_id: Some(frame.header.snapshot_id),
                        source_snapshot_epoch: Some(self.snapshot_stream_epoch),
                    }],
                };
                actions
                    .events
                    .push(ClientProtocolEvent::DamageWatched(report));
            }
        }
    }

    fn remember_damage_identity(
        &mut self,
        carrier: DamageEventCarrier,
        victim_player_id: u16,
        victim_life_seq: Option<u32>,
        damage_seq: Option<u32>,
    ) -> bool {
        let (Some(victim_life_seq), Some(damage_seq)) = (victim_life_seq, damage_seq) else {
            return true;
        };
        let identity = DamageEventIdentity {
            carrier,
            victim_player_id,
            victim_life_seq,
            damage_seq,
        };
        if !self.recent_damage_identity_set.insert(identity) {
            return false;
        }
        if let Some(latest) = self.latest_damage_sequence {
            let advance = damage_seq.wrapping_sub(latest);
            if advance != 0 && advance < (1 << 31) {
                self.latest_damage_sequence = Some(damage_seq);
            } else {
                let distance = latest.wrapping_sub(damage_seq) as usize;
                if distance >= DAMAGE_EVENT_IDENTITY_CACHE {
                    self.recent_damage_identity_set.remove(&identity);
                    return false;
                }
            }
        } else {
            self.latest_damage_sequence = Some(damage_seq);
        }
        self.recent_damage_identities.push_back(identity);
        while self.recent_damage_identities.len() > DAMAGE_EVENT_IDENTITY_CACHE {
            if let Some(retired) = self.recent_damage_identities.pop_front() {
                self.recent_damage_identity_set.remove(&retired);
            }
        }
        true
    }

    pub(crate) fn apply_player_death(&mut self, death: PlayerDeath) {
        self.detach_roster_player_and_children(death.killed_id);
        if let Some(killed) = self.roster.get_mut(&death.killed_id) {
            killed.losses = killed.losses.saturating_add(1);
            killed.flags = 0;
        }

        if let Some(killer) = self.roster.get_mut(&death.killer_id) {
            killer.wins = killer.wins.saturating_add(1);
            if death.killer_id != death.killed_id {
                killer.flags = killer.flags.saturating_add(death.flag_transfer);
            }
        }
    }

    pub(crate) fn apply_flag_claim(&mut self, claim: FlagClaim) {
        if self.turf_flag_ids.contains(&claim.flag_id) {
            return;
        }

        if let Some(entry) = self.roster.get_mut(&claim.player_id) {
            entry.flags = entry.flags.saturating_add(1);
        }
    }

    pub(crate) fn advance_zone_file_requests(
        &mut self,
        actions: &mut ClientProtocolActions,
        local_tick: u32,
    ) -> Result<(), String> {
        self.queue_next_file_request(actions, local_tick)
    }

    fn queue_next_file_request(
        &mut self,
        actions: &mut ClientProtocolActions,
        local_tick: u32,
    ) -> Result<(), String> {
        loop {
            let Some(request) = self.pending_file_requests.front().cloned() else {
                return Ok(());
            };
            if self.file_request_in_flight == Some(request.index) {
                return Ok(());
            }
            if self.sequencer.sent.len() >= ReliableSequencer::MAX_SENT_RELIABLES {
                return Ok(());
            }
            #[cfg(target_arch = "wasm32")]
            if let Some(identity) = request.cache_identity() {
                if let Some(bytes) = crate::transport::load_browser_cached_map(
                    &identity,
                    crate::zone_wire::MAX_TRANSFER_FILE_BYTES,
                ) {
                    match self.accept_cached_zone_file(&request, bytes, actions, local_tick) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(error) => {
                            crate::transport::remove_browser_cached_map(&identity);
                            return Err(error);
                        }
                    }
                    crate::transport::remove_browser_cached_map(&identity);
                }
            }
            let Some((_, packet)) = self
                .sequencer
                .try_send_tracked_reliable_at_tick(&request.encode_request_packet(), local_tick)
            else {
                return Ok(());
            };
            actions.outbound_packets.push(packet);
            self.file_request_in_flight = Some(request.index);
            self.file_request_frames_since_send = 0;
            self.file_request_reissues = 0;
            actions
                .events
                .push(ClientProtocolEvent::MapRequestQueued(request));
            return Ok(());
        }
    }

    #[cfg(any(test, target_arch = "wasm32"))]
    pub(crate) fn accept_cached_zone_file(
        &mut self,
        request: &FileRequestSpec,
        bytes: Vec<u8>,
        actions: &mut ClientProtocolActions,
        local_tick: u32,
    ) -> Result<bool, String> {
        if self.sequencer.sent.len() >= ReliableSequencer::MAX_SENT_RELIABLES
            || self.file_request_in_flight.is_some()
            || (request.index != 0 && !self.server_held_zone_files)
            || request.cache_identity().is_none()
            || self.pending_file_requests.front() != Some(request)
            || !request.matches_download_checksum(&bytes)
            || !request.matches_download_digest(&bytes)
            || (!request.decompress
                && request.size_is_known
                && bytes.len() != request.size as usize)
        {
            return Ok(false);
        }
        if request.index == 0 {
            if !self.accept_cached_map(request, bytes, actions, local_tick) {
                return Ok(false);
            }
            self.pending_file_requests.pop_front();
        } else {
            let Some(map_sha256) = self.pending_zone_map_sha256 else {
                return Ok(false);
            };
            self.pending_file_requests.pop_front();
            self.accept_zone_file_bytes(request.clone(), bytes, actions)?;
            // The common parser does not allocate C2S ids. Capacity checked
            // above therefore remains available for this exact receipt.
            actions.push_reliable_at_tick(
                &mut self.sequencer,
                &crate::zone_wire::HeldZoneFile {
                    zone_id: request.zone_id.clone().unwrap(),
                    map_sha256,
                    index: request.index,
                    sha256: request.sha256.unwrap(),
                }
                .encode(),
                local_tick,
            );
        }
        Ok(true)
    }

    fn release_validated_lvz_downloads(&mut self, actions: &mut ClientProtocolActions) {
        actions.events.extend(
            std::mem::take(&mut self.pending_zone_downloads)
                .into_iter()
                .map(ClientProtocolEvent::MapDownloaded),
        );
    }

    fn accept_zone_file_bytes(
        &mut self,
        request: FileRequestSpec,
        decoded: Vec<u8>,
        actions: &mut ClientProtocolActions,
    ) -> Result<(), String> {
        if !request.matches_download_checksum(&decoded) {
            let message = format!("downloaded file checksum mismatch: {}", request.filename);
            self.reject_file_response(request);
            return Err(message);
        }
        if !request.matches_download_digest(&decoded) {
            let message = format!("downloaded file SHA-256 mismatch: {}", request.filename);
            self.reject_file_response(request);
            return Err(message);
        }

        let outcome = (|| -> Result<(), String> {
            let parsed = match parse_downloaded_file_structural(&request.filename, &decoded) {
                Ok(parsed) => parsed,
                Err(err) => {
                    #[cfg(target_arch = "wasm32")]
                    if request.index == 0
                        && let Some(cache_identity) = request.cache_identity()
                    {
                        crate::transport::remove_browser_cached_map(&cache_identity);
                    }
                    return Err(format!(
                        "downloaded file {:?} is malformed: {err}",
                        request.filename
                    ));
                }
            };
            let (
                next_render_bytes,
                next_pixel_bytes,
                render_image_replacements,
                reset_render_images,
                next_lvz_usage,
            ) = match &parsed {
                DownloadedFile::Level(map) => {
                    let render_bytes = map.retained_render_bytes().map_err(|err| {
                        format!("downloaded map representation is invalid: {err}")
                    })?;
                    let images = BTreeMap::new();
                    let totals = crate::render_limits::checked_zone_render_totals(
                        render_bytes
                            .checked_sub(map.embedded_tileset_pixel_bytes)
                            .ok_or_else(|| "downloaded map representation underflow".to_string())?,
                        map.embedded_tileset_pixel_bytes,
                        &images,
                    )?;
                    (
                        totals.0,
                        totals.1,
                        images,
                        true,
                        crate::lvz::LvzRenderUsage::default(),
                    )
                }
                DownloadedFile::Lvz(archive) => {
                    let usage = archive.render_usage().map_err(|err| {
                        format!("downloaded LVZ representation is invalid: {err}")
                    })?;
                    let mut aggregate_usage = usage;
                    aggregate_usage.pixel_bytes = 0;
                    aggregate_usage.texture_count = 0;
                    aggregate_usage = match self.pending_zone_lvz_usage.checked_add(aggregate_usage)
                    {
                        Ok(usage) => usage,
                        Err(err) => {
                            self.abort_pending_zone_transfer();
                            return Err(format!("downloaded LVZ aggregate is invalid: {err}"));
                        }
                    };
                    if aggregate_usage.decoded_source_bytes
                        > crate::zone_wire::MAX_ZONE_DECODED_LVZ_BYTES
                    {
                        let decoded_source_bytes = aggregate_usage.decoded_source_bytes;
                        self.abort_pending_zone_transfer();
                        return Err(format!(
                            "downloaded LVZ files expand to {decoded_source_bytes} bytes; maximum is {}",
                            crate::zone_wire::MAX_ZONE_DECODED_LVZ_BYTES
                        ));
                    }
                    let asset_nonpixel_bytes = usage
                        .retained_bytes()?
                        .checked_sub(usage.pixel_bytes)
                        .ok_or_else(|| "downloaded LVZ representation underflow".to_string())?;
                    let mut images = BTreeMap::new();
                    for (name, bytes) in archive.render_image_allocations()? {
                        images.insert(name, bytes);
                    }
                    let existing_image_bytes = self
                        .pending_zone_render_images
                        .values()
                        .try_fold(0_usize, |total, bytes| total.checked_add(*bytes))
                        .ok_or_else(|| "downloaded zone pixel size overflow".to_string())?;
                    let map_pixel_bytes = self
                        .pending_zone_pixel_bytes
                        .checked_sub(existing_image_bytes)
                        .ok_or_else(|| "downloaded zone pixel size overflow".to_string())?;
                    let nonpixel_bytes = self
                        .pending_zone_render_bytes
                        .checked_sub(self.pending_zone_pixel_bytes)
                        .and_then(|bytes| bytes.checked_add(asset_nonpixel_bytes))
                        .ok_or_else(|| "downloaded zone render size overflow".to_string())?;
                    let totals =
                        crate::render_limits::checked_zone_render_totals_with_replacements(
                            nonpixel_bytes,
                            map_pixel_bytes,
                            &self.pending_zone_render_images,
                            &images,
                        )?;
                    crate::render_limits::checked_zone_render_peak(
                        self.pending_zone_render_bytes,
                        totals.0,
                        images.values().copied().max().unwrap_or(0),
                    )?;
                    (totals.0, totals.1, images, false, aggregate_usage)
                }
            };
            if let DownloadedFile::Level(map) = &parsed {
                self.set_map_security_shared_tiles(
                    &map.tiles,
                    map.width,
                    map.height,
                    map.continuum_crc32(),
                );
            }
            self.pending_zone_render_bytes = next_render_bytes;
            self.pending_zone_render_high_water_bytes = if reset_render_images {
                next_render_bytes
            } else {
                self.pending_zone_render_high_water_bytes
                    .max(next_render_bytes)
            };
            self.pending_zone_pixel_bytes = next_pixel_bytes;
            if reset_render_images {
                self.pending_zone_render_images = render_image_replacements;
            } else {
                self.pending_zone_render_images
                    .extend(render_image_replacements);
            }
            self.pending_zone_lvz_usage = next_lvz_usage;

            let cache_identity = request.cache_identity();
            let is_level = matches!(&parsed, DownloadedFile::Level(_));
            let download = Arc::new(ValidatedDownloadedFile::new(
                request.filename.clone(),
                decoded,
                parsed,
            ));
            if let Some(identity) = cache_identity {
                self.pending_zone_cache_candidates
                    .push((identity, Arc::clone(&download)));
            }
            if is_level {
                download.seal_full_renderability().map_err(|err| {
                    format!("downloaded map full render validation failed: {err}")
                })?;
                // Keep a shared candidate until the complete manifest
                // passes. Browser storage is publication state: writing
                // here would leave a partial cache entry if a later LVZ
                // pushes the zone over its decoded aggregate ceiling.
                self.pending_zone_map_download = Some(Arc::clone(&download));
                self.pending_zone_map_needs_cache_store =
                    self.pending_zone_map_cache_identity.is_some();
                actions
                    .events
                    .push(ClientProtocolEvent::MapDownloaded(download));
                actions.events.push(ClientProtocolEvent::ZoneMapPublished);
            } else {
                self.pending_zone_downloads.push(download);
            }
            if self.pending_file_requests.is_empty() {
                // Structural/header preflight and the exact aggregate
                // budget have succeeded for the complete manifest. Full
                // decoders are now permitted, in manifest order.
                let mut image_budget =
                    crate::render_limits::ImagePreparationBudget::for_zone_high_water(
                        self.pending_zone_render_high_water_bytes,
                    );
                for download in &self.pending_zone_downloads {
                    if let Err(err) = download.seal_with_prepared_images(&mut image_budget) {
                        self.pending_zone_downloads.clear();
                        return Err(format!(
                            "downloaded LVZ full render validation failed: {err}"
                        ));
                    }
                }
                // An all-cached zone can finish here before the reliable spawn
                // boundary releases its cached map. Keep overlays behind it.
                if self.pending_cached_map_install.is_none() {
                    self.release_validated_lvz_downloads(actions);
                }
                self.publish_zone_files_if_complete(actions);
            }
            Ok(())
        })();
        if outcome.is_err() {
            self.abort_pending_zone_transfer();
        }
        outcome
    }

    fn reject_file_response(&mut self, request: FileRequestSpec) {
        self.file_response_rejects = self.file_response_rejects.saturating_add(1);
        self.pending_file_requests.push_front(request);
    }

    /// Next zone-file request for the frame pump to re-send, if the queue holds work and nothing is already outstanding.
    ///
    /// Inbound processing is circular for exactly the case that strands a session: a completed file that fails to decode or
    /// fails an integrity check puts its request back, but the retry could previously only be sent by another inbound packet,
    /// and no further inbound file arrives while the transfer is stuck. The frame pump calls this every rendered frame, for
    /// the same reason it flushes reliable acknowledgements.
    pub(crate) fn take_file_request_retry(&mut self) -> Option<(u32, Vec<u8>)> {
        const FILE_REQUEST_REISSUE_AFTER_FRAMES: u32 = 90;
        const FILE_REQUEST_REISSUE_BUDGET: u8 = 4;
        let reissue = self.file_request_in_flight.is_some();
        if reissue {
            self.file_request_frames_since_send =
                self.file_request_frames_since_send.saturating_add(1);
            // A lost response is the other half of this stall: nothing arrived, so nothing cleared the outstanding mark, and
            // the client would wait on a packet that is never coming. Re-issue on a bounded frame budget instead. The server
            // treats a re-request for a file it already delivered as a bounded re-drive, so this cannot loop without end.
            if self.file_request_frames_since_send < FILE_REQUEST_REISSUE_AFTER_FRAMES
                || self.file_request_reissues >= FILE_REQUEST_REISSUE_BUDGET
            {
                return None;
            }
            self.file_request_reissues = self.file_request_reissues.saturating_add(1);
        }
        let request = self.pending_file_requests.front().cloned()?;
        // The request the server accepts travels through the application reliable sequencer, and a retry has to travel the
        // same way. A raw write of the same three bytes never reached the server while acknowledgements written on the same
        // socket kept arriving, which made a retry look like a drop on a healthy lane.
        let (reliable_id, packet) =
            self.try_send_tracked_reliable_payload(&request.encode_request_packet())?;
        self.file_request_in_flight = Some(request.index);
        self.file_request_frames_since_send = 0;
        if !reissue {
            self.file_request_reissues = 0;
        }
        Some((reliable_id, packet))
    }

    /// Re-arm the bounded file-request retry cycle after an authority wait expired with no delivery to retry.
    ///
    /// `take_file_request_retry` re-issues a request at most `FILE_REQUEST_REISSUE_BUDGET` times and then returns
    /// `None` forever. That is right for a transfer that is genuinely complete and wrong for a client whose join is
    /// still waiting on the arena's files: the client asks four times, stops asking, and stays unready, with nothing
    /// else in the session to re-arm it. Closing `file_request_in_flight` also restores the pre-reissue state, where
    /// the next call sends immediately, so the re-arm asks rather than waiting out another frame budget.
    pub(crate) fn rearm_file_request_retry(&mut self) {
        self.file_request_in_flight = None;
        self.file_request_reissues = 0;
        self.file_request_frames_since_send = 0;
    }

    /// Give a queued request back after its send failed, so a congested lane retries on a later frame instead of losing it.
    pub(crate) fn cancel_file_request_retry(&mut self) {
        self.file_request_in_flight = None;
    }

    fn release_pending_cached_map_install(&mut self, actions: &mut ClientProtocolActions) {
        if let Some(download) = self.pending_cached_map_install.take() {
            actions
                .events
                .push(ClientProtocolEvent::MapDownloaded(download));
            actions.events.push(ClientProtocolEvent::ZoneMapPublished);
        }
        if self.pending_file_requests.is_empty() {
            self.release_validated_lvz_downloads(actions);
        }
        self.publish_zone_files_if_complete(actions);
        // `accept_cached_map` is also a focused native entry point. Outside a
        // live manifest there is no later ZoneFilesPublished boundary to own
        // this shared candidate, so release it with the install event.
        if !self.pending_zone_publication {
            self.pending_zone_cache_candidates.clear();
            self.pending_zone_map_download = None;
            self.pending_zone_map_cache_identity = None;
            self.pending_zone_map_needs_cache_store = false;
        }
    }

    fn publish_zone_files_if_complete(&mut self, actions: &mut ClientProtocolActions) {
        if self.pending_zone_publication
            && self.pending_file_requests.is_empty()
            && self.pending_cached_map_install.is_none()
            && self.pending_zone_downloads.is_empty()
        {
            #[cfg(target_arch = "wasm32")]
            for (identity, download) in &self.pending_zone_cache_candidates {
                crate::transport::store_browser_cached_map(identity, download.data());
            }
            self.pending_zone_cache_candidates.clear();
            self.pending_zone_map_download = None;
            self.pending_zone_map_cache_identity = None;
            self.pending_zone_map_needs_cache_store = false;
            self.pending_zone_publication = false;
            actions.events.push(ClientProtocolEvent::ZoneFilesPublished);
        }
    }

    fn clear_pending_zone_transfer_state(&mut self) {
        self.sequencer.sent.retain(|sent| {
            !matches!(
                sent.payload.first().copied(),
                Some(FileRequestSpec::MAP_REQUEST_TYPE) | Some(CachedMapCheck::TYPE)
            )
        });
        self.pending_file_requests = VecDeque::new();
        self.file_transfer_assembler.clear();
        self.pending_cached_map_install = None;
        self.pending_zone_map_download = None;
        self.pending_zone_map_sha256 = None;
        self.pending_zone_cache_candidates.clear();
        self.pending_zone_map_cache_identity = None;
        self.pending_zone_map_needs_cache_store = false;
        self.pending_zone_downloads = Vec::new();
        self.pending_zone_publication = false;
        self.pending_zone_render_bytes = 0;
        self.pending_zone_render_high_water_bytes = 0;
        self.pending_zone_pixel_bytes = 0;
        self.pending_zone_render_images = BTreeMap::new();
        self.pending_zone_lvz_usage = crate::lvz::LvzRenderUsage::default();
    }

    fn abort_pending_zone_transfer(&mut self) {
        #[cfg(target_arch = "wasm32")]
        if let Some(cache_identity) = self.pending_zone_map_cache_identity.as_deref() {
            crate::transport::remove_browser_cached_map(cache_identity);
        }
        self.clear_pending_zone_transfer_state();
        self.map_tiles = None;
        self.map_width = 0;
        self.map_height = 0;
        self.map_security_checksum = 0;
        self.continuum_map_crc32 = 0;
    }

    pub(crate) fn accept_cached_map(
        &mut self,
        request: &FileRequestSpec,
        cached: Vec<u8>,
        actions: &mut ClientProtocolActions,
        local_tick: u32,
    ) -> bool {
        if request.index != 0
            || !request.matches_download_checksum(&cached)
            || !request.matches_download_digest(&cached)
        {
            return false;
        }
        let Ok(parsed) = parse_downloaded_file(&request.filename, &cached) else {
            return false;
        };
        let DownloadedFile::Level(map) = &parsed else {
            return false;
        };
        let Ok(render_bytes) = map.retained_render_bytes() else {
            return false;
        };
        let Some(nonpixel_bytes) = render_bytes.checked_sub(map.embedded_tileset_pixel_bytes)
        else {
            return false;
        };
        let render_images = BTreeMap::<Arc<str>, usize>::new();
        let Ok((render_bytes, pixel_bytes)) = crate::render_limits::checked_zone_render_totals(
            nonpixel_bytes,
            map.embedded_tileset_pixel_bytes,
            &render_images,
        ) else {
            return false;
        };
        self.set_map_security_shared_tiles(
            &map.tiles,
            map.width,
            map.height,
            map.continuum_crc32(),
        );
        actions.push_reliable_at_tick(
            &mut self.sequencer,
            &CachedMapCheck {
                zone_id: request
                    .zone_id
                    .clone()
                    .expect("cache identity requires an authenticated zone"),
                sha256: request
                    .sha256
                    .expect("cache identity requires an authenticated digest"),
            }
            .encode(),
            local_tick,
        );
        // A normal reliable map transfer drains before the arena lifecycle
        // boundary. Live ships release after SetCoordinates so spawn ordering
        // stays exact; spectators (which never receive coordinates) release
        // at the following ArenaDirectory packet.
        let download = Arc::new(ValidatedDownloadedFile::new(
            request.filename.clone(),
            cached,
            parsed,
        ));
        self.pending_zone_cache_candidates
            .push((request.cache_identity().unwrap(), Arc::clone(&download)));
        self.pending_cached_map_install = Some(Arc::clone(&download));
        self.pending_zone_map_download = Some(download);
        self.pending_zone_map_cache_identity = request.cache_identity();
        self.pending_zone_map_needs_cache_store = false;
        self.pending_zone_render_bytes = render_bytes;
        self.pending_zone_render_high_water_bytes = render_bytes;
        self.pending_zone_pixel_bytes = pixel_bytes;
        self.pending_zone_render_images.clear();
        self.pending_zone_lvz_usage = crate::lvz::LvzRenderUsage::default();
        true
    }

    fn push_local_detach_request_if_attached(
        &mut self,
        actions: &mut ClientProtocolActions,
        player_id: u16,
        local_tick: u32,
    ) {
        if self.self_player_id != Some(player_id)
            || self
                .roster
                .get(&player_id)
                .is_none_or(|entry| entry.attach_parent == INVALID_PLAYER_ID)
        {
            return;
        }

        actions.push_reliable_at_tick(
            &mut self.sequencer,
            &AttachRequest {
                player_id: INVALID_PLAYER_ID,
            }
            .encode_payload(),
            local_tick,
        );
    }

    fn detach_roster_player_and_children(&mut self, player_id: u16) {
        if let Some(entry) = self.roster.get_mut(&player_id) {
            entry.attach_parent = INVALID_PLAYER_ID;
        }
        for entry in self.roster.values_mut() {
            if entry.attach_parent == player_id {
                entry.attach_parent = INVALID_PLAYER_ID;
            }
        }
    }

    pub(crate) fn handle_transport_packet(
        &mut self,
        packet: &[u8],
    ) -> Result<ClientProtocolActions, String> {
        self.handle_transport_packet_at_tick(packet, self.outbound_protocol_tick)
    }

    pub(crate) fn handle_transport_packet_at_tick(
        &mut self,
        packet: &[u8],
        local_tick: u32,
    ) -> Result<ClientProtocolActions, String> {
        self.handle_plain_transport_packet_at_tick(packet, local_tick)
    }

    pub(crate) fn handle_transport_packet_from_lane_at_tick(
        &mut self,
        packet: &[u8],
        local_tick: u32,
        lane: crate::transport::PacketTransportLane,
    ) -> Result<ClientProtocolActions, String> {
        if !matches!(
            lane,
            crate::transport::PacketTransportLane::Game
                | crate::transport::PacketTransportLane::Event
        ) {
            return self.handle_plain_transport_packet_at_tick(packet, local_tick);
        }

        // Small/huge chunk framing belongs to the ordered reliable
        // DataChannel. A realtime packet may be delivered between two
        // reliable callbacks, but it is not a logical boundary in that
        // independent byte stream. Isolate the reliable assembler while the
        // game/event packet is parsed; malformed or unrelated packets on the
        // reliable lane still abort fail closed in the ordinary path.
        let reliable_chunker = std::mem::take(&mut self.chunker);
        let result = self.handle_plain_transport_packet_at_tick(packet, local_tick);
        self.chunker.clear_incomplete();
        self.chunker = reliable_chunker;
        if result.as_ref().is_ok_and(|actions| {
            actions
                .events
                .iter()
                .any(|event| matches!(event, ClientProtocolEvent::ConnectionQualityDirective(_)))
        }) {
            return Err("connection-quality directives require the reliable lane".into());
        }
        result
    }

    fn handle_plain_transport_packet_at_tick(
        &mut self,
        packet: &[u8],
        local_tick: u32,
    ) -> Result<ClientProtocolActions, String> {
        if packet.first().copied() != Some(ReliableSequencer::CORE_BYTE) {
            self.chunker.abort_incomplete_for_packet(packet);
            return self.handle_server_packet_at_tick(packet, local_tick);
        }
        if packet.len() < 2 {
            self.chunker.abort_incomplete_for_packet(packet);
            return Err("core packet too short".to_string());
        }

        // A reliable wrapper is only an envelope: its ordered payload below
        // is the logical boundary. Every other direct core packet is itself a
        // boundary (including ACK/time/disconnect/unknown), while the chunker
        // preserves the matching continuation and cluster types explicitly.
        if packet[1] != ReliableSequencer::RELIABLE_MESSAGE_TYPE {
            self.chunker.abort_incomplete_for_packet(packet);
        }

        let mut actions = ClientProtocolActions::default();
        match packet[1] {
            0x07 => {
                self.state = ClientProtocolState::Disconnected;
                actions.events.push(ClientProtocolEvent::Disconnected);
            }
            ReliableSequencer::RELIABLE_MESSAGE_TYPE => {
                let reliable_id = match read_u32(packet, 2) {
                    Ok(id) => id,
                    Err(err) => {
                        self.chunker.clear_incomplete();
                        return Err(err);
                    }
                };
                let payload = &packet[ReliableSequencer::RELIABLE_HEADER_SIZE..];
                let distance_ahead = reliable_id.wrapping_sub(self.sequencer.next_process_id);
                let priority_application_receipt = (distance_ahead != 0
                    && distance_ahead < (1_u32 << 31))
                    .then(|| exact_priority_application_receipt(payload))
                    .flatten();
                if let Err(err) = self.sequencer.receive_reliable(packet) {
                    self.chunker.clear_incomplete();
                    return Err(err);
                }
                actions
                    .outbound_packets
                    .extend(self.sequencer.encode_outbound_acks());
                for (ordered_id, payload) in self.sequencer.drain_ordered_with_ids() {
                    self.chunker.abort_incomplete_for_packet(&payload);
                    if self.priority_dispatched_reliable_ids.remove(&ordered_id) {
                        continue;
                    }
                    // A control-looking payload is application authority only
                    // when its packet-specific exact-length parser succeeds. Retire and ACK a
                    // malformed/trailing wrapper as ordinary reliable traffic,
                    // but never let it enter the priority path, emit an event,
                    // or discard the transport ACK through an application
                    // parse error.
                    if malformed_application_receipt_prefix(&payload) {
                        continue;
                    }
                    if self.dispatch_ordered_stale_remote_weapon_cutoff(&payload, &mut actions) {
                        continue;
                    }
                    actions
                        .append(self.handle_plain_transport_packet_at_tick(&payload, local_tick)?);
                }
                if let Some(receipt) = priority_application_receipt {
                    // These exact private controls are identity-bound and do
                    // not mutate shared roster/world order. Let them cross an
                    // unrelated reliable-prefix gap so lifecycle, Warp, and
                    // owner movement authority cannot remain blocked behind a
                    // full reorder queue. The ordinary
                    // sequencer still retains/ACKs (or retries) the wrapper,
                    // and duplicate delivery is idempotent at the marker.
                    if self.priority_dispatched_reliable_ids.insert(reliable_id) {
                        actions.events.push(receipt);
                    }
                }
            }
            ReliableSequencer::RELIABLE_ACK_TYPE => {
                self.sequencer.receive_ack(packet)?;
            }
            TimeSyncResponse::REQUEST_TYPE => {
                // Never publish a client-owned wall-clock/tick in this legacy
                // response slot. If a peer asks, echo the latest server tick
                // this session has already observed; otherwise send zero.
                let observed_server_tick = self.latest_received_snapshot_server_tick.unwrap_or(0);
                let response =
                    TimeSyncResponse::response_to_request(packet, observed_server_tick)?.encode();
                actions.push_unreliable(response);
            }
            TimeSyncResponse::RESPONSE_TYPE => {
                let response = TimeSyncResponse::parse_response(packet)?;
                actions.events.push(ClientProtocolEvent::TimeSyncReceived(
                    ScopedTimeSyncResponse {
                        response,
                        server_session_id: self
                            .latest_server_clock_bootstrap
                            .map(|bootstrap| bootstrap.server_session_id),
                        arena_stream_id: self
                            .latest_server_clock_bootstrap
                            .map(|bootstrap| bootstrap.arena_stream_id),
                    },
                ));
            }
            PacketChunker::SMALL_BODY_TYPE | PacketChunker::SMALL_TAIL_TYPE => {
                if let Some(body) = self.chunker.push_small_chunk(packet)? {
                    actions.append(self.handle_plain_transport_packet_at_tick(&body, local_tick)?);
                }
            }
            PacketChunker::HUGE_TYPE => {
                if let Some(body) = self.chunker.push_huge_chunk(packet)? {
                    actions.append(self.handle_plain_transport_packet_at_tick(&body, local_tick)?);
                }
            }
            PacketChunker::HUGE_CANCEL_TYPE => {
                actions.push_unreliable(self.chunker.cancel_huge_chunk());
            }
            PacketChunker::HUGE_CANCEL_ACK_TYPE => {
                self.chunker.huge.clear();
            }
            0x0d => {}
            PacketChunker::PACKET_CLUSTER_TYPE => {
                let clustered_packets = match PacketChunker::split_packet_cluster(packet) {
                    Ok(clustered) => clustered,
                    Err(err) => {
                        self.chunker.clear_incomplete();
                        return Err(err);
                    }
                };
                for clustered in clustered_packets {
                    self.chunker.abort_incomplete_for_packet(clustered);
                    actions
                        .append(self.handle_plain_transport_packet_at_tick(clustered, local_tick)?);
                }
            }
            packet_type => {
                actions
                    .events
                    .push(ClientProtocolEvent::UnknownCorePacket(packet_type));
            }
        }

        Ok(actions)
    }
}

fn snapshot_id_advances(previous: Option<u16>, candidate: u16) -> bool {
    previous.map_or(true, |last| snapshot_id_is_forward(candidate, last))
}
