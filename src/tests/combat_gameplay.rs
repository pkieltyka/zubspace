use super::*;

fn remote_test_source(player_id: u16) -> RemoteWeaponSource {
    let tuning = ShipTuning::continuum_skirmish_default();
    RemoteWeaponSource::capture(
        RemoteWeaponSourceIdentity::authenticated(player_id, u64::MAX, u32::MAX).unwrap(),
        RemoteWeaponArenaScope::authenticated(u64::MAX, u64::MAX),
        Vec2::ZERO,
        Vec2::ZERO,
        0.0,
        1,
        0,
        tuning,
        false,
    )
    .unwrap()
}

#[test]
fn shrapnel_projectile_position_matches_reference_game_spawn_origin() {
    let pos = vec2(100.0, 200.0);
    assert_eq!(shrapnel_projectile_position(pos), pos);
}

#[test]
fn protocol_position_helpers_match_reference_game_units() {
    assert_eq!(protocol_position_coord(32.4), 32);
    assert_eq!(protocol_position_coord(32.6), 33);
    assert_eq!(protocol_position_coord(-4.0), 0);
    assert_eq!(protocol_velocity_component(12.5), 125);
    assert_eq!(protocol_velocity_component(-8.25), -83);
    assert_eq!(protocol_direction(0.25), 10);
    assert_eq!(protocol_direction(0.4999), 19);
    assert_eq!(protocol_direction(0.999), 39);
    assert_eq!(
        shrapnel_rng_seed(vec2(10.0, 20.0), vec2(3.4, 5.6), 4, 2, 7),
        30_102
    );
}

#[test]
fn ship_togglables_use_reference_status_bits() {
    let ship = Ship {
        pos: Vec2::ZERO,
        vel: Vec2::ZERO,
        orientation: 0.0,
        ship_type: 0,
        multifire_capable: false,
        multifire: false,
        stealth_capable: true,
        stealth: true,
        cloak_capable: true,
        cloak: true,
        xradar_capable: true,
        xradar: true,
        antiwarp_capable: true,
        antiwarp: true,
        flash: true,
        proximity_bombs: false,
        bouncing_bullets: false,
        super_timer: 0.0,
        shield_timer: 0.0,
        engine_shutdown_timer: 0.0,
        emp_shutdown_timer: 0.0,
        repel_timer: 0.0,
        repels: 0,
        bursts: 0,
        bricks: 0,
        decoys: 0,
        thors: 0,
        rockets: 0,
        portals: 0,
        gun_level: WEAPON_LEVEL_MIN,
        bomb_level: WEAPON_LEVEL_MIN,
        rotation_per_second: 0.0,
        thrust_acceleration: 0.0,
        top_speed: 0.0,
        recharge_per_second: 0.0,
        shrapnel_upgrades: 0,
        bullet_cooldown: 0.0,
        burst_cooldown: 0.0,
        bomb_cooldown: 0.0,
        repel_cooldown: 0.0,
        wormhole_cooldown: 0.0,
        fake_antiwarp_timer: 0.0,
        rocket_timer: 0.0,
        rocket_cooldown: 0.0,
        max_energy: 100.0,
        energy: 100.0,
    };

    assert_eq!(ship_togglables(&ship, true), 0b0011_1111);
}

#[test]
fn cloak_toggle_flash_matches_reference_game_status_rule() {
    assert!(!cloak_toggle_sets_flash(false, true));
    assert!(cloak_toggle_sets_flash(true, false));
    assert!(!cloak_toggle_sets_flash(false, false));
    assert!(!cloak_toggle_sets_flash(true, true));
}

#[test]
fn status_toggle_off_sound_matches_reference_game_rule() {
    assert!(!status_toggle_uses_off_sound(false, true));
    assert!(status_toggle_uses_off_sound(true, false));
    assert!(!status_toggle_uses_off_sound(false, false));
    assert!(!status_toggle_uses_off_sound(true, true));
}

#[test]
fn status_energy_drains_in_reference_game_priority_order() {
    let mut stealth = true;
    let mut cloak = true;
    let mut xradar = true;
    let mut antiwarp = true;
    let mut energy = 25.0;

    drain_ship_status_energy(
        &mut stealth,
        &mut cloak,
        &mut xradar,
        &mut antiwarp,
        &mut energy,
        10.0,
        10.0,
        10.0,
        10.0,
        1.0,
    );

    assert!(xradar);
    assert!(stealth);
    assert!(!cloak);
    assert!(!antiwarp);
    assert_eq!(energy, 5.0);
}

#[test]
fn ship_speed_limit_matches_reference_game_without_extra_drag() {
    let below_limit = vec2(50.0, 20.0);
    let below = truncate_movement_vector(MovementVector::new(below_limit.x, below_limit.y), 100.0);
    assert_eq!(vec2(below.x, below.y), below_limit);

    let limited = truncate_movement_vector(MovementVector::new(120.0, 0.0), 100.0);
    assert_eq!(vec2(limited.x, limited.y), vec2(100.0, 0.0));
}

#[test]
fn timed_indicator_labels_match_reference_shapes() {
    assert_eq!(timed_indicator_seconds_label(12.34), "12.3");
    assert_eq!(timed_indicator_seconds_label(-1.0), "0.0");
    assert_eq!(timed_indicator_centiseconds_label(1250), "12.5");
    assert_eq!(koth_timer_label(333), "3.3");
    assert_eq!(koth_timer_label(12_345), "2:03.4");
    assert_eq!(timed_indicator_percent_label(5.0, 20.0), "25%");
    assert_eq!(timed_indicator_percent_label(30.0, 20.0), "100%");
    assert_eq!(timed_indicator_percent_label(5.0, 0.0), "0%");
}

#[test]
fn weapon_use_reveals_cloak_only_outside_safe_zone() {
    assert!(weapon_use_reveals_cloak(false, true));
    assert!(!weapon_use_reveals_cloak(true, true));
    assert!(!weapon_use_reveals_cloak(false, false));
    assert!(!weapon_use_reveals_cloak(true, false));
}

#[test]
fn spectator_flags_follow_hide_flags_setting() {
    assert!(flags_visible_to_viewer(false, false, 0.0, 0.0));
    assert!(flags_visible_to_viewer(false, true, 0.0, 0.0));
    assert!(flags_visible_to_viewer(true, false, 0.0, 0.0));
    assert!(!flags_visible_to_viewer(true, true, 100.0, 0.0));
}

#[test]
fn flags_stay_hidden_until_enter_game_flagging_delay_passes() {
    assert!(!flags_visible_to_viewer(false, false, 4.9, 5.0));
    assert!(!flags_visible_to_viewer(false, false, 5.0, 5.0));
    assert!(flags_visible_to_viewer(false, false, 5.1, 5.0));
    assert!(!flags_visible_to_viewer(true, true, 5.1, 5.0));
}

#[test]
fn spectator_xradar_respects_arena_no_xradar_flag() {
    assert_eq!(spectator_xradar_toggle_after(false, false), Some(true));
    assert_eq!(spectator_xradar_toggle_after(true, false), Some(false));
    assert_eq!(spectator_xradar_toggle_after(false, true), None);
}

#[test]
fn spectator_visibility_follows_watched_frequency_and_ship() {
    assert_eq!(viewer_frequency_after_spectate(3, false, Some(7)), 3);
    assert_eq!(viewer_frequency_after_spectate(3, true, Some(7)), 7);
    assert_eq!(viewer_frequency_after_spectate(3, true, None), 3);

    assert_eq!(visibility_ship_type_after_spectate(2, false, Some(6)), 2);
    assert_eq!(visibility_ship_type_after_spectate(2, true, Some(6)), 6);
    assert_eq!(visibility_ship_type_after_spectate(2, true, Some(8)), 2);
    assert_eq!(visibility_ship_type_after_spectate(2, true, None), 2);
}

#[test]
fn radar_visibility_uses_stealth_xradar_and_team() {
    assert!(network_player_radar_visible(0, 4, 5, false));
    assert!(!network_player_radar_visible(STATUS_STEALTH, 4, 5, false));
    assert!(network_player_radar_visible(STATUS_STEALTH, 4, 5, true));
    assert!(network_player_radar_visible(STATUS_STEALTH, 4, 4, false));
}

#[test]
fn world_visibility_uses_cloak_xradar_and_team() {
    assert!(network_player_world_visible(0, 4, 5, false));
    assert!(!network_player_world_visible(STATUS_CLOAK, 4, 5, false));
    assert!(network_player_world_visible(STATUS_CLOAK, 4, 5, true));
    assert!(network_player_world_visible(STATUS_CLOAK, 4, 4, false));
    assert!(network_player_world_visible(STATUS_STEALTH, 4, 5, false));
}

#[test]
fn remote_weapon_visuals_decode_reference_game_weapon_data_bits() {
    let bullet = 1;
    let bomb = 3;
    let mine = 3 | (1 << 15);
    let proximity_bomb = 4 | (2 << 5);
    let repel = 5;
    let burst = 7;
    let thor = 8;

    assert_eq!(remote_weapon_visual(bullet), None);
    assert_eq!(
        remote_weapon_visual(bomb),
        Some(RemoteWeaponVisual::BombFlash)
    );
    assert_eq!(remote_weapon_visual(mine), None);
    assert_eq!(
        remote_weapon_visual(proximity_bomb),
        Some(RemoteWeaponVisual::BombFlash)
    );
    assert_eq!(remote_weapon_visual(repel), Some(RemoteWeaponVisual::Repel));
    assert_eq!(remote_weapon_visual(burst), Some(RemoteWeaponVisual::Burst));
    assert_eq!(
        remote_weapon_visual(thor),
        Some(RemoteWeaponVisual::BombFlash)
    );
}

#[test]
fn remote_weapon_sounds_match_reference_game_weapon_audio_types() {
    assert_eq!(
        remote_weapon_sound(encode_weapon_data(1, 3, false, 1, 0, false), false),
        Some(RemoteWeaponSoundKind::Gun(3))
    );
    assert_eq!(
        remote_weapon_sound(encode_weapon_data(2, 4, false, 1, 0, true), false),
        Some(RemoteWeaponSoundKind::Gun(4))
    );
    assert_eq!(
        remote_weapon_sound(encode_weapon_data(3, 2, false, 1, 0, false), false),
        Some(RemoteWeaponSoundKind::Bomb(2))
    );
    assert_eq!(
        remote_weapon_sound(encode_weapon_data(4, 2, false, 1, 0, false), true),
        Some(RemoteWeaponSoundKind::EmpBomb(2))
    );
    assert_eq!(
        remote_weapon_sound(encode_weapon_data(3, 2, false, 1, 0, true), false),
        Some(RemoteWeaponSoundKind::Mine(2))
    );
    assert_eq!(
        remote_weapon_sound(encode_weapon_data(5, 1, false, 1, 0, false), false),
        Some(RemoteWeaponSoundKind::Repel)
    );
    assert_eq!(
        remote_weapon_sound(encode_weapon_data(6, 1, false, 1, 0, false), false),
        Some(RemoteWeaponSoundKind::Decoy)
    );
    assert_eq!(
        remote_weapon_sound(encode_weapon_data(7, 1, false, 1, 0, false), false),
        Some(RemoteWeaponSoundKind::Burst)
    );
    assert_eq!(
        remote_weapon_sound(encode_weapon_data(8, 1, false, 1, 0, false), false),
        Some(RemoteWeaponSoundKind::Thor)
    );
    assert_eq!(
        remote_weapon_sound(encode_weapon_data(15, 1, false, 1, 0, false), false),
        None
    );
}

#[test]
fn remote_weapon_data_decodes_reference_game_bitfields() {
    let weapon = 2 | (3 << 5) | (1 << 7) | (2 << 8) | (12 << 10) | (1 << 15);
    assert_eq!(
        remote_weapon_data(weapon),
        RemoteWeaponData {
            weapon_type: 2,
            level: 4,
            shrap_bouncing: true,
            shrap_level: 3,
            shrap: 12,
            alternate: true,
        }
    );
}

#[test]
fn outbound_weapon_data_encodes_reference_game_bitfields() {
    assert_eq!(
        encode_weapon_data(2, 4, true, 3, 12, true),
        2 | (3 << 5) | (1 << 7) | (2 << 8) | (12 << 10) | (1 << 15)
    );
    assert_eq!(encode_weapon_data(3, 1, false, 1, 0, false), 3);
    assert_eq!(encode_weapon_data(3, 1, false, 1, 0, true), 3 | (1 << 15));
}

#[test]
fn explosive_shrapnel_matches_parent_bomb_level_as_project_override() {
    for bomb_level in 1..=4 {
        assert_eq!(explosive_shrapnel_level(bomb_level), bomb_level);
        let weapon = encode_weapon_data(
            3,
            bomb_level,
            true,
            explosive_shrapnel_level(bomb_level),
            7,
            false,
        );
        let decoded = remote_weapon_data(weapon);
        assert_eq!(decoded.level, bomb_level);
        assert_eq!(decoded.shrap_level, bomb_level);
        assert_eq!(decoded.shrap, 7);
    }
}

#[test]
fn attached_children_reveal_radar_parent_only_when_not_stealthed() {
    let mut child = NetworkPlayer::from_roster(
        &PlayerRosterEntry::parse(&sample_player_enter_packet(12, b"turret")).unwrap(),
    );
    child.togglables = 0;
    assert!(network_child_reveals_parent_on_radar(&child));

    child.togglables = STATUS_STEALTH;
    assert!(!network_child_reveals_parent_on_radar(&child));
}

#[test]
fn bot_orientation_helpers_track_shortest_turns() {
    assert!((heading_to_orientation(vec2(0.0, -1.0)) - 0.0).abs() < 0.001);
    assert!((heading_to_orientation(vec2(1.0, 0.0)) - 0.25).abs() < 0.001);

    let wrapped = turn_towards_orientation(0.98, 0.02, 0.1, 0.5);
    assert!(!(0.03..=0.99).contains(&wrapped));
}

#[test]
fn radial_damage_falls_off_to_zero() {
    assert_eq!(radial_damage(0.0, 10.0, 100.0), 100.0);
    assert_eq!(radial_damage(10.0, 10.0, 100.0), 0.0);
    assert!((radial_damage(5.0, 10.0, 100.0) - 50.0).abs() < 0.001);
    assert_eq!(radial_damage(5.0, 10.0, 101.0), 50.0);
    assert_eq!(
        radial_damage_with_shooter_proximity_reduction(0.0, Some(0.0), 10.0, 100.0),
        50.0
    );
    assert_eq!(
        radial_damage_with_shooter_proximity_reduction(0.0, Some(10.0), 10.0, 100.0),
        100.0
    );
    assert_eq!(
        radial_damage_with_shooter_proximity_reduction(9.0, Some(0.0), 10.0, 100.0),
        0.0
    );
    assert_eq!(
        radial_damage_with_shooter_proximity_reduction(0.0, Some(5.0), 10.0, 101.0),
        76.0
    );
    assert_eq!(
        radial_damage_with_shooter_proximity_reduction(5.0, None, 10.0, 100.0),
        50.0
    );
    assert!(explosion_bot_damage_active(true, 0.0));
    assert!(explosion_bot_damage_active(true, 0.05));
    assert!(!explosion_bot_damage_active(true, 0.051));
    assert!(!explosion_bot_damage_active(false, 0.0));
}

#[test]
fn player_triggered_bomb_explosions_rewind_to_previous_position() {
    assert_eq!(
        player_triggered_bomb_explosion_position(vec2(10.0, 20.0), vec2(30.0, 40.0)),
        vec2(10.0, 20.0)
    );
    assert_eq!(
        player_triggered_bomb_damage_position(vec2(10.0, 20.0), vec2(30.0, 40.0)),
        vec2(30.0, 40.0)
    );
}

