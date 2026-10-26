use super::prize_notifications::*;
use super::*;

/// Keep more client-side acknowledgement identities than the server keeps
/// retry receipts (`MAX_PRIZE_CLAIM_RECEIPTS == 1024`). Once an identity falls
/// out of this history, the authoritative server can no longer re-ACK it, and
/// collectible identities are never reused within a server session.
const APPLIED_COLLECTIBLE_ACK_HISTORY_LIMIT: usize = 2_048;

#[derive(Default)]
pub(in crate::game) struct AppliedCollectibleAckHistory {
    ids: VecDeque<u32>,
}

impl AppliedCollectibleAckHistory {
    /// Returns true exactly once for a non-zero collectible identity in the
    /// current scope. Duplicate ACKs may still remove a lingering visual, but
    /// must not apply the prize effect again.
    fn record_if_new(&mut self, collectible_id: u32) -> bool {
        if collectible_id == 0 || self.ids.contains(&collectible_id) {
            return false;
        }
        if self.ids.len() == APPLIED_COLLECTIBLE_ACK_HISTORY_LIMIT {
            self.ids.pop_front();
        }
        self.ids.push_back(collectible_id);
        true
    }

    pub(in crate::game) fn clear(&mut self) {
        self.ids.clear();
    }

    #[cfg(test)]
    pub(in crate::game) fn contains(&self, collectible_id: u32) -> bool {
        self.ids.contains(&collectible_id)
    }
}

impl Game {
    pub(in crate::game) fn apply_network_hidden_prize_spawn(&mut self, spawn: HiddenPrizeSpawn) {
        if spawn.collectible_id == 0 || spawn.ttl_ticks == 0 {
            return;
        }
        let Some((kind, negative)) = PrizeKind::from_protocol_id(spawn.prize_id) else {
            return;
        };

        remove_prize_by_collectible_id(&mut self.prizes, spawn.collectible_id);
        self.prizes.push(PrizePickup {
            collectible_id: spawn.collectible_id,
            pos: vec2(spawn.x as f32, spawn.y as f32),
            kind,
            negative,
            age: 0.0,
            ttl: spawn.ttl_ticks as f32 / 100.0,
            claim_retry_timer: 0.0,
        });
    }

    pub(in crate::game) fn apply_network_player_prize(&mut self, prize: PlayerPrize) {
        remove_prize_by_collectible_id(&mut self.prizes, prize.collectible_id);
        // PLAYER_PRIZE is public presentation/removal only. Team sharing is
        // computed against one server roster snapshot and repeated through
        // each recipient's private PrizeState; mutating here would restore
        // multiplayer self-authority and diverge under packet loss/reorder.
    }

    pub(in crate::game) fn apply_network_prize_pickup_ack(
        &mut self,
        scoped: ScopedPrizePickupAck,
    ) -> bool {
        let authority = self.client_authority.view();
        let lifecycle_cutover_pending = matches!(
            authority.transition_kind,
            Some(
                ClientAuthorityTransitionKind::Ship
                    | ClientAuthorityTransitionKind::Frequency
                    | ClientAuthorityTransitionKind::Respawn
                    | ClientAuthorityTransitionKind::Correction
            )
        );
        if !crate::game::network::prize_pickup_ack_matches_current_runtime(
            authority.scope.server_session_id,
            authority.scope.arena_stream_id,
            authority.scope.player_id,
            authority.scope.life_seq,
            authority.scope.alive,
            lifecycle_cutover_pending,
            scoped,
        ) {
            return false;
        }
        let ack = scoped.ack;
        remove_prize_by_collectible_id(&mut self.prizes, ack.collectible_id);
        if ack.prize_id == PrizeKind::Warp.protocol_id() && self.pending_repel_application.is_some()
        {
            if !self
                .applied_collectible_acks
                .ids
                .contains(&ack.collectible_id)
                && !self
                    .client_authority
                    .has_deferred_warp_pickup_ack(ack.collectible_id)
            {
                if self.client_authority.defer_warp_pickup_ack(scoped)
                    == ClientAuthorityDisposition::Rejected(
                        ClientAuthorityRejection::CapacityExceeded,
                    )
                {
                    #[cfg(target_arch = "wasm32")]
                    self.set_network_notice(
                        "Network application barrier capacity exceeded.",
                        TextColor::DarkRed,
                    );
                    self.push_notification("Network application barrier failed closed.", RED);
                }
            }
            return false;
        }
        self.apply_network_prize_pickup_ack_after_repel(ack)
    }

