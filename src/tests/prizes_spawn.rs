use super::*;

#[test]
fn protocol_prize_ids_match_reference_game_order() {
    assert_eq!(PrizeKind::Recharge.protocol_id(), 1);
    assert_eq!(PrizeKind::Warp.protocol_id(), 7);
    assert_eq!(PrizeKind::Repel.protocol_id(), 21);
    assert_eq!(PrizeKind::Portal.protocol_id(), 28);
    assert_eq!(
        PrizeKind::from_protocol_id(1),
        Some((PrizeKind::Recharge, false))
    );
    assert_eq!(
        PrizeKind::from_protocol_id(7),
        Some((PrizeKind::Warp, false))
    );
    assert_eq!(
        PrizeKind::from_protocol_id(21),
        Some((PrizeKind::Repel, false))
    );
    assert_eq!(
        PrizeKind::from_protocol_id(28),
        Some((PrizeKind::Portal, false))
    );
    assert_eq!(
        PrizeKind::from_protocol_id(-21),
        Some((PrizeKind::Repel, true))
    );
    assert_eq!(PrizeKind::from_protocol_id(0), None);
    assert_eq!(PrizeKind::from_protocol_id(29), None);
    assert_eq!(PrizeKind::from_protocol_id(i16::MIN), None);
}

#[test]
fn weighted_prize_picker_uses_continuum_prize_order_and_vie_rng() {
    let mut weights = [0; CONTINUUM_PRIZE_WEIGHT_COUNT];
    let mut seed = 1;
    assert_eq!(
        generate_weighted_prize(&weights, 0, 0, false, &mut seed),
        None
    );
    assert_eq!(seed, 1);

    weights[(PrizeKind::Portal.protocol_id() - 1) as usize] = 9;
    assert_eq!(
        generate_weighted_prize(&weights, 9, 0, false, &mut seed),
        Some((PrizeKind::Portal, false))
    );
    assert_eq!(seed, 284_542_633);

    let mut seed = 1;
    assert_eq!(
        generate_weighted_prize(&weights, 9, 1, true, &mut seed),
        Some((PrizeKind::Portal, true))
    );
    assert_eq!(seed, 284_542_633);

    let mut split = [0; CONTINUUM_PRIZE_WEIGHT_COUNT];
    split[(PrizeKind::Recharge.protocol_id() - 1) as usize] = 1;
    split[(PrizeKind::Energy.protocol_id() - 1) as usize] = 1;
    let mut seed = 5;
    assert_eq!(
        generate_weighted_prize(&split, 2, 0, false, &mut seed),
        Some((PrizeKind::Recharge, false))
    );
    assert_eq!(
        generate_weighted_prize(&split, 2, 0, false, &mut seed),
        Some((PrizeKind::Energy, false))
    );
}

#[test]
fn initial_bounty_prizes_use_safe_weighted_prizes_without_mutating_security_seed() {
    let mut weights = [0; CONTINUUM_PRIZE_WEIGHT_COUNT];
    weights[(PrizeKind::Energy.protocol_id() - 1) as usize] = 1;
    assert_eq!(
        initial_bounty_prizes(&weights, 1, 3, 1),
        vec![PrizeKind::Energy, PrizeKind::Energy, PrizeKind::Energy]
    );

    weights = [0; CONTINUUM_PRIZE_WEIGHT_COUNT];
    weights[(PrizeKind::FullCharge.protocol_id() - 1) as usize] = 1;
    assert!(initial_bounty_prizes(&weights, 1, 3, 1).is_empty());

    assert!(!initial_bounty_prize_allowed(PrizeKind::FullCharge));
    assert!(!initial_bounty_prize_allowed(PrizeKind::EngineShutdown));
    assert!(!initial_bounty_prize_allowed(PrizeKind::Shield));
    assert!(!initial_bounty_prize_allowed(PrizeKind::Super));
    assert!(!initial_bounty_prize_allowed(PrizeKind::Warp));
    assert!(!initial_bounty_prize_allowed(PrizeKind::Brick));
    assert!(initial_bounty_prize_allowed(PrizeKind::Energy));
    assert!(initial_bounty_prize_allowed(PrizeKind::Multiprize));
}

