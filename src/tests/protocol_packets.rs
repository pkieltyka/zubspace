use super::*;

#[test]
fn cached_map_check_has_exact_zone_and_sha256_wire_shape() {
    let check = CachedMapCheck {
        zone_id: "chaos-zone".to_string(),
        sha256: [0x11; crate::zone_wire::SHA256_BYTES],
    };
    let encoded = check.encode();
    assert_eq!(encoded.len(), CachedMapCheck::PACKET_LEN);
    assert_eq!(encoded[0], CachedMapCheck::TYPE);
    assert_eq!(encoded[1], crate::zone_wire::CACHED_MAP_CHECK_VERSION);
    assert_eq!(&encoded[2..12], b"chaos-zone");
    assert_eq!(&encoded[18..], &[0x11; crate::zone_wire::SHA256_BYTES]);
    assert_eq!(CachedMapCheck::parse(&encoded).unwrap(), check);
    assert!(CachedMapCheck::parse(&encoded[..encoded.len() - 1]).is_err());
    let mut noncanonical_padding = encoded;
    noncanonical_padding[17] = b'x';
    assert!(CachedMapCheck::parse(&noncanonical_padding).is_err());
}

#[test]
fn authenticated_map_information_binds_zone_filenames_sizes_and_sha256() {
    let mut payload =
        vec![0; MapInformation::AUTHENTICATED_HEADER_LEN + MapInformation::AUTHENTICATED_FILE_LEN];
    payload[..4].copy_from_slice(&crate::zone_wire::AUTHENTICATED_MAP_INFO_MAGIC);
    payload[4] = crate::zone_wire::AUTHENTICATED_MAP_INFO_VERSION;
    payload[5..15].copy_from_slice(b"chaos-zone");
    payload[21..30].copy_from_slice(b"arena.lvl");
    put_u32(&mut payload, 37, 0x1122_3344);
    put_u32(&mut payload, 41, 77);
    payload[45..77].fill(0xaa);
    payload[77..79].copy_from_slice(&2_u16.to_le_bytes());
    let lvz = MapInformation::AUTHENTICATED_HEADER_LEN;
    payload[lvz..lvz + 8].copy_from_slice(b"zone.lvz");
    put_u32(&mut payload, lvz + 16, 0x5566_7788);
    put_u32(&mut payload, lvz + 20, 55);
    payload[lvz + 24..lvz + 56].fill(0xbb);

    let info = MapInformation::parse_payload(&payload, true).expect("authenticated map info");
    assert_eq!(info.zone_id.as_deref(), Some("chaos-zone"));
    assert_eq!(info.filename, "arena.lvl");
    assert_eq!(info.sha256, Some([0xaa; crate::zone_wire::SHA256_BYTES]));
    assert_eq!(info.lvz_files[0].zone_id.as_deref(), Some("chaos-zone"));
    assert_eq!(
        info.lvz_files[0].sha256,
        Some([0xbb; crate::zone_wire::SHA256_BYTES])
    );

    let mut hidden_zone_suffix = payload.clone();
    hidden_zone_suffix[20] = b'x';
    assert!(MapInformation::parse_payload(&hidden_zone_suffix, true).is_err());
    let mut hidden_filename_suffix = payload;
    hidden_filename_suffix[36] = b'x';
    assert!(MapInformation::parse_payload(&hidden_filename_suffix, true).is_err());
}

#[test]
fn map_information_rejects_oversized_transfer_advertisements() {
    let mut packet = vec![0; 24];
    packet[..9].copy_from_slice(b"arena.lvl");
    put_u32(
        &mut packet,
        20,
        MapInformation::MAX_COMPRESSED_MAP_BYTES + 1,
    );
    assert!(MapInformation::parse_payload(&packet, true).is_err());
}

#[test]
fn map_information_rejects_traversal_wrong_extensions_and_duplicate_assets() {
    let mut traversal = vec![0; 24];
    traversal[..10].copy_from_slice(b"../bad.lvl");
    assert!(MapInformation::parse_payload(&traversal, true).is_err());

    let mut wrong_extension = vec![0; 24];
    wrong_extension[..9].copy_from_slice(b"arena.lvz");
    assert!(MapInformation::parse_payload(&wrong_extension, true).is_err());

    let mut duplicate = vec![0; 24 + 24 * 2];
    duplicate[..9].copy_from_slice(b"arena.lvl");
    duplicate[24..32].copy_from_slice(b"zone.lvz");
    duplicate[48..56].copy_from_slice(b"ZONE.LVZ");
    assert!(MapInformation::parse_payload(&duplicate, true).is_err());
}

#[test]
fn server_packet_dispatches_known_protocol_packets() {
    let mut password = vec![0; PasswordResponse::MIN_PACKET_LEN];
    password[0] = ServerPacket::PASSWORD_RESPONSE;
    assert_eq!(
        ServerPacket::parse(&password, true).expect("password packet should parse"),
        ServerPacket::PasswordResponse(PasswordResponse {
            code: 0,
            registration_form_requested: false,
        })
    );

    let mut settings = sample_continuum_settings_packet();
    settings[0] = ServerPacket::ARENA_SETTINGS;
    assert!(matches!(
        ServerPacket::parse(&settings, true).expect("settings packet should parse"),
        ServerPacket::ArenaSettings(settings) if settings.arena_type == ServerPacket::ARENA_SETTINGS
    ));

    let mut security = vec![ServerPacket::SECURITY];
    security.extend_from_slice(&1_u32.to_le_bytes());
    security.extend_from_slice(&2_u32.to_le_bytes());
    security.extend_from_slice(&3_u32.to_le_bytes());
    security.extend_from_slice(&4_u32.to_le_bytes());
    assert_eq!(
        ServerPacket::parse(&security, true).expect("security packet should parse"),
        ServerPacket::SecurityChallenge(SecurityChallenge {
            prize_seed: 1,
            door_seed: 2,
            timestamp: 3,
            checksum_key: 4,
        })
    );

    assert_eq!(
        ServerPacket::parse(
            &[ServerPacket::PLAYER_VOICE, 9, 0, b'R', b'I', b'F', b'F'],
            true
        )
        .expect("voice packet should parse"),
        ServerPacket::PlayerVoice(PlayerVoice {
            player_id: 9,
            wav_data: b"RIFF".to_vec(),
        })
    );
    assert!(
        PlayerVoice {
            player_id: 9,
            wav_data: b"RIFF\x24\x00\x00\x00WAVEfmt ".to_vec(),
        }
        .has_wav_header()
    );
    assert!(
        !PlayerVoice {
            player_id: 9,
            wav_data: b"RIFF".to_vec(),
        }
        .has_wav_header()
    );

    let mut transfer = vec![0; ServerFileTransfer::HEADER_LEN];
    transfer[0] = ServerPacket::FILE_TRANSFER;
    transfer[1..9].copy_from_slice(b"news.txt");
    transfer.extend_from_slice(b"hello");
    assert_eq!(
        ServerPacket::parse(&transfer, true).expect("file transfer packet should parse"),
        ServerPacket::FileTransfer(ServerFileTransfer {
            filename: "news.txt".to_string(),
            data: b"hello".to_vec(),
        })
    );
    let mut unnamed_transfer = vec![0; ServerFileTransfer::HEADER_LEN];
    unnamed_transfer[0] = ServerPacket::FILE_TRANSFER;
    assert_eq!(
        ServerPacket::parse(&unnamed_transfer, true).expect("unnamed file should parse"),
        ServerPacket::FileTransfer(ServerFileTransfer {
            filename: "News.txt".to_string(),
            data: Vec::new(),
        })
    );
    for packet_type in [
        ServerPacket::NO_OP_11,
        ServerPacket::NO_OP_17,
        ServerPacket::NO_OP_26,
    ] {
        assert_eq!(
            ServerPacket::parse(&[packet_type], true).expect("no-op packet should parse"),
            ServerPacket::NoOp(packet_type)
        );
    }

    let mut file_request = vec![0; ClientFileRequest::PACKET_LEN];
    file_request[0] = ServerPacket::REQUEST_FILE;
    file_request[1..12].copy_from_slice(b"profile.dat");
    let remote_offset = 1 + ClientFileRequest::LOCAL_FILENAME_LEN;
    file_request[remote_offset..remote_offset + 10].copy_from_slice(b"upload.bin");
    assert_eq!(
        ServerPacket::parse(&file_request, true).expect("client file request should parse"),
        ServerPacket::ClientFileRequest(ClientFileRequest {
            local_filename: "profile.dat".to_string(),
            remote_filename: "upload.bin".to_string(),
        })
    );

    assert_eq!(
        ServerPacket::parse(&[ServerPacket::SHIP_RESET], true).expect("ship reset should parse"),
        ServerPacket::ShipReset(ShipReset)
    );
    assert_eq!(
        ServerPacket::parse(&[ServerPacket::KEEP_ALIVE], true).expect("keep-alive should parse"),
        ServerPacket::KeepAlive(KeepAlive)
    );
    assert_eq!(
        ServerPacket::parse(
            &[
                ServerPacket::HIDDEN_PRIZE_SPAWN,
                0x44,
                0x33,
                0x22,
                0x11,
                0,
                2,
                128,
                2,
                235,
                255,
                210,
                4,
            ],
            true
        )
        .expect("hidden prize spawn should parse"),
        ServerPacket::HiddenPrizeSpawn(HiddenPrizeSpawn {
            collectible_id: 0x1122_3344,
            x: 512,
            y: 640,
            prize_id: -21,
            ttl_ticks: 1234,
        })
    );
    assert_eq!(
        ServerPacket::parse(&[ServerPacket::POST_LOGIN], true).expect("post-login should parse"),
        ServerPacket::PostLogin(PostLogin)
    );

    assert_eq!(
        ServerPacket::parse(&[ServerPacket::SET_COORDINATES, 44, 0, 55, 0], true)
            .expect("set coordinates should parse"),
        ServerPacket::SetCoordinates(SetCoordinates { x: 44, y: 55 })
    );
    assert_eq!(
        ServerPacket::parse(&[ServerPacket::SPECTATE, 1], true)
            .expect("spectate packet should parse"),
        ServerPacket::Spectate(SpectatePacket {
            extra_position_info: true,
        })
    );
    assert_eq!(
        ServerPacket::parse(
            &[ServerPacket::VERSION, 38, 0, 0x44, 0x33, 0x22, 0x11],
            true
        )
        .expect("version packet should parse"),
        ServerPacket::Version(ServerVersionPacket {
            version: 38,
            checksum: 0x1122_3344,
        })
    );
    assert_eq!(
        ServerPacket::parse(&[ServerPacket::LOGIN_FAILURE, b'n', b'o', 0, b'!'], true)
            .expect("login failure packet should parse"),
        ServerPacket::LoginFailure(LoginFailurePacket {
            reason: "no".to_string(),
        })
    );

    let mut personal_banner = vec![0; PersonalBannerChange::PACKET_LEN];
    personal_banner[0] = ServerPacket::PERSONAL_BANNER_CHANGE;
    personal_banner[1] = 8;
    assert!(matches!(
        ServerPacket::parse(&personal_banner, true).expect("personal banner should parse"),
        ServerPacket::PersonalBannerChange(change) if change.banner[0] == 8
    ));

    let mut banner = vec![0; PlayerBannerChange::PACKET_LEN];
    banner[0] = ServerPacket::PLAYER_BANNER_CHANGE;
    put_u16(&mut banner, 1, 9);
    banner[3] = 7;
    assert!(matches!(
        ServerPacket::parse(&banner, true).expect("banner packet should parse"),
        ServerPacket::PlayerBannerChange(change)
            if change.player_id == 9 && change.banner[0] == 7
    ));
    assert_eq!(
        ServerPacket::parse(&[ServerPacket::TOGGLE_SEND_DAMAGE, 1], true)
            .expect("toggle damage packet should parse"),
        ServerPacket::ToggleSendDamage(ToggleSendDamagePacket { enabled: true })
    );
    assert_eq!(
        ServerPacket::parse(&[ServerPacket::TOGGLE_UFO, 1], true)
            .expect("toggle ufo packet should parse"),
        ServerPacket::ToggleUfo(ToggleUfoPacket { available: true })
    );
    let mut personal_koth = vec![0; SetPersonalKothTimer::PACKET_LEN];
    personal_koth[0] = ServerPacket::SET_PERSONAL_KOTH_TIMER;
    put_u32(&mut personal_koth, 1, 12_345);
    assert_eq!(
        ServerPacket::parse(&personal_koth, true).expect("personal koth timer should dispatch"),
        ServerPacket::SetPersonalKothTimer(SetPersonalKothTimer { timer: 12_345 })
    );
    let mut koth_reset = vec![0; KothGameReset::PACKET_LEN];
    koth_reset[0] = ServerPacket::KOTH_GAME_RESET;
    koth_reset[1] = 1;
    put_u32(&mut koth_reset, 2, 22_222);
    put_u16(&mut koth_reset, 6, 9);
    assert_eq!(
        ServerPacket::parse(&koth_reset, true).expect("koth reset should dispatch"),
        ServerPacket::KothGameReset(KothGameReset {
            adding_timer: true,
            timer: 22_222,
            player_id: 9,
        })
    );
    let mut add_koth = vec![0; AddKothTime::PACKET_LEN];
    add_koth[0] = ServerPacket::ADD_KOTH_TIME;
    put_u32(&mut add_koth, 1, 333);
    assert_eq!(
        ServerPacket::parse(&add_koth, true).expect("add koth time should dispatch"),
        ServerPacket::AddKothTime(AddKothTime {
            additional_time: 333,
        })
    );
    let mut arena_directory = vec![ServerPacket::ARENA_DIRECTORY];
    arena_directory.extend_from_slice(b"Public 0\0");
    arena_directory.extend_from_slice(&42_u16.to_le_bytes());
    assert_eq!(
        ServerPacket::parse(&arena_directory, true).expect("arena directory should dispatch"),
        ServerPacket::ArenaDirectory(ArenaDirectoryListing {
            arenas: vec![ArenaDirectoryEntry {
                name: "Public 0".to_string(),
                population: 42,
            }],
        })
    );
    let mut zone_banner = vec![0; ZoneBannerAdvertisement::HEADER_LEN + 4];
    zone_banner[0] = ServerPacket::ZONE_BANNER_ADVERTISEMENT;
    zone_banner[1] = 1;
    put_u16(&mut zone_banner, 2, 2);
    put_u16(&mut zone_banner, 4, 2);
    put_u32(&mut zone_banner, 6, 1_500);
    zone_banner[10..].copy_from_slice(&[1, 2, 3, 4]);
    assert_eq!(
        ServerPacket::parse(&zone_banner, true).expect("zone banner should dispatch"),
        ServerPacket::ZoneBannerAdvertisement(ZoneBannerAdvertisement {
            display_mode: 1,
            width: 2,
            height: 2,
            duration: 1_500,
            pixels: vec![1, 2, 3, 4],
        })
    );

    let mut damage = vec![0; ServerDamageReport::HEADER_LEN + ServerDamageReport::ENTRY_LEN];
    damage[0] = ServerPacket::WATCH_DAMAGE;
    put_u16(&mut damage, 1, 9);
    put_u32(&mut damage, 3, 0x0102_0304);
    put_u16(&mut damage, 7, 10);
    put_u16(&mut damage, 9, 0x1234);
    put_u16(&mut damage, 11, 900);
    put_u16(&mut damage, 13, 45);
    damage[15] = 6;
    assert!(matches!(
        ServerPacket::parse(&damage, true).expect("watch damage packet should parse"),
        ServerPacket::WatchDamage(report)
            if report.player_id == 9 && report.entries[0].damage == 45
    ));

    let bootstrap = ServerClockBootstrap {
        server_session_id: 0x0102_0304_0506_0708,
        arena_stream_id: 0x1112_1314_1516_1718,
        server_tick: 0x1122_3344,
        input_sequence_base: 10,
        position_sequence_base: 11,
        weapon_sequence_base: 12,
        reliable_sequence_base: 13,
        tick_rate_hz: 60,
        snapshot_rate_hz: 30,
    };
    assert_eq!(
        ServerPacket::parse(&bootstrap.encode(), true).expect("clock bootstrap should dispatch"),
        ServerPacket::ServerClockBootstrap(bootstrap)
    );
    let mut unsupported_bootstrap = bootstrap.encode();
    unsupported_bootstrap[1] = ServerClockBootstrap::VERSION + 1;
    assert!(ServerPacket::parse(&unsupported_bootstrap, true).is_err());
}

