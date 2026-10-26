use super::*;

#[test]
fn client_protocol_session_queues_arena_login_after_password_acceptance() {
    let arena_login = ArenaLoginRequest::new(8, 0, 1024, 768, 0xffff, "alpha", true);
    let mut session = ClientProtocolSession::new(true, arena_login.clone(), 0, 0);
    let login = PasswordLoginRequest::new(LoginProtocol::Continuum, false, "pilot", "", 0, 0, 0, 0);

    assert_eq!(
        session.send_password_login(&login),
        ReliableSequencer::encode_reliable_message(0, &login.encode_payload())
    );
    assert_eq!(session.state, ClientProtocolState::LoginSent);

    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    response[1] = 0x00;
    let actions = session
        .handle_server_packet(&response)
        .expect("accepted login response should queue arena login");

    assert_eq!(session.state, ClientProtocolState::ArenaLoginSent);
    assert_eq!(actions.events, vec![ClientProtocolEvent::PasswordAccepted]);
    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_reliable_message(
            1,
            &arena_login.encode_payload()
        )]
    );
}

#[test]
fn client_protocol_session_uses_explicit_protocol_tick_for_reliable_bookkeeping() {
    let arena_login = ArenaLoginRequest::new(8, 0, 1024, 768, 0xffff, "alpha", true);
    let mut session = ClientProtocolSession::new(true, arena_login.clone(), 0, 0);
    let login = PasswordLoginRequest::new(LoginProtocol::Continuum, false, "pilot", "", 0, 0, 0, 0);

    let _ = session.send_password_login(&login);
    assert_eq!(
        session.sequencer.sent[0].last_sent_tick, 0,
        "fresh sessions must not seed reliable state from client wall-clock time"
    );

    session.sequencer = ReliableSequencer::default();
    session.state = ClientProtocolState::Disconnected;
    session.set_outbound_protocol_tick(0x1122_3344);
    let _ = session.send_password_login(&login);
    assert_eq!(session.sequencer.sent[0].last_sent_tick, 0x1122_3344);

    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    response[1] = 0x00;
    let actions = session
        .handle_transport_packet_at_tick(&response, 0x5566_7788)
        .expect("accepted login response should queue arena login");

    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_reliable_message(
            1,
            &arena_login.encode_payload()
        )]
    );
    assert_eq!(session.sequencer.sent[1].last_sent_tick, 0x5566_7788);
}

#[test]
fn client_protocol_session_default_handlers_reuse_observed_server_tick() {
    let arena_login = ArenaLoginRequest::new(8, 0, 1024, 768, 0xffff, "alpha", true);
    let mut session = ClientProtocolSession::new(true, arena_login.clone(), 0, 0);

    session.set_outbound_protocol_tick(0x0102_0304);
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    response[1] = 0x00;
    let actions = session
        .handle_server_packet(&response)
        .expect("accepted login response should queue arena login");

    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_reliable_message(
            0,
            &arena_login.encode_payload()
        )]
    );
    assert_eq!(
        session.sequencer.sent[0].last_sent_tick, 0x0102_0304,
        "default packet handlers must use the session's server-owned protocol tick"
    );
}

#[test]
fn client_protocol_session_processes_time_sync_core_packets() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    let actions = session
        .handle_transport_packet_at_tick(&[0x00, 0x05, 0x44, 0x33, 0x22, 0x11], 0x5566_7788)
        .expect("time sync request should process");
    assert_eq!(
        actions.outbound_packets,
        vec![
            TimeSyncResponse {
                received_timestamp: 0x1122_3344,
                local_timestamp: 0,
                ..TimeSyncResponse::default()
            }
            .encode()
        ]
    );
    assert!(actions.events.is_empty());

    let actions = session
        .handle_transport_packet(&[0x00, 0x06, 0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55])
        .expect("time sync response should process");
    assert_eq!(actions.outbound_packets, Vec::<Vec<u8>>::new());
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::TimeSyncReceived(
            ScopedTimeSyncResponse {
                response: TimeSyncResponse {
                    received_timestamp: 0x1122_3344,
                    local_timestamp: 0x5566_7788,
                    ..TimeSyncResponse::default()
                },
                server_session_id: None,
                arena_stream_id: None,
            }
        )]
    );
}

#[test]
fn client_protocol_session_answers_time_sync_requests_with_observed_server_tick() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.latest_received_snapshot_server_tick = Some(0x1234_5678);

    let actions = session
        .handle_transport_packet_at_tick(&[0x00, 0x05, 0xef, 0xbe, 0xad, 0xde], 0x5566_7788)
        .expect("time sync request should process");

    assert_eq!(
        actions.outbound_packets,
        vec![
            TimeSyncResponse {
                received_timestamp: 0xdead_beef,
                local_timestamp: 0x1234_5678,
                ..TimeSyncResponse::default()
            }
            .encode()
        ]
    );
    assert!(actions.events.is_empty());
}

#[test]
fn client_protocol_session_records_server_clock_bootstrap() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let bootstrap = ServerClockBootstrap {
        server_session_id: 0x0102_0304_0506_0708,
        arena_stream_id: 0x1112_1314_1516_1718,
        server_tick: 0x1234_5678,
        input_sequence_base: 20,
        position_sequence_base: 20,
        weapon_sequence_base: 20,
        reliable_sequence_base: 0,
        tick_rate_hz: 60,
        snapshot_rate_hz: 30,
    };

    let actions = session
        .handle_server_packet(&bootstrap.encode())
        .expect("bootstrap should process");

    assert_eq!(session.latest_server_clock_bootstrap, Some(bootstrap));
    assert_eq!(
        session.latest_received_snapshot_server_tick,
        Some(bootstrap.server_tick)
    );
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::ServerClockBootstrapped(bootstrap)]
    );
}

#[test]
fn replacement_bootstrap_and_self_left_clear_protocol_identity_epoch_and_snapshot_state() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let old = ServerClockBootstrap {
        server_session_id: u64::MAX,
        arena_stream_id: u64::MAX,
        server_tick: 0x7fff_ff00,
        ..ServerClockBootstrap::default()
    };
    session
        .handle_server_packet(&old.encode())
        .expect("old bootstrap");
    session
        .handle_server_packet(&[ServerPacket::PLAYER_ID, 0xff, 0xff])
        .expect("old player id");
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .expect("old arena join");
    session.latest_received_snapshot_id = Some(u16::MAX);
    session
        .snapshot_entity_carry
        .insert(7, SnapshotEntityCarry::default());

    let replacement = ServerClockBootstrap {
        server_session_id: 1,
        arena_stream_id: 1,
        server_tick: 7,
        ..ServerClockBootstrap::default()
    };
    session
        .handle_server_packet(&replacement.encode())
        .expect("replacement bootstrap");
    assert_eq!(session.self_player_id, None);
    assert_eq!(session.snapshot_stream_epoch, 0);
    assert_eq!(session.latest_received_snapshot_id, None);
    assert!(session.snapshot_entity_carry.is_empty());
    assert_eq!(session.latest_received_snapshot_server_tick, Some(7));

    session
        .handle_server_packet(&[ServerPacket::PLAYER_ID, 0, 0])
        .expect("replacement player id");
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .expect("replacement arena join");
    assert_eq!(session.snapshot_stream_epoch, 1);
    session.latest_received_snapshot_id = Some(9);
    session
        .handle_server_packet(&[PlayerLeaving::TYPE, 0, 0])
        .expect("self leave");
    assert_eq!(session.self_player_id, None);
    assert_eq!(session.snapshot_stream_epoch, 0);
    assert_eq!(session.latest_received_snapshot_id, None);
}

#[test]
fn security_challenge_event_captures_exact_protocol_session_and_arena_generation() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let bootstrap = ServerClockBootstrap {
        server_session_id: 0x0102_0304_0506_0708,
        arena_stream_id: 0x1112_1314_1516_1718,
        server_tick: 1_000,
        input_sequence_base: 0,
        position_sequence_base: 0,
        weapon_sequence_base: 0,
        reliable_sequence_base: 0,
        tick_rate_hz: 60,
        snapshot_rate_hz: 30,
    };
    session
        .handle_server_packet(&bootstrap.encode())
        .expect("bootstrap");
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .expect("arena join");
    let challenge = SecurityChallenge {
        prize_seed: 1,
        door_seed: 2,
        timestamp: 1_001,
        checksum_key: 4,
    };
    let mut challenge_packet = vec![SecurityChallenge::TYPE];
    challenge_packet.extend_from_slice(&challenge.prize_seed.to_le_bytes());
    challenge_packet.extend_from_slice(&challenge.door_seed.to_le_bytes());
    challenge_packet.extend_from_slice(&challenge.timestamp.to_le_bytes());
    challenge_packet.extend_from_slice(&challenge.checksum_key.to_le_bytes());
    let actions = session
        .handle_server_packet(&challenge_packet)
        .expect("security challenge");
    assert!(
        actions
            .events
            .contains(&ClientProtocolEvent::SecurityChallengeReceived {
                challenge,
                server_session_id: Some(bootstrap.server_session_id),
                arena_epoch: 1,
            })
    );

    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .expect("same-session rejoin");
    let actions = session
        .handle_server_packet(&challenge_packet)
        .expect("challenge after rejoin");
    assert!(
        actions
            .events
            .contains(&ClientProtocolEvent::SecurityChallengeReceived {
                challenge,
                server_session_id: Some(bootstrap.server_session_id),
                arena_epoch: 2,
            })
    );
}

#[test]
fn client_protocol_session_handles_misc_reference_core_packets_without_failing() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.chunker.huge.push(b"partial").unwrap();

    let actions = session
        .handle_transport_packet(&[0x00, 0x0b])
        .expect("huge chunk cancel should process");
    assert_eq!(actions.outbound_packets, vec![vec![0x00, 0x0c]]);
    assert!(session.chunker.huge.data.is_empty());
    assert_eq!(session.chunker.huge.len, 0);
    assert!(actions.events.is_empty());

    let actions = session
        .handle_transport_packet(&[0x00, 0x0d])
        .expect("noop should process");
    assert!(actions.outbound_packets.is_empty());
    assert!(actions.events.is_empty());

    let actions = session
        .handle_transport_packet(&[0x00, 0x0f])
        .expect("unknown core extension should not disconnect");
    assert!(actions.outbound_packets.is_empty());
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::UnknownCorePacket(0x0f)]
    );

    let actions = session
        .handle_transport_packet(&[0x00, 0x07])
        .expect("disconnect should process");
    assert!(actions.outbound_packets.is_empty());
    assert_eq!(actions.events, vec![ClientProtocolEvent::Disconnected]);
    assert_eq!(session.state, ClientProtocolState::Disconnected);
}

#[test]
fn direct_unrelated_core_boundaries_abort_all_incomplete_chunk_assemblies() {
    let boundaries: &[&[u8]] = &[
        &[0x00, ReliableSequencer::RELIABLE_ACK_TYPE, 0, 0, 0, 0],
        &[0x00, TimeSyncResponse::REQUEST_TYPE, 1, 0, 0, 0],
        &[
            0x00,
            TimeSyncResponse::RESPONSE_TYPE,
            1,
            0,
            0,
            0,
            2,
            0,
            0,
            0,
        ],
        &[0x00, 0x07],
        &[0x00, PacketChunker::HUGE_CANCEL_TYPE],
        &[0x00, PacketChunker::HUGE_CANCEL_ACK_TYPE],
        &[0x00, 0x0d],
        &[0x00, 0x0f],
        &[0x00],
    ];

    for boundary in boundaries {
        let mut session =
            ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
        session
            .chunker
            .push_small_chunk(&[0x00, PacketChunker::SMALL_BODY_TYPE, b'a'])
            .expect("start small assembly");
        let _ = session.handle_transport_packet(boundary);
        assert_eq!(session.chunker.small.len, 0, "boundary {boundary:?}");
        assert_eq!(session.chunker.small.fragments, 0, "boundary {boundary:?}");
        assert_eq!(
            session.chunker.small.data.capacity(),
            0,
            "boundary {boundary:?}"
        );
        assert!(
            session
                .chunker
                .push_small_chunk(&[0x00, PacketChunker::SMALL_TAIL_TYPE, b'z'])
                .is_err(),
            "boundary {boundary:?} must orphan the old tail"
        );

        let mut session =
            ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
        let mut partial = vec![0x00, PacketChunker::HUGE_TYPE];
        partial.extend_from_slice(&5_u32.to_le_bytes());
        partial.extend_from_slice(b"ab");
        session
            .chunker
            .push_huge_chunk(&partial)
            .expect("start huge assembly");
        let _ = session.handle_transport_packet(boundary);
        assert_eq!(session.chunker.huge.len, 0, "boundary {boundary:?}");
        assert_eq!(session.chunker.huge.fragments, 0, "boundary {boundary:?}");
        assert_eq!(
            session.chunker.huge.data.capacity(),
            0,
            "boundary {boundary:?}"
        );
        assert_eq!(
            session.chunker.huge.target_len, None,
            "boundary {boundary:?}"
        );
    }
}

#[test]
fn malformed_reliable_cluster_and_receipt_boundaries_abort_small_and_huge_assemblies() {
    let boundaries = [
        (
            "truncated reliable envelope",
            vec![0x00, ReliableSequencer::RELIABLE_MESSAGE_TYPE, 0],
            true,
        ),
        (
            "malformed packet cluster",
            vec![0x00, PacketChunker::PACKET_CLUSTER_TYPE, 4, 0xff],
            true,
        ),
        (
            "malformed private application receipt",
            ReliableSequencer::encode_reliable_message(0, &[LifecycleBaselineAccepted::TYPE]),
            false,
        ),
    ];

    for (label, boundary, should_error) in boundaries {
        let mut session =
            ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
        session
            .chunker
            .push_small_chunk(&[0x00, PacketChunker::SMALL_BODY_TYPE, b'a'])
            .expect("start small assembly");
        assert_eq!(
            session.handle_transport_packet(&boundary).is_err(),
            should_error,
            "{label} result"
        );
        assert_eq!(session.chunker.small.len, 0, "{label}");
        assert_eq!(session.chunker.small.fragments, 0, "{label}");
        assert_eq!(session.chunker.small.data.capacity(), 0, "{label}");
        assert!(
            session
                .chunker
                .push_small_chunk(&[0x00, PacketChunker::SMALL_TAIL_TYPE, b'z'])
                .is_err(),
            "{label}: a later tail cannot complete the pre-boundary small assembly"
        );

        let mut session =
            ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
        let mut first = vec![0x00, PacketChunker::HUGE_TYPE];
        first.extend_from_slice(&3_u32.to_le_bytes());
        first.extend_from_slice(b"ab");
        session
            .chunker
            .push_huge_chunk(&first)
            .expect("start huge assembly");
        assert_eq!(
            session.handle_transport_packet(&boundary).is_err(),
            should_error,
            "{label} result"
        );
        assert_eq!(session.chunker.huge.len, 0, "{label}");
        assert_eq!(session.chunker.huge.fragments, 0, "{label}");
        assert_eq!(session.chunker.huge.data.capacity(), 0, "{label}");
        assert_eq!(session.chunker.huge.target_len, None, "{label}");

        let mut later = vec![0x00, PacketChunker::HUGE_TYPE];
        later.extend_from_slice(&3_u32.to_le_bytes());
        later.push(b'c');
        assert_eq!(
            session
                .chunker
                .push_huge_chunk(&later)
                .expect("later huge fragment starts a new assembly"),
            None,
            "{label}: a later fragment cannot complete the pre-boundary huge assembly"
        );
        assert_eq!(session.chunker.huge.len, 1, "{label}");
    }
}

#[test]
fn realtime_datachannel_packets_do_not_abort_the_reliable_file_chunk_stream() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let split = 7;
    let mut body = vec![PacketChunker::CORE_BYTE, PacketChunker::SMALL_BODY_TYPE];
    body.extend_from_slice(&response[..split]);
    let mut tail = vec![PacketChunker::CORE_BYTE, PacketChunker::SMALL_TAIL_TYPE];
    tail.extend_from_slice(&response[split..]);

    session
        .handle_transport_packet_from_lane_at_tick(
            &body,
            0,
            crate::transport::PacketTransportLane::Reliable,
        )
        .expect("reliable file body");
    assert_eq!(session.chunker.small.len, split);

    let realtime = [0xee, 1, 2, 3];
    let game_actions = session
        .handle_transport_packet_from_lane_at_tick(
            &realtime,
            0,
            crate::transport::PacketTransportLane::Game,
        )
        .expect("independent game-channel packet");
    assert_eq!(
        game_actions.events,
        vec![ClientProtocolEvent::UnknownPacket(0xee)]
    );
    let event_actions = session
        .handle_transport_packet_from_lane_at_tick(
            &realtime,
            0,
            crate::transport::PacketTransportLane::Event,
        )
        .expect("independent event-channel packet");
    assert_eq!(
        event_actions.events,
        vec![ClientProtocolEvent::UnknownPacket(0xee)]
    );
    assert_eq!(session.chunker.small.len, split);

    let completed = session
        .handle_transport_packet_from_lane_at_tick(
            &tail,
            0,
            crate::transport::PacketTransportLane::Reliable,
        )
        .expect("reliable file tail after cross-channel traffic");
    assert_eq!(
        completed.events,
        vec![ClientProtocolEvent::PasswordAccepted]
    );
    assert_eq!(session.chunker.small.data.capacity(), 0);
}

#[test]
fn client_protocol_session_tracks_registration_and_login_failure_states() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut response = [0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    response[1] = 0x01;

    let actions = session
        .handle_server_packet(&response)
        .expect("registration response should parse");
    assert_eq!(session.state, ClientProtocolState::RegistrationRequested);
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::RegistrationRequired]
    );

    response[1] = 0x02;
    let actions = session
        .handle_server_packet(&response)
        .expect("failed response should parse");
    assert_eq!(session.state, ClientProtocolState::Failed(0x02));
    assert_eq!(actions.events, vec![ClientProtocolEvent::LoginFailed(0x02)]);

    response[1] = 0x00;
    response[19] = 1;
    let actions = session
        .handle_server_packet(&response)
        .expect("registration form response should parse");
    assert_eq!(session.state, ClientProtocolState::RegistrationForm);
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::RegistrationFormRequested]
    );
}

#[test]
fn qa_settings_fingerprint_tracks_exact_valid_packets_without_challenge_keys() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    assert_eq!(session.arena_settings_crc32, None);
    let mut packet = sample_continuum_settings_packet();
    packet[0] = ServerPacket::ARENA_SETTINGS;
    session
        .handle_server_packet(&packet)
        .expect("valid settings");
    let first = session.arena_settings_crc32;
    assert_eq!(first, Some(continuum_file_crc32(&packet)));
    packet[128] ^= 1;
    session
        .handle_server_packet(&packet)
        .expect("replacement settings");
    assert_eq!(
        session.arena_settings_crc32,
        Some(continuum_file_crc32(&packet))
    );
    assert_ne!(session.arena_settings_crc32, first);
    let before_invalid = session.arena_settings_crc32;
    assert!(session.handle_server_packet(&packet[..8]).is_err());
    assert_eq!(session.arena_settings_crc32, before_invalid);
}

#[test]
fn client_protocol_session_queues_security_response_from_challenge() {
    let mut session = ClientProtocolSession::new(
        true,
        ArenaLoginRequest::spectator_default(),
        0x1122_3344,
        0x5566_7788,
    );
    session.set_security_context(
        0x0102_0304,
        0x99aa_bbcc,
        SecurityPacketStats {
            timer_drift: 7,
            ..SecurityPacketStats::default()
        },
    );
    let mut settings = sample_continuum_settings_packet();
    settings[0] = ServerPacket::ARENA_SETTINGS;
    let parsed_settings = ContinuumArenaSettings::parse(&settings).expect("settings should parse");
    assert_eq!(
        session
            .handle_server_packet(&settings)
            .expect("settings packet should parse")
            .events,
        vec![ClientProtocolEvent::ArenaSettingsUpdated(parsed_settings)]
    );
    assert!(session.take_prize_reliable);

    let checksum_key = 0xaabb_ccdd;
    let expected_settings_checksum =
        continuum_settings_checksum(checksum_key, &settings).expect("checksum should compute");
    let mut challenge = vec![SecurityChallenge::TYPE];
    challenge.extend_from_slice(&1_u32.to_le_bytes());
    challenge.extend_from_slice(&2_u32.to_le_bytes());
    challenge.extend_from_slice(&3_u32.to_le_bytes());
    challenge.extend_from_slice(&checksum_key.to_le_bytes());

    let actions = session
        .handle_server_packet(&challenge)
        .expect("challenge should queue security response");
    assert_eq!(
        actions.events,
        vec![
            ClientProtocolEvent::SecurityChallengeReceived {
                challenge: SecurityChallenge {
                    prize_seed: 1,
                    door_seed: 2,
                    timestamp: 3,
                    checksum_key,
                },
                server_session_id: None,
                arena_epoch: 0,
            },
            ClientProtocolEvent::SecurityResponseQueued,
        ]
    );
    assert_eq!(actions.outbound_packets.len(), 1);

    let packet = &actions.outbound_packets[0];
    assert_eq!(packet.len(), 46);
    assert_eq!(packet[0], SecurityPacket::TYPE);
    assert_eq!(read_u32(packet, 1).unwrap(), 0x0102_0304);
    assert_eq!(read_u32(packet, 5).unwrap(), expected_settings_checksum);
    assert_eq!(read_u32(packet, 9).unwrap(), 0x99aa_bbcc);
    assert_eq!(read_u32(packet, 13).unwrap(), 0x1122_3344);
    assert_eq!(read_u16(packet, 40).unwrap(), 7);
    assert_eq!(read_u32(packet, 42).unwrap(), 0x5566_7788);
}

#[test]
fn client_protocol_session_requests_and_decodes_downloaded_map() {
    let original = 0x0100_1001_u32.to_le_bytes();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&original)
        .expect("test zlib write should succeed");
    let compressed = encoder.finish().expect("test zlib finish should succeed");
    let checksum = continuum_file_crc32(&original);
    let mut map_info = vec![0; 25];
    map_info[0] = ServerPacket::MAP_INFORMATION;
    map_info[1..10].copy_from_slice(b"arena.lvl");
    put_u32(&mut map_info, 17, checksum);
    put_u32(&mut map_info, 21, compressed.len() as u32);
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    let actions = session
        .handle_server_packet(&map_info)
        .expect("map info should queue file request");
    let expected_request = FileRequestSpec {
        zone_id: None,
        filename: "arena.lvl".to_string(),
        index: 0,
        size: compressed.len() as u32,
        size_is_known: true,
        checksum,
        sha256: None,
        decompress: true,
    };
    assert_eq!(
        actions.events,
        vec![
            ClientProtocolEvent::ZoneFilesPending,
            ClientProtocolEvent::MapRequestQueued(expected_request.clone()),
        ]
    );
    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_reliable_message(
            0,
            &expected_request.encode_request_packet()
        )]
    );
    assert_eq!(
        session.pending_file_requests,
        VecDeque::from([expected_request])
    );

    let mut map_packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
    map_packet.extend_from_slice(&compressed);
    let actions = session
        .handle_server_packet(&map_packet)
        .expect("downloaded map should decode");
    assert_eq!(actions.events.len(), 3);
    let ClientProtocolEvent::MapDownloaded(download) = &actions.events[0] else {
        panic!("expected validated map download");
    };
    assert_eq!(download.filename(), "arena.lvl");
    assert_eq!(download.data(), original);
    assert_eq!(actions.events[1], ClientProtocolEvent::ZoneMapPublished);
    assert_eq!(actions.events[2], ClientProtocolEvent::ZoneFilesPublished);
    assert!(session.pending_file_requests.is_empty());
}

#[test]
fn repeated_valid_map_advertisements_replace_single_bounded_current_map_state() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    for coordinate in 1_u32..=12 {
        let level = ((coordinate << 12) | 1).to_le_bytes();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&level).expect("test map zlib input");
        let compressed = encoder.finish().expect("test map zlib output");
        let mut map_info = vec![0; 25];
        map_info[0] = ServerPacket::MAP_INFORMATION;
        map_info[1..10].copy_from_slice(b"arena.lvl");
        put_u32(&mut map_info, 17, continuum_file_crc32(&level));
        put_u32(&mut map_info, 21, compressed.len() as u32);
        session
            .handle_server_packet(&map_info)
            .expect("repeated map advertisement");

        let mut map_packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
        map_packet.extend_from_slice(&compressed);
        let actions = session
            .handle_server_packet(&map_packet)
            .expect("repeated valid map download");
        assert!(matches!(
            actions.events.as_slice(),
            [
                ClientProtocolEvent::MapDownloaded(_),
                ClientProtocolEvent::ZoneMapPublished,
                ClientProtocolEvent::ZoneFilesPublished,
            ]
        ));
        assert!(session.pending_file_requests.is_empty());
        assert!(session.pending_cached_map_install.is_none());
        assert_eq!(
            session.map_tiles.as_ref().map(|tiles| tiles.len()),
            Some(1024 * 1024),
            "the session retains only the replaceable current security tile grid"
        );
    }
}

#[test]
fn authenticated_cached_map_queues_check_instead_of_download_request() {
    let cached = Vec::new();
    let request = FileRequestSpec {
        zone_id: Some("chaos-zone".to_string()),
        filename: "cached.lvl".to_string(),
        index: 0,
        size: 8,
        size_is_known: true,
        checksum: continuum_file_crc32(&cached),
        sha256: Some(crate::zone_wire::sha256_digest(&cached)),
        decompress: true,
    };
    let mut session = ClientProtocolSession::new(
        true,
        ArenaLoginRequest::new(0, 0, 1024, 768, 0xffff, "chaos-zone", true),
        0,
        0,
    );
    let mut actions = ClientProtocolActions::default();

    assert!(session.accept_cached_map(&request, cached.clone(), &mut actions, 77));
    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_reliable_message(
            0,
            &CachedMapCheck {
                zone_id: "chaos-zone".to_string(),
                sha256: request.sha256.unwrap(),
            }
            .encode(),
        )]
    );
    assert_eq!(session.sequencer.sent[0].last_sent_tick, 77);
    assert!(actions.events.is_empty());
    let pending = session.pending_cached_map_install.as_ref().unwrap();
    assert_eq!(pending.filename(), request.filename);
    assert_eq!(pending.data(), cached);

    let coordinate_actions = session
        .handle_server_packet(&[SetCoordinates::TYPE, 8, 0, 9, 0])
        .expect("set-coordinates should release the cached map install");
    assert_eq!(coordinate_actions.events.len(), 3);
    let ClientProtocolEvent::MapDownloaded(download) = &coordinate_actions.events[0] else {
        panic!("expected cached validated map");
    };
    assert_eq!(download.filename(), request.filename);
    assert_eq!(download.data(), cached);
    assert_eq!(
        coordinate_actions.events[1],
        ClientProtocolEvent::ZoneMapPublished
    );
    assert_eq!(
        coordinate_actions.events[2],
        ClientProtocolEvent::SetCoordinates(SetCoordinates { x: 8, y: 9 })
    );
    assert!(session.pending_cached_map_install.is_none());
}

