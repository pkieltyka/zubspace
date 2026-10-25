use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::CORE_BYTE;
use super::arena_settings::{arena_settings_packet_for_zone, continuum_settings_checksum};
use super::doors::ClientDoorState;
use super::state::{ClientSecurityReport, ServerState};
use super::wire::current_tick;
use crate::authority_prizes::SECURITY_PRIZE_SEED;
use tracing::{info, warn};

pub(crate) const SECURITY_RESPONSE: u8 = 0x1a;
pub(crate) const SECURITY_CHALLENGE: u8 = 0x18;
pub(crate) const SECURITY_CHECKSUM_KEY: u32 = 0x3132_3334;
const TIME_SYNC_REQUEST: u8 = 0x05;
const TIME_SYNC_RESPONSE: u8 = 0x06;

/// Phase 4 anti-cheat instrumentation: aggregate (category, reason)
/// reject counts so a noisy attacker doesn't drown the per-event log
/// and operators can graph or alert on totals over time. Reasons are
/// `&'static str` literals everywhere in the tree (most come from
/// authoritative position, input, and weapon validation), so the key
/// type is `(&'static str, &'static str)` with no per-call alloc.
fn security_reject_counts() -> &'static Mutex<HashMap<(&'static str, &'static str), u64>> {
    static COUNTS: OnceLock<Mutex<HashMap<(&'static str, &'static str), u64>>> = OnceLock::new();
    COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}

// Per-test mirror of the reject counters.
// `security_reject_counts` is process-global and `cargo test` runs suites concurrently,
// so both halves of the usual pattern - "read a production reason's count, drive one
// packet through the handler, assert it rose by exactly one" and its twin "assert this
// packet caused no rejection at all" - race every other test rejecting for the same
// reason. `security.rs`'s own counter test escapes that by inventing a reason of its
// own, which an authority test cannot do because the reason under test is the point.
// This mirror is keyed by the current thread name, which libtest sets to the test path,
// so a bucket belongs to a test rather than to a worker thread; a plain thread-local
// would hand one test another test's counts, because a worker runs many tests over a
// suite. Rejects are logged on the thread that handled the packet, and these tests call
// the handlers inline. Serial runs and non-test threads have no per-test name to key on,
// so they fall back to the aggregate map, which is race-free when nothing runs in
// parallel. Production keeps only the aggregate map.
#[cfg(test)]
fn test_security_reject_counts()
-> &'static Mutex<HashMap<(String, &'static str, &'static str), u64>> {
    static COUNTS: OnceLock<Mutex<HashMap<(String, &'static str, &'static str), u64>>> =
        OnceLock::new();
    COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The running test's bucket key, or `None` when the current thread is not a libtest
/// worker with the test's name.
#[cfg(test)]
fn test_security_reject_scope() -> Option<String> {
    std::thread::current()
        .name()
        .filter(|name| !name.is_empty() && *name != "main")
        .map(str::to_string)
}

#[cfg(test)]
pub(crate) fn peek_security_reject_count_for_test(
    category: &'static str,
    reason: &'static str,
) -> u64 {
    match test_security_reject_scope() {
        Some(scope) => test_security_reject_counts()
            .lock()
            .map(|counts| counts.get(&(scope, category, reason)).copied().unwrap_or(0))
            .unwrap_or(0),
        None => security_reject_counts()
            .lock()
            .map(|counts| counts.get(&(category, reason)).copied().unwrap_or(0))
            .unwrap_or(0),
    }
}

pub(crate) fn log_security_reject(player_id: u16, category: &'static str, reason: &'static str) {
    warn!(player_id, category, reason, "server security reject");
    if let Ok(mut counts) = security_reject_counts().lock() {
        *counts.entry((category, reason)).or_insert(0) += 1;
    }
    #[cfg(test)]
    if let Some(scope) = test_security_reject_scope()
        && let Ok(mut counts) = test_security_reject_counts().lock()
    {
        *counts.entry((scope, category, reason)).or_insert(0) += 1;
    }
}
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn security_reject_log_line(player_id: u16, category: &str, reason: &str) -> String {
    format!("server security reject id={player_id} category={category} reason={reason}")
}

