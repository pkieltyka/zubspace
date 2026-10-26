use super::ClientHandle;
use crate::arena_settings::{
    arena_engine_shutdown_ticks, arena_multiprize_count, arena_prize_weight_total,
    arena_prize_weights, arena_ship_antiwarp_status, arena_ship_cloak_status,
    arena_ship_energy_upgrade, arena_ship_initial_bomb_level, arena_ship_initial_bounty,
    arena_ship_initial_brick_count, arena_ship_initial_burst_count, arena_ship_initial_decoy_count,
    arena_ship_initial_energy, arena_ship_initial_gun_level, arena_ship_initial_portal_count,
    arena_ship_initial_recharge_per_second, arena_ship_initial_repel_count,
    arena_ship_initial_rocket_count, arena_ship_initial_rotation_per_second,
    arena_ship_initial_thor_count, arena_ship_initial_thrust_pixels_per_second_squared,
    arena_ship_initial_top_speed_pixels_per_second, arena_ship_max_bomb_level,
    arena_ship_max_brick_count, arena_ship_max_burst_count, arena_ship_max_decoy_count,
    arena_ship_max_energy, arena_ship_max_gun_level, arena_ship_max_portal_count,
    arena_ship_max_recharge_per_second, arena_ship_max_repel_count, arena_ship_max_rocket_count,
    arena_ship_max_rotation_per_second, arena_ship_max_speed_pixels_per_second,
    arena_ship_max_thor_count, arena_ship_max_thrust_pixels_per_second_squared,
    arena_ship_recharge_upgrade_per_second, arena_ship_rotation_upgrade_per_second,
    arena_ship_shield_lifetime_ticks, arena_ship_shrapnel_max, arena_ship_shrapnel_rate,
    arena_ship_stealth_status, arena_ship_super_lifetime_ticks,
    arena_ship_thrust_upgrade_pixels_per_second_squared,
    arena_ship_top_speed_upgrade_pixels_per_second, arena_ship_xradar_status,
};
use crate::authority::{
    StatusInventory, StatusInventoryTuning, WeaponInventory, WeaponInventoryTuning,
};
use crate::authority_prizes::{
    SECURITY_PRIZE_SEED, initial_bounty_prize_ids, multiprize_sub_prize_ids,
};

// Continuum protocol prize id for Multiprize. Matches
// `crate::prize::PrizeKind::Multiprize.protocol_id()` on the client. Hardcoded
// here because the server binary intentionally does not pull in the full
// shared `prize`/`model` modules.
const MULTIPRIZE_PROTOCOL_ID: i16 = 25;
const SUPER_PRIZE_PROTOCOL_ID: i16 = 17;
const SHIELD_PRIZE_PROTOCOL_ID: i16 = 18;
const ENGINE_SHUTDOWN_PRIZE_PROTOCOL_ID: i16 = 14;
// Phase 1c-3c-ii: movement-upgrade prize ids. Match the Continuum
// protocol enum used by the client's PrizeKind table.
const ROTATION_PRIZE_PROTOCOL_ID: i16 = 3;
const THRUST_PRIZE_PROTOCOL_ID: i16 = 11;
const SPEED_PRIZE_PROTOCOL_ID: i16 = 12;
const RECHARGE_PRIZE_PROTOCOL_ID: i16 = 1;
/// [REVIEW M3] Width of the trailing weapon-sequence window. Wide enough
/// for any realistic reliable-mirror delay (64 frames ≈ 1 s at 60 Hz);
/// anything older is rejected as stale.
const WEAPON_SEQUENCE_WINDOW: u16 = 64;
/// A quarter of the u16 sequence space: about 4.5 minutes at 60 Hz.
const WEAPON_SEQUENCE_MAX_LAG: u16 = 0x4000;
/// After a damage prize change, the browser keeps its previous movement
/// tuning for about one round trip; this margin covers applying the private
/// prize state plus frame and send jitter.
const DAMAGE_MOVEMENT_GRACE_MARGIN_MS: f64 = 200.0;
/// Round trip assumed before connection quality has a reading: the prod and
/// dev forced-spectator limit.
const DAMAGE_MOVEMENT_GRACE_UNMEASURED_RTT_MS: f64 = 400.0;
/// Never more than one second of 60 Hz input.
const DAMAGE_MOVEMENT_GRACE_MAX_STEPS: u16 = 60;
const POSITION_VALIDATION_ANCHOR_RETENTION_TICKS: u32 = 10 * 60 * 100;

impl ClientHandle {
    pub(crate) fn begin_actor_motion_segment(&mut self) {
        let actor = crate::authority_identity::PlayerRuntimeIdentity::new(
            crate::authority_identity::PlayerSessionIdentity::new(self.player.id, self.session_id),
            crate::authority_identity::LifeIdentity::new(self.player.id, self.life_seq),
        );
        self.actor_motion_history
            .begin_segment(actor, self.arena_stream_id);
    }

    pub(crate) fn append_actor_motion_witness(
        &mut self,
        server_tick: u32,
        witness: crate::state::CanonicalPositionWitness,
    ) -> Result<(), crate::combat_timeline::ClockedHistoryError> {
        if witness.life_seq != self.life_seq {
            return Err(crate::combat_timeline::ClockedHistoryError::WrongSegment);
        }
        let actor = crate::authority_identity::PlayerRuntimeIdentity::new(
            crate::authority_identity::PlayerSessionIdentity::new(self.player.id, self.session_id),
            crate::authority_identity::LifeIdentity::new(self.player.id, self.life_seq),
        );
        let segment = match self.actor_motion_history.current_segment() {
            Some(segment)
                if segment.actor == actor && segment.arena_stream_id == self.arena_stream_id =>
            {
                segment
            }
            _ => self
                .actor_motion_history
                .begin_segment(actor, self.arena_stream_id),
        };
        let sample = self.actor_sample_from_witness(segment, server_tick, witness);
        self.actor_motion_history
            .begin_accepted_samples(witness.sequence, sample)
    }

    fn actor_sample_from_witness(
        &self,
        segment: crate::combat_timeline::MotionSegmentIdentity,
        server_tick: u32,
        witness: crate::state::CanonicalPositionWitness,
    ) -> crate::combat_timeline::ActorMotionSampleV1 {
        crate::combat_timeline::ActorMotionSampleV1::from_state(
            segment,
            server_tick,
            witness.state.position_x,
            witness.state.position_y,
            witness.state.velocity_x,
            witness.state.velocity_y,
            witness.state.orientation,
            crate::arena_settings::arena_ship_radius_pixels(self.player.ship),
            u8::from(self.objective_last_safe),
            crate::combat_timeline::ActorMotionSourceKind::AcceptedHumanWitness,
        )
    }

    pub(crate) fn append_actor_motion_replay(
        &mut self,
        server_tick: u32,
        witnesses: &[crate::state::CanonicalPositionWitness],
    ) -> Result<(), crate::combat_timeline::ClockedHistoryError> {
        use crate::combat_timeline::ClockedHistoryError;
        let segment = self
            .actor_motion_history
            .current_segment()
            .ok_or(ClockedHistoryError::MissingClock)?;
        if witnesses.len() > crate::combat_timeline::ACTOR_MOTION_HISTORY_SAMPLE_CAPACITY {
            return Err(ClockedHistoryError::BatchLength);
        }
        if witnesses.iter().any(|w| w.life_seq != self.life_seq) {
            return Err(ClockedHistoryError::WrongSegment);
        }
        let samples: Vec<_> = witnesses
            .iter()
            .map(|&w| {
                (
                    w.sequence,
                    self.actor_sample_from_witness(segment, server_tick, w),
                )
            })
            .collect();
        self.actor_motion_history.append_accepted_samples(&samples)
    }

