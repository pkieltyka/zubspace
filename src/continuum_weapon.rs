pub(crate) const WEAPON_TYPE_MASK: u16 = 0x001f;
pub(crate) const CONTINUUM_PROXIMITY_BOMB_TRIGGER_SCALE: f32 = 18.0;
pub(crate) const CONTINUUM_WEAPON_CONTACT_HALF_EXTENT: f32 = 4.0;
const CONTINUUM_PROXIMITY_BASE_HULL_RADIUS: f32 = 14.0;
#[allow(dead_code)]
pub(crate) const MAX_RELAYED_WEAPON_TYPE: u8 = 15;

/// Decode the per-shot Continuum `weapon_data` level field. Weapon packets
/// store level as `level - 1`, so raw bits 0..=3 mean levels 1..=4.
///
/// Do not use this for arena settings capability fields such as InitialBombs:
/// those settings store 0..=3 directly, where 0 means "no weapon".
pub(crate) fn weapon_level_from_bits(bits: u8) -> u8 {
    bits.min(3) + 1
}

/// Encode a per-shot weapon level (1..=4) into Continuum `weapon_data` bits.
/// Level 0 is not representable in a weapon packet, so it clamps to level 1.
pub(crate) fn weapon_level_to_bits(level: u8) -> u8 {
    level.clamp(1, 4) - 1
}

/// Decode arena settings capability fields such as InitialGuns/MaxBombs.
/// Unlike `weapon_data`, these settings store 0..=3 directly.
pub(crate) fn settings_weapon_level_from_bits(bits: u8) -> u8 {
    bits.min(3)
}

/// Encode arena settings capability fields such as InitialGuns/MaxBombs.
/// Unlike `weapon_data`, these settings store 0..=3 directly.
pub(crate) fn settings_weapon_level_to_bits(level: u8) -> u8 {
    level.min(3)
}

pub(crate) fn continuum_weapon_type(weapon_data: u16) -> u8 {
    (weapon_data & WEAPON_TYPE_MASK) as u8
}

#[allow(dead_code)]
pub(crate) fn continuum_weapon_level(weapon_data: u16) -> u8 {
    weapon_level_from_bits(continuum_weapon_raw_level(weapon_data))
}

/// Read the 2-bit weapon-level field from `weapon_data` as raw bits
/// (0..=3). Use this when a formula wants the 0-indexed value directly
/// (e.g. The reference game's `prox = ProximityDistance + raw_level + 3`).
/// Prefer `continuum_weapon_level` when you actually want the 1-indexed
/// level (1..=4) — that routes through the canonical `weapon_level_from_bits`.
pub(crate) fn continuum_weapon_raw_level(weapon_data: u16) -> u8 {
    ((weapon_data >> 5) & 0x0003) as u8
}

#[allow(dead_code)]
pub(crate) fn continuum_weapon_shrapnel_level(weapon_data: u16) -> u8 {
    weapon_level_from_bits(((weapon_data >> 8) & 0x0003) as u8)
}

#[allow(dead_code)]
pub(crate) fn continuum_weapon_shrapnel_count(weapon_data: u16) -> u8 {
    ((weapon_data >> 10) & 0x001f) as u8
}

#[allow(dead_code)]
pub(crate) fn continuum_weapon_alternate(weapon_data: u16) -> bool {
    weapon_data & 0x8000 != 0
}

#[allow(dead_code)]
pub(crate) fn continuum_weapon_is_proximity_bomb(weapon_data: u16) -> bool {
    continuum_weapon_type(weapon_data) == 4
}

#[allow(dead_code)]
pub(crate) fn continuum_weapon_is_mine(weapon_data: u16) -> bool {
    matches!(continuum_weapon_type(weapon_data), 3 | 4) && continuum_weapon_alternate(weapon_data)
}

/// Whether the reference randomizes this weapon's damage when the arena's
/// `ExactDamage` flag is clear.
///
/// The reference game gates the inexact roll in `ShipController::OnWeaponHit` on
/// `Bullet`/`BouncingBullet`/`Burst`. Shrapnel is a bullet carrying
/// `shrap > 0`, and our wire type 15 is that shrapnel form, so it rolls on
/// the same terms. Bombs (including Thor) are never rolled.
pub(crate) fn continuum_weapon_uses_bullet_exactness(weapon_data: u16) -> bool {
    matches!(continuum_weapon_type(weapon_data), 1 | 2 | 7 | 15)
}

