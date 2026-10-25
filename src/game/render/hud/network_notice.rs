use super::*;

impl Game {
    pub(super) fn draw_network_notice(&self) {
        let Some((message, color)) = self.network_notice_text() else {
            return;
        };
        let scale = 1.2;
        let panel_margin = 16.0;
        let text_pad_x = 10.0;
        let text_pad_y = 5.0;
        let glyph_width = self.assets.tall_font.measure_width_scaled("W", scale);
        let max_text_width = (screen_width() - panel_margin * 2.0 - text_pad_x * 2.0).max(80.0);
        let max_chars = (max_text_width / glyph_width.max(1.0)).floor() as usize;
        let message = fitted_network_notice_message(&message, max_chars);
        let text_width = self.assets.tall_font.measure_width_scaled(&message, scale);
        let text_height = self.assets.tall_font.line_height_scaled(scale);
        let panel_width = text_width + text_pad_x * 2.0;
        let panel_height = text_height + text_pad_y * 2.0;
        let x = screen_width() - panel_margin - panel_width;
        let y = 38.0;
        draw_rectangle(
            x,
            y,
            panel_width,
            panel_height,
            Color::from_rgba(7, 10, 16, 220),
        );
        draw_rectangle(x, y, 4.0, panel_height, network_notice_accent_color(color));
        draw_rectangle_lines(
            x,
            y,
            panel_width,
            panel_height,
            1.0,
            Color::from_rgba(140, 165, 190, 170),
        );
        self.assets.tall_font.draw_scaled(
            &message,
            x + text_pad_x,
            y + text_pad_y,
            color,
            TextAlign::Left,
            scale,
        );
    }

    // The corner indicator reports lost connectivity only. Diagnostic notices
    // and failure notifications remain available separately.
    pub(super) fn network_notice_text(&self) -> Option<(String, TextColor)> {
        self.browser_network_notice()
    }

    #[cfg(target_arch = "wasm32")]
    fn browser_network_notice(&self) -> Option<(String, TextColor)> {
        let browser = self.browser_network.as_ref()?;
        browser_network_notice_text(
            BrowserNoticeStatus::from(browser.client.transport().status()),
            self.client_authority.view().readiness.joined,
            browser.rejoin_count > 0,
        )
        .map(|(message, color)| (message.to_string(), color))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn browser_network_notice(&self) -> Option<(String, TextColor)> {
        None
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BrowserNoticeStatus {
    Idle,
    Connecting,
    Open,
    Closed,
    Error,
    Unknown,
}

#[cfg(target_arch = "wasm32")]
impl From<BrowserRtcStatus> for BrowserNoticeStatus {
    fn from(status: BrowserRtcStatus) -> Self {
        match status {
            BrowserRtcStatus::Idle => Self::Idle,
            BrowserRtcStatus::Connecting => Self::Connecting,
            BrowserRtcStatus::Open => Self::Open,
            BrowserRtcStatus::Closed => Self::Closed,
            BrowserRtcStatus::Error => Self::Error,
            BrowserRtcStatus::Unknown(_) => Self::Unknown,
        }
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn browser_network_notice_text(
    status: BrowserNoticeStatus,
    joined_arena: bool,
    recovering: bool,
) -> Option<(&'static str, TextColor)> {
    match status {
        BrowserNoticeStatus::Closed | BrowserNoticeStatus::Error | BrowserNoticeStatus::Unknown => {
            Some(("[ Disconnected ]", TextColor::DarkRed))
        }
        _ if recovering && !joined_arena => Some(("[ Disconnected ]", TextColor::DarkRed)),
        _ => None,
    }
}

fn network_notice_accent_color(color: TextColor) -> Color {
    match color {
        TextColor::Green => Color::from_rgba(70, 220, 115, 255),
        TextColor::Yellow => Color::from_rgba(235, 205, 65, 255),
        TextColor::DarkRed => Color::from_rgba(235, 70, 60, 255),
        TextColor::Blue => Color::from_rgba(90, 155, 235, 255),
        TextColor::White => Color::from_rgba(210, 220, 230, 255),
    }
}

fn fitted_network_notice_message(message: &str, max_chars: usize) -> String {
    let char_count = message.chars().count();
    if char_count <= max_chars {
        return message.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }

    let prefix: String = message.chars().take(max_chars - 3).collect();
    format!("{prefix}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_connection_and_initial_handshake_have_no_indicator() {
        for status in [
            BrowserNoticeStatus::Idle,
            BrowserNoticeStatus::Connecting,
            BrowserNoticeStatus::Open,
        ] {
            assert_eq!(browser_network_notice_text(status, false, false), None);
            assert_eq!(browser_network_notice_text(status, true, false), None);
        }
    }

    #[test]
    fn lost_connection_is_red_even_before_the_join_fact_is_cleared() {
        for status in [
            BrowserNoticeStatus::Closed,
            BrowserNoticeStatus::Error,
            BrowserNoticeStatus::Unknown,
        ] {
            for joined in [false, true] {
                assert_eq!(
                    browser_network_notice_text(status, joined, false),
                    Some(("[ Disconnected ]", TextColor::DarkRed))
                );
            }
        }
    }

    #[test]
    fn recovery_keeps_the_indicator_until_arena_rejoin_completes() {
        for status in [
            BrowserNoticeStatus::Idle,
            BrowserNoticeStatus::Connecting,
            BrowserNoticeStatus::Open,
        ] {
            assert_eq!(
                browser_network_notice_text(status, false, true),
                Some(("[ Disconnected ]", TextColor::DarkRed))
            );
        }
        assert_eq!(
            browser_network_notice_text(BrowserNoticeStatus::Open, true, true),
            None
        );
    }

    #[test]
    fn network_notice_accent_color_matches_state_severity() {
        assert_eq!(
            network_notice_accent_color(TextColor::Green),
            Color::from_rgba(70, 220, 115, 255)
        );
        assert_eq!(
            network_notice_accent_color(TextColor::Yellow),
            Color::from_rgba(235, 205, 65, 255)
        );
        assert_eq!(
            network_notice_accent_color(TextColor::DarkRed),
            Color::from_rgba(235, 70, 60, 255)
        );
    }

    #[test]
    fn fitted_network_notice_message_limits_long_failure_text() {
        assert_eq!(
            fitted_network_notice_message("Login rejected: invalid password", 18),
            "Login rejected:..."
        );
        assert_eq!(
            fitted_network_notice_message("Network error.", 20),
            "Network error."
        );
        assert_eq!(fitted_network_notice_message("Network error.", 3), "...");
        assert_eq!(fitted_network_notice_message("Network error.", 0), "");
    }
}
