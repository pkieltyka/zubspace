use super::*;

impl Game {
    pub(in crate::game) fn toggle_multifire(&mut self) {
        if !self.ship.multifire_capable {
            return;
        }

        self.ship.multifire = !self.ship.multifire;
        let sound = if self.ship.multifire {
            self.assets.multifire_on.as_ref()
        } else {
            self.assets.multifire_off.as_ref()
        };

        if let Some(sound) = sound {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.34),
                },
            );
        }
    }

    pub(in crate::game) fn toggle_stealth(&mut self) {
        if !self.ship.stealth_capable {
            return;
        }
        self.ship.stealth = !self.ship.stealth;

        if let Some(sound) = status_toggle_sound(
            self.ship.stealth,
            self.assets.stealth_sound.as_ref(),
            self.assets.toggle_off_sound.as_ref(),
        ) {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.32),
                },
            );
        }
    }

    pub(in crate::game) fn toggle_cloak(&mut self) {
        if !self.ship.cloak_capable {
            return;
        }
        let was_cloaked = self.ship.cloak;
        self.ship.cloak = !self.ship.cloak;
        if cloak_toggle_sets_flash(was_cloaked, self.ship.cloak) {
            self.ship.flash = true;
        }

        if let Some(sound) = status_toggle_sound(
            self.ship.cloak,
            self.assets.cloak_sound.as_ref(),
            self.assets.toggle_off_sound.as_ref(),
        ) {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.32),
                },
            );
        }
    }

    pub(in crate::game) fn toggle_xradar(&mut self) {
        if !self.ship.xradar_capable {
            return;
        }
        self.ship.xradar = !self.ship.xradar;

        if let Some(sound) = status_toggle_sound(
            self.ship.xradar,
            self.assets.xradar_sound.as_ref(),
            self.assets.toggle_off_sound.as_ref(),
        ) {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.32),
                },
            );
        }
    }

    pub(in crate::game) fn toggle_antiwarp(&mut self) {
        if !self.ship.antiwarp_capable {
            return;
        }
        self.ship.antiwarp = !self.ship.antiwarp;

        if let Some(sound) = status_toggle_sound(
            self.ship.antiwarp,
            self.assets.antiwarp_sound.as_ref(),
            self.assets.toggle_off_sound.as_ref(),
        ) {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.32),
                },
            );
        }
    }
}

fn status_toggle_sound<'a>(
    enabled: bool,
    on_sound: Option<&'a Sound>,
    off_sound: Option<&'a Sound>,
) -> Option<&'a Sound> {
    if status_toggle_uses_off_sound(!enabled, enabled) {
        off_sound
    } else {
        on_sound
    }
}
