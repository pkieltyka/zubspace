//! One bounded mailbox and one capacity waiter per SCTP stream. A slow ordered
//! stream must not park the game/event writer. The association still shares
//! congestion control; this is application scheduling, not extra bandwidth.
use super::*;

const CRITICAL_BURST: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EnqueueError {
    Closed,
    Capacity,
}

struct LanePacket {
    bytes: Arc<Vec<u8>>,
    queued_at: Instant,
}

#[derive(Default)]
struct Lane {
    bytes: usize,
    critical: VecDeque<LanePacket>,
    snapshot: Option<LanePacket>,
    consecutive_critical: usize,
}

#[derive(Default)]
struct Mailboxes {
    lanes: [Lane; 3],
    packets: usize,
    bytes: usize,
    closed: bool,
}

pub(super) struct LaneWriter {
    state: Mutex<Mailboxes>,
    ready: [Notify; 3],
    closed: watch::Sender<bool>,
    metrics: Arc<RtcOverloadMetrics>,
    packet_limit: usize,
    byte_limit: usize,
}

impl LaneWriter {
    pub(super) fn new(metrics: Arc<RtcOverloadMetrics>) -> Arc<Self> {
        Self::with_limits(
            metrics,
            RTC_APPLICATION_RELIABLE_QUEUE_PACKETS,
            RTC_APPLICATION_QUEUE_BYTES,
        )
    }

