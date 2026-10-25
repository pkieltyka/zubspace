use super::proximity::network_player_can_trigger_local_weapon;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LocalBulletMotionEvent {
    BotHit { pos: Vec2, bot_id: usize },
    NetworkPlayerHit { pos: Vec2 },
    WallImpact(Vec2),
    TimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LocalShrapnelMotionEvent {
    BotHit { pos: Vec2, bot_id: usize },
    NetworkPlayerHit { pos: Vec2 },
    WallImpact(Vec2),
    TimedOut,
}

pub(crate) fn update_local_bullet_motion(
    bullet: &mut Bullet,
    dt: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    bots: &[BotShip],
    network_world: &NetworkWorld,
    local_frequency: u16,
    tuning: ArenaTuning,
) -> (Option<LocalBulletMotionEvent>, Option<Vec2>) {
    let start_age = bullet.age;
    let mut elapsed = 0.0;
    let mut remaining = dt.max(0.0);
    let mut bounce_pos = None;

    while remaining > 0.0 && bullet.ttl > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        remaining -= step;
        elapsed += step;

        if bullet.activates_on_wall || bullet.bouncing || bullet.bounces_remaining > 0 {
            let bounced = move_bouncing_point_step_for_frequency(
                map,
                &mut bullet.pos,
                &mut bullet.vel,
                step,
                bullet.frequency,
                bricks,
            );
            if bounced {
                bounce_pos = Some(bullet.pos);
                if bullet.activates_on_wall {
                    bullet.active = true;
                } else if !bullet.bouncing && bullet.bounces_remaining > 0 {
                    bullet.bounces_remaining -= 1;
                }
            }
        } else if move_point_until_solid_step_for_frequency(
            map,
            &mut bullet.pos,
            bullet.vel,
            step,
            bullet.frequency,
            bricks,
        ) {
            bullet.age = start_age + elapsed;
            bullet.ttl = 0.0;
            return (
                Some(LocalBulletMotionEvent::WallImpact(bullet.pos)),
                bounce_pos,
            );
        }

        if weapon_times_out_on_wormhole_tile(map, bullet.pos) {
            bullet.age = start_age + elapsed;
            bullet.ttl = 0.0;
            return (Some(LocalBulletMotionEvent::TimedOut), bounce_pos);
        }

        if bullet.active {
            if let Some(bot) = bots.iter().find(|bot| {
                bot.respawn_timer <= 0.0
                    && bullet_contact_overlaps_ship(
                        bullet.pos,
                        bot.pos,
                        tuning.ship(bot.ship_type).radius_pixels,
                    )
            }) {
                bullet.age = start_age + elapsed;
                bullet.ttl = 0.0;
                return (
                    Some(LocalBulletMotionEvent::BotHit {
                        pos: bullet.pos,
                        bot_id: bot.id,
                    }),
                    bounce_pos,
                );
            }
            if network_world.players.values().any(|player| {
                let Some(target_pos) = player.position else {
                    return false;
                };
                network_player_can_trigger_local_weapon(player, local_frequency)
                    && bullet_contact_overlaps_ship(
                        bullet.pos,
                        target_pos,
                        tuning.ship(player.ship as usize).radius_pixels,
                    )
            }) {
                bullet.age = start_age + elapsed;
                bullet.ttl = 0.0;
                return (
                    Some(LocalBulletMotionEvent::NetworkPlayerHit { pos: bullet.pos }),
                    bounce_pos,
                );
            }
        }
    }

    (None, bounce_pos)
}

pub(crate) fn update_local_shrapnel_motion(
    shrap: &mut Shrapnel,
    dt: f32,
    map: &ArenaMap,
    bricks: &[Brick],
    bots: &[BotShip],
    network_world: &NetworkWorld,
    local_frequency: u16,
    tuning: ArenaTuning,
) -> (Option<LocalShrapnelMotionEvent>, Option<Vec2>) {
    let start_age = shrap.age;
    let mut elapsed = 0.0;
    let mut remaining = dt.max(0.0);
    let mut bounce_pos = None;

    while remaining > 0.0 && shrap.ttl > 0.0 {
        let step = remaining.min(WEAPON_COLLISION_STEP_SECONDS);
        remaining -= step;
        elapsed += step;

        if shrap.bouncing {
            let bounced = move_bouncing_point_step_for_frequency(
                map,
                &mut shrap.pos,
                &mut shrap.vel,
                step,
                shrap.frequency,
                bricks,
            );
            if bounced {
                bounce_pos = Some(shrap.pos);
                if early_shrapnel_wall_bounce_times_out(start_age + elapsed) {
                    shrap.age = start_age + elapsed;
                    shrap.ttl = 0.0;
                    return (Some(LocalShrapnelMotionEvent::TimedOut), bounce_pos);
                }
            }
        } else if move_point_until_solid_step_for_frequency(
            map,
            &mut shrap.pos,
            shrap.vel,
            step,
            shrap.frequency,
            bricks,
        ) {
            shrap.age = start_age + elapsed;
            shrap.ttl = 0.0;
            return (
                Some(LocalShrapnelMotionEvent::WallImpact(shrap.pos)),
                bounce_pos,
            );
        }

        if weapon_times_out_on_wormhole_tile(map, shrap.pos) {
            shrap.age = start_age + elapsed;
            shrap.ttl = 0.0;
            return (Some(LocalShrapnelMotionEvent::TimedOut), bounce_pos);
        }

        if let Some(bot) = bots.iter().find(|bot| {
            bot.respawn_timer <= 0.0
                && bullet_contact_overlaps_ship(
                    shrap.pos,
                    bot.pos,
                    tuning.ship(bot.ship_type).radius_pixels,
                )
        }) {
            shrap.age = start_age + elapsed;
            shrap.ttl = 0.0;
            return (
                Some(LocalShrapnelMotionEvent::BotHit {
                    pos: shrap.pos,
                    bot_id: bot.id,
                }),
                bounce_pos,
            );
        }
        if network_world.players.values().any(|player| {
            let Some(target_pos) = player.position else {
                return false;
            };
            network_player_can_trigger_local_weapon(player, local_frequency)
                && bullet_contact_overlaps_ship(
                    shrap.pos,
                    target_pos,
                    tuning.ship(player.ship as usize).radius_pixels,
                )
        }) {
            shrap.age = start_age + elapsed;
            shrap.ttl = 0.0;
            return (
                Some(LocalShrapnelMotionEvent::NetworkPlayerHit { pos: shrap.pos }),
                bounce_pos,
            );
        }
    }

    (None, bounce_pos)
}
