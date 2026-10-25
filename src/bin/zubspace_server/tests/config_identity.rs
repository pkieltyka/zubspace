use super::*;

#[test]
fn player_name_policy_matches_shared_unicode_and_ascii_vectors() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/player_names.json"
    )))
    .unwrap();
    for row in vectors.as_array().unwrap() {
        let raw = row["raw"].as_str().unwrap();
        let expected = row["valid"]
            .as_bool()
            .unwrap()
            .then(|| row["normalized"].as_str().unwrap().to_owned());
        assert_eq!(normalized_login_name(raw), expected, "raw name {raw:?}");
    }
}

#[test]
fn invalid_name_admission_is_atomic_and_normalized_names_cannot_evade_duplicates() {
    use crate::identity::{LoginAdmissionError, admit_player_login};
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    assert_eq!(
        admit_player_login(
            &state,
            alpha,
            "123456789012".into(),
            "\u{200b}\t Ace!*\u{a0}".into()
        ),
        Ok(())
    );
    let (tx, _rx) = mpsc::channel();
    let beta = register_connection(&state, tx);
    for name in [
        "bad name",
        "bad_name",
        "x$y",
        "x.y",
        "x(y)",
        "",
        "\t\u{200b}",
        "abcdefghijklmnop",
    ] {
        assert_eq!(
            admit_player_login(&state, beta, "123456789013".into(), name.into()),
            Err(LoginAdmissionError::InvalidName)
        );
        let guard = state.lock().unwrap();
        assert!(!guard.clients[&beta].arena_admission_reserved);
        assert!(guard.clients[&beta].player_identity.is_none());
        assert_eq!(guard.clients[&alpha].player.name, "Ace!*");
    }
    assert_eq!(
        admit_player_login(
            &state,
            beta,
            "123456789013".into(),
            "\u{feff}aCE!*\u{2060}".into()
        ),
        Err(LoginAdmissionError::NameInUse)
    );
    assert!(
        state.lock().unwrap().clients[&beta]
            .player_identity
            .is_none()
    );
}

#[test]
fn server_extracts_continuum_login_name() {
    let mut payload = vec![0; 165];
    payload[0] = PASSWORD_LOGIN_CONTINUUM;
    payload[2..7].copy_from_slice(b"pilot");
    set_login_identity(&mut payload, 123_456_789_012);
    assert_eq!(login_name(&payload), Some("pilot".to_string()));
    assert_eq!(
        login_player_identity(&payload),
        Some("123456789012".to_string())
    );
    assert_eq!(normalized_login_name("pilot"), Some("pilot".to_string()));
    assert_eq!(normalized_login_name("bad_name"), None);
}

#[test]
fn server_config_parses_flags_and_rejects_unknown_zones() {
    assert_eq!(ServerConfig::default().max_players, 500);
    // [REVIEW H10] The compiled fallback matches the shipped TOML (60 Hz),
    // pinned against the file itself by
    // `shipped_config_matches_compiled_defaults` below.
    assert_eq!(ServerConfig::default().tick_rate_hz, 60);
    assert!(ServerConfig::default().movement_snapback);
    assert!(
        !ServerConfig::default().enable_objectives,
        "flags and powerballs are deferred, so objectives default off"
    );
    assert_eq!(
        ServerConfig::default().observer_network_profile,
        crate::combat_timeline::ObserverNetworkProfile::AutomaticWan
    );

    let snapback_disabled = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--no-movement-snapback".to_string(),
    ])
    .expect("the default correction can be disabled explicitly");
    assert!(!snapback_disabled.movement_snapback);

    let config = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--bind".to_string(),
        "127.0.0.1:7777".to_string(),
        "--zone".to_string(),
        config::DEFAULT_SERVER_ZONE.to_string(),
        "--max-players".to_string(),
        "12".to_string(),
        "--bot-count".to_string(),
        "3".to_string(),
        "--tick-rate".to_string(),
        "40".to_string(),
        "--log-level".to_string(),
        "debug".to_string(),
        "--log-file".to_string(),
        "target/tmp/zubspace-server-test.log".to_string(),
        "--sim-observe".to_string(),
        "--enable-objectives".to_string(),
        "--observer-network-profile".to_string(),
        "supported-slow-wan".to_string(),
        "--movement-snapback".to_string(),
        "--tx-loss-pct".to_string(),
        "20".to_string(),
        "--tx-loss-seed".to_string(),
        "141421".to_string(),
    ])
    .expect("config should parse");

    assert_eq!(config.listen_addr, "127.0.0.1:7777");
    assert_eq!(config.default_zone, config::DEFAULT_SERVER_ZONE);
    assert_eq!(config.max_players, 12);
    assert_eq!(config.bot_count, 3);
    assert_eq!(config.tick_rate_hz, 40);
    assert_eq!(config.log_level, ServerLogLevel::Debug);
    assert!(config.sim_observe);
    assert!(config.enable_objectives);
    assert_eq!(
        config.observer_network_profile,
        crate::combat_timeline::ObserverNetworkProfile::SupportedSlowWan
    );
    assert!(config.movement_snapback);
    assert_eq!(config.simulated_tx_loss.probability_millionths(), 200_000);
    assert_eq!(config.simulated_tx_loss.configured_seed(), Some(141_421));
    assert_eq!(
        config.log_file.as_deref(),
        Some("target/tmp/zubspace-server-test.log")
    );

    let extreme = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--zone".to_string(),
        "extreme".to_string(),
    ])
    .expect("bundled extreme zone should parse");
    assert_eq!(extreme.default_zone, "extreme");
    let unknown = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--zone".to_string(),
        "unknown-zone".to_string(),
    ])
    .expect("zone identity validation is deferred to the installed catalog");
    assert!(
        unknown
            .validate_zone_ids(["chaos-zone", "extreme"])
            .is_err()
    );
    assert!(
        ServerConfig::from_env_args([
            "zubspace_server".to_string(),
            "--max-players".to_string(),
            "0".to_string(),
        ])
        .is_err()
    );
    let invalid_profile = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--observer-network-profile".to_string(),
        "slow".to_string(),
    ])
    .expect_err("browser profile aliases are not trusted server policy names");
    assert!(
        invalid_profile.contains("expected observer network profile"),
        "got: {invalid_profile}"
    );
}

