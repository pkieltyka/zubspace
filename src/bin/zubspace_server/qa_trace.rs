//! Bounded QA-only browser/server trace correlation.
//!
//! Nothing in this module is consulted by gameplay admission, timing,
//! collision, damage, or lifecycle code. Missing, forged, delayed, or
//! withheld diagnostics therefore cannot affect authority.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::authority_identity::PlayerSessionIdentity;
use crate::qa_trace_protocol::QaTraceContext;

pub(crate) const QA_TRACE_MAX_UPLOAD_BYTES: usize = 256 * 1024;
pub(crate) const QA_TRACE_MAX_SERVER_EVENTS: usize = 512;
pub(crate) const QA_TRACE_MAX_SERVER_BYTES: usize = 128 * 1024;
pub(crate) const QA_TRACE_MAX_RETAINED_BUNDLES: usize = 16;
pub(crate) const QA_TRACE_MAX_UPLOADS_PER_WINDOW: usize = 4;
pub(crate) const QA_TRACE_UPLOAD_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(crate) struct QaTraceError {
    pub(crate) status: &'static str,
    pub(crate) code: &'static str,
}

impl QaTraceError {
    fn new(status: &'static str, code: &'static str) -> Self {
        Self { status, code }
    }
}

#[derive(Clone, Debug)]
struct QaTraceServerEvent {
    value: Arc<Value>,
    encoded_bytes: usize,
}

impl QaTraceServerEvent {
    fn new(value: Value) -> Self {
        let encoded_bytes = serde_json::to_vec(&value).map_or(usize::MAX, |bytes| bytes.len());
        Self {
            value: Arc::new(value),
            encoded_bytes,
        }
    }
}

#[derive(Clone, Debug)]
struct QaTraceServerRing {
    events: VecDeque<QaTraceServerEvent>,
    retained_bytes: usize,
    overwritten_events: u64,
    oversized_events: u64,
}

impl Default for QaTraceServerRing {
    fn default() -> Self {
        Self {
            events: VecDeque::with_capacity(QA_TRACE_MAX_SERVER_EVENTS),
            retained_bytes: 0,
            overwritten_events: 0,
            oversized_events: 0,
        }
    }
}

impl QaTraceServerRing {
    fn push(&mut self, event: Value) {
        self.push_shared(QaTraceServerEvent::new(event));
    }

    fn push_shared(&mut self, event: QaTraceServerEvent) {
        let bytes = event.encoded_bytes;
        if bytes > QA_TRACE_MAX_SERVER_BYTES {
            self.oversized_events = self.oversized_events.saturating_add(1);
            return;
        }
        while self.events.len() >= QA_TRACE_MAX_SERVER_EVENTS
            || self.retained_bytes.saturating_add(bytes) > QA_TRACE_MAX_SERVER_BYTES
        {
            let Some(evicted) = self.events.pop_front() else {
                break;
            };
            self.retained_bytes = self.retained_bytes.saturating_sub(evicted.encoded_bytes);
            self.overwritten_events = self.overwritten_events.saturating_add(1);
        }
        self.retained_bytes = self.retained_bytes.saturating_add(bytes);
        self.events.push_back(event);
    }

