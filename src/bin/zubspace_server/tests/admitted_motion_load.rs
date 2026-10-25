//! Native load-generator foundation, not browser or independent physics proof.
//! Generate exact shared-replay tails, but submit explicit input records through
//! production admission rather than the test queue that fabricates neutral input.
//! Applies emitted snapshots and submits their receipts through input admission.
//! Opposing-team coverage exercises weapon admission, projection and sustained
//! death/respawn. No RTC or browser Game/render application proof.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tracing_subscriber::{Layer, prelude::*};

use crate::combat_timeline::CombatViewReceiptV1;
use crate::continuum_position::{ClientPositionCanonicalState, ClientPositionSnapshot};
use crate::state::ServerState;

#[derive(Clone, Debug, Default, PartialEq)]
struct DamagePrizeEvidence {
    fields: std::collections::BTreeMap<String, u64>,
}

impl tracing::field::Visit for DamagePrizeEvidence {
    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.insert(field.name().to_owned(), value);
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        if let Ok(value) = value.try_into() {
            self.record_u64(field, value);
        }
    }
}

#[derive(Clone, Default)]
struct DamagePrizeCapture(Arc<Mutex<std::collections::BTreeMap<u32, DamagePrizeEvidence>>>);

impl<S: tracing::Subscriber> Layer<S> for DamagePrizeCapture {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "zubspace_server::server_damage"
            || event
                .metadata()
                .fields()
                .field("energy_before_prize")
                .is_none()
        {
            return;
        }
        let mut evidence = DamagePrizeEvidence::default();
        event.record(&mut evidence);
        let sequence = u32::try_from(evidence.fields["damage_seq"]).unwrap();
        let mut captures = self.0.lock().unwrap();
        assert!(
            captures.insert(sequence, evidence).is_none(),
            "one committed prize mutation per damage fact"
        );
        assert!(captures.len() <= 512, "bounded native diagnostic capture");
    }
}

#[derive(Default)]
struct SnapshotReceiver {
    bases: VecDeque<(u16, crate::snapshot_wire::SnapshotEntityStateMap)>,
    timeline: crate::combat_timeline::ClientObserverCombatTimeline,
    latest: Option<u16>,
    applied: usize,
    input_base: Option<u16>,
    // Sender-owned logical cursor survives server-side life-history retirement.
    // Snapshot consumption must not read the server's last admitted position.
    input_sequence: Option<u16>,
    baseline_receipts: std::collections::BTreeMap<u32, Vec<u8>>,
    damage: std::collections::BTreeMap<u32, crate::realtime_event::RealtimeWatchDamageEvent>,
}

struct AdmittedPeer {
    player: u16,
    rx: mpsc::Receiver<Vec<u8>>,
    receiver: SnapshotReceiver,
    previous: crate::state::ServerSimMovement,
    distance: f32,
    verified_inputs: usize,
    last_packet: Vec<u8>,
    next_baseline_delivery_id: u32,
    last_runtime: Option<crate::authority_identity::PlayerRuntimeIdentity>,
    life_inputs: std::collections::BTreeMap<u32, usize>,
}

#[derive(Clone, Copy, Debug)]
enum LoadMode {
    Movement,
    StaggeredMovement,
    SingleVolley,
    Sustained,
    Reconnect,
}

// Same logical inputs and steady-state packet cadence as the synchronized
// driver. Initial/final partial batches preserve every step at window edges.
fn staggered_batch(step: u32, total: u32, batch: u8, phase: u8) -> Option<u8> {
    assert!(step > 0 && step <= total && phase > 0 && phase <= batch);
    let phase = u32::from(phase);
    if step == phase {
        return Some(phase as u8);
    }
    if step > phase && (step - phase) % u32::from(batch) == 0 {
        return Some(batch);
    }
    if step == total {
        return Some(if step < phase {
            step as u8
        } else {
            ((step - phase) % u32::from(batch)) as u8
        });
    }
    None
}

#[test]
fn staggered_schedule_preserves_every_input_and_complete_window_tails() {
    for total in 1..=120 {
        for batch in 1..=4 {
            for phase in 1..=batch {
                let mut emitted = 0;
                for step in 1..=total {
                    if let Some(count) = staggered_batch(step, total, batch, phase) {
                        assert!(count > 0 && count <= batch);
                        emitted += u32::from(count);
                        assert_eq!(
                            emitted, step,
                            "each tail includes every elapsed logical input"
                        );
                    }
                }
                assert_eq!(emitted, total);
            }
        }
    }
}