#[test]
fn player_triggered_proximity_explosions_rewind_to_previous_position() {
    assert_eq!(
        player_triggered_proximity_explosion_position(vec2(12.0, 24.0), vec2(36.0, 48.0)),
        vec2(12.0, 24.0)
    );
    assert_eq!(
        player_triggered_proximity_damage_position(vec2(12.0, 24.0), vec2(36.0, 48.0)),
        vec2(36.0, 48.0)
    );
}

#[test]
fn emp_explosion_shutdown_scales_with_actual_damage() {
    let explosion = Explosion {
        prediction: None,
        pos: Vec2::ZERO,
        damage_pos: Vec2::ZERO,
        age: 0.0,
        kind: ExplosionKind::EmpBomb,
        damages_bots: true,
        damage: 100.0,
        radius: 100.0,
        emp_shutdown_time: 4.0,
        remote_source: None,
    };

    assert_eq!(explosion_emp_shutdown_seconds(&explosion, 0.0), 0.0);
    assert_eq!(explosion_emp_shutdown_seconds(&explosion, 50.0), 2.0);
    assert_eq!(explosion_emp_shutdown_seconds(&explosion, 100.0), 4.0);
    assert_eq!(emp_shutdown_seconds_from_raw_damage(4.0, 50.0, 100.0), 2.0);
    assert_eq!(emp_shutdown_seconds_from_raw_damage(4.0, 150.0, 100.0), 4.0);
    assert_eq!(emp_shutdown_seconds_from_raw_damage(0.0, 50.0, 100.0), 0.0);
    assert_eq!(emp_shutdown_seconds_from_raw_damage(4.0, 0.0, 100.0), 0.0);
}

#[test]
fn networked_play_never_applies_local_visual_damage_to_player_energy() {
    assert!(local_visual_damage_applies(false, 1.0));
    assert!(!local_visual_damage_applies(false, 0.0));
    assert!(!local_visual_damage_applies(false, -1.0));
    assert!(!local_visual_damage_applies(true, 1.0));
    assert!(!local_visual_damage_applies(true, 400.0));
}

#[test]
fn explosion_visual_kind_matches_reference_bomb_audio_sets() {
    assert_eq!(explosion_kind_for_emp_shutdown(0.0), ExplosionKind::Bomb);
    assert_eq!(
        explosion_kind_for_emp_shutdown(0.01),
        ExplosionKind::EmpBomb
    );
    assert_eq!(
        explosion_visual_lifetime(ExplosionKind::Bomb),
        EXPLOSION_LIFETIME
    );
    assert_eq!(
        explosion_visual_lifetime(ExplosionKind::EmpBomb),
        EMP_EXPLOSION_LIFETIME
    );
}

#[test]
fn bomb_and_mine_sprite_frames_match_reference_game_reference_ranges() {
    assert_eq!(bomb_sprite_frame_start(WEAPON_LEVEL_MIN, 0, false), 0);
    assert_eq!(bomb_sprite_frame_start(WEAPON_LEVEL_MAX, 0, false), 30);
    assert_eq!(bomb_sprite_frame_start(WEAPON_LEVEL_MIN, 0, true), 40);
    assert_eq!(bomb_sprite_frame_start(WEAPON_LEVEL_MAX, 0, true), 70);
    assert_eq!(bomb_sprite_frame_start(WEAPON_LEVEL_MIN, 1, false), 80);
    assert_eq!(bomb_sprite_frame_start(WEAPON_LEVEL_MAX, 1, false), 110);
    assert_eq!(bomb_sprite_frame_start(WEAPON_LEVEL_MIN, 1, true), 40);

    assert_eq!(mine_sprite_frame_start(WEAPON_LEVEL_MIN, false), 0);
    assert_eq!(mine_sprite_frame_start(WEAPON_LEVEL_MAX, false), 30);
    assert_eq!(mine_sprite_frame_start(WEAPON_LEVEL_MIN, true), 40);
    assert_eq!(mine_sprite_frame_start(WEAPON_LEVEL_MAX, true), 70);
}

#[test]
fn leveled_weapon_damage_uses_zero_based_continuum_upgrade_steps() {
    assert_eq!(leveled_weapon_damage(90.0, 10.0, 0), 90.0);
    assert_eq!(leveled_weapon_damage(90.0, 10.0, 1), 90.0);
    assert_eq!(leveled_weapon_damage(90.0, 10.0, 2), 100.0);
    assert_eq!(leveled_weapon_damage(90.0, 10.0, 4), 120.0);
    assert_eq!(leveled_weapon_energy_cost(120.0, 15.0, 1), 120.0);
    assert_eq!(leveled_weapon_energy_cost(120.0, 15.0, 3), 150.0);
    assert_eq!(leveled_gun_energy_cost(30.0, 0), 30.0);
    assert_eq!(leveled_gun_energy_cost(30.0, 1), 30.0);
    assert_eq!(leveled_gun_energy_cost(30.0, 3), 90.0);
}

#[test]
fn bomb_recoil_applies_before_bomb_velocity_is_sampled() {
    assert_eq!(
        bomb_recoil_velocity(vec2(20.0, 5.0), vec2(0.0, -1.0), 16.0),
        vec2(20.0, 21.0)
    );
}

#[test]
fn active_mine_limit_uses_ship_and_team_caps() {
    assert_eq!(mine_limit(8, 4), 4);
    assert_eq!(mine_limit(4, 8), 4);
    assert_eq!(mine_limit(8, usize::MAX), 8);
    assert_eq!(mine_limit(8, 0), 0);
    assert!(mine_drop_allowed(1, 3, 4, 4, false));
    assert!(!mine_drop_allowed(4, 4, 4, 8, false));
    assert!(!mine_drop_allowed(1, 4, 8, 4, false));
    assert!(!mine_drop_allowed(1, 3, 4, 4, true));
    assert!(mine_positions_overlap(vec2(10.0, 20.0), vec2(10.0, 20.0)));
    assert!(!mine_positions_overlap(vec2(10.0, 20.0), vec2(10.5, 20.5)));
}

#[test]
fn rockets_wait_for_rocket_and_bomb_cooldowns() {
    assert!(rocket_available(0.0, 0.0));
    assert!(!rocket_available(0.1, 0.0));
    assert!(!rocket_available(0.0, 0.1));
}

#[test]
fn weapons_set_reference_repel_reuse_delay() {
    assert_eq!(repel_cooldown_after_weapon(0.0), REPEL_REUSE_COOLDOWN);
    assert_eq!(repel_cooldown_after_weapon(0.25), REPEL_REUSE_COOLDOWN);
    assert_eq!(repel_cooldown_after_weapon(0.75), 0.75);
}

#[test]
fn normal_weapon_cooldowns_cross_lock_like_reference_game() {
    assert_eq!(gun_cooldowns_after_fire(0.0, 0.08), (0.08, 0.08));
    assert_eq!(gun_cooldowns_after_fire(0.2, 0.08), (0.08, 0.2));

    assert_eq!(bomb_cooldowns_after_fire(0.0, 0.65, false), (0.65, 0.65));
    assert_eq!(bomb_cooldowns_after_fire(0.2, 0.65, false), (0.65, 0.65));
    assert_eq!(bomb_cooldowns_after_fire(0.2, 0.65, true), (0.2, 0.65));
}

#[test]
fn weapon_energy_availability_uses_reference_strict_comparison() {
    assert!(weapon_energy_available(20.01, 20.0));
    assert!(!weapon_energy_available(20.0, 20.0));
    assert!(weapon_energy_available(0.01, 0.0));
    assert!(!weapon_energy_available(0.0, 0.0));

    assert!(special_weapon_energy_available(true, 0.0, 20.0));
    assert!(special_weapon_energy_available(false, 0.01, 0.0));
    assert!(!special_weapon_energy_available(false, 0.0, 0.0));
    assert!(!special_weapon_energy_available(false, 20.0, 20.0));
    assert_eq!(special_weapon_energy_after_fire(123.0), 123.0);
}

#[test]
fn repel_availability_uses_repel_reuse_not_bomb_cooldown() {
    assert!(repel_weapon_available(0.0, false, 1.0, 0.0, 1));
    assert!(!repel_weapon_available(0.0, false, 0.0, 0.0, 1));
    assert!(repel_weapon_available(0.0, true, 0.0, 20.0, 1));
    assert!(!repel_weapon_available(0.01, false, 100.0, 0.0, 1));
    assert!(!repel_weapon_available(0.0, false, 100.0, 0.0, 0));
}

#[test]
fn disable_fast_shooting_blocks_fast_weapons_only_while_afterburning() {
    assert!(fast_weapon_available(false, true));
    assert!(fast_weapon_available(true, false));
    assert!(!fast_weapon_available(true, true));
}

#[test]
fn overlapping_special_weapon_inputs_use_reference_game_priority() {
    assert_eq!(
        prioritized_special_weapon_action(true, true, true, true),
        Some(SpecialWeaponAction::Repel)
    );
    assert_eq!(
        prioritized_special_weapon_action(false, true, true, true),
        Some(SpecialWeaponAction::Burst)
    );
    assert_eq!(
        prioritized_special_weapon_action(false, false, true, true),
        Some(SpecialWeaponAction::Thor)
    );
    assert_eq!(
        prioritized_special_weapon_action(false, false, false, true),
        Some(SpecialWeaponAction::Decoy)
    );
    assert_eq!(
        prioritized_special_weapon_action(false, false, false, false),
        None
    );
}

// Wormholes are 5x5 footprints; the anchor (top-left) is at tile (8, 8) so
// the wormhole center lands at (10.5, 10.5) tiles. Tests place a full 5x5
// footprint to also exercise the anchor-only iteration (the gravity well
// must contribute exactly once, not 25 times).
fn place_wormhole_at_anchor(map: &mut ArenaMap, anchor_x: i32, anchor_y: i32) {
    for dy in 0..5 {
        for dx in 0..5 {
            map.set_tile(anchor_x + dx, anchor_y + dy, WORMHOLE_TILE_ID);
        }
    }
}

#[test]
fn wormhole_gravity_applies_nearby_velocity_and_top_speed_override() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    place_wormhole_at_anchor(&mut map, 8, 8);

    let pos = vec2(7.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    let (vel, top_speed) = wormhole_gravity_velocity(&map, pos, Vec2::ZERO, 40, 560.0, 0.1);

    assert!(vel.x > 0.0);
    assert!(vel.y.abs() < 0.001);
    assert_eq!(top_speed, Some(560.0));

    let (vel, top_speed) = wormhole_gravity_velocity(&map, pos, Vec2::ZERO, -40, 560.0, 0.1);
    assert!(vel.x < 0.0);
    assert_eq!(top_speed, Some(560.0));
}

#[test]
fn gravity_bombs_gate_wormhole_pull_for_bombs() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    place_wormhole_at_anchor(&mut map, 8, 8);

    let pos = vec2(7.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    let unchanged = bomb_wormhole_gravity_velocity(&map, pos, Vec2::ZERO, 40, false, 0.1);
    let affected = bomb_wormhole_gravity_velocity(&map, pos, Vec2::ZERO, 40, true, 0.1);

    assert_eq!(unchanged, Vec2::ZERO);
    assert!(affected.x > 0.0);
    assert!(affected.y.abs() < 0.001);
}

#[test]
fn remote_bomb_gravity_uses_immutable_fire_time_setting() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    place_wormhole_at_anchor(&mut map, 8, 8);

    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.gravity_bombs = true;
    tuning.ships[0].wormhole_gravity = 40;
    tuning.ships[1].wormhole_gravity = -40;

    let pos = vec2(7.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    let attracted = remote_bomb_wormhole_gravity_velocity(
        &map,
        pos,
        Vec2::ZERO,
        Some(tuning.ship(0).wormhole_gravity),
        0.1,
    );
    let repelled = remote_bomb_wormhole_gravity_velocity(
        &map,
        pos,
        Vec2::ZERO,
        Some(tuning.ship(1).wormhole_gravity),
        0.1,
    );
    let disabled = remote_bomb_wormhole_gravity_velocity(&map, pos, Vec2::ZERO, None, 0.1);

    assert!(attracted.x > 0.0);
    assert!(repelled.x < 0.0);
    assert_eq!(disabled, Vec2::ZERO);
}

#[test]
fn timed_protection_damage_matches_fading_shield() {
    assert_eq!(damage_after_timed_protection(100.0, 1.0, 0.0, 10.0), 0.0);
    assert_eq!(damage_after_timed_protection(100.0, 0.0, 10.0, 10.0), 0.0);
    assert_eq!(damage_after_timed_protection(100.0, 0.0, 5.0, 10.0), 50.0);
    assert_eq!(damage_after_timed_protection(101.0, 0.0, 5.0, 10.0), 51.0);
    assert_eq!(damage_after_timed_protection(100.0, 0.0, 0.0, 10.0), 100.0);
    assert_eq!(damage_after_timed_protection(100.0, 0.0, 5.0, 0.0), 100.0);
}

#[test]
fn continuum_bounce_factor_matches_reference_scale() {
    assert_eq!(continuum_bounce_factor(16), 1.0);
    assert_eq!(continuum_bounce_factor(32), 0.5);
    assert_eq!(continuum_bounce_factor(0), 16.0);
    assert_eq!(continuum_bounce_factor(-4), 16.0);
}

#[test]
fn ship_wall_bounce_scales_both_velocity_axes_like_reference() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.set_tile(11, 10, 1);

    let mut pos = vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    let mut vel = vec2(64.0, 20.0);

    assert!(move_entity_with_collision(
        &map, &mut pos, &mut vel, 1.0, 0.25, 0.5
    ));
    assert!(pos.x < 11.0 * TILE_SIZE);
    assert!(pos.y > 10.5 * TILE_SIZE);
    assert!(map.can_fit_world(pos, 1.0));
    assert_eq!(vel, vec2(-32.0, 10.0));

    let mut pos = vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    let mut vel = vec2(64.0, 64.0);
    map.set_tile(10, 11, 1);

    assert!(move_entity_with_collision(
        &map, &mut pos, &mut vel, 1.0, 0.25, 0.5
    ));
    assert!(pos.x < 11.0 * TILE_SIZE);
    assert!(pos.y < 11.0 * TILE_SIZE);
    assert!(map.can_fit_world(pos, 1.0));
    // The reference game selects the bounce factor before simulating either axis, then
    // updates the same-tick bounce latch only after the X/Y pair. A corner
    // therefore applies the same factor in both SimulateAxis calls; because
    // each call also scales the perpendicular component, both components end
    // this pair at one quarter magnitude.
    assert_eq!(vel, vec2(-16.0, -16.0));
    assert_eq!(
        ship_bounce_sound_threshold_squared(),
        (3.0 * TILE_SIZE) * (3.0 * TILE_SIZE)
    );
}

#[test]
fn live_ship_collision_retains_reference_discrete_high_speed_query() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.set_tile(12, 10, 1);

    let mut pos = vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    let mut vel = vec2(640.0, 0.0);

    assert!(!move_entity_with_collision(
        &map, &mut pos, &mut vel, 1.0, 0.10, 0.5
    ));
    // The reference game calls SimulatePlayer once for the full live-frame dt. Its
    // direction-dependent leading column lands beyond the thin wall here;
    // live player collision deliberately does not add 10 ms sweep slices.
    assert_eq!(pos.x.to_bits(), 232.0_f32.to_bits());
    assert_eq!(pos.y.to_bits(), 168.0_f32.to_bits());
    assert_eq!(vel, vec2(640.0, 0.0));
}

#[test]
fn ship_collision_uses_configured_radius_for_wall_fit() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.set_tile(11, 10, 1);
    let pos = vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE);

    assert!(map.can_fit_world(pos, 1.0));
    assert!(!map.can_fit_world(pos, 20.0));
}

