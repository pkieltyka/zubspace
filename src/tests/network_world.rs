use super::*;

fn objective_update(generation: u64, version: u32, state: u8) -> ObjectiveStateUpdate {
    ObjectiveStateUpdate {
        kind: ObjectiveStateUpdate::KIND_POWERBALL,
        state,
        objective_id: 2,
        generation,
        version,
        x: 320,
        y: 640,
        velocity_x: 160,
        velocity_y: -160,
        owner_id: None,
        owner_frequency: Some(4),
        server_tick: 10,
        pickup_unlock_tick: None,
        killer_id: None,
        killer_pickup_unlock_tick: None,
        action_id: 9,
        outcome: ObjectiveStateUpdate::OUTCOME_APPLIED,
    }
}

#[test]
fn objective_scores_reconcile_generation_version_and_legacy_reward_packets() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::SoccerGoal(SoccerGoal {
        frequency: 2,
        team_points: 900,
    }));
    world.apply_event(&ClientProtocolEvent::ObjectiveScore(ObjectiveScoreUpdate {
        generation: 3,
        version: 5,
        frequency: 2,
        score: 4,
        server_tick: 10,
        outcome: ObjectiveScoreUpdate::OUTCOME_APPLIED,
    }));
    assert_eq!(world.soccer_scores.get(&2), Some(&4));

    world.apply_event(&ClientProtocolEvent::ObjectiveScore(ObjectiveScoreUpdate {
        generation: 3,
        version: 4,
        frequency: 2,
        score: 99,
        server_tick: 11,
        outcome: ObjectiveScoreUpdate::OUTCOME_REPLAY,
    }));
    assert_eq!(world.soccer_scores.get(&2), Some(&4));

    world.apply_event(&ClientProtocolEvent::ObjectiveScore(ObjectiveScoreUpdate {
        generation: 4,
        version: 1,
        frequency: 2,
        score: 0,
        server_tick: 12,
        outcome: ObjectiveScoreUpdate::OUTCOME_REPLAY,
    }));
    assert_eq!(world.soccer_scores.get(&2), Some(&0));
}

#[test]
fn objective_metadata_reconciles_generation_version_reorder_and_waiting_removal() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::ObjectiveState(objective_update(
        3,
        5,
        ObjectiveStateUpdate::STATE_ON_MAP,
    )));
    assert_eq!(
        world.powerballs.get(&2).unwrap().position,
        vec2(320.0, 640.0)
    );

    let mut stale = objective_update(3, 4, ObjectiveStateUpdate::STATE_CARRIED);
    stale.owner_id = Some(7);
    world.apply_event(&ClientProtocolEvent::ObjectiveState(stale));
    assert!(!world.powerballs.get(&2).unwrap().carried);

    world.apply_event(&ClientProtocolEvent::ObjectiveState(objective_update(
        4,
        1,
        ObjectiveStateUpdate::STATE_WAITING,
    )));
    assert!(!world.powerballs.contains_key(&2));
    assert_eq!(
        world.objective_versions[&(ObjectiveStateUpdate::KIND_POWERBALL, 2)].generation,
        4
    );
}

#[test]
fn authoritative_flag_extensions_idempotently_reconcile_every_carrier_count() {
    let mut world = NetworkWorld::default();
    for (id, name) in [(9, b"alpha".as_slice()), (10, b"beta".as_slice())] {
        world.apply_event(&ClientProtocolEvent::PlayerEntered(
            PlayerRosterEntry::parse(&sample_player_enter_packet(id, name)).unwrap(),
        ));
    }
    let flag = |id, generation, version, tick, state, carrier| ObjectiveStateUpdate {
        kind: ObjectiveStateUpdate::KIND_FLAG,
        state,
        objective_id: id,
        generation,
        version,
        x: 320,
        y: 640,
        velocity_x: 0,
        velocity_y: 0,
        owner_id: carrier,
        owner_frequency: Some(4),
        server_tick: tick,
        pickup_unlock_tick: None,
        killer_id: None,
        killer_pickup_unlock_tick: None,
        action_id: 1,
        outcome: ObjectiveStateUpdate::OUTCOME_APPLIED,
    };

    let first = flag(0, 3, 1, 10, ObjectiveStateUpdate::STATE_CARRIED, Some(9));
    world.apply_event(&ClientProtocolEvent::ObjectiveState(first));
    world.apply_event(&ClientProtocolEvent::ObjectiveState(first));
    world.apply_event(&ClientProtocolEvent::ObjectiveState(flag(
        1,
        3,
        1,
        10,
        ObjectiveStateUpdate::STATE_CARRIED,
        Some(9),
    )));
    assert_eq!(
        world.players[&9].flags, 2,
        "duplicate claim stays idempotent"
    );

    world.apply_event(&ClientProtocolEvent::ObjectiveState(flag(
        0,
        3,
        2,
        11,
        ObjectiveStateUpdate::STATE_CARRIED,
        Some(10),
    )));
    assert_eq!((world.players[&9].flags, world.players[&10].flags), (1, 1));

    world.apply_event(&ClientProtocolEvent::ObjectiveState(first));
    assert_eq!(
        (world.players[&9].flags, world.players[&10].flags),
        (1, 1),
        "reordered pre-transfer state cannot restore the old carrier"
    );

    world.apply_event(&ClientProtocolEvent::ObjectiveState(flag(
        1,
        3,
        2,
        12,
        ObjectiveStateUpdate::STATE_WAITING,
        None,
    )));
    assert_eq!((world.players[&9].flags, world.players[&10].flags), (0, 1));
    world.apply_event(&ClientProtocolEvent::ObjectiveState(flag(
        0,
        3,
        3,
        13,
        ObjectiveStateUpdate::STATE_ON_MAP,
        None,
    )));
    assert_eq!((world.players[&9].flags, world.players[&10].flags), (0, 0));

    let mut late_join = NetworkWorld::default();
    late_join.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"alpha")).unwrap(),
    ));
    late_join.apply_event(&ClientProtocolEvent::ObjectiveState(first));
    late_join.apply_event(&ClientProtocolEvent::ObjectiveState(flag(
        1,
        3,
        1,
        10,
        ObjectiveStateUpdate::STATE_CARRIED,
        Some(9),
    )));
    assert_eq!(late_join.players[&9].flags, 2, "bootstrap replay converges");
}

#[test]
fn objective_powerball_unlock_survives_motion_updates_and_gates_pickup() {
    let mut world = NetworkWorld::default();
    let mut initially_unlocked = objective_update(2, 1, ObjectiveStateUpdate::STATE_ON_MAP);
    initially_unlocked.objective_id = 1;
    world.apply_event(&ClientProtocolEvent::ObjectiveState(initially_unlocked));
    world.update_powerballs(2.0);
    assert!(world.powerballs[&1].authoritative_unlock_provenance);
    assert!(
        !world.powerballs[&1].objective_phased_passing(10.0, Some(11)),
        "an explicitly unlocked authoritative spawn never falls back to legacy age phasing"
    );

    let mut released = objective_update(3, 5, ObjectiveStateUpdate::STATE_ON_MAP);
    released.server_tick = 30;
    released.pickup_unlock_tick = Some(40);
    released.killer_id = Some(7);
    released.killer_pickup_unlock_tick = Some(35);
    world.apply_event(&ClientProtocolEvent::ObjectiveState(released));
    world.update_powerballs(0.4);
    assert!(world.powerballs[&2].objective_pickup_locked(Some(39)));
    assert!(!world.powerballs[&2].objective_pickup_locked(Some(40)));
    assert!(world.powerballs[&2].objective_pickup_locked(None));
    assert!(world.powerballs[&2].objective_pickup_locked_for(7, Some(34)));
    assert!(!world.powerballs[&2].objective_pickup_locked_for(7, Some(35)));
    assert!(world.powerballs[&2].objective_pickup_locked_for(8, Some(35)));

    let mut motion = objective_update(3, 5, ObjectiveStateUpdate::STATE_ON_MAP);
    motion.server_tick = 32;
    motion.action_id = 0;
    motion.outcome = ObjectiveStateUpdate::OUTCOME_REPLAY;
    motion.x = 400;
    motion.pickup_unlock_tick = Some(40);
    motion.killer_id = Some(7);
    motion.killer_pickup_unlock_tick = Some(35);
    world.apply_event(&ClientProtocolEvent::ObjectiveState(motion));

    let mut reordered_motion = motion;
    reordered_motion.server_tick = 31;
    reordered_motion.x = 200;
    world.apply_event(&ClientProtocolEvent::ObjectiveState(reordered_motion));

    let ball = world.powerballs[&2].clone();
    assert_eq!(ball.position.x, 400.0);
    assert_eq!(ball.pickup_unlock_tick, Some(40));
    assert_eq!(ball.killer_id, Some(7));
    assert_eq!(ball.killer_pickup_unlock_tick, Some(35));
    assert_eq!(ball.age_since_update, 0.4);
    assert!(ball.objective_phased_passing(0.0, Some(39)));
    assert!(!ball.objective_phased_passing(1.0, Some(40)));
    let identity = world.objective_versions[&(ObjectiveStateUpdate::KIND_POWERBALL, 2)];
    assert_eq!(
        identity.version, 5,
        "motion does not consume semantic versions"
    );
    assert_eq!(identity.last_action_id, released.action_id);
    assert_eq!(identity.last_update_tick, 32);

    let mut cleared = motion;
    cleared.server_tick = 33;
    cleared.owner_id = None;
    cleared.owner_frequency = None;
    cleared.pickup_unlock_tick = None;
    cleared.killer_id = None;
    cleared.killer_pickup_unlock_tick = None;
    world.apply_event(&ClientProtocolEvent::ObjectiveState(cleared));
    let cleared_ball = &world.powerballs[&2];
    assert_eq!(cleared_ball.killer_id, None);
    assert_eq!(cleared_ball.killer_pickup_unlock_tick, None);
    assert!(!cleared_ball.objective_pickup_locked_for(7, Some(39)));
    assert!(!cleared_ball.objective_phased_passing(10.0, Some(39)));
    let mut reordered_locked = motion;
    reordered_locked.server_tick = 32;
    world.apply_event(&ClientProtocolEvent::ObjectiveState(reordered_locked));
    assert_eq!(world.powerballs[&2].pickup_unlock_tick, None);
    assert_eq!(
        world.objective_versions[&(ObjectiveStateUpdate::KIND_POWERBALL, 2)].version,
        5,
        "metadata cleanup keeps the semantic version while advancing motion time"
    );

    let mut own_moving_release = ball.clone();
    own_moving_release.owner_id = Some(7);
    assert!(own_moving_release.objective_pickup_blocked_for_last_thrower(7));
    assert!(!own_moving_release.objective_pickup_blocked_for_last_thrower(8));
    own_moving_release.velocity = Vec2::ZERO;
    assert!(!own_moving_release.objective_pickup_blocked_for_last_thrower(7));
}

