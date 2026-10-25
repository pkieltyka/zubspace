use super::*;
use crate::continuum::continuum_radius_pixels;

#[test]
fn default_ship_tuning_preserves_current_arcade_values() {
    let tuning = ArenaTuning::continuum_skirmish_default();
    let warbird = tuning.ship(0);

    assert_eq!(warbird.radius_pixels, SHIP_RADIUS);
    assert_eq!(warbird.energy_max, ENERGY_MAX);
    assert_eq!(warbird.energy_recharge, ENERGY_RECHARGE);
    assert_eq!(warbird.recharge_upgrade_step, RECHARGE_UPGRADE_STEP);
    assert_eq!(
        warbird.energy_recharge_max,
        ENERGY_RECHARGE + RECHARGE_UPGRADE_STEP * STAT_UPGRADE_CAP as f32
    );
    assert_eq!(warbird.energy_upgrade_step, 350.0);
    assert_eq!(warbird.stealth_energy_per_second, 45.0);
    assert_eq!(warbird.cloak_energy_per_second, 70.0);
    assert_eq!(warbird.xradar_energy_per_second, 40.0);
    assert_eq!(warbird.antiwarp_energy_per_second, 55.0);
    assert!(warbird.stealth_available);
    assert!(warbird.cloak_available);
    assert!(warbird.xradar_available);
    assert!(warbird.antiwarp_available);
    assert_eq!(warbird.shrapnel_max, MAX_SHRAPNEL_UPGRADES);
    assert_eq!(warbird.shrapnel_rate, 1);
    assert!((warbird.rotation_per_second - 1.445).abs() < 0.001);
    assert_eq!(warbird.rotation_upgrade_step, ROTATION_UPGRADE_STEP);
    assert_eq!(
        warbird.rotation_max_per_second,
        (1.7 + ROTATION_UPGRADE_STEP * STAT_UPGRADE_CAP as f32) * 0.85
    );
    assert_eq!(warbird.thrust_acceleration, THRUST_ACCELERATION);
    assert_eq!(warbird.thrust_upgrade_step, THRUST_UPGRADE_STEP);
    assert_eq!(
        warbird.thrust_max_acceleration,
        THRUST_ACCELERATION + THRUST_UPGRADE_STEP * STAT_UPGRADE_CAP as f32
    );
    assert_eq!(warbird.top_speed, SHIP_TOP_SPEED);
    assert_eq!(warbird.top_speed_upgrade_step, TOP_SPEED_UPGRADE_STEP);
    assert_eq!(
        warbird.top_speed_max,
        SHIP_TOP_SPEED + TOP_SPEED_UPGRADE_STEP * STAT_UPGRADE_CAP as f32
    );
    assert_eq!(warbird.bullet_energy, 30.0);
    assert_eq!(warbird.bullet_speed, 760.0);
    assert_eq!(warbird.initial_gun_level, 3);
    assert_eq!(warbird.max_gun_level, 3);
    assert_eq!(warbird.multifire_energy, MULTIFIRE_ENERGY_COST);
    assert!(!warbird.double_barrel);
    assert_eq!(warbird.burst_energy, BURST_ENERGY_COST);
    assert_eq!(warbird.burst_bullet_count, BURST_BULLET_COUNT);
    assert_eq!(warbird.bomb_bounces, BOMB_BOUNCES);
    assert_eq!(warbird.max_mines, MAX_MINES);
    assert_eq!(warbird.repel_energy, 80.0);
    assert_eq!(warbird.initial_repels, 3);
    assert_eq!(warbird.max_repels, 5);
    assert_eq!(warbird.initial_bursts, 3);
    assert_eq!(warbird.max_bursts, 5);
    assert_eq!(warbird.initial_bricks, 3);
    assert_eq!(warbird.max_bricks, 5);
    assert_eq!(warbird.initial_decoys, 3);
    assert_eq!(warbird.max_decoys, 5);
    assert_eq!(warbird.initial_thors, 3);
    assert_eq!(warbird.max_thors, 5);
    assert_eq!(warbird.initial_rockets, 3);
    assert_eq!(warbird.max_rockets, 5);
    assert_eq!(warbird.rocket_cooldown, 0.0);
    assert_eq!(warbird.initial_portals, 3);
    assert_eq!(warbird.max_portals, 5);
    assert_eq!(warbird.super_lifetime, SUPER_LIFETIME);
    assert_eq!(warbird.shield_lifetime, SHIELD_LIFETIME);
    assert_eq!(warbird.engine_shutdown_lifetime, ENGINE_SHUTDOWN_LIFETIME);
    assert_eq!(tuning.map_zoom_factor, 48);
}

#[test]
fn continuum_ship_radius_wire_byte_is_already_in_pixels() {
    // This fixture is intentionally independent of the packet builder. ASSS
    // and SubspaceServer put the configured pixel count directly on the wire;
    // the reference game's division by 16 converts into its tile-scaled world only.
    assert_eq!(continuum_radius_pixels(0), 14.0);
    assert_eq!(continuum_radius_pixels(7), 7.0);
    assert_eq!(continuum_radius_pixels(14), 14.0);
    assert_eq!(continuum_radius_pixels(224), 224.0);
}

