//! Phase 4 — operability benchmark for the authoritative server.
//!
//! Spawns N synthetic clients (default 200), joins them into the
//! arena, drives a steady-state load (orbiting position relays plus a
//! low-rate bullet fire), and runs `run_server_tick` for K seconds at
//! the configured server tick rate. Captures per-tick wallclock time,
//! peak/avg active projectile count, and outbound byte volume per
//! recipient to give a baseline number for "how does the server hold
//! up at 200 players?".
//!
//! Scope: decoded synthetic position relays, real reliable bootstrap ACKs,
//! applied FULL/delta snapshots and production-verified observer receipts. This is
//! not browser/RTC acceptance or a normal moving-client workload: the test-only
//! queue helper fabricates neutral input records while these packets report
//! changing positions and zero velocity. Orbits also ignore map collision.
//! The driver therefore exercises corrections/rejections it does not apply,
//! not merely valid movement load. Report resolved
//! and unavailable geometry separately; nonzero projectiles alone are not
//! evidence that observer projection ran. `ZUBSPACE_SCALE_FULL_ONLY=1` disables
//! codec ACKs for a separate FULL-only stress case, not normal delta bandwidth.
//!
//! Marked `#[ignore]` so a regular `cargo test` sweep skips it. To
//! run a baseline measurement:
//!
//! ```sh
//! cargo test --release --bin zubspace_server \
//!     scale_bench::two_hundred_player_steady_state \
//!     -- --ignored --nocapture
//! ```
//!
//! For a quick sanity check that the bench infrastructure itself
//! still works in debug, run it without `--release` — the absolute
//! numbers won't be meaningful but every assertion still holds.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::arena::{join_arena, register_connection};
use crate::server_tick::run_server_tick;
use crate::state::ServerState;

/// Steady-state load parameters. Defaults chosen to be the smallest
/// numbers that still stress every server tick path: positions go in,
/// snapshots come out, projectiles spawn / step / despawn.
struct BenchConfig {
    client_count: usize,
    duration_seconds: u64,
    /// Synthetic driver tick rate (Hz); the loop sleeps for
    /// `1 / tick_rate` between ticks. Not a production-default assertion.
    tick_rate_hz: u64,
    /// Position-relay cadence per client. 50 ms ≈ 20 Hz matches what
    /// a real Continuum client sends.
    position_interval: Duration,
    /// Bullet-fire cadence per client. Low enough that the registry
    /// doesn't immediately saturate the per-owner cap (default 32).
    fire_interval: Duration,
    /// Grid width for the orbit-center distribution. Together with
    /// `cell_pitch_px` controls how densely clients cluster — a wide
    /// pitch on a small `cells_per_row` packs everyone inside one
    /// near-AOI radius, where tiered visibility doesn't help; a wider
    /// pitch tiers more pairs to MID / FAR.
    cells_per_row: u16,
    /// Pixel distance between adjacent orbit centers.
    cell_pitch_px: u16,
    /// World pixel of the top-left orbit center.
    arena_origin_px: u16,
    /// Optional separate y origin. `None` reuses `arena_origin_px`, which is what every caller but the
    /// density measurement wants; that one re-centres the grid on the bot cluster, whose x and y differ.
    arena_origin_y_px: Option<u16>,
    /// Server bots placed around the arena for the entity-density measurement
    /// (`dense_entity_density_measurement`). Zero keeps the steady-state benchmark's load
    /// identical to its recorded baseline.
    bots: usize,
    /// Active hidden prizes placed across the arena for that same measurement. Zero keeps the
    /// baseline unchanged.
    prizes: usize,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            client_count: 200,
            duration_seconds: 10,
            tick_rate_hz: u64::from(crate::config::DEFAULT_TICK_RATE_HZ),
            position_interval: Duration::from_millis(50),
            fire_interval: Duration::from_millis(500),
            // Mixed-AOI default: 16×16 cells × 384 px pitch ≈ 6144 px
            // square. Neighbours are near; distant pairs are not. This
            // fixture must not be described as an all-pairs near-AOI load.
            cells_per_row: 16,
            cell_pitch_px: 384,
            arena_origin_px: 4096,
            arena_origin_y_px: None,
            bots: 0,
            prizes: 0,
        }
    }
}