    fn with_limits(
        metrics: Arc<RtcOverloadMetrics>,
        packet_limit: usize,
        byte_limit: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(Mailboxes::default()),
            ready: std::array::from_fn(|_| Notify::new()),
            closed: watch::channel(false).0,
            metrics,
            packet_limit,
            byte_limit,
        })
    }

    /// The dispatcher retains the original reliable id until the actual lane
    /// send completes. Exact event copies share immutable storage, but each
    /// destination is charged against the aggregate retained-byte ceiling.
    pub(super) fn enqueue(&self, index: usize, bytes: Arc<Vec<u8>>) -> Result<(), EnqueueError> {
        let snapshot = replaceable_realtime_server_packet(&bytes);
        let mut state = self.state.lock().expect("RTC lane mailboxes");
        if state.closed {
            return Err(EnqueueError::Closed);
        }
        let previous = if snapshot {
            state.lanes[index].snapshot.as_ref()
        } else {
            None
        };
        let previous_bytes = previous.map_or(0, |p| p.bytes.len());
        let added_packet = usize::from(previous.is_none());
        let next_bytes = state.bytes - previous_bytes + bytes.len();
        if state.packets + added_packet > self.packet_limit || next_bytes > self.byte_limit {
            // Only snapshots are replaceable. Critical facts must cause
            // explicit session recovery rather than silent reliable loss.
            if snapshot {
                if state.lanes[index].snapshot.take().is_some() {
                    state.lanes[index].bytes -= previous_bytes;
                    state.packets -= 1;
                    state.bytes -= previous_bytes;
                    self.metrics.remove_queue(1, previous_bytes);
                }
                self.metrics.record_realtime_coalesced();
                return Ok(());
            }
            return Err(EnqueueError::Capacity);
        }
        let packet = LanePacket {
            bytes,
            queued_at: Instant::now(),
        };
        state.lanes[index].bytes = state.lanes[index].bytes - previous_bytes + packet.bytes.len();
        if snapshot {
            if state.lanes[index].snapshot.replace(packet).is_some() {
                self.metrics.replace_queue_bytes(
                    previous_bytes,
                    state.lanes[index].snapshot.as_ref().unwrap().bytes.len(),
                );
                self.metrics.record_realtime_coalesced();
            } else {
                self.metrics.add_queue(1, next_bytes - state.bytes);
            }
        } else {
            self.metrics.add_queue(1, packet.bytes.len());
            state.lanes[index].critical.push_back(packet);
        }
        state.packets += added_packet;
        state.bytes = next_bytes;
        drop(state);
        self.ready[index].notify_one();
        Ok(())
    }

    async fn wait_ready(&self, index: usize) -> bool {
        loop {
            let notified = self.ready[index].notified();
            {
                let state = self.state.lock().expect("RTC lane mailboxes");
                if state.closed {
                    return false;
                }
                let lane = &state.lanes[index];
                if lane.snapshot.is_some() || !lane.critical.is_empty() {
                    return true;
                }
            }
            notified.await;
        }
    }

    // Called only AFTER send capacity is available, so a stale selected
    // snapshot cannot sit in a blocked send while newer state replaces it.
    fn pop(&self, index: usize) -> Option<LanePacket> {
        let mut state = self.state.lock().expect("RTC lane mailboxes");
        if state.closed {
            return None;
        }
        let lane = &mut state.lanes[index];
        let packet = if lane.snapshot.is_some()
            && (lane.critical.is_empty() || lane.consecutive_critical >= CRITICAL_BURST)
        {
            lane.consecutive_critical = 0;
            lane.snapshot.take()
        } else if let Some(packet) = lane.critical.pop_front() {
            lane.consecutive_critical = lane.consecutive_critical.saturating_add(1);
            Some(packet)
        } else {
            None
        }?;
        lane.bytes -= packet.bytes.len();
        state.packets -= 1;
        state.bytes -= packet.bytes.len();
        self.metrics.remove_queue(1, packet.bytes.len());
        Some(packet)
    }

    pub(super) fn close(&self) {
        let mut state = self.state.lock().expect("RTC lane mailboxes");
        self.metrics.remove_queue(state.packets, state.bytes);
        *state = Mailboxes {
            closed: true,
            ..Mailboxes::default()
        };
        drop(state);
        self.closed.send_replace(true);
        for ready in &self.ready {
            ready.notify_waiters();
        }
    }

    pub(super) fn diagnostic_snapshot(&self, now: Instant) -> crate::state::RtcLaneBacklog {
        let state = self.state.lock().expect("RTC lane mailboxes");
        let mut result = crate::state::RtcLaneBacklog::default();
        for (index, lane) in state.lanes.iter().enumerate() {
            result.packets[index] = lane.critical.len() + usize::from(lane.snapshot.is_some());
            result.bytes[index] = lane.bytes;
            let oldest = lane
                .critical
                .front()
                .into_iter()
                .chain(lane.snapshot.as_ref())
                .map(|packet| packet.queued_at)
                .min();
            result.oldest_age_us[index] = oldest.map_or(0, |at| {
                now.saturating_duration_since(at)
                    .as_micros()
                    .min(u64::MAX as u128) as u64
            });
        }
        result
    }

    pub(super) fn start(
        self: &Arc<Self>,
        handle: &tokio::runtime::Handle,
        session: &Arc<RtcSession>,
        outbound: &Arc<RtcOutboundQueue>,
    ) {
        let channels = session.channels.lock().expect("RTC channels");
        for (index, channel) in [&channels.game, &channels.event, &channels.reliable]
            .into_iter()
            .enumerate()
        {
            if let Some(channel) = channel {
                let (writer, session, outbound, channel) = (
                    Arc::clone(self),
                    Arc::clone(session),
                    Arc::clone(outbound),
                    Arc::clone(channel),
                );
                handle.spawn(async move {
                    let mut lifecycle = session.lifecycle_changed.subscribe();
                    let transport = ChannelTransport(channel);
                    let drive = writer.drive(index, &transport, |packet| {
                        outbound.complete_send(packet);
                    }, |packet, sent_at| {
                        if packet.first() != Some(&crate::connection_quality_protocol::ConnectionQualityProbe::TYPE) { return true; }
                        let Ok(probe) = crate::connection_quality_protocol::ConnectionQualityProbe::parse(packet) else { return false; };
                        outbound.quality.lock().expect("session quality").begin_dispatch(probe, sent_at)
                    });
                    tokio::pin!(drive);
                    loop {
                        if matches!(*lifecycle.borrow_and_update(), RtcSessionLifecycle::Closing | RtcSessionLifecycle::Closed) { break; }
                        tokio::select! {
                            result = &mut drive => {
                                if let Err(error) = result {
                                    warn!(peer = %session.peer, channel_index = index, error, "server rtc lane writer failed");
                                    let cleanup_session = Arc::clone(&session);
                                    let _ = tokio::task::spawn_blocking(move || disconnect_rtc_session(&cleanup_session)).await;
                                }
                                break;
                            }
                            changed = lifecycle.changed() => { if changed.is_err() { break; } }
                        }
                    }
                    writer.close();
                });
            }
        }
    }

    async fn drive<T: LaneTransport>(
        &self,
        index: usize,
        transport: &T,
        complete: impl Fn(&[u8]),
        begin: impl Fn(&[u8], Instant) -> bool,
    ) -> Result<(), String> {
        let mut closed = self.closed.subscribe();
        if *closed.borrow() {
            return Ok(());
        }
        tokio::select! {
            result = self.drive_open(index, transport, complete, begin) => result,
            _ = closed.changed() => Ok(()),
        }
    }

    async fn drive_open<T: LaneTransport>(
        &self,
        index: usize,
        transport: &T,
        complete: impl Fn(&[u8]),
        begin: impl Fn(&[u8], Instant) -> bool,
    ) -> Result<(), String> {
        while self.wait_ready(index).await {
            let outstanding = transport.outstanding_bytes().await?;
            {
                // Drop records completed AND cancelled waits. Teardown cannot
                // erase evidence of the capacity wait that preceded it.
                let mut wait = CapacityWait {
                    metrics: &self.metrics,
                    index,
                    started: Instant::now(),
                    waited: false,
                    outstanding,
                };
                let writable = transport.writable();
                tokio::pin!(writable);
                std::future::poll_fn(|cx| {
                    let result = std::future::Future::poll(writable.as_mut(), cx);
                    wait.waited |= result.is_pending();
                    result
                })
                .await?;
            }
            let Some(packet) = self.pop(index) else {
                continue;
            };
            if !begin(&packet.bytes, Instant::now()) {
                complete(&packet.bytes);
                continue;
            }
            transport.send(&packet.bytes).await?;
            self.metrics
                .record_lane_send(index, packet.bytes.len(), packet.queued_at.elapsed());
            complete(&packet.bytes);
        }
        Ok(())
    }
}

