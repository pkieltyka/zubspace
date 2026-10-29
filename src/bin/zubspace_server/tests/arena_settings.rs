use super::*;

#[test]
fn rocket_authority_helpers_match_client_chaos_tuning() {
    assert_eq!(arena_ship_rocket_lifetime_ticks(0), 1_000);
    assert_eq!(arena_ship_initial_rocket_count(0), 0);
    assert_eq!(arena_ship_max_rocket_count(0), 3);
}

/// Owner decision: Chaos plays *exact* bullet/burst/shrapnel damage
/// (its SVS-derived profile, since no live Chaos config was ever captured)
/// while Extreme keeps the randomized behavior of its hash-verified upstream
/// Extreme Games config. Pin both the packet flag and the file the
/// hand-authored Chaos builder claims to mirror, so neither choice drifts
/// silently.
#[test]
fn bundled_zones_pin_their_exact_damage_choice() {
    let chaos = arena_settings_packet();
    assert_eq!(
        read_u32_at(chaos, 0) & (1 << 8),
        1 << 8,
        "Chaos ExactDamage"
    );
    assert_eq!(
        chaos_conf_value(&chaos_arena_conf_values(), "Bullet", "ExactDamage"),
        "1",
        "the Chaos profile must declare the choice the hand-authored packet encodes"
    );

    let extreme =
        crate::arena_settings::arena_settings_packet_for_zone(crate::zone::EXTREME_ZONE_ID)
            .unwrap();
    assert_eq!(
        read_u32_at(extreme, 0) & (1 << 8),
        0,
        "Extreme ExactDamage stays randomized"
    );
}

#[test]
fn bundled_zone_prize_authority_settings_are_read_from_each_exact_packet() {
    for (zone_id, shutdown, hide_count, damage_factors, shrapnel_rates) in [
        (
            crate::zone::CHAOS_ZONE_ID,
            700_i16,
            10_u8,
            [30_u16; 8],
            [2_u8; 8],
        ),
        (
            crate::zone::EXTREME_ZONE_ID,
            150_i16,
            90_u8,
            [5_000_u16; 8],
            [3_u8, 7, 3, 7, 3, 3, 3, 3],
        ),
    ] {
        let packet = crate::arena_settings::arena_settings_packet_for_zone(zone_id)
            .expect("bundled settings packet");
        assert_eq!(
            i16::from_le_bytes(
                packet[CONTINUUM_SHORT_SETTINGS_OFFSET + 14..CONTINUUM_SHORT_SETTINGS_OFFSET + 16]
                    .try_into()
                    .unwrap()
            ),
            shutdown,
            "{zone_id} EngineShutdownTime"
        );
        assert_eq!(
            packet[CONTINUUM_BYTE_SETTINGS_OFFSET + 13],
            hide_count,
            "{zone_id} PrizeHideCount"
        );
        assert!(
            packet[CONTINUUM_PRIZE_WEIGHTS_OFFSET
                ..CONTINUUM_PRIZE_WEIGHTS_OFFSET + CONTINUUM_PRIZE_WEIGHT_COUNT]
                .iter()
                .any(|weight| *weight > 0),
            "{zone_id} has a nonempty authored prize table"
        );
        for (ship, expected) in damage_factors.into_iter().enumerate() {
            assert_eq!(
                read_u16_at(
                    packet,
                    CONTINUUM_SHIP_SETTINGS_OFFSET + ship * CONTINUUM_SHIP_SETTINGS_SIZE + 92,
                ),
                expected,
                "{zone_id} ship {ship} DamageFactor"
            );
            let capability_offset =
                CONTINUUM_SHIP_SETTINGS_OFFSET + ship * CONTINUUM_SHIP_SETTINGS_SIZE + 124;
            let capability = u32::from_le_bytes(
                packet[capability_offset..capability_offset + 4]
                    .try_into()
                    .unwrap(),
            );
            assert_eq!(
                ((capability >> 5) & 0x1f) as u8,
                shrapnel_rates[ship],
                "{zone_id} ship {ship} ShrapnelRate",
            );
        }
    }
}

#[test]
fn server_builds_complete_nonzero_arena_settings_packet() {
    let settings = arena_settings_packet();
    assert_eq!(settings.len(), CONTINUUM_ARENA_SETTINGS_SIZE);
    assert_eq!(settings[0], ARENA_SETTINGS);
    assert_ne!(settings, vec![0; CONTINUUM_ARENA_SETTINGS_SIZE]);
    assert_eq!(
        i32::from_le_bytes(
            settings[CONTINUUM_GLOBAL_INTS_OFFSET..CONTINUUM_GLOBAL_INTS_OFFSET + 4]
                .try_into()
                .unwrap()
        ),
        200_000
    );
    assert_eq!(
        i16::from_le_bytes(
            settings[CONTINUUM_SHORT_SETTINGS_OFFSET + 22..CONTINUUM_SHORT_SETTINGS_OFFSET + 24]
                .try_into()
                .unwrap()
        ),
        48
    );
    for ship_index in 0..8 {
        let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE;
        assert_eq!(read_u16_at(settings, offset + 24), 2000);
        assert_eq!(read_u16_at(settings, offset + 48), 1700);
        assert_eq!((read_u16_at(settings, offset + 28) >> 3) & 0xff, 14);
        let capability =
            u32::from_le_bytes(settings[offset + 124..offset + 128].try_into().unwrap());
        // Arena settings capability fields store 0..=3 directly.
        assert_eq!((capability >> 18) & 0x03, 3);
        assert_eq!((capability >> 20) & 0x03, 3);
    }
}

