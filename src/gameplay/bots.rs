use super::*;

pub(crate) fn apply_bot_prize(bot: &mut BotShip, kind: PrizeKind, tuning: ShipTuning) {
    bot.bounty = bot.bounty.saturating_add(1);
    match kind {
        PrizeKind::Energy => {
            bot.max_energy = (bot.max_energy + tuning.energy_upgrade_step).min(tuning.energy_max);
            bot.energy = bot.energy.min(bot.max_energy);
        }
        PrizeKind::FullCharge | PrizeKind::Super | PrizeKind::Shield => {
            bot.energy = bot.max_energy;
            if kind == PrizeKind::Super {
                bot.super_timer = bot.super_timer.max(SUPER_LIFETIME);
            } else if kind == PrizeKind::Shield {
                bot.shield_timer = SHIELD_LIFETIME;
            }
        }
        PrizeKind::EngineShutdown => {
            bot.engine_shutdown_timer = ENGINE_SHUTDOWN_LIFETIME;
            bot.vel *= 0.25;
            bot.fire_cooldown = bot.fire_cooldown.max(BOT_FIRE_COOLDOWN);
        }
        _ => {}
    }
}

pub(crate) fn bot_damage_after_timed_protection(bot: &BotShip, damage: f32) -> f32 {
    damage_after_timed_protection(damage, bot.super_timer, bot.shield_timer, SHIELD_LIFETIME)
}

pub(crate) fn spawn_initial_bots(
    map: &ArenaMap,
    player_pos: Vec2,
    tuning: ArenaTuning,
) -> Vec<BotShip> {
    (0..BOT_COUNT)
        .map(|id| {
            let pos = map
                .random_open_position_near(player_pos, 12, 52, 120)
                .unwrap_or_else(|| map.spawn_position());
            let ship_type = (id + 1) % 8;
            let max_energy = bot_spawn_energy(tuning.ship(ship_type));
            BotShip {
                id,
                pos,
                vel: Vec2::ZERO,
                orientation: bot_spawn_orientation(id),
                ship_type,
                energy: max_energy,
                max_energy,
                bounty: 0,
                kills: 0,
                deaths: 0,
                flag_goals: 0,
                ball_goals: 0,
                carrying_flag: false,
                super_timer: 0.0,
                shield_timer: 0.0,
                engine_shutdown_timer: 0.0,
                emp_shutdown_timer: 0.0,
                fire_cooldown: bot_initial_fire_cooldown(
                    tuning.ship(ship_type).bullet_cooldown,
                    id,
                ),
                exhaust_timer: 0.0,
                respawn_timer: 0.0,
                age: 0.0,
            }
        })
        .collect()
}
