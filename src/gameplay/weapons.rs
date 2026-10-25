use super::*;

mod bot_bullets;
mod gravity;
mod local_projectiles;
mod presentation;
mod proximity;
mod remote_projectiles;

pub(crate) use bot_bullets::*;
pub(crate) use gravity::*;
pub(crate) use local_projectiles::*;
pub(crate) use presentation::*;
pub(crate) use proximity::*;
pub(crate) use remote_projectiles::*;

pub(crate) fn explosion_bot_damage(explosion: &Explosion, bot_pos: Vec2) -> f32 {
    if !explosion.damages_bots {
        return 0.0;
    }

    radial_damage(
        explosion.damage_pos.distance(bot_pos),
        explosion.radius,
        explosion.damage,
    )
}

pub(crate) fn explosion_emp_shutdown_seconds(explosion: &Explosion, damage: f32) -> f32 {
    if explosion.emp_shutdown_time <= 0.0 || explosion.damage <= 0.0 || damage <= 0.0 {
        0.0
    } else {
        explosion.emp_shutdown_time * (damage / explosion.damage).clamp(0.0, 1.0)
    }
}

pub(crate) fn emp_shutdown_seconds_from_raw_damage(
    emp_shutdown_time: f32,
    raw_damage: f32,
    base_damage: f32,
) -> f32 {
    if emp_shutdown_time <= 0.0 || raw_damage <= 0.0 || base_damage <= 0.0 {
        0.0
    } else {
        emp_shutdown_time * (raw_damage / base_damage).clamp(0.0, 1.0)
    }
}

pub(crate) fn explosion_kind_for_emp_shutdown(emp_shutdown_time: f32) -> ExplosionKind {
    if emp_shutdown_time > 0.0 {
        ExplosionKind::EmpBomb
    } else {
        ExplosionKind::Bomb
    }
}

pub(crate) fn explosion_visual_lifetime(kind: ExplosionKind) -> f32 {
    match kind {
        ExplosionKind::Bomb => EXPLOSION_LIFETIME,
        ExplosionKind::EmpBomb => EMP_EXPLOSION_LIFETIME,
    }
}

pub(crate) fn player_triggered_bomb_explosion_position(
    previous_pos: Vec2,
    _current_pos: Vec2,
) -> Vec2 {
    previous_pos
}

pub(crate) fn player_triggered_bomb_damage_position(
    _previous_pos: Vec2,
    current_pos: Vec2,
) -> Vec2 {
    current_pos
}

pub(crate) fn player_triggered_proximity_explosion_position(
    previous_pos: Vec2,
    _current_pos: Vec2,
) -> Vec2 {
    previous_pos
}

pub(crate) fn player_triggered_proximity_damage_position(
    _previous_pos: Vec2,
    current_pos: Vec2,
) -> Vec2 {
    current_pos
}

pub(crate) fn bomb_sprite_frame_start(level: u8, bounces_remaining: u8, emp: bool) -> usize {
    let base = if emp {
        EMP_BOMB_FRAME_START
    } else if bounces_remaining > 0 {
        BOUNCEABLE_BOMB_FRAME_START
    } else {
        0
    };
    base + weapon_level_index(level) * BOMB_FRAMES_PER_LEVEL
}

/// The reference game's Graphics splits the 8x8 sheet into six ten-frame loops:
/// levels 1–3, then the same levels with bouncing enabled. There is no fourth
/// artwork level; clamp unsupported wire levels to the strongest shipped art.
pub(crate) fn shrapnel_sprite_frame_start(level: u8, bouncing: bool) -> usize {
    usize::from(level.clamp(1, 3) - 1) * 10 + if bouncing { 30 } else { 0 }
}

pub(crate) fn mine_sprite_frame_start(level: u8, emp: bool) -> usize {
    let base = if emp { EMP_MINE_FRAME_START } else { 0 };
    base + weapon_level_index(level) * MINE_FRAMES_PER_LEVEL
}

pub(crate) fn mine_triggered_by_bot(mine_pos: Vec2, bots: &[BotShip], tuning: ArenaTuning) -> bool {
    bots.iter().any(|bot| {
        bot.respawn_timer <= 0.0
            && explosive_contact_overlaps_ship(
                mine_pos,
                bot.pos,
                tuning.ship(bot.ship_type).radius_pixels,
            )
    })
}

pub(crate) fn weapon_lifetime(tuning: &ArenaTuning, kind: WeaponLifetimeKind) -> f32 {
    match kind {
        WeaponLifetimeKind::Bullet | WeaponLifetimeKind::Burst | WeaponLifetimeKind::Shrapnel => {
            tuning.bullet_lifetime
        }
        WeaponLifetimeKind::Bomb | WeaponLifetimeKind::Thor => tuning.bomb_lifetime,
        WeaponLifetimeKind::Mine => tuning.mine_lifetime,
        WeaponLifetimeKind::Decoy => tuning.decoy_lifetime,
    }
}