#[test]
fn server_packet_dispatches_map_and_unknown_packets() {
    let mut map = vec![0; 25];
    map[0] = ServerPacket::MAP_INFORMATION;
    map[1..10].copy_from_slice(b"arena.lvl");
    put_u32(&mut map, 17, 0x1122_3344);
    put_u32(&mut map, 21, 0x0006_7788);
    assert_eq!(
        ServerPacket::parse(&map, true).expect("map info should parse"),
        ServerPacket::MapInformation(MapInformation {
            zone_id: None,
            filename: "arena.lvl".to_string(),
            checksum: 0x1122_3344,
            compressed_size: 0x0006_7788,
            compressed_size_is_known: true,
            sha256: None,
            lvz_files: Vec::new(),
        })
    );

    let mut compressed = vec![ServerPacket::COMPRESSED_MAP; CompressedMapPacket::HEADER_SIZE];
    compressed.extend_from_slice(b"payload");
    assert!(matches!(
        ServerPacket::parse(&compressed, true).expect("compressed map should parse"),
        ServerPacket::CompressedMap(packet) if packet.payload == b"payload"
    ));

    assert_eq!(
        ServerPacket::parse(&[ServerPacket::REDIRECT, 1, 2, 3], true)
            .expect("redirect should parse"),
        ServerPacket::Redirect(ServerRedirectPacket {
            payload: vec![1, 2, 3],
        })
    );
    assert_eq!(
        ServerPacket::parse(&[ServerPacket::SELECT_BOX, 4, 5], true)
            .expect("select box should parse"),
        ServerPacket::SelectBox(SelectBoxPacket {
            payload: vec![4, 5],
        })
    );
    assert_eq!(
        ServerPacket::parse(&[0xee, 1, 2, 3], true).expect("unknown should parse"),
        ServerPacket::Unknown {
            packet_type: 0xee,
            payload: &[1, 2, 3],
        }
    );
    assert!(ServerPacket::parse(&[], true).is_err());
    assert!(ServerPacket::parse(&[ServerPacket::KEEP_ALIVE, 0], true).is_err());
    assert!(ServerPacket::parse(&[ServerPacket::POST_LOGIN, 0], true).is_err());
    assert!(ServerPacket::parse(&[ServerPacket::MAP_INFORMATION; 10], true).is_err());
}

#[test]
fn lvz_toggle_and_modify_packets_parse_reference_game_layouts() {
    let mut toggle = vec![LvzTogglePacket::TYPE];
    toggle.extend_from_slice(&0x1234_u16.to_le_bytes());
    toggle.extend_from_slice(&0x8042_u16.to_le_bytes());

    assert_eq!(
        LvzTogglePacket::parse(&toggle).expect("toggle should parse"),
        LvzTogglePacket {
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
        }
    );
    assert_eq!(
        LvzTogglePacket::parse(&toggle)
            .expect("toggle should parse")
            .encode_payload(),
        toggle
    );
    assert!(LvzTogglePacket::parse(&[LvzTogglePacket::TYPE, 0]).is_err());

    let mut modify = vec![0; LvzModifyPacket::PACKET_SIZE];
    modify[0] = LvzModifyPacket::TYPE;
    modify[1] = 0x1f;
    put_u16(&mut modify, 2, (55 << 1) | 1);
    put_i16(&mut modify, 4, -300);
    put_i16(&mut modify, 6, 444);
    modify[8] = 7;
    modify[9] = 2;
    put_u16(&mut modify, 10, (3 << 12) | 900);

    assert_eq!(
        LvzModifyPacket::parse(&modify).expect("map modify should parse"),
        LvzModifyPacket {
            changed: LvzModificationFlags {
                xy: true,
                image: true,
                layer: true,
                time: true,
                mode: true,
            },
            map_object: true,
            object_id: 55,
            map_x: -300,
            map_y: 444,
            screen_x: -19,
            screen_y: 27,
            x_type: 4,
            y_type: 12,
            image_id: 7,
            layer: 2,
            display_time: 900,
            display_mode: 3,
        }
    );
    assert_eq!(
        LvzModifyPacket::parse(&modify)
            .expect("map modify should parse")
            .encode_payload(),
        modify
    );

    modify[1] = 0x01;
    put_u16(&mut modify, 2, 8 << 1);
    put_u16(&mut modify, 4, (0x0ff9 << 4) | 3);
    put_u16(&mut modify, 6, (25 << 4) | 4);
    put_u16(&mut modify, 10, 0);
    assert_eq!(
        LvzModifyPacket::parse(&modify).expect("screen modify should parse"),
        LvzModifyPacket {
            changed: LvzModificationFlags {
                xy: true,
                image: false,
                layer: false,
                time: false,
                mode: false,
            },
            map_object: false,
            object_id: 8,
            map_x: -109,
            map_y: 404,
            screen_x: -7,
            screen_y: 25,
            x_type: 3,
            y_type: 4,
            image_id: 7,
            layer: 2,
            display_time: 0,
            display_mode: 0,
        }
    );
    assert_eq!(
        LvzModifyPacket::parse(&modify)
            .expect("screen modify should parse")
            .encode_payload(),
        modify
    );
    assert!(LvzModifyPacket::parse(&modify[..LvzModifyPacket::PACKET_SIZE - 1]).is_err());
}