/// One synthetic client's local state — what it's about to send next.
struct SynthClient {
    player_id: u16,
    rx: mpsc::Receiver<Vec<u8>>,
    /// Center of the per-client orbit, in world pixels.
    cx: u16,
    cy: u16,
    /// Phase offset into the orbit so 200 clients aren't all at the
    /// same point each tick.
    phase: f32,
    next_position_at: Instant,
    next_fire_at: Instant,
    position_sequence: u32,
    applied_snapshot: Option<u16>,
    /// Snapshots this client actually applied. A receipt rejection says how stale the *latest applied* snapshot
    /// is; this says how many arrived at all, which is the difference between a stream that stalled and a stream
    /// that was never fast enough.
    snapshots_applied: u64,
    directive: Option<crate::combat_timeline::ObserverTimelineDirectiveV1>,
    snapshot_bases: std::collections::VecDeque<(u16, crate::snapshot_wire::SnapshotEntityStateMap)>,
}

fn build_position_packet(sequence: u32, x: u16, y: u16, weapon_data: u16) -> Vec<u8> {
    // Mirrors `tests.rs::client_position_packet_with_weapon` so the
    // server's `arena/positions.rs` accept path treats it like real
    // client traffic.
    let mut packet = vec![0; 22];
    packet[0] = crate::arena::CLIENT_POSITION;
    packet[1] = 7;
    packet[2..6].copy_from_slice(&0_u32.to_le_bytes());
    packet[8..10].copy_from_slice(&y.to_le_bytes());
    packet[12..14].copy_from_slice(&x.to_le_bytes());
    packet[18..20].copy_from_slice(&900_u16.to_le_bytes());
    packet[20..22].copy_from_slice(&weapon_data.to_le_bytes());
    packet.resize(crate::continuum_position::CLIENT_POSITION_EXTRA_LEN, 0);
    packet[28..32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(sequence as u16).to_le_bytes(),
    );
    crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(7, x, y, 0, 0)
        .encode(&mut packet);
    packet
}

/// Place this client's orbit center on the configured grid.
/// `cells_per_row * cells_per_row * cell_pitch_px` must fit inside
/// the arena minus the ±256 px orbit swing — caller's responsibility.
fn orbit_center(client_index: usize, config: &BenchConfig) -> (u16, u16) {
    let modulus = (config.cells_per_row as usize) * (config.cells_per_row as usize);
    let cell = (client_index % modulus.max(1)) as u16;
    let cell_x = cell % config.cells_per_row;
    let cell_y = cell / config.cells_per_row;
    let x = config.arena_origin_px + cell_x * config.cell_pitch_px;
    let y =
        config.arena_origin_y_px.unwrap_or(config.arena_origin_px) + cell_y * config.cell_pitch_px;
    (x, y)
}

#[test]
#[ignore = "benchmark — opt in with cargo test --release ... -- --ignored"]
fn two_hundred_player_steady_state() {
    run_bench(BenchConfig::default());
}

/// Same client count and load profile as the dense baseline, but
/// spread across the full 16384 px arena (14×15 cells × 1024 px
/// pitch). Most pairs now sit beyond 2048 px (near-AOI) and many
/// beyond 2560 px (mid-AOI), so AOI-tiered visibility actually
/// filters traffic. Snapshot emission is always enabled; bootstrap receipt
/// completion, not the removed snapshot feature flag, opens delivery.
#[test]
#[ignore = "benchmark — opt in with cargo test --release ... -- --ignored"]
fn two_hundred_player_spread_across_full_arena() {
    let config = BenchConfig {
        // 14 × 14 = 196 cells, the 4 wrap-around clients land back on
        // row 0. Cell pitch 1024 px ⇒ neighbours are at the near-AOI
        // edge, two cells apart are firmly in mid-AOI (2048 px), and
        // anything 3+ cells apart is mid → far (≥ 3072 px).
        cells_per_row: 14,
        cell_pitch_px: 1024,
        // 14×1024 = 14336 px wide; origin 1024 ⇒ rightmost cell at
        // 14336+256 (orbit swing) = 14592, well inside 16384.
        arena_origin_px: 1024,
        ..BenchConfig::default()
    };
    run_bench(config);
}

