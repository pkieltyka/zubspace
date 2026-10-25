use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WeaponPresentation {
    pub(crate) position: Vec2,
    pub(crate) animation_seconds: f32,
    pub(crate) bounces_remaining: u8,
    pub(crate) burst_active: bool,
    /// Cosmetic owner-only clock lead; never changes canonical pose or TTL.
    pub(crate) owner_prediction_lead_seconds: f32,
}

fn weapon_render_seconds(partial_seconds: f32, ttl: f32) -> f32 {
    if !partial_seconds.is_finite() || !ttl.is_finite() {
        return 0.0;
    }
    partial_seconds
        .max(0.0)
        .min(crate::fixed_step::GAMEPLAY_STEP_SECONDS)
        .min(ttl.max(0.0))
}

/// Project a disposable copy through the camera's fixed-step remainder. Use
/// the ordinary terrain/gravity/bounce simulator, without actor contacts or
/// effects. A predicted terminal only clamps the displayed pose: callers must
/// still draw the existing weapon and leave its lifetime/retirement untouched.
pub(crate) fn local_bomb_presentation(
    bomb: &Bomb,
    partial_seconds: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    tuning: &ArenaTuning,
    bomb_gravity: Option<i16>,
) -> WeaponPresentation {
    let dt = weapon_render_seconds(partial_seconds, bomb.ttl);
    let mut projected = *bomb;
    if dt > 0.0 {
        // Empty containers allocate no actor records. This is intentionally a
        // terrain-only sample; rendering cannot invent a player collision.
        let _ = update_bomb_motion(
            &mut projected,
            dt,
            map,
            bricks,
            &[],
            &NetworkWorld::default(),
            bomb.frequency,
            *tuning,
            bomb_gravity,
        );
    }
    WeaponPresentation {
        position: projected.pos,
        // The reference game advances one per-weapon animation clock. Adding wall time
        // to age advances twice and also loses precision as uptime grows.
        animation_seconds: bomb.age + dt,
        bounces_remaining: projected.bounces_remaining,
        burst_active: false,
        owner_prediction_lead_seconds: 0.0,
    }
}

pub(crate) fn remote_projectile_presentation(
    projectile: &RemoteProjectile,
    partial_seconds: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    tuning: &ArenaTuning,
) -> WeaponPresentation {
    let dt = weapon_render_seconds(
        projectile
            .presentation_clock
            .map_or(partial_seconds, |clock| {
                if clock.motion_frozen {
                    0.0
                } else {
                    clock.render_partial_seconds
                }
            }),
        projectile.ttl,
    );
    let mut projected = *projectile;
    if dt > 0.0 {
        // The remote simulator receives age after its ordinary age increment.
        // Do not pre-decrement TTL: a last partial step may reach its deadline.
        projected.age += dt;
        match projected.kind {
            RemoteProjectileKind::Bullet { .. }
            | RemoteProjectileKind::Burst { .. }
            | RemoteProjectileKind::Shrapnel { .. } => {
                let _ = update_remote_bullet_motion(
                    &mut projected,
                    dt,
                    map,
                    bricks,
                    Vec2::ZERO,
                    0.0,
                    false,
                );
            }
            RemoteProjectileKind::Bomb { .. } | RemoteProjectileKind::Mine { .. } => {
                let _ = update_remote_bomb_motion(
                    &mut projected,
                    dt,
                    map,
                    bricks,
                    Vec2::ZERO,
                    0.0,
                    *tuning,
                    projectile.source.bomb_gravity,
                    false,
                );
            }
            RemoteProjectileKind::Thor => projected.pos += projected.vel * dt,
        }
    }
    WeaponPresentation {
        position: projected.pos,
        animation_seconds: projectile.age + dt,
        bounces_remaining: projected.bounces_remaining,
        burst_active: matches!(projected.kind, RemoteProjectileKind::Burst { active: true }),
        owner_prediction_lead_seconds: 0.0,
    }
}

pub(crate) fn local_bullet_presentation(
    bullet: &Bullet,
    partial_seconds: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    tuning: &ArenaTuning,
) -> WeaponPresentation {
    let dt = weapon_render_seconds(partial_seconds, bullet.ttl);
    let mut projected = *bullet;
    if dt > 0.0 {
        let _ = update_local_bullet_motion(
            &mut projected,
            dt,
            map,
            bricks,
            &[],
            &NetworkWorld::default(),
            bullet.frequency,
            *tuning,
        );
    }
    WeaponPresentation {
        position: projected.pos,
        animation_seconds: bullet.age + dt,
        bounces_remaining: projected.bounces_remaining,
        burst_active: projected.active,
        owner_prediction_lead_seconds: 0.0,
    }
}

pub(crate) fn local_shrapnel_presentation(
    shrapnel: &Shrapnel,
    partial_seconds: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    tuning: &ArenaTuning,
) -> WeaponPresentation {
    let dt = weapon_render_seconds(partial_seconds, shrapnel.ttl);
    let mut projected = *shrapnel;
    if dt > 0.0 {
        let _ = update_local_shrapnel_motion(
            &mut projected,
            dt,
            map,
            bricks,
            &[],
            &NetworkWorld::default(),
            shrapnel.frequency,
            *tuning,
        );
    }
    WeaponPresentation {
        position: projected.pos,
        animation_seconds: shrapnel.age + dt,
        bounces_remaining: 0,
        burst_active: false,
        owner_prediction_lead_seconds: 0.0,
    }
}

pub(crate) fn bot_bullet_presentation(
    bullet: &BotBullet,
    partial_seconds: f32,
    map: &ArenaMap,
    bricks: &[Brick],
) -> WeaponPresentation {
    let dt = weapon_render_seconds(partial_seconds, bullet.ttl);
    let mut projected = *bullet;
    if dt > 0.0 {
        let _ = update_bot_bullet_motion_with_contacts(map, bricks, &mut projected, dt, None, &[]);
    }
    WeaponPresentation {
        position: projected.pos,
        animation_seconds: bullet.age + dt,
        bounces_remaining: 0,
        burst_active: false,
        owner_prediction_lead_seconds: 0.0,
    }
}

pub(crate) fn thor_presentation(thor: &Thor, partial_seconds: f32) -> WeaponPresentation {
    let dt = weapon_render_seconds(partial_seconds, thor.ttl);
    WeaponPresentation {
        position: thor.pos + thor.vel * dt,
        animation_seconds: thor.age + dt,
        bounces_remaining: 0,
        burst_active: false,
        owner_prediction_lead_seconds: 0.0,
    }
}

pub(crate) fn decoy_presentation_position(
    decoy: &Decoy,
    partial_seconds: f32,
    map: &ArenaMap,
    bricks: &[Brick],
) -> Vec2 {
    let dt = weapon_render_seconds(
        decoy
            .presentation_clock
            .map_or(partial_seconds, |clock| clock.render_partial_seconds),
        decoy.ttl,
    );
    let mut pos = decoy.pos;
    let mut vel = decoy.vel;
    if dt > 0.0 {
        // Copy only kinematics, not the owned label string or runtime identity.
        move_bouncing_point_for_frequency(map, &mut pos, &mut vel, dt, decoy.frequency, bricks);
    }
    pos
}
