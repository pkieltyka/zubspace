//! Advisory `0x86` warnings. A player in a ship whose link stays unhealthy, or
//! whose shots keep being refused because their view of the game arrives too
//! late, is told why, at most once a minute. A warning changes no state.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::connection_quality_protocol::{
    ConnectionQualityWarning, ConnectionQualityWarningReason,
};
use crate::state::ClientHandle;

use super::{ConnectionQualityPolicy, MIN_SAMPLES, QualityReading};

/// This many view-refused shots within `REFUSED_SHOT_WINDOW` earn a warning.
const REFUSED_SHOTS: usize = 3;
const REFUSED_SHOT_WINDOW: Duration = Duration::from_secs(10);
/// How long a link must stay unhealthy before the player is told.
const UNHEALTHY_FOR: Duration = Duration::from_secs(5);
const WARNING_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default)]
pub(crate) struct QualityWarnings {
    refused_shots: VecDeque<Instant>,
    unhealthy_since: Option<Instant>,
    last_sent: Option<Instant>,
}

impl QualityWarnings {
    /// Record a shot refused for an unverified view; true when it is time to
    /// warn the player.
    fn shot_refused(&mut self, now: Instant) -> bool {
        self.refused_shots.push_back(now);
        while self.refused_shots.len() > REFUSED_SHOTS
            || self
                .refused_shots
                .front()
                .is_some_and(|at| now.saturating_duration_since(*at) > REFUSED_SHOT_WINDOW)
        {
            self.refused_shots.pop_front();
        }
        self.refused_shots.len() >= REFUSED_SHOTS && self.take_slot(now)
    }

    /// The link's current fault, if any; returns it once it has lasted
    /// `UNHEALTHY_FOR` and a warning is due.
    fn link(
        &mut self,
        fault: Option<ConnectionQualityWarningReason>,
        now: Instant,
    ) -> Option<ConnectionQualityWarningReason> {
        let Some(fault) = fault else {
            self.unhealthy_since = None;
            return None;
        };
        let since = *self.unhealthy_since.get_or_insert(now);
        (now.saturating_duration_since(since) >= UNHEALTHY_FOR && self.take_slot(now))
            .then_some(fault)
    }

    fn take_slot(&mut self, now: Instant) -> bool {
        if self
            .last_sent
            .is_some_and(|at| now.saturating_duration_since(at) < WARNING_INTERVAL)
        {
            return false;
        }
        self.last_sent = Some(now);
        true
    }
}

/// Unhealthy means a full window that would not let a held player return:
/// loss or RTT at or above the recovery limits. That is the band in which play
/// suffers but `max_*` has not (yet) moved the player to spectator.
pub(super) fn link_fault(
    reading: &QualityReading,
    policy: ConnectionQualityPolicy,
) -> Option<ConnectionQualityWarningReason> {
    if reading.samples < MIN_SAMPLES {
        return None;
    }
    if reading.round_trip_loss * 1000.0 >= f64::from(policy.recovery_loss_per_mille) {
        return Some(ConnectionQualityWarningReason::PacketLoss);
    }
    reading
        .average_rtt_ms
        .is_some_and(|rtt| rtt >= f64::from(policy.recovery_rtt_ms))
        .then_some(ConnectionQualityWarningReason::HighLatency)
}

/// Called from each quality poll with the link's fault (none for a spectator
/// or an unmeasured link).
pub(super) fn poll_link(
    client: &mut ClientHandle,
    fault: Option<ConnectionQualityWarningReason>,
    reading: Option<QualityReading>,
    now: Instant,
) {
    if let Some(reason) = client.quality_warnings.link(fault, now) {
        send(client, reason, reading);
    }
}

/// Called when a shot is refused because the shooter's view could not be
/// verified (`combat_view_*` causes).
pub(crate) fn note_view_refused_shot(client: &mut ClientHandle, now: Instant) {
    if client.player.ship >= 8 || !client.quality_warnings.shot_refused(now) {
        return;
    }
    let reading = client.connection_quality.as_ref().and_then(|quality| {
        quality
            .lock()
            .expect("session quality lock poisoned")
            .reading
    });
    send(
        client,
        ConnectionQualityWarningReason::ShotsRefused,
        reading,
    );
}

