#![allow(dead_code)]

#[path = "authority/energy.rs"]
mod energy;
#[path = "authority/status.rs"]
mod status;
#[path = "authority/weapons.rs"]
mod weapons;

pub(crate) use energy::EnergyPool;
#[allow(unused_imports)]
pub(crate) use status::POSITION_STATUS_FLASH;
#[allow(unused_imports)]
pub(crate) use status::{
    POSITION_STATUS_ANTIWARP, POSITION_STATUS_CLOAK, POSITION_STATUS_RELAY_MASK,
    POSITION_STATUS_SAFETY, POSITION_STATUS_STEALTH, POSITION_STATUS_XRADAR, PositionStatusBits,
    StatusEnergyCosts, StatusInventory, StatusInventoryTuning,
};
#[allow(unused_imports)]
pub(crate) use weapons::{
    PROVEN_COOLDOWN_ALLOWANCE_TICKS, REPEL_DELAY_TICKS, WeaponCooldownProof,
    WeaponCooldownSchedule, WeaponFireRequest, WeaponFireState, WeaponInventory,
    WeaponInventoryTuning, accept_weapon_fire, accept_weapon_fire_with_levels,
    accept_weapon_fire_with_levels_and_charge,
    accept_weapon_fire_with_levels_and_charge_and_logical_cooldown,
};
#[allow(unused_imports)]
pub(crate) use weapons::{WeaponFireOutcome, WeaponFireRejectReason};

#[cfg(test)]
#[path = "authority/tests.rs"]
mod tests;
