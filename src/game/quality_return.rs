//! One bounded connection recovery. Reconnection stays in spectator; a fresh
//! user ship selection and server admission are required to return to play.

// Includes the ordinary old-session retirement window plus a fresh server
// measurement window; this is a retry budget, not a second cooldown.
const RECOVERY_TIMEOUT: f64 = 75.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct QualityReturnRecovery {
    pub(super) ship: u8,
    started_at: f64,
    reconnected: bool,
    ready: bool,
    stopped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecoveryAction {
    Reconnect,
    Ready,
    Wait,
    Complete,
    TimedOut,
}

impl QualityReturnRecovery {
    pub(super) fn new(ship: u8, now: f64) -> Option<Self> {
        (ship < 8 && now.is_finite()).then_some(Self {
            ship,
            started_at: now,
            reconnected: false,
            ready: false,
            stopped: false,
        })
    }

    pub(super) fn stop(&mut self) {
        self.stopped = true;
    }

    pub(super) fn awaiting_current_connection(ship: u8, now: f64) -> Option<Self> {
        let mut attempt = Self::new(ship, now)?;
        attempt.reconnected = true;
        Some(attempt)
    }

    pub(super) fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub(super) fn is_ready(&self) -> bool {
        self.ready && !self.stopped
    }

    pub(super) fn progress_text(&self, now: f64) -> String {
        if self.stopped {
            "Connection recovery stopped. ESC and select a ship to retry.".into()
        } else if self.ready {
            "Reconnected. ESC and select a ship to return.".into()
        } else {
            let elapsed = if now.is_finite() {
                (now - self.started_at).max(0.0)
            } else {
                0.0
            };
            let seconds = (RECOVERY_TIMEOUT - elapsed)
                .clamp(0.0, RECOVERY_TIMEOUT)
                .ceil() as u32;
            format!(
                "Reconnecting... Retry time left {}:{:02}. Staying in spectator.",
                seconds / 60,
                seconds % 60
            )
        }
    }

    pub(super) fn poll(
        &mut self,
        now: f64,
        admitted_selected_ship: bool,
        lifecycle_ready: bool,
        transition_pending: bool,
    ) -> RecoveryAction {
        if !now.is_finite() || now < self.started_at {
            return RecoveryAction::Wait;
        }
        if admitted_selected_ship {
            return RecoveryAction::Complete;
        }
        if self.stopped {
            return RecoveryAction::Wait;
        }
        if self.ready {
            return RecoveryAction::Wait;
        }
        if now - self.started_at >= RECOVERY_TIMEOUT {
            self.stopped = true;
            return RecoveryAction::TimedOut;
        }
        if !self.reconnected {
            self.reconnected = true;
            return RecoveryAction::Reconnect;
        }
        if lifecycle_ready && !transition_pending {
            self.ready = true;
            return RecoveryAction::Ready;
        }
        RecoveryAction::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_return_can_follow_an_already_opening_connection_without_a_second_reconnect() {
        let mut attempt = QualityReturnRecovery::awaiting_current_connection(3, 100.0).unwrap();
        assert_eq!(
            attempt.poll(100.0, false, false, false),
            RecoveryAction::Wait
        );
        assert_eq!(attempt.poll(101.0, false, true, true), RecoveryAction::Wait);
        assert_eq!(
            attempt.poll(102.0, false, true, false),
            RecoveryAction::Ready
        );
        assert_eq!(
            attempt.poll(103.0, true, true, false),
            RecoveryAction::Complete
        );
    }

    #[test]
    fn quality_return_reconnects_once_then_waits_for_explicit_user_return() {
        let mut attempt = QualityReturnRecovery::new(6, 100.0).unwrap();
        assert_eq!(
            attempt.poll(100.0, false, false, false),
            RecoveryAction::Reconnect
        );
        // An open transport alone cannot report readiness before observer scope.
        assert_eq!(
            attempt.poll(101.0, false, false, false),
            RecoveryAction::Wait
        );
        assert_eq!(
            attempt.poll(102.0, false, true, false),
            RecoveryAction::Ready
        );
        // Looking away indefinitely cannot schedule a playable ship or a new
        // connection. Only a later genuine selection can obtain admission.
        for now in [110.0, 111.0, 175.0, 3600.0] {
            assert_eq!(attempt.poll(now, false, true, false), RecoveryAction::Wait);
            assert!(attempt.is_ready());
        }
        assert_eq!(
            attempt.poll(3601.0, false, true, true),
            RecoveryAction::Wait
        );
        assert_eq!(
            attempt.poll(3602.0, true, false, false),
            RecoveryAction::Complete
        );
    }

    #[test]
    fn quality_return_is_bounded_even_if_connection_or_measurements_never_recover() {
        let mut attempt = QualityReturnRecovery::new(0, 100.0).unwrap();
        assert_eq!(
            attempt.poll(100.0, false, false, false),
            RecoveryAction::Reconnect
        );
        assert_eq!(
            attempt.poll(174.9, false, false, false),
            RecoveryAction::Wait
        );
        assert_eq!(
            attempt.poll(175.0, false, false, false),
            RecoveryAction::TimedOut
        );
        assert_eq!(
            attempt.poll(200.0, false, true, false),
            RecoveryAction::Wait
        );
        assert!(attempt.is_stopped());
        assert!(attempt.progress_text(200.0).contains("retry"));
        assert_eq!(
            attempt.poll(201.0, true, false, false),
            RecoveryAction::Complete
        );
    }

    #[test]
    fn quality_return_failed_construction_cannot_start_a_reconnect_loop() {
        let mut attempt = QualityReturnRecovery::new(2, 0.0).unwrap();
        assert_eq!(
            attempt.poll(0.0, false, false, false),
            RecoveryAction::Reconnect
        );
        attempt.stop();
        for now in [1.0, 10.0, 100.0] {
            assert_eq!(attempt.poll(now, false, true, false), RecoveryAction::Wait);
        }
    }

    #[test]
    fn quality_return_rejects_invalid_ships_and_cannot_advance_on_bad_clocks() {
        for ship in [8, 9, u8::MAX] {
            assert!(QualityReturnRecovery::new(ship, 1.0).is_none());
        }
        assert!(QualityReturnRecovery::new(0, f64::NAN).is_none());
        let mut attempt = QualityReturnRecovery::new(7, 100.0).unwrap();
        for now in [f64::NAN, f64::INFINITY, 99.0] {
            assert_eq!(attempt.poll(now, false, true, false), RecoveryAction::Wait);
        }
        assert_eq!(
            attempt.poll(100.0, false, false, false),
            RecoveryAction::Reconnect
        );
    }

    #[test]
    fn quality_return_countdown_labels_a_retry_budget_never_a_spawn_deadline() {
        let mut attempt = QualityReturnRecovery::new(0, 100.0).unwrap();
        for (now, time) in [
            (100.0, "1:15"),
            (100.1, "1:15"),
            (115.0, "1:00"),
            (174.1, "0:01"),
            (175.0, "0:00"),
        ] {
            let text = attempt.progress_text(now);
            assert!(text.contains(&format!("Retry time left {time}")));
            assert!(text.contains("Staying in spectator"));
            assert!(!text.contains("automatically"));
        }
        assert_eq!(
            attempt.poll(100.0, false, false, false),
            RecoveryAction::Reconnect
        );
        assert_eq!(
            attempt.poll(101.0, false, true, false),
            RecoveryAction::Ready
        );
        assert!(attempt.progress_text(200.0).contains("select a ship"));
        assert!(!attempt.progress_text(200.0).contains("Retry time"));
        attempt.stop();
        assert!(!attempt.is_ready());
        assert!(attempt.progress_text(200.0).contains("stopped"));
    }
}