/// Tiny variant that runs in debug-mode `cargo test` to guarantee
/// the bench driver itself still compiles and works — without this,
/// drift in `register_connection`, `join_arena`, the position-packet
/// shape, or the projectile registry could silently break the
/// benchmark and we'd only notice the next time someone opts in.
/// How many bots, projectiles and prizes sit inside a recipient's AOI in a dense arena.
///
/// A measurement, not a gate. The scenario is synthetic - neutral-input orbit clients on a 4x4 grid, server bots
/// where `initialize_configured_bots` places them, stationary prizes on the client grid - so it sizes the
/// per-recipient coefficient of each entity type rather than claiming a production density distribution. The
/// only assertions are the ones `run_bench` already makes.
#[test]
#[ignore = "measurement: dense bot/projectile/prize density for the AOI visibility-index decision"]
fn dense_entity_density_measurement() {
    run_bench(BenchConfig {
        // 16 recipients on a 4x4 grid at 96 px pitch: every recipient is inside every other's near radius, so the
        // *density* is saturated, while the recipient count stays small enough that the synthetic driver keeps its
        // own snapshot receipts. The entity counts below are the quantity being measured and are not reduced. A
        // 64-recipient version of this scenario costs 46-65 ms per tick and the receipt check rejects it; that
        // number is arena tick-cost evidence, recorded in the item, not a density reading.
        client_count: 16,
        duration_seconds: 5,
        // 20 Hz, not 60: `bench_driver_smokes_with_eight_clients` already runs at 20 Hz, and the density question
        // is about positions and counts, not pacing.
        tick_rate_hz: 20,
        cells_per_row: 4,
        cell_pitch_px: 96,
        bots: 128,
        prizes: 256,
        ..BenchConfig::default()
    });
}

#[test]
fn bench_driver_smokes_with_eight_clients() {
    let config = BenchConfig {
        client_count: 8,
        duration_seconds: 1,
        tick_rate_hz: 20,
        position_interval: Duration::from_millis(50),
        fire_interval: Duration::from_millis(200),
        ..BenchConfig::default()
    };
    run_bench(config);
}

