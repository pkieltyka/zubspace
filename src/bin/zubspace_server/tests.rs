use super::admin::{
    AdminCommand, admin_command_response, parse_admin_command, reset_arena_at_tick,
};
use super::arena::{
    SET_COORDINATES, join_arena_at_tick, join_arena_at_tick_with_delivery_hooks,
    join_arena_at_tick_with_objective_capture_hook, join_arena_for_login,
    join_arena_for_login_at_tick_with_zone_selection_hook, large_position_from_client,
    update_frequency_at_tick, update_frequency_instant_at_tick, update_ship_at_tick,
};
use super::arena_settings::{
    ARENA_SETTINGS, CONTINUUM_ARENA_SETTINGS_SIZE, CONTINUUM_BYTE_SETTINGS_OFFSET,
    CONTINUUM_GLOBAL_INTS_OFFSET, CONTINUUM_PRIZE_WEIGHT_COUNT, CONTINUUM_PRIZE_WEIGHTS_OFFSET,
    CONTINUUM_SHIP_SETTINGS_OFFSET, CONTINUUM_SHIP_SETTINGS_SIZE, CONTINUUM_SHORT_SETTINGS_OFFSET,
    arena_bomb_explode_delay_ticks, arena_bomb_safety_enabled, arena_max_frequency,
    arena_mine_lifetime, arena_proximity_distance, arena_settings_packet,
    arena_ship_initial_bomb_level, arena_ship_initial_rocket_count, arena_ship_max_bomb_level,
    arena_ship_max_mines, arena_ship_max_rocket_count,
    arena_ship_max_thrust_pixels_per_second_squared, arena_ship_radius_pixels,
    arena_ship_rocket_lifetime_ticks, arena_ship_shrapnel_max, arena_ship_shrapnel_rate,
    arena_ship_weapon_energy_cost, arena_ship_weapon_fire_delay_ticks, arena_soccer_mode,
    arena_team_max_mines, arena_weapon_damage_settings, arena_weapon_max_origin_distance_pixels,
    continuum_settings_checksum,
};
use super::authority::{
    POSITION_STATUS_ANTIWARP, POSITION_STATUS_CLOAK, POSITION_STATUS_FLASH, POSITION_STATUS_SAFETY,
    POSITION_STATUS_STEALTH, POSITION_STATUS_XRADAR,
};
use super::authority_damage::maximum_reported_weapon_damage;
use super::continuum_position::{
    reported_position_hull_overlaps_blocked_tile, reported_position_path_crosses_blocked_tile,
    reported_position_path_hull_overlaps_blocked_tile,
};
use super::identity::{LOGIN_ID_HIGH_OFFSET, LOGIN_ID_LOW_OFFSET};
use super::objectives::{
    BRICK_DROPPED, FlagDropBehavior, FlagDropTile, FlagPolicy, FlagProfileSettings,
    FlagRewardSplit, FlagState, FlagWinMode, ObjectiveAward, ObjectiveLifecycleReason,
    ObjectiveMutation, ObjectiveMutationError, ObjectiveParticipant, ObjectivePlayer,
    ObjectiveRegistry, ObjectiveRegistryError, ObjectiveSpawnArea, ObjectiveTeamLimits,
    POWERBALL_POSITION, PowerballMotion, PowerballPolicy, PowerballProfileSettings, PowerballState,
    ServerFlagProfile, ServerPowerballProfile, apply_player_objective_lifecycle,
    authenticate_objective_action, ball_spawn_wire_position, brick_tile_from_ship_position,
    claim_flag, clear_bricks_packet, create_turret_link, current_flag_packets,
    current_turret_packets, destroy_turret_link, drop_flag_packet, evaluate_flag_win,
    expire_bricks, fire_powerball, flag_claim_packet, flag_drop_tile_near, flag_position_packet,
    flag_spawn_tile, initialize_objectives, legacy_flag_drop_packets, mutation_packets,
    objective_state_replay_packets, pickup_powerball, player_carries_powerball,
    powerball_removal_packet, relay_drop_brick_at_tick, score_powerball_goal,
    simulate_powerball_step, soccer_goal_counts_world, step_objective_deadlines,
    step_powerball_motion, synthetic_profile,
};
use super::player_events::{
    CHAT_MESSAGE_REQUEST, CHAT_PACKET, CHAT_TYPE_PRIVATE, CHAT_TYPE_PUBLIC, CHAT_TYPE_TEAM,
    DEATH_REQUEST, PLAYER_DEATH, TAKE_PRIZE_REQUEST, TOGGLE_SEND_DAMAGE, chat_packet_from_request,
    player_prize_from_claim, prize_pickup_ack_from_claim, relay_prize_pickup_at_tick,
    spectate_packet,
};
use super::reliable::{
    RELIABLE_ACK, RELIABLE_MESSAGE, SMALL_CHUNK_BODY, SMALL_CHUNK_TAIL, reliable, reliable_parts,
    reliable_payload,
};
use super::roster::{
    ARENA_DIRECTORY, PLAYER_BANNER_CHANGE, SHIP_FREQUENCY_CHANGE, arena_directory_listing,
    personal_banner_change, player_banner_change, score_update,
};
use super::security::{SECURITY_CHALLENGE, SECURITY_CHECKSUM_KEY, security_challenge};
use super::session::{FILE_REQUEST, LOGIN_FAILURE_MESSAGE, NEWS_REQUEST, PASSWORD_LOGIN_CONTINUUM};
use super::state::{
    ClientSecurityReport, PLAYER_BANNER_BYTES, PlayerStats, PlayerSummary, ServerBrick,
    ServerHiddenPrize, ServerPrizeClaim, ServerPrizeReceipt, ServerProjectile,
};
use super::wire::continuum_file_crc32;
use super::zone::{
    COMPRESSED_MAP, COMPRESSED_MAP_HEADER_SIZE, GOAL_TILE_ID, MAP_INFORMATION, MAP_TILES,
    SAFE_TILE_ID, SERVER_FILE_TRANSFER, SERVER_LVZ_FILES, SERVER_MAP_BYTES, SERVER_MAP_FILENAME,
    SERVER_NEWS_BYTES, SERVER_NEWS_FILENAME, TILE_SIZE_PX, compressed_map,
    large_position_packet_in_safe_tile, map_information, server_file_transfer,
    server_spawn_coordinates, solid_tile, tile_at_world, world_position_in_safe_tile,
    world_position_touches_goal, zone_file_download, zone_map,
};
use super::zone::{ZoneFlag, ZoneMap, zone_map_for_id};
use super::zone::{position_blocking_tile, position_blocking_tile_for_door_seeds};
use super::*;
use flate2::read::ZlibDecoder;
use std::cell::Cell;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