/// Fire-only BombSafety radius, in pixels. The reference game's ShipController checks
/// `(ProximityDistance + raw_level)` in 16px tile units. This is a circle,
/// distinct from WeaponManager's expanded 18px collision sensor.
pub(crate) fn continuum_proximity_bomb_firing_safety_radius(
    proximity_distance: f32,
    bomb_level: u8,
) -> f32 {
    (proximity_distance + f32::from(bomb_level.saturating_sub(1))) * 16.0
}

#[allow(dead_code)]
pub(crate) fn continuum_proximity_bomb_trigger_radius(
    proximity_distance: f32,
    bomb_level: u8,
) -> f32 {
    // Callers pass a 1-indexed bomb level (1..=4). The reference game uses the raw
    // 2-bit weapon-data level (0..=3): `prox = ProximityDistance + level`
    // — see WeaponManager::SimulateHit. Subtract 1 here so existing callers
    // can keep passing the 1-indexed value.
    let raw_level = bomb_level.saturating_sub(1) as f32;
    (proximity_distance + raw_level) * CONTINUUM_PROXIMITY_BOMB_TRIGGER_SCALE
}

/// Proximity-sensor trigger radius for a proximity bomb or Thor.
///
/// The reference game's `WeaponManager::SimulateHit`: `prox = ProximityDistance +
/// weapon.data.level` (raw, 0-based), plus an extra `3` for a Thor, scaled by
/// [`CONTINUUM_PROXIMITY_BOMB_TRIGGER_SCALE`]. Prefer this over the raw
/// level helper so a Thor's larger sensor cannot drift.
pub(crate) fn continuum_weapon_proximity_trigger_radius(
    proximity_distance: f32,
    weapon_data: u16,
) -> f32 {
    let raw_level = f32::from(continuum_weapon_raw_level(weapon_data));
    let thor_bonus = if continuum_weapon_type(weapon_data) == 8 {
        3.0
    } else {
        0.0
    };
    (proximity_distance + raw_level + thor_bonus) * CONTINUUM_PROXIMITY_BOMB_TRIGGER_SCALE
}

/// Effective `BombExplodePixels` multiplier for one explosive weapon.
///
/// Bombs use their 1-indexed level (1..=4). A Thor uses `4 + raw_level`:
/// The reference game's `OnWeaponHit` sets `level = 3 + weapon.data.level` and then
/// computes `BombExplodePixels + BombExplodePixels * level`, so a stock L1
/// Thor reaches `4 *` and an L4 Thor `7 *`.
pub(crate) fn continuum_weapon_explosion_level(weapon_data: u16) -> u8 {
    if continuum_weapon_type(weapon_data) == 8 {
        4 + continuum_weapon_raw_level(weapon_data)
    } else {
        continuum_weapon_level(weapon_data)
    }
}

fn continuum_pixel_truncated(value: f32) -> f32 {
    value as i32 as f32
}

fn continuum_contact_boxes_overlap(
    first_x: f32,
    first_y: f32,
    first_half_extent: f32,
    second_x: f32,
    second_y: f32,
    second_half_extent: f32,
) -> bool {
    let first_x = continuum_pixel_truncated(first_x);
    let first_y = continuum_pixel_truncated(first_y);
    let second_x = continuum_pixel_truncated(second_x);
    let second_y = continuum_pixel_truncated(second_y);
    let first_min_x = first_x - first_half_extent;
    let first_max_x = first_x + first_half_extent;
    let first_min_y = first_y - first_half_extent;
    let first_max_y = first_y + first_half_extent;
    let second_min_x = second_x - second_half_extent;
    let second_max_x = second_x + second_half_extent;
    let second_min_y = second_y - second_half_extent;
    let second_max_y = second_y + second_half_extent;

    first_max_x >= second_min_x
        && first_min_x < second_max_x
        && first_max_y >= second_min_y
        && first_min_y < second_max_y
}

