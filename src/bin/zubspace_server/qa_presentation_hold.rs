//! Debug-only real-browser seam: retain presentation packets in their real
//! outbound event queue before normal emission, without falsifying receipts.
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_HOLD: Duration = Duration::from_secs(10);

#[derive(Default)]
struct HoldWindow {
    started: Option<Instant>,
}
impl HoldWindow {
    fn holds(&mut self, armed: bool, now: Instant) -> bool {
        if !armed {
            self.started = None;
            return false;
        }
        let started = self.started.get_or_insert(now);
        now.saturating_duration_since(*started) < MAX_HOLD
    }
}
struct Fixture {
    player: String,
    file: PathBuf,
    window: Mutex<HoldWindow>,
}

pub(crate) fn holds(player: &str) -> bool {
    static FIXTURE: OnceLock<Option<Fixture>> = OnceLock::new();
    let Some(fixture) = FIXTURE.get_or_init(|| {
        let player = std::env::var("ZUBSPACE_SERVER_TEST_PRESENTATION_HOLD_PLAYER").ok()?;
        let file = PathBuf::from(std::env::var_os(
            "ZUBSPACE_SERVER_TEST_PRESENTATION_HOLD_FILE",
        )?);
        assert!(
            file.is_absolute(),
            "presentation hold fixture requires an absolute file path"
        );
        Some(Fixture {
            player,
            file,
            window: Mutex::new(HoldWindow::default()),
        })
    }) else {
        return false;
    };
    if fixture.player != player {
        return false;
    }
    let mut window = fixture.window.lock().expect("QA presentation hold");
    let was_started = window.started.is_some();
    let held = window.holds(fixture.file.is_file(), Instant::now());
    if held && !was_started {
        tracing::info!(
            player,
            maximum_ms = MAX_HOLD.as_millis() as u64,
            "QA projectile presentation hold started"
        );
    } else if was_started && window.started.is_none() {
        tracing::info!(player, "QA projectile presentation hold released");
    }
    held
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hold_is_bounded_and_only_rearms_after_explicit_release() {
        let start = Instant::now();
        let mut hold = HoldWindow::default();
        assert!(!hold.holds(false, start));
        assert!(hold.holds(true, start));
        assert!(hold.holds(true, start + Duration::from_secs(2)));
        assert!(!hold.holds(true, start + MAX_HOLD));
        assert!(!hold.holds(true, start + MAX_HOLD + Duration::from_secs(1)));
        assert!(!hold.holds(false, start + MAX_HOLD));
        assert!(hold.holds(true, start + MAX_HOLD));
    }
}