#[test]
fn server_packet_dispatches_lvz_packets() {
    let toggle = [LvzTogglePacket::TYPE, 0x34, 0x12, 0x42, 0x80];
    assert_eq!(
        ServerPacket::parse(&toggle, true).expect("toggle should dispatch"),
        ServerPacket::ToggleLvz(LvzTogglePacket {
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
        })
    );

    let mut modify = vec![0; LvzModifyPacket::PACKET_SIZE];
    modify[0] = LvzModifyPacket::TYPE;
    modify[1] = 0x03;
    put_u16(&mut modify, 2, 14 << 1);
    put_u16(&mut modify, 4, (6 << 4) | 2);
    put_u16(&mut modify, 6, (7 << 4) | 1);
    modify[8] = 9;
    assert!(matches!(
        ServerPacket::parse(&modify, true).expect("modify should dispatch"),
        ServerPacket::ModifyLvz(packet)
            if packet.object_id == 14
                && !packet.map_object
                && packet.changed.xy
                && packet.changed.image
                && packet.screen_x == 6
                && packet.screen_y == 7
                && packet.image_id == 9
    ));
}

#[test]
fn player_position_parses_large_s2c_layout_with_extra_fields() {
    let mut packet = vec![0; 31];
    packet[0] = PlayerPositionUpdate::LARGE_TYPE;
    packet[1] = 13;
    put_u16(&mut packet, 2, 0x3344);
    put_u16(&mut packet, 4, 0x1234);
    put_i16(&mut packet, 6, -321);
    put_u16(&mut packet, 8, 0x4567);
    put_i16(&mut packet, 10, 234);
    packet[12] = 0x5a;
    packet[13] = 0b1010_0001;
    packet[14] = 9;
    put_u16(&mut packet, 15, 0x2345);
    put_u16(&mut packet, 17, 17);
    put_u16(&mut packet, 19, 0x789a);
    put_u16(&mut packet, 21, 900);
    put_u16(&mut packet, 23, 12);
    put_u16(&mut packet, 25, 34);
    put_u32(&mut packet, 27, 0x1122_3344);

    assert_eq!(
        PlayerPositionUpdate::parse_large(&packet).expect("large position should parse"),
        PlayerPositionUpdate {
            player_id: 0x4567,
            direction: 13,
            timestamp: 0x3344,
            x: 0x1234,
            y: 0x2345,
            velocity_x: 234,
            velocity_y: -321,
            togglables: 0b1010_0001,
            ping: 9,
            bounty: 17,
            weapon: 0x789a,
            checksum: Some(0x5a),
            energy: Some(900),
            s2c_latency: Some(12),
            flag_timer: Some(34),
            items: Some(0x1122_3344),
        }
    );
    assert!(PlayerPositionUpdate::parse_large(&packet[..20]).is_err());
    packet[0] = 0xff;
    assert!(PlayerPositionUpdate::parse_large(&packet).is_err());
}

#[test]
fn player_position_parses_small_s2c_layout() {
    let mut packet = vec![0; 26];
    packet[0] = PlayerPositionUpdate::SMALL_TYPE;
    packet[1] = 25;
    put_u16(&mut packet, 2, 0x0102);
    put_u16(&mut packet, 4, 0x0203);
    packet[6] = 7;
    packet[7] = 8;
    packet[8] = 9;
    packet[9] = 0b0001_0010;
    put_i16(&mut packet, 10, -44);
    put_u16(&mut packet, 12, 0x0304);
    put_i16(&mut packet, 14, 55);
    put_u16(&mut packet, 16, 600);
    put_u16(&mut packet, 18, 11);
    put_u16(&mut packet, 20, 22);
    put_u32(&mut packet, 22, 0xaabb_ccdd);

    assert_eq!(
        PlayerPositionUpdate::parse_small(&packet).expect("small position should parse"),
        PlayerPositionUpdate {
            player_id: 9,
            direction: 25,
            timestamp: 0x0102,
            x: 0x0203,
            y: 0x0304,
            velocity_x: 55,
            velocity_y: -44,
            togglables: 0b0001_0010,
            ping: 7,
            bounty: 8,
            weapon: 0,
            checksum: None,
            energy: Some(600),
            s2c_latency: Some(11),
            flag_timer: Some(22),
            items: Some(0xaabb_ccdd),
        }
    );
    assert!(PlayerPositionUpdate::parse_small(&packet[..15]).is_err());
    packet[0] = 0xff;
    assert!(PlayerPositionUpdate::parse_small(&packet).is_err());
}

#[test]
fn player_position_parses_batched_large_s2c_layout() {
    let mut packet = vec![0; 1 + PlayerPositionUpdate::BATCHED_LARGE_ENTRY_LEN];
    packet[0] = PlayerPositionUpdate::BATCHED_LARGE_TYPE;
    put_u16(&mut packet, 1, 0b10_1010 << 10 | 0x0155);
    put_u16(&mut packet, 3, 17 << 10 | 0x002a);
    put_u32(&mut packet, 5, (3 << 28) | (0x0345 << 14) | 0x0123);
    put_u16(&mut packet, 9, (2 << 14) | 0x3ff0);
    packet[11] = 0xff;

    assert_eq!(
        PlayerPositionUpdate::parse_batched_large(&packet).expect("batched large should parse"),
        vec![PlayerPositionUpdate {
            player_id: 0x0155,
            direction: 17,
            timestamp: 0x002a,
            x: 0x0123,
            y: 0x0345,
            velocity_x: -29,
            velocity_y: -16,
            togglables: 0b10_1010,
            ping: 0,
            bounty: 0,
            weapon: 0,
            checksum: None,
            energy: None,
            s2c_latency: None,
            flag_timer: None,
            items: None,
        }]
    );
    assert!(PlayerPositionUpdate::parse_batched_large(&packet[..10]).is_err());
    let mut trailing = packet.clone();
    trailing.push(0);
    assert!(PlayerPositionUpdate::parse_batched_large(&trailing).is_err());
}

#[test]
fn player_position_parses_batched_small_s2c_layout() {
    let mut packet = vec![0; 1 + PlayerPositionUpdate::BATCHED_SMALL_ENTRY_LEN * 2];
    packet[0] = PlayerPositionUpdate::BATCHED_SMALL_TYPE;
    packet[1] = 9;
    put_u16(&mut packet, 2, 12 << 10 | 0x0004);
    put_u32(&mut packet, 4, (0x0022 << 14) | 0x0011);
    put_u16(&mut packet, 8, (1 << 14) | 0x0008);
    packet[10] = 0;
    packet[11] = 10;
    put_u16(&mut packet, 12, 13 << 10 | 0x0005);
    put_u32(&mut packet, 14, (2 << 28) | (0x0044 << 14) | 0x0033);
    put_u16(&mut packet, 18, 0x3fff);
    packet[20] = 1;

    assert_eq!(
        PlayerPositionUpdate::parse_batched_small(&packet).expect("batched small should parse"),
        vec![
            PlayerPositionUpdate {
                player_id: 9,
                direction: 12,
                timestamp: 4,
                x: 0x0011,
                y: 0x0022,
                velocity_x: 16,
                velocity_y: 8,
                togglables: 0,
                ping: 0,
                bounty: 0,
                weapon: 0,
                checksum: None,
                energy: None,
                s2c_latency: None,
                flag_timer: None,
                items: None,
            },
            PlayerPositionUpdate {
                player_id: 10,
                direction: 13,
                timestamp: 5,
                x: 0x0033,
                y: 0x0044,
                velocity_x: 66,
                velocity_y: -1,
                togglables: 0,
                ping: 0,
                bounty: 0,
                weapon: 0,
                checksum: None,
                energy: None,
                s2c_latency: None,
                flag_timer: None,
                items: None,
            },
        ]
    );
    packet[0] = 0xff;
    assert!(PlayerPositionUpdate::parse_batched_small(&packet).is_err());
}

#[test]
fn player_death_parses_reference_game_layout() {
    let mut packet = vec![0; PlayerDeath::PACKET_LEN];
    packet[0] = PlayerDeath::TYPE;
    packet[1] = 0xfe;
    put_u16(&mut packet, 2, 11);
    put_u16(&mut packet, 4, 12);
    put_u16(&mut packet, 6, 44);
    put_u16(&mut packet, 8, 3);
    put_u32(&mut packet, 10, 0x1122_3344);
    put_u32(&mut packet, 14, 0xaabb_ccdd);
    put_u32(&mut packet, 18, 0x0102_0304);

    assert_eq!(
        PlayerDeath::parse(&packet).expect("player death should parse"),
        PlayerDeath {
            green_id: -2,
            green_collectible_id: 0x1122_3344,
            killer_id: 11,
            killed_id: 12,
            bounty: 44,
            flag_transfer: 3,
            life_seq: 0xaabb_ccdd,
            respawn_at_tick: 0x0102_0304,
        }
    );
    assert!(PlayerDeath::parse(&packet[..21]).is_err());
    packet[0] = 0xff;
    assert!(PlayerDeath::parse(&packet).is_err());
}

