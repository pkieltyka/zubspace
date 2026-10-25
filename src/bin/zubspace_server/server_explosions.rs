//! Frozen explosion geometry and delivery, shared by blast-hit transactions
//! and wall explosions which have no blast victim.
use crate::authority_identity::PlayerSessionIdentity;
use crate::authority_projectiles::{ProjectileContactReason, ProjectileHit, ProjectileRecord};
use crate::gameplay_rules::RulesVector;
use crate::snapshot::RealtimeEventSessionDelivery;
use crate::state::ServerState;

pub(crate) fn freeze_wall_explosions(state: &mut ServerState) {
    let pending = state
        .active_projectiles
        .pending_wall_explosions()
        .copied()
        .collect::<Vec<_>>();
    let live_ids = pending
        .iter()
        .map(|explosion| explosion.parent.id)
        .collect::<std::collections::BTreeSet<_>>();
    state
        .pending_wall_explosion_transactions
        .retain(|id, _| live_ids.contains(id));
    for explosion in pending {
        if state
            .pending_wall_explosion_transactions
            .contains_key(&explosion.parent.id)
        {
            continue;
        }
        let position = RulesVector::new(explosion.parent.pos_x, explosion.parent.pos_y);
        if let Some(prepared) =
            prepare_explosion(state, explosion.parent, position, explosion.collision_tick)
        {
            state
                .pending_wall_explosion_transactions
                .insert(explosion.parent.id, prepared);
        } else {
            state
                .active_projectiles
                .resolve_wall_explosion(explosion.parent.id);
        }
    }
}

