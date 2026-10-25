use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::CORE_BYTE;
use super::state::{ClientHandle, ClientPacketSender, ServerState};
use crate::authority_identity::{PlayerRuntimeIdentity, PlayerSessionId, PlayerSessionIdentity};

// Per-event lines log under this module; see `log_targets`.
const _: () = assert!(crate::log_targets::is_child_of(
    crate::log_targets::RELIABLE_ACKS,
    module_path!()
));

pub(crate) const RELIABLE_MESSAGE: u8 = 0x03;
pub(crate) const RELIABLE_ACK: u8 = 0x04;
pub(crate) const SMALL_CHUNK_BODY: u8 = 0x08;
pub(crate) const SMALL_CHUNK_TAIL: u8 = 0x09;
pub(crate) const PACKET_CLUSTER: u8 = 0x0e;
const RELIABLE_HEADER_SIZE: usize = 6;
const MAX_PACKET_SIZE: usize = 520;
pub(crate) const MAX_RELIABLE_PAYLOAD_SIZE: usize = MAX_PACKET_SIZE - RELIABLE_HEADER_SIZE;
const MAX_SMALL_CHUNK_PAYLOAD_SIZE: usize = MAX_RELIABLE_PAYLOAD_SIZE - 2;
const RELIABLE_ID_HALF_WINDOW: u32 = 1 << 31;
const MAX_INCOMING_RELIABLE_QUEUE: usize = 256;
const MAX_INCOMING_RELIABLE_DISPATCH_BACKLOG: usize = MAX_INCOMING_RELIABLE_QUEUE + 1;
const MAX_INCOMING_SMALL_CHUNK_BYTES: usize = crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES;
const MAX_INCOMING_SMALL_CHUNK_FRAGMENTS: usize = crate::zone_wire::MAX_TRANSFER_FRAGMENTS;
// This remains the aggregate per-client reliable cap for lifecycle/control and
// other application traffic. Map/LVZ downloads use the separate 64-fragment
// sliding window below, so one large asset is never eagerly retained here.
// A peer that genuinely never ACKs eventually trips this cap and must be torn
// down rather than displacing already-retained reliable facts.
const MAX_OUTGOING_RELIABLE_QUEUE: usize = 4096;
const APPLICATION_TRANSFER_CONTROL_RESERVE: usize = 512;
/// Keep every later control within the client's 256-wrapper reorder window,
/// even when an early file fragment is lost. Received fragments are ACKed and
/// replenished, while lost fragments remain counted until the ordinary
/// reliable resend lands.
pub(crate) const APPLICATION_TRANSFER_IN_FLIGHT_FRAMES: usize = 64;
const OUTGOING_RELIABLE_RESEND_DELAY: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
struct ServerReliableSentPacket {
    packet: Vec<u8>,
    last_sent_at: Instant,
}

/// [REVIEW M7] The sent queue is keyed by reliable id so per-ack removal
/// is O(log n) instead of an O(n) `Vec::retain` — a clustered ack burst
/// during a map download (up to 74 acks per cluster against a ~4096-entry
/// queue) previously cost O(n·k) under the state lock. `BTreeMap`
/// iteration is ascending-id, which matches send order everywhere except
/// across the u32 id wrap; resend emission order within one burst is not
/// a protocol guarantee (the receiver reorders by id), so that boundary
/// is harmless.
#[derive(Clone, Debug, Default)]
pub(crate) struct ServerReliableSender {
    next_id: u32,
    sent: BTreeMap<u32, ServerReliableSentPacket>,
}

/// Session-scoped join fence. A successful sink call only establishes that
/// the initial send pass ran; it does not establish delivery (and synthetic
/// loss can intentionally suppress every frame). Readiness therefore depends
/// on acknowledgements for the exact ids reserved by this bootstrap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BootstrapReliableBarrier {
    session_id: PlayerSessionId,
    pending_ids: BTreeSet<u32>,
    delivery_complete: bool,
    // Diagnostic timestamps never participate in readiness or retry policy.
    trace_started: Option<(Instant, usize)>,
}

impl BootstrapReliableBarrier {
    pub(crate) fn new(session_id: PlayerSessionId, pending_ids: BTreeSet<u32>) -> Self {
        let trace_started = (crate::connection::position_timing_trace_enabled()
            && tracing::enabled!(tracing::Level::INFO))
        .then(Instant::now);
        Self::new_with_trace(session_id, pending_ids, trace_started)
    }

    fn new_with_trace(
        session_id: PlayerSessionId,
        pending_ids: BTreeSet<u32>,
        trace_started: Option<Instant>,
    ) -> Self {
        let barrier = Self {
            session_id,
            trace_started: trace_started.map(|started| (started, pending_ids.len())),
            pending_ids,
            delivery_complete: false,
        };
        barrier.trace_progress("reserved");
        barrier
    }

    fn trace_progress(&self, phase: &'static str) {
        if let Some((started, initial_frames)) = self.trace_started {
            tracing::info!(
                session_id = ?self.session_id,
                phase,
                elapsed_us = started.elapsed().as_micros() as u64,
                initial_frames,
                pending_frames = self.pending_ids.len(),
                delivery_complete = self.delivery_complete,
                "bootstrap reliable progress"
            );
        }
    }

    fn acknowledge(&mut self, session_id: PlayerSessionId, id: u32) {
        let previous_count = self.pending_ids.len();
        if self.session_id == session_id && self.pending_ids.remove(&id) {
            if self
                .trace_started
                .is_some_and(|(_, initial)| previous_count == initial)
            {
                self.trace_progress("first_ack");
            }
            if self.pending_ids.is_empty() {
                self.trace_progress("all_acked");
            }
        }
    }

    fn mark_delivery_complete(&mut self, session_id: PlayerSessionId) -> bool {
        if self.session_id != session_id {
            return false;
        }
        if !self.delivery_complete {
            self.delivery_complete = true;
            self.trace_progress("sink_complete");
        }
        true
    }

    pub(crate) fn initial_delivery_complete(&self) -> bool {
        self.delivery_complete
    }

