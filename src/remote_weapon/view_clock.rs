use crate::continuum::{continuum_tick_add, continuum_tick_diff, continuum_tick_reached};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ViewDecision {
    Hidden,
    History { tick: u32, rate: u32 },
    Current,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Unseen,
    Recovering { tick: u32, last_server_tick: u32 },
    Current,
}

/// Rendering alone starts a fresh carrier at its source actor's issued cursor,
/// then follows recorded forward poses back to the ordinary current clock.
/// Any path that cannot converge before the existing harm floor uses current
/// authority immediately. It cannot add a fairness wait or retain a terminal.
#[derive(Clone, Copy, Debug)]
pub(super) struct WeaponBirthClock {
    birth: u32,
    state: State,
}

impl WeaponBirthClock {
    pub(super) fn new(birth: u32) -> Self {
        Self {
            birth,
            state: State::Unseen,
        }
    }

    pub(super) fn current(&mut self) {
        self.state = State::Current;
    }

    pub(super) fn select(&mut self, now: u32, actor: Option<u32>) -> ViewDecision {
        let deadline = continuum_tick_add(
            self.birth,
            crate::combat_timeline::PROJECTILE_PRESENTATION_MIN_LEAD_TICKS,
        );
        let rate =
            (crate::combat_timeline::OBSERVER_PRESENTATION_RECOVERY_TICKS_PER_SECOND / 100) as u32;
        if self.state == State::Current {
            return ViewDecision::Current;
        }
        let Some(actor) = actor else {
            self.current();
            return ViewDecision::Current;
        };
        if rate <= 1 || continuum_tick_reached(now, deadline) || continuum_tick_diff(actor, now) > 0
        {
            self.current();
            return ViewDecision::Current;
        }
        match self.state {
            State::Unseen => {
                if continuum_tick_diff(now, self.birth) < 0 {
                    return ViewDecision::Hidden;
                }
                let delay = continuum_tick_diff(now, actor).max(0) as u32;
                let wait_for_birth = continuum_tick_diff(self.birth, actor).max(0) as u32;
                let catch_up = delay.div_ceil(rate - 1);
                let finish = continuum_tick_add(now, wait_for_birth.saturating_add(catch_up));
                if continuum_tick_diff(finish, deadline) > 0 {
                    self.current();
                    return ViewDecision::Current;
                }
                if wait_for_birth > 0 {
                    return ViewDecision::Hidden;
                }
                if actor == now {
                    self.current();
                    return ViewDecision::Current;
                }
                self.state = State::Recovering {
                    tick: actor,
                    last_server_tick: now,
                };
                ViewDecision::History {
                    tick: actor,
                    rate: 0,
                }
            }
            State::Recovering {
                tick,
                last_server_tick,
            } => {
                let elapsed = continuum_tick_diff(now, last_server_tick);
                if !(0..=20).contains(&elapsed) {
                    self.current();
                    return ViewDecision::Current;
                }
                let candidate = continuum_tick_add(tick, (elapsed as u32).saturating_mul(rate));
                if continuum_tick_reached(candidate, now) {
                    self.current();
                    return ViewDecision::Current;
                }
                self.state = State::Recovering {
                    tick: candidate,
                    last_server_tick: now,
                };
                ViewDecision::History {
                    tick: candidate,
                    rate,
                }
            }
            State::Current => ViewDecision::Current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_birth_starts_at_the_drawn_source_then_joins_current_before_harm() {
        let mut clock = WeaponBirthClock::new(100);
        for now in 100..107 {
            assert_eq!(clock.select(now, Some(now - 7)), ViewDecision::Hidden);
        }
        assert_eq!(
            clock.select(107, Some(100)),
            ViewDecision::History { tick: 100, rate: 0 }
        );
        for now in 108..114 {
            assert_eq!(
                clock.select(now, Some(now - 7)),
                ViewDecision::History {
                    tick: 100 + (now - 107) * 2,
                    rate: 2
                }
            );
        }
        assert_eq!(clock.select(114, Some(107)), ViewDecision::Current);
        assert_eq!(
            clock.select(115, Some(90)),
            ViewDecision::Current,
            "a completed clock cannot rewind with its actor"
        );
    }

    #[test]
    fn all_supported_warmups_fit_the_existing_server_deadline() {
        for delay in 0..=28_u32 {
            let mut clock = WeaponBirthClock::new(100);
            let mut converged = None;
            for now in 100..=128 {
                let decision = clock.select(now, Some(now - delay));
                if decision == ViewDecision::Current {
                    converged = Some(now);
                    break;
                }
                if let ViewDecision::History { tick, .. } = decision {
                    assert!((100..=now).contains(&tick));
                }
            }
            assert!(converged.is_some_and(|tick| tick <= 128));
            if delay > 14 {
                assert_eq!(
                    converged,
                    Some(100),
                    "high delay cannot add hidden flight before possible harm"
                );
            }
        }
    }

    #[test]
    fn late_source_missing_clock_and_stalls_use_the_current_pose() {
        for (now, actor) in [(125, Some(118)), (107, None), (107, Some(108))] {
            assert_eq!(
                WeaponBirthClock::new(100).select(now, actor),
                ViewDecision::Current
            );
        }
        let mut clock = WeaponBirthClock::new(100);
        clock.select(107, Some(100));
        assert_eq!(clock.select(140, Some(100)), ViewDecision::Current);
        let mut clock = WeaponBirthClock::new(100);
        clock.select(107, Some(100));
        assert_eq!(clock.select(106, Some(99)), ViewDecision::Current);
    }

    #[test]
    fn tick_wrap_preserves_the_same_deadline_and_convergence() {
        let birth = 0x7fff_fffc;
        let mut clock = WeaponBirthClock::new(birth);
        let start = continuum_tick_add(birth, 7);
        assert_eq!(
            clock.select(start, Some(birth)),
            ViewDecision::History {
                tick: birth,
                rate: 0
            }
        );
        assert_eq!(
            clock.select(continuum_tick_add(start, 7), Some(start)),
            ViewDecision::Current
        );
    }
}