#[test]
fn chaos_initial_bounty_replays_thirty_reference_prizes() {
    let tuning = arena_tuning_from_subgame_settings_text(include_str!(
        "../../assets/zones/chaos-zone/arena.conf"
    ))
    .expect("Chaos settings should parse");
    let warbird = &tuning.ships[0];

    assert_eq!(warbird.initial_bounty, 30);
    let prizes = initial_bounty_prizes(
        &tuning.prize_weights,
        tuning.prize_weight_total,
        warbird.initial_bounty,
        1,
    );

    assert_eq!(prizes.len(), warbird.initial_bounty as usize);
    // Lower bounty means fewer rolls, not a free grant of rare upgrades.
    assert!(!prizes.contains(&PrizeKind::Proximity));
    let legacy = initial_bounty_prizes(&tuning.prize_weights, tuning.prize_weight_total, 500, 1);
    assert_eq!(prizes, legacy[..30]);
    assert!(legacy.contains(&PrizeKind::Proximity));
    assert!(legacy.contains(&PrizeKind::Multiprize));
}

#[test]
fn prize_lifetime_uses_continuum_tick_range_and_vie_rng() {
    let mut seed = 1;
    assert_eq!(prize_lifetime_seconds(30.0, 30.0, &mut seed), 30.0);
    assert_eq!(seed, 1);

    assert_eq!(prize_lifetime_seconds(10.0, 30.0, &mut seed), 19.22);
    assert_eq!(seed, 16_930);
    assert_eq!(prize_lifetime_seconds(10.0, 30.0, &mut seed), 14.33);
}

#[test]
fn continuum_prize_spawn_position_uses_centered_virtual_extent() {
    let mut seed = 1;
    let pos = continuum_prize_spawn_position(1024, 1024, 800, 20, 1, &mut seed)
        .expect("spawn position should generate");

    assert_eq!(pos, tile_center(ivec2(673, 618)));
    assert_eq!(seed, 284_542_633);

    let mut seed = 1;
    let pos = continuum_prize_spawn_position(1024, 1024, 1, 0, 1, &mut seed)
        .expect("small virtual extent should clamp");
    assert_eq!(pos, tile_center(ivec2(511, 511)));

    let mut seed = 1;
    let pos = continuum_prize_spawn_position(1024, 1024, 2000, 2000, 100, &mut seed)
        .expect("large virtual extent should clamp");
    let tile = pos / TILE_SIZE;
    assert!(tile.x >= 1.0 && tile.x < 1023.0);
    assert!(tile.y >= 1.0 && tile.y < 1023.0);
}

#[test]
fn continuum_spawn_region_decodes_signed_bitfields() {
    let region = ContinuumSpawnRegion::parse(pack_continuum_spawn_region(-12, 256, 21));

    assert_eq!(
        region,
        ContinuumSpawnRegion {
            x: -12,
            y: 256,
            radius: 21
        }
    );
}

#[test]
fn continuum_player_spawn_uses_configured_regions_by_frequency() {
    let map = empty_test_map();
    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.spawn_regions[0] = ContinuumSpawnRegion {
        x: 100,
        y: -20,
        radius: 0,
    };
    tuning.spawn_regions[1] = ContinuumSpawnRegion {
        x: 200,
        y: 300,
        radius: 0,
    };

    let mut seed = 1;
    let first = continuum_player_spawn_position(&map, &tuning, 0, 1, SHIP_RADIUS, &mut seed);
    let mut seed = 1;
    let second = continuum_player_spawn_position(&map, &tuning, 1, 1, SHIP_RADIUS, &mut seed);

    assert_eq!(first, tile_center(ivec2(100, 1004)));
    assert_eq!(second, tile_center(ivec2(200, 300)));
}

#[test]
fn continuum_player_spawn_skips_unconfigured_region_slots() {
    let map = empty_test_map();
    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.spawn_regions[1] = ContinuumSpawnRegion {
        x: 200,
        y: 300,
        radius: 0,
    };
    tuning.spawn_regions[3] = ContinuumSpawnRegion {
        x: 400,
        y: 500,
        radius: 0,
    };

    let mut seed = 1;
    let first = continuum_player_spawn_position(&map, &tuning, 0, 1, SHIP_RADIUS, &mut seed);
    let mut seed = 1;
    let second = continuum_player_spawn_position(&map, &tuning, 1, 1, SHIP_RADIUS, &mut seed);

    assert_eq!(first, tile_center(ivec2(200, 300)));
    assert_eq!(second, tile_center(ivec2(400, 500)));
}

