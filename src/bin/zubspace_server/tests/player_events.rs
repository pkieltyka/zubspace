use super::*;

#[test]
fn server_encodes_chat_request_as_sender_chat_packet() {
    let packet = chat_packet_from_request(9, &[0x06, 2, 0, 0, 0, b'h', b'i', 0])
        .expect("chat request should encode");

    assert_eq!(packet, [0x07, 2, 0, 9, 0, b'h', b'i', 0]);
    assert_eq!(chat_packet_from_request(9, &[0x06, 2, 0, 0, 0, 0]), None);
}

#[test]
fn server_accepts_bounded_chat_recovery_burst_and_preserves_sustained_rate() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let now = Instant::now();
    for _ in 0..4 {
        crate::player_events::relay_chat_at(
            &state,
            alpha,
            &[CHAT_MESSAGE_REQUEST, 2, 0, 0, 0, b'h', b'i', 0],
            now,
        );
        let expected = [CHAT_PACKET, CHAT_TYPE_PUBLIC, 0, 0, 0, b'h', b'i', 0];
        assert_eq!(read_reliable_payload_from_channel(&rx_a), expected);
        assert_eq!(read_reliable_payload_from_channel(&rx_b), expected);
    }

    for _ in 0..100 {
        crate::player_events::relay_chat_at(
            &state,
            alpha,
            &[CHAT_MESSAGE_REQUEST, 2, 0, 0, 0, b'n', b'o', 0],
            now + Duration::from_millis(249),
        );
    }
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    crate::player_events::relay_chat_at(
        &state,
        alpha,
        &[CHAT_MESSAGE_REQUEST, 2, 0, 0, 0, b'o', b'k', 0],
        now + Duration::from_millis(250),
    );
    let expected = [CHAT_PACKET, CHAT_TYPE_PUBLIC, 0, 0, 0, b'o', b'k', 0];
    assert_eq!(read_reliable_payload_from_channel(&rx_a), expected);
    assert_eq!(read_reliable_payload_from_channel(&rx_b), expected);
    // An idle minute restores only four messages, not unlimited saved credit.
    for _ in 0..5 {
        crate::player_events::relay_chat_at(
            &state,
            alpha,
            &[CHAT_MESSAGE_REQUEST, 2, 0, 0, 0, b'o', b'k', 0],
            now + Duration::from_secs(60),
        );
    }
    for _ in 0..4 {
        assert_eq!(read_reliable_payload_from_channel(&rx_a), expected);
        assert_eq!(read_reliable_payload_from_channel(&rx_b), expected);
    }
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn server_routes_team_chat_only_to_matching_frequency() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let (tx_c, rx_c) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    let gamma = register_connection(&state, tx_c);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    join_arena(&state, gamma, 0);
    update_frequency(&state, beta, &frequency_change_request(&state, beta, 0));
    consume_pending_lifecycle_baseline(&state, beta);
    update_frequency(&state, gamma, &frequency_change_request(&state, gamma, 7));
    consume_pending_lifecycle_baseline(&state, gamma);
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);

    relay_chat(
        &state,
        alpha,
        &[CHAT_MESSAGE_REQUEST, CHAT_TYPE_TEAM, 0, 0, 0, b'g', b'o', 0],
    );

    let expected = [CHAT_PACKET, CHAT_TYPE_TEAM, 0, 0, 0, b'g', b'o', 0];
    assert_eq!(read_reliable_payload_from_channel(&rx_a), expected);
    assert_eq!(read_reliable_payload_from_channel(&rx_b), expected);
    assert!(rx_c.try_recv().is_err());
}

#[test]
fn server_routes_private_chat_only_to_sender_and_target() {
    let state = Arc::new(Mutex::new(team_mode_state()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let (tx_c, rx_c) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    let gamma = register_connection(&state, tx_c);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    join_arena(&state, gamma, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);

    relay_chat(
        &state,
        alpha,
        &[
            CHAT_MESSAGE_REQUEST,
            CHAT_TYPE_PRIVATE,
            5,
            beta.to_le_bytes()[0],
            beta.to_le_bytes()[1],
            b'h',
            b'i',
            0,
        ],
    );

    let expected = [CHAT_PACKET, CHAT_TYPE_PRIVATE, 5, 0, 0, b'h', b'i', 0];
    assert_eq!(read_reliable_payload_from_channel(&rx_a), expected);
    assert_eq!(read_reliable_payload_from_channel(&rx_b), expected);
    assert!(rx_c.try_recv().is_err());
}

#[test]
fn server_preserves_long_chat_and_bounds_oversized_unicode_messages() {
    for message in [
        "x".repeat(256),
        "🚀".repeat(256),
        "x".repeat(300),
        "🚀".repeat(300),
    ] {
        let mut request = vec![0x06, 2, 0, 0, 0];
        request.extend(message.as_bytes());
        request.push(0);
        let packet = chat_packet_from_request(9, &request).unwrap();
        let body = std::str::from_utf8(&packet[5..packet.len() - 1]).unwrap();
        assert_eq!(body, message.chars().take(256).collect::<String>());
        assert!(body.len() <= 1024);
    }
}
