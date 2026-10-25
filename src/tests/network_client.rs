use super::*;

fn captured_terminal_poll_client(
    tail: Vec<(Vec<u8>, crate::transport::PacketTransportLane)>,
) -> NetworkClient<MemoryPacketTransport> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/remote_terminal_frozen_713.json"
    ))
    .unwrap();
    let hex = fixture["packets"][2]["hex"].as_str().unwrap();
    let terminal = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect::<Vec<_>>();
    let mut inbound = vec![(terminal, crate::transport::PacketTransportLane::Event)];
    inbound.extend(tail);
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.self_player_id = Some(0);
    session.latest_server_clock_bootstrap = Some(ServerClockBootstrap {
        server_session_id: 1_792_888_843_678_433_671,
        arena_stream_id: 1,
        ..ServerClockBootstrap::default()
    });
    NetworkClient::new(session, MemoryPacketTransport::with_laned_inbound(inbound))
}

fn assert_captured_terminal_published_once(actions: &ClientProtocolActions) {
    assert_eq!(
        actions
            .events
            .iter()
            .filter(|event| matches!(event,
        ClientProtocolEvent::RealtimeProjectileState { event_id: 1871, state, .. }
            if state.terminal && state.projectile_id == 713))
            .count(),
        1,
        "a decoded terminal must reach the consumer after the recoverable poll error"
    );
}

#[test]
fn decoded_terminal_survives_a_later_malformed_datagram_in_the_same_poll() {
    let mut client = captured_terminal_poll_client(vec![(
        vec![0x7d, 2],
        crate::transport::PacketTransportLane::Event,
    )]);
    assert!(client.poll_actions_at_tick(6_158).is_err());
    let recovered = client.poll_actions_at_tick(6_159).unwrap();
    assert_captured_terminal_published_once(&recovered);
    assert!(
        client
            .poll_actions_at_tick(6_160)
            .unwrap()
            .events
            .is_empty()
    );
}

#[test]
fn decoded_terminal_and_current_packet_survive_an_outbound_ack_send_failure() {
    let reliable = ReliableSequencer::encode_reliable_message(0, &[0xee, 1]);
    let mut client = captured_terminal_poll_client(vec![(
        reliable.clone(),
        crate::transport::PacketTransportLane::Reliable,
    )]);
    client.transport_mut().send_failures_remaining = 1;
    assert_eq!(
        client.poll_actions_at_tick(6_158),
        Err("synthetic transport send failure".into())
    );
    let recovered = client.poll_actions_at_tick(6_159).unwrap();
    assert_captured_terminal_published_once(&recovered);
    assert!(
        recovered
            .events
            .contains(&ClientProtocolEvent::UnknownPacket(0xee)),
        "the packet whose ACK failed must retain its already decoded event too"
    );
    client.transport_mut().inbound.push_back(reliable);
    client
        .transport_mut()
        .inbound_lanes
        .push_back(crate::transport::PacketTransportLane::Reliable);
    assert!(
        client
            .poll_actions_at_tick(6_160)
            .unwrap()
            .events
            .is_empty(),
        "the server's retransmitted wrapper is ACKed without replaying its event"
    );
    assert!(
        client
            .transport()
            .sent
            .contains(&ReliableSequencer::encode_ack(0))
    );
}

#[test]
fn repeated_poll_errors_retain_decoded_events_without_exceeding_the_event_budget() {
    let count = crate::MAX_POLLED_EVENTS_PER_POLL + 3;
    let inbound = (0..count)
        .flat_map(|_| {
            [
                (vec![0xee, 1], crate::transport::PacketTransportLane::Event),
                (vec![0x7d, 2], crate::transport::PacketTransportLane::Event),
            ]
        })
        .collect();
    let session = ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut client =
        NetworkClient::new(session, MemoryPacketTransport::with_laned_inbound(inbound));
    let mut delivered = 0;
    while !client.transport().inbound.is_empty() {
        if let Ok(actions) = client.poll_actions_at_tick(100) {
            assert!(actions.events.len() <= crate::MAX_POLLED_EVENTS_PER_POLL);
            delivered += actions.events.len();
        }
    }
    let tail = client.poll_actions_at_tick(101).unwrap();
    assert!(tail.events.len() <= crate::MAX_POLLED_EVENTS_PER_POLL);
    delivered += tail.events.len();
    assert_eq!(
        delivered, count,
        "recoverable errors cannot silently discard an admitted prefix"
    );
}