impl SnapshotReceiver {
    fn drain(
        &mut self,
        state: &Arc<Mutex<ServerState>>,
        player: u16,
        rx: &mpsc::Receiver<Vec<u8>>,
    ) {
        for packet in rx.try_iter() {
            if let Some((id, payload)) = crate::reliable::reliable_parts(&packet) {
                if payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED) {
                    assert_eq!(payload.len(), 20);
                    let delivery_id = u32::from_le_bytes(payload[2..6].try_into().unwrap());
                    if let Some(previous) =
                        self.baseline_receipts.insert(delivery_id, payload.to_vec())
                    {
                        assert_eq!(previous, payload, "application receipt is immutable");
                    }
                    assert!(self.baseline_receipts.len() <= 32);
                }
                if let Ok(bootstrap) = crate::clock_bootstrap::ServerClockBootstrap::parse(payload)
                {
                    self.input_base = Some(bootstrap.input_sequence_base);
                    self.input_sequence
                        .get_or_insert(bootstrap.input_sequence_base);
                }
                let session = state
                    .lock()
                    .unwrap()
                    .active_player_session_identity(player)
                    .unwrap();
                assert!(crate::reliable::receive_reliable_acks(
                    state,
                    session,
                    &crate::reliable::reliable_ack(id)
                ));
            } else if packet.first() == Some(&crate::realtime_event::REALTIME_EVENT_PACKET) {
                let event = crate::realtime_event::AuthenticatedRealtimeEvent::parse(&packet)
                    .expect("emitted realtime fact decodes");
                if let crate::realtime_event::RealtimeEventPayload::WatchDamage(damage) =
                    event.payload
                {
                    if let Some(previous) = self.damage.insert(damage.damage_seq, damage) {
                        assert_eq!(previous, damage, "duplicate damage must remain immutable");
                    }
                    assert!(self.damage.len() <= 512, "bounded fixture damage evidence");
                }
            } else if packet.first() == Some(&crate::snapshot_wire::SNAPSHOT_PACKET) {
                let frame = crate::snapshot_wire::SnapshotFrame::parse(&packet)
                    .expect("emitted snapshot decodes");
                let base = self
                    .bases
                    .iter()
                    .find(|(id, _)| *id == frame.header.base_id)
                    .map(|(_, entities)| entities);
                let applied = crate::snapshot_wire::delta_apply(base, &frame)
                    .expect("exact retained delta base");
                self.bases
                    .push_back((frame.header.snapshot_id, applied.entities));
                while self.bases.len() > crate::snapshot_wire::SNAPSHOT_HISTORY_CAPACITY {
                    self.bases.pop_front();
                }
                crate::connection::record_snapshot_ack_at(
                    state,
                    player,
                    &crate::snapshot_wire::SnapshotAckPacket {
                        snapshot_id: frame.header.snapshot_id,
                    }
                    .encode(),
                    Instant::now(),
                );
                self.latest = Some(frame.header.snapshot_id);
                self.applied += 1;
                for event in frame.events {
                    if let crate::snapshot_wire::SnapshotEvent::ObserverTimeline(directive) = event
                    {
                        let sequence = self.input_sequence.expect("received input bootstrap");
                        self.timeline.observe_directive(directive, sequence);
                    }
                }
            }
        }
    }
}

fn thrust_records(
    first: u16,
    count: u8,
    direction: u8,
    door_seed: u8,
    life: u32,
    receiver: &mut SnapshotReceiver,
) -> (Vec<u8>, Vec<CombatViewReceiptV1>) {
    let mut packet = vec![crate::connection::CLIENT_INPUT_RECORD_TYPE];
    let mut receipts = Vec::new();
    packet.extend_from_slice(&first.to_le_bytes());
    packet.push(count);
    for offset in 0..count {
        packet.extend_from_slice(&1_u16.to_le_bytes()); // held forward thrust
        packet.extend_from_slice(&[direction, door_seed]);
        let sequence = first.wrapping_add(u16::from(offset));
        receiver.input_sequence = Some(sequence);
        receiver.timeline.advance_input(sequence);
        let view = receiver
            .timeline
            .active_directive()
            .zip(receiver.latest)
            .map(|(directive, snapshot)| (life, directive.generation, snapshot));
        let receipt = view.map_or_else(
            || CombatViewReceiptV1::unavailable(sequence),
            |(life, generation, snapshot)| CombatViewReceiptV1 {
                observer_life_seq: life,
                input_sequence: sequence,
                timeline_generation: generation,
                latest_game_applied_snapshot_id: snapshot,
                flags: 0,
            },
        );
        packet.extend_from_slice(&receipt.encode());
        receipts.push(receipt);
    }
    (packet, receipts)
}

#[test]
fn receiver_timeline_does_not_depend_on_retired_server_position_history() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let mut peer = register_admitted_peer(&state, &mut [], 0, false);
    let cursor = peer.receiver.input_sequence;
    let directive = {
        let mut locked = state.lock().unwrap();
        let client = locked.clients.get_mut(&peer.player).unwrap();
        let directive = client.observer_combat_timeline.current_directive().unwrap();
        // Isolated receiver dependency test, not evidence of a combat death.
        client.last_position_sequence = None;
        directive
    };
    let (tx, rx) = mpsc::channel();
    let snapshot_id = peer.receiver.latest.unwrap().wrapping_add(1);
    tx.send(
        crate::snapshot_wire::SnapshotFrame {
            header: crate::snapshot_wire::SnapshotHeader {
                flags: crate::snapshot_wire::SNAPSHOT_FLAG_FULL,
                snapshot_id,
                base_id: 0,
                tick: crate::wire::current_tick(),
                arena_stream_id: 1,
                near_count: 0,
                mid_count: 0,
                event_count: 1,
            },
            near_entities: Vec::new(),
            mid_entities: Vec::new(),
            events: vec![crate::snapshot_wire::SnapshotEvent::ObserverTimeline(
                directive,
            )],
        }
        .encode(),
    )
    .unwrap();
    peer.receiver.drain(&state, peer.player, &rx);
    assert_eq!(peer.receiver.latest, Some(snapshot_id));
    assert_eq!(peer.receiver.input_sequence, cursor);
    assert!(
        state.lock().unwrap().clients[&peer.player]
            .last_position_sequence
            .is_none()
    );
}

#[test]
fn explicit_thrust_load_advances_admitted_motion_without_correction() {
    for batch_size in [1, 3] {
        run_admitted_load(batch_size, 1, 120, false, LoadMode::Movement);
    }
}

#[test]
fn eight_paced_peers_preserve_admitted_movement_and_combat_receipts() {
    run_admitted_load(3, 8, 120, true, LoadMode::Movement);
}