#[test]
fn network_world_tracks_powerball_position_and_removal() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PowerballPosition(
        PowerballPositionUpdate {
            ball_id: 2,
            x: 300,
            y: 450,
            velocity_x: 125,
            velocity_y: -75,
            owner_id: PowerballPositionUpdate::INVALID_OWNER,
            timestamp: 55,
        },
    ));

    assert_eq!(
        world.powerballs.get(&2),
        Some(&NetworkPowerball {
            ball_id: 2,
            position: vec2(300.0, 450.0),
            velocity: vec2(12.5, -7.5),
            owner_id: None,
            timestamp: 55,
            carried: false,
            pickup_unlock_tick: None,
            killer_id: None,
            killer_pickup_unlock_tick: None,
            authoritative_unlock_provenance: false,
            age_since_update: 0.0,
        })
    );
    assert_eq!(
        world
            .powerballs
            .get(&2)
            .map(|ball| ball.phased_passing(0.5)),
        Some(true)
    );
    world.update_powerballs(2.0);
    assert_eq!(
        world.powerballs.get(&2).map(|ball| ball.position),
        Some(vec2(325.0, 435.0))
    );
    assert_eq!(
        world
            .powerballs
            .get(&2)
            .map(|ball| ball.phased_passing(0.5)),
        Some(false)
    );

    world.apply_event(&ClientProtocolEvent::PowerballPosition(
        PowerballPositionUpdate {
            ball_id: 2,
            x: 800,
            y: 900,
            velocity_x: 0,
            velocity_y: 0,
            owner_id: PowerballPositionUpdate::INVALID_OWNER,
            timestamp: 54,
        },
    ));
    assert_eq!(
        world.powerballs.get(&2).map(|ball| {
            (
                ball.position,
                ball.velocity,
                ball.timestamp,
                ball.age_since_update,
            )
        }),
        Some((vec2(325.0, 435.0), vec2(12.5, -7.5), 55, 2.0))
    );

    world.apply_event(&ClientProtocolEvent::PowerballPosition(
        PowerballPositionUpdate {
            ball_id: 2,
            x: 400,
            y: 500,
            velocity_x: 160,
            velocity_y: 0,
            owner_id: PowerballPositionUpdate::INVALID_OWNER,
            timestamp: 56,
        },
    ));
    assert_eq!(
        world.powerballs.get(&2).map(|ball| {
            (
                ball.position,
                ball.velocity,
                ball.timestamp,
                ball.age_since_update,
            )
        }),
        Some((vec2(400.0, 500.0), vec2(16.0, 0.0), 56, 0.0))
    );
    assert_eq!(
        world
            .powerballs
            .get(&2)
            .map(|ball| ball.radar_kind(&world, 7)),
        Some(NetworkPowerballRadarKind::Loose)
    );

    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"carrier")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 1,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: Some(88),
        items: None,
    }));
    {
        let carrier = world.players.get_mut(&9).unwrap();
        carrier.position = Some(vec2(100.75, 200.75));
        carrier.render_position = Some(vec2(100.75, 200.75));
    }

    world.apply_event(&ClientProtocolEvent::PowerballPosition(
        PowerballPositionUpdate {
            ball_id: 2,
            x: 300,
            y: 450,
            velocity_x: 0,
            velocity_y: 0,
            owner_id: 9,
            timestamp: 0,
        },
    ));
    assert!(matches!(
        world.powerballs.get(&2),
        Some(ball) if ball.owner_id == Some(9) && ball.carried
    ));
    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.ships[3].radius_pixels = 20.0;
    let carried_ball = world.powerballs.get(&2).expect("carried ball should exist");
    let (expected_x, expected_y) =
        crate::continuum_position::canonical_carried_powerball_position(100.75, 200.75, 0.0, 20.0);
    let expected_carried_position = vec2(expected_x, expected_y);
    assert!(
        carried_ball
            .display_position(&world, &tuning)
            .distance(expected_carried_position)
            < 0.001
    );
    assert!(!carried_ball.phased_passing(0.5));
    assert_eq!(
        carried_ball.radar_kind(&world, 7),
        NetworkPowerballRadarKind::TeamCarrier
    );
    assert_eq!(
        carried_ball.radar_kind(&world, 8),
        NetworkPowerballRadarKind::EnemyCarrier
    );
    world.update_powerballs(2.0);
    let carried_ball = world
        .powerballs
        .get(&2)
        .expect("carried ball should remain");
    assert!(
        carried_ball
            .display_position(&world, &tuning)
            .distance(expected_carried_position)
            < 0.001
    );

    world.apply_event(&ClientProtocolEvent::PowerballPosition(
        PowerballPositionUpdate {
            ball_id: 2,
            x: 0,
            y: 0,
            velocity_x: 0,
            velocity_y: 0,
            owner_id: PowerballPositionUpdate::INVALID_OWNER,
            timestamp: 0,
        },
    ));
    assert!(!world.powerballs.contains_key(&2));

    world.apply_event(&ClientProtocolEvent::SoccerGoal(SoccerGoal {
        frequency: 3,
        team_points: 12_345,
    }));
    assert_eq!(world.soccer_scores.get(&3), Some(&12_345));

    world.apply_event(&ClientProtocolEvent::PowerballPosition(
        PowerballPositionUpdate {
            ball_id: 3,
            x: 1,
            y: 1,
            velocity_x: 0,
            velocity_y: 0,
            owner_id: PowerballPositionUpdate::INVALID_OWNER,
            timestamp: 1,
        },
    ));
    world.apply_event(&ClientProtocolEvent::PlayerIdAssigned(7));
    assert!(world.powerballs.is_empty());
    assert!(world.soccer_scores.is_empty());
}