#[path = "tests/admin.rs"]
mod admin;
#[path = "tests/admission.rs"]
mod admission;
#[path = "tests/admitted_motion_load.rs"]
mod admitted_motion_load;
#[path = "tests/arena_settings.rs"]
mod arena_settings;
#[path = "tests/attachments.rs"]
mod attachments;
#[path = "tests/bricks.rs"]
mod bricks;
#[path = "tests/config_identity.rs"]
mod config_identity;
#[path = "tests/connection.rs"]
mod connection;
#[path = "tests/deaths.rs"]
mod deaths;
#[path = "tests/flags.rs"]
mod flags;
#[path = "tests/free_for_all.rs"]
mod free_for_all;
#[path = "tests/lifecycle.rs"]
mod lifecycle;
#[path = "tests/logging.rs"]
mod logging;
#[path = "tests/objectives_engine.rs"]
mod objectives_engine;
#[path = "tests/objectives_foundation.rs"]
mod objectives_foundation;
#[path = "tests/packet_helpers.rs"]
mod packet_helpers;
#[path = "tests/payload_helpers.rs"]
mod payload_helpers;
#[path = "tests/player_events.rs"]
mod player_events;
#[path = "tests/portal_placements.rs"]
mod portal_placements;
#[path = "tests/portal_warps.rs"]
mod portal_warps;
#[path = "tests/positions.rs"]
mod positions;
#[path = "tests/powerballs.rs"]
mod powerballs;
#[path = "tests/prizes.rs"]
mod prizes;
#[path = "tests/roster.rs"]
mod roster;
#[path = "tests/scale_bench.rs"]
mod scale_bench;
#[path = "tests/snapshot.rs"]
mod snapshot;
#[path = "tests/zone_files.rs"]
mod zone_files;

use packet_helpers::*;
use payload_helpers::*;

thread_local! {
    /// Position witnesses are per connection. Keeping the generic fixture
    /// sequence local to each test worker avoids unrelated parallel tests
    /// manufacturing impossible >32-record gaps for the same fixture player.
    static NEXT_TEST_CLIENT_POSITION_SEQUENCE: Cell<u16> = const { Cell::new(1) };
}

