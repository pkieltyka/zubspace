use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BotBulletContactTarget {
    pub(crate) decoy_index: usize,
    pub(crate) pos: Vec2,
    pub(crate) radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BotBulletMotionEvent {
    PlayerHit(Vec2),
    DecoyHit { pos: Vec2, decoy_index: usize },
    WallImpact(Vec2),
    TimedOut,
}

pub(crate) fn update_bot_bullet_motion_with_contacts(
    map: &ArenaMap,
    bricks: &[Brick],
    bullet: &mut BotBullet,
    dt: f32,
    player_target: Option<(Vec2, f32)>,
    decoy_targets: &[BotBulletContactTarget],
) -> (Option<BotBulletMotionEvent>, Option<Vec2>) {
    let start_age = bullet.age;
    let start_trail_timer = bullet.trail_timer;
    let mut elapsed = 0.0;
    let mut remaining = dt.max(0.0);
    let mut bounce_pos = None;

    while remaining > 0.0 && bullet.ttl > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        remaining -= step;
        elapsed += step;

        let solid_hit = if bullet.bouncing {
            move_bouncing_point_step_for_frequency(
                map,
                &mut bullet.pos,
                &mut bullet.vel,
                step,
                bullet.frequency,
                bricks,
            )
        } else {
            move_point_until_solid_step_for_frequency(
                map,
                &mut bullet.pos,
                bullet.vel,
                step,
                bullet.frequency,
                bricks,
            )
        };

        if solid_hit && bullet.bouncing {
            bounce_pos = Some(bullet.pos);
        } else if solid_hit {
            bullet.age = start_age + elapsed;
            bullet.trail_timer = start_trail_timer - elapsed;
            bullet.ttl = 0.0;
            return (
                Some(BotBulletMotionEvent::WallImpact(bullet.pos)),
                bounce_pos,
            );
        }

        if weapon_times_out_on_wormhole_tile(map, bullet.pos) {
            bullet.age = start_age + elapsed;
            bullet.trail_timer = start_trail_timer - elapsed;
            bullet.ttl = 0.0;
            return (Some(BotBulletMotionEvent::TimedOut), bounce_pos);
        }

        if let Some(target) = decoy_targets
            .iter()
            .find(|target| bot_bullet_hits_ship(bullet.pos, target.pos, target.radius))
        {
            bullet.age = start_age + elapsed;
            bullet.trail_timer = start_trail_timer - elapsed;
            bullet.ttl = 0.0;
            return (
                Some(BotBulletMotionEvent::DecoyHit {
                    pos: bullet.pos,
                    decoy_index: target.decoy_index,
                }),
                bounce_pos,
            );
        }

        if let Some((player_pos, player_radius)) = player_target {
            if bot_bullet_hits_ship(bullet.pos, player_pos, player_radius) {
                bullet.age = start_age + elapsed;
                bullet.trail_timer = start_trail_timer - elapsed;
                bullet.ttl = 0.0;
                return (
                    Some(BotBulletMotionEvent::PlayerHit(bullet.pos)),
                    bounce_pos,
                );
            }
        }
    }

    bullet.ttl -= dt;
    bullet.age += dt;
    bullet.trail_timer -= dt;
    (None, bounce_pos)
}
