use super::*;
use crate::model::OwnedProjectilePrediction;

fn bomb_fixture() -> Bomb {
    Bomb {
        prediction: Some(OwnedProjectilePrediction {
            activation: 17,
            life_seq: 2,
            input_sequence: Some(10),
            projectile_index: 0,
        }),
        pos: vec2(168.0, 168.0),
        vel: vec2(525.0, 0.0),
        frequency: 7,
        ttl: 10.0,
        age: 0.0,
        trail_timer: 0.0,
        bounces_remaining: 0,
        level: 3,
        damage: 100.0,
        emp_shutdown_time: 0.0,
        shrapnel: ExplosiveShrapnel::none(),
        proximity: false,
        proximity_sensor: None,
    }
}

fn remote_fixture(bomb: &Bomb, arena: ArenaTuning) -> RemoteProjectile {
    let ship = arena.ship(0);
    let source = RemoteWeaponSource::capture(
        RemoteWeaponSourceIdentity::authenticated(768, 5, 2).unwrap(),
        RemoteWeaponArenaScope::authenticated(9, 1),
        bomb.pos,
        Vec2::ZERO,
        0.0,
        bomb.frequency,
        0,
        ship,
        false,
    )
    .unwrap()
    .with_realtime_order(10, 20, 30)
    .unwrap();
    let mut remote = remote_weapon_projectiles(
        source,
        encode_weapon_data(3, bomb.level, false, 0, 0, false),
        ship,
        &arena,
    )[0];
    remote.pos = bomb.pos;
    remote.vel = bomb.vel;
    remote.age = bomb.age;
    remote.ttl = bomb.ttl;
    remote.bounces_remaining = bomb.bounces_remaining;
    remote
}

#[test]
fn bomb_render_tracks_camera_remainder_and_one_animation_clock_at_multiple_cadences() {
    let map = empty_test_map();
    let tuning = ArenaTuning::continuum_skirmish_default();
    for hz in [60, 120, 144, 240] {
        let mut bomb = bomb_fixture();
        let initial = bomb;
        let mut clock = crate::fixed_step::FixedGameplayClock::default();
        let mut elapsed = 0.0;
        let mut canonical_camera = bomb.pos;
        let mut raw_max_error: f32 = 0.0;
        let mut projected_max_error: f32 = 0.0;
        for frame in 0..(hz * 2) {
            // Alternating jitter also exercises zero-step/two-step draws at 60Hz.
            let render_dt = (if frame % 2 == 0 { 0.8 } else { 1.2 }) / hz as f32;
            elapsed += render_dt;
            for _ in 0..clock.advance(render_dt) {
                update_bomb_motion(
                    &mut bomb,
                    GAMEPLAY_STEP_SECONDS,
                    &map,
                    &[],
                    &[],
                    &NetworkWorld::default(),
                    7,
                    tuning,
                    None,
                );
                canonical_camera.x += 325.0 * GAMEPLAY_STEP_SECONDS;
            }
            let partial = clock.remainder_seconds() as f32;
            let camera = canonical_camera.x + 325.0 * partial;
            let pose = local_bomb_presentation(&bomb, partial, &map, &[], &tuning, None);
            let remote = remote_fixture(&bomb, tuning);
            let remote_pose = remote_projectile_presentation(&remote, partial, &map, &[], &tuning);
            let expected_screen = (initial.vel.x - 325.0) * elapsed;
            raw_max_error = raw_max_error.max((bomb.pos.x - camera - expected_screen).abs());
            projected_max_error =
                projected_max_error.max((pose.position.x - camera - expected_screen).abs());
            assert!((pose.animation_seconds - elapsed).abs() < 0.0001);
            assert!(pose.position.distance(remote_pose.position) < 0.001);
            assert!((pose.animation_seconds - remote_pose.animation_seconds).abs() < 0.0001);
        }
        println!(
            "bomb render {hz}Hz: raw_max_error_px={raw_max_error:.4}, projected_max_error_px={projected_max_error:.4}"
        );
        assert!(raw_max_error > 2.0);
        assert!(projected_max_error < 0.1);
    }
}

