use super::*;

impl Game {
    pub(in crate::game) fn spawn_prize_near_ship(&mut self, max_prizes: usize) {
        if self.tuning.prize_minimum_virtual > 0 || self.tuning.prize_upgrade_virtual > 0 {
            if let Some(pos) = continuum_prize_spawn_position(
                self.map().width,
                self.map().height,
                self.tuning.prize_minimum_virtual,
                self.tuning.prize_upgrade_virtual,
                self.prize_player_count(),
                &mut self.prize_rng_seed,
            ) {
                self.spawn_prize_at(pos, max_prizes);
            }
            return;
        }

        if let Some(pos) = self
            .map()
            .random_open_position_near(self.ship.pos, 5, 24, 48)
        {
            self.spawn_prize_at(pos, max_prizes);
        }
    }

    pub(in crate::game) fn max_hidden_prizes(&self) -> usize {
        max_hidden_prize_count(self.tuning.prize_factor, self.prize_player_count())
    }

    fn prize_player_count(&self) -> usize {
        1 + self
            .bots
            .iter()
            .filter(|bot| bot.respawn_timer <= 0.0)
            .count()
            + self.network_world.visible_players().count()
    }

    pub(in crate::game) fn spawn_prize_at(&mut self, pos: Vec2, max_prizes: usize) {
        if !can_spawn_prize_at(self.map(), self.prizes.len(), max_prizes, pos) {
            return;
        }

        let (kind, negative) = generate_weighted_prize(
            &self.tuning.prize_weights,
            self.tuning.prize_weight_total,
            self.tuning.prize_negative_factor,
            true,
            &mut self.prize_rng_seed,
        )
        .unwrap_or_else(|| (PrizeKind::random(), false));
        let ttl = prize_lifetime_seconds(
            self.tuning.prize_lifetime_min,
            self.tuning.prize_lifetime_max,
            &mut self.prize_rng_seed,
        );

        self.prizes.push(PrizePickup {
            collectible_id: 0,
            pos,
            kind,
            negative,
            age: 0.0,
            ttl,
            claim_retry_timer: 0.0,
        });
    }

    pub(in crate::game) fn spawn_death_prize_at(
        &mut self,
        collectible_id: u32,
        pos: Vec2,
        kind: PrizeKind,
    ) {
        if collectible_id == 0 {
            return;
        }
        if self.prizes.len() >= CONTINUUM_MAX_GREEN_COUNT {
            return;
        }

        self.prizes.push(PrizePickup {
            collectible_id,
            pos,
            kind,
            negative: false,
            age: 0.0,
            ttl: self.tuning.death_prize_lifetime,
            claim_retry_timer: 0.0,
        });
    }
}
