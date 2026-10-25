#![allow(dead_code)]

use crate::continuum_weapon::{continuum_weapon_level, continuum_weapon_type};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArenaWeaponDamageSettings {
    pub(crate) bullet_damage_level: u32,
    pub(crate) bullet_damage_upgrade: u32,
    pub(crate) bomb_damage_level: u32,
    pub(crate) burst_damage_level: u32,
    pub(crate) inactive_shrapnel_damage: u32,
    pub(crate) shrapnel_damage_percent: u16,
    pub(crate) emp_bomb_damage_percent: u16,
    pub(crate) bouncing_bomb_damage_percent: u16,
}

pub(crate) fn maximum_reported_weapon_damage(
    weapon_data: u16,
    settings: ArenaWeaponDamageSettings,
) -> Option<u16> {
    let level = u32::from(continuum_weapon_level(weapon_data));
    let raw_damage = match continuum_weapon_type(weapon_data) {
        0 => return None,
        1 | 2 => leveled_raw_damage(
            settings.bullet_damage_level,
            settings.bullet_damage_upgrade,
            level,
        ),
        3 | 4 | 8 => {
            max_adjusted_bomb_raw_damage(bomb_full_raw_damage(weapon_data, settings), settings)
        }
        7 => settings.burst_damage_level,
        15 => active_shrapnel_raw_damage(settings, level).max(inactive_shrapnel_raw_damage(
            settings.inactive_shrapnel_damage,
            level,
        )),
        _ => return Some(0),
    };
    Some(raw_damage_to_report_limit(raw_damage))
}

/// The reference's bomb or Thor full damage, before falloff and before the
/// integer `/1000`.
///
/// The reference game's `ShipController::OnWeaponHit` starts every bomb from
/// `BombDamageLevel` and, for a Thor only, scales it by `1 + raw_level^2`:
/// `bomb_dmg + bomb_dmg * level * level`.
pub(crate) fn bomb_full_raw_damage(weapon_data: u16, settings: ArenaWeaponDamageSettings) -> u32 {
    let raw_level = u32::from(crate::continuum_weapon::continuum_weapon_raw_level(
        weapon_data,
    ));
    if continuum_weapon_type(weapon_data) == 8 {
        settings.bomb_damage_level.saturating_add(
            settings
                .bomb_damage_level
                .saturating_mul(raw_level * raw_level),
        )
    } else {
        settings.bomb_damage_level
    }
}

/// Authoritative override for direct-damage weapons (bullets/bursts).
///
/// Returns `Some(damage)` only when the arena's `ExactDamage` flag is set
/// — the reference game's deterministic path. With `ExactDamage = 0` (Continuum
/// default for many arenas), each hit is re-rolled on the client as
/// `sqrt((rand() * 1000) % (damage^2 + 1))`, so client values average
/// well below the ceiling and overriding to the deterministic value
/// would inflate damage. In that case we return `None`; the
/// server-authoritative damage path may choose the arena maximum via
/// `maximum_reported_weapon_damage`, but it does not read a client
/// damage timestamp or client damage value.
pub(crate) fn authoritative_unprotected_direct_damage(
    weapon_data: u16,
    settings: ArenaWeaponDamageSettings,
    exact_damage: bool,
) -> Option<u16> {
    if !exact_damage {
        return None;
    }
    match continuum_weapon_type(weapon_data) {
        1 | 2 => Some(authoritative_leveled_damage(
            settings.bullet_damage_level,
            settings.bullet_damage_upgrade,
            u32::from(continuum_weapon_level(weapon_data)),
        )),
        7 => Some((settings.burst_damage_level / 1000).min(u32::from(u16::MAX)) as u16),
        _ => None,
    }
}

/// The reference game's fragment damage changes after the first 25 Continuum ticks.
/// This is the pre-shield, pre-exactness value: the caller still applies the
/// ordinary bullet roll and protection in their existing order. Do not use
/// the permissive report ceiling as the actual age-specific damage.
pub(crate) fn authoritative_unprotected_shrapnel_damage(
    weapon_data: u16,
    age_ticks: u32,
    settings: ArenaWeaponDamageSettings,
) -> Option<u16> {
    if continuum_weapon_type(weapon_data) != 15 {
        return None;
    }
    let raw_level = u32::from(crate::continuum_weapon::continuum_weapon_raw_level(
        weapon_data,
    ));
    let damage = if age_ticks <= 25 {
        (settings.inactive_shrapnel_damage / 1000).saturating_mul(raw_level)
    } else {
        let base = (settings.bullet_damage_level / 1000)
            .saturating_add((settings.bullet_damage_upgrade / 1000).saturating_mul(raw_level));
        (base as f32 * (f32::from(settings.shrapnel_damage_percent) / 1000.0)).trunc() as u32
    };
    Some(damage.min(u32::from(u16::MAX)) as u16)
}