#[test]
fn bomb_render_is_bounded_by_one_step_and_remaining_lifetime_without_mutation() {
    let map = empty_test_map();
    let tuning = ArenaTuning::continuum_skirmish_default();
    let mut bomb = bomb_fixture();
    bomb.ttl = 0.004;
    let remote = remote_fixture(&bomb, tuning);
    let before = bomb;
    for partial in [-1.0, f32::NAN, f32::INFINITY, 0.0, 0.003, 10.0] {
        let local = local_bomb_presentation(&bomb, partial, &map, &[], &tuning, None);
        let other = remote_projectile_presentation(&remote, partial, &map, &[], &tuning);
        let dt = if partial.is_finite() {
            partial.clamp(0.0, 0.004)
        } else {
            0.0
        };
        let expected = bomb.pos + bomb.vel * dt;
        assert!(local.position.distance(expected) < 0.001);
        assert!(other.position.distance(expected) < 0.001);
        assert_eq!(bomb, before);
        assert_eq!(remote, remote_fixture(&before, tuning));
    }
    bomb.ttl = 10.0;
    let pose = local_bomb_presentation(&bomb, 100.0, &map, &[], &tuning, None);
    assert!((pose.position.x - bomb.pos.x - bomb.vel.x * GAMEPLAY_STEP_SECONDS).abs() < 0.001);
}

#[test]
fn bomb_render_respects_walls_doors_and_frequency_owned_bricks() {
    let tuning = ArenaTuning::continuum_skirmish_default();
    let mut map = empty_test_map();
    let mut bomb = bomb_fixture();
    bomb.pos.x = 175.0; // One pixel before tile 11.
    bomb.vel.x = 640.0;
    for tile in [1, DOOR_FIRST_TILE_ID, BRICK_TILE_ID] {
        map.set_tile(11, 10, tile);
        map.door_seed = u8::MAX;
        let bricks = [Brick {
            tile: ivec2(11, 10),
            previous_tile: 0,
            team: 8,
            ttl: 10.0,
            age: 0.0,
            server_timestamp: None,
        }];
        let remote = remote_fixture(&bomb, tuning);
        let local = local_bomb_presentation(&bomb, 0.016, &map, &bricks, &tuning, None);
        let other = remote_projectile_presentation(&remote, 0.016, &map, &bricks, &tuning);
        assert!(local.position.x < 176.0);
        assert!(other.position.x < 176.0);
        assert_eq!(
            bomb.ttl, 10.0,
            "predicted wall terminal cannot retire the actual bomb"
        );
        if tile == DOOR_FIRST_TILE_ID {
            map.door_seed = 0;
            let open = local_bomb_presentation(&bomb, 0.016, &map, &bricks, &tuning, None);
            let remote_open =
                remote_projectile_presentation(&remote, 0.016, &map, &bricks, &tuning);
            assert!(open.position.x > 176.0 && remote_open.position.x > 176.0);
        } else if tile == BRICK_TILE_ID {
            bomb.frequency = 8;
            let friendly = local_bomb_presentation(&bomb, 0.016, &map, &bricks, &tuning, None);
            let remote_friendly = remote_projectile_presentation(
                &remote_fixture(&bomb, tuning),
                0.016,
                &map,
                &bricks,
                &tuning,
            );
            assert!(friendly.position.x > 176.0 && remote_friendly.position.x > 176.0);
        }
    }
}