pub(crate) fn commit_wall_explosions(
    state: &std::sync::Arc<std::sync::Mutex<ServerState>>,
    commit_tick: u32,
) {
    let mut state = state.lock().expect("server state should not be poisoned");
    let pending = state
        .active_projectiles
        .pending_wall_explosions()
        .take(crate::authority_projectiles::MAX_PENDING_HIT_RETRIES_PER_TICK)
        .map(|explosion| explosion.parent.id)
        .collect::<Vec<_>>();
    for id in pending {
        let Some(explosion) = state.pending_wall_explosion_transactions.get(&id).cloned() else {
            continue;
        };
        if !explosion.can_commit(&state) {
            break;
        }
        let recipients = explosion.recipient_groups(&state);
        let Some(reservation) = crate::snapshot::reserve_realtime_event_session_deliveries(
            &mut state,
            &recipients,
            crate::snapshot::RealtimeEventPressureDisposition::Deferred,
        ) else {
            break;
        };
        let deliveries = explosion.commit(&mut state, commit_tick);
        reservation.commit_sessions(&mut state, deliveries);
        state.active_projectiles.resolve_wall_explosion(id);
        state.pending_wall_explosion_transactions.remove(&id);
    }
    state.prune_retired_projectile_owners();
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedProjectileExplosion {
    parent: ProjectileRecord,
    position: RulesVector,
    collision_tick: u32,
    children: Vec<ProjectileRecord>,
    child_lifetime_ticks: u32,
    terminal_recipients: Vec<PlayerSessionIdentity>,
    child_recipients: Vec<PlayerSessionIdentity>,
}

pub(crate) fn prepare_collision_explosion(
    state: &ServerState,
    hit: &ProjectileHit,
) -> Option<PreparedProjectileExplosion> {
    let parent = *state.active_projectiles.projectile(hit.projectile_id)?;
    let kind = (parent.weapon_data & 0x1f) as u8;
    if !matches!(kind, 3 | 4 | 8) {
        return None;
    }
    let reason = state
        .active_projectiles
        .collision_witness(hit.projectile_id)
        .map(|witness| witness.contact_reason);
    let rollback = matches!(kind, 3 | 4) && reason != Some(ProjectileContactReason::WallExplosion)
        || kind == 8
            && matches!(
                reason,
                Some(
                    ProjectileContactReason::ProximityImmediate
                        | ProjectileContactReason::ProximityClosestApproach
                        | ProjectileContactReason::ProximityDeadline
                )
            );
    let mut position = RulesVector::new(parent.pos_x, parent.pos_y);
    if rollback {
        // The reference game restores the previous 100Hz simulation point for bomb
        // contacts and Thor proximity, while blast damage keeps its own anchor.
        position.x -= f32::from(parent.velocity_x) / 1000.0;
        position.y -= f32::from(parent.velocity_y) / 1000.0;
    }
    prepare_explosion(state, parent, position, hit.tick)
}

pub(crate) fn prepare_explosion(
    state: &ServerState,
    parent: ProjectileRecord,
    position: RulesVector,
    collision_tick: u32,
) -> Option<PreparedProjectileExplosion> {
    let source = parent.presentation_source?;
    if !matches!(parent.weapon_data & 0x1f, 3 | 4 | 8) {
        return None;
    }
    let tuning = crate::arena_settings::arena_explosion_fragment_tuning(source.ship);
    let map = state.zone_map();
    let door_seeds = crate::doors::client_door_simulation_seed_at_tick(
        Some(state.server_door_state),
        collision_tick,
    )
    .into_iter()
    .collect::<Vec<_>>();
    let children = crate::authority_projectiles::build_shrapnel_records(
        &parent,
        tuning,
        position,
        collision_tick,
        |x, y, frequency| {
            let extent = crate::zone::MAP_TILES as f32 * crate::zone::TILE_SIZE_PX;
            if x < 0.0 || y < 0.0 || x >= extent || y >= extent {
                return true;
            }
            if crate::zone::position_blocking_tile_for_door_seeds(
                crate::zone::tile_at_world(map, x as u16, y as u16),
                &door_seeds,
            ) {
                return true;
            }
            let tile = ((x / 16.0).floor() as u16, (y / 16.0).floor() as u16);
            state.active_bricks.iter().any(|brick| {
                !crate::continuum::continuum_tick_reached(collision_tick, brick.expires_at_tick)
                    && brick
                        .packet
                        .get(9..11)
                        .is_some_and(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]) != frequency)
                    && brick.tiles.contains(&tile)
            })
        },
        || 0, // IDs are allocated exactly once, after reservation.
    );
    let child_recipients = state
        .clients
        .iter()
        .filter_map(|(&id, client)| {
            client
                .joined
                .then_some(PlayerSessionIdentity::new(id, client.session_id))
        })
        .collect::<Vec<_>>();
    let terminal_recipients = child_recipients
        .iter()
        .copied()
        .filter(|recipient| {
            recipient.player_id != parent.owner_id
                || (parent.combat_timeline.is_some()
                    && recipient.session_id.get() == source.source_session_id)
        })
        .collect();
    Some(PreparedProjectileExplosion {
        parent,
        position,
        collision_tick,
        children,
        child_lifetime_ticks: tuning.alive_ticks,
        terminal_recipients,
        child_recipients,
    })
}

impl PreparedProjectileExplosion {
    pub(crate) fn can_commit(&self, state: &ServerState) -> bool {
        let count = if state.projectile_cancelled_by_owner(&self.parent) {
            0
        } else {
            self.children.len()
        };
        state.can_record_explosion_fan(self.parent.owner_id, count)
            && self.child_recipients.iter().all(|recipient| {
                !state.player_session_matches(*recipient)
                    || state.clients[&recipient.player_id]
                        .threat_presentation
                        .can_queue_new_projectiles(count)
            })
    }

    pub(crate) fn recipient_groups(&self, state: &ServerState) -> Vec<&[PlayerSessionIdentity]> {
        let mut groups = vec![self.terminal_recipients.as_slice()];
        groups.extend(std::iter::repeat_n(
            self.child_recipients.as_slice(),
            if state.projectile_cancelled_by_owner(&self.parent) {
                0
            } else {
                self.children.len()
            },
        ));
        groups
    }