#[test]
fn server_config_requires_explicit_validated_disk_override_policy() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/zones");
    let root = root.display().to_string();
    let config = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--zone-root".to_string(),
        root.clone(),
        "--allow-zone-overrides".to_string(),
        "--zone".to_string(),
        "extreme".to_string(),
    ])
    .expect("config should parse before the single catalog scan");
    let catalog = crate::zone::load_server_zone_catalog(
        config.zone_bundle_root.as_deref(),
        config.allow_zone_overrides,
    )
    .expect("authenticated checked-in disk bundles should validate");
    config
        .validate_zone_ids(catalog.zone_ids())
        .expect("selected disk zone is in exact catalog");
    assert_eq!(config.zone_bundle_root.as_deref(), Some(root.as_str()));
    assert!(config.allow_zone_overrides);

    let denied = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--zone-root".to_string(),
        root.clone(),
    ])
    .expect("config should parse before catalog construction");
    assert!(
        crate::zone::load_server_zone_catalog(denied.zone_bundle_root.as_deref(), false).is_err(),
        "a manifest override alone cannot replace embedded identities"
    );
    let missing = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--zone-root".to_string(),
        "/definitely/missing/zubspace-zone-root".to_string(),
    ])
    .expect("config should parse before catalog construction");
    assert!(
        crate::zone::load_server_zone_catalog(missing.zone_bundle_root.as_deref(), false).is_err(),
        "an explicitly configured missing root must not silently fall back"
    );
}

#[test]
fn max_players_is_bounded_by_the_unique_client_id_space() {
    let boundary = limits::MAX_PLAYER_CLIENTS;
    let config = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--max-players".to_string(),
        boundary.to_string(),
    ])
    .expect("the complete unique client-id space should be configurable");
    assert_eq!(config.max_players, boundary);

    let error = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--max-players".to_string(),
        (boundary + 1).to_string(),
    ])
    .expect_err("capacity beyond the unique client-id space must fail startup");
    assert!(error.contains(&boundary.to_string()), "got: {error}");
    assert!(error.contains("unique player client IDs"), "got: {error}");
}

#[test]
fn rtc_port_capacity_and_ice_configuration_validate_at_startup() {
    let error = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--max-players".to_string(),
        "3".to_string(),
        "--rtc-udp-port-range".to_string(),
        "19000-19001".to_string(),
    ])
    .expect_err("two RTC ports cannot admit three peers");
    assert!(error.contains("provides 2"), "got: {error}");
    assert!(error.contains("max_players is 3"), "got: {error}");

    let config = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--max-players".to_string(),
        "2".to_string(),
        "--rtc-udp-port-range".to_string(),
        "19000-19001".to_string(),
        "--rtc-ice-urls".to_string(),
        "stun:stun.example.test:3478,turn:turn.example.test:3478?transport=udp".to_string(),
        "--rtc-ice-username".to_string(),
        "pilot".to_string(),
        "--rtc-ice-credential".to_string(),
        "secret".to_string(),
        "--rtc-ice-policy".to_string(),
        "relay".to_string(),
        "--signaling-allowed-origins".to_string(),
        "https://play.example.test".to_string(),
        "--signaling-auth-token".to_string(),
        "admit-me".to_string(),
        "--signaling-require-https".to_string(),
        "--signaling-trust-proxy-headers".to_string(),
    ])
    .expect("capacity-matched RTC configuration should parse");
    let range = config
        .rtc_udp_port_range
        .expect("range should be configured");
    assert_eq!(range.capacity(), 2);
    assert!(config.rtc_external_ips.is_empty());
    assert_eq!(config.rtc_ice_transport_policy.as_str(), "relay");
    assert_eq!(config.signaling_auth_token.as_deref(), Some("admit-me"));
    assert!(config.signaling_require_https);
    assert!(config.signaling_trust_proxy_headers);
}