// Deterministic spatial setup for native queue/collision contracts. This
// supplies ordinary authored settings to the production bounded spawn search;
// it is not available in the game server or any browser E2E profile. Tests of
// the shipped random FFA spawn policy deliberately do not use this fixture.
fn use_fixed_authored_spawn_fixture(state: &mut ServerState) {
    state.config.free_for_all = false;
    let mut settings = state.zone_map().settings_packet.to_vec();
    let offset = crate::arena_settings::CONTINUUM_GLOBAL_INTS_OFFSET + 80;
    for index in 0..4 {
        settings[offset + index * 4..offset + index * 4 + 4]
            .copy_from_slice(&(512_u32 | (512_u32 << 10)).to_le_bytes());
    }
    state.fixture_spawn_settings_packet = Some(settings);
}

fn client_position_packet(_timestamp: u32, x: u16, y: u16) -> Vec<u8> {
    client_position_packet_with_weapon(0, x, y, 0)
}

fn ship_change_request(ship: u8) -> [u8; 4] {
    let sequence_base = NEXT_TEST_CLIENT_POSITION_SEQUENCE.with(|next| next.get());
    [
        SHIP_CHANGE_REQUEST,
        ship,
        sequence_base.to_le_bytes()[0],
        sequence_base.to_le_bytes()[1],
    ]
}

fn frequency_baseline_packet(
    snapshot: crate::continuum_position::ClientPositionSnapshot,
    sequence: u16,
) -> Vec<u8> {
    let mut packet = vec![0; crate::continuum_position::CLIENT_POSITION_EXTRA_LEN];
    packet[0] = CLIENT_POSITION;
    packet[1] = snapshot.direction;
    packet[2..6].copy_from_slice(&0_u32.to_le_bytes());
    packet[6..8].copy_from_slice(&snapshot.velocity_x.to_le_bytes());
    packet[8..10].copy_from_slice(&snapshot.y.to_le_bytes());
    packet[11] = snapshot.togglables;
    packet[12..14].copy_from_slice(&snapshot.x.to_le_bytes());
    packet[14..16].copy_from_slice(&snapshot.velocity_y.to_le_bytes());
    packet[16..18].copy_from_slice(&snapshot.bounty.to_le_bytes());
    packet[18..20].copy_from_slice(&snapshot.energy.to_le_bytes());
    packet[20..22].copy_from_slice(&0_u16.to_le_bytes());
    packet[28..32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(sequence).to_le_bytes(),
    );
    snapshot.canonical_state.encode(&mut packet);
    packet.extend_from_slice(&[
        crate::connection::CLIENT_INPUT_RECORD_TYPE,
        sequence.to_le_bytes()[0],
        sequence.to_le_bytes()[1],
        1,
        0,
        0,
        snapshot.direction,
        0,
    ]);
    packet.extend_from_slice(
        &crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence).encode(),
    );
    packet
}

fn unavailable_combat_view_receipt(sequence: u16) -> [u8; 14] {
    crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence).encode()
}

fn consume_pending_lifecycle_baseline(state: &Arc<Mutex<ServerState>>, player_id: u16) {
    consume_pending_lifecycle_baseline_at_tick(state, player_id, crate::wire::current_tick());
}

