use super::*;

#[derive(Default)]
pub(in crate::game) struct PlayerListScroll {
    pub(in crate::game) offset: usize,
    pub(in crate::game) selected: usize,
    selected_player_id: Option<u16>,
    direction: i8,
    repeat_seconds: f32,
}

impl PlayerListScroll {
    fn update(
        &mut self,
        direction: i8,
        dt: f32,
        rows: &[crate::scoreboard::ScoreboardEntry],
        visible_rows: usize,
    ) {
        // Keep the same network player under the cursor when others join,
        // leave or change their name. If that player leaves, keep a nearby row.
        if let Some(index) = self
            .selected_player_id
            .and_then(|id| rows.iter().position(|row| row.player_id == Some(id)))
        {
            self.selected = index;
        }
        self.selected = self.selected.min(rows.len().saturating_sub(1));
        if direction == 0 {
            self.direction = 0;
            self.repeat_seconds = 0.0;
        }
        let steps = if direction == 0 {
            0
        } else if direction != self.direction {
            self.direction = direction;
            self.repeat_seconds = 0.0;
            1
        } else {
            // Scroll in render time, independently of gameplay readiness or
            // catch-up steps; bound stalls so returning focus cannot jump pages.
            self.repeat_seconds += if dt.is_finite() {
                dt.clamp(0.0, 0.1)
            } else {
                0.0
            };
            // Account for f32 accumulation at an exact repeat boundary.
            let steps = ((self.repeat_seconds + f32::EPSILON) / 0.08) as usize;
            self.repeat_seconds = (self.repeat_seconds - steps as f32 * 0.08).max(0.0);
            steps
        };
        self.selected = if direction > 0 {
            self.selected
                .saturating_add(steps)
                .min(rows.len().saturating_sub(1))
        } else {
            self.selected.saturating_sub(steps)
        };
        self.selected_player_id = rows.get(self.selected).and_then(|row| row.player_id);
        self.offset = self.offset.min(rows.len().saturating_sub(visible_rows));
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if visible_rows > 0 && self.selected >= self.offset + visible_rows {
            self.offset = self.selected + 1 - visible_rows;
        }
    }
}

