use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RemoteExplosivePlayerEvent {
    DirectHit(Vec2),
    ProximityExplosion(Vec2),
}

impl RemoteExplosivePlayerEvent {
    pub(crate) fn spawns_shrapnel(self) -> bool {
        matches!(self, Self::DirectHit(_) | Self::ProximityExplosion(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RemoteBulletMotionEvent {
    PlayerHit(Vec2),
    WallImpact(Vec2),
    TimedOut,
}

pub(crate) fn update_remote_bullet_motion(
    projectile: &mut RemoteProjectile,
    dt: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    player_pos: Vec2,
    player_radius: f32,
    check_player_contact: bool,
) -> (Option<RemoteBulletMotionEvent>, Option<Vec2>) {
    update_remote_bullet_motion_observed(
        projectile,
        dt,
        map,
        bricks,
        player_pos,
        player_radius,
        check_player_contact,
        |_, _| {},
    )
}

/// Witness the ordinary forward substeps without changing their contact or
/// timing rules. Historical draw capture must use this path, not a second
/// approximation of weapon physics.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_remote_bullet_motion_observed(
    projectile: &mut RemoteProjectile,
    dt: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    player_pos: Vec2,
    player_radius: f32,
    check_player_contact: bool,
    mut observe: impl FnMut(&RemoteProjectile, f32),
) -> (Option<RemoteBulletMotionEvent>, Option<Vec2>) {
    let start_age = projectile.age - dt.max(0.0);
    let mut elapsed = 0.0;
    let mut remaining = dt.max(0.0);
    let mut bounce_pos = None;

    while remaining > 0.0 && projectile.ttl > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        remaining -= step;
        elapsed += step;

        let mut active = true;
        let event = match projectile.kind {
            RemoteProjectileKind::Bullet { bouncing: true }
            | RemoteProjectileKind::Shrapnel { bouncing: true } => {
                let bounced = move_bouncing_point_step_for_frequency(
                    map,
                    &mut projectile.pos,
                    &mut projectile.vel,
                    step,
                    projectile.frequency,
                    bricks,
                );
                if bounced {
                    bounce_pos = Some(projectile.pos);
                    if let RemoteProjectileKind::Shrapnel { .. } = projectile.kind {
                        if early_shrapnel_wall_bounce_times_out(start_age + elapsed) {
                            projectile.age = start_age + elapsed;
                            projectile.ttl = 0.0;
                            observe(projectile, elapsed);
                            return (Some(RemoteBulletMotionEvent::TimedOut), bounce_pos);
                        }
                    }
                }
                None
            }
            RemoteProjectileKind::Burst {
                active: burst_active,
            } => {
                active = burst_active;
                let bounced = move_bouncing_point_step_for_frequency(
                    map,
                    &mut projectile.pos,
                    &mut projectile.vel,
                    step,
                    projectile.frequency,
                    bricks,
                );
                if bounced {
                    bounce_pos = Some(projectile.pos);
                    projectile.kind = RemoteProjectileKind::Burst { active: true };
                    active = true;
                }
                None
            }
            RemoteProjectileKind::Bullet { .. } | RemoteProjectileKind::Shrapnel { .. } => {
                let solid_hit = move_point_until_solid_step_for_frequency(
                    map,
                    &mut projectile.pos,
                    projectile.vel,
                    step,
                    projectile.frequency,
                    bricks,
                );
                solid_hit.then_some(RemoteBulletMotionEvent::WallImpact(projectile.pos))
            }
            _ => None,
        };

        if let Some(event) = event {
            projectile.age = start_age + elapsed;
            projectile.ttl = 0.0;
            observe(projectile, elapsed);
            return (Some(event), bounce_pos);
        }

        if weapon_times_out_on_wormhole_tile(map, projectile.pos) {
            projectile.age = start_age + elapsed;
            projectile.ttl = 0.0;
            observe(projectile, elapsed);
            return (Some(RemoteBulletMotionEvent::TimedOut), bounce_pos);
        }

        if check_player_contact
            && active
            && bullet_contact_overlaps_ship(projectile.pos, player_pos, player_radius)
        {
            projectile.age = start_age + elapsed;
            projectile.ttl = 0.0;
            observe(projectile, elapsed);
            return (
                Some(RemoteBulletMotionEvent::PlayerHit(projectile.pos)),
                bounce_pos,
            );
        }
        observe(projectile, elapsed);
    }

    (None, bounce_pos)
}

pub(crate) fn update_remote_thor_player_contact(
    projectile: &mut RemoteProjectile,
    dt: f32,
    player_pos: Vec2,
    player_radius: f32,
    tuning: ArenaTuning,
) -> Option<RemoteExplosivePlayerEvent> {
    let start_age = projectile.age - dt.max(0.0);
    let mut elapsed = 0.0;
    let mut remaining = dt.max(0.0);
    while remaining > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        remaining -= step;
        elapsed += step;
        let previous_pos = projectile.pos;
        projectile.pos += projectile.vel * step;

        if explosive_contact_overlaps_ship(projectile.pos, player_pos, player_radius) {
            projectile.age = start_age + elapsed;
            projectile.ttl = 0.0;
            return Some(RemoteExplosivePlayerEvent::DirectHit(projectile.pos));
        }

        let proximity_trigger = proximity_sensor_player_trigger(
            projectile.proximity_sensor,
            projectile.pos,
            0,
            player_pos,
            player_radius,
            thor_proximity_trigger_radius(
                tuning.proximity_distance,
                crate::continuum_weapon::continuum_weapon_raw_level(projectile.weapon_data),
            ),
        );
        if proximity_bomb_sensor_exploded(
            &mut projectile.proximity_sensor,
            proximity_trigger,
            start_age + elapsed,
            step,
            tuning.bomb_explode_delay_seconds,
        ) {
            projectile.age = start_age + elapsed;
            projectile.ttl = 0.0;
            return Some(RemoteExplosivePlayerEvent::ProximityExplosion(previous_pos));
        }
    }

    None
}

pub(crate) fn update_remote_bomb_motion(
    projectile: &mut RemoteProjectile,
    dt: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    player_pos: Vec2,
    player_radius: f32,
    tuning: ArenaTuning,
    bomb_gravity: Option<i16>,
    check_player_contact: bool,
) -> (Option<BombMotionEvent>, Option<Vec2>) {
    update_remote_bomb_motion_observed(
        projectile,
        dt,
        map,
        bricks,
        player_pos,
        player_radius,
        tuning,
        bomb_gravity,
        check_player_contact,
        |_, _| {},
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_remote_bomb_motion_observed(
    projectile: &mut RemoteProjectile,
    dt: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    player_pos: Vec2,
    player_radius: f32,
    tuning: ArenaTuning,
    bomb_gravity: Option<i16>,
    check_player_contact: bool,
    mut observe: impl FnMut(&RemoteProjectile, f32),
) -> (Option<BombMotionEvent>, Option<Vec2>) {
    let proximity = matches!(
        projectile.kind,
        RemoteProjectileKind::Bomb { proximity: true }
            | RemoteProjectileKind::Mine { proximity: true }
    );
    let start_age = projectile.age - dt.max(0.0);
    let mut elapsed = 0.0;
    let mut remaining = dt.max(0.0);
    let mut bounce_pos = None;

    while remaining > 0.0 && projectile.ttl > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        remaining -= step;
        elapsed += step;
        let previous_pos = projectile.pos;

        projectile.vel = remote_bomb_wormhole_gravity_velocity(
            map,
            projectile.pos,
            projectile.vel,
            bomb_gravity,
            step,
        );

        let bounced = move_bouncing_point_for_frequency(
            map,
            &mut projectile.pos,
            &mut projectile.vel,
            step,
            projectile.frequency,
            bricks,
        );

        if weapon_times_out_on_wormhole_tile(map, projectile.pos) {
            projectile.ttl = 0.0;
            observe(projectile, elapsed);
            break;
        }

        if bounced {
            if projectile.bounces_remaining == 0 {
                projectile.age = start_age + elapsed;
                projectile.ttl = 0.0;
                observe(projectile, elapsed);
                return (
                    Some(BombMotionEvent::WallExplosion(projectile.pos)),
                    bounce_pos,
                );
            }
            projectile.bounces_remaining -= 1;
            bounce_pos = Some(projectile.pos);
        }

        if check_player_contact
            && explosive_contact_overlaps_ship(projectile.pos, player_pos, player_radius)
        {
            projectile.age = start_age + elapsed;
            projectile.ttl = 0.0;
            observe(projectile, elapsed);
            return (
                Some(BombMotionEvent::DirectHit {
                    explosion_pos: player_triggered_bomb_explosion_position(
                        previous_pos,
                        projectile.pos,
                    ),
                    damage_pos: player_triggered_bomb_damage_position(previous_pos, projectile.pos),
                }),
                bounce_pos,
            );
        }

        let proximity_exploded = if check_player_contact && proximity {
            let proximity_trigger = proximity_sensor_player_trigger(
                projectile.proximity_sensor,
                projectile.pos,
                0,
                player_pos,
                player_radius,
                proximity_bomb_trigger_radius(tuning.proximity_distance, projectile.level),
            );
            proximity_bomb_sensor_exploded(
                &mut projectile.proximity_sensor,
                proximity_trigger,
                start_age + elapsed,
                step,
                tuning.bomb_explode_delay_seconds,
            )
        } else {
            false
        };

        if proximity_exploded {
            projectile.age = start_age + elapsed;
            projectile.ttl = 0.0;
            observe(projectile, elapsed);
            return (
                Some(BombMotionEvent::ProximityExplosion {
                    explosion_pos: player_triggered_proximity_explosion_position(
                        previous_pos,
                        projectile.pos,
                    ),
                    damage_pos: player_triggered_proximity_damage_position(
                        previous_pos,
                        projectile.pos,
                    ),
                }),
                bounce_pos,
            );
        }
        observe(projectile, elapsed);
    }

    (None, bounce_pos)
}