#[test]
fn rtc_udp_bind_ip_requires_canonical_assigned_ipv4() {
    let config = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--rtc-udp-bind-ip".to_string(),
        "127.0.0.1".to_string(),
    ])
    .expect("the assigned loopback IPv4 address should be accepted");
    assert_eq!(config.rtc_udp_bind_ip, Some("127.0.0.1".parse().unwrap()));
    assert_eq!(config.rtc_udp_bind_address(0).to_string(), "127.0.0.1:0");

    for rejected in [
        "0.0.0.0",
        "::1",
        "100.072.32.72",
        " 127.0.0.1",
        "127.0.0.1 ",
        "224.0.0.1",
        "255.255.255.255",
    ] {
        let error = ServerConfig::from_env_args([
            "zubspace_server".to_string(),
            "--rtc-udp-bind-ip".to_string(),
            rejected.to_string(),
        ])
        .expect_err("unsafe or noncanonical RTC bind address must fail closed");
        assert!(
            error.contains("RTC UDP bind IP"),
            "unexpected error for {rejected:?}: {error}"
        );
    }

    let error = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--rtc-udp-bind-ip".to_string(),
        "192.0.2.1".to_string(),
    ])
    .expect_err("a syntactically valid but nonlocal address must fail closed");
    assert!(error.contains("not assigned to a local IPv4 interface"));

    assert_eq!(
        config::parse_optional_rtc_udp_bind_ip("100.72.32.72")
            .expect("a Tailscale-style IPv4 literal should parse independently of this host"),
        Some("100.72.32.72".parse().unwrap())
    );
}

#[test]
fn rtc_udp_path_mtu_caps_sctp_packets_without_reducing_message_capacity() {
    let defaults = ServerConfig::default();
    assert_eq!(defaults.rtc_udp_path_mtu, None);
    assert_eq!(defaults.rtc_sctp_max_packet_size(), None);

    let tailscale = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--rtc-udp-path-mtu".to_string(),
        "1280".to_string(),
    ])
    .expect("Tailscale's common IPv4 MTU should be accepted");
    assert_eq!(tailscale.rtc_udp_path_mtu, Some(1280));
    assert_eq!(tailscale.rtc_sctp_max_packet_size(), Some(1200));

    // DTLS 1.2 AES-GCM adds 13 record-header + 8 explicit-nonce + 16 tag
    // bytes. IPv4 and UDP add another 28. The explicit reserve retains 15
    // bytes beyond that exact 65-byte outer overhead.
    const DTLS_AES_GCM_OVERHEAD: u32 = 13 + 8 + 16;
    const IPV4_UDP_OVERHEAD: u32 = 20 + 8;
    let outer_datagram =
        tailscale.rtc_sctp_max_packet_size().unwrap() + DTLS_AES_GCM_OVERHEAD + IPV4_UDP_OVERHEAD;
    assert_eq!(outer_datagram, 1265);
    assert!(outer_datagram <= u32::from(tailscale.rtc_udp_path_mtu.unwrap()));

    let ordinary = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--rtc-udp-path-mtu".to_string(),
        "1500".to_string(),
    ])
    .expect("standard Ethernet MTU should be accepted");
    assert_eq!(ordinary.rtc_sctp_max_packet_size(), Some(1228));

    for rejected in ["1279", "01280", " 1280", "1280 ", "-1", "65536"] {
        let error = ServerConfig::from_env_args([
            "zubspace_server".to_string(),
            "--rtc-udp-path-mtu".to_string(),
            rejected.to_string(),
        ])
        .expect_err("unsafe or noncanonical path MTU must fail closed");
        assert!(
            error.contains("RTC UDP path MTU"),
            "unexpected error for {rejected:?}: {error}"
        );
    }
}