#[test]
fn prize_pickup_overlap_matches_reference_game_box_intersection() {
    let prize = vec2(100.0, 100.0);

    assert!(ship_overlaps_prize(vec2(100.0, 100.0), 14.0, prize));
    assert!(ship_overlaps_prize(vec2(78.0, 100.0), 14.0, prize));
    assert!(!ship_overlaps_prize(vec2(77.9, 100.0), 14.0, prize));
    assert!(ship_overlaps_prize(vec2(121.9, 100.0), 14.0, prize));
    assert!(!ship_overlaps_prize(vec2(122.0, 100.0), 14.0, prize));
    assert!(!ship_overlaps_prize(vec2(100.0, 122.0), 14.0, prize));

    // Captured private Warp/Portal tests spawned their idle peer inside the
    // eventual green's box, consuming it before the preparation wait. The
    // declared central origin leaves both unchanged seeded pickups available.
    for (peer, target) in [
        (vec2(8168.0, 8216.0), vec2(8152.0, 8200.0)),
        (vec2(8184.0, 8216.0), vec2(8200.0, 8232.0)),
    ] {
        assert!(ship_overlaps_prize(peer, 14.0, target));
        assert!(!ship_overlaps_prize(vec2(8200.0, 8200.0), 14.0, target));
    }
}

#[test]
fn local_prize_pickup_requires_live_non_spectator_ship() {
    assert!(local_player_can_collect_prizes(false, 0.0));
    assert!(!local_player_can_collect_prizes(true, 0.0));
    assert!(!local_player_can_collect_prizes(false, 0.01));
}

#[test]
fn remote_prize_overlap_uses_visible_player_ship_box() {
    let mut player = NetworkPlayer::from_roster(
        &PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap(),
    );
    let prize = vec2(100.0, 100.0);

    player.position = Some(vec2(78.0, 100.0));
    assert!(network_player_overlaps_prize(&player, 14.0, prize));

    player.position = Some(vec2(77.9, 100.0));
    assert!(!network_player_overlaps_prize(&player, 14.0, prize));

    player.position = None;
    assert!(!network_player_overlaps_prize(&player, 14.0, prize));
}

#[test]
fn collected_prize_bounty_gain_matches_reference_game_positive_prizes() {
    assert_eq!(collected_prize_bounty_gain(0, 3), 3);
    assert_eq!(
        collected_prize_bounty_gain(PrizeKind::Repel.protocol_id(), 2),
        2
    );
    assert_eq!(
        collected_prize_bounty_gain(-PrizeKind::Repel.protocol_id(), 2),
        0
    );
}

#[test]
fn continuum_powerball_friction_maps_higher_values_to_faster_slowdown() {
    assert_eq!(continuum_powerball_friction(0), 1.0);
    assert_eq!(continuum_powerball_friction(1500), POWERBALL_FRICTION);
    assert!(continuum_powerball_friction(3000) < continuum_powerball_friction(1500));
    assert!((continuum_powerball_friction(u16::MAX) - 0.34465).abs() < 0.00001);
}

#[test]
fn powerball_pickup_line_of_sight_respects_solid_walls() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    let from = vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    let ball = vec2(14.5 * TILE_SIZE, 10.5 * TILE_SIZE);

    assert!(can_pickup_powerball(&map, from, ball, true));
    assert!(can_touch_powerball(&map, from, ball, true, 0.0));
    assert!(!can_touch_powerball(&map, from, ball, true, 0.1));

    map.tiles[10 * map.width + 12] = 1;
    assert!(solid_between_world_points(&map, from, ball));
    assert!(!can_pickup_powerball(&map, from, ball, true));
    assert!(can_pickup_powerball(&map, from, ball, false));
}

#[test]
fn soccer_goal_modes_ignore_own_team_goals() {
    let upper_left = vec2(256.0, 256.0) * TILE_SIZE;
    let upper_right = vec2(768.0, 256.0) * TILE_SIZE;
    let lower_left = vec2(256.0, 768.0) * TILE_SIZE;

    assert!(soccer_goal_counts(upper_left, TILE_SIZE, 0, 0));
    assert!(!soccer_goal_counts(upper_left, TILE_SIZE, 0, 1));
    assert!(soccer_goal_counts(upper_right, TILE_SIZE, 0, 1));
    assert!(!soccer_goal_counts(upper_right, TILE_SIZE, 1, 1));
    assert!(!soccer_goal_counts(lower_left, TILE_SIZE, 2, 3));
    assert!(soccer_goal_counts(upper_left, TILE_SIZE, 2, 3));
}

#[test]
fn soccer_team_goal_frame_ranges_match_reference_game_split() {
    let upper_left = vec2(256.0, 256.0);
    let upper_right = vec2(768.0, 256.0);

    assert!(soccer_team_goal(upper_left, 0, 1));
    assert!(!soccer_team_goal(upper_right, 0, 1));
    assert_eq!(GOAL_SCORING_FRAME_START, GOAL_FRAMES_PER_TEAM);
}

#[test]
fn flag_frame_ranges_match_reference_game_split() {
    assert_eq!(FLAG_TEAM_FRAME_START, FLAG_FRAMES_PER_TEAM);
}

#[test]
fn ball_carrier_weapon_settings_only_kick_when_carrying() {
    assert!(ball_carrier_should_kick(true, false));
    assert!(!ball_carrier_should_kick(true, true));
    assert!(!ball_carrier_should_kick(false, false));

    assert!(ball_carrier_warp_should_kick(true));
    assert!(!ball_carrier_warp_should_kick(false));
}

#[test]
fn powerball_manual_and_timer_release_velocities_match_reference_game() {
    let ship_velocity = vec2(5.0, -3.0);
    let ship_position = vec2(100.0, 200.0);
    let heading = vec2(1.0, 0.0);

    assert_eq!(
        manual_powerball_fire_velocity(ship_velocity, heading, 12.0),
        vec2(17.0, -3.0)
    );
    assert_eq!(
        manual_powerball_fire_request_position(ship_position),
        ship_position
    );
    assert_eq!(
        timed_powerball_release_velocity(ship_velocity, heading, 12.0),
        vec2(-7.0, -3.0)
    );
}

#[test]
fn local_powerball_pickup_requires_active_unattached_ship() {
    assert!(local_player_can_attempt_powerball_pickup(
        false, 0, false, false, true
    ));
    assert!(!local_player_can_attempt_powerball_pickup(
        true, 0, false, false, true
    ));
    assert!(!local_player_can_attempt_powerball_pickup(
        false, 8, false, false, true
    ));
    assert!(!local_player_can_attempt_powerball_pickup(
        false, 0, true, false, true
    ));
    assert!(!local_player_can_attempt_powerball_pickup(
        false, 0, false, true, true
    ));
    assert!(!local_player_can_attempt_powerball_pickup(
        false, 0, false, false, false
    ));

    assert_eq!(
        powerball_pickup_weapon_cooldowns_after(0.0, 0.0, 0.65),
        (0.65, 0.65)
    );
    assert_eq!(
        powerball_pickup_weapon_cooldowns_after(0.8, 0.2, 0.65),
        (0.8, 0.65)
    );
}

#[test]
fn inexact_weapon_damage_uses_reference_game_sqrt_roll_shape() {
    assert_eq!(inexact_weapon_damage_from_roll(100.0, 0), 0.0);
    assert_eq!(inexact_weapon_damage_from_roll(100.0, 24), 4.0);
    assert_eq!(inexact_weapon_damage_from_roll(100.0, 25), 5.0);
    assert_eq!(inexact_weapon_damage_from_roll(100.0, 10_000), 100.0);
    assert_eq!(inexact_weapon_damage_from_roll(100.0, 10_001), 0.0);
    assert_eq!(bullet_or_burst_damage_after_exactness(true, 75.0), 75.0);
    assert_eq!(bullet_or_burst_damage_after_exactness(false, 0.0), 0.0);
}

#[test]
fn damage_prize_loss_uses_reference_game_factor_and_exclusions() {
    assert!(!damage_can_remove_prize(0, 900.0, 1000.0, 5000));
    assert!(!damage_can_remove_prize(1, 1000.0, 1000.0, 5000));
    assert!(!damage_can_remove_prize(1, 900.0, 1000.0, 0));
    assert!(damage_can_remove_prize(1, 900.0, 1000.0, 5000));
    assert!(player_damage_is_fatal(99.0, 100.0));
    assert!(!player_damage_is_fatal(100.0, 100.0));
    assert!(!player_damage_is_fatal(101.0, 100.0));
    assert_eq!(local_bomb_self_damage_energy_after(150.0, 40.0), 110.0);
    assert_eq!(local_bomb_self_damage_energy_after(100.0, 100.0), 1.0);
    assert_eq!(local_bomb_self_damage_energy_after(99.0, 100.0), 1.0);
    assert!(safe_zone_weapon_hit_drops_flag(true, true, true));
    assert!(!safe_zone_weapon_hit_drops_flag(false, true, true));
    assert!(!safe_zone_weapon_hit_drops_flag(true, false, true));
    assert!(!safe_zone_weapon_hit_drops_flag(true, true, false));

    assert_eq!(damage_prize_loss_chance(5000, 100.0), 10_001);
    assert_eq!(damage_prize_loss_chance(1, 100.0), 3);
    assert_eq!(DAMAGE_PRIZE_LOSS_REROLL_LIMIT, 9999);

    for excluded in [7, 13, 14, 17, 18, 25] {
        assert!(!damage_prize_loss_id_allowed(excluded));
    }
    assert!(damage_prize_loss_id_allowed(0));
    assert!(damage_prize_loss_id_allowed(PrizeKind::Repel.protocol_id()));
}

#[test]
fn repel_velocity_pushes_nearby_targets_only() {
    let mut near_pos = vec2(10.0, 0.0);
    let mut near_vel = Vec2::ZERO;
    assert!(repel_velocity(
        Vec2::ZERO,
        &mut near_pos,
        &mut near_vel,
        REPEL_SPEED,
        REPEL_RADIUS,
    ));

    assert_eq!(near_pos, vec2(10.0, 0.0));
    assert!((near_vel - vec2(REPEL_SPEED, 0.0)).length() < 0.001);

    let mut edge_pos = vec2(REPEL_RADIUS, 0.0);
    let mut edge_vel = Vec2::ZERO;
    assert!(!repel_velocity(
        Vec2::ZERO,
        &mut edge_pos,
        &mut edge_vel,
        REPEL_SPEED,
        REPEL_RADIUS,
    ));
    assert_eq!(edge_vel, Vec2::ZERO);

    let mut far_pos = vec2(REPEL_RADIUS + 1.0, 0.0);
    let mut far_vel = Vec2::ZERO;
    assert!(!repel_velocity(
        Vec2::ZERO,
        &mut far_pos,
        &mut far_vel,
        REPEL_SPEED,
        REPEL_RADIUS,
    ));

    assert_eq!(far_pos, vec2(REPEL_RADIUS + 1.0, 0.0));
    assert_eq!(far_vel, Vec2::ZERO);
}

#[test]
fn repel_velocity_uses_reference_game_square_range_without_position_nudge() {
    let mut diagonal_pos = vec2(REPEL_RADIUS - 0.1, REPEL_RADIUS - 0.1);
    let mut diagonal_vel = Vec2::ZERO;
    assert!(repel_velocity(
        Vec2::ZERO,
        &mut diagonal_pos,
        &mut diagonal_vel,
        REPEL_SPEED,
        REPEL_RADIUS,
    ));

    assert_eq!(diagonal_pos, vec2(REPEL_RADIUS - 0.1, REPEL_RADIUS - 0.1));
    assert!(
        (diagonal_vel - vec2(REPEL_SPEED, REPEL_SPEED).normalize() * REPEL_SPEED).length() < 0.001
    );

    let mut edge_diagonal_pos = vec2(REPEL_RADIUS, REPEL_RADIUS);
    let mut edge_diagonal_vel = Vec2::ZERO;
    assert!(!repel_velocity(
        Vec2::ZERO,
        &mut edge_diagonal_pos,
        &mut edge_diagonal_vel,
        REPEL_SPEED,
        REPEL_RADIUS,
    ));
    assert_eq!(edge_diagonal_vel, Vec2::ZERO);
}

#[test]
fn repel_velocity_uses_arena_radius() {
    let mut pos = vec2(REPEL_RADIUS + 1.0, 0.0);
    let mut vel = Vec2::ZERO;
    assert!(repel_velocity(
        Vec2::ZERO,
        &mut pos,
        &mut vel,
        REPEL_SPEED,
        200.0
    ));

    assert_eq!(pos, vec2(REPEL_RADIUS + 1.0, 0.0));
    assert!((vel - vec2(REPEL_SPEED, 0.0)).length() < 0.001);
}

#[test]
fn repel_frequency_rule_matches_reference_game_enemy_only_weapons() {
    assert!(repel_can_affect_frequency(1, 2));
    assert!(!repel_can_affect_frequency(1, 1));
}

#[test]
fn repelled_shrapnel_lifetime_refresh_restarts_inactive_window_like_reference_game() {
    let mut ttl = 0.2;
    let mut age = SHRAPNEL_INACTIVE_DAMAGE_TIME + 0.1;

    refresh_repelled_shrapnel_lifetime(&mut ttl, &mut age, 1.0);

    assert_eq!(ttl, 1.0);
    assert_eq!(age, 0.0);
    assert_eq!(shrapnel_damage(age, 12.0, 40.0), 12.0);
    assert!(!shrapnel_cleanup_creates_impact(true, false, age));
}

#[test]
fn repelled_non_shrapnel_lifetime_refresh_preserves_animation_age() {
    let mut ttl = 0.2;
    let age = 0.6;

    refresh_repelled_weapon_lifetime(&mut ttl, 1.0);

    assert_eq!(ttl, 1.0);
    assert_eq!(age, 0.6);
}

#[test]
fn radar_bomb_visibility_matches_see_bomb_level_threshold() {
    assert!(!radar_bomb_visible(0, 4));
    assert!(radar_bomb_visible(1, 1));
    assert!(!radar_bomb_visible(2, 1));
    assert!(radar_bomb_visible(2, 2));
    assert!(radar_bomb_visible(3, 4));
}

#[test]
fn bot_bullet_shape_can_use_repel_velocity() {
    let mut bullet = BotBullet {
        owner_id: 0,
        pos: vec2(12.0, 0.0),
        vel: vec2(-100.0, 0.0),
        frequency: u16::MAX,
        ttl: BOT_BULLET_LIFETIME,
        age: 0.0,
        trail_timer: 0.0,
        level: WEAPON_LEVEL_MIN,
        damage: BOT_BULLET_DAMAGE,
        bouncing: false,
    };

    assert!(repel_velocity(
        Vec2::ZERO,
        &mut bullet.pos,
        &mut bullet.vel,
        REPEL_SPEED,
        REPEL_RADIUS,
    ));

    assert_eq!(bullet.pos, vec2(12.0, 0.0));
    assert!((bullet.vel - vec2(REPEL_SPEED, 0.0)).length() < 0.001);
}

#[test]
fn bot_bullet_motion_substeps_to_prevent_thin_wall_tunneling() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.set_tile(12, 10, 1);

    let mut bullet = BotBullet {
        owner_id: 0,
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        frequency: u16::MAX,
        ttl: 1.0,
        age: 0.0,
        trail_timer: 0.0,
        level: WEAPON_LEVEL_MIN,
        damage: 10.0,
        bouncing: false,
    };

    let (event, bounced_at) =
        update_bot_bullet_motion_with_contacts(&map, &[], &mut bullet, 0.1, None, &[]);

    assert!(matches!(event, Some(BotBulletMotionEvent::WallImpact(_))));
    assert_eq!(bounced_at, None);
    assert!(bullet.pos.x < 12.0 * TILE_SIZE);
    assert_eq!(bullet.ttl, 0.0);
    assert!(bullet.age < 0.1);
}

