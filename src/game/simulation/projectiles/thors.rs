use super::*;

impl Game {
    pub(in crate::game) fn update_thors(&mut self, dt: f32) {
        let mut explosions = Vec::new();
        let map = &self.maps[self.map_index];
        let local_frequency = self.player_frequency();
        let bots = &self.bots;
        let network_world = &self.network_world;
        let tuning = self.tuning;
        for thor in &mut self.thors {
            if let Some(explosion) =
                update_thor_motion(thor, dt, map, bots, network_world, local_frequency, tuning)
            {
                explosions.push((explosion, thor.raw_level, thor.prediction));
            }
        }
        self.thors.retain(|thor| thor.ttl > 0.0);

        for ((pos, damage, shrapnel), raw_level, prediction) in explosions {
            // The reference game sizes Thor explosion as `BombExplodePixels * (4 + raw_level)`.
            // bomb_explosion_radius multiplies by clamp(level, 1), so the
            // bomb_level argument is `4 + raw_level`.
            let level = WEAPON_LEVEL_MAX.saturating_add(raw_level);
            self.add_predicted_explosion(pos, pos, level, damage, 0.0, shrapnel, prediction);
        }
    }
}