fn consume_pending_lifecycle_baseline_at_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    server_tick: u32,
) {
    let pending = {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        client
            .life_position_sequence_base
            .filter(|_| client.last_position_sequence.is_none())
            .map(|sequence| {
                let snapshot = client.pending_frequency_baseline.unwrap_or_else(|| {
                    let (x, y) = server_spawn_coordinates(state.zone_map());
                    crate::continuum_position::ClientPositionSnapshot {
                        direction: 0,
                        timestamp: 0,
                        client_sequence: Some(sequence),
                        x,
                        y,
                        velocity_x: 0,
                        velocity_y: 0,
                        togglables: 0,
                        bounty: client.server_bounty,
                        energy: client.server_energy,
                        weapon: 0,
                        canonical_state:
                            crate::continuum_position::ClientPositionCanonicalState::from_f32(
                                x as f32, y as f32, 0.0, 0.0, 0.0,
                            ),
                        repel_snapshot_id: None,
                        repel_application_sequence: None,
                        repel_delivery_generation: None,
                        warp_collectible_id: None,
                    }
                });
                (
                    state.active_player_runtime_identity(player_id).unwrap(),
                    sequence,
                    snapshot,
                    client.pending_frequency_baseline.is_some(),
                )
            })
    };
    if let Some((identity, sequence, snapshot, frequency_baseline)) = pending {
        let door_seed = {
            let state = state.lock().expect("state");
            crate::doors::client_door_simulation_seed_at_tick(
                state.clients[&player_id].door_state,
                server_tick,
            )
            .unwrap_or_default()
        };
        let mut packet = frequency_baseline_packet(snapshot, sequence);
        packet[crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 7] = door_seed;
        let input = &packet[crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET..];
        let recorded = if frequency_baseline {
            crate::connection::record_client_frequency_input_baseline_at_tick(
                state,
                identity,
                input,
                server_tick,
            )
        } else {
            crate::connection::record_client_lifecycle_input_baseline_at_tick(
                state,
                identity,
                input,
                server_tick,
            )
        };
        assert!(recorded);
        let relay = crate::arena::accept_position_relay_for_life_at_tick(
            state,
            identity.life,
            &packet,
            server_tick,
        );
        assert!(
            relay.is_some() || {
                state.lock().expect("state").clients[&player_id].last_position_sequence
                    == Some(sequence)
            }
        );
        NEXT_TEST_CLIENT_POSITION_SEQUENCE.with(|next| {
            if next.get() == sequence {
                next.set(sequence.wrapping_add(1));
            }
        });
    }
}

/// Build the production frequency-cutover request used by tests. If a prior
/// ship/frequency lifecycle baseline is still outstanding, consume that exact
/// boundary first, matching the browser's paused reliable cutover sequence.
fn frequency_change_request(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    frequency: u16,
) -> Vec<u8> {
    consume_pending_lifecycle_baseline(state, player_id);

    let (sequence, snapshot) = {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        let sequence = client.last_input_seq.map_or_else(
            || NEXT_TEST_CLIENT_POSITION_SEQUENCE.with(|next| next.get()),
            |last| last.wrapping_add(1),
        );
        let snapshot = client
            .last_position
            .as_deref()
            .and_then(|position| {
                let witness = client.canonical_movement_witness?;
                crate::arena::frequency_baseline_from_large_position(
                    position,
                    sequence,
                    crate::continuum_position::ClientPositionCanonicalState::from_f32(
                        witness.state.position_x,
                        witness.state.position_y,
                        witness.state.velocity_x,
                        witness.state.velocity_y,
                        witness.state.orientation,
                    ),
                )
            })
            .unwrap_or_else(|| {
                let (x, y) = server_spawn_coordinates(state.zone_map());
                crate::continuum_position::ClientPositionSnapshot {
                    // A first frequency request is itself a lifecycle
                    // boundary. Current-protocol authority therefore begins
                    // from the same server-known neutral orientation as join,
                    // never from a reconstructed legacy direction bin.
                    direction: 0,
                    timestamp: 0,
                    client_sequence: Some(sequence),
                    x,
                    y,
                    velocity_x: 0,
                    velocity_y: 0,
                    togglables: 0,
                    bounty: client.server_bounty,
                    energy: client.server_energy,
                    weapon: 0,
                    canonical_state:
                        crate::continuum_position::ClientPositionCanonicalState::from_f32(
                            x as f32, y as f32, 0.0, 0.0, 0.0,
                        ),
                    repel_snapshot_id: None,
                    repel_application_sequence: None,
                    repel_delivery_generation: None,
                    warp_collectible_id: None,
                }
            });
        (sequence, snapshot)
    };
    // Unit-test arena joins intentionally do not reserve a wall-clock-derived
    // sequence. Mark a first synthetic frequency request as its exact
    // server-captured cutover boundary; production receives the same state
    // from the browser only after its join baseline has been admitted.
    {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("client");
        if client.last_position_sequence.is_none() && client.life_position_sequence_base.is_none() {
            client.pending_frequency_baseline = Some(snapshot);
        }
    }
    NEXT_TEST_CLIENT_POSITION_SEQUENCE.with(|next| next.set(sequence.wrapping_add(1)));
    let door_seed = {
        let state = state.lock().expect("state");
        crate::doors::client_door_simulation_seed_at_tick(
            state.clients[&player_id].door_state,
            crate::wire::current_tick(),
        )
        .unwrap_or_default()
    };
    let mut baseline = frequency_baseline_packet(snapshot, sequence);
    baseline[crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 7] = door_seed;
    let mut request = Vec::with_capacity(5 + baseline.len());
    request.push(FREQUENCY_CHANGE_REQUEST);
    request.extend_from_slice(&frequency.to_le_bytes());
    request.extend_from_slice(&sequence.to_le_bytes());
    request.extend_from_slice(&baseline);
    request
}