#[test]
fn prize_packets_parse_reference_game_layouts() {
    let mut player_prize = vec![0; PlayerPrize::PACKET_LEN];
    player_prize[0] = PlayerPrize::TYPE;
    put_u32(&mut player_prize, 1, 0x0102_0304);
    put_u16(&mut player_prize, 5, 44);
    put_u16(&mut player_prize, 7, 55);
    put_i16(&mut player_prize, 9, -21);
    put_u16(&mut player_prize, 11, 9);

    assert_eq!(
        PlayerPrize::parse(&player_prize).expect("player prize should parse"),
        PlayerPrize {
            collectible_id: 0x0102_0304,
            x: 44,
            y: 55,
            prize_id: -21,
            player_id: 9,
        }
    );
    assert!(PlayerPrize::parse(&player_prize[..12]).is_err());
    player_prize[0] = 0xff;
    assert!(PlayerPrize::parse(&player_prize).is_err());

    let mut ack = vec![0; PrizePickupAck::PACKET_LEN];
    ack[0] = PrizePickupAck::TYPE;
    put_u32(&mut ack, 1, 0x1122_3344);
    put_u16(&mut ack, 5, 500);
    put_u16(&mut ack, 7, 640);
    put_i16(&mut ack, 9, 16);
    put_u16(&mut ack, 11, 1234);
    put_u32(&mut ack, 13, 41);
    put_u32(&mut ack, 17, 9);
    ack[26..34].copy_from_slice(&77_u64.to_le_bytes());
    ack[34..42].copy_from_slice(&3_u64.to_le_bytes());
    put_u16(&mut ack, 42, 9);
    assert_eq!(
        PrizePickupAck::parse(&ack).expect("prize pickup ack should parse"),
        PrizePickupAck {
            server_session_id: 77,
            arena_stream_id: 3,
            player_id: 9,
            collectible_id: 0x1122_3344,
            x: 500,
            y: 640,
            prize_id: 16,
            authoritative_energy: 1234,
            life_seq: 41,
            prize_state_revision: 9,
            engine_shutdown_until_tick: None,
            engine_shutdown_application_required: false,
        }
    );
    assert!(PrizePickupAck::parse(&ack[..16]).is_err());
    // Deadline presence is independent of its value: zero is a legitimate
    // wrapped Continuum tick, not an absence sentinel.
    put_i16(&mut ack, 9, 14);
    put_u32(&mut ack, 21, 0);
    ack[25] = 0b11;
    let wrapped_shutdown = PrizePickupAck::parse(&ack).expect("wrapped Shutdown ACK");
    assert_eq!(wrapped_shutdown.engine_shutdown_until_tick, Some(0));
    assert!(wrapped_shutdown.engine_shutdown_application_required);
    let mut invalid_identity = ack.clone();
    invalid_identity[26..34].copy_from_slice(&0_u64.to_le_bytes());
    assert!(PrizePickupAck::parse(&invalid_identity).is_err());
    invalid_identity = ack.clone();
    invalid_identity[34..42].copy_from_slice(&0_u64.to_le_bytes());
    assert!(PrizePickupAck::parse(&invalid_identity).is_err());
    invalid_identity = ack.clone();
    put_u16(&mut invalid_identity, 42, u16::MAX);
    assert!(PrizePickupAck::parse(&invalid_identity).is_err());
    ack[25] = 0b01;
    assert!(PrizePickupAck::parse(&ack).is_err());
    ack[25] = 0b100;
    assert!(PrizePickupAck::parse(&ack).is_err());
    ack[0] = 0xff;
    assert!(PrizePickupAck::parse(&ack).is_err());

    let mut collected = vec![0; CollectedPrize::PACKET_LEN];
    collected[0] = CollectedPrize::TYPE;
    put_u16(&mut collected, 1, 3);
    put_i16(&mut collected, 3, 27);
    assert_eq!(
        CollectedPrize::parse(&collected).expect("collected prize should parse"),
        CollectedPrize {
            count: 3,
            prize_id: 27,
        }
    );
    assert!(CollectedPrize::parse(&collected[..4]).is_err());
    collected[0] = 0xff;
    assert!(CollectedPrize::parse(&collected).is_err());
}

#[test]
fn fixed_collectible_and_extended_death_packets_reject_under_and_trailing_bytes() {
    let mut death = vec![0; PlayerDeath::PACKET_LEN];
    death[0] = PlayerDeath::TYPE;
    let mut player_prize = vec![0; PlayerPrize::PACKET_LEN];
    player_prize[0] = PlayerPrize::TYPE;
    let mut ack = vec![0; PrizePickupAck::PACKET_LEN];
    ack[0] = PrizePickupAck::TYPE;
    let mut spawn = vec![0; HiddenPrizeSpawn::PACKET_LEN];
    spawn[0] = HiddenPrizeSpawn::TYPE;

    for (name, packet, direct_parse) in [(
        "extended player death",
        death,
        PlayerDeath::parse as fn(&[u8]) -> Result<PlayerDeath, String>,
    )] {
        assert!(
            direct_parse(&packet[..packet.len() - 1]).is_err(),
            "{name} under-length"
        );
        let mut trailing = packet.clone();
        trailing.push(0);
        assert!(direct_parse(&trailing).is_err(), "{name} trailing byte");
        assert!(ServerPacket::parse(&packet[..packet.len() - 1], true).is_err());
        assert!(ServerPacket::parse(&trailing, true).is_err());
    }

    assert!(PlayerPrize::parse(&player_prize[..PlayerPrize::PACKET_LEN - 1]).is_err());
    assert!(PrizePickupAck::parse(&ack[..PrizePickupAck::PACKET_LEN - 1]).is_err());
    assert!(HiddenPrizeSpawn::parse(&spawn[..HiddenPrizeSpawn::PACKET_LEN - 1]).is_err());
    player_prize.push(0);
    ack.push(0);
    spawn.push(0);
    assert!(PlayerPrize::parse(&player_prize).is_err());
    assert!(PrizePickupAck::parse(&ack).is_err());
    assert!(HiddenPrizeSpawn::parse(&spawn).is_err());
    for packet in [&player_prize, &ack, &spawn] {
        assert!(ServerPacket::parse(packet, true).is_err());
        assert!(ServerPacket::parse(&packet[..packet.len() - 2], true).is_err());
    }
}

#[test]
fn flag_packets_parse_reference_game_layouts() {
    let mut position = vec![0; FlagPosition::PACKET_LEN];
    position[0] = FlagPosition::TYPE;
    put_u16(&mut position, 1, 3);
    put_u16(&mut position, 3, 44);
    put_u16(&mut position, 5, 55);
    put_u16(&mut position, 7, 9);
    assert_eq!(
        FlagPosition::parse(&position).expect("flag position should parse"),
        FlagPosition {
            flag_id: 3,
            x: 44,
            y: 55,
            owner: 9,
        }
    );
    assert!(FlagPosition::parse(&position[..8]).is_err());
    position[0] = 0xff;
    assert!(FlagPosition::parse(&position).is_err());

    let claim = [FlagClaim::TYPE, 3, 0, 9, 0];
    assert_eq!(
        FlagClaim::parse(&claim).expect("flag claim should parse"),
        FlagClaim {
            flag_id: 3,
            player_id: 9,
        }
    );
    assert!(FlagClaim::parse(&claim[..4]).is_err());

    let mut victory = vec![0; FlagVictory::PACKET_LEN];
    victory[0] = FlagVictory::TYPE;
    put_u16(&mut victory, 1, 7);
    put_u32(&mut victory, 3, 123_456);
    assert_eq!(
        FlagVictory::parse(&victory).expect("flag victory should parse"),
        FlagVictory {
            team: 7,
            jackpot_points: 123_456,
        }
    );
    assert!(FlagVictory::parse(&victory[..6]).is_err());

    let drop = [DropFlag::TYPE, 9, 0];
    assert_eq!(
        DropFlag::parse(&drop).expect("drop flag should parse"),
        DropFlag { player_id: 9 }
    );
    assert!(DropFlag::parse(&drop[..2]).is_err());

    let turf = [TurfFlagUpdate::TYPE, 2, 0, 4, 0];
    assert_eq!(
        TurfFlagUpdate::parse(&turf).expect("turf update should parse"),
        TurfFlagUpdate { owners: vec![2, 4] }
    );
    let trailing = [TurfFlagUpdate::TYPE, 2, 0, 4];
    assert!(TurfFlagUpdate::parse(&trailing).is_err());

    let reward = [FlagReward::TYPE, 7, 0, 44, 0, 8, 0, 55, 0];
    assert_eq!(
        FlagReward::parse(&reward).expect("flag reward should parse"),
        FlagReward {
            grants: vec![
                FlagRewardGrant {
                    team: 7,
                    points: 44,
                },
                FlagRewardGrant {
                    team: 8,
                    points: 55,
                },
            ],
        }
    );
    let trailing = [FlagReward::TYPE, 7, 0, 44];
    assert!(FlagReward::parse(&trailing).is_err());
}

#[test]
fn brick_dropped_parses_reference_game_layout_and_clear_signal() {
    assert_eq!(
        BrickDropped::parse(&[BrickDropped::TYPE]),
        Ok(BrickDropped::Clear)
    );

    let mut packet = vec![0; BrickDropped::PACKET_LEN];
    packet[0] = BrickDropped::TYPE;
    put_u16(&mut packet, 1, 10);
    put_u16(&mut packet, 3, 11);
    put_u16(&mut packet, 5, 14);
    put_u16(&mut packet, 7, 11);
    put_u16(&mut packet, 9, 3);
    put_u16(&mut packet, 11, 99);
    put_u32(&mut packet, 13, 0x0102_0304);

    assert_eq!(
        BrickDropped::parse(&packet).expect("brick drop should parse"),
        BrickDropped::Drop(BrickDrop {
            x1: 10,
            y1: 11,
            x2: 14,
            y2: 11,
            team: 3,
            brick_id: 99,
            timestamp: 0x0102_0304,
        })
    );
    packet[0] = 0xff;
    assert!(BrickDropped::parse(&packet).is_err());
}

#[test]
fn set_coordinates_parses_reference_game_layout() {
    let packet = [SetCoordinates::TYPE, 44, 0, 55, 0];
    let coordinates = SetCoordinates::parse(&packet).expect("coordinates should parse");
    assert_eq!(coordinates, SetCoordinates { x: 44, y: 55 });
    assert_eq!(coordinates.position(), vec2(44.5, 55.5));
    assert!(SetCoordinates::parse(&packet[..4]).is_err());

    let wrong_type = [0xff, 44, 0, 55, 0];
    assert!(SetCoordinates::parse(&wrong_type).is_err());
}

#[test]
fn set_coordinates_snaps_ship_stops_velocity_and_returns_warp_position() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    let mut pos = vec2(10.0, 20.0);
    let mut vel = vec2(30.0, -40.0);
    let warp_pos = apply_set_coordinates_to_ship(
        &map,
        &mut pos,
        &mut vel,
        SetCoordinates { x: 44, y: 55 },
        SHIP_RADIUS,
    );

    assert_eq!(pos, vec2(44.5, 55.5));
    assert_eq!(vel, Vec2::ZERO);
    assert_eq!(warp_pos, pos);
}

#[test]
fn set_coordinates_unstucks_blocked_destination_like_reference_game() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.tiles[10 * map.width + 10] = 1;
    let mut pos = vec2(10.0, 20.0);
    let mut vel = vec2(30.0, -40.0);
    let warp_pos = apply_set_coordinates_to_ship(
        &map,
        &mut pos,
        &mut vel,
        SetCoordinates { x: 160, y: 160 },
        SHIP_RADIUS,
    );

    assert_eq!(pos, vec2(150.0, 150.0));
    assert!(map.can_fit_world(pos, SHIP_RADIUS));
    assert_eq!(vel, Vec2::ZERO);
    assert_eq!(warp_pos, pos);
}

#[test]
fn set_coordinates_unstuck_uses_configured_ship_radius() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.tiles[10 * map.width + 10] = 1;

    let mut small_pos = vec2(10.0, 20.0);
    let mut small_vel = vec2(30.0, -40.0);
    apply_set_coordinates_to_ship(
        &map,
        &mut small_pos,
        &mut small_vel,
        SetCoordinates { x: 160, y: 160 },
        1.0,
    );

    let mut large_pos = vec2(10.0, 20.0);
    let mut large_vel = vec2(30.0, -40.0);
    apply_set_coordinates_to_ship(
        &map,
        &mut large_pos,
        &mut large_vel,
        SetCoordinates { x: 160, y: 160 },
        20.0,
    );

    assert!(map.can_fit_world(small_pos, 1.0));
    assert!(!map.can_fit_world(small_pos, 20.0));
    assert!(map.can_fit_world(large_pos, 20.0));
    assert!(large_pos.x < small_pos.x);
    assert!(large_pos.y < small_pos.y);
}