#[test]
fn network_world_tracks_roster_identity_and_visibility() {
    let mut world = NetworkWorld::default();
    let self_entry = PlayerRosterEntry::parse(&sample_player_enter_packet(7, b"self")).unwrap();
    let mut remote_entry =
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap();
    remote_entry.ship = 2;
    let mut spectator_entry =
        PlayerRosterEntry::parse(&sample_player_enter_packet(10, b"spec")).unwrap();
    spectator_entry.ship = 8;

    world.apply_event(&ClientProtocolEvent::PlayerIdAssigned(7));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(self_entry.clone()));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(remote_entry.clone()));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(spectator_entry));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 7,
        direction: 0,
        timestamp: 1,
        x: 16,
        y: 16,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 10,
        timestamp: 2,
        x: 32,
        y: 48,
        velocity_x: 160,
        velocity_y: -80,
        togglables: 0b1010_0000,
        ping: 6,
        bounty: 4,
        weapon: 3,
        checksum: None,
        energy: Some(900),
        s2c_latency: None,
        flag_timer: Some(321),
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(11, b"child")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
        requester_id: 11,
        destination_id: Some(9),
    }));

    assert_eq!(world.viewer_player_id(), Some(7));
    assert_eq!(world.players.len(), 4);
    assert_eq!(
        world.players.get(&9).and_then(|player| player.flag_timer),
        Some(321)
    );

    world.apply_event(&ClientProtocolEvent::ScoreUpdated(ScoreUpdate {
        assists: 0,
        player_id: 9,
        kill_points: 55_000,
        flag_points: 66_000,
        wins: 13,
        losses: 4,
    }));
    let remote = world.players.get(&9).expect("remote player should exist");
    assert_eq!(remote.name, "remote");
    assert_eq!(remote.kill_points, 55_000);
    assert_eq!(remote.flag_points, 66_000);
    assert_eq!(remote.wins, 13);
    assert_eq!(remote.losses, 4);
    world.apply_event(&ClientProtocolEvent::ScoresReset(ResetScore {
        player_id: 9,
    }));
    let remote = world.players.get(&9).expect("remote player should exist");
    assert_eq!(remote.kill_points, 0);
    assert_eq!(remote.flag_points, 0);
    assert_eq!(remote.wins, 0);
    assert_eq!(remote.losses, 0);
    assert_eq!(remote.position, Some(vec2(32.0, 48.0)));
    assert_eq!(remote.velocity, vec2(16.0, -8.0));
    assert_eq!(remote.orientation, 0.25);
    assert_eq!(remote.bounty, 4);
    assert_eq!(remote.energy, Some(900.0));
    assert_eq!(remote.togglables, 0b1010_0000);
    assert_eq!(remote.weapon, 3);
    assert_eq!(remote.last_server_position_tick, Some(2));

    world.apply_event(&ClientProtocolEvent::DamageWatched(ServerDamageReport {
        player_id: 9,
        timestamp: 5,
        entries: vec![ServerDamageReportEntry {
            attacker_id: 7,
            weapon_data: 2,
            energy: 760,
            damage: 140,
            timestamp_offset: 0,
            energy_before: None,
            damage_seq: None,
            projectile_id: None,
            victim_life_seq: None,
            shooter_life_seq: None,
            explicit_lethal: None,
            source_snapshot_id: None,
            source_snapshot_epoch: None,
        }],
    }));
    assert_eq!(
        world.players.get(&9).and_then(|player| player.energy),
        Some(900.0),
        "WATCH_DAMAGE must not mutate remote energy by reusable numeric id"
    );

    let visible: Vec<u16> = world
        .visible_players()
        .map(|player| player.player_id)
        .collect();
    assert_eq!(visible, vec![9]);

    world.apply_event(&ClientProtocolEvent::PlayerLeft(9));
    assert!(!world.players.contains_key(&9));
    let child = world.players.get(&11).expect("child should remain");
    assert_eq!(child.attach_parent, INVALID_PLAYER_ID);
    assert_eq!(child.position, None);
    assert_eq!(child.velocity, Vec2::ZERO);
}

#[test]
fn network_world_smooths_remote_player_display_positions() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 100,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    assert_eq!(
        world
            .players
            .get(&9)
            .and_then(NetworkPlayer::display_position),
        Some(vec2(100.0, 200.0))
    );

    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 10,
        timestamp: 105,
        x: 200,
        y: 200,
        velocity_x: 1000,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    let admitted = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("accepted remote update should retain a display position");
    assert_eq!(admitted, vec2(188.0, 200.0));
    assert_eq!(
        admitted.distance(vec2(200.0, 200.0)),
        12.0,
        "snapshot admission should immediately restore the visible combat hull to the bounded authority leash"
    );
    assert!(
        world
            .players
            .get(&9)
            .is_some_and(|player| player.display_orientation().abs() < 0.001)
    );

    world.update_remote_players(0.025, None);
    let halfway = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("remote player should have a display position");
    // Mid-interpolation: advancing toward the 200.0 target without snapping or overshooting.
    assert!(
        halfway.x > 100.0 && halfway.x < 200.0,
        "display x should be mid-interpolation between start and target, got {}",
        halfway.x
    );
    assert!((halfway.y - 200.0).abs() < 0.01);
    let halfway_orientation = world
        .players
        .get(&9)
        .expect("remote player should exist")
        .display_orientation();
    // Orientation interpolates linearly along the short path toward the target (0.25).
    assert!(
        halfway_orientation > 0.0 && halfway_orientation <= 0.25 + 1e-4,
        "orientation should advance along the short path, got {halfway_orientation}"
    );

    world.update_remote_players(0.025, None);
    let later = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("remote player should still interpolate");
    // Monotonic progress toward the target with no overshoot.
    assert!(
        later.x >= halfway.x && later.x <= 200.0 + 1e-3,
        "display x should advance monotonically toward the target, got {} after {}",
        later.x,
        halfway.x
    );
    let later_orientation = world
        .players
        .get(&9)
        .expect("remote player should exist")
        .display_orientation();
    assert!(
        later_orientation >= halfway_orientation && later_orientation <= 0.25 + 1e-4,
        "orientation should keep advancing along the short path, got {later_orientation}"
    );

    world.update_remote_players(0.08, None);
    let buffered = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("remote player should still have a display position");
    assert!(
        buffered.x >= later.x && buffered.x <= 200.0 + 1e-3,
        "display x should keep advancing toward the target without overshooting, got {}",
        buffered.x
    );

    world.update_remote_players(0.4, None);
    let held = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("remote player should keep converging during a network hiccup");
    let maximum_normal_frame_step = (100.0 + 180.0) / 30.0;
    assert!(
        held.x >= buffered.x && held.x - buffered.x <= maximum_normal_frame_step + 1e-3,
        "one presentation call must not turn a long stall into a visible lurch: {} -> {}",
        buffered.x,
        held.x,
    );
    assert_eq!(
        world.stats.remote_extrapolated_frames, 0,
        "remote rendering should avoid client-side extrapolation overshoot"
    );

    world.update_remote_players(0.4, None);
    let clamped = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("remote player should keep converging toward its server target");
    assert!(
        clamped.x >= held.x && clamped.x - held.x <= maximum_normal_frame_step + 1e-3,
        "each presentation call must retain the same visible correction bound: {} -> {}",
        held.x,
        clamped.x,
    );
    for _ in 0..20 {
        world.update_remote_players(1.0 / 60.0, None);
    }
    assert!(
        world.players.get(&9).is_some_and(|player| player
            .display_position()
            .is_some_and(|position| (position.x - 200.0).abs() < 0.01)),
        "bounded corrections must still converge to the server target"
    );

    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 110,
        x: 2000,
        y: 2000,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    assert_eq!(
        world
            .players
            .get(&9)
            .and_then(NetworkPlayer::display_position),
        Some(vec2(2000.0, 2000.0))
    );
}

#[test]
fn unchanged_remote_position_does_not_restart_interpolation() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 100,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 105,
        x: 200,
        y: 200,
        velocity_x: 1000,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.update_remote_players(0.025, None);
    let before_noop = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("remote player should have started interpolating");
    // Mid-interpolation between the 100.0 start and the 200.0 target.
    assert!(
        before_noop.x > 100.0 && before_noop.x < 200.0,
        "remote player should be interpolating between start and target, got {}",
        before_noop.x
    );

    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 106,
        x: 200,
        y: 200,
        velocity_x: 1000,
        velocity_y: 0,
        togglables: 0,
        ping: 1,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: Some(1600),
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.update_remote_players(0.025, None);
    let after_noop = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("unchanged position snapshot should preserve interpolation progress");
    // The same-position update must not restart interpolation: progress continues
    // forward from where it was, advancing toward the target without going backwards.
    assert!(
        after_noop.x > before_noop.x && after_noop.x <= 200.0 + 1e-3,
        "unchanged position update should not restart interpolation from {before_noop:?}; got {after_noop:?}"
    );
    let player = world.players.get(&9).expect("remote player should exist");
    assert_eq!(player.ping, 1);
    assert_eq!(player.energy, Some(1600.0));
    assert_eq!(player.last_server_position_tick, Some(106));
}

#[test]
fn unchanged_remote_position_still_smooths_orientation_updates() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 100,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 105,
        x: 200,
        y: 200,
        velocity_x: 1000,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.update_remote_players(0.025, None);
    let before = world.players.get(&9).expect("remote player should exist");
    let before_x = before.display_position().unwrap().x;
    assert!(
        before_x > 100.0 && before_x < 200.0,
        "remote player should be mid-interpolation, got {before_x}"
    );
    assert!(before.display_orientation().abs() < 0.001);

    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 38,
        timestamp: 106,
        x: 200,
        y: 200,
        velocity_x: 1000,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    let just_after_update = world
        .players
        .get(&9)
        .expect("remote player should still exist");
    assert!(
        just_after_update.display_orientation().abs() < 0.001,
        "same-position rotation update should start from the current displayed orientation"
    );

    world.update_remote_players(0.04, None);
    let halfway = world
        .players
        .get(&9)
        .expect("remote player should still exist");
    let halfway_x = halfway.display_position().unwrap().x;
    assert!(
        halfway_x > before_x && halfway_x <= 200.0 + 1e-3,
        "same-position rotation update should not restart position interpolation, got {halfway_x} after {before_x}"
    );
    let orientation = halfway.display_orientation();
    // Target direction 38 == orientation 0.95. From 0.0 the short path wraps backwards
    // through 1.0, so the displayed orientation stays in the arc (0.95, 1.0) heading
    // toward 0.95 and never jumps the long way around.
    assert!(
        orientation > 0.95 && orientation < 1.0,
        "orientation should take the short wrapped path smoothly toward 0.95, got {orientation}"
    );

    world.update_remote_players(0.4, None);
    let settled = world
        .players
        .get(&9)
        .expect("remote player should still exist");
    let settled_orientation = settled.display_orientation();
    assert!(
        (settled_orientation - 0.95).abs() < 0.001,
        "orientation should reach and hold the target after enough elapsed time, got {settled_orientation}"
    );
}

