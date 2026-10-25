use super::ProjectileRecord;
use crate::gameplay_rules::{ProjectileInitializationTuning, RulesVector, WeaponDataView};

#[derive(Clone, Copy, Debug)]
pub(crate) struct ExplosionFragmentTuning {
    pub(crate) initial_projectile: ProjectileInitializationTuning,
    pub(crate) speed: f32,
    pub(crate) alive_ticks: u32,
    pub(crate) random: bool,
}

/// Prepare authoritative child records without mutating the parent/registry.
/// The explosion transaction must reserve presentation and registry capacity
/// before committing this fan. In particular, calling this helper is not an
/// explosion commit or permission to drop a queue-blocked parent.
pub(crate) fn build_shrapnel_records(
    parent: &ProjectileRecord,
    tuning: ExplosionFragmentTuning,
    explosion_position: RulesVector,
    explosion_tick: u32,
    is_solid: impl Fn(f32, f32, u16) -> bool,
    mut allocate_id: impl FnMut() -> u32,
) -> Vec<ProjectileRecord> {
    let Some(source) = parent.presentation_source else {
        return Vec::new();
    };
    let source_weapon = WeaponDataView::decode(source.weapon_data);
    if !matches!(source_weapon.weapon_type, 3 | 4 | 8)
        || !matches!(parent.weapon_data & 0x1f, 3 | 4 | 8)
        || source.source_player_id != parent.owner_id
        || source.source_life_seq != parent.owner_life_seq
        || source.source_session_id == 0
        || source.shot_group_id != parent.shot_group_id
        || tuning.alive_ticks == 0
        || is_solid(explosion_position.x, explosion_position.y, source.frequency)
    {
        return Vec::new();
    }
    // Bounces and Repel may have changed the parent's current velocity.
    // The reference game seeds once from the immutable original launch instead.
    let Some(initial) = crate::gameplay_rules::initialize_projectiles_for_protocol_direction(
        source.weapon_data,
        RulesVector::new(source.x, source.y),
        RulesVector::new(source.velocity_x, source.velocity_y),
        crate::continuum_position::protocol_direction(source.orientation),
        tuning.initial_projectile,
    )
    .into_iter()
    .next() else {
        return Vec::new();
    };
    let seed = crate::gameplay_rules::shrapnel_rng_seed(
        initial.position,
        initial.velocity,
        usize::from(source_weapon.shrapnel_count),
        source_weapon.level,
        source.frequency,
    );
    crate::gameplay_rules::initialize_shrapnel_projectiles(
        parent.weapon_data,
        explosion_position,
        seed,
        tuning.speed,
        tuning.random,
    )
    .into_iter()
    .enumerate()
    .map(|(index, fragment)| ProjectileRecord {
        id: allocate_id(),
        shot_index: index as u8,
        weapon_data: crate::gameplay_rules::encode_weapon_data(
            15,
            fragment.level,
            false,
            1,
            0,
            false,
        ),
        x: crate::continuum_position::protocol_position_coord(fragment.position.x),
        y: crate::continuum_position::protocol_position_coord(fragment.position.y),
        timestamp: explosion_tick,
        created_tick: explosion_tick,
        expiry_tick: Some(crate::continuum::continuum_tick_add(
            explosion_tick,
            tuning.alive_ticks,
        )),
        pos_x: fragment.position.x,
        precise_velocity: Some(fragment.velocity),
        pos_y: fragment.position.y,
        velocity_x: crate::continuum_position::protocol_velocity_component(fragment.velocity.x),
        velocity_y: crate::continuum_position::protocol_velocity_component(fragment.velocity.y),
        bounces_remaining: fragment.bounces_remaining,
        behavior: fragment.behavior,
        proximity_target: None,
        proximity_sensor_end_tick: None,
        proximity_highest_offset: 0.0,
        ..*parent
    })
    .collect()
}
