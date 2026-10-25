use super::*;

pub(crate) fn gun_cooldowns_after_fire(
    current_bomb_cooldown: f32,
    gun_cooldown: f32,
) -> (f32, f32) {
    (gun_cooldown, current_bomb_cooldown.max(gun_cooldown))
}

pub(crate) fn bomb_cooldowns_after_fire(
    current_bullet_cooldown: f32,
    bomb_cooldown: f32,
    emp_bomb: bool,
) -> (f32, f32) {
    let bullet_cooldown = if emp_bomb {
        current_bullet_cooldown
    } else {
        current_bullet_cooldown.max(bomb_cooldown)
    };
    (bullet_cooldown, bomb_cooldown)
}

pub(crate) fn weapon_energy_available(energy: f32, cost: f32) -> bool {
    crate::gameplay_rules::can_fire(energy, cost)
}

pub(crate) fn special_weapon_energy_available(in_safe: bool, energy: f32, cost: f32) -> bool {
    in_safe || weapon_energy_available(energy, cost.max(0.0))
}

pub(crate) fn special_weapon_energy_after_fire(current_energy: f32) -> f32 {
    current_energy
}

pub(crate) fn repel_weapon_available(
    repel_cooldown: f32,
    in_safe: bool,
    energy: f32,
    cost: f32,
    inventory: u32,
) -> bool {
    repel_cooldown <= 0.0 && special_weapon_energy_available(in_safe, energy, cost) && inventory > 0
}

#[allow(dead_code)]
pub(crate) fn ship_togglables(ship: &Ship, safe: bool) -> u8 {
    let mut togglables = 0;
    if ship.stealth {
        togglables |= STATUS_STEALTH;
    }
    if ship.cloak {
        togglables |= STATUS_CLOAK;
    }
    if ship.xradar {
        togglables |= STATUS_XRADAR;
    }
    if ship.antiwarp {
        togglables |= STATUS_ANTIWARP;
    }
    if ship.flash {
        togglables |= STATUS_FLASH;
    }
    if safe {
        togglables |= STATUS_SAFETY;
    }
    togglables
}

#[allow(dead_code)]
pub(crate) fn cloak_toggle_sets_flash(was_cloaked: bool, now_cloaked: bool) -> bool {
    was_cloaked && !now_cloaked
}

#[allow(dead_code)]
pub(crate) fn weapon_use_reveals_cloak(in_safe: bool, cloaked: bool) -> bool {
    !in_safe && cloaked
}

pub(crate) fn status_toggle_uses_off_sound(was_enabled: bool, now_enabled: bool) -> bool {
    was_enabled && !now_enabled
}

pub(crate) fn timed_indicator_seconds_label(seconds: f32) -> String {
    format!("{:.1}", seconds.max(0.0))
}

pub(crate) fn timed_indicator_centiseconds_label(centiseconds: u16) -> String {
    timed_indicator_seconds_label(centiseconds as f32 / 100.0)
}

pub(crate) fn timed_indicator_percent_label(remaining: f32, total: f32) -> String {
    let percent = if total <= 0.0 {
        0.0
    } else {
        (remaining / total).clamp(0.0, 1.0) * 100.0
    };
    format!("{}%", percent as u32)
}
