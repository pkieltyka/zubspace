#[path = "public_api.rs"]
mod public_api;

use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::arena::{
    ARENA_LOGIN, ARENA_LOGIN_PROTOCOL_REVISION_OFFSET, CLIENT_POSITION, FREQUENCY_CHANGE_REQUEST,
    LEAVE_ARENA_REQUEST, SHIP_CHANGE_REQUEST, join_arena_for_login, normalized_ship,
};
use crate::arena_settings::arena_ship_rocket_lifetime_ticks;
use crate::authority_identity::{PlayerRuntimeIdentity, PlayerSessionIdentity};
use crate::http::read_http_header;
use crate::identity::{
    admit_player_login, login_name, login_player_identity, normalized_login_name,
    player_login_accepted,
};
use crate::objectives::{
    ATTACH_REQUEST, BALL_FIRE_REQUEST, BALL_GOAL_REQUEST, BALL_PICKUP_REQUEST,
    DROP_ATTACHED_REQUEST, DROP_BRICK_REQUEST, DROP_FLAGS_REQUEST, FLAG_REQUEST, relay_ball_fire,
    relay_ball_goal, relay_ball_pickup, relay_drop_flags, relay_flag_request,
    relay_objective_action_at_delivery,
};
use crate::player_events::{
    CHAT_MESSAGE_REQUEST, DEATH_REQUEST, SPECTATE_REQUEST, TAKE_PRIZE_REQUEST,
    apply_prize_effect_application, relay_chat, relay_prize_pickup, update_spectate,
};
use crate::reliable::{
    packet_cluster_parts, receive_reliable_acks, reliable_ack, reliable_parts, send_reliable_to,
    send_to,
};
use crate::roster::{SET_BANNER_REQUEST, update_banner};
use crate::rtc::{RtcSignalingError, create_answer_for_offer_json, signaling_capabilities_json};
use crate::security::{
    SECURITY_RESPONSE, is_time_sync_request, log_security_reject, time_sync_response,
    time_sync_response_for_token, update_security_report,
};
use crate::session::{
    CACHED_MAP_CHECK, FILE_REQUEST, NEWS_REQUEST, PASSWORD_LOGIN_CONTINUUM, login_failure_message,
    password_response, serve_cached_map_check, serve_file_request, serve_news_request,
};
use crate::state::{
    BrickSourceBinding, PendingReliableGameplayMutationKind, ReliableAttachRequest,
    ReliableBrickRequest, ReliableFrequencyChange, ReliablePrizeRequest, ReliableShipChange,
    ServerState,
};
use tracing::{debug, info, warn};

const SIGNALING_RATE_WINDOW: Duration = Duration::from_secs(60);

/// Explicit debug-server fault seam for proving the browser's initial map-file re-request path.
/// The dropped request is acknowledged by the reliable application layer, but no file response is started.
#[cfg(debug_assertions)]
fn qa_drop_initial_map_file_request(payload: &[u8]) -> Option<(usize, usize)> {
    if payload.len() != 3
        || payload.first() != Some(&FILE_REQUEST)
        || u16::from_le_bytes([payload[1], payload[2]]) != 0
    {
        return None;
    }
    let budget = std::env::var("ZUBSPACE_SERVER_TEST_DROP_INITIAL_MAP_FILE_REQUESTS")
        .ok()?
        .parse::<usize>()
        .ok()?;
    if budget == 0 {
        return None;
    }
    static SUPPRESSED: AtomicUsize = AtomicUsize::new(0);
    let ordinal = SUPPRESSED.fetch_add(1, Ordering::Relaxed) + 1;
    (ordinal <= budget).then_some((ordinal, budget))
}

#[cfg(not(debug_assertions))]
fn qa_drop_initial_map_file_request(_payload: &[u8]) -> Option<(usize, usize)> {
    None
}

struct SignalingRequestContext {
    client_key: String,
    cors_origin: String,
}

struct SignalingRejection {
    status: &'static str,
    code: &'static str,
    message: String,
    cors_origin: Option<String>,
}

#[cfg(test)]
pub(crate) fn handle_client(stream: TcpStream, state: Arc<Mutex<ServerState>>) -> io::Result<()> {
    handle_client_with_started_at(stream, state, Instant::now())
}

pub(crate) fn handle_client_with_started_at(
    mut stream: TcpStream,
    state: Arc<Mutex<ServerState>>,
    started_at: Instant,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let peer = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let Some(request) = read_http_header(&mut stream)? else {
        debug!(event = "http_preconnect_closed", peer = %peer,
            "HTTP socket closed before sending request bytes");
        return Ok(());
    };
    handle_http_request(&mut stream, &request, &peer, &state, started_at)
}

