#![cfg_attr(test, allow(unused_imports))]

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, ErrorKind};
#[cfg(test)]
use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[path = "zubspace_server/client_debug.rs"]
mod client_debug;

#[path = "zubspace_server/admin.rs"]
mod admin;
#[path = "zubspace_server/arena.rs"]
mod arena;
#[path = "zubspace_server/arena_settings.rs"]
mod arena_settings;
#[path = "../authority.rs"]
mod authority;
#[path = "../authority_damage.rs"]
mod authority_damage;
#[path = "../authority_identity.rs"]
mod authority_identity;
#[path = "../authority_inputs.rs"]
mod authority_inputs;
#[path = "../authority_prizes.rs"]
mod authority_prizes;
#[path = "../authority_projectiles.rs"]
mod authority_projectiles;
#[path = "zubspace_server/binary_identity.rs"]
mod binary_identity;
#[path = "../bm2.rs"]
mod bm2;
#[path = "../clock_bootstrap.rs"]
mod clock_bootstrap;
#[path = "../combat_timeline.rs"]
mod combat_timeline;
#[cfg(test)]
#[path = "../combat_timeline_reference_model.rs"]
mod combat_timeline_reference_model;
#[path = "zubspace_server/config.rs"]
mod config;
#[path = "zubspace_server/connection.rs"]
mod connection;
// Measurement policy is being integrated with authenticated RTC probes and
// the server-owned spectator lifecycle; it is not enforcement by itself.
#[allow(dead_code)]
#[path = "zubspace_server/connection_quality.rs"]
mod connection_quality;
#[path = "../protocol/connection_quality.rs"]
mod connection_quality_protocol;
#[path = "../continuum.rs"]
mod continuum;
#[path = "../protocol/forced_spectator.rs"]
mod forced_spectator_protocol;
#[path = "zubspace_server/spawn_idle.rs"]
mod spawn_idle;
#[path = "../spectator_view.rs"]
mod spectator_view;
// `src/continuum.rs` references `crate::CONTINUUM_ARENA_SETTINGS_SIZE`; the
// server's copy lives under arena_settings, so re-export it at the crate
// root for shared modules to find.
pub(crate) use arena_settings::CONTINUUM_ARENA_SETTINGS_SIZE;
#[path = "../continuum_position.rs"]
mod continuum_position;
#[path = "../continuum_weapon.rs"]
mod continuum_weapon;
#[path = "../door.rs"]
mod door;
#[path = "zubspace_server/doors.rs"]
mod doors;
#[path = "../file_transfer_chunk.rs"]
mod file_transfer_chunk;
#[path = "../gameplay/shared_rules.rs"]
mod gameplay_rules;
#[path = "zubspace_server/hidden_prizes.rs"]
mod hidden_prizes;
#[path = "zubspace_server/http.rs"]
mod http;
#[path = "zubspace_server/identity.rs"]
mod identity;
#[path = "zubspace_server/limits.rs"]
mod limits;
#[path = "zubspace_server/log_queue.rs"]
mod log_queue;
#[path = "zubspace_server/log_targets.rs"]
mod log_targets;
#[allow(dead_code)]
#[path = "../lvz/archive.rs"]
mod lvz_archive;
#[path = "../protocol/movement_correction.rs"]
mod movement_correction;
#[path = "../protocol/objectives.rs"]
mod objective_protocol;
#[path = "zubspace_server/objectives.rs"]
mod objectives;
#[path = "zubspace_server/observability.rs"]
mod observability;
#[allow(dead_code)]
#[path = "../packet_io.rs"]
mod packet_io;
#[path = "../physics.rs"]
mod physics;
#[path = "zubspace_server/player_events.rs"]
mod player_events;
#[path = "../protocol/portal_placement.rs"]
mod portal_placement;
#[path = "../protocol/portal_warp.rs"]
mod portal_warp;
#[path = "../protocol/prize_application.rs"]
mod prize_application;
#[path = "zubspace_server/qa_trace.rs"]
mod qa_trace;
#[path = "../protocol/qa_trace.rs"]
mod qa_trace_protocol;
#[path = "zubspace_server/rate_limit.rs"]
mod rate_limit;
#[path = "../protocol/realtime_event.rs"]
mod realtime_event;
#[path = "zubspace_server/reliable.rs"]
mod reliable;
#[path = "../render_limits.rs"]
mod render_limits;
#[path = "../respawn.rs"]
mod respawn;
#[path = "zubspace_server/roster.rs"]
mod roster;
#[path = "zubspace_server/rtc.rs"]
mod rtc;
#[path = "zubspace_server/security.rs"]
mod security;
#[path = "zubspace_server/server_damage.rs"]
mod server_damage;
#[path = "zubspace_server/server_explosions.rs"]
mod server_explosions;
#[path = "zubspace_server/server_tick.rs"]
mod server_tick;
#[path = "zubspace_server/session.rs"]
mod session;
#[path = "zubspace_server/session_summary.rs"]
mod session_summary;
#[path = "../gameplay/player_movement.rs"]
mod shared_player_movement;
#[path = "zubspace_server/sim_observation.rs"]
mod sim_observation;
#[path = "zubspace_server/simulated_loss.rs"]
mod simulated_loss;
#[path = "zubspace_server/snapshot.rs"]
mod snapshot;
#[path = "../protocol/snapshot.rs"]
mod snapshot_wire;
#[path = "zubspace_server/state.rs"]
mod state;
#[path = "zubspace_server/threat_presentation.rs"]
mod threat_presentation;
#[allow(dead_code)]
#[path = "../vie.rs"]
mod vie;
#[path = "zubspace_server/visibility.rs"]
mod visibility;
#[path = "zubspace_server/wire.rs"]
mod wire;
#[path = "zubspace_server/zone.rs"]
mod zone;
#[path = "../zone_wire.rs"]
mod zone_wire;

