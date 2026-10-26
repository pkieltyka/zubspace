use super::*;
use std::sync::Arc;

/// The server can retain 3,072 recipient-local realtime facts. Cross-channel
/// copies may be overtaken by that entire queue, so keep the same fixed
/// power-of-two horizon as authenticated realtime event ordering.
pub(crate) const DAMAGE_EVENT_IDENTITY_CACHE: usize =
    crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS;
const REALTIME_EVENT_WINDOW_BITS: usize = crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientProtocolState {
    Disconnected,
    LoginSent,
    ArenaLoginSent,
    RegistrationRequested,
    RegistrationForm,
    Failed(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClientProtocolEvent {
    Disconnected,
    TimeSyncReceived(ScopedTimeSyncResponse),
    PlayerIdAssigned(u16),
    JoinedArena(u32),
    PlayerEntered(PlayerRosterEntry),
    PlayerLeft(u16),
    PasswordAccepted,
    RegistrationRequired,
    RegistrationFormRequested,
    LoginFailed(u8),
    Chat(ChatPacket),
    ScoreUpdated(ScoreUpdate),
    ScoresReset(ResetScore),
    SoccerGoal(SoccerGoal),
    PlayerVoice(PlayerVoice),
    FrequencyChanged(FrequencyChange),
    ArenaSettingsUpdated(ContinuumArenaSettings),
    FileTransferred(ServerFileTransfer),
    SecurityResponseQueued,
    ClientFileRequested(ClientFileRequest),
    ShipFrequencyChanged(ShipFrequencyChange),
    FrequencyCutoverAcknowledged(FrequencyCutoverAck),
    ShipChangeRejected(ShipChangeRejected),
    ConnectionQualityDirective(ConnectionQualityDirective),
    SpawnIdleSpectator(SpawnIdleSpectator),
    LifecycleBaselineAccepted(LifecycleBaselineAccepted),
    WarpRelocationAccepted(WarpRelocationAccepted),
    RepelApplicationAccepted(RepelApplicationAccepted),
    MovementCorrected(MovementCorrection),
    MovementCorrectionAccepted(MovementCorrectionAccepted),
    TurretLinkCreated(CreateTurretLink),
    TurretLinkDestroyed(DestroyTurretLink),
    SpectateUpdated(SpectatePacket),
    LoginFailureMessage(LoginFailurePacket),
    ServerVersion(ServerVersionPacket),
    PersonalBannerChanged(PersonalBannerChange),
    PlayerBannerChanged(PlayerBannerChange),
    SendDamageToggled(ToggleSendDamagePacket),
    UfoToggled(ToggleUfoPacket),
    PersonalKothTimerSet(SetPersonalKothTimer),
    KothGameReset(KothGameReset),
    KothTimeAdded(AddKothTime),
    DamageWatched(ServerDamageReport),
    /// SECURITY is carried on a different RTC channel from arena lifecycle.
    /// Capture the protocol session/arena generation at decode time so a
    /// delayed packet cannot later be rebound to whichever arena happens to
    /// occupy the reusable `Game` instance.
    SecurityChallengeReceived {
        challenge: SecurityChallenge,
        server_session_id: Option<u64>,
        arena_epoch: u32,
    },
    /// An authenticated map manifest opened a replacement-zone transaction.
    /// Gameplay must remain closed until every advertised file is validated
    /// and the ordered publication batch below is applied.
    ZoneFilesPending,
    MapRequestQueued(FileRequestSpec),
    MapDownloaded(Arc<ValidatedDownloadedFile>),
    /// The validated level event immediately before this marker has been
    /// published. Gameplay may bind to that map while the independently
    /// withheld LVZ archive batch continues downloading.
    ZoneMapPublished,
    /// Follows the final `MapDownloaded` event for one complete manifest.
    ZoneFilesPublished,
    PlayerPosition(PlayerPositionUpdate),
    /// A position materialized from one decoded snapshot, not a legacy
    /// timestamped datagram. Preserve its full-width publication identity.
    SnapshotPlayerPosition {
        update: PlayerPositionUpdate,
        snapshot_id: u16,
        server_tick: u32,
        arena_stream_id: u64,
    },
    /// Immutable one-shot weapon fact authenticated to one arena stream and
    /// deduplicated independently of snapshot ordering.
    RealtimeWeaponFired {
        server_session_id: u64,
        arena_stream_id: u64,
        event_id: u32,
        server_tick: u32,
        source: crate::realtime_event::RealtimeWeaponEvent,
    },
    /// Exact source-session/shot-group cutoff for silent weapon cleanup.
    RealtimeOwnerWeaponsCancelled {
        server_session_id: u64,
        arena_stream_id: u64,
        event_id: u32,
        server_tick: u32,
        cancellation: crate::realtime_event::RealtimeOwnerWeaponCancellation,
    },
    /// Replaceable current presentation state for one exact authoritative
    /// projectile. Unlike an original weapon carrier, this event is
    /// self-contained and does not wait for a renderable source actor.
    RealtimeProjectileState {
        server_session_id: u64,
        arena_stream_id: u64,
        event_id: u32,
        server_tick: u32,
        state: crate::realtime_event::RealtimeProjectileStateEvent,
    },
    /// Server-owned post-Repel presentation state. It is independently
    /// ordered so it may arrive before the weapon carrier it identifies.
    RealtimeRepelMutations {
        server_session_id: u64,
        arena_stream_id: u64,
        event_id: u32,
        server_tick: u32,
        batch: crate::realtime_event::RealtimeRepelMutationBatch,
    },
    BatchedSmallPlayerPosition(PlayerPositionUpdate),
    BatchedLargePlayerPosition(PlayerPositionUpdate),
    PlayerDeath(PlayerDeath),
    PlayerPrize(PlayerPrize),
    PrizePickupAck(ScopedPrizePickupAck),
    PrizeEffectAccepted(PrizeEffectAccepted),
    PortalPlacementAccepted(PortalPlacementAccepted),
    PortalWarpAccepted(PortalWarpAccepted),
    HiddenPrizeSpawn(HiddenPrizeSpawn),
    FlagPosition(FlagPosition),
    FlagClaim(FlagClaim),
    FlagVictory(FlagVictory),
    DropFlag(DropFlag),
    ShipReset,
    CollectedPrize(CollectedPrize),
    BrickDropped(BrickDropped),
    TurfFlagUpdate(TurfFlagUpdate),
    FlagReward(FlagReward),
    SpeedGameOver(SpeedGameOver),
    LvzToggled(LvzTogglePacket),
    LvzModified(LvzModifyPacket),
    PowerballPosition(PowerballPositionUpdate),
    ObjectiveState(ObjectiveStateUpdate),
    ObjectiveScore(ObjectiveScoreUpdate),
    ArenaDirectory(ArenaDirectoryListing),
    ZoneBannerAdvertised(ZoneBannerAdvertisement),
    KeepAlive,
    PostLogin,
    SetCoordinates(SetCoordinates),
    Redirect(ServerRedirectPacket),
    SelectBox(SelectBoxPacket),
    SnapshotReceived(SnapshotFrame),
    /// Browser-only staging event extracted from an authenticated snapshot.
    /// The private prize state must reconcile before the next fixed gameplay
    /// step, while the snapshot's entities and all other events retain their
    /// established post-step boundary.
    PreGameplayPrizeState {
        snapshot_id: u16,
        snapshot_tick: u32,
        state: SnapshotPrizeState,
    },
    /// Browser-only fail-closed marker emitted once when an immutable poll
    /// plan contains a hard authority replacement.  It invalidates private
    /// prediction before the fixed step; the real lifecycle/session events
    /// retain their established post-step application boundary.
    PreGameplayAuthorityReset,
    ServerClockBootstrapped(ServerClockBootstrap),
    QaTraceContextReceived(QaTraceContext),
    UnknownPacket(u8),
    UnknownCorePacket(u8),
}

/// Arena/runtime identity captured when a private pickup acknowledgement is
/// decoded. The packet carries the complete identity and this wrapper captures
/// the receiver's contemporaneous bootstrap/player scope independently.
/// Requiring both prevents a delayed payload from being rebound to a
/// replacement arena, server session, or reused numeric player id in the
/// long-lived `Game` instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopedPrizePickupAck {
    pub(crate) ack: PrizePickupAck,
    pub(crate) server_session_id: Option<u64>,
    pub(crate) arena_stream_id: Option<u64>,
    pub(crate) player_id: Option<u16>,
}

/// Time-sync replies are admitted only for the exact request/session scope in
/// which their echoed nonce was issued. Decode captures the protocol scope;
/// the browser clock also binds that nonce to its immutable Game generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopedTimeSyncResponse {
    pub(crate) response: TimeSyncResponse,
    pub(crate) server_session_id: Option<u64>,
    pub(crate) arena_stream_id: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientProtocolActions {
    pub(crate) outbound_packets: Vec<Vec<u8>>,
    pub(crate) events: Vec<ClientProtocolEvent>,
    /// Immutable browser authority admission computed once from the Game
    /// scope that existed before this poll batch mutates anything. The vector
    /// is either empty (native/protocol callers that do not partition) or is
    /// exactly aligned with `events`, including after deferred-batch append.
    pub(crate) authority_admission: Vec<AuthorityEventAdmission>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AuthorityScopeIdentity {
    pub(crate) server_session_id: Option<u64>,
    pub(crate) arena_stream_id: Option<u64>,
    pub(crate) player_id: Option<u16>,
    pub(crate) life_seq: Option<u32>,
    pub(crate) alive: Option<bool>,
}

impl AuthorityScopeIdentity {
    pub(crate) fn is_complete_live(self) -> bool {
        self.server_session_id.is_some()
            && self.arena_stream_id.is_some()
            && self.player_id.is_some()
            && self.life_seq.is_some()
            && self.alive == Some(true)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PrivateAuthorityAdmission {
    #[default]
    Irrelevant,
    Reject,
    Current(AuthorityScopeIdentity),
    Replacement(AuthorityScopeIdentity),
}

/// Ordered admission for facts that participate in the gameplay-readiness
/// barrier. These facts are not recipient-private, but they are just as bound
/// to the immutable hard-boundary segment in which they were observed. In
/// particular, wrapped/reused numeric SECURITY identity cannot rebind an old
/// segment to a replacement arena.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AuthorityReadinessAdmission {
    #[default]
    Irrelevant,
    Reject,
    Current,
    Replacement,
}

/// Immutable authority binding for every decoded action. Gameplay/world facts
/// are bound to the exact generation and scope in which they were observed;
/// only an explicit transport/session-neutral allowlist is `Neutral`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AuthorityActionAdmission {
    #[default]
    Reject,
    Neutral,
    Bound(AuthorityScopeIdentity),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AuthorityBoundaryAdmission {
    #[default]
    None,
    Soft,
    Hard {
        /// Internal monotonic generation allocated by the immutable batch
        /// planner. This is deliberately not derived from wrapped/reusable
        /// network ids.
        segment: u64,
        replacement: Option<AuthorityScopeIdentity>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AuthorityReadiness {
    pub(crate) joined: bool,
    pub(crate) arena_epoch: Option<u32>,
    pub(crate) settings: bool,
    pub(crate) security: bool,
    pub(crate) map: bool,
}

impl AuthorityReadiness {
    pub(crate) fn complete(self) -> bool {
        self.joined && self.settings && self.security && self.map
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AuthorityEventAdmission {
    /// Ordered hard-boundary generation in which this action was observed.
    /// Segment zero is the pre-batch authority; every hard boundary advances
    /// this value before classifying that boundary and subsequent actions.
    pub(crate) segment: u64,
    pub(crate) action: AuthorityActionAdmission,
    pub(crate) private_fact: PrivateAuthorityAdmission,
    pub(crate) readiness_fact: AuthorityReadinessAdmission,
    pub(crate) boundary: AuthorityBoundaryAdmission,
    pub(crate) readiness_after: AuthorityReadiness,
}

impl ClientProtocolActions {
    pub(crate) fn push_reliable_at_tick(
        &mut self,
        sequencer: &mut ReliableSequencer,
        payload: &[u8],
        server_tick: u32,
    ) {
        self.outbound_packets
            .push(sequencer.send_reliable_at_tick(payload, server_tick));
    }

    pub(crate) fn push_unreliable(&mut self, packet: Vec<u8>) {
        self.outbound_packets.push(packet);
    }

    pub(crate) fn append(&mut self, mut other: ClientProtocolActions) {
        if !self.authority_admission.is_empty() || !other.authority_admission.is_empty() {
            if self.authority_admission.is_empty() {
                self.authority_admission
                    .resize(self.events.len(), AuthorityEventAdmission::default());
            }
            if other.authority_admission.is_empty() {
                other
                    .authority_admission
                    .resize(other.events.len(), AuthorityEventAdmission::default());
            }
            self.authority_admission
                .append(&mut other.authority_admission);
        }
        self.outbound_packets.append(&mut other.outbound_packets);
        self.events.append(&mut other.events);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientProtocolSession {
    pub(crate) continuum_packet_format: bool,
    pub(crate) arena_login: ArenaLoginRequest,
    pub(crate) sequencer: ReliableSequencer,
    pub(crate) chunker: PacketChunker,
    pub(crate) state: ClientProtocolState,
    pub(crate) weapons_received: u32,
    pub(crate) settings_checksum: u32,
    pub(crate) exe_checksum: u32,
    pub(crate) map_security_checksum: u32,
    pub(crate) continuum_map_crc32: u32,
    pub(crate) map_tiles: Option<Arc<Vec<u8>>>,
    pub(crate) map_width: usize,
    pub(crate) map_height: usize,
    pub(crate) stats: SecurityPacketStats,
    pub(crate) last_settings_packet: Option<Vec<u8>>,
    /// Stable diagnostic fingerprint of the exact received settings packet.
    /// Compute once on replacement, never from the hot HUD/render path.
    pub(crate) arena_settings_crc32: Option<u32>,
    pub(crate) pending_file_requests: VecDeque<FileRequestSpec>,
    /// The zone-file index whose request is currently outstanding. A completed file the client could not use puts its request
    /// back into the queue, and the frame pump re-sends only when nothing is already outstanding, so retries cannot pile up.
    pub(crate) file_request_in_flight: Option<u16>,
    /// Frames since the outstanding zone-file request was sent, counted by the frame pump. A response that never arrives
    /// leaves the request outstanding forever, and without a bounded re-issue the client waits on a packet that is gone.
    /// Frame counting is used because the protocol tick is itself frozen during exactly this stall.
    pub(crate) file_request_frames_since_send: u32,
    /// Re-issues already performed for the current outstanding request, bounded so a genuinely absent asset cannot make the
    /// client transmit forever.
    pub(crate) file_request_reissues: u8,
    /// Counts completed-file packets that arrived but failed to decode or failed an integrity check. Each one used to consume
    /// its request and abort the transfer with a red line of text on screen and nothing else: the queue walked forward exactly
    /// once, so a stranded session stopped sending zone-file requests altogether.
    pub(crate) file_response_rejects: u32,
    pub(crate) file_transfer_assembler: ServerFileTransferAssembler,
    pub(crate) pending_cached_map_install: Option<Arc<ValidatedDownloadedFile>>,
    /// Validated map retained only until the complete zone manifest commits.
    /// This defers browser-cache publication so a later aggregate LVZ failure
    /// cannot leave a partially admitted zone in persistent storage.
    pub(crate) pending_zone_map_download: Option<Arc<ValidatedDownloadedFile>>,
    pub(crate) pending_zone_map_cache_identity: Option<String>,
    pub(crate) pending_zone_map_sha256: Option<crate::zone_wire::Sha256Digest>,
    pub(crate) server_held_zone_files: bool,
    pub(crate) pending_zone_cache_candidates: Vec<(String, Arc<ValidatedDownloadedFile>)>,
    pub(crate) pending_zone_map_needs_cache_store: bool,
    pub(crate) pending_zone_publication: bool,
    /// Counts every `FileTransferChunk` rejected because its index was not the front of `pending_file_requests`,
    /// plus the no-pending-request case. The live RTC path can only pop the front request, so a stalled queue is
    /// indistinguishable from a lost-head queue in the counters that already exist; this separates them.
    pub(crate) file_chunk_index_mismatches: u32,
    /// Counts chunks that passed the front-index check and reached the assembler. With
    /// `file_chunk_index_mismatches` at zero, this separates a silent transfer (nothing arrives) from an
    /// assembler that receives data but never completes a file.
    pub(crate) file_chunks_accepted: u32,
    /// Counts chunks that reached the assembler and were rejected by it (offset gap, identity change, orphan).
    pub(crate) file_chunk_assembler_rejects: u32,
    /// Realtime (`0x7d`) events on the game or event lane that failed to decode.
    /// Each is skipped on its own, so one bad event cannot stall the poll.
    pub(crate) realtime_event_rejects: u32,
    pub(crate) last_realtime_event_reject: Option<String>,
    /// Structurally parsed downloads retained in manifest order until every
    /// LVZ's aggregate decode-work/header budget has passed. Only then are all
    /// archives sealed and published together.
    pub(crate) pending_zone_downloads: Vec<Arc<ValidatedDownloadedFile>>,
    pub(crate) pending_zone_render_bytes: usize,
    /// Maximum admitted manifest prefix, including textures later replaced
    /// by smaller files. Prepared pixel headroom must not use only the final
    /// (potentially smaller) zone total while earlier archives install.
    pub(crate) pending_zone_render_high_water_bytes: usize,
    pub(crate) pending_zone_pixel_bytes: usize,
    pub(crate) pending_zone_render_images: BTreeMap<Arc<str>, usize>,
    pub(crate) pending_zone_lvz_usage: crate::lvz::LvzRenderUsage,
    pub(crate) self_player_id: Option<u16>,
    pub(crate) extra_position_info: bool,
    pub(crate) take_prize_reliable: bool,
    pub(crate) roster: BTreeMap<u16, PlayerRosterEntry>,
    pub(crate) turf_flag_ids: BTreeSet<u16>,
    /// Counts received snapshots between acks so the client can emit a
    /// `SnapshotAckPacket` every `SNAPSHOT_ACK_INTERVAL` frames (about
    /// 12 Hz at the normal 60 Hz near cadence).
    /// Reset to 0 each time an ack is sent.
    pub(crate) snapshots_since_last_ack: u8,
    /// Latest `snapshot_id` decoded from a `ServerPacket::Snapshot`,
    /// for piggybacking the ack value.
    pub(crate) latest_received_snapshot_id: Option<u16>,
    /// Latest server tick carried by a decoded snapshot frame.
    pub(crate) latest_received_snapshot_server_tick: Option<u32>,
    /// Latest snapshot base id. FULL is represented independently in the
    /// frame flags; this value is ignored for full frames.
    pub(crate) latest_received_snapshot_base_id: Option<u16>,
    /// Latest snapshot id acknowledged back to the server.
    pub(crate) latest_acked_snapshot_id: Option<u16>,
    /// Server tick carried by the latest snapshot id acknowledged back
    /// to the server.
    pub(crate) latest_acked_snapshot_server_tick: Option<u32>,
    /// Latest exact snapshot id whose repel weapon has been applied locally.
    /// Position updates carry this as an atomic delivery/application witness.
    pub(crate) latest_repel_snapshot_id: Option<u16>,
    /// Server-owned clock/session metadata received during arena join.
    /// Used for client prediction/tracing only; gameplay authority still
    /// comes from server-stamped snapshots and damage events.
    pub(crate) latest_server_clock_bootstrap: Option<ServerClockBootstrap>,
    /// Tick used only for client-side reliable resend timers and reliable
    /// messages generated as reactions to server packets. Browser clients
    /// set this from the latest server tick they have observed, avoiding
    /// local browser time in protocol bookkeeping.
    pub(crate) outbound_protocol_tick: u32,
    /// Applied full entity states keyed by snapshot id. Incoming deltas are
    /// reconstructed only against the exact `base_id` entry.
    pub(crate) snapshot_history: VecDeque<ClientSnapshotHistoryEntry>,
    /// The last full prize bodies received, by carrying snapshot id, so a
    /// `PrizeReference` can be expanded back into the `PrizeState` it names.
    pub(crate) prize_bodies: VecDeque<(u16, SnapshotPrizeState)>,
    /// Unwrapped local monotonic time supplied by the transport edge. It is
    /// clamped forward so reordered callbacks cannot move recovery pacing
    /// backward.
    pub(crate) local_monotonic_micros: u64,
    /// Local monotonic time when the latest missing-base recovery request was
    /// sent. Elapsed subtraction remains safe even if the injected clock has
    /// saturated at `u64::MAX`.
    pub(crate) last_snapshot_full_request_micros: Option<u64>,
    /// Phase 3e-7: per-entity carry-forward of the most recently
    /// applied near-tier state. Used to fill unmasked fields when
    /// synthesizing a `PlayerPositionUpdate` from a delta snapshot.
    /// Cleared on each explicitly flagged full snapshot.
    pub(crate) snapshot_entity_carry: BTreeMap<u16, SnapshotEntityCarry>,
    /// Exact life-scoped WATCH_DAMAGE receipts already published on each
    /// carrier. Snapshot and authenticated realtime copies remain independent
    /// until Game authority admission; only that boundary owns logical
    /// cross-carrier idempotence.
    pub(crate) recent_damage_identities: VecDeque<DamageEventIdentity>,
    pub(crate) recent_damage_identity_set: BTreeSet<DamageEventIdentity>,
    /// Newest global authoritative damage sequence decoded in this arena
    /// generation. Once a same-carrier identity ages out of the bounded set,
    /// this watermark still rejects copies older than the supported horizon.
    pub(crate) latest_damage_sequence: Option<u32>,
    /// Wrapping 256-id window for independently sequenced one-shot events.
    /// It is reset only when a new authenticated arena bootstrap is accepted.
    pub(crate) realtime_event_window: RealtimeEventWindow,
    /// Exact out-of-order receipt wrappers already dispatched across an
    /// unrelated reliable-prefix gap. Entries remain until that id retires in
    /// ordinary order, preventing duplicates and the later drain from
    /// producing a second application event.
    pub(crate) priority_dispatched_reliable_ids: BTreeSet<u32>,
    /// Monotonic generation for the currently joined arena's snapshot stream.
    /// Snapshot-derived lifecycle facts carry this value so an old arena's
    /// delayed event can never mutate the new roster generation.
    pub(crate) snapshot_stream_epoch: u32,
}

/// Phase 3e-7: snapshot of all near-tier fields the client tracks
/// between successive snapshot frames. Mid-tier entries leave the
/// non-position fields at zero, matching the server's mid encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SnapshotEntityCarry {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) velocity_x: i16,
    pub(crate) velocity_y: i16,
    pub(crate) direction: u8,
    pub(crate) energy: u16,
    pub(crate) weapon: u16,
    pub(crate) status: u8,
    pub(crate) bounty: u16,
    pub(crate) ping: u8,
    pub(crate) flag_timer: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClientSnapshotHistoryEntry {
    pub(crate) snapshot_id: u16,
    pub(crate) entities: SnapshotEntityStateMap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DamageEventCarrier {
    Reliable,
    Realtime,
    Snapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DamageEventIdentity {
    carrier: DamageEventCarrier,
    victim_player_id: u16,
    victim_life_seq: u32,
    damage_seq: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RealtimeEventWindow {
    latest: Option<u32>,
    seen: [u64; REALTIME_EVENT_WINDOW_BITS / 64],
}

impl Default for RealtimeEventWindow {
    fn default() -> Self {
        Self {
            latest: None,
            seen: [0; REALTIME_EVENT_WINDOW_BITS / 64],
        }
    }
}

impl RealtimeEventWindow {
    pub(crate) fn is_older_than_window(&self, candidate: u32) -> bool {
        candidate != 0
            && self.latest.is_some_and(|latest| {
                let distance = latest.wrapping_sub(candidate);
                distance >= REALTIME_EVENT_WINDOW_BITS as u32 && distance < (1_u32 << 31)
            })
    }

    fn bit(&self, distance: usize) -> bool {
        self.seen[distance / 64] & (1_u64 << (distance % 64)) != 0
    }

    fn set_bit(&mut self, distance: usize) {
        self.seen[distance / 64] |= 1_u64 << (distance % 64);
    }

    pub(crate) fn admit(&mut self, candidate: u32) -> bool {
        if candidate == 0 {
            return false;
        }
        let Some(latest) = self.latest else {
            self.latest = Some(candidate);
            self.set_bit(0);
            return true;
        };
        let advance = candidate.wrapping_sub(latest);
        if advance != 0 && advance < (1_u32 << 31) {
            let advance = advance as usize;
            let old = self.seen;
            self.seen = [0; REALTIME_EVENT_WINDOW_BITS / 64];
            if advance < REALTIME_EVENT_WINDOW_BITS {
                for distance in 0..REALTIME_EVENT_WINDOW_BITS - advance {
                    if old[distance / 64] & (1_u64 << (distance % 64)) != 0 {
                        self.set_bit(distance + advance);
                    }
                }
            }
            self.latest = Some(candidate);
            self.set_bit(0);
            return true;
        }
        let distance = latest.wrapping_sub(candidate) as usize;
        if distance >= REALTIME_EVENT_WINDOW_BITS || self.bit(distance) {
            return false;
        }
        self.set_bit(distance);
        true
    }
}

#[allow(dead_code)]
impl ClientProtocolSession {
    pub(crate) fn new(
        continuum_packet_format: bool,
        arena_login: ArenaLoginRequest,
        map_security_checksum: u32,
        continuum_map_crc32: u32,
    ) -> Self {
        Self {
            continuum_packet_format,
            arena_login,
            sequencer: ReliableSequencer::default(),
            chunker: PacketChunker::default(),
            state: ClientProtocolState::Disconnected,
            weapons_received: 0,
            settings_checksum: 0,
            exe_checksum: 0,
            map_security_checksum,
            continuum_map_crc32,
            map_tiles: None,
            map_width: 0,
            map_height: 0,
            stats: SecurityPacketStats::default(),
            last_settings_packet: None,
            arena_settings_crc32: None,
            pending_file_requests: VecDeque::new(),
            file_request_in_flight: None,
            file_request_frames_since_send: 0,
            file_request_reissues: 0,
            file_response_rejects: 0,
            file_transfer_assembler: ServerFileTransferAssembler::default(),
            pending_cached_map_install: None,
            pending_zone_map_download: None,
            pending_zone_map_cache_identity: None,
            pending_zone_map_sha256: None,
            server_held_zone_files: false,
            pending_zone_cache_candidates: Vec::new(),
            pending_zone_map_needs_cache_store: false,
            pending_zone_publication: false,
            file_chunk_index_mismatches: 0,
            file_chunks_accepted: 0,
            file_chunk_assembler_rejects: 0,
            realtime_event_rejects: 0,
            last_realtime_event_reject: None,
            pending_zone_downloads: Vec::new(),
            pending_zone_render_bytes: 0,
            pending_zone_render_high_water_bytes: 0,
            pending_zone_pixel_bytes: 0,
            pending_zone_render_images: BTreeMap::new(),
            pending_zone_lvz_usage: crate::lvz::LvzRenderUsage::default(),
            self_player_id: None,
            extra_position_info: false,
            take_prize_reliable: false,
            roster: BTreeMap::new(),
            turf_flag_ids: BTreeSet::new(),
            snapshots_since_last_ack: 0,
            latest_received_snapshot_id: None,
            latest_received_snapshot_server_tick: None,
            latest_received_snapshot_base_id: None,
            latest_acked_snapshot_id: None,
            latest_acked_snapshot_server_tick: None,
            latest_repel_snapshot_id: None,
            latest_server_clock_bootstrap: None,
            // Starts inert until a server-owned tick is observed via join
            // bootstrap, time sync, or snapshots. Client wall-clock time must
            // not seed reliable resend bookkeeping.
            outbound_protocol_tick: 0,
            snapshot_history: VecDeque::new(),
            prize_bodies: VecDeque::new(),
            local_monotonic_micros: 0,
            last_snapshot_full_request_micros: None,
            snapshot_entity_carry: BTreeMap::new(),
            recent_damage_identities: VecDeque::new(),
            recent_damage_identity_set: BTreeSet::new(),
            latest_damage_sequence: None,
            realtime_event_window: RealtimeEventWindow::default(),
            priority_dispatched_reliable_ids: BTreeSet::new(),
            snapshot_stream_epoch: 0,
        }
    }

    pub(crate) fn set_outbound_protocol_tick(&mut self, tick: u32) {
        self.outbound_protocol_tick = tick & 0x7fff_ffff;
    }

    pub(crate) fn set_local_monotonic_micros(&mut self, now_micros: u64) {
        self.local_monotonic_micros = self.local_monotonic_micros.max(now_micros);
    }

    /// Reliable acknowledgements queued by inbound processing, taken for transmission. The frame pump drives this in
    /// addition to inbound handling; see `Client::flush_reliable_acks` for why.
    pub(crate) fn take_outbound_ack_packets(&mut self) -> Vec<Vec<u8>> {
        let mut packets = std::mem::take(&mut self.sequencer.pending_ack_retries);
        packets.extend(self.sequencer.encode_outbound_acks());
        // Bounded so a persistently unwritable channel cannot grow this without limit; the oldest batches are the ones a
        // peer has most likely already been told about by a later cumulative arrival.
        const MAX_PENDING_ACK_PACKETS: usize = 32;
        if packets.len() > MAX_PENDING_ACK_PACKETS {
            let excess = packets.len() - MAX_PENDING_ACK_PACKETS;
            packets.drain(0..excess);
        }
        packets
    }

    /// Record an acknowledgement batch whose write failed, so the next frame can try it again.
    pub(crate) fn requeue_failed_ack_packet(&mut self, packet: Vec<u8>) {
        self.sequencer.ack_send_failures = self.sequencer.ack_send_failures.saturating_add(1);
        const MAX_PENDING_ACK_RETRIES: usize = 32;
        let pending = &mut self.sequencer.pending_ack_retries;
        pending.push(packet);
        if pending.len() > MAX_PENDING_ACK_RETRIES {
            let excess = pending.len() - MAX_PENDING_ACK_RETRIES;
            pending.drain(0..excess);
        }
    }

    pub(crate) fn ack_send_failures(&self) -> u32 {
        self.sequencer.ack_send_failures
    }

    /// Emitted-ack counters for the failure evidence, paired with the peer's own receive record.
    pub(crate) fn emitted_ack_stats(&self) -> (u32, u32) {
        (
            self.sequencer.acks_emitted,
            self.sequencer.acks_emitted_max_id,
        )
    }

    pub(crate) fn reliable_prefix_acked_through(&self, inclusive_id: u32) -> bool {
        self.sequencer.outbound_prefix_acked_through(inclusive_id)
    }

    pub(crate) fn tracked_reliable_capacity_available(&self) -> bool {
        self.sequencer.sent.len() < ReliableSequencer::MAX_SENT_RELIABLES
    }

    pub(super) fn try_send_tracked_reliable_payload(
        &mut self,
        payload: &[u8],
    ) -> Option<(u32, Vec<u8>)> {
        self.sequencer
            .try_send_tracked_reliable_at_tick(payload, self.outbound_protocol_tick)
    }

    pub(crate) fn rollback_tracked_reliable(&mut self, reliable_id: u32) -> bool {
        self.sequencer.rollback_tracked_reliable(reliable_id)
    }

    pub(super) fn send_reliable_payload(&mut self, payload: &[u8]) -> Vec<u8> {
        self.sequencer
            .send_reliable_at_tick(payload, self.outbound_protocol_tick)
    }

    pub(super) fn send_reliable_chunked_payload(&mut self, payload: &[u8]) -> Vec<Vec<u8>> {
        self.sequencer
            .send_reliable_chunked_at_tick(payload, self.outbound_protocol_tick)
    }

    pub(crate) fn set_security_context(
        &mut self,
        weapons_received: u32,
        exe_checksum: u32,
        stats: SecurityPacketStats,
    ) {
        self.weapons_received = weapons_received;
        self.exe_checksum = exe_checksum;
        self.stats = stats;
    }

    pub(crate) fn set_map_security_tiles(
        &mut self,
        tiles: &[u8],
        width: usize,
        height: usize,
        continuum_map_crc32: u32,
    ) {
        self.map_tiles = Some(Arc::new(tiles.to_vec()));
        self.map_width = width;
        self.map_height = height;
        self.continuum_map_crc32 = continuum_map_crc32;
    }

    pub(crate) fn set_map_security_shared_tiles(
        &mut self,
        tiles: &MapTiles,
        width: usize,
        height: usize,
        continuum_map_crc32: u32,
    ) {
        self.map_tiles = Some(tiles.shared());
        self.map_width = width;
        self.map_height = height;
        self.continuum_map_crc32 = continuum_map_crc32;
    }

    fn map_security_checksum_for_key(&self, key: u32) -> u32 {
        self.map_tiles
            .as_ref()
            .map_or(self.map_security_checksum, |tiles| {
                map_security_checksum_for_tiles(tiles, self.map_width, self.map_height, key)
            })
    }
}

mod inbound;
mod outbound;

#[cfg(test)]
#[test]
fn departure_realtime_expiry_distinguishes_duplicates_future_and_wrapping_past_boundary() {
    let mut window = RealtimeEventWindow::default();
    assert!(!window.is_older_than_window(1));
    assert!(window.admit(1));
    assert!(!window.is_older_than_window(0));
    assert!(!window.is_older_than_window(1));
    assert!(!window.is_older_than_window(2));
    assert!(!window.is_older_than_window(1_u32.wrapping_add(1 << 31)));
    assert!(!window.is_older_than_window(u32::MAX - 4093));
    assert!(window.is_older_than_window(u32::MAX - 4094));
    assert!(window.admit(u32::MAX - 4093));
    assert!(!window.admit(u32::MAX - 4094));
}
