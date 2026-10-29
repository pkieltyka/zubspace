use super::*;

#[derive(Clone, Debug, PartialEq)]
pub enum ServerPacket<'a> {
    PlayerId(PlayerIdAssignment),
    JoinGame,
    PlayerEntering(PlayerRosterEntry),
    PlayerLeaving(PlayerLeaving),
    PasswordResponse(PasswordResponse),
    Chat(ChatPacket),
    ScoreUpdate(ScoreUpdate),
    ResetScore(ResetScore),
    SoccerGoal(SoccerGoal),
    PlayerVoice(PlayerVoice),
    FrequencyChange(FrequencyChange),
    ArenaSettings(ContinuumArenaSettings),
    FileTransfer(ServerFileTransfer),
    FileTransferChunk(ServerFileTransferChunk<'a>),
    NoOp(u8),
    SecurityChallenge(SecurityChallenge),
    ClientFileRequest(ClientFileRequest),
    ShipFrequencyChange(ShipFrequencyChange),
    FrequencyCutoverAck(FrequencyCutoverAck),
    ShipChangeRejected(ShipChangeRejected),
    LifecycleBaselineAccepted(LifecycleBaselineAccepted),
    WarpRelocationAccepted(WarpRelocationAccepted),
    RepelApplicationAccepted(RepelApplicationAccepted),
    MovementCorrection(MovementCorrection),
    MovementCorrectionAccepted(MovementCorrectionAccepted),
    CreateTurretLink(CreateTurretLink),
    DestroyTurretLink(DestroyTurretLink),
    Spectate(SpectatePacket),
    LoginFailure(LoginFailurePacket),
    Version(ServerVersionPacket),
    PersonalBannerChange(PersonalBannerChange),
    PlayerBannerChange(PlayerBannerChange),
    ToggleSendDamage(ToggleSendDamagePacket),
    ToggleUfo(ToggleUfoPacket),
    SetPersonalKothTimer(SetPersonalKothTimer),
    KothGameReset(KothGameReset),
    AddKothTime(AddKothTime),
    WatchDamage(ServerDamageReport),
    MapInformation(MapInformation),
    CompressedMap(CompressedMapPacket<'a>),
    LargePosition(PlayerPositionUpdate),
    PlayerDeath(PlayerDeath),
    PlayerPrize(PlayerPrize),
    PrizePickupAck(PrizePickupAck),
    PrizeEffectAccepted(PrizeEffectAccepted),
    PortalPlacementAccepted(PortalPlacementAccepted),
    PortalWarpAccepted(PortalWarpAccepted),
    HiddenPrizeSpawn(HiddenPrizeSpawn),
    FlagPosition(FlagPosition),
    FlagClaim(FlagClaim),
    FlagVictory(FlagVictory),
    DropFlag(DropFlag),
    ShipReset(ShipReset),
    CollectedPrize(CollectedPrize),
    BrickDropped(BrickDropped),
    TurfFlagUpdate(TurfFlagUpdate),
    FlagReward(FlagReward),
    SpeedGameOver(SpeedGameOver),
    KeepAlive(KeepAlive),
    SmallPosition(PlayerPositionUpdate),
    BatchedSmallPositions(Vec<PlayerPositionUpdate>),
    BatchedLargePositions(Vec<PlayerPositionUpdate>),
    ToggleLvz(LvzTogglePacket),
    ModifyLvz(LvzModifyPacket),
    PowerballPosition(PowerballPositionUpdate),
    ObjectiveState(ObjectiveStateUpdate),
    ObjectiveScore(ObjectiveScoreUpdate),
    ArenaDirectory(ArenaDirectoryListing),
    ZoneBannerAdvertisement(ZoneBannerAdvertisement),
    PostLogin(PostLogin),
    SetCoordinates(SetCoordinates),
    Redirect(ServerRedirectPacket),
    SelectBox(SelectBoxPacket),
    Snapshot(SnapshotFrame),
    RealtimeEvent(crate::realtime_event::AuthenticatedRealtimeEvent),
    ServerClockBootstrap(ServerClockBootstrap),
    QaTraceContext(QaTraceContext),
    ConnectionQualityProbe(ConnectionQualityProbe),
    ConnectionQualityDirective(ConnectionQualityDirective),
    ConnectionQualityWarning(ConnectionQualityWarning),
    SpawnIdleSpectator(SpawnIdleSpectator),
    Unknown { packet_type: u8, payload: &'a [u8] },
}

impl<'a> ServerPacket<'a> {
    pub(crate) const PLAYER_ID: u8 = PlayerIdAssignment::TYPE;
    pub(crate) const JOIN_GAME: u8 = 0x02;
    pub(crate) const PLAYER_ENTERING: u8 = PlayerRosterEntry::TYPE;
    pub(crate) const PLAYER_LEAVING: u8 = PlayerLeaving::TYPE;
    pub(crate) const LARGE_POSITION: u8 = PlayerPositionUpdate::LARGE_TYPE;
    pub(crate) const PLAYER_DEATH: u8 = PlayerDeath::TYPE;
    pub(crate) const CHAT: u8 = ChatPacket::TYPE;
    pub(crate) const PLAYER_PRIZE: u8 = PlayerPrize::TYPE;
    pub(crate) const PRIZE_PICKUP_ACK: u8 = PrizePickupAck::TYPE;
    pub(crate) const PRIZE_EFFECT_ACCEPTED: u8 = PrizeEffectAccepted::TYPE;
    pub(crate) const PORTAL_PLACEMENT_ACCEPTED: u8 = PortalPlacementAccepted::TYPE;
    pub(crate) const PORTAL_WARP_ACCEPTED: u8 = PortalWarpAccepted::TYPE;
    pub(crate) const HIDDEN_PRIZE_SPAWN: u8 = HiddenPrizeSpawn::TYPE;
    pub(crate) const SCORE_UPDATE: u8 = ScoreUpdate::TYPE;
    pub(crate) const RESET_SCORE: u8 = ResetScore::TYPE;
    pub(crate) const PASSWORD_RESPONSE: u8 = 0x0a;
    pub(crate) const SOCCER_GOAL: u8 = SoccerGoal::TYPE;
    pub(crate) const PLAYER_VOICE: u8 = PlayerVoice::TYPE;
    pub(crate) const FREQUENCY_CHANGE: u8 = FrequencyChange::TYPE;
    pub(crate) const ARENA_SETTINGS: u8 = 0x0f;
    pub(crate) const FILE_TRANSFER: u8 = ServerFileTransfer::TYPE;
    pub(crate) const FILE_TRANSFER_CHUNK: u8 = ServerFileTransferChunk::TYPE;
    pub(crate) const NO_OP_11: u8 = 0x11;
    pub(crate) const NO_OP_17: u8 = 0x17;
    pub(crate) const NO_OP_26: u8 = 0x26;
    pub(crate) const SECURITY: u8 = 0x18;
    pub(crate) const REQUEST_FILE: u8 = ClientFileRequest::TYPE;
    pub(crate) const SHIP_FREQUENCY_CHANGE: u8 = ShipFrequencyChange::TYPE;
    pub(crate) const FREQUENCY_CUTOVER_ACK: u8 = FrequencyCutoverAck::TYPE;
    pub(crate) const SHIP_CHANGE_REJECTED: u8 = ShipChangeRejected::TYPE;
    pub(crate) const LIFECYCLE_BASELINE_ACCEPTED: u8 = LifecycleBaselineAccepted::TYPE;
    pub(crate) const WARP_RELOCATION_ACCEPTED: u8 = WarpRelocationAccepted::TYPE;
    pub(crate) const REPEL_APPLICATION_ACCEPTED: u8 = RepelApplicationAccepted::TYPE;
    pub(crate) const MOVEMENT_CORRECTION: u8 = MovementCorrection::TYPE;
    pub(crate) const MOVEMENT_CORRECTION_ACCEPTED: u8 = MovementCorrectionAccepted::TYPE;
    pub(crate) const CREATE_TURRET_LINK: u8 = CreateTurretLink::TYPE;
    pub(crate) const DESTROY_TURRET_LINK: u8 = DestroyTurretLink::TYPE;
    pub(crate) const SPECTATE: u8 = SpectatePacket::TYPE;
    pub(crate) const LOGIN_FAILURE: u8 = LoginFailurePacket::TYPE;
    pub(crate) const VERSION: u8 = ServerVersionPacket::TYPE;
    pub(crate) const PERSONAL_BANNER_CHANGE: u8 = PersonalBannerChange::TYPE;
    pub(crate) const PLAYER_BANNER_CHANGE: u8 = PlayerBannerChange::TYPE;
    pub(crate) const TOGGLE_SEND_DAMAGE: u8 = ToggleSendDamagePacket::TYPE;
    pub(crate) const TOGGLE_UFO: u8 = ToggleUfoPacket::TYPE;
    pub(crate) const SET_PERSONAL_KOTH_TIMER: u8 = SetPersonalKothTimer::TYPE;
    pub(crate) const KOTH_GAME_RESET: u8 = KothGameReset::TYPE;
    pub(crate) const ADD_KOTH_TIME: u8 = AddKothTime::TYPE;
    pub(crate) const WATCH_DAMAGE: u8 = ServerDamageReport::TYPE;
    pub(crate) const COLLECTED_PRIZE: u8 = CollectedPrize::TYPE;
    pub(crate) const BRICK_DROPPED: u8 = BrickDropped::TYPE;
    pub(crate) const SMALL_POSITION: u8 = PlayerPositionUpdate::SMALL_TYPE;
    pub(crate) const MAP_INFORMATION: u8 = 0x29;
    pub(crate) const COMPRESSED_MAP: u8 = 0x2a;
    pub(crate) const FLAG_POSITION: u8 = FlagPosition::TYPE;
    pub(crate) const FLAG_CLAIM: u8 = FlagClaim::TYPE;
    pub(crate) const FLAG_VICTORY: u8 = FlagVictory::TYPE;
    pub(crate) const DROP_FLAG: u8 = DropFlag::TYPE;
    pub(crate) const SHIP_RESET: u8 = ShipReset::TYPE;
    pub(crate) const KEEP_ALIVE: u8 = KeepAlive::TYPE;
    pub(crate) const POWERBALL_POSITION: u8 = PowerballPositionUpdate::TYPE;
    pub(crate) const OBJECTIVE_STATE: u8 = ObjectiveStateUpdate::TYPE;
    pub(crate) const OBJECTIVE_SCORE: u8 = ObjectiveScoreUpdate::TYPE;
    pub(crate) const ARENA_DIRECTORY: u8 = ArenaDirectoryListing::TYPE;
    pub(crate) const ZONE_BANNER_ADVERTISEMENT: u8 = ZoneBannerAdvertisement::TYPE;
    pub(crate) const POST_LOGIN: u8 = PostLogin::TYPE;
    pub(crate) const SET_COORDINATES: u8 = SetCoordinates::TYPE;
    pub(crate) const TURF_FLAG_UPDATE: u8 = TurfFlagUpdate::TYPE;
    pub(crate) const FLAG_REWARD: u8 = FlagReward::TYPE;
    pub(crate) const SPEED_GAME_OVER: u8 = SpeedGameOver::TYPE;
    pub(crate) const TOGGLE_LVZ: u8 = LvzTogglePacket::TYPE;
    pub(crate) const MODIFY_LVZ: u8 = LvzModifyPacket::TYPE;
    pub(crate) const BATCHED_SMALL_POSITION: u8 = PlayerPositionUpdate::BATCHED_SMALL_TYPE;
    pub(crate) const BATCHED_LARGE_POSITION: u8 = PlayerPositionUpdate::BATCHED_LARGE_TYPE;
    pub(crate) const REDIRECT: u8 = ServerRedirectPacket::TYPE;
    pub(crate) const SELECT_BOX: u8 = SelectBoxPacket::TYPE;
    pub(crate) const SNAPSHOT: u8 = SnapshotHeader::TYPE;
    pub(crate) const REALTIME_EVENT: u8 = crate::realtime_event::AuthenticatedRealtimeEvent::TYPE;
    pub(crate) const SERVER_CLOCK_BOOTSTRAP: u8 = ServerClockBootstrap::TYPE;
    pub(crate) const QA_TRACE_CONTEXT: u8 = QaTraceContext::TYPE;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &'a [u8], continuum_packet_format: bool) -> Result<Self, String> {
        let Some(packet_type) = packet.first().copied() else {
            return Err("empty server packet".to_string());
        };

        match packet_type {
            Self::PLAYER_ID => PlayerIdAssignment::parse(packet).map(Self::PlayerId),
            Self::JOIN_GAME => Ok(Self::JoinGame),
            Self::PLAYER_ENTERING => PlayerRosterEntry::parse(packet).map(Self::PlayerEntering),
            Self::PLAYER_LEAVING => PlayerLeaving::parse(packet).map(Self::PlayerLeaving),
            Self::LARGE_POSITION => {
                PlayerPositionUpdate::parse_large(packet).map(Self::LargePosition)
            }
            Self::PLAYER_DEATH => PlayerDeath::parse(packet).map(Self::PlayerDeath),
            Self::CHAT => ChatPacket::parse(packet).map(Self::Chat),
            Self::PLAYER_PRIZE => PlayerPrize::parse(packet).map(Self::PlayerPrize),
            Self::PRIZE_PICKUP_ACK => PrizePickupAck::parse(packet).map(Self::PrizePickupAck),
            Self::PRIZE_EFFECT_ACCEPTED => {
                PrizeEffectAccepted::parse(packet).map(Self::PrizeEffectAccepted)
            }
            Self::PORTAL_PLACEMENT_ACCEPTED => {
                PortalPlacementAccepted::parse(packet).map(Self::PortalPlacementAccepted)
            }
            Self::PORTAL_WARP_ACCEPTED => {
                PortalWarpAccepted::parse(packet).map(Self::PortalWarpAccepted)
            }
            Self::HIDDEN_PRIZE_SPAWN => HiddenPrizeSpawn::parse(packet).map(Self::HiddenPrizeSpawn),
            Self::SCORE_UPDATE => ScoreUpdate::parse(packet).map(Self::ScoreUpdate),
            Self::RESET_SCORE => ResetScore::parse(packet).map(Self::ResetScore),
            Self::PASSWORD_RESPONSE => {
                PasswordResponse::parse_packet(packet).map(Self::PasswordResponse)
            }
            Self::SOCCER_GOAL => SoccerGoal::parse(packet).map(Self::SoccerGoal),
            Self::PLAYER_VOICE => PlayerVoice::parse(packet).map(Self::PlayerVoice),
            Self::FREQUENCY_CHANGE => FrequencyChange::parse(packet).map(Self::FrequencyChange),
            Self::ARENA_SETTINGS => ContinuumArenaSettings::parse(packet).map(Self::ArenaSettings),
            Self::FILE_TRANSFER => ServerFileTransfer::parse(packet).map(Self::FileTransfer),
            Self::FILE_TRANSFER_CHUNK => {
                ServerFileTransferChunk::parse(packet).map(Self::FileTransferChunk)
            }
            Self::NO_OP_11 | Self::NO_OP_17 | Self::NO_OP_26 => Ok(Self::NoOp(packet_type)),
            Self::SECURITY => SecurityChallenge::parse_packet(packet).map(Self::SecurityChallenge),
            Self::REQUEST_FILE => ClientFileRequest::parse(packet).map(Self::ClientFileRequest),
            Self::SHIP_FREQUENCY_CHANGE => {
                ShipFrequencyChange::parse(packet).map(Self::ShipFrequencyChange)
            }
            Self::FREQUENCY_CUTOVER_ACK => {
                FrequencyCutoverAck::parse(packet).map(Self::FrequencyCutoverAck)
            }
            Self::SHIP_CHANGE_REJECTED => {
                ShipChangeRejected::parse(packet).map(Self::ShipChangeRejected)
            }
            Self::LIFECYCLE_BASELINE_ACCEPTED => {
                LifecycleBaselineAccepted::parse(packet).map(Self::LifecycleBaselineAccepted)
            }
            Self::WARP_RELOCATION_ACCEPTED => {
                WarpRelocationAccepted::parse(packet).map(Self::WarpRelocationAccepted)
            }
            Self::REPEL_APPLICATION_ACCEPTED => {
                RepelApplicationAccepted::parse(packet).map(Self::RepelApplicationAccepted)
            }
            Self::MOVEMENT_CORRECTION => {
                MovementCorrection::parse(packet).map(Self::MovementCorrection)
            }
            Self::MOVEMENT_CORRECTION_ACCEPTED => {
                MovementCorrectionAccepted::parse(packet).map(Self::MovementCorrectionAccepted)
            }
            Self::CREATE_TURRET_LINK => CreateTurretLink::parse(packet).map(Self::CreateTurretLink),
            Self::DESTROY_TURRET_LINK => {
                DestroyTurretLink::parse(packet).map(Self::DestroyTurretLink)
            }
            Self::SPECTATE => SpectatePacket::parse(packet).map(Self::Spectate),
            Self::LOGIN_FAILURE => LoginFailurePacket::parse(packet).map(Self::LoginFailure),
            Self::VERSION => ServerVersionPacket::parse(packet).map(Self::Version),
            Self::PERSONAL_BANNER_CHANGE => {
                PersonalBannerChange::parse(packet).map(Self::PersonalBannerChange)
            }
            Self::PLAYER_BANNER_CHANGE => {
                PlayerBannerChange::parse(packet).map(Self::PlayerBannerChange)
            }
            Self::TOGGLE_SEND_DAMAGE => {
                ToggleSendDamagePacket::parse(packet).map(Self::ToggleSendDamage)
            }
            Self::TOGGLE_UFO => ToggleUfoPacket::parse(packet).map(Self::ToggleUfo),
            Self::SET_PERSONAL_KOTH_TIMER => {
                SetPersonalKothTimer::parse(packet).map(Self::SetPersonalKothTimer)
            }
            Self::KOTH_GAME_RESET => KothGameReset::parse(packet).map(Self::KothGameReset),
            Self::ADD_KOTH_TIME => AddKothTime::parse(packet).map(Self::AddKothTime),
            Self::WATCH_DAMAGE => ServerDamageReport::parse(packet).map(Self::WatchDamage),
            Self::COLLECTED_PRIZE => CollectedPrize::parse(packet).map(Self::CollectedPrize),
            Self::BRICK_DROPPED => BrickDropped::parse(packet).map(Self::BrickDropped),
            Self::SMALL_POSITION => {
                PlayerPositionUpdate::parse_small(packet).map(Self::SmallPosition)
            }
            Self::MAP_INFORMATION => {
                MapInformation::parse_payload(&packet[1..], continuum_packet_format)
                    .map(Self::MapInformation)
            }
            Self::COMPRESSED_MAP => CompressedMapPacket::parse(packet).map(Self::CompressedMap),
            Self::FLAG_POSITION => FlagPosition::parse(packet).map(Self::FlagPosition),
            Self::FLAG_CLAIM => FlagClaim::parse(packet).map(Self::FlagClaim),
            Self::FLAG_VICTORY => FlagVictory::parse(packet).map(Self::FlagVictory),
            Self::DROP_FLAG => DropFlag::parse(packet).map(Self::DropFlag),
            Self::SHIP_RESET => Ok(Self::ShipReset(ShipReset)),
            Self::KEEP_ALIVE => KeepAlive::parse(packet).map(Self::KeepAlive),
            Self::POWERBALL_POSITION => {
                PowerballPositionUpdate::parse(packet).map(Self::PowerballPosition)
            }
            Self::OBJECTIVE_STATE => ObjectiveStateUpdate::parse(packet).map(Self::ObjectiveState),
            Self::OBJECTIVE_SCORE => ObjectiveScoreUpdate::parse(packet).map(Self::ObjectiveScore),
            Self::ARENA_DIRECTORY => ArenaDirectoryListing::parse(packet).map(Self::ArenaDirectory),
            Self::ZONE_BANNER_ADVERTISEMENT => {
                ZoneBannerAdvertisement::parse(packet).map(Self::ZoneBannerAdvertisement)
            }
            Self::POST_LOGIN => PostLogin::parse(packet).map(Self::PostLogin),
            Self::SET_COORDINATES => SetCoordinates::parse(packet).map(Self::SetCoordinates),
            Self::TURF_FLAG_UPDATE => TurfFlagUpdate::parse(packet).map(Self::TurfFlagUpdate),
            Self::FLAG_REWARD => FlagReward::parse(packet).map(Self::FlagReward),
            Self::SPEED_GAME_OVER => SpeedGameOver::parse(packet).map(Self::SpeedGameOver),
            Self::TOGGLE_LVZ => LvzTogglePacket::parse(packet).map(Self::ToggleLvz),
            Self::MODIFY_LVZ => LvzModifyPacket::parse(packet).map(Self::ModifyLvz),
            Self::BATCHED_SMALL_POSITION => {
                PlayerPositionUpdate::parse_batched_small(packet).map(Self::BatchedSmallPositions)
            }
            Self::BATCHED_LARGE_POSITION => {
                PlayerPositionUpdate::parse_batched_large(packet).map(Self::BatchedLargePositions)
            }
            Self::REDIRECT => ServerRedirectPacket::parse(packet).map(Self::Redirect),
            Self::SELECT_BOX => SelectBoxPacket::parse(packet).map(Self::SelectBox),
            Self::SNAPSHOT => SnapshotFrame::parse(packet).map(Self::Snapshot),
            Self::REALTIME_EVENT => {
                crate::realtime_event::AuthenticatedRealtimeEvent::parse(packet)
                    .map(Self::RealtimeEvent)
            }
            Self::SERVER_CLOCK_BOOTSTRAP => {
                ServerClockBootstrap::parse(packet).map(Self::ServerClockBootstrap)
            }
            Self::QA_TRACE_CONTEXT => QaTraceContext::parse(packet).map(Self::QaTraceContext),
            ConnectionQualityProbe::TYPE => {
                ConnectionQualityProbe::parse(packet).map(Self::ConnectionQualityProbe)
            }
            ConnectionQualityDirective::TYPE => {
                ConnectionQualityDirective::parse(packet).map(Self::ConnectionQualityDirective)
            }
            ConnectionQualityWarning::TYPE => {
                ConnectionQualityWarning::parse(packet).map(Self::ConnectionQualityWarning)
            }
            SpawnIdleSpectator::TYPE => {
                SpawnIdleSpectator::parse(packet).map(Self::SpawnIdleSpectator)
            }
            packet_type => Ok(Self::Unknown {
                packet_type,
                payload: &packet[1..],
            }),
        }
    }
}