fn client_position_packet_with_sequence(_timestamp: u32, x: u16, y: u16, sequence: u16) -> Vec<u8> {
    let mut packet = client_position_packet(0, x, y);
    packet[28..32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(sequence).to_le_bytes(),
    );
    packet
}

fn append_warp_collectible_marker(mut packet: Vec<u8>, collectible_id: u32) -> Vec<u8> {
    packet.push(crate::continuum_position::CLIENT_POSITION_WARP_MARKER_TYPE);
    packet.extend_from_slice(&collectible_id.to_le_bytes());
    packet
}

/// Rebuild the current-protocol exact-state block after a legacy fixture has
/// deliberately edited the leading Continuum fields. Production code never
/// infers this state; real clients serialize their live f32 bits directly.
fn sync_client_position_canonical_state_from_wire(packet: &mut [u8]) {
    let state = crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
        packet[1],
        u16::from_le_bytes(packet[12..14].try_into().expect("position x")),
        u16::from_le_bytes(packet[8..10].try_into().expect("position y")),
        i16::from_le_bytes(packet[6..8].try_into().expect("velocity x")),
        i16::from_le_bytes(packet[14..16].try_into().expect("velocity y")),
    );
    let mut encoded =
        Vec::with_capacity(crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_LEN);
    state.encode(&mut encoded);
    packet[crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_OFFSET
        ..crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET]
        .copy_from_slice(&encoded);
}

fn set_client_position_canonical_state(
    packet: &mut [u8],
    position_x: f32,
    position_y: f32,
    velocity_x: f32,
    velocity_y: f32,
    orientation: f32,
) {
    let state = crate::continuum_position::ClientPositionCanonicalState::from_f32(
        position_x,
        position_y,
        velocity_x,
        velocity_y,
        orientation,
    );
    let mut encoded =
        Vec::with_capacity(crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_LEN);
    state.encode(&mut encoded);
    packet[crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_OFFSET
        ..crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET]
        .copy_from_slice(&encoded);
}

fn client_position_packet_with_weapon(
    _timestamp: u32,
    x: u16,
    y: u16,
    weapon_data: u16,
) -> Vec<u8> {
    let mut packet = vec![0; 22];
    packet[0] = CLIENT_POSITION;
    packet[1] = 0;
    packet[2..6].copy_from_slice(&0_u32.to_le_bytes());
    packet[8..10].copy_from_slice(&y.to_le_bytes());
    packet[12..14].copy_from_slice(&x.to_le_bytes());
    packet[18..20].copy_from_slice(&900_u16.to_le_bytes());
    packet[20..22].copy_from_slice(&weapon_data.to_le_bytes());
    packet.resize(crate::continuum_position::CLIENT_POSITION_EXTRA_LEN, 0);
    let sequence = NEXT_TEST_CLIENT_POSITION_SEQUENCE.with(|next| {
        let sequence = next.get();
        next.set(sequence.wrapping_add(1));
        sequence
    });
    packet[28..32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(sequence).to_le_bytes(),
    );
    crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(0, x, y, 0, 0)
        .encode(&mut packet);
    packet
}

fn is_reliable_ack(packet: &[u8]) -> bool {
    packet.len() == 6 && packet[0] == CORE_BYTE && packet[1] == RELIABLE_ACK
}

fn drain_available(rx: &mpsc::Receiver<Vec<u8>>) {
    while rx.try_recv().is_ok() {}
}

/// Complete the reliable join fence in tests that are concerned with later
/// snapshot content rather than bootstrap transport. Focused lifecycle tests
/// exercise partial/lost/reordered ACKs explicitly.
fn acknowledge_join_bootstrap(state: &Arc<Mutex<ServerState>>, player_id: u16) {
    let (identity, ids) = {
        let state = state.lock().expect("server state should not be poisoned");
        let identity = state
            .active_player_session_identity(player_id)
            .expect("joined test client should have a session");
        let ids = state.clients[&player_id]
            .bootstrap_reliable_barrier
            .as_ref()
            .map(|barrier| barrier.pending_ids())
            .unwrap_or_default();
        (identity, ids)
    };
    for id in ids {
        assert!(crate::reliable::receive_reliable_acks(
            state,
            identity,
            &reliable_ack(id),
        ));
    }
}

