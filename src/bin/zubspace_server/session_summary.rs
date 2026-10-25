//! One INFO line per session at removal: how the session went for the player.
use std::time::Instant;

use crate::state::ClientHandle;

#[derive(Clone, Debug)]
pub(crate) struct SessionSummary {
    started: Instant,
    // Kills and deaths restored from the identity's earlier sessions.
    start_kills: u16,
    start_deaths: u16,
    browser_reports: u32,
    frames: u64,
    frame_time_us: u64,
    slow_frames: u64,
    worst_frame_us: u32,
    corrections: u64,
    ice_rtt_us_total: u64,
    ice_rtt_samples: u32,
    ice_rtt_us_max: u32,
    pub(crate) snapbacks: u32,
    pub(crate) position_rejects: u32,
    /// Times the page reported itself hidden (tabbed away or minimized).
    pub(crate) page_hidden_periods: u32,
    weapons_fired: u32,
    weapons_rejected: u32,
    weapons_rejected_combat_view: u32,
}

impl Default for SessionSummary {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            start_kills: 0,
            start_deaths: 0,
            browser_reports: 0,
            frames: 0,
            frame_time_us: 0,
            slow_frames: 0,
            worst_frame_us: 0,
            corrections: 0,
            ice_rtt_us_total: 0,
            ice_rtt_samples: 0,
            ice_rtt_us_max: 0,
            snapbacks: 0,
            position_rejects: 0,
            weapons_fired: 0,
            weapons_rejected: 0,
            weapons_rejected_combat_view: 0,
            page_hidden_periods: 0,
        }
    }
}

impl SessionSummary {
    pub(crate) fn scores_restored(&mut self, kills: u16, deaths: u16) {
        self.start_kills = kills;
        self.start_deaths = deaths;
    }

    /// One accepted browser experience report (see `observability`).
    pub(crate) fn browser_report(
        &mut self,
        frames: u32,
        frame_time_us: u32,
        slow_frames: u32,
        worst_frame_us: u32,
        corrections: u32,
        ice_rtt_us: Option<u32>,
    ) {
        self.browser_reports += 1;
        self.frames += u64::from(frames);
        self.frame_time_us += u64::from(frame_time_us);
        self.slow_frames += u64::from(slow_frames);
        self.worst_frame_us = self.worst_frame_us.max(worst_frame_us);
        self.corrections += u64::from(corrections);
        // A browser can report 0 before its first ICE measurement.
        if let Some(rtt) = ice_rtt_us.filter(|&rtt| rtt > 0) {
            self.ice_rtt_us_total += u64::from(rtt);
            self.ice_rtt_samples += 1;
            self.ice_rtt_us_max = self.ice_rtt_us_max.max(rtt);
        }
    }

    pub(crate) fn weapon(&mut self, fired: bool, combat_view_blocked: bool) {
        if fired {
            self.weapons_fired += 1;
        } else {
            self.weapons_rejected += 1;
            self.weapons_rejected_combat_view += u32::from(combat_view_blocked);
        }
    }
}

pub(crate) fn log(client: &ClientHandle) {
    let summary = &client.session_summary;
    let quality = client
        .connection_quality
        .as_ref()
        .map(|shared| shared.lock().expect("session quality lock poisoned"));
    let ms = |us: u64| us as f64 / 1000.0;
    tracing::info!(
        event = "session_summary",
        player_id = client.player.id,
        session_id = client.session_id.get(),
        name = %client.player.name,
        ship = client.player.ship,
        duration_s = summary.started.elapsed().as_secs(),
        kills = client.player.wins.saturating_sub(summary.start_kills),
        deaths = client.player.losses.saturating_sub(summary.start_deaths),
        positions_received = client.positions_received,
        browser_reports = summary.browser_reports,
        fps_mean = (summary.frame_time_us > 0)
            .then(|| summary.frames as f64 * 1_000_000.0 / summary.frame_time_us as f64),
        slow_frames = summary.slow_frames,
        worst_frame_ms = ms(u64::from(summary.worst_frame_us)),
        corrections = summary.corrections,
        ice_rtt_ms_mean = (summary.ice_rtt_samples > 0)
            .then(|| ms(summary.ice_rtt_us_total) / f64::from(summary.ice_rtt_samples)),
        ice_rtt_ms_max = ms(u64::from(summary.ice_rtt_us_max)),
        probe_rtt_ms_mean = quality.as_ref().and_then(|quality| quality.window_rtt_ms_mean()),
        probe_rtt_ms_peak = quality.as_ref().map(|quality| quality.window_rtt_ms_peak),
        probes_sent = quality.as_ref().map(|quality| quality.sent),
        probe_echoes = quality.as_ref().map(|quality| quality.echoes),
        snapbacks = summary.snapbacks,
        position_rejects = summary.position_rejects,
        weapons_fired = summary.weapons_fired,
        weapons_rejected = summary.weapons_rejected,
        weapons_rejected_combat_view = summary.weapons_rejected_combat_view,
        page_hidden_periods = summary.page_hidden_periods,
        "session summary"
    );
}