/// Drain the (category, reason) counter map and emit one aggregated
/// summary log line. Returns the number of distinct buckets emitted
/// so callers can decide whether to skip the log when the window was
/// quiet. The map starts fresh after this call.
pub(crate) fn dump_security_reject_summary() -> usize {
    let drained: Vec<((&'static str, &'static str), u64)> = match security_reject_counts().lock() {
        Ok(mut counts) => counts.drain().collect(),
        Err(_) => return 0,
    };
    summarize_drained_rejects(drained)
}

/// Aggregate already-drained buckets into one summary line and log it.
/// Returns the number of distinct buckets, and logs nothing for a quiet
/// window. Kept separate from the process-global map so the aggregation
/// and quiet-window contract stays testable while other tests are
/// concurrently logging into (and asserting deltas on) that map.
fn summarize_drained_rejects(mut buckets: Vec<((&'static str, &'static str), u64)>) -> usize {
    if buckets.is_empty() {
        return 0;
    }
    // Sort for stable output: category, then reason.
    buckets.sort_by_key(|((cat, reason), _)| (*cat, *reason));
    let mut line = String::from("server security reject summary");
    let mut total: u64 = 0;
    for ((cat, reason), count) in &buckets {
        total += count;
        line.push_str(&format!(" {cat}/{reason}={count}"));
    }
    line.push_str(&format!(" total={total}"));
    info!(total, summary = %line, "server security reject summary");
    buckets.len()
}

#[cfg(test)]
pub(crate) fn peek_security_reject_count(category: &'static str, reason: &'static str) -> u64 {
    security_reject_counts()
        .lock()
        .map(|counts| counts.get(&(category, reason)).copied().unwrap_or(0))
        .unwrap_or(0)
}

pub(crate) fn update_security_report(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    packet: &[u8],
) {
    let Some(report) = parse_security_report(packet) else {
        return;
    };
    let (settings_checksum, map) = {
        let state = state.lock().expect("server state should not be poisoned");
        (
            continuum_settings_checksum(
                SECURITY_CHECKSUM_KEY,
                arena_settings_packet_for_zone(&state.config.default_zone)
                    .expect("server state must name a bundled zone"),
            ),
            state.zone_map(),
        )
    };
    let valid = report.settings_checksum == settings_checksum
        && report.map_checksum == map.map_checksum
        && report
            .continuum_map_crc32
            .is_none_or(|checksum| checksum == map.continuum_crc32);
    if let Some(client) = state
        .lock()
        .expect("server state should not be poisoned")
        .clients
        .get_mut(&player_id)
    {
        client.last_security_report = Some(report);
        client.last_security_valid = Some(valid);
    }
}