/// Public entry point so future variants (different N, different
/// fire cadence, etc.) can reuse the same driver.
fn run_bench(config: BenchConfig) {
    let diagnostic_subscriber = std::env::var_os("ZUBSPACE_SCALE_BENCH_TRACE").map(|_| {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .finish()
    });
    let _diagnostic_guard = diagnostic_subscriber.map(tracing::subscriber::set_default);
    let start_wallclock = Instant::now();
    // A density measurement needs the recipients in the same neighbourhood as the bots, and
    // `initialize_configured_bots` places them in a ring around the zone's spawn - thousands of pixels from the
    // configured origin, where the measured bot density is zero for every recipient. Re-centre the client grid on
    // that ring before any client joins or moves.
    let mut config = config;
    let state = Arc::new(Mutex::new(ServerState::new(crate::config::ServerConfig {
        tick_rate_hz: config
            .tick_rate_hz
            .try_into()
            .expect("benchmark tick rate fits server config"),
        bot_count: config.bots,
        ..crate::config::ServerConfig::default()
    })));
    if config.bots > 0 {
        state
            .lock()
            .expect("state poisoned")
            .initialize_configured_bots();
        let (cx, cy) = {
            let guard = state.lock().expect("state poisoned");
            let count = guard.bots.len().max(1) as f32;
            let (sx, sy) = guard
                .bots
                .iter()
                .fold((0.0_f32, 0.0_f32), |(ax, ay), bot| (ax + bot.x, ay + bot.y));
            ((sx / count).round() as u16, (sy / count).round() as u16)
        };
        let half_span = (config.cells_per_row / 2).saturating_mul(config.cell_pitch_px);
        config.arena_origin_px = cx.saturating_sub(half_span);
        config.arena_origin_y_px = Some(cy.saturating_sub(half_span));
    }
    let mut clients = register_synthetic_clients(&state, &config);
    if config.prizes > 0 {
        // Spread over the same grid the clients orbit, so prizes are neither all inside one recipient's AOI nor
        // all outside every one. The lifetime must be wrap-safe: `expire_hidden_prizes` compares with
        // `continuum_tick_reached`, so `u32::MAX` reads as already reached and all but the newest prize vanish on
        // the next spawn (measured: 256 requested, 1 active).
        let mut guard = state.lock().expect("state poisoned");
        let pitch = config.cell_pitch_px.max(1);
        let columns = config.cells_per_row.max(1);
        let origin_y = config.arena_origin_y_px.unwrap_or(config.arena_origin_px);
        for index in 0..config.prizes {
            let x = config.arena_origin_px + ((index % columns as usize) as u16) * pitch;
            let y = origin_y + ((index / columns as usize) as u16) * pitch;
            let _ =
                crate::hidden_prizes::create_death_collectible(&mut guard, x, y, 0, 0, 1_000_000);
        }
    }
    eprintln!(
        "scale_bench: {} clients joined, running {}s at {}Hz",
        config.client_count, config.duration_seconds, config.tick_rate_hz
    );

    let tick_interval = Duration::from_nanos(1_000_000_000 / config.tick_rate_hz);
    let total_duration = Duration::from_secs(config.duration_seconds);

    let mut tick_durations: Vec<Duration> =
        Vec::with_capacity((config.duration_seconds * config.tick_rate_hz) as usize + 1);
    let mut projectile_count_samples: Vec<usize> = Vec::with_capacity(tick_durations.capacity());
    let mut total_outbound_bytes: u64 = 0;
    let mut total_outbound_packets: u64 = 0;

    let mut now = Instant::now();
    let motion_started_at = now;
    let bench_deadline = now + total_duration;
    while now < bench_deadline {
        // Each client pushes a fresh position packet on its cadence,
        // then optionally a fire request. Position carries weapon=0
        // most of the time; on the fire cadence we set weapon to a
        // type-1 bullet.
        for client in clients.iter_mut() {
            // Bootstrap defines the first legal sequence. Sending arbitrary
            // pre-directive positions then jumping to that floor exhausts the
            // input window when other tests advanced the sequence allocator.
            if client.directive.is_none() {
                continue;
            }
            if now >= client.next_position_at {
                let (x, y) = synth_position(client, now.duration_since(motion_started_at));
                let weapon = if client.applied_snapshot.is_some() && now >= client.next_fire_at {
                    client.next_fire_at = now + config.fire_interval;
                    bullet_weapon_data()
                } else {
                    0
                };
                let sequence = client.position_sequence;
                let packet = build_position_packet(sequence, x, y, weapon);
                client.position_sequence = client.position_sequence.wrapping_add(1);
                let mut state = state.lock().expect("state poisoned");
                if let (Some(snapshot_id), Some(directive)) =
                    (client.applied_snapshot, client.directive)
                {
                    let runtime = state
                        .active_player_runtime_identity(client.player_id)
                        .expect("synthetic runtime");
                    let handle = state
                        .clients
                        .get_mut(&client.player_id)
                        .expect("synthetic client");
                    handle
                        .observer_combat_timeline
                        .verify_receipt(
                            runtime,
                            crate::wire::current_tick(),
                            crate::combat_timeline::CombatViewReceiptV1 {
                                observer_life_seq: runtime.life.life_seq,
                                input_sequence: sequence as u16,
                                timeline_generation: directive.generation,
                                latest_game_applied_snapshot_id: snapshot_id,
                                flags: 0,
                            },
                            &handle.observer_presentation_history,
                        )
                        .unwrap_or_else(|reason| {
                            let applied = handle.snapshot_history.iter()
                                .find(|frame| frame.snapshot_id == snapshot_id)
                                .map(|frame| frame.server_tick);
                            // The discriminator this bench was missing. If the server's newest published snapshot is
                            // far ahead of `applied_snapshot`, the server did publish newer frames and this client
                            // never applied them - a throttled or dropped stream, which is the egress-cap case.
                            // If it is not ahead, the server itself stopped publishing to this client. Reading the
                            // rejection without this meant guessing between the two.
                            let server_newest = handle.snapshot_history.iter()
                                .map(|frame| frame.snapshot_id)
                                .max();
                            panic!("synthetic receipt rejected: {reason:?}; player={} sequence={} applied_snapshot={} applied_tick={applied:?} server_newest_snapshot={server_newest:?} snapshots_applied={} current_tick={} directive={directive:?} preceding_tick_work={:?} send_pacing={:?}",
                                client.player_id, sequence, snapshot_id, client.snapshots_applied, crate::wire::current_tick(),
                                tick_durations.iter().rev().take(8).collect::<Vec<_>>(), handle.send_pacing);
                        });
                }
                state.queue_position_input(client.player_id, packet);
                client.next_position_at = now + config.position_interval;
            }
        }

        let tick_started = Instant::now();
        let _result = run_server_tick(&state, now);
        let tick_elapsed = tick_started.elapsed();
        tick_durations.push(tick_elapsed);

        // Drain everything each client received, accumulating volume.
        for client in clients.iter_mut() {
            while let Ok(packet) = client.rx.try_recv() {
                total_outbound_bytes += packet.len() as u64;
                total_outbound_packets += 1;
                consume_server_packet(&state, client, &packet);
            }
        }

        let projectile_count = state
            .lock()
            .expect("state poisoned")
            .active_projectiles
            .len();
        projectile_count_samples.push(projectile_count);

        // Pace the loop so we approximately match the configured
        // tick rate. If the previous tick already took longer than
        // the interval, just continue immediately.
        let next_wakeup = now + tick_interval;
        let wait_for = next_wakeup.saturating_duration_since(Instant::now());
        if wait_for > Duration::ZERO {
            std::thread::sleep(wait_for);
        }
        now = Instant::now();
    }

    print_density_report(&state);
    print_report(
        &config,
        start_wallclock.elapsed(),
        &tick_durations,
        &projectile_count_samples,
        total_outbound_bytes,
        total_outbound_packets,
    );
    // Disclose the exercised geometry path instead of treating projectile
    // count as proof that this synthetic driver measures observer projection.
    let state_guard = state.lock().expect("state poisoned");
    let geometry = state_guard
        .active_projectiles
        .presentation_geometry_metrics();
    eprintln!("  geometry paths : {geometry:?}");
    let observer_frames: usize = state_guard
        .clients
        .values()
        .map(|client| client.observer_presentation_history.len())
        .sum();
    let motion_samples: usize = state_guard
        .clients
        .values()
        .map(|client| client.actor_motion_history.len())
        .sum();
    eprintln!(
        "  retained input : {observer_frames} observer frames, {motion_samples} actor motion samples"
    );
    let correction_clients = state_guard
        .clients
        .values()
        .filter(|client| client.pending_movement_correction.is_some())
        .count();
    let nonneutral_input_records: usize = state_guard
        .clients
        .values()
        .map(|client| {
            client
                .recent_input_records
                .iter()
                .filter(|record| record.input.bits != 0)
                .count()
        })
        .sum();
    eprintln!(
        "  admission scope: synthetic neutral-input orbit; {correction_clients} clients awaiting correction application; {nonneutral_input_records} retained nonneutral input records; NOT valid moving-player capacity acceptance"
    );
    drop(state_guard);

    // Sanity assertions — the bench is also a regression test: if
    // the server tick rate collapses or projectiles never spawn,
    // these fail loudly.
    let peak_tick = tick_durations.iter().max().copied().unwrap_or_default();
    let avg_projectiles = projectile_count_samples.iter().sum::<usize>() as f64
        / projectile_count_samples.len().max(1) as f64;
    assert!(
        peak_tick < Duration::from_millis(200),
        "peak server tick under {} synthetic clients exceeded 200 ms ({} ms)",
        config.client_count,
        peak_tick.as_millis()
    );
    assert!(
        avg_projectiles >= 1.0,
        "expected projectiles to accumulate during the bench (avg={:.1})",
        avg_projectiles
    );
    assert!(
        geometry.resolved_total > 0,
        "benchmark must exercise resolved observer geometry: {geometry:?}"
    );
}