fn reset_server_runtime_without_initial_bounty(state: &Arc<Mutex<ServerState>>, player_id: u16) {
    let mut state = state.lock().expect("server state should not be poisoned");
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("player should exist");
    client.server_bounty = 0;
    client.reset_server_loadout_for_ship();
}

fn read_reliable_payload_from_channel(rx: &mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    let mut small_chunks = Vec::new();
    loop {
        let packet = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("expected server packet");
        let payload = reliable_payload(&packet).expect("expected reliable payload");
        if payload.len() >= 2 && payload[0] == CORE_BYTE && payload[1] == SMALL_CHUNK_BODY {
            small_chunks.extend_from_slice(&payload[2..]);
            continue;
        }
        if payload.len() >= 2 && payload[0] == CORE_BYTE && payload[1] == SMALL_CHUNK_TAIL {
            small_chunks.extend_from_slice(&payload[2..]);
            return small_chunks;
        }
        return payload.to_vec();
    }
}

fn assert_server_clock_bootstrap(packet: &[u8], player_id: u16) {
    let bootstrap = crate::clock_bootstrap::ServerClockBootstrap::parse(packet)
        .expect("bootstrap should parse");
    assert_ne!(bootstrap.server_session_id, 0);
    assert_eq!(bootstrap.tick_rate_hz, ServerConfig::default().tick_rate_hz);
    assert_eq!(bootstrap.snapshot_rate_hz, crate::snapshot::SNAPSHOT_HZ);
    assert_eq!(
        bootstrap.input_sequence_base,
        bootstrap.position_sequence_base
    );
    assert_eq!(
        bootstrap.input_sequence_base,
        bootstrap.weapon_sequence_base
    );
    assert_eq!(
        bootstrap.input_sequence_base,
        (bootstrap.server_tick as u16)
            .wrapping_mul(31)
            .wrapping_add(player_id)
    );
}

fn read_reliable_payload_with_type(rx: &mpsc::Receiver<Vec<u8>>, packet_type: u8) -> Vec<u8> {
    loop {
        let payload = read_reliable_payload_from_channel(rx);
        if payload.first().copied() == Some(packet_type) {
            return payload;
        }
    }
}

fn read_non_reliable_from_channel(rx: &mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    loop {
        let packet = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("expected server packet");
        if reliable_payload(&packet).is_none()
            && packet.first().copied() != Some(crate::realtime_event::REALTIME_EVENT_PACKET)
        {
            return packet;
        }
    }
}

fn assert_map_information(payload: &[u8]) {
    let map = zone_map();
    assert_map_information_for_map(payload, map);
}

fn assert_map_information_for_map(payload: &[u8], map: &ZoneMap) {
    assert_eq!(payload[0], MAP_INFORMATION);
    assert_eq!(
        payload.len(),
        1 + crate::zone_wire::AUTHENTICATED_MAP_INFO_HEADER_BYTES
            + map.lvz_files.len() * crate::zone_wire::AUTHENTICATED_MAP_INFO_FILE_BYTES
    );
    assert_eq!(&payload[1..5], b"ZMAP");
    assert_eq!(read_fixed_test_string(&payload[6..22]), map.zone_id);
    assert_eq!(read_fixed_test_string(&payload[22..38]), map.filename);
    assert_eq!(
        u32::from_le_bytes(payload[38..42].try_into().unwrap()),
        map.checksum
    );
    assert_eq!(
        u32::from_le_bytes(payload[42..46].try_into().unwrap()),
        map.compressed.len() as u32
    );
    assert_eq!(&payload[46..78], &map.sha256);
    for (index, lvz) in map.lvz_files.iter().enumerate() {
        let offset = 80 + index * crate::zone_wire::AUTHENTICATED_MAP_INFO_FILE_BYTES;
        assert_eq!(
            read_fixed_test_string(&payload[offset..offset + 16]),
            lvz.filename
        );
        assert_eq!(
            u32::from_le_bytes(payload[offset + 16..offset + 20].try_into().unwrap()),
            lvz.checksum
        );
        assert_eq!(
            u32::from_le_bytes(payload[offset + 20..offset + 24].try_into().unwrap()),
            lvz.bytes.len() as u32
        );
        assert_eq!(&payload[offset + 24..offset + 56], &lvz.sha256);
    }
}