#[test]
fn rtc_external_candidate_mapping_is_rejected_until_the_runtime_supports_it() {
    let error = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--rtc-external-ips".to_string(),
        "192.0.2.10".to_string(),
    ])
    .expect_err("silently ignored external candidate mapping must be rejected");
    assert!(error.contains("unsupported"), "got: {error}");
    assert!(error.contains("does not apply"), "got: {error}");
}

#[test]
fn rtc_turn_configuration_rejects_missing_credentials() {
    let error = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--rtc-ice-urls".to_string(),
        "turn:turn.example.test:3478?transport=udp".to_string(),
    ])
    .expect_err("TURN without credentials must be rejected");
    assert!(error.contains("require both"), "got: {error}");
}

#[test]
fn rtc_ice_configuration_rejects_noncanonical_runtime_inputs() {
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rtc_ice_urls.json"
    )))
    .expect("shared ICE URL fixtures should be valid JSON");
    for url in fixtures["rejected"]
        .as_array()
        .expect("rejected ICE fixture array")
        .iter()
        .map(|value| value.as_str().expect("ICE fixture should be a string"))
    {
        let error = ServerConfig::from_env_args([
            "zubspace_server".to_string(),
            "--rtc-ice-urls".to_string(),
            url.to_string(),
            "--rtc-ice-username".to_string(),
            "pilot".to_string(),
            "--rtc-ice-credential".to_string(),
            "secret".to_string(),
        ])
        .expect_err("noncanonical ICE URL must be rejected");
        assert!(error.contains("canonical"), "{url}: {error}");
    }
}

#[test]
fn rtc_ice_configuration_accepts_shared_canonical_host_fixtures() {
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rtc_ice_urls.json"
    )))
    .expect("shared ICE URL fixtures should be valid JSON");
    for url in fixtures["accepted"]
        .as_array()
        .expect("accepted ICE fixture array")
        .iter()
        .map(|value| value.as_str().expect("ICE fixture should be a string"))
    {
        ServerConfig::from_env_args([
            "zubspace_server".to_string(),
            "--rtc-ice-urls".to_string(),
            url.to_string(),
            "--rtc-ice-username".to_string(),
            "pilot".to_string(),
            "--rtc-ice-credential".to_string(),
            "secret".to_string(),
        ])
        .unwrap_or_else(|error| panic!("canonical ICE fixture {url:?} was rejected: {error}"));
    }
}

#[test]
fn signaling_allowed_origins_require_canonical_browser_serialization() {
    for origin in [
        "http://localhost:7000",
        "https://play.example.test",
        "https://[2001:db8::1]:8443",
    ] {
        ServerConfig {
            signaling_allowed_origins: vec![origin.to_string()],
            ..ServerConfig::default()
        }
        .validate()
        .unwrap_or_else(|error| panic!("canonical origin {origin:?} was rejected: {error}"));
    }

    for origin in [
        "HTTPS://play.example.test",
        "https://PLAY.example.test",
        "https://play.example.test:443",
        "https://play.example.test:0",
        "http://localhost:0",
        "https://play.example.test:",
        "https://pilot@play.example.test",
        "https://play.example.test/",
        "https://play.example.test/path",
        "https://play.example.test?query",
        "https://play.example.test#fragment",
        "https://%65xample.com",
        "https://[::::]",
    ] {
        let error = ServerConfig {
            signaling_allowed_origins: vec![origin.to_string()],
            ..ServerConfig::default()
        }
        .validate()
        .expect_err("noncanonical signaling origin must be rejected");
        assert!(
            error.contains("canonical")
                || error.contains("cannot contain")
                || error.contains("cannot use port 0")
                || error.contains("invalid signaling allowed origin"),
            "{origin}: {error}"
        );
    }
}

#[test]
fn https_signaling_requires_trusted_proxy_headers_at_startup() {
    let error = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--signaling-require-https".to_string(),
    ])
    .expect_err("native plaintext signaling cannot verify HTTPS without trusted proxy headers");
    assert!(
        error.contains("signaling_require_https=true"),
        "got: {error}"
    );
    assert!(
        error.contains("signaling_trust_proxy_headers=true"),
        "got: {error}"
    );
    assert!(error.contains("X-Forwarded-Proto"), "got: {error}");

    let config = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--signaling-require-https".to_string(),
        "--signaling-trust-proxy-headers".to_string(),
    ])
    .expect("HTTPS enforcement behind an explicitly trusted proxy should validate");
    assert!(config.signaling_require_https);
    assert!(config.signaling_trust_proxy_headers);
}