use admin::{AdminCommand, admin_command_response, parse_admin_command};
#[cfg(test)]
use arena::{
    ARENA_LOGIN, CLIENT_POSITION, FREQUENCY_CHANGE_REQUEST, LEAVE_ARENA_REQUEST, SET_COORDINATES,
    SHIP_CHANGE_REQUEST, broadcast_position, broadcast_position_observable, disconnect_player,
    join_arena, large_position_from_client, leave_arena, normalized_ship, register_connection,
    set_coordinates_packet, update_frequency, update_ship,
};
use config::{ServerConfig, ServerLogLevel};
#[cfg(test)]
pub(crate) use connection::handle_client;
use connection::handle_client_with_started_at;
#[cfg(test)]
use identity::{
    login_name, login_player_identity, normalized_login_name, server_at_capacity,
    set_player_identity_and_name,
};
#[cfg(test)]
use objectives::{
    ATTACH_REQUEST, BALL_FIRE_REQUEST, BALL_GOAL_REQUEST, BALL_PICKUP_REQUEST,
    DROP_ATTACHED_REQUEST, DROP_BRICK_REQUEST, DROP_FLAGS_REQUEST, FLAG_REQUEST,
    current_flag_packets, current_turret_packets, expire_bricks as test_expire_bricks,
    relay_attach_request, relay_ball_fire, relay_ball_goal, relay_ball_pickup, relay_drop_attached,
    relay_drop_brick, relay_drop_flags, relay_flag_request,
};
#[cfg(test)]
use player_events::{
    CHAT_MESSAGE_REQUEST, DEATH_REQUEST, SPECTATE_REQUEST, TAKE_PRIZE_REQUEST, relay_chat,
    relay_death, relay_prize_pickup, update_spectate,
};
#[cfg(test)]
use reliable::{reliable_ack, reliable_parts, send_reliable_to, send_to};
#[cfg(test)]
use roster::{SET_BANNER_REQUEST, update_banner};
#[cfg(test)]
use security::{SECURITY_RESPONSE, update_security_report};
#[cfg(test)]
use session::{
    FILE_REQUEST, NEWS_REQUEST, PASSWORD_LOGIN_CONTINUUM, serve_file_request, serve_news_request,
};
use state::ServerState;
use tracing::{error, info};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::prelude::*;
const CORE_BYTE: u8 = 0x00;
pub(crate) const DOOR_FIRST_TILE_ID: u8 = 162;
pub(crate) const DOOR_LAST_TILE_ID: u8 = 169;
pub(crate) const OPEN_DOOR_TILE_ID: u8 = DOOR_LAST_TILE_ID + 1;
pub(crate) const DOOR_MODE_LOCAL_INCREMENT: i16 = i16::MIN;
fn main() -> io::Result<()> {
    let started_at = Instant::now();
    let config = ServerConfig::from_env_args(env::args()).map_err(|err| {
        io::Error::new(
            if err.starts_with("usage:") {
                ErrorKind::InvalidInput
            } else {
                ErrorKind::Other
            },
            err,
        )
    })?;
    init_logging(
        config.log_level,
        &config.log_filter,
        config.log_file.as_deref(),
        config.log_json,
    )?;
    let zone_catalog = zone::load_server_zone_catalog(
        config.zone_bundle_root.as_deref(),
        config.allow_zone_overrides,
    )
    .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))?;
    config
        .validate_zone_ids(zone_catalog.zone_ids())
        .map_err(|err| io::Error::new(ErrorKind::InvalidInput, err))?;
    // Render the bounded discovery response from the same validated catalog
    // before installation and before the listener binds. HTTP requests never
    // perform map/LVZ parsing or trigger the embedded OnceLock cold path.
    let zone_catalog_json = connection::server_zone_catalog_json(zone_catalog.maps());
    zone::install_server_zone_catalog(zone_catalog)
        .map_err(|err| io::Error::new(ErrorKind::InvalidData, err))?;
    validate_rtc_udp_socket_availability(&config)?;
    if cfg!(debug_assertions) || config.qa_trace_upload || config.client_debug_trace {
        binary_identity::initialize();
    }
    let listener = TcpListener::bind(&config.listen_addr)?;
    arena_settings::activate_settings_profile(&config.default_zone)
        .expect("validated default zone must have an arena-settings profile");
    let state = Arc::new(Mutex::new(ServerState::new_with_zone_catalog(
        config.clone(),
        zone_catalog_json,
    )));
    state
        .lock()
        .expect("server state should not be poisoned")
        .initialize_configured_bots();
    let _metrics_listener = observability::start_metrics_listener(&state, started_at)?;
    info!(
        bind = %config.listen_addr,
        // [REVIEW M17] Surface where the bind came from so a positional
        // argument or env var overriding the config file is visible.
        bind_source = %config.listen_addr_source.as_str(),
        rtc = %format!("http://{}/v1/rtc/offer", config.listen_addr),
        zone = %config.default_zone,
        zone_bundle_root = %config.zone_bundle_root.as_deref().unwrap_or("embedded-only"),
        allow_zone_overrides = config.allow_zone_overrides,
        max_players = config.max_players,
        bot_count = config.bot_count,
        maxed_spawn_loadout = config.maxed_spawn_loadout,
        bot_ship_types = ?config.bot_ship_types.iter().map(|ship| ship + 1).collect::<Vec<_>>(),
        tick_rate_hz = config.tick_rate_hz,
        log_level = %config.log_level.as_str(),
        log_filter = %config.log_filter,
        client_debug_trace = config.client_debug_trace,
        log_file = %config.log_file.as_deref().unwrap_or("stdout"),
        sim_observe = config.sim_observe,
        observer_network_profile = %config.observer_network_profile.as_str(),
        movement_snapback = config.movement_snapback,
        tx_loss_millionths = config.simulated_tx_loss.probability_millionths(),
        tx_loss_seed = ?config.simulated_tx_loss.configured_seed(),
        tx_loss_after_bootstrap = config.simulated_tx_loss_after_bootstrap,
        rtc_udp_ports = %config.rtc_udp_port_range
            .map(|range| format!("{}-{}", range.min, range.max))
            .unwrap_or_else(|| "ephemeral".to_string()),
        rtc_udp_bind_ip = %config.rtc_udp_bind_ip
            .map(|address| address.to_string())
            .unwrap_or_else(|| "0.0.0.0 (wildcard)".to_string()),
        rtc_udp_path_mtu = %config.rtc_udp_path_mtu
            .map(|mtu| mtu.to_string())
            .unwrap_or_else(|| "default".to_string()),
        rtc_sctp_max_packet_size = config.rtc_sctp_max_packet_size().unwrap_or(1191),
        rtc_ice_policy = %config.rtc_ice_transport_policy.as_str(),
        realtime_event_game_copy = config.realtime_event_game_copy.as_str(),
        rtc_defer_sack_to_data = config.rtc_defer_sack_to_data,
        send_budget_kib_per_second = config.send_budget_kib_per_second,
        rtc_write_coalesce_micros = config.rtc_write_coalesce_micros,
        rtc_ice_servers = config.rtc_ice_urls.len(),
        signaling_auth = config.signaling_auth_token.is_some(),
        signaling_require_https = config.signaling_require_https,
        "zubspace server started"
    );
    rtc::configure_realtime_event_game_copy(config.realtime_event_game_copy);
    webrtc::set_defer_sack_to_data(config.rtc_defer_sack_to_data);
    webrtc::set_write_coalescing(std::time::Duration::from_micros(u64::from(
        config.rtc_write_coalesce_micros,
    )));
    let settings_profile = arena_settings::settings_profile_zone_for(&config.default_zone)
        .expect("validated default zone must have an arena-settings profile");
    info!(
        zone = %config.default_zone,
        settings_profile,
        "activated authored arena-settings profile"
    );

    listener.set_nonblocking(true)?;
    let admin_rx = spawn_admin_stdin_reader();
    let tick_interval = server_tick::server_tick_interval(config.tick_rate_hz);
    let mut next_tick = Instant::now() + tick_interval;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let state = Arc::clone(&state);
                thread::spawn(move || {
                    if let Err(err) = handle_client_with_started_at(stream, state, started_at) {
                        error!(error = %err, "server client handler ended");
                    }
                });
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {}
            Err(err) => return Err(err),
        }

        while let Ok(line) = admin_rx.try_recv() {
            let Some(command) = parse_admin_command(&line) else {
                continue;
            };
            println!("{}", admin_command_response(&state, &command));
            if command == AdminCommand::Shutdown {
                info!("zubspace server stopped");
                return Ok(());
            }
        }

        let now = Instant::now();
        if now >= next_tick {
            let tick = server_tick::run_server_tick(&state, now);
            for packet in tick.brick_packets {
                reliable::broadcast_reliable_to_joined(&state, packet);
            }
            for packet in tick.hidden_prize_packets {
                reliable::broadcast_reliable_to_joined(&state, packet);
            }
            // Retry after this tick's durable broadcasts so simulated-loss
            // ordering and reliable pacing remain stable across ticks.
            reliable::resend_due_reliable_to_clients(&state, now);
            // `run_server_tick` and its broadcasts can take longer than one
            // interval under tracing, browser stalls, or ordinary load. Skip
            // every deadline that elapsed through the *completed* tick. Using
            // the pre-tick `now` here left the driver behind after a slow
            // frame, so it issued rapid catch-up frames that could reuse the
            // same 100 Hz Continuum tick and double-step authoritative state.
            next_tick = next_server_tick_deadline(next_tick, tick_interval, Instant::now());
        }

        // Phase D (AOI pacing): sleep precisely until the next tick deadline
        // instead of a fixed 2 ms poll, so ticks — and therefore snapshot
        // emission — fire on an even cadence. Server-side timing jitter
        // shows up as remote-ship stutter on every client. The floor avoids
        // a 100% busy-spin when the loop is momentarily behind; the cap
        // (one interval) keeps the connection-accept / admin poll responsive.
        let until_next = next_tick.saturating_duration_since(Instant::now());
        thread::sleep(until_next.clamp(Duration::from_micros(200), tick_interval));
    }
}

