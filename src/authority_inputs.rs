#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::ops::Deref;

use crate::authority_identity::PlayerRuntimeIdentity;
use crate::continuum_position::parse_client_position_snapshot;
use crate::continuum_weapon::continuum_weapon_type;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueuedPositionInput {
    pub(crate) identity: PlayerRuntimeIdentity,
    packet: Vec<u8>,
    deferred_recovery: bool,
    lifecycle_baseline_delivery_id: Option<u32>,
}

impl Deref for QueuedPositionInput {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.packet
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PositionInputQueues {
    max_per_player: usize,
    queues: BTreeMap<u16, VecDeque<QueuedPositionInput>>,
    capacity_rejections: u64,
    /// One exact weapon edge that authority has already inspected but could
    /// not admit because an outbound recipient queue was full. It is outside
    /// the ordinary bounded/coalescing queue, so later weapon mirrors cannot
    /// evict it. The scheduler always drains it before subsequent inputs.
    protected_deferred_weapons: BTreeMap<u16, QueuedPositionInput>,
}

impl Default for PositionInputQueues {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_POSITION_INPUTS_PER_PLAYER)
    }
}

impl PositionInputQueues {
    pub(crate) fn new(max_per_player: usize) -> Self {
        Self {
            max_per_player,
            queues: BTreeMap::new(),
            capacity_rejections: 0,
            protected_deferred_weapons: BTreeMap::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.protected_deferred_weapons.is_empty() && self.queues.values().all(VecDeque::is_empty)
    }

    pub(crate) fn clear(&mut self) {
        self.queues.clear();
        self.protected_deferred_weapons.clear();
    }

    pub(crate) fn clear_player(&mut self, player_id: u16) {
        self.queues.remove(&player_id);
        self.protected_deferred_weapons.remove(&player_id);
    }

    pub(crate) fn queued_count_for_player(&self, player_id: u16) -> usize {
        self.queues.get(&player_id).map_or(0, VecDeque::len)
            + usize::from(self.protected_deferred_weapons.contains_key(&player_id))
    }

    pub(crate) fn capacity_rejections(&self) -> u64 {
        self.capacity_rejections
    }

    pub(crate) fn queued_for_player(
        &self,
        player_id: u16,
    ) -> Option<&VecDeque<QueuedPositionInput>> {
        self.queues.get(&player_id)
    }

    /// Sequence of the protected lifecycle boundary already queued for this
    /// exact runtime. A realtime suffix may overtake server-tick admission of
    /// that reliable baseline, but traffic from the ended life must not use a
    /// baseline belonging to another session/life as its authority fence.
    pub(crate) fn lifecycle_baseline_sequence_for_runtime(
        &self,
        identity: PlayerRuntimeIdentity,
    ) -> Option<u16> {
        self.queues
            .get(&identity.player_id())?
            .iter()
            .find(|queued| {
                queued.identity == identity && queued.lifecycle_baseline_delivery_id.is_some()
            })
            .and_then(|queued| parse_client_position_snapshot(&queued.packet))
            .and_then(|snapshot| snapshot.client_sequence)
    }

    pub(crate) fn queue(&mut self, identity: PlayerRuntimeIdentity, packet: Vec<u8>) {
        self.queue_classified(identity, packet, None);
    }

    /// Queue the exact application-reliable position which opens a new
    /// lifecycle boundary. Unlike ordinary movement, this entry may never be
    /// coalesced away by a later realtime sample: application acceptance of
    /// this exact reliable delivery is what opens the browser's realtime
    /// barrier.
    pub(crate) fn queue_lifecycle_baseline(
        &mut self,
        identity: PlayerRuntimeIdentity,
        packet: Vec<u8>,
        reliable_delivery_id: u32,
    ) {
        self.queue_classified(identity, packet, Some(reliable_delivery_id));
    }

    fn queue_classified(
        &mut self,
        identity: PlayerRuntimeIdentity,
        packet: Vec<u8>,
        lifecycle_baseline_delivery_id: Option<u32>,
    ) {
        let queue = self.queues.entry(identity.player_id()).or_default();
        let movement_only = !position_input_requests_weapon(&packet);
        if !movement_only
            && lifecycle_baseline_delivery_id.is_none()
            && position_input_sequence(&packet).is_some_and(|sequence| {
                queue.iter().any(|queued| {
                    queued.identity == identity
                        && queued.lifecycle_baseline_delivery_id.is_none()
                        && position_input_requests_weapon(&queued.packet)
                        && position_input_sequence(&queued.packet) == Some(sequence)
                }) || self
                    .protected_deferred_weapons
                    .get(&identity.player_id())
                    .is_some_and(|queued| {
                        queued.identity == identity
                            && position_input_sequence(&queued.packet) == Some(sequence)
                    })
            })
        {
            // The same semantic weapon edge can arrive on more than one RTC
            // lane. Its life-bound input sequence is the action identity, so
            // retaining another copy only consumes bounded authority capacity
            // and can evict a genuinely later edge during a stalled tick.
            return;
        }
        if movement_only {
            // A deferred movement is a witness bridge, not an ordinary stale
            // movement sample. Preserve it while coalescing only the latest
            // newly arrived realtime movement.
            queue.retain(|queued| {
                queued.deferred_recovery
                    || queued.lifecycle_baseline_delivery_id.is_some()
                    || position_input_requests_weapon(&queued.packet)
            });
        }
        queue.push_back(QueuedPositionInput {
            identity,
            packet,
            deferred_recovery: false,
            lifecycle_baseline_delivery_id,
        });
        while queue.len() > self.max_per_player {
            let victim = queue
                .iter()
                .position(|queued| {
                    queued.lifecycle_baseline_delivery_id.is_none()
                        && !queued.deferred_recovery
                        && !position_input_requests_weapon(&queued.packet)
                })
                .or_else(|| {
                    queue.iter().position(|queued| {
                        queued.lifecycle_baseline_delivery_id.is_none()
                            && queued.deferred_recovery
                            && !position_input_requests_weapon(&queued.packet)
                    })
                })
                .or_else(|| {
                    queue
                        .iter()
                        .position(|queued| queued.lifecycle_baseline_delivery_id.is_none())
                });
            let Some(victim) = victim else {
                // A lifecycle baseline is a correctness fence, not optional
                // realtime work. The configured cap is soft for the (normally
                // singular) protected entry rather than allowing its loss.
                break;
            };
            queue.remove(victim);
            self.capacity_rejections = self.capacity_rejections.saturating_add(1);
        }
    }

    /// Put a movement packet deferred for missing input history back after a
    /// drain/process pass. A realtime packet may have arrived while the state
    /// lock was released; ordinary `queue` coalescing would let this older
    /// packet erase that newer movement. Retain both in wrap-safe sequence
    /// order: the older witness bridge may be required before the newer
    /// movement can validate. Weapon packets remain in the same queue and are
    /// never selected as the capacity-pressure victim.
    pub(crate) fn requeue_deferred(&mut self, identity: PlayerRuntimeIdentity, packet: Vec<u8>) {
        self.requeue_deferred_classified(identity, packet, None);
    }

    pub(crate) fn requeue_deferred_lifecycle_baseline(
        &mut self,
        identity: PlayerRuntimeIdentity,
        packet: Vec<u8>,
        reliable_delivery_id: u32,
    ) {
        self.requeue_deferred_classified(identity, packet, Some(reliable_delivery_id));
    }

    fn requeue_deferred_classified(
        &mut self,
        identity: PlayerRuntimeIdentity,
        packet: Vec<u8>,
        lifecycle_baseline_delivery_id: Option<u32>,
    ) {
        if position_input_requests_weapon(&packet) {
            let player_id = identity.player_id();
            let deferred_sequence = position_input_sequence(&packet);
            if self
                .protected_deferred_weapons
                .get(&player_id)
                .is_some_and(|queued| {
                    queued.identity == identity
                        && position_input_sequence(&queued.packet) == deferred_sequence
                })
            {
                return;
            }
            if self.protected_deferred_weapons.contains_key(&player_id) {
                // A later weapon is bounded ordinary work until the first
                // authoritative recovery edge admits or lifecycle cleanup
                // cancels it. It cannot replace the protected edge.
                self.queue_classified(identity, packet, lifecycle_baseline_delivery_id);
                return;
            }
            self.protected_deferred_weapons.insert(
                player_id,
                QueuedPositionInput {
                    identity,
                    packet,
                    deferred_recovery: true,
                    lifecycle_baseline_delivery_id,
                },
            );
            return;
        }
        let queue = self.queues.entry(identity.player_id()).or_default();
        let deferred_sequence = position_input_sequence(&packet);
        if deferred_sequence.is_some_and(|deferred| {
            queue.iter().any(|queued| {
                queued.identity == identity
                    && position_input_sequence(&queued.packet) == Some(deferred)
            })
        }) {
            return;
        }

        let insertion_index = deferred_sequence
            .and_then(|deferred| {
                queue.iter().position(|queued| {
                    queued.identity == identity
                        && position_input_sequence(&queued.packet).is_some_and(|candidate| {
                            candidate != deferred
                                && candidate.wrapping_sub(deferred) < (1_u16 << 15)
                        })
                })
            })
            .unwrap_or(queue.len());
        queue.insert(
            insertion_index,
            QueuedPositionInput {
                identity,
                packet,
                deferred_recovery: true,
                lifecycle_baseline_delivery_id,
            },
        );
        while queue.len() > self.max_per_player {
            let Some(oldest_movement) = queue.iter().position(|queued| {
                queued.lifecycle_baseline_delivery_id.is_none()
                    && !position_input_requests_weapon(&queued.packet)
            }) else {
                let Some(non_baseline) = queue
                    .iter()
                    .position(|queued| queued.lifecycle_baseline_delivery_id.is_none())
                else {
                    break;
                };
                queue.remove(non_baseline);
                self.capacity_rejections = self.capacity_rejections.saturating_add(1);
                continue;
            };
            queue.remove(oldest_movement);
            self.capacity_rejections = self.capacity_rejections.saturating_add(1);
        }
    }

    pub(crate) fn drain(&mut self) -> Vec<(PlayerRuntimeIdentity, Vec<u8>, bool, Option<u32>)> {
        let protected = std::mem::take(&mut self.protected_deferred_weapons);
        let pending = std::mem::take(&mut self.queues);
        let mut by_runtime = BTreeMap::<PlayerRuntimeIdentity, Vec<QueuedPositionInput>>::new();
        for queued in protected.into_values() {
            by_runtime.entry(queued.identity).or_default().push(queued);
        }
        for queued in pending.into_values().flatten() {
            by_runtime.entry(queued.identity).or_default().push(queued);
        }

        by_runtime
            .into_values()
            .flat_map(|mut packets| {
                order_sequenced_position_inputs(&mut packets);
                packets.into_iter().map(|queued| {
                    (
                        queued.identity,
                        queued.packet,
                        queued.deferred_recovery,
                        queued.lifecycle_baseline_delivery_id,
                    )
                })
            })
            .collect()
    }
}

/// RTC lanes are independently unordered. Several packets that reached the
/// same server tick must therefore be applied in their authenticated logical
/// input order, not callback arrival order. In particular, a movement-only
/// tail may already contain displacement from an immediately preceding bomb
/// recoil. Applying that tail before the retained weapon packet would replay
/// the gap without the recoil and falsely classify the honest raw-f32 state as
/// a canonical mismatch.
///
/// The queue remains bounded to 64 entries, far below the half-range used by
/// the wrapping sequence comparison. Client-authored compatibility timestamps
/// are deliberately ignored. Legacy packets without a logical sequence keep
/// their arrival order and continue through the existing legacy admission
/// path.
fn order_sequenced_position_inputs(packets: &mut [QueuedPositionInput]) {
    let Some(mut newest) = packets
        .iter()
        .find_map(|queued| position_input_sequence(&queued.packet))
    else {
        return;
    };
    if packets
        .iter()
        .any(|queued| position_input_sequence(&queued.packet).is_none())
    {
        return;
    }
    for sequence in packets
        .iter()
        .filter_map(|queued| position_input_sequence(&queued.packet))
    {
        let distance = sequence.wrapping_sub(newest);
        if distance != 0 && distance < (1 << 15) {
            newest = sequence;
        }
    }
    packets.sort_by_key(|queued| {
        let sequence = position_input_sequence(&queued.packet)
            .expect("all packets were proven to carry a logical sequence");
        let age = newest.wrapping_sub(sequence);
        let bounded_age = (age < (1 << 15)).then_some(age).unwrap_or(0);
        let boundary_rank = if queued.lifecycle_baseline_delivery_id.is_some() {
            0_u8
        } else if position_input_requests_weapon(&queued.packet) {
            1
        } else {
            2
        };
        (std::cmp::Reverse(bounded_age), boundary_rank)
    });
}

/// Match the server's wrap-safe input-witness history. Extreme can author a
/// 25 ms gun cadence, while browsers and RTC may legitimately batch more than
/// eight position/action witnesses across a delayed server tick. Keeping the
/// same bounded 64-sequence window prevents a valid edge from being discarded
/// before its corresponding canonical input witness can be evaluated.
pub(crate) const DEFAULT_MAX_POSITION_INPUTS_PER_PLAYER: usize = 64;

fn position_input_requests_weapon(packet: &[u8]) -> bool {
    parse_client_position_snapshot(packet)
        .is_some_and(|snapshot| continuum_weapon_type(snapshot.weapon) != 0)
}

fn position_input_sequence(packet: &[u8]) -> Option<u16> {
    parse_client_position_snapshot(packet).and_then(|snapshot| snapshot.client_sequence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority_identity::{LifeIdentity, PlayerSessionId, PlayerSessionIdentity};
    use crate::continuum_position::{
        CLIENT_POSITION_EXTRA_LEN, CLIENT_POSITION_TYPE, ClientPositionCanonicalState,
    };

    #[test]
    fn movement_only_inputs_coalesce_to_latest() {
        let mut queues = PositionInputQueues::new(8);
        for timestamp in 0_u32..10 {
            queues.queue(identity(7, 1), position_packet(timestamp, 0));
        }

        assert_eq!(queues.queued_count_for_player(7), 1);
        let drained = queues.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].0, identity(7, 1));
        assert_eq!(timestamp(&drained[0].1), 9);
        assert!(queues.is_empty());
    }

