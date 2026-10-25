pub(crate) fn flag_source_hidden(carrying: bool, respawn_timer: f32, dropped: bool) -> bool {
    carrying || respawn_timer > 0.0 || dropped
}

pub(crate) fn carryable_flag_limit(carry_flags: u8) -> Option<u16> {
    match carry_flags {
        0 => None,
        1 => Some(u16::MAX),
        count => Some(count as u16 - 1),
    }
}

pub(crate) fn can_pickup_carryable_flag(carry_flags: u8, current_flags: u16) -> bool {
    carryable_flag_limit(carry_flags)
        .map(|limit| current_flags < limit)
        .unwrap_or(false)
}

pub(crate) fn flagger_damage_received(damage: f32, multiplier: f32, active: bool) -> f32 {
    if active {
        (damage * multiplier.max(0.0)).trunc()
    } else {
        damage
    }
}

pub(crate) fn flagger_bomb_cooldown(base_cooldown: f32, extra_delay: f32, active: bool) -> f32 {
    if active {
        base_cooldown + extra_delay.max(0.0)
    } else {
        base_cooldown
    }
}