#[test]
fn network_world_remote_energy_is_fully_live_from_snapshots_after_damage() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 100,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: Some(1700),
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));

    world.apply_event(&ClientProtocolEvent::DamageWatched(ServerDamageReport {
        player_id: 9,
        timestamp: 200,
        entries: vec![ServerDamageReportEntry {
            attacker_id: 7,
            weapon_data: 66,
            energy: 1300,
            damage: 400,
            timestamp_offset: 0,
            energy_before: Some(1700),
            damage_seq: Some(1),
            projectile_id: Some(9),
            victim_life_seq: Some(1),
            shooter_life_seq: Some(1),
            explicit_lethal: Some(false),
            source_snapshot_id: Some(1),
            source_snapshot_epoch: Some(1),
        }],
    }));
    assert_eq!(
        world
            .players
            .get(&9)
            .and_then(NetworkPlayer::display_energy),
        Some(1700.0),
        "WATCH_DAMAGE remains an event and must not clamp remote world energy"
    );

    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 101,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: Some(1318),
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    assert_eq!(
        world
            .players
            .get(&9)
            .and_then(NetworkPlayer::display_energy),
        Some(1318.0),
        "remote energy is fully live: a recharge snapshot shows immediately, not frozen at the hit value"
    );

    // Further recharge keeps tracking the live server energy.
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 102,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: Some(1450),
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    assert_eq!(
        world
            .players
            .get(&9)
            .and_then(NetworkPlayer::display_energy),
        Some(1450.0),
        "remote energy keeps following live server recharge"
    );
}

#[test]
fn network_world_watch_damage_does_not_mutate_self_or_remote_energy() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerIdAssigned(5));
    world.apply_event(&ClientProtocolEvent::JoinedArena(1));
    for (player_id, name, energy) in [(5, b"self".as_slice(), 1_700), (9, b"remote", 1_500)] {
        world.apply_event(&ClientProtocolEvent::PlayerEntered(
            PlayerRosterEntry::parse(&sample_player_enter_packet(player_id, name)).unwrap(),
        ));
        world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
            player_id,
            direction: 0,
            timestamp: 100,
            x: 100,
            y: 200,
            velocity_x: 0,
            velocity_y: 0,
            togglables: 0,
            ping: 0,
            bounty: 0,
            weapon: 0,
            checksum: None,
            energy: Some(energy),
            s2c_latency: None,
            flag_timer: None,
            items: None,
        }));
    }

    for (player_id, stale_energy) in [(5, 200), (9, 300)] {
        world.apply_event(&ClientProtocolEvent::DamageWatched(ServerDamageReport {
            player_id,
            timestamp: 200,
            entries: vec![ServerDamageReportEntry {
                attacker_id: 7,
                weapon_data: 66,
                energy: stale_energy,
                damage: 400,
                timestamp_offset: 0,
                energy_before: Some(stale_energy.saturating_add(400)),
                damage_seq: Some(1),
                projectile_id: Some(9),
                victim_life_seq: Some(1),
                shooter_life_seq: Some(1),
                explicit_lethal: Some(false),
                source_snapshot_id: Some(1),
                source_snapshot_epoch: Some(1),
            }],
        }));
    }

    assert_eq!(world.players[&5].energy, Some(1_700.0));
    assert_eq!(world.players[&9].energy, Some(1_500.0));

    world.apply_event(&ClientProtocolEvent::DamageWatched(ServerDamageReport {
        player_id: 9,
        timestamp: 201,
        entries: vec![ServerDamageReportEntry {
            attacker_id: 7,
            weapon_data: 66,
            energy: 0,
            damage: 400,
            timestamp_offset: 0,
            energy_before: Some(300),
            damage_seq: Some(2),
            projectile_id: Some(10),
            victim_life_seq: Some(3),
            shooter_life_seq: Some(1),
            explicit_lethal: Some(true),
            source_snapshot_id: Some(2),
            source_snapshot_epoch: Some(1),
        }],
    }));
    assert_eq!(world.players[&9].life_seq, Some(4));
    assert_eq!(world.players[&9].energy, Some(1_500.0));

    world.apply_event(&ClientProtocolEvent::DamageWatched(ServerDamageReport {
        player_id: 9,
        timestamp: 202,
        entries: vec![ServerDamageReportEntry {
            attacker_id: 7,
            weapon_data: 66,
            energy: 0,
            damage: 400,
            timestamp_offset: 0,
            energy_before: Some(300),
            damage_seq: Some(1),
            projectile_id: Some(9),
            victim_life_seq: Some(1),
            shooter_life_seq: Some(1),
            explicit_lethal: Some(true),
            source_snapshot_id: Some(3),
            source_snapshot_epoch: Some(1),
        }],
    }));
    assert_eq!(world.players[&9].life_seq, Some(4));
    assert_eq!(world.players[&9].energy, Some(1_500.0));

    world.players.get_mut(&9).unwrap().life_seq = Some(u32::MAX);
    world.apply_event(&ClientProtocolEvent::DamageWatched(ServerDamageReport {
        player_id: 9,
        timestamp: 203,
        entries: vec![ServerDamageReportEntry {
            attacker_id: 7,
            weapon_data: 66,
            energy: 0,
            damage: 400,
            timestamp_offset: 0,
            energy_before: Some(300),
            damage_seq: Some(3),
            projectile_id: Some(11),
            victim_life_seq: Some(u32::MAX),
            shooter_life_seq: Some(1),
            explicit_lethal: Some(true),
            source_snapshot_id: Some(4),
            source_snapshot_epoch: Some(1),
        }],
    }));
    assert_eq!(world.players[&9].life_seq, Some(1));
    assert_eq!(world.players[&9].energy, Some(1_500.0));
}

fn snapshot_damage_report(
    victim_id: u16,
    attacker_id: u16,
    victim_life_seq: u32,
    source_snapshot_id: u16,
    source_snapshot_epoch: u32,
    explicit_lethal: bool,
    energy: u16,
) -> ServerDamageReport {
    ServerDamageReport {
        player_id: victim_id,
        timestamp: u32::from(source_snapshot_id),
        entries: vec![ServerDamageReportEntry {
            attacker_id,
            weapon_data: 66,
            energy,
            damage: 400,
            timestamp_offset: 0,
            energy_before: Some(energy.saturating_add(400)),
            damage_seq: Some(u32::from(source_snapshot_id)),
            projectile_id: Some(u32::from(source_snapshot_id)),
            victim_life_seq: Some(victim_life_seq),
            shooter_life_seq: Some(8),
            explicit_lethal: Some(explicit_lethal),
            source_snapshot_id: Some(source_snapshot_id),
            source_snapshot_epoch: Some(source_snapshot_epoch),
        }],
    }
}

