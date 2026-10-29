use super::*;

mod actions;
mod audio;
mod events;
use events::AppliedCollectibleAckHistory;
mod flags;
mod frame_rate;
#[cfg(test)]
mod lifecycle_reference_model;
mod lifecycle_sequencer;
use lifecycle_sequencer::*;
mod network;
mod player;
#[cfg(any(test, target_arch = "wasm32"))]
mod quality_return;
mod render;
mod simulation;

#[cfg(any(test, target_arch = "wasm32"))]
mod browser_state;
mod init;
mod state;

pub(in crate::game) use actions::{
    local_zone_tuning, merge_local_zone_lvz, merge_local_zone_lvz_sounds,
    zone_settings_path_for_map,
};
pub(in crate::game) use audio::AudioVolumes;
#[cfg(any(test, target_arch = "wasm32"))]
pub(in crate::game) use browser_state::*;

/// A lifecycle action accepted from local input but not yet assigned an
/// application-reliable id. Keeping this separate from the sent/ACK-pending
/// states means a saturated reliable table cannot consume the cutover
/// boundary, input history, or timeout budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // variants are constructed by the wasm browser edge
enum PendingLifecycleRequest {
    Ship(u8),
    Frequency(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NetworkDeathIdentity {
    authority_segment: u64,
    killed_id: u16,
    life_seq: u32,
    green_collectible_id: u32,
    respawn_at_tick: u32,
}

const MAX_REMOTE_SHIP_EXPLOSION_WITNESSES: usize = 64;

/// Bounded diagnostic history of effects actually created from admitted deaths.
/// The wire death carries the successor life, not the killed predecessor life.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RemoteShipExplosionWitness {
    death: NetworkDeathIdentity,
    ordinal: u32,
    position: Vec2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct OwnerWeaponCancellationFact {
    event_id: u32,
    arena_scope: RemoteWeaponArenaScope,
    cancellation: crate::realtime_event::RealtimeOwnerWeaponCancellation,
}

/// A short owner-only visual lead while an exact predicted bomb converges to
/// its confirmed server timeline. Never stored in the live projectile state.
#[derive(Clone, Copy, Debug)]
struct OwnerBombDrawBridge {
    key: crate::remote_weapon::RemoteWeaponViewKey,
    projectile_id: u32,
    start_tick: u32,
    start_position: Vec2,
    velocity: Vec2,
    bounces_remaining: u8,
    initial_lead_seconds: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RemoteProjectileTerminal {
    event_id: u32,
    arena_scope: RemoteWeaponArenaScope,
    source_identity: RemoteWeaponSourceIdentity,
    shot_group_id: u64,
    weapon_type: u8,
    child_index: u8,
}

#[derive(Clone, Copy, Debug)]
struct PendingRemoteWeaponBirthEffect {
    key: crate::remote_weapon::RemoteWeaponViewKey,
    source: RemoteWeaponSource,
    weapon_data: u16,
}

#[derive(Clone, Copy, Debug)]
enum RemoteWeaponTrailKind {
    Bullet { level: u8, bouncing: bool },
    Bomb,
}

#[derive(Clone, Copy, Debug)]
struct RemoteWeaponTrail {
    key: crate::remote_weapon::RemoteWeaponViewKey,
    source: RemoteWeaponSource,
    born_tick: u32,
    position: Vec2,
    kind: RemoteWeaponTrailKind,
    presented: bool,
}

const MAX_REMOTE_WEAPON_BIRTH_EFFECTS: usize = MAX_PENDING_REMOTE_WEAPON_PRESENTATIONS;
const MAX_REMOTE_WEAPON_TRAILS: usize = MAX_PENDING_REMOTE_WEAPON_PRESENTATIONS * 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NetworkDamageIdentity {
    authority_segment: u64,
    victim_player_id: u16,
    victim_life_seq: u32,
    damage_seq: u32,
}

#[cfg(any(test, target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClientObjectiveActionAttempt {
    request: ObjectiveActionRequest,
    sent_at_tick: Option<u32>,
}

/// Exact logical input whose one-record Rocket pulse represents one successful
/// local activation. Callers bind this at the movement/action phase boundary;
/// the activation code never infers it from mutable `Game` sequence state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RocketActivation {
    pulse_sequence: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AcceptedPrizeEngineShutdown {
    life_seq: u32,
    prize_state_revision: u32,
    until_tick: Option<u32>,
}

/// Combat-view receipt captured at the same pre-action boundary as local
/// weapon prediction. The later network owner must transmit this exact value,
/// even if applying deferred snapshot actions advances the observer timeline
/// before the position packet is assembled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreparedCombatViewReceipt {
    authority_segment: u64,
    receipt: crate::combat_timeline::CombatViewReceiptV1,
}

/// Authenticated realtime weapon fact that overtook the exact snapshot actor
/// generation needed to present its source. The event lane is independent of
/// the snapshot lane, so retaining this immutable fact is the merge barrier;
/// neither a reusable player id nor a later life may materialize it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemoteWeaponPresentationKey {
    arena_scope: RemoteWeaponArenaScope,
    event_id: u32,
    source_player_id: u16,
    source_session_id: u64,
    source_life_seq: u32,
}