#[test]
fn continuum_region_spawn_fit_uses_configured_ship_radius() {
    let mut map = empty_test_map();
    map.set_tile(101, 100, 1);
    let region = ContinuumSpawnRegion {
        x: 100,
        y: 100,
        radius: 0,
    };

    let mut seed = 1;
    let small = continuum_region_spawn_position(&map, region, 1.0, &mut seed)
        .expect("region center should be available for small ships");
    let mut seed = 1;
    let large = continuum_region_spawn_position(&map, region, 20.0, &mut seed)
        .expect("blocked region center should fall back");

    assert_eq!(small, tile_center(ivec2(100, 100)));
    assert_ne!(large, small);
    assert!(map.can_fit_world(large, 20.0));
}

#[test]
fn continuum_player_spawn_uses_radar_quadrants_without_regions() {
    let map = empty_test_map();
    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.radar_mode = 2;

    let mut seed = 1;
    let pos = continuum_player_spawn_position(&map, &tuning, 3, 1, SHIP_RADIUS, &mut seed);
    let tile = (pos / TILE_SIZE).floor();

    assert!(tile.x >= 768.0 && tile.x < 1024.0);
    assert!(tile.y >= 768.0 && tile.y < 1024.0);
}

#[test]
fn continuum_default_spawn_radius_honors_warp_radius_limit() {
    assert_eq!(continuum_default_spawn_radius(1, 1024), 266);
    assert_eq!(continuum_default_spawn_radius(100, 768), 768);
    assert_eq!(continuum_default_spawn_radius(1, 0), 3);
}

#[test]
fn multiprize_uses_reference_safe_green_subset() {
    for excluded in [
        PrizeKind::EngineShutdown,
        PrizeKind::Shield,
        PrizeKind::Super,
        PrizeKind::Multiprize,
        PrizeKind::Warp,
        PrizeKind::Brick,
    ] {
        assert!(!multiprize_prize_allowed(excluded));
    }

    for expected in [
        PrizeKind::FullCharge,
        PrizeKind::Stealth,
        PrizeKind::Cloak,
        PrizeKind::XRadar,
        PrizeKind::Multifire,
        PrizeKind::Proximity,
        PrizeKind::Antiwarp,
    ] {
        assert!(multiprize_prize_allowed(expected));
    }

    let mut weights = [0; CONTINUUM_PRIZE_WEIGHT_COUNT];
    weights[(PrizeKind::Energy.protocol_id() - 1) as usize] = 1;
    assert_eq!(
        multiprize_prizes(&weights, 1, 3, 1),
        vec![PrizeKind::Energy, PrizeKind::Energy, PrizeKind::Energy]
    );
    assert_eq!(
        authoritative_multiprize_prizes(&weights, 1, 3),
        multiprize_prizes(&weights, 1, 3, crate::authority_prizes::SECURITY_PRIZE_SEED)
    );

    weights = [0; CONTINUUM_PRIZE_WEIGHT_COUNT];
    weights[(PrizeKind::Multiprize.protocol_id() - 1) as usize] = 1;
    assert!(multiprize_prizes(&weights, 1, 3, 1).is_empty());
}

#[test]
fn authoritative_multiprize_does_not_use_hidden_prize_rng_progress() {
    let mut weights = [0; CONTINUUM_PRIZE_WEIGHT_COUNT];
    weights[(PrizeKind::Energy.protocol_id() - 1) as usize] = 1;
    weights[(PrizeKind::Recharge.protocol_id() - 1) as usize] = 1;

    let mut hidden_prize_seed = crate::authority_prizes::SECURITY_PRIZE_SEED;
    let _ = generate_weighted_prize(&weights, 2, 0, false, &mut hidden_prize_seed);
    assert_ne!(
        hidden_prize_seed,
        crate::authority_prizes::SECURITY_PRIZE_SEED
    );

    assert_eq!(
        authoritative_multiprize_prizes(&weights, 2, 5),
        multiprize_prizes(&weights, 2, 5, crate::authority_prizes::SECURITY_PRIZE_SEED)
    );
}

#[test]
fn none_prize_uses_reference_safe_random_replacement_subset() {
    for excluded in [
        PrizeKind::EngineShutdown,
        PrizeKind::Shield,
        PrizeKind::Super,
        PrizeKind::Multiprize,
        PrizeKind::Warp,
    ] {
        assert!(!none_prize_replacement_prize_allowed(excluded));
    }
    assert!(none_prize_replacement_prize_allowed(PrizeKind::Brick));

    let mut weights = [0; CONTINUUM_PRIZE_WEIGHT_COUNT];
    weights[(PrizeKind::Energy.protocol_id() - 1) as usize] = 1;
    assert_eq!(
        none_prize_replacement_prize(&weights, 1, 1),
        Some(PrizeKind::Energy)
    );

    weights = [0; CONTINUUM_PRIZE_WEIGHT_COUNT];
    weights[(PrizeKind::Warp.protocol_id() - 1) as usize] = 1;
    assert_eq!(none_prize_replacement_prize(&weights, 1, 1), None);
}