    #[test]
    fn weapon_inputs_are_preserved_while_later_movement_coalesces() {
        let mut queues = PositionInputQueues::new(8);
        queues.queue(identity(7, 1), position_packet(1, 0));
        queues.queue(identity(7, 1), position_packet(2, 2));
        queues.queue(identity(7, 1), position_packet(3, 0));
        queues.queue(identity(7, 1), position_packet(4, 0));

        let queued = queues
            .queued_for_player(7)
            .expect("player queue should exist");
        assert_eq!(queued.len(), 2);
        assert_eq!(timestamp(&queued[0]), 2);
        assert_eq!(weapon(&queued[0]), 2);
        assert_eq!(timestamp(&queued[1]), 4);
        assert_eq!(weapon(&queued[1]), 0);
    }

    #[test]
    fn cross_channel_weapon_duplicates_share_one_sequence_slot() {
        let mut queues = PositionInputQueues::new(8);
        let life = identity(7, 41);
        let edge = sequenced_position_packet(1234, 2);

        queues.queue(life, edge.clone());
        queues.queue(life, edge.clone());
        queues.queue(life, edge);

        let drained = queues.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(sequence(&drained[0].1), Some(1234));
        assert_eq!(queues.capacity_rejections(), 0);
    }

    #[test]
    fn default_queue_retains_the_complete_authority_witness_window() {
        let mut queues = PositionInputQueues::default();
        let life = identity(7, 41);

        for sequence in 0..DEFAULT_MAX_POSITION_INPUTS_PER_PLAYER as u16 {
            queues.queue(life, sequenced_position_packet(sequence, 2));
        }

        assert_eq!(
            queues.queued_count_for_player(life.player_id()),
            DEFAULT_MAX_POSITION_INPUTS_PER_PLAYER
        );
        assert_eq!(queues.capacity_rejections(), 0);
    }