#[test]
fn arena_directory_listing_parses_reference_game_layout() {
    let mut packet = vec![ArenaDirectoryListing::TYPE];
    packet.extend_from_slice(b"Public 0\0");
    packet.extend_from_slice(&42_u16.to_le_bytes());
    packet.extend_from_slice(b"duel\0");
    packet.extend_from_slice(&3_u16.to_le_bytes());

    assert_eq!(
        ArenaDirectoryListing::parse(&packet).expect("arena directory should parse"),
        ArenaDirectoryListing {
            arenas: vec![
                ArenaDirectoryEntry {
                    name: "Public 0".to_string(),
                    population: 42,
                },
                ArenaDirectoryEntry {
                    name: "duel".to_string(),
                    population: 3,
                },
            ],
        }
    );
    assert_eq!(
        ArenaDirectoryListing::parse(&[ArenaDirectoryListing::TYPE])
            .expect("empty arena directory should parse"),
        ArenaDirectoryListing::default()
    );

    let missing_population = [ArenaDirectoryListing::TYPE, b'a', 0, 1];
    assert!(ArenaDirectoryListing::parse(&missing_population).is_err());
    let missing_null = [ArenaDirectoryListing::TYPE, b'a', b'b'];
    assert!(ArenaDirectoryListing::parse(&missing_null).is_err());
}

#[test]
fn zone_banner_advertisement_parses_reference_game_layout() {
    let mut packet = vec![0; ZoneBannerAdvertisement::HEADER_LEN + 6];
    packet[0] = ZoneBannerAdvertisement::TYPE;
    packet[1] = 2;
    put_u16(&mut packet, 2, 3);
    put_u16(&mut packet, 4, 2);
    put_u32(&mut packet, 6, 1_500);
    packet[10..].copy_from_slice(&[0, 1, 2, 3, 4, 5]);

    assert_eq!(
        ZoneBannerAdvertisement::parse(&packet).expect("zone banner should parse"),
        ZoneBannerAdvertisement {
            display_mode: 2,
            width: 3,
            height: 2,
            duration: 1_500,
            pixels: vec![0, 1, 2, 3, 4, 5],
        }
    );
    assert!(ZoneBannerAdvertisement::parse(&packet[..9]).is_err());
    assert!(ZoneBannerAdvertisement::parse(&packet[..15]).is_err());

    packet[0] = 0xff;
    assert!(ZoneBannerAdvertisement::parse(&packet).is_err());
}

#[test]
fn connection_state_packets_parse_reference_game_layouts() {
    assert_eq!(
        SpectatePacket::parse(&[SpectatePacket::TYPE, 1]).expect("spectate should parse"),
        SpectatePacket {
            extra_position_info: true,
        }
    );
    assert_eq!(
        SpectatePacket::parse(&[SpectatePacket::TYPE, 0]).expect("spectate should parse"),
        SpectatePacket {
            extra_position_info: false,
        }
    );
    assert!(SpectatePacket::parse(&[SpectatePacket::TYPE]).is_err());
    assert!(SpectatePacket::parse(&[SpectatePacket::TYPE, 1, 0]).is_err());

    assert_eq!(
        ServerVersionPacket::parse(&[ServerVersionPacket::TYPE, 38, 0, 4, 3, 2, 1])
            .expect("version should parse"),
        ServerVersionPacket {
            version: 38,
            checksum: 0x0102_0304,
        }
    );
    assert!(ServerVersionPacket::parse(&[ServerVersionPacket::TYPE, 38, 0]).is_err());

    assert_eq!(
        LoginFailurePacket::parse(&[LoginFailurePacket::TYPE, b'b', b'a', b'd', 0, b'!'])
            .expect("failure should parse"),
        LoginFailurePacket {
            reason: "bad".to_string(),
        }
    );
    assert!(LoginFailurePacket::parse(&[LoginFailurePacket::TYPE]).is_err());

    let mut personal_banner_packet = vec![0; PersonalBannerChange::PACKET_LEN];
    personal_banner_packet[0] = PersonalBannerChange::TYPE;
    for (index, byte) in personal_banner_packet[1..].iter_mut().enumerate() {
        *byte = index as u8;
    }
    let personal_banner =
        PersonalBannerChange::parse(&personal_banner_packet).expect("banner should parse");
    assert_eq!(personal_banner.banner[0], 0);
    assert_eq!(personal_banner.banner[95], 95);
    assert!(PersonalBannerChange::parse(&personal_banner_packet[..96]).is_err());

    let mut banner_packet = vec![0; PlayerBannerChange::PACKET_LEN];
    banner_packet[0] = PlayerBannerChange::TYPE;
    put_u16(&mut banner_packet, 1, 9);
    for (index, byte) in banner_packet[3..].iter_mut().enumerate() {
        *byte = index as u8;
    }
    let banner = PlayerBannerChange::parse(&banner_packet).expect("banner should parse");
    assert_eq!(banner.player_id, 9);
    assert_eq!(banner.banner[0], 0);
    assert_eq!(banner.banner[95], 95);
    assert!(PlayerBannerChange::parse(&banner_packet[..98]).is_err());

    assert_eq!(
        ToggleSendDamagePacket::parse(&[ToggleSendDamagePacket::TYPE, 1])
            .expect("toggle damage should parse"),
        ToggleSendDamagePacket { enabled: true }
    );
    assert_eq!(
        ToggleSendDamagePacket::parse(&[ToggleSendDamagePacket::TYPE, 0])
            .expect("toggle damage should parse"),
        ToggleSendDamagePacket { enabled: false }
    );
    assert!(ToggleSendDamagePacket::parse(&[ToggleSendDamagePacket::TYPE]).is_err());
    assert!(ToggleSendDamagePacket::parse(&[ToggleSendDamagePacket::TYPE, 1, 0]).is_err());

    assert_eq!(
        ToggleUfoPacket::parse(&[ToggleUfoPacket::TYPE, 1]).expect("toggle ufo should parse"),
        ToggleUfoPacket { available: true }
    );
    assert_eq!(
        ToggleUfoPacket::parse(&[ToggleUfoPacket::TYPE, 0]).expect("toggle ufo should parse"),
        ToggleUfoPacket { available: false }
    );
    assert!(ToggleUfoPacket::parse(&[ToggleUfoPacket::TYPE]).is_err());
    assert!(ToggleUfoPacket::parse(&[ToggleUfoPacket::TYPE, 1, 0]).is_err());

    let mut personal_koth = vec![0; SetPersonalKothTimer::PACKET_LEN];
    personal_koth[0] = SetPersonalKothTimer::TYPE;
    put_u32(&mut personal_koth, 1, 12_345);
    assert_eq!(
        SetPersonalKothTimer::parse(&personal_koth).expect("personal koth timer should parse"),
        SetPersonalKothTimer { timer: 12_345 }
    );
    assert!(SetPersonalKothTimer::parse(&personal_koth[..4]).is_err());

    let mut koth_reset = vec![0; KothGameReset::PACKET_LEN];
    koth_reset[0] = KothGameReset::TYPE;
    koth_reset[1] = 1;
    put_u32(&mut koth_reset, 2, 22_222);
    put_u16(&mut koth_reset, 6, 9);
    assert_eq!(
        KothGameReset::parse(&koth_reset).expect("koth reset should parse"),
        KothGameReset {
            adding_timer: true,
            timer: 22_222,
            player_id: 9,
        }
    );
    assert!(KothGameReset::parse(&koth_reset[..7]).is_err());

    let mut add_koth = vec![0; AddKothTime::PACKET_LEN];
    add_koth[0] = AddKothTime::TYPE;
    put_u32(&mut add_koth, 1, 333);
    assert_eq!(
        AddKothTime::parse(&add_koth).expect("add koth time should parse"),
        AddKothTime {
            additional_time: 333,
        }
    );
    assert!(AddKothTime::parse(&add_koth[..4]).is_err());

    let mut report = vec![0; ServerDamageReport::HEADER_LEN + ServerDamageReport::ENTRY_LEN];
    report[0] = ServerDamageReport::TYPE;
    put_u16(&mut report, 1, 9);
    put_u32(&mut report, 3, 0x0102_0304);
    put_u16(&mut report, 7, 10);
    put_u16(&mut report, 9, 0x1234);
    put_u16(&mut report, 11, 900);
    put_u16(&mut report, 13, 45);
    report[15] = 6;
    assert_eq!(
        ServerDamageReport::parse(&report).expect("watch damage should parse"),
        ServerDamageReport {
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
        }
    );
    assert!(ServerDamageReport::parse(&report[..6]).is_err());
    assert!(ServerDamageReport::parse(&report[..15]).is_err());
}

#[test]
fn banner_palette_matches_reference_game_default_colors() {
    assert_eq!(banner_palette_color(0), Color::from_rgba(0, 0, 0, 255));
    assert_eq!(banner_palette_color(1), Color::from_rgba(57, 57, 57, 255));
    assert_eq!(
        banner_palette_color(20),
        Color::from_rgba(255, 222, 222, 255)
    );
    assert_eq!(
        banner_palette_color(255),
        Color::from_rgba(255, 255, 255, 255)
    );
}

#[test]
fn client_position_update_encodes_c2s_layout_and_checksum() {
    let update = ClientPositionUpdate {
        direction: 4,
        timestamp: 0x1122_3344,
        x: 0x1234,
        y: 0x5678,
        velocity_x: -120,
        velocity_y: 240,
        togglables: 0b1000_0001,
        bounty: 77,
        energy: 900,
        weapon: 0x0456,
        extra: Some(ClientPositionExtra {
            s2c_latency: 11,
            flag_timer: 22,
            items: 0x89ab_cdef,
        }),
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            4, 0x1234, 0x5678, -120, 240,
        ),
        input_history: None,
        warp_collectible_id: None,
        repel_application: None,
    };

    let packet = update.encode();
    assert_eq!(
        packet.len(),
        crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET
    );
    assert_eq!(packet[0], ClientPositionUpdate::TYPE);
    assert_eq!(packet[1], 4);
    assert_eq!(read_u32(&packet, 2).unwrap(), 0x1122_3344);
    assert_eq!(read_i16(&packet, 6).unwrap(), -120);
    assert_eq!(read_u16(&packet, 8).unwrap(), 0x5678);
    assert_eq!(packet[11], 0b1000_0001);
    assert_eq!(read_u16(&packet, 12).unwrap(), 0x1234);
    assert_eq!(read_i16(&packet, 14).unwrap(), 240);
    assert_eq!(read_u16(&packet, 16).unwrap(), 77);
    assert_eq!(read_u16(&packet, 18).unwrap(), 900);
    assert_eq!(read_u16(&packet, 20).unwrap(), 0x0456);
    assert_eq!(read_u16(&packet, 22).unwrap(), 900);
    assert_eq!(read_u16(&packet, 24).unwrap(), 11);
    assert_eq!(read_u16(&packet, 26).unwrap(), 22);
    assert_eq!(read_u32(&packet, 28).unwrap(), 0x89ab_cdef);
    assert_eq!(
        packet[crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_OFFSET],
        crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_TYPE
    );
    assert_eq!(
        packet[crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_OFFSET + 1],
        crate::continuum_position::CLIENT_POSITION_CANONICAL_STATE_VERSION
    );
    assert_eq!(
        crate::continuum_position::client_position_canonical_state(&packet),
        Some(update.canonical_state)
    );
    assert_eq!(weapon_checksum(&packet), 0);
    assert_eq!(packet[ClientPositionUpdate::CHECKSUM_OFFSET], {
        let mut without_checksum = packet.clone();
        without_checksum[ClientPositionUpdate::CHECKSUM_OFFSET] = 0;
        weapon_checksum(&without_checksum)
    });
}