#[test]
fn spectator_cached_map_installs_at_real_arena_directory_boundary_without_coordinates() {
    let cached = Vec::new();
    let request = FileRequestSpec {
        zone_id: Some("chaos-zone".to_string()),
        filename: "cached.lvl".to_string(),
        index: 0,
        size: 8,
        size_is_known: true,
        checksum: continuum_file_crc32(&cached),
        sha256: Some(crate::zone_wire::sha256_digest(&cached)),
        decompress: true,
    };
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut accepted = ClientProtocolActions::default();
    assert!(session.accept_cached_map(&request, cached.clone(), &mut accepted, 77));

    // This is the real spectator bootstrap boundary emitted by the server:
    // MapInformation -> ArenaDirectory, with no SetCoordinates in between.
    let directory = [
        ArenaDirectoryListing::TYPE,
        b'C',
        b'h',
        b'a',
        b'o',
        b's',
        0,
        1,
        0,
    ];
    let actions = session
        .handle_server_packet(&directory)
        .expect("spectator arena directory");
    assert!(matches!(
        actions.events.first(),
        Some(ClientProtocolEvent::ArenaDirectory(_))
    ));
    let Some(ClientProtocolEvent::MapDownloaded(download)) = actions.events.get(1) else {
        panic!("expected spectator cached validated map");
    };
    assert_eq!(download.filename(), request.filename);
    assert_eq!(download.data(), cached);
    assert_eq!(
        actions.events.last(),
        Some(&ClientProtocolEvent::ZoneMapPublished)
    );
    assert!(session.pending_cached_map_install.is_none());
}

#[test]
fn cached_map_publication_uses_assigned_ship_even_when_login_requested_another_role() {
    let cached = Vec::new();
    let request = FileRequestSpec {
        zone_id: Some("chaos-zone".to_string()),
        filename: "cached.lvl".to_string(),
        index: 0,
        size: 8,
        size_is_known: true,
        checksum: continuum_file_crc32(&cached),
        sha256: Some(crate::zone_wire::sha256_digest(&cached)),
        decompress: true,
    };
    for (requested_ship, assigned_ship) in [(0, 8), (8, 0), (0, 0), (8, 8)] {
        let mut login = ArenaLoginRequest::spectator_default();
        login.ship = requested_ship;
        let mut session = ClientProtocolSession::new(true, login, 0, 0);
        session.handle_server_packet(&[0x01, 7, 0]).unwrap();
        let mut entry = vec![0; PlayerRosterEntry::PACKET_LEN];
        entry[0] = PlayerRosterEntry::TYPE;
        entry[1] = assigned_ship;
        entry[51] = 7;
        session.handle_server_packet(&entry).unwrap();
        let mut accepted = ClientProtocolActions::default();
        assert!(session.accept_cached_map(&request, cached.clone(), &mut accepted, 77));
        let directory = [ArenaDirectoryListing::TYPE, b'C', 0, 1, 0];
        let actions = session.handle_server_packet(&directory).unwrap();
        let maps = actions
            .events
            .iter()
            .filter(|event| matches!(event, ClientProtocolEvent::MapDownloaded(_)))
            .count();
        assert_eq!(
            maps,
            usize::from(assigned_ship == 8),
            "requested {requested_ship}, assigned {assigned_ship}"
        );
        if assigned_ship < 8 {
            assert!(session.pending_cached_map_install.is_some());
            let coordinates = session
                .handle_server_packet(&[SetCoordinates::TYPE, 8, 0, 9, 0])
                .unwrap();
            assert!(matches!(
                coordinates.events.first(),
                Some(ClientProtocolEvent::MapDownloaded(_))
            ));
            assert!(matches!(
                coordinates.events.last(),
                Some(ClientProtocolEvent::SetCoordinates(_))
            ));
        }
        assert!(session.pending_cached_map_install.is_none());
    }
}

#[test]
fn cached_map_with_wrong_checksum_fails_closed_without_opening_protocol_path() {
    let request = FileRequestSpec {
        zone_id: None,
        filename: "cached.lvl".to_string(),
        index: 0,
        size: 8,
        size_is_known: true,
        checksum: 1,
        sha256: None,
        decompress: true,
    };
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut actions = ClientProtocolActions::default();
    assert!(!session.accept_cached_map(&request, Vec::new(), &mut actions, 77));
    assert!(actions.outbound_packets.is_empty());
    assert!(actions.events.is_empty());
    assert!(session.pending_cached_map_install.is_none());
}

#[test]
fn malformed_download_with_valid_crc_and_sha_is_never_published_or_installed() {
    let corrupt = vec![0xaa, 0xbb, 0xcc];
    let request = FileRequestSpec {
        zone_id: Some("chaos-zone".to_string()),
        filename: "corrupt.lvl".to_string(),
        index: 0,
        size: corrupt.len() as u32,
        size_is_known: true,
        checksum: continuum_file_crc32(&corrupt),
        sha256: Some(crate::zone_wire::sha256_digest(&corrupt)),
        decompress: false,
    };
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.pending_file_requests.push_back(request);
    let mut packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
    packet.extend_from_slice(&corrupt);

    let error = session
        .handle_server_packet(&packet)
        .expect_err("digest-valid malformed LVL must fail before publication");
    assert!(error.contains("malformed"), "got: {error}");
    assert!(session.pending_cached_map_install.is_none());
    assert!(session.pending_file_requests.is_empty());
}

#[test]
fn rejected_completed_file_returns_to_the_queue_and_the_pump_re_requests_it() {
    let payload = vec![0x10, 0x20, 0x30, 0x40];
    let request = FileRequestSpec {
        zone_id: Some("chaos-zone".to_string()),
        filename: "stuck.lvl".to_string(),
        index: 3,
        size: payload.len() as u32,
        size_is_known: true,
        checksum: continuum_file_crc32(&payload),
        // The recorded digest no longer matches what the server delivers, so the completed file cannot be used. Every path
        // like this one used to consume the queued request and stop, which is how a client came to sit for tens of seconds
        // with no zone-file requests in flight at all and no refusal to show for it.
        sha256: Some([0x5c_u8; crate::zone_wire::SHA256_BYTES]),
        decompress: false,
    };
    let index = request.index;
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.pending_file_requests.push_back(request);
    session.file_request_in_flight = Some(index);
    let mut packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
    packet.extend_from_slice(&payload);

    let error = session.handle_server_packet(&packet).expect_err(
        "a completed file that fails its digest must still reach the player as an error",
    );
    assert!(error.contains("SHA-256"), "got: {error}");
    assert_eq!(session.file_response_rejects, 1);
    assert_eq!(
        session
            .pending_file_requests
            .front()
            .map(|pending| pending.index),
        Some(index),
        "the unusable file must go back on the queue rather than being consumed by its own failure"
    );
    assert_eq!(
        session.file_request_in_flight, None,
        "receiving the response releases the outstanding mark"
    );

    // The frame pump can re-issue the request without another inbound packet, and never more than one at a time.
    let expected = [
        FileRequestSpec::MAP_REQUEST_TYPE,
        index as u8,
        (index >> 8) as u8,
    ];
    let (_, retry) = session
        .take_file_request_retry()
        .expect("the pump must re-issue the requeued request");
    assert!(
        retry.ends_with(&expected[1..]),
        "the retry carries the same request payload, framed through the reliable sequencer"
    );
    assert_eq!(session.file_request_in_flight, Some(index));
    assert!(
        session.take_file_request_retry().is_none(),
        "a request already outstanding must not be sent twice"
    );
    session.cancel_file_request_retry();
    assert!(
        session.take_file_request_retry().is_some(),
        "a failed send gives the request back for a later frame"
    );
}
#[test]
fn an_unanswered_zone_file_request_is_re_issued_on_a_bounded_frame_budget() {
    let payload = vec![0x01, 0x02];
    let request = FileRequestSpec {
        zone_id: Some("chaos-zone".to_string()),
        filename: "lost.lvl".to_string(),
        index: 9,
        size: payload.len() as u32,
        size_is_known: true,
        checksum: continuum_file_crc32(&payload),
        sha256: Some(crate::zone_wire::sha256_digest(&payload)),
        decompress: false,
    };
    let index = request.index;
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.pending_file_requests.push_back(request);
    session.file_request_in_flight = Some(index);

    // Nothing answered the first request. The pump must not chatter on every frame...
    for _ in 0..89 {
        assert!(session.take_file_request_retry().is_none());
    }
    // ...but it must re-issue once the wait has clearly failed, and keep the same file outstanding.
    assert!(session.take_file_request_retry().is_some());
    assert_eq!(session.file_request_reissues, 1);
    assert_eq!(session.file_request_in_flight, Some(index));
    // And it must stop, rather than transmit forever for an asset that is genuinely not there.
    session.file_request_reissues = 4;
    for _ in 0..200 {
        assert!(
            session.take_file_request_retry().is_none(),
            "past the re-issue budget the client must go quiet"
        );
    }
}

#[test]
fn a_stalled_join_re_arms_the_file_request_retry_after_the_budget_is_spent() {
    // The budget above is what stops a genuinely missing asset from being asked for forever. It is also what made a
    // stalled join unrecoverable: once the re-issues were spent, nothing in the session ever asked again, so a client
    // waiting on arena files sat unready with no request in flight. The authority sequencer now reports that wait as
    // `Reattempt`, and re-arming is the caller's answer to it - ask once more rather than tear down a session the
    // server may be answering slowly.
    let payload = vec![0x01, 0x02];
    let request = FileRequestSpec {
        zone_id: Some("chaos-zone".to_string()),
        filename: "slow.lvl".to_string(),
        index: 9,
        size: payload.len() as u32,
        size_is_known: true,
        checksum: continuum_file_crc32(&payload),
        sha256: Some(crate::zone_wire::sha256_digest(&payload)),
        decompress: false,
    };
    let index = request.index;
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.pending_file_requests.push_back(request);
    session.file_request_in_flight = Some(index);
    session.file_request_reissues = 4;
    for _ in 0..200 {
        assert!(session.take_file_request_retry().is_none(), "budget spent");
    }

    session.rearm_file_request_retry();
    assert_eq!(session.file_request_reissues, 0);
    assert_eq!(session.file_request_in_flight, None);
    assert!(
        session.take_file_request_retry().is_some(),
        "re-arming must ask again rather than wait out another frame budget",
    );
    assert_eq!(session.file_request_in_flight, Some(index));
}

#[test]
fn digest_valid_lvz_with_header_only_png_is_never_published_or_retained() {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&13_u32.to_be_bytes());
    let mut ihdr = b"IHDR".to_vec();
    ihdr.extend_from_slice(&2_u32.to_be_bytes());
    ihdr.extend_from_slice(&2_u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png.extend_from_slice(&ihdr);
    png.extend_from_slice(&continuum_file_crc32(&ihdr).to_be_bytes());
    png.extend_from_slice(&0_u32.to_be_bytes());
    png.extend_from_slice(b"IEND");
    png.extend_from_slice(&continuum_file_crc32(b"IEND").to_be_bytes());

    let mut corrupt = Vec::new();
    corrupt.extend_from_slice(b"CONT");
    corrupt.extend_from_slice(&1_u32.to_le_bytes());
    corrupt.extend_from_slice(b"CONT");
    corrupt.extend_from_slice(&(png.len() as u32).to_le_bytes());
    corrupt.extend_from_slice(&1_u32.to_le_bytes());
    corrupt.extend_from_slice(&(png.len() as u32).to_le_bytes());
    corrupt.extend_from_slice(b"sprite.png\0");
    corrupt.extend_from_slice(&png);

    let request = FileRequestSpec {
        zone_id: Some("chaos-zone".to_string()),
        filename: "corrupt.lvz".to_string(),
        index: 1,
        size: corrupt.len() as u32,
        size_is_known: true,
        checksum: continuum_file_crc32(&corrupt),
        sha256: Some(crate::zone_wire::sha256_digest(&corrupt)),
        decompress: false,
    };
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.pending_file_requests.push_back(request);
    let mut packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
    packet.extend_from_slice(&corrupt);

    let error = session
        .handle_server_packet(&packet)
        .expect_err("digest-valid structurally corrupt image must fail before LVZ publication");
    assert!(error.contains("malformed"), "got: {error}");
    assert!(session.pending_file_requests.is_empty());
    assert_eq!(session.pending_zone_render_bytes, 0);
    assert_eq!(session.pending_zone_render_high_water_bytes, 0);
    assert!(session.pending_zone_render_images.is_empty());
}

#[test]
fn downloaded_map_security_grid_shares_the_parsed_tile_allocation() {
    let parsed = parse_downloaded_file("shared.lvl", &[]).unwrap();
    let DownloadedFile::Level(map) = parsed else {
        panic!("expected level");
    };
    let shared = map.tiles.shared();
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.set_map_security_shared_tiles(&map.tiles, map.width, map.height, map.continuum_crc32());
    assert!(Arc::ptr_eq(
        &shared,
        session.map_tiles.as_ref().expect("security tile grid")
    ));
}

#[test]
fn client_protocol_session_uses_downloaded_map_for_security_response() {
    let original = Vec::new();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&original)
        .expect("test zlib write should succeed");
    let compressed = encoder.finish().expect("test zlib finish should succeed");
    let checksum = continuum_file_crc32(&original);
    let mut map_info = vec![0; 25];
    map_info[0] = ServerPacket::MAP_INFORMATION;
    map_info[1..10].copy_from_slice(b"arena.lvl");
    put_u32(&mut map_info, 17, checksum);
    put_u32(&mut map_info, 21, compressed.len() as u32);
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    session
        .handle_server_packet(&map_info)
        .expect("map info should queue request");
    let mut map_packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
    map_packet.extend_from_slice(&compressed);
    session
        .handle_server_packet(&map_packet)
        .expect("downloaded map should decode and update security map");

    let checksum_key = 63_u32;
    let mut challenge = vec![SecurityChallenge::TYPE];
    challenge.extend_from_slice(&1_u32.to_le_bytes());
    challenge.extend_from_slice(&2_u32.to_le_bytes());
    challenge.extend_from_slice(&3_u32.to_le_bytes());
    challenge.extend_from_slice(&checksum_key.to_le_bytes());
    let actions = session
        .handle_server_packet(&challenge)
        .expect("challenge should queue security response");

    assert_eq!(
        actions.events,
        vec![
            ClientProtocolEvent::SecurityChallengeReceived {
                challenge: SecurityChallenge {
                    prize_seed: 1,
                    door_seed: 2,
                    timestamp: 3,
                    checksum_key,
                },
                server_session_id: None,
                arena_epoch: 0,
            },
            ClientProtocolEvent::SecurityResponseQueued,
        ]
    );
    assert_eq!(actions.outbound_packets.len(), 1);
    assert_eq!(actions.outbound_packets[0][0], SecurityPacket::TYPE);
    assert_eq!(
        read_u32(&actions.outbound_packets[0], 13).unwrap(),
        checksum_key
    );
}

#[test]
fn client_protocol_session_queues_and_decodes_lvz_downloads_after_map() {
    let map_bytes = 0x0100_1001_u32.to_le_bytes();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&map_bytes)
        .expect("test zlib write should succeed");
    let compressed_map = encoder.finish().expect("test zlib finish should succeed");
    let lvz_bytes = b"CONT\0\0\0\0";
    let mut map_info = vec![0; 1 + 24 + 24];
    map_info[0] = ServerPacket::MAP_INFORMATION;
    map_info[1..10].copy_from_slice(b"arena.lvl");
    put_u32(&mut map_info, 17, continuum_file_crc32(&map_bytes));
    put_u32(&mut map_info, 21, compressed_map.len() as u32);
    map_info[25..33].copy_from_slice(b"zone.lvz");
    put_u32(&mut map_info, 41, continuum_file_crc32(lvz_bytes));
    put_u32(&mut map_info, 45, lvz_bytes.len() as u32);
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    let actions = session
        .handle_server_packet(&map_info)
        .expect("map info should queue map and lvz requests");
    let expected_map = FileRequestSpec {
        zone_id: None,
        filename: "arena.lvl".to_string(),
        index: 0,
        size: compressed_map.len() as u32,
        size_is_known: true,
        checksum: continuum_file_crc32(&map_bytes),
        sha256: None,
        decompress: true,
    };
    let expected_lvz = FileRequestSpec {
        zone_id: None,
        filename: "zone.lvz".to_string(),
        index: 1,
        size: lvz_bytes.len() as u32,
        size_is_known: true,
        checksum: continuum_file_crc32(lvz_bytes),
        sha256: None,
        decompress: false,
    };

    assert_eq!(
        actions.events,
        vec![
            ClientProtocolEvent::ZoneFilesPending,
            ClientProtocolEvent::MapRequestQueued(expected_map.clone()),
        ]
    );
    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_reliable_message(
            0,
            &expected_map.encode_request_packet()
        )]
    );
    assert_eq!(
        session.pending_file_requests,
        VecDeque::from([expected_map.clone(), expected_lvz.clone()])
    );

    let mut map_packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
    map_packet.extend_from_slice(&compressed_map);
    let actions = session
        .handle_server_packet(&map_packet)
        .expect("map download should decode");
    assert_eq!(actions.events.len(), 3);
    let ClientProtocolEvent::MapDownloaded(map_download) = &actions.events[0] else {
        panic!("expected validated map before the withheld LVZ tail");
    };
    assert_eq!(map_download.filename(), "arena.lvl");
    assert_eq!(map_download.data(), map_bytes);
    assert_eq!(actions.events[1], ClientProtocolEvent::ZoneMapPublished);
    assert_eq!(
        actions.events[2],
        ClientProtocolEvent::MapRequestQueued(expected_lvz.clone())
    );
    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_reliable_message(
            1,
            &expected_lvz.encode_request_packet()
        )]
    );

    let mut lvz_packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
    lvz_packet.extend_from_slice(lvz_bytes);
    let actions = session
        .handle_server_packet(&lvz_packet)
        .expect("lvz download should decode without zlib");
    assert_eq!(actions.events.len(), 2);
    let ClientProtocolEvent::MapDownloaded(lvz_download) = &actions.events[0] else {
        panic!("expected validated LVZ only after the archive tail completes");
    };
    assert_eq!(lvz_download.filename(), "zone.lvz");
    assert_eq!(lvz_download.data(), lvz_bytes);
    assert_eq!(actions.events[1], ClientProtocolEvent::ZoneFilesPublished);
    assert!(actions.outbound_packets.is_empty());
    assert!(session.pending_file_requests.is_empty());
}

fn highly_compressed_lvz(decoded_bytes: usize, marker: u8) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    let mut source = vec![0_u8; decoded_bytes];
    if let Some(first) = source.first_mut() {
        *first = marker;
    }
    encoder.write_all(&source).expect("compress LVZ source");
    let compressed = encoder.finish().expect("finish LVZ source");
    let filename = format!("payload-{marker}.bin");
    let mut archive = Vec::with_capacity(8 + 16 + filename.len() + 1 + compressed.len());
    archive.extend_from_slice(b"CONT");
    archive.extend_from_slice(&1_u32.to_le_bytes());
    archive.extend_from_slice(b"CONT");
    archive.extend_from_slice(&(decoded_bytes as u32).to_le_bytes());
    archive.extend_from_slice(&1_u32.to_le_bytes());
    archive.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    archive.extend_from_slice(filename.as_bytes());
    archive.push(0);
    archive.extend_from_slice(&compressed);
    archive
}

fn authenticated_map_info_with_lvzs(
    zone_id: &str,
    map: &[u8],
    compressed_map: &[u8],
    lvzs: &[(String, Vec<u8>)],
) -> Vec<u8> {
    let mut packet = vec![
        0;
        1 + MapInformation::AUTHENTICATED_HEADER_LEN
            + lvzs.len() * MapInformation::AUTHENTICATED_FILE_LEN
    ];
    packet[0] = ServerPacket::MAP_INFORMATION;
    packet[1..5].copy_from_slice(&crate::zone_wire::AUTHENTICATED_MAP_INFO_MAGIC);
    packet[5] = crate::zone_wire::AUTHENTICATED_MAP_INFO_VERSION;
    packet[6..6 + zone_id.len()].copy_from_slice(zone_id.as_bytes());
    packet[22..30].copy_from_slice(b"tiny.lvl");
    put_u32(&mut packet, 38, continuum_file_crc32(map));
    put_u32(&mut packet, 42, compressed_map.len() as u32);
    packet[46..78].copy_from_slice(&crate::zone_wire::sha256_digest(map));
    packet[78..80].copy_from_slice(&((1 + lvzs.len()) as u16).to_le_bytes());
    for (index, (filename, bytes)) in lvzs.iter().enumerate() {
        let offset = 80 + index * MapInformation::AUTHENTICATED_FILE_LEN;
        packet[offset..offset + filename.len()].copy_from_slice(filename.as_bytes());
        put_u32(&mut packet, offset + 16, continuum_file_crc32(bytes));
        put_u32(&mut packet, offset + 20, bytes.len() as u32);
        packet[offset + 24..offset + 56].copy_from_slice(&crate::zone_wire::sha256_digest(bytes));
    }
    packet
}

fn compressed_map_packet(bytes: &[u8]) -> Vec<u8> {
    let mut packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
    packet.extend_from_slice(bytes);
    packet
}