#[test]
fn bomb_render_bounce_is_disposable_and_relocation_has_no_cached_predecessor() {
    let tuning = ArenaTuning::continuum_skirmish_default();
    let mut map = empty_test_map();
    map.set_tile(11, 10, 1);
    let mut bomb = bomb_fixture();
    bomb.pos.x = 175.0;
    bomb.vel.x = 640.0;
    bomb.bounces_remaining = 1;
    let before = bomb;
    let projected = local_bomb_presentation(&bomb, 0.016, &map, &[], &tuning, None);
    let remote = remote_fixture(&bomb, tuning);
    let remote_pose = remote_projectile_presentation(&remote, 0.016, &map, &[], &tuning);
    assert!(projected.position.x < bomb.pos.x);
    assert_eq!(projected.bounces_remaining, 0);
    assert_eq!(projected, remote_pose);
    assert_eq!(bomb, before);
    assert_eq!(
        projected,
        local_bomb_presentation(&bomb, 0.016, &map, &[], &tuning, None)
    );
    bomb.pos = vec2(1000.0, 1000.0);
    bomb.prediction.as_mut().unwrap().life_seq += 1;
    let relocated = local_bomb_presentation(&bomb, 0.004, &map, &[], &tuning, None);
    assert!(relocated.position.distance(bomb.pos + bomb.vel * 0.004) < 0.001);
}