const REMOTE_WEAPON_PRESENTATION_REASON_COUNT: usize = 13;
const MAX_PENDING_REMOTE_WEAPON_PRESENTATIONS: usize =
    crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
enum RemoteWeaponPresentationSuppressionReason {
    ActorMissing = 0,
    LifeUnresolved = 1,
    ShipMismatch = 2,
    SnapshotMissing = 3,
    PositionMissing = 4,
    DisplayPositionMissing = 5,
    SessionSuperseded = 6,
    LifeSuperseded = 7,
    CapacityRejected = 8,
    EventWindowExpired = 9,
    ScopeCancelled = 10,
    InvalidIdentity = 11,
    ServerClockUnavailable = 12,
}

impl RemoteWeaponPresentationSuppressionReason {
    const ALL: [Self; REMOTE_WEAPON_PRESENTATION_REASON_COUNT] = [
        Self::ActorMissing,
        Self::LifeUnresolved,
        Self::ShipMismatch,
        Self::SnapshotMissing,
        Self::PositionMissing,
        Self::DisplayPositionMissing,
        Self::SessionSuperseded,
        Self::LifeSuperseded,
        Self::CapacityRejected,
        Self::EventWindowExpired,
        Self::ScopeCancelled,
        Self::InvalidIdentity,
        Self::ServerClockUnavailable,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::ActorMissing => "actorMissing",
            Self::LifeUnresolved => "lifeUnresolved",
            Self::ShipMismatch => "shipMismatch",
            Self::SnapshotMissing => "snapshotMissing",
            Self::PositionMissing => "positionMissing",
            Self::DisplayPositionMissing => "displayPositionMissing",
            Self::SessionSuperseded => "sessionSuperseded",
            Self::LifeSuperseded => "lifeSuperseded",
            Self::CapacityRejected => "capacityRejected",
            Self::EventWindowExpired => "eventWindowExpired",
            Self::ScopeCancelled => "scopeCancelled",
            Self::InvalidIdentity => "invalidIdentity",
            Self::ServerClockUnavailable => "serverClockUnavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingRemoteWeaponPresentation {
    key: RemoteWeaponPresentationKey,
    server_tick: u32,
    source: crate::realtime_event::RealtimeWeaponEvent,
    suppression_reason: RemoteWeaponPresentationSuppressionReason,
}

#[derive(Debug)]
struct RemoteWeaponPresentationQueue {
    pending: VecDeque<PendingRemoteWeaponPresentation>,
    pending_by_reason: [usize; REMOTE_WEAPON_PRESENTATION_REASON_COUNT],
    terminal_by_reason: [u32; REMOTE_WEAPON_PRESENTATION_REASON_COUNT],
    queued_total: u32,
    released_total: u32,
}

impl Default for RemoteWeaponPresentationQueue {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            pending_by_reason: [0; REMOTE_WEAPON_PRESENTATION_REASON_COUNT],
            terminal_by_reason: [0; REMOTE_WEAPON_PRESENTATION_REASON_COUNT],
            queued_total: 0,
            released_total: 0,
        }
    }
}