#[test]
fn bounty_notifications_match_reference_kill_shape() {
    assert_eq!(
        player_destroyed_notification("Stardust", 4, Some("Bot 2")),
        "Stardust(4) killed by: Bot 2"
    );
    assert_eq!(
        player_destroyed_notification("Stardust", 120, Some("claudepilot")),
        "Stardust(120) killed by: claudepilot"
    );
    assert_eq!(
        player_destroyed_notification("Stardust", 0, None),
        "Stardust(0) destroyed."
    );
    assert_eq!(
        bot_destroyed_notification(2, 5, "Stardust"),
        "Bot 3(5) killed by: Stardust"
    );
    assert_eq!(
        player_killed_by_notification("Bot 1", 50, "Stardust"),
        "Bot 1(50) killed by: Stardust"
    );
}

#[test]
fn server_status_notifications_are_concise() {
    assert_eq!(
        server_version_notification(&ServerVersionPacket {
            version: 38,
            checksum: 0x0102_0304,
        }),
        "Server version 38 checksum 0x01020304."
    );
    assert_eq!(
        arena_directory_notification(&ArenaDirectoryListing {
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
        }),
        "Arena directory: 2 arenas, 45 players."
    );
    assert_eq!(
        arena_directory_notification(&ArenaDirectoryListing {
            arenas: vec![ArenaDirectoryEntry {
                name: "Public 0".to_string(),
                population: 42,
            }],
        }),
        "Arena directory: Public 0 (42 players)."
    );
    assert_eq!(
        arena_directory_rows(
            &[
                ArenaDirectoryEntry {
                    name: "Public 0".to_string(),
                    population: 42,
                },
                ArenaDirectoryEntry {
                    name: "duel".to_string(),
                    population: 3,
                },
                ArenaDirectoryEntry {
                    name: "race".to_string(),
                    population: 1,
                },
                ArenaDirectoryEntry {
                    name: "Very Long Public Arena Name".to_string(),
                    population: 99,
                },
            ],
            4,
        ),
        vec![
            "Public 0               42".to_string(),
            "duel                    3".to_string(),
            "race                    1".to_string(),
            "Very Long Public ...   99".to_string(),
        ]
    );
    assert_eq!(
        arena_directory_panel_position(1280.0, 720.0, 320.0, 180.0, 920.0, 280.0),
        (936.0, 110.0)
    );
    assert_eq!(
        arena_directory_panel_position(720.0, 720.0, 260.0, 180.0, 540.0, 280.0),
        (24.0, 296.0)
    );
    assert_eq!(
        server_file_transfer_notification(&ServerFileTransfer {
            filename: "news.txt".to_string(),
            data: b"hello".to_vec(),
        }),
        "Received file: news.txt."
    );
    assert_eq!(
        client_file_request_notification(&ClientFileRequest {
            local_filename: "profile.dat".to_string(),
            remote_filename: "upload.bin".to_string(),
        }),
        "Server requested file: profile.dat."
    );
    assert_eq!(
        player_voice_notification(&PlayerVoice {
            player_id: 9,
            wav_data: b"RIFF".to_vec(),
        }),
        "Voice message from #9."
    );
}

