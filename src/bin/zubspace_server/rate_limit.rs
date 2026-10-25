use std::time::{Duration, Instant};

pub(crate) fn allow_short_burst(
    last_event_at: &mut Option<Instant>,
    burst_count: &mut u8,
    now: Instant,
    window: Duration,
    burst_limit: u8,
) -> bool {
    if last_event_at.is_some_and(|last| now.duration_since(last) < window) {
        if *burst_count >= burst_limit {
            return false;
        }
        *burst_count = burst_count.saturating_add(1);
    } else {
        *burst_count = 1;
    }
    *last_event_at = Some(now);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_burst_limiter_allows_limited_burst_then_resets_after_window() {
        let start = Instant::now();
        let mut last_event_at = None;
        let mut burst_count = 0;
        let window = Duration::from_millis(100);

        assert!(allow_short_burst(
            &mut last_event_at,
            &mut burst_count,
            start,
            window,
            2,
        ));
        assert!(allow_short_burst(
            &mut last_event_at,
            &mut burst_count,
            start + Duration::from_millis(10),
            window,
            2,
        ));
        assert!(!allow_short_burst(
            &mut last_event_at,
            &mut burst_count,
            start + Duration::from_millis(20),
            window,
            2,
        ));
        assert!(allow_short_burst(
            &mut last_event_at,
            &mut burst_count,
            start + Duration::from_millis(140),
            window,
            2,
        ));
        assert_eq!(burst_count, 1);
    }
}