#[test]
fn environment_booleans_are_strict() {
    for value in ["true", "TRUE", "1"] {
        assert_eq!(config::parse_env_bool(value), Ok(true));
    }
    for value in ["false", "FALSE", "0"] {
        assert_eq!(config::parse_env_bool(value), Ok(false));
    }
    for value in ["", "yes", "2", "truthy"] {
        assert!(config::parse_env_bool(value).is_err(), "accepted {value:?}");
    }
}

#[test]
fn rtc_ice_configuration_rejects_runtime_unsupported_transports() {
    for url in [
        "stuns:stun.example.test:5349",
        "turns:turn.example.test:5349?transport=udp",
        "turn:turn.example.test:3478?transport=tcp",
    ] {
        let error = ServerConfig::from_env_args([
            "zubspace_server".to_string(),
            "--rtc-ice-urls".to_string(),
            url.to_string(),
            "--rtc-ice-username".to_string(),
            "pilot".to_string(),
            "--rtc-ice-credential".to_string(),
            "secret".to_string(),
        ])
        .expect_err("unsupported ICE URL must be rejected");
        assert!(error.contains("unsupported RTC ICE URL"), "{url}: {error}");
    }
}

#[test]
fn rtc_ice_configuration_rejects_missing_authority_before_runtime_setup() {
    let error = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--rtc-ice-urls".to_string(),
        "stun:".to_string(),
    ])
    .expect_err("malformed ICE URL must be rejected");
    assert!(error.contains("canonical"), "got: {error}");
}

#[test]
fn rtc_udp_preflight_reports_an_occupied_socket() {
    let occupied = std::net::UdpSocket::bind("0.0.0.0:0").expect("test UDP socket should bind");
    let port = occupied.local_addr().expect("socket address").port();
    let config = ServerConfig {
        max_players: 1,
        rtc_udp_port_range: Some(config::RtcUdpPortRange {
            min: port,
            max: port,
        }),
        ..ServerConfig::default()
    };
    let error = validate_rtc_udp_socket_availability(&config)
        .expect_err("occupied configured RTC socket must fail preflight");
    assert!(error.to_string().contains(&format!("RTC UDP port {port}")));
}

#[test]
fn rtc_udp_preflight_uses_the_selected_local_interface() {
    let occupied =
        std::net::UdpSocket::bind("127.0.0.1:0").expect("loopback UDP socket should bind");
    let port = occupied.local_addr().expect("socket address").port();
    let config = ServerConfig {
        max_players: 1,
        rtc_udp_bind_ip: Some("127.0.0.1".parse().unwrap()),
        rtc_udp_port_range: Some(config::RtcUdpPortRange {
            min: port,
            max: port,
        }),
        ..ServerConfig::default()
    };
    let error = validate_rtc_udp_socket_availability(&config)
        .expect_err("preflight must probe the selected RTC interface");
    assert!(error.to_string().contains(&format!("127.0.0.1:{port}")));
}

#[test]
fn server_config_loads_toml_file_before_cli_overrides() {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "zubspace-server-config-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ));
    std::fs::write(
        &path,
        r#"
[server]
bind = "127.0.0.1:7778"
zone = "extreme"
max_players = 40
bot_count = 12
bot_ship_types = "1,2,4,5,7"
tick_rate = 40
log_level = "warn"
log_filter = "rtc=warn"
log_file = "/tmp/zubspace-config-test.log"
sim_observe = true
enable_objectives = true
observer_network_profile = "supported-mild-wan"
movement_snapback = true
tx_loss_pct = 0.25
tx_loss_seed = 77
rtc_udp_bind_ip = "127.0.0.1"
rtc_udp_path_mtu = 1280
realtime_event_game_copy = "on-loss"
send_budget_kib_per_second = 96
rtc_write_coalesce_micros = 500
"#,
    )
    .expect("config file should be writable");

    let config = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--config".to_string(),
        path.display().to_string(),
        "--bot-count".to_string(),
        "0".to_string(),
        "--log-filter".to_string(),
        "rtc=debug".to_string(),
    ])
    .expect("config file should parse");

    let _ = std::fs::remove_file(path);
    assert_eq!(config.listen_addr, "127.0.0.1:7778");
    assert_eq!(config.default_zone, "extreme");
    assert_eq!(config.max_players, 40);
    assert_eq!(config.bot_count, 0);
    assert_eq!(config.bot_ship_types, vec![0, 1, 3, 4, 6]);
    assert_eq!(config.tick_rate_hz, 40);
    assert_eq!(config.log_level, ServerLogLevel::Warn);
    assert_eq!(config.log_filter, "rtc=debug");
    assert!(config.sim_observe);
    assert!(config.enable_objectives);
    assert_eq!(
        config.observer_network_profile,
        crate::combat_timeline::ObserverNetworkProfile::SupportedMildWan
    );
    assert!(config.movement_snapback);
    assert_eq!(config.simulated_tx_loss.probability_millionths(), 250_000);
    assert_eq!(config.simulated_tx_loss.configured_seed(), Some(77));
    assert_eq!(config.rtc_udp_bind_ip, Some("127.0.0.1".parse().unwrap()));
    assert_eq!(config.rtc_udp_path_mtu, Some(1280));
    assert_eq!(
        config.realtime_event_game_copy,
        crate::rtc::RealtimeEventGameCopy::OnLoss
    );
    assert_eq!(config.send_budget_kib_per_second, 96);
    assert_eq!(config.rtc_write_coalesce_micros, 500);
    assert_eq!(config.rtc_sctp_max_packet_size(), Some(1200));
    assert_eq!(
        config.log_file.as_deref(),
        Some("/tmp/zubspace-config-test.log")
    );
}

