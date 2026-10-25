//! Canonical player-input record.
//!
//! This is the input boundary between the human (or bot) decision layer
//! and the deterministic physics step. It carries the held movement bits
//! plus the Rocket pulse and safe-zone weapon stop; weapons travel in the
//! position packet's weapon payload (see `docs/authoritative_server.md` §4).
//!
//! Both client (predictive) and server (authoritative) physics steps
//! consume this struct. Keeping the bit layout stable — and the
//! enum-of-fields short — is what makes the wire format tractable
//! later.

use super::*;

/// Compact intent-record describing what a player wants to do this
/// tick. Held-key style — `thrust` means "the thrust key is currently
/// down," not "fire one thrust pulse." Edge-triggered actions (toggle
/// stealth, fire one bullet) belong in a separate `PlayerAction` event
/// stream and are not part of this struct.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlayerInput {
    pub(crate) thrust: bool,
    pub(crate) reverse: bool,
    pub(crate) rotate_left: bool,
    pub(crate) rotate_right: bool,
    /// Raw afterburner-modifier key state (shift on the default
    /// keymap). Whether the player is *actually* afterburning depends
    /// on this bit AND `(thrust || reverse)` AND sufficient energy —
    /// that decision is made downstream in the physics step.
    pub(crate) afterburner_held: bool,
    /// One-record Rocket activation pulse. This is an edge assertion, not a
    /// held active-state bit: the server owns the resulting timer and derives
    /// the Rocket movement interval from the accepted pulse sequence.
    pub(crate) rocket_activation_pulse: bool,
    /// One-record proof that Continuum's local safe-zone weapon path replaced
    /// velocity with zero after this movement step. Safe-zone weapons do not
    /// publish a projectile, so this bit is the exact replay evidence needed
    /// when the position for this record is lost and a later tail arrives.
    pub(crate) safe_zone_weapon_stop: bool,
}

#[allow(dead_code)] // wire-format helpers consumed in Phase 1b.
impl PlayerInput {
    pub(crate) const BIT_THRUST: u16 = 1 << 0;
    pub(crate) const BIT_REVERSE: u16 = 1 << 1;
    pub(crate) const BIT_ROTATE_LEFT: u16 = 1 << 2;
    pub(crate) const BIT_ROTATE_RIGHT: u16 = 1 << 3;
    pub(crate) const BIT_AFTERBURNER_HELD: u16 = 1 << 4;
    pub(crate) const BIT_ROCKET_ACTIVATION_PULSE: u16 =
        crate::gameplay_rules::ROCKET_ACTIVATION_PULSE_BIT;
    pub(crate) const BIT_SAFE_ZONE_WEAPON_STOP: u16 =
        crate::gameplay_rules::SAFE_ZONE_WEAPON_STOP_BIT;

    pub(crate) fn bits(self) -> u16 {
        let mut bits = 0;
        if self.thrust {
            bits |= Self::BIT_THRUST;
        }
        if self.reverse {
            bits |= Self::BIT_REVERSE;
        }
        if self.rotate_left {
            bits |= Self::BIT_ROTATE_LEFT;
        }
        if self.rotate_right {
            bits |= Self::BIT_ROTATE_RIGHT;
        }
        if self.afterburner_held {
            bits |= Self::BIT_AFTERBURNER_HELD;
        }
        if self.rocket_activation_pulse {
            bits |= Self::BIT_ROCKET_ACTIVATION_PULSE;
        }
        if self.safe_zone_weapon_stop {
            bits |= Self::BIT_SAFE_ZONE_WEAPON_STOP;
        }
        bits
    }

    pub(crate) fn from_bits(bits: u16) -> Self {
        Self {
            thrust: bits & Self::BIT_THRUST != 0,
            reverse: bits & Self::BIT_REVERSE != 0,
            rotate_left: bits & Self::BIT_ROTATE_LEFT != 0,
            rotate_right: bits & Self::BIT_ROTATE_RIGHT != 0,
            afterburner_held: bits & Self::BIT_AFTERBURNER_HELD != 0,
            rocket_activation_pulse: bits & Self::BIT_ROCKET_ACTIVATION_PULSE != 0,
            safe_zone_weapon_stop: bits & Self::BIT_SAFE_ZONE_WEAPON_STOP != 0,
        }
    }
}