    /// Perform a staged positive-Warp relocation on the position report that
    /// first carries its marker, so that report is exactly the spawn with
    /// zero velocity. A report that also carries a weapon or Repel marker is
    /// not a valid relocation report; the Warp waits for the next one.
    #[cfg(any(test, target_arch = "wasm32"))]
    pub(in crate::game) fn apply_staged_warp_relocation_for_emission(
        &mut self,
        weapon: u16,
        repel_application: bool,
    ) -> bool {
        if self.pending_warp_relocation_collectible_id.is_none() {
            self.pending_warp_relocation_staged = false;
            return false;
        }
        if !self.pending_warp_relocation_staged || weapon != 0 || repel_application {
            return false;
        }
        self.pending_warp_relocation_staged = false;
        self.activate_warp_prize();
        true
    }

    /// An ACKed Warp whose relocation report has not been emitted yet.
    #[cfg(any(test, target_arch = "wasm32"))]
    pub(in crate::game) fn staged_warp_relocation_pending(&self) -> bool {
        self.pending_warp_relocation_staged && self.pending_warp_relocation_collectible_id.is_some()
    }

    /// A staged Warp the last report could not carry forces the next report,
    /// except while it waits for a pending Repel's receipt.
    #[cfg(any(test, target_arch = "wasm32"))]
    pub(in crate::game) fn staged_warp_relocation_forces_next_report(&self) -> bool {
        self.staged_warp_relocation_pending() && self.pending_repel_application.is_none()
    }

    pub(in crate::game) fn apply_deferred_warp_pickup_acks(&mut self) {
        while self.pending_repel_application.is_none() {
            let Some(scoped) = self.client_authority.pop_deferred_warp_pickup_ack() else {
                break;
            };
            let _ = self.apply_network_prize_pickup_ack(scoped);
        }
    }

    fn apply_network_prize_pickup_ack_after_repel(&mut self, ack: PrizePickupAck) -> bool {
        let newly_applied = self
            .applied_collectible_acks
            .record_if_new(ack.collectible_id);
        if newly_applied {
            let already_reconciled = self.last_self_server_prize_life_seq == Some(ack.life_seq)
                && self
                    .last_self_server_prize_revision
                    .is_some_and(|revision| {
                        revision.wrapping_sub(ack.prize_state_revision) < (1 << 31)
                    });
            // The Warp relocation report is an exact server-issued witness.
            // Re-anchor predicted energy from the ACK immediately before the
            // warp moves to spawn; the next position packet therefore cannot
            // differ merely because local recharge ran while the reliable ACK
            // was in flight.
            if ack.prize_id == PrizeKind::Warp.protocol_id() {
                self.ship.energy = f32::from(ack.authoritative_energy);
                self.last_self_server_energy = Some(self.ship.energy);
                self.pending_warp_relocation_collectible_id = Some(ack.collectible_id);
                self.pending_warp_relocation_life_seq = Some(ack.life_seq);
                self.pending_warp_relocation_sequences.clear();
            }
            if !already_reconciled {
                self.apply_accepted_collected_prize(ack.prize_id, 1, true);
            } else if ack.engine_shutdown_application_required
                && prize_may_change_movement_tuning(ack.prize_id)
            {
                // A private PrizeState can overtake its reliable collectible
                // ACK. While the server advertises an unapplied movement
                // boundary, that snapshot deliberately leaves local movement
                // tuning untouched. Apply only the movement portion here;
                // all inventory/energy/bounty fields already came from the
                // authoritative aggregate revision.
                self.apply_prize_movement_effect_only(ack.prize_id);
                self.present_accepted_collected_prize(ack.prize_id, 1);
            } else if ack.prize_id == PrizeKind::Warp.protocol_id() {
                // PrizeState already supplied the bounty/loadout revision, but
                // only this exact collectible ACK authorizes relocation.
                let authoritative_bounty = self.player_bounty;
                self.apply_accepted_collected_prize(ack.prize_id, 1, true);
                self.player_bounty = authoritative_bounty;
            } else {
                // The private PrizeState commonly overtakes this reliable ACK.
                // It owns aggregate state but cannot own pickup presentation:
                // only the exact, deduplicated collectible identity proves
                // which green this browser collected. Preserve the reference game's
                // one named notification plus one Prize sound without
                // applying the already-reconciled effect a second time.
                self.present_accepted_collected_prize(ack.prize_id, 1);
            }
            if ack.prize_id.checked_abs() == Some(14) {
                let snapshot_is_strictly_newer = self.last_self_server_prize_life_seq
                    == Some(ack.life_seq)
                    && self
                        .last_self_server_prize_revision
                        .is_some_and(|revision| {
                            let advance = revision.wrapping_sub(ack.prize_state_revision);
                            advance != 0 && advance < (1 << 31)
                        });
                if !ack.engine_shutdown_application_required && !snapshot_is_strictly_newer {
                    self.accepted_prize_engine_shutdown =
                        Some(crate::game::AcceptedPrizeEngineShutdown {
                            life_seq: ack.life_seq,
                            prize_state_revision: ack.prize_state_revision,
                            until_tick: ack.engine_shutdown_until_tick,
                        });
                }
                self.reconcile_authoritative_engine_shutdown();
            }
        }
        newly_applied
    }