#[test]
fn staggered_eight_paced_peers_preserve_admission_and_receipts() {
    run_admitted_load(3, 8, 120, true, LoadMode::StaggeredMovement);
}

#[test]
fn eight_paced_peers_admit_bullets_and_resolve_recipient_geometry() {
    run_admitted_load(3, 8, 120, true, LoadMode::SingleVolley);
}

#[test]
fn paced_reconnect_preserves_other_peers_and_rejects_retired_session() {
    run_admitted_load(3, 8, 240, true, LoadMode::Reconnect);
}

#[test]
#[ignore = "projection reuse measurement, not acceptance"]
fn measure_admitted_projection_reuse() {
    let report = crate::server_tick::projection_measurement::measure(|| {
        run_admitted_load(3, 8, 900, true, LoadMode::Sustained);
    });
    println!("{report}");
}

#[test]
fn sustained_admitted_combat_crosses_death_and_respawn() {
    run_admitted_load(3, 8, 900, true, LoadMode::Sustained);
}

// Use real reliable lifecycle ingress; the tick fixture adapter must never
// supply a synthetic frequency boundary for authenticated lifecycle baselines.
fn respawn_due_peer(state: &Arc<Mutex<ServerState>>, peer: &mut AdmittedPeer, tick: u32) -> bool {
    let (runtime, mut packet) = {
        let locked = state.lock().unwrap();
        let client = &locked.clients[&peer.player];
        if client.alive
            || !client
                .respawn_at_tick
                .is_some_and(|deadline| crate::continuum::continuum_tick_reached(tick, deadline))
        {
            return false;
        }
        assert!(client.canonical_movement_witness.is_none());
        let mut expected = client.clone();
        expected.alive = true;
        expected.respawn_at_tick = None;
        expected.reset_server_runtime_for_ship_at_tick(tick);
        let sequence = peer.receiver.input_sequence.unwrap().wrapping_add(1);
        let (x, y) = crate::zone::server_spawn_coordinates(locked.zone_map());
        let packet = super::frequency_baseline_packet(
            ClientPositionSnapshot {
                direction: 0,
                timestamp: 0,
                client_sequence: Some(sequence),
                x,
                y,
                velocity_x: 0,
                velocity_y: 0,
                togglables: 0,
                bounty: expected.server_bounty,
                energy: expected.server_energy,
                weapon: 0,
                canonical_state: ClientPositionCanonicalState::from_f32(
                    x as f32, y as f32, 0.0, 0.0, 0.0,
                ),
                repel_snapshot_id: None,
                repel_application_sequence: None,
                repel_delivery_generation: None,
                warp_collectible_id: None,
            },
            sequence,
        );
        (
            locked.active_player_runtime_identity(peer.player).unwrap(),
            packet,
        )
    };
    let offset = crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET;
    packet[offset + 7] = crate::doors::client_door_simulation_seed_at_tick(
        state.lock().unwrap().clients[&peer.player].door_state,
        tick,
    )
    .expect("issued respawn phase");
    let delivery_id = peer.next_baseline_delivery_id;
    peer.next_baseline_delivery_id += 1;
    peer.receiver.timeline.reset();
    let mut joined = true;
    crate::connection::handle_client_packet(
        state,
        peer.player,
        crate::reliable::reliable(delivery_id, &packet),
        "admitted-respawn",
        &mut joined,
    );
    let accepted_tick = crate::wire::current_tick();
    crate::server_tick::run_server_tick_at_tick(state, Instant::now(), accepted_tick);
    peer.receiver.drain(state, peer.player, &peer.rx);
    let locked = state.lock().unwrap();
    let client = &locked.clients[&peer.player];
    assert!(client.alive && client.respawn_at_tick.is_none());
    assert!(client.pending_movement_correction.is_none());
    let witness = client.canonical_movement_witness.unwrap();
    assert_eq!(
        peer.receiver.baseline_receipts.get(&delivery_id),
        Some(&crate::roster::lifecycle_baseline_accepted(
            delivery_id,
            runtime.life.life_seq,
            witness.sequence,
            accepted_tick,
            (witness.wire_x, witness.wire_y),
        ))
    );
    peer.receiver.input_sequence = Some(witness.sequence);
    peer.previous = witness.state;
    drop(locked);
    let retired = peer.last_runtime.expect("moving predecessor life");
    assert_eq!(retired.session, runtime.session);
    assert_ne!(retired.life, runtime.life);
    assert!(matches!(
        crate::arena::accept_position_relay_for_runtime_at_tick_outcome(
            state,
            retired,
            &peer.last_packet,
            accepted_tick,
        ),
        crate::arena::RuntimePositionAdmission::Rejected
    ));
    assert_eq!(
        state.lock().unwrap().clients[&peer.player].canonical_movement_witness,
        Some(witness)
    );
    assert!(peer.life_inputs.insert(runtime.life.life_seq, 0).is_none());
    peer.last_runtime = Some(runtime);
    peer.last_packet = packet;
    true
}

#[test]
#[ignore = "paced 200-peer native movement measurement; not RTC/combat capacity"]
fn two_hundred_paced_admitted_peers() {
    run_admitted_load(3, 200, 600, true, LoadMode::Movement);
}

#[test]
#[ignore = "staggered native movement measurement; not RTC/combat capacity"]
fn two_hundred_staggered_paced_admitted_peers() {
    run_admitted_load(3, 200, 600, true, LoadMode::StaggeredMovement);
}