fn consume_server_packet(state: &Arc<Mutex<ServerState>>, client: &mut SynthClient, packet: &[u8]) {
    if let Some((id, _)) = crate::reliable::reliable_parts(packet) {
        let identity = state
            .lock()
            .expect("state poisoned")
            .active_player_session_identity(client.player_id)
            .expect("synthetic session");
        assert!(crate::reliable::receive_reliable_acks(
            state,
            identity,
            &crate::reliable::reliable_ack(id)
        ));
    } else if packet.first() == Some(&crate::snapshot_wire::SNAPSHOT_PACKET) {
        let frame =
            crate::snapshot_wire::SnapshotFrame::parse(packet).expect("synthetic snapshot decodes");
        let base = client
            .snapshot_bases
            .iter()
            .find(|(id, _)| *id == frame.header.base_id)
            .map(|(_, entities)| entities);
        let applied = crate::snapshot_wire::delta_apply(base, &frame)
            .expect("synthetic snapshot applies against its exact retained base");
        client
            .snapshot_bases
            .push_back((frame.header.snapshot_id, applied.entities));
        while client.snapshot_bases.len() > crate::snapshot_wire::SNAPSHOT_HISTORY_CAPACITY {
            client.snapshot_bases.pop_front();
        }
        if std::env::var_os("ZUBSPACE_SCALE_FULL_ONLY").is_none() {
            crate::connection::record_snapshot_ack_at(
                state,
                client.player_id,
                &crate::snapshot_wire::SnapshotAckPacket {
                    snapshot_id: frame.header.snapshot_id,
                }
                .encode(),
                Instant::now(),
            );
        }
        client.applied_snapshot = Some(frame.header.snapshot_id);
        client.snapshots_applied += 1;
        for event in frame.events {
            if let crate::snapshot_wire::SnapshotEvent::ObserverTimeline(directive) = event {
                if client.directive.is_none() {
                    client.position_sequence = u32::from(directive.effective_from_input_sequence);
                }
                client.directive = Some(directive);
            }
        }
    }
}

