use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::time::Instant;

use super::config::ServerConfig;
use super::doors::{ClientDoorState, DoorSeedCache};
pub(crate) use super::limits::{MAX_SERVER_BOTS, SERVER_BOT_ID_BASE};
use super::reliable::{BootstrapReliableBarrier, ServerReliableReceiver, ServerReliableSender};
use super::zone::{ZoneMap, zone_map_for_id};
use crate::authority::{StatusInventory, WeaponInventory};
use crate::authority_identity::{
    LifeIdentity, PlayerRuntimeIdentity, PlayerSessionId, PlayerSessionIdentity,
};
use crate::authority_inputs::PositionInputQueues;
use crate::authority_projectiles::{ProjectileRecord, ProjectileRegistry};
#[cfg(test)]
use crate::visibility::{POSITION_FANOUT_RADIUS_PX, position_visible_to_recipient};
use crate::visibility::{VisibilityCell, VisibilityIndex, visibility_cell_for_large_position};

#[path = "state/bots.rs"]
mod bots;
#[path = "state/client_runtime.rs"]
mod client_runtime;
#[path = "state/projectiles.rs"]
mod projectiles;
#[path = "state/timing.rs"]
pub(crate) mod timing;

#[path = "state/movement_clock.rs"]
mod movement_clock;
pub(crate) use movement_clock::MovementTimeBudget;

pub(crate) use bots::ServerBot;
#[allow(unused_imports)]
pub(crate) use client_runtime::{status_inventory_tuning, weapon_inventory_tuning};

pub(crate) const PLAYER_BANNER_BYTES: usize = 96;
/// Whole-frame load shedding is a last resort. Require this many consecutive
/// ticks that actually exceed the configured wall-clock budget so one
/// scheduler/logging spike cannot turn into several additional missing clean-
/// LAN snapshots while the EWMA decays.
pub(crate) const LOAD_SHED_FRAME_OVERRUN_STREAK: u16 = 4;