#[test]
fn bomb_and_mine_render_use_fire_time_gravity_while_guns_and_thor_do_not() {
    let tuning = ArenaTuning::continuum_skirmish_default();
    let mut map = empty_test_map();
    for y in 8..13 {
        for x in 8..13 {
            map.set_tile(x, y, WORMHOLE_TILE_ID);
        }
    }
    let mut bomb = bomb_fixture();
    bomb.pos = vec2(7.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    bomb.vel = Vec2::ZERO;
    let mut remote = remote_fixture(&bomb, tuning);
    let local = local_bomb_presentation(&bomb, 0.016, &map, &[], &tuning, Some(40));
    let no_gravity = remote_projectile_presentation(&remote, 0.016, &map, &[], &tuning);
    assert_eq!(no_gravity.position, bomb.pos);
    remote.source.bomb_gravity = Some(40);
    let affected = remote_projectile_presentation(&remote, 0.016, &map, &[], &tuning);
    assert!(affected.position.x > bomb.pos.x);
    assert_eq!(local.position, affected.position);
    remote.kind = RemoteProjectileKind::Mine { proximity: false };
    let mine = remote_projectile_presentation(&remote, 0.016, &map, &[], &tuning);
    assert_eq!(
        mine.position, affected.position,
        "alternate bombs share gravity"
    );
    let mut mine_position = remote.pos;
    let mut mine_velocity = remote.vel;
    assert_eq!(
        update_mine_terrain_motion(
            &mut mine_position,
            &mut mine_velocity,
            0.016,
            &map,
            &[],
            remote.frequency,
            remote.source.bomb_gravity
        ),
        (false, false)
    );
    assert_eq!(mine_position, mine.position);
    for kind in [
        RemoteProjectileKind::Bullet { bouncing: false },
        RemoteProjectileKind::Thor,
    ] {
        remote.kind = kind;
        let unchanged = remote_projectile_presentation(&remote, 0.016, &map, &[], &tuning);
        assert_eq!(unchanged.position, remote.pos);
        assert!((unchanged.animation_seconds - remote.age - 0.016).abs() < 0.0001);
    }
}

fn bullet_fixture() -> Bullet {
    Bullet {
        prediction: None,
        pos: vec2(168.0, 168.0),
        vel: vec2(525.0, 0.0),
        frequency: 7,
        ttl: 10.0,
        age: 0.0,
        trail_timer: 0.0,
        bounces_remaining: 0,
        bouncing: false,
        activates_on_wall: false,
        active: true,
        level: 3,
        damage: 100.0,
    }
}

#[test]
fn owned_and_enemy_bullets_track_camera_between_steps_at_all_render_cadences() {
    let map = empty_test_map();
    let tuning = ArenaTuning::continuum_skirmish_default();
    for hz in [60, 120, 144, 240] {
        let mut bullet = bullet_fixture();
        let initial = bullet;
        let mut remote = remote_fixture(&bomb_fixture(), tuning);
        remote.kind = RemoteProjectileKind::Bullet { bouncing: false };
        remote.pos = bullet.pos;
        remote.vel = bullet.vel;
        remote.age = bullet.age;
        let mut clock = crate::fixed_step::FixedGameplayClock::default();
        let mut elapsed = 0.0;
        let mut raw_error: f32 = 0.0;
        let mut projected_error: f32 = 0.0;
        for frame in 0..hz * 2 {
            let render_dt = (if frame % 2 == 0 { 0.8 } else { 1.2 }) / hz as f32;
            elapsed += render_dt;
            for _ in 0..clock.advance(render_dt) {
                update_local_bullet_motion(
                    &mut bullet,
                    GAMEPLAY_STEP_SECONDS,
                    &map,
                    &[],
                    &[],
                    &NetworkWorld::default(),
                    7,
                    tuning,
                );
                bullet.age += GAMEPLAY_STEP_SECONDS;
                bullet.ttl -= GAMEPLAY_STEP_SECONDS;
                remote.age += GAMEPLAY_STEP_SECONDS;
                update_remote_bullet_motion(
                    &mut remote,
                    GAMEPLAY_STEP_SECONDS,
                    &map,
                    &[],
                    Vec2::ZERO,
                    0.0,
                    false,
                );
                remote.ttl -= GAMEPLAY_STEP_SECONDS;
            }
            let partial = clock.remainder_seconds() as f32;
            let local = local_bullet_presentation(&bullet, partial, &map, &[], &tuning);
            let enemy = remote_projectile_presentation(&remote, partial, &map, &[], &tuning);
            let expected = initial.pos + initial.vel * elapsed;
            raw_error = raw_error.max((bullet.pos - expected).length());
            projected_error = projected_error.max((local.position - expected).length());
            assert!(local.position.distance(enemy.position) < 0.001);
            assert!((local.animation_seconds - elapsed).abs() < 0.0001);
            assert!((enemy.animation_seconds - elapsed).abs() < 0.0001);
        }
        println!(
            "bullet render {hz}Hz: raw_error_px={raw_error:.4}, projected_error_px={projected_error:.4}"
        );
        assert!(raw_error > 2.0);
        assert!(projected_error < 0.1);
    }
}

#[test]
fn moving_weapon_projection_clamps_terrain_ttl_and_preserves_canonical_state() {
    let tuning = ArenaTuning::continuum_skirmish_default();
    let mut map = empty_test_map();
    let mut bullet = bullet_fixture();
    bullet.pos.x = 175.0;
    bullet.vel.x = 640.0;
    let mut remote = remote_fixture(&bomb_fixture(), tuning);
    remote.kind = RemoteProjectileKind::Bullet { bouncing: false };
    remote.pos = bullet.pos;
    remote.vel = bullet.vel;
    for tile in [1, DOOR_FIRST_TILE_ID, BRICK_TILE_ID] {
        map.set_tile(11, 10, tile);
        map.door_seed = u8::MAX;
        let bricks = [Brick {
            tile: ivec2(11, 10),
            previous_tile: 0,
            team: 8,
            ttl: 10.0,
            age: 0.0,
            server_timestamp: None,
        }];
        for partial in [-1.0, f32::NAN, f32::INFINITY, 0.0, 0.004, 0.016, 1.0] {
            let before = bullet;
            let remote_before = remote;
            let local = local_bullet_presentation(&bullet, partial, &map, &bricks, &tuning);
            let enemy = remote_projectile_presentation(&remote, partial, &map, &bricks, &tuning);
            assert!(local.position.x < 176.0);
            assert!(enemy.position.x < 176.0);
            assert_eq!(bullet, before);
            assert_eq!(remote, remote_before);
        }
        if tile == DOOR_FIRST_TILE_ID {
            map.door_seed = 0;
        }
        if tile == BRICK_TILE_ID {
            bullet.frequency = 8;
            remote.frequency = 8;
        }
        if tile != 1 {
            assert!(
                local_bullet_presentation(&bullet, 0.016, &map, &bricks, &tuning)
                    .position
                    .x
                    > 176.0
            );
            assert!(
                remote_projectile_presentation(&remote, 0.016, &map, &bricks, &tuning)
                    .position
                    .x
                    > 176.0
            );
        }
    }
    map.tiles.fill(0);
    bullet.ttl = 0.003;
    assert!(
        (local_bullet_presentation(&bullet, 1.0, &map, &[], &tuning)
            .position
            .x
            - bullet.pos.x
            - bullet.vel.x * 0.003)
            .abs()
            < 0.001
    );
    remote.ttl = bullet.ttl;
    assert!(
        (remote_projectile_presentation(&remote, 1.0, &map, &[], &tuning)
            .position
            .x
            - remote.pos.x
            - remote.vel.x * 0.003)
            .abs()
            < 0.001
    );
}

#[test]
fn burst_projection_activates_at_wall_without_mutating_or_retiring_the_weapon() {
    let tuning = ArenaTuning::continuum_skirmish_default();
    let mut map = empty_test_map();
    map.set_tile(11, 10, 1);
    let mut bullet = bullet_fixture();
    bullet.pos.x = 175.0;
    bullet.vel.x = 640.0;
    bullet.activates_on_wall = true;
    bullet.active = false;
    let mut remote = remote_fixture(&bomb_fixture(), tuning);
    remote.kind = RemoteProjectileKind::Burst { active: false };
    remote.pos = bullet.pos;
    remote.vel = bullet.vel;
    let local = local_bullet_presentation(&bullet, 0.016, &map, &[], &tuning);
    let enemy = remote_projectile_presentation(&remote, 0.016, &map, &[], &tuning);
    assert!(local.burst_active && enemy.burst_active);
    assert!(!bullet.active);
    assert_eq!(remote.kind, RemoteProjectileKind::Burst { active: false });
    assert_eq!(bullet.ttl, 10.0);
    assert_eq!(remote.ttl, 10.0);
}

#[test]
fn shared_weapon_trajectory_matches_reference_bounce_and_gravity_oracles() {
    for scenario in ["clear", "bounce", "gravity", "mine_gravity"] {
        let tuning = ArenaTuning::continuum_skirmish_default();
        let mut map = empty_test_map();
        let mut bomb = bomb_fixture();
        bomb.bounces_remaining = 2;
        if scenario == "bounce" {
            bomb.pos.x = 175.0;
            bomb.vel.x = 640.0;
            map.set_tile(11, 10, 1);
        }
        if matches!(scenario, "gravity" | "mine_gravity") {
            bomb.pos.x = 120.0;
            bomb.vel.x = if scenario == "mine_gravity" {
                0.0
            } else {
                100.0
            };
            for y in 8..13 {
                for x in 8..13 {
                    map.set_tile(x, y, WORMHOLE_TILE_ID);
                }
            }
        }
        let mut client = remote_fixture(&bomb, tuning);
        client.source.bomb_gravity = matches!(scenario, "gravity" | "mine_gravity").then_some(1000);
        if scenario == "mine_gravity" {
            client.kind = RemoteProjectileKind::Mine { proximity: false };
            client.weapon_data |= 0x8000;
        }
        let source = crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: 768,
            source_session_id: 5,
            source_life_seq: 2,
            shot_group_id: 30,
            x: bomb.pos.x,
            y: bomb.pos.y,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.0,
            frequency: 7,
            ship: 0,
            weapon_data: client.weapon_data,
        };
        let record = crate::authority_projectiles::ProjectileRecord {
            id: 1,
            shot_group_id: 30,
            shot_index: 0,
            presentation_source: Some(source),
            owner_id: 768,
            owner_life_seq: 2,
            combat_timeline: None,
            weapon_data: client.weapon_data,
            x: bomb.pos.x as u16,
            y: bomb.pos.y as u16,
            timestamp: 100,
            created_tick: 100,
            velocity_x: (bomb.vel.x * 10.0) as i16,
            velocity_y: (bomb.vel.y * 10.0) as i16,
            source_server_tick: None,
            precise_velocity: Some(crate::gameplay_rules::RulesVector::new(
                bomb.vel.x, bomb.vel.y,
            )),
            expiry_tick: Some(1000),
            pos_x: bomb.pos.x,
            pos_y: bomb.pos.y,
            bounces_remaining: 2,
            behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        };
        let mut server = crate::authority_projectiles::ProjectileRegistry::default();
        server.step(100, |_, _| false, &[]);
        server.record(record, 128, 1024);
        server.step_with_environment_transactional(
            102,
            |_, position, dt| {
                let old = vec2(bomb.vel.x, bomb.vel.y);
                let next = remote_bomb_wormhole_gravity_velocity(
                    &map,
                    vec2(position.x, position.y),
                    old,
                    client.source.bomb_gravity,
                    dt,
                );
                crate::gameplay_rules::RulesVector::new(next.x - old.x, next.y - old.y)
            },
            |x, y| weapon_times_out_on_wormhole_tile(&map, vec2(x, y)),
            |_, x, y| is_solid_world_for_frequency(&map, vec2(x, y), 7, &[]),
            &[],
            0.0,
            0,
            0.0,
            |_, _, _| crate::combat_timeline::ProjectilePresentationWitnessV1::not_evaluated(),
            |_| true,
        );
        client.age += 0.02;
        let gravity = client.source.bomb_gravity;
        update_remote_bomb_motion(
            &mut client,
            0.02,
            &map,
            &[],
            Vec2::ZERO,
            0.0,
            tuning,
            gravity,
            false,
        );
        let server = server.last().unwrap();
        let expected_x = match scenario {
            "clear" => 178.5,
            "bounce" => 168.6,
            "gravity" => 122.936,
            "mine_gravity" => 120.936,
            _ => unreachable!(),
        };
        let expected_vx = match scenario {
            "clear" => 525.0,
            "bounce" => -640.0,
            "gravity" => 162.4,
            "mine_gravity" => 62.4,
            _ => unreachable!(),
        };
        assert!(
            (server.pos_x - expected_x).abs() < 0.001,
            "{scenario}: server position {}",
            server.pos_x
        );
        assert!(
            (server.motion_velocity().x - expected_vx).abs() < 0.001,
            "{scenario}: server velocity {:?}",
            server.motion_velocity()
        );
        assert!(
            vec2(server.pos_x, server.pos_y).distance(client.pos) < 0.001,
            "{scenario}"
        );
        assert!(
            (server.motion_velocity().x - client.vel.x).abs() < 0.001,
            "{scenario}"
        );
        assert_eq!(server.bounces_remaining, client.bounces_remaining);
    }
}