fn next_server_tick_deadline(
    mut next_tick: Instant,
    tick_interval: Duration,
    completed_at: Instant,
) -> Instant {
    debug_assert!(!tick_interval.is_zero());
    while next_tick <= completed_at {
        next_tick += tick_interval;
    }
    next_tick
}

fn validate_rtc_udp_socket_availability(config: &ServerConfig) -> io::Result<()> {
    let Some(range) = config.rtc_udp_port_range else {
        return Ok(());
    };
    let mut sockets = Vec::with_capacity(range.capacity());
    for port in range.ports() {
        let bind_address = config.rtc_udp_bind_address(port);
        let socket = UdpSocket::bind(bind_address).map_err(|err| {
            io::Error::new(
                ErrorKind::AddrInUse,
                format!(
                    "RTC UDP port {port} at {bind_address} in configured range {}-{} is unavailable: {err}; free the port, choose a different range, or correct rtc_udp_bind_ip",
                    range.min, range.max,
                ),
            )
        })?;
        sockets.push(socket);
    }
    Ok(())
}

fn server_log_filter(level: ServerLogLevel, overrides: &str) -> io::Result<Targets> {
    let fallback = match level {
        ServerLogLevel::Error => LevelFilter::ERROR,
        ServerLogLevel::Warn => LevelFilter::WARN,
        ServerLogLevel::Info => LevelFilter::INFO,
        ServerLogLevel::Debug => LevelFilter::DEBUG,
        ServerLogLevel::Trace => LevelFilter::TRACE,
        ServerLogLevel::Off => LevelFilter::OFF,
    };
    let filter = if overrides.is_empty() {
        Targets::new()
    } else {
        overrides
            .parse::<Targets>()
            .map_err(|error| io::Error::new(ErrorKind::InvalidInput, error))?
    };
    Ok(filter.with_default(fallback))
}

