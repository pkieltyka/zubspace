use super::*;

#[cfg(test)]
pub(crate) fn proximity_bomb_triggered_by_bot(
    bomb_pos: Vec2,
    bots: &[BotShip],
    tuning: ArenaTuning,
    radius: f32,
) -> bool {
    proximity_bomb_trigger_by_bot(None, bomb_pos, bots, tuning, radius).is_some()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProximitySensorTrigger {
    pub(crate) target_id: u16,
    pub(crate) highest_axis_offset: f32,
}

pub(crate) fn proximity_sensor_trigger(
    weapon_pos: Vec2,
    target_id: u16,
    target_pos: Vec2,
) -> ProximitySensorTrigger {
    let delta = weapon_pos - target_pos;
    ProximitySensorTrigger {
        target_id,
        highest_axis_offset: delta.x.abs().max(delta.y.abs()),
    }
}

pub(crate) fn proximity_bomb_trigger_by_bot(
    sensor: Option<ProximitySensorState>,
    bomb_pos: Vec2,
    bots: &[BotShip],
    tuning: ArenaTuning,
    radius: f32,
) -> Option<ProximitySensorTrigger> {
    if let Some(sensor) = sensor {
        let bot = bots
            .iter()
            .find(|bot| bot.id.min(u16::MAX as usize) as u16 == sensor.target_id)?;
        if bot.respawn_timer > 0.0 {
            return None;
        }
        return Some(proximity_sensor_trigger(
            bomb_pos,
            sensor.target_id,
            bot.pos,
        ));
    }

    bots.iter().find_map(|bot| {
        if bot.respawn_timer <= 0.0
            && proximity_sensor_overlaps_ship(
                bomb_pos,
                bot.pos,
                radius,
                tuning.ship(bot.ship_type).radius_pixels,
            )
        {
            Some(proximity_sensor_trigger(
                bomb_pos,
                bot.id.min(u16::MAX as usize) as u16,
                bot.pos,
            ))
        } else {
            None
        }
    })
}

pub(crate) fn proximity_bomb_trigger_by_network_player(
    sensor: Option<ProximitySensorState>,
    bomb_pos: Vec2,
    network_world: &NetworkWorld,
    local_frequency: u16,
    tuning: ArenaTuning,
    radius: f32,
) -> Option<ProximitySensorTrigger> {
    if let Some(sensor) = sensor {
        let player = network_world.players.get(&sensor.target_id)?;
        let target_pos = player.position?;
        if !network_player_can_trigger_local_weapon(player, local_frequency) {
            return None;
        }
        return Some(proximity_sensor_trigger(
            bomb_pos,
            sensor.target_id,
            target_pos,
        ));
    }

    network_world.players.values().find_map(|player| {
        let target_pos = player.position?;
        if !network_player_can_trigger_local_weapon(player, local_frequency) {
            return None;
        }
        if proximity_sensor_overlaps_ship(
            bomb_pos,
            target_pos,
            radius,
            tuning.ship(player.ship as usize).radius_pixels,
        ) {
            Some(proximity_sensor_trigger(
                bomb_pos,
                player.player_id,
                target_pos,
            ))
        } else {
            None
        }
    })
}

pub(crate) fn proximity_sensor_player_trigger(
    sensor: Option<ProximitySensorState>,
    weapon_pos: Vec2,
    target_id: u16,
    target_pos: Vec2,
    target_radius: f32,
    sensor_radius: f32,
) -> Option<ProximitySensorTrigger> {
    if sensor.is_some_and(|sensor| sensor.target_id == target_id)
        || proximity_sensor_overlaps_ship(weapon_pos, target_pos, sensor_radius, target_radius)
    {
        Some(proximity_sensor_trigger(weapon_pos, target_id, target_pos))
    } else {
        None
    }
}

#[cfg(test)]
pub(crate) fn proximity_sensor_contact_radius(sensor_radius: f32, ship_radius: f32) -> f32 {
    (sensor_radius - 14.0 + ship_radius).max(0.0)
}

pub(crate) fn explosive_contact_triggered_by_bot(
    weapon_pos: Vec2,
    bots: &[BotShip],
    tuning: ArenaTuning,
) -> bool {
    bots.iter().any(|bot| {
        bot.respawn_timer <= 0.0
            && explosive_contact_overlaps_ship(
                weapon_pos,
                bot.pos,
                tuning.ship(bot.ship_type).radius_pixels,
            )
    })
}

pub(crate) fn explosive_contact_triggered_by_network_player(
    weapon_pos: Vec2,
    network_world: &NetworkWorld,
    local_frequency: u16,
    tuning: ArenaTuning,
) -> bool {
    network_world.players.values().any(|player| {
        let Some(position) = player.position else {
            return false;
        };
        network_player_can_trigger_local_weapon(player, local_frequency)
            && explosive_contact_overlaps_ship(
                weapon_pos,
                position,
                tuning.ship(player.ship as usize).radius_pixels,
            )
    })
}

pub(crate) fn proximity_bomb_safe_to_fire(
    bomb_safety: bool,
    proximity_enabled: bool,
    ship_pos: Vec2,
    bots: &[BotShip],
    network_world: &NetworkWorld,
    local_frequency: u16,
    _tuning: ArenaTuning,
    radius: f32,
) -> bool {
    !bomb_safety
        || !proximity_enabled
        || (!proximity_bomb_safety_blocked_by_bot(ship_pos, bots, radius)
            && !proximity_bomb_safety_blocked_by_network_player(
                ship_pos,
                network_world,
                local_frequency,
                radius,
            ))
}

fn proximity_bomb_safety_blocked_by_bot(
    ship_pos: Vec2,
    bots: &[BotShip],
    safety_radius: f32,
) -> bool {
    bots.iter().any(|bot| {
        bot.respawn_timer <= 0.0
            && proximity_bomb_safety_blocks_ship(ship_pos, bot.pos, safety_radius)
    })
}

fn proximity_bomb_safety_blocked_by_network_player(
    ship_pos: Vec2,
    network_world: &NetworkWorld,
    local_frequency: u16,
    safety_radius: f32,
) -> bool {
    network_world.players.values().any(|player| {
        let Some(position) = player.position else {
            return false;
        };
        network_player_can_trigger_local_weapon(player, local_frequency)
            && proximity_bomb_safety_blocks_ship(ship_pos, position, safety_radius)
    })
}

pub(crate) fn network_player_can_trigger_local_weapon(
    player: &NetworkPlayer,
    local_frequency: u16,
) -> bool {
    player.ship != SPECTATOR_SHIP && player.frequency != local_frequency
}

pub(crate) fn proximity_bomb_sensor_exploded(
    sensor: &mut Option<ProximitySensorState>,
    trigger: Option<ProximitySensorTrigger>,
    bomb_age: f32,
    dt: f32,
    delay: f32,
) -> bool {
    if sensor.is_none() && trigger.is_none() {
        return false;
    }

    if let Some(state) = sensor.as_mut() {
        let Some(trigger) = trigger.filter(|trigger| trigger.target_id == state.target_id) else {
            return true;
        };
        if trigger.highest_axis_offset > state.highest_axis_offset {
            return true;
        }
        state.highest_axis_offset = trigger.highest_axis_offset;
        state.timer -= dt;
        return state.timer <= 0.0;
    }

    let Some(trigger) = trigger else {
        return false;
    };
    let mut state = ProximitySensorState {
        target_id: trigger.target_id,
        highest_axis_offset: trigger.highest_axis_offset,
        timer: if bomb_age <= dt { 0.0 } else { delay.max(0.0) },
    };
    state.timer -= dt;
    let exploded = state.timer <= 0.0;
    *sensor = Some(state);
    exploded
}
