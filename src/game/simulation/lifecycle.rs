use super::*;

impl Game {
    fn clear_respawn_input_boundary(&mut self) {
        // The admitted successor-life fact already cancelled any correction in
        // the sole sequencer before presentation-side respawn cleanup runs.
        self.pending_same_life_movement_boundary_position = false;
        self.clear_prize_effect_application_state();
        self.clear_portal_placement_state();
        self.last_input = PlayerInput::default();
        self.outbound_input_ring.clear();
        self.pending_weapon = 0;
        self.cancel_pending_weapon_prediction();
        self.pending_warp_relocation_collectible_id = None;
        self.pending_repel_application = None;
        self.pending_repel_application_expiry_tick = None;
        self.expired_repel_applications.clear();
        self.consumed_repel_delivery_generations.clear();
        self.client_authority.clear_deferred_warp_pickup_acks();
        self.repel_movement_sequence_window = None;
        self.pending_warp_relocation_life_seq = None;
        self.pending_warp_relocation_sequences.clear();
    }

    pub(in crate::game) fn arm_respawn_input_boundary(&mut self) {
        self.last_input = PlayerInput::default();
        self.outbound_input_ring.clear();
        self.pending_weapon = 0;
        self.cancel_pending_weapon_prediction();
        // `reset_ship` installs the locally predicted replacement loadout,
        // while the sequencer keeps server authority dead until this exact
        // neutral boundary is admitted.
    }

    pub(in crate::game) fn start_player_respawn(&mut self, death_pos: Vec2) {
        let delay = respawn_delay_seconds(self.tuning.enter_delay_seconds);
        self.start_player_respawn_with_delay(death_pos, delay);
    }

    pub(in crate::game) fn start_player_respawn_with_delay(&mut self, death_pos: Vec2, delay: f32) {
        self.require_forward_self_server_rocket_life =
            self.last_self_server_rocket_life_seq.is_some();
        self.player_respawn_seq = self.player_respawn_seq.wrapping_add(1);
        self.network_world.lvz.trigger_death();
        self.add_ship_explosion(death_pos);
        self.clear_respawn_input_boundary();
        if delay <= 0.0 {
            if self.networked_play {
                let disposition = self.client_authority.advance_respawn_eligibility(0.0);
                debug_assert_eq!(disposition, ClientAuthorityDisposition::Applied);
            }
            self.reset_ship_after_death();
            self.arm_respawn_input_boundary();
            return;
        }

        self.player_respawn_timer = delay;
        self.respawn_input_latch.arm();
        self.ship.energy = 0.0;
        self.last_self_server_energy = Some(0.0);
        self.ship.vel = Vec2::ZERO;
        self.ship.rocket_timer = 0.0;
        self.ship.rocket_cooldown = 0.0;
        self.pending_rocket_activation_sequence = None;
        self.ship.super_timer = 0.0;
        self.ship.shield_timer = 0.0;
        self.ship.engine_shutdown_timer = 0.0;
        self.ship.emp_shutdown_timer = 0.0;
        self.ship.fake_antiwarp_timer = 0.0;
        self.thrust_sound_started = false;
        self.exhaust_timer = 0.0;
        self.powerball.carried = false;
    }
}