struct CapacityWait<'a> {
    metrics: &'a RtcOverloadMetrics,
    index: usize,
    started: Instant,
    waited: bool,
    outstanding: usize,
}

impl Drop for CapacityWait<'_> {
    fn drop(&mut self) {
        self.metrics.record_lane_capacity(
            self.index,
            self.waited,
            self.started.elapsed(),
            self.outstanding,
        );
    }
}

impl Drop for LaneWriter {
    fn drop(&mut self) {
        self.close();
    }
}

#[async_trait::async_trait]
trait LaneTransport: Send + Sync {
    async fn outstanding_bytes(&self) -> Result<usize, String> {
        Ok(0)
    }
    async fn writable(&self) -> Result<(), String>;
    async fn send(&self, packet: &[u8]) -> Result<(), String>;
}

struct ChannelTransport(Arc<dyn DataChannel>);

#[async_trait::async_trait]
impl LaneTransport for ChannelTransport {
    async fn outstanding_bytes(&self) -> Result<usize, String> {
        self.0
            .outstanding_bytes()
            .await
            .map_err(|error| error.to_string())
    }
    async fn writable(&self) -> Result<(), String> {
        self.0.writable().await.map_err(|error| error.to_string())
    }
    async fn send(&self, packet: &[u8]) -> Result<(), String> {
        self.0
            .send(BytesMut::from(packet))
            .await
            .map_err(|error| error.to_string())
    }
}