impl RemoteWeaponPresentationQueue {
    fn len(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    fn pending_for(&self, reason: RemoteWeaponPresentationSuppressionReason) -> usize {
        self.pending_by_reason[reason as usize]
    }

    fn terminal_for(&self, reason: RemoteWeaponPresentationSuppressionReason) -> u32 {
        self.terminal_by_reason[reason as usize]
    }

    fn dropped_total(&self) -> u32 {
        [
            RemoteWeaponPresentationSuppressionReason::SessionSuperseded,
            RemoteWeaponPresentationSuppressionReason::LifeSuperseded,
            RemoteWeaponPresentationSuppressionReason::CapacityRejected,
            RemoteWeaponPresentationSuppressionReason::EventWindowExpired,
            RemoteWeaponPresentationSuppressionReason::InvalidIdentity,
        ]
        .into_iter()
        .fold(0_u32, |total, reason| {
            total.saturating_add(self.terminal_for(reason))
        })
    }

    fn superseded_total(&self) -> u32 {
        self.terminal_for(RemoteWeaponPresentationSuppressionReason::SessionSuperseded)
            .saturating_add(
                self.terminal_for(RemoteWeaponPresentationSuppressionReason::LifeSuperseded),
            )
    }

    fn mark_terminal(&mut self, reason: RemoteWeaponPresentationSuppressionReason) {
        self.terminal_by_reason[reason as usize] =
            self.terminal_by_reason[reason as usize].saturating_add(1);
    }

    fn enqueue(&mut self, pending: PendingRemoteWeaponPresentation) {
        if let Some(existing) = self
            .pending
            .iter()
            .find(|existing| existing.key == pending.key)
        {
            if existing.server_tick != pending.server_tick || existing.source != pending.source {
                self.mark_terminal(RemoteWeaponPresentationSuppressionReason::InvalidIdentity);
            }
            return;
        }
        if self.pending.iter().any(|existing| {
            existing.key.arena_scope == pending.key.arena_scope
                && existing.key.event_id == pending.key.event_id
        }) {
            self.mark_terminal(RemoteWeaponPresentationSuppressionReason::InvalidIdentity);
            return;
        }
        if self.pending.len() >= MAX_PENDING_REMOTE_WEAPON_PRESENTATIONS {
            // Preserve the older admitted dependency. Capacity pressure may
            // suppress the new presentation, but cannot evict and reorder an
            // existing immutable event merely because a newer event arrived.
            self.mark_terminal(RemoteWeaponPresentationSuppressionReason::CapacityRejected);
            return;
        }
        self.queued_total = self.queued_total.saturating_add(1);
        self.pending_by_reason[pending.suppression_reason as usize] =
            self.pending_by_reason[pending.suppression_reason as usize].saturating_add(1);
        self.pending.push_back(pending);
    }

    fn pop_front(&mut self) -> Option<PendingRemoteWeaponPresentation> {
        let pending = self.pending.pop_front()?;
        self.pending_by_reason[pending.suppression_reason as usize] =
            self.pending_by_reason[pending.suppression_reason as usize].saturating_sub(1);
        Some(pending)
    }

    fn requeue(
        &mut self,
        mut pending: PendingRemoteWeaponPresentation,
        reason: RemoteWeaponPresentationSuppressionReason,
    ) {
        pending.suppression_reason = reason;
        self.pending_by_reason[reason as usize] =
            self.pending_by_reason[reason as usize].saturating_add(1);
        self.pending.push_back(pending);
    }

    fn mark_released(&mut self) {
        self.released_total = self.released_total.saturating_add(1);
    }

    fn expire_outside_window(&mut self, latest_event_id: u32) {
        let pending_count = self.pending.len();
        for _ in 0..pending_count {
            let Some(pending) = self.pop_front() else {
                break;
            };
            if latest_event_id.wrapping_sub(pending.key.event_id)
                < crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS as u32
            {
                let reason = pending.suppression_reason;
                self.requeue(pending, reason);
            } else {
                self.mark_terminal(RemoteWeaponPresentationSuppressionReason::EventWindowExpired);
            }
        }
    }

    fn cancel_all(&mut self) {
        while self.pop_front().is_some() {
            self.mark_terminal(RemoteWeaponPresentationSuppressionReason::ScopeCancelled);
        }
    }
}

/// Keys already held when an authoritative respawn becomes playable. The
/// first post-respawn sample captures them; only those stale holds remain
/// suppressed. A subsequently pressed movement/weapon key is intentional
/// gameplay and releases the latch even if an action key is still held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RespawnInputLatch {
    armed: bool,
    captured: bool,
    movement_bits: u16,
    action_bits: u16,
}

/// Disposable draw data, never a movement witness or gameplay authority.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LocalRenderPrediction {
    position: Vec2,
    velocity: Vec2,
    orientation: f32,
    input_bits: u16,
    partial_seconds: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClientPortalPlacement {
    request: PortalPlacementRequest,
}

#[cfg(target_arch = "wasm32")]
const MAX_PENDING_PORTAL_PLACEMENTS: usize = 64;

impl RocketActivation {
    fn on_next_input(next_input_sequence: u16) -> Self {
        Self {
            pulse_sequence: next_input_sequence,
        }
    }
}

impl Game {
    pub(crate) fn finish_gameplay_batch(&mut self) {
        self.suppress_lifecycle_tail_position = false;
    }
}
#[cfg(target_arch = "wasm32")]
pub(in crate::game) struct VoicePlayback {
    sound: Sound,
    ttl: f32,
}

