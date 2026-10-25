//! Short, bounded tactical forecasts only. The production projectile registry
//! still owns motion, contacts and damage after a shot commits.
use super::{
    BOT_COMBAT_ORBIT_RADIUS, BOT_FIRE_COOLDOWN_MULTIPLIER, BotTarget, bot_weapon_intercept_target,
};
use crate::state::ServerBot;

pub(super) fn bot_launch_velocity(
    ship: u8,
    weapon: u16,
    direction: u8,
    x: f32,
    y: f32,
) -> crate::gameplay_rules::RulesVector {
    let recoil = if matches!(weapon & 0x1f, 3 | 4) && weapon & 0x8000 == 0 {
        crate::arena_settings::arena_ship_bomb_recoil_speed(ship)
    } else {
        0.0
    };
    let (hx, hy) = crate::continuum_position::canonical_discrete_heading_components(
        crate::continuum_position::protocol_direction_to_orientation(direction),
    );
    crate::gameplay_rules::RulesVector::new(x - hx * recoil, y - hy * recoil)
}

pub(super) fn bot_bomb_safety_blocked(
    bot: &ServerBot,
    weapon: u16,
    targets: impl Iterator<Item = (u16, f32, f32)>,
) -> bool {
    if weapon & 0x1f != 4 || !crate::arena_settings::arena_bomb_safety_enabled() {
        return false;
    }
    let radius = crate::continuum_weapon::continuum_proximity_bomb_firing_safety_radius(
        crate::arena_settings::arena_proximity_distance(),
        crate::continuum_weapon::continuum_weapon_level(weapon),
    );
    targets.into_iter().any(|(frequency, x, y)| {
        frequency != bot.player.frequency && (x - bot.x).hypot(y - bot.y) <= radius
    })
}

pub(super) fn bot_fire_ready(bot: &ServerBot, tick: u32, next_weapon: u16) -> bool {
    // Switching back to guns must not bypass the preceding bomb's lockout.
    let previous = bot.last_fire_weapon_data.unwrap_or(next_weapon);
    let delay = crate::arena_settings::arena_ship_weapon_fire_delay_ticks(
        bot.player.ship,
        (previous & 0x1f) as u8,
    )
    .saturating_mul(BOT_FIRE_COOLDOWN_MULTIPLIER);
    bot.last_fire_tick
        .is_none_or(|last| crate::continuum::continuum_tick_elapsed(tick, last) >= delay)
}

fn bomb_reserve_available(bot: &ServerBot, weapon: u16) -> bool {
    let cost = crate::arena_settings::arena_ship_weapon_energy_cost(bot.player.ship, weapon);
    let energy = f32::from(bot.energy) + bot.energy_remainder;
    energy > f32::from(cost) && energy - f32::from(cost) >= f32::from(bot.max_energy) * 0.25
}

pub(super) fn bot_tactical_orbit_radius(bot: &ServerBot) -> f32 {
    if bot.consecutive_gun_volleys >= 2 {
        if let Some(weapon) = bot
            .bomb_weapon_data()
            .filter(|&weapon| bomb_reserve_available(bot, weapon))
        {
            let blast = crate::arena_settings::arena_bomb_explode_pixels()
                * f32::from(crate::continuum_weapon::continuum_weapon_level(weapon));
            let hull = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship);
            return BOT_COMBAT_ORBIT_RADIUS.max(blast + 2.0 * hull + 48.0);
        }
    }
    BOT_COMBAT_ORBIT_RADIUS
}