#[test]
fn bouncing_bot_bullets_bounce_instead_of_timing_out_on_walls() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.set_tile(12, 10, 1);

    let mut bullet = BotBullet {
        owner_id: 0,
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        frequency: u16::MAX,
        ttl: 1.0,
        age: 0.0,
        trail_timer: 0.0,
        level: WEAPON_LEVEL_MIN,
        damage: 10.0,
        bouncing: true,
    };

    let (event, bounced_at) =
        update_bot_bullet_motion_with_contacts(&map, &[], &mut bullet, 0.1, None, &[]);

    assert_eq!(event, None);
    assert!(bounced_at.is_some());
    assert!(bullet.pos.x < 12.0 * TILE_SIZE);
    assert!(bullet.vel.x < 0.0);
    assert_eq!(bullet.ttl, 0.9);
    assert_eq!(bullet.age, 0.1);
}

#[test]
fn bot_bullet_player_contact_uses_100hz_weapon_steps() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    let mut bullet = BotBullet {
        owner_id: 0,
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        frequency: u16::MAX,
        ttl: 1.0,
        age: 0.0,
        trail_timer: 0.0,
        level: WEAPON_LEVEL_MIN,
        damage: 10.0,
        bouncing: false,
    };
    let player_pos = vec2(11.75 * TILE_SIZE, 10.5 * TILE_SIZE);

    let (event, bounced_at) = update_bot_bullet_motion_with_contacts(
        &map,
        &[],
        &mut bullet,
        0.1,
        Some((player_pos, SHIP_RADIUS)),
        &[],
    );

    assert_eq!(bounced_at, None);
    assert!(matches!(event, Some(BotBulletMotionEvent::PlayerHit(pos)) if pos.x < player_pos.x));
    assert_eq!(bullet.ttl, 0.0);
    assert!(bullet.pos.x < 12.0 * TILE_SIZE);
    assert!(bullet.age < 0.1);
}

#[test]
fn local_bomb_direct_contact_uses_100hz_weapon_steps() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    let tuning = ArenaTuning::continuum_skirmish_default();
    let bots = vec![BotShip {
        id: 0,
        pos: vec2(11.25 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: Vec2::ZERO,
        orientation: 0.0,
        ship_type: 1,
        energy: BOT_ENERGY_MAX,
        max_energy: BOT_ENERGY_MAX,
        bounty: 0,
        kills: 0,
        deaths: 0,
        flag_goals: 0,
        fire_cooldown: 0.0,
        exhaust_timer: 0.0,
        ball_goals: 0,
        carrying_flag: false,
        super_timer: 0.0,
        shield_timer: 0.0,
        engine_shutdown_timer: 0.0,
        emp_shutdown_timer: 0.0,
        respawn_timer: 0.0,
        age: 0.0,
    }];
    let mut bomb = Bomb {
        prediction: None,
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        frequency: 0,
        ttl: 1.0,
        age: 0.0,
        trail_timer: 0.0,
        bounces_remaining: 0,
        level: WEAPON_LEVEL_MIN,
        damage: 100.0,
        emp_shutdown_time: 0.0,
        shrapnel: ExplosiveShrapnel {
            count: 0,
            level: 0,
            bouncing: false,
            rng_seed: 0,
        },
        proximity: false,
        proximity_sensor: None,
    };

    let event = update_bomb_motion(
        &mut bomb,
        0.1,
        &map,
        &[],
        &bots,
        &NetworkWorld::default(),
        0,
        tuning,
        None,
    );

    assert!(matches!(
        event,
        Some(BombMotionEvent::DirectHit {
            explosion_pos,
            damage_pos,
        }) if explosion_pos.x < bots[0].pos.x && damage_pos.x >= explosion_pos.x
    ));
    assert_eq!(bomb.ttl, 0.0);
    assert!(bomb.age < 0.1);
}

#[test]
fn all_explosive_contacts_preserve_shrapnel_like_reference_game_create_explosion() {
    let shrapnel = ExplosiveShrapnel {
        count: 4,
        level: 3,
        bouncing: true,
        rng_seed: 99,
    };
    let direct = BombMotionEvent::DirectHit {
        explosion_pos: vec2(10.0, 20.0),
        damage_pos: vec2(11.0, 20.0),
    };
    let proximity = BombMotionEvent::ProximityExplosion {
        explosion_pos: vec2(10.0, 20.0),
        damage_pos: vec2(11.0, 20.0),
    };
    let wall = BombMotionEvent::WallExplosion(vec2(10.0, 20.0));

    assert!(direct.spawns_shrapnel());
    assert_eq!(
        explosion_shrapnel_for_bomb_event(direct, shrapnel),
        shrapnel
    );
    assert!(proximity.spawns_shrapnel());
    assert_eq!(
        explosion_shrapnel_for_bomb_event(proximity, shrapnel),
        shrapnel
    );
    assert!(wall.spawns_shrapnel());
    assert_eq!(explosion_shrapnel_for_bomb_event(wall, shrapnel), shrapnel);

    assert!(RemoteExplosivePlayerEvent::DirectHit(vec2(1.0, 2.0)).spawns_shrapnel());
    assert!(RemoteExplosivePlayerEvent::ProximityExplosion(vec2(1.0, 2.0)).spawns_shrapnel());
}

#[test]
fn local_bomb_wall_explosion_wins_same_step_player_overlap() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.set_tile(11, 10, 1);
    let tuning = ArenaTuning::continuum_skirmish_default();
    let bots = vec![BotShip {
        id: 0,
        pos: vec2(10.95 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: Vec2::ZERO,
        orientation: 0.0,
        ship_type: 1,
        energy: BOT_ENERGY_MAX,
        max_energy: BOT_ENERGY_MAX,
        bounty: 0,
        kills: 0,
        deaths: 0,
        flag_goals: 0,
        fire_cooldown: 0.0,
        exhaust_timer: 0.0,
        ball_goals: 0,
        carrying_flag: false,
        super_timer: 0.0,
        shield_timer: 0.0,
        engine_shutdown_timer: 0.0,
        emp_shutdown_timer: 0.0,
        respawn_timer: 0.0,
        age: 0.0,
    }];
    let mut bomb = Bomb {
        prediction: None,
        pos: vec2(10.95 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        frequency: 0,
        ttl: 1.0,
        age: 0.0,
        trail_timer: 0.0,
        bounces_remaining: 0,
        level: WEAPON_LEVEL_MIN,
        damage: 100.0,
        emp_shutdown_time: 0.0,
        shrapnel: ExplosiveShrapnel {
            count: 0,
            level: 0,
            bouncing: false,
            rng_seed: 0,
        },
        proximity: false,
        proximity_sensor: None,
    };

    let event = update_bomb_motion(
        &mut bomb,
        0.1,
        &map,
        &[],
        &bots,
        &NetworkWorld::default(),
        0,
        tuning,
        None,
    );

    assert!(matches!(event, Some(BombMotionEvent::WallExplosion(_))));
    assert_eq!(bomb.ttl, 0.0);
}

#[test]
fn remote_thor_direct_contact_uses_100hz_weapon_steps() {
    let tuning = ArenaTuning::continuum_skirmish_default();
    let mut projectile = RemoteProjectile {
        source: remote_test_source(7),
        source_projectile_index: 0,
        authority_binding: None,
        presentation_clock: None,
        last_repel_mutation_event_id: None,
        authoritative_repel_state: None,
        frequency: 1,
        weapon_data: encode_weapon_data(8, WEAPON_LEVEL_MIN, false, WEAPON_LEVEL_MIN, 0, false),
        kind: RemoteProjectileKind::Thor,
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        ttl: 1.0,
        age: 0.1,
        trail_timer: 0.0,
        bounces_remaining: 0,
        proximity_sensor: None,
        level: WEAPON_LEVEL_MAX,
        damage: 100.0,
        inactive_damage: 0.0,
        radius: 72.0,
        emp_shutdown_time: 0.0,
        shrapnel_rng_seed: 0,
    };
    let player_pos = vec2(11.75 * TILE_SIZE, 10.5 * TILE_SIZE);

    let event =
        update_remote_thor_player_contact(&mut projectile, 0.1, player_pos, SHIP_RADIUS, tuning);

    assert!(
        matches!(event, Some(RemoteExplosivePlayerEvent::DirectHit(pos)) if pos.x < player_pos.x)
    );
    assert_eq!(projectile.ttl, 0.0);
    assert!(projectile.age < 0.1);
    assert!(projectile.pos.x < 12.0 * TILE_SIZE);
}

#[test]
fn remote_bomb_direct_contact_uses_100hz_weapon_steps() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    let tuning = ArenaTuning::continuum_skirmish_default();
    let mut projectile = RemoteProjectile {
        source: remote_test_source(7),
        source_projectile_index: 0,
        authority_binding: None,
        presentation_clock: None,
        last_repel_mutation_event_id: None,
        authoritative_repel_state: None,
        frequency: 1,
        weapon_data: encode_weapon_data(3, WEAPON_LEVEL_MIN, false, WEAPON_LEVEL_MIN, 0, false),
        kind: RemoteProjectileKind::Bomb { proximity: false },
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        ttl: 1.0,
        age: 0.1,
        trail_timer: 0.0,
        bounces_remaining: 0,
        proximity_sensor: None,
        level: WEAPON_LEVEL_MIN,
        damage: 100.0,
        inactive_damage: 0.0,
        radius: 72.0,
        emp_shutdown_time: 0.0,
        shrapnel_rng_seed: 0,
    };
    let player_pos = vec2(11.75 * TILE_SIZE, 10.5 * TILE_SIZE);

    let (event, bounced_at) = update_remote_bomb_motion(
        &mut projectile,
        0.1,
        &map,
        &[],
        player_pos,
        SHIP_RADIUS,
        tuning,
        None,
        true,
    );

    assert_eq!(bounced_at, None);
    assert!(matches!(
        event,
        Some(BombMotionEvent::DirectHit {
            explosion_pos,
            damage_pos,
        }) if explosion_pos.x < player_pos.x && damage_pos.x >= explosion_pos.x
    ));
    assert_eq!(projectile.ttl, 0.0);
    assert!(projectile.age < 0.1);
    assert!(projectile.pos.x < 12.0 * TILE_SIZE);
}

#[test]
fn remote_bomb_wall_explosion_wins_same_step_player_overlap() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.set_tile(11, 10, 1);
    let tuning = ArenaTuning::continuum_skirmish_default();
    let mut projectile = RemoteProjectile {
        source: remote_test_source(7),
        source_projectile_index: 0,
        authority_binding: None,
        presentation_clock: None,
        last_repel_mutation_event_id: None,
        authoritative_repel_state: None,
        frequency: 1,
        weapon_data: encode_weapon_data(3, WEAPON_LEVEL_MIN, false, WEAPON_LEVEL_MIN, 0, false),
        kind: RemoteProjectileKind::Bomb { proximity: false },
        pos: vec2(10.95 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        ttl: 1.0,
        age: 0.1,
        trail_timer: 0.0,
        bounces_remaining: 0,
        proximity_sensor: None,
        level: WEAPON_LEVEL_MIN,
        damage: 100.0,
        inactive_damage: 0.0,
        radius: 72.0,
        emp_shutdown_time: 0.0,
        shrapnel_rng_seed: 0,
    };

    let (event, bounced_at) = update_remote_bomb_motion(
        &mut projectile,
        0.1,
        &map,
        &[],
        vec2(10.95 * TILE_SIZE, 10.5 * TILE_SIZE),
        SHIP_RADIUS,
        tuning,
        None,
        true,
    );

    assert_eq!(bounced_at, None);
    assert!(matches!(event, Some(BombMotionEvent::WallExplosion(_))));
    assert_eq!(projectile.ttl, 0.0);
}

#[test]
fn remote_bullet_direct_contact_uses_100hz_weapon_steps() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    let mut projectile = RemoteProjectile {
        source: remote_test_source(7),
        source_projectile_index: 0,
        authority_binding: None,
        presentation_clock: None,
        last_repel_mutation_event_id: None,
        authoritative_repel_state: None,
        frequency: 1,
        weapon_data: encode_weapon_data(1, WEAPON_LEVEL_MIN, false, WEAPON_LEVEL_MIN, 0, false),
        kind: RemoteProjectileKind::Bullet { bouncing: false },
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        ttl: 1.0,
        age: 0.1,
        trail_timer: 0.0,
        bounces_remaining: 0,
        proximity_sensor: None,
        level: WEAPON_LEVEL_MIN,
        damage: 100.0,
        inactive_damage: 0.0,
        radius: 0.0,
        emp_shutdown_time: 0.0,
        shrapnel_rng_seed: 0,
    };
    let player_pos = vec2(11.75 * TILE_SIZE, 10.5 * TILE_SIZE);

    let (event, bounced_at) = update_remote_bullet_motion(
        &mut projectile,
        0.1,
        &map,
        &[],
        player_pos,
        SHIP_RADIUS,
        true,
    );

    assert_eq!(bounced_at, None);
    assert!(matches!(event, Some(RemoteBulletMotionEvent::PlayerHit(pos)) if pos.x < player_pos.x));
    assert_eq!(projectile.ttl, 0.0);
    assert!(projectile.pos.x < 12.0 * TILE_SIZE);
    assert!(projectile.age < 0.1);
}

#[test]
fn inactive_remote_burst_ignores_player_contact_until_first_wall_bounce() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    let mut projectile = RemoteProjectile {
        source: remote_test_source(7),
        source_projectile_index: 0,
        authority_binding: None,
        presentation_clock: None,
        last_repel_mutation_event_id: None,
        authoritative_repel_state: None,
        frequency: 1,
        weapon_data: encode_weapon_data(7, WEAPON_LEVEL_MIN, false, WEAPON_LEVEL_MIN, 0, false),
        kind: RemoteProjectileKind::Burst { active: false },
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        ttl: 1.0,
        age: 0.1,
        trail_timer: 0.0,
        bounces_remaining: 0,
        proximity_sensor: None,
        level: WEAPON_LEVEL_MIN,
        damage: 100.0,
        inactive_damage: 0.0,
        radius: 0.0,
        emp_shutdown_time: 0.0,
        shrapnel_rng_seed: 0,
    };
    let player_pos = vec2(11.75 * TILE_SIZE, 10.5 * TILE_SIZE);

    let (event, bounced_at) = update_remote_bullet_motion(
        &mut projectile,
        0.1,
        &map,
        &[],
        player_pos,
        SHIP_RADIUS,
        true,
    );

    assert_eq!(event, None);
    assert_eq!(bounced_at, None);
    assert_eq!(
        projectile.kind,
        RemoteProjectileKind::Burst { active: false }
    );
    assert!(projectile.ttl > 0.0);
    assert!(projectile.pos.x > player_pos.x);
}

#[test]
fn local_bullet_direct_contact_uses_100hz_weapon_steps() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    let tuning = ArenaTuning::continuum_skirmish_default();
    let bots = vec![BotShip {
        id: 0,
        pos: vec2(11.75 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: Vec2::ZERO,
        orientation: 0.0,
        ship_type: 1,
        energy: BOT_ENERGY_MAX,
        max_energy: BOT_ENERGY_MAX,
        bounty: 0,
        kills: 0,
        deaths: 0,
        flag_goals: 0,
        fire_cooldown: 0.0,
        exhaust_timer: 0.0,
        ball_goals: 0,
        carrying_flag: false,
        super_timer: 0.0,
        shield_timer: 0.0,
        engine_shutdown_timer: 0.0,
        emp_shutdown_timer: 0.0,
        respawn_timer: 0.0,
        age: 0.0,
    }];
    let mut bullet = Bullet {
        prediction: None,
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        frequency: 0,
        ttl: 1.0,
        age: 0.0,
        trail_timer: 0.0,
        bounces_remaining: 0,
        bouncing: false,
        activates_on_wall: false,
        active: true,
        level: WEAPON_LEVEL_MIN,
        damage: 100.0,
    };

    let network_world = NetworkWorld::default();
    let (event, bounced_at) = update_local_bullet_motion(
        &mut bullet,
        0.1,
        &map,
        &[],
        &bots,
        &network_world,
        0,
        tuning,
    );

    assert_eq!(bounced_at, None);
    assert!(
        matches!(event, Some(LocalBulletMotionEvent::BotHit { pos, bot_id: 0 }) if pos.x < bots[0].pos.x)
    );
    assert_eq!(bullet.ttl, 0.0);
    assert!(bullet.pos.x < 12.0 * TILE_SIZE);
    assert!(bullet.age < 0.1);
}

