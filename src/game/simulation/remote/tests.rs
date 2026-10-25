use super::*;

#[test]
fn remote_shrapnel_damage_uses_bullet_exactness_rule() {
    assert!(remote_projectile_damage_uses_bullet_exactness(
        encode_weapon_data(15, 3, false, WEAPON_LEVEL_MIN, 0, false)
    ));
    assert!(remote_projectile_damage_uses_bullet_exactness(
        encode_weapon_data(1, 3, false, WEAPON_LEVEL_MIN, 0, false)
    ));
    assert!(!remote_projectile_damage_uses_bullet_exactness(
        encode_weapon_data(3, 3, false, WEAPON_LEVEL_MIN, 0, false)
    ));
}

#[test]
fn remote_contact_shapes_include_configured_ship_radius() {
    for radius in [14.0, 20.0] {
        assert!(bullet_contact_overlaps_ship(
            vec2(radius + 3.9, 0.0),
            Vec2::ZERO,
            radius
        ));
        assert!(bullet_contact_overlaps_ship(
            vec2(radius + 4.0, radius + 4.0),
            Vec2::ZERO,
            radius
        ));
        assert!(!bullet_contact_overlaps_ship(
            vec2(radius + 5.0, 0.0),
            Vec2::ZERO,
            radius
        ));
        assert!(!bullet_contact_overlaps_ship(
            vec2(-radius - 4.0, 0.0),
            Vec2::ZERO,
            radius
        ));
        assert_eq!(remote_explosive_contact_radius(radius), radius + 4.0);
    }
}

#[test]
fn remote_proximity_bomb_uses_configured_trigger_radius() {
    let radius = proximity_bomb_trigger_radius(3.0, 2);

    assert!(remote_proximity_bomb_triggered(
        Vec2::ZERO,
        vec2(radius - 0.1, 0.0),
        14.0,
        3.0,
        2
    ));
    assert!(!remote_proximity_bomb_triggered(
        Vec2::ZERO,
        vec2(radius + 1.0, 0.0),
        14.0,
        3.0,
        2
    ));
    assert!(remote_proximity_bomb_triggered(
        Vec2::ZERO,
        vec2(-radius, -radius),
        14.0,
        3.0,
        2
    ));
    // Ship is the reference's first box: this exact lower edge is open.
    assert!(!remote_proximity_bomb_triggered(
        Vec2::ZERO,
        vec2(radius, radius),
        14.0,
        3.0,
        2
    ));
    assert!(remote_proximity_bomb_triggered(
        Vec2::ZERO,
        vec2(proximity_sensor_contact_radius(radius, 20.0) - 0.1, 0.0),
        20.0,
        3.0,
        2
    ));
}

#[test]
fn remote_thor_uses_reference_proximity_bonus() {
    let weapon_data = encode_weapon_data(8, 3, false, WEAPON_LEVEL_MIN, 0, false);
    let radius = thor_proximity_trigger_radius(3.0, 2);

    assert!(remote_thor_proximity_triggered(
        Vec2::ZERO,
        vec2(radius - 0.1, 0.0),
        14.0,
        3.0,
        weapon_data
    ));
    assert!(!remote_thor_proximity_triggered(
        Vec2::ZERO,
        vec2(radius + 1.0, 0.0),
        14.0,
        3.0,
        weapon_data
    ));
    assert!(remote_thor_proximity_triggered(
        Vec2::ZERO,
        vec2(-radius, -radius),
        14.0,
        3.0,
        weapon_data
    ));
    // Ship is the reference's first box: this exact lower edge is open.
    assert!(!remote_thor_proximity_triggered(
        Vec2::ZERO,
        vec2(radius, radius),
        14.0,
        3.0,
        weapon_data
    ));
}

#[test]
fn remote_bounce_sound_position_keeps_nearest_bounce() {
    let listener = vec2(10.0, 10.0);

    assert_eq!(
        nearest_sound_position(None, listener, vec2(60.0, 10.0)),
        Some(vec2(60.0, 10.0))
    );
    assert_eq!(
        nearest_sound_position(Some(vec2(60.0, 10.0)), listener, vec2(20.0, 10.0)),
        Some(vec2(20.0, 10.0))
    );
    assert_eq!(
        nearest_sound_position(Some(vec2(20.0, 10.0)), listener, vec2(60.0, 10.0)),
        Some(vec2(20.0, 10.0))
    );
}