fn assert_arena_directory(payload: &[u8], population: u16) {
    assert_arena_directory_entries(
        payload,
        &[
            ("Chaos Zone".to_string(), population),
            ("Extreme".to_string(), 0),
        ],
    );
}

fn assert_arena_directory_name(payload: &[u8], name: &str, population: u16) {
    let expected = match name {
        "Chaos Zone" => vec![
            ("Chaos Zone".to_string(), population),
            ("Extreme".to_string(), 0),
        ],
        "Extreme" => vec![
            ("Extreme".to_string(), population),
            ("Chaos Zone".to_string(), 0),
        ],
        _ => vec![(name.to_string(), population)],
    };
    assert_arena_directory_entries(payload, &expected);
}

fn assert_arena_directory_entries(payload: &[u8], expected: &[(String, u16)]) {
    assert_eq!(payload[0], ARENA_DIRECTORY);
    assert_eq!(decode_arena_directory_entries(payload), expected);
}

fn decode_arena_directory_entries(payload: &[u8]) -> Vec<(String, u16)> {
    let mut entries = Vec::new();
    let mut offset = 1;
    while offset < payload.len() {
        let relative_end = payload[offset..]
            .iter()
            .position(|&byte| byte == 0)
            .expect("arena directory entry should be null-terminated");
        let name_end = offset + relative_end;
        assert!(
            name_end + 3 <= payload.len(),
            "arena directory entry should include population"
        );
        let name = String::from_utf8(payload[offset..name_end].to_vec())
            .expect("arena directory name should be UTF-8");
        let population =
            u16::from_le_bytes(payload[name_end + 1..name_end + 3].try_into().unwrap());
        entries.push((name, population));
        offset = name_end + 3;
    }
    entries
}

fn assert_score_update(payload: &[u8], player_id: u16, wins: u16, losses: u16) {
    assert_score_update_with_assists(payload, player_id, 0, wins, losses);
}

fn assert_score_update_with_assists(
    payload: &[u8],
    player_id: u16,
    assists: u32,
    wins: u16,
    losses: u16,
) {
    // Preserve the original identity/count/assist assertions. Combat points
    // have their own exact bounty-accounting regressions, separate from assists.
    assert_eq!(payload.len(), 20);
    assert_eq!(payload[0], 0x09);
    assert_eq!(
        u16::from_le_bytes(payload[1..3].try_into().unwrap()),
        player_id
    );
    assert_eq!(u32::from_le_bytes(payload[7..11].try_into().unwrap()), 0);
    assert_eq!(
        u16::from_le_bytes(payload[11..13].try_into().unwrap()),
        wins
    );
    assert_eq!(
        u16::from_le_bytes(payload[13..15].try_into().unwrap()),
        losses
    );
    assert_eq!(payload[15], 1);
    assert_eq!(
        u32::from_le_bytes(payload[16..20].try_into().unwrap()),
        assists
    );
}

fn assert_initial_flag_packets(client: &TestPacketClient) {
    assert_initial_flag_packets_for_map(client, zone_map());
}

/// Assert that `actual` matches `expected` for a PLAYER_DEATH packet,
/// ignoring byte 1 (the rolled green_id). Byte 1 must be either -1 (no
/// green) or a positive prize id 1..=28.
#[track_caller]
pub(super) fn assert_death_packet_eq_ignoring_green(actual: &[u8], expected: &[u8]) {
    assert_eq!(actual.len(), 22, "death packet length mismatch");
    assert_eq!(
        expected.len(),
        10,
        "legacy expected fixture length mismatch"
    );
    assert_eq!(actual[0], expected[0], "death packet type byte mismatch");
    let green = actual[1] as i8;
    assert!(
        green == -1 || (1..=28).contains(&(green as i16)),
        "death packet green_id out of range: got {}",
        green
    );
    assert_eq!(
        &actual[2..10],
        &expected[2..],
        "death packet body (post-green) mismatch"
    );
    let collectible_id = u32::from_le_bytes(actual[10..14].try_into().unwrap());
    assert_eq!(collectible_id != 0, green > 0);
}

fn assert_initial_flag_packets_for_map(_client: &TestPacketClient, map: &ZoneMap) {
    assert!(
        map.objective_profile.is_disabled(),
        "this fixture must not assume an enabled objective profile"
    );
}