fn register_admitted_peer(
    state: &Arc<Mutex<ServerState>>,
    peers: &mut [AdmittedPeer],
    index: usize,
    combat: bool,
) -> AdmittedPeer {
    let (tx, rx) = mpsc::channel();
    let player = crate::arena::register_connection(state, tx);
    let join_tick = crate::wire::current_tick();
    crate::arena::join_arena_at_tick(state, player, 0, join_tick);
    let mut receiver = SnapshotReceiver::default();
    receiver.drain(state, player, &rx);
    let input_base = receiver
        .input_base
        .expect("received server clock bootstrap");
    // Native setup omits map download. Install the same issued challenge
    // as map completion, not the legacy no-challenge input fallback.
    {
        let mut locked = state.lock().unwrap();
        let (_, door_state) =
            crate::security::security_challenge_for_door_state(locked.server_door_challenge_state);
        locked.clients.get_mut(&player).unwrap().door_state = Some(door_state);
        // Controlled native initial teams, not a frequency-transition test.
        // Opposing recipients must exercise collision projection rather
        // than having every projectile discarded by same-team eligibility.
        if combat {
            locked.clients.get_mut(&player).unwrap().player.frequency = (index % 2) as u16;
        }
    }
    let mut baseline = {
        let locked = state.lock().unwrap();
        let client = &locked.clients[&player];
        let (x, y) = crate::zone::server_spawn_coordinates(locked.zone_map());
        super::frequency_baseline_packet(
            ClientPositionSnapshot {
                direction: 0,
                timestamp: 0,
                client_sequence: Some(input_base),
                x,
                y,
                velocity_x: 0,
                velocity_y: 0,
                togglables: 0,
                bounty: client.server_bounty,
                energy: client.server_energy,
                weapon: 0,
                canonical_state: ClientPositionCanonicalState::from_f32(
                    x as f32, y as f32, 0.0, 0.0, 0.0,
                ),
                repel_snapshot_id: None,
                repel_application_sequence: None,
                repel_delivery_generation: None,
                warp_collectible_id: None,
            },
            input_base,
        )
    };
    baseline[crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 7] = {
        let locked = state.lock().unwrap();
        crate::doors::client_door_simulation_seed_at_tick(
            locked.clients[&player].door_state,
            crate::wire::current_tick(),
        )
        .expect("issued baseline door phase")
    };
    let mut joined = true;
    crate::connection::handle_client_packet(
        state,
        player,
        crate::reliable::reliable(0, &baseline),
        "admitted-motion-load",
        &mut joined,
    );
    // Joining can load assets. The packet reader timestamps its input at
    // receipt, so draining against the pre-join tick would travel backward
    // and correctly defer the future-phase record forever in this fixture.
    let start_tick = crate::wire::current_tick();
    crate::server_tick::run_server_tick_at_tick(state, Instant::now(), start_tick);
    // Existing peers keep consuming while later peers join. Pausing them
    // through the entire setup would manufacture an ACK/base-retention
    // outage before the paced workload even starts.
    for peer in peers.iter_mut() {
        peer.receiver.drain(state, peer.player, &peer.rx);
    }
    let initial = state.lock().unwrap().clients[&player]
        .canonical_movement_witness
        .expect("admitted baseline");
    receiver.drain(state, player, &rx);
    AdmittedPeer {
        player,
        rx,
        receiver,
        previous: initial.state,
        distance: 0.0,
        verified_inputs: 0,
        last_packet: Vec::new(),
        next_baseline_delivery_id: 1,
        last_runtime: None,
        life_inputs: std::collections::BTreeMap::from([(
            state.lock().unwrap().clients[&player].life_seq,
            0,
        )]),
    }
}

