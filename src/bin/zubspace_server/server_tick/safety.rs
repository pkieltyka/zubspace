use crate::authority_identity::PlayerSessionIdentity;
use crate::realtime_event::{RealtimeEventPayload, RealtimeOwnerWeaponCancellation};
use crate::snapshot::{RealtimeEventPressureDisposition, RealtimeEventSessionDelivery};
use crate::state::ServerState;
use crate::zone::{ZoneMap, world_position_in_safe_tile_for_map};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
enum CancellationCause {
    SafeEntry,
    Lifecycle,
}

pub(super) fn cancel_safe_owner_weapons(state: &mut ServerState, tick: u32, map: &ZoneMap) {
    let safe_sources = state
        .clients
        .iter()
        .filter_map(|(&id, client)| {
            if !client.joined || !client.alive || client.player.ship >= 8 {
                return None;
            }
            let position = client.last_position.as_deref()?;
            let x = u16::from_le_bytes(position.get(4..6)?.try_into().ok()?);
            let y = u16::from_le_bytes(position.get(15..17)?.try_into().ok()?);
            world_position_in_safe_tile_for_map(map, x, y).then_some((
                PlayerSessionIdentity::new(id, client.session_id),
                (client.life_seq, client.last_position_sequence),
            ))
        })
        .chain(state.bots.iter().filter_map(|bot| {
            (bot.alive()
                && world_position_in_safe_tile_for_map(
                    map,
                    bot.x.round().clamp(0.0, u16::MAX as f32) as u16,
                    bot.y.round().clamp(0.0, u16::MAX as f32) as u16,
                ))
            .then_some((bot.runtime_identity().session, (bot.life_seq, None)))
        }))
        .collect::<BTreeMap<_, _>>();
    cancel_for_sources(state, tick, &safe_sources, CancellationCause::SafeEntry);
    flush_pending_cancellations(state, tick);
}

/// Called only after a movement transaction has validated and committed a
/// safe witness. Do not consume event slots reserved by that transaction.
pub(crate) fn cancel_owner_weapons_for_admitted_safe_entry(
    state: &mut ServerState,
    tick: u32,
    owner: crate::authority_identity::PlayerRuntimeIdentity,
    sequence: Option<u16>,
) {
    if state.active_actor_runtime_identity(owner.player_id()) != Some(owner) {
        return;
    }
    let sources = [(owner.session, (owner.life.life_seq, sequence))]
        .into_iter()
        .collect();
    cancel_for_sources(state, tick, &sources, CancellationCause::SafeEntry);
}

/// The caller has committed a roster transition and advanced its life. Record
/// the old launch cutoff before physical roster cleanup, without reserving or
/// consuming notification slots owned by the lifecycle transaction.
pub(crate) fn cancel_owner_weapons_for_lifecycle_change(
    state: &mut ServerState,
    tick: u32,
    owner: crate::authority_identity::PlayerRuntimeIdentity,
) {
    if state.active_player_runtime_identity(owner.player_id()) != Some(owner) {
        return;
    }
    let sources = [(owner.session, (owner.life.life_seq, None))]
        .into_iter()
        .collect();
    cancel_for_sources(state, tick, &sources, CancellationCause::Lifecycle);
}