#[test]
fn parses_continuum_arena_settings_packet_layout() {
    let data = sample_continuum_settings_packet();
    let settings = ContinuumArenaSettings::parse(&data).expect("settings should parse");
    let ship = settings.ships[0];

    assert_eq!(settings.arena_type, 3);
    assert!(settings.exact_damage);
    assert!(settings.hide_flags);
    assert!(settings.no_xradar);
    assert!(settings.disable_ball_through_walls);
    assert_eq!(settings.bullet_damage_level, 90_000);
    assert_eq!(settings.bomb_damage_level, 260_000);
    assert_eq!(settings.burst_damage_level, 70_000);
    assert_eq!(settings.bullet_damage_upgrade, 10_000);
    assert_eq!(settings.flag_drop_delay, 12_000);
    assert_eq!(settings.enter_game_flagging_delay, 500);
    assert_eq!(settings.bullet_alive_time, 125);
    assert_eq!(settings.bomb_alive_time, 240);
    assert_eq!(settings.decoy_alive_time, 1200);
    assert_eq!(settings.max_frequency, 1000);
    assert_eq!(settings.repel_speed, 5200);
    assert_eq!(settings.mine_alive_time, 4500);
    assert_eq!(settings.rocket_thrust, 92);
    assert_eq!(settings.rocket_speed, 7000);
    assert_eq!(settings.inactive_shrapnel_damage, 20_000);
    assert_eq!(settings.shrapnel_speed, 4300);
    assert_eq!(settings.bomb_explode_delay, 25);
    assert_eq!(settings.bomb_explode_pixels, 72);
    assert_eq!(settings.death_prize_time, 1500);
    assert_eq!(settings.enter_delay, 150);
    assert_eq!(settings.engine_shutdown_time, 400);
    assert_eq!(settings.proximity_distance, 3);
    assert_eq!(settings.bounty_increase_for_kill, 2);
    assert_eq!(settings.bounce_factor, 16);
    assert_eq!(settings.map_zoom_factor, 48);
    assert_eq!(settings.repel_time, 50);
    assert_eq!(settings.repel_distance, 160);
    assert_eq!(settings.antiwarp_pixels, 640);
    assert_eq!(settings.door_mode, -1);
    assert_eq!(settings.multiprize_count, 5);
    assert_eq!(settings.brick_time, 1200);
    assert_eq!(
        settings.spawn_regions[0],
        ContinuumSpawnRegion {
            x: 128,
            y: -12,
            radius: 21
        }
    );
    assert_eq!(
        settings.spawn_regions[1],
        ContinuumSpawnRegion {
            x: -128,
            y: 256,
            radius: 0
        }
    );
    assert_eq!(settings.warp_radius_limit, 768);
    assert_eq!(settings.emp_bomb_shutdown_time, 300);
    assert_eq!(settings.emp_bomb_damage_percent, 600);
    assert_eq!(settings.warp_point_delay, 2000);
    assert_eq!(settings.bouncing_bomb_damage_percent, 750);
    assert_eq!(settings.shrapnel_damage_percent, 500);
    assert_eq!(settings.flagger_fire_cost_percent, 800);
    assert_eq!(settings.flagger_damage_percent, 750);
    assert_eq!(settings.flagger_bomb_fire_delay, 35);
    assert_eq!(settings.flagger_thrust_adjustment, -7);
    assert_eq!(settings.flagger_speed_adjustment, -40);
    assert_eq!(settings.prize_factor, 1000);
    assert_eq!(settings.minimum_virtual, 800);
    assert_eq!(settings.upgrade_virtual, 20);
    assert_eq!(settings.prize_negative_factor, 32000);
    assert!(settings.shrapnel_random);
    assert!(settings.ball_bounce);
    assert!(settings.allow_bombs);
    assert!(settings.allow_guns);
    assert_eq!(settings.team_max_mines, 4);
    assert!(settings.gravity_bombs);
    assert!(settings.bomb_safety);
    assert!(settings.take_prize_reliable);
    assert_eq!(settings.prize_hide_count, 3);
    assert_eq!(settings.carry_flags, 2);
    assert_eq!(settings.radar_mode, 4);
    assert!(settings.victory_music);
    assert_eq!(settings.soccer_mode, 3);
    assert!(settings.flagger_gun_upgrade);
    assert!(settings.flagger_bomb_upgrade);
    assert!(settings.use_flagger_for_ball);
    assert!(settings.ball_location);
    assert_eq!(settings.antiwarp_settle_delay, 50);
    assert_eq!(settings.pass_delay, 100);
    assert_eq!(settings.prize_weight_total(), 406);

    assert_eq!(ship.super_time, 1200);
    assert_eq!(ship.shield_time, 600);
    assert_eq!(ship.gravity, 40);
    assert_eq!(ship.gravity_top_speed, 5600);
    assert_eq!(ship.bullet_fire_energy, 30);
    assert_eq!(ship.multifire_energy, 45);
    assert_eq!(ship.bomb_fire_energy, 120);
    assert_eq!(ship.bomb_fire_energy_upgrade, 15);
    assert_eq!(ship.landmine_fire_energy, 130);
    assert_eq!(ship.landmine_fire_energy_upgrade, 20);
    assert_eq!(ship.bullet_speed, 7600);
    assert_eq!(ship.bomb_speed, 4300);
    assert_eq!(ship.see_bomb_level, 2);
    assert!(ship.disable_fast_shooting);
    assert_eq!(ship.radius_pixels, 14.0);
    assert_eq!(ship.multifire_angle, 2220);
    assert_eq!(ship.cloak_status, 2);
    assert_eq!(ship.stealth_status, 1);
    assert_eq!(ship.xradar_status, 2);
    assert_eq!(ship.antiwarp_status, 1);
    assert_eq!(ship.initial_guns, 1);
    assert_eq!(ship.max_guns, 3);
    assert_eq!(ship.initial_bombs, 1);
    assert_eq!(ship.max_bombs, 3);
    assert_eq!(ship.bomb_thrust, 160);
    assert_eq!(ship.turret_thrust_penalty, 8);
    assert_eq!(ship.turret_speed_penalty, 120);
    assert_eq!(ship.damage_factor, 5000);
    assert_eq!(ship.prize_share_limit, 12);
    assert_eq!(ship.attach_bounty, 9);
    assert_eq!(ship.upgrade_energy, 100);
    assert_eq!(ship.soccer_ball_throw_timer, 450);
    assert_eq!(ship.soccer_ball_friction, 1500);
    assert_eq!(ship.soccer_ball_proximity, 28);
    assert_eq!(ship.soccer_ball_speed, 9000);
    assert_eq!(ship.turret_limit, 2);
    assert_eq!(ship.shrapnel_max, 13);
    assert_eq!(ship.shrapnel_rate, 3);
    assert!(ship.double_barrel);
    assert!(ship.emp_bomb);
    assert!(ship.see_mines);

    assert!(ContinuumArenaSettings::parse(&data[..CONTINUUM_ARENA_SETTINGS_SIZE - 1]).is_err());
}