/// Floored damage formula matching the reference game's `ShipController`:
/// `damage = (BulletDamageLevel / 1000) + (BulletDamageUpgrade / 1000) * (level - 1)`
/// — each component floored independently. The earlier
/// `raw_damage_to_report_limit` rounded UP, biasing server damage 1 above
/// Continuum whenever the raw configured value wasn't a multiple of 1000.
fn authoritative_leveled_damage(base: u32, upgrade: u32, level: u32) -> u16 {
    let level_steps = level.saturating_sub(1);
    let raw = (base / 1000).saturating_add((upgrade / 1000).saturating_mul(level_steps));
    raw.min(u32::from(u16::MAX)) as u16
}

fn leveled_raw_damage(base: u32, upgrade: u32, level: u32) -> u32 {
    base.saturating_add(upgrade.saturating_mul(level.saturating_sub(1)))
}

fn active_shrapnel_raw_damage(settings: ArenaWeaponDamageSettings, level: u32) -> u32 {
    leveled_raw_damage(
        settings.bullet_damage_level,
        settings.bullet_damage_upgrade,
        level,
    )
    .saturating_mul(u32::from(settings.shrapnel_damage_percent))
        / 1000
}

fn inactive_shrapnel_raw_damage(inactive_damage: u32, level: u32) -> u32 {
    inactive_damage.saturating_mul(level.saturating_sub(1))
}

fn max_adjusted_bomb_raw_damage(base_damage: u32, settings: ArenaWeaponDamageSettings) -> u32 {
    // The reference game applies EMP and bouncing-bomb multipliers sequentially (each
    // ×percent/1000). For a worst-case upper bound, multiply both — taking
    // max() under-counts when both percents exceed 1000 (e.g. an EMP bouncing
    // bomb on a ship with `BombBounceCount > 0`).
    let emp = u32::from(settings.emp_bomb_damage_percent).max(1000);
    let bouncing = u32::from(settings.bouncing_bomb_damage_percent).max(1000);
    let combined = emp.saturating_mul(bouncing) / 1000;
    base_damage.saturating_mul(combined) / 1000
}

fn raw_damage_to_report_limit(raw_damage: u32) -> u16 {
    let rounded_up = raw_damage.saturating_add(999) / 1000;
    rounded_up.min(u32::from(u16::MAX)) as u16
}

/// The reference game's inexact-damage roll for arenas whose `ExactDamage` flag is
/// clear (`ShipController::OnWeaponHit`):
/// `r = (rand() * 1000) % (damage * damage + 1); damage = sqrt(r)`.
///
/// The server is authoritative and can stage one prepared hit more than once
/// (capacity retry, replay of the same collision), so the roll is derived
/// from a per-hit seed rather than a stream RNG: the same hit always commits
/// the same damage, while the value still varies uniformly across hits.
/// `damage` is the final pre-roll value, i.e. already shield-reduced.
pub(crate) fn inexact_direct_damage(damage: u16, seed: u32) -> u16 {
    if damage == 0 {
        return 0;
    }
    let max_roll = u32::from(damage) * u32::from(damage) + 1;
    let roll = mix_u32(seed) % max_roll;
    (f64::from(roll)).sqrt() as u16
}