/// Ensures every dispatcher exit wakes lane tasks, even on an unwinding
/// diagnostic path. Normal session teardown also interrupts capacity waits.
pub(super) struct DispatcherGuard(pub(super) Arc<LaneWriter>);
impl Drop for DispatcherGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeTransport {
        capacity: tokio::sync::Semaphore,
        attempted: AtomicUsize,
        sent: Mutex<Vec<Vec<u8>>>,
    }

    impl FakeTransport {
        fn new(capacity: usize) -> Arc<Self> {
            Arc::new(Self {
                capacity: tokio::sync::Semaphore::new(capacity),
                attempted: AtomicUsize::new(0),
                sent: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl LaneTransport for FakeTransport {
        async fn writable(&self) -> Result<(), String> {
            self.attempted.fetch_add(1, Ordering::Relaxed);
            self.capacity
                .acquire()
                .await
                .map_err(|error| error.to_string())?
                .forget();
            Ok(())
        }
        async fn send(&self, packet: &[u8]) -> Result<(), String> {
            self.sent.lock().unwrap().push(packet.to_vec());
            Ok(())
        }
    }

    fn snapshot(id: u16) -> Arc<Vec<u8>> {
        Arc::new(
            crate::snapshot_wire::SnapshotFrame {
                header: crate::snapshot_wire::SnapshotHeader {
                    snapshot_id: id,
                    arena_stream_id: 1,
                    ..Default::default()
                },
                ..Default::default()
            }
            .encode(),
        )
    }

    async fn until(mut predicate: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !predicate() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("lane progress deadline");
    }

    #[tokio::test]
    async fn blocked_control_lane_does_not_block_game_or_events_and_recovers_in_fifo_order() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let writer = LaneWriter::new(Arc::clone(&metrics));
        let transports = [
            FakeTransport::new(32),
            FakeTransport::new(32),
            FakeTransport::new(0),
        ];
        let mut tasks = Vec::new();
        for (index, transport) in transports.iter().enumerate() {
            let (writer, transport) = (Arc::clone(&writer), Arc::clone(transport));
            tasks.push(tokio::spawn(async move {
                writer
                    .drive(index, transport.as_ref(), |_| {}, |_, _| true)
                    .await
            }));
        }
        for id in 0..10 {
            writer.enqueue(2, Arc::new(vec![1, id])).unwrap();
        }
        until(|| transports[2].attempted.load(Ordering::Relaxed) != 0).await;
        let event = Arc::new(vec![0x7d, 1]);
        writer.enqueue(0, Arc::clone(&event)).unwrap();
        writer.enqueue(0, snapshot(1)).unwrap();
        writer.enqueue(1, Arc::clone(&event)).unwrap();
        until(|| {
            transports[0].sent.lock().unwrap().len() == 2
                && transports[1].sent.lock().unwrap().len() == 1
        })
        .await;
        assert!(transports[2].sent.lock().unwrap().is_empty());
        assert_eq!(metrics.snapshot().queue_packets, 10);
        transports[2].capacity.add_permits(10);
        until(|| transports[2].sent.lock().unwrap().len() == 10).await;
        assert_eq!(
            *transports[2].sent.lock().unwrap(),
            (0..10).map(|id| vec![1, id]).collect::<Vec<_>>()
        );
        writer.close();
        for task in tasks {
            assert!(task.await.unwrap().is_ok());
        }
        assert_eq!(metrics.snapshot().queue_packets, 0);
        assert_eq!(metrics.snapshot().queue_bytes, 0);
        assert_eq!(
            metrics.snapshot().outbound_bytes[0],
            event.len() + snapshot(1).len()
        );
        assert_eq!(metrics.snapshot().outbound_bytes[1], event.len());
    }

    #[tokio::test]
    async fn capacity_wait_keeps_latest_snapshot_and_close_cancels_blocked_lane_without_leaks() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let writer = LaneWriter::new(Arc::clone(&metrics));
        let transport = FakeTransport::new(0);
        writer.enqueue(0, snapshot(1)).unwrap();
        let (w, t) = (Arc::clone(&writer), Arc::clone(&transport));
        let task = tokio::spawn(async move { w.drive(0, t.as_ref(), |_| {}, |_, _| true).await });
        until(|| transport.attempted.load(Ordering::Relaxed) != 0).await;
        writer.enqueue(0, snapshot(2)).unwrap();
        writer.enqueue(0, snapshot(3)).unwrap();
        assert_eq!(metrics.snapshot().queue_packets, 1);
        transport.capacity.add_permits(1);
        until(|| transport.sent.lock().unwrap().len() == 1).await;
        assert_eq!(transport.sent.lock().unwrap()[0], *snapshot(3));
        writer.enqueue(0, snapshot(4)).unwrap();
        until(|| transport.attempted.load(Ordering::Relaxed) >= 2).await;
        writer.close();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
        assert_eq!(metrics.snapshot().queue_bytes, 0);
        assert_eq!(metrics.snapshot().queue_packets, 0);
        assert_eq!(transport.sent.lock().unwrap().len(), 1);
        assert_eq!(
            metrics.snapshot().outbound_capacity_waits[0],
            2,
            "cancelled capacity waits remain observable"
        );
    }

    #[test]
    fn lane_budget_is_aggregate_and_critical_fifo_survives_snapshot_service() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let writer = LaneWriter::with_limits(Arc::clone(&metrics), 3, 6);
        for lane in 0..3 {
            writer.enqueue(lane, Arc::new(vec![1, lane as u8])).unwrap();
        }
        assert!(writer.enqueue(1, Arc::new(vec![1])).is_err());
        assert_eq!(metrics.snapshot().queue_bytes, 6);
        assert_eq!(metrics.snapshot().queue_packets, 3);
        let sample = writer.diagnostic_snapshot(Instant::now() + Duration::from_secs(1));
        assert_eq!(sample.packets, [1, 1, 1]);
        assert_eq!(sample.bytes, [2, 2, 2]);
        assert!(sample.oldest_age_us.into_iter().all(|age| age >= 1_000_000));
        writer.close();
        writer.close();
        assert_eq!(metrics.snapshot().queue_bytes, 0);
        assert_eq!(metrics.snapshot().queue_packets, 0);
        assert_eq!(
            writer.diagnostic_snapshot(Instant::now()).oldest_age_us,
            [0; 3]
        );

        let writer = LaneWriter::new(metrics);
        for id in 0..20 {
            writer.enqueue(0, Arc::new(vec![0x7d, id])).unwrap();
        }
        writer.enqueue(0, snapshot(9)).unwrap();
        for id in 0..CRITICAL_BURST {
            assert_eq!(*writer.pop(0).unwrap().bytes, vec![0x7d, id as u8]);
        }
        assert_eq!(*writer.pop(0).unwrap().bytes, *snapshot(9));
        for id in CRITICAL_BURST..20 {
            assert_eq!(*writer.pop(0).unwrap().bytes, vec![0x7d, id as u8]);
        }
        assert!(writer.pop(0).is_none());
    }

    #[tokio::test]
    async fn reliable_id_remains_owned_until_actual_lane_send_and_close_drains_mailboxes_synchronously()
     {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let outbound = Arc::new(RtcOutboundQueue::new(Arc::clone(&metrics)));
        let writer = LaneWriter::new(Arc::clone(&metrics));
        outbound.attach_lane_writer(&writer);
        let packet = crate::reliable::reliable(17, &[9, 8, 7]);
        outbound.enqueue(packet.clone()).unwrap();
        let RtcOutboundReceive::Packets(batch) = outbound.receive(1) else {
            panic!("queued reliable frame");
        };
        writer.enqueue(2, Arc::new(batch[0].clone())).unwrap();
        outbound.enqueue(packet.clone()).unwrap();
        assert_eq!(
            outbound.depth(),
            (0, 0),
            "timer retry stays coalesced in a lane capacity wait"
        );
        let transport = FakeTransport::new(0);
        let (w, t, o) = (
            Arc::clone(&writer),
            Arc::clone(&transport),
            Arc::clone(&outbound),
        );
        let task = tokio::spawn(async move {
            w.drive(2, t.as_ref(), |packet| o.complete_send(packet), |_, _| true)
                .await
        });
        until(|| transport.attempted.load(Ordering::Relaxed) != 0).await;
        transport.capacity.add_permits(1);
        until(|| {
            transport.sent.lock().unwrap().len() == 1
                && !outbound
                    .state
                    .lock()
                    .unwrap()
                    .active_reliable_ids
                    .contains(&17)
        })
        .await;
        outbound.enqueue(packet).unwrap();
        writer.enqueue(0, snapshot(9)).unwrap();
        outbound.close();
        assert_eq!(
            metrics.snapshot().queue_packets,
            0,
            "terminal publication must not precede mailbox cleanup"
        );
        assert_eq!(metrics.snapshot().queue_bytes, 0);
        assert!(task.await.unwrap().is_ok());
        assert_eq!(metrics.snapshot().outbound_packets, [0, 0, 1]);
        assert_eq!(metrics.snapshot().outbound_capacity_waits, [0, 0, 1]);
    }

    #[test]
    fn rtc_closing_lane_does_not_invent_reliable_capacity_overflow() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let outbound = RtcOutboundQueue::new(Arc::clone(&metrics));
        let writer = LaneWriter::with_limits(Arc::clone(&metrics), 4, 128);
        outbound.attach_lane_writer(&writer);
        // Model an already-drained dispatcher packet racing a worker that
        // received Closing, while the central queue still awaits cleanup.
        outbound.enqueue(vec![1, 2]).unwrap();
        writer.enqueue(2, Arc::new(vec![1, 3])).unwrap();
        writer.close();
        assert!(!enqueue_rtc_lane(
            &outbound,
            &writer,
            2,
            Arc::new(vec![1, 4])
        ));
        assert_eq!(
            metrics.snapshot().reliable_overflows,
            0,
            "normal closed-lane rejection must not manufacture capacity pressure"
        );
        let state = outbound.state.lock().unwrap();
        assert!(
            state.closed,
            "a closed writer cannot leave a live dispatcher queue"
        );
        assert!(!state.reliable_overflow);
        assert_eq!(metrics.snapshot().queue_packets, 0);
        assert_eq!(metrics.snapshot().queue_bytes, 0);
    }

    #[test]
    fn full_lane_omits_optional_measurement_but_critical_overflow_requires_session_recovery() {
        let metrics = Arc::new(RtcOverloadMetrics::default());
        let outbound = RtcOutboundQueue::new(Arc::clone(&metrics));
        let writer = LaneWriter::with_limits(Arc::clone(&metrics), 1, 128);
        outbound.attach_lane_writer(&writer);
        writer.enqueue(2, Arc::new(vec![1])).unwrap();
        let now = Instant::now();
        let probe = {
            let mut quality = outbound.quality.lock().unwrap();
            quality.reset_scope(42, now);
            quality.reserve_probe(17, now).unwrap()
        };
        assert!(enqueue_rtc_lane(
            &outbound,
            &writer,
            0,
            Arc::new(probe.encode())
        ));
        assert!(!outbound.state.lock().unwrap().closed);
        assert_eq!(metrics.snapshot().reliable_overflows, 0);
        assert!(!outbound.quality.lock().unwrap().begin_dispatch(probe, now));
        assert!(!enqueue_rtc_lane(
            &outbound,
            &writer,
            1,
            Arc::new(vec![0x7d, 1])
        ));
        assert!(outbound.state.lock().unwrap().closed);
        assert_eq!(metrics.snapshot().reliable_overflows, 1);
        assert_eq!(metrics.snapshot().queue_packets, 0);
        assert_eq!(metrics.snapshot().queue_bytes, 0);
    }
}