#[test]
fn server_config_empty_log_file_uses_stdout() {
    let config = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--log-file".to_string(),
        "".to_string(),
    ])
    .expect("empty log file should parse");

    assert_eq!(config.log_file, None);
}

#[test]
fn cloned_configs_build_isolated_wired_loss_controllers() {
    let config = ServerConfig {
        simulated_tx_loss: crate::simulated_loss::SimulatedTxLossSettings::new(
            1_000_000,
            Some(141_422),
        ),
        ..ServerConfig::default()
    };
    let state_a = Arc::new(Mutex::new(ServerState::new(config.clone())));
    let state_b = Arc::new(Mutex::new(ServerState::new(config)));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state_a, tx_a);
    let beta = register_connection(&state_b, tx_b);

    let packet_a = vec![1, 2, 3];
    let packet_b = vec![4, 5, 6, 7];
    send_to(&state_a, alpha, packet_a.clone());
    send_to(&state_b, beta, packet_b.clone());
    assert!(
        rx_a.try_recv().is_err(),
        "state A should realize its configured drop"
    );
    assert!(
        rx_b.try_recv().is_err(),
        "state B should realize its configured drop"
    );

    let loss_a = state_a
        .lock()
        .expect("server state should not be poisoned")
        .simulated_tx_loss
        .clone();
    let loss_b = state_b
        .lock()
        .expect("server state should not be poisoned")
        .simulated_tx_loss
        .clone();
    assert_eq!(loss_a.drain_counts(), (1, packet_a.len() as u64));
    assert_eq!(
        loss_b.drain_counts(),
        (1, packet_b.len() as u64),
        "draining state A must not drain state B's controller"
    );
}

#[test]
fn server_capacity_counts_authenticated_players() {
    let state = Arc::new(Mutex::new(ServerState {
        config: ServerConfig {
            max_players: 1,
            ..ServerConfig::default()
        },
        ..ServerState::default()
    }));
    let (tx_a, _) = mpsc::channel();
    let (tx_b, _) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);

    assert!(!server_at_capacity(&state, beta));
    set_player_identity_and_name(
        &state,
        alpha,
        "100000000001".to_string(),
        "alpha".to_string(),
    );

    assert!(server_at_capacity(&state, beta));
    assert!(!server_at_capacity(&state, alpha));
}

#[test]
fn quality_capacity_reserves_future_holds_atomically_at_login() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    {
        let mut guard = state.lock().unwrap();
        for i in 0..4095 {
            guard
                .quality_holds
                .begin(&format!("held-{i}"), Instant::now())
                .unwrap();
        }
    }
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let a = register_connection(&state, tx_a);
    let b = register_connection(&state, tx_b);
    assert!(set_player_identity_and_name(
        &state,
        a,
        "last-free".into(),
        "last".into()
    ));
    assert!(!set_player_identity_and_name(
        &state,
        b,
        "overflow".into(),
        "overflow".into()
    ));
    assert!(state.lock().unwrap().clients[&b].player_identity.is_none());
    assert!(set_player_identity_and_name(
        &state,
        b,
        "held-0".into(),
        "held".into()
    ));
    let mut guard = state.lock().unwrap();
    assert!(
        guard
            .quality_holds
            .begin("last-free", Instant::now())
            .is_ok()
    );
    assert_eq!(guard.quality_holds.len(), 4096);
}

#[test]
fn server_crc_matches_continuum_file_checksum() {
    assert_eq!(continuum_file_crc32(b""), 0);
    assert_eq!(continuum_file_crc32(b"123456789"), 0xcbf4_3926);
    assert_eq!(continuum_file_crc32(b"abc"), 0x3524_41c2);
}