#[test]
fn client_position_atomically_embeds_exact_rolling_input_history() {
    let history = ClientInputRecordPacket {
        base_seq: u16::MAX,
        inputs: vec![
            PlayerInputRecord {
                bits: 0x11,
                direction: 39,
                door_seed: 0x21,
                combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(
                    u16::MAX,
                ),
            },
            PlayerInputRecord {
                bits: 0x02,
                direction: 0,
                door_seed: 0x22,
                combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(0),
            },
        ],
    };
    let update = ClientPositionUpdate {
        direction: 0,
        timestamp: 0,
        x: 500,
        y: 700,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        bounty: 0,
        energy: 900,
        weapon: 0,
        extra: Some(ClientPositionExtra {
            s2c_latency: u16::MAX - 1,
            flag_timer: crate::continuum_position::CLIENT_POSITION_REPEL_MARKER_MAGIC,
            items: crate::continuum_position::zubspace_position_sequence_items(0),
        }),
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            0, 500, 700, 0, 0,
        ),
        input_history: Some(history.clone()),
        warp_collectible_id: None,
        repel_application: Some(crate::continuum_position::ClientRepelApplicationMarker {
            snapshot_id: u16::MAX - 1,
            application_sequence: 0,
            delivery_generation: 1,
        }),
    };

    let packet = update.encode();
    assert_eq!(
        packet.len(),
        crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET
            + history.encode().len()
            + crate::continuum_position::CLIENT_POSITION_REPEL_APPLICATION_MARKER_LEN
    );
    assert_eq!(weapon_checksum(&packet), 0);
    let embedded = crate::continuum_position::embedded_client_input_record(&packet)
        .expect("embedded input history");
    assert_eq!(ClientInputRecordPacket::parse(embedded).unwrap(), history);
    let position = crate::continuum_position::parse_client_position_snapshot(&packet)
        .expect("position snapshot");
    assert_eq!(position.repel_snapshot_id, Some(u16::MAX - 1));
    assert_eq!(position.repel_application_sequence, Some(0));
    assert_eq!(position.repel_delivery_generation, Some(1));
}

#[test]
fn client_position_warp_marker_is_atomic_and_unambiguous_with_repel() {
    let history = ClientInputRecordPacket {
        base_seq: 7,
        inputs: vec![PlayerInputRecord::default()],
    };
    let encoded_history = history.encode();
    let update = ClientPositionUpdate {
        direction: 0,
        timestamp: 0,
        x: 500,
        y: 700,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        bounty: u16::MAX,
        energy: 900,
        weapon: 0,
        extra: Some(ClientPositionExtra {
            s2c_latency: 0x1234,
            flag_timer: crate::continuum_position::CLIENT_POSITION_REPEL_MARKER_MAGIC,
            items: crate::continuum_position::zubspace_position_sequence_items(7),
        }),
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            0, 500, 700, 0, 0,
        ),
        input_history: Some(history),
        warp_collectible_id: Some(0x5250_1234),
        repel_application: Some(crate::continuum_position::ClientRepelApplicationMarker {
            snapshot_id: 0x1234,
            application_sequence: 7,
            delivery_generation: 1,
        }),
    };
    let packet = update.encode();
    assert_eq!(
        crate::continuum_position::embedded_client_input_record(&packet),
        Some(encoded_history.as_slice())
    );
    let snapshot = crate::continuum_position::parse_client_position_snapshot(&packet)
        .expect("marked position");
    assert_eq!(snapshot.repel_snapshot_id, Some(0x1234));
    assert_eq!(snapshot.repel_application_sequence, Some(7));
    assert_eq!(snapshot.repel_delivery_generation, Some(1));
    assert_eq!(snapshot.warp_collectible_id, Some(0x5250_1234));
}

#[test]
fn gameplay_action_requests_encode_reference_game_payloads() {
    let take_prize = TakePrizeRequest {
        collectible_id: 0x0102_0304,
        x: 44,
        y: 55,
        prize_id: -21,
    }
    .encode_payload();
    assert_eq!(take_prize[0], TakePrizeRequest::TYPE);
    assert_eq!(read_u32(&take_prize, 1).unwrap(), 0x0102_0304);
    assert_eq!(read_u16(&take_prize, 5).unwrap(), 44);
    assert_eq!(read_u16(&take_prize, 7).unwrap(), 55);
    assert_eq!(read_i16(&take_prize, 9).unwrap(), -21);

    let death = DeathRequest {
        killer_id: 9,
        bounty: 77,
    }
    .encode_payload();
    assert_eq!(death, vec![DeathRequest::TYPE, 9, 0, 77, 0]);

    assert_eq!(
        LeaveArenaRequest.encode_payload(),
        vec![LeaveArenaRequest::TYPE]
    );
    assert_eq!(
        CoreDisconnectRequest.encode(),
        vec![
            CoreDisconnectRequest::CORE_BYTE,
            CoreDisconnectRequest::TYPE
        ]
    );
    let voice = VoiceUploadRequest {
        index: 2,
        player_id: 9,
        wav_data: b"RIFF".to_vec(),
    }
    .encode_payload();
    assert_eq!(
        voice,
        vec![VoiceUploadRequest::TYPE, 2, 9, 0, b'R', b'I', b'F', b'F']
    );
    let file_transfer = ClientFileTransferRequest {
        filename: "profile.dat".to_string(),
        data: b"payload".to_vec(),
    }
    .encode_payload();
    assert_eq!(file_transfer[0], ClientFileTransferRequest::TYPE);
    assert_eq!(&file_transfer[1..12], b"profile.dat");
    assert_eq!(&file_transfer[17..], b"payload");
    assert_eq!(
        ClientFileTransferRequest::empty_response_to(&ClientFileRequest {
            local_filename: "profile.dat".to_string(),
            remote_filename: "upload.bin".to_string(),
        }),
        ClientFileTransferRequest {
            filename: "upload.bin".to_string(),
            data: Vec::new(),
        }
    );
    assert_eq!(
        ClientFileTransferRequest::empty_response_to(&ClientFileRequest {
            local_filename: "profile.dat".to_string(),
            remote_filename: String::new(),
        }),
        ClientFileTransferRequest {
            filename: "profile.dat".to_string(),
            data: Vec::new(),
        }
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
        unknown: 0x1112_1314,
        windows_registration_name: "Windows Pilot".to_string(),
        windows_registration_organization: "Subspace".to_string(),
        driver_descriptions: std::array::from_fn(|index| format!("Driver {index}")),
    }
    .encode_payload();
    assert_eq!(registration.len(), 766);
    assert_eq!(registration[0], RegistrationFormResponse::TYPE);
    assert_eq!(&registration[1..6], b"Pilot");
    assert_eq!(&registration[33..50], b"pilot@example.net");
    assert_eq!(&registration[97..103], b"Ottawa");
    assert_eq!(&registration[129..131], b"ON");
    assert_eq!(registration[153], b'M');
    assert_eq!(registration[154], 42);
    assert_eq!(&registration[155..158], &[1, 0, 1]);
    assert_eq!(read_u32(&registration, 158).unwrap(), 0x0102_0304);
    assert_eq!(read_u32(&registration, 162).unwrap(), 0x1112_1314);
    assert_eq!(&registration[166..179], b"Windows Pilot");
    assert_eq!(&registration[206..214], b"Subspace");
    assert_eq!(&registration[246..254], b"Driver 0");

    assert_eq!(ExeRequest.encode_payload(), vec![ExeRequest::TYPE]);
    assert_eq!(NewsRequest.encode_payload(), vec![NewsRequest::TYPE]);
    assert_eq!(
        SecurityViolationReport { violation_id: 0x15 }.encode_payload(),
        vec![SecurityViolationReport::TYPE, 0x15]
    );
    let checksum_violation = SecurityViolationChecksumReport {
        unknown: 0x0102_0304,
        arena_settings_checksum: 0x1112_1314,
        code_checksum_1: 0x2122_2324,
        code_checksum_2: 0x3132_3334,
        violation_id: 0x1b,
    }
    .encode_payload();
    assert_eq!(checksum_violation.len(), 18);
    assert_eq!(checksum_violation[0], SecurityViolationChecksumReport::TYPE);
    assert_eq!(read_u32(&checksum_violation, 1).unwrap(), 0x0102_0304);
    assert_eq!(read_u32(&checksum_violation, 5).unwrap(), 0x1112_1314);
    assert_eq!(read_u32(&checksum_violation, 9).unwrap(), 0x2122_2324);
    assert_eq!(read_u32(&checksum_violation, 13).unwrap(), 0x3132_3334);
    assert_eq!(checksum_violation[17], 0x1b);

    let settings_change = ArenaSettingsChangeRequest::new([
        "Warbird:MaximumEnergy:2800",
        "Weasel:SoccerBallSpeed:9000",
    ])
    .encode_payload();
    assert_eq!(settings_change[0], ArenaSettingsChangeRequest::TYPE);
    assert_eq!(
        &settings_change[1..],
        b"Warbird:MaximumEnergy:2800\0Weasel:SoccerBallSpeed:9000\0\0"
    );
    assert_eq!(
        ArenaSettingsChangeRequest::new(Vec::<String>::new()).encode_payload(),
        vec![ArenaSettingsChangeRequest::TYPE, 0]
    );

    let drop_brick = DropBrickRequest { x: 100, y: 200 }.encode_payload();
    assert_eq!(drop_brick, vec![DropBrickRequest::TYPE, 100, 0, 200, 0]);

    assert_eq!(
        EndKothTimerRequest.encode_payload(),
        vec![EndKothTimerRequest::TYPE]
    );

    let mut banner = [0; PLAYER_BANNER_BYTES];
    banner[0] = 7;
    banner[95] = 9;
    let set_banner = SetBannerRequest { banner }.encode_payload();
    assert_eq!(set_banner.len(), 1 + PLAYER_BANNER_BYTES);
    assert_eq!(set_banner[0], SetBannerRequest::TYPE);
    assert_eq!(set_banner[1], 7);
    assert_eq!(set_banner[96], 9);

    let pickup = BallPickupRequest {
        ball_id: 2,
        timestamp: 0x1122_3344,
    }
    .encode_payload();
    assert_eq!(pickup[0], BallPickupRequest::TYPE);
    assert_eq!(pickup[1], 2);
    assert_eq!(read_u32(&pickup, 2).unwrap(), 0x1122_3344);

    let fire = BallFireRequest {
        ball_id: 3,
        x: 16,
        y: 32,
        velocity_x: -120,
        velocity_y: 240,
        player_id: 9,
        timestamp: 0x0102_0304,
    }
    .encode_payload();
    assert_eq!(fire[0], BallFireRequest::TYPE);
    assert_eq!(fire[1], 3);
    assert_eq!(read_u16(&fire, 2).unwrap(), 16);
    assert_eq!(read_u16(&fire, 4).unwrap(), 32);
    assert_eq!(read_i16(&fire, 6).unwrap(), -120);
    assert_eq!(read_i16(&fire, 8).unwrap(), 240);
    assert_eq!(read_u16(&fire, 10).unwrap(), 9);
    assert_eq!(read_u32(&fire, 12).unwrap(), 0x0102_0304);

    let goal = BallGoalRequest {
        ball_id: 4,
        x: -5,
        y: 6,
    }
    .encode_payload();
    assert_eq!(goal[0], BallGoalRequest::TYPE);
    assert_eq!(goal[1], 4);
    assert_eq!(read_i16(&goal, 2).unwrap(), -5);
    assert_eq!(read_i16(&goal, 4).unwrap(), 6);
}

