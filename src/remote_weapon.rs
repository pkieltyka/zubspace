use crate::{
    ArenaTuning, ProximitySensorState, ShipTuning, WEAPON_LEVEL_MAX, WEAPON_LEVEL_MIN,
    WeaponLifetimeKind, active_shrapnel_damage, bomb_damage_for_ship, bomb_explosion_radius,
    bomb_flash_position, inactive_shrapnel_damage, leveled_weapon_damage, orientation_to_heading,
    shrapnel_rng_seed, thor_damage_for_ship, weapon_lifetime,
};
use macroquad::prelude::*;

#[path = "remote_weapon/view_history.rs"]
mod view_history;
pub(crate) use view_history::{RemoteWeaponViewHistory, RemoteWeaponViewKey};
#[path = "remote_weapon/view_clock.rs"]
mod view_clock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteProjectileKind {
    Bullet { bouncing: bool },
    Burst { active: bool },
    Shrapnel { bouncing: bool },
    Bomb { proximity: bool },
    Mine { proximity: bool },
    Thor,
}

/// Immutable identity of the runtime/life that emitted a remote weapon.
///
/// Authenticated zubspace events always use `Authenticated`. `Legacy` is
/// retained only for the Continuum-compatible position packet path, whose
/// wire contract does not carry a server session id. Callers must never use a
/// legacy identity to authorize an interaction with a current roster entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RemoteWeaponSourceIdentity {
    Authenticated {
        player_id: u16,
        session_id: u64,
        life_seq: u32,
    },
    Legacy {
        player_id: u16,
        life_seq: Option<u32>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWeaponArenaScope {
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
}

impl RemoteWeaponArenaScope {
    pub(crate) fn authenticated(server_session_id: u64, arena_stream_id: u64) -> Option<Self> {
        (server_session_id != 0 && arena_stream_id != 0).then_some(Self {
            server_session_id,
            arena_stream_id,
        })
    }
}

impl RemoteWeaponSourceIdentity {
    pub(crate) fn player_id(self) -> u16 {
        match self {
            Self::Authenticated { player_id, .. } | Self::Legacy { player_id, .. } => player_id,
        }
    }

    pub(crate) fn authenticated(player_id: u16, session_id: u64, life_seq: u32) -> Option<Self> {
        (session_id != 0 && life_seq != 0).then_some(Self::Authenticated {
            player_id,
            session_id,
            life_seq,
        })
    }

    pub(crate) fn legacy(player_id: u16, life_seq: Option<u32>) -> Self {
        Self::Legacy {
            player_id,
            life_seq: life_seq.filter(|life_seq| *life_seq != 0),
        }
    }

    pub(crate) fn matches_authenticated_runtime(
        self,
        player_id: u16,
        session_id: Option<u64>,
        life_seq: Option<u32>,
    ) -> bool {
        matches!(
            self,
            Self::Authenticated {
                player_id: source_player_id,
                session_id: source_session_id,
                life_seq: source_life_seq,
            } if source_player_id == player_id
                && Some(source_session_id) == session_id
                && Some(source_life_seq) == life_seq
        )
    }
}

/// Exact fire-time state and behavior inputs retained by every remote weapon
/// object. Simulation must use these values instead of consulting a mutable
/// roster slot by numeric player id after the fact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RemoteWeaponSource {
    pub(crate) identity: RemoteWeaponSourceIdentity,
    pub(crate) arena_scope: Option<RemoteWeaponArenaScope>,
    /// Recipient-local authenticated realtime ordering. Legacy snapshot
    /// presentation leaves these unset because it has no causal event id.
    pub(crate) realtime_event_id: Option<u32>,
    pub(crate) realtime_server_tick: Option<u32>,
    /// Server-owned immutable identity shared by every projectile created by
    /// one accepted weapon action. Legacy Continuum carriers leave it unset.
    pub(crate) shot_group_id: Option<u64>,
    pub(crate) fire_position: Vec2,
    pub(crate) fire_velocity: Vec2,
    pub(crate) fire_orientation: f32,
    pub(crate) frequency: u16,
    pub(crate) ship: u8,
    pub(crate) bomb_gravity: Option<i16>,
    pub(crate) bomb_bounces: u8,
}