/// The reference game's WeaponManager compares the pixel-truncated ship box FIRST with
/// the four-pixel weapon box SECOND. The order matters at exact edges because
/// BoxBoxOverlap uses >= for the first maximum and < for its minimum.
/// Ordinary bullets, activated burst, fragments and explosives share this box.
pub(crate) fn continuum_weapon_contact_overlaps_ship(
    weapon_x: f32,
    weapon_y: f32,
    ship_x: f32,
    ship_y: f32,
    ship_radius: f32,
) -> bool {
    continuum_contact_boxes_overlap(
        ship_x,
        ship_y,
        ship_radius,
        weapon_x,
        weapon_y,
        CONTINUUM_WEAPON_CONTACT_HALF_EXTENT,
    )
}

pub(crate) fn continuum_proximity_sensor_overlaps_ship(
    weapon_x: f32,
    weapon_y: f32,
    ship_x: f32,
    ship_y: f32,
    sensor_radius: f32,
    ship_radius: f32,
) -> bool {
    continuum_contact_boxes_overlap(
        ship_x,
        ship_y,
        ship_radius,
        weapon_x,
        weapon_y,
        (sensor_radius - CONTINUUM_PROXIMITY_BASE_HULL_RADIUS).max(0.0),
    )
}

#[allow(dead_code)]
pub(crate) fn continuum_weapon_with_limits(
    weapon_data: u16,
    weapon_level: u8,
    shrapnel_level: u8,
    shrapnel_count: u8,
) -> u16 {
    let level_bits = weapon_level_to_bits(weapon_level) as u16;
    let shrapnel_level_bits = weapon_level_to_bits(shrapnel_level) as u16;
    let shrapnel_count_bits = shrapnel_count.min(31) as u16;
    (weapon_data & !(0x0060 | 0x0300 | 0x7c00))
        | (level_bits << 5)
        | (shrapnel_level_bits << 8)
        | (shrapnel_count_bits << 10)
}