#[test]
fn server_respawn_delay_uses_zubspace_two_second_floor() {
    assert_eq!(
        crate::arena_settings::arena_respawn_delay(),
        std::time::Duration::from_secs(2)
    );
}

#[test]
fn server_chaos_ship_settings_cover_movement_energy_bounty_and_weapons_per_ship() {
    let settings = arena_settings_packet();
    let global_weapon_ints = [
        (0, 200_000),
        (4, 750_000),
        (8, 550),
        (12, 8000),
        (16, 1500),
        (28, 10_000),
        (32, 5000),
        (36, 12_000),
        (40, 700_000),
        (44, 100_000),
        (48, 0),
        (52, 12_000),
        (56, 25),
        (60, 5500),
        (64, 3000),
        (76, 3000),
    ];
    for (offset, expected) in global_weapon_ints {
        assert_eq!(
            read_i32_at(settings, CONTINUUM_GLOBAL_INTS_OFFSET + offset),
            expected
        );
    }
    assert_eq!(arena_max_frequency(), 10_000);

    let short_weapon_settings = [
        (2, 150),
        (6, 80),
        (8, 800),
        (14, 700),
        (30, 225),
        (32, 512),
        (66, 12_000),
        (70, 400),
        (72, 1000),
        (80, 1000),
        (82, 1000),
    ];
    for (offset, expected) in short_weapon_settings {
        assert_eq!(
            read_i16_at(settings, CONTINUUM_SHORT_SETTINGS_OFFSET + offset),
            expected
        );
    }

    let short_prize_settings = [
        (40, 10_000),
        (42, 700),
        (44, 10),
        (46, 6),
        (48, 8000),
        (50, 4000),
        (52, 300),
        (64, 10),
    ];
    for (offset, expected) in short_prize_settings {
        assert_eq!(
            read_i16_at(settings, CONTINUUM_SHORT_SETTINGS_OFFSET + offset),
            expected
        );
    }

    let short_flag_soccer_settings = [
        (88, 1000),
        (90, 1000),
        (92, 0),
        (94, 20),
        (100, 0),
        (102, 0),
    ];
    for (offset, expected) in short_flag_soccer_settings {
        assert_eq!(
            read_i16_at(settings, CONTINUUM_SHORT_SETTINGS_OFFSET + offset),
            expected
        );
    }

    let movement = [
        (290, 19, 3250, 1150, 1700, 24, 50),
        (230, 17, 3750, 1150, 1700, 24, 50),
        (230, 18, 3250, 1150, 1700, 24, 50),
        (230, 18, 3250, 1150, 1700, 24, 50),
        (230, 18, 3250, 1150, 1700, 25, 50),
        (230, 18, 3250, 1150, 1700, 24, 50),
        (230, 18, 3250, 1150, 1700, 24, 50),
        (260, 18, 3250, 1150, 1700, 16, 40),
    ];
    let max_bombs = [2, 2, 2, 3, 2, 2, 2, 2];
    let shrapnel_max = [8, 8, 8, 8, 8, 0, 8, 8];
    let cloak_energy = [0, 0, 100, 0, 0, 0, 0, 100];
    let cloak_status = [0, 0, 2, 0, 0, 0, 0, 2];
    let double_barrel = [false, false, false, false, true, false, false, false];
    let emp_bomb = [false, false, false, false, false, true, false, false];
    let see_mines = [false, false, false, false, false, true, false, false];
    let see_bomb_level = [0, 0, 0, 0, 0, 0, 0, 1];
    let bomb_bounces = [0, 0, 0, 0, 0, 0, 1, 0];
    let gravity = [1500, 1500, 1500, 1500, 1500, 1500, 1500, 0];

    for ship_index in 0..8 {
        let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE;
        let (rotation, thrust, speed, recharge, energy, fire_delay, multifire_delay) =
            movement[ship_index];

        assert_eq!(read_u32_at(settings, offset), 6000);
        assert_eq!(read_u32_at(settings, offset + 4), 4000);
        assert_eq!(read_i16_at(settings, offset + 8), gravity[ship_index]);
        assert_eq!(read_i16_at(settings, offset + 10), 100);
        assert_eq!(read_u16_at(settings, offset + 40), rotation);
        assert_eq!(read_u16_at(settings, offset + 42), thrust);
        assert_eq!(read_u16_at(settings, offset + 44), speed);
        assert_eq!(read_u16_at(settings, offset + 46), recharge);
        assert_eq!(read_u16_at(settings, offset + 48), energy);
        assert_eq!(read_u16_at(settings, offset + 50), 200);
        assert_eq!(read_u16_at(settings, offset + 52), 15);
        assert_eq!(read_u16_at(settings, offset + 54), 2010);
        assert_eq!(read_u16_at(settings, offset + 56), 400);
        assert_eq!(read_u16_at(settings, offset + 58), 1700);
        assert_eq!(read_u16_at(settings, offset + 60), 40);
        assert_eq!(read_u16_at(settings, offset + 62), 2);
        assert_eq!(read_u16_at(settings, offset + 64), 250);
        assert_eq!(read_u16_at(settings, offset + 66), 166);
        assert_eq!(read_u16_at(settings, offset + 68), 100);
        assert_eq!(read_u16_at(settings, offset + 70), 1200);

        assert_eq!(
            read_u16_at(settings, offset + 28) & 0x03,
            see_bomb_level[ship_index]
        );
        assert_eq!(read_u16_at(settings, offset + 32), cloak_energy[ship_index]);
        assert_eq!(read_u16_at(settings, offset + 34), 333);
        assert_eq!(read_u16_at(settings, offset + 36), 100);
        assert_eq!(read_u16_at(settings, offset + 38), 250);
        assert_eq!(read_u16_at(settings, offset + 12), 20);
        assert_eq!(read_u16_at(settings, offset + 14), 30);
        assert_eq!(read_u16_at(settings, offset + 16), 300);
        assert_eq!(read_u16_at(settings, offset + 18), 50);
        assert_eq!(read_u16_at(settings, offset + 20), 270);
        assert_eq!(read_u16_at(settings, offset + 22), 150);
        assert_eq!(read_u16_at(settings, offset + 24), 2000);
        assert_eq!(read_u16_at(settings, offset + 26), 2000);
        assert_eq!(read_u16_at(settings, offset + 30), 500);
        assert_eq!(read_u16_at(settings, offset + 72), 400);
        assert_eq!(read_u16_at(settings, offset + 74), 3000);
        assert_eq!(read_i16_at(settings, offset + 76), 1);
        assert_eq!(read_i16_at(settings, offset + 78), 125);
        assert_eq!(read_u16_at(settings, offset + 80), fire_delay);
        assert_eq!(read_u16_at(settings, offset + 82), multifire_delay);
        assert_eq!(read_u16_at(settings, offset + 84), 150);
        assert_eq!(read_u16_at(settings, offset + 86), 125);
        assert_eq!(read_u16_at(settings, offset + 88), 1000);

        assert_eq!(read_u16_at(settings, offset + 90), 30);
        assert_eq!(read_u16_at(settings, offset + 92), 30);
        assert_eq!(read_u16_at(settings, offset + 94), 100);
        assert_eq!(read_u16_at(settings, offset + 96), 0);
        assert_eq!(read_u16_at(settings, offset + 98), 1200);
        assert_eq!(read_u16_at(settings, offset + 100), 12);
        assert_eq!(read_u16_at(settings, offset + 102), 64);
        assert_eq!(read_u16_at(settings, offset + 104), 2500);

        assert_eq!(settings[offset + 106], 5);
        assert_eq!(settings[offset + 107], 24);
        assert_eq!(settings[offset + 108], 5);
        assert_eq!(settings[offset + 109], 3);
        assert_eq!(settings[offset + 110], 3);
        assert_eq!(settings[offset + 111], 3);
        assert_eq!(settings[offset + 112], 3);
        assert_eq!(settings[offset + 113], 3);
        assert_eq!(settings[offset + 114], 3);
        assert_eq!(settings[offset + 115], 3);
        assert_eq!(settings[offset + 116], 0);
        assert_eq!(settings[offset + 117], 0);
        assert_eq!(settings[offset + 118], 0);
        assert_eq!(settings[offset + 119], 0);
        assert_eq!(settings[offset + 120], 0);
        assert_eq!(settings[offset + 121], 0);
        assert_eq!(settings[offset + 122], 0);
        assert_eq!(settings[offset + 123], bomb_bounces[ship_index]);

        let capability = read_u32_at(settings, offset + 124);
        assert_eq!(capability & 0x1f, shrapnel_max[ship_index] as u32);
        assert_eq!((capability >> 5) & 0x1f, 2);
        assert_eq!((capability >> 10) & 0x03, cloak_status[ship_index] as u32);
        assert_eq!((capability >> 12) & 0x03, 1);
        assert_eq!((capability >> 14) & 0x03, 1);
        assert_eq!((capability >> 16) & 0x03, 1);
        assert_eq!((capability >> 18) & 0x03, 3);
        assert_eq!((capability >> 20) & 0x03, 3);
        assert_eq!((capability >> 22) & 0x03, 0);
        assert_eq!((capability >> 24) & 0x03, max_bombs[ship_index] as u32);
        assert_eq!(capability & (1 << 26) != 0, double_barrel[ship_index]);
        assert_eq!(capability & (1 << 27) != 0, emp_bomb[ship_index]);
        assert_eq!(capability & (1 << 28) != 0, see_mines[ship_index]);
    }

    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 4], 1);
    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 7], 12);
    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 8], 1);
    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 11], 0);
    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 13], 10);
    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 16], 1);
    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 19], 0);
    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 20], 0);
    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 21], 0);
    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 22], 0);
    assert_eq!(settings[CONTINUUM_BYTE_SETTINGS_OFFSET + 23], 0);

    assert_eq!(
        &settings[CONTINUUM_PRIZE_WEIGHTS_OFFSET
            ..CONTINUUM_PRIZE_WEIGHTS_OFFSET + CONTINUUM_PRIZE_WEIGHT_COUNT],
        // MultiFire weight (index 14) is intentionally 0 — see the
        // comment on `SVS_PRIZE_WEIGHTS` in
        // `arena_settings/chaos.rs` for the rationale.
        &[
            40, 40, 40, 40, 25, 40, 7, 25, 25, 25, 40, 40, 10, 0, 0, 25, 10, 10, 30, 25, 70, 70,
            40, 30, 130, 200, 50, 60,
        ]
    );
}

