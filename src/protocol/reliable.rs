use super::*;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkAssembler {
    pub(crate) data: Vec<u8>,
    pub(crate) len: usize,
    pub(crate) fragments: usize,
    pub(crate) target_len: Option<usize>,
}

impl ChunkAssembler {
    #[allow(dead_code)]
    pub(crate) fn push(&mut self, data: &[u8]) -> Result<(), String> {
        if data.is_empty() {
            self.clear();
            return Err("empty chunk fragment".to_string());
        }
        let next_fragments = self
            .fragments
            .checked_add(1)
            .ok_or_else(|| "chunk assembly fragment count overflow".to_string())?;
        let next_len = self
            .len
            .checked_add(data.len())
            .ok_or_else(|| "chunk assembly length overflow".to_string())?;
        if next_fragments > crate::zone_wire::MAX_TRANSFER_FRAGMENTS
            || next_len > crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES
            || self.target_len.is_some_and(|target| next_len > target)
        {
            self.clear();
            return Err("chunk assembly exceeds its byte or fragment budget".to_string());
        }
        self.data.extend_from_slice(data);
        self.len = next_len;
        self.fragments = next_fragments;
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn construct(&self) -> Vec<u8> {
        self.data.clone()
    }

    #[allow(dead_code)]
    pub(crate) fn clear(&mut self) {
        // Drop the allocation, not just its length. A peer may drive an
        // assembly to the exact transfer ceiling before cancelling or
        // crossing a logical boundary; retaining that capacity would keep an
        // attacker-sized buffer alive for the rest of the session.
        self.data = Vec::new();
        self.len = 0;
        self.fragments = 0;
        self.target_len = None;
    }

    fn take_complete(&mut self) -> Vec<u8> {
        let data = std::mem::take(&mut self.data);
        self.len = 0;
        self.fragments = 0;
        self.target_len = None;
        data
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PacketChunker {
    pub(crate) small: ChunkAssembler,
    pub(crate) huge: ChunkAssembler,
    small_in_progress: bool,
}

impl PacketChunker {
    pub(crate) const CORE_BYTE: u8 = 0x00;
    pub(crate) const SMALL_BODY_TYPE: u8 = 0x08;
    pub(crate) const SMALL_TAIL_TYPE: u8 = 0x09;
    pub(crate) const HUGE_TYPE: u8 = 0x0a;
    pub(crate) const HUGE_CANCEL_TYPE: u8 = 0x0b;
    pub(crate) const HUGE_CANCEL_ACK_TYPE: u8 = 0x0c;
    pub(crate) const PACKET_CLUSTER_TYPE: u8 = 0x0e;

    pub(crate) fn clear_incomplete(&mut self) {
        self.small.clear();
        self.small_in_progress = false;
        self.huge.clear();
    }

    pub(crate) fn abort_incomplete_for_packet(&mut self, packet: &[u8]) {
        let chunk_type = packet
            .first()
            .copied()
            .zip(packet.get(1).copied())
            .filter(|(core, _)| *core == Self::CORE_BYTE)
            .map(|(_, packet_type)| packet_type);
        if self.small_in_progress
            && !matches!(
                chunk_type,
                Some(Self::SMALL_BODY_TYPE | Self::SMALL_TAIL_TYPE | Self::PACKET_CLUSTER_TYPE)
            )
        {
            self.small.clear();
            self.small_in_progress = false;
        }
        if self.huge.target_len.is_some()
            && !matches!(
                chunk_type,
                Some(Self::HUGE_TYPE | Self::HUGE_CANCEL_TYPE | Self::PACKET_CLUSTER_TYPE)
            )
        {
            self.huge.clear();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn push_small_chunk(&mut self, packet: &[u8]) -> Result<Option<Vec<u8>>, String> {
        if packet.len() < 2 || packet[0] != Self::CORE_BYTE {
            return Err("invalid small chunk packet header".to_string());
        }

        match packet[1] {
            Self::SMALL_BODY_TYPE => {
                if packet.len() == 2 {
                    self.small.clear();
                    self.small_in_progress = false;
                    return Err("empty small chunk body".to_string());
                }
                self.small_in_progress = true;
                if let Err(err) = self.small.push(&packet[2..]) {
                    self.small_in_progress = false;
                    return Err(err);
                }
                Ok(None)
            }
            Self::SMALL_TAIL_TYPE => {
                if !self.small_in_progress || packet.len() == 2 {
                    self.small.clear();
                    self.small_in_progress = false;
                    return Err("orphan or empty small chunk tail".to_string());
                }
                if let Err(err) = self.small.push(&packet[2..]) {
                    self.small_in_progress = false;
                    return Err(err);
                }
                let body = self.small.take_complete();
                self.small_in_progress = false;
                Ok(Some(body))
            }
            other => Err(format!("unexpected small chunk packet type: {other:#04x}")),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn push_huge_chunk(&mut self, packet: &[u8]) -> Result<Option<Vec<u8>>, String> {
        if packet.len() <= 6 || packet[0] != Self::CORE_BYTE || packet[1] != Self::HUGE_TYPE {
            self.huge.clear();
            return Err("invalid huge chunk packet header".to_string());
        }

        let target_len = read_u32(packet, 2)? as usize;
        if target_len == 0 || target_len > crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES {
            self.huge.clear();
            return Err(format!("invalid huge chunk target length {target_len}"));
        }
        if let Some(existing) = self.huge.target_len {
            if existing != target_len {
                self.huge.clear();
                return Err("huge chunk target changed during assembly".to_string());
            }
        } else {
            self.huge.target_len = Some(target_len);
        }
        self.huge.push(&packet[6..])?;

        if self.huge.len == target_len {
            let body = self.huge.take_complete();
            Ok(Some(body))
        } else {
            Ok(None)
        }
    }

    #[allow(dead_code)]
    pub(crate) fn cancel_huge_chunk(&mut self) -> Vec<u8> {
        self.huge.clear();
        vec![Self::CORE_BYTE, Self::HUGE_CANCEL_ACK_TYPE]
    }

    #[allow(dead_code)]
    pub(crate) fn split_packet_cluster(packet: &[u8]) -> Result<Vec<&[u8]>, String> {
        if packet.len() < 2
            || packet[0] != Self::CORE_BYTE
            || packet[1] != Self::PACKET_CLUSTER_TYPE
        {
            return Err("invalid packet cluster header".to_string());
        }

        let mut packets = Vec::new();
        let mut offset = 2;
        while offset < packet.len() {
            let len = packet[offset] as usize;
            offset += 1;

            if offset + len > packet.len() {
                return Err("packet cluster entry extends past end of packet".to_string());
            }

            packets.push(&packet[offset..offset + len]);
            offset += len;
        }

        Ok(packets)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReliableSentPacket {
    pub(crate) id: u32,
    pub(crate) payload: Vec<u8>,
    pub(crate) last_sent_tick: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReliableSequencer {
    pub(crate) next_outbound_id: u32,
    pub(crate) next_process_id: u32,
    pub(crate) sent: Vec<ReliableSentPacket>,
    pub(crate) queued: BTreeMap<u32, Vec<u8>>,
    pub(crate) outbound_acks: Vec<u32>,
    /// Reliable payloads discarded because their id was already behind the in-order cursor. The packet is acknowledged
    /// again - duplicate ACKs are intentional - but the payload is dropped, so a peer whose id space restarted below this
    /// cursor sees its data vanish without any error surface. Counted so a stalled zone-file transfer is diagnosable.
    pub(crate) duplicate_drops: u32,
    /// New out-of-order payloads discarded because the reorder queue was full. Deliberately not acknowledged, so the peer
    /// must retry them; counting separates that backpressure from the duplicate case above.
    pub(crate) queue_full_drops: u32,
    /// Acknowledgement ids this endpoint has actually handed to the transport, and the highest id among them. A stalled
    /// peer whose window never frees can now be told apart: ids received but never emitted is a client-side halt, ids
    /// emitted but unseen by the peer is transport or ingress loss.
    pub(crate) acks_emitted: u32,
    pub(crate) acks_emitted_max_id: u32,
    /// Acknowledgement packets whose write to the transport failed. A channel whose send buffer is full throws, and an
    /// acknowledgement is one-shot, so discarding it on a failed write silently reproduces the exact stall the
    /// acknowledgement exists to prevent: the peer keeps an in-flight window it can never retire. Failed batches are
    /// retried from the next frame instead, and the counter makes the difference visible in the retained evidence.
    pub(crate) ack_send_failures: u32,
    pub(crate) pending_ack_retries: Vec<Vec<u8>>,
}

const RELIABLE_ID_HALF_WINDOW: u32 = 1 << 31;

#[allow(dead_code)]
impl ReliableSequencer {
    pub(crate) const CORE_BYTE: u8 = 0x00;
    pub(crate) const RELIABLE_MESSAGE_TYPE: u8 = 0x03;
    pub(crate) const RELIABLE_ACK_TYPE: u8 = 0x04;
    pub(crate) const PACKET_CLUSTER_TYPE: u8 = 0x0e;
    pub(crate) const RELIABLE_HEADER_SIZE: usize = 6;
    pub(crate) const ACK_SIZE: usize = 6;
    pub(crate) const CLUSTER_ACK_SIZE: usize = Self::ACK_SIZE + 1;
    pub(crate) const CLUSTER_TYPE_SIZE: usize = 2;
    pub(crate) const MAX_PACKET_SIZE: usize = 520;
    pub(crate) const MAX_RELIABLE_PAYLOAD_SIZE: usize =
        Self::MAX_PACKET_SIZE - Self::RELIABLE_HEADER_SIZE;
    pub(crate) const MAX_SMALL_CHUNK_PAYLOAD_SIZE: usize = Self::MAX_RELIABLE_PAYLOAD_SIZE - 2;
    pub(crate) const MAX_OUTBOUND_ACKS: usize = 256;
    pub(crate) const MAX_QUEUED_RELIABLES: usize = 256;
    pub(crate) const MAX_SENT_RELIABLES: usize = 256;
    pub(crate) const RESEND_DELAY_TICKS: i32 = 300;

    pub(crate) fn send_reliable(&mut self, payload: &[u8]) -> Vec<u8> {
        // Compatibility helper for tests/legacy callers. Production browser
        // paths pass a server-owned tick through ClientProtocolSession.
        self.send_reliable_at_tick(payload, 0)
    }

    pub(crate) fn send_reliable_at_tick(&mut self, payload: &[u8], timestamp: u32) -> Vec<u8> {
        let id = self.next_outbound_id;
        self.next_outbound_id = self.next_outbound_id.wrapping_add(1);
        if self.sent.len() < Self::MAX_SENT_RELIABLES {
            self.sent.push(ReliableSentPacket {
                id,
                payload: payload.to_vec(),
                last_sent_tick: timestamp,
            });
        }
        Self::encode_reliable_message(id, payload)
    }

    /// Reserve, retain, and encode one application-reliable packet as one
    /// atomic operation. Lifecycle barriers use this instead of the
    /// compatibility sender above: a full sent table must preserve the
    /// caller's intent without allocating or emitting an untracked id.
    pub(crate) fn try_send_tracked_reliable_at_tick(
        &mut self,
        payload: &[u8],
        timestamp: u32,
    ) -> Option<(u32, Vec<u8>)> {
        if self.sent.len() >= Self::MAX_SENT_RELIABLES {
            return None;
        }
        let id = self.next_outbound_id;
        self.next_outbound_id = self.next_outbound_id.wrapping_add(1);
        self.sent.push(ReliableSentPacket {
            id,
            payload: payload.to_vec(),
            last_sent_tick: timestamp,
        });
        Some((id, Self::encode_reliable_message(id, payload)))
    }

    /// Undo a reservation whose transport write failed before any bytes were
    /// emitted. Only the newest tracked packet can be rolled back; this keeps
    /// reliable ids contiguous without disturbing earlier in-flight work.
    pub(crate) fn rollback_tracked_reliable(&mut self, id: u32) -> bool {
        if self.next_outbound_id != id.wrapping_add(1)
            || self.sent.last().is_none_or(|packet| packet.id != id)
        {
            return false;
        }
        self.sent.pop();
        self.next_outbound_id = id;
        true
    }

    pub(crate) fn encode_timed_out_reliable_at_tick(&mut self, timestamp: u32) -> Vec<Vec<u8>> {
        self.sent
            .iter_mut()
            .filter_map(|packet| {
                if continuum_tick_diff(timestamp, packet.last_sent_tick) >= Self::RESEND_DELAY_TICKS
                {
                    packet.last_sent_tick = timestamp;
                    Some(Self::encode_reliable_message(packet.id, &packet.payload))
                } else {
                    None
                }
            })
            .collect()
    }

    pub(crate) fn encode_reliable_message(id: u32, payload: &[u8]) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::RELIABLE_HEADER_SIZE + payload.len());
        push_u8(&mut packet, Self::CORE_BYTE);
        push_u8(&mut packet, Self::RELIABLE_MESSAGE_TYPE);
        push_u32(&mut packet, id);
        packet.extend_from_slice(payload);
        packet
    }

    pub(crate) fn send_reliable_chunked(&mut self, payload: &[u8]) -> Vec<Vec<u8>> {
        // Compatibility helper for tests/legacy callers. Production browser
        // paths pass a server-owned tick through ClientProtocolSession.
        self.send_reliable_chunked_at_tick(payload, 0)
    }

    pub(crate) fn send_reliable_chunked_at_tick(
        &mut self,
        payload: &[u8],
        timestamp: u32,
    ) -> Vec<Vec<u8>> {
        if payload.len() <= Self::MAX_RELIABLE_PAYLOAD_SIZE {
            return vec![self.send_reliable_at_tick(payload, timestamp)];
        }

        let chunks: Vec<&[u8]> = payload.chunks(Self::MAX_SMALL_CHUNK_PAYLOAD_SIZE).collect();
        chunks
            .iter()
            .enumerate()
            .map(|(index, chunk)| {
                let mut payload = Vec::with_capacity(chunk.len() + 2);
                push_u8(&mut payload, PacketChunker::CORE_BYTE);
                push_u8(
                    &mut payload,
                    if index + 1 == chunks.len() {
                        PacketChunker::SMALL_TAIL_TYPE
                    } else {
                        PacketChunker::SMALL_BODY_TYPE
                    },
                );
                payload.extend_from_slice(chunk);
                self.send_reliable_at_tick(&payload, timestamp)
            })
            .collect()
    }

    pub(crate) fn receive_reliable(&mut self, packet: &[u8]) -> Result<(), String> {
        if packet.len() < Self::RELIABLE_HEADER_SIZE
            || packet[0] != Self::CORE_BYTE
            || packet[1] != Self::RELIABLE_MESSAGE_TYPE
        {
            return Err("invalid reliable message packet".to_string());
        }

        let id = read_u32(packet, 2)?;
        if reliable_id_before(id, self.next_process_id) || self.queued.contains_key(&id) {
            // Already processed and already retained packets are safe to ACK
            // again. Duplicate ACKs let a peer retire a resend whose earlier
            // ACK was lost without dispatching the payload twice.
            if reliable_id_before(id, self.next_process_id) {
                self.duplicate_drops = self.duplicate_drops.saturating_add(1);
            }
            self.outbound_acks.push(id);
            return Ok(());
        }

        if id == self.next_process_id || self.queued.len() < Self::MAX_QUEUED_RELIABLES {
            self.queued
                .insert(id, packet[Self::RELIABLE_HEADER_SIZE..].to_vec());
            self.outbound_acks.push(id);
        } else {
            self.queue_full_drops = self.queue_full_drops.saturating_add(1);
        }
        // A new out-of-order packet that cannot be retained must not be
        // ACKed. The peer will retry it after the current gap drains. ACKing
        // an unretained packet creates a permanent ordered-stream hole: the
        // peer retires its only copy while every newer payload remains stuck
        // behind the missing id.
        Ok(())
    }

    pub(crate) fn drain_ordered(&mut self) -> Vec<Vec<u8>> {
        self.drain_ordered_with_ids()
            .into_iter()
            .map(|(_, payload)| payload)
            .collect()
    }

    /// Drain the contiguous prefix while retaining each transport identity.
    /// Application receipts may be dispatched early across an unrelated gap;
    /// their reliable ids are required when the prefix later retires so the
    /// same wrapper cannot emit a second application event.
    pub(crate) fn drain_ordered_with_ids(&mut self) -> Vec<(u32, Vec<u8>)> {
        let mut packets = Vec::new();
        while let Some(payload) = self.queued.remove(&self.next_process_id) {
            packets.push((self.next_process_id, payload));
            self.next_process_id = self.next_process_id.wrapping_add(1);
        }
        packets
    }

    pub(crate) fn receive_ack(&mut self, packet: &[u8]) -> Result<bool, String> {
        if packet.len() != Self::ACK_SIZE
            || packet[0] != Self::CORE_BYTE
            || packet[1] != Self::RELIABLE_ACK_TYPE
        {
            return Err("invalid reliable ack packet".to_string());
        }

        let id = read_u32(packet, 2)?;
        let before = self.sent.len();
        self.sent.retain(|packet| packet.id != id);
        Ok(self.sent.len() != before)
    }

    /// True once no tracked outbound reliable at or before `inclusive_id`
    /// remains unacknowledged. The half-range comparison is wrap-safe; the
    /// sequencer's bounded 256-packet window is far smaller than the
    /// ambiguous half of the u32 sequence space.
    pub(crate) fn outbound_prefix_acked_through(&self, inclusive_id: u32) -> bool {
        self.sent
            .iter()
            .all(|packet| packet.id != inclusive_id && !reliable_id_before(packet.id, inclusive_id))
    }

    pub(crate) fn encode_outbound_acks(&mut self) -> Vec<Vec<u8>> {
        // Counted at the point of encoding because that is where an acknowledgement stops being this endpoint's
        // responsibility and becomes something the transport must deliver.
        self.acks_emitted = self
            .acks_emitted
            .saturating_add(self.outbound_acks.len() as u32);
        for pending in &self.outbound_acks {
            self.acks_emitted_max_id = self.acks_emitted_max_id.max(*pending);
        }

        let mut packets = Vec::new();
        let max_ack_per_cluster =
            (Self::MAX_PACKET_SIZE - Self::CLUSTER_TYPE_SIZE) / Self::CLUSTER_ACK_SIZE;

        for chunk in self.outbound_acks.chunks(max_ack_per_cluster) {
            if chunk.len() == 1 {
                packets.push(Self::encode_ack(chunk[0]));
                continue;
            }

            let mut packet =
                Vec::with_capacity(Self::CLUSTER_TYPE_SIZE + Self::CLUSTER_ACK_SIZE * chunk.len());
            push_u8(&mut packet, Self::CORE_BYTE);
            push_u8(&mut packet, Self::PACKET_CLUSTER_TYPE);
            for id in chunk {
                push_u8(&mut packet, Self::ACK_SIZE as u8);
                packet.extend_from_slice(&Self::encode_ack(*id));
            }
            packets.push(packet);
        }

        self.outbound_acks.clear();
        packets
    }

    pub(crate) fn encode_ack(id: u32) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::ACK_SIZE);
        push_u8(&mut packet, Self::CORE_BYTE);
        push_u8(&mut packet, Self::RELIABLE_ACK_TYPE);
        push_u32(&mut packet, id);
        packet
    }
}

fn reliable_id_before(id: u32, reference: u32) -> bool {
    id != reference && reference.wrapping_sub(id) < RELIABLE_ID_HALF_WINDOW
}
