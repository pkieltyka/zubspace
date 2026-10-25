//! Opt-in, bounded client diagnostics. No report can mutate gameplay state.
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tracing_subscriber::fmt::time::FormatTime;

use crate::authority_identity::PlayerSessionIdentity;
use crate::qa_trace_protocol::QaTraceContext;
use crate::state::ServerState;

pub(crate) const MAX_BYTES: usize = 192 * 1024;
pub(crate) const MAX_FRAMES: usize = 1200;
pub(crate) const FRAME_WIDTH: usize = 34;
static LOG_WRITE: Mutex<()> = Mutex::new(());

#[derive(Default)]
pub(crate) struct ClientDebugMetrics {
    pub(crate) reports: u64,
    pub(crate) frames: u64,
    pub(crate) dropped_frames: u64,
    pub(crate) rejected: u64,
    pub(crate) log_failures: u64,
    pub(crate) browser_errors: u64,
    pub(crate) context_losses: u64,
    pub(crate) context_restorations: u64,
    pub(crate) max_follow_error_millipixels: u64,
    recent: BTreeMap<[u8; 16], (u32, Instant)>,
}

pub(crate) fn authorize(
    state: &ServerState,
    bundle: &str,
    token: &str,
) -> Option<(QaTraceContext, PlayerSessionIdentity)> {
    if !state.config.client_debug_trace {
        return None;
    }
    let (context, identity) = state.qa_traces.authenticate_client_debug(bundle, token)?;
    let client = state.clients.get(&identity.player_id)?;
    (client.joined
        && client.session_id == identity.session_id
        && client.arena_stream_id == context.arena_stream_id)
        .then_some((context, identity))
}

pub(crate) fn validate(body: &[u8]) -> Result<Value, &'static str> {
    if body.len() > MAX_BYTES {
        return Err("client_debug_too_large");
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| "invalid_client_debug_json")?;
    let object = value.as_object().ok_or("invalid_client_debug_schema")?;
    let keys = [
        "sequence",
        "build",
        "scope",
        "frames",
        "events",
        "droppedFrames",
    ];
    if object.len() != keys.len() || object.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err("invalid_client_debug_schema");
    }
    if value["sequence"]
        .as_u64()
        .is_none_or(|n| n == 0 || n > u64::from(u32::MAX))
        || value["droppedFrames"]
            .as_u64()
            .is_none_or(|n| n > 1_000_000)
        || value["build"].as_str().is_none_or(|tag| {
            tag.is_empty()
                || tag.len() > 128
                || !tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._:+-".contains(c))
        })
    {
        return Err("invalid_client_debug_schema");
    }
    let scope = value["scope"]
        .as_object()
        .ok_or("invalid_client_debug_scope")?;
    if scope.len() != 3
        || !["serverSessionId", "arenaStreamId", "playerId"]
            .iter()
            .all(|key| scope.contains_key(*key))
    {
        return Err("invalid_client_debug_scope");
    }
    let frames = value["frames"]
        .as_array()
        .ok_or("invalid_client_debug_frames")?;
    if frames.len() > MAX_FRAMES {
        return Err("invalid_client_debug_frames");
    }
    let mut previous = -1.0;
    for frame in frames {
        let row = frame.as_array().ok_or("invalid_client_debug_frames")?;
        if row.len() != FRAME_WIDTH
            || row
                .iter()
                .any(|n| n.as_f64().is_none_or(|v| !v.is_finite() || v.abs() > 1e12))
        {
            return Err("invalid_client_debug_frames");
        }
        let time = row[0].as_f64().unwrap();
        let delta = row[1].as_f64().unwrap();
        if time < previous || time < 0.0 || !(0.0..=30_000.0).contains(&delta) {
            return Err("invalid_client_debug_frames");
        }
        previous = time;
    }
    let events = value["events"]
        .as_array()
        .ok_or("invalid_client_debug_events")?;
    if frames.is_empty() && events.is_empty() {
        return Err("empty_client_debug_report");
    }
    let mut event_counts = [0; 7];
    let valid_events = events.len() <= 49
        && events.iter().all(|event| {
            let Some(row) = event.as_array() else {
                return false;
            };
            let Some(kind @ 1..=6) = row.first().and_then(Value::as_u64) else {
                return false;
            };
            let width = match kind {
                5 => 34,
                6 => 3,
                _ => 2,
            };
            if row.len() != width
                || row
                    .iter()
                    .any(|n| n.as_f64().is_none_or(|v| !v.is_finite() || v.abs() > 1e12))
                || row[1].as_f64().is_none_or(|n| n < 0.0)
                || (kind == 6 && row[2].as_u64().is_none_or(|n| n > 1_000_000))
            {
                return false;
            }
            event_counts[kind as usize] += 1;
            true
        });
    if !valid_events
        || event_counts[1..=4].iter().sum::<usize>() > 32
        || event_counts[5] > 16
        || event_counts[6] > 1
    {
        return Err("invalid_client_debug_events");
    }
    Ok(value)
}