pub(crate) fn refresh_repelled_weapon_lifetime(ttl: &mut f32, lifetime: f32) {
    *ttl = lifetime;
}

pub(crate) fn refresh_repelled_shrapnel_lifetime(ttl: &mut f32, age: &mut f32, lifetime: f32) {
    refresh_repelled_weapon_lifetime(ttl, lifetime);
    *age = 0.0;
}

pub(crate) fn weapon_owner_position_in_safe_tile(
    map: &ArenaMap,
    owner_position: Option<Vec2>,
) -> bool {
    owner_position.is_some_and(|position| map.tile_at_world(position) == SAFE_TILE_ID)
}

pub(crate) fn weapon_times_out_on_wormhole_tile(map: &ArenaMap, weapon_pos: Vec2) -> bool {
    map.tile_at_world(weapon_pos) == WORMHOLE_TILE_ID
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SpecialWeaponAction {
    Repel,
    Burst,
    Thor,
    Decoy,
}

#[cfg(test)]
pub(crate) fn prioritized_special_weapon_action(
    repel: bool,
    burst: bool,
    thor: bool,
    decoy: bool,
) -> Option<SpecialWeaponAction> {
    if repel {
        Some(SpecialWeaponAction::Repel)
    } else if burst {
        Some(SpecialWeaponAction::Burst)
    } else if thor {
        Some(SpecialWeaponAction::Thor)
    } else if decoy {
        Some(SpecialWeaponAction::Decoy)
    } else {
        None
    }
}

pub(crate) fn shrapnel_spawn_allowed(
    map: &ArenaMap,
    spawn_pos: Vec2,
    frequency: u16,
    bricks: &[Brick],
) -> bool {
    !is_solid_world_for_frequency(map, spawn_pos, frequency, bricks)
}

/// Mines start at rest, but the reference game applies GravityBombs to both regular
/// and alternate bombs. Keep their velocity so a well can pull a mine too.
/// Returns (wall impact, wormhole timeout); neither result authors damage.
pub(crate) fn update_mine_terrain_motion(
    position: &mut Vec2,
    velocity: &mut Vec2,
    dt: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    frequency: u16,
    gravity: Option<i16>,
) -> (bool, bool) {
    let mut remaining = dt.max(0.0);
    while remaining > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        remaining -= step;
        *velocity = remote_bomb_wormhole_gravity_velocity(map, *position, *velocity, gravity, step);
        let wall = move_point_until_solid_step_for_frequency(
            map, position, *velocity, step, frequency, bricks,
        );
        if weapon_times_out_on_wormhole_tile(map, *position) {
            return (false, true);
        }
        if wall {
            return (true, false);
        }
    }
    (false, false)
}