#[test]
fn production_poll_preserves_reliable_chunks_across_realtime_channel_callbacks() {
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let split = 7;
    let mut body = vec![PacketChunker::CORE_BYTE, PacketChunker::SMALL_BODY_TYPE];
    body.extend_from_slice(&response[..split]);
    let mut tail = vec![PacketChunker::CORE_BYTE, PacketChunker::SMALL_TAIL_TYPE];
    tail.extend_from_slice(&response[split..]);
    let transport = MemoryPacketTransport::with_laned_inbound(vec![
        (
            ReliableSequencer::encode_reliable_message(0, &body),
            crate::transport::PacketTransportLane::Reliable,
        ),
        (
            vec![0xee, 1, 2, 3],
            crate::transport::PacketTransportLane::Game,
        ),
        (
            ReliableSequencer::encode_reliable_message(1, &tail),
            crate::transport::PacketTransportLane::Reliable,
        ),
    ]);
    let session = ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut client = NetworkClient::new(session, transport);

    let actions = client
        .poll_actions_at_tick(77)
        .expect("cross-channel callback order must preserve reliable assembly");
    assert!(
        actions
            .events
            .contains(&ClientProtocolEvent::PasswordAccepted)
    );
    assert!(
        actions
            .events
            .contains(&ClientProtocolEvent::UnknownPacket(0xee))
    );
    assert_eq!(client.session.chunker.small.data.capacity(), 0);
    assert_eq!(
        client
            .transport
            .sent
            .iter()
            .filter(|packet| {
                packet.get(..2)
                    == Some(&[
                        ReliableSequencer::CORE_BYTE,
                        ReliableSequencer::RELIABLE_ACK_TYPE,
                    ])
            })
            .count(),
        2,
        "both wrappers are ACKed"
    );
}

#[test]
fn one_poll_decodes_only_a_bounded_slice_of_a_transport_backlog() {
    use crate::MAX_POLLED_EVENTS_PER_POLL;
    let backlog = MAX_POLLED_EVENTS_PER_POLL + 64;
    let inbound: Vec<(Vec<u8>, crate::transport::PacketTransportLane)> = (0..backlog)
        .map(|_| {
            (
                vec![0xee, 1, 2, 3],
                crate::transport::PacketTransportLane::Game,
            )
        })
        .collect();
    let transport = MemoryPacketTransport::with_laned_inbound(inbound);
    let session = ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut client = NetworkClient::new(session, transport);

    let first = client
        .poll_actions_at_tick(10)
        .expect("the bounded slice must decode");
    assert_eq!(
        first.events.len(),
        MAX_POLLED_EVENTS_PER_POLL,
        "a backlog must not arrive as one unbounded batch",
    );
    let second = client
        .poll_actions_at_tick(11)
        .expect("the queued remainder must survive the budget");
    assert_eq!(
        second.events.len(),
        backlog - MAX_POLLED_EVENTS_PER_POLL,
        "the next poll must decode exactly the queued remainder",
    );
}

