use super::*;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn capture(level: ServerLogLevel, filter: &str, json: bool, emit: impl FnOnce()) -> String {
    let output = Capture::default();
    let writer = output.clone();
    let dispatch = server_log_dispatch(
        level,
        filter,
        tracing_subscriber::fmt::writer::BoxMakeWriter::new(move || writer.clone()),
        json,
    )
    .unwrap();
    tracing::dispatcher::with_default(&dispatch, emit);
    let bytes = output.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

#[test]
fn log_filter_keeps_structured_gameplay_debug_and_transport_info_warn_error() {
    let text = capture(ServerLogLevel::Debug, "rtc=info,webrtc=info", true, || {
        tracing::debug!(target: "zubspace_server::server_damage", event = "gameplay-kept", player_id = 7);
        tracing::trace!(target: "zubspace_server::server_damage", "gameplay-trace-hidden");
        tracing::debug!(target: "rtc::peer_connection::handler::dtls", "transport-debug-hidden");
        tracing::debug!(target: "rtc_sctp::association", "sctp-debug-hidden");
        tracing::debug!(target: "rtc_ice::agent", "ice-debug-hidden");
        tracing::trace!(target: "webrtc::peer_connection", "webrtc-trace-hidden");
        tracing::info!(target: "rtc_sctp::association", "transport-info-kept");
        tracing::warn!(target: "rtc_ice::agent", "transport-warning-kept");
        tracing::error!(target: "webrtc::peer_connection", "transport-error-kept");
    });
    assert!(!text.contains("hidden"));
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["fields"]["event"], "gameplay-kept");
    assert_eq!(lines[0]["fields"]["player_id"], 7);
    assert_eq!(lines[0]["level"], "DEBUG");
    assert!(
        lines
            .iter()
            .all(|line| line["timestamp"].as_str().unwrap().ends_with('Z'))
    );
    assert_eq!(lines[1]["level"], "INFO");
    assert_eq!(lines[2]["level"], "WARN");
    assert_eq!(lines[3]["level"], "ERROR");
}

#[test]
fn log_filter_empty_preserves_global_behavior_and_explicit_transport_debug_works() {
    for filter in ["", "rtc=debug,webrtc=debug"] {
        let text = capture(ServerLogLevel::Debug, filter, true, || {
            tracing::debug!(target: "rtc_sctp::association", "full-transport-kept");
        });
        assert!(text.contains("full-transport-kept"));
    }
}

#[test]
fn log_filter_longest_prefix_wins_and_can_enable_a_target_above_fallback() {
    let filter = server_log_filter(
        ServerLogLevel::Off,
        "rtc=info,rtc_sctp=debug,rtc_sctp::association=warn",
    )
    .unwrap();
    assert!(!filter.would_enable("other_crate", &tracing::Level::ERROR));
    assert!(filter.would_enable("rtc_ice::agent", &tracing::Level::INFO));
    assert!(!filter.would_enable("rtc_ice::agent", &tracing::Level::DEBUG));
    assert!(filter.would_enable("rtc_sctp::endpoint", &tracing::Level::DEBUG));
    assert!(!filter.would_enable("rtc_sctp::association", &tracing::Level::DEBUG));
    let text = capture(
        ServerLogLevel::Info,
        "zubspace_server=debug,rtc=off",
        false,
        || {
            tracing::debug!(target: "zubspace_server::connection", "above-fallback-kept");
            tracing::error!(target: "rtc_ice::agent", "explicit-off-hidden");
        },
    );
    assert!(text.contains("above-fallback-kept"));
    assert!(!text.contains("explicit-off-hidden"));
}

fn deployed_log_settings(toml: &str) -> (ServerLogLevel, Targets) {
    let value = |key: &str| {
        toml.lines()
            .find_map(|line| line.trim().strip_prefix(key)?.trim().strip_prefix('='))
            .map(|value| value.trim().trim_matches('"').to_string())
            .unwrap()
    };
    let config = ServerConfig::from_env_args(
        [
            "server".to_string(),
            "--log-level".to_string(),
            value("log_level"),
            "--log-filter".to_string(),
            value("log_filter"),
        ]
        .into_iter(),
    )
    .unwrap();
    let filter = server_log_filter(config.log_level, &config.log_filter).unwrap();
    (config.log_level, filter)
}

#[test]
fn prod_drops_per_event_lines_and_dev_keeps_them() {
    let (_, prod) =
        deployed_log_settings(include_str!("../../../../deploy/prod/zubspace_server.toml"));
    let (_, dev) =
        deployed_log_settings(include_str!("../../../../deploy/dev/zubspace_server.toml"));
    for target in crate::log_targets::PER_EVENT_TARGETS {
        let module = target.rsplit_once("::").unwrap().0;
        assert!(
            !prod.would_enable(target, &tracing::Level::INFO),
            "{target}"
        );
        assert!(prod.would_enable(target, &tracing::Level::WARN), "{target}");
        assert!(prod.would_enable(module, &tracing::Level::INFO), "{module}");
        assert!(dev.would_enable(target, &tracing::Level::DEBUG), "{target}");
    }
}

#[test]
fn log_filter_config_rejects_ambiguous_or_invalid_directives_before_startup() {
    for value in [
        "debug",
        "rtc",
        "rtc=",
        "=debug",
        "rtc=nope",
        "rtc=info,",
        "rtc[span]=debug",
        "rtc*=debug",
        "rtc=debug=info",
    ] {
        let error =
            ServerConfig::from_env_args(["server", "--log-filter", value].map(str::to_string))
                .unwrap_err();
        assert!(
            error.contains("log_filter"),
            "unexpected error for {value:?}: {error}"
        );
    }
    let config = ServerConfig::from_env_args(
        ["server", "--log-filter", " rtc = WARNING , webrtc = Debug "].map(str::to_string),
    )
    .unwrap();
    assert_eq!(config.log_filter, "rtc=warn,webrtc=debug");
}