#[test]
fn server_chaos_settings_packet_matches_local_arena_conf_for_gameplay_fields() {
    let settings = arena_settings_packet();
    let conf = chaos_arena_conf_values();
    let ship_sections = [
        "Warbird",
        "Javelin",
        "Spider",
        "Leviathan",
        "Terrier",
        "Weasel",
        "Lancaster",
        "Shark",
    ];

    for (ship_index, ship) in ship_sections.into_iter().enumerate() {
        let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE;
        for (key, field_offset) in [("SuperTime", 0), ("ShieldsTime", 4)] {
            assert_eq!(
                read_u32_at(settings, offset + field_offset),
                chaos_conf_ship_u32(&conf, ship, key),
                "{ship}:{key}"
            );
        }
        for (key, field_offset) in [
            ("Gravity", 8),
            ("GravityTopSpeed", 10),
            ("TurretThrustPenalty", 76),
            ("TurretSpeedPenalty", 78),
        ] {
            assert_eq!(
                read_i16_at(settings, offset + field_offset),
                chaos_conf_ship_i16(&conf, ship, key),
                "{ship}:{key}"
            );
        }
        for (key, field_offset) in [
            ("BulletFireEnergy", 12),
            ("MultiFireEnergy", 14),
            ("BombFireEnergy", 16),
            ("BombFireEnergyUpgrade", 18),
            ("LandmineFireEnergy", 20),
            ("LandmineFireEnergyUpgrade", 22),
            ("BulletSpeed", 24),
            ("BombSpeed", 26),
            ("MultiFireAngle", 30),
            ("CloakEnergy", 32),
            ("StealthEnergy", 34),
            ("AntiWarpEnergy", 36),
            ("XRadarEnergy", 38),
            ("MaximumRotation", 40),
            ("MaximumThrust", 42),
            ("MaximumSpeed", 44),
            ("MaximumRecharge", 46),
            ("MaximumEnergy", 48),
            ("InitialRotation", 50),
            ("InitialThrust", 52),
            ("InitialSpeed", 54),
            ("InitialRecharge", 56),
            ("InitialEnergy", 58),
            ("UpgradeRotation", 60),
            ("UpgradeThrust", 62),
            ("UpgradeSpeed", 64),
            ("UpgradeRecharge", 66),
            ("UpgradeEnergy", 68),
            ("AfterburnerEnergy", 70),
            ("BombThrust", 72),
            ("BurstSpeed", 74),
            ("BulletFireDelay", 80),
            ("MultiFireDelay", 82),
            ("BombFireDelay", 84),
            ("LandmineFireDelay", 86),
            ("RocketTime", 88),
            ("InitialBounty", 90),
            ("DamageFactor", 92),
            ("PrizeShareLimit", 94),
            ("AttachBounty", 96),
            ("SoccerThrowTime", 98),
            ("SoccerBallFriction", 100),
            ("SoccerBallProximity", 102),
            ("SoccerBallSpeed", 104),
        ] {
            assert_eq!(
                read_u16_at(settings, offset + field_offset),
                chaos_conf_ship_u16(&conf, ship, key),
                "{ship}:{key}"
            );
        }

        let visibility = read_u16_at(settings, offset + 28);
        assert_eq!(
            (visibility >> 3) & 0xff,
            chaos_conf_ship_u16(&conf, ship, "Radius"),
            "{ship}:Radius"
        );
        assert_eq!(
            visibility & 0x03,
            chaos_conf_ship_u16(&conf, ship, "SeeBombLevel"),
            "{ship}:SeeBombLevel"
        );
        assert_eq!(
            visibility & (1 << 2) != 0,
            chaos_conf_ship_bool(&conf, ship, "DisableFastShooting"),
            "{ship}:DisableFastShooting"
        );

        for (key, field_offset) in [
            ("TurretLimit", 106),
            ("BurstShrapnel", 107),
            ("MaxMines", 108),
            ("RepelMax", 109),
            ("BurstMax", 110),
            ("DecoyMax", 111),
            ("ThorMax", 112),
            ("BrickMax", 113),
            ("RocketMax", 114),
            ("PortalMax", 115),
            ("InitialRepel", 116),
            ("InitialBurst", 117),
            ("InitialBrick", 118),
            ("InitialRocket", 119),
            ("InitialThor", 120),
            ("InitialDecoy", 121),
            ("InitialPortal", 122),
            ("BombBounceCount", 123),
        ] {
            assert_eq!(
                settings[offset + field_offset],
                chaos_conf_ship_u8(&conf, ship, key),
                "{ship}:{key}"
            );
        }

        let capability = read_u32_at(settings, offset + 124);
        assert_eq!(
            capability & 0x1f,
            chaos_conf_ship_u32(&conf, ship, "ShrapnelMax"),
            "{ship}:ShrapnelMax"
        );
        assert_eq!(
            (capability >> 5) & 0x1f,
            chaos_conf_ship_u32(&conf, ship, "ShrapnelRate"),
            "{ship}:ShrapnelRate"
        );
        assert_eq!(
            (capability >> 10) & 0x03,
            chaos_conf_ship_u32(&conf, ship, "CloakStatus"),
            "{ship}:CloakStatus"
        );
        assert_eq!(
            (capability >> 12) & 0x03,
            chaos_conf_ship_u32(&conf, ship, "StealthStatus"),
            "{ship}:StealthStatus"
        );
        assert_eq!(
            (capability >> 14) & 0x03,
            chaos_conf_ship_u32(&conf, ship, "XRadarStatus"),
            "{ship}:XRadarStatus"
        );
        assert_eq!(
            (capability >> 16) & 0x03,
            chaos_conf_ship_u32(&conf, ship, "AntiWarpStatus"),
            "{ship}:AntiWarpStatus"
        );
        // Arena settings weapon capability fields store 0..=3 directly;
        // unlike per-shot weapon_data, 0 means "no weapon".
        assert_eq!(
            (capability >> 18) & 0x03,
            chaos_conf_ship_u32(&conf, ship, "InitialGuns"),
            "{ship}:InitialGuns"
        );
        assert_eq!(
            (capability >> 20) & 0x03,
            chaos_conf_ship_u32(&conf, ship, "MaxGuns"),
            "{ship}:MaxGuns"
        );
        assert_eq!(
            (capability >> 22) & 0x03,
            chaos_conf_ship_u32(&conf, ship, "InitialBombs"),
            "{ship}:InitialBombs"
        );
        assert_eq!(
            (capability >> 24) & 0x03,
            chaos_conf_ship_u32(&conf, ship, "MaxBombs"),
            "{ship}:MaxBombs"
        );
        assert_eq!(
            capability & (1 << 26) != 0,
            chaos_conf_ship_bool(&conf, ship, "DoubleBarrel"),
            "{ship}:DoubleBarrel"
        );
        assert_eq!(
            capability & (1 << 27) != 0,
            chaos_conf_ship_bool(&conf, ship, "EmpBomb"),
            "{ship}:EmpBomb"
        );
        assert_eq!(
            capability & (1 << 28) != 0,
            chaos_conf_ship_bool(&conf, ship, "SeeMines"),
            "{ship}:SeeMines"
        );
    }

    for (section, key, offset) in [
        ("Bullet", "BulletDamageLevel", 0),
        ("Bomb", "BombDamageLevel", 4),
        ("Bullet", "BulletAliveTime", 8),
        ("Bomb", "BombAliveTime", 12),
        ("Misc", "DecoyAliveTime", 16),
        ("Misc", "MaxFrequency", 28),
        ("Repel", "RepelSpeed", 32),
        ("Mine", "MineAliveTime", 36),
        ("Burst", "BurstDamageLevel", 40),
        ("Bullet", "BulletDamageUpgrade", 44),
        ("Rocket", "RocketThrust", 56),
        ("Rocket", "RocketSpeed", 60),
        ("Shrapnel", "InactiveShrapDamage", 64),
        ("Shrapnel", "ShrapnelSpeed", 76),
    ] {
        assert_eq!(
            read_i32_at(settings, CONTINUUM_GLOBAL_INTS_OFFSET + offset),
            chaos_conf_i32(&conf, section, key),
            "{section}:{key}"
        );
    }

    for (section, key, offset) in [
        ("Bomb", "BombExplodeDelay", 2),
        ("Bomb", "BombExplodePixels", 6),
        ("Prize", "DeathPrizeTime", 8),
        ("Kill", "EnterDelay", 12),
        ("Prize", "EngineShutdownTime", 14),
        ("Bomb", "ProximityDistance", 16),
        ("Kill", "BountyIncreaseForKill", 18),
        ("Misc", "BounceFactor", 20),
        ("Radar", "MapZoomFactor", 22),
        ("Repel", "RepelTime", 30),
        ("Repel", "RepelDistance", 32),
        ("Prize", "PrizeFactor", 40),
        ("Prize", "PrizeDelay", 42),
        ("Prize", "MinimumVirtual", 44),
        ("Prize", "UpgradeVirtual", 46),
        ("Prize", "PrizeMaxExist", 48),
        ("Prize", "PrizeMinExist", 50),
        ("Prize", "PrizeNegativeFactor", 52),
        ("Door", "DoorDelay", 54),
        ("Toggle", "AntiWarpPixels", 56),
        ("Door", "DoorMode", 58),
        ("Prize", "MultiPrizeCount", 64),
        ("Brick", "BrickTime", 66),
        ("Misc", "WarpRadiusLimit", 68),
        ("Bomb", "EBombShutdownTime", 70),
        ("Bomb", "EBombDamagePercent", 72),
        ("Misc", "WarpPointDelay", 76),
        ("Bomb", "BBombDamagePercent", 80),
        ("Shrapnel", "ShrapnelDamagePercent", 82),
    ] {
        assert_eq!(
            read_i16_at(settings, CONTINUUM_SHORT_SETTINGS_OFFSET + offset),
            chaos_conf_i16(&conf, section, key),
            "{section}:{key}"
        );
    }
    assert_eq!(
        crate::arena_settings::arena_multiprize_count() as i16,
        chaos_conf_i16(&conf, "Prize", "MultiPrizeCount")
    );
    assert_eq!(
        crate::arena_settings::arena_prize_claim_retention(),
        Duration::from_millis(chaos_conf_i16(&conf, "Prize", "PrizeMaxExist").max(0) as u64 * 10)
    );

    for (section, key, offset) in [
        ("Shrapnel", "Random", 0),
        ("Misc", "AllowBombs", 2),
        ("Misc", "AllowGuns", 3),
        ("Mine", "TeamMaxMines", 7),
        ("Wormhole", "GravityBombs", 8),
        ("Bomb", "BombSafety", 9),
        ("Prize", "TakePrizeReliable", 11),
        ("Prize", "PrizeHideCount", 13),
        ("Radar", "RadarMode", 18),
        ("Misc", "VictoryMusic", 19),
    ] {
        assert_eq!(
            settings[CONTINUUM_BYTE_SETTINGS_OFFSET + offset],
            chaos_conf_u8(&conf, section, key),
            "{section}:{key}"
        );
    }

    for (index, key) in [
        "QuickCharge",
        "Energy",
        "Rotation",
        "Stealth",
        "Cloak",
        "XRadar",
        "Warp",
        "Gun",
        "Bomb",
        "BouncingBullets",
        "Thruster",
        "TopSpeed",
        "Recharge",
        "Glue",
        "MultiFire",
        "Proximity",
        "AllWeapons",
        "Shields",
        "Shrapnel",
        "AntiWarp",
        "Repel",
        "Burst",
        "Decoy",
        "Thor",
        "MultiPrize",
        "Brick",
        "Rocket",
        "Portal",
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            settings[CONTINUUM_PRIZE_WEIGHTS_OFFSET + index],
            chaos_conf_u8(&conf, "PrizeWeight", key),
            "PrizeWeight:{key}"
        );
    }
}