impl RemoteWeaponSource {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn capture(
        identity: RemoteWeaponSourceIdentity,
        arena_scope: Option<RemoteWeaponArenaScope>,
        fire_position: Vec2,
        fire_velocity: Vec2,
        fire_orientation: f32,
        frequency: u16,
        ship: u8,
        tuning: ShipTuning,
        gravity_bombs: bool,
    ) -> Option<Self> {
        let scope_matches_identity = matches!(
            (identity, arena_scope),
            (
                RemoteWeaponSourceIdentity::Authenticated { .. },
                Some(RemoteWeaponArenaScope { .. })
            ) | (RemoteWeaponSourceIdentity::Legacy { .. }, None)
        );
        (scope_matches_identity
            && ship < 8
            && fire_position.is_finite()
            && fire_velocity.is_finite()
            && fire_orientation.is_finite()
            && (0.0..=1.0).contains(&fire_orientation))
        .then_some(Self {
            identity,
            arena_scope,
            realtime_event_id: None,
            realtime_server_tick: None,
            shot_group_id: None,
            fire_position,
            fire_velocity,
            fire_orientation,
            frequency,
            ship,
            bomb_gravity: gravity_bombs.then_some(tuning.wormhole_gravity),
            bomb_bounces: tuning.bomb_bounces,
        })
    }