#[test]
fn continuum_arena_settings_convert_to_local_tuning_units() {
    let data = sample_continuum_settings_packet();
    let settings = ContinuumArenaSettings::parse(&data).expect("settings should parse");
    let arena_tuning = ArenaTuning::from_continuum_settings(&settings);
    let tuning = arena_tuning.ship(0);

    assert!(arena_tuning.take_prize_reliable);
    assert_eq!(arena_tuning.door_delay_seconds, 1.2);
    assert_eq!(arena_tuning.door_mode, -1);
    assert_eq!(arena_tuning.spawn_regions, settings.spawn_regions);
    assert_eq!(arena_tuning.radar_mode, 4);
    assert!(arena_tuning.victory_music);
    assert_eq!(arena_tuning.warp_radius_limit, 768);
    assert_eq!(arena_tuning.enter_delay_seconds, 1.5);
    assert_eq!(arena_tuning.enter_game_flagging_delay_seconds, 5.0);
    assert_eq!(arena_tuning.antiwarp_settle_seconds, 0.5);
    assert!(arena_tuning.hide_flags_from_spectators);
    assert!(arena_tuning.no_xradar);
    assert!(arena_tuning.exact_damage);
    assert_eq!(arena_tuning.bounty_increase_for_kill, 2);
    assert!(arena_tuning.allow_bombs);
    assert!(arena_tuning.allow_guns);
    assert!(arena_tuning.gravity_bombs);
    assert_eq!(arena_tuning.team_max_mines, 4);
    assert!(arena_tuning.bomb_safety);
    assert_eq!(arena_tuning.bounce_factor, 1.0);
    assert!(arena_tuning.disable_ball_through_walls);
    assert_eq!(arena_tuning.carry_flags, 2);
    assert_eq!(arena_tuning.flag_drop_delay_seconds, 120.0);
    assert_eq!(arena_tuning.flagger_thrust_adjustment, -70.0);
    assert_eq!(arena_tuning.flagger_speed_adjustment, -4.0);
    assert_eq!(arena_tuning.flagger_fire_cost_multiplier, 0.8);
    assert_eq!(arena_tuning.flagger_damage_multiplier, 0.75);
    assert_eq!(arena_tuning.flagger_bomb_fire_delay_seconds, 0.35);
    assert!(arena_tuning.flagger_gun_upgrade);
    assert!(arena_tuning.flagger_bomb_upgrade);
    assert!(arena_tuning.use_flagger_for_ball);
    assert_eq!(arena_tuning.soccer_mode, 3);
    assert_eq!(arena_tuning.map_zoom_factor, 48);
    assert_eq!(arena_tuning.max_frequency, 1000);
    assert!(arena_tuning.ball_location);
    assert_eq!(arena_tuning.pass_delay_seconds, 1.0);
    assert_eq!(arena_tuning.prize_factor, 1000);
    assert_eq!(arena_tuning.multiprize_count, 5);
    assert_eq!(arena_tuning.prize_spawn_interval, 2.0);
    assert_eq!(arena_tuning.prize_spawn_count, 3);
    assert_eq!(arena_tuning.prize_minimum_virtual, 800);
    assert_eq!(arena_tuning.prize_upgrade_virtual, 20);
    assert_eq!(arena_tuning.prize_lifetime_min, 10.0);
    assert_eq!(arena_tuning.prize_lifetime_max, 30.0);
    assert_eq!(arena_tuning.death_prize_lifetime, 15.0);
    assert_eq!(arena_tuning.bullet_lifetime, 1.25);
    assert_eq!(arena_tuning.bomb_lifetime, 2.4);
    assert_eq!(arena_tuning.bomb_explode_delay_seconds, 0.25);
    assert_eq!(arena_tuning.mine_lifetime, 45.0);
    assert_eq!(arena_tuning.decoy_lifetime, 12.0);
    assert_eq!(arena_tuning.brick_lifetime, 12.0);
    assert_eq!(arena_tuning.portal_lifetime, 20.0);
    assert_eq!(arena_tuning.antiwarp_radius, 640.0);
    assert_eq!(arena_tuning.proximity_distance, 3.0);
    assert_eq!(arena_tuning.bullet_damage, 90.0);
    assert_eq!(arena_tuning.bullet_damage_upgrade, 10.0);
    assert_eq!(arena_tuning.bomb_damage, 260.0);
    assert_eq!(arena_tuning.bouncing_bomb_damage_multiplier, 0.75);
    assert_eq!(arena_tuning.emp_bomb_damage_multiplier, 0.6);
    assert_eq!(arena_tuning.emp_bomb_shutdown_seconds, 3.0);
    assert_eq!(arena_tuning.bomb_explode_pixels, 72.0);
    assert_eq!(arena_tuning.burst_damage, 70.0);
    assert_eq!(arena_tuning.repel_lifetime, 0.5);
    assert_eq!(arena_tuning.repel_radius, 160.0);
    assert_eq!(arena_tuning.repel_speed, 520.0);
    assert!(arena_tuning.shrapnel_random);
    assert_eq!(arena_tuning.inactive_shrapnel_damage, 20.0);
    assert_eq!(arena_tuning.shrapnel_damage_multiplier, 0.5);
    assert_eq!(arena_tuning.shrapnel_speed, 430.0);
    assert_eq!(arena_tuning.prize_negative_factor, 32000);
    assert_eq!(arena_tuning.prize_weight_total, 406);
    assert_eq!(arena_tuning.prize_weights, settings.prize_weights);
    assert_eq!(tuning.radius_pixels, 14.0);
    assert_eq!(tuning.attach_bounty, 9);
    assert_eq!(tuning.soccer_ball_throw_timer, 4.5);
    assert_eq!(tuning.soccer_ball_friction, 0.985);
    assert_eq!(tuning.soccer_ball_proximity, 28.0);
    assert_eq!(tuning.soccer_ball_speed, 900.0);
    assert_eq!(tuning.turret_limit, 2);
    assert_eq!(tuning.see_bomb_level, 2);
    assert!(tuning.see_mines);
    assert!(tuning.emp_bomb);
    assert_eq!(tuning.energy_initial, 1200.0);
    assert_eq!(tuning.energy_max, 1500.0);
    assert_eq!(tuning.energy_recharge, 120.0);
    assert_eq!(tuning.recharge_upgrade_step, 10.0);
    assert_eq!(tuning.energy_recharge_max, 180.0);
    assert_eq!(tuning.energy_upgrade_step, 100.0);
    assert_eq!(tuning.afterburner_energy_per_second, 180.0);
    assert_eq!(tuning.stealth_energy_per_second, 45.0);
    assert_eq!(tuning.cloak_energy_per_second, 70.0);
    assert_eq!(tuning.xradar_energy_per_second, 40.0);
    assert_eq!(tuning.antiwarp_energy_per_second, 55.0);
    assert_eq!(tuning.wormhole_gravity, 40);
    assert_eq!(tuning.wormhole_top_speed, 560.0);
    assert_eq!(tuning.rotation_per_second, 0.5);
    assert_eq!(tuning.rotation_upgrade_step, 0.1);
    assert_eq!(tuning.rotation_max_per_second, 1.0);
    assert_eq!(tuning.thrust_acceleration, 360.0);
    assert_eq!(tuning.thrust_upgrade_step, 40.0);
    assert_eq!(tuning.thrust_max_acceleration, 520.0);
    assert_eq!(tuning.afterburner_thrust_acceleration, 520.0);
    assert_eq!(tuning.top_speed, 320.0);
    assert_eq!(tuning.top_speed_upgrade_step, 40.0);
    assert_eq!(tuning.top_speed_max, 520.0);
    assert_eq!(tuning.afterburner_top_speed, 520.0);
    assert_eq!(tuning.rocket_thrust_acceleration, 920.0);
    assert_eq!(tuning.rocket_top_speed, 700.0);
    assert_eq!(tuning.rocket_lifetime, 2.5);
    assert_eq!(tuning.rocket_cooldown, 0.0);
    assert_eq!(tuning.bullet_energy, 30.0);
    assert_eq!(tuning.bullet_speed, 760.0);
    assert_eq!(tuning.bullet_cooldown, 0.08);
    assert_eq!(tuning.multifire_energy, 45.0);
    assert_eq!(tuning.multifire_cooldown, 0.12);
    assert!((tuning.multifire_spread - 2220.0 / (111.0 * 360.0)).abs() < 0.0001);
    assert!(tuning.double_barrel);
    assert_eq!(tuning.burst_energy, 0.0);
    assert_eq!(tuning.burst_speed, 620.0);
    assert_eq!(tuning.burst_bullet_count, 12);
    assert_eq!(tuning.burst_cooldown, 0.65);
    assert_eq!(tuning.bomb_energy, 120.0);
    assert_eq!(tuning.bomb_energy_upgrade, 15.0);
    assert_eq!(tuning.mine_energy, 130.0);
    assert_eq!(tuning.mine_energy_upgrade, 20.0);
    assert_eq!(tuning.bomb_speed, 430.0);
    assert_eq!(tuning.bomb_recoil_speed, 16.0);
    assert_eq!(tuning.bomb_cooldown, 0.65);
    assert_eq!(tuning.bomb_bounces, 2);
    assert!(tuning.disable_fast_shooting);
    assert_eq!(tuning.turret_thrust_penalty, 80.0);
    assert_eq!(tuning.turret_speed_penalty, 12.0);
    assert_eq!(tuning.max_mines, 8);
    assert_eq!(tuning.repel_energy, 0.0);
    assert_eq!(tuning.repel_cooldown, 0.5);
    assert_eq!(tuning.initial_repels, 1);
    assert_eq!(tuning.max_repels, 5);
    assert_eq!(tuning.brick_cooldown, 0.65);
    assert_eq!(tuning.decoy_cooldown, 0.65);
    assert_eq!(tuning.initial_thors, 1);
    assert_eq!(tuning.max_thors, 2);
    assert_eq!(tuning.thor_speed, 430.0);
    assert_eq!(tuning.thor_cooldown, 0.65);
    // Arena settings capability fields store 0..=3 directly, unlike
    // per-shot weapon_data where the level bits mean level - 1.
    assert_eq!(tuning.initial_gun_level, 1);
    assert_eq!(tuning.max_gun_level, 3);
    assert_eq!(tuning.initial_bomb_level, 1);
    assert_eq!(tuning.max_bomb_level, 3);
    assert!(!tuning.initial_stealth);
    assert!(tuning.initial_cloak);
    assert!(tuning.initial_xradar);
    assert!(!tuning.initial_antiwarp);
    assert!(tuning.stealth_available);
    assert!(tuning.cloak_available);
    assert!(tuning.xradar_available);
    assert!(tuning.antiwarp_available);
    assert_eq!(tuning.shrapnel_max, 13);
    assert_eq!(tuning.shrapnel_rate, 3);
    assert_eq!(tuning.initial_bounty, 7);
    assert_eq!(tuning.damage_factor, 5000);
    assert_eq!(tuning.prize_share_limit, 12);
    assert_eq!(tuning.super_lifetime, 12.0);
    assert_eq!(tuning.shield_lifetime, 6.0);
    assert_eq!(tuning.engine_shutdown_lifetime, 4.0);
}