    fn ready(&self) -> bool {
        self.delivery_complete && self.pending_ids.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn pending_ids(&self) -> Vec<u32> {
        self.pending_ids.iter().copied().collect()
    }
}

impl ServerReliableSender {
    fn send(&mut self, payload: &[u8], now: Instant) -> Option<Vec<u8>> {
        if self.sent.len() >= MAX_OUTGOING_RELIABLE_QUEUE {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let packet = reliable(id, payload);
        self.sent.insert(
            id,
            ServerReliableSentPacket {
                packet: packet.clone(),
                last_sent_at: now,
            },
        );
        Some(packet)
    }

    fn receive_ack(&mut self, id: u32) -> bool {
        self.sent.remove(&id).is_some()
    }

    pub(crate) fn can_retain_application_frames(&self, frames: usize) -> bool {
        frames <= MAX_OUTGOING_RELIABLE_QUEUE - APPLICATION_TRANSFER_CONTROL_RESERVE
            && self.sent.len().saturating_add(frames)
                <= MAX_OUTGOING_RELIABLE_QUEUE - APPLICATION_TRANSFER_CONTROL_RESERVE
    }

    fn resend_now(&mut self, id: u32, now: Instant) -> Option<Vec<u8>> {
        let packet = self.sent.get_mut(&id)?;
        packet.last_sent_at = now;
        Some(packet.packet.clone())
    }

    fn resend_due(&mut self, now: Instant) -> Vec<Vec<u8>> {
        self.sent
            .values_mut()
            .filter_map(|packet| {
                if now.duration_since(packet.last_sent_at) >= OUTGOING_RELIABLE_RESEND_DELAY {
                    packet.last_sent_at = now;
                    Some(packet.packet.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.sent.len()
    }

    #[cfg(test)]
    pub(crate) fn pending_ids(&self) -> Vec<u32> {
        self.sent.keys().copied().collect()
    }

    #[cfg(test)]
    pub(crate) fn set_next_id_for_test(&mut self, next_id: u32) {
        assert!(
            self.sent.is_empty(),
            "test id may only be set on an empty sender"
        );
        self.next_id = next_id;
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ServerReliableReceiver {
    next_process_id: u32,
    queued: BTreeMap<u32, Vec<u8>>,
    small_chunk_buffer: Vec<u8>,
    small_chunk_in_progress: bool,
    small_chunk_fragments: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReceivedReliablePayload {
    pub(crate) delivery_id: u32,
    pub(crate) payload: Vec<u8>,
}

impl ServerReliableReceiver {
    #[cfg(test)]
    pub(crate) fn set_next_process_id_for_test(&mut self, next_process_id: u32) {
        assert!(
            self.queued.is_empty(),
            "test rebase requires an empty queue"
        );
        assert!(
            !self.small_chunk_in_progress,
            "test rebase requires no partial logical payload"
        );
        self.next_process_id = next_process_id;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn receive(&mut self, id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
        self.receive_tagged(id, payload)
            .into_iter()
            .map(|received| received.payload)
            .collect()
    }

    pub(crate) fn receive_tagged(
        &mut self,
        id: u32,
        payload: &[u8],
    ) -> Vec<ReceivedReliablePayload> {
        self.receive_tagged_with_ack(id, payload).1
    }

    /// Returns whether the wrapper was already known or retained and may be
    /// transport-ACKed, together with every newly ordered logical payload.
    /// A new packet rejected by the bounded reorder queue must remain
    /// unacknowledged so the client retries it after the blocking gap drains.
    pub(crate) fn receive_tagged_with_ack(
        &mut self,
        id: u32,
        payload: &[u8],
    ) -> (bool, Vec<ReceivedReliablePayload>) {
        if reliable_id_before(id, self.next_process_id) || self.queued.contains_key(&id) {
            return (true, Vec::new());
        }
        if id == self.next_process_id {
            self.next_process_id = self.next_process_id.wrapping_add(1);
            let mut ordered = vec![(id, payload.to_vec())];
            ordered.extend(self.drain_ordered());
            return (true, self.logical_payloads(ordered));
        }
        let distance_ahead = id.wrapping_sub(self.next_process_id);
        if distance_ahead > MAX_INCOMING_RELIABLE_QUEUE as u32 {
            return (false, Vec::new());
        }
        if self.queued.len() >= MAX_INCOMING_RELIABLE_QUEUE {
            return (false, Vec::new());
        }
        self.queued.insert(id, payload.to_vec());
        (true, Vec::new())
    }

    /// Production receive boundary. Every current/future wrapper remains in
    /// the bounded reorder map until the exact next id reaches semantic
    /// terminal handling or durable gameplay admission. Returning `true`
    /// means only that this wrapper is an already-committed duplicate and may
    /// be ACKed immediately. Resident gaps and capacity-blocked heads stay
    /// unacknowledged even when their bytes are retained.
    pub(crate) fn receive_to_dispatch_backlog_with_ack(&mut self, id: u32, payload: &[u8]) -> bool {
        if reliable_id_before(id, self.next_process_id) {
            return true;
        }
        if self.queued.contains_key(&id) {
            return false;
        }
        let distance_ahead = id.wrapping_sub(self.next_process_id);
        if distance_ahead > MAX_INCOMING_RELIABLE_QUEUE as u32
            || self.queued.len() >= MAX_INCOMING_RELIABLE_DISPATCH_BACKLOG
        {
            return false;
        }
        self.queued.insert(
            id,
            if payload.len() <= MAX_RELIABLE_PAYLOAD_SIZE {
                payload.to_vec()
            } else {
                // Retain only an ordered terminal tombstone. The wrapper must
                // still wait for its gap, but attacker-controlled oversize
                // bytes never enter the 257-slot reorder map.
                Vec::new()
            },
        );
        false
    }

    pub(crate) fn take_dispatch_head(&mut self) -> Option<ReceivedReliablePayload> {
        self.queued
            .remove(&self.next_process_id)
            .map(|payload| ReceivedReliablePayload {
                delivery_id: self.next_process_id,
                payload,
            })
    }

    pub(crate) fn restore_dispatch_head(&mut self, payload: ReceivedReliablePayload) {
        debug_assert_eq!(payload.delivery_id, self.next_process_id);
        let previous = self.queued.insert(payload.delivery_id, payload.payload);
        debug_assert!(previous.is_none());
    }

    /// Decode the currently removed wrapper without changing the reliable
    /// cursor or chunk-reassembly state. `None` is a framing-only terminal
    /// (BODY, orphan/invalid TAIL, or an over-limit fragment). A valid TAIL is
    /// returned as one assembled logical payload but does not consume the
    /// retained prefix until `commit_dispatch_head` runs after application.
    pub(crate) fn prepare_dispatch_head(
        &self,
        wrapper: &ReceivedReliablePayload,
    ) -> Option<ReceivedReliablePayload> {
        debug_assert_eq!(wrapper.delivery_id, self.next_process_id);
        let payload = &wrapper.payload;
        if payload.len() >= 2 && payload[0] == CORE_BYTE && payload[1] == SMALL_CHUNK_BODY {
            return None;
        }
        if payload.len() >= 2 && payload[0] == CORE_BYTE && payload[1] == SMALL_CHUNK_TAIL {
            if !self.small_chunk_in_progress
                || payload.len() == 2
                || !self.can_push_small_chunk(&payload[2..])
            {
                return None;
            }
            let mut logical = Vec::with_capacity(
                self.small_chunk_buffer
                    .len()
                    .saturating_add(payload.len() - 2),
            );
            logical.extend_from_slice(&self.small_chunk_buffer);
            logical.extend_from_slice(&payload[2..]);
            return Some(ReceivedReliablePayload {
                delivery_id: wrapper.delivery_id,
                payload: logical,
            });
        }
        Some(wrapper.clone())
    }

    /// Advance exactly one wrapper after its logical payload has reached a
    /// terminal or durable admission. Chunk state changes are committed at
    /// this same point, so a blocked TAIL can be restored and retried without
    /// losing its prefix.
    pub(crate) fn commit_dispatch_head(&mut self, wrapper: &ReceivedReliablePayload) {
        debug_assert_eq!(wrapper.delivery_id, self.next_process_id);
        let _ = self.logical_payloads(vec![(wrapper.delivery_id, wrapper.payload.clone())]);
        self.next_process_id = self.next_process_id.wrapping_add(1);
    }

    #[cfg(test)]
    pub(crate) fn dispatch_backlog_len(&self) -> usize {
        self.queued.len()
    }

    fn drain_ordered(&mut self) -> Vec<(u32, Vec<u8>)> {
        let mut ordered = Vec::new();
        while let Some(payload) = self.queued.remove(&self.next_process_id) {
            let id = self.next_process_id;
            self.next_process_id = self.next_process_id.wrapping_add(1);
            ordered.push((id, payload));
        }
        ordered
    }

    fn logical_payloads(
        &mut self,
        ordered_payloads: Vec<(u32, Vec<u8>)>,
    ) -> Vec<ReceivedReliablePayload> {
        let mut logical = Vec::new();
        for (delivery_id, payload) in ordered_payloads {
            if payload.len() >= 2 && payload[0] == CORE_BYTE && payload[1] == SMALL_CHUNK_BODY {
                if payload.len() == 2 {
                    self.clear_small_chunk();
                    continue;
                }
                if !self.small_chunk_in_progress {
                    self.clear_small_chunk();
                }
                self.small_chunk_in_progress = true;
                self.push_small_chunk(&payload[2..]);
                continue;
            }
            if payload.len() >= 2 && payload[0] == CORE_BYTE && payload[1] == SMALL_CHUNK_TAIL {
                if !self.small_chunk_in_progress || payload.len() == 2 {
                    // Orphan TAIL with no preceding BODY — drop without dispatching
                    // so a peer cannot smuggle a payload by skipping the BODY framing.
                    self.clear_small_chunk();
                    continue;
                }
                if self.push_small_chunk(&payload[2..]) {
                    logical.push(ReceivedReliablePayload {
                        delivery_id,
                        payload: std::mem::take(&mut self.small_chunk_buffer),
                    });
                }
                self.clear_small_chunk();
                continue;
            }
            // Any ordered non-chunk payload is a logical boundary. It remains
            // dispatchable, but a later TAIL may not complete bytes from
            // before this boundary.
            self.clear_small_chunk();
            logical.push(ReceivedReliablePayload {
                delivery_id,
                payload,
            });
        }
        logical
    }

    fn push_small_chunk(&mut self, chunk: &[u8]) -> bool {
        let Some(next_fragments) = self.small_chunk_fragments.checked_add(1) else {
            self.clear_small_chunk();
            return false;
        };
        let Some(next_bytes) = self.small_chunk_buffer.len().checked_add(chunk.len()) else {
            self.clear_small_chunk();
            return false;
        };
        if chunk.is_empty()
            || next_fragments > MAX_INCOMING_SMALL_CHUNK_FRAGMENTS
            || next_bytes > MAX_INCOMING_SMALL_CHUNK_BYTES
        {
            self.clear_small_chunk();
            return false;
        }
        self.small_chunk_buffer.extend_from_slice(chunk);
        self.small_chunk_fragments = next_fragments;
        true
    }

    fn can_push_small_chunk(&self, chunk: &[u8]) -> bool {
        !chunk.is_empty()
            && self.small_chunk_fragments < MAX_INCOMING_SMALL_CHUNK_FRAGMENTS
            && self.small_chunk_buffer.len() <= MAX_INCOMING_SMALL_CHUNK_BYTES
            && chunk.len()
                <= MAX_INCOMING_SMALL_CHUNK_BYTES.saturating_sub(self.small_chunk_buffer.len())
    }

    fn clear_small_chunk(&mut self) {
        self.small_chunk_buffer = Vec::new();
        self.small_chunk_in_progress = false;
        self.small_chunk_fragments = 0;
    }
}

static ACK_MISMATCH_LOG_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static ACK_NO_HANDLE_LOG_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static ACK_APPLIED_LOG_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One-second throttle for the acknowledgement-path instruments, which sit on a per-packet hot path.
fn ack_instrument_due(slot: &std::sync::atomic::AtomicU64) -> bool {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let last = slot.load(std::sync::atomic::Ordering::Relaxed);
    now_ms.saturating_sub(last) >= 1_000
        && slot.swap(now_ms, std::sync::atomic::Ordering::Relaxed) != now_ms
}

fn reliable_id_before(id: u32, reference: u32) -> bool {
    id != reference && reference.wrapping_sub(id) < RELIABLE_ID_HALF_WINDOW
}

pub(crate) fn receive_reliable_acks(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerSessionIdentity,
    packet: &[u8],
) -> bool {
    let Some(ids) = reliable_ack_ids(packet) else {
        return false;
    };
    // Whether acknowledgements reach this handler at all. A stalled session with no `acks_received` line means the
    // packets are not arriving here and the question is lane/dispatch; a line alongside a still-full transfer window means
    // the ids do not match what the transfer is waiting on. Throttled: this is a per-packet hot path.
    {
        static LAST_ACK_LOG_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let last = LAST_ACK_LOG_MS.load(std::sync::atomic::Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= 1000
            && LAST_ACK_LOG_MS
                .compare_exchange(
                    last,
                    now_ms,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
        {
            tracing::info!(
                target: crate::log_targets::RELIABLE_ACKS,
                acks = ids.len(),
                first_id = ids.first().copied().unwrap_or(0),
                player_id = identity.player_id,
                "acks_received"
            );
        }
    }
    let mut state = state.lock().expect("server state should not be poisoned");
    // The lines above prove an acknowledgement reached this function; they say nothing about it being applied. Both early
    // returns below consume the packet and answer "handled" while discarding every id, so a stalled transfer looks exactly
    // like an absent acknowledgement unless the discard itself is logged.
    if !state.player_session_matches(identity) {
        if ack_instrument_due(&ACK_MISMATCH_LOG_MS) {
            tracing::info!(
                player_id = identity.player_id,
                acks = ids.len(),
                identity = ?identity,
                "acks_dropped_session_mismatch"
            );
        }
        return true;
    }
    let Some(client) = state.clients.get_mut(&identity.player_id) else {
        if ack_instrument_due(&ACK_NO_HANDLE_LOG_MS) {
            tracing::info!(
                player_id = identity.player_id,
                acks = ids.len(),
                identity = ?identity,
                "acks_dropped_no_client_handle"
            );
        }
        return true;
    };
    if ack_instrument_due(&ACK_APPLIED_LOG_MS) {
        tracing::info!(
            target: crate::log_targets::RELIABLE_ACKS,
            player_id = identity.player_id,
            acks = ids.len(),
            first_id = ids.first().copied().unwrap_or(0),
            last_id = ids.last().copied().unwrap_or(0),
            "acks_applied"
        );
    }
    for id in ids {
        // The decisive comparison for the frozen transfer window: every id this session acknowledged, bounded, next to the
        // window range logged by transfer_window_full. Sampling one ack per second cannot answer whether any id inside the
        // window was ever acknowledged, and that is the whole question.
        if client.ack_ids_logged < 400 {
            client.ack_ids_logged = client.ack_ids_logged.saturating_add(1);
            tracing::info!(
                target: crate::log_targets::RELIABLE_ACKS,
                player_id = identity.player_id,
                id,
                "ack_id_seen"
            );
        }
        // Acknowledge the bounded application transfer for every id the peer reports, independently of whether the
        // outgoing window still tracks that id. The transfer keeps its own in-flight list and its own containment check,
        // so this stays idempotent, whereas gating it on `receive_ack` tied the transfer's lifetime to a second,
        // separately-trimmed window: once an id left `sent`, its ack arrived, `receive_ack` answered false, and the
        // transfer was never told - so its window entry leaked forever. A zone-file transfer acks a 64-frame window, the
        // entries leaked, the window filled and froze mid-file, the file never completed, the client never entered
        // gameplay authority, never sent a position, and could never be shown to a peer.
        acknowledge_bounded_map_application_transfer(client, id);
        if client.outgoing_reliable.receive_ack(id) {
            if client
                .pending_movement_correction
                .as_ref()
                .is_some_and(|transaction| transaction.receipt_reliable_id == Some(id))
            {
                client.pending_movement_correction = None;
            }
            if client
                .pending_repel_observations
                .front()
                .is_some_and(|event| event.receipt_acknowledges_head(id))
            {
                let retired = client
                    .pending_repel_observations
                    .pop_front()
                    .expect("matched Repel receipt belongs to queue head");
                if let Some(application) = retired.accepted_application {
                    client.retired_repel_applications.push_back(
                        crate::state::ServerRepelRetiredApplication {
                            delivery_generation: retired.delivery_generation,
                            application,
                        },
                    );
                    while client.retired_repel_applications.len()
                        > crate::state::MAX_RETIRED_REPEL_APPLICATIONS
                    {
                        client.retired_repel_applications.pop_front();
                    }
                }
                tracing::debug!(
                    player_id = identity.player_id,
                    target_life_seq = client.life_seq,
                    delivery_generation = retired.delivery_generation,
                    reliable_id = id,
                    queue_len = client.pending_repel_observations.len(),
                    "target-private Repel receipt acknowledged and queue advanced"
                );
                release_deferred_warp_prize_acks_if_ready(client);
            }
            if let Some(barrier) = client.bootstrap_reliable_barrier.as_mut() {
                barrier.acknowledge(identity.session_id, id);
            }
        }
    }
    if client
        .bootstrap_reliable_barrier
        .as_ref()
        .is_some_and(BootstrapReliableBarrier::ready)
    {
        client.snapshot_ready = client.joined;
        client.bootstrap_reliable_barrier = None;
    }
    // ACKs retire the exact contiguous file frontier and open bounded fragment
    // slots. Refill the same session-scoped transfer before releasing the
    // state lock; unrelated control traffic remains independently assignable.
    crate::session::flush_pending_file_transfer_locked(&mut state, identity);
    true
}

/// Finish the bootstrap's first sink pass for the captured session. ACKs may
/// race ahead of this call (for example in a loopback test), so readiness is
/// evaluated both here and in the ACK path. A failed sink pass deliberately
/// leaves the fence closed even if an invalid/forged ACK arrives later.
pub(crate) fn complete_bootstrap_delivery(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerSessionIdentity,
    delivery_succeeded: bool,
) -> bool {
    if !delivery_succeeded {
        return false;
    }
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state.player_session_matches(identity) {
        return false;
    }
    let client = state
        .clients
        .get_mut(&identity.player_id)
        .expect("matching bootstrap session should remain registered");
    if !client.joined {
        return false;
    }
    let Some(barrier) = client.bootstrap_reliable_barrier.as_mut() else {
        return false;
    };
    if !barrier.mark_delivery_complete(identity.session_id) {
        return false;
    }
    if barrier.ready() {
        client.snapshot_ready = true;
        client.bootstrap_reliable_barrier = None;
    }
    true
}

pub(crate) fn send_to(state: &Arc<Mutex<ServerState>>, player_id: u16, packet: Vec<u8>) {
    let destination = {
        let state = state.lock().expect("server state should not be poisoned");
        state.clients.get(&player_id).map(|client| {
            (
                client.tx.clone(),
                client.should_drop_simulated_tx(packet.len()),
            )
        })
    };
    if let Some((tx, should_drop)) = destination {
        // Phase 5 simulated WebRTC loss — see `simulated_loss`.
        if !should_drop {
            let _ = tx.send(packet);
        }
    }
}

pub(crate) fn send_reliable_to(state: &Arc<Mutex<ServerState>>, player_id: u16, payload: Vec<u8>) {
    let mut state = state.lock().expect("server state should not be poisoned");
    let Some(client) = state.clients.get_mut(&player_id) else {
        return;
    };
    send_reliable_payload(client, &payload);
}

/// Send a private application receipt only to the exact runtime that earned
/// it. Player ids and life counters are reusable; the never-reused session id
/// must still match while the reliable queue is locked, otherwise a reconnect
/// occupying the same wire id could receive and apply an old acceptance.
pub(crate) fn send_reliable_to_runtime(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    payload: Vec<u8>,
) -> bool {
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state.player_runtime_matches(identity) {
        return false;
    }
    let client = state
        .clients
        .get_mut(&identity.player_id())
        .expect("matching runtime should remain registered");
    send_reliable_payload(client, &payload);
    true
}

/// Retain a terminal for the exact connection session after a hard lifecycle
/// mutation has intentionally advanced or retired its runtime life. Session
/// identity remains never-reused, so this cannot retarget a replacement that
/// later occupies the same u16 player id.
pub(crate) fn send_reliable_to_session(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerSessionIdentity,
    payload: Vec<u8>,
) -> bool {
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state.player_session_matches(identity) {
        return false;
    }
    let client = state
        .clients
        .get_mut(&identity.player_id)
        .expect("matching session should remain registered");
    send_reliable_payload(client, &payload);
    true
}

pub(crate) fn send_movement_correction_offer_to_runtime(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    proposal: crate::movement_correction::MovementCorrection,
) -> bool {
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state.player_runtime_matches(identity) {
        return false;
    }
    let client = state
        .clients
        .get_mut(&identity.player_id())
        .expect("matching correction runtime should remain registered");
    let Some(transaction) = client.pending_movement_correction.as_ref() else {
        return false;
    };
    if transaction.proposal != proposal || !transaction.awaiting_application() {
        return false;
    }
    if transaction.offer_reliable_id.is_some() {
        return true;
    }
    let Some(id) = send_reliable_frame_tracked(client, &proposal.encode(), Instant::now()) else {
        return false;
    };
    client
        .pending_movement_correction
        .as_mut()
        .expect("validated correction transaction")
        .offer_reliable_id = Some(id);
    true
}

pub(crate) fn send_movement_correction_receipt_to_runtime(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    receipt: crate::movement_correction::MovementCorrectionAccepted,
) -> bool {
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state.player_runtime_matches(identity) {
        return false;
    }
    let client = state
        .clients
        .get_mut(&identity.player_id())
        .expect("matching correction runtime should remain registered");
    let Some(transaction) = client.pending_movement_correction.as_ref() else {
        return false;
    };
    if !transaction.receipt_matches(receipt) {
        return false;
    }
    if transaction.receipt_reliable_id.is_some() {
        return true;
    }
    let Some(id) = send_reliable_frame_tracked(client, &receipt.encode(), Instant::now()) else {
        return false;
    };
    client
        .pending_movement_correction
        .as_mut()
        .expect("validated correction transaction")
        .receipt_reliable_id = Some(id);
    true
}

/// Retry only transactions that could not reserve an outgoing reliable slot.
/// Once an id exists, the ordinary reliable sender owns retransmission.
pub(crate) fn retry_pending_movement_correction_delivery(state: &Arc<Mutex<ServerState>>) {
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        for client in state.clients.values_mut() {
            while let Some(terminal) = client.pending_reset_application_terminals.front().copied() {
                if send_reliable_frame_tracked(client, &terminal.encode(), Instant::now()).is_none()
                {
                    break;
                }
                client.pending_reset_application_terminals.pop_front();
            }
        }
    }
    let pending = {
        let state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .values()
            .filter_map(|client| {
                let identity = state.active_player_runtime_identity(client.player.id)?;
                let transaction = client.pending_movement_correction.as_ref()?;
                if let Some(receipt) = transaction.terminal_receipt() {
                    transaction.receipt_reliable_id.is_none().then_some((
                        identity,
                        None,
                        Some(receipt),
                    ))
                } else {
                    transaction.offer_reliable_id.is_none().then_some((
                        identity,
                        Some(transaction.proposal),
                        None,
                    ))
                }
            })
            .collect::<Vec<_>>()
    };
    for (identity, proposal, receipt) in pending {
        if let Some(proposal) = proposal {
            let _ = send_movement_correction_offer_to_runtime(state, identity, proposal);
        }
        if let Some(receipt) = receipt {
            let _ = send_movement_correction_receipt_to_runtime(state, identity, receipt);
        }
    }
}

/// Queue and track an exact Repel application receipt under the same runtime
/// lock that validates the immutable queue head. The reliable id is retained
/// on that head so only its transport ACK may advance the next event.
pub(crate) fn send_repel_application_receipt_to_runtime(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    delivery_generation: u32,
    payload: Vec<u8>,
) -> bool {
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state.player_runtime_matches(identity) {
        return false;
    }
    let client = state
        .clients
        .get_mut(&identity.player_id())
        .expect("matching runtime should remain registered");
    let head_matches = client
        .pending_repel_observations
        .front()
        .is_some_and(|event| {
            event.delivery_generation == delivery_generation && event.accepted_application.is_some()
        });
    let retired_matches = client
        .retired_repel_applications
        .iter()
        .any(|event| event.delivery_generation == delivery_generation);
    if retired_matches {
        send_reliable_payload(client, &payload);
        return true;
    }
    if !head_matches {
        return false;
    }
    if client
        .pending_repel_observations
        .front()
        .is_some_and(crate::state::ServerRepelObservation::has_receipt_in_flight)
    {
        // The reliable sender already owns this exact receipt. A duplicate
        // unordered 0x79 is an application-level retry request, so re-emit
        // the retained wrapper immediately instead of waiting for the generic
        // three-second reliable timeout (which is longer than RepelTime).
        // Reusing the original id prevents unbounded equivalent receipts and
        // preserves the one ACK identity that advances the FIFO.
        let reliable_id = client
            .pending_repel_observations
            .front()
            .and_then(crate::state::ServerRepelObservation::receipt_reliable_id)
            .expect("in-flight Repel receipt must retain its reliable id");
        if let Some(packet) = client
            .outgoing_reliable
            .resend_now(reliable_id, Instant::now())
            && !client.should_drop_simulated_tx(packet.len())
        {
            let _ = client.tx.send(packet);
        }
        return true;
    }
    let Some(id) = send_reliable_frame_tracked(client, &payload, Instant::now()) else {
        return false;
    };
    client
        .pending_repel_observations
        .front_mut()
        .expect("validated Repel queue head")
        .remember_receipt_reliable_id(id);
    true
}

pub(crate) fn send_reliable_payload(client: &mut ClientHandle, payload: &[u8]) {
    let now = Instant::now();
    if payload.len() <= MAX_RELIABLE_PAYLOAD_SIZE {
        send_reliable_frame(client, payload, now);
        return;
    }

    let chunks: Vec<&[u8]> = payload.chunks(MAX_SMALL_CHUNK_PAYLOAD_SIZE).collect();
    for (index, chunk) in chunks.iter().enumerate() {
        let mut chunk_payload = Vec::with_capacity(2 + chunk.len());
        chunk_payload.push(CORE_BYTE);
        chunk_payload.push(if index + 1 == chunks.len() {
            SMALL_CHUNK_TAIL
        } else {
            SMALL_CHUNK_BODY
        });
        chunk_payload.extend_from_slice(chunk);
        send_reliable_frame(client, &chunk_payload, now);
    }
}

pub(crate) fn can_retain_application_payload(client: &ClientHandle, payload_len: usize) -> bool {
    payload_len > 0
        && payload_len <= crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES
        && client
            .outgoing_reliable
            .can_retain_application_frames(reliable_frame_count(payload_len))
}

#[cfg(test)]
pub(crate) fn send_bounded_application_transfer(client: &mut ClientHandle, payload: &[u8]) -> bool {
    if payload.is_empty() || payload.len() > crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES {
        return false;
    }
    let required_frames = reliable_frame_count(payload.len());
    if !client
        .outgoing_reliable
        .can_retain_application_frames(required_frames)
    {
        return false;
    }
    let before = client.outgoing_reliable.sent.len();
    send_reliable_payload(client, payload);
    debug_assert_eq!(
        client.outgoing_reliable.sent.len() - before,
        required_frames
    );
    true
}

/// Begin one map/LVZ transfer without eagerly assigning reliable ids to its
/// complete (potentially 1 MiB) body. Each assigned wrapper is a self-contained
/// `0x7e` fragment, so ordinary roster/lifecycle controls remain legal between
/// bursts and can never sit hundreds of ids beyond a retained loss gap.
pub(crate) fn begin_bounded_map_application_transfer(
    client: &mut ClientHandle,
    file_index: u16,
    payload: Vec<u8>,
) -> bool {
    if payload.is_empty() || payload.len() > crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES {
        return false;
    }
    let Some(transfer) = client.map_transfer.as_mut() else {
        return false;
    };
    if transfer.active.is_some() {
        return false;
    }
    transfer.active = Some(crate::state::ActiveMapApplicationTransfer {
        file_index,
        started_at: Instant::now(),
        payload: payload.into(),
        next_offset: 0,
        assigned_ids: std::collections::VecDeque::new(),
        acknowledged_ids: BTreeSet::new(),
        stall_offset: 0,
        stall_since_ms: 0,
    });
    let _ = flush_bounded_map_application_transfer(client);
    true
}

pub(crate) fn acknowledge_bounded_map_application_transfer(
    client: &mut ClientHandle,
    reliable_id: u32,
) {
    if let Some(active) = client
        .map_transfer
        .as_mut()
        .and_then(|transfer| transfer.active.as_mut())
    {
        // The shape of the mismatch, because arriving acknowledgements that free nothing look identical to lost ones from
        // the outside. When an id that arrives is not one this transfer is waiting on, say what it was and what the window
        // holds. Throttled to one line per second.
        if !active.assigned_ids.contains(&reliable_id) {
            static LAST_UNMATCHED_LOG_MS: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as u64)
                .unwrap_or(0);
            let last = LAST_UNMATCHED_LOG_MS.load(std::sync::atomic::Ordering::Relaxed);
            if now_ms.saturating_sub(last) >= 1000
                && LAST_UNMATCHED_LOG_MS
                    .compare_exchange(
                        last,
                        now_ms,
                        std::sync::atomic::Ordering::Relaxed,
                        std::sync::atomic::Ordering::Relaxed,
                    )
                    .is_ok()
            {
                tracing::info!(
                    ack_id = reliable_id,
                    window_front = ?active.assigned_ids.front(),
                    window_back = ?active.assigned_ids.back(),
                    window_len = active.assigned_ids.len(),
                    "ack_id_not_in_transfer_window"
                );
            }
        }
        if active.assigned_ids.contains(&reliable_id) {
            active.acknowledged_ids.insert(reliable_id);
            while active
                .assigned_ids
                .front()
                .is_some_and(|id| active.acknowledged_ids.remove(id))
            {
                active.assigned_ids.pop_front();
            }
        }
    }
}

/// Fill the fixed in-flight window and return the completed file index only
/// after every one of its fragments has been transport-ACKed. Delaying the
/// next indexed request and map SECURITY challenge until that point keeps the
/// existing application publication boundary exact.
pub(crate) fn flush_bounded_map_application_transfer(client: &mut ClientHandle) -> Option<u16> {
    // The window line above can only fire once a transfer is active, so its absence is ambiguous between "never jammed"
    // and "never started". A client can sit waiting on an indexed file the server never began transferring, which looks
    // identical from the client side. Log the inactive case on the same throttle so a retained log says which one it was.
    if client
        .map_transfer
        .as_ref()
        .is_some_and(|transfer| transfer.active.is_none())
    {
        static LAST_INACTIVE_LOG_MS: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let last = LAST_INACTIVE_LOG_MS.load(std::sync::atomic::Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= 1000
            && LAST_INACTIVE_LOG_MS
                .compare_exchange(
                    last,
                    now_ms,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
        {
            tracing::info!(
                pending_index = ?client
                    .map_transfer
                    .as_ref()
                    .and_then(|transfer| transfer.pending_index),
                "transfer_inactive"
            );
        }
    }
    let mut active = client.map_transfer.as_mut()?.active.take()?;
    if active.next_offset == active.payload.len() && active.assigned_ids.is_empty() {
        // File completions were invisible: requests were logged and stalls were logged, but nothing said a file finished,
        // so a run where a client stopped asking after file 4 could not be told apart from one where files 0-14 had all
        // completed and something else was wrong. The observed failing client (the live session, selfPlayerId 3) requested
        // files 1, 2, 3 and 4 and then never requested 5 - and no request was refused - so the file boundary is exactly
        // where the evidence ran out. Bytes and elapsed-per-file are what a slow-versus-stuck judgement needs.
        tracing::info!(
            session = ?client.session_id,
            file_index = active.file_index,
            payload_len = active.payload.len(),
            elapsed_ms = active.started_at.elapsed().as_millis() as u64,
            "server zone_file_transfer_complete"
        );
        return Some(active.file_index);
    }

    // Receiver-side proof for the stall class. The in-flight ids for this file are removed only when the client's ack
    // arrives. The claim that used to sit here - that those acks ride the game DataChannel with its 80 ms packet lifetime -
    // was wrong and is corrected here rather than left standing: `sendNetworkPacket` in index.html routes a packet to the
    // game lane only when its first byte is 0x03 or 0x62, acknowledgements begin with CORE_BYTE (0x00) and so travel on the
    // ordered, lifetime-unbounded reliable lane, and measurement agrees - per-session door counts matched each client's
    // emitted acknowledgement count almost exactly. A full window therefore means the front frame is unacknowledged and must
    // be explained by frames not arriving or the client being slow, not by a lossy lane discarding the ack. The window front
    // and back are logged because with head-of-line ordering, one stuck front id is the whole story. A
    // window that sits full is the signature; logging it here puts the fact in the log window a failing smoke retains,
    // instead of only at teardown. Throttled to one line per second so it cannot drown the log.
    if active.assigned_ids.len() >= APPLICATION_TRANSFER_IN_FLIGHT_FRAMES {
        static LAST_STALL_LOG_MS: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let last = LAST_STALL_LOG_MS.load(std::sync::atomic::Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= 1000
            && LAST_STALL_LOG_MS
                .compare_exchange(
                    last,
                    now_ms,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
        {
            tracing::info!(
                file_index = active.file_index,
                in_flight = active.assigned_ids.len(),
                window = APPLICATION_TRANSFER_IN_FLIGHT_FRAMES,
                window_front = ?active.assigned_ids.front(),
                window_back = ?active.assigned_ids.back(),
                acknowledged_pending = active.acknowledged_ids.len(),
                next_offset = active.next_offset,
                // How long this window has been full without `next_offset` moving, straight from the stall tracker. The
                // retained artifact caps this marker at a few lines, so without the elapsed value the artifact cannot say
                // whether a stall is seconds long or runs to the end of the session - the difference between a transient
                // full window and the permanent freeze the recovery is meant to end.
                stalled_ms = if active.stall_since_ms == 0 {
                    0
                } else {
                    now_ms.saturating_sub(active.stall_since_ms)
                },
                payload_len = active.payload.len(),
                "transfer_window_full"
            );
        }
    }
    // Capacity, not the transfer window, can also stop the loop. When the outbound reliable window cannot retain another
    // application frame the loop below sends nothing at all, which looks identical to a completed or full window: no bytes
    // reach the client, no error is raised, and neither line above fires. These ACKs use the reliable channel too;
    // a capacity wait is not evidence of a game-channel lifetime drop.
    if active.next_offset < active.payload.len()
        && !client.outgoing_reliable.can_retain_application_frames(1)
    {
        static LAST_CAPACITY_LOG_MS: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let last = LAST_CAPACITY_LOG_MS.load(std::sync::atomic::Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= 1000
            && LAST_CAPACITY_LOG_MS
                .compare_exchange(
                    last,
                    now_ms,
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
        {
            tracing::info!(
                file_index = active.file_index,
                next_offset = active.next_offset,
                payload_len = active.payload.len(),
                in_flight = active.assigned_ids.len(),
                "reliable_capacity_blocked"
            );
        }
    }
    let now = Instant::now();
    while active.assigned_ids.len() < APPLICATION_TRANSFER_IN_FLIGHT_FRAMES
        && active.next_offset < active.payload.len()
        && client.outgoing_reliable.can_retain_application_frames(1)
    {
        let offset = active.next_offset;
        let end = offset
            .saturating_add(crate::file_transfer_chunk::ServerFileTransferChunk::MAX_DATA_LEN)
            .min(active.payload.len());
        let Some(fragment) = crate::file_transfer_chunk::ServerFileTransferChunk::encode(
            active.file_index,
            active.payload.len(),
            offset,
            &active.payload[offset..end],
        ) else {
            break;
        };
        let Some(reliable_id) = send_reliable_frame_tracked(client, &fragment, now) else {
            break;
        };
        active.assigned_ids.push_back(reliable_id);
        active.next_offset = end;
    }
    client
        .map_transfer
        .as_mut()
        .expect("active application transfer retains map session")
        .active = Some(active);
    None
}

fn reliable_frame_count(payload_len: usize) -> usize {
    if payload_len <= MAX_RELIABLE_PAYLOAD_SIZE {
        1
    } else {
        payload_len.div_ceil(MAX_SMALL_CHUNK_PAYLOAD_SIZE)
    }
}

/// Release every winner-only Warp pickup ACK in claim order only after this
/// exact runtime's complete applicable Repel FIFO is empty. The relocation
/// authorizations were committed at claim time, but withholding their public
/// application ACK prevents Warp from overtaking either an awaiting-marker or
/// awaiting-receipt Repel head.
pub(crate) fn release_deferred_warp_prize_acks_if_ready(client: &mut ClientHandle) -> usize {
    if !client.pending_repel_observations.is_empty() {
        return 0;
    }
    let pending = client
        .pending_warp_prize_relocations
        .iter()
        .filter(|authorization| {
            authorization.life_seq == client.life_seq && !authorization.pickup_ack_sent
        })
        .map(|authorization| {
            (
                authorization.collectible_id,
                crate::player_events::prize_pickup_ack_from_claim(
                    authorization.claim,
                    crate::player_events::PrizeRecipientAck {
                        player_id: client.player.id,
                        arena_stream_id: client.arena_stream_id,
                        authoritative_energy: authorization.expected_energy,
                        life_seq: authorization.life_seq,
                        prize_state_revision: authorization.prize_state_revision,
                        engine_shutdown_until_tick: None,
                        engine_shutdown_application_required: false,
                    },
                ),
            )
        })
        .collect::<Vec<_>>();
    for (collectible_id, payload) in &pending {
        send_reliable_payload(client, payload);
        if let Some(authorization) = client
            .pending_warp_prize_relocations
            .iter_mut()
            .find(|authorization| authorization.collectible_id == *collectible_id)
        {
            authorization.pickup_ack_sent = true;
        }
    }
    pending.len()
}

/// Reliable frames reserved while an authoritative state transaction is
/// locked, but delivered only after that lock has been released. Reserving
/// the ids first makes later broadcasts strictly newer without coupling a
/// potentially blocking packet sink to the global server-state mutex.
pub(crate) struct PreparedReliableBatch {
    tx: ClientPacketSender,
    packets: Vec<Vec<u8>>,
    reliable_ids: BTreeSet<u32>,
    overflowed: bool,
}

impl PreparedReliableBatch {
    pub(crate) fn bootstrap_ids(&self) -> Option<BTreeSet<u32>> {
        (!self.overflowed && !self.reliable_ids.is_empty()).then(|| self.reliable_ids.clone())
    }

    pub(crate) fn send(self) -> bool {
        let mut complete = !self.overflowed;
        for packet in self.packets {
            if self.tx.send(packet).is_err() {
                complete = false;
            }
        }
        if self.overflowed {
            self.tx.reliable_overflow();
        }
        complete
    }
}

pub(crate) fn send_prepared_reliable_batch_to_session_identity(
    state: &Arc<Mutex<ServerState>>,
    recipient: PlayerSessionIdentity,
    batch: PreparedReliableBatch,
) -> bool {
    let still_owns_join = {
        let state = state.lock().expect("server state should not be poisoned");
        state.player_session_matches(recipient) && state.clients[&recipient.player_id].joined
    };
    still_owns_join && batch.send()
}

pub(crate) fn prepare_reliable_payloads(
    client: &mut ClientHandle,
    payloads: &[Vec<u8>],
    now: Instant,
) -> PreparedReliableBatch {
    let mut packets = Vec::new();
    let mut reliable_ids = BTreeSet::new();
    let mut overflowed = false;
    for payload in payloads {
        if payload.len() <= MAX_RELIABLE_PAYLOAD_SIZE {
            prepare_reliable_frame(
                client,
                payload,
                now,
                &mut packets,
                &mut reliable_ids,
                &mut overflowed,
            );
            continue;
        }

        let chunks: Vec<&[u8]> = payload.chunks(MAX_SMALL_CHUNK_PAYLOAD_SIZE).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            let mut chunk_payload = Vec::with_capacity(2 + chunk.len());
            chunk_payload.push(CORE_BYTE);
            chunk_payload.push(if index + 1 == chunks.len() {
                SMALL_CHUNK_TAIL
            } else {
                SMALL_CHUNK_BODY
            });
            chunk_payload.extend_from_slice(chunk);
            prepare_reliable_frame(
                client,
                &chunk_payload,
                now,
                &mut packets,
                &mut reliable_ids,
                &mut overflowed,
            );
        }
    }
    PreparedReliableBatch {
        tx: client.tx.clone(),
        packets,
        reliable_ids,
        overflowed,
    }
}

pub(crate) fn prepare_reliable_payloads_for_session_subject_and_recipient(
    state: &Arc<Mutex<ServerState>>,
    subject: PlayerSessionIdentity,
    recipient: PlayerSessionIdentity,
    payloads: &[Vec<u8>],
    now: Instant,
) -> Option<PreparedReliableBatch> {
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state.player_session_matches(subject)
        || !state.clients[&subject.player_id].joined
        || !state.player_session_matches(recipient)
        || !state.clients[&recipient.player_id].joined
    {
        return None;
    }
    let client = state
        .clients
        .get_mut(&recipient.player_id)
        .expect("matching reliable recipient should remain registered");
    Some(prepare_reliable_payloads(client, payloads, now))
}

pub(crate) fn send_to_session_identity(
    state: &Arc<Mutex<ServerState>>,
    recipient: PlayerSessionIdentity,
    packet: Vec<u8>,
) -> bool {
    let destination = {
        let state = state.lock().expect("server state should not be poisoned");
        if !state.player_session_matches(recipient) || !state.clients[&recipient.player_id].joined {
            None
        } else {
            let client = &state.clients[&recipient.player_id];
            Some((
                client.tx.clone(),
                client.should_drop_simulated_tx(packet.len()),
            ))
        }
    };
    let Some((tx, should_drop)) = destination else {
        return false;
    };
    should_drop || tx.send(packet).is_ok()
}

fn prepare_reliable_frame(
    client: &mut ClientHandle,
    payload: &[u8],
    now: Instant,
    packets: &mut Vec<Vec<u8>>,
    reliable_ids: &mut BTreeSet<u32>,
    overflowed: &mut bool,
) {
    if let Some(packet) = client.outgoing_reliable.send(payload, now) {
        let (id, _) = reliable_parts(&packet).expect("sender must create a reliable frame");
        reliable_ids.insert(id);
        if !client.should_drop_simulated_tx(packet.len()) {
            packets.push(packet);
        }
    } else {
        *overflowed = true;
    }
}

fn send_reliable_frame(client: &mut ClientHandle, payload: &[u8], now: Instant) {
    let _ = send_reliable_frame_tracked(client, payload, now);
}

fn send_reliable_frame_tracked(
    client: &mut ClientHandle,
    payload: &[u8],
    now: Instant,
) -> Option<u32> {
    if let Some(packet) = client.outgoing_reliable.send(payload, now) {
        let (id, _) = reliable_parts(&packet).expect("sender must create a reliable frame");
        // Phase 5 simulated WebRTC loss — the reliable channel's own
        // retransmit logic in `resend_due_reliable_to_clients` handles
        // recovery, so dropping here exercises that path.
        if !client.should_drop_simulated_tx(packet.len()) {
            let _ = client.tx.send(packet);
        }
        Some(id)
    } else {
        // RTC sinks treat reliable retention overflow as terminal. Continuing
        // would silently lose ordered protocol data and leave the peer in a
        // permanently divergent state. Native/test mpsc sinks remain a no-op.
        client.tx.reliable_overflow();
        None
    }
}

/// Terminal timeout for an application transfer window that has stopped advancing, in milliseconds. Measurements on the
/// browser smoke showed a session whose uplink stopped reaching this process entirely: the client wrote thousands of
/// acknowledgements into an ordered DataChannel without error, this server's packet-handler door counted 65 of them, and
/// the same session recorded zero input and zero position packets, while the server-to-client direction kept delivering
/// hundreds of kilobytes. A window that waits for an acknowledgement which cannot arrive freezes the join forever: the
/// client stays registered, invisible to peers, and never retries. The window's ordering invariant - a later ACK may not
/// free a slot while an earlier fragment is missing - rules out sliding past an unacknowledged fragment, so the recovery
/// reuses what this server already does for a dead reliable lane: end the session loudly and let the client rejoin and
/// re-transfer, instead of holding a session that can never complete.
const MAP_TRANSFER_STALL_TERMINAL_MS: u64 = 8_000;

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Report a session whose active map transfer window has been full without progress past the terminal timeout. The
/// returned identity is the session to end; the caller performs the disconnect because it holds the state lock this
/// function does not. Any observed progress in `next_offset` restarts the timer, so a slow but living transfer on a real
/// network is never retired by this path.
pub(crate) fn note_map_transfer_stall(
    player_id: u16,
    client: &mut ClientHandle,
) -> Option<PlayerSessionIdentity> {
    note_map_transfer_stall_with_timeout(player_id, client, MAP_TRANSFER_STALL_TERMINAL_MS)
}

/// Same recovery with an explicit timeout. A timeout a test can pass is preferred to a process-wide value a test would
/// have to mutate: these suites run in one process, and a global lowered by one test would retire other tests' sessions.
pub(crate) fn note_map_transfer_stall_with_timeout(
    player_id: u16,
    client: &mut ClientHandle,
    timeout_ms: u64,
) -> Option<PlayerSessionIdentity> {
    let Some(transfer) = client.map_transfer.as_mut() else {
        return None;
    };
    let Some(active) = transfer.active.as_mut() else {
        return None;
    };
    let now_ms = unix_time_ms();
    // Window fullness is not the stall signal. A healthy transfer of a 117 KB or 362 KB file keeps all 64 window slots
    // assigned nearly every tick and refills them as acknowledgements trickle in, so a detector keyed on a full window
    // restarts its clock constantly and can never expire - measured at 137 clock restarts in one failing run, with
    // `next_offset` climbing normally through 32128, 64256 and 96384 in the same second. The signal is progress itself:
    // while any file transfer is active, `next_offset` must keep moving.
    if active.stall_since_ms == 0 || active.stall_offset != active.next_offset {
        let restarted = active.stall_since_ms != 0;
        let previous_offset = active.stall_offset;
        active.stall_offset = active.next_offset;
        active.stall_since_ms = now_ms;
        if restarted {
            tracing::debug!(
                player_id,
                file_index = active.file_index,
                previous_offset,
                next_offset = active.next_offset,
                "transfer_window_stall_clock_restarted"
            );
        }
        return None;
    }
    let stalled_ms = now_ms.saturating_sub(active.stall_since_ms);
    if stalled_ms < timeout_ms {
        return None;
    }
    tracing::warn!(
        player_id,
        file_index = active.file_index,
        next_offset = active.next_offset,
        in_flight = active.assigned_ids.len(),
        stalled_ms,
        terminal_timeout_ms = timeout_ms,
        "transfer_window_stalled_terminal"
    );
    Some(PlayerSessionIdentity::new(player_id, client.session_id))
}

pub(crate) fn resend_due_reliable_to_clients(
    state: &Arc<Mutex<ServerState>>,
    now: Instant,
) -> usize {
    let mut resent = 0;
    let mut guard = state.lock().expect("server state should not be poisoned");
    let mut stalled_sessions = Vec::new();
    for (player_id, client) in guard.clients.iter_mut() {
        for packet in client.outgoing_reliable.resend_due(now) {
            // Phase 5 simulated WebRTC loss — also drop resends so
            // the test can observe behaviour under sustained loss
            // (otherwise the retransmit always lands on the first
            // try and the reliable channel never spins).
            if !client.should_drop_simulated_tx(packet.len()) {
                let _ = client.tx.send(packet);
            }
            resent += 1;
        }
        if let Some(identity) = note_map_transfer_stall(*player_id, client) {
            stalled_sessions.push(identity);
        }
    }
    drop(guard);
    // The disconnect path takes the state lock itself, so stalled sessions are retired only after this function's own
    // guard is released. The disconnect performs the same removal, objective cleanup and transport close that any other
    // terminal session loss uses; nothing here is a softer substitute for it.
    for identity in stalled_sessions {
        crate::arena::disconnect_player_session_after_removal(state, identity, || {});
    }
    resent
}

pub(crate) fn broadcast_reliable_except(
    state: &Arc<Mutex<ServerState>>,
    except_player_id: u16,
    payload: Vec<u8>,
) {
    let mut state = state.lock().expect("server state should not be poisoned");
    for (&id, client) in &mut state.clients {
        if id == except_player_id || !client.joined {
            continue;
        }
        send_reliable_payload(client, &payload);
    }
}

pub(crate) fn broadcast_reliable_to_joined(state: &Arc<Mutex<ServerState>>, payload: Vec<u8>) {
    let mut state = state.lock().expect("server state should not be poisoned");
    for client in state.clients.values_mut().filter(|client| client.joined) {
        send_reliable_payload(client, &payload);
    }
}

/// Commit a pre-built reliable artifact batch only to the exact sessions
/// captured by the authoritative mutation. Durable score and objective-state
/// packets remain valid across a same-session life transition, but can never
/// be redirected to a replacement connection that reused the numeric id.
/// Validation and queue insertion share the state lock.
pub(crate) fn send_reliable_packets_to_session_identities(
    state: &Arc<Mutex<ServerState>>,
    recipients: &[PlayerSessionIdentity],
    payloads: &[Vec<u8>],
) {
    let mut state = state.lock().expect("server state should not be poisoned");
    for &recipient in recipients {
        if !state.player_session_matches(recipient) || !state.clients[&recipient.player_id].joined {
            continue;
        }
        let client = state
            .clients
            .get_mut(&recipient.player_id)
            .expect("matching reliable recipient should remain registered");
        for payload in payloads {
            send_reliable_payload(client, payload);
        }
    }
}

/// Send a life-scoped reliable fact only while its subject is still the exact
/// runtime incarnation captured by the authoritative mutation. The recipient
/// sessions are also validated under this lock, preventing both a stale
/// subject id and a recycled recipient id from being resolved at delivery.
pub(crate) fn send_reliable_packet_to_sessions_for_runtime(
    state: &Arc<Mutex<ServerState>>,
    subject: PlayerRuntimeIdentity,
    recipients: &[PlayerSessionIdentity],
    payload: &[u8],
) {
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state.player_runtime_matches(subject) {
        return;
    }
    for &recipient in recipients {
        if !state.player_session_matches(recipient) || !state.clients[&recipient.player_id].joined {
            continue;
        }
        let client = state
            .clients
            .get_mut(&recipient.player_id)
            .expect("matching reliable recipient should remain registered");
        send_reliable_payload(client, payload);
    }
}

#[cfg(test)]
pub(crate) fn reliable_payload(packet: &[u8]) -> Option<&[u8]> {
    reliable_parts(packet).map(|(_, payload)| payload)
}

pub(crate) fn reliable_parts(packet: &[u8]) -> Option<(u32, &[u8])> {
    (packet.len() >= RELIABLE_HEADER_SIZE
        && packet[0] == CORE_BYTE
        && packet[1] == RELIABLE_MESSAGE)
        .then(|| {
            (
                u32::from_le_bytes(
                    packet[2..RELIABLE_HEADER_SIZE]
                        .try_into()
                        .expect("slice length is checked"),
                ),
                &packet[RELIABLE_HEADER_SIZE..],
            )
        })
}

pub(crate) fn reliable_ack(id: u32) -> Vec<u8> {
    let mut packet = Vec::with_capacity(RELIABLE_HEADER_SIZE);
    packet.extend([CORE_BYTE, RELIABLE_ACK]);
    packet.extend(id.to_le_bytes());
    packet
}

pub(crate) fn reliable_ack_ids(packet: &[u8]) -> Option<Vec<u32>> {
    if packet.len() == RELIABLE_HEADER_SIZE && packet[0] == CORE_BYTE && packet[1] == RELIABLE_ACK {
        return Some(vec![u32::from_le_bytes(packet[2..6].try_into().ok()?)]);
    }
    let mut ids = Vec::new();
    for entry in packet_cluster_parts(packet)? {
        if entry.len() != RELIABLE_HEADER_SIZE || entry[0] != CORE_BYTE || entry[1] != RELIABLE_ACK
        {
            return None;
        }
        ids.push(u32::from_le_bytes(entry[2..6].try_into().ok()?));
    }
    Some(ids)
}

pub(crate) fn packet_cluster_parts(packet: &[u8]) -> Option<Vec<&[u8]>> {
    if packet.len() < 2 || packet[0] != CORE_BYTE || packet[1] != PACKET_CLUSTER {
        return None;
    }
    let mut parts = Vec::new();
    let mut offset = 2;
    while offset < packet.len() {
        let len = *packet.get(offset)? as usize;
        offset += 1;
        let entry = packet.get(offset..offset + len)?;
        offset += len;
        parts.push(entry);
    }
    Some(parts)
}

pub(crate) fn reliable(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(RELIABLE_HEADER_SIZE + payload.len());
    packet.extend([CORE_BYTE, RELIABLE_MESSAGE]);
    packet.extend(id.to_le_bytes());
    packet.extend(payload);
    packet
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn bootstrap_progress_is_bounded_and_does_not_change_readiness() {
        #[derive(Clone)]
        struct Log(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Log {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for ack_first in [false, true] {
            let log = Log(Arc::new(Mutex::new(Vec::new())));
            let writer = log.clone();
            let subscriber = tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_max_level(tracing::Level::INFO)
                .with_writer(move || writer.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                let session = PlayerSessionId::new(42);
                let stale = PlayerSessionId::new(43);
                let ids = BTreeSet::from([2, 4, 6]);
                let mut observed = BootstrapReliableBarrier::new_with_trace(
                    session,
                    ids.clone(),
                    Some(Instant::now()),
                );
                let mut plain = BootstrapReliableBarrier::new_with_trace(session, ids, None);
                for barrier in [&mut observed, &mut plain] {
                    barrier.acknowledge(stale, 2);
                    barrier.acknowledge(session, 99);
                    assert!(!barrier.mark_delivery_complete(stale));
                    if !ack_first {
                        assert!(barrier.mark_delivery_complete(session));
                    }
                    for id in [4, 2, 6] {
                        assert!(!barrier.ready());
                        barrier.acknowledge(session, id);
                        barrier.acknowledge(session, id);
                    }
                    assert_eq!(barrier.ready(), !ack_first);
                    assert!(barrier.mark_delivery_complete(session));
                    assert!(barrier.mark_delivery_complete(session));
                    assert!(barrier.ready());
                }
                assert_eq!(observed.pending_ids(), plain.pending_ids());
            });
            let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
            assert_eq!(text.lines().count(), 4, "{text}");
            for phase in ["reserved", "first_ack", "all_acked", "sink_complete"] {
                assert_eq!(
                    text.matches(&format!("phase=\"{phase}\"")).count(),
                    1,
                    "{text}"
                );
            }
            assert!(text.contains("initial_frames=3"));
            assert!(text.contains("pending_frames=0"));
        }
    }

    fn accepted_repel_event(
        source: PlayerRuntimeIdentity,
        generation: u32,
        snapshot_id: u16,
        sequence: u16,
    ) -> crate::state::ServerRepelObservation {
        crate::state::ServerRepelObservation {
            source,
            event_tick: 100,
            expires_at_tick: 325,
            origin_x: 500.0,
            origin_y: 700.0,
            expected_velocity_x: 500.0,
            expected_velocity_y: 0.0,
            weapon_data: 5,
            source_frequency: 2,
            source_ship: 0,
            delivery_generation: generation,
            active_steps: 135,
            delivered_snapshots: [None; crate::state::MAX_REPEL_DELIVERY_SNAPSHOTS],
            delivered_snapshot_cursor: 0,
            accepted_application: Some(crate::state::ServerRepelAcceptedApplication {
                snapshot_id,
                application_sequence: sequence,
                accepted_end_sequence: sequence.wrapping_add(135),
                active_steps: 135,
                receipt_reliable_ids: [None; crate::state::MAX_REPEL_RECEIPT_COPIES],
                receipt_reliable_cursor: 0,
            }),
            delivered_snapshot_id: Some(snapshot_id),
            delivered_snapshot_tick: Some(100),
            delivered_active_steps: Some(135),
            delivery_acked: true,
        }
    }

    #[test]
    fn repel_receipt_ack_serializes_fifo_and_lost_receipt_resends_idempotently() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        while rx.try_recv().is_ok() {}
        let (runtime, session) = {
            let mut locked = state.lock().unwrap();
            locked
                .clients
                .get_mut(&player_id)
                .unwrap()
                .outgoing_reliable = ServerReliableSender::default();
            let runtime = locked.active_player_runtime_identity(player_id).unwrap();
            let session = locked.active_player_session_identity(player_id).unwrap();
            for generation in 1..=3 {
                locked
                    .clients
                    .get_mut(&player_id)
                    .unwrap()
                    .pending_repel_observations
                    .push_back(accepted_repel_event(
                        runtime,
                        generation,
                        generation as u16,
                        u16::MAX.wrapping_add(generation as u16),
                    ));
            }
            locked
                .clients
                .get_mut(&player_id)
                .unwrap()
                .pending_warp_prize_relocations
                .push_back(crate::state::PendingWarpPrizeRelocation {
                    collectible_id: 77,
                    life_seq: runtime.life.life_seq,
                    expected_energy: 1_700,
                    expected_bounty: 1,
                    prize_state_revision: 1,
                    claim: crate::state::ServerPrizeClaim {
                        collectible_id: 77,
                        x: 800,
                        y: 800,
                        prize_id: 7,
                    },
                    pickup_ack_sent: false,
                });
            (runtime, session)
        };

        let receipt = crate::roster::repel_application_accepted(
            crate::wire::server_session_id(),
            1,
            1,
            1,
            0,
            135,
            135,
        );
        assert!(send_repel_application_receipt_to_runtime(
            &state,
            runtime,
            1,
            receipt.clone(),
        ));
        let first = rx.recv().unwrap();
        let (first_id, first_payload) = reliable_parts(&first).unwrap();
        assert_eq!(first_payload, receipt);
        assert_eq!(
            state.lock().unwrap().clients[&player_id]
                .pending_repel_observations
                .len(),
            3
        );

        // A duplicate 0x79 while the first receipt is in flight immediately
        // re-emits the same retained wrapper rather than minting another id
        // or waiting beyond the authoritative Repel interval.
        assert!(send_repel_application_receipt_to_runtime(
            &state,
            runtime,
            1,
            receipt.clone(),
        ));
        let prompt_retry = rx.recv().unwrap();
        assert_eq!(reliable_parts(&prompt_retry).unwrap().0, first_id);
        assert_eq!(reliable_parts(&prompt_retry).unwrap().1, receipt);

        // A still-lost receipt remains on the generic reliable timer and
        // continues to reuse the same id.
        assert_eq!(
            resend_due_reliable_to_clients(
                &state,
                Instant::now() + OUTGOING_RELIABLE_RESEND_DELAY + Duration::from_millis(1),
            ),
            1
        );
        assert_eq!(reliable_parts(&rx.recv().unwrap()).unwrap().0, first_id);

        assert!(receive_reliable_acks(
            &state,
            session,
            &reliable_ack(first_id)
        ));
        {
            let locked = state.lock().unwrap();
            let client = &locked.clients[&player_id];
            assert_eq!(client.pending_repel_observations.len(), 2);
            assert_eq!(client.pending_repel_observations[0].delivery_generation, 2);
            assert_eq!(client.retired_repel_applications.len(), 1);
        }

        // A late duplicate of A gets an idempotent receipt but that ACK must
        // not retire B because the wrapper id is not tracked on B's head.
        assert!(send_repel_application_receipt_to_runtime(
            &state, runtime, 1, receipt,
        ));
        let duplicate_id = reliable_parts(&rx.recv().unwrap()).unwrap().0;
        assert!(receive_reliable_acks(
            &state,
            session,
            &reliable_ack(duplicate_id)
        ));
        assert_eq!(
            state.lock().unwrap().clients[&player_id]
                .pending_repel_observations
                .len(),
            2
        );
        assert!(rx.try_recv().is_err(), "Warp cannot overtake B or C");

        for generation in 2..=3 {
            let sequence = generation as u16 - 1;
            let receipt = crate::roster::repel_application_accepted(
                crate::wire::server_session_id(),
                1,
                generation,
                generation as u16,
                sequence,
                sequence.wrapping_add(135),
                135,
            );
            assert!(send_repel_application_receipt_to_runtime(
                &state, runtime, generation, receipt,
            ));
            let receipt_id = reliable_parts(&rx.recv().unwrap()).unwrap().0;
            assert!(receive_reliable_acks(
                &state,
                session,
                &reliable_ack(receipt_id),
            ));
            if generation == 2 {
                assert!(rx.try_recv().is_err(), "Warp cannot overtake C");
            }
        }
        let warp_frame = rx.recv().expect("Warp ACK after complete FIFO");
        let (_, warp_payload) = reliable_parts(&warp_frame).expect("reliable Warp ACK");
        assert_eq!(warp_payload[0], 0x61);
        assert_eq!(
            u32::from_le_bytes(warp_payload[1..5].try_into().unwrap()),
            77
        );
        assert!(
            state.lock().unwrap().clients[&player_id].pending_warp_prize_relocations[0]
                .pickup_ack_sent
        );
    }

    #[test]
    fn repel_fifo_overflow_is_fail_closed_and_never_displaces_the_oldest_event() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        let mut locked = state.lock().unwrap();
        let runtime = locked.active_player_runtime_identity(player_id).unwrap();
        let client = locked.clients.get_mut(&player_id).unwrap();
        for generation in 1..=crate::state::MAX_PENDING_REPEL_EVENTS as u32 {
            assert!(client.try_enqueue_repel(accepted_repel_event(
                runtime,
                generation,
                generation as u16,
                generation as u16,
            )));
        }
        assert_eq!(
            client
                .pending_repel_observations
                .front()
                .unwrap()
                .delivery_generation,
            1
        );
        assert!(!client.try_enqueue_repel(accepted_repel_event(
            runtime,
            crate::state::MAX_PENDING_REPEL_EVENTS as u32 + 1,
            99,
            99,
        )));
        assert_eq!(
            client.pending_repel_observations.len(),
            crate::state::MAX_PENDING_REPEL_EVENTS
        );
        assert_eq!(
            client
                .pending_repel_observations
                .front()
                .unwrap()
                .delivery_generation,
            1,
            "overflow must preserve the immutable queue head"
        );
        assert_eq!(
            client
                .pending_repel_observations
                .back()
                .unwrap()
                .delivery_generation,
            crate::state::MAX_PENDING_REPEL_EVENTS as u32
        );
    }

    #[test]
    fn outgoing_reliable_tracks_ack_and_resend_timing() {
        let start = Instant::now();
        let mut sender = ServerReliableSender::default();
        let first = sender.send(b"first", start).expect("queue has room");
        let second = sender.send(b"second", start).expect("queue has room");

        assert_eq!(sender.pending_len(), 2);
        assert!(
            sender
                .resend_due(start + OUTGOING_RELIABLE_RESEND_DELAY - Duration::from_millis(1))
                .is_empty()
        );
        assert_eq!(
            sender.resend_due(start + OUTGOING_RELIABLE_RESEND_DELAY),
            vec![first.clone(), second.clone()]
        );
        assert!(sender.receive_ack(0));
        assert_eq!(sender.pending_len(), 1);
        assert!(!sender.receive_ack(0));
        assert_eq!(
            sender.resend_due(start + OUTGOING_RELIABLE_RESEND_DELAY * 2),
            vec![second]
        );
    }

    #[test]
    fn clustered_out_of_order_acks_remove_exactly_the_acked_entries() {
        // [REVIEW M7] The indexed sent-queue must behave exactly like the
        // old linear scan: an ack burst in arbitrary order removes
        // precisely the acked ids, unknown/duplicate ids are no-ops, and
        // the survivors keep resending in send order.
        let start = Instant::now();
        let mut sender = ServerReliableSender::default();
        let packets: Vec<Vec<u8>> = (0..6)
            .map(|i| {
                sender
                    .send(format!("p{i}").as_bytes(), start)
                    .expect("queue has room")
            })
            .collect();

        // Ack ids 4, 1, 3 out of order (a clustered ack burst).
        for id in [4u32, 1, 3] {
            assert!(sender.receive_ack(id), "ack {id} should remove an entry");
        }
        assert!(!sender.receive_ack(4), "duplicate ack is a no-op");
        assert!(!sender.receive_ack(99), "unknown ack is a no-op");
        assert_eq!(sender.pending_len(), 3);

        let resent = sender.resend_due(start + OUTGOING_RELIABLE_RESEND_DELAY);
        assert_eq!(
            resent,
            vec![packets[0].clone(), packets[2].clone(), packets[5].clone()],
            "survivors resend in send order"
        );
    }

    #[test]
    fn outgoing_reliable_drops_send_when_queue_full_without_advancing_id() {
        let start = Instant::now();
        let mut sender = ServerReliableSender::default();
        for _ in 0..MAX_OUTGOING_RELIABLE_QUEUE {
            assert!(sender.send(b"x", start).is_some());
        }

        assert_eq!(sender.pending_len(), MAX_OUTGOING_RELIABLE_QUEUE);
        assert!(sender.send(b"dropped", start).is_none());
        assert!(sender.send(b"also-dropped", start).is_none());
        assert_eq!(sender.pending_len(), MAX_OUTGOING_RELIABLE_QUEUE);

        // Once an ack frees a slot, the next send reuses the next sequential id with
        // no gap on the wire (id 0 was acked, next send becomes id 256).
        assert!(sender.receive_ack(0));
        let resumed = sender.send(b"after", start).expect("ack freed slot");
        assert_eq!(
            u32::from_le_bytes(resumed[2..6].try_into().unwrap()),
            MAX_OUTGOING_RELIABLE_QUEUE as u32
        );
        assert_eq!(sender.pending_len(), MAX_OUTGOING_RELIABLE_QUEUE);
    }

    #[test]
    fn bounded_application_transfer_accepts_exact_max_rejects_max_plus_one_atomically() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        let exact = vec![0x5a; crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES];
        let expected_frames = reliable_frame_count(exact.len());
        {
            let mut locked = state.lock().unwrap();
            let client = locked.clients.get_mut(&player_id).unwrap();
            assert!(send_bounded_application_transfer(client, &exact));
            assert_eq!(client.outgoing_reliable.pending_len(), expected_frames);
            let before = client.outgoing_reliable.pending_len();
            assert!(!send_bounded_application_transfer(
                client,
                &vec![0; crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES + 1]
            ));
            assert_eq!(client.outgoing_reliable.pending_len(), before);
        }
        assert_eq!(rx.try_iter().count(), expected_frames);
    }

    #[test]
    fn application_transfer_capacity_preflight_preserves_control_reserve() {
        let frames = reliable_frame_count(crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES);
        assert!(frames < MAX_OUTGOING_RELIABLE_QUEUE - APPLICATION_TRANSFER_CONTROL_RESERVE);
        let mut sender = ServerReliableSender::default();
        let existing =
            MAX_OUTGOING_RELIABLE_QUEUE - APPLICATION_TRANSFER_CONTROL_RESERVE - frames + 1;
        for _ in 0..existing {
            sender.send(b"control", Instant::now()).unwrap();
        }
        let before = sender.pending_len();
        assert!(!sender.can_retain_application_frames(frames));
        assert_eq!(
            sender.pending_len(),
            before,
            "capacity checks are non-mutating"
        );
        assert!(sender.can_retain_application_frames(frames - 1));
    }

    #[test]
    fn reliable_ack_ids_parses_single_and_clustered_acks() {
        let ack_7 = reliable_ack(7);
        let ack_8 = reliable_ack(8);
        let mut cluster = vec![CORE_BYTE, PACKET_CLUSTER, ack_7.len() as u8];
        cluster.extend_from_slice(&ack_7);
        cluster.push(ack_8.len() as u8);
        cluster.extend_from_slice(&ack_8);

        assert_eq!(reliable_ack_ids(&ack_7), Some(vec![7]));
        assert_eq!(reliable_ack_ids(&cluster), Some(vec![7, 8]));
    }

    #[test]
    fn reliable_ack_ids_rejects_mixed_clusters() {
        let mut cluster = vec![CORE_BYTE, PACKET_CLUSTER, 1, 0xff];

        assert_eq!(reliable_ack_ids(&cluster), None);
        cluster.pop();
        assert_eq!(reliable_ack_ids(&cluster), None);
    }

    #[test]
    fn packet_cluster_parts_splits_cluster_entries() {
        let first = reliable_ack(1);
        let second = reliable(2, b"payload");
        let mut cluster = vec![CORE_BYTE, PACKET_CLUSTER, first.len() as u8];
        cluster.extend_from_slice(&first);
        cluster.push(second.len() as u8);
        cluster.extend_from_slice(&second);

        assert_eq!(
            packet_cluster_parts(&cluster),
            Some(vec![first.as_slice(), second.as_slice()])
        );
    }

    #[test]
    fn packet_cluster_parts_rejects_truncated_entry() {
        assert_eq!(
            packet_cluster_parts(&[CORE_BYTE, PACKET_CLUSTER, 2, 0xff]),
            None
        );
    }

    #[test]
    fn incoming_reliable_drops_duplicate_without_reprocessing() {
        let mut receiver = ServerReliableReceiver::default();
        assert_eq!(receiver.receive(0, b"first"), vec![b"first".to_vec()]);
        assert!(receiver.receive(0, b"duplicate").is_empty());
    }

    #[test]
    fn incoming_reliable_queues_future_payloads_until_gap_arrives() {
        let mut receiver = ServerReliableReceiver::default();
        assert!(receiver.receive(1, b"second").is_empty());
        assert_eq!(
            receiver.receive(0, b"first"),
            vec![b"first".to_vec(), b"second".to_vec()]
        );
    }

    #[test]
    fn incoming_reliable_handles_id_wraparound() {
        let mut receiver = ServerReliableReceiver {
            next_process_id: u32::MAX,
            queued: BTreeMap::new(),
            small_chunk_buffer: Vec::new(),
            small_chunk_in_progress: false,
            small_chunk_fragments: 0,
        };
        assert!(receiver.receive(0, b"wrapped").is_empty());
        assert_eq!(
            receiver.receive(u32::MAX, b"last"),
            vec![b"last".to_vec(), b"wrapped".to_vec()]
        );
        assert!(receiver.receive(u32::MAX, b"duplicate").is_empty());
    }

    #[test]
    fn production_dispatch_backlog_retains_exact_gap_window_and_backpressures_fresh_ids() {
        let mut receiver = ServerReliableReceiver::default();
        for id in 1..=MAX_INCOMING_RELIABLE_QUEUE as u32 {
            assert!(!receiver.receive_to_dispatch_backlog_with_ack(id, b"suffix"));
        }
        assert!(!receiver.receive_to_dispatch_backlog_with_ack(
            MAX_INCOMING_RELIABLE_QUEUE as u32 + 1,
            b"outside-window",
        ));
        assert!(!receiver.receive_to_dispatch_backlog_with_ack(0, b"head"));
        assert_eq!(
            receiver.dispatch_backlog_len(),
            MAX_INCOMING_RELIABLE_DISPATCH_BACKLOG
        );
        assert!(!receiver.receive_to_dispatch_backlog_with_ack(
            MAX_INCOMING_RELIABLE_QUEUE as u32 + 1,
            b"blocked-fresh",
        ));
        let head = receiver.take_dispatch_head().unwrap();
        assert_eq!(head.delivery_id, 0);
        receiver.restore_dispatch_head(head);
        assert_eq!(receiver.dispatch_backlog_len(), 257);
    }

    #[test]
    fn production_cursor_and_ack_eligibility_advance_only_after_ordered_commit_across_wrap() {
        let mut receiver = ServerReliableReceiver::default();
        receiver.set_next_process_id_for_test(u32::MAX);
        assert!(!receiver.receive_to_dispatch_backlog_with_ack(0, b"suffix"));
        assert!(!receiver.receive_to_dispatch_backlog_with_ack(u32::MAX, b"head"));
        assert!(!receiver.receive_to_dispatch_backlog_with_ack(0, b"suffix-retry"));

        let head = receiver.take_dispatch_head().unwrap();
        assert_eq!(head.delivery_id, u32::MAX);
        assert_eq!(
            receiver.prepare_dispatch_head(&head).unwrap().payload,
            b"head"
        );
        receiver.restore_dispatch_head(head);
        assert!(!receiver.receive_to_dispatch_backlog_with_ack(u32::MAX, b"head-retry"));

        let head = receiver.take_dispatch_head().unwrap();
        receiver.commit_dispatch_head(&head);
        assert!(receiver.receive_to_dispatch_backlog_with_ack(u32::MAX, b"committed-duplicate"));
        assert!(!receiver.receive_to_dispatch_backlog_with_ack(0, b"resident-suffix"));
        let suffix = receiver.take_dispatch_head().unwrap();
        assert_eq!(suffix.delivery_id, 0);
        receiver.commit_dispatch_head(&suffix);
        assert!(receiver.receive_to_dispatch_backlog_with_ack(0, b"committed-suffix-duplicate"));

        let oversized = vec![0xaa; MAX_RELIABLE_PAYLOAD_SIZE + 1];
        assert!(!receiver.receive_to_dispatch_backlog_with_ack(1, &oversized));
        let tombstone = receiver.take_dispatch_head().unwrap();
        assert!(tombstone.payload.is_empty());
        receiver.commit_dispatch_head(&tombstone);
        assert!(receiver.receive_to_dispatch_backlog_with_ack(1, &oversized));
    }

    #[test]
    fn incoming_reliable_reassembles_small_chunks() {
        let mut receiver = ServerReliableReceiver::default();
        assert!(
            receiver
                .receive(0, &[CORE_BYTE, SMALL_CHUNK_BODY, b'a', b'b'])
                .is_empty()
        );
        assert_eq!(
            receiver.receive(1, &[CORE_BYTE, SMALL_CHUNK_TAIL, b'c']),
            vec![b"abc".to_vec()]
        );
        assert_eq!(receiver.small_chunk_buffer.capacity(), 0);
    }

    #[test]
    fn incoming_reliable_reassembles_queued_small_chunks_in_order() {
        let mut receiver = ServerReliableReceiver::default();
        assert!(
            receiver
                .receive(1, &[CORE_BYTE, SMALL_CHUNK_TAIL, b'c'])
                .is_empty()
        );
        assert_eq!(
            receiver.receive(0, &[CORE_BYTE, SMALL_CHUNK_BODY, b'a', b'b']),
            vec![b"abc".to_vec()]
        );
    }

    #[test]
    fn incoming_reliable_drops_orphan_small_chunk_tail() {
        let mut receiver = ServerReliableReceiver::default();
        // A TAIL with no preceding BODY must not be flushed as a logical payload.
        assert!(
            receiver
                .receive(0, &[CORE_BYTE, SMALL_CHUNK_TAIL, b'z'])
                .is_empty()
        );
        // Subsequent legitimate chunks should still reassemble correctly.
        assert!(
            receiver
                .receive(1, &[CORE_BYTE, SMALL_CHUNK_BODY, b'a', b'b'])
                .is_empty()
        );
        assert_eq!(
            receiver.receive(2, &[CORE_BYTE, SMALL_CHUNK_TAIL, b'c']),
            vec![b"abc".to_vec()]
        );
    }

    #[test]
    fn incoming_reliable_aborts_small_chunk_at_unrelated_logical_payload() {
        let mut receiver = ServerReliableReceiver::default();
        assert!(
            receiver
                .receive(0, &[CORE_BYTE, SMALL_CHUNK_BODY, b'a'])
                .is_empty()
        );
        assert_eq!(
            receiver.receive(1, b"unrelated"),
            vec![b"unrelated".to_vec()]
        );
        assert!(
            receiver
                .receive(2, &[CORE_BYTE, SMALL_CHUNK_TAIL, b'b'])
                .is_empty(),
            "a tail cannot complete bytes from before an unrelated reliable payload"
        );
        assert!(receiver.small_chunk_buffer.is_empty());
        assert_eq!(receiver.small_chunk_buffer.capacity(), 0);
        assert!(!receiver.small_chunk_in_progress);
        assert_eq!(receiver.small_chunk_fragments, 0);
    }

    #[test]
    fn incoming_reliable_rejects_empty_small_fragments_and_recovers() {
        let mut receiver = ServerReliableReceiver::default();
        assert!(
            receiver
                .receive(0, &[CORE_BYTE, SMALL_CHUNK_BODY])
                .is_empty()
        );
        assert!(
            receiver
                .receive(1, &[CORE_BYTE, SMALL_CHUNK_TAIL, b'x'])
                .is_empty()
        );
        assert!(
            receiver
                .receive(2, &[CORE_BYTE, SMALL_CHUNK_BODY, b'a'])
                .is_empty()
        );
        assert!(
            receiver
                .receive(3, &[CORE_BYTE, SMALL_CHUNK_TAIL])
                .is_empty()
        );
        assert!(
            receiver
                .receive(4, &[CORE_BYTE, SMALL_CHUNK_TAIL, b'b'])
                .is_empty()
        );
        assert!(
            receiver
                .receive(5, &[CORE_BYTE, SMALL_CHUNK_BODY, b'o'])
                .is_empty()
        );
        assert_eq!(
            receiver.receive(6, &[CORE_BYTE, SMALL_CHUNK_TAIL, b'k']),
            vec![b"ok".to_vec()]
        );
    }

    #[test]
    fn incoming_small_chunk_byte_budget_accepts_exact_max_and_clears_max_plus_one() {
        let mut receiver = ServerReliableReceiver::default();
        receiver.small_chunk_in_progress = true;
        assert!(receiver.push_small_chunk(&vec![0x5a; MAX_INCOMING_SMALL_CHUNK_BYTES]));
        assert_eq!(
            receiver.small_chunk_buffer.len(),
            MAX_INCOMING_SMALL_CHUNK_BYTES
        );
        assert!(!receiver.push_small_chunk(&[0x5b]));
        assert!(receiver.small_chunk_buffer.is_empty());
        assert_eq!(receiver.small_chunk_buffer.capacity(), 0);
        assert!(!receiver.small_chunk_in_progress);
        assert_eq!(receiver.small_chunk_fragments, 0);
    }

    #[test]
    fn incoming_small_chunk_fragment_budget_accepts_exact_max_and_clears_max_plus_one() {
        let mut receiver = ServerReliableReceiver::default();
        receiver.small_chunk_in_progress = true;
        for _ in 0..MAX_INCOMING_SMALL_CHUNK_FRAGMENTS {
            assert!(receiver.push_small_chunk(&[0x5a]));
        }
        assert_eq!(
            receiver.small_chunk_fragments,
            MAX_INCOMING_SMALL_CHUNK_FRAGMENTS
        );
        assert!(!receiver.push_small_chunk(&[0x5b]));
        assert!(receiver.small_chunk_buffer.is_empty());
        assert!(!receiver.small_chunk_in_progress);
        assert_eq!(receiver.small_chunk_fragments, 0);
    }

    #[test]
    fn incoming_reliable_rejects_ids_too_far_ahead_of_next_process_id() {
        let mut receiver = ServerReliableReceiver::default();
        let too_far = MAX_INCOMING_RELIABLE_QUEUE as u32 + 1;
        assert!(receiver.receive(too_far, b"unreachable").is_empty());
        assert!(!receiver.queued.contains_key(&too_far));

        let at_limit = MAX_INCOMING_RELIABLE_QUEUE as u32;
        assert!(receiver.receive(at_limit, b"reachable").is_empty());
        assert!(receiver.queued.contains_key(&at_limit));
    }

    #[test]
    fn incoming_reliable_caps_future_queue_without_evicting_earlier_gaps() {
        let mut receiver = ServerReliableReceiver::default();
        for id in 1..=300 {
            assert!(receiver.receive(id, b"x").is_empty());
        }

        assert_eq!(receiver.queued.len(), MAX_INCOMING_RELIABLE_QUEUE);
        assert!(receiver.queued.contains_key(&1));
        assert!(
            receiver
                .queued
                .contains_key(&(MAX_INCOMING_RELIABLE_QUEUE as u32))
        );
        assert!(
            !receiver
                .queued
                .contains_key(&(MAX_INCOMING_RELIABLE_QUEUE as u32 + 1))
        );
    }

    #[test]
    fn incoming_reliable_withholds_ack_until_full_queue_can_retain_retry() {
        let mut receiver = ServerReliableReceiver::default();
        for id in 1..=MAX_INCOMING_RELIABLE_QUEUE as u32 {
            let (acknowledge, ordered) = receiver.receive_tagged_with_ack(id, b"queued");
            assert!(acknowledge, "retained future id {id} must be ACKed");
            assert!(ordered.is_empty());
        }

        let overflow_id = MAX_INCOMING_RELIABLE_QUEUE as u32 + 1;
        let (acknowledge, ordered) = receiver.receive_tagged_with_ack(overflow_id, b"retry-me");
        assert!(!acknowledge, "an unretained wrapper must not be ACKed");
        assert!(ordered.is_empty());

        let (acknowledge, ordered) = receiver.receive_tagged_with_ack(0, b"gap");
        assert!(acknowledge);
        assert_eq!(ordered.len(), MAX_INCOMING_RELIABLE_QUEUE + 1);
        assert_eq!(receiver.next_process_id, overflow_id);

        let (acknowledge, ordered) = receiver.receive_tagged_with_ack(overflow_id, b"retry-me");
        assert!(acknowledge, "retry is ACKed after it can be retained");
        assert_eq!(
            ordered,
            vec![ReceivedReliablePayload {
                delivery_id: overflow_id,
                payload: b"retry-me".to_vec(),
            }]
        );
    }
}
