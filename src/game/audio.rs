use super::*;

const ENERGY_ALARM_INTERVAL: f32 = 1.2;

// The original PCM ignition is32751 mono frames at22050Hz. Sustain is the
// distinct rocket2 asset, never a repeat of ignition. Pin this duration to the
// embedded WAV in the focused asset test below.
const ROCKET_IGNITION_SECONDS: f64 = 32751.0 / 22050.0;

#[derive(Default)]
pub(in crate::game) struct RocketAudioLoop {
    volume: Option<f32>,
    sustain_at: Option<f64>,
    sustaining: bool,
}

#[derive(Debug, PartialEq)]
enum RocketAudioAction {
    StartIgnition(f32),
    StartSustain(f32),
    SetVolume(f32),
    Stop,
}

impl RocketAudioLoop {
    fn update(&mut self, desired_volume: Option<f32>, now: f64) -> Option<RocketAudioAction> {
        let previous = self.volume;
        self.volume = desired_volume;
        match (previous, desired_volume) {
            (None, Some(volume)) => {
                self.sustain_at = Some(now + ROCKET_IGNITION_SECONDS);
                self.sustaining = false;
                Some(RocketAudioAction::StartIgnition(volume))
            }
            (Some(_), None) => {
                self.sustain_at = None;
                self.sustaining = false;
                Some(RocketAudioAction::Stop)
            }
            (Some(_), Some(volume))
                if !self.sustaining && self.sustain_at.is_some_and(|at| now >= at) =>
            {
                self.sustaining = true;
                Some(RocketAudioAction::StartSustain(volume))
            }
            (Some(before), Some(after)) if before != after => {
                Some(RocketAudioAction::SetVolume(after))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::game) struct AudioVolumes {
    effects: f32,
    chat: f32,
    voice: f32,
    music: f32,
}

impl Default for AudioVolumes {
    fn default() -> Self {
        Self {
            effects: 1.0,
            chat: 1.0,
            voice: 1.0,
            music: 1.0,
        }
    }
}

pub(crate) fn audio_volume_from_command_value(value: u8) -> f32 {
    value.min(10) as f32 / 10.0
}

pub(crate) fn audio_volume_command_notification(
    category: AudioVolumeCategory,
    value: u8,
) -> String {
    let label = match category {
        AudioVolumeCategory::Effects => "Sound",
        AudioVolumeCategory::Chat => "Chat sound",
        AudioVolumeCategory::Voice => "Voice",
        AudioVolumeCategory::Music => "Music",
    };
    format!("{label} volume set to {}.", value.min(10))
}

impl Game {
    fn rocket_audio_volume(&self) -> Option<f32> {
        // The reconciled effect timer is the source of truth. Ordinary input
        // or movement-correction barriers must not restart an active loop.
        // A retired/terminal session is different: its timer can be frozen
        // while a replacement transport waits for its own baseline.
        let live_session = !self.networked_play
            || (self.client_authority.view().scope.is_complete_live()
                && !crate::transport::browser_rtc_transport_is_terminal());
        let active = live_session
            && self.ship.rocket_timer > 0.0
            && self.player_respawn_timer <= 0.0
            && !self.spectator_mode
            && !self.main_menu_requested;
        active.then(|| self.effect_volume(0.36))
    }

    pub(crate) fn update_rocket_audio(&mut self) {
        let desired_volume = self.rocket_audio_volume();
        let Some(action) = self.rocket_audio.update(desired_volume, get_time()) else {
            return;
        };
        let ignition = self.assets.rocket_sound.as_ref();
        let sustain = self.assets.rocket_sustain_sound.as_ref();
        match action {
            RocketAudioAction::StartIgnition(volume) => {
                if let Some(sound) = ignition {
                    play_sound(
                        sound,
                        PlaySoundParams {
                            looped: false,
                            volume,
                        },
                    );
                }
            }
            RocketAudioAction::StartSustain(volume) => {
                if let Some(sound) = ignition {
                    stop_sound(sound);
                }
                if let Some(sound) = sustain {
                    play_sound(
                        sound,
                        PlaySoundParams {
                            looped: true,
                            volume,
                        },
                    );
                }
            }
            RocketAudioAction::SetVolume(volume) => {
                for sound in [ignition, sustain].into_iter().flatten() {
                    set_sound_volume(sound, volume);
                }
            }
            RocketAudioAction::Stop => {
                for sound in [ignition, sustain].into_iter().flatten() {
                    stop_sound(sound);
                }
            }
        }
    }

    pub(in crate::game) fn audio_listener_position(&self) -> Vec2 {
        if self.spectator_mode {
            self.camera.target
        } else {
            self.ship.pos
        }
    }

    pub(in crate::game) fn positional_sound_volume(
        &self,
        source_pos: Vec2,
        base_volume: f32,
    ) -> Option<f32> {
        positional_sound_volume(
            self.audio_listener_position(),
            source_pos,
            positional_sound_radius_px(screen_width(), screen_height()),
            base_volume,
        )
        .map(|volume| self.effect_volume(volume))
    }

    pub(in crate::game) fn effect_volume(&self, base_volume: f32) -> f32 {
        (base_volume * self.audio_volumes.effects).clamp(0.0, 1.0)
    }

    pub(in crate::game) fn chat_volume(&self, base_volume: f32) -> f32 {
        (base_volume * self.audio_volumes.chat).clamp(0.0, 1.0)
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(in crate::game) fn voice_volume(&self, base_volume: f32) -> f32 {
        (base_volume * self.audio_volumes.voice).clamp(0.0, 1.0)
    }

    pub(in crate::game) fn music_volume(&self, base_volume: f32) -> f32 {
        (base_volume * self.audio_volumes.music).clamp(0.0, 1.0)
    }

    pub(in crate::game) fn apply_audio_volume_command(
        &mut self,
        category: AudioVolumeCategory,
        value: u8,
    ) -> String {
        let volume = audio_volume_from_command_value(value);
        match category {
            AudioVolumeCategory::Effects => self.audio_volumes.effects = volume,
            AudioVolumeCategory::Chat => self.audio_volumes.chat = volume,
            AudioVolumeCategory::Voice => self.audio_volumes.voice = volume,
            AudioVolumeCategory::Music => self.audio_volumes.music = volume,
        }
        audio_volume_command_notification(category, value)
    }

    pub(in crate::game) fn update_energy_alarm(&mut self, dt: f32) {
        if !energy_alarm_active(
            self.ship.energy,
            self.ship.max_energy,
            self.spectator_mode,
            self.player_respawn_timer,
        ) {
            self.energy_alarm_timer = 0.0;
            return;
        }

        self.energy_alarm_timer -= dt;
        if self.energy_alarm_timer > 0.0 {
            return;
        }

        if let Some(sound) = self.assets.energy_alarm_sound.as_ref() {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.22),
                },
            );
        }
        self.energy_alarm_timer = ENERGY_ALARM_INTERVAL;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rocket_audio_ignites_once_then_sustains_without_confirmation_restarts() {
        let mut audio = RocketAudioLoop::default();
        assert_eq!(
            audio.update(Some(0.36), 10.0),
            Some(RocketAudioAction::StartIgnition(0.36))
        );
        for i in 1..100 {
            assert_eq!(audio.update(Some(0.36), 10.0 + i as f64 * 0.01), None);
        }
        assert_eq!(
            audio.update(Some(0.18), 11.0),
            Some(RocketAudioAction::SetVolume(0.18))
        );
        assert_eq!(
            audio.update(Some(0.18), 10.0 + ROCKET_IGNITION_SECONDS - 0.001),
            None
        );
        assert_eq!(
            audio.update(Some(0.18), 10.0 + ROCKET_IGNITION_SECONDS),
            Some(RocketAudioAction::StartSustain(0.18))
        );
        for i in 1..100 {
            assert_eq!(audio.update(Some(0.18), 12.0 + i as f64), None);
        }
        assert_eq!(
            audio.update(Some(0.0), 112.0),
            Some(RocketAudioAction::SetVolume(0.0))
        );
        assert_eq!(
            audio.update(Some(0.36), 113.0),
            Some(RocketAudioAction::SetVolume(0.36))
        );
        assert_eq!(audio.update(None, 114.0), Some(RocketAudioAction::Stop));
        assert_eq!(audio.update(None, 115.0), None);
        assert_eq!(
            audio.update(Some(0.36), 116.0),
            Some(RocketAudioAction::StartIgnition(0.36))
        );
    }

    #[test]
    fn rocket_audio_cancellation_during_ignition_cannot_start_a_late_sustain() {
        let mut audio = RocketAudioLoop::default();
        assert_eq!(
            audio.update(Some(0.36), 1.0),
            Some(RocketAudioAction::StartIgnition(0.36))
        );
        assert_eq!(audio.update(None, 1.1), Some(RocketAudioAction::Stop));
        assert_eq!(audio.update(None, 10.0), None);
        assert_eq!(
            audio.update(Some(0.36), 10.0),
            Some(RocketAudioAction::StartIgnition(0.36))
        );
        assert_eq!(
            audio.update(Some(0.36), 10.0 + ROCKET_IGNITION_SECONDS),
            Some(RocketAudioAction::StartSustain(0.36))
        );
    }

    #[test]
    fn rocket_ignition_deadline_matches_the_original_pcm_asset() {
        let wav = include_bytes!("../../assets/sound/rocket1.wa2");
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let mut offset = 12;
        let mut bytes_per_second = None;
        let mut payload_bytes = None;
        while offset + 8 <= wav.len() {
            let len = u32::from_le_bytes(wav[offset + 4..offset + 8].try_into().unwrap()) as usize;
            if &wav[offset..offset + 4] == b"fmt " {
                assert_eq!(
                    u16::from_le_bytes(wav[offset + 8..offset + 10].try_into().unwrap()),
                    1
                );
                bytes_per_second = Some(u32::from_le_bytes(
                    wav[offset + 16..offset + 20].try_into().unwrap(),
                ));
            }
            if &wav[offset..offset + 4] == b"data" {
                payload_bytes = Some(len);
            }
            offset += 8 + len + (len & 1);
        }
        assert_eq!(
            payload_bytes.unwrap() as f64 / bytes_per_second.unwrap() as f64,
            ROCKET_IGNITION_SECONDS
        );
    }

    #[test]
    fn rocket_audio_stops_for_expiry_death_spectator_and_main_menu() {
        let mut game = Game::new_network_authority_test();
        game.ship.rocket_timer = 1.0;
        assert_eq!(game.rocket_audio_volume(), Some(0.36));
        game.ship.rocket_timer = 0.0;
        assert_eq!(game.rocket_audio_volume(), None);
        game.ship.rocket_timer = 1.0;
        game.player_respawn_timer = 2.0;
        assert_eq!(game.rocket_audio_volume(), None);
        game.player_respawn_timer = 0.0;
        game.spectator_mode = true;
        assert_eq!(game.rocket_audio_volume(), None);
        game.spectator_mode = false;
        game.main_menu_requested = true;
        assert_eq!(game.rocket_audio_volume(), None);
        game.main_menu_requested = false;
        assert_eq!(game.rocket_audio_volume(), Some(0.36));
        game.ship.rocket_timer = 0.1; // Shortening an active timer keeps the voice.
        assert_eq!(game.rocket_audio_volume(), Some(0.36));
    }

    #[test]
    fn rocket_audio_retires_an_old_network_scope_without_waiting_for_rejoin() {
        let mut game = Game::new_network_authority_test();
        game.ship.rocket_timer = 10.0;
        game.networked_play = true;
        // Transport recovery clears authority before the replacement baseline
        // arrives. The old gameplay timer may remain frozen during that wait.
        assert_eq!(game.rocket_audio_volume(), None);
        let scope = AuthorityScopeIdentity {
            server_session_id: Some(77),
            arena_stream_id: Some(88),
            player_id: Some(9),
            life_seq: Some(3),
            alive: Some(true),
        };
        game.client_authority
            .install_test_scope(1, scope, Default::default());
        // Presentation/input barriers must not toggle an existing voice; the
        // immutable live life remains the same even when readiness is closed.
        assert_eq!(game.rocket_audio_volume(), Some(0.36));
        game.client_authority.install_test_scope(
            1,
            AuthorityScopeIdentity {
                alive: Some(false),
                ..scope
            },
            Default::default(),
        );
        assert_eq!(game.rocket_audio_volume(), None);
        game.client_authority = Default::default();
        assert_eq!(game.rocket_audio_volume(), None);
        assert_eq!(game.ship.rocket_timer, 10.0, "audio cannot mutate gameplay");
    }

    #[test]
    fn spectator_audio_listens_at_the_presented_camera_without_moving_the_ship() {
        let mut game = Game::new_network_authority_test();
        game.ship.pos = vec2(100.0, 200.0);
        game.camera.target = vec2(300.0, 400.0);
        assert_eq!(game.audio_listener_position(), game.ship.pos);
        game.spectator_mode = true;
        assert_eq!(game.audio_listener_position(), game.camera.target);
        assert_eq!(game.ship.pos, vec2(100.0, 200.0));
        game.camera.target = vec2(500.0, 600.0);
        assert_eq!(game.audio_listener_position(), vec2(500.0, 600.0));
        game.spectator_mode = false;
        assert_eq!(game.audio_listener_position(), game.ship.pos);
    }

    #[test]
    fn audio_volume_command_values_map_to_tenths() {
        assert_eq!(audio_volume_from_command_value(0), 0.0);
        assert_eq!(audio_volume_from_command_value(5), 0.5);
        assert_eq!(audio_volume_from_command_value(10), 1.0);
        assert_eq!(audio_volume_from_command_value(99), 1.0);
    }

    #[test]
    fn audio_volume_command_notifications_name_categories() {
        assert_eq!(
            audio_volume_command_notification(AudioVolumeCategory::Effects, 7),
            "Sound volume set to 7."
        );
        assert_eq!(
            audio_volume_command_notification(AudioVolumeCategory::Chat, 3),
            "Chat sound volume set to 3."
        );
        assert_eq!(
            audio_volume_command_notification(AudioVolumeCategory::Voice, 0),
            "Voice volume set to 0."
        );
        assert_eq!(
            audio_volume_command_notification(AudioVolumeCategory::Music, 10),
            "Music volume set to 10."
        );
    }
}