impl Game {
    pub(crate) fn update_player_list_input(
        &mut self,
        render_dt: f32,
        width: f32,
        height: f32,
        input: &mut RenderInputContext,
    ) {
        let visible =
            input.is_down(KeyCode::Space) && !self.chat.active && !self.main_menu_requested;
        if visible && !self.scoreboard_visible {
            self.scoreboard_scroll = PlayerListScroll::default();
            self.client_authority.discard_gameplay_actions_for_menu();
        }
        self.scoreboard_visible = visible;
        if visible {
            let direction =
                i8::from(input.is_down(KeyCode::Down)) - i8::from(input.is_down(KeyCode::Up));
            let rows = self.player_list_rows();
            let (_, visible_rows) = Self::player_list_geometry(width, height, rows.len());
            self.scoreboard_scroll
                .update(direction, render_dt, &rows, visible_rows);
            // Clear immediately even on a zero-step frame. Otherwise a key
            // pressed under the list can fire after Space is released.
            input.drain_actions();
            input.take_gameplay_key_press();
        } else {
            self.scoreboard_scroll.direction = 0;
            self.scoreboard_scroll.repeat_seconds = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(count: usize) -> Vec<crate::scoreboard::ScoreboardEntry> {
        let template = Game::new_network_authority_test()
            .player_list_rows()
            .remove(0);
        (0..count)
            .map(|id| {
                let mut row = template.clone();
                row.player_id = Some(id as u16);
                row.name = format!("Player{id}");
                row
            })
            .collect()
    }

    #[test]
    fn scroll_repeats_in_render_time_clamps_and_reverses_immediately() {
        let mut scroll = PlayerListScroll::default();
        let rows = rows(30);
        scroll.update(1, 0.0, &rows, 20);
        assert_eq!((scroll.selected, scroll.offset), (1, 0));
        for _ in 0..8 {
            scroll.update(1, 0.01, &rows, 20);
        }
        assert_eq!((scroll.selected, scroll.offset), (2, 0));
        for _ in 0..100 {
            scroll.update(1, 0.08, &rows, 20);
        }
        assert_eq!((scroll.selected, scroll.offset), (29, 10));
        scroll.update(-1, 0.0, &rows, 20);
        assert_eq!((scroll.selected, scroll.offset), (28, 10));
        scroll.update(0, 1.0, &rows[..3], 20);
        assert_eq!((scroll.selected, scroll.offset), (2, 0));
        scroll.update(-1, f32::NAN, &rows[..3], 20);
        assert_eq!((scroll.selected, scroll.offset), (1, 0));
    }

    #[test]
    fn cursor_follows_identity_and_stays_visible_across_roster_and_viewport_changes() {
        let mut rows = rows(40);
        let mut scroll = PlayerListScroll::default();
        for _ in 0..25 {
            scroll.update(1, 0.08, &rows, 20);
        }
        assert_eq!((scroll.selected, scroll.offset), (25, 6));
        let mut newcomer = rows[0].clone();
        newcomer.player_id = Some(1000);
        newcomer.name = "Newcomer".into();
        rows.insert(0, newcomer);
        scroll.update(0, 0.0, &rows, 20);
        assert_eq!(scroll.selected, 26);
        assert_eq!(scroll.selected_player_id, Some(25));
        scroll.update(0, 0.0, &rows, 5);
        assert_eq!(scroll.offset, 22);
        rows.retain(|row| row.player_id != Some(25));
        scroll.update(0, 0.0, &rows, 5);
        assert_eq!(scroll.selected_player_id, Some(26));
        for _ in 0..100 {
            scroll.update(-1, 0.08, &rows, 5);
        }
        assert_eq!((scroll.selected, scroll.offset), (0, 0));
        scroll.update(0, 0.0, &[], 0);
        assert_eq!(
            (scroll.selected, scroll.offset, scroll.selected_player_id),
            (0, 0, None)
        );
    }

    #[test]
    fn list_layer_publishes_neutral_flight_input_without_firing_or_turning() {
        let mut game = Game::new_network_authority_test();
        game.ship.vel = Vec2::ZERO;
        let before = (game.ship.pos, game.ship.orientation, game.ship.energy);
        let mut input = RenderInputContext::from_keys(
            [
                KeyCode::Space,
                KeyCode::Up,
                KeyCode::Left,
                KeyCode::LeftControl,
            ],
            [
                KeyCode::Space,
                KeyCode::Up,
                KeyCode::Left,
                KeyCode::LeftControl,
            ],
        );
        game.update_player_list_input(0.016, 1280.0, 720.0, &mut input);
        game.update_ship(GAMEPLAY_STEP_SECONDS, &mut input, &[], true, true);
        assert_eq!(game.last_input.bits(), 0);
        assert_eq!(game.ship.pos, before.0);
        assert_eq!(game.ship.orientation, before.1);
        assert_eq!(game.ship.energy, before.2);
        assert!(game.bullets.is_empty());
        assert_eq!(game.pending_weapon, 0);
    }

    #[test]
    fn held_space_owns_inputs_and_release_restores_gameplay_without_replaying_edges() {
        let mut game = Game::new_network_authority_test();
        let mut input = RenderInputContext::from_keys(
            [KeyCode::Space, KeyCode::Up, KeyCode::LeftControl],
            [KeyCode::Space, KeyCode::Up, KeyCode::LeftControl],
        );
        game.update_player_list_input(0.016, 1280.0, 720.0, &mut input);
        assert!(game.scoreboard_visible);
        assert!(input.drain_actions().is_empty());
        assert!(!input.take_gameplay_key_press());
        input.merge_keys([], []);
        game.update_player_list_input(0.016, 1280.0, 720.0, &mut input);
        assert!(!game.scoreboard_visible);
        assert!(input.drain_actions().is_empty());
        game.chat.begin_input();
        input.merge_keys([KeyCode::Space], [KeyCode::Space]);
        game.update_player_list_input(0.016, 1280.0, 720.0, &mut input);
        assert!(
            !game.scoreboard_visible,
            "typing a chat space cannot open the list"
        );
    }
}