fn assert_arena_settings(payload: &[u8]) {
    assert_arena_settings_for_zone(payload, crate::zone::CHAOS_ZONE_ID);
}

fn assert_arena_settings_for_zone(payload: &[u8], zone_id: &str) {
    assert_eq!(
        payload,
        crate::arena_settings::arena_settings_packet_for_zone(zone_id)
            .expect("test zone must have an arena-settings profile")
    );
}

fn assert_set_coordinates_for_map(payload: &[u8], map: &ZoneMap) {
    assert_eq!(payload[0], SET_COORDINATES);
    assert_eq!(payload.len(), 5);
    let x = u16::from_le_bytes(payload[1..3].try_into().unwrap());
    let y = u16::from_le_bytes(payload[3..5].try_into().unwrap());
    let max_world = (MAP_TILES as f32 * TILE_SIZE_PX) as u16;
    assert!(x < max_world);
    assert!(y < max_world);
    assert!(!solid_tile(tile_at_world(map, x, y)));
}

fn assert_security_challenge(payload: &[u8]) {
    assert_eq!(payload[0], SECURITY_CHALLENGE);
    assert_eq!(
        u32::from_le_bytes(payload[13..17].try_into().unwrap()),
        SECURITY_CHECKSUM_KEY
    );
}

fn decompress_map_payload(packet: &[u8]) -> Vec<u8> {
    let mut decoder = ZlibDecoder::new(download_payload(packet));
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();
    decoded
}

fn download_payload(packet: &[u8]) -> &[u8] {
    assert_eq!(packet[0], COMPRESSED_MAP);
    &packet[COMPRESSED_MAP_HEADER_SIZE..]
}

fn read_fixed_test_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

fn read_u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_i16_at(bytes: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_i32_at(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn security_response_packet(
    settings_checksum: u32,
    map_checksum: u32,
    continuum_map_crc32: Option<u32>,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(if continuum_map_crc32.is_some() {
        46
    } else {
        40
    });
    packet.push(SECURITY_RESPONSE);
    packet.extend(0_u32.to_le_bytes());
    packet.extend(settings_checksum.to_le_bytes());
    packet.extend(0_u32.to_le_bytes());
    packet.extend(map_checksum.to_le_bytes());
    packet.extend(0_u32.to_le_bytes());
    packet.extend(0_u32.to_le_bytes());
    packet.extend(0_u16.to_le_bytes());
    packet.extend(0_u16.to_le_bytes());
    packet.extend(0_u16.to_le_bytes());
    packet.extend(0_u16.to_le_bytes());
    packet.extend(0_u16.to_le_bytes());
    packet.extend(0_u16.to_le_bytes());
    packet.extend(0_u16.to_le_bytes());
    packet.push(0);
    if let Some(checksum) = continuum_map_crc32 {
        packet.extend(0_u16.to_le_bytes());
        packet.extend(checksum.to_le_bytes());
    }
    packet
}
#[test]
fn server_tick_deadline_skips_every_interval_elapsed_during_work() {
    let started_at = std::time::Instant::now();
    let interval = std::time::Duration::from_millis(10);
    let first_deadline = started_at + interval;

    assert_eq!(
        super::next_server_tick_deadline(
            first_deadline,
            interval,
            started_at + std::time::Duration::from_millis(35),
        ),
        started_at + std::time::Duration::from_millis(40),
    );
    assert_eq!(
        super::next_server_tick_deadline(first_deadline, interval, started_at),
        first_deadline,
    );
}

// Preserve the historical team/lifecycle protocol contracts as explicit team-mode
// fixtures. Shipped defaults and the separate FFA regressions never opt out.
fn team_mode_state() -> ServerState {
    ServerState::new(ServerConfig {
        free_for_all: false,
        ..ServerConfig::default()
    })
}

// Historical geometry captures were recorded with the old 500-roll loadout.
// Keep that exact fixture explicit while shipped spawns now roll only 30 prizes.
fn grant_legacy_capture_loadout(client: &mut crate::state::ClientHandle, tick: u32) {
    client.reset_server_loadout_for_ship_at_tick(tick);
    for prize in crate::authority_prizes::initial_bounty_prize_ids(
        &crate::arena_settings::arena_prize_weights(),
        crate::arena_settings::arena_prize_weight_total(),
        500,
        1,
    ) {
        client.apply_server_prize_effect_at_tick(prize, tick);
    }
    client.server_bounty = 500;
}
