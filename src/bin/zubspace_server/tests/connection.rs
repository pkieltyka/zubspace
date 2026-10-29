use super::*;

#[test]
fn arena_login_records_the_declared_protocol_revision() {
    for (payload, revision) in [
        (arena_login_payload(0, ""), 0),
        ([arena_login_payload(0, ""), vec![1]].concat(), 1),
        (vec![ARENA_LOGIN, 0], 0),
    ] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let mut client = connect_and_login(&state, "reviser", test_identity_for_name("reviser"));
        let _password = client.read_reliable_payload();
        client.send_reliable(&state, 1, &payload);
        let locked = state.lock().expect("state");
        let handle = locked.clients.get(&client.player_id).expect("client");
        assert!(handle.joined);
        assert_eq!(handle.protocol_revision, revision);
    }
}

#[test]
fn empty_http_preconnect_is_quiet_but_truncated_requests_still_fail_closed() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    for (label, request, expect_ok) in [
        ("empty preconnect", &b""[..], true),
        ("truncated header", &b"GET /v1/status HTTP/1.1\r\nHost: localhost"[..], false),
        ("truncated body", &b"POST /v1/rtc/offer HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{"[..], false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let shared = Arc::clone(&state);
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_client(stream, shared)
        });
        let mut client = TcpStream::connect(address).unwrap();
        client.write_all(request).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let result = server.join().unwrap();
        assert_eq!(result.is_ok(), expect_ok, "{label}: {result:?}");
        if !expect_ok { assert_eq!(result.unwrap_err().kind(), ErrorKind::UnexpectedEof); }
        assert!(state.lock().unwrap().clients.is_empty());
    }
    let response = http_round_trip(state, "GET /v1/status HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert!(response.starts_with("HTTP/1.1 200"));
}