#[test]
fn local_shrapnel_direct_contact_uses_100hz_weapon_steps() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    let tuning = ArenaTuning::continuum_skirmish_default();
    let bots = vec![BotShip {
        id: 0,
        pos: vec2(11.75 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: Vec2::ZERO,
        orientation: 0.0,
        ship_type: 1,
        energy: BOT_ENERGY_MAX,
        max_energy: BOT_ENERGY_MAX,
        bounty: 0,
        kills: 0,
        deaths: 0,
        flag_goals: 0,
        fire_cooldown: 0.0,
        exhaust_timer: 0.0,
        ball_goals: 0,
        carrying_flag: false,
        super_timer: 0.0,
        shield_timer: 0.0,
        engine_shutdown_timer: 0.0,
        emp_shutdown_timer: 0.0,
        respawn_timer: 0.0,
        age: 0.0,
    }];
    let mut shrap = Shrapnel {
        level: 1,
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        frequency: 0,
        ttl: 1.0,
        age: 0.0,
        bouncing: false,
        damage: 100.0,
        inactive_damage: 10.0,
    };

    let network_world = NetworkWorld::default();
    let (event, bounced_at) =
        update_local_shrapnel_motion(&mut shrap, 0.1, &map, &[], &bots, &network_world, 0, tuning);

    assert_eq!(bounced_at, None);
    assert!(
        matches!(event, Some(LocalShrapnelMotionEvent::BotHit { pos, bot_id: 0 }) if pos.x < bots[0].pos.x)
    );
    assert_eq!(shrap.ttl, 0.0);
    assert!(shrap.pos.x < 12.0 * TILE_SIZE);
    assert!(shrap.age < 0.1);
}

#[test]
fn exhaust_particle_helper_spawns_twin_thruster_puffs() {
    let mut exhausts = Vec::new();
    spawn_exhaust_particles(&mut exhausts, Vec2::ZERO, vec2(0.0, -1.0), false, 20.0);

    assert_eq!(exhausts.len(), 2);
    assert!(exhausts.iter().all(|exhaust| exhaust.vel.y > 0.0));
    assert!(exhausts[0].pos.distance(exhausts[1].pos) > 0.0);
    assert!(
        exhausts
            .iter()
            .all(|exhaust| (exhaust.pos.y - 12.0).abs() < 0.001)
    );
}

#[test]
fn visual_explosions_do_not_damage_bots() {
    let damaging = Explosion {
        prediction: None,
        pos: Vec2::ZERO,
        damage_pos: Vec2::ZERO,
        age: 0.0,
        kind: ExplosionKind::Bomb,
        damages_bots: true,
        damage: 260.0,
        radius: 72.0,
        emp_shutdown_time: 0.0,
        remote_source: None,
    };
    let visual = Explosion {
        prediction: None,
        pos: Vec2::ZERO,
        damage_pos: Vec2::ZERO,
        age: 0.0,
        kind: ExplosionKind::Bomb,
        damages_bots: false,
        damage: 260.0,
        radius: 72.0,
        emp_shutdown_time: 0.0,
        remote_source: None,
    };

    assert!(explosion_bot_damage(&damaging, Vec2::ZERO) > 0.0);
    assert_eq!(explosion_bot_damage(&visual, Vec2::ZERO), 0.0);
    assert_eq!(explosion_bot_damage(&damaging, vec2(72.0, 0.0)), 0.0);
}

#[test]
fn initial_bots_spawn_open_and_use_ship_slots() {
    let map = ArenaMap::fallback_skirmish();
    let player_pos = map.spawn_position();
    let tuning = ArenaTuning::continuum_skirmish_default();
    let bots = spawn_initial_bots(&map, player_pos, tuning);

    assert_eq!(bots.len(), BOT_COUNT);
    for bot in bots {
        assert!(bot.ship_type < 8);
        assert_eq!(bot.max_energy, bot_spawn_energy(tuning.ship(bot.ship_type)));
        assert_eq!(bot.energy, bot.max_energy);
        assert!(!map.is_solid_world(bot.pos));
    }
}

#[test]
fn initial_bots_use_active_arena_ship_energy() {
    let map = ArenaMap::fallback_skirmish();
    let player_pos = map.spawn_position();
    let tuning = arena_tuning_from_subgame_settings_text(include_str!(
        "../../assets/zones/chaos-zone/arena.conf"
    ))
    .expect("chaos-zone settings should parse");
    let bots = spawn_initial_bots(&map, player_pos, tuning);

    for bot in bots {
        assert_eq!(bot.max_energy, 1700.0);
        assert_eq!(bot.energy, 1700.0);
    }
}

#[test]
fn chaos_arena_damage_and_health_settings_drive_combat_helpers() {
    let tuning = arena_tuning_from_subgame_settings_text(include_str!(
        "../../assets/zones/chaos-zone/arena.conf"
    ))
    .expect("chaos-zone settings should parse");

    let warbird = tuning.ship(0);
    assert_eq!(warbird.energy_initial, 1700.0);
    assert_eq!(warbird.energy_max, 1700.0);
    assert_eq!(tuning.bullet_damage, 200.0);
    assert_eq!(tuning.bullet_damage_upgrade, 100.0);
    assert_eq!(tuning.bomb_damage, 750.0);
    assert_eq!(tuning.burst_damage, 700.0);
    assert_eq!(tuning.inactive_shrapnel_damage, 3.0);
    assert_eq!(tuning.shrapnel_damage_multiplier, 1.0);
    assert_eq!(tuning.bouncing_bomb_damage_multiplier, 1.0);
    assert_eq!(tuning.emp_bomb_damage_multiplier, 1.0);
    assert_eq!(tuning.proximity_distance, 3.0);
    assert_eq!(tuning.bomb_explode_pixels, 80.0);

    assert_eq!(
        leveled_weapon_damage(tuning.bullet_damage, tuning.bullet_damage_upgrade, 1),
        200.0
    );
    assert_eq!(
        leveled_weapon_damage(tuning.bullet_damage, tuning.bullet_damage_upgrade, 4),
        500.0
    );
    assert_eq!(
        bomb_damage_for_ship(
            tuning.bomb_damage,
            tuning.bouncing_bomb_damage_multiplier,
            tuning.emp_bomb_damage_multiplier,
            0,
            false
        ),
        750.0
    );
    assert_eq!(
        bomb_damage_for_ship(
            tuning.bomb_damage,
            tuning.bouncing_bomb_damage_multiplier,
            tuning.emp_bomb_damage_multiplier,
            1,
            true
        ),
        750.0
    );

    let bomb_radius = bomb_explosion_radius(tuning.bomb_explode_pixels, 3);
    assert_eq!(bomb_radius, 240.0);
    assert_eq!(radial_damage(0.0, bomb_radius, tuning.bomb_damage), 750.0);
    assert_eq!(
        radial_damage(bomb_radius / 2.0, bomb_radius, tuning.bomb_damage),
        375.0
    );
    assert_eq!(
        radial_damage_with_shooter_proximity_reduction(
            0.0,
            Some(0.0),
            bomb_radius,
            tuning.bomb_damage
        ),
        375.0
    );
    // The reference game's formula: prox = ProximityDistance + raw_level (0..=3 bit
    // value), so a level-3 bomb adds raw_level=2 to ProximityDistance=3.
    assert_eq!(
        proximity_bomb_trigger_radius(tuning.proximity_distance, 3),
        (3.0 + 2.0) * PROXIMITY_BOMB_TRIGGER_SCALE
    );

    assert_eq!(thor_damage(tuning.bomb_damage, 2), 3750.0);
    assert_eq!(
        thor_damage_for_ship(
            tuning.bomb_damage,
            tuning.bouncing_bomb_damage_multiplier,
            1,
            2
        ),
        3750.0
    );

    let inactive = inactive_shrapnel_damage(tuning.inactive_shrapnel_damage, 4);
    let active = active_shrapnel_damage(
        tuning.bullet_damage,
        tuning.bullet_damage_upgrade,
        tuning.shrapnel_damage_multiplier,
        4,
    );
    assert_eq!(inactive, 9.0);
    assert_eq!(active, 500.0);
    assert_eq!(
        shrapnel_damage(SHRAPNEL_INACTIVE_DAMAGE_TIME, inactive, active),
        inactive
    );
    assert_eq!(
        shrapnel_damage(SHRAPNEL_INACTIVE_DAMAGE_TIME + 0.001, inactive, active),
        active
    );
}

#[test]
fn chaos_arena_weapon_motion_and_lifetimes_drive_runtime_helpers() {
    let tuning = arena_tuning_from_subgame_settings_text(include_str!(
        "../../assets/zones/chaos-zone/arena.conf"
    ))
    .expect("chaos-zone settings should parse");

    let warbird = tuning.ship(0);
    assert_eq!(warbird.bullet_speed, 200.0);
    assert_eq!(warbird.burst_speed, 300.0);
    assert_eq!(warbird.burst_bullet_count, 24);
    assert_eq!(warbird.bomb_speed, 200.0);
    assert_eq!(warbird.thor_speed, 200.0);
    assert_eq!(warbird.bomb_recoil_speed, 40.0);
    assert_eq!(warbird.rocket_thrust_acceleration, 250.0);
    assert_eq!(warbird.rocket_top_speed, 550.0);
    assert_eq!(warbird.rocket_lifetime, 10.0);
    assert_eq!(warbird.initial_repels, 0);
    assert_eq!(warbird.max_repels, 3);
    assert_eq!(warbird.initial_bursts, 0);
    assert_eq!(warbird.max_bursts, 3);
    assert_eq!(warbird.initial_decoys, 0);
    assert_eq!(warbird.max_decoys, 3);
    assert_eq!(warbird.initial_thors, 0);
    assert_eq!(warbird.max_thors, 3);
    assert_eq!(warbird.initial_rockets, 0);
    assert_eq!(warbird.max_rockets, 3);
    assert_eq!(warbird.initial_portals, 0);
    assert_eq!(warbird.max_portals, 3);

    assert_eq!(tuning.bullet_lifetime, 5.5);
    assert_eq!(tuning.bomb_lifetime, 80.0);
    assert_eq!(tuning.mine_lifetime, 120.0);
    assert_eq!(tuning.decoy_lifetime, 15.0);
    assert_eq!(tuning.brick_lifetime, 120.0);
    assert_eq!(tuning.portal_lifetime, 20.0);
    assert_eq!(tuning.bomb_explode_delay_seconds, 1.5);
    assert_eq!(tuning.repel_lifetime, 2.25);
    assert_eq!(tuning.repel_radius, 512.0);
    assert_eq!(tuning.repel_speed, 500.0);
    assert_eq!(tuning.shrapnel_speed, 300.0);

    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Bullet),
        tuning.bullet_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Burst),
        tuning.bullet_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Shrapnel),
        tuning.bullet_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Bomb),
        tuning.bomb_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Thor),
        tuning.bomb_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Mine),
        tuning.mine_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Decoy),
        tuning.decoy_lifetime
    );
}

#[test]
fn prize_spawns_require_open_space_and_capacity() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.tiles[10 * map.width + 10] = 1;
    map.tiles[11 * map.width + 10] = SAFE_TILE_ID;

    assert!(can_spawn_prize_at(
        &map,
        0,
        MAX_PRIZES,
        vec2(12.5 * TILE_SIZE, 10.5 * TILE_SIZE)
    ));
    assert!(!can_spawn_prize_at(
        &map,
        0,
        MAX_PRIZES,
        vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE)
    ));
    assert!(!map.is_solid_world(vec2(10.5 * TILE_SIZE, 11.5 * TILE_SIZE)));
    assert!(!can_spawn_prize_at(
        &map,
        0,
        MAX_PRIZES,
        vec2(10.5 * TILE_SIZE, 11.5 * TILE_SIZE)
    ));
    assert!(!can_spawn_prize_at(
        &map,
        MAX_PRIZES,
        MAX_PRIZES,
        vec2(12.5 * TILE_SIZE, 10.5 * TILE_SIZE)
    ));
}

#[test]
fn max_hidden_prizes_follow_reference_game_prize_factor_cap() {
    assert_eq!(max_hidden_prize_count(1000, 0), 0);
    assert_eq!(max_hidden_prize_count(1000, 1), 1);
    assert_eq!(max_hidden_prize_count(1000, 4), 4);
    assert_eq!(max_hidden_prize_count(250, 3), 0);
    assert_eq!(max_hidden_prize_count(10_000, 2), 20);
    assert_eq!(
        max_hidden_prize_count(u16::MAX, 100),
        CONTINUUM_MAX_GREEN_COUNT
    );
}

#[test]
fn warp_prize_preserves_energy_but_manual_spawn_warp_drains() {
    let from = vec2(10.0, 0.0);
    let destination = vec2(20.0, 0.0);

    assert_eq!(
        warp_energy_after(900.0, true, false, from, destination),
        1.0
    );
    assert_eq!(
        warp_energy_after(900.0, false, false, from, destination),
        900.0
    );
    assert_eq!(
        warp_energy_after(900.0, true, true, from, destination),
        900.0
    );
}

#[test]
fn warp_prize_uses_spawn_destination_and_stops_even_with_portal() {
    let spawn = vec2(20.0, 0.0);
    let portal = vec2(100.0, 100.0);
    let velocity = vec2(12.0, -4.0);

    assert_eq!(warp_prize_destination(spawn, Some(portal)), spawn);
    assert_eq!(warp_prize_velocity_after(velocity), Vec2::ZERO);
}

#[test]
fn portal_warp_preserves_velocity_but_spawn_warp_stops_ship() {
    let velocity = vec2(12.0, -4.0);

    assert_eq!(warp_velocity_after(velocity, true), velocity);
    assert_eq!(warp_velocity_after(velocity, false), Vec2::ZERO);
}

#[test]
fn portal_place_available_only_depends_on_inventory_and_antiwarp() {
    assert!(portal_place_available(1, 0.0));
    assert!(portal_place_available(3, 0.0));
    assert!(!portal_place_available(0, 0.0));
    assert!(!portal_place_available(1, 0.01));
}

#[test]
fn warp_requires_full_energy_without_portal_and_respects_settle_delay() {
    assert!(warp_available(false, 1000.0, 1000.0, 0.0, 0.0));
    assert!(!warp_available(false, 999.0, 1000.0, 0.0, 0.0));
    assert!(!warp_available(false, 1000.0, 1000.0, 0.0, 0.01));
    assert!(warp_available(true, 100.0, 1000.0, 0.0, 0.5));
    assert!(!warp_available(true, 1000.0, 1000.0, 0.01, 0.0));
    assert_eq!(
        manual_warp_bomb_cooldown_after(0.0, REPEL_REUSE_COOLDOWN),
        REPEL_REUSE_COOLDOWN
    );
    assert_eq!(manual_warp_bomb_cooldown_after(0.75, 0.5), 0.75);
    assert!(antiwarp_settle_active(0.01));
    assert!(!antiwarp_settle_active(0.0));
}