#[test]
fn subgame_settings_text_maps_to_arena_tuning() {
    let tuning = arena_tuning_from_subgame_settings_text(
        r#"
        #include conf/svs/ship-warbird

        [AllShips]
        Radius=12
        DisableFastShooting=1

        [Warbird]
        MaximumRotation=290
        MaximumThrust=19
        MaximumSpeed=3250
        MaximumRecharge=1150
        MaximumEnergy=1700
        InitialRotation=200
        InitialThrust=15
        InitialSpeed=2010
        InitialRecharge=400
        InitialEnergy=1000
        UpgradeRotation=40
        UpgradeThrust=2
        UpgradeSpeed=250
        UpgradeRecharge=166
        UpgradeEnergy=100
        BulletFireEnergy=20
        BulletSpeed=2000
        BulletFireDelay=24
        MultiFireEnergy=30
        MultiFireDelay=50
        MultiFireAngle=500
        BombFireEnergy=300
        BombFireEnergyUpgrade=50
        LandmineFireEnergy=270
        LandmineFireEnergyUpgrade=150
        BombSpeed=2000
        BombThrust=400
        BombFireDelay=150
        BurstSpeed=3000
        BurstShrapnel=24
        TurretThrustPenalty=1
        TurretSpeedPenalty=125
        TurretLimit=5
        RocketTime=1000
        InitialBounty=500
        MaxMines=5
        RepelMax=3
        BurstMax=3
        DecoyMax=3
        ThorMax=3
        BrickMax=3
        PortalMax=3
        RocketMax=3
        InitialRepel=1
        InitialBurst=2
        InitialBrick=3
        InitialRocket=1
        InitialThor=2
        InitialDecoy=3
        InitialPortal=1
        BombBounceCount=0
        CloakStatus=0
        StealthStatus=1
        XRadarStatus=2
        AntiWarpStatus=1
        DamageFactor=30
        PrizeShareLimit=100
        AttachBounty=7
        ShrapnelMax=8
        ShrapnelRate=2
        MaxGuns=3
        MaxBombs=2
        InitialGuns=0
        InitialBombs=0
        EmpBomb=1
        SeeMines=1
        SeeBombLevel=2
        DoubleBarrel=1
        SoccerBallFriction=12
        SoccerBallProximity=64
        SoccerThrowTime=1200
        SoccerBallSpeed=2500

        [Bomb]
        BombDamageLevel=750000
        BombAliveTime=8000
        BombExplodeDelay=150
        BombExplodePixels=80
        ProximityDistance=3
        BombSafety=1
        EBombShutdownTime=400
        EBombDamagePercent=1000
        BBombDamagePercent=900

        [Bullet]
        BulletDamageLevel=200000
        BulletAliveTime=550
        BulletDamageUpgrade=100000

        [Prize]
        MultiPrizeCount=10
        PrizeFactor=10000
        PrizeDelay=700
        PrizeHideCount=10
        MinimumVirtual=10
        UpgradeVirtual=6
        PrizeMaxExist=8000
        PrizeMinExist=4000
        PrizeNegativeFactor=300
        DeathPrizeTime=800
        EngineShutdownTime=700
        TakePrizeReliable=1

        [Radar]
        RadarMode=4
        MapZoomFactor=48

        [Flag]
        FlaggerOnRadar=1

        [Wormhole]
        GravityBombs=1

        [Misc]
        VictoryMusic=1
        BounceFactor=22
        WarpRadiusLimit=20
        MaxFrequency=1234
        AllowBombs=0
        AllowGuns=0

        [PrizeWeight]
        QuickCharge=40
        Brick=200
        MultiPrize=130
        "#,
    )
    .expect("settings should parse");

    let warbird = tuning.ship(0);
    assert_eq!(warbird.radius_pixels, 12.0);
    assert_eq!(warbird.rotation_max_per_second, 290.0 / 400.0);
    assert_eq!(warbird.thrust_max_acceleration, 190.0);
    assert_eq!(warbird.afterburner_thrust_acceleration, 190.0);
    assert_eq!(warbird.top_speed_max, 325.0);
    assert_eq!(warbird.afterburner_top_speed, 325.0);
    assert_eq!(warbird.energy_recharge_max, 115.0);
    assert_eq!(warbird.energy_max, 1700.0);
    assert_eq!(warbird.rotation_per_second, 0.5);
    assert_eq!(warbird.thrust_acceleration, 150.0);
    assert_eq!(warbird.top_speed, 201.0);
    assert_eq!(warbird.energy_recharge, 40.0);
    assert_eq!(warbird.energy_initial, 1000.0);
    assert_eq!(warbird.rotation_upgrade_step, 0.1);
    assert_eq!(warbird.thrust_upgrade_step, 20.0);
    assert_eq!(warbird.top_speed_upgrade_step, 25.0);
    assert_eq!(warbird.recharge_upgrade_step, 16.6);
    assert_eq!(warbird.energy_upgrade_step, 100.0);
    assert_eq!(warbird.bullet_energy, 20.0);
    assert_eq!(warbird.bullet_speed, 200.0);
    assert_eq!(warbird.bullet_cooldown, 0.24);
    assert_eq!(warbird.multifire_energy, 30.0);
    assert_eq!(warbird.multifire_cooldown, 0.5);
    assert!((warbird.multifire_spread - 500.0 / (111.0 * 360.0)).abs() < 0.0001);
    assert_eq!(warbird.bomb_energy, 300.0);
    assert_eq!(warbird.bomb_energy_upgrade, 50.0);
    assert_eq!(warbird.mine_energy, 270.0);
    assert_eq!(warbird.mine_energy_upgrade, 150.0);
    assert_eq!(warbird.bomb_speed, 200.0);
    assert_eq!(warbird.thor_speed, 200.0);
    assert_eq!(warbird.bomb_recoil_speed, 40.0);
    assert_eq!(warbird.bomb_cooldown, 1.5);
    assert!(warbird.disable_fast_shooting);
    assert_eq!(warbird.burst_speed, 300.0);
    assert_eq!(warbird.burst_bullet_count, 24);
    assert_eq!(warbird.turret_thrust_penalty, 10.0);
    assert_eq!(warbird.turret_speed_penalty, 12.5);
    assert_eq!(warbird.turret_limit, 5);
    assert_eq!(warbird.rocket_lifetime, 10.0);
    assert_eq!(warbird.initial_bounty, 500);
    assert_eq!(warbird.max_mines, 5);
    assert_eq!(warbird.max_repels, 3);
    assert_eq!(warbird.initial_repels, 1);
    assert_eq!(warbird.initial_portals, 1);
    assert!(!warbird.cloak_available);
    assert!(warbird.stealth_available);
    assert!(!warbird.initial_stealth);
    assert!(warbird.xradar_available);
    assert!(warbird.initial_xradar);
    assert_eq!(warbird.damage_factor, 30);
    assert_eq!(warbird.prize_share_limit, 100);
    assert_eq!(warbird.attach_bounty, 7);
    assert_eq!(warbird.shrapnel_max, 8);
    assert_eq!(warbird.shrapnel_rate, 2);
    assert_eq!(warbird.max_gun_level, 3);
    assert_eq!(warbird.max_bomb_level, 2);
    assert_eq!(warbird.initial_gun_level, 0);
    assert_eq!(warbird.initial_bomb_level, 0);
    assert!(warbird.emp_bomb);
    assert!(warbird.see_mines);
    assert_eq!(warbird.see_bomb_level, 2);
    assert!(warbird.double_barrel);
    assert_eq!(warbird.soccer_ball_proximity, 64.0);
    assert_eq!(warbird.soccer_ball_throw_timer, 12.0);
    assert_eq!(warbird.soccer_ball_speed, 250.0);

    assert_eq!(tuning.bomb_damage, 750.0);
    assert_eq!(tuning.bullet_damage, 200.0);
    assert_eq!(tuning.bullet_lifetime, 5.5);
    assert_eq!(tuning.bullet_damage_upgrade, 100.0);
    assert_eq!(tuning.bomb_lifetime, 80.0);
    assert_eq!(tuning.bomb_explode_delay_seconds, 1.5);
    assert_eq!(tuning.bomb_explode_pixels, 80.0);
    assert_eq!(tuning.proximity_distance, 3.0);
    assert!(tuning.bomb_safety);
    assert_eq!(tuning.emp_bomb_shutdown_seconds, 4.0);
    assert_eq!(tuning.emp_bomb_damage_multiplier, 1.0);
    assert_eq!(tuning.bouncing_bomb_damage_multiplier, 0.9);
    assert_eq!(tuning.multiprize_count, 10);
    assert_eq!(tuning.prize_factor, 10000);
    assert_eq!(tuning.prize_spawn_interval, 7.0);
    assert_eq!(tuning.prize_spawn_count, 10);
    assert_eq!(tuning.prize_minimum_virtual, 10);
    assert_eq!(tuning.prize_upgrade_virtual, 6);
    assert_eq!(tuning.prize_lifetime_max, 80.0);
    assert_eq!(tuning.prize_lifetime_min, 40.0);
    assert_eq!(tuning.prize_negative_factor, 300);
    assert_eq!(tuning.death_prize_lifetime, 8.0);
    assert_eq!(warbird.engine_shutdown_lifetime, 7.0);
    assert!(tuning.take_prize_reliable);
    assert_eq!(tuning.radar_mode, 4);
    assert_eq!(tuning.map_zoom_factor, 48);
    assert!(tuning.flagger_on_radar);
    assert!(tuning.gravity_bombs);
    assert!(tuning.victory_music);
    assert_eq!(tuning.bounce_factor, continuum_bounce_factor(22));
    assert_eq!(tuning.warp_radius_limit, 20);
    assert_eq!(tuning.max_frequency, 1234);
    assert!(!tuning.allow_bombs);
    assert!(!tuning.allow_guns);
    assert_eq!(tuning.prize_weights[0], 40);
    assert_eq!(tuning.prize_weights[25], 200);
    assert_eq!(tuning.prize_weights[24], 130);
    assert_eq!(tuning.prize_weight_total, 370);
}

