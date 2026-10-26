#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::thread;
use std::time::{Duration, Instant};

use bytes::BytesMut;
use tokio::sync::{Notify, watch};
use webrtc::data_channel::{
    DataChannel, DataChannelEvent, RTCDataChannelInit, RTCDataChannelState,
};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfiguration,
    RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer,
    RTCIceTransportPolicy as WebRtcIceTransportPolicy, RTCPeerConnectionState,
    RTCSessionDescription, RTCStatsReportEntry, SettingEngineBuilder, StatsSelector,
};
use webrtc::runtime::TokioRuntime;

use crate::arena::disconnect_player_session;
#[cfg(test)]
use crate::arena::register_connection_with_sender;
use crate::authority_identity::{PlayerRuntimeIdentity, PlayerSessionIdentity};
use crate::config::{RtcIceTransportPolicy, ServerConfig};
use crate::connection::handle_client_packet_for_session;
use crate::state::{ClientPacketSender, ClientPacketSink, RtcOverloadMetrics, ServerState};
use tracing::{debug, info, warn};

// Per-event lines log under this module; see `log_targets`.
const _: () = assert!(crate::log_targets::is_child_of(
    crate::log_targets::RTC_ACKS,
    module_path!()
));

#[path = "rtc/writer.rs"]
mod writer;

static NEXT_RTC_UDP_PORT: AtomicUsize = AtomicUsize::new(0);
static RTC_UDP_PORT_LIFECYCLES: OnceLock<Mutex<HashMap<u16, Weak<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();
const RTC_PEER_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const RTC_UDP_REUSE_TIMEOUT: Duration = Duration::from_millis(250);
const RTC_UDP_RECOVERY_INTERVAL: Duration = Duration::from_secs(1);
const RTC_UDP_RECOVERY_ATTEMPTS_PER_CYCLE: usize = 60;
const RTC_UDP_RECOVERY_INITIAL_BACKOFF: Duration = Duration::from_secs(5);
const RTC_UDP_RECOVERY_MAX_BACKOFF: Duration = Duration::from_secs(60);
/// A registered peer holds a player id and one of the bounded UDP ports, so it
/// must log in. The browser sends its login as soon as the channels open, but
/// retries an in-use identity on the same connection for up to 45 s
/// (`IdentityLoginRetry`), so the deadline stays well beyond that.
const RTC_LOGIN_DEADLINE: Duration = Duration::from_secs(90);
pub(crate) const RTC_DATA_CHANNEL_SEND_BUFFER_BYTES: usize = 256 * 1024;
pub(crate) const RTC_APPLICATION_RELIABLE_QUEUE_PACKETS: usize = 4_096;
pub(crate) const RTC_APPLICATION_QUEUE_BYTES: usize = 4 * 1024 * 1024;
/// Drain the WebRTC implementation's small internal event queue into a
/// session-owned application queue before packet admission does any locking.
/// A bounded overflow is explicit and disconnects the session; reliable
/// gameplay is therefore retried by a fresh session rather than silently
/// dropped by the DataChannel driver's `try_send(...): Full` path.
pub(crate) const RTC_APPLICATION_INBOUND_QUEUE_PACKETS: usize = 4_096;
/// Client intent/control packets are small; dense 64 KiB snapshots travel in
/// the opposite direction. Admit bounded rolling input plus marker overhead.
pub(crate) const RTC_APPLICATION_INBOUND_MESSAGE_BYTES: usize = 2_048;
pub(crate) const RTC_APPLICATION_INBOUND_QUEUE_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RtcSignalingErrorKind {
    InvalidOffer,
    CapacityUnavailable,
    ServiceUnavailable,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RtcSignalingError {
    kind: RtcSignalingErrorKind,
    public_message: &'static str,
    detail: String,
}

impl RtcSignalingError {
    pub(crate) fn invalid_offer(detail: impl Into<String>) -> Self {
        Self {
            kind: RtcSignalingErrorKind::InvalidOffer,
            public_message: "Provide a valid JSON WebRTC offer session description.",
            detail: detail.into(),
        }
    }

    pub(crate) fn capacity_unavailable(detail: impl Into<String>) -> Self {
        Self {
            kind: RtcSignalingErrorKind::CapacityUnavailable,
            public_message: "WebRTC session capacity is temporarily unavailable; retry shortly.",
            detail: detail.into(),
        }
    }

    pub(crate) fn service_unavailable(detail: impl Into<String>) -> Self {
        Self {
            kind: RtcSignalingErrorKind::ServiceUnavailable,
            public_message: "WebRTC transport is temporarily unavailable; retry after UDP/ICE service recovery.",
            detail: detail.into(),
        }
    }

    pub(crate) fn internal(detail: impl Into<String>) -> Self {
        Self {
            kind: RtcSignalingErrorKind::Internal,
            public_message: "WebRTC signaling failed due to an internal server error.",
            detail: detail.into(),
        }
    }

    pub(crate) fn kind(&self) -> RtcSignalingErrorKind {
        self.kind
    }

    pub(crate) fn status(&self) -> &'static str {
        match self.kind {
            RtcSignalingErrorKind::InvalidOffer => "400 Bad Request",
            RtcSignalingErrorKind::CapacityUnavailable
            | RtcSignalingErrorKind::ServiceUnavailable => "503 Service Unavailable",
            RtcSignalingErrorKind::Internal => "500 Internal Server Error",
        }
    }

    pub(crate) fn code(&self) -> &'static str {
        match self.kind {
            RtcSignalingErrorKind::InvalidOffer => "invalid_webrtc_offer",
            RtcSignalingErrorKind::CapacityUnavailable => "rtc_capacity_unavailable",
            RtcSignalingErrorKind::ServiceUnavailable => "rtc_service_unavailable",
            RtcSignalingErrorKind::Internal => "internal_server_error",
        }
    }

    pub(crate) fn public_message(&self) -> &'static str {
        self.public_message
    }

    pub(crate) fn retry_after_seconds(&self) -> Option<&'static str> {
        matches!(
            self.kind,
            RtcSignalingErrorKind::CapacityUnavailable | RtcSignalingErrorKind::ServiceUnavailable
        )
        .then_some("1")
    }
}

impl std::fmt::Display for RtcSignalingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.detail)
    }
}

impl std::error::Error for RtcSignalingError {}

#[derive(Clone)]
struct FixedRtcUdpPort {
    port: u16,
    lifecycle: Arc<tokio::sync::Mutex<()>>,
}

struct RtcUdpBinding {
    address: String,
    fixed: Option<FixedRtcUdpPort>,
}

impl RtcUdpBinding {
    fn new(address: String) -> Self {
        let fixed = address
            .parse::<std::net::SocketAddr>()
            .ok()
            .filter(|address| address.port() != 0)
            .map(|address| FixedRtcUdpPort {
                port: address.port(),
                lifecycle: rtc_udp_port_lifecycle(address.port()),
            });
        Self { address, fixed }
    }
}

fn rtc_udp_port_lifecycle(port: u16) -> Arc<tokio::sync::Mutex<()>> {
    let registry = RTC_UDP_PORT_LIFECYCLES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry
        .lock()
        .expect("RTC UDP port lifecycle registry should not be poisoned");
    if let Some(lifecycle) = registry.get(&port).and_then(Weak::upgrade) {
        return lifecycle;
    }
    let lifecycle = Arc::new(tokio::sync::Mutex::new(()));
    registry.insert(port, Arc::downgrade(&lifecycle));
    lifecycle
}

fn rtc_udp_bind_addresses(config: &ServerConfig) -> Vec<String> {
    let Some(range) = config.rtc_udp_port_range else {
        return vec![config.rtc_udp_bind_address(0).to_string()];
    };
    let count = range.capacity();
    let start = NEXT_RTC_UDP_PORT.fetch_add(1, Ordering::Relaxed) % count;
    (0..count)
        .map(|offset| usize::from(range.min) + (start + offset) % count)
        .map(|port| config.rtc_udp_bind_address(port as u16).to_string())
        .collect()
}

fn rtc_configuration(config: &ServerConfig) -> RTCConfiguration {
    let ice_servers = if config.rtc_ice_urls.is_empty() {
        Vec::new()
    } else {
        vec![RTCIceServer {
            urls: config.rtc_ice_urls.clone(),
            username: config.rtc_ice_username.clone(),
            credential: config.rtc_ice_credential.clone(),
        }]
    };
    let policy = match config.rtc_ice_transport_policy {
        RtcIceTransportPolicy::All => WebRtcIceTransportPolicy::All,
        RtcIceTransportPolicy::Relay => WebRtcIceTransportPolicy::Relay,
    };
    RTCConfigurationBuilder::new()
        .with_ice_servers(ice_servers)
        .with_ice_transport_policy(policy)
        .build()
}

pub(crate) const GAME_CHANNEL_LABEL: &str = "zubspace-game";
pub(crate) const EVENT_CHANNEL_LABEL: &str = "zubspace-events";
pub(crate) const RELIABLE_CHANNEL_LABEL: &str = "zubspace-reliable";
pub(crate) const GAME_CHANNEL_MAX_PACKET_LIFETIME_MS: u16 = 80;
const RTC_OUTBOUND_DRAIN_LIMIT: usize = 256;

pub(crate) fn game_data_channel_init() -> RTCDataChannelInit {
    RTCDataChannelInit {
        ordered: false,
        max_packet_life_time: Some(GAME_CHANNEL_MAX_PACKET_LIFETIME_MS),
        ..Default::default()
    }
}

pub(crate) fn reliable_data_channel_init() -> RTCDataChannelInit {
    RTCDataChannelInit {
        ordered: true,
        ..Default::default()
    }
}

pub(crate) fn event_data_channel_init() -> RTCDataChannelInit {
    RTCDataChannelInit {
        ordered: false,
        ..Default::default()
    }
}

pub(crate) fn signaling_capabilities_json() -> &'static str {
    r#"{"ok":true,"transport":"webrtc-datachannel","status":"signaling-ready","offer_endpoint":"POST /v1/rtc/offer","max_data_channels":3,"data_channel_send_buffer_bytes":262144,"inbound_queue_packets_per_channel":4096,"inbound_queue_bytes_per_channel":262144,"inbound_message_bytes":2048,"outbound_reliable_queue_packets":4096,"outbound_queue_bytes":4194304,"outbound_lane_queue_packets":4096,"outbound_lane_queue_bytes":4194304,"game_channel":{"label":"zubspace-game","ordered":false,"maxPacketLifeTime":80},"event_channel":{"label":"zubspace-events","ordered":false},"reliable_channel":{"label":"zubspace-reliable","ordered":true}}"#
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RtcChannelKind {
    Game,
    Event,
    Reliable,
}

impl RtcChannelKind {
    const fn metric_index(self) -> usize {
        match self {
            Self::Game => 0,
            Self::Event => 1,
            Self::Reliable => 2,
        }
    }
}

struct RtcInboundQueue {
    sender: tokio::sync::mpsc::Sender<Vec<u8>>,
    metrics: Arc<RtcOverloadMetrics>,
    kind: RtcChannelKind,
    queued_bytes: Arc<AtomicUsize>,
    byte_limit: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RtcInboundEnqueueError {
    Full,
    ByteLimit,
    Closed,
    Oversized,
}

impl RtcInboundQueue {
    fn with_capacity(
        metrics: Arc<RtcOverloadMetrics>,
        kind: RtcChannelKind,
        capacity: usize,
    ) -> (Self, RtcInboundReceiver) {
        Self::with_limits(metrics, kind, capacity, RTC_APPLICATION_INBOUND_QUEUE_BYTES)
    }

    fn with_limits(
        metrics: Arc<RtcOverloadMetrics>,
        kind: RtcChannelKind,
        capacity: usize,
        byte_limit: usize,
    ) -> (Self, RtcInboundReceiver) {
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity);
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        (
            Self {
                sender,
                metrics: Arc::clone(&metrics),
                kind,
                queued_bytes: Arc::clone(&queued_bytes),
                byte_limit,
            },
            RtcInboundReceiver {
                receiver,
                metrics,
                kind,
                queued_bytes,
            },
        )
    }

    fn try_enqueue(&self, packet: Vec<u8>) -> Result<(), RtcInboundEnqueueError> {
        let bytes = packet.len();
        if bytes > RTC_APPLICATION_INBOUND_MESSAGE_BYTES {
            return Err(RtcInboundEnqueueError::Oversized);
        }
        if self
            .queued_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |queued| {
                queued
                    .checked_add(bytes)
                    .filter(|sum| *sum <= self.byte_limit)
            })
            .is_err()
        {
            self.metrics
                .record_inbound_backpressure(self.kind.metric_index());
            return Err(RtcInboundEnqueueError::ByteLimit);
        }
        // Account before publishing to the receiver: it may dequeue on
        // another runtime worker immediately after `try_send` succeeds.
        let queued = self.metrics.add_inbound_queue(self.kind.metric_index());
        match self.sender.try_send(packet) {
            Ok(()) => {
                self.metrics
                    .record_inbound_queue_peak(self.kind.metric_index(), queued);
                Ok(())
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
                self.metrics.remove_inbound_queue(self.kind.metric_index());
                self.metrics
                    .record_inbound_backpressure(self.kind.metric_index());
                Err(RtcInboundEnqueueError::Full)
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
                self.metrics.remove_inbound_queue(self.kind.metric_index());
                Err(RtcInboundEnqueueError::Closed)
            }
        }
    }
}

/// Owns production receive-side accounting even if a handler panics or its
/// async caller is cancelled while a worker owns this receiver.
struct RtcInboundReceiver {
    receiver: tokio::sync::mpsc::Receiver<Vec<u8>>,
    metrics: Arc<RtcOverloadMetrics>,
    kind: RtcChannelKind,
    queued_bytes: Arc<AtomicUsize>,
}

impl RtcInboundReceiver {
    fn account_pop(&self, bytes: usize) {
        self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
        self.metrics.remove_inbound_queue(self.kind.metric_index());
        self.metrics
            .record_inbound_packet(self.kind.metric_index(), bytes);
    }

    async fn recv(&mut self) -> Option<Vec<u8>> {
        let packet = self.receiver.recv().await?;
        self.account_pop(packet.len());
        Some(packet)
    }

    fn try_recv(&mut self) -> Option<Vec<u8>> {
        let packet = self.receiver.try_recv().ok()?;
        self.account_pop(packet.len());
        Some(packet)
    }
}

impl Drop for RtcInboundReceiver {
    fn drop(&mut self) {
        self.receiver.close();
        while let Ok(packet) = self.receiver.try_recv() {
            self.queued_bytes.fetch_sub(packet.len(), Ordering::AcqRel);
            self.metrics.remove_inbound_queue(self.kind.metric_index());
        }
    }
}

// Scheduling fairness bound, not a gameplay delay. Never wait for more packets
// or prefetch a batch outside the channel's existing bounded queue.
const RTC_INGRESS_DRAIN_LIMIT: usize = 64;

