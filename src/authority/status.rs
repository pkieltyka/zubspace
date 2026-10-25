use super::EnergyPool;

pub(crate) const POSITION_STATUS_STEALTH: u8 = 1 << 0;
pub(crate) const POSITION_STATUS_CLOAK: u8 = 1 << 1;
pub(crate) const POSITION_STATUS_XRADAR: u8 = 1 << 2;
pub(crate) const POSITION_STATUS_ANTIWARP: u8 = 1 << 3;
pub(crate) const POSITION_STATUS_FLASH: u8 = 1 << 4;
pub(crate) const POSITION_STATUS_SAFETY: u8 = 1 << 5;
pub(crate) const POSITION_STATUS_RELAY_MASK: u8 = POSITION_STATUS_STEALTH
    | POSITION_STATUS_CLOAK
    | POSITION_STATUS_XRADAR
    | POSITION_STATUS_ANTIWARP
    | POSITION_STATUS_FLASH
    | POSITION_STATUS_SAFETY;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PositionStatusBits {
    bits: u8,
}

impl PositionStatusBits {
    pub(crate) fn from_reported_position(reported: u8, in_safe_tile: bool) -> Self {
        let mut bits = reported & POSITION_STATUS_RELAY_MASK;
        if in_safe_tile {
            bits |= POSITION_STATUS_SAFETY;
        } else {
            bits &= !POSITION_STATUS_SAFETY;
        }
        Self { bits }
    }

    pub(crate) fn bits(self) -> u8 {
        self.bits
    }

    fn strip_unowned_statuses(mut self, inventory: StatusInventory) -> Self {
        if !inventory.stealth_capable {
            self.bits &= !POSITION_STATUS_STEALTH;
        }
        if !inventory.cloak_capable {
            self.bits &= !POSITION_STATUS_CLOAK;
        }
        if !inventory.xradar_capable {
            self.bits &= !POSITION_STATUS_XRADAR;
        }
        if !inventory.antiwarp_capable {
            self.bits &= !POSITION_STATUS_ANTIWARP;
        }
        self
    }

    pub(crate) fn drain_energy_for_statuses(
        mut self,
        energy: &mut EnergyPool,
        costs: StatusEnergyCosts,
        elapsed_ticks: u32,
    ) -> Self {
        drain_status_bit(
            &mut self.bits,
            POSITION_STATUS_XRADAR,
            energy,
            costs.xradar_per_second,
            elapsed_ticks,
        );
        drain_status_bit(
            &mut self.bits,
            POSITION_STATUS_STEALTH,
            energy,
            costs.stealth_per_second,
            elapsed_ticks,
        );
        drain_status_bit(
            &mut self.bits,
            POSITION_STATUS_CLOAK,
            energy,
            costs.cloak_per_second,
            elapsed_ticks,
        );
        drain_status_bit(
            &mut self.bits,
            POSITION_STATUS_ANTIWARP,
            energy,
            costs.antiwarp_per_second,
            elapsed_ticks,
        );
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StatusInventory {
    pub(crate) stealth_capable: bool,
    pub(crate) cloak_capable: bool,
    pub(crate) xradar_capable: bool,
    pub(crate) antiwarp_capable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StatusInventoryTuning {
    pub(crate) stealth_available: bool,
    pub(crate) initial_stealth: bool,
    pub(crate) cloak_available: bool,
    pub(crate) initial_cloak: bool,
    pub(crate) xradar_available: bool,
    pub(crate) initial_xradar: bool,
    pub(crate) antiwarp_available: bool,
    pub(crate) initial_antiwarp: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct StatusEnergyCosts {
    pub(crate) stealth_per_second: f32,
    pub(crate) cloak_per_second: f32,
    pub(crate) xradar_per_second: f32,
    pub(crate) antiwarp_per_second: f32,
}

impl StatusInventory {
    pub(crate) fn cleared() -> Self {
        Self::default()
    }

    pub(crate) fn initialized(tuning: StatusInventoryTuning) -> Self {
        Self {
            stealth_capable: tuning.initial_stealth,
            cloak_capable: tuning.initial_cloak,
            xradar_capable: tuning.initial_xradar,
            antiwarp_capable: tuning.initial_antiwarp,
        }
    }

    pub(crate) fn maximized(tuning: StatusInventoryTuning) -> Self {
        Self {
            stealth_capable: tuning.stealth_available,
            cloak_capable: tuning.cloak_available,
            xradar_capable: tuning.xradar_available,
            antiwarp_capable: tuning.antiwarp_available,
        }
    }

    pub(crate) fn apply_prize_effect(&mut self, tuning: StatusInventoryTuning, prize_id: i16) {
        let negative = prize_id < 0;
        match prize_id.checked_abs() {
            Some(4) => apply_status_capability(
                &mut self.stealth_capable,
                tuning.stealth_available,
                negative,
            ),
            Some(5) => {
                apply_status_capability(&mut self.cloak_capable, tuning.cloak_available, negative)
            }
            Some(6) => {
                apply_status_capability(&mut self.xradar_capable, tuning.xradar_available, negative)
            }
            Some(20) => apply_status_capability(
                &mut self.antiwarp_capable,
                tuning.antiwarp_available,
                negative,
            ),
            _ => {}
        }
    }

    pub(crate) fn position_status_bits(
        self,
        reported: u8,
        in_safe_tile: bool,
    ) -> PositionStatusBits {
        PositionStatusBits::from_reported_position(reported, in_safe_tile)
            .strip_unowned_statuses(self)
    }
}

fn drain_status_bit(
    bits: &mut u8,
    flag: u8,
    energy: &mut EnergyPool,
    cost_per_second: f32,
    elapsed_ticks: u32,
) {
    if *bits & flag == 0 {
        return;
    }

    let cost = cost_per_second.max(0.0) * crate::continuum::ticks_to_seconds_u32(elapsed_ticks);
    if !energy.spend_continuous_energy(cost) {
        *bits &= !flag;
    }
}

fn apply_status_capability(capable: &mut bool, available: bool, negative: bool) {
    if available || (negative && *capable) {
        *capable = !negative;
    }
}