pub(crate) fn accept(
    state: &mut ServerState,
    value: &Value,
    bundle: &str,
    token: &str,
    now: Instant,
) -> Result<(QaTraceContext, PathBuf), &'static str> {
    let (context, _) = authorize(state, bundle, token).ok_or("invalid_client_debug_session")?;
    let scope = &value["scope"];
    if scope["serverSessionId"].as_str() != Some(context.server_session_id.to_string().as_str())
        || scope["arenaStreamId"].as_str() != Some(context.arena_stream_id.to_string().as_str())
        || scope["playerId"].as_u64() != Some(u64::from(context.player_id))
    {
        return Err("client_debug_scope_mismatch");
    }
    let sequence = value["sequence"].as_u64().unwrap() as u32;
    let metrics = &mut state.telemetry.client_debug;
    if let Some((last_sequence, last_at)) = metrics.recent.get(&context.bundle_id) {
        if sequence <= *last_sequence {
            return Err("client_debug_duplicate");
        }
        if now.saturating_duration_since(*last_at) < Duration::from_secs(10) {
            return Err("client_debug_rate_limited");
        }
    }
    if !metrics.recent.contains_key(&context.bundle_id) && metrics.recent.len() >= 16 {
        if let Some(oldest) = metrics
            .recent
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(key, _)| *key)
        {
            metrics.recent.remove(&oldest);
        }
    }
    metrics.recent.insert(context.bundle_id, (sequence, now));
    metrics.reports = metrics.reports.saturating_add(1);
    let frames = value["frames"].as_array().unwrap();
    metrics.frames = metrics.frames.saturating_add(frames.len() as u64);
    metrics.dropped_frames = metrics
        .dropped_frames
        .saturating_add(value["droppedFrames"].as_u64().unwrap());
    for event in value["events"].as_array().unwrap() {
        match event[0].as_u64().unwrap() {
            1 | 2 => metrics.browser_errors = metrics.browser_errors.saturating_add(1),
            3 => metrics.context_losses = metrics.context_losses.saturating_add(1),
            4 => metrics.context_restorations = metrics.context_restorations.saturating_add(1),
            5 | 6 => {} // Paired muzzle evidence and disclosed ring evictions, not browser errors.
            _ => unreachable!("validated event"),
        }
    }
    for frame in frames {
        // Rows: browser at/dt, then the 32-column WASM presentation record.
        if frame[7].as_f64().unwrap() >= 0.0 {
            let dx = frame[3].as_f64().unwrap() - frame[8].as_f64().unwrap();
            let dy = frame[4].as_f64().unwrap() - frame[9].as_f64().unwrap();
            let milli = (dx.hypot(dy).min(1_000_000.0) * 1000.0) as u64;
            metrics.max_follow_error_millipixels = metrics.max_follow_error_millipixels.max(milli);
        }
    }
    let log = state
        .config
        .log_file
        .as_deref()
        .and_then(|file| std::path::Path::new(file).parent())
        .unwrap_or_else(|| std::path::Path::new(".debug"))
        .join("client.log");
    Ok((context, log))
}

