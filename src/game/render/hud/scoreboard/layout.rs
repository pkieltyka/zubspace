use crate::scoreboard::ScoreboardEntry;

pub(super) struct ScoreboardLayout {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) text_x: f32,
    pub(super) header_height: f32,
    pub(super) footer_height: f32,
    pub(super) row_height: f32,
    pub(super) score_right: f32,
    pub(super) kills_right: f32,
    pub(super) deaths_right: f32,
    pub(super) rating_right: f32,
    pub(super) average_right: f32,
    pub(super) wrapped_rows: bool,
    pub(super) visible_player_rows: usize,
    pub(super) hidden_player_rows: usize,
}

pub(super) fn scoreboard_layout(
    screen_width: f32,
    screen_height: f32,
    player_rows: usize,
) -> ScoreboardLayout {
    // The reference game's Full view uses 452 pixels and 12-pixel rows. Score replaces
    // its unused squad column; narrow windows wrap stats rather than hide them.
    let width = 452.0_f32.min((screen_width - 8.0).max(1.0));
    let wrapped_rows = width < 452.0;
    // The reference game defaults to twenty rows and limits its sliding view to 3/4
    // of the screen. Keep that compact default even on a tall desktop.
    let height_budget = (screen_height * 0.75).min((screen_height - 8.0).max(0.0));
    let header_height = if wrapped_rows { 28.0_f32 } else { 16.0_f32 }.min(height_budget);
    let row_height = if wrapped_rows { 24.0 } else { 12.0 };
    let capacity = |footer: f32| {
        (((height_budget - header_height - footer).max(0.0) / row_height) as usize).min(20)
    };
    let footer_height =
        if player_rows > capacity(0.0) && height_budget >= header_height + row_height + 14.0 {
            14.0
        } else {
            0.0
        };
    let visible_player_rows = player_rows.min(capacity(footer_height));
    ScoreboardLayout {
        x: 4.0,
        y: 4.0,
        width,
        height: header_height + visible_player_rows as f32 * row_height + footer_height,
        text_x: 12.0,
        header_height,
        footer_height,
        row_height,
        wrapped_rows,
        score_right: if wrapped_rows { width } else { 228.0 },
        kills_right: if wrapped_rows { width * 0.2 } else { 284.0 },
        deaths_right: if wrapped_rows { width * 0.4 } else { 340.0 },
        rating_right: if wrapped_rows { width * 0.63 } else { 396.0 },
        average_right: width,
        visible_player_rows,
        hidden_player_rows: player_rows.saturating_sub(visible_player_rows),
    }
}

pub(super) fn scoreboard_name(row: &ScoreboardEntry) -> String {
    row.name.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanded_view_preserves_reference_desktop_geometry_and_wraps_narrow_rows() {
        for width in [640.0, 1280.0] {
            let panel = scoreboard_layout(width, 720.0, 16);
            assert_eq!(
                (panel.x, panel.y, panel.width, panel.height),
                (4.0, 4.0, 452.0, 208.0)
            );
            assert!(!panel.wrapped_rows);
            assert_eq!(panel.visible_player_rows, 16);
        }
        let panel = scoreboard_layout(320.0, 720.0, 16);
        assert_eq!(panel.width, 312.0);
        assert!(panel.wrapped_rows);
        assert_eq!(panel.row_height, 24.0);
        assert_eq!(panel.height, 412.0);
    }

    #[test]
    fn short_viewports_scroll_without_extending_below_the_screen() {
        let panel = scoreboard_layout(320.0, 160.0, 40);
        assert!(panel.visible_player_rows < 40);
        assert_eq!(panel.hidden_player_rows, 40 - panel.visible_player_rows);
        assert!(panel.y + panel.height <= 156.0);
    }

    #[test]
    fn large_rosters_stop_at_twenty_rows_and_reserve_scroll_guidance() {
        for width in [320.0, 1280.0] {
            let twenty = scoreboard_layout(width, 1080.0, 20);
            let full = scoreboard_layout(width, 1080.0, 500);
            assert_eq!(twenty.visible_player_rows, 20);
            assert_eq!(full.visible_player_rows, 20);
            assert_eq!(full.hidden_player_rows, 480);
            assert_eq!(full.height, twenty.height + full.footer_height);
            assert_eq!(full.footer_height, 14.0);
        }
        for height in [0.0, 16.0, 80.0, 160.0, 320.0, 720.0] {
            for width in [320.0, 1280.0] {
                let panel = scoreboard_layout(width, height, 500);
                assert!(panel.height <= (height * 0.75).min((height - 8.0).max(0.0)));
                assert!(panel.visible_player_rows <= 20);
            }
        }
    }
}