pub(crate) fn update_decoy_motion(
    map: &ArenaMap,
    bricks: &[Brick],
    decoy: &mut Decoy,
    dt: f32,
) -> bool {
    let bounced = move_bouncing_point_for_frequency(
        map,
        &mut decoy.pos,
        &mut decoy.vel,
        dt,
        decoy.frequency,
        bricks,
    );
    if weapon_times_out_on_wormhole_tile(map, decoy.pos) {
        decoy.ttl = 0.0;
    } else {
        decoy.ttl -= dt;
    }
    decoy.age += dt;
    bounced
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BombMotionEvent {
    DirectHit {
        explosion_pos: Vec2,
        damage_pos: Vec2,
    },
    WallExplosion(Vec2),
    ProximityExplosion {
        explosion_pos: Vec2,
        damage_pos: Vec2,
    },
}

impl BombMotionEvent {
    pub(crate) fn spawns_shrapnel(self) -> bool {
        // The reference game's Update calls CreateExplosion for both PlayerExplosion
        // and WallExplosion. Direct contact does not clear the fragment fan.
        matches!(
            self,
            Self::DirectHit { .. } | Self::WallExplosion(_) | Self::ProximityExplosion { .. }
        )
    }
}

pub(crate) fn explosion_shrapnel_for_bomb_event(
    event: BombMotionEvent,
    shrapnel: ExplosiveShrapnel,
) -> ExplosiveShrapnel {
    if event.spawns_shrapnel() {
        shrapnel
    } else {
        shrapnel.without_fragments()
    }
}

pub(crate) fn update_bomb_motion(
    bomb: &mut Bomb,
    dt: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    bots: &[BotShip],
    network_world: &NetworkWorld,
    local_frequency: u16,
    tuning: ArenaTuning,
    bomb_gravity: Option<i16>,
) -> Option<BombMotionEvent> {
    let mut remaining = dt.max(0.0);
    while remaining > 0.0 && bomb.ttl > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        remaining -= step;
        let previous_pos = bomb.pos;

        if let Some(gravity) = bomb_gravity {
            bomb.vel = bomb_wormhole_gravity_velocity(map, bomb.pos, bomb.vel, gravity, true, step);
        }

        let bounced = move_bouncing_point_for_frequency(
            map,
            &mut bomb.pos,
            &mut bomb.vel,
            step,
            bomb.frequency,
            bricks,
        );
        bomb.ttl -= step;
        bomb.age += step;

        if weapon_times_out_on_wormhole_tile(map, bomb.pos) {
            bomb.ttl = 0.0;
            break;
        }

        if bounced {
            if bomb.bounces_remaining == 0 {
                let pos = bomb.pos;
                bomb.ttl = 0.0;
                return Some(BombMotionEvent::WallExplosion(pos));
            }
            bomb.bounces_remaining -= 1;
        }

        let hit_target = explosive_contact_triggered_by_bot(bomb.pos, bots, tuning)
            || explosive_contact_triggered_by_network_player(
                bomb.pos,
                network_world,
                local_frequency,
                tuning,
            );
        let proximity_trigger = if bomb.proximity {
            let radius = proximity_bomb_trigger_radius(tuning.proximity_distance, bomb.level);
            proximity_bomb_trigger_by_bot(bomb.proximity_sensor, bomb.pos, bots, tuning, radius)
                .or_else(|| {
                    proximity_bomb_trigger_by_network_player(
                        bomb.proximity_sensor,
                        bomb.pos,
                        network_world,
                        local_frequency,
                        tuning,
                        radius,
                    )
                })
        } else {
            None
        };
        let proximity_exploded = bomb.proximity
            && proximity_bomb_sensor_exploded(
                &mut bomb.proximity_sensor,
                proximity_trigger,
                bomb.age,
                step,
                tuning.bomb_explode_delay_seconds,
            );

        if hit_target {
            let explosion_pos = player_triggered_bomb_explosion_position(previous_pos, bomb.pos);
            let damage_pos = player_triggered_bomb_damage_position(previous_pos, bomb.pos);
            bomb.ttl = 0.0;
            return Some(BombMotionEvent::DirectHit {
                explosion_pos,
                damage_pos,
            });
        } else if proximity_exploded {
            let explosion_pos =
                player_triggered_proximity_explosion_position(previous_pos, bomb.pos);
            let damage_pos = player_triggered_proximity_damage_position(previous_pos, bomb.pos);
            bomb.ttl = 0.0;
            return Some(BombMotionEvent::ProximityExplosion {
                explosion_pos,
                damage_pos,
            });
        } else if bomb.ttl <= 0.0 {
            bomb.ttl = 0.0;
        }
    }

    None
}

pub(crate) fn update_thor_motion(
    thor: &mut Thor,
    dt: f32,
    map: &ArenaMap,
    bots: &[BotShip],
    network_world: &NetworkWorld,
    local_frequency: u16,
    tuning: ArenaTuning,
) -> Option<(Vec2, f32, ExplosiveShrapnel)> {
    let mut remaining = dt.max(0.0);
    while remaining > 0.0 && thor.ttl > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        remaining -= step;

        let previous_pos = thor.pos;
        thor.pos += thor.vel * step;
        thor.ttl -= step;
        thor.age += step;
        if weapon_times_out_on_wormhole_tile(map, thor.pos) {
            thor.ttl = 0.0;
            break;
        }

        let hit_target = explosive_contact_triggered_by_bot(thor.pos, bots, tuning)
            || explosive_contact_triggered_by_network_player(
                thor.pos,
                network_world,
                local_frequency,
                tuning,
            );
        if hit_target {
            thor.ttl = 0.0;
            return Some((thor.pos, thor.damage, thor.shrapnel));
        }

        // Use the Thor's actual raw level so a remote-relayed Thor at a higher
        // level gets the correctly-sized proximity sensor (the reference game:
        // `prox = ProximityDistance + raw_level + 3`).
        let radius = thor_proximity_trigger_radius(tuning.proximity_distance, thor.raw_level);
        let proximity_trigger =
            proximity_bomb_trigger_by_bot(thor.proximity_sensor, thor.pos, bots, tuning, radius)
                .or_else(|| {
                    proximity_bomb_trigger_by_network_player(
                        thor.proximity_sensor,
                        thor.pos,
                        network_world,
                        local_frequency,
                        tuning,
                        radius,
                    )
                });
        let proximity_exploded = proximity_bomb_sensor_exploded(
            &mut thor.proximity_sensor,
            proximity_trigger,
            thor.age,
            step,
            tuning.bomb_explode_delay_seconds,
        );
        if proximity_exploded {
            let pos = player_triggered_proximity_explosion_position(previous_pos, thor.pos);
            thor.ttl = 0.0;
            return Some((pos, thor.damage, thor.shrapnel));
        }
    }

    if thor.ttl <= 0.0 {
        thor.ttl = 0.0;
    }
    None
}