#[test]
fn server_http_rtc_signaling_probe() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(Mutex::new(ServerState::default()));
    let server_state = Arc::clone(&state);
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_client(stream, server_state).unwrap();
    });

    let mut client = TcpStream::connect(addr).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .write_all(
            b"GET /rtc/signaling HTTP/1.1\r\n\
              Host: 127.0.0.1\r\n\
              \r\n",
        )
        .unwrap();

    let response = read_http_response(&mut client);
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains(r#""transport":"webrtc-datachannel""#));
    assert!(response.contains(r#""offer_endpoint":"POST /v1/rtc/offer""#));
    assert!(response.contains(r#""max_data_channels":3"#));
    assert!(response.contains(r#""data_channel_send_buffer_bytes":262144"#));
    assert!(response.contains(r#""outbound_reliable_queue_packets":4096"#));
    assert!(response.contains(r#""outbound_queue_bytes":4194304"#));
    server.join().unwrap();
}

#[test]
fn server_http_zone_catalog_exposes_runtime_zone_identities() {
    // Catalog construction performs full map/LVZ/render validation and is
    // intentionally outside the timed network round trip, matching production
    // startup before listener bind. This test therefore stays cold-start safe
    // when selected alone rather than depending on another test warming the
    // global embedded catalog.
    let catalog = crate::zone::load_server_zone_catalog(None, false)
        .expect("cold embedded catalog should validate before HTTP startup");
    let catalog_json = crate::connection::server_zone_catalog_json(catalog.maps());
    let state = Arc::new(Mutex::new(ServerState::new_with_zone_catalog(
        ServerConfig::default(),
        catalog_json,
    )));
    let response = http_round_trip(state, "GET /v1/zones HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    let body: serde_json::Value =
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    let chaos = body["zones"]
        .as_array()
        .unwrap()
        .iter()
        .find(|zone| zone["id"] == "chaos-zone")
        .unwrap();
    assert_eq!(chaos["display_name"], "Chaos Zone");
    assert_eq!(chaos["map"], "bdhadar.lvl");
    assert!(response.contains(r#""id":"extreme""#), "{response}");
    assert!(
        response.contains("Access-Control-Allow-Origin: *"),
        "{response}"
    );
}

#[test]
fn server_http_zone_catalog_fails_closed_without_preinstalled_response() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let response = http_round_trip(state, "GET /v1/zones HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(response.contains(r#""error":"zone_catalog_unavailable""#));
}

#[test]
fn server_status_exposes_degraded_rtc_capacity_and_snapshot_ack_observation() {
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        max_players: 2,
        rtc_udp_port_range: Some(config::RtcUdpPortRange {
            min: 19_000,
            max: 19_001,
        }),
        ..ServerConfig::default()
    })));
    install_public_test_catalog(&mut state.lock().unwrap());
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    {
        let mut state = state.lock().unwrap();
        state.rtc_udp_ports_in_use.insert(19_001);
        state.rtc_udp_quarantined_ports.insert(19_000);
        state
            .clients
            .get_mut(&player_id)
            .unwrap()
            .last_acked_snapshot_id = Some(7);
    }

    let process = http_round_trip(
        Arc::clone(&state),
        "GET /v1/status HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
    );
    let zone = http_round_trip(
        state,
        "GET /v1/zones/chaos-zone/status HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
    );
    assert!(zone.starts_with("HTTP/1.1 200 OK"));
    let response = process + &zone;
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(
        response.contains(r#""objective_profile_disabled":true"#),
        "{response}"
    );
    assert!(response.contains(r#""objective_flags":0"#), "{response}");
    assert!(
        response.contains(r#""objective_powerballs":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""reliable_gameplay":{"head_visits":0,"queued":0,"#),
        "a quiet server must publish reliable gameplay residency: {response}"
    );
    assert!(response.contains(r#""rtc_capacity":2"#), "{response}");
    assert!(
        response.contains(r#""rtc_effective_capacity":1"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_udp_ports_in_use":1"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_udp_ports_degraded":1"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_udp_degraded_ports":[19000]"#),
        "{response}"
    );
    assert!(
        response.contains(r#""snapshot_acked_clients":1"#),
        "{response}"
    );
    assert!(
        response.contains(r#""threat_contracts_current":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""threat_contract_capacity_per_recipient":512"#),
        "{response}"
    );
    for field in [
        "threat_contract_capacity_evictions",
        "threat_contract_stale_runtime_evictions",
        "projectile_threat_contracts_current",
        "projectile_threat_contracts_peak",
        "projectile_threat_contract_payload_bytes",
        "projectile_threat_contract_capacity_rejections",
        "projectile_threat_contract_terminal_cleanups",
        "threat_harmful_readiness_deferrals_total",
    ] {
        assert!(response.contains(&format!(r#""{field}":0"#)), "{response}");
    }
    assert!(
        response.contains(&format!(
            r#""projectile_threat_contract_capacity_per_recipient":{}"#,
            crate::threat_presentation::MAX_PROJECTILE_THREATS_PER_RECIPIENT,
        )),
        "{response}"
    );
    assert!(
        response.contains(&format!(
            r#""projectile_threat_contract_payload_capacity_per_recipient":{}"#,
            crate::threat_presentation::MAX_PROJECTILE_PRESENTATION_PAYLOAD_BYTES_PER_RECIPIENT,
        )),
        "{response}"
    );
    assert!(
        response.contains(&format!(
            r#""projectile_threat_contract_payload_capacity_200_players":{}"#,
            crate::threat_presentation::MAX_PROJECTILE_PRESENTATION_PAYLOAD_BYTES_AT_200_PLAYERS,
        )),
        "{response}"
    );
    assert!(
        response.contains(r#""movement_snapbacks_total":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""movement_corrections_awaiting_application":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""movement_corrections_awaiting_receipt_ack":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""repel_queue_packets":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""repel_queue_max_target_depth":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""repel_queue_awaiting_application":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""repel_queue_awaiting_receipt_ack":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""repel_queue_overflows":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""position_input_queue_capacity_rejections":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""realtime_event_queue_capacity_per_client":3072"#),
        "{response}"
    );
    for field in [
        "realtime_event_queue_current",
        "realtime_event_queue_peak",
        "realtime_event_queue_overflows",
        "realtime_event_actions_rejected",
        "realtime_event_actions_deferred",
    ] {
        assert!(response.contains(&format!(r#""{field}":0"#)), "{response}");
    }
    assert!(
        response.contains(r#""projectile_pending_hit_capacity_total":3072"#),
        "{response}"
    );
    assert!(
        response.contains(r#""projectile_pending_hit_capacity_per_owner":128"#),
        "{response}"
    );
    assert!(
        response.contains(r#""projectile_pending_hit_retry_budget_per_tick":256"#),
        "{response}"
    );
    for field in [
        "projectile_pending_hit_current",
        "projectile_pending_hit_retained_bytes",
        "projectile_pending_damage_transactions",
        "projectile_pending_damage_payload_bytes",
    ] {
        assert!(response.contains(&format!(r#""{field}":0"#)), "{response}");
    }
    assert!(
        response.contains(r#""rtc_outbound_queue_packets":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_outbound_queue_bytes":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_outbound_realtime_coalesced":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_outbound_reliable_overflows":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_channel_protocol_violations":0"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_inbound_queue_capacity_packets":4096"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_inbound_packets":{"event":0,"game":0,"reliable":0}"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_inbound_queue_packets":{"event":0,"game":0,"reliable":0}"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_inbound_backpressure":{"event":0,"game":0,"reliable":0}"#),
        "{response}"
    );
    assert!(
        response.contains(r#""rtc_data_channel_send_buffer_bytes":262144"#),
        "{response}"
    );
}

#[test]
fn server_http_rtc_offer_endpoint_rejects_invalid_offer() {
    const WORKERS: usize = 4;
    const ATTEMPTS_PER_WORKER: usize = 8;
    let reserved = (0..WORKERS)
        .map(|_| std::net::UdpSocket::bind("0.0.0.0:0").expect("free UDP socket"))
        .collect::<Vec<_>>();
    let ports = reserved
        .iter()
        .map(|socket| socket.local_addr().expect("UDP address").port())
        .collect::<Vec<_>>();
    drop(reserved);

    let body = r#"{"type":"offer","sdp":"v=0"}"#;
    let request = format!(
        "POST /v1/rtc/offer HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    );

    let workers = ports
        .into_iter()
        .enumerate()
        .map(|(worker, port)| {
            let request = request.clone();
            thread::spawn(move || {
                let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
                    max_players: 1,
                    rtc_udp_port_range: Some(config::RtcUdpPortRange {
                        min: port,
                        max: port,
                    }),
                    ..ServerConfig::default()
                })));
                let mut invalid_rejections = 0;
                for attempt in 1..=ATTEMPTS_PER_WORKER {
                    let response = http_round_trip(Arc::clone(&state), &request);
                    // The contract this probe defends is that an invalid offer is never accepted and never retains
                    // admission. Reaching the offer validator is only one of the documented rejections: this test gives each
                    // worker a single-port UDP range, and when the RTC service cannot open that socket the signalling
                    // taxonomy answers 503 rtc_service_unavailable with Retry-After, which is a correct, retryable rejection
                    // of the same request. Asserting 400 on every attempt made the suite load-sensitive - one failure in
                    // twelve full-suite runs, with "worker 2 attempt 4" and a retained 503 body - so the assertion now names
                    // both rejection shapes, still forbids any acceptance, and requires that the validation path was
                    // actually exercised at least once per worker.
                    let rejected_as_invalid = response.starts_with("HTTP/1.1 400 Bad Request")
                        && response.contains(r#""error":"invalid_webrtc_offer""#)
                        && response.contains("Provide a valid JSON WebRTC offer");
                    let rejected_as_retryable_service = response.starts_with("HTTP/1.1 503 Service Unavailable")
                        && response.contains(r#""error":"rtc_service_unavailable""#)
                        && response.contains("Retry-After");
                    assert!(
                        rejected_as_invalid || rejected_as_retryable_service,
                        "worker {worker} attempt {attempt}: {response}"
                    );
                    if rejected_as_invalid {
                        invalid_rejections += 1;
                    }
                    assert!(
                        !response.starts_with("HTTP/1.1 2"),
                        "worker {worker} attempt {attempt}: invalid offer was accepted: {response}"
                    );
                    assert!(
                        !response.contains(r#""ok":true"#),
                        "worker {worker} attempt {attempt}: invalid offer reported success: {response}"
                    );
                    assert!(
                        !response.contains("Address already in use"),
                        "worker {worker} attempt {attempt}: leaked fixed UDP port {port}"
                    );
                    assert_eq!(
                        state.lock().expect("server state").rtc_sessions,
                        0,
                        "worker {worker} attempt {attempt}: rejected offer retained admission"
                    );
                    let rebound = std::net::UdpSocket::bind(("0.0.0.0", port)).unwrap_or_else(
                        |error| {
                            panic!(
                                "worker {worker} attempt {attempt}: rejected offer retained UDP port {port}: {error}"
                            )
                        },
                    );
                    drop(rebound);
                }
                // A run in which every attempt answered with the retryable service shape would pass without ever reaching
                // the validator, so each worker must land on the validation rejection at least once.
                assert!(
                    invalid_rejections > 0,
                    "worker {worker}: no attempt reached the invalid-offer validator, so the rejection path under test was never exercised"
                );
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("RTC socket-reuse worker should pass");
    }
}

#[test]
fn rtc_offer_capacity_and_degraded_capacity_are_retryable() {
    let exhausted = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        max_players: 1,
        ..ServerConfig::default()
    })));
    exhausted.lock().unwrap().rtc_sessions = 1;
    let response = http_round_trip(
        exhausted,
        &rtc_offer_request(r#"{"type":"offer","sdp":"v=0"}"#),
    );
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(response.contains(r#""error":"rtc_capacity_unavailable""#));
    assert!(response.contains("Retry-After: 1"));
    assert!(!response.contains("configured 1"));

    let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let degraded = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        max_players: 1,
        rtc_udp_port_range: Some(config::RtcUdpPortRange {
            min: port,
            max: port,
        }),
        ..ServerConfig::default()
    })));
    degraded
        .lock()
        .unwrap()
        .rtc_udp_quarantined_ports
        .insert(port);
    let response = http_round_trip(
        degraded,
        &rtc_offer_request(r#"{"type":"offer","sdp":"v=0"}"#),
    );
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(response.contains(r#""error":"rtc_capacity_unavailable""#));
    assert!(response.contains("Retry-After: 1"));
    assert!(!response.contains(&port.to_string()));
}

#[test]
fn rtc_offer_udp_bind_failure_is_retryable_and_does_not_leak_details() {
    let occupied = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let port = occupied.local_addr().unwrap().port();
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        max_players: 1,
        rtc_udp_port_range: Some(config::RtcUdpPortRange {
            min: port,
            max: port,
        }),
        ..ServerConfig::default()
    })));

    let response = http_round_trip(
        Arc::clone(&state),
        &rtc_offer_request(r#"{"type":"offer","sdp":"v=0"}"#),
    );
    assert!(
        response.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    assert!(response.contains(r#""error":"rtc_service_unavailable""#));
    assert!(response.contains("Retry-After: 1"));
    assert!(response.contains("retry after UDP/ICE service recovery"));
    assert!(!response.contains("Address already in use"));
    assert!(!response.contains(&port.to_string()));
    let state = state.lock().unwrap();
    assert_eq!(state.rtc_sessions, 0);
    assert!(state.rtc_udp_quarantined_ports.contains(&port));
    drop(state);
    drop(occupied);
}

#[test]
fn rtc_ice_outages_and_internal_failures_have_safe_http_responses() {
    let service = rtc_error_round_trip(crate::rtc::RtcSignalingError::service_unavailable(
        "ICE timeout at secret.internal.example:3478",
    ));
    assert!(
        service.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{service}"
    );
    assert!(service.contains(r#""error":"rtc_service_unavailable""#));
    assert!(service.contains("Retry-After: 1"));
    assert!(!service.contains("secret.internal.example"));

    let internal = rtc_error_round_trip(crate::rtc::RtcSignalingError::internal(
        "runtime builder secret",
    ));
    assert!(
        internal.starts_with("HTTP/1.1 500 Internal Server Error"),
        "{internal}"
    );
    assert!(internal.contains(r#""error":"internal_server_error""#));
    assert!(!internal.contains("Retry-After"));
    assert!(!internal.contains("runtime builder secret"));
}

#[test]
fn signaling_boundary_enforces_origin_tls_and_admission_token() {
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        signaling_allowed_origins: vec!["https://play.example.test".to_string()],
        signaling_auth_token: Some("admit-me".to_string()),
        signaling_require_https: true,
        signaling_trust_proxy_headers: true,
        ..ServerConfig::default()
    })));
    let body = r#"{"type":"offer","sdp":"v=0"}"#;
    let request = |origin: &str, proto: &str, authorization: &str| {
        format!(
            "POST /v1/rtc/offer HTTP/1.1\r\n\
             Host: signal.example.test\r\n\
             Origin: {origin}\r\n\
             X-Forwarded-Proto: {proto}\r\n\
             X-Real-IP: 192.0.2.44\r\n\
             Authorization: {authorization}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {body}",
            body.len()
        )
    };

    let bad_origin = http_round_trip(
        Arc::clone(&state),
        &request("https://attacker.example", "https", "Bearer admit-me"),
    );
    assert!(bad_origin.starts_with("HTTP/1.1 403 Forbidden"));
    assert!(bad_origin.contains(r#""error":"origin_not_allowed""#));

    let missing_tls = http_round_trip(
        Arc::clone(&state),
        &request("https://play.example.test", "http", "Bearer admit-me"),
    );
    assert!(missing_tls.starts_with("HTTP/1.1 403 Forbidden"));
    assert!(missing_tls.contains(r#""error":"https_required""#));

    let bad_token = http_round_trip(
        Arc::clone(&state),
        &request("https://play.example.test", "https", "Bearer wrong"),
    );
    assert!(bad_token.starts_with("HTTP/1.1 401 Unauthorized"));
    assert!(bad_token.contains(r#""error":"invalid_admission_token""#));

    let admitted = http_round_trip(
        state,
        &request("https://play.example.test", "https", "Bearer admit-me"),
    );
    assert!(admitted.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(admitted.contains("Access-Control-Allow-Origin: https://play.example.test"));
    assert!(admitted.contains(r#""error":"invalid_webrtc_offer""#));
}

#[test]
fn signaling_boundary_caps_offer_rate_and_body_size_before_ice() {
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        signaling_offer_rate_per_minute: 1,
        signaling_max_offer_bytes: 32,
        ..ServerConfig::default()
    })));
    let invalid = "{}";
    let request = format!(
        "POST /v1/rtc/offer HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {invalid}",
        invalid.len()
    );
    let first = http_round_trip(Arc::clone(&state), &request);
    assert!(first.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(first.contains(r#""error":"invalid_webrtc_offer""#));
    let limited = http_round_trip(Arc::clone(&state), &request);
    assert!(limited.starts_with("HTTP/1.1 429 Too Many Requests"));
    assert!(limited.contains(r#""error":"offer_rate_limited""#));

    state
        .lock()
        .expect("state should not be poisoned")
        .signaling_rate_windows
        .clear();
    let oversized = "x".repeat(33);
    let response = http_round_trip(
        state,
        &format!(
            "POST /v1/rtc/offer HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {oversized}",
            oversized.len()
        ),
    );
    assert!(response.starts_with("HTTP/1.1 413 Content Too Large"));
    assert!(response.contains(r#""error":"offer_too_large""#));
}

#[test]
fn signaling_rate_identity_uses_only_trusted_forwarded_addresses() {
    let request = |real_ip: &str, forwarded_for: &str| {
        let invalid = "{}";
        format!(
            "POST /v1/rtc/offer HTTP/1.1\r\n\
             Host: play.example.test\r\n\
             X-Real-IP: {real_ip}\r\n\
             X-Forwarded-For: {forwarded_for}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {invalid}",
            invalid.len()
        )
    };

    let trusted = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        signaling_offer_rate_per_minute: 1,
        signaling_trust_proxy_headers: true,
        ..ServerConfig::default()
    })));
    let first = http_round_trip(
        Arc::clone(&trusted),
        &request("192.0.2.10", "203.0.113.200"),
    );
    assert!(first.starts_with("HTTP/1.1 400 Bad Request"), "{first}");
    let second = http_round_trip(
        Arc::clone(&trusted),
        &request("198.51.100.20", "203.0.113.200"),
    );
    assert!(second.starts_with("HTTP/1.1 400 Bad Request"), "{second}");
    let limited = http_round_trip(
        Arc::clone(&trusted),
        &request("192.0.2.10", "198.51.100.99"),
    );
    assert!(
        limited.starts_with("HTTP/1.1 429 Too Many Requests"),
        "{limited}"
    );
    let trusted = trusted.lock().expect("trusted state");
    assert!(trusted.signaling_rate_windows.contains_key("192.0.2.10"));
    assert!(trusted.signaling_rate_windows.contains_key("198.51.100.20"));
    assert!(!trusted.signaling_rate_windows.contains_key("203.0.113.200"));
    drop(trusted);

    let untrusted = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        signaling_offer_rate_per_minute: 1,
        signaling_trust_proxy_headers: false,
        ..ServerConfig::default()
    })));
    let first = http_round_trip(
        Arc::clone(&untrusted),
        &request("192.0.2.10", "203.0.113.200"),
    );
    assert!(first.starts_with("HTTP/1.1 400 Bad Request"), "{first}");
    let spoofed = http_round_trip(
        Arc::clone(&untrusted),
        &request("198.51.100.20", "198.51.100.99"),
    );
    assert!(
        spoofed.starts_with("HTTP/1.1 429 Too Many Requests"),
        "{spoofed}"
    );
    let untrusted = untrusted.lock().expect("untrusted state");
    assert_eq!(untrusted.signaling_rate_windows.len(), 1);
    assert!(untrusted.signaling_rate_windows.contains_key("127.0.0.1"));
}

#[test]
fn server_http_root_is_not_a_gameplay_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Arc::new(Mutex::new(ServerState::default()));
    let server_state = Arc::clone(&state);
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_client(stream, server_state).unwrap();
    });

    let mut client = TcpStream::connect(addr).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .write_all(
            b"GET / HTTP/1.1\r\n\
              Host: 127.0.0.1\r\n\
              \r\n",
        )
        .unwrap();

    let response = read_http_response(&mut client);
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("Content-Type: text/plain; charset=utf-8\r\n"));
    assert_eq!(
        response.split_once("\r\n\r\n").unwrap().1,
        "Welcome to the Zubspace game server.\n"
    );
    assert!(!response.to_ascii_lowercase().contains("upgrade:"));
    server.join().unwrap();
    assert!(
        state.lock().unwrap().clients.is_empty(),
        "the HTTP welcome must not create a gameplay session"
    );
}

fn read_http_response(client: &mut TcpStream) -> String {
    let mut response = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        match client.read(&mut chunk) {
            Ok(0) => break,
            Ok(len) => response.extend_from_slice(&chunk[..len]),
            Err(err) if err.kind() == ErrorKind::ConnectionReset => break,
            Err(err) => panic!("failed to read HTTP response: {err}"),
        }
    }
    String::from_utf8(response).expect("HTTP response should be UTF-8")
}

fn http_round_trip(state: Arc<Mutex<ServerState>>, request: &str) -> String {
    http_round_trip_with_started_at(state, request, Instant::now())
}

fn http_round_trip_with_started_at(
    state: Arc<Mutex<ServerState>>,
    request: &str,
    started_at: Instant,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        crate::connection::handle_client_with_started_at(stream, state, started_at).unwrap();
    });
    let mut client = TcpStream::connect(addr).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client.write_all(request.as_bytes()).unwrap();
    let response = read_http_response(&mut client);
    server.join().unwrap();
    response
}

#[test]
fn every_http_response_forbids_caching_exactly_once() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    for request in [
        "GET /v1/status HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "GET /v1/players HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "GET /v1/zones HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "OPTIONS /v1/zones HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "POST /v1/rtc/offer HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}",
        "GET /v1/qa/trace/0123456789abcdef HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "GET /v1/missing HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "GET /status HTTP/1.1\r\nHost: localhost\r\n\r\n",
    ] {
        let response = http_round_trip(Arc::clone(&state), request);
        let (head, _) = response.split_once("\r\n\r\n").unwrap();
        let cache_headers: Vec<_> = head
            .lines()
            .filter(|line| line.to_ascii_lowercase().starts_with("cache-control:"))
            .collect();
        assert_eq!(
            cache_headers,
            ["Cache-Control: no-store"],
            "{request:?} -> {head}"
        );
    }
}

#[test]
fn public_arena_routes_are_fresh_cross_origin_read_only_http() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    for path in ["/", "/index.html", "/v1/ping", "/v1/players", "/v1/status"] {
        let response = http_round_trip(
            Arc::clone(&state),
            &format!(
                "GET {path} HTTP/1.1\r\nHost: localhost\r\nOrigin: https://game.example\r\n\r\n"
            ),
        );
        assert!(
            response.starts_with("HTTP/1.1 200 OK"),
            "{path}: {response}"
        );
        assert!(response.contains("Cache-Control: no-store\r\n"));
        assert!(response.contains("Access-Control-Allow-Origin: *\r\n"));
        let (_, body) = response.split_once("\r\n\r\n").unwrap();
        match path {
            "/v1/ping" => assert_eq!(body, "."),
            "/" | "/index.html" => assert_eq!(body, "Welcome to the Zubspace game server.\n"),
            "/v1/players" => assert_eq!(
                serde_json::from_str::<serde_json::Value>(body).unwrap(),
                serde_json::json!({"total_players":0,"players":[]})
            ),
            _ => assert!(serde_json::from_str::<serde_json::Value>(body).is_ok()),
        }
        assert!(
            response.contains(if matches!(path, "/v1/players" | "/v1/status") {
                "Content-Type: application/json\r\n"
            } else {
                "Content-Type: text/plain; charset=utf-8\r\n"
            })
        );
        let preflight = http_round_trip(
            Arc::clone(&state),
            &format!("OPTIONS {path} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
        );
        assert!(preflight.starts_with("HTTP/1.1 204 No Content"));
        assert!(preflight.contains("Access-Control-Allow-Methods: GET, OPTIONS\r\n"));
        let post = http_round_trip(
            Arc::clone(&state),
            &format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n"),
        );
        assert!(
            post.starts_with("HTTP/1.1 405 Method Not Allowed"),
            "public endpoint must not accept writes: {path}"
        );
    }
}

#[test]
fn public_population_matches_roster_and_preserves_process_uptime() {
    let mut initial = ServerState::new(ServerConfig {
        bot_count: 2,
        ..ServerConfig::default()
    });
    initial.initialize_configured_bots();
    let state = Arc::new(Mutex::new(initial));
    let mut receivers = Vec::new();
    let mut ids = Vec::new();
    for index in 0..4 {
        let (tx, rx) = mpsc::channel();
        receivers.push(rx);
        let id = register_connection(&state, tx);
        ids.push(id);
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&id).unwrap();
        client.player.name = if index == 0 {
            "A\"ce\\".to_string()
        } else {
            format!("Pilot{index}")
        };
        client.player_identity = Some(format!("private-{index}"));
        client.joined = index < 3;
        if index == 1 {
            client.alive = false;
        }
        if index == 2 {
            client.player.ship = 8;
            client.alive = false;
        }
    }
    let started = Instant::now() - Duration::from_secs(123);
    let get = |path: &str| {
        let response = http_round_trip_with_started_at(
            Arc::clone(&state),
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
            started,
        );
        serde_json::from_str::<serde_json::Value>(response.split_once("\r\n\r\n").unwrap().1)
            .unwrap()
    };
    let listing = get("/v1/players");
    assert_eq!(
        listing.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["players", "total_players"]
    );
    let roster = listing["players"].as_array().unwrap();
    assert_eq!(
        roster.len(),
        5,
        "joined alive/dead/spectators plus bots; exclude pending transport"
    );
    assert_eq!(listing["total_players"], roster.len());
    assert!(
        roster
            .windows(2)
            .all(|pair| pair[0]["id"].as_u64() < pair[1]["id"].as_u64())
    );
    assert!(
        roster
            .iter()
            .all(|entry| entry.as_object().unwrap().len() == 4)
    );
    assert_eq!(
        roster.iter().filter(|player| player["bot"] == true).count(),
        2
    );
    assert_eq!(
        roster
            .iter()
            .filter(|player| player["bot"] == false)
            .count(),
        3
    );
    assert!(roster.iter().all(|player| player["zone"] == "chaos-zone"));
    assert_eq!(roster[0]["name"], "A\"ce\\");
    assert!(!serde_json::to_string(roster).unwrap().contains("private-"));
    let status = get("/v1/status");
    assert_eq!(status["total_players"], roster.len());
    assert!(status.get("players").is_none());
    assert!(status.get("bots").is_none());
    assert_eq!(status["clients"], 4);
    assert_eq!(status["version"], env!("ZUBSPACE_SERVER_BUILD_COMMIT"));
    assert!(status["version"].as_str().unwrap().len() >= 40);
    assert!(status["uptime"].as_u64().unwrap() >= 123);
    assert!(status.get("uptime_seconds").is_none());
    assert!(status.get("tick_timing").is_none() && status.get("rtc_capacity").is_some());
    {
        let mut state = state.lock().unwrap();
        state.clients.get_mut(&ids[1]).unwrap().joined = false;
        state.bots.clear();
    }
    let updated = get("/v1/status");
    assert_eq!(updated["total_players"], 2);
    let listing = get("/v1/players");
    assert_eq!(listing["total_players"], 2);
    assert_eq!(listing["players"].as_array().unwrap().len(), 2);
    assert!(updated["uptime"].as_u64() >= status["uptime"].as_u64());
}

fn rtc_offer_request(body: &str) -> String {
    format!(
        "POST /v1/rtc/offer HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    )
}

fn rtc_error_round_trip(error: crate::rtc::RtcSignalingError) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        crate::connection::write_rtc_offer_error(&mut stream, &error, "https://play.example.test")
            .unwrap();
    });
    let mut client = TcpStream::connect(addr).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let response = read_http_response(&mut client);
    server.join().unwrap();
    response
}

#[test]
fn server_packet_login_and_join_round_trip() {
    // This historical round trip includes an explicit team-frequency cutover.
    let state = Arc::new(Mutex::new(team_mode_state()));
    let mut client = connect_and_login(&state, "pilot", 123_456_789_012);

    let password = client.read_reliable_payload();
    assert_eq!(password.first().copied(), Some(0x0a));
    assert_eq!(password.get(1).copied(), Some(0x00));

    client.send_reliable(&state, 1, &[ARENA_LOGIN, 0]);
    let bootstrap = client.read_reliable_payload();
    let assigned = client.read_reliable_payload();
    let joined = client.read_reliable_payload();
    let entering = client.read_reliable_payload();
    let settings = client.read_reliable_payload();
    let toggle_damage = client.read_reliable_payload();
    let map_info = client.read_reliable_payload();
    let spawn = client.read_reliable_payload();
    let directory = client.read_reliable_payload();
    assert_eq!(assigned, [0x01, 0x00, 0x00]);
    assert_server_clock_bootstrap(&bootstrap, 0);
    assert_eq!(joined, [0x02]);
    assert_eq!(entering.first().copied(), Some(0x03));
    assert_eq!(String::from_utf8_lossy(&entering[3..8]), "pilot");
    assert_arena_settings(&settings);
    assert_eq!(toggle_damage, [TOGGLE_SEND_DAMAGE, 0]);
    assert_map_information(&map_info);
    assert_set_coordinates_for_map(&spawn, zone_map());
    assert_arena_directory(&directory, 1);
    assert_initial_flag_packets(&client);

    client.send_reliable(&state, 2, &[FILE_REQUEST, 0, 0]);
    let map_download = client.read_file_transfer(&state);
    assert_eq!(decompress_map_payload(&map_download), SERVER_MAP_BYTES);
    assert_security_challenge(&client.read_reliable_payload());

    client.send_packet(
        &state,
        security_response_packet(
            continuum_settings_checksum(SECURITY_CHECKSUM_KEY, arena_settings_packet()),
            zone_map().map_checksum,
            Some(zone_map().continuum_crc32),
        ),
    );

    client.send_reliable(&state, 3, &[FILE_REQUEST, 1, 0]);
    let lvz_download = client.read_file_transfer(&state);
    assert_eq!(download_payload(&lvz_download), SERVER_LVZ_FILES[0].1);

    client.send_reliable(&state, 4, &[NEWS_REQUEST]);
    let news = client.read_reliable_payload();
    assert_eq!(
        news,
        server_file_transfer(SERVER_NEWS_FILENAME, SERVER_NEWS_BYTES)
    );

    client.send_reliable(
        &state,
        5,
        &[CHAT_MESSAGE_REQUEST, 2, 0, 0, 0, b'h', b'i', 0],
    );
    let chat = client.read_reliable_payload();
    assert_eq!(chat, [CHAT_PACKET, 2, 0, 0, 0, b'h', b'i', 0]);

    client.send_reliable(&state, 6, &ship_change_request(4));
    crate::server_tick::run_server_tick(&state, Instant::now());
    let ship_change = client.read_reliable_payload();
    assert_eq!(&ship_change[..6], &[SHIP_FREQUENCY_CHANGE, 4, 0, 0, 0, 0]);
    assert_eq!(
        u32::from_le_bytes(ship_change[6..10].try_into().unwrap()),
        state.lock().expect("state").clients[&client.player_id].life_seq
    );

    client.send_reliable(
        &state,
        7,
        &frequency_change_request(&state, client.player_id, 33),
    );
    crate::server_tick::run_server_tick(&state, Instant::now());
    let cutover = client.read_reliable_payload();
    assert_eq!(cutover[0], crate::roster::FREQUENCY_CUTOVER_ACK);
    let frequency_change = client.read_reliable_payload();
    assert_eq!(
        &frequency_change[..6],
        &[SHIP_FREQUENCY_CHANGE, 4, 0, 0, 33, 0]
    );
    assert_eq!(
        u32::from_le_bytes(frequency_change[6..10].try_into().unwrap()),
        state.lock().expect("state").clients[&client.player_id].life_seq
    );

    client.disconnect(&state);
}

#[test]
fn server_reliable_duplicate_is_acked_without_reprocessing_payload() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let mut joined = false;

    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(0, &[NEWS_REQUEST]),
        "test-peer",
        &mut joined,
    );
    let first_ack = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("expected reliable ack");
    assert!(is_reliable_ack(&first_ack));
    let first_news = read_reliable_payload_from_channel(&rx);
    assert_eq!(first_news.first().copied(), Some(SERVER_FILE_TRANSFER));

    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(0, &[NEWS_REQUEST]),
        "test-peer",
        &mut joined,
    );
    let duplicate_ack = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("expected duplicate reliable ack");
    assert!(is_reliable_ack(&duplicate_ack));
    assert!(
        rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "duplicate reliable payload should not send a second news response"
    );
}

#[test]
fn server_reliable_reassembles_chunked_request_before_dispatch() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let mut joined = false;

    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(0, &[CORE_BYTE, SMALL_CHUNK_BODY, NEWS_REQUEST]),
        "test-peer",
        &mut joined,
    );
    let first_ack = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("expected first reliable ack");
    assert!(is_reliable_ack(&first_ack));
    assert!(
        rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "partial small chunk should not dispatch a logical request"
    );

    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable(1, &[CORE_BYTE, SMALL_CHUNK_TAIL, 0xff]),
        "test-peer",
        &mut joined,
    );
    let second_ack = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("expected second reliable ack");
    assert!(is_reliable_ack(&second_ack));
    let news = read_reliable_payload_from_channel(&rx);
    assert_eq!(news.first().copied(), Some(SERVER_FILE_TRANSFER));
}

#[test]
fn server_reliable_ack_removes_pending_server_frame() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let mut joined = false;

    send_reliable_to(&state, player_id, vec![NEWS_REQUEST]);
    assert_eq!(
        state
            .lock()
            .expect("state")
            .clients
            .get(&player_id)
            .expect("client")
            .outgoing_reliable
            .pending_len(),
        1
    );

    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable_ack(0),
        "test-peer",
        &mut joined,
    );

    assert_eq!(
        state
            .lock()
            .expect("state")
            .clients
            .get(&player_id)
            .expect("client")
            .outgoing_reliable
            .pending_len(),
        0
    );
}

#[test]
fn server_reliable_clustered_acks_remove_pending_server_frames() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let mut joined = false;

    send_reliable_to(&state, player_id, vec![NEWS_REQUEST]);
    send_reliable_to(&state, player_id, vec![NEWS_REQUEST]);
    let ack_0 = reliable_ack(0);
    let ack_1 = reliable_ack(1);
    let mut cluster = vec![
        CORE_BYTE,
        crate::reliable::PACKET_CLUSTER,
        ack_0.len() as u8,
    ];
    cluster.extend_from_slice(&ack_0);
    cluster.push(ack_1.len() as u8);
    cluster.extend_from_slice(&ack_1);

    crate::connection::handle_client_packet(&state, player_id, cluster, "test-peer", &mut joined);

    assert_eq!(
        state
            .lock()
            .expect("state")
            .clients
            .get(&player_id)
            .expect("client")
            .outgoing_reliable
            .pending_len(),
        0
    );
}

#[test]
fn server_reliable_resend_helper_reemits_unacked_and_stops_after_ack() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let mut joined = false;

    send_reliable_to(&state, player_id, vec![NEWS_REQUEST]);
    let original = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("expected original reliable frame");
    assert_eq!(
        reliable_payload(&original).and_then(|payload| payload.first().copied()),
        Some(NEWS_REQUEST)
    );

    assert_eq!(
        crate::reliable::resend_due_reliable_to_clients(
            &state,
            Instant::now() + Duration::from_secs(4),
        ),
        1
    );
    let resent = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("expected resent reliable frame");
    assert_eq!(resent, original);

    crate::connection::handle_client_packet(
        &state,
        player_id,
        reliable_ack(0),
        "test-peer",
        &mut joined,
    );
    assert_eq!(
        crate::reliable::resend_due_reliable_to_clients(
            &state,
            Instant::now() + Duration::from_secs(8),
        ),
        0
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "acked reliable frame should not be resent"
    );
}

#[test]
fn server_packet_cluster_dispatches_reliable_entries() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let mut joined = false;
    let reliable_news = reliable(0, &[NEWS_REQUEST]);
    let mut cluster = vec![
        CORE_BYTE,
        crate::reliable::PACKET_CLUSTER,
        reliable_news.len() as u8,
    ];
    cluster.extend_from_slice(&reliable_news);

    crate::connection::handle_client_packet(&state, player_id, cluster, "test-peer", &mut joined);

    let ack = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("expected reliable ack for clustered request");
    assert!(is_reliable_ack(&ack));
    let news = read_reliable_payload_from_channel(&rx);
    assert_eq!(news.first().copied(), Some(SERVER_FILE_TRANSFER));
}

#[test]
fn server_packet_cluster_preserves_reliable_ordering() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let mut joined = false;
    let reliable_one = reliable(1, &[NEWS_REQUEST]);
    let reliable_zero = reliable(0, &[NEWS_REQUEST]);
    let mut cluster = vec![
        CORE_BYTE,
        crate::reliable::PACKET_CLUSTER,
        reliable_one.len() as u8,
    ];
    cluster.extend_from_slice(&reliable_one);
    cluster.push(reliable_zero.len() as u8);
    cluster.extend_from_slice(&reliable_zero);

    crate::connection::handle_client_packet(&state, player_id, cluster, "test-peer", &mut joined);

    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        reliable_ack(0),
        "the gap suffix is not ACK-eligible until id 0 terminals"
    );
    let first_news = read_reliable_payload_from_channel(&rx);
    assert_eq!(first_news.first().copied(), Some(SERVER_FILE_TRANSFER));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        reliable_ack(1)
    );
    let second_news = read_reliable_payload_from_channel(&rx);
    assert_eq!(second_news.first().copied(), Some(SERVER_FILE_TRANSFER));
}

#[test]
fn server_packet_cluster_reassembles_reliable_small_chunks() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let mut joined = false;
    let body = reliable(0, &[CORE_BYTE, SMALL_CHUNK_BODY, NEWS_REQUEST]);
    let tail = reliable(1, &[CORE_BYTE, SMALL_CHUNK_TAIL, 0xff]);
    let mut cluster = vec![CORE_BYTE, crate::reliable::PACKET_CLUSTER, body.len() as u8];
    cluster.extend_from_slice(&body);
    cluster.push(tail.len() as u8);
    cluster.extend_from_slice(&tail);

    crate::connection::handle_client_packet(&state, player_id, cluster, "test-peer", &mut joined);

    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1))
            .expect("expected body ack"),
        reliable_ack(0)
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1))
            .expect("expected tail ack"),
        reliable_ack(1)
    );
    let news = read_reliable_payload_from_channel(&rx);
    assert_eq!(news.first().copied(), Some(SERVER_FILE_TRANSFER));
}

#[test]
fn server_packet_cluster_processes_nested_clusters() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let mut joined = false;
    let reliable_news = reliable(0, &[NEWS_REQUEST]);
    let mut inner_cluster = vec![
        CORE_BYTE,
        crate::reliable::PACKET_CLUSTER,
        reliable_news.len() as u8,
    ];
    inner_cluster.extend_from_slice(&reliable_news);
    let mut outer_cluster = vec![
        CORE_BYTE,
        crate::reliable::PACKET_CLUSTER,
        inner_cluster.len() as u8,
    ];
    outer_cluster.extend_from_slice(&inner_cluster);

    crate::connection::handle_client_packet(
        &state,
        player_id,
        outer_cluster,
        "test-peer",
        &mut joined,
    );

    assert_eq!(
        rx.recv_timeout(Duration::from_secs(1))
            .expect("expected nested reliable ack"),
        reliable_ack(0)
    );
    let news = read_reliable_payload_from_channel(&rx);
    assert_eq!(news.first().copied(), Some(SERVER_FILE_TRANSFER));
}

#[test]
fn server_packet_harness_serves_selected_non_chaos_zone_assets() {
    let extreme_map = zone_map_for_id("extreme").expect("extreme should be bundled");
    // Built through ServerState::new so the zone boots exactly the way a real
    // server boot does, including the empty objective replay of its disabled
    // profile.
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        default_zone: "extreme".to_string(),
        ..ServerConfig::default()
    })));

    let mut client = connect_login_and_join(&state, "extreme", 0);
    assert_server_clock_bootstrap(&client.read_reliable_payload(), 0);
    assert_eq!(client.read_reliable_payload(), [0x01, 0x00, 0x00]);
    assert_eq!(client.read_reliable_payload(), [0x02]);
    assert_eq!(client.read_reliable_payload()[0], 0x03);
    assert_arena_settings_for_zone(&client.read_reliable_payload(), "extreme");
    assert_eq!(client.read_reliable_payload(), [TOGGLE_SEND_DAMAGE, 0]);
    assert_map_information_for_map(&client.read_reliable_payload(), extreme_map);
    assert_set_coordinates_for_map(&client.read_reliable_payload(), extreme_map);
    assert_arena_directory_name(&client.read_reliable_payload(), "Extreme", 1);
    // The join bootstrap replays objective facts after the directory. This
    // zone authors no objective profile, so any flag packet reaching the client
    // here means the disabled profile silently came alive.
    let remaining_join_payloads = client
        .rx
        .try_iter()
        .filter_map(|packet| reliable_parts(&packet).map(|(_, payload)| payload.to_vec()))
        .collect::<Vec<_>>();
    assert!(
        remaining_join_payloads.iter().all(|payload| {
            payload.first() != Some(&crate::objectives::FLAG_POSITION)
                && payload.first() != Some(&crate::objective_protocol::ObjectiveStateUpdate::TYPE)
        }),
        "a zone with a disabled objective profile must not replay flag state: {remaining_join_payloads:?}"
    );

    client.send_reliable(&state, 2, &[FILE_REQUEST, 0, 0]);
    let map_download = client.read_file_transfer(&state);
    assert_eq!(
        continuum_file_crc32(&decompress_map_payload(&map_download)),
        extreme_map.checksum
    );
    assert_security_challenge(&client.read_reliable_payload());

    client.send_reliable(&state, 3, &[FILE_REQUEST, 1, 0]);
    let lvz_download = client.read_file_transfer(&state);
    assert_eq!(
        download_payload(&lvz_download),
        extreme_map.lvz_files[0].bytes.as_ref()
    );

    client.send_reliable(&state, 4, &[NEWS_REQUEST]);
    let news = client.read_reliable_payload();
    assert_eq!(
        news,
        server_file_transfer(
            extreme_map.news_filename.as_ref(),
            extreme_map.news_bytes.as_ref(),
        )
    );

    client.disconnect(&state);
}

#[test]
fn server_packet_arena_login_name_selects_bundled_zone_when_empty() {
    let extreme_map = zone_map_for_id("extreme").expect("extreme should be bundled");
    let state = Arc::new(Mutex::new(ServerState::default()));
    let old_prize_spawn_tick = 50_000;
    {
        let mut locked = state.lock().expect("state lock");
        locked.active_hidden_prizes.push(ServerHiddenPrize {
            collectible_id: 1,
            x: 640,
            y: 768,
            prize_id: 15,
            expires_at_tick: crate::continuum::continuum_tick_add(old_prize_spawn_tick, 300),
        });
        locked.next_hidden_prize_spawn_at_tick = Some(old_prize_spawn_tick);
    }

    let mut client = connect_and_login(&state, "extreme", test_identity_for_name("extreme"));
    let password = client.read_reliable_payload();
    assert_eq!(password.first().copied(), Some(0x0a));
    assert_eq!(password.get(1).copied(), Some(0x00));

    client.send_reliable(&state, 1, &arena_login_payload(0, "Extreme"));
    assert_server_clock_bootstrap(&client.read_reliable_payload(), 0);
    assert_eq!(client.read_reliable_payload(), [0x01, 0x00, 0x00]);
    assert_eq!(client.read_reliable_payload(), [0x02]);
    assert_eq!(client.read_reliable_payload()[0], 0x03);
    assert_arena_settings_for_zone(&client.read_reliable_payload(), "extreme");
    assert_eq!(client.read_reliable_payload(), [TOGGLE_SEND_DAMAGE, 0]);
    assert_map_information_for_map(&client.read_reliable_payload(), extreme_map);
    assert_set_coordinates_for_map(&client.read_reliable_payload(), extreme_map);
    assert_arena_directory_name(&client.read_reliable_payload(), "Extreme", 1);

    let remaining_join_payloads = client
        .rx
        .try_iter()
        .filter_map(|packet| reliable_parts(&packet).map(|(_, payload)| payload.to_vec()))
        .collect::<Vec<_>>();
    assert!(
        remaining_join_payloads
            .iter()
            .all(|payload| payload.first() != Some(&crate::hidden_prizes::HIDDEN_PRIZE_SPAWN)),
        "the new-zone join must not replay a hidden prize from the old zone"
    );
    {
        let locked = state.lock().expect("state lock");
        assert!(locked.active_hidden_prizes.is_empty());
        assert!(locked.next_hidden_prize_spawn_at_tick.is_none());
    }
    assert!(
        crate::hidden_prizes::step_hidden_prizes(&state, old_prize_spawn_tick).is_empty(),
        "the old zone's due spawn deadline must not fire in the new zone"
    );
    assert_eq!(
        state
            .lock()
            .expect("state lock")
            .next_hidden_prize_spawn_at_tick,
        Some(crate::continuum::continuum_tick_add(
            old_prize_spawn_tick,
            crate::arena_settings::arena_prize_spawn_delay_ticks(),
        ))
    );

    client.disconnect(&state);

    assert_eq!(state.lock().expect("state").config.default_zone, "extreme");
}

#[test]
fn server_packet_rejects_different_arena_login_while_active_zone_has_players() {
    let state = Arc::new(Mutex::new(ServerState::default()));

    let alpha = connect_login_and_join(&state, "alpha", 0);
    assert_server_clock_bootstrap(&alpha.read_reliable_payload(), 0);
    assert_eq!(alpha.read_reliable_payload(), [0x01, 0x00, 0x00]);
    assert_eq!(alpha.read_reliable_payload(), [0x02]);
    assert_eq!(alpha.read_reliable_payload()[0], 0x03);
    assert_arena_settings(&alpha.read_reliable_payload());
    assert_eq!(alpha.read_reliable_payload(), [TOGGLE_SEND_DAMAGE, 0]);
    assert_map_information(&alpha.read_reliable_payload());
    assert_set_coordinates_for_map(&alpha.read_reliable_payload(), zone_map());
    assert_arena_directory(&alpha.read_reliable_payload(), 1);
    assert_initial_flag_packets(&alpha);

    let mut beta = connect_and_login(&state, "beta", test_identity_for_name("beta"));
    let password = beta.read_reliable_payload();
    assert_eq!(password.first().copied(), Some(0x0a));
    assert_eq!(password.get(1).copied(), Some(0x00));

    beta.send_reliable(&state, 1, &arena_login_payload(0, "Extreme"));
    let rejected = beta.read_reliable_payload();
    assert_eq!(rejected[0], LOGIN_FAILURE_MESSAGE);
    assert_eq!(
        read_fixed_test_string(&rejected[1..]),
        "ZUBSPACE_ARENA_BUSY"
    );

    alpha.disconnect(&state);
    beta.disconnect(&state);

    assert_eq!(
        state.lock().expect("state").config.default_zone,
        "chaos-zone"
    );
}

#[test]
fn zone_selection_generation_exhaustion_is_atomic_and_then_recoverable() {
    let mut state = ServerState::default();
    state.objectives.reset(synthetic_profile(1, 0)).unwrap();
    state
        .objectives
        .create_flag(FlagState::OnMap {
            x: 320,
            y: 320,
            owner: None,
        })
        .unwrap();
    state.arena_tick = 91;
    state.objectives.force_generation(u64::MAX);
    let before_zone = state.config.default_zone.clone();
    let before_replay = state.objectives.replay();
    let request = arena_login_payload(0, "Extreme");

    assert_eq!(
        crate::arena::select_arena_zone_for_login_in_state(&mut state, &request),
        Err("ZUBSPACE_ARENA_RESET_UNAVAILABLE")
    );
    assert_eq!(state.config.default_zone, before_zone);
    assert_eq!(state.arena_tick, 91);
    assert_eq!(state.objectives.replay(), before_replay);

    state.objectives.force_generation(200);
    assert_eq!(
        crate::arena::select_arena_zone_for_login_in_state(&mut state, &request),
        Ok(())
    );
    assert_eq!(state.config.default_zone, crate::zone::EXTREME_ZONE_ID);
    assert_eq!(state.arena_tick, 0);
    assert_eq!(state.objectives.replay().generation, 201);
}

#[test]
fn concurrent_different_zone_logins_publish_selection_and_join_atomically() {
    // Zone decoding is intentionally lazy and may exceed this test's 1 s
    // synchronization timeout in an otherwise idle debug process. Warm it
    // before the concurrency window so the test measures only publication
    // atomicity.
    let _ = crate::zone::server_zone_maps();
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (alpha_tx, _alpha_rx) = mpsc::channel();
    let alpha = register_connection(&state, alpha_tx);
    let (beta_tx, _beta_rx) = mpsc::channel();
    let beta = register_connection(&state, beta_tx);

    let extreme_login = arena_login_payload(0, "Extreme");
    let chaos_login = arena_login_payload(0, "Chaos Zone");
    let (selected_tx, selected_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let alpha_state = Arc::clone(&state);
    let alpha_join = thread::spawn(move || {
        join_arena_for_login_at_tick_with_zone_selection_hook(
            &alpha_state,
            alpha,
            0,
            &extreme_login,
            70_000,
            || {
                selected_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            },
        )
    });

    selected_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("first login must reach the selected-zone hook");

    let (beta_started_tx, beta_started_rx) = mpsc::channel();
    let (beta_result_tx, beta_result_rx) = mpsc::channel();
    let beta_state = Arc::clone(&state);
    let beta_join = thread::spawn(move || {
        beta_started_tx.send(()).unwrap();
        beta_result_tx
            .send(join_arena_for_login(&beta_state, beta, 0, &chaos_login))
            .unwrap();
    });
    beta_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second login attempt must start");
    assert_eq!(
        beta_result_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "the second selector must remain blocked until the first joined state is published"
    );

    release_tx.send(()).unwrap();
    assert_eq!(alpha_join.join().unwrap(), Ok(()));
    assert_eq!(
        beta_result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err("ZUBSPACE_ARENA_BUSY")
    );
    beta_join.join().unwrap();

    let locked = state.lock().expect("state lock");
    assert_eq!(locked.config.default_zone, crate::zone::EXTREME_ZONE_ID);
    assert!(locked.clients.get(&alpha).unwrap().joined);
    assert!(!locked.clients.get(&beta).unwrap().joined);
}

// Legacy LARGE_POSITION fan-out integration test removed in Phase 4.
// The wire shape it asserted (0x05 LARGE_POSITION on a stream) was the
// legacy per-event fan-out; the snapshot stream now owns that delivery.
// Snapshot/AOI coverage of two-client position exchange lives in
// `tests::snapshot::*`.

#[test]
fn server_rejects_duplicate_player_identity_and_invalid_names() {
    let state = Arc::new(Mutex::new(ServerState::default()));

    let alpha = connect_login_and_join_with_identity(&state, "alpha", 0, 123_456_789_012);
    let mut beta = connect_and_login(&state, "beta", 123_456_789_012);
    let duplicate = beta.read_reliable_payload();
    assert_eq!(duplicate[0], LOGIN_FAILURE_MESSAGE);
    assert_eq!(
        read_fixed_test_string(&duplicate[1..]),
        "ZUBSPACE_ID_IN_USE"
    );

    beta.send_reliable(&state, 1, &[ARENA_LOGIN, 0]);
    let rejected_join = beta.read_reliable_payload();
    assert_eq!(rejected_join[0], LOGIN_FAILURE_MESSAGE);
    assert_eq!(
        read_fixed_test_string(&rejected_join[1..]),
        "ZUBSPACE_ID_INVALID"
    );

    alpha.disconnect(&state);
    beta.disconnect(&state);

    assert_eq!(normalized_login_name("bad  name"), None);
    for name in ["bad\tname", "bad  name", "x.y", "x$y", "x(y)", ""] {
        let mut invalid = connect_and_login(&state, name, 987_654_321_098);
        let rejection = invalid.read_reliable_payload();
        assert_eq!(rejection[0], LOGIN_FAILURE_MESSAGE);
        assert_eq!(
            read_fixed_test_string(&rejection[1..]),
            "ZUBSPACE_NAME_INVALID"
        );
        assert!(!crate::identity::player_login_accepted(
            &state,
            invalid.player_id
        ));
        invalid.disconnect(&state);
    }
    let mut named = connect_and_login(&state, "\u{200b}\tAce!*\u{a0}", 123_456_789_014);
    assert_eq!(named.read_reliable_payload()[0], 0x0a); // Password response.
    assert_eq!(
        state.lock().unwrap().clients[&named.player_id].player.name,
        "Ace!*"
    );
    let mut duplicate = connect_and_login(&state, "  aCE!*  ", 123_456_789_015);
    let rejection = duplicate.read_reliable_payload();
    assert_eq!(
        read_fixed_test_string(&rejection[1..]),
        "ZUBSPACE_NAME_IN_USE"
    );
    named.disconnect(&state);
    duplicate.disconnect(&state);
}

#[test]
fn wire_login_rejects_normalized_duplicate_name_without_disturbing_original() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let alpha = connect_login_and_join_with_identity(&state, "  Ace  ", 0, 123_456_789_012);
    let id = state
        .lock()
        .unwrap()
        .clients
        .values()
        .find(|client| client.player.name == "Ace")
        .unwrap()
        .player
        .id;
    let before = {
        let guard = state.lock().unwrap();
        let client = &guard.clients[&id];
        (
            client.session_id,
            client.player_identity.clone(),
            client.life_seq,
            client.player.clone(),
        )
    };
    let mut beta = connect_and_login(&state, "aCe", 123_456_789_013);
    let failure = beta.read_reliable_payload();
    assert_eq!(failure[0], LOGIN_FAILURE_MESSAGE);
    assert_eq!(
        read_fixed_test_string(&failure[1..]),
        "ZUBSPACE_NAME_IN_USE"
    );
    {
        let guard = state.lock().unwrap();
        let client = &guard.clients[&id];
        assert_eq!(
            (
                client.session_id,
                client.player_identity.clone(),
                client.life_seq
            ),
            (before.0, before.1, before.2)
        );
        assert_eq!(client.player.name, before.3.name);
        assert!(client.joined && client.alive);
    }
    alpha.disconnect(&state);
    beta.disconnect(&state);
    let replacement = connect_login_and_join_with_identity(&state, "ACE", 0, 123_456_789_014);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .clients
            .values()
            .filter(|c| c.joined)
            .count(),
        1
    );
    replacement.disconnect(&state);
}

// Startup-validated descriptor metadata; no map parsing on an HTTP request.
fn install_public_test_catalog(state: &mut ServerState) {
    state.zone_catalog_json = Some(Arc::from(
        r#"{"ok":true,"zones":[{"id":"chaos-zone","display_name":"Chaos Zone","map":"bdhadar.lvl"},{"id":"extreme","display_name":"Extreme","map":"doomsy8.lvl"}]}"#,
    ));
}

fn public_body(state: &Arc<Mutex<ServerState>>, path: &str) -> serde_json::Value {
    let response = http_round_trip(
        Arc::clone(state),
        &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
    );
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "{path}: {response}"
    );
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[test]
fn public_discovery_serializes_requested_field_order_and_escapes_names() {
    let mut initial = ServerState::default();
    install_public_test_catalog(&mut initial);
    let state = Arc::new(Mutex::new(initial));
    let (tx, _rx) = mpsc::channel();
    let id = register_connection(&state, tx);
    {
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&id).unwrap();
        client.joined = true;
        client.player.name = "A\"ce\\".into();
    }
    let get = |path| {
        http_round_trip(
            Arc::clone(&state),
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
        )
        .split_once("\r\n\r\n")
        .unwrap()
        .1
        .to_owned()
    };
    let roster = get("/v1/players");
    assert_eq!(get("/v1/zones/chaos-zone/players"), roster);
    assert_eq!(
        get("/v1/zones/extreme/players"),
        r#"{"total_players":0,"players":[]}"#
    );
    assert_eq!(
        roster,
        format!(
            r#"{{"total_players":1,"players":[{{"id":{id},"name":"A\"ce\\","bot":false,"zone":"chaos-zone"}}]}}"#
        )
    );
    let zone = get("/v1/zones/chaos-zone");
    assert_eq!(
        zone,
        r#"{"id":"chaos-zone","display_name":"Chaos Zone","status":"open","default":true,"listed":true,"total_players":1,"max_players":500,"map":"bdhadar.lvl","maxed_spawn_loadout":true,"free_for_all":true}"#
    );
    let catalog = get("/v1/zones");
    assert!(catalog.starts_with(&format!("{{\"total_players\":1,\"zones\":[{zone},")));
    assert!(
        serde_json::from_str::<serde_json::Value>(&catalog)
            .unwrap()
            .get("ok")
            .is_none()
    );
    let status = get("/v1/status");
    assert!(status.starts_with(&format!(
        "{{\"ok\":true,\"service\":\"zubspace-server\",\"version\":\"{}\",\"uptime\":",
        env!("ZUBSPACE_SERVER_BUILD_COMMIT")
    )));
    assert!(status.contains("\"zones\":2,\"zones_open\":1,\"total_players\":1,"));
}