#[test]
fn prepared_images_use_manifest_high_water_even_when_a_later_texture_is_smaller() {
    use image::ImageEncoder;
    let archive = |size: u32| {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                &vec![0; (size * size * 4) as usize],
                size,
                size,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        let mut bytes = b"CONT".to_vec();
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(b"CONT");
        bytes.extend_from_slice(&(png.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&(png.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"same.png\0");
        bytes.extend_from_slice(&png);
        bytes
    };
    let files = [
        ("large.lvz".to_string(), archive(128)),
        ("small.lvz".to_string(), archive(1)),
    ];
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&[]).unwrap();
    let map = encoder.finish().unwrap();
    let mut session = ClientProtocolSession::new(
        true,
        ArenaLoginRequest::new(0, 0, 1024, 768, 0xffff, "peak", true),
        0,
        0,
    );
    session
        .handle_server_packet(&authenticated_map_info_with_lvzs("peak", &[], &map, &files))
        .unwrap();
    session
        .handle_server_packet(&compressed_map_packet(&map))
        .unwrap();
    session
        .handle_server_packet(&compressed_map_packet(&files[0].1))
        .unwrap();
    let large_prefix = session.pending_zone_render_bytes;
    assert_eq!(session.pending_zone_render_high_water_bytes, large_prefix);
    let actions = session
        .handle_server_packet(&compressed_map_packet(&files[1].1))
        .unwrap();
    assert!(session.pending_zone_render_bytes < large_prefix);
    assert_eq!(session.pending_zone_render_high_water_bytes, large_prefix);
    let mut prepared_count = 0;
    for event in actions.events {
        if let ClientProtocolEvent::MapDownloaded(file) = event
            && let Some(DownloadedFile::Lvz(archive)) = file.take_parsed()
        {
            archive.require_full_validation_seal().unwrap();
            assert!(archive.take_prepared_image(0).is_some());
            prepared_count += 1;
        }
    }
    assert_eq!(prepared_count, 2);
    session
        .handle_server_packet(&authenticated_map_info_with_lvzs("peak", &[], &map, &[]))
        .unwrap();
    assert_eq!(session.pending_zone_render_high_water_bytes, 0);
}

#[test]
fn client_lvz_decoded_aggregate_accepts_exact_cap_after_reordered_delivery_and_cleans_on_lifecycle()
{
    let archive_bytes = crate::lvz::LvzArchive::MAX_ARCHIVE_DECODED_BYTES;
    assert_eq!(
        archive_bytes * 4,
        crate::zone_wire::MAX_ZONE_DECODED_LVZ_BYTES
    );
    let lvzs = (0_u8..4)
        .rev()
        .map(|marker| {
            (
                format!("z{marker}.lvz"),
                highly_compressed_lvz(archive_bytes, marker),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        lvzs.iter()
            .all(|(_, bytes)| { bytes.len() < crate::zone_wire::MAX_TRANSFER_FILE_BYTES / 8 })
    );
    let map = Vec::new();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&map).unwrap();
    let compressed_map = encoder.finish().unwrap();
    let map_info = authenticated_map_info_with_lvzs("aggregate", &map, &compressed_map, &lvzs);
    let mut session = ClientProtocolSession::new(
        true,
        ArenaLoginRequest::new(0, 0, 1024, 768, 0xffff, "aggregate", true),
        0,
        0,
    );
    session.handle_server_packet(&map_info).unwrap();
    session
        .handle_server_packet(&compressed_map_packet(&compressed_map))
        .expect("map before exact LVZ aggregate");

    let reliable = lvzs
        .iter()
        .enumerate()
        .map(|(id, (_, bytes))| {
            ReliableSequencer::encode_reliable_message(id as u32, &compressed_map_packet(bytes))
        })
        .collect::<Vec<_>>();
    for id in [3_usize, 1, 2] {
        let actions = session
            .handle_transport_packet(&reliable[id])
            .expect("out-of-order archive queues");
        assert!(actions.events.is_empty());
    }
    let actions = session
        .handle_transport_packet(&reliable[0])
        .expect("missing reliable prefix releases exact aggregate");
    assert_eq!(
        actions
            .events
            .iter()
            .filter(|event| matches!(event, ClientProtocolEvent::MapDownloaded(download) if download.filename().ends_with(".lvz")))
            .count(),
        4
    );
    assert_eq!(
        actions.events.last(),
        Some(&ClientProtocolEvent::ZoneFilesPublished)
    );
    assert_eq!(
        session.pending_zone_lvz_usage.decoded_source_bytes,
        crate::zone_wire::MAX_ZONE_DECODED_LVZ_BYTES
    );
    assert!(session.pending_zone_downloads.is_empty());
    assert!(session.pending_zone_map_download.is_none());
    assert!(session.pending_zone_map_cache_identity.is_none());
    assert!(!session.pending_zone_map_needs_cache_store);
    assert!(!session.pending_zone_publication);

    let replacement = authenticated_map_info_with_lvzs("aggregate", &map, &compressed_map, &[]);
    session
        .handle_server_packet(&replacement)
        .expect("next lifecycle clears prior zone accounting");
    assert_eq!(
        session.pending_zone_lvz_usage,
        crate::lvz::LvzRenderUsage::default()
    );
    assert!(session.pending_zone_downloads.is_empty());
    assert!(session.pending_zone_render_images.is_empty());
    assert_eq!(session.pending_zone_render_bytes, 0);
    assert_eq!(session.pending_zone_pixel_bytes, 0);
}

#[test]
fn client_lvz_decoded_aggregate_rejects_cap_plus_one_atomically_on_cached_map_reorder() {
    let archive_bytes = crate::lvz::LvzArchive::MAX_ARCHIVE_DECODED_BYTES;
    let mut lvzs = (0_u8..4)
        .rev()
        .map(|marker| {
            (
                format!("z{marker}.lvz"),
                highly_compressed_lvz(archive_bytes, marker),
            )
        })
        .collect::<Vec<_>>();
    lvzs.push(("a4.lvz".to_string(), highly_compressed_lvz(1, 4)));
    let map = Vec::new();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&map).unwrap();
    let compressed_map = encoder.finish().unwrap();
    let map_info = authenticated_map_info_with_lvzs("aggregate", &map, &compressed_map, &lvzs);
    let mut session = ClientProtocolSession::new(
        true,
        ArenaLoginRequest::new(0, 0, 1024, 768, 0xffff, "aggregate", true),
        0,
        0,
    );
    session.handle_server_packet(&map_info).unwrap();
    let cached_request = session
        .pending_file_requests
        .pop_front()
        .expect("authenticated map request");
    let mut cache_actions = ClientProtocolActions::default();
    assert!(session.accept_cached_map(&cached_request, map, &mut cache_actions, 77));
    assert!(cache_actions.events.is_empty());
    assert!(session.pending_cached_map_install.is_some());

    let reliable = lvzs
        .iter()
        .enumerate()
        .map(|(id, (_, bytes))| {
            ReliableSequencer::encode_reliable_message(id as u32, &compressed_map_packet(bytes))
        })
        .collect::<Vec<_>>();
    for id in [4_usize, 2, 3, 1] {
        let actions = session
            .handle_transport_packet(&reliable[id])
            .expect("out-of-order archive queues");
        assert!(actions.events.is_empty());
    }
    let error = session
        .handle_transport_packet(&reliable[0])
        .expect_err("64 MiB plus one decoded byte must reject");
    assert!(error.contains("67108865 bytes; maximum is 67108864"));

    assert!(session.pending_file_requests.is_empty());
    assert!(session.pending_cached_map_install.is_none());
    assert!(session.pending_zone_map_download.is_none());
    assert!(session.pending_zone_map_cache_identity.is_none());
    assert!(!session.pending_zone_map_needs_cache_store);
    assert!(session.pending_zone_downloads.is_empty());
    assert!(!session.pending_zone_publication);
    assert_eq!(session.pending_zone_render_bytes, 0);
    assert_eq!(session.pending_zone_pixel_bytes, 0);
    assert!(session.pending_zone_render_images.is_empty());
    assert_eq!(
        session.pending_zone_lvz_usage,
        crate::lvz::LvzRenderUsage::default()
    );
    assert!(session.map_tiles.is_none());
    assert_eq!(session.map_width, 0);
    assert_eq!(session.map_height, 0);
    assert_eq!(session.map_security_checksum, 0);
    assert_eq!(session.continuum_map_crc32, 0);
    assert!(session.sequencer.sent.iter().all(|sent| !matches!(
        sent.payload.first().copied(),
        Some(FileRequestSpec::MAP_REQUEST_TYPE) | Some(CachedMapCheck::TYPE)
    )));
}

#[test]
fn client_protocol_session_completes_the_exact_chaos_zone_manifest() {
    const ZONE_ID: &str = "chaos-zone";
    const MAP_FILENAME: &str = "bdhadar.lvl";
    const MAP_BYTES: &[u8] = include_bytes!("../../assets/zones/chaos-zone/bdhadar.lvl");
    const LVZ_FILES: &[(&str, &[u8])] = &[
        (
            "aiq.lvz",
            include_bytes!("../../assets/zones/chaos-zone/aiq.lvz"),
        ),
        (
            "bonus.lvz",
            include_bytes!("../../assets/zones/chaos-zone/bonus.lvz"),
        ),
        (
            "cpwavs.lvz",
            include_bytes!("../../assets/zones/chaos-zone/cpwavs.lvz"),
        ),
        (
            "crw1a.lvz",
            include_bytes!("../../assets/zones/chaos-zone/crw1a.lvz"),
        ),
        (
            "ctrw1b.lvz",
            include_bytes!("../../assets/zones/chaos-zone/ctrw1b.lvz"),
        ),
        (
            "cts1a.lvz",
            include_bytes!("../../assets/zones/chaos-zone/cts1a.lvz"),
        ),
        (
            "czi.lvz",
            include_bytes!("../../assets/zones/chaos-zone/czi.lvz"),
        ),
        (
            "g.lvz",
            include_bytes!("../../assets/zones/chaos-zone/g.lvz"),
        ),
        (
            "newb2a.lvz",
            include_bytes!("../../assets/zones/chaos-zone/newb2a.lvz"),
        ),
        (
            "rnk.lvz",
            include_bytes!("../../assets/zones/chaos-zone/rnk.lvz"),
        ),
        (
            "teamwavs.lvz",
            include_bytes!("../../assets/zones/chaos-zone/teamwavs.lvz"),
        ),
        (
            "urkills.lvz",
            include_bytes!("../../assets/zones/chaos-zone/urkills.lvz"),
        ),
        (
            "zmoc.lvz",
            include_bytes!("../../assets/zones/chaos-zone/zmoc.lvz"),
        ),
        (
            "zmoct.lvz",
            include_bytes!("../../assets/zones/chaos-zone/zmoct.lvz"),
        ),
    ];

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(MAP_BYTES).unwrap();
    let compressed_map = encoder.finish().unwrap();
    let mut map_info = vec![
        0;
        1 + MapInformation::AUTHENTICATED_HEADER_LEN
            + LVZ_FILES.len() * MapInformation::AUTHENTICATED_FILE_LEN
    ];
    map_info[0] = ServerPacket::MAP_INFORMATION;
    map_info[1..5].copy_from_slice(&crate::zone_wire::AUTHENTICATED_MAP_INFO_MAGIC);
    map_info[5] = crate::zone_wire::AUTHENTICATED_MAP_INFO_VERSION;
    map_info[6..6 + ZONE_ID.len()].copy_from_slice(ZONE_ID.as_bytes());
    map_info[22..22 + MAP_FILENAME.len()].copy_from_slice(MAP_FILENAME.as_bytes());
    put_u32(&mut map_info, 38, continuum_file_crc32(MAP_BYTES));
    put_u32(&mut map_info, 42, compressed_map.len() as u32);
    map_info[46..78].copy_from_slice(&crate::zone_wire::sha256_digest(MAP_BYTES));
    map_info[78..80].copy_from_slice(&((1 + LVZ_FILES.len()) as u16).to_le_bytes());
    for (index, (filename, bytes)) in LVZ_FILES.iter().enumerate() {
        let offset = 80 + index * MapInformation::AUTHENTICATED_FILE_LEN;
        map_info[offset..offset + filename.len()].copy_from_slice(filename.as_bytes());
        put_u32(&mut map_info, offset + 16, continuum_file_crc32(bytes));
        put_u32(&mut map_info, offset + 20, bytes.len() as u32);
        map_info[offset + 24..offset + 56].copy_from_slice(&crate::zone_wire::sha256_digest(bytes));
    }

    let mut session = ClientProtocolSession::new(
        true,
        ArenaLoginRequest::new(0, 0, 1024, 768, 0xffff, ZONE_ID, true),
        0,
        0,
    );
    let first = session.handle_server_packet(&map_info).unwrap();
    assert!(matches!(
        first.events.last(),
        Some(ClientProtocolEvent::MapRequestQueued(request)) if request.index == 0
    ));

    let mut map_packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
    map_packet.extend_from_slice(&compressed_map);
    session
        .handle_server_packet(&map_packet)
        .expect("exact Chaos map download");
    for (index, (filename, bytes)) in LVZ_FILES.iter().enumerate() {
        let mut packet = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
        packet.extend_from_slice(bytes);
        session.handle_server_packet(&packet).unwrap_or_else(|err| {
            panic!("exact Chaos manifest rejected file {index} {filename:?}: {err}")
        });
    }
    assert!(session.pending_file_requests.is_empty());
    assert!(!session.pending_zone_publication);
}

#[test]
fn client_protocol_session_processes_reliable_transport_packets() {
    let arena_login = ArenaLoginRequest::new(8, 0, 1024, 768, 0xffff, "alpha", true);
    let mut session = ClientProtocolSession::new(true, arena_login.clone(), 0, 0);
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let reliable = ReliableSequencer::encode_reliable_message(0, &response);

    let actions = session
        .handle_transport_packet(&reliable)
        .expect("reliable server packet should process");

    assert_eq!(session.state, ClientProtocolState::ArenaLoginSent);
    assert_eq!(actions.events, vec![ClientProtocolEvent::PasswordAccepted]);
    assert_eq!(
        actions.outbound_packets,
        vec![
            ReliableSequencer::encode_ack(0),
            ReliableSequencer::encode_reliable_message(0, &arena_login.encode_payload()),
        ]
    );
}

#[test]
fn client_protocol_session_acks_duplicate_reliable_without_reprocessing() {
    let arena_login = ArenaLoginRequest::new(8, 0, 1024, 768, 0xffff, "alpha", true);
    let mut session = ClientProtocolSession::new(true, arena_login.clone(), 0, 0);
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let reliable = ReliableSequencer::encode_reliable_message(0, &response);

    let first = session
        .handle_transport_packet(&reliable)
        .expect("first reliable server packet should process");
    assert_eq!(first.events, vec![ClientProtocolEvent::PasswordAccepted]);
    assert_eq!(
        first.outbound_packets,
        vec![
            ReliableSequencer::encode_ack(0),
            ReliableSequencer::encode_reliable_message(0, &arena_login.encode_payload()),
        ]
    );

    let duplicate = session
        .handle_transport_packet(&reliable)
        .expect("duplicate reliable packet should still ack");
    assert_eq!(
        duplicate.outbound_packets,
        vec![ReliableSequencer::encode_ack(0)]
    );
    assert!(duplicate.events.is_empty());
    assert_eq!(session.state, ClientProtocolState::ArenaLoginSent);
}

#[test]
fn client_protocol_session_processes_reliable_core_payloads() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let reliable =
        ReliableSequencer::encode_reliable_message(0, &[0x00, 0x05, 0x44, 0x33, 0x22, 0x11]);

    let actions = session
        .handle_transport_packet_at_tick(&reliable, 0x5566_7788)
        .expect("reliable core payload should process");

    assert_eq!(
        actions.outbound_packets,
        vec![
            ReliableSequencer::encode_ack(0),
            TimeSyncResponse {
                received_timestamp: 0x1122_3344,
                local_timestamp: 0,
                ..TimeSyncResponse::default()
            }
            .encode(),
        ]
    );
    assert!(actions.events.is_empty());
}

#[test]
fn client_protocol_session_processes_reliable_acks() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let login = PasswordLoginRequest::new(LoginProtocol::Continuum, false, "pilot", "", 0, 0, 0, 0);
    session.send_password_login(&login);
    assert_eq!(session.sequencer.sent.len(), 1);

    let actions = session
        .handle_transport_packet(&ReliableSequencer::encode_ack(0))
        .expect("ack should process");

    assert!(actions.outbound_packets.is_empty());
    assert!(actions.events.is_empty());
    assert!(session.sequencer.sent.is_empty());
}

#[test]
fn reliable_sequencer_resends_unacked_packets_after_timeout() {
    let mut sequencer = ReliableSequencer::default();
    let payload = b"hello";
    let sent_at = 10_000;

    assert_eq!(
        sequencer.send_reliable_at_tick(payload, sent_at),
        ReliableSequencer::encode_reliable_message(0, payload)
    );
    assert_eq!(sequencer.sent.len(), 1);

    assert!(
        sequencer
            .encode_timed_out_reliable_at_tick(
                sent_at + ReliableSequencer::RESEND_DELAY_TICKS as u32 - 1
            )
            .is_empty()
    );

    assert_eq!(
        sequencer.encode_timed_out_reliable_at_tick(
            sent_at + ReliableSequencer::RESEND_DELAY_TICKS as u32
        ),
        vec![ReliableSequencer::encode_reliable_message(0, payload)]
    );
    assert!(
        sequencer
            .encode_timed_out_reliable_at_tick(
                sent_at + ReliableSequencer::RESEND_DELAY_TICKS as u32 + 1
            )
            .is_empty()
    );

    assert!(
        sequencer
            .receive_ack(&ReliableSequencer::encode_ack(0))
            .expect("ack should parse")
    );
    assert!(
        sequencer
            .encode_timed_out_reliable_at_tick(
                sent_at + ReliableSequencer::RESEND_DELAY_TICKS as u32 * 2
            )
            .is_empty()
    );
}

#[test]
fn exact_application_receipt_crosses_full_reliable_gap_without_acknowledging_drop() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let unknown = [0xee];
    for id in 1..=ReliableSequencer::MAX_QUEUED_RELIABLES as u32 {
        let actions = session
            .handle_transport_packet(&ReliableSequencer::encode_reliable_message(id, &unknown))
            .expect("future reliable payload should queue");
        assert_eq!(
            actions.outbound_packets,
            vec![ReliableSequencer::encode_ack(id)]
        );
        assert!(actions.events.is_empty());
    }
    assert_eq!(
        session.sequencer.queued.len(),
        ReliableSequencer::MAX_QUEUED_RELIABLES
    );

    let overflow_id = ReliableSequencer::MAX_QUEUED_RELIABLES as u32 + 1;
    let receipt = LifecycleBaselineAccepted {
        client_reliable_delivery_id: u32::MAX,
        life_seq: 9,
        position_sequence: 0,
        accepted_server_tick: 0x7fff_fffe,
        accepted_position: Some((8104, 8312)),
    };
    let mut receipt_payload = vec![
        LifecycleBaselineAccepted::TYPE,
        LifecycleBaselineAccepted::VERSION,
    ];
    receipt_payload.extend(receipt.client_reliable_delivery_id.to_le_bytes());
    receipt_payload.extend(receipt.life_seq.to_le_bytes());
    receipt_payload.extend(receipt.position_sequence.to_le_bytes());
    receipt_payload.extend(receipt.accepted_server_tick.to_le_bytes());
    receipt_payload.extend(8104_u16.to_le_bytes());
    receipt_payload.extend(8312_u16.to_le_bytes());
    let actions = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(
            overflow_id,
            &receipt_payload,
        ))
        .expect("priority receipt should parse while reorder queue is full");
    assert!(
        actions.outbound_packets.is_empty(),
        "unretained receipt wrapper stays unacknowledged for retry"
    );
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::LifecycleBaselineAccepted(receipt)],
        "the exact application receipt is safe to apply across an unrelated prefix gap"
    );

    let actions = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(0, &unknown))
        .expect("gap payload should drain retained prefix");
    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_ack(0)]
    );
    assert_eq!(session.sequencer.next_process_id, overflow_id);

    let actions = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(
            overflow_id,
            &receipt_payload,
        ))
        .expect("receipt retry should enter the ordered stream");
    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_ack(overflow_id)]
    );
    assert_eq!(
        actions.events,
        Vec::<ClientProtocolEvent>::new(),
        "ordered retry must not emit the already priority-dispatched receipt twice"
    );
    let duplicate = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(
            overflow_id,
            &receipt_payload,
        ))
        .expect("retired receipt duplicate should remain transport-idempotent");
    assert_eq!(
        duplicate.outbound_packets,
        vec![ReliableSequencer::encode_ack(overflow_id)]
    );
    assert!(duplicate.events.is_empty());
    assert!(session.priority_dispatched_reliable_ids.is_empty());
}

#[test]
fn prize_effect_acceptance_crosses_loss_reorder_once_and_keeps_exact_wrapped_identity() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let receipt = PrizeEffectAccepted {
        server_session_id: 77,
        player_id: 3,
        life_seq: 9,
        collectible_id: u32::MAX,
        prize_id: -14,
        prize_state_revision: u32::MAX,
        requested_application_sequence: 0,
        application_sequence: 0,
        outcome: PrizeEffectAccepted::OUTCOME_APPLIED,
    };
    let payload = receipt.encode();
    let future_id = 2;
    let first = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(
            future_id, &payload,
        ))
        .expect("future prize terminal");
    assert_eq!(
        first.outbound_packets,
        vec![ReliableSequencer::encode_ack(future_id)]
    );
    assert_eq!(
        first.events,
        vec![ClientProtocolEvent::PrizeEffectAccepted(receipt)]
    );
    let duplicate = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(
            future_id, &payload,
        ))
        .expect("duplicate future terminal");
    assert_eq!(
        duplicate.outbound_packets,
        vec![ReliableSequencer::encode_ack(future_id)]
    );
    assert!(duplicate.events.is_empty());

    for id in 0..future_id {
        let drained = session
            .handle_transport_packet(&ReliableSequencer::encode_reliable_message(id, &[0xee]))
            .expect("fill prefix");
        assert_eq!(
            drained.outbound_packets,
            vec![ReliableSequencer::encode_ack(id)]
        );
        assert!(
            drained
                .events
                .iter()
                .all(|event| !matches!(event, ClientProtocolEvent::PrizeEffectAccepted(_)))
        );
    }
    assert_eq!(session.sequencer.next_process_id, future_id + 1);
    assert!(session.priority_dispatched_reliable_ids.is_empty());
}

#[test]
fn duplicate_priority_receipt_dispatches_once_without_advancing_reliable_prefix() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let receipt = RepelApplicationAccepted {
        server_session_id: 77,
        life_seq: u32::MAX,
        delivery_generation: u32::MAX,
        snapshot_id: u16::MAX,
        application_sequence: 0,
        accepted_end_sequence: 135,
        active_steps: 135,
    };
    let mut payload = vec![
        RepelApplicationAccepted::TYPE,
        RepelApplicationAccepted::VERSION,
    ];
    payload.extend(receipt.server_session_id.to_le_bytes());
    payload.extend(receipt.life_seq.to_le_bytes());
    payload.extend(receipt.delivery_generation.to_le_bytes());
    payload.extend(receipt.snapshot_id.to_le_bytes());
    payload.extend(receipt.application_sequence.to_le_bytes());
    payload.extend(receipt.accepted_end_sequence.to_le_bytes());
    payload.extend(receipt.active_steps.to_le_bytes());
    let packet = ReliableSequencer::encode_reliable_message(4, &payload);

    for attempt in 0..2 {
        let actions = session
            .handle_transport_packet(&packet)
            .expect("duplicate out-of-order receipt should parse");
        if attempt == 0 {
            assert_eq!(
                actions.events,
                vec![ClientProtocolEvent::RepelApplicationAccepted(receipt)]
            );
        } else {
            assert!(actions.events.is_empty());
        }
        assert_eq!(
            actions.outbound_packets,
            vec![ReliableSequencer::encode_ack(4)]
        );
        assert_eq!(session.sequencer.next_process_id, 0);
        assert_eq!(session.sequencer.queued.len(), 1);
    }
}

#[test]
fn exact_movement_correction_crosses_reliable_gap_once_with_raw_f32_state() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let correction = MovementCorrection {
        reason: MovementCorrection::REASON_SUSTAINED_DRIFT,
        repel_window: None,
        server_session_id: 77,
        player_id: 9,
        life_seq: 41,
        correction_generation: 3,
        position_sequence: u16::MAX,
        server_tick: 1_234,
        position_x: 10_500.375,
        position_y: 8_191.75,
        velocity_x: -321.125,
        velocity_y: 47.5,
        orientation: 0.9875,
    };
    let packet = ReliableSequencer::encode_reliable_message(2, &correction.encode());

    let early = session
        .handle_transport_packet(&packet)
        .expect("future exact correction should parse");
    assert_eq!(
        early.events,
        vec![ClientProtocolEvent::MovementCorrected(correction)]
    );
    assert_eq!(session.sequencer.next_process_id, 0);
    assert_eq!(session.sequencer.queued.len(), 1);

    let duplicate = session
        .handle_transport_packet(&packet)
        .expect("future correction duplicate should remain idempotent");
    assert!(duplicate.events.is_empty());

    for id in 0..2 {
        let actions = session
            .handle_transport_packet(&ReliableSequencer::encode_reliable_message(id, &[0xee]))
            .expect("gap payload should drain through retained correction");
        assert!(
            actions
                .events
                .iter()
                .all(|event| !matches!(event, ClientProtocolEvent::MovementCorrected(_)))
        );
    }
    assert_eq!(session.sequencer.next_process_id, 3);
    assert!(session.priority_dispatched_reliable_ids.is_empty());
}

#[test]
fn malformed_or_trailing_receipt_prefix_is_acked_and_retired_without_priority_dispatch() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut trailing = vec![LifecycleBaselineAccepted::TYPE];
    trailing.extend(41_u32.to_le_bytes());
    trailing.extend(9_u32.to_le_bytes());
    trailing.extend(7_u16.to_le_bytes());
    trailing.push(0xee);

    let first = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(0, &trailing))
        .expect("trailing receipt prefix must retire as malformed application traffic");
    assert_eq!(
        first.outbound_packets,
        vec![ReliableSequencer::encode_ack(0)]
    );
    assert!(first.events.is_empty());
    assert_eq!(session.sequencer.next_process_id, 1);

    let malformed = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(
            3,
            &[WarpRelocationAccepted::TYPE],
        ))
        .expect("future malformed prefix should follow ordinary reliable retention");
    assert_eq!(
        malformed.outbound_packets,
        vec![ReliableSequencer::encode_ack(3)]
    );
    assert!(malformed.events.is_empty());
    assert!(session.priority_dispatched_reliable_ids.is_empty());

    for id in 1..=2 {
        let actions = session
            .handle_transport_packet(&ReliableSequencer::encode_reliable_message(id, &[0xee]))
            .expect("gap packet should drain through malformed retained receipt");
        assert_eq!(
            actions.outbound_packets,
            vec![ReliableSequencer::encode_ack(id)]
        );
        assert!(actions.events.iter().all(|event| {
            !matches!(
                event,
                ClientProtocolEvent::LifecycleBaselineAccepted(_)
                    | ClientProtocolEvent::WarpRelocationAccepted(_)
                    | ClientProtocolEvent::RepelApplicationAccepted(_)
                    | ClientProtocolEvent::MovementCorrected(_)
            )
        }));
    }
    assert_eq!(session.sequencer.next_process_id, 4);
    assert!(session.priority_dispatched_reliable_ids.is_empty());
}

#[test]
fn priority_receipt_is_exactly_once_across_reliable_id_wrap_and_ordered_retirement() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.sequencer.next_process_id = u32::MAX - 1;
    let receipt = WarpRelocationAccepted {
        collectible_id: 77,
        life_seq: 3,
        position_sequence: 0,
    };
    let mut payload = vec![WarpRelocationAccepted::TYPE];
    payload.extend(receipt.collectible_id.to_le_bytes());
    payload.extend(receipt.life_seq.to_le_bytes());
    payload.extend(receipt.position_sequence.to_le_bytes());
    let wrapped = ReliableSequencer::encode_reliable_message(0, &payload);

    let early = session
        .handle_transport_packet(&wrapped)
        .expect("wrapped future receipt should parse");
    assert_eq!(
        early.events,
        vec![ClientProtocolEvent::WarpRelocationAccepted(receipt)]
    );
    let duplicate = session
        .handle_transport_packet(&wrapped)
        .expect("wrapped duplicate should parse");
    assert!(duplicate.events.is_empty());

    for id in [u32::MAX - 1, u32::MAX] {
        let actions = session
            .handle_transport_packet(&ReliableSequencer::encode_reliable_message(id, &[0xee]))
            .expect("wrapped gap should drain");
        assert!(
            actions
                .events
                .iter()
                .all(|event| { !matches!(event, ClientProtocolEvent::WarpRelocationAccepted(_)) })
        );
    }
    assert_eq!(session.sequencer.next_process_id, 1);
    assert!(session.priority_dispatched_reliable_ids.is_empty());
    let retired_duplicate = session
        .handle_transport_packet(&wrapped)
        .expect("retired wrapped duplicate should ACK");
    assert_eq!(
        retired_duplicate.outbound_packets,
        vec![ReliableSequencer::encode_ack(0)]
    );
    assert!(retired_duplicate.events.is_empty());
}

#[test]
fn client_protocol_session_processes_reliable_acks_inside_packet_clusters() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let login = PasswordLoginRequest::new(LoginProtocol::Continuum, false, "pilot", "", 0, 0, 0, 0);
    session.send_password_login(&login);
    session
        .sequencer
        .send_reliable_at_tick(&ArenaLoginRequest::spectator_default().encode_payload(), 10);
    assert_eq!(session.sequencer.sent.len(), 2);

    let ack_zero = ReliableSequencer::encode_ack(0);
    let ack_one = ReliableSequencer::encode_ack(1);
    let mut cluster = vec![PacketChunker::CORE_BYTE, PacketChunker::PACKET_CLUSTER_TYPE];
    cluster.push(ack_zero.len() as u8);
    cluster.extend_from_slice(&ack_zero);
    cluster.push(ack_one.len() as u8);
    cluster.extend_from_slice(&ack_one);

    let actions = session
        .handle_transport_packet(&cluster)
        .expect("clustered acks should process");

    assert!(actions.outbound_packets.is_empty());
    assert!(actions.events.is_empty());
    assert!(session.sequencer.sent.is_empty());
}

#[test]
fn client_protocol_session_processes_reliable_messages_inside_packet_clusters_in_order() {
    let arena_login = ArenaLoginRequest::spectator_default();
    let mut session = ClientProtocolSession::new(true, arena_login.clone(), 0, 0);
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let unknown = [0xee, 1, 2, 3];
    let reliable_one = ReliableSequencer::encode_reliable_message(1, &unknown);
    let reliable_zero = ReliableSequencer::encode_reliable_message(0, &response);
    let mut cluster = vec![PacketChunker::CORE_BYTE, PacketChunker::PACKET_CLUSTER_TYPE];
    cluster.push(reliable_one.len() as u8);
    cluster.extend_from_slice(&reliable_one);
    cluster.push(reliable_zero.len() as u8);
    cluster.extend_from_slice(&reliable_zero);

    let actions = session
        .handle_transport_packet(&cluster)
        .expect("clustered reliable messages should process");

    assert_eq!(
        actions.outbound_packets,
        vec![
            ReliableSequencer::encode_ack(1),
            ReliableSequencer::encode_ack(0),
            ReliableSequencer::encode_reliable_message(0, &arena_login.encode_payload()),
        ]
    );
    assert_eq!(
        actions.events,
        vec![
            ClientProtocolEvent::PasswordAccepted,
            ClientProtocolEvent::UnknownPacket(0xee),
        ]
    );
    assert_eq!(session.state, ClientProtocolState::ArenaLoginSent);
}