#[test]
fn chaos_zone_asset_settings_parse_to_expected_local_tuning() {
    let tuning = arena_tuning_from_subgame_settings_text(include_str!(
        "../../assets/zones/chaos-zone/arena.conf"
    ))
    .expect("chaos-zone settings should parse");

    let warbird = tuning.ship(0);
    let javelin = tuning.ship(1);
    let spider = tuning.ship(2);
    let leviathan = tuning.ship(3);
    let terrier = tuning.ship(4);
    let weasel = tuning.ship(5);
    let lancaster = tuning.ship(6);
    let shark = tuning.ship(7);

    assert_eq!(warbird.rotation_max_per_second, 290.0 / 400.0);
    assert_eq!(warbird.initial_gun_level, 3);
    assert_eq!(warbird.max_gun_level, 3);
    assert_eq!(warbird.initial_bomb_level, 0);
    assert_eq!(warbird.max_bomb_level, 2);
    assert_eq!(javelin.thrust_max_acceleration, 170.0);
    assert_eq!(javelin.afterburner_top_speed, 375.0);
    assert!(spider.initial_cloak);
    assert_eq!(leviathan.max_bomb_level, 3);
    assert!(terrier.double_barrel);
    assert_eq!(terrier.bullet_cooldown, 0.25);
    assert!(weasel.emp_bomb);
    assert!(weasel.see_mines);
    assert_eq!(weasel.shrapnel_max, 0);
    assert_eq!(lancaster.bomb_bounces, 1);
    assert_eq!(lancaster.max_bricks, 100);
    assert!(shark.initial_cloak);
    assert!(shark.cloak_available);
    assert_eq!(
        shark.cloak_energy_per_second,
        spider.cloak_energy_per_second
    );
    assert_eq!(shark.cloak_energy_per_second, 10.0);
    assert_eq!(shark.initial_bounty, 30);
    assert_eq!(shark.wormhole_gravity, 0);
    assert_eq!(shark.see_bomb_level, 1);
    assert_eq!(shark.bullet_cooldown, 0.16);

    assert_eq!(warbird.radius_pixels, 14.0);
    assert_eq!(warbird.energy_initial, 1700.0);
    assert_eq!(warbird.energy_max, 1700.0);
    assert_eq!(warbird.bullet_speed, 200.0);
    assert_eq!(warbird.bomb_speed, 200.0);
    assert_eq!(warbird.burst_speed, 300.0);
    assert_eq!(warbird.burst_bullet_count, 24);
    assert_eq!(warbird.rocket_thrust_acceleration, 250.0);
    assert_eq!(warbird.rocket_top_speed, 550.0);
    assert_eq!(warbird.max_repels, 3);
    assert_eq!(warbird.initial_repels, 0);

    assert_eq!(tuning.bullet_damage, 200.0);
    assert_eq!(tuning.bullet_lifetime, 5.5);
    assert_eq!(tuning.bullet_damage_upgrade, 100.0);
    assert_eq!(tuning.bomb_damage, 750.0);
    assert_eq!(tuning.bomb_lifetime, 80.0);
    assert_eq!(tuning.bomb_explode_pixels, 80.0);
    assert_eq!(tuning.bouncing_bomb_damage_multiplier, 1.0);
    assert_eq!(tuning.emp_bomb_damage_multiplier, 1.0);
    assert_eq!(tuning.burst_damage, 700.0);
    assert_eq!(tuning.inactive_shrapnel_damage, 3.0);
    assert_eq!(tuning.shrapnel_damage_multiplier, 1.0);
    assert_eq!(tuning.repel_speed, 500.0);
    assert_eq!(tuning.repel_radius, 512.0);
    assert_eq!(tuning.shrapnel_speed, 300.0);
    assert_eq!(warbird.engine_shutdown_lifetime, 7.0);
    assert_eq!(tuning.map_zoom_factor, 48);
    assert_eq!(tuning.max_frequency, 10_000);
    assert_eq!(tuning.prize_weight_total, 1147);
    assert!(tuning.allow_bombs);
    assert!(tuning.allow_guns);
    assert!(tuning.gravity_bombs);
}