fn register_synthetic_clients(
    state: &Arc<Mutex<ServerState>>,
    config: &BenchConfig,
) -> Vec<SynthClient> {
    let mut clients = Vec::with_capacity(config.client_count);
    let now = Instant::now();
    for index in 0..config.client_count {
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(state, tx);
        join_arena(state, player_id, 0);
        let (cx, cy) = orbit_center(index, config);
        clients.push(SynthClient {
            player_id,
            rx,
            cx,
            cy,
            phase: (index as f32) * 0.31,
            next_position_at: now + Duration::from_millis((index as u64) % 50),
            next_fire_at: now + Duration::from_millis(200 + (index as u64) % 300),
            position_sequence: index as u32,
            applied_snapshot: None,
            snapshots_applied: 0,
            directive: None,
            snapshot_bases: std::collections::VecDeque::new(),
        });
    }
    clients
}

/// Compute a position on a 256-pixel orbit around `(cx, cy)` based on
/// elapsed wallclock time and the client's phase offset. Real-shape
/// movement isn't needed; the only requirements are (1) the position
/// changes between updates so the relay path isn't a no-op, and (2)
/// it stays within arena bounds.
fn synth_position(client: &SynthClient, elapsed: Duration) -> (u16, u16) {
    let t = elapsed.as_secs_f32();
    let angle = client.phase + t * std::f32::consts::FRAC_PI_2;
    let dx = (angle.cos() * 256.0) as i32;
    let dy = (angle.sin() * 256.0) as i32;
    let x = (client.cx as i32 + dx).clamp(0, u16::MAX as i32) as u16;
    let y = (client.cy as i32 + dy).clamp(0, u16::MAX as i32) as u16;
    (x, y)
}