#[test]
fn local_playtest_config_enables_structured_operator_telemetry() {
    let mut config = ServerConfig::default();
    config
        .apply_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/etc/server/playtest.toml"
        ))
        .expect("local playtest configuration should parse with the real server parser");
    assert!(config.log_json);
    assert_eq!(config.log_file, None);
    assert_eq!(config.log_level, ServerLogLevel::Debug);
    assert_eq!(config.log_filter, "rtc=info,webrtc=info");
    assert_eq!(
        config.metrics_listen,
        Some("127.0.0.1:9100".parse().unwrap())
    );
    assert_eq!(config.listen_addr, "127.0.0.1:9001");
    assert_eq!(config.bot_count, 15);
    assert_eq!(config.bot_ship_types, vec![0, 3, 4, 7]);
    assert!(config.maxed_spawn_loadout);
    assert!(config.free_for_all);
    assert!(config.allow_zone_overrides);
    assert_eq!(config.tick_rate_hz, 60);
    assert!(!config.qa_trace_upload);
    assert!(config.client_debug_trace);
}

#[test]
fn shipped_config_matches_compiled_defaults() {
    // [REVIEW H10] The binary fallback and the checked-in local/Docker
    // config must agree, so a bare `zubspace_server` run behaves like the
    // configured deployments. This parses the real shipped file.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/config/zubspace_server.toml");
    let mut config = ServerConfig::default();
    config
        .apply_file(path)
        .expect("shipped config should parse");
    assert_eq!(
        config.tick_rate_hz,
        ServerConfig::default().tick_rate_hz,
        "config/zubspace_server.toml tick_rate must equal DEFAULT_TICK_RATE_HZ"
    );
    assert_eq!(
        config.default_zone,
        ServerConfig::default().default_zone,
        "shipped default zone must match the compiled default"
    );
    assert_eq!(
        config.movement_snapback,
        ServerConfig::default().movement_snapback,
        "shipped movement snap-back default must match the compiled default"
    );
    assert_eq!(
        config.observer_network_profile,
        ServerConfig::default().observer_network_profile,
        "shipped observer timeline policy must match the compiled default"
    );
    assert_eq!(
        config.realtime_event_game_copy,
        ServerConfig::default().realtime_event_game_copy,
        "shipped realtime event game-copy policy must match the compiled default"
    );
    assert_eq!(
        config.send_budget_kib_per_second,
        ServerConfig::default().send_budget_kib_per_second,
        "shipped send budget must match the compiled default"
    );
}

#[test]
fn send_budget_rejects_values_that_cannot_bound_or_fit_a_frame() {
    for value in ["15", "4097", "lots", "-1"] {
        let mut config = ServerConfig::default();
        let path = std::env::temp_dir().join(format!(
            "zubspace-send-budget-{value}-{}.toml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            format!("[server]\nsend_budget_kib_per_second = {value}\n"),
        )
        .unwrap();
        let result = config.apply_file(path.to_str().unwrap());
        let _ = std::fs::remove_file(path);
        assert!(result.is_err(), "{value} must be rejected");
    }
    let state = crate::state::ServerState {
        config: ServerConfig {
            send_budget_kib_per_second: 96,
            ..ServerConfig::default()
        },
        ..crate::state::ServerState::default()
    };
    let state = std::sync::Arc::new(std::sync::Mutex::new(state));
    let (tx, _rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&state, tx);
    let cap = state.lock().unwrap().clients[&player_id]
        .send_pacing
        .cap_bytes_per_second;
    assert_eq!(cap, 96.0 * 1024.0, "new clients take the configured budget");
}

#[test]
fn duplicate_config_flag_is_an_error() {
    // [REVIEW M17] A second --config used to be silently discarded.
    let err = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--config".to_string(),
        "a.toml".to_string(),
        "--config".to_string(),
        "b.toml".to_string(),
    ])
    .expect_err("duplicate --config must be rejected");
    assert!(err.contains("more than once"), "got: {err}");
}

#[test]
fn tick_rate_is_clamped_to_a_sane_range() {
    // [REVIEW M17] An absurd tick rate is a configuration mistake, not a
    // request for a 15 µs simulation interval.
    let err = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--tick-rate".to_string(),
        "65535".to_string(),
    ])
    .expect_err("absurd tick rate must be rejected");
    assert!(err.contains("between 1 and"), "got: {err}");
    assert!(
        ServerConfig::from_env_args([
            "zubspace_server".to_string(),
            "--tick-rate".to_string(),
            "240".to_string(),
        ])
        .is_ok()
    );
}