#[test]
fn network_client_sends_session_packets_to_transport() {
    let session = ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut client = NetworkClient::new(session, MemoryPacketTransport::default());
    let login = PasswordLoginRequest::new(LoginProtocol::Continuum, false, "pilot", "", 0, 0, 0, 0);
    let position = ClientPositionUpdate {
        direction: 4,
        timestamp: 10,
        x: 16,
        y: 32,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        bounty: 0,
        energy: 900,
        weapon: 0,
        extra: None,
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            4, 16, 32, 0, 0,
        ),
        input_history: None,
        warp_collectible_id: None,
        repel_application: None,
    };

    client
        .send_password_login(&login)
        .expect("login send should succeed");
    client
        .send_position_update(&position)
        .expect("position send should succeed");
    client
        .send_chat_message(&ChatMessageRequest::public("hi"))
        .expect("chat send should succeed");
    client
        .try_send_frequency_change(123, 77, &position)
        .expect("frequency send should succeed");
    client
        .send_spectate_request(9)
        .expect("spectate send should succeed");
    client
        .send_attach_request(10)
        .expect("attach send should succeed");
    client
        .send_flag_request(3)
        .expect("flag send should succeed");
    let death = DeathRequest {
        killer_id: 9,
        bounty: 77,
    };
    client
        .send_death(&death)
        .expect("death send should succeed");
    client
        .send_leave_arena()
        .expect("leave arena send should succeed");
    client
        .send_disconnect()
        .expect("disconnect send should succeed");
    let voice_upload = VoiceUploadRequest {
        index: 2,
        player_id: 9,
        wav_data: b"RIFF".to_vec(),
    };
    client
        .send_voice_upload(&voice_upload)
        .expect("voice upload send should succeed");
    let client_file = ClientFileTransferRequest {
        filename: "profile.dat".to_string(),
        data: b"payload".to_vec(),
    };
    client
        .send_client_file_transfer(&client_file)
        .expect("client file transfer send should succeed");
    let registration = RegistrationFormResponse {
        real_name: "Pilot".to_string(),
        email: "pilot@example.net".to_string(),
        city: "Ottawa".to_string(),
        state: "ON".to_string(),
        sex: b'M',
        age: 42,
        connection_home: true,
        connection_work: false,
        connection_school: true,
        processor_type: 0x0102_0304,
        unknown: 0,
        windows_registration_name: String::new(),
        windows_registration_organization: String::new(),
        driver_descriptions: std::array::from_fn(|_| String::new()),
    };
    client
        .send_registration_response(&registration)
        .expect("registration response send should succeed");
    let lvz_toggle = LvzTogglePacket {
        toggles: vec![
            LvzObjectToggle {
                object_id: 0x1234,
                enabled: true,
            },
            LvzObjectToggle {
                object_id: 0x0042,
                enabled: false,
            },
        ],
    };
    client
        .send_lvz_toggle(&lvz_toggle)
        .expect("lvz toggle send should succeed");
    let lvz_modify = LvzModifyPacket {
        changed: LvzModificationFlags {
            xy: true,
            image: true,
            layer: true,
            time: false,
            mode: false,
        },
        map_object: false,
        object_id: 8,
        map_x: 0,
        map_y: 0,
        screen_x: -7,
        screen_y: 25,
        x_type: 3,
        y_type: 4,
        image_id: 7,
        layer: 2,
        display_time: 0,
        display_mode: 0,
    };
    client
        .send_lvz_modify(&lvz_modify)
        .expect("lvz modify send should succeed");
    client
        .send_exe_request()
        .expect("exe request send should succeed");
    client
        .send_news_request()
        .expect("news request send should succeed");
    let violation = SecurityViolationReport { violation_id: 0x15 };
    client
        .send_security_violation(&violation)
        .expect("security violation send should succeed");
    let checksum_violation = SecurityViolationChecksumReport {
        unknown: 0x0102_0304,
        arena_settings_checksum: 0x1112_1314,
        code_checksum_1: 0x2122_2324,
        code_checksum_2: 0x3132_3334,
        violation_id: 0x1b,
    };
    client
        .send_security_violation_checksum(&checksum_violation)
        .expect("security violation checksum send should succeed");
    let settings_change = ArenaSettingsChangeRequest::new(["Warbird:MaximumEnergy:2800"]);
    client
        .send_arena_settings_change(&settings_change)
        .expect("arena settings change send should succeed");
    client
        .send_drop_attached()
        .expect("drop attached send should succeed");
    client
        .send_drop_flags()
        .expect("drop flags send should succeed");
    client
        .send_end_koth_timer()
        .expect("end koth timer send should succeed");
    client
        .try_send_ship_change(4, None)
        .expect("ship change send should succeed");
    let mut banner = [0; PLAYER_BANNER_BYTES];
    banner[0] = 7;
    banner[95] = 9;
    let set_banner = SetBannerRequest { banner };
    client
        .send_set_banner(&set_banner)
        .expect("set banner send should succeed");
    let take_prize = TakePrizeRequest {
        collectible_id: 0x0102_0304,
        x: 44,
        y: 55,
        prize_id: -21,
    };
    client
        .send_take_prize(&take_prize, false)
        .expect("unreliable prize send should succeed");
    client
        .send_take_prize(&take_prize, true)
        .expect("reliable prize send should succeed");
    let drop_brick = DropBrickRequest { x: 100, y: 200 };
    client
        .send_drop_brick(&drop_brick)
        .expect("drop brick send should succeed");
    let pickup = BallPickupRequest {
        ball_id: 2,
        timestamp: 0x1122_3344,
    };
    client
        .send_ball_pickup(&pickup)
        .expect("ball pickup send should succeed");
    let fire = BallFireRequest {
        ball_id: 3,
        x: 16,
        y: 32,
        velocity_x: -120,
        velocity_y: 240,
        player_id: 9,
        timestamp: 0x0102_0304,
    };
    client
        .send_ball_fire(&fire)
        .expect("ball fire send should succeed");
    let goal = BallGoalRequest {
        ball_id: 4,
        x: -5,
        y: 6,
    };
    client
        .send_ball_goal(&goal)
        .expect("ball goal send should succeed");

    let mut expected_packets = vec![
        ReliableSequencer::encode_reliable_message(0, &login.encode_payload()),
        position.encode(),
        ReliableSequencer::encode_reliable_message(
            1,
            &[ChatMessageRequest::TYPE, 2, 0, 0, 0, b'h', b'i', 0],
        ),
        ReliableSequencer::encode_reliable_message(
            2,
            &FrequencyChangeRequest {
                frequency: 123,
                position_sequence_base: 77,
                baseline: position.encode(),
            }
            .encode_payload(),
        ),
        ReliableSequencer::encode_reliable_message(3, &[SpectateRequest::TYPE, 9, 0]),
        ReliableSequencer::encode_reliable_message(4, &[AttachRequest::TYPE, 10, 0]),
        ReliableSequencer::encode_reliable_message(5, &[FlagRequest::TYPE, 3, 0]),
        ReliableSequencer::encode_reliable_message(6, &death.encode_payload()),
        ReliableSequencer::encode_reliable_message(7, &[LeaveArenaRequest::TYPE]),
        CoreDisconnectRequest.encode(),
        ReliableSequencer::encode_reliable_message(8, &voice_upload.encode_payload()),
        ReliableSequencer::encode_reliable_message(9, &client_file.encode_payload()),
    ];
    expected_packets.extend(reliable_small_chunk_packets(
        10,
        &registration.encode_payload(),
    ));
    expected_packets.extend([
        ReliableSequencer::encode_reliable_message(12, &lvz_toggle.encode_payload()),
        ReliableSequencer::encode_reliable_message(13, &lvz_modify.encode_payload()),
        ReliableSequencer::encode_reliable_message(14, &[ExeRequest::TYPE]),
        ReliableSequencer::encode_reliable_message(15, &[NewsRequest::TYPE]),
        ReliableSequencer::encode_reliable_message(16, &violation.encode_payload()),
        ReliableSequencer::encode_reliable_message(17, &checksum_violation.encode_payload()),
        ReliableSequencer::encode_reliable_message(18, &settings_change.encode_payload()),
        ReliableSequencer::encode_reliable_message(19, &[DropAttachedRequest::TYPE]),
        ReliableSequencer::encode_reliable_message(20, &[DropFlagsRequest::TYPE]),
        ReliableSequencer::encode_reliable_message(21, &[EndKothTimerRequest::TYPE]),
        ReliableSequencer::encode_reliable_message(22, &[ShipChangeRequest::TYPE, 4]),
        ReliableSequencer::encode_reliable_message(23, &set_banner.encode_payload()),
        take_prize.encode_payload(),
        ReliableSequencer::encode_reliable_message(24, &take_prize.encode_payload()),
        ReliableSequencer::encode_reliable_message(25, &drop_brick.encode_payload()),
        ReliableSequencer::encode_reliable_message(26, &pickup.encode_payload()),
        ReliableSequencer::encode_reliable_message(27, &fire.encode_payload()),
        ReliableSequencer::encode_reliable_message(28, &goal.encode_payload()),
    ]);

    assert_eq!(client.transport().sent, expected_packets);
    assert_eq!(client.session().state, ClientProtocolState::Disconnected);
}