pub(super) fn bot_combat_weapon(
    bot: &ServerBot,
    target: BotTarget,
    tick: u32,
    solid: impl Fn(f32, f32) -> bool,
) -> Option<u16> {
    let gun = bot.gun_weapon_data();
    let Some(bomb) = bot.bomb_weapon_data() else {
        return gun;
    };
    if bot.consecutive_gun_volleys < 2
        || target.player_id.is_none()
        || target.in_safe_zone
        || !super::bot_can_detect_target(&target, bot.xradar)
        || bot.shutdown_active(tick)
        || !bomb_reserve_available(bot, bomb)
        || !bot_fire_ready(bot, tick, bomb)
        || (target.x - bot.x).hypot(target.y - bot.y) > super::BOT_COMBAT_PURSUIT_DISTANCE
    {
        return gun;
    }
    let lead = bot_weapon_intercept_target(bot, target, bomb);
    let orientation = (lead.x - bot.x).atan2(-(lead.y - bot.y)) / std::f32::consts::TAU;
    let direction = crate::continuum_position::protocol_direction(
        super::nearest_protocol_orientation(orientation.rem_euclid(1.0)),
    );
    let tuning = crate::arena_settings::arena_projectile_initialization_tuning(bot.player.ship);
    let launch_velocity = bot_launch_velocity(
        bot.player.ship,
        bomb,
        direction,
        bot.velocity_x,
        bot.velocity_y,
    );
    let initial = crate::gameplay_rules::initialize_projectiles_for_protocol_direction(
        bomb,
        crate::gameplay_rules::RulesVector::new(bot.x, bot.y),
        launch_velocity,
        direction,
        tuning,
    )
    .into_iter()
    .next()?;
    let blast = crate::arena_settings::arena_bomb_explode_pixels()
        * f32::from(crate::continuum_weapon::continuum_weapon_level(bomb));
    let hull = crate::arena_settings::arena_ship_radius_pixels(bot.player.ship);
    let mut position = initial.position;
    let mut velocity = initial.velocity;
    let mut bounces = tuning.bomb_bounce_count;
    let mut sensor: Option<(f32, u32)> = None;
    // Two seconds / 200 shared 100Hz steps: no lifetime-sized work on the tick.
    let steps = crate::arena_settings::arena_weapon_lifetime_ticks(bomb)
        .unwrap_or(0)
        .min(200);
    for step in 1..=steps {
        let contact = crate::gameplay_rules::weapon_motion::step_terrain(
            &mut position,
            &mut velocity,
            0.01,
            bounces > 0,
            false,
            &solid,
        );
        let wall_explosion = contact.any() && bounces == 0;
        if contact.any() {
            bounces = bounces.saturating_sub(1);
        }
        let seconds = step as f32 * 0.01;
        let target_x = target.x + target.velocity_x * seconds;
        let target_y = target.y + target.velocity_y * seconds;
        // A tactical estimate, not a new collision authority: standard 14px
        // target hull, linear target/self motion, current terrain. Recheck
        // after movement and use a generous self-blast margin.
        let direct = crate::continuum_weapon::continuum_weapon_contact_overlaps_ship(
            position.x, position.y, target_x, target_y, 14.0,
        );
        let offset = (position.x - target_x)
            .abs()
            .max((position.y - target_y).abs());
        let mut prox_explosion = false;
        if let Some((closest, deadline)) = sensor {
            prox_explosion = offset > closest || step >= deadline;
            sensor = Some((offset, deadline));
        } else if bomb & 0x1f == 4
            && crate::continuum_weapon::continuum_proximity_sensor_overlaps_ship(
                position.x,
                position.y,
                target_x,
                target_y,
                crate::continuum_weapon::continuum_weapon_proximity_trigger_radius(
                    crate::arena_settings::arena_proximity_distance(),
                    bomb,
                ),
                14.0,
            )
        {
            sensor = Some((
                offset,
                step + crate::arena_settings::arena_bomb_explode_delay_ticks(),
            ));
        }
        if wall_explosion || direct || prox_explosion {
            let self_x = bot.x + launch_velocity.x * seconds;
            let self_y = bot.y + launch_velocity.y * seconds;
            let self_safe = (position.x - self_x).hypot(position.y - self_y) > blast + hull + 32.0;
            let useful =
                (position.x - target_x).hypot(position.y - target_y) < (blast - 16.0).max(0.0);
            return if self_safe && useful { Some(bomb) } else { gun };
        }
    }
    gun
}