#[test]
fn client_protocol_session_processes_nested_packet_clusters_like_reference_game() {
    let arena_login = ArenaLoginRequest::spectator_default();
    let mut session = ClientProtocolSession::new(true, arena_login.clone(), 0, 0);
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let reliable = ReliableSequencer::encode_reliable_message(0, &response);

    let mut inner_cluster = vec![PacketChunker::CORE_BYTE, PacketChunker::PACKET_CLUSTER_TYPE];
    inner_cluster.push(reliable.len() as u8);
    inner_cluster.extend_from_slice(&reliable);

    let mut outer_cluster = vec![PacketChunker::CORE_BYTE, PacketChunker::PACKET_CLUSTER_TYPE];
    outer_cluster.push(inner_cluster.len() as u8);
    outer_cluster.extend_from_slice(&inner_cluster);

    let actions = session
        .handle_transport_packet(&outer_cluster)
        .expect("nested packet cluster should process recursively");

    assert_eq!(
        actions.outbound_packets,
        vec![
            ReliableSequencer::encode_ack(0),
            ReliableSequencer::encode_reliable_message(0, &arena_login.encode_payload()),
        ]
    );
    assert_eq!(actions.events, vec![ClientProtocolEvent::PasswordAccepted]);
    assert_eq!(session.state, ClientProtocolState::ArenaLoginSent);
}

#[test]
fn client_protocol_session_processes_clustered_transport_packets() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let unknown = [0xee, 1, 2, 3];
    let mut cluster = vec![PacketChunker::CORE_BYTE, PacketChunker::PACKET_CLUSTER_TYPE];
    cluster.push(response.len() as u8);
    cluster.extend_from_slice(&response);
    cluster.push(unknown.len() as u8);
    cluster.extend_from_slice(&unknown);

    let actions = session
        .handle_transport_packet(&cluster)
        .expect("cluster should process");

    assert_eq!(
        actions.events,
        vec![
            ClientProtocolEvent::PasswordAccepted,
            ClientProtocolEvent::UnknownPacket(0xee),
        ]
    );
    assert_eq!(actions.outbound_packets.len(), 1);
    assert_eq!(session.state, ClientProtocolState::ArenaLoginSent);
}

#[test]
fn client_protocol_session_reassembles_small_chunks_inside_packet_clusters() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let split = 7;
    let mut body = vec![PacketChunker::CORE_BYTE, PacketChunker::SMALL_BODY_TYPE];
    body.extend_from_slice(&response[..split]);
    let mut tail = vec![PacketChunker::CORE_BYTE, PacketChunker::SMALL_TAIL_TYPE];
    tail.extend_from_slice(&response[split..]);
    let mut cluster = vec![PacketChunker::CORE_BYTE, PacketChunker::PACKET_CLUSTER_TYPE];
    cluster.push(body.len() as u8);
    cluster.extend_from_slice(&body);
    cluster.push(tail.len() as u8);
    cluster.extend_from_slice(&tail);

    let actions = session
        .handle_transport_packet(&cluster)
        .expect("clustered small chunks should process");

    assert_eq!(actions.events, vec![ClientProtocolEvent::PasswordAccepted]);
    assert_eq!(actions.outbound_packets.len(), 1);
    assert_eq!(session.state, ClientProtocolState::ArenaLoginSent);
}

#[test]
fn client_protocol_session_reassembles_huge_chunks_inside_packet_clusters() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut response = vec![0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let split = 9;
    let mut first = vec![PacketChunker::CORE_BYTE, PacketChunker::HUGE_TYPE];
    first.extend_from_slice(&(response.len() as u32).to_le_bytes());
    first.extend_from_slice(&response[..split]);
    let mut second = vec![PacketChunker::CORE_BYTE, PacketChunker::HUGE_TYPE];
    second.extend_from_slice(&(response.len() as u32).to_le_bytes());
    second.extend_from_slice(&response[split..]);
    let mut cluster = vec![PacketChunker::CORE_BYTE, PacketChunker::PACKET_CLUSTER_TYPE];
    cluster.push(first.len() as u8);
    cluster.extend_from_slice(&first);
    cluster.push(second.len() as u8);
    cluster.extend_from_slice(&second);

    let actions = session
        .handle_transport_packet(&cluster)
        .expect("clustered huge chunks should process");

    assert_eq!(actions.events, vec![ClientProtocolEvent::PasswordAccepted]);
    assert_eq!(actions.outbound_packets.len(), 1);
    assert_eq!(session.state, ClientProtocolState::ArenaLoginSent);
}

#[test]
fn client_protocol_session_processes_huge_chunk_cancel_inside_packet_clusters() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.chunker.huge.push(b"partial").unwrap();
    let cancel = [PacketChunker::CORE_BYTE, PacketChunker::HUGE_CANCEL_TYPE];
    let mut cluster = vec![PacketChunker::CORE_BYTE, PacketChunker::PACKET_CLUSTER_TYPE];
    cluster.push(cancel.len() as u8);
    cluster.extend_from_slice(&cancel);

    let actions = session
        .handle_transport_packet(&cluster)
        .expect("clustered huge cancel should process");

    assert_eq!(
        actions.outbound_packets,
        vec![vec![
            PacketChunker::CORE_BYTE,
            PacketChunker::HUGE_CANCEL_ACK_TYPE
        ]]
    );
    assert!(actions.events.is_empty());
    assert!(session.chunker.huge.data.is_empty());
    assert_eq!(session.chunker.huge.len, 0);
}

#[test]
fn client_protocol_session_ignores_server_no_op_packets() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    for packet_type in [
        ServerPacket::NO_OP_11,
        ServerPacket::NO_OP_17,
        ServerPacket::NO_OP_26,
    ] {
        let actions = session
            .handle_server_packet(&[packet_type])
            .expect("server no-op should process");

        assert!(actions.events.is_empty());
        assert!(actions.outbound_packets.is_empty());
    }
}

#[test]
fn client_protocol_session_emits_late_continuum_extension_events() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    let actions = session
        .handle_server_packet(&[ServerPacket::REDIRECT, 0x7f, 0, 0, 1, 0x39, 0x30])
        .expect("redirect should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::Redirect(ServerRedirectPacket {
            payload: vec![0x7f, 0, 0, 1, 0x39, 0x30],
        })]
    );
    assert!(actions.outbound_packets.is_empty());

    let actions = session
        .handle_server_packet(&[ServerPacket::SELECT_BOX, b'o', b'k'])
        .expect("select box should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::SelectBox(SelectBoxPacket {
            payload: b"ok".to_vec(),
        })]
    );
    assert!(actions.outbound_packets.is_empty());
}

#[test]
fn client_protocol_session_reassembles_small_chunk_transport_packets() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut response = [0; PasswordResponse::MIN_PACKET_LEN];
    response[0] = PasswordResponse::TYPE;
    let split = 7;
    let mut body = vec![PacketChunker::CORE_BYTE, PacketChunker::SMALL_BODY_TYPE];
    body.extend_from_slice(&response[..split]);
    let mut tail = vec![PacketChunker::CORE_BYTE, PacketChunker::SMALL_TAIL_TYPE];
    tail.extend_from_slice(&response[split..]);

    assert_eq!(
        session
            .handle_transport_packet(&body)
            .expect("body chunk should process"),
        ClientProtocolActions::default()
    );
    let actions = session
        .handle_transport_packet(&tail)
        .expect("tail chunk should process");

    assert_eq!(actions.events, vec![ClientProtocolEvent::PasswordAccepted]);
    assert_eq!(session.state, ClientProtocolState::ArenaLoginSent);
}

#[test]
fn client_protocol_session_emits_position_events_and_counts_weapons() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut large = vec![0; PlayerPositionUpdate::LARGE_MIN_LEN];
    large[0] = ServerPacket::LARGE_POSITION;
    large[1] = 10;
    put_u16(&mut large, 2, 44);
    put_u16(&mut large, 4, 100);
    put_i16(&mut large, 6, -20);
    put_u16(&mut large, 8, 7);
    put_i16(&mut large, 10, 30);
    put_u16(&mut large, 15, 200);
    put_u16(&mut large, 19, 3);

    let actions = session
        .handle_server_packet(&large)
        .expect("large position should emit event");

    assert_eq!(session.weapons_received, 1);
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
            player_id: 7,
            direction: 10,
            timestamp: 44,
            x: 100,
            y: 200,
            velocity_x: 30,
            velocity_y: -20,
            togglables: 0,
            ping: 0,
            bounty: 0,
            weapon: 3,
            checksum: Some(0),
            energy: None,
            s2c_latency: None,
            flag_timer: None,
            items: None,
        })]
    );
}

#[test]
fn client_protocol_session_emits_batched_position_events() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut packet = vec![0; 1 + PlayerPositionUpdate::BATCHED_SMALL_ENTRY_LEN * 2];
    packet[0] = ServerPacket::BATCHED_SMALL_POSITION;
    packet[1] = 9;
    put_u16(&mut packet, 2, 12 << 10 | 4);
    put_u32(&mut packet, 4, (0x0022 << 14) | 0x0011);
    put_u16(&mut packet, 8, 8);
    packet[11] = 10;
    put_u16(&mut packet, 12, 13 << 10 | 5);
    put_u32(&mut packet, 14, (0x0044 << 14) | 0x0033);
    put_u16(&mut packet, 18, 9);

    let actions = session
        .handle_server_packet(&packet)
        .expect("batched positions should emit events");

    assert_eq!(actions.events.len(), 2);
    assert!(matches!(
        &actions.events[0],
        ClientProtocolEvent::BatchedSmallPlayerPosition(update)
            if update.player_id == 9 && update.direction == 12 && update.x == 0x0011
    ));
    assert!(matches!(
        &actions.events[1],
        ClientProtocolEvent::BatchedSmallPlayerPosition(update)
            if update.player_id == 10 && update.direction == 13 && update.y == 0x0044
    ));
}

#[test]
fn client_protocol_session_emits_powerball_position_events() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut packet = vec![0; PowerballPositionUpdate::PACKET_LEN];
    packet[0] = PowerballPositionUpdate::TYPE;
    packet[1] = 2;
    put_u16(&mut packet, 2, 100);
    put_u16(&mut packet, 4, 200);
    put_i16(&mut packet, 6, -50);
    put_i16(&mut packet, 8, 75);
    put_u16(&mut packet, 10, 9);
    put_u32(&mut packet, 12, 33);

    let actions = session
        .handle_server_packet(&packet)
        .expect("powerball should emit event");

    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::PowerballPosition(
            PowerballPositionUpdate {
                ball_id: 2,
                x: 100,
                y: 200,
                velocity_x: -50,
                velocity_y: 75,
                owner_id: 9,
                timestamp: 33,
            }
        )]
    );
}

#[test]
fn client_protocol_session_emits_prize_events() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.self_player_id = Some(9);
    session.latest_server_clock_bootstrap = Some(ServerClockBootstrap {
        server_session_id: 77,
        arena_stream_id: 3,
        ..ServerClockBootstrap::default()
    });

    let mut player_prize = vec![0; PlayerPrize::PACKET_LEN];
    player_prize[0] = PlayerPrize::TYPE;
    put_u32(&mut player_prize, 1, 99);
    put_u16(&mut player_prize, 5, 44);
    put_u16(&mut player_prize, 7, 55);
    put_i16(&mut player_prize, 9, 21);
    put_u16(&mut player_prize, 11, 9);
    let actions = session
        .handle_server_packet(&player_prize)
        .expect("player prize should emit event");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::PlayerPrize(PlayerPrize {
            collectible_id: 99,
            x: 44,
            y: 55,
            prize_id: 21,
            player_id: 9,
        })]
    );

    let mut ack = vec![0; PrizePickupAck::PACKET_LEN];
    ack[0] = PrizePickupAck::TYPE;
    put_u32(&mut ack, 1, 100);
    put_u16(&mut ack, 5, 44);
    put_u16(&mut ack, 7, 55);
    put_i16(&mut ack, 9, 21);
    put_u16(&mut ack, 11, 1300);
    put_u32(&mut ack, 13, 7);
    put_u32(&mut ack, 17, 3);
    ack[26..34].copy_from_slice(&77_u64.to_le_bytes());
    ack[34..42].copy_from_slice(&3_u64.to_le_bytes());
    put_u16(&mut ack, 42, 9);
    let actions = session
        .handle_server_packet(&ack)
        .expect("prize pickup ack should emit event");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::PrizePickupAck(ScopedPrizePickupAck {
            ack: PrizePickupAck {
                server_session_id: 77,
                arena_stream_id: 3,
                player_id: 9,
                collectible_id: 100,
                x: 44,
                y: 55,
                prize_id: 21,
                authoritative_energy: 1300,
                life_seq: 7,
                prize_state_revision: 3,
                engine_shutdown_until_tick: None,
                engine_shutdown_application_required: false,
            },
            server_session_id: Some(77),
            arena_stream_id: Some(3),
            player_id: Some(9),
        })]
    );

    let mut collected = vec![0; CollectedPrize::PACKET_LEN];
    collected[0] = CollectedPrize::TYPE;
    put_u16(&mut collected, 1, 2);
    put_i16(&mut collected, 3, 27);
    let actions = session
        .handle_server_packet(&collected)
        .expect("collected prize should emit event");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::CollectedPrize(CollectedPrize {
            count: 2,
            prize_id: 27,
        })]
    );
}

#[test]
fn client_protocol_session_emits_flag_events_and_updates_roster() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let enter = sample_player_enter_packet(9, b"carrier");
    session
        .handle_server_packet(&enter)
        .expect("player enter should process");

    let mut position = vec![0; FlagPosition::PACKET_LEN];
    position[0] = FlagPosition::TYPE;
    put_u16(&mut position, 1, 3);
    put_u16(&mut position, 3, 44);
    put_u16(&mut position, 5, 55);
    put_u16(&mut position, 7, 2);
    let actions = session
        .handle_server_packet(&position)
        .expect("flag position should emit event");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::FlagPosition(FlagPosition {
            flag_id: 3,
            x: 44,
            y: 55,
            owner: 2,
        })]
    );

    let actions = session
        .handle_server_packet(&[FlagClaim::TYPE, 3, 0, 9, 0])
        .expect("flag claim should emit event");
    assert_eq!(session.roster.get(&9).map(|entry| entry.flags), Some(3));
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::FlagClaim(FlagClaim {
            flag_id: 3,
            player_id: 9,
        })]
    );

    let mut victory = vec![0; FlagVictory::PACKET_LEN];
    victory[0] = FlagVictory::TYPE;
    put_u16(&mut victory, 1, 7);
    put_u32(&mut victory, 3, 123_456);
    let actions = session
        .handle_server_packet(&victory)
        .expect("flag victory should emit event");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::FlagVictory(FlagVictory {
            team: 7,
            jackpot_points: 123_456,
        })]
    );

    let actions = session
        .handle_server_packet(&[DropFlag::TYPE, 9, 0])
        .expect("drop flag should emit event");
    assert_eq!(session.roster.get(&9).map(|entry| entry.flags), Some(0));
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::DropFlag(DropFlag { player_id: 9 })]
    );

    let actions = session
        .handle_server_packet(&[TurfFlagUpdate::TYPE, 2, 0, 4, 0])
        .expect("turf flag update should emit event");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::TurfFlagUpdate(TurfFlagUpdate {
            owners: vec![2, 4],
        })]
    );

    let actions = session
        .handle_server_packet(&[FlagClaim::TYPE, 0, 0, 9, 0])
        .expect("turf flag claim should emit event");
    assert_eq!(session.roster.get(&9).map(|entry| entry.flags), Some(0));
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::FlagClaim(FlagClaim {
            flag_id: 0,
            player_id: 9,
        })]
    );

    session
        .handle_server_packet(&[ServerPacket::PLAYER_ID, 7, 0])
        .expect("player id reset should process");
    session
        .handle_server_packet(&enter)
        .expect("player re-enter should process");
    session
        .handle_server_packet(&[FlagClaim::TYPE, 0, 0, 9, 0])
        .expect("post-reset flag claim should emit event");
    assert_eq!(session.roster.get(&9).map(|entry| entry.flags), Some(3));

    let actions = session
        .handle_server_packet(&[FlagReward::TYPE, 7, 0, 44, 0])
        .expect("flag reward should emit event");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::FlagReward(FlagReward {
            grants: vec![FlagRewardGrant {
                team: 7,
                points: 44,
            }],
        })]
    );
}

#[test]
fn client_protocol_session_emits_brick_dropped_events() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let actions = session
        .handle_server_packet(&[BrickDropped::TYPE])
        .expect("brick clear should emit event");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::BrickDropped(BrickDropped::Clear)]
    );

    let mut packet = vec![0; BrickDropped::PACKET_LEN];
    packet[0] = BrickDropped::TYPE;
    put_u16(&mut packet, 1, 10);
    put_u16(&mut packet, 3, 11);
    put_u16(&mut packet, 5, 14);
    put_u16(&mut packet, 7, 11);
    put_u16(&mut packet, 9, 3);
    put_u16(&mut packet, 11, 99);
    put_u32(&mut packet, 13, 1234);
    let actions = session
        .handle_server_packet(&packet)
        .expect("brick drop should emit event");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::BrickDropped(BrickDropped::Drop(
            BrickDrop {
                x1: 10,
                y1: 11,
                x2: 14,
                y2: 11,
                team: 3,
                brick_id: 99,
                timestamp: 1234,
            }
        ))]
    );
}

#[test]
fn client_protocol_session_emits_lvz_events() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let toggle = [LvzTogglePacket::TYPE, 0x22, 0x00, 0x23, 0x80];

    let actions = session
        .handle_server_packet(&toggle)
        .expect("toggle should emit event");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::LvzToggled(LvzTogglePacket {
            toggles: vec![
                LvzObjectToggle {
                    object_id: 0x0022,
                    enabled: true,
                },
                LvzObjectToggle {
                    object_id: 0x0023,
                    enabled: false,
                },
            ],
        })]
    );

    let mut modify = vec![0; LvzModifyPacket::PACKET_SIZE];
    modify[0] = LvzModifyPacket::TYPE;
    modify[1] = 0x14;
    put_u16(&mut modify, 2, 33 << 1);
    put_u16(&mut modify, 4, 5 << 4);
    put_u16(&mut modify, 6, 6 << 4);
    modify[9] = 6;
    put_u16(&mut modify, 10, 2 << 12);
    let actions = session
        .handle_server_packet(&modify)
        .expect("modify should emit event");

    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::LvzModified(LvzModifyPacket {
            changed: LvzModificationFlags {
                xy: false,
                image: false,
                layer: true,
                time: false,
                mode: true,
            },
            map_object: false,
            object_id: 33,
            map_x: 80,
            map_y: 96,
            screen_x: 5,
            screen_y: 6,
            x_type: 0,
            y_type: 0,
            image_id: 0,
            layer: 6,
            display_time: 0,
            display_mode: 2,
        })]
    );
}

#[test]
fn client_protocol_session_tracks_roster_and_chat_events() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    let actions = session
        .handle_server_packet(&[ServerPacket::PLAYER_ID, 7, 0])
        .expect("player id should process");
    assert_eq!(session.self_player_id, Some(7));
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::PlayerIdAssigned(7)]
    );

    let enter = sample_player_enter_packet(9, b"pilot");
    let actions = session
        .handle_server_packet(&enter)
        .expect("player enter should process");
    assert_eq!(
        session.roster.get(&9).map(|entry| entry.name.as_str()),
        Some("pilot")
    );
    assert!(matches!(
        &actions.events[..],
        [ClientProtocolEvent::PlayerEntered(entry)] if entry.player_id == 9
    ));

    let enter = sample_player_enter_packet(10, b"victim");
    session
        .handle_server_packet(&enter)
        .expect("second player enter should process");

    let mut banner_packet = vec![0; PlayerBannerChange::PACKET_LEN];
    banner_packet[0] = PlayerBannerChange::TYPE;
    put_u16(&mut banner_packet, 1, 9);
    banner_packet[3] = 44;
    let actions = session
        .handle_server_packet(&banner_packet)
        .expect("banner should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::PlayerBannerChanged(
            PlayerBannerChange {
                player_id: 9,
                banner: {
                    let mut banner = [0; PLAYER_BANNER_BYTES];
                    banner[0] = 44;
                    banner
                },
            }
        )]
    );

    let mut score = vec![0; ScoreUpdate::PACKET_LEN];
    score[0] = ScoreUpdate::TYPE;
    put_u16(&mut score, 1, 9);
    put_u32(&mut score, 3, 10_001);
    put_u32(&mut score, 7, 20_002);
    put_u16(&mut score, 11, 12);
    put_u16(&mut score, 13, 5);
    let actions = session
        .handle_server_packet(&score)
        .expect("score update should process");
    let entry = session
        .roster
        .get(&9)
        .expect("score update should keep roster entry");
    assert_eq!(entry.kill_points, 10_001);
    assert_eq!(entry.flag_points, 20_002);
    assert_eq!(entry.wins, 12);
    assert_eq!(entry.losses, 5);
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::ScoreUpdated(ScoreUpdate {
            assists: 0,
            player_id: 9,
            kill_points: 10_001,
            flag_points: 20_002,
            wins: 12,
            losses: 5,
        })]
    );

    let mut enter_with_tail = sample_player_enter_packet(11, b"tailed");
    let mut tailed_score = vec![0; ScoreUpdate::PACKET_LEN];
    tailed_score[0] = ScoreUpdate::TYPE;
    put_u16(&mut tailed_score, 1, 11);
    put_u32(&mut tailed_score, 3, 30_003);
    put_u32(&mut tailed_score, 7, 40_004);
    put_u16(&mut tailed_score, 11, 13);
    put_u16(&mut tailed_score, 13, 6);
    enter_with_tail.extend_from_slice(&tailed_score);
    let actions = session
        .handle_server_packet(&enter_with_tail)
        .expect("player-enter packet tail should process");
    assert!(matches!(
        actions.events.as_slice(),
        [
            ClientProtocolEvent::PlayerEntered(entry),
            ClientProtocolEvent::ScoreUpdated(score),
        ] if entry.player_id == 11
            && score.player_id == 11
            && score.kill_points == 30_003
            && score.losses == 6
    ));
    let entry = session
        .roster
        .get(&11)
        .expect("tailed score should update roster");
    assert_eq!(entry.kill_points, 30_003);
    assert_eq!(entry.flag_points, 40_004);
    let mut extended_enter = sample_player_enter_packet(12, b"extended");
    let mut extended_score = tailed_score.clone();
    put_u16(&mut extended_score, 1, 12);
    extended_score.push(1);
    extended_score.extend(7u32.to_le_bytes());
    extended_enter.extend(extended_score);
    let extended_actions = session.handle_server_packet(&extended_enter).unwrap();
    assert!(matches!(extended_actions.events.as_slice(),
        [ClientProtocolEvent::PlayerEntered(entry), ClientProtocolEvent::ScoreUpdated(score)]
        if entry.player_id == 12 && score.kill_points == 30_003 && score.assists == 7));

    let actions = session
        .handle_server_packet(&[FrequencyChange::TYPE, 9, 0, 22, 0])
        .expect("frequency change should process");
    assert_eq!(
        session.roster.get(&9).map(|entry| entry.frequency),
        Some(22)
    );
    assert_eq!(session.roster.get(&9).map(|entry| entry.flags), Some(0));
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::FrequencyChanged(FrequencyChange {
            player_id: 9,
            frequency: 22,
        })]
    );

    let actions = session
        .handle_server_packet(&[ShipFrequencyChange::TYPE, 4, 9, 0, 33, 0])
        .expect("ship/frequency change should process");
    assert_eq!(session.roster.get(&9).map(|entry| entry.ship), Some(4));
    assert_eq!(
        session.roster.get(&9).map(|entry| entry.frequency),
        Some(33)
    );
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::ShipFrequencyChanged(
            ShipFrequencyChange {
                ship: 4,
                player_id: 9,
                frequency: 33,
                life_seq: None,
            }
        )]
    );

    let actions = session
        .handle_server_packet(&[CreateTurretLink::TYPE, 9, 0, 10, 0])
        .expect("create turret should process");
    assert_eq!(
        session.roster.get(&9).map(|entry| entry.attach_parent),
        Some(10)
    );
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
            requester_id: 9,
            destination_id: Some(10),
        })]
    );

    let actions = session
        .handle_server_packet(&[CreateTurretLink::TYPE, 9, 0, 0xff, 0xff])
        .expect("turret detach should process");
    assert_eq!(
        session.roster.get(&9).map(|entry| entry.attach_parent),
        Some(INVALID_PLAYER_ID)
    );
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
            requester_id: 9,
            destination_id: Some(INVALID_PLAYER_ID),
        })]
    );

    session
        .handle_server_packet(&[CreateTurretLink::TYPE, 9, 0, 10, 0])
        .expect("second create turret should process");
    let actions = session
        .handle_server_packet(&[DestroyTurretLink::TYPE, 10, 0])
        .expect("destroy turret should process");
    assert_eq!(
        session.roster.get(&9).map(|entry| entry.attach_parent),
        Some(INVALID_PLAYER_ID)
    );
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::TurretLinkDestroyed(
            DestroyTurretLink { player_id: 10 }
        )]
    );

    let actions = session
        .handle_server_packet(&[ChatPacket::TYPE, 2, 3, 9, 0, b'h', b'i'])
        .expect("chat should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::Chat(ChatPacket {
            chat_type: 2,
            sound: 3,
            sender_id: 9,
            message: "hi".to_string(),
        })]
    );

    let mut death = vec![0; PlayerDeath::PACKET_LEN];
    death[0] = PlayerDeath::TYPE;
    death[1] = 3;
    put_u16(&mut death, 2, 9);
    put_u16(&mut death, 4, 10);
    put_u16(&mut death, 6, 77);
    put_u16(&mut death, 8, 4);
    put_u32(&mut death, 14, 3);
    put_u32(&mut death, 18, 1_234);
    let actions = session
        .handle_server_packet(&death)
        .expect("death should process");
    assert_eq!(session.roster.get(&9).map(|entry| entry.wins), Some(13));
    assert_eq!(session.roster.get(&9).map(|entry| entry.flags), Some(4));
    assert_eq!(session.roster.get(&10).map(|entry| entry.losses), Some(10));
    assert_eq!(session.roster.get(&10).map(|entry| entry.flags), Some(0));
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::PlayerDeath(PlayerDeath {
            green_id: 3,
            green_collectible_id: 0,
            killer_id: 9,
            killed_id: 10,
            bounty: 77,
            flag_transfer: 4,
            life_seq: 3,
            respawn_at_tick: 1_234,
        })]
    );

    let actions = session
        .handle_server_packet(&[ResetScore::TYPE, 0xff, 0xff])
        .expect("global score reset should process");
    assert_eq!(
        session.roster.get(&9).map(|entry| entry.kill_points),
        Some(0)
    );
    assert_eq!(
        session.roster.get(&9).map(|entry| entry.flag_points),
        Some(0)
    );
    assert_eq!(session.roster.get(&9).map(|entry| entry.wins), Some(0));
    assert_eq!(session.roster.get(&9).map(|entry| entry.losses), Some(0));
    assert_eq!(session.roster.get(&10).map(|entry| entry.losses), Some(0));
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::ScoresReset(ResetScore {
            player_id: INVALID_PLAYER_ID,
        })]
    );

    let mut goal = vec![0; SoccerGoal::PACKET_LEN];
    goal[0] = SoccerGoal::TYPE;
    put_u16(&mut goal, 1, 3);
    put_u32(&mut goal, 3, 12_345);
    let actions = session
        .handle_server_packet(&goal)
        .expect("soccer goal should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::SoccerGoal(SoccerGoal {
            frequency: 3,
            team_points: 12_345,
        })]
    );
    let actions = session
        .handle_server_packet(&[PlayerVoice::TYPE, 9, 0, b'R', b'I', b'F', b'F'])
        .expect("voice should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::PlayerVoice(PlayerVoice {
            player_id: 9,
            wav_data: b"RIFF".to_vec(),
        })]
    );

    let actions = session
        .handle_server_packet(&[PlayerLeaving::TYPE, 9, 0])
        .expect("leave should process");
    assert!(!session.roster.contains_key(&9));
    assert_eq!(actions.events, vec![ClientProtocolEvent::PlayerLeft(9)]);
}

