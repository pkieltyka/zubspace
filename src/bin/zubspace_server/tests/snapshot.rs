use super::*;
use crate::snapshot::{
    AOI_PROXY_LIFETIME_SNAPSHOTS, LOAD_SHED_FRAME_THRESHOLD,
    LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS, LOAD_SHED_MID_THRESHOLD, NearEntityState,
    SNAPSHOT_ENTITY_HEADER_LEN, SNAPSHOT_EVENT_HEADER_LEN, SNAPSHOT_EVENT_WATCH_DAMAGE,
    SNAPSHOT_EVENT_WATCH_DAMAGE_BODY_LEN, SNAPSHOT_FIELD_BOUNTY, SNAPSHOT_FIELD_DIRECTION,
    SNAPSHOT_FIELD_ENERGY, SNAPSHOT_FIELD_FLAG_TIMER, SNAPSHOT_FIELD_LIFE_SEQ, SNAPSHOT_FIELD_PING,
    SNAPSHOT_FIELD_STATUS, SNAPSHOT_FIELD_VELOCITY_X, SNAPSHOT_FIELD_VELOCITY_Y,
    SNAPSHOT_FIELD_WEAPON, SNAPSHOT_FIELD_X, SNAPSHOT_FIELD_Y, SNAPSHOT_FLAG_FULL,
    SNAPSHOT_HEADER_LEN, SNAPSHOT_HISTORY_CAPACITY, SNAPSHOT_PACKET, SnapshotEntity,
    SnapshotEntityMap, SnapshotEvent, SnapshotFrame, build_empty_snapshot, compute_aoi_entities,
    delta_encode_mid_tier, delta_encode_near_tier, emit_snapshots_for_tick_at_tick,
    emit_snapshots_for_tick_inner, near_snapshot_interval_ticks, snapshot_interval_ticks,
};
use crate::snapshot_wire::SnapshotPrizeState;
use crate::state::LOAD_SHED_FRAME_OVERRUN_STREAK;

// Snapshot-content tests model a client that has consumed and ACKed its join
// bootstrap. Transport ordering/loss behavior is covered in lifecycle tests.
fn join_arena(state: &Arc<Mutex<ServerState>>, player_id: u16, ship: u8) {
    crate::arena::join_arena(state, player_id, ship);
    acknowledge_join_bootstrap(state, player_id);
}

fn canonical_respawn_position_packet(state: &Arc<Mutex<ServerState>>, player_id: u16) -> Vec<u8> {
    let (spawn_x, spawn_y, bounty, energy) = {
        let state = state.lock().expect("server state should not be poisoned");
        let mut prospective = state.clients[&player_id].clone();
        prospective.alive = true;
        prospective.reset_server_runtime_for_ship_at_tick(crate::wire::current_tick());
        let (spawn_x, spawn_y) = server_spawn_coordinates(state.zone_map());
        (
            spawn_x,
            spawn_y,
            prospective.server_bounty,
            prospective.server_energy,
        )
    };
    let mut packet = client_position_packet(0, spawn_x, spawn_y);
    packet[1] = 0;
    packet[16..18].copy_from_slice(&bounty.to_le_bytes());
    packet[18..20].copy_from_slice(&energy.to_le_bytes());
    packet
}

fn near_state(x: u16, y: u16) -> NearEntityState {
    NearEntityState {
        x,
        y,
        ..NearEntityState::default()
    }
}

// Synthetic snapshot fixtures install a position without running movement
// admission. Seed its matching accepted history explicitly; production must
// never invent this baseline during publication.
fn seed_snapshot_motion(client: &mut crate::state::ClientHandle) {
    let state =
        crate::snapshot::extract_near_entity_state(client.last_position.as_deref().unwrap())
            .unwrap();
    client.begin_actor_motion_segment();
    let segment = client.actor_motion_history.current_segment().unwrap();
    let sample = crate::combat_timeline::ActorMotionSampleV1::from_state(
        segment,
        client.last_position_server_tick.unwrap_or(0),
        state.x as f32,
        state.y as f32,
        state.velocity_x as f32 / 10.0,
        state.velocity_y as f32 / 10.0,
        crate::continuum_position::protocol_direction_to_orientation(state.direction),
        crate::arena_settings::arena_ship_radius_pixels(client.player.ship),
        0,
        crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
    );
    client
        .actor_motion_history
        .begin_accepted_samples(client.last_position_sequence.unwrap_or(0), sample)
        .unwrap();
}

#[test]
fn server_emits_one_snapshot_per_joined_client_at_interval_tick() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        // Default tick rate is 40 Hz, where both near and mid intervals
        // are 1 tick, so arena_tick = 2 emits a frame carrying both tiers.
        state_guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);

    let alpha_packet = rx_a.try_recv().expect("alpha");
    let beta_packet = rx_b.try_recv().expect("beta");

    assert_eq!(
        alpha_packet.len(),
        SNAPSHOT_HEADER_LEN
            + SnapshotEvent::HEADER_LEN
            + SnapshotEvent::ROCKET_STATE_BODY_LEN
            + SnapshotEvent::HEADER_LEN
            + SnapshotEvent::LIFECYCLE_STATE_BODY_LEN
            + SnapshotEvent::HEADER_LEN
            + SnapshotEvent::PRIZE_STATE_BODY_LEN
            + SnapshotEvent::HEADER_LEN
            + SnapshotEvent::OBSERVER_TIMELINE_BODY_LEN
    );
    assert_eq!(alpha_packet[0], SNAPSHOT_PACKET);
    assert_eq!(beta_packet.len(), alpha_packet.len());
    assert_eq!(beta_packet[0], SNAPSHOT_PACKET);

    // Both clients receive snapshot_id == 1 because each tracks its
    // own monotonic counter; recipients don't share a counter.
    assert_eq!(u16::from_le_bytes([alpha_packet[2], alpha_packet[3]]), 1);
    assert_eq!(u16::from_le_bytes([beta_packet[2], beta_packet[3]]), 1);

    // FULL is explicit; base_id is ignored for this frame.
    assert_eq!(alpha_packet[1], SNAPSHOT_FLAG_FULL | 14);
    assert_eq!(u16::from_le_bytes([alpha_packet[4], alpha_packet[5]]), 0);

    // No AOI entities yet; repeated recipient-private Rocket, lifecycle,
    // prize, and observer-timeline state keep authority self-healing from the
    // first frame.
    assert_eq!(alpha_packet[18], 0);
    assert_eq!(alpha_packet[19], 0);
    assert_eq!(alpha_packet[20], 4);
    assert!(matches!(
        SnapshotFrame::parse(&alpha_packet)
            .unwrap()
            .events
            .as_slice(),
        [
            SnapshotEvent::RocketState(_),
            SnapshotEvent::LifecycleState(_),
            SnapshotEvent::PrizeState(_),
            SnapshotEvent::ObserverTimeline(_)
        ]
    ));
    let frame = SnapshotFrame::parse(&alpha_packet).expect("snapshot");
    let emitted_directive = frame
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::ObserverTimeline(directive) => Some(*directive),
            _ => None,
        })
        .expect("snapshot must repeat the observer timeline directive");
    assert_eq!(
        Some(emitted_directive),
        state.lock().unwrap().clients[&alpha]
            .observer_combat_timeline
            .current_directive()
    );
}

#[test]
fn published_history_preserves_all_forty_source_headings() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let mut peers = Vec::new();
    for x in [500, 550] {
        let (tx, rx) = mpsc::channel();
        let id = register_connection(&state, tx);
        join_arena(&state, id, 0);
        let mut locked = state.lock().unwrap();
        let position =
            large_position_from_client(id, &client_position_packet_with_sequence(0, x, 700, 1))
                .unwrap();
        locked.set_client_position(id, position);
        seed_snapshot_motion(locked.clients.get_mut(&id).unwrap());
        peers.push((id, rx));
    }
    let source = peers[0].0;
    for direction in 0..40_u8 {
        for (_, rx) in &peers {
            drain_available(rx);
        }
        {
            let mut locked = state.lock().unwrap();
            locked.arena_tick = 2 + u64::from(direction) * 2;
            let mut packet =
                client_position_packet_with_sequence(0, 500, 700, u16::from(direction) + 2);
            packet[1] = direction;
            let position = large_position_from_client(source, &packet).unwrap();
            locked.set_client_position(source, position);
            seed_snapshot_motion(locked.clients.get_mut(&source).unwrap());
        }
        emit_snapshots_for_tick_at_tick(&state, 9_000 + u32::from(direction));
        let packet = peers[1]
            .1
            .try_iter()
            .find(|p| p.first() == Some(&SNAPSHOT_PACKET))
            .unwrap();
        let frame = SnapshotFrame::parse(&packet).unwrap();
        let published = frame
            .near_entities
            .iter()
            .find(|entity| entity.entity_id == source)
            .unwrap();
        assert_eq!(
            published.direction,
            Some(direction),
            "source direction {direction}"
        );
    }
}

#[test]
fn publication_samples_are_shared_per_tick_but_not_across_ticks_or_lives() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let mut peers = Vec::new();
    for x in [500, 550, 600] {
        let (tx, rx) = mpsc::channel();
        let id = register_connection(&state, tx);
        join_arena(&state, id, 0);
        let mut locked = state.lock().unwrap();
        let position =
            large_position_from_client(id, &client_position_packet_with_sequence(0, x, 700, 1))
                .unwrap();
        locked.set_client_position(id, position);
        seed_snapshot_motion(locked.clients.get_mut(&id).unwrap());
        peers.push((id, rx));
    }
    for (_, rx) in &peers {
        drain_available(rx);
    }
    let source = peers[0].0;
    let initial_life = state.lock().unwrap().clients[&source].life_seq;
    for phase in 0..4_u32 {
        {
            let mut locked = state.lock().unwrap();
            locked.arena_tick = u64::from(2 + phase * 2);
            if phase == 1 || phase == 3 {
                let x = if phase == 1 { 700 } else { 900 };
                let position = large_position_from_client(
                    source,
                    &client_position_packet_with_sequence(0, x, 700, (phase + 1) as u16),
                )
                .unwrap();
                locked.set_client_position(source, position);
            }
            if phase == 2 {
                locked.clients.get_mut(&source).unwrap().life_seq += 1;
            }
            if phase == 3 {
                seed_snapshot_motion(locked.clients.get_mut(&source).unwrap());
            }
        }
        emit_snapshots_for_tick_at_tick(&state, 9_000 + phase);
        for (recipient, rx) in &peers {
            let packet = rx
                .try_iter()
                .find(|p| p.first() == Some(&SNAPSHOT_PACKET))
                .unwrap();
            let frame = SnapshotFrame::parse(&packet).unwrap();
            let entity = frame.near_entities.iter().find(|e| e.entity_id == source);
            if *recipient != source && phase == 2 {
                assert!(
                    entity.is_none(),
                    "old-life history cannot publish a new life"
                );
                continue;
            }
            let entity = entity.expect("self and both observers retain the exact source");
            let expected_x = if phase == 3 {
                900
            } else if *recipient == source && phase >= 1 {
                700
            } else {
                500
            };
            assert_eq!(
                entity.x,
                Some(expected_x),
                "phase {phase}, recipient {recipient}"
            );
            assert_eq!(entity.life_seq, Some(initial_life + u32::from(phase >= 2)));
        }
    }
}

#[test]
fn successful_snapshot_send_records_exact_source_life_without_using_codec_ack() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (source_tx, source_rx) = mpsc::channel();
    let (recipient_tx, recipient_rx) = mpsc::channel();
    let source = register_connection(&state, source_tx);
    let recipient = register_connection(&state, recipient_tx);
    join_arena(&state, source, 0);
    join_arena(&state, recipient, 0);
    drain_available(&source_rx);
    drain_available(&recipient_rx);
    let tick = 9_000;
    let source_runtime = {
        let mut state = state.lock().expect("state");
        let source_position = large_position_from_client(
            source,
            &client_position_packet_with_sequence(tick, 500, 700, 1),
        )
        .expect("source position");
        let recipient_position = large_position_from_client(
            recipient,
            &client_position_packet_with_sequence(tick, 600, 700, 1),
        )
        .expect("recipient position");
        state.set_client_position(source, source_position);
        seed_snapshot_motion(state.clients.get_mut(&source).unwrap());
        state.set_client_position(recipient, recipient_position);
        seed_snapshot_motion(state.clients.get_mut(&recipient).unwrap());
        state.arena_tick = 2;
        state
            .active_player_runtime_identity(source)
            .expect("source runtime")
    };

    emit_snapshots_for_tick_at_tick(&state, tick);
    let _source_frame = source_rx.try_recv().expect("source frame");
    let recipient_frame = SnapshotFrame::parse(&recipient_rx.try_recv().expect("recipient frame"))
        .expect("recipient snapshot");
    assert!(
        recipient_frame.near_entities.iter().any(|entity| {
            entity.entity_id == source && entity.life_seq == Some(source_runtime.life.life_seq)
        }) || recipient_frame.mid_entities.iter().any(|entity| {
            entity.entity_id == source && entity.life_seq == Some(source_runtime.life.life_seq)
        }),
        "the exact source life is in the successfully sent frame"
    );
    let state = state.lock().expect("state");
    let recipient_client = &state.clients[&recipient];
    assert_eq!(recipient_client.last_acked_snapshot_id, None);
    assert_eq!(recipient_client.observer_presentation_history.len(), 1);
    let presentation_frame = recipient_client
        .observer_presentation_history
        .frames()
        .next()
        .expect("successful send records presentation frame");
    assert_eq!(
        presentation_frame.snapshot_id,
        recipient_frame.header.snapshot_id
    );
    assert!(presentation_frame.samples().any(|sample| {
        sample.actor == source_runtime
            && sample.motion_segment.actor == source_runtime
            && sample.position() == (500.0, 700.0)
    }));
    let decision = recipient_client
        .threat_presentation
        .readiness(source_runtime, tick);
    let contract = decision.contract.expect("emission contract");
    assert_eq!(
        contract.first_snapshot_id,
        recipient_frame.header.snapshot_id
    );
    assert_eq!(contract.first_emitted_tick, tick);
    assert_eq!(
        decision.reason,
        crate::threat_presentation::ThreatReadinessReason::AwaitingConservativeLead
    );
}

#[test]
fn bootstrap_spawn_observer_emits_nearby_bots_without_synthetic_self_authority() {
    let state = Arc::new(Mutex::new(ServerState {
        config: ServerConfig {
            bot_count: 2,
            ..ServerConfig::default()
        },
        ..ServerState::default()
    }));
    state.lock().expect("state").initialize_configured_bots();
    let (tx, _rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);

    let guard = state.lock().expect("state");
    let client = &guard.clients[&alpha];
    assert!(client.last_position.is_none());
    assert!(client.aoi_observer_position.is_some());
    let (near, mid) = compute_aoi_entities(&guard, alpha);
    let actor_ids = near
        .iter()
        .map(|(id, _)| *id)
        .chain(mid.iter().map(|(id, _, _, _)| *id))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(!actor_ids.contains(&alpha));
    assert_eq!(
        actor_ids,
        [
            crate::state::SERVER_BOT_ID_BASE,
            crate::state::SERVER_BOT_ID_BASE + 1,
        ]
        .into_iter()
        .collect(),
        "pre-map snapshots must retain bot presentation around the read-only spawn anchor"
    );
}

#[test]
fn snapshot_holds_go_to_mid_range_then_the_farthest_nearby_actors() {
    use crate::combat_timeline::PresentationSampleTime;
    use crate::snapshot_wire::{
        SnapshotEntityState, delta_encode_mid_tier, delta_encode_near_tier,
    };
    let moving = |x: u16, life_seq: u32| SnapshotEntityState {
        x,
        y: 1_000,
        velocity_x: 30,
        life_seq,
        motion_segment: 1,
        relocation_generation: 1,
        sample_time: PresentationSampleTime::from_tick(100),
        ..SnapshotEntityState::default()
    };
    // Recipient 1 at x 1,000; actor 2 close; 3 far; 4 far but respawned since
    // the base; 5 mid-range.
    let near = vec![
        (1, moving(1_000, 1)),
        (2, moving(1_100, 1)),
        (3, moving(2_500, 1)),
        (4, moving(2_600, 2)),
    ];
    let mid = vec![(5, 3_000, 1_000, 1)];
    let mut base = crate::snapshot_wire::SnapshotEntityStateMap::new();
    for (id, current) in &near {
        let earlier = SnapshotEntityState {
            x: current.x - 10,
            life_seq: 1,
            sample_time: PresentationSampleTime::from_tick(99),
            ..*current
        };
        base.insert(*id, earlier);
    }
    base.insert(
        5,
        SnapshotEntityState {
            x: 2_990,
            y: 1_000,
            life_seq: 1,
            ..SnapshotEntityState::default()
        },
    );
    let near_wire = delta_encode_near_tier(&near, Some(&base));
    let mid_wire = delta_encode_mid_tier(&mid, Some(&base));
    let holds = |consecutive: &std::collections::HashMap<u16, u8>, excess: usize| {
        crate::snapshot::choose_snapshot_holds(
            &near,
            &near_wire,
            &mid,
            &mid_wire,
            &base,
            1,
            Some((1_000, 1_000)),
            consecutive,
            None,
            excess,
        )
    };
    let none = std::collections::HashMap::new();
    // A mid entry saves 2 bytes (x), a nearby one 7 (x and sample time).
    assert_eq!(holds(&none, 2), vec![5]);
    assert_eq!(holds(&none, 3), vec![5, 3]);
    assert_eq!(holds(&none, 10), vec![5, 3, 2]);
    // The recipient and the respawned actor 4 are never held, so 17 bytes
    // cannot be saved: hold every candidate and let the budget decide.
    assert_eq!(holds(&none, 17), vec![5, 3, 2]);
    // An actor held twice in a row is updated next.
    let twice = std::collections::HashMap::from([(3, 2)]);
    assert_eq!(holds(&twice, 3), vec![5, 2]);
}

#[test]
fn an_input_receipt_advances_the_delta_base_only_to_a_frame_this_runtime_holds() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let mut ids = Vec::new();
    for tick in [2, 3, 4] {
        crate::snapshot::emit_snapshots_for_tick_at_tick(&state, tick);
        let frame = SnapshotFrame::parse(&rx.try_recv().expect("snapshot")).unwrap();
        ids.push(frame.header.snapshot_id);
    }
    let mut locked = state.lock().unwrap();
    let runtime = locked.active_player_runtime_identity(player_id).unwrap();
    let client = locked.clients.get_mut(&player_id).unwrap();
    client.last_acked_snapshot_id = Some(ids[0]);
    let receipt = |life_seq: u32, snapshot_id: u16| crate::combat_timeline::CombatViewReceiptV1 {
        observer_life_seq: life_seq,
        latest_game_applied_snapshot_id: snapshot_id,
        ..Default::default()
    };
    let life = runtime.life.life_seq;
    let advance = |client: &mut crate::state::ClientHandle, life_seq, snapshot_id| {
        crate::connection::advance_snapshot_base_from_receipt(
            client,
            runtime,
            receipt(life_seq, snapshot_id),
        );
        client.last_acked_snapshot_id
    };
    // Another life, a frame never emitted, or no frame at all: unchanged.
    assert_eq!(advance(client, life + 1, ids[2]), Some(ids[0]));
    assert_eq!(advance(client, life, ids[2].wrapping_add(5)), Some(ids[0]));
    assert_eq!(advance(client, life, 0), Some(ids[0]));
    // The newest applied frame becomes the base.
    assert_eq!(advance(client, life, ids[2]), Some(ids[2]));
    // An older receipt never moves it back.
    assert_eq!(advance(client, life, ids[1]), Some(ids[2]));
}

#[test]
fn frames_shrink_towards_one_ticks_refill_once_the_budget_runs_low() {
    use crate::snapshot::snapshot_hold_target;
    // 1,092 B refill per tick (64 KiB/s at 60 Hz).
    assert_eq!(snapshot_hold_target(800, 10_000, 1_092), 800, "plenty left");
    assert_eq!(
        snapshot_hold_target(800, 2_984, 1_092),
        800,
        "exactly two ticks spare"
    );
    assert_eq!(
        snapshot_hold_target(800, 2_983, 1_092),
        1_092,
        "low, but the frame fits a tick"
    );
    assert_eq!(
        snapshot_hold_target(1_500, 2_000, 1_092),
        1_092,
        "low: shrink to a tick"
    );
    assert_eq!(
        snapshot_hold_target(1_500, 600, 1_092),
        600,
        "never above what is left"
    );
}

#[test]
fn an_over_budget_frame_holds_a_moving_bot_instead_of_being_skipped() {
    let state = Arc::new(Mutex::new(ServerState {
        config: ServerConfig {
            bot_count: 1,
            ..ServerConfig::default()
        },
        ..ServerState::default()
    }));
    state.lock().unwrap().initialize_configured_bots();
    let (tx, rx) = mpsc::channel();
    let observer = register_connection(&state, tx);
    join_arena(&state, observer, 8);
    drain_available(&rx);
    let bot_id = crate::state::SERVER_BOT_ID_BASE;
    let start = 1_000;
    {
        let mut locked = state.lock().unwrap();
        let center = server_spawn_coordinates(locked.zone_map());
        let bot = &mut locked.bots[0];
        bot.x = f32::from(center.0 + 64);
        bot.y = f32::from(center.1);
        bot.actor_motion_history = Default::default();
        bot.begin_actor_motion_segment();
        bot.append_actor_motion_sample(start, false);
        locked.arena_tick = 2;
    }
    let frame_at = |step: u32, tokens: f64| {
        {
            let mut locked = state.lock().unwrap();
            let bot = &mut locked.bots[0];
            bot.x += 30.0;
            bot.append_actor_motion_sample(start + step, false);
            locked.arena_tick = 2 + u64::from(step) * 2;
            locked
                .clients
                .get_mut(&observer)
                .unwrap()
                .send_pacing
                .tokens = tokens;
        }
        emit_snapshots_for_tick_at_tick(&state, start + step);
        rx.try_iter().find(|p| p.first() == Some(&SNAPSHOT_PACKET))
    };
    let bot_entry = |packet: &[u8]| {
        SnapshotFrame::parse(packet)
            .unwrap()
            .near_entities
            .into_iter()
            .find(|entity| entity.entity_id == bot_id)
            .expect("the bot stays listed")
    };

    emit_snapshots_for_tick_at_tick(&state, start);
    let first = SnapshotFrame::parse(
        &rx.try_iter()
            .find(|p| p.first() == Some(&SNAPSHOT_PACKET))
            .unwrap(),
    )
    .unwrap();
    let acked_bot = state.lock().unwrap().clients[&observer]
        .snapshot_history
        .back()
        .unwrap()
        .entities[&bot_id];
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&observer)
        .unwrap()
        .last_acked_snapshot_id = Some(first.header.snapshot_id);

    // Ample budget: the moved bot is a real delta.
    let unheld = frame_at(1, 1_000_000.0).expect("delta frame");
    assert_ne!(bot_entry(&unheld).field_mask, 0);

    // One byte short: the bot is held at its acknowledged state instead.
    for step in [2, 3] {
        let held = frame_at(step, unheld.len() as f64 - 1.0).expect("held frame is sent");
        assert!(held.len() < unheld.len());
        assert_eq!(bot_entry(&held).field_mask, 0, "held bot carries no fields");
        let locked = state.lock().unwrap();
        let client = &locked.clients[&observer];
        assert_eq!(
            client.snapshot_history.back().unwrap().entities[&bot_id],
            acked_bot,
            "history records what the client still shows"
        );
        let shown = client
            .observer_presentation_history
            .frames()
            .next_back()
            .unwrap()
            .samples()
            .find(|sample| sample.actor.player_id() == bot_id)
            .unwrap();
        assert_eq!(
            (shown.x_q8, shown.sample_time),
            (i32::from(acked_bot.x) * 256, acked_bot.sample_time),
            "hits are judged against the held position the player sees"
        );
        assert_eq!(client.snapshot_holds.get(&bot_id), Some(&(step as u8 - 1)));
    }
    // Held twice in a row: it cannot be held again, so the frame is skipped.
    assert!(frame_at(4, unheld.len() as f64 - 1.0).is_none());
    // With budget it goes out as a real delta and the hold count resets.
    assert_ne!(bot_entry(&frame_at(5, 1_000_000.0).unwrap()).field_mask, 0);
    assert!(
        state.lock().unwrap().clients[&observer]
            .snapshot_holds
            .is_empty()
    );
}

#[test]
fn snapshot_emission_repeats_bot_motion_generation_until_the_new_base_is_acknowledged() {
    let state = Arc::new(Mutex::new(ServerState {
        config: ServerConfig {
            bot_count: 1,
            ..ServerConfig::default()
        },
        ..ServerState::default()
    }));
    state.lock().unwrap().initialize_configured_bots();
    let (tx, rx) = mpsc::channel();
    let observer = register_connection(&state, tx);
    join_arena(&state, observer, 8);
    drain_available(&rx);
    let bot_id = crate::state::SERVER_BOT_ID_BASE;
    let start = 1_000;
    {
        let mut locked = state.lock().unwrap();
        let center = server_spawn_coordinates(locked.zone_map());
        let bot = &mut locked.bots[0];
        bot.x = f32::from(center.0 + 64);
        bot.y = f32::from(center.1);
        bot.actor_motion_history = Default::default();
        bot.begin_actor_motion_segment();
        bot.append_actor_motion_sample(start, false);
        locked.arena_tick = 2;
    }
    emit_snapshots_for_tick_at_tick(&state, start);
    let first_packet = rx
        .try_iter()
        .find(|p| p.first() == Some(&SNAPSHOT_PACKET))
        .unwrap();
    let first = SnapshotFrame::parse(&first_packet).unwrap();
    let initial = first
        .near_entities
        .iter()
        .find(|entity| entity.entity_id == bot_id)
        .unwrap();
    assert_eq!(initial.motion_segment, Some(1));
    assert_eq!(initial.relocation_generation, Some(1));
    let initial_x = initial.x.unwrap();
    {
        let mut locked = state.lock().unwrap();
        locked
            .clients
            .get_mut(&observer)
            .unwrap()
            .last_acked_snapshot_id = Some(first.header.snapshot_id);
        let bot = &mut locked.bots[0];
        bot.x += 30.0;
        bot.begin_actor_motion_segment();
        bot.append_actor_motion_sample(start + 1, false);
    }
    for step in [1, 2, 3] {
        if step == 3 {
            let mut locked = state.lock().unwrap();
            let bot = &mut locked.bots[0];
            bot.velocity_x = 500.0;
            bot.begin_actor_impulse_segment();
            bot.append_actor_motion_sample(start + step, false);
        }
        state.lock().unwrap().arena_tick = 2 + u64::from(step) * 2;
        emit_snapshots_for_tick_at_tick(&state, start + step);
        // The first update is deliberately not acknowledged. The next one
        // must repeat the relocation against the original acknowledged base.
        let packet = rx
            .try_iter()
            .find(|p| p.first() == Some(&SNAPSHOT_PACKET))
            .unwrap();
        let frame = SnapshotFrame::parse(&packet).unwrap();
        assert_eq!(frame.header.base_id, first.header.snapshot_id);
        let moved = frame
            .near_entities
            .iter()
            .find(|entity| entity.entity_id == bot_id)
            .unwrap();
        assert_eq!(moved.motion_segment, Some(if step == 3 { 3 } else { 2 }));
        assert_eq!(
            moved.relocation_generation,
            Some(2),
            "a later impulse cannot hide the unacknowledged relocation"
        );
        assert_eq!(moved.x, Some(initial_x + 30));
        assert_eq!(
            moved.sample_time,
            Some(crate::combat_timeline::PresentationSampleTime::from_tick(
                start + if step == 3 { 3 } else { 1 }
            ))
        );
    }
}