    fn apply_prize_movement_effect_only(&mut self, prize_id: i16) {
        let Some((kind, negative)) = PrizeKind::from_protocol_id(prize_id) else {
            return;
        };
        let tuning = self.ship_tuning();
        match kind {
            PrizeKind::Rotation => {
                self.ship.rotation_per_second = stat_value_after_prize(
                    self.ship.rotation_per_second,
                    tuning.rotation_per_second,
                    tuning.rotation_upgrade_step,
                    tuning.rotation_max_per_second,
                    negative,
                );
            }
            PrizeKind::Thruster => {
                self.ship.thrust_acceleration = stat_value_after_prize(
                    self.ship.thrust_acceleration,
                    tuning.thrust_acceleration,
                    tuning.thrust_upgrade_step,
                    tuning.thrust_max_acceleration,
                    negative,
                );
            }
            PrizeKind::TopSpeed => {
                self.ship.top_speed = stat_value_after_prize(
                    self.ship.top_speed,
                    tuning.top_speed,
                    tuning.top_speed_upgrade_step,
                    tuning.top_speed_max,
                    negative,
                );
            }
            PrizeKind::Recharge => {
                self.ship.recharge_per_second = stat_value_after_prize(
                    self.ship.recharge_per_second,
                    tuning.energy_recharge,
                    tuning.recharge_upgrade_step,
                    tuning.energy_recharge_max,
                    negative,
                );
            }
            PrizeKind::Multiprize if !negative => {
                for kind in authoritative_multiprize_prizes(
                    &self.tuning.prize_weights,
                    self.tuning.prize_weight_total,
                    self.tuning.multiprize_count,
                ) {
                    self.apply_prize_movement_effect_only(kind.protocol_id());
                }
            }
            _ => {}
        }
    }

    pub(in crate::game) fn apply_network_collected_prize(&mut self, prize: CollectedPrize) {
        self.apply_accepted_collected_prize(prize.prize_id, prize.count, false);
    }

    fn apply_accepted_collected_prize(
        &mut self,
        prize_id: i16,
        count: u16,
        positive_warp_relocation_authorized: bool,
    ) {
        self.player_bounty = self
            .player_bounty
            .saturating_add(collected_prize_bounty_gain(prize_id, count));

        if prize_id == 0 {
            for _ in 0..count {
                let seed = macroquad::rand::gen_range(1, i32::MAX);
                if let Some(kind) = none_prize_replacement_prize(
                    &self.tuning.prize_weights,
                    self.tuning.prize_weight_total,
                    seed,
                ) {
                    self.apply_prize_effect(kind, false, false);
                }
            }
        } else if let Some((kind, negative)) = PrizeKind::from_protocol_id(prize_id) {
            for _ in 0..count {
                // A positive Warp green moves the local ship to spawn, so it
                // must stay paired with the server's one-shot relocation
                // authorization. Only the exact collectible ACK path creates
                // that server marker. Aggregate collected-prize packets may
                // still apply negative Warp (energy depletion), but cannot
                // manufacture an uncorrelated teleport.
                if !accepted_prize_effect_allowed(
                    kind,
                    negative,
                    positive_warp_relocation_authorized,
                ) {
                    continue;
                }
                if let Some(notification) = self.apply_prize_effect(kind, negative, false) {
                    self.push_prize_notification(notification);
                }
            }
        }

        self.play_accepted_prize_sound(count);
    }