#[test]
fn upstream_eg_assets_parse_to_expected_local_tuning() {
    let extreme = arena_tuning_from_subgame_settings_text(include_str!(
        "../../assets/zones/extreme/arena.conf"
    ))
    .expect("extreme settings should parse");

    assert_eq!(extreme.bullet_damage, 210.0);
    assert_eq!(extreme.bomb_damage, 5_600.0);
    assert_eq!(extreme.max_frequency, 10_000);
    assert_eq!(extreme.ship(0).bullet_speed, 410.0);
    assert_eq!(extreme.ship(0).top_speed, 320.0);
    assert_eq!(extreme.ship(0).energy_max, 2_800.0);
    assert!(
        extreme.ships.iter().all(|ship| ship.radius_pixels == 14.0),
        "upstream EG Radius=0 uses Continuum's 14-pixel default"
    );
    assert_eq!(extreme.prize_weights[5], 23, "XRadar protocol id");
    assert_eq!(extreme.prize_weights[19], 10, "AntiWarp protocol id");
    assert_eq!(extreme.prize_weights[24], 5, "MultiPrize protocol id");
    assert_eq!(extreme.prize_weights[27], 0, "Portal protocol id");
}

#[test]
fn repel_reuse_delay_stays_reference_constant_not_effect_lifetime() {
    let mut data = sample_continuum_settings_packet();
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 30, 250);

    let settings = ContinuumArenaSettings::parse(&data).expect("settings should parse");
    let arena_tuning = ArenaTuning::from_continuum_settings(&settings);
    let ship_tuning = arena_tuning.ship(0);

    assert_eq!(arena_tuning.repel_lifetime, 2.5);
    assert_eq!(ship_tuning.repel_cooldown, REPEL_REUSE_COOLDOWN);
}

#[test]
fn parsed_special_weapon_reuse_delays_follow_bomb_fire_delay() {
    let mut data = sample_continuum_settings_packet();
    put_u16(&mut data, CONTINUUM_SHIP_SETTINGS_OFFSET + 84, 90);

    let settings = ContinuumArenaSettings::parse(&data).expect("settings should parse");
    let arena_tuning = ArenaTuning::from_continuum_settings(&settings);
    let ship_tuning = arena_tuning.ship(0);

    assert_eq!(ship_tuning.bomb_cooldown, 0.9);
    assert_eq!(ship_tuning.burst_cooldown, 0.9);
    assert_eq!(ship_tuning.brick_cooldown, 0.9);
    assert_eq!(ship_tuning.decoy_cooldown, 0.9);
    assert_eq!(ship_tuning.thor_cooldown, 0.9);
}

#[test]
fn parsed_burst_shrapnel_count_allows_zero_like_reference_game() {
    let mut data = sample_continuum_settings_packet();
    data[CONTINUUM_SHIP_SETTINGS_OFFSET + 107] = 0;

    let settings = ContinuumArenaSettings::parse(&data).expect("settings should parse");
    let arena_tuning = ArenaTuning::from_continuum_settings(&settings);

    assert_eq!(arena_tuning.ship(0).burst_bullet_count, 0);
}

#[test]
fn continuum_settings_checksum_matches_reference_game_word_sum() {
    let mut data = vec![0; CONTINUUM_ARENA_SETTINGS_SIZE];
    for (index, byte) in data.iter_mut().enumerate() {
        *byte = ((index * 13 + 7) & 0xff) as u8;
    }
    let key = 0x1357_9bdf;
    let expected = data
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) ^ key)
        .fold(0_u32, u32::wrapping_add);

    assert_eq!(
        continuum_settings_checksum(key, &data).expect("checksum should parse"),
        expected
    );
    assert!(continuum_settings_checksum(key, &data[..data.len() - 1]).is_err());
}

#[test]
fn continuum_map_crc32_filters_tiles_like_reference_game() {
    let tiles = [1, 0, 0xa1, 0xab, 0xff, SAFE_TILE_ID, 160, 161];

    assert_eq!(continuum_map_crc32(&tiles), 0x280f_33cd);
    assert_ne!(
        continuum_map_crc32(&tiles),
        continuum_map_crc32(&[1, 0xab, 160])
    );
    assert_eq!(continuum_map_crc32(&[0, 0xa1, 0xff, 161]), 0xffff_ffff);
}

#[test]
fn map_security_checksum_matches_reference_game_sparse_tile_sum() {
    let width = MAP_TILES as usize;
    let height = MAP_TILES as usize;
    let mut tiles = vec![0; width * height];
    tiles[31 * width + 1] = 7;
    tiles[31 * width + 32] = SAFE_TILE_ID;
    tiles[31 * width + 63] = BRICK_TILE_ID;
    tiles[31 * width + 94] = 161;
    tiles[63 * width + 1] = FLAG_TILE_ID;
    tiles[63 * width + 32] = 160;
    let map = ArenaMap {
        name: "checksum".to_string(),
        width,
        height,
        tiles: tiles.into(),
        objects: Vec::new(),
        doors: Vec::new(),
        radar_points: Vec::new(),
        radar_index: std::sync::OnceLock::new(),
        embedded_tileset_pixel_bytes: 0,
        door_seed: u8::MAX,
        door_rng_seed: 1,
    };

    assert_eq!(map.security_checksum(63), 426);
}

#[test]
fn security_packet_encodes_reference_game_vie_layout() {
    let packet = SecurityPacket::vie(
        0x0102_0304,
        0x1122_3344,
        0x5566_7788,
        0x99aa_bbcc,
        SecurityPacketStats {
            s2c_slow_total: 0x0a0b_0c0d,
            s2c_fast_total: 0x0102_0305,
            s2c_slow_current: 0x1234,
            s2c_fast_current: 0x5678,
            s2c_reliable_out: 0x9abc,
            ping_current: 11,
            ping_average: 22,
            ping_low: 33,
            ping_high: 44,
            slow_frame: 7,
            timer_drift: 0xbeef,
        },
    );

    assert_eq!(
        packet.encode(),
        vec![
            0x1a, 0x04, 0x03, 0x02, 0x01, 0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55, 0xcc,
            0xbb, 0xaa, 0x99, 0x0d, 0x0c, 0x0b, 0x0a, 0x05, 0x03, 0x02, 0x01, 0x34, 0x12, 0x78,
            0x56, 0xbc, 0x9a, 0x0b, 0x00, 0x16, 0x00, 0x21, 0x00, 0x2c, 0x00, 0x07,
        ]
    );
}