#[test]
fn own_snapshots_report_inputs_applied_through_once_a_position_is_accepted() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let input_applied = |frame: &SnapshotFrame| {
        frame.events.iter().find_map(|event| match event {
            SnapshotEvent::InputApplied(applied) => Some(*applied),
            _ => None,
        })
    };

    // Nothing accepted yet this life: no acknowledgement, so the client keeps
    // sending whole windows.
    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.life_seq = 9;
        client.last_position_sequence = None;
        // The newest admitted input sits above a gap and must not leak in.
        client.last_input_seq = Some(40);
    }
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 2);
    let frame = SnapshotFrame::parse(&rx.try_recv().expect("snapshot")).unwrap();
    assert_eq!(input_applied(&frame), None);

    state
        .lock()
        .expect("state")
        .clients
        .get_mut(&player_id)
        .expect("client")
        .last_position_sequence = Some(37);
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 3);
    let frame = SnapshotFrame::parse(&rx.try_recv().expect("snapshot")).unwrap();
    assert_eq!(
        input_applied(&frame),
        Some(crate::snapshot_wire::SnapshotInputApplied {
            life_seq: 9,
            through_sequence: 37,
        })
    );
}

#[test]
fn only_clients_that_declare_the_revision_receive_compact_sample_times() {
    for (revision, compact) in [(0, false), (1, true), (2, true)] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        drain_available(&rx);
        state
            .lock()
            .expect("state")
            .clients
            .get_mut(&player_id)
            .expect("client")
            .protocol_revision = revision;
        crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 2);
        let packet = rx.try_recv().expect("snapshot");
        let frame = SnapshotFrame::parse(&packet).unwrap();
        assert_eq!(
            frame.header.flags & crate::snapshot_wire::SNAPSHOT_FLAG_COMPACT_SAMPLE_TIME != 0,
            compact,
            "revision {revision}"
        );
    }
}

#[test]
fn position_offset_clients_get_one_byte_moves_in_delta_frames() {
    let state = Arc::new(Mutex::new(ServerState {
        config: ServerConfig {
            bot_count: 1,
            ..ServerConfig::default()
        },
        ..ServerState::default()
    }));
    state.lock().unwrap().initialize_configured_bots();
    let (tx, rx) = mpsc::channel();
    let observer = register_connection(&state, tx);
    join_arena(&state, observer, 8);
    drain_available(&rx);
    let bot_id = crate::state::SERVER_BOT_ID_BASE;
    let start = 1_000;
    {
        let mut locked = state.lock().unwrap();
        locked.clients.get_mut(&observer).unwrap().protocol_revision =
            crate::snapshot_wire::CLIENT_PROTOCOL_POSITION_OFFSETS;
        let center = server_spawn_coordinates(locked.zone_map());
        let bot = &mut locked.bots[0];
        bot.x = f32::from(center.0 + 64);
        bot.y = f32::from(center.1);
        bot.actor_motion_history = Default::default();
        bot.begin_actor_motion_segment();
        bot.append_actor_motion_sample(start, false);
        locked.arena_tick = 2;
    }
    let bot_entry = |packet: &[u8]| {
        SnapshotFrame::parse(packet)
            .unwrap()
            .near_entities
            .into_iter()
            .find(|entity| entity.entity_id == bot_id)
            .unwrap()
    };
    emit_snapshots_for_tick_at_tick(&state, start);
    let first = rx
        .try_iter()
        .find(|p| p.first() == Some(&SNAPSHOT_PACKET))
        .unwrap();
    let full = bot_entry(&first);
    assert_eq!(
        full.field_mask & SnapshotEntity::FIELD_POSITION_OFFSET,
        0,
        "full frames are absolute"
    );
    {
        let mut locked = state.lock().unwrap();
        locked
            .clients
            .get_mut(&observer)
            .unwrap()
            .last_acked_snapshot_id =
            Some(SnapshotFrame::parse(&first).unwrap().header.snapshot_id);
        let bot = &mut locked.bots[0];
        bot.x += 30.0;
        bot.append_actor_motion_sample(start + 1, false);
        locked.arena_tick = 4;
    }
    emit_snapshots_for_tick_at_tick(&state, start + 1);
    let moved = bot_entry(
        &rx.try_iter()
            .find(|p| p.first() == Some(&SNAPSHOT_PACKET))
            .unwrap(),
    );
    assert_ne!(moved.field_mask & SnapshotEntity::FIELD_POSITION_OFFSET, 0);
    assert_eq!(moved.x, None);
    assert_eq!(moved.x_offset, Some(30));
}

#[test]
fn compact_private_clients_get_prize_references_until_the_body_changes() {
    use crate::snapshot_wire::SnapshotPrizeReference;
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&player_id)
        .unwrap()
        .protocol_revision = crate::snapshot_wire::CLIENT_PROTOCOL_COMPACT_PRIVATE;
    let mut tick = 2;
    let mut frame = |acknowledge: bool, change: &dyn Fn(&mut crate::state::ClientHandle)| {
        {
            let mut locked = state.lock().unwrap();
            let client = locked.clients.get_mut(&player_id).unwrap();
            change(client);
            if acknowledge {
                client.last_acked_snapshot_id =
                    client.snapshot_history.back().map(|e| e.snapshot_id);
            }
        }
        crate::snapshot::emit_snapshots_for_tick_at_tick(&state, tick);
        tick += 1;
        SnapshotFrame::parse(&rx.try_recv().expect("snapshot")).unwrap()
    };
    let prize = |frame: &SnapshotFrame| {
        frame.events.iter().find_map(|event| match event {
            SnapshotEvent::PrizeState(prize) => Some(*prize),
            _ => None,
        })
    };
    let reference = |frame: &SnapshotFrame| {
        frame.events.iter().find_map(|event| match event {
            SnapshotEvent::PrizeReference(reference) => Some(*reference),
            _ => None,
        })
    };
    let unchanged = |_: &mut crate::state::ClientHandle| {};

    // A full frame always carries the body.
    let first = frame(false, &unchanged);
    assert!(first.header.flags & crate::snapshot_wire::SNAPSHOT_FLAG_COMPACT_PRIVATE != 0);
    let body = prize(&first).expect("full frames carry the prize body");
    assert_eq!(reference(&first), None);

    // Once acknowledged, only a reference with the current energy travels.
    let second = frame(true, &|client| client.server_energy = 900);
    assert_eq!(prize(&second), None);
    assert_eq!(
        reference(&second),
        Some(SnapshotPrizeReference {
            snapshot_id: first.header.snapshot_id,
            energy: 900,
        })
    );
    assert!(body.energy.is_some());

    // Another field changes: the body travels until a frame carrying it is
    // acknowledged.
    let bounty = body.bounty + 5;
    let changed = frame(false, &|client| client.server_bounty = bounty);
    assert_eq!(prize(&changed).map(|prize| prize.bounty), Some(bounty));
    let repeated = frame(false, &unchanged);
    assert_eq!(prize(&repeated).map(|prize| prize.bounty), Some(bounty));
    let after_ack = frame(true, &unchanged);
    assert_eq!(prize(&after_ack), None);
    assert_eq!(
        reference(&after_ack).map(|reference| reference.snapshot_id),
        Some(repeated.header.snapshot_id)
    );

    // The body is refreshed at least every PRIZE_BODY_REFRESH_SNAPSHOTS.
    let mut refreshed = false;
    for _ in 0..crate::snapshot::PRIZE_BODY_REFRESH_SNAPSHOTS {
        let next = frame(true, &unchanged);
        if prize(&next).is_some() {
            refreshed = true;
            break;
        }
    }
    assert!(refreshed);
}

#[test]
fn private_rocket_state_repeats_across_loss_and_expires_at_absolute_u31_deadline() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    {
        let mut locked = state.lock().expect("state");
        locked.arena_tick = 2;
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.life_seq = 9;
        client.last_input_seq = Some(u16::MAX);
        client
            .recent_input_rocket_assertions
            .push_back((u16::MAX, true));
        client.server_weapons.rockets = 2;
        client.server_rocket_until_tick = Some(2);
        client
            .server_rocket_authorizations
            .push_back(crate::state::ServerRocketAuthorization {
                start_sequence: u16::MAX,
                start_tick: crate::continuum::CONTINUUM_TICK_MASK - 1,
                deadline_tick: 2,
                lifetime_ticks: 3,
            });
    }

    crate::snapshot::emit_snapshots_for_tick_at_tick(
        &state,
        crate::continuum::CONTINUUM_TICK_MASK - 1,
    );
    let lost = SnapshotFrame::parse(&rx.try_recv().expect("first active snapshot")).unwrap();
    let first = lost
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::RocketState(state) => Some(*state),
            _ => None,
        })
        .expect("private Rocket state");
    assert!(first.active);
    assert_eq!(first.observed_input_sequence, Some(u16::MAX));
    assert_eq!(first.input_sequence_watermark, Some(u16::MAX));
    assert_eq!(first.activation_sequence, Some(u16::MAX));
    assert_eq!(first.until_tick, Some(2));
    assert_eq!(first.rockets, 2);

    // Treat the first packet as lost. The next snapshot repeats the complete
    // state rather than relying on a one-shot activation receipt.
    state.lock().expect("state").arena_tick = 3;
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 0);
    let repeated = SnapshotFrame::parse(&rx.try_recv().expect("repeated active snapshot")).unwrap();
    assert!(repeated.header.snapshot_id != lost.header.snapshot_id);
    assert!(repeated.events.contains(&SnapshotEvent::RocketState(first)));

    // At the absolute wrapped deadline, a later advancing snapshot is
    // authoritative inactive; replaying either older frame cannot advance a
    // conforming client's snapshot id.
    state.lock().expect("state").arena_tick = 4;
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 2);
    let expired = SnapshotFrame::parse(&rx.try_recv().expect("expired snapshot")).unwrap();
    let expired_state = expired
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::RocketState(state) => Some(*state),
            _ => None,
        })
        .expect("private Rocket state");
    assert!(!expired_state.active);
    assert_eq!(expired_state.until_tick, Some(2));
    assert_eq!(
        state.lock().expect("state").clients[&player_id].last_input_seq,
        Some(u16::MAX),
        "wall expiry must publish inactive even when no newer input arrives",
    );
    assert!(crate::snapshot::snapshot_id_is_forward(
        expired.header.snapshot_id,
        repeated.header.snapshot_id
    ));
    assert!(!crate::snapshot::snapshot_id_is_forward(
        lost.header.snapshot_id,
        expired.header.snapshot_id
    ));
}

#[test]
fn private_prize_state_repeats_exact_loadout_revision_and_absolute_deadlines() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let tick = crate::continuum::CONTINUUM_TICK_MASK - 9;
    let (life_seq, revision, expected_deadline, emp_deadline) = {
        let mut locked = state.lock().expect("state");
        locked.arena_tick = 2;
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.apply_server_prize_effect_at_tick(21, tick);
        client.apply_server_prize_effect_at_tick(-14, tick);
        let prize_deadline = client
            .server_engine_shutdown_until_tick
            .expect("prize shutdown");
        client.server_engine_shutdown_start_sequence = Some(10);
        client.server_emp_until_tick =
            Some(crate::continuum::continuum_tick_add(prize_deadline, 25));
        (
            client.life_seq,
            client.server_prize_state_revision,
            Some(prize_deadline),
            client.server_emp_until_tick,
        )
    };
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, tick);
    let frame = SnapshotFrame::parse(&rx.try_recv().expect("snapshot")).expect("frame");
    let prize = frame
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::PrizeState(prize) => Some(*prize),
            _ => None,
        })
        .expect("private prize state");
    assert_eq!(prize.life_seq, life_seq);
    assert_eq!(prize.revision, revision);
    assert_eq!(
        prize.repels,
        state.lock().expect("state").clients[&player_id]
            .server_weapons
            .repels
    );
    assert_eq!(
        prize.portals,
        state.lock().expect("state").clients[&player_id]
            .server_weapons
            .portals
    );
    assert_eq!(prize.engine_shutdown_until_tick, expected_deadline);
    assert_eq!(prize.emp_until_tick, emp_deadline);

    // Treat the first frame as lost: the next one repeats the same authority
    // revision while recomputing only timer activity from the absolute clock.
    state.lock().expect("state").arena_tick = 3;
    crate::snapshot::emit_snapshots_for_tick_at_tick(
        &state,
        crate::continuum::continuum_tick_add(tick, 1),
    );
    let repeated = SnapshotFrame::parse(&rx.try_recv().expect("repeat")).expect("frame");
    assert!(repeated.events.contains(&SnapshotEvent::PrizeState(prize)));

    state.lock().expect("state").arena_tick = 4;
    crate::snapshot::emit_snapshots_for_tick_at_tick(
        &state,
        expected_deadline.expect("shutdown deadline"),
    );
    let expired = SnapshotFrame::parse(&rx.try_recv().expect("expired")).expect("frame");
    let expired = expired
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::PrizeState(prize) => Some(*prize),
            _ => None,
        })
        .expect("private prize state");
    assert_eq!(expired.revision, revision);
    assert_eq!(expired.engine_shutdown_until_tick, None);
    assert_eq!(
        expired.emp_until_tick, emp_deadline,
        "prize expiry cannot erase the later EMP"
    );
    state.lock().expect("state").arena_tick = 5;
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, emp_deadline.unwrap());
    let frame = SnapshotFrame::parse(&rx.try_recv().expect("EMP expiry")).unwrap();
    let fully_expired = frame
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::PrizeState(prize) => Some(prize),
            _ => None,
        })
        .unwrap();
    assert_eq!(fully_expired.revision, revision);
    assert_eq!(fully_expired.engine_shutdown_until_tick, None);
    assert_eq!(fully_expired.emp_until_tick, None);
}

#[test]
fn private_prize_state_publishes_emp_separately_without_movement_shutdown_or_revision_overwrite() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let tick = crate::continuum::CONTINUUM_TICK_MASK - 4;
    let deadline = crate::continuum::continuum_tick_add(tick, 90);
    let revision = {
        let mut locked = state.lock().expect("state");
        locked.arena_tick = 2;
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.server_engine_shutdown_until_tick = None;
        client.server_emp_until_tick = Some(deadline);
        client.server_prize_state_revision
    };
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, tick);
    let frame = SnapshotFrame::parse(&rx.try_recv().expect("snapshot")).expect("frame");
    let prize = frame
        .events
        .into_iter()
        .find_map(|event| match event {
            SnapshotEvent::PrizeState(prize) => Some(prize),
            _ => None,
        })
        .expect("private prize state");
    assert_eq!(prize.revision, revision);
    assert_eq!(prize.engine_shutdown_until_tick, None);
    assert_eq!(prize.emp_until_tick, Some(deadline));
}

#[test]
fn pending_shutdown_survives_a_newer_unrelated_private_revision() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let tick = 5_000;
    let (deadline, revision) = {
        let mut locked = state.lock().expect("state");
        locked.arena_tick = 2;
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.replace_server_engine_shutdown_at_tick(700, tick);
        let shutdown_revision = client.server_prize_state_revision;
        client.pending_prize_effect_applications.push_back(
            crate::state::PendingPrizeEffectApplication {
                collectible_id: 77,
                prize_id: 14,
                life_seq: client.life_seq,
                prize_state_revision: shutdown_revision,
                minimum_application_sequence: Some(10),
                maximum_application_sequence: Some(10),
                latest_application_sequence: Some(10),
                deferred_application: None,
                movement_tuning_before_bits: None,
            },
        );
        client.apply_server_prize_effect_at_tick(21, tick);
        (
            client.server_engine_shutdown_until_tick,
            client.server_prize_state_revision,
        )
    };
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, tick);
    let frame = SnapshotFrame::parse(&rx.try_recv().expect("snapshot")).expect("frame");
    let prize = frame
        .events
        .into_iter()
        .find_map(|event| match event {
            SnapshotEvent::PrizeState(prize) => Some(prize),
            _ => None,
        })
        .expect("private prize state");
    assert_eq!(prize.revision, revision);
    assert_eq!(prize.engine_shutdown_until_tick, deadline);
    assert!(deadline.is_some());
    assert_ne!(prize.flags & (1 << 11), 0);
    assert!(
        client_snapshot_shutdown_is_pending(prize.flags),
        "the newer aggregate revision carries continuity without pre-ACK activation"
    );
}

fn client_snapshot_shutdown_is_pending(flags: u16) -> bool {
    flags & (1 << 11) != 0
}

#[test]
fn private_rocket_receipt_ignores_overtaking_false_until_raw_assertion_arrives() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    {
        let mut locked = state.lock().expect("state");
        locked.arena_tick = 2;
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.life_seq = 3;
        client.last_input_seq = Some(101);
        client
            .recent_input_rocket_assertions
            .push_back((101, false));
    }
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 1_000);
    let before_assertion = SnapshotFrame::parse(&rx.try_recv().expect("false-only snapshot"))
        .expect("snapshot parses");
    let receipt = before_assertion
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::RocketState(state) => Some(*state),
            _ => None,
        })
        .expect("private Rocket state");
    assert_eq!(receipt.observed_input_sequence, None);
    assert_eq!(receipt.input_sequence_watermark, Some(101));

    state
        .lock()
        .expect("state")
        .clients
        .get_mut(&player_id)
        .expect("client")
        .recent_input_rocket_assertions
        .push_front((100, true));
    state.lock().expect("state").arena_tick = 4;
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 1_002);
    let after_assertion =
        SnapshotFrame::parse(&rx.try_recv().expect("assertion snapshot")).expect("snapshot parses");
    let receipt = after_assertion
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::RocketState(state) => Some(*state),
            _ => None,
        })
        .expect("private Rocket state");
    assert_eq!(receipt.observed_input_sequence, Some(100));
    assert_eq!(receipt.input_sequence_watermark, Some(101));
}

#[test]
fn wholly_lost_rocket_assertion_reports_only_safe_advancing_watermark_across_wrap() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let identity = {
        let mut locked = state.lock().expect("state");
        locked
            .clients
            .get_mut(&player_id)
            .unwrap()
            .server_weapons
            .rockets = 3;
        locked.active_player_runtime_identity(player_id).unwrap()
    };
    let lost_assertion = u16::MAX - 31;
    let send_false = |sequence: u16| {
        let mut packet = vec![crate::connection::CLIENT_INPUT_RECORD_TYPE];
        packet.extend_from_slice(&sequence.to_le_bytes());
        packet.push(1);
        packet.extend_from_slice(&0_u16.to_le_bytes());
        packet.extend_from_slice(&[0, 0]);
        packet.extend_from_slice(&unavailable_combat_view_receipt(sequence));
        assert!(crate::connection::record_client_input_observation_at_tick(
            &state,
            identity,
            &packet,
            40_000_u32.wrapping_add(u32::from(sequence.wrapping_sub(lost_assertion))),
        ));
    };
    for distance in 1..=63_u16 {
        send_false(lost_assertion.wrapping_add(distance));
    }
    state.lock().expect("state").arena_tick = 2;
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 40_063);
    let before_safe = SnapshotFrame::parse(&rx.try_recv().expect("distance-63 snapshot")).unwrap();
    let before_safe = before_safe
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::RocketState(rocket) => Some(*rocket),
            _ => None,
        })
        .unwrap();
    assert_eq!(before_safe.observed_input_sequence, None);
    assert_eq!(
        before_safe.input_sequence_watermark,
        Some(lost_assertion.wrapping_add(63))
    );
    assert_eq!(before_safe.rockets, 3);
    assert!(!before_safe.active);
    assert_eq!(before_safe.until_tick, None);

    send_false(lost_assertion.wrapping_add(64));
    send_false(lost_assertion.wrapping_add(65));
    state.lock().expect("state").arena_tick = 4;
    crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 40_065);
    let after_safe = SnapshotFrame::parse(&rx.try_recv().expect("distance-65 snapshot")).unwrap();
    let after_safe = after_safe
        .events
        .iter()
        .find_map(|event| match event {
            SnapshotEvent::RocketState(rocket) => Some(*rocket),
            _ => None,
        })
        .unwrap();
    assert_eq!(after_safe.observed_input_sequence, None);
    assert_eq!(
        after_safe.input_sequence_watermark,
        Some(lost_assertion.wrapping_add(65))
    );
    assert_eq!(after_safe.rockets, 3);
    assert!(!after_safe.active);
    assert_eq!(after_safe.until_tick, None);
    let locked = state.lock().expect("state");
    let client = &locked.clients[&player_id];
    assert_eq!(
        client.recent_input_records.len(),
        crate::state::MAX_RECENT_INPUT_OBSERVATIONS
    );
    assert_eq!(client.server_weapons.rockets, 3);
    assert_eq!(client.server_rocket_until_tick, None);
    assert!(client.server_rocket_authorizations.is_empty());
}