    /// Called under the same state lock, after the full transaction reserves
    /// capacity and revalidates predecessors. Fragment flight begins when the
    /// explosion commits, so queue delay cannot expire an invisible fan.
    pub(crate) fn commit(
        &self,
        state: &mut ServerState,
        commit_tick: u32,
    ) -> Vec<RealtimeEventSessionDelivery> {
        let source = self
            .parent
            .presentation_source
            .expect("prepared explosion source");
        let mut deliveries = vec![RealtimeEventSessionDelivery {
            recipients: self.terminal_recipients.clone(),
            server_tick: self.collision_tick,
            payload: crate::realtime_event::RealtimeEventPayload::ProjectileState(
                crate::realtime_event::RealtimeProjectileStateEvent {
                    source_input_sequence: self
                        .parent
                        .combat_timeline
                        .map(|binding| binding.owner_input_sequence),
                    terminal: true,
                    motion_frozen: false,
                    source,
                    projectile_id: self.parent.id,
                    source_server_tick: self.parent.created_tick,
                    source_projectile_index: self.parent.shot_index,
                    weapon_data: self.parent.weapon_data,
                    position_x: self.position.x,
                    position_y: self.position.y,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    expiry_tick: Some(self.collision_tick),
                    behavior: crate::realtime_event::PROJECTILE_BEHAVIOR_STANDARD,
                    bounces_remaining: 0,
                    proximity: None,
                },
            ),
        }];
        let mut children = if state.projectile_cancelled_by_owner(&self.parent) {
            Vec::new()
        } else {
            self.children.clone()
        };
        let fragment_count = children.len();
        for child in &mut children {
            child.id = state.allocate_projectile_id();
            child.created_tick = commit_tick;
            child.timestamp = commit_tick;
            child.expiry_tick = Some(crate::continuum::continuum_tick_add(
                commit_tick,
                self.child_lifetime_ticks,
            ));
            deliveries.push(RealtimeEventSessionDelivery {
                recipients: self.child_recipients.clone(),
                server_tick: commit_tick,
                payload: crate::realtime_event::RealtimeEventPayload::ProjectileState(
                    crate::realtime_event::RealtimeProjectileStateEvent {
                        source_input_sequence: child
                            .combat_timeline
                            .map(|binding| binding.owner_input_sequence),
                        terminal: false,
                        motion_frozen: false,
                        source,
                        projectile_id: child.id,
                        source_server_tick: commit_tick,
                        source_projectile_index: child.shot_index,
                        weapon_data: child.weapon_data,
                        position_x: child.pos_x,
                        position_y: child.pos_y,
                        velocity_x: child.motion_velocity().x,
                        velocity_y: child.motion_velocity().y,
                        expiry_tick: child.expiry_tick,
                        behavior: crate::realtime_event::PROJECTILE_BEHAVIOR_STANDARD,
                        bounces_remaining: 0,
                        proximity: None,
                    },
                ),
            });
        }
        state.record_projectile_group(children);
        if state.qa_traces.is_recording() {
            state.qa_traces.record_for_all(serde_json::json!({
                "type": "server_projectile_explosion_committed",
                "parentProjectileId": self.parent.id,
                "shotGroupId": self.parent.shot_group_id.to_string(),
                "ownerId": self.parent.owner_id,
                "ownerLifeSeq": self.parent.owner_life_seq,
                "collisionTick": self.collision_tick,
                "commitTick": commit_tick,
                "fragmentCount": fragment_count,
                "preparedFragmentCount": self.children.len(),
                "x": self.position.x,
                "y": self.position.y,
            }));
        }
        deliveries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, mpsc};