#[test]
fn synthetic_orbit_advances_against_one_fixed_epoch() {
    let (_, rx) = mpsc::channel();
    let client = SynthClient {
        player_id: 0,
        rx,
        cx: 4096,
        cy: 4096,
        phase: 0.0,
        next_position_at: Instant::now(),
        next_fire_at: Instant::now(),
        position_sequence: 0,
        applied_snapshot: None,
        snapshots_applied: 0,
        directive: None,
        snapshot_bases: std::collections::VecDeque::new(),
    };
    let points: Vec<_> = (0..4)
        .map(|second| synth_position(&client, Duration::from_secs(second)))
        .collect();
    for (actual, expected) in points.iter().zip([
        (4352_u16, 4096_u16),
        (4096, 4352),
        (3840, 4096),
        (4096, 3840),
    ]) {
        assert!(actual.0.abs_diff(expected.0) <= 1);
        assert!(actual.1.abs_diff(expected.1) <= 1);
    }
    assert_ne!(points[0], points[1]);
}

/// Weapon-data encoding for a level-1 bullet (type 1). Continuum
/// packs: low 5 bits = type, next 2 bits = level, etc. Type=1, level=0
/// is a plain bullet that the server's projectile registry will accept.
fn bullet_weapon_data() -> u16 {
    1
}

/// Per-recipient bot/projectile/prize density at the shipped AOI radii.
///
/// This sizes the case for extending the player visibility index to other entity types. That index
/// contains players only; bots are tiered by a direct scan over `state.bots` for **every** recipient, which is
/// the cost an index would remove, while projectiles are not snapshot entities at all (they render from accepted
/// weapon state on the firing player's entry) and hidden prizes travel on their own path. Counting all four shows
/// those last two coefficients instead of assuming them.
///
/// The scenario is synthetic: neutral-input orbit clients, server bots wherever `initialize_configured_bots`
/// places them (a ring around the spawn), and stationary prizes on the client grid. It sizes the per-recipient
/// coefficient of each entity type; it is not a production density distribution.
fn print_density_report(state: &Arc<Mutex<ServerState>>) {
    let guard = state.lock().expect("state poisoned");
    let recipients: Vec<(f32, f32)> = guard
        .clients
        .values()
        .filter_map(|client| {
            crate::arena::large_position_coordinates(client.last_position.as_deref()?)
                .map(|(x, y)| (x as f32, y as f32))
        })
        .collect();
    let bots: Vec<(f32, f32)> = guard
        .bots
        .iter()
        .filter(|bot| bot.alive())
        .map(|bot| (bot.x, bot.y))
        .collect();
    let prizes: Vec<(f32, f32)> = guard
        .active_hidden_prizes
        .iter()
        .map(|prize| (prize.x as f32, prize.y as f32))
        .collect();
    let projectiles: Vec<(f32, f32)> = guard
        .active_projectiles
        .iter()
        .map(|projectile| (projectile.x as f32, projectile.y as f32))
        .collect();

    eprintln!(
        "entity density ({} recipients with a position, {} alive bots, {} active prizes, {} live projectiles, \
         {} joined players; near {:.0} px, mid {:.0} px):",
        recipients.len(),
        bots.len(),
        prizes.len(),
        projectiles.len(),
        guard.clients.len(),
        crate::visibility::NEAR_AOI_RADIUS_PX,
        crate::visibility::MID_AOI_RADIUS_PX,
    );
    for (label, others) in [
        ("players", recipients.clone()),
        ("bots", bots),
        ("prizes", prizes),
        ("projectiles", projectiles),
    ] {
        let counts: Vec<(usize, usize)> = recipients
            .iter()
            .map(|&(rx, ry)| {
                let mut near = 0usize;
                let mut mid = 0usize;
                for &(x, y) in &others {
                    let dx = x - rx;
                    let dy = y - ry;
                    match crate::visibility::aoi_tier_for_distance_squared(
                        dx * dx + dy * dy,
                        crate::visibility::NEAR_AOI_RADIUS_PX,
                        crate::visibility::MID_AOI_RADIUS_PX,
                    ) {
                        crate::visibility::AoiTier::Near => near += 1,
                        crate::visibility::AoiTier::Mid => mid += 1,
                        crate::visibility::AoiTier::Far => {}
                    }
                }
                (near, mid)
            })
            .collect();
        let near: Vec<usize> = counts.iter().map(|&(near, _)| near).collect();
        let mid: Vec<usize> = counts.iter().map(|&(_, mid)| mid).collect();
        eprintln!(
            "  {label:<12} near {}   mid {}   (per recipient)",
            density_summary(&near),
            density_summary(&mid)
        );
    }
    drop(guard);
}