#[test]
fn repel_observation_records_only_actual_snapshot_delivery_before_ack() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (source_tx, source_rx) = mpsc::channel();
    let (target_tx, target_rx) = mpsc::channel();
    let source = register_connection(&state, source_tx);
    let target = register_connection(&state, target_tx);
    join_arena(&state, source, 0);
    join_arena(&state, target, 0);
    drain_available(&source_rx);
    drain_available(&target_rx);
    let repel_event_tick = crate::wire::current_tick();
    let repel_expires_at_tick = crate::continuum::continuum_tick_add(
        repel_event_tick,
        crate::arena_settings::arena_repel_lifetime_ticks(),
    );

    let mut source_position = large_position_from_client(
        source,
        &client_position_packet_with_sequence(0, 500, 700, 10),
    )
    .unwrap();
    source_position[19..21].copy_from_slice(&5_u16.to_le_bytes());
    let target_position = large_position_from_client(
        target,
        &client_position_packet_with_sequence(0, 600, 700, 20),
    )
    .unwrap();
    let source_runtime = {
        let mut state = state.lock().unwrap();
        state.set_client_position(source, source_position);
        seed_snapshot_motion(state.clients.get_mut(&source).unwrap());
        state.set_client_position(target, target_position);
        seed_snapshot_motion(state.clients.get_mut(&target).unwrap());
        let source_runtime = state.active_player_runtime_identity(source).unwrap();
        state
            .clients
            .get_mut(&source)
            .unwrap()
            .pending_snapshot_weapon = 5;
        let target_client = state.clients.get_mut(&target).unwrap();
        target_client.player.frequency = 1;
        target_client
            .pending_repel_observations
            .push_back(crate::state::ServerRepelObservation {
                source: source_runtime,
                event_tick: repel_event_tick,
                expires_at_tick: repel_expires_at_tick,
                origin_x: 500.0,
                origin_y: 700.0,
                expected_velocity_x: 500.0,
                expected_velocity_y: 0.0,
                weapon_data: 5,
                source_frequency: 0,
                source_ship: 0,
                delivery_generation: 1,
                active_steps: 135,
                delivered_snapshots: [None; crate::state::MAX_REPEL_DELIVERY_SNAPSHOTS],
                delivered_snapshot_cursor: 0,
                accepted_application: None,
                delivered_snapshot_id: None,
                delivered_snapshot_tick: None,
                delivered_active_steps: None,
                delivery_acked: false,
            });
        target_client.server_sim_state = Some(crate::state::ServerSimMovement {
            position_x: 600.25,
            position_y: 700.25,
            orientation: 0.0,
            velocity_x: 10.0,
            velocity_y: 0.0,
            energy: target_client.server_energy as f32,
            max_energy: target_client.server_max_energy as f32,
        });
        target_client.server_sim_anchor_tick = Some(repel_event_tick);
        target_client.movement_drift_x = 9.0;
        target_client.movement_drift_y = -4.0;
        target_client.snapshot_seq = u16::MAX - 2;
        target_client.simulated_tx_loss =
            crate::simulated_loss::SimulatedTxLossSettings::new(1_000_000, Some(35)).controller();
        state.arena_tick = 2;
        source_runtime
    };

    emit_snapshots_for_tick_inner(&state, true);
    let _source_packet = source_rx.try_recv().expect("source snapshot");
    assert!(
        target_rx.try_recv().is_err(),
        "the first exact weapon-bearing target frame is deliberately lost"
    );
    {
        let mut state = state.lock().unwrap();
        assert_eq!(
            state.clients[&target]
                .pending_repel_observations
                .front()
                .copied()
                .unwrap()
                .delivered_snapshot_id,
            None,
            "a transport drop cannot fabricate the application identity"
        );
        assert_eq!(
            state.clients[&target].observer_presentation_history.len(),
            0,
            "simulated loss cannot fabricate an emitted presentation frame"
        );
        assert_eq!(
            state.clients[&source].pending_snapshot_weapon, 0,
            "ordinary one-shot public weapon state still retires after its emission tick"
        );
        assert_eq!(
            state.clients[&target]
                .threat_presentation
                .readiness(source_runtime, repel_event_tick)
                .reason,
            crate::threat_presentation::ThreatReadinessReason::SourceLifeNotEmitted,
            "a simulated transport drop cannot fabricate exact-source emission"
        );
        state.clients.get_mut(&target).unwrap().simulated_tx_loss =
            crate::simulated_loss::SimulatedTxLoss::disabled();
        // Retry must use the immutable event, not the source's mutable
        // roster/AOI state. Move the source far away, change its life and
        // frequency, then disconnect it before the successful retry.
        let mut moved_source = large_position_from_client(
            source,
            &client_position_packet_with_sequence(0, 2_000, 2_000, 11),
        )
        .unwrap();
        moved_source[19..21].copy_from_slice(&0_u16.to_le_bytes());
        state.set_client_position(source, moved_source);
        seed_snapshot_motion(state.clients.get_mut(&source).unwrap());
        {
            let source_client = state.clients.get_mut(&source).unwrap();
            source_client.life_seq = source_client.life_seq.wrapping_add(1).max(1);
            source_client.player.frequency = 9;
            source_client.pending_snapshot_weapon = 0;
        }
        state.remove_client(source).expect("source disconnects");
        state.next_player_id = source;
        state.arena_tick = 3;
    }
    drain_available(&source_rx);
    let (replacement_tx, replacement_rx) = mpsc::channel();
    let replacement = register_connection(&state, replacement_tx);
    assert_eq!(
        replacement, source,
        "the numeric source id is deliberately reused"
    );
    join_arena(&state, replacement, 0);
    drain_available(&replacement_rx);
    drain_available(&target_rx);
    {
        let mut state = state.lock().unwrap();
        let mut replacement_position = large_position_from_client(
            replacement,
            &client_position_packet_with_sequence(0, 2_500, 2_500, 50),
        )
        .unwrap();
        replacement_position[19..21].copy_from_slice(&0_u16.to_le_bytes());
        state.set_client_position(replacement, replacement_position);
        seed_snapshot_motion(state.clients.get_mut(&replacement).unwrap());
        state
            .clients
            .get_mut(&replacement)
            .unwrap()
            .player
            .frequency = 9;
    }
    emit_snapshots_for_tick_inner(&state, true);
    assert!(
        source_rx.try_recv().is_err(),
        "a disconnected source receives no retry traffic"
    );
    let _replacement_snapshot = replacement_rx
        .try_recv()
        .expect("replacement source receives its own ordinary snapshot");
    let target_packet = target_rx.try_recv().expect("target snapshot");
    let delivered_snapshot_id = u16::from_le_bytes([target_packet[2], target_packet[3]]);
    let target_frame = SnapshotFrame::parse(&target_packet).expect("target snapshot parses");
    let expected_active_steps = 135;
    assert!(
        target_frame
            .near_entities
            .iter()
            .all(|entity| entity.entity_id != source),
        "private authority must not alias the replacement source entity: {:?}",
        target_frame.near_entities
    );
    assert!(
        target_frame.events.iter().any(|event| matches!(
            event,
            SnapshotEvent::LifecycleState(state)
                if state.life_seq == 1
                    && state.alive
                    && state.repel_until_tick == Some(repel_expires_at_tick)
                    && state.repel_delivery.is_some_and(|delivery| {
                        delivery.active_steps == expected_active_steps
                            && delivery.delivery_generation == 1
                            && delivery.source_player_id == source
                            && delivery.source_life_seq == 1
                            && delivery.origin_x == 500
                            && delivery.origin_y == 700
                            && delivery.source_frequency == 0
                            && delivery.source_ship == 0
                            && delivery.weapon_data == 5
                            && f32::from_bits(delivery.expected_velocity_x_bits) == 500.0
                            && f32::from_bits(delivery.expected_velocity_y_bits) == 0.0
                    })
        )),
        "the weapon-bearing frame carries the original private Repel deadline before its ACK: {:?}",
        target_frame.events
    );
    {
        let state = state.lock().unwrap();
        let target_client = &state.clients[&target];
        assert_eq!(
            target_client
                .pending_repel_observations
                .front()
                .copied()
                .unwrap()
                .delivered_snapshot_id,
            Some(delivered_snapshot_id)
        );
        assert_eq!(
            target_client
                .pending_repel_observations
                .front()
                .copied()
                .unwrap()
                .delivered_snapshot_tick,
            Some(target_frame.header.tick),
            "canonical lifetime is anchored to the exact weapon-bearing frame tick"
        );
        assert_eq!(
            target_client
                .pending_repel_observations
                .front()
                .copied()
                .unwrap()
                .delivered_active_steps,
            Some(expected_active_steps),
            "the delivery witness retains the exact logical duration encoded for the browser"
        );
        assert_eq!(target_client.last_acked_snapshot_id, None);
        assert!(
            !target_client
                .pending_repel_observations
                .front()
                .copied()
                .unwrap()
                .delivery_acked
        );
    }

    {
        state.lock().unwrap().arena_tick = 4;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let _replacement_followup = replacement_rx
        .try_recv()
        .expect("replacement follow-up snapshot");
    let target_followup = target_rx.try_recv().expect("target follow-up snapshot");
    let followup_frame = SnapshotFrame::parse(&target_followup).expect("follow-up parses");
    assert!(
        followup_frame.events.iter().any(|event| matches!(
            event,
            SnapshotEvent::LifecycleState(state)
                if state.life_seq == 1
                    && state.alive
                    && state.repel_until_tick == Some(repel_expires_at_tick)
                    && state.repel_delivery.is_some()
        )),
        "the private Repel delivery remains paired with its weapon until application admission"
    );
    assert!(
        followup_frame.near_entities.iter().all(|entity| {
            !entity
                .weapon
                .is_some_and(|weapon| crate::continuum_weapon::continuum_weapon_type(weapon) == 5)
        }),
        "private Repel retry authority must remain isolated from public entity presentation"
    );

    // A later unordered snapshot ACK is not evidence that the unreliable
    // weapon-bearing frame arrived.
    crate::connection::record_snapshot_ack_at(
        &state,
        target,
        &snapshot_ack_payload(2),
        std::time::Instant::now(),
    );
    assert!(
        !state.lock().unwrap().clients[&target]
            .pending_repel_observations
            .front()
            .copied()
            .unwrap()
            .delivery_acked
    );
    assert_eq!(
        state.lock().unwrap().clients[&target].server_repel_until_tick,
        None,
        "a newer unrelated ACK must not arm the repel environment"
    );
    assert_eq!(
        state.lock().unwrap().clients[&target].server_repel_observer_until_tick,
        None,
        "an unrelated ACK must not arm observer-only delivery grace"
    );
    assert!(
        state.lock().unwrap().clients[&target]
            .server_sim_state
            .is_some()
    );
    // The exact ACK may itself arrive after that newer ACK. It still proves
    // the weapon frame was decoded without regressing the active delta base.
    crate::connection::record_snapshot_ack_at(
        &state,
        target,
        &snapshot_ack_payload(delivered_snapshot_id),
        std::time::Instant::now(),
    );
    assert_eq!(
        state.lock().unwrap().clients[&target].last_acked_snapshot_id,
        Some(2)
    );
    assert!(
        state.lock().unwrap().clients[&target]
            .pending_repel_observations
            .front()
            .copied()
            .unwrap()
            .delivery_acked,
        "the exact transport ACK is timing evidence but not application admission"
    );
    assert_eq!(
        state.lock().unwrap().clients[&target].server_repel_until_tick,
        None,
        "transport ACK still cannot arm canonical movement before the 0x79 application boundary"
    );
    assert_eq!(
        state.lock().unwrap().clients[&target].server_repel_observer_until_tick,
        None,
        "snapshot ACKs cannot arm any application environment"
    );
    assert!(
        !state.lock().unwrap().clients[&target]
            .pending_repel_observations
            .is_empty()
    );

    // A transport ACK never authenticates application. At the immutable
    // deadline an unadmitted event is cancelled and cannot be re-offered.
    drain_available(&source_rx);
    drain_available(&target_rx);
    state.lock().unwrap().arena_tick = 6;
    emit_snapshots_for_tick_at_tick(&state, repel_expires_at_tick);
    assert!(source_rx.try_recv().is_err());
    let _replacement_expired = replacement_rx
        .try_recv()
        .expect("replacement expiry snapshot");
    let target_expired = target_rx.try_recv().expect("target expiry snapshot");
    let expired_frame = SnapshotFrame::parse(&target_expired).expect("expiry snapshot parses");
    assert!(
        expired_frame.events.iter().all(|event| !matches!(
            event,
            SnapshotEvent::LifecycleState(state) if state.repel_delivery.is_some()
        )),
        "an expired retained event cannot be offered again: {:?}",
        expired_frame.events
    );
    assert!(
        expired_frame.near_entities.iter().all(|entity| {
            !entity
                .weapon
                .is_some_and(|weapon| crate::continuum_weapon::continuum_weapon_type(weapon) == 5)
        }),
        "an expired retained event cannot replay its immutable weapon"
    );
    assert_eq!(
        state.lock().unwrap().clients[&target]
            .pending_repel_observations
            .len(),
        0,
        "the deadline cancels an event whose first application marker never arrived"
    );
}

#[test]
fn server_does_not_emit_when_flag_off() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, false);

    assert!(
        rx.try_recv().is_err(),
        "no snapshot should be emitted when flag is off"
    );
}

#[test]
fn server_emits_near_tier_every_tick_at_60hz() {
    // Phase A: at 60 Hz the near tier emits every tick (interval 1) while
    // mid rides every 2nd tick. So an odd tick still emits a (near-only)
    // frame — the old "off-interval ticks don't emit" behavior is gone.
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.config.tick_rate_hz = 60;
        state_guard.arena_tick = 1;
    }
    emit_snapshots_for_tick_inner(&state, true);
    assert!(
        rx.try_recv().is_ok(),
        "near tier should emit on every tick at 60 Hz (odd tick = near-only frame)"
    );

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);
    assert!(rx.try_recv().is_ok(), "mid-interval tick should also emit");
}

#[test]
fn near_emits_every_tick_but_mid_only_on_mid_interval_at_60hz() {
    // Phase A crux: with beta in alpha's near AOI and gamma in mid, a
    // 60 Hz odd tick carries near but suppresses the peripheral mid tier;
    // the even (mid-interval) tick carries both.
    let (state, alpha, _beta, _gamma, rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    drain_available(&rx_a);
    {
        let mut g = state.lock().expect("server state should not be poisoned");
        g.config.tick_rate_hz = 60;
        g.arena_tick = 1;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let odd = rx_a.try_recv().expect("alpha near-only frame");
    assert_eq!(odd[0], SNAPSHOT_PACKET);
    assert!(
        odd[18] >= 1,
        "near tier present on the near-only (odd) tick"
    );
    assert_eq!(odd[19], 0, "mid tier suppressed on a near-only (odd) tick");

    {
        let mut g = state.lock().expect("server state should not be poisoned");
        g.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let even = rx_a.try_recv().expect("alpha mid-bearing frame");
    assert_eq!(
        even[19], 1,
        "mid tier present on the mid-interval (even) tick"
    );
    let _ = alpha;
}

#[test]
fn snapshot_id_allocation_skips_zero_on_wrap() {
    // Id zero remains unallocated as defense in depth even though FULL now
    // has an explicit header flag.
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    {
        let mut g = state.lock().expect("server state should not be poisoned");
        g.arena_tick = 2;
        g.clients
            .get_mut(&player_id)
            .expect("client present")
            .snapshot_seq = u16::MAX;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let packet = rx.try_recv().expect("snapshot frame");
    assert_eq!(packet[0], SNAPSHOT_PACKET);
    assert_eq!(
        u16::from_le_bytes([packet[2], packet[3]]),
        1,
        "id allocation must skip 0 across the u16 wrap"
    );
}

#[test]
fn saturated_realtime_lane_leaves_the_snapshot_event_queue_untouched() {
    // The dedicated `0x7d` realtime queue and the `0x70` snapshot event region are separate queues by
    // construction, and until now nothing said so: a change that gave them a shared budget would pass
    // every existing suite, because each lane was only ever tested against its own capacity. Saturate
    // the realtime recipient, then show that it refuses new reservations while the snapshot queue
    // neither shrinks nor stalls - it still drains its own pending event on its own frame.
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    let recipient = {
        let guard = state.lock().expect("server state should not be poisoned");
        guard
            .active_player_runtime_identity(player_id)
            .expect("joined player has a runtime identity")
    };
    let snapshot_event = crate::snapshot::PendingWatchDamage {
        victim_id: player_id,
        shooter_id: player_id,
        victim_life_seq: 0,
        shooter_life_seq: 0,
        weapon_data: 66,
        damage: 400,
        energy_before: 1_700,
        energy_after: 1_300,
        server_tick: 2,
        damage_seq: 7,
        projectile_id: 9,
        lethal: false,
    };
    let capacity = crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY;
    {
        let mut guard = state.lock().expect("server state should not be poisoned");
        guard.arena_tick = 2;
        let client = guard.clients.get_mut(&player_id).expect("client present");
        client.pending_snapshot_events.watch_damage.push_back(
            crate::snapshot::PendingWatchDamageEnvelope {
                recipient,
                event: snapshot_event,
            },
        );
        while client.pending_realtime_events.queue.len() < capacity {
            let filled = client.pending_realtime_events.queue.len();
            client.pending_realtime_events.queue.push_back(
                crate::snapshot::PendingRealtimeEventEnvelope {
                    flight_transition: false,
                    recipient,
                    event_id: filled as u32 + 1,
                    server_tick: 2,
                    payload: crate::realtime_event::RealtimeEventPayload::WatchDamage(
                        crate::realtime_event::RealtimeWatchDamageEvent {
                            victim_player_id: player_id,
                            victim_session_id: 1,
                            victim_life_seq: 0,
                            shooter_player_id: player_id,
                            shooter_session_id: 1,
                            shooter_life_seq: 0,
                            weapon_data: 66,
                            damage: 400,
                            energy_before: 1_700,
                            energy_after: 1_300,
                            projectile_id: 9,
                            damage_seq: filled as u32 + 1,
                            lethal: false,
                        },
                    ),
                },
            );
        }
    }

    let reservation = {
        let mut guard = state.lock().expect("server state should not be poisoned");
        crate::snapshot::reserve_realtime_event_deliveries(
            &mut guard,
            &[&[recipient]],
            crate::snapshot::RealtimeEventPressureDisposition::Deferred,
        )
    };
    assert!(
        reservation.is_none(),
        "a recipient already holding the full dedicated realtime queue must not reserve another slot"
    );
    {
        let guard = state.lock().expect("server state should not be poisoned");
        let client = guard.clients.get(&player_id).expect("client present");
        assert_eq!(
            client.pending_snapshot_events.watch_damage.len(),
            1,
            "a realtime reservation attempt may not consume the other lane's queue"
        );
        assert_eq!(client.pending_realtime_events.queue.len(), capacity);
    }

    {
        let mut guard = state.lock().expect("server state should not be poisoned");
        guard.arena_tick = 4;
        guard
            .clients
            .get_mut(&player_id)
            .expect("client present")
            .send_pacing
            .tokens = 1_000_000.0;
    }
    emit_snapshots_for_tick_inner(&state, true);
    // One pass of the emit path services both lanes: it flushes the dedicated realtime carrier and
    // builds the snapshot frame. The first packet delivered here was a `0x7d` realtime event, which is
    // the fact this test had wrong on its first run - the lanes share the emit pass, so what has to
    // hold is that each lane's own queue is bounded and drained by its own carrier, not that a
    // snapshot pass leaves the realtime queue alone.
    let mut saw_snapshot = false;
    let mut saw_realtime = false;
    for _ in 0..capacity + 4 {
        let Ok(packet) = rx.try_recv() else { break };
        match packet.first().copied() {
            Some(SNAPSHOT_PACKET) => saw_snapshot = true,
            Some(crate::realtime_event::REALTIME_EVENT_PACKET) => saw_realtime = true,
            _ => {}
        }
        if saw_snapshot && saw_realtime {
            break;
        }
    }
    assert!(
        saw_realtime,
        "the saturated realtime lane is delivered on its own `0x7d` carrier"
    );
    assert!(
        saw_snapshot,
        "the snapshot frame is still built while the realtime lane is at its capacity"
    );
    let guard = state.lock().expect("server state should not be poisoned");
    let client = guard.clients.get(&player_id).expect("client present");
    assert!(
        client.pending_snapshot_events.watch_damage.is_empty(),
        "saturation on the realtime lane cannot hold the snapshot lane's own event back"
    );
    assert!(
        client.pending_realtime_events.queue.len() < capacity,
        "the realtime lane drains on its own carrier rather than being squeezed out by the snapshot cap"
    );
}

#[test]
fn unsent_snapshot_requeues_pending_watch_damage_events() {
    // [REVIEW H5] WATCH_DAMAGE events are one-shot combat facts deduped by
    // damage_seq on the client; a frame dropped by send pacing (or the loss
    // simulator) must re-queue its drained events for the next frame
    // instead of losing them permanently.
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    let event = crate::snapshot::PendingWatchDamage {
        victim_id: player_id,
        shooter_id: player_id,
        victim_life_seq: 0,
        shooter_life_seq: 0,
        weapon_data: 66,
        damage: 400,
        energy_before: 1_700,
        energy_after: 1_300,
        server_tick: 123,
        damage_seq: 7,
        projectile_id: 9,
        lethal: false,
    };
    {
        let mut g = state.lock().expect("server state should not be poisoned");
        g.arena_tick = 2;
        let recipient = g.active_player_runtime_identity(player_id).unwrap();
        let client = g.clients.get_mut(&player_id).expect("client present");
        client
            .pending_snapshot_events
            .watch_damage
            .push_back(crate::snapshot::PendingWatchDamageEnvelope { recipient, event });
        client.send_pacing.tokens = 0.0; // pacing drops this frame
    }
    emit_snapshots_for_tick_inner(&state, true);
    assert!(rx.try_recv().is_err(), "paced-out frame must not be sent");
    {
        let mut g = state.lock().expect("server state should not be poisoned");
        g.arena_tick = 4;
        let client = g.clients.get_mut(&player_id).expect("client present");
        assert_eq!(
            client.observer_presentation_history.len(),
            0,
            "pacing rejection cannot fabricate an emitted presentation frame"
        );
        assert_eq!(
            client.pending_snapshot_events.watch_damage.len(),
            1,
            "a dropped frame must re-queue its drained events"
        );
        client.send_pacing.tokens = 1_000_000.0;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let packet = rx.try_recv().expect("next snapshot frame");
    assert_eq!(packet[0], SNAPSHOT_PACKET);
    assert_eq!(
        state.lock().unwrap().clients[&player_id]
            .observer_presentation_history
            .len(),
        1,
        "only the successful retry becomes presentation evidence"
    );
    assert_eq!(
        packet[20], 5,
        "the re-queued WATCH_DAMAGE must ride the next emitted frame"
    );
}

#[test]
fn closed_snapshot_sink_cannot_fabricate_observer_presentation_history() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    drop(rx);

    {
        let mut guard = state.lock().expect("server state should not be poisoned");
        let position = large_position_from_client(
            player_id,
            &client_position_packet_with_sequence(1_000, 500, 500, 1),
        )
        .expect("position");
        guard.set_client_position(player_id, position);
        seed_snapshot_motion(guard.clients.get_mut(&player_id).unwrap());
        guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_at_tick(&state, 1_000);

    let guard = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        guard.clients[&player_id]
            .observer_presentation_history
            .len(),
        0,
        "a failed outbound queue write is not an emitted frame"
    );
}

#[test]
fn observer_life_advance_clears_prior_presentation_scope() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    {
        let mut guard = state.lock().expect("server state should not be poisoned");
        guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_at_tick(&state, 100);
    rx.try_recv().expect("emitted snapshot");
    {
        let mut guard = state.lock().expect("server state should not be poisoned");
        assert_eq!(
            guard.clients[&player_id]
                .observer_presentation_history
                .len(),
            1
        );
        guard
            .advance_client_life_at_tick(player_id, 101)
            .expect("new life");
        assert_eq!(
            guard.clients[&player_id]
                .observer_presentation_history
                .len(),
            0,
            "a new observer life cannot rebind a prior applied watermark"
        );
    }
}

fn set_load_pressure(state: &mut ServerState, ratio: f64) {
    let tick_budget_us = 1_000_000.0 / state.config.tick_rate_hz.max(1) as f64;
    state.tick_wallclock_ewma_us = tick_budget_us * ratio;
}

#[test]
fn mid_tier_load_shed_keeps_near_entities_and_increments_counter() {
    let (state, alpha, beta, _gamma, rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.arena_tick = 2;
        set_load_pressure(&mut state, LOAD_SHED_MID_THRESHOLD + 0.01);
    }

    emit_snapshots_for_tick_inner(&state, true);
    let packet = rx_a.try_recv().expect("near-preserving shed frame");
    let (near, mid) = snapshot_entity_ids(&packet);
    assert_eq!(near, vec![alpha, beta]);
    assert!(mid.is_empty(), "mid entities should be shed under pressure");
    let state = state.lock().expect("server state should not be poisoned");
    assert!(
        state.load_shed_mid_skipped > 0,
        "shed entity count should be observable"
    );
    assert_eq!(state.load_shed_frame_skipped, 0);
}

#[test]
fn sustained_mid_tier_load_shed_refreshes_before_client_proxy_expiry() {
    let (state, alpha, beta, gamma, rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let near_interval = {
        let state = state.lock().expect("server state should not be poisoned");
        near_snapshot_interval_ticks(state.config.tick_rate_hz)
    };
    let mut last_mid_refresh = 0_u64;
    let mut refreshes = Vec::new();

    for emission in 1..=u64::from(AOI_PROXY_LIFETIME_SNAPSHOTS) * 2 {
        {
            let mut state = state.lock().expect("server state should not be poisoned");
            state.arena_tick = emission * near_interval;
            set_load_pressure(&mut state, LOAD_SHED_MID_THRESHOLD + 0.01);
        }
        emit_snapshots_for_tick_inner(&state, true);
        let packet = rx_a.try_recv().expect("load-shed snapshot frame");
        let (near, mid) = snapshot_entity_ids(&packet);
        assert_eq!(near, vec![alpha, beta]);
        if mid.contains(&gamma) {
            assert!(
                emission - last_mid_refresh <= LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS,
                "MID actor went too long without authoritative membership refresh"
            );
            last_mid_refresh = emission;
            refreshes.push(emission);
        }
    }

    assert_eq!(
        refreshes,
        vec![
            LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS,
            LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS * 2,
            LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS * 3,
            LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS * 4,
        ],
        "sustained pressure must retain a deterministic bounded MID heartbeat"
    );
    assert!(
        u64::from(AOI_PROXY_LIFETIME_SNAPSHOTS) >= LOAD_SHED_MID_REFRESH_INTERVAL_SNAPSHOTS * 2,
        "one lost heartbeat must not expire a living in-AOI actor"
    );
}

#[test]
fn dead_recipient_retains_read_only_aoi_anchor_without_presenting_self() {
    let (state, alpha, beta, gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.clients.get_mut(&alpha).expect("alpha present").alive = false;
        state.clear_client_position_preserving_aoi_observer(alpha);
        assert!(state.clients[&alpha].last_position.is_none());
        assert!(state.clients[&alpha].visibility_cell.is_none());
        assert!(state.clients[&alpha].aoi_observer_position.is_some());
    }

    let state = state.lock().expect("server state should not be poisoned");
    let (near, mid) = compute_aoi_entities(&state, alpha);
    assert_eq!(
        near.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![beta],
        "a dead viewer must keep live nearby actors without presenting itself"
    );
    assert_eq!(
        mid.iter().map(|(id, _, _, _)| *id).collect::<Vec<_>>(),
        vec![gamma]
    );
}

#[test]
fn ordinary_position_clear_removes_death_view_aoi_anchor() {
    let (state, alpha, _beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let mut state = state.lock().expect("server state should not be poisoned");
    state.clear_client_position(alpha);
    assert!(state.clients[&alpha].last_position.is_none());
    assert!(state.clients[&alpha].aoi_observer_position.is_none());
    assert_eq!(
        compute_aoi_entities(&state, alpha),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn a_production_damage_death_followed_by_respawn_still_offers_the_peer_to_that_observer() {
    // Found on the browser side first. An observer that died recorded 2034 consecutive snapshots in which
    // a peer it had listed in a tier 193 times before its death was known in its world and present in neither tier,
    // while the other client listed that same peer in near for 1652 snapshots. Separation was about 2559 px for both,
    // because they are the same pair of actors seen from each end, so no distance rule explains the asymmetry. Six
    // client-side theories were retired on evidence before this question was asked of the server: after the production
    // death path and the life advance a respawn performs, is the peer still offered to that recipient at all?
    //
    // The anchor-retaining behaviour is already tested, but only through the legacy boundary that is `dead_code`
    // outside tests. This drives the path production actually takes.
    let (state, alpha, beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let listed_ids = |state: &ServerState| {
        let (near, mid) = compute_aoi_entities(state, alpha);
        near.iter()
            .map(|(player_id, _)| *player_id)
            .chain(mid.iter().map(|(player_id, _, _, _)| *player_id))
            .collect::<Vec<_>>()
    };

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        assert!(
            listed_ids(&state).contains(&beta),
            "the fixture must start with alpha able to see beta"
        );
        let runtime = state
            .active_player_runtime_identity(alpha)
            .expect("alpha has an active runtime");
        state.clear_client_position_after_damage_death(runtime, 100);
    }

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .advance_client_life_at_tick(alpha, 101)
            .expect("respawn advances life");
    }

    let state = state.lock().expect("server state should not be poisoned");
    assert!(
        listed_ids(&state).contains(&beta),
        "a respawned observer must keep being offered its peer: the browser evidence shows a peer vanishing from          both tiers for thousands of snapshots after exactly this transition, while the other observer keeps          receiving it at the same separation"
    );
}

// An observer anchor preserves interest during an incomplete life cutover;
// it must never become actor authority. The old ignored reproduction called
// this a completed respawn despite skipping both production death state and
// the mandatory spawn baseline. The actual death/respawn test below verifies
// the positive idle-presentation contract after baseline admission.
#[test]
fn a_partial_life_cutover_does_not_publish_an_actor_from_its_observer_anchor() {
    let (state, alpha, beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let runtime = state
            .active_player_runtime_identity(alpha)
            .expect("alpha has an active runtime");
        state.clear_client_position_after_damage_death(runtime, 100);
        state
            .advance_client_life_at_tick(alpha, 101)
            .expect("respawn advances life");
        // Deliberately omit the authoritative position baseline.
    }

    let state = state.lock().expect("server state should not be poisoned");
    let (near, mid) = compute_aoi_entities(&state, beta);
    let listed: Vec<u16> = near
        .iter()
        .map(|(player_id, _)| *player_id)
        .chain(mid.iter().map(|(player_id, _, _, _)| *player_id))
        .collect();
    let alpha_client = &state.clients[&alpha];
    assert!(
        alpha_client.alive,
        "the stale alive flag alone is not authority"
    );
    assert!(alpha_client.last_position.is_none());
    assert!(alpha_client.visibility_cell.is_none());
    assert!(alpha_client.aoi_observer_position.is_some());
    assert!(
        !listed.contains(&alpha),
        "an incomplete life cutover must not publish its read-only anchor as an actor: {:?}",
        listed
    );
}

#[test]
fn a_second_joiner_that_never_reports_a_position_is_invisible_to_its_peer_and_observes_from_the_spawn_anchor()
 {
    // A joined connection with no admitted position baseline may observe
    // from the authored anchor, but is not yet a published actor. Stationary
    // admitted players are covered separately by the production death/respawn
    // test; lack of movement must not be confused with lack of admission.
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let (tx_g, rx_g) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    let gamma = register_connection(&state, tx_g);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    join_arena(&state, gamma, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_g);

    let spawn = {
        let guard = state.lock().expect("server state should not be poisoned");
        crate::zone::server_spawn_coordinates(guard.zone_map())
    };
    {
        let mut guard = state.lock().expect("server state should not be poisoned");
        // Alpha reports a position. Beta joins and never moves, so it never reports anything.
        let alpha_pos = large_position_from_client(alpha, &client_position_packet(0x1, 500, 500))
            .expect("alpha large pos");
        guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(guard.clients.get_mut(&alpha).unwrap());
        // The join path installs the read-only anchor; nothing else does for beta.
        guard.set_client_spawn_aoi_observer(beta);
        // Gamma parks beside the authored spawn, which is the only place beta's interest can be centred.
        let gamma_pos = large_position_from_client(
            gamma,
            &client_position_packet(
                0x1,
                spawn.0.saturating_add(120),
                spawn.1.saturating_add(120),
            ),
        )
        .expect("gamma large pos");
        guard.set_client_position(gamma, gamma_pos);
        seed_snapshot_motion(guard.clients.get_mut(&gamma).unwrap());
    }

    let guard = state.lock().expect("server state should not be poisoned");
    assert!(
        guard.clients[&beta].last_position.is_none()
            && guard.clients[&beta].aoi_observer_position.is_some(),
        "beta is joined, anchored, and has never reported a position - the parked joiner's exact state"
    );

    let listed_by_alpha: Vec<u16> = {
        let (near, mid) = compute_aoi_entities(&guard, alpha);
        near.iter()
            .map(|(player_id, _)| *player_id)
            .chain(mid.iter().map(|(player_id, _, _, _)| *player_id))
            .collect()
    };
    assert!(
        !listed_by_alpha.contains(&beta),
        "documented current behaviour: a peer that never reported a position is dropped from the candidate list, so          alpha cannot be offered beta at all: got {:?}",
        listed_by_alpha
    );

    let listed_by_beta: Vec<u16> = {
        let (near, mid) = compute_aoi_entities(&guard, beta);
        near.iter()
            .map(|(player_id, _)| *player_id)
            .chain(mid.iter().map(|(player_id, _, _, _)| *player_id))
            .collect()
    };
    assert!(
        listed_by_beta.contains(&gamma),
        "the observer path needs no reported position, so beta is served whoever sits near the authored spawn: got {:?}",
        listed_by_beta
    );
    assert!(
        !listed_by_beta.contains(&alpha),
        "and because that interest is centred on the spawn rather than where beta actually is, alpha at (500, 500) is          not offered to beta: got {:?}",
        listed_by_beta
    );
}

#[test]
fn aoi_tiers_fall_back_to_the_anchor_for_the_observer_but_not_for_a_presented_candidate() {
    // An incomplete lifecycle cutover can retain read-only observer interest
    // without an admitted actor pose. Preserve that authority separation;
    // this is not a completed respawn. The original historical browser report
    // is not attributed merely by constructing this internal partial state.
    let (state, alpha, beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let runtime = state
            .active_player_runtime_identity(beta)
            .expect("beta has an active runtime");
        state.clear_client_position_after_damage_death(runtime, 100);
        state
            .advance_client_life_at_tick(beta, 101)
            .expect("respawn advances life");
    }

    let state = state.lock().expect("server state should not be poisoned");
    let respawned = &state.clients[&beta];
    assert!(
        respawned.alive
            && respawned.last_position.is_none()
            && respawned.aoi_observer_position.is_some(),
        "the transition under test: alive again, no client-reported position, anchor retained"
    );

    // Observer side: the anchor is enough, so a stationary respawned client is still served peers.
    let (near, mid) = compute_aoi_entities(&state, beta);
    let as_observer: Vec<u16> = near
        .iter()
        .map(|(player_id, _)| *player_id)
        .chain(mid.iter().map(|(player_id, _, _, _)| *player_id))
        .collect();
    assert!(
        as_observer.contains(&alpha),
        "the observer path uses the anchor fallback and must keep offering peers: got {:?}",
        as_observer
    );

    // Candidate side: never publish a read-only anchor as an admitted actor.
    let (peer_near, peer_mid) = compute_aoi_entities(&state, alpha);
    let as_candidate: Vec<u16> = peer_near
        .iter()
        .map(|(player_id, _)| *player_id)
        .chain(peer_mid.iter().map(|(player_id, _, _, _)| *player_id))
        .collect();
    assert!(
        !as_candidate.contains(&beta),
        "documented current behaviour: a respawned player with no reported position is dropped from the peer's          candidate list, so it is invisible to that peer: got {:?}",
        as_candidate
    );
}

#[test]
fn full_frame_load_shed_preserves_pending_one_shot_events() {
    let (state, alpha, _beta, _gamma, rx_a, rx_b, rx_c) = place_three_aoi_clients();
    let event = watch_damage_event(alpha, alpha);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.arena_tick = 2;
        set_load_pressure(&mut state, LOAD_SHED_FRAME_THRESHOLD + 0.01);
        state.tick_wallclock_overrun_streak = LOAD_SHED_FRAME_OVERRUN_STREAK;
        let recipient = state.active_player_runtime_identity(alpha).unwrap();
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .pending_snapshot_events
            .watch_damage
            .push_back(crate::snapshot::PendingWatchDamageEnvelope { recipient, event });
    }

    emit_snapshots_for_tick_inner(&state, true);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    assert!(rx_c.try_recv().is_err());
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        assert_eq!(state.load_shed_frame_skipped, 3);
        assert_eq!(state.load_shed_mid_skipped, 0);
        assert_eq!(
            state.clients[&alpha]
                .pending_snapshot_events
                .watch_damage
                .front()
                .map(|envelope| envelope.event),
            Some(event),
            "a skipped frame must not drain a one-shot combat event"
        );
        state.arena_tick = 4;
        set_load_pressure(&mut state, 0.0);
    }
    emit_snapshots_for_tick_inner(&state, true);
    let recovery = rx_a.try_recv().expect("recovery snapshot");
    assert_eq!(snapshot_watch_damage_events(&recovery).len(), 1);
}

/// A recipient whose frames the server skipped to protect its own tick budget must not then be refused for being
/// behind frames that were never sent. `scale_bench.rs` hits this on demand at 200 peers: sustained load pressure
/// sets `shed_frame`, the whole frame is skipped for a recipient, its applied snapshot stops advancing, and
/// `verify_receipt` rejects its movement input as `ApplicationTooFarBehind` - so a client the server chose to shed
/// freezes while the server believes it is healthy.
#[test]
fn a_shed_recipient_is_not_refused_for_frames_the_server_never_sent() {
    let (state, alpha, _beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);

    // Everything the recipient can legitimately cite: the newest frame it was actually given, and the decision it
    // owns for the input sequence that will carry the receipt.
    let oldest_sent = {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        let oldest_sent = {
            let oldest = client
                .observer_presentation_history
                .frames()
                .next()
                .expect("a clean emission reaches the recipient");
            oldest.snapshot_id
        };
        // Sequences come from the directive that is effective now, not from invented numbers: the shared test
        // sequence allocator moves `effective_from_input_sequence` around, and a receipt citing a sequence outside
        // the directive's range is refused as GenerationNotEffective for a reason unrelated to the defect under test.
        oldest_sent
    };

    // Now shed sustained for far longer than any profile's hold grace, so ticks pass with nothing new sent.
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        set_load_pressure(&mut state, LOAD_SHED_FRAME_THRESHOLD + 0.01);
        state.tick_wallclock_overrun_streak = LOAD_SHED_FRAME_OVERRUN_STREAK;
    }
    for tick in 3..=40 {
        {
            let mut state = state.lock().expect("server state should not be poisoned");
            state.arena_tick = tick;
        }
        emit_snapshots_for_tick_inner(&state, true);
    }
    {
        let state = state.lock().expect("server state should not be poisoned");
        assert!(
            state.load_shed_frame_skipped > 0,
            "the fixture must actually shed frames, or it does not reproduce the defect"
        );
        let client = state.clients.get(&alpha).expect("alpha should exist");
        let newest = client
            .observer_presentation_history
            .frames()
            .next_back()
            .expect("the recipient still holds its emitted frame");
        assert_eq!(
            newest.snapshot_id, oldest_sent,
            "shedding must have sent the recipient nothing newer"
        );
    }

    // Read the generation that is effective *now*, after the shedding has issued whatever directives it issued: a
    // generation read before the loop is superseded and the receipt is then refused as GenerationNotEffective for a
    // reason that has nothing to do with this defect.
    let (sequence, generation) = {
        let state = state.lock().expect("server state should not be poisoned");
        let directive = state.clients[&alpha]
            .observer_combat_timeline
            .current_directive()
            .expect("a joined client owns a directive");
        (
            directive.effective_from_input_sequence.wrapping_add(1),
            directive.generation,
        )
    };

    // The recipient is current with everything it was given, far beyond the hold grace. That must be accepted: the
    // only frames it is behind are the ones the server declined to send.
    let shed_decision = {
        let mut state = state.lock().expect("server state should not be poisoned");
        let runtime = state
            .active_player_runtime_identity(alpha)
            .expect("alpha should have a runtime");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.observer_combat_timeline.verify_receipt(
            runtime,
            // Anchored to the same clock the emissions use, the way the bench does it: a hard-coded tick drifts
            // against the shared server clock and gets rejected as AppliedSnapshotAheadOfInput before the age
            // check runs at all. 40 ticks past now is far beyond any profile's hold grace.
            crate::wire::current_tick() + 40,
            crate::combat_timeline::CombatViewReceiptV1 {
                observer_life_seq: runtime.life.life_seq,
                input_sequence: sequence,
                timeline_generation: generation,
                latest_game_applied_snapshot_id: oldest_sent,
                flags: 0,
            },
            &client.observer_presentation_history,
        )
    };
    assert!(
        shed_decision.is_ok(),
        "a recipient the server shed was refused for frames the server never sent: {shed_decision:?}"
    );

    // Companion assertion, so the repair cannot be a blanket loosening: once the server resumes sending, a client
    // that is behind the newest *sent* frame is still refused at the same age.
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        set_load_pressure(&mut state, 0.0);
        state.tick_wallclock_overrun_streak = 0;
        state.arena_tick = 41;
    }
    emit_snapshots_for_tick_inner(&state, true);
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        let newest = client
            .observer_presentation_history
            .frames()
            .next_back()
            .expect("the recovery frame");
        assert!(
            newest.snapshot_id != oldest_sent,
            "the recovery emission must send something newer"
        );
    }
    let (companion_sequence, companion_generation) = {
        let state = state.lock().expect("server state should not be poisoned");
        let directive = state.clients[&alpha]
            .observer_combat_timeline
            .current_directive()
            .expect("a joined client owns a directive");
        (
            directive.effective_from_input_sequence.wrapping_add(2),
            directive.generation,
        )
    };
    let stale_decision = {
        let mut state = state.lock().expect("server state should not be poisoned");
        let runtime = state
            .active_player_runtime_identity(alpha)
            .expect("alpha should have a runtime");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.observer_combat_timeline.verify_receipt(
            runtime,
            crate::wire::current_tick() + 90,
            crate::combat_timeline::CombatViewReceiptV1 {
                observer_life_seq: runtime.life.life_seq,
                input_sequence: companion_sequence,
                timeline_generation: companion_generation,
                latest_game_applied_snapshot_id: oldest_sent,
                flags: 0,
            },
            &client.observer_presentation_history,
        )
    };
    assert_eq!(
        stale_decision.err(),
        Some(crate::combat_timeline::ObserverTimelineReceiptRejection::ApplicationTooFarBehind),
        "a client genuinely behind the newest sent frame must still be refused"
    );
}

#[test]
fn transient_high_ewma_does_not_shed_a_clean_lan_frame() {
    let (state, _alpha, _beta, _gamma, rx_a, rx_b, rx_c) = place_three_aoi_clients();
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.arena_tick = 2;
        set_load_pressure(&mut state, LOAD_SHED_FRAME_THRESHOLD + 0.01);
        state.tick_wallclock_overrun_streak = 0;
    }

    emit_snapshots_for_tick_inner(&state, true);
    assert!(rx_a.try_recv().is_ok());
    assert!(rx_b.try_recv().is_ok());
    assert!(rx_c.try_recv().is_ok());
    let state = state.lock().expect("server state should not be poisoned");
    assert_eq!(state.load_shed_frame_skipped, 0);
}

#[test]
fn soft_life_advance_preserves_public_damage_but_rejects_a_replaced_session() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let event = watch_damage_event(player_id, player_id);

    let stale_recipient = {
        let mut state = state.lock().unwrap();
        let recipient = state.active_player_runtime_identity(player_id).unwrap();
        state
            .clients
            .get_mut(&player_id)
            .unwrap()
            .pending_snapshot_events
            .watch_damage
            .push_back(crate::snapshot::PendingWatchDamageEnvelope { recipient, event });
        state.advance_client_life(player_id).unwrap();
        assert_eq!(
            state.clients[&player_id]
                .pending_snapshot_events
                .watch_damage
                .len(),
            1,
            "observer life advancement must preserve a self-contained public combat fact"
        );
        recipient
    };

    {
        let mut state = state.lock().unwrap();
        state.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let packet = rx.try_recv().expect("snapshot should still emit");
    assert_eq!(snapshot_watch_damage_events(&packet).len(), 1);
    assert!(
        state.lock().unwrap().clients[&player_id]
            .pending_snapshot_events
            .watch_damage
            .is_empty()
    );

    {
        let mut state = state.lock().unwrap();
        state.arena_tick = 4;
        let replacement_session = crate::authority_identity::PlayerSessionId::new(
            stale_recipient.session.session_id.get().wrapping_add(1),
        );
        let client = state.clients.get_mut(&player_id).unwrap();
        client.session_id = replacement_session;
        client.pending_snapshot_events.watch_damage.push_back(
            crate::snapshot::PendingWatchDamageEnvelope {
                recipient: stale_recipient,
                event,
            },
        );
    }
    emit_snapshots_for_tick_inner(&state, true);
    let packet = rx
        .try_recv()
        .expect("replacement snapshot should still emit");
    assert!(snapshot_watch_damage_events(&packet).is_empty());
}

#[test]
fn snapshot_seq_increments_per_recipient_across_emissions() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    for emit_index in 1..=3 {
        {
            let mut state_guard = state.lock().expect("server state should not be poisoned");
            // Land on each successive interval tick (2, 4, 6).
            state_guard.arena_tick = emit_index * 2;
        }
        emit_snapshots_for_tick_inner(&state, true);
        let packet = rx.try_recv().expect("packet");
        assert_eq!(
            u16::from_le_bytes([packet[2], packet[3]]),
            emit_index as u16,
            "snapshot_id should increment per emission"
        );
    }
}