fn drain_rtc_ingress_turn(
    first: Vec<u8>,
    receiver: &mut RtcInboundReceiver,
    mut dispatch: impl FnMut(Vec<u8>),
) -> usize {
    dispatch(first);
    let mut count = 1;
    while count < RTC_INGRESS_DRAIN_LIMIT {
        let Some(packet) = receiver.try_recv() else {
            break;
        };
        dispatch(packet);
        count += 1;
    }
    count
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RtcChannelMetadata {
    label: String,
    ordered: bool,
    max_packet_life_time: Option<u16>,
    max_retransmits: Option<u16>,
    protocol: String,
    negotiated: bool,
}

#[derive(Default)]
struct RtcChannelProtocol {
    announced: usize,
    game_announced: bool,
    event_announced: bool,
    reliable_announced: bool,
    game_open: bool,
    event_open: bool,
    reliable_open: bool,
}

impl RtcChannelProtocol {
    fn announce(&mut self, metadata: &RtcChannelMetadata) -> Result<RtcChannelKind, String> {
        self.announced = self.announced.saturating_add(1);
        if self.announced > 3 {
            return Err(format!(
                "excessive DataChannels: received {}, maximum is 3",
                self.announced
            ));
        }
        if metadata.protocol != "" || metadata.negotiated {
            return Err(format!(
                "channel {} must use the in-band empty subprotocol",
                metadata.label
            ));
        }
        let (kind, announced, expected_ordered, expected_lifetime) = match metadata.label.as_str() {
            GAME_CHANNEL_LABEL => (
                RtcChannelKind::Game,
                &mut self.game_announced,
                false,
                Some(GAME_CHANNEL_MAX_PACKET_LIFETIME_MS),
            ),
            EVENT_CHANNEL_LABEL => (
                RtcChannelKind::Event,
                &mut self.event_announced,
                false,
                None,
            ),
            RELIABLE_CHANNEL_LABEL => (
                RtcChannelKind::Reliable,
                &mut self.reliable_announced,
                true,
                None,
            ),
            _ => return Err(format!("unknown DataChannel label {}", metadata.label)),
        };
        if *announced {
            return Err(format!("duplicate DataChannel label {}", metadata.label));
        }
        if metadata.ordered != expected_ordered
            || metadata.max_packet_life_time != expected_lifetime
            || metadata.max_retransmits.is_some()
        {
            return Err(format!(
                "unsupported DataChannel attributes for {}: ordered={}, lifetime={:?}, retransmits={:?}",
                metadata.label,
                metadata.ordered,
                metadata.max_packet_life_time,
                metadata.max_retransmits
            ));
        }
        *announced = true;
        Ok(kind)
    }

    fn mark_open(&mut self, kind: RtcChannelKind) -> bool {
        match kind {
            RtcChannelKind::Game => self.game_open = true,
            RtcChannelKind::Event => self.event_open = true,
            RtcChannelKind::Reliable => self.reliable_open = true,
        }
        self.all_open()
    }

    fn all_open(&self) -> bool {
        self.game_announced
            && self.event_announced
            && self.reliable_announced
            && self.game_open
            && self.event_open
            && self.reliable_open
    }
}

#[derive(Default)]
struct RtcChannels {
    protocol: RtcChannelProtocol,
    game: Option<Arc<dyn DataChannel>>,
    event: Option<Arc<dyn DataChannel>>,
    reliable: Option<Arc<dyn DataChannel>>,
}

#[derive(Default)]
struct RtcOutboundQueueState {
    reliable: VecDeque<Vec<u8>>,
    /// Server-reliable ids currently owned by either the queued FIFO or the
    /// writer's bounded in-flight batch. The legacy retransmit timer can fire
    /// while a large file's original frames are still waiting for SCTP send
    /// capacity; coalescing that byte-identical retry prevents transport
    /// scheduling delay from being misclassified as 4,096 distinct packets.
    active_reliable_ids: HashSet<u32>,
    realtime: Option<Vec<u8>>,
    bytes: usize,
    closed: bool,
    reliable_overflow: bool,
}

enum RtcOutboundReceive {
    Packets(Vec<Vec<u8>>),
    ReliableOverflow,
    Closed,
}

struct RtcOutboundQueue {
    quality: Arc<Mutex<crate::connection_quality::SessionConnectionQuality>>,
    state: Mutex<RtcOutboundQueueState>,
    available: Condvar,
    metrics: Arc<RtcOverloadMetrics>,
    reliable_packet_limit: usize,
    byte_limit: usize,
    overflow_cleanup: Mutex<Option<(Weak<RtcSession>, tokio::runtime::Handle)>>,
    lane_writer: Mutex<Option<Weak<writer::LaneWriter>>>,
}

impl RtcOutboundQueue {
    fn new(metrics: Arc<RtcOverloadMetrics>) -> Self {
        Self::with_limits(
            metrics,
            RTC_APPLICATION_RELIABLE_QUEUE_PACKETS,
            RTC_APPLICATION_QUEUE_BYTES,
        )
    }

    fn with_limits(
        metrics: Arc<RtcOverloadMetrics>,
        reliable_packet_limit: usize,
        byte_limit: usize,
    ) -> Self {
        Self {
            state: Mutex::new(RtcOutboundQueueState::default()),
            quality: Arc::new(Mutex::new(
                crate::connection_quality::SessionConnectionQuality::new(Instant::now()),
            )),
            available: Condvar::new(),
            metrics,
            reliable_packet_limit,
            byte_limit,
            overflow_cleanup: Mutex::new(None),
            lane_writer: Mutex::new(None),
        }
    }

    fn attach_session(&self, session: Weak<RtcSession>, handle: tokio::runtime::Handle) {
        *self
            .overflow_cleanup
            .lock()
            .expect("RTC overflow cleanup lock should not be poisoned") = Some((session, handle));
    }

    fn attach_lane_writer(&self, lanes: &Arc<writer::LaneWriter>) {
        let state = self.state.lock().expect("RTC outbound queue");
        *self.lane_writer.lock().expect("RTC lane writer") = Some(Arc::downgrade(lanes));
        if state.closed {
            lanes.close();
        }
    }

    fn close_lane_writer(&self) {
        if let Some(lanes) = self
            .lane_writer
            .lock()
            .expect("RTC lane writer")
            .as_ref()
            .and_then(Weak::upgrade)
        {
            lanes.close();
        }
    }

    fn trigger_session_cleanup(&self) {
        let cleanup = self
            .overflow_cleanup
            .lock()
            .expect("RTC overflow cleanup lock should not be poisoned")
            .take();
        let Some((session, handle)) = cleanup else {
            return;
        };
        handle.spawn_blocking(move || {
            if let Some(session) = session.upgrade() {
                disconnect_rtc_session(&session);
            }
        });
    }

    fn fail_reliable_overflow(&self) {
        let mut state = self
            .state
            .lock()
            .expect("RTC outbound queue should not be poisoned");
        if state.closed {
            return;
        }
        let abandoned_packets = state.reliable.len() + usize::from(state.realtime.is_some());
        let abandoned_bytes = state.bytes;
        state.reliable.clear();
        state.active_reliable_ids.clear();
        state.realtime = None;
        state.bytes = 0;
        state.closed = true;
        state.reliable_overflow = true;
        self.metrics
            .remove_queue(abandoned_packets, abandoned_bytes);
        self.metrics.record_reliable_overflow();
        self.available.notify_all();
        drop(state);
        self.close_lane_writer();
        self.trigger_session_cleanup();
    }

    fn enqueue(&self, packet: Vec<u8>) -> Result<(), ()> {
        let mut state = self
            .state
            .lock()
            .expect("RTC outbound queue should not be poisoned");
        if state.closed {
            return Err(());
        }
        if replaceable_realtime_server_packet(&packet) {
            let previous_len = state.realtime.as_ref().map_or(0, Vec::len);
            let reliable_bytes = state.bytes.saturating_sub(previous_len);
            if reliable_bytes.saturating_add(packet.len()) > self.byte_limit {
                if state.realtime.take().is_some() {
                    state.bytes = state.bytes.saturating_sub(previous_len);
                    self.metrics.remove_queue(1, previous_len);
                }
                self.metrics.record_realtime_coalesced();
                return Ok(());
            }
            if let Some(previous) = state.realtime.replace(packet) {
                let replacement_len = state.realtime.as_ref().map_or(0, Vec::len);
                state.bytes = state
                    .bytes
                    .saturating_sub(previous.len())
                    .saturating_add(replacement_len);
                self.metrics
                    .replace_queue_bytes(previous.len(), replacement_len);
                self.metrics.record_realtime_coalesced();
            } else {
                let bytes = state.realtime.as_ref().map_or(0, Vec::len);
                state.bytes = state.bytes.saturating_add(bytes);
                self.metrics.add_queue(1, bytes);
            }
            self.available.notify_one();
            return Ok(());
        }

        let reliable_id = crate::reliable::reliable_parts(&packet).map(|(id, _)| id);
        if reliable_id.is_some_and(|id| state.active_reliable_ids.contains(&id)) {
            // This exact application-level retransmit is already queued or in
            // the writer batch. The reliable DataChannel preserves it; a
            // later timer may retry again after the current send completes if
            // its ACK still has not arrived.
            return Ok(());
        }

        let over_packets = state.reliable.len() >= self.reliable_packet_limit;
        let over_bytes = state.bytes.saturating_add(packet.len()) > self.byte_limit;
        if over_packets || over_bytes {
            if packet.first()
                == Some(&crate::connection_quality_protocol::ConnectionQualityProbe::TYPE)
            {
                // Optional measurement traffic must never turn queue pressure
                // into a reliable-lifecycle disconnect or a path-loss sample.
                return Err(());
            }
            drop(state);
            self.fail_reliable_overflow();
            return Err(());
        }
        state.bytes = state.bytes.saturating_add(packet.len());
        self.metrics.add_queue(1, packet.len());
        if let Some(id) = reliable_id {
            state.active_reliable_ids.insert(id);
        }
        state.reliable.push_back(packet);
        self.available.notify_one();
        Ok(())
    }

    /// Queues a tick's `0x7d` events under one lock and wakes the writer once,
    /// so its next receive holds all of them and can send them as one batch.
    /// They are neither replaceable nor application-reliable, so only the
    /// queue limits apply.
    fn enqueue_events(&self, packets: Vec<Vec<u8>>) -> Result<(), ()> {
        debug_assert!(
            packets
                .iter()
                .all(|packet| weapon_event_server_packet(packet))
        );
        let mut state = self
            .state
            .lock()
            .expect("RTC outbound queue should not be poisoned");
        if state.closed {
            return Err(());
        }
        let bytes = packets.iter().map(Vec::len).sum::<usize>();
        if state.reliable.len() + packets.len() > self.reliable_packet_limit
            || state.bytes.saturating_add(bytes) > self.byte_limit
        {
            drop(state);
            self.fail_reliable_overflow();
            return Err(());
        }
        state.bytes += bytes;
        self.metrics.add_queue(packets.len(), bytes);
        state.reliable.extend(packets);
        self.available.notify_one();
        Ok(())
    }

    /// Release a frame that this queue handed to the writer but that never reached a channel, so a later retransmit timer
    /// can actually re-queue it. The dedupe guard deliberately covers "queued or in the writer batch", which is correct
    /// only while the writer still holds the frame: when the send fails, or no lane is open for the route, the id stayed in
    /// the set until the acknowledgement arrived - and the acknowledgement could not arrive, because the payload was
    /// discarded. Every later retry then answered "already in flight" and did nothing, freezing the frame's application
    /// window forever: a stalled zone-file transfer, no gameplay scope, and a client no peer could ever see.
    fn release_unsent(&self, packet: &[u8]) {
        let Some((id, _)) = crate::reliable::reliable_parts(packet) else {
            return;
        };
        let mut state = self
            .state
            .lock()
            .expect("RTC outbound queue should not be poisoned");
        state.active_reliable_ids.remove(&id);
    }

    fn receive(&self, limit: usize) -> RtcOutboundReceive {
        let mut state = self
            .state
            .lock()
            .expect("RTC outbound queue should not be poisoned");
        while !state.closed && state.realtime.is_none() && state.reliable.is_empty() {
            state = self
                .available
                .wait(state)
                .expect("RTC outbound queue should not be poisoned");
        }
        if state.reliable_overflow {
            return RtcOutboundReceive::ReliableOverflow;
        }
        if state.closed {
            return RtcOutboundReceive::Closed;
        }
        let mut queued = Vec::with_capacity(limit.min(state.reliable.len() + 1));
        let mut removed_bytes = 0;
        // Reserve fresh-state service before selecting the control FIFO.
        // Prioritizing only the completed batch cannot rescue an omitted
        // snapshot when a slow recipient has a full control/event backlog.
        let control_limit = if state.realtime.is_some() && limit > 1 {
            limit - 1
        } else {
            limit
        };
        while queued.len() < control_limit {
            let Some(packet) = state.reliable.pop_front() else {
                break;
            };
            removed_bytes += packet.len();
            queued.push(packet);
        }
        if queued.len() < limit
            && let Some(packet) = state.realtime.take()
        {
            removed_bytes += packet.len();
            queued.push(packet);
        }
        state.bytes = state.bytes.saturating_sub(removed_bytes);
        self.metrics.remove_queue(queued.len(), removed_bytes);
        RtcOutboundReceive::Packets(prioritized_rtc_packets(&mut queued))
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .expect("RTC outbound queue should not be poisoned");
        if state.closed {
            self.close_lane_writer();
            self.available.notify_all();
            return;
        }
        let abandoned_packets = state.reliable.len() + usize::from(state.realtime.is_some());
        let abandoned_bytes = state.bytes;
        state.reliable.clear();
        state.active_reliable_ids.clear();
        state.realtime = None;
        state.bytes = 0;
        state.closed = true;
        self.metrics
            .remove_queue(abandoned_packets, abandoned_bytes);
        self.close_lane_writer();
        self.available.notify_all();
    }

    fn complete_send(&self, packet: &[u8]) {
        let Some((id, _)) = crate::reliable::reliable_parts(packet) else {
            return;
        };
        self.state
            .lock()
            .expect("RTC outbound queue should not be poisoned")
            .active_reliable_ids
            .remove(&id);
    }

    #[cfg(test)]
    fn depth(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (
            state.reliable.len() + usize::from(state.realtime.is_some()),
            state.bytes,
        )
    }
}

impl ClientPacketSink for RtcOutboundQueue {
    fn try_send(&self, packet: Vec<u8>) -> Result<(), ()> {
        self.enqueue(packet)
    }

    fn try_send_events(&self, packets: Vec<Vec<u8>>) -> Result<(), ()> {
        self.enqueue_events(packets)
    }

    fn reliable_overflow(&self) {
        self.fail_reliable_overflow();
    }

    fn rtc_lane_backlog(&self, now: Instant) -> crate::state::RtcLaneBacklog {
        self.lane_writer
            .lock()
            .expect("RTC lane writer")
            .as_ref()
            .and_then(Weak::upgrade)
            .map_or_else(crate::state::RtcLaneBacklog::default, |writer| {
                writer.diagnostic_snapshot(now)
            })
    }
}

impl RtcSession {
    /// The per-session outbound queue, when the session has one. The writer needs it to release a reliable frame it could
    /// not hand to a channel; see `RtcOutboundQueue::release_unsent`.
    fn outbound_queue(&self) -> Option<Arc<RtcOutboundQueue>> {
        self.outbound
            .lock()
            .expect("rtc outbound lock should not be poisoned")
            .clone()
    }
}

struct RtcSession {
    state: Arc<Mutex<ServerState>>,
    peer: String,
    lifecycle: Mutex<RtcSessionLifecycle>,
    lifecycle_changed: watch::Sender<RtcSessionLifecycle>,
    joined: Mutex<bool>,
    inbound_acks: AtomicUsize,
    registered_acks: AtomicUsize,
    channels: Mutex<RtcChannels>,
    outbound: Mutex<Option<Arc<RtcOutboundQueue>>>,
    metrics: Arc<RtcOverloadMetrics>,
    closed: Arc<Notify>,
    admission: RtcAdmission,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RtcSessionLifecycle {
    #[default]
    Handshaking,
    Registered(PlayerSessionIdentity),
    /// Terminal transition is in progress. No new channel, registration, or
    /// packet work may begin, but public `Closed` is withheld until client and
    /// outbound cleanup have both completed.
    Closing,
    Closed,
}

struct RtcAdmission {
    state: Arc<Mutex<ServerState>>,
    released: AtomicBool,
}

impl RtcAdmission {
    fn reserve(state: Arc<Mutex<ServerState>>) -> Result<Self, RtcSignalingError> {
        {
            let mut guard = state.lock().expect("server state should not be poisoned");
            let effective_capacity = guard.rtc_effective_capacity();
            if guard.rtc_sessions >= effective_capacity {
                return Err(RtcSignalingError::capacity_unavailable(format!(
                    "WebRTC session capacity reached ({}/{effective_capacity}; configured {}, degraded UDP ports {})",
                    guard.rtc_sessions,
                    guard.config.max_players,
                    guard.rtc_udp_quarantined_ports.len(),
                )));
            }
            guard.rtc_sessions += 1;
        }
        Ok(Self {
            state,
            released: AtomicBool::new(false),
        })
    }

    fn release(&self) {
        self.settle(None, false);
    }

    fn release_fixed_port(&self, port: u16) {
        self.settle(Some(port), false);
    }

    fn quarantine_fixed_port(&self, port: u16) {
        self.settle(Some(port), true);
    }

    fn settle(&self, fixed_port: Option<u16>, quarantine: bool) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut state = self
            .state
            .lock()
            .expect("server state should not be poisoned");
        state.rtc_sessions = state.rtc_sessions.saturating_sub(1);
        if let Some(port) = fixed_port {
            state.rtc_udp_ports_in_use.remove(&port);
            if quarantine {
                state.rtc_udp_quarantined_ports.insert(port);
            }
        }
    }
}

impl Drop for RtcAdmission {
    fn drop(&mut self) {
        self.release();
    }
}

struct RtcEventHandler {
    session: Arc<RtcSession>,
    gather_complete_tx: tokio::sync::mpsc::Sender<()>,
    executor: Arc<RtcIngressExecutor>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for RtcEventHandler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gather_complete_tx.try_send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        debug!(peer = %self.session.peer, %state, "server rtc peer state changed");
        if let Some(outbound) = self.session.outbound_queue() {
            outbound
                .quality
                .lock()
                .expect("session quality lock poisoned")
                .set_transport_available(
                    state == RTCPeerConnectionState::Connected,
                    Instant::now(),
                );
        }
        if matches!(
            state,
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
        ) {
            disconnect_rtc_session(&self.session);
        }
    }

    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        let metadata = match inspect_rtc_channel(&channel).await {
            Ok(metadata) => metadata,
            Err(reason) => {
                reject_rtc_channel_protocol(&self.session, &reason);
                return;
            }
        };
        let kind = {
            let lifecycle = self
                .session
                .lifecycle
                .lock()
                .expect("rtc session lifecycle lock should not be poisoned");
            if matches!(
                *lifecycle,
                RtcSessionLifecycle::Closing | RtcSessionLifecycle::Closed
            ) {
                return;
            }
            let mut channels = self
                .session
                .channels
                .lock()
                .expect("rtc channels lock should not be poisoned");
            match channels.protocol.announce(&metadata) {
                Ok(RtcChannelKind::Game) => {
                    channels.game = Some(Arc::clone(&channel));
                    RtcChannelKind::Game
                }
                Ok(RtcChannelKind::Event) => {
                    channels.event = Some(Arc::clone(&channel));
                    RtcChannelKind::Event
                }
                Ok(RtcChannelKind::Reliable) => {
                    channels.reliable = Some(Arc::clone(&channel));
                    RtcChannelKind::Reliable
                }
                Err(reason) => {
                    drop(channels);
                    drop(lifecycle);
                    reject_rtc_channel_protocol(&self.session, &reason);
                    return;
                }
            }
        };
        let label = metadata.label.clone();
        debug!(
            label,
            channel_id = channel.id(),
            ordered = metadata.ordered,
            max_packet_life_time = ?metadata.max_packet_life_time,
            max_retransmits = ?metadata.max_retransmits,
            "server rtc data channel announced"
        );

        let session = Arc::clone(&self.session);
        let (ingress, mut ingress_receiver) = RtcInboundQueue::with_capacity(
            Arc::clone(&session.metrics),
            kind,
            RTC_APPLICATION_INBOUND_QUEUE_PACKETS,
        );
        let ingress_session = Arc::clone(&session);
        let ingress_label = label.clone();
        let executor = Arc::clone(&self.executor);
        let registration_executor = Arc::clone(&executor);
        tokio::spawn(async move {
            while let Some(packet) = ingress_receiver.recv().await {
                if wait_for_rtc_registration(&ingress_session).await {
                    let Some(receiver) = dispatch_rtc_ingress_turn(
                        &executor,
                        Arc::clone(&ingress_session),
                        ingress_label.clone(),
                        packet,
                        ingress_receiver,
                    )
                    .await
                    else {
                        break;
                    };
                    ingress_receiver = receiver;
                } else {
                    // The pump had no instrumentation on this path: a packet popped from the lane queue was discarded
                    // whenever the registration wait returned false, with no log, no counter and no session closure, while
                    // the lane kept running and the outbound direction kept working. That silent drop is the only place
                    // left where a session's uplink can vanish while its downlink continues, which is the observed shape
                    // for a stalled join, so the state at the moment of the drop is recorded rather than inferred.
                    static UNREGISTERED_DROPS: std::sync::atomic::AtomicUsize =
                        std::sync::atomic::AtomicUsize::new(0);
                    let drops =
                        UNREGISTERED_DROPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    let lifecycle_state = match *ingress_session
                        .lifecycle
                        .lock()
                        .expect("rtc session lifecycle lock should not be poisoned")
                    {
                        RtcSessionLifecycle::Registered(_) => "registered",
                        RtcSessionLifecycle::Handshaking => "handshaking",
                        RtcSessionLifecycle::Closing => "closing",
                        RtcSessionLifecycle::Closed => "closed",
                    };
                    if drops % 32 == 1 {
                        warn!(
                            label = ingress_label.as_str(),
                            peer = %ingress_session.peer,
                            packets_dropped_before_registration = drops,
                            lifecycle = lifecycle_state,
                            bytes = packet.len(),
                            "server rtc ingress dropped before registration"
                        );
                    }
                }
            }
        });
        tokio::spawn(async move {
            loop {
                if matches!(
                    *session
                        .lifecycle
                        .lock()
                        .expect("rtc session lifecycle lock should not be poisoned"),
                    RtcSessionLifecycle::Closing | RtcSessionLifecycle::Closed
                ) {
                    break;
                }
                let event = tokio::select! {
                    event = channel.poll() => event,
                    _ = session.closed.notified() => None,
                };
                let Some(event) = event else {
                    break;
                };
                match event {
                    DataChannelEvent::OnOpen => {
                        debug!(label, "server rtc data channel open");
                        let all_open = session
                            .channels
                            .lock()
                            .expect("rtc channels lock should not be poisoned")
                            .protocol
                            .mark_open(kind);
                        if all_open {
                            dispatch_rtc_registration(&registration_executor, Arc::clone(&session))
                                .await;
                        }
                    }
                    DataChannelEvent::OnMessage(message) => {
                        if message.data.len() > RTC_APPLICATION_INBOUND_MESSAGE_BYTES {
                            // Validate before copying the transport-owned bytes
                            // into application retention, even on a reliable lane.
                            session.metrics.record_protocol_violation();
                            warn!(
                                label,
                                bytes = message.data.len(),
                                maximum = RTC_APPLICATION_INBOUND_MESSAGE_BYTES,
                                "server rtc inbound message exceeds intent/control limit"
                            );
                            disconnect_rtc_session(&session);
                            break;
                        }
                        if message.is_string {
                            warn!(
                                label,
                                bytes = message.data.len(),
                                "server rtc ignored text data channel message"
                            );
                        } else {
                            if !enqueue_rtc_ingress_message(
                                &session,
                                &label,
                                &ingress,
                                message.data.to_vec(),
                            ) {
                                break;
                            }
                        }
                    }
                    DataChannelEvent::OnClose => {
                        debug!(label, "server rtc data channel closed");
                        disconnect_rtc_session(&session);
                        break;
                    }
                    DataChannelEvent::OnError => {
                        warn!(label, peer = %session.peer, "server rtc data channel error");
                        disconnect_rtc_session(&session);
                        break;
                    }
                    _ => {}
                }
            }
        });
    }
}

fn enqueue_rtc_ingress_message(
    session: &Arc<RtcSession>,
    label: &str,
    ingress: &RtcInboundQueue,
    packet: Vec<u8>,
) -> bool {
    if packet.first()
        == Some(&crate::connection_quality_protocol::ConnectionQualityProbe::ECHO_TYPE)
    {
        // The peer/channel are already authenticated by this RTC session.
        // Record arrival before waiting for the simulation/handler pool.
        session
            .metrics
            .record_inbound_packet(ingress.kind.metric_index(), packet.len());
        let registered = matches!(
            *session.lifecycle.lock().expect("RTC lifecycle poisoned"),
            RtcSessionLifecycle::Registered(_)
        );
        if registered && label == GAME_CHANNEL_LABEL {
            if let (Some(outbound), Ok(probe)) = (
                session.outbound_queue(),
                crate::connection_quality_protocol::ConnectionQualityProbe::parse_echo(&packet),
            ) {
                outbound
                    .quality
                    .lock()
                    .expect("session quality lock poisoned")
                    .receive_echo(probe, Instant::now());
            }
        }
        return true;
    }
    if crate::connection::position_timing_trace_enabled()
        && tracing::enabled!(tracing::Level::INFO)
        && packet.first().copied() == Some(crate::arena::CLIENT_POSITION)
    {
        let input_sequence = crate::continuum_position::parse_client_position_snapshot(&packet)
            .and_then(|snapshot| snapshot.client_sequence);
        // The DataChannel callback has yielded this packet. This is an enqueue
        // attempt, not proof of queue acceptance, authentication or admission.
        tracing::info!(
            peer = %session.peer,
            label,
            input_sequence = ?input_sequence,
            "position rtc queue entry"
        );
    }
    match ingress.try_enqueue(packet) {
        Ok(()) => true,
        Err(
            reason @ (RtcInboundEnqueueError::Full
            | RtcInboundEnqueueError::ByteLimit
            | RtcInboundEnqueueError::Oversized),
        ) => {
            warn!(
                label,
                peer = %session.peer,
                capacity = RTC_APPLICATION_INBOUND_QUEUE_PACKETS,
                queued_bytes = ingress.queued_bytes.load(Ordering::Relaxed),
                byte_capacity = ingress.byte_limit,
                reason = ?reason,
                "server RTC application ingress queue overflowed; disconnecting peer without silent reliable loss"
            );
            disconnect_rtc_session(session);
            false
        }
        Err(RtcInboundEnqueueError::Closed) => false,
    }
}

async fn inspect_rtc_channel(channel: &Arc<dyn DataChannel>) -> Result<RtcChannelMetadata, String> {
    Ok(RtcChannelMetadata {
        label: channel
            .label()
            .await
            .map_err(|error| format!("failed to inspect DataChannel label: {error}"))?,
        ordered: channel
            .ordered()
            .await
            .map_err(|error| format!("failed to inspect DataChannel ordering: {error}"))?,
        max_packet_life_time: channel
            .max_packet_life_time()
            .await
            .map_err(|error| format!("failed to inspect DataChannel lifetime: {error}"))?,
        max_retransmits: channel
            .max_retransmits()
            .await
            .map_err(|error| format!("failed to inspect DataChannel retransmits: {error}"))?,
        protocol: channel
            .protocol()
            .await
            .map_err(|error| format!("failed to inspect DataChannel protocol: {error}"))?,
        negotiated: channel
            .negotiated()
            .await
            .map_err(|error| format!("failed to inspect DataChannel negotiation: {error}"))?,
    })
}

fn reject_rtc_channel_protocol(session: &Arc<RtcSession>, reason: &str) {
    session.metrics.record_protocol_violation();
    warn!(peer = %session.peer, reason, "server RTC peer violated DataChannel protocol");
    disconnect_rtc_session(session);
}

async fn build_peer(
    handler: Arc<RtcEventHandler>,
    config: &ServerConfig,
    state: &Arc<Mutex<ServerState>>,
) -> Result<(Arc<dyn PeerConnection>, RtcUdpBinding), RtcSignalingError> {
    let mut setting_engine = SettingEngineBuilder::new();
    if let Some(max_packet_size) = config.rtc_sctp_max_packet_size() {
        setting_engine = setting_engine.with_sctp_mtu(max_packet_size);
    }
    let setting_engine = setting_engine.build();
    let addresses = rtc_udp_bind_addresses(config);
    let mut last_error = None;
    for address in addresses.iter().cloned() {
        let binding = RtcUdpBinding::new(address.clone());
        let _port_lifecycle = if let Some(fixed) = binding.fixed.as_ref() {
            let lifecycle = Arc::clone(&fixed.lifecycle);
            let guard = lifecycle.lock_owned().await;
            let (in_use, quarantined) = {
                let state = state.lock().expect("server state should not be poisoned");
                (
                    state.rtc_udp_ports_in_use.contains(&fixed.port),
                    state.rtc_udp_quarantined_ports.contains(&fixed.port),
                )
            };
            if in_use {
                continue;
            }
            if quarantined {
                // A cancelled/crashed recovery owner is restarted by normal
                // admission traffic; an existing worker is deduplicated.
                spawn_rtc_udp_port_recovery(
                    Arc::clone(state),
                    fixed.clone(),
                    binding.address.clone(),
                );
                continue;
            }
            match std::net::UdpSocket::bind(&binding.address) {
                Ok(probe) => drop(probe),
                Err(error) => {
                    quarantine_rtc_udp_port(state, fixed.clone(), binding.address.clone(), &error);
                    continue;
                }
            }
            Some(guard)
        } else {
            None
        };
        let result = PeerConnectionBuilder::new()
            .with_configuration(rtc_configuration(config))
            .with_setting_engine(setting_engine.clone())
            .with_handler(handler.clone())
            .with_runtime(Arc::new(TokioRuntime))
            .with_udp_addrs(vec![binding.address.clone()])
            .with_data_channel_send_buffer_limit(RTC_DATA_CHANNEL_SEND_BUFFER_BYTES)
            .build()
            .await;
        match result {
            Ok(peer) => {
                if let Some(fixed) = binding.fixed.as_ref() {
                    state
                        .lock()
                        .expect("server state should not be poisoned")
                        .rtc_udp_ports_in_use
                        .insert(fixed.port);
                }
                return Ok((Arc::new(peer), binding));
            }
            Err(err) => {
                if let Some(fixed) = binding.fixed.as_ref()
                    && let Err(bind_error) = std::net::UdpSocket::bind(&binding.address)
                {
                    quarantine_rtc_udp_port(
                        state,
                        fixed.clone(),
                        binding.address.clone(),
                        &bind_error,
                    );
                }
                last_error = Some(err);
            }
        }
    }

    let error = last_error
        .map(|err| err.to_string())
        .unwrap_or_else(|| "no UDP bind addresses were available".to_string());
    Err(RtcSignalingError::service_unavailable(format!(
        "failed to create WebRTC peer after trying {} UDP address(es): {error}",
        addresses.len()
    )))
}

async fn wait_for_ice_gathering(
    gather_complete_rx: &mut tokio::sync::mpsc::Receiver<()>,
    timeout: Duration,
) -> Result<(), RtcSignalingError> {
    match tokio::time::timeout(timeout, gather_complete_rx.recv()).await {
        Ok(Some(())) => Ok(()),
        Ok(None) => Err(RtcSignalingError::service_unavailable(
            "WebRTC ICE gathering service stopped before candidate completion",
        )),
        Err(_) => Err(RtcSignalingError::service_unavailable(format!(
            "timed out gathering WebRTC ICE candidates after {}ms",
            timeout.as_millis()
        ))),
    }
}

pub(crate) fn create_answer_for_offer_json(
    state: Arc<Mutex<ServerState>>,
    peer_label: String,
    offer_json: &[u8],
) -> Result<String, RtcSignalingError> {
    let offer: RTCSessionDescription = serde_json::from_slice(offer_json)
        .map_err(|err| RtcSignalingError::invalid_offer(format!("invalid JSON: {err}")))?;
    if offer.sdp_type.to_string() != "offer" {
        return Err(RtcSignalingError::invalid_offer(format!(
            "expected offer SDP type, got {}",
            offer.sdp_type
        )));
    }

    let admission = RtcAdmission::reserve(Arc::clone(&state))?;
    let config = state
        .lock()
        .expect("server state should not be poisoned")
        .config
        .clone();
    let runtime = rtc_runtime()?;
    // Signaling runs on its caller, not a DataChannel poll worker. Publish the
    // shared executor here so channel announcements never lock ServerState to
    // discover it while the simulation owns that lock.
    let executor = {
        let state = state.lock().expect("server state should not be poisoned");
        if let Some(executor) = state.rtc_ingress_executor.get() {
            Arc::clone(executor)
        } else {
            let executor = Arc::new(
                RtcIngressExecutor::try_new(runtime.metrics().num_workers()).map_err(|error| {
                    RtcSignalingError::internal(format!(
                        "unable to start RTC ingress workers: {error}"
                    ))
                })?,
            );
            Arc::clone(state.rtc_ingress_executor.get_or_init(|| executor))
        }
    };
    let peer = runtime.block_on(async move {
        let closed = Arc::new(Notify::new());
        let metrics = state
            .lock()
            .expect("server state should not be poisoned")
            .rtc_overload_metrics
            .clone();
        let lifecycle = RtcSessionLifecycle::Handshaking;
        let session = Arc::new(RtcSession {
            state,
            peer: peer_label,
            lifecycle: Mutex::new(lifecycle),
            lifecycle_changed: watch::channel(lifecycle).0,
            joined: Mutex::new(false),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(None),
            metrics,
            closed: Arc::clone(&closed),
            admission,
        });
        #[cfg(test)]
        observe_test_rtc_session(&session);
        let (gather_complete_tx, mut gather_complete_rx) = tokio::sync::mpsc::channel(1);
        let handler = Arc::new(RtcEventHandler {
            executor,
            session: Arc::clone(&session),
            gather_complete_tx,
        });
        let (peer, binding) = build_peer(handler, &config, &session.state).await?;
        let answer_result = async {
            peer.set_remote_description(offer)
                .await
                .map_err(|err| {
                    RtcSignalingError::invalid_offer(format!(
                        "failed to apply remote offer SDP: {err}"
                    ))
                })?;
            let answer = peer
                .create_answer(None)
                .await
                .map_err(|err| {
                    RtcSignalingError::internal(format!(
                        "failed to create WebRTC answer: {err}"
                    ))
                })?;
            peer.set_local_description(answer)
                .await
                .map_err(|err| {
                    RtcSignalingError::internal(format!(
                        "failed to set WebRTC local answer: {err}"
                    ))
                })?;
            wait_for_ice_gathering(&mut gather_complete_rx, Duration::from_secs(10)).await?;
            let answer = peer
                .local_description()
                .await
                .ok_or_else(|| {
                    RtcSignalingError::internal("WebRTC local answer was not available")
                })?;
            serde_json::to_string(&answer).map_err(|err| {
                RtcSignalingError::internal(format!(
                    "failed to encode WebRTC answer JSON: {err}"
                ))
            })
        }
        .await;
        let answer_json = match answer_result {
            Ok(answer_json) => answer_json,
            Err(error) => {
                disconnect_rtc_session(&session);
                close_rtc_peer_and_release(
                    peer,
                    binding,
                    Arc::clone(&session),
                    "server rejected RTC peer",
                )
                .await;
                return Err(error);
            }
        };
        let handshake_session = Arc::downgrade(&session);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let Some(handshake_session) = handshake_session.upgrade() else {
                return;
            };
            if expire_handshaking_rtc_session(&handshake_session) {
                warn!(peer = %handshake_session.peer, "server rtc handshake expired before DataChannel open");
            }
        });
        Ok::<_, RtcSignalingError>((peer, binding, session, answer_json))
    })?;

    let (peer, binding, session, answer_json) = peer;
    keep_peer_alive(runtime, peer, binding, session);
    Ok(answer_json)
}