#[test]
fn late_protocol_ids_match_reference_game_order() {
    assert_eq!(LvzTogglePacket::TYPE, 0x35);
    assert_eq!(LvzModifyPacket::TYPE, 0x36);
    assert_eq!(ToggleSendDamagePacket::TYPE, 0x37);
    assert_eq!(ToggleUfoPacket::TYPE, 0x25);
    assert_eq!(SetPersonalKothTimer::TYPE, 0x2b);
    assert_eq!(KothGameReset::TYPE, 0x2c);
    assert_eq!(AddKothTime::TYPE, 0x2d);
    assert_eq!(ArenaDirectoryListing::TYPE, 0x2f);
    assert_eq!(ZoneBannerAdvertisement::TYPE, 0x30);
    assert_eq!(ServerDamageReport::TYPE, 0x38);
    assert_eq!(ResetScore::TYPE, 0x1a);
    assert_eq!(SoccerGoal::TYPE, 0x0b);
    assert_eq!(PlayerVoice::TYPE, 0x0c);
    assert_eq!(ServerFileTransfer::TYPE, 0x10);
    assert_eq!(FlagVictory::TYPE, 0x14);
    assert_eq!(FlagReward::TYPE, 0x23);
    assert_eq!(SpeedGameOver::TYPE, 0x24);
    assert_eq!(ClientFileRequest::TYPE, 0x19);
    assert_eq!(PersonalBannerChange::TYPE, 0x1e);
    assert_eq!(LeaveArenaRequest::TYPE, 0x02);
    assert_eq!(DeathRequest::TYPE, 0x05);
    assert_eq!(VoiceUploadRequest::TYPE, 0x0e);
    assert_eq!(ClientFileTransferRequest::TYPE, 0x16);
    assert_eq!(RegistrationFormResponse::TYPE, 0x17);
    assert_eq!(ExeRequest::TYPE, 0x0b);
    assert_eq!(NewsRequest::TYPE, 0x0d);
    assert_eq!(SecurityViolationReport::TYPE, 0x1b);
    assert_eq!(SecurityViolationChecksumReport::TYPE, 0x22);
    assert_eq!(ArenaSettingsChangeRequest::TYPE, 0x1d);
    assert_eq!(TakePrizeRequest::TYPE, 0x07);
    assert_eq!(DropBrickRequest::TYPE, 0x1c);
    assert_eq!(EndKothTimerRequest::TYPE, 0x1e);
    assert_eq!(SetBannerRequest::TYPE, 0x19);
    assert_eq!(BallFireRequest::TYPE, 0x1f);
    assert_eq!(BallPickupRequest::TYPE, 0x20);
    assert_eq!(BallGoalRequest::TYPE, 0x21);
    assert_eq!(PlayerPositionUpdate::BATCHED_SMALL_TYPE, 0x39);
    assert_eq!(PlayerPositionUpdate::BATCHED_LARGE_TYPE, 0x3a);
    assert_eq!(ServerRedirectPacket::TYPE, 0x3b);
    assert_eq!(SelectBoxPacket::TYPE, 0x3c);
}

#[test]
fn server_packet_dispatches_position_updates() {
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
    assert!(matches!(
        ServerPacket::parse(&large, true).expect("large position should dispatch"),
        ServerPacket::LargePosition(update)
            if update.player_id == 7 && update.weapon == 3 && update.velocity_y == -20
    ));

    let mut small = vec![0; PlayerPositionUpdate::SMALL_MIN_LEN];
    small[0] = ServerPacket::SMALL_POSITION;
    small[8] = 6;
    put_i16(&mut small, 14, -9);
    assert!(matches!(
        ServerPacket::parse(&small, true).expect("small position should dispatch"),
        ServerPacket::SmallPosition(update) if update.player_id == 6 && update.velocity_x == -9
    ));

    let mut batched = vec![0; 1 + PlayerPositionUpdate::BATCHED_SMALL_ENTRY_LEN];
    batched[0] = ServerPacket::BATCHED_SMALL_POSITION;
    batched[1] = 5;
    assert!(matches!(
        ServerPacket::parse(&batched, true).expect("batched position should dispatch"),
        ServerPacket::BatchedSmallPositions(updates) if updates.len() == 1 && updates[0].player_id == 5
    ));

    let mut death = vec![0; PlayerDeath::PACKET_LEN];
    death[0] = ServerPacket::PLAYER_DEATH;
    put_u16(&mut death, 2, 11);
    put_u16(&mut death, 4, 12);
    put_u16(&mut death, 6, 44);
    assert!(matches!(
        ServerPacket::parse(&death, true).expect("death should dispatch"),
        ServerPacket::PlayerDeath(packet)
            if packet.killer_id == 11 && packet.killed_id == 12 && packet.bounty == 44
    ));

    let mut player_prize = vec![0; PlayerPrize::PACKET_LEN];
    player_prize[0] = ServerPacket::PLAYER_PRIZE;
    put_u16(&mut player_prize, 5, 44);
    put_i16(&mut player_prize, 9, 21);
    assert!(matches!(
        ServerPacket::parse(&player_prize, true).expect("player prize should dispatch"),
        ServerPacket::PlayerPrize(packet) if packet.x == 44 && packet.prize_id == 21
    ));

    let mut ack = vec![0; PrizePickupAck::PACKET_LEN];
    ack[0] = ServerPacket::PRIZE_PICKUP_ACK;
    put_u32(&mut ack, 1, 1);
    put_u16(&mut ack, 5, 44);
    put_i16(&mut ack, 9, 21);
    put_u32(&mut ack, 13, 1);
    put_u32(&mut ack, 17, 1);
    ack[26..34].copy_from_slice(&77_u64.to_le_bytes());
    ack[34..42].copy_from_slice(&3_u64.to_le_bytes());
    put_u16(&mut ack, 42, 9);
    assert!(matches!(
        ServerPacket::parse(&ack, true).expect("prize pickup ack should dispatch"),
        ServerPacket::PrizePickupAck(packet) if packet.x == 44 && packet.prize_id == 21
    ));

    let mut collected = vec![0; CollectedPrize::PACKET_LEN];
    collected[0] = ServerPacket::COLLECTED_PRIZE;
    put_u16(&mut collected, 1, 2);
    put_i16(&mut collected, 3, 27);
    assert!(matches!(
        ServerPacket::parse(&collected, true).expect("collected prize should dispatch"),
        ServerPacket::CollectedPrize(packet) if packet.count == 2 && packet.prize_id == 27
    ));

    let mut flag_position = vec![0; FlagPosition::PACKET_LEN];
    flag_position[0] = ServerPacket::FLAG_POSITION;
    put_u16(&mut flag_position, 1, 3);
    assert!(matches!(
        ServerPacket::parse(&flag_position, true).expect("flag position should dispatch"),
        ServerPacket::FlagPosition(packet) if packet.flag_id == 3
    ));

    let flag_claim = [ServerPacket::FLAG_CLAIM, 3, 0, 9, 0];
    assert!(matches!(
        ServerPacket::parse(&flag_claim, true).expect("flag claim should dispatch"),
        ServerPacket::FlagClaim(packet) if packet.flag_id == 3 && packet.player_id == 9
    ));

    let mut flag_victory = vec![0; FlagVictory::PACKET_LEN];
    flag_victory[0] = ServerPacket::FLAG_VICTORY;
    put_u16(&mut flag_victory, 1, 7);
    put_u32(&mut flag_victory, 3, 123_456);
    assert!(matches!(
        ServerPacket::parse(&flag_victory, true).expect("flag victory should dispatch"),
        ServerPacket::FlagVictory(packet)
            if packet.team == 7 && packet.jackpot_points == 123_456
    ));

    let drop_flag = [ServerPacket::DROP_FLAG, 9, 0];
    assert!(matches!(
        ServerPacket::parse(&drop_flag, true).expect("drop flag should dispatch"),
        ServerPacket::DropFlag(packet) if packet.player_id == 9
    ));

    let turf = [ServerPacket::TURF_FLAG_UPDATE, 2, 0, 4, 0];
    assert!(matches!(
        ServerPacket::parse(&turf, true).expect("turf update should dispatch"),
        ServerPacket::TurfFlagUpdate(packet) if packet.owners == vec![2, 4]
    ));

    let reward = [ServerPacket::FLAG_REWARD, 7, 0, 44, 0, 8, 0, 55, 0];
    assert!(matches!(
        ServerPacket::parse(&reward, true).expect("flag reward should dispatch"),
        ServerPacket::FlagReward(packet)
            if packet.grants.len() == 2 && packet.grants[1].points == 55
    ));

    let mut speed = vec![0; SpeedGameOver::PACKET_LEN];
    speed[0] = ServerPacket::SPEED_GAME_OVER;
    speed[1] = 1;
    put_u16(&mut speed, 2, 3);
    put_u32(&mut speed, 4, 12_345);
    for index in 0..5 {
        put_u32(&mut speed, 8 + index * 4, 1000 + index as u32);
        put_u16(&mut speed, 28 + index * 2, 20 + index as u16);
    }
    assert!(matches!(
        ServerPacket::parse(&speed, true).expect("speed game over should dispatch"),
        ServerPacket::SpeedGameOver(packet)
            if packet.best
                && packet.rank == 3
                && packet.score == 12_345
                && packet.player_scores[4] == 1004
                && packet.player_ids[4] == 24
    ));

    let clear_bricks = [ServerPacket::BRICK_DROPPED];
    assert!(matches!(
        ServerPacket::parse(&clear_bricks, true).expect("brick clear should dispatch"),
        ServerPacket::BrickDropped(BrickDropped::Clear)
    ));

    let mut brick = vec![0; BrickDropped::PACKET_LEN];
    brick[0] = ServerPacket::BRICK_DROPPED;
    put_u16(&mut brick, 1, 10);
    put_u16(&mut brick, 5, 14);
    assert!(matches!(
        ServerPacket::parse(&brick, true).expect("brick drop should dispatch"),
        ServerPacket::BrickDropped(BrickDropped::Drop(drop))
            if drop.x1 == 10 && drop.x2 == 14
    ));
}