#[test]
fn server_chaos_movement_settings_match_reference_game_unit_mapping() {
    let settings = arena_settings_packet();
    let movement = [
        (290, 19, 3250),
        (230, 17, 3750),
        (230, 18, 3250),
        (230, 18, 3250),
        (230, 18, 3250),
        (230, 18, 3250),
        (230, 18, 3250),
        (260, 18, 3250),
    ];

    for (ship_index, (maximum_rotation, maximum_thrust, maximum_speed)) in
        movement.into_iter().enumerate()
    {
        let offset = CONTINUUM_SHIP_SETTINGS_OFFSET + ship_index * CONTINUUM_SHIP_SETTINGS_SIZE;
        assert_eq!(read_u16_at(settings, offset + 40), maximum_rotation);
        assert_eq!(read_u16_at(settings, offset + 42), maximum_thrust);
        assert_eq!(read_u16_at(settings, offset + 44), maximum_speed);

        let initial_rotation_per_second = read_u16_at(settings, offset + 50) as f32 / 400.0;
        let maximum_rotation_per_second = read_u16_at(settings, offset + 40) as f32 / 400.0;
        let initial_thrust_pixels_per_second_squared =
            read_u16_at(settings, offset + 52) as f32 * 10.0;
        let afterburner_thrust_pixels_per_second_squared =
            read_u16_at(settings, offset + 42) as f32 * 10.0;
        let initial_speed_pixels_per_second = read_u16_at(settings, offset + 54) as f32 / 10.0;
        let afterburner_speed_pixels_per_second = read_u16_at(settings, offset + 44) as f32 / 10.0;

        assert_eq!(initial_rotation_per_second, 0.5);
        assert_eq!(maximum_rotation_per_second, maximum_rotation as f32 / 400.0);
        assert_eq!(initial_thrust_pixels_per_second_squared, 150.0);
        assert_eq!(
            afterburner_thrust_pixels_per_second_squared,
            maximum_thrust as f32 * 10.0
        );
        assert_eq!(
            arena_ship_max_thrust_pixels_per_second_squared(ship_index as u8),
            maximum_thrust as f32 * 10.0
        );
        assert_eq!(initial_speed_pixels_per_second, 201.0);
        assert_eq!(
            afterburner_speed_pixels_per_second,
            maximum_speed as f32 / 10.0
        );
    }
}

