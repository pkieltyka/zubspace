use super::*;

fn remote_source(tuning: ShipTuning, pos: Vec2, vel: Vec2, orientation: f32) -> RemoteWeaponSource {
    RemoteWeaponSource::capture(
        RemoteWeaponSourceIdentity::authenticated(9, u64::MAX, u32::MAX).unwrap(),
        RemoteWeaponArenaScope::authenticated(u64::MAX, u64::MAX),
        pos,
        vel,
        orientation,
        7,
        0,
        tuning,
        true,
    )
    .unwrap()
}

#[test]
fn remote_weapon_projectiles_spawn_visual_only_shapes() {
    let arena = ArenaTuning::continuum_skirmish_default();
    let tuning = ShipTuning::continuum_skirmish_default();
    let pos = vec2(100.0, 200.0);
    let vel = vec2(10.0, -20.0);

    let source = remote_source(tuning, pos, vel, 0.25);
    let bullet = remote_weapon_projectiles(source, 1 | (2 << 5), tuning, &arena);
    assert_eq!(bullet.len(), 1);
    assert_eq!(
        bullet[0],
        RemoteProjectile {
            source,
            source_projectile_index: 0,
            authority_binding: None,
            presentation_clock: None,
            last_repel_mutation_event_id: None,
            authoritative_repel_state: None,
            frequency: 7,
            weapon_data: 1 | (2 << 5),
            kind: RemoteProjectileKind::Bullet { bouncing: false },
            pos,
            vel: vel + vec2(1.0, 0.0) * tuning.bullet_speed,
            ttl: weapon_lifetime(&arena, WeaponLifetimeKind::Bullet),
            age: 0.0,
            trail_timer: 0.0,
            bounces_remaining: 0,
            proximity_sensor: None,
            level: 3,
            damage: leveled_weapon_damage(arena.bullet_damage, arena.bullet_damage_upgrade, 3),
            inactive_damage: 0.0,
            radius: 0.0,
            emp_shutdown_time: 0.0,
            shrapnel_rng_seed: 0,
        }
    );
    let bouncing_bullet = remote_weapon_projectiles(source, 2 | (2 << 5), tuning, &arena);
    assert_eq!(
        bouncing_bullet[0].kind,
        RemoteProjectileKind::Bullet { bouncing: true }
    );
    assert_eq!(bouncing_bullet[0].bounces_remaining, 0);

    let multifire = remote_weapon_projectiles(
        remote_source(tuning, pos, vel, 0.0),
        1 | (1 << 15),
        tuning,
        &arena,
    );
    assert_eq!(multifire.len(), MULTIFIRE_BULLET_COUNT);
    for projectile in &multifire {
        assert!(((projectile.vel - vel).length() - tuning.bullet_speed).abs() < 0.001);
    }

    let mut double_tuning = tuning;
    double_tuning.double_barrel = true;
    let double_bullet = remote_weapon_projectiles(
        remote_source(double_tuning, pos, vel, 0.25),
        1,
        double_tuning,
        &arena,
    );
    assert_eq!(double_bullet.len(), 2);
    // The reference game truncates the packet-space barrel offset to integer pixels.
    let double_offset = 10.0;
    assert_eq!(double_bullet[0].pos, vec2(100.0, 200.0 - double_offset));
    assert_eq!(double_bullet[1].pos, vec2(100.0, 200.0 + double_offset));

    let double_multifire = remote_weapon_projectiles(
        remote_source(double_tuning, pos, vel, 0.0),
        1 | (1 << 15),
        double_tuning,
        &arena,
    );
    assert_eq!(double_multifire.len(), 4);

    let burst = remote_weapon_projectiles(remote_source(tuning, pos, vel, 0.0), 7, tuning, &arena);
    assert_eq!(burst.len(), tuning.burst_bullet_count);
    assert_eq!(burst[0].kind, RemoteProjectileKind::Burst { active: false });
    assert_eq!(burst[0].pos, pos);
    assert!((burst[0].vel - vec2(0.0, -tuning.burst_speed)).length() < 0.001);
    assert_eq!(burst[0].bounces_remaining, 0);
    assert_eq!(
        burst[0].ttl,
        weapon_lifetime(&arena, WeaponLifetimeKind::Burst)
    );

    let mine = remote_weapon_projectiles(source, 3 | (1 << 15), tuning, &arena);
    assert_eq!(
        mine[0].kind,
        RemoteProjectileKind::Mine { proximity: false }
    );
    assert_eq!(mine[0].frequency, 7);
    assert_eq!(mine[0].pos, pos);

    let proximity_mine = remote_weapon_projectiles(source, 4 | (1 << 15), tuning, &arena);
    assert_eq!(
        proximity_mine[0].kind,
        RemoteProjectileKind::Mine { proximity: true }
    );

    let bomb = remote_weapon_projectiles(source, 3, tuning, &arena);
    assert_eq!(
        bomb[0].kind,
        RemoteProjectileKind::Bomb { proximity: false }
    );
    assert_eq!(bomb[0].pos, pos);
    assert_eq!(bomb[0].bounces_remaining, tuning.bomb_bounces);
    assert_eq!(
        bomb[0].shrapnel_rng_seed,
        shrapnel_rng_seed(
            pos,
            vel + vec2(1.0, 0.0) * tuning.bomb_speed,
            0,
            WEAPON_LEVEL_MIN,
            7
        )
    );

    let shrapnel_bomb = remote_weapon_projectiles(
        source,
        encode_weapon_data(3, 2, true, 4, 3, false),
        tuning,
        &arena,
    );
    assert_eq!(
        shrapnel_bomb[0].shrapnel_rng_seed,
        shrapnel_rng_seed(pos, vel + vec2(1.0, 0.0) * tuning.bomb_speed, 3, 2, 7)
    );

    let mut emp_tuning = tuning;
    emp_tuning.emp_bomb = true;
    let mut emp_arena = arena;
    emp_arena.emp_bomb_shutdown_seconds = 4.0;
    let emp_bomb = remote_weapon_projectiles(
        remote_source(emp_tuning, pos, vel, 0.25),
        3,
        emp_tuning,
        &emp_arena,
    );
    assert_eq!(emp_bomb[0].emp_shutdown_time, 4.0);

    let proximity_bomb = remote_weapon_projectiles(source, 4, tuning, &arena);
    assert_eq!(
        proximity_bomb[0].kind,
        RemoteProjectileKind::Bomb { proximity: true }
    );
    assert_eq!(proximity_bomb[0].proximity_sensor, None);

    let mut thor_tuning = tuning;
    thor_tuning.bomb_speed = 430.0;
    thor_tuning.thor_speed = 999.0;
    thor_tuning.bomb_bounces = 1;
    let mut thor_arena = arena;
    thor_arena.bouncing_bomb_damage_multiplier = 0.75;
    let thor = remote_weapon_projectiles(
        remote_source(thor_tuning, pos, vel, 0.25),
        8,
        thor_tuning,
        &arena,
    );
    assert_eq!(thor[0].kind, RemoteProjectileKind::Thor);
    assert_eq!(thor[0].pos, pos);
    assert_eq!(thor[0].vel, vel + vec2(1.0, 0.0) * thor_tuning.bomb_speed);
    let bounced_thor = remote_weapon_projectiles(
        remote_source(thor_tuning, pos, vel, 0.25),
        8,
        thor_tuning,
        &thor_arena,
    );
    assert_eq!(bounced_thor[0].damage, thor_arena.bomb_damage * 0.75);
}