    pub(crate) fn with_realtime_order(
        mut self,
        event_id: u32,
        server_tick: u32,
        shot_group_id: u64,
    ) -> Option<Self> {
        if event_id == 0
            || shot_group_id == 0
            || !matches!(
                self.identity,
                RemoteWeaponSourceIdentity::Authenticated { .. }
            )
            || self.arena_scope.is_none()
        {
            return None;
        }
        self.realtime_event_id = Some(event_id);
        self.realtime_server_tick = Some(server_tick);
        self.shot_group_id = Some(shot_group_id);
        Some(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteProjectileAuthorityBinding {
    pub(crate) projectile_id: u32,
    pub(crate) state_tick: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RemoteProjectileClock {
    /// Server has frozen flight while an immutable contact awaits commit.
    pub(crate) motion_frozen: bool,
    pub(crate) pose_tick: u32,
    pub(crate) expiry_tick: u32,
    pub(crate) render_partial_seconds: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RemoteProjectile {
    pub(crate) source: RemoteWeaponSource,
    pub(crate) source_projectile_index: u8,
    pub(crate) authority_binding: Option<RemoteProjectileAuthorityBinding>,
    pub(crate) presentation_clock: Option<RemoteProjectileClock>,
    pub(crate) last_repel_mutation_event_id: Option<u32>,
    pub(crate) authoritative_repel_state: Option<crate::realtime_event::RealtimeRepelMutationState>,
    pub(crate) frequency: u16,
    pub(crate) weapon_data: u16,
    pub(crate) kind: RemoteProjectileKind,
    pub(crate) pos: Vec2,
    pub(crate) vel: Vec2,
    pub(crate) ttl: f32,
    pub(crate) age: f32,
    pub(crate) trail_timer: f32,
    pub(crate) bounces_remaining: u8,
    pub(crate) proximity_sensor: Option<ProximitySensorState>,
    pub(crate) level: u8,
    pub(crate) damage: f32,
    pub(crate) inactive_damage: f32,
    pub(crate) radius: f32,
    pub(crate) emp_shutdown_time: f32,
    pub(crate) shrapnel_rng_seed: u32,
}

impl RemoteProjectile {
    pub(crate) fn bind_presentation_clock(&mut self, tick: u32) {
        self.presentation_clock = Some(RemoteProjectileClock {
            pose_tick: tick,
            expiry_tick: crate::continuum::continuum_tick_add(
                tick,
                (self.ttl.max(0.0) * 100.0).round() as u32,
            ),
            render_partial_seconds: 0.0,
            motion_frozen: false,
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RemoteWeaponData {
    pub(crate) weapon_type: u8,
    pub(crate) level: u8,
    pub(crate) shrap_bouncing: bool,
    pub(crate) shrap_level: u8,
    pub(crate) shrap: u8,
    pub(crate) alternate: bool,
}

pub(crate) fn encode_weapon_data(
    weapon_type: u8,
    level: u8,
    shrap_bouncing: bool,
    shrap_level: u8,
    shrap: u8,
    alternate: bool,
) -> u16 {
    crate::gameplay_rules::encode_weapon_data(
        weapon_type,
        level,
        shrap_bouncing,
        shrap_level,
        shrap,
        alternate,
    )
}

pub(crate) fn remote_weapon_data(weapon_data: u16) -> RemoteWeaponData {
    let data = crate::gameplay_rules::WeaponDataView::decode(weapon_data);
    RemoteWeaponData {
        weapon_type: data.weapon_type,
        level: data.level,
        shrap_bouncing: data.shrapnel_bouncing,
        shrap_level: data.shrapnel_level,
        shrap: data.shrapnel_count,
        alternate: data.alternate,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteWeaponVisual {
    BombFlash,
    Repel,
    Burst,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteWeaponSoundKind {
    Gun(u8),
    Bomb(u8),
    EmpBomb(u8),
    Mine(u8),
    Repel,
    Decoy,
    Burst,
    Thor,
}

pub(crate) fn remote_projectile_uses_solid_cleanup(kind: RemoteProjectileKind) -> bool {
    matches!(
        kind,
        RemoteProjectileKind::Bullet { bouncing: false }
            | RemoteProjectileKind::Shrapnel { bouncing: false }
    )
}

pub(crate) fn remote_mine_repel_bounces(projectile: &RemoteProjectile) -> u8 {
    projectile.source.bomb_bounces
}

pub(crate) fn remote_weapon_sound(
    weapon_data: u16,
    emp_bomb_ship: bool,
) -> Option<RemoteWeaponSoundKind> {
    let data = remote_weapon_data(weapon_data);
    match data.weapon_type {
        1 | 2 => Some(RemoteWeaponSoundKind::Gun(data.level)),
        3 | 4 if emp_bomb_ship => Some(RemoteWeaponSoundKind::EmpBomb(data.level)),
        3 | 4 if data.alternate => Some(RemoteWeaponSoundKind::Mine(data.level)),
        3 | 4 => Some(RemoteWeaponSoundKind::Bomb(data.level)),
        5 => Some(RemoteWeaponSoundKind::Repel),
        6 => Some(RemoteWeaponSoundKind::Decoy),
        7 => Some(RemoteWeaponSoundKind::Burst),
        8 => Some(RemoteWeaponSoundKind::Thor),
        _ => None,
    }
}

pub(crate) fn remote_weapon_visual(weapon_data: u16) -> Option<RemoteWeaponVisual> {
    let data = remote_weapon_data(weapon_data);

    match data.weapon_type {
        3 | 4 if !data.alternate => Some(RemoteWeaponVisual::BombFlash),
        5 => Some(RemoteWeaponVisual::Repel),
        7 => Some(RemoteWeaponVisual::Burst),
        8 => Some(RemoteWeaponVisual::BombFlash),
        _ => None,
    }
}

pub(crate) fn remote_weapon_projectiles(
    source: RemoteWeaponSource,
    weapon_data: u16,
    tuning: ShipTuning,
    arena: &ArenaTuning,
) -> Vec<RemoteProjectile> {
    let data = remote_weapon_data(weapon_data);
    let initialized = crate::gameplay_rules::initialize_projectiles_for_protocol_direction(
        weapon_data,
        crate::gameplay_rules::RulesVector::new(source.fire_position.x, source.fire_position.y),
        crate::gameplay_rules::RulesVector::new(source.fire_velocity.x, source.fire_velocity.y),
        crate::continuum_position::protocol_direction(source.fire_orientation),
        crate::gameplay_rules::ProjectileInitializationTuning {
            bullet_speed: tuning.bullet_speed,
            bomb_speed: tuning.bomb_speed,
            burst_speed: tuning.burst_speed,
            ship_radius: tuning.radius_pixels,
            multifire_angle: tuning.multifire_angle,
            double_barrel: tuning.double_barrel,
            burst_count: tuning.burst_bullet_count,
            bomb_bounce_count: tuning.bomb_bounces,
        },
    );
    let bomb_damage = bomb_damage_for_ship(
        arena.bomb_damage,
        arena.bouncing_bomb_damage_multiplier,
        arena.emp_bomb_damage_multiplier,
        tuning.bomb_bounces,
        tuning.emp_bomb,
    );
    let emp_shutdown_time = if tuning.emp_bomb {
        arena.emp_bomb_shutdown_seconds
    } else {
        0.0
    };

    match data.weapon_type {
        1 | 2 => remote_gun_projectiles(source, weapon_data, data, initialized, arena),
        3 | 4 if data.alternate => initialized
            .into_iter()
            .map(|projectile| {
                let pos = vec2(projectile.position.x, projectile.position.y);
                let vel = vec2(projectile.velocity.x, projectile.velocity.y);
                RemoteProjectile {
                    source,
                    source_projectile_index: 0,
                    authority_binding: None,
                    presentation_clock: None,
                    last_repel_mutation_event_id: None,
                    authoritative_repel_state: None,
                    frequency: source.frequency,
                    weapon_data,
                    kind: RemoteProjectileKind::Mine {
                        proximity: data.weapon_type == 4,
                    },
                    pos,
                    vel,
                    ttl: weapon_lifetime(arena, WeaponLifetimeKind::Mine),
                    age: 0.0,
                    trail_timer: 0.0,
                    bounces_remaining: projectile.bounces_remaining,
                    proximity_sensor: None,
                    level: projectile.level,
                    damage: bomb_damage,
                    inactive_damage: 0.0,
                    radius: bomb_explosion_radius(arena.bomb_explode_pixels, projectile.level),
                    emp_shutdown_time,
                    shrapnel_rng_seed: shrapnel_rng_seed(
                        pos,
                        vel,
                        data.shrap as usize,
                        projectile.level,
                        source.frequency,
                    ),
                }
            })
            .collect(),
        3 | 4 => initialized
            .into_iter()
            .map(|projectile| {
                let pos = vec2(projectile.position.x, projectile.position.y);
                let vel = vec2(projectile.velocity.x, projectile.velocity.y);
                RemoteProjectile {
                    source,
                    source_projectile_index: 0,
                    authority_binding: None,
                    presentation_clock: None,
                    last_repel_mutation_event_id: None,
                    authoritative_repel_state: None,
                    frequency: source.frequency,
                    weapon_data,
                    kind: RemoteProjectileKind::Bomb {
                        proximity: data.weapon_type == 4,
                    },
                    pos,
                    vel,
                    ttl: weapon_lifetime(arena, WeaponLifetimeKind::Bomb),
                    age: 0.0,
                    trail_timer: 0.0,
                    bounces_remaining: projectile.bounces_remaining,
                    proximity_sensor: None,
                    level: projectile.level,
                    damage: bomb_damage,
                    inactive_damage: 0.0,
                    radius: bomb_explosion_radius(arena.bomb_explode_pixels, projectile.level),
                    emp_shutdown_time,
                    shrapnel_rng_seed: shrapnel_rng_seed(
                        pos,
                        vel,
                        data.shrap as usize,
                        projectile.level,
                        source.frequency,
                    ),
                }
            })
            .collect(),
        7 => initialized
            .into_iter()
            .map(|projectile| {
                let active = match projectile.behavior {
                    crate::gameplay_rules::ProjectileBehavior::Burst { active } => active,
                    _ => true,
                };
                RemoteProjectile {
                    source,
                    source_projectile_index: 0,
                    authority_binding: None,
                    presentation_clock: None,
                    last_repel_mutation_event_id: None,
                    authoritative_repel_state: None,
                    frequency: source.frequency,
                    weapon_data,
                    kind: RemoteProjectileKind::Burst { active },
                    pos: vec2(projectile.position.x, projectile.position.y),
                    vel: vec2(projectile.velocity.x, projectile.velocity.y),
                    ttl: weapon_lifetime(arena, WeaponLifetimeKind::Burst),
                    age: 0.0,
                    trail_timer: 0.0,
                    bounces_remaining: 0,
                    proximity_sensor: None,
                    level: projectile.level,
                    damage: arena.burst_damage,
                    inactive_damage: 0.0,
                    radius: 0.0,
                    emp_shutdown_time: 0.0,
                    shrapnel_rng_seed: 0,
                }
            })
            .collect(),
        8 => initialized
            .into_iter()
            .map(|projectile| {
                let pos = vec2(projectile.position.x, projectile.position.y);
                let vel = vec2(projectile.velocity.x, projectile.velocity.y);
                RemoteProjectile {
                    source,
                    source_projectile_index: 0,
                    authority_binding: None,
                    presentation_clock: None,
                    last_repel_mutation_event_id: None,
                    authoritative_repel_state: None,
                    frequency: source.frequency,
                    weapon_data,
                    kind: RemoteProjectileKind::Thor,
                    pos,
                    vel,
                    ttl: weapon_lifetime(arena, WeaponLifetimeKind::Thor),
                    age: 0.0,
                    trail_timer: 0.0,
                    bounces_remaining: 0,
                    proximity_sensor: None,
                    level: WEAPON_LEVEL_MAX,
                    damage: thor_damage_for_ship(
                        arena.bomb_damage,
                        arena.bouncing_bomb_damage_multiplier,
                        tuning.bomb_bounces,
                        data.level.saturating_sub(WEAPON_LEVEL_MIN),
                    ),
                    inactive_damage: 0.0,
                    // The reference game's `ShipController:1967-1986` sizes the Thor
                    // explosion as `BombExplodePixels * (4 + raw_level)`. The raw
                    // level lives in `data.level` as 1..=4 (already +1), so a
                    // L1 Thor uses bomb_level 4, L4 uses 7. bomb_explosion_radius
                    // multiplies base by clamp(level, 1) so passing 4 + raw_level
                    // gives the correct radius.
                    radius: bomb_explosion_radius(
                        arena.bomb_explode_pixels,
                        WEAPON_LEVEL_MAX
                            .saturating_add(data.level.saturating_sub(WEAPON_LEVEL_MIN)),
                    ),
                    emp_shutdown_time: 0.0,
                    shrapnel_rng_seed: shrapnel_rng_seed(
                        pos,
                        vel,
                        data.shrap as usize,
                        data.level,
                        source.frequency,
                    ),
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn remote_gun_projectiles(
    source: RemoteWeaponSource,
    weapon_data: u16,
    data: RemoteWeaponData,
    initialized: Vec<crate::gameplay_rules::ProjectileInitialization>,
    arena: &ArenaTuning,
) -> Vec<RemoteProjectile> {
    let bouncing = data.weapon_type == 2;
    initialized
        .into_iter()
        .map(|projectile| RemoteProjectile {
            source,
            source_projectile_index: 0,
            authority_binding: None,
            presentation_clock: None,
            last_repel_mutation_event_id: None,
            authoritative_repel_state: None,
            frequency: source.frequency,
            weapon_data,
            kind: RemoteProjectileKind::Bullet { bouncing },
            pos: vec2(projectile.position.x, projectile.position.y),
            vel: vec2(projectile.velocity.x, projectile.velocity.y),
            ttl: weapon_lifetime(arena, WeaponLifetimeKind::Bullet),
            age: 0.0,
            trail_timer: 0.0,
            bounces_remaining: projectile.bounces_remaining,
            proximity_sensor: None,
            level: projectile.level,
            damage: leveled_weapon_damage(
                arena.bullet_damage,
                arena.bullet_damage_upgrade,
                data.level,
            ),
            inactive_damage: 0.0,
            radius: 0.0,
            emp_shutdown_time: 0.0,
            shrapnel_rng_seed: 0,
        })
        .collect()
}

pub(crate) fn remote_shrapnel_projectiles(
    source: RemoteWeaponSource,
    source_weapon_data: u16,
    pos: Vec2,
    rng_seed: u32,
    arena: &ArenaTuning,
) -> Vec<RemoteProjectile> {
    let data = remote_weapon_data(source_weapon_data);
    if data.shrap == 0 {
        return Vec::new();
    }

    let weapon_data = encode_weapon_data(15, data.shrap_level, false, WEAPON_LEVEL_MIN, 0, false);
    let damage = active_shrapnel_damage(
        arena.bullet_damage,
        arena.bullet_damage_upgrade,
        arena.shrapnel_damage_multiplier,
        data.shrap_level,
    );
    let inactive_damage =
        inactive_shrapnel_damage(arena.inactive_shrapnel_damage, data.shrap_level);

    crate::gameplay_rules::initialize_shrapnel_projectiles(
        source_weapon_data,
        crate::gameplay_rules::RulesVector::new(pos.x, pos.y),
        rng_seed,
        arena.shrapnel_speed,
        arena.shrapnel_random,
    )
    .into_iter()
    .map(|projectile| RemoteProjectile {
        source,
        source_projectile_index: 0,
        authority_binding: None,
        presentation_clock: None,
        last_repel_mutation_event_id: None,
        authoritative_repel_state: None,
        frequency: source.frequency,
        weapon_data,
        kind: RemoteProjectileKind::Shrapnel {
            bouncing: data.shrap_bouncing,
        },
        pos: vec2(projectile.position.x, projectile.position.y),
        vel: vec2(projectile.velocity.x, projectile.velocity.y),
        ttl: weapon_lifetime(arena, WeaponLifetimeKind::Shrapnel),
        age: 0.0,
        trail_timer: 0.0,
        bounces_remaining: 0,
        proximity_sensor: None,
        level: data.shrap_level,
        damage,
        inactive_damage,
        radius: 0.0,
        emp_shutdown_time: 0.0,
        shrapnel_rng_seed: 0,
    })
    .collect()
}

pub(crate) fn remote_weapon_visual_position(
    weapon_data: u16,
    player_pos: Vec2,
    player_orientation: f32,
    player_radius: f32,
) -> Option<(RemoteWeaponVisual, Vec2)> {
    let visual = remote_weapon_visual(weapon_data)?;
    let pos = match visual {
        RemoteWeaponVisual::BombFlash => bomb_flash_position(
            player_pos,
            orientation_to_heading(player_orientation),
            player_radius,
        ),
        RemoteWeaponVisual::Repel | RemoteWeaponVisual::Burst => player_pos,
    };
    Some((visual, pos))
}