#[test]
fn mines_trigger_on_active_bots_only() {
    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.ships[1].radius_pixels = 14.0;
    let mut bots = vec![
        BotShip {
            id: 0,
            pos: vec2(explosive_contact_radius(14.0), 0.0),
            vel: Vec2::ZERO,
            orientation: 0.0,
            ship_type: 1,
            energy: BOT_ENERGY_MAX,
            max_energy: BOT_ENERGY_MAX,
            bounty: 0,
            kills: 0,
            deaths: 0,
            flag_goals: 0,
            ball_goals: 0,
            carrying_flag: false,
            super_timer: 0.0,
            shield_timer: 0.0,
            engine_shutdown_timer: 0.0,
            emp_shutdown_timer: 0.0,
            fire_cooldown: 0.0,
            exhaust_timer: 0.0,
            respawn_timer: 0.0,
            age: 0.0,
        },
        BotShip {
            id: 1,
            pos: Vec2::ZERO,
            vel: Vec2::ZERO,
            orientation: 0.0,
            ship_type: 2,
            energy: 0.0,
            max_energy: BOT_ENERGY_MAX,
            bounty: 0,
            kills: 0,
            deaths: 1,
            flag_goals: 0,
            ball_goals: 0,
            carrying_flag: false,
            super_timer: 0.0,
            shield_timer: 0.0,
            engine_shutdown_timer: 0.0,
            emp_shutdown_timer: 0.0,
            fire_cooldown: 0.0,
            exhaust_timer: 0.0,
            respawn_timer: 1.0,
            age: 0.0,
        },
    ];

    // A ship on the positive exact boundary touches the strict lower edge.
    assert!(!mine_triggered_by_bot(Vec2::ZERO, &bots, tuning));
    bots[0].pos = vec2(explosive_contact_radius(14.0) - 1.0, 0.0);
    assert!(mine_triggered_by_bot(Vec2::ZERO, &bots, tuning));
    bots[0].pos = vec2(-explosive_contact_radius(14.0), 0.0);
    assert!(mine_triggered_by_bot(Vec2::ZERO, &bots, tuning));

    bots[0].pos = vec2(explosive_contact_radius(14.0) + 1.0, 0.0);
    assert!(!mine_triggered_by_bot(Vec2::ZERO, &bots, tuning));
}

#[test]
fn proximity_bombs_trigger_on_active_bots_only() {
    let mut bots = vec![
        BotShip {
            id: 0,
            pos: vec2(PROXIMITY_BOMB_TRIGGER_RADIUS - 1.0, 0.0),
            vel: Vec2::ZERO,
            orientation: 0.0,
            ship_type: 1,
            energy: BOT_ENERGY_MAX,
            max_energy: BOT_ENERGY_MAX,
            bounty: 0,
            kills: 0,
            deaths: 0,
            flag_goals: 0,
            ball_goals: 0,
            carrying_flag: false,
            super_timer: 0.0,
            shield_timer: 0.0,
            engine_shutdown_timer: 0.0,
            emp_shutdown_timer: 0.0,
            fire_cooldown: 0.0,
            exhaust_timer: 0.0,
            respawn_timer: 0.0,
            age: 0.0,
        },
        BotShip {
            id: 1,
            pos: Vec2::ZERO,
            vel: Vec2::ZERO,
            orientation: 0.0,
            ship_type: 2,
            energy: 0.0,
            max_energy: BOT_ENERGY_MAX,
            bounty: 0,
            kills: 0,
            deaths: 1,
            flag_goals: 0,
            ball_goals: 0,
            carrying_flag: false,
            super_timer: 0.0,
            shield_timer: 0.0,
            engine_shutdown_timer: 0.0,
            emp_shutdown_timer: 0.0,
            fire_cooldown: 0.0,
            exhaust_timer: 0.0,
            respawn_timer: 1.0,
            age: 0.0,
        },
    ];

    let radius = proximity_bomb_trigger_radius(3.0, 1);
    assert_eq!(radius, PROXIMITY_BOMB_TRIGGER_RADIUS);

    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.ships[1].radius_pixels = 14.0;
    assert_eq!(proximity_sensor_contact_radius(radius, 14.0), radius);
    assert!(proximity_bomb_triggered_by_bot(
        Vec2::ZERO,
        &bots,
        tuning,
        radius
    ));
    bots[0].pos = vec2(radius, radius);
    assert!(!proximity_bomb_triggered_by_bot(
        Vec2::ZERO,
        &bots,
        tuning,
        radius
    ));
    bots[0].pos = vec2(-radius, -radius);
    assert!(proximity_bomb_triggered_by_bot(
        Vec2::ZERO,
        &bots,
        tuning,
        radius
    ));
    bots[0].pos = vec2(radius + 1.0, 0.0);
    assert!(!proximity_bomb_triggered_by_bot(
        Vec2::ZERO,
        &bots,
        tuning,
        radius
    ));
    bots[0].pos = vec2(explosive_contact_radius(14.0), 0.0);
    assert!(!explosive_contact_triggered_by_bot(
        Vec2::ZERO,
        &bots,
        tuning
    ));
    bots[0].pos = vec2(-explosive_contact_radius(14.0), 0.0);
    assert!(explosive_contact_triggered_by_bot(
        Vec2::ZERO,
        &bots,
        tuning
    ));
    bots[0].pos = vec2(explosive_contact_radius(14.0) + 1.0, 0.0);
    assert!(!explosive_contact_triggered_by_bot(
        Vec2::ZERO,
        &bots,
        tuning
    ));

    bots[0].pos = vec2(PROXIMITY_BOMB_TRIGGER_RADIUS + 1.0, 0.0);
    assert!(!proximity_bomb_triggered_by_bot(
        Vec2::ZERO,
        &bots,
        tuning,
        radius
    ));

    tuning.ships[1].radius_pixels = 20.0;
    bots[0].pos = vec2(proximity_sensor_contact_radius(radius, 20.0) - 0.1, 0.0);
    assert!(proximity_bomb_triggered_by_bot(
        Vec2::ZERO,
        &bots,
        tuning,
        radius
    ));
}

#[test]
fn bomb_safety_blocks_nearby_proximity_bomb_firing_only() {
    let mut network_world = NetworkWorld::default();
    let mut enemy = NetworkPlayer::from_roster(&PlayerRosterEntry {
        ship: 1,
        audio: 0,
        name: "enemy".to_string(),
        squad: String::new(),
        kill_points: 0,
        flag_points: 0,
        player_id: 9,
        frequency: 2,
        wins: 0,
        losses: 0,
        attach_parent: 0xffff,
        flags: 0,
        koth: 0,
    });
    enemy.position = Some(vec2(PROXIMITY_BOMB_TRIGGER_RADIUS - 1.0, 0.0));
    network_world.players.insert(enemy.player_id, enemy);

    let bots = vec![BotShip {
        id: 0,
        pos: vec2(PROXIMITY_BOMB_TRIGGER_RADIUS + 100.0, 0.0),
        vel: Vec2::ZERO,
        orientation: 0.0,
        ship_type: 1,
        energy: BOT_ENERGY_MAX,
        max_energy: BOT_ENERGY_MAX,
        bounty: 0,
        kills: 0,
        deaths: 0,
        flag_goals: 0,
        ball_goals: 0,
        carrying_flag: false,
        super_timer: 0.0,
        shield_timer: 0.0,
        engine_shutdown_timer: 0.0,
        emp_shutdown_timer: 0.0,
        fire_cooldown: 0.0,
        exhaust_timer: 0.0,
        respawn_timer: 0.0,
        age: 0.0,
    }];
    let radius = proximity_bomb_trigger_radius(3.0, 1);

    assert!(!proximity_bomb_safe_to_fire(
        true,
        true,
        Vec2::ZERO,
        &bots,
        &network_world,
        1,
        ArenaTuning::continuum_skirmish_default(),
        radius
    ));
    assert!(proximity_bomb_safe_to_fire(
        false,
        true,
        Vec2::ZERO,
        &bots,
        &network_world,
        1,
        ArenaTuning::continuum_skirmish_default(),
        radius
    ));
    assert!(proximity_bomb_safe_to_fire(
        true,
        false,
        Vec2::ZERO,
        &bots,
        &network_world,
        1,
        ArenaTuning::continuum_skirmish_default(),
        radius
    ));

    network_world
        .players
        .get_mut(&9)
        .expect("enemy should exist")
        .position = Some(vec2(radius, radius));
    assert!(proximity_bomb_safety_blocks_ship(
        Vec2::ZERO,
        vec2(radius, 0.0),
        radius
    ));
    assert!(!proximity_bomb_safety_blocks_ship(
        Vec2::ZERO,
        vec2(radius, radius),
        radius
    ));
    assert!(proximity_bomb_safe_to_fire(
        true,
        true,
        Vec2::ZERO,
        &bots,
        &network_world,
        1,
        ArenaTuning::continuum_skirmish_default(),
        radius
    ));

    let mut same_frequency_world = network_world.clone();
    same_frequency_world
        .players
        .get_mut(&9)
        .expect("enemy should exist")
        .frequency = 1;
    assert!(proximity_bomb_safe_to_fire(
        true,
        true,
        Vec2::ZERO,
        &bots,
        &same_frequency_world,
        1,
        ArenaTuning::continuum_skirmish_default(),
        radius
    ));
}

#[test]
fn network_players_trigger_local_proximity_weapons_like_reference_game_targets() {
    let mut world = NetworkWorld::default();
    let mut enemy = NetworkPlayer::from_roster(&PlayerRosterEntry {
        ship: 1,
        audio: 0,
        name: "enemy".to_string(),
        squad: String::new(),
        kill_points: 0,
        flag_points: 0,
        player_id: 42,
        frequency: 2,
        wins: 0,
        losses: 0,
        attach_parent: 0xffff,
        flags: 0,
        koth: 0,
    });
    enemy.position = Some(vec2(PROXIMITY_BOMB_TRIGGER_RADIUS - 1.0, 0.0));
    world.players.insert(enemy.player_id, enemy);

    let radius = proximity_bomb_trigger_radius(3.0, 1);
    let trigger = proximity_bomb_trigger_by_network_player(
        None,
        Vec2::ZERO,
        &world,
        1,
        ArenaTuning::continuum_skirmish_default(),
        radius,
    )
    .expect("enemy network player should trip proximity");
    assert_eq!(trigger.target_id, 42);
    world
        .players
        .get_mut(&42)
        .expect("enemy should exist")
        .position = Some(vec2(radius, radius));
    assert!(
        proximity_bomb_trigger_by_network_player(
            None,
            Vec2::ZERO,
            &world,
            1,
            ArenaTuning::continuum_skirmish_default(),
            radius,
        )
        .is_none()
    );
    world.players.get_mut(&42).unwrap().position = Some(vec2(-radius, -radius));
    assert!(
        proximity_bomb_trigger_by_network_player(
            None,
            Vec2::ZERO,
            &world,
            1,
            ArenaTuning::continuum_skirmish_default(),
            radius
        )
        .is_some()
    );
    world
        .players
        .get_mut(&42)
        .expect("enemy should exist")
        .position = Some(vec2(1.0, 0.0));
    assert!(explosive_contact_triggered_by_network_player(
        Vec2::ZERO,
        &world,
        1,
        ArenaTuning::continuum_skirmish_default()
    ));

    let tracked = proximity_bomb_trigger_by_network_player(
        Some(ProximitySensorState {
            target_id: 42,
            highest_axis_offset: trigger.highest_axis_offset,
            timer: 0.1,
        }),
        Vec2::ZERO,
        &world,
        1,
        ArenaTuning::continuum_skirmish_default(),
        radius,
    )
    .expect("tracked enemy should keep sensor active");
    assert_eq!(tracked.target_id, 42);

    world
        .players
        .get_mut(&42)
        .expect("enemy should exist")
        .frequency = 1;
    assert!(
        proximity_bomb_trigger_by_network_player(
            None,
            Vec2::ZERO,
            &world,
            1,
            ArenaTuning::continuum_skirmish_default(),
            radius
        )
        .is_none()
    );
    assert!(!explosive_contact_triggered_by_network_player(
        Vec2::ZERO,
        &world,
        1,
        ArenaTuning::continuum_skirmish_default()
    ));
}

#[test]
fn proximity_bomb_sensor_waits_for_explode_delay() {
    let mut sensor = None;
    let trigger = proximity_sensor_trigger(Vec2::ZERO, 7, vec2(10.0, 0.0));

    assert!(!proximity_bomb_sensor_exploded(
        &mut sensor,
        Some(trigger),
        1.0,
        0.1,
        0.25
    ));
    assert!((sensor.unwrap().timer - 0.15).abs() < 0.001);
    assert!(proximity_bomb_sensor_exploded(
        &mut sensor,
        Some(trigger),
        1.1,
        0.15,
        0.25
    ));
}

#[test]
fn proximity_bomb_sensor_explodes_immediately_on_initial_overlap() {
    let mut sensor = None;

    assert!(proximity_bomb_sensor_exploded(
        &mut sensor,
        Some(proximity_sensor_trigger(Vec2::ZERO, 7, vec2(10.0, 0.0))),
        0.05,
        0.05,
        0.25
    ));
}

#[test]
fn proximity_bomb_sensor_explodes_when_tracked_target_moves_away() {
    let mut sensor = None;

    assert!(!proximity_bomb_sensor_exploded(
        &mut sensor,
        Some(proximity_sensor_trigger(Vec2::ZERO, 7, vec2(10.0, 0.0))),
        1.0,
        0.05,
        0.25
    ));
    assert!(!proximity_bomb_sensor_exploded(
        &mut sensor,
        Some(proximity_sensor_trigger(Vec2::ZERO, 7, vec2(8.0, 0.0))),
        1.05,
        0.05,
        0.25
    ));
    assert!(proximity_bomb_sensor_exploded(
        &mut sensor,
        Some(proximity_sensor_trigger(Vec2::ZERO, 7, vec2(9.0, 0.0))),
        1.10,
        0.05,
        0.25
    ));
}

#[test]
fn thor_proximity_radius_adds_reference_bonus() {
    assert_eq!(thor_proximity_trigger_radius(3.0, 0), 108.0);
    assert_eq!(thor_proximity_trigger_radius(3.0, 2), 144.0);
}

#[test]
fn local_thor_proximity_uses_reference_game_100hz_weapon_steps() {
    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.proximity_distance = 3.0;
    tuning.bomb_explode_delay_seconds = 0.25;
    let map = empty_test_map();
    let bot = BotShip {
        id: 7,
        pos: vec2(182.0, 160.0),
        vel: Vec2::ZERO,
        orientation: 0.0,
        ship_type: 0,
        energy: 100.0,
        max_energy: BOT_ENERGY_MAX,
        bounty: 0,
        kills: 0,
        deaths: 0,
        flag_goals: 0,
        fire_cooldown: 0.0,
        exhaust_timer: 0.0,
        ball_goals: 0,
        carrying_flag: false,
        super_timer: 0.0,
        shield_timer: 0.0,
        engine_shutdown_timer: 0.0,
        emp_shutdown_timer: 0.0,
        respawn_timer: 0.0,
        age: 0.0,
    };
    let mut thor = Thor {
        prediction: None,
        pos: vec2(32.0, 160.0),
        vel: vec2(10_000.0, 0.0),
        ttl: 1.0,
        age: 0.0,
        damage: 260.0,
        shrapnel: ExplosiveShrapnel {
            count: 0,
            level: 0,
            bouncing: false,
            rng_seed: 0,
        },
        proximity_sensor: None,
        raw_level: 0,
    };

    let explosion = update_thor_motion(
        &mut thor,
        0.03,
        &map,
        &[bot],
        &NetworkWorld::default(),
        0,
        tuning,
    )
    .expect("sub-stepped Thor should detect the crossed target");

    assert_eq!(explosion.0, vec2(32.0, 160.0));
    assert_eq!(explosion.1, 260.0);
    assert_eq!(thor.ttl, 0.0);
}