type ChaosConfValues = std::collections::BTreeMap<(String, String), String>;

fn chaos_arena_conf_values() -> ChaosConfValues {
    let mut section = String::new();
    let mut values = std::collections::BTreeMap::new();
    for raw_line in include_str!("../../../../assets/zones/chaos-zone/arena.conf").lines() {
        let line = raw_line
            .split_once(';')
            .map(|(line, _)| line)
            .unwrap_or(raw_line)
            .trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = normalize_chaos_conf_name(name);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        values.insert(
            (section.clone(), normalize_chaos_conf_name(key)),
            value.trim().to_string(),
        );
    }
    values
}

fn chaos_conf_ship_value<'a>(values: &'a ChaosConfValues, ship: &str, key: &str) -> &'a str {
    let ship = normalize_chaos_conf_name(ship);
    let key = normalize_chaos_conf_name(key);
    values
        .get(&(ship, key.clone()))
        .or_else(|| values.get(&(String::from("allships"), key.clone())))
        .unwrap_or_else(|| panic!("missing chaos arena setting for ship {key}"))
}

fn chaos_conf_value<'a>(values: &'a ChaosConfValues, section: &str, key: &str) -> &'a str {
    values
        .get(&(
            normalize_chaos_conf_name(section),
            normalize_chaos_conf_name(key),
        ))
        .unwrap_or_else(|| panic!("missing chaos arena setting {section}:{key}"))
}

