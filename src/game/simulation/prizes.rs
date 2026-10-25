use super::*;

const NETWORK_PRIZE_CLAIM_RETRY_SECONDS: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalPrizeOverlapAction {
    CollectAndRemove,
    SendAndRetain,
    WaitForAcknowledgement,
}

fn local_prize_overlap_action(
    networked_play: bool,
    claim_retry_timer: &mut f32,
) -> LocalPrizeOverlapAction {
    if !networked_play {
        return LocalPrizeOverlapAction::CollectAndRemove;
    }
    if *claim_retry_timer > 0.0 {
        return LocalPrizeOverlapAction::WaitForAcknowledgement;
    }

    *claim_retry_timer = NETWORK_PRIZE_CLAIM_RETRY_SECONDS;
    LocalPrizeOverlapAction::SendAndRetain
}

impl Game {
    pub(in crate::game) fn apply_damage_prize_loss(&mut self, damage: f32) {
        let tuning = self.ship_tuning();
        if !damage_can_remove_prize(
            self.player_bounty,
            self.ship.energy,
            self.ship.max_energy,
            tuning.damage_factor,
        ) {
            return;
        }

        let chance = damage_prize_loss_chance(tuning.damage_factor, damage);
        if chance == 0 || macroquad::rand::gen_range(0, chance as i32) != 0 {
            return;
        }

        for _ in 0..DAMAGE_PRIZE_LOSS_REROLL_LIMIT {
            let prize_id = macroquad::rand::gen_range(0, PRIZE_PROTOCOL_COUNT as i32) as i16;
            if damage_prize_loss_id_allowed(prize_id) {
                if prize_id == 0 {
                    self.player_bounty = self.player_bounty.saturating_add(1);
                    let seed = macroquad::rand::gen_range(1, i32::MAX);
                    if let Some(kind) = none_prize_replacement_prize(
                        &self.tuning.prize_weights,
                        self.tuning.prize_weight_total,
                        seed,
                    ) {
                        self.apply_prize_effect(kind, false, false);
                    }
                } else if let Some((kind, _)) = PrizeKind::from_protocol_id(prize_id) {
                    if let Some(notification) = self.apply_prize_effect(kind, true, false) {
                        self.push_damage_prize_notification(notification);
                    }
                }
                break;
            }
        }
    }

    pub(in crate::game) fn update_prizes(&mut self, dt: f32) {
        if !self.networked_play {
            self.prize_spawn_timer -= dt;
            if self.prize_spawn_timer <= 0.0 {
                self.prize_spawn_timer = self.tuning.prize_spawn_interval.max(0.01);
                let max_hidden_prizes = self.max_hidden_prizes();
                for _ in 0..self.tuning.prize_spawn_count {
                    if self.prizes.len() >= max_hidden_prizes {
                        break;
                    }
                    self.spawn_prize_near_ship(max_hidden_prizes);
                }
            }
        }

        let player_can_collect =
            local_player_can_collect_prizes(self.spectator_mode, self.player_respawn_timer);
        let player_pickup_radius = self.ship_tuning().radius_pixels;
        let remote_collectors: Vec<_> = if self.networked_play {
            Vec::new()
        } else {
            self.network_world
                .visible_players()
                .map(|player| {
                    (
                        player.clone(),
                        self.tuning.ship(player.ship as usize).radius_pixels,
                    )
                })
                .collect()
        };
        let mut collected = Vec::new();
        let mut bot_collected = Vec::new();
        for prize in &mut self.prizes {
            prize.age += dt;
            prize.ttl -= dt;
            prize.claim_retry_timer = (prize.claim_retry_timer - dt).max(0.0);

            if player_can_collect
                && ship_overlaps_prize(self.ship.pos, player_pickup_radius, prize.pos)
            {
                match local_prize_overlap_action(self.networked_play, &mut prize.claim_retry_timer)
                {
                    LocalPrizeOverlapAction::CollectAndRemove => {
                        collected.push((
                            prize.collectible_id,
                            prize.pos,
                            prize.kind,
                            prize.negative,
                        ));
                        prize.ttl = 0.0;
                    }
                    LocalPrizeOverlapAction::SendAndRetain => {
                        collected.push((
                            prize.collectible_id,
                            prize.pos,
                            prize.kind,
                            prize.negative,
                        ));
                    }
                    LocalPrizeOverlapAction::WaitForAcknowledgement => {}
                }
            } else if remote_collectors
                .iter()
                .any(|(player, radius)| network_player_overlaps_prize(player, *radius, prize.pos))
            {
                prize.ttl = 0.0;
            } else if let Some(bot_id) = self
                .bots
                .iter()
                .filter(|bot| bot.respawn_timer <= 0.0)
                .filter(|bot| {
                    ship_overlaps_prize(
                        bot.pos,
                        self.tuning.ship(bot.ship_type).radius_pixels,
                        prize.pos,
                    )
                })
                .min_by(|left, right| {
                    left.pos
                        .distance_squared(prize.pos)
                        .total_cmp(&right.pos.distance_squared(prize.pos))
                })
                .map(|bot| bot.id)
            {
                bot_collected.push((bot_id, prize.kind, prize.negative));
                prize.ttl = 0.0;
            }
        }
        self.prizes.retain(|prize| prize.ttl > 0.0);

        for (bot_id, kind, negative) in &bot_collected {
            if let Some(bot) = self.bots.iter_mut().find(|bot| bot.id == *bot_id) {
                if !negative {
                    apply_bot_prize(bot, *kind, self.tuning.ship(bot.ship_type));
                }
            }
        }

        if !collected.is_empty() || !bot_collected.is_empty() {
            let mut play_local_pickup_sound = false;
            for (collectible_id, pos, kind, negative) in collected {
                self.send_network_take_prize(collectible_id, pos, kind, negative);
                if !self.networked_play {
                    if let Some(notification) = self.apply_prize_effect(kind, negative, !negative) {
                        self.push_prize_notification(notification);
                    }
                    play_local_pickup_sound = true;
                }
            }

            for (bot_id, _, _) in bot_collected {
                play_local_pickup_sound = true;
                self.push_notification(
                    &format!("Bot {} picked up a green.", bot_id + 1),
                    Color::from_rgba(255, 190, 95, 255),
                );
            }

            if play_local_pickup_sound {
                if let Some(sound) = self.assets.prize_sound.as_ref() {
                    play_sound(
                        sound,
                        PlaySoundParams {
                            looped: false,
                            volume: self.effect_volume(0.35),
                        },
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_prize_claim_is_retained_and_retried_until_authoritative_acknowledgement() {
        let mut retry_timer = 0.0;

        assert_eq!(
            local_prize_overlap_action(true, &mut retry_timer),
            LocalPrizeOverlapAction::SendAndRetain
        );
        assert_eq!(retry_timer, NETWORK_PRIZE_CLAIM_RETRY_SECONDS);
        assert_eq!(
            local_prize_overlap_action(true, &mut retry_timer),
            LocalPrizeOverlapAction::WaitForAcknowledgement
        );

        retry_timer = 0.0;
        assert_eq!(
            local_prize_overlap_action(true, &mut retry_timer),
            LocalPrizeOverlapAction::SendAndRetain
        );
    }

    #[test]
    fn offline_prize_claim_still_removes_the_pickup_immediately() {
        let mut retry_timer = 0.0;

        assert_eq!(
            local_prize_overlap_action(false, &mut retry_timer),
            LocalPrizeOverlapAction::CollectAndRemove
        );
        assert_eq!(retry_timer, 0.0);
    }
}