#[test]
fn local_thor_direct_contact_uses_100hz_weapon_steps() {
    let tuning = ArenaTuning::continuum_skirmish_default();
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    let bot_pos = vec2(11.75 * TILE_SIZE, 10.5 * TILE_SIZE);
    let bot = BotShip {
        id: 7,
        pos: bot_pos,
        vel: Vec2::ZERO,
        orientation: 0.0,
        ship_type: 0,
        energy: 100.0,
        max_energy: BOT_ENERGY_MAX,
        bounty: 0,
        kills: 0,
        deaths: 0,
        flag_goals: 0,
        fire_cooldown: 0.0,
        exhaust_timer: 0.0,
        ball_goals: 0,
        carrying_flag: false,
        super_timer: 0.0,
        shield_timer: 0.0,
        engine_shutdown_timer: 0.0,
        emp_shutdown_timer: 0.0,
        respawn_timer: 0.0,
        age: 0.0,
    };
    let mut thor = Thor {
        prediction: None,
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(640.0, 0.0),
        ttl: 1.0,
        age: 0.0,
        damage: 260.0,
        shrapnel: ExplosiveShrapnel {
            count: 0,
            level: 0,
            bouncing: false,
            rng_seed: 0,
        },
        proximity_sensor: None,
        raw_level: 0,
    };

    let explosion = update_thor_motion(
        &mut thor,
        0.1,
        &map,
        &[bot],
        &NetworkWorld::default(),
        0,
        tuning,
    )
    .expect("sub-stepped Thor should detect direct contact");

    assert!(explosion.0.x < bot_pos.x);
    assert_eq!(explosion.1, 260.0);
    assert_eq!(thor.ttl, 0.0);
    assert!(thor.age < 0.1);
}

#[test]
fn weapon_lifetimes_follow_reference_game_reference_groups() {
    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.bullet_lifetime = 1.25;
    tuning.bomb_lifetime = 2.4;
    tuning.mine_lifetime = 45.0;
    tuning.decoy_lifetime = 12.0;

    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Bullet),
        tuning.bullet_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Burst),
        tuning.bullet_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Shrapnel),
        tuning.bullet_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Bomb),
        tuning.bomb_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Thor),
        tuning.bomb_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Mine),
        tuning.mine_lifetime
    );
    assert_eq!(
        weapon_lifetime(&tuning, WeaponLifetimeKind::Decoy),
        tuning.decoy_lifetime
    );
}

#[test]
fn weapon_owner_safe_zone_cleanup_uses_owner_position_tile() {
    let mut map = empty_test_map();
    map.tiles[12 * map.width + 11] = SAFE_TILE_ID;
    let safe_pos = vec2(11.5 * TILE_SIZE, 12.5 * TILE_SIZE);
    let open_pos = vec2(10.5 * TILE_SIZE, 12.5 * TILE_SIZE);

    assert!(weapon_owner_position_in_safe_tile(&map, Some(safe_pos)));
    assert!(!weapon_owner_position_in_safe_tile(&map, Some(open_pos)));
    assert!(!weapon_owner_position_in_safe_tile(&map, None));
}

#[test]
fn weapons_time_out_on_wormhole_tiles_like_reference_game() {
    let mut map = empty_test_map();
    map.tiles[12 * map.width + 11] = WORMHOLE_TILE_ID;
    let wormhole_pos = vec2(11.5 * TILE_SIZE, 12.5 * TILE_SIZE);
    let open_pos = vec2(10.5 * TILE_SIZE, 12.5 * TILE_SIZE);

    assert!(weapon_times_out_on_wormhole_tile(&map, wormhole_pos));
    assert!(!weapon_times_out_on_wormhole_tile(&map, open_pos));
}

#[test]
fn bomb_explosion_radius_scales_from_arena_pixels_by_level() {
    assert_eq!(bomb_explosion_radius(72.0, 0), 72.0);
    assert_eq!(bomb_explosion_radius(72.0, 1), 72.0);
    assert_eq!(bomb_explosion_radius(72.0, 2), 144.0);
    assert_eq!(bomb_explosion_radius(72.0, 4), 288.0);
}

#[test]
fn thor_damage_uses_reference_bomb_damage_formula() {
    assert_eq!(thor_damage(260.0, 0), 260.0);
    assert_eq!(thor_damage(260.0, 2), 1300.0);
    assert_eq!(thor_damage_for_ship(260.0, 0.75, 0, 2), 1300.0);
    assert_eq!(thor_damage_for_ship(260.0, 0.75, 1, 2), 975.0);
}

#[test]
fn bomb_damage_uses_arena_percent_for_bounceable_and_emp_bombs() {
    assert_eq!(bomb_damage_for_ship(260.0, 0.75, 0.6, 0, false), 260.0);
    assert_eq!(bomb_damage_for_ship(260.0, 0.75, 0.6, 1, false), 195.0);
    assert_eq!(bomb_damage_for_ship(260.0, 0.75, 0.6, 0, true), 156.0);
    assert_eq!(bomb_damage_for_ship(260.0, 0.75, 0.6, 1, true), 117.0);
    assert_eq!(bomb_damage_for_ship(261.0, 0.75, 0.6, 1, false), 195.0);
    assert_eq!(bomb_damage_for_ship(261.0, 0.75, 0.6, 1, true), 117.0);
    assert_eq!(emp_bomb_shutdown_time(true, 3.0), 3.0);
    assert_eq!(emp_bomb_shutdown_time(false, 3.0), 0.0);
    assert_eq!(explosive_contact_radius(14.0), 18.0);
    assert!(!proximity_sensor_overlaps_ship(
        Vec2::ZERO,
        vec2(72.0, 72.0),
        72.0,
        14.0
    ));
    assert!(!proximity_sensor_overlaps_ship(
        Vec2::ZERO,
        vec2(73.0, 0.0),
        72.0,
        14.0
    ));
    assert!(!explosive_contact_overlaps_ship(
        Vec2::ZERO,
        vec2(18.0, 18.0),
        14.0
    ));
    assert!(!explosive_contact_overlaps_ship(
        Vec2::ZERO,
        vec2(19.0, 0.0),
        14.0
    ));
    assert!(proximity_sensor_overlaps_ship(
        vec2(72.0, 72.0),
        Vec2::ZERO,
        72.0,
        14.0
    ));
    assert!(explosive_contact_overlaps_ship(
        vec2(18.0, 18.0),
        Vec2::ZERO,
        14.0
    ));
    assert!(explosive_contact_overlaps_ship(
        Vec2::ZERO,
        vec2(17.0, 17.0),
        14.0
    ));
}

#[test]
fn active_bots_kick_loose_powerballs() {
    let mut bots = vec![
        BotShip {
            id: 0,
            pos: Vec2::ZERO,
            vel: vec2(10.0, 0.0),
            orientation: 0.25,
            ship_type: 1,
            energy: BOT_ENERGY_MAX,
            max_energy: BOT_ENERGY_MAX,
            bounty: 0,
            kills: 0,
            deaths: 0,
            flag_goals: 0,
            ball_goals: 0,
            carrying_flag: false,
            super_timer: 0.0,
            shield_timer: 0.0,
            engine_shutdown_timer: 0.0,
            emp_shutdown_timer: 0.0,
            fire_cooldown: 0.0,
            exhaust_timer: 0.0,
            respawn_timer: 0.0,
            age: 0.0,
        },
        BotShip {
            id: 1,
            pos: vec2(1.0, 0.0),
            vel: Vec2::ZERO,
            orientation: 0.0,
            ship_type: 2,
            energy: 0.0,
            max_energy: BOT_ENERGY_MAX,
            bounty: 0,
            kills: 0,
            deaths: 1,
            flag_goals: 0,
            ball_goals: 0,
            carrying_flag: false,
            super_timer: 0.0,
            shield_timer: 0.0,
            engine_shutdown_timer: 0.0,
            emp_shutdown_timer: 0.0,
            fire_cooldown: 0.0,
            exhaust_timer: 0.0,
            respawn_timer: 1.0,
            age: 0.0,
        },
    ];

    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);

    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.ships[1].soccer_ball_proximity = POWERBALL_PICKUP_RADIUS;
    tuning.ships[1].soccer_ball_speed = BOT_POWERBALL_KICK_SPEED;

    let (bot_id, pos, vel, friction) =
        bot_powerball_kick(vec2(12.0, 0.0), &bots, &map, &tuning, false, 0.0)
            .expect("active bot should kick");
    assert_eq!(bot_id, 0);
    assert!(pos.x > POWERBALL_PICKUP_RADIUS);
    assert!(vel.x > BOT_POWERBALL_KICK_SPEED);
    assert_eq!(friction, POWERBALL_FRICTION);
    assert!(bot_powerball_kick(vec2(12.0, 0.0), &bots, &map, &tuning, false, 0.25).is_none());

    bots[0].respawn_timer = 1.0;
    assert!(bot_powerball_kick(vec2(12.0, 0.0), &bots, &map, &tuning, false, 0.0).is_none());
}

#[test]
fn bot_prizes_award_bounty_and_apply_local_energy_effects() {
    let ship_tuning = ArenaTuning::continuum_skirmish_default().ship(1);
    let mut bot = BotShip {
        id: 0,
        pos: Vec2::ZERO,
        vel: vec2(100.0, 0.0),
        orientation: 0.0,
        ship_type: 1,
        energy: 100.0,
        max_energy: BOT_ENERGY_MAX,
        bounty: 0,
        kills: 0,
        deaths: 0,
        flag_goals: 0,
        ball_goals: 0,
        carrying_flag: false,
        super_timer: 0.0,
        shield_timer: 0.0,
        engine_shutdown_timer: 0.0,
        emp_shutdown_timer: 0.0,
        fire_cooldown: 0.0,
        exhaust_timer: 0.0,
        respawn_timer: 0.0,
        age: 0.0,
    };

    apply_bot_prize(&mut bot, PrizeKind::Energy, ship_tuning);
    assert_eq!(bot.bounty, 1);
    assert_eq!(bot.energy, 100.0);
    assert_eq!(bot.max_energy, BOT_ENERGY_MAX + 350.0);

    apply_bot_prize(&mut bot, PrizeKind::FullCharge, ship_tuning);
    assert_eq!(bot.bounty, 2);
    assert_eq!(bot.energy, bot.max_energy);

    apply_bot_prize(&mut bot, PrizeKind::EngineShutdown, ship_tuning);
    assert_eq!(bot.bounty, 3);
    assert_eq!(bot.engine_shutdown_timer, ENGINE_SHUTDOWN_LIFETIME);
    assert_eq!(bot.fire_cooldown, BOT_FIRE_COOLDOWN);
    assert!(bot.vel.length() < 100.0);
}

#[test]
fn bot_engine_shutdown_blocks_firing() {
    let range = 760.0;
    assert!(bot_can_fire(400.0, 0.98, 0.0, false, false, range));
    assert!(!bot_can_fire(400.0, 0.98, 0.0, false, true, range));
    assert!(!bot_can_fire(800.0, 0.98, 0.0, false, false, range));
    assert!(!bot_can_fire(400.0, 0.90, 0.0, false, false, range));
    assert!(!bot_can_fire(400.0, 0.98, 0.1, false, false, range));
    assert!(!bot_can_fire(400.0, 0.98, 0.0, true, false, range));
}

#[test]
fn bot_bullet_settings_follow_arena_tuning() {
    let mut tuning = ArenaTuning::continuum_skirmish_default();
    tuning.bullet_lifetime = 2.25;
    tuning.bullet_damage = 90.0;
    tuning.bullet_damage_upgrade = 15.0;
    tuning.ships[2].bullet_speed = 420.0;
    tuning.ships[2].bullet_cooldown = 0.42;
    tuning.ships[2].initial_gun_level = 3;
    tuning.ships[2].max_gun_level = 4;

    assert_eq!(bot_bullet_level(tuning.ship(2)), 3);
    assert_eq!(bot_bullet_range(tuning, 2), 945.0);
    assert_eq!(bot_bullet_damage(tuning, 2), 120.0);
    assert!((bot_initial_fire_cooldown(tuning.ship(2).bullet_cooldown, 1) - 0.294).abs() < 0.001);
    assert!((bot_fire_cooldown_after_shot(tuning.ship(2).bullet_cooldown, 1) - 0.54).abs() < 0.001);
}

#[test]
fn bot_engine_shutdown_uses_slow_rotation() {
    assert_eq!(bot_rotation_per_second(false), BOT_ROTATION_PER_SECOND);
    assert_eq!(
        bot_rotation_per_second(true),
        ENGINE_SHUTDOWN_ROTATION_PER_SECOND
    );
}

#[test]
fn ship_labels_include_bounty_and_carried_objectives() {
    assert_eq!(ship_label("You", 0, false, false), "You(0)");
    assert_eq!(ship_label("You", 3, true, false), "You(3:1)");
    assert_eq!(ship_label("Bot 2", 7, false, false), "Bot 2(7)");
    assert_eq!(ship_label("You", 4, true, true), "You(4:1) (Ball)");
}

#[test]
fn low_energy_labels_match_reference_thresholds() {
    assert!(energy_label(500.0, 1000.0).is_none());

    let (warning, warning_color) = energy_label(499.0, 1000.0).expect("below half shows energy");
    assert_eq!(warning, "499");
    assert_eq!(warning_color, Color::from_rgba(255, 225, 95, 255));

    let (critical, critical_color) =
        energy_label(249.0, 1000.0).expect("below quarter is critical");
    assert_eq!(critical, "249");
    assert_eq!(critical_color, Color::from_rgba(255, 90, 70, 255));
}

#[test]
fn critical_energy_alarm_uses_same_threshold_as_critical_label() {
    assert!(!energy_alarm_active(250.0, 1000.0, false, 0.0));
    assert!(energy_alarm_active(249.0, 1000.0, false, 0.0));
    assert!(!energy_alarm_active(249.0, 1000.0, true, 0.0));
    assert!(!energy_alarm_active(249.0, 1000.0, false, 0.1));
}

#[test]
fn bot_shield_and_super_prizes_enable_temporary_protection() {
    let ship_tuning = ArenaTuning::continuum_skirmish_default().ship(1);
    let mut bot = BotShip {
        id: 0,
        pos: Vec2::ZERO,
        vel: Vec2::ZERO,
        orientation: 0.0,
        ship_type: 1,
        energy: 100.0,
        max_energy: BOT_ENERGY_MAX,
        bounty: 0,
        kills: 0,
        deaths: 0,
        flag_goals: 0,
        ball_goals: 0,
        carrying_flag: false,
        super_timer: 0.0,
        shield_timer: 0.0,
        engine_shutdown_timer: 0.0,
        emp_shutdown_timer: 0.0,
        fire_cooldown: 0.0,
        exhaust_timer: 0.0,
        respawn_timer: 0.0,
        age: 0.0,
    };

    assert_eq!(bot_damage_after_timed_protection(&bot, 100.0), 100.0);

    apply_bot_prize(&mut bot, PrizeKind::Shield, ship_tuning);
    assert_eq!(bot.shield_timer, SHIELD_LIFETIME);
    assert_eq!(bot_damage_after_timed_protection(&bot, 100.0), 0.0);

    bot.shield_timer = 0.0;
    apply_bot_prize(&mut bot, PrizeKind::Super, ship_tuning);
    assert_eq!(bot.super_timer, SUPER_LIFETIME);
    assert_eq!(bot_damage_after_timed_protection(&bot, 100.0), 0.0);
}

#[test]
fn low_energy_bots_seek_nearby_prizes() {
    let prizes = vec![
        PrizePickup {
            collectible_id: 0,
            pos: vec2(BOT_PRIZE_SEEK_RADIUS + 10.0, 0.0),
            kind: PrizeKind::Energy,
            negative: false,
            age: 0.0,
            ttl: PRIZE_LIFETIME,
            claim_retry_timer: 0.0,
        },
        PrizePickup {
            collectible_id: 0,
            pos: vec2(160.0, 0.0),
            kind: PrizeKind::Energy,
            negative: false,
            age: 0.0,
            ttl: PRIZE_LIFETIME,
            claim_retry_timer: 0.0,
        },
        PrizePickup {
            collectible_id: 0,
            pos: vec2(80.0, 0.0),
            kind: PrizeKind::Energy,
            negative: false,
            age: 0.0,
            ttl: 0.0,
            claim_retry_timer: 0.0,
        },
    ];

    assert_eq!(
        bot_prize_target_position(
            Vec2::ZERO,
            crate::bot::bot_prize_seek_energy(BOT_ENERGY_MAX) - 1.0,
            BOT_ENERGY_MAX,
            &prizes,
        ),
        Some(vec2(160.0, 0.0))
    );
    assert_eq!(
        bot_prize_target_position(
            Vec2::ZERO,
            crate::bot::bot_prize_seek_energy(BOT_ENERGY_MAX),
            BOT_ENERGY_MAX,
            &prizes,
        ),
        None
    );
}

