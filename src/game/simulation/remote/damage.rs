use super::*;

pub(super) fn remote_projectile_damage_after_protection(
    damage: f32,
    weapon_data: u16,
    exact_damage: bool,
    super_timer: f32,
    shield_timer: f32,
    shield_lifetime: f32,
    flagger_damage_multiplier: f32,
    flagger_active: bool,
) -> f32 {
    let damage = damage_after_timed_protection(damage, super_timer, shield_timer, shield_lifetime);
    let damage = if remote_projectile_damage_uses_bullet_exactness(weapon_data) {
        bullet_or_burst_damage_after_exactness(exact_damage, damage)
    } else {
        damage
    };
    flagger_damage_received(damage, flagger_damage_multiplier, flagger_active)
}

pub(super) fn remote_projectile_damage_uses_bullet_exactness(weapon_data: u16) -> bool {
    matches!(weapon_data & 0x001f, 1 | 2 | 7 | 15)
}

#[cfg(test)]
pub(crate) fn remote_explosive_contact_radius(ship_radius: f32) -> f32 {
    explosive_contact_radius(ship_radius)
}

#[cfg(test)]
pub(super) fn remote_proximity_bomb_triggered(
    bomb_pos: Vec2,
    player_pos: Vec2,
    player_radius: f32,
    proximity_distance: f32,
    level: u8,
) -> bool {
    proximity_sensor_overlaps_ship(
        bomb_pos,
        player_pos,
        proximity_bomb_trigger_radius(proximity_distance, level),
        player_radius,
    )
}

#[cfg(test)]
pub(super) fn remote_thor_proximity_triggered(
    thor_pos: Vec2,
    player_pos: Vec2,
    player_radius: f32,
    proximity_distance: f32,
    weapon_data: u16,
) -> bool {
    let thor_level = crate::continuum_weapon::continuum_weapon_raw_level(weapon_data);
    proximity_sensor_overlaps_ship(
        thor_pos,
        player_pos,
        thor_proximity_trigger_radius(proximity_distance, thor_level),
        player_radius,
    )
}

pub(super) fn remote_projectile_contact_damage(projectile: &RemoteProjectile) -> f32 {
    match projectile.kind {
        RemoteProjectileKind::Shrapnel { .. } => shrapnel_damage(
            projectile.age,
            projectile.inactive_damage,
            projectile.damage,
        ),
        _ => projectile.damage,
    }
}
