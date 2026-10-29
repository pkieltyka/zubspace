use macroquad::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

/// A bounded semantic queue prevents a malicious or stalled renderer from
/// retaining unbounded UI/gameplay work. When full, the newest action fails
/// closed so actions already observed keep their chronological order.
pub(crate) const MAX_PENDING_SEMANTIC_ACTIONS: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct InputModifiers {
    pub(crate) shift: bool,
    pub(crate) control: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SemanticInputActionKind {
    ChatEnter,
    ChatBackspace,
    ChatAutocompleteOrBomb,
    ChatHistoryPrevious,
    ChatHistoryNext,
    CancelChatOrToggleMenu,
    ResetShip,
    ToggleSpectator,
    ToggleArenaDirectory,
    ToggleDeveloperOverlay,
    SelectShip(u8),
    NumberShortcut(u8),
    MenuEnterSpectator,
    MenuControls,
    MenuFullscreen,
    MenuToggleArenaDirectory,
    MenuFrequencyStep(i16),
    ChangeMap(i8),
    ChangeZone(i8),
    PrimaryFireOrSpectate,
    BombOrSpectate,
    Mine,
    Repel,
    RepelOrMenuQuit,
    Burst,
    ToggleMultifire,
    ToggleCloak,
    ToggleStealth,
    ToggleAntiwarp,
    ToggleXradar,
    PlacePortal,
    UseOrPlacePortal,
    RandomWarp,
    Warp,
    Rocket,
    Brick,
    Decoy,
    Thor,
    Attach,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SemanticInputAction {
    pub(crate) kind: SemanticInputActionKind,
    pub(crate) modifiers: InputModifiers,
}

/// Stable developer-metric slots used by real-browser input regressions.
/// These count semantic dispatch attempts, independently of inventory and
/// cooldown acceptance, so a zero-step key tap can be proven exactly once.
pub(crate) fn semantic_input_metric_index(kind: SemanticInputActionKind) -> Option<usize> {
    use SemanticInputActionKind as Action;
    match kind {
        Action::PrimaryFireOrSpectate => Some(0),
        Action::ChatAutocompleteOrBomb | Action::BombOrSpectate => Some(11),
        Action::Repel | Action::RepelOrMenuQuit => Some(1),
        Action::Rocket => Some(2),
        Action::Brick => Some(3),
        Action::Decoy => Some(4),
        Action::Thor => Some(5),
        Action::Burst => Some(6),
        Action::ToggleCloak => Some(7),
        Action::ToggleAntiwarp => Some(8),
        Action::PlacePortal => Some(9),
        Action::Warp | Action::RandomWarp => Some(10),
        _ => None,
    }
}

/// Press-time actions whose local prediction and outbound weapon field must
/// bind to the combat view that was drawn when the browser captured the edge.
/// Movement/status actions and Rocket/Warp/Portal lifecycle transactions do
/// not use the projectile combat-view admission contract.
pub(crate) const fn semantic_action_requires_weapon_submission(
    kind: SemanticInputActionKind,
) -> bool {
    use SemanticInputActionKind as Action;
    matches!(
        kind,
        Action::PrimaryFireOrSpectate
            | Action::ChatAutocompleteOrBomb
            | Action::BombOrSpectate
            | Action::Mine
            | Action::Repel
            | Action::RepelOrMenuQuit
            | Action::Burst
            | Action::Brick
            | Action::Decoy
            | Action::Thor
            | Action::NumberShortcut(1 | 3 | 4 | 5)
    )
}

/// One live subscription, kept outside the cloneable gameplay snapshot.
/// Drain it every rendered frame so press modifiers come from physical events,
/// including presses released before the renderer next samples held keys.
pub(crate) struct FrameInputEvents {
    subscriber: usize,
}

impl FrameInputEvents {
    pub(crate) fn new() -> Self {
        Self {
            subscriber: macroquad::input::utils::register_input_subscriber(),
        }
    }

    fn take_press_modifiers(&mut self) -> KeyPressModifiers {
        let mut events = KeyPressModifiers::default();
        macroquad::input::utils::repeat_all_miniquad_input(&mut events, self.subscriber);
        events
    }
}

#[derive(Default)]
struct KeyPressModifiers {
    by_key: HashMap<KeyCode, InputModifiers>,
    edit_presses: Vec<KeyCode>,
}

impl macroquad::miniquad::EventHandler for KeyPressModifiers {
    fn update(&mut self) {}
    fn draw(&mut self) {}

    fn key_down_event(
        &mut self,
        key: KeyCode,
        modifiers: macroquad::miniquad::KeyMods,
        repeat: bool,
    ) {
        if matches!(key, KeyCode::Backspace | KeyCode::Delete)
            && self.edit_presses.len() < MAX_PENDING_SEMANTIC_ACTIONS
        {
            self.edit_presses.push(key);
        }
        if !repeat {
            // Macroquad exposes one pressed edge per key per frame. Retain
            // that first edge's modifiers, never a later repeat/chord's state.
            self.by_key.entry(key).or_insert(InputModifiers {
                shift: modifiers.shift,
                control: modifiers.ctrl,
            });
        }
    }
}

/// Latest held-key snapshot plus press-time semantic actions waiting for the
/// next logical gameplay tick. A 144 Hz render frame may execute no 60 Hz
/// logical step, so actions retain their original modifier snapshot and order
/// until a step drains them. Held movement remains a current-state sample and
/// is therefore visible to every step in a 2–8-step catch-up batch.
#[derive(Clone, Debug, Default)]
pub(crate) struct RenderInputContext {
    down: HashSet<KeyCode>,
    mouse_down: HashSet<MouseButton>,
    pending_actions: VecDeque<SemanticInputAction>,
    // A gameplay key pressed while authoritative simulation is temporarily
    // closed (death/lifecycle/correction) is still a fresh human edge. Keep
    // one bounded bit until the next live ship step so the respawn latch can
    // distinguish it from a key that was merely held across the boundary.
    pending_gameplay_key_press: bool,
    dropped_actions: u64,
}

impl RenderInputContext {
    pub(crate) fn capture_frame(&mut self, events: &mut FrameInputEvents, chat_active: bool) {
        let frame_action_start = self.pending_actions.len();
        self.down = get_keys_down();
        let modifiers = events.take_press_modifiers();
        self.capture_chat_frame_keys(get_keys_pressed(), &modifiers, chat_active);
        self.capture_mouse_buttons(
            [MouseButton::Left, MouseButton::Right]
                .into_iter()
                .filter(|&button| is_mouse_button_down(button)),
            [MouseButton::Left, MouseButton::Right]
                .into_iter()
                .filter(|&button| is_mouse_button_pressed(button)),
            frame_action_start,
        );
    }

    fn capture_chat_frame_keys(
        &mut self,
        mut pressed: HashSet<KeyCode>,
        events: &KeyPressModifiers,
        chat_active: bool,
    ) {
        let mut fallback_edits = 0;
        if chat_active {
            fallback_edits += usize::from(pressed.remove(&KeyCode::Backspace));
            fallback_edits += usize::from(pressed.remove(&KeyCode::Delete));
        }
        self.capture_pressed_keys(pressed, &events.by_key);
        if chat_active {
            // Consume real repeats only in the chat layer; Delete must never
            // become multifire/Burst while editing. Fallback covers the first
            // subscription frame when only Macroquad's pressed set is known.
            let count = if events.edit_presses.is_empty() {
                fallback_edits
            } else {
                events.edit_presses.len()
            };
            for _ in 0..count {
                if self.pending_actions.len() >= MAX_PENDING_SEMANTIC_ACTIONS {
                    self.dropped_actions = self.dropped_actions.saturating_add(1);
                    break;
                }
                self.pending_actions.push_back(SemanticInputAction {
                    kind: SemanticInputActionKind::ChatBackspace,
                    modifiers: InputModifiers::default(),
                });
            }
        }
    }

    pub(crate) fn mouse_is_down(&self, button: MouseButton) -> bool {
        self.mouse_down.contains(&button)
    }

    fn capture_mouse_buttons(
        &mut self,
        down: impl IntoIterator<Item = MouseButton>,
        pressed: impl IntoIterator<Item = MouseButton>,
        frame_action_start: usize,
    ) {
        self.mouse_down = down.into_iter().collect();
        for button in pressed {
            let kind = match button {
                MouseButton::Left => SemanticInputActionKind::PrimaryFireOrSpectate,
                MouseButton::Right => SemanticInputActionKind::BombOrSpectate,
                _ => continue,
            };
            self.pending_gameplay_key_press = true;
            // Keyboard and mouse aliases pressed in the same frame are one
            // intent. Earlier zero-step frames retain their separate taps.
            if self
                .pending_actions
                .iter()
                .skip(frame_action_start)
                .any(|action| {
                    action.kind == kind
                        || (kind == SemanticInputActionKind::BombOrSpectate
                            && action.kind == SemanticInputActionKind::ChatAutocompleteOrBomb)
                })
            {
                continue;
            }
            if self.pending_actions.len() >= MAX_PENDING_SEMANTIC_ACTIONS {
                self.dropped_actions = self.dropped_actions.saturating_add(1);
                continue;
            }
            // Mouse weapons remain bullets/bombs while afterburning. They
            // never inherit Shift+Control Repel or Shift+Tab Mine semantics.
            self.pending_actions.push_back(SemanticInputAction {
                kind,
                modifiers: InputModifiers::default(),
            });
        }
    }

    #[cfg(test)]
    pub(crate) fn from_mouse(
        down: impl IntoIterator<Item = MouseButton>,
        pressed: impl IntoIterator<Item = MouseButton>,
    ) -> Self {
        let mut context = Self::default();
        context.capture_mouse_buttons(down, pressed, 0);
        context
    }

    /// Retire anything not drained by the first logical step. In normal game
    /// flow `Game::update` drains the queue in that step; the clear is defense
    /// in depth for a mode/session path that intentionally ignores actions.
    pub(crate) fn finish_gameplay_batch(&mut self) {
        self.pending_actions.clear();
    }

    #[cfg(test)]
    pub(crate) fn clear_pending_actions(&mut self) {
        self.pending_actions.clear();
    }

    pub(crate) fn is_down(&self, key: KeyCode) -> bool {
        self.down.contains(&key)
    }

    pub(crate) fn drain_actions(&mut self) -> Vec<SemanticInputAction> {
        self.pending_actions.drain(..).collect()
    }

    pub(crate) fn has_pending_weapon_submission_action(&self) -> bool {
        self.pending_actions
            .iter()
            .any(|action| semantic_action_requires_weapon_submission(action.kind))
    }

    pub(crate) fn take_gameplay_key_press(&mut self) -> bool {
        std::mem::take(&mut self.pending_gameplay_key_press)
    }

    #[cfg(test)]
    pub(crate) fn pending_actions(&self) -> Vec<SemanticInputAction> {
        self.pending_actions.iter().copied().collect()
    }

    #[cfg(test)]
    pub(crate) fn dropped_actions(&self) -> u64 {
        self.dropped_actions
    }

    fn capture_pressed_keys(
        &mut self,
        pressed: HashSet<KeyCode>,
        press_modifiers: &HashMap<KeyCode, InputModifiers>,
    ) {
        if pressed.is_empty() {
            return;
        }
        self.pending_gameplay_key_press |= pressed.iter().any(|key| {
            matches!(
                key,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::W
                    | KeyCode::A
                    | KeyCode::S
                    | KeyCode::D
                    | KeyCode::LeftShift
                    | KeyCode::RightShift
                    | KeyCode::LeftControl
                    | KeyCode::RightControl
                    | KeyCode::Tab
                    | KeyCode::X
                    | KeyCode::Z
                    | KeyCode::M
                    | KeyCode::Q
                    | KeyCode::GraveAccent
                    | KeyCode::Comma
                    | KeyCode::Period
                    | KeyCode::Slash
                    | KeyCode::Delete
                    | KeyCode::Minus
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::Insert
                    | KeyCode::F3
                    | KeyCode::F4
                    | KeyCode::F5
                    | KeyCode::F6
                    | KeyCode::F7
                    | KeyCode::Key0
                    | KeyCode::Key1
                    | KeyCode::Key2
                    | KeyCode::Key3
                    | KeyCode::Key4
                    | KeyCode::Key5
                    | KeyCode::Key6
                    | KeyCode::Key7
                    | KeyCode::Key8
                    | KeyCode::Key9
            )
        });
        let held_modifiers = InputModifiers {
            shift: self.down.contains(&KeyCode::LeftShift)
                || self.down.contains(&KeyCode::RightShift),
            control: self.down.contains(&KeyCode::LeftControl)
                || self.down.contains(&KeyCode::RightControl),
        };
        let mut captured_this_frame = HashSet::new();
        for key in CANONICAL_EDGE_KEY_ORDER {
            if !pressed.contains(key) {
                continue;
            }
            // Held state is a fallback for synthetic/native test snapshots and
            // the subscription's first frame; actual events retain press time.
            let modifiers = press_modifiers.get(key).copied().unwrap_or(held_modifiers);
            let Some(kind) = semantic_action_for_press(*key, modifiers) else {
                continue;
            };
            // Left/right Control, Comma and X; Z and Period; M and Shift with
            // Tab, Z or Period; and Q/Grave/Slash/Shift+Control are aliases.
            // Preserve canonical per-frame weapon-phase ordering and coalesce
            // each alias family once, using each edge's original modifiers.
            let alias = if kind == SemanticInputActionKind::RepelOrMenuQuit {
                SemanticInputActionKind::Repel
            } else {
                match kind {
                    SemanticInputActionKind::NumberShortcut(number) => {
                        number_gameplay_action(number, modifiers)
                    }
                    _ => kind,
                }
            };
            if !captured_this_frame.insert(alias) {
                continue;
            }
            if self.pending_actions.len() >= MAX_PENDING_SEMANTIC_ACTIONS {
                self.dropped_actions = self.dropped_actions.saturating_add(1);
                continue;
            }
            self.pending_actions
                .push_back(SemanticInputAction { kind, modifiers });
        }
    }

    #[cfg(test)]
    pub(crate) fn from_keys(
        down: impl IntoIterator<Item = KeyCode>,
        pressed: impl IntoIterator<Item = KeyCode>,
    ) -> Self {
        let mut context = Self {
            down: down.into_iter().collect(),
            ..Self::default()
        };
        context.capture_pressed_keys(pressed.into_iter().collect(), &HashMap::new());
        context
    }

    #[cfg(test)]
    pub(crate) fn merge_keys(
        &mut self,
        down: impl IntoIterator<Item = KeyCode>,
        pressed: impl IntoIterator<Item = KeyCode>,
    ) {
        self.down = down.into_iter().collect();
        self.capture_pressed_keys(pressed.into_iter().collect(), &HashMap::new());
    }
}

fn semantic_action_for_press(
    key: KeyCode,
    modifiers: InputModifiers,
) -> Option<SemanticInputActionKind> {
    use SemanticInputActionKind as Action;
    match key {
        KeyCode::Enter => Some(Action::ChatEnter),
        KeyCode::Backspace => Some(Action::ChatBackspace),
        KeyCode::Tab if modifiers.shift => Some(Action::Mine),
        KeyCode::Tab => Some(Action::ChatAutocompleteOrBomb),
        KeyCode::Up => Some(Action::ChatHistoryPrevious),
        KeyCode::Down => Some(Action::ChatHistoryNext),
        KeyCode::Escape => Some(Action::CancelChatOrToggleMenu),
        KeyCode::F11 => Some(Action::ToggleSpectator),

        KeyCode::F9 => Some(Action::ToggleDeveloperOverlay),
        KeyCode::Key1 => Some(Action::NumberShortcut(1)),
        KeyCode::Key2 => Some(Action::NumberShortcut(2)),
        KeyCode::Key3 => Some(Action::NumberShortcut(3)),
        KeyCode::Key4 => Some(Action::NumberShortcut(4)),
        KeyCode::Key5 => Some(Action::NumberShortcut(5)),
        KeyCode::Key6 => Some(Action::NumberShortcut(6)),
        KeyCode::Key7 => Some(Action::NumberShortcut(7)),
        KeyCode::Key8 => Some(Action::NumberShortcut(8)),
        KeyCode::Key9 => Some(Action::NumberShortcut(9)),
        KeyCode::Key0 => Some(Action::NumberShortcut(0)),
        KeyCode::S => Some(Action::MenuEnterSpectator),
        KeyCode::K => Some(Action::MenuControls),
        KeyCode::F => Some(Action::MenuFullscreen),

        // Z bombs and X fires bullets for a left hand beside the arrow keys.
        // Like Tab, Z lays a mine with Shift; X keeps firing while Shift runs
        // the afterburner.
        KeyCode::Z if modifiers.shift => Some(Action::Mine),
        KeyCode::Z => Some(Action::BombOrSpectate),
        KeyCode::X => Some(Action::PrimaryFireOrSpectate),
        KeyCode::M => Some(Action::Mine),
        KeyCode::Q => Some(Action::RepelOrMenuQuit),
        KeyCode::GraveAccent => Some(Action::Repel),
        // The second layout (WASD flight): `,` bullets, `.` bombs, `/`
        // repel. Shift+`.` lays a mine; Shift changes neither `,` nor `/`.
        KeyCode::Comma => Some(Action::PrimaryFireOrSpectate),
        KeyCode::Period if modifiers.shift => Some(Action::Mine),
        KeyCode::Period => Some(Action::BombOrSpectate),
        KeyCode::Slash => Some(Action::Repel),
        KeyCode::LeftControl | KeyCode::RightControl if modifiers.shift => Some(Action::Repel),
        KeyCode::LeftControl | KeyCode::RightControl => Some(Action::PrimaryFireOrSpectate),
        KeyCode::LeftShift | KeyCode::RightShift if modifiers.control => Some(Action::Repel),
        KeyCode::Delete if modifiers.shift => Some(Action::Burst),
        KeyCode::Delete | KeyCode::Minus => Some(Action::ToggleMultifire),
        KeyCode::Home if modifiers.shift => Some(Action::ToggleCloak),
        KeyCode::Home => Some(Action::ToggleStealth),
        KeyCode::End if modifiers.shift => Some(Action::ToggleAntiwarp),
        KeyCode::End => Some(Action::ToggleXradar),
        KeyCode::Insert if modifiers.shift => Some(Action::PlacePortal),
        KeyCode::Insert => Some(Action::Warp),
        KeyCode::F3 => Some(Action::Rocket),
        KeyCode::F4 => Some(Action::Brick),
        KeyCode::F5 => Some(Action::Decoy),
        KeyCode::F6 => Some(Action::Thor),
        KeyCode::F7 => Some(Action::Attach),
        _ => None,
    }
}

/// Resolve number edges only after chat/menu has consumed its input layer.
pub(crate) fn number_gameplay_action(
    number: u8,
    modifiers: InputModifiers,
) -> SemanticInputActionKind {
    use SemanticInputActionKind as Action;
    match number {
        1 => Action::Burst,
        2 => Action::Rocket,
        3 => Action::Decoy,
        4 => Action::Brick,
        5 => Action::Thor,
        6 => Action::ToggleStealth,
        7 => Action::ToggleCloak,
        8 => Action::ToggleXradar,
        9 => Action::ToggleAntiwarp,
        0 if modifiers.shift => Action::RandomWarp,
        0 => Action::UseOrPlacePortal,
        _ => unreachable!("number shortcut outside 0..9"),
    }
}

#[cfg(test)]
pub(crate) fn ship_number_key(key: KeyCode) -> Option<usize> {
    match semantic_action_for_press(key, InputModifiers::default()) {
        Some(SemanticInputActionKind::NumberShortcut(number @ 1..=8)) => Some(number as usize - 1),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn frequency_step_key(key: KeyCode) -> Option<i16> {
    match semantic_action_for_press(key, InputModifiers::default()) {
        Some(SemanticInputActionKind::MenuFrequencyStep(step)) => Some(step),
        _ => None,
    }
}

// `get_keys_pressed()` is an unordered set. This table supplies stable order
// within one frame; order across zero-step frames is naturally FIFO.
const CANONICAL_EDGE_KEY_ORDER: &[KeyCode] = &[
    KeyCode::Enter,
    KeyCode::Backspace,
    KeyCode::Tab,
    KeyCode::Up,
    KeyCode::Down,
    KeyCode::Escape,
    KeyCode::F11,
    KeyCode::K,
    KeyCode::F,
    KeyCode::F10,
    KeyCode::F9,
    // Rocket precedes weapon fire for both its aliases.
    KeyCode::Key2,
    KeyCode::F3,
    KeyCode::Key1,
    KeyCode::Key3,
    KeyCode::Key4,
    KeyCode::Key5,
    KeyCode::Key6,
    KeyCode::Key7,
    KeyCode::Key8,
    KeyCode::Key9,
    KeyCode::Key0,
    KeyCode::S,
    KeyCode::A,
    // Primary fire precedes bomb fire, matching the former Space alias and
    // the reference game's weapon phase ordering for a simultaneous chord.
    KeyCode::LeftControl,
    KeyCode::RightControl,
    KeyCode::Comma,
    KeyCode::X,
    KeyCode::Z,
    KeyCode::Period,
    KeyCode::M,
    KeyCode::Q,
    KeyCode::GraveAccent,
    KeyCode::Slash,
    KeyCode::LeftShift,
    KeyCode::RightShift,
    KeyCode::Delete,
    KeyCode::Minus,
    KeyCode::Home,
    KeyCode::End,
    KeyCode::Insert,
    KeyCode::F4,
    KeyCode::F5,
    KeyCode::F6,
    KeyCode::F7,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(input: &RenderInputContext) -> Vec<SemanticInputActionKind> {
        input
            .pending_actions()
            .into_iter()
            .map(|action| action.kind)
            .collect()
    }

    #[test]
    fn space_has_no_gameplay_action_and_does_not_wake_weapons() {
        let mut input = RenderInputContext::from_keys([KeyCode::Space], [KeyCode::Space]);
        assert!(input.drain_actions().is_empty());
        assert!(!input.take_gameplay_key_press());
        input.merge_keys([KeyCode::Space], [KeyCode::Space]);
        assert!(input.drain_actions().is_empty());
        assert!(!input.take_gameplay_key_press());
    }

    #[test]
    fn minus_and_delete_share_multifire_action() {
        let modifiers = InputModifiers::default();
        assert_eq!(
            semantic_action_for_press(KeyCode::Minus, modifiers),
            semantic_action_for_press(KeyCode::Delete, modifiers)
        );
        let input = RenderInputContext::from_keys([KeyCode::Minus], [KeyCode::Minus]);
        assert_eq!(kinds(&input), [SemanticInputActionKind::ToggleMultifire]);
    }

    #[test]
    fn comma_period_and_slash_fire_bullets_bombs_and_repel_and_shift_period_mines() {
        use SemanticInputActionKind as Action;
        for shift in [false, true] {
            let modifiers = InputModifiers {
                shift,
                control: false,
            };
            assert_eq!(
                semantic_action_for_press(KeyCode::Comma, modifiers),
                Some(Action::PrimaryFireOrSpectate)
            );
            assert_eq!(
                semantic_action_for_press(KeyCode::Period, modifiers),
                Some(if shift {
                    Action::Mine
                } else {
                    Action::BombOrSpectate
                })
            );
            assert_eq!(
                semantic_action_for_press(KeyCode::Slash, modifiers),
                Some(Action::Repel)
            );
        }

        let mut input = RenderInputContext::from_keys(
            [KeyCode::Comma, KeyCode::Period, KeyCode::Slash],
            [KeyCode::Comma, KeyCode::Period, KeyCode::Slash],
        );
        assert_eq!(
            kinds(&input),
            [
                Action::PrimaryFireOrSpectate,
                Action::BombOrSpectate,
                Action::Repel
            ]
        );
        assert!(input.take_gameplay_key_press());
        assert!(input.has_pending_weapon_submission_action());
    }

    #[test]
    fn z_bombs_x_fires_bullets_and_shift_z_mines() {
        use SemanticInputActionKind as Action;
        for shift in [false, true] {
            let modifiers = InputModifiers {
                shift,
                control: false,
            };
            assert_eq!(
                semantic_action_for_press(KeyCode::X, modifiers),
                Some(Action::PrimaryFireOrSpectate)
            );
            assert_eq!(
                semantic_action_for_press(KeyCode::Z, modifiers),
                Some(if shift {
                    Action::Mine
                } else {
                    Action::BombOrSpectate
                })
            );
        }

        let mut input =
            RenderInputContext::from_keys([KeyCode::Z, KeyCode::X], [KeyCode::Z, KeyCode::X]);
        assert_eq!(
            kinds(&input),
            [Action::PrimaryFireOrSpectate, Action::BombOrSpectate]
        );
        assert!(input.take_gameplay_key_press());
        assert!(input.has_pending_weapon_submission_action());
    }

    #[test]
    fn both_layouts_coalesce_into_one_action_per_frame() {
        use SemanticInputActionKind as Action;
        let keys = [
            KeyCode::LeftControl,
            KeyCode::Comma,
            KeyCode::X,
            KeyCode::Z,
            KeyCode::Period,
            KeyCode::Q,
            KeyCode::Slash,
        ];
        let input = RenderInputContext::from_keys(keys, keys);
        assert_eq!(
            kinds(&input),
            [
                Action::PrimaryFireOrSpectate,
                Action::BombOrSpectate,
                Action::RepelOrMenuQuit,
            ]
        );

        let mines = [KeyCode::Tab, KeyCode::Z, KeyCode::Period, KeyCode::M];
        let input =
            RenderInputContext::from_keys(mines.into_iter().chain([KeyCode::LeftShift]), mines);
        assert_eq!(kinds(&input), [Action::Mine]);
    }

    #[test]
    fn zero_step_short_taps_keep_press_time_modifiers_and_fifo_order() {
        let mut input = RenderInputContext::default();
        input.merge_keys(
            [KeyCode::LeftShift, KeyCode::Home],
            [KeyCode::LeftShift, KeyCode::Home],
        );
        input.merge_keys([], []);
        input.merge_keys([KeyCode::LeftControl], [KeyCode::LeftControl]);
        input.merge_keys([], []);
        input.merge_keys([KeyCode::F3], [KeyCode::F3]);
        input.merge_keys([], []);

        let actions = input.drain_actions();
        assert_eq!(
            actions.iter().map(|action| action.kind).collect::<Vec<_>>(),
            [
                SemanticInputActionKind::ToggleCloak,
                SemanticInputActionKind::PrimaryFireOrSpectate,
                SemanticInputActionKind::Rocket,
            ]
        );
        assert!(actions[0].modifiers.shift);
        assert!(!actions[1].modifiers.shift);
        assert!(!actions[2].modifiers.shift);
    }

    #[test]
    fn later_shift_in_the_same_frame_does_not_relabel_an_earlier_control_tap() {
        use macroquad::miniquad::{EventHandler, KeyMods};
        let mut events = KeyPressModifiers::default();
        events.key_down_event(
            KeyCode::LeftControl,
            KeyMods {
                ctrl: true,
                ..KeyMods::default()
            },
            false,
        );
        events.key_up_event(KeyCode::LeftControl, KeyMods::default());
        events.key_down_event(KeyCode::Q, KeyMods::default(), false);
        events.key_up_event(KeyCode::Q, KeyMods::default());
        events.key_down_event(
            KeyCode::LeftShift,
            KeyMods {
                shift: true,
                ..KeyMods::default()
            },
            false,
        );
        events.key_down_event(
            KeyCode::Delete,
            KeyMods {
                shift: true,
                ..KeyMods::default()
            },
            false,
        );
        events.key_up_event(
            KeyCode::Delete,
            KeyMods {
                shift: true,
                ..KeyMods::default()
            },
        );

        // Reproduce the retained browser trace: all taps precede the draw,
        // with only the final chord's Shift still held when it samples input.
        let mut input = RenderInputContext::default();
        input.down.insert(KeyCode::LeftShift);
        input.capture_pressed_keys(
            [
                KeyCode::LeftControl,
                KeyCode::Q,
                KeyCode::LeftShift,
                KeyCode::Delete,
            ]
            .into(),
            &events.by_key,
        );
        let actions = input.drain_actions();
        assert_eq!(
            actions.iter().map(|action| action.kind).collect::<Vec<_>>(),
            [
                SemanticInputActionKind::PrimaryFireOrSpectate,
                SemanticInputActionKind::RepelOrMenuQuit,
                SemanticInputActionKind::Burst,
            ]
        );
        assert!(!actions[0].modifiers.shift);
        assert!(actions[0].modifiers.control);
        assert!(!actions[1].modifiers.shift);
        assert!(actions[2].modifiers.shift);
        assert!(
            input.drain_actions().is_empty(),
            "each physical tap drains once"
        );
    }

    #[test]
    fn released_shift_keeps_its_cloak_chord_modifiers_until_the_draw() {
        use macroquad::miniquad::{EventHandler, KeyMods};
        let mut events = KeyPressModifiers::default();
        let shifted = KeyMods {
            shift: true,
            ..KeyMods::default()
        };
        events.key_down_event(KeyCode::LeftShift, shifted, false);
        events.key_down_event(KeyCode::Home, shifted, false);
        events.key_up_event(KeyCode::Home, shifted);
        events.key_up_event(KeyCode::LeftShift, KeyMods::default());
        let mut input = RenderInputContext::default();
        input.capture_pressed_keys([KeyCode::LeftShift, KeyCode::Home].into(), &events.by_key);
        let actions = input.drain_actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, SemanticInputActionKind::ToggleCloak);
        assert!(actions[0].modifiers.shift);
        assert!(!input.is_down(KeyCode::LeftShift));
    }

    #[test]
    fn key_repeat_cannot_replace_the_first_press_modifier_snapshot() {
        use macroquad::miniquad::{EventHandler, KeyMods};
        let mut events = KeyPressModifiers::default();
        events.key_down_event(
            KeyCode::LeftControl,
            KeyMods {
                ctrl: true,
                ..KeyMods::default()
            },
            false,
        );
        events.key_down_event(
            KeyCode::LeftControl,
            KeyMods {
                ctrl: true,
                shift: true,
                ..KeyMods::default()
            },
            true,
        );
        let mut input = RenderInputContext::default();
        input
            .down
            .extend([KeyCode::LeftControl, KeyCode::LeftShift]);
        input.capture_pressed_keys([KeyCode::LeftControl].into(), &events.by_key);
        assert_eq!(
            kinds(&input),
            [SemanticInputActionKind::PrimaryFireOrSpectate]
        );
        assert!(!input.pending_actions()[0].modifiers.shift);
    }

    #[test]
    fn removed_reset_map_and_zone_keys_do_not_queue_actions() {
        let keys = [
            KeyCode::R,
            KeyCode::LeftBracket,
            KeyCode::RightBracket,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::F8,
        ];
        let mut input = RenderInputContext::from_keys(keys, keys);
        assert!(input.drain_actions().is_empty());
    }

    #[test]
    fn gameplay_key_press_survives_an_unready_batch_and_is_consumed_once() {
        let mut input = RenderInputContext::from_keys(
            [KeyCode::LeftControl, KeyCode::Right],
            [KeyCode::LeftControl, KeyCode::Right],
        );
        input.finish_gameplay_batch();

        assert!(input.take_gameplay_key_press());
        assert!(!input.take_gameplay_key_press());
    }

    #[test]
    fn every_modifier_pair_is_resolved_at_press_time() {
        let input = RenderInputContext::from_keys(
            [
                KeyCode::LeftShift,
                KeyCode::LeftControl,
                KeyCode::Delete,
                KeyCode::Home,
                KeyCode::End,
                KeyCode::Insert,
            ],
            [
                KeyCode::LeftControl,
                KeyCode::Delete,
                KeyCode::Home,
                KeyCode::End,
                KeyCode::Insert,
            ],
        );
        assert_eq!(
            kinds(&input),
            [
                SemanticInputActionKind::Repel,
                SemanticInputActionKind::Burst,
                SemanticInputActionKind::ToggleCloak,
                SemanticInputActionKind::ToggleAntiwarp,
                SemanticInputActionKind::PlacePortal,
            ]
        );
        assert!(
            input
                .pending_actions()
                .iter()
                .all(|action| action.modifiers.shift && action.modifiers.control)
        );
    }

    #[test]
    fn simultaneous_rocket_and_bomb_follow_reference_game_order() {
        let input =
            RenderInputContext::from_keys([KeyCode::F3, KeyCode::Z], [KeyCode::F3, KeyCode::Z]);
        assert_eq!(
            kinds(&input),
            [
                SemanticInputActionKind::Rocket,
                SemanticInputActionKind::BombOrSpectate,
            ]
        );
    }

    #[test]
    fn semantic_weapon_metrics_distinguish_primary_fire_from_bombs() {
        assert_eq!(
            semantic_input_metric_index(SemanticInputActionKind::PrimaryFireOrSpectate),
            Some(0)
        );
        assert_eq!(
            semantic_input_metric_index(SemanticInputActionKind::BombOrSpectate),
            Some(11)
        );
        assert_eq!(
            semantic_input_metric_index(SemanticInputActionKind::ChatAutocompleteOrBomb),
            Some(11)
        );
    }

    #[test]
    fn combat_view_receipt_actions_match_the_weapon_submission_boundary() {
        use SemanticInputActionKind as Action;

        for kind in [
            Action::PrimaryFireOrSpectate,
            Action::ChatAutocompleteOrBomb,
            Action::BombOrSpectate,
            Action::Mine,
            Action::Repel,
            Action::Burst,
            Action::Brick,
            Action::Decoy,
            Action::Thor,
        ] {
            assert!(semantic_action_requires_weapon_submission(kind), "{kind:?}");
        }
        for kind in [
            Action::Rocket,
            Action::Warp,
            Action::RandomWarp,
            Action::PlacePortal,
            Action::ToggleMultifire,
            Action::ToggleCloak,
            Action::Attach,
            Action::MenuFullscreen,
        ] {
            assert!(
                !semantic_action_requires_weapon_submission(kind),
                "{kind:?}"
            );
        }

        let weapon = RenderInputContext::from_keys([KeyCode::Q], [KeyCode::Q]);
        let lifecycle = RenderInputContext::from_keys([KeyCode::F3], [KeyCode::F3]);
        assert!(weapon.has_pending_weapon_submission_action());
        assert!(!lifecycle.has_pending_weapon_submission_action());
    }

    #[test]
    fn number_aliases_preserve_receipts_modifiers_and_rocket_order() {
        let input = RenderInputContext::from_keys(
            [KeyCode::Key2, KeyCode::F3, KeyCode::Key1],
            [KeyCode::Key2, KeyCode::F3, KeyCode::Key1],
        );
        assert_eq!(
            kinds(&input),
            [
                SemanticInputActionKind::NumberShortcut(2),
                SemanticInputActionKind::NumberShortcut(1)
            ]
        );
        assert!(input.has_pending_weapon_submission_action());
        let input =
            RenderInputContext::from_keys([KeyCode::LeftShift, KeyCode::Key0], [KeyCode::Key0]);
        assert!(input.pending_actions()[0].modifiers.shift);
        for number in 0..=9 {
            assert_eq!(
                semantic_action_requires_weapon_submission(
                    SemanticInputActionKind::NumberShortcut(number)
                ),
                [1, 3, 4, 5].contains(&number)
            );
        }
    }

    #[test]
    fn alias_keys_coalesce_within_one_frame_but_not_across_frames() {
        let mut input = RenderInputContext::from_keys(
            [KeyCode::LeftControl, KeyCode::RightControl],
            [KeyCode::LeftControl, KeyCode::RightControl],
        );
        assert_eq!(
            kinds(&input),
            [SemanticInputActionKind::PrimaryFireOrSpectate]
        );
        input.merge_keys([], []);
        input.merge_keys([KeyCode::LeftControl], [KeyCode::LeftControl]);
        assert_eq!(
            kinds(&input),
            [
                SemanticInputActionKind::PrimaryFireOrSpectate,
                SemanticInputActionKind::PrimaryFireOrSpectate,
            ]
        );
    }

    #[test]
    fn held_control_queues_one_semantic_edge_until_repressed() {
        let mut input =
            RenderInputContext::from_keys([KeyCode::LeftControl], [KeyCode::LeftControl]);
        input.merge_keys([KeyCode::LeftControl], []);
        input.merge_keys([KeyCode::LeftControl], []);
        assert_eq!(
            kinds(&input),
            [SemanticInputActionKind::PrimaryFireOrSpectate],
            "a physical Control hold queues one edge; held fire is evaluated separately"
        );

        input.merge_keys([], []);
        input.merge_keys([KeyCode::LeftControl], [KeyCode::LeftControl]);
        assert_eq!(
            kinds(&input),
            [
                SemanticInputActionKind::PrimaryFireOrSpectate,
                SemanticInputActionKind::PrimaryFireOrSpectate,
            ],
            "a genuine release and repress creates the next one-shot edge"
        );
    }

    #[test]
    fn queue_bound_preserves_oldest_order_and_counts_newest_drops() {
        let mut input = RenderInputContext::default();
        for index in 0..(MAX_PENDING_SEMANTIC_ACTIONS + 7) {
            let key = if index % 2 == 0 {
                KeyCode::F3
            } else {
                KeyCode::F4
            };
            input.merge_keys([key], [key]);
            input.merge_keys([], []);
        }
        assert_eq!(input.pending_actions().len(), MAX_PENDING_SEMANTIC_ACTIONS);
        assert_eq!(input.dropped_actions(), 7);
        assert_eq!(
            input.pending_actions()[0].kind,
            SemanticInputActionKind::Rocket
        );
        assert_eq!(
            input.pending_actions()[MAX_PENDING_SEMANTIC_ACTIONS - 1].kind,
            SemanticInputActionKind::Brick
        );
    }

    #[test]
    fn batch_and_session_clear_retire_all_pending_actions() {
        let mut input = RenderInputContext::from_keys([], [KeyCode::Escape, KeyCode::F3]);
        input.finish_gameplay_batch();
        assert!(input.pending_actions().is_empty());
        input.merge_keys([KeyCode::F4], [KeyCode::F4]);
        input.clear_pending_actions();
        assert!(input.pending_actions().is_empty());
    }
}

#[cfg(test)]
mod mouse_tests {
    use super::*;

    #[test]
    fn mouse_taps_survive_zero_step_frames_and_request_weapon_receipts() {
        let mut input = RenderInputContext::from_mouse([], [MouseButton::Left, MouseButton::Right]);
        assert!(input.has_pending_weapon_submission_action());
        assert!(input.take_gameplay_key_press());
        let next_frame = input.pending_actions.len();
        input.capture_mouse_buttons([], [], next_frame);
        assert_eq!(
            input
                .drain_actions()
                .iter()
                .map(|a| a.kind)
                .collect::<Vec<_>>(),
            [
                SemanticInputActionKind::PrimaryFireOrSpectate,
                SemanticInputActionKind::BombOrSpectate
            ]
        );
        assert!(!input.mouse_is_down(MouseButton::Left));
        assert!(input.drain_actions().is_empty());
    }

    #[test]
    fn mouse_aliases_coalesce_with_keyboard_only_within_the_same_frame() {
        let mut input = RenderInputContext::from_keys([], [KeyCode::Comma, KeyCode::Tab]);
        input.capture_mouse_buttons([], [MouseButton::Left, MouseButton::Right], 0);
        assert_eq!(input.pending_actions.len(), 2);
        let next_frame = input.pending_actions.len();
        input.capture_mouse_buttons([], [MouseButton::Left], next_frame);
        assert_eq!(
            input.pending_actions.len(),
            3,
            "another frame's tap remains distinct"
        );
    }

    #[test]
    fn mouse_queue_is_bounded_and_release_clears_held_state() {
        let mut input = RenderInputContext::default();
        for _ in 0..MAX_PENDING_SEMANTIC_ACTIONS + 1 {
            let start = input.pending_actions.len();
            input.capture_mouse_buttons([MouseButton::Left], [MouseButton::Left], start);
        }
        assert_eq!(input.pending_actions.len(), MAX_PENDING_SEMANTIC_ACTIONS);
        assert_eq!(input.dropped_actions(), 1);
        assert!(input.mouse_is_down(MouseButton::Left));
        let start = input.pending_actions.len();
        input.capture_mouse_buttons([], [], start);
        assert!(!input.mouse_is_down(MouseButton::Left));
    }
}

#[cfg(test)]
mod chat_edit_tests {
    use super::*;
    use macroquad::miniquad::{EventHandler, KeyMods};

    #[test]
    fn chat_accepts_backspace_delete_and_native_key_repeat_without_gameplay_actions() {
        for key in [KeyCode::Backspace, KeyCode::Delete] {
            let mut events = KeyPressModifiers::default();
            events.key_down_event(key, KeyMods::default(), false);
            events.key_down_event(key, KeyMods::default(), true);
            events.key_down_event(key, KeyMods::default(), true);
            let mut input = RenderInputContext::default();
            input.capture_chat_frame_keys([key].into_iter().collect(), &events, true);
            let actions = input.drain_actions();
            assert_eq!(actions.len(), 3);
            assert!(
                actions
                    .iter()
                    .all(|a| a.kind == SemanticInputActionKind::ChatBackspace)
            );
            input.capture_chat_frame_keys(HashSet::new(), &KeyPressModifiers::default(), true);
            assert!(input.drain_actions().is_empty());
        }
        for shift in [false, true] {
            let mut input = RenderInputContext::default();
            let mut events = KeyPressModifiers::default();
            events.key_down_event(
                KeyCode::Delete,
                KeyMods {
                    shift,
                    ..KeyMods::default()
                },
                false,
            );
            input.capture_chat_frame_keys([KeyCode::Delete].into_iter().collect(), &events, false);
            assert_eq!(
                input.drain_actions()[0].kind,
                if shift {
                    SemanticInputActionKind::Burst
                } else {
                    SemanticInputActionKind::ToggleMultifire
                }
            );
        }
    }
}