    pub(crate) fn movement_tuning_values_for_replay(&self) -> (f32, f32, f32, f32) {
        self.pending_prize_effect_applications
            .iter()
            .find_map(|pending| pending.movement_tuning_before_bits)
            .map(|bits| {
                (
                    f32::from_bits(bits[0]),
                    f32::from_bits(bits[1]),
                    f32::from_bits(bits[2]),
                    f32::from_bits(bits[3]),
                )
            })
            .unwrap_or((
                self.server_thrust_acceleration,
                self.server_top_speed,
                self.server_rotation_per_second,
                self.server_recharge_per_second,
            ))
    }

    /// The replay tuning a damage prize change replaced, while input at
    /// `sequence` may still have been generated without knowing of it.
    pub(crate) fn damage_movement_grace_tuning(
        &self,
        sequence: u16,
    ) -> Option<(f32, f32, f32, f32)> {
        let grace = self
            .damage_movement_grace
            .filter(|grace| grace.life_seq == self.life_seq)?;
        // Wrap-safe `sequence <= last_sequence`.
        (grace.last_sequence.wrapping_sub(sequence) < (1 << 15)).then(|| {
            let bits = grace.tuning_before_bits;
            (
                f32::from_bits(bits[0]),
                f32::from_bits(bits[1]),
                f32::from_bits(bits[2]),
                f32::from_bits(bits[3]),
            )
        })
    }

    /// Damage has just changed this player's prizes. If that changed the
    /// replay tuning from `before`, accept `before` for input the browser may
    /// generate during the next round trip, while the private prize state is
    /// still on its way. The window is bounded by the measured RTT.
    pub(crate) fn begin_damage_movement_grace(&mut self, before: (f32, f32, f32, f32)) {
        let bits = |tuning: (f32, f32, f32, f32)| {
            [
                tuning.0.to_bits(),
                tuning.1.to_bits(),
                tuning.2.to_bits(),
                tuning.3.to_bits(),
            ]
        };
        let Some(last_input) = self.last_input_seq else {
            return;
        };
        if bits(self.movement_tuning_values_for_replay()) == bits(before) {
            return;
        }
        let rtt_ms = self
            .connection_quality
            .as_ref()
            .and_then(|shared| {
                shared
                    .lock()
                    .expect("session quality lock poisoned")
                    .reading
                    .and_then(|reading| reading.average_rtt_ms)
            })
            .unwrap_or(DAMAGE_MOVEMENT_GRACE_UNMEASURED_RTT_MS);
        let steps = ((rtt_ms + DAMAGE_MOVEMENT_GRACE_MARGIN_MS) * 60.0 / 1000.0).ceil();
        let steps = (steps as u16).min(DAMAGE_MOVEMENT_GRACE_MAX_STEPS);
        let mut grace = crate::state::DamageMovementGrace {
            life_seq: self.life_seq,
            last_sequence: last_input.wrapping_add(steps),
            tuning_before_bits: bits(before),
        };
        // A second change inside an open window keeps the browser's oldest
        // tuning and the later end; intermediate tuning is not retried.
        if let Some(open) = self.damage_movement_grace
            && self.damage_movement_grace_tuning(last_input).is_some()
        {
            grace.tuning_before_bits = open.tuning_before_bits;
            if open.last_sequence.wrapping_sub(grace.last_sequence) < (1 << 15) {
                grace.last_sequence = open.last_sequence;
            }
        }
        self.damage_movement_grace = Some(grace);
    }

    pub(crate) fn allocate_realtime_event_id(&mut self) -> u32 {
        self.next_realtime_event_id = self.next_realtime_event_id.wrapping_add(1).max(1);
        self.next_realtime_event_id
    }

    /// Decide one synthetic outbound-loss sample. When the focused browser
    /// gate requests post-bootstrap loss, the reliable join fence and bounded
    /// map/LVZ transfer remain lossless until this exact client has ACKed
    /// every file fragment. Every later reliable/event/snapshot send uses the
    /// normal seeded loss stream and counters.
    pub(crate) fn should_drop_simulated_tx(&self, packet_len: usize) -> bool {
        if self.simulated_tx_loss_after_bootstrap {
            let files_complete = self.map_transfer.as_ref().is_none_or(|transfer| {
                transfer.next_index >= transfer.file_count
                    && transfer.pending_index.is_none()
                    && transfer.deferred_successor_index.is_none()
                    && transfer.active.is_none()
            });
            if !self.snapshot_ready || !files_complete {
                return false;
            }
        }
        self.simulated_tx_loss.should_drop(packet_len)
    }

    /// Sweep one-shot deadlines and bounded-history anchors while they are
    /// still inside Continuum's signed half-range. Wrap-safe comparisons are
    /// only unambiguous for spans below 2^30 ticks; the server tick loop calls
    /// this continuously, so no completed timer remains dormant long enough
    /// to be interpreted as active again on a later clock cycle.
    pub(crate) fn maintain_short_tick_state(
        &mut self,
        tick: u32,
        gun_cooldown_ticks: u32,
        bomb_cooldown_ticks: u32,
        brick_cooldown_ticks: u32,
        projectile_retention_ticks: u32,
    ) {
        clear_reached_deadline(&mut self.server_super_until_tick, tick);
        clear_reached_deadline(&mut self.server_shield_until_tick, tick);
        let prize_shutdown_was_pending = self.server_engine_shutdown_until_tick.is_some()
            && !self.pending_prize_effect_applications.is_empty();
        clear_reached_deadline(&mut self.server_engine_shutdown_until_tick, tick);
        clear_reached_deadline(&mut self.server_emp_until_tick, tick);
        if self.server_engine_shutdown_until_tick.is_none() {
            self.server_engine_shutdown_start_sequence = None;
            if prize_shutdown_was_pending {
                self.cancel_pending_prize_effect_applications();
            }
        }
        if self.server_emp_until_tick.is_none() {
            self.server_emp_start_sequence = None;
        }
        clear_reached_deadline(&mut self.server_rocket_until_tick, tick);
        if self.active_portal.is_some_and(|portal| {
            crate::continuum::continuum_tick_reached(tick, portal.expires_at_tick)
        }) {
            self.active_portal = None;
        }
        clear_reached_deadline(&mut self.server_warp_settle_until_tick, tick);
        clear_reached_deadline(&mut self.server_warp_bomb_lock_until_tick, tick);
        // A position generated just before RepelTime expires can arrive just
        // after the wall-clock deadline on the unordered game channel. Keep
        // the original deadline as bounded replay provenance so each retained
        // input record is evaluated against its own reconstructed server tick;
        // all live/status users still compare the deadline and therefore see
        // it as inactive immediately. Clear only after the complete rolling
        // input recovery window can no longer reference a pre-deadline step.
        const REPEL_REPLAY_RETENTION_TICKS: u32 = 200;
        if self.server_repel_until_tick.is_some_and(|deadline| {
            crate::continuum::continuum_tick_reached(
                tick,
                crate::continuum::continuum_tick_add(deadline, REPEL_REPLAY_RETENTION_TICKS),
            )
        }) {
            self.server_repel_until_tick = None;
        }
        let repel_observer_window_ended = self
            .server_repel_observer_until_tick
            .is_some_and(|deadline| crate::continuum::continuum_tick_reached(tick, deadline));
        if repel_observer_window_ended {
            self.server_repel_observer_until_tick = None;
            // Snapshot ACK and browser application are separated by main-loop
            // scheduling, so their local RepelTime endpoints can differ by a
            // frame. End the delivery-relative diagnostic interval with a
            // fresh accepted baseline rather than accumulating that known
            // boundary skew as malicious drift. Canonical admission state is
            // deliberately untouched.
            self.reset_private_movement_observer();
        }
        // Unlike one-shot effect timers, a due respawn deadline is also the
        // server's readiness latch. Keep it until the first exact canonical
        // respawn report commits the life transition; clearing it here would
        // leave `alive == false` with no way for position admission to
        // recognize that the deadline has elapsed.
        // A target-private Repel is application authority, not presentation
        // state. Keep repeating it after the original wall deadline until an
        // exact application marker commits or lifecycle/session reset clears
        // the runtime.

        // A deadline includes its anchor's carried earliness, so keep each
        // anchor until an unproven weapon would also be past it.
        let schedule = &mut self.weapon_cooldown_schedule;
        if clear_elapsed_anchor(
            &mut self.last_gun_fire_tick,
            tick,
            gun_cooldown_ticks + schedule.gun_early_ticks,
        ) {
            schedule.gun_early_ticks = 0;
        }
        if clear_elapsed_anchor(
            &mut self.last_bomb_fire_tick,
            tick,
            bomb_cooldown_ticks + schedule.bomb_early_ticks,
        ) {
            schedule.bomb_early_ticks = 0;
        }
        if clear_elapsed_anchor(
            &mut schedule.repel_lock_tick,
            tick,
            crate::authority::REPEL_DELAY_TICKS + schedule.repel_early_ticks,
        ) {
            schedule.repel_early_ticks = 0;
        }
        clear_elapsed_anchor(
            &mut self.last_weapon_fire_tick,
            tick,
            gun_cooldown_ticks.max(bomb_cooldown_ticks),
        );
        clear_elapsed_anchor(&mut self.last_brick_drop_tick, tick, brick_cooldown_ticks);
        if clear_elapsed_anchor(
            &mut self.last_accepted_weapon_tick,
            tick,
            projectile_retention_ticks
                .max(gun_cooldown_ticks)
                .max(bomb_cooldown_ticks),
        ) {
            self.last_accepted_weapon_data = None;
        }

        self.recent_damagers.retain(|(_, at_tick)| {
            crate::continuum::continuum_tick_elapsed(tick, *at_tick)
                <= crate::state::ASSIST_DAMAGE_WINDOW_TICKS
        });
        self.position_history.retain(|sample| {
            crate::continuum::continuum_tick_elapsed(tick, sample.tick)
                <= POSITION_VALIDATION_ANCHOR_RETENTION_TICKS
        });
        if self.last_position_server_tick.is_some_and(|anchor| {
            crate::continuum::continuum_tick_elapsed(tick, anchor)
                > POSITION_VALIDATION_ANCHOR_RETENTION_TICKS
        }) {
            self.last_position_server_tick = None;
            self.server_sim_anchor_tick = None;
            self.server_sim_input_bits = None;
            self.position_history.clear();
        }
    }