fn cancel_for_sources(
    state: &mut ServerState,
    tick: u32,
    safe_sources: &BTreeMap<PlayerSessionIdentity, (u32, Option<u16>)>,
    cause: CancellationCause,
) {
    if safe_sources.is_empty() {
        return;
    }
    let safe_sources = safe_sources
        .iter()
        .filter_map(|(&source, &boundary)| {
            let latest = state.latest_owner_weapon_groups.get(&source).copied()?;
            (latest
                > state
                    .owner_weapon_cancellation_watermarks
                    .get(&source)
                    .copied()
                    .unwrap_or(0))
            .then_some((source, boundary))
        })
        .collect::<BTreeMap<_, _>>();
    if safe_sources.is_empty() {
        return;
    }
    // A frozen earlier blast keeps its damage outcome, but its still-pending
    // fragment fan must also inherit the owner's safety cut.
    let mut affected_sources = safe_sources
        .keys()
        .map(|&source| (source, state.latest_owner_weapon_groups[&source]))
        .collect::<BTreeMap<_, _>>();
    for record in state.active_projectiles.iter().chain(
        state
            .active_projectiles
            .pending_wall_explosions()
            .map(|p| &p.parent),
    ) {
        let Some(source) = record.presentation_source else {
            continue;
        };
        if source.source_session_id == 0
            || source.source_player_id != record.owner_id
            || source.source_life_seq != record.owner_life_seq
            || source.shot_group_id != record.shot_group_id
        {
            continue;
        }
        let session = PlayerSessionIdentity::new(
            source.source_player_id,
            crate::authority_identity::PlayerSessionId::new(source.source_session_id),
        );
        if safe_sources.contains_key(&session) {
            let group = affected_sources.entry(session).or_default();
            *group = (*group).max(record.shot_group_id);
        }
    }
    let mut removed_by_source = BTreeMap::<PlayerSessionIdentity, usize>::new();
    state.active_projectiles.cancel_unfrozen_where(|record| {
        let Some(source) = record.presentation_source else {
            return false;
        };
        if source.source_session_id == 0
            || source.source_player_id != record.owner_id
            || source.source_life_seq != record.owner_life_seq
            || source.shot_group_id != record.shot_group_id
        {
            return false;
        }
        let session = PlayerSessionIdentity::new(
            source.source_player_id,
            crate::authority_identity::PlayerSessionId::new(source.source_session_id),
        );
        if !safe_sources.contains_key(&session) {
            return false;
        }
        *removed_by_source.entry(session).or_default() += 1;
        true
    });
    state.active_decoys.retain(|decoy| {
        if !safe_sources.contains_key(&decoy.owner.session) {
            return true;
        }
        *removed_by_source.entry(decoy.owner.session).or_default() += 1;
        let group = affected_sources.entry(decoy.owner.session).or_default();
        *group = (*group).max(state.next_projectile_group_id);
        false
    });
    for (source, through) in affected_sources {
        if state
            .owner_weapon_cancellation_watermarks
            .get(&source)
            .is_some_and(|old| *old >= through)
        {
            continue;
        }
        let count = removed_by_source.get(&source).copied().unwrap_or(0);
        let (life, sequence) = safe_sources[&source];
        let cancellation = RealtimeOwnerWeaponCancellation {
            source_player_id: source.player_id,
            source_session_id: source.session_id.get(),
            through_shot_group_id: through,
            owner_life_seq: life,
            owner_input_sequence: sequence,
        };
        state
            .pending_owner_weapon_cancellations
            .insert(source, cancellation);
        state
            .owner_weapon_cancellation_watermarks
            .insert(source, cancellation.through_shot_group_id);
        let (event_type, reason) = match cause {
            CancellationCause::SafeEntry => ("server_safe_weapon_cancellation", "safe_entry"),
            CancellationCause::Lifecycle => ("server_lifecycle_weapon_cancellation", "lifecycle"),
        };
        state.qa_traces.record_for_all(serde_json::json!({
            "type": event_type, "reason": reason, "serverTick": tick,
            "sourcePlayerId": source.player_id, "sourceSessionId": source.session_id.get().to_string(),
            "throughShotGroupId": cancellation.through_shot_group_id.to_string(), "removed": count,
        }));
        tracing::info!(
            player_id = source.player_id,
            session_id = source.session_id.get(),
            through_shot_group_id = cancellation.through_shot_group_id,
            removed = count,
            reason,
            "owner weapons cancelled"
        );
    }
}