#[test]
fn server_skips_unjoined_clients() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_joined, rx_joined) = mpsc::channel();
    let (tx_pending, rx_pending) = mpsc::channel();
    let joined_id = register_connection(&state, tx_joined);
    let _pending_id = register_connection(&state, tx_pending);
    // Only `joined_id` calls `join_arena` — the other stays unjoined.
    join_arena(&state, joined_id, 0);
    drain_available(&rx_joined);
    drain_available(&rx_pending);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);

    assert!(rx_joined.try_recv().is_ok());
    assert!(
        rx_pending.try_recv().is_err(),
        "unjoined client should not receive a snapshot"
    );
}

#[test]
fn build_empty_snapshot_matches_client_parser_layout() {
    // Sanity: the bytes the server writes match the protocol-side
    // SnapshotHeader::HEADER_LEN. Keeps the duplicated wire constants
    // honest. (Cross-check via the 13-byte expectation; the protocol
    // module's tests independently verify the parse direction.)
    let packet = build_empty_snapshot(7, 5, 0, 9_999);
    assert_eq!(packet.len(), SNAPSHOT_HEADER_LEN);
    assert_eq!(packet[0], 0x70);
    assert_eq!(SNAPSHOT_HEADER_LEN, 21);
    assert_eq!(snapshot_interval_ticks(60), 2);
}

// Phase 3e-2: per-recipient AOI entity computation. The fixture
// places three players at known positions and exercises
// `compute_aoi_entities` directly for full control over distance
// classifications.
fn place_three_aoi_clients() -> (
    Arc<Mutex<ServerState>>,
    u16,
    u16,
    u16,
    mpsc::Receiver<Vec<u8>>,
    mpsc::Receiver<Vec<u8>>,
    mpsc::Receiver<Vec<u8>>,
) {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let (tx_c, rx_c) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    let gamma = register_connection(&state, tx_c);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    join_arena(&state, gamma, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        // Alpha at (500, 500).
        let alpha_pos = large_position_from_client(alpha, &client_position_packet(0x1, 500, 500))
            .expect("alpha pos");
        // Beta at (1500, 500) — 1000 px away from alpha (inside near = 2048).
        let beta_pos = large_position_from_client(beta, &client_position_packet(0x1, 1_500, 500))
            .expect("beta pos");
        // Gamma at (3000, 500) — 2500 px away from alpha (in mid = 2560).
        let gamma_pos = large_position_from_client(gamma, &client_position_packet(0x1, 3_000, 500))
            .expect("gamma pos");
        state_guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&alpha).unwrap());
        state_guard.set_client_position(beta, beta_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&beta).unwrap());
        state_guard.set_client_position(gamma, gamma_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&gamma).unwrap());
    }
    (state, alpha, beta, gamma, rx_a, rx_b, rx_c)
}

#[test]
fn aoi_entities_classify_others_into_near_and_mid_buckets() {
    let (state, alpha, beta, gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let state = state.lock().expect("server state should not be poisoned");
    let (near, mid) = compute_aoi_entities(&state, alpha);

    assert_eq!(
        near.len(),
        2,
        "alpha self + beta should be in alpha's near AOI"
    );
    assert_eq!(near[0].0, alpha, "self is first for reconciliation");
    assert_eq!(near[0].1.x, 500);
    assert_eq!(near[0].1.y, 500);
    assert_eq!(near[1].0, beta);
    assert_eq!(near[1].1.x, 1_500);
    assert_eq!(near[1].1.y, 500);

    assert_eq!(mid.len(), 1, "gamma should be in alpha's mid AOI");
    assert_eq!(mid[0], (gamma, 3_000, 500, 1));
}

/// Authors a carry expiry and puts a map flag exactly on beta's hull (which the AOI fixture parks at
/// (1500, 500)), so a claim from beta is granted by the ordinary path.
fn flag_on_beta_with_carry_expiry(
    state: &Arc<Mutex<ServerState>>,
    beta: u16,
    carry_timeout_ticks: u32,
) -> u16 {
    let mut guard = state.lock().expect("server state should not be poisoned");
    let mut profile = synthetic_profile(1, 0);
    match &mut profile.flags {
        ServerFlagProfile::Enabled(settings) => settings.carry_timeout_ticks = carry_timeout_ticks,
        _ => panic!("the synthetic flag profile must be enabled"),
    }
    guard
        .objectives
        .reset(profile)
        .expect("the flag profile should validate");
    // The claim resolves the claimant's authoritative hull from a client-format position packet
    // (the AOI fixture's canonical 23-byte form carries no client sequence), so hand beta the same
    // bytes a real client would have sent from the flag's tile.
    let client = guard.clients.get_mut(&beta).expect("beta joined");
    client.alive = true;
    guard.set_client_position(beta, client_position_packet_with_sequence(0, 1_500, 500, 1));
    guard
        .objectives
        .create_flag(crate::objectives::FlagState::OnMap {
            x: 1_500,
            y: 500,
            owner: None,
        })
        .expect("the flag should fit the profile")
        .identity
        .id
}

fn near_entity<'a>(
    near: &'a [(u16, crate::snapshot::NearEntityState)],
    actor_id: u16,
) -> &'a crate::snapshot::NearEntityState {
    &near
        .iter()
        .find(|(id, _)| *id == actor_id)
        .unwrap_or_else(|| panic!("actor {actor_id} was not in the near list"))
        .1
}

/// The near-snapshot carry countdown (`flag_timer`) means carry *remaining* in continuum ticks. The
/// bytes ride in the client's own `LARGE_POSITION` extension, but the server stores a canonical 23-byte
/// position form that drops client-provided time and prize state, so before this producer every near
/// entity reported 0. The registry deadline is the only truth about a carry, so a carrier in a zone that
/// authors one now reports its remaining ticks.
#[test]
fn the_near_carry_countdown_reports_the_authored_carry_deadline() {
    const CARRY_TIMEOUT_TICKS: u32 = 250;
    let (state, alpha, beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let flag_id = flag_on_beta_with_carry_expiry(&state, beta, CARRY_TIMEOUT_TICKS);

    {
        let mut guard = state.lock().expect("server state should not be poisoned");
        crate::objectives::legacy_flag_claim_packets(
            &mut guard,
            beta,
            flag_id,
            crate::wire::current_tick(),
        )
        .expect("a hull sitting on the flag should be granted the claim");
    }
    assert!(
        matches!(
            state
                .lock()
                .expect("server state should not be poisoned")
                .objectives
                .replay()
                .flags[0]
                .state,
            crate::objectives::FlagState::Carried { .. }
        ),
        "the claim has to be granted before any countdown can mean anything"
    );

    let guard = state.lock().expect("server state should not be poisoned");
    let (near, _) = compute_aoi_entities(&guard, alpha);
    assert_eq!(
        near_entity(&near, beta).flag_timer,
        u16::try_from(CARRY_TIMEOUT_TICKS).expect("fits"),
        "a carrier must present the registry's remaining ticks to a peer"
    );
    assert_eq!(
        near_entity(&near, alpha).flag_timer,
        0,
        "the viewer carries nothing and must not show a countdown"
    );
}

/// `FlagCarryTimeoutTicks = 0` means a carry never expires - `step_objective_deadlines` continues past a
/// zero timeout - so there is nothing truthful to encode and the field stays 0 even for the carrier.
/// Every shipped zone authors 0 today.
#[test]
fn a_zone_without_an_authored_carry_expiry_reports_no_countdown_for_its_carrier() {
    let (state, alpha, beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let flag_id = flag_on_beta_with_carry_expiry(&state, beta, 0);

    {
        let mut guard = state.lock().expect("server state should not be poisoned");
        crate::objectives::legacy_flag_claim_packets(
            &mut guard,
            beta,
            flag_id,
            crate::wire::current_tick(),
        )
        .expect("a hull sitting on the flag should be granted the claim");
    }

    let guard = state.lock().expect("server state should not be poisoned");
    let (near, _) = compute_aoi_entities(&guard, alpha);
    assert!(
        matches!(
            guard.objectives.replay().flags[0].state,
            crate::objectives::FlagState::Carried { .. }
        ),
        "beta really is carrying; the zero below is the authored no-expiry, not a lost grant"
    );
    assert_eq!(
        near_entity(&near, beta).flag_timer,
        0,
        "with no authored carry expiry there is no remaining time to encode"
    );
}

#[test]
fn shared_cell_publication_preserves_recipient_tiers_and_refreshes_next_tick() {
    let (state, alpha, beta, gamma, rx_a, rx_b, rx_c) = place_three_aoi_clients();
    for (step, gamma_x) in [10_148, 15_000, 10_148].into_iter().enumerate() {
        let expected = {
            let mut guard = state.lock().unwrap();
            for (id, x) in [(alpha, 8_000), (beta, 8_020), (gamma, gamma_x)] {
                let packet =
                    large_position_from_client(id, &client_position_packet(1, x, 8_000)).unwrap();
                guard.set_client_position(id, packet);
                let client = guard.clients.get_mut(&id).unwrap();
                if id == gamma {
                    client.life_seq = step as u32 + 1;
                }
                seed_snapshot_motion(client);
                client.aoi_tier_memory.clear();
            }
            guard
                .clients
                .get_mut(&alpha)
                .unwrap()
                .aoi_tier_memory
                .insert(gamma, crate::visibility::AoiTier::Near);
            assert_eq!(
                crate::visibility::visibility_cell_for_large_position(
                    guard.clients[&alpha].last_position.as_deref().unwrap()
                ),
                crate::visibility::visibility_cell_for_large_position(
                    guard.clients[&beta].last_position.as_deref().unwrap()
                ),
            );
            guard.arena_tick = 2 + step as u64 * 2;
            [alpha, beta].map(|id| {
                let (near, mid) = compute_aoi_entities(&guard, id);
                (
                    near.into_iter()
                        .map(|(id, entity)| (id, entity.life_seq))
                        .collect::<Vec<_>>(),
                    mid.into_iter()
                        .map(|(id, _, _, life)| (id, life))
                        .collect::<Vec<_>>(),
                )
            })
        };
        if step != 1 {
            assert!(expected[0].0.iter().any(|(id, _)| *id == gamma));
            assert!(expected[1].1.iter().any(|(id, _)| *id == gamma));
        } else {
            assert!(
                expected
                    .iter()
                    .all(|(near, mid)| near.iter().chain(mid).all(|(id, _)| *id != gamma))
            );
        }
        emit_snapshots_for_tick_inner(&state, true);
        for (rx, (near, mid)) in [&rx_a, &rx_b].into_iter().zip(expected) {
            let frame = SnapshotFrame::parse(&rx.try_recv().unwrap()).unwrap();
            assert_eq!(
                frame
                    .near_entities
                    .iter()
                    .map(|entity| (entity.entity_id, entity.life_seq.unwrap()))
                    .collect::<Vec<_>>(),
                near
            );
            assert_eq!(
                frame
                    .mid_entities
                    .iter()
                    .map(|entity| (entity.entity_id, entity.life_seq.unwrap()))
                    .collect::<Vec<_>>(),
                mid
            );
        }
        drain_available(&rx_c);
    }
}

#[test]
fn predictive_aoi_wired_path_admits_closing_candidate_early() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);

    let mut state = state.lock().expect("server state should not be poisoned");
    let alpha_position =
        large_position_from_client(alpha, &client_position_packet(0, 8_000, 8_000))
            .expect("alpha position");
    let stationary = large_position_from_client(
        beta,
        &client_position_packet(
            0,
            8_000 + crate::visibility::MID_AOI_RADIUS_PX as u16 + 20,
            8_000,
        ),
    )
    .expect("stationary beta position");
    state.set_client_position(alpha, alpha_position);
    seed_snapshot_motion(state.clients.get_mut(&alpha).unwrap());
    state.set_client_position(beta, stationary.clone());
    seed_snapshot_motion(state.clients.get_mut(&beta).unwrap());

    let (near, mid) = compute_aoi_entities(&state, alpha);
    assert!(near.iter().all(|(id, _)| *id != beta));
    assert!(mid.iter().all(|(id, _, _, _)| *id != beta));

    let mut closing = stationary;
    closing[10..12].copy_from_slice(&(-1_000_i16).to_le_bytes());
    state.set_client_position(beta, closing);
    seed_snapshot_motion(state.clients.get_mut(&beta).unwrap());
    let (near, mid) = compute_aoi_entities(&state, alpha);
    assert!(near.iter().all(|(id, _)| *id != beta));
    assert!(
        mid.iter().any(|(id, _, _, _)| *id == beta),
        "the wired candidate gather and predictive classifier should pull a closing ship into mid"
    );
}