#[test]
fn client_protocol_session_keeps_duplicate_player_names_by_player_id() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    session
        .handle_server_packet(&sample_player_enter_packet(9, b"pilot"))
        .expect("first player enter should process");
    session
        .handle_server_packet(&sample_player_enter_packet(10, b"child"))
        .expect("child player enter should process");
    session
        .handle_server_packet(&[CreateTurretLink::TYPE, 10, 0, 9, 0])
        .expect("create turret should process");

    let actions = session
        .handle_server_packet(&sample_player_enter_packet(12, b"pilot"))
        .expect("duplicate-name enter should process");

    assert_eq!(
        session.roster.get(&9).map(|entry| entry.name.as_str()),
        Some("pilot")
    );
    assert_eq!(
        session.roster.get(&12).map(|entry| entry.name.as_str()),
        Some("pilot")
    );
    assert_eq!(
        session.roster.get(&10).map(|entry| entry.attach_parent),
        Some(9)
    );
    assert!(matches!(
        actions.events.as_slice(),
        [ClientProtocolEvent::PlayerEntered(entry)] if entry.player_id == 12
    ));
}

#[test]
fn client_protocol_session_detaches_roster_links_on_reference_events() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    for (id, name) in [(9, b"parent".as_slice()), (10, b"player"), (11, b"child")] {
        session
            .handle_server_packet(&sample_player_enter_packet(id, name))
            .expect("player enter should process");
    }

    session
        .handle_server_packet(&[CreateTurretLink::TYPE, 10, 0, 9, 0])
        .expect("attach player to parent should process");
    session
        .handle_server_packet(&[CreateTurretLink::TYPE, 11, 0, 10, 0])
        .expect("attach child to player should process");
    session
        .handle_server_packet(&[FrequencyChange::TYPE, 10, 0, 44, 0])
        .expect("frequency change should process");
    assert_eq!(
        session.roster.get(&10).map(|entry| entry.attach_parent),
        Some(INVALID_PLAYER_ID)
    );
    assert_eq!(
        session.roster.get(&10).map(|entry| entry.frequency),
        Some(44)
    );
    assert_eq!(session.roster.get(&10).map(|entry| entry.flags), Some(0));
    assert_eq!(
        session.roster.get(&11).map(|entry| entry.attach_parent),
        Some(INVALID_PLAYER_ID)
    );

    session
        .handle_server_packet(&[CreateTurretLink::TYPE, 11, 0, 10, 0])
        .expect("reattach child should process");
    session
        .handle_server_packet(&[ShipFrequencyChange::TYPE, 5, 10, 0, 66, 0])
        .expect("ship/frequency change should process");
    assert_eq!(session.roster.get(&10).map(|entry| entry.ship), Some(5));
    assert_eq!(
        session.roster.get(&10).map(|entry| entry.frequency),
        Some(66)
    );
    assert_eq!(
        session.roster.get(&11).map(|entry| entry.attach_parent),
        Some(INVALID_PLAYER_ID)
    );

    session
        .handle_server_packet(&[CreateTurretLink::TYPE, 10, 0, 9, 0])
        .expect("reattach player should process");
    session
        .handle_server_packet(&[CreateTurretLink::TYPE, 11, 0, 10, 0])
        .expect("reattach child should process");
    session
        .handle_server_packet(&[PlayerLeaving::TYPE, 10, 0])
        .expect("leave should process");
    assert!(!session.roster.contains_key(&10));
    assert_eq!(
        session.roster.get(&11).map(|entry| entry.attach_parent),
        Some(INVALID_PLAYER_ID)
    );

    session
        .handle_server_packet(&sample_player_enter_packet(10, b"player"))
        .expect("player re-enter should process");
    session
        .handle_server_packet(&[CreateTurretLink::TYPE, 10, 0, 9, 0])
        .expect("reattach killed player should process");
    session
        .handle_server_packet(&[CreateTurretLink::TYPE, 11, 0, 10, 0])
        .expect("reattach killed player child should process");
    let mut death = vec![0; PlayerDeath::PACKET_LEN];
    death[0] = PlayerDeath::TYPE;
    death[1] = 3;
    put_u16(&mut death, 2, 9);
    put_u16(&mut death, 4, 10);
    put_u16(&mut death, 6, 77);
    put_u16(&mut death, 8, 4);
    session
        .handle_server_packet(&death)
        .expect("death should process");
    assert_eq!(
        session.roster.get(&10).map(|entry| entry.attach_parent),
        Some(INVALID_PLAYER_ID)
    );
    assert_eq!(session.roster.get(&10).map(|entry| entry.losses), Some(10));
    assert_eq!(session.roster.get(&10).map(|entry| entry.flags), Some(0));
    assert_eq!(
        session.roster.get(&11).map(|entry| entry.attach_parent),
        Some(INVALID_PLAYER_ID)
    );
}

#[test]
fn client_protocol_session_confirms_server_forced_local_turret_detach() {
    let detach_request = AttachRequest {
        player_id: INVALID_PLAYER_ID,
    }
    .encode_payload();

    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.self_player_id = Some(7);
    let mut local = PlayerRosterEntry::parse(&sample_player_enter_packet(7, b"self")).unwrap();
    local.attach_parent = 10;
    session.roster.insert(7, local);
    session.roster.insert(
        10,
        PlayerRosterEntry::parse(&sample_player_enter_packet(10, b"parent")).unwrap(),
    );

    let actions = session
        .handle_server_packet(&[DestroyTurretLink::TYPE, 10, 0])
        .expect("destroying local parent should process");

    assert_eq!(
        session.roster.get(&7).map(|entry| entry.attach_parent),
        Some(INVALID_PLAYER_ID)
    );
    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_reliable_message(
            0,
            &detach_request
        )]
    );
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::TurretLinkDestroyed(
            DestroyTurretLink { player_id: 10 }
        )]
    );

    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.self_player_id = Some(7);
    let mut local = PlayerRosterEntry::parse(&sample_player_enter_packet(7, b"self")).unwrap();
    local.attach_parent = 10;
    session.roster.insert(7, local);

    let actions = session
        .handle_server_packet(&[CreateTurretLink::TYPE, 99, 0])
        .expect("short create-turret detach should process");

    assert_eq!(
        session.roster.get(&7).map(|entry| entry.attach_parent),
        Some(INVALID_PLAYER_ID)
    );
    assert_eq!(
        actions.outbound_packets,
        vec![ReliableSequencer::encode_reliable_message(
            0,
            &detach_request
        )]
    );
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
            requester_id: 99,
            destination_id: None,
        })]
    );
}

#[test]
fn client_protocol_session_tracks_join_and_sends_frequency_change() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.weapons_received = 3;
    let actions = session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .expect("join should process");

    assert_eq!(session.weapons_received, 0);
    assert_eq!(actions.events, vec![ClientProtocolEvent::JoinedArena(1)]);
    let actions = session
        .handle_server_packet(&[ShipReset::TYPE])
        .expect("ship reset should process");
    assert_eq!(actions.events, vec![ClientProtocolEvent::ShipReset]);
    let actions = session
        .handle_server_packet(&[KeepAlive::TYPE])
        .expect("keep-alive should process");
    assert_eq!(actions.events, vec![ClientProtocolEvent::KeepAlive]);
    let actions = session
        .handle_server_packet(&[PostLogin::TYPE])
        .expect("post-login should process");
    assert_eq!(actions.events, vec![ClientProtocolEvent::PostLogin]);
    let actions = session
        .handle_server_packet(&[SetCoordinates::TYPE, 44, 0, 55, 0])
        .expect("set coordinates should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::SetCoordinates(SetCoordinates {
            x: 44,
            y: 55,
        })]
    );
    let actions = session
        .handle_server_packet(&[SpectatePacket::TYPE, 1])
        .expect("spectate should process");
    assert!(session.extra_position_info);
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::SpectateUpdated(SpectatePacket {
            extra_position_info: true,
        })]
    );
    let actions = session
        .handle_server_packet(&[ServerVersionPacket::TYPE, 38, 0, 4, 3, 2, 1])
        .expect("version should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::ServerVersion(ServerVersionPacket {
            version: 38,
            checksum: 0x0102_0304,
        })]
    );
    let actions = session
        .handle_server_packet(&[LoginFailurePacket::TYPE, b'b', b'a', b'd', 0])
        .expect("login failure should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::LoginFailureMessage(
            LoginFailurePacket {
                reason: "bad".to_string(),
            }
        )]
    );
    let mut personal_banner = vec![0; PersonalBannerChange::PACKET_LEN];
    personal_banner[0] = PersonalBannerChange::TYPE;
    personal_banner[1] = 8;
    let actions = session
        .handle_server_packet(&personal_banner)
        .expect("personal banner should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::PersonalBannerChanged(
            PersonalBannerChange {
                banner: {
                    let mut banner = [0; PLAYER_BANNER_BYTES];
                    banner[0] = 8;
                    banner
                },
            }
        )]
    );
    let actions = session
        .handle_server_packet(&[ToggleSendDamagePacket::TYPE, 1])
        .expect("toggle send damage should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::SendDamageToggled(
            ToggleSendDamagePacket { enabled: true }
        )]
    );
    let actions = session
        .handle_server_packet(&[ToggleUfoPacket::TYPE, 1])
        .expect("toggle ufo should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::UfoToggled(ToggleUfoPacket {
            available: true
        })]
    );
    let mut personal_koth = vec![0; SetPersonalKothTimer::PACKET_LEN];
    personal_koth[0] = SetPersonalKothTimer::TYPE;
    put_u32(&mut personal_koth, 1, 12_345);
    let actions = session
        .handle_server_packet(&personal_koth)
        .expect("personal koth timer should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::PersonalKothTimerSet(
            SetPersonalKothTimer { timer: 12_345 }
        )]
    );
    let mut koth_reset = vec![0; KothGameReset::PACKET_LEN];
    koth_reset[0] = KothGameReset::TYPE;
    koth_reset[1] = 1;
    put_u32(&mut koth_reset, 2, 22_222);
    put_u16(&mut koth_reset, 6, 9);
    let actions = session
        .handle_server_packet(&koth_reset)
        .expect("koth reset should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::KothGameReset(KothGameReset {
            adding_timer: true,
            timer: 22_222,
            player_id: 9,
        })]
    );
    let mut add_koth = vec![0; AddKothTime::PACKET_LEN];
    add_koth[0] = AddKothTime::TYPE;
    put_u32(&mut add_koth, 1, 333);
    let actions = session
        .handle_server_packet(&add_koth)
        .expect("add koth time should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::KothTimeAdded(AddKothTime {
            additional_time: 333,
        })]
    );
    let mut arena_directory = vec![ArenaDirectoryListing::TYPE];
    arena_directory.extend_from_slice(b"Public 0\0");
    arena_directory.extend_from_slice(&42_u16.to_le_bytes());
    let actions = session
        .handle_server_packet(&arena_directory)
        .expect("arena directory should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::ArenaDirectory(ArenaDirectoryListing {
            arenas: vec![ArenaDirectoryEntry {
                name: "Public 0".to_string(),
                population: 42,
            }],
        })]
    );
    let mut zone_banner = vec![0; ZoneBannerAdvertisement::HEADER_LEN + 4];
    zone_banner[0] = ZoneBannerAdvertisement::TYPE;
    zone_banner[1] = 1;
    put_u16(&mut zone_banner, 2, 2);
    put_u16(&mut zone_banner, 4, 2);
    put_u32(&mut zone_banner, 6, 1_500);
    zone_banner[10..].copy_from_slice(&[1, 2, 3, 4]);
    let actions = session
        .handle_server_packet(&zone_banner)
        .expect("zone banner should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::ZoneBannerAdvertised(
            ZoneBannerAdvertisement {
                display_mode: 1,
                width: 2,
                height: 2,
                duration: 1_500,
                pixels: vec![1, 2, 3, 4],
            }
        )]
    );
    let mut transfer = vec![0; ServerFileTransfer::HEADER_LEN];
    transfer[0] = ServerFileTransfer::TYPE;
    transfer[1..9].copy_from_slice(b"news.txt");
    transfer.extend_from_slice(b"hello");
    let actions = session
        .handle_server_packet(&transfer)
        .expect("file transfer should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::FileTransferred(ServerFileTransfer {
            filename: "news.txt".to_string(),
            data: b"hello".to_vec(),
        })]
    );
    let mut file_request = vec![0; ClientFileRequest::PACKET_LEN];
    file_request[0] = ClientFileRequest::TYPE;
    file_request[1..12].copy_from_slice(b"profile.dat");
    let remote_offset = 1 + ClientFileRequest::LOCAL_FILENAME_LEN;
    file_request[remote_offset..remote_offset + 10].copy_from_slice(b"upload.bin");
    let actions = session
        .handle_server_packet(&file_request)
        .expect("client file request should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::ClientFileRequested(
            ClientFileRequest {
                local_filename: "profile.dat".to_string(),
                remote_filename: "upload.bin".to_string(),
            }
        )]
    );
    let mut speed = vec![0; SpeedGameOver::PACKET_LEN];
    speed[0] = SpeedGameOver::TYPE;
    speed[1] = 1;
    put_u16(&mut speed, 2, 3);
    put_u32(&mut speed, 4, 12_345);
    for index in 0..5 {
        put_u32(&mut speed, 8 + index * 4, 1000 + index as u32);
        put_u16(&mut speed, 28 + index * 2, 20 + index as u16);
    }
    let actions = session
        .handle_server_packet(&speed)
        .expect("speed game over should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::SpeedGameOver(SpeedGameOver {
            best: true,
            rank: 3,
            score: 12_345,
            player_scores: [1000, 1001, 1002, 1003, 1004],
            player_ids: [20, 21, 22, 23, 24],
        })]
    );
    let mut damage = vec![0; ServerDamageReport::HEADER_LEN + ServerDamageReport::ENTRY_LEN];
    damage[0] = ServerDamageReport::TYPE;
    put_u16(&mut damage, 1, 9);
    put_u32(&mut damage, 3, 0x0102_0304);
    put_u16(&mut damage, 7, 10);
    put_u16(&mut damage, 9, 0x1234);
    put_u16(&mut damage, 11, 900);
    put_u16(&mut damage, 13, 45);
    damage[15] = 6;
    let actions = session
        .handle_server_packet(&damage)
        .expect("watch damage should process");
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::DamageWatched(ServerDamageReport {
            player_id: 9,
            timestamp: 0x0102_0304,
            entries: vec![ServerDamageReportEntry {
                attacker_id: 10,
                weapon_data: 0x1234,
                energy: 900,
                damage: 45,
                timestamp_offset: 6,
                energy_before: None,
                damage_seq: None,
                projectile_id: None,
                victim_life_seq: None,
                shooter_life_seq: None,
                explicit_lethal: None,
                source_snapshot_id: None,
                source_snapshot_epoch: None,
            }],
        })]
    );
    let frequency_baseline = ClientPositionUpdate {
        direction: 0,
        timestamp: 0,
        x: 8_200,
        y: 8_200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        bounty: 500,
        energy: 1_700,
        weapon: 0,
        extra: Some(ClientPositionExtra::default()),
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            0, 8_200, 8_200, 0, 0,
        ),
        input_history: Some(ClientInputRecordPacket {
            base_seq: 77,
            inputs: vec![PlayerInputRecord::default()],
        }),
        warp_collectible_id: None,
        repel_application: None,
    };
    let frequency_payload = FrequencyChangeRequest {
        frequency: 123,
        position_sequence_base: 77,
        baseline: frequency_baseline.encode(),
    }
    .encode_payload();
    assert_eq!(
        session.try_send_frequency_change(123, 77, &frequency_baseline),
        Some((
            0,
            ReliableSequencer::encode_reliable_message(0, &frequency_payload)
        ))
    );
    assert_eq!(
        session.send_spectate_request(9),
        ReliableSequencer::encode_reliable_message(1, &[SpectateRequest::TYPE, 9, 0])
    );
    assert_eq!(
        session.send_attach_request(10),
        ReliableSequencer::encode_reliable_message(2, &[AttachRequest::TYPE, 10, 0])
    );
    assert_eq!(
        session.send_flag_request(3),
        ReliableSequencer::encode_reliable_message(3, &[FlagRequest::TYPE, 3, 0])
    );
    let death = DeathRequest {
        killer_id: 9,
        bounty: 77,
    };
    assert_eq!(
        session.send_death(&death),
        ReliableSequencer::encode_reliable_message(4, &death.encode_payload())
    );
    assert_eq!(
        session.send_leave_arena(),
        ReliableSequencer::encode_reliable_message(5, &[LeaveArenaRequest::TYPE])
    );
    assert_eq!(session.send_disconnect(), CoreDisconnectRequest.encode());
    assert_eq!(session.state, ClientProtocolState::Disconnected);
    let voice_upload = VoiceUploadRequest {
        index: 2,
        player_id: 9,
        wav_data: b"RIFF".to_vec(),
    };
    assert_eq!(
        session.send_voice_upload_packets(&voice_upload),
        vec![ReliableSequencer::encode_reliable_message(
            6,
            &voice_upload.encode_payload()
        )]
    );
    let client_file = ClientFileTransferRequest {
        filename: "profile.dat".to_string(),
        data: b"payload".to_vec(),
    };
    assert_eq!(
        session.send_client_file_transfer_packets(&client_file),
        vec![ReliableSequencer::encode_reliable_message(
            7,
            &client_file.encode_payload()
        )]
    );
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
    assert_eq!(
        session.send_registration_response_packets(&registration),
        reliable_small_chunk_packets(8, &registration.encode_payload())
    );
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
    assert_eq!(
        session.send_lvz_toggle(&lvz_toggle),
        ReliableSequencer::encode_reliable_message(10, &lvz_toggle.encode_payload())
    );
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
    assert_eq!(
        session.send_lvz_modify(&lvz_modify),
        ReliableSequencer::encode_reliable_message(11, &lvz_modify.encode_payload())
    );
    assert_eq!(
        session.send_exe_request(),
        ReliableSequencer::encode_reliable_message(12, &[ExeRequest::TYPE])
    );
    assert_eq!(
        session.send_news_request(),
        ReliableSequencer::encode_reliable_message(13, &[NewsRequest::TYPE])
    );
    let violation = SecurityViolationReport { violation_id: 0x15 };
    assert_eq!(
        session.send_security_violation(&violation),
        ReliableSequencer::encode_reliable_message(14, &violation.encode_payload())
    );
    let checksum_violation = SecurityViolationChecksumReport {
        unknown: 0x0102_0304,
        arena_settings_checksum: 0x1112_1314,
        code_checksum_1: 0x2122_2324,
        code_checksum_2: 0x3132_3334,
        violation_id: 0x1b,
    };
    assert_eq!(
        session.send_security_violation_checksum(&checksum_violation),
        ReliableSequencer::encode_reliable_message(15, &checksum_violation.encode_payload())
    );
    let settings_change = ArenaSettingsChangeRequest::new(["Warbird:MaximumEnergy:2800"]);
    assert_eq!(
        session.send_arena_settings_change(&settings_change),
        ReliableSequencer::encode_reliable_message(16, &settings_change.encode_payload())
    );
    assert_eq!(
        session.send_drop_attached(),
        ReliableSequencer::encode_reliable_message(17, &[DropAttachedRequest::TYPE])
    );
    assert_eq!(
        session.send_drop_flags(),
        ReliableSequencer::encode_reliable_message(18, &[DropFlagsRequest::TYPE])
    );
    assert_eq!(
        session.send_end_koth_timer(),
        ReliableSequencer::encode_reliable_message(19, &[EndKothTimerRequest::TYPE])
    );
    assert_eq!(
        session.try_send_ship_change(4, None),
        Some((
            20,
            ReliableSequencer::encode_reliable_message(20, &[ShipChangeRequest::TYPE, 4])
        ))
    );
    assert_eq!(
        ShipChangeRequest {
            ship: 4,
            position_sequence_base: Some(0x1234),
        }
        .encode_payload(),
        [ShipChangeRequest::TYPE, 4, 0x34, 0x12]
    );
    let mut banner = [0; PLAYER_BANNER_BYTES];
    banner[0] = 7;
    banner[95] = 9;
    let set_banner = SetBannerRequest { banner };
    assert_eq!(
        session.send_set_banner(&set_banner),
        ReliableSequencer::encode_reliable_message(21, &set_banner.encode_payload())
    );
    let take_prize = TakePrizeRequest {
        collectible_id: 0x0102_0304,
        x: 44,
        y: 55,
        prize_id: -21,
    };
    let take_prize_payload = take_prize.encode_payload();
    assert_eq!(
        session.send_take_prize(&take_prize, false),
        take_prize_payload
    );
    assert_eq!(
        session.send_take_prize(&take_prize, true),
        ReliableSequencer::encode_reliable_message(22, &take_prize_payload)
    );
    let drop_brick = DropBrickRequest { x: 100, y: 200 };
    assert_eq!(
        session.send_drop_brick(&drop_brick),
        ReliableSequencer::encode_reliable_message(23, &drop_brick.encode_payload())
    );
    let pickup = BallPickupRequest {
        ball_id: 2,
        timestamp: 0x1122_3344,
    };
    assert_eq!(
        session.send_ball_pickup(&pickup),
        ReliableSequencer::encode_reliable_message(24, &pickup.encode_payload())
    );
    let fire = BallFireRequest {
        ball_id: 3,
        x: 16,
        y: 32,
        velocity_x: -120,
        velocity_y: 240,
        player_id: 9,
        timestamp: 0x0102_0304,
    };
    assert_eq!(
        session.send_ball_fire(&fire),
        ReliableSequencer::encode_reliable_message(25, &fire.encode_payload())
    );
    let goal = BallGoalRequest {
        ball_id: 4,
        x: -5,
        y: 6,
    };
    assert_eq!(
        session.send_ball_goal(&goal),
        ReliableSequencer::encode_reliable_message(26, &goal.encode_payload())
    );
}

#[test]
fn client_protocol_session_sends_unreliable_position_updates() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let update = ClientPositionUpdate {
        direction: 4,
        timestamp: 0x1122_3344,
        x: 0x1234,
        y: 0x5678,
        velocity_x: -120,
        velocity_y: 240,
        togglables: 0,
        bounty: 77,
        energy: 900,
        weapon: 0,
        extra: None,
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            4, 0x1234, 0x5678, -120, 240,
        ),
        input_history: None,
        warp_collectible_id: None,
        repel_application: None,
    };

    let packet = session.send_position_update(&update);

    assert_eq!(packet, update.encode());
    assert!(session.sequencer.sent.is_empty());
}

#[test]
fn client_protocol_session_reliably_wraps_life_reset_position() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let update = ClientPositionUpdate {
        direction: 0,
        timestamp: 0,
        x: 8_200,
        y: 8_200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        bounty: 500,
        energy: 1_700,
        weapon: 0,
        extra: Some(ClientPositionExtra::default()),
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            0, 8_200, 8_200, 0, 0,
        ),
        input_history: Some(ClientInputRecordPacket {
            base_seq: 123,
            inputs: vec![PlayerInputRecord::default()],
        }),
        warp_collectible_id: None,
        repel_application: None,
    };

    let payload = session.send_position_update(&update);
    let (reliable_id, packet) = session
        .try_send_reliable_position_update(&update)
        .expect("lifecycle reliable table should have capacity");

    assert_eq!(reliable_id, 0);
    assert_eq!(
        packet,
        ReliableSequencer::encode_reliable_message(0, &payload)
    );
    assert_eq!(session.sequencer.sent.len(), 1);
}

#[test]
fn reliable_lifecycle_prefix_barrier_is_ordered_and_wrap_safe() {
    let mut sequencer = ReliableSequencer {
        next_outbound_id: u32::MAX - 1,
        ..ReliableSequencer::default()
    };
    for payload in [b"before".as_slice(), b"gap", b"reset", b"after"] {
        sequencer.send_reliable(payload);
    }

    // The reset is ID 0. Its own early and duplicate ACKs cannot release the
    // barrier while MAX-1/MAX are still missing. The later ordinary ID 1 is
    // deliberately left pending and must not hold the prefix once it clears.
    assert!(
        sequencer
            .receive_ack(&ReliableSequencer::encode_ack(0))
            .unwrap()
    );
    assert!(
        !sequencer
            .receive_ack(&ReliableSequencer::encode_ack(0))
            .unwrap()
    );
    assert!(!sequencer.outbound_prefix_acked_through(0));

    sequencer
        .receive_ack(&ReliableSequencer::encode_ack(u32::MAX - 1))
        .unwrap();
    assert!(!sequencer.outbound_prefix_acked_through(0));
    sequencer
        .receive_ack(&ReliableSequencer::encode_ack(u32::MAX))
        .unwrap();
    assert!(sequencer.outbound_prefix_acked_through(0));
    assert_eq!(
        sequencer
            .sent
            .iter()
            .map(|packet| packet.id)
            .collect::<Vec<_>>(),
        [1]
    );
}

#[test]
fn client_protocol_session_chunks_oversized_voice_and_file_transfers() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let voice = VoiceUploadRequest {
        index: 2,
        player_id: 9,
        wav_data: vec![0x55; ReliableSequencer::MAX_RELIABLE_PAYLOAD_SIZE],
    };
    let voice_payload = voice.encode_payload();

    let voice_packets = session.send_voice_upload_packets(&voice);
    assert_eq!(
        voice_packets,
        reliable_small_chunk_packets(0, &voice_payload)
    );
    assert!(
        voice_packets
            .iter()
            .all(|packet| packet.len() <= ReliableSequencer::MAX_PACKET_SIZE)
    );

    let file = ClientFileTransferRequest {
        filename: "profile.dat".to_string(),
        data: vec![0xaa; ReliableSequencer::MAX_RELIABLE_PAYLOAD_SIZE],
    };
    let file_payload = file.encode_payload();
    let file_packets = session.send_client_file_transfer_packets(&file);
    assert_eq!(file_packets, reliable_small_chunk_packets(2, &file_payload));
    assert!(
        file_packets
            .iter()
            .all(|packet| packet.len() <= ReliableSequencer::MAX_PACKET_SIZE)
    );
}