#[test]
fn network_client_time_sync_reports_packet_counters() {
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let reliable = ReliableSequencer::encode_reliable_message(0, &response);
    let arena_login = ArenaLoginRequest::new(8, 0, 1024, 768, 0xffff, "alpha", true);
    let session = ClientProtocolSession::new(true, arena_login, 0, 0);
    let mut client =
        NetworkClient::new(session, MemoryPacketTransport::with_inbound(vec![reliable]));

    client.poll_actions().expect("poll should process packet");
    client
        .send_time_sync_request(0x1122_3344)
        .expect("time sync send should succeed");

    let sync = client
        .transport()
        .sent
        .last()
        .expect("time sync should send");
    assert_eq!(
        sync,
        &vec![
            0x00, 0x05, 0x44, 0x33, 0x22, 0x11, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        ]
    );
}

#[test]
fn network_client_resends_timed_out_reliable_packets() {
    let arena_login = ArenaLoginRequest::new(8, 0, 1024, 768, 0xffff, "alpha", true);
    let mut session = ClientProtocolSession::new(true, arena_login, 0, 0);
    let login = PasswordLoginRequest::new(LoginProtocol::Continuum, false, "pilot", "", 0, 0, 0, 0);
    let login_packet = session.send_password_login(&login);
    session.sequencer.sent[0].last_sent_tick = 100;
    let mut client = NetworkClient::new(session, MemoryPacketTransport::default());

    assert_eq!(
        client
            .resend_timed_out_reliable(399)
            .expect("resend should run"),
        0
    );
    assert!(client.transport().sent.is_empty());
    assert_eq!(
        client
            .resend_timed_out_reliable(400)
            .expect("resend should run"),
        1
    );
    assert_eq!(client.transport().sent, vec![login_packet]);
}