#[test]
fn aoi_hysteresis_makes_a_tier_sticky_against_demotion() {
    use crate::visibility::AoiTier;
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);

    let mut g = state.lock().expect("server state should not be poisoned");
    // alpha at 8000,8000; beta 2148 px to the right — just outside near
    // (2048) but within near + hysteresis margin (2304).
    let a = large_position_from_client(alpha, &client_position_packet(0x1, 8_000, 8_000))
        .expect("alpha pos");
    let b = large_position_from_client(beta, &client_position_packet(0x1, 10_148, 8_000))
        .expect("beta pos");
    g.set_client_position(alpha, a);
    seed_snapshot_motion(g.clients.get_mut(&alpha).unwrap());
    g.set_client_position(beta, b);
    seed_snapshot_motion(g.clients.get_mut(&beta).unwrap());

    // No tier memory → plain radii → beta classifies as Mid at 2148 px.
    let (near, mid) = compute_aoi_entities(&g, alpha);
    assert!(
        near.iter().all(|(id, _)| *id != beta),
        "without memory beta should not be near at 2148 px"
    );
    assert!(
        mid.iter().any(|(id, _, _, _)| *id == beta),
        "without memory beta should be mid at 2148 px"
    );

    // Seed beta as previously Near → sticky demotion keeps it Near at the
    // same spot (within the margin).
    g.clients
        .get_mut(&alpha)
        .expect("alpha")
        .aoi_tier_memory
        .insert(beta, AoiTier::Near);
    let (near, mid) = compute_aoi_entities(&g, alpha);
    assert!(
        near.iter().any(|(id, _)| *id == beta),
        "a previously-near ship within the margin stays near (hysteresis)"
    );
    assert!(
        mid.iter().all(|(id, _, _, _)| *id != beta),
        "the sticky-near ship is not also in mid"
    );
}

#[test]
fn aoi_entities_include_recipient_self_for_authoritative_energy() {
    let (state, alpha, _beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let state = state.lock().expect("server state should not be poisoned");
    let (near, mid) = compute_aoi_entities(&state, alpha);
    assert!(
        near.iter()
            .any(|(id, entity)| *id == alpha && entity.energy == 900),
        "alpha should receive their own server-authoritative energy"
    );
    assert!(
        !mid.iter().any(|&(id, _, _, _)| id == alpha),
        "alpha should not be in their own mid list"
    );
}

#[test]
fn aoi_entities_drops_far_recipients() {
    // Re-place gamma far enough that they fall outside mid AOI.
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let alpha_pos = large_position_from_client(alpha, &client_position_packet(0x1, 500, 500))
            .expect("alpha pos");
        // 5000 px away — outside mid (2560).
        let beta_pos = large_position_from_client(beta, &client_position_packet(0x1, 5_500, 500))
            .expect("beta pos");
        state_guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&alpha).unwrap());
        state_guard.set_client_position(beta, beta_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&beta).unwrap());
    }
    let state = state.lock().expect("server state should not be poisoned");
    let (near, mid) = compute_aoi_entities(&state, alpha);
    assert_eq!(near.len(), 1, "self should be the only near entity");
    assert_eq!(near[0].0, alpha);
    assert!(mid.is_empty(), "far beta should not be in mid");
}

#[test]
fn aoi_entities_emit_full_snapshot_with_correct_counts_and_payload() {
    // No ack — server falls back to a full snapshot. With Phase 3e-7
    // the near tier carries every field (including exact actor life) and the
    // wire layout is variable per entity. Verify high-level shape:
    // packet starts with type + FULL flag, base_id ignored, near_count = 2
    // (self + beta), mid_count = 1, and the second near entity id is beta.
    let (state, alpha, beta, gamma, rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);

    let packet = rx_a.try_recv().expect("alpha");
    assert_eq!(packet[0], SNAPSHOT_PACKET);
    assert_eq!(packet[18], 2, "near_count");
    assert_eq!(packet[19], 1, "mid_count");
    assert_eq!(packet[20], 4, "private authority event_count");
    assert_eq!(u16::from_le_bytes([packet[4], packet[5]]), 0, "base_id");

    let near_off = SNAPSHOT_HEADER_LEN;
    assert_eq!(
        u16::from_le_bytes([packet[near_off], packet[near_off + 1]]),
        alpha,
        "first near entity id should be self"
    );
    let beta_near_off = skip_snapshot_entity(&packet, near_off);
    assert_eq!(
        u16::from_le_bytes([packet[beta_near_off], packet[beta_near_off + 1]]),
        beta,
        "second near entity id should be beta"
    );
    let near_mask = u16::from_le_bytes([packet[near_off + 2], packet[near_off + 3]]);
    assert_eq!(
        near_mask & (SNAPSHOT_FIELD_X | SNAPSHOT_FIELD_Y),
        SNAPSHOT_FIELD_X | SNAPSHOT_FIELD_Y,
        "full-state near entity has X+Y bits set"
    );
    // x follows mask at offset near_off + 4.
    assert_eq!(
        u16::from_le_bytes([packet[near_off + 4], packet[near_off + 5]]),
        500,
        "self near entity x"
    );
    // Verify gamma is the only mid entity immediately after both variable
    // length near entities; the private Rocket event follows it.
    let mid_tail = skip_snapshot_entity(&packet, beta_near_off);
    assert_eq!(
        u16::from_le_bytes([packet[mid_tail], packet[mid_tail + 1]]),
        gamma,
        "mid entity id should be gamma"
    );
    let mid_mask = u16::from_le_bytes([packet[mid_tail + 2], packet[mid_tail + 3]]);
    assert_eq!(
        mid_mask,
        SNAPSHOT_FIELD_X | SNAPSHOT_FIELD_Y | SNAPSHOT_FIELD_LIFE_SEQ,
        "mid entity has x+y plus exact actor life"
    );
    assert_eq!(
        u16::from_le_bytes([packet[mid_tail + 4], packet[mid_tail + 5]]),
        3_000,
        "mid entity x"
    );
}

#[test]
fn aoi_entities_are_empty_for_recipient_without_position_or_observer() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    {
        // Only beta gets a position. Explicitly remove alpha's join-time
        // read-only observer to cover the fail-closed no-anchor state.
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let beta_pos = large_position_from_client(beta, &client_position_packet(0x1, 1_500, 500))
            .expect("beta pos");
        state_guard.set_client_position(beta, beta_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&beta).unwrap());
        state_guard
            .clients
            .get_mut(&alpha)
            .expect("alpha")
            .aoi_observer_position = None;
    }
    let state = state.lock().expect("server state should not be poisoned");
    let (near, mid) = compute_aoi_entities(&state, alpha);
    assert!(near.is_empty(), "unpositioned recipient gets empty near");
    assert!(mid.is_empty(), "unpositioned recipient gets empty mid");
}

#[test]
fn aoi_entities_skip_other_unpositioned_players() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let _beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, _beta, 0);
    {
        // Only alpha gets a position. Beta has joined but is
        // unpositioned; they should not appear in the entity list
        // because we have no x/y to send.
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let alpha_pos = large_position_from_client(alpha, &client_position_packet(0x1, 500, 500))
            .expect("alpha pos");
        state_guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&alpha).unwrap());
    }
    let state = state.lock().expect("server state should not be poisoned");
    let (near, mid) = compute_aoi_entities(&state, alpha);
    assert_eq!(near.len(), 1, "self should still be included");
    assert_eq!(near[0].0, alpha);
    assert!(mid.is_empty());
}

#[test]
fn aoi_entities_skip_spectator_ships() {
    // Beta with ship = 8 is a spectator and should not appear in
    // entity lists even though they're inside the AOI radius.
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 8); // spectator ship
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let alpha_pos = large_position_from_client(alpha, &client_position_packet(0x1, 500, 500))
            .expect("alpha pos");
        let beta_pos = large_position_from_client(beta, &client_position_packet(0x1, 1_500, 500))
            .expect("beta pos");
        state_guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&alpha).unwrap());
        state_guard.set_client_position(beta, beta_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&beta).unwrap());
    }
    let state = state.lock().expect("server state should not be poisoned");
    let (near, mid) = compute_aoi_entities(&state, alpha);
    assert_eq!(near.len(), 1, "self should still be included");
    assert_eq!(near[0].0, alpha);
    assert!(mid.is_empty(), "spectator should not appear in mid");
}

// Phase 3e-4a: SNAPSHOT_ACK reception updates last_acked_snapshot_id.
fn snapshot_ack_payload(snapshot_id: u16) -> Vec<u8> {
    let mut payload = vec![crate::connection::SNAPSHOT_ACK_TYPE];
    payload.extend_from_slice(&snapshot_id.to_le_bytes());
    payload
}

#[test]
fn server_records_last_acked_snapshot_id_from_reliable_payload() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(0, &snapshot_ack_payload(0x1234)),
        "test-peer",
        &mut true,
    );

    let state_guard = state.lock().expect("server state should not be poisoned");
    let client = state_guard.clients.get(&player_id).expect("client present");
    assert_eq!(client.last_acked_snapshot_id, Some(0x1234));
}

#[test]
fn server_ignores_backward_snapshot_acks_to_avoid_regressing() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(0, &snapshot_ack_payload(100)),
        "test-peer",
        &mut true,
    );
    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(1, &snapshot_ack_payload(50)),
        "test-peer",
        &mut true,
    );

    let state_guard = state.lock().expect("server state should not be poisoned");
    let client = state_guard.clients.get(&player_id).expect("client present");
    assert_eq!(
        client.last_acked_snapshot_id,
        Some(100),
        "later ack with lower id should not regress"
    );
}

#[test]
fn server_handles_snapshot_ack_wraparound() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(0, &snapshot_ack_payload(u16::MAX - 1)),
        "test-peer",
        &mut true,
    );
    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(1, &snapshot_ack_payload(2)),
        "test-peer",
        &mut true,
    );

    let state_guard = state.lock().expect("server state should not be poisoned");
    let client = state_guard.clients.get(&player_id).expect("client present");
    assert_eq!(
        client.last_acked_snapshot_id,
        Some(2),
        "wraparound ack within forward half should advance"
    );
}

#[test]
fn server_ignores_truncated_snapshot_ack_payload() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    let truncated = vec![crate::connection::SNAPSHOT_ACK_TYPE, 0x00];
    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(0, &truncated),
        "test-peer",
        &mut true,
    );

    let state_guard = state.lock().expect("server state should not be poisoned");
    let client = state_guard.clients.get(&player_id).expect("client present");
    assert_eq!(
        client.last_acked_snapshot_id, None,
        "truncated payload must not modify last_acked"
    );
}

// Phase 2 polish (RTT): snapshot acks should turn into a smoothed
// per-shooter RTT and a cached `lag_compensation_ticks` derived from
// half-RTT, capped at MAX_LAG_COMPENSATION_TICKS.
fn install_history_entry(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    snapshot_id: u16,
    emitted_at: std::time::Instant,
) {
    let mut guard = state.lock().expect("server state should not be poisoned");
    let client = guard.clients.get_mut(&player_id).expect("client present");
    client
        .snapshot_history
        .push_back(crate::snapshot::SnapshotHistoryEntry {
            snapshot_id,
            server_tick: 0,
            entities: Default::default(),
            emitted_at,
            prize_known: None,
        });
}

#[test]
fn snapshot_ack_with_matching_history_entry_updates_measured_rtt() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    let received_at = std::time::Instant::now();
    let emitted_at = received_at - std::time::Duration::from_millis(100);
    install_history_entry(&state, player_id, 7, emitted_at);

    crate::connection::record_snapshot_ack_at(
        &state,
        player_id,
        &snapshot_ack_payload(7),
        received_at,
    );

    let guard = state.lock().expect("server state should not be poisoned");
    let client = guard.clients.get(&player_id).expect("client present");
    let measured = client.measured_rtt.expect("RTT should be recorded");
    assert_eq!(measured, std::time::Duration::from_millis(100));
    assert_eq!(client.lag_compensation_ticks, 5);
}

#[test]
fn snapshot_ack_rtt_caps_lag_compensation_at_two_hundred_ms() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    // Simulate a ~2-second RTT (way beyond the 200ms cap).
    let received_at = std::time::Instant::now();
    let emitted_at = received_at - std::time::Duration::from_millis(2_000);
    install_history_entry(&state, player_id, 9, emitted_at);

    crate::connection::record_snapshot_ack_at(
        &state,
        player_id,
        &snapshot_ack_payload(9),
        received_at,
    );

    let guard = state.lock().expect("server state should not be poisoned");
    let client = guard.clients.get(&player_id).expect("client present");
    assert_eq!(
        client.lag_compensation_ticks,
        crate::state::MAX_LAG_COMPENSATION_TICKS,
        "extreme RTT must be clamped at MAX_LAG_COMPENSATION_TICKS"
    );
}

#[test]
fn snapshot_ack_without_matching_history_entry_leaves_rtt_unmeasured() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    // No history entry installed — the ack should advance
    // `last_acked_snapshot_id` but leave `measured_rtt = None`.
    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(0, &snapshot_ack_payload(42)),
        "test-peer",
        &mut true,
    );

    let guard = state.lock().expect("server state should not be poisoned");
    let client = guard.clients.get(&player_id).expect("client present");
    assert_eq!(client.last_acked_snapshot_id, Some(42));
    assert!(
        client.measured_rtt.is_none(),
        "ack without a history entry should not produce a measurement"
    );
    assert_eq!(
        client.lag_compensation_ticks,
        crate::state::DEFAULT_LAG_COMPENSATION_TICKS,
        "lag-comp ticks must stay at the default until first measurement"
    );
}

#[test]
fn snapshot_ack_rtt_smooths_across_consecutive_samples() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    // First sample: 100 ms RTT.
    let received_at = std::time::Instant::now();
    let first_emitted = received_at - std::time::Duration::from_millis(100);
    install_history_entry(&state, player_id, 1, first_emitted);
    crate::connection::record_snapshot_ack_at(
        &state,
        player_id,
        &snapshot_ack_payload(1),
        received_at,
    );

    // Second sample: 20 ms RTT — much shorter, but the EWMA should
    // pull the smoothed value down only ~1/8 of the way toward 20 ms.
    let second_received_at = received_at + std::time::Duration::from_millis(1);
    let second_emitted = second_received_at - std::time::Duration::from_millis(20);
    install_history_entry(&state, player_id, 2, second_emitted);
    crate::connection::record_snapshot_ack_at(
        &state,
        player_id,
        &snapshot_ack_payload(2),
        second_received_at,
    );

    let guard = state.lock().expect("server state should not be poisoned");
    let client = guard.clients.get(&player_id).expect("client present");
    let smoothed = client.measured_rtt.expect("RTT should be recorded");
    // After one outlier, smoothed ≈ 7/8 * 100 + 1/8 * 20 = 90 ms.
    assert_eq!(smoothed, std::time::Duration::from_millis(90));
}

#[test]
fn snapshot_entity_header_size_matches_protocol_constant() {
    // Pin the per-entity header (id + mask) wire size so the server
    // encoder can't drift from the protocol parser. Phase 3e-7
    // widened mask to u16 → 4-byte header.
    assert_eq!(SNAPSHOT_ENTITY_HEADER_LEN, 4);
}

// Phase 3e-4b: delta encoding regression checks. These exercise
// `delta_encode_tier` and the emission path's history bookkeeping.

#[test]
fn delta_encode_near_uses_full_state_when_no_base() {
    let current = vec![(7, near_state(100, 200)), (8, near_state(300, 400))];
    let encoded = delta_encode_near_tier(&current, None);
    assert_eq!(encoded.len(), 2);
    for entity in &encoded {
        assert_eq!(
            entity.field_mask & (SNAPSHOT_FIELD_X | SNAPSHOT_FIELD_Y),
            SNAPSHOT_FIELD_X | SNAPSHOT_FIELD_Y
        );
        assert!(entity.x.is_some());
        assert!(entity.y.is_some());
    }
}

#[test]
fn delta_encode_near_omits_unchanged_fields_against_base() {
    let mut base = SnapshotEntityMap::new();
    base.insert(7, near_state(100, 200));
    let current = vec![(7, near_state(100, 200))];
    let encoded = delta_encode_near_tier(&current, Some(&base));
    assert_eq!(encoded.len(), 1);
    assert_eq!(encoded[0].field_mask, 0, "no fields changed → mask = 0");
    assert!(encoded[0].x.is_none());
    assert!(encoded[0].y.is_none());
}

#[test]
fn delta_encode_near_emits_changed_field_only() {
    let mut base = SnapshotEntityMap::new();
    base.insert(7, near_state(100, 200));
    let current = vec![(7, near_state(150, 200))];
    let encoded = delta_encode_near_tier(&current, Some(&base));
    assert_eq!(encoded[0].field_mask, SNAPSHOT_FIELD_X);
    assert_eq!(encoded[0].x, Some(150));
    assert!(encoded[0].y.is_none());
}

#[test]
fn delta_encode_near_treats_new_entity_as_full_state() {
    let mut base = SnapshotEntityMap::new();
    base.insert(7, near_state(100, 200));
    let current = vec![(8, near_state(50, 50))];
    let encoded = delta_encode_near_tier(&current, Some(&base));
    assert_eq!(
        encoded[0].field_mask & (SNAPSHOT_FIELD_X | SNAPSHOT_FIELD_Y),
        SNAPSHOT_FIELD_X | SNAPSHOT_FIELD_Y
    );
    assert_eq!(encoded[0].x, Some(50));
    assert_eq!(encoded[0].y, Some(50));
}

#[test]
fn delta_encode_mid_omits_unchanged_position() {
    let mut base = SnapshotEntityMap::new();
    base.insert(7, near_state(100, 200));
    let current = vec![(7, 100, 200, 0)];
    let encoded = delta_encode_mid_tier(&current, Some(&base));
    assert_eq!(encoded.len(), 1);
    assert_eq!(encoded[0].field_mask, 0);
}

#[test]
fn server_emits_delta_when_last_acked_is_in_history() {
    let (state, alpha, beta, _gamma, rx_a, _rx_b, _rx_c) = place_three_aoi_clients();

    // First emission populates history; alpha receives a full snapshot.
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let first = rx_a.try_recv().expect("first");
    let first_sid = u16::from_le_bytes([first[2], first[3]]);
    assert_eq!(
        u16::from_le_bytes([first[4], first[5]]),
        0,
        "no ack yet → full snapshot"
    );

    // Simulate alpha acking the first snapshot.
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let client = state_guard.clients.get_mut(&alpha).unwrap();
        client.last_acked_snapshot_id = Some(first_sid);
    }

    // Second emission with the same beta position → delta against
    // the first snapshot. Beta entity should have field_mask = 0.
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 4;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let second = rx_a.try_recv().expect("second");
    assert_eq!(
        u16::from_le_bytes([second[4], second[5]]),
        first_sid,
        "base_id should reference the acked snapshot"
    );
    // near_count = 2 (self + beta), mid_count = 1. Self and beta are
    // compact delta entities: id(2) + field_mask(2). Since neither
    // moved, both masks are 0.
    assert_eq!(second[18], 2);
    assert_eq!(
        u16::from_le_bytes([second[21], second[22]]),
        alpha,
        "first near entity id is self"
    );
    assert_eq!(
        u16::from_le_bytes([second[23], second[24]]),
        0,
        "self hasn't changed → field_mask = 0 (no fields)"
    );
    assert_eq!(
        u16::from_le_bytes([second[25], second[26]]),
        beta,
        "second near entity id is beta"
    );
    assert_eq!(
        u16::from_le_bytes([second[27], second[28]]),
        0,
        "beta hasn't moved → field_mask = 0 (no fields)"
    );
}

#[test]
fn snapshot_history_evicts_oldest_when_capacity_exceeded() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let pos = large_position_from_client(player_id, &client_position_packet(0x1, 500, 500))
            .expect("pos");
        state_guard.set_client_position(player_id, pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&player_id).unwrap());
    }

    // Drive emissions past capacity. Each emission pushes one history
    // entry; after `SNAPSHOT_HISTORY_CAPACITY` emissions we should be
    // at capacity, and the next emission evicts the oldest.
    for tick in 1..=(SNAPSHOT_HISTORY_CAPACITY + 2) {
        {
            let mut state_guard = state.lock().expect("server state should not be poisoned");
            state_guard.arena_tick = (tick * 2) as u64;
        }
        emit_snapshots_for_tick_inner(&state, true);
        let _ = rx.try_recv();
    }

    let state_guard = state.lock().expect("server state should not be poisoned");
    let client = state_guard.clients.get(&player_id).expect("client present");
    assert!(
        client.snapshot_history.len() <= SNAPSHOT_HISTORY_CAPACITY,
        "history must not exceed capacity"
    );
}

#[test]
fn snapshot_history_trims_entries_older_than_last_acked() {
    let (state, alpha, _beta, _gamma, rx_a, _rx_b, _rx_c) = place_three_aoi_clients();

    // Emit a handful of snapshots so the history has multiple entries.
    for tick in 1..=5 {
        {
            let mut state_guard = state.lock().expect("server state should not be poisoned");
            state_guard.arena_tick = (tick * 2) as u64;
        }
        emit_snapshots_for_tick_inner(&state, true);
        let _ = rx_a.try_recv();
    }

    // Ack the 3rd snapshot; the next emission should drop entries 1+2.
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let client = state_guard.clients.get_mut(&alpha).unwrap();
        client.last_acked_snapshot_id = Some(3);
        state_guard.arena_tick = 12;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let _ = rx_a.try_recv();

    let state_guard = state.lock().expect("server state should not be poisoned");
    let client = state_guard.clients.get(&alpha).expect("client present");
    // After trim: entries with snapshot_id < 3 are gone; 3, 4, 5, 6 remain.
    let ids: Vec<u16> = client
        .snapshot_history
        .iter()
        .map(|e| e.snapshot_id)
        .collect();
    for &id in &ids {
        assert!(id >= 3, "history should not contain pre-ack snapshot {id}");
    }
}

#[test]
fn snapshot_history_never_evicts_stalled_active_base() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    for tick in 1..=(SNAPSHOT_HISTORY_CAPACITY + 8) {
        {
            let mut state_guard = state.lock().expect("server state should not be poisoned");
            state_guard.arena_tick = tick as u64;
            if tick == 2 {
                state_guard
                    .clients
                    .get_mut(&player_id)
                    .unwrap()
                    .last_acked_snapshot_id = Some(1);
            }
        }
        emit_snapshots_for_tick_inner(&state, true);
        let _ = rx.try_recv().unwrap();
    }

    let state_guard = state.lock().expect("server state should not be poisoned");
    let history = &state_guard.clients[&player_id].snapshot_history;
    assert_eq!(history.len(), SNAPSHOT_HISTORY_CAPACITY);
    assert!(
        history.iter().any(|entry| entry.snapshot_id == 1),
        "the active acknowledged base must survive a long ack stall"
    );
}

#[test]
fn snapshot_full_request_idempotently_forces_next_frame_full() {
    let (state, alpha, _beta, _gamma, rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let first = rx_a.try_recv().unwrap();
    let first_id = u16::from_le_bytes([first[2], first[3]]);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&alpha)
        .unwrap()
        .last_acked_snapshot_id = Some(first_id);

    let request = crate::snapshot_wire::SnapshotFullRequestPacket.encode();
    crate::connection::record_snapshot_full_request(&state, alpha, &request);
    crate::connection::record_snapshot_full_request(&state, alpha, &request);
    assert_eq!(
        state.lock().unwrap().clients[&alpha].last_acked_snapshot_id,
        None
    );

    state.lock().unwrap().arena_tick = 4;
    emit_snapshots_for_tick_inner(&state, true);
    let recovery = rx_a.try_recv().unwrap();
    let frame = SnapshotFrame::parse(&recovery).unwrap();
    assert!(frame.header.is_full());
    assert_eq!(frame.header.flags, SNAPSHOT_FLAG_FULL);
}

// Phase 3e-5: WATCH_DAMAGE events queue onto near-AOI recipients and
// drain into the next snapshot's events region.
fn watch_damage_event(victim: u16, shooter: u16) -> crate::snapshot::PendingWatchDamage {
    crate::snapshot::PendingWatchDamage {
        victim_id: victim,
        shooter_id: shooter,
        victim_life_seq: 1,
        shooter_life_seq: 2,
        weapon_data: 0x1234,
        damage: 100,
        energy_before: 600,
        energy_after: 500,
        server_tick: 12345,
        damage_seq: 99,
        projectile_id: 88,
        lethal: false,
    }
}

#[test]
fn queue_watch_damage_event_targets_near_aoi_plus_shooter() {
    // beta is in alpha's near AOI; gamma is in mid (outside near).
    // Queueing a WATCH_DAMAGE with anchor=alpha should land on
    // alpha + beta but NOT gamma — unless gamma is forced (i.e.
    // shooter).
    let (state, alpha, beta, gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    // Force-include gamma as the shooter to confirm forced recipients
    // get the event regardless of AOI tier.
    crate::snapshot::queue_watch_damage_event_to_near_aoi_inner(
        &state,
        alpha,
        &[gamma],
        watch_damage_event(alpha, gamma),
        true,
    );
    let state_guard = state.lock().expect("server state should not be poisoned");
    let alpha_queue = &state_guard.clients[&alpha]
        .pending_snapshot_events
        .watch_damage;
    let beta_queue = &state_guard.clients[&beta]
        .pending_snapshot_events
        .watch_damage;
    let gamma_queue = &state_guard.clients[&gamma]
        .pending_snapshot_events
        .watch_damage;
    assert_eq!(alpha_queue.len(), 1, "anchor receives its own event");
    assert_eq!(beta_queue.len(), 1, "near-AOI peer receives event");
    assert_eq!(
        gamma_queue.len(),
        1,
        "forced (shooter) recipient receives event even though in mid"
    );
}

#[test]
fn queue_watch_damage_event_skips_far_recipients() {
    // beta moved out of mid AOI — should not queue.
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let alpha_pos = large_position_from_client(alpha, &client_position_packet(0x1, 500, 500))
            .expect("alpha pos");
        // beta 5500 px away — far AOI.
        let beta_pos = large_position_from_client(beta, &client_position_packet(0x1, 5_500, 500))
            .expect("beta pos");
        state_guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&alpha).unwrap());
        state_guard.set_client_position(beta, beta_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&beta).unwrap());
    }
    crate::snapshot::queue_watch_damage_event_to_near_aoi_inner(
        &state,
        alpha,
        &[],
        watch_damage_event(alpha, 999),
        true,
    );
    let state_guard = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        state_guard.clients[&beta]
            .pending_snapshot_events
            .watch_damage
            .len(),
        0,
        "far recipient should not receive the event"
    );
}