fn server_log_dispatch(
    level: ServerLogLevel,
    overrides: &str,
    writer: tracing_subscriber::fmt::writer::BoxMakeWriter,
    json: bool,
) -> io::Result<tracing::Dispatch> {
    let filter = server_log_filter(level, overrides)?;
    let builder = tracing_subscriber::fmt()
        // The outer Targets filter decides before event formatting. A second
        // global INFO/DEBUG ceiling would incorrectly suppress explicit overrides.
        .with_max_level(LevelFilter::TRACE)
        .with_target(true)
        .with_ansi(false)
        .with_writer(writer);
    // SystemTime's default UTC RFC3339 formatter retains sub-millisecond precision.
    Ok(if json {
        tracing::Dispatch::new(builder.json().finish().with(filter))
    } else {
        tracing::Dispatch::new(builder.with_target(false).compact().finish().with(filter))
    })
}

fn init_logging(
    level: ServerLogLevel,
    overrides: &str,
    log_file: Option<&str>,
    json: bool,
) -> io::Result<()> {
    // Lines go to a writer thread so a stalled journald or log disk never
    // blocks the tick; see log_queue.
    let writer = match log_file.filter(|path| !path.trim().is_empty()) {
        Some(path) => log_queue::QueuedWriter::spawn(io::BufWriter::new(open_log_file(path)?))?,
        None => log_queue::QueuedWriter::spawn(io::stdout())?,
    };
    let writer = tracing_subscriber::fmt::writer::BoxMakeWriter::new(writer);
    server_log_dispatch(level, overrides, writer, json)?
        .try_init()
        .map_err(|error| io::Error::other(format!("failed to initialize server logging: {error}")))
}

fn open_log_file(path: &str) -> io::Result<std::fs::File> {
    let path = Path::new(path);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to create server log directory {parent:?}: {err}"),
            )
        })?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to open server log file {path:?}: {err}"),
            )
        })
}

fn spawn_admin_stdin_reader() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

#[cfg(test)]
#[path = "zubspace_server/tests.rs"]
mod tests;
