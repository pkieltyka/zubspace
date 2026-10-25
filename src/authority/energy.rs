#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EnergyPool {
    current: u16,
    max: u16,
    recharge_remainder: f32,
}

impl EnergyPool {
    pub(crate) fn new(current: u16, max: u16, recharge_remainder: f32) -> Self {
        Self {
            // A negative Energy prize lowers capacity without touching the
            // live pool. Preserve that authoritative transient until an
            // ordinary spend or recharge step changes it.
            current,
            max,
            recharge_remainder: if current >= max {
                0.0
            } else {
                recharge_remainder.max(0.0)
            },
        }
    }

    pub(crate) fn current(self) -> u16 {
        self.current
    }

    pub(crate) fn recharge_remainder(self) -> f32 {
        self.recharge_remainder
    }

    pub(crate) fn available(self) -> f32 {
        self.current as f32 + self.recharge_remainder
    }

    pub(crate) fn can_spend_weapon_energy(self, cost: f32) -> bool {
        crate::gameplay_rules::can_fire(self.available(), cost)
    }

    pub(crate) fn spend_weapon_energy(&mut self, cost: f32) {
        self.set_available((self.available() - cost.max(0.0)).max(0.0));
    }

    pub(crate) fn spend_continuous_energy(&mut self, cost: f32) -> bool {
        let cost = cost.max(0.0);
        let available = self.available();
        if available <= cost {
            return false;
        }
        let remaining = available - cost;
        self.set_available(remaining);
        true
    }

    pub(crate) fn apply_damage(&mut self, damage: u16) {
        self.current = self.current.saturating_sub(damage);
        self.recharge_remainder = 0.0;
    }

    /// Retain an exact fraction of the complete authoritative pool. This is
    /// used by deterministic world effects such as wormholes; keeping the
    /// fractional remainder prevents the server from introducing an extra
    /// whole-energy rounding error relative to browser prediction.
    pub(crate) fn retain_fraction(&mut self, fraction: f32, minimum: f32) {
        self.set_available((self.available() * fraction.clamp(0.0, 1.0)).max(minimum.max(0.0)));
    }

    pub(crate) fn recharge_for_position_ticks(
        &mut self,
        elapsed_ticks: u32,
        recharge_per_second: f32,
    ) {
        if self.current >= self.max {
            self.current = self.max;
            self.recharge_remainder = 0.0;
            return;
        }

        let recharge = recharge_per_second * crate::continuum::ticks_to_seconds_u32(elapsed_ticks)
            + self.recharge_remainder;
        let whole_recharge = recharge.floor().max(0.0) as u16;
        self.recharge_remainder = recharge - whole_recharge as f32;
        self.current = self.current.saturating_add(whole_recharge).min(self.max);
        if self.current >= self.max {
            self.recharge_remainder = 0.0;
        }
    }

    fn set_available(&mut self, available: f32) {
        let available = available.clamp(0.0, u16::MAX as f32);
        self.current = available.floor() as u16;
        self.recharge_remainder = if self.current >= self.max {
            0.0
        } else {
            available - self.current as f32
        };
    }
}
