use super::*;

const MENU_BASE_WIDTH: f32 = 284.0;
const MENU_BASE_HEIGHT: f32 = 172.0;
const MENU_MARGIN: f32 = 12.0;
const MENU_ROW_HEIGHT: f32 = 12.0;
const MENU_TEXT_SCALE: f32 = 1.0;

const MENU_LEFT_ROWS: [&str; 7] = [
    "Esc = Resume",
    "Q = Leave Arena",
    "Space = Player List",
    "K = Controls",
    "F = Fullscreen",
    "Enter = Chat",
    "Alt = Full Map",
];

const MENU_SHIP_ROWS: [&str; 9] = [
    "Ships",
    "1 = Warbird",
    "2 = Javelin",
    "3 = Spider",
    "4 = Leviathan",
    "5 = Terrier",
    "6 = Weasel",
    "7 = Lancaster",
    "8 = Shark",
];

impl Game {
    pub(super) fn draw_menu(&self) {
        if !self.menu_visible {
            return;
        }

        let rect = menu_panel_rect(screen_width(), screen_height());
        draw_rectangle(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            Color::from_rgba(6, 8, 12, 235),
        );
        draw_rectangle(
            rect.x,
            rect.y + 13.0,
            rect.w,
            1.0,
            Color::from_rgba(80, 105, 130, 220),
        );
        draw_rectangle_lines(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            1.0,
            Color::from_rgba(150, 170, 190, 220),
        );

        let center_x = rect.x + rect.w * 0.5;
        self.assets.tall_font.draw_scaled(
            "-= Menu =-",
            center_x,
            rect.y + 4.0,
            TextColor::Green,
            TextAlign::Center,
            MENU_TEXT_SCALE,
        );

        let left_x = rect.x + 8.0;
        let right_x = (rect.x + rect.w - 116.0).max(left_x + 126.0);
        let mut y = rect.y + 20.0;
        for row in MENU_LEFT_ROWS {
            self.assets.tall_font.draw_scaled(
                row,
                left_x,
                y,
                TextColor::White,
                TextAlign::Left,
                MENU_TEXT_SCALE,
            );
            y += MENU_ROW_HEIGHT;
        }

        y = rect.y + 20.0;
        for (index, row) in MENU_SHIP_ROWS.iter().enumerate() {
            let color = if index == 0 {
                TextColor::DarkRed
            } else {
                TextColor::White
            };
            self.assets.tall_font.draw_scaled(
                row,
                right_x,
                y,
                color,
                TextAlign::Left,
                MENU_TEXT_SCALE,
            );
            y += MENU_ROW_HEIGHT;
        }

        self.assets.tall_font.draw_scaled(
            "S = Spectator",
            right_x,
            y,
            TextColor::White,
            TextAlign::Left,
            MENU_TEXT_SCALE,
        );
    }
}

fn menu_panel_rect(screen_width: f32, screen_height: f32) -> Rect {
    let width = MENU_BASE_WIDTH.min((screen_width - MENU_MARGIN * 2.0).max(220.0));
    let height = MENU_BASE_HEIGHT.min((screen_height - MENU_MARGIN * 2.0).max(150.0));
    let x = ((screen_width - width) * 0.5).max(MENU_MARGIN);
    let y = 3.0_f32.max(((screen_height - height) * 0.08).min(MENU_MARGIN));
    Rect::new(x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_panel_stays_inside_common_viewports() {
        for (width, height) in [(1366.0, 768.0), (800.0, 600.0), (320.0, 480.0)] {
            let rect = menu_panel_rect(width, height);
            assert!(rect.x >= MENU_MARGIN);
            assert!(rect.y >= 3.0);
            assert!(rect.x + rect.w <= width - MENU_MARGIN + 0.001);
            assert!(rect.y + rect.h <= height - MENU_MARGIN + 0.001);
            assert!(rect.w >= 220.0);
            assert!(rect.h >= 150.0);
        }
    }

    #[test]
    fn menu_rows_fit_inside_panel_height() {
        let used_left_height = 20.0 + MENU_LEFT_ROWS.len() as f32 * MENU_ROW_HEIGHT + 14.0;
        let used_right_height = 20.0 + (MENU_SHIP_ROWS.len() as f32 + 1.0) * MENU_ROW_HEIGHT;
        assert!(used_left_height <= MENU_BASE_HEIGHT);
        assert!(used_right_height <= MENU_BASE_HEIGHT);
    }
}
