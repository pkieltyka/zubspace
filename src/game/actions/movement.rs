use super::*;

impl Game {
    /// The reference game's `PlayerManager::Update`: integrate/collide using velocity left
    /// by the previous frame. `ShipController::Update` applies current input
    /// and its speed clamp only after this returns.
    pub(in crate::game) fn move_ship_before_current_input(&mut self, dt: f32) {
        let frequency = self.player_frequency();
        let passable_bricks: Vec<_> = self
            .bricks
            .iter()
            .filter(|brick| brick.team == frequency)
            .map(|brick| brick.tile)
            .collect();
        let bounced = {
            let map = &self.maps[self.map_index];
            let radius = self.ship_tuning().radius_pixels;
            move_entity_with_collision_and_passable_tiles(
                map,
                &mut self.ship.pos,
                &mut self.ship.vel,
                radius,
                dt,
                self.tuning.bounce_factor,
                &passable_bricks,
            )
        };

        if bounced && self.ship.vel.length_squared() >= ship_bounce_sound_threshold_squared() {
            if let Some(sound) = self.assets.bounce_sound.as_ref() {
                play_sound(
                    sound,
                    PlaySoundParams {
                        looped: false,
                        volume: self.effect_volume(0.24),
                    },
                );
            }
        }
    }
}
