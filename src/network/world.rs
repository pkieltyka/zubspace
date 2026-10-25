use super::*;

#[path = "world/flags.rs"]
mod flags;
#[path = "world/lifecycle.rs"]
mod lifecycle;
#[path = "world/metadata.rs"]
mod metadata;
#[path = "world/objectives.rs"]
mod objectives;
#[path = "world/players.rs"]
mod players;
#[path = "world/positions.rs"]
mod positions;
#[path = "world/snapshot.rs"]
mod snapshot;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetworkWorld {
    /// Presentation-only viewer identity installed by an admitted sequencer
    /// command. It suppresses the local player from remote rendering but is
    /// never consulted to admit gameplay or lifecycle facts.
    viewer_player_id: Option<u16>,
    pub(crate) ufo_available: bool,
    pub(crate) personal_koth_timer: Option<u32>,
    pub(crate) koth_timer: Option<u32>,
    pub(crate) koth_timer_adding: bool,
    pub(crate) koth_player_id: Option<u16>,
    personal_koth_remainder: f32,
    koth_remainder: f32,
    pub(crate) players: BTreeMap<u16, NetworkPlayer>,
    pub(crate) chat: Vec<NetworkChatEntry>,
    pub(crate) personal_banner: Option<[u8; PLAYER_BANNER_BYTES]>,
    pub(crate) voice_messages: Vec<PlayerVoice>,
    pub(crate) transferred_files: Vec<ServerFileTransfer>,
    pub(crate) powerballs: BTreeMap<u8, NetworkPowerball>,
    pub(crate) soccer_scores: BTreeMap<u16, u32>,
    pub(crate) arena_directory: Vec<ArenaDirectoryEntry>,
    pub(crate) last_zone_banner: Option<ZoneBannerAdvertisement>,
    pub(crate) client_file_requests: Vec<ClientFileRequest>,
    pub(crate) flags: BTreeMap<u16, NetworkFlag>,
    /// Accepted reliable arena lines, independent of the currently installed
    /// map/render tiles. The u16 wire ID bounds this cache to 65,536 entries;
    /// Clear, expiry and hard arena/session replacement retire records.
    pub(crate) brick_lines: BTreeMap<u16, BrickDrop>,
    pub(crate) objective_versions: BTreeMap<(u8, u16), NetworkObjectiveIdentity>,
    pub(crate) objective_score_versions: BTreeMap<u16, NetworkObjectiveScoreIdentity>,
    pub(crate) last_flag_victory: Option<FlagVictory>,
    pub(crate) last_flag_rewards: Vec<FlagRewardGrant>,
    /// Which code path last fed the flag presentation. The legacy `FlagPosition`
    /// packet carries no version, so a client that presents an on-map unowned flag
    /// while the server holds a `Carried` state can be diagnosed from the browser
    /// alone by comparing these against `flagAuthoritativeCarriedEvents`.
    pub(crate) flag_legacy_position_events: u32,
    pub(crate) flag_legacy_position_guard_rejections: u32,
    pub(crate) flag_authoritative_state_events: u32,
    pub(crate) flag_authoritative_carried_events: u32,
    /// Discriminator for the same defect. `reset_presentation_scope` clears
    /// `objective_versions`, after which any replayed state looks new to the ordering
    /// gate; these say how often that happened, how many authoritative flag states the
    /// gate rejected as older, and what the last accepted flag state actually was.
    pub(crate) flag_version_resets: u32,
    pub(crate) flag_authoritative_stale_rejects: u32,
    pub(crate) flag_last_authoritative_version: u32,
    pub(crate) flag_last_authoritative_state: u8,
    pub(crate) flag_last_authoritative_x: u16,
    pub(crate) flag_last_authoritative_y: u16,
    pub(crate) flag_last_authoritative_tick: u32,
    pub(crate) last_speed_game_over: Option<SpeedGameOver>,
    pub(crate) pending_warp_effects: Vec<Vec2>,
    pub(crate) lvz: NetworkLvzState,
    pub(crate) stats: NetworkWorldStats,
    /// Phase 3e-3: most recent snapshot id observed on the wire.
    /// `None` until the first `SnapshotReceived` event arrives.
    pub(crate) last_received_snapshot_id: Option<u16>,
    /// Exact actor membership of the most recent successfully applied wire
    /// snapshot. This is captured before cloak/xradar/render/attachment
    /// filtering and is independent from the delayed render-proxy TTL.
    pub(crate) authoritative_snapshot_id: Option<u16>,
    /// Server tick of the exact snapshot whose actors were applied to this
    /// `NetworkWorld`. This is a Game/application fact, not the protocol
    /// session's latest decoded or codec-ACKed watermark.
    pub(crate) authoritative_snapshot_server_tick: Option<u32>,
    authoritative_snapshot_stream_id: Option<u64>,
    /// Exact near/mid-tier positions expected immediately after the corresponding
    /// SnapshotReceived action. Consuming this marker distinguishes generated
    /// snapshot samples from legacy position packets with the same timestamp.
    pending_snapshot_position_provenance: BTreeMap<
        u16,
        (
            u16,
            u32,
            Option<crate::combat_timeline::PresentationSampleTime>,
            Option<u32>,
            Option<u32>,
        ),
    >,
    snapshot_epoch: Option<u32>,
    pub(crate) authoritative_snapshot_actors: Vec<AuthoritativeSnapshotActor>,
    /// Monotonic per-arena stream serial and explicit eviction count for the
    /// bounded raw-membership trace. Diagnostics must fail closed if they miss
    /// a serial or observe overflow; ring truncation is never silent.
    pub(crate) authoritative_snapshot_actor_serial: u32,
    pub(crate) authoritative_snapshot_actor_overflow: u32,
    /// Last raw membership frame published to the browser diagnostics bridge.
    /// The ring's overflow counter represents frames evicted before this
    /// consumer observed them, rather than harmless eviction of already
    /// published history.
    pub(crate) authoritative_snapshot_actor_metrics_cursor: std::cell::Cell<Option<(u32, u32)>>,
    /// Bounded, exact membership for every successfully applied snapshot.
    /// Rendering can consume several unordered RTC packets in one browser
    /// frame, so retaining only the latest membership would hide an
    /// authoritative leave/reentry between two UI-metric publications.
    pub(crate) authoritative_snapshot_actor_trace: VecDeque<AuthoritativeSnapshotActorMembership>,
    /// Phase 3e-4b: per-entity world position carried forward by
    /// snapshot deltas. A new snapshot's `field_mask = 0` entry
    /// means the entity's position is unchanged since the base
    /// snapshot, and the client reads the current value from this
    /// map. A frame with the explicit FULL flag resets the map to the
    /// listed entities' positions. Near-tier snapshots synthesize
    /// `PlayerPositionUpdate` events, so the existing render path
    /// consumes snapshot-driven `NetworkPlayer.position`.
    pub(crate) snapshot_entity_state: BTreeMap<u16, (u16, u16)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuthoritativeSnapshotActor {
    pub(crate) player_id: u16,
    pub(crate) life_seq: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SnapshotPresentabilitySummary {
    pub(crate) snapshot_id: Option<u16>,
    pub(crate) server_tick: Option<u32>,
    pub(crate) remote_actor_count: usize,
    pub(crate) presentable_remote_actor_count: usize,
    pub(crate) unresolved_life_count: usize,
    pub(crate) missing_position_count: usize,
}

impl SnapshotPresentabilitySummary {
    pub(crate) fn snapshot_is_presentable(self) -> bool {
        self.snapshot_id.is_some()
            && self.remote_actor_count == self.presentable_remote_actor_count
            && self.unresolved_life_count == 0
            && self.missing_position_count == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NetworkObjectiveIdentity {
    pub(crate) generation: u64,
    pub(crate) version: u32,
    pub(crate) last_update_tick: u32,
    pub(crate) last_action_id: u32,
    pub(crate) last_outcome: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NetworkObjectiveScoreIdentity {
    pub(crate) generation: u64,
    pub(crate) version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuthoritativeSnapshotActorMembership {
    pub(crate) epoch: u32,
    pub(crate) serial: u32,
    pub(crate) snapshot_id: u16,
    pub(crate) actors: Vec<AuthoritativeSnapshotActor>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NetworkWorldStats {
    pub(crate) remote_position_updates: u32,
    pub(crate) remote_position_stale_drops: u32,
    pub(crate) remote_extrapolated_frames: u32,
    pub(crate) remote_snap_count: u32,
    /// Recipient-relative delivery counters, added for the observer that held a remote player with no
    /// position permanently. Frames where this client was offered a remote peer in the near tier, the mid
    /// tier, and frames where a remote peer was known in the world but listed in neither tier, counted
    /// separately because they are different faults: the last is a delivery decision, the others are an
    /// application failure on the client.
    pub(crate) remote_listing_near_frames: u32,
    pub(crate) remote_listing_mid_frames: u32,
    pub(crate) remote_listing_absent_frames: u32,
    /// 1 near, 2 mid, 3 known-but-unlisted in the most recent frame that said anything.
    pub(crate) remote_last_listing_tier: u8,
    pub(crate) last_remote_snap_reason: u8,
}

#[allow(dead_code)]
impl NetworkWorld {
    pub(crate) const CHAT_HISTORY_LIMIT: usize = 64;

    /// Classify the current Game-applied snapshot without consulting protocol
    /// ACK state or renderer draw state. The local viewer is rendered from the
    /// local ship projection and therefore is not a remote presentability
    /// dependency.
    pub(crate) fn snapshot_presentability(&self) -> SnapshotPresentabilitySummary {
        let mut summary = SnapshotPresentabilitySummary {
            snapshot_id: self.authoritative_snapshot_id,
            server_tick: self.authoritative_snapshot_server_tick,
            ..SnapshotPresentabilitySummary::default()
        };
        for actor in &self.authoritative_snapshot_actors {
            if Some(actor.player_id) == self.viewer_player_id {
                continue;
            }
            summary.remote_actor_count += 1;
            let Some(life_seq) = actor.life_seq.filter(|life_seq| *life_seq != 0) else {
                summary.unresolved_life_count += 1;
                continue;
            };
            let Some(player) = self.players.get(&actor.player_id) else {
                summary.missing_position_count += 1;
                continue;
            };
            if player.life_seq != Some(life_seq) {
                summary.unresolved_life_count += 1;
                continue;
            }
            if player.last_seen_snapshot_id != self.authoritative_snapshot_id
                || player.position.is_none()
                || player.display_position().is_none()
            {
                summary.missing_position_count += 1;
                continue;
            }
            summary.presentable_remote_actor_count += 1;
        }
        summary
    }

    pub(crate) fn apply_event(&mut self, event: &ClientProtocolEvent) {
        match event {
            ClientProtocolEvent::Disconnected => {
                self.apply_disconnected_event();
            }
            ClientProtocolEvent::PlayerIdAssigned(player_id) => {
                self.apply_player_id_assigned_event(*player_id);
            }
            ClientProtocolEvent::JoinedArena(snapshot_epoch) => {
                self.apply_arena_presentation_joined_event(*snapshot_epoch);
            }
            ClientProtocolEvent::BrickDropped(BrickDropped::Drop(line)) => {
                self.brick_lines.insert(line.brick_id, *line);
            }
            ClientProtocolEvent::BrickDropped(BrickDropped::Clear) => {
                self.brick_lines.clear();
            }
            ClientProtocolEvent::PlayerEntered(entry) => {
                self.apply_player_entered_event(entry);
            }
            ClientProtocolEvent::PlayerLeft(player_id) => {
                self.apply_player_left_event(*player_id);
            }
            ClientProtocolEvent::ScoreUpdated(score) => {
                self.apply_score_updated_event(score);
            }
            ClientProtocolEvent::ScoresReset(reset) => {
                self.apply_scores_reset_event(reset);
            }
            ClientProtocolEvent::SoccerGoal(goal) => {
                self.apply_soccer_goal_event(goal);
            }
            ClientProtocolEvent::PlayerVoice(voice) => {
                self.apply_player_voice_event(voice);
            }
            ClientProtocolEvent::FileTransferred(file) => {
                self.apply_file_transferred_event(file);
            }
            ClientProtocolEvent::UfoToggled(packet) => {
                self.apply_ufo_toggled_event(packet);
            }
            ClientProtocolEvent::PersonalKothTimerSet(packet) => {
                self.apply_personal_koth_timer_set_event(packet);
            }
            ClientProtocolEvent::KothGameReset(packet) => {
                self.apply_koth_game_reset_event(packet);
            }
            ClientProtocolEvent::KothTimeAdded(packet) => {
                self.apply_koth_time_added_event(packet);
            }
            ClientProtocolEvent::ArenaDirectory(listing) => {
                self.apply_arena_directory_event(listing);
            }
            ClientProtocolEvent::ZoneBannerAdvertised(advertisement) => {
                self.apply_zone_banner_advertised_event(advertisement);
            }
            ClientProtocolEvent::ClientFileRequested(request) => {
                self.apply_client_file_requested_event(request);
            }
            ClientProtocolEvent::PersonalBannerChanged(change) => {
                self.apply_personal_banner_changed_event(change);
            }
            ClientProtocolEvent::PlayerBannerChanged(change) => {
                self.apply_player_banner_changed_event(change);
            }
            ClientProtocolEvent::FrequencyChanged(change) => {
                self.apply_frequency_changed_event(change);
            }
            ClientProtocolEvent::ShipFrequencyChanged(change) => {
                self.apply_ship_frequency_changed_event(change);
            }
            ClientProtocolEvent::TurretLinkCreated(link) => {
                self.apply_turret_link_created_event(link);
            }
            ClientProtocolEvent::TurretLinkDestroyed(link) => {
                self.apply_turret_link_destroyed_event(link);
            }
            ClientProtocolEvent::PlayerPosition(update) => {
                self.apply_player_position_event(update);
            }
            ClientProtocolEvent::SnapshotPlayerPosition {
                update,
                snapshot_id,
                server_tick,
                arena_stream_id,
            } => {
                self.apply_snapshot_player_position_event(
                    update,
                    *snapshot_id,
                    *server_tick,
                    *arena_stream_id,
                );
            }
            ClientProtocolEvent::BatchedSmallPlayerPosition(update) => {
                self.apply_batched_small_player_position_event(update);
            }
            ClientProtocolEvent::BatchedLargePlayerPosition(update) => {
                self.apply_batched_large_player_position_event(update);
            }
            ClientProtocolEvent::PlayerDeath(death) => {
                self.apply_player_death_event(death);
            }
            // WATCH_DAMAGE never mutates energy/position by reusable numeric
            // id. A lethal event does carry an authoritative victim life,
            // though, so use it to keep remote identity current even while a
            // reliable PLAYER_DEATH is delayed behind an ordered gap.
            ClientProtocolEvent::DamageWatched(report) => {
                self.apply_damage_watched_lifecycle_event(report);
            }
            ClientProtocolEvent::FlagPosition(position) => {
                self.apply_flag_position_event(position);
            }
            ClientProtocolEvent::FlagClaim(claim) => {
                self.apply_flag_claim_event(claim);
            }
            ClientProtocolEvent::FlagVictory(victory) => {
                self.apply_flag_victory_event(victory);
            }
            ClientProtocolEvent::DropFlag(drop) => {
                self.apply_drop_flag_event(drop);
            }
            ClientProtocolEvent::TurfFlagUpdate(update) => {
                self.apply_turf_flag_update_event(update);
            }
            ClientProtocolEvent::FlagReward(reward) => {
                self.apply_flag_reward_event(reward);
            }
            ClientProtocolEvent::SpeedGameOver(result) => {
                self.apply_speed_game_over_event(result);
            }
            ClientProtocolEvent::Chat(chat) => {
                self.apply_chat_event(chat);
            }
            ClientProtocolEvent::LvzToggled(packet) => {
                self.apply_lvz_toggled_event(packet);
            }
            ClientProtocolEvent::LvzModified(packet) => {
                self.apply_lvz_modified_event(packet);
            }
            ClientProtocolEvent::PowerballPosition(update) => {
                self.apply_powerball_position_event(update);
            }
            ClientProtocolEvent::ObjectiveState(update) => {
                self.apply_objective_state_event(*update);
            }
            ClientProtocolEvent::ObjectiveScore(update) => {
                self.apply_objective_score_event(*update);
            }
            ClientProtocolEvent::SnapshotReceived(frame) => {
                self.apply_snapshot_received_event(frame);
            }
            ClientProtocolEvent::PasswordAccepted
            | ClientProtocolEvent::TimeSyncReceived(_)
            | ClientProtocolEvent::ServerClockBootstrapped(_)
            | ClientProtocolEvent::QaTraceContextReceived(_)
            | ClientProtocolEvent::RegistrationRequired
            | ClientProtocolEvent::RegistrationFormRequested
            | ClientProtocolEvent::LoginFailed(_)
            | ClientProtocolEvent::LoginFailureMessage(_)
            | ClientProtocolEvent::ServerVersion(_)
            | ClientProtocolEvent::SpectateUpdated(_)
            | ClientProtocolEvent::SendDamageToggled(_)
            | ClientProtocolEvent::ArenaSettingsUpdated(_)
            | ClientProtocolEvent::SecurityChallengeReceived { .. }
            | ClientProtocolEvent::SecurityResponseQueued
            | ClientProtocolEvent::KeepAlive
            | ClientProtocolEvent::PostLogin
            | ClientProtocolEvent::ZoneFilesPending
            | ClientProtocolEvent::MapRequestQueued(_)
            | ClientProtocolEvent::MapDownloaded(_)
            | ClientProtocolEvent::ZoneMapPublished
            | ClientProtocolEvent::ZoneFilesPublished
            | ClientProtocolEvent::RealtimeWeaponFired { .. }
            | ClientProtocolEvent::RealtimeProjectileState { .. }
            | ClientProtocolEvent::RealtimeOwnerWeaponsCancelled { .. }
            | ClientProtocolEvent::RealtimeRepelMutations { .. }
            | ClientProtocolEvent::PlayerPrize(_)
            | ClientProtocolEvent::PrizePickupAck(_)
            | ClientProtocolEvent::PrizeEffectAccepted(_)
            | ClientProtocolEvent::HiddenPrizeSpawn(_)
            | ClientProtocolEvent::CollectedPrize(_)
            | ClientProtocolEvent::ShipReset
            | ClientProtocolEvent::FrequencyCutoverAcknowledged(_)
            | ClientProtocolEvent::ShipChangeRejected(_)
            | ClientProtocolEvent::ConnectionQualityDirective(_)
            | ClientProtocolEvent::SpawnIdleSpectator(_)
            | ClientProtocolEvent::LifecycleBaselineAccepted(_)
            | ClientProtocolEvent::WarpRelocationAccepted(_)
            | ClientProtocolEvent::RepelApplicationAccepted(_)
            | ClientProtocolEvent::MovementCorrected(_)
            | ClientProtocolEvent::MovementCorrectionAccepted(_)
            | ClientProtocolEvent::SetCoordinates(_)
            | ClientProtocolEvent::Redirect(_)
            | ClientProtocolEvent::SelectBox(_)
            | ClientProtocolEvent::PreGameplayAuthorityReset
            | ClientProtocolEvent::PreGameplayPrizeState { .. }
            | ClientProtocolEvent::UnknownPacket(_)
            | ClientProtocolEvent::PortalPlacementAccepted(_)
            | ClientProtocolEvent::PortalWarpAccepted(_)
            | ClientProtocolEvent::UnknownCorePacket(_) => {}
        }
    }
}