#[allow(dead_code)]
pub(crate) fn valid_relayed_weapon_data(weapon_data: u16) -> bool {
    continuum_weapon_type(weapon_data) <= MAX_RELAYED_WEAPON_TYPE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_box_argument_order_matches_reference_game_ship_then_weapon() {
        for radius in [14.0_f32, 20.0] {
            // Strict lower and inclusive upper are intentionally asymmetric.
            for (offset, expected) in [
                (radius + 3.9, true),
                (radius + 4.0, true),
                (radius + 5.0, false),
                (-radius - 3.0, true),
                (-radius - 4.0, false),
                (-radius - 5.0, false),
            ] {
                for (x, y) in [(1000.0 + offset, 1000.0), (1000.0, 1000.0 + offset)] {
                    assert_eq!(
                        continuum_weapon_contact_overlaps_ship(x, y, 1000.0, 1000.0, radius),
                        expected,
                        "radius {radius}, weapon ({x},{y})"
                    );
                    assert_eq!(
                        continuum_proximity_sensor_overlaps_ship(
                            x, y, 1000.0, 1000.0, 18.0, radius
                        ),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn direct_and_proximity_contacts_match_retained_reference_game_oracle() {
        let fixture = include_str!("../tests/fixtures/reference_game_weapon_contacts.csv");
        let mut cases = 0;
        for line in fixture.lines().filter(|line| !line.starts_with('#')) {
            let fields: Vec<f32> = line
                .split(',')
                .map(|value| value.parse().unwrap())
                .collect();
            assert_eq!(fields.len(), 8);
            let [
                ship_x,
                ship_y,
                radius,
                weapon_x,
                weapon_y,
                sensor,
                direct,
                proximity,
            ] = fields[..]
            else {
                unreachable!()
            };
            assert_eq!(
                continuum_weapon_contact_overlaps_ship(weapon_x, weapon_y, ship_x, ship_y, radius),
                direct != 0.0,
                "direct: {line}"
            );
            assert_eq!(
                continuum_proximity_sensor_overlaps_ship(
                    weapon_x, weapon_y, ship_x, ship_y, sensor, radius
                ),
                proximity != 0.0,
                "proximity: {line}"
            );
            cases += 1;
        }
        assert_eq!(cases, 176);
    }

    #[test]
    fn firing_safety_and_contact_sensor_keep_their_distinct_reference_units() {
        for proximity in [0.0, 1.0, 3.0, 8.0] {
            for level in 1..=4 {
                let tiles = proximity + f32::from(level - 1);
                assert_eq!(
                    continuum_proximity_bomb_firing_safety_radius(proximity, level),
                    tiles * 16.0
                );
                assert_eq!(
                    continuum_proximity_bomb_trigger_radius(proximity, level),
                    tiles * 18.0
                );
            }
        }
    }

    #[test]
    fn continuum_weapon_type_reads_low_five_bits() {
        assert_eq!(continuum_weapon_type(0), 0);
        assert_eq!(continuum_weapon_type(0x83), 3);
        assert_eq!(continuum_weapon_type(0xffff), 31);
    }

    #[test]
    fn weapon_level_bit_round_trip_is_identity() {
        // Per-shot weapon_data stores level - 1. Round-tripping every
        // representable weapon packet level must be lossless.
        for level in 1..=4 {
            assert_eq!(weapon_level_from_bits(weapon_level_to_bits(level)), level);
        }
    }

    #[test]
    fn weapon_level_to_bits_clamps_out_of_range_levels() {
        // Levels below 1 or above 4 saturate to the representable extremes.
        assert_eq!(weapon_level_to_bits(0), 0);
        assert_eq!(weapon_level_to_bits(1), 0);
        assert_eq!(weapon_level_to_bits(4), 3);
        assert_eq!(weapon_level_to_bits(255), 3);
    }

    #[test]
    fn weapon_level_from_bits_decodes_weapon_packet_levels() {
        assert_eq!(weapon_level_from_bits(0), 1);
        assert_eq!(weapon_level_from_bits(1), 2);
        assert_eq!(weapon_level_from_bits(2), 3);
        assert_eq!(weapon_level_from_bits(3), 4);
        // Defensive: any out-of-range bits saturate to level 4 (matches
        // server's previous .min(3) + 1 contract).
        assert_eq!(weapon_level_from_bits(255), 4);
    }

    #[test]
    fn settings_weapon_level_bits_store_zero_to_three_directly() {
        assert_eq!(settings_weapon_level_from_bits(0), 0);
        assert_eq!(settings_weapon_level_from_bits(1), 1);
        assert_eq!(settings_weapon_level_from_bits(2), 2);
        assert_eq!(settings_weapon_level_from_bits(3), 3);
        assert_eq!(settings_weapon_level_from_bits(255), 3);
        assert_eq!(settings_weapon_level_to_bits(0), 0);
        assert_eq!(settings_weapon_level_to_bits(1), 1);
        assert_eq!(settings_weapon_level_to_bits(2), 2);
        assert_eq!(settings_weapon_level_to_bits(3), 3);
        assert_eq!(settings_weapon_level_to_bits(255), 3);
    }

    #[test]
    fn weapon_helpers_decode_level_and_alternate_bits() {
        assert_eq!(continuum_weapon_level(0), 1);
        assert_eq!(continuum_weapon_level(1 | (2 << 5)), 3);
        assert_eq!(continuum_weapon_level(0xffff), 4);
        assert!(!continuum_weapon_alternate(0x7fff));
        assert!(continuum_weapon_alternate(0x8000));
        assert!(continuum_weapon_is_proximity_bomb(4));
        assert!(continuum_weapon_is_proximity_bomb(4 | (2 << 5)));
        assert!(continuum_weapon_is_mine(3 | 0x8000));
        assert!(continuum_weapon_is_mine(4 | 0x8000));
        assert!(!continuum_weapon_is_mine(4));
    }

    #[test]
    fn weapon_helpers_decode_and_clamp_shrapnel_bitfields() {
        let weapon = 3 | (3 << 5) | (2 << 8) | (17 << 10) | (1 << 15);
        assert_eq!(continuum_weapon_level(weapon), 4);
        assert_eq!(continuum_weapon_shrapnel_level(weapon), 3);
        assert_eq!(continuum_weapon_shrapnel_count(weapon), 17);
        assert_eq!(
            continuum_weapon_with_limits(weapon, 2, 1, 4),
            3 | (1 << 5) | (4 << 10) | (1 << 15)
        );
    }

    #[test]
    fn relay_validation_accepts_continuum_weapon_types_we_forward() {
        assert!(valid_relayed_weapon_data(0));
        assert!(valid_relayed_weapon_data(15));
        assert!(valid_relayed_weapon_data(0xffef));
        assert!(!valid_relayed_weapon_data(16));
        assert!(!valid_relayed_weapon_data(31));
    }
}