#[test]
fn interleaved_server_file_fragments_survive_loss_gap_and_roster_control() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.pending_file_requests.push_back(FileRequestSpec {
        zone_id: Some("chaos-zone".to_string()),
        filename: "test.lvz".to_string(),
        index: 1,
        size: 5,
        size_is_known: true,
        checksum: 0,
        sha256: None,
        decompress: false,
    });

    let mut logical = vec![0; ServerFileTransfer::HEADER_LEN];
    logical[0] = ServerFileTransfer::TYPE;
    logical[1..9].copy_from_slice(b"test.lvz");
    logical.extend_from_slice(b"hello");
    let split = ServerFileTransfer::HEADER_LEN + 2;
    let first = ServerFileTransferChunk::encode(1, logical.len(), 0, &logical[..split]).unwrap();
    let second =
        ServerFileTransferChunk::encode(1, logical.len(), split, &logical[split..]).unwrap();

    let mut entering = vec![0; PlayerRosterEntry::PACKET_LEN];
    entering[0] = PlayerRosterEntry::TYPE;
    entering[1..6].copy_from_slice(b"alpha");
    entering[51..53].copy_from_slice(&7_u16.to_le_bytes());

    // The first file fragment is lost. Both the later roster control and tail
    // arrive inside the bounded reorder window and remain retained.
    let roster_actions = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(1, &entering))
        .unwrap();
    assert!(roster_actions.events.is_empty());
    let tail_actions = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(2, &second))
        .unwrap();
    assert!(tail_actions.events.is_empty());

    // Recovery of the exact gap drains fragment -> roster -> fragment. The
    // independent file framing survives that control boundary and publishes
    // the byte-identical legacy transfer only after complete reassembly.
    let recovered = session
        .handle_transport_packet(&ReliableSequencer::encode_reliable_message(0, &first))
        .unwrap();
    assert_eq!(recovered.events.len(), 2);
    assert!(matches!(
        &recovered.events[0],
        ClientProtocolEvent::PlayerEntered(entry) if entry.player_id == 7
    ));
    assert_eq!(
        recovered.events[1],
        ClientProtocolEvent::FileTransferred(ServerFileTransfer {
            filename: "test.lvz".to_string(),
            data: b"hello".to_vec(),
        })
    );
}

#[test]
fn client_protocol_session_sends_extra_position_tail_when_spectate_requests_it() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let update = ClientPositionUpdate {
        direction: 4,
        timestamp: 0x1122_3344,
        x: 0x1234,
        y: 0x5678,
        velocity_x: -120,
        velocity_y: 240,
        togglables: 0,
        bounty: 77,
        energy: 900,
        weapon: 0,
        extra: None,
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            4, 0x1234, 0x5678, -120, 240,
        ),
        input_history: None,
        warp_collectible_id: None,
        repel_application: None,
    };
    session
        .handle_server_packet(&[SpectatePacket::TYPE, 1])
        .expect("spectate should enable extra info");

    let packet = session.send_position_update(&update);

    assert_eq!(
        packet.len(),
        crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET
    );
    assert_eq!(read_u16(&packet, 22).unwrap(), 900);
    assert_eq!(read_u16(&packet, 24).unwrap(), 0);
    assert_eq!(read_u16(&packet, 26).unwrap(), 0);
    assert_eq!(read_u32(&packet, 28).unwrap(), 0);
    assert_eq!(
        crate::continuum_position::client_position_canonical_state(&packet),
        Some(update.canonical_state)
    );
    assert_eq!(weapon_checksum(&packet), 0);

    session
        .handle_server_packet(&[SpectatePacket::TYPE, 0])
        .expect("spectate should disable extra info");
    assert_eq!(session.send_position_update(&update), update.encode());
}

#[test]
fn chat_message_request_encodes_reliable_public_payload() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let request = ChatMessageRequest::public("hello");

    assert_eq!(
        request.encode_payload(),
        vec![
            ChatMessageRequest::TYPE,
            2,
            0,
            0,
            0,
            b'h',
            b'e',
            b'l',
            b'l',
            b'o',
            0
        ]
    );
    assert_eq!(
        session.send_chat_message(&request),
        ReliableSequencer::encode_reliable_message(0, &request.encode_payload())
    );
}

#[test]
fn chat_message_request_encodes_team_private_and_sound_fields() {
    assert_eq!(
        ChatMessageRequest::team("team").encode_payload(),
        vec![
            ChatMessageRequest::TYPE,
            3,
            0,
            0,
            0,
            b't',
            b'e',
            b'a',
            b'm',
            0
        ]
    );
    assert_eq!(
        ChatMessageRequest::private(42, "hi")
            .with_sound(7)
            .encode_payload(),
        vec![ChatMessageRequest::TYPE, 5, 7, 42, 0, b'h', b'i', 0]
    );
}

// Phase 3e-4a: client emits a SnapshotAckPacket every
// SNAPSHOT_ACK_INTERVAL received snapshots and records the latest id.
fn snapshot_frame_bytes(snapshot_id: u16) -> Vec<u8> {
    snapshot_frame_bytes_with_header(snapshot_id, 0, u32::from(snapshot_id) * 10)
}

fn snapshot_frame_bytes_with_header(snapshot_id: u16, base_id: u16, tick: u32) -> Vec<u8> {
    SnapshotFrame {
        header: SnapshotHeader {
            flags: if base_id == 0 { SNAPSHOT_FLAG_FULL } else { 0 },
            snapshot_id,
            base_id,
            tick,
            arena_stream_id: 0,
            near_count: 0,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![],
        mid_entities: vec![],
        events: vec![],
    }
    .encode()
}

#[test]
fn prize_references_expand_into_the_full_prize_state_they_name() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let body = SnapshotPrizeState {
        life_seq: 3,
        revision: 4,
        bounty: 25,
        max_energy: 1_700,
        energy: Some(1_700),
        ..SnapshotPrizeState::default()
    };
    let frame = |snapshot_id: u16, base_id: u16, event: SnapshotEvent| {
        SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_COMPACT_PRIVATE
                    | if base_id == 0 { SNAPSHOT_FLAG_FULL } else { 0 },
                snapshot_id,
                base_id,
                tick: u32::from(snapshot_id) * 10,
                event_count: 1,
                ..SnapshotHeader::default()
            },
            events: vec![event],
            ..SnapshotFrame::default()
        }
        .encode()
    };
    let received_prize = |actions: &ClientProtocolActions| {
        actions.events.iter().find_map(|event| match event {
            ClientProtocolEvent::SnapshotReceived(frame) => Some(
                frame
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        SnapshotEvent::PrizeState(prize) => Some(*prize),
                        SnapshotEvent::PrizeReference(_) => {
                            panic!("references never leave the decoder")
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
    };
    let full = session
        .handle_server_packet(&frame(10, 0, SnapshotEvent::PrizeState(body)))
        .unwrap();
    assert_eq!(received_prize(&full), Some(vec![body]));

    let referenced = session
        .handle_server_packet(&frame(
            11,
            10,
            SnapshotEvent::PrizeReference(SnapshotPrizeReference {
                snapshot_id: 10,
                energy: 640,
            }),
        ))
        .unwrap();
    assert_eq!(
        received_prize(&referenced),
        Some(vec![SnapshotPrizeState {
            energy: Some(640),
            ..body
        }])
    );

    // A body this session never held yields no prize state this frame.
    let unknown = session
        .handle_server_packet(&frame(
            12,
            10,
            SnapshotEvent::PrizeReference(SnapshotPrizeReference {
                snapshot_id: 9,
                energy: 1,
            }),
        ))
        .unwrap();
    assert_eq!(received_prize(&unknown), Some(vec![]));
}

#[test]
fn repel_bearing_snapshot_is_acknowledged_immediately_by_exact_id() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut entity = full_near_entity(9, 500, 700);
    entity.weapon = Some(5);
    let frame = SnapshotFrame {
        header: SnapshotHeader {
            flags: SNAPSHOT_FLAG_FULL,
            snapshot_id: u16::MAX - 1,
            base_id: 0,
            tick: 123,
            arena_stream_id: 0,
            near_count: 1,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![entity],
        mid_entities: vec![],
        events: vec![],
    };

    let actions = session.handle_server_packet(&frame.encode()).unwrap();
    assert!(actions.events.iter().any(|event| matches!(
        event,
        ClientProtocolEvent::SnapshotPlayerPosition { update: position, .. }
            if position.player_id == 9 && position.weapon == 5
    )));
    assert_eq!(actions.outbound_packets.len(), 1);
    let ack = SnapshotAckPacket::parse(&actions.outbound_packets[0][6..]).unwrap();
    assert_eq!(ack.snapshot_id, u16::MAX - 1);
    assert_eq!(session.latest_repel_snapshot_id, Some(u16::MAX - 1));
    assert_eq!(session.snapshots_since_last_ack, 0);

    session
        .handle_server_packet(&[ServerPacket::PLAYER_ID, 7, 0])
        .unwrap();
    assert_eq!(session.latest_repel_snapshot_id, None);
}

#[test]
fn self_contained_private_repel_lifecycle_is_acked_without_a_source_entity() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let delivery = SnapshotRepelDelivery {
        active_steps: 134,
        delivery_generation: u32::MAX,
        source_player_id: 9,
        source_life_seq: 7,
        origin_x: 500,
        origin_y: 700,
        source_frequency: 2,
        expected_velocity_x_bits: 500.0_f32.to_bits(),
        expected_velocity_y_bits: (-1.25_f32).to_bits(),
        weapon_data: 5,
        source_ship: 3,
    };
    let frame = SnapshotFrame {
        header: SnapshotHeader {
            flags: SNAPSHOT_FLAG_FULL,
            snapshot_id: u16::MAX,
            base_id: 0,
            tick: 123,
            arena_stream_id: 0,
            near_count: 0,
            mid_count: 0,
            event_count: 1,
        },
        events: vec![SnapshotEvent::LifecycleState(SnapshotLifecycleState {
            life_seq: 11,
            alive: true,
            respawn_at_tick: None,
            repel_until_tick: Some(325),
            repel_delivery: Some(delivery),
        })],
        ..SnapshotFrame::default()
    };

    let actions = session.handle_server_packet(&frame.encode()).unwrap();
    assert!(actions.events.iter().any(|event| matches!(
        event,
        ClientProtocolEvent::SnapshotReceived(parsed)
            if parsed.near_entities.is_empty()
                && parsed.events.iter().any(|event| matches!(
                    event,
                    SnapshotEvent::LifecycleState(lifecycle)
                        if lifecycle.repel_delivery == Some(delivery)
                ))
    )));
    assert_eq!(actions.outbound_packets.len(), 1);
    let ack = SnapshotAckPacket::parse(&actions.outbound_packets[0][6..]).unwrap();
    assert_eq!(ack.snapshot_id, u16::MAX);
    assert_eq!(session.latest_repel_snapshot_id, Some(u16::MAX));
    assert_eq!(session.snapshots_since_last_ack, 0);
}

#[test]
fn mid_tier_snapshot_entities_synthesize_radar_position_updates() {
    // [REVIEW H4] Mid-tier (radar-range) entities must produce a
    // position-grade update so a never-near player still draws a radar
    // dot. The synthesized update is radar-only: zero velocity, no
    // fabricated energy, and no weapon visual.
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let frame = SnapshotFrame {
        header: SnapshotHeader {
            flags: SNAPSHOT_FLAG_FULL,
            snapshot_id: 1,
            base_id: 0,
            tick: 40,
            arena_stream_id: 0,
            near_count: 0,
            mid_count: 1,
            event_count: 0,
        },
        near_entities: vec![],
        mid_entities: vec![SnapshotEntity::full(9, 5_000, 6_000)],
        events: vec![],
    };
    let actions = session
        .handle_server_packet(&frame.encode())
        .expect("snapshot frame should parse");
    let update = actions
        .events
        .iter()
        .find_map(|event| match event {
            ClientProtocolEvent::SnapshotPlayerPosition { update, .. } if update.player_id == 9 => {
                Some(update)
            }
            _ => None,
        })
        .expect("a mid entity must synthesize a radar position update");
    assert_eq!(update.x, 5_000);
    assert_eq!(update.y, 6_000);
    assert_eq!(update.velocity_x, 0, "mid wire carries no velocity");
    assert_eq!(update.velocity_y, 0);
    assert_eq!(update.weapon, 0, "no remote fire visual from radar range");
    assert_eq!(update.energy, None, "mid range must not fabricate energy");
}

fn near_snapshot_frame(
    snapshot_id: u16,
    base_id: u16,
    flags: u8,
    entity: SnapshotEntity,
) -> Vec<u8> {
    SnapshotFrame {
        header: SnapshotHeader {
            flags,
            snapshot_id,
            base_id,
            tick: u32::from(snapshot_id),
            arena_stream_id: 0,
            near_count: 1,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![entity],
        mid_entities: vec![],
        events: vec![],
    }
    .encode()
}

fn full_near_entity(entity_id: u16, x: u16, y: u16) -> SnapshotEntity {
    SnapshotEntity::near_full(
        entity_id,
        SnapshotEntityState {
            x,
            y,
            ..SnapshotEntityState::default()
        },
    )
}

fn player_position_x(actions: &ClientProtocolActions, player_id: u16) -> Option<u16> {
    actions.events.iter().find_map(|event| match event {
        ClientProtocolEvent::SnapshotPlayerPosition { update, .. }
            if update.player_id == player_id =>
        {
            Some(update.x)
        }
        _ => None,
    })
}

fn player_position_status(actions: &ClientProtocolActions, player_id: u16) -> Option<u8> {
    actions.events.iter().find_map(|event| match event {
        ClientProtocolEvent::SnapshotPlayerPosition { update, .. }
            if update.player_id == player_id =>
        {
            Some(update.togglables)
        }
        _ => None,
    })
}

#[test]
fn session_applies_return_to_base_delta_against_referenced_history() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    let initial = near_snapshot_frame(1, 0, SNAPSHOT_FLAG_FULL, full_near_entity(7, 100, 200));
    let changed = near_snapshot_frame(2, 1, 0, SnapshotEntity::full(7, 900, 200));
    let returned = near_snapshot_frame(3, 1, 0, SnapshotEntity::unchanged(7));

    let first = session.handle_server_packet(&initial).unwrap();
    let second = session.handle_server_packet(&changed).unwrap();
    let third = session.handle_server_packet(&returned).unwrap();

    assert_eq!(player_position_x(&first, 7), Some(100));
    assert_eq!(player_position_x(&second, 7), Some(900));
    assert_eq!(
        player_position_x(&third, 7),
        Some(100),
        "an omitted field must return to the referenced base, not last-received carry"
    );
}

#[test]
fn session_status_returns_to_referenced_base_without_replaying_flash() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut initial_entity = full_near_entity(7, 100, 200);
    initial_entity.status = Some(STATUS_SAFETY);
    let changed_entity = SnapshotEntity {
        entity_id: 7,
        field_mask: SnapshotEntity::FIELD_STATUS,
        status: Some(0),
        ..SnapshotEntity::default()
    };

    let first = session
        .handle_server_packet(&near_snapshot_frame(
            1,
            0,
            SNAPSHOT_FLAG_FULL,
            initial_entity,
        ))
        .unwrap();
    let second = session
        .handle_server_packet(&near_snapshot_frame(2, 1, 0, changed_entity))
        .unwrap();
    let returned = session
        .handle_server_packet(&near_snapshot_frame(3, 1, 0, SnapshotEntity::unchanged(7)))
        .unwrap();

    assert_eq!(player_position_status(&first, 7), Some(STATUS_SAFETY));
    assert_eq!(player_position_status(&second, 7), Some(0));
    assert_eq!(
        player_position_status(&returned, 7),
        Some(STATUS_SAFETY),
        "persistent status must return to the exact referenced base"
    );
    assert_eq!(
        player_position_status(&returned, 7).unwrap() & STATUS_FLASH,
        0,
        "an omitted status field must not replay a transient flash"
    );
}

#[test]
fn session_drops_unknown_base_and_rate_limits_full_requests() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.set_outbound_protocol_tick(100);
    session.set_local_monotonic_micros(1_000_000);
    session
        .handle_server_packet(&near_snapshot_frame(
            1,
            0,
            SNAPSHOT_FLAG_FULL,
            full_near_entity(7, 100, 200),
        ))
        .unwrap();

    let missing_base = near_snapshot_frame(3, 2, 0, SnapshotEntity::unchanged(7));
    let first = session.handle_server_packet(&missing_base).unwrap();
    assert_eq!(session.latest_received_snapshot_id, Some(1));
    assert!(
        first.events.is_empty(),
        "unknown-base frame must be dropped"
    );
    assert_eq!(first.outbound_packets.len(), 1);
    assert_eq!(
        &first.outbound_packets[0][6..],
        SnapshotFullRequestPacket.encode().as_slice()
    );

    let duplicate = session.handle_server_packet(&missing_base).unwrap();
    assert!(
        duplicate.outbound_packets.is_empty(),
        "duplicate request inside the cooldown must be suppressed"
    );

    session.set_local_monotonic_micros(1_000_000 + SNAPSHOT_FULL_REQUEST_COOLDOWN_MICROS - 1);
    let just_before_cooldown = session.handle_server_packet(&missing_base).unwrap();
    assert!(just_before_cooldown.outbound_packets.is_empty());

    session.set_local_monotonic_micros(1_000_000 + SNAPSHOT_FULL_REQUEST_COOLDOWN_MICROS);
    let after_cooldown = session.handle_server_packet(&missing_base).unwrap();
    assert_eq!(after_cooldown.outbound_packets.len(), 1);

    let recovery = near_snapshot_frame(4, 0, SNAPSHOT_FLAG_FULL, full_near_entity(7, 400, 200));
    let recovered = session.handle_server_packet(&recovery).unwrap();
    assert_eq!(player_position_x(&recovered, 7), Some(400));
    assert_eq!(session.latest_received_snapshot_id, Some(4));
}

#[test]
fn snapshot_recovery_cooldown_survives_no_inbound_until_half_range_or_beyond() {
    const FIRST_REQUEST_MICROS: u64 = 5_000_000;
    const REQUEST_TICK: u32 = 100;

    for gameplay_delta in [1_u32 << 30, (1_u32 << 30) + 73] {
        let mut session =
            ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
        session.set_outbound_protocol_tick(REQUEST_TICK);
        session.set_local_monotonic_micros(FIRST_REQUEST_MICROS);
        session
            .handle_server_packet(&near_snapshot_frame(
                1,
                0,
                SNAPSHOT_FLAG_FULL,
                full_near_entity(7, 100, 200),
            ))
            .unwrap();
        let missing_base = near_snapshot_frame(3, 2, 0, SnapshotEntity::unchanged(7));
        assert_eq!(
            session
                .handle_server_packet(&missing_base)
                .unwrap()
                .outbound_packets
                .len(),
            1
        );

        // This is the first inbound packet since the request. The wrapped
        // gameplay tick is now exactly/beyond the signed half-range, but the
        // local transport deadline remains unambiguous and is due.
        session.set_outbound_protocol_tick(crate::continuum::continuum_tick_add(
            REQUEST_TICK,
            gameplay_delta,
        ));
        session.set_local_monotonic_micros(
            FIRST_REQUEST_MICROS + SNAPSHOT_FULL_REQUEST_COOLDOWN_MICROS,
        );
        assert_eq!(
            session
                .handle_server_packet(&missing_base)
                .unwrap()
                .outbound_packets
                .len(),
            1,
            "no-intermediate recovery must be due after gameplay delta {gameplay_delta}"
        );
    }
}

#[test]
fn reordered_gameplay_ticks_cannot_reenable_snapshot_recovery_spam_and_reset_clears_it() {
    const FIRST_REQUEST_MICROS: u64 = 8_000_000;
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.set_outbound_protocol_tick(100);
    session.set_local_monotonic_micros(FIRST_REQUEST_MICROS);
    session
        .handle_server_packet(&near_snapshot_frame(
            1,
            0,
            SNAPSHOT_FLAG_FULL,
            full_near_entity(7, 100, 200),
        ))
        .unwrap();
    let missing_base = near_snapshot_frame(3, 2, 0, SnapshotEntity::unchanged(7));
    assert_eq!(
        session
            .handle_server_packet(&missing_base)
            .unwrap()
            .outbound_packets
            .len(),
        1
    );

    session.set_local_monotonic_micros(FIRST_REQUEST_MICROS + 1);
    for reordered_tick in [99, crate::continuum::continuum_tick_add(100, 1 << 30), 7] {
        session.set_outbound_protocol_tick(reordered_tick);
        assert!(
            session
                .handle_server_packet(&missing_base)
                .unwrap()
                .outbound_packets
                .is_empty(),
            "reordered gameplay tick {reordered_tick} must not bypass the local deadline"
        );
    }

    // An accidentally stale injected clock is clamped forward too.
    session.set_local_monotonic_micros(FIRST_REQUEST_MICROS - 1);
    assert_eq!(session.local_monotonic_micros, FIRST_REQUEST_MICROS + 1);
    assert!(
        session
            .handle_server_packet(&missing_base)
            .unwrap()
            .outbound_packets
            .is_empty()
    );

    // A new player/session assignment resets recovery state immediately.
    session
        .handle_server_packet(&[ServerPacket::PLAYER_ID, 7, 0])
        .unwrap();
    assert!(session.last_snapshot_full_request_micros.is_none());
    assert_eq!(
        session
            .handle_server_packet(&missing_base)
            .unwrap()
            .outbound_packets
            .len(),
        1
    );
}

#[test]
fn snapshot_recovery_cooldown_remains_suppressed_at_saturated_monotonic_time() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session
        .handle_server_packet(&near_snapshot_frame(
            1,
            0,
            SNAPSHOT_FLAG_FULL,
            full_near_entity(7, 100, 200),
        ))
        .unwrap();
    let missing_base = near_snapshot_frame(3, 2, 0, SnapshotEntity::unchanged(7));
    let near_max = u64::MAX - SNAPSHOT_FULL_REQUEST_COOLDOWN_MICROS + 1;
    session.set_local_monotonic_micros(near_max);
    assert_eq!(
        session
            .handle_server_packet(&missing_base)
            .unwrap()
            .outbound_packets
            .len(),
        1
    );

    session.set_local_monotonic_micros(u64::MAX);
    for _ in 0..3 {
        assert!(
            session
                .handle_server_packet(&missing_base)
                .unwrap()
                .outbound_packets
                .is_empty(),
            "a clock frozen at u64::MAX must not repeatedly enable recovery"
        );
    }

    session.set_local_monotonic_micros(near_max - 1);
    assert_eq!(session.local_monotonic_micros, u64::MAX);
    assert!(
        session
            .handle_server_packet(&missing_base)
            .unwrap()
            .outbound_packets
            .is_empty(),
        "backward injection must remain clamped at the saturated clock"
    );

    session
        .handle_server_packet(&[ServerPacket::PLAYER_ID, 7, 0])
        .unwrap();
    assert!(session.last_snapshot_full_request_micros.is_none());
    assert_eq!(
        session
            .handle_server_packet(&missing_base)
            .unwrap()
            .outbound_packets
            .len(),
        1,
        "reset must permit one request even when local time is exactly u64::MAX"
    );
    assert!(
        session
            .handle_server_packet(&missing_base)
            .unwrap()
            .outbound_packets
            .is_empty(),
        "repeated exact-max time must remain inside the new cooldown"
    );
}

#[test]
fn session_keeps_unique_damage_event_when_entity_base_is_unknown() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.set_outbound_protocol_tick(100);
    let frame = SnapshotFrame {
        header: SnapshotHeader {
            flags: 0,
            snapshot_id: 3,
            base_id: 2,
            tick: 12345,
            arena_stream_id: 0,
            near_count: 0,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![],
        mid_entities: vec![],
        events: vec![SnapshotEvent::WatchDamage(SnapshotWatchDamage {
            victim_id: 7,
            shooter_id: 11,
            weapon_data: 0x42,
            damage: 400,
            energy_before: Some(400),
            energy_after: 0,
            server_tick: Some(12340),
            damage_seq: Some(77),
            projectile_id: Some(99),
            victim_life_seq: Some(5),
            shooter_life_seq: Some(2),
            lethal: true,
        })],
    };

    let first = session.handle_server_packet(&frame.encode()).unwrap();
    assert_eq!(session.latest_received_snapshot_id, None);
    assert_eq!(first.outbound_packets.len(), 1, "resync must be requested");
    assert_eq!(
        first
            .events
            .iter()
            .filter(|event| matches!(event, ClientProtocolEvent::DamageWatched(_)))
            .count(),
        1,
        "self-contained damage must survive entity-state resync"
    );
    assert!(
        first
            .events
            .iter()
            .all(|event| !matches!(event, ClientProtocolEvent::SnapshotReceived(_))),
        "unknown-base entity state must stay dropped"
    );

    let duplicate = session.handle_server_packet(&frame.encode()).unwrap();
    assert!(duplicate.outbound_packets.is_empty());
    assert!(
        duplicate
            .events
            .iter()
            .all(|event| !matches!(event, ClientProtocolEvent::DamageWatched(_))),
        "damage_seq dedupe must still apply on the resync path"
    );
}

#[test]
fn session_history_retains_server_active_base_during_ack_backlog() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session
        .handle_server_packet(&near_snapshot_frame(
            1,
            0,
            SNAPSHOT_FLAG_FULL,
            full_near_entity(7, 100, 200),
        ))
        .unwrap();

    for snapshot_id in 2..=50 {
        let actions = session
            .handle_server_packet(&near_snapshot_frame(
                snapshot_id,
                1,
                0,
                SnapshotEntity::unchanged(7),
            ))
            .unwrap();
        assert_eq!(player_position_x(&actions, 7), Some(100));
        assert!(
            actions
                .outbound_packets
                .iter()
                .all(|packet| packet.get(6) != Some(&SNAPSHOT_FULL_REQUEST_PACKET)),
            "retained server base must not trigger a full request"
        );
    }

    assert_eq!(session.snapshot_history.len(), SNAPSHOT_HISTORY_CAPACITY);
    assert!(
        session
            .snapshot_history
            .iter()
            .any(|entry| entry.snapshot_id == 1),
        "the base named by incoming deltas must survive newer locally-sent ACKs"
    );
}

