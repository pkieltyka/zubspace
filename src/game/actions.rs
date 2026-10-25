use super::*;

mod bricks;
mod effects;
mod lifecycle;
mod movement;
mod prizes;
mod status;
mod warp;
mod weapons;

pub(in crate::game) use lifecycle::{
    local_zone_tuning, merge_local_zone_lvz, merge_local_zone_lvz_sounds,
    zone_settings_path_for_map,
};

fn special_inventory_after_use(current: u32, in_safe: bool) -> u32 {
    if in_safe {
        current
    } else {
        current.saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_zone_special_weapons_do_not_spend_inventory() {
        assert_eq!(special_inventory_after_use(3, true), 3);
        assert_eq!(special_inventory_after_use(3, false), 2);
        assert_eq!(special_inventory_after_use(0, false), 0);
    }
}