    /// Present a collectible whose aggregate effect already arrived through
    /// recipient-private PrizeState. State must not be applied again, but the
    /// exact reliable collectible ACK still owns one named pickup message and
    /// one sound, matching the reference game's `OnCollectedPrize` ordering.
    fn present_accepted_collected_prize(&mut self, prize_id: i16, count: u16) {
        if let Some((kind, negative)) = PrizeKind::from_protocol_id(prize_id) {
            for _ in 0..count {
                self.push_prize_notification(PrizeNotification::new(kind, negative));
            }
        }
        self.play_accepted_prize_sound(count);
    }

    fn play_accepted_prize_sound(&mut self, count: u16) {
        if count == 0 {
            return;
        }

        #[cfg(not(test))]
        let prize_sound = self.assets.prize_sound.as_ref();
        #[cfg(test)]
        let prize_sound = self
            .assets
            .0
            .as_ref()
            .and_then(|assets| assets.prize_sound.as_ref());
        if let Some(sound) = prize_sound {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume: self.effect_volume(0.32),
                },
            );
            self.prize_pickup_sound_play_count = self.prize_pickup_sound_play_count.wrapping_add(1);
        }
    }

    pub(in crate::game) fn apply_prize_effect(
        &mut self,
        kind: PrizeKind,
        negative: bool,
        count_bounty: bool,
    ) -> Option<PrizeNotification> {
        if count_bounty {
            self.player_bounty = self.player_bounty.saturating_add(1);
        }

        let tuning = self.ship_tuning();
        let mut notification = Some(PrizeNotification::new(kind, negative));
        match kind {
            PrizeKind::Energy => {
                let before = self.ship.max_energy;
                self.ship.max_energy = energy_capacity_after_prize(
                    self.ship.max_energy,
                    tuning.energy_upgrade_step,
                    tuning.energy_initial,
                    tuning.energy_max,
                    negative,
                );
                self.ship.energy =
                    current_energy_after_capacity_prize(self.ship.energy, self.ship.max_energy);
                notification =
                    changed_prize_notification(kind, negative, before, self.ship.max_energy);
            }
            PrizeKind::FullCharge => {
                self.ship.energy = if negative { 1.0 } else { self.ship.max_energy };
            }
            PrizeKind::Rocket => {
                let before = self.ship.rockets;
                if negative {
                    self.ship.rockets = self.ship.rockets.saturating_sub(1);
                } else {
                    self.ship.rockets = (self.ship.rockets + 1).min(tuning.max_rockets);
                }
                notification =
                    changed_prize_notification(kind, negative, before, self.ship.rockets);
            }
            PrizeKind::Repel => {
                let before = self.ship.repels;
                if negative {
                    self.ship.repels = self.ship.repels.saturating_sub(1);
                } else {
                    self.ship.repels = (self.ship.repels + 1).min(tuning.max_repels);
                }
                notification = changed_prize_notification(kind, negative, before, self.ship.repels);
            }
            PrizeKind::Burst => {
                let before = self.ship.bursts;
                if negative {
                    self.ship.bursts = self.ship.bursts.saturating_sub(1);
                } else {
                    self.ship.bursts = (self.ship.bursts + 1).min(tuning.max_bursts);
                }
                notification = changed_prize_notification(kind, negative, before, self.ship.bursts);
            }
            PrizeKind::Brick => {
                let before = self.ship.bricks;
                if negative {
                    self.ship.bricks = self.ship.bricks.saturating_sub(1);
                } else {
                    self.ship.bricks = (self.ship.bricks + 1).min(tuning.max_bricks);
                }
                notification = changed_prize_notification(kind, negative, before, self.ship.bricks);
            }
            PrizeKind::Decoy => {
                let before = self.ship.decoys;
                if negative {
                    self.ship.decoys = self.ship.decoys.saturating_sub(1);
                } else {
                    self.ship.decoys = (self.ship.decoys + 1).min(tuning.max_decoys);
                }
                notification = changed_prize_notification(kind, negative, before, self.ship.decoys);
            }
            PrizeKind::Thor => {
                let before = self.ship.thors;
                if negative {
                    self.ship.thors = self.ship.thors.saturating_sub(1);
                } else {
                    self.ship.thors = (self.ship.thors + 1).min(tuning.max_thors);
                }
                notification = changed_prize_notification(kind, negative, before, self.ship.thors);
            }
            PrizeKind::Portal => {
                let before = self.ship.portals;
                if negative {
                    self.ship.portals = self.ship.portals.saturating_sub(1);
                } else {
                    self.ship.portals = (self.ship.portals + 1).min(tuning.max_portals);
                }
                notification =
                    changed_prize_notification(kind, negative, before, self.ship.portals);
            }
            PrizeKind::Super => {
                let max_ticks =
                    crate::continuum::seconds_to_ticks_i32(tuning.super_lifetime).max(0);
                if max_ticks > 0 {
                    let previous_timer = self.ship.super_timer;
                    self.ship.super_timer = super_prize_lifetime_after(
                        self.ship.super_timer,
                        tuning.super_lifetime,
                        macroquad::rand::gen_range(0, max_ticks) as u32,
                    );
                    if self.ship.super_timer > previous_timer {
                        self.super_animation_age = 0.0;
                    }
                }
            }
            PrizeKind::Shield => {
                self.shield_animation_age = 0.0;
                self.ship.shield_timer = shield_prize_lifetime_after(tuning.shield_lifetime);
            }
            PrizeKind::EngineShutdown => {
                self.ship.engine_shutdown_timer =
                    engine_shutdown_lifetime_after(tuning.engine_shutdown_lifetime, negative);
                self.thrust_sound_started = false;
                self.exhaust_timer = 0.0;
            }
            PrizeKind::BouncingBullets => {
                notification =
                    capability_prize_notification_entry(kind, self.ship.bouncing_bullets, negative);
                self.ship.bouncing_bullets = !negative;
            }
            PrizeKind::Stealth => {
                notification = status_prize_notification_entry(
                    kind,
                    tuning.stealth_available,
                    self.ship.stealth_capable,
                    negative,
                )
                .map(PrizeNotification::from);
                apply_status_prize(
                    &mut self.ship.stealth_capable,
                    &mut self.ship.stealth,
                    &mut self.ship.energy,
                    self.ship.max_energy,
                    tuning.stealth_available,
                    negative,
                );
            }
            PrizeKind::Cloak => {
                notification = status_prize_notification_entry(
                    kind,
                    tuning.cloak_available,
                    self.ship.cloak_capable,
                    negative,
                )
                .map(PrizeNotification::from);
                apply_status_prize(
                    &mut self.ship.cloak_capable,
                    &mut self.ship.cloak,
                    &mut self.ship.energy,
                    self.ship.max_energy,
                    tuning.cloak_available,
                    negative,
                );
            }
            PrizeKind::XRadar => {
                notification = status_prize_notification_entry(
                    kind,
                    tuning.xradar_available,
                    self.ship.xradar_capable,
                    negative,
                )
                .map(PrizeNotification::from);
                apply_status_prize(
                    &mut self.ship.xradar_capable,
                    &mut self.ship.xradar,
                    &mut self.ship.energy,
                    self.ship.max_energy,
                    tuning.xradar_available,
                    negative,
                );
            }
            PrizeKind::Warp => {
                if negative {
                    self.ship.energy =
                        warp_prize_energy_after(self.ship.energy, self.ship.max_energy, true);
                    notification =
                        Some(PrizeNotification::from(warp_prize_notification_after(true)));
                } else {
                    if self.networked_play && self.pending_warp_relocation_collectible_id.is_some()
                    {
                        // Gameplay steps can run before the next position is
                        // emitted. Relocating now would let them add motion
                        // to the one-shot spawn report the server checks.
                        self.pending_warp_relocation_staged = true;
                        self.pending_same_life_movement_boundary_position = true;
                    } else {
                        self.activate_warp_prize();
                    }
                    notification = Some(PrizeNotification::from(warp_prize_notification_after(
                        false,
                    )));
                }
            }
            PrizeKind::Rotation => {
                let before = self.ship.rotation_per_second;
                self.ship.rotation_per_second = stat_value_after_prize(
                    self.ship.rotation_per_second,
                    tuning.rotation_per_second,
                    tuning.rotation_upgrade_step,
                    tuning.rotation_max_per_second,
                    negative,
                );
                notification = changed_prize_notification(
                    kind,
                    negative,
                    before,
                    self.ship.rotation_per_second,
                );
            }
            PrizeKind::Thruster => {
                let before = self.ship.thrust_acceleration;
                self.ship.thrust_acceleration = stat_value_after_prize(
                    self.ship.thrust_acceleration,
                    tuning.thrust_acceleration,
                    tuning.thrust_upgrade_step,
                    tuning.thrust_max_acceleration,
                    negative,
                );
                notification = changed_prize_notification(
                    kind,
                    negative,
                    before,
                    self.ship.thrust_acceleration,
                );
            }
            PrizeKind::TopSpeed => {
                let before = self.ship.top_speed;
                self.ship.top_speed = stat_value_after_prize(
                    self.ship.top_speed,
                    tuning.top_speed,
                    tuning.top_speed_upgrade_step,
                    tuning.top_speed_max,
                    negative,
                );
                notification =
                    changed_prize_notification(kind, negative, before, self.ship.top_speed);
            }
            PrizeKind::Recharge => {
                let before = self.ship.recharge_per_second;
                self.ship.recharge_per_second = stat_value_after_prize(
                    self.ship.recharge_per_second,
                    tuning.energy_recharge,
                    tuning.recharge_upgrade_step,
                    tuning.energy_recharge_max,
                    negative,
                );
                notification = changed_prize_notification(
                    kind,
                    negative,
                    before,
                    self.ship.recharge_per_second,
                );
            }
            PrizeKind::Shrapnel => {
                let before = self.ship.shrapnel_upgrades;
                self.ship.shrapnel_upgrades = shrapnel_prize_amount_after(
                    self.ship.shrapnel_upgrades,
                    tuning.shrapnel_rate,
                    tuning.shrapnel_max,
                    negative,
                );
                notification =
                    changed_prize_notification(kind, negative, before, self.ship.shrapnel_upgrades);
            }
            PrizeKind::Guns => {
                let before = self.ship.gun_level;
                self.ship.gun_level = if negative {
                    self.ship
                        .gun_level
                        .saturating_sub(1)
                        .max(tuning.initial_gun_level)
                } else {
                    (self.ship.gun_level + 1).min(tuning.max_gun_level)
                };
                notification =
                    changed_prize_notification(kind, negative, before, self.ship.gun_level);
            }
            PrizeKind::Bombs => {
                let before = self.ship.bomb_level;
                self.ship.bomb_level = if negative {
                    self.ship
                        .bomb_level
                        .saturating_sub(1)
                        .max(tuning.initial_bomb_level)
                } else {
                    (self.ship.bomb_level + 1).min(tuning.max_bomb_level)
                };
                notification =
                    changed_prize_notification(kind, negative, before, self.ship.bomb_level);
            }
            PrizeKind::Multifire => {
                notification = capability_prize_notification_entry(
                    kind,
                    self.ship.multifire_capable,
                    negative,
                );
                apply_multifire_prize(
                    &mut self.ship.multifire_capable,
                    &mut self.ship.multifire,
                    negative,
                );
            }
            PrizeKind::Proximity => {
                notification =
                    capability_prize_notification_entry(kind, self.ship.proximity_bombs, negative);
                self.ship.proximity_bombs = !negative;
            }
            PrizeKind::Antiwarp => {
                notification = status_prize_notification_entry(
                    kind,
                    tuning.antiwarp_available,
                    self.ship.antiwarp_capable,
                    negative,
                )
                .map(PrizeNotification::from);
                apply_status_prize(
                    &mut self.ship.antiwarp_capable,
                    &mut self.ship.antiwarp,
                    &mut self.ship.energy,
                    self.ship.max_energy,
                    tuning.antiwarp_available,
                    negative,
                );
            }
            PrizeKind::Multiprize => {
                if !negative {
                    for kind in authoritative_multiprize_prizes(
                        &self.tuning.prize_weights,
                        self.tuning.prize_weight_total,
                        self.tuning.multiprize_count,
                    ) {
                        self.apply_prize_effect(kind, false, false);
                    }
                }
            }
        }
        notification
    }

    pub(in crate::game) fn push_prize_notification(&mut self, notification: PrizeNotification) {
        let color = match (notification.kind, notification.negative) {
            (_, true) => Color::from_rgba(255, 170, 80, 255),
            (PrizeKind::EngineShutdown, false) => Color::from_rgba(255, 170, 80, 255),
            (PrizeKind::Super | PrizeKind::Shield, false) => Color::from_rgba(255, 235, 120, 255),
            _ => Color::from_rgba(125, 230, 150, 255),
        };
        let message = prize_notification_text_for_entry(notification, false);
        if !message.is_empty() {
            self.prize_pickup_notification_count =
                self.prize_pickup_notification_count.wrapping_add(1);
            self.last_prize_pickup_notification = Some(message.clone());
            self.push_notification(&message, color);
        }
    }

    pub(in crate::game) fn push_damage_prize_notification(
        &mut self,
        notification: PrizeNotification,
    ) {
        let message = prize_notification_text_for_entry(notification, true);
        if !message.is_empty() {
            self.push_notification(&message, Color::from_rgba(255, 235, 120, 255));
        }
    }
}