async fn dispatch_rtc_registration(
    executor: &RtcIngressExecutor,
    session: Arc<RtcSession>,
) -> bool {
    executor
        .execute(move || register_rtc_session_if_needed(&session))
        .await
        .unwrap_or(false)
}

fn register_rtc_session_if_needed(session: &Arc<RtcSession>) -> bool {
    if !session
        .channels
        .lock()
        .expect("rtc channels lock should not be poisoned")
        .protocol
        .all_open()
    {
        return false;
    }
    // Never hold the session lifecycle lock while waiting for simulation.
    // Expiry remains able to retire a queued registration; recheck only after
    // acquiring server state, before allocating any client identity.
    let mut state = session
        .state
        .lock()
        .expect("server state should not be poisoned");
    let mut lifecycle = session
        .lifecycle
        .lock()
        .expect("rtc session lifecycle lock should not be poisoned");
    if *lifecycle != RtcSessionLifecycle::Handshaking {
        return false;
    }

    let outbound = Arc::new(RtcOutboundQueue::new(Arc::clone(&session.metrics)));
    outbound
        .quality
        .lock()
        .expect("session quality lock poisoned")
        .configure(state.config.connection_quality);
    outbound.attach_session(Arc::downgrade(session), tokio::runtime::Handle::current());
    let identity = crate::arena::register_connection_with_sender_in_state_at_tick(
        &mut state,
        ClientPacketSender::bounded(outbound.clone()),
        crate::wire::current_tick(),
    );
    state
        .clients
        .get_mut(&identity.player_id)
        .expect("registered RTC client")
        .connection_quality = Some(Arc::clone(&outbound.quality));
    *session
        .outbound
        .lock()
        .expect("rtc outbound lock should not be poisoned") = Some(Arc::clone(&outbound));
    *lifecycle = RtcSessionLifecycle::Registered(identity);
    session.lifecycle_changed.send_replace(*lifecycle);
    drop(state);
    info!(player_id = identity.player_id, peer = %session.peer, "server rtc player connected");

    let writer_session = Arc::clone(session);
    let handle = tokio::runtime::Handle::current();
    let login_session = Arc::downgrade(session);
    handle.spawn(async move {
        tokio::time::sleep(RTC_LOGIN_DEADLINE).await;
        if let Some(session) = login_session.upgrade() {
            expire_rtc_session_without_login(&session, identity);
        }
    });
    thread::spawn(move || write_rtc_packets(outbound, handle, writer_session));
    true
}

/// Disconnect a still-registered session whose login was never accepted.
fn expire_rtc_session_without_login(
    session: &Arc<RtcSession>,
    identity: PlayerSessionIdentity,
) -> bool {
    // Registration takes server state before the lifecycle lock, so release
    // the lifecycle lock before asking server state about the login.
    let registered = *session
        .lifecycle
        .lock()
        .expect("rtc session lifecycle lock should not be poisoned")
        == RtcSessionLifecycle::Registered(identity);
    if !registered || crate::identity::player_login_accepted(&session.state, identity.player_id) {
        return false;
    }
    warn!(
        player_id = identity.player_id,
        peer = %session.peer,
        deadline_s = RTC_LOGIN_DEADLINE.as_secs(),
        "server rtc session never logged in; disconnecting"
    );
    disconnect_rtc_session(session);
    true
}

fn disconnect_rtc_session(session: &Arc<RtcSession>) {
    let Some((identity, retained_channels)) = transition_rtc_session_closing(session, false) else {
        return;
    };
    // Drop every session-owned DataChannel before waking peer teardown. A
    // DataChannel owns PeerConnectionRef, whose handler owns this session; if
    // these fields survive terminal transition that graph is a permanent Arc
    // cycle even after all status counters return to zero.
    drop(retained_channels);
    if let Some(identity) = identity {
        disconnect_player_session(&session.state, identity);
        info!(player_id = identity.player_id, peer = %session.peer, "server rtc player disconnected");
    }
    if let Some(outbound) = session
        .outbound
        .lock()
        .expect("rtc outbound lock should not be poisoned")
        .take()
    {
        outbound.close();
    }
    finish_rtc_session_closed(session);
}

fn finish_rtc_session_closed(session: &Arc<RtcSession>) {
    let mut lifecycle = session
        .lifecycle
        .lock()
        .expect("rtc session lifecycle lock should not be poisoned");
    if *lifecycle != RtcSessionLifecycle::Closing {
        return;
    }
    *lifecycle = RtcSessionLifecycle::Closed;
    session
        .lifecycle_changed
        .send_replace(RtcSessionLifecycle::Closed);
    drop(lifecycle);
    session.closed.notify_waiters();
    // Leave one permit for a keepalive task that races registration after the
    // broadcast; channel pollers also re-check the terminal lifecycle flag.
    session.closed.notify_one();
}

fn transition_rtc_session_closing(
    session: &Arc<RtcSession>,
    handshaking_only: bool,
) -> Option<(Option<PlayerSessionIdentity>, RtcChannels)> {
    // All paths that can store a newly announced channel take lifecycle before
    // channels in this same order. Holding lifecycle across the take makes the
    // terminal transition atomic: a raced callback either stores before this
    // take or observes Closed and drops its channel without retaining it.
    let mut lifecycle = session
        .lifecycle
        .lock()
        .expect("rtc session lifecycle lock should not be poisoned");
    if handshaking_only && *lifecycle != RtcSessionLifecycle::Handshaking {
        return None;
    }
    let identity = match *lifecycle {
        RtcSessionLifecycle::Registered(identity) => Some(identity),
        RtcSessionLifecycle::Handshaking => None,
        RtcSessionLifecycle::Closing | RtcSessionLifecycle::Closed => return None,
    };
    // Drain both bounded application stages before waking workers that react
    // to Closing. Otherwise a lane can close while an already-drained central
    // batch is still dispatching, falsely classifying normal teardown as full.
    // The queue handle remains attached until authoritative client removal.
    if let Some(outbound) = session.outbound_queue() {
        outbound.close();
    }
    *lifecycle = RtcSessionLifecycle::Closing;
    session
        .lifecycle_changed
        .send_replace(RtcSessionLifecycle::Closing);
    let retained_channels = std::mem::take(
        &mut *session
            .channels
            .lock()
            .expect("rtc channels lock should not be poisoned"),
    );
    Some((identity, retained_channels))
}

fn expire_handshaking_rtc_session(session: &Arc<RtcSession>) -> bool {
    let Some((identity, retained_channels)) = transition_rtc_session_closing(session, true) else {
        return false;
    };
    debug_assert!(identity.is_none());
    drop(retained_channels);
    finish_rtc_session_closed(session);
    true
}

async fn wait_for_rtc_registration(session: &Arc<RtcSession>) -> bool {
    // A browser can observe all three local channels as Open before the three
    // independent server poll tasks have all consumed their OnOpen events.
    // Preserve an immediately sent login packet until registration wins that
    // small race; terminal transitions publish Closed and release the waiter.
    let mut lifecycle_changed = session.lifecycle_changed.subscribe();
    loop {
        match *session
            .lifecycle
            .lock()
            .expect("rtc session lifecycle lock should not be poisoned")
        {
            RtcSessionLifecycle::Registered(_) => return true,
            RtcSessionLifecycle::Closing | RtcSessionLifecycle::Closed => return false,
            RtcSessionLifecycle::Handshaking => {}
        }
        if lifecycle_changed.changed().await.is_err() {
            return false;
        }
    }
}

type RtcIngressWork = Box<dyn FnOnce() + Send + 'static>;

#[derive(Debug, PartialEq, Eq)]
enum RtcIngressExecutionError {
    Unavailable,
    Panicked,
}

#[cfg(test)]
impl RtcIngressExecutionError {
    fn is_panic(&self) -> bool {
        *self == Self::Panicked
    }
}

pub(crate) struct RtcIngressExecutor {
    permits: Arc<tokio::sync::Semaphore>,
    limit: usize,
    waiting: AtomicUsize,
    sender: std::sync::mpsc::SyncSender<RtcIngressWork>,
    workers: Arc<AtomicUsize>,
}

struct RtcIngressWorkerLifetime(Arc<AtomicUsize>);

impl Drop for RtcIngressWorkerLifetime {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct RtcIngressWaiter<'a>(&'a AtomicUsize);

impl Drop for RtcIngressWaiter<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl RtcIngressExecutor {
    #[cfg(test)]
    fn new(limit: usize) -> Self {
        Self::try_new(limit).expect("test ingress workers start")
    }

    fn try_new(limit: usize) -> std::io::Result<Self> {
        if limit == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "zero RTC ingress worker limit",
            ));
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel::<RtcIngressWork>(limit);
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = Arc::new(AtomicUsize::new(0));
        for _ in 0..limit {
            let receiver = Arc::clone(&receiver);
            workers.fetch_add(1, Ordering::AcqRel);
            let lifetime = RtcIngressWorkerLifetime(Arc::clone(&workers));
            // Detached only from joining: dropping the last sender closes the
            // receiver and all idle workers exit. A task may own the final
            // server reference, so joining workers from Drop could self-join.
            // The lifetime count also covers threads not yet scheduled and
            // unwinds correctly if spawning a partial pool fails.
            thread::Builder::new()
                .name("zubspace-input".into())
                .spawn(move || {
                    let _lifetime = lifetime;
                    loop {
                        let work = receiver.lock().expect("ingress receiver poisoned").recv();
                        let Ok(work) = work else {
                            break;
                        };
                        work();
                    }
                })?;
        }
        Ok(Self {
            permits: Arc::new(tokio::sync::Semaphore::new(limit)),
            limit,
            waiting: AtomicUsize::new(0),
            sender,
            workers,
        })
    }

    pub(crate) fn worker_count(&self) -> usize {
        self.workers.load(Ordering::Acquire)
    }

    /// Reserved includes submitted/running work, not only occupied CPU time.
    /// Packet callers hold at most one popped packet per channel. Registration
    /// callers are bounded by admitted sessions and share these execution slots.
    pub(crate) fn status(&self) -> (usize, usize, usize) {
        (
            self.limit,
            self.limit - self.permits.available_permits(),
            self.waiting.load(Ordering::Acquire),
        )
    }

    async fn execute<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, RtcIngressExecutionError> {
        // Acquire before submission: the worker pool never becomes a second
        // unbounded packet queue. Each channel awaits its bounded drain turn
        // before submitting another. Preserve the permit inside the closure so
        // cancellation of its async caller cannot release a still-running slot.
        self.waiting.fetch_add(1, Ordering::AcqRel);
        let waiting = RtcIngressWaiter(&self.waiting);
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .expect("RTC ingress execution semaphore is never closed");
        drop(waiting);
        let (completed, result) = tokio::sync::oneshot::channel();
        let runtime = tokio::runtime::Handle::current();
        self.sender
            .try_send(Box::new(move || {
                // Handler cleanup/send paths may spawn asynchronous RTC work.
                // Preserve the submitting runtime context on dedicated workers.
                let _runtime = runtime.enter();
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                    .map_err(|_| RtcIngressExecutionError::Panicked);
                drop(permit);
                let _ = completed.send(outcome);
            }))
            .map_err(|_| RtcIngressExecutionError::Unavailable)?;
        result
            .await
            .unwrap_or(Err(RtcIngressExecutionError::Unavailable))
    }
}

async fn dispatch_rtc_ingress_turn(
    executor: &RtcIngressExecutor,
    session: Arc<RtcSession>,
    label: String,
    packet: Vec<u8>,
    mut receiver: RtcInboundReceiver,
) -> Option<RtcInboundReceiver> {
    let failed_session = Arc::clone(&session);
    match executor
        .execute(move || {
            drain_rtc_ingress_turn(packet, &mut receiver, |packet| {
                handle_rtc_packet(&session, &label, packet)
            });
            receiver
        })
        .await
    {
        Ok(receiver) => Some(receiver),
        Err(_) => {
            disconnect_rtc_session(&failed_session);
            None
        }
    }
}

fn handle_rtc_packet(session: &Arc<RtcSession>, label: &str, packet: Vec<u8>) {
    if packet.as_slice() == [crate::CORE_BYTE, 0x07] {
        if label != RELIABLE_CHANNEL_LABEL {
            reject_rtc_channel_protocol(session, "core disconnect arrived outside reliable lane");
        } else {
            disconnect_rtc_session(session);
        }
        return;
    }
    handle_rtc_packet_after_lifecycle_lookup(session, label, packet, || {});
}

// Diagnostic budget only: never used for packet admission, queues or timing
// policy. Keep slow non-position handlers observable without enabling an
// unbounded per-packet INFO stream during a loaded RTC run.
fn slow_rtc_handler_trace_candidate(packet_type: Option<u8>, elapsed: Duration) -> bool {
    packet_type.is_some()
        && packet_type != Some(crate::arena::CLIENT_POSITION)
        && elapsed >= Duration::from_millis(5)
}

struct RtcSlowHandlerTraceBudget {
    window_started: Instant,
    emitted: u8,
    suppressed: u64,
}

impl RtcSlowHandlerTraceBudget {
    fn new(now: Instant) -> Self {
        Self {
            window_started: now,
            emitted: 0,
            suppressed: 0,
        }
    }

    fn admit(&mut self, now: Instant) -> Option<u64> {
        if now.saturating_duration_since(self.window_started) >= Duration::from_secs(1) {
            self.window_started = now;
            self.emitted = 0;
        }
        if self.emitted >= 16 {
            self.suppressed = self.suppressed.saturating_add(1);
            return None;
        }
        self.emitted += 1;
        Some(std::mem::take(&mut self.suppressed))
    }
}

fn handle_rtc_packet_after_lifecycle_lookup(
    session: &Arc<RtcSession>,
    label: &str,
    packet: Vec<u8>,
    after_lookup: impl FnOnce(),
) {
    // This begins after the RTC application queue is popped, not at UDP or
    // DataChannel arrival. Keep that boundary explicit in diagnostic evidence.
    let timing_started = (crate::connection::position_timing_trace_enabled()
        && tracing::enabled!(tracing::Level::INFO))
    .then(Instant::now);
    let packet_type = packet.first().copied();
    let core_subtype = (packet_type == Some(crate::CORE_BYTE))
        .then(|| packet.get(1).copied())
        .flatten();
    let is_ack = packet_type == Some(crate::CORE_BYTE)
        && core_subtype == Some(crate::reliable::RELIABLE_ACK);
    // Session ownership bounds diagnostic storage and distinguishes reconnects,
    // including multiple sessions sharing the same IP. No formatted map keys.
    if is_ack {
        static ACK_INBOUND_SEEN: AtomicUsize = AtomicUsize::new(0);
        let seen = ACK_INBOUND_SEEN
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let session_seen = session
            .inbound_acks
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if seen % 256 == 1 || session_seen % 64 == 1 {
            tracing::info!(target: crate::log_targets::RTC_ACKS, label, peer = %session.peer,
                ack_packets_seen = seen, session_ack_packets_seen = session_seen,
                packet_len = packet.len(), "server rtc_ack_inbound");
        }
    }
    let packet_bytes = packet.len();
    let timing_sequence = timing_started
        .filter(|_| packet_type == Some(crate::arena::CLIENT_POSITION))
        .and_then(|_| {
            crate::continuum_position::parse_client_position_snapshot(&packet)
                .and_then(|snapshot| snapshot.client_sequence)
        });
    let identity = match *session
        .lifecycle
        .lock()
        .expect("rtc session lifecycle lock should not be poisoned")
    {
        RtcSessionLifecycle::Registered(identity) => Some(identity),
        RtcSessionLifecycle::Handshaking
        | RtcSessionLifecycle::Closing
        | RtcSessionLifecycle::Closed => None,
    };
    let Some(identity) = identity else {
        warn!(
            label,
            peer = %session.peer,
            bytes = packet.len(),
            "server rtc dropped pre-login packet"
        );
        return;
    };
    if is_ack {
        let door = session
            .registered_acks
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if door % 64 == 1 {
            tracing::info!(target: crate::log_targets::RTC_ACKS,
                player_id = identity.player_id, session = ?identity.session_id,
                label, door_acks = door, "server rtc_ack_door_for_player");
        }
    }

    let runtime_identity: Option<PlayerRuntimeIdentity> = session
        .state
        .lock()
        .expect("server state should not be poisoned")
        .active_player_runtime_identity(identity.player_id)
        .filter(|runtime| runtime.session == identity);
    after_lookup();
    let lookup_completed = timing_started.map(|_| Instant::now());

    let mut joined = session
        .joined
        .lock()
        .expect("rtc joined lock should not be poisoned");
    let joined_acquired = timing_started.map(|_| Instant::now());
    let handler_phases = handle_client_packet_for_session(
        &session.state,
        identity,
        runtime_identity,
        packet,
        &session.peer,
        &mut joined,
        timing_started.is_some(),
    );
    drop(joined);
    if let (Some(started), Some(lookup), Some(acquired)) =
        (timing_started, lookup_completed, joined_acquired)
    {
        let finished = Instant::now();
        let total = finished.duration_since(started);
        if packet_type == Some(crate::arena::CLIENT_POSITION) {
            tracing::info!(
            runtime = ?runtime_identity,
            peer = %session.peer,
            input_sequence = ?timing_sequence,
            lookup_and_decode_us = lookup.duration_since(started).as_micros() as u64,
            joined_lock_wait_us = acquired.duration_since(lookup).as_micros() as u64,
            authenticated_handler_us = finished.duration_since(acquired).as_micros() as u64,
            total_us = total.as_micros() as u64,
            "position rtc handler timing"
            );
        } else if slow_rtc_handler_trace_candidate(packet_type, total) {
            static BUDGET: std::sync::OnceLock<Mutex<RtcSlowHandlerTraceBudget>> =
                std::sync::OnceLock::new();
            let suppressed = BUDGET
                .get_or_init(|| Mutex::new(RtcSlowHandlerTraceBudget::new(finished)))
                .lock()
                .ok()
                .and_then(|mut budget| budget.admit(finished));
            if let Some(suppressed) = suppressed {
                tracing::info!(
                    runtime = ?runtime_identity, peer = %session.peer, label,
                    packet_type = ?packet_type, core_subtype = ?core_subtype, packet_bytes,
                    lookup_and_decode_us = lookup.duration_since(started).as_micros() as u64,
                    joined_lock_wait_us = acquired.duration_since(lookup).as_micros() as u64,
                    authenticated_handler_us = finished.duration_since(acquired).as_micros() as u64,
                    total_us = total.as_micros() as u64, suppressed_since_previous = suppressed,
                    arena_lookup_us = ?handler_phases.as_ref().map(|p| p.arena_lookup_us),
                    arena_wait_us = ?handler_phases.as_ref().map(|p| p.arena_wait_us),
                    ingress_lookup_us = ?handler_phases.as_ref().map(|p| p.ingress_lookup_us),
                    ingress_wait_us = ?handler_phases.as_ref().map(|p| p.ingress_wait_us),
                    identity_check_us = ?handler_phases.as_ref().map(|p| p.identity_check_us),
                    dispatch_us = ?handler_phases.as_ref().map(|p| p.dispatch_us),
                    "slow non-position rtc handler timing"
                );
            }
        }
    }
}