pub(crate) fn is_time_sync_request(packet: &[u8]) -> bool {
    packet.len() >= 6 && packet[0] == CORE_BYTE && packet[1] == TIME_SYNC_REQUEST
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn security_challenge() -> Vec<u8> {
    security_challenge_with_door_state().0
}

pub(crate) fn security_challenge_with_door_state() -> (Vec<u8>, ClientDoorState) {
    security_challenge_for_door_state(ClientDoorState::for_challenge(current_tick()))
}

/// Encode one exact arena-owned door anchor into the standard security
/// challenge. Production sessions all receive the same anchor; generating a
/// fresh timestamp per connection would give every browser a different door
/// phase even though projectile and bot collision are arena-wide.
pub(crate) fn security_challenge_for_door_state(
    door_state: ClientDoorState,
) -> (Vec<u8>, ClientDoorState) {
    let mut payload = Vec::with_capacity(17);
    payload.push(SECURITY_CHALLENGE);
    payload.extend((SECURITY_PRIZE_SEED as u32).to_le_bytes());
    payload.extend((door_state.rng_seed as u32).to_le_bytes());
    payload.extend(door_state.timestamp.to_le_bytes());
    payload.extend(SECURITY_CHECKSUM_KEY.to_le_bytes());
    (payload, door_state)
}

fn parse_security_report(packet: &[u8]) -> Option<ClientSecurityReport> {
    if packet.len() != 40 && packet.len() != 46 {
        return None;
    }
    let continuum_map_crc32 = if packet.len() == 46 {
        Some(u32::from_le_bytes(packet[42..46].try_into().ok()?))
    } else {
        None
    };
    Some(ClientSecurityReport {
        settings_checksum: u32::from_le_bytes(packet[5..9].try_into().ok()?),
        map_checksum: u32::from_le_bytes(packet[13..17].try_into().ok()?),
        continuum_map_crc32,
    })
}

pub(crate) fn time_sync_response(
    request: &[u8],
    server_session_id: u64,
    arena_stream_id: u64,
) -> Vec<u8> {
    let token = u32::from_le_bytes(
        request
            .get(2..6)
            .and_then(|bytes| bytes.try_into().ok())
            .unwrap_or([0; 4]),
    );
    time_sync_response_for_scope(token, server_session_id, arena_stream_id)
}

pub(crate) fn time_sync_response_for_token(token: u32) -> Vec<u8> {
    let mut response = Vec::with_capacity(10);
    response.extend([CORE_BYTE, TIME_SYNC_RESPONSE]);
    response.extend(token.to_le_bytes());
    response.extend(current_tick().to_le_bytes());
    response
}

pub(crate) fn time_sync_response_for_scope(
    token: u32,
    server_session_id: u64,
    arena_stream_id: u64,
) -> Vec<u8> {
    let mut response = time_sync_response_for_token(token);
    response.extend(server_session_id.to_le_bytes());
    response.extend(arena_stream_id.to_le_bytes());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_challenge_encodes_the_supplied_arena_door_anchor() {
        let door_state = ClientDoorState::for_challenge(0x1234_5678);
        let (packet, retained) = security_challenge_for_door_state(door_state);

        assert_eq!(retained, door_state);
        assert_eq!(packet.len(), 17);
        assert_eq!(packet[0], SECURITY_CHALLENGE);
        assert_eq!(
            u32::from_le_bytes(packet[5..9].try_into().expect("door RNG bytes")),
            door_state.rng_seed as u32
        );
        assert_eq!(
            u32::from_le_bytes(packet[9..13].try_into().expect("door anchor bytes")),
            door_state.timestamp
        );
    }

    #[test]
    fn security_reject_log_line_is_structured() {
        assert_eq!(
            security_reject_log_line(42, "position", "impossible_velocity"),
            "server security reject id=42 category=position reason=impossible_velocity"
        );
    }

    #[test]
    fn security_reject_summary_aggregates_drained_buckets() {
        assert_eq!(
            summarize_drained_rejects(vec![
                (("damage", "stale_weapon"), 1),
                (("position", "teleport"), 2),
                (("position", "impossible_velocity"), 1),
            ]),
            3,
            "three distinct (category, reason) buckets"
        );
        // A quiet window drains nothing and logs nothing.
        assert_eq!(summarize_drained_rejects(Vec::new()), 0);
    }

    #[test]
    fn log_security_reject_accumulates_one_count_per_reason() {
        // Test-only reason and a delta assertion: the counter map is
        // process-global and `cargo test` runs this beside every other test
        // that logs rejects, so an absolute count is scheduling-dependent.
        let before = peek_security_reject_count("position", "test_counter_accumulation");
        log_security_reject(7, "position", "test_counter_accumulation");
        log_security_reject(7, "position", "test_counter_accumulation");
        log_security_reject(9, "position", "test_counter_other_reason");
        assert_eq!(
            peek_security_reject_count("position", "test_counter_accumulation") - before,
            2
        );
    }

    #[test]
    fn time_sync_response_carries_the_exact_server_and_arena_generation() {
        let packet = time_sync_response(
            &[CORE_BYTE, 0x05, 0x44, 0x33, 0x22, 0x11],
            0x0102_0304_0506_0708,
            0x1112_1314_1516_1718,
        );
        assert_eq!(packet.len(), 26);
        assert_eq!(
            &packet[0..6],
            &[CORE_BYTE, TIME_SYNC_RESPONSE, 0x44, 0x33, 0x22, 0x11]
        );
        assert_eq!(
            u64::from_le_bytes(packet[10..18].try_into().expect("server session bytes")),
            0x0102_0304_0506_0708
        );
        assert_eq!(
            u64::from_le_bytes(packet[18..26].try_into().expect("arena stream bytes")),
            0x1112_1314_1516_1718
        );
    }
}