#[test]
fn network_client_retries_the_exact_prize_application_when_its_receipt_is_lost() {
    let arena_login = ArenaLoginRequest::new(8, 0, 1024, 768, 0xffff, "alpha", true);
    let mut session = ClientProtocolSession::new(true, arena_login, 0, 0);
    session.set_outbound_protocol_tick(100);
    let application = PrizeEffectApplied {
        server_session_id: 77,
        player_id: 3,
        life_seq: 9,
        collectible_id: u32::MAX,
        prize_id: -14,
        prize_state_revision: u32::MAX,
        application_sequence: 0,
    };
    let expected = ReliableSequencer::encode_reliable_message(0, &application.encode());
    let mut client = NetworkClient::new(session, MemoryPacketTransport::default());

    assert_eq!(
        client
            .try_send_prize_effect_applied(application)
            .expect("initial application send"),
        Some(0)
    );
    assert_eq!(client.transport().sent, vec![expected.clone()]);
    assert_eq!(
        client
            .resend_timed_out_reliable(399)
            .expect("pre-timeout retry check"),
        0
    );
    assert_eq!(
        client
            .resend_timed_out_reliable(400)
            .expect("lost receipt retry"),
        1
    );
    assert_eq!(
        client.transport().sent,
        vec![expected.clone(), expected],
        "retry preserves the exact identity, revision, and wrapped application sequence"
    );
}

#[test]
fn network_client_polls_transport_flushes_outbound_protocol_packets() {
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let reliable = ReliableSequencer::encode_reliable_message(0, &response);
    let arena_login = ArenaLoginRequest::new(8, 0, 1024, 768, 0xffff, "alpha", true);
    let mut session = ClientProtocolSession::new(true, arena_login.clone(), 0, 0);
    session.set_outbound_protocol_tick(0x7788_9900);
    let mut client =
        NetworkClient::new(session, MemoryPacketTransport::with_inbound(vec![reliable]));

    let actions = client.poll_actions().expect("poll should process packet");

    assert_eq!(actions.events, vec![ClientProtocolEvent::PasswordAccepted]);
    assert_eq!(
        client.transport().sent,
        vec![
            ReliableSequencer::encode_ack(0),
            ReliableSequencer::encode_reliable_message(0, &arena_login.encode_payload()),
        ]
    );
    assert_eq!(
        client.session().sequencer.sent[0].last_sent_tick,
        0x7788_9900
    );
}