#[test]
fn snapshot_emission_drains_pending_events_into_frame() {
    let (state, alpha, _beta, _gamma, rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    crate::snapshot::queue_watch_damage_event_to_near_aoi_inner(
        &state,
        alpha,
        &[],
        watch_damage_event(alpha, 42),
        true,
    );
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 2;
        // Confirm alpha has a pending event before emit.
        assert_eq!(
            state_guard.clients[&alpha]
                .pending_snapshot_events
                .watch_damage
                .len(),
            1
        );
    }
    emit_snapshots_for_tick_inner(&state, true);
    let packet = rx_a.try_recv().expect("alpha");
    // event_count is byte 20 of the header.
    assert_eq!(
        packet[20], 5,
        "snapshot should carry WATCH_DAMAGE plus private Rocket, lifecycle, prize, and observer-timeline state"
    );
    // Pending queue cleared after emit.
    let state_guard = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        state_guard.clients[&alpha]
            .pending_snapshot_events
            .watch_damage
            .len(),
        0,
        "pending events drained on emit"
    );
}

#[derive(Debug, PartialEq, Eq)]
struct DecodedSnapshotWatchDamage {
    victim_id: u16,
    shooter_id: u16,
    victim_life_seq: u32,
    shooter_life_seq: u32,
    weapon_data: u16,
    damage: u16,
    energy_before: u16,
    energy_after: u16,
    server_tick: u32,
    damage_seq: u32,
    projectile_id: u32,
    lethal: bool,
}

fn snapshot_watch_damage_events(packet: &[u8]) -> Vec<DecodedSnapshotWatchDamage> {
    assert_eq!(packet[0], SNAPSHOT_PACKET, "expected snapshot packet");
    let near_count = packet[18] as usize;
    let mid_count = packet[19] as usize;
    let event_count = packet[20] as usize;
    let mut offset = SNAPSHOT_HEADER_LEN;
    for _ in 0..near_count + mid_count {
        offset = skip_snapshot_entity(packet, offset);
    }

    let mut events = Vec::new();
    for _ in 0..event_count {
        assert!(
            offset + SNAPSHOT_EVENT_HEADER_LEN <= packet.len(),
            "snapshot event header truncated"
        );
        let event_type = packet[offset];
        let body_len = packet[offset + 1] as usize;
        let body = offset + SNAPSHOT_EVENT_HEADER_LEN;
        let end = body + body_len;
        assert!(end <= packet.len(), "snapshot event body truncated");
        if event_type == SNAPSHOT_EVENT_WATCH_DAMAGE {
            assert_eq!(body_len, SNAPSHOT_EVENT_WATCH_DAMAGE_BODY_LEN);
            events.push(DecodedSnapshotWatchDamage {
                victim_id: u16::from_le_bytes(packet[body..body + 2].try_into().unwrap()),
                shooter_id: u16::from_le_bytes(packet[body + 2..body + 4].try_into().unwrap()),
                weapon_data: u16::from_le_bytes(packet[body + 4..body + 6].try_into().unwrap()),
                damage: u16::from_le_bytes(packet[body + 6..body + 8].try_into().unwrap()),
                energy_before: u16::from_le_bytes(packet[body + 8..body + 10].try_into().unwrap()),
                energy_after: u16::from_le_bytes(packet[body + 10..body + 12].try_into().unwrap()),
                server_tick: u32::from_le_bytes(packet[body + 12..body + 16].try_into().unwrap()),
                damage_seq: u32::from_le_bytes(packet[body + 16..body + 20].try_into().unwrap()),
                lethal: packet[body + 20] != 0,
                victim_life_seq: u32::from_le_bytes(
                    packet[body + 21..body + 25].try_into().unwrap(),
                ),
                shooter_life_seq: u32::from_le_bytes(
                    packet[body + 25..body + 29].try_into().unwrap(),
                ),
                projectile_id: u32::from_le_bytes(packet[body + 29..body + 33].try_into().unwrap()),
            });
        }
        offset = end;
    }
    events
}

fn skip_snapshot_entity(packet: &[u8], offset: usize) -> usize {
    assert!(
        offset + SNAPSHOT_ENTITY_HEADER_LEN <= packet.len(),
        "snapshot entity header truncated"
    );
    let field_mask = u16::from_le_bytes(packet[offset + 2..offset + 4].try_into().unwrap());
    let mut len = SNAPSHOT_ENTITY_HEADER_LEN;
    for (field, field_len) in [
        (SNAPSHOT_FIELD_X, 2),
        (SNAPSHOT_FIELD_Y, 2),
        (SNAPSHOT_FIELD_VELOCITY_X, 2),
        (SNAPSHOT_FIELD_VELOCITY_Y, 2),
        (SNAPSHOT_FIELD_DIRECTION, 1),
        (SNAPSHOT_FIELD_ENERGY, 2),
        (SNAPSHOT_FIELD_WEAPON, 2),
        (SNAPSHOT_FIELD_STATUS, 1),
        (SNAPSHOT_FIELD_BOUNTY, 2),
        (SNAPSHOT_FIELD_PING, 1),
        (SNAPSHOT_FIELD_FLAG_TIMER, 2),
        (SNAPSHOT_FIELD_LIFE_SEQ, 4),
        (crate::snapshot_wire::SNAPSHOT_FIELD_SAMPLE_TIME, 5),
        (crate::snapshot_wire::SNAPSHOT_FIELD_MOTION_SEGMENT, 8),
    ] {
        if field_mask & field != 0 {
            len += field_len;
        }
    }
    assert!(
        offset + len <= packet.len(),
        "snapshot entity body truncated"
    );
    offset + len
}

fn snapshot_entity_ids(packet: &[u8]) -> (Vec<u16>, Vec<u16>) {
    assert_eq!(packet[0], SNAPSHOT_PACKET, "expected snapshot packet");
    let near_count = packet[18] as usize;
    let mid_count = packet[19] as usize;
    let mut offset = SNAPSHOT_HEADER_LEN;
    let mut near = Vec::with_capacity(near_count);
    let mut mid = Vec::with_capacity(mid_count);

    for _ in 0..near_count {
        assert!(
            offset + 2 <= packet.len(),
            "near snapshot entity id truncated"
        );
        near.push(u16::from_le_bytes(
            packet[offset..offset + 2].try_into().unwrap(),
        ));
        offset = skip_snapshot_entity(packet, offset);
    }
    for _ in 0..mid_count {
        assert!(
            offset + 2 <= packet.len(),
            "mid snapshot entity id truncated"
        );
        mid.push(u16::from_le_bytes(
            packet[offset..offset + 2].try_into().unwrap(),
        ));
        offset = skip_snapshot_entity(packet, offset);
    }

    (near, mid)
}

#[test]
fn authoritative_damage_reaches_victim_and_shooter_snapshot_streams() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let mut alpha_pos =
            large_position_from_client(alpha, &client_position_packet(0, 8_200, 8_200))
                .expect("alpha position");
        let mut beta_pos =
            large_position_from_client(beta, &client_position_packet(0, 8_260, 8_200))
                .expect("beta position");
        alpha_pos[21..23].copy_from_slice(&1_700_u16.to_le_bytes());
        beta_pos[21..23].copy_from_slice(&1_700_u16.to_le_bytes());
        state_guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&alpha).unwrap());
        state_guard.set_client_position(beta, beta_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&beta).unwrap());
        for player_id in [alpha, beta] {
            let client = state_guard
                .clients
                .get_mut(&player_id)
                .expect("client should exist");
            client.server_max_energy = 1_700;
            client.server_energy = 1_700;
            client.server_energy_remainder = 0.0;
            client.last_reported_energy = 1_700;
            client.alive = true;
            client.respawn_at_tick = None;
            client.life_seq = 1;
        }
    }

    let hit = crate::authority_projectiles::ProjectileHit {
        projectile_id: 1,
        owner_id: alpha,
        owner_life_seq: 1,
        victim_id: beta,
        victim_life_seq: 1,
        weapon_data: 66,
        x: 8_260,
        y: 8_200,
        tick: 12_345,
    };
    let damage = crate::server_damage::apply_server_detected_hit_with_broadcast(&state, &hit)
        .expect("bullet hit should apply authoritative damage");
    assert_eq!(damage.shooter_id, alpha);
    assert_eq!(damage.victim_id, beta);
    assert_eq!(damage.damage, 400);
    assert_eq!(damage.energy_before, 1_700);
    assert_eq!(damage.energy_after, 1_300);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);

    let expected = DecodedSnapshotWatchDamage {
        victim_id: beta,
        shooter_id: alpha,
        victim_life_seq: damage.victim_life_seq,
        shooter_life_seq: damage.shooter_life_seq,
        weapon_data: 66,
        damage: 400,
        energy_before: 1_700,
        energy_after: 1_300,
        server_tick: 12_345,
        damage_seq: damage.damage_seq,
        projectile_id: damage.projectile_id,
        lethal: false,
    };
    assert_eq!(
        snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_a)),
        vec![expected]
    );
    assert_eq!(
        snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_b)),
        vec![DecodedSnapshotWatchDamage {
            victim_id: beta,
            shooter_id: alpha,
            victim_life_seq: damage.victim_life_seq,
            shooter_life_seq: damage.shooter_life_seq,
            weapon_data: 66,
            damage: 400,
            energy_before: 1_700,
            energy_after: 1_300,
            server_tick: 12_345,
            damage_seq: damage.damage_seq,
            projectile_id: damage.projectile_id,
            lethal: false,
        }]
    );

    let state_guard = state.lock().expect("server state should not be poisoned");
    assert_eq!(state_guard.clients[&beta].server_energy, 1_300);
}

#[test]
fn aoi_snapshots_hide_dead_player_until_respawn_reintroduces_position() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let killed_bounty = {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let mut alpha_pos =
            large_position_from_client(alpha, &client_position_packet(0, 8_200, 8_200))
                .expect("alpha position");
        let mut beta_pos =
            large_position_from_client(beta, &client_position_packet(0, 8_260, 8_200))
                .expect("beta position");
        alpha_pos[21..23].copy_from_slice(&1_700_u16.to_le_bytes());
        beta_pos[21..23].copy_from_slice(&100_u16.to_le_bytes());
        state_guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&alpha).unwrap());
        state_guard.set_client_position(beta, beta_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&beta).unwrap());
        for player_id in [alpha, beta] {
            let client = state_guard
                .clients
                .get_mut(&player_id)
                .expect("client should exist");
            client.server_max_energy = 1_700;
            client.server_energy = if player_id == beta { 100 } else { 1_700 };
            client.server_energy_remainder = 0.0;
            client.last_reported_energy = client.server_energy;
            client.alive = true;
            client.respawn_at_tick = None;
            client.life_seq = 1;
        }
        state_guard.clients[&beta].server_bounty
    };

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let (alpha_near, alpha_mid) = snapshot_entity_ids(&read_non_reliable_from_channel(&rx_a));
    let (beta_near, beta_mid) = snapshot_entity_ids(&read_non_reliable_from_channel(&rx_b));
    assert_eq!(alpha_near, vec![alpha, beta]);
    assert!(alpha_mid.is_empty());
    assert_eq!(beta_near, vec![beta, alpha]);
    assert!(beta_mid.is_empty());

    let lethal = crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 21,
            owner_id: alpha,
            owner_life_seq: 1,
            victim_id: beta,
            victim_life_seq: 1,
            weapon_data: 66,
            x: 8_260,
            y: 8_200,
            tick: 66_001,
        },
    )
    .expect("lethal bullet hit should apply authoritative damage");
    assert!(lethal.lethal);
    assert_eq!(lethal.energy_before, 100);
    assert_eq!(lethal.energy_after, 0);

    for rx in [&rx_a, &rx_b] {
        assert_death_packet_eq_ignoring_green(
            &read_reliable_payload_from_channel(rx),
            &[
                PLAYER_DEATH,
                0xff,
                (alpha & 0xff) as u8,
                (alpha >> 8) as u8,
                (beta & 0xff) as u8,
                (beta >> 8) as u8,
                (killed_bounty & 0xff) as u8,
                (killed_bounty >> 8) as u8,
                0,
                0,
            ],
        );
        assert_score_update(&read_reliable_payload_from_channel(rx), beta, 0, 1);
        assert_score_update(&read_reliable_payload_from_channel(rx), alpha, 1, 0);
    }

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 4;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let alpha_dead_packet = read_non_reliable_from_channel(&rx_a);
    let beta_dead_packet = read_non_reliable_from_channel(&rx_b);
    let (alpha_near, alpha_mid) = snapshot_entity_ids(&alpha_dead_packet);
    let (beta_near, beta_mid) = snapshot_entity_ids(&beta_dead_packet);
    assert_eq!(
        alpha_near,
        vec![alpha],
        "dead beta should disappear from nearby players' AOI snapshots"
    );
    assert!(alpha_mid.is_empty());
    assert_eq!(
        beta_near,
        vec![alpha],
        "dead beta keeps a read-only death-view anchor for nearby live actors"
    );
    assert!(beta_mid.is_empty());
    assert_eq!(
        snapshot_watch_damage_events(&alpha_dead_packet),
        vec![DecodedSnapshotWatchDamage {
            victim_id: beta,
            shooter_id: alpha,
            victim_life_seq: 1,
            shooter_life_seq: 1,
            weapon_data: 66,
            damage: 400,
            energy_before: 100,
            energy_after: 0,
            server_tick: 66_001,
            damage_seq: lethal.damage_seq,
            projectile_id: lethal.projectile_id,
            lethal: true,
        }]
    );
    assert_eq!(
        snapshot_watch_damage_events(&beta_dead_packet),
        vec![DecodedSnapshotWatchDamage {
            victim_id: beta,
            shooter_id: alpha,
            victim_life_seq: 1,
            shooter_life_seq: 1,
            weapon_data: 66,
            damage: 400,
            energy_before: 100,
            energy_after: 0,
            server_tick: 66_001,
            damage_seq: lethal.damage_seq,
            projectile_id: lethal.projectile_id,
            lethal: true,
        }]
    );

    broadcast_position_observable(
        &state,
        beta,
        &canonical_respawn_position_packet(&state, beta),
    );
    {
        let state_guard = state.lock().expect("server state should not be poisoned");
        assert!(
            state_guard.clients[&beta].last_position.is_none(),
            "pre-gate respawn movement must not recreate an AOI position"
        );
    }
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 6;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let (alpha_near, alpha_mid) = snapshot_entity_ids(&read_non_reliable_from_channel(&rx_a));
    let (beta_near, beta_mid) = snapshot_entity_ids(&read_non_reliable_from_channel(&rx_b));
    assert_eq!(alpha_near, vec![alpha]);
    assert!(alpha_mid.is_empty());
    assert_eq!(beta_near, vec![alpha]);
    assert!(beta_mid.is_empty());

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard
            .clients
            .get_mut(&beta)
            .expect("beta should exist")
            .respawn_at_tick = Some(crate::continuum::continuum_tick_sub(
            crate::wire::current_tick(),
            1,
        ));
    }
    broadcast_position_observable(
        &state,
        beta,
        &canonical_respawn_position_packet(&state, beta),
    );
    drain_available(&rx_a);
    drain_available(&rx_b);
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let beta_client = state_guard.clients.get(&beta).expect("beta should exist");
        assert!(beta_client.alive);
        assert!(beta_client.respawn_at_tick.is_none());
        assert_eq!(beta_client.life_seq, 2);
        assert_eq!(beta_client.server_energy, 1_700);
        state_guard.arena_tick = 8;
    }
    // Restore the AOI fixture's nearby observer at the actual new respawn
    // point. Production now randomizes beta; proximity must follow the
    // committed position rather than presuming the old map-centre cell.
    let respawn_point = crate::arena::large_position_coordinates(
        state.lock().unwrap().clients[&beta]
            .last_position
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0, respawn_point.0, respawn_point.1),
    );
    drain_available(&rx_a);
    drain_available(&rx_b);
    emit_snapshots_for_tick_inner(&state, true);
    let (alpha_near, alpha_mid) = snapshot_entity_ids(&read_non_reliable_from_channel(&rx_a));
    let (beta_near, beta_mid) = snapshot_entity_ids(&read_non_reliable_from_channel(&rx_b));
    assert_eq!(
        alpha_near,
        vec![alpha, beta],
        "respawned beta should reappear to nearby players"
    );
    assert!(alpha_mid.is_empty());
    assert_eq!(
        beta_near,
        vec![beta, alpha],
        "respawned beta should receive self plus nearby alpha"
    );
    assert!(beta_mid.is_empty());

    let (baseline_tick, baseline_sequence, life_seq) = {
        let guard = state.lock().unwrap();
        let client = &guard.clients[&beta];
        (
            client.last_position_server_tick.unwrap(),
            client.last_position_sequence,
            client.life_seq,
        )
    };
    // No further client position/input is supplied. An admitted stationary
    // actor must stay visible; an anchor is never a substitute for admission.
    for elapsed_ticks in [1, 10, 50, 100, 300] {
        emit_snapshots_for_tick_at_tick(
            &state,
            crate::continuum::continuum_tick_add(baseline_tick, elapsed_ticks),
        );
        let (near, mid) = snapshot_entity_ids(&read_non_reliable_from_channel(&rx_a));
        assert!(
            near.contains(&beta) || mid.contains(&beta),
            "admitted idle life disappeared after {elapsed_ticks} ticks"
        );
        drain_available(&rx_b);
        let guard = state.lock().unwrap();
        let client = &guard.clients[&beta];
        assert_eq!(client.life_seq, life_seq);
        assert_eq!(
            client.last_position_sequence, baseline_sequence,
            "the test must not manufacture a new input to keep the actor visible"
        );
    }
}

#[test]
fn server_tick_projectile_damage_reaches_victim_and_shooter_snapshot_streams() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let now = Instant::now();
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let mut alpha_pos =
            large_position_from_client(alpha, &client_position_packet(0, 8_200, 8_200))
                .expect("alpha position");
        let mut beta_pos =
            large_position_from_client(beta, &client_position_packet(0, 8_260, 8_200))
                .expect("beta position");
        alpha_pos[21..23].copy_from_slice(&1_700_u16.to_le_bytes());
        beta_pos[21..23].copy_from_slice(&1_700_u16.to_le_bytes());
        state_guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&alpha).unwrap());
        state_guard.set_client_position(beta, beta_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&beta).unwrap());
        let alpha_frequency = state_guard.clients[&alpha].player.frequency;
        for player_id in [alpha, beta] {
            let client = state_guard
                .clients
                .get_mut(&player_id)
                .expect("client should exist");
            client.server_max_energy = 1_700;
            client.server_energy = 1_700;
            client.server_energy_remainder = 0.0;
            client.last_reported_energy = 1_700;
            client.alive = true;
            client.respawn_at_tick = None;
            client.life_seq = 1;
        }
        state_guard
            .clients
            .get_mut(&beta)
            .expect("beta should exist")
            .player
            .frequency = alpha_frequency.wrapping_add(1);
        let motion_tick = crate::continuum::continuum_tick_sub(
            crate::wire::current_tick(),
            u32::from(crate::state::DEFAULT_LAG_COMPENSATION_TICKS),
        );
        for (player_id, x, y) in [(alpha, 8_200_u16, 8_200_u16), (beta, 8_260, 8_200)] {
            let client = state_guard
                .clients
                .get_mut(&player_id)
                .expect("client should exist");
            // This fixture seeds an already-existing stationary life at the
            // rewind tick. Its birth must precede that accepted motion rather
            // than retain the later wall-clock join timestamp.
            client.life_started_tick = Some(motion_tick);
            let witness = crate::state::CanonicalPositionWitness {
                sequence: 0,
                life_seq: client.life_seq,
                state: crate::state::ServerSimMovement {
                    position_x: f32::from(x),
                    position_y: f32::from(y),
                    orientation: 0.0,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    energy: 1_700.0,
                    max_energy: 1_700.0,
                },
                wire_x: x,
                wire_y: y,
                wire_velocity_x: 0,
                wire_velocity_y: 0,
                direction: 0,
                door_seed: 0,
                door_phase_tick: motion_tick,
            };
            assert_eq!(
                client.append_actor_motion_witness(motion_tick, witness),
                Ok(())
            );
        }
        let alpha_runtime = state_guard
            .active_player_runtime_identity(alpha)
            .expect("alpha runtime");
        state_guard
            .clients
            .get_mut(&beta)
            .expect("beta should exist")
            .threat_presentation
            .observe_emitted(
                alpha_runtime,
                0xdafe,
                crate::continuum::continuum_tick_sub(
                    crate::wire::current_tick(),
                    crate::threat_presentation::THREAT_HARD_OPEN_TICKS,
                ),
            );
        state_guard.record_projectile(ServerProjectile {
            id: 1,
            shot_group_id: 1,
            shot_index: 0,
            owner_id: alpha,
            owner_life_seq: 1,
            combat_timeline: None,
            presentation_source: None,
            weapon_data: 66,
            x: 8_260,
            y: 8_200,
            timestamp: crate::wire::current_tick(),
            created_tick: crate::wire::current_tick(),
            velocity_x: 0,
            velocity_y: 0,
            expiry_tick: None,
            pos_x: 8_260.0,
            source_server_tick: None,
            precise_velocity: None,
            pos_y: 8_200.0,
            bounces_remaining: 0,
            behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        });
    }

    crate::server_tick::run_server_tick(&state, now);
    drain_available(&rx_a);
    drain_available(&rx_b);
    std::thread::sleep(Duration::from_millis(20));
    crate::server_tick::run_server_tick(&state, now + Duration::from_millis(17));

    let shooter_events = snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_a));
    let victim_events = snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_b));
    assert_eq!(shooter_events.len(), 1);
    assert_eq!(victim_events.len(), 1);
    assert_eq!(shooter_events, victim_events);
    let event = shooter_events.first().expect("damage event");
    assert_eq!(event.shooter_id, alpha);
    assert_eq!(event.victim_id, beta);
    assert_eq!(event.victim_life_seq, 1);
    assert_eq!(event.shooter_life_seq, 1);
    assert_eq!(event.weapon_data, 66);
    assert_eq!(event.damage, 400);
    assert_eq!(event.energy_before, 1_700);
    assert_eq!(event.energy_after, 1_300);
    assert!(!event.lethal);

    let state_guard = state.lock().expect("server state should not be poisoned");
    assert_eq!(state_guard.clients[&beta].server_energy, 1_300);
    assert_eq!(state_guard.active_projectiles.len(), 0);
}

#[test]
fn server_tick_stale_projectile_cannot_damage_respawned_life() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let now = Instant::now();
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let mut alpha_pos =
            large_position_from_client(alpha, &client_position_packet(0, 8_200, 8_200))
                .expect("alpha position");
        let mut beta_pos =
            large_position_from_client(beta, &client_position_packet(0, 8_260, 8_200))
                .expect("beta position");
        alpha_pos[21..23].copy_from_slice(&1_700_u16.to_le_bytes());
        beta_pos[21..23].copy_from_slice(&1_700_u16.to_le_bytes());
        state_guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&alpha).unwrap());
        state_guard.set_client_position(beta, beta_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&beta).unwrap());
        let alpha_frequency = state_guard.clients[&alpha].player.frequency;
        for player_id in [alpha, beta] {
            let client = state_guard
                .clients
                .get_mut(&player_id)
                .expect("client should exist");
            client.server_max_energy = 1_700;
            client.server_energy = 1_700;
            client.server_energy_remainder = 0.0;
            client.last_reported_energy = 1_700;
            client.alive = true;
            client.respawn_at_tick = None;
        }
        let stale_projectile_tick = 900;
        let beta_client = state_guard
            .clients
            .get_mut(&beta)
            .expect("beta should exist");
        beta_client.player.frequency = alpha_frequency.wrapping_add(1);
        beta_client.life_seq = 2;
        beta_client.life_started_tick = Some(stale_projectile_tick + 1);
        state_guard.record_projectile(ServerProjectile {
            id: 2,
            shot_group_id: 2,
            shot_index: 0,
            owner_id: alpha,
            owner_life_seq: 1,
            combat_timeline: None,
            presentation_source: None,
            weapon_data: 66,
            x: 8_260,
            y: 8_200,
            timestamp: stale_projectile_tick,
            created_tick: crate::wire::current_tick(),
            velocity_x: 0,
            velocity_y: 0,
            expiry_tick: None,
            pos_x: 8_260.0,
            source_server_tick: None,
            precise_velocity: None,
            pos_y: 8_200.0,
            bounces_remaining: 0,
            behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        });
    }

    crate::server_tick::run_server_tick(&state, now);
    drain_available(&rx_a);
    drain_available(&rx_b);
    std::thread::sleep(Duration::from_millis(20));
    crate::server_tick::run_server_tick(&state, now + Duration::from_millis(17));

    assert!(snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_a)).is_empty());
    assert!(snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_b)).is_empty());
    let state_guard = state.lock().expect("server state should not be poisoned");
    assert_eq!(state_guard.clients[&beta].server_energy, 1_700);
    assert_eq!(
        state_guard.active_projectiles.len(),
        1,
        "stale projectile is ignored for the new life but can still exist until expiry"
    );
}