    pub(crate) fn clear_expired_life_start_gate(
        &mut self,
        tick: u32,
        projectile_retention_ticks: u32,
    ) {
        clear_elapsed_anchor(
            &mut self.life_started_tick,
            tick,
            projectile_retention_ticks,
        );
    }

    /// [REVIEW M3] Window semantics: sequences ahead of `latest` are fresh;
    /// sequences inside the trailing window are checked against the mask
    /// (delayed reliable weapon mirrors stay accepted exactly once); and
    /// anything older than the window reports as seen, so stale replays are
    /// rejected regardless of how far the sequence has advanced since.
    pub(crate) fn weapon_sequence_seen(&self, sequence: u16) -> bool {
        let Some(latest) = self.latest_weapon_seq else {
            return false;
        };
        let ahead = sequence.wrapping_sub(latest);
        if ahead != 0 && ahead < 0x8000 {
            return false;
        }
        let behind = latest.wrapping_sub(sequence);
        if behind < WEAPON_SEQUENCE_WINDOW {
            self.weapon_seq_mask & (1u64 << behind) != 0
        } else {
            true
        }
    }

    pub(crate) fn remember_weapon_sequence(&mut self, sequence: u16) {
        let Some(latest) = self.latest_weapon_seq else {
            self.latest_weapon_seq = Some(sequence);
            self.weapon_seq_mask = 1;
            return;
        };
        let ahead = sequence.wrapping_sub(latest);
        if ahead != 0 && ahead < 0x8000 {
            self.weapon_seq_mask = if u32::from(ahead) >= u64::BITS {
                0
            } else {
                self.weapon_seq_mask << ahead
            };
            self.weapon_seq_mask |= 1;
            self.latest_weapon_seq = Some(sequence);
            return;
        }
        let behind = latest.wrapping_sub(sequence);
        if behind < WEAPON_SEQUENCE_WINDOW {
            self.weapon_seq_mask |= 1u64 << behind;
        }
        // Older than the window: nothing to record — `weapon_sequence_seen`
        // already reports stale sequences as seen.
    }

    /// Positions keep arriving while a player holds fire. Past the u16 half
    /// range (about nine minutes at 60 Hz), a fresh shot would compare as
    /// older than `latest_weapon_seq` and be dropped as a replay. Keep the
    /// window at most `WEAPON_SEQUENCE_MAX_LAG` behind the admitted position
    /// stream. Sliding forward marks nothing consumed and can only make more
    /// sequences report as seen, never fewer.
    pub(crate) fn keep_weapon_window_near(&mut self, position_sequence: u16) {
        let Some(latest) = self.latest_weapon_seq else {
            return;
        };
        let lag = position_sequence.wrapping_sub(latest);
        if lag <= WEAPON_SEQUENCE_MAX_LAG || lag >= 0x8000 {
            return;
        }
        let shift = lag - WEAPON_SEQUENCE_MAX_LAG;
        self.weapon_seq_mask = if u32::from(shift) >= u64::BITS {
            0
        } else {
            self.weapon_seq_mask << shift
        };
        self.latest_weapon_seq = Some(position_sequence.wrapping_sub(WEAPON_SEQUENCE_MAX_LAG));
    }

    pub(crate) fn portal_action_seen(&self, action_id: u32) -> bool {
        let Some(latest) = self.latest_portal_action_id else {
            return false;
        };
        let ahead = action_id.wrapping_sub(latest);
        if ahead != 0 && ahead < (1 << 31) {
            return false;
        }
        let behind = latest.wrapping_sub(action_id);
        if behind < u64::BITS {
            self.portal_action_mask & (1_u64 << behind) != 0
        } else {
            true
        }
    }

    pub(crate) fn remember_portal_action(
        &mut self,
        terminal: crate::portal_placement::PortalPlacementAccepted,
    ) {
        let action_id = terminal.action_id;
        self.remember_portal_action_id(action_id);
        self.portal_placement_terminals
            .retain(|known| known.action_id != action_id);
        self.portal_placement_terminals.push_back(terminal);
        while self.portal_placement_terminals.len() > 128 {
            self.portal_placement_terminals.pop_front();
        }
    }

    pub(crate) fn remember_portal_warp_action(
        &mut self,
        terminal: crate::portal_warp::PortalWarpAccepted,
    ) {
        let action_id = terminal.action_id;
        self.remember_portal_action_id(action_id);
        self.portal_warp_terminals
            .retain(|known| known.action_id != action_id);
        self.portal_warp_terminals.push_back(terminal);
        while self.portal_warp_terminals.len() > 128 {
            self.portal_warp_terminals.pop_front();
        }
    }