#[test]
fn player_prize_removal_uses_protocol_integer_position() {
    let mut prizes = vec![
        PrizePickup {
            collectible_id: 44,
            pos: vec2(44.8, 55.1),
            kind: PrizeKind::Repel,
            negative: false,
            age: 0.0,
            ttl: 10.0,
            claim_retry_timer: 0.0,
        },
        PrizePickup {
            collectible_id: 45,
            pos: vec2(80.0, 90.0),
            kind: PrizeKind::Rocket,
            negative: false,
            age: 0.0,
            ttl: 10.0,
            claim_retry_timer: 0.0,
        },
    ];

    assert!(remove_prize_at_protocol_position(&mut prizes, 44, 55));
    assert_eq!(prizes.len(), 1);
    assert_eq!(prizes[0].pos, vec2(80.0, 90.0));
    assert!(!remove_prize_at_protocol_position(&mut prizes, 44, 55));
}

#[test]
fn player_death_green_uses_previous_remote_position() {
    let mut world = NetworkWorld::default();
    let killer = PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"killer")).unwrap();
    let killed = PlayerRosterEntry::parse(&sample_player_enter_packet(10, b"killed")).unwrap();

    world.apply_event(&ClientProtocolEvent::PlayerEntered(killer));
    world.apply_event(&ClientProtocolEvent::PlayerEntered(killed));
    world.apply_event(&ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
        player_id: 10,
        direction: 0,
        timestamp: 1,
        x: 320,
        y: 480,
        velocity_x: 10,
        velocity_y: -20,
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

    let death = PlayerDeath {
        green_id: PrizeKind::Repel.protocol_id() as i8,
        green_collectible_id: 77,
        killer_id: 9,
        killed_id: 10,
        bounty: 0,
        flag_transfer: 0,
        life_seq: 1,
        respawn_at_tick: 100,
    };
    assert_eq!(
        death_prize_from_player_death(&world, death),
        Some((77, vec2(320.0, 480.0), PrizeKind::Repel))
    );
    assert_eq!(
        ship_explosion_position_from_player_death(&world, death),
        Some(vec2(320.0, 480.0))
    );

    world
        .players
        .get_mut(&10)
        .expect("killed player should exist")
        .velocity = Vec2::ZERO;
    assert_eq!(death_prize_from_player_death(&world, death), None);
    assert_eq!(
        ship_explosion_position_from_player_death(&world, death),
        Some(vec2(320.0, 480.0))
    );

    let no_green = PlayerDeath {
        green_id: 0,
        ..death
    };
    assert_eq!(death_prize_from_player_death(&world, no_green), None);
}

#[test]
fn gameplay_action_requests_use_reference_game_world_units() {
    let prize = take_prize_request(77, vec2(44.8, 55.9), PrizeKind::Repel, false);
    assert_eq!(
        prize,
        TakePrizeRequest {
            collectible_id: 77,
            x: 44,
            y: 55,
            prize_id: 21,
        }
    );
    assert_eq!(
        take_prize_request(77, vec2(44.8, 55.9), PrizeKind::Repel, true).prize_id,
        -21
    );

    assert_eq!(
        drop_brick_request(vec2(100.9, 200.1)),
        DropBrickRequest { x: 100, y: 200 }
    );

    assert_eq!(
        ball_fire_request(2, vec2(10.5, 20.25), vec2(3.5, -4.25), 9),
        BallFireRequest {
            ball_id: 2,
            x: 168,
            y: 324,
            velocity_x: 560,
            velocity_y: -680,
            player_id: 9,
            timestamp: 0,
        }
    );

    assert_eq!(
        ball_goal_request(3, vec2(123.9, -4.2)),
        BallGoalRequest {
            ball_id: 3,
            x: 123,
            y: -4,
        }
    );
}

#[test]
fn number_keys_map_to_reference_game_ship_requests() {
    assert_eq!(ship_number_key(KeyCode::Key1), Some(0));
    assert_eq!(ship_number_key(KeyCode::Key8), Some(7));
    assert_eq!(ship_number_key(KeyCode::Tab), None);
}

#[test]
fn free_for_all_has_no_frequency_step_shortcuts() {
    assert_eq!(frequency_step_key(KeyCode::Comma), None);
    assert_eq!(frequency_step_key(KeyCode::Period), None);
    assert_eq!(frequency_step_key(KeyCode::Tab), None);
}

#[test]
fn brick_line_tiles_match_reference_endpoint_walk() {
    assert_eq!(
        brick_line_tiles(vec2(10.0, 11.0), vec2(14.0, 11.0)),
        vec![
            ivec2(10, 11),
            ivec2(11, 11),
            ivec2(12, 11),
            ivec2(13, 11),
            ivec2(14, 11)
        ]
    );
    assert_eq!(
        brick_line_tiles(vec2(4.0, 5.0), vec2(4.0, 5.0)),
        vec![ivec2(4, 5)]
    );
}

#[test]
fn bot_flag_objectives_prefer_scoring_then_dropped_then_source_flags() {
    let dropped = vec2(10.0, 0.0);
    let source = vec2(20.0, 0.0);
    let goal = vec2(30.0, 0.0);

    assert_eq!(
        bot_flag_objective_target(true, Some(dropped), Some(source), Some(goal), false),
        Some(goal)
    );
    assert_eq!(
        bot_flag_objective_target(false, Some(dropped), Some(source), Some(goal), false),
        Some(dropped)
    );
    assert_eq!(
        bot_flag_objective_target(false, None, Some(source), Some(goal), false),
        Some(source)
    );
    assert_eq!(
        bot_flag_objective_target(false, None, Some(source), Some(goal), true),
        None
    );
}

#[test]
fn antiwarp_jams_bots_inside_radius_only_when_active() {
    assert!(bot_jammed_by_antiwarp(
        Vec2::ZERO,
        true,
        vec2(639.0, 0.0),
        640.0
    ));
    assert!(!bot_jammed_by_antiwarp(
        Vec2::ZERO,
        true,
        vec2(641.0, 0.0),
        640.0
    ));
    assert!(!bot_jammed_by_antiwarp(
        Vec2::ZERO,
        false,
        Vec2::ZERO,
        640.0
    ));
}

#[test]
fn bot_bullet_ship_contact_uses_reference_boxes_and_pixel_edges() {
    for radius in [SHIP_RADIUS, 20.0] {
        assert!(bot_bullet_hits_ship(
            vec2(radius + BOT_BULLET_RADIUS - 0.1, 0.0),
            Vec2::ZERO,
            radius
        ));
        assert!(bot_bullet_hits_ship(
            vec2(radius + BOT_BULLET_RADIUS, 0.0),
            Vec2::ZERO,
            radius
        ));
        assert!(!bot_bullet_hits_ship(
            vec2(-radius - BOT_BULLET_RADIUS, 0.0),
            Vec2::ZERO,
            radius
        ));
        assert!(!bot_bullet_hits_ship(
            vec2(radius + BOT_BULLET_RADIUS + 1.0, 0.0),
            Vec2::ZERO,
            radius
        ));
        assert!(bullet_contact_overlaps_ship(
            vec2(radius + 4.0, radius + 4.0),
            Vec2::ZERO,
            radius
        ));
        assert!(!bullet_contact_overlaps_ship(
            vec2(-radius - 4.0, -radius - 4.0),
            Vec2::ZERO,
            radius
        ));
    }
}

#[test]
fn expanded_scoreboard_uses_reference_average_rating_and_nonwrapping_total() {
    let mut rows = scoreboard_entries(
        "self",
        2,
        1,
        30,
        100,
        0,
        0,
        1700.0,
        &[],
        &NetworkWorld::default(),
    );
    let row = &mut rows[0];
    row.kill_points = 61;
    assert_eq!(row.total_score(), 161);
    assert_eq!(row.average_bounty(), 30.5);
    assert_eq!(row.rating(), 6); // (61 + (2 - 1) * 10) * 10 / 102
    row.kills = 0;
    row.deaths = 100;
    row.kill_points = 0;
    assert_eq!(row.average_bounty(), 0.0);
    assert_eq!(row.rating(), 0);
    row.kill_points = u32::MAX;
    row.flag_goals = u32::MAX;
    assert_eq!(row.total_score(), 2 * u64::from(u32::MAX));
}

#[test]
fn scoreboard_entries_sort_by_kills_and_track_respawns() {
    let map = ArenaMap::fallback_skirmish();
    let player_pos = map.spawn_position();
    let mut bots = spawn_initial_bots(&map, player_pos, ArenaTuning::continuum_skirmish_default());
    bots[0].kills = 3;
    bots[0].deaths = 1;
    bots[0].bounty = 7;
    bots[0].flag_goals = 1;
    bots[0].ball_goals = 2;
    bots[1].kills = 3;
    bots[1].deaths = 2;
    bots[1].respawn_timer = 1.0;

    let rows = scoreboard_entries(
        "pilot",
        2,
        4,
        5,
        3,
        1,
        0,
        900.0,
        &bots,
        &NetworkWorld::default(),
    );

    assert_eq!(rows[0].name, "Bot 1");
    assert_eq!(rows[0].bounty, 7);
    assert_eq!(rows[0].flag_goals, 1);
    assert_eq!(rows[0].ball_goals, 2);
    assert_eq!(rows[1].name, "Bot 2");
    assert!(!rows[1].active);
    assert!(rows.iter().any(|row| row.name == "pilot" && row.local));
    assert!(
        rows.iter()
            .any(|row| row.name == "pilot" && row.bounty == 5)
    );
    assert!(
        rows.iter()
            .any(|row| row.name == "pilot" && row.flag_goals == 3)
    );
}

#[test]
fn bot_target_prefers_live_decoys_and_respects_hidden_player() {
    let bot_pos = vec2(0.0, 0.0);
    let player_pos = vec2(100.0, 0.0);
    let decoys = vec![
        Decoy {
            authority_binding: None,
            presentation_clock: None,
            prediction: None,
            pos: vec2(30.0, 0.0),
            vel: Vec2::ZERO,
            frequency: 0,
            orientation: 0.0,
            initial_orientation: 0.0,
            remote_source: None,
            last_repel_mutation_event_id: None,
            authoritative_repel_state: None,
            ship_type: 0,
            owner_name: "pilot".to_owned(),
            owner_bounty: 0,
            ttl: DECOY_LIFETIME,
            age: 0.0,
        },
        Decoy {
            authority_binding: None,
            presentation_clock: None,
            prediction: None,
            pos: vec2(10.0, 0.0),
            vel: Vec2::ZERO,
            frequency: 0,
            orientation: 0.0,
            initial_orientation: 0.0,
            remote_source: None,
            last_repel_mutation_event_id: None,
            authoritative_repel_state: None,
            ship_type: 0,
            owner_name: "pilot".to_owned(),
            owner_bounty: 0,
            ttl: 0.0,
            age: 0.0,
        },
    ];

    assert_eq!(
        bot_target_position(bot_pos, player_pos, false, &decoys),
        Some(vec2(30.0, 0.0))
    );
    assert_eq!(
        bot_target_position(bot_pos, player_pos, true, &decoys),
        Some(vec2(30.0, 0.0))
    );
    assert_eq!(bot_target_position(bot_pos, player_pos, true, &[]), None);
}

#[test]
fn decoy_orientation_mirrors_owner_around_launch_orientation() {
    assert_eq!(decoy_mirrored_orientation(0.25, 0.25), 0.25);
    assert!((decoy_mirrored_orientation(0.25, 0.35) - 0.15).abs() < 0.001);
    assert!((decoy_mirrored_orientation(0.1, 0.3) - 0.9).abs() < 0.001);
}

#[test]
fn decoy_motion_matches_reference_game_weapon_simulation() {
    let mut map = empty_test_map();
    let mut decoy = Decoy {
        authority_binding: None,
        presentation_clock: None,
        prediction: None,
        pos: vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE),
        vel: vec2(64.0, 0.0),
        frequency: 3,
        orientation: 0.0,
        initial_orientation: 0.0,
        remote_source: None,
        last_repel_mutation_event_id: None,
        authoritative_repel_state: None,
        ship_type: 0,
        owner_name: "pilot".to_owned(),
        owner_bounty: 0,
        ttl: 10.0,
        age: 0.0,
    };

    assert!(!update_decoy_motion(&map, &[], &mut decoy, 0.25));
    assert_eq!(decoy.vel, vec2(64.0, 0.0));
    assert!((decoy.pos - vec2(10.5 * TILE_SIZE + 16.0, 10.5 * TILE_SIZE)).length() < 0.001);
    assert_eq!(decoy.ttl, 9.75);
    assert_eq!(decoy.age, 0.25);

    map.tiles[10 * map.width + 11] = 1;
    decoy.pos = vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    decoy.vel = vec2(64.0, 0.0);
    assert!(update_decoy_motion(&map, &[], &mut decoy, 0.25));
    assert!(decoy.pos.x < 11.0 * TILE_SIZE);
    assert_eq!(decoy.vel, vec2(-64.0, 0.0));

    map.tiles.fill(0);
    map.tiles[10 * map.width + 10] = WORMHOLE_TILE_ID;
    decoy.pos = vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    decoy.vel = Vec2::ZERO;
    decoy.ttl = 10.0;
    assert!(!update_decoy_motion(&map, &[], &mut decoy, 0.01));
    assert_eq!(decoy.ttl, 0.0);
}

#[test]
fn projectile_timeouts_disappear_without_contact_impact() {
    assert!(projectile_cleanup_creates_impact(true, false));
    assert!(!projectile_cleanup_creates_impact(false, true));
    assert!(!projectile_cleanup_creates_impact(true, true));
    assert!(explosive_cleanup_creates_explosion(true, false));
    assert!(!explosive_cleanup_creates_explosion(false, true));
    assert!(!explosive_cleanup_creates_explosion(true, true));
}

#[test]
fn early_shrapnel_wall_hits_disappear_without_impact_like_reference_game() {
    assert!(!shrapnel_cleanup_creates_impact(true, false, 0.0));
    assert!(!shrapnel_cleanup_creates_impact(
        true,
        false,
        SHRAPNEL_INACTIVE_DAMAGE_TIME
    ));
    assert!(shrapnel_cleanup_creates_impact(
        true,
        false,
        SHRAPNEL_INACTIVE_DAMAGE_TIME + 0.001
    ));
    assert!(!shrapnel_cleanup_creates_impact(false, true, 1.0));
}

#[test]
fn shrapnel_sprite_selects_all_reference_level_and_bounce_loops() {
    let atlas = decode_bm2(
        include_bytes!("../../assets/graphics/shrapnel.bm2"),
        Transparency::Black,
    )
    .expect("shipped shrapnel atlas");
    assert_eq!((atlas.width / 8) * (atlas.height / 8), 60);
    for (level, expected) in [(1, 0), (2, 10), (3, 20)] {
        assert_eq!(shrapnel_sprite_frame_start(level, false), expected);
        assert_eq!(shrapnel_sprite_frame_start(level, true), expected + 30);
    }
    assert_eq!(shrapnel_sprite_frame_start(0, false), 0);
    assert_eq!(shrapnel_sprite_frame_start(4, false), 20);
    assert_eq!(shrapnel_sprite_frame_start(255, true), 50);
}