#[test]
fn watch_damage_lifecycle_repair_is_remote_explicit_ordered_and_arena_bound() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerIdAssigned(5));
    world.apply_event(&ClientProtocolEvent::JoinedArena(1));
    for (player_id, name, life_seq) in [
        (5, b"self".as_slice(), 10),
        (7, b"attacker", 8),
        (9, b"victim", 20),
    ] {
        world.apply_event(&ClientProtocolEvent::PlayerEntered(
            PlayerRosterEntry::parse(&sample_player_enter_packet(player_id, name)).unwrap(),
        ));
        let player = world.players.get_mut(&player_id).unwrap();
        player.set_life_seq(Some(life_seq));
        player.position = Some(vec2(f32::from(player_id), 64.0));
        player.render_position = player.position;
        player.energy = Some(1_500.0);
        player.last_seen_snapshot_id = Some(10);
    }
    let victim_before = (
        world.players[&9].position,
        world.players[&9].render_position,
        world.players[&9].energy,
        world.players[&9].last_seen_snapshot_id,
    );

    // Explicit non-lethal wins over a contradictory zero-energy diagnostic.
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        9, 7, 20, 11, 1, false, 0,
    )));
    assert_eq!(world.players[&9].life_seq, Some(20));

    // Explicit lethal wins independently of the diagnostic energy value.
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        9, 7, 20, 12, 1, true, 900,
    )));
    assert_eq!(world.players[&9].life_seq, Some(21));
    assert_eq!(world.players[&7].life_seq, Some(8), "attacker is isolated");
    assert_eq!(world.players[&5].life_seq, Some(10));
    assert_eq!(
        (
            world.players[&9].position,
            world.players[&9].render_position,
            world.players[&9].energy,
            world.players[&9].last_seen_snapshot_id,
        ),
        victim_before,
        "lifecycle repair cannot mutate position, visibility, or energy",
    );

    // A self-victim report is never a self-lifecycle source.
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        5, 7, 10, 13, 1, true, 0,
    )));
    assert_eq!(world.players[&5].life_seq, Some(10));

    // Once snapshot 12 is applied, duplicate/backward reports cannot repair.
    world.last_received_snapshot_id = Some(12);
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        9, 7, 21, 12, 1, true, 0,
    )));
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        9, 7, 21, 11, 1, true, 0,
    )));
    assert_eq!(world.players[&9].life_seq, Some(21));

    // Snapshot ids wrap from MAX to 1 and still admit the exact victim life.
    world
        .players
        .get_mut(&9)
        .unwrap()
        .set_life_seq(Some(u32::MAX));
    world.last_received_snapshot_id = Some(u16::MAX);
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        9,
        7,
        u32::MAX,
        1,
        1,
        true,
        0,
    )));
    assert_eq!(world.players[&9].life_seq, Some(1));

    // PLAYER_DEATH before or after the same lethal snapshot converges on one
    // generation without an extra advance.
    world.apply_event(&ClientProtocolEvent::PlayerDeath(PlayerDeath {
        green_id: -1,
        green_collectible_id: 0,
        killer_id: 7,
        killed_id: 9,
        bounty: 0,
        flag_transfer: 0,
        life_seq: 30,
        respawn_at_tick: 0,
    }));
    world.last_received_snapshot_id = Some(1);
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        9, 7, 29, 2, 1, true, 0,
    )));
    assert_eq!(world.players[&9].life_seq, Some(30));
    world.players.get_mut(&9).unwrap().set_life_seq(Some(40));
    world.last_received_snapshot_id = Some(2);
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        9, 7, 40, 3, 1, true, 0,
    )));
    world.apply_event(&ClientProtocolEvent::PlayerDeath(PlayerDeath {
        green_id: -1,
        green_collectible_id: 0,
        killer_id: 7,
        killed_id: 9,
        bounty: 0,
        flag_transfer: 0,
        life_seq: 41,
        respawn_at_tick: 0,
    }));
    assert_eq!(world.players[&9].life_seq, Some(41));

    // A reliable death delayed until an authoritative snapshot already proves
    // its entered life alive must keep that exact life's render proxy and
    // actor-scoped presentation counters intact.
    {
        let player = world.players.get_mut(&9).unwrap();
        player.position = Some(vec2(90.0, 90.0));
        player.render_position = player.position;
        player.presentation_initial_count = 7;
    }
    world.authoritative_snapshot_id = Some(4);
    world.authoritative_snapshot_actors = vec![AuthoritativeSnapshotActor {
        player_id: 9,
        life_seq: Some(41),
    }];
    world.apply_event(&ClientProtocolEvent::PlayerDeath(PlayerDeath {
        green_id: -1,
        green_collectible_id: 0,
        killer_id: 7,
        killed_id: 9,
        bounty: 0,
        flag_transfer: 0,
        life_seq: 41,
        respawn_at_tick: 0,
    }));
    assert_eq!(world.players[&9].life_seq, Some(41));
    assert_eq!(world.players[&9].position, Some(vec2(90.0, 90.0)));
    assert_eq!(world.players[&9].presentation_initial_count, 7);

    world.players.get_mut(&9).unwrap().set_life_seq(Some(1));
    world.authoritative_snapshot_actors[0].life_seq = Some(1);
    world.apply_event(&ClientProtocolEvent::PlayerDeath(PlayerDeath {
        green_id: -1,
        green_collectible_id: 0,
        killer_id: 7,
        killed_id: 9,
        bounty: 0,
        flag_transfer: 0,
        life_seq: u32::MAX,
        respawn_at_tick: 0,
    }));
    assert_eq!(world.players[&9].life_seq, Some(1));
    assert_eq!(world.players[&9].position, Some(vec2(90.0, 90.0)));
    world.players.get_mut(&9).unwrap().set_life_seq(Some(41));

    // Arena generation is authoritative even when an old snapshot id looks
    // forward after the new arena reset.
    world.apply_event(&ClientProtocolEvent::JoinedArena(2));
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        9, 7, 41, 100, 1, true, 0,
    )));
    assert_eq!(world.players[&9].life_seq, Some(41));
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        9, 7, 41, 1, 2, true, 0,
    )));
    assert_eq!(world.players[&9].life_seq, Some(42));

    // A new transport/session assignment clears the old roster, so even a
    // colliding player id cannot receive an old event.
    world.apply_event(&ClientProtocolEvent::PlayerIdAssigned(5));
    world.apply_event(&ClientProtocolEvent::DamageWatched(snapshot_damage_report(
        9, 7, 42, 2, 2, true, 0,
    )));
    assert!(!world.players.contains_key(&9));
}

#[test]
fn network_world_buffers_short_rtc_timestamp_gaps_for_smoother_rotation() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 100,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 10,
        timestamp: 102,
        x: 120,
        y: 200,
        velocity_x: 1000,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    assert_eq!(world.stats.remote_snap_count, 1);
    assert_eq!(world.stats.last_remote_snap_reason, 1);

    let admitted = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("accepted remote update should retain a display position");
    assert_eq!(admitted, vec2(108.0, 200.0));
    assert_eq!(
        admitted.distance(vec2(120.0, 200.0)),
        12.0,
        "short-gap admission should begin at the bounded authority leash"
    );

    // Buffering invariant: a short tick-gap (timestamp 100 -> 102) still plays out
    // over several frames rather than completing in one small step. Admission first
    // restores the visible hull to the authority leash, then the first 0.02s frame
    // advances smoothly without reaching the 120.0 target.
    world.update_remote_players(0.02, None);
    let mut prev = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("remote player should interpolate between short-gap packets");
    assert!(
        prev.x > admitted.x && prev.x < 120.0,
        "short tick-gap should still be buffered after one frame, got {}",
        prev.x
    );
    let mut prev_orientation = world
        .players
        .get(&9)
        .expect("remote player should exist")
        .display_orientation();
    assert!(
        prev_orientation > 0.0 && prev_orientation <= 0.25 + 1e-4,
        "orientation should advance along the short path toward 0.25, got {prev_orientation}"
    );

    // Step forward several more small frames, asserting monotonic progress toward the
    // target with no overshoot for both position and orientation.
    for _ in 0..4 {
        world.update_remote_players(0.02, None);
        let sample = world
            .players
            .get(&9)
            .and_then(NetworkPlayer::display_position)
            .expect("remote player should still be buffered");
        assert!(
            sample.x >= prev.x && sample.x <= 120.0 + 1e-3,
            "display x should advance monotonically toward the target without overshoot, got {} after {}",
            sample.x,
            prev.x
        );
        let orientation = world
            .players
            .get(&9)
            .expect("remote player should exist")
            .display_orientation();
        assert!(
            orientation >= prev_orientation && orientation <= 0.25 + 1e-4,
            "orientation should advance monotonically toward 0.25, got {orientation} after {prev_orientation}"
        );
        prev = sample;
        prev_orientation = orientation;
    }

    world.update_remote_players(0.02, None);
    assert_eq!(
        world
            .players
            .get(&9)
            .and_then(NetworkPlayer::display_position),
        Some(vec2(120.0, 200.0))
    );
    assert!(
        world
            .players
            .get(&9)
            .is_some_and(|player| player.display_orientation() < 0.25),
        "the fastest-ship turn-rate bound should keep a quarter-turn in flight after 120 ms"
    );
    for _ in 0..11 {
        world.update_remote_players(0.02, None);
    }
    assert!(
        world
            .players
            .get(&9)
            .is_some_and(|player| (player.display_orientation() - 0.25).abs() < 0.001)
    );
    assert_eq!(
        world.stats.remote_snap_count, 1,
        "smooth remote interpolation should not create new snap corrections"
    );
}

#[test]
fn remote_hermite_straight_line_motion_stays_near_linear_without_overshoot() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    ));
    // Straight, constant-velocity +x motion that is self-consistent: 100 px over 5 ticks
    // (0.05s) at velocity_x 2000 (200 px/s) exactly matches the displacement. y and
    // direction are constant, so the path is a straight horizontal line.
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 100,
        x: 1000,
        y: 200,
        velocity_x: 2000,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 105,
        x: 1100,
        y: 200,
        velocity_x: 2000,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));

    // For consistent straight motion the velocity-aware Hermite should stay essentially
    // linear: monotonic, no overshoot past the 1100 target, and within ~2px of the chord.
    let mut prev_x = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("remote player should have a display position")
        .x;
    for _ in 0..30 {
        world.update_remote_players(0.01, None);
        let pos = world
            .players
            .get(&9)
            .and_then(NetworkPlayer::display_position)
            .expect("remote player should keep a display position");
        assert!(
            pos.x >= prev_x - 1e-4,
            "display x should be monotonically non-decreasing, got {} after {}",
            pos.x,
            prev_x
        );
        assert!(
            pos.x <= 1100.0 + 1e-3,
            "display x should never overshoot the straight-line target, got {}",
            pos.x
        );
        // The straight-line chord between 1000 and 1100 lives on y = 200, so any sample
        // within the interpolation window must sit within ~2px of that chord in x.
        let clamped = pos.x.clamp(1000.0, 1100.0);
        assert!(
            (pos.x - clamped).abs() < 2.0,
            "Hermite should track the straight-line chord within ~2px, got {}",
            pos.x
        );
        assert!(
            (pos.y - 200.0).abs() < 1e-3,
            "y should stay on the straight-line chord, got {}",
            pos.y
        );
        prev_x = pos.x;
    }

    // After enough total elapsed time the display reaches and holds the server target.
    world.update_remote_players(0.4, None);
    let settled = world
        .players
        .get(&9)
        .and_then(NetworkPlayer::display_position)
        .expect("remote player should reach the target");
    assert!(
        (settled.x - 1100.0).abs() < 0.01,
        "display x should reach the straight-line target, got {}",
        settled.x
    );
}