fn prize_may_change_movement_tuning(prize_id: i16) -> bool {
    matches!(prize_id.checked_abs(), Some(1 | 3 | 11 | 12)) || prize_id == 25
}

fn accepted_prize_effect_allowed(
    kind: PrizeKind,
    negative: bool,
    positive_warp_relocation_authorized: bool,
) -> bool {
    kind != PrizeKind::Warp || negative || positive_warp_relocation_authorized
}

#[cfg(test)]
mod authorization_tests {
    use super::*;

    #[test]
    fn only_exact_collectible_ack_origin_authorizes_positive_warp_relocation() {
        assert!(accepted_prize_effect_allowed(PrizeKind::Warp, false, true));
        assert!(!accepted_prize_effect_allowed(
            PrizeKind::Warp,
            false,
            false
        ));
        assert!(accepted_prize_effect_allowed(PrizeKind::Warp, true, false));
        assert!(accepted_prize_effect_allowed(
            PrizeKind::Energy,
            false,
            false
        ));
    }
}

pub(super) fn apply_multifire_prize(capable: &mut bool, active: &mut bool, negative: bool) {
    if negative {
        *capable = false;
        *active = false;
    } else {
        *capable = true;
    }
}

#[cfg(test)]
mod ack_history_tests {
    use super::*;