/// Read the local keyboard state into a canonical `PlayerInput`. When
/// `enabled` is false (e.g. chat is open) returns the all-zero default
/// so the physics step sees no motion intent. Only the client binary
/// links macroquad's input layer and calls this; the server consumes
/// `PlayerInput` records arriving over the wire (Phase 1b).
pub(crate) fn read_local_player_input(
    enabled: bool,
    render_input: &crate::input::RenderInputContext,
) -> PlayerInput {
    if !enabled {
        return PlayerInput::default();
    }
    PlayerInput {
        thrust: render_input.is_down(KeyCode::Up) || render_input.is_down(KeyCode::W),
        reverse: render_input.is_down(KeyCode::Down) || render_input.is_down(KeyCode::S),
        rotate_left: render_input.is_down(KeyCode::Left) || render_input.is_down(KeyCode::A),
        rotate_right: render_input.is_down(KeyCode::Right) || render_input.is_down(KeyCode::D),
        afterburner_held: render_input.is_down(KeyCode::LeftShift)
            || render_input.is_down(KeyCode::RightShift),
        rocket_activation_pulse: false,
        safe_zone_weapon_stop: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fire_turn_thrust_shift_held_set_transition_samples_exactly() {
        use crate::input::RenderInputContext;
        // Independent physical-key table: Control is a semantic weapon key,
        // not a movement bit. The other four bits must survive any combination
        // of simultaneous presses/releases, including opposing turn keys.
        const KEYS: [KeyCode; 5] = [
            KeyCode::LeftControl,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::LeftShift,
        ];
        const MOVEMENT_BITS: [u16; 5] = [0, 4, 8, 1, 16];
        let keys = |mask: u8| {
            KEYS.iter()
                .copied()
                .enumerate()
                .filter_map(move |(index, key)| (mask & (1 << index) != 0).then_some(key))
        };
        let expected = |mask: u8| {
            MOVEMENT_BITS
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .fold(0, |bits, (_, bit)| bits | bit)
        };
        for before in 0u8..32 {
            for after in 0u8..32 {
                let mut context = RenderInputContext::from_keys(keys(before), keys(before));
                assert_eq!(
                    read_local_player_input(true, &context).bits(),
                    expected(before)
                );
                context.merge_keys(keys(after), keys(after & !before));
                for _ in 0..3 {
                    assert_eq!(
                        read_local_player_input(true, &context).bits(),
                        expected(after),
                        "held transition {before:05b}->{after:05b}"
                    );
                    assert_eq!(context.is_down(KeyCode::LeftControl), after & 1 != 0);
                    assert_eq!(
                        read_local_player_input(false, &context).bits(),
                        0,
                        "chat suppression must not mutate held state"
                    );
                    context.finish_gameplay_batch();
                }
                context.clear_pending_actions();
                assert_eq!(
                    read_local_player_input(true, &context).bits(),
                    expected(after)
                );
                context.merge_keys([], []);
                assert_eq!(read_local_player_input(true, &context).bits(), 0);
                assert!(!context.is_down(KeyCode::LeftControl));
            }
        }
    }

    #[test]
    fn bit_round_trip_preserves_every_field() {
        for bits in 0u16..=0x7f {
            let input = PlayerInput::from_bits(bits);
            assert_eq!(input.bits(), bits, "round trip failed for {bits:05b}");
        }
    }

    #[test]
    fn default_is_all_false() {
        let input = PlayerInput::default();
        assert_eq!(input.bits(), 0);
    }

    #[test]
    fn unknown_high_bits_are_ignored() {
        // Future phases will populate bits 5-15. For now, a packet that
        // sets them must round-trip the movement bits correctly.
        let input = PlayerInput::from_bits(0xffff);
        assert!(input.thrust);
        assert!(input.reverse);
        assert!(input.rotate_left);
        assert!(input.rotate_right);
        assert!(input.afterburner_held);
        assert!(input.rocket_activation_pulse);
        assert!(input.safe_zone_weapon_stop);
        // Re-encoded value drops the still-reserved high bits.
        assert_eq!(input.bits(), 0x7f);
    }

    #[test]
    fn bit_constants_are_distinct_and_packed_low() {
        let bits = [
            PlayerInput::BIT_THRUST,
            PlayerInput::BIT_REVERSE,
            PlayerInput::BIT_ROTATE_LEFT,
            PlayerInput::BIT_ROTATE_RIGHT,
            PlayerInput::BIT_AFTERBURNER_HELD,
            PlayerInput::BIT_ROCKET_ACTIVATION_PULSE,
            PlayerInput::BIT_SAFE_ZONE_WEAPON_STOP,
        ];
        // Each constant is a single distinct bit.
        for &b in &bits {
            assert_eq!(b.count_ones(), 1);
        }
        // Constants don't overlap.
        let or: u16 = bits.iter().copied().reduce(|a, b| a | b).unwrap();
        assert_eq!(or.count_ones(), bits.len() as u32);
    }
}