#[test]
fn powerball_position_parses_reference_game_layout() {
    let mut packet = vec![0; PowerballPositionUpdate::PACKET_LEN];
    packet[0] = PowerballPositionUpdate::TYPE;
    packet[1] = 3;
    put_u16(&mut packet, 2, 320);
    put_u16(&mut packet, 4, 640);
    put_i16(&mut packet, 6, -120);
    put_i16(&mut packet, 8, 240);
    put_u16(&mut packet, 10, 9);
    put_u32(&mut packet, 12, 0x8000_1234);

    let update = PowerballPositionUpdate::parse(&packet).expect("powerball position should parse");

    assert_eq!(
        update,
        PowerballPositionUpdate {
            ball_id: 3,
            x: 320,
            y: 640,
            velocity_x: -120,
            velocity_y: 240,
            owner_id: 9,
            timestamp: 0x1234,
        }
    );
    assert_eq!(update.owner(), Some(9));
    packet[10] = 0xff;
    packet[11] = 0xff;
    assert_eq!(
        PowerballPositionUpdate::parse(&packet)
            .expect("invalid owner should parse")
            .owner(),
        None
    );
    assert!(PowerballPositionUpdate::parse(&packet[..15]).is_err());
}

#[test]
fn server_packet_dispatches_powerball_position() {
    let mut packet = vec![0; PowerballPositionUpdate::PACKET_LEN];
    packet[0] = ServerPacket::POWERBALL_POSITION;
    packet[1] = 1;
    put_u16(&mut packet, 2, 44);
    put_u16(&mut packet, 4, 55);
    put_u16(&mut packet, 10, PowerballPositionUpdate::INVALID_OWNER);
    put_u32(&mut packet, 12, 22);

    assert!(matches!(
        ServerPacket::parse(&packet, true).expect("powerball should dispatch"),
        ServerPacket::PowerballPosition(update)
            if update.ball_id == 1
                && update.x == 44
                && update.y == 55
                && update.owner().is_none()
                && update.timestamp == 22
    ));
}

#[test]
fn player_roster_packets_parse_reference_game_layouts() {
    let enter = sample_player_enter_packet(0x1234, b"pilot");
    assert_eq!(
        PlayerRosterEntry::parse(&enter).expect("enter packet should parse"),
        PlayerRosterEntry {
            ship: 3,
            audio: 4,
            name: "pilot".to_string(),
            squad: "squad".to_string(),
            kill_points: 1000,
            flag_points: 2000,
            player_id: 0x1234,
            frequency: 7,
            wins: 8,
            losses: 9,
            attach_parent: 0xffff,
            flags: 2,
            koth: 1,
        }
    );
    assert!(PlayerRosterEntry::parse(&enter[..63]).is_err());

    assert_eq!(
        PlayerIdAssignment::parse(&[PlayerIdAssignment::TYPE, 0x34, 0x12])
            .expect("id packet should parse"),
        PlayerIdAssignment { player_id: 0x1234 }
    );
    assert_eq!(
        PlayerLeaving::parse(&[PlayerLeaving::TYPE, 0x34, 0x12])
            .expect("leave packet should parse"),
        PlayerLeaving { player_id: 0x1234 }
    );
    let mut score = vec![0; ScoreUpdate::PACKET_LEN];
    score[0] = ScoreUpdate::TYPE;
    put_u16(&mut score, 1, 0x1234);
    put_u32(&mut score, 3, 111);
    put_u32(&mut score, 7, 222);
    put_u16(&mut score, 11, 8);
    put_u16(&mut score, 13, 9);
    assert_eq!(
        ScoreUpdate::parse(&score).expect("score packet should parse"),
        ScoreUpdate {
            assists: 0,
            player_id: 0x1234,
            kill_points: 111,
            flag_points: 222,
            wins: 8,
            losses: 9,
        }
    );
    assert!(ScoreUpdate::parse(&score[..14]).is_err());
    let mut extended_score = score.clone();
    extended_score.push(1);
    extended_score.extend(17u32.to_le_bytes());
    let extended = ScoreUpdate::parse(&extended_score).unwrap();
    assert_eq!(extended.kill_points, 111);
    assert_eq!(extended.assists, 17);
    for length in 16..20 {
        assert!(ScoreUpdate::parse(&extended_score[..length]).is_err());
    }
    extended_score[15] = 2;
    assert!(ScoreUpdate::parse(&extended_score).is_err());
    assert_eq!(
        ResetScore::parse(&[ResetScore::TYPE, 0x34, 0x12])
            .expect("reset score packet should parse"),
        ResetScore { player_id: 0x1234 }
    );
    assert!(ResetScore::parse(&[ResetScore::TYPE, 0x34]).is_err());

    let mut goal = vec![0; SoccerGoal::PACKET_LEN];
    goal[0] = SoccerGoal::TYPE;
    put_u16(&mut goal, 1, 3);
    put_u32(&mut goal, 3, 12_345);
    assert_eq!(
        SoccerGoal::parse(&goal).expect("soccer goal packet should parse"),
        SoccerGoal {
            frequency: 3,
            team_points: 12_345,
        }
    );
    assert!(SoccerGoal::parse(&goal[..6]).is_err());
}

#[test]
fn roster_change_and_chat_packets_parse_reference_game_layouts() {
    assert_eq!(
        FrequencyChange::parse(&[FrequencyChange::TYPE, 1, 0, 2, 0])
            .expect("freq packet should parse"),
        FrequencyChange {
            player_id: 1,
            frequency: 2,
        }
    );
    assert_eq!(
        ShipFrequencyChange::parse(&[ShipFrequencyChange::TYPE, 7, 1, 0, 2, 0])
            .expect("ship/freq packet should parse"),
        ShipFrequencyChange {
            ship: 7,
            player_id: 1,
            frequency: 2,
            life_seq: None,
        }
    );
    assert_eq!(
        ShipFrequencyChange::parse(&[ShipFrequencyChange::TYPE, 7, 1, 0, 2, 0, 41, 0, 0, 0,])
            .expect("extended ship/freq packet should parse"),
        ShipFrequencyChange {
            ship: 7,
            player_id: 1,
            frequency: 2,
            life_seq: Some(41),
        }
    );
    assert!(
        ShipFrequencyChange::parse(&[ShipFrequencyChange::TYPE, 7, 1, 0, 2, 0, 41]).is_err(),
        "a partial life-sequence extension must fail closed"
    );
    assert_eq!(
        CreateTurretLink::parse(&[CreateTurretLink::TYPE, 1, 0, 2, 0])
            .expect("create turret packet should parse"),
        CreateTurretLink {
            requester_id: 1,
            destination_id: Some(2),
        }
    );
    assert_eq!(
        CreateTurretLink::parse(&[CreateTurretLink::TYPE, 1, 0])
            .expect("short create turret packet should parse"),
        CreateTurretLink {
            requester_id: 1,
            destination_id: None,
        }
    );
    assert_eq!(
        DestroyTurretLink::parse(&[DestroyTurretLink::TYPE, 2, 0])
            .expect("destroy turret packet should parse"),
        DestroyTurretLink { player_id: 2 }
    );
    assert_eq!(
        ChatPacket::parse(&[ChatPacket::TYPE, 2, 3, 1, 0, b'h', b'i', 0, b'!'])
            .expect("chat packet should parse"),
        ChatPacket {
            chat_type: 2,
            sound: 3,
            sender_id: 1,
            message: "hi".to_string(),
        }
    );
}

#[test]
fn server_packet_dispatches_roster_and_chat_packets() {
    let enter = sample_player_enter_packet(9, b"pilot");
    assert!(matches!(
        ServerPacket::parse(&enter, true).expect("enter packet should dispatch"),
        ServerPacket::PlayerEntering(entry) if entry.player_id == 9 && entry.name == "pilot"
    ));
    assert_eq!(
        ServerPacket::parse(&[ServerPacket::JOIN_GAME], true).expect("join packet should dispatch"),
        ServerPacket::JoinGame
    );
    assert!(matches!(
        ServerPacket::parse(&[ServerPacket::CHAT, 2, 0, 9, 0, b'o', b'k'], true)
            .expect("chat packet should dispatch"),
        ServerPacket::Chat(chat) if chat.sender_id == 9 && chat.message == "ok"
    ));
    let mut score = vec![0; ScoreUpdate::PACKET_LEN];
    score[0] = ServerPacket::SCORE_UPDATE;
    put_u16(&mut score, 1, 9);
    put_u32(&mut score, 3, 111);
    put_u32(&mut score, 7, 222);
    put_u16(&mut score, 11, 8);
    put_u16(&mut score, 13, 9);
    assert!(matches!(
        ServerPacket::parse(&score, true).expect("score packet should dispatch"),
        ServerPacket::ScoreUpdate(update)
            if update.player_id == 9 && update.kill_points == 111 && update.losses == 9
    ));
    assert_eq!(
        ServerPacket::parse(&[ServerPacket::RESET_SCORE, 9, 0], true)
            .expect("reset score packet should dispatch"),
        ServerPacket::ResetScore(ResetScore { player_id: 9 })
    );
    let mut goal = vec![0; SoccerGoal::PACKET_LEN];
    goal[0] = ServerPacket::SOCCER_GOAL;
    put_u16(&mut goal, 1, 3);
    put_u32(&mut goal, 3, 12_345);
    assert_eq!(
        ServerPacket::parse(&goal, true).expect("soccer goal packet should dispatch"),
        ServerPacket::SoccerGoal(SoccerGoal {
            frequency: 3,
            team_points: 12_345,
        })
    );
    assert!(matches!(
        ServerPacket::parse(&[ServerPacket::CREATE_TURRET_LINK, 9, 0, 10, 0], true)
            .expect("create turret should dispatch"),
        ServerPacket::CreateTurretLink(link)
            if link.requester_id == 9 && link.destination_id == Some(10)
    ));
    assert!(matches!(
        ServerPacket::parse(&[ServerPacket::DESTROY_TURRET_LINK, 10, 0], true)
            .expect("destroy turret should dispatch"),
        ServerPacket::DestroyTurretLink(link) if link.player_id == 10
    ));
}