pub(crate) trait ClientPacketSink: Send + Sync {
    fn try_send(&self, packet: Vec<u8>) -> Result<(), ()>;
    /// Queues one tick's realtime events together, so the writer can send
    /// them as one message.
    fn try_send_events(&self, packets: Vec<Vec<u8>>) -> Result<(), ()> {
        packets
            .into_iter()
            .try_for_each(|packet| self.try_send(packet))
    }
    fn reliable_overflow(&self) {}
    fn rtc_lane_backlog(&self, _now: Instant) -> RtcLaneBacklog {
        RtcLaneBacklog::default()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RtcLaneBacklog {
    pub(crate) packets: [usize; 3],
    pub(crate) bytes: [usize; 3],
    pub(crate) oldest_age_us: [u64; 3],
}

#[derive(Clone)]
pub(crate) enum ClientPacketSender {
    Unbounded(Sender<Vec<u8>>),
    Bounded(Arc<dyn ClientPacketSink>),
}

impl ClientPacketSender {
    pub(crate) fn bounded(sink: Arc<dyn ClientPacketSink>) -> Self {
        Self::Bounded(sink)
    }

    pub(crate) fn send(&self, packet: Vec<u8>) -> Result<(), ()> {
        match self {
            Self::Unbounded(sender) => sender.send(packet).map_err(|_| ()),
            Self::Bounded(sink) => sink.try_send(packet),
        }
    }

    /// Sends one tick's `0x7d` events for this client. An RTC sink queues them
    /// all at once, so its writer carries them in one batch; a plain channel
    /// receives each packet unchanged.
    pub(crate) fn send_events(&self, packets: Vec<Vec<u8>>) -> Result<(), ()> {
        match self {
            Self::Unbounded(sender) => packets
                .into_iter()
                .try_for_each(|packet| sender.send(packet).map_err(|_| ())),
            Self::Bounded(sink) => sink.try_send_events(packets),
        }
    }

    pub(crate) fn reliable_overflow(&self) {
        if let Self::Bounded(sink) = self {
            sink.reliable_overflow();
        }
    }
}

impl From<Sender<Vec<u8>>> for ClientPacketSender {
    fn from(sender: Sender<Vec<u8>>) -> Self {
        Self::Unbounded(sender)
    }
}

#[derive(Debug, Default)]
pub(crate) struct RtcOverloadMetrics {
    queue_packets: AtomicUsize,
    queue_bytes: AtomicUsize,
    peak_queue_packets: AtomicUsize,
    peak_queue_bytes: AtomicUsize,
    realtime_coalesced: AtomicUsize,
    reliable_overflows: AtomicUsize,
    protocol_violations: AtomicUsize,
    inbound_queue_packets: [AtomicUsize; 3],
    inbound_peak_queue_packets: [AtomicUsize; 3],
    inbound_packets: [AtomicUsize; 3],
    inbound_bytes: [AtomicUsize; 3],
    inbound_backpressure: [AtomicUsize; 3],
    outbound_packets: [AtomicUsize; 3],
    outbound_bytes: [AtomicUsize; 3],
    outbound_capacity_waits: [AtomicUsize; 3],
    outbound_capacity_wait_us: [AtomicUsize; 3],
    outbound_queue_age_max_us: [AtomicUsize; 3],
    outbound_outstanding_peak_bytes: [AtomicUsize; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RtcOverloadSnapshot {
    pub(crate) queue_packets: usize,
    pub(crate) queue_bytes: usize,
    pub(crate) peak_queue_packets: usize,
    pub(crate) peak_queue_bytes: usize,
    pub(crate) realtime_coalesced: usize,
    pub(crate) reliable_overflows: usize,
    pub(crate) protocol_violations: usize,
    /// Game, event, and reliable application-ingress queues, in that order.
    pub(crate) inbound_queue_packets: [usize; 3],
    pub(crate) inbound_peak_queue_packets: [usize; 3],
    pub(crate) inbound_packets: [usize; 3],
    pub(crate) inbound_bytes: [usize; 3],
    pub(crate) inbound_backpressure: [usize; 3],
    pub(crate) outbound_packets: [usize; 3],
    pub(crate) outbound_bytes: [usize; 3],
    pub(crate) outbound_capacity_waits: [usize; 3],
    pub(crate) outbound_capacity_wait_us: [usize; 3],
    pub(crate) outbound_queue_age_max_us: [usize; 3],
    pub(crate) outbound_outstanding_peak_bytes: [usize; 3],
}

impl RtcOverloadMetrics {
    pub(crate) fn add_queue(&self, packets: usize, bytes: usize) {
        let packets = self.queue_packets.fetch_add(packets, Ordering::Relaxed) + packets;
        let bytes = self.queue_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak_queue_packets
            .fetch_max(packets, Ordering::Relaxed);
        self.peak_queue_bytes.fetch_max(bytes, Ordering::Relaxed);
    }

    pub(crate) fn remove_queue(&self, packets: usize, bytes: usize) {
        let _ = self
            .queue_packets
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(packets))
            });
        let _ = self
            .queue_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(bytes))
            });
    }

    pub(crate) fn replace_queue_bytes(&self, previous: usize, replacement: usize) {
        if replacement >= previous {
            self.add_queue(0, replacement - previous);
        } else {
            self.remove_queue(0, previous - replacement);
        }
    }

    pub(crate) fn record_realtime_coalesced(&self) {
        self.realtime_coalesced.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_reliable_overflow(&self) {
        self.reliable_overflows.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_protocol_violation(&self) {
        self.protocol_violations.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn add_inbound_queue(&self, channel_index: usize) -> usize {
        let Some(current) = self.inbound_queue_packets.get(channel_index) else {
            return 0;
        };
        current.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub(crate) fn record_inbound_queue_peak(&self, channel_index: usize, queued: usize) {
        if let Some(peak) = self.inbound_peak_queue_packets.get(channel_index) {
            peak.fetch_max(queued, Ordering::Relaxed);
        }
    }

    pub(crate) fn remove_inbound_queue(&self, channel_index: usize) {
        if let Some(current) = self.inbound_queue_packets.get(channel_index) {
            let _ = current.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            });
        }
    }

    pub(crate) fn record_inbound_backpressure(&self, channel_index: usize) {
        if let Some(count) = self.inbound_backpressure.get(channel_index) {
            count.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_inbound_packet(&self, channel_index: usize, bytes: usize) {
        if let Some(count) = self.inbound_packets.get(channel_index) {
            count.fetch_add(1, Ordering::Relaxed);
            self.inbound_bytes[channel_index].fetch_add(bytes, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_lane_send(&self, index: usize, bytes: usize, age: std::time::Duration) {
        self.outbound_packets[index].fetch_add(1, Ordering::Relaxed);
        self.outbound_bytes[index].fetch_add(bytes, Ordering::Relaxed);
        self.outbound_queue_age_max_us[index].fetch_max(
            age.as_micros().min(usize::MAX as u128) as usize,
            Ordering::Relaxed,
        );
    }

    pub(crate) fn record_lane_capacity(
        &self,
        index: usize,
        waited: bool,
        duration: std::time::Duration,
        outstanding: usize,
    ) {
        self.outbound_outstanding_peak_bytes[index].fetch_max(outstanding, Ordering::Relaxed);
        if waited {
            self.outbound_capacity_waits[index].fetch_add(1, Ordering::Relaxed);
            self.outbound_capacity_wait_us[index].fetch_add(
                duration.as_micros().min(usize::MAX as u128) as usize,
                Ordering::Relaxed,
            );
        }
    }

    pub(crate) fn snapshot(&self) -> RtcOverloadSnapshot {
        RtcOverloadSnapshot {
            queue_packets: self.queue_packets.load(Ordering::Relaxed),
            queue_bytes: self.queue_bytes.load(Ordering::Relaxed),
            peak_queue_packets: self.peak_queue_packets.load(Ordering::Relaxed),
            peak_queue_bytes: self.peak_queue_bytes.load(Ordering::Relaxed),
            realtime_coalesced: self.realtime_coalesced.load(Ordering::Relaxed),
            reliable_overflows: self.reliable_overflows.load(Ordering::Relaxed),
            protocol_violations: self.protocol_violations.load(Ordering::Relaxed),
            inbound_queue_packets: std::array::from_fn(|index| {
                self.inbound_queue_packets[index].load(Ordering::Relaxed)
            }),
            inbound_peak_queue_packets: std::array::from_fn(|index| {
                self.inbound_peak_queue_packets[index].load(Ordering::Relaxed)
            }),
            inbound_packets: std::array::from_fn(|index| {
                self.inbound_packets[index].load(Ordering::Relaxed)
            }),
            inbound_bytes: std::array::from_fn(|index| {
                self.inbound_bytes[index].load(Ordering::Relaxed)
            }),
            inbound_backpressure: std::array::from_fn(|index| {
                self.inbound_backpressure[index].load(Ordering::Relaxed)
            }),
            outbound_packets: std::array::from_fn(|index| {
                self.outbound_packets[index].load(Ordering::Relaxed)
            }),
            outbound_bytes: std::array::from_fn(|index| {
                self.outbound_bytes[index].load(Ordering::Relaxed)
            }),
            outbound_capacity_waits: std::array::from_fn(|index| {
                self.outbound_capacity_waits[index].load(Ordering::Relaxed)
            }),
            outbound_capacity_wait_us: std::array::from_fn(|index| {
                self.outbound_capacity_wait_us[index].load(Ordering::Relaxed)
            }),
            outbound_queue_age_max_us: std::array::from_fn(|index| {
                self.outbound_queue_age_max_us[index].load(Ordering::Relaxed)
            }),
            outbound_outstanding_peak_bytes: std::array::from_fn(|index| {
                self.outbound_outstanding_peak_bytes[index].load(Ordering::Relaxed)
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlayerSummary {
    pub(crate) id: u16,
    pub(crate) name: String,
    pub(crate) ship: u8,
    pub(crate) frequency: u16,
    pub(crate) assists: u32,
    pub(crate) kill_points: u32,
    pub(crate) flag_points: u32,
    pub(crate) wins: u16,
    pub(crate) losses: u16,
    pub(crate) flags: u16,
    pub(crate) banner: [u8; PLAYER_BANNER_BYTES],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ServerRepelDelivery {
    pub(crate) snapshot_id: u16,
    pub(crate) snapshot_tick: u32,
    /// Exact number of live input records remaining at this offer. Retry
    /// frames never restore time already spent waiting in the queue.
    pub(crate) active_steps: u16,
    /// Server tick at which this exact snapshot was acknowledged. A marker
    /// arriving after the event deadline is admissible only when this proves
    /// the application frame reached the target before expiry.
    pub(crate) acknowledged_at_tick: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ServerRepelAcceptedApplication {
    pub(crate) snapshot_id: u16,
    pub(crate) application_sequence: u16,
    pub(crate) accepted_end_sequence: u16,
    pub(crate) active_steps: u16,
    /// Reliable wrapper ids carrying the exact 0x7a application receipt.
    /// Any acknowledged copy retires the queue head; the rest are harmless
    /// idempotent duplicates.
    pub(crate) receipt_reliable_ids: [Option<u32>; MAX_REPEL_RECEIPT_COPIES],
    pub(crate) receipt_reliable_cursor: u8,
}

pub(crate) const MAX_REPEL_DELIVERY_SNAPSHOTS: usize = 256;
pub(crate) const MAX_PENDING_REPEL_EVENTS: usize = 16;
pub(crate) const MAX_REPEL_RECEIPT_COPIES: usize = 8;
pub(crate) const MAX_RETIRED_REPEL_APPLICATIONS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ServerRepelRetiredApplication {
    pub(crate) delivery_generation: u32,
    pub(crate) application: ServerRepelAcceptedApplication,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ServerRepelObservation {
    pub(crate) source: PlayerRuntimeIdentity,
    pub(crate) event_tick: u32,
    pub(crate) expires_at_tick: u32,
    pub(crate) origin_x: f32,
    pub(crate) origin_y: f32,
    pub(crate) expected_velocity_x: f32,
    pub(crate) expected_velocity_y: f32,
    /// Exact accepted Repel weapon word synthesized into this target's
    /// private retry frame if the first unordered snapshot is lost.
    pub(crate) weapon_data: u16,
    pub(crate) source_frequency: u16,
    /// Immutable ship tuning identity used for the target-private sound and
    /// presentation. A later player-id reuse must not select another ship.
    pub(crate) source_ship: u8,
    /// Never-zero server-session-scoped identity. Snapshot ids wrap, so this
    /// generation is carried in both the private frame and application marker.
    pub(crate) delivery_generation: u32,
    /// Full server-authored logical Repel interval at event creation. Each
    /// delivery offer carries only its remaining suffix.
    pub(crate) active_steps: u16,
    /// Every snapshot that actually reached the DataChannel sender. Pacing
    /// rejection and simulated pre-send loss never enter this list.
    pub(crate) delivered_snapshots: [Option<ServerRepelDelivery>; MAX_REPEL_DELIVERY_SNAPSHOTS],
    pub(crate) delivered_snapshot_cursor: u16,
    pub(crate) accepted_application: Option<ServerRepelAcceptedApplication>,
    /// Snapshot that actually carried the source's repel weapon to this
    /// target. Movement authority consumes the discontinuity only through the
    /// exact application marker bound to this identity; ACK is delta state.
    pub(crate) delivered_snapshot_id: Option<u16>,
    /// Server tick encoded in the exact weapon-bearing snapshot. It is retained
    /// for deadline/presentation provenance; logical movement uses the
    /// server-authored step count below.
    pub(crate) delivered_snapshot_tick: Option<u32>,
    /// Exact server-authored logical RepelSpeed duration encoded in the
    /// weapon-bearing snapshot. This is retained with its delivery identity
    /// so client and replay never independently round the tick conversion.
    pub(crate) delivered_active_steps: Option<u16>,
    pub(crate) delivery_acked: bool,
}

impl ServerRepelObservation {
    pub(crate) fn awaiting_application(&self) -> bool {
        self.accepted_application.is_none()
    }

    pub(crate) fn remember_receipt_reliable_id(&mut self, id: u32) {
        let Some(application) = self.accepted_application.as_mut() else {
            return;
        };
        if application.receipt_reliable_ids.contains(&Some(id)) {
            return;
        }
        let slot = usize::from(application.receipt_reliable_cursor) % MAX_REPEL_RECEIPT_COPIES;
        application.receipt_reliable_ids[slot] = Some(id);
        application.receipt_reliable_cursor = application.receipt_reliable_cursor.wrapping_add(1);
    }

    pub(crate) fn receipt_acknowledges_head(&self, id: u32) -> bool {
        self.accepted_application
            .is_some_and(|application| application.receipt_reliable_ids.contains(&Some(id)))
    }

    pub(crate) fn has_receipt_in_flight(&self) -> bool {
        self.accepted_application
            .is_some_and(|application| application.receipt_reliable_ids.iter().any(Option::is_some))
    }

    pub(crate) fn receipt_reliable_id(&self) -> Option<u32> {
        self.accepted_application.and_then(|application| {
            application
                .receipt_reliable_ids
                .into_iter()
                .flatten()
                .next()
        })
    }
}

impl ClientHandle {
    /// Enqueue one immutable target-private Repel without ever displacing an
    /// older unreceipted event. The caller records the observable overflow;
    /// returning `false` is deliberately fail-closed.
    pub(crate) fn try_enqueue_repel(&mut self, event: ServerRepelObservation) -> bool {
        if self.pending_repel_observations.len() >= MAX_PENDING_REPEL_EVENTS {
            return false;
        }
        self.pending_repel_observations.push_back(event);
        true
    }
}

#[derive(Clone)]
pub(crate) struct ActiveMapApplicationTransfer {
    pub(crate) file_index: u16,
    /// Monotonic diagnostic duration only; never used for transfer admission,
    /// ACK ordering, readiness or stall/disconnection policy.
    pub(crate) started_at: Instant,
    /// Immutable logical payload shared by the rare `ClientHandle` rollback
    /// clone instead of copying up to 1 MiB of active asset bytes.
    pub(crate) payload: Arc<[u8]>,
    pub(crate) next_offset: usize,
    /// Assigned fragment ids in exact file order. Later ACKs do not free a
    /// window slot while an earlier fragment remains missing; otherwise one
    /// loss could still let the sender allocate the rest of a large asset far
    /// beyond the client's reorder horizon.
    pub(crate) assigned_ids: VecDeque<u32>,
    pub(crate) acknowledged_ids: BTreeSet<u32>,
    /// Progress tracker for the stall recovery in `note_map_transfer_stall`: the `next_offset` last seen while the
    /// window was full, and the unix millisecond instant at which that fullness began. Zero `stall_since_ms` means no
    /// full window is being observed. Stored per file because each file has its own window and its own progress.
    pub(crate) stall_offset: usize,
    pub(crate) stall_since_ms: u64,
}

#[derive(Clone)]
pub(crate) struct MapTransferState {
    pub(crate) held_files: BTreeSet<u16>,
    pub(crate) counted_held_files: BTreeSet<u16>,
    pub(crate) sent_files: BTreeSet<u16>,
    pub(crate) held_bytes: u64,
    pub(crate) sent_bytes: u64,
    pub(crate) completed_transfers: u16,
    pub(crate) started_at: Instant,
    pub(crate) summary_logged: bool,
    pub(crate) zone_id: &'static str,
    pub(crate) next_index: u16,
    /// A validated request waiting for enough retained-reliable capacity.
    /// At most one application file may be pending for a session.
    pub(crate) pending_index: Option<u16>,
    /// The exact successor requested after the browser has assembled the
    /// current file but before its final transport ACK reaches the server.
    /// Retaining one index closes the application-request/transport-ACK race
    /// without allowing prefetch, skipping, or an unbounded request queue.
    pub(crate) deferred_successor_index: Option<u16>,
    /// At most 64 self-contained file fragments are retained concurrently.
    /// Unlike legacy generic small chunks, roster/lifecycle packets may be
    /// assigned between these fragments without invalidating reassembly.
    pub(crate) active: Option<ActiveMapApplicationTransfer>,
    pub(crate) file_count: u16,
    pub(crate) map_sha256: crate::zone_wire::Sha256Digest,
    /// How many times this session has re-requested a file the server already delivered. A client asks again only when it
    /// could not use what it got, so re-serving is correct - but an asset that is genuinely unusable would otherwise let the
    /// client drive the same transfer forever, so the count is bounded.
    pub(crate) redrive_count: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ResetApplicationTerminal {
    Objective([u8; crate::objective_protocol::ObjectiveStateUpdate::PACKET_LEN]),
    MovementCorrection(crate::movement_correction::MovementCorrectionAccepted),
}

impl ResetApplicationTerminal {
    pub(crate) fn encode(self) -> Vec<u8> {
        match self {
            Self::Objective(packet) => packet.to_vec(),
            Self::MovementCorrection(receipt) => receipt.encode(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ClientHandle {
    /// Newest client protocol revision the browser declared in its arena
    /// login (`ArenaLoginRequest::PROTOCOL_REVISION`); 0 for older clients.
    /// Gates wire forms they could not parse.
    pub(crate) protocol_revision: u8,
    /// Consecutive sent snapshots in which each listed actor was held at its
    /// acknowledged state to fit the send budget
    /// (`snapshot::choose_snapshot_holds`).
    pub(crate) snapshot_holds: std::collections::HashMap<u16, u8>,
    pub(crate) maxed_spawn_loadout: bool,
    pub(crate) spawn_idle: crate::spawn_idle::SpawnIdleTracker,
    pub(crate) session_summary: crate::session_summary::SessionSummary,
    /// Internal transport incarnation. Unlike the reusable Continuum player
    /// id, this value is allocated once and never reused by this server state.
    pub(crate) session_id: PlayerSessionId,
    /// Never-reused generation for the current arena join. Zero means the
    /// registered connection has not completed an arena selection yet.
    pub(crate) arena_stream_id: u64,
    /// Last dedicated event id allocated for this exact recipient arena
    /// stream. Recipient-local allocation avoids sparse-AOI gaps.
    pub(crate) next_realtime_event_id: u32,
    /// Current join's non-authoritative QA trace correlation id. It is never
    /// consulted by gameplay and carries no upload credential.
    pub(crate) qa_trace_bundle_id: Option<[u8; 16]>,
    /// Exact remote actor lives emitted to this recipient and their bounded,
    /// server-owned harmful-readiness deadlines. Codec ACK never mutates it.
    pub(crate) threat_presentation: crate::threat_presentation::ThreatPresentationLedger,
    /// Serializes this connection's authenticated ingress with its removal.
    /// A stale handler may clone a replacement's gate after id reuse, but its
    /// session-token validation then fails before dispatch.
    pub(crate) ingress_gate: Arc<std::sync::Mutex<()>>,
    pub(crate) tx: ClientPacketSender,
    pub(crate) simulated_tx_loss: crate::simulated_loss::SimulatedTxLoss,
    pub(crate) simulated_tx_loss_after_bootstrap: bool,
    pub(crate) player: PlayerSummary,
    pub(crate) player_identity: Option<String>,
    /// Owns this display name and arena slot while login/bootstrap is pending.
    /// Joined state also owns the slot; committed leave releases both together.
    pub(crate) arena_admission_reserved: bool,
    pub(crate) joined: bool,
    /// The gameplay/session join has committed, and the complete reliable
    /// bootstrap has been acknowledged by this exact peer session.
    /// Snapshot emission is gated separately from `joined` so authenticated
    /// input and disconnect cleanup keep their existing lifecycle semantics
    /// while realtime frames cannot overtake bootstrap/PlayerIdAssigned.
    pub(crate) snapshot_ready: bool,
    /// Independent RTC timing lock; absent for native in-memory fixtures.
    pub(crate) connection_quality:
        Option<Arc<std::sync::Mutex<crate::connection_quality::SessionConnectionQuality>>>,
    /// Exact reliable ids reserved by the current session's join bootstrap.
    /// Realtime snapshots remain disabled until this session acknowledges
    /// every id and the initial sink delivery pass has completed.
    pub(crate) bootstrap_reliable_barrier: Option<BootstrapReliableBarrier>,
    pub(crate) outgoing_reliable: ServerReliableSender,
    pub(crate) incoming_reliable: ServerReliableReceiver,
    /// Join-scoped application transfer cursor. The three-byte 0x0c request
    /// carries only an index, so its active zone, ordering, count, and map
    /// identity are authenticated by this exact session state.
    pub(crate) map_transfer: Option<MapTransferState>,
    pub(crate) last_security_report: Option<ClientSecurityReport>,
    pub(crate) last_security_valid: Option<bool>,
    pub(crate) door_state: Option<ClientDoorState>,
    pub(crate) door_seed_cache: Option<crate::doors::DoorSeedCache>,
    pub(crate) server_bounty: u16,
    pub(crate) server_energy: u16,
    pub(crate) server_max_energy: u16,
    pub(crate) server_energy_remainder: f32,
    pub(crate) last_energy_recharge_tick: Option<u32>,
    pub(crate) last_continuous_energy_drain_tick: Option<u32>,
    pub(crate) server_super_until_tick: Option<u32>,
    pub(crate) server_shield_until_tick: Option<u32>,
    /// Prize EngineShutdownTime deadline. EMP has a separate source/deadline;
    /// movement and recharge consult their union.
    pub(crate) server_engine_shutdown_start_sequence: Option<u16>,
    pub(crate) server_engine_shutdown_until_tick: Option<u32>,
    pub(crate) pending_prize_effect_applications:
        std::collections::VecDeque<PendingPrizeEffectApplication>,
    pub(crate) prize_effect_application_terminals:
        std::collections::VecDeque<crate::prize_application::PrizeEffectAccepted>,
    pub(crate) server_emp_start_sequence: Option<u16>,
    pub(crate) server_emp_until_tick: Option<u32>,
    /// Player-session deterministic stream used by prize effects whose
    /// duration is randomized (currently SuperTime).
    pub(crate) server_prize_effect_rng_seed: i32,
    /// Non-zero life-local version of the private prize authority record.
    pub(crate) server_prize_state_revision: u32,
    pub(crate) last_damage_prize_loss_seq: Option<u32>,
    pub(crate) last_damage_prize_loss_id: Option<i16>,
    /// Server-owned RocketTime deadline. Wire input bit 5 is only a one-record
    /// activation pulse; movement replay consumes the canonical active
    /// interval derived from this deadline and retained authorization.
    pub(crate) server_rocket_until_tick: Option<u32>,
    /// Latest forward raw bit-5 pulse state, retained across timer expiry so a
    /// client cannot extend RocketTime merely by holding/repeating true.
    pub(crate) server_rocket_input_asserted: bool,
    /// Recent server-owned logical Rocket intervals. Kept past wall-clock
    /// expiry until their input sequences age out so late redundant history
    /// receives the same canonical movement bit it had on first arrival.
    pub(crate) server_rocket_authorizations: std::collections::VecDeque<ServerRocketAuthorization>,
    pub(crate) server_status: StatusInventory,
    pub(crate) server_weapons: WeaponInventory,
    /// Exceptional previously-authorized position energy ceiling. Nonzero
    /// only while an exact negative-Energy private ACK may be in flight; an
    /// admitted converged report retires or monotonically narrows it.
    pub(crate) last_reported_energy: u16,
    pub(crate) last_position: Option<Vec<u8>>,
    /// Receiver-side counts for the session facts a failing smoke must carry. Sender-side counters are ambiguous - they
    /// reset when the client rebuilds its network object - so these record what this server accepted from the session.
    pub(crate) inputs_received: u32,
    pub(crate) positions_received: u32,
    /// Bound for the per-id acknowledgement instrument, so a stall cannot produce an unbounded retained log.
    pub(crate) ack_ids_logged: u32,
    /// Last admitted position used only to anchor this recipient's AOI while
    /// their live position is intentionally cleared during the death delay.
    /// It cannot authorize movement, weapons, collisions, or make the dead
    /// player visible to others, and is cleared at non-death boundaries.
    pub(crate) aoi_observer_position: Option<Vec<u8>>,
    pub(crate) spectator_view_sequence: Option<u16>,
    pub(crate) spectator_next_input_sequence: Option<u16>,
    /// Last admitted authoritative safe-tile classification. This survives
    /// death/respawn position clearing so objective reward eligibility does
    /// not silently change while the player remains joined and non-spectating.
    pub(crate) objective_last_safe: bool,
    pub(crate) visibility_cell: Option<VisibilityCell>,
    pub(crate) last_position_server_tick: Option<u32>,
    /// Server-owned cumulative simulation time; survives life/arena rebases.
    pub(crate) movement_time_budget: MovementTimeBudget,
    pub(crate) last_position_relay_at: Option<Instant>,
    pub(crate) position_relay_tokens: f32,
    pub(crate) position_relay_burst_count: u8,
    pub(crate) last_weapon_fire_tick: Option<u32>,
    pub(crate) last_gun_fire_tick: Option<u32>,
    pub(crate) last_bomb_fire_tick: Option<u32>,
    pub(crate) objective_weapon_lock_until_tick: Option<u32>,
    pub(crate) last_accepted_weapon_data: Option<u16>,
    pub(crate) last_accepted_weapon_tick: Option<u32>,
    /// Life-scoped canonical witnesses which admitted projectiles. A
    /// powerball kick and a weapon may never consume the same witness, even
    /// when either reliable/unordered path is delayed behind later inputs.
    pub(crate) accepted_weapon_witnesses: CanonicalWitnessConsumptionLedger,
    /// Life-scoped canonical witnesses which admitted powerball kicks.
    /// Retained across retries and the complete canonical witness history.
    pub(crate) objective_ball_fire_witnesses: CanonicalWitnessConsumptionLedger,
    pub(crate) last_special_weapon_request: Option<u8>,
    pub(crate) last_brick_drop_tick: Option<u32>,
    pub(crate) chat_rate_debt_until: Option<Instant>,
    pub(crate) last_ship_change_at: Option<Instant>,
    pub(crate) ship_change_burst_count: u8,
    pub(crate) last_frequency_change_at: Option<Instant>,
    pub(crate) frequency_change_burst_count: u8,
    pub(crate) last_attach_request_at: Option<Instant>,
    pub(crate) attach_request_burst_count: u8,
    pub(crate) alive: bool,
    pub(crate) respawn_at_tick: Option<u32>,
    /// Server-owned life/loadout generation. Incremented when a player dies
    /// or changes ships so tests and future projectile ownership can
    /// distinguish stale pre-death state from the same player id after
    /// respawn. This is a logical counter, not a timestamp.
    pub(crate) life_seq: u32,
    /// Server tick at which the current life/loadout began. Projectiles
    /// spawned before this tick are stale for this player and cannot damage
    /// the new life after a respawn or ship change.
    pub(crate) life_started_tick: Option<u32>,
    /// Most recently received `PlayerInputRecord` from this client
    /// (input bits + direction). The server movement sim consumes this
    /// when accepting the next position update. The sequence is the latest
    /// accepted input entry; its per-record door byte is retained separately
    /// with the canonical security-derived wall-clock phase below.
    pub(crate) last_input_seq: Option<u16>,
    pub(crate) last_input_record: Option<ClientInputObservation>,
    /// Bounded sequence-indexed input history. Input and position messages
    /// share an unordered realtime channel, so the newest input received is
    /// not necessarily the one paired with the position currently drained by
    /// the server tick. The movement observer resolves the exact sequence.
    pub(crate) recent_input_records: std::collections::VecDeque<SequencedInputObservation>,
    /// Raw bit-5 values by sequence. Canonical observations deliberately do
    /// not retain this untrusted value, but overlap/rewrite validation still
    /// has to compare retransmitted wire payloads exactly.
    pub(crate) recent_input_rocket_assertions: std::collections::VecDeque<(u16, bool)>,
    /// Most recent zubspace extended-position sequence accepted or rejected
    /// for this player. This is a logical counter, not time; it lets the
    /// server drop duplicated/replayed movement and weapon position frames
    /// without consulting client timestamps.
    pub(crate) last_position_sequence: Option<u16>,
    /// First input/position sequence belonging to a newly requested ship
    /// life. The browser pauses realtime emission until the reliable roster
    /// acknowledgement; packets before this wrap-aware boundary are late
    /// traffic from the previous life and are dropped before any mutation.
    /// The boundary remains life-scoped; once accepted progress moves beyond
    /// the input-history window, ordinary wrap-aware replay age provides the
    /// same protection without blocking the eventual u16 wrap.
    pub(crate) life_position_sequence_base: Option<u16>,
    /// Canonical physical baseline captured by validating the reliable
    /// frequency request under the pre-cutover runtime identity. The first
    /// new-frequency position must match every field exactly before it can
    /// consume this boundary.
    pub(crate) pending_frequency_baseline:
        Option<crate::continuum_position::ClientPositionSnapshot>,
    /// Server-accepted Warp greens awaiting their exact neutral-velocity
    /// relocation report. Reliable pickup ACKs are ordered but may be delayed
    /// behind arbitrarily many retries, so authorizations do not expire by
    /// wall clock. They remain bounded per player and are cleared at the
    /// life/session/arena boundary.
    pub(crate) pending_warp_prize_relocations:
        std::collections::VecDeque<PendingWarpPrizeRelocation>,
    /// Recently consumed explicit Warp markers for idempotent repeats while
    /// the reliable application receipt is in flight.
    pub(crate) consumed_warp_prize_relocations: std::collections::VecDeque<u32>,
    /// [REVIEW M3] Wrap-aware weapon-sequence replay window, kept separate
    /// from `last_position_sequence` so a delayed reliable weapon mirror can
    /// survive later movement-only frames while still being accepted at most
    /// once. `latest_weapon_seq` is the newest consumed weapon sequence;
    /// `weapon_seq_mask` bit `d` records whether `latest - d` was consumed
    /// (64-wide trailing window). Sequences older than the window are
    /// treated as already-seen, so a client can never replay an old weapon
    /// by first advancing its sequence far ahead — the previous 512-entry
    /// membership ring evicted old entries and re-opened them.
    pub(crate) latest_weapon_seq: Option<u16>,
    pub(crate) weapon_seq_mask: u64,
    pub(crate) latest_portal_action_id: Option<u32>,
    pub(crate) portal_action_mask: u64,
    pub(crate) portal_placement_terminals:
        std::collections::VecDeque<crate::portal_placement::PortalPlacementAccepted>,
    pub(crate) portal_warp_terminals:
        std::collections::VecDeque<crate::portal_warp::PortalWarpAccepted>,
    pub(crate) active_portal: Option<ServerPortal>,
    pub(crate) server_warp_settle_until_tick: Option<u32>,
    pub(crate) server_warp_bomb_lock_until_tick: Option<u32>,
    /// Bounded, life-scoped canonical movement states for accepted position
    /// sequences and any exact intermediate input records reconstructed by a
    /// later accepted tail. Delayed weapon mirrors may use only this history;
    /// the current public position is never a grace-radius origin witness.
    pub(crate) canonical_position_witnesses: std::collections::VecDeque<CanonicalPositionWitness>,
    /// Sole full-precision anchor for the next forward movement replay.
    /// Discontinuities clear this anchor while retaining the separate bounded
    /// weapon-origin history above; historical entries can therefore prove a
    /// delayed weapon mirror but can never seed later movement.
    pub(crate) canonical_movement_witness: Option<CanonicalPositionWitness>,
    pub(crate) canonical_mismatch_log_gate: crate::observability::DiagnosticRateGate,
    /// Server-simulated movement state. Seeded from accepted client
    /// positions; the server runs one simulation step per accepted
    /// position as a private validation/correction model and re-seeds
    /// from accepted reported state on large divergence. Remote snapshots
    /// use one motion source: the server-validated accepted position.
    pub(crate) server_sim_state: Option<ServerSimMovement>,
    pub(crate) server_sim_anchor_tick: Option<u32>,
    /// Input bits paired with the position that seeded the current private
    /// simulation state. Equal consecutive values identify intervals whose
    /// full input state is known; changed values are transition diagnostics.
    pub(crate) server_sim_input_bits: Option<u16>,
    /// Cheat-snap-back drift accumulator (signed, in pixels). Each accepted
    /// position adds the gap between the client's reported position and the
    /// authoritative one-step sim prediction, with a per-sample leak. Honest
    /// jitter is sign-random and stays bounded near zero; a client that
    /// systematically gains ground accumulates here until it crosses the
    /// snap threshold, at which point the server proposes a recipient-private
    /// exact rebase. This is default-on and remains explicitly configurable.
    /// Reset on respawn/ship-change/re-seed so honest play never trips it.
    pub(crate) movement_drift_x: f32,
    pub(crate) movement_drift_y: f32,
    /// At most one correction transaction may own this runtime. Detection
    /// retains the proposal without changing canonical movement; application
    /// commits it at the client's later barrier tail, and the transaction is
    /// retired only when the reliable server receipt is transport-ACKed.
    pub(crate) pending_movement_correction: Option<ServerMovementCorrection>,
    /// Server-issued correction identities retired by a soft life boundary.
    /// An exact application already in flight may arrive after death/respawn;
    /// retaining only its immutable proposal scope lets ingress classify that
    /// packet as an inert duplicate without accepting arbitrary stale lives.
    /// The ring belongs to this never-reused transport session and is cleared
    /// at every hard arena-stream boundary.
    pub(crate) retired_movement_corrections: VecDeque<RetiredMovementCorrection>,
    /// Session-scoped reset terminals that could not yet reserve outgoing
    /// reliable ids. Unlike the life-scoped transaction above these survive
    /// reset life advancement, so ACKed objective/correction applications get
    /// explicit ordered closure even under sender saturation. The retired
    /// unified FIFO bounds this queue to at most 64 entries.
    pub(crate) pending_reset_application_terminals: VecDeque<ResetApplicationTerminal>,
    pub(crate) next_movement_correction_generation: u32,
    /// Debug-build E2E fault injection is one-shot per life/runtime.
    pub(crate) movement_correction_test_forced: bool,
    /// Phase B (AOI hysteresis): this recipient's last-classified AOI tier
    /// for each other player, rebuilt every emit from the snapshot buckets.
    /// The classifier widens the radii for a tier the entity is already in
    /// (sticky), so an entity hovering on the near/mid boundary doesn't
    /// flicker tiers (and cause render re-snaps) frame to frame.
    pub(crate) aoi_tier_memory: std::collections::HashMap<u16, crate::visibility::AoiTier>,
    /// Live prize-mutated movement values. The reference game clamps after each
    /// operation, so partial maxima cannot be represented by upgrade counts.
    pub(crate) server_thrust_acceleration: f32,
    pub(crate) server_top_speed: f32,
    pub(crate) server_rotation_per_second: f32,
    pub(crate) server_recharge_per_second: f32,
    /// Original server-owned RepelTime deadline. It remains bounded replay
    /// provenance briefly after wall-clock expiry so a pre-deadline input
    /// received out of order is still evaluated against the right state.
    pub(crate) server_repel_until_tick: Option<u32>,
    /// Exact logical input window whose post-application movement uses the
    /// RepelSpeed cap. `(application_sequence, exclusive_end)` is derived by
    /// the server when the first exact 0x79 application report commits, using
    /// the exact step count encoded in the matching weapon frame. This
    /// prevents packet arrival and receiver tick reconstruction from changing
    /// already-simulated client frames.
    pub(crate) server_repel_sequence_window: Option<(u16, u16)>,
    /// Private-observer-only estimate of how long the browser can still be
    /// applying RepelSpeed after it decoded the exact weapon-bearing
    /// snapshot. Snapshot delivery may lag the server event substantially;
    /// the browser starts its local RepelTime when it applies that frame.
    /// This deadline never participates in position admission or grants a
    /// velocity discontinuity. It only prevents the optional diagnostic
    /// snap-back model from treating the known delivery skew as cheating.
    pub(crate) server_repel_observer_until_tick: Option<u32>,
    /// Immutable hostile-Repel events serialized for this exact target life.
    /// Only the head may be offered/applied. It remains until the reliable
    /// 0x7a receipt is transport-ACKed, so later Repels can neither overwrite
    /// nor overtake an applied-but-unreceipted event.
    pub(crate) pending_repel_observations: std::collections::VecDeque<ServerRepelObservation>,
    /// Recently retired exact applications make cross-channel late/duplicate
    /// 0x79 markers idempotent after the receipt's transport ACK advanced the
    /// live queue. Entries are life-scoped and bounded.
    pub(crate) retired_repel_applications:
        std::collections::VecDeque<ServerRepelRetiredApplication>,
    /// Phase 2e: short ring of recently-accepted positions, used by
    /// lag-compensated hit detection to rewind a victim's position
    /// to where they were when the shooter's projectile would have
    /// visually hit them on the shooter's screen. Capped at
    /// `MAX_POSITION_HISTORY` entries (~20 server ticks ≈ 200 ms);
    /// older samples are evicted FIFO.
    pub(crate) position_history: std::collections::VecDeque<RecentPositionSample>,
    /// Sole production combat-motion history, populated from exact accepted
    /// movement witnesses and explicitly segmented at discontinuities.
    pub(crate) actor_motion_history: crate::combat_timeline::ActorMotionHistory,
    /// Exact render-grade snapshots that successfully left this recipient's
    /// outbound queue. This is intentionally distinct from `snapshot_history`,
    /// which is populated before pacing/loss and exists only for delta bases.
    pub(crate) observer_presentation_history: crate::combat_timeline::ObserverPresentationHistory,
    /// Sole server-owned observer-wide combat-view schedule for this
    /// authenticated session/arena stream.
    pub(crate) observer_combat_timeline: crate::combat_timeline::ObserverCombatTimeline,
    pub(crate) adaptive_presentation: crate::connection_quality::AdaptivePresentationPolicy,
    /// Accepted old-life schedules/watermarks, retained only while that life
    /// has authoritative flight. Emitted target frames stay in one bounded
    /// session ring; do not duplicate an actor history for every retired life.
    pub(crate) retired_projectile_observers:
        BTreeMap<LifeIdentity, crate::combat_timeline::ObserverCombatTimeline>,
    /// Phase 3e-1: monotonic per-recipient snapshot id, allocated
    /// from the server side. Wraps cleanly (u16) at ~36 minutes of
    /// continuous play at 30 Hz.
    pub(crate) snapshot_seq: u16,
    /// Debug-only delayed-baseline replay starts counting when this session
    /// first becomes alive, rather than during its initial asset/bootstrap
    /// transfer.
    #[cfg(debug_assertions)]
    pub(crate) qa_prize_baseline_delay_start_snapshot: Option<u16>,
    /// Phase 3e-4a: most recent `snapshot_id` this client has
    /// acknowledged. `None` until the first ack arrives — the
    /// server falls back to an explicitly flagged full snapshot when
    /// there's no acknowledged base in history.
    pub(crate) last_acked_snapshot_id: Option<u16>,
    /// Bounded by the active registry; refreshing poses never resets harmful readiness.
    pub(crate) projectile_refresh_pending: std::collections::BTreeSet<u32>,
    /// Held-flight facts actually retained in the realtime queue. A mismatch
    /// forces a new scoped state; bounded/pruned by the active registry.
    pub(crate) projectile_motion_frozen_published: std::collections::BTreeSet<u32>,
    pub(crate) last_projectile_refresh: Option<Instant>,
    /// Phase 3e-4b: ring of recently-emitted snapshots' entity maps.
    /// Capacity capped at `SNAPSHOT_HISTORY_CAPACITY`; entries older
    /// than `last_acked_snapshot_id` are evicted on each emit. Used
    /// to delta-encode new snapshots against the client's last-acked
    /// state. Lives on `ClientHandle` to stay co-located with the
    /// outbound tx + ack tracking.
    pub(crate) snapshot_history: std::collections::VecDeque<crate::snapshot::SnapshotHistoryEntry>,
    /// Phase 3e-5: events queued for the next snapshot emission.
    /// `server_damage` pushes WATCH_DAMAGE entries here for each
    /// near-AOI recipient; `emit_snapshots_for_tick` drains and
    /// encodes them into the frame's `events[]` region.
    pub(crate) pending_snapshot_events: crate::snapshot::PendingSnapshotEvents,
    /// Independently bounded authoritative 0x7d one-shot facts. This queue is
    /// not constrained by the snapshot wire's one-byte compatibility-event
    /// count and never evicts an accepted fact.
    pub(crate) pending_realtime_events: crate::snapshot::PendingRealtimeEvents,
    /// One-shot weapon data that has been accepted by the server but has not
    /// yet been exposed through a snapshot. This prevents fast follow-up
    /// position packets with weapon=0 from erasing remote bomb/bullet visuals
    /// before the 30 Hz snapshot stream samples them.
    pub(crate) pending_snapshot_weapon: u16,
    /// Phase 2 polish: per-victim ring of recent damagers. Pushed
    /// onto each server-authoritative hit and consumed by the kill
    /// pipeline to award assists to non-killing damagers within the
    /// assist window. Cleared on respawn / ship change via
    /// `clear_server_runtime`.
    pub(crate) recent_damagers: Vec<(LifeIdentity, u32)>,
    /// Phase 2 polish (RTT): EWMA-smoothed round-trip time measured
    /// from 0x70 snapshot emit to 0x71 ack arrival. `None` until the
    /// first ack carries a usable measurement; falls back to the
    /// default rewind in `step()`.
    pub(crate) measured_rtt: Option<std::time::Duration>,
    /// Phase 2 polish (RTT): pre-computed `(measured_rtt / 2 /
    /// 10ms).min(MAX_LAG_COMPENSATION_TICKS)`. Cached on every ack so
    /// the projectile step does an O(1) field read per shooter
    /// instead of recomputing from `Duration` arithmetic.
    pub(crate) lag_compensation_ticks: u8,
    /// Phase 4 send pacing: per-client outbound rate limiter. Skips
    /// replaceable snapshot frames when the per-tick byte budget is exhausted
    /// and retains dedicated one-shot facts in their bounded FIFO for a later
    /// tick. Reliable control packets bypass this gate.
    pub(crate) send_pacing: SendPacing,
}

/// Phase 4 send pacing: bytes-per-second token bucket per client.
/// The snapshot emit path calls `try_consume` before each frame; if
/// the bucket is exhausted a replaceable frame is skipped while a dedicated
/// event remains queued. Reliable control packets are not paced.
///
/// The 64 KiB/s cap is intentionally generous relative to the current
/// synthetic scale benchmark and should only fire under a real runaway
/// (for example, an AOI density spike with oversized snapshots).
#[derive(Clone, Copy, Debug)]
pub(crate) struct SendPacing {
    /// Available bytes. Refills each server tick by
    /// `cap_bytes_per_second / tick_rate_hz`, clamped at the cap so
    /// idle clients can absorb a single burst up to `cap`.
    pub(crate) tokens: f64,
    pub(crate) cap_bytes_per_second: f64,
    pub(crate) packets_dropped: u64,
    pub(crate) bytes_dropped: u64,
}

impl SendPacing {
    pub(crate) fn refill_for_tick(&mut self, tick_rate_hz: u16) {
        let per_tick = self.cap_bytes_per_second / f64::from(tick_rate_hz.max(1));
        self.tokens = (self.tokens + per_tick).min(self.cap_bytes_per_second);
    }

    /// Try to spend `bytes` tokens. Returns true on success; on
    /// failure increments the drop counters so the periodic summary
    /// surfaces it without touching the hot path.
    pub(crate) fn try_consume(&mut self, bytes: usize) -> bool {
        let cost = bytes as f64;
        if self.tokens >= cost {
            self.tokens -= cost;
            true
        } else {
            self.packets_dropped = self.packets_dropped.saturating_add(1);
            self.bytes_dropped = self.bytes_dropped.saturating_add(bytes as u64);
            false
        }
    }
}

/// Default per-client snapshot and event budget, `send_budget_kib_per_second`.
/// Steady state at 200 spread-out players is about 6.6 KiB/s; a dense fight
/// of 48 ships around one player measured 54-60 KB/s on dev.
pub(crate) const DEFAULT_SEND_BUDGET_KIB_PER_SECOND: u32 = 64;

impl SendPacing {
    /// A full bucket refilled at `kib_per_second`, which it can also burst.
    pub(crate) fn with_budget_kib(kib_per_second: u32) -> Self {
        let cap = f64::from(kib_per_second) * 1024.0;
        Self {
            tokens: cap,
            cap_bytes_per_second: cap,
            packets_dropped: 0,
            bytes_dropped: 0,
        }
    }
}

impl Default for SendPacing {
    fn default() -> Self {
        Self::with_budget_kib(DEFAULT_SEND_BUDGET_KIB_PER_SECOND)
    }
}

/// Phase 2 polish: how far back to consider a damager an assister.
/// Mirrors the 5-second-ish window typical of Continuum-derived
/// scoreboards. Anything older than this when a lethal hit lands is
/// no longer credited.
pub(crate) const ASSIST_DAMAGE_WINDOW_TICKS: u32 = 5 * 100;

/// Cap on the recent-damagers ring per victim. With sustained combat
/// at 100 Hz fire cadence and the 5-second window, ~50 entries cover
/// the worst plausible case; the cap evicts oldest first to keep
/// memory bounded under degenerate inputs.
pub(crate) const RECENT_DAMAGERS_CAPACITY: usize = 64;

/// Server-tick anchored position sample for the lag-compensation
/// history. `tick` is `crate::wire::current_tick()` at the moment
/// the position was accepted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RecentPositionSample {
    pub(crate) tick: u32,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

pub(crate) const MAX_POSITION_HISTORY: usize = 20;

/// Phase 2 polish (RTT): default rewind tick count when no per-shooter
/// RTT has been measured yet. Matches the pre-RTT placeholder so
/// freshly-joined clients aren't penalized by a zero-rewind window
/// before their first snapshot ack lands.
pub(crate) const DEFAULT_LAG_COMPENSATION_TICKS: u8 = 5;

/// Phase 2 polish (RTT): hard cap on per-shooter lag compensation.
/// 20 Continuum ticks (200 ms) follows the spec note in
/// `authority_projectiles::LAG_COMPENSATION_TICKS` — beyond this a
/// high-ping shooter can no longer rewind a victim arbitrarily far.
pub(crate) const MAX_LAG_COMPENSATION_TICKS: u8 = 20;

/// Phase 2 polish (RTT): EWMA weight for new RTT samples
/// (`new = (1 - alpha) * old + alpha * sample`). 1/8 mirrors TCP's
/// classic smoothing — fast enough to track route changes, slow
/// enough to ignore single-sample jitter.
pub(crate) const RTT_SMOOTHING_ALPHA_NUM: u32 = 1;
pub(crate) const RTT_SMOOTHING_ALPHA_DEN: u32 = 8;

/// Server-side mirror of player kinematics for Phase 1c-2 observation.
/// Mirrors `gameplay::PlayerMovementState` (the canonical type pinned
/// by the parity tests in `gameplay/player_movement.rs`) but lives in
/// the server crate's tree so it doesn't drag the whole `gameplay/`
/// module in via `#[path]`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ServerSimMovement {
    /// Private full-precision canonical position. Starting the next replay
    /// from rounded u16 relay coordinates can put an otherwise-clear hull
    /// exactly back onto a door boundary and create a false second bounce.
    pub(crate) position_x: f32,
    pub(crate) position_y: f32,
    pub(crate) orientation: f32,
    pub(crate) velocity_x: f32,
    pub(crate) velocity_y: f32,
    pub(crate) energy: f32,
    pub(crate) max_energy: f32,
}

/// Exact server reconstruction bound to one input/position sequence. Float
/// state is retained for the next deterministic replay; wire components are
/// retained separately so a delayed weapon mirror must reproduce the exact
/// quantized vector the server authorized for that historical sequence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CanonicalPositionWitness {
    pub(crate) sequence: u16,
    pub(crate) life_seq: u32,
    pub(crate) state: ServerSimMovement,
    pub(crate) wire_x: u16,
    pub(crate) wire_y: u16,
    pub(crate) wire_velocity_x: i16,
    pub(crate) wire_velocity_y: i16,
    pub(crate) direction: u8,
    pub(crate) door_seed: u8,
    pub(crate) door_phase_tick: u32,
}

/// Wrap-safe, bounded consumption set for the same 64 input witnesses retained
/// by position authority. Sequences older than the window fail closed as
/// already consumed; a forward sequence is fresh until it is recorded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CanonicalWitnessConsumptionLedger {
    latest: Option<u16>,
    mask: u64,
}

impl CanonicalWitnessConsumptionLedger {
    pub(crate) fn latest(self) -> Option<u16> {
        self.latest
    }

    pub(crate) fn contains(self, sequence: u16) -> bool {
        let Some(latest) = self.latest else {
            return false;
        };
        let ahead = sequence.wrapping_sub(latest);
        if ahead != 0 && ahead < 0x8000 {
            return false;
        }
        let behind = latest.wrapping_sub(sequence);
        if behind < u64::BITS as u16 {
            self.mask & (1_u64 << behind) != 0
        } else {
            true
        }
    }

    pub(crate) fn insert(&mut self, sequence: u16) {
        let Some(latest) = self.latest else {
            self.latest = Some(sequence);
            self.mask = 1;
            return;
        };
        let ahead = sequence.wrapping_sub(latest);
        if ahead != 0 && ahead < 0x8000 {
            self.mask = if u32::from(ahead) >= u64::BITS {
                0
            } else {
                self.mask << ahead
            };
            self.mask |= 1;
            self.latest = Some(sequence);
            return;
        }
        let behind = latest.wrapping_sub(sequence);
        if behind < u64::BITS as u16 {
            self.mask |= 1_u64 << behind;
        }
    }
}

impl PartialEq for ServerSimMovement {
    fn eq(&self, other: &Self) -> bool {
        fn same_float(left: f32, right: f32) -> bool {
            left == right || (left.is_nan() && right.is_nan())
        }

        same_float(self.position_x, other.position_x)
            && same_float(self.position_y, other.position_y)
            && same_float(self.orientation, other.orientation)
            && same_float(self.velocity_x, other.velocity_x)
            && same_float(self.velocity_y, other.velocity_y)
            && same_float(self.energy, other.energy)
            && same_float(self.max_energy, other.max_energy)
    }
}

/// Compact server-side mirror of the most recent input the client
/// reported. Kept thin (no `Vec`) so a busy server doesn't allocate
/// per packet. This intentionally stores no client timing byte/timestamp; the
/// server simulation derives elapsed time from server ticks only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClientInputObservation {
    pub(crate) bits: u16,
    pub(crate) direction: u8,
}

/// One server-authorized Rocket activation. `deadline_tick` is anchored from
/// the sequence-relative remainder observed on receipt; the logical interval
/// remains the exact authority for historical movement and retrigger overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ServerRocketAuthorization {
    pub(crate) start_sequence: u16,
    pub(crate) start_tick: u32,
    pub(crate) deadline_tick: u32,
    pub(crate) lifetime_ticks: u32,
}

impl ServerRocketAuthorization {
    /// The client decrements RocketTime before each 60 Hz movement step. A
    /// record at offset `d` is therefore active exactly while
    /// `(d + 1) * (100 / 60) < lifetime_ticks`.
    pub(crate) fn contains_sequence(self, sequence: u16) -> bool {
        crate::gameplay_rules::rocket_sequence_is_active(
            self.start_sequence,
            self.lifetime_ticks,
            sequence,
        )
    }

    pub(crate) fn last_active_sequence(self) -> Option<u16> {
        let active_count = (u64::from(self.lifetime_ticks) * 3).saturating_sub(1) / 5;
        (active_count > 0).then(|| {
            self.start_sequence
                .wrapping_add(active_count.saturating_sub(1).min(u64::from(u16::MAX)) as u16)
        })
    }

    #[cfg(test)]
    pub(crate) fn remaining_ticks_after(self, observed_sequence: u16) -> Option<u32> {
        crate::gameplay_rules::rocket_remaining_ticks_after_sequence(
            self.start_sequence,
            self.lifetime_ticks,
            observed_sequence,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SequencedInputObservation {
    pub(crate) sequence: u16,
    pub(crate) input: ClientInputObservation,
    /// Browser-selected seed, admitted only after it maps to a canonical
    /// security-derived adjacent phase.
    pub(crate) door_seed: u8,
    /// Canonical start tick of that phase. Unlike the randomized seed byte,
    /// this is ordinal and makes same/forward phase enforcement wrap-safe.
    pub(crate) door_phase_tick: u32,
}

/// Server-side replay retention is deliberately wider than one client's
/// 32-entry redundancy packet. Several overlapping packets reconstruct a
/// longer exact history, allowing a later position to recover after both
/// unordered-channel omission and an intervening transport-expired packet.
/// Admission still requires every sequence in the gap; this is retention,
/// not extrapolation or a generic movement rebase.
pub(crate) const MAX_RECENT_INPUT_OBSERVATIONS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlayerStats {
    pub(crate) assists: u32,
    pub(crate) kill_points: u32,
    pub(crate) flag_points: u32,
    pub(crate) wins: u16,
    pub(crate) losses: u16,
}

impl Default for PlayerStats {
    fn default() -> Self {
        Self {
            assists: 0,
            kill_points: 0,
            flag_points: 0,
            wins: 0,
            losses: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerBrick {
    /// Every map cell the wall occupies. A 1-tile brick has one
    /// entry; a multi-tile wall has up to `arena_brick_span()`
    /// entries (capped by the perpendicular-to-heading layout
    /// halting at existing solid tiles). Collision callers
    /// (`arena/positions.rs::enemy_brick_tiles`, projectile step
    /// when wired) iterate over the whole list — one wall =
    /// one packet but N collision cells.
    pub(crate) tiles: Vec<(u16, u16)>,
    pub(crate) packet: Vec<u8>,
    pub(crate) expires_at_tick: u32,
}

pub(crate) type ServerProjectile = ProjectileRecord;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ServerPrizeClaim {
    pub(crate) collectible_id: u32,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) prize_id: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingWarpPrizeRelocation {
    pub(crate) collectible_id: u32,
    pub(crate) life_seq: u32,
    pub(crate) expected_energy: u16,
    pub(crate) expected_bounty: u16,
    /// Immutable private revision produced by this exact claim. Deferred ACK
    /// release must not borrow a later unrelated mutation's revision.
    pub(crate) prize_state_revision: u32,
    /// Immutable winner ACK payload, held server-side while any applicable
    /// target-private Repel remains in this runtime's FIFO.
    pub(crate) claim: ServerPrizeClaim,
    pub(crate) pickup_ack_sent: bool,
}

/// A client cannot collect unbounded outstanding Warp greens while reliable
/// delivery is stalled. A full queue rejects the next distinct Warp claim
/// before the collectible is removed; existing authorizations remain valid.
pub(crate) const MAX_PENDING_WARP_PRIZE_RELOCATIONS: usize = 64;
pub(crate) const MAX_CONSUMED_WARP_PRIZE_RELOCATIONS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingPrizeEffectApplication {
    pub(crate) collectible_id: u32,
    pub(crate) prize_id: i16,
    pub(crate) life_seq: u32,
    pub(crate) prize_state_revision: u32,
    pub(crate) minimum_application_sequence: Option<u16>,
    pub(crate) maximum_application_sequence: Option<u16>,
    /// Newest exact marker identity observed for this FIFO entry. Terminal
    /// cancellation is only meaningful for a marker the server has actually
    /// received; inferred boundaries are never substituted for its identity.
    pub(crate) latest_application_sequence: Option<u16>,
    /// Exact client-declared successor held while an already-generated
    /// unordered input tail is still in flight. The reliable marker may
    /// overtake that tail, but it never moves the authority boundary
    /// backward merely because the server has not observed the tail yet.
    pub(crate) deferred_application: Option<crate::prize_application::PrizeEffectApplied>,
    /// Exact movement tuning in force before this prize revision. While the
    /// client has not completed the reliable application boundary, canonical
    /// replay must continue using these raw f32 values. Bits keep this state
    /// Eq/copyable and preserve the precise browser/server arithmetic contract.
    pub(crate) movement_tuning_before_bits: Option<[u32; 4]>,
}

pub(crate) const MAX_PENDING_PRIZE_EFFECT_APPLICATIONS: usize = 64;
pub(crate) const MAX_PRIZE_EFFECT_APPLICATION_TERMINALS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ServerHiddenPrize {
    pub(crate) collectible_id: u32,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) prize_id: i16,
    pub(crate) expires_at_tick: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ServerPrizeReceipt {
    pub(crate) claim: ServerPrizeClaim,
    pub(crate) winner: PlayerRuntimeIdentity,
    pub(crate) claimed_at_tick: u32,
    /// Exact post-effect energy echoed in every idempotent winner ACK.
    pub(crate) authoritative_energy: u16,
    pub(crate) prize_state_revision: u32,
    pub(crate) engine_shutdown_until_tick: Option<u32>,
    pub(crate) engine_shutdown_application_required: bool,
}

/// Recent winners retained for idempotent claim retries. Collectible ids are
/// never reused, so evicting the oldest receipt can only turn a very late
/// winner retry into a reject; it can never make the prize collectible again.
pub(crate) const MAX_PRIZE_CLAIM_RECEIPTS: usize = 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClientSecurityReport {
    pub(crate) settings_checksum: u32,
    pub(crate) map_checksum: u32,
    pub(crate) continuum_map_crc32: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SignalingRateWindow {
    pub(crate) started_at: Instant,
    pub(crate) offers: usize,
}

#[derive(Default)]
pub(crate) struct ServerState {
    pub(crate) config: ServerConfig,
    /// Native collision fixtures may supply authored spawn settings to keep
    /// controlled opposing hulls together. The production build has no
    /// override, environment flag or alternative admission path.
    #[cfg(test)]
    pub(crate) fixture_spawn_settings_packet: Option<Vec<u8>>,
    pub(crate) telemetry: crate::observability::Telemetry,
    /// Survives transport/arena replacement. Only a committed healthy return
    /// may clear a connection-quality hold; ordinary disconnect must not.
    pub(crate) quality_holds: crate::connection_quality::holds::QualityHolds,
    /// Arena-wide transaction fence. Ordinary authenticated ingress takes a
    /// shared guard; the 60 Hz authoritative tick and admin reset take an
    /// exclusive guard. This lets reset exclude new aliases while it stages
    /// and commits pending lifecycle/objective closure without serializing
    /// independent connections against one another during normal play.
    pub(crate) arena_transaction_gate: Arc<std::sync::RwLock<()>>,
    /// Shared bounded blocking ingress execution, initialized by the RTC
    /// runtime. Existing channel queues remain the sole packet backlog.
    pub(crate) rtc_ingress_executor: std::sync::OnceLock<Arc<crate::rtc::RtcIngressExecutor>>,
    /// Bounded validated discovery metadata prepared before the production
    /// listener binds. HTTP adds live counts from a single state snapshot.
    /// `None` is fail-closed test/setup state; HTTP never parses map/LVZ assets.
    pub(crate) zone_catalog_json: Option<Arc<str>>,
    pub(crate) simulated_tx_loss: crate::simulated_loss::SimulatedTxLoss,
    pub(crate) arena_tick: u64,
    pub(crate) last_tick_at: Option<Instant>,
    /// Whether the tick in flight also produced a cadence sample. A cadence rebase (a zone switch
    /// clearing `last_tick_at`, so the switch itself is not charged as jitter) leaves the next tick
    /// with no interval to record; its work sample is withheld for the same reason. The three
    /// `tick_timing` populations then count exactly the same tick set, which is the invariant the
    /// browser gates assert in `tests/e2e/server_timing_evidence.mjs`. Load-shedding state is
    /// unaffected: the wallclock EWMA and overrun streak still see every tick.
    pub(crate) tick_cadence_sampled: bool,
    /// The cadence sample of the tick in flight. It is recorded together with that tick's work
    /// sample when the tick finishes, so no `/status` snapshot can observe a tick boundary whose
    /// work sample has not caught up with it.
    pub(crate) pending_interval_us: Option<u64>,
    pub(crate) pending_jitter_us: Option<u64>,
    pub(crate) next_player_id: u16,
    pub(crate) next_repel_delivery_generation: u32,
    /// Arena-stream timeline generations skip zero and are never selected by
    /// the browser. Exact session+arena scope prevents wrapped numeric alias.
    pub(crate) next_observer_timeline_generation: u32,
    /// Last internal transport incarnation allocated by this server state.
    /// Allocation uses checked addition, so an id is never silently reused.
    pub(crate) next_player_session_id: u64,
    /// Last arena-stream generation allocated by this server state. Checked
    /// allocation fails the join instead of wrapping to a reusable identity.
    pub(crate) next_arena_stream_id: u64,
    /// Player ids temporarily withheld from allocation while disconnect-side
    /// cleanup runs after removing the old client handle.
    pub(crate) disconnecting_player_ids: BTreeMap<u16, PlayerSessionId>,
    /// Highest life generation allocated for each reusable Continuum player
    /// id. Entries survive disconnect to avoid collisions within the bounded
    /// lifetime of deferred gameplay authority work. `life_seq` wraps from
    /// `u32::MAX` to 1; the never-reused session id independently closes the
    /// reconnect/id-reuse boundary. The map is bounded by the 768-id range.
    pub(crate) life_seq_watermarks: HashMap<u16, u32>,
    pub(crate) next_brick_id: u16,
    pub(crate) objectives: crate::objectives::ObjectiveRegistry,
    pub(crate) turret_links: HashMap<u16, u16>,
    pub(crate) active_bricks: Vec<ServerBrick>,
    pub(crate) active_projectiles: ProjectileRegistry,
    /// Server-owned Decoy motion exists for lifecycle/Repel authority even
    /// though ordinary Decoy rendering remains an authenticated client
    /// presentation fact and cannot damage players.
    pub(crate) active_decoys: Vec<ServerDecoy>,
    /// Coalesced silent safety notifications, at most one per live source session.
    /// Physical cancellation is immediate; delivery may wait for bounded queues.
    pub(crate) pending_owner_weapon_cancellations:
        BTreeMap<PlayerSessionIdentity, crate::realtime_event::RealtimeOwnerWeaponCancellation>,
    /// Persist the cut through later commits, so a previously frozen blast
    /// cannot resurrect older-group fragment flight after the owner leaves safety.
    pub(crate) owner_weapon_cancellation_watermarks: BTreeMap<PlayerSessionIdentity, u64>,
    /// Latest accepted launch per source session; avoids rescanning flight on idle safe positions.
    pub(crate) latest_owner_weapon_groups: BTreeMap<PlayerSessionIdentity, u64>,
    /// Fully frozen collision-time damage/splash/AOI inputs keyed by the
    /// authoritative projectile id. Queue pressure may delay commit, but may
    /// never cause movement/history/lifecycle state to recompute the outcome.
    pub(crate) pending_damage_transactions:
        BTreeMap<u32, crate::server_damage::PreparedDamageTransaction>,
    pub(crate) pending_wall_explosion_transactions:
        BTreeMap<u32, crate::server_explosions::PreparedProjectileExplosion>,
    /// Damage deaths retire live projectile authority immediately, but a
    /// collision already frozen in that exact server tick remains a valid
    /// transaction. The runtime+tick token is never accepted for later
    /// projectile movement, packets, ship/frequency changes, or reconnects.
    pub(crate) retired_projectile_owners: BTreeMap<PlayerRuntimeIdentity, u32>,
    pub(crate) active_hidden_prizes: Vec<ServerHiddenPrize>,
    pub(crate) hidden_prize_rng_seed: i32,
    pub(crate) death_prize_rng_seed: i32,
    pub(crate) damage_prize_rng_seed: i32,
    /// Last server-issued collectible identity. Allocation is checked and
    /// never wraps to a reusable identity; exhaustion simply stops spawning
    /// new greens for the lifetime of this server state.
    pub(crate) next_collectible_id: u32,
    /// Server-owned monotonic event id for authoritative damage. This is not
    /// derived from any client timestamp or packet counter.
    pub(crate) next_damage_seq: u32,
    /// Server-owned monotonic id for authoritative projectile records.
    /// Lets traces and tests correlate spawn, hit, splash, and expiry
    /// without trusting client timestamps or "last damage" fields.
    pub(crate) next_projectile_id: u32,
    /// Checked monotonic id for all projectile children from one accepted
    /// packet. Unlike the wire-facing projectile id, this never wraps/reuses.
    pub(crate) next_projectile_group_id: u64,
    /// Phase 1 diagnostic counters. These describe immutable collision
    /// witnesses only and never participate in admission or damage authority.
    pub(crate) collision_witnesses_total: u64,
    /// Bounded debug/QA evidence only. No gameplay path reads this registry.
    pub(crate) qa_traces: crate::qa_trace::QaTraceRegistry,
    pub(crate) next_hidden_prize_spawn_at_tick: Option<u32>,
    pub(crate) claimed_prizes: HashMap<u32, ServerPrizeReceipt>,
    pub(crate) identity_stats: HashMap<String, PlayerStats>,
    pub(crate) visibility_index: VisibilityIndex,
    pub(crate) pending_position_inputs: PositionInputQueues,
    /// Transient handoff from position/weapon authority to the tick
    /// scheduler. Queue pressure retains the exact runtime/life/sequence
    /// packet for automatic retry instead of relying on another browser send.
    pub(crate) weapon_admission_deferred: BTreeSet<LifeIdentity>,
    /// One exact reliable gameplay FIFO per runtime. Objective, lifecycle,
    /// Brick, prize, and correction mutations share aggregate capacity and a
    /// fair ready-runtime index, so no feature queue can overtake another or
    /// temporarily free an in-flight head's slot.
    pub(crate) pending_reliable_gameplay: ReliableGameplaySequencer,
    /// Application-reliable Portal requests wait behind the unordered
    /// position drain so their semantic edge can bind to the exact canonical
    /// post-movement witness named by the request.
    pub(crate) pending_portal_placements: VecDeque<PendingPortalPlacement>,
    /// Application-reliable manual Warp requests consume the same action-id
    /// window but retain an independent bounded drain/retry queue.
    pub(crate) pending_portal_warps: VecDeque<PendingPortalWarp>,
    /// First server tick at which this life's newest queued position was
    /// deferred waiting for its redundant input bundle. One entry per active
    /// life keeps omission recovery bounded by player capacity.
    pub(crate) input_recovery_deferred_at: HashMap<LifeIdentity, u32>,
    pub(crate) clients: HashMap<u16, ClientHandle>,
    /// RTC admission is reserved before the expensive ICE/DTLS handshake and
    /// released only when the peer closes. This bounds half-open offers as
    /// well as registered gameplay clients to `max_players`.
    pub(crate) rtc_sessions: usize,
    /// Fixed UDP ports currently owned by live or handshaking peers. Keeping
    /// this separate from OS bind errors lets admission distinguish expected
    /// active sockets from externally stolen/unhealthy configured ports.
    pub(crate) rtc_udp_ports_in_use: BTreeSet<u16>,
    /// Configured fixed UDP ports that failed bounded teardown/reuse probing or
    /// were found externally occupied. Quarantined ports reduce effective RTC
    /// capacity until a bounded-cycle background recovery proves them bindable.
    pub(crate) rtc_udp_quarantined_ports: BTreeSet<u16>,
    /// Quarantined ports with an active background recovery owner. The map
    /// deduplicates workers per port and lets an admission attempt wake a
    /// worker from backoff for an opportunistic probe. Entries are cleared
    /// when a worker recovers, exits, or is cancelled during runtime shutdown.
    pub(crate) rtc_udp_recovery_ports: BTreeMap<u16, Arc<tokio::sync::Notify>>,
    /// Lock-free counters shared by RTC writer queues. Enqueue paths often run
    /// while `ServerState` is already locked, so atomics avoid recursive locks
    /// while still exposing current/peak overload state through `/status`.
    pub(crate) rtc_overload_metrics: Arc<RtcOverloadMetrics>,
    /// Fixed-window per-client signaling limiter. Entries are opportunistically
    /// pruned as offers arrive, so unauthenticated source addresses cannot make
    /// this map grow without bound over the lifetime of the process.
    pub(crate) signaling_rate_windows: HashMap<String, SignalingRateWindow>,
    pub(crate) bots: Vec<ServerBot>,
    /// Phase 2 polish (P1): reusable scratch buffer for the per-tick
    /// hull snapshot the projectile step consumes. `server_tick`
    /// overwrites entries in place so the inner exact motion-sample Vec keeps
    /// its capacity across ticks; only growth allocates.
    /// Eliminates the ~4.4 MB/s allocator churn projected at 200
    /// players × 60 Hz the old `.collect()` path produced.
    pub(crate) hull_snapshots_scratch: Vec<crate::authority_projectiles::PlayerHullSnapshot>,
    /// Arena-wide serializable door-cycle anchor sent to every client. It is
    /// intentionally kept separate from the periodically rebased simulation
    /// state below: rebasing is an internal bounded-work optimization, while
    /// every browser must derive the same visible phase from one stable
    /// security challenge.
    pub(crate) server_door_challenge_state: ClientDoorState,
    /// Periodically rebased representation of the same arena-wide door cycle
    /// used by authoritative projectile, bot, objective, and placement
    /// collision.
    pub(crate) server_door_state: ClientDoorState,
    /// Cached phase retained for the periodic bounded-work rebase of the
    /// authoritative door state. Per-client input validation keeps its own
    /// equivalent cache.
    pub(crate) server_door_seed_cache: Option<DoorSeedCache>,
    /// Phase 4 load-shedding: EWMA of `run_server_tick` wallclock
    /// in microseconds, updated at the end of each tick. The next
    /// tick's snapshot emit reads `load_pressure_ratio()` to decide
    /// whether to degrade — it skips MID-tier entities when pressure
    /// ≥ 0.60. Whole-frame shedding additionally requires sustained actual
    /// overruns; a transient high EWMA cannot suppress later healthy
    /// snapshots. Zero until the first tick completes.
    pub(crate) tick_wallclock_ewma_us: f64,
    /// Consecutive completed ticks whose measured work met or exceeded the
    /// configured tick budget. Unlike the EWMA, this resets immediately after
    /// a healthy tick and therefore distinguishes sustained overload from one
    /// transient scheduling hitch.
    pub(crate) tick_wallclock_overrun_streak: u16,
    /// Phase 4 load-shedding: rolling counts since the last periodic
    /// summary drain. Aggregated by `run_server_tick` alongside the
    /// security-reject summary so a sustained shed pressure shows
    /// up in the operator log without per-tick chatter.
    pub(crate) load_shed_mid_skipped: u64,
    pub(crate) load_shed_frame_skipped: u64,
    /// Cumulative successful sustained-drift corrections. This is exposed by
    /// `/status` so security evidence can distinguish a clean zero from a
    /// missing log window.
    pub(crate) movement_snapbacks_total: u64,
    /// Corrections since the last security/pacing summary interval.
    pub(crate) movement_snapbacks_since_summary: u64,
    /// Hostile Repel events rejected because a target's immutable queue was
    /// already full. This must stay zero in clean E2E/status evidence.
    pub(crate) repel_queue_overflows: u64,
    /// Maximum aggregate number of committed dedicated 0x7d recipient
    /// envelopes retained at once. Current depth is derived from client queues
    /// under the state lock; the remaining counters are cumulative pressure
    /// evidence for `/status` and retained browser artifacts.
    pub(crate) realtime_event_queue_peak: usize,
    pub(crate) realtime_event_queue_overflows: u64,
    pub(crate) realtime_event_actions_rejected: u64,
    pub(crate) realtime_event_actions_deferred: u64,
    /// Phase D (AOI pacing): EWMA of how far each tick's actual interval
    /// drifts from the target tick interval, in microseconds. Captures
    /// snapshot-emission cadence jitter — a value near 0 means frames go
    /// out evenly; a large value means the tick loop is pacing unevenly,
    /// which renders as remote-ship stutter. Surfaced in the periodic
    /// summary so the effect of the deadline-precise driver sleep is
    /// observable. Zero until the second tick. Blended in `advance_arena_tick`
    /// from the existing `last_tick_at`.
    pub(crate) tick_interval_jitter_ewma_us: f64,
    pub(crate) tick_intervals: timing::TimingHistogram,
    pub(crate) tick_jitter: timing::TimingHistogram,
    pub(crate) tick_work: timing::TimingHistogram,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BrickSourceGeometry {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) direction: u8,
    pub(crate) ship: u8,
    pub(crate) frequency: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrickSourceBinding {
    Pending,
    Bound(BrickSourceGeometry),
    Unavailable,
}

pub(crate) const RELIABLE_FREQUENCY_CHANGE_PACKET_LEN: usize = 5
    + crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET
    + crate::continuum_position::CLIENT_POSITION_SINGLE_INPUT_RECORD_LEN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReliableFrequencyChange {
    pub(crate) packet: [u8; RELIABLE_FREQUENCY_CHANGE_PACKET_LEN],
}

impl ReliableFrequencyChange {
    pub(crate) fn parse(payload: &[u8]) -> Option<Self> {
        let packet: [u8; RELIABLE_FREQUENCY_CHANGE_PACKET_LEN] = payload.try_into().ok()?;
        if packet[0] != crate::arena::FREQUENCY_CHANGE_REQUEST {
            return None;
        }
        let frequency = u16::from_le_bytes([packet[1], packet[2]]);
        let position_sequence = u16::from_le_bytes([packet[3], packet[4]]);
        let baseline = &packet[5..];
        let snapshot = crate::continuum_position::parse_client_position_snapshot(baseline)?;
        let input = crate::continuum_position::embedded_client_input_record(baseline)?;
        let receipt = crate::combat_timeline::CombatViewReceiptV1::decode(
            input.get(8..crate::continuum_position::CLIENT_POSITION_SINGLE_INPUT_RECORD_LEN)?,
        )
        .ok()?;
        (frequency < crate::arena_settings::arena_max_frequency()
            && snapshot.client_sequence == Some(position_sequence)
            && snapshot.timestamp == 0
            && snapshot.weapon == 0
            && snapshot.repel_snapshot_id.is_none()
            && snapshot.warp_collectible_id.is_none()
            && input.len() == crate::continuum_position::CLIENT_POSITION_SINGLE_INPUT_RECORD_LEN
            && input[0] == crate::connection::CLIENT_INPUT_RECORD_TYPE
            && u16::from_le_bytes([input[1], input[2]]) == position_sequence
            && input[3] == 1
            && u16::from_le_bytes([input[4], input[5]]) & (1 << 5) == 0
            && input[6] == snapshot.direction
            && receipt.input_sequence == position_sequence)
            .then_some(Self { packet })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReliableShipChange {
    pub(crate) ship: u8,
    pub(crate) position_sequence_base: u16,
}

impl ReliableShipChange {
    pub(crate) fn parse(payload: &[u8]) -> Option<Self> {
        (payload.len() == 4 && payload[0] == crate::arena::SHIP_CHANGE_REQUEST).then(|| Self {
            ship: payload[1],
            position_sequence_base: u16::from_le_bytes([payload[2], payload[3]]),
        })
    }

    pub(crate) fn encode(self) -> [u8; 4] {
        let sequence = self.position_sequence_base.to_le_bytes();
        [
            crate::arena::SHIP_CHANGE_REQUEST,
            self.ship,
            sequence[0],
            sequence[1],
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReliableAttachRequest {
    pub(crate) destination_id: u16,
}

impl ReliableAttachRequest {
    pub(crate) fn parse(payload: &[u8]) -> Option<Self> {
        (payload.len() == 3 && payload[0] == crate::objectives::ATTACH_REQUEST).then(|| Self {
            destination_id: u16::from_le_bytes([payload[1], payload[2]]),
        })
    }

    pub(crate) fn encode(self) -> [u8; 3] {
        let destination = self.destination_id.to_le_bytes();
        [
            crate::objectives::ATTACH_REQUEST,
            destination[0],
            destination[1],
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReliableBrickRequest {
    pub(crate) x: u16,
    pub(crate) y: u16,
}

impl ReliableBrickRequest {
    pub(crate) fn parse(payload: &[u8]) -> Option<Self> {
        (payload.len() == 5 && payload[0] == crate::objectives::DROP_BRICK_REQUEST).then(|| Self {
            x: u16::from_le_bytes([payload[1], payload[2]]),
            y: u16::from_le_bytes([payload[3], payload[4]]),
        })
    }

    pub(crate) fn encode(self) -> [u8; 5] {
        let x = self.x.to_le_bytes();
        let y = self.y.to_le_bytes();
        [
            crate::objectives::DROP_BRICK_REQUEST,
            x[0],
            x[1],
            y[0],
            y[1],
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReliablePrizeRequest {
    pub(crate) collectible_id: u32,
}

impl ReliablePrizeRequest {
    pub(crate) fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() != 11 || payload[0] != crate::player_events::TAKE_PRIZE_REQUEST {
            return None;
        }
        let collectible_id = u32::from_le_bytes(payload[1..5].try_into().ok()?);
        (collectible_id != 0).then_some(Self { collectible_id })
    }

    pub(crate) fn encode(self) -> [u8; 11] {
        let mut packet = [0; 11];
        packet[0] = crate::player_events::TAKE_PRIZE_REQUEST;
        packet[1..5].copy_from_slice(&self.collectible_id.to_le_bytes());
        packet
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingReliableGameplayMutation {
    pub(crate) identity: PlayerRuntimeIdentity,
    pub(crate) reliable_delivery_id: u32,
    pub(crate) kind: PendingReliableGameplayMutationKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingReliableGameplayMutationKind {
    Objective(PendingObjectiveAction),
    Frequency(ReliableFrequencyChange),
    Ship(ReliableShipChange),
    Leave,
    Attach(ReliableAttachRequest),
    DropAttached,
    Brick {
        request: ReliableBrickRequest,
        source: BrickSourceBinding,
    },
    Prize(ReliablePrizeRequest),
    MovementCorrection(crate::movement_correction::MovementCorrectionApplied),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReliableLifecycleMutationOutcome {
    Applied,
    TerminalNoop,
    Deferred,
}

pub(crate) const MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME: usize = 64;
pub(crate) const MAX_PENDING_RELIABLE_GAMEPLAY_TOTAL: usize =
    MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME * 768;
/// Defense-in-depth inline-item accounting bound for the unified sequencer.
/// Every mutation is compact/fixed-size and no attacker-owned packet buffer
/// survives admission. Allocator and `VecDeque`/map/set node overhead is not
/// included; separately enforced entry/container caps are the primary bound.
pub(crate) const MAX_PENDING_RELIABLE_GAMEPLAY_BYTES: usize =
    MAX_PENDING_RELIABLE_GAMEPLAY_TOTAL * std::mem::size_of::<PendingReliableGameplayMutation>();

#[derive(Clone, Debug, Default)]
struct ReliableRuntimeGameplayQueue {
    items: VecDeque<PendingReliableGameplayMutation>,
    delivery_ids: BTreeSet<u32>,
    /// Prefix already inspected at a post-position source boundary. This
    /// makes immutable Brick binding proportional to newly admitted items,
    /// not total retained suffix depth on every tick.
    brick_capture_prefix_len: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ReliableGameplaySequencer {
    queues: BTreeMap<PlayerRuntimeIdentity, ReliableRuntimeGameplayQueue>,
    ready_runtimes: VecDeque<PlayerRuntimeIdentity>,
    ready_members: BTreeSet<PlayerRuntimeIdentity>,
    total_len: usize,
    head_visits: u64,
    brick_capture_visits: u64,
}

impl ReliableGameplaySequencer {
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.total_len
    }

    /// Resident reliable gameplay mutations across every runtime and kind.
    /// Production-visible so `/status` can show a claim that never resolved;
    /// without it a vanished objective action leaves nothing to observe.
    pub(crate) fn queued_len(&self) -> usize {
        self.total_len
    }

    /// Count of queue heads offered to the tick processor. Pair it with
    /// `queued_len`: residency that stays put while visits stall means the fair
    /// rotation lost that runtime, not that the action failed its checks.
    pub(crate) fn head_visits(&self) -> u64 {
        self.head_visits
    }

    /// Runtimes the fair rotation still knows about. Pair with `queued_len` and
    /// `head_visits`: queued items with an empty ready deque means a head went
    /// un-offered, which is a different failure from an action that keeps
    /// failing its own checks.
    pub(crate) fn ready_len(&self) -> usize {
        self.ready_members.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.total_len == 0
    }

    pub(crate) fn runtime_len(&self, identity: PlayerRuntimeIdentity) -> usize {
        self.queues
            .get(&identity)
            .map_or(0, |queue| queue.items.len())
    }

    pub(crate) fn enqueue(&mut self, pending: PendingReliableGameplayMutation) -> bool {
        if self
            .queues
            .get(&pending.identity)
            .is_some_and(|queue| queue.delivery_ids.contains(&pending.reliable_delivery_id))
        {
            return true;
        }
        if self.total_len >= MAX_PENDING_RELIABLE_GAMEPLAY_TOTAL {
            return false;
        }
        let identity = pending.identity;
        let queue = self.queues.entry(identity).or_default();
        if queue.items.len() >= MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME {
            return false;
        }
        queue.delivery_ids.insert(pending.reliable_delivery_id);
        queue.items.push_back(pending);
        self.total_len += 1;
        debug_assert!(
            self.total_len * std::mem::size_of::<PendingReliableGameplayMutation>()
                <= MAX_PENDING_RELIABLE_GAMEPLAY_BYTES
        );
        if self.ready_members.insert(identity) {
            self.ready_runtimes.push_back(identity);
        }
        true
    }

    /// Returns exactly one resident head per active runtime and rotates the
    /// ready index, providing deterministic round-robin fairness without
    /// scanning any retained suffix.
    #[cfg(test)]
    pub(crate) fn fair_heads(&mut self) -> Vec<PendingReliableGameplayMutation> {
        let identities = self.fair_runtime_identities();
        let heads = identities
            .into_iter()
            .filter_map(|identity| self.runtime_head(identity))
            .collect::<Vec<_>>();
        heads
    }

    #[cfg(test)]
    pub(crate) fn fair_runtime_identities(&mut self) -> Vec<PlayerRuntimeIdentity> {
        self.fair_runtime_identities_with_budget(usize::MAX)
    }

    pub(crate) fn fair_runtime_identities_with_budget(
        &mut self,
        budget: usize,
    ) -> Vec<PlayerRuntimeIdentity> {
        let visit_count = self.ready_runtimes.len().min(budget);
        let mut identities = Vec::with_capacity(visit_count);
        for _ in 0..visit_count {
            let identity = self
                .ready_runtimes
                .pop_front()
                .expect("ready length was captured");
            self.ready_members.remove(&identity);
            let has_head = self
                .queues
                .get(&identity)
                .is_some_and(|queue| !queue.items.is_empty());
            if !has_head {
                continue;
            }
            identities.push(identity);
            self.ready_members.insert(identity);
            self.ready_runtimes.push_back(identity);
        }
        self.head_visits = self.head_visits.saturating_add(identities.len() as u64);
        identities
    }

    pub(crate) fn runtime_head(
        &self,
        identity: PlayerRuntimeIdentity,
    ) -> Option<PendingReliableGameplayMutation> {
        self.queues
            .get(&identity)
            .and_then(|queue| queue.items.front())
            .copied()
    }

    pub(crate) fn pop_terminal(&mut self, terminal: &PendingReliableGameplayMutation) -> bool {
        let Some(queue) = self.queues.get_mut(&terminal.identity) else {
            return false;
        };
        if queue.items.front() != Some(terminal) {
            return false;
        }
        let removed = queue.items.pop_front().expect("head was present");
        queue.delivery_ids.remove(&removed.reliable_delivery_id);
        queue.brick_capture_prefix_len = queue.brick_capture_prefix_len.saturating_sub(1);
        self.total_len -= 1;
        if queue.items.is_empty() {
            self.queues.remove(&terminal.identity);
        }
        true
    }

    /// Drops every queued mutation for the runtimes `retain` rejects and returns
    /// how many items went with them. A sweep that silently destroys a client's
    /// in-flight objective action leaves nothing to diagnose - the field case
    /// produced one `objective action received` line and then permanent silence -
    /// so callers need the count to log the loss.
    pub(crate) fn retain_runtime(
        &mut self,
        mut retain: impl FnMut(PlayerRuntimeIdentity) -> bool,
    ) -> usize {
        let retired = self
            .queues
            .keys()
            .copied()
            .filter(|identity| !retain(*identity))
            .collect::<Vec<_>>();
        let mut dropped = 0;
        for identity in retired {
            if let Some(queue) = self.queues.remove(&identity) {
                self.total_len -= queue.items.len();
                dropped += queue.items.len();
            }
            self.ready_members.remove(&identity);
        }
        self.ready_runtimes
            .retain(|identity| self.ready_members.contains(identity));
        dropped
    }

    pub(crate) fn runtime_items(
        &self,
        identity: PlayerRuntimeIdentity,
    ) -> impl Iterator<Item = &PendingReliableGameplayMutation> {
        self.queues
            .get(&identity)
            .into_iter()
            .flat_map(|queue| queue.items.iter())
    }

    pub(crate) fn bind_brick_sources(
        &mut self,
        sources: &BTreeMap<PlayerRuntimeIdentity, Option<BrickSourceGeometry>>,
    ) {
        for (&identity, queue) in &mut self.queues {
            let source = sources.get(&identity).copied().flatten();
            Self::bind_revealed_prefix(queue, source, &mut self.brick_capture_visits);
        }
    }

    pub(crate) fn bind_brick_source_for_runtime(
        &mut self,
        identity: PlayerRuntimeIdentity,
        source: Option<BrickSourceGeometry>,
    ) {
        let Some(queue) = self.queues.get_mut(&identity) else {
            return;
        };
        Self::bind_revealed_prefix(queue, source, &mut self.brick_capture_visits);
    }

    fn bind_revealed_prefix(
        queue: &mut ReliableRuntimeGameplayQueue,
        source: Option<BrickSourceGeometry>,
        visits: &mut u64,
    ) {
        while queue.brick_capture_prefix_len < queue.items.len() {
            let index = queue.brick_capture_prefix_len;
            let pending = queue
                .items
                .get_mut(index)
                .expect("capture prefix stays within queue");
            *visits = visits.saturating_add(1);
            match &mut pending.kind {
                PendingReliableGameplayMutationKind::Frequency(_)
                | PendingReliableGameplayMutationKind::Ship(_)
                | PendingReliableGameplayMutationKind::Leave
                | PendingReliableGameplayMutationKind::MovementCorrection(_) => {
                    break;
                }
                PendingReliableGameplayMutationKind::Brick {
                    source: binding, ..
                } => {
                    debug_assert_eq!(*binding, BrickSourceBinding::Pending);
                    if let Some(source) = source {
                        *binding = BrickSourceBinding::Bound(source);
                    } else {
                        *binding = BrickSourceBinding::Unavailable;
                    }
                }
                _ => {}
            }
            queue.brick_capture_prefix_len += 1;
        }
    }

    #[cfg(test)]
    pub(crate) fn operation_counts(&self) -> (u64, u64) {
        (self.head_visits, self.brick_capture_visits)
    }

    #[cfg(test)]
    pub(crate) fn container_counts(&self) -> (usize, usize, usize, usize) {
        (
            self.queues.len(),
            self.ready_runtimes.len(),
            self.ready_members.len(),
            self.queues
                .values()
                .map(|queue| queue.delivery_ids.len())
                .sum(),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingPortalPlacement {
    pub(crate) identity: PlayerRuntimeIdentity,
    pub(crate) request: crate::portal_placement::PortalPlacementRequest,
    pub(crate) queued_at_tick: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ServerPortal {
    pub(crate) position_x: f32,
    pub(crate) position_y: f32,
    pub(crate) placed_at_tick: u32,
    pub(crate) expires_at_tick: u32,
    pub(crate) placement_action_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ServerDecoy {
    pub(crate) presentation_id: u32,
    pub(crate) presentation_source: Option<crate::realtime_event::RealtimeWeaponEvent>,
    pub(crate) source_input_sequence: Option<u16>,
    pub(crate) owner: PlayerRuntimeIdentity,
    pub(crate) source_weapon_data: u16,
    pub(crate) frequency: u16,
    pub(crate) ship: u8,
    pub(crate) position_x: f32,
    pub(crate) position_y: f32,
    pub(crate) velocity_x: f32,
    pub(crate) velocity_y: f32,
    pub(crate) orientation: f32,
    pub(crate) created_tick: u32,
    pub(crate) last_step_tick: u32,
    pub(crate) expires_at_tick: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingPortalWarp {
    pub(crate) identity: PlayerRuntimeIdentity,
    pub(crate) request: crate::portal_warp::PortalWarpRequest,
    pub(crate) queued_at_tick: u32,
}

pub(crate) const MAX_PENDING_PORTAL_PLACEMENTS_PER_RUNTIME: usize = 64;
pub(crate) const MAX_PENDING_PORTAL_PLACEMENTS_TOTAL: usize =
    MAX_PENDING_PORTAL_PLACEMENTS_PER_RUNTIME * 768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingObjectiveAction {
    pub(crate) identity: PlayerRuntimeIdentity,
    pub(crate) reliable_delivery_id: u32,
    pub(crate) request: crate::objective_protocol::ObjectiveActionRequest,
    pub(crate) queued_at_tick: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ServerMovementCorrection {
    pub(crate) proposal: crate::movement_correction::MovementCorrection,
    /// Exact reported prediction lineage at detection, independently proved
    /// where required. It is not admitted world state. Authority carries the
    /// proposal's canonical lineage to the later application tail.
    pub(crate) detected_state: ServerSimMovement,
    /// Independently replayed corrected lineage. Each accepted old-lineage
    /// position advances this witness through the same authenticated inputs
    /// and live collision environment. Application may commit only this
    /// exact witness; a missing replay cancels the transaction.
    pub(crate) replay_witness: Option<CanonicalPositionWitness>,
    /// Server tick paired with `replay_witness`, used to resolve time-bound
    /// movement environments during the next exact replay interval.
    pub(crate) replay_server_tick: u32,
    /// Exactly proved unreceipted Repel prediction, used only to recognize
    /// overtaken in-flight packets. Canonical replay never uses this window.
    pub(crate) predicted_repel_window: Option<(u16, u16)>,
    pub(crate) offer_reliable_id: Option<u32>,
    pub(crate) application_sequence: Option<u16>,
    /// Exact current-tail state serialized into an applied terminal.
    pub(crate) terminal_state: Option<ServerSimMovement>,
    pub(crate) terminal_server_tick: Option<u32>,
    pub(crate) cancelled: bool,
    pub(crate) receipt_reliable_id: Option<u32>,
}

pub(crate) const MAX_RETIRED_MOVEMENT_CORRECTIONS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RetiredMovementCorrection {
    pub(crate) server_session_id: u64,
    pub(crate) life_seq: u32,
    pub(crate) correction_generation: u32,
    pub(crate) detected_sequence: u16,
}

impl RetiredMovementCorrection {
    pub(crate) fn from_transaction(transaction: &ServerMovementCorrection) -> Self {
        Self {
            server_session_id: transaction.proposal.server_session_id,
            life_seq: transaction.proposal.life_seq,
            correction_generation: transaction.proposal.correction_generation,
            detected_sequence: transaction.proposal.position_sequence,
        }
    }

    pub(crate) fn matches(
        self,
        application: crate::movement_correction::MovementCorrectionApplied,
    ) -> bool {
        self.server_session_id == application.server_session_id
            && self.life_seq == application.life_seq
            && self.correction_generation == application.correction_generation
            && self.detected_sequence == application.detected_sequence
    }
}

impl ServerMovementCorrection {
    pub(crate) fn awaiting_application(&self) -> bool {
        self.application_sequence.is_none() && !self.cancelled
    }

    pub(crate) fn cancel_before_application(&mut self) {
        if self.application_sequence.is_none() {
            self.cancelled = true;
        }
    }

    pub(crate) fn cancel_at_application_sequence(&mut self, application_sequence: u16) {
        if self.application_sequence.is_none() {
            self.application_sequence = Some(application_sequence);
            self.cancelled = true;
        }
    }

    pub(crate) fn application_matches(
        &self,
        application: crate::movement_correction::MovementCorrectionApplied,
    ) -> bool {
        let proposal = self.proposal;
        proposal.server_session_id == application.server_session_id
            && proposal.player_id == application.player_id
            && proposal.life_seq == application.life_seq
            && proposal.correction_generation == application.correction_generation
            && proposal.position_sequence == application.detected_sequence
    }

    pub(crate) fn receipt_matches(
        &self,
        receipt: crate::movement_correction::MovementCorrectionAccepted,
    ) -> bool {
        self.terminal_receipt() == Some(receipt)
    }

    pub(crate) fn terminal_receipt(
        &self,
    ) -> Option<crate::movement_correction::MovementCorrectionAccepted> {
        let (outcome, state) = if self.cancelled {
            (
                crate::movement_correction::MovementCorrectionAccepted::OUTCOME_CANCELLED,
                self.detected_state,
            )
        } else if self.application_sequence.is_some() {
            (
                crate::movement_correction::MovementCorrectionAccepted::OUTCOME_APPLIED,
                self.terminal_state?,
            )
        } else {
            return None;
        };
        Some(crate::movement_correction::MovementCorrectionAccepted {
            server_tick: if self.cancelled {
                0 // Cancellation does not install a movement/door boundary.
            } else {
                self.terminal_server_tick?
            },
            outcome,
            server_session_id: self.proposal.server_session_id,
            player_id: self.proposal.player_id,
            life_seq: self.proposal.life_seq,
            correction_generation: self.proposal.correction_generation,
            detected_sequence: self.proposal.position_sequence,
            application_sequence: self
                .application_sequence
                .unwrap_or(self.proposal.position_sequence),
            position_x: state.position_x,
            position_y: state.position_y,
            velocity_x: state.velocity_x,
            velocity_y: state.velocity_y,
            orientation: state.orientation,
        })
    }
}

impl ServerState {
    pub(crate) fn new(config: ServerConfig) -> Self {
        let simulated_tx_loss = config.simulated_tx_loss.controller();
        let objective_map = super::zone::zone_map_for_id(&config.default_zone);
        // Flags and powerballs are deferred for this product iteration: the
        // authored `[Objective]` profile is only activated behind the explicit
        // operator opt-in, so a shipped or dynamic zone cannot turn them on.
        let objective_profile = crate::objectives::gated_objective_profile(
            config.enable_objectives,
            objective_map.map(|map| &map.objective_profile),
        );
        let mut state = Self {
            config,
            simulated_tx_loss,
            qa_traces: crate::qa_trace::QaTraceRegistry::with_process_secret(),
            objectives: crate::objectives::ObjectiveRegistry::new(objective_profile),
            ..Self::default()
        };
        if let Some(map) = objective_map {
            if let Err(error) = crate::objectives::initialize_objectives(
                &mut state.objectives,
                0,
                |x, y| {
                    let index = usize::from(y) * super::zone::MAP_TILES + usize::from(x);
                    map.tiles.get(index).copied().is_none_or(|tile| tile != 0)
                },
                |x, y| {
                    let index = usize::from(y) * super::zone::MAP_TILES + usize::from(x);
                    crate::objectives::FlagDropTile::from_map_tile(
                        map.tiles.get(index).copied().unwrap_or(20),
                        map.no_flag_drops_at(x, y),
                    )
                },
            ) {
                tracing::error!(
                    zone = %state.config.default_zone,
                    %error,
                    "objective initialization failed; objectives disabled fail-closed"
                );
                state.objectives = crate::objectives::ObjectiveRegistry::default();
            }
        }
        state
    }

    pub(crate) fn new_with_zone_catalog(config: ServerConfig, zone_catalog_json: String) -> Self {
        let mut state = Self::new(config);
        state.zone_catalog_json = Some(Arc::from(zone_catalog_json));
        state
    }

    pub(crate) fn rtc_effective_capacity(&self) -> usize {
        let usable_socket_capacity = self
            .config
            .rtc_udp_port_range
            .map(|range| {
                range
                    .capacity()
                    .saturating_sub(self.rtc_udp_quarantined_ports.len())
            })
            .unwrap_or(self.config.max_players);
        self.config.max_players.min(usable_socket_capacity)
    }

    pub(crate) fn allocate_client_id(&mut self) -> u16 {
        let mut id = self.next_player_id;
        for _ in 0..SERVER_BOT_ID_BASE {
            if id >= SERVER_BOT_ID_BASE {
                id = 0;
            }
            if !self.clients.contains_key(&id) && !self.disconnecting_player_ids.contains_key(&id) {
                self.next_player_id = (id + 1).min(SERVER_BOT_ID_BASE);
                return id;
            }
            id = id.wrapping_add(1);
        }
        panic!("Continuum player id space exhausted")
    }

    pub(crate) fn allocate_collectible_id(&mut self) -> Option<u32> {
        let next = self.next_collectible_id.checked_add(1)?;
        if next == 0 {
            return None;
        }
        self.next_collectible_id = next;
        Some(next)
    }

    pub(crate) fn allocate_player_session_id(&mut self) -> PlayerSessionId {
        self.next_player_session_id = self
            .next_player_session_id
            .checked_add(1)
            .expect("player session id space exhausted");
        PlayerSessionId::new(self.next_player_session_id)
    }

    pub(crate) fn allocate_arena_stream_id(&mut self) -> Option<u64> {
        let next = self.next_arena_stream_id.checked_add(1)?;
        self.next_arena_stream_id = next;
        Some(next)
    }

    pub(crate) fn allocate_observer_timeline_generation(&mut self) -> u32 {
        self.next_observer_timeline_generation = self
            .next_observer_timeline_generation
            .wrapping_add(1)
            .max(1);
        self.next_observer_timeline_generation
    }

    pub(crate) fn realtime_event_queue_current(&self) -> usize {
        self.clients
            .values()
            .map(|client| client.pending_realtime_events.queue.len())
            .fold(0usize, usize::saturating_add)
    }

    pub(crate) fn pending_damage_transaction_payload_bytes(&self) -> usize {
        self.pending_damage_transactions
            .values()
            .fold(0usize, |total, transaction| {
                total.saturating_add(transaction.retained_payload_bytes())
            })
    }

    pub(crate) fn active_player_session_identity(
        &self,
        player_id: u16,
    ) -> Option<PlayerSessionIdentity> {
        self.clients
            .get(&player_id)
            .map(|client| PlayerSessionIdentity::new(player_id, client.session_id))
    }

    pub(crate) fn player_session_matches(&self, identity: PlayerSessionIdentity) -> bool {
        self.active_player_session_identity(identity.player_id) == Some(identity)
    }

    pub(crate) fn active_player_runtime_identity(
        &self,
        player_id: u16,
    ) -> Option<PlayerRuntimeIdentity> {
        let client = self.clients.get(&player_id)?;
        client.joined.then_some(PlayerRuntimeIdentity::new(
            PlayerSessionIdentity::new(player_id, client.session_id),
            LifeIdentity::new(player_id, client.life_seq),
        ))
    }

    pub(crate) fn player_runtime_matches(&self, identity: PlayerRuntimeIdentity) -> bool {
        self.active_player_runtime_identity(identity.player_id()) == Some(identity)
    }

    pub(crate) fn joined_player_session_identities(&self) -> Vec<PlayerSessionIdentity> {
        self.clients
            .iter()
            .filter_map(|(&player_id, client)| {
                client
                    .joined
                    .then_some(PlayerSessionIdentity::new(player_id, client.session_id))
            })
            .collect()
    }

    pub(crate) fn player_session_ingress_gate(
        &self,
        player_id: u16,
    ) -> Option<Arc<std::sync::Mutex<()>>> {
        self.clients
            .get(&player_id)
            .map(|client| Arc::clone(&client.ingress_gate))
    }

    pub(crate) fn reserve_disconnecting_player_id(&mut self, identity: PlayerSessionIdentity) {
        let previous = self
            .disconnecting_player_ids
            .insert(identity.player_id, identity.session_id);
        assert!(previous.is_none(), "player id already reserved for cleanup");
    }

    pub(crate) fn release_disconnecting_player_id(
        &mut self,
        identity: PlayerSessionIdentity,
    ) -> bool {
        if self.disconnecting_player_ids.get(&identity.player_id) != Some(&identity.session_id) {
            return false;
        }
        self.disconnecting_player_ids.remove(&identity.player_id);
        true
    }

    #[cfg(test)]
    pub(crate) fn advance_client_life(&mut self, player_id: u16) -> Option<LifeIdentity> {
        self.advance_client_life_at_tick(player_id, crate::wire::current_tick())
    }

    pub(crate) fn advance_client_life_at_tick(
        &mut self,
        player_id: u16,
        tick: u32,
    ) -> Option<LifeIdentity> {
        self.advance_client_life_inner(player_id, tick, false)
    }

    pub(crate) fn advance_client_life_after_damage_death(
        &mut self,
        player_id: u16,
        tick: u32,
    ) -> Option<LifeIdentity> {
        self.advance_client_life_inner(player_id, tick, true)
    }

    fn advance_client_life_inner(
        &mut self,
        player_id: u16,
        tick: u32,
        preserve_decoys: bool,
    ) -> Option<LifeIdentity> {
        if let Some(retiring_runtime) = self.active_player_runtime_identity(player_id) {
            // Damage death advances the life before registering the retired
            // runtime. Capture the admitted view before advance clears it;
            // normal lifecycle cancellation/pruning revokes unused contexts.
            if self
                .active_projectiles
                .contains_flight_of_life(retiring_runtime.life)
            {
                let client = self.clients.get_mut(&player_id)?;
                client.retired_projectile_observers.insert(
                    retiring_runtime.life,
                    client
                        .observer_combat_timeline
                        .retained_projectile_context(),
                );
            }
            if !preserve_decoys {
                // A ship/spectator change removes all this session's decoys,
                // including any still flying from a previous life. Death alone
                // preserves their captured source and independent deadline.
                self.active_decoys
                    .retain(|decoy| decoy.owner.session != retiring_runtime.session);
            }
        }
        let current = self
            .clients
            .get(&player_id)
            .map(|client| client.life_seq)?
            .max(
                self.life_seq_watermarks
                    .get(&player_id)
                    .copied()
                    .unwrap_or(0),
            );
        let mut next = current.wrapping_add(1);
        if next == 0 {
            next = 1;
        }
        self.life_seq_watermarks.insert(player_id, next);
        let client = self.clients.get_mut(&player_id)?;
        client.life_seq = next;
        client.life_started_tick = Some(tick);
        client.last_damage_prize_loss_seq = None;
        client.last_damage_prize_loss_id = None;
        client.clear_life_scoped_observations();
        client.begin_actor_motion_segment();
        let observer = PlayerRuntimeIdentity::new(
            PlayerSessionIdentity::new(player_id, client.session_id),
            LifeIdentity::new(player_id, next),
        );
        if client.retired_projectile_observers.is_empty() {
            client
                .observer_presentation_history
                .begin_scope(observer, client.arena_stream_id);
        } else {
            client
                .observer_presentation_history
                .advance_life(observer, client.arena_stream_id);
        }
        client
            .observer_combat_timeline
            .advance_life(observer)
            .expect("a soft life boundary cannot change observer session");
        Some(LifeIdentity::new(player_id, next))
    }

    pub(crate) fn active_life_identity(&self, player_id: u16) -> Option<LifeIdentity> {
        self.clients.get(&player_id).and_then(|client| {
            client
                .joined
                .then_some(LifeIdentity::new(player_id, client.life_seq))
        })
    }

    pub(crate) fn identity_matches_active_life(&self, identity: LifeIdentity) -> bool {
        self.active_life_identity(identity.player_id) == Some(identity)
    }

    pub(crate) fn zone_map(&self) -> &'static ZoneMap {
        zone_map_for_id(&self.config.default_zone)
            .expect("validated server state must name an exact loaded zone")
    }

    /// Phase 4 load-shedding: ratio of EWMA tick wallclock to the
    /// configured tick budget (1 / tick_rate_hz). 0.0 when the
    /// EWMA hasn't started; 1.0 means the server is spending all of
    /// each tick's wallclock budget; > 1.0 means the loop is
    /// falling behind. The snapshot path reads this to decide whether to shed
    /// MID-tier (≥ 0.60). Whole-frame shedding uses the ≥ 0.80 EWMA
    /// threshold plus `sustained_frame_load_pressure`.
    pub(crate) fn load_pressure_ratio(&self) -> f64 {
        let tick_rate = self.config.tick_rate_hz.max(1) as f64;
        let budget_us = 1_000_000.0 / tick_rate;
        if budget_us <= 0.0 {
            0.0
        } else {
            self.tick_wallclock_ewma_us / budget_us
        }
    }

    pub(crate) fn sustained_frame_load_pressure(&self) -> bool {
        self.tick_wallclock_overrun_streak >= LOAD_SHED_FRAME_OVERRUN_STREAK
    }

    /// Phase 4 load-shedding: blend a new tick wallclock sample
    /// into the EWMA. `alpha = 1/8` matches the RTT smoother — fast
    /// enough to react to a sustained spike, slow enough to ignore
    /// single-tick noise.
    pub(crate) fn record_tick_wallclock(&mut self, sample_us: u64) {
        if self.tick_cadence_sampled {
            self.tick_work.record(sample_us);
            // The tick's cadence sample waits here so the three lifetime populations move in one
            // step: a status read taken while a tick is in flight used to see the boundary sample
            // without the work sample and report `work.samples == interval.samples - 1`.
            if let Some(interval_us) = self.pending_interval_us.take() {
                self.tick_intervals.record(interval_us);
            }
            if let Some(jitter_us) = self.pending_jitter_us.take() {
                self.tick_jitter.record(jitter_us);
            }
        }
        const ALPHA_NUM: f64 = 1.0;
        const ALPHA_DEN: f64 = 8.0;
        let sample = sample_us as f64;
        let budget_us = 1_000_000.0 / self.config.tick_rate_hz.max(1) as f64;
        if sample >= budget_us {
            self.telemetry.tick_overruns = self.telemetry.tick_overruns.saturating_add(1);
            self.tick_wallclock_overrun_streak =
                self.tick_wallclock_overrun_streak.saturating_add(1);
        } else {
            self.tick_wallclock_overrun_streak = 0;
        }
        if self.tick_wallclock_ewma_us == 0.0 {
            self.tick_wallclock_ewma_us = sample;
        } else {
            self.tick_wallclock_ewma_us = ((ALPHA_DEN - ALPHA_NUM) * self.tick_wallclock_ewma_us
                + ALPHA_NUM * sample)
                / ALPHA_DEN;
        }
    }

    /// The one exact door seed for the supplied authoritative gameplay tick.
    /// Previous/current/next candidates are appropriate when authenticating a
    /// delayed client record, but using that grace union for world collision
    /// would make an actually closed door passable whenever either neighbor
    /// phase happened to be open.
    pub(crate) fn server_door_seeds(&mut self, tick: u32) -> Vec<u8> {
        let seeds =
            crate::doors::client_door_simulation_seed_at_tick(Some(self.server_door_state), tick)
                .into_iter()
                .collect();
        crate::doors::rebase_door_state_at_tick(
            &mut self.server_door_state,
            &mut self.server_door_seed_cache,
            tick,
        );
        seeds
    }

    pub(crate) fn advance_arena_tick(&mut self, now: Instant) -> u64 {
        self.arena_tick = self.arena_tick.wrapping_add(1);
        // Phase D: blend this tick's interval-vs-target deviation into the
        // pacing-jitter EWMA before overwriting the timestamp, so the
        // deadline-precise driver's even cadence is observable in the
        // periodic summary. Same alpha = 1/8 as the wallclock smoother.
        self.tick_cadence_sampled = self.last_tick_at.is_some();
        if let Some(last) = self.last_tick_at {
            const ALPHA_NUM: f64 = 1.0;
            const ALPHA_DEN: f64 = 8.0;
            let interval_us = now
                .saturating_duration_since(last)
                .as_micros()
                .min(u64::MAX as u128) as f64;
            let target_us = 1_000_000.0 / self.config.tick_rate_hz.max(1) as f64;
            let deviation = (interval_us - target_us).abs();
            self.pending_interval_us = Some(interval_us as u64);
            // Round the absolute deviation upward to whole microseconds.
            self.pending_jitter_us = Some(deviation.ceil() as u64);
            if self.tick_interval_jitter_ewma_us == 0.0 {
                self.tick_interval_jitter_ewma_us = deviation;
            } else {
                self.tick_interval_jitter_ewma_us = ((ALPHA_DEN - ALPHA_NUM)
                    * self.tick_interval_jitter_ewma_us
                    + ALPHA_NUM * deviation)
                    / ALPHA_DEN;
            }
        }
        self.last_tick_at = Some(now);
        self.arena_tick
    }

    pub(crate) fn allocate_damage_seq(&mut self) -> u32 {
        self.next_damage_seq = self.next_damage_seq.wrapping_add(1);
        if self.next_damage_seq == 0 {
            self.next_damage_seq = 1;
        }
        self.next_damage_seq
    }

    pub(crate) fn allocate_projectile_id(&mut self) -> u32 {
        self.next_projectile_id = self.next_projectile_id.wrapping_add(1);
        if self.next_projectile_id == 0 {
            self.next_projectile_id = 1;
        }
        self.next_projectile_id
    }

    pub(crate) fn allocate_projectile_group_id(&mut self) -> Option<u64> {
        let next = self.next_projectile_group_id.checked_add(1)?;
        self.next_projectile_group_id = next;
        Some(next)
    }

    pub(crate) fn set_client_position(&mut self, player_id: u16, position: Vec<u8>) {
        let new_cell = visibility_cell_for_large_position(&position);
        let objective_last_safe =
            crate::continuum_position::large_position_world_coordinates(&position).is_some_and(
                |(x, y)| super::zone::world_position_in_safe_tile_for_map(self.zone_map(), x, y),
            );
        let Some(client) = self.clients.get_mut(&player_id) else {
            return;
        };
        let old_cell = client.visibility_cell;
        self.visibility_index
            .set_player_cell(player_id, old_cell, new_cell);
        client.visibility_cell = new_cell;
        client.aoi_observer_position = Some(position.clone());
        client.last_position = Some(position);
        client.objective_last_safe = objective_last_safe;
    }

    pub(crate) fn clear_client_position(&mut self, player_id: u16) {
        self.clear_client_position_inner(player_id, false);
    }

    /// Install a read-only AOI anchor at the authored spawn without granting
    /// position, movement, safe-zone, or visibility-index authority.
    ///
    /// Join and live ship-change transitions can precede the client's exact
    /// reliable position baseline by several snapshot frames (especially
    /// while a browser finishes map bootstrap). Keeping this separate from
    /// `set_client_position` lets those frames continue presenting nearby
    /// actors without accepting a synthetic self position.
    pub(crate) fn set_client_spawn_aoi_observer(&mut self, player_id: u16) {
        let (x, y) = crate::zone::server_spawn_coordinates(self.zone_map());
        let mut position = vec![0; 23];
        position[0] = crate::arena::LARGE_POSITION_PACKET;
        position[4..6].copy_from_slice(&x.to_le_bytes());
        position[8..10].copy_from_slice(&player_id.to_le_bytes());
        position[15..17].copy_from_slice(&y.to_le_bytes());
        if let Some(client) = self.clients.get_mut(&player_id) {
            client.aoi_observer_position = Some(position);
        }
    }

    /// Legacy/test lifecycle boundary that clears movement authority while
    /// retaining the read-only death-view AOI anchor. Production
    /// server-detected damage uses the narrower same-tick collision variant
    /// below.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn clear_client_position_preserving_aoi_observer(&mut self, player_id: u16) {
        self.clear_client_position_inner(player_id, true);
    }

    /// Damage-death cutover variant. It clears every movement/input authority
    /// surface but retains an outgoing collision transaction that the server
    /// already froze in this exact tick. Ordinary lifecycle cutovers continue
    /// to use `clear_client_position*` and cancel all old-life work.
    pub(crate) fn clear_client_position_after_damage_death(
        &mut self,
        runtime: PlayerRuntimeIdentity,
        collision_tick: u32,
    ) {
        let player_id = runtime.player_id();
        let Some(client) = self.clients.get_mut(&player_id) else {
            return;
        };
        self.visibility_index
            .remove_player(player_id, client.visibility_cell);
        client.visibility_cell = None;
        client.last_position = None;
        client.last_position_server_tick = None;
        self.clear_pending_position_inputs_after_damage_death(runtime, collision_tick);
        self.retire_projectile_owner_after_damage_death(runtime, collision_tick);
    }

    fn clear_client_position_inner(&mut self, player_id: u16, preserve_aoi_observer: bool) {
        let Some(client) = self.clients.get_mut(&player_id) else {
            return;
        };
        self.visibility_index
            .remove_player(player_id, client.visibility_cell);
        client.visibility_cell = None;
        client.last_position = None;
        if !preserve_aoi_observer {
            client.aoi_observer_position = None;
            client.spectator_view_sequence = None;
            client.spectator_next_input_sequence = None;
        }
        client.last_position_server_tick = None;
        self.clear_pending_position_inputs_for_player(player_id);
    }

    pub(crate) fn remove_client(&mut self, player_id: u16) -> Option<ClientHandle> {
        let client = self.clients.remove(&player_id)?;
        self.telemetry.sessions_ended = self.telemetry.sessions_ended.saturating_add(1);
        // Receiver-side truth for the blindness class of bug. Every counter the investigation had used was published by
        // the sending client, and those numbers are ambiguous by construction: `positions_sent` resets when the client's
        // network object is rebuilt, so a small total can coexist with a high rate. The server knows the one fact that
        // actually gates presentability - whether this session ever handed it a position - so record it at removal.
        // Note that an arena switch clears position state before removal (see the clearing helper above), so this line
        // says what was true at removal, not for the whole session; `arena_stream_id` separates a never-joined socket
        // from a session that was playing and then switched.
        tracing::info!(
            player_id,
            session_id = ?client.session_id,
            arena_stream_id = client.arena_stream_id,
            had_position_at_removal = client.last_position.is_some(),
            position_relay_seen = client.last_position_relay_at.is_some(),
            aoi_cell_registered = client.visibility_cell.is_some(),
            inputs_received = client.inputs_received,
            positions_received = client.positions_received,
            "session_audit"
        );
        crate::session_summary::log(&client);
        let retired_session = PlayerSessionIdentity::new(player_id, client.session_id);
        self.pending_owner_weapon_cancellations
            .remove(&retired_session);
        self.owner_weapon_cancellation_watermarks
            .remove(&retired_session);
        self.latest_owner_weapon_groups.remove(&retired_session);
        crate::observability::retire_session(self, retired_session);
        self.active_decoys
            .retain(|decoy| decoy.owner.session != retired_session);
        self.visibility_index
            .remove_player(player_id, client.visibility_cell);
        self.remove_projectiles_for_player(player_id);
        self.clear_pending_position_inputs_for_player(player_id);
        Some(client)
    }

    pub(crate) fn remove_client_for_session(
        &mut self,
        identity: PlayerSessionIdentity,
    ) -> Option<ClientHandle> {
        self.player_session_matches(identity)
            .then(|| self.remove_client(identity.player_id))
            .flatten()
    }

    #[cfg(test)]
    pub(crate) fn queue_position_input(&mut self, player_id: u16, packet: Vec<u8>) {
        let Some(identity) = self.active_player_runtime_identity(player_id) else {
            return;
        };
        if let Some(snapshot) = crate::continuum_position::parse_client_position_snapshot(&packet)
            && let Some(sequence) = snapshot.client_sequence
        {
            let client = self.clients.get_mut(&player_id).expect("active client");
            let first = client
                .last_input_seq
                .map_or(sequence, |last| last.wrapping_add(1));
            let count = sequence.wrapping_sub(first).saturating_add(1).min(12);
            for offset in 0..count {
                let sequence = first.wrapping_add(offset);
                if client
                    .recent_input_records
                    .iter()
                    .any(|record| record.sequence == sequence)
                {
                    continue;
                }
                let input = ClientInputObservation {
                    bits: 0,
                    direction: snapshot.direction,
                };
                client
                    .recent_input_records
                    .push_back(SequencedInputObservation {
                        sequence,
                        input,
                        door_seed: 0,
                        door_phase_tick: 0,
                    });
                if client
                    .last_input_seq
                    .is_none_or(|last| sequence != last && sequence.wrapping_sub(last) < (1 << 15))
                {
                    client.last_input_seq = Some(sequence);
                    client.last_input_record = Some(input);
                }
            }
        }
        self.queue_position_input_for_runtime(identity, packet);
    }

    pub(crate) fn queue_position_input_for_runtime(
        &mut self,
        identity: PlayerRuntimeIdentity,
        packet: Vec<u8>,
    ) -> bool {
        if !self.player_runtime_matches(identity) {
            return false;
        }
        self.pending_position_inputs.queue(identity, packet);
        true
    }

    pub(crate) fn queue_lifecycle_baseline_for_runtime(
        &mut self,
        identity: PlayerRuntimeIdentity,
        packet: Vec<u8>,
        reliable_delivery_id: u32,
    ) -> bool {
        if !self.player_runtime_matches(identity) {
            return false;
        }
        self.pending_position_inputs.queue_lifecycle_baseline(
            identity,
            packet,
            reliable_delivery_id,
        );
        true
    }

    pub(crate) fn drain_position_inputs(
        &mut self,
    ) -> Vec<(PlayerRuntimeIdentity, Vec<u8>, bool, Option<u32>)> {
        self.pending_position_inputs.drain()
    }

    pub(crate) fn queue_reliable_gameplay_for_runtime(
        &mut self,
        identity: PlayerRuntimeIdentity,
        reliable_delivery_id: u32,
        kind: PendingReliableGameplayMutationKind,
    ) -> bool {
        if !self.player_runtime_matches(identity) {
            return false;
        }
        self.pending_reliable_gameplay
            .enqueue(PendingReliableGameplayMutation {
                identity,
                reliable_delivery_id,
                kind,
            })
    }

    #[cfg(test)]
    pub(crate) fn queue_frequency_change_for_runtime(
        &mut self,
        identity: PlayerRuntimeIdentity,
        reliable_delivery_id: u32,
        payload: Vec<u8>,
    ) -> bool {
        let Some(request) = ReliableFrequencyChange::parse(&payload) else {
            return false;
        };
        self.queue_reliable_gameplay_for_runtime(
            identity,
            reliable_delivery_id,
            PendingReliableGameplayMutationKind::Frequency(request),
        )
    }

    #[cfg(test)]
    pub(crate) fn reliable_gameplay_heads(&mut self) -> Vec<PendingReliableGameplayMutation> {
        self.pending_reliable_gameplay.fair_heads()
    }

    pub(crate) fn reliable_gameplay_ready_runtimes_with_budget(
        &mut self,
        budget: usize,
    ) -> Vec<PlayerRuntimeIdentity> {
        self.pending_reliable_gameplay
            .fair_runtime_identities_with_budget(budget)
    }

    pub(crate) fn reliable_gameplay_head_for_runtime(
        &self,
        identity: PlayerRuntimeIdentity,
    ) -> Option<PendingReliableGameplayMutation> {
        self.pending_reliable_gameplay.runtime_head(identity)
    }

    pub(crate) fn reliable_gameplay_runtime_len(&self, identity: PlayerRuntimeIdentity) -> usize {
        self.pending_reliable_gameplay.runtime_len(identity)
    }

    pub(crate) fn pop_reliable_gameplay_terminal(
        &mut self,
        terminal: &PendingReliableGameplayMutation,
    ) -> bool {
        self.pending_reliable_gameplay.pop_terminal(terminal)
    }

    pub(crate) fn queue_portal_placement_for_runtime(
        &mut self,
        identity: PlayerRuntimeIdentity,
        request: crate::portal_placement::PortalPlacementRequest,
        queued_at_tick: u32,
    ) -> bool {
        if !self.player_runtime_matches(identity) {
            return false;
        }
        if self.pending_portal_placements.iter().any(|pending| {
            pending.identity == identity && pending.request.action_id == request.action_id
        }) {
            return true;
        }
        let runtime_depth = self
            .pending_portal_placements
            .iter()
            .filter(|pending| pending.identity == identity)
            .count();
        if runtime_depth >= MAX_PENDING_PORTAL_PLACEMENTS_PER_RUNTIME
            || self.pending_portal_placements.len() >= MAX_PENDING_PORTAL_PLACEMENTS_TOTAL
        {
            return false;
        }
        self.pending_portal_placements
            .push_back(PendingPortalPlacement {
                identity,
                request,
                queued_at_tick,
            });
        true
    }

    pub(crate) fn drain_portal_placements(&mut self) -> Vec<PendingPortalPlacement> {
        self.pending_portal_placements.drain(..).collect()
    }

    pub(crate) fn requeue_deferred_portal_placement(
        &mut self,
        pending: PendingPortalPlacement,
    ) -> bool {
        if !self.player_runtime_matches(pending.identity)
            || self.pending_portal_placements.len() >= MAX_PENDING_PORTAL_PLACEMENTS_TOTAL
        {
            return false;
        }
        self.pending_portal_placements.push_back(pending);
        true
    }

    pub(crate) fn queue_portal_warp_for_runtime(
        &mut self,
        identity: PlayerRuntimeIdentity,
        request: crate::portal_warp::PortalWarpRequest,
        queued_at_tick: u32,
    ) -> bool {
        if !self.player_runtime_matches(identity) {
            return false;
        }
        if self.pending_portal_warps.iter().any(|pending| {
            pending.identity == identity && pending.request.action_id == request.action_id
        }) {
            return true;
        }
        let runtime_depth = self
            .pending_portal_warps
            .iter()
            .filter(|pending| pending.identity == identity)
            .count();
        if runtime_depth >= MAX_PENDING_PORTAL_PLACEMENTS_PER_RUNTIME
            || self.pending_portal_warps.len() >= MAX_PENDING_PORTAL_PLACEMENTS_TOTAL
        {
            return false;
        }
        self.pending_portal_warps.push_back(PendingPortalWarp {
            identity,
            request,
            queued_at_tick,
        });
        true
    }

    pub(crate) fn drain_portal_warps(&mut self) -> Vec<PendingPortalWarp> {
        self.pending_portal_warps.drain(..).collect()
    }

    pub(crate) fn requeue_deferred_portal_warp(&mut self, pending: PendingPortalWarp) -> bool {
        if !self.player_runtime_matches(pending.identity)
            || self.pending_portal_warps.len() >= MAX_PENDING_PORTAL_PLACEMENTS_TOTAL
        {
            return false;
        }
        self.pending_portal_warps.push_back(pending);
        true
    }

    pub(crate) fn queue_objective_action_for_runtime(
        &mut self,
        identity: PlayerRuntimeIdentity,
        reliable_delivery_id: u32,
        request: crate::objective_protocol::ObjectiveActionRequest,
        queued_at_tick: u32,
    ) -> bool {
        if !self.player_runtime_matches(identity) {
            return false;
        }
        if self
            .pending_reliable_gameplay
            .runtime_items(identity)
            .any(|pending| {
                matches!(
                    pending.kind,
                    PendingReliableGameplayMutationKind::Objective(objective)
                        if objective.request.action_id == request.action_id
                )
            })
        {
            return true;
        }
        self.queue_reliable_gameplay_for_runtime(
            identity,
            reliable_delivery_id,
            PendingReliableGameplayMutationKind::Objective(PendingObjectiveAction {
                identity,
                reliable_delivery_id,
                request,
                queued_at_tick,
            }),
        )
    }

    /// Snapshot at most one ordered objective-action head per runtime. The
    /// resident queue entry is removed only after a terminal result.
    #[cfg(test)]
    pub(crate) fn objective_action_heads(
        &mut self,
    ) -> Vec<(PendingReliableGameplayMutation, PendingObjectiveAction)> {
        self.reliable_gameplay_heads()
            .into_iter()
            .filter_map(|pending| {
                let PendingReliableGameplayMutationKind::Objective(objective) = pending.kind else {
                    return None;
                };
                Some((pending, objective))
            })
            .collect()
    }

    pub(crate) fn objective_actions_for_runtime(
        &self,
        identity: PlayerRuntimeIdentity,
    ) -> Vec<PendingObjectiveAction> {
        self.pending_reliable_gameplay
            .runtime_items(identity)
            .filter_map(|pending| match pending.kind {
                PendingReliableGameplayMutationKind::Objective(objective) => Some(objective),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn reliable_gameplay_items_for_runtime(
        &self,
        identity: PlayerRuntimeIdentity,
    ) -> Vec<PendingReliableGameplayMutation> {
        self.pending_reliable_gameplay
            .runtime_items(identity)
            .copied()
            .collect()
    }

    pub(crate) fn pending_leave_identities(&self) -> Vec<PlayerRuntimeIdentity> {
        self.pending_reliable_gameplay
            .queues
            .iter()
            .filter_map(|(&identity, queue)| {
                queue
                    .items
                    .iter()
                    .any(|pending| {
                        matches!(pending.kind, PendingReliableGameplayMutationKind::Leave)
                    })
                    .then_some(identity)
            })
            .collect()
    }

    pub(crate) fn retire_pending_applications_for_runtime(
        &mut self,
        identity: PlayerRuntimeIdentity,
    ) {
        self.pending_reliable_gameplay
            .retain_runtime(|candidate| candidate != identity);
    }

    pub(crate) fn queue_movement_correction_application_for_runtime(
        &mut self,
        identity: PlayerRuntimeIdentity,
        reliable_delivery_id: u32,
        application: crate::movement_correction::MovementCorrectionApplied,
    ) -> bool {
        self.queue_reliable_gameplay_for_runtime(
            identity,
            reliable_delivery_id,
            PendingReliableGameplayMutationKind::MovementCorrection(application),
        )
    }

    pub(crate) fn bind_pending_brick_sources_after_positions(&mut self) {
        let sources = self
            .pending_reliable_gameplay
            .queues
            .keys()
            .copied()
            .map(|identity| {
                let source = self.clients.get(&identity.player_id()).and_then(|client| {
                    if !self.player_runtime_matches(identity) {
                        return None;
                    }
                    let (x, y, direction) = client
                        .canonical_movement_witness
                        .filter(|witness| witness.life_seq == identity.life.life_seq)
                        .map(|witness| (witness.wire_x, witness.wire_y, witness.direction))
                        .or_else(|| {
                            let position = client.last_position.as_deref()?;
                            (position.len() >= 17
                                && position[0] == crate::arena::LARGE_POSITION_PACKET)
                                .then(|| {
                                    (
                                        u16::from_le_bytes([position[4], position[5]]),
                                        u16::from_le_bytes([position[15], position[16]]),
                                        position[1],
                                    )
                                })
                        })?;
                    Some(BrickSourceGeometry {
                        x,
                        y,
                        direction,
                        ship: client.player.ship,
                        frequency: client.player.frequency,
                    })
                });
                (identity, source)
            })
            .collect::<BTreeMap<_, _>>();
        self.pending_reliable_gameplay.bind_brick_sources(&sources);
    }

    pub(crate) fn bind_pending_brick_source_for_runtime_after_correction(
        &mut self,
        identity: PlayerRuntimeIdentity,
    ) {
        let source = self.brick_source_geometry(identity);
        self.pending_reliable_gameplay
            .bind_brick_source_for_runtime(identity, source);
    }

    fn brick_source_geometry(
        &self,
        identity: PlayerRuntimeIdentity,
    ) -> Option<BrickSourceGeometry> {
        let client = self.clients.get(&identity.player_id())?;
        if !self.player_runtime_matches(identity) {
            return None;
        }
        let (x, y, direction) = client
            .canonical_movement_witness
            .filter(|witness| witness.life_seq == identity.life.life_seq)
            .map(|witness| (witness.wire_x, witness.wire_y, witness.direction))
            .or_else(|| {
                let position = client.last_position.as_deref()?;
                (position.len() >= 17 && position[0] == crate::arena::LARGE_POSITION_PACKET).then(
                    || {
                        (
                            u16::from_le_bytes([position[4], position[5]]),
                            u16::from_le_bytes([position[15], position[16]]),
                            position[1],
                        )
                    },
                )
            })?;
        Some(BrickSourceGeometry {
            x,
            y,
            direction,
            ship: client.player.ship,
            frequency: client.player.frequency,
        })
    }

    pub(crate) fn clear_pending_position_inputs_for_player(&mut self, player_id: u16) {
        self.pending_position_inputs.clear_player(player_id);
        self.weapon_admission_deferred
            .retain(|identity| identity.player_id != player_id);
        let dropped = self
            .pending_reliable_gameplay
            .retain_runtime(|identity| identity.player_id() != player_id);
        if dropped > 0 {
            tracing::warn!(
                player_id,
                dropped,
                "reliable gameplay queued for this player was dropped at a position boundary"
            );
        }
        self.pending_portal_placements
            .retain(|pending| pending.identity.player_id() != player_id);
        self.pending_portal_warps
            .retain(|pending| pending.identity.player_id() != player_id);
        self.input_recovery_deferred_at
            .retain(|identity, _| identity.player_id != player_id);
        let cancelled = self
            .pending_damage_transactions
            .iter()
            .filter_map(|(&projectile_id, transaction)| {
                transaction
                    .involves_player(player_id)
                    .then_some(projectile_id)
            })
            .collect::<Vec<_>>();
        for projectile_id in cancelled {
            self.active_projectiles.resolve_hit(projectile_id);
            self.pending_damage_transactions.remove(&projectile_id);
        }
        self.prune_retired_projectile_owners();
    }

    fn clear_pending_position_inputs_after_damage_death(
        &mut self,
        runtime: PlayerRuntimeIdentity,
        collision_tick: u32,
    ) {
        let player_id = runtime.player_id();
        self.pending_position_inputs.clear_player(player_id);
        self.weapon_admission_deferred
            .retain(|identity| identity.player_id != player_id);
        let dropped = self
            .pending_reliable_gameplay
            .retain_runtime(|identity| identity.player_id() != player_id);
        if dropped > 0 {
            tracing::warn!(
                player_id,
                dropped,
                collision_tick,
                "reliable gameplay queued for this player was dropped at a death boundary"
            );
        }
        self.pending_portal_placements
            .retain(|pending| pending.identity.player_id() != player_id);
        self.pending_portal_warps
            .retain(|pending| pending.identity.player_id() != player_id);
        self.input_recovery_deferred_at
            .retain(|identity, _| identity.player_id != player_id);

        let cancelled = self
            .pending_damage_transactions
            .iter()
            .filter_map(|(&projectile_id, transaction)| {
                (!transaction.survives_same_tick_shooter_death(runtime, collision_tick))
                    .then_some(projectile_id)
                    .filter(|_| transaction.involves_player(player_id))
            })
            .collect::<Vec<_>>();
        for projectile_id in cancelled {
            self.active_projectiles.resolve_hit(projectile_id);
            self.pending_damage_transactions.remove(&projectile_id);
        }
    }

    pub(crate) fn retire_projectile_owner_after_damage_death(
        &mut self,
        runtime: PlayerRuntimeIdentity,
        collision_tick: u32,
    ) {
        self.retired_projectile_owners
            .insert(runtime, collision_tick);
        // The reference retires a projectile by its own alive time or its own explosion, never by
        // its shooter's death, so only the collisions that already targeted this dying actor are
        // settled here; the shots it fired stay in flight until their own `expiry_tick`.
        self.active_projectiles
            .resolve_incoming_hits_of_life(runtime.life);
        self.pending_damage_transactions
            .retain(|projectile_id, transaction| {
                (transaction.survives_same_tick_shooter_death(runtime, collision_tick)
                    && self.active_projectiles.contains_pending_hit(*projectile_id))
                    || !transaction.involves_player(runtime.player_id())
            });
        self.prune_retired_projectile_owners();
    }

    pub(crate) fn actor_runtime_matches_damage_collision(
        &self,
        runtime: PlayerRuntimeIdentity,
        collision_tick: u32,
    ) -> bool {
        if self.actor_runtime_matches(runtime) {
            return true;
        }
        if !self.retired_projectile_owners.contains_key(&runtime) {
            return false;
        }
        // A retired shooter stays authoritative for its own shots until that weapon's alive time
        // ends, which is the reference's removal rule rather than a same-tick grace window. The
        // session check below is what actually matters: a client or bot that has really left
        // cannot claim anything, and the per-hit record pin in `damage_shooter_runtime` ties each
        // accepted hit to a projectile the server authored.
        let _ = collision_tick;
        self.clients
            .get(&runtime.player_id())
            .is_some_and(|client| client.session_id == runtime.session.session_id && client.joined)
            || self
                .bot(runtime.player_id())
                .is_some_and(|bot| bot.session_id == runtime.session.session_id)
    }

    /// True when the server still holds the live record this hit names, owned by the same life the
    /// hit claims. This is what lets a dead shooter's shot keep scoring without letting a hit that
    /// merely names a retired life be accepted.
    pub(crate) fn projectile_flight_is_authoritative(
        &self,
        hit: &crate::authority_projectiles::ProjectileHit,
    ) -> bool {
        self.active_projectiles.flight_owner_of(hit.projectile_id) == Some(hit.owner_identity())
    }

    pub(crate) fn prune_retired_projectile_owners(&mut self) {
        let completed = self
            .retired_projectile_owners
            .iter()
            .filter_map(|(runtime, collision_tick)| {
                (!self
                    .active_projectiles
                    .contains_flight_of_life(runtime.life)
                    && !self
                        .active_projectiles
                        .contains_pending_hit_for_owner_at_tick(runtime.life, *collision_tick))
                .then_some((*runtime, *collision_tick))
            })
            .collect::<Vec<_>>();
        for (runtime, collision_tick) in completed {
            self.retired_projectile_owners.remove(&runtime);
            // No `remove_owner_life` here: the shots are gone by now because their own alive time
            // or explosion removed them, and deleting by owner would re-introduce the bug this
            // replaced. Only the transactions that could never be delivered again are dropped.
            let _ = collision_tick;
            self.pending_damage_transactions.retain(|_, transaction| {
                !transaction.survives_same_tick_shooter_death(runtime, collision_tick)
            });
        }
        for (&player_id, client) in &mut self.clients {
            client
                .retired_projectile_observers
                .retain(|life, timeline| {
                    let runtime = PlayerRuntimeIdentity::new(
                        PlayerSessionIdentity::new(player_id, client.session_id),
                        *life,
                    );
                    self.retired_projectile_owners.contains_key(&runtime)
                        && timeline.arena_stream_id() == Some(client.arena_stream_id)
                });
        }
    }

    pub(crate) fn remove_projectiles_for_player(&mut self, player_id: u16) {
        self.active_projectiles.remove_owner(player_id);
        self.pending_damage_transactions
            .retain(|projectile_id, transaction| {
                !transaction.involves_player(player_id)
                    && self.active_projectiles.contains_pending_hit(*projectile_id)
            });
        self.prune_retired_projectile_owners();
    }

    pub(crate) fn expire_prize_claims_older_than(
        &mut self,
        tick: u32,
        max_age_ticks: u32,
    ) -> usize {
        let before = self.claimed_prizes.len();
        self.claimed_prizes.retain(|_, receipt| {
            crate::continuum::continuum_tick_elapsed(tick, receipt.claimed_at_tick) <= max_age_ticks
        });
        before.saturating_sub(self.claimed_prizes.len())
    }

    pub(crate) fn record_prize_claim_receipt(
        &mut self,
        collectible_id: u32,
        receipt: ServerPrizeReceipt,
        tick: u32,
    ) {
        if !self.claimed_prizes.contains_key(&collectible_id)
            && self.claimed_prizes.len() >= MAX_PRIZE_CLAIM_RECEIPTS
        {
            let oldest_id = self
                .claimed_prizes
                .iter()
                .max_by_key(|entry| {
                    let (id, prior) = *entry;
                    (
                        crate::continuum::continuum_tick_elapsed(tick, prior.claimed_at_tick),
                        std::cmp::Reverse(*id),
                    )
                })
                .map(|(&id, _)| id);
            if let Some(oldest_id) = oldest_id {
                self.claimed_prizes.remove(&oldest_id);
            }
        }
        self.claimed_prizes.insert(collectible_id, receipt);
    }

    #[cfg(test)]
    pub(crate) fn visible_position_recipient_ids(
        &self,
        sender_id: u16,
        position: &[u8],
    ) -> Vec<u16> {
        let mut candidates = self
            .visibility_index
            .candidate_player_ids(position, POSITION_FANOUT_RADIUS_PX);
        candidates.extend(self.clients.iter().filter_map(|(&id, client)| {
            (id != sender_id && client.joined && client.visibility_cell.is_none()).then_some(id)
        }));
        candidates
            .into_iter()
            .filter(|&id| {
                id != sender_id
                    && self.clients.get(&id).is_some_and(|client| {
                        client.joined
                            && position_visible_to_recipient(
                                position,
                                client.last_position.as_deref(),
                                POSITION_FANOUT_RADIUS_PX,
                            )
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod send_pacing_tests {
    use super::SendPacing;

    #[test]
    fn try_consume_drains_tokens_then_drops_until_refill() {
        let mut pacing = SendPacing::default();
        // Default cap = 64 KiB starting balance.
        let start_tokens = pacing.tokens;
        assert!(pacing.try_consume(1024));
        assert!((start_tokens - pacing.tokens - 1024.0).abs() < f64::EPSILON);
        assert_eq!(pacing.packets_dropped, 0);

        // Spend the rest of the bucket and verify the next call is
        // dropped (and counters bump).
        let remaining = pacing.tokens as usize;
        assert!(pacing.try_consume(remaining));
        assert!(!pacing.try_consume(1));
        assert_eq!(pacing.packets_dropped, 1);
        assert_eq!(pacing.bytes_dropped, 1);
    }

    #[test]
    fn refill_caps_at_one_second_worth() {
        let mut pacing = SendPacing::default();
        pacing.tokens = 0.0;
        for _ in 0..200 {
            pacing.refill_for_tick(40);
        }
        // Even after many refills the bucket can't exceed `cap`.
        assert!(
            (pacing.tokens - pacing.cap_bytes_per_second).abs() < f64::EPSILON,
            "tokens={} cap={}",
            pacing.tokens,
            pacing.cap_bytes_per_second
        );
    }

    #[test]
    fn refill_at_40hz_adds_one_fortieth_of_cap_per_tick() {
        let mut pacing = SendPacing::default();
        pacing.tokens = 0.0;
        pacing.refill_for_tick(40);
        let expected = pacing.cap_bytes_per_second / 40.0;
        assert!((pacing.tokens - expected).abs() < 1.0);
    }
}

#[cfg(test)]
mod load_shed_tests {
    use super::{LOAD_SHED_FRAME_OVERRUN_STREAK, ServerState};

    #[test]
    fn load_pressure_ratio_zero_until_first_tick_recorded() {
        let state = ServerState::default();
        assert_eq!(state.load_pressure_ratio(), 0.0);
    }

    #[test]
    fn load_pressure_ratio_matches_budget_fraction() {
        let mut state = ServerState::default();
        let tick_rate = state.config.tick_rate_hz.max(1) as u64;
        let budget_us = 1_000_000 / tick_rate;
        // Record a tick that took exactly half the budget; first
        // sample fully seeds the EWMA so the ratio reads 0.5
        // immediately.
        state.record_tick_wallclock(budget_us / 2);
        let ratio = state.load_pressure_ratio();
        assert!(
            (ratio - 0.5).abs() < 0.001,
            "expected ratio ≈ 0.5, got {ratio}"
        );
    }

    #[test]
    fn load_pressure_ratio_caps_above_one_when_over_budget() {
        let mut state = ServerState::default();
        let tick_rate = state.config.tick_rate_hz.max(1) as u64;
        let budget_us = 1_000_000 / tick_rate;
        // Record a tick that took 1.5× the budget — pressure climbs
        // above 1.0 so callers see the "running over" signal.
        state.record_tick_wallclock(budget_us * 3 / 2);
        assert!(state.load_pressure_ratio() > 1.0);
    }

    #[test]
    fn advance_arena_tick_tracks_interval_jitter() {
        use std::time::{Duration, Instant};
        let mut state = ServerState::default();
        state.config.tick_rate_hz = 100; // target interval = 10 ms
        let t0 = Instant::now();
        state.advance_arena_tick(t0);
        assert_eq!(
            state.tick_interval_jitter_ewma_us, 0.0,
            "first tick only seeds the timestamp"
        );
        // A second tick exactly on the 10 ms target → ~0 jitter.
        state.advance_arena_tick(t0 + Duration::from_millis(10));
        assert!(
            state.tick_interval_jitter_ewma_us < 1.0,
            "an on-target interval registers ~0 jitter, got {}",
            state.tick_interval_jitter_ewma_us
        );
        // A tick 5 ms late (15 ms gap) blends a ~5000 µs deviation in.
        state.advance_arena_tick(t0 + Duration::from_millis(25));
        assert!(
            state.tick_interval_jitter_ewma_us > 100.0,
            "a late tick raises the jitter EWMA, got {}",
            state.tick_interval_jitter_ewma_us
        );
    }

    #[test]
    fn record_tick_wallclock_uses_ewma_after_seed() {
        let mut state = ServerState::default();
        state.record_tick_wallclock(1000); // seed
        state.record_tick_wallclock(2000);
        // After one blend at α=1/8: 7/8 * 1000 + 1/8 * 2000 = 1125.
        assert!(
            (state.tick_wallclock_ewma_us - 1125.0).abs() < 0.5,
            "got {}",
            state.tick_wallclock_ewma_us
        );
    }

    #[test]
    fn whole_frame_pressure_requires_consecutive_actual_tick_overruns() {
        let mut state = ServerState::default();
        let tick_rate = state.config.tick_rate_hz.max(1) as u64;
        let budget_us = 1_000_000 / tick_rate;

        for _ in 0..LOAD_SHED_FRAME_OVERRUN_STREAK - 1 {
            state.record_tick_wallclock(budget_us + 1);
            assert!(!state.sustained_frame_load_pressure());
        }
        state.record_tick_wallclock(budget_us + 1);
        assert!(state.sustained_frame_load_pressure());

        state.record_tick_wallclock(budget_us / 2);
        assert_eq!(state.tick_wallclock_overrun_streak, 0);
        assert!(!state.sustained_frame_load_pressure());
    }
}

#[cfg(test)]
mod realtime_identity_tests {
    use super::ServerState;

    #[test]
    fn arena_stream_identity_never_wraps_or_reuses_zero() {
        let mut state = ServerState {
            next_arena_stream_id: u64::MAX - 1,
            ..ServerState::default()
        };
        assert_eq!(state.allocate_arena_stream_id(), Some(u64::MAX));
        assert_eq!(state.allocate_arena_stream_id(), None);
        assert_eq!(state.next_arena_stream_id, u64::MAX);
    }

    #[test]
    fn observer_timeline_generation_wraps_directly_to_one() {
        let mut state = ServerState {
            next_observer_timeline_generation: u32::MAX - 1,
            ..ServerState::default()
        };
        assert_eq!(state.allocate_observer_timeline_generation(), u32::MAX);
        assert_eq!(state.allocate_observer_timeline_generation(), 1);
        assert_eq!(state.allocate_observer_timeline_generation(), 2);
    }

    #[test]
    fn recipient_realtime_event_identity_wraps_directly_to_one() {
        let state = std::sync::Arc::new(std::sync::Mutex::new(ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.next_realtime_event_id = u32::MAX - 1;
        assert_eq!(client.allocate_realtime_event_id(), u32::MAX);
        assert_eq!(client.allocate_realtime_event_id(), 1);
        assert_eq!(client.allocate_realtime_event_id(), 2);
    }
}