#[test]
fn security_packet_encodes_continuum_timer_drift_and_map_crc() {
    let packet = SecurityPacket::continuum(
        1,
        2,
        3,
        4,
        SecurityPacketStats {
            timer_drift: 0xbeef,
            ..SecurityPacketStats::default()
        },
        0x1234_5678,
    );
    let encoded = packet.encode();

    assert_eq!(encoded.len(), 46);
    assert_eq!(
        &encoded[0..17],
        &[0x1a, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0]
    );
    assert_eq!(&encoded[40..], &[0xef, 0xbe, 0x78, 0x56, 0x34, 0x12]);
}

#[test]
fn security_challenge_parses_reference_game_s2c_layout() {
    let mut packet = vec![0x18];
    packet.extend_from_slice(&0x0102_0304_u32.to_le_bytes());
    packet.extend_from_slice(&0x1112_1314_u32.to_le_bytes());
    packet.extend_from_slice(&0x2122_2324_u32.to_le_bytes());
    packet.extend_from_slice(&0x3132_3334_u32.to_le_bytes());

    let challenge = SecurityChallenge::parse_packet(&packet).expect("challenge should parse");
    assert_eq!(
        challenge,
        SecurityChallenge {
            prize_seed: 0x0102_0304,
            door_seed: 0x1112_1314,
            timestamp: 0x2122_2324,
            checksum_key: 0x3132_3334,
        }
    );
    assert!(challenge.should_send_security(0xfeed_beef));
    assert!(!challenge.should_send_security(0));
}

#[test]
fn security_challenge_rejects_wrong_type_or_short_packet() {
    let mut packet = vec![0x18; SecurityChallenge::PACKET_LEN];

    assert!(SecurityChallenge::parse_packet(&packet[..SecurityChallenge::PACKET_LEN - 1]).is_err());
    packet[0] = 0x17;
    assert!(SecurityChallenge::parse_packet(&packet).is_err());

    let no_key = SecurityChallenge {
        checksum_key: 0,
        ..SecurityChallenge::default()
    };
    assert!(!no_key.should_send_security(1));
}

#[test]
fn map_information_parses_vie_payload_and_queues_map_request() {
    let mut payload = vec![0; 20];
    payload[0..9].copy_from_slice(b"arena.lvl");
    put_u32(&mut payload, 16, 0xaabb_ccdd);

    let info = MapInformation::parse_payload(&payload, false).expect("map info should parse");
    assert_eq!(info.filename, "arena.lvl");
    assert_eq!(info.checksum, 0xaabb_ccdd);
    assert_eq!(info.compressed_size, 0);
    assert!(info.lvz_files.is_empty());

    let request = info.file_request();
    assert_eq!(
        request,
        FileRequestSpec {
            zone_id: None,
            filename: "arena.lvl".to_string(),
            index: 0,
            size: 0,
            size_is_known: false,
            checksum: 0xaabb_ccdd,
            sha256: None,
            decompress: true,
        }
    );
    assert_eq!(request.encode_request_packet(), vec![0x0c, 0x00, 0x00]);
}

#[test]
fn map_information_parses_continuum_compressed_size() {
    let mut payload = vec![0; 24];
    payload[0..15].copy_from_slice(b"long-map-nm.lvl");
    put_u32(&mut payload, 16, 0x0102_0304);
    put_u32(&mut payload, 20, 0x0006_0708);

    let info = MapInformation::parse_payload(&payload, true).expect("map info should parse");
    assert_eq!(info.filename, "long-map-nm.lvl");
    assert_eq!(info.checksum, 0x0102_0304);
    assert_eq!(info.compressed_size, 0x0006_0708);
    assert!(info.lvz_files.is_empty());
    assert_eq!(
        info.file_request().encode_request_packet(),
        vec![0x0c, 0, 0]
    );

    assert!(MapInformation::parse_payload(&payload[..23], true).is_err());
    assert!(MapInformation::parse_payload(&payload[..19], false).is_err());
}

#[test]
fn map_information_parses_lvz_file_requests() {
    let mut payload = vec![0; 24 + 24 * 2];
    payload[0..9].copy_from_slice(b"arena.lvl");
    put_u32(&mut payload, 16, 0x1111_2222);
    put_u32(&mut payload, 20, 333);
    payload[24..32].copy_from_slice(b"zone.lvz");
    put_u32(&mut payload, 40, 0x3333_4444);
    put_u32(&mut payload, 44, 555);
    payload[48..57].copy_from_slice(b"decor.lvz");
    put_u32(&mut payload, 64, 0x5555_6666);
    put_u32(&mut payload, 68, 777);

    let info = MapInformation::parse_payload(&payload, true).expect("map info should parse");

    assert_eq!(
        info.file_requests(),
        vec![
            FileRequestSpec {
                zone_id: None,
                filename: "arena.lvl".to_string(),
                index: 0,
                size: 333,
                size_is_known: true,
                checksum: 0x1111_2222,
                sha256: None,
                decompress: true,
            },
            FileRequestSpec {
                zone_id: None,
                filename: "zone.lvz".to_string(),
                index: 1,
                size: 555,
                size_is_known: true,
                checksum: 0x3333_4444,
                sha256: None,
                decompress: false,
            },
            FileRequestSpec {
                zone_id: None,
                filename: "decor.lvz".to_string(),
                index: 2,
                size: 777,
                size_is_known: true,
                checksum: 0x5555_6666,
                sha256: None,
                decompress: false,
            },
        ]
    );
    assert!(MapInformation::parse_payload(&payload[..payload.len() - 1], true).is_err());
}

#[test]
fn arena_login_request_encodes_reference_game_payload_layout() {
    let request = ArenaLoginRequest::new(2, 1, 1366, 768, 7, "pub", true);

    let payload = request.encode_payload();
    // The reference game's layout, then this client's protocol revision.
    assert_eq!(
        payload[..27],
        [
            0x01, 0x02, 0x01, 0x00, 0x56, 0x05, 0x00, 0x03, 0x07, 0x00, b'p', b'u', b'b', 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
        ]
    );
    assert_eq!(payload[27..], [ArenaLoginRequest::PROTOCOL_REVISION]);
}

#[test]
fn arena_login_request_truncates_fixed_arena_name_and_wraps_reliably() {
    let request = ArenaLoginRequest::new(
        8,
        0,
        1920,
        1080,
        0xffff,
        "sixteen-byte-name-plus-extra",
        false,
    );
    let payload = request.encode_payload();
    let mut sequencer = ReliableSequencer::default();

    assert_eq!(&payload[10..26], b"sixteen-byte-nam");
    assert_eq!(payload[26], 0);
    assert_eq!(
        sequencer.send_reliable(&payload),
        [&[0x00, 0x03, 0, 0, 0, 0][..], &payload[..],].concat()
    );
}

#[test]
fn arena_login_spectator_default_matches_reference_game_post_password_request() {
    let payload = ArenaLoginRequest::spectator_default().encode_payload();

    assert_eq!(payload.len(), 28);
    assert_eq!(
        &payload[0..10],
        &[0x01, 8, 0, 0, 0x80, 0x07, 0x38, 0x04, 0xff, 0xff]
    );
    assert_eq!(&payload[10..26], &[0; 16]);
    assert_eq!(payload[26], 1);
    // Appended after the 27-byte reference-game request; servers that predate it
    // ignore the extra byte.
    assert_eq!(payload[27], ArenaLoginRequest::PROTOCOL_REVISION);
}