fn chaos_conf_ship_u32(values: &ChaosConfValues, ship: &str, key: &str) -> u32 {
    chaos_conf_ship_value(values, ship, key)
        .parse()
        .expect("u32 chaos ship setting should parse")
}

fn chaos_conf_ship_u16(values: &ChaosConfValues, ship: &str, key: &str) -> u16 {
    chaos_conf_ship_value(values, ship, key)
        .parse()
        .expect("u16 chaos ship setting should parse")
}

fn chaos_conf_ship_i16(values: &ChaosConfValues, ship: &str, key: &str) -> i16 {
    chaos_conf_ship_value(values, ship, key)
        .parse()
        .expect("i16 chaos ship setting should parse")
}

fn chaos_conf_ship_u8(values: &ChaosConfValues, ship: &str, key: &str) -> u8 {
    chaos_conf_ship_value(values, ship, key)
        .parse()
        .expect("u8 chaos ship setting should parse")
}

fn chaos_conf_ship_bool(values: &ChaosConfValues, ship: &str, key: &str) -> bool {
    chaos_conf_ship_value(values, ship, key) != "0"
}

fn chaos_conf_i32(values: &ChaosConfValues, section: &str, key: &str) -> i32 {
    chaos_conf_value(values, section, key)
        .parse()
        .expect("i32 chaos arena setting should parse")
}

