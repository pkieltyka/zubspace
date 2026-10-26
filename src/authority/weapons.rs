use super::EnergyPool;
use crate::continuum_weapon::{
    continuum_weapon_alternate, continuum_weapon_level, continuum_weapon_shrapnel_count,
    continuum_weapon_shrapnel_level, continuum_weapon_type, continuum_weapon_with_limits,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WeaponInventory {
    pub(crate) gun_level: u8,
    pub(crate) bomb_level: u8,
    pub(crate) shrapnel: u8,
    pub(crate) bouncing_bullets: bool,
    pub(crate) multifire_capable: bool,
    pub(crate) proximity_bombs: bool,
    pub(crate) repels: u8,
    pub(crate) bursts: u8,
    pub(crate) decoys: u8,
    pub(crate) thors: u8,
    pub(crate) bricks: u8,
    pub(crate) rockets: u8,
    pub(crate) portals: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WeaponInventoryTuning {
    pub(crate) initial_gun_level: u8,
    pub(crate) max_gun_level: u8,
    pub(crate) initial_bomb_level: u8,
    pub(crate) max_bomb_level: u8,
    pub(crate) shrapnel_rate: u8,
    pub(crate) max_shrapnel: u8,
    pub(crate) initial_repels: u8,
    pub(crate) max_repels: u8,
    pub(crate) initial_bursts: u8,
    pub(crate) max_bursts: u8,
    pub(crate) initial_decoys: u8,
    pub(crate) max_decoys: u8,
    pub(crate) initial_thors: u8,
    pub(crate) max_thors: u8,
    pub(crate) initial_bricks: u8,
    pub(crate) max_bricks: u8,
    pub(crate) initial_rockets: u8,
    pub(crate) max_rockets: u8,
    pub(crate) initial_portals: u8,
    pub(crate) max_portals: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WeaponFireState {
    pub(crate) energy: EnergyPool,
    pub(crate) weapons: WeaponInventory,
    pub(crate) last_fire_tick: Option<u32>,
    pub(crate) last_gun_fire_tick: Option<u32>,
    pub(crate) last_bomb_fire_tick: Option<u32>,
    pub(crate) last_special_weapon_request: Option<u8>,
    pub(crate) schedule: WeaponCooldownSchedule,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WeaponFireRequest {
    pub(crate) weapon_data: u16,
    pub(crate) server_tick: u32,
    pub(crate) in_safe_tile: bool,
    pub(crate) blocked_by_arena_rule: bool,
    pub(crate) energy_cost: f32,
    pub(crate) fire_delay_ticks: u32,
    pub(crate) cross_fire_delay_ticks: u32,
    /// The shooter's ship has EmpBomb: its bombs and mines leave guns and
    /// Repel unlocked, as in the reference game.
    pub(crate) emp_bomb: bool,
}

/// The reference game's `kRepelDelayTicks`: Repel waits this long after the
/// last Repel, Burst, Thor, Decoy or non-EMP bomb/mine, and nothing else.
pub(crate) const REPEL_DELAY_TICKS: u32 = 50;

/// How far before its wall-clock deadline a weapon may be received when its
/// exact 60 Hz input spacing proves the cooldown elapsed: the same bound the
/// movement clock already lets logical input run ahead of receipt time
/// (`MAX_GAMEPLAY_CATCH_UP_STEPS`). It absorbs network jitter, deferred
/// admission and the 60 Hz server loop. The earliness is carried into the
/// class's next deadline (`WeaponCooldownSchedule`), so a stream of shots
/// can never fire faster than its authored delays.
pub(crate) const PROVEN_COOLDOWN_ALLOWANCE_TICKS: u32 =
    (crate::gameplay_rules::MAX_GAMEPLAY_CATCH_UP_STEPS as u32 * 5).div_ceil(3);

/// Logical-input proof that the cooldown owned by the last accepted weapon
/// has elapsed, proved independently for each deadline. It never clears the
/// other class's outstanding deadline (notably bombs followed by guns on EMP
/// ships).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WeaponCooldownProof {
    pub(crate) gun: bool,
    pub(crate) bomb: bool,
    pub(crate) repel: bool,
}

/// Deadline bookkeeping the per-class receipt ticks alone cannot carry: how
/// many ticks before its wall-clock deadline each class's latest weapon was
/// received, which later deadlines repay, and the reference game's separate
/// Repel deadline (`next_repel_tick`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WeaponCooldownSchedule {
    pub(crate) gun_early_ticks: u32,
    pub(crate) bomb_early_ticks: u32,
    pub(crate) repel_lock_tick: Option<u32>,
    pub(crate) repel_early_ticks: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WeaponFireOutcome {
    pub(crate) accepted_weapon_data: u16,
    pub(crate) state: WeaponFireState,
    pub(crate) reject_reason: Option<WeaponFireRejectReason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WeaponFireRejectReason {
    BlockedByArenaRule,
    RepeatedSpecial,
    NotInInventory,
    ZeroWeapon,
    SafeTile,
    InsufficientEnergy,
    Cooldown,
}

impl WeaponFireRejectReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BlockedByArenaRule => "blocked_by_arena_rule",
            Self::RepeatedSpecial => "repeated_special",
            Self::NotInInventory => "not_in_inventory",
            Self::ZeroWeapon => "zero_weapon",
            Self::SafeTile => "safe_tile",
            Self::InsufficientEnergy => "insufficient_energy",
            Self::Cooldown => "cooldown",
        }
    }
}

pub(crate) fn accept_weapon_fire(
    state: WeaponFireState,
    request: WeaponFireRequest,
) -> WeaponFireOutcome {
    let gun_level = state.weapons.gun_level;
    let bomb_level = state.weapons.bomb_level;
    accept_weapon_fire_with_levels(state, request, gun_level, bomb_level)
}

pub(crate) fn accept_weapon_fire_with_levels(
    state: WeaponFireState,
    request: WeaponFireRequest,
    effective_gun_level: u8,
    effective_bomb_level: u8,
) -> WeaponFireOutcome {
    let charged_energy_cost = request.energy_cost;
    accept_weapon_fire_with_levels_and_charge(
        state,
        request,
        effective_gun_level,
        effective_bomb_level,
        charged_energy_cost,
    )
}

pub(crate) fn accept_weapon_fire_with_levels_and_charge(
    state: WeaponFireState,
    request: WeaponFireRequest,
    effective_gun_level: u8,
    effective_bomb_level: u8,
    charged_energy_cost: f32,
) -> WeaponFireOutcome {
    accept_weapon_fire_with_levels_and_charge_and_logical_cooldown(
        state,
        request,
        effective_gun_level,
        effective_bomb_level,
        charged_energy_cost,
        WeaponCooldownProof::default(),
    )
}

pub(crate) fn accept_weapon_fire_with_levels_and_charge_and_logical_cooldown(
    mut state: WeaponFireState,
    request: WeaponFireRequest,
    effective_gun_level: u8,
    effective_bomb_level: u8,
    charged_energy_cost: f32,
    logical_cooldown: WeaponCooldownProof,
) -> WeaponFireOutcome {
    let requested_weapon_type = continuum_weapon_type(request.weapon_data);
    if request.blocked_by_arena_rule {
        return WeaponFireOutcome {
            accepted_weapon_data: 0,
            state,
            reject_reason: Some(WeaponFireRejectReason::BlockedByArenaRule),
        };
    }
    if matches!(requested_weapon_type, 5..=8) {
        if state.last_special_weapon_request == Some(requested_weapon_type) {
            return WeaponFireOutcome {
                accepted_weapon_data: 0,
                state,
                reject_reason: Some(WeaponFireRejectReason::RepeatedSpecial),
            };
        }
        state.last_special_weapon_request = Some(requested_weapon_type);
    } else {
        state.last_special_weapon_request = None;
    }

    let Some(weapon_data) = state.weapons.accepted_weapon_data_with_levels(
        request.weapon_data,
        effective_gun_level,
        effective_bomb_level,
    ) else {
        return WeaponFireOutcome {
            accepted_weapon_data: 0,
            state,
            reject_reason: Some(WeaponFireRejectReason::NotInInventory),
        };
    };
    let weapon_type = continuum_weapon_type(weapon_data);
    if weapon_type == 0 {
        return WeaponFireOutcome {
            accepted_weapon_data: 0,
            state,
            reject_reason: Some(WeaponFireRejectReason::ZeroWeapon),
        };
    }
    if request.in_safe_tile {
        return WeaponFireOutcome {
            accepted_weapon_data: 0,
            state,
            reject_reason: Some(WeaponFireRejectReason::SafeTile),
        };
    }
    if !state.energy.can_spend_weapon_energy(request.energy_cost) {
        return WeaponFireOutcome {
            accepted_weapon_data: 0,
            state,
            reject_reason: Some(WeaponFireRejectReason::InsufficientEnergy),
        };
    }
    let Some(early_ticks) =
        weapon_fire_delay_early_ticks(state, weapon_type, request, logical_cooldown)
    else {
        return WeaponFireOutcome {
            accepted_weapon_data: 0,
            state,
            reject_reason: Some(WeaponFireRejectReason::Cooldown),
        };
    };

    state.energy.spend_weapon_energy(charged_energy_cost);
    state.last_fire_tick = Some(request.server_tick);
    match weapon_cooldown_class(weapon_type) {
        WeaponCooldownClass::Gun => {
            state.last_gun_fire_tick = Some(request.server_tick);
            state.schedule.gun_early_ticks = early_ticks;
        }
        // The reference game's Repel sets the bomb and gun deadlines like any
        // other bomb-class weapon; only its own gate differs.
        WeaponCooldownClass::Bomb | WeaponCooldownClass::Repel => {
            state.last_bomb_fire_tick = Some(request.server_tick);
            state.schedule.bomb_early_ticks = early_ticks;
        }
        WeaponCooldownClass::None => {}
    }
    if weapon_sets_repel_lock(weapon_type, request.emp_bomb) {
        state.schedule.repel_lock_tick = Some(request.server_tick);
        state.schedule.repel_early_ticks = early_ticks;
    }
    state.weapons.spend_consumable(weapon_type);
    WeaponFireOutcome {
        accepted_weapon_data: weapon_data,
        state,
        reject_reason: None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WeaponCooldownClass {
    None,
    Gun,
    Bomb,
    Repel,
}

fn weapon_cooldown_class(weapon_type: u8) -> WeaponCooldownClass {
    match weapon_type {
        1 | 2 | 15 => WeaponCooldownClass::Gun,
        5 => WeaponCooldownClass::Repel,
        3..=8 => WeaponCooldownClass::Bomb,
        _ => WeaponCooldownClass::None,
    }
}

/// Weapons that set the reference game's `next_repel_tick`. Guns, bricks and
/// rockets never do, and neither do bombs or mines on an EmpBomb ship.
fn weapon_sets_repel_lock(weapon_type: u8, emp_bomb: bool) -> bool {
    match weapon_type {
        3 | 4 => !emp_bomb,
        5..=8 => true,
        _ => false,
    }
}

/// Whether every deadline governing `weapon_type` allows it now, and if so
/// how many ticks before the latest of them it was received.
fn weapon_fire_delay_early_ticks(
    state: WeaponFireState,
    weapon_type: u8,
    request: WeaponFireRequest,
    logical_cooldown: WeaponCooldownProof,
) -> Option<u32> {
    // A class whose reference lock is zero (guns after an EMP bomb) still
    // cannot move the accepted weapon timeline backwards.
    if state
        .last_fire_tick
        .is_some_and(|last| crate::continuum::continuum_tick_diff(request.server_tick, last) < 0)
    {
        return None;
    }
    let schedule = state.schedule;
    let deadline = |anchor, anchor_early, delay, proven| {
        cooldown_early_ticks(anchor, anchor_early, request.server_tick, delay, proven)
    };
    match weapon_cooldown_class(weapon_type) {
        WeaponCooldownClass::Gun => Some(
            deadline(
                state.last_gun_fire_tick,
                schedule.gun_early_ticks,
                request.fire_delay_ticks,
                logical_cooldown.gun,
            )?
            .max(deadline(
                state.last_bomb_fire_tick,
                schedule.bomb_early_ticks,
                request.cross_fire_delay_ticks,
                logical_cooldown.bomb,
            )?),
        ),
        WeaponCooldownClass::Bomb => Some(
            deadline(
                state.last_bomb_fire_tick,
                schedule.bomb_early_ticks,
                request.fire_delay_ticks,
                logical_cooldown.bomb,
            )?
            .max(deadline(
                state.last_gun_fire_tick,
                schedule.gun_early_ticks,
                request.cross_fire_delay_ticks,
                logical_cooldown.gun,
            )?),
        ),
        // The reference game gates Repel only on `next_repel_tick`, never on
        // the gun or bomb deadlines.
        WeaponCooldownClass::Repel => deadline(
            schedule.repel_lock_tick,
            schedule.repel_early_ticks,
            REPEL_DELAY_TICKS,
            logical_cooldown.repel,
        ),
        WeaponCooldownClass::None => Some(0),
    }
}

/// `None` while the deadline `anchor + anchor_early + delay` is still ahead;
/// otherwise how many ticks before it the weapon was received. Receipt time
/// stays authoritative: only a logically proven weapon may be early, by at
/// most `PROVEN_COOLDOWN_ALLOWANCE_TICKS` including the anchor's own carried
/// earliness, so the allowance never compounds across a stream of shots.
fn cooldown_early_ticks(
    anchor: Option<u32>,
    anchor_early: u32,
    server_tick: u32,
    delay_ticks: u32,
    logical_cooldown_ready: bool,
) -> Option<u32> {
    let Some(anchor) = anchor.filter(|_| delay_ticks > 0) else {
        return Some(0);
    };
    let elapsed = i64::from(crate::continuum::continuum_tick_diff(server_tick, anchor));
    let early = i64::from(delay_ticks) + i64::from(anchor_early) - elapsed;
    if early <= 0 {
        Some(0)
    } else if logical_cooldown_ready && early <= i64::from(PROVEN_COOLDOWN_ALLOWANCE_TICKS) {
        Some(early as u32)
    } else {
        None
    }
}

impl WeaponInventory {
    pub(crate) fn cleared() -> Self {
        Self::default()
    }

    pub(crate) fn initialized(tuning: WeaponInventoryTuning) -> Self {
        Self {
            gun_level: tuning.initial_gun_level.min(tuning.max_gun_level),
            bomb_level: tuning.initial_bomb_level.min(tuning.max_bomb_level),
            shrapnel: 0,
            // The reference game's ResetShip clears capabilities before initial prizes.
            bouncing_bullets: false,
            multifire_capable: false,
            proximity_bombs: false,
            repels: tuning.initial_repels.min(tuning.max_repels),
            bursts: tuning.initial_bursts.min(tuning.max_bursts),
            decoys: tuning.initial_decoys.min(tuning.max_decoys),
            thors: tuning.initial_thors.min(tuning.max_thors),
            bricks: tuning.initial_bricks.min(tuning.max_bricks),
            rockets: tuning.initial_rockets.min(tuning.max_rockets),
            portals: tuning.initial_portals.min(tuning.max_portals),
        }
    }

    /// zubspace's full starting loadout, independent of InitialBounty rolls.
    /// Temporary Super/Shield effects and active toggles are not granted here.
    pub(crate) fn maximized(tuning: WeaponInventoryTuning) -> Self {
        Self {
            gun_level: tuning.max_gun_level,
            bomb_level: tuning.max_bomb_level,
            shrapnel: tuning.max_shrapnel,
            bouncing_bullets: tuning.max_gun_level > 0,
            multifire_capable: tuning.max_gun_level > 0,
            proximity_bombs: tuning.max_bomb_level > 0,
            repels: tuning.max_repels,
            bursts: tuning.max_bursts,
            decoys: tuning.max_decoys,
            thors: tuning.max_thors,
            bricks: tuning.max_bricks,
            rockets: tuning.max_rockets,
            portals: tuning.max_portals,
        }
    }

    pub(crate) fn apply_prize_effect(&mut self, tuning: WeaponInventoryTuning, prize_id: i16) {
        let negative = prize_id < 0;
        match prize_id.checked_abs() {
            Some(8) => {
                self.gun_level = if negative {
                    self.gun_level
                        .saturating_sub(1)
                        .max(tuning.initial_gun_level)
                } else {
                    self.gun_level.saturating_add(1).min(tuning.max_gun_level)
                };
            }
            Some(9) => {
                self.bomb_level = if negative {
                    self.bomb_level
                        .saturating_sub(1)
                        .max(tuning.initial_bomb_level)
                } else {
                    self.bomb_level.saturating_add(1).min(tuning.max_bomb_level)
                };
            }
            Some(10) => self.bouncing_bullets = !negative,
            Some(15) => self.multifire_capable = !negative,
            Some(16) => self.proximity_bombs = !negative,
            Some(19) => {
                self.shrapnel = crate::gameplay_rules::shrapnel_prize_amount_after(
                    self.shrapnel,
                    tuning.shrapnel_rate,
                    tuning.max_shrapnel,
                    negative,
                );
            }
            Some(21) => {
                self.repels = adjusted_consumable_count(self.repels, tuning.max_repels, negative);
            }
            Some(22) => {
                self.bursts = adjusted_consumable_count(self.bursts, tuning.max_bursts, negative);
            }
            Some(23) => {
                self.decoys = adjusted_consumable_count(self.decoys, tuning.max_decoys, negative);
            }
            Some(24) => {
                self.thors = adjusted_consumable_count(self.thors, tuning.max_thors, negative);
            }
            Some(26) => {
                self.bricks = adjusted_consumable_count(self.bricks, tuning.max_bricks, negative);
            }
            Some(27) => {
                self.rockets =
                    adjusted_consumable_count(self.rockets, tuning.max_rockets, negative);
            }
            Some(28) => {
                self.portals =
                    adjusted_consumable_count(self.portals, tuning.max_portals, negative);
            }
            _ => {}
        }
    }

    pub(crate) fn accepted_weapon_data(self, weapon_data: u16) -> Option<u16> {
        self.accepted_weapon_data_with_levels(weapon_data, self.gun_level, self.bomb_level)
    }

    pub(crate) fn accepted_weapon_data_with_levels(
        self,
        weapon_data: u16,
        effective_gun_level: u8,
        effective_bomb_level: u8,
    ) -> Option<u16> {
        let weapon_type = continuum_weapon_type(weapon_data);
        let weapon_level = continuum_weapon_level(weapon_data);
        let shrapnel_level = continuum_weapon_shrapnel_level(weapon_data);
        let shrapnel_count = continuum_weapon_shrapnel_count(weapon_data);
        match weapon_type {
            0 => Some(0),
            1 if (!continuum_weapon_alternate(weapon_data) || self.multifire_capable)
                && weapon_level <= effective_gun_level =>
            {
                Some(self.weapon_data_clamped_to_inventory(
                    weapon_data,
                    weapon_level,
                    shrapnel_level,
                    shrapnel_count,
                ))
            }
            2 if self.bouncing_bullets
                && (!continuum_weapon_alternate(weapon_data) || self.multifire_capable)
                && weapon_level <= effective_gun_level =>
            {
                Some(self.weapon_data_clamped_to_inventory(
                    weapon_data,
                    weapon_level,
                    shrapnel_level,
                    shrapnel_count,
                ))
            }
            3 if weapon_level <= effective_bomb_level => {
                Some(self.weapon_data_clamped_to_inventory(
                    weapon_data,
                    weapon_level,
                    shrapnel_level,
                    shrapnel_count,
                ))
            }
            4 if self.proximity_bombs && weapon_level <= effective_bomb_level => {
                Some(self.weapon_data_clamped_to_inventory(
                    weapon_data,
                    weapon_level,
                    shrapnel_level,
                    shrapnel_count,
                ))
            }
            5 if self.repels > 0 => Some(weapon_data),
            6 if self.decoys > 0 => Some(weapon_data),
            7 if self.bursts > 0 => Some(weapon_data),
            // The reference game clears the weapon before Thor input sets only type.
            // Bomb upgrades do not upgrade Thor or grant it fragments. Keep
            // arbitrary wire levels decodable, but never grant them at our
            // authoritative admission boundary, even from a custom client.
            8 if self.thors > 0 => Some(8),
            15 if weapon_level <= effective_gun_level => {
                Some(self.weapon_data_clamped_to_inventory(
                    weapon_data,
                    weapon_level,
                    shrapnel_level,
                    shrapnel_count,
                ))
            }
            _ => None,
        }
    }

    pub(crate) fn damage_weapon_allowed(self, weapon_data: u16) -> bool {
        self.accepted_weapon_data(weapon_data).is_some()
    }

    pub(crate) fn spend_consumable(&mut self, weapon_type: u8) {
        match weapon_type {
            5 => self.repels = self.repels.saturating_sub(1),
            6 => self.decoys = self.decoys.saturating_sub(1),
            7 => self.bursts = self.bursts.saturating_sub(1),
            8 => self.thors = self.thors.saturating_sub(1),
            _ => {}
        }
    }

    pub(crate) fn can_spend_brick(self) -> bool {
        self.bricks > 0
    }

    pub(crate) fn spend_brick(&mut self) {
        self.bricks = self.bricks.saturating_sub(1);
    }

    fn weapon_data_clamped_to_inventory(
        self,
        weapon_data: u16,
        weapon_level: u8,
        shrapnel_level: u8,
        shrapnel_count: u8,
    ) -> u16 {
        // Intentional project override: an accepted explosive's fragments use
        // that parent's level, never a client-selected power or its gun level.
        let shrapnel_level = if matches!(continuum_weapon_type(weapon_data), 3 | 4 | 8) {
            weapon_level
        } else {
            shrapnel_level.min(self.gun_level)
        };
        continuum_weapon_with_limits(
            weapon_data,
            weapon_level,
            shrapnel_level,
            shrapnel_count.min(self.shrapnel),
        )
    }
}

fn adjusted_consumable_count(current: u8, max: u8, negative: bool) -> u8 {
    if negative {
        current.saturating_sub(1)
    } else {
        current.saturating_add(1).min(max)
    }
}