fn write_rtc_packets(
    outbound: Arc<RtcOutboundQueue>,
    handle: tokio::runtime::Handle,
    session: Arc<RtcSession>,
) {
    let lanes = writer::LaneWriter::new(Arc::clone(&session.metrics));
    outbound.attach_lane_writer(&lanes);
    lanes.start(&handle, &session, &outbound);
    let _lane_cleanup = writer::DispatcherGuard(Arc::clone(&lanes));
    let mut force_repel_reorder =
        std::env::var("ZUBSPACE_E2E_REALTIME_REPEL_REORDER").as_deref() == Ok("1");
    let mut held_pre_repel_weapons = Vec::new();
    loop {
        match outbound.receive(RTC_OUTBOUND_DRAIN_LIMIT) {
            RtcOutboundReceive::Packets(packets) => {
                // A tick's events arrive together (`ClientPacketSender::send_events`)
                // and leave as one message. The forced Repel reorder below
                // needs them one by one.
                let packets = if force_repel_reorder {
                    packets
                } else {
                    crate::realtime_event::batch_realtime_events(packets)
                };
                for packet in packets {
                    if force_repel_reorder
                        && let Some(order) = realtime_reorder_packet_order(&packet)
                    {
                        if matches!(order.weapon_type, 6 | 8) {
                            held_pre_repel_weapons.push((order, packet));
                            continue;
                        }
                        if order.weapon_type == 5 && !held_pre_repel_weapons.is_empty() {
                            // Inject one causal reorder per session. Later
                            // refresh replies must flow normally so bounded
                            // client recovery is not turned into a permanent
                            // blackhole waiting for another Repel.
                            force_repel_reorder = false;
                            if !send_rtc_packet(&handle, &session, &lanes, &packet) {
                                return;
                            }
                            let held_event_ids = held_pre_repel_weapons
                                .iter()
                                .map(|(order, _)| order.event_id.to_string())
                                .collect::<Vec<_>>()
                                .join(",");
                            let held_weapon_types = held_pre_repel_weapons
                                .iter()
                                .map(|(order, _)| order.weapon_type.to_string())
                                .collect::<Vec<_>>()
                                .join(",");
                            let held_recovery_event_ids = held_pre_repel_weapons
                                .iter()
                                .filter(|(order, _)| {
                                    order.packet_kind
                                        == crate::realtime_event::REALTIME_EVENT_PROJECTILE_STATE
                                })
                                .map(|(order, _)| order.event_id.to_string())
                                .collect::<Vec<_>>()
                                .join(",");
                            let recipient_id = rtc_session_player_id(&session)
                                .map_or_else(|| "null".to_owned(), |id| id.to_string());
                            info!(
                                "zubspace_event {{\"type\":\"server_realtime_repel_reorder_forced\",\"recipient_id\":{},\"repel_event_id\":{},\"held_event_ids\":[{}],\"held_weapon_types\":[{}],\"held_recovery_event_ids\":[{}]}}",
                                recipient_id,
                                order.event_id,
                                held_event_ids,
                                held_weapon_types,
                                held_recovery_event_ids,
                            );
                            for (_, held) in held_pre_repel_weapons.drain(..) {
                                if !send_rtc_packet(&handle, &session, &lanes, &held) {
                                    return;
                                }
                            }
                            continue;
                        }
                    }
                    if !send_rtc_packet(&handle, &session, &lanes, &packet) {
                        return;
                    }
                }
            }
            RtcOutboundReceive::ReliableOverflow => {
                warn!(peer = %session.peer, "server RTC reliable outbound queue overflowed; disconnecting stalled peer");
                handle_rtc_reliable_overflow(&session);
                return;
            }
            RtcOutboundReceive::Closed => {
                for (_, held) in held_pre_repel_weapons.drain(..) {
                    outbound.complete_send(&held);
                }
                return;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RealtimeReorderPacketOrder {
    event_id: u32,
    weapon_type: u8,
    packet_kind: u8,
}

fn realtime_reorder_packet_order(packet: &[u8]) -> Option<RealtimeReorderPacketOrder> {
    use crate::realtime_event::*;
    let event = AuthenticatedRealtimeEvent::parse(packet).ok()?;
    let (weapon_type, packet_kind) = match event.payload {
        RealtimeEventPayload::Weapon(weapon) => {
            ((weapon.weapon_data & 0x1f) as u8, REALTIME_EVENT_WEAPON)
        }
        RealtimeEventPayload::ProjectileState(state) if !state.terminal => (
            (state.weapon_data & 0x1f) as u8,
            REALTIME_EVENT_PROJECTILE_STATE,
        ),
        _ => return None,
    };
    Some(RealtimeReorderPacketOrder {
        event_id: event.event_id,
        weapon_type,
        packet_kind,
    })
}

fn rtc_session_player_id(session: &RtcSession) -> Option<u16> {
    match *session
        .lifecycle
        .lock()
        .expect("rtc session lifecycle lock should not be poisoned")
    {
        RtcSessionLifecycle::Registered(identity) => Some(identity.player_id),
        _ => None,
    }
}

fn handle_rtc_reliable_overflow(session: &Arc<RtcSession>) {
    disconnect_rtc_session(session);
}

/// Classify a server-emitted packet as "realtime" — meaning it
/// belongs on the unordered, lifetime-bounded game channel rather
/// than the reliable channel. Snapshot frames (`0x70`) ship replaceable
/// position, velocity, energy, status, and bounty deltas. Independently
/// sequenced authenticated one-shot events (`0x7d`) share the realtime queue
/// but are never replaceable. Snapshot delta-base-not-in-history
/// fallback is what makes loss tolerable, so routing it through the
/// reliable channel (as we did before this fix) defeated the design.
///
/// The legacy `0x05` LARGE_POSITION and `0x3a` batched-movement
/// packet types were retired in `29f1aa3` — they no longer flow
/// from the server, so they're omitted here.
fn realtime_server_packet(packet: &[u8]) -> bool {
    matches!(
        packet.first().copied(),
        Some(
            0x70 | 0x7d
                | crate::realtime_event::REALTIME_EVENT_BATCH_PACKET
                | crate::connection_quality_protocol::ConnectionQualityProbe::TYPE
        )
    )
}

/// Snapshot state is self-healing and replaceable. Dedicated `0x7d` events
/// are never coalesced because their independent event id and immutable
/// source state make every packet a distinct one-shot fact.
fn replaceable_realtime_server_packet(packet: &[u8]) -> bool {
    packet.first().copied() == Some(0x70)
        && crate::snapshot_wire::SnapshotFrame::parse(packet).is_ok()
}

/// Dedicated authenticated event facts, alone or in a `0x84` batch, go on the
/// unordered-reliable event channel and, while the recipient's link loses
/// packets, also as exact bytes on the game channel. The client deduplicates
/// their independent wrapping id after authenticating the arena stream.
fn weapon_event_server_packet(packet: &[u8]) -> bool {
    matches!(
        packet.first().copied(),
        Some(
            crate::realtime_event::REALTIME_EVENT_PACKET
                | crate::realtime_event::REALTIME_EVENT_BATCH_PACKET
        )
    )
}

/// When an event message also goes on the game channel. The event channel is
/// reliable, so the game copy only saves time when a datagram is lost: the
/// copy that survives arrives on time instead of after SCTP's retransmission,
/// about one round trip later. On a clean link it doubles the event bytes for
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RealtimeEventGameCopy {
    /// Every event message, as before batching.
    Always,
    /// Only while the session's connection-quality probes show loss in their
    /// 30-second window, or have too few samples to show a clean link.
    OnLoss,
    Never,
}

impl RealtimeEventGameCopy {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "always" => Ok(Self::Always),
            "on-loss" => Ok(Self::OnLoss),
            "never" => Ok(Self::Never),
            other => Err(format!(
                "expected realtime event game copy always|on-loss|never, got {other:?}"
            )),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::OnLoss => "on-loss",
            Self::Never => "never",
        }
    }

    fn wanted(self, reading: Option<crate::connection_quality::QualityReading>) -> bool {
        match self {
            Self::Always => true,
            Self::OnLoss => !reading.is_some_and(|reading| reading.shows_clean_link()),
            Self::Never => false,
        }
    }
}

static REALTIME_EVENT_GAME_COPY: Mutex<RealtimeEventGameCopy> =
    Mutex::new(RealtimeEventGameCopy::Always);

/// Applies the configured policy to every session; set once at startup.
pub(crate) fn configure_realtime_event_game_copy(policy: RealtimeEventGameCopy) {
    *REALTIME_EVENT_GAME_COPY
        .lock()
        .expect("realtime event game copy policy") = policy;
}

fn realtime_event_game_copy_wanted(session: &RtcSession) -> bool {
    let policy = *REALTIME_EVENT_GAME_COPY
        .lock()
        .expect("realtime event game copy policy");
    let reading = (policy == RealtimeEventGameCopy::OnLoss)
        .then(|| session.outbound_queue())
        .flatten()
        .and_then(|outbound| outbound.quality.lock().expect("session quality").reading);
    policy.wanted(reading)
}

pub(crate) struct RealtimeEventGameCopies {
    pub(crate) sent: AtomicU64,
    pub(crate) skipped: AtomicU64,
}

pub(crate) static REALTIME_EVENT_GAME_COPIES: RealtimeEventGameCopies = RealtimeEventGameCopies {
    sent: AtomicU64::new(0),
    skipped: AtomicU64::new(0),
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RtcOutboundRoute {
    Game,
    GameAndEvent,
    Reliable,
}

fn rtc_outbound_route(packet: &[u8]) -> RtcOutboundRoute {
    if weapon_event_server_packet(packet) {
        // Exact event bytes are independently consumable and idempotent.
        RtcOutboundRoute::GameAndEvent
    } else if realtime_server_packet(packet) {
        RtcOutboundRoute::Game
    } else {
        RtcOutboundRoute::Reliable
    }
}

fn prioritized_rtc_packets(packets: &mut Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut weapon_events = Vec::new();
    let mut realtime = Vec::new();
    let mut reliable = Vec::new();
    for packet in packets.drain(..) {
        if weapon_event_server_packet(&packet) {
            weapon_events.push(packet);
        } else if realtime_server_packet(&packet) {
            realtime.push(packet);
        } else {
            reliable.push(packet);
        }
    }
    weapon_events.extend(realtime);
    let mut realtime = weapon_events;
    realtime.extend(reliable);
    realtime
}

fn send_rtc_packet(
    handle: &tokio::runtime::Handle,
    session: &Arc<RtcSession>,
    lanes: &writer::LaneWriter,
    packet: &[u8],
) -> bool {
    let (game, event, reliable) = {
        let channels = session.channels.lock().expect("rtc channels");
        (
            channels.game.clone(),
            channels.event.clone(),
            channels.reliable.clone(),
        )
    };
    let route = rtc_outbound_route(packet);
    let game_copy =
        route == RtcOutboundRoute::GameAndEvent && realtime_event_game_copy_wanted(session);
    let quality_probe =
        packet.first() == Some(&crate::connection_quality_protocol::ConnectionQualityProbe::TYPE);
    // Only readiness lookup runs on the dispatcher. Capacity waits belong to
    // independent lane tasks, never this thread or the simulation lock.
    let destinations = handle.block_on(async {
        let mut destinations = Vec::with_capacity(2);
        match route {
            RtcOutboundRoute::GameAndEvent => {
                let event = open_rtc_channel(event).await;
                // Without an open event channel the game channel carries it.
                if (game_copy || event.is_none()) && open_rtc_channel(game).await.is_some() {
                    destinations.push(0);
                }
                if event.is_some() {
                    destinations.push(1);
                    let copies = &REALTIME_EVENT_GAME_COPIES;
                    let counter = if destinations.contains(&0) {
                        &copies.sent
                    } else {
                        &copies.skipped
                    };
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                if destinations.is_empty() && open_rtc_channel(reliable).await.is_some() {
                    destinations.push(2);
                }
            }
            RtcOutboundRoute::Game => {
                if open_rtc_channel(game).await.is_some() {
                    destinations.push(0);
                } else if !quality_probe && open_rtc_channel(reliable).await.is_some() {
                    destinations.push(2);
                }
            }
            RtcOutboundRoute::Reliable => {
                if open_rtc_channel(reliable).await.is_some() {
                    destinations.push(2);
                }
            }
        }
        destinations
    });
    if destinations.is_empty() {
        if let Some(outbound) = session.outbound_queue() {
            outbound.release_unsent(packet);
            if quality_probe {
                outbound
                    .quality
                    .lock()
                    .expect("session quality")
                    .cancel_queued();
            }
        }
        if quality_probe {
            return true;
        }
        if matches!(
            *session.lifecycle.lock().expect("RTC session lifecycle"),
            RtcSessionLifecycle::Closing | RtcSessionLifecycle::Closed
        ) {
            return false;
        }
        warn!(peer = %session.peer, ?route, packet_len = packet.len(), "server rtc route_unroutable");
        disconnect_rtc_session(session);
        return false;
    }
    let Some(outbound) = session.outbound_queue() else {
        return false;
    };
    let packet = Arc::new(packet.to_vec());
    for index in destinations {
        if !enqueue_rtc_lane(&outbound, lanes, index, Arc::clone(&packet)) {
            return false;
        }
    }
    true
}

fn enqueue_rtc_lane(
    outbound: &RtcOutboundQueue,
    lanes: &writer::LaneWriter,
    index: usize,
    packet: Arc<Vec<u8>>,
) -> bool {
    let Err(reason) = lanes.enqueue(index, Arc::clone(&packet)) else {
        return true;
    };
    let quality_probe =
        packet.first() == Some(&crate::connection_quality_protocol::ConnectionQualityProbe::TYPE);
    if quality_probe {
        // A measurement which never reached the channel is never path loss.
        outbound
            .quality
            .lock()
            .expect("session quality")
            .cancel_queued();
    }
    match reason {
        writer::EnqueueError::Closed => {
            // A dispatcher packet may already be in hand when a worker sees
            // Closing. Stop and clear retention without inventing pressure.
            // An unexpectedly closed writer must still recover its session.
            outbound.close();
            outbound.trigger_session_cleanup();
            false
        }
        writer::EnqueueError::Capacity if quality_probe => true,
        writer::EnqueueError::Capacity => {
            outbound.fail_reliable_overflow();
            false
        }
    }
}

async fn open_rtc_channel(channel: Option<Arc<dyn DataChannel>>) -> Option<Arc<dyn DataChannel>> {
    let is_open = if let Some(channel) = channel.as_ref() {
        matches!(channel.ready_state().await, Ok(RTCDataChannelState::Open))
    } else {
        false
    };
    is_open.then_some(channel).flatten()
}

fn keep_peer_alive(
    runtime: &'static tokio::runtime::Runtime,
    peer: Arc<dyn PeerConnection>,
    binding: RtcUdpBinding,
    session: Arc<RtcSession>,
) {
    runtime.spawn(async move {
        let mut datagrams = SessionDatagramTally::default();
        {
            let closed = session.closed.notified();
            tokio::pin!(closed);
            loop {
                tokio::select! {
                    () = &mut closed => break,
                    () = tokio::time::sleep(RTC_DATAGRAM_POLL_INTERVAL) => {
                        datagrams.poll(peer.as_ref()).await;
                    }
                }
            }
        }
        datagrams.poll(peer.as_ref()).await;
        close_rtc_peer_and_release(peer, binding, session, "server RTC peer").await;
    });
}

/// UDP datagrams and bytes above UDP on every session's selected candidate
/// pair, from each peer connection's own stats. STUN checks are not counted.
/// Next to the DataChannel message counters these show how many messages
/// share a datagram and how much DTLS and SCTP framing (SACKs included) costs.
pub(crate) struct RtcDatagramTotals {
    pub(crate) sent: AtomicU64,
    pub(crate) received: AtomicU64,
    pub(crate) bytes_sent: AtomicU64,
    pub(crate) bytes_received: AtomicU64,
}

pub(crate) static RTC_DATAGRAMS: RtcDatagramTotals = RtcDatagramTotals {
    sent: AtomicU64::new(0),
    received: AtomicU64::new(0),
    bytes_sent: AtomicU64::new(0),
    bytes_received: AtomicU64::new(0),
};

const RTC_DATAGRAM_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// One session's totals at its last poll, so each poll adds only the change.
#[derive(Default)]
struct SessionDatagramTally {
    sent: u64,
    received: u64,
    bytes_sent: u64,
    bytes_received: u64,
}

impl SessionDatagramTally {
    async fn poll(&mut self, peer: &dyn PeerConnection) {
        let report = peer.get_stats(Instant::now(), StatsSelector::None).await;
        let mut now = Self::default();
        for entry in report.iter() {
            if let RTCStatsReportEntry::IceCandidatePair(pair) = entry {
                now.sent += pair.packets_sent;
                now.received += pair.packets_received;
                now.bytes_sent += pair.bytes_sent;
                now.bytes_received += pair.bytes_received;
            }
        }
        self.add_change_to(&now, &RTC_DATAGRAMS);
        *self = now;
    }

    fn add_change_to(&self, now: &Self, totals: &RtcDatagramTotals) {
        let add = |total: &AtomicU64, before: u64, after: u64| {
            total.fetch_add(after.saturating_sub(before), Ordering::Relaxed);
        };
        add(&totals.sent, self.sent, now.sent);
        add(&totals.received, self.received, now.received);
        add(&totals.bytes_sent, self.bytes_sent, now.bytes_sent);
        add(
            &totals.bytes_received,
            self.bytes_received,
            now.bytes_received,
        );
    }
}

async fn close_rtc_peer_and_release(
    peer: Arc<dyn PeerConnection>,
    binding: RtcUdpBinding,
    session: Arc<RtcSession>,
    log_context: &'static str,
) {
    // Teardown owns only this port's lifecycle. A stuck peer or externally
    // retained socket can degrade this one slot without blocking builds or
    // cleanup on every other configured port.
    let _port_lifecycle = if let Some(fixed) = binding.fixed.as_ref() {
        Some(Arc::clone(&fixed.lifecycle).lock_owned().await)
    } else {
        None
    };

    match tokio::time::timeout(RTC_PEER_CLOSE_TIMEOUT, peer.close()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!(error = %error, "{log_context} close failed"),
        Err(_) => warn!(
            timeout_ms = RTC_PEER_CLOSE_TIMEOUT.as_millis(),
            "{log_context} close timed out"
        ),
    }
    // webrtc-rs aborts its default driver task from close(); dropping the last
    // application peer reference lets that cancelled task and its UDP socket
    // finish tearing down on the runtime.
    drop(peer);

    let Some(fixed) = binding.fixed else {
        session.admission.release();
        return;
    };
    match wait_for_rtc_udp_socket_reuse(&binding.address, RTC_UDP_REUSE_TIMEOUT).await {
        Ok(()) => {
            session.admission.release_fixed_port(fixed.port);
        }
        Err(error) => {
            warn!(
                peer = %session.peer,
                port = fixed.port,
                address = binding.address,
                error = %error,
                "server RTC UDP port did not recover before teardown deadline; quarantining capacity"
            );
            session.admission.quarantine_fixed_port(fixed.port);
            spawn_rtc_udp_port_recovery(Arc::clone(&session.state), fixed, binding.address);
        }
    }
}

async fn wait_for_rtc_udp_socket_reuse(
    bind_address: &str,
    timeout: Duration,
) -> std::io::Result<()> {
    let started = tokio::time::Instant::now();
    loop {
        match std::net::UdpSocket::bind(bind_address) {
            Ok(probe) => {
                drop(probe);
                return Ok(());
            }
            Err(error) => {
                if started.elapsed() >= timeout {
                    return Err(error);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn quarantine_rtc_udp_port(
    state: &Arc<Mutex<ServerState>>,
    fixed: FixedRtcUdpPort,
    bind_address: String,
    error: &std::io::Error,
) {
    let inserted = {
        state
            .lock()
            .expect("server state should not be poisoned")
            .rtc_udp_quarantined_ports
            .insert(fixed.port)
    };
    if inserted {
        warn!(
            port = fixed.port,
            address = bind_address,
            error = %error,
            "configured RTC UDP port is unavailable; quarantining capacity"
        );
    }
    // Scheduling is intentionally attempted even when the port was already
    // quarantined. The in-progress registry deduplicates a live owner while
    // allowing admission traffic to restart a cancelled recovery worker.
    spawn_rtc_udp_port_recovery(Arc::clone(state), fixed, bind_address);
}

#[derive(Clone, Copy)]
struct RtcUdpRecoveryPolicy {
    probe_interval: Duration,
    attempts_per_cycle: usize,
    initial_backoff: Duration,
    max_backoff: Duration,
}

impl Default for RtcUdpRecoveryPolicy {
    fn default() -> Self {
        Self {
            probe_interval: RTC_UDP_RECOVERY_INTERVAL,
            attempts_per_cycle: RTC_UDP_RECOVERY_ATTEMPTS_PER_CYCLE,
            initial_backoff: RTC_UDP_RECOVERY_INITIAL_BACKOFF,
            max_backoff: RTC_UDP_RECOVERY_MAX_BACKOFF,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RtcUdpRecoveryOutcome {
    Recovered,
    NoLongerQuarantined,
    StateDropped,
}

struct RtcUdpRecoveryRegistration {
    state: Weak<Mutex<ServerState>>,
    port: u16,
    wake: Arc<Notify>,
}

impl RtcUdpRecoveryRegistration {
    fn acquire(state: &Arc<Mutex<ServerState>>, port: u16) -> Option<Self> {
        let mut guard = state.lock().expect("server state should not be poisoned");
        if !guard.rtc_udp_quarantined_ports.contains(&port) {
            return None;
        }
        if let Some(wake) = guard.rtc_udp_recovery_ports.get(&port) {
            wake.notify_one();
            return None;
        }
        let wake = Arc::new(Notify::new());
        guard.rtc_udp_recovery_ports.insert(port, Arc::clone(&wake));
        Some(Self {
            state: Arc::downgrade(state),
            port,
            wake,
        })
    }
}

impl Drop for RtcUdpRecoveryRegistration {
    fn drop(&mut self) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        if let Ok(mut state) = state.lock() {
            let owns_entry = state
                .rtc_udp_recovery_ports
                .get(&self.port)
                .is_some_and(|wake| Arc::ptr_eq(wake, &self.wake));
            if owns_entry {
                state.rtc_udp_recovery_ports.remove(&self.port);
            }
        }
    }
}

fn spawn_rtc_udp_port_recovery(
    state: Arc<Mutex<ServerState>>,
    fixed: FixedRtcUdpPort,
    bind_address: String,
) {
    let _ = spawn_rtc_udp_port_recovery_with_probe(
        state,
        fixed,
        bind_address,
        RtcUdpRecoveryPolicy::default(),
        |address| std::net::UdpSocket::bind(address).map(drop),
    );
}

fn spawn_rtc_udp_port_recovery_with_probe<Probe>(
    state: Arc<Mutex<ServerState>>,
    fixed: FixedRtcUdpPort,
    bind_address: String,
    policy: RtcUdpRecoveryPolicy,
    probe: Probe,
) -> Option<tokio::task::JoinHandle<RtcUdpRecoveryOutcome>>
where
    Probe: FnMut(&str) -> std::io::Result<()> + Send + 'static,
{
    let registration = RtcUdpRecoveryRegistration::acquire(&state, fixed.port)?;
    let state = Arc::downgrade(&state);
    Some(tokio::spawn(async move {
        run_rtc_udp_port_recovery(state, fixed, bind_address, policy, probe, registration).await
    }))
}

async fn run_rtc_udp_port_recovery<Probe>(
    state: Weak<Mutex<ServerState>>,
    fixed: FixedRtcUdpPort,
    bind_address: String,
    policy: RtcUdpRecoveryPolicy,
    mut probe: Probe,
    registration: RtcUdpRecoveryRegistration,
) -> RtcUdpRecoveryOutcome
where
    Probe: FnMut(&str) -> std::io::Result<()>,
{
    debug_assert!(policy.attempts_per_cycle > 0);
    let mut cycle = 1_usize;
    let mut backoff = policy.initial_backoff;
    loop {
        let mut last_error = None;
        for attempt in 1..=policy.attempts_per_cycle {
            tokio::time::sleep(policy.probe_interval).await;
            let Some(state) = state.upgrade() else {
                return RtcUdpRecoveryOutcome::StateDropped;
            };
            let _port_lifecycle = Arc::clone(&fixed.lifecycle).lock_owned().await;
            if !state
                .lock()
                .expect("server state should not be poisoned")
                .rtc_udp_quarantined_ports
                .contains(&fixed.port)
            {
                return RtcUdpRecoveryOutcome::NoLongerQuarantined;
            }
            match probe(&bind_address) {
                Ok(()) => {
                    let recovered = state
                        .lock()
                        .expect("server state should not be poisoned")
                        .rtc_udp_quarantined_ports
                        .remove(&fixed.port);
                    if recovered {
                        info!(
                            port = fixed.port,
                            cycle,
                            attempt,
                            "quarantined RTC UDP port recovered; effective capacity restored"
                        );
                    }
                    return if recovered {
                        RtcUdpRecoveryOutcome::Recovered
                    } else {
                        RtcUdpRecoveryOutcome::NoLongerQuarantined
                    };
                }
                Err(error) => last_error = Some(error),
            }
        }
        warn!(
            port = fixed.port,
            cycle,
            attempts = policy.attempts_per_cycle,
            retry_backoff_ms = backoff.as_millis(),
            error = %last_error.expect("a nonempty recovery cycle must record an error"),
            "quarantined RTC UDP port remains unavailable after bounded recovery cycle"
        );
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = registration.wake.notified() => {
                debug!(
                    port = fixed.port,
                    cycle,
                    "admission activity requested an opportunistic RTC UDP recovery probe"
                );
            }
        }
        backoff = backoff.saturating_mul(2).min(policy.max_backoff);
        cycle = cycle.saturating_add(1);
    }
}

static RTC_RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();

#[cfg(test)]
static TEST_RTC_SESSIONS: OnceLock<Mutex<HashMap<String, Weak<RtcSession>>>> = OnceLock::new();

#[cfg(test)]
fn observe_test_rtc_session(session: &Arc<RtcSession>) {
    TEST_RTC_SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("test RTC session observer should not be poisoned")
        .insert(session.peer.clone(), Arc::downgrade(session));
}

#[cfg(test)]
fn take_test_rtc_session(peer: &str) -> Weak<RtcSession> {
    TEST_RTC_SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("test RTC session observer should not be poisoned")
        .remove(peer)
        .expect("test RTC session should have been observed")
}

fn rtc_runtime() -> Result<&'static tokio::runtime::Runtime, RtcSignalingError> {
    match RTC_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("zubspace-rtc")
            .build()
            .map_err(|err| format!("failed to create shared WebRTC runtime: {err}"))
    }) {
        Ok(runtime) => Ok(runtime),
        Err(err) => Err(RtcSignalingError::internal(err.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use webrtc::peer_connection::RTCSdpType;

    #[test]
    fn datagram_tally_adds_only_each_session_change_to_the_totals() {
        let totals = RtcDatagramTotals {
            sent: AtomicU64::new(0),
            received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
        };
        let polls = [
            SessionDatagramTally {
                sent: 10,
                received: 12,
                bytes_sent: 1_000,
                bytes_received: 900,
            },
            SessionDatagramTally {
                sent: 25,
                received: 20,
                bytes_sent: 2_500,
                bytes_received: 1_800,
            },
            // A pair that disappears from the report must not subtract.
            SessionDatagramTally {
                sent: 5,
                received: 20,
                bytes_sent: 400,
                bytes_received: 1_800,
            },
        ];
        let mut last = SessionDatagramTally::default();
        for poll in polls {
            last.add_change_to(&poll, &totals);
            last = poll;
        }
        let load = |total: &AtomicU64| total.load(Ordering::Relaxed);
        assert_eq!(load(&totals.sent), 25);
        assert_eq!(load(&totals.received), 20);
        assert_eq!(load(&totals.bytes_sent), 2_500);
        assert_eq!(load(&totals.bytes_received), 1_800);
    }

    #[test]
    fn ingress_turn_matches_fifo_model_and_retains_queued_accounting() {
        for count in [1, 2, 63, 64, 65, 129] {
            let metrics = Arc::new(RtcOverloadMetrics::default());
            let (queue, mut receiver) =
                RtcInboundQueue::with_capacity(Arc::clone(&metrics), RtcChannelKind::Game, count);
            for id in 0..count {
                queue.try_enqueue(vec![id as u8]).unwrap();
            }
            let mut observed = Vec::new();
            let first = receiver.try_recv().unwrap();
            let drained =
                drain_rtc_ingress_turn(first, &mut receiver, |packet| observed.push(packet[0]));
            assert_eq!(drained, count.min(64));
            assert_eq!(
                observed,
                (0..count.min(64)).map(|id| id as u8).collect::<Vec<_>>()
            );
            assert_eq!(metrics.snapshot().inbound_queue_packets[0], count - drained);
            drop(receiver);
            assert_eq!(metrics.snapshot().inbound_queue_packets[0], 0);
            assert_eq!(
                queue.try_enqueue(vec![255]),
                Err(RtcInboundEnqueueError::Closed)
            );
            assert_eq!(metrics.snapshot().inbound_queue_packets[0], 0);
        }
    }

    #[test]
    fn ingress_turn_yields_even_when_producer_keeps_queue_nonempty() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let (queue, mut receiver) =
            RtcInboundQueue::with_capacity(Arc::clone(&metrics), RtcChannelKind::Game, 1);
        queue.try_enqueue(vec![0]).unwrap();
        let first = receiver.try_recv().unwrap();
        let mut observed = Vec::new();
        let count = drain_rtc_ingress_turn(first, &mut receiver, |packet| {
            observed.push(packet[0]);
            queue.try_enqueue(vec![packet[0] + 1]).unwrap();
        });
        assert_eq!(count, 64);
        assert_eq!(observed, (0..64).collect::<Vec<u8>>());
        assert_eq!(metrics.snapshot().inbound_queue_packets[0], 1);
        assert_eq!(receiver.try_recv(), Some(vec![64]));
        assert_eq!(metrics.snapshot().inbound_queue_packets[0], 0);
    }

    #[tokio::test]
    async fn ingress_turn_panic_closes_receiver_and_clears_backlog() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let (queue, mut receiver) =
            RtcInboundQueue::with_capacity(Arc::clone(&metrics), RtcChannelKind::Game, 3);
        for id in 0..3 {
            queue.try_enqueue(vec![id]).unwrap();
        }
        let first = receiver.try_recv().unwrap();
        let executor = RtcIngressExecutor::new(1);
        let result = executor
            .execute(move || {
                drain_rtc_ingress_turn(first, &mut receiver, |_| panic!("injected drain failure"));
            })
            .await;
        assert_eq!(result, Err(RtcIngressExecutionError::Panicked));
        assert_eq!(metrics.snapshot().inbound_queue_packets[0], 0);
        assert_eq!(
            queue.try_enqueue(vec![9]),
            Err(RtcInboundEnqueueError::Closed)
        );
        assert_eq!(metrics.snapshot().inbound_queue_packets[0], 0);
        assert_eq!(executor.status(), (1, 0, 0));
    }

    #[tokio::test]
    async fn ingress_turn_cancelled_caller_drops_returned_receiver() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let (queue, mut receiver) =
            RtcInboundQueue::with_capacity(Arc::clone(&metrics), RtcChannelKind::Game, 65);
        for id in 0..65 {
            queue.try_enqueue(vec![id]).unwrap();
        }
        let first = receiver.try_recv().unwrap();
        let executor = Arc::new(RtcIngressExecutor::new(1));
        let worker = Arc::clone(&executor);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let caller = tokio::spawn(async move {
            worker
                .execute(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    drain_rtc_ingress_turn(first, &mut receiver, |_| {});
                    receiver
                })
                .await
        });
        started_rx.await.unwrap();
        caller.abort();
        let cancelled = matches!(caller.await, Err(error) if error.is_cancelled());
        let held = executor.status();
        let queued = metrics.snapshot().inbound_queue_packets[0];
        release_tx.send(()).unwrap();
        // Same single worker must finish sending/dropping the abandoned
        // receiver before it executes this barrier job.
        executor.execute(|| {}).await.unwrap();
        assert!(cancelled);
        assert_eq!(held, (1, 1, 0));
        assert_eq!(queued, 64);
        assert_eq!(
            queue.try_enqueue(vec![99]),
            Err(RtcInboundEnqueueError::Closed)
        );
        assert_eq!(metrics.snapshot().inbound_queue_packets[0], 0);
        assert_eq!(executor.status(), (1, 0, 0));
    }

    #[test]
    fn blocked_ingress_handler_leaves_runtime_worker_responsive() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let identity = register_connection_with_sender(&state, sender.into());
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "blocked-ingress".into(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Registered(identity)),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(identity)).0,
            joined: Mutex::new(false),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(None),
            metrics,
            closed: Arc::new(Notify::new()),
            admission,
        });
        let gate = state.lock().unwrap().arena_transaction_gate.clone();
        let held = gate.write().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let observed_session = Arc::clone(&session);
        let handler = runtime.spawn(async move {
            entered_tx.send(()).unwrap();
            let (_queue, receiver) = RtcInboundQueue::with_capacity(
                Arc::clone(&session.metrics),
                RtcChannelKind::Game,
                1,
            );
            dispatch_rtc_ingress_turn(
                &RtcIngressExecutor::new(1),
                session,
                GAME_CHANNEL_LABEL.into(),
                vec![0xff],
                receiver,
            )
            .await;
        });
        let entered = entered_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        let deadline = Instant::now() + Duration::from_secs(2);
        let blocked_in_handler = loop {
            if matches!(
                observed_session.joined.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            thread::yield_now();
        };
        let (heartbeat_tx, heartbeat_rx) = std::sync::mpsc::channel();
        let heartbeat = runtime.spawn(async move {
            heartbeat_tx.send(()).unwrap();
        });
        let responsive = heartbeat_rx
            .recv_timeout(Duration::from_millis(500))
            .is_ok();
        // Always unblock and join both tasks before asserting, including red
        // runs, so this regression cannot leave a wedged runtime behind.
        drop(held);
        runtime.block_on(async {
            handler.await.unwrap();
            heartbeat.await.unwrap();
        });
        assert!(entered, "handler task did not start");
        assert!(
            blocked_in_handler,
            "handler did not reach the held arena gate"
        );
        assert!(
            responsive,
            "arena-gate waiting occupied the networking worker"
        );
    }

    #[test]
    fn slow_handler_trace_excludes_positions_empty_and_fast_packets() {
        for kind in [None, Some(crate::arena::CLIENT_POSITION)] {
            assert!(!slow_rtc_handler_trace_candidate(
                kind,
                Duration::from_secs(1)
            ));
        }
        assert!(!slow_rtc_handler_trace_candidate(
            Some(0),
            Duration::from_micros(4_999)
        ));
        assert!(slow_rtc_handler_trace_candidate(
            Some(0),
            Duration::from_micros(5_000)
        ));
        assert!(slow_rtc_handler_trace_candidate(
            Some(0x77),
            Duration::from_millis(100)
        ));
    }

    #[tokio::test]
    async fn ingress_executor_cancellation_keeps_running_work_bounded() {
        use std::future::Future;
        let executor = Arc::new(RtcIngressExecutor::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_executor = Arc::clone(&executor);
        let first = tokio::spawn(async move {
            first_executor
                .execute(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                })
                .await
        });
        started_rx.await.unwrap();
        first.abort();
        let cancelled = first.await.unwrap_err().is_cancelled();
        let held = executor.status();
        let second_ran = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&second_ran);
        let mut second = Box::pin(executor.execute(move || {
            observed.store(true, Ordering::SeqCst);
        }));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let pending = second.as_mut().poll(&mut context).is_pending();
        let waiting_status = executor.status();
        drop(second);
        let cancelled_waiter_status = executor.status();
        // Release the blocking closure before assertions so a failing bound
        // cannot wedge runtime teardown. Dropping the pending waiter must
        // not leave its work queued in the blocking pool.
        release_tx.send(()).unwrap();
        executor.execute(|| {}).await.unwrap();
        assert!(cancelled);
        assert_eq!(held, (1, 1, 0));
        assert!(pending);
        assert_eq!(waiting_status, (1, 1, 1));
        assert_eq!(cancelled_waiter_status, (1, 1, 0));
        assert!(!second_ran.load(Ordering::SeqCst));
        assert_eq!(executor.status(), (1, 0, 0));
    }

    #[tokio::test]
    async fn ingress_executor_panic_releases_execution_slot() {
        let executor = RtcIngressExecutor::new(1);
        assert!(
            executor
                .execute(|| panic!("injected handler failure"))
                .await
                .unwrap_err()
                .is_panic()
        );
        executor.execute(|| {}).await.unwrap();
        assert_eq!(executor.permits.available_permits(), 1);
    }

    #[tokio::test]
    async fn ingress_executor_reuses_bounded_workers_and_closes_them() {
        let executor = RtcIngressExecutor::new(3);
        let workers = Arc::clone(&executor.workers);
        let observed_threads = Arc::new(Mutex::new(HashSet::new()));
        for _ in 0..64 {
            let observed = Arc::clone(&observed_threads);
            executor
                .execute(move || {
                    assert_eq!(thread::current().name(), Some("zubspace-input"));
                    // Dedicated execution must retain the submitting runtime.
                    let _ = tokio::runtime::Handle::current();
                    observed.lock().unwrap().insert(thread::current().id());
                })
                .await
                .unwrap();
        }
        assert_eq!(executor.worker_count(), 3);
        assert!(observed_threads.lock().unwrap().len() <= 3);
        assert_eq!(executor.status(), (3, 0, 0));
        drop(executor);
        tokio::time::timeout(Duration::from_secs(2), async {
            while workers.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all persistent ingress workers exit after sender closure");
    }

    #[test]
    fn slow_handler_trace_budget_is_bounded_and_reports_suppression() {
        let start = Instant::now();
        let mut budget = RtcSlowHandlerTraceBudget::new(start);
        for _ in 0..16 {
            assert_eq!(budget.admit(start), Some(0));
        }
        assert_eq!(budget.admit(start + Duration::from_millis(999)), None);
        assert_eq!(budget.admit(start + Duration::from_millis(999)), None);
        assert_eq!(budget.admit(start + Duration::from_secs(1)), Some(2));
        assert_eq!(budget.admit(start + Duration::from_secs(1)), Some(0));
        // An older observation cannot reopen a spent window.
        for _ in 0..14 {
            assert_eq!(budget.admit(start), Some(0));
        }
        assert_eq!(budget.admit(start), None);
        assert_eq!(budget.admit(start + Duration::from_secs(2)), Some(1));
    }

    fn current_input_record(sequence: u16, bits: u16, direction: u8) -> Vec<u8> {
        let mut packet = vec![
            crate::connection::CLIENT_INPUT_RECORD_TYPE,
            sequence as u8,
            (sequence >> 8) as u8,
            1,
            bits as u8,
            (bits >> 8) as u8,
            direction,
            0,
        ];
        packet.extend_from_slice(
            &crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence).encode(),
        );
        packet
    }

    #[test]
    fn game_channel_is_unordered_and_unreliable() {
        let init = game_data_channel_init();
        assert!(!init.ordered);
        assert!(init.max_retransmits.is_none());
        assert_eq!(
            init.max_packet_life_time,
            Some(GAME_CHANNEL_MAX_PACKET_LIFETIME_MS)
        );
    }

    #[test]
    fn reliable_channel_is_ordered_reliable() {
        let init = reliable_data_channel_init();
        assert!(init.ordered);
        assert!(init.max_retransmits.is_none());
        assert!(init.max_packet_life_time.is_none());
    }

    #[test]
    fn event_channel_is_unordered_reliable() {
        let init = event_data_channel_init();
        assert!(!init.ordered);
        assert!(init.max_retransmits.is_none());
        assert!(init.max_packet_life_time.is_none());
    }

    #[test]
    fn inbound_handoff_preserves_fifo_and_accounts_each_channel_overflow() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        for (kind, index) in [
            (RtcChannelKind::Game, 0),
            (RtcChannelKind::Event, 1),
            (RtcChannelKind::Reliable, 2),
        ] {
            let (queue, mut receiver) =
                RtcInboundQueue::with_capacity(Arc::clone(&metrics), kind, 2);
            assert_eq!(queue.try_enqueue(vec![index as u8, 1]), Ok(()));
            assert_eq!(queue.try_enqueue(vec![index as u8, 2]), Ok(()));
            assert_eq!(
                queue.try_enqueue(vec![index as u8, 3]),
                Err(RtcInboundEnqueueError::Full),
            );

            let snapshot = metrics.snapshot();
            assert_eq!(snapshot.inbound_queue_packets[index], 2);
            assert_eq!(snapshot.inbound_peak_queue_packets[index], 2);
            assert_eq!(snapshot.inbound_backpressure[index], 1);
            for expected in [vec![index as u8, 1], vec![index as u8, 2]] {
                assert_eq!(receiver.try_recv(), Some(expected));
            }
            assert_eq!(metrics.snapshot().inbound_queue_packets[index], 0);
        }
    }

    #[test]
    fn inbound_overflow_disconnects_instead_of_silently_dropping_reliable_gameplay() {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let identity = register_connection_with_sender(&state, sender.into());
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "reliable-ingress-overflow".to_string(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Registered(identity)),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(identity)).0,
            joined: Mutex::new(true),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(None),
            metrics: Arc::clone(&metrics),
            closed: Arc::new(Notify::new()),
            admission,
        });
        let (ingress, mut receiver) =
            RtcInboundQueue::with_capacity(Arc::clone(&metrics), RtcChannelKind::Reliable, 1);
        assert!(enqueue_rtc_ingress_message(
            &session,
            RELIABLE_CHANNEL_LABEL,
            &ingress,
            vec![1],
        ));
        assert!(!enqueue_rtc_ingress_message(
            &session,
            RELIABLE_CHANNEL_LABEL,
            &ingress,
            vec![2],
        ));
        assert_eq!(
            *session.lifecycle.lock().unwrap(),
            RtcSessionLifecycle::Closed,
        );
        assert!(state.lock().unwrap().clients.is_empty());
        assert_eq!(metrics.snapshot().inbound_backpressure, [0, 0, 1]);
        assert_eq!(receiver.try_recv(), Some(vec![1]));
        assert_eq!(metrics.snapshot().inbound_queue_packets, [0, 0, 0]);
        session.admission.release();
    }

    #[test]
    fn rtc_channel_protocol_accepts_exact_supported_three_channel_setup() {
        let mut protocol = RtcChannelProtocol::default();
        assert_eq!(
            protocol.announce(&valid_channel_metadata(GAME_CHANNEL_LABEL)),
            Ok(RtcChannelKind::Game)
        );
        assert_eq!(
            protocol.announce(&valid_channel_metadata(EVENT_CHANNEL_LABEL)),
            Ok(RtcChannelKind::Event)
        );
        assert_eq!(
            protocol.announce(&valid_channel_metadata(RELIABLE_CHANNEL_LABEL)),
            Ok(RtcChannelKind::Reliable)
        );
        assert!(!protocol.mark_open(RtcChannelKind::Game));
        assert!(!protocol.mark_open(RtcChannelKind::Event));
        assert!(protocol.mark_open(RtcChannelKind::Reliable));
        assert!(protocol.all_open());
    }

    #[test]
    fn rtc_channel_protocol_rejects_duplicate_and_unknown_labels() {
        let mut duplicate = RtcChannelProtocol::default();
        duplicate
            .announce(&valid_channel_metadata(GAME_CHANNEL_LABEL))
            .unwrap();
        assert!(
            duplicate
                .announce(&valid_channel_metadata(GAME_CHANNEL_LABEL))
                .unwrap_err()
                .contains("duplicate")
        );

        let mut unknown = RtcChannelProtocol::default();
        let mut metadata = valid_channel_metadata(GAME_CHANNEL_LABEL);
        metadata.label = "attacker-channel".to_string();
        assert!(unknown.announce(&metadata).unwrap_err().contains("unknown"));
    }

    #[test]
    fn rtc_channel_protocol_caps_excessive_channel_creation() {
        let mut protocol = RtcChannelProtocol::default();
        protocol
            .announce(&valid_channel_metadata(GAME_CHANNEL_LABEL))
            .unwrap();
        protocol
            .announce(&valid_channel_metadata(EVENT_CHANNEL_LABEL))
            .unwrap();
        protocol
            .announce(&valid_channel_metadata(RELIABLE_CHANNEL_LABEL))
            .unwrap();
        assert!(
            protocol
                .announce(&valid_channel_metadata(GAME_CHANNEL_LABEL))
                .unwrap_err()
                .contains("excessive")
        );
    }

    #[test]
    fn rtc_channel_protocol_rejects_attribute_variants() {
        let mut protocol = RtcChannelProtocol::default();
        let mut game = valid_channel_metadata(GAME_CHANNEL_LABEL);
        game.max_packet_life_time = Some(GAME_CHANNEL_MAX_PACKET_LIFETIME_MS - 1);
        assert!(protocol.announce(&game).unwrap_err().contains("attributes"));

        let mut protocol = RtcChannelProtocol::default();
        let mut reliable = valid_channel_metadata(RELIABLE_CHANNEL_LABEL);
        reliable.ordered = false;
        assert!(
            protocol
                .announce(&reliable)
                .unwrap_err()
                .contains("attributes")
        );
    }

    #[test]
    fn rtc_channel_protocol_rejects_missing_game_lifetime() {
        let mut protocol = RtcChannelProtocol::default();
        let mut game = valid_channel_metadata(GAME_CHANNEL_LABEL);
        game.max_packet_life_time = None;
        assert!(protocol.announce(&game).unwrap_err().contains("attributes"));
    }

    #[test]
    fn rtc_channel_protocol_violation_disconnects_without_session_or_client_leak() {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let player_id = register_connection_with_sender(&state, sender.into());
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "protocol-violator".to_string(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Registered(player_id)),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(player_id)).0,
            joined: Mutex::new(false),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(None),
            metrics: Arc::clone(&metrics),
            closed: Arc::new(Notify::new()),
            admission,
        });

        reject_rtc_channel_protocol(&session, "duplicate DataChannel label");
        assert_eq!(
            *session.lifecycle.lock().unwrap(),
            RtcSessionLifecycle::Closed
        );
        assert!(state.lock().unwrap().clients.is_empty());
        assert_eq!(metrics.snapshot().protocol_violations, 1);
        drop(session);
        assert_eq!(state.lock().unwrap().rtc_sessions, 0);
    }

    #[test]
    fn reliable_core_disconnect_closes_registered_rtc_session_immediately() {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let identity = register_connection_with_sender(&state, sender.into());
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "core-disconnect-peer".to_string(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Registered(identity)),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(identity)).0,
            joined: Mutex::new(true),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(None),
            metrics,
            closed: Arc::new(Notify::new()),
            admission,
        });

        handle_rtc_packet(
            &session,
            RELIABLE_CHANNEL_LABEL,
            vec![crate::CORE_BYTE, 0x07],
        );

        assert_eq!(
            *session.lifecycle.lock().unwrap(),
            RtcSessionLifecycle::Closed,
        );
        assert!(state.lock().unwrap().clients.is_empty());
        drop(session);
        assert_eq!(state.lock().unwrap().rtc_sessions, 0);
    }

    #[tokio::test]
    async fn disconnected_rtc_suspends_quality_without_retiring_the_session() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let outbound = Arc::new(RtcOutboundQueue::new(Arc::clone(&metrics)));
        let identity =
            register_connection_with_sender(&state, ClientPacketSender::bounded(outbound.clone()));
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "quality-disconnected".into(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Registered(identity)),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(identity)).0,
            joined: Mutex::new(true),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(Some(Arc::clone(&outbound))),
            metrics,
            closed: Arc::new(Notify::new()),
            admission,
        });
        let handler = RtcEventHandler {
            session: Arc::clone(&session),
            gather_complete_tx: tokio::sync::mpsc::channel(1).0,
            executor: Arc::new(RtcIngressExecutor::new(1)),
        };
        let now = Instant::now();
        outbound.quality.lock().unwrap().reset_scope(1, now);
        let queued = outbound
            .quality
            .lock()
            .unwrap()
            .reserve_probe(1, now)
            .unwrap();
        handler
            .on_connection_state_change(RTCPeerConnectionState::Disconnected)
            .await;
        {
            let mut quality = outbound.quality.lock().unwrap();
            assert!(
                quality.poll(now, true).is_none(),
                "offline probes must not create a persistent quality hold"
            );
            assert!(quality.reserve_probe(2, now).is_none());
            assert!(!quality.begin_dispatch(queued, now));
            quality.reset_scope(2, now);
            assert!(
                quality.poll(now, true).is_none(),
                "arena reset cannot revive an offline measurement window"
            );
        }
        assert!(state.lock().unwrap().player_session_matches(identity));
        assert_eq!(
            *session.lifecycle.lock().unwrap(),
            RtcSessionLifecycle::Registered(identity)
        );
        handler
            .on_connection_state_change(RTCPeerConnectionState::Connected)
            .await;
        let reading = outbound.quality.lock().unwrap().poll(now, true).unwrap();
        assert_eq!(reading.samples, 0);
        assert!(!reading.should_spectate);
        handler
            .on_connection_state_change(RTCPeerConnectionState::Closed)
            .await;
        assert!(!state.lock().unwrap().player_session_matches(identity));
    }

    #[test]
    fn rtc_closed_is_published_only_after_client_and_outbound_cleanup() {
        for iteration in 0..20 {
            let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
                max_players: 1,
                ..ServerConfig::default()
            })));
            let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
            let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
            let outbound = Arc::new(RtcOutboundQueue::new(Arc::clone(&metrics)));
            let identity = register_connection_with_sender(
                &state,
                ClientPacketSender::bounded(outbound.clone()),
            );
            let session = Arc::new(RtcSession {
                state: Arc::clone(&state),
                peer: format!("cleanup-contract-{iteration}"),
                lifecycle: Mutex::new(RtcSessionLifecycle::Registered(identity)),
                lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(identity)).0,
                joined: Mutex::new(false),
                inbound_acks: AtomicUsize::new(0),
                registered_acks: AtomicUsize::new(0),
                channels: Mutex::new(RtcChannels::default()),
                outbound: Mutex::new(Some(Arc::clone(&outbound))),
                metrics,
                closed: Arc::new(Notify::new()),
                admission,
            });
            let lifecycle_observer = session.lifecycle_changed.subscribe();
            let lanes = writer::LaneWriter::new(Arc::clone(&session.metrics));
            outbound.attach_lane_writer(&lanes);
            outbound.enqueue(vec![1, 2]).unwrap();
            lanes.enqueue(2, Arc::new(vec![1, 3])).unwrap();

            let (captured_identity, retained_channels) =
                transition_rtc_session_closing(&session, false)
                    .expect("registered session should begin terminal cleanup");
            assert_eq!(captured_identity, Some(identity));
            assert_eq!(
                *session.lifecycle.lock().unwrap(),
                RtcSessionLifecycle::Closing,
            );
            assert_eq!(*lifecycle_observer.borrow(), RtcSessionLifecycle::Closing);
            assert!(
                outbound.state.lock().unwrap().closed,
                "Closing must not wake a lane worker while dispatcher retention remains open"
            );
            assert_eq!(session.metrics.snapshot().queue_packets, 0);
            assert_eq!(session.metrics.snapshot().queue_bytes, 0);
            assert!(
                state
                    .lock()
                    .unwrap()
                    .clients
                    .contains_key(&identity.player_id),
                "Closing remains observable while the authoritative client exists",
            );
            assert!(session.outbound.lock().unwrap().is_some());

            drop(retained_channels);
            disconnect_player_session(&state, identity);
            session
                .outbound
                .lock()
                .unwrap()
                .take()
                .expect("outbound queue should remain until cleanup")
                .close();
            finish_rtc_session_closed(&session);

            assert_eq!(
                *session.lifecycle.lock().unwrap(),
                RtcSessionLifecycle::Closed,
            );
            assert_eq!(*lifecycle_observer.borrow(), RtcSessionLifecycle::Closed);
            assert!(state.lock().unwrap().clients.is_empty());
            assert!(session.outbound.lock().unwrap().is_none());
            drop(session);
            assert_eq!(state.lock().unwrap().rtc_sessions, 0);
        }
    }

    #[test]
    fn stale_post_lookup_packets_and_cleanup_cannot_touch_reused_player_id() {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 2,
            ..ServerConfig::default()
        })));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let (old_sender, _old_receiver) = std::sync::mpsc::channel();
        let old_identity = register_connection_with_sender(&state, old_sender.into());
        {
            let mut guard = state.lock().unwrap();
            guard
                .clients
                .get_mut(&old_identity.player_id)
                .unwrap()
                .joined = true;
            guard.advance_client_life(old_identity.player_id).unwrap();
        }
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "stale-ingress-peer".to_string(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Registered(old_identity)),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(old_identity)).0,
            joined: Mutex::new(true),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(None),
            metrics,
            closed: Arc::new(Notify::new()),
            admission,
        });

        // Both handlers pause after copying the old lifecycle identity but
        // before entering connection-authenticated dispatch.
        let looked_up = Arc::new(Barrier::new(3));
        let resume = Arc::new(Barrier::new(3));
        let spawn_stale_packet = |label: &'static str, packet: Vec<u8>| {
            let session = Arc::clone(&session);
            let looked_up = Arc::clone(&looked_up);
            let resume = Arc::clone(&resume);
            thread::spawn(move || {
                handle_rtc_packet_after_lifecycle_lookup(&session, label, packet, || {
                    looked_up.wait();
                    resume.wait();
                });
            })
        };
        let position = spawn_stale_packet("zubspace-game", vec![crate::arena::CLIENT_POSITION]);
        let input = spawn_stale_packet("zubspace-game", current_input_record(42, 0x1234, 7));
        looked_up.wait();

        disconnect_rtc_session(&session);
        {
            let mut guard = state.lock().unwrap();
            assert!(!guard.clients.contains_key(&old_identity.player_id));
            guard.next_player_id = old_identity.player_id;
        }
        let (replacement_sender, _replacement_receiver) = std::sync::mpsc::channel();
        let replacement = register_connection_with_sender(&state, replacement_sender.into());
        assert_eq!(replacement.player_id, old_identity.player_id);
        assert_ne!(replacement.session_id, old_identity.session_id);
        {
            let mut guard = state.lock().unwrap();
            guard
                .clients
                .get_mut(&replacement.player_id)
                .unwrap()
                .joined = true;
            guard.advance_client_life(replacement.player_id).unwrap();
            guard.next_brick_id = 77;
        }

        resume.wait();
        position.join().unwrap();
        input.join().unwrap();

        {
            let mut guard = state.lock().unwrap();
            assert!(guard.player_session_matches(replacement));
            let replacement_client = guard.clients.get(&replacement.player_id).unwrap();
            assert_eq!(replacement_client.last_input_seq, None);
            assert_eq!(replacement_client.last_input_record, None);
            assert!(guard.drain_position_inputs().is_empty());
        }

        // A delayed transport cleanup callback carries the same old identity
        // and must not remove the replacement either.
        disconnect_player_session(&state, old_identity);
        let guard = state.lock().unwrap();
        assert!(guard.player_session_matches(replacement));
        assert_eq!(guard.next_brick_id, 77);
    }

    #[test]
    fn post_lookup_packets_cannot_cross_same_session_life_advance() {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let identity = register_connection_with_sender(&state, sender.into());
        {
            let mut guard = state.lock().unwrap();
            guard.clients.get_mut(&identity.player_id).unwrap().joined = true;
            guard.advance_client_life(identity.player_id).unwrap();
        }
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "same-session-life-race".to_string(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Registered(identity)),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(identity)).0,
            joined: Mutex::new(true),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(None),
            metrics,
            closed: Arc::new(Notify::new()),
            admission,
        });

        let mut weapon_position = vec![0; crate::continuum_position::CLIENT_POSITION_EXTRA_LEN];
        weapon_position[0] = crate::arena::CLIENT_POSITION;
        weapon_position[18..20].copy_from_slice(&1_000_u16.to_le_bytes());
        weapon_position[20..22].copy_from_slice(&66_u16.to_le_bytes());
        // Movement-only fixture: bit 5 is the server-authorized Rocket
        // activation pulse and would correctly reject an unowned activation
        // instead of exercising recursive cluster identity.
        let input_observation = current_input_record(42, 0x14, 7);

        let looked_up = Arc::new(Barrier::new(3));
        let resume = Arc::new(Barrier::new(3));
        let spawn_paused = |packet: Vec<u8>| {
            let session = Arc::clone(&session);
            let looked_up = Arc::clone(&looked_up);
            let resume = Arc::clone(&resume);
            thread::spawn(move || {
                handle_rtc_packet_after_lifecycle_lookup(
                    &session,
                    GAME_CHANNEL_LABEL,
                    packet,
                    || {
                        looked_up.wait();
                        resume.wait();
                    },
                );
            })
        };
        let stale_position = spawn_paused(weapon_position.clone());
        let stale_input = spawn_paused(input_observation.clone());
        looked_up.wait();

        let old_runtime = state
            .lock()
            .unwrap()
            .active_player_runtime_identity(identity.player_id)
            .unwrap();
        let new_life = state
            .lock()
            .unwrap()
            .advance_client_life(identity.player_id)
            .unwrap();
        assert_ne!(old_runtime.life, new_life);

        resume.wait();
        stale_position.join().unwrap();
        stale_input.join().unwrap();
        {
            let mut guard = state.lock().unwrap();
            assert!(guard.drain_position_inputs().is_empty());
            let client = &guard.clients[&identity.player_id];
            assert_eq!(client.last_input_seq, None);
            assert_eq!(client.last_input_record, None);
        }

        // A same-life cluster proves that the captured runtime identity is
        // carried through recursive packet dispatch and accepted normally
        // once the replacement life is active. Dead-life realtime input is
        // intentionally inert until the separately validated respawn
        // baseline arrives.
        state
            .lock()
            .unwrap()
            .clients
            .get_mut(&identity.player_id)
            .unwrap()
            .alive = true;
        let mut cluster = vec![
            crate::CORE_BYTE,
            crate::reliable::PACKET_CLUSTER,
            weapon_position.len() as u8,
        ];
        cluster.extend_from_slice(&weapon_position);
        cluster.push(input_observation.len() as u8);
        cluster.extend_from_slice(&input_observation);
        handle_rtc_packet(&session, GAME_CHANNEL_LABEL, cluster);

        let mut guard = state.lock().unwrap();
        let queued = guard.drain_position_inputs();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].0.life, new_life);
        assert_eq!(
            u16::from_le_bytes(queued[0].1[20..22].try_into().unwrap()),
            66
        );
        let client = &guard.clients[&identity.player_id];
        assert_eq!(client.last_input_seq, Some(42));
        assert_eq!(client.last_input_record.unwrap().bits, 0x14);
        assert_eq!(client.last_input_record.unwrap().direction, 7);
    }

    #[test]
    fn disconnect_reservation_blocks_reuse_until_owning_cleanup_finishes() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (old_sender, _old_receiver) = std::sync::mpsc::channel();
        let old_identity = register_connection_with_sender(&state, old_sender.into());
        let reserved = Arc::new(Barrier::new(2));
        let finish_cleanup = Arc::new(Barrier::new(2));
        let cleanup = {
            let state = Arc::clone(&state);
            let reserved = Arc::clone(&reserved);
            let finish_cleanup = Arc::clone(&finish_cleanup);
            thread::spawn(move || {
                crate::arena::disconnect_player_session_after_removal(&state, old_identity, || {
                    reserved.wait();
                    finish_cleanup.wait();
                });
            })
        };
        reserved.wait();

        {
            let mut guard = state.lock().unwrap();
            assert_eq!(
                guard.disconnecting_player_ids.get(&old_identity.player_id),
                Some(&old_identity.session_id)
            );
            let unrelated_stale_cleanup = PlayerSessionIdentity::new(
                old_identity.player_id,
                crate::authority_identity::PlayerSessionId::new(u64::MAX),
            );
            assert!(!guard.release_disconnecting_player_id(unrelated_stale_cleanup));
            assert_eq!(
                guard.disconnecting_player_ids.get(&old_identity.player_id),
                Some(&old_identity.session_id),
                "stale cleanup cannot release another session's reservation"
            );
            guard.next_player_id = old_identity.player_id;
        }

        let (during_sender, _during_receiver) = std::sync::mpsc::channel();
        let during_cleanup = register_connection_with_sender(&state, during_sender.into());
        assert_ne!(
            during_cleanup.player_id, old_identity.player_id,
            "reserved numeric id must be skipped while cleanup is paused"
        );

        finish_cleanup.wait();
        cleanup.join().unwrap();
        {
            let mut guard = state.lock().unwrap();
            assert!(
                !guard
                    .disconnecting_player_ids
                    .contains_key(&old_identity.player_id)
            );
            guard.next_player_id = old_identity.player_id;
        }
        let (after_sender, _after_receiver) = std::sync::mpsc::channel();
        let after_cleanup = register_connection_with_sender(&state, after_sender.into());
        assert_eq!(
            after_cleanup.player_id, old_identity.player_id,
            "numeric id becomes reusable only after owning cleanup releases it"
        );
        assert_ne!(after_cleanup.session_id, old_identity.session_id);
    }

    #[test]
    fn stalled_realtime_queue_is_bounded_and_keeps_the_freshest_frame() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let queue = RtcOutboundQueue::with_limits(Arc::clone(&metrics), 3, 64);
        queue.enqueue(test_snapshot(1)).unwrap();
        queue.enqueue(test_snapshot(2)).unwrap();
        queue.enqueue(test_snapshot(3)).unwrap();
        assert_eq!(queue.depth(), (1, test_snapshot(3).len()));
        let RtcOutboundReceive::Packets(packets) = queue.receive(8) else {
            panic!("fresh realtime frame should be available");
        };
        assert_eq!(packets, vec![test_snapshot(3)]);
        assert_eq!(queue.depth(), (0, 0));
        let metrics = metrics.snapshot();
        assert_eq!(metrics.realtime_coalesced, 2);
        assert_eq!(metrics.peak_queue_packets, 1);
        assert_eq!(metrics.queue_packets, 0);
        assert_eq!(metrics.queue_bytes, 0);
    }

    /// The dedupe guard covers a frame that is queued or still held by the writer, which is right for an ordered channel.
    /// It must not become permanent once the writer could not deliver: while the id stayed claimed, every retransmit timer
    /// answered "already in flight" and re-queued nothing, so a frame discarded on a closed or absent lane froze its
    /// application window forever - the stalled zone-file transfer, missing gameplay scope, and permanently invisible client
    /// this class produced.
    #[test]
    fn a_reliable_retransmit_is_blocked_only_while_the_writer_still_holds_it() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let queue = RtcOutboundQueue::with_limits(Arc::clone(&metrics), 8, 4096);
        let frame = {
            let mut frame = vec![0x00, 0x03, 0, 0, 0, 7];
            frame.extend_from_slice(b"map transfer frame");
            frame
        };
        queue
            .enqueue(frame.clone())
            .expect("first send should queue");
        let RtcOutboundReceive::Packets(packets) = queue.receive(8) else {
            panic!("queued reliable frame should be handed to the writer");
        };
        assert_eq!(packets, vec![frame.clone()]);
        // Writer still holds it: a retry must not duplicate the frame on an ordered channel.
        queue
            .enqueue(frame.clone())
            .expect("retry while writer-owned is a no-op");
        assert_eq!(queue.depth(), (0, 0));
        // The writer failed to deliver it, so the claim is released and the next timer may re-send.
        queue.release_unsent(&frame);
        queue
            .enqueue(frame.clone())
            .expect("a released frame must be re-sendable");
        assert_eq!(queue.depth(), (1, frame.len()));
    }

    #[test]
    fn stalled_reliable_queue_overflow_is_terminal_and_bounded() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let queue = RtcOutboundQueue::with_limits(Arc::clone(&metrics), 3, 64);
        for value in 1..=3 {
            queue.enqueue(vec![0x00, value]).unwrap();
        }
        assert_eq!(queue.depth(), (3, 6));
        assert!(queue.enqueue(vec![0x00, 4]).is_err());
        assert_eq!(queue.depth(), (0, 0));
        assert!(matches!(
            queue.receive(8),
            RtcOutboundReceive::ReliableOverflow
        ));
        let metrics = metrics.snapshot();
        assert_eq!(metrics.reliable_overflows, 1);
        assert_eq!(metrics.peak_queue_packets, 3);
        assert_eq!(metrics.queue_packets, 0);
        assert_eq!(metrics.queue_bytes, 0);
    }

    #[test]
    fn reliable_retransmit_bursts_do_not_duplicate_queued_or_in_flight_file_frames() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let queue = RtcOutboundQueue::with_limits(
            Arc::clone(&metrics),
            RTC_APPLICATION_RELIABLE_QUEUE_PACKETS,
            RTC_APPLICATION_QUEUE_BYTES,
        );
        // One exact maximum-size logical transfer uses at most 2,048
        // 512-byte small chunks. Retain every original frame as a slow SCTP
        // writer would, then fire several complete legacy resend bursts.
        let frames = (0..2_048_u32)
            .map(|id| crate::reliable::reliable(id, &[0x00; 512]))
            .collect::<Vec<_>>();
        for frame in &frames {
            queue.enqueue(frame.clone()).unwrap();
        }
        for _ in 0..4 {
            for frame in frames.iter().rev() {
                queue.enqueue(frame.clone()).unwrap();
            }
        }
        assert_eq!(queue.depth(), (2_048, 2_048 * 518));
        assert_eq!(metrics.snapshot().reliable_overflows, 0);

        let RtcOutboundReceive::Packets(in_flight) = queue.receive(256) else {
            panic!("a bounded writer batch should be available");
        };
        assert_eq!(in_flight.len(), 256);
        for frame in &frames {
            queue.enqueue(frame.clone()).unwrap();
        }
        assert_eq!(
            queue.depth().0,
            2_048 - 256,
            "both queued and writer-owned ids suppress byte-identical timer retries"
        );

        queue.complete_send(&in_flight[0]);
        queue.enqueue(in_flight[0].clone()).unwrap();
        queue.enqueue(in_flight[0].clone()).unwrap();
        assert_eq!(
            queue.depth().0,
            2_048 - 256 + 1,
            "a completed-but-unacknowledged id permits exactly one later retry"
        );
        assert_eq!(metrics.snapshot().reliable_overflows, 0);
    }

    #[test]
    fn independently_sequenced_realtime_events_are_not_coalesced() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let queue = RtcOutboundQueue::with_limits(Arc::clone(&metrics), 3, 256);
        let first = test_realtime_event(1);
        let second = test_realtime_event(2);
        queue.enqueue(first.clone()).unwrap();
        queue.enqueue(second.clone()).unwrap();
        assert_eq!(queue.depth(), (2, first.len() + second.len()));
        let RtcOutboundReceive::Packets(packets) = queue.receive(8) else {
            panic!("event snapshots should remain queued");
        };
        assert_eq!(packets, vec![first, second]);
        assert_eq!(metrics.snapshot().realtime_coalesced, 0);
    }

    #[test]
    fn weapon_events_are_not_coalesced_and_overtake_replaceable_state() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let queue = RtcOutboundQueue::with_limits(Arc::clone(&metrics), 3, 256);
        let weapon = test_realtime_event(1);
        let ordinary = test_snapshot(2);
        queue.enqueue(weapon.clone()).unwrap();
        queue.enqueue(ordinary.clone()).unwrap();
        assert_eq!(queue.depth().0, 2);
        let RtcOutboundReceive::Packets(packets) = queue.receive(8) else {
            panic!("weapon snapshot and following state frame should remain queued");
        };
        assert_eq!(packets, vec![weapon, ordinary]);
        assert_eq!(metrics.snapshot().realtime_coalesced, 0);
    }

    #[test]
    fn repeated_private_rocket_state_remains_realtime_coalescible() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let queue = RtcOutboundQueue::with_limits(Arc::clone(&metrics), 3, 128);
        queue.enqueue(test_rocket_snapshot(1, 3)).unwrap();
        queue.enqueue(test_rocket_snapshot(2, 2)).unwrap();
        assert_eq!(queue.depth().0, 1);
        let RtcOutboundReceive::Packets(packets) = queue.receive(8) else {
            panic!("fresh repeated Rocket state should be available");
        };
        let parsed = crate::snapshot_wire::SnapshotFrame::parse(&packets[0]).unwrap();
        assert_eq!(parsed.header.snapshot_id, 2);
        assert!(matches!(
            parsed.events.as_slice(),
            [crate::snapshot_wire::SnapshotEvent::RocketState(state)] if state.rockets == 2
        ));
        assert_eq!(metrics.snapshot().realtime_coalesced, 1);
    }

    #[test]
    fn departure_cutoff_overflow_closes_the_stalled_observer_without_orphaned_work() {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 2,
            ..ServerConfig::default()
        })));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let owner = crate::arena::register_connection(&state, tx.clone());
        let peer = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, owner, 0);
        crate::arena::join_arena(&state, peer, 0);
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let queue = Arc::new(RtcOutboundQueue::with_limits(Arc::clone(&metrics), 1, 256));
        let identity = {
            let mut guard = state.lock().unwrap();
            guard.next_projectile_group_id = 42;
            guard.clients.get_mut(&peer).unwrap().tx = ClientPacketSender::bounded(queue.clone());
            guard.active_player_session_identity(peer).unwrap()
        };
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "departure-stalled-observer".to_string(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Registered(identity)),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(identity)).0,
            joined: Mutex::new(true),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(Some(Arc::clone(&queue))),
            metrics,
            closed: Arc::new(Notify::new()),
            admission,
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        queue.attach_session(Arc::downgrade(&session), runtime.handle().clone());
        queue.enqueue(vec![0x00, 1]).unwrap();
        crate::arena::leave_arena(&state, owner);
        assert!(matches!(
            queue.receive(8),
            RtcOutboundReceive::ReliableOverflow
        ));
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(1), async {
                while *session.lifecycle.lock().unwrap() != RtcSessionLifecycle::Closed {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("stalled observer must close instead of silently missing its departure cutoff");
        });
        let guard = state.lock().unwrap();
        assert!(!guard.clients.contains_key(&peer));
        assert!(!guard.clients[&owner].joined);
        assert!(guard.pending_owner_weapon_cancellations.is_empty());
        assert_eq!(guard.rtc_overload_metrics.snapshot().reliable_overflows, 1);
    }

    #[test]
    fn rtc_closing_lane_recovers_registered_session_without_capacity_failure() {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let outbound = Arc::new(RtcOutboundQueue::new(Arc::clone(&metrics)));
        let identity =
            register_connection_with_sender(&state, ClientPacketSender::bounded(outbound.clone()));
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "closed-lane-recovery".to_string(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Registered(identity)),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(identity)).0,
            joined: Mutex::new(false),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(Some(Arc::clone(&outbound))),
            metrics: Arc::clone(&metrics),
            closed: Arc::new(Notify::new()),
            admission,
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        outbound.attach_session(Arc::downgrade(&session), runtime.handle().clone());
        let writer = writer::LaneWriter::new(Arc::clone(&metrics));
        outbound.attach_lane_writer(&writer);
        outbound.enqueue(vec![1, 2]).unwrap();
        writer.close();
        assert!(!enqueue_rtc_lane(
            &outbound,
            &writer,
            2,
            Arc::new(vec![1, 3])
        ));
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), async {
                while *session.lifecycle.lock().unwrap() != RtcSessionLifecycle::Closed
                    || Arc::strong_count(&session) != 1
                {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("unexpectedly closed writer must terminate its registered session");
        });
        assert!(state.lock().unwrap().clients.is_empty());
        assert!(session.outbound_queue().is_none());
        assert_eq!(metrics.snapshot().reliable_overflows, 0);
        assert_eq!(metrics.snapshot().queue_packets, 0);
        assert_eq!(metrics.snapshot().queue_bytes, 0);
        drop(session);
        assert_eq!(state.lock().unwrap().rtc_sessions, 0);
    }

    #[test]
    fn reliable_queue_overflow_disconnects_and_cleans_registered_session() {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let queue = Arc::new(RtcOutboundQueue::with_limits(Arc::clone(&metrics), 1, 64));
        let player_id =
            register_connection_with_sender(&state, ClientPacketSender::bounded(queue.clone()));
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "stalled-peer".to_string(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Registered(player_id)),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Registered(player_id)).0,
            joined: Mutex::new(false),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(RtcChannels::default()),
            outbound: Mutex::new(Some(Arc::clone(&queue))),
            metrics,
            closed: Arc::new(Notify::new()),
            admission,
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        queue.attach_session(Arc::downgrade(&session), runtime.handle().clone());
        queue.enqueue(vec![0x00, 1]).unwrap();
        assert!(queue.enqueue(vec![0x00, 2]).is_err());
        assert!(matches!(
            queue.receive(8),
            RtcOutboundReceive::ReliableOverflow
        ));
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if *session.lifecycle.lock().unwrap() == RtcSessionLifecycle::Closed {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("overflow cleanup should disconnect promptly");
        });

        assert_eq!(
            *session.lifecycle.lock().unwrap(),
            RtcSessionLifecycle::Closed
        );
        assert!(session.outbound.lock().unwrap().is_none());
        assert!(state.lock().unwrap().clients.is_empty());
        drop(session);
        assert_eq!(state.lock().unwrap().rtc_sessions, 0);
    }

    #[test]
    fn rtc_send_buffer_limits_are_finite() {
        assert!(RTC_DATA_CHANNEL_SEND_BUFFER_BYTES > 0);
        assert!(RTC_APPLICATION_RELIABLE_QUEUE_PACKETS > 0);
        assert!(RTC_APPLICATION_QUEUE_BYTES > RTC_DATA_CHANNEL_SEND_BUFFER_BYTES);
    }

    #[test]
    fn creates_answer_json_for_data_channel_offer() {
        let offer_json = browser_style_offer_json();
        let state = Arc::new(Mutex::new(ServerState::default()));
        let answer_json = create_answer_for_offer_json(state, "test".to_string(), &offer_json)
            .expect("server should create RTC answer");
        let answer: RTCSessionDescription =
            serde_json::from_str(&answer_json).expect("answer should be valid JSON SDP");
        assert_eq!(answer.sdp_type, RTCSdpType::Answer);
        assert!(answer.sdp.contains("m=application"));
        assert!(answer.sdp.contains("UDP/DTLS/SCTP"));
        assert!(
            answer.sdp.contains(" 0.0.0.0 "),
            "the wildcard-bound local mode must not be mistaken for public-IP remapping: {}",
            answer.sdp
        );
    }

    #[test]
    fn explicit_local_rtc_udp_bind_gathers_concrete_host_candidate() {
        let offer_json = browser_style_offer_json();
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            rtc_udp_bind_ip: Some("127.0.0.1".parse().unwrap()),
            ..ServerConfig::default()
        })));
        let answer_json = create_answer_for_offer_json(state, "test".to_string(), &offer_json)
            .expect("server should gather an answer on the configured local interface");
        let answer: RTCSessionDescription =
            serde_json::from_str(&answer_json).expect("answer should be valid JSON SDP");
        assert!(
            answer.sdp.contains(" 127.0.0.1 "),
            "answer must advertise the explicitly bound local interface: {}",
            answer.sdp
        );
        assert!(
            !answer.sdp.contains(" 0.0.0.0 "),
            "explicit local binding must not gather an unspecified host candidate: {}",
            answer.sdp
        );
    }

    #[test]
    fn rtc_udp_bind_ip_preserves_ephemeral_and_fixed_capacity_addresses() {
        let ephemeral = ServerConfig {
            rtc_udp_bind_ip: Some("127.0.0.1".parse().unwrap()),
            ..ServerConfig::default()
        };
        assert_eq!(
            rtc_udp_bind_addresses(&ephemeral),
            vec!["127.0.0.1:0".to_string()]
        );

        let fixed = ServerConfig {
            max_players: 2,
            rtc_udp_bind_ip: Some("127.0.0.1".parse().unwrap()),
            rtc_udp_port_range: Some(crate::config::RtcUdpPortRange {
                min: 19_000,
                max: 19_001,
            }),
            ..ServerConfig::default()
        };
        let addresses = rtc_udp_bind_addresses(&fixed);
        assert_eq!(addresses.len(), 2);
        assert!(addresses.contains(&"127.0.0.1:19000".to_string()));
        assert!(addresses.contains(&"127.0.0.1:19001".to_string()));
    }

    #[test]
    fn rtc_admission_reserves_half_open_sessions_and_releases_capacity() {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).expect("first offer should fit");
        assert_eq!(state.lock().unwrap().rtc_sessions, 1);
        let error = RtcAdmission::reserve(Arc::clone(&state))
            .err()
            .expect("second half-open offer must be rejected");
        assert!(error.to_string().contains("capacity reached"));
        admission.release();
        assert_eq!(state.lock().unwrap().rtc_sessions, 0);
        let replacement = RtcAdmission::reserve(Arc::clone(&state))
            .expect("released capacity should be reusable");
        drop(replacement);
        assert_eq!(state.lock().unwrap().rtc_sessions, 0);
    }

    #[test]
    fn signaling_error_taxonomy_has_stable_status_codes_and_safe_messages() {
        let cases = [
            (
                RtcSignalingError::invalid_offer("parser secret: malformed field"),
                RtcSignalingErrorKind::InvalidOffer,
                "400 Bad Request",
                "invalid_webrtc_offer",
                false,
            ),
            (
                RtcSignalingError::capacity_unavailable("configured slots exhausted"),
                RtcSignalingErrorKind::CapacityUnavailable,
                "503 Service Unavailable",
                "rtc_capacity_unavailable",
                true,
            ),
            (
                RtcSignalingError::service_unavailable("socket 192.0.2.4:1234 failed"),
                RtcSignalingErrorKind::ServiceUnavailable,
                "503 Service Unavailable",
                "rtc_service_unavailable",
                true,
            ),
            (
                RtcSignalingError::internal("runtime secret: builder exploded"),
                RtcSignalingErrorKind::Internal,
                "500 Internal Server Error",
                "internal_server_error",
                false,
            ),
        ];
        for (error, kind, status, code, retryable) in cases {
            assert_eq!(error.kind(), kind);
            assert_eq!(error.status(), status);
            assert_eq!(error.code(), code);
            assert_eq!(error.retry_after_seconds().is_some(), retryable);
            assert!(!error.public_message().contains("secret"));
            assert!(error.to_string().contains("secret") || !error.to_string().is_empty());
        }
    }

    #[test]
    fn ice_gathering_timeout_and_outage_are_retryable_service_failures() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let (held_tx, mut timeout_rx) = tokio::sync::mpsc::channel(1);
            let timeout = wait_for_ice_gathering(&mut timeout_rx, Duration::from_millis(5))
                .await
                .expect_err("held ICE sender must reach the deadline");
            assert_eq!(timeout.kind(), RtcSignalingErrorKind::ServiceUnavailable);
            assert_eq!(timeout.status(), "503 Service Unavailable");
            assert_eq!(timeout.retry_after_seconds(), Some("1"));
            assert!(timeout.to_string().contains("timed out gathering"));
            drop(held_tx);

            let (dropped_tx, mut outage_rx) = tokio::sync::mpsc::channel(1);
            drop(dropped_tx);
            let outage = wait_for_ice_gathering(&mut outage_rx, Duration::from_secs(1))
                .await
                .expect_err("dropped ICE service must be reported immediately");
            assert_eq!(outage.kind(), RtcSignalingErrorKind::ServiceUnavailable);
            assert_eq!(outage.status(), "503 Service Unavailable");
            assert_eq!(outage.retry_after_seconds(), Some("1"));
            assert!(outage.to_string().contains("service stopped"));
        });
    }

    #[test]
    fn blocked_registration_leaves_runtime_worker_responsive() {
        for expire in [false, true] {
            let state = Arc::new(Mutex::new(ServerState::default()));
            let admission = RtcAdmission::reserve(Arc::clone(&state)).unwrap();
            let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
            let mut channels = RtcChannels::default();
            channels.protocol = valid_open_channel_protocol();
            let session = Arc::new(RtcSession {
                state: Arc::clone(&state),
                peer: "blocked-registration".into(),
                lifecycle: Mutex::new(RtcSessionLifecycle::Handshaking),
                lifecycle_changed: watch::channel(RtcSessionLifecycle::Handshaking).0,
                joined: Mutex::new(false),
                inbound_acks: AtomicUsize::new(0),
                registered_acks: AtomicUsize::new(0),
                channels: Mutex::new(channels),
                outbound: Mutex::new(None),
                metrics,
                closed: Arc::new(Notify::new()),
                admission,
            });
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            let held = state.lock().unwrap();
            let registration_session = Arc::clone(&session);
            let (entered_tx, entered_rx) = std::sync::mpsc::channel();
            let registration = runtime.spawn(async move {
                let executor = RtcIngressExecutor::new(1);
                entered_tx.send(()).unwrap();
                dispatch_rtc_registration(&executor, registration_session).await
            });
            let registration_started = entered_rx.recv_timeout(Duration::from_secs(2)).is_ok();
            let (heartbeat_tx, heartbeat_rx) = std::sync::mpsc::channel();
            let heartbeat = runtime.spawn(async move {
                heartbeat_tx.send(()).unwrap();
            });
            let responsive = heartbeat_rx
                .recv_timeout(Duration::from_millis(500))
                .is_ok();
            let (expiry_tx, expiry_rx) = std::sync::mpsc::channel();
            let expiry_session = Arc::clone(&session);
            let expiry = runtime.spawn(async move {
                let retired = expire && expire_handshaking_rtc_session(&expiry_session);
                expiry_tx.send(retired).unwrap();
            });
            let expired_before_release = expiry_rx.recv_timeout(Duration::from_millis(500)).ok();
            // Release before asserting even on a red run: never strand the runtime
            // or its outbound writer behind the deliberately held server lock.
            drop(held);
            let registered = runtime.block_on(async {
                let registered = registration.await.unwrap();
                heartbeat.await.unwrap();
                expiry.await.unwrap();
                disconnect_rtc_session(&session);
                registered
            });
            assert!(registration_started, "registration task did not start");
            assert_eq!(
                expired_before_release,
                Some(expire),
                "expiry must not wait for server state"
            );
            assert_eq!(
                registered, !expire,
                "only an unexpired registration may allocate a client"
            );
            assert!(state.lock().unwrap().clients.is_empty());
            assert!(
                responsive,
                "registration's server-state wait occupied the networking worker"
            );
        }
    }

    #[test]
    fn late_reliable_open_cannot_revive_an_expired_rtc_session() {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let mut channels = RtcChannels::default();
        channels.protocol = valid_open_channel_protocol();
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "expired-race".to_string(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Handshaking),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Handshaking).0,
            joined: Mutex::new(false),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(channels),
            outbound: Mutex::new(None),
            metrics,
            closed: Arc::new(Notify::new()),
            admission: RtcAdmission::reserve(Arc::clone(&state))
                .expect("test handshake should reserve its only slot"),
        });

        // Deterministically model the harmful ordering: expiry wins the
        // lifecycle serialization, then a queued reliable-channel OnOpen is
        // delivered. Closed is terminal, so the late callback cannot register.
        let late_open_gate = Arc::new(std::sync::Barrier::new(2));
        let late_open_session = Arc::clone(&session);
        let late_open_thread_gate = Arc::clone(&late_open_gate);
        let late_open = thread::spawn(move || {
            late_open_thread_gate.wait();
            register_rtc_session_if_needed(&late_open_session)
        });
        assert!(expire_handshaking_rtc_session(&session));
        late_open_gate.wait();
        assert!(
            !late_open
                .join()
                .expect("late OnOpen callback should finish")
        );
        disconnect_rtc_session(&session);
        assert_eq!(
            *session.lifecycle.lock().unwrap(),
            RtcSessionLifecycle::Closed
        );
        assert!(state.lock().unwrap().clients.is_empty());

        // The peer owner dropping after its notified cleanup releases the
        // admission reservation, and the slot is immediately reusable.
        drop(session);
        assert_eq!(state.lock().unwrap().rtc_sessions, 0);
        let replacement = RtcAdmission::reserve(Arc::clone(&state))
            .expect("expired session must not leak its admission slot");
        drop(replacement);
        assert_eq!(state.lock().unwrap().rtc_sessions, 0);
    }

    #[test]
    fn pre_registration_packet_waiter_observes_registration_and_terminal_close() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("registration waiter test runtime should build");
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));
        let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
        let mut channels = RtcChannels::default();
        channels.protocol = valid_open_channel_protocol();
        let session = Arc::new(RtcSession {
            state: Arc::clone(&state),
            peer: "pre-registration-packet".to_string(),
            lifecycle: Mutex::new(RtcSessionLifecycle::Handshaking),
            lifecycle_changed: watch::channel(RtcSessionLifecycle::Handshaking).0,
            joined: Mutex::new(false),
            inbound_acks: AtomicUsize::new(0),
            registered_acks: AtomicUsize::new(0),
            channels: Mutex::new(channels),
            outbound: Mutex::new(None),
            metrics,
            closed: Arc::new(Notify::new()),
            admission: RtcAdmission::reserve(Arc::clone(&state))
                .expect("test handshake should reserve its only slot"),
        });

        runtime.block_on(async {
            let registration_session = Arc::clone(&session);
            let registration_waiter =
                tokio::spawn(async move { wait_for_rtc_registration(&registration_session).await });
            tokio::task::yield_now().await;
            assert!(register_rtc_session_if_needed(&session));
            assert!(
                tokio::time::timeout(Duration::from_secs(1), registration_waiter)
                    .await
                    .expect("registration waiter should wake")
                    .expect("registration waiter task should finish")
            );

            disconnect_rtc_session(&session);
            assert!(!wait_for_rtc_registration(&session).await);
        });
        let session_reclamation = Arc::downgrade(&session);
        drop(session);
        wait_for_real_peer_condition(
            "registration waiter session reclamation",
            Duration::from_secs(1),
            || {
                let state = state.lock().unwrap();
                // Do not upgrade while holding state: the writer can release
                // its last reference concurrently, making this probe's Arc
                // run RtcAdmission::drop and recursively lock the same state.
                // A zero strong count plus the settled admission counter also
                // proves destruction finished, without acquiring ownership.
                session_reclamation.strong_count() == 0
                    && state.clients.is_empty()
                    && state.rtc_sessions == 0
            },
        );
    }

    #[test]
    fn registered_session_is_disconnected_only_when_its_login_was_never_accepted() {
        assert!(
            RTC_LOGIN_DEADLINE > Duration::from_secs(45),
            "the deadline must outlast the browser's in-use identity retries"
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("login deadline test runtime should build");
        for logged_in in [false, true] {
            let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
                max_players: 1,
                ..ServerConfig::default()
            })));
            let metrics = state.lock().unwrap().rtc_overload_metrics.clone();
            let mut channels = RtcChannels::default();
            channels.protocol = valid_open_channel_protocol();
            let session = Arc::new(RtcSession {
                state: Arc::clone(&state),
                peer: "login-deadline".to_string(),
                lifecycle: Mutex::new(RtcSessionLifecycle::Handshaking),
                lifecycle_changed: watch::channel(RtcSessionLifecycle::Handshaking).0,
                joined: Mutex::new(false),
                inbound_acks: AtomicUsize::new(0),
                registered_acks: AtomicUsize::new(0),
                channels: Mutex::new(channels),
                outbound: Mutex::new(None),
                metrics,
                closed: Arc::new(Notify::new()),
                admission: RtcAdmission::reserve(Arc::clone(&state))
                    .expect("test handshake should reserve its only slot"),
            });

            runtime.block_on(async {
                assert!(register_rtc_session_if_needed(&session));
                let RtcSessionLifecycle::Registered(identity) = *session.lifecycle.lock().unwrap()
                else {
                    panic!("registration should record the session identity");
                };
                if logged_in {
                    state
                        .lock()
                        .unwrap()
                        .clients
                        .get_mut(&identity.player_id)
                        .unwrap()
                        .player_identity = Some("000000000001".to_string());
                }

                assert_eq!(
                    expire_rtc_session_without_login(&session, identity),
                    !logged_in
                );
                assert_eq!(
                    state
                        .lock()
                        .unwrap()
                        .clients
                        .contains_key(&identity.player_id),
                    logged_in
                );
                assert_eq!(
                    *session.lifecycle.lock().unwrap() == RtcSessionLifecycle::Closed,
                    !logged_in
                );
                disconnect_rtc_session(&session);
                assert!(
                    !expire_rtc_session_without_login(&session, identity),
                    "a closed session is never disconnected twice"
                );
            });
        }
    }

    #[test]
    fn retained_port_is_quarantined_without_blocking_unrelated_port_capacity() {
        let (retained_socket, unrelated_socket) = adjacent_udp_sockets();
        let retained_port = retained_socket.local_addr().unwrap().port();
        let unrelated_port = unrelated_socket.local_addr().unwrap().port();
        drop(unrelated_socket);

        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 2,
            rtc_udp_port_range: Some(crate::config::RtcUdpPortRange {
                min: retained_port,
                max: unrelated_port,
            }),
            ..ServerConfig::default()
        })));
        let admission = RtcAdmission::reserve(Arc::clone(&state)).expect("first admission");
        state
            .lock()
            .unwrap()
            .rtc_udp_ports_in_use
            .insert(retained_port);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let retained_lifecycle = rtc_udp_port_lifecycle(retained_port);
            let unrelated_lifecycle = rtc_udp_port_lifecycle(unrelated_port);
            let _retained_guard = retained_lifecycle.lock().await;
            let started = std::time::Instant::now();
            let error = wait_for_rtc_udp_socket_reuse(
                &format!("0.0.0.0:{retained_port}"),
                Duration::from_millis(20),
            )
            .await
            .expect_err("externally retained port must miss the bounded deadline");
            assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
            assert!(started.elapsed() < Duration::from_millis(200));

            admission.quarantine_fixed_port(retained_port);
            {
                let state = state.lock().unwrap();
                assert_eq!(state.rtc_sessions, 0);
                assert_eq!(state.rtc_effective_capacity(), 1);
                assert_eq!(
                    state
                        .rtc_udp_quarantined_ports
                        .iter()
                        .copied()
                        .collect::<Vec<_>>(),
                    vec![retained_port]
                );
            }

            let _unrelated_guard =
                tokio::time::timeout(Duration::from_millis(50), unrelated_lifecycle.lock())
                    .await
                    .expect("retained port lifecycle must not block an unrelated port");
            wait_for_rtc_udp_socket_reuse(
                &format!("0.0.0.0:{unrelated_port}"),
                Duration::from_millis(20),
            )
            .await
            .expect("unrelated configured port should remain reusable");

            let replacement = RtcAdmission::reserve(Arc::clone(&state))
                .expect("unrelated port capacity should remain admissible");
            let error = RtcAdmission::reserve(Arc::clone(&state))
                .err()
                .expect("quarantined port must not be advertised as capacity");
            assert!(
                error.to_string().contains("degraded UDP ports 1"),
                "{error}"
            );
            drop(replacement);
        });
        drop(retained_socket);
    }

    #[test]
    fn quarantined_udp_port_recovers_after_multiple_bounded_cycles_without_duplicates() {
        let conflicted_socket = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let conflicted_port = conflicted_socket.local_addr().unwrap().port();
        let healthy_port = if conflicted_port == u16::MAX {
            conflicted_port - 1
        } else {
            conflicted_port + 1
        };
        let range_min = conflicted_port.min(healthy_port);
        let range_max = conflicted_port.max(healthy_port);
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 2,
            rtc_udp_port_range: Some(crate::config::RtcUdpPortRange {
                min: range_min,
                max: range_max,
            }),
            ..ServerConfig::default()
        })));
        {
            let mut state = state.lock().unwrap();
            state.rtc_udp_quarantined_ports.insert(conflicted_port);
            state.rtc_udp_ports_in_use.insert(healthy_port);
            assert_eq!(state.rtc_effective_capacity(), 1);
        }
        let retained = Arc::new(Mutex::new(Some(conflicted_socket)));
        let attempts = Arc::new(AtomicUsize::new(0));
        let first_cycle_complete = Arc::new(Notify::new());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let retained = Arc::clone(&retained);
            let attempts_for_probe = Arc::clone(&attempts);
            let first_cycle_complete_for_probe = Arc::clone(&first_cycle_complete);
            let recovery = spawn_rtc_udp_port_recovery_with_probe(
                Arc::clone(&state),
                FixedRtcUdpPort {
                    port: conflicted_port,
                    lifecycle: rtc_udp_port_lifecycle(conflicted_port),
                },
                format!("0.0.0.0:{conflicted_port}"),
                RtcUdpRecoveryPolicy {
                    probe_interval: Duration::from_millis(1),
                    attempts_per_cycle: 2,
                    initial_backoff: Duration::from_secs(1),
                    max_backoff: Duration::from_secs(1),
                },
                move |address| {
                    let attempt = attempts_for_probe.fetch_add(1, Ordering::SeqCst) + 1;
                    if attempt == 2 {
                        first_cycle_complete_for_probe.notify_one();
                    }
                    if attempt == 4 {
                        drop(retained.lock().unwrap().take());
                    }
                    std::net::UdpSocket::bind(address).map(drop)
                },
            )
            .expect("first recovery worker should own the quarantined port");
            assert_eq!(
                state
                    .lock()
                    .unwrap()
                    .rtc_udp_recovery_ports
                    .keys()
                    .copied()
                    .collect::<Vec<_>>(),
                vec![conflicted_port]
            );
            first_cycle_complete.notified().await;
            {
                let state = state.lock().unwrap();
                assert!(state.rtc_udp_quarantined_ports.contains(&conflicted_port));
                assert!(state.rtc_udp_recovery_ports.contains_key(&conflicted_port));
                assert_eq!(state.rtc_effective_capacity(), 1);
            }
            let duplicate = spawn_rtc_udp_port_recovery_with_probe(
                Arc::clone(&state),
                FixedRtcUdpPort {
                    port: conflicted_port,
                    lifecycle: rtc_udp_port_lifecycle(conflicted_port),
                },
                format!("0.0.0.0:{conflicted_port}"),
                RtcUdpRecoveryPolicy {
                    probe_interval: Duration::from_millis(1),
                    attempts_per_cycle: 1,
                    initial_backoff: Duration::from_millis(1),
                    max_backoff: Duration::from_millis(1),
                },
                |_| panic!("deduplicated recovery probe must never run"),
            );
            assert!(duplicate.is_none());
            assert_eq!(
                tokio::time::timeout(Duration::from_millis(250), recovery)
                    .await
                    .expect("admission activity should wake recovery backoff")
                    .expect("recovery task should finish"),
                RtcUdpRecoveryOutcome::Recovered
            );
        });
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        let state = state.lock().unwrap();
        assert!(state.rtc_udp_quarantined_ports.is_empty());
        assert!(state.rtc_udp_recovery_ports.is_empty());
        assert_eq!(state.rtc_effective_capacity(), 2);
        assert_eq!(
            state
                .rtc_udp_ports_in_use
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![healthy_port],
            "recovery of one port must not disturb healthy port ownership"
        );
    }

    #[test]
    fn recovery_owner_cleans_up_on_runtime_shutdown_and_can_be_restarted() {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            rtc_udp_port_range: Some(crate::config::RtcUdpPortRange {
                min: port,
                max: port,
            }),
            ..ServerConfig::default()
        })));
        state.lock().unwrap().rtc_udp_quarantined_ports.insert(port);
        let fixed = FixedRtcUdpPort {
            port,
            lifecycle: rtc_udp_port_lifecycle(port),
        };

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("first recovery runtime");
        runtime.block_on(async {
            spawn_rtc_udp_port_recovery_with_probe(
                Arc::clone(&state),
                fixed.clone(),
                format!("0.0.0.0:{port}"),
                RtcUdpRecoveryPolicy {
                    probe_interval: Duration::from_secs(60),
                    attempts_per_cycle: 1,
                    initial_backoff: Duration::from_secs(60),
                    max_backoff: Duration::from_secs(60),
                },
                |_| Err(std::io::Error::from(std::io::ErrorKind::AddrInUse)),
            )
            .expect("first recovery owner");
            tokio::task::yield_now().await;
        });
        assert!(
            state
                .lock()
                .unwrap()
                .rtc_udp_recovery_ports
                .contains_key(&port)
        );
        drop(runtime);
        assert!(state.lock().unwrap().rtc_udp_recovery_ports.is_empty());
        assert!(
            state
                .lock()
                .unwrap()
                .rtc_udp_quarantined_ports
                .contains(&port),
            "worker shutdown must leave degraded capacity observable"
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("replacement recovery runtime");
        runtime.block_on(async {
            let replacement = spawn_rtc_udp_port_recovery_with_probe(
                Arc::clone(&state),
                fixed,
                format!("0.0.0.0:{port}"),
                RtcUdpRecoveryPolicy {
                    probe_interval: Duration::from_millis(1),
                    attempts_per_cycle: 1,
                    initial_backoff: Duration::from_millis(1),
                    max_backoff: Duration::from_millis(1),
                },
                |address| std::net::UdpSocket::bind(address).map(drop),
            )
            .expect("cancelled recovery must be restartable");
            assert_eq!(
                replacement.await.expect("replacement task should finish"),
                RtcUdpRecoveryOutcome::Recovered
            );
        });
        let state = state.lock().unwrap();
        assert!(state.rtc_udp_quarantined_ports.is_empty());
        assert!(state.rtc_udp_recovery_ports.is_empty());
        assert_eq!(state.rtc_effective_capacity(), 1);
    }

    #[test]
    fn rtc_configuration_carries_server_stun_and_turn_shapes() {
        let config = ServerConfig {
            rtc_ice_urls: vec![
                "stun:stun.example.test:3478".to_string(),
                "turn:turn.example.test:3478?transport=udp".to_string(),
            ],
            rtc_ice_username: "pilot".to_string(),
            rtc_ice_credential: "secret".to_string(),
            rtc_ice_transport_policy: RtcIceTransportPolicy::Relay,
            ..ServerConfig::default()
        };
        let rtc = rtc_configuration(&config);
        assert_eq!(rtc.ice_servers().len(), 1);
        assert_eq!(rtc.ice_servers()[0].urls, config.rtc_ice_urls);
        assert_eq!(rtc.ice_transport_policy(), WebRtcIceTransportPolicy::Relay);
    }

    #[test]
    fn rtc_writer_prioritizes_realtime_before_reliable_packets() {
        // Snapshot frames (`0x70`) and dedicated event facts (`0x7d`) are the
        // realtime carriers. This fixture pins the state-frame ordering;
        // dedicated-event-first ordering is covered separately below.
        let snapshot = vec![0x70, 1, 2, 3];
        let mut packets = vec![vec![0x00, 0x03], snapshot.clone(), vec![0x07]];
        assert_eq!(
            prioritized_rtc_packets(&mut packets),
            vec![snapshot, vec![0x00, 0x03], vec![0x07]],
        );
        assert!(packets.is_empty());
    }

    #[test]
    fn snapshot_packet_routes_to_game_channel() {
        // The post-Phase-4 routing rule: `0x70` is realtime.
        let snapshot = vec![0x70, 0, 0];
        assert!(realtime_server_packet(&snapshot));
        // Legacy `0x05` / `0x3a` shapes are gone — they should NOT
        // be classified as realtime any more (no fan-out path
        // emits them).
        assert!(!realtime_server_packet(&[0x05, 0]));
        assert!(!realtime_server_packet(&[0x3a, 0]));
        // Anything else (chat, roster, ack, reliable wrapper) stays
        // off the game channel.
        assert!(!realtime_server_packet(&[0x00, 0x03]));
        assert!(!realtime_server_packet(&[0x07]));
    }

    #[test]
    fn quality_probes_are_unreliable_distinct_and_cannot_trigger_reliable_overflow() {
        use crate::connection_quality_protocol::ConnectionQualityProbe;
        let probe = ConnectionQualityProbe {
            server_session_id: 1,
            arena_stream_id: 2,
            sequence: 3,
            nonce: 4,
        }
        .encode();
        assert_eq!(rtc_outbound_route(&probe), RtcOutboundRoute::Game);
        assert!(!replaceable_realtime_server_packet(&probe));
        assert!(!weapon_event_server_packet(&probe));
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let queue = RtcOutboundQueue::with_limits(metrics.clone(), 1, 64);
        queue.enqueue(vec![7; 40]).unwrap();
        assert!(queue.enqueue(probe).is_err());
        assert_eq!(metrics.snapshot().reliable_overflows, 0);
        assert!(!queue.state.lock().unwrap().closed);
        assert!(
            matches!(queue.receive(1), RtcOutboundReceive::Packets(packets) if packets == vec![vec![7;40]])
        );
    }

    #[test]
    fn keyboard_weapon_event_duplicates_to_game_and_unordered_reliable_event_channels() {
        let weapon = test_realtime_event(7);
        assert!(weapon_event_server_packet(&weapon));
        assert!(!weapon_event_server_packet(&test_snapshot(8)));
        assert_eq!(rtc_outbound_route(&weapon), RtcOutboundRoute::GameAndEvent);
        let batch = crate::realtime_event::batch_realtime_events(vec![
            test_realtime_event(9),
            test_realtime_event(10),
        ])
        .remove(0);
        assert_eq!(batch[0], crate::realtime_event::REALTIME_EVENT_BATCH_PACKET);
        assert!(realtime_server_packet(&batch));
        assert!(!replaceable_realtime_server_packet(&batch));
        assert_eq!(rtc_outbound_route(&batch), RtcOutboundRoute::GameAndEvent);
        assert_eq!(
            rtc_outbound_route(&test_snapshot(8)),
            RtcOutboundRoute::Game
        );
        assert_eq!(rtc_outbound_route(&[0x07]), RtcOutboundRoute::Reliable);
    }

    #[test]
    fn the_game_channel_copy_follows_the_configured_policy_and_probe_loss() {
        let reading = |samples: usize, round_trip_loss: f64| {
            Some(crate::connection_quality::QualityReading {
                samples,
                average_rtt_ms: Some(40.0),
                round_trip_loss,
                should_spectate: false,
                sustained_healthy: false,
                may_play: true,
                cooldown_remaining: Duration::ZERO,
            })
        };
        let on_loss = RealtimeEventGameCopy::OnLoss;
        // No reading yet, or under 10 s of probes: keep the copy.
        assert!(on_loss.wanted(None));
        assert!(on_loss.wanted(reading(19, 0.0)));
        // A full window with every probe answered: one copy is enough.
        assert!(!on_loss.wanted(reading(20, 0.0)));
        assert!(!on_loss.wanted(reading(60, 0.0)));
        // One lost probe in the window brings the copy back.
        assert!(on_loss.wanted(reading(60, 1.0 / 60.0)));
        assert!(RealtimeEventGameCopy::Always.wanted(reading(60, 0.0)));
        assert!(!RealtimeEventGameCopy::Never.wanted(reading(60, 0.5)));
        for policy in [
            RealtimeEventGameCopy::Always,
            RealtimeEventGameCopy::OnLoss,
            RealtimeEventGameCopy::Never,
        ] {
            assert_eq!(RealtimeEventGameCopy::parse(policy.as_str()), Ok(policy));
        }
        assert!(RealtimeEventGameCopy::parse("sometimes").is_err());
    }

    #[test]
    fn a_ticks_events_queue_together_and_leave_as_one_batch() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let queue = RtcOutboundQueue::with_limits(metrics.clone(), 8, 64 * 1024);
        let events: Vec<_> = (1..=3).map(test_realtime_event).collect();
        queue.enqueue_events(events.clone()).unwrap();
        queue.enqueue(test_snapshot(4)).unwrap();
        assert_eq!(metrics.snapshot().queue_packets, 4);
        let RtcOutboundReceive::Packets(packets) = queue.receive(RTC_OUTBOUND_DRAIN_LIMIT) else {
            panic!("queued packets");
        };
        let sent = crate::realtime_event::batch_realtime_events(packets);
        assert_eq!(sent.len(), 2);
        assert_eq!(
            crate::realtime_event::expand_realtime_event_batch(&sent[0]),
            Ok(events)
        );
        assert_eq!(sent[1], test_snapshot(4));

        // Events that would exceed the queue's limits fail the session as
        // single sends do.
        let queue = RtcOutboundQueue::with_limits(metrics.clone(), 2, 64 * 1024);
        assert!(
            queue
                .enqueue_events((1..=3).map(test_realtime_event).collect())
                .is_err()
        );
        assert!(matches!(
            queue.receive(RTC_OUTBOUND_DRAIN_LIMIT),
            RtcOutboundReceive::ReliableOverflow
        ));
    }

    #[test]
    fn e2e_repel_reorder_classification_uses_authenticated_event_id_and_weapon_type() {
        assert_eq!(
            realtime_reorder_packet_order(&test_realtime_weapon_event(11, 8)),
            Some(RealtimeReorderPacketOrder {
                event_id: 11,
                weapon_type: 8,
                packet_kind: crate::realtime_event::REALTIME_EVENT_WEAPON,
            })
        );
        assert_eq!(
            realtime_reorder_packet_order(&test_realtime_weapon_event(12, 5)),
            Some(RealtimeReorderPacketOrder {
                event_id: 12,
                weapon_type: 5,
                packet_kind: crate::realtime_event::REALTIME_EVENT_WEAPON,
            })
        );
        let mut event = crate::realtime_event::AuthenticatedRealtimeEvent::parse(
            &test_realtime_weapon_event(13, 8),
        )
        .unwrap();
        let crate::realtime_event::RealtimeEventPayload::Weapon(source) = event.payload else {
            unreachable!()
        };
        let state = crate::realtime_event::RealtimeProjectileStateEvent {
            source_input_sequence: Some(u16::MAX),
            terminal: false,
            motion_frozen: false,
            source,
            projectile_id: 99,
            source_server_tick: event.server_tick,
            source_projectile_index: 0,
            weapon_data: 8,
            position_x: source.x,
            position_y: source.y,
            velocity_x: source.velocity_x,
            velocity_y: source.velocity_y,
            expiry_tick: Some(event.server_tick.wrapping_add(100)),
            behavior: crate::realtime_event::PROJECTILE_BEHAVIOR_STANDARD,
            bounces_remaining: 0,
            proximity: None,
        };
        event.payload = crate::realtime_event::RealtimeEventPayload::ProjectileState(state);
        assert_eq!(
            realtime_reorder_packet_order(&event.encode()),
            Some(RealtimeReorderPacketOrder {
                event_id: 13,
                weapon_type: 8,
                packet_kind: crate::realtime_event::REALTIME_EVENT_PROJECTILE_STATE,
            })
        );
        event.payload = crate::realtime_event::RealtimeEventPayload::ProjectileState(
            crate::realtime_event::RealtimeProjectileStateEvent {
                terminal: true,
                motion_frozen: false,
                ..state
            },
        );
        assert_eq!(
            realtime_reorder_packet_order(&event.encode()),
            None,
            "terminals must never be trapped waiting for a future Repel"
        );
        assert_eq!(realtime_reorder_packet_order(&test_snapshot(12)), None);
    }

    fn valid_channel_metadata(label: &str) -> RtcChannelMetadata {
        RtcChannelMetadata {
            label: label.to_string(),
            ordered: label == RELIABLE_CHANNEL_LABEL,
            max_packet_life_time: (label == GAME_CHANNEL_LABEL)
                .then_some(GAME_CHANNEL_MAX_PACKET_LIFETIME_MS),
            max_retransmits: None,
            protocol: String::new(),
            negotiated: false,
        }
    }

    fn test_snapshot(marker: u8) -> Vec<u8> {
        crate::snapshot_wire::SnapshotFrame {
            header: crate::snapshot_wire::SnapshotHeader {
                snapshot_id: u16::from(marker),
                arena_stream_id: 1,
                ..Default::default()
            },
            ..Default::default()
        }
        .encode()
    }

    fn test_realtime_event(event_id: u32) -> Vec<u8> {
        test_realtime_weapon_event(event_id, 2)
    }

    fn test_realtime_weapon_event(event_id: u32, weapon_data: u16) -> Vec<u8> {
        crate::realtime_event::AuthenticatedRealtimeEvent {
            server_session_id: 1,
            arena_stream_id: 1,
            event_id,
            server_tick: event_id,
            payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                crate::realtime_event::RealtimeWeaponEvent {
                    source_player_id: 7,
                    source_session_id: 2,
                    source_life_seq: 3,
                    shot_group_id: 4,
                    x: 100.0,
                    y: 200.0,
                    velocity_x: 3.0,
                    velocity_y: 4.0,
                    orientation: 0.5,
                    frequency: 5,
                    ship: 1,
                    weapon_data,
                },
            ),
        }
        .encode()
    }

    fn test_rocket_snapshot(snapshot_id: u16, rockets: u8) -> Vec<u8> {
        crate::snapshot_wire::SnapshotFrame {
            header: crate::snapshot_wire::SnapshotHeader {
                snapshot_id,
                ..Default::default()
            },
            events: vec![crate::snapshot_wire::SnapshotEvent::RocketState(
                crate::snapshot_wire::SnapshotRocketState {
                    life_seq: 1,
                    rockets,
                    ..Default::default()
                },
            )],
            ..Default::default()
        }
        .encode()
    }

    fn test_weapon_snapshot(snapshot_id: u16, weapon: u16) -> Vec<u8> {
        crate::snapshot_wire::SnapshotFrame {
            header: crate::snapshot_wire::SnapshotHeader {
                flags: crate::snapshot_wire::SNAPSHOT_FLAG_FULL,
                snapshot_id,
                ..Default::default()
            },
            near_entities: vec![crate::snapshot_wire::SnapshotEntity {
                entity_id: 7,
                field_mask: crate::snapshot_wire::SnapshotEntity::FIELD_WEAPON,
                weapon: Some(weapon),
                ..Default::default()
            }],
            ..Default::default()
        }
        .encode()
    }

    fn valid_open_channel_protocol() -> RtcChannelProtocol {
        let mut protocol = RtcChannelProtocol::default();
        for label in [
            GAME_CHANNEL_LABEL,
            EVENT_CHANNEL_LABEL,
            RELIABLE_CHANNEL_LABEL,
        ] {
            let kind = protocol
                .announce(&valid_channel_metadata(label))
                .expect("valid channel metadata");
            protocol.mark_open(kind);
        }
        protocol
    }

    async fn build_real_test_peer_offer(
        game_init: RTCDataChannelInit,
        event_init: RTCDataChannelInit,
        reliable_init: RTCDataChannelInit,
    ) -> (Arc<dyn PeerConnection>, Vec<Arc<dyn DataChannel>>, Vec<u8>) {
        let (gather_complete_tx, mut gather_complete_rx) = tokio::sync::mpsc::channel(1);
        let peer = PeerConnectionBuilder::new()
            .with_configuration(RTCConfiguration::default())
            .with_handler(Arc::new(GatherOnlyHandler { gather_complete_tx }))
            .with_runtime(Arc::new(TokioRuntime))
            .with_udp_addrs(vec!["127.0.0.1:0".to_string()])
            .build()
            .await
            .expect("real test peer should build");
        let peer: Arc<dyn PeerConnection> = Arc::new(peer);
        let game = peer
            .create_data_channel(GAME_CHANNEL_LABEL, Some(game_init))
            .await
            .expect("real test game channel should build");
        let event = peer
            .create_data_channel(EVENT_CHANNEL_LABEL, Some(event_init))
            .await
            .expect("real test event channel should build");
        let reliable = peer
            .create_data_channel(RELIABLE_CHANNEL_LABEL, Some(reliable_init))
            .await
            .expect("real test reliable channel should build");
        let offer = peer
            .create_offer(None)
            .await
            .expect("real test offer should build");
        peer.set_local_description(offer)
            .await
            .expect("real test local description should apply");
        tokio::time::timeout(Duration::from_secs(5), gather_complete_rx.recv())
            .await
            .expect("real test ICE gathering should finish")
            .expect("real test ICE observer should remain available");
        let offer = peer
            .local_description()
            .await
            .expect("real test local offer should exist");
        (
            peer,
            vec![game, event, reliable],
            serde_json::to_vec(&offer).expect("real test offer should serialize"),
        )
    }

    fn connect_real_test_peer(
        runtime: &tokio::runtime::Runtime,
        state: &Arc<Mutex<ServerState>>,
        peer_label: &str,
        game_init: RTCDataChannelInit,
        event_init: RTCDataChannelInit,
        reliable_init: RTCDataChannelInit,
    ) -> (
        Arc<dyn PeerConnection>,
        Vec<Arc<dyn DataChannel>>,
        Weak<RtcSession>,
    ) {
        let (peer, channels, offer_json) = runtime.block_on(build_real_test_peer_offer(
            game_init,
            event_init,
            reliable_init,
        ));
        let answer_json =
            create_answer_for_offer_json(Arc::clone(state), peer_label.to_string(), &offer_json)
                .expect("real test offer should be accepted for DCEP negotiation");
        let session = take_test_rtc_session(peer_label);
        let answer = serde_json::from_str(&answer_json).expect("real test answer should decode");
        runtime.block_on(async {
            peer.set_remote_description(answer)
                .await
                .expect("real test remote answer should apply");
        });
        (peer, channels, session)
    }

    fn wait_for_real_peer_condition(label: &str, timeout: Duration, condition: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if condition() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {label}");
    }

    #[test]
    fn real_reliable_data_channel_burst_drains_through_application_ingress_fifo() {
        const MESSAGE_COUNT: u32 = 600;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("real ingress runtime should build");
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));
        let (peer, channels, session) = connect_real_test_peer(
            &runtime,
            &state,
            "reliable-ingress-burst",
            game_data_channel_init(),
            event_data_channel_init(),
            reliable_data_channel_init(),
        );
        let drain_tasks = runtime.block_on(async {
            channels
                .iter()
                .cloned()
                .map(|channel| tokio::spawn(async move { while channel.poll().await.is_some() {} }))
                .collect::<Vec<_>>()
        });
        wait_for_real_peer_condition(
            "reliable ingress peer registration",
            Duration::from_secs(5),
            || state.lock().unwrap().clients.len() == 1,
        );
        let session = session.upgrade().expect("registered RTC session");
        let identity = match *session.lifecycle.lock().unwrap() {
            RtcSessionLifecycle::Registered(identity) => identity,
            lifecycle => panic!("expected registered session, got {lifecycle:?}"),
        };
        crate::arena::join_arena(&state, identity.player_id, 0);
        state
            .lock()
            .unwrap()
            .clients
            .get_mut(&identity.player_id)
            .expect("joined client")
            .life_position_sequence_base = None;
        *session.joined.lock().unwrap() = true;
        let received_before = state
            .lock()
            .unwrap()
            .rtc_overload_metrics
            .snapshot()
            .inbound_packets[2];

        let reliable_channel = Arc::clone(&channels[2]);
        runtime.block_on(async {
            for message_index in 0..MESSAGE_COUNT {
                let sequence = message_index as u16;
                let input_record = current_input_record(sequence, 0, 7);
                reliable_channel
                    .send(BytesMut::from(input_record.as_slice()))
                    .await
                    .expect("reliable DataChannel burst send");
            }
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline && {
            let locked = state.lock().unwrap();
            locked.clients[&identity.player_id].last_input_seq != Some((MESSAGE_COUNT - 1) as u16)
                || locked.rtc_overload_metrics.snapshot().inbound_packets[2]
                    != received_before + MESSAGE_COUNT as usize
        } {
            thread::sleep(Duration::from_millis(5));
        }
        let observed_tail = state.lock().unwrap().clients[&identity.player_id].last_input_seq;
        assert_eq!(
            observed_tail,
            Some((MESSAGE_COUNT - 1) as u16),
            "real application reliable FIFO did not reach the burst tail; metrics={:?}",
            state.lock().unwrap().rtc_overload_metrics.snapshot(),
        );
        let metrics = state.lock().unwrap().rtc_overload_metrics.snapshot();
        assert_eq!(
            metrics.inbound_packets[2] - received_before,
            MESSAGE_COUNT as usize,
            "every real reliable DataChannel message must reach the application handoff",
        );
        assert_eq!(metrics.inbound_queue_packets, [0, 0, 0]);
        assert_eq!(metrics.inbound_backpressure, [0, 0, 0]);
        assert!(
            metrics.inbound_peak_queue_packets[2] > 0,
            "the real reliable burst must traverse the measured handoff",
        );

        disconnect_rtc_session(&session);
        session.admission.release();
        runtime.block_on(async {
            peer.close().await.expect("real ingress peer should close");
            for task in drain_tasks {
                task.abort();
                let _ = task.await;
            }
        });
        drop(channels);
        drop(peer);
        drop(session);
        wait_for_real_peer_condition("real ingress peer cleanup", Duration::from_secs(5), || {
            let state = state.lock().unwrap();
            state.clients.is_empty() && state.rtc_sessions == 0
        });
    }

    #[test]
    fn repeated_real_peer_churn_reclaims_normal_and_rejected_sessions() {
        const CHURN_CYCLES: usize = 6;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("real peer churn runtime should build");
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        })));

        for cycle in 0..CHURN_CYCLES {
            let normal_label = format!("reclaim-normal-{cycle}");
            let (normal_peer, normal_channels, normal_session) = connect_real_test_peer(
                &runtime,
                &state,
                &normal_label,
                game_data_channel_init(),
                event_data_channel_init(),
                reliable_data_channel_init(),
            );
            wait_for_real_peer_condition(
                "normal peer registration",
                Duration::from_secs(5),
                || state.lock().unwrap().clients.len() == 1,
            );
            let registered_session = normal_session
                .upgrade()
                .expect("registered real test session should still be live");
            disconnect_rtc_session(&registered_session);
            drop(registered_session);
            runtime.block_on(async {
                normal_peer
                    .close()
                    .await
                    .expect("normal real test peer should close");
            });
            drop(normal_channels);
            drop(normal_peer);
            wait_for_real_peer_condition(
                "normal peer counter cleanup",
                Duration::from_secs(5),
                || {
                    let state = state.lock().unwrap();
                    state.clients.is_empty() && state.rtc_sessions == 0
                },
            );
            let reclamation_deadline = std::time::Instant::now() + Duration::from_secs(5);
            while normal_session.upgrade().is_some()
                && std::time::Instant::now() < reclamation_deadline
            {
                thread::sleep(Duration::from_millis(5));
            }
            if let Some(retained) = normal_session.upgrade() {
                let (game, event, reliable) = {
                    let channels = retained.channels.lock().unwrap();
                    (
                        channels.game.is_some(),
                        channels.event.is_some(),
                        channels.reliable.is_some(),
                    )
                };
                let lifecycle = *retained.lifecycle.lock().unwrap();
                let has_outbound = retained.outbound.lock().unwrap().is_some();
                panic!(
                    "normal RtcSession remained retained: strong={}, lifecycle={:?}, channels=({},{},{}), outbound={}",
                    Arc::strong_count(&retained),
                    lifecycle,
                    game,
                    event,
                    reliable,
                    has_outbound,
                );
            }

            let violations_before = state
                .lock()
                .unwrap()
                .rtc_overload_metrics
                .snapshot()
                .protocol_violations;
            let rejected_label = format!("reclaim-rejected-{cycle}");
            let (rejected_peer, rejected_channels, rejected_session) = connect_real_test_peer(
                &runtime,
                &state,
                &rejected_label,
                RTCDataChannelInit {
                    ordered: false,
                    ..Default::default()
                },
                event_data_channel_init(),
                reliable_data_channel_init(),
            );
            wait_for_real_peer_condition(
                "protocol-violation rejection",
                Duration::from_secs(5),
                || {
                    state
                        .lock()
                        .unwrap()
                        .rtc_overload_metrics
                        .snapshot()
                        .protocol_violations
                        == violations_before + 1
                },
            );
            runtime.block_on(async {
                let _ = rejected_peer.close().await;
            });
            drop(rejected_channels);
            drop(rejected_peer);
            wait_for_real_peer_condition(
                "rejected RtcSession Weak reclamation",
                Duration::from_secs(5),
                || {
                    let state = state.lock().unwrap();
                    // Observing reclamation must not acquire the last Arc
                    // under the lock needed by RtcAdmission::drop.
                    rejected_session.strong_count() == 0
                        && state.clients.is_empty()
                        && state.rtc_sessions == 0
                },
            );
        }

        assert_eq!(
            state
                .lock()
                .unwrap()
                .rtc_overload_metrics
                .snapshot()
                .protocol_violations,
            CHURN_CYCLES
        );
    }

    fn browser_style_offer_json() -> Vec<u8> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build");
        runtime.block_on(async {
            let (gather_complete_tx, mut gather_complete_rx) = tokio::sync::mpsc::channel(1);
            let peer = PeerConnectionBuilder::new()
                .with_configuration(RTCConfiguration::default())
                .with_handler(Arc::new(GatherOnlyHandler { gather_complete_tx }))
                .with_runtime(Arc::new(TokioRuntime))
                .with_udp_addrs(vec!["127.0.0.1:0".to_string()])
                .build()
                .await
                .expect("test peer should be created");
            let _game = peer
                .create_data_channel(GAME_CHANNEL_LABEL, Some(game_data_channel_init()))
                .await
                .expect("game channel should be created");
            let _event = peer
                .create_data_channel(EVENT_CHANNEL_LABEL, Some(event_data_channel_init()))
                .await
                .expect("event channel should be created");
            let _reliable = peer
                .create_data_channel(RELIABLE_CHANNEL_LABEL, Some(reliable_data_channel_init()))
                .await
                .expect("reliable channel should be created");
            let offer = peer
                .create_offer(None)
                .await
                .expect("offer should be created");
            peer.set_local_description(offer)
                .await
                .expect("local offer should be set");
            let _ = gather_complete_rx.recv().await;
            let offer = peer
                .local_description()
                .await
                .expect("local offer should exist");
            let offer_json = serde_json::to_vec(&offer).expect("offer should serialize to JSON");
            peer.close().await.expect("test peer should close");
            offer_json
        })
    }

    fn adjacent_udp_sockets() -> (std::net::UdpSocket, std::net::UdpSocket) {
        for base in 12_000_u16..30_000 {
            let Ok(first) = std::net::UdpSocket::bind(("0.0.0.0", base)) else {
                continue;
            };
            if let Ok(second) = std::net::UdpSocket::bind(("0.0.0.0", base + 1)) {
                return (first, second);
            }
        }
        panic!("test could not reserve two adjacent UDP ports");
    }

    struct GatherOnlyHandler {
        gather_complete_tx: tokio::sync::mpsc::Sender<()>,
    }

    #[async_trait::async_trait]
    impl PeerConnectionEventHandler for GatherOnlyHandler {
        async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
            if state == RTCIceGatheringState::Complete {
                let _ = self.gather_complete_tx.try_send(());
            }
        }
    }
    #[test]
    fn fresh_snapshot_is_selected_despite_a_full_control_writer_batch() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let queue = RtcOutboundQueue::with_limits(Arc::clone(&metrics), 4096, 4 * 1024 * 1024);
        for id in 0..257u32 {
            queue
                .enqueue(crate::reliable::reliable(id, &[0x07]))
                .unwrap();
        }
        let snapshot = test_snapshot(77);
        queue.enqueue(snapshot.clone()).unwrap();
        let RtcOutboundReceive::Packets(first) = queue.receive(RTC_OUTBOUND_DRAIN_LIMIT) else {
            panic!("packets")
        };
        let present = first.iter().any(|p| p == &snapshot);
        eprintln!(
            "WRITER_PROBE critical_backlog=257 batch_limit={} first_batch={} fresh_snapshot_in_first={} remaining={:?}",
            RTC_OUTBOUND_DRAIN_LIMIT,
            first.len(),
            present,
            queue.depth()
        );
        assert!(
            present,
            "a fresh replaceable game snapshot should not wait behind a whole control batch"
        );
    }
    #[test]
    fn inbound_byte_budget_preserves_fifo_and_releases_all_storage_on_pop_drop_and_rejection() {
        for kind in [
            RtcChannelKind::Game,
            RtcChannelKind::Event,
            RtcChannelKind::Reliable,
        ] {
            let metrics = Arc::new(RtcOverloadMetrics::default());
            let (queue, mut receiver) =
                RtcInboundQueue::with_limits(Arc::clone(&metrics), kind, 10, 5);
            queue.try_enqueue(vec![1, 2]).unwrap();
            queue.try_enqueue(vec![3, 4, 5]).unwrap();
            assert_eq!(queue.queued_bytes.load(Ordering::Relaxed), 5);
            assert_eq!(
                queue.try_enqueue(vec![6]),
                Err(RtcInboundEnqueueError::ByteLimit)
            );
            assert_eq!(
                queue.try_enqueue(vec![0; RTC_APPLICATION_INBOUND_MESSAGE_BYTES + 1]),
                Err(RtcInboundEnqueueError::Oversized)
            );
            assert_eq!(queue.queued_bytes.load(Ordering::Relaxed), 5);
            assert_eq!(receiver.try_recv(), Some(vec![1, 2]));
            assert_eq!(queue.queued_bytes.load(Ordering::Relaxed), 3);
            queue.try_enqueue(vec![7, 8]).unwrap();
            assert_eq!(queue.queued_bytes.load(Ordering::Relaxed), 5);
            drop(receiver);
            assert_eq!(queue.queued_bytes.load(Ordering::Relaxed), 0);
            assert_eq!(metrics.snapshot().inbound_queue_packets, [0, 0, 0]);
            assert_eq!(
                metrics.snapshot().inbound_packets[kind.metric_index()],
                1,
                "discarded backlog is not handled traffic"
            );
            assert_eq!(
                queue.try_enqueue(vec![9]),
                Err(RtcInboundEnqueueError::Closed)
            );
            assert_eq!(queue.queued_bytes.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn inbound_byte_reservation_is_atomic_across_producers_and_message_cap_has_exact_edges() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let (queue, mut receiver) =
            RtcInboundQueue::with_limits(Arc::clone(&metrics), RtcChannelKind::Game, 100, 512);
        std::thread::scope(|scope| {
            for id in 0..8 {
                let q = &queue;
                scope.spawn(move || {
                    for _ in 0..8 {
                        let _ = q.try_enqueue(vec![id; 32]);
                    }
                });
            }
        });
        assert_eq!(queue.queued_bytes.load(Ordering::Relaxed), 512);
        let mut drained = 0;
        while receiver.try_recv().is_some() {
            drained += 1;
        }
        assert_eq!(drained, 16);
        assert_eq!(queue.queued_bytes.load(Ordering::Relaxed), 0);
        let (queue, mut receiver) =
            RtcInboundQueue::with_capacity(metrics, RtcChannelKind::Game, 2);
        assert!(
            queue
                .try_enqueue(vec![0; RTC_APPLICATION_INBOUND_MESSAGE_BYTES])
                .is_ok()
        );
        assert_eq!(
            queue.try_enqueue(vec![0; RTC_APPLICATION_INBOUND_MESSAGE_BYTES + 1]),
            Err(RtcInboundEnqueueError::Oversized)
        );
        assert_eq!(
            receiver.try_recv().unwrap().len(),
            RTC_APPLICATION_INBOUND_MESSAGE_BYTES
        );
        assert_eq!(queue.queued_bytes.load(Ordering::Relaxed), 0);
    }
}