fn chaos_conf_i16(values: &ChaosConfValues, section: &str, key: &str) -> i16 {
    chaos_conf_value(values, section, key)
        .parse()
        .expect("i16 chaos arena setting should parse")
}

fn chaos_conf_u8(values: &ChaosConfValues, section: &str, key: &str) -> u8 {
    chaos_conf_value(values, section, key)
        .parse()
        .expect("u8 chaos arena setting should parse")
}

fn normalize_chaos_conf_name(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

#[test]
fn server_uses_chaos_ship_weapon_energy_costs() {
    assert_eq!(arena_ship_weapon_energy_cost(0, 2), 20);
    assert_eq!(arena_ship_weapon_energy_cost(0, 2 | (2 << 5)), 60);
    assert_eq!(arena_ship_weapon_energy_cost(0, 2 | (1 << 15)), 30);
    assert_eq!(arena_ship_weapon_energy_cost(0, 3), 300);
    assert_eq!(arena_ship_weapon_energy_cost(0, 3 | (2 << 5)), 400);
    assert_eq!(arena_ship_weapon_energy_cost(0, 3 | (1 << 15)), 270);
    assert_eq!(
        arena_ship_weapon_energy_cost(0, 3 | (2 << 5) | (1 << 15)),
        570
    );
    assert_eq!(arena_ship_weapon_energy_cost(0, 4), 300);
    assert_eq!(arena_ship_weapon_energy_cost(0, 4 | (2 << 5)), 400);
    assert_eq!(arena_ship_weapon_energy_cost(0, 4 | (1 << 15)), 270);
    assert_eq!(
        arena_ship_weapon_energy_cost(0, 4 | (2 << 5) | (1 << 15)),
        570
    );
    assert_eq!(arena_ship_weapon_energy_cost(0, 5), 0);
    assert_eq!(arena_ship_weapon_energy_cost(0, 8), 0);
}

#[test]
fn server_uses_chaos_bomb_level_caps() {
    assert_eq!(arena_ship_initial_bomb_level(0), 0);
    assert_eq!(arena_ship_max_bomb_level(0), 2);
    assert_eq!(arena_ship_initial_bomb_level(3), 0);
    assert_eq!(arena_ship_max_bomb_level(3), 3);
}

#[test]
fn server_uses_chaos_arena_weapon_damage_limits() {
    let damage_settings = arena_weapon_damage_settings();
    assert_eq!(damage_settings.bullet_damage_level, 200_000);
    assert_eq!(damage_settings.bullet_damage_upgrade, 100_000);
    assert_eq!(damage_settings.bomb_damage_level, 750_000);
    assert_eq!(damage_settings.burst_damage_level, 700_000);
    assert_eq!(damage_settings.inactive_shrapnel_damage, 3_000);
    assert_eq!(damage_settings.emp_bomb_damage_percent, 1000);
    assert_eq!(damage_settings.bouncing_bomb_damage_percent, 1000);
    assert_eq!(damage_settings.shrapnel_damage_percent, 1000);

    assert_eq!(
        maximum_reported_weapon_damage(3, damage_settings),
        Some(750)
    );
    assert_eq!(
        maximum_reported_weapon_damage(4 | (2 << 5), damage_settings),
        Some(750)
    );
    assert_eq!(
        maximum_reported_weapon_damage(1, damage_settings),
        Some(200)
    );
    assert_eq!(
        maximum_reported_weapon_damage(1 | (3 << 5), damage_settings),
        Some(500)
    );
    assert_eq!(
        maximum_reported_weapon_damage(7, damage_settings),
        Some(700)
    );
    assert_eq!(
        maximum_reported_weapon_damage(8 | (2 << 5), damage_settings),
        Some(3750)
    );
    assert_eq!(
        maximum_reported_weapon_damage(15 | (3 << 5), damage_settings),
        Some(500)
    );
}

#[test]
fn server_reads_chaos_ship_weapon_state_limits() {
    assert_eq!(arena_ship_shrapnel_max(0), 8);
    assert_eq!(arena_ship_shrapnel_rate(0), 2);
    assert_eq!(arena_ship_shrapnel_max(5), 0);
    assert_eq!(arena_ship_shrapnel_rate(5), 2);
}

#[test]
fn server_reads_chaos_mine_limits_and_lifetime() {
    assert_eq!(arena_ship_max_mines(0), 5);
    assert_eq!(arena_ship_max_mines(5), 5);
    assert_eq!(arena_team_max_mines(), 12);
    assert_eq!(arena_mine_lifetime(), Duration::from_secs(120));
}

#[test]
fn server_uses_chaos_weapon_travel_ranges_for_interim_damage_validation() {
    assert_eq!(
        arena_weapon_max_origin_distance_pixels(0, 0x83, 12_000),
        Some(17024.0)
    );
    assert_eq!(
        arena_weapon_max_origin_distance_pixels(0, 2, 12_000),
        Some(1612.0)
    );
    assert_eq!(arena_weapon_max_origin_distance_pixels(0, 7, 12_000), None);
}

#[test]
fn server_reads_chaos_bomb_safety_settings() {
    assert!(arena_bomb_safety_enabled());
    assert_eq!(arena_proximity_distance(), 3.0);
    assert_eq!(arena_bomb_explode_delay_ticks(), 150);
}

#[test]
fn server_security_challenge_and_report_validation_cover_map_and_settings() {
    let challenge = security_challenge();
    assert_eq!(challenge[0], SECURITY_CHALLENGE);
    assert_eq!(
        u32::from_le_bytes(challenge[13..17].try_into().unwrap()),
        SECURITY_CHECKSUM_KEY
    );

    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let valid = security_response_packet(
        continuum_settings_checksum(SECURITY_CHECKSUM_KEY, arena_settings_packet()),
        zone_map().map_checksum,
        Some(zone_map().continuum_crc32),
    );

    update_security_report(&state, player_id, &valid);

    let state_guard = state.lock().expect("server state should not be poisoned");
    let client = state_guard
        .clients
        .get(&player_id)
        .expect("client should still be connected");
    assert_eq!(client.last_security_valid, Some(true));
    assert_eq!(
        client.last_security_report,
        Some(ClientSecurityReport {
            settings_checksum: continuum_settings_checksum(
                SECURITY_CHECKSUM_KEY,
                arena_settings_packet()
            ),
            map_checksum: zone_map().map_checksum,
            continuum_map_crc32: Some(zone_map().continuum_crc32),
        })
    );
    drop(state_guard);

    let invalid = security_response_packet(
        continuum_settings_checksum(SECURITY_CHECKSUM_KEY, arena_settings_packet()),
        zone_map().map_checksum.wrapping_add(1),
        Some(zone_map().continuum_crc32),
    );
    update_security_report(&state, player_id, &invalid);
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&player_id)
            .and_then(|client| client.last_security_valid),
        Some(false)
    );
}