fn run_admitted_load(batch_size: u8, peer_count: usize, steps: u32, paced: bool, mode: LoadMode) {
    let combat = matches!(mode, LoadMode::SingleVolley | LoadMode::Sustained);
    let sustained = matches!(mode, LoadMode::Sustained);
    let staggered = matches!(mode, LoadMode::StaggeredMovement);
    let prize_capture = DamagePrizeCapture::default();
    let trace_level = if std::env::var_os("ZUBSPACE_ADMITTED_LOAD_TRACE").is_some() {
        tracing_subscriber::filter::LevelFilter::DEBUG
    } else {
        tracing_subscriber::filter::LevelFilter::OFF
    };
    let subscriber = tracing_subscriber::registry()
        .with(
            prize_capture
                .clone()
                .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                    metadata.target() == "zubspace_server::server_damage"
                        && metadata.fields().field("energy_before_prize").is_some()
                })),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_test_writer()
                .with_filter(trace_level),
        );
    let _trace = tracing::subscriber::set_default(subscriber);
    let state = Arc::new(Mutex::new(ServerState::default()));
    if sustained {
        let mut locked = state.lock().unwrap();
        // This is the controlled two-team collision fixture, not a Chaos
        // spawn-distribution test. Use an actual authored fixed spawn region
        // so repeated fresh-life damage remains deterministic. The production
        // random sampler, lifecycle ingress, receipt and replay all still run.
        // The separate lifecycle_respawns_choose_diverse... regression uses
        // the unmodified shipped FFA profile and proves distributed spawns.
        super::use_fixed_authored_spawn_fixture(&mut locked);
    }
    let mut peers: Vec<AdmittedPeer> = Vec::new();
    for index in 0..peer_count {
        let peer = register_admitted_peer(&state, &mut peers, index, combat);
        peers.push(peer);
    }
    let start_tick = crate::wire::current_tick();
    let started = Instant::now();
    let mut tick_work = Vec::new();
    let mut snapshot_work = Vec::new();
    let mut other_tick_work = Vec::new();
    let mut publication_prepare_work = Vec::new();
    let mut publication_encode_history_work = Vec::new();
    let mut publication_delivery_work = Vec::new();
    let mut driver_work = Vec::new();
    let mut frame_start_lateness = Vec::new();
    let mut input_work = Vec::new();
    let mut generation_work = Vec::new();
    let mut admission_work = Vec::new();
    let mut receive_work = Vec::new();
    let mut admitted_shots = 0;
    let mut declined_shots = 0;
    let mut accepted_non_bullet = 0;
    let mut admitted_packets = 0;
    let mut shots = std::collections::BTreeMap::new();
    let mut reconnects = 0;
    let initial_lives: std::collections::BTreeMap<_, _> = {
        let locked = state.lock().unwrap();
        peers
            .iter()
            .map(|peer| (peer.player, locked.clients[&peer.player].life_seq))
            .collect()
    };
    let mut respawns = 0;
    for completed_steps in 1..=steps {
        let scheduled = started + Duration::from_secs_f64(completed_steps as f64 / 60.0);
        if paced {
            std::thread::sleep(scheduled.saturating_duration_since(Instant::now()));
        }
        let frame_started = Instant::now();
        if paced {
            frame_start_lateness.push(frame_started.saturating_duration_since(scheduled));
        }
        if matches!(mode, LoadMode::Reconnect) && completed_steps == 120 {
            let retired = peers.remove(0);
            assert!(retired.distance > 50.0 && retired.verified_inputs > 60);
            let old_runtime = state
                .lock()
                .unwrap()
                .active_player_runtime_identity(retired.player)
                .unwrap();
            crate::arena::disconnect_player_session(&state, old_runtime.session);
            {
                let mut locked = state.lock().unwrap();
                assert!(!locked.clients.contains_key(&retired.player));
                // Force numeric-slot reuse without hundreds of unrelated joins.
                // The normal allocator still creates the replacement session.
                locked.next_player_id = retired.player;
            }
            let replacement = register_admitted_peer(&state, &mut peers, 0, false);
            assert_eq!(replacement.player, retired.player);
            let (new_runtime, before) = {
                let locked = state.lock().unwrap();
                (
                    locked
                        .active_player_runtime_identity(replacement.player)
                        .unwrap(),
                    locked.clients[&replacement.player].canonical_movement_witness,
                )
            };
            assert_ne!(new_runtime.session, old_runtime.session);
            assert!(matches!(
                crate::arena::accept_position_relay_for_runtime_at_tick_outcome(
                    &state,
                    old_runtime,
                    &retired.last_packet,
                    crate::wire::current_tick()
                ),
                crate::arena::RuntimePositionAdmission::Rejected
            ));
            assert_eq!(
                state.lock().unwrap().clients[&replacement.player].canonical_movement_witness,
                before
            );
            peers.push(replacement);
            reconnects += 1;
        }
        let mut input_elapsed = Duration::ZERO;
        let mut generation_elapsed = Duration::ZERO;
        let mut admission_elapsed = Duration::ZERO;
        let mut tick = if paced {
            crate::wire::current_tick()
        } else {
            crate::continuum::continuum_tick_add(start_tick, completed_steps * 100 / 60)
        };
        if staggered || completed_steps % u32::from(batch_size) == 0 {
            if sustained {
                for peer in &mut peers {
                    respawns += usize::from(respawn_due_peer(&state, peer, tick));
                }
                // Reliable ingress and its application tick can cross a clock
                // boundary. Never stamp subsequent input before a snapshot
                // just consumed during respawn.
                tick = crate::wire::current_tick();
            }
            for AdmittedPeer {
                player,
                receiver,
                previous,
                distance,
                verified_inputs,
                last_packet,
                last_runtime,
                life_inputs,
                ..
            } in &mut peers
            {
                let player = *player;
                let batch_size = if staggered {
                    let Some(count) = staggered_batch(
                        completed_steps,
                        steps,
                        batch_size,
                        (player % u16::from(batch_size)) as u8 + 1,
                    ) else {
                        continue;
                    };
                    count
                } else {
                    batch_size
                };
                if sustained && !state.lock().unwrap().clients[&player].alive {
                    continue;
                }
                // Sixty immutable logical steps per second; packet cadence is
                // either 60 Hz or 20 Hz. Do not confuse packet count with step count.
                let (runtime, first, direction, door_seed) = {
                    let locked = state.lock().unwrap();
                    let client = &locked.clients[&player];
                    (
                        locked.active_player_runtime_identity(player).unwrap(),
                        receiver
                            .input_sequence
                            .expect("sender input cursor")
                            .wrapping_add(1),
                        client.canonical_movement_witness.unwrap().direction,
                        crate::doors::client_door_simulation_seed_at_tick(client.door_state, tick)
                            .expect("issued door phase"),
                    )
                };
                let last = first.wrapping_add(u16::from(batch_size) - 1);
                let (records, receipts) = thrust_records(
                    first,
                    batch_size,
                    direction,
                    door_seed,
                    runtime.life.life_seq,
                    receiver,
                );
                let input_started = Instant::now();
                assert!(crate::connection::record_client_input_observation_at_tick(
                    &state, runtime, &records, tick,
                ));
                input_elapsed += input_started.elapsed();
                let generation_started = Instant::now();
                let expected = {
                    let locked = state.lock().unwrap();
                    let client = &locked.clients[&player];
                    let (spawn_x, spawn_y) =
                        crate::zone::server_spawn_coordinates(locked.zone_map());
                    crate::sim_observation::replay_canonical_position_gap(
                        client,
                        locked.zone_map(),
                        false,
                        false,
                        false,
                        crate::sim_observation::movement_upgrades_for_client(client),
                        &[],
                        last,
                        tick,
                        spawn_x,
                        spawn_y,
                    )
                    .expect("explicit input batch has a shared-physics tail")
                    .witnesses
                    .last()
                    .copied()
                    .unwrap()
                };
                let mut packet = {
                    let locked = state.lock().unwrap();
                    let client = &locked.clients[&player];
                    super::frequency_baseline_packet(
                        ClientPositionSnapshot {
                            direction: expected.direction,
                            timestamp: 0,
                            client_sequence: Some(last),
                            x: expected.wire_x,
                            y: expected.wire_y,
                            velocity_x: expected.wire_velocity_x,
                            velocity_y: expected.wire_velocity_y,
                            togglables: 0,
                            bounty: client.server_bounty,
                            energy: client.server_energy,
                            weapon: 0,
                            canonical_state: ClientPositionCanonicalState::from_f32(
                                expected.state.position_x,
                                expected.state.position_y,
                                expected.state.velocity_x,
                                expected.state.velocity_y,
                                expected.state.orientation,
                            ),
                            repel_snapshot_id: None,
                            repel_application_sequence: None,
                            repel_delivery_generation: None,
                            warp_collectible_id: None,
                        },
                        last,
                    )
                };
                packet.truncate(crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET);
                packet.extend_from_slice(&records);
                // One shot request per schedule mark, with no attempt to predict the
                // server's verdict. The server declines a fire for reasons this harness
                // must not reimplement and drift from - `WeaponFireRejectReason` covers
                // not-in-inventory, zero weapon, safe tile, insufficient energy and
                // cooldown - so the outcome is classified below from the witness the
                // server itself records for the sequence whose fire it applied.
                let fire = combat
                    && (completed_steps == 60
                        || (sustained
                            && completed_steps > 60
                            && completed_steps % 30 == 0
                            && receipts
                                .last()
                                .is_some_and(|receipt| receipt.timeline_generation != 0)));
                if fire {
                    // One level-1 bullet per peer; no synthetic projectile or
                    // combat decision injection. The exact tail owns the shot.
                    packet[20..22].copy_from_slice(&1_u16.to_le_bytes());
                }
                generation_elapsed += generation_started.elapsed();
                // run_server_tick's cfg(test) queue adapter supplies a synthetic
                // frequency baseline for every queued position. Exercise the same
                // unadapted entry point as production/focused authority tests;
                // otherwise a three-step packet only retains its final sample.
                let admission_started = Instant::now();
                assert!(matches!(
                    crate::arena::accept_position_relay_for_runtime_at_tick_outcome(
                        &state, runtime, &packet, tick,
                    ),
                    crate::arena::RuntimePositionAdmission::Accepted(_)
                ));
                admission_elapsed += admission_started.elapsed();
                admitted_packets += 1;
                *last_packet = packet;
                *last_runtime = Some(runtime);
                *life_inputs
                    .get_mut(&runtime.life.life_seq)
                    .expect("known admitted life") += usize::from(batch_size);
                let locked = state.lock().unwrap();
                let client = &locked.clients[&player];
                if fire {
                    // Classify the outcome from what the server recorded, never from a
                    // reimplementation of its rules. `remember_accepted_weapon_witness`
                    // runs only when a fire is actually applied, and the witness set is
                    // life-scoped, so its presence for `last` is the authority's own
                    // statement that it accepted this request. The contract is therefore
                    // "every fire the server accepted produced the bullet it claims",
                    // which is stronger than the old "every request this harness sent
                    // produced a bullet" - the latter was violated by legitimate declines.
                    // Two of the four outcomes below are bugs and stay fatal.
                    let accepted = client.accepted_weapon_witness_consumed(last);
                    let accepted_weapon = client.last_accepted_weapon_data;
                    let projectile = locked.active_projectiles.iter().find(|projectile| {
                        projectile.owner_id == player
                            && projectile.owner_life_seq == runtime.life.life_seq
                            && projectile
                                .combat_timeline
                                .is_some_and(|timeline| timeline.owner_input_sequence == last)
                    });
                    match (projectile, accepted) {
                        (None, false) => declined_shots += 1,
                        (Some(_), false) => panic!(
                            "a bullet entered the production registry for input {last} of life_seq {} with no acceptance witness: something spawned it outside weapon authority",
                            runtime.life.life_seq
                        ),
                        // A prize can move a peer onto a non-bullet weapon, whose accepted
                        // fire correctly produces no entry in the projectile registry.
                        (None, true)
                            if !accepted_weapon.is_some_and(|weapon| {
                                crate::continuum_weapon::continuum_weapon_type(weapon) == 2
                            }) =>
                        {
                            accepted_non_bullet += 1;
                        }
                        (None, true) => {
                            let current = locked.active_player_runtime_identity(player);
                            let owned: Vec<_> = locked
                                .active_projectiles
                                .iter()
                                .filter(|other| other.owner_id == player)
                                .map(|other| {
                                    (
                                        other.owner_life_seq,
                                        other
                                            .combat_timeline
                                            .map(|timeline| timeline.owner_input_sequence),
                                    )
                                })
                                .collect();
                            panic!(
                                "an accepted fire produced no registered bullet: step {completed_steps} tick {tick} input {last} for life_seq {} accepted_weapon={accepted_weapon:?} while alive={} server_energy={} current_life_seq={:?} registry_len={} owned_by_owner={owned:?} inventory={:?}",
                                runtime.life.life_seq,
                                client.alive,
                                client.server_energy,
                                current.map(|identity| identity.life.life_seq),
                                locked.active_projectiles.len(),
                                client.server_weapons,
                            );
                        }
                        (Some(projectile), true) => {
                            let timeline = projectile
                                .combat_timeline
                                .expect("verified projectile timeline");
                            assert_eq!(timeline.owner, runtime);
                            assert_eq!(timeline.owner_input_sequence, last);
                            assert!(projectile.presentation_source.is_some());
                            assert!(shots.insert(projectile.id, runtime).is_none());
                            admitted_shots += 1;
                        }
                    }
                }
                for receipt in receipts {
                    if receipt.timeline_generation != 0 {
                        let decision = client
                            .observer_combat_timeline
                            .decision_for_sequence(receipt.input_sequence)
                            .expect("snapshot-backed input must have combat authority");
                        assert_eq!(decision.generation, receipt.timeline_generation);
                        assert_eq!(
                            decision.applied_snapshot_id,
                            receipt.latest_game_applied_snapshot_id
                        );
                        assert!(decision.weapon_edge_allowed());
                        *verified_inputs += 1;
                    }
                }
                assert!(
                    client.pending_movement_correction.is_none(),
                    "valid generated input must not enter correction"
                );
                let admitted = client
                    .canonical_movement_witness
                    .expect("admitted moving tail");
                assert_eq!(admitted.sequence, last);
                assert_eq!(admitted.state.position_x, expected.state.position_x);
                assert_eq!(admitted.state.position_y, expected.state.position_y);
                assert_eq!(admitted.state.velocity_x, expected.state.velocity_x);
                assert_eq!(admitted.state.velocity_y, expected.state.velocity_y);
                assert!(
                    client
                        .recent_input_records
                        .iter()
                        .any(|record| record.input.bits & 1 != 0)
                );
                *distance += (admitted.state.position_x - previous.position_x)
                    .hypot(admitted.state.position_y - previous.position_y);
                *previous = admitted.state;
                drop(locked);
            }
        }
        crate::snapshot::publication_timing::take();
        let tick_started = Instant::now();
        let tick_result = crate::server_tick::run_server_tick_at_tick(
            &state,
            if paced { tick_started } else { scheduled },
            tick,
        );
        let tick_elapsed = tick_started.elapsed();
        let [prepare, encode_history, delivery] = crate::snapshot::publication_timing::take();
        assert!(prepare + encode_history + delivery <= tick_result.snapshot_publication_work);
        publication_prepare_work.push(prepare);
        publication_encode_history_work.push(encode_history);
        publication_delivery_work.push(delivery);
        let other_elapsed = tick_elapsed
            .checked_sub(tick_result.snapshot_publication_work)
            .expect("publication timing must be contained in the complete tick");
        snapshot_work.push(tick_result.snapshot_publication_work);
        other_tick_work.push(other_elapsed);
        tick_work.push(tick_elapsed);
        let receive_started = Instant::now();
        for AdmittedPeer {
            player,
            rx,
            receiver,
            ..
        } in &mut peers
        {
            receiver.drain(&state, *player, rx);
        }
        receive_work.push(receive_started.elapsed());
        input_work.push(input_elapsed);
        generation_work.push(generation_elapsed);
        admission_work.push(admission_elapsed);
        driver_work.push(frame_started.elapsed());
    }
    let elapsed = started.elapsed();
    if paced {
        // End-to-end elapsed includes the final frame's work after its
        // scheduled start. Even a perfectly paced finite run therefore
        // reports slightly below 60 Hz. Expose actual schedule lateness
        // separately; do not subtract work or hide missed start deadlines.
        let final_start_lateness = *frame_start_lateness.last().unwrap();
        frame_start_lateness.sort();
        eprintln!(
            "  scheduled_start_lateness: p99={:?} max={:?} final={final_start_lateness:?}; elapsed driver rate includes final-frame work",
            frame_start_lateness[(frame_start_lateness.len() - 1) * 99 / 100],
            frame_start_lateness.last().unwrap(),
        );
    }
    assert_eq!(reconnects, usize::from(matches!(mode, LoadMode::Reconnect)));
    eprintln!(
        "admitted native movement: mode={mode:?} peers={peer_count} steps={steps} batch={batch_size} paced={paced} elapsed={elapsed:?} achieved_driver_hz={:.2}; all peers clustered at spawn; admitted_shots={admitted_shots}; declined_shots={declined_shots}; accepted_non_bullet={accepted_non_bullet}; admitted_packets={admitted_packets}; no RTC; admission success is NOT pacing acceptance",
        f64::from(steps) / elapsed.as_secs_f64(),
    );
    if combat {
        if !sustained {
            assert_eq!(admitted_shots, peer_count);
        } else {
            // Sustained mode now tolerates legitimate declines, so pin the other
            // direction: across a full run the gate must still observe an accepted
            // fire for every peer. Otherwise a run whose every request was declined
            // would pass on silence, and the leniency above would have quietly
            // replaced the proof with a counter. The observed margin is wide - 186
            // accepted shots across 8 peers, 23 per peer against a floor of 1.
            assert!(
                admitted_shots >= peer_count,
                "sustained combat must still admit an accepted fire per peer: got \
                 admitted={admitted_shots} declined={declined_shots} \
                 accepted_non_bullet={accepted_non_bullet}"
            );
        }
        let locked = state.lock().unwrap();
        let geometry = locked.active_projectiles.presentation_geometry_metrics();
        eprintln!("  admitted bullet geometry: {geometry:?}");
        assert!(
            geometry.resolved_total > 0,
            "must resolve recipient geometry, not merely spawn bullets"
        );
        let mut damage_by_projectile = std::collections::BTreeMap::new();
        for peer in &peers {
            for damage in peer.receiver.damage.values() {
                let owner = shots
                    .get(&damage.projectile_id)
                    .expect("damage belongs to admitted shot");
                assert_eq!(damage.shooter_player_id, owner.life.player_id);
                assert_eq!(damage.shooter_life_seq, owner.life.life_seq);
                assert_eq!(damage.shooter_session_id, owner.session.session_id.get());
                let victim = locked
                    .active_player_runtime_identity(damage.victim_player_id)
                    .unwrap();
                if !sustained {
                    assert_eq!(damage.victim_life_seq, victim.life.life_seq);
                }
                assert!(
                    peers
                        .iter()
                        .find(|peer| peer.player == damage.victim_player_id)
                        .unwrap()
                        .life_inputs
                        .contains_key(&damage.victim_life_seq),
                    "damage must name an actually admitted victim life"
                );
                assert_eq!(damage.victim_session_id, victim.session.session_id.get());
                assert!(damage.damage > 0);
                assert!(
                    sustained || !damage.lethal,
                    "single-volley fixture must preserve moving lives"
                );
                if !damage.lethal {
                    let captures = prize_capture.0.lock().unwrap();
                    let energy_before_subtraction = if let Some(loss) =
                        captures.get(&damage.damage_seq)
                    {
                        assert_eq!(loss.fields["player_id"], u64::from(damage.victim_player_id));
                        assert_eq!(
                            loss.fields["energy_before_prize"],
                            u64::from(damage.energy_before)
                        );
                        u16::try_from(loss.fields["energy_after_prize"]).unwrap()
                    } else {
                        damage.energy_before
                    };
                    assert_eq!(
                        energy_before_subtraction.saturating_sub(damage.damage),
                        damage.energy_after,
                        "damage must subtract exactly after its correlated prize mutation"
                    );
                }
                if let Some(previous) = damage_by_projectile.insert(damage.projectile_id, *damage) {
                    assert_eq!(
                        previous, *damage,
                        "recipients must receive the same frozen damage"
                    );
                }
            }
        }
        if !sustained {
            assert_eq!(
                damage_by_projectile.len(),
                shots.len(),
                "every admitted shot commits and publishes damage"
            );
        } else {
            let lethal = damage_by_projectile
                .values()
                .filter(|damage| damage.lethal)
                .count();
            let post_respawn = damage_by_projectile
                .values()
                .filter(|damage| damage.victim_life_seq > initial_lives[&damage.victim_player_id])
                .filter(|damage| {
                    peers
                        .iter()
                        .find(|peer| peer.player == damage.victim_player_id)
                        .unwrap()
                        .receiver
                        .baseline_receipts
                        .iter()
                        .any(|(delivery_id, payload)| {
                            *delivery_id > 0
                                && u32::from_le_bytes(payload[6..10].try_into().unwrap())
                                    == damage.victim_life_seq
                        })
                })
                .count();
            eprintln!(
                "sustained combat: lethal={lethal} respawns={respawns} post_respawn_damage={post_respawn}"
            );
            assert!(
                lethal >= 2 && respawns >= 2 && post_respawn > 0,
                "must exercise repeated real death, due respawn and fresh-life damage"
            );
            assert_eq!(
                locked
                    .clients
                    .values()
                    .map(|client| usize::from(client.player.losses))
                    .sum::<usize>(),
                lethal
            );
            assert_eq!(
                locked
                    .clients
                    .values()
                    .map(|client| usize::from(client.player.wins))
                    .sum::<usize>(),
                lethal
            );
        }
    }
    for (name, mut samples) in [
        ("input_admission", input_work),
        ("position_generation_test_only", generation_work),
        ("position_admission", admission_work),
        ("server_tick_excluding_admission", tick_work),
        ("snapshot_publication_in_server_tick", snapshot_work),
        ("publication_prepare", publication_prepare_work),
        (
            "publication_encode_and_history",
            publication_encode_history_work,
        ),
        (
            "publication_delivery_and_observer_history",
            publication_delivery_work,
        ),
        (
            "server_tick_excluding_publication_and_admission",
            other_tick_work,
        ),
        ("native_receiver_codec_and_server_acks", receive_work),
        ("complete_driver", driver_work),
    ] {
        samples.sort();
        eprintln!(
            "  {name}: total={:?} p99={:?} max={:?}",
            samples.iter().sum::<Duration>(),
            samples[(samples.len() - 1) * 99 / 100],
            samples.last().unwrap()
        );
    }
    for AdmittedPeer {
        player,
        receiver,
        distance,
        verified_inputs,
        life_inputs,
        ..
    } in peers
    {
        if matches!(mode, LoadMode::Movement | LoadMode::StaggeredMovement) {
            assert_eq!(
                life_inputs.values().sum::<usize>(),
                steps as usize,
                "measurement must admit the full logical workload for every peer"
            );
        }
        assert!(
            receiver.applied > 10,
            "must consume continuing emitted snapshots"
        );
        assert!(
            verified_inputs > 60,
            "must verify receipts, not merely accept movement"
        );
        assert!(
            distance > 50.0,
            "fixture must measure moving, not idle, actors: {distance}"
        );
        let locked = state.lock().unwrap();
        let history = &locked.clients[&player].actor_motion_history;
        let current_life = locked.clients[&player].life_seq;
        let expected_history = if sustained {
            // A freshly accepted life has its baseline plus only the inputs
            // actually sent since respawn, not its predecessor's history.
            (life_inputs.get(&current_life).copied().unwrap_or(0) + 1).min(20)
        } else {
            20
        };
        assert!(
            (sustained && !locked.clients[&player].alive) || history.len() >= expected_history,
            "batch={batch_size} distance={distance} len={} samples={:?}",
            history.len(),
            history.current_samples().collect::<Vec<_>>()
        );
    }
}