#[test]
fn network_game_event_notifications_are_concise() {
    let roster = PlayerRosterEntry {
        ship: 2,
        audio: 0,
        name: "remote".to_string(),
        squad: "squad".to_string(),
        kill_points: 0,
        flag_points: 0,
        player_id: 9,
        frequency: 3,
        wins: 0,
        losses: 0,
        attach_parent: INVALID_PLAYER_ID,
        flags: 0,
        koth: 0,
    };
    assert_eq!(
        player_entered_notification(&roster),
        "Player remote joined."
    );
    assert_eq!(
        player_left_notification(Some("remote")),
        "Player remote left."
    );
    assert_eq!(player_left_notification(None), "A player left.");
    assert_eq!(player_left_notification(Some("  ")), "A player left.");
    assert_eq!(
        frequency_change_notification(
            &FrequencyChange {
                player_id: 9,
                frequency: 4,
            },
            Some("remote"),
        ),
        "remote changed to freq 4."
    );
    assert_eq!(
        ship_frequency_change_notification(
            &ShipFrequencyChange {
                ship: 7,
                player_id: 9,
                frequency: 4,
                life_seq: None,
            },
            None,
        ),
        "Player #9 changed to ship 8, freq 4."
    );
    assert_eq!(
        soccer_goal_notification(&SoccerGoal {
            frequency: 3,
            team_points: 12_345,
        }),
        "Soccer goal: freq 3 now has 12345 points."
    );
    assert_eq!(
        flag_victory_notification(&FlagVictory {
            team: 7,
            jackpot_points: 44_000,
        }),
        "Flag victory: freq 7 wins 44000 jackpot points."
    );
    assert_eq!(
        flag_reward_notification(&FlagReward {
            grants: vec![FlagRewardGrant {
                team: 7,
                points: 44,
            }],
        }),
        "Flag reward: freq 7 +44."
    );
    assert_eq!(
        flag_reward_notification(&FlagReward {
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
        }),
        "Flag rewards received for 2 teams."
    );
    assert_eq!(
        speed_game_over_notification(&SpeedGameOver {
            best: true,
            rank: 2,
            score: 3_333,
            player_scores: [10, 9, 8, 7, 6],
            player_ids: [1, 2, 3, 4, 5],
        }),
        "Speed game over: rank 2, score 3333 best."
    );
    assert_eq!(
        ufo_toggle_notification(&ToggleUfoPacket { available: true }),
        "UFO mode available."
    );
    assert_eq!(
        personal_koth_timer_notification(&SetPersonalKothTimer { timer: 12_345 }),
        "Personal KOTH timer set to 2:03.4."
    );
    assert_eq!(
        koth_reset_notification(&KothGameReset {
            adding_timer: true,
            timer: 22_222,
            player_id: 9,
        }),
        "KOTH reset: player #9 timer 3:42.2 (adding)."
    );
    assert_eq!(
        koth_time_added_notification(&AddKothTime {
            additional_time: 333,
        }),
        "KOTH time added: 3.3."
    );
    assert_eq!(
        watched_damage_notification(&ServerDamageReport {
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
        }),
        "Watched damage for #9: 45 from #10."
    );
    assert_eq!(
        watched_damage_notification(&ServerDamageReport {
            player_id: 9,
            timestamp: 0x0102_0304,
            entries: vec![
                ServerDamageReportEntry {
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
                },
                ServerDamageReportEntry {
                    attacker_id: 11,
                    weapon_data: 0x5678,
                    energy: 850,
                    damage: 30,
                    timestamp_offset: 7,
                    energy_before: None,
                    damage_seq: None,
                    projectile_id: None,
                    victim_life_seq: None,
                    shooter_life_seq: None,
                    explicit_lethal: None,
                    source_snapshot_id: None,
                    source_snapshot_epoch: None,
                },
            ],
        }),
        "Watched damage for #9: 75 across 2 hits."
    );

    let mut scores = BTreeMap::new();
    assert_eq!(network_objective_status(&scores, None, None, false), None);
    scores.insert(3, 12_345);
    scores.insert(4, 7);
    assert_eq!(
        network_objective_status(&scores, Some(111), Some(222), true),
        Some("Server  Ball F3:12345 F4:7  KOTH 2.2  Personal 1.1  UFO".to_string())
    );
}

#[test]
fn notifications_trim_to_limit_and_expire() {
    let mut notifications = Vec::new();
    for index in 0..NOTIFICATION_LIMIT + 2 {
        push_notification_entry(
            &mut notifications,
            format!("note {index}"),
            WHITE,
            NOTIFICATION_LIMIT,
            NOTIFICATION_LIFETIME,
        );
    }

    assert_eq!(notifications.len(), NOTIFICATION_LIMIT);
    assert_eq!(
        notifications.first().map(|entry| entry.message.as_str()),
        Some("note 2")
    );

    update_notification_entries(&mut notifications, NOTIFICATION_LIFETIME - 0.1);
    assert_eq!(notifications.len(), NOTIFICATION_LIMIT);

    update_notification_entries(&mut notifications, 0.1);
    assert!(notifications.is_empty());
}

#[test]
fn notification_layout_matches_reference_game_reference_slots() {
    assert_eq!(NOTIFICATION_LIMIT, 7);
    assert_eq!(NOTIFICATION_LIFETIME, 5.0);
    assert_eq!(NOTIFICATION_TEXT_SCALE, 0.875);
    assert_eq!(NOTIFICATION_ANCHOR_X_FRACTION, 0.2);
    assert_eq!(NOTIFICATION_ANCHOR_Y_FRACTION, 0.6);
}