pub(crate) struct Game {
    #[cfg(not(test))]
    assets: Assets,
    #[cfg(test)]
    assets: TestGameAssets,
    tuning: ArenaTuning,
    chat: ChatState,
    notifications: Vec<Notification>,
    ship: Ship,
    last_network_damage_player_id: Option<u16>,
    last_network_damage_attacker_id: Option<u16>,
    last_network_damage_energy: Option<f32>,
    last_network_damage_energy_before: Option<f32>,
    last_network_damage_amount: Option<u16>,
    last_network_damage_weapon: Option<u16>,
    last_network_damage_server_tick: Option<u32>,
    last_network_damage_server_seq: Option<u32>,
    last_network_damage_victim_life_seq: Option<u32>,
    last_network_damage_shooter_life_seq: Option<u32>,
    last_network_damage_for_self: bool,
    network_damage_event_seq: u32,
    last_self_damage_attacker_id: Option<u16>,
    last_self_damage_energy: Option<f32>,
    last_self_damage_energy_before: Option<f32>,
    last_self_damage_amount: Option<u16>,
    last_self_damage_weapon: Option<u16>,
    last_self_damage_server_tick: Option<u32>,
    last_self_damage_server_seq: Option<u32>,
    last_self_damage_victim_life_seq: Option<u32>,
    last_self_damage_shooter_life_seq: Option<u32>,
    self_damage_event_seq: u32,
    last_self_server_energy: Option<f32>,
    last_self_server_rocket_life_seq: Option<u32>,
    /// A local life reset has happened after the last Rocket snapshot. Equal
    /// or older delayed state is ignored until the server proves a strictly
    /// forward life, including across u32 wrap.
    require_forward_self_server_rocket_life: bool,
    last_self_server_rocket_activation_sequence: Option<u16>,
    last_self_server_rocket_until_tick: Option<u32>,
    pending_rocket_activation_sequence: Option<u16>,
    last_self_server_prize_life_seq: Option<u32>,
    last_self_server_prize_revision: Option<u32>,
    /// Explicit entry policy from the admitted private server snapshot.
    /// Retained across death, cleared on hard authority replacement.
    server_maxed_spawn_loadout: Option<bool>,
    /// Newest exact private Portal count retained independently from local
    /// prediction. While placement terminals are pending, snapshots update
    /// this source without resurrecting an item already predicted as spent.
    last_self_server_portals: Option<(u32, u32, u8)>,
    /// A hard session/arena/player authority replacement invalidated all
    /// locally held private loadout prediction. Gameplay stays closed until
    /// one exact current-scope PrizeState baseline installs.
    network_prize_baseline_pending: bool,
    /// Latest accepted private PrizeState Shutdown union. Keep the Option
    /// distinct from a present wrapped deadline of tick zero.
    last_self_server_engine_shutdown_until_tick: Option<u32>,
    /// Latest accepted EMP-only shutdown deadline. Recharge freezes on this
    /// alone; the union above is a movement gate that also covers the
    /// Shutdown prize, which still recharges.
    last_self_server_emp_until_tick: Option<u32>,
    /// The latest private PrizeState still describes a Shutdown whose exact
    /// client application boundary has not been accepted by the server. Its
    /// deadline is continuity evidence, not permission to apply new movement
    /// semantics before the matching pickup ACK/application transaction.
    last_self_server_engine_shutdown_application_pending: bool,
    last_self_server_damage_prize_loss_seq: Option<u32>,
    last_self_server_damage_prize_loss_id: Option<i16>,
    last_self_server_damage_prize_loss_life_seq: Option<u32>,
    last_self_server_damage_prize_loss_revision: Option<u32>,
    damage_prize_notification_count: u32,
    last_damage_prize_notification: Option<String>,
    prize_pickup_notification_count: u32,
    last_prize_pickup_notification: Option<String>,
    prize_pickup_sound_play_count: u32,
    combat_kill_notification_count: u32,
    last_combat_kill_notification: Option<String>,
    remote_ship_explosion_count: u32,
    last_remote_ship_explosion_position: Option<Vec2>,
    remote_ship_explosion_witnesses: VecDeque<RemoteShipExplosionWitness>,
    bullets: Vec<Bullet>,
    bullet_trails: Vec<BulletTrail>,
    bullet_impacts: Vec<BulletImpact>,
    bombs: Vec<Bomb>,
    bomb_flashes: Vec<BombFlash>,
    bomb_trails: Vec<BombTrail>,
    mines: Vec<Mine>,
    shrapnel: Vec<Shrapnel>,
    thors: Vec<Thor>,
    remote_projectiles: Vec<RemoteProjectile>,
    owner_bomb_draw_bridges: VecDeque<OwnerBombDrawBridge>,
    remote_weapon_view_history: crate::remote_weapon::RemoteWeaponViewHistory,
    prepared_remote_projectiles: Vec<RemoteProjectile>,
    prepared_remote_weapon_groups: Vec<(
        crate::remote_weapon::RemoteWeaponViewKey,
        Option<(u32, f32)>,
    )>,
    pending_remote_birth_effects: VecDeque<PendingRemoteWeaponBirthEffect>,
    remote_weapon_trails: VecDeque<RemoteWeaponTrail>,
    prepared_remote_bullet_trails: Vec<BulletTrail>,
    prepared_remote_bomb_trails: Vec<BombTrail>,
    remote_weapon_view_effect_drops: u64,
    remote_projectile_spawns: u32,
    remote_bomb_spawns: u32,
    remote_bullet_spawns: u32,
    remote_mine_spawns: u32,
    remote_repel_spawns: u32,
    /// Public Repel facts retained across the complete authenticated realtime
    /// reorder horizon so a delayed older weapon is materialized in its
    /// server-causal post-Repel state.
    remote_repel_presentation_history: VecDeque<RemoteRepelPresentationFact>,
    remote_repel_authoritative_history: VecDeque<RemoteRepelAuthoritativeMutation>,
    latest_remote_realtime_event_id: Option<u32>,
    remote_weapon_presentations: RemoteWeaponPresentationQueue,
    bricks: Vec<Brick>,
    decoys: Vec<Decoy>,
    decoy_ordinary_ship_presentations: std::cell::Cell<usize>,
    decoy_name_label_presentations: std::cell::Cell<usize>,
    decoy_radar_presentations: std::cell::Cell<usize>,
    repels: Vec<RepelEffect>,
    warps: Vec<WarpEffect>,
    portal: Option<Portal>,
    /// Monotonic endpoint generation. A successful Warp leaves a consumption
    /// tombstone here so an older placement terminal cannot resurrect it.
    portal_endpoint_authority: Option<PortalEndpointAuthority>,
    explosions: Vec<Explosion>,
    ship_explosions: Vec<ShipExplosion>,
    recent_network_deaths: VecDeque<NetworkDeathIdentity>,
    /// Logical damage facts are deduplicated only after the immutable
    /// authority plan admits them. Snapshot and realtime carriers may both be
    /// decoded because a rejected carrier must not consume the valid copy.
    remote_projectile_terminals: VecDeque<RemoteProjectileTerminal>,
    remote_owner_weapon_cancellations: VecDeque<OwnerWeaponCancellationFact>,
    recent_network_damage: VecDeque<NetworkDamageIdentity>,
    latest_network_damage_sequence: Option<(u64, u32)>,
    burst_effects: Vec<BurstEffect>,
    exhausts: Vec<ExhaustParticle>,
    rocket_particles: Vec<RocketParticle>,
    prizes: Vec<PrizePickup>,
    /// Collectible acknowledgements already applied in this server-session/
    /// arena scope. This is deliberately not life-scoped: prize effects and
    /// their acknowledgement identity survive death/respawn.
    applied_collectible_acks: AppliedCollectibleAckHistory,
    bots: Vec<BotShip>,
    bot_bullets: Vec<BotBullet>,
    powerball: Powerball,
    powerball_trails: Vec<PowerballTrail>,
    carrying_flag: bool,
    dropped_flag: Option<DroppedFlag>,
    flag_score: u32,
    bot_flag_score: u32,
    ball_score: u32,
    bot_ball_score: u32,
    bot_score: u32,
    player_deaths: u32,
    player_bounty: u32,
    local_player_name: String,
    pending_weapon: u16,
    pending_weapon_prediction: Option<u64>,
    next_weapon_prediction_id: u64,
    owned_projectile_reconciliations: u64,
    predicted_explosion_receipts: VecDeque<crate::model::OwnedProjectilePrediction>,
    owned_projectile_life_floor: Option<(u64, u64, u16, u32)>,
    /// Exact positive-Warp collectible whose relocation marker is repeated on
    /// positions until authoritative application acceptance is received.
    pending_warp_relocation_collectible_id: Option<u32>,
    /// Newest hostile-Repel snapshot and the exact logical movement tail
    /// after which its velocity replacement was applied.
    pending_repel_application: Option<crate::continuum_position::ClientRepelApplicationMarker>,
    /// Original server deadline for the local pending marker. If no first
    /// marker reaches the server before it, the browser cancels the local
    /// provisional window so a later FIFO event cannot be stranded.
    pending_repel_application_expiry_tick: Option<u32>,
    /// Locally expired markers retained only so a reliable receipt for an
    /// application the server admitted before the deadline can still be
    /// matched. These entries carry no movement window and cannot re-arm.
    expired_repel_applications:
        std::collections::VecDeque<crate::continuum_position::ClientRepelApplicationMarker>,
    /// Recently application-receipted server generations. Delayed unordered
    /// snapshots from one of these generations are presentation duplicates,
    /// never a second impulse.
    consumed_repel_delivery_generations: std::collections::VecDeque<u32>,
    /// Browser-visible application diagnostics. A drained server queue cannot
    /// hide a private delivery that the client rejected before arming 0x79.
    repel_delivery_frames_seen: u32,
    repel_delivery_frames_applied: u32,
    repel_application_positions_sent: u32,
    repel_delivery_life_rejects: u32,
    repel_delivery_timing_rejects: u32,
    repel_delivery_application_rejects: u32,
    repel_delivery_expired_application_cancellations: u32,
    repel_delivery_expired_receipts_recovered: u32,
    repel_delivery_pending_retries: u32,
    repel_delivery_consumed_retries: u32,
    repel_delivery_last_snapshot_tick: Option<u32>,
    repel_delivery_last_expiry_tick: Option<u32>,
    repel_delivery_last_observed_tick: Option<u32>,
    repel_delivery_last_offered_steps: Option<u16>,
    repel_delivery_last_accepted_end_sequence: Option<u16>,
    repel_delivery_last_accepted_steps: Option<u16>,
    /// Server-derived logical movement interval under RepelSpeed. The float
    /// `repel_timer` remains presentation only; this window owns simulation.
    repel_movement_sequence_window: Option<(u16, u16)>,
    pending_warp_relocation_life_seq: Option<u32>,
    pending_warp_relocation_sequences: std::collections::VecDeque<u16>,
    /// The ACKed Warp has not moved the ship yet. It relocates on the next
    /// emitted position, which is then exactly the spawn report the server
    /// requires; meaningful only while the collectible id above is set.
    pending_warp_relocation_staged: bool,
    /// Last `PlayerInput` constructed by `update_ship`. Phase 1b sends
    /// this to the server as an observation packet (parallel to the
    /// position relay). Phase 1c will use the same record for client
    /// prediction reconciliation.
    last_input: PlayerInput,
    /// Monotonic sequence number for outbound input/position packets. The
    /// server uses it to reject duplicated or replayed movement/weapon frames
    /// without reading client-provided timestamps.
    #[allow(dead_code)] // reader is wasm32-only
    next_input_seq: u16,
    #[allow(dead_code)] // reader is wasm32-only
    /// Once the exact reset boundary is emitted, suppress an ordinary tail
    /// position for the remainder of that catch-up batch. The unordered game
    /// channel must never be able to deliver a moved tail before the reset.
    suppress_lifecycle_tail_position: bool,
    /// Up to 32 per-tick inputs, repeated on the unordered realtime channel so
    /// dropped datagrams can be reconstructed from overlapping packets.
    #[allow(dead_code)] // reader is wasm32-only
    outbound_input_ring: ClientInputRing,
    /// The server's contiguous applied-through input sequence from the newest
    /// accepted snapshot; inputs through it are no longer resent.
    server_input_applied_through: Option<u16>,
    pending_portal_placements: VecDeque<ClientPortalPlacement>,
    #[cfg(target_arch = "wasm32")]
    next_portal_placement_action_id: u32,
    #[cfg(target_arch = "wasm32")]
    next_objective_action_id: u32,
    #[cfg(any(test, target_arch = "wasm32"))]
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    objective_action_attempts: BTreeMap<(u8, u16), ClientObjectiveActionAttempt>,
    accepted_prize_engine_shutdown: Option<AcceptedPrizeEngineShutdown>,
    /// A Warp/coordinate/Repel/door boundary has changed local same-life
    /// movement but its exact position witness has not yet been emitted.
    /// This transient publication flag is combined with the receipt-owned
    /// Warp/Repel markers when deciding whether an older correction must be
    /// cancelled: a successful unordered send is not server admission.
    pending_same_life_movement_boundary_position: bool,
    /// A lifecycle application receipt rebased local state during the
    /// pre-gameplay poll. Suppress exactly this render frame so the same poll
    /// cannot immediately consume and publish one additional input sequence.
    suppress_gameplay_step_after_network_rebase: bool,
    self_correction_count: u32,
    last_self_correction_reason: u8,
    respawn_input_latch: RespawnInputLatch,
    /// Developer-overlay counters for real-browser semantic edge evidence.
    semantic_input_counts: [u32; 12],
    spectator_mode: bool,
    spectating_player_id: Option<u16>,
    spectator_view_sequence: u16,
    arena_enter_time: f32,
    network_world: NetworkWorld,
    /// Largest single pre-step network poll, in decoded events, and how many of
    /// those events the authority staging had to defer. Distinguishes a backlog
    /// burst (hundreds of packets in one poll) from one oversized snapshot.
    poll_peak_events: usize,
    poll_peak_deferred_events: usize,
    /// Page-life cost of installing a downloaded arena's LVZ render merge. The
    /// stripped-image bench (`make bench-lvz-merge`) cannot see a texture upload,
    /// and an arena switch pays the merge inside a rendered frame, so these
    /// counters let a browser gate read the cost a real client actually paid.
    lvz_merge_count: u32,
    lvz_merge_total_ms: f32,
    lvz_merge_max_ms: f32,
    /// Stages of the networked flag claim: frames the local player was eligible
    /// to claim, frames a pickable flag was found nearby, and frames the send
    /// gate refused the claim. Distinguishes a predicate that never matches from
    /// a send path that silently drops it.
    flag_claim_eligible_frames: u32,
    flag_claim_attempts: u32,
    flag_claim_send_gated: u32,
    networked_play: bool,
    network_settings_received: bool,
    /// False from map advertisement until the complete authenticated
    /// map+LVZ publication batch has been installed.
    network_zone_files_ready: bool,
    /// True between a zone's `ZoneFilesPending` and `ZoneFilesPublished`. Downloaded LVZ archives
    /// merged in that window postpone their whole-cache resolve/band pass to one finalize at
    /// publication, because per-archive banding is O(archives x objects) and joins merge fourteen.
    zone_files_batch_open: bool,
    /// True between `ZoneFilesPublished` and the completion of the budgeted LVZ resolve that follows
    /// it. Zone-file readiness is withheld until the resolve completes, so the join does not open
    /// gameplay with a partly resolved zone; the per-frame driver is `advance_deferred_lvz_merge`.
    zone_files_resolve_pending: bool,
    /// False until the fully validated level itself is installed. Security
    /// can arrive earlier, but it must not open gameplay on the previous map;
    /// LVZ validation may safely continue behind this boundary.
    network_zone_map_ready: bool,
    /// Last authenticated door/prize challenge. Level downloads can arrive
    /// after the challenge and replace the active ArenaMap, so the new map
    /// must be rebound to this exact server anchor before gameplay resumes.
    network_security_challenge: Option<SecurityChallenge>,
    network_security_challenge_generation: Option<NetworkSecurityGeneration>,
    network_security_generation: Option<NetworkSecurityGeneration>,
    /// Sole mutable owner of client authority scope and lifecycle state.
    client_authority: ClientAuthoritySequencer,
    /// Browser-side consumer of the server-owned observer combat schedule.
    /// Its watermark advances only after a snapshot crosses Game application;
    /// codec ACK state remains owned by the protocol session.
    observer_combat_timeline: crate::combat_timeline::ClientObserverCombatTimeline,
    prepared_combat_view_receipt: Option<PreparedCombatViewReceipt>,
    /// Remote ships per Game-applied snapshot, for BombSafety against the
    /// snapshot a step's receipt names. Reset with the observer timeline.
    combat_view_ship_positions: crate::gameplay::CombatViewShipPositions,
    /// Exact most recently observed server tick retained while
    /// `update_browser_network` temporarily moves the browser owner out of
    /// `Game` to poll packets. Authority consumers must not see a false
    /// `None` clock during that ownership window.
    last_observed_browser_server_tick: Option<u32>,
    projectile_refresh_needed: bool,
    weapon_recovery_steps_remaining: u32,
    world_effects_advanced_before_presentation: bool,
    last_projectile_refresh_request_tick: Option<u32>,
    network_notice: Option<NetworkNotice>,
    #[cfg(any(test, target_arch = "wasm32"))]
    quality_return_recovery: Option<quality_return::QualityReturnRecovery>,
    menu_visible: bool,
    frame_rate: frame_rate::FrameRate,
    main_menu_requested: bool,
    scoreboard_visible: bool,
    scoreboard_scroll: player::player_list::PlayerListScroll,
    arena_directory_visible: bool,
    developer_overlay: bool,
    super_animation_age: f32,
    shield_animation_age: f32,
    super_visual_deadline: Option<u32>,
    shield_visual_deadline: Option<u32>,
    lvz_archives: BTreeMap<String, LvzArchive>,
    /// Exact manifest/download order, independent from keyed lookup order.
    /// Cross-archive last-wins files and chat sounds must follow this list.
    lvz_archive_order: Vec<String>,
    lvz_render_cache: LvzRenderCache,
    lvz_chat_sounds: BTreeMap<u8, Sound>,
    dynamic_lvz_chat_sound_sources: BTreeMap<u8, Arc<[u8]>>,
    dynamic_lvz_chat_sound_loaded_ids: BTreeSet<u8>,
    dynamic_lvz_chat_sound_generation: u64,
    #[cfg(target_arch = "wasm32")]
    dynamic_lvz_sound_loads: Vec<DynamicLvzSoundLoad>,
    prize_rng_seed: i32,
    #[cfg(target_arch = "wasm32")]
    browser_network: Option<BrowserNetworkClient>,
    #[cfg(target_arch = "wasm32")]
    voice_loads: Vec<Coroutine<Option<Sound>>>,
    #[cfg(target_arch = "wasm32")]
    active_voice_sounds: Vec<VoicePlayback>,
    player_respawn_timer: f32,
    player_respawn_seq: u32,
    flag_respawn_timer: f32,
    flag_carry_timer: f32,
    audio_volumes: AudioVolumes,
    rocket_audio: audio::RocketAudioLoop,
    camera: Camera2D,
    maps: Vec<ArenaMap>,
    map_tilesets: Vec<Option<Texture2D>>,
    map_index: usize,
    thrust_sound_started: bool,
    thrust_sound_reversing: bool,
    thrust_sound_afterburning: bool,
    afterburner_sound_play_count: u32,
    /// Last shared movement outcome, exposed only through diagnostic browser
    /// metrics so E2E can distinguish source-faithful capped Afterburner from
    /// a dropped Shift key.
    last_afterburning: bool,
    last_movement_thrust_acceleration: f32,
    last_movement_top_speed: f32,
    energy_alarm_timer: f32,
    victory_music_looping: bool,
    exhaust_timer: f32,
    prize_spawn_timer: f32,
    door_timer: f32,
    /// Realtime gameplay cannot carry an exact door phase until the map
    /// security challenge has seeded and synchronized the local door clock.
    network_security_synchronized: bool,
    /// Latest monotonic server tick consumed by the multiplayer door clock.
    /// Browser render time is not authoritative and can drift by whole door
    /// phases during scheduler stalls or long loss/recovery sessions.
    door_clock_server_tick: Option<u32>,
    /// Newest authoritative tick seen from the challenge/snapshot stream.
    door_clock_observed_server_tick: Option<u32>,
    /// Local monotonic time corresponding to `door_clock_server_tick`.
    door_clock_anchor_seconds: f64,
    /// Local monotonic time when the authoritative tick last advanced.
    door_clock_last_observed_seconds: f64,
    /// False when synchronized-tick extrapolation exceeded its safe bound;
    /// realtime gameplay stays gated until a newer server tick arrives.
    door_clock_fresh: bool,
}