/// SplitMix32 finalizer, used to spread a structured per-hit seed (projectile
/// id, victim identity, tick) across the roll range.
fn mix_u32(mut value: u32) -> u32 {
    value = value.wrapping_add(0x9e37_79b9);
    value = (value ^ (value >> 16)).wrapping_mul(0x85eb_ca6b);
    value = (value ^ (value >> 13)).wrapping_mul(0xc2b2_ae35);
    value ^ (value >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ArenaWeaponDamageSettings {
        ArenaWeaponDamageSettings {
            bullet_damage_level: 200_000,
            bullet_damage_upgrade: 100_000,
            bomb_damage_level: 750_000,
            burst_damage_level: 700_000,
            inactive_shrapnel_damage: 3_000,
            shrapnel_damage_percent: 500,
            emp_bomb_damage_percent: 1000,
            bouncing_bomb_damage_percent: 900,
        }
    }

    #[test]
    fn shrapnel_damage_uses_age_boundary_and_reference_integer_truncation() {
        let mut settings = settings();
        settings.bullet_damage_level = 200_999;
        settings.bullet_damage_upgrade = 100_999;
        settings.inactive_shrapnel_damage = 3_999;
        settings.shrapnel_damage_percent = 555;
        for (level, inactive, active) in [(1, 0, 111), (2, 3, 166), (3, 6, 222), (4, 9, 277)] {
            let weapon_data = 15 | ((level - 1) << 5);
            for age in [0, 1, 24, 25] {
                assert_eq!(
                    authoritative_unprotected_shrapnel_damage(weapon_data, age, settings),
                    Some(inactive)
                );
            }
            for age in [26, 100, u32::MAX] {
                assert_eq!(
                    authoritative_unprotected_shrapnel_damage(weapon_data, age, settings),
                    Some(active)
                );
            }
        }
        for weapon in [1, 2, 3, 4, 7, 8] {
            assert_eq!(
                authoritative_unprotected_shrapnel_damage(weapon, 26, settings),
                None
            );
        }
    }

    #[test]
    fn reported_weapon_damage_limits_follow_arena_weapon_settings() {
        assert_eq!(maximum_reported_weapon_damage(1, settings()), Some(200));
        assert_eq!(
            maximum_reported_weapon_damage(1 | (3 << 5), settings()),
            Some(500)
        );
        assert_eq!(maximum_reported_weapon_damage(3, settings()), Some(750));
        assert_eq!(maximum_reported_weapon_damage(7, settings()), Some(700));
        assert_eq!(
            maximum_reported_weapon_damage(8 | (2 << 5), settings()),
            Some(3750)
        );
    }

    #[test]
    fn authoritative_unprotected_direct_damage_follows_arena_settings() {
        assert_eq!(
            authoritative_unprotected_direct_damage(1, settings(), true),
            Some(200)
        );
        assert_eq!(
            authoritative_unprotected_direct_damage(2 | (3 << 5), settings(), true),
            Some(500)
        );
        assert_eq!(
            authoritative_unprotected_direct_damage(7, settings(), true),
            Some(700)
        );
        assert_eq!(
            authoritative_unprotected_direct_damage(3, settings(), true),
            None
        );
    }

    #[test]
    fn authoritative_direct_damage_returns_none_when_exact_damage_is_clear() {
        // With ExactDamage=0 the client re-rolls each hit, so the
        // deterministic value is the ceiling — not the canonical damage.
        // Returning None signals the caller to leave the client's
        // (already validated) damage in place.
        assert_eq!(
            authoritative_unprotected_direct_damage(1, settings(), false),
            None
        );
        assert_eq!(
            authoritative_unprotected_direct_damage(7, settings(), false),
            None
        );
    }

    #[test]
    fn reported_shrapnel_damage_limit_uses_active_or_inactive_max() {
        assert_eq!(
            maximum_reported_weapon_damage(15 | (1 << 5), settings()),
            Some(150)
        );
        let mut inactive_heavy = settings();
        inactive_heavy.inactive_shrapnel_damage = 200_000;
        assert_eq!(
            maximum_reported_weapon_damage(15 | (2 << 5), inactive_heavy),
            Some(400)
        );
    }

    #[test]
    fn wormhole_damage_uses_general_energy_bounds_instead_of_weapon_bounds() {
        assert_eq!(maximum_reported_weapon_damage(0, settings()), None);
    }

    #[test]
    fn inexact_direct_damage_matches_the_reference_roll_range_and_mean() {
        let damage = 210_u16;
        let mut total = 0_u64;
        let mut max_seen = 0_u16;
        for seed in 0..4096_u32 {
            let rolled = inexact_direct_damage(damage, seed);
            // Same hit seed must always commit the same damage so a capacity
            // retry cannot change the outcome.
            assert_eq!(rolled, inexact_direct_damage(damage, seed));
            assert!(rolled <= damage, "roll {rolled} exceeded {damage}");
            total += u64::from(rolled);
            max_seen = max_seen.max(rolled);
        }
        // E[sqrt(U(0, d^2))] = 2d/3. A generous band pins the shape without
        // making the assertion depend on the exact finalizer.
        let mean = total as f64 / 4096.0;
        assert!(
            (mean - (2.0 / 3.0) * f64::from(damage)).abs() < 12.0,
            "mean {mean} should sit near 2d/3"
        );
        assert!(
            max_seen > damage / 2,
            "rolls should still reach high values"
        );
        assert_eq!(inexact_direct_damage(0, 7), 0);
    }
}