#[test]
fn lethal_damage_broadcasts_death_scores_snapshot_and_gates_respawn() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let killed_bounty = {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let mut alpha_pos =
            large_position_from_client(alpha, &client_position_packet(0, 8_200, 8_200))
                .expect("alpha position");
        let mut beta_pos =
            large_position_from_client(beta, &client_position_packet(0, 8_260, 8_200))
                .expect("beta position");
        alpha_pos[21..23].copy_from_slice(&1_700_u16.to_le_bytes());
        beta_pos[21..23].copy_from_slice(&100_u16.to_le_bytes());
        state_guard.set_client_position(alpha, alpha_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&alpha).unwrap());
        state_guard.set_client_position(beta, beta_pos);
        seed_snapshot_motion(state_guard.clients.get_mut(&beta).unwrap());
        for player_id in [alpha, beta] {
            let client = state_guard
                .clients
                .get_mut(&player_id)
                .expect("client should exist");
            client.server_max_energy = 1_700;
            client.server_energy = if player_id == beta { 100 } else { 1_700 };
            client.server_energy_remainder = 0.0;
            client.last_reported_energy = client.server_energy;
            client.alive = true;
            client.respawn_at_tick = None;
            client.life_seq = 1;
        }
        state_guard.clients[&beta].server_bounty
    };

    let lethal = crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 2,
            owner_id: alpha,
            owner_life_seq: 1,
            victim_id: beta,
            victim_life_seq: 1,
            weapon_data: 66,
            x: 8_260,
            y: 8_200,
            tick: 22_222,
        },
    )
    .expect("lethal bullet hit should apply authoritative damage");
    assert!(lethal.lethal);
    assert_eq!(lethal.energy_before, 100);
    assert_eq!(lethal.energy_after, 0);

    for rx in [&rx_a, &rx_b] {
        assert_death_packet_eq_ignoring_green(
            &read_reliable_payload_from_channel(rx),
            &[
                PLAYER_DEATH,
                0xff,
                (alpha & 0xff) as u8,
                (alpha >> 8) as u8,
                (beta & 0xff) as u8,
                (beta >> 8) as u8,
                (killed_bounty & 0xff) as u8,
                (killed_bounty >> 8) as u8,
                0,
                0,
            ],
        );
        assert_score_update(&read_reliable_payload_from_channel(rx), beta, 0, 1);
        assert_score_update(&read_reliable_payload_from_channel(rx), alpha, 1, 0);
    }

    {
        let state_guard = state.lock().expect("server state should not be poisoned");
        let beta_client = state_guard.clients.get(&beta).expect("beta should exist");
        assert!(!beta_client.alive);
        assert_eq!(beta_client.server_energy, 0);
        assert_eq!(beta_client.life_seq, 2);
        let respawn_delay_ticks = crate::continuum::continuum_tick_diff(
            beta_client
                .respawn_at_tick
                .expect("lethal damage should arm respawn timer"),
            lethal.server_tick,
        );
        assert_eq!(
            respawn_delay_ticks,
            crate::arena_settings::arena_respawn_delay_ticks() as i32,
        );
    }

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 2;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let expected = DecodedSnapshotWatchDamage {
        victim_id: beta,
        shooter_id: alpha,
        victim_life_seq: 1,
        shooter_life_seq: 1,
        weapon_data: 66,
        damage: 400,
        energy_before: 100,
        energy_after: 0,
        server_tick: 22_222,
        damage_seq: lethal.damage_seq,
        projectile_id: lethal.projectile_id,
        lethal: true,
    };
    assert_eq!(
        snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_a)),
        vec![expected]
    );
    assert_eq!(
        snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_b)),
        vec![DecodedSnapshotWatchDamage {
            victim_id: beta,
            shooter_id: alpha,
            victim_life_seq: 1,
            shooter_life_seq: 1,
            weapon_data: 66,
            damage: 400,
            energy_before: 100,
            energy_after: 0,
            server_tick: 22_222,
            damage_seq: lethal.damage_seq,
            projectile_id: lethal.projectile_id,
            lethal: true,
        }]
    );

    broadcast_position_observable(
        &state,
        beta,
        &canonical_respawn_position_packet(&state, beta),
    );
    {
        let state_guard = state.lock().expect("server state should not be poisoned");
        let beta_client = state_guard.clients.get(&beta).expect("beta should exist");
        assert!(
            !beta_client.alive,
            "position before respawn gate must be ignored"
        );
        assert_eq!(beta_client.server_energy, 0);
    }

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard
            .clients
            .get_mut(&beta)
            .expect("beta should exist")
            .respawn_at_tick = Some(crate::continuum::continuum_tick_sub(
            crate::wire::current_tick(),
            1,
        ));
    }
    broadcast_position_observable(
        &state,
        beta,
        &canonical_respawn_position_packet(&state, beta),
    );
    let state_guard = state.lock().expect("server state should not be poisoned");
    let beta_client = state_guard.clients.get(&beta).expect("beta should exist");
    assert!(beta_client.alive);
    assert!(beta_client.respawn_at_tick.is_none());
    assert_eq!(beta_client.life_seq, 2);
    assert_eq!(beta_client.server_energy, beta_client.server_max_energy);
    assert_eq!(beta_client.server_energy, 1_700);
    drop(state_guard);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let post_respawn = crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 3,
            owner_id: alpha,
            owner_life_seq: 1,
            victim_id: beta,
            victim_life_seq: 2,
            weapon_data: 66,
            x: 8_260,
            y: 8_200,
            tick: 33_333,
        },
    )
    .expect("post-respawn bullet hit should apply authoritative damage");
    assert!(!post_respawn.lethal);
    assert_eq!(post_respawn.victim_life_seq, 2);
    assert_eq!(post_respawn.energy_before, 1_700);
    assert_eq!(post_respawn.energy_after, 1_300);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 4;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let expected = DecodedSnapshotWatchDamage {
        victim_id: beta,
        shooter_id: alpha,
        victim_life_seq: 2,
        shooter_life_seq: 1,
        weapon_data: 66,
        damage: 400,
        energy_before: 1_700,
        energy_after: 1_300,
        server_tick: 33_333,
        damage_seq: post_respawn.damage_seq,
        projectile_id: post_respawn.projectile_id,
        lethal: false,
    };
    assert_eq!(
        snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_a)),
        vec![expected]
    );
    assert_eq!(
        snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_b)),
        vec![DecodedSnapshotWatchDamage {
            victim_id: beta,
            shooter_id: alpha,
            victim_life_seq: 2,
            shooter_life_seq: 1,
            weapon_data: 66,
            damage: 400,
            energy_before: 1_700,
            energy_after: 1_300,
            server_tick: 33_333,
            damage_seq: post_respawn.damage_seq,
            projectile_id: post_respawn.projectile_id,
            lethal: false,
        }]
    );
    drain_available(&rx_a);
    drain_available(&rx_b);

    let second_killed_bounty = state
        .lock()
        .expect("server state should not be poisoned")
        .clients[&beta]
        .server_bounty;
    let mut second_lethal = None;
    for (index, tick) in [44_441, 44_442, 44_443, 44_444].into_iter().enumerate() {
        let hit = crate::server_damage::apply_server_detected_hit_with_broadcast(
            &state,
            &crate::authority_projectiles::ProjectileHit {
                projectile_id: 10 + index as u32,
                owner_id: alpha,
                owner_life_seq: 1,
                victim_id: beta,
                victim_life_seq: 2,
                weapon_data: 66,
                x: 8_260,
                y: 8_200,
                tick,
            },
        )
        .expect("second-cycle bullet hit should apply authoritative damage");
        if index == 3 {
            second_lethal = Some(hit);
        }
    }
    let second_lethal = second_lethal.expect("fourth post-respawn hit should be lethal");
    assert!(second_lethal.lethal);
    assert_eq!(second_lethal.victim_life_seq, 2);
    assert_eq!(second_lethal.energy_before, 100);
    assert_eq!(second_lethal.energy_after, 0);

    for rx in [&rx_a, &rx_b] {
        assert_death_packet_eq_ignoring_green(
            &read_reliable_payload_from_channel(rx),
            &[
                PLAYER_DEATH,
                0xff,
                (alpha & 0xff) as u8,
                (alpha >> 8) as u8,
                (beta & 0xff) as u8,
                (beta >> 8) as u8,
                (second_killed_bounty & 0xff) as u8,
                (second_killed_bounty >> 8) as u8,
                0,
                0,
            ],
        );
        assert_score_update(&read_reliable_payload_from_channel(rx), beta, 0, 2);
        assert_score_update(&read_reliable_payload_from_channel(rx), alpha, 2, 0);
    }

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 6;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let expected_second_lethal = DecodedSnapshotWatchDamage {
        victim_id: beta,
        shooter_id: alpha,
        victim_life_seq: 2,
        shooter_life_seq: 1,
        weapon_data: 66,
        damage: 400,
        energy_before: 100,
        energy_after: 0,
        server_tick: 44_444,
        damage_seq: second_lethal.damage_seq,
        projectile_id: second_lethal.projectile_id,
        lethal: true,
    };
    let shooter_events = snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_a));
    let victim_events = snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_b));
    assert_eq!(shooter_events.len(), 4);
    assert_eq!(shooter_events.last(), Some(&expected_second_lethal));
    assert_eq!(
        victim_events, shooter_events,
        "the victim life cutover must not erase accepted nonlethal predecessors and expose only the lethal tail"
    );

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard
            .clients
            .get_mut(&beta)
            .expect("beta should exist")
            .respawn_at_tick = Some(crate::continuum::continuum_tick_sub(
            crate::wire::current_tick(),
            1,
        ));
    }
    broadcast_position_observable(
        &state,
        beta,
        &canonical_respawn_position_packet(&state, beta),
    );
    drain_available(&rx_a);
    drain_available(&rx_b);

    let second_post_respawn = crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 4,
            owner_id: alpha,
            owner_life_seq: 1,
            victim_id: beta,
            victim_life_seq: 3,
            weapon_data: 66,
            x: 8_260,
            y: 8_200,
            tick: 55_555,
        },
    )
    .expect("second post-respawn bullet hit should apply authoritative damage");
    assert_eq!(second_post_respawn.victim_life_seq, 3);
    assert_eq!(second_post_respawn.energy_before, 1_700);
    assert_eq!(second_post_respawn.energy_after, 1_300);
    assert!(!second_post_respawn.lethal);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = 8;
    }
    emit_snapshots_for_tick_inner(&state, true);
    let expected_second_post_respawn = DecodedSnapshotWatchDamage {
        victim_id: beta,
        shooter_id: alpha,
        victim_life_seq: 3,
        shooter_life_seq: 1,
        weapon_data: 66,
        damage: 400,
        energy_before: 1_700,
        energy_after: 1_300,
        server_tick: 55_555,
        damage_seq: second_post_respawn.damage_seq,
        projectile_id: second_post_respawn.projectile_id,
        lethal: false,
    };
    assert_eq!(
        snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_a)),
        vec![expected_second_post_respawn]
    );
    assert_eq!(
        snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_b)),
        vec![DecodedSnapshotWatchDamage {
            victim_id: beta,
            shooter_id: alpha,
            victim_life_seq: 3,
            shooter_life_seq: 1,
            weapon_data: 66,
            damage: 400,
            energy_before: 1_700,
            energy_after: 1_300,
            server_tick: 55_555,
            damage_seq: second_post_respawn.damage_seq,
            projectile_id: second_post_respawn.projectile_id,
            lethal: false,
        }]
    );
}

#[test]
fn post_respawn_bomb_damage_covers_direct_proximity_splash_and_self_splash() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let (tx_c, rx_c) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    let gamma = register_connection(&state, tx_c);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    join_arena(&state, gamma, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        let alpha_frequency = state_guard.clients[&alpha].player.frequency;
        state_guard
            .clients
            .get_mut(&beta)
            .expect("beta should exist")
            .player
            .frequency = alpha_frequency.wrapping_add(1);
        state_guard
            .clients
            .get_mut(&gamma)
            .expect("gamma should exist")
            .player
            .frequency = alpha_frequency.wrapping_add(2);
        set_damage_fixture_client(&mut state_guard, alpha, 8_000, 8_200, 1_700, 1);
        set_damage_fixture_client(&mut state_guard, beta, 8_500, 8_200, 100, 1);
        set_damage_fixture_client(&mut state_guard, gamma, 9_000, 8_200, 1_700, 1);
    }

    let lethal = crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 80,
            owner_id: alpha,
            owner_life_seq: 1,
            victim_id: beta,
            victim_life_seq: 1,
            weapon_data: 66,
            x: 8_500,
            y: 8_200,
            tick: 80_000,
        },
    )
    .expect("lethal setup hit should apply");
    assert!(lethal.lethal);

    emit_snapshot_at_tick(&state, 2);
    let _ = read_non_reliable_from_channel(&rx_a);
    let _ = read_non_reliable_from_channel(&rx_b);
    let _ = read_non_reliable_from_channel(&rx_c);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard
            .clients
            .get_mut(&beta)
            .expect("beta should exist")
            .respawn_at_tick = Some(crate::continuum::continuum_tick_sub(
            crate::wire::current_tick(),
            1,
        ));
    }
    broadcast_position_observable(
        &state,
        beta,
        &canonical_respawn_position_packet(&state, beta),
    );
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        assert_eq!(state_guard.clients[&beta].life_seq, 2);
        set_damage_fixture_client(&mut state_guard, alpha, 8_000, 8_200, 1_700, 1);
        set_damage_fixture_client(&mut state_guard, beta, 8_500, 8_200, 1_700, 2);
        set_damage_fixture_client(&mut state_guard, gamma, 9_000, 8_200, 1_700, 1);
    }

    let direct_bomb = 3 | (2 << 5);
    let direct_tick = crate::wire::current_tick();
    let direct = crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 81,
            owner_id: alpha,
            owner_life_seq: 1,
            victim_id: beta,
            victim_life_seq: 2,
            weapon_data: direct_bomb,
            x: 8_500,
            y: 8_200,
            tick: direct_tick,
        },
    )
    .expect("post-respawn direct bomb should apply");
    assert_eq!(direct.victim_life_seq, 2);
    assert_eq!(direct.damage, 750);
    assert_eq!(direct.energy_before, 1_700);
    assert_eq!(direct.energy_after, 950);

    emit_snapshot_at_tick(&state, 4);
    let direct_events = snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_a));
    assert_eq!(direct_events.len(), 1);
    assert_eq!(direct_events[0].projectile_id, 81);
    assert_eq!(direct_events[0].victim_id, beta);
    assert_eq!(direct_events[0].victim_life_seq, 2);
    assert_eq!(direct_events[0].damage, 750);
    let _ = read_non_reliable_from_channel(&rx_b);
    let _ = read_non_reliable_from_channel(&rx_c);

    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        set_damage_fixture_client(&mut state_guard, alpha, 8_200, 8_200, 1_700, 1);
        set_damage_fixture_client(&mut state_guard, beta, 8_380, 8_200, 1_700, 2);
        set_damage_fixture_client(&mut state_guard, gamma, 8_330, 8_200, 1_700, 1);
    }

    let proximity_bomb = 4 | (2 << 5);
    let proximity_tick = crate::wire::current_tick();
    let proximity = crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 82,
            owner_id: alpha,
            owner_life_seq: 1,
            victim_id: beta,
            victim_life_seq: 2,
            weapon_data: proximity_bomb,
            x: 8_290,
            y: 8_200,
            tick: proximity_tick,
        },
    )
    .expect("post-respawn proximity bomb should apply");
    assert_eq!(proximity.victim_life_seq, 2);
    assert_eq!(proximity.damage, 468);

    emit_snapshot_at_tick(&state, 6);
    let mut proximity_events = snapshot_watch_damage_events(&read_non_reliable_from_channel(&rx_a));
    proximity_events.sort_by_key(|event| event.victim_id);
    assert_eq!(proximity_events.len(), 3);
    let self_splash = proximity_events
        .iter()
        .find(|event| event.victim_id == alpha)
        .expect("self-splash event");
    assert_eq!(self_splash.shooter_id, alpha);
    assert_eq!(self_splash.projectile_id, 82);
    assert_eq!(self_splash.damage, 468);
    assert_eq!(self_splash.energy_before, 1_700);
    assert_eq!(self_splash.energy_after, 1_232);

    let primary = proximity_events
        .iter()
        .find(|event| event.victim_id == beta)
        .expect("primary proximity event");
    assert_eq!(primary.shooter_id, alpha);
    assert_eq!(primary.victim_life_seq, 2);
    assert_eq!(primary.projectile_id, 82);
    assert_eq!(primary.damage, 468);
    assert_eq!(primary.energy_before, 1_700);
    assert_eq!(primary.energy_after, 1_232);

    let bystander_splash = proximity_events
        .iter()
        .find(|event| event.victim_id == gamma)
        .expect("bystander splash event");
    assert_eq!(bystander_splash.shooter_id, alpha);
    assert_eq!(bystander_splash.projectile_id, 82);
    assert_eq!(bystander_splash.damage, 625);
    assert_eq!(bystander_splash.energy_before, 1_700);
    assert_eq!(bystander_splash.energy_after, 1_075);

    let state_guard = state.lock().expect("server state should not be poisoned");
    assert_eq!(state_guard.clients[&alpha].server_energy, 1_232);
    assert_eq!(state_guard.clients[&beta].server_energy, 1_232);
    assert_eq!(state_guard.clients[&gamma].server_energy, 1_075);
}

#[test]
fn splash_eligibility_uses_lag_comp_rewound_positions() {
    // [REVIEW H2] Splash victim selection must use the same lag-comp
    // rewound position the primary hull hit used. Level-2 bomb splash
    // radius is 240 px (pinned by the falloff numbers in
    // `post_respawn_bomb_damage_covers_direct_proximity_splash_and_self_splash`).
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let (tx_c, _rx_c) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    let gamma = register_connection(&state, tx_c);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    join_arena(&state, gamma, 0);

    let proximity_bomb = 4 | (2 << 5);
    let fixture = |state: &Arc<Mutex<ServerState>>, gamma_history_x: f32, history_tick: u32| {
        let mut g = state.lock().expect("server state should not be poisoned");
        let alpha_frequency = g.clients[&alpha].player.frequency;
        g.clients.get_mut(&beta).expect("beta").player.frequency = alpha_frequency.wrapping_add(1);
        g.clients.get_mut(&gamma).expect("gamma").player.frequency =
            alpha_frequency.wrapping_add(2);
        set_damage_fixture_client(&mut g, alpha, 8_200, 8_200, 1_700, 1);
        set_damage_fixture_client(&mut g, beta, 8_380, 8_200, 1_700, 1);
        // Gamma's CURRENT position is well inside the splash radius
        // (40 px from the explosion center at 8_290).
        set_damage_fixture_client(&mut g, gamma, 8_330, 8_200, 1_700, 1);
        let shooter = g.clients.get_mut(&alpha).expect("alpha");
        shooter.lag_compensation_ticks = 10;
        let bystander = g.clients.get_mut(&gamma).expect("gamma");
        bystander.position_history.clear();
        bystander
            .position_history
            .push_back(crate::state::RecentPositionSample {
                tick: history_tick,
                x: gamma_history_x,
                y: 8_200.0,
            });
    };

    // Scenario A — the dodge: gamma's REWOUND position (610 px out) is
    // outside the radius even though their current position is inside.
    // No splash may land.
    fixture(&state, 8_900.0, 81_990);
    crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 90,
            owner_id: alpha,
            owner_life_seq: 1,
            victim_id: beta,
            victim_life_seq: 1,
            weapon_data: proximity_bomb,
            x: 8_290,
            y: 8_200,
            tick: 82_000,
        },
    )
    .expect("primary proximity hit should apply");
    {
        let g = state.lock().expect("server state should not be poisoned");
        assert!(
            g.clients[&beta].server_energy < 1_700,
            "primary victim still takes the hit"
        );
        assert_eq!(
            g.clients[&gamma].server_energy, 1_700,
            "a bystander whose rewound position dodged the blast must not be splashed"
        );
    }

    // Control — rewound position inside the radius (matching current):
    // the splash applies with the pinned falloff damage (625 at 40 px).
    fixture(&state, 8_330.0, 82_990);
    crate::server_damage::apply_server_detected_hit_with_broadcast(
        &state,
        &crate::authority_projectiles::ProjectileHit {
            projectile_id: 91,
            owner_id: alpha,
            owner_life_seq: 1,
            victim_id: beta,
            victim_life_seq: 1,
            weapon_data: proximity_bomb,
            x: 8_290,
            y: 8_200,
            tick: 83_000,
        },
    )
    .expect("control proximity hit should apply");
    let g = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        g.clients[&gamma].server_energy, 1_075,
        "a bystander inside the rewound radius takes the pinned splash damage"
    );
}

fn set_damage_fixture_client(
    state: &mut ServerState,
    player_id: u16,
    x: u16,
    y: u16,
    energy: u16,
    life_seq: u32,
) {
    let mut position = large_position_from_client(player_id, &client_position_packet(0, x, y))
        .expect("fixture position should decode");
    position[21..23].copy_from_slice(&energy.to_le_bytes());
    state.set_client_position(player_id, position);
    seed_snapshot_motion(state.clients.get_mut(&player_id).unwrap());
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("client should exist");
    client.alive = true;
    client.respawn_at_tick = None;
    client.life_seq = life_seq;
    client.server_max_energy = 1_700;
    client.server_energy = energy;
    client.server_energy_remainder = 0.0;
    client.last_reported_energy = energy;
}

fn emit_snapshot_at_tick(state: &Arc<Mutex<ServerState>>, tick: u64) {
    {
        let mut state_guard = state.lock().expect("server state should not be poisoned");
        state_guard.arena_tick = tick;
    }
    emit_snapshots_for_tick_inner(state, true);
}

fn dedicated_weapon_payload(
    player_id: u16,
    session_id: u64,
    life_seq: u32,
) -> crate::realtime_event::RealtimeEventPayload {
    crate::realtime_event::RealtimeEventPayload::Weapon(
        crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: player_id,
            source_session_id: session_id,
            source_life_seq: life_seq,
            shot_group_id: 1,
            x: 8_200.0,
            y: 8_200.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.0,
            frequency: 0,
            ship: 1,
            weapon_data: 2,
        },
    )
}

fn dedicated_envelope(
    recipient: crate::authority_identity::PlayerRuntimeIdentity,
    event_id: u32,
) -> crate::snapshot::PendingRealtimeEventEnvelope {
    crate::snapshot::PendingRealtimeEventEnvelope {
        flight_transition: false,
        recipient,
        event_id,
        server_tick: event_id,
        payload: dedicated_weapon_payload(
            recipient.player_id(),
            recipient.session.session_id.get(),
            recipient.life.life_seq,
        ),
    }
}