#[test]
fn authenticated_source_behavior_is_immutable_across_owner_replacement_and_wrap() {
    let mut old_tuning = ShipTuning::continuum_skirmish_default();
    old_tuning.wormhole_gravity = 40;
    old_tuning.bomb_bounces = 7;
    let old_identity = RemoteWeaponSourceIdentity::authenticated(9, u64::MAX, u32::MAX).unwrap();
    let old_source = RemoteWeaponSource::capture(
        old_identity,
        RemoteWeaponArenaScope::authenticated(u64::MAX, u64::MAX),
        vec2(7.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vec2(3.0, -4.0),
        0.75,
        7,
        0,
        old_tuning,
        true,
    )
    .unwrap();

    let replacement_identity = RemoteWeaponSourceIdentity::authenticated(9, 1, 1).unwrap();
    assert_ne!(old_identity, replacement_identity);
    assert!(!old_identity.matches_authenticated_runtime(9, Some(1), Some(1)));
    assert!(old_identity.matches_authenticated_runtime(9, Some(u64::MAX), Some(u32::MAX)));

    let arena = ArenaTuning::continuum_skirmish_default();
    let bomb = remote_weapon_projectiles(old_source, 3, old_tuning, &arena).remove(0);
    let mine = remote_weapon_projectiles(old_source, 3 | (1 << 15), old_tuning, &arena).remove(0);
    assert_eq!(bomb.source, old_source);
    assert_eq!(remote_mine_repel_bounces(&mine), 7);

    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    for dy in 0..5 {
        for dx in 0..5 {
            map.set_tile(8 + dx, 8 + dy, WORMHOLE_TILE_ID);
        }
    }
    let old_velocity = remote_bomb_wormhole_gravity_velocity(
        &map,
        old_source.fire_position,
        Vec2::ZERO,
        bomb.source.bomb_gravity,
        0.1,
    );
    assert!(old_velocity.x > 0.0);

    let mut replacement_tuning = old_tuning;
    replacement_tuning.wormhole_gravity = -40;
    replacement_tuning.bomb_bounces = 1;
    let replacement_source = RemoteWeaponSource::capture(
        replacement_identity,
        RemoteWeaponArenaScope::authenticated(1, 1),
        vec2(1000.0, 2000.0),
        Vec2::ZERO,
        0.0,
        99,
        1,
        replacement_tuning,
        true,
    )
    .unwrap();
    assert_ne!(bomb.source.fire_position, replacement_source.fire_position);
    assert_eq!(bomb.source.bomb_gravity, Some(40));
    assert_eq!(bomb.source.bomb_bounces, 7);
    assert_eq!(bomb.source.frequency, 7);
}

#[test]
fn remote_weapon_source_requires_identity_appropriate_arena_scope() {
    let tuning = ShipTuning::continuum_skirmish_default();
    let authenticated = RemoteWeaponSourceIdentity::authenticated(9, 10, 11).unwrap();
    let legacy = RemoteWeaponSourceIdentity::legacy(9, Some(11));
    let scope = RemoteWeaponArenaScope::authenticated(12, 13);
    let capture = |identity, arena_scope| {
        RemoteWeaponSource::capture(
            identity,
            arena_scope,
            vec2(100.0, 200.0),
            Vec2::ZERO,
            0.5,
            7,
            0,
            tuning,
            true,
        )
    };

    assert!(capture(authenticated, scope).is_some());
    assert!(capture(legacy, None).is_some());
    assert!(capture(authenticated, None).is_none());
    assert!(capture(legacy, scope).is_none());
}

#[test]
fn remote_projectile_solid_cleanup_matches_reference_weapon_types() {
    assert!(remote_projectile_uses_solid_cleanup(
        RemoteProjectileKind::Bullet { bouncing: false }
    ));
    assert!(!remote_projectile_uses_solid_cleanup(
        RemoteProjectileKind::Bullet { bouncing: true }
    ));
    assert!(remote_projectile_uses_solid_cleanup(
        RemoteProjectileKind::Shrapnel { bouncing: false }
    ));
    assert!(!remote_projectile_uses_solid_cleanup(
        RemoteProjectileKind::Shrapnel { bouncing: true }
    ));
    assert!(!remote_projectile_uses_solid_cleanup(
        RemoteProjectileKind::Burst { active: true }
    ));
    assert!(!remote_projectile_uses_solid_cleanup(
        RemoteProjectileKind::Bomb { proximity: false }
    ));
    assert!(!remote_projectile_uses_solid_cleanup(
        RemoteProjectileKind::Mine { proximity: false }
    ));
    assert!(!remote_projectile_uses_solid_cleanup(
        RemoteProjectileKind::Thor
    ));
}

#[test]
fn remote_shrapnel_projectiles_spawn_from_reference_game_weapon_bits() {
    let mut arena = ArenaTuning::continuum_skirmish_default();
    arena.shrapnel_random = false;
    let pos = vec2(100.0, 200.0);
    let weapon = encode_weapon_data(3, 2, true, 4, 3, false);

    let source = remote_source(
        ShipTuning::continuum_skirmish_default(),
        pos,
        Vec2::ZERO,
        0.0,
    );
    let shrapnel = remote_shrapnel_projectiles(source, weapon, pos, 1234, &arena);

    assert_eq!(shrapnel.len(), 3);
    assert!(shrapnel.iter().all(|projectile| {
        projectile.source == source
            && projectile.frequency == 7
            && projectile.weapon_data == encode_weapon_data(15, 4, false, 1, 0, false)
            && projectile.kind == RemoteProjectileKind::Shrapnel { bouncing: true }
            && projectile.pos == pos
            && projectile.bounces_remaining == 0
            && projectile.ttl == weapon_lifetime(&arena, WeaponLifetimeKind::Bullet)
            && projectile.level == 4
            && projectile.radius == 0.0
            && projectile.emp_shutdown_time == 0.0
            && projectile.shrapnel_rng_seed == 0
    }));
    assert_eq!(
        shrapnel[0].damage,
        active_shrapnel_damage(
            arena.bullet_damage,
            arena.bullet_damage_upgrade,
            arena.shrapnel_damage_multiplier,
            4,
        )
    );
    assert_eq!(
        shrapnel[0].inactive_damage,
        inactive_shrapnel_damage(arena.inactive_shrapnel_damage, 4)
    );
    assert!(
        remote_shrapnel_projectiles(
            source,
            encode_weapon_data(3, 2, false, 1, 0, false),
            pos,
            1234,
            &arena
        )
        .is_empty()
    );
}

#[test]
fn remote_weapon_visual_positions_follow_player_orientation() {
    let player_pos = vec2(100.0, 200.0);
    assert_eq!(
        bomb_flash_position(player_pos, vec2(1.0, 0.0), 14.0),
        vec2(114.0, 200.0)
    );
    assert_eq!(
        bomb_flash_position(player_pos, vec2(1.0, 0.0), 20.0),
        vec2(120.0, 200.0)
    );
    assert_eq!(
        remote_weapon_visual_position(3, player_pos, 0.25, 14.0),
        Some((RemoteWeaponVisual::BombFlash, vec2(114.0, 200.0)))
    );
    assert_eq!(
        remote_weapon_visual_position(5, player_pos, 0.25, 14.0),
        Some((RemoteWeaponVisual::Repel, player_pos))
    );
}