fn flush_pending_cancellations(state: &mut ServerState, tick: u32) {
    if state.pending_owner_weapon_cancellations.is_empty()
        && state.owner_weapon_cancellation_watermarks.is_empty()
        && state.latest_owner_weapon_groups.is_empty()
    {
        return;
    }
    let live_sources = state
        .clients
        .iter()
        .filter_map(|(&id, client)| {
            client
                .joined
                .then_some(PlayerSessionIdentity::new(id, client.session_id))
        })
        .chain(state.bots.iter().map(|bot| bot.runtime_identity().session))
        .collect::<BTreeSet<_>>();
    state
        .pending_owner_weapon_cancellations
        .retain(|source, _| live_sources.contains(source));
    state
        .owner_weapon_cancellation_watermarks
        .retain(|source, _| live_sources.contains(source));
    state
        .latest_owner_weapon_groups
        .retain(|source, _| live_sources.contains(source));
    let recipients = state
        .clients
        .iter()
        .filter_map(|(&id, client)| {
            client
                .joined
                .then_some(PlayerSessionIdentity::new(id, client.session_id))
        })
        .collect::<Vec<_>>();
    let pending = state
        .pending_owner_weapon_cancellations
        .iter()
        .take(crate::authority_projectiles::MAX_PENDING_HIT_RETRIES_PER_TICK)
        .map(|(&source, &cancel)| (source, cancel))
        .collect::<Vec<_>>();
    for (source, cancellation) in pending {
        let Some(reservation) = crate::snapshot::reserve_realtime_event_session_deliveries(
            state,
            &[&recipients],
            RealtimeEventPressureDisposition::Deferred,
        ) else {
            break;
        };
        reservation.commit_sessions(
            state,
            vec![RealtimeEventSessionDelivery {
                recipients: recipients.clone(),
                server_tick: tick,
                payload: RealtimeEventPayload::OwnerWeaponsCancelled(cancellation),
            }],
        );
        state.pending_owner_weapon_cancellations.remove(&source);
    }
    state.prune_retired_projectile_owners();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority_identity::{PlayerRuntimeIdentity, PlayerSessionId};
    use crate::authority_projectiles::{PlayerHullSnapshot, ProjectileRecord};
    use std::sync::{Arc, Mutex, mpsc};

    fn fixture() -> (Arc<Mutex<ServerState>>, u16, u16) {
        let shared = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let owner = crate::arena::register_connection(&shared, tx.clone());
        let peer = crate::arena::register_connection(&shared, tx);
        crate::arena::join_arena(&shared, owner, 0);
        crate::arena::join_arena(&shared, peer, 0);
        (shared, owner, peer)
    }

    fn place(state: &mut ServerState, id: u16, safe: bool) {
        let map = state.zone_map();
        let index = map
            .tiles
            .iter()
            .position(|tile| *tile == if safe { 171 } else { 0 })
            .unwrap();
        let x = (index % crate::zone::MAP_TILES) as u16 * 16 + 8;
        let y = (index / crate::zone::MAP_TILES) as u16 * 16 + 8;
        let mut position = vec![0; 23];
        position[0] = 0x05;
        position[4..6].copy_from_slice(&x.to_le_bytes());
        position[8..10].copy_from_slice(&id.to_le_bytes());
        position[15..17].copy_from_slice(&y.to_le_bytes());
        state.set_client_position(id, position);
        state.clients.get_mut(&id).unwrap().last_position_sequence = Some(50);
    }

    fn projectile(
        state: &mut ServerState,
        owner: PlayerRuntimeIdentity,
        weapon: u16,
    ) -> ProjectileRecord {
        let group = state.allocate_projectile_group_id().unwrap();
        let frequency = state
            .clients
            .get(&owner.player_id())
            .map(|p| p.player.frequency)
            .or_else(|| state.bot(owner.player_id()).map(|p| p.player.frequency))
            .unwrap();
        ProjectileRecord {
            id: state.allocate_projectile_id(),
            shot_group_id: group,
            shot_index: 0,
            presentation_source: Some(crate::realtime_event::RealtimeWeaponEvent {
                source_player_id: owner.player_id(),
                source_session_id: owner.session.session_id.get(),
                source_life_seq: owner.life.life_seq,
                shot_group_id: group,
                x: 1000.0,
                y: 1000.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                orientation: 0.0,
                frequency,
                ship: 0,
                weapon_data: if weapon & 31 == 15 { 3 } else { weapon },
            }),
            owner_id: owner.player_id(),
            owner_life_seq: owner.life.life_seq,
            combat_timeline: None,
            weapon_data: weapon,
            x: 1000,
            y: 1000,
            timestamp: 100,
            created_tick: 100,
            velocity_x: 0,
            velocity_y: 0,
            expiry_tick: Some(500),
            pos_x: 1000.0,
            source_server_tick: None,
            precise_velocity: None,
            pos_y: 1000.0,
            bounces_remaining: 0,
            behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        }
    }

    fn decoy(owner: PlayerRuntimeIdentity) -> crate::state::ServerDecoy {
        crate::state::ServerDecoy {
            presentation_id: 0,
            presentation_source: None,
            source_input_sequence: None,
            owner,
            source_weapon_data: 6,
            frequency: 0,
            ship: 0,
            position_x: 1000.0,
            position_y: 1000.0,
            velocity_x: 1.0,
            velocity_y: 0.0,
            orientation: 0.0,
            created_tick: 100,
            last_step_tick: 100,
            expires_at_tick: 500,
        }
    }

    #[test]
    fn safe_owner_cancellation_covers_all_flight_and_decoys_across_lives_without_id_rebinding() {
        let (shared, owner_id, peer_id) = fixture();
        let mut state = shared.lock().unwrap();
        let owner = state.active_player_runtime_identity(owner_id).unwrap();
        for weapon in [1, 2, 3, 4, 7, 8, 15, 3 | (1 << 15), 4 | (1 << 15)] {
            let shot = projectile(&mut state, owner, weapon);
            state.record_projectile(shot);
        }
        state.allocate_projectile_group_id().unwrap(); // authenticated Decoy activation
        state.active_decoys.push(decoy(owner));
        let other_session = PlayerRuntimeIdentity::new(
            PlayerSessionIdentity::new(
                owner_id,
                PlayerSessionId::new(owner.session.session_id.get() + 100),
            ),
            owner.life,
        );
        let foreign = projectile(&mut state, other_session, 3);
        state.record_projectile(foreign);
        state.active_decoys.push(decoy(other_session));
        state
            .advance_client_life_after_damage_death(owner_id, 110)
            .unwrap();
        place(&mut state, owner_id, true);
        let map = state.zone_map();
        cancel_safe_owner_weapons(&mut state, 120, map);
        assert_eq!(state.active_projectiles.len(), 1);
        assert_eq!(state.active_projectiles.last().unwrap().id, foreign.id);
        assert_eq!(state.active_decoys.len(), 1);
        assert_eq!(state.active_decoys[0].owner, other_session);
        let cut = state.owner_weapon_cancellation_watermarks[&owner.session];
        for id in [owner_id, peer_id] {
            let queue = &state.clients[&id].pending_realtime_events.queue;
            assert_eq!(queue.len(), 1);
            assert!(
                matches!(queue[0].payload, RealtimeEventPayload::OwnerWeaponsCancelled(c)
                if c.source_session_id == owner.session.session_id.get() && c.through_shot_group_id == cut
                    && c.owner_life_seq != owner.life.life_seq && c.owner_input_sequence == Some(50))
            );
        }
        assert!(
            state
                .active_projectiles
                .pending_wall_explosions()
                .next()
                .is_none()
        );
        assert!(state.pending_owner_weapon_cancellations.is_empty());
        place(&mut state, owner_id, false);
        let current = state.active_player_runtime_identity(owner_id).unwrap();
        let future = projectile(&mut state, current, 2);
        assert!(future.shot_group_id > cut);
        state.record_projectile(future);
        // A delayed old-group fragment cannot resurrect after leaving safety.
        let mut late = future;
        late.shot_group_id = cut;
        late.presentation_source.as_mut().unwrap().shot_group_id = cut;
        late.weapon_data = 15;
        state.record_projectile(late);
        assert_eq!(state.active_projectiles.len(), 2);
        cancel_safe_owner_weapons(&mut state, 121, map);
        assert_eq!(state.active_projectiles.len(), 2);
    }

    #[test]
    fn safe_owner_cancellation_stops_flight_under_queue_pressure_and_coalesces_delivery() {
        let (shared, owner_id, peer_id) = fixture();
        let mut state = shared.lock().unwrap();
        let owner = state.active_player_runtime_identity(owner_id).unwrap();
        let shot = projectile(&mut state, owner, 3);
        state.record_projectile(shot);
        place(&mut state, owner_id, true);
        let recipient = state.active_player_runtime_identity(peer_id).unwrap();
        let filler = crate::snapshot::PendingRealtimeEventEnvelope {
            flight_transition: false,
            recipient,
            event_id: 1,
            server_tick: 100,
            payload: RealtimeEventPayload::Weapon(shot.presentation_source.unwrap()),
        };
        state
            .clients
            .get_mut(&peer_id)
            .unwrap()
            .pending_realtime_events
            .queue
            .extend(std::iter::repeat_n(
                filler,
                crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
            ));
        let map = state.zone_map();
        cancel_safe_owner_weapons(&mut state, 120, map);
        assert!(state.active_projectiles.is_empty());
        assert_eq!(state.pending_owner_weapon_cancellations.len(), 1);
        assert!(
            state.clients[&owner_id]
                .pending_realtime_events
                .queue
                .is_empty()
        );
        for tick in 121..130 {
            cancel_safe_owner_weapons(&mut state, tick, map);
        }
        assert_eq!(state.pending_owner_weapon_cancellations.len(), 1);
        state
            .clients
            .get_mut(&peer_id)
            .unwrap()
            .pending_realtime_events
            .queue
            .clear();
        cancel_safe_owner_weapons(&mut state, 130, map);
        assert!(state.pending_owner_weapon_cancellations.is_empty());
        assert_eq!(
            state.clients[&peer_id].pending_realtime_events.queue.len(),
            1
        );
        assert_eq!(
            state.clients[&owner_id].pending_realtime_events.queue.len(),
            1
        );
        state.remove_client(owner_id);
        assert!(
            !state
                .owner_weapon_cancellation_watermarks
                .contains_key(&owner.session)
        );
    }

    #[test]
    fn safe_owner_cancellation_at_admission_does_not_steal_reserved_slots_or_cancel_post_egress_fire()
     {
        let (shared, owner_id, peer_id) = fixture();
        let mut state = shared.lock().unwrap();
        let owner = state.active_player_runtime_identity(owner_id).unwrap();
        let shot = projectile(&mut state, owner, 3);
        state.record_projectile(shot);
        place(&mut state, owner_id, false); // accepted batch ended outside safety
        let recipient = state.active_player_session_identity(peer_id).unwrap();
        let reservation = crate::snapshot::reserve_realtime_event_session_deliveries(
            &mut state,
            &[&[recipient]],
            RealtimeEventPressureDisposition::Deferred,
        )
        .unwrap();
        cancel_owner_weapons_for_admitted_safe_entry(&mut state, 120, owner, Some(41));
        assert!(state.active_projectiles.is_empty());
        assert_eq!(state.pending_owner_weapon_cancellations.len(), 1);
        assert_eq!(
            state.pending_owner_weapon_cancellations[&owner.session].owner_input_sequence,
            Some(41)
        );
        assert!(
            state.clients[&owner_id]
                .pending_realtime_events
                .queue
                .is_empty()
        );
        assert!(
            state.clients[&peer_id]
                .pending_realtime_events
                .queue
                .is_empty()
        );
        let future = projectile(&mut state, owner, 2);
        state.record_projectile(future);
        reservation.commit_sessions(
            &mut state,
            vec![RealtimeEventSessionDelivery {
                recipients: vec![recipient],
                server_tick: 120,
                payload: RealtimeEventPayload::Weapon(future.presentation_source.unwrap()),
            }],
        );
        let map = state.zone_map();
        cancel_safe_owner_weapons(&mut state, 121, map);
        assert_eq!(state.active_projectiles.len(), 1);
        assert_eq!(state.active_projectiles.last().unwrap().id, future.id);
        assert!(state.pending_owner_weapon_cancellations.is_empty());
        assert_eq!(
            state.clients[&peer_id].pending_realtime_events.queue.len(),
            2
        );
        // An old/replaced runtime cannot invoke an admission cancellation.
        state.clients.get_mut(&owner_id).unwrap().session_id =
            PlayerSessionId::new(owner.session.session_id.get() + 100);
        cancel_owner_weapons_for_admitted_safe_entry(&mut state, 122, owner, Some(42));
        assert_eq!(state.active_projectiles.len(), 1);
    }

    fn hull(actor: PlayerRuntimeIdentity, frequency: u16) -> PlayerHullSnapshot {
        use crate::combat_timeline::*;
        let segment = MotionSegmentIdentity {
            actor,
            arena_stream_id: 1,
            discontinuity_generation: 1,
        };
        PlayerHullSnapshot {
            player_id: actor.player_id(),
            life_seq: actor.life.life_seq,
            x: 1000.0,
            y: 1000.0,
            frequency,
            radius_px: 14.0,
            motion_segment: Some(segment),
            motion_samples: vec![ActorMotionSampleV1 {
                segment,
                server_tick: 100,
                sample_time: PresentationSampleTime::from_tick(100),
                x_q8: 1000 * 256,
                y_q8: 1000 * 256,
                velocity_x_q8: 0,
                velocity_y_q8: 0,
                orientation_u16: 0,
                protocol_direction: 0,
                hull_radius_q8: 14 * 256,
                safe_or_attached: 0,
                source_kind: ActorMotionSourceKind::AcceptedHumanWitness,
            }],
            motion_source_kind: Some(ActorMotionSourceKind::AcceptedHumanWitness),
            lag_compensation_ticks: 0,
            life_started_tick: Some(0),
        }
    }

    #[test]
    fn safe_owner_cancellation_preserves_outgoing_and_incoming_already_frozen_contacts() {
        let (shared, owner_id, peer_id) = fixture();
        let mut state = shared.lock().unwrap();
        let owner = state.active_player_runtime_identity(owner_id).unwrap();
        let peer = state.active_player_runtime_identity(peer_id).unwrap();
        let outgoing = projectile(&mut state, owner, 3);
        let incoming = projectile(&mut state, peer, 1);
        state.record_projectile(outgoing);
        state.record_projectile(incoming);
        let hulls = [
            hull(owner, state.clients[&owner_id].player.frequency),
            hull(peer, state.clients[&peer_id].player.frequency),
        ];
        state.active_projectiles.step(100, |_, _| false, &[]);
        assert_eq!(
            state
                .active_projectiles
                .step_with_proximity_distance_transactional(101, |_, _| false, &hulls, 3.0, 0)
                .len(),
            2
        );
        let before = state
            .active_projectiles
            .collision_witness(outgoing.id)
            .unwrap();
        place(&mut state, owner_id, true);
        let map = state.zone_map();
        cancel_safe_owner_weapons(&mut state, 102, map);
        assert!(state.active_projectiles.contains_pending_hit(outgoing.id));
        assert!(state.active_projectiles.contains_pending_hit(incoming.id));
        assert_eq!(
            state.active_projectiles.collision_witness(outgoing.id),
            Some(before)
        );
        assert!(
            state
                .owner_weapon_cancellation_watermarks
                .contains_key(&owner.session)
        );
    }

    #[test]
    fn committed_roster_change_cancels_exact_session_launches_under_pressure() {
        for ship in [1, 8] {
            let (shared, owner_id, peer_id) = fixture();
            let (owner, old_shot, foreign, through) = {
                let mut state = shared.lock().unwrap();
                let owner = state.active_player_runtime_identity(owner_id).unwrap();
                let peer = state.active_player_runtime_identity(peer_id).unwrap();
                let own = projectile(&mut state, owner, 3);
                state.record_projectile(own);
                let foreign = projectile(&mut state, peer, 2);
                state.record_projectile(foreign);
                let launch = projectile(&mut state, owner, 6);
                state.note_owner_weapon_group(launch.presentation_source.unwrap());
                state.active_decoys.push(decoy(owner));
                state
                    .advance_client_life_after_damage_death(owner_id, 110)
                    .unwrap();
                let client = state.clients.get_mut(&owner_id).unwrap();
                client.server_energy = client.server_max_energy - 1;
                (owner, own, foreign, launch.shot_group_id)
            };
            let request = [crate::arena::SHIP_CHANGE_REQUEST, ship, 51, 0];
            crate::arena::update_ship_at_tick(&shared, owner_id, &request, 120);
            {
                let mut state = shared.lock().unwrap();
                assert_eq!(state.clients[&owner_id].player.ship, 0);
                assert_eq!(state.active_projectiles.len(), 2);
                assert_eq!(state.active_decoys.len(), 1);
                assert!(state.pending_owner_weapon_cancellations.is_empty());
                let client = state.clients.get_mut(&owner_id).unwrap();
                client.server_energy = client.server_max_energy;
                let peer = state.active_player_runtime_identity(peer_id).unwrap();
                let envelope = crate::snapshot::PendingRealtimeEventEnvelope {
                    flight_transition: false,
                    recipient: peer,
                    event_id: 50,
                    server_tick: 120,
                    payload: RealtimeEventPayload::Weapon(foreign.presentation_source.unwrap()),
                };
                state
                    .clients
                    .get_mut(&peer_id)
                    .unwrap()
                    .pending_realtime_events
                    .queue
                    .extend(std::iter::repeat_n(
                        envelope,
                        crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
                    ));
            }
            crate::arena::update_ship_at_tick(&shared, owner_id, &request, 130);
            {
                let mut state = shared.lock().unwrap();
                assert_eq!(state.clients[&owner_id].player.ship, ship);
                assert_eq!(state.active_projectiles.len(), 1);
                assert_eq!(state.active_projectiles.last().unwrap().id, foreign.id);
                assert!(state.active_decoys.is_empty());
                let cut = state.pending_owner_weapon_cancellations[&owner.session];
                assert_eq!(cut.through_shot_group_id, through);
                assert_eq!(cut.source_session_id, owner.session.session_id.get());
                assert_eq!(cut.owner_life_seq, state.clients[&owner_id].life_seq);
                let map = state.zone_map();
                cancel_safe_owner_weapons(&mut state, 131, map);
                assert_eq!(state.pending_owner_weapon_cancellations.len(), 1);
                assert_eq!(
                    state.clients[&peer_id].pending_realtime_events.queue.len(),
                    crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY
                );
                state
                    .clients
                    .get_mut(&peer_id)
                    .unwrap()
                    .pending_realtime_events
                    .queue
                    .clear();
                cancel_safe_owner_weapons(&mut state, 132, map);
                assert!(state.pending_owner_weapon_cancellations.is_empty());
                assert!(
                    matches!(state.clients[&peer_id].pending_realtime_events.queue.back().unwrap().payload,
                    RealtimeEventPayload::OwnerWeaponsCancelled(c) if c == cut)
                );
                state.record_projectile(old_shot);
                assert_eq!(
                    state.active_projectiles.len(),
                    1,
                    "late old launch must stay cancelled"
                );
            }
            if ship == 8 {
                crate::arena::update_ship_at_tick(
                    &shared,
                    owner_id,
                    &[crate::arena::SHIP_CHANGE_REQUEST, 0, 52, 0],
                    140,
                );
            }
            let mut state = shared.lock().unwrap();
            let current = state.active_player_runtime_identity(owner_id).unwrap();
            let future = projectile(&mut state, current, 1);
            assert!(future.shot_group_id > through);
            state.record_projectile(future);
            cancel_owner_weapons_for_lifecycle_change(&mut state, 150, owner);
            assert_eq!(state.active_projectiles.len(), 2);
            assert!(state.pending_owner_weapon_cancellations.is_empty());
        }
    }

    #[test]
    fn safe_owner_cancellation_tracks_decoy_only_and_expired_launches() {
        for with_decoy in [false, true] {
            let (shared, owner_id, peer_id) = fixture();
            let mut state = shared.lock().unwrap();
            let owner = state.active_player_runtime_identity(owner_id).unwrap();
            // The accepted launch may have no registry record: Decoys use a
            // separate store, and an observer's delayed launch can outlive flight.
            let shot = projectile(&mut state, owner, if with_decoy { 6 } else { 1 });
            state.note_owner_weapon_group(shot.presentation_source.unwrap());
            if with_decoy {
                state.active_decoys.push(decoy(owner));
            }
            place(&mut state, owner_id, true);
            let map = state.zone_map();
            cancel_safe_owner_weapons(&mut state, 120, map);
            assert!(state.active_decoys.is_empty());
            let cut = state.owner_weapon_cancellation_watermarks[&owner.session];
            assert!(cut >= shot.shot_group_id);
            assert_eq!(
                state.clients[&peer_id].pending_realtime_events.queue.len(),
                1
            );
            cancel_safe_owner_weapons(&mut state, 121, map);
            assert_eq!(
                state.clients[&peer_id].pending_realtime_events.queue.len(),
                1
            );
            place(&mut state, owner_id, false);
            state.record_projectile(shot); // delayed old-group record stays cancelled
            assert!(state.active_projectiles.is_empty());
            let future = projectile(&mut state, owner, 1);
            state.record_projectile(future);
            assert_eq!(state.active_projectiles.len(), 1);
        }
    }

    #[test]
    fn safe_owner_cancellation_also_covers_retired_bot_source_lives() {
        let (shared, _owner, peer) = fixture();
        let mut state = shared.lock().unwrap();
        state.config.bot_count = 1;
        state.initialize_configured_bots();
        let owner = state.bots[0].runtime_identity();
        let shot = projectile(&mut state, owner, 4);
        state.record_projectile(shot);
        state.active_decoys.push(decoy(owner));
        let map = state.zone_map();
        let index = map.tiles.iter().position(|tile| *tile == 171).unwrap();
        state.bots[0].x = (index % crate::zone::MAP_TILES) as f32 * 16.0 + 8.0;
        state.bots[0].y = (index / crate::zone::MAP_TILES) as f32 * 16.0 + 8.0;
        state.bots[0].advance_life();
        cancel_safe_owner_weapons(&mut state, 120, map);
        assert!(state.active_projectiles.is_empty() && state.active_decoys.is_empty());
        assert_eq!(state.clients[&peer].pending_realtime_events.queue.len(), 1);
        assert!(
            state
                .owner_weapon_cancellation_watermarks
                .contains_key(&owner.session)
        );
    }
}