fn density_summary(values: &[usize]) -> String {
    if values.is_empty() {
        return "n/a".to_owned();
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let pick = |pct: f64| sorted[((pct / 100.0) * (sorted.len() - 1) as f64).round() as usize];
    let mean = values.iter().sum::<usize>() as f64 / values.len() as f64;
    format!(
        "mean {mean:.1} p50 {} p95 {} max {}",
        pick(50.0),
        pick(95.0),
        sorted.last().copied().unwrap_or(0)
    )
}

fn print_report(
    config: &BenchConfig,
    elapsed_wallclock: Duration,
    tick_durations: &[Duration],
    projectile_count_samples: &[usize],
    total_outbound_bytes: u64,
    total_outbound_packets: u64,
) {
    let tick_count = tick_durations.len();
    let mut sorted: Vec<Duration> = tick_durations.to_vec();
    sorted.sort();
    let p50 = percentile(&sorted, 50.0);
    let p95 = percentile(&sorted, 95.0);
    let p99 = percentile(&sorted, 99.0);
    let peak = sorted.last().copied().unwrap_or_default();
    let avg_tick_us = if tick_count > 0 {
        tick_durations
            .iter()
            .map(|d| d.as_micros() as u64)
            .sum::<u64>()
            / tick_count as u64
    } else {
        0
    };
    let peak_projectiles = projectile_count_samples.iter().max().copied().unwrap_or(0);
    let avg_projectiles = if projectile_count_samples.is_empty() {
        0.0
    } else {
        projectile_count_samples.iter().sum::<usize>() as f64
            / projectile_count_samples.len() as f64
    };
    let bytes_per_client_per_sec = total_outbound_bytes as f64
        / (config.client_count.max(1) as f64 * config.duration_seconds.max(1) as f64);
    let kib_per_client_per_sec = bytes_per_client_per_sec / 1024.0;
    let grid_extent = config.cells_per_row * config.cell_pitch_px;
    eprintln!(
        "scale_bench summary ({} clients, {}s, {}×{} grid @ {} px pitch ≈ {} px span):",
        config.client_count,
        config.duration_seconds,
        config.cells_per_row,
        config.cells_per_row,
        config.cell_pitch_px,
        grid_extent,
    );
    eprintln!("  wallclock      : {:.2}s", elapsed_wallclock.as_secs_f64());
    eprintln!(
        "  ticks observed : {} (target {})",
        tick_count,
        config.duration_seconds * config.tick_rate_hz
    );
    eprintln!("  tick wallclock : avg {} µs", avg_tick_us);
    eprintln!("                   p50 {} µs", p50.as_micros());
    eprintln!("                   p95 {} µs", p95.as_micros());
    eprintln!("                   p99 {} µs", p99.as_micros());
    eprintln!("                   max {} µs", peak.as_micros());
    eprintln!(
        "  projectiles    : avg {:.1}  peak {}",
        avg_projectiles, peak_projectiles
    );
    eprintln!(
        "  outbound       : {} packets, {} bytes total",
        total_outbound_packets, total_outbound_bytes
    );
    eprintln!(
        "                   per-client {:.2} KiB/s",
        kib_per_client_per_sec
    );
}

fn percentile(sorted: &[Duration], pct: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((pct / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