#[test]
fn network_world_counts_remote_distance_snaps() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 100,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 101,
        x: 2000,
        y: 2000,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));

    assert_eq!(world.stats.remote_snap_count, 2);
    assert_eq!(world.stats.last_remote_snap_reason, 2);
    assert_eq!(
        world
            .players
            .get(&9)
            .and_then(NetworkPlayer::display_position),
        Some(vec2(2000.0, 2000.0))
    );
}

#[test]
fn network_world_does_not_count_self_authority_snapshots_as_remote_snaps() {
    let mut world = NetworkWorld::default();
    world.install_test_viewer_player_id(Some(9));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"self")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 100,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: Some(1700),
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 101,
        x: 2000,
        y: 2000,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: Some(1700),
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));

    assert_eq!(world.stats.remote_position_updates, 0);
    assert_eq!(world.stats.remote_snap_count, 0);
    assert_eq!(
        world
            .players
            .get(&9)
            .and_then(NetworkPlayer::display_position),
        Some(vec2(2000.0, 2000.0)),
        "self authority snapshots should still update the stored player state"
    );
}

#[test]
fn network_world_smooths_remote_player_display_orientation_across_wrap() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 38,
        timestamp: 100,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 2,
        timestamp: 104,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));

    assert!(
        world
            .players
            .get(&9)
            .is_some_and(|player| (player.display_orientation() - 0.95).abs() < 0.001)
    );

    // The short path from 0.95 to 0.05 wraps forward through 1.0, so every sampled
    // orientation stays in the arc (0.95, 1.0) ∪ [0.0, 0.05] and advances monotonically
    // in that wrapped sense (measured as forward distance traveled from 0.95).
    let wrapped_progress = |orientation: f32| {
        // Forward distance from the 0.95 start along the wrapping arc, in [0.0, 0.1].
        (orientation - 0.95).rem_euclid(1.0)
    };
    let on_short_arc = |orientation: f32| orientation > 0.95 || orientation <= 0.05 + 1e-4;

    let mut prev_progress = 0.0_f32;
    for (dt, label) in [
        (0.02, "frame 1"),
        (0.02, "frame 2"),
        (0.02, "frame 3"),
        (0.03, "frame 4"),
        (0.03, "frame 5"),
    ] {
        world.update_remote_players(dt, None);
        let orientation = world
            .players
            .get(&9)
            .expect("remote player should exist")
            .display_orientation();
        assert!(
            on_short_arc(orientation),
            "orientation should stay on the short wrapped arc at {label}, got {orientation}"
        );
        let progress = wrapped_progress(orientation);
        assert!(
            progress >= prev_progress - 1e-4 && progress <= 0.1 + 1e-3,
            "orientation should advance monotonically along the wrapped short path at {label}, got progress {progress} after {prev_progress}"
        );
        prev_progress = progress;
    }

    world.update_remote_players(0.02, None);
    assert!(
        world
            .players
            .get(&9)
            .is_some_and(|player| (player.display_orientation() - 0.05).abs() < 0.001)
    );
}

#[test]
fn network_world_drops_stale_remote_player_positions() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 100,
        x: 100,
        y: 200,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 99,
        x: 900,
        y: 900,
        velocity_x: 500,
        velocity_y: 500,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::BatchedLargePlayerPosition(
        PlayerPositionUpdate {
            player_id: 9,
            direction: 0,
            timestamp: 100,
            x: 901,
            y: 901,
            velocity_x: 500,
            velocity_y: 500,
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
    ));

    let remote = world.players.get(&9).expect("remote player should exist");
    assert_eq!(remote.position, Some(vec2(100.0, 200.0)));
    assert_eq!(remote.velocity, Vec2::ZERO);
    assert_eq!(remote.last_server_position_tick, Some(100));
    assert_eq!(world.stats.remote_position_updates, 3);
    assert_eq!(world.stats.remote_position_stale_drops, 2);
}

#[test]
fn network_world_keeps_duplicate_player_names_by_player_id() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(10, b"child")).unwrap(),
    ));
    world.apply_event(&ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
        requester_id: 10,
        destination_id: Some(9),
    }));

    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(12, b"remote")).unwrap(),
    ));

    assert_eq!(
        world.players.get(&9).map(|player| player.name.as_str()),
        Some("remote")
    );
    assert!(world.players.contains_key(&12));
    let child = world.players.get(&10).expect("child should remain");
    assert_eq!(child.attach_parent, 9);
}

#[test]
fn network_world_applies_batched_position_togglable_reference_rules() {
    let mut world = NetworkWorld::default();
    let remote = PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(remote));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 1,
        x: 16,
        y: 16,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0b1100_0000,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::BatchedLargePlayerPosition(
        PlayerPositionUpdate {
            player_id: 9,
            direction: 1,
            timestamp: 2,
            x: 32,
            y: 32,
            velocity_x: 0,
            velocity_y: 0,
            togglables: 0b0010_1010,
            ping: 0,
            bounty: 0,
            weapon: 0,
            checksum: None,
            energy: None,
            s2c_latency: None,
            flag_timer: None,
            items: None,
        },
    ));
    assert_eq!(
        world.players.get(&9).map(|player| player.togglables),
        Some(0b1110_1010)
    );

    world.apply_event(&ClientProtocolEvent::BatchedSmallPlayerPosition(
        PlayerPositionUpdate {
            player_id: 9,
            direction: 2,
            timestamp: 3,
            x: 48,
            y: 48,
            velocity_x: 0,
            velocity_y: 0,
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
    ));
    let remote = world.players.get(&9).expect("remote should remain");
    assert_eq!(remote.togglables, 0b1110_1010);
    assert_eq!(remote.position, Some(vec2(48.0, 48.0)));
    assert_eq!(remote.orientation, 0.05);
}

#[test]
fn network_world_converts_flash_status_into_warp_effects() {
    let mut world = NetworkWorld::default();
    let remote = PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(remote));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 1,
        x: 16,
        y: 24,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 2,
        x: 32,
        y: 40,
        velocity_x: 0,
        velocity_y: 0,
        togglables: STATUS_FLASH,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    assert_eq!(world.pending_warp_effects, vec![vec2(16.0, 24.0)]);
    assert_eq!(
        world.players.get(&9).map(|player| player.togglables),
        Some(0)
    );
    assert_eq!(
        world.drain_pending_warp_effects().collect::<Vec<_>>(),
        vec![vec2(16.0, 24.0)]
    );

    world.players.get_mut(&9).unwrap().togglables |= STATUS_FLASH;
    world.apply_event(&ClientProtocolEvent::BatchedSmallPlayerPosition(
        PlayerPositionUpdate {
            player_id: 9,
            direction: 0,
            timestamp: 3,
            x: 48,
            y: 56,
            velocity_x: 0,
            velocity_y: 0,
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
    ));
    assert_eq!(world.pending_warp_effects, vec![vec2(32.0, 40.0)]);
}

#[test]
fn network_world_applies_player_deaths_to_remote_state() {
    let mut world = NetworkWorld::default();
    let killer = PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"killer")).unwrap();
    let killed = PlayerRosterEntry::parse(&sample_player_enter_packet(10, b"killed")).unwrap();
    let child = PlayerRosterEntry::parse(&sample_player_enter_packet(11, b"child")).unwrap();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(killer));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(killed));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(child));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 10,
        direction: 0,
        timestamp: 1,
        x: 100,
        y: 200,
        velocity_x: 15,
        velocity_y: -25,
        togglables: 0,
        ping: 0,
        bounty: 44,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: Some(88),
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
        requester_id: 10,
        destination_id: Some(9),
    }));
    world.apply_event(&ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
        requester_id: 11,
        destination_id: Some(10),
    }));

    world.apply_event(&ClientProtocolEvent::PlayerDeath(PlayerDeath {
        green_id: 3,
        green_collectible_id: 0,
        killer_id: 9,
        killed_id: 10,
        bounty: 44,
        flag_transfer: 2,
        life_seq: 1,
        respawn_at_tick: 100,
    }));

    let killed = world.players.get(&10).expect("killed player should remain");
    assert_eq!(killed.life_seq, Some(1));
    assert_eq!(killed.position, None);
    assert_eq!(killed.velocity, Vec2::ZERO);
    assert_eq!(killed.bounty, 44);
    assert_eq!(killed.flags, 0);
    assert_eq!(killed.flag_timer, None);
    assert_eq!(killed.attach_parent, INVALID_PLAYER_ID);
    assert_eq!(killed.losses, 10);
    let child = world.players.get(&11).expect("child should remain");
    assert_eq!(child.attach_parent, INVALID_PLAYER_ID);
    assert_eq!(child.position, None);
    assert_eq!(child.velocity, Vec2::ZERO);

    let killer = world.players.get(&9).expect("killer should remain");
    assert_eq!(killer.wins, 9);
    assert_eq!(killer.flags, 4);
    assert!(world.visible_players().all(|player| player.player_id != 10));
}