#[test]
fn bind_source_tracks_where_the_listen_addr_came_from() {
    // [REVIEW M17] The startup log surfaces the bind origin; verify the
    // default, flag, and positional paths report correctly.
    assert_eq!(
        ServerConfig::default().listen_addr_source,
        config::BindSource::Default
    );
    let flagged = ServerConfig::from_env_args([
        "zubspace_server".to_string(),
        "--bind".to_string(),
        "127.0.0.1:7777".to_string(),
    ])
    .expect("config should parse");
    assert_eq!(flagged.listen_addr_source, config::BindSource::CliFlag);
    let positional =
        ServerConfig::from_env_args(["zubspace_server".to_string(), "127.0.0.1:8888".to_string()])
            .expect("config should parse");
    assert_eq!(positional.listen_addr, "127.0.0.1:8888");
    assert_eq!(
        positional.listen_addr_source,
        config::BindSource::Positional
    );
}

#[test]
fn every_bundled_zone_resolves_a_settings_profile() {
    // [REVIEW M16] Each bundled zone maps to an explicit settings profile
    // (Chaos is the only authored one today); the mapping must stay total
    // over the supported zone list and closed within it.
    for zone in crate::zone::supported_server_zone_ids() {
        let profile = crate::arena_settings::settings_profile_zone_for(zone)
            .expect("every bundled zone must resolve a settings profile");
        assert_eq!(profile, *zone, "each bundled zone maps to itself");
    }
    assert_eq!(
        crate::arena_settings::settings_profile_zone_for(crate::zone::CHAOS_ZONE_ID),
        Some(crate::zone::CHAOS_ZONE_ID),
        "chaos runs its own authored profile"
    );
    assert_eq!(
        crate::arena_settings::settings_profile_zone_for("unknown-zone"),
        None,
        "unknown identities must fail closed"
    );
    assert!(
        crate::arena_settings::arena_settings_packet_for_zone("unknown-zone").is_none(),
        "unknown identities must not borrow the Chaos packet"
    );
    assert_eq!(
        crate::arena_settings::activate_settings_profile("unknown-zone"),
        Err("unknown arena-settings profile")
    );
}

#[test]
fn embedded_zone_assets_match_the_on_disk_tree() {
    // [REVIEW H9] The server's include_bytes! zone table is maintained by
    // hand while the client's manifest is generated by build.rs from the
    // same assets/zones tree. This cross-check fails when an asset is
    // added/renamed on disk without updating the server embed (or vice
    // versa), closing the silent-drift gap between the two mechanisms.
    for (zone_id, map_filename, embedded_lvz) in crate::zone::zone_asset_manifest() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/zones")
            .join(zone_id);
        assert!(
            dir.join(map_filename).is_file(),
            "zone {zone_id}: embedded map {map_filename} is missing from {dir:?}"
        );

        let mut disk_lvz: Vec<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|err| panic!("zone dir {dir:?} should be readable: {err}"))
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().to_string_lossy().to_string();
                name.to_ascii_lowercase().ends_with(".lvz").then_some(name)
            })
            .collect();
        disk_lvz.sort();
        let mut embedded: Vec<String> = embedded_lvz.iter().map(|name| name.to_string()).collect();
        embedded.sort();
        assert_eq!(
            embedded, disk_lvz,
            "zone {zone_id}: embedded lvz set drifted from assets/zones/{zone_id}"
        );
    }
}

#[test]
fn bot_ship_types_accept_menu_numbers_and_reject_invalid_or_duplicate_entries() {
    assert_eq!(config::parse_bot_ship_types("1").unwrap(), vec![0]);
    assert_eq!(
        config::parse_bot_ship_types(" 1, 2,4,5,7 ").unwrap(),
        vec![0, 1, 3, 4, 6]
    );
    assert_eq!(
        config::parse_bot_ship_types("8,7,6,5,4,3,2,1").unwrap(),
        vec![7, 6, 5, 4, 3, 2, 1, 0]
    );
    for invalid in [
        "", " ", "0", "9", "1,", ",1", "1,,2", "1,1", "2,1,2", "-1", "Warbird", "1.5",
    ] {
        assert!(
            config::parse_bot_ship_types(invalid).is_err(),
            "accepted {invalid:?}"
        );
    }
    let config = ServerConfig::from_env_args(
        [
            "zubspace_server",
            "--bot-count",
            "15",
            "--bot-ship-types",
            "1,2,4,5,7",
        ]
        .map(str::to_string),
    )
    .unwrap();
    assert_eq!(config.bot_count, 15);
    assert_eq!(config.bot_ship_types, vec![0, 1, 3, 4, 6]);
    assert!(
        ServerConfig::from_env_args(
            ["zubspace_server", "--bot-ship-types", "1,9"].map(str::to_string)
        )
        .is_err()
    );
}