pub(crate) fn write_report(
    path: &std::path::Path,
    context: QaTraceContext,
    report: Value,
) -> std::io::Result<()> {
    let mut timestamp = String::new();
    tracing_subscriber::fmt::time::SystemTime
        .format_time(&mut tracing_subscriber::fmt::format::Writer::new(
            &mut timestamp,
        ))
        .map_err(std::io::Error::other)?;
    let event = json!({"timestamp":timestamp,"level":"DEBUG","event":"client_presentation_window","schemaVersion":1,"bundleId":context.bundle_id_hex(),"serverSessionId":context.server_session_id.to_string(),"arenaStreamId":context.arena_stream_id.to_string(),"playerId":context.player_id,"serverBinarySha256":context.server_binary_sha256_hex(),"report":report});
    let mut bytes = serde_json::to_vec(&event)?;
    bytes.push(b'\n');
    let _guard = LOG_WRITE.lock().expect("client diagnostic log lock");
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, mpsc};

    fn fixture() -> (
        Arc<Mutex<ServerState>>,
        QaTraceContext,
        mpsc::Receiver<Vec<u8>>,
    ) {
        let shared = Arc::new(Mutex::new(ServerState::new(crate::config::ServerConfig {
            client_debug_trace: true,
            ..Default::default()
        })));
        let (tx, rx) = mpsc::channel();
        let id = crate::arena::register_connection(&shared, tx);
        crate::arena::join_arena(&shared, id, 0);
        let mut state = shared.lock().unwrap();
        let identity = state.active_player_session_identity(id).unwrap();
        let arena = state.clients[&id].arena_stream_id;
        let context = state.qa_traces.register(identity, arena);
        drop(state);
        (shared, context, rx)
    }

    fn report(context: QaTraceContext) -> Value {
        let mut row = vec![0.0; FRAME_WIDTH];
        row[7] = 7.0;
        row[3] = 10.0;
        row[8] = 7.0;
        row[4] = 20.0;
        row[9] = 16.0;
        json!({"sequence":1,"build":"dev","scope":{"serverSessionId":context.server_session_id.to_string(),"arenaStreamId":context.arena_stream_id.to_string(),"playerId":context.player_id},"frames":[row],"events":[[1,0]],"droppedFrames":2})
    }

    #[test]
    fn client_debug_payload_is_bounded_numeric_and_rejects_unknown_content() {
        let (_, context, _rx) = fixture();
        let valid = report(context);
        assert!(validate(&serde_json::to_vec(&valid).unwrap()).is_ok());
        let mut event_only = valid.clone();
        event_only["frames"] = json!([]);
        assert!(validate(&serde_json::to_vec(&event_only).unwrap()).is_ok());
        event_only["events"] = json!([]);
        assert!(validate(&serde_json::to_vec(&event_only).unwrap()).is_err());
        for pointer in ["/frames/0/0", "/frames/0/10", "/events/0/0", "/sequence"] {
            let mut malformed = valid.clone();
            *malformed.pointer_mut(pointer).unwrap() = json!("private text");
            assert!(validate(&serde_json::to_vec(&malformed).unwrap()).is_err());
        }
        let mut malformed = valid.clone();
        malformed["chat"] = json!("not permitted");
        assert!(validate(&serde_json::to_vec(&malformed).unwrap()).is_err());
        malformed = valid.clone();
        malformed["frames"] = json!(vec![vec![0.0; FRAME_WIDTH]; MAX_FRAMES + 1]);
        assert!(validate(&serde_json::to_vec(&malformed).unwrap()).is_err());
        assert!(validate(&vec![b' '; MAX_BYTES + 1]).is_err());
    }

    #[test]
    fn client_debug_muzzle_rows_are_numeric_bounded_and_isolated_from_error_metrics() {
        let (shared, context, _rx) = fixture();
        let mut report = report(context);
        let mut row = vec![json!(0.0); 34];
        row[0] = json!(5);
        row[19] = json!(0.024_999_99);
        report["events"] = json!([row, [6, 0, 184]]);
        let validated = validate(&serde_json::to_vec(&report).unwrap()).unwrap();
        let mut state = shared.lock().unwrap();
        let energy = state.clients[&context.player_id].server_energy;
        accept(
            &mut state,
            &validated,
            &context.bundle_id_hex(),
            &context.upload_token_hex(),
            Instant::now(),
        )
        .unwrap();
        assert_eq!(state.clients[&context.player_id].server_energy, energy);
        assert_eq!(state.telemetry.client_debug.browser_errors, 0);
        assert_eq!(state.telemetry.client_debug.context_losses, 0);
        drop(state);
        for bad in [
            json!([5, 0]),
            json!([6, 0, -1]),
            json!([6, 0, 1.5]),
            json!([7, 0]),
            json!([5, "private", 1]),
            json!(vec![vec![1, 0]; 33]),
        ] {
            let mut malformed = report.clone();
            malformed["events"] = if bad[0].is_array() { bad } else { json!([bad]) };
            assert!(validate(&serde_json::to_vec(&malformed).unwrap()).is_err());
        }
        for events in [json!(vec![row.clone(); 17]), json!([[6, 0, 1], [6, 0, 1]])] {
            let mut malformed = report.clone();
            malformed["events"] = events;
            assert!(validate(&serde_json::to_vec(&malformed).unwrap()).is_err());
        }
    }

    #[test]
    fn client_debug_auth_rate_scope_and_retirement_do_not_change_gameplay() {
        let (shared, context, _rx) = fixture();
        let value = report(context);
        let body = validate(&serde_json::to_vec(&value).unwrap()).unwrap();
        let mut state = shared.lock().unwrap();
        let now = Instant::now();
        let before = (
            state.clients[&context.player_id].server_energy,
            state.clients[&context.player_id].life_seq,
            state.clients[&context.player_id].inputs_received,
        );
        assert!(authorize(&state, &context.bundle_id_hex(), &"0".repeat(64)).is_none());
        let mut wrong = body.clone();
        wrong["scope"]["arenaStreamId"] = json!("999");
        assert!(
            accept(
                &mut state,
                &wrong,
                &context.bundle_id_hex(),
                &context.upload_token_hex(),
                now
            )
            .is_err()
        );
        assert!(
            accept(
                &mut state,
                &body,
                &context.bundle_id_hex(),
                &context.upload_token_hex(),
                now
            )
            .is_ok()
        );
        assert_eq!(
            accept(
                &mut state,
                &body,
                &context.bundle_id_hex(),
                &context.upload_token_hex(),
                now
            )
            .unwrap_err(),
            "client_debug_duplicate"
        );
        let mut next = body.clone();
        next["sequence"] = json!(2);
        assert_eq!(
            accept(
                &mut state,
                &next,
                &context.bundle_id_hex(),
                &context.upload_token_hex(),
                now
            )
            .unwrap_err(),
            "client_debug_rate_limited"
        );
        assert!(
            accept(
                &mut state,
                &next,
                &context.bundle_id_hex(),
                &context.upload_token_hex(),
                now + Duration::from_secs(20)
            )
            .is_ok()
        );
        assert_eq!(
            state.telemetry.client_debug.max_follow_error_millipixels,
            5000
        );
        let exposition = crate::observability::metrics(&state, Instant::now());
        assert!(exposition.contains("zubspace_client_debug_reports_total 2\n"));
        assert!(exposition.contains("zubspace_client_debug_follow_camera_error_pixels_max 5\n"));
        assert_eq!(
            before,
            (
                state.clients[&context.player_id].server_energy,
                state.clients[&context.player_id].life_seq,
                state.clients[&context.player_id].inputs_received
            )
        );
        state.clients.get_mut(&context.player_id).unwrap().joined = false;
        assert!(
            authorize(
                &state,
                &context.bundle_id_hex(),
                &context.upload_token_hex()
            )
            .is_none()
        );
    }

    fn request(
        shared: Arc<Mutex<ServerState>>,
        context: QaTraceContext,
        method: &str,
        body: &[u8],
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            crate::connection::handle_client(stream, shared).unwrap();
        });
        let mut client = TcpStream::connect(addr).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let origin = if method == "GET" {
            ""
        } else {
            "Origin: http://localhost:7000\r\n"
        };
        let header = format!(
            "{method} /v1/debug/client-trace HTTP/1.1\r\nHost: localhost\r\n{origin}Content-Type: application/json\r\nX-Zubspace-QA-Bundle: {}\r\nX-Zubspace-QA-Token: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            context.bundle_id_hex(),
            context.upload_token_hex(),
            body.len()
        );
        let message = [header.as_bytes(), body].concat();
        client.write_all(&message).unwrap();
        // An early authenticated rejection can close with unread request
        // bytes. Read the complete HTTP response by its declared framing;
        // waiting for TCP EOF instead can report a later reset after 403.
        let mut response = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let read = client.read(&mut chunk).unwrap();
            assert_ne!(read, 0, "HTTP response ended before its declared body");
            response.extend_from_slice(&chunk[..read]);
            if let Some(end) = response.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&response[..end]).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .expect("HTTP response must declare its body length");
                if response.len() >= end + 4 + length {
                    break;
                }
            }
        }
        server.join().unwrap();
        String::from_utf8(response).unwrap()
    }

    #[test]
    fn client_debug_http_gate_and_ndjson_ingestion_work_without_qa_upload() {
        let (shared, context, _rx) = fixture();
        let directory = std::env::temp_dir().join(format!(
            "zubspace-client-debug-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        shared.lock().unwrap().config.log_file =
            Some(directory.join("server.log").to_string_lossy().into_owned());
        assert!(!shared.lock().unwrap().config.qa_trace_upload);
        shared.lock().unwrap().config.client_debug_trace = false;
        assert!(request(shared.clone(), context, "GET", b"").starts_with("HTTP/1.1 404"));
        shared.lock().unwrap().config.client_debug_trace = true;
        let policy = request(shared.clone(), context, "GET", b"");
        assert!(policy.starts_with("HTTP/1.1 200"));
        assert!(policy.contains("\"intervalMs\":20000"));
        let body = serde_json::to_vec(&report(context)).unwrap();
        assert!(request(shared.clone(), context, "POST", &body).starts_with("HTTP/1.1 200"));
        let raw = fs::read_to_string(directory.join("client.log")).unwrap();
        let line: Value = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(line["event"], "client_presentation_window");
        assert!(line["report"]["frames"].is_array());
        assert!(!raw.contains(&context.upload_token_hex()));
        assert_eq!(raw.lines().count(), 1);
        assert!(request(shared.clone(), context, "POST", &body).starts_with("HTTP/1.1 403"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn client_debug_http_rejects_retired_tokens_after_real_lifecycle_boundaries() {
        for transition in ["leave", "disconnect", "rejoin", "replacement", "zone"] {
            let (shared, old, _rx) = fixture();
            let directory = std::env::temp_dir().join(format!(
                "zubspace-client-debug-retired-{}-{}-{transition}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            shared.lock().unwrap().config.log_file =
                Some(directory.join("server.log").to_string_lossy().into_owned());
            let body = serde_json::to_vec(&report(old)).unwrap();
            assert!(request(shared.clone(), old, "POST", &body).starts_with("HTTP/1.1 200"));
            let original_log = fs::read(directory.join("client.log")).unwrap();
            let identity = shared
                .lock()
                .unwrap()
                .active_player_session_identity(old.player_id)
                .unwrap();
            let mut replacement_receiver = None;
            if matches!(transition, "disconnect" | "replacement") {
                crate::arena::disconnect_player_session(&shared, identity);
            } else {
                crate::arena::leave_arena(&shared, old.player_id);
            }
            if transition == "replacement" {
                shared.lock().unwrap().next_player_id = old.player_id;
                let (tx, rx) = mpsc::channel();
                replacement_receiver = Some(rx);
                let id = crate::arena::register_connection(&shared, tx);
                assert_eq!(id, old.player_id);
                crate::arena::join_arena(&shared, id, 0);
                assert_ne!(
                    shared.lock().unwrap().clients[&id].session_id,
                    identity.session_id
                );
                // A delayed departure from the retired session cannot evict
                // the new session or revive its diagnostic credentials.
                crate::arena::disconnect_player_session(&shared, identity);
            } else if transition == "zone" {
                let mut login = vec![0; 27];
                login[0] = 1;
                login[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
                login[26] = 1;
                login[10..17].copy_from_slice(b"Extreme");
                crate::arena::join_arena_for_login(&shared, old.player_id, 0, &login).unwrap();
                assert_eq!(
                    shared.lock().unwrap().config.default_zone,
                    crate::zone::EXTREME_ZONE_ID
                );
            } else if transition == "rejoin" {
                crate::arena::join_arena(&shared, old.player_id, 0);
            }
            let before = {
                let state = shared.lock().unwrap();
                (
                    state.telemetry.client_debug.reports,
                    state.telemetry.client_debug.frames,
                    state.telemetry.client_debug.log_failures,
                    state.clients.get(&old.player_id).map(|c| {
                        (
                            c.session_id,
                            c.arena_stream_id,
                            c.life_seq,
                            c.server_energy,
                            c.inputs_received,
                        )
                    }),
                )
            };
            // A fresh sequence rules out duplicate/rate errors as the cause
            // of rejection: HTTP authorization must reject the old session.
            let mut next = report(old);
            next["sequence"] = json!(2);
            let next = serde_json::to_vec(&next).unwrap();
            for (method, payload) in [("GET", &b""[..]), ("POST", next.as_slice())] {
                let response = request(shared.clone(), old, method, payload);
                assert!(
                    response.starts_with("HTTP/1.1 403"),
                    "{transition}: {response}"
                );
                assert!(
                    response.contains("invalid_client_debug_session"),
                    "{transition}: {response}"
                );
                assert!(!response.contains(&old.upload_token_hex()));
            }
            assert_eq!(
                fs::read(directory.join("client.log")).unwrap(),
                original_log
            );
            let mut state = shared.lock().unwrap();
            assert_eq!(
                before,
                (
                    state.telemetry.client_debug.reports,
                    state.telemetry.client_debug.frames,
                    state.telemetry.client_debug.log_failures,
                    state.clients.get(&old.player_id).map(|c| (
                        c.session_id,
                        c.arena_stream_id,
                        c.life_seq,
                        c.server_energy,
                        c.inputs_received
                    ))
                )
            );
            if matches!(transition, "rejoin" | "replacement" | "zone") {
                let fresh_identity = state.active_player_session_identity(old.player_id).unwrap();
                let stream = state.clients[&old.player_id].arena_stream_id;
                let fresh = state.qa_traces.register(fresh_identity, stream);
                assert_ne!(fresh.bundle_id, old.bundle_id);
                drop(state);
                assert!(request(shared.clone(), fresh, "GET", b"").starts_with("HTTP/1.1 200"));
                let body = serde_json::to_vec(&report(fresh)).unwrap();
                assert!(request(shared.clone(), fresh, "POST", &body).starts_with("HTTP/1.1 200"));
                assert_eq!(
                    fs::read_to_string(directory.join("client.log"))
                        .unwrap()
                        .lines()
                        .count(),
                    2
                );
            } else {
                drop(state);
            }
            drop(replacement_receiver);
            fs::remove_dir_all(directory).unwrap();
        }
    }
}