#[test]
fn network_world_triggers_local_lvz_kill_and_death_modes() {
    let kill_object = LvzObjectDefinition {
        map_object: false,
        object_id: 50,
        x_map: 0,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 25,
        display_mode: LVZ_DISPLAY_KILL,
    };
    let death_object = LvzObjectDefinition {
        object_id: 51,
        display_mode: LVZ_DISPLAY_DEATH,
        ..kill_object.clone()
    };
    let mut world = NetworkWorld::default();

    world.apply_event(&ClientProtocolEvent::PlayerIdAssigned(7));
    world.apply_event(&ClientProtocolEvent::PlayerDeath(PlayerDeath {
        green_id: 0,
        green_collectible_id: 0,
        killer_id: 7,
        killed_id: 9,
        bounty: 0,
        flag_transfer: 0,
        life_seq: 1,
        respawn_at_tick: 100,
    }));
    assert!(world.lvz.object_is_active(&kill_object));
    assert!(!world.lvz.object_is_active(&death_object));

    world.apply_event(&ClientProtocolEvent::PlayerDeath(PlayerDeath {
        green_id: 0,
        green_collectible_id: 0,
        killer_id: 9,
        killed_id: 7,
        bounty: 0,
        flag_transfer: 0,
        life_seq: 1,
        respawn_at_tick: 100,
    }));
    assert!(world.lvz.object_is_active(&death_object));

    world
        .lvz
        .update_with_objects(2.5, [&kill_object, &death_object]);
    assert!(!world.lvz.object_is_active(&kill_object));
    assert!(!world.lvz.object_is_active(&death_object));
}

#[test]
fn network_world_tracks_server_flag_state() {
    let mut world = NetworkWorld::default();
    let carrier = PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"carrier")).unwrap();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(carrier));
    world.apply_event(&ClientProtocolEvent::FlagPosition(FlagPosition {
        flag_id: 3,
        x: 44,
        y: 55,
        owner: 2,
    }));

    assert_eq!(
        world.flags.get(&3),
        Some(&NetworkFlag {
            flag_id: 3,
            owner: 2,
            position: vec2(44.0, 55.0),
            dropped: true,
            turf: false,
            carried_by: None,
        })
    );
    assert_eq!(
        world.pickable_flag_id_near(vec2(45.0, 55.0), 4.0, 7),
        Some(3)
    );
    assert_eq!(world.pickable_flag_id_near(vec2(80.0, 55.0), 4.0, 7), None);
    assert_eq!(world.pickable_flag_id_near(vec2(45.0, 55.0), 4.0, 2), None);

    world.apply_event(&ClientProtocolEvent::FlagClaim(FlagClaim {
        flag_id: 3,
        player_id: 9,
    }));
    assert!(matches!(
        world.flags.get(&3),
        Some(flag) if !flag.dropped && flag.carried_by == Some(9)
    ));
    assert_eq!(world.pickable_flag_id_near(vec2(45.0, 55.0), 4.0, 7), None);
    assert_eq!(world.players.get(&9).map(|player| player.flags), Some(3));
    world.players.get_mut(&9).unwrap().flag_timer = Some(222);

    world.apply_event(&ClientProtocolEvent::DropFlag(DropFlag { player_id: 9 }));
    assert_eq!(world.players.get(&9).map(|player| player.flags), Some(0));
    assert_eq!(
        world.players.get(&9).and_then(|player| player.flag_timer),
        None
    );
    assert!(matches!(
        world.flags.get(&3),
        Some(flag) if flag.carried_by.is_none()
    ));

    world.apply_event(&ClientProtocolEvent::TurfFlagUpdate(TurfFlagUpdate {
        owners: vec![7, 8],
    }));
    world.apply_turf_flag_positions(&[vec2(100.0, 200.0), vec2(300.0, 400.0)]);
    assert!(matches!(
        world.flags.get(&0),
        Some(flag) if flag.owner == 7 && flag.position == vec2(100.0, 200.0) && flag.dropped && flag.turf
    ));
    assert!(matches!(
        world.flags.get(&1),
        Some(flag) if flag.owner == 8 && flag.position == vec2(300.0, 400.0) && flag.dropped && flag.turf
    ));

    world.apply_event(&ClientProtocolEvent::FlagClaim(FlagClaim {
        flag_id: 0,
        player_id: 9,
    }));
    assert!(matches!(
        world.flags.get(&0),
        Some(flag) if flag.owner == 7 && flag.dropped && flag.turf && flag.carried_by.is_none()
    ));
    assert_eq!(world.players.get(&9).map(|player| player.flags), Some(0));

    world.apply_event(&ClientProtocolEvent::FlagVictory(FlagVictory {
        team: 7,
        jackpot_points: 123_456,
    }));
    assert_eq!(
        world.last_flag_victory,
        Some(FlagVictory {
            team: 7,
            jackpot_points: 123_456,
        })
    );

    world.apply_event(&ClientProtocolEvent::FlagReward(FlagReward {
        grants: vec![FlagRewardGrant {
            team: 7,
            points: 44,
        }],
    }));
    assert_eq!(
        world.last_flag_rewards,
        vec![FlagRewardGrant {
            team: 7,
            points: 44,
        }]
    );
}

#[test]
fn a_released_flag_becomes_pickable_again() {
    let mut world = NetworkWorld::default();
    let carrier = PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"carrier")).unwrap();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(carrier));
    world.apply_event(&ClientProtocolEvent::FlagPosition(FlagPosition {
        flag_id: 3,
        x: 44,
        y: 55,
        owner: 2,
    }));
    assert_eq!(
        world.pickable_flag_id_near(vec2(45.0, 55.0), 4.0, 7),
        Some(3)
    );

    world.apply_event(&ClientProtocolEvent::FlagClaim(FlagClaim {
        flag_id: 3,
        player_id: 9,
    }));
    assert_eq!(world.pickable_flag_id_near(vec2(45.0, 55.0), 4.0, 7), None);

    // The server put the flag back on the map. A client that keeps it flagged as
    // carried can never claim it again, which is what strands an authored flag
    // after its first carrier releases it.
    world.apply_event(&ClientProtocolEvent::DropFlag(DropFlag { player_id: 9 }));
    assert!(matches!(
        world.flags.get(&3),
        Some(flag) if flag.carried_by.is_none() && flag.dropped
    ));
    assert_eq!(
        world.pickable_flag_id_near(vec2(45.0, 55.0), 4.0, 7),
        Some(3),
        "a released flag must be claimable again",
    );
}