#[test]
fn session_retains_acked_full_base_when_later_full_was_already_in_flight() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    for snapshot_id in 2075..=2079 {
        session
            .handle_server_packet(&near_snapshot_frame(
                snapshot_id,
                0,
                SNAPSHOT_FLAG_FULL,
                full_near_entity(7, snapshot_id, 200),
            ))
            .unwrap();
    }
    assert_eq!(session.latest_acked_snapshot_id, Some(2079));

    session
        .handle_server_packet(&near_snapshot_frame(
            2080,
            0,
            SNAPSHOT_FLAG_FULL,
            full_near_entity(7, 2080, 200),
        ))
        .unwrap();
    assert!(
        session
            .snapshot_history
            .iter()
            .any(|entry| entry.snapshot_id == 2079),
        "an in-flight FULL must not delete the base just acknowledged to the server"
    );

    let delta = session
        .handle_server_packet(&near_snapshot_frame(
            2081,
            2079,
            0,
            SnapshotEntity::unchanged(7),
        ))
        .unwrap();
    assert_eq!(
        player_position_x(&delta, 7),
        Some(2079),
        "the subsequent server delta must reconstruct against ACKed FULL 2079"
    );
    assert!(
        delta
            .outbound_packets
            .iter()
            .all(|packet| packet.get(6) != Some(&SNAPSHOT_FULL_REQUEST_PACKET)),
        "a retained ACK-transit base must not trigger a full resync request"
    );
}

#[test]
fn session_schedules_snapshot_ack_every_interval_frames() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    // First N-1 snapshots should not produce an outbound ack.
    for snapshot_id in 1..crate::protocol::SNAPSHOT_ACK_INTERVAL as u16 {
        let actions = session
            .handle_server_packet(&snapshot_frame_bytes(snapshot_id))
            .expect("snapshot frame should parse");
        assert!(
            actions.outbound_packets.is_empty(),
            "snapshot {snapshot_id} should not yet trigger an ack"
        );
    }
    assert_eq!(
        session.latest_received_snapshot_id,
        Some(crate::protocol::SNAPSHOT_ACK_INTERVAL as u16 - 1)
    );
    assert_eq!(
        session.latest_received_snapshot_server_tick,
        Some((crate::protocol::SNAPSHOT_ACK_INTERVAL as u32 - 1) * 10)
    );
    assert_eq!(session.latest_received_snapshot_base_id, Some(0));
    assert_eq!(session.latest_acked_snapshot_id, None);
    assert_eq!(session.latest_acked_snapshot_server_tick, None);

    // Nth snapshot should produce an ack of THIS snapshot id, wrapped
    // in a reliable frame.
    let nth_snapshot_id = crate::protocol::SNAPSHOT_ACK_INTERVAL as u16;
    let actions = session
        .handle_server_packet(&snapshot_frame_bytes_with_header(
            nth_snapshot_id,
            nth_snapshot_id - 1,
            12345,
        ))
        .expect("snapshot frame should parse");
    assert_eq!(
        actions.outbound_packets.len(),
        1,
        "Nth snapshot should emit exactly one outbound packet (the ack)"
    );

    // Decode the reliable wrapper (`0x00 | 0x03 | u32 id | payload`)
    // and verify the inner payload is a SnapshotAckPacket with the
    // latest snapshot_id.
    let outbound = &actions.outbound_packets[0];
    assert_eq!(outbound[0], 0x00, "reliable core byte");
    assert_eq!(outbound[1], 0x03, "reliable message type");
    let payload = &outbound[6..];
    let ack = SnapshotAckPacket::parse(payload).expect("ack should parse");
    assert_eq!(ack.snapshot_id, nth_snapshot_id,);
    assert_eq!(session.latest_received_snapshot_id, Some(nth_snapshot_id));
    assert_eq!(session.latest_received_snapshot_server_tick, Some(12345));
    assert_eq!(
        session.latest_received_snapshot_base_id,
        Some(nth_snapshot_id - 1)
    );
    assert_eq!(session.latest_acked_snapshot_id, Some(nth_snapshot_id));
    assert_eq!(session.latest_acked_snapshot_server_tick, Some(12345));
    // Counter resets after sending.
    assert_eq!(session.snapshots_since_last_ack, 0);
}

#[test]
fn snapshot_ack_proves_protocol_consumption_before_world_application() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(PlayerRosterEntry {
        ship: 0,
        audio: 0,
        name: "remote".to_owned(),
        squad: String::new(),
        kill_points: 0,
        flag_points: 0,
        player_id: 9,
        frequency: 1,
        wins: 0,
        losses: 0,
        attach_parent: INVALID_PLAYER_ID,
        flags: 0,
        koth: 0,
    }));

    for snapshot_id in 1..crate::protocol::SNAPSHOT_ACK_INTERVAL as u16 {
        let actions = session
            .handle_server_packet(&snapshot_frame_bytes(snapshot_id))
            .expect("snapshot frame should parse");
        assert!(actions.outbound_packets.is_empty());
    }

    let snapshot_id = crate::protocol::SNAPSHOT_ACK_INTERVAL as u16;
    let mut entity = full_near_entity(9, 500, 700);
    entity.life_seq = Some(7);
    let actions = session
        .handle_server_packet(&near_snapshot_frame(
            snapshot_id,
            snapshot_id - 1,
            0,
            entity,
        ))
        .expect("ACK-bearing snapshot should parse");

    let ack = SnapshotAckPacket::parse(&actions.outbound_packets[0][6..])
        .expect("protocol session must queue the periodic codec ACK");
    assert_eq!(ack.snapshot_id, snapshot_id);
    assert_eq!(session.latest_acked_snapshot_id, Some(snapshot_id));
    assert_eq!(world.last_received_snapshot_id, None);
    assert_eq!(world.players[&9].position, None);

    for event in &actions.events {
        world.apply_event(event);
    }
    assert_eq!(world.last_received_snapshot_id, Some(snapshot_id));
    assert_eq!(world.players[&9].life_seq, Some(7));
    assert_eq!(world.players[&9].position, Some(vec2(500.0, 700.0)));
}

#[test]
fn session_synthesizes_damage_watched_event_from_snapshot_watch_damage() {
    // Phase 3e-6: when a snapshot frame carries a
    // SnapshotEvent::WatchDamage, the session inbound dispatch
    // synthesizes a `ClientProtocolEvent::DamageWatched` so the
    // existing notification + energy-update consumers fire
    // identically whether the damage arrived via legacy reliable
    // (flag off) or snapshot events (flag on).
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    let wd = SnapshotWatchDamage {
        victim_id: 7,
        shooter_id: 11,
        weapon_data: 0x42,
        damage: 400,
        energy_before: Some(1700),
        energy_after: 1300,
        server_tick: Some(12340),
        damage_seq: Some(77),
        projectile_id: Some(99),
        victim_life_seq: Some(5),
        shooter_life_seq: Some(2),
        lethal: false,
    };
    let frame = SnapshotFrame {
        header: SnapshotHeader {
            flags: SNAPSHOT_FLAG_FULL,
            snapshot_id: 99,
            base_id: 0,
            tick: 12345,
            arena_stream_id: 0,
            near_count: 0,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![],
        mid_entities: vec![],
        events: vec![SnapshotEvent::WatchDamage(wd)],
    };
    let bytes = frame.encode();
    let actions = session
        .handle_server_packet(&bytes)
        .expect("snapshot parse");

    let damage_watched_count = actions
        .events
        .iter()
        .filter(|e| matches!(e, ClientProtocolEvent::DamageWatched(_)))
        .count();
    assert_eq!(damage_watched_count, 1);

    if let Some(ClientProtocolEvent::DamageWatched(report)) = actions
        .events
        .iter()
        .find(|e| matches!(e, ClientProtocolEvent::DamageWatched(_)))
    {
        assert_eq!(report.player_id, 7);
        assert_eq!(report.timestamp, 12340);
        assert_eq!(report.entries.len(), 1);
        let entry = &report.entries[0];
        assert_eq!(entry.attacker_id, 11);
        assert_eq!(entry.weapon_data, 0x42);
        assert_eq!(entry.damage, 400);
        assert_eq!(entry.energy, 1300);
        assert_eq!(entry.energy_before, Some(1700));
        assert_eq!(entry.damage_seq, Some(77));
        assert_eq!(entry.projectile_id, Some(99));
        assert_eq!(entry.victim_life_seq, Some(5));
        assert_eq!(entry.shooter_life_seq, Some(2));
        assert_eq!(entry.explicit_lethal, Some(false));
        assert_eq!(entry.source_snapshot_id, Some(99));
        assert_eq!(entry.source_snapshot_epoch, Some(0));
    } else {
        panic!("expected DamageWatched in actions");
    }
}

#[test]
fn session_ignores_stale_snapshot_positions_but_keeps_unique_damage_events() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    let first_frame = SnapshotFrame {
        header: SnapshotHeader {
            flags: SNAPSHOT_FLAG_FULL,
            snapshot_id: 10,
            base_id: 0,
            tick: 1_000,
            arena_stream_id: 0,
            near_count: 0,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![full_near_entity(7, 1_000, 2_000)],
        mid_entities: vec![],
        events: vec![],
    };
    let first_actions = session
        .handle_server_packet(&first_frame.encode())
        .expect("fresh snapshot should parse");
    assert_eq!(session.latest_received_snapshot_id, Some(10));
    assert!(first_actions.events.iter().any(|event| {
        matches!(
            event,
            ClientProtocolEvent::SnapshotPlayerPosition { update, .. }
                if update.player_id == 7 && update.x == 1_000 && update.y == 2_000
        )
    }));
    assert!(
        first_actions
            .events
            .iter()
            .any(|event| matches!(event, ClientProtocolEvent::SnapshotReceived(_)))
    );
    let membership_index = first_actions
        .events
        .iter()
        .position(|event| matches!(event, ClientProtocolEvent::SnapshotReceived(_)))
        .unwrap();
    let position_index = first_actions
        .events
        .iter()
        .position(|event| matches!(event, ClientProtocolEvent::SnapshotPlayerPosition { .. }))
        .unwrap();
    assert!(
        membership_index < position_index,
        "same-frame actor identity must bind presentation before position admission",
    );

    let stale_damage = SnapshotWatchDamage {
        victim_id: 7,
        shooter_id: 11,
        weapon_data: 0x42,
        damage: 400,
        energy_before: Some(1700),
        energy_after: 1300,
        server_tick: Some(900),
        damage_seq: Some(88),
        projectile_id: Some(100),
        victim_life_seq: Some(5),
        shooter_life_seq: Some(2),
        lethal: false,
    };
    let stale_frame = SnapshotFrame {
        header: SnapshotHeader {
            flags: 0,
            snapshot_id: 9,
            base_id: 8,
            tick: 900,
            arena_stream_id: 0,
            near_count: 0,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![SnapshotEntity::full(7, 5_000, 6_000)],
        mid_entities: vec![],
        events: vec![SnapshotEvent::WatchDamage(stale_damage)],
    };
    let stale_actions = session
        .handle_server_packet(&stale_frame.encode())
        .expect("stale snapshot should parse");
    assert_eq!(
        session.latest_received_snapshot_id,
        Some(10),
        "stale snapshots must not move the applied snapshot cursor backward"
    );
    assert!(
        stale_actions
            .events
            .iter()
            .all(|event| !matches!(event, ClientProtocolEvent::SnapshotPlayerPosition { .. })),
        "stale snapshot positions must not drive render/AOI state"
    );
    assert!(
        stale_actions
            .events
            .iter()
            .all(|event| !matches!(event, ClientProtocolEvent::SnapshotReceived(_))),
        "stale snapshot lifecycle events must not clear/re-add AOI proxies"
    );
    assert_eq!(
        stale_actions
            .events
            .iter()
            .filter(|event| matches!(event, ClientProtocolEvent::DamageWatched(_)))
            .count(),
        1,
        "a unique server damage event in a late snapshot should still be delivered"
    );

    let duplicate_actions = session
        .handle_server_packet(&stale_frame.encode())
        .expect("duplicate stale snapshot should parse");
    assert!(
        duplicate_actions
            .events
            .iter()
            .all(|event| !matches!(event, ClientProtocolEvent::DamageWatched(_))),
        "damage_seq dedupe must prevent repeated damage delivery from duplicate snapshots"
    );
}

fn realtime_weapon_event_count(actions: &ClientProtocolActions) -> usize {
    actions
        .events
        .iter()
        .filter(|event| matches!(event, ClientProtocolEvent::RealtimeWeaponFired { .. }))
        .count()
}

#[test]
fn authenticated_realtime_weapon_is_generation_bound_wrap_safe_and_duplicate_idempotent() {
    fn bootstrap(arena_stream_id: u64) -> ServerClockBootstrap {
        ServerClockBootstrap {
            server_session_id: 77,
            arena_stream_id,
            server_tick: 100,
            input_sequence_base: 1,
            position_sequence_base: 1,
            weapon_sequence_base: 1,
            reliable_sequence_base: 0,
            tick_rate_hz: 60,
            snapshot_rate_hz: 30,
        }
    }
    fn weapon(arena_stream_id: u64, event_id: u32, x: f32) -> Vec<u8> {
        crate::realtime_event::AuthenticatedRealtimeEvent {
            server_session_id: 77,
            arena_stream_id,
            event_id,
            server_tick: event_id,
            payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                crate::realtime_event::RealtimeWeaponEvent {
                    source_player_id: 7,
                    source_session_id: 55,
                    source_life_seq: 9,
                    shot_group_id: 10,
                    x,
                    y: 8_200.5,
                    velocity_x: 3.25,
                    velocity_y: -1.5,
                    orientation: 0.75,
                    frequency: 42,
                    ship: 3,
                    weapon_data: 0x42,
                },
            ),
        }
        .encode()
    }
    fn watch_damage(arena_stream_id: u64, event_id: u32, damage_seq: u32) -> Vec<u8> {
        crate::realtime_event::AuthenticatedRealtimeEvent {
            server_session_id: 77,
            arena_stream_id,
            event_id,
            server_tick: 900,
            payload: crate::realtime_event::RealtimeEventPayload::WatchDamage(
                crate::realtime_event::RealtimeWatchDamageEvent {
                    victim_player_id: 7,
                    victim_session_id: 55,
                    victim_life_seq: 9,
                    shooter_player_id: 8,
                    shooter_session_id: 56,
                    shooter_life_seq: 10,
                    weapon_data: 2,
                    damage: 100,
                    energy_before: 500,
                    energy_after: 400,
                    projectile_id: 99,
                    damage_seq,
                    lethal: false,
                },
            ),
        }
        .encode()
    }

    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session
        .handle_server_packet(&bootstrap(1).encode())
        .unwrap();
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .unwrap();

    let current = weapon(1, 10, 8_100.25);
    let first = session.handle_server_packet(&current).unwrap();
    assert!(first.events.iter().any(|event| matches!(
        event,
        ClientProtocolEvent::RealtimeWeaponFired {
            server_session_id: 77,
            arena_stream_id: 1,
            event_id: 10,
            server_tick: 10,
            source,
        }
            if source.source_player_id == 7 && source.source_session_id == 55
                && source.source_life_seq == 9 && source.x == 8_100.25
                && source.orientation == 0.75 && source.weapon_data == 0x42
    )));
    assert_eq!(realtime_weapon_event_count(&first), 1);
    assert_eq!(
        realtime_weapon_event_count(&session.handle_server_packet(&current).unwrap()),
        0
    );

    assert_eq!(
        realtime_weapon_event_count(&session.handle_server_packet(&weapon(1, 12, 1.0)).unwrap()),
        1
    );
    assert_eq!(
        realtime_weapon_event_count(&session.handle_server_packet(&weapon(1, 11, 2.0)).unwrap()),
        1
    );
    for id in 13..=267 {
        assert_eq!(
            realtime_weapon_event_count(
                &session
                    .handle_server_packet(&weapon(1, id, id as f32))
                    .unwrap()
            ),
            1
        );
    }
    assert_eq!(
        realtime_weapon_event_count(&session.handle_server_packet(&weapon(1, 10, 3.0)).unwrap()),
        0
    );

    let held_old = weapon(1, 268, 4.0);
    session
        .handle_server_packet(&bootstrap(2).encode())
        .unwrap();
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .unwrap();
    assert_eq!(
        realtime_weapon_event_count(&session.handle_server_packet(&held_old).unwrap()),
        0
    );
    let mut old_entity = full_near_entity(7, 111, 222);
    old_entity.weapon = Some(0x42);
    let held_old_private_snapshot = SnapshotFrame {
        header: SnapshotHeader {
            flags: SNAPSHOT_FLAG_FULL,
            snapshot_id: 1,
            arena_stream_id: 1,
            ..SnapshotHeader::default()
        },
        near_entities: vec![old_entity],
        events: vec![
            SnapshotEvent::RocketState(SnapshotRocketState {
                life_seq: 9,
                observed_input_sequence: Some(8),
                input_sequence_watermark: Some(8),
                activation_sequence: Some(8),
                until_tick: Some(999),
                active: true,
                rockets: 3,
                ..SnapshotRocketState::default()
            }),
            SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                life_seq: 9,
                alive: true,
                respawn_at_tick: None,
                repel_until_tick: Some(900),
                repel_delivery: Some(SnapshotRepelDelivery {
                    active_steps: 2,
                    delivery_generation: 1,
                    source_player_id: 8,
                    source_life_seq: 4,
                    origin_x: 111,
                    origin_y: 222,
                    source_frequency: 42,
                    expected_velocity_x_bits: 1.0_f32.to_bits(),
                    expected_velocity_y_bits: 2.0_f32.to_bits(),
                    weapon_data: 5,
                    source_ship: 1,
                }),
                ..SnapshotLifecycleState::default()
            }),
            SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                life_seq: 9,
                alive: false,
                respawn_at_tick: Some(999),
                ..SnapshotLifecycleState::default()
            }),
            SnapshotEvent::WatchDamage(SnapshotWatchDamage {
                victim_id: 7,
                shooter_id: 8,
                victim_life_seq: Some(9),
                shooter_life_seq: Some(4),
                damage_seq: Some(44),
                lethal: true,
                ..SnapshotWatchDamage::default()
            }),
        ],
        ..SnapshotFrame::default()
    };
    let rejected = session
        .handle_server_packet(&held_old_private_snapshot.encode())
        .unwrap();
    assert!(rejected.events.is_empty());
    assert_eq!(session.latest_received_snapshot_id, None);
    assert_eq!(
        realtime_weapon_event_count(
            &session
                .handle_server_packet(&weapon(2, u32::MAX, 5.0))
                .unwrap()
        ),
        1
    );
    assert_eq!(
        realtime_weapon_event_count(&session.handle_server_packet(&weapon(2, 1, 6.0)).unwrap()),
        1
    );
    let damage = watch_damage(2, 2, 44);
    let first_damage = session.handle_server_packet(&damage).unwrap();
    assert_eq!(
        first_damage
            .events
            .iter()
            .filter(|event| matches!(event, ClientProtocolEvent::DamageWatched(_)))
            .count(),
        1
    );
    assert!(
        session
            .handle_server_packet(&damage)
            .unwrap()
            .events
            .is_empty(),
        "game/event duplicate must be exactly-once"
    );
    assert!(
        session
            .handle_server_packet(&watch_damage(1, 269, 45))
            .unwrap()
            .events
            .is_empty(),
        "held prior-arena damage must fail before publication"
    );

    // A copy may be delayed behind every one of the server's 3,072 retained
    // recipient-local envelopes on the other unordered channel. An unseen
    // older fact inside the shared 4,096-bit window remains admissible;
    // duplicate and arena-reset behavior stays fail closed.
    session
        .handle_server_packet(&bootstrap(3).encode())
        .unwrap();
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .unwrap();
    for event_id in 2..=3_074 {
        assert_eq!(
            realtime_weapon_event_count(
                &session
                    .handle_server_packet(&weapon(3, event_id, event_id as f32))
                    .unwrap()
            ),
            1,
        );
    }
    assert_eq!(
        realtime_weapon_event_count(&session.handle_server_packet(&weapon(3, 1, 1.0)).unwrap()),
        1,
        "an unseen fact overtaken by the maximum queue remains admissible",
    );
    assert_eq!(
        realtime_weapon_event_count(&session.handle_server_packet(&weapon(3, 1, 1.0)).unwrap()),
        0,
        "the late fact remains exactly-once across game/event duplicates",
    );
    session
        .handle_server_packet(&bootstrap(4).encode())
        .unwrap();
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .unwrap();
    for event_id in [u32::MAX - 1, u32::MAX, 1, 2] {
        assert_eq!(
            realtime_weapon_event_count(
                &session
                    .handle_server_packet(&weapon(4, event_id, 7.0))
                    .unwrap()
            ),
            1,
            "recipient-local event ids advance across u32 wrap",
        );
    }
    assert_eq!(
        realtime_weapon_event_count(
            &session
                .handle_server_packet(&weapon(4, u32::MAX, 7.0))
                .unwrap()
        ),
        0,
    );
}

#[test]
fn an_undecodable_realtime_event_is_skipped_and_counted_without_failing_the_poll() {
    use crate::realtime_event::*;
    use crate::transport::PacketTransportLane;

    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session
        .handle_server_packet(
            &ServerClockBootstrap {
                server_session_id: 77,
                arena_stream_id: 1,
                server_tick: 100,
                input_sequence_base: 1,
                position_sequence_base: 1,
                weapon_sequence_base: 1,
                reliable_sequence_base: 0,
                tick_rate_hz: 60,
                snapshot_rate_hz: 30,
            }
            .encode(),
        )
        .unwrap();
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .unwrap();
    let state = |event_id: u32, weapon_type: u16| {
        AuthenticatedRealtimeEvent {
            server_session_id: 77,
            arena_stream_id: 1,
            event_id,
            server_tick: 200,
            payload: RealtimeEventPayload::ProjectileState(RealtimeProjectileStateEvent {
                source_input_sequence: None,
                terminal: false,
                motion_frozen: false,
                source: RealtimeWeaponEvent {
                    source_player_id: 7,
                    source_session_id: 55,
                    source_life_seq: 9,
                    shot_group_id: 10,
                    x: 100.0,
                    y: 100.0,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    orientation: 0.25,
                    frequency: 1,
                    ship: 0,
                    weapon_data: weapon_type,
                },
                projectile_id: 20 + event_id,
                source_server_tick: 190,
                source_projectile_index: 0,
                weapon_data: weapon_type,
                position_x: 110.0,
                position_y: 100.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                expiry_tick: Some(500),
                behavior: PROJECTILE_BEHAVIOR_STANDARD,
                bounces_remaining: 0,
                proximity: Some(RealtimeProjectileProximityState {
                    target_player_id: 8,
                    target_life_seq: 3,
                    sensor_end_tick: 300,
                    highest_axis_offset: 40.0,
                }),
            }),
        }
        .encode()
    };
    let projectile_states = |actions: &ClientProtocolActions| {
        actions
            .events
            .iter()
            .filter(|event| matches!(event, ClientProtocolEvent::RealtimeProjectileState { .. }))
            .count()
    };

    // A bullet never carries a proximity sensor, so this state is invalid.
    let invalid = state(10, 2);
    for lane in [PacketTransportLane::Event, PacketTransportLane::Game] {
        let skipped = session
            .handle_transport_packet_from_lane_at_tick(&invalid, 0, lane)
            .expect("one undecodable event does not fail the poll");
        assert!(skipped.events.is_empty());
    }
    assert_eq!(session.realtime_event_rejects, 2);
    assert_eq!(
        session.last_realtime_event_reject.as_deref(),
        Some("realtime projectile state payload is invalid: proximity")
    );
    assert!(
        session.handle_server_packet(&invalid).is_err(),
        "the reliable path stays fail closed"
    );
    assert_eq!(session.realtime_event_rejects, 2);

    // The next event is processed, and so is a Thor's armed sensor.
    let thor = session
        .handle_transport_packet_from_lane_at_tick(&state(11, 8), 0, PacketTransportLane::Event)
        .unwrap();
    assert_eq!(projectile_states(&thor), 1);
    assert_eq!(session.realtime_event_rejects, 2);
}

#[test]
fn watch_damage_per_carrier_dedupe_covers_full_horizon_wrap_and_lifecycle() {
    fn bootstrap(arena_stream_id: u64) -> ServerClockBootstrap {
        ServerClockBootstrap {
            server_session_id: 77,
            arena_stream_id,
            server_tick: 100,
            input_sequence_base: 1,
            position_sequence_base: 1,
            weapon_sequence_base: 1,
            reliable_sequence_base: 0,
            tick_rate_hz: 60,
            snapshot_rate_hz: 30,
        }
    }
    fn realtime(arena_stream_id: u64, event_id: u32, damage_seq: u32) -> Vec<u8> {
        crate::realtime_event::AuthenticatedRealtimeEvent {
            server_session_id: 77,
            arena_stream_id,
            event_id,
            server_tick: damage_seq,
            payload: crate::realtime_event::RealtimeEventPayload::WatchDamage(
                crate::realtime_event::RealtimeWatchDamageEvent {
                    victim_player_id: 7,
                    victim_session_id: 55,
                    victim_life_seq: 9,
                    shooter_player_id: 8,
                    shooter_session_id: 56,
                    shooter_life_seq: 10,
                    weapon_data: 2,
                    damage: 100,
                    energy_before: 500,
                    energy_after: 400,
                    projectile_id: damage_seq,
                    damage_seq,
                    lethal: false,
                },
            ),
        }
        .encode()
    }
    fn snapshot(arena_stream_id: u64, snapshot_id: u16, damage_seq: u32) -> Vec<u8> {
        SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_FULL,
                snapshot_id,
                base_id: 0,
                tick: damage_seq,
                arena_stream_id,
                near_count: 0,
                mid_count: 0,
                event_count: 0,
            },
            events: vec![SnapshotEvent::WatchDamage(SnapshotWatchDamage {
                victim_id: 7,
                shooter_id: 8,
                weapon_data: 2,
                damage: 100,
                energy_before: Some(500),
                energy_after: 400,
                server_tick: Some(damage_seq),
                damage_seq: Some(damage_seq),
                projectile_id: Some(damage_seq),
                victim_life_seq: Some(9),
                shooter_life_seq: Some(10),
                lethal: false,
            })],
            ..SnapshotFrame::default()
        }
        .encode()
    }
    fn damage_count(actions: &ClientProtocolActions) -> usize {
        actions
            .events
            .iter()
            .filter(|event| matches!(event, ClientProtocolEvent::DamageWatched(_)))
            .count()
    }

    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session
        .handle_server_packet(&bootstrap(11).encode())
        .unwrap();
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .unwrap();

    assert_eq!(
        damage_count(&session.handle_server_packet(&realtime(11, 1, 1)).unwrap()),
        1,
    );
    for damage_seq in 2..=3_073 {
        assert_eq!(
            damage_count(
                &session
                    .handle_server_packet(&realtime(11, damage_seq, damage_seq))
                    .unwrap(),
            ),
            1,
        );
    }
    assert_eq!(session.recent_damage_identities.len(), 3_073);
    assert_eq!(
        damage_count(&session.handle_server_packet(&snapshot(11, 1, 1)).unwrap()),
        1,
        "an independent snapshot copy must reach the application boundary even after realtime receipt",
    );
    assert_eq!(
        damage_count(&session.handle_server_packet(&snapshot(11, 1, 1)).unwrap()),
        0,
        "the same snapshot carrier remains idempotent",
    );

    for damage_seq in 3_074..=DAMAGE_EVENT_IDENTITY_CACHE as u32 {
        assert_eq!(
            damage_count(
                &session
                    .handle_server_packet(&realtime(11, damage_seq, damage_seq))
                    .unwrap(),
            ),
            1,
        );
    }
    assert_eq!(
        session.recent_damage_identities.len(),
        DAMAGE_EVENT_IDENTITY_CACHE,
    );
    assert_eq!(
        damage_count(&session.handle_server_packet(&snapshot(11, 2, 1)).unwrap()),
        0,
        "the exact cache maximum retains cross-carrier idempotence",
    );
    let max_plus_one = DAMAGE_EVENT_IDENTITY_CACHE as u32 + 1;
    assert_eq!(
        damage_count(
            &session
                .handle_server_packet(&realtime(11, max_plus_one, max_plus_one))
                .unwrap(),
        ),
        1,
    );
    assert_eq!(
        session.recent_damage_identities.len(),
        DAMAGE_EVENT_IDENTITY_CACHE,
        "max+1 evicts one exact identity without growing memory",
    );
    assert_eq!(
        session.recent_damage_identity_set.len(),
        DAMAGE_EVENT_IDENTITY_CACHE
    );
    assert_eq!(
        damage_count(&session.handle_server_packet(&snapshot(11, 3, 1)).unwrap()),
        0,
        "the sequence watermark prevents an evicted old identity redisplay",
    );

    session
        .handle_server_packet(&bootstrap(12).encode())
        .unwrap();
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .unwrap();
    assert!(session.recent_damage_identities.is_empty());
    assert!(session.recent_damage_identity_set.is_empty());
    for (event_id, damage_seq) in [(1, u32::MAX - 1), (2, u32::MAX), (3, 1), (4, 2)] {
        assert_eq!(
            damage_count(
                &session
                    .handle_server_packet(&realtime(12, event_id, damage_seq))
                    .unwrap(),
            ),
            1,
            "life-scoped identity history advances across u32 wrap",
        );
    }
    assert_eq!(
        damage_count(
            &session
                .handle_server_packet(&snapshot(12, 1, u32::MAX))
                .unwrap(),
        ),
        1,
        "the snapshot carrier remains independently deliverable across sequence wrap",
    );
}