#[test]
fn network_client_polls_transport_into_network_world() {
    let enter = sample_player_enter_packet(9, b"remote");
    let mut position = vec![0; PlayerPositionUpdate::LARGE_MIN_LEN];
    position[0] = PlayerPositionUpdate::LARGE_TYPE;
    position[1] = 10;
    put_u16(&mut position, 2, 44);
    put_u16(&mut position, 4, 32);
    put_i16(&mut position, 6, -80);
    put_u16(&mut position, 8, 9);
    put_i16(&mut position, 10, 160);
    put_u16(&mut position, 15, 48);
    put_u16(&mut position, 17, 6);
    let session = ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut client = NetworkClient::new(
        session,
        MemoryPacketTransport::with_inbound(vec![enter, position]),
    );
    let mut world = NetworkWorld::default();

    let actions = client
        .poll_world(&mut world)
        .expect("poll should apply into world");

    assert_eq!(actions.events.len(), 2);
    let player = world.players.get(&9).expect("remote player should exist");
    assert_eq!(player.name, "remote");
    assert_eq!(player.position, Some(vec2(32.0, 48.0)));
    assert_eq!(player.velocity, vec2(16.0, -8.0));
    assert_eq!(player.orientation, 0.25);
    assert_eq!(player.bounty, 6);
}

fn full_wrapping_reliable_client() -> (NetworkClient<MemoryPacketTransport>, Vec<u32>) {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.sequencer.next_outbound_id = u32::MAX - 127;
    let mut ids = Vec::new();
    for index in 0..ReliableSequencer::MAX_SENT_RELIABLES {
        let (id, _) = session
            .sequencer
            .try_send_tracked_reliable_at_tick(&[0xa0, index as u8], 0)
            .expect("table should fill exactly");
        ids.push(id);
    }
    assert_eq!(ids[127], u32::MAX);
    assert_eq!(ids[128], 0);
    (
        NetworkClient::new(session, MemoryPacketTransport::default()),
        ids,
    )
}

fn objective_action_request(action_id: u32) -> ObjectiveActionRequest {
    ObjectiveActionRequest {
        kind: ObjectiveActionKind::FlagClaim,
        server_session_id: 11,
        arena_stream_id: 12,
        player_id: 13,
        life_seq: 14,
        action_id,
        position_sequence: 15,
        objective_id: 16,
        objective_generation: 17,
        expected_version: 18,
    }
}

#[test]
fn objective_action_waits_at_reliable_capacity_without_consuming_an_id() {
    let (mut client, ids) = full_wrapping_reliable_client();
    let request = objective_action_request(1);
    let next_id = client.session.sequencer.next_outbound_id;
    let before = client.session.sequencer.sent.clone();

    assert_eq!(client.try_send_objective_action(request), Ok(None));
    assert_eq!(client.session.sequencer.next_outbound_id, next_id);
    assert_eq!(client.session.sequencer.sent, before);
    assert!(client.transport().sent.is_empty());

    assert!(
        client
            .session
            .sequencer
            .receive_ack(&ReliableSequencer::encode_ack(ids[37]))
            .unwrap()
    );
    assert_eq!(client.try_send_objective_action(request), Ok(Some(next_id)));
    assert_eq!(client.session.sequencer.sent.len(), before.len());
    assert_eq!(client.transport().sent.len(), 1);
}

#[test]
fn objective_action_transport_failure_rolls_back_its_exact_reservation() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.sequencer.next_outbound_id = 77;
    let mut transport = MemoryPacketTransport::default();
    transport.send_failures_remaining = 1;
    let mut client = NetworkClient::new(session, transport);
    let request = objective_action_request(2);

    assert_eq!(
        client.try_send_objective_action(request),
        Err("synthetic transport send failure".to_string())
    );
    assert_eq!(client.session.sequencer.next_outbound_id, 77);
    assert!(client.session.sequencer.sent.is_empty());

    assert_eq!(client.try_send_objective_action(request), Ok(Some(77)));
    assert_eq!(client.session.sequencer.next_outbound_id, 78);
    assert_eq!(client.session.sequencer.sent.len(), 1);
}