#[test]
fn time_sync_packets_encode_core_layout() {
    assert_eq!(
        TimeSyncRequest {
            nonce: 0x1122_3344,
            total_sent: 0x5566_7788,
            total_received: 0x99aa_bbcc,
        }
        .encode(),
        vec![
            0x00, 0x05, 0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55, 0xcc, 0xbb, 0xaa, 0x99,
        ]
    );

    let response = TimeSyncResponse::parse_request(&[0x00, 0x05, 0x78, 0x56, 0x34, 0x12])
        .expect("time sync request should parse");
    assert_eq!(
        response,
        TimeSyncResponse {
            received_timestamp: 0x1234_5678,
            local_timestamp: 0,
            ..TimeSyncResponse::default()
        }
    );
    assert_eq!(
        response.encode(),
        vec![0x00, 0x06, 0x78, 0x56, 0x34, 0x12, 0, 0, 0, 0]
    );
    assert_eq!(
        TimeSyncResponse::response_to_request(&[0x00, 0x05, 0x78, 0x56, 0x34, 0x12], 0x99aa_bbcc,)
            .expect("time sync response should build")
            .encode(),
        vec![0x00, 0x06, 0x78, 0x56, 0x34, 0x12, 0xcc, 0xbb, 0xaa, 0x99]
    );
    assert_eq!(
        TimeSyncResponse::parse_response(&[
            0x00, 0x06, 0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55,
        ])
        .expect("time sync response should parse"),
        TimeSyncResponse {
            received_timestamp: 0x1122_3344,
            local_timestamp: 0x5566_7788,
            ..TimeSyncResponse::default()
        }
    );
    let scoped = TimeSyncResponse {
        received_timestamp: 0x0102_0304,
        local_timestamp: 0x0506_0708,
        server_session_id: Some(0x1112_1314_1516_1718),
        arena_stream_id: Some(0x2122_2324_2526_2728),
    };
    assert_eq!(
        TimeSyncResponse::parse_response(&scoped.encode())
            .expect("scoped time sync response should parse"),
        scoped
    );
    assert!(TimeSyncResponse::parse_response(&[0x00, 0x06, 0]).is_err());
    assert!(
        TimeSyncResponse::parse_response(&[0x00, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0,]).is_err(),
        "a partial scope suffix must not silently downgrade to the legacy scope"
    );
}

#[test]
fn password_login_request_encodes_subspace_layout() {
    let payload = PasswordLoginRequest::new(
        LoginProtocol::Subspace,
        true,
        "pilot",
        "secret",
        0x1122_3344,
        0xff10,
        0x0102_0304,
        0xabcd,
    )
    .encode_payload();

    assert_eq!(payload.len(), 101);
    assert_eq!(payload[0], 0x09);
    assert_eq!(payload[1], 1);
    assert_eq!(&payload[2..7], b"pilot");
    assert_eq!(&payload[7..34], &[0; 27]);
    assert_eq!(&payload[34..40], b"secret");
    assert_eq!(read_u32(&payload, 66).unwrap(), 0x1122_3344);
    assert_eq!(payload[70], 0x04);
    assert_eq!(read_u16(&payload, 71).unwrap(), 0xff10);
    assert_eq!(read_u16(&payload, 73).unwrap(), 0);
    assert_eq!(read_u16(&payload, 75).unwrap(), 0x86);
    assert_eq!(read_u16(&payload, 77).unwrap(), 444);
    assert_eq!(read_u16(&payload, 79).unwrap(), 0x000f);
    assert_eq!(read_u32(&payload, 81).unwrap(), 0);
    assert_eq!(read_u32(&payload, 85).unwrap(), 0);
    assert_eq!(read_u32(&payload, 89).unwrap(), 0x0102_0304);
    assert_eq!(read_u32(&payload, 93).unwrap(), 0x0000_abcd);
    assert_eq!(read_u32(&payload, 97).unwrap(), 0);
}

#[test]
fn password_login_request_encodes_continuum_layout_and_key_values() {
    let payload = PasswordLoginRequest::new(
        LoginProtocol::Continuum,
        false,
        "012345678901234567890123456789012345",
        "abcdefghijklmnopqrstuvwxyz0123456789",
        1,
        2,
        3,
        4,
    )
    .encode_payload();

    assert_eq!(payload.len(), 165);
    assert_eq!(payload[0], 0x24);
    assert_eq!(payload[1], 0);
    assert_eq!(&payload[2..34], b"01234567890123456789012345678901");
    assert_eq!(&payload[34..66], b"abcdefghijklmnopqrstuvwxyz012345");
    assert_eq!(read_u16(&payload, 75).unwrap(), 40);
    assert_eq!(read_u16(&payload, 79).unwrap(), 0x000f);
    assert_eq!(read_u32(&payload, 101).unwrap(), 0x5a56_42a8);
    assert_eq!(read_u32(&payload, 161).unwrap(), 0x63d1_1557);
}

#[test]
fn password_login_request_wraps_reliably() {
    let payload =
        PasswordLoginRequest::new(LoginProtocol::Continuum, false, "pilot", "", 0, 0, 0, 0)
            .encode_payload();
    let mut sequencer = ReliableSequencer::default();

    assert_eq!(
        sequencer.send_reliable(&payload),
        [&[0x00, 0x03, 0, 0, 0, 0][..], &payload[..]].concat()
    );
}

#[test]
fn password_response_parses_acceptance_and_join_action() {
    let mut packet = vec![0; PasswordResponse::MIN_PACKET_LEN];
    packet[0] = PasswordResponse::TYPE;
    packet[1] = 0x0d;

    let response = PasswordResponse::parse_packet(&packet).expect("response should parse");
    assert_eq!(
        response,
        PasswordResponse {
            code: 0x0d,
            registration_form_requested: false,
        }
    );
    assert_eq!(response.action(), PasswordResponseAction::SendArenaLogin);

    packet[1] = 0x00;
    assert_eq!(
        PasswordResponse::parse_packet(&packet)
            .expect("response should parse")
            .action(),
        PasswordResponseAction::SendArenaLogin
    );
}

#[test]
fn password_response_requests_registration_or_reports_failure() {
    let mut packet = vec![0; PasswordResponse::MIN_PACKET_LEN];
    packet[0] = PasswordResponse::TYPE;
    packet[1] = 0x01;
    assert_eq!(
        PasswordResponse::parse_packet(&packet)
            .expect("response should parse")
            .action(),
        PasswordResponseAction::RequestRegistration
    );

    packet[1] = 0x02;
    assert_eq!(
        PasswordResponse::parse_packet(&packet)
            .expect("response should parse")
            .action(),
        PasswordResponseAction::Failed(0x02)
    );
}

#[test]
fn password_response_registration_form_flag_takes_precedence() {
    let mut packet = vec![0; PasswordResponse::MIN_PACKET_LEN];
    packet[0] = PasswordResponse::TYPE;
    packet[1] = 0x00;
    packet[19] = 1;

    let response = PasswordResponse::parse_packet(&packet).expect("response should parse");
    assert!(response.registration_form_requested);
    assert_eq!(
        response.action(),
        PasswordResponseAction::WaitForRegistrationForm
    );
    assert!(PasswordResponse::parse_packet(&packet[..19]).is_err());
    packet[0] = 0xff;
    assert!(PasswordResponse::parse_packet(&packet).is_err());
}