/// Native authority tests exercise `Game::apply_network_actions` without a
/// macroquad render context.  Keep that accommodation test-only: production
/// builds retain the concrete `Assets` field, while ordinary tests created by
/// `Game::new` still transparently dereference the real bundle.
#[cfg(test)]
struct TestGameAssets(Option<Assets>);

#[cfg(test)]
impl std::ops::Deref for TestGameAssets {
    type Target = Assets;

    fn deref(&self) -> &Self::Target {
        self.0
            .as_ref()
            .expect("headless authority test touched render/audio assets")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NetworkSecurityGeneration {
    server_session_id: u64,
    arena_epoch: u32,
}

#[cfg(target_arch = "wasm32")]
struct DynamicLvzSoundLoad {
    generation: u64,
    sound_id: u8,
    load: Coroutine<Option<Sound>>,
}

impl NetworkSecurityGeneration {
    fn admits_challenge(
        self,
        captured_server_session_id: Option<u64>,
        captured_arena_epoch: u32,
        challenge: SecurityChallenge,
        previous: Option<SecurityChallenge>,
    ) -> bool {
        captured_server_session_id == Some(self.server_session_id)
            && captured_arena_epoch == self.arena_epoch
            && previous.is_none_or(|previous| {
                crate::continuum::continuum_tick_diff(challenge.timestamp, previous.timestamp) >= 0
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::game) struct NetworkNotice {
    pub(in crate::game) message: String,
    pub(in crate::game) color: TextColor,
}