    #[test]
    fn duplicate_and_delayed_ack_ids_apply_once_per_scope() {
        let mut history = AppliedCollectibleAckHistory::default();
        assert!(history.record_if_new(41));
        assert!(!history.record_if_new(41));
        assert!(history.record_if_new(43));
        assert!(history.record_if_new(42));
        assert!(!history.record_if_new(41));
        assert!(!history.record_if_new(42));
        assert!(!history.record_if_new(0));

        // A new server-session or arena scope may start a new non-reusing ID
        // namespace. Death/respawn does not call this reset.
        history.clear();
        assert!(history.record_if_new(41));
    }

    #[test]
    fn acknowledgement_history_is_bounded_beyond_server_retry_retention() {
        let mut history = AppliedCollectibleAckHistory::default();
        for id in 1..=(APPLIED_COLLECTIBLE_ACK_HISTORY_LIMIT as u32 + 1) {
            assert!(history.record_if_new(id));
        }
        assert_eq!(history.ids.len(), APPLIED_COLLECTIBLE_ACK_HISTORY_LIMIT);
        assert_eq!(history.ids.front().copied(), Some(2));
        assert!(!history.record_if_new(2));
        assert!(!history.record_if_new(APPLIED_COLLECTIBLE_ACK_HISTORY_LIMIT as u32 + 1));
    }
}