    fn artifact(&self) -> Value {
        json!({
            "limits": {
                "maxEvents": QA_TRACE_MAX_SERVER_EVENTS,
                "maxBytes": QA_TRACE_MAX_SERVER_BYTES,
            },
            "retainedEvents": self.events.len(),
            "retainedBytes": self.retained_bytes,
            "overwrittenEvents": self.overwritten_events,
            "oversizedEvents": self.oversized_events,
            "truncated": self.overwritten_events != 0 || self.oversized_events != 0,
            "events": self.events.iter().map(|event| event.value.as_ref()).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug)]
struct QaTraceSession {
    context: QaTraceContext,
    player_session: PlayerSessionIdentity,
    created_serial: u64,
    server_ring: QaTraceServerRing,
    upload_window_started: Instant,
    uploads_in_window: usize,
    next_upload_serial: u64,
    retained_upload_serial: u64,
    retained_joined_bundle: Option<Arc<Value>>,
}

/// A bounded immutable snapshot of the ring. Cloning it under the server-state
/// lock copies Arc handles, never the event JSON. Assembly happens after unlock.
#[derive(Debug)]
pub(crate) struct QaTraceUploadSnapshot {
    context: QaTraceContext,
    player_session: PlayerSessionIdentity,
    serial: u64,
    server_ring: QaTraceServerRing,
}

#[derive(Debug)]
pub(crate) struct CompletedQaTraceUpload {
    context: QaTraceContext,
    serial: u64,
    artifact: Arc<Value>,
}

impl QaTraceUploadSnapshot {
    pub(crate) fn finish(self, browser: Value) -> CompletedQaTraceUpload {
        let joined = json!({
            "schema": "zubspace-qa-trace-joined-v1",
            "version": 1,
            "bundleId": self.context.bundle_id_hex(),
            "scope": {
                "serverSessionId": self.context.server_session_id.to_string(),
                "arenaStreamId": self.context.arena_stream_id.to_string(),
                "playerId": self.context.player_id,
                "playerSessionId": self.player_session.session_id.get().to_string(),
            },
            "server": self.server_ring.artifact(),
            "provenance": { "serverBinary": server_binary_provenance(self.context) },
            "browser": browser,
        });
        CompletedQaTraceUpload {
            context: self.context,
            serial: self.serial,
            artifact: Arc::new(joined),
        }
    }
}

/// Parsing and recursive schema validation do not need access to mutable game
/// state. Callers must authenticate first and recheck at snapshot/retention.
pub(crate) fn parse_upload(body: &[u8], context: QaTraceContext) -> Result<Value, QaTraceError> {
    if body.len() > QA_TRACE_MAX_UPLOAD_BYTES {
        return Err(QaTraceError::new(
            "413 Content Too Large",
            "qa_trace_too_large",
        ));
    }
    let browser: Value = serde_json::from_slice(body)
        .map_err(|_| QaTraceError::new("400 Bad Request", "invalid_trace_json"))?;
    validate_browser_bundle(&browser, context)?;
    Ok(browser)
}

#[derive(Debug)]
pub(crate) struct QaTraceRegistry {
    secret: [u8; 32],
    next_serial: u64,
    sessions: BTreeMap<[u8; 16], QaTraceSession>,
}

impl Default for QaTraceRegistry {
    fn default() -> Self {
        Self {
            secret: [0; 32],
            next_serial: 0,
            sessions: BTreeMap::new(),
        }
    }
}

impl QaTraceRegistry {
    pub(crate) fn authenticate_client_debug(
        &self,
        bundle: &str,
        token: &str,
    ) -> Option<(QaTraceContext, PlayerSessionIdentity)> {
        if self.secret == [0; 32] {
            return None;
        }
        let bundle = decode_hex::<16>(bundle)?;
        let token = decode_hex::<32>(token)?;
        let session = self.sessions.get(&bundle)?;
        constant_time_eq(&session.context.upload_token, &token)
            .then_some((session.context, session.player_session))
    }

    pub(crate) fn with_process_secret() -> Self {
        let mut registry = Self::default();
        if getrandom::fill(&mut registry.secret).is_err() {
            // Failure is fail-closed for upload authentication. A zero secret
            // is never exposed as an enabled production endpoint; server
            // startup can still proceed with trace upload disabled.
            registry.secret = [0; 32];
        }
        registry
    }

    pub(crate) fn register(
        &mut self,
        player_session: PlayerSessionIdentity,
        arena_stream_id: u64,
    ) -> QaTraceContext {
        self.next_serial = self.next_serial.checked_add(1).unwrap_or(1);
        let server_session_id = crate::wire::server_session_id();
        let bundle_id = self.derive::<16>(
            b"zubspace-qa-trace-bundle-v1",
            player_session,
            arena_stream_id,
            self.next_serial,
        );
        let upload_token = self.derive::<32>(
            b"zubspace-qa-trace-upload-v1",
            player_session,
            arena_stream_id,
            self.next_serial,
        );
        let context = QaTraceContext {
            server_session_id,
            arena_stream_id,
            player_id: player_session.player_id,
            bundle_id,
            upload_token,
            server_binary_sha256: crate::binary_identity::captured()
                .map_or([0; 32], |identity| identity.sha256),
            server_binary_bytes: crate::binary_identity::captured()
                .map_or(0, |identity| identity.bytes),
        };
        while self.sessions.len() >= QA_TRACE_MAX_RETAINED_BUNDLES {
            let Some(oldest) = self
                .sessions
                .iter()
                .min_by_key(|(_, session)| session.created_serial)
                .map(|(id, _)| *id)
            else {
                break;
            };
            self.sessions.remove(&oldest);
        }
        let mut session = QaTraceSession {
            context,
            player_session,
            created_serial: self.next_serial,
            server_ring: QaTraceServerRing::default(),
            upload_window_started: Instant::now(),
            uploads_in_window: 0,
            next_upload_serial: 0,
            retained_upload_serial: 0,
            retained_joined_bundle: None,
        };
        session.server_ring.push(json!({
            "type": "qa_trace_context_issued",
            "serverSessionId": server_session_id.to_string(),
            "arenaStreamId": arena_stream_id.to_string(),
            "playerId": player_session.player_id,
            "playerSessionId": player_session.session_id.get().to_string(),
            "bundleId": context.bundle_id_hex(),
        }));
        self.sessions.insert(bundle_id, session);
        context
    }

    fn derive<const N: usize>(
        &self,
        domain: &[u8],
        player_session: PlayerSessionIdentity,
        arena_stream_id: u64,
        serial: u64,
    ) -> [u8; N] {
        debug_assert!(N <= 32);
        let mut digest = Sha256::new();
        digest.update(domain);
        digest.update(self.secret);
        digest.update(crate::wire::server_session_id().to_le_bytes());
        digest.update(player_session.player_id.to_le_bytes());
        digest.update(player_session.session_id.get().to_le_bytes());
        digest.update(arena_stream_id.to_le_bytes());
        digest.update(serial.to_le_bytes());
        let digest = digest.finalize();
        let mut output = [0; N];
        output.copy_from_slice(&digest[..N]);
        output
    }

    pub(crate) fn record_for_bundle(&mut self, bundle_id: [u8; 16], event: Value) {
        if let Some(session) = self.sessions.get_mut(&bundle_id) {
            session.server_ring.push(event);
        }
    }

    /// Construct diagnostic data only while its bounded context is retained.
    /// Client handles may outlive registry eviction; those IDs are not sinks.
    pub(crate) fn record_for_bundle_lazy(
        &mut self,
        bundle_id: [u8; 16],
        make_event: impl FnOnce() -> Value,
    ) {
        if let Some(session) = self.sessions.get_mut(&bundle_id) {
            session.server_ring.push(make_event());
        }
    }

    pub(crate) fn record_for_player(&mut self, player_id: u16, event: Value) {
        let mut sessions = self
            .sessions
            .values_mut()
            .filter(|session| session.context.player_id == player_id);
        let Some(first) = sessions.next() else {
            return;
        };
        let shared = QaTraceServerEvent::new(event);
        first.server_ring.push_shared(shared.clone());
        for session in sessions {
            session.server_ring.push_shared(shared.clone());
        }
    }

    /// Record an arena-wide authority fact for every currently retained QA
    /// observer. This remains diagnostics-only: the registry has no handle to
    /// gameplay state and its bounded overwrite behavior cannot backpressure
    /// simulation, admission, or delivery queues.
    /// Whether any QA session is retained. Hot paths check it before building
    /// a per-shot or per-hit event that no session would keep.
    pub(crate) fn is_recording(&self) -> bool {
        !self.sessions.is_empty()
    }

    pub(crate) fn record_for_all(&mut self, event: Value) {
        if self.sessions.is_empty() {
            return;
        }
        let shared = QaTraceServerEvent::new(event);
        for session in self.sessions.values_mut() {
            session.server_ring.push_shared(shared.clone());
        }
    }

    pub(crate) fn upload_context(
        &self,
        bundle_id_hex: &str,
        supplied_token_hex: &str,
        now: Instant,
    ) -> Result<QaTraceContext, QaTraceError> {
        let bundle_id = decode_hex::<16>(bundle_id_hex)
            .ok_or_else(|| QaTraceError::new("400 Bad Request", "invalid_bundle_id"))?;
        let supplied_token = decode_hex::<32>(supplied_token_hex)
            .ok_or_else(|| QaTraceError::new("401 Unauthorized", "invalid_trace_token"))?;
        let session = self
            .sessions
            .get(&bundle_id)
            .ok_or_else(|| QaTraceError::new("404 Not Found", "unknown_trace_bundle"))?;
        if !constant_time_eq(&session.context.upload_token, &supplied_token) {
            return Err(QaTraceError::new("401 Unauthorized", "invalid_trace_token"));
        }
        if now.saturating_duration_since(session.upload_window_started) < QA_TRACE_UPLOAD_WINDOW
            && session.uploads_in_window >= QA_TRACE_MAX_UPLOADS_PER_WINDOW
        {
            return Err(QaTraceError::new(
                "429 Too Many Requests",
                "qa_trace_rate_limited",
            ));
        }
        Ok(session.context)
    }

    /// Recheck the exact retained context and consume quota atomically after
    /// external parsing. Concurrent uploads cannot race past the existing cap.
    pub(crate) fn capture_upload(
        &mut self,
        context: QaTraceContext,
        now: Instant,
    ) -> Result<QaTraceUploadSnapshot, QaTraceError> {
        self.upload_context(&context.bundle_id_hex(), &context.upload_token_hex(), now)?;
        let session = self
            .sessions
            .get_mut(&context.bundle_id)
            .expect("authenticated retained context");
        if session.context != context {
            return Err(QaTraceError::new("401 Unauthorized", "invalid_trace_token"));
        }
        if now.saturating_duration_since(session.upload_window_started) >= QA_TRACE_UPLOAD_WINDOW {
            session.upload_window_started = now;
            session.uploads_in_window = 0;
        }
        session.next_upload_serial =
            session.next_upload_serial.checked_add(1).ok_or_else(|| {
                QaTraceError::new("503 Service Unavailable", "trace_serial_exhausted")
            })?;
        session.uploads_in_window += 1;
        Ok(QaTraceUploadSnapshot {
            context,
            player_session: session.player_session,
            serial: session.next_upload_serial,
            server_ring: session.server_ring.clone(),
        })
    }

    /// Retain an immutable assembled artifact without copying its JSON. Return
    /// replaced storage to the caller so it can be destroyed after unlocking.
    /// A slower older upload must not overwrite a newer accepted snapshot.
    pub(crate) fn retain_upload(
        &mut self,
        upload: &CompletedQaTraceUpload,
    ) -> Result<(Arc<Value>, Option<Arc<Value>>), QaTraceError> {
        let session = self
            .sessions
            .get_mut(&upload.context.bundle_id)
            .ok_or_else(|| QaTraceError::new("404 Not Found", "unknown_trace_bundle"))?;
        if session.context != upload.context {
            return Err(QaTraceError::new("401 Unauthorized", "invalid_trace_token"));
        }
        let replaced = if upload.serial > session.retained_upload_serial {
            session.retained_upload_serial = upload.serial;
            session
                .retained_joined_bundle
                .replace(Arc::clone(&upload.artifact))
        } else {
            None
        };
        Ok((Arc::clone(&upload.artifact), replaced))
    }

    #[cfg(test)]
    pub(crate) fn upload(
        &mut self,
        body: &[u8],
        bundle_id_hex: &str,
        supplied_token_hex: &str,
        now: Instant,
    ) -> Result<Value, QaTraceError> {
        if body.len() > QA_TRACE_MAX_UPLOAD_BYTES {
            return Err(QaTraceError::new(
                "413 Content Too Large",
                "qa_trace_too_large",
            ));
        }
        let context = self.upload_context(bundle_id_hex, supplied_token_hex, now)?;
        let browser = parse_upload(body, context)?;
        let completed = self.capture_upload(context, now)?.finish(browser);
        let (joined, _replaced) = self.retain_upload(&completed)?;
        Ok(joined.as_ref().clone())
    }

    pub(crate) fn retained(&self, bundle_id_hex: &str) -> Result<Arc<Value>, QaTraceError> {
        let bundle_id = decode_hex::<16>(bundle_id_hex)
            .ok_or_else(|| QaTraceError::new("400 Bad Request", "invalid_bundle_id"))?;
        let session = self
            .sessions
            .get(&bundle_id)
            .ok_or_else(|| QaTraceError::new("404 Not Found", "unknown_trace_bundle"))?;
        session
            .retained_joined_bundle
            .clone()
            .ok_or_else(|| QaTraceError::new("404 Not Found", "trace_bundle_not_marked"))
    }
}

fn server_binary_provenance(context: QaTraceContext) -> Value {
    json!({
        "kind": "running-executable-sha256",
        "status": if context.server_binary_bytes == 0 { "unavailable" } else { "ready" },
        "sha256": if context.server_binary_bytes == 0 { None } else { Some(context.server_binary_sha256_hex()) },
        "bytes": context.server_binary_bytes,
    })
}

fn validate_browser_bundle(value: &Value, context: QaTraceContext) -> Result<(), QaTraceError> {
    let object = value
        .as_object()
        .ok_or_else(|| QaTraceError::new("400 Bad Request", "invalid_trace_schema"))?;
    const ALLOWED: &[&str] = &[
        "schema",
        "version",
        "bundleId",
        "scope",
        "marker",
        "browser",
        "arena",
        "provenance",
        "options",
        "impairment",
        "rings",
        "limits",
        "overwritten",
        "truncated",
        "location",
    ];
    if object.keys().any(|key| !ALLOWED.contains(&key.as_str()))
        || object.get("schema").and_then(Value::as_str) != Some("zubspace-qa-trace-v1")
        || object.get("version").and_then(Value::as_u64) != Some(1)
        || object.get("bundleId").and_then(Value::as_str) != Some(context.bundle_id_hex().as_str())
    {
        return Err(QaTraceError::new("400 Bad Request", "invalid_trace_schema"));
    }
    let scope = object
        .get("scope")
        .and_then(Value::as_object)
        .ok_or_else(|| QaTraceError::new("400 Bad Request", "invalid_trace_scope"))?;
    if scope.get("serverSessionId").and_then(Value::as_str)
        != Some(context.server_session_id.to_string().as_str())
        || scope.get("arenaStreamId").and_then(Value::as_str)
            != Some(context.arena_stream_id.to_string().as_str())
        || scope.get("playerId").and_then(Value::as_u64) != Some(u64::from(context.player_id))
    {
        return Err(QaTraceError::new("403 Forbidden", "trace_scope_mismatch"));
    }
    if value.pointer("/provenance/serverBinary") != Some(&server_binary_provenance(context)) {
        return Err(QaTraceError::new(
            "400 Bad Request",
            "trace_server_binary_mismatch",
        ));
    }
    let mut nodes = 0usize;
    validate_value_shape(value, 0, &mut nodes)?;
    Ok(())
}

fn validate_value_shape(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), QaTraceError> {
    *nodes = nodes.saturating_add(1);
    if depth > 10 || *nodes > 20_000 {
        return Err(QaTraceError::new(
            "400 Bad Request",
            "trace_schema_too_complex",
        ));
    }
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let lower = key.to_ascii_lowercase();
                const FORBIDDEN: &[&str] = &[
                    "sdp",
                    "icecandidate",
                    "credential",
                    "authorization",
                    "uploadtoken",
                    "authtoken",
                    "chat",
                    "voice",
                    "audiopayload",
                    "peeraddress",
                ];
                if key.len() > 80 || FORBIDDEN.iter().any(|needle| lower.contains(needle)) {
                    return Err(QaTraceError::new(
                        "400 Bad Request",
                        "forbidden_trace_field",
                    ));
                }
                validate_value_shape(value, depth + 1, nodes)?;
            }
        }
        Value::Array(values) => {
            if values.len() > 2_000 {
                return Err(QaTraceError::new(
                    "400 Bad Request",
                    "trace_array_too_large",
                ));
            }
            for value in values {
                validate_value_shape(value, depth + 1, nodes)?;
            }
        }
        Value::String(value) if value.len() > 2_048 => {
            return Err(QaTraceError::new(
                "400 Bad Request",
                "trace_string_too_large",
            ));
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn constant_time_eq(expected: &[u8], supplied: &[u8]) -> bool {
    let mut difference = expected.len() ^ supplied.len();
    let length = expected.len().max(supplied.len());
    for index in 0..length {
        difference |= usize::from(
            expected.get(index).copied().unwrap_or(0) ^ supplied.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut decoded = [0; N];
    for (index, byte) in decoded.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority_identity::PlayerSessionId;

    fn registry_and_context() -> (QaTraceRegistry, QaTraceContext) {
        let mut registry = QaTraceRegistry::default();
        registry.secret = [7; 32];
        let context =
            registry.register(PlayerSessionIdentity::new(9, PlayerSessionId::new(11)), 13);
        (registry, context)
    }

    #[test]
    fn lazy_bundle_record_preserves_retained_event_and_skips_evicted_context() {
        let (mut registry, context) = registry_and_context();
        let event =
            json!({"type": "snapshot_emitted", "nearActors": [{"playerId": 3, "lifeSeq": 7}]});
        let mut calls = 0;
        registry.record_for_bundle_lazy(context.bundle_id, || {
            calls += 1;
            event.clone()
        });
        assert_eq!(calls, 1);
        assert_eq!(
            registry.sessions[&context.bundle_id]
                .server_ring
                .events
                .back()
                .map(|event| event.value.as_ref()),
            Some(&event)
        );
        for serial in 0..QA_TRACE_MAX_RETAINED_BUNDLES {
            registry.register(
                PlayerSessionIdentity::new(10, PlayerSessionId::new(100 + serial as u64)),
                13,
            );
        }
        assert!(!registry.sessions.contains_key(&context.bundle_id));
        assert_eq!(registry.sessions.len(), QA_TRACE_MAX_RETAINED_BUNDLES);
        registry.record_for_bundle_lazy(context.bundle_id, || {
            panic!("evicted context must not construct discarded trace JSON")
        });
        registry.record_for_bundle_lazy([0; 16], || {
            panic!("unknown context must not construct discarded trace JSON")
        });
    }

    fn valid_bundle(context: QaTraceContext) -> Value {
        json!({
            "schema": "zubspace-qa-trace-v1",
            "version": 1,
            "bundleId": context.bundle_id_hex(),
            "scope": {
                "serverSessionId": context.server_session_id.to_string(),
                "arenaStreamId": context.arena_stream_id.to_string(),
                "playerId": context.player_id,
                "lifeSeq": 4,
            },
            "marker": {"monotonicMs": 12, "postTailMs": 500},
            "browser": {"name": "Firefox", "version": "123"},
            "arena": "chaos-zone",
            "provenance": {"build": "test", "serverBinary": server_binary_provenance(context)},
            "options": {"trace": true},
            "impairment": {"requested": null, "realized": null},
            "rings": {"metrics": [], "sprites": [], "inputs": [], "damage": []},
            "limits": {"maxBytes": QA_TRACE_MAX_UPLOAD_BYTES},
            "overwritten": {"metrics": 0},
            "truncated": false,
            "location": {"origin": "http://pax:7000", "path": "/index.html"},
        })
    }

    #[test]
    fn upload_preserves_browser_f64_diagnostic_bits() {
        let (mut registry, context) = registry_and_context();
        // Retained Chrome first-stall upload changed this diagnostic by one
        // representable f64 step with the default JSON parsing path.
        let expected = "1173.5999999195337".parse::<f64>().unwrap();
        let mut bundle = valid_bundle(context);
        bundle["marker"]["workerCpuMs"] = json!(expected);
        let joined = registry
            .upload(
                &serde_json::to_vec(&bundle).unwrap(),
                &context.bundle_id_hex(),
                &context.upload_token_hex(),
                Instant::now(),
            )
            .unwrap();
        let observed = joined["browser"]["marker"]["workerCpuMs"].as_f64().unwrap();
        assert_eq!(observed.to_bits(), expected.to_bits());
    }

    #[test]
    fn upload_is_scope_and_token_bound_and_redacts_token_from_artifact() {
        let (mut registry, context) = registry_and_context();
        let body = serde_json::to_vec(&valid_bundle(context)).unwrap();
        let joined = registry
            .upload(
                &body,
                &context.bundle_id_hex(),
                &context.upload_token_hex(),
                Instant::now(),
            )
            .unwrap();
        let encoded = serde_json::to_string(&joined).unwrap();
        assert!(encoded.contains(&context.bundle_id_hex()));
        assert!(!encoded.contains(&context.upload_token_hex()));
        assert!(
            registry
                .upload(
                    &body,
                    &context.bundle_id_hex(),
                    &"00".repeat(32),
                    Instant::now()
                )
                .is_err()
        );
    }

    #[test]
    fn upload_cannot_invent_or_omit_the_running_server_identity() {
        let (mut registry, mut context) = registry_and_context();
        context.server_binary_sha256 = [0x12; 32];
        context.server_binary_bytes = 42;
        registry
            .sessions
            .get_mut(&context.bundle_id)
            .unwrap()
            .context = context;
        for replacement in [
            Value::Null,
            json!({
                "kind": "running-executable-sha256", "status": "ready",
                "sha256": "34".repeat(32), "bytes": 42,
            }),
            json!({
                "kind": "running-executable-sha256", "status": "ready",
                "sha256": "12".repeat(32), "bytes": 43,
            }),
        ] {
            let mut bundle = valid_bundle(context);
            bundle["provenance"]["serverBinary"] = replacement;
            assert_eq!(
                registry
                    .upload(
                        &serde_json::to_vec(&bundle).unwrap(),
                        &context.bundle_id_hex(),
                        &context.upload_token_hex(),
                        Instant::now()
                    )
                    .unwrap_err()
                    .code,
                "trace_server_binary_mismatch"
            );
        }
        let joined = registry
            .upload(
                &serde_json::to_vec(&valid_bundle(context)).unwrap(),
                &context.bundle_id_hex(),
                &context.upload_token_hex(),
                Instant::now(),
            )
            .unwrap();
        assert_eq!(
            joined["provenance"]["serverBinary"],
            server_binary_provenance(context)
        );
        assert_eq!(registry.sessions[&context.bundle_id].uploads_in_window, 1);
    }

    #[test]
    fn upload_rejects_scope_mismatch_forbidden_fields_and_rate_overflow() {
        let (mut registry, context) = registry_and_context();
        let mut mismatch = valid_bundle(context);
        mismatch["scope"]["playerId"] = json!(10);
        assert_eq!(
            registry
                .upload(
                    &serde_json::to_vec(&mismatch).unwrap(),
                    &context.bundle_id_hex(),
                    &context.upload_token_hex(),
                    Instant::now(),
                )
                .unwrap_err()
                .code,
            "trace_scope_mismatch"
        );
        let mut forbidden = valid_bundle(context);
        forbidden["rings"]["iceCandidates"] = json!(["secret"]);
        assert_eq!(
            registry
                .upload(
                    &serde_json::to_vec(&forbidden).unwrap(),
                    &context.bundle_id_hex(),
                    &context.upload_token_hex(),
                    Instant::now(),
                )
                .unwrap_err()
                .code,
            "forbidden_trace_field"
        );
        let body = serde_json::to_vec(&valid_bundle(context)).unwrap();
        let now = Instant::now();
        for _ in 0..QA_TRACE_MAX_UPLOADS_PER_WINDOW {
            registry
                .upload(
                    &body,
                    &context.bundle_id_hex(),
                    &context.upload_token_hex(),
                    now,
                )
                .unwrap();
        }
        assert_eq!(
            registry
                .upload(
                    &body,
                    &context.bundle_id_hex(),
                    &context.upload_token_hex(),
                    now,
                )
                .unwrap_err()
                .code,
            "qa_trace_rate_limited"
        );
    }

    #[test]
    fn upload_rejects_oversize_and_unknown_schema_fields_before_retention() {
        let (mut registry, context) = registry_and_context();
        let oversized = vec![b' '; QA_TRACE_MAX_UPLOAD_BYTES + 1];
        assert_eq!(
            registry
                .upload(
                    &oversized,
                    &context.bundle_id_hex(),
                    &context.upload_token_hex(),
                    Instant::now(),
                )
                .unwrap_err()
                .code,
            "qa_trace_too_large"
        );
        let mut unknown = valid_bundle(context);
        unknown["unexpectedAuthorityControl"] = json!(true);
        assert_eq!(
            registry
                .upload(
                    &serde_json::to_vec(&unknown).unwrap(),
                    &context.bundle_id_hex(),
                    &context.upload_token_hex(),
                    Instant::now(),
                )
                .unwrap_err()
                .code,
            "invalid_trace_schema"
        );
        assert!(
            registry
                .sessions
                .get(&context.bundle_id)
                .unwrap()
                .retained_joined_bundle
                .is_none()
        );
    }

    #[test]
    fn server_ring_discloses_bounded_overwrite() {
        let (mut registry, context) = registry_and_context();
        for serial in 0..=QA_TRACE_MAX_SERVER_EVENTS {
            registry.record_for_bundle(context.bundle_id, json!({"serial": serial}));
        }
        let body = serde_json::to_vec(&valid_bundle(context)).unwrap();
        let joined = registry
            .upload(
                &body,
                &context.bundle_id_hex(),
                &context.upload_token_hex(),
                Instant::now(),
            )
            .unwrap();
        assert_eq!(joined["server"]["truncated"], true);
        assert!(joined["server"]["overwrittenEvents"].as_u64().unwrap() > 0);
        assert_eq!(
            joined["server"]["events"].as_array().unwrap().len(),
            QA_TRACE_MAX_SERVER_EVENTS
        );
    }

    #[test]
    fn arena_wide_diagnostics_are_bounded_and_never_create_gameplay_feedback() {
        let (mut registry, context) = registry_and_context();
        let before_session_count = registry.sessions.len();
        let before_serial = registry.next_serial;
        for serial in 0..QA_TRACE_MAX_SERVER_EVENTS * 2 {
            registry.record_for_all(json!({
                "type": "server_projectile_spawned",
                "serial": serial,
            }));
        }
        assert_eq!(registry.sessions.len(), before_session_count);
        assert_eq!(registry.next_serial, before_serial);

        let body = serde_json::to_vec(&valid_bundle(context)).unwrap();
        let joined = registry
            .upload(
                &body,
                &context.bundle_id_hex(),
                &context.upload_token_hex(),
                Instant::now(),
            )
            .unwrap();
        assert_eq!(joined["server"]["truncated"], true);
        assert_eq!(
            joined["server"]["events"].as_array().unwrap().len(),
            QA_TRACE_MAX_SERVER_EVENTS
        );
    }

    #[test]
    fn detached_upload_keeps_its_exact_ring_cut_and_newest_retention_order() {
        let (mut registry, context) = registry_and_context();
        registry.record_for_bundle(context.bundle_id, json!({"type":"before"}));
        let first = registry.capture_upload(context, Instant::now()).unwrap();
        assert!(
            Arc::ptr_eq(
                &first.server_ring.events.back().unwrap().value,
                &registry.sessions[&context.bundle_id]
                    .server_ring
                    .events
                    .back()
                    .unwrap()
                    .value,
            ),
            "the locked snapshot must share immutable event storage"
        );
        registry.record_for_bundle(context.bundle_id, json!({"type":"after"}));
        let second = registry.capture_upload(context, Instant::now()).unwrap();
        let second = second.finish(valid_bundle(context));
        let (newest, replaced) = registry.retain_upload(&second).unwrap();
        assert!(replaced.is_none());
        let first = first.finish(valid_bundle(context));
        let (older, replaced) = registry.retain_upload(&first).unwrap();
        assert!(
            replaced.is_none(),
            "older completion cannot displace newer retention"
        );
        assert!(Arc::ptr_eq(
            &registry.retained(&context.bundle_id_hex()).unwrap(),
            &newest
        ));
        assert_eq!(
            older["server"]["events"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["type"],
            "before"
        );
        assert_eq!(
            newest["server"]["events"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["type"],
            "after"
        );
    }

    #[test]
    fn detached_parse_and_eviction_do_not_restore_a_retired_bundle() {
        let (mut registry, context) = registry_and_context();
        assert_eq!(
            parse_upload(b"{", context).unwrap_err().code,
            "invalid_trace_json"
        );
        assert_eq!(registry.sessions[&context.bundle_id].uploads_in_window, 0);
        let mut wrong_scope = context;
        wrong_scope.arena_stream_id += 1;
        assert_eq!(
            registry
                .capture_upload(wrong_scope, Instant::now())
                .unwrap_err()
                .code,
            "invalid_trace_token"
        );
        let captured = registry.capture_upload(context, Instant::now()).unwrap();
        for id in 0..QA_TRACE_MAX_RETAINED_BUNDLES {
            registry.register(
                PlayerSessionIdentity::new(id as u16, PlayerSessionId::new(50 + id as u64)),
                15,
            );
        }
        let completed = captured.finish(valid_bundle(context));
        assert_eq!(
            registry.retain_upload(&completed).unwrap_err().code,
            "unknown_trace_bundle"
        );
        assert_eq!(
            registry
                .capture_upload(context, Instant::now())
                .unwrap_err()
                .code,
            "unknown_trace_bundle"
        );
        assert!(!registry.sessions.contains_key(&context.bundle_id));
        assert_eq!(registry.sessions.len(), QA_TRACE_MAX_RETAINED_BUNDLES);
    }

    #[test]
    fn simultaneous_detached_uploads_recheck_quota_after_parsing() {
        use std::sync::{Barrier, Mutex};
        let (registry, context) = registry_and_context();
        let registry = Arc::new(Mutex::new(registry));
        let barrier = Arc::new(Barrier::new(8));
        let now = Instant::now();
        let handles = (0..8)
            .map(|_| {
                let registry = Arc::clone(&registry);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let context = registry
                        .lock()
                        .unwrap()
                        .upload_context(&context.bundle_id_hex(), &context.upload_token_hex(), now)
                        .unwrap();
                    let body = serde_json::to_vec(&valid_bundle(context)).unwrap();
                    let browser = parse_upload(&body, context).unwrap();
                    barrier.wait();
                    let capture = registry
                        .lock()
                        .unwrap()
                        .capture_upload(context, now)
                        .map_err(|error| error.code)?;
                    let completed = capture.finish(browser);
                    let (_joined, retired) = registry
                        .lock()
                        .unwrap()
                        .retain_upload(&completed)
                        .map_err(|error| error.code)?;
                    drop(retired);
                    Ok::<_, &'static str>(())
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            results.iter().filter(|result| result.is_ok()).count(),
            QA_TRACE_MAX_UPLOADS_PER_WINDOW
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err("qa_trace_rate_limited"))
                .count(),
            4
        );
        let registry = registry.lock().unwrap();
        assert_eq!(registry.sessions[&context.bundle_id].uploads_in_window, 4);
        assert_eq!(
            registry.sessions[&context.bundle_id].retained_upload_serial,
            4
        );
    }

    #[test]
    fn event_fanout_shares_immutable_json_and_preserves_exact_ring_byte_caps() {
        let (mut registry, context) = registry_and_context();
        let other = registry.register(PlayerSessionIdentity::new(10, PlayerSessionId::new(12)), 13);
        registry.record_for_all(json!({"type":"large", "payload":"x".repeat(100_000)}));
        assert!(Arc::ptr_eq(
            &registry.sessions[&context.bundle_id]
                .server_ring
                .events
                .back()
                .unwrap()
                .value,
            &registry.sessions[&other.bundle_id]
                .server_ring
                .events
                .back()
                .unwrap()
                .value,
        ));
        registry.record_for_all(json!({"type":"next", "payload":"x".repeat(100_000)}));
        registry.record_for_all(
            json!({"type":"oversized", "payload":"x".repeat(QA_TRACE_MAX_SERVER_BYTES)}),
        );
        for session in registry.sessions.values() {
            let ring = &session.server_ring;
            assert_eq!(
                ring.events
                    .iter()
                    .map(|event| event.encoded_bytes)
                    .sum::<usize>(),
                ring.retained_bytes
            );
            assert!(ring.retained_bytes <= QA_TRACE_MAX_SERVER_BYTES);
            assert_eq!(ring.oversized_events, 1);
            assert_eq!(ring.events.len(), 1);
            assert_eq!(ring.events.back().unwrap().value["type"], "next");
        }
    }
}