    #[test]
    fn drain_preserves_arrival_order_instead_of_trusting_client_timestamps() {
        let mut queues = PositionInputQueues::new(8);
        queues.queue(identity(7, 1), position_packet(20, 0));
        queues.queue(identity(7, 1), position_packet(10, 2));

        let drained = queues.drain();
        let timestamps = drained
            .iter()
            .map(|(_, packet, _, _)| (timestamp(packet), weapon(packet)))
            .collect::<Vec<_>>();
        assert_eq!(timestamps, vec![(20, 0), (10, 2)]);
    }

    #[test]
    fn drain_orders_current_protocol_inputs_by_logical_sequence_not_lane_arrival() {
        let mut queues = PositionInputQueues::new(8);
        let life = identity(7, 41);

        // Models a movement-only N+1 packet overtaking a bomb-bearing N copy
        // from another unordered DataChannel before the same server tick.
        queues.queue(life, sequenced_position_packet(101, 0));
        queues.queue(life, sequenced_position_packet(100, 4));

        let drained = queues.drain();
        assert_eq!(sequence(&drained[0].1), Some(100));
        assert_eq!(weapon(&drained[0].1), 4);
        assert_eq!(sequence(&drained[1].1), Some(101));
        assert_eq!(weapon(&drained[1].1), 0);
    }