#[test]
fn watch_damage_snapshot_then_realtime_reaches_application_boundary_from_both_carriers() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let bootstrap = ServerClockBootstrap {
        server_session_id: 77,
        arena_stream_id: 21,
        server_tick: 100,
        input_sequence_base: 1,
        position_sequence_base: 1,
        weapon_sequence_base: 1,
        reliable_sequence_base: 0,
        tick_rate_hz: 60,
        snapshot_rate_hz: 30,
    };
    session.handle_server_packet(&bootstrap.encode()).unwrap();
    session
        .handle_server_packet(&[ServerPacket::JOIN_GAME])
        .unwrap();
    let snapshot = SnapshotFrame {
        header: SnapshotHeader {
            flags: SNAPSHOT_FLAG_FULL,
            snapshot_id: 1,
            arena_stream_id: 21,
            ..SnapshotHeader::default()
        },
        events: vec![SnapshotEvent::WatchDamage(SnapshotWatchDamage {
            victim_id: 7,
            shooter_id: 8,
            victim_life_seq: Some(9),
            shooter_life_seq: Some(10),
            damage_seq: Some(44),
            projectile_id: Some(55),
            damage: 100,
            energy_before: Some(500),
            energy_after: 400,
            ..SnapshotWatchDamage::default()
        })],
        ..SnapshotFrame::default()
    };
    assert_eq!(
        session
            .handle_server_packet(&snapshot.encode())
            .unwrap()
            .events
            .iter()
            .filter(|event| matches!(event, ClientProtocolEvent::DamageWatched(_)))
            .count(),
        1,
    );
    let realtime = crate::realtime_event::AuthenticatedRealtimeEvent {
        server_session_id: 77,
        arena_stream_id: 21,
        event_id: 1,
        server_tick: 100,
        payload: crate::realtime_event::RealtimeEventPayload::WatchDamage(
            crate::realtime_event::RealtimeWatchDamageEvent {
                victim_player_id: 7,
                victim_session_id: 55,
                victim_life_seq: 9,
                shooter_player_id: 8,
                shooter_session_id: 56,
                shooter_life_seq: 10,
                weapon_data: 0,
                damage: 100,
                energy_before: 500,
                energy_after: 400,
                projectile_id: 55,
                damage_seq: 44,
                lethal: false,
            },
        ),
    }
    .encode();
    assert_eq!(
        session
            .handle_server_packet(&realtime)
            .unwrap()
            .events
            .iter()
            .filter(|event| matches!(event, ClientProtocolEvent::DamageWatched(_)))
            .count(),
        1,
        "carrier receipt is not application: Game owns logical exactly-once after authority admission",
    );
}

#[test]
fn session_snapshot_weapon_byte_is_not_carried_forward_to_subsequent_frames() {
    // Regression for the bullet-inflation bug: the synthesized
    // `PlayerPositionUpdate` MUST source its `weapon` byte from the
    // current frame's delta only — never from the carry-forward
    // state. The weapon byte semantically means "this tick I fired
    // X", not a persistent capability; carrying it forward causes
    // `apply_remote_weapon_visual` to spawn a fresh bullet/bomb on
    // every snapshot until the carry is cleared, multiplying
    // rendered projectiles 3-16x against the actual fire count
    // observed by the shooter.
    //
    // Scenario: snapshot 1 carries position + weapon=0x42 for
    // entity 7. Snapshot 2 carries an unchanged position (no fields
    // present at all) plus a different unrelated field for entity 7
    // so the entity is still encoded. The session must emit a
    // PlayerPosition event for both frames, but only the first one
    // should have a non-zero weapon byte — the second has no
    // weapon delta, so the synthesized event must carry weapon=0.
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    // Frame 1: full near entity for player 7 with weapon=0x42.
    let mut frame1_entity = full_near_entity(7, 8200, 8200);
    frame1_entity.weapon = Some(0x42);
    let frame1 = SnapshotFrame {
        header: SnapshotHeader {
            flags: SNAPSHOT_FLAG_FULL,
            snapshot_id: 1,
            base_id: 0,
            tick: 100,
            arena_stream_id: 0,
            near_count: 1,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![frame1_entity],
        mid_entities: vec![],
        events: vec![],
    };
    let actions1 = session
        .handle_server_packet(&frame1.encode())
        .expect("frame 1 parses");
    let pos_events1: Vec<_> = actions1
        .events
        .iter()
        .filter_map(|e| match e {
            ClientProtocolEvent::SnapshotPlayerPosition { update: u, .. } => Some(u),
            _ => None,
        })
        .collect();
    assert_eq!(
        pos_events1.len(),
        1,
        "frame 1 should synthesize one PlayerPosition for entity 7"
    );
    assert_eq!(
        pos_events1[0].weapon, 0x42,
        "frame 1's weapon delta of 0x42 should pass through"
    );

    // Frame 2: same entity but only the ping field is in the delta.
    // Weapon is NOT in the delta (`entity.weapon == None`) — server
    // would omit it once the shooter's accepted_weapon clears or
    // stays unchanged across consecutive snapshots.
    let frame2_entity = SnapshotEntity {
        entity_id: 7,
        field_mask: SnapshotEntity::FIELD_PING,
        ping: Some(42),
        ..Default::default()
    };
    let frame2 = SnapshotFrame {
        header: SnapshotHeader {
            flags: 0,
            snapshot_id: 2,
            base_id: 1,
            tick: 101,
            arena_stream_id: 0,
            near_count: 1,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![frame2_entity],
        mid_entities: vec![],
        events: vec![],
    };
    let actions2 = session
        .handle_server_packet(&frame2.encode())
        .expect("frame 2 parses");
    let pos_events2: Vec<_> = actions2
        .events
        .iter()
        .filter_map(|e| match e {
            ClientProtocolEvent::SnapshotPlayerPosition { update: u, .. } => Some(u),
            _ => None,
        })
        .collect();
    assert_eq!(
        pos_events2.len(),
        1,
        "frame 2 should still synthesize a PlayerPosition for entity 7"
    );
    assert_eq!(
        pos_events2[0].weapon, 0,
        "frame 2 has no weapon delta — synthesized event must carry 0, \
         not the carry-forward value from frame 1"
    );
    // Sanity check: carry-forward of OTHER fields still works
    // (position/ping should persist).
    assert_eq!(pos_events2[0].x, 8200, "x carries forward from frame 1");
    assert_eq!(pos_events2[0].y, 8200, "y carries forward from frame 1");
    assert_eq!(pos_events2[0].ping, 42, "ping comes from frame 2's delta");
}

#[test]
fn session_snapshot_flash_status_is_not_carried_forward_to_subsequent_frames() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    let mut frame1_entity = full_near_entity(7, 8200, 8200);
    frame1_entity.status = Some(STATUS_FLASH | STATUS_SAFETY);
    let frame1 = SnapshotFrame {
        header: SnapshotHeader {
            flags: SNAPSHOT_FLAG_FULL,
            snapshot_id: 1,
            base_id: 0,
            tick: 100,
            arena_stream_id: 0,
            near_count: 1,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![frame1_entity],
        mid_entities: vec![],
        events: vec![],
    };
    let actions1 = session
        .handle_server_packet(&frame1.encode())
        .expect("frame 1 parses");
    let pos_events1: Vec<_> = actions1
        .events
        .iter()
        .filter_map(|e| match e {
            ClientProtocolEvent::SnapshotPlayerPosition { update: u, .. } => Some(u),
            _ => None,
        })
        .collect();
    assert_eq!(pos_events1.len(), 1);
    assert_ne!(pos_events1[0].togglables & STATUS_FLASH, 0);
    assert_ne!(pos_events1[0].togglables & STATUS_SAFETY, 0);

    let frame2_entity = SnapshotEntity {
        entity_id: 7,
        field_mask: SnapshotEntity::FIELD_PING,
        ping: Some(42),
        ..Default::default()
    };
    let frame2 = SnapshotFrame {
        header: SnapshotHeader {
            flags: 0,
            snapshot_id: 2,
            base_id: 1,
            tick: 101,
            arena_stream_id: 0,
            near_count: 1,
            mid_count: 0,
            event_count: 0,
        },
        near_entities: vec![frame2_entity],
        mid_entities: vec![],
        events: vec![],
    };
    let actions2 = session
        .handle_server_packet(&frame2.encode())
        .expect("frame 2 parses");
    let pos_events2: Vec<_> = actions2
        .events
        .iter()
        .filter_map(|e| match e {
            ClientProtocolEvent::SnapshotPlayerPosition { update: u, .. } => Some(u),
            _ => None,
        })
        .collect();
    assert_eq!(pos_events2.len(), 1);
    assert_eq!(pos_events2[0].togglables & STATUS_FLASH, 0);
    assert_ne!(pos_events2[0].togglables & STATUS_SAFETY, 0);
}

#[test]
fn session_resets_ack_counter_after_emit() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);

    // Drive past one interval to emit an ack.
    for i in 1..=(crate::protocol::SNAPSHOT_ACK_INTERVAL as u16) {
        let _ = session.handle_server_packet(&snapshot_frame_bytes(i));
    }
    assert_eq!(session.snapshots_since_last_ack, 0);

    // The next snapshot starts a fresh interval — no ack until N more.
    let actions = session
        .handle_server_packet(&snapshot_frame_bytes(
            crate::protocol::SNAPSHOT_ACK_INTERVAL as u16 + 1,
        ))
        .expect("snapshot should parse");
    assert!(
        actions.outbound_packets.is_empty(),
        "interval-resetting snapshot should not immediately re-emit"
    );
}

#[test]
fn ship_rejection_decodes_once_on_the_reliable_lane_and_rejects_malformed_packets() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut payload = vec![ShipChangeRejected::TYPE, 1, 8];
    payload.extend(7u64.to_le_bytes());
    payload.extend(9u64.to_le_bytes());
    payload.extend(23u16.to_le_bytes());
    payload.extend(41u32.to_le_bytes());
    payload.extend(u16::MAX.to_le_bytes());
    let expected = ShipChangeRejected {
        ship: 8,
        server_session_id: 7,
        arena_stream_id: 9,
        player_id: 23,
        life_seq: 41,
        position_sequence: u16::MAX,
    };
    let packet = ReliableSequencer::encode_reliable_message(0, &payload);
    assert_eq!(
        session.handle_transport_packet(&packet).unwrap().events,
        vec![ClientProtocolEvent::ShipChangeRejected(expected)]
    );
    assert!(
        session
            .handle_transport_packet(&packet)
            .unwrap()
            .events
            .is_empty()
    );
    for length in 0..payload.len() {
        assert!(ShipChangeRejected::parse(&payload[..length]).is_err());
    }
    for (offset, value) in [(1, 2), (2, 9), (3, 0), (11, 0), (21, 0)] {
        let mut bad = payload.clone();
        bad[offset] = value;
        assert!(ShipChangeRejected::parse(&bad).is_err());
    }
    payload.push(0);
    assert!(ShipChangeRejected::parse(&payload).is_err());
}

#[test]
fn client_protocol_session_quality_echo_requires_exact_bootstrap_scope() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let probe = ConnectionQualityProbe {
        server_session_id: 12,
        arena_stream_id: 42,
        sequence: 3,
        nonce: 99,
    };
    assert!(
        session
            .handle_server_packet(&probe.encode())
            .unwrap()
            .outbound_packets
            .is_empty()
    );
    let bootstrap = ServerClockBootstrap {
        server_session_id: 12,
        arena_stream_id: 42,
        server_tick: 9,
        input_sequence_base: 1,
        position_sequence_base: 1,
        weapon_sequence_base: 1,
        reliable_sequence_base: 0,
        tick_rate_hz: 60,
        snapshot_rate_hz: 30,
    };
    session.handle_server_packet(&bootstrap.encode()).unwrap();
    let actions = session.handle_server_packet(&probe.encode()).unwrap();
    assert_eq!(actions.outbound_packets, vec![probe.encode_echo()]);
    assert!(
        actions.events.is_empty(),
        "measurement echo must not mutate gameplay"
    );
    for foreign in [
        ConnectionQualityProbe {
            arena_stream_id: 41,
            ..probe
        },
        ConnectionQualityProbe {
            server_session_id: 11,
            ..probe
        },
    ] {
        assert!(
            session
                .handle_server_packet(&foreign.encode())
                .unwrap()
                .outbound_packets
                .is_empty()
        );
    }
}

#[test]
fn quality_directives_require_reliable_delivery_and_are_pre_gameplay_events() {
    let fact = ConnectionQualityDirective {
        kind: ConnectionQualityDirectiveKind::Force,
        server_session_id: 1,
        arena_stream_id: 2,
        player_id: 3,
        predecessor_life_seq: 4,
        life_seq: 5,
        hold_id: 6,
        remaining_ms: 120_000,
        rejected_request: None,
    };
    for lane in [
        crate::transport::PacketTransportLane::Game,
        crate::transport::PacketTransportLane::Event,
    ] {
        let mut session =
            ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
        assert!(
            session
                .handle_transport_packet_from_lane_at_tick(&fact.encode(), 0, lane)
                .is_err()
        );
        let wrapped = ReliableSequencer::encode_reliable_message(0, &fact.encode());
        assert!(
            session
                .handle_transport_packet_from_lane_at_tick(&wrapped, 0, lane)
                .is_err()
        );
    }
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let actions = session
        .handle_transport_packet_from_lane_at_tick(
            &fact.encode(),
            0,
            crate::transport::PacketTransportLane::Reliable,
        )
        .unwrap();
    assert_eq!(
        actions.events,
        vec![ClientProtocolEvent::ConnectionQualityDirective(fact)]
    );
}

#[test]
fn departure_cutoff_crosses_a_reliable_gap_after_realtime_window_expiry_only_for_remote_scope() {
    use crate::realtime_event::*;
    let cutoff = |id, player, server, arena| {
        AuthenticatedRealtimeEvent {
            server_session_id: server,
            arena_stream_id: arena,
            event_id: id,
            server_tick: 100,
            payload: RealtimeEventPayload::OwnerWeaponsCancelled(RealtimeOwnerWeaponCancellation {
                source_player_id: player,
                source_session_id: 101,
                through_shot_group_id: 10,
                owner_life_seq: 2,
                owner_input_sequence: None,
            }),
        }
        .encode()
    };
    for (player, server, arena, expected) in [
        (1, 77, 88, true),
        (9, 77, 88, false),
        (1, 78, 88, false),
        (1, 77, 89, false),
    ] {
        let mut session = ClientProtocolSession::new(
            true,
            ArenaLoginRequest::new(0, 0, 1024, 768, 0xffff, "alpha", true),
            0,
            0,
        );
        session.self_player_id = Some(9);
        session.latest_server_clock_bootstrap = Some(ServerClockBootstrap {
            server_session_id: 77,
            arena_stream_id: 88,
            ..Default::default()
        });
        let old = cutoff(1, player, server, arena);
        // The gap holds a real ordered wrapper while unrelated realtime
        // traffic advances its independent replay horizon.
        let held = ReliableSequencer::encode_reliable_message(1, &old);
        let first = session.handle_transport_packet(&held).unwrap();
        assert!(first.events.is_empty());
        session
            .handle_transport_packet(&cutoff(6000, 3, 77, 88))
            .unwrap();
        assert!(
            session
                .handle_transport_packet(&old)
                .unwrap()
                .events
                .is_empty(),
            "an unordered old cutoff must not bypass the replay window"
        );
        let released = session
            .handle_transport_packet(&ReliableSequencer::encode_reliable_message(
                0,
                &[0x04, 77, 0],
            ))
            .unwrap();
        let delivered = released
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ClientProtocolEvent::RealtimeOwnerWeaponsCancelled { event_id: 1, .. }
                )
            })
            .count();
        assert_eq!(delivered, usize::from(expected));
        assert_eq!(
            released.events.first(),
            Some(&ClientProtocolEvent::PlayerLeft(77))
        );
        assert!(
            session
                .handle_transport_packet(&held)
                .unwrap()
                .events
                .is_empty(),
            "the ordered wrapper must dispatch exactly once"
        );
    }
}

fn cached_zone_fixture(lvzs: &[(String, Vec<u8>)]) -> ClientProtocolSession {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&[]).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let mut login = vec![0; PasswordResponse::MIN_PACKET_LEN];
    login[0] = PasswordResponse::TYPE;
    login.extend_from_slice(crate::zone_wire::HELD_ZONE_FILES_LOGIN_TRAILER);
    session.handle_server_packet(&login).unwrap();
    let actions = session
        .handle_server_packet(&authenticated_map_info_with_lvzs(
            "cached",
            &[],
            &compressed,
            lvzs,
        ))
        .unwrap();
    // Model the real tracked-send rollback: the first download request has
    // never left the transport, so an available HTTP/cache copy may replace it.
    let id = u32::from_le_bytes(actions.outbound_packets[0][2..6].try_into().unwrap());
    assert!(session.rollback_tracked_reliable(id));
    session.cancel_file_request_retry();
    session
}

#[test]
fn all_cached_zone_files_keep_overlays_and_publication_behind_the_cached_map_boundary() {
    let lvz = b"CONT\0\0\0\0".to_vec();
    let mut session = cached_zone_fixture(&[("art.lvz".into(), lvz.clone())]);
    let mut actions = ClientProtocolActions::default();
    let map = session.pending_file_requests.front().unwrap().clone();
    assert!(
        session
            .accept_cached_zone_file(&map, Vec::new(), &mut actions, 7)
            .unwrap()
    );
    let archive = session.pending_file_requests.front().unwrap().clone();
    assert!(
        session
            .accept_cached_zone_file(&archive, lvz, &mut actions, 7)
            .unwrap()
    );
    assert!(actions.events.is_empty());
    assert_eq!(actions.outbound_packets.len(), 2);
    assert_eq!(actions.outbound_packets[0][6], CachedMapCheck::TYPE);
    let held = crate::zone_wire::HeldZoneFile::parse(&actions.outbound_packets[1][6..]).unwrap();
    assert_eq!(held.index, 1);
    assert_eq!(held.map_sha256, map.sha256.unwrap());
    assert_eq!(held.sha256, archive.sha256.unwrap());
    assert_eq!(session.pending_zone_cache_candidates.len(), 2);
    let published = session
        .handle_server_packet(&[ArenaDirectoryListing::TYPE, b'C', 0, 1, 0])
        .unwrap();
    let files = published
        .events
        .iter()
        .filter_map(|e| {
            if let ClientProtocolEvent::MapDownloaded(f) = e {
                Some(f.filename())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(files, ["tiny.lvl", "art.lvz"]);
    assert_eq!(
        published.events.last(),
        Some(&ClientProtocolEvent::ZoneFilesPublished)
    );
    assert!(session.pending_zone_cache_candidates.is_empty());
}

#[test]
fn corrupt_cached_bytes_do_not_claim_possession_or_consume_the_download_cursor() {
    let bytes = b"CONT\0\0\0\0".to_vec();
    let mut session = cached_zone_fixture(&[("art.lvz".into(), bytes)]);
    let mut actions = ClientProtocolActions::default();
    let map = session.pending_file_requests.front().unwrap().clone();
    assert!(
        !session
            .accept_cached_zone_file(&map, vec![1], &mut actions, 7)
            .unwrap()
    );
    assert_eq!(session.pending_file_requests.front(), Some(&map));
    assert!(actions.outbound_packets.is_empty());
    assert!(
        session
            .accept_cached_zone_file(&map, Vec::new(), &mut actions, 7)
            .unwrap()
    );
    let archive = session.pending_file_requests.front().unwrap().clone();
    let before = actions.outbound_packets.len();
    assert!(
        !session
            .accept_cached_zone_file(&archive, vec![9; 8], &mut actions, 7)
            .unwrap()
    );
    assert_eq!(session.pending_file_requests.front(), Some(&archive));
    assert_eq!(actions.outbound_packets.len(), before);
}

#[test]
fn cached_zone_validation_failure_cannot_publish_at_a_later_spawn_or_directory_boundary() {
    let malformed = b"not-an-lvz".to_vec();
    let mut session = cached_zone_fixture(&[("broken.lvz".into(), malformed.clone())]);
    let mut actions = ClientProtocolActions::default();
    let map = session.pending_file_requests.front().unwrap().clone();
    assert!(
        session
            .accept_cached_zone_file(&map, Vec::new(), &mut actions, 7)
            .unwrap()
    );
    let archive = session.pending_file_requests.front().unwrap().clone();
    assert!(
        session
            .accept_cached_zone_file(&archive, malformed, &mut actions, 7)
            .is_err()
    );
    assert!(!session.pending_zone_publication);
    assert!(session.pending_cached_map_install.is_none());
    assert!(session.pending_zone_cache_candidates.is_empty());
    for packet in [
        vec![SetCoordinates::TYPE, 8, 0, 9, 0],
        vec![ArenaDirectoryListing::TYPE, b'C', 0, 1, 0],
    ] {
        let later = session.handle_server_packet(&packet).unwrap();
        assert!(!later.events.iter().any(|e| matches!(
            e,
            ClientProtocolEvent::MapDownloaded(_) | ClientProtocolEvent::ZoneFilesPublished
        )));
    }
}

#[test]
fn cached_front_does_not_emit_or_mutate_when_the_reliable_table_is_full() {
    let mut session = cached_zone_fixture(&[]);
    for _ in 0..ReliableSequencer::MAX_SENT_RELIABLES {
        session.sequencer.send_reliable(&[0x06]);
    }
    let request = session.pending_file_requests.front().unwrap().clone();
    let mut actions = ClientProtocolActions::default();
    assert!(
        !session
            .accept_cached_zone_file(&request, Vec::new(), &mut actions, 7)
            .unwrap()
    );
    session.advance_zone_file_requests(&mut actions, 7).unwrap();
    assert!(actions.outbound_packets.is_empty());
    assert_eq!(session.pending_file_requests.front(), Some(&request));
    assert!(session.pending_cached_map_install.is_none());
}

#[test]
fn lvz_held_reporting_is_negotiated_without_changing_legacy_password_response_semantics() {
    let mut old = vec![0; PasswordResponse::MIN_PACKET_LEN];
    old[0] = PasswordResponse::TYPE;
    let mut new = old.clone();
    new.extend_from_slice(crate::zone_wire::HELD_ZONE_FILES_LOGIN_TRAILER);
    assert_eq!(
        PasswordResponse::parse_packet(&old).unwrap(),
        PasswordResponse::parse_packet(&new).unwrap()
    );
    let bytes = b"CONT\0\0\0\0".to_vec();
    let mut session = cached_zone_fixture(&[("art.lvz".into(), bytes.clone())]);
    session.handle_server_packet(&old).unwrap();
    assert!(!session.server_held_zone_files);
    let mut actions = ClientProtocolActions::default();
    let map = session.pending_file_requests.front().unwrap().clone();
    assert!(
        session
            .accept_cached_zone_file(&map, Vec::new(), &mut actions, 7)
            .unwrap()
    );
    let archive = session.pending_file_requests.front().unwrap().clone();
    assert!(
        !session
            .accept_cached_zone_file(&archive, bytes, &mut actions, 7)
            .unwrap()
    );
    session.advance_zone_file_requests(&mut actions, 7).unwrap();
    assert_eq!(
        actions.outbound_packets.last().unwrap()[6],
        FileRequestSpec::MAP_REQUEST_TYPE
    );
    session.handle_server_packet(&new).unwrap();
    assert!(session.server_held_zone_files);
}

#[test]
fn cached_file_crc_collision_cannot_substitute_for_the_advertised_sha256() {
    let first = b"\x34\xc4\x99\x54\x07\x0a\x6f\xcb\x46\x32\xb8\x2d\xc5\x17\x05\x1a";
    let second = b"\x28\x00\x19\x15\x4e\x1c\x9f\x1a\xc8\x22\x29\xc0\x2b\x8e\x11\x19";
    let request = FileRequestSpec {
        zone_id: Some("cached".into()),
        filename: "tiny.lvl".into(),
        index: 0,
        size: 16,
        size_is_known: true,
        checksum: continuum_file_crc32(second),
        sha256: Some(crate::zone_wire::sha256_digest(second)),
        decompress: true,
    };
    assert!(request.matches_download_checksum(first));
    assert!(!request.matches_download_digest(first));
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.pending_file_requests.push_back(request.clone());
    let mut actions = ClientProtocolActions::default();
    assert!(
        !session
            .accept_cached_zone_file(&request, first.to_vec(), &mut actions, 7)
            .unwrap()
    );
    assert_eq!(session.pending_file_requests.front(), Some(&request));
    assert!(actions.outbound_packets.is_empty());
}