#[test]
fn nearby_hostile_projectile_emits_self_contained_exact_state_once() {
    let (state, alpha, beta, _gamma, rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let (source, exact) = {
        let mut guard = state.lock().expect("state");
        guard.clients.get_mut(&beta).expect("beta").player.frequency = 1;
        let source_runtime = guard
            .active_player_runtime_identity(beta)
            .expect("beta runtime");
        let source = crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: beta,
            source_session_id: source_runtime.session.session_id.get(),
            source_life_seq: source_runtime.life.life_seq,
            shot_group_id: 41,
            x: 1_500.0,
            y: 500.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.5,
            frequency: 1,
            ship: guard.clients[&beta].player.ship,
            weapon_data: 2,
        };
        let created_tick = crate::wire::current_tick();
        guard.record_projectile(ServerProjectile {
            id: 99,
            shot_group_id: 41,
            shot_index: 0,
            owner_id: beta,
            owner_life_seq: source_runtime.life.life_seq,
            presentation_source: Some(source),
            combat_timeline: None,
            weapon_data: 2,
            x: 550,
            y: 500,
            timestamp: created_tick,
            created_tick,
            velocity_x: 1_000,
            velocity_y: 0,
            expiry_tick: Some(crate::continuum::continuum_tick_add(created_tick, 100)),
            pos_x: 550.0,
            source_server_tick: None,
            precise_velocity: None,
            pos_y: 500.0,
            bounces_remaining: 2,
            behavior: crate::gameplay_rules::ProjectileBehavior::BouncingBullet,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        });
        let exact = crate::threat_presentation::ProjectilePresentationIdentity::projectile(
            source_runtime,
            41,
            99,
        )
        .expect("exact projectile");
        (source, exact)
    };

    emit_snapshot_at_tick(&state, 2);
    let events = rx_a
        .try_iter()
        .filter_map(|packet| crate::realtime_event::AuthenticatedRealtimeEvent::parse(&packet).ok())
        .collect::<Vec<_>>();
    let projectile_states = events
        .iter()
        .filter_map(|event| match event.payload {
            crate::realtime_event::RealtimeEventPayload::ProjectileState(state) => Some(state),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(projectile_states.len(), 1);
    let projectile = projectile_states[0];
    assert_eq!(projectile.source, source);
    assert_eq!(projectile.projectile_id, 99);
    assert_eq!(projectile.position_x, 550.0);
    assert_eq!(projectile.velocity_x, 100.0);
    assert_eq!(projectile.bounces_remaining, 2);
    let guard = state.lock().expect("state");
    let contract = guard.clients[&alpha]
        .threat_presentation
        .exact_projectile_readiness(exact, crate::wire::current_tick())
        .contract
        .expect("exact emitted contract");
    assert!(contract.first_emitted_tick.is_some());
    assert_eq!(contract.queued_event_id, events[0].event_id);
}

#[test]
fn dedicated_event_queue_accepts_exact_max_rejects_max_plus_one_and_retries_after_drain() {
    let (state, alpha, _beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let mut guard = state.lock().expect("state");
    let recipient = guard.active_player_runtime_identity(alpha).unwrap();
    let template = dedicated_envelope(recipient, 7);
    guard
        .clients
        .get_mut(&alpha)
        .unwrap()
        .pending_realtime_events
        .queue
        .extend(std::iter::repeat_n(
            template,
            crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY - 1,
        ));
    let recipients = [recipient];
    let reservation = crate::snapshot::reserve_realtime_event_deliveries(
        &mut guard,
        &[&recipients],
        crate::snapshot::RealtimeEventPressureDisposition::Deferred,
    )
    .expect("exact maximum must reserve");
    reservation
        .commit(
            &mut guard,
            vec![crate::snapshot::RealtimeEventDelivery {
                recipients: recipients.to_vec(),
                server_tick: 8,
                payload: dedicated_weapon_payload(
                    alpha,
                    recipient.session.session_id.get(),
                    recipient.life.life_seq,
                ),
            }],
        )
        .unwrap();
    assert_eq!(
        guard.clients[&alpha].pending_realtime_events.queue.len(),
        crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY
    );
    assert_eq!(
        guard.clients[&alpha]
            .pending_realtime_events
            .queue
            .front()
            .unwrap()
            .event_id,
        7,
        "exact admission never evicts the oldest fact"
    );
    assert!(
        crate::snapshot::reserve_realtime_event_deliveries(
            &mut guard,
            &[&recipients],
            crate::snapshot::RealtimeEventPressureDisposition::Deferred,
        )
        .is_none(),
        "maximum plus one must fail before mutation"
    );
    assert_eq!(guard.realtime_event_queue_overflows, 1);
    assert_eq!(guard.realtime_event_actions_deferred, 1);

    guard
        .clients
        .get_mut(&alpha)
        .unwrap()
        .pending_realtime_events
        .queue
        .pop_front();
    let retry = crate::snapshot::reserve_realtime_event_deliveries(
        &mut guard,
        &[&recipients],
        crate::snapshot::RealtimeEventPressureDisposition::Deferred,
    )
    .expect("a retry after one drained slot must reserve");
    retry
        .commit(
            &mut guard,
            vec![crate::snapshot::RealtimeEventDelivery {
                recipients: recipients.to_vec(),
                server_tick: 9,
                payload: dedicated_weapon_payload(
                    alpha,
                    recipient.session.session_id.get(),
                    recipient.life.life_seq,
                ),
            }],
        )
        .unwrap();
    assert_eq!(
        guard.realtime_event_queue_peak,
        crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY
    );
    guard
        .clients
        .get_mut(&alpha)
        .unwrap()
        .reset_server_runtime_for_ship_at_tick(10);
    assert_eq!(
        guard.realtime_event_queue_current(),
        0,
        "life reset drains the dedicated queue"
    );
}

#[test]
fn dedicated_event_ids_advance_only_for_each_exact_recipient_group() {
    let (state, alpha, beta, gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let mut guard = state.lock().expect("state");
    let alpha_runtime = guard.active_player_runtime_identity(alpha).unwrap();
    let beta_runtime = guard.active_player_runtime_identity(beta).unwrap();
    let gamma_runtime = guard.active_player_runtime_identity(gamma).unwrap();
    let alpha_only = vec![alpha_runtime];
    let beta_only = vec![beta_runtime];
    let alpha_beta = vec![alpha_runtime, beta_runtime];
    let groups = [
        alpha_only.as_slice(),
        beta_only.as_slice(),
        alpha_beta.as_slice(),
    ];
    let reservation = crate::snapshot::reserve_realtime_event_deliveries(
        &mut guard,
        &groups,
        crate::snapshot::RealtimeEventPressureDisposition::Rejected,
    )
    .expect("exact sparse recipient groups fit");
    reservation
        .commit(
            &mut guard,
            vec![
                crate::snapshot::RealtimeEventDelivery {
                    recipients: alpha_only,
                    server_tick: 1,
                    payload: dedicated_weapon_payload(
                        gamma,
                        gamma_runtime.session.session_id.get(),
                        gamma_runtime.life.life_seq,
                    ),
                },
                crate::snapshot::RealtimeEventDelivery {
                    recipients: beta_only,
                    server_tick: 2,
                    payload: dedicated_weapon_payload(
                        gamma,
                        gamma_runtime.session.session_id.get(),
                        gamma_runtime.life.life_seq,
                    ),
                },
                crate::snapshot::RealtimeEventDelivery {
                    recipients: alpha_beta,
                    server_tick: 3,
                    payload: dedicated_weapon_payload(
                        gamma,
                        gamma_runtime.session.session_id.get(),
                        gamma_runtime.life.life_seq,
                    ),
                },
            ],
        )
        .unwrap();

    assert_eq!(
        guard.clients[&alpha]
            .pending_realtime_events
            .queue
            .iter()
            .map(|event| event.event_id)
            .collect::<Vec<_>>(),
        vec![1, 2],
    );
    assert_eq!(
        guard.clients[&beta]
            .pending_realtime_events
            .queue
            .iter()
            .map(|event| event.event_id)
            .collect::<Vec<_>>(),
        vec![1, 2],
    );
    assert!(
        guard.clients[&gamma]
            .pending_realtime_events
            .queue
            .is_empty()
    );
    assert_eq!(guard.clients[&gamma].next_realtime_event_id, 0);
}

#[test]
fn dedicated_event_capacity_covers_768_client_weapon_damage_burst_and_one_pacing_backlog() {
    let (state, alpha, _beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    let mut guard = state.lock().expect("state");
    let template = guard.clients[&alpha].clone();
    guard.clients.clear();
    for player_id in 0..crate::limits::MAX_PLAYER_CLIENTS as u16 {
        let mut client = template.clone();
        client.player.id = player_id;
        client.session_id =
            crate::authority_identity::PlayerSessionId::new(u64::from(player_id) + 1);
        client.life_seq = 1;
        client.pending_snapshot_events = crate::snapshot::PendingSnapshotEvents::default();
        client.pending_realtime_events = crate::snapshot::PendingRealtimeEvents::default();
        guard.clients.insert(player_id, client);
    }
    let facts_per_dense_tick = crate::limits::MAX_PLAYER_CLIENTS * 2;
    let slots = guard
        .clients
        .keys()
        .copied()
        .map(|player_id| (player_id, facts_per_dense_tick))
        .collect();
    assert!(
        crate::snapshot::reserve_realtime_event_slots(
            &mut guard,
            slots,
            crate::snapshot::RealtimeEventPressureDisposition::Rejected,
        )
        .is_some(),
        "all 768 clients can each retain one same-tick weapon+damage fact from every source"
    );
    assert_eq!(
        crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
        facts_per_dense_tick * 2,
        "one additional dense tick is reserved for pacing backlog"
    );
    assert!(
        crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY
            < crate::rtc::RTC_APPLICATION_RELIABLE_QUEUE_PACKETS
    );
    assert!(
        crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY
            * crate::realtime_event::MAX_REALTIME_EVENT_PACKET_LEN
            < crate::rtc::RTC_APPLICATION_QUEUE_BYTES
    );
}

#[test]
fn dedicated_event_pacing_backlog_preserves_fifo_and_releases_every_slot() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    {
        let mut guard = state.lock().unwrap();
        let recipient = guard.active_player_runtime_identity(player_id).unwrap();
        let client = guard.clients.get_mut(&player_id).unwrap();
        client.pending_realtime_events.queue.extend(
            (1..=crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY as u32)
                .map(|event_id| dedicated_envelope(recipient, event_id)),
        );
        client.send_pacing.tokens = 0.0;
    }
    emit_snapshot_at_tick(&state, 2);
    assert_eq!(
        state.lock().unwrap().clients[&player_id]
            .pending_realtime_events
            .queue
            .len(),
        crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
        "a paced tick retains the entire one-shot FIFO"
    );
    assert!(rx.try_recv().is_err());

    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&player_id)
        .unwrap()
        .send_pacing
        .tokens = crate::realtime_event::REALTIME_WEAPON_PACKET_LEN as f64;
    emit_snapshot_at_tick(&state, 4);
    let first = rx.try_recv().expect("one exact event drains");
    assert_eq!(first[0], crate::realtime_event::REALTIME_EVENT_PACKET);
    assert_eq!(u32::from_le_bytes(first[19..23].try_into().unwrap()), 1);
    assert_eq!(
        state.lock().unwrap().clients[&player_id]
            .pending_realtime_events
            .queue
            .front()
            .unwrap()
            .event_id,
        2,
        "pacing stops at the first blocked fact without reordering later ids"
    );

    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&player_id)
        .unwrap()
        .send_pacing
        .tokens = 1_000_000.0;
    emit_snapshot_at_tick(&state, 6);
    assert_eq!(state.lock().unwrap().realtime_event_queue_current(), 0);
}

#[test]
fn queue_watch_damage_event_drops_oldest_at_u8_max_cap() {
    // Pin the cap-overflow eviction semantics. The wire `event_count`
    // is u8, so the queue can hold at most `u8::MAX` entries; pushing
    // a 256th must evict the oldest (FIFO). Implementation uses
    // `VecDeque::pop_front` for O(1) eviction — the prior `Vec::remove(0)`
    // would have been O(n) at exactly the boundary.
    let (state, alpha, _beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    // Fill the queue to the cap.
    for shooter in 0..(u8::MAX as u16) {
        crate::snapshot::queue_watch_damage_event_to_near_aoi_inner(
            &state,
            alpha,
            &[shooter],
            watch_damage_event(alpha, shooter),
            true,
        );
    }
    {
        let guard = state.lock().expect("server state should not be poisoned");
        let queue = &guard.clients[&alpha].pending_snapshot_events.watch_damage;
        assert_eq!(queue.len(), u8::MAX as usize);
        assert_eq!(queue.front().unwrap().shooter_id, 0);
        assert_eq!(queue.back().unwrap().shooter_id, (u8::MAX as u16) - 1);
    }
    // One more push — the oldest (shooter_id=0) must be evicted; the
    // newest (shooter_id=999) must land at the back.
    crate::snapshot::queue_watch_damage_event_to_near_aoi_inner(
        &state,
        alpha,
        &[999],
        watch_damage_event(alpha, 999),
        true,
    );
    let guard = state.lock().expect("server state should not be poisoned");
    let queue = &guard.clients[&alpha].pending_snapshot_events.watch_damage;
    assert_eq!(queue.len(), u8::MAX as usize, "cap holds at u8::MAX");
    assert_eq!(queue.front().unwrap().shooter_id, 1, "oldest evicted");
    assert_eq!(queue.back().unwrap().shooter_id, 999, "newest at the back");
}

#[test]
fn queue_watch_damage_event_is_noop_when_snapshots_flag_off() {
    let (state, alpha, _beta, _gamma, _rx_a, _rx_b, _rx_c) = place_three_aoi_clients();
    crate::snapshot::queue_watch_damage_event_to_near_aoi_inner(
        &state,
        alpha,
        &[],
        watch_damage_event(alpha, 7),
        false,
    );
    let state_guard = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        state_guard.clients[&alpha]
            .pending_snapshot_events
            .watch_damage
            .len(),
        0,
        "flag off should not queue"
    );
}

/// The client clears a remote player's position when an entity is absent from the snapshots it
/// receives - that is the documented rule behind a second joiner holding a peer entry with identity,
/// no position, and no repair inside 30 seconds. For that rule to be safe, one server-side fact has
/// to hold: once a second client joins an arena where a player is already authoritative and moving,
/// that player reaches the newcomer's snapshot frames carrying an actual position.
#[test]
fn a_late_joiners_snapshot_carries_the_established_player_with_a_position() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let established = register_connection(&state, tx);
    join_arena(&state, established, 0);
    {
        let mut locked = state.lock().expect("state");
        let packet = client_position_packet_with_sequence(0, 500, 700, 1);
        let position =
            large_position_from_client(established, &packet).expect("established position");
        locked.set_client_position(established, position);
        seed_snapshot_motion(locked.clients.get_mut(&established).expect("client"));
    }

    let (tx, rx) = mpsc::channel();
    let newcomer = register_connection(&state, tx);
    join_arena(&state, newcomer, 0);
    {
        let mut locked = state.lock().expect("state");
        let packet = client_position_packet_with_sequence(0, 550, 700, 1);
        let position = large_position_from_client(newcomer, &packet).expect("newcomer position");
        locked.set_client_position(newcomer, position);
        seed_snapshot_motion(locked.clients.get_mut(&newcomer).expect("client"));
    }

    emit_snapshots_for_tick_at_tick(&state, 9_100);

    let packet = rx
        .try_iter()
        .find(|packet| packet.first() == Some(&SNAPSHOT_PACKET))
        .expect("the newcomer receives a snapshot frame");
    let frame = SnapshotFrame::parse(&packet).expect("parseable snapshot frame");
    let near_ids = frame
        .near_entities
        .iter()
        .map(|entity| entity.entity_id)
        .collect::<Vec<_>>();
    let mid_ids = frame
        .mid_entities
        .iter()
        .map(|entity| entity.entity_id)
        .collect::<Vec<_>>();
    let entity = frame
        .near_entities
        .iter()
        .chain(frame.mid_entities.iter())
        .find(|entity| entity.entity_id == established)
        .unwrap_or_else(|| {
            panic!(
                "the established player never reaches the newcomer's snapshot: near={near_ids:?} \
                 mid={mid_ids:?}"
            )
        });
    assert!(
        entity.x.is_some() && entity.y.is_some(),
        "the established player reaches the newcomer with no position, which the client's \
         snapshot rule turns into a peer that is never rendered"
    );
}

/// The same client rule applies to every frame, not only the first one: an entity absent from a
/// received snapshot has its position cleared. A peer that is standing still is exactly the peer
/// delta encoding is tempted to omit, so a follow-up frame must keep carrying it. If this fails
/// while the first-frame test passes, a stationary peer is what turns invisible for a late joiner.
#[test]
fn a_follow_up_delta_frame_keeps_carrying_a_stationary_established_player() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let established = register_connection(&state, tx);
    join_arena(&state, established, 0);
    {
        let mut locked = state.lock().expect("state");
        let packet = client_position_packet_with_sequence(0, 500, 700, 1);
        let position =
            large_position_from_client(established, &packet).expect("established position");
        locked.set_client_position(established, position);
        seed_snapshot_motion(locked.clients.get_mut(&established).expect("client"));
    }

    let (tx, rx) = mpsc::channel();
    let newcomer = register_connection(&state, tx);
    join_arena(&state, newcomer, 0);
    {
        let mut locked = state.lock().expect("state");
        let packet = client_position_packet_with_sequence(0, 550, 700, 1);
        let position = large_position_from_client(newcomer, &packet).expect("newcomer position");
        locked.set_client_position(newcomer, position);
        seed_snapshot_motion(locked.clients.get_mut(&newcomer).expect("client"));
    }

    emit_snapshots_for_tick_at_tick(&state, 9_100);
    let first = rx
        .try_iter()
        .find(|packet| packet.first() == Some(&SNAPSHOT_PACKET))
        .expect("the newcomer receives a first snapshot frame");
    assert!(
        SnapshotFrame::parse(&first)
            .expect("parseable first frame")
            .near_entities
            .iter()
            .chain(
                SnapshotFrame::parse(&first)
                    .expect("parseable first frame")
                    .mid_entities
                    .iter()
            )
            .any(|entity| entity.entity_id == established)
    );

    // Neither client moves or sends anything; the arena simply ticks forward.
    emit_snapshots_for_tick_at_tick(&state, 9_110);
    let second = rx
        .try_iter()
        .find(|packet| packet.first() == Some(&SNAPSHOT_PACKET))
        .expect("the newcomer receives a follow-up snapshot frame");
    let frame = SnapshotFrame::parse(&second).expect("parseable follow-up frame");
    let entity = frame
        .near_entities
        .iter()
        .chain(frame.mid_entities.iter())
        .find(|entity| entity.entity_id == established)
        .unwrap_or_else(|| {
            panic!(
                "a stationary established player vanished from the newcomer's follow-up frame: \
                 near={:?} mid={:?}",
                frame
                    .near_entities
                    .iter()
                    .map(|entity| entity.entity_id)
                    .collect::<Vec<_>>(),
                frame
                    .mid_entities
                    .iter()
                    .map(|entity| entity.entity_id)
                    .collect::<Vec<_>>(),
            )
        });
    assert!(
        entity.x.is_some() && entity.y.is_some(),
        "the follow-up frame names the stationary peer without a position"
    );
}

#[test]
fn spectator_aoi_receives_nearby_actors_without_publishing_a_self_actor() {
    let shared = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let viewer = register_connection(&shared, tx_a);
    let target = register_connection(&shared, tx_b);
    join_arena(&shared, viewer, 8);
    join_arena(&shared, target, 0);
    let mut state = shared.lock().unwrap();
    let target_pos =
        large_position_from_client(target, &client_position_packet(1, 12000, 12000)).unwrap();
    state.set_client_position(target, target_pos.clone());
    seed_snapshot_motion(state.clients.get_mut(&target).unwrap());
    state.clients.get_mut(&viewer).unwrap().snapshot_ready = true;
    state
        .clients
        .get_mut(&viewer)
        .unwrap()
        .aoi_observer_position = Some(target_pos);
    let (near, _) = compute_aoi_entities(&state, viewer);
    assert!(near.iter().any(|(id, _)| *id == target));
    assert!(near.iter().all(|(id, _)| *id != viewer));
    assert!(!state.clients[&viewer].alive);
    assert!(state.clients[&viewer].last_position.is_none());
    assert!(state.clients[&viewer].visibility_cell.is_none());
}

#[test]
fn private_snapshot_names_the_configured_respawn_loadout_policy() {
    for maxed_spawn_loadout in [false, true] {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            maxed_spawn_loadout,
            ..ServerConfig::default()
        })));
        let (tx, rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, 0);
        drain_available(&rx);
        crate::snapshot::emit_snapshots_for_tick_at_tick(&state, 1000);
        let frame = SnapshotFrame::parse(&rx.try_recv().expect("snapshot")).expect("valid flags");
        let prize = frame
            .events
            .iter()
            .find_map(|event| match event {
                SnapshotEvent::PrizeState(prize) => Some(prize),
                _ => None,
            })
            .expect("private spawn policy");
        assert_eq!(
            prize.flags & SnapshotPrizeState::MAXED_SPAWN_LOADOUT != 0,
            maxed_spawn_loadout
        );
    }
}

#[test]
fn pending_contact_publishes_group_hold_retries_pressure_and_resumes_surviving_child() {
    use crate::realtime_event::{AuthenticatedRealtimeEvent, RealtimeEventPayload};
    let (state, alpha, beta, _gamma, rx, _rx_b, _rx_c) = place_three_aoi_clients();
    let base = crate::wire::current_tick();
    let source = {
        let mut guard = state.lock().unwrap();
        let owner = guard.active_player_runtime_identity(beta).unwrap();
        let source = crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: beta,
            source_session_id: owner.session.session_id.get(),
            source_life_seq: owner.life.life_seq,
            shot_group_id: 41,
            x: 481.0,
            y: 500.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.25,
            frequency: guard.clients[&beta].player.frequency,
            ship: guard.clients[&beta].player.ship,
            weapon_data: 7,
        };
        for (id, x) in [(99, 481.0_f32), (100, 600.0)] {
            guard.record_projectile(ServerProjectile {
                id,
                shot_group_id: 41,
                shot_index: (id - 99) as u8,
                owner_id: beta,
                owner_life_seq: owner.life.life_seq,
                presentation_source: Some(source),
                combat_timeline: None,
                weapon_data: 7,
                x: x as u16,
                y: 500,
                timestamp: base,
                created_tick: base,
                velocity_x: 1000,
                velocity_y: 0,
                expiry_tick: Some(crate::continuum::continuum_tick_add(base, 1000)),
                pos_x: x,
                pos_y: 500.0,
                source_server_tick: None,
                precise_velocity: None,
                bounces_remaining: 0,
                behavior: crate::gameplay_rules::ProjectileBehavior::Burst { active: true },
                proximity_target: None,
                proximity_sensor_end_tick: None,
                proximity_highest_offset: 0.0,
            });
        }
        source
    };
    let emit = |offset| {
        {
            let mut guard = state.lock().unwrap();
            guard.arena_tick += 1;
            let rate = guard.config.tick_rate_hz;
            for client in guard.clients.values_mut() {
                client.send_pacing.refill_for_tick(rate);
            }
        }
        emit_snapshots_for_tick_at_tick(&state, crate::continuum::continuum_tick_add(base, offset));
    };
    let states = || {
        rx.try_iter()
            .filter_map(|bytes| AuthenticatedRealtimeEvent::parse(&bytes).ok())
            .filter_map(|event| {
                if let RealtimeEventPayload::ProjectileState(p) = event.payload {
                    Some(p)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };
    emit(0);
    let initial = states();
    assert_eq!(initial.len(), 2);
    {
        let guard = state.lock().unwrap();
        let recipient = guard.active_player_runtime_identity(alpha).unwrap();
        let successor = crate::authority_identity::PlayerRuntimeIdentity::new(
            recipient.session,
            crate::authority_identity::LifeIdentity::new(alpha, recipient.life.life_seq + 1),
        );
        let mut queued = crate::snapshot::PendingRealtimeEvents::default();
        for (event_id, motion_frozen, flight_transition) in [
            (9001, false, false),
            (9002, true, true),
            (9003, false, true),
        ] {
            let mut payload = initial[0];
            payload.motion_frozen = motion_frozen;
            queued
                .queue
                .push_back(crate::snapshot::PendingRealtimeEventEnvelope {
                    recipient,
                    event_id,
                    server_tick: base,
                    payload: RealtimeEventPayload::ProjectileState(payload),
                    flight_transition,
                });
        }
        queued.retain_for_recipient(successor);
        assert_eq!(
            queued.queue.len(),
            2,
            "held and resumed flight survive a soft recipient life change"
        );
        queued.retain_public_combat_for_session(successor.session);
        assert_eq!(queued.queue.len(), 2);
        let replacement = crate::authority_identity::PlayerSessionIdentity::new(
            alpha,
            crate::authority_identity::PlayerSessionId::new(recipient.session.session_id.get() + 1),
        );
        queued.retain_public_combat_for_session(replacement);
        assert!(
            queued.queue.is_empty(),
            "flight state cannot cross a replacement session"
        );
    }
    let readiness = state.lock().unwrap().clients[&alpha]
        .threat_presentation
        .clone();
    {
        let mut guard = state.lock().unwrap();
        let c = &guard.clients[&alpha];
        let target = crate::authority_projectiles::PlayerHullSnapshot {
            player_id: alpha,
            life_seq: c.life_seq,
            x: 500.0,
            y: 500.0,
            frequency: c.player.frequency,
            radius_px: 14.0,
            motion_segment: c.actor_motion_history.current_segment(),
            motion_samples: c.actor_motion_history.current_samples().collect(),
            motion_source_kind: Some(
                crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
            ),
            lag_compensation_ticks: 0,
            life_started_tick: None,
        };
        guard.active_projectiles.step(base, |_, _| false, &[]);
        assert!(
            guard
                .active_projectiles
                .step_with_damage_geometry_and_harmful_eligibility_transactional(
                    crate::continuum::continuum_tick_add(base, 1),
                    |_, _| false,
                    &[target.clone()],
                    3.0,
                    0,
                    80.0,
                    |_| false
                )
                .is_empty()
        );
        assert!(
            guard
                .active_projectiles
                .step_with_damage_geometry_and_harmful_eligibility_transactional(
                    crate::continuum::continuum_tick_add(base, 2),
                    |_, _| false,
                    &[target],
                    3.0,
                    0,
                    80.0,
                    |_| false
                )
                .is_empty()
        );
        assert_eq!(guard.active_projectiles.pending_hit_count(), 1);
        assert_eq!(guard.active_projectiles.frozen_projectile_ids().len(), 2);
        let recipient = guard.active_player_runtime_identity(alpha).unwrap();
        let client = guard.clients.get_mut(&alpha).unwrap();
        while client.pending_realtime_events.queue.len()
            < crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY
        {
            let id = client.allocate_realtime_event_id();
            client
                .pending_realtime_events
                .queue
                .push_back(dedicated_envelope(recipient, id));
        }
    }
    emit(2);
    assert!(
        state.lock().unwrap().clients[&alpha]
            .projectile_motion_frozen_published
            .is_empty(),
        "a full queue cannot claim publication of a held state"
    );
    states(); // discard capacity-fixture traffic, not application evidence
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&alpha)
        .unwrap()
        .pending_realtime_events
        .queue
        .clear();
    emit(3);
    let held = states()
        .into_iter()
        .filter(|p| p.motion_frozen)
        .collect::<Vec<_>>();
    assert_eq!(
        held.len(),
        2,
        "both the contact and its frozen independent sibling need a fact"
    );
    assert!(
        held.iter()
            .all(|p| !p.terminal && p.velocity_x == 100.0 && p.source == source)
    );
    {
        let guard = state.lock().unwrap();
        assert_eq!(
            guard.clients[&alpha]
                .projectile_motion_frozen_published
                .len(),
            2
        );
        let ledger = &guard.clients[&alpha].threat_presentation;
        let owner = guard.active_player_runtime_identity(beta).unwrap();
        for id in [99, 100] {
            let exact = crate::threat_presentation::ProjectilePresentationIdentity::projectile(
                owner, 41, id,
            )
            .unwrap();
            assert_eq!(
                ledger.exact_projectile_readiness(exact, base).contract,
                readiness.exact_projectile_readiness(exact, base).contract,
                "a hold must not reset any exact projectile readiness field"
            );
        }
        let before = readiness.readiness(owner, base).contract.unwrap();
        let after = ledger.readiness(owner, base).contract.unwrap();
        assert_eq!(
            (
                after.first_emitted_tick,
                after.conservative_open_tick,
                after.hard_open_tick
            ),
            (
                before.first_emitted_tick,
                before.conservative_open_tick,
                before.hard_open_tick
            ),
            "normal latest-snapshot progress cannot reset source admission deadlines"
        );
    }
    state.lock().unwrap().active_projectiles.resolve_hit(99);
    emit(4);
    let resumed = states()
        .into_iter()
        .filter(|p| p.projectile_id == 100 && !p.motion_frozen)
        .collect::<Vec<_>>();
    assert_eq!(
        resumed.len(),
        1,
        "the surviving independent child resumes without a new fire event"
    );
    assert_eq!(resumed[0].velocity_x, 100.0);
    assert!(
        state.lock().unwrap().clients[&alpha]
            .projectile_motion_frozen_published
            .is_empty()
    );
    {
        let mut guard = state.lock().unwrap();
        guard.active_projectiles.clear();
        guard
            .clients
            .get_mut(&alpha)
            .unwrap()
            .projectile_motion_frozen_published
            .insert(99);
        guard
            .clients
            .get_mut(&alpha)
            .unwrap()
            .projectile_refresh_pending
            .insert(99);
    }
    emit(5);
    let guard = state.lock().unwrap();
    assert!(
        guard.clients[&alpha]
            .projectile_motion_frozen_published
            .is_empty()
    );
    assert!(guard.clients[&alpha].projectile_refresh_pending.is_empty());
}

#[test]
fn cloned_snapshot_bases_share_maps_but_keep_history_and_ack_transactions_independent() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let id = register_connection(&state, tx);
    join_arena(&state, id, 0);
    let mut guard = state.lock().unwrap();
    let live = guard.clients.get_mut(&id).unwrap();
    live.snapshot_history
        .push_back(crate::snapshot::SnapshotHistoryEntry {
            snapshot_id: 7,
            server_tick: 100,
            entities: Arc::new(SnapshotEntityMap::from([(id, near_state(8200, 8200))])),
            emitted_at: Instant::now(),
            prize_known: None,
        });
    live.last_acked_snapshot_id = Some(7);
    let mut prospective = live.clone();
    assert!(
        Arc::ptr_eq(
            &live.snapshot_history.back().unwrap().entities,
            &prospective.snapshot_history.back().unwrap().entities
        ),
        "transaction clones share immutable map storage"
    );
    prospective.snapshot_history.pop_back();
    prospective.last_acked_snapshot_id = None;
    assert_eq!(live.last_acked_snapshot_id, Some(7));
    assert_eq!(live.snapshot_history.back().unwrap().entities[&id].x, 8200);
    assert_eq!(live.snapshot_history.back().unwrap().snapshot_id, 7);
}