fn handle_http_request(
    stream: &mut TcpStream,
    request: &str,
    peer: &str,
    state: &Arc<Mutex<ServerState>>,
    started_at: Instant,
) -> io::Result<()> {
    let Some((method, path)) = http_request_target(request) else {
        warn!(peer = %peer, reason = "bad_request", "server http request rejected");
        return write_http_json(
            stream,
            "400 Bad Request",
            r#"{"ok":false,"error":"bad_request"}"#,
        );
    };

    if let Some(response) = public_api::handle(stream, method, path, state, started_at) {
        return response;
    }

    if matches!(path, "/v1/ping" | "/" | "/index.html") && !matches!(method, "GET" | "OPTIONS") {
        return write_http_response(
            stream,
            "405 Method Not Allowed",
            "application/json",
            r#"{"ok":false,"error":"method_not_allowed"}"#,
            &[
                ("Access-Control-Allow-Origin", "*"),
                ("Allow", "GET, OPTIONS"),
            ],
        );
    }

    if method == "OPTIONS" && path == "/v1/rtc/offer" {
        let context = match validate_signaling_request(request, peer, state, true) {
            Ok(context) => context,
            Err(rejection) => return write_signaling_rejection(stream, rejection),
        };
        debug!(peer = %peer, method, path, "server http preflight");
        return write_http_response(
            stream,
            "204 No Content",
            "application/json",
            "",
            &[
                ("Access-Control-Allow-Origin", &context.cors_origin),
                ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
                (
                    "Access-Control-Allow-Headers",
                    "Authorization, Content-Type",
                ),
                ("Vary", "Origin"),
            ],
        );
    }
    if method == "OPTIONS" && matches!(path, "/v1/qa/trace" | "/v1/debug/client-trace") {
        let origin = match diagnostic_cors_origin(request, state, path == "/v1/debug/client-trace")
        {
            Ok(origin) => origin,
            Err((status, code)) => {
                return write_http_json(
                    stream,
                    status,
                    &format!(r#"{{"ok":false,"error":"{code}"}}"#),
                );
            }
        };
        return write_http_response(
            stream,
            "204 No Content",
            "application/json",
            "",
            &[
                ("Access-Control-Allow-Origin", &origin),
                (
                    "Access-Control-Allow-Methods",
                    if path == "/v1/debug/client-trace" {
                        "GET, POST, OPTIONS"
                    } else {
                        "POST, OPTIONS"
                    },
                ),
                (
                    "Access-Control-Allow-Headers",
                    "Content-Type, X-Zubspace-QA-Bundle, X-Zubspace-QA-Token",
                ),
                ("Vary", "Origin"),
            ],
        );
    }
    if method == "OPTIONS" && matches!(path, "/v1/ping" | "/" | "/index.html") {
        return write_http_response(
            stream,
            "204 No Content",
            "application/json",
            "",
            &[
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Methods", "GET, OPTIONS"),
                ("Access-Control-Allow-Headers", "Content-Type"),
                ("Cache-Control", "no-store"),
            ],
        );
    }
    if method == "OPTIONS" && path.starts_with("/v1/rtc") {
        debug!(peer = %peer, method, path, "server http preflight");
        return write_http_response(
            stream,
            "204 No Content",
            "application/json",
            "",
            &[
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
                ("Access-Control-Allow-Headers", "Content-Type"),
            ],
        );
    }

    match (method, path) {
        ("GET", "/v1/ping") => write_public_response(stream, "text/plain; charset=utf-8", "."),
        ("GET", "/") | ("GET", "/index.html") => write_public_response(
            stream,
            "text/plain; charset=utf-8",
            "Welcome to the Zubspace game server.\n",
        ),
        ("GET", "/rtc") | ("GET", "/rtc/signaling") => {
            debug!(peer = %peer, "server rtc signaling probe");
            write_http_json(stream, "200 OK", signaling_capabilities_json())
        }
        ("POST", "/v1/rtc/offer") => {
            let context = match validate_signaling_request(request, peer, state, false) {
                Ok(context) => context,
                Err(rejection) => return write_signaling_rejection(stream, rejection),
            };
            if !http_content_type_is_json(request) {
                return write_http_json_with_origin(
                    stream,
                    "415 Unsupported Media Type",
                    r#"{"ok":false,"error":"content_type_must_be_json"}"#,
                    &context.cors_origin,
                );
            }
            let content_length = http_content_length(request)?;
            let max_offer_bytes = state
                .lock()
                .expect("server state should not be poisoned")
                .config
                .signaling_max_offer_bytes;
            if content_length > max_offer_bytes {
                return write_http_json_with_origin(
                    stream,
                    "413 Content Too Large",
                    &format!(
                        r#"{{"ok":false,"error":"offer_too_large","max_bytes":{max_offer_bytes}}}"#
                    ),
                    &context.cors_origin,
                );
            }
            let mut body = vec![0; content_length];
            stream.read_exact(&mut body)?;
            info!(peer = %context.client_key, bytes = body.len(), "server rtc offer received");
            match create_answer_for_offer_json(Arc::clone(state), context.client_key.clone(), &body)
            {
                Ok(answer) => {
                    write_http_json_with_origin(stream, "200 OK", &answer, &context.cors_origin)
                }
                Err(err) => {
                    warn!(peer = %context.client_key, reason = %err, "server rtc offer rejected");
                    write_rtc_offer_error(stream, &err, &context.cors_origin)
                }
            }
        }
        ("GET", "/v1/debug/client-trace") | ("POST", "/v1/debug/client-trace") => {
            handle_client_debug(stream, method, request, state)
        }
        ("POST", "/v1/qa/trace") => {
            let origin = match qa_trace_cors_origin(request, state) {
                Ok(origin) => origin,
                Err((status, code)) => {
                    return write_http_json(
                        stream,
                        status,
                        &format!(r#"{{"ok":false,"error":"{code}"}}"#),
                    );
                }
            };
            if !http_content_type_is_json(request) {
                return write_http_json_with_origin(
                    stream,
                    "415 Unsupported Media Type",
                    r#"{"ok":false,"error":"content_type_must_be_json"}"#,
                    &origin,
                );
            }
            let content_length = http_content_length(request)?;
            if content_length > crate::qa_trace::QA_TRACE_MAX_UPLOAD_BYTES {
                return write_http_json_with_origin(
                    stream,
                    "413 Content Too Large",
                    &format!(
                        r#"{{"ok":false,"error":"qa_trace_too_large","max_bytes":{}}}"#,
                        crate::qa_trace::QA_TRACE_MAX_UPLOAD_BYTES
                    ),
                    &origin,
                );
            }
            let bundle_id = http_header(request, "x-zubspace-qa-bundle").unwrap_or_default();
            let token = http_header(request, "x-zubspace-qa-token").unwrap_or_default();
            let mut body = vec![0; content_length];
            stream.read_exact(&mut body)?;
            let result = (|| {
                let started = Instant::now();
                let context = state
                    .lock()
                    .expect("server state should not be poisoned")
                    .qa_traces
                    .upload_context(bundle_id, token, Instant::now())?;
                let authenticated = Instant::now();
                let browser = crate::qa_trace::parse_upload(&body, context)?;
                let parsed = Instant::now();
                let snapshot = state
                    .lock()
                    .expect("server state should not be poisoned")
                    .qa_traces
                    .capture_upload(context, Instant::now())?;
                let captured = Instant::now();
                let completed = snapshot.finish(browser);
                let assembled = Instant::now();
                let (joined, replaced) = state
                    .lock()
                    .expect("server state should not be poisoned")
                    .qa_traces
                    .retain_upload(&completed)?;
                // Large JSON parsing, assembly, destruction and serialization
                // must not occupy the game-state lock.
                drop(replaced);
                tracing::debug!(
                    event = "qa_trace_upload_wall_stages",
                    player_id = context.player_id,
                    bundle_id = %context.bundle_id_hex(),
                    authenticate_us = authenticated.duration_since(started).as_micros() as u64,
                    parse_validate_us = parsed.duration_since(authenticated).as_micros() as u64,
                    snapshot_wait_and_copy_us = captured.duration_since(parsed).as_micros() as u64,
                    assemble_us = assembled.duration_since(captured).as_micros() as u64,
                    retention_wait_commit_and_drop_us = assembled.elapsed().as_micros() as u64,
                    "diagnostic upload wall stages include scheduling and lock waits"
                );
                Ok::<_, crate::qa_trace::QaTraceError>(joined)
            })();
            match result {
                Ok(joined) => write_http_json_with_origin(
                    stream,
                    "200 OK",
                    &serde_json::to_string(joined.as_ref())
                        .expect("QA trace artifact should encode"),
                    &origin,
                ),
                Err(error) => write_http_json_with_origin(
                    stream,
                    error.status,
                    &format!(r#"{{"ok":false,"error":"{}"}}"#, error.code),
                    &origin,
                ),
            }
        }
        ("GET", path) if path.starts_with("/v1/qa/trace/") => {
            let enabled = state
                .lock()
                .expect("server state should not be poisoned")
                .config
                .qa_trace_upload;
            if !enabled {
                return write_http_json(
                    stream,
                    "404 Not Found",
                    r#"{"ok":false,"error":"not_found"}"#,
                );
            }
            if !socket_peer_is_loopback(peer) {
                return write_http_json(
                    stream,
                    "403 Forbidden",
                    r#"{"ok":false,"error":"qa_trace_retrieval_is_host_local"}"#,
                );
            }
            let bundle_id = path.trim_start_matches("/v1/qa/trace/");
            let result = state
                .lock()
                .expect("server state should not be poisoned")
                .qa_traces
                .retained(bundle_id);
            match result {
                Ok(joined) => write_http_json(
                    stream,
                    "200 OK",
                    &serde_json::to_string(joined.as_ref())
                        .expect("QA trace artifact should encode"),
                ),
                Err(error) => write_http_json(
                    stream,
                    error.status,
                    &format!(r#"{{"ok":false,"error":"{}"}}"#, error.code),
                ),
            }
        }
        _ => {
            warn!(
                peer = %peer,
                method,
                path,
                status = 404,
                "server http request rejected"
            );
            write_http_json(
                stream,
                "404 Not Found",
                r#"{"ok":false,"error":"not_found"}"#,
            )
        }
    }
}

fn qa_trace_cors_origin(
    request: &str,
    state: &Arc<Mutex<ServerState>>,
) -> Result<String, (&'static str, &'static str)> {
    diagnostic_cors_origin(request, state, false)
}

fn diagnostic_cors_origin(
    request: &str,
    state: &Arc<Mutex<ServerState>>,
    client_debug: bool,
) -> Result<String, (&'static str, &'static str)> {
    let config = state
        .lock()
        .expect("server state should not be poisoned")
        .config
        .clone();
    if !(if client_debug {
        config.client_debug_trace
    } else {
        config.qa_trace_upload
    }) {
        return Err(("404 Not Found", "not_found"));
    }
    // Same-origin browser GETs may omit Origin. This read-only policy still
    // requires a live session token in handle_client_debug; uploads always
    // require Origin and the normal allow-list check below.
    let origin = match http_header(request, "origin") {
        Some(origin) => origin,
        None if client_debug && request.starts_with("GET /v1/debug/client-trace ") => {
            return Ok("*".to_string());
        }
        None => return Err(("403 Forbidden", "origin_required")),
    };
    let allow_any = config
        .signaling_allowed_origins
        .iter()
        .any(|allowed| allowed == "*");
    if !allow_any
        && !config
            .signaling_allowed_origins
            .iter()
            .any(|allowed| allowed == origin)
    {
        return Err(("403 Forbidden", "origin_not_allowed"));
    }
    Ok(if allow_any { "*" } else { origin }.to_string())
}

fn handle_client_debug(
    stream: &mut TcpStream,
    method: &str,
    request: &str,
    state: &Arc<Mutex<ServerState>>,
) -> io::Result<()> {
    let origin = match diagnostic_cors_origin(request, state, true) {
        Ok(origin) => origin,
        Err((status, code)) => {
            return write_http_json(
                stream,
                status,
                &format!(r#"{{"ok":false,"error":"{code}"}}"#),
            );
        }
    };
    let bundle = http_header(request, "x-zubspace-qa-bundle").unwrap_or_default();
    let token = http_header(request, "x-zubspace-qa-token").unwrap_or_default();
    if crate::client_debug::authorize(&state.lock().expect("server state"), bundle, token).is_none()
    {
        return write_http_json_with_origin(
            stream,
            "403 Forbidden",
            r#"{"ok":false,"error":"invalid_client_debug_session"}"#,
            &origin,
        );
    }
    if method == "GET" {
        return write_http_json_with_origin(
            stream,
            "200 OK",
            r#"{"enabled":true,"schemaVersion":1,"frameWidth":34,"muzzleWidth":32,"intervalMs":20000,"maxFrames":1200,"maxBytes":196608}"#,
            &origin,
        );
    }
    if !http_content_type_is_json(request) {
        return write_http_json_with_origin(
            stream,
            "415 Unsupported Media Type",
            r#"{"ok":false,"error":"content_type_must_be_json"}"#,
            &origin,
        );
    }
    let length = http_content_length(request)?;
    if length > crate::client_debug::MAX_BYTES {
        return write_http_json_with_origin(
            stream,
            "413 Content Too Large",
            r#"{"ok":false,"error":"client_debug_too_large"}"#,
            &origin,
        );
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    let value = match crate::client_debug::validate(&body) {
        Ok(value) => value,
        Err(code) => {
            let mut state = state.lock().expect("server state");
            state.telemetry.client_debug.rejected =
                state.telemetry.client_debug.rejected.saturating_add(1);
            drop(state);
            return write_http_json_with_origin(
                stream,
                "400 Bad Request",
                &format!(r#"{{"ok":false,"error":"{code}"}}"#),
                &origin,
            );
        }
    };
    let accepted = {
        let mut state = state.lock().expect("server state");
        let result = crate::client_debug::accept(&mut state, &value, bundle, token, Instant::now());
        if result.is_err() {
            state.telemetry.client_debug.rejected =
                state.telemetry.client_debug.rejected.saturating_add(1);
        }
        result
    };
    let (context, path) = match accepted {
        Ok(result) => result,
        Err(code) => {
            return write_http_json_with_origin(
                stream,
                if code == "client_debug_rate_limited" {
                    "429 Too Many Requests"
                } else {
                    "403 Forbidden"
                },
                &format!(r#"{{"ok":false,"error":"{code}"}}"#),
                &origin,
            );
        }
    };
    let frames = value["frames"].as_array().map_or(0, Vec::len);
    let sequence = value["sequence"].as_u64().unwrap_or(0);
    if let Err(error) = crate::client_debug::write_report(&path, context, value) {
        let mut state = state.lock().expect("server state");
        state.telemetry.client_debug.log_failures =
            state.telemetry.client_debug.log_failures.saturating_add(1);
        drop(state);
        tracing::error!(event="client_debug_log_failed",%error,"client diagnostic log write failed");
        return write_http_json_with_origin(
            stream,
            "503 Service Unavailable",
            r#"{"ok":false,"error":"client_debug_log_failed"}"#,
            &origin,
        );
    }
    tracing::info!(event="client_debug_window",player_id=context.player_id,bundle_id=%context.bundle_id_hex(),sequence,frames,bytes=length,"client presentation diagnostics recorded");
    write_http_json_with_origin(stream, "200 OK", r#"{"ok":true}"#, &origin)
}

fn socket_peer_is_loopback(peer: &str) -> bool {
    peer.parse::<std::net::SocketAddr>()
        .is_ok_and(|address| address.ip().is_loopback())
}

fn validate_signaling_request(
    request: &str,
    socket_peer: &str,
    state: &Arc<Mutex<ServerState>>,
    preflight: bool,
) -> Result<SignalingRequestContext, SignalingRejection> {
    let config = state
        .lock()
        .expect("server state should not be poisoned")
        .config
        .clone();
    let origin = http_header(request, "origin");
    let allow_any_origin = config
        .signaling_allowed_origins
        .iter()
        .any(|allowed| allowed == "*");
    let origin_allowed = allow_any_origin
        || origin.is_some_and(|origin| {
            config
                .signaling_allowed_origins
                .iter()
                .any(|allowed| allowed == origin)
        });
    if !origin_allowed {
        return Err(SignalingRejection {
            status: "403 Forbidden",
            code: "origin_not_allowed",
            message: "request Origin is not allowed by the signaling policy".to_string(),
            cors_origin: None,
        });
    }
    let cors_origin = if allow_any_origin {
        "*".to_string()
    } else {
        origin.unwrap_or_default().to_string()
    };

    if config.signaling_require_https
        && (!config.signaling_trust_proxy_headers
            || !http_header(request, "x-forwarded-proto")
                .is_some_and(|value| value.eq_ignore_ascii_case("https")))
    {
        return Err(SignalingRejection {
            status: "403 Forbidden",
            code: "https_required",
            message: "signaling requires TLS at the trusted reverse proxy".to_string(),
            cors_origin: Some(cors_origin),
        });
    }

    let client_key = if config.signaling_trust_proxy_headers {
        http_header(request, "x-real-ip")
            .or_else(|| {
                http_header(request, "x-forwarded-for").and_then(|value| value.split(',').next())
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(socket_peer)
            .to_string()
    } else {
        socket_peer
            .parse::<std::net::SocketAddr>()
            .map(|address| address.ip().to_string())
            .unwrap_or_else(|_| socket_peer.to_string())
    };

    if !preflight {
        consume_signaling_rate(state, &client_key, &cors_origin)?;
        if let Some(expected) = config.signaling_auth_token.as_deref() {
            let supplied = http_header(request, "authorization")
                .and_then(|value| value.strip_prefix("Bearer "))
                .unwrap_or_default();
            if !constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
                return Err(SignalingRejection {
                    status: "401 Unauthorized",
                    code: "invalid_admission_token",
                    message: "a valid bearer admission token is required".to_string(),
                    cors_origin: Some(cors_origin),
                });
            }
        }
    }

    Ok(SignalingRequestContext {
        client_key,
        cors_origin,
    })
}

fn consume_signaling_rate(
    state: &Arc<Mutex<ServerState>>,
    client_key: &str,
    cors_origin: &str,
) -> Result<(), SignalingRejection> {
    let now = Instant::now();
    let mut guard = state.lock().expect("server state should not be poisoned");
    guard.signaling_rate_windows.retain(|_, window| {
        now.saturating_duration_since(window.started_at) < SIGNALING_RATE_WINDOW
    });
    let rate = guard.config.signaling_offer_rate_per_minute;
    let window = guard
        .signaling_rate_windows
        .entry(client_key.to_string())
        .or_insert(crate::state::SignalingRateWindow {
            started_at: now,
            offers: 0,
        });
    if window.offers >= rate {
        return Err(SignalingRejection {
            status: "429 Too Many Requests",
            code: "offer_rate_limited",
            message: format!("at most {rate} offers are allowed per client per minute"),
            cors_origin: Some(cors_origin.to_string()),
        });
    }
    window.offers += 1;
    Ok(())
}

fn write_signaling_rejection(
    stream: &mut TcpStream,
    rejection: SignalingRejection,
) -> io::Result<()> {
    let body = format!(
        r#"{{"ok":false,"error":"{}","message":"{}"}}"#,
        rejection.code,
        json_escape(&rejection.message)
    );
    if let Some(origin) = rejection.cors_origin {
        write_http_json_with_origin(stream, rejection.status, &body, &origin)
    } else {
        write_http_json(stream, rejection.status, &body)
    }
}

fn constant_time_eq(expected: &[u8], supplied: &[u8]) -> bool {
    let mut difference = expected.len() ^ supplied.len();
    let length = expected.len().max(supplied.len());
    for index in 0..length {
        let left = expected.get(index).copied().unwrap_or(0);
        let right = supplied.get(index).copied().unwrap_or(0);
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

fn http_header<'a>(request: &'a str, wanted: &str) -> Option<&'a str> {
    request.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(wanted).then(|| value.trim())
    })
}

fn http_content_type_is_json(request: &str) -> bool {
    http_header(request, "content-type")
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn rtc_error_json(error: &RtcSignalingError) -> String {
    format!(
        r#"{{"ok":false,"error":"{}","message":"{}"}}"#,
        error.code(),
        json_escape(error.public_message())
    )
}

pub(crate) fn write_rtc_offer_error(
    stream: &mut TcpStream,
    error: &RtcSignalingError,
    origin: &str,
) -> io::Result<()> {
    let body = rtc_error_json(error);
    if let Some(retry_after) = error.retry_after_seconds() {
        write_http_response(
            stream,
            error.status(),
            "application/json",
            &body,
            &[
                ("Access-Control-Allow-Origin", origin),
                ("Vary", "Origin"),
                ("Retry-After", retry_after),
            ],
        )
    } else {
        write_http_json_with_origin(stream, error.status(), &body, origin)
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn server_status_snapshot(
    state: &ServerState,
    started_at: Instant,
) -> serde_json::Map<String, serde_json::Value> {
    let snapshot_acked_clients = state
        .clients
        .values()
        .filter(|client| client.last_acked_snapshot_id.is_some())
        .count();
    let threat_contracts_current: usize = state
        .clients
        .values()
        .map(|client| client.threat_presentation.len())
        .sum();
    let threat_contract_capacity_evictions: u64 = state
        .clients
        .values()
        .map(|client| client.threat_presentation.capacity_evictions)
        .sum();
    let threat_contract_stale_runtime_evictions: u64 = state
        .clients
        .values()
        .map(|client| client.threat_presentation.stale_runtime_evictions)
        .sum();
    let projectile_threat_contracts_current: usize = state
        .clients
        .values()
        .map(|client| client.threat_presentation.projectile_len())
        .sum();
    let projectile_threat_contracts_peak: usize = state
        .clients
        .values()
        .map(|client| client.threat_presentation.projectile_peak)
        .sum();
    let projectile_threat_contract_payload_bytes: usize = state
        .clients
        .values()
        .map(|client| {
            client
                .threat_presentation
                .projectile_retained_payload_bytes()
        })
        .sum();
    let projectile_threat_contract_capacity_rejections: u64 = state
        .clients
        .values()
        .map(|client| client.threat_presentation.projectile_capacity_rejections)
        .sum();
    let projectile_threat_contract_terminal_cleanups: u64 = state
        .clients
        .values()
        .map(|client| client.threat_presentation.projectile_terminal_cleanups)
        .sum();
    let rtc_overload = state.rtc_overload_metrics.snapshot();
    let repel_queue_packets: usize = state
        .clients
        .values()
        .map(|client| client.pending_repel_observations.len())
        .sum();
    let repel_queue_max_target_depth = state
        .clients
        .values()
        .map(|client| client.pending_repel_observations.len())
        .max()
        .unwrap_or(0);
    let repel_queue_awaiting_application: usize = state
        .clients
        .values()
        .flat_map(|client| client.pending_repel_observations.iter())
        .filter(|event| event.awaiting_application())
        .count();
    let repel_queue_awaiting_receipt_ack =
        repel_queue_packets.saturating_sub(repel_queue_awaiting_application);
    let warp_pickup_acks_deferred = state
        .clients
        .values()
        .flat_map(|client| client.pending_warp_prize_relocations.iter())
        .filter(|authorization| !authorization.pickup_ack_sent)
        .count();
    let movement_corrections_awaiting_application = state
        .clients
        .values()
        .filter(|client| {
            client
                .pending_movement_correction
                .as_ref()
                .is_some_and(|transaction| transaction.awaiting_application())
        })
        .count();
    let movement_corrections_awaiting_receipt_ack = state
        .clients
        .values()
        .filter(|client| {
            client
                .pending_movement_correction
                .as_ref()
                .is_some_and(|transaction| transaction.terminal_receipt().is_some())
        })
        .count();
    let objective_replay = state.objectives.replay();
    let client_debug_trace = state.config.client_debug_trace;
    let server_version = env!("ZUBSPACE_SERVER_BUILD_COMMIT");
    let uptime = started_at.elapsed().as_secs();
    let mut status = serde_json::Map::new();
    status.insert("ok".into(), serde_json::json!(true));
    status.insert("service".into(), serde_json::json!("zubspace-server"));
    status.insert("version".into(), serde_json::json!(server_version));
    status.insert("uptime".into(), serde_json::json!(uptime));
    status.insert(
        "tick_rate".into(),
        serde_json::json!(state.config.tick_rate_hz),
    );
    status.insert(
        "client_debug_trace".into(),
        serde_json::json!(client_debug_trace),
    );
    status.insert(
        "observer_network_profile".into(),
        serde_json::json!(state.config.observer_network_profile.as_str()),
    );
    status.insert("clients".into(), serde_json::json!(state.clients.len()));
    status.insert(
        "objective_profile_disabled".into(),
        serde_json::json!(state.objectives.profile().is_disabled()),
    );
    status.insert(
        "objective_flags".into(),
        serde_json::json!(objective_replay.flags.len()),
    );
    status.insert(
        "objective_powerballs".into(),
        serde_json::json!(objective_replay.powerballs.len()),
    );
    status.insert("rtc_sessions".into(), serde_json::json!(state.rtc_sessions));
    status.insert(
        "rtc_capacity".into(),
        serde_json::json!(state.config.max_players),
    );
    status.insert(
        "rtc_effective_capacity".into(),
        serde_json::json!(state.rtc_effective_capacity()),
    );
    status.insert(
        "rtc_udp_port_capacity".into(),
        serde_json::json!(
            state
                .config
                .rtc_udp_port_range
                .map(|range| range.capacity())
        ),
    );
    status.insert(
        "rtc_udp_ports_in_use".into(),
        serde_json::json!(state.rtc_udp_ports_in_use.len()),
    );
    status.insert(
        "rtc_udp_ports_degraded".into(),
        serde_json::json!(state.rtc_udp_quarantined_ports.len()),
    );
    status.insert(
        "rtc_udp_degraded_ports".into(),
        serde_json::json!(
            state
                .rtc_udp_quarantined_ports
                .iter()
                .copied()
                .collect::<Vec<_>>()
        ),
    );
    status.insert(
        "snapshot_acked_clients".into(),
        serde_json::json!(snapshot_acked_clients),
    );
    status.insert(
        "threat_contracts_current".into(),
        serde_json::json!(threat_contracts_current),
    );
    status.insert(
        "threat_contract_capacity_per_recipient".into(),
        serde_json::json!(crate::threat_presentation::MAX_THREAT_SOURCE_LIVES_PER_RECIPIENT),
    );
    status.insert(
        "threat_contract_capacity_evictions".into(),
        serde_json::json!(threat_contract_capacity_evictions),
    );
    status.insert(
        "threat_contract_stale_runtime_evictions".into(),
        serde_json::json!(threat_contract_stale_runtime_evictions),
    );
    status.insert(
        "projectile_threat_contracts_current".into(),
        serde_json::json!(projectile_threat_contracts_current),
    );
    status.insert(
        "projectile_threat_contracts_peak".into(),
        serde_json::json!(projectile_threat_contracts_peak),
    );
    status.insert(
        "projectile_threat_contract_capacity_per_recipient".into(),
        serde_json::json!(crate::threat_presentation::MAX_PROJECTILE_THREATS_PER_RECIPIENT),
    );
    status.insert(
        "projectile_threat_contract_payload_bytes".into(),
        serde_json::json!(projectile_threat_contract_payload_bytes),
    );
    status.insert(
        "projectile_threat_contract_payload_capacity_per_recipient".into(),
        serde_json::json!(
            crate::threat_presentation::MAX_PROJECTILE_PRESENTATION_PAYLOAD_BYTES_PER_RECIPIENT
        ),
    );
    status.insert(
        "projectile_threat_contract_payload_capacity_200_players".into(),
        serde_json::json!(
            crate::threat_presentation::MAX_PROJECTILE_PRESENTATION_PAYLOAD_BYTES_AT_200_PLAYERS
        ),
    );
    status.insert(
        "projectile_threat_contract_capacity_rejections".into(),
        serde_json::json!(projectile_threat_contract_capacity_rejections),
    );
    status.insert(
        "projectile_threat_contract_terminal_cleanups".into(),
        serde_json::json!(projectile_threat_contract_terminal_cleanups),
    );
    status.insert(
        "threat_harmful_readiness_deferrals_total".into(),
        serde_json::json!(state.active_projectiles.harmful_readiness_deferrals_total()),
    );
    for (name, total) in [
        (
            "damage_readiness_waits_total",
            state.damage_readiness_waits_total,
        ),
        (
            "damage_conflict_waits_total",
            state.damage_conflict_waits_total,
        ),
        (
            "damage_independent_commits_total",
            state.damage_independent_commits_total,
        ),
    ] {
        status.insert(name.into(), serde_json::json!(total));
    }
    status.insert(
        "collision_witnesses_total".into(),
        serde_json::json!(state.collision_witnesses_total),
    );
    status.insert(
        "movement_snapbacks_total".into(),
        serde_json::json!(state.movement_snapbacks_total),
    );
    status.insert(
        "movement_corrections_awaiting_application".into(),
        serde_json::json!(movement_corrections_awaiting_application),
    );
    status.insert(
        "movement_corrections_awaiting_receipt_ack".into(),
        serde_json::json!(movement_corrections_awaiting_receipt_ack),
    );
    status.insert(
        "repel_queue_packets".into(),
        serde_json::json!(repel_queue_packets),
    );
    status.insert(
        "repel_queue_max_target_depth".into(),
        serde_json::json!(repel_queue_max_target_depth),
    );
    status.insert(
        "repel_queue_awaiting_application".into(),
        serde_json::json!(repel_queue_awaiting_application),
    );
    status.insert(
        "repel_queue_awaiting_receipt_ack".into(),
        serde_json::json!(repel_queue_awaiting_receipt_ack),
    );
    status.insert(
        "repel_queue_overflows".into(),
        serde_json::json!(state.repel_queue_overflows),
    );
    status.insert(
        "warp_pickup_acks_deferred".into(),
        serde_json::json!(warp_pickup_acks_deferred),
    );
    status.insert(
        "position_input_queue_capacity_rejections".into(),
        serde_json::json!(state.pending_position_inputs.capacity_rejections()),
    );
    status.insert(
        "realtime_event_queue_capacity_per_client".into(),
        serde_json::json!(crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY),
    );
    status.insert(
        "realtime_event_queue_current".into(),
        serde_json::json!(state.realtime_event_queue_current()),
    );
    status.insert(
        "realtime_event_queue_peak".into(),
        serde_json::json!(state.realtime_event_queue_peak),
    );
    status.insert(
        "realtime_event_queue_overflows".into(),
        serde_json::json!(state.realtime_event_queue_overflows),
    );
    status.insert(
        "realtime_event_actions_rejected".into(),
        serde_json::json!(state.realtime_event_actions_rejected),
    );
    status.insert(
        "realtime_event_actions_deferred".into(),
        serde_json::json!(state.realtime_event_actions_deferred),
    );
    status.insert(
        "projectile_pending_hit_capacity_total".into(),
        serde_json::json!(crate::authority_projectiles::MAX_PENDING_HITS_TOTAL),
    );
    status.insert(
        "projectile_pending_hit_capacity_per_owner".into(),
        serde_json::json!(crate::authority_projectiles::MAX_PENDING_HITS_PER_OWNER),
    );
    status.insert(
        "projectile_pending_hit_retry_budget_per_tick".into(),
        serde_json::json!(crate::authority_projectiles::MAX_PENDING_HIT_RETRIES_PER_TICK),
    );
    status.insert(
        "projectile_pending_hit_current".into(),
        serde_json::json!(state.active_projectiles.pending_hit_count()),
    );
    status.insert(
        "projectile_pending_hit_retained_bytes".into(),
        serde_json::json!(state.active_projectiles.pending_hit_retained_bytes()),
    );
    status.insert(
        "projectile_pending_damage_transactions".into(),
        serde_json::json!(state.pending_damage_transactions.len()),
    );
    status.insert(
        "projectile_pending_damage_payload_bytes".into(),
        serde_json::json!(state.pending_damage_transaction_payload_bytes()),
    );
    status.insert(
        "rtc_outbound_queue_packets".into(),
        serde_json::json!(rtc_overload.queue_packets),
    );
    status.insert(
        "rtc_outbound_queue_bytes".into(),
        serde_json::json!(rtc_overload.queue_bytes),
    );
    status.insert(
        "rtc_outbound_peak_queue_packets".into(),
        serde_json::json!(rtc_overload.peak_queue_packets),
    );
    status.insert(
        "rtc_outbound_peak_queue_bytes".into(),
        serde_json::json!(rtc_overload.peak_queue_bytes),
    );
    status.insert(
        "rtc_outbound_realtime_coalesced".into(),
        serde_json::json!(rtc_overload.realtime_coalesced),
    );
    status.insert(
        "rtc_outbound_reliable_overflows".into(),
        serde_json::json!(rtc_overload.reliable_overflows),
    );
    status.insert(
        "rtc_channel_protocol_violations".into(),
        serde_json::json!(rtc_overload.protocol_violations),
    );
    status.insert(
        "rtc_inbound_queue_capacity_packets".into(),
        serde_json::json!(crate::rtc::RTC_APPLICATION_INBOUND_QUEUE_PACKETS),
    );
    status.insert("rtc_inbound_packets".into(), serde_json::json!({"game":rtc_overload.inbound_packets[0],"event":rtc_overload.inbound_packets[1],"reliable":rtc_overload.inbound_packets[2]}));
    status.insert("rtc_inbound_queue_packets".into(), serde_json::json!({"game":rtc_overload.inbound_queue_packets[0],"event":rtc_overload.inbound_queue_packets[1],"reliable":rtc_overload.inbound_queue_packets[2]}));
    status.insert("rtc_inbound_peak_queue_packets".into(), serde_json::json!({"game":rtc_overload.inbound_peak_queue_packets[0],"event":rtc_overload.inbound_peak_queue_packets[1],"reliable":rtc_overload.inbound_peak_queue_packets[2]}));
    status.insert("rtc_inbound_backpressure".into(), serde_json::json!({"game":rtc_overload.inbound_backpressure[0],"event":rtc_overload.inbound_backpressure[1],"reliable":rtc_overload.inbound_backpressure[2]}));
    status.insert(
        "rtc_data_channel_send_buffer_bytes".into(),
        serde_json::json!(crate::rtc::RTC_DATA_CHANNEL_SEND_BUFFER_BYTES),
    );
    let presentation_geometry = state.active_projectiles.presentation_geometry_metrics();
    status.insert(
        "presentation_geometry".into(),
        serde_json::json!({
            "lookups_total": presentation_geometry.lookups_total,
            "resolved_total": presentation_geometry.resolved_total,
            "unavailable_total": presentation_geometry.unavailable_total,
            "server_native_total": presentation_geometry.server_native_total,
            "local_self_total": presentation_geometry.local_self_total,
            "server_advanced_total": presentation_geometry.server_advanced_total,
            "eval_nanos_total": presentation_geometry.eval_nanos_total,
            "eval_nanos_max": presentation_geometry.eval_nanos_max,
        }),
    );
    status.insert(
        "reliable_gameplay".into(),
        serde_json::json!({
            "queued": state.pending_reliable_gameplay.queued_len(),
            "head_visits": state.pending_reliable_gameplay.head_visits(),
            "ready_runtimes": state.pending_reliable_gameplay.ready_len(),
        }),
    );
    status.insert(
        "tick_timing".into(),
        serde_json::json!({
            "scope": "server-lifetime",
            "interval": state.tick_intervals.summary(),
            "absolute_jitter": state.tick_jitter.summary(),
            "work": state.tick_work.summary(),
        }),
    );
    let (limit, in_flight, waiting) = state
        .rtc_ingress_executor
        .get()
        .map(|executor| executor.status())
        .unwrap_or((0, 0, 0));
    status.insert("rtc_ingress_execution".into(), serde_json::json!({
        "limit": limit, "in_flight": in_flight, "waiting": waiting,
        "workers": state.rtc_ingress_executor.get().map(|executor| executor.worker_count()).unwrap_or(0),
    }));
    status.insert(
        "connection_quality".into(),
        crate::connection_quality::summary(&state),
    );
    status
}

pub(crate) fn server_zone_catalog_json(zone_maps: &[crate::zone::ZoneMap]) -> String {
    let zones = zone_maps
        .iter()
        .map(|zone| {
            format!(
                r#"{{"id":"{}","display_name":"{}","map":"{}"}}"#,
                json_escape(zone.zone_id.as_ref()),
                json_escape(zone.display_name.as_ref()),
                json_escape(zone.filename.as_ref()),
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"ok":true,"zones":[{zones}]}}"#)
}

fn http_request_target(request: &str) -> Option<(&str, &str)> {
    let mut parts = request.lines().next()?.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?.split('?').next()?;
    Some((method, path))
}

fn http_content_length(request: &str) -> io::Result<usize> {
    for line in request.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse::<usize>().map_err(|err| {
                io::Error::new(
                    ErrorKind::InvalidData,
                    format!("invalid Content-Length header: {err}"),
                )
            });
        }
    }
    Ok(0)
}

fn write_public_response(stream: &mut TcpStream, content_type: &str, body: &str) -> io::Result<()> {
    write_http_response(
        stream,
        "200 OK",
        content_type,
        body,
        &[
            ("Access-Control-Allow-Origin", "*"),
            ("Cache-Control", "no-store"),
        ],
    )
}

fn write_http_json(stream: &mut TcpStream, status: &str, body: &str) -> io::Result<()> {
    write_http_response(
        stream,
        status,
        "application/json",
        body,
        &[("Access-Control-Allow-Origin", "*")],
    )
}

fn write_http_json_with_origin(
    stream: &mut TcpStream,
    status: &str,
    body: &str,
    origin: &str,
) -> io::Result<()> {
    write_http_response(
        stream,
        status,
        "application/json",
        body,
        &[("Access-Control-Allow-Origin", origin), ("Vary", "Origin")],
    )
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
    extra_headers: &[(&str, &str)],
) -> io::Result<()> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n",
        body.len()
    );
    // Every response is live state or a one-off answer, so no browser, proxy
    // or CDN may store it. Callers that already set Cache-Control keep theirs.
    if !extra_headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("cache-control"))
    {
        response.push_str("Cache-Control: no-store\r\n");
    }
    for (name, value) in extra_headers {
        response.push_str(name);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    response.push_str(body);
    stream.write_all(response.as_bytes())
}

/// Wire type byte for client input records. Must match the constant in
/// `src/protocol/client_input_record.rs`. The server keeps a local copy
/// because that module isn't pulled into the server binary's `#[path]` tree.
pub(crate) const CLIENT_INPUT_RECORD_TYPE: u8 = 0x62;
const CLIENT_INPUT_RECORD_HEADER_LEN: usize = 4;
const CLIENT_INPUT_RECORD_ENTRY_LEN: usize =
    4 + crate::combat_timeline::CombatViewReceiptV1::ENCODED_LEN;
/// Exact wire cap shared with `ClientInputRecordPacket::MAX_INPUTS`. Server
/// retention is wider because overlapping 32-entry packets reconstruct a
/// longer bounded history; a single packet may never claim that larger cap.
pub(crate) const CLIENT_INPUT_RECORD_MAX_INPUTS: usize = 32;
const CLIENT_INPUT_RECORD_VALID_BITS: u16 = 0x007f;
const CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT: u16 =
    crate::gameplay_rules::ROCKET_ACTIVATION_PULSE_BIT;
#[cfg(test)]
const CLIENT_INPUT_ROCKET_ACTIVE_BIT: u16 = CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParsedClientInputObservation {
    sequence: u16,
    input: crate::state::ClientInputObservation,
    door_seed: u8,
    combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputDoorPhaseResolutionError {
    /// The claimed phase could affect current/future authority but is not a
    /// valid authenticated phase for that logical input.
    Security(&'static str),
    /// An already-admitted upper input proves this is only a stale predecessor,
    /// but the exact historical phase has aged beyond the bounded recovery
    /// window. The packet remains inert; lack of retained evidence is not
    /// evidence that an honest browser forged the one-byte seed.
    HistoricalPredecessorUnavailable,
}

/// The RTC reader and arena tick task can observe opposite sides of one 10 ms
/// boundary. Admit only the authenticated immediately-next door phase across
/// that scheduling seam; position replay keeps the packet queued until the
/// phase's exact start tick. Wider future claims remain security violations.
const INPUT_DOOR_PHASE_SCHEDULING_GRACE_TICKS: u32 = 1;

fn retained_input_door_phase_is_plausible(
    client: &crate::state::ClientHandle,
    record: &crate::state::SequencedInputObservation,
    server_tick: u32,
) -> bool {
    if client.door_state.is_none() {
        return true;
    }
    if client.last_input_seq == Some(record.sequence)
        || client.last_position_sequence == Some(record.sequence)
    {
        // The newest admitted input and the exact input named by the last
        // canonical position are causal boundaries for queued recovery even
        // when an arena-thread hitch makes their phase older than the ordinary
        // rolling-history horizon. Raw-sequence aliasing only affects
        // predecessor entries that re-enter the u16 window after a
        // wrap/catch-up jump; never discard either exact committed witness.
        return true;
    }
    if client.life_position_sequence_base == Some(record.sequence)
        && client.last_input_seq.is_some_and(|newest| {
            let progress = newest.wrapping_sub(record.sequence);
            progress < (1 << 15)
                && (progress as usize) < crate::state::MAX_RECENT_INPUT_OBSERVATIONS
        })
    {
        // The application-reliable neutral life baseline is a server-reserved,
        // canonically phased lower witness for the complete bounded input
        // window that follows it. A browser render stall can advance the
        // synchronized door clock across several authored phases between two
        // adjacent logical inputs; receipt-age pruning must not erase this
        // exact life anchor while those inputs are still retained. The raw
        // sequence is not trusted after it leaves the 64-entry life window,
        // preventing a later u16 alias from reopening the old phase.
        return true;
    }
    let phase_age = crate::continuum::continuum_tick_diff(server_tick, record.door_phase_tick);
    let maximum_age = crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS
        .saturating_add(crate::arena_settings::arena_door_delay_ticks().max(1) as u32)
        .saturating_add(retained_input_history_ticks());
    phase_age >= -(INPUT_DOOR_PHASE_SCHEDULING_GRACE_TICKS as i32)
        && (phase_age < 0 || phase_age as u32 <= maximum_age)
}

fn retained_input_history_ticks() -> u32 {
    (crate::state::MAX_RECENT_INPUT_OBSERVATIONS as u32 * 5).div_ceil(3)
}

/// Canonical door phases a browser can have reached from the newest snapshot
/// it demonstrably acknowledged. The client advances its synchronized door
/// clock for at most one second after the last forward authority sample and
/// then freezes. Reconstructing that exact challenge-derived interval closes
/// a server-thread hitch without accepting an arbitrary seed from the gap
/// between the frozen browser clock and the later receipt-time window.
fn acknowledged_snapshot_door_phases(
    client: &crate::state::ClientHandle,
    server_tick: u32,
) -> Vec<crate::doors::ClientDoorPhase> {
    let Some(acked_snapshot_id) = client.last_acked_snapshot_id else {
        return Vec::new();
    };
    let Some(acked_tick) = client
        .snapshot_history
        .iter()
        .find(|entry| entry.snapshot_id == acked_snapshot_id)
        .map(|entry| entry.server_tick)
    else {
        return Vec::new();
    };
    let distance_to_receipt = crate::continuum::continuum_tick_diff(server_tick, acked_tick);
    if distance_to_receipt < 0 {
        return Vec::new();
    }
    let horizon_tick = crate::continuum::continuum_tick_add(
        acked_tick,
        (distance_to_receipt as u32).min(crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS),
    );
    let current_phase_at = |tick| {
        crate::doors::client_door_phase_candidates_at_tick(client.door_state, tick)
            .into_iter()
            .filter(|phase| crate::continuum::continuum_tick_diff(tick, phase.started_tick) >= 0)
            .max_by_key(|phase| crate::continuum::continuum_tick_diff(phase.started_tick, tick))
    };
    let (Some(start), Some(through)) =
        (current_phase_at(acked_tick), current_phase_at(horizon_tick))
    else {
        return Vec::new();
    };
    crate::doors::client_door_phase_sequence(
        client.door_state,
        start.started_tick,
        through.started_tick,
    )
    .unwrap_or_default()
}

fn parse_client_input_observations(
    packet: &[u8],
) -> Result<Vec<ParsedClientInputObservation>, &'static str> {
    if packet.len() < CLIENT_INPUT_RECORD_HEADER_LEN {
        return Err("header_too_short");
    }
    if packet[0] != CLIENT_INPUT_RECORD_TYPE {
        return Err("invalid_type");
    }
    let base_seq = u16::from_le_bytes([packet[1], packet[2]]);
    let count = packet[3] as usize;
    if count == 0 {
        return Err("empty");
    }
    if count > CLIENT_INPUT_RECORD_MAX_INPUTS {
        return Err("count_above_max");
    }
    let expected_len = CLIENT_INPUT_RECORD_HEADER_LEN + count * CLIENT_INPUT_RECORD_ENTRY_LEN;
    if packet.len() != expected_len {
        return Err("length_mismatch");
    }
    let mut observations = Vec::with_capacity(count);
    for index in 0..count {
        let off = CLIENT_INPUT_RECORD_HEADER_LEN + index * CLIENT_INPUT_RECORD_ENTRY_LEN;
        let bits = u16::from_le_bytes([packet[off], packet[off + 1]]);
        if bits & !CLIENT_INPUT_RECORD_VALID_BITS != 0 {
            return Err("unknown_bits");
        }
        if packet[off + 2] >= 40 {
            return Err("invalid_direction");
        }
        let sequence = base_seq.wrapping_add(index as u16);
        let combat_view_receipt = crate::combat_timeline::CombatViewReceiptV1::decode(
            &packet[off + 4..off + CLIENT_INPUT_RECORD_ENTRY_LEN],
        )
        .map_err(|_| "invalid_combat_view_receipt")?;
        if combat_view_receipt.input_sequence != sequence {
            return Err("combat_view_sequence_mismatch");
        }
        observations.push(ParsedClientInputObservation {
            sequence,
            input: crate::state::ClientInputObservation {
                bits,
                direction: packet[off + 2],
            },
            door_seed: packet[off + 3],
            combat_view_receipt,
        });
    }
    Ok(observations)
}

fn resolve_client_input_door_phases(
    client: &crate::state::ClientHandle,
    observations: &[ParsedClientInputObservation],
    newest: u16,
    server_tick: u32,
) -> Result<Vec<crate::state::SequencedInputObservation>, InputDoorPhaseResolutionError> {
    // `Game::update_doors` advances the authenticated door clock once per
    // render frame before the bounded fixed-step catch-up batch. Therefore a
    // scheduler hitch legitimately stamps every recovered logical input in
    // that batch with the door phase current at receipt time, even when the
    // oldest input sequence nominally represents an earlier 60 Hz slot.
    // Sequence age is not door-time provenance. The challenge-derived phase
    // must instead have started by this server receipt tick, apart from the
    // one-tick scheduling seam that position replay explicitly defers until
    // the exact phase boundary. Retained lower/upper sequence records still
    // enforce monotonic phase ordering.
    let receipt_phase_candidates =
        crate::doors::client_door_phase_candidates_at_tick(client.door_state, server_tick);
    let mut resolved = Vec::with_capacity(observations.len());

    for observation in observations {
        if let Some(stored) = client.recent_input_records.iter().find(|record| {
            record.sequence == observation.sequence
                && retained_input_door_phase_is_plausible(client, record, server_tick)
        }) {
            if stored.door_seed != observation.door_seed {
                return Err(InputDoorPhaseResolutionError::Security("sequence_rewrite"));
            }
            resolved.push(*stored);
            continue;
        }

        let lower_record = client
            .recent_input_records
            .iter()
            .filter(|record| retained_input_door_phase_is_plausible(client, record, server_tick))
            .chain(resolved.iter())
            .filter_map(|record| {
                let distance = observation.sequence.wrapping_sub(record.sequence);
                (distance > 0 && distance < (1 << 15)).then_some((distance, *record))
            })
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, record)| record);
        let upper_record = client
            .recent_input_records
            .iter()
            .filter(|record| retained_input_door_phase_is_plausible(client, record, server_tick))
            .chain(resolved.iter())
            .filter_map(|record| {
                let distance = record.sequence.wrapping_sub(observation.sequence);
                (distance > 0 && distance < (1 << 15)).then_some((distance, *record))
            })
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, record)| record);
        // A committed correction clears rolling input redundancy but retains
        // its exact canonical witness. That witness remains an authority
        // boundary, not another input record: later compact seed bytes cannot
        // resolve to an earlier phase merely because the input ring is empty.
        let canonical_lower = client
            .canonical_position_witnesses
            .iter()
            .filter(|witness| witness.life_seq == client.life_seq)
            .filter_map(|witness| {
                let distance = observation.sequence.wrapping_sub(witness.sequence);
                (distance > 0 && distance < (1 << 15)).then_some((distance, witness))
            })
            .min_by_key(|(distance, _)| *distance)
            .map(|(_, witness)| crate::doors::ClientDoorPhase {
                seed: witness.door_seed,
                started_tick: witness.door_phase_tick,
            });
        let lower_phase = lower_record
            .map(|record| record.door_phase_tick)
            .into_iter()
            .chain(canonical_lower.map(|phase| phase.started_tick))
            .max_by_key(|tick| crate::continuum::continuum_tick_diff(*tick, server_tick));
        let upper_phase = upper_record.map(|record| record.door_phase_tick);

        // The RTC reader may receive a packet while the arena's exclusive
        // server step is still running. Dispatch then waits behind that step,
        // so the receipt-time current phase window can be several phases newer
        // than this exact input. Recover only the deterministic forward range
        // reachable from the nearest authenticated lower sequence within the
        // same one-second bound used by the browser door clock. This closes
        // the queueing seam without allowing arbitrary historical/open-door
        // selection: every candidate is challenge-derived, monotonic, and no
        // more than the client's fail-closed extrapolation horizon ahead of
        // the already admitted lower record.
        let mut phase_candidates = receipt_phase_candidates.clone();
        phase_candidates.extend(canonical_lower);
        phase_candidates.extend(acknowledged_snapshot_door_phases(client, server_tick));
        if let Some(upper) = upper_record {
            // A rolling unordered packet can arrive after a later sequence
            // from the same logical history.  The already admitted upper
            // witness is the strongest conservative phase available for an
            // older record carrying the same authenticated seed: binding the
            // older record to it cannot advance door geometry or cross the
            // upper ordering boundary.  This is essential when a slow render
            // batch stamped many recovered inputs with one phase and the
            // upper record has aged outside the receipt-time candidate set.
            phase_candidates.push(crate::doors::ClientDoorPhase {
                seed: upper.door_seed,
                started_tick: upper.door_phase_tick,
            });

            // The symmetric recovery case has no retained lower witness: a
            // later record was already admitted, then an unordered rolling
            // packet restores predecessors from the browser's bounded input
            // history. A render-thread stall can leave that upper record one
            // or more door phases behind receipt time, so receipt candidates
            // plus the upper phase alone omit an honest immediately preceding
            // phase. Reconstruct only the authenticated predecessor interval
            // that the 64-record history can span, plus one full phase because
            // records name a phase start rather than their sample tick. The
            // upper sequence remains a hard monotonic ceiling.
            let predecessor_ticks = retained_input_history_ticks()
                .saturating_add(crate::arena_settings::arena_door_delay_ticks().max(1) as u32);
            let predecessor_bound =
                crate::continuum::continuum_tick_sub(upper.door_phase_tick, predecessor_ticks);
            let predecessor_start = crate::doors::client_door_phase_candidates_at_tick(
                client.door_state,
                predecessor_bound,
            )
            .into_iter()
            .filter(|phase| {
                crate::continuum::continuum_tick_diff(predecessor_bound, phase.started_tick) >= 0
            })
            .max_by_key(|phase| {
                crate::continuum::continuum_tick_diff(phase.started_tick, predecessor_bound)
            });
            if let Some(predecessor_start) = predecessor_start
                && let Some(predecessors) = crate::doors::client_door_phase_sequence(
                    client.door_state,
                    predecessor_start.started_tick,
                    upper.door_phase_tick,
                )
            {
                phase_candidates.extend(predecessors);
            }
        }
        if let Some(lower) = lower_phase {
            let distance_to_receipt =
                crate::continuum::continuum_tick_diff(server_tick, lower).max(0) as u32;
            // The lower record names the phase start, not the tick within
            // that phase at which it was sampled. Include one complete
            // authored phase plus the browser's extrapolation horizon.
            let recovery_ticks = crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS
                .saturating_add(crate::arena_settings::arena_door_delay_ticks().max(1) as u32);
            let bounded_distance = distance_to_receipt.min(recovery_ticks);
            let bounded_through = crate::continuum::continuum_tick_add(lower, bounded_distance);
            let through_phase = crate::doors::client_door_phase_candidates_at_tick(
                client.door_state,
                bounded_through,
            )
            .into_iter()
            .filter(|phase| {
                crate::continuum::continuum_tick_diff(phase.started_tick, lower) >= 0
                    && crate::continuum::continuum_tick_diff(bounded_through, phase.started_tick)
                        >= 0
            })
            .max_by_key(|phase| crate::continuum::continuum_tick_diff(phase.started_tick, lower));
            if let Some(through_phase) = through_phase
                && let Some(recovered) = crate::doors::client_door_phase_sequence(
                    client.door_state,
                    lower,
                    through_phase.started_tick,
                )
            {
                phase_candidates.extend(recovered);
                phase_candidates.sort_by_key(|phase| {
                    crate::continuum::continuum_tick_diff(phase.started_tick, lower)
                });
                phase_candidates.dedup_by_key(|phase| phase.started_tick);
            }
        }

        let age = newest.wrapping_sub(observation.sequence) as u32;
        let expected_tick =
            crate::continuum::continuum_tick_sub(server_tick, age.saturating_mul(5) / 3);
        let mut latest_plausible_phase_tick = crate::continuum::continuum_tick_add(
            server_tick,
            INPUT_DOOR_PHASE_SCHEDULING_GRACE_TICKS,
        );
        if let Some(lower) = lower_phase
            && crate::continuum::continuum_tick_diff(lower, latest_plausible_phase_tick) > 0
        {
            // A previously admitted lower sequence is stronger ordering
            // evidence than the receipt-time slot estimate for a recovered
            // record. Under loss/reordering, the client's 60 Hz sequence age
            // can place that estimate just before a door transition already
            // proven by the lower record. Permit the same proven phase here;
            // the first record that established it still had to pass the
            // server-clock future-phase check, so this cannot bootstrap an
            // unstarted phase.
            latest_plausible_phase_tick = lower;
        }

        let mut matching = phase_candidates
            .iter()
            .copied()
            .filter(|phase| phase.seed == observation.door_seed)
            .filter(|phase| {
                lower_phase.is_none_or(|lower| {
                    crate::continuum::continuum_tick_diff(phase.started_tick, lower) >= 0
                }) && upper_phase.is_none_or(|upper| {
                    crate::continuum::continuum_tick_diff(upper, phase.started_tick) >= 0
                })
            })
            .collect::<Vec<_>>();
        let had_monotonic_matching_phase = !matching.is_empty();
        matching.retain(|phase| {
            crate::continuum::continuum_tick_diff(phase.started_tick, latest_plausible_phase_tick)
                <= 0
        });

        // Unit fixtures without a security challenge retain the legacy zero
        // byte. Production clients always have a door state before gameplay.
        if matching.is_empty() && client.door_state.is_none() && observation.door_seed == 0 {
            matching.push(crate::doors::ClientDoorPhase {
                seed: 0,
                started_tick: lower_phase.or(upper_phase).unwrap_or(server_tick),
            });
        }
        if matching.is_empty() {
            let raw_is_in_window = phase_candidates
                .iter()
                .any(|phase| phase.seed == observation.door_seed);
            debug!(
                player_id = client.player.id,
                input_sequence = observation.sequence,
                newest_input_sequence = newest,
                input_door_seed = observation.door_seed,
                server_tick,
                lower_phase_tick = ?lower_phase,
                upper_phase_tick = ?upper_phase,
                expected_tick,
                latest_plausible_phase_tick,
                phase_candidates = ?phase_candidates,
                raw_is_in_window,
                "input door phase resolution rejected"
            );
            return Err(if had_monotonic_matching_phase {
                InputDoorPhaseResolutionError::Security("door_phase_future")
            } else if upper_record.is_some() {
                // The server has already admitted a strictly newer logical
                // input, so this record cannot advance movement, weapon, or
                // collision authority. A browser scheduler pause can leave a
                // genuine predecessor in the rolling 32-record bundle for
                // several wall-clock door phases even though its sequence is
                // nearby. Once the exact phase is outside every retained,
                // challenge-derived recovery interval, discard the whole
                // packet as unavailable instead of calling the client
                // malicious. A current/new record has no upper witness and
                // still fails as `door_seed_out_of_window` below.
                InputDoorPhaseResolutionError::HistoricalPredecessorUnavailable
            } else if raw_is_in_window {
                InputDoorPhaseResolutionError::Security("door_phase_backtrack")
            } else {
                InputDoorPhaseResolutionError::Security("door_seed_out_of_window")
            });
        }

        // A door seed is only one byte and the authenticated RNG can repeat
        // it. Once a lower sequence has established a phase, retain the
        // earliest monotonic matching occurrence. Receipt time is not allowed
        // to jump an adjacent delayed input over intervening phases merely
        // because a later occurrence of the same byte is closer to `now`.
        // That exact ambiguity occurs when the arena thread stalls: an old
        // `255` can drain while the challenge stream has already reached a
        // later `255`, and greedily choosing the latter makes the following
        // honest `226` look like a backtrack. Both occurrences describe the
        // same door geometry, so the earliest authenticated occurrence is the
        // conservative assignment; a later distinct seed advances it normally.
        // Without a lower witness, retain the receipt-slot rule needed for the
        // first admitted record.
        let phase = if let Some(lower) = lower_phase {
            matching
                .into_iter()
                .min_by_key(|phase| {
                    crate::continuum::continuum_tick_diff(phase.started_tick, lower).unsigned_abs()
                })
                .expect("matching door phase is nonempty")
        } else {
            matching
                .into_iter()
                .min_by_key(|phase| {
                    let offset =
                        crate::continuum::continuum_tick_diff(phase.started_tick, expected_tick);
                    (offset > 0, offset.unsigned_abs())
                })
                .expect("matching door phase is nonempty")
        };
        resolved.push(crate::state::SequencedInputObservation {
            sequence: observation.sequence,
            input: observation.input,
            door_seed: observation.door_seed,
            door_phase_tick: phase.started_tick,
        });
    }
    Ok(resolved)
}

/// Re-resolve the neutral boundary and any already-admitted forward inputs as
/// one new-life sequence. Realtime N+1 may reach ingress while reliable N is
/// waiting for the server tick; resetting the life must not erase that
/// independently admitted record.
pub(crate) fn rebase_input_door_phases_for_life_boundary(
    client: &crate::state::ClientHandle,
    boundary_sequence: u16,
    server_tick: u32,
) -> Option<Vec<crate::state::SequencedInputObservation>> {
    let mut records = client
        .recent_input_records
        .iter()
        .copied()
        .filter(|record| {
            let distance = record.sequence.wrapping_sub(boundary_sequence);
            distance < (1 << 15)
                && (distance as usize) < crate::state::MAX_RECENT_INPUT_OBSERVATIONS
        })
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record.sequence.wrapping_sub(boundary_sequence));
    if records.first().map(|record| record.sequence) != Some(boundary_sequence) {
        return None;
    }
    let newest = records.last()?.sequence;
    let observations = records
        .into_iter()
        .map(|record| ParsedClientInputObservation {
            sequence: record.sequence,
            input: record.input,
            door_seed: record.door_seed,
            combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(
                record.sequence,
            ),
        })
        .collect::<Vec<_>>();
    let mut boundary_client = client.clone();
    boundary_client.last_input_seq = None;
    boundary_client.last_input_record = None;
    boundary_client.recent_input_records.clear();
    resolve_client_input_door_phases(&boundary_client, &observations, newest, server_tick).ok()
}

/// Phase 1b: parse a client-input packet, retain every new sequence in the
/// bounded observation history, and expose its newest entry on the client
/// handle. Errors are logged via the security reject channel (mirroring the
/// existing observation path) so malformed input stays visible without
/// crashing the connection.
fn record_client_input_observation(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    packet: &[u8],
) -> bool {
    record_client_input_observation_with_policy_at_tick(
        state,
        identity,
        packet,
        crate::wire::current_tick(),
        ClientInputObservationPolicy::Ordinary,
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn record_client_input_observation_at_tick(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    packet: &[u8],
    server_tick: u32,
) -> bool {
    record_client_input_observation_with_policy_at_tick(
        state,
        identity,
        packet,
        server_tick,
        ClientInputObservationPolicy::Ordinary,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClientInputObservationPolicy {
    Ordinary,
    CanonicalNeutralLifecycleBaseline,
    AuthenticatedMotionLifecycleBaseline,
}

impl ClientInputObservationPolicy {
    fn is_authenticated_lifecycle_baseline(self) -> bool {
        self != Self::Ordinary
    }

    fn canonicalizes_neutral_boundary(self) -> bool {
        self == Self::CanonicalNeutralLifecycleBaseline
    }
}

/// Test-facing equivalent of the production reliable lifecycle-baseline
/// validation immediately above the dead-runtime ingress exception.
#[cfg(test)]
pub(crate) fn record_client_lifecycle_input_baseline_at_tick(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    packet: &[u8],
    server_tick: u32,
) -> bool {
    let Ok(observations) = parse_client_input_observations(packet) else {
        return false;
    };
    if observations.len() != 1
        || observations[0].input
            != (crate::state::ClientInputObservation {
                bits: 0,
                direction: 0,
            })
    {
        return false;
    }
    record_client_input_observation_with_policy_at_tick(
        state,
        identity,
        packet,
        server_tick,
        ClientInputObservationPolicy::CanonicalNeutralLifecycleBaseline,
    )
}

/// Test-facing equivalent of the production reliable motion-preserving
/// frequency baseline after its full position payload has been validated.
#[cfg(test)]
pub(crate) fn record_client_frequency_input_baseline_at_tick(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    packet: &[u8],
    server_tick: u32,
) -> bool {
    let Ok(observations) = parse_client_input_observations(packet) else {
        return false;
    };
    if observations.len() != 1
        || observations[0].input.bits & CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT != 0
    {
        return false;
    }
    record_client_input_observation_with_policy_at_tick(
        state,
        identity,
        packet,
        server_tick,
        ClientInputObservationPolicy::AuthenticatedMotionLifecycleBaseline,
    )
}

fn record_client_input_observation_with_policy_at_tick(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerRuntimeIdentity,
    packet: &[u8],
    server_tick: u32,
    policy: ClientInputObservationPolicy,
) -> bool {
    let mut state = state.lock().expect("server state should not be poisoned");
    record_client_input_observation_in_state_with_policy_at_tick(
        &mut state,
        identity,
        packet,
        server_tick,
        policy,
    )
}

pub(crate) fn record_client_input_observation_in_state_at_tick(
    state: &mut ServerState,
    identity: PlayerRuntimeIdentity,
    packet: &[u8],
    server_tick: u32,
) -> bool {
    record_client_input_observation_in_state_with_policy_at_tick(
        state,
        identity,
        packet,
        server_tick,
        ClientInputObservationPolicy::Ordinary,
    )
}

fn record_client_input_observation_in_state_with_policy_at_tick(
    state: &mut ServerState,
    identity: PlayerRuntimeIdentity,
    packet: &[u8],
    server_tick: u32,
    policy: ClientInputObservationPolicy,
) -> bool {
    let player_id = identity.player_id();
    let observations = match parse_client_input_observations(packet) {
        Ok(observations) => observations,
        Err(reason) => {
            crate::security::log_security_reject(player_id, "input_record", reason);
            return false;
        }
    };
    let count = observations.len();
    if !state.player_runtime_matches(identity) {
        return false;
    }
    // A reliable lifecycle baseline can commit the successor life one
    // unordered-game-channel flight before its application receipt reaches
    // the browser. The newest record in an already-sent rolling packet then
    // still carries an authenticated predecessor-life presentation receipt.
    // Discard that packet before it can mutate successor input history or be
    // misclassified as an attack by combat-view verification. An unavailable
    // receipt remains admissible for movement as before; only an explicit,
    // nonzero, mismatched life proves this packet belongs to another scope.
    if !policy.is_authenticated_lifecycle_baseline()
        && observations.last().is_some_and(|observation| {
            let observed_life = observation.combat_view_receipt.observer_life_seq;
            observed_life != 0 && observed_life != identity.life.life_seq
        })
    {
        return false;
    }
    let pending_lifecycle_baseline_sequence = state
        .pending_position_inputs
        .lifecycle_baseline_sequence_for_runtime(identity);
    if let Some(client) = state.clients.get_mut(&player_id) {
        // The server can kill a player one realtime-channel flight before the
        // browser applies that lifecycle snapshot.  Its unordered rolling
        // input packet then still contains the just-ended life's authenticated
        // door history, while `clear_server_runtime` has intentionally erased
        // the canonical lower/upper bounds for that life.  Treat the whole
        // packet like the matching stale position: inert until the reliable
        // respawn baseline starts a new life.  Re-resolving it against the
        // dead receipt tick would manufacture door-seed security rejects and
        // must never rebuild movement/weapon authority for a dead player.
        let packet_tail = observations[count - 1].sequence;
        let follows_protected_dead_lifecycle_baseline = pending_lifecycle_baseline_sequence
            .is_some_and(|baseline| {
                let tail_distance = packet_tail.wrapping_sub(baseline);
                tail_distance > 0
                    && tail_distance < (1 << 15)
                    && (tail_distance as usize) < crate::state::MAX_RECENT_INPUT_OBSERVATIONS
                    && observations.iter().all(|observation| {
                        let distance = observation.sequence.wrapping_sub(baseline);
                        distance > 0 && distance <= tail_distance
                    })
            });
        if !client.alive
            && !policy.canonicalizes_neutral_boundary()
            && !follows_protected_dead_lifecycle_baseline
        {
            return false;
        }
        if observations.iter().any(|observation| {
            !client.position_sequence_belongs_to_active_life(observation.sequence)
        }) {
            return false;
        }
        if client.life_position_sequence_base.is_some()
            && client.last_position_sequence.is_none()
            && !policy.is_authenticated_lifecycle_baseline()
        {
            // A reliable ship/life mutation can overtake an old-life raw
            // input record on the unordered game lane. Its sequence may be
            // numerically identical to the newly reserved baseline, but only
            // the application-reliable position transaction is allowed to
            // define that record. Treat every standalone input as inert until
            // the exact baseline commits; otherwise the old record can be
            // installed first and make the honest neutral baseline look like
            // a sequence rewrite.
            return false;
        }
        if client.life_position_sequence_base.is_some()
            && client.last_position_sequence.is_none()
            && (observations.len() != 1 || Some(packet_tail) != client.life_position_sequence_base)
        {
            return false;
        }
        let newest = match client.last_input_seq {
            Some(last) if packet_tail != last && packet_tail.wrapping_sub(last) < (1 << 15) => {
                packet_tail
            }
            Some(last) => last,
            None => packet_tail,
        };
        let resolved_observations = if policy.canonicalizes_neutral_boundary() {
            // A ship/life boundary is already constrained to one exact
            // application-reliable neutral record and the server-owned spawn
            // pose. Its door byte cannot authorize movement or collision.
            // Canonicalize that unused byte to the current authenticated
            // server phase so a browser-generated baseline cannot become
            // invalid merely because the server/RTC reader was descheduled
            // after the browser's bounded door-clock extrapolation.
            //
            // Ordinary movement and the motion-preserving frequency baseline
            // continue through `resolve_client_input_door_phases` below; this
            // is not a stale-input allowance. The reliable delivery identity,
            // exact life sequence, neutral bits/direction, and canonical spawn
            // remain mandatory before this policy is selected.
            if observations.len() != 1
                || observations[0].input
                    != (crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    })
            {
                crate::security::log_security_reject(
                    player_id,
                    "input_record",
                    "nonneutral_lifecycle_baseline",
                );
                return false;
            }
            let phase =
                crate::doors::client_door_phase_candidates_at_tick(client.door_state, server_tick)
                    .into_iter()
                    .filter(|phase| {
                        crate::continuum::continuum_tick_diff(server_tick, phase.started_tick) >= 0
                    })
                    .min_by_key(|phase| {
                        crate::continuum::continuum_tick_diff(server_tick, phase.started_tick)
                            .unsigned_abs()
                    })
                    // Native fixtures without a completed security challenge retain
                    // the legacy zero byte. Production gameplay always has a door
                    // challenge before this boundary becomes ready.
                    .or_else(|| {
                        (client.door_state.is_none() && observations[0].door_seed == 0).then_some(
                            crate::doors::ClientDoorPhase {
                                seed: 0,
                                started_tick: server_tick,
                            },
                        )
                    });
            let Some(phase) = phase else {
                crate::security::log_security_reject(
                    player_id,
                    "input_record",
                    "missing_lifecycle_door_phase",
                );
                return false;
            };
            if observations[0].door_seed != phase.seed {
                debug!(
                    player_id,
                    input_sequence = observations[0].sequence,
                    claimed_door_seed = observations[0].door_seed,
                    canonical_door_seed = phase.seed,
                    canonical_door_phase_tick = phase.started_tick,
                    server_tick,
                    "canonicalized neutral lifecycle baseline door phase"
                );
            }
            vec![crate::state::SequencedInputObservation {
                sequence: observations[0].sequence,
                input: observations[0].input,
                door_seed: phase.seed,
                door_phase_tick: phase.started_tick,
            }]
        } else {
            match resolve_client_input_door_phases(client, &observations, newest, server_tick) {
                Ok(resolved) => resolved,
                Err(InputDoorPhaseResolutionError::Security(reason)) => {
                    crate::security::log_security_reject(player_id, "input_record", reason);
                    return false;
                }
                Err(InputDoorPhaseResolutionError::HistoricalPredecessorUnavailable) => {
                    debug!(
                        player_id,
                        packet_base_sequence = observations[0].sequence,
                        packet_tail_sequence = packet_tail,
                        server_tick,
                        "stale input door history unavailable; packet left inert"
                    );
                    return false;
                }
            }
        };

        // Validate the whole bundle before mutating. Canonical bit 5 may
        // differ from the wire pulse, so rewrite detection compares the
        // separately retained raw value plus every other input field.
        for observation in &observations {
            let sequence = observation.sequence;
            let input = observation.input;
            let age = newest.wrapping_sub(sequence);
            if age >= (1 << 15) || age as usize >= crate::state::MAX_RECENT_INPUT_OBSERVATIONS {
                continue;
            }
            let stored = client
                .recent_input_records
                .iter()
                .filter(|record| {
                    retained_input_door_phase_is_plausible(client, record, server_tick)
                })
                .find(|record| record.sequence == sequence);
            let stored_rocket_assertion = client.recent_input_rocket_assertions.iter().find_map(
                |&(stored_sequence, asserted)| (stored_sequence == sequence).then_some(asserted),
            );
            if stored.is_some_and(|record| {
                record.input.direction != input.direction
                    || record.input.bits & !CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT
                        != input.bits & !CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT
                    || stored_rocket_assertion
                        != Some(input.bits & CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT != 0)
            }) {
                crate::security::log_security_reject(player_id, "input_record", "sequence_rewrite");
                return false;
            }
        }

        // Freeze the first combat-view proof for every logical input on a
        // clone, then commit it with the rest of this packet's admission
        // transaction below. An unavailable/stale proof never discards valid
        // movement, but no later lane arrival may upgrade that same sequence
        // into weapon authority after seeing an outcome.
        let mut observer_combat_timeline = client.observer_combat_timeline.clone();
        // A lifecycle baseline is the transaction that creates the successor
        // runtime. Its embedded receipt can honestly name that successor
        // while `identity` and presentation history still name the retiring
        // life. The reliable lifecycle validator already constrains this
        // packet's delivery, life, pose, and neutral/motion semantics. Do not
        // verify it against the predecessor combat view or create a weapon
        // decision; `advance_life` clears the old decision scope at commit.
        if !policy.is_authenticated_lifecycle_baseline() {
            for observation in &observations {
                if let Err(rejection) = observer_combat_timeline.verify_receipt(
                    identity,
                    server_tick,
                    observation.combat_view_receipt,
                    &client.observer_presentation_history,
                ) {
                    if rejection.is_security_violation() {
                        let receipt = observation.combat_view_receipt;
                        let schedule = observer_combat_timeline.current_directive();
                        // Preserve the exact rejected claim and server offer at
                        // WARN, even when production DEBUG tracing is disabled.
                        // This is attribution only: neither admission nor the
                        // frozen first-observation decision changes here.
                        tracing::warn_span!(
                            "combat_view_validation",
                            input_sequence = observation.sequence,
                            server_tick,
                            receipt_generation = receipt.timeline_generation,
                            receipt_life_seq = receipt.observer_life_seq,
                            applied_snapshot_id = receipt.latest_game_applied_snapshot_id,
                            receipt_flags = receipt.flags,
                            offered_generation = ?schedule.map(|value| value.generation),
                            offered_effective_sequence = ?schedule.map(|value| value.effective_from_input_sequence),
                            offered_effective_tick = ?schedule.map(|value| value.effective_server_tick),
                        )
                        .in_scope(|| {
                            crate::security::log_security_reject(
                                player_id,
                                "input_record",
                                rejection.reason(),
                            );
                        });
                    } else {
                        debug!(
                            player_id,
                            input_sequence = observation.sequence,
                            server_tick,
                            reason = rejection.reason(),
                            "combat-view receipt unavailable for this input"
                        );
                    }
                }
            }
        }

        // Stage the full admission transaction. Wire bit 5 is an untrusted,
        // one-record activation pulse, not a client-owned active interval and
        // not permission to discard otherwise-valid movement history.
        // Impossible pulses are logged and canonicalized false while every
        // other field remains admissible.
        let mut canonical_records = client
            .recent_input_records
            .iter()
            .copied()
            .filter(|record| retained_input_door_phase_is_plausible(client, record, server_tick))
            .collect::<std::collections::VecDeque<_>>();
        let mut rocket_assertions = client.recent_input_rocket_assertions.clone();
        rocket_assertions.retain(|(sequence, _)| {
            canonical_records
                .iter()
                .any(|record| record.sequence == *sequence)
        });
        let mut rocket_until_tick = client.server_rocket_until_tick;
        let mut rocket_input_asserted = client.server_rocket_input_asserted;
        let mut rocket_authorizations = client.server_rocket_authorizations.clone();
        let mut weapons = client.server_weapons;
        let mut rocket_activation = None;
        let previous_newest = client.last_input_seq;
        let ship = client.player.ship;

        for resolved in resolved_observations {
            let sequence = resolved.sequence;
            let mut input = resolved.input;
            let age = newest.wrapping_sub(sequence);
            if age < (1 << 15)
                && (age as usize) < crate::state::MAX_RECENT_INPUT_OBSERVATIONS
                && !canonical_records
                    .iter()
                    .any(|record| record.sequence == sequence)
            {
                let raw_rocket_pulse = input.bits & CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT != 0;
                let is_forward = previous_newest
                    .is_none_or(|last| sequence != last && sequence.wrapping_sub(last) < (1 << 15));
                let mut rocket_sequence_active = rocket_authorizations
                    .iter()
                    .any(|authorization| authorization.contains_sequence(sequence));

                if is_forward {
                    // An honest client emits one true record followed by
                    // false records throughout the server-owned active
                    // interval. The latch also quarantines malicious held
                    // true records; a duplicate of one sequence never enters
                    // this new-record branch.
                    let new_pulse = raw_rocket_pulse && !rocket_input_asserted;
                    if new_pulse && !rocket_sequence_active {
                        let lifetime_ticks = arena_ship_rocket_lifetime_ticks(ship);
                        let reject_reason = if !client.joined || !client.alive || ship >= 8 {
                            Some("rocket_invalid_life")
                        } else if client
                            .server_warp_bomb_lock_until_tick
                            .is_some_and(|until| {
                                !crate::continuum::continuum_tick_reached(server_tick, until)
                            })
                        {
                            Some("rocket_manual_warp_cooldown")
                        } else if weapons.rockets == 0 {
                            Some("rocket_not_in_inventory")
                        } else if u64::from(lifetime_ticks) * 3 <= 5 {
                            Some("rocket_zero_lifetime")
                        } else {
                            None
                        };
                        if let Some(reason) = reject_reason {
                            crate::security::log_security_reject(player_id, "input_record", reason);
                        } else {
                            weapons.rockets = weapons.rockets.saturating_sub(1);
                            let remaining_ticks =
                                crate::gameplay_rules::rocket_remaining_ticks_after_sequence(
                                    sequence,
                                    lifetime_ticks,
                                    newest,
                                )
                                .expect("admitted pulse must precede the bounded packet tail");
                            let deadline =
                                crate::continuum::continuum_tick_add(server_tick, remaining_ticks);
                            rocket_until_tick = (remaining_ticks > 0).then_some(deadline);
                            rocket_authorizations.push_back(
                                crate::state::ServerRocketAuthorization {
                                    start_sequence: sequence,
                                    start_tick: server_tick,
                                    deadline_tick: deadline,
                                    lifetime_ticks,
                                },
                            );
                            rocket_activation = Some((
                                sequence,
                                newest,
                                deadline,
                                remaining_ticks,
                                weapons.rockets,
                            ));
                            rocket_sequence_active = true;
                        }
                    } else if raw_rocket_pulse && !rocket_sequence_active {
                        // A held/repeated true bit after a rejected pulse or
                        // outside the authorized logical interval cannot
                        // activate or extend RocketTime. The first pulse was
                        // already logged, so silently quarantine continuation
                        // records to avoid attacker-controlled log
                        // amplification.
                    }
                    rocket_input_asserted = raw_rocket_pulse;
                } else {
                    // A late recovered record cannot activate or extend a
                    // timer. Its canonical value comes from the retained
                    // logical authorization even when wall time has already
                    // passed the deadline.
                }

                input.bits &= !CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT;
                if rocket_sequence_active {
                    // Bit 5 is repurposed only inside the private canonical
                    // observation as the server-derived active interval. It
                    // is never echoed as client pulse state.
                    input.bits |= CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT;
                }
                canonical_records.push_back(crate::state::SequencedInputObservation {
                    sequence,
                    input,
                    door_seed: resolved.door_seed,
                    door_phase_tick: resolved.door_phase_tick,
                });
                rocket_assertions.push_back((sequence, raw_rocket_pulse));
            }
        }
        canonical_records.retain(|record| {
            let age = newest.wrapping_sub(record.sequence);
            age < (1 << 15) && (age as usize) < crate::state::MAX_RECENT_INPUT_OBSERVATIONS
        });
        rocket_assertions.retain(|(sequence, _)| {
            let age = newest.wrapping_sub(*sequence);
            age < (1 << 15) && (age as usize) < crate::state::MAX_RECENT_INPUT_OBSERVATIONS
        });
        canonical_records
            .make_contiguous()
            .sort_by_key(|record| std::cmp::Reverse(newest.wrapping_sub(record.sequence)));
        rocket_assertions
            .make_contiguous()
            .sort_by_key(|(sequence, _)| std::cmp::Reverse(newest.wrapping_sub(*sequence)));
        while canonical_records.len() > crate::state::MAX_RECENT_INPUT_OBSERVATIONS {
            canonical_records.pop_front();
        }
        while rocket_assertions.len() > crate::state::MAX_RECENT_INPUT_OBSERVATIONS {
            rocket_assertions.pop_front();
        }
        rocket_authorizations.retain(|authorization| {
            authorization
                .last_active_sequence()
                .is_some_and(|last_active| {
                    let age = newest.wrapping_sub(last_active);
                    age >= (1 << 15) || (age as usize) < crate::state::MAX_RECENT_INPUT_OBSERVATIONS
                })
        });
        while rocket_authorizations.len() > crate::state::MAX_RECENT_INPUT_OBSERVATIONS {
            rocket_authorizations.pop_front();
        }

        client.recent_input_records = canonical_records;
        client.observer_combat_timeline = observer_combat_timeline;
        if !policy.is_authenticated_lifecycle_baseline()
            && let Some(newest_receipt) = observations
                .iter()
                .min_by_key(|observation| newest.wrapping_sub(observation.sequence))
                .map(|observation| observation.combat_view_receipt)
        {
            advance_snapshot_base_from_receipt(client, identity, newest_receipt);
        }
        client.recent_input_rocket_assertions = rocket_assertions;
        client.server_rocket_until_tick = rocket_until_tick;
        client.server_rocket_input_asserted = rocket_input_asserted;
        client.server_rocket_authorizations = rocket_authorizations;
        client.server_weapons = weapons;
        client.observe_prize_effect_application_boundary(newest);
        if let Some((sequence, observed_sequence, deadline, remaining_ticks, rockets_remaining)) =
            rocket_activation
        {
            let elapsed_logical_steps =
                crate::gameplay_rules::rocket_elapsed_logical_steps(sequence, observed_sequence)
                    .expect("activation sequence must precede observed sequence");
            info!(
                "zubspace_event {{\"type\":\"server_rocket_activated\",\"player_id\":{},\"life_seq\":{},\"input_sequence\":{},\"observed_input_sequence\":{},\"elapsed_logical_steps\":{},\"server_tick\":{},\"deadline_tick\":{},\"remaining_ticks\":{},\"rockets_remaining\":{}}}",
                player_id,
                client.life_seq,
                sequence,
                observed_sequence,
                elapsed_logical_steps,
                server_tick,
                deadline,
                remaining_ticks,
                rockets_remaining
            );
        }
        if client.last_input_seq != Some(newest) {
            client.last_input_seq = Some(newest);
            client.last_input_record = client
                .recent_input_records
                .iter()
                .find(|record| record.sequence == newest)
                .map(|record| record.input);
        } else if client.last_input_record.is_none() {
            client.last_input_record = client
                .recent_input_records
                .iter()
                .find(|record| record.sequence == newest)
                .map(|record| record.input);
        }
    }
    true
}

/// Local diagnostic opt-in only; never consulted by gameplay admission policy.
pub(crate) fn position_timing_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("ZUBSPACE_SERVER_POSITION_TIMING").as_deref() == Ok("1"))
}

/// Optional wall-clock diagnostics only; these phases are never authority input.
#[derive(Debug)]
pub(crate) struct ClientPacketHandlerTiming {
    pub(crate) arena_lookup_us: u64,
    pub(crate) arena_wait_us: u64,
    pub(crate) ingress_lookup_us: u64,
    pub(crate) ingress_wait_us: u64,
    pub(crate) identity_check_us: u64,
    pub(crate) dispatch_us: u64,
}

pub(crate) fn handle_client_packet_for_session(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerSessionIdentity,
    runtime_identity: Option<PlayerRuntimeIdentity>,
    packet: Vec<u8>,
    peer: &str,
    joined: &mut bool,
    observe_timing: bool,
) -> Option<ClientPacketHandlerTiming> {
    let started = observe_timing.then(Instant::now);
    let arena_gate = state
        .lock()
        .expect("server state should not be poisoned")
        .arena_transaction_gate
        .clone();
    let arena_lookup = started.map(|_| Instant::now());
    let _arena_transaction = arena_gate
        .read()
        .expect("arena transaction gate should not be poisoned");
    let arena_acquired = started.map(|_| Instant::now());
    // Clone the gate belonging to the current id occupant, then validate the
    // never-reused session token while holding it. Disconnect/removal takes the
    // same gate. If an old handler resumes after id reuse it may clone the new
    // occupant's gate, but the identity check fails before any packet mutation.
    let ingress_gate = state
        .lock()
        .expect("server state should not be poisoned")
        .player_session_ingress_gate(identity.player_id);
    let ingress_lookup = started.map(|_| Instant::now());
    let Some(ingress_gate) = ingress_gate else {
        return None;
    };
    let _ingress = ingress_gate
        .lock()
        .expect("client ingress gate should not be poisoned");
    let ingress_acquired = started.map(|_| Instant::now());
    if !state
        .lock()
        .expect("server state should not be poisoned")
        .player_session_matches(identity)
    {
        return None;
    }
    let identity_checked = started.map(|_| Instant::now());
    handle_authenticated_client_packet(
        state,
        identity,
        runtime_identity,
        packet,
        None,
        peer,
        joined,
    );
    let micros = |end: Instant, start: Instant| end.duration_since(start).as_micros() as u64;
    started.map(|start| ClientPacketHandlerTiming {
        arena_lookup_us: micros(arena_lookup.unwrap(), start),
        arena_wait_us: micros(arena_acquired.unwrap(), arena_lookup.unwrap()),
        ingress_lookup_us: micros(ingress_lookup.unwrap(), arena_acquired.unwrap()),
        ingress_wait_us: micros(ingress_acquired.unwrap(), ingress_lookup.unwrap()),
        identity_check_us: micros(identity_checked.unwrap(), ingress_acquired.unwrap()),
        dispatch_us: micros(Instant::now(), identity_checked.unwrap()),
    })
}

#[cfg(test)]
pub(crate) fn handle_client_packet(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    packet: Vec<u8>,
    peer: &str,
    joined: &mut bool,
) {
    let (identity, runtime_identity) = {
        let state = state.lock().expect("server state should not be poisoned");
        (
            state.active_player_session_identity(player_id),
            state.active_player_runtime_identity(player_id),
        )
    };
    if let Some(identity) = identity {
        handle_client_packet_for_session(
            state,
            identity,
            runtime_identity,
            packet,
            peer,
            joined,
            false,
        );
    }
}

/// Dispatch after the connection boundary has authenticated and serialized
/// this session. Recursive packet-cluster dispatch remains under that one gate.
fn handle_authenticated_client_packet(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerSessionIdentity,
    runtime_identity: Option<PlayerRuntimeIdentity>,
    packet: Vec<u8>,
    reliable_delivery_id: Option<u32>,
    peer: &str,
    joined: &mut bool,
) {
    let player_id = identity.player_id;
    if packet.first() == Some(&crate::observability::PLAYER_REPORT_TYPE) {
        crate::observability::receive_player_report(state, identity, &packet);
        return;
    }
    if packet.first() == Some(&crate::connection_quality::PageVisibilityNotice::TYPE) {
        let mut state = state.lock().expect("server state should not be poisoned");
        crate::connection_quality::receive_page_visibility(
            &mut state,
            identity,
            &packet,
            Instant::now(),
        );
        return;
    }
    if is_time_sync_request(&packet) {
        let arena_stream_id = state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&player_id)
            .filter(|client| client.session_id == identity.session_id)
            .map(|client| client.arena_stream_id)
            .unwrap_or(0);
        send_to(
            state,
            player_id,
            time_sync_response(&packet, crate::wire::server_session_id(), arena_stream_id),
        );
        return;
    }
    if packet.first().copied() == Some(SECURITY_RESPONSE) {
        update_security_report(state, player_id, &packet);
        return;
    }
    if packet.first().copied() == Some(CLIENT_POSITION) {
        let Some(runtime_identity) = runtime_identity else {
            return;
        };
        // Diagnostic only: isolate time spent validating/queueing a position
        // from browser-to-dispatch and queue-to-admission delays. Never use
        // this wall-clock measurement in movement or receipt policy.
        let timing_at_info = position_timing_trace_enabled();
        let dispatch_started = (tracing::enabled!(tracing::Level::DEBUG)
            || (timing_at_info && tracing::enabled!(tracing::Level::INFO)))
        .then(std::time::Instant::now);
        let mut diagnostic_sequence = None;
        let mut lifecycle_baseline = false;
        // The current browser appends the same bounded rolling input packet
        // to its position. Record it before queueing that position so an
        // unordered DataChannel can never expose the position without the
        // exact logical history needed to validate it.
        if packet.len() > crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET {
            let Some(snapshot) = crate::continuum_position::parse_client_position_snapshot(&packet)
            else {
                tracing::debug!(
                    player_id,
                    packet_len = packet.len(),
                    trailer_well_formed = crate::continuum_position::client_position_trailer_is_well_formed(&packet),
                    canonical_state = ?crate::continuum_position::client_position_canonical_state(&packet),
                    embedded_input_count = ?packet.get(
                        crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET + 3,
                    ),
                    packet_tail = ?packet.get(packet.len().saturating_sub(16)..),
                    "malformed current-protocol client position detail"
                );
                crate::security::log_security_reject(player_id, "position", "malformed");
                return;
            };
            let Some(position_sequence) = snapshot.client_sequence else {
                crate::security::log_security_reject(player_id, "position", "missing_sequence");
                return;
            };
            diagnostic_sequence = Some(position_sequence);
            let belongs_to_active_life = {
                let state = state.lock().expect("server state should not be poisoned");
                if !state.player_runtime_matches(runtime_identity) {
                    return;
                }
                state.clients.get(&player_id).is_some_and(|client| {
                    client.position_sequence_belongs_to_active_life(position_sequence)
                })
            };
            if !belongs_to_active_life {
                tracing::debug!(
                    player_id,
                    position_sequence,
                    "dropping previous-life position before embedded input admission"
                );
                return;
            }
            let pending_ship_life_reset_canonical = {
                let state = state.lock().expect("server state should not be poisoned");
                crate::arena::pending_ship_life_reset_position_is_canonical(
                    &state, player_id, snapshot,
                )
            };
            let pending_frequency_baseline_canonical = {
                let state = state.lock().expect("server state should not be poisoned");
                crate::arena::pending_frequency_baseline_is_canonical(&state, player_id, snapshot)
            };
            if reliable_delivery_id.is_none()
                && (pending_ship_life_reset_canonical.is_some()
                    || pending_frequency_baseline_canonical.is_some())
            {
                // The reliable ship/frequency request can legitimately
                // overtake an already-sent unordered realtime position. It
                // cannot satisfy the new lifecycle boundary, but it is not a
                // security violation either: discard it without admitting
                // any input or position state and wait for the reliable
                // baseline carrying the exact delivery identity.
                debug!(
                    player_id,
                    pending_ship_life_reset = pending_ship_life_reset_canonical.is_some(),
                    pending_frequency_baseline = pending_frequency_baseline_canonical.is_some(),
                    "dropping unordered position overtaken by reliable lifecycle cutover"
                );
                return;
            }
            let pending_ship_life_reset = match pending_ship_life_reset_canonical {
                Some(false) => {
                    let state = state.lock().expect("server state should not be poisoned");
                    if let Some(client) = state.clients.get(&player_id) {
                        let (spawn_x, spawn_y) =
                            crate::zone::server_spawn_coordinates(state.zone_map());
                        tracing::warn!(
                            player_id,
                            position_sequence,
                            snapshot = ?snapshot,
                            expected_spawn_x = spawn_x,
                            expected_spawn_y = spawn_y,
                            expected_bounty = client.server_bounty,
                            expected_energy = client.server_energy,
                            "noncanonical ship-life reset details"
                        );
                    }
                    drop(state);
                    crate::security::log_security_reject(
                        player_id,
                        "position",
                        "noncanonical_ship_life_reset",
                    );
                    return;
                }
                Some(true) => true,
                None => false,
            };
            let pending_frequency_baseline = match pending_frequency_baseline_canonical {
                Some(false) => {
                    crate::security::log_security_reject(
                        player_id,
                        "position",
                        "noncanonical_frequency_baseline",
                    );
                    return;
                }
                Some(true) => true,
                None => false,
            };
            let awaiting_respawn = state
                .lock()
                .expect("server state should not be poisoned")
                .clients
                .get(&player_id)
                .is_some_and(|client| !client.alive);
            lifecycle_baseline = pending_ship_life_reset
                || pending_frequency_baseline
                || (awaiting_respawn && reliable_delivery_id.is_some());
            let dead_respawn_baseline = awaiting_respawn && reliable_delivery_id.is_some();
            let canonical_neutral_lifecycle_baseline =
                pending_ship_life_reset || dead_respawn_baseline;
            let input_observation_policy = if canonical_neutral_lifecycle_baseline {
                ClientInputObservationPolicy::CanonicalNeutralLifecycleBaseline
            } else if lifecycle_baseline {
                ClientInputObservationPolicy::AuthenticatedMotionLifecycleBaseline
            } else {
                ClientInputObservationPolicy::Ordinary
            };
            let Some(input_packet) =
                crate::continuum_position::embedded_client_input_record(&packet)
            else {
                crate::security::log_security_reject(
                    player_id,
                    "position",
                    "invalid_embedded_input_type",
                );
                return;
            };
            let observations = match parse_client_input_observations(input_packet) {
                Ok(observations) => observations,
                Err(reason) => {
                    crate::security::log_security_reject(player_id, "input_record", reason);
                    return;
                }
            };
            let Some(tail) = observations.last().copied() else {
                crate::security::log_security_reject(player_id, "input_record", "empty");
                return;
            };
            let tail_sequence = tail.sequence;
            let tail_input = tail.input;
            if dead_respawn_baseline
                && (observations.len() != 1
                    || tail_sequence != position_sequence
                    || tail_input
                        != (crate::state::ClientInputObservation {
                            bits: 0,
                            direction: 0,
                        }))
            {
                // Only the exact application-reliable neutral boundary may
                // seed a dead runtime. Ordinary old-life rolling history is
                // inert until this transaction is due and committed.
                debug!(
                    player_id,
                    position_sequence,
                    input_count = observations.len(),
                    input_bits = tail_input.bits,
                    input_direction = tail_input.direction,
                    "dropping noncanonical reliable respawn baseline"
                );
                return;
            }
            if pending_ship_life_reset
                && (observations.len() != 1
                    || tail_sequence != position_sequence
                    || tail_input
                        != (crate::state::ClientInputObservation {
                            bits: 0,
                            direction: 0,
                        }))
            {
                crate::security::log_security_reject(
                    player_id,
                    "position",
                    "noncanonical_ship_life_reset",
                );
                return;
            }
            if pending_frequency_baseline
                && (observations.len() != 1
                    || observations[0].sequence != position_sequence
                    || observations[0].input.bits & CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT != 0)
            {
                crate::security::log_security_reject(
                    player_id,
                    "position",
                    "noncanonical_frequency_baseline",
                );
                return;
            }
            if tail_sequence != position_sequence {
                crate::security::log_security_reject(
                    player_id,
                    "position",
                    "embedded_tail_sequence_mismatch",
                );
                return;
            }
            if tail_input.direction != snapshot.direction {
                crate::security::log_security_reject(
                    player_id,
                    "position",
                    "embedded_tail_direction_mismatch",
                );
                return;
            }
            // Use one receipt-time tick for the embedded input history and
            // its prospective movement witness. Door recovery below retains
            // the bounded deterministic history needed when a long server
            // step delayed dispatch of an otherwise timely RTC packet.
            let ingress_server_tick = crate::wire::current_tick();
            let prospective_validation = {
                let state = state.lock().expect("server state should not be poisoned");
                if !state.player_runtime_matches(runtime_identity) {
                    return;
                }
                let witness_additions = observations
                    .iter()
                    .map(|observation| (observation.sequence, observation.input))
                    .collect::<Vec<_>>();
                state.clients.get(&player_id).map(|client| {
                    crate::sim_observation::validate_position_input_witness_with_additions(
                        client,
                        position_sequence,
                        snapshot.direction,
                        ingress_server_tick,
                        &witness_additions,
                    )
                })
            };
            if let Some(Err(error)) = prospective_validation {
                // Missing older history remains eligible for the bounded tick
                // deferral. Impossible direction evidence fails atomically
                // before the embedded records are installed.
                if error != crate::sim_observation::PositionInputWitnessError::Missing {
                    if error
                        == crate::sim_observation::PositionInputWitnessError::InvalidDirectionHistory
                    {
                        let state = state.lock().expect("server state should not be poisoned");
                        if let Some(client) = state.clients.get(&player_id) {
                            tracing::warn!(
                                player_id,
                                position_sequence,
                                snapshot_direction = snapshot.direction,
                                last_position_sequence = ?client.last_position_sequence,
                                last_published_direction = ?client
                                    .last_position
                                    .as_deref()
                                    .and_then(|position| position.get(1).copied()),
                                private_orientation = ?client
                                    .server_sim_state
                                    .map(|sim| sim.orientation),
                                rotation_per_second = client.server_rotation_per_second,
                                canonical_movement_witness = ?client
                                    .canonical_movement_witness,
                                prospective_inputs = ?observations,
                                "embedded position direction witness is impossible"
                            );
                        }
                    }
                    crate::security::log_security_reject(
                        player_id,
                        "position",
                        error.security_reason(),
                    );
                    return;
                }
            }
            if !record_client_input_observation_with_policy_at_tick(
                state,
                runtime_identity,
                input_packet,
                ingress_server_tick,
                input_observation_policy,
            ) {
                return;
            }
        }
        let mut state = state.lock().expect("server state should not be poisoned");
        if lifecycle_baseline {
            let Some(reliable_delivery_id) = reliable_delivery_id else {
                drop(state);
                crate::security::log_security_reject(
                    player_id,
                    "position",
                    "unreliable_lifecycle_baseline",
                );
                return;
            };
            state.queue_lifecycle_baseline_for_runtime(
                runtime_identity,
                packet,
                reliable_delivery_id,
            );
        } else {
            if let Some(client) = state.clients.get_mut(&player_id) {
                client.positions_received = client.positions_received.saturating_add(1);
                // A hidden page sends no positions; a "hidden" one that keeps
                // playing loses the measurement pause.
                if client.connection_quality.as_ref().is_some_and(|quality| {
                    quality
                        .lock()
                        .expect("session quality lock poisoned")
                        .note_gameplay(Instant::now())
                }) {
                    tracing::warn!(
                        player_id,
                        session_id = client.session_id.get(),
                        "page reported hidden while sending gameplay; measurement resumed"
                    );
                }
            }
            state.queue_position_input_for_runtime(runtime_identity, packet);
        }
        drop(state);
        if let Some(started) = dispatch_started {
            if timing_at_info {
                tracing::info!(
                    player_id,
                    runtime = ?runtime_identity,
                    input_sequence = ?diagnostic_sequence,
                    dispatch_elapsed_us = started.elapsed().as_micros() as u64,
                    "position dispatch timing"
                );
            } else {
                tracing::debug!(
                player_id,
                runtime = ?runtime_identity,
                input_sequence = ?diagnostic_sequence,
                dispatch_elapsed_us = started.elapsed().as_micros() as u64,
                "position dispatch timing"
                );
            }
        }
        return;
    }
    // Phase 1b+ movement authority packet: client → server input
    // record. Parse and stash the most recent input on the client
    // handle; the server simulation consumes that mirror when
    // accepting the next position update.
    if packet.first().copied() == Some(CLIENT_INPUT_RECORD_TYPE) {
        let Some(runtime_identity) = runtime_identity else {
            return;
        };
        let _ = record_client_input_observation(state, runtime_identity, &packet);
        // Session evidence for the failure artifact: what this server accepted, not what a client says it sent.
        if let Ok(mut guard) = state.lock()
            && let Some(client) = guard.clients.get_mut(&player_id)
        {
            client.inputs_received = client.inputs_received.saturating_add(1);
        }
        return;
    }
    if packet.first().copied() == Some(TAKE_PRIZE_REQUEST) {
        relay_prize_pickup(state, player_id, &packet);
        return;
    }
    if let Some(clustered) = packet_cluster_parts(&packet) {
        for packet in clustered {
            // A clustered acknowledgement has to be recognised here, before the cluster is handed to the authenticated
            // packet handler and the function returns. Reliable acknowledgements are parsed further down, which this
            // branch never reaches, so a cluster of acks was swallowed whole while a single ack travelled as its own
            // packet and worked. The zone-file transfer acks a whole in-flight window (64 frames) in one clustered
            // packet, so every one of those acknowledgements vanished: the server's bounded application-transfer window
            // stayed full, the file froze mid-transfer, the client never reached gameplay authority, never sent a
            // position, and could never be shown to a peer. Single-ack cases kept working, which is what made the
            // failure intermittent.
            if receive_reliable_acks(state, identity, packet) {
                continue;
            }
            handle_authenticated_client_packet(
                state,
                identity,
                runtime_identity,
                packet.to_vec(),
                reliable_delivery_id,
                peer,
                joined,
            );
        }
        return;
    }
    if receive_reliable_acks(state, identity, &packet) {
        return;
    }
    let Some((reliable_id, payload)) = reliable_parts(&packet) else {
        return;
    };
    if payload.len() > crate::reliable::MAX_RELIABLE_PAYLOAD_SIZE {
        log_security_reject(player_id, "reliable", "oversized_wrapper_payload");
    }

    let lifecycle_candidate = matches!(
        payload.first().copied(),
        Some(CLIENT_POSITION) | Some(SHIP_CHANGE_REQUEST)
    );
    if lifecycle_candidate {
        tracing::debug!(
            player_id,
            reliable_id,
            payload_type = payload.first().copied(),
            "received reliable lifecycle payload"
        );
    }

    let committed_duplicate = {
        let mut state = state.lock().expect("server state should not be poisoned");
        let Some(client) = state.clients.get_mut(&player_id) else {
            return;
        };
        client
            .incoming_reliable
            .receive_to_dispatch_backlog_with_ack(reliable_id, payload)
    };
    if lifecycle_candidate {
        tracing::debug!(
            player_id,
            reliable_id,
            committed_duplicate,
            "sequenced reliable lifecycle payload"
        );
    }
    if committed_duplicate {
        send_to(state, player_id, reliable_ack(reliable_id));
    }
    loop {
        let (wrapper, received) = {
            let mut state = state.lock().expect("server state should not be poisoned");
            let Some(client) = state.clients.get_mut(&player_id) else {
                return;
            };
            let Some(wrapper) = client.incoming_reliable.take_dispatch_head() else {
                break;
            };
            let received = client.incoming_reliable.prepare_dispatch_head(&wrapper);
            (wrapper, received)
        };
        let non_gameplay = received.as_ref().is_some_and(|received| {
            !is_unified_gameplay_mutation_type(received.payload.first().copied())
        });
        if non_gameplay {
            // Preserve the historical ACK-before-response ordering for
            // non-gameplay requests, while still committing wrappers strictly
            // in order. Gameplay mutations take the stricter path below and
            // are ACKed only after durable sequencer admission.
            send_to(state, player_id, reliable_ack(wrapper.delivery_id));
        }
        if let Some(received) = &received
            && !try_admit_sequenced_gameplay_payload(
                state,
                player_id,
                received.delivery_id,
                &received.payload,
                peer,
                joined,
            )
        {
            let mut state = state.lock().expect("server state should not be poisoned");
            if let Some(client) = state.clients.get_mut(&player_id) {
                client.incoming_reliable.restore_dispatch_head(wrapper);
            }
            break;
        }
        {
            let mut state = state.lock().expect("server state should not be poisoned");
            let Some(client) = state.clients.get_mut(&player_id) else {
                return;
            };
            client.incoming_reliable.commit_dispatch_head(&wrapper);
        }
        if !non_gameplay {
            send_to(state, player_id, reliable_ack(wrapper.delivery_id));
        }
    }
}

fn is_unified_gameplay_mutation_type(packet_type: Option<u8>) -> bool {
    matches!(
        packet_type,
        Some(crate::objective_protocol::ObjectiveActionRequest::TYPE)
            | Some(crate::movement_correction::MovementCorrectionApplied::TYPE)
            | Some(TAKE_PRIZE_REQUEST)
            | Some(ATTACH_REQUEST)
            | Some(DROP_ATTACHED_REQUEST)
            | Some(DROP_BRICK_REQUEST)
            | Some(FREQUENCY_CHANGE_REQUEST)
            | Some(SHIP_CHANGE_REQUEST)
            | Some(LEAVE_ARENA_REQUEST)
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MovementCorrectionApplicationScope {
    Active(PlayerRuntimeIdentity),
    ExactRetired,
    Invalid,
}

pub(crate) fn movement_correction_application_scope(
    state: &ServerState,
    player_id: u16,
    application: crate::movement_correction::MovementCorrectionApplied,
) -> MovementCorrectionApplicationScope {
    if application.server_session_id != crate::wire::server_session_id()
        || application.player_id != player_id
    {
        return MovementCorrectionApplicationScope::Invalid;
    }
    let Some(identity) = state.active_player_runtime_identity(player_id) else {
        return MovementCorrectionApplicationScope::Invalid;
    };
    if application.life_seq == identity.life.life_seq {
        return MovementCorrectionApplicationScope::Active(identity);
    }
    let exact_retired = state.clients.get(&player_id).is_some_and(|client| {
        client
            .retired_movement_corrections
            .iter()
            .copied()
            .any(|retired| retired.matches(application))
    });
    if exact_retired {
        MovementCorrectionApplicationScope::ExactRetired
    } else {
        MovementCorrectionApplicationScope::Invalid
    }
}

/// Returns false only when a well-framed, current-runtime gameplay mutation
/// could not be durably admitted. The receiver retains that exact logical
/// payload and withholds progress on every later reliable until capacity is
/// available; malformed/stale inputs are terminal security no-ops.
fn try_admit_sequenced_gameplay_payload(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    reliable_delivery_id: u32,
    payload: &[u8],
    peer: &str,
    joined: &mut bool,
) -> bool {
    let packet_type = payload.first().copied();
    if packet_type == Some(crate::objective_protocol::ObjectiveActionRequest::TYPE) {
        return relay_objective_action_at_delivery(state, player_id, reliable_delivery_id, payload);
    }
    if packet_type == Some(crate::movement_correction::MovementCorrectionApplied::TYPE) {
        let Ok(application) = crate::movement_correction::MovementCorrectionApplied::parse(payload)
        else {
            log_security_reject(player_id, "movement_correction", "malformed_application");
            return true;
        };
        let mut state = state.lock().expect("server state should not be poisoned");
        return match movement_correction_application_scope(&state, player_id, application) {
            MovementCorrectionApplicationScope::Active(identity) => state
                .queue_movement_correction_application_for_runtime(
                    identity,
                    reliable_delivery_id,
                    application,
                ),
            MovementCorrectionApplicationScope::ExactRetired => {
                tracing::debug!(
                    player_id,
                    life_seq = application.life_seq,
                    correction_generation = application.correction_generation,
                    detected_sequence = application.detected_sequence,
                    application_sequence = application.application_sequence,
                    "ignored exact correction application retired by a newer life"
                );
                true
            }
            MovementCorrectionApplicationScope::Invalid => {
                log_security_reject(
                    player_id,
                    "movement_correction",
                    "stale_application_runtime",
                );
                true
            }
        };
    }
    let parsed = match packet_type {
        Some(TAKE_PRIZE_REQUEST) => {
            ReliablePrizeRequest::parse(payload).map(PendingReliableGameplayMutationKind::Prize)
        }
        Some(ATTACH_REQUEST) => {
            ReliableAttachRequest::parse(payload).map(PendingReliableGameplayMutationKind::Attach)
        }
        Some(DROP_ATTACHED_REQUEST) if payload.len() == 1 => {
            Some(PendingReliableGameplayMutationKind::DropAttached)
        }
        Some(DROP_BRICK_REQUEST) => ReliableBrickRequest::parse(payload).map(|request| {
            PendingReliableGameplayMutationKind::Brick {
                request,
                source: BrickSourceBinding::Pending,
            }
        }),
        Some(FREQUENCY_CHANGE_REQUEST) => ReliableFrequencyChange::parse(payload)
            .map(PendingReliableGameplayMutationKind::Frequency),
        Some(SHIP_CHANGE_REQUEST) => {
            ReliableShipChange::parse(payload).map(PendingReliableGameplayMutationKind::Ship)
        }
        Some(LEAVE_ARENA_REQUEST) if payload.len() == 1 && *joined => {
            Some(PendingReliableGameplayMutationKind::Leave)
        }
        Some(DROP_ATTACHED_REQUEST) | Some(LEAVE_ARENA_REQUEST) => None,
        _ => {
            handle_client_reliable_payload(
                state,
                player_id,
                reliable_delivery_id,
                payload,
                peer,
                joined,
            );
            return true;
        }
    };
    let Some(kind) = parsed else {
        log_security_reject(player_id, "reliable_gameplay", "malformed_semantic_framing");
        return true;
    };
    let queued = queue_reliable_gameplay_mutation(state, player_id, reliable_delivery_id, kind);
    if queued && matches!(packet_type, Some(LEAVE_ARENA_REQUEST)) {
        *joined = false;
        info!(player_id, peer = %peer, "server arena leave queued");
    }
    queued
}

fn handle_client_reliable_payload(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    reliable_delivery_id: u32,
    payload: &[u8],
    peer: &str,
    joined: &mut bool,
) {
    if payload.first().copied() == Some(crate::objective_protocol::ObjectiveActionRequest::TYPE) {
        relay_objective_action_at_delivery(state, player_id, reliable_delivery_id, payload);
    } else if payload.first().copied()
        == Some(crate::portal_placement::PortalPlacementRequest::TYPE)
    {
        let Ok(request) = crate::portal_placement::PortalPlacementRequest::parse(payload) else {
            log_security_reject(player_id, "portal", "malformed_placement_request");
            return;
        };
        let mut state = state.lock().expect("server state should not be poisoned");
        let Some(identity) = state.active_player_runtime_identity(player_id) else {
            return;
        };
        let scoped = request.server_session_id == crate::wire::server_session_id()
            && request.arena_stream_id
                == state
                    .clients
                    .get(&player_id)
                    .map_or(0, |client| client.arena_stream_id)
            && request.player_id == player_id
            && request.life_seq == identity.life.life_seq;
        if !scoped {
            tracing::debug!(
                player_id,
                request_player_id = request.player_id,
                request_life_seq = request.life_seq,
                active_life_seq = identity.life.life_seq,
                action_id = request.action_id,
                "ignored stale Portal placement request"
            );
            return;
        }
        if !state.queue_portal_placement_for_runtime(identity, request, crate::wire::current_tick())
        {
            log_security_reject(player_id, "portal", "placement_queue_capacity");
        }
    } else if payload.first().copied() == Some(crate::portal_warp::PortalWarpRequest::TYPE) {
        let Ok(request) = crate::portal_warp::PortalWarpRequest::parse(payload) else {
            log_security_reject(player_id, "portal", "malformed_warp_request");
            return;
        };
        let mut state = state.lock().expect("server state should not be poisoned");
        let Some(identity) = state.active_player_runtime_identity(player_id) else {
            return;
        };
        let scoped = request.server_session_id == crate::wire::server_session_id()
            && request.arena_stream_id
                == state
                    .clients
                    .get(&player_id)
                    .map_or(0, |client| client.arena_stream_id)
            && request.player_id == player_id
            && request.life_seq == identity.life.life_seq;
        if !scoped {
            tracing::debug!(
                player_id,
                request_player_id = request.player_id,
                request_life_seq = request.life_seq,
                active_life_seq = identity.life.life_seq,
                action_id = request.action_id,
                "ignored stale Portal Warp request"
            );
            return;
        }
        if !state.queue_portal_warp_for_runtime(identity, request, crate::wire::current_tick()) {
            log_security_reject(player_id, "portal", "warp_queue_capacity");
        }
    } else if payload.first().copied() == Some(crate::prize_application::PrizeEffectApplied::TYPE) {
        let Ok(application) = crate::prize_application::PrizeEffectApplied::parse(payload) else {
            log_security_reject(player_id, "prize", "malformed_effect_application");
            return;
        };
        if let Some(accepted) = apply_prize_effect_application(state, player_id, application) {
            send_reliable_to(state, player_id, accepted.encode());
        } else {
            // A valid application can arrive after its life/session queue was
            // retired, or out of order while the reliable prefix is being
            // retried. Treat that as an idempotent stale no-op; only malformed
            // wire data is a security rejection.
            tracing::debug!(
                player_id,
                collectible_id = application.collectible_id,
                life_seq = application.life_seq,
                prize_state_revision = application.prize_state_revision,
                application_sequence = application.application_sequence,
                "ignored stale prize effect application"
            );
        }
    } else if payload.first().copied()
        == Some(crate::movement_correction::MovementCorrectionApplied::TYPE)
    {
        let Ok(application) = crate::movement_correction::MovementCorrectionApplied::parse(payload)
        else {
            log_security_reject(player_id, "movement_correction", "malformed_application");
            return;
        };
        let mut state = state.lock().expect("server state should not be poisoned");
        match movement_correction_application_scope(&state, player_id, application) {
            MovementCorrectionApplicationScope::Active(runtime_identity) => {
                if !state.queue_movement_correction_application_for_runtime(
                    runtime_identity,
                    reliable_delivery_id,
                    application,
                ) {
                    log_security_reject(
                        player_id,
                        "movement_correction",
                        "application_queue_capacity",
                    );
                }
            }
            MovementCorrectionApplicationScope::ExactRetired => {
                tracing::debug!(
                    player_id,
                    life_seq = application.life_seq,
                    correction_generation = application.correction_generation,
                    detected_sequence = application.detected_sequence,
                    application_sequence = application.application_sequence,
                    "ignored exact correction application retired by a newer life"
                );
            }
            MovementCorrectionApplicationScope::Invalid => {
                log_security_reject(
                    player_id,
                    "movement_correction",
                    "stale_application_runtime",
                );
            }
        }
    } else if payload.first().copied() == Some(CLIENT_POSITION) {
        // The first exact position after a ship/life cutover is deliberately
        // application-reliable. Dispatch it through the same atomic embedded-
        // input admission path as ordinary realtime positions; reliable FIFO
        // ordering guarantees the preceding ship-change request was applied.
        let (identity, runtime_identity) = {
            let state = state.lock().expect("server state should not be poisoned");
            (
                state.active_player_session_identity(player_id),
                state.active_player_runtime_identity(player_id),
            )
        };
        if let Some(identity) = identity {
            tracing::debug!(
                player_id,
                payload_len = payload.len(),
                "dispatching reliable lifecycle position"
            );
            handle_authenticated_client_packet(
                state,
                identity,
                runtime_identity,
                payload.to_vec(),
                Some(reliable_delivery_id),
                peer,
                joined,
            );
        }
    } else if payload.first().copied() == Some(PASSWORD_LOGIN_CONTINUUM) {
        let Some(identity) = login_player_identity(payload) else {
            warn!(
                player_id,
                peer = %peer,
                reason = "invalid_identity",
                "server login rejected"
            );
            send_reliable_to(
                state,
                player_id,
                login_failure_message("ZUBSPACE_ID_INVALID"),
            );
            return;
        };
        let name = normalized_login_name(login_name(payload).as_deref().unwrap_or_default())
            .unwrap_or_default();
        if let Err(error) = admit_player_login(state, player_id, identity.clone(), name.clone()) {
            warn!(player_id, peer = %peer, reason = error.reason(), "server login rejected");
            send_reliable_to(state, player_id, login_failure_message(error.code()));
            return;
        }
        info!(
            player_id,
            identity = %identity,
            name = %name,
            peer = %peer,
            "server login accepted"
        );
        send_reliable_to(state, player_id, password_response());
    } else if payload.first().copied() == Some(ARENA_LOGIN) && !*joined {
        if state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&player_id)
            .is_some_and(|client| client.joined)
        {
            // The connection-local gate closes as soon as a reliable leave is
            // retained, while authoritative joined state stays live until its
            // ordered objective prefix terminals on the server tick. A new
            // arena login is valid only after that leave commits; never let it
            // jump the pending lifecycle barrier synchronously.
            log_security_reject(player_id, "arena", "join_before_leave_terminal");
            return;
        }
        if !player_login_accepted(state, player_id) {
            warn!(
                player_id,
                peer = %peer,
                reason = "not_logged_in",
                "server arena join rejected"
            );
            send_reliable_to(
                state,
                player_id,
                login_failure_message("ZUBSPACE_ID_INVALID"),
            );
            return;
        }
        let ship = normalized_ship(payload.get(1).copied().unwrap_or(0));
        // Older browsers end the login at byte 27 and declare revision 0.
        if let Some(client) = state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get_mut(&player_id)
        {
            client.protocol_revision = payload
                .get(ARENA_LOGIN_PROTOCOL_REVISION_OFFSET)
                .copied()
                .unwrap_or(0);
        }
        if let Err(reason) = join_arena_for_login(state, player_id, ship, payload) {
            warn!(
                player_id,
                peer = %peer,
                reason,
                "server arena join rejected"
            );
            send_reliable_to(state, player_id, login_failure_message(reason));
            return;
        }
        let (name, frequency, zone) = {
            let state = state.lock().expect("server state should not be poisoned");
            let client = state
                .clients
                .get(&player_id)
                .expect("joining client should remain registered");
            (
                client.player.name.clone(),
                client.player.frequency,
                state.config.default_zone.clone(),
            )
        };
        info!(
            player_id,
            name = %name,
            ship,
            frequency,
            zone = %zone,
            peer = %peer,
            "server arena joined"
        );
        send_to(state, player_id, time_sync_response_for_token(0));
        *joined = true;
    } else if payload.first().copied() == Some(LEAVE_ARENA_REQUEST) && *joined {
        if queue_reliable_gameplay_mutation(
            state,
            player_id,
            reliable_delivery_id,
            PendingReliableGameplayMutationKind::Leave,
        ) {
            // Stop admitting joined-only traffic from this connection while
            // the server tick waits for any earlier objective action to reach
            // a terminal. Authoritative joined state remains intact until the
            // ordered mutation itself commits.
            *joined = false;
            info!(player_id, peer = %peer, "server arena leave queued");
        }
    } else if payload.first().copied() == Some(DEATH_REQUEST) && *joined {
        log_security_reject(player_id, "death", "client_reported_death_disabled");
    } else if payload.first().copied() == Some(FILE_REQUEST) && *joined {
        let Some(identity) = state
            .lock()
            .expect("server state should not be poisoned")
            .active_player_session_identity(player_id)
        else {
            return;
        };
        if let Some((request_ordinal, suppression_budget)) =
            qa_drop_initial_map_file_request(payload)
        {
            info!(
                player_id,
                session = ?identity.session_id,
                file_index = 0,
                request_ordinal,
                suppression_budget,
                "server test initial map file request suppressed"
            );
            return;
        }
        if !serve_file_request(state, identity, payload) {
            log_security_reject(player_id, "map", "invalid_file_request");
        }
    } else if payload.first().copied() == Some(CACHED_MAP_CHECK) && *joined {
        let Some(identity) = state
            .lock()
            .expect("server state should not be poisoned")
            .active_player_session_identity(player_id)
        else {
            return;
        };
        if !serve_cached_map_check(state, identity, payload) {
            log_security_reject(player_id, "map", "invalid_cached_map_check");
        }
    } else if payload.first().copied() == Some(NEWS_REQUEST) {
        serve_news_request(state, player_id);
    } else if payload.first().copied() == Some(SPECTATE_REQUEST) {
        update_spectate(state, player_id, payload);
    } else if payload.first().copied() == Some(FLAG_REQUEST) {
        relay_flag_request(state, player_id, payload);
    } else if payload.first().copied() == Some(DROP_FLAGS_REQUEST) {
        relay_drop_flags(state, player_id, payload);
    } else if payload.first().copied() == Some(BALL_PICKUP_REQUEST) {
        relay_ball_pickup(state, player_id, payload);
    } else if payload.first().copied() == Some(BALL_FIRE_REQUEST) {
        relay_ball_fire(state, player_id, payload);
    } else if payload.first().copied() == Some(BALL_GOAL_REQUEST) {
        relay_ball_goal(state, player_id, payload);
    } else if payload.first().copied() == Some(CHAT_MESSAGE_REQUEST) {
        relay_chat(state, player_id, payload);
    } else if payload.first().copied() == Some(SET_BANNER_REQUEST) {
        update_banner(state, player_id, payload);
    } else if payload.first().copied() == Some(SNAPSHOT_ACK_TYPE) {
        record_snapshot_ack(state, player_id, payload);
    } else if payload.first().copied() == Some(SNAPSHOT_FULL_REQUEST_TYPE) {
        record_snapshot_full_request(state, player_id, payload);
    }
}

fn queue_reliable_gameplay_mutation(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    reliable_delivery_id: u32,
    kind: PendingReliableGameplayMutationKind,
) -> bool {
    let mut state = state.lock().expect("server state should not be poisoned");
    let Some(identity) = state.active_player_runtime_identity(player_id) else {
        // No current runtime can ever admit this mutation. Terminal it so a
        // pre-join/stale application cannot wedge the connection's reliable
        // dispatch backlog forever.
        return true;
    };
    let queued = state.queue_reliable_gameplay_for_runtime(identity, reliable_delivery_id, kind);
    if !queued {
        log_security_reject(player_id, "objective", "reliable_gameplay_queue_capacity");
    }
    queued
}

/// Wire type byte for Phase 3e-4a snapshot ack packets. Must match
/// `src/protocol/snapshot.rs::SnapshotAckPacket::TYPE` — same local
/// copy precedent as `CLIENT_INPUT_RECORD_TYPE` since the server bin
/// doesn't pull `crate::protocol` into its module tree.
pub(crate) const SNAPSHOT_ACK_TYPE: u8 = crate::snapshot_wire::SnapshotAckPacket::TYPE;
pub(crate) const SNAPSHOT_FULL_REQUEST_TYPE: u8 =
    crate::snapshot_wire::SnapshotFullRequestPacket::TYPE;

fn record_snapshot_ack(state: &Arc<Mutex<ServerState>>, player_id: u16, payload: &[u8]) {
    record_snapshot_ack_at(state, player_id, payload, Instant::now());
}

#[cfg_attr(not(test), allow(dead_code))]
/// Each input receipt names the newest snapshot the browser's game applied
/// (`latest_game_applied_snapshot_id`). The client keeps every snapshot it
/// decoded among its last 32 as a delta base, so that frame is a valid base
/// even though no `0x71` named it. Taking it as a base-only acknowledgement
/// lets deltas start from about one round trip ago instead of the last
/// periodic `0x71`, which shrinks them. It only moves `last_acked_snapshot_id`
/// forward, and only to a frame emitted to this exact runtime and life that is
/// still in history. RTT samples, Repel delivery and the ACK telemetry stay
/// with the explicit `0x71`.
pub(crate) fn advance_snapshot_base_from_receipt(
    client: &mut crate::state::ClientHandle,
    runtime: PlayerRuntimeIdentity,
    receipt: crate::combat_timeline::CombatViewReceiptV1,
) {
    let snapshot_id = receipt.latest_game_applied_snapshot_id;
    let forward = client
        .last_acked_snapshot_id
        .is_none_or(|last| crate::snapshot_wire::snapshot_id_is_forward(snapshot_id, last));
    if snapshot_id != 0
        && forward
        && receipt.observer_life_seq == runtime.life.life_seq
        && client
            .snapshot_history
            .iter()
            .any(|entry| entry.snapshot_id == snapshot_id)
        && client
            .observer_presentation_history
            .emitted_to(runtime, snapshot_id)
    {
        client.last_acked_snapshot_id = Some(snapshot_id);
    }
}

pub(crate) fn record_snapshot_ack_at(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    payload: &[u8],
    received_at: Instant,
) {
    let Ok(ack) = crate::snapshot_wire::SnapshotAckPacket::parse(payload) else {
        return;
    };
    let snapshot_id = ack.snapshot_id;
    let mut state = state.lock().expect("server state should not be poisoned");
    let Some(client) = state.clients.get_mut(&player_id) else {
        return;
    };
    if let Some(observation) = client.pending_repel_observations.front_mut() {
        if let Some(delivery) = observation
            .delivered_snapshots
            .iter_mut()
            .flatten()
            .find(|delivery| delivery.snapshot_id == snapshot_id)
        {
            delivery
                .acknowledged_at_tick
                .get_or_insert_with(crate::wire::current_tick);
            observation.delivery_acked = true;
            // Transport delivery is diagnostic only. Exact 0x79 application
            // admission is the sole boundary allowed to mutate movement
            // authority or retire repeated target-private delivery.
        }
    }
    // Acks may arrive out of order under packet loss; only advance
    // forward (with u16 wraparound tolerance — a "new" id within
    // `u16::MAX / 2` of the current value is treated as forward).
    let advance = match client.last_acked_snapshot_id {
        None => true,
        Some(last) => crate::snapshot_wire::snapshot_id_is_forward(snapshot_id, last),
    };
    if advance {
        client.last_acked_snapshot_id = Some(snapshot_id);
    }
    // Phase 2 polish (RTT): every ack — even an out-of-order one —
    // gives us a usable round-trip sample as long as the matching
    // history entry hasn't been evicted yet. Update the smoothed RTT
    // and re-derive the cached `lag_compensation_ticks`.
    let ack_sample = client
        .snapshot_history
        .iter()
        .find(|e| e.snapshot_id == snapshot_id)
        .map(|entry| received_at.saturating_duration_since(entry.emitted_at));
    if let Some(sample) = ack_sample {
        record_rtt_sample(client, sample);
    }
    let qa_event = client.qa_trace_bundle_id.map(|bundle_id| {
        (
            bundle_id,
            serde_json::json!({
                "type": "snapshot_codec_ack_received",
                "snapshotId": snapshot_id,
                "advancedWatermark": advance,
                "playerId": player_id,
                "arenaStreamId": client.arena_stream_id.to_string(),
                "measuredRttMicros": client.measured_rtt.map(|rtt| rtt.as_micros().min(u128::from(u64::MAX)) as u64),
            }),
        )
    });
    if let Some(sample) = ack_sample {
        state
            .telemetry
            .snapshot_ack
            .record(sample.as_micros().min(u128::from(u64::MAX)) as u64);
    }
    if let Some((bundle_id, event)) = qa_event {
        state.qa_traces.record_for_bundle(bundle_id, event);
    }
}

pub(crate) fn record_snapshot_full_request(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    payload: &[u8],
) {
    record_snapshot_full_request_at(state, player_id, payload, Instant::now());
}

pub(crate) fn record_snapshot_full_request_at(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    payload: &[u8],
    now: Instant,
) {
    if let Some(request) = crate::snapshot_wire::ProjectileRefreshRequestPacket::parse(payload) {
        let mut state = state.lock().expect("server state should not be poisoned");
        let admitted = state.clients.get(&player_id).is_some_and(|client| {
            client.joined
                && client.snapshot_ready
                && request.server_session_id == crate::wire::server_session_id()
                && request.arena_stream_id == client.arena_stream_id
                && client.last_projectile_refresh.is_none_or(|last| {
                    now.saturating_duration_since(last) >= Duration::from_secs(1)
                })
        });
        if admitted {
            let ids = state
                .active_projectiles
                .iter()
                .map(|projectile| projectile.id)
                .chain(
                    state
                        .active_decoys
                        .iter()
                        .filter(|decoy| decoy.presentation_source.is_some())
                        .map(|decoy| decoy.presentation_id),
                )
                .collect();
            if let Some(client) = state.clients.get_mut(&player_id) {
                client.projectile_refresh_pending = ids;
                client.last_projectile_refresh = Some(now);
            }
        }
        return;
    }
    if crate::snapshot_wire::SnapshotFullRequestPacket::parse(payload).is_err() {
        return;
    }
    let mut state = state.lock().expect("server state should not be poisoned");
    if let Some(client) = state.clients.get_mut(&player_id) {
        // Idempotent: duplicate reliable requests leave the same state. The
        // next emission cannot choose a delta base and is therefore FULL.
        client.last_acked_snapshot_id = None;
    }
}

/// Phase 2 polish (RTT): blend a new RTT sample into the client's
/// smoothed estimate and refresh the cached `lag_compensation_ticks`.
/// Exponential moving average with `alpha = 1/8` to track the long-
/// term RTT without being yanked around by single-tick jitter.
fn record_rtt_sample(client: &mut crate::state::ClientHandle, sample: std::time::Duration) {
    use crate::state::{
        MAX_LAG_COMPENSATION_TICKS, RTT_SMOOTHING_ALPHA_DEN, RTT_SMOOTHING_ALPHA_NUM,
    };
    let smoothed_nanos = match client.measured_rtt {
        None => sample.as_nanos() as u128,
        Some(prev) => {
            let prev_nanos = prev.as_nanos() as u128;
            let sample_nanos = sample.as_nanos() as u128;
            // new = ((DEN - NUM) * prev + NUM * sample) / DEN
            let weighted = prev_nanos * (RTT_SMOOTHING_ALPHA_DEN - RTT_SMOOTHING_ALPHA_NUM) as u128
                + sample_nanos * RTT_SMOOTHING_ALPHA_NUM as u128;
            weighted / RTT_SMOOTHING_ALPHA_DEN as u128
        }
    };
    let smoothed = std::time::Duration::from_nanos(smoothed_nanos as u64);
    client.measured_rtt = Some(smoothed);
    // Continuum tick = 10 ms; lag-comp rewind = RTT/2 in ticks.
    let half_rtt_ticks = (smoothed.as_millis() / 2 / 10) as u32;
    client.lag_compensation_ticks = half_rtt_ticks.min(MAX_LAG_COMPENSATION_TICKS as u32) as u8;
}

#[cfg(test)]
mod input_record_wire_tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn handler_phase_observation_is_optional_and_preserves_session_admission() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (sender, _receiver) = mpsc::channel();
        let identity = crate::arena::register_connection_with_sender(&state, sender.into());
        let mut joined = false;
        for observed in [false, true] {
            let started = Instant::now();
            // Unknown payload has no authority effects, but traverses the same
            // session/arena gates as ordinary authenticated packet dispatch.
            let phases = handle_client_packet_for_session(
                &state,
                identity,
                None,
                vec![0xff],
                "phase-test",
                &mut joined,
                observed,
            );
            assert_eq!(phases.is_some(), observed);
            if let Some(phases) = phases {
                let accounted = phases.arena_lookup_us
                    + phases.arena_wait_us
                    + phases.ingress_lookup_us
                    + phases.ingress_wait_us
                    + phases.identity_check_us
                    + phases.dispatch_us;
                assert!(u128::from(accounted) <= started.elapsed().as_micros());
            }
            assert!(!joined);
            assert!(state.lock().unwrap().player_session_matches(identity));
        }
        crate::arena::disconnect_player_session(&state, identity);
        // Instrumentation must not revive a retired session or fabricate
        // completed dispatch measurements for its rejected packet.
        assert!(
            handle_client_packet_for_session(
                &state,
                identity,
                None,
                vec![0xff],
                "phase-test",
                &mut joined,
                true,
            )
            .is_none()
        );
        assert!(state.lock().unwrap().clients.is_empty());
    }

    #[test]
    fn status_exposes_parseable_production_presentation_geometry_evidence() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let status: serde_json::Value = serde_json::Value::Object(server_status_snapshot(
            &state.lock().unwrap(),
            Instant::now(),
        ));
        assert_eq!(status["observer_network_profile"], "auto");
        assert_eq!(
            status["rtc_ingress_execution"],
            serde_json::json!({"limit":0,"in_flight":0,"waiting":0,"workers":0})
        );
        assert_eq!(status["presentation_geometry"]["lookups_total"], 0);
        assert_eq!(status["presentation_geometry"]["resolved_total"], 0);
        assert_eq!(status["presentation_geometry"]["unavailable_total"], 0);
        assert_eq!(status["presentation_geometry"]["server_native_total"], 0);
        assert_eq!(status["presentation_geometry"]["local_self_total"], 0);
        assert_eq!(status["presentation_geometry"]["server_advanced_total"], 0);
        assert_eq!(status["presentation_geometry"]["eval_nanos_total"], 0);
        assert_eq!(status["presentation_geometry"]["eval_nanos_max"], 0);
        assert_eq!(status["tick_timing"]["scope"], "server-lifetime");
        assert_eq!(status["tick_timing"]["interval"]["samples"], 0);
        assert_eq!(
            status["tick_timing"]["work"]["p99"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn status_records_real_tick_boundary_and_work_distributions_without_changing_ewma() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let now = Instant::now();
        {
            let mut state = state.lock().unwrap();
            state.config.tick_rate_hz = 60;
            state.advance_arena_tick(now);
            state.advance_arena_tick(now + Duration::from_micros(16_667));
            state.record_tick_wallclock(1000);
            state.record_tick_wallclock(2000);
            assert_eq!(state.tick_wallclock_ewma_us, 1125.0);
        }
        let status: serde_json::Value =
            serde_json::Value::Object(server_status_snapshot(&state.lock().unwrap(), now));
        let timing = &status["tick_timing"];
        assert_eq!(timing["interval"]["samples"], 1);
        assert_eq!(timing["interval"]["p99"]["lower_us"], 16_667);
        assert_eq!(timing["interval"]["p99"]["upper_us"], 16_667);
        assert_eq!(timing["absolute_jitter"]["max_us"], 1);
        // Both work samples land after both advances here, so both see a cadence baseline; the
        // withheld-sample path is `a_midlife_cadence_rebase_keeps_the_tick_timing_populations_comparable`.
        assert_eq!(timing["work"]["samples"], 2);
        assert_eq!(timing["work"]["max_us"], 2000);
    }

    /// A zone switch rebases the cadence baseline (`last_tick_at = None`, `arena/zones.rs`) so the
    /// switch itself is not charged as tick jitter. Every rebase lets the `work` population record a
    /// tick that the `interval` population cannot, and the browser gates assert that
    /// `work.samples` is at most `interval.samples + 1` - so a run that boots, joins one zone and
    /// switches to another (exactly what the aged-server gate does) eventually reports populations
    /// that no longer describe the same tick set.
    #[test]
    fn a_midlife_cadence_rebase_keeps_the_tick_timing_populations_comparable() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let now = Instant::now();
        {
            let mut state = state.lock().unwrap();
            state.config.tick_rate_hz = 60;
            for step in 0..5_u64 {
                state.advance_arena_tick(now + Duration::from_micros(16_667 * step));
                state.record_tick_wallclock(1000);
            }
            // The arena switch: cadence baseline dropped, lifetime histograms kept.
            state.last_tick_at = None;
            for step in 10..15_u64 {
                state.advance_arena_tick(now + Duration::from_micros(16_667 * step));
                state.record_tick_wallclock(1000);
            }
        }
        let status: serde_json::Value =
            serde_json::Value::Object(server_status_snapshot(&state.lock().unwrap(), now));
        let timing = &status["tick_timing"];
        assert_eq!(
            timing["interval"]["samples"], timing["absolute_jitter"]["samples"],
            "interval and jitter must count exactly the same ticks"
        );
        assert_eq!(
            timing["work"]["samples"], timing["interval"]["samples"],
            "boot plus one mid-life rebase must leave the work and tick-boundary populations \
             counting the same ticks: {timing}"
        );
    }

    /// The end of a tick is when its work sample is recorded, and `/v1/status` can be served between
    /// the tick's cadence sample and that record. The boundary sample therefore waits for the work
    /// sample, so a status snapshot taken while a tick is in flight still reports three equal
    /// populations - which is what `tests/e2e/server_timing_evidence.mjs` asserts.
    #[test]
    fn an_in_flight_tick_never_shows_a_tick_boundary_without_its_work_sample() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let now = Instant::now();
        {
            let mut state = state.lock().unwrap();
            state.config.tick_rate_hz = 60;
            for step in 0..5_u64 {
                state.advance_arena_tick(now + Duration::from_micros(16_667 * step));
                state.record_tick_wallclock(1000);
            }
            // The tick the driver has just started: boundary known, work duration not yet.
            state.advance_arena_tick(now + Duration::from_micros(16_667 * 5));
        }
        let status: serde_json::Value =
            serde_json::Value::Object(server_status_snapshot(&state.lock().unwrap(), now));
        let timing = &status["tick_timing"];
        assert_eq!(
            timing["work"]["samples"], timing["interval"]["samples"],
            "a status read during a tick must not report a boundary the work population has not \
             reached: {timing}"
        );
        assert_eq!(
            timing["absolute_jitter"]["samples"], timing["interval"]["samples"],
            "jitter must move with the interval population: {timing}"
        );
    }

    #[test]
    fn qa_trace_upload_is_disabled_by_default_and_exact_origin_bound_when_enabled() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let allowed_request = "POST /v1/qa/trace HTTP/1.1\r\nOrigin: http://pax:7000\r\n\r\n";
        assert_eq!(
            qa_trace_cors_origin(allowed_request, &state),
            Err(("404 Not Found", "not_found"))
        );
        {
            let mut state = state.lock().expect("state");
            state.config.qa_trace_upload = true;
            state.config.signaling_allowed_origins = vec!["http://pax:7000".to_string()];
        }
        assert_eq!(
            qa_trace_cors_origin(allowed_request, &state),
            Ok("http://pax:7000".to_string())
        );
        assert_eq!(
            qa_trace_cors_origin("POST /v1/qa/trace HTTP/1.1\r\n\r\n", &state),
            Err(("403 Forbidden", "origin_required"))
        );
        assert_eq!(
            qa_trace_cors_origin(
                "POST /v1/qa/trace HTTP/1.1\r\nOrigin: http://attacker.invalid\r\n\r\n",
                &state,
            ),
            Err(("403 Forbidden", "origin_not_allowed"))
        );
    }

    fn input_packet(count: usize) -> Vec<u8> {
        let mut packet = vec![CLIENT_INPUT_RECORD_TYPE, 0, 0, count as u8];
        for sequence in 0..count as u16 {
            packet.extend_from_slice(&[0, 0, 0, 0]);
            packet.extend_from_slice(
                &crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence).encode(),
            );
        }
        packet
    }

    #[test]
    #[ignore = "decode-cost measurement, not an acceptance gate"]
    fn measure_maximum_position_decode_cost() {
        use std::hint::black_box;
        let inputs = input_packet(CLIENT_INPUT_RECORD_MAX_INPUTS);
        let mut position = vec![0; crate::continuum_position::CLIENT_POSITION_EXTRA_LEN];
        position[0] = crate::continuum_position::CLIENT_POSITION_TYPE;
        crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(0, 0, 0, 0, 0)
            .encode(&mut position);
        let offset = position.len();
        position.extend_from_slice(&inputs);
        assert!(crate::continuum_position::parse_client_position_snapshot(&position).is_some());
        assert_eq!(
            parse_client_input_observations(&position[offset..])
                .unwrap()
                .len(),
            32
        );
        let iterations = 10_000;
        let mut timings = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            for _ in 0..iterations {
                black_box(crate::continuum_position::parse_client_position_snapshot(
                    black_box(&position),
                ))
                .unwrap();
                black_box(parse_client_input_observations(black_box(
                    &position[offset..],
                )))
                .unwrap();
            }
            timings.push(started.elapsed().as_nanos() as f64 / iterations as f64);
        }
        timings.sort_by(f64::total_cmp);
        println!(
            "{}",
            serde_json::json!({
                "measurement": "maximum_position_and_input_decode_only",
                "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
                "packetBytes": position.len(), "inputs": 32, "iterationsPerRound": iterations,
                "medianNsPerPacket": timings[2],
                "byteBudgetPackets": crate::rtc::RTC_APPLICATION_INBOUND_QUEUE_BYTES / position.len(),
                "excludes": "state locks, security/receipt checks, movement replay and network delivery"
            })
        );
    }

    fn encoded_input_packet(base_sequence: u16, inputs: &[(u16, u8)]) -> Vec<u8> {
        let mut packet = vec![
            CLIENT_INPUT_RECORD_TYPE,
            base_sequence as u8,
            (base_sequence >> 8) as u8,
            inputs.len() as u8,
        ];
        for (index, &(bits, direction)) in inputs.iter().enumerate() {
            packet.extend_from_slice(&bits.to_le_bytes());
            packet.push(direction);
            packet.push(0);
            let sequence = base_sequence.wrapping_add(index as u16);
            packet.extend_from_slice(
                &crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence).encode(),
            );
        }
        packet
    }

    fn encoded_door_input_packet(base_sequence: u16, inputs: &[(u16, u8, u8)]) -> Vec<u8> {
        let mut packet = vec![
            CLIENT_INPUT_RECORD_TYPE,
            base_sequence as u8,
            (base_sequence >> 8) as u8,
            inputs.len() as u8,
        ];
        for (index, &(bits, direction, door_seed)) in inputs.iter().enumerate() {
            packet.extend_from_slice(&bits.to_le_bytes());
            packet.push(direction);
            packet.push(door_seed);
            let sequence = base_sequence.wrapping_add(index as u16);
            packet.extend_from_slice(
                &crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence).encode(),
            );
        }
        packet
    }

    fn encoded_door_input_packet_with_receipts(
        base_sequence: u16,
        inputs: &[(u16, u8, u8, crate::combat_timeline::CombatViewReceiptV1)],
    ) -> Vec<u8> {
        let mut packet = vec![
            CLIENT_INPUT_RECORD_TYPE,
            base_sequence as u8,
            (base_sequence >> 8) as u8,
            inputs.len() as u8,
        ];
        for &(bits, direction, door_seed, receipt) in inputs {
            packet.extend_from_slice(&bits.to_le_bytes());
            packet.push(direction);
            packet.push(door_seed);
            packet.extend_from_slice(&receipt.encode());
        }
        packet
    }

    #[test]
    fn combat_view_receipt_rejects_wrong_version_and_cross_input_rebinding() {
        let packet = encoded_input_packet(77, &[(1, 4)]);
        let receipt_offset = CLIENT_INPUT_RECORD_HEADER_LEN + 4;
        let mut wrong_version = packet.clone();
        wrong_version[receipt_offset] =
            crate::combat_timeline::COMBAT_VIEW_PROTOCOL_VERSION.wrapping_add(1);
        assert_eq!(
            parse_client_input_observations(&wrong_version),
            Err("invalid_combat_view_receipt")
        );

        let mut wrong_sequence = packet;
        wrong_sequence[receipt_offset + 6..receipt_offset + 8]
            .copy_from_slice(&78_u16.to_le_bytes());
        assert_eq!(
            parse_client_input_observations(&wrong_sequence),
            Err("combat_view_sequence_mismatch")
        );
    }

    #[test]
    fn input_admission_freezes_exact_combat_view_without_blocking_movement_recovery() {
        let tick = 1_000;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        let (sequence, directive, door_seed) = {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.life_position_sequence_base = None;
            let door_state = crate::doors::ClientDoorState::for_challenge(0);
            client.door_state = Some(door_state);
            let door_seed =
                crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick)[1].seed;
            let directive = client
                .observer_combat_timeline
                .current_directive()
                .expect("join directive");
            assert_eq!(
                client.observer_presentation_history.observe_emitted(
                    crate::combat_timeline::ObserverPresentationFrameV1 {
                        observer: identity,
                        arena_stream_id: client.arena_stream_id,
                        snapshot_id: 50,
                        server_tick: 994,
                        actors: Vec::new(),
                    },
                ),
                crate::combat_timeline::ObserverPresentationAppendDisposition::Applied
            );
            (
                directive.effective_from_input_sequence,
                directive,
                door_seed,
            )
        };
        let receipt = crate::combat_timeline::CombatViewReceiptV1 {
            observer_life_seq: identity.life.life_seq,
            input_sequence: sequence,
            timeline_generation: directive.generation,
            latest_game_applied_snapshot_id: 50,
            flags: 0,
        };
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet_with_receipts(sequence, &[(0, 0, door_seed, receipt)],),
            tick,
        ));
        let frozen = state.lock().expect("state").clients[&player_id]
            .observer_combat_timeline
            .decision_for_sequence(sequence)
            .expect("verified input view");
        assert_eq!(frozen.applied_snapshot_id, 50);

        let missing_sequence = sequence.wrapping_add(1);
        let missing = crate::combat_timeline::CombatViewReceiptV1 {
            input_sequence: missing_sequence,
            latest_game_applied_snapshot_id: 51,
            ..receipt
        };
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet_with_receipts(
                missing_sequence,
                &[(1, 0, door_seed, missing)],
            ),
            tick,
        ));
        assert_eq!(
            state.lock().expect("state").clients[&player_id]
                .observer_combat_timeline
                .decision_for_sequence(missing_sequence),
            None,
            "unknown view proof does not discard movement but grants no combat authority"
        );

        let corrected = crate::combat_timeline::CombatViewReceiptV1 {
            input_sequence: missing_sequence,
            ..receipt
        };
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet_with_receipts(
                missing_sequence,
                &[(1, 0, door_seed, corrected)],
            ),
            tick,
        ));
        assert_eq!(
            state.lock().expect("state").clients[&player_id]
                .observer_combat_timeline
                .decision_for_sequence(missing_sequence),
            None,
            "a rejected first proof cannot be upgraded by retransmission"
        );
    }

    #[test]
    fn delayed_auto_receipt_keeps_movement_but_never_gains_combat_authority() {
        use crate::combat_timeline::{
            CombatViewReceiptV1, ObserverNetworkProfile, ObserverTimelineDirectiveV1,
            ObserverTimelineReceiptRejection,
        };
        let (state, identity, player_id) = joined_runtime_at_tick(700, 0);
        let (old_generation, offered_generation, door_seed) = {
            let mut locked = state.lock().unwrap();
            let client = locked.clients.get_mut(&player_id).unwrap();
            client.life_position_sequence_base = None;
            let doors = crate::doors::ClientDoorState::for_challenge(0);
            client.door_state = Some(doors);
            let initial = client
                .observer_combat_timeline
                .begin_scope(
                    identity,
                    client.arena_stream_id,
                    ObserverNetworkProfile::AutomaticWan,
                    2,
                    700,
                    100,
                )
                .unwrap();
            client
                .observer_combat_timeline
                .issue_delay_schedule(ObserverTimelineDirectiveV1 {
                    generation: 4,
                    issued_server_tick: 860,
                    effective_server_tick: 910,
                    effective_from_input_sequence: 102,
                    target_delay_ticks: 7,
                    ..initial
                })
                .unwrap();
            let seed = crate::doors::client_door_phase_candidates_at_tick(Some(doors), 936)[1].seed;
            (2, 4, seed)
        };
        // The expired snapshot need not still exist. A missing/old receipt
        // cannot authorize a weapon; it must not discard ordinary movement.
        let old = CombatViewReceiptV1 {
            observer_life_seq: identity.life.life_seq,
            input_sequence: 102,
            timeline_generation: old_generation,
            latest_game_applied_snapshot_id: 93,
            flags: 1,
        };
        let packet = encoded_door_input_packet_with_receipts(102, &[(1, 0, door_seed, old)]);
        for _ in 0..3 {
            assert!(record_client_input_observation_at_tick(
                &state, identity, &packet, 936
            ));
        }
        {
            let locked = state.lock().unwrap();
            let client = &locked.clients[&player_id];
            assert!(
                client
                    .recent_input_records
                    .iter()
                    .any(|record| record.sequence == 102 && record.input.bits == 1)
            );
            assert!(
                client
                    .observer_combat_timeline
                    .decision_for_sequence(102)
                    .is_none()
            );
            assert_eq!(
                client
                    .observer_combat_timeline
                    .receipt_rejection_for_sequence(102),
                Some(ObserverTimelineReceiptRejection::SupersededGeneration)
            );
        }
        assert_eq!(
            crate::security::peek_security_reject_count_for_test(
                "input_record",
                "combat_view_generation_not_effective"
            ),
            0
        );
        // Rewriting the already-frozen proof is still a hard security reject.
        let rewritten = CombatViewReceiptV1 {
            timeline_generation: offered_generation,
            ..old
        };
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet_with_receipts(102, &[(1, 0, door_seed, rewritten)]),
            936
        ));
        assert_eq!(
            crate::security::peek_security_reject_count_for_test(
                "input_record",
                "combat_view_duplicate_mismatch"
            ),
            1
        );
        assert!(
            state.lock().unwrap().clients[&player_id]
                .observer_combat_timeline
                .decision_for_sequence(102)
                .is_none()
        );
    }

    fn joined_runtime_at_tick(
        tick: u32,
        rockets: u8,
    ) -> (Arc<Mutex<ServerState>>, PlayerRuntimeIdentity, u16) {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena_at_tick(&state, player_id, 0, tick);
        let identity = {
            let mut state = state.lock().expect("state");
            let identity = state
                .active_player_runtime_identity(player_id)
                .expect("joined runtime");
            state
                .clients
                .get_mut(&player_id)
                .expect("client")
                .server_weapons
                .rockets = rockets;
            identity
        };
        (state, identity, player_id)
    }

    fn canonical_bits(state: &Arc<Mutex<ServerState>>, player_id: u16, sequence: u16) -> u16 {
        state
            .lock()
            .expect("state")
            .clients
            .get(&player_id)
            .expect("client")
            .recent_input_records
            .iter()
            .find(|record| record.sequence == sequence)
            .expect("canonical record")
            .input
            .bits
    }

    #[test]
    fn wire_bundle_cap_stays_32_while_server_retention_is_wider() {
        assert!(
            crate::state::MAX_RECENT_INPUT_OBSERVATIONS > CLIENT_INPUT_RECORD_MAX_INPUTS,
            "overlapping wire bundles need wider bounded server retention"
        );
        assert_eq!(
            parse_client_input_observations(&input_packet(32))
                .expect("32-entry bundle")
                .len(),
            32
        );
        assert_eq!(
            parse_client_input_observations(&input_packet(33)),
            Err("count_above_max")
        );
    }

    #[test]
    fn long_session_sequence_reuse_discards_stale_door_phase_alias() {
        let tick = 34_000;
        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let current_phase =
            crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick)[1];
        let stale_phase_tick = crate::continuum::continuum_tick_sub(tick, 20_000);
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client.last_input_seq = Some(163);
            client.last_input_record = Some(crate::state::ClientInputObservation {
                bits: 0,
                direction: 0,
            });
            client.recent_input_records.extend([
                crate::state::SequencedInputObservation {
                    sequence: 100,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: current_phase.seed,
                    door_phase_tick: stale_phase_tick,
                },
                crate::state::SequencedInputObservation {
                    sequence: 163,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: current_phase.seed,
                    door_phase_tick: current_phase.started_tick,
                },
            ]);
            client.recent_input_rocket_assertions.push_back((100, true));
        }

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(
                99,
                &[(0, 0, current_phase.seed), (0, 0, current_phase.seed),],
            ),
            tick,
        ));

        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert!(
            client
                .recent_input_records
                .iter()
                .all(|record| record.sequence != 99),
            "the record exactly outside the 64-entry retention window stays inert",
        );
        let reused = client
            .recent_input_records
            .iter()
            .find(|record| record.sequence == 100)
            .expect("reused in-window sequence");
        assert_ne!(reused.door_phase_tick, stale_phase_tick);
        assert!(
            crate::continuum::continuum_tick_diff(tick, reused.door_phase_tick).unsigned_abs()
                <= crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS,
            "the reused raw sequence resolves in the current authenticated door window",
        );
        assert_eq!(
            client
                .recent_input_rocket_assertions
                .iter()
                .filter(|(sequence, _)| *sequence == 100)
                .copied()
                .collect::<Vec<_>>(),
            vec![(100, false)],
            "the stale raw rocket assertion cannot alias the reused sequence",
        );
    }

    #[test]
    fn delayed_rolling_window_reuses_same_seed_upper_phase_without_backtrack() {
        let tick = 16_000;
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let upper_tick = crate::continuum::continuum_tick_sub(
            tick,
            crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS
                .saturating_add(delay)
                .saturating_add(2_000),
        );
        let upper_phase =
            crate::doors::client_door_phase_candidates_at_tick(Some(door_state), upper_tick)[1];
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client.life_position_sequence_base = None;
            client.last_position_sequence = Some(64);
            client.last_input_seq = Some(64);
            client.last_input_record = Some(crate::state::ClientInputObservation {
                bits: 0,
                direction: 0,
            });
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: 64,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: upper_phase.seed,
                    door_phase_tick: upper_phase.started_tick,
                });
            client.recent_input_rocket_assertions.push_back((64, false));
        }

        let recovered = vec![(0, 0, upper_phase.seed); 32];
        let packet = encoded_door_input_packet(33, &recovered);
        {
            let locked = state.lock().expect("state");
            let client = &locked.clients[&player_id];
            let parsed = parse_client_input_observations(&packet).expect("rolling input packet");
            resolve_client_input_door_phases(client, &parsed, 64, tick)
                .expect("same-seed upper witness resolves the delayed rolling window");
        }
        assert!(record_client_input_observation_at_tick(
            &state, identity, &packet, tick,
        ));

        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert_eq!(client.last_input_seq, Some(64));
        assert_eq!(client.recent_input_records.len(), 32);
        assert!(client.recent_input_records.iter().all(|record| {
            record.door_seed == upper_phase.seed
                && record.door_phase_tick == upper_phase.started_tick
        }));
    }

    #[test]
    fn delayed_rolling_window_recovers_authenticated_phase_before_upper_witness() {
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let (tick, predecessor, upper) = (3_u32..512)
            .find_map(|count| {
                let current_start = count.saturating_mul(delay);
                let tick = crate::continuum::continuum_tick_add(current_start, delay / 2);
                let predecessor_start =
                    crate::continuum::continuum_tick_sub(current_start, delay.saturating_mul(2));
                let upper_start = crate::continuum::continuum_tick_sub(current_start, delay);
                let phases = crate::doors::client_door_phase_sequence(
                    Some(door_state),
                    predecessor_start,
                    upper_start,
                )?;
                let predecessor = phases[0];
                let upper = phases[1];
                let receipt =
                    crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
                receipt
                    .iter()
                    .all(|phase| phase.seed != predecessor.seed)
                    .then_some((tick, predecessor, upper))
            })
            .expect("authenticated history has a predecessor outside the receipt window");
        let upper_sequence = 28_016;
        let predecessor_sequence = upper_sequence - 21;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client.life_position_sequence_base = None;
            client.last_position_sequence = Some(upper_sequence);
            client.last_input_seq = Some(upper_sequence);
            client.last_input_record = Some(crate::state::ClientInputObservation {
                bits: 0,
                direction: 0,
            });
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: upper_sequence,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: upper.seed,
                    door_phase_tick: upper.started_tick,
                });
            client
                .recent_input_rocket_assertions
                .push_back((upper_sequence, false));
        }

        let packet = encoded_door_input_packet(predecessor_sequence, &[(0, 0, predecessor.seed)]);
        assert!(record_client_input_observation_at_tick(
            &state, identity, &packet, tick,
        ));

        let locked = state.lock().expect("state");
        let record = locked.clients[&player_id]
            .recent_input_records
            .iter()
            .find(|record| record.sequence == predecessor_sequence)
            .expect("recovered predecessor record");
        assert_eq!(record.door_seed, predecessor.seed);
        assert_eq!(record.door_phase_tick, predecessor.started_tick);
        assert_eq!(
            locked.clients[&player_id].last_input_seq,
            Some(upper_sequence)
        );
    }

    #[test]
    fn scheduler_stalled_predecessor_beyond_history_is_inert_not_a_security_claim() {
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let (tick, predecessor, upper) = (8_u32..512)
            .find_map(|count| {
                let upper_start = count.saturating_mul(delay);
                let predecessor_start =
                    crate::continuum::continuum_tick_sub(upper_start, delay.saturating_mul(4));
                let phases = crate::doors::client_door_phase_sequence(
                    Some(door_state),
                    predecessor_start,
                    upper_start,
                )?;
                let predecessor = phases[0];
                let upper = *phases.last()?;
                let tick = upper_start.saturating_add(delay / 2);
                let ordinary =
                    crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
                let predecessor_ticks = retained_input_history_ticks().saturating_add(delay);
                let retained_bound =
                    crate::continuum::continuum_tick_sub(upper_start, predecessor_ticks);
                let retained_start = crate::doors::client_door_phase_candidates_at_tick(
                    Some(door_state),
                    retained_bound,
                )
                .into_iter()
                .filter(|phase| {
                    crate::continuum::continuum_tick_diff(retained_bound, phase.started_tick) >= 0
                })
                .max_by_key(|phase| {
                    crate::continuum::continuum_tick_diff(phase.started_tick, retained_bound)
                })?;
                let retained = crate::doors::client_door_phase_sequence(
                    Some(door_state),
                    retained_start.started_tick,
                    upper_start,
                )?;
                (ordinary
                    .iter()
                    .chain(retained.iter())
                    .all(|phase| phase.seed != predecessor.seed))
                .then_some((tick, predecessor, upper))
            })
            .expect("fixture needs a unique predecessor beyond bounded recovery");
        let upper_sequence = 28_042_u16;
        let predecessor_sequence = upper_sequence.wrapping_sub(31);
        let (state, _identity, player_id) = joined_runtime_at_tick(tick, 0);
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client.life_position_sequence_base = None;
            client.last_position_sequence = Some(upper_sequence);
            client.last_input_seq = Some(upper_sequence);
            client.last_input_record = Some(crate::state::ClientInputObservation {
                bits: 0,
                direction: 0,
            });
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: upper_sequence,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: upper.seed,
                    door_phase_tick: upper.started_tick,
                });
            client
                .recent_input_rocket_assertions
                .push_back((upper_sequence, false));
        }

        let packet = encoded_door_input_packet(predecessor_sequence, &[(0, 0, predecessor.seed)]);
        let parsed = parse_client_input_observations(&packet).expect("stale rolling input");
        let locked = state.lock().expect("state");
        assert_eq!(
            resolve_client_input_door_phases(
                &locked.clients[&player_id],
                &parsed,
                upper_sequence,
                tick,
            ),
            Err(InputDoorPhaseResolutionError::HistoricalPredecessorUnavailable),
            "an unprovable predecessor behind exact upper authority is unavailable, not forged",
        );
    }

    #[test]
    fn unavailable_predecessor_seed_aliasing_a_future_phase_remains_inert() {
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let (tick, upper, future_alias) = (8_u32..2_048)
            .find_map(|count| {
                let upper_start = count.saturating_mul(delay);
                let tick = upper_start.saturating_add(delay);
                let upper = crate::doors::client_door_phase_candidates_at_tick(
                    Some(door_state),
                    upper_start,
                )[1];
                let receipt =
                    crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
                let future_alias = *receipt.last()?;
                let predecessor_ticks = retained_input_history_ticks().saturating_add(delay);
                let predecessor_bound =
                    crate::continuum::continuum_tick_sub(upper.started_tick, predecessor_ticks);
                let predecessor_start = crate::doors::client_door_phase_candidates_at_tick(
                    Some(door_state),
                    predecessor_bound,
                )
                .into_iter()
                .filter(|phase| {
                    crate::continuum::continuum_tick_diff(predecessor_bound, phase.started_tick)
                        >= 0
                })
                .max_by_key(|phase| {
                    crate::continuum::continuum_tick_diff(phase.started_tick, predecessor_bound)
                })?;
                let predecessors = crate::doors::client_door_phase_sequence(
                    Some(door_state),
                    predecessor_start.started_tick,
                    upper.started_tick,
                )?;
                (future_alias.started_tick > tick
                    && predecessors
                        .iter()
                        .all(|phase| phase.seed != future_alias.seed))
                .then_some((tick, upper, future_alias))
            })
            .expect("fixture needs a future byte alias absent from retained predecessors");
        let upper_sequence = 31_000_u16;
        let predecessor_sequence = upper_sequence.wrapping_sub(31);
        let (state, _identity, player_id) = joined_runtime_at_tick(tick, 0);
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client.life_position_sequence_base = None;
            client.last_position_sequence = Some(upper_sequence);
            client.last_input_seq = Some(upper_sequence);
            client.recent_input_records.clear();
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: upper_sequence,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: upper.seed,
                    door_phase_tick: upper.started_tick,
                });
        }

        let packet = encoded_door_input_packet(predecessor_sequence, &[(0, 0, future_alias.seed)]);
        let parsed = parse_client_input_observations(&packet).expect("stale rolling input");
        let locked = state.lock().expect("state");
        assert_eq!(
            resolve_client_input_door_phases(
                &locked.clients[&player_id],
                &parsed,
                upper_sequence,
                tick,
            ),
            Err(InputDoorPhaseResolutionError::HistoricalPredecessorUnavailable),
            "a future byte alias cannot turn an inert predecessor into a security claim",
        );
    }

    #[test]
    fn delayed_rolling_window_keeps_life_baseline_across_multi_phase_browser_stall() {
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let (tick, baseline, upper) = (8_u32..512)
            .find_map(|count| {
                let upper_start = count.saturating_mul(delay);
                let tick = upper_start.saturating_add(delay / 2);
                let baseline_start =
                    crate::continuum::continuum_tick_sub(upper_start, delay.saturating_mul(3));
                let baseline = crate::doors::client_door_phase_candidates_at_tick(
                    Some(door_state),
                    baseline_start,
                )[1];
                let upper = crate::doors::client_door_phase_candidates_at_tick(
                    Some(door_state),
                    upper_start,
                )[1];
                let ordinary_maximum_age =
                    crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS
                        .saturating_add(delay)
                        .saturating_add(retained_input_history_ticks());
                (baseline.seed != upper.seed
                    && crate::continuum::continuum_tick_diff(tick, baseline.started_tick) as u32
                        > ordinary_maximum_age)
                    .then_some((tick, baseline, upper))
            })
            .expect("fixture needs a life baseline older than ordinary phase retention");
        let baseline_sequence = 32_028_u16;
        let delayed_sequence = baseline_sequence.wrapping_add(1);
        let upper_sequence = baseline_sequence.wrapping_add(2);
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client.life_position_sequence_base = Some(baseline_sequence);
            client.last_position_sequence = Some(upper_sequence);
            client.last_input_seq = Some(upper_sequence);
            client.last_input_record = Some(crate::state::ClientInputObservation {
                bits: 0,
                direction: 0,
            });
            client.recent_input_records.extend([
                crate::state::SequencedInputObservation {
                    sequence: baseline_sequence,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: baseline.seed,
                    door_phase_tick: baseline.started_tick,
                },
                crate::state::SequencedInputObservation {
                    sequence: upper_sequence,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: upper.seed,
                    door_phase_tick: upper.started_tick,
                },
            ]);
            client
                .recent_input_rocket_assertions
                .extend([(baseline_sequence, false), (upper_sequence, false)]);
        }

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(delayed_sequence, &[(0, 0, baseline.seed)]),
            tick,
        ));

        let locked = state.lock().expect("state");
        let delayed = locked.clients[&player_id]
            .recent_input_records
            .iter()
            .find(|record| record.sequence == delayed_sequence)
            .expect("delayed record after exact life baseline");
        assert_eq!(delayed.door_seed, baseline.seed);
        assert_eq!(delayed.door_phase_tick, baseline.started_tick);
        assert_eq!(
            locked.clients[&player_id].last_input_seq,
            Some(upper_sequence)
        );
    }

    #[test]
    fn delayed_rolling_window_keeps_canonical_position_phase_across_server_hitch() {
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let door_state = crate::doors::ClientDoorState::for_challenge(0);
        let (tick, lower, upper) = (8_u32..512)
            .find_map(|count| {
                let tick = count.saturating_mul(delay).saturating_add(delay / 2);
                let receipt =
                    crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
                let upper = receipt[1];
                let lower_tick = crate::continuum::continuum_tick_sub(
                    upper.started_tick,
                    delay.saturating_mul(4),
                );
                let lower = crate::doors::client_door_phase_candidates_at_tick(
                    Some(door_state),
                    lower_tick,
                )[1];
                (receipt.iter().all(|phase| phase.seed != lower.seed)
                    && crate::continuum::continuum_tick_diff(tick, lower.started_tick) as u32
                        > crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS
                            .saturating_add(delay)
                            .saturating_add(retained_input_history_ticks()))
                .then_some((tick, lower, upper))
            })
            .expect("fixture needs a committed phase older than rolling plausibility");
        let lower_sequence = 29_630;
        let delayed_sequence = 29_646;
        let upper_sequence = 29_677;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client.life_position_sequence_base = None;
            client.last_position_sequence = Some(lower_sequence);
            client.last_input_seq = Some(upper_sequence);
            client.last_input_record = Some(crate::state::ClientInputObservation {
                bits: 0,
                direction: 0,
            });
            client.recent_input_records.extend([
                crate::state::SequencedInputObservation {
                    sequence: lower_sequence,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: lower.seed,
                    door_phase_tick: lower.started_tick,
                },
                crate::state::SequencedInputObservation {
                    sequence: upper_sequence,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: upper.seed,
                    door_phase_tick: upper.started_tick,
                },
            ]);
            client
                .recent_input_rocket_assertions
                .extend([(lower_sequence, false), (upper_sequence, false)]);
        }

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(delayed_sequence, &[(0, 0, lower.seed)]),
            tick,
        ));

        let locked = state.lock().expect("state");
        let delayed = locked.clients[&player_id]
            .recent_input_records
            .iter()
            .find(|record| record.sequence == delayed_sequence)
            .expect("delayed record behind the current upper witness");
        assert_eq!(delayed.door_seed, lower.seed);
        assert_eq!(delayed.door_phase_tick, lower.started_tick);
        assert_eq!(
            locked.clients[&player_id].last_input_seq,
            Some(upper_sequence)
        );
    }

    #[test]
    fn safe_zone_weapon_stop_is_an_exact_wire_bit_and_higher_bits_stay_reserved() {
        let stop = crate::gameplay_rules::SAFE_ZONE_WEAPON_STOP_BIT;
        let parsed = parse_client_input_observations(&encoded_input_packet(91, &[(stop, 17)]))
            .expect("safe-stop input bit");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].sequence, 91);
        assert_eq!(parsed[0].input.bits, stop);
        assert_eq!(parsed[0].input.direction, 17);
        assert_eq!(
            parse_client_input_observations(&encoded_input_packet(91, &[(1 << 7, 17)])),
            Err("unknown_bits"),
        );
    }

    #[test]
    fn queued_input_recovers_the_next_authenticated_phase_outside_the_receipt_window() {
        let tick: u32 = 10_000;
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        let door_state = crate::doors::ClientDoorState::for_challenge(
            crate::continuum::continuum_tick_sub(tick, delay.saturating_mul(8)),
        );
        let receipt_phases =
            crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
        let receipt_current = receipt_phases[1];
        let lower_tick = crate::continuum::continuum_tick_sub(
            receipt_current.started_tick,
            delay.saturating_mul(3),
        );
        let recovered = crate::doors::client_door_phase_sequence(
            Some(door_state),
            lower_tick,
            receipt_current.started_tick,
        )
        .expect("challenge-derived phase history");
        let lower = recovered[0];
        let delayed_next = recovered[1];
        assert!(
            receipt_phases
                .iter()
                .all(|phase| phase.started_tick != delayed_next.started_tick)
        );
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: 9,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: lower.seed,
                    door_phase_tick: lower.started_tick,
                });
            client.last_input_seq = Some(9);
            client.last_input_record = Some(crate::state::ClientInputObservation {
                bits: 0,
                direction: 0,
            });
        }

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(10, &[(0, 0, delayed_next.seed)]),
            tick,
        ));
        let locked = state.lock().expect("state");
        let admitted = locked.clients[&player_id]
            .recent_input_records
            .iter()
            .find(|record| record.sequence == 10)
            .expect("delayed record");
        assert_eq!(admitted.door_phase_tick, delayed_next.started_tick);
        assert_eq!(admitted.door_seed, delayed_next.seed);
    }

    #[test]
    fn server_hitch_accepts_browser_phase_frozen_from_acked_snapshot_clock() {
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        assert!(
            delay > crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS / 2,
            "fixture needs the authored Chaos door cadence"
        );
        let door_state = crate::doors::ClientDoorState::for_challenge(1);
        let phases = crate::doors::client_door_phase_sequence(
            Some(door_state),
            1,
            1_u32.saturating_add(delay.saturating_mul(512)),
        )
        .expect("challenge-derived door stream");
        let (lower, ack_tick, frozen, receipt_tick) = phases
            .windows(4)
            .find_map(|window| {
                let repeated = window[0].seed == window[1].seed && window[1].seed == window[2].seed;
                let ack_tick = crate::continuum::continuum_tick_add(
                    window[2].started_tick,
                    delay.saturating_sub(18),
                );
                let horizon = crate::continuum::continuum_tick_add(
                    ack_tick,
                    crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS,
                );
                let frozen =
                    crate::doors::client_door_phase_candidates_at_tick(Some(door_state), horizon)
                        .into_iter()
                        .filter(|phase| {
                            crate::continuum::continuum_tick_diff(horizon, phase.started_tick) >= 0
                        })
                        .max_by_key(|phase| {
                            crate::continuum::continuum_tick_diff(phase.started_tick, horizon)
                        })?;
                let receipt_tick =
                    crate::continuum::continuum_tick_add(ack_tick, delay.saturating_mul(3));
                let receipt = crate::doors::client_door_phase_candidates_at_tick(
                    Some(door_state),
                    receipt_tick,
                );
                (repeated
                    && frozen.seed != window[0].seed
                    && receipt.iter().all(|phase| phase.seed != frozen.seed))
                .then_some((window[0], ack_tick, frozen, receipt_tick))
            })
            .expect("fixture needs a repeated seed followed by a frozen phase gap");
        let (state, identity, player_id) = joined_runtime_at_tick(receipt_tick, 0);
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client.life_position_sequence_base = None;
            client.last_position_sequence = Some(9);
            client.last_input_seq = Some(9);
            client.last_input_record = Some(crate::state::ClientInputObservation {
                bits: 0,
                direction: 0,
            });
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: 9,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: lower.seed,
                    door_phase_tick: lower.started_tick,
                });
            client.recent_input_rocket_assertions.push_back((9, false));
            client.last_acked_snapshot_id = Some(7);
            client
                .snapshot_history
                .push_back(crate::snapshot::SnapshotHistoryEntry {
                    snapshot_id: 7,
                    server_tick: ack_tick,
                    entities: Default::default(),
                    emitted_at: std::time::Instant::now(),
                    prize_known: None,
                });
        }

        assert!(
            record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_door_input_packet(10, &[(0, 0, frozen.seed)]),
                receipt_tick,
            ),
            "the acknowledged snapshot proves the exact phase the browser can reach before its one-second clock freezes"
        );
        let locked = state.lock().expect("state");
        let admitted = locked.clients[&player_id]
            .recent_input_records
            .iter()
            .find(|record| record.sequence == 10)
            .expect("frozen browser input admitted");
        assert_eq!(admitted.door_seed, frozen.seed);
        assert_eq!(admitted.door_phase_tick, frozen.started_tick);
    }

    #[test]
    fn queued_input_cannot_recover_a_phase_beyond_the_browser_clock_horizon() {
        let tick: u32 = 20_000;
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let recovery_ticks =
            crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS.saturating_add(delay);
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        let door_state = crate::doors::ClientDoorState::for_challenge(
            crate::continuum::continuum_tick_sub(tick, recovery_ticks.saturating_add(2_000)),
        );
        let receipt_phases =
            crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
        let receipt_current = receipt_phases[1];
        let lower_tick = crate::continuum::continuum_tick_sub(
            receipt_current.started_tick,
            recovery_ticks.saturating_add(delay.saturating_mul(12)),
        );
        let lower =
            crate::doors::client_door_phase_candidates_at_tick(Some(door_state), lower_tick)[1];
        let bounded_through =
            crate::continuum::continuum_tick_add(lower.started_tick, recovery_ticks);
        let allowed = crate::doors::client_door_phase_sequence(
            Some(door_state),
            lower.started_tick,
            crate::doors::client_door_phase_candidates_at_tick(Some(door_state), bounded_through)
                [1]
            .started_tick,
        )
        .expect("bounded challenge-derived recovery phases");
        let all = crate::doors::client_door_phase_sequence(
            Some(door_state),
            lower.started_tick,
            receipt_current.started_tick,
        )
        .expect("challenge-derived gap through receipt");
        let rejected = all
            .iter()
            .copied()
            .find(|phase| {
                crate::continuum::continuum_tick_diff(phase.started_tick, bounded_through) > 0
                    && receipt_phases
                        .iter()
                        .all(|candidate| candidate.seed != phase.seed)
                    && allowed.iter().all(|candidate| candidate.seed != phase.seed)
            })
            .expect("fixture needs a unique phase beyond the recovery horizon");
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: 9,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: lower.seed,
                    door_phase_tick: lower.started_tick,
                });
            client.last_input_seq = Some(9);
            client.last_input_record = Some(crate::state::ClientInputObservation {
                bits: 0,
                direction: 0,
            });
            client.last_acked_snapshot_id = Some(8);
            client
                .snapshot_history
                .push_back(crate::snapshot::SnapshotHistoryEntry {
                    snapshot_id: 8,
                    server_tick: lower.started_tick,
                    entities: Default::default(),
                    emitted_at: std::time::Instant::now(),
                    prize_known: None,
                });
        }

        assert!(
            !record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_door_input_packet(10, &[(0, 0, rejected.seed)]),
                tick,
            ),
            "queue recovery must remain bounded by the browser fail-closed clock horizon"
        );
        let locked = state.lock().expect("state");
        let records = &locked.clients[&player_id].recent_input_records;
        assert_eq!(
            records.len(),
            1,
            "rejection must preserve the lower witness"
        );
        assert_eq!(records[0].door_phase_tick, lower.started_tick);
    }

    #[test]
    fn correction_witness_remains_door_phase_floor_after_input_history_clear() {
        let tick = 10_000;
        let (state, _, player_id) = joined_runtime_at_tick(tick, 0);
        let door_state =
            crate::doors::ClientDoorState::for_challenge(crate::continuum::continuum_tick_sub(
                tick,
                crate::arena_settings::arena_door_delay_ticks() as u32 * 2,
            ));
        let phases = crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
        let previous = phases[0];
        let current = phases[1];
        assert_ne!(previous.seed, current.seed);
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.door_state = Some(door_state);
        // A committed correction replaces the replay witness and clears the
        // rolling inputs. Its authenticated phase must still constrain every
        // subsequent input; otherwise admission can undo that boundary before
        // canonical movement replay sees the record.
        client.recent_input_records.clear();
        client.last_input_record = None;
        client.last_input_seq = Some(100);
        client.last_position_sequence = Some(100);
        client.canonical_position_witnesses.clear();
        client
            .canonical_position_witnesses
            .push_back(crate::state::CanonicalPositionWitness {
                sequence: 100,
                life_seq: client.life_seq,
                state: crate::state::ServerSimMovement {
                    position_x: 1_000.0,
                    position_y: 1_000.0,
                    orientation: 0.0,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    energy: 1_000.0,
                    max_energy: 1_000.0,
                },
                wire_x: 1_000,
                wire_y: 1_000,
                wire_velocity_x: 0,
                wire_velocity_y: 0,
                direction: 0,
                door_seed: current.seed,
                door_phase_tick: current.started_tick,
            });
        let packet = encoded_door_input_packet(101, &[(1, 0, previous.seed)]);
        let observations = parse_client_input_observations(&packet).expect("input shape");
        assert_eq!(
            resolve_client_input_door_phases(client, &observations, 101, tick),
            Err(InputDoorPhaseResolutionError::Security(
                "door_phase_backtrack"
            )),
            "clearing rolling inputs must not erase the committed canonical phase floor",
        );
        for boundary_sequence in [100_u16, u16::MAX] {
            client
                .canonical_position_witnesses
                .back_mut()
                .unwrap()
                .sequence = boundary_sequence;
            let next_sequence = boundary_sequence.wrapping_add(1);
            let packet = encoded_door_input_packet(next_sequence, &[(1, 0, current.seed)]);
            let observations = parse_client_input_observations(&packet).expect("input shape");
            let resolved =
                resolve_client_input_door_phases(client, &observations, next_sequence, tick)
                    .expect("the committed seed must resume normally across sequence wrap");
            assert_eq!(resolved[0].door_phase_tick, current.started_tick);
            let packet = encoded_door_input_packet(next_sequence, &[(1, 0, previous.seed)]);
            let observations = parse_client_input_observations(&packet).expect("input shape");
            assert_eq!(
                resolve_client_input_door_phases(client, &observations, next_sequence, tick),
                Err(InputDoorPhaseResolutionError::Security(
                    "door_phase_backtrack"
                )),
            );
            client
                .canonical_position_witnesses
                .back_mut()
                .unwrap()
                .life_seq = client.life_seq.wrapping_add(1);
            let resolved =
                resolve_client_input_door_phases(client, &observations, next_sequence, tick)
                    .expect("another life's witness cannot constrain this life's phase");
            assert_eq!(resolved[0].door_phase_tick, previous.started_tick);
            client
                .canonical_position_witnesses
                .back_mut()
                .unwrap()
                .life_seq = client.life_seq;
        }
    }

    #[test]
    fn door_seed_byte_requires_a_canonical_monotonic_phase() {
        let tick: u32 = 10_000;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        let door_state =
            crate::doors::ClientDoorState::for_challenge(crate::continuum::continuum_tick_sub(
                tick,
                (crate::arena_settings::arena_door_delay_ticks() as u32) * 2,
            ));
        let phases = crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
        assert_eq!(
            phases.len(),
            3,
            "fixture needs previous/current/next phases"
        );
        let previous = phases[0];
        let current = phases[1];
        let next = phases[2];
        let forged_seed = (0..=u8::MAX)
            .find(|seed| phases.iter().all(|phase| phase.seed != *seed))
            .expect("three phases cannot cover every seed");
        state
            .lock()
            .expect("state")
            .clients
            .get_mut(&player_id)
            .expect("client")
            .door_state = Some(door_state);

        {
            let locked = state.lock().expect("state");
            let forged = encoded_door_input_packet(10, &[(0, 0, forged_seed)]);
            let parsed = parse_client_input_observations(&forged).expect("forged input shape");
            assert_eq!(
                resolve_client_input_door_phases(&locked.clients[&player_id], &parsed, 10, tick,),
                Err(InputDoorPhaseResolutionError::Security(
                    "door_seed_out_of_window",
                )),
                "a current record with no authenticated upper witness remains a security reject",
            );
        }

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(10, &[(0, 0, current.seed)]),
            tick,
        ));
        assert!(!record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(11, &[(0, 0, previous.seed)]),
            tick,
        ));
        assert!(!record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(11, &[(0, 0, forged_seed)]),
            tick,
        ));
        assert!(
            !record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_door_input_packet(11, &[(0, 0, next.seed)]),
                tick,
            ),
            "adjacent validation grace must not admit a phase before its plausible slot"
        );
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(11, &[(0, 0, next.seed)]),
            crate::continuum::continuum_tick_sub(next.started_tick, 1),
        ));
        assert!(!record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(12, &[(0, 0, current.seed)]),
            next.started_tick,
        ));
        assert!(!record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(11, &[(0, 0, current.seed)]),
            next.started_tick,
        ));

        let state = state.lock().expect("state");
        let records = &state
            .clients
            .get(&player_id)
            .expect("client")
            .recent_input_records;
        assert_eq!(records.len(), 2, "rejects must not mutate history");
        assert_eq!(records[0].door_phase_tick, current.started_tick);
        assert_eq!(records[1].door_phase_tick, next.started_tick);
    }

    #[test]
    fn post_death_rolling_input_is_inert_until_the_respawn_baseline() {
        let tick: u32 = 12_000;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 1);
        let door_state =
            crate::doors::ClientDoorState::for_challenge(crate::continuum::continuum_tick_sub(
                tick,
                crate::arena_settings::arena_door_delay_ticks().max(1) as u32,
            ));
        let seed =
            crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick)[1].seed;
        {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
        }
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(500, &[(0, 0, seed)]),
            tick,
        ));

        // This is the exact server-first death race: authority clears the old
        // life while an already-generated rolling packet is still in flight.
        // Use a seed that is valid at receipt so the assertion specifically
        // proves the dead-life gate, rather than an incidental door reject.
        {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.clear_server_runtime();
            client.alive = false;
            client.server_weapons.rockets = 1;
        }
        assert!(!record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(500, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0, seed)]),
            tick,
        ));

        {
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("client");
            assert_eq!(client.last_input_seq, None);
            assert!(client.recent_input_records.is_empty());
            assert!(client.recent_input_rocket_assertions.is_empty());
            assert_eq!(client.server_weapons.rockets, 1);
            assert_eq!(client.server_rocket_until_tick, None);
        }

        // The separately validated application-reliable neutral boundary is
        // the sole exception. It must remain admissible before the deadline
        // so the protected server-tick queue can commit the due respawn.
        assert!(record_client_input_observation_with_policy_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(501, &[(0, 0, seed)]),
            tick,
            ClientInputObservationPolicy::CanonicalNeutralLifecycleBaseline,
        ));
        let state = state.lock().expect("state");
        let client = state.clients.get(&player_id).expect("client");
        assert_eq!(client.last_input_seq, Some(501));
        assert_eq!(client.recent_input_records.len(), 1);
        assert_eq!(client.server_weapons.rockets, 1);
    }

    #[test]
    fn predecessor_life_receipt_after_respawn_commit_is_inert() {
        let tick: u32 = 14_000;
        let (state, first_identity, player_id) = joined_runtime_at_tick(tick, 1);
        let door_state =
            crate::doors::ClientDoorState::for_challenge(crate::continuum::continuum_tick_sub(
                tick,
                crate::arena_settings::arena_door_delay_ticks().max(1) as u32,
            ));
        let seed =
            crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick)[1].seed;
        let declared_successor_life = first_identity.life.life_seq.wrapping_add(1).max(1);
        let successor_receipt = crate::combat_timeline::CombatViewReceiptV1 {
            observer_life_seq: declared_successor_life,
            input_sequence: 501,
            timeline_generation: 1,
            latest_game_applied_snapshot_id: 1,
            flags: 0,
        };
        {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.clear_server_runtime();
            client.alive = false;
            client.door_state = Some(door_state);
        }
        assert!(
            record_client_input_observation_with_policy_at_tick(
                &state,
                first_identity,
                &encoded_door_input_packet_with_receipts(501, &[(0, 0, seed, successor_receipt)],),
                tick,
                ClientInputObservationPolicy::CanonicalNeutralLifecycleBaseline,
            ),
            "the reliable neutral baseline may name the successor it creates",
        );
        {
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("client");
            assert_eq!(client.last_input_seq, Some(501));
            assert_eq!(
                client.observer_combat_timeline.decision_for_sequence(501),
                None
            );
        }

        let successor_life = {
            let mut state = state.lock().expect("state");
            let successor_life = state
                .advance_client_life_at_tick(player_id, tick)
                .expect("successor life");
            assert_eq!(successor_life.life_seq, declared_successor_life);
            let client = state.clients.get_mut(&player_id).expect("client");
            // Model the exact instant after the reliable neutral baseline
            // commits but before its application receipt reaches the browser.
            client.alive = true;
            client.door_state = Some(door_state);
            client.life_position_sequence_base = Some(501);
            client.last_position_sequence = Some(501);
            successor_life
        };
        let successor_identity = PlayerRuntimeIdentity::new(first_identity.session, successor_life);
        let predecessor_receipt = crate::combat_timeline::CombatViewReceiptV1 {
            observer_life_seq: first_identity.life.life_seq,
            input_sequence: 502,
            timeline_generation: 1,
            latest_game_applied_snapshot_id: 1,
            flags: 0,
        };

        assert!(
            !record_client_input_observation_at_tick(
                &state,
                successor_identity,
                &encoded_door_input_packet_with_receipts(502, &[(1, 0, seed, predecessor_receipt)],),
                tick,
            ),
            "an in-flight predecessor receipt must not become successor input",
        );
        {
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("client");
            assert_eq!(client.last_input_seq, None);
            assert!(client.recent_input_records.is_empty());
        }

        assert!(
            record_client_input_observation_at_tick(
                &state,
                successor_identity,
                &encoded_door_input_packet(502, &[(1, 0, seed)]),
                tick,
            ),
            "discarding the predecessor must not poison the same successor sequence",
        );
        let state = state.lock().expect("state");
        let client = state.clients.get(&player_id).expect("client");
        assert_eq!(client.last_input_seq, Some(502));
        assert_eq!(client.recent_input_records.len(), 1);
        assert_eq!(client.last_input_record.map(|input| input.bits), Some(1));
    }

    #[test]
    fn neutral_lifecycle_baseline_uses_current_server_door_phase_after_reader_stall() {
        let send_tick: u32 = 18_000;
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let (state, identity, player_id) = joined_runtime_at_tick(send_tick, 0);
        let door_state = crate::doors::ClientDoorState::for_challenge(
            crate::continuum::continuum_tick_sub(send_tick, delay * 2),
        );
        let sent_phase =
            crate::doors::client_door_phase_candidates_at_tick(Some(door_state), send_tick)
                .into_iter()
                .filter(|phase| {
                    crate::continuum::continuum_tick_diff(send_tick, phase.started_tick) >= 0
                })
                .min_by_key(|phase| {
                    crate::continuum::continuum_tick_diff(send_tick, phase.started_tick)
                        .unsigned_abs()
                })
                .expect("send tick has a current authenticated phase");
        let (receipt_tick, receipt_phase) = (4_u32..32)
            .find_map(|phase_count| {
                let receipt_tick =
                    crate::continuum::continuum_tick_add(send_tick, delay * phase_count);
                let candidates = crate::doors::client_door_phase_candidates_at_tick(
                    Some(door_state),
                    receipt_tick,
                );
                let current = candidates
                    .iter()
                    .copied()
                    .filter(|phase| {
                        crate::continuum::continuum_tick_diff(receipt_tick, phase.started_tick) >= 0
                    })
                    .min_by_key(|phase| {
                        crate::continuum::continuum_tick_diff(receipt_tick, phase.started_tick)
                            .unsigned_abs()
                    })?;
                candidates
                    .iter()
                    .all(|phase| phase.seed != sent_phase.seed)
                    .then_some((receipt_tick, current))
            })
            .expect("challenge stream exposes a later nonmatching validation window");
        {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
        }

        // The old seed remains forbidden for ordinary movement after the
        // authenticated receipt window advances beyond it.
        assert!(!record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(700, &[(0, 0, sent_phase.seed)]),
            receipt_tick,
        ));

        // A server-first death clears the predecessor movement history. The
        // separately validated reliable neutral respawn boundary cannot move
        // through a door, so bind it to current server geometry instead of
        // rejecting an honest pre-stall byte or retaining that old phase.
        {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.clear_server_runtime();
            client.alive = false;
        }
        assert!(record_client_lifecycle_input_baseline_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(701, &[(0, 0, sent_phase.seed)]),
            receipt_tick,
        ));
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        assert_eq!(client.last_input_seq, Some(701));
        assert_eq!(client.recent_input_records.len(), 1);
        assert_eq!(client.recent_input_records[0].door_seed, receipt_phase.seed);
        assert_eq!(
            client.recent_input_records[0].door_phase_tick,
            receipt_phase.started_tick
        );
        // The position admission stage commits this protected baseline and
        // marks the successor life alive before the receipt reaches the
        // browser. This focused input fixture models that adjacent boundary.
        client.alive = true;
        drop(locked);

        // Once the client consumes the application receipt and rebases to the
        // canonical commit tick, its first ordinary successor-life record is
        // strict movement input again—not another lifecycle exception.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(702, &[(1, 0, receipt_phase.seed)]),
            crate::continuum::continuum_tick_add(receipt_tick, 1),
        ));
        let locked = state.lock().expect("state");
        let client = locked.clients.get(&player_id).expect("client");
        assert_eq!(client.last_input_seq, Some(702));
        assert_eq!(client.last_input_record.map(|input| input.bits), Some(1));
    }

    #[test]
    fn sustained_phase_ahead_records_fail_atomically_then_honest_delayed_boundary_recovers() {
        let tick: u32 = 25_000;
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        let door_state = crate::doors::ClientDoorState::for_challenge(
            crate::continuum::continuum_tick_sub(tick, delay * 3),
        );
        let phases = crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
        let previous = phases[0];
        let current = phases[1];
        let next = phases[2];
        state
            .lock()
            .expect("state")
            .clients
            .get_mut(&player_id)
            .expect("client")
            .door_state = Some(door_state);

        // An honest delayed two-record bundle may straddle the prior/current
        // boundary even though both arrive in the current receipt phase.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(40, &[(0, 0, previous.seed), (0, 0, current.seed)],),
            tick,
        ));
        let before = state
            .lock()
            .expect("state")
            .clients
            .get(&player_id)
            .expect("client")
            .recent_input_records
            .clone();

        for receipt_tick in [tick, crate::continuum::continuum_tick_add(tick, 2)] {
            assert!(!record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_door_input_packet(42, &[(0, 0, next.seed)]),
                receipt_tick,
            ));
            let client_records = state
                .lock()
                .expect("state")
                .clients
                .get(&player_id)
                .expect("client")
                .recent_input_records
                .clone();
            assert_eq!(
                client_records, before,
                "future rejects must not mutate history"
            );
        }

        // The same sequence remains correctable with the exact current phase;
        // the rejected future byte never poisons its monotonic lower bound.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(42, &[(0, 0, current.seed)]),
            tick,
        ));
    }

    #[test]
    fn recovered_record_may_share_the_server_proven_lower_door_phase() {
        let tick: u32 = 27_500;
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        let door_state = crate::doors::ClientDoorState::for_challenge(
            crate::continuum::continuum_tick_sub(tick, delay * 2),
        );
        let phases = crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
        let current = phases[1];
        state
            .lock()
            .expect("state")
            .clients
            .get_mut(&player_id)
            .expect("client")
            .door_state = Some(door_state);

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(10, &[(0, 0, current.seed)]),
            tick,
        ));
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(28, &[(0, 0, current.seed)]),
            tick,
        ));
        assert!(
            record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_door_input_packet(11, &[(0, 0, current.seed)]),
                tick,
            ),
            "a recovered record cannot predate the already-proven phase of its lower sequence",
        );

        let locked = state.lock().expect("state");
        let records = &locked.clients[&player_id].recent_input_records;
        assert_eq!(
            records
                .iter()
                .find(|record| record.sequence == 11)
                .expect("recovered sequence")
                .door_phase_tick,
            current.started_tick,
        );
    }

    #[test]
    fn repeated_door_seed_does_not_jump_ahead_of_a_following_current_phase() {
        let tick: u32 = 10_000;
        let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
        let (door_state, phases) = (1..=512)
            .find_map(|current_count| {
                let phase_start = tick.wrapping_sub(24);
                let timestamp = phase_start.wrapping_sub(current_count * delay);
                let door_state = crate::doors::ClientDoorState::for_challenge(timestamp);
                let phases =
                    crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick);
                (phases.len() == 3
                    && phases[0].seed == phases[2].seed
                    && phases[0].seed != phases[1].seed)
                    .then_some((door_state, phases))
            })
            .expect("canonical RNG should expose the repeated previous/next artifact pattern");
        let previous = phases[0];
        let current = phases[1];
        let next = phases[2];
        assert_eq!(
            crate::continuum::continuum_tick_diff(next.started_tick, tick),
            delay as i32 - 24,
            "fixture must make the repeated future seed closer than the previous seed"
        );

        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        state
            .lock()
            .expect("state")
            .clients
            .get_mut(&player_id)
            .expect("client")
            .door_state = Some(door_state);

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(10, &[(0, 0, previous.seed), (0, 0, current.seed)],),
            tick,
        ));

        let state = state.lock().expect("state");
        let records = &state
            .clients
            .get(&player_id)
            .expect("client")
            .recent_input_records;
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].door_phase_tick, previous.started_tick);
        assert_eq!(records[1].door_phase_tick, current.started_tick);
    }

    #[test]
    fn queued_adjacent_input_keeps_earliest_repeated_phase_then_advances_after_hitch() {
        // Retained headed evidence produced this exact challenge stream and
        // timing: 255@7184, 226@7304, 236@7424, 255@7544, 204@7664. The arena
        // thread then stalled until tick 7624 while the RTC reader had already
        // received adjacent 255 records followed by 226. The delayed 255 must
        // remain in its proven earlier phase; binding it to 7544 would reject
        // the following honest 226 as a backtrack.
        let receipt_tick = 7_624_u32;
        let door_state = crate::doors::ClientDoorState::for_challenge(824);
        let phases = crate::doors::client_door_phase_sequence(Some(door_state), 7_184, 7_664)
            .expect("exact challenge-derived hitch phases");
        assert_eq!(
            phases
                .iter()
                .map(|phase| (phase.started_tick, phase.seed))
                .collect::<Vec<_>>(),
            vec![
                (7_184, 255),
                (7_304, 226),
                (7_424, 236),
                (7_544, 255),
                (7_664, 204),
            ]
        );
        let earlier_repeated = phases[0];
        let following = phases[1];
        let later_repeated = phases[3];

        let (state, identity, player_id) = joined_runtime_at_tick(receipt_tick, 0);
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.door_state = Some(door_state);
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: 29_311,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 37,
                    },
                    door_seed: earlier_repeated.seed,
                    door_phase_tick: earlier_repeated.started_tick,
                });
            client.last_input_seq = Some(29_311);
            client.last_input_record = Some(crate::state::ClientInputObservation {
                bits: 0,
                direction: 37,
            });
        }

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(
                29_312,
                &[
                    (0, 37, earlier_repeated.seed),
                    (0, 37, earlier_repeated.seed),
                ],
            ),
            receipt_tick,
        ));
        {
            let locked = state.lock().expect("state");
            let records = &locked.clients[&player_id].recent_input_records;
            for sequence in [29_312, 29_313] {
                let record = records
                    .iter()
                    .find(|record| record.sequence == sequence)
                    .expect("queued adjacent record");
                assert_eq!(record.door_phase_tick, earlier_repeated.started_tick);
                assert_ne!(record.door_phase_tick, later_repeated.started_tick);
            }
        }

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_door_input_packet(29_314, &[(0, 37, following.seed)]),
            receipt_tick,
        ));
        let locked = state.lock().expect("state");
        let following_record = locked.clients[&player_id]
            .recent_input_records
            .iter()
            .find(|record| record.sequence == 29_314)
            .expect("following honest phase");
        assert_eq!(following_record.door_phase_tick, following.started_tick);
    }

    #[test]
    fn rocket_illegal_pulses_are_quarantined_without_losing_movement_history() {
        let tick = 10_000;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        let packet = encoded_input_packet(40, &[(0, 0), (CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0)]);

        assert!(record_client_input_observation_at_tick(
            &state, identity, &packet, tick
        ));
        {
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("client");
            assert_eq!(client.last_input_seq, Some(41));
            assert_eq!(client.recent_input_records.len(), 2);
            assert_eq!(client.recent_input_rocket_assertions.len(), 2);
            assert_eq!(
                client
                    .recent_input_records
                    .iter()
                    .find(|record| record.sequence == 41)
                    .expect("canonical record")
                    .input
                    .bits
                    & CLIENT_INPUT_ROCKET_ACTIVE_BIT,
                0,
                "a never-owned pulse is retained only as canonical false"
            );
            assert_eq!(client.server_rocket_until_tick, None);
            assert_eq!(client.server_weapons.rockets, 0);
        }

        // A malicious held bit cannot create a history wedge: more than the
        // 64-record retention window remains admissible and canonical false.
        for sequence in 42..=110 {
            assert!(record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_input_packet(sequence, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 1)]),
                tick
            ));
        }
        {
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("client");
            assert_eq!(
                client.recent_input_records.len(),
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS
            );
            assert!(
                client
                    .recent_input_records
                    .iter()
                    .all(|record| { record.input.bits & CLIENT_INPUT_ROCKET_ACTIVE_BIT == 0 })
            );
        }

        {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.server_weapons.rockets = 1;
            client.last_bomb_fire_tick = Some(tick);
        }
        // Release and re-press after a bomb was already accepted at this
        // server tick. Rocket and bomb are independent same-step actions.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(111, &[(0, 0), (CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0)]),
            tick
        ));
        let state = state.lock().expect("state");
        let client = state.clients.get(&player_id).expect("client");
        assert_eq!(client.server_weapons.rockets, 0);
        assert!(client.server_rocket_until_tick.is_some());
        assert_eq!(client.last_input_seq, Some(112));
    }

    #[test]
    fn delayed_shutdown_ack_tracks_many_rotating_thrusting_tails_without_rewind_across_wrap() {
        let tick = 15_000;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 0);
        let first = u16::MAX - 9;
        let input_for = |sequence: u16| {
            if sequence & 1 == 0 {
                (0b0000_0101, 39)
            } else {
                (0b0000_1001, 1)
            }
        };
        let (life_seq, revision) = {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.last_input_seq = Some(first.wrapping_sub(1));
            client.apply_server_prize_effect_at_tick(-14, tick);
            client.pending_prize_effect_applications.push_back(
                crate::state::PendingPrizeEffectApplication {
                    collectible_id: 0x1020_3040,
                    prize_id: -14,
                    life_seq: client.life_seq,
                    prize_state_revision: client.server_prize_state_revision,
                    minimum_application_sequence: Some(first),
                    maximum_application_sequence: Some(first),
                    latest_application_sequence: None,
                    deferred_application: None,
                    movement_tuning_before_bits: None,
                },
            );
            (client.life_seq, client.server_prize_state_revision)
        };

        for (base, count) in [(first, 32_usize), (22, 32), (54, 16)] {
            let inputs = (0..count)
                .map(|offset| input_for(base.wrapping_add(offset as u16)))
                .collect::<Vec<_>>();
            assert!(record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_input_packet(base, &inputs),
                tick,
            ));
        }
        // An exact duplicate/reordered suffix is admissible but cannot rewind
        // the boundary advanced by the 80 authenticated old-semantics tails.
        let duplicate_base = u16::MAX - 4;
        let duplicate = (0..8)
            .map(|offset| input_for(duplicate_base.wrapping_add(offset)))
            .collect::<Vec<_>>();
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(duplicate_base, &duplicate),
            tick,
        ));
        {
            let state = state.lock().expect("state");
            let client = &state.clients[&player_id];
            assert_eq!(client.last_input_seq, Some(69));
            assert_eq!(
                client.pending_prize_effect_applications[0].maximum_application_sequence,
                Some(70)
            );
            assert_eq!(
                client.recent_input_records.len(),
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS,
                "old-semantics history remains explicitly bounded while its successor keeps advancing"
            );
            assert!(client.recent_input_records.iter().all(|record| {
                let expected = input_for(record.sequence);
                record.input.bits == expected.0 && record.input.direction == expected.1
            }));
        }

        let application = crate::prize_application::PrizeEffectApplied {
            server_session_id: crate::wire::server_session_id(),
            player_id,
            life_seq,
            collectible_id: 0x1020_3040,
            prize_id: -14,
            prize_state_revision: revision,
            application_sequence: 70,
        };
        let accepted =
            crate::player_events::apply_prize_effect_application(&state, player_id, application)
                .expect("exact successor applies");
        assert_eq!(
            accepted.outcome,
            crate::prize_application::PrizeEffectAccepted::OUTCOME_APPLIED
        );
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        assert!(!client.server_engine_shutdown_active_for_sequence(tick, 69));
        assert!(client.server_engine_shutdown_active_for_sequence(tick, 70));
    }

    #[test]
    fn shutdown_marker_overtakes_wrapped_final_tail_and_applies_only_after_exact_ingress() {
        let tick = 16_000;
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena_at_tick(&state, player_id, 0, tick);
        while rx.try_recv().is_ok() {}
        let (identity, application) = {
            let mut state = state.lock().expect("state");
            let identity = state
                .active_player_runtime_identity(player_id)
                .expect("joined runtime");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.last_input_seq = Some(u16::MAX - 1);
            client.apply_server_prize_effect_at_tick(-14, tick);
            let application = crate::prize_application::PrizeEffectApplied {
                server_session_id: crate::wire::server_session_id(),
                player_id,
                life_seq: client.life_seq,
                collectible_id: 0x5566_7788,
                prize_id: -14,
                prize_state_revision: client.server_prize_state_revision,
                application_sequence: 2,
            };
            client.pending_prize_effect_applications.push_back(
                crate::state::PendingPrizeEffectApplication {
                    collectible_id: application.collectible_id,
                    prize_id: application.prize_id,
                    life_seq: application.life_seq,
                    prize_state_revision: application.prize_state_revision,
                    minimum_application_sequence: Some(u16::MAX),
                    maximum_application_sequence: Some(u16::MAX),
                    latest_application_sequence: None,
                    deferred_application: None,
                    movement_tuning_before_bits: None,
                },
            );
            (identity, application)
        };

        assert_eq!(
            crate::player_events::apply_prize_effect_application(&state, player_id, application,),
            None,
            "the reliable marker must wait for its omitted unordered tail",
        );
        assert_eq!(
            crate::player_events::apply_prize_effect_application(&state, player_id, application,),
            None,
            "a duplicate marker remains the same deferred declaration",
        );
        {
            let state = state.lock().expect("state");
            let client = &state.clients[&player_id];
            assert_eq!(
                client.pending_prize_effect_applications[0].deferred_application,
                Some(application),
            );
            assert!(!client.server_engine_shutdown_active_for_sequence(tick, 1));
            assert!(!client.server_engine_shutdown_active_for_sequence(tick, 2));
        }

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(
                u16::MAX,
                &[(0b0000_0101, 39), (0b0000_1001, 0), (0b0000_0101, 1)],
            ),
            tick,
        ));
        let terminal = rx
            .try_iter()
            .find_map(|packet| {
                crate::reliable::reliable_payload(&packet).and_then(|payload| {
                    crate::prize_application::PrizeEffectAccepted::parse(payload).ok()
                })
            })
            .expect("input ingress emits the durable application terminal");
        assert_eq!(terminal.application_sequence, 2);
        assert_eq!(
            terminal.outcome,
            crate::prize_application::PrizeEffectAccepted::OUTCOME_APPLIED,
        );
        {
            let state = state.lock().expect("state");
            let client = &state.clients[&player_id];
            assert!(client.pending_prize_effect_applications.is_empty());
            for old_sequence in [u16::MAX, 0, 1] {
                assert!(!client.server_engine_shutdown_active_for_sequence(tick, old_sequence));
            }
            assert!(client.server_engine_shutdown_active_for_sequence(tick, 2));
        }
        assert_eq!(
            crate::player_events::apply_prize_effect_application(&state, player_id, application,),
            Some(terminal),
            "loss of the asynchronously emitted terminal retries idempotently",
        );
    }

    #[test]
    fn shutdown_marker_beyond_retained_tail_window_is_durably_cancelled() {
        let tick = 16_500;
        let (state, _identity, player_id) = joined_runtime_at_tick(tick, 0);
        let application = {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.last_input_seq = Some(9);
            client.apply_server_prize_effect_at_tick(-14, tick);
            let application = crate::prize_application::PrizeEffectApplied {
                server_session_id: crate::wire::server_session_id(),
                player_id,
                life_seq: client.life_seq,
                collectible_id: 0x6677_8899,
                prize_id: -14,
                prize_state_revision: client.server_prize_state_revision,
                application_sequence: 10_u16
                    .wrapping_add(crate::state::MAX_RECENT_INPUT_OBSERVATIONS as u16 + 1),
            };
            client.pending_prize_effect_applications.push_back(
                crate::state::PendingPrizeEffectApplication {
                    collectible_id: application.collectible_id,
                    prize_id: application.prize_id,
                    life_seq: application.life_seq,
                    prize_state_revision: application.prize_state_revision,
                    minimum_application_sequence: Some(10),
                    maximum_application_sequence: Some(10),
                    latest_application_sequence: None,
                    deferred_application: None,
                    movement_tuning_before_bits: None,
                },
            );
            application
        };
        let cancelled =
            crate::player_events::apply_prize_effect_application(&state, player_id, application)
                .expect("out-of-window declaration gets a terminal");
        assert_eq!(
            cancelled.outcome,
            crate::prize_application::PrizeEffectAccepted::OUTCOME_CANCELLED,
        );
        assert!(
            state.lock().expect("state").clients[&player_id]
                .pending_prize_effect_applications
                .is_empty()
        );
        assert_eq!(
            crate::player_events::apply_prize_effect_application(&state, player_id, application,),
            Some(cancelled),
        );
    }

    #[test]
    fn rocket_rising_edge_is_single_use_and_server_timer_is_canonical() {
        let tick = 20_000;
        let lifetime = arena_ship_rocket_lifetime_ticks(0);
        assert!(lifetime > 2);
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 2);
        let rising = encoded_input_packet(100, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 7)]);

        assert!(record_client_input_observation_at_tick(
            &state, identity, &rising, tick
        ));
        let deadline = crate::continuum::continuum_tick_add(
            tick,
            crate::gameplay_rules::rocket_remaining_ticks_after_sequence(100, lifetime, 100)
                .expect("first logical record"),
        );
        {
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("client");
            assert_eq!(client.server_weapons.rockets, 1);
            assert_eq!(client.server_rocket_until_tick, Some(deadline));
        }
        assert_ne!(
            canonical_bits(&state, player_id, 100) & CLIENT_INPUT_ROCKET_ACTIVE_BIT,
            0
        );

        // Retransmission and held pulses cannot consume or extend.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &rising,
            tick.wrapping_add(1)
        ));
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(101, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 8)]),
            tick.wrapping_add(1)
        ));
        // A false/true overlap while active is another edge, but it still
        // cannot consume inventory or move the authoritative deadline.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(102, &[(0, 9), (CLIENT_INPUT_ROCKET_ACTIVE_BIT, 10)]),
            tick.wrapping_add(2)
        ));
        let state = state.lock().expect("state");
        let client = state.clients.get(&player_id).expect("client");
        assert_eq!(client.server_weapons.rockets, 1);
        assert_eq!(client.server_rocket_until_tick, Some(deadline));
        assert_ne!(
            client
                .recent_input_records
                .iter()
                .find(|record| record.sequence == 102)
                .expect("server-owned continuation")
                .input
                .bits
                & CLIENT_INPUT_ROCKET_ACTIVE_BIT,
            0,
            "raw false cannot voluntarily end a live server timer"
        );
    }

    #[test]
    fn manual_warp_bomb_cooldown_quarantines_rocket_without_spending_inventory() {
        let tick = 20_500;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 1);
        state
            .lock()
            .expect("state")
            .clients
            .get_mut(&player_id)
            .expect("client")
            .server_warp_bomb_lock_until_tick =
            Some(crate::continuum::continuum_tick_add(tick, 50));

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(100, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0)]),
            tick,
        ));
        let state = state.lock().expect("state");
        let client = state.clients.get(&player_id).expect("client");
        assert_eq!(client.server_weapons.rockets, 1);
        assert_eq!(client.server_rocket_until_tick, None);
        assert_eq!(
            client
                .recent_input_records
                .iter()
                .find(|record| record.sequence == 100)
                .expect("canonical record")
                .input
                .bits
                & CLIENT_INPUT_ROCKET_ACTIVE_BIT,
            0,
        );
    }

    #[test]
    fn rocket_expiry_is_wrap_safe_and_requires_a_new_edge_and_inventory() {
        let lifetime = arena_ship_rocket_lifetime_ticks(0);
        assert!(lifetime > 0);
        let tick = crate::continuum::CONTINUUM_TICK_MASK - lifetime / 2;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 2);

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(500, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0)]),
            tick
        ));
        let authorization = state
            .lock()
            .expect("state")
            .clients
            .get(&player_id)
            .expect("client")
            .server_rocket_authorizations[0];
        let deadline = authorization.deadline_tick;
        assert!(deadline < tick, "fixture must cross the u31 wrap");
        let last_active = authorization
            .last_active_sequence()
            .expect("nonzero logical Rocket interval");
        let first_inactive = last_active.wrapping_add(1);

        // A valid logical record may arrive after the wall-clock deadline.
        // Its canonical bit is recovered from the server authorization, not
        // from receipt time or the wire pulse.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(501, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0)]),
            deadline
        ));
        assert_ne!(
            canonical_bits(&state, player_id, 501) & CLIENT_INPUT_ROCKET_ACTIVE_BIT,
            0
        );

        // A held pulse on and beyond the first inactive logical step is
        // quarantined false without discarding the valid movement bundle.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(
                last_active.wrapping_sub(1),
                &[
                    (CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0),
                    (CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0),
                    (CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0),
                ],
            ),
            deadline
        ));
        {
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("client");
            assert_eq!(client.last_input_seq, Some(first_inactive));
            assert_eq!(client.server_weapons.rockets, 1);
            assert_eq!(client.server_rocket_until_tick, Some(deadline));
            assert!(
                client
                    .recent_input_records
                    .iter()
                    .find(|record| record.sequence == first_inactive)
                    .expect("inactive canonical record")
                    .input
                    .bits
                    & CLIENT_INPUT_ROCKET_ACTIVE_BIT
                    == 0,
                "held Rocket pulse must not extend the logical interval"
            );
        }

        // A release followed by a fresh edge consumes exactly the final owned
        // Rocket and starts a new server interval.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(
                first_inactive.wrapping_add(1),
                &[(0, 0), (CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0)],
            ),
            deadline
        ));
        let second_deadline = state
            .lock()
            .expect("state")
            .clients
            .get(&player_id)
            .expect("client")
            .server_rocket_authorizations[1]
            .deadline_tick;
        {
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("client");
            assert_eq!(client.server_weapons.rockets, 0);
            assert_eq!(client.server_rocket_until_tick, Some(second_deadline));
        }

        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(
                first_inactive
                    .wrapping_add(1)
                    .wrapping_add(
                        authorization
                            .last_active_sequence()
                            .expect("last")
                            .wrapping_sub(authorization.start_sequence),
                    )
                    .wrapping_add(1),
                &[(0, 0)],
            ),
            second_deadline
        ));
        let after_second = state
            .lock()
            .expect("state")
            .clients
            .get(&player_id)
            .expect("client")
            .last_input_seq
            .expect("inactive sequence");
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(
                after_second.wrapping_add(1),
                &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0)],
            ),
            second_deadline
        ));
        let state = state.lock().expect("state");
        let client = state.clients.get(&player_id).expect("client");
        assert_eq!(client.last_input_seq, Some(after_second.wrapping_add(1)));
        assert_eq!(client.server_weapons.rockets, 0);
        assert_eq!(
            client
                .recent_input_records
                .iter()
                .find(|record| record.sequence == after_second.wrapping_add(1))
                .expect("canonical rejected Rocket record")
                .input
                .bits
                & CLIENT_INPUT_ROCKET_ACTIVE_BIT,
            0,
            "out-of-inventory edge cannot discard or authorize the record"
        );
    }

    #[test]
    fn client_wire_server_exact_expiry_matches_n_plus_one_activation_interval() {
        const CLIENT_STEP_SECONDS: f32 = 1.0 / 60.0;
        let lifetime = arena_ship_rocket_lifetime_ticks(0);
        let tick = crate::continuum::CONTINUUM_TICK_MASK - lifetime / 2;
        let start = u16::MAX - 100;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 2);
        let mut client_timer_seconds = lifetime as f32 / 100.0;
        let first_step = crate::gameplay_rules::begin_rocket_movement_step(
            &mut client_timer_seconds,
            CLIENT_STEP_SECONDS,
            Some(start),
            start,
        );
        assert!(first_step.active);
        assert!(first_step.activation_pulse);
        let first_pulse = encoded_input_packet(
            start,
            &[(
                u16::from(first_step.activation_pulse) * CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT,
                0,
            )],
        );
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &first_pulse,
            tick,
        ));
        let first_authorization =
            state.lock().expect("state").clients[&player_id].server_rocket_authorizations[0];
        let last_active = first_authorization
            .last_active_sequence()
            .expect("nonzero Rocket interval");
        let first_inactive = last_active.wrapping_add(1);

        assert!(first_authorization.contains_sequence(start));

        // Compare every real client decrement-before-movement sample with the
        // server's logical authorization through the exact expiry record.
        // The first activation occurred after movement start-1, so start is
        // both its pulse and first active movement sequence.
        let mut sequence = start.wrapping_add(1);
        let mut steps = 0;
        while sequence != first_inactive.wrapping_add(1) {
            let client_step = crate::gameplay_rules::begin_rocket_movement_step(
                &mut client_timer_seconds,
                CLIENT_STEP_SECONDS,
                Some(start),
                sequence,
            );
            assert_eq!(
                client_step.active,
                first_authorization.contains_sequence(sequence),
                "client/server Rocket interval diverged at {sequence}",
            );
            assert!(record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_input_packet(
                    sequence,
                    &[(
                        u16::from(client_step.activation_pulse)
                            * CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT,
                        0,
                    )],
                ),
                if client_step.active {
                    tick.wrapping_add(1)
                } else {
                    first_authorization.deadline_tick
                },
            ));
            sequence = sequence.wrapping_add(1);
            steps += 1;
            assert!(steps < 2_000, "fixture must reach exact expiry");
        }
        assert_eq!(
            canonical_bits(&state, player_id, first_inactive)
                & CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT,
            0,
            "expiry movement record is inactive and pulse-free"
        );

        let deadline = first_authorization.deadline_tick;
        assert!(deadline < tick, "fixture must cross the u31 tick boundary");
        // F3 is processed only after the inactive expiry movement step. Its
        // accepted pulse and first Rocket movement therefore belong to the
        // following sequence, never to `first_inactive` itself.
        let second_sequence =
            crate::gameplay_rules::rocket_activation_sequence_after_movement(first_inactive);
        client_timer_seconds = lifetime as f32 / 100.0;
        let second_step = crate::gameplay_rules::begin_rocket_movement_step(
            &mut client_timer_seconds,
            CLIENT_STEP_SECONDS,
            Some(second_sequence),
            second_sequence,
        );
        assert!(second_step.active);
        assert!(second_step.activation_pulse);
        let second_pulse = encoded_input_packet(
            second_sequence,
            &[(
                u16::from(second_step.activation_pulse) * CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT,
                0,
            )],
        );
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &second_pulse,
            deadline,
        ));
        {
            let state = state.lock().expect("state");
            let client = &state.clients[&player_id];
            assert_eq!(client.server_weapons.rockets, 0);
            assert_eq!(client.server_rocket_authorizations.len(), 2);
            assert_eq!(
                client.server_rocket_authorizations[1].start_sequence,
                second_sequence,
            );
        }

        // The exact rolling-tail retry and a following held true record are
        // both idempotent and cannot spend or extend the second activation.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &second_pulse,
            deadline.wrapping_add(1),
        ));
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(
                second_sequence.wrapping_add(1),
                &[(CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT, 0)],
            ),
            deadline.wrapping_add(1),
        ));
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        assert_eq!(client.server_weapons.rockets, 0);
        assert_eq!(client.server_rocket_authorizations.len(), 2);
    }

    #[test]
    fn delayed_full_wire_rocket_uses_sequence_remainder_and_exact_retrigger() {
        const CLIENT_STEP_SECONDS: f32 = 1.0 / 60.0;
        const DELAYED_STEPS: u16 = 25;
        let lifetime_ticks = arena_ship_rocket_lifetime_ticks(0);
        let receipt_tick = crate::continuum::CONTINUUM_TICK_MASK - 500;
        let activation_sequence = u16::MAX - 10;
        let newest_sequence = activation_sequence.wrapping_add(DELAYED_STEPS - 1);
        let (state, identity, player_id) = joined_runtime_at_tick(receipt_tick, 2);

        // All pulse-bearing copies were delayed. The first admitted wire
        // bundle safely carries the original pulse plus 24 newer logical
        // records, including u16 wrap.
        let mut client_timer = lifetime_ticks as f32 / 100.0;
        let mut inputs = Vec::with_capacity(usize::from(DELAYED_STEPS));
        let mut expected_active = Vec::with_capacity(usize::from(DELAYED_STEPS));
        for offset in 0..DELAYED_STEPS {
            let sequence = activation_sequence.wrapping_add(offset);
            let step = crate::gameplay_rules::begin_rocket_movement_step(
                &mut client_timer,
                CLIENT_STEP_SECONDS,
                Some(activation_sequence),
                sequence,
            );
            inputs.push((
                u16::from(step.activation_pulse) * CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT,
                0,
            ));
            expected_active.push((sequence, step.active));
        }
        let predicted_before_reconciliation = client_timer;
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(activation_sequence, &inputs),
            receipt_tick,
        ));

        let authorization =
            state.lock().expect("state").clients[&player_id].server_rocket_authorizations[0];
        let remaining_ticks = authorization
            .remaining_ticks_after(newest_sequence)
            .expect("bounded delayed tail");
        assert_eq!(remaining_ticks, 959);
        assert_eq!(
            authorization.deadline_tick,
            crate::continuum::continuum_tick_add(receipt_tick, remaining_ticks),
        );
        assert!(
            authorization.deadline_tick < receipt_tick,
            "u31 wrap fixture"
        );
        {
            let state = state.lock().expect("state");
            let client = &state.clients[&player_id];
            assert_eq!(client.last_input_seq, Some(newest_sequence));
            assert_eq!(
                client.server_rocket_until_tick,
                Some(authorization.deadline_tick)
            );
            assert_eq!(client.server_weapons.rockets, 1);
        }
        for (sequence, active) in expected_active {
            assert_eq!(
                canonical_bits(&state, player_id, sequence)
                    & CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT
                    != 0,
                active,
                "canonical delayed record disagreed at {sequence}",
            );
            assert_eq!(authorization.contains_sequence(sequence), active);
        }

        // Even a maliciously fresh-looking receipt deadline cannot increase
        // the timer the client has already decremented for those 25 records.
        let reconciled = crate::gameplay_rules::reconciled_rocket_timer_seconds(
            predicted_before_reconciliation,
            lifetime_ticks as f32 / 100.0,
            CLIENT_STEP_SECONDS,
            Some(activation_sequence),
            newest_sequence,
            remaining_ticks as f32 / 100.0,
            true,
        );
        assert!(reconciled <= predicted_before_reconciliation);

        let first_inactive = authorization
            .last_active_sequence()
            .expect("active interval")
            .wrapping_add(1);
        let mut sequence = newest_sequence.wrapping_add(1);
        while sequence != first_inactive.wrapping_add(1) {
            let step = crate::gameplay_rules::begin_rocket_movement_step(
                &mut client_timer,
                CLIENT_STEP_SECONDS,
                Some(activation_sequence),
                sequence,
            );
            assert_eq!(step.active, authorization.contains_sequence(sequence));
            assert!(record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_input_packet(sequence, &[(0, 0)]),
                receipt_tick.wrapping_add(1),
            ));
            assert_eq!(
                canonical_bits(&state, player_id, sequence)
                    & CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT
                    != 0,
                step.active,
            );
            sequence = sequence.wrapping_add(1);
        }
        assert!(
            state.lock().expect("state").clients[&player_id]
                .server_rocket_until_tick
                .is_some(),
            "fixture keeps the adjusted wall deadline live past logical expiry",
        );

        // F3 runs after the first inactive movement record and binds N+1. The
        // logical boundary admits it even though the prior wall deadline has
        // not elapsed. Duplicate and held-true copies remain idempotent.
        let retrigger_sequence = first_inactive.wrapping_add(1);
        let retrigger = encoded_input_packet(
            retrigger_sequence,
            &[(CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT, 0)],
        );
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &retrigger,
            receipt_tick.wrapping_add(1),
        ));
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &retrigger,
            receipt_tick.wrapping_add(2),
        ));
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(
                retrigger_sequence.wrapping_add(1),
                &[(CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT, 0)],
            ),
            receipt_tick.wrapping_add(2),
        ));
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        assert_eq!(client.server_weapons.rockets, 0);
        assert_eq!(client.server_rocket_authorizations.len(), 2);
        assert_eq!(
            client.server_rocket_authorizations[1].start_sequence,
            retrigger_sequence,
        );
    }

    #[test]
    fn held_true_records_cannot_auto_spend_at_exact_expiry() {
        let tick = 45_000;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 2);
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(10_000, &[(CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT, 0)],),
            tick,
        ));
        let authorization =
            state.lock().expect("state").clients[&player_id].server_rocket_authorizations[0];
        let first_inactive = authorization
            .last_active_sequence()
            .expect("last active")
            .wrapping_add(1);
        let mut sequence = 10_001_u16;
        while sequence != first_inactive.wrapping_add(1) {
            assert!(record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_input_packet(sequence, &[(CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT, 0)],),
                authorization.deadline_tick,
            ));
            sequence = sequence.wrapping_add(1);
        }
        {
            let state = state.lock().expect("state");
            let client = &state.clients[&player_id];
            assert_eq!(client.server_weapons.rockets, 1);
            assert_eq!(client.server_rocket_authorizations.len(), 1);
            assert_eq!(
                client
                    .recent_input_records
                    .iter()
                    .find(|record| record.sequence == first_inactive)
                    .expect("first inactive canonical record")
                    .input
                    .bits
                    & CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT,
                0,
            );
        }

        // Only a release followed by a genuinely new pulse can spend again.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(first_inactive.wrapping_add(1), &[(0, 0)]),
            authorization.deadline_tick,
        ));
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(
                first_inactive.wrapping_add(2),
                &[(CLIENT_INPUT_ROCKET_ACTIVATION_PULSE_BIT, 0)],
            ),
            authorization.deadline_tick,
        ));
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        assert_eq!(client.server_weapons.rockets, 0);
        assert_eq!(client.server_rocket_authorizations.len(), 2);
    }

    #[test]
    fn rocket_loss_recovery_ring_and_stale_life_do_not_duplicate_authority() {
        let tick = 30_000;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 1);
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(1_000, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 3)]),
            tick
        ));
        // Receive 1002 before 1001, then recover the missing sequence from a
        // later overlapping bundle. Neither path can consume twice.
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(1_002, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 5)]),
            tick.wrapping_add(1)
        ));
        assert!(record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(1_001, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 4)]),
            tick.wrapping_add(1)
        ));
        assert_ne!(
            canonical_bits(&state, player_id, 1_001) & CLIENT_INPUT_ROCKET_ACTIVE_BIT,
            0
        );

        // Raw pulse history, not the canonical server bit, pins overlap
        // rewrites even when the server timer would still be active.
        assert!(!record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(1_001, &[(0, 4)]),
            tick.wrapping_add(2)
        ));

        for sequence in 1_003..1_080 {
            assert!(record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_input_packet(sequence, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 6)]),
                tick.wrapping_add(2)
            ));
        }
        {
            let state = state.lock().expect("state");
            let client = state.clients.get(&player_id).expect("client");
            assert_eq!(client.server_weapons.rockets, 0);
            assert_eq!(
                client.recent_input_records.len(),
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS
            );
            assert_eq!(
                client.recent_input_rocket_assertions.len(),
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS
            );
        }

        // A previous-life handler cannot mutate the new life's inventory or
        // history even when its sequence and bit pattern look fresh.
        {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.life_seq = client.life_seq.wrapping_add(1).max(1);
            client.clear_server_runtime();
            client.alive = true;
            client.reset_server_loadout_for_ship_at_tick(tick.wrapping_add(3));
            client.server_weapons.rockets = 2;
        }
        assert!(!record_client_input_observation_at_tick(
            &state,
            identity,
            &encoded_input_packet(2_000, &[(CLIENT_INPUT_ROCKET_ACTIVE_BIT, 0)]),
            tick.wrapping_add(3)
        ));
        let state = state.lock().expect("state");
        let client = state.clients.get(&player_id).expect("client");
        assert_eq!(client.server_weapons.rockets, 2);
        assert_eq!(client.server_rocket_until_tick, None);
        assert!(client.recent_input_records.is_empty());
    }

    #[test]
    fn wholly_lost_rocket_interval_does_not_wedge_later_input_history_across_wrap() {
        let tick = 35_000;
        let (state, identity, player_id) = joined_runtime_at_tick(tick, 3);
        let start = u16::MAX - 39;
        for offset in 0..96_u16 {
            let sequence = start.wrapping_add(offset);
            assert!(record_client_input_observation_at_tick(
                &state,
                identity,
                &encoded_input_packet(sequence, &[(0, (offset % 40) as u8)]),
                tick.wrapping_add(u32::from(offset)),
            ));
        }
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        assert_eq!(client.last_input_seq, Some(start.wrapping_add(95)));
        assert_eq!(
            client.recent_input_records.len(),
            crate::state::MAX_RECENT_INPUT_OBSERVATIONS
        );
        assert_eq!(client.server_weapons.rockets, 3);
        assert_eq!(client.server_rocket_until_tick, None);
        assert!(client.server_rocket_authorizations.is_empty());
        assert!(
            client
                .recent_input_records
                .iter()
                .all(|record| record.input.bits & CLIENT_INPUT_ROCKET_ACTIVE_BIT == 0)
        );
    }

    #[test]
    fn reliable_portal_ingress_commits_only_on_server_tick_and_emits_exact_terminal() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, player_id, 0);
        while rx.try_recv().is_ok() {}
        let request = {
            let mut state = state.lock().expect("state");
            let client = state.clients.get_mut(&player_id).expect("client");
            client.server_weapons.portals = 1;
            let sequence = u16::MAX;
            client.last_position_sequence = Some(sequence);
            client
                .canonical_position_witnesses
                .push_back(crate::state::CanonicalPositionWitness {
                    sequence,
                    life_seq: client.life_seq,
                    state: crate::state::ServerSimMovement {
                        position_x: 1_000.0,
                        position_y: 1_000.0,
                        orientation: 0.0,
                        velocity_x: 0.0,
                        velocity_y: 0.0,
                        energy: 1_000.0,
                        max_energy: 1_000.0,
                    },
                    wire_x: 1_000,
                    wire_y: 1_000,
                    wire_velocity_x: 0,
                    wire_velocity_y: 0,
                    direction: 0,
                    door_seed: 0,
                    door_phase_tick: crate::wire::current_tick(),
                });
            crate::portal_placement::PortalPlacementRequest {
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: client.arena_stream_id,
                player_id,
                life_seq: client.life_seq,
                action_id: u32::MAX,
                position_sequence: sequence,
            }
        };
        let mut joined = true;
        handle_client_reliable_payload(
            &state,
            player_id,
            77,
            &request.encode(),
            "portal-test",
            &mut joined,
        );
        {
            let state = state.lock().expect("state");
            assert_eq!(state.pending_portal_placements.len(), 1);
            assert_eq!(state.clients[&player_id].server_weapons.portals, 1);
        }

        crate::server_tick::run_server_tick(&state, Instant::now());
        let terminal = rx
            .try_iter()
            .find_map(|packet| {
                crate::reliable::reliable_payload(&packet).and_then(|payload| {
                    crate::portal_placement::PortalPlacementAccepted::parse(payload).ok()
                })
            })
            .expect("tick emits a private Portal terminal");
        assert_eq!(terminal.action_id, request.action_id);
        assert_eq!(terminal.position_sequence, request.position_sequence);
        assert_eq!(
            terminal.outcome,
            crate::portal_placement::PortalPlacementAccepted::OUTCOME_APPLIED
        );
        let state = state.lock().expect("state");
        assert!(state.pending_portal_placements.is_empty());
        assert_eq!(state.clients[&player_id].server_weapons.portals, 0);
    }
}