fn lifecycle_baseline(sequence: u16, input: ClientInputRecordPacket) -> ClientPositionUpdate {
    ClientPositionUpdate {
        direction: 7,
        timestamp: 0,
        x: 8_200,
        y: 8_200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        bounty: 500,
        energy: 1_700,
        weapon: 0,
        extra: Some(ClientPositionExtra {
            items: crate::continuum_position::zubspace_position_sequence_items(sequence),
            ..ClientPositionExtra::default()
        }),
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            7, 8_200, 8_200, 0, 0,
        ),
        input_history: Some(input),
        warp_collectible_id: None,
        repel_application: None,
    }
}

#[test]
fn ship_request_waits_at_256_without_frame_id_or_boundary_consumption_then_wraps_once() {
    let (mut client, ids) = full_wrapping_reliable_client();
    let before_sequencer = client.session.sequencer.clone();
    let before_transport = client.transport().sent.clone();
    let input_ring = ClientInputRing::default();
    let before_ring = input_ring.clone();
    let next_input_sequence = u16::MAX;

    assert_eq!(
        client
            .try_send_ship_change(4, Some(next_input_sequence))
            .expect("full table is ordinary backpressure"),
        None
    );
    assert_eq!(client.session.sequencer, before_sequencer);
    assert_eq!(client.transport().sent, before_transport);
    assert_eq!(input_ring, before_ring);
    assert_eq!(next_input_sequence, u16::MAX);

    client
        .session
        .sequencer
        .receive_ack(&ReliableSequencer::encode_ack(ids[0]))
        .unwrap();
    let request_id = client
        .try_send_ship_change(4, Some(next_input_sequence))
        .unwrap()
        .expect("one ACK should admit the exact queued request");
    assert_eq!(request_id, 128);
    assert_eq!(client.transport().sent.len(), 1);
    assert_eq!(
        client.transport().sent[0],
        ReliableSequencer::encode_reliable_message(
            request_id,
            &ShipChangeRequest {
                ship: 4,
                position_sequence_base: Some(u16::MAX),
            }
            .encode_payload(),
        )
    );

    client
        .session
        .sequencer
        .receive_ack(&ReliableSequencer::encode_ack(request_id))
        .unwrap();
    assert!(!client.reliable_prefix_acked_through(request_id));
    for id in ids.into_iter().skip(1) {
        client
            .session
            .sequencer
            .receive_ack(&ReliableSequencer::encode_ack(id))
            .unwrap();
    }
    assert!(client.reliable_prefix_acked_through(request_id));
    assert!(
        !client
            .session
            .sequencer
            .receive_ack(&ReliableSequencer::encode_ack(request_id))
            .unwrap()
    );
}

#[test]
fn frequency_request_waits_at_256_without_frame_id_ring_or_sequence_then_commits_once() {
    let (mut client, ids) = full_wrapping_reliable_client();
    let mut input_ring = ClientInputRing::default();
    input_ring.record(
        u16::MAX - 1,
        PlayerInputRecord {
            bits: 3,
            direction: 6,
            door_seed: 0,
            combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(
                u16::MAX - 1,
            ),
        },
    );
    let before_ring = input_ring.clone();
    let mut next_input_sequence = u16::MAX;
    let mut candidate_ring = input_ring.clone();
    candidate_ring.clear();
    let baseline_input = candidate_ring.record(
        next_input_sequence,
        PlayerInputRecord {
            bits: 1,
            direction: 7,
            door_seed: 0,
            combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(
                next_input_sequence,
            ),
        },
    );
    let baseline = lifecycle_baseline(next_input_sequence, baseline_input);
    let before_sequencer = client.session.sequencer.clone();

    assert_eq!(
        client
            .try_send_frequency_change(19, next_input_sequence, &baseline)
            .expect("full table is ordinary backpressure"),
        None
    );
    assert_eq!(client.session.sequencer, before_sequencer);
    assert!(client.transport().sent.is_empty());
    assert_eq!(input_ring, before_ring);
    assert_eq!(next_input_sequence, u16::MAX);

    client
        .session
        .sequencer
        .receive_ack(&ReliableSequencer::encode_ack(ids[0]))
        .unwrap();
    let request_id = client
        .try_send_frequency_change(19, next_input_sequence, &baseline)
        .unwrap()
        .expect("one ACK should admit the exact queued request");
    input_ring = candidate_ring;
    next_input_sequence = next_input_sequence.wrapping_add(1);
    assert_eq!(request_id, 128);
    assert_eq!(
        input_ring
            .record(
                u16::MAX,
                PlayerInputRecord {
                    bits: 1,
                    direction: 7,
                    door_seed: 0,
                    combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(
                        u16::MAX
                    ),
                }
            )
            .base_seq,
        u16::MAX
    );
    assert_eq!(next_input_sequence, 0);
    assert_eq!(client.transport().sent.len(), 1);
    let expected_payload = FrequencyChangeRequest {
        frequency: 19,
        position_sequence_base: u16::MAX,
        baseline: baseline.encode(),
    }
    .encode_payload();
    assert_eq!(
        client.transport().sent[0],
        ReliableSequencer::encode_reliable_message(request_id, &expected_payload)
    );

    client
        .session
        .sequencer
        .receive_ack(&ReliableSequencer::encode_ack(request_id))
        .unwrap();
    assert!(!client.reliable_prefix_acked_through(request_id));
    for id in ids.into_iter().skip(1) {
        client
            .session
            .sequencer
            .receive_ack(&ReliableSequencer::encode_ack(id))
            .unwrap();
    }
    assert!(client.reliable_prefix_acked_through(request_id));
}