#[test]
fn network_world_applies_frequency_ship_join_and_chat_events() {
    let mut world = NetworkWorld::default();
    let entry = PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(entry));
    world.apply_event(&ClientProtocolEvent::JoinedArena(1));
    world.apply_event(&ClientProtocolEvent::UfoToggled(ToggleUfoPacket {
        available: true,
    }));
    world.apply_event(&ClientProtocolEvent::PersonalKothTimerSet(
        SetPersonalKothTimer { timer: 12_345 },
    ));
    world.apply_event(&ClientProtocolEvent::KothGameReset(KothGameReset {
        adding_timer: true,
        timer: 22_222,
        player_id: 9,
    }));
    world.apply_event(&ClientProtocolEvent::KothTimeAdded(AddKothTime {
        additional_time: 333,
    }));
    world.apply_event(&ClientProtocolEvent::ArenaDirectory(
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
        },
    ));
    world.apply_event(&ClientProtocolEvent::ZoneBannerAdvertised(
        ZoneBannerAdvertisement {
            display_mode: 1,
            width: 2,
            height: 2,
            duration: 1_500,
            pixels: vec![1, 2, 3, 4],
        },
    ));
    world.apply_event(&ClientProtocolEvent::PlayerVoice(PlayerVoice {
        player_id: 9,
        wav_data: b"RIFF".to_vec(),
    }));
    world.apply_event(&ClientProtocolEvent::FileTransferred(ServerFileTransfer {
        filename: "news.txt".to_string(),
        data: b"hello".to_vec(),
    }));
    world.apply_event(&ClientProtocolEvent::ClientFileRequested(
        ClientFileRequest {
            local_filename: "profile.dat".to_string(),
            remote_filename: "upload.bin".to_string(),
        },
    ));
    world.apply_event(&ClientProtocolEvent::SpeedGameOver(SpeedGameOver {
        best: true,
        rank: 3,
        score: 12_345,
        player_scores: [1000, 1001, 1002, 1003, 1004],
        player_ids: [20, 21, 22, 23, 24],
    }));
    world.apply_event(&ClientProtocolEvent::FrequencyChanged(FrequencyChange {
        player_id: 9,
        frequency: 12,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 1,
        x: 1,
        y: 1,
        velocity_x: 50,
        velocity_y: 60,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    let mut banner = [0; PLAYER_BANNER_BYTES];
    banner[0] = 12;
    banner[95] = 34;
    world.apply_event(&ClientProtocolEvent::PersonalBannerChanged(
        PersonalBannerChange { banner },
    ));
    world.apply_event(&ClientProtocolEvent::PlayerBannerChanged(
        PlayerBannerChange {
            player_id: 9,
            banner,
        },
    ));
    world.apply_event(&ClientProtocolEvent::ShipFrequencyChanged(
        ShipFrequencyChange {
            ship: 4,
            player_id: 9,
            frequency: 13,
            life_seq: Some(17),
        },
    ));
    world.apply_event(&ClientProtocolEvent::Chat(ChatPacket {
        chat_type: 2,
        sound: 3,
        sender_id: 9,
        message: "hello".to_string(),
    }));

    assert_eq!(world.snapshot_epoch(), Some(1));
    assert!(world.ufo_available);
    assert_eq!(world.personal_koth_timer, Some(12_345));
    assert!(world.koth_timer_adding);
    assert_eq!(world.koth_timer, Some(22_555));
    assert_eq!(world.koth_player_id, Some(9));
    assert_eq!(
        world.arena_directory,
        vec![
            ArenaDirectoryEntry {
                name: "Public 0".to_string(),
                population: 42,
            },
            ArenaDirectoryEntry {
                name: "duel".to_string(),
                population: 3,
            },
        ]
    );
    assert_eq!(
        world.last_zone_banner,
        Some(ZoneBannerAdvertisement {
            display_mode: 1,
            width: 2,
            height: 2,
            duration: 1_500,
            pixels: vec![1, 2, 3, 4],
        })
    );
    assert_eq!(
        world.voice_messages,
        vec![PlayerVoice {
            player_id: 9,
            wav_data: b"RIFF".to_vec(),
        }]
    );
    assert_eq!(
        world.transferred_files,
        vec![ServerFileTransfer {
            filename: "news.txt".to_string(),
            data: b"hello".to_vec(),
        }]
    );
    assert_eq!(
        world.client_file_requests,
        vec![ClientFileRequest {
            local_filename: "profile.dat".to_string(),
            remote_filename: "upload.bin".to_string(),
        }]
    );
    assert_eq!(
        world.last_speed_game_over,
        Some(SpeedGameOver {
            best: true,
            rank: 3,
            score: 12_345,
            player_scores: [1000, 1001, 1002, 1003, 1004],
            player_ids: [20, 21, 22, 23, 24],
        })
    );
    assert_eq!(world.personal_banner, Some(banner));
    let player = world.players.get(&9).expect("player should exist");
    assert_eq!(player.frequency, 13);
    assert_eq!(player.ship, 4);
    assert_eq!(player.life_seq, Some(17));
    assert_eq!(player.velocity, Vec2::ZERO);
    assert_eq!(player.flags, 0);
    assert_eq!(player.banner, Some(banner));
    assert_eq!(
        world.chat,
        vec![NetworkChatEntry {
            chat_type: 2,
            sound: 3,
            sender_id: 9,
            sender_name: Some("remote".to_string()),
            message: "hello".to_string(),
        }]
    );
}

#[test]
fn network_world_ticks_koth_timers_and_reports_personal_expiration() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PersonalKothTimerSet(
        SetPersonalKothTimer { timer: 125 },
    ));
    world.apply_event(&ClientProtocolEvent::KothGameReset(KothGameReset {
        adding_timer: false,
        timer: 250,
        player_id: 9,
    }));

    assert!(!world.update_koth_timers(0.75));
    assert_eq!(world.personal_koth_timer, Some(50));
    assert_eq!(world.koth_timer, Some(175));

    assert!(world.update_koth_timers(0.50));
    assert_eq!(world.personal_koth_timer, None);
    assert_eq!(world.koth_timer, Some(125));

    assert!(!world.update_koth_timers(10.0));
    assert_eq!(world.koth_timer, Some(0));
}

#[test]
fn network_world_detaches_turrets_on_frequency_and_ship_changes() {
    let mut world = NetworkWorld::default();
    for (id, name) in [(9, b"ship".as_slice()), (10, b"parent"), (11, b"child")] {
        world.apply_event(&ClientProtocolEvent::PlayerEntered(
            PlayerRosterEntry::parse(&sample_player_enter_packet(id, name)).unwrap(),
        ));
    }
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 10,
        direction: 0,
        timestamp: 1,
        x: 44,
        y: 55,
        velocity_x: 30,
        velocity_y: 40,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
        requester_id: 9,
        destination_id: Some(10),
    }));
    world.apply_event(&ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
        requester_id: 11,
        destination_id: Some(9),
    }));

    world.apply_event(&ClientProtocolEvent::FrequencyChanged(FrequencyChange {
        player_id: 9,
        frequency: 44,
    }));

    let player = world.players.get(&9).expect("player should remain");
    assert_eq!(player.frequency, 44);
    assert_eq!(player.attach_parent, INVALID_PLAYER_ID);
    assert_eq!(player.position, None);
    assert_eq!(player.velocity, Vec2::ZERO);
    assert_eq!(player.flags, 0);
    let child = world.players.get(&11).expect("child should remain");
    assert_eq!(child.attach_parent, INVALID_PLAYER_ID);
    assert_eq!(child.position, None);
    assert_eq!(child.velocity, Vec2::ZERO);

    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 2,
        x: 20,
        y: 30,
        velocity_x: 50,
        velocity_y: 60,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
        requester_id: 11,
        destination_id: Some(9),
    }));

    world.apply_event(&ClientProtocolEvent::ShipFrequencyChanged(
        ShipFrequencyChange {
            ship: 5,
            player_id: 9,
            frequency: 66,
            life_seq: None,
        },
    ));

    let player = world.players.get(&9).expect("player should remain");
    assert_eq!(player.ship, 5);
    assert_eq!(player.frequency, 66);
    assert_eq!(player.attach_parent, INVALID_PLAYER_ID);
    assert_eq!(player.position, Some(vec2(20.0, 30.0)));
    assert_eq!(player.velocity, Vec2::ZERO);
    assert_eq!(player.flags, 0);
    let child = world.players.get(&11).expect("child should remain");
    assert_eq!(child.attach_parent, INVALID_PLAYER_ID);
    assert_eq!(child.position, None);
    assert_eq!(child.velocity, Vec2::ZERO);
}

#[test]
fn network_world_accumulates_adding_koth_timer_with_fractional_dt() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::KothGameReset(KothGameReset {
        adding_timer: true,
        timer: 10,
        player_id: 9,
    }));

    assert!(!world.update_koth_timers(0.016));
    assert_eq!(world.koth_timer, Some(11));
    assert!(!world.update_koth_timers(0.004));
    assert_eq!(world.koth_timer, Some(12));
}

#[test]
fn network_world_tracks_turret_links() {
    let mut world = NetworkWorld::default();
    let parent = PlayerRosterEntry::parse(&sample_player_enter_packet(10, b"parent")).unwrap();
    let child = PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"turret")).unwrap();
    world.apply_event(&ClientProtocolEvent::PlayerEntered(parent));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(child));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 10,
        direction: 8,
        timestamp: 1,
        x: 44,
        y: 55,
        velocity_x: 30,
        velocity_y: 40,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 12,
        timestamp: 2,
        x: 1,
        y: 2,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }));

    world.apply_event(&ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
        requester_id: 9,
        destination_id: Some(10),
    }));

    let child = world.players.get(&9).expect("child should exist");
    assert_eq!(child.attach_parent, 10);
    assert_eq!(child.position, Some(vec2(44.0, 55.0)));
    assert_eq!(
        world
            .visible_players()
            .map(|player| player.player_id)
            .collect::<Vec<_>>(),
        vec![10]
    );
    assert_eq!(
        world
            .visible_attached_players(10)
            .map(|player| player.player_id)
            .collect::<Vec<_>>(),
        vec![9]
    );

    world.apply_event(&ClientProtocolEvent::TurretLinkDestroyed(
        DestroyTurretLink { player_id: 10 },
    ));
    let child = world.players.get(&9).expect("child should still exist");
    assert_eq!(child.attach_parent, INVALID_PLAYER_ID);
    assert_eq!(child.position, None);
}

/// The HUD's spectator carry countdown reads exactly one thing: `NetworkPlayer::flag_timer`. The server
/// now authors that field from the objective registry, so the client must carry it onto the entity and
/// must not keep lighting a phantom countdown after the server stops reporting one.
#[test]
fn a_carried_flag_countdown_lands_on_the_entity_and_clears_when_the_server_stops_reporting_one() {
    let mut world = NetworkWorld::default();
    world.apply_event(&ClientProtocolEvent::PlayerIdAssigned(7));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(
        PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"carrier")).unwrap(),
    ));

    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 1,
        x: 16,
        y: 16,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: Some(250),
        items: None,
    }));
    assert_eq!(
        world.players[&9].flag_timer,
        Some(250),
        "the countdown the server authored has to reach the entity the spectator HUD reads"
    );

    // The carry ended - dropped, expired, or the carrier died - and the server's next frame reports no
    // remaining time. A held 250 would keep the spectator indicator lit forever.
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 9,
        direction: 0,
        timestamp: 2,
        x: 32,
        y: 16,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: None,
        s2c_latency: None,
        flag_timer: Some(0),
        items: None,
    }));
    assert_eq!(
        world.players[&9].flag_timer,
        Some(0),
        "a server-reported zero must not leave the previous countdown on the entity"
    );
}