#[test]
fn received_weapon_fraction_uses_its_server_clock_at_high_refresh_without_ship_remainder() {
    let tuning = ArenaTuning::continuum_skirmish_default();
    let map = empty_test_map();
    for hz in [60_u32, 120, 144, 240] {
        let bomb = bomb_fixture();
        let mut remote = remote_fixture(&bomb, tuning);
        let initial = remote;
        remote.bind_presentation_clock(100);
        for frame in 1..=hz * 2 {
            let elapsed = frame as f64 / hz as f64;
            let ticks = (elapsed * 100.0).floor() as u32;
            let fraction = (elapsed - ticks as f64 / 100.0) as f32;
            // Oracle uses analytic straight flight, independent of our stepper.
            remote.pos = initial.pos + initial.vel * (ticks as f32 / 100.0);
            remote.age = ticks as f32 / 100.0;
            let clock = remote.presentation_clock.as_mut().unwrap();
            clock.pose_tick = 100 + ticks;
            clock.render_partial_seconds = fraction;
            let before = remote;
            let pose = remote_projectile_presentation(
                &remote,
                GAMEPLAY_STEP_SECONDS * 0.9,
                &map,
                &[],
                &tuning,
            );
            assert!(
                pose.position
                    .distance(initial.pos + initial.vel * elapsed as f32)
                    < 0.001,
                "{hz} Hz frame {frame}"
            );
            assert!((pose.animation_seconds - elapsed as f32).abs() < 0.0001);
            assert_eq!(remote, before);
        }
    }
}