#[test]
fn public_zone_routes_are_exact_fresh_and_read_only() {
    let mut initial = ServerState::default();
    install_public_test_catalog(&mut initial);
    let state = Arc::new(Mutex::new(initial));
    for path in [
        "/v1/zones",
        "/v1/zones/chaos-zone",
        "/v1/zones/extreme/players",
        "/v1/zones/chaos-zone/status",
        "/v1/zones/extreme/status",
    ] {
        for method in ["GET", "OPTIONS", "POST", "PUT", "DELETE", "HEAD"] {
            let response = http_round_trip(
                Arc::clone(&state),
                &format!(
                    "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n"
                ),
            );
            let expected = match method {
                "GET" => "200 OK",
                "OPTIONS" => "204 No Content",
                _ => "405 Method Not Allowed",
            };
            assert!(
                response.starts_with(&format!("HTTP/1.1 {expected}")),
                "{method} {path}: {response}"
            );
            assert!(response.contains("Access-Control-Allow-Origin: *\r\n"));
            assert_eq!(response.matches("Cache-Control: no-store\r\n").count(), 1);
        }
    }
    for path in [
        "/v1/zones/Chaos-Zone",
        "/v1/zones/-bad",
        "/v1/zones/bad-",
        "/v1/zones/abcdefghijklmnop",
        "/v1/zones/chaos-zone/",
        "/v1/zones/chaos-zone/players/",
        "/v1/zones/chaos-zone/extra",
        "/v1/zones/chaos%2fzone",
        "/v1/zones/%63haos-zone",
    ] {
        let response = http_round_trip(
            Arc::clone(&state),
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
        );
        assert!(
            response.starts_with("HTTP/1.1 404 Not Found"),
            "{path}: {response}"
        );
    }
    for suffix in ["", "/players", "/status"] {
        let response = http_round_trip(
            Arc::clone(&state),
            &format!("GET /v1/zones/missing{suffix} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
        );
        assert!(response.starts_with("HTTP/1.1 404 Not Found"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(response.split_once("\r\n\r\n").unwrap().1)
                .unwrap(),
            serde_json::json!({"ok":false,"error":"unknown_zone"})
        );
    }
}

#[test]
fn public_zone_payloads_partition_diagnostics_and_follow_the_active_zone() {
    let mut initial = ServerState::new(ServerConfig {
        bot_count: 2,
        max_players: 23,
        ..ServerConfig::default()
    });
    install_public_test_catalog(&mut initial);
    initial.initialize_configured_bots();
    let state = Arc::new(Mutex::new(initial));
    let process = public_body(&state, "/v1/status");
    assert_eq!(process["total_players"], 2);
    assert_eq!(process["zones"], 2);
    assert_eq!(process["zones_open"], 1);
    assert_eq!(
        process
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "client_debug_trace",
            "clients",
            "connection_quality",
            "observer_network_profile",
            "ok",
            "rtc_capacity",
            "rtc_channel_protocol_violations",
            "rtc_data_channel_send_buffer_bytes",
            "rtc_effective_capacity",
            "rtc_inbound_backpressure",
            "rtc_inbound_packets",
            "rtc_inbound_peak_queue_packets",
            "rtc_inbound_queue_capacity_packets",
            "rtc_inbound_queue_packets",
            "rtc_ingress_execution",
            "rtc_outbound_peak_queue_bytes",
            "rtc_outbound_peak_queue_packets",
            "rtc_outbound_queue_bytes",
            "rtc_outbound_queue_packets",
            "rtc_outbound_realtime_coalesced",
            "rtc_outbound_reliable_overflows",
            "rtc_sessions",
            "rtc_udp_degraded_ports",
            "rtc_udp_port_capacity",
            "rtc_udp_ports_degraded",
            "rtc_udp_ports_in_use",
            "service",
            "tick_rate",
            "total_players",
            "uptime",
            "version",
            "zones",
            "zones_open"
        ]
    );
    for key in [
        "zone",
        "num_players",
        "players",
        "bots",
        "uptime_seconds",
        "total",
        "free_for_all",
        "maxed_spawn_loadout",
        "rtc_snapshot_acked_clients",
    ] {
        assert!(process.get(key).is_none(), "{key}");
    }
    let zones = public_body(&state, "/v1/zones");
    assert_eq!(zones["total_players"], process["total_players"]);
    assert_eq!(zones.as_object().unwrap().len(), 2);
    let roster = public_body(&state, "/v1/players");
    assert_eq!(roster["total_players"], process["total_players"]);
    assert_eq!(roster["players"].as_array().unwrap().len(), 2);
    for player in roster["players"].as_array().unwrap() {
        assert_eq!(
            player
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["bot", "id", "name", "zone"]
        );
        assert_eq!(player["bot"], true);
        assert_eq!(player["zone"], "chaos-zone");
    }
    for metadata in zones["zones"].as_array().unwrap() {
        assert_eq!(
            metadata
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "default",
                "display_name",
                "free_for_all",
                "id",
                "listed",
                "map",
                "max_players",
                "maxed_spawn_loadout",
                "status",
                "total_players"
            ]
        );
        assert_eq!(metadata["max_players"], 23);
        let id = metadata["id"].as_str().unwrap();
        let summary = public_body(&state, &format!("/v1/zones/{id}"));
        assert!(summary.get("ok").is_none());
        assert_eq!(&summary, metadata);
        let scoped = public_body(&state, &format!("/v1/zones/{id}/players"));
        assert_eq!(scoped["total_players"], metadata["total_players"]);
        assert_eq!(
            scoped["total_players"],
            scoped["players"].as_array().unwrap().len()
        );
    }
    let simulation = public_body(&state, "/v1/zones/chaos-zone/status");
    assert_eq!(simulation["snapshot_acked_clients"], 0);
    assert!(simulation.get("tick_timing").is_some());
    for key in simulation.as_object().unwrap().keys() {
        if !matches!(key.as_str(), "ok" | "total") {
            assert!(
                process.get(key).is_none(),
                "zone field leaked into process status: {key}"
            );
        }
    }
    assert_eq!(
        public_body(&state, "/v1/zones/extreme/status"),
        serde_json::json!({"ok":true,"zone":"extreme","status":"standby","total":0})
    );
    assert_eq!(
        public_body(&state, "/v1/zones/extreme/players"),
        serde_json::json!({"total_players":0,"players":[]})
    );
    {
        let mut state = state.lock().unwrap();
        state.config.default_zone = "extreme".into();
        state.bots.clear();
        state.initialize_configured_bots();
    }
    let switched = public_body(&state, "/v1/zones");
    assert_eq!(switched["zones"][0]["status"], "standby");
    assert_eq!(switched["zones"][0]["total_players"], 0);
    assert_eq!(switched["zones"][1]["status"], "open");
    assert_eq!(switched["zones"][1]["default"], true);
    assert_eq!(switched["zones"][1]["listed"], false);
    assert_eq!(switched["zones"][1]["total_players"], 2);
    assert_eq!(
        public_body(&state, "/v1/zones/chaos-zone/players")["total_players"],
        0
    );
    assert!(
        public_body(&state, "/v1/players")["players"]
            .as_array()
            .unwrap()
            .iter()
            .all(|player| player["zone"] == "extreme")
    );
}