    #[test]
    fn drain_logical_order_and_same_sequence_priority_survive_wrap() {
        let mut queues = PositionInputQueues::new(8);
        let life = identity(7, 41);

        queues.queue(life, sequenced_position_packet(0, 0));
        queues.queue(life, sequenced_position_packet(u16::MAX, 4));
        queues.queue(life, sequenced_position_packet(0, 4));

        let drained = queues.drain();
        assert_eq!(sequence(&drained[0].1), Some(u16::MAX));
        assert_eq!(weapon(&drained[0].1), 4);
        assert_eq!(sequence(&drained[1].1), Some(0));
        assert_eq!(weapon(&drained[1].1), 4);
        assert_eq!(sequence(&drained[2].1), Some(0));
        assert_eq!(weapon(&drained[2].1), 0);
    }

    #[test]
    fn queue_respects_per_player_cap() {
        let mut queues = PositionInputQueues::new(3);
        for timestamp in 0_u32..5 {
            queues.queue(identity(7, 1), position_packet(timestamp, 2));
        }

        assert_eq!(
            queues
                .drain()
                .into_iter()
                .map(|(_, packet, _, _)| timestamp(&packet))
                .collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
    }

    #[test]
    fn queued_inputs_retain_the_life_that_enqueued_them() {
        let mut queues = PositionInputQueues::new(8);
        queues.queue(identity(7, 41), position_packet(1, 2));

        let drained = queues.drain();
        assert_eq!(drained[0].0, identity(7, 41));
    }

    #[test]
    fn deferred_requeue_never_overwrites_intervening_newer_movement() {
        let mut queues = PositionInputQueues::new(8);
        let life = identity(7, 41);
        queues.queue(life, sequenced_position_packet(100, 0));
        let mut drained = queues.drain();
        let (_, deferred, _, _) = drained.pop().expect("deferred movement");

        // Models the server-tick lock gap: a weapon and a newer movement
        // arrive after drain but before the old packet is requeued.
        queues.queue(life, sequenced_position_packet(101, 2));
        queues.queue(life, sequenced_position_packet(102, 0));
        queues.requeue_deferred(life, deferred);

        let queued = queues.queued_for_player(7).expect("player queue");
        assert_eq!(queued.len(), 3);
        assert_eq!(sequence(&queued[0]), Some(100));
        assert_eq!(weapon(&queued[0]), 0);
        assert_eq!(sequence(&queued[1]), Some(101));
        assert_eq!(weapon(&queued[1]), 2);
        assert_eq!(sequence(&queued[2]), Some(102));
        assert_eq!(weapon(&queued[2]), 0);
    }

    #[test]
    fn deferred_requeue_compares_movement_sequences_across_u16_wrap() {
        let mut queues = PositionInputQueues::new(8);
        let life = identity(7, 41);
        queues.queue(life, sequenced_position_packet(u16::MAX, 0));
        let (_, deferred, _, _) = queues.drain().pop().expect("deferred movement");

        queues.queue(life, sequenced_position_packet(0, 0));
        queues.requeue_deferred(life, deferred);

        let queued = queues.queued_for_player(7).expect("player queue");
        assert_eq!(queued.len(), 2);
        assert_eq!(sequence(&queued[0]), Some(u16::MAX));
        assert_eq!(sequence(&queued[1]), Some(0));
    }

    #[test]
    fn deferred_weapon_cross_channel_duplicates_collapse_and_lifecycle_clear_releases_it() {
        let mut queues = PositionInputQueues::new(8);
        let life = identity(7, 41);
        let exact = sequenced_position_packet(u16::MAX, 2);
        // Models the exact same browser packet arriving on game, event, and
        // reliable before a full recipient queue defers every copy.
        for _ in 0..3 {
            queues.queue(life, exact.clone());
        }
        let drained = queues.drain();
        assert_eq!(drained.len(), 1);
        for (_, packet, _, _) in drained {
            queues.requeue_deferred(life, packet);
        }
        queues.queue(life, sequenced_position_packet(0, 0));
        let drained = queues.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(sequence(&drained[0].1), Some(u16::MAX));
        assert_eq!(weapon(&drained[0].1), 2);
        assert_eq!(sequence(&drained[1].1), Some(0));

        queues.clear_player(7);
        assert!(
            queues.is_empty(),
            "life/session teardown releases authority"
        );
    }

    #[test]
    fn protected_deferred_weapon_survives_later_weapon_pressure_and_wrap() {
        let mut queues = PositionInputQueues::new(8);
        let life = identity(7, 41);
        let protected = sequenced_position_packet(u16::MAX - 3, 2);
        queues.requeue_deferred(life, protected.clone());

        for offset in 0..12_u16 {
            queues.queue(
                life,
                sequenced_position_packet((u16::MAX - 2).wrapping_add(offset), 3),
            );
        }
        assert_eq!(
            queues.queued_count_for_player(7),
            9,
            "the protected recovery slot is separate from eight bounded later inputs"
        );
        assert_eq!(
            queues.capacity_rejections(),
            4,
            "later unadmitted weapon pressure is explicit and observable"
        );
        let drained = queues.drain();
        assert_eq!(drained[0].0, life);
        assert_eq!(drained[0].1, protected);
        assert!(drained[0].2);
        assert_eq!(drained.len(), 9);

        let replacement = identity(7, 42);
        queues.requeue_deferred(life, sequenced_position_packet(7, 2));
        queues.clear_player(7);
        queues.requeue_deferred(replacement, sequenced_position_packet(0, 2));
        let drained = queues.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].0, replacement);
        assert_eq!(sequence(&drained[0].1), Some(0));
    }

    #[test]
    fn deferred_requeue_restores_old_movement_ahead_of_weapons_without_evicting_them() {
        let mut queues = PositionInputQueues::new(3);
        let life = identity(7, 41);
        queues.queue(life, sequenced_position_packet(100, 0));
        let (_, deferred, _, _) = queues.drain().pop().expect("deferred movement");

        queues.queue(life, sequenced_position_packet(101, 2));
        queues.queue(life, sequenced_position_packet(102, 3));
        queues.requeue_deferred(life, deferred);

        let queued = queues.queued_for_player(7).expect("player queue");
        assert_eq!(queued.len(), 3);
        assert_eq!(sequence(&queued[0]), Some(100));
        assert_eq!(weapon(&queued[1]), 2);
        assert_eq!(weapon(&queued[2]), 3);
    }

    #[test]
    fn deferred_requeue_capacity_pressure_keeps_weapons_and_newest_movement() {
        let mut queues = PositionInputQueues::new(2);
        let life = identity(7, 41);
        queues.queue(life, sequenced_position_packet(100, 0));
        let (_, deferred, _, _) = queues.drain().pop().expect("deferred movement");

        queues.queue(life, sequenced_position_packet(101, 2));
        queues.queue(life, sequenced_position_packet(102, 0));
        queues.requeue_deferred(life, deferred);

        let queued = queues.queued_for_player(7).expect("player queue");
        assert_eq!(queued.len(), 2);
        assert_eq!(sequence(&queued[0]), Some(101));
        assert_eq!(weapon(&queued[0]), 2);
        assert_eq!(sequence(&queued[1]), Some(102));
        assert_eq!(weapon(&queued[1]), 0);
    }

    #[test]
    fn lifecycle_baseline_survives_newer_movement_coalescing_across_u16_wrap() {
        let mut queues = PositionInputQueues::new(8);
        let life = identity(7, 41);
        queues.queue_lifecycle_baseline(life, sequenced_position_packet(u16::MAX, 0), 0xffff_fffe);
        queues.queue(life, sequenced_position_packet(0, 0));

        let drained = queues.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(sequence(&drained[0].1), Some(u16::MAX));
        assert_eq!(drained[0].3, Some(0xffff_fffe));
        assert_eq!(sequence(&drained[1].1), Some(0));
        assert_eq!(drained[1].3, None);
    }

    fn identity(player_id: u16, life_seq: u32) -> PlayerRuntimeIdentity {
        PlayerRuntimeIdentity::new(
            PlayerSessionIdentity::new(player_id, PlayerSessionId::new(u64::from(life_seq) + 99)),
            LifeIdentity::new(player_id, life_seq),
        )
    }

    fn position_packet(timestamp: u32, weapon_data: u16) -> Vec<u8> {
        let mut packet = vec![0; CLIENT_POSITION_EXTRA_LEN];
        packet[0] = CLIENT_POSITION_TYPE;
        packet[2..6].copy_from_slice(&timestamp.to_le_bytes());
        packet[8..10].copy_from_slice(&700_u16.to_le_bytes());
        packet[12..14].copy_from_slice(&500_u16.to_le_bytes());
        packet[18..20].copy_from_slice(&1_000_u16.to_le_bytes());
        packet[20..22].copy_from_slice(&weapon_data.to_le_bytes());
        ClientPositionCanonicalState::from_legacy_wire(0, 500, 700, 0, 0).encode(&mut packet);
        packet
    }

    fn sequenced_position_packet(sequence: u16, weapon_data: u16) -> Vec<u8> {
        let mut packet = position_packet(u32::from(sequence), weapon_data);
        packet[28..32].copy_from_slice(
            &crate::continuum_position::zubspace_position_sequence_items(sequence).to_le_bytes(),
        );
        packet
    }

    fn timestamp(packet: &[u8]) -> u32 {
        u32::from_le_bytes(packet[2..6].try_into().unwrap())
    }

    fn weapon(packet: &[u8]) -> u16 {
        u16::from_le_bytes(packet[20..22].try_into().unwrap())
    }

    fn sequence(packet: &[u8]) -> Option<u16> {
        position_input_sequence(packet)
    }
}