    fn fixture(bot_count: usize) -> (Arc<Mutex<ServerState>>, u16, ProjectileRecord) {
        let state = Arc::new(Mutex::new(ServerState {
            config: crate::config::ServerConfig {
                bot_count,
                ..Default::default()
            },
            ..Default::default()
        }));
        state.lock().unwrap().initialize_configured_bots();
        let (tx, _rx) = mpsc::channel();
        let observer = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, observer, 8);
        let parent = {
            let mut guard = state.lock().unwrap();
            let bot = &guard.bots[0];
            let weapon_data = crate::gameplay_rules::encode_weapon_data(3, 2, true, 3, 4, false);
            let source = crate::realtime_event::RealtimeWeaponEvent {
                source_player_id: bot.player.id,
                source_session_id: bot.session_id.get(),
                source_life_seq: bot.life_seq,
                shot_group_id: 1000,
                x: bot.x,
                y: bot.y,
                velocity_x: 0.0,
                velocity_y: 0.0,
                orientation: 0.25,
                frequency: bot.player.frequency,
                ship: bot.player.ship,
                weapon_data,
            };
            ProjectileRecord {
                id: guard.allocate_projectile_id(),
                shot_group_id: 1000,
                shot_index: 0,
                presentation_source: Some(source),
                owner_id: source.source_player_id,
                owner_life_seq: source.source_life_seq,
                combat_timeline: None,
                weapon_data,
                x: source.x.round() as u16,
                y: source.y.round() as u16,
                timestamp: 100,
                created_tick: 100,
                velocity_x: 1000,
                velocity_y: 0,
                expiry_tick: Some(1000),
                pos_x: source.x,
                source_server_tick: None,
                precise_velocity: None,
                pos_y: source.y,
                bounces_remaining: 0,
                behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
                proximity_target: None,
                proximity_sensor_end_tick: None,
                proximity_highest_offset: 0.0,
            }
        };
        (state, observer, parent)
    }

    fn fill_queue(state: &mut ServerState, observer: u16, parent: ProjectileRecord) {
        let recipient = state.active_player_runtime_identity(observer).unwrap();
        let envelope = crate::snapshot::PendingRealtimeEventEnvelope {
            flight_transition: false,
            recipient,
            event_id: 42,
            server_tick: 100,
            payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                parent.presentation_source.unwrap(),
            ),
        };
        state
            .clients
            .get_mut(&observer)
            .unwrap()
            .pending_realtime_events
            .queue = std::iter::repeat_n(
            envelope,
            crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
        )
        .collect();
    }

    #[test]
    fn shrapnel_wall_without_blast_victim_retries_and_commits_once() {
        let (state, observer, parent) = fixture(1);
        {
            let mut guard = state.lock().unwrap();
            guard.record_projectile(parent);
            let hull = crate::authority_projectiles::PlayerHullSnapshot {
                player_id: parent.owner_id,
                life_seq: parent.owner_life_seq,
                x: parent.pos_x + 2000.0,
                y: parent.pos_y + 2000.0,
                frequency: parent.presentation_source.unwrap().frequency,
                radius_px: 14.0,
                motion_segment: None,
                motion_samples: Vec::new(),
                motion_source_kind: Some(
                    crate::combat_timeline::ActorMotionSourceKind::ServerBotSimulation,
                ),
                lag_compensation_ticks: 0,
                life_started_tick: None,
            };
            guard
                .active_projectiles
                .step(100, |_, _| false, std::slice::from_ref(&hull));
            assert!(
                guard
                    .active_projectiles
                    .step(101, |x, _| x > parent.pos_x + 0.5, &[hull])
                    .is_empty()
            );
            assert_eq!(guard.active_projectiles.len(), 0);
            assert_eq!(
                guard.active_projectiles.pending_wall_explosions().count(),
                1
            );
            freeze_wall_explosions(&mut guard);
            assert_eq!(
                guard.pending_wall_explosion_transactions[&parent.id]
                    .children
                    .len(),
                4
            );
            fill_queue(&mut guard, observer, parent);
        }
        commit_wall_explosions(&state, 150);
        {
            let mut guard = state.lock().unwrap();
            assert_eq!(guard.active_projectiles.len(), 0);
            assert_eq!(
                guard.next_projectile_id, parent.id,
                "deferral must not allocate child IDs"
            );
            assert_eq!(
                guard.active_projectiles.pending_wall_explosions().count(),
                1
            );
            guard
                .clients
                .get_mut(&observer)
                .unwrap()
                .pending_realtime_events
                .queue
                .clear();
        }
        commit_wall_explosions(&state, 200);
        {
            let guard = state.lock().unwrap();
            assert_eq!(guard.active_projectiles.len(), 4);
            assert_eq!(
                guard.active_projectiles.pending_wall_explosions().count(),
                0
            );
            assert!(guard.pending_wall_explosion_transactions.is_empty());
            let owner = guard.bots[0].runtime_identity();
            let observer = &guard.clients[&observer];
            assert_eq!(observer.pending_realtime_events.queue.len(), 5);
            for child in guard.active_projectiles.iter() {
                assert_eq!(child.weapon_data & 0x1f, 15);
                assert_eq!(child.created_tick, 200);
                assert_eq!(
                    child.expiry_tick,
                    Some(crate::continuum::continuum_tick_add(
                        200,
                        crate::arena_settings::arena_weapon_lifetime_ticks(15).unwrap()
                    ))
                );
                let identity =
                    crate::threat_presentation::ProjectilePresentationIdentity::projectile(
                        owner,
                        child.shot_group_id,
                        child.id,
                    )
                    .unwrap();
                assert!(
                    observer
                        .threat_presentation
                        .exact_projectile_known(identity)
                );
            }
            for envelope in &observer.pending_realtime_events.queue {
                let event = crate::realtime_event::AuthenticatedRealtimeEvent {
                    server_session_id: 1,
                    arena_stream_id: 2,
                    event_id: envelope.event_id,
                    server_tick: envelope.server_tick,
                    payload: envelope.payload,
                };
                assert_eq!(
                    crate::realtime_event::AuthenticatedRealtimeEvent::parse(&event.encode()),
                    Ok(event)
                );
            }
        }
        commit_wall_explosions(&state, 201);
        let guard = state.lock().unwrap();
        assert_eq!(guard.active_projectiles.len(), 4);
        assert_eq!(
            guard.clients[&observer].pending_realtime_events.queue.len(),
            5
        );
    }

    #[test]
    fn scoreboard_bounty_points_commit_once_with_death_and_survive_bot_respawn() {
        use crate::server_damage::{
            DetectedHitApplication, try_apply_server_detected_hit_with_broadcast,
        };
        let (state, observer, parent) = fixture(2);
        let hit = {
            let mut guard = state.lock().unwrap();
            guard.bots[1].x = parent.pos_x + 10.0;
            guard.bots[1].y = parent.pos_y;
            guard.bots[1].energy = 1;
            guard.bots[1].bounty = 47;
            let hit = ProjectileHit {
                projectile_id: parent.id,
                owner_id: parent.owner_id,
                owner_life_seq: parent.owner_life_seq,
                victim_id: guard.bots[1].player.id,
                victim_life_seq: guard.bots[1].life_seq,
                weapon_data: parent.weapon_data,
                x: parent.x,
                y: parent.y,
                tick: 150,
            };
            guard.record_projectile(parent);
            fill_queue(&mut guard, observer, parent);
            hit
        };
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::DeferredCapacity
        );
        {
            let mut guard = state.lock().unwrap();
            assert_eq!(guard.bots[0].player.kill_points, 0);
            assert_eq!(guard.bots[0].player.wins, 0);
            assert_eq!(guard.bots[1].player.losses, 0);
            guard
                .clients
                .get_mut(&observer)
                .unwrap()
                .pending_realtime_events
                .queue
                .clear();
        }
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Applied
        );
        assert_eq!(
            try_apply_server_detected_hit_with_broadcast(&state, &hit),
            DetectedHitApplication::Invalid
        );
        let mut guard = state.lock().unwrap();
        assert_eq!(guard.bots[0].player.kill_points, 47);
        assert_eq!(guard.bots[0].player.wins, 1);
        assert_eq!(guard.bots[1].player.losses, 1);
        let roster = crate::roster::player_entering(&guard.bots[0].player);
        assert_eq!(u32::from_le_bytes(roster[43..47].try_into().unwrap()), 47);
        let update = crate::roster::score_update(&guard.bots[0].player);
        assert_eq!(u32::from_le_bytes(update[3..7].try_into().unwrap()), 47);
        guard.bots[0].advance_life();
        guard.bots[0].reset_loadout_at_tick(200);
        assert_eq!(guard.bots[0].player.kill_points, 47);
        assert_eq!(guard.bots[0].player.wins, 1);
    }

    #[test]
    fn retired_source_bomb_uses_original_ship_traits_after_successor_ship_change() {
        let (state, _, parent) = fixture(2);
        let hit = {
            let mut guard = state.lock().unwrap();
            let old_runtime = guard.bots[0].runtime_identity();
            guard.bots[1].x = parent.pos_x + 10.0;
            guard.bots[1].y = parent.pos_y;
            guard.bots[1].engine_shutdown_until_tick = None;
            let hit = ProjectileHit {
                projectile_id: parent.id,
                owner_id: parent.owner_id,
                owner_life_seq: parent.owner_life_seq,
                victim_id: guard.bots[1].player.id,
                victim_life_seq: guard.bots[1].life_seq,
                weapon_data: parent.weapon_data,
                x: parent.x,
                y: parent.y,
                tick: 150,
            };
            guard.record_projectile(parent);
            guard.retire_projectile_owner_after_damage_death(old_runtime, 110);
            guard.bots[0].advance_life();
            guard.bots[0].player.ship = 5; // Successor Weasel must not turn the old Warbird bomb into EMP.
            hit
        };
        assert_eq!(
            crate::server_damage::try_apply_server_detected_hit_with_broadcast(&state, &hit),
            crate::server_damage::DetectedHitApplication::Applied
        );
        let guard = state.lock().unwrap();
        assert_eq!(guard.bots[1].engine_shutdown_until_tick, None);
        assert_eq!(
            guard
                .active_projectiles
                .iter()
                .filter(|child| child.weapon_data & 0x1f == 15)
                .count(),
            4
        );
    }

    #[test]
    fn safe_owner_cancellation_keeps_frozen_blast_terminal_but_cancels_delayed_fragment_flight() {
        let (shared, _observer, parent) = fixture(1);
        let mut state = shared.lock().unwrap();
        let explosion = prepare_explosion(
            &state,
            parent,
            RulesVector::new(parent.pos_x, parent.pos_y),
            100,
        )
        .unwrap();
        assert!(!explosion.children.is_empty());
        let source = parent.presentation_source.unwrap();
        let session = PlayerSessionIdentity::new(
            source.source_player_id,
            crate::authority_identity::PlayerSessionId::new(source.source_session_id),
        );
        state
            .owner_weapon_cancellation_watermarks
            .insert(session, parent.shot_group_id);
        assert!(explosion.can_commit(&state));
        assert_eq!(explosion.recipient_groups(&state).len(), 1);
        let deliveries = explosion.commit(&mut state, 300);
        assert_eq!(deliveries.len(), 1);
        assert!(
            matches!(deliveries[0].payload, crate::realtime_event::RealtimeEventPayload::ProjectileState(p) if p.terminal)
        );
        assert!(
            !state
                .active_projectiles
                .iter()
                .any(|p| p.weapon_data & 0x1f == 15)
        );
    }

    #[test]
    fn shrapnel_zero_damage_contact_still_reserves_its_terminal() {
        let (state, observer, mut parent) = fixture(2);
        parent.weapon_data = crate::gameplay_rules::encode_weapon_data(3, 2, false, 1, 1, false);
        parent.presentation_source.as_mut().unwrap().weapon_data = parent.weapon_data;
        let (hit, before) = {
            let mut guard = state.lock().unwrap();
            let explosion = prepare_explosion(
                &guard,
                parent,
                RulesVector::new(parent.pos_x, parent.pos_y),
                100,
            )
            .unwrap();
            let recipients = explosion.recipient_groups(&guard);
            let reservation = crate::snapshot::reserve_realtime_event_session_deliveries(
                &mut guard,
                &recipients,
                crate::snapshot::RealtimeEventPressureDisposition::Deferred,
            )
            .unwrap();
            let deliveries = explosion.commit(&mut guard, 100);
            reservation.commit_sessions(&mut guard, deliveries);
            let child = *guard.active_projectiles.last().unwrap();
            let victim = &guard.bots[1];
            let hit = ProjectileHit {
                projectile_id: child.id,
                owner_id: child.owner_id,
                owner_life_seq: child.owner_life_seq,
                victim_id: victim.player.id,
                victim_life_seq: victim.life_seq,
                weapon_data: child.weapon_data,
                x: child.x,
                y: child.y,
                tick: 101,
            };
            let before = victim.energy;
            fill_queue(&mut guard, observer, parent);
            (hit, before)
        };
        assert_eq!(
            crate::server_damage::try_apply_server_detected_hit_with_broadcast(&state, &hit),
            crate::server_damage::DetectedHitApplication::DeferredCapacity
        );
        state
            .lock()
            .unwrap()
            .clients
            .get_mut(&observer)
            .unwrap()
            .pending_realtime_events
            .queue
            .clear();
        assert_eq!(
            crate::server_damage::try_apply_server_detected_hit_with_broadcast(&state, &hit),
            crate::server_damage::DetectedHitApplication::Applied
        );
        let guard = state.lock().unwrap();
        assert_eq!(guard.bots[1].energy, before);
        let queue = &guard.clients[&observer].pending_realtime_events.queue;
        assert_eq!(queue.len(), 1);
        assert!(
            matches!(queue[0].payload, crate::realtime_event::RealtimeEventPayload::ProjectileState(state) if state.terminal && state.projectile_id == hit.projectile_id)
        );
    }

    #[test]
    fn shrapnel_damage_and_fragment_delivery_share_one_capacity_reservation() {
        check_blast_and_fragment_reservation(false);
    }

    #[test]
    fn shrapnel_zero_damage_blast_still_retries_and_delivers_the_fan() {
        check_blast_and_fragment_reservation(true);
    }

    fn check_blast_and_fragment_reservation(zero_damage: bool) {
        let (state, observer, parent) = fixture(2);
        let (hit, energy_before) = {
            let mut guard = state.lock().unwrap();
            let distance = if zero_damage {
                crate::arena_settings::arena_bomb_explode_pixels()
                    * crate::continuum_weapon::continuum_weapon_explosion_level(parent.weapon_data)
                        as f32
                    - 0.001
            } else {
                10.0
            };
            guard.bots[1].x = parent.x as f32 + distance;
            guard.bots[1].y = parent.pos_y;
            let hit = ProjectileHit {
                projectile_id: parent.id,
                owner_id: parent.owner_id,
                owner_life_seq: parent.owner_life_seq,
                victim_id: guard.bots[1].player.id,
                victim_life_seq: guard.bots[1].life_seq,
                weapon_data: parent.weapon_data,
                x: parent.x,
                y: parent.y,
                tick: 101,
            };
            let energy = guard.bots[1].energy;
            guard.record_projectile(parent);
            fill_queue(&mut guard, observer, parent);
            (hit, energy)
        };
        assert_eq!(
            crate::server_damage::try_apply_server_detected_hit_with_broadcast(&state, &hit),
            crate::server_damage::DetectedHitApplication::DeferredCapacity
        );
        {
            let mut guard = state.lock().unwrap();
            assert_eq!(guard.bots[1].energy, energy_before);
            assert_eq!(guard.active_projectiles.len(), 1);
            assert_eq!(guard.next_projectile_id, parent.id);
            guard
                .clients
                .get_mut(&observer)
                .unwrap()
                .pending_realtime_events
                .queue
                .clear();
        }
        assert_eq!(
            crate::server_damage::try_apply_server_detected_hit_with_broadcast(&state, &hit),
            crate::server_damage::DetectedHitApplication::Applied
        );
        let guard = state.lock().unwrap();
        if zero_damage {
            assert_eq!(guard.bots[1].energy, energy_before);
        } else {
            assert!(guard.bots[1].energy < energy_before);
        }
        assert_eq!(
            guard
                .active_projectiles
                .iter()
                .filter(|child| child.weapon_data & 0x1f == 15)
                .count(),
            4
        );
        let queue = &guard.clients[&observer].pending_realtime_events.queue;
        assert_eq!(queue.iter().filter(|envelope| matches!(envelope.payload, crate::realtime_event::RealtimeEventPayload::ProjectileState(state) if state.terminal && state.projectile_id == parent.id)).count(), 1);
        assert_eq!(queue.iter().filter(|envelope| matches!(envelope.payload, crate::realtime_event::RealtimeEventPayload::ProjectileState(state) if !state.terminal && state.weapon_data & 0x1f == 15)).count(), 4);
    }
}