    fn remember_portal_action_id(&mut self, action_id: u32) {
        match self.latest_portal_action_id {
            None => {
                self.latest_portal_action_id = Some(action_id);
                self.portal_action_mask = 1;
            }
            Some(latest) => {
                let ahead = action_id.wrapping_sub(latest);
                if ahead != 0 && ahead < (1 << 31) {
                    self.portal_action_mask = if ahead >= u64::BITS {
                        0
                    } else {
                        self.portal_action_mask << ahead
                    };
                    self.portal_action_mask |= 1;
                    self.latest_portal_action_id = Some(action_id);
                } else {
                    let behind = latest.wrapping_sub(action_id);
                    if behind < u64::BITS {
                        self.portal_action_mask |= 1_u64 << behind;
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn reset_server_runtime_for_ship(&mut self) {
        self.reset_server_runtime_for_ship_at_tick(crate::wire::current_tick());
    }

    pub(crate) fn reset_server_runtime_for_ship_at_tick(&mut self, tick: u32) {
        self.server_bounty = 0;
        self.reset_server_loadout_for_ship_at_tick(tick);
        self.apply_server_initial_bounty_prizes(tick);
        if self.maxed_spawn_loadout && self.alive && self.player.ship < 8 {
            let ship = self.player.ship;
            self.server_max_energy = arena_ship_max_energy(ship);
            self.server_energy = self.server_max_energy;
            self.server_energy_remainder = 0.0;
            self.server_thrust_acceleration = arena_ship_max_thrust_pixels_per_second_squared(ship);
            self.server_top_speed = arena_ship_max_speed_pixels_per_second(ship);
            self.server_rotation_per_second = arena_ship_max_rotation_per_second(ship);
            self.server_recharge_per_second = arena_ship_max_recharge_per_second(ship);
            self.server_weapons = WeaponInventory::maximized(weapon_inventory_tuning(ship));
            self.server_status = StatusInventory::maximized(status_inventory_tuning(ship));
        }
        self.advance_server_prize_state_revision();
    }

    #[cfg(test)]
    pub(crate) fn reset_server_loadout_for_ship(&mut self) {
        self.reset_server_loadout_for_ship_at_tick(crate::wire::current_tick());
    }

    pub(crate) fn reset_server_loadout_for_ship_at_tick(&mut self, tick: u32) {
        self.last_damage_prize_loss_seq = None;
        self.last_damage_prize_loss_id = None;
        if self.alive && self.player.ship < 8 {
            self.server_max_energy = arena_ship_initial_energy(self.player.ship);
            self.server_energy = self.server_max_energy;
            self.server_energy_remainder = 0.0;
            self.last_energy_recharge_tick = Some(tick);
            self.last_continuous_energy_drain_tick = Some(tick);
            self.life_started_tick = Some(tick);
            self.reset_server_status_state();
            self.reset_server_weapon_state();
        } else {
            self.clear_server_runtime();
        }
        self.clear_server_timed_protections();
        self.clear_server_weapon_latches();
        self.reset_server_movement_stats();
        self.clear_life_scoped_observations();
    }

    fn apply_server_initial_bounty_prizes(&mut self, tick: u32) {
        if !self.alive || self.player.ship >= 8 {
            return;
        }

        let initial_bounty = arena_ship_initial_bounty(self.player.ship);
        let weights = arena_prize_weights();
        let weight_total = arena_prize_weight_total();
        for prize_id in initial_bounty_prize_ids(
            &weights,
            weight_total,
            initial_bounty as u32,
            SECURITY_PRIZE_SEED,
        ) {
            self.apply_server_prize_effect_at_tick(prize_id, tick);
        }
        self.server_bounty = initial_bounty;
    }

    pub(crate) fn clear_server_runtime(&mut self) {
        self.server_bounty = 0;
        self.last_damage_prize_loss_seq = None;
        self.last_damage_prize_loss_id = None;
        self.server_energy = 0;
        self.server_max_energy = 0;
        self.server_energy_remainder = 0.0;
        self.last_energy_recharge_tick = None;
        self.last_continuous_energy_drain_tick = None;
        self.clear_server_timed_protections();
        self.clear_server_status_state();
        self.clear_server_weapon_state();
        self.clear_server_weapon_latches();
        self.server_thrust_acceleration = 0.0;
        self.server_top_speed = 0.0;
        self.server_rotation_per_second = 0.0;
        self.server_recharge_per_second = 0.0;
        self.clear_life_scoped_observations();
    }

    pub(crate) fn clear_life_scoped_observations(&mut self) {
        // Frequency cutover and other life advances may intentionally preserve
        // inventory/loadout. Objective weapon locks and same-witness
        // consumption are authority-life state, never loadout state.
        self.objective_weapon_lock_until_tick = None;
        self.accepted_weapon_witnesses = Default::default();
        self.objective_ball_fire_witnesses = Default::default();
        self.last_input_seq = None;
        self.last_input_record = None;
        self.recent_input_records.clear();
        self.recent_input_rocket_assertions.clear();
        self.server_rocket_input_asserted = false;
        self.server_rocket_authorizations.clear();
        // Input and position sequences form one life-scoped replay stream.
        // Keeping the previous life's position sequence after clearing its
        // input history would make the first post-respawn position require an
        // impossible cross-life gap and permanently reject honest movement.
        self.last_position_sequence = None;
        self.canonical_position_witnesses.clear();
        self.canonical_movement_witness = None;
        self.latest_portal_action_id = None;
        self.portal_action_mask = 0;
        self.portal_placement_terminals.clear();
        self.portal_warp_terminals.clear();
        self.active_portal = None;
        self.server_warp_settle_until_tick = None;
        self.server_warp_bomb_lock_until_tick = None;
        self.life_position_sequence_base = None;
        self.pending_frequency_baseline = None;
        self.pending_warp_prize_relocations.clear();
        self.consumed_warp_prize_relocations.clear();
        self.position_history.clear();
        self.server_sim_state = None;
        self.server_sim_anchor_tick = None;
        self.server_sim_input_bits = None;
        self.pending_repel_observations.clear();
        self.retired_repel_applications.clear();
        self.movement_drift_x = 0.0;
        self.movement_drift_y = 0.0;
        if let Some(transaction) = self.pending_movement_correction.take() {
            let retired = crate::state::RetiredMovementCorrection::from_transaction(&transaction);
            if self.retired_movement_corrections.back().copied() != Some(retired) {
                self.retired_movement_corrections.push_back(retired);
            }
            while self.retired_movement_corrections.len()
                > crate::state::MAX_RETIRED_MOVEMENT_CORRECTIONS
            {
                self.retired_movement_corrections.pop_front();
            }
        }
        self.movement_correction_test_forced = false;
        self.aoi_tier_memory.clear();
        self.recent_damagers.clear();
        // Public WATCH_DAMAGE facts carry the exact victim/shooter lives in
        // their payload. Preserve them across this observer's soft life
        // cutover so a same-tick death, ship, or frequency transition cannot
        // erase already-accepted combat presentation. Private/source runtime
        // facts remain life-scoped. Arena leave/rejoin clears both queues at
        // the hard stream boundary.
        let session =
            crate::authority_identity::PlayerSessionIdentity::new(self.player.id, self.session_id);
        self.pending_snapshot_events.retain_for_session(session);
        self.pending_realtime_events
            .retain_public_combat_for_session(session);
    }

    /// End the current life-scoped Rocket authorization without refunding the
    /// inventory item that started it. Frequency cutover preserves the
    /// loadout but, like the reference game's Spawn(true), must not carry RocketTime or
    /// its raw-input edge latch into the next authority life.
    pub(crate) fn clear_server_rocket_lifecycle(&mut self) {
        self.server_rocket_until_tick = None;
        self.server_rocket_input_asserted = false;
        self.server_rocket_authorizations.clear();
    }

    pub(crate) fn position_sequence_belongs_to_active_life(&self, sequence: u16) -> bool {
        if let Some(base) = self.life_position_sequence_base {
            if self.last_position_sequence.is_none() {
                return sequence == base;
            }
            let boundary_window_active = self.last_position_sequence.is_none_or(|last| {
                let progress = last.wrapping_sub(base);
                progress < (1 << 15)
                    && progress <= crate::state::MAX_RECENT_INPUT_OBSERVATIONS as u16
            });
            let before_base = sequence != base && base.wrapping_sub(sequence) < (1 << 15);
            if boundary_window_active && before_base {
                return false;
            }
        }
        if let Some(last) = self.last_position_sequence {
            let age = last.wrapping_sub(sequence);
            if sequence != last
                && age < (1 << 15)
                && age as usize >= crate::state::MAX_RECENT_INPUT_OBSERVATIONS
            {
                return false;
            }
        }
        true
    }

    pub(crate) fn reset_server_movement_tracking_for_discontinuity(&mut self) {
        self.begin_actor_motion_segment();
        self.reset_server_movement_tracking_after_boundary();
    }

    pub(crate) fn reset_server_movement_tracking_for_impulse(&mut self) {
        let actor = crate::authority_identity::PlayerRuntimeIdentity::new(
            crate::authority_identity::PlayerSessionIdentity::new(self.player.id, self.session_id),
            crate::authority_identity::LifeIdentity::new(self.player.id, self.life_seq),
        );
        self.actor_motion_history
            .begin_impulse_segment(actor, self.arena_stream_id);
        self.reset_server_movement_tracking_after_boundary();
    }

    fn reset_server_movement_tracking_after_boundary(&mut self) {
        self.reset_private_movement_observer();
        self.canonical_movement_witness = None;
        if let Some(transaction) = self.pending_movement_correction.as_mut() {
            // Before application, the legal boundary wins and the durable
            // cancelled terminal releases an unchanged browser lineage. Once
            // application is committed, the client serializes every later
            // same-life Warp/door/Repel boundary behind its exact terminal,
            // so that outcome stays immutable.
            transaction.cancel_before_application();
        }
    }

    /// Rebase the independently opt-in diagnostics used by the default-on,
    /// configurable correction model. Delivery of an
    /// authenticated discontinuity can make that model stale before the
    /// exact application position arrives, but delivery alone is not
    /// movement authority and therefore must not clear canonical witnesses.
    pub(crate) fn reset_private_movement_observer(&mut self) {
        self.server_sim_state = None;
        self.server_sim_anchor_tick = None;
        self.server_sim_input_bits = None;
        self.movement_drift_x = 0.0;
        self.movement_drift_y = 0.0;
    }

    pub(crate) fn clear_server_weapon_latches(&mut self) {
        self.last_weapon_fire_tick = None;
        self.last_gun_fire_tick = None;
        self.last_bomb_fire_tick = None;
        self.weapon_cooldown_schedule = Default::default();
        self.objective_weapon_lock_until_tick = None;
        self.last_accepted_weapon_data = None;
        self.last_accepted_weapon_tick = None;
        self.accepted_weapon_witnesses = Default::default();
        self.objective_ball_fire_witnesses = Default::default();
        self.last_special_weapon_request = None;
        self.last_brick_drop_tick = None;
        self.pending_snapshot_weapon = 0;
    }

    pub(crate) fn objective_weapon_lock_active(&self, tick: u32) -> bool {
        self.objective_weapon_lock_until_tick
            .is_some_and(|until| !crate::continuum::continuum_tick_reached(tick, until))
    }

    pub(crate) fn accepted_weapon_witness_consumed(&self, sequence: u16) -> bool {
        self.accepted_weapon_witnesses.contains(sequence)
    }

    pub(crate) fn remember_accepted_weapon_witness(&mut self, sequence: u16) {
        self.accepted_weapon_witnesses.insert(sequence);
    }

    pub(crate) fn objective_ball_fire_witness_consumed(&self, sequence: u16) -> bool {
        self.objective_ball_fire_witnesses.contains(sequence)
    }

    pub(crate) fn remember_objective_ball_fire_witness(&mut self, sequence: u16) {
        self.objective_ball_fire_witnesses.insert(sequence);
    }

    pub(crate) fn extend_objective_weapon_lock(&mut self, tick: u32, delay_ticks: u32) {
        let candidate = crate::continuum::continuum_tick_add(tick, delay_ticks);
        self.objective_weapon_lock_until_tick = match self.objective_weapon_lock_until_tick {
            Some(current)
                if !crate::continuum::continuum_tick_reached(tick, current)
                    && crate::continuum::continuum_tick_diff(candidate, current) <= 0 =>
            {
                Some(current)
            }
            _ => Some(candidate),
        };
    }

    pub(crate) fn reset_server_weapon_state(&mut self) {
        self.server_weapons =
            WeaponInventory::initialized(weapon_inventory_tuning(self.player.ship));
    }

    pub(crate) fn clear_server_weapon_state(&mut self) {
        self.server_weapons = WeaponInventory::cleared();
    }

    pub(crate) fn reset_server_status_state(&mut self) {
        self.server_status =
            StatusInventory::initialized(status_inventory_tuning(self.player.ship));
    }

    pub(crate) fn clear_server_status_state(&mut self) {
        self.server_status = StatusInventory::cleared();
    }

    pub(crate) fn clear_server_timed_protections(&mut self) {
        self.server_super_until_tick = None;
        self.server_shield_until_tick = None;
        self.server_engine_shutdown_start_sequence = None;
        self.server_engine_shutdown_until_tick = None;
        self.cancel_pending_prize_effect_applications();
        self.server_emp_start_sequence = None;
        self.server_emp_until_tick = None;
        self.clear_server_rocket_lifecycle();
        self.server_repel_until_tick = None;
        self.server_repel_sequence_window = None;
        self.server_repel_observer_until_tick = None;
        self.pending_repel_observations.clear();
        self.retired_repel_applications.clear();
    }

    /// Whether the player is currently EMP-shutdown (recharge blocked).
    /// Mirrors the reference game's `ShipController:252-259` where `emped_time > 0`
    /// freezes recharge until the timer ticks down.
    pub(crate) fn server_emp_active(&self, tick: u32) -> bool {
        self.server_emp_until_tick
            .is_some_and(|until| !crate::continuum::continuum_tick_reached(tick, until))
    }

    pub(crate) fn server_engine_shutdown_active(&self, tick: u32) -> bool {
        self.server_prize_engine_shutdown_active(tick)
    }

    /// Prize-owned Shutdown is active only after its exact input boundary has
    /// been accepted. A stale start marker past the deadline is not an active
    /// semantic source for a later claim.
    pub(crate) fn server_prize_engine_shutdown_active(&self, tick: u32) -> bool {
        self.server_engine_shutdown_start_sequence.is_some()
            && self
                .server_engine_shutdown_until_tick
                .is_some_and(|until| !crate::continuum::continuum_tick_reached(tick, until))
    }

    /// Exact prize-owned movement deadline. EMP travels independently and
    /// blocks only recharge. A wrapped tick-zero deadline stays present.
    pub(crate) fn server_engine_shutdown_deadline_for_delivery(&self, tick: u32) -> Option<u32> {
        self.server_engine_shutdown_until_tick
            .filter(|until| !crate::continuum::continuum_tick_reached(tick, *until))
    }

    pub(crate) fn server_engine_shutdown_active_for_sequence(
        &self,
        tick: u32,
        sequence: u16,
    ) -> bool {
        let sequence_reached = |start: u16| sequence.wrapping_sub(start) < (1 << 15);
        self.server_engine_shutdown_until_tick
            .is_some_and(|until| !crate::continuum::continuum_tick_reached(tick, until))
            && self
                .server_engine_shutdown_start_sequence
                .is_some_and(sequence_reached)
    }

    /// Sequence-exact recharge suppression. EMP does not alter movement or
    /// weapon eligibility; prize Shutdown remains a separate semantic source.
    pub(crate) fn server_emp_active_for_sequence(&self, tick: u32, sequence: u16) -> bool {
        let sequence_reached = |start: u16| sequence.wrapping_sub(start) < (1 << 15);
        self.server_emp_active(tick) && self.server_emp_start_sequence.is_none_or(sequence_reached)
    }

    pub(crate) fn remember_prize_effect_application_terminal(
        &mut self,
        terminal: crate::prize_application::PrizeEffectAccepted,
    ) {
        self.prize_effect_application_terminals.retain(|known| {
            known.server_session_id != terminal.server_session_id
                || known.player_id != terminal.player_id
                || known.life_seq != terminal.life_seq
                || known.collectible_id != terminal.collectible_id
                || known.prize_id != terminal.prize_id
                || known.prize_state_revision != terminal.prize_state_revision
                || known.requested_application_sequence != terminal.requested_application_sequence
        });
        self.prize_effect_application_terminals.push_back(terminal);
        while self.prize_effect_application_terminals.len()
            > crate::state::MAX_PRIZE_EFFECT_APPLICATION_TERMINALS
        {
            self.prize_effect_application_terminals.pop_front();
        }
    }

    pub(crate) fn observe_prize_effect_application_boundary(&mut self, newest_sequence: u16) {
        for pending in &mut self.pending_prize_effect_applications {
            let candidate = newest_sequence.wrapping_add(1);
            if pending.maximum_application_sequence.is_none_or(|previous| {
                let advance = candidate.wrapping_sub(previous);
                advance != 0 && advance < (1 << 15)
            }) {
                // Every newly observed pre-application tail moves the exact
                // successor boundary, including across u16 wrap. Reordered
                // older tails cannot rewind it.
                pending.maximum_application_sequence = Some(candidate);
            }
        }

        let Some(head) = self.pending_prize_effect_applications.front().copied() else {
            return;
        };
        let Some(application) = head.deferred_application else {
            return;
        };
        let exact_boundary = head
            .maximum_application_sequence
            .or(head.minimum_application_sequence)
            .unwrap_or(application.application_sequence);
        let advance = exact_boundary.wrapping_sub(application.application_sequence);
        let outcome = if exact_boundary == application.application_sequence {
            crate::prize_application::PrizeEffectAccepted::OUTCOME_APPLIED
        } else if advance < (1 << 15) {
            // More old-semantics input than the client originally declared
            // has now arrived. Move only forward to its exact successor.
            crate::prize_application::PrizeEffectAccepted::OUTCOME_REBASE
        } else {
            return;
        };
        let terminal = crate::prize_application::PrizeEffectAccepted {
            server_session_id: application.server_session_id,
            player_id: application.player_id,
            life_seq: application.life_seq,
            collectible_id: application.collectible_id,
            prize_id: application.prize_id,
            prize_state_revision: application.prize_state_revision,
            requested_application_sequence: application.application_sequence,
            application_sequence: exact_boundary,
            outcome,
        };
        if outcome == crate::prize_application::PrizeEffectAccepted::OUTCOME_APPLIED {
            self.pending_prize_effect_applications.pop_front();
            if application.prize_id.checked_abs() == Some(14) {
                self.server_engine_shutdown_start_sequence
                    .get_or_insert(application.application_sequence);
            }
        } else if let Some(head) = self.pending_prize_effect_applications.front_mut() {
            head.deferred_application = None;
        }
        self.remember_prize_effect_application_terminal(terminal);
        crate::reliable::send_reliable_payload(self, &terminal.encode());
    }

    pub(crate) fn cancel_pending_prize_effect_applications(&mut self) {
        let pending = self
            .pending_prize_effect_applications
            .drain(..)
            .collect::<Vec<_>>();
        for pending in pending {
            let terminal = crate::prize_application::PrizeEffectAccepted {
                server_session_id: crate::wire::server_session_id(),
                player_id: self.player.id,
                life_seq: pending.life_seq,
                collectible_id: pending.collectible_id,
                prize_id: pending.prize_id,
                prize_state_revision: pending.prize_state_revision,
                requested_application_sequence: pending
                    .deferred_application
                    .map(|application| application.application_sequence)
                    .or(pending.latest_application_sequence)
                    .or(pending.maximum_application_sequence)
                    .or(pending.minimum_application_sequence)
                    .unwrap_or(0),
                application_sequence: pending
                    .deferred_application
                    .map(|application| application.application_sequence)
                    .or(pending.latest_application_sequence)
                    .or(pending.maximum_application_sequence)
                    .or(pending.minimum_application_sequence)
                    .unwrap_or(0),
                outcome: crate::prize_application::PrizeEffectAccepted::OUTCOME_CANCELLED,
            };
            crate::reliable::send_reliable_payload(self, &terminal.encode());
            self.remember_prize_effect_application_terminal(terminal);
        }
    }

    pub(crate) fn advance_server_prize_state_revision(&mut self) {
        self.server_prize_state_revision = self.server_prize_state_revision.wrapping_add(1).max(1);
    }

    #[cfg(test)]
    pub(crate) fn apply_server_prize_effect(&mut self, prize_id: i16) {
        self.apply_server_prize_effect_at_tick(prize_id, crate::wire::current_tick());
    }

    pub(crate) fn apply_server_prize_effect_at_tick(&mut self, prize_id: i16, tick: u32) {
        self.apply_one_server_prize_effect(prize_id, tick);

        // Multiprize expands into a fixed count of random sub-prizes using the
        // shared security seed. Keep this detached from hidden-prize RNG so
        // server-owned weapon/status state does not drift from the browser.
        if prize_id == MULTIPRIZE_PROTOCOL_ID {
            let weights = arena_prize_weights();
            let weight_total = arena_prize_weight_total();
            let multiprize_count = arena_multiprize_count();
            for sub_prize_id in multiprize_sub_prize_ids(
                &weights,
                weight_total,
                multiprize_count,
                SECURITY_PRIZE_SEED,
            ) {
                self.apply_one_server_prize_effect(sub_prize_id, tick);
            }
        }
        self.advance_server_prize_state_revision();
    }

    fn apply_one_server_prize_effect(&mut self, prize_id: i16, tick: u32) {
        let status_tuning = status_inventory_tuning(self.player.ship);
        let status_fallback = match prize_id.checked_abs() {
            Some(4) => status_full_charge_fallback(
                status_tuning.stealth_available,
                self.server_status.stealth_capable,
                prize_id < 0,
            ),
            Some(5) => status_full_charge_fallback(
                status_tuning.cloak_available,
                self.server_status.cloak_capable,
                prize_id < 0,
            ),
            Some(6) => status_full_charge_fallback(
                status_tuning.xradar_available,
                self.server_status.xradar_capable,
                prize_id < 0,
            ),
            Some(20) => status_full_charge_fallback(
                status_tuning.antiwarp_available,
                self.server_status.antiwarp_capable,
                prize_id < 0,
            ),
            _ => false,
        };
        if status_fallback {
            self.server_energy = if prize_id < 0 {
                1
            } else {
                self.server_max_energy
            };
            self.server_energy_remainder = 0.0;
        } else {
            self.apply_server_energy_prize_effect(prize_id);
            self.server_status
                .apply_prize_effect(status_tuning, prize_id);
        }
        self.apply_server_timed_prize_effect(prize_id, tick);
        self.apply_server_movement_upgrade_prize_effect(prize_id);
        self.server_weapons
            .apply_prize_effect(weapon_inventory_tuning(self.player.ship), prize_id);
    }

    fn reset_server_movement_stats(&mut self) {
        let ship = self.player.ship;
        if ship >= 8 {
            self.server_thrust_acceleration = 0.0;
            self.server_top_speed = 0.0;
            self.server_rotation_per_second = 0.0;
            self.server_recharge_per_second = 0.0;
            return;
        }
        self.server_thrust_acceleration = arena_ship_initial_thrust_pixels_per_second_squared(ship);
        self.server_top_speed = arena_ship_initial_top_speed_pixels_per_second(ship);
        self.server_rotation_per_second = arena_ship_initial_rotation_per_second(ship);
        self.server_recharge_per_second = arena_ship_initial_recharge_per_second(ship);
    }

    /// Mutate and clamp the live stat once, including partial-cap reversal.
    fn apply_server_movement_upgrade_prize_effect(&mut self, prize_id: i16) {
        let negative = prize_id < 0;
        let abs = match prize_id.checked_abs() {
            Some(value) => value,
            None => return,
        };
        let ship = self.player.ship;
        let (value, base, step, max) = match abs {
            ROTATION_PRIZE_PROTOCOL_ID => (
                &mut self.server_rotation_per_second,
                arena_ship_initial_rotation_per_second(ship),
                arena_ship_rotation_upgrade_per_second(ship),
                arena_ship_max_rotation_per_second(ship),
            ),
            THRUST_PRIZE_PROTOCOL_ID => (
                &mut self.server_thrust_acceleration,
                arena_ship_initial_thrust_pixels_per_second_squared(ship),
                arena_ship_thrust_upgrade_pixels_per_second_squared(ship),
                arena_ship_max_thrust_pixels_per_second_squared(ship),
            ),
            SPEED_PRIZE_PROTOCOL_ID => (
                &mut self.server_top_speed,
                arena_ship_initial_top_speed_pixels_per_second(ship),
                arena_ship_top_speed_upgrade_pixels_per_second(ship),
                arena_ship_max_speed_pixels_per_second(ship),
            ),
            RECHARGE_PRIZE_PROTOCOL_ID => (
                &mut self.server_recharge_per_second,
                arena_ship_initial_recharge_per_second(ship),
                arena_ship_recharge_upgrade_per_second(ship),
                arena_ship_max_recharge_per_second(ship),
            ),
            _ => return,
        };
        *value = crate::gameplay_rules::stat_value_after_prize(*value, base, step, max, negative);
    }

    fn apply_server_timed_prize_effect(&mut self, prize_id: i16, tick: u32) {
        match prize_id.checked_abs() {
            Some(SUPER_PRIZE_PROTOCOL_ID) => {
                let lifetime_ticks = arena_ship_super_lifetime_ticks(self.player.ship);
                if lifetime_ticks > 0 {
                    // The reference game consumes one RNG draw and retains the longer
                    // of the current Super interval and the new [0,max)
                    // duration. A zero roll neither starts nor shortens it.
                    let duration = crate::vie::vie_rng_next(&mut self.server_prize_effect_rng_seed)
                        % lifetime_ticks;
                    if duration > 0 {
                        let candidate = crate::continuum::continuum_tick_add(tick, duration);
                        let current_remaining = self.server_super_until_tick.map_or(0, |until| {
                            crate::continuum::continuum_tick_diff(until, tick).max(0) as u32
                        });
                        if duration > current_remaining {
                            self.server_super_until_tick = Some(candidate);
                        }
                    }
                }
            }
            Some(SHIELD_PRIZE_PROTOCOL_ID) => {
                let lifetime_ticks = arena_ship_shield_lifetime_ticks(self.player.ship);
                if lifetime_ticks > 0 {
                    self.server_shield_until_tick =
                        Some(crate::continuum::continuum_tick_add(tick, lifetime_ticks));
                }
            }
            Some(ENGINE_SHUTDOWN_PRIZE_PROTOCOL_ID) => {
                let base = arena_engine_shutdown_ticks();
                let duration = if prize_id < 0 {
                    base.saturating_mul(3)
                } else {
                    base
                };
                self.replace_server_engine_shutdown_at_tick(duration, tick);
            }
            _ => {}
        }
    }

    pub(crate) fn replace_server_engine_shutdown_at_tick(&mut self, duration: u32, tick: u32) {
        if duration > 0 && !self.server_prize_engine_shutdown_active(tick) {
            // A reliable recipient-private ACK establishes a browser barrier.
            // Canonical movement remains on the old lineage until its exact
            // PrizeEffectApplied sequence arrives.
            self.server_engine_shutdown_start_sequence = None;
        }
        self.server_engine_shutdown_until_tick =
            (duration > 0).then(|| crate::continuum::continuum_tick_add(tick, duration));
        if duration == 0 {
            self.server_engine_shutdown_start_sequence = None;
            self.cancel_pending_prize_effect_applications();
        }
    }

    /// Whether the player has a timed protection that gates the server's
    /// authoritative damage override. In the reference game, Super only zeroes firing
    /// energy cost — it does NOT reduce or block incoming damage. Only Shield
    /// provides damage protection. Treating Super as protection here would
    /// effectively make Super-holders invulnerable to authoritative damage
    /// normalization, which is wrong.
    #[allow(dead_code)]
    pub(crate) fn server_timed_damage_protection_active(&self, tick: u32) -> bool {
        self.server_shield_until_tick
            .is_some_and(|until| !crate::continuum::continuum_tick_reached(tick, until))
    }

    fn apply_server_energy_prize_effect(&mut self, prize_id: i16) {
        let negative = prize_id < 0;
        match prize_id.checked_abs() {
            Some(2) => {
                let initial = arena_ship_initial_energy(self.player.ship);
                let maximum = arena_ship_max_energy(self.player.ship);
                let step = arena_ship_energy_upgrade(self.player.ship);
                self.server_max_energy = adjusted_server_max_energy(
                    self.server_max_energy,
                    initial,
                    maximum,
                    step,
                    negative,
                );
                if negative && self.server_energy > self.server_max_energy {
                    // The exact private pickup ACK exposes this authoritative
                    // current value. Keep it as the bounded position-tail
                    // ceiling even if the next server recharge tick converges
                    // before the browser receives that ACK.
                    self.last_reported_energy = self.last_reported_energy.max(self.server_energy);
                }
            }
            Some(13) => {
                self.server_energy = if negative { 1 } else { self.server_max_energy };
                self.server_energy_remainder = 0.0;
            }
            Some(7) if negative => {
                self.server_energy = 1;
                self.server_energy_remainder = 0.0;
            }
            _ => {}
        }
    }
}

fn status_full_charge_fallback(available: bool, capable: bool, negative: bool) -> bool {
    !available && (!negative || !capable)
}

fn clear_reached_deadline(deadline: &mut Option<u32>, tick: u32) {
    if deadline.is_some_and(|deadline| crate::continuum::continuum_tick_reached(tick, deadline)) {
        *deadline = None;
    }
}

fn clear_elapsed_anchor(anchor: &mut Option<u32>, tick: u32, retention_ticks: u32) -> bool {
    if anchor.is_some_and(|anchor| {
        crate::continuum::continuum_tick_elapsed(tick, anchor) >= retention_ticks
    }) {
        *anchor = None;
        true
    } else {
        false
    }
}

fn adjusted_server_max_energy(
    current: u16,
    initial: u16,
    maximum: u16,
    step: u16,
    negative: bool,
) -> u16 {
    if negative {
        current.saturating_sub(step).max(initial)
    } else {
        current.saturating_add(step).min(maximum)
    }
}

pub(crate) fn weapon_inventory_tuning(ship: u8) -> WeaponInventoryTuning {
    WeaponInventoryTuning {
        initial_gun_level: arena_ship_initial_gun_level(ship),
        max_gun_level: arena_ship_max_gun_level(ship),
        initial_bomb_level: arena_ship_initial_bomb_level(ship),
        max_bomb_level: arena_ship_max_bomb_level(ship),
        shrapnel_rate: arena_ship_shrapnel_rate(ship),
        max_shrapnel: arena_ship_shrapnel_max(ship),
        initial_repels: arena_ship_initial_repel_count(ship),
        max_repels: arena_ship_max_repel_count(ship),
        initial_bursts: arena_ship_initial_burst_count(ship),
        max_bursts: arena_ship_max_burst_count(ship),
        initial_decoys: arena_ship_initial_decoy_count(ship),
        max_decoys: arena_ship_max_decoy_count(ship),
        initial_thors: arena_ship_initial_thor_count(ship),
        max_thors: arena_ship_max_thor_count(ship),
        initial_bricks: arena_ship_initial_brick_count(ship),
        max_bricks: arena_ship_max_brick_count(ship),
        initial_rockets: arena_ship_initial_rocket_count(ship),
        max_rockets: arena_ship_max_rocket_count(ship),
        initial_portals: arena_ship_initial_portal_count(ship),
        max_portals: arena_ship_max_portal_count(ship),
    }
}

pub(crate) fn status_inventory_tuning(ship: u8) -> StatusInventoryTuning {
    let stealth_status = arena_ship_stealth_status(ship);
    let cloak_status = arena_ship_cloak_status(ship);
    let xradar_status = arena_ship_xradar_status(ship);
    let antiwarp_status = arena_ship_antiwarp_status(ship);
    StatusInventoryTuning {
        stealth_available: stealth_status != 0,
        initial_stealth: stealth_status == 2,
        cloak_available: cloak_status != 0,
        initial_cloak: cloak_status == 2,
        xradar_available: xradar_status != 0,
        initial_xradar: xradar_status == 2,
        antiwarp_available: antiwarp_status != 0,
        initial_antiwarp: antiwarp_status == 2,
    }
}

#[cfg(test)]
mod weapon_sequence_tests {
    #[test]
    fn cooldown_metadata_outlives_short_projectiles_and_clears_safely_across_wrap() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let id = crate::arena::register_connection(&state, tx);
        let mut guard = state.lock().unwrap();
        let client = guard.clients.get_mut(&id).unwrap();
        for tick in [100, crate::continuum::CONTINUUM_TICK_MASK - 10] {
            client.last_weapon_fire_tick = Some(tick);
            client.last_gun_fire_tick = Some(tick);
            client.last_accepted_weapon_tick = Some(tick);
            client.last_accepted_weapon_data = Some(0x8041);
            for elapsed in [50, 999] {
                client.maintain_short_tick_state(
                    crate::continuum::continuum_tick_add(tick, elapsed),
                    1000,
                    150,
                    150,
                    50,
                );
                assert_eq!(client.last_gun_fire_tick, Some(tick));
                assert_eq!(client.last_accepted_weapon_tick, Some(tick));
                assert_eq!(client.last_accepted_weapon_data, Some(0x8041));
            }
            client.maintain_short_tick_state(
                crate::continuum::continuum_tick_add(tick, 1000),
                1000,
                150,
                150,
                50,
            );
            assert_eq!(client.last_gun_fire_tick, None);
            assert_eq!(client.last_accepted_weapon_tick, None);
            assert_eq!(client.last_accepted_weapon_data, None);
        }
    }

    #[test]
    fn emp_suppresses_recharge_without_becoming_prize_engine_shutdown() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let id = crate::arena::register_connection(&state, tx);
        let mut guard = state.lock().unwrap();
        let client = guard.clients.get_mut(&id).unwrap();
        for tick in [100, crate::continuum::CONTINUUM_TICK_MASK - 50] {
            let emp = crate::continuum::continuum_tick_add(tick, 300);
            let prize = crate::continuum::continuum_tick_add(tick, 100);
            client.server_engine_shutdown_until_tick = None;
            client.server_engine_shutdown_start_sequence = None;
            client.server_emp_until_tick = Some(emp);
            client.server_emp_start_sequence = Some(10);
            assert!(!client.server_emp_active_for_sequence(tick, 9));
            assert!(client.server_emp_active_for_sequence(tick, 10));
            assert!(!client.server_engine_shutdown_active(tick));
            assert!(!client.server_engine_shutdown_active_for_sequence(tick, 10));
            assert_eq!(
                client.server_engine_shutdown_deadline_for_delivery(tick),
                None
            );
            client.server_engine_shutdown_until_tick = Some(prize);
            client.server_engine_shutdown_start_sequence = Some(20);
            assert!(!client.server_engine_shutdown_active_for_sequence(tick, 19));
            assert!(client.server_engine_shutdown_active_for_sequence(tick, 20));
            assert_eq!(
                client.server_engine_shutdown_deadline_for_delivery(tick),
                Some(prize)
            );
            assert!(!client.server_engine_shutdown_active(prize));
            assert!(client.server_emp_active(prize));
            assert!(!client.server_emp_active(emp));
        }
    }

    #[test]
    fn repel_deadline_remains_bounded_replay_provenance_then_clears_across_wrap() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        let mut guard = state.lock().expect("server state");
        let client = guard.clients.get_mut(&player_id).expect("client present");
        let deadline = crate::continuum::CONTINUUM_TICK_MASK - 50;
        client.server_repel_until_tick = Some(deadline);

        for elapsed in [0, 1, 199] {
            client.maintain_short_tick_state(
                crate::continuum::continuum_tick_add(deadline, elapsed),
                1,
                1,
                1,
                1,
            );
            assert_eq!(client.server_repel_until_tick, Some(deadline));
        }
        client.maintain_short_tick_state(
            crate::continuum::continuum_tick_add(deadline, 200),
            1,
            1,
            1,
            1,
        );
        assert_eq!(client.server_repel_until_tick, None);
    }

    #[test]
    fn weapon_sequence_window_semantics() {
        // [REVIEW M3] fresh / replay / reorder / stale / far-jump / wrap.
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        let mut g = state.lock().expect("server state");
        let client = g.clients.get_mut(&player_id).expect("client present");

        // Fresh forward sequences are unseen, then remembered exactly once.
        assert!(!client.weapon_sequence_seen(100));
        client.remember_weapon_sequence(100);
        assert!(client.weapon_sequence_seen(100), "exact replay rejected");
        assert!(!client.weapon_sequence_seen(101));

        // Reorder within the window: a delayed mirror is accepted once.
        client.remember_weapon_sequence(105);
        assert!(!client.weapon_sequence_seen(102), "late mirror still fresh");
        client.remember_weapon_sequence(102);
        assert!(
            client.weapon_sequence_seen(102),
            "late mirror consumed once"
        );
        assert!(client.weapon_sequence_seen(105));
        assert!(
            client.weapon_sequence_seen(100),
            "window retains older bits"
        );

        // Stale: anything older than the 64-wide window is seen/rejected,
        // including after a far sequence jump (the old 512-ring hole).
        client.remember_weapon_sequence(105 + 600);
        assert!(
            client.weapon_sequence_seen(100),
            "replay after a >512 jump must stay rejected"
        );
        assert!(
            client.weapon_sequence_seen(105),
            "previously consumed sequence stays rejected after the jump"
        );
        assert!(
            client.weapon_sequence_seen(705 - 80),
            "never-consumed but beyond-window sequences are stale"
        );
        assert!(
            !client.weapon_sequence_seen(705 - 10),
            "never-consumed within-window sequences stay acceptable"
        );

        // Wrap across the u16 boundary.
        client.latest_weapon_seq = Some(65_500);
        client.weapon_seq_mask = 1;
        assert!(
            !client.weapon_sequence_seen(10),
            "forward across wrap is fresh"
        );
        client.remember_weapon_sequence(10);
        assert!(client.weapon_sequence_seen(10));
        assert!(
            client.weapon_sequence_seen(65_500),
            "pre-wrap consumed sequence still inside the window is rejected"
        );
        assert!(
            !client.weapon_sequence_seen(65_510),
            "pre-wrap never-consumed sequence inside the window stays acceptable"
        );
    }

    #[test]
    fn objective_weapon_consumption_ledgers_retain_intervening_and_wrapped_witnesses() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = crate::arena::register_connection(&state, tx);
        let mut guard = state.lock().expect("server state");
        let client = guard.clients.get_mut(&player_id).expect("client present");

        client.remember_accepted_weapon_witness(u16::MAX);
        client.remember_accepted_weapon_witness(0);
        client.remember_accepted_weapon_witness(1);
        assert!(client.accepted_weapon_witness_consumed(u16::MAX));
        assert!(client.accepted_weapon_witness_consumed(0));
        assert!(!client.accepted_weapon_witness_consumed(2));

        client.remember_objective_ball_fire_witness(40);
        client.remember_objective_ball_fire_witness(42);
        assert!(client.objective_ball_fire_witness_consumed(40));
        assert!(!client.objective_ball_fire_witness_consumed(41));
        client.clear_server_weapon_latches();
        assert!(!client.accepted_weapon_witness_consumed(u16::MAX));
        assert!(!client.objective_ball_fire_witness_consumed(40));
    }
}