fn send(
    client: &mut ClientHandle,
    reason: ConnectionQualityWarningReason,
    reading: Option<QualityReading>,
) {
    if client.arena_stream_id == 0 {
        return;
    }
    let warning = ConnectionQualityWarning {
        server_session_id: crate::wire::server_session_id(),
        arena_stream_id: client.arena_stream_id,
        reason,
        rtt_ms: reading
            .and_then(|reading| reading.average_rtt_ms)
            .map_or(0, |rtt| rtt.round().min(f64::from(u16::MAX)) as u16),
        loss_per_mille: reading.map_or(0, |reading| {
            (reading.round_trip_loss * 1000.0).round().min(1000.0) as u16
        }),
    };
    crate::reliable::send_reliable_payload(client, &warning.encode());
    tracing::info!(
        player_id = client.player.id,
        session_id = client.session_id.get(),
        arena_stream_id = client.arena_stream_id,
        reason = ?warning.reason,
        rtt_ms = warning.rtt_ms,
        loss_per_mille = warning.loss_per_mille,
        "connection quality warning"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(samples: usize, rtt_ms: f64, loss: f64) -> QualityReading {
        QualityReading {
            samples,
            average_rtt_ms: Some(rtt_ms),
            round_trip_loss: loss,
            should_spectate: false,
            sustained_healthy: false,
            may_play: true,
            cooldown_remaining: Duration::ZERO,
        }
    }

    #[test]
    fn a_link_is_faulted_from_the_recovery_limits_once_the_window_is_full() {
        let policy = ConnectionQualityPolicy {
            max_rtt_ms: 400,
            max_loss_per_mille: 150,
            recovery_rtt_ms: 300,
            recovery_loss_per_mille: 100,
        };
        assert_eq!(link_fault(&reading(20, 299.0, 0.099), policy), None);
        assert_eq!(
            link_fault(&reading(20, 300.0, 0.0), policy),
            Some(ConnectionQualityWarningReason::HighLatency)
        );
        assert_eq!(
            link_fault(&reading(20, 40.0, 0.1), policy),
            Some(ConnectionQualityWarningReason::PacketLoss)
        );
        assert_eq!(
            link_fault(&reading(20, 500.0, 0.3), policy),
            Some(ConnectionQualityWarningReason::PacketLoss),
            "loss is named first: it is the less obvious cause"
        );
        assert_eq!(link_fault(&reading(19, 500.0, 0.3), policy), None);
    }

    #[test]
    fn an_unhealthy_link_is_reported_after_five_seconds_then_at_most_once_a_minute() {
        let start = Instant::now();
        let at = |seconds: f64| start + Duration::from_secs_f64(seconds);
        let high = Some(ConnectionQualityWarningReason::HighLatency);
        let mut warnings = QualityWarnings::default();
        assert_eq!(warnings.link(high, at(0.0)), None);
        assert_eq!(warnings.link(high, at(4.9)), None);
        assert_eq!(warnings.link(None, at(5.0)), None, "a healthy poll resets");
        assert_eq!(warnings.link(high, at(6.0)), None);
        assert_eq!(warnings.link(high, at(11.0)), high);
        assert_eq!(warnings.link(high, at(12.0)), None);
        assert_eq!(warnings.link(high, at(70.9)), None);
        assert_eq!(warnings.link(high, at(71.0)), high);
    }

    #[test]
    fn three_view_refused_shots_within_ten_seconds_warn_and_share_the_minute_limit() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut warnings = QualityWarnings::default();
        assert!(!warnings.shot_refused(at(0)));
        assert!(!warnings.shot_refused(at(1)));
        assert!(
            !warnings.shot_refused(at(12)),
            "the first two have aged out"
        );
        assert!(!warnings.shot_refused(at(13)));
        assert!(warnings.shot_refused(at(14)));
        assert!(!warnings.shot_refused(at(15)));
        let high = Some(ConnectionQualityWarningReason::HighLatency);
        assert_eq!(warnings.link(high, at(20)), None);
        assert_eq!(
            warnings.link(high, at(30)),
            None,
            "one warning a minute, whatever the reason"
        );
        assert!(!warnings.shot_refused(at(74)));
        assert!(!warnings.shot_refused(at(75)));
        assert!(
            warnings.shot_refused(at(76)),
            "a minute after the last warning"
        );
    }
}