#[test]
fn a_legacy_flag_position_packet_cannot_erase_an_authoritative_carrier() {
    use crate::protocol::{FlagPosition, ObjectiveStateUpdate};

    // The registry says the flag is carried (version 2), and the client has that
    // presentation. A join baseline for a second client then sends the legacy
    // 0x12 pair, whose position leg carries the carrier's own flight position.
    // Applying it used to replace the whole entry - carrier erased, flag back on
    // the map unowned - which is what made every networked carry last one sample.
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::ObjectiveState(ObjectiveStateUpdate {
        kind: ObjectiveStateUpdate::KIND_FLAG,
        state: ObjectiveStateUpdate::STATE_CARRIED,
        objective_id: 0,
        generation: 1,
        version: 2,
        x: 8200,
        y: 8200,
        velocity_x: 0,
        velocity_y: 0,
        owner_id: Some(0),
        owner_frequency: Some(7),
        server_tick: 100,
        pickup_unlock_tick: None,
        killer_id: None,
        killer_pickup_unlock_tick: None,
        action_id: 1,
        outcome: 0,
    }));
    assert_eq!(world.flags[&0].carried_by, Some(0));

    world.apply_event(&ClientProtocolEvent::FlagPosition(FlagPosition {
        flag_id: 0,
        x: 7320,
        y: 7736,
        owner: u16::MAX,
    }));

    let flag = &world.flags[&0];
    assert_eq!(
        flag.carried_by,
        Some(0),
        "a versionless legacy position packet must not take a carried flag away"
    );
    assert_eq!(
        flag.position,
        vec2(8200.0, 8200.0),
        "the authoritative position survives with the carrier"
    );
    assert!(
        !flag.dropped,
        "a carried flag must not be re-presented as dropped on the map"
    );
}

#[test]
fn a_flag_claim_with_no_existing_entry_still_presents_the_carrier() {
    use crate::protocol::{FlagClaim, FlagPosition};

    // The join baseline sends the legacy position and claim as a pair. The claim
    // used to be a silent no-op when no entry existed yet, which is why a peer
    // that joined mid-carry never showed the carrier at all.
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::FlagPosition(FlagPosition {
        flag_id: 0,
        x: 8200,
        y: 8200,
        owner: u16::MAX,
    }));
    world.apply_event(&ClientProtocolEvent::DropFlag(crate::protocol::DropFlag {
        player_id: 0,
    }));
    assert!(world.flags[&0].carried_by.is_none());
    world.flags.remove(&0);

    world.apply_event(&ClientProtocolEvent::FlagClaim(FlagClaim {
        flag_id: 0,
        player_id: 0,
    }));

    let flag = &world.flags[&0];
    assert_eq!(
        flag.carried_by,
        Some(0),
        "a claim must present the carrier even with no prior entry"
    );
    assert!(!flag.dropped);
}