#[test]
fn selected_zone_security_checksum_uses_the_same_explicit_profile_as_join() {
    // Chaos itself is excluded: the reject leg feeds the Chaos profile in as
    // the borrowed value, and for a Chaos selection both legs carry the same
    // packet, so that leg would prove nothing. Iterating the installed catalog
    // keeps every other bundled zone covered automatically.
    for map in crate::zone::server_zone_maps()
        .iter()
        .filter(|map| map.zone_id.as_ref() != crate::zone::CHAOS_ZONE_ID)
    {
        let zone_id = map.zone_id.as_ref();
        let state = Arc::new(Mutex::new(ServerState {
            config: ServerConfig {
                default_zone: zone_id.to_string(),
                ..ServerConfig::default()
            },
            ..ServerState::default()
        }));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);

        let valid = security_response_packet(
            continuum_settings_checksum(
                SECURITY_CHECKSUM_KEY,
                crate::arena_settings::arena_settings_packet_for_zone(zone_id)
                    .expect("bundled zone settings"),
            ),
            map.map_checksum,
            Some(map.continuum_crc32),
        );
        update_security_report(&state, player_id, &valid);
        assert_eq!(
            state
                .lock()
                .expect("state lock")
                .clients
                .get(&player_id)
                .and_then(|client| client.last_security_valid),
            Some(true),
            "{zone_id} checksum should accept its own profile"
        );

        let chaos_checksum = security_response_packet(
            continuum_settings_checksum(
                SECURITY_CHECKSUM_KEY,
                crate::arena_settings::arena_settings_packet_for_zone(crate::zone::CHAOS_ZONE_ID)
                    .expect("bundled Chaos settings"),
            ),
            map.map_checksum,
            Some(map.continuum_crc32),
        );
        update_security_report(&state, player_id, &chaos_checksum);
        assert_eq!(
            state
                .lock()
                .expect("state lock")
                .clients
                .get(&player_id)
                .and_then(|client| client.last_security_valid),
            Some(false),
            "{zone_id} checksum must reject the formerly borrowed Chaos profile"
        );
    }
}

/// The Chaos zone ships its own values, and the afterburner feel depends entirely on them:
/// `arena.conf` sets `[AllShips] MaximumThrust=18` with `[Warbird]=19` and `[Javelin]=17`
/// overrides plus `AfterburnerEnergy=1200`, and the reference charges that as 120 energy per
/// second. The ship table is hand-written, so this is the check that it still says what the
/// authored zone says - including that ships genuinely differ from each other.
#[test]
fn chaos_afterburner_tuning_matches_the_authored_zone_values() {
    use crate::arena_settings::{
        arena_ship_afterburner_energy_per_second,
        arena_ship_initial_thrust_pixels_per_second_squared,
        arena_ship_initial_top_speed_pixels_per_second, arena_ship_max_speed_pixels_per_second,
    };
    assert_eq!(arena_ship_max_thrust_pixels_per_second_squared(0), 190.0);
    assert_eq!(arena_ship_max_thrust_pixels_per_second_squared(1), 170.0);
    assert_eq!(arena_ship_max_thrust_pixels_per_second_squared(2), 180.0);
    assert_eq!(
        arena_ship_initial_thrust_pixels_per_second_squared(0),
        150.0
    );
    for ship in 0..8 {
        assert_eq!(
            arena_ship_afterburner_energy_per_second(ship),
            120.0,
            "ship {ship} charges AfterburnerEnergy / 10"
        );
        assert!(
            arena_ship_max_thrust_pixels_per_second_squared(ship)
                > arena_ship_initial_thrust_pixels_per_second_squared(ship),
            "ship {ship} must actually gain thrust from afterburners"
        );
        assert!(
            arena_ship_max_speed_pixels_per_second(ship)
                > arena_ship_initial_top_speed_pixels_per_second(ship),
            "ship {ship} must actually gain top speed from afterburners"
        );
    }
}
