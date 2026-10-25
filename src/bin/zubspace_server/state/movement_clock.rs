//! Cumulative movement time belongs to the server, independently of packet rate.
//! One quantum is 1/300 second: a 100 Hz tick earns three, a 60 Hz step costs five.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MovementTimeBudget {
    accounted_tick: Option<u32>,
    credit: i32,
}

impl MovementTimeBudget {
    // Bank only the same interval that canonical replay can actually recover.
    const MAX_CREDIT: i32 = super::MAX_RECENT_INPUT_OBSERVATIONS as i32 * 5;
    // A browser may publish one bounded catch-up batch at a scheduling seam.
    // This is cumulative debt, never a fresh allowance for each packet/life.
    const MAX_DEBT: i32 = crate::gameplay_rules::MAX_GAMEPLAY_CATCH_UP_STEPS as i32 * 5;

    /// Prospective accounting. Rejected/duplicate positions must not spend or
    /// replenish the live budget. Preserve this value across soft boundaries.
    pub(crate) fn admit(
        self,
        server_tick: u32,
        previous_accepted_tick: Option<u32>,
        steps: u16,
    ) -> Option<Self> {
        if usize::from(steps) > super::MAX_RECENT_INPUT_OBSERVATIONS {
            return None;
        }
        let anchor = self
            .accounted_tick
            .or(previous_accepted_tick)
            .unwrap_or(server_tick);
        let elapsed = crate::continuum::continuum_tick_diff(server_tick, anchor);
        if elapsed < 0 {
            return None;
        }
        let earned = i64::from(elapsed) * 3;
        let credit = (i64::from(self.credit) + earned).min(i64::from(Self::MAX_CREDIT))
            - i64::from(steps) * 5;
        (credit >= -i64::from(Self::MAX_DEBT)).then_some(Self {
            accounted_tick: Some(server_tick),
            credit: credit as i32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accelerated_packets_exhaust_one_shared_allowance_then_recover_with_real_time() {
        let mut budget = MovementTimeBudget::default().admit(1000, None, 0).unwrap();
        for _ in 0..crate::gameplay_rules::MAX_GAMEPLAY_CATCH_UP_STEPS {
            budget = budget.admit(1000, Some(1000), 1).unwrap();
        }
        assert!(budget.admit(1000, Some(1000), 1).is_none());
        assert!(budget.admit(1001, Some(1000), 1).is_none());
        assert!(budget.admit(1002, Some(1000), 1).is_some());
        assert!(budget.admit(1000, Some(1000), 16).is_none());
    }

    #[test]
    fn ordinary_sixty_hz_and_delayed_history_preserve_exact_long_term_rate() {
        for origin in [1000, crate::continuum::CONTINUUM_TICK_MASK - 50] {
            let mut budget = MovementTimeBudget::default()
                .admit(origin, None, 0)
                .unwrap();
            for step in 1..=6000 {
                let tick = crate::continuum::continuum_tick_add(origin, step * 5 / 3);
                budget = budget.admit(tick, None, 1).unwrap();
            }
            let now = crate::continuum::continuum_tick_add(origin, 10000);
            assert!(
                budget
                    .admit(crate::continuum::continuum_tick_add(now, 100), None, 60)
                    .is_some()
            );
            assert!(
                budget
                    .admit(crate::continuum::continuum_tick_sub(now, 1), None, 1)
                    .is_none()
            );
        }
    }

    #[test]
    fn burst_debt_survives_life_and_correction_boundaries_and_idle_credit_is_bounded() {
        let budget = MovementTimeBudget::default().admit(1000, None, 8).unwrap();
        let boundary = budget.admit(1000, None, 0).unwrap();
        assert!(boundary.admit(1000, Some(1000), 1).is_none());
        let recovered = boundary.admit(1100, None, 0).unwrap();
        assert!(recovered.admit(1100, None, 48).is_some());
        let bank = budget.admit(1000000, None, 0).unwrap();
        assert_eq!(bank.credit, MovementTimeBudget::MAX_CREDIT);
        assert!(bank.admit(1000000, None, 65).is_none());
        assert!(
            MovementTimeBudget::default()
                .admit(1000, Some(1000), 16)
                .is_none()
        );
    }
}
