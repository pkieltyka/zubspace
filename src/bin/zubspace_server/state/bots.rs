use super::{MAX_SERVER_BOTS, PLAYER_BANNER_BYTES, PlayerSummary, SERVER_BOT_ID_BASE, ServerState};
use crate::authority::{StatusInventory, WeaponInventory};
use crate::authority_identity::{
    LifeIdentity, PlayerRuntimeIdentity, PlayerSessionId, PlayerSessionIdentity,
};
use crate::zone::server_spawn_coordinates;
use std::collections::VecDeque;

#[path = "bots/names.rs"]
mod names;

fn shuffled_xradar_assignments(count: usize, entropy: &[u8]) -> Vec<bool> {
    let mut indices = (0..count).collect::<Vec<_>>();
    for index in (1..count).rev() {
        let offset = index * 8;
        let random = u64::from_le_bytes(entropy[offset..offset + 8].try_into().unwrap());
        indices.swap(index, (random % (index as u64 + 1)) as usize);
    }
    let mut selected = vec![false; count];
    for index in indices.into_iter().take(count.div_ceil(2)) {
        selected[index] = true;
    }
    selected
}

const BOT_SESSION_ID_BASE: u64 = u64::MAX - MAX_SERVER_BOTS as u64;
#[cfg(debug_assertions)]
pub(crate) const E2E_STATIONARY_AFTER_HUMAN_DAMAGE_LEAD_TICKS: u32 = 90;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ServerBot {
    pub(crate) maxed_spawn_loadout: bool,
    pub(crate) player: PlayerSummary,
    pub(crate) session_id: PlayerSessionId,
    pub(crate) life_seq: u32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) velocity_x: f32,
    pub(crate) velocity_y: f32,
    pub(crate) orientation: f32,
    pub(crate) direction: u8,
    pub(crate) energy: u16,
    pub(crate) max_energy: u16,
    pub(crate) energy_remainder: f32,
    pub(crate) recharge_per_second: f32,
    pub(crate) thrust_pixels_per_second_squared: f32,
    pub(crate) top_speed_pixels_per_second: f32,
    pub(crate) rotation_per_second: f32,
    pub(crate) weapons: WeaponInventory,
    pub(crate) status: StatusInventory,
    /// Random roster assignment persists across lives; exhaustion turns the
    /// active toggle off until the next loadout reset.
    pub(crate) xradar_selected: bool,
    pub(crate) xradar: bool,
    pub(crate) bounty: u16,
    pub(crate) last_step_tick: u32,
    pub(crate) last_fire_tick: Option<u32>,
    pub(crate) last_fire_weapon_data: Option<u16>,
    pub(crate) consecutive_gun_volleys: u8,
    pub(crate) respawn_at_tick: Option<u32>,
    pub(crate) super_until_tick: Option<u32>,
    pub(crate) shield_until_tick: Option<u32>,
    pub(crate) engine_shutdown_until_tick: Option<u32>,
    pub(crate) emp_until_tick: Option<u32>,
    /// The reference game keeps the authored RepelSpeed as the ship's temporary
    /// movement cap for RepelTime after applying the radial impulse. Bots are
    /// authoritative actors too, so they retain the same server-tick window
    /// rather than having AI movement clamp the impulse to ordinary top speed.
    pub(crate) repel_until_tick: Option<u32>,
    pub(crate) objective_position_sequence: u16,
    pub(crate) prize_effect_rng_seed: i32,
    /// Hull-clear tile centers leading from the current authored safe region
    /// to combat space. The route is planned once and consumed waypoint by
    /// waypoint so a complex safe boundary cannot stall the 60 Hz tick.
    pub(crate) safe_egress_path: VecDeque<(u16, u16)>,
    /// Bounded tangent retained after wall contact so a moving target cannot
    /// reverse the bot's escape direction before it rounds the obstacle.
    pub(crate) wall_follow_until_tick: Option<u32>,
    pub(crate) wall_follow_x: i8,
    pub(crate) wall_follow_y: i8,
    /// Exact server-simulated history shared with human combat actors.
    pub(crate) actor_motion_history: crate::combat_timeline::ActorMotionHistory,
    /// Debug-E2E-only deterministic geometry fixture. Normal production bot
    /// lives always leave this unset. When configured, the named and newer
    /// lives still rotate, aim, fire, recharge, die, respawn, publish normal
    /// snapshots, and participate in the complete combat timeline, but stop
    /// translating after they have left authored safety. This lets genuine
    /// browser input prove stationary exact-life collision geometry in the
    /// same run whose predecessor life proves ordinary moving AI combat.
    #[cfg(debug_assertions)]
    pub(crate) e2e_stationary_from_life_seq: Option<u32>,
    /// Optional exact immediate-stationary lives for hybrid E2E scenarios.
    /// This keeps a moving predecessor eligible for the damage-armed fixture
    /// while only the named successor lives become deterministic geometry
    /// oracles. A later hostile successor therefore resumes ordinary pursuit
    /// instead of inheriting a permanent test-only movement freeze.
    #[cfg(debug_assertions)]
    pub(crate) e2e_stationary_static_life_seqs: Vec<u32>,
    /// Exact wrap-safe activation deadline for the first human-damaged life.
    /// A short moving lead lets the same authoritative life retain material
    /// collision-anchor motion before the deterministic stationary window.
    /// An explicitly configured `from_life_seq` fixture leaves this unset and
    /// therefore remains stationary immediately, preserving that contract.
    #[cfg(debug_assertions)]
    pub(crate) e2e_stationary_not_before_tick: Option<u32>,
    /// Debug-E2E-only trigger for the stationary geometry fixture. It arms
    /// independently on each life only after an ordinary authoritative hit
    /// from a current human runtime, so every exact life first proves real
    /// moving presentation/collision and then becomes a deterministic
    /// stationary target. Release servers do not contain this field or
    /// behavior.
    #[cfg(debug_assertions)]
    pub(crate) e2e_stationary_after_first_human_damage: bool,
    /// Debug-E2E-only exact lives on which the bot remains a normal damage,
    /// lifecycle, snapshot, and presentation target but does not create new
    /// weapons. This separates a stationary collision oracle from reciprocal
    /// hostile-fire coverage on its predecessor and successor lives. Release
    /// servers do not contain this field or behavior.
    #[cfg(debug_assertions)]
    pub(crate) e2e_passive_fire_life_seqs: Vec<u32>,
}

impl ServerBot {
    pub(crate) fn begin_actor_motion_segment(&mut self) {
        let actor = self.runtime_identity();
        self.actor_motion_history.begin_segment(
            actor,
            crate::combat_timeline::SERVER_BOT_MOTION_ARENA_STREAM_ID,
        );
    }

    pub(crate) fn begin_actor_impulse_segment(&mut self) {
        self.actor_motion_history.begin_impulse_segment(
            self.runtime_identity(),
            crate::combat_timeline::SERVER_BOT_MOTION_ARENA_STREAM_ID,
        );
    }

    pub(crate) fn append_actor_motion_sample(
        &mut self,
        server_tick: u32,
        safe: bool,
    ) -> crate::combat_timeline::HistoryAppendDisposition {
        let actor = self.runtime_identity();
        self.actor_motion_history.append_state(
            actor,
            crate::combat_timeline::SERVER_BOT_MOTION_ARENA_STREAM_ID,
            server_tick,
            self.x,
            self.y,
            self.velocity_x,
            self.velocity_y,
            self.orientation,
            crate::arena_settings::arena_ship_radius_pixels(self.player.ship),
            u8::from(safe),
            crate::combat_timeline::ActorMotionSourceKind::ServerBotSimulation,
        )
    }

    pub(crate) fn life_identity(&self) -> LifeIdentity {
        LifeIdentity::new(self.player.id, self.life_seq)
    }

    pub(crate) fn runtime_identity(&self) -> PlayerRuntimeIdentity {
        PlayerRuntimeIdentity::new(
            PlayerSessionIdentity::new(self.player.id, self.session_id),
            self.life_identity(),
        )
    }

    pub(crate) fn alive(&self) -> bool {
        self.respawn_at_tick.is_none() && self.player.ship < 8
    }

    pub(crate) fn e2e_stationary_motion_active(
        &self,
        in_safe_zone: bool,
        server_tick: u32,
    ) -> bool {
        #[cfg(debug_assertions)]
        {
            let configured_life_eligible =
                self.e2e_stationary_from_life_seq.is_some_and(|first_life| {
                    if self.e2e_stationary_after_first_human_damage {
                        self.life_seq == first_life && self.e2e_stationary_not_before_tick.is_some()
                    } else {
                        self.life_seq == first_life
                            || self.life_seq.wrapping_sub(first_life) < (1_u32 << 31)
                    }
                });
            let damage_armed = configured_life_eligible
                && self.e2e_stationary_not_before_tick.is_none_or(|deadline| {
                    crate::continuum::continuum_tick_reached(server_tick, deadline)
                });
            let static_life_active = self
                .e2e_stationary_static_life_seqs
                .contains(&self.life_seq);
            return !in_safe_zone && (damage_armed || static_life_active);
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = (in_safe_zone, server_tick);
            false
        }
    }

    pub(crate) fn arm_e2e_stationary_after_human_damage(&mut self, server_tick: u32) -> bool {
        #[cfg(debug_assertions)]
        {
            let static_life_active = self
                .e2e_stationary_static_life_seqs
                .contains(&self.life_seq);
            let life_eligible = self.e2e_stationary_from_life_seq.is_none_or(|first_life| {
                self.life_seq == first_life
                    || self.life_seq.wrapping_sub(first_life) < (1_u32 << 31)
            });
            let already_armed_on_life = self.e2e_stationary_from_life_seq == Some(self.life_seq)
                && self.e2e_stationary_not_before_tick.is_some();
            if self.e2e_stationary_after_first_human_damage
                && !static_life_active
                && life_eligible
                && !already_armed_on_life
            {
                self.e2e_stationary_from_life_seq = Some(self.life_seq);
                self.e2e_stationary_not_before_tick = Some(crate::continuum::continuum_tick_add(
                    server_tick,
                    E2E_STATIONARY_AFTER_HUMAN_DAMAGE_LEAD_TICKS,
                ));
                return true;
            }
            false
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = server_tick;
            false
        }
    }

    pub(crate) fn e2e_passive_fire_active(&self) -> bool {
        #[cfg(debug_assertions)]
        {
            return self.e2e_passive_fire_life_seqs.contains(&self.life_seq);
        }
        #[cfg(not(debug_assertions))]
        false
    }

    pub(crate) fn session_id_for_wire(&self) -> u64 {
        self.session_id.get()
    }

    pub(crate) fn reset_loadout_at_tick(&mut self, tick: u32) {
        let ship = self.player.ship;
        self.max_energy = crate::arena_settings::arena_ship_initial_energy(ship);
        self.energy = self.max_energy;
        self.energy_remainder = 0.0;
        self.recharge_per_second =
            crate::arena_settings::arena_ship_initial_recharge_per_second(ship);
        self.thrust_pixels_per_second_squared =
            crate::arena_settings::arena_ship_initial_thrust_pixels_per_second_squared(ship);
        self.top_speed_pixels_per_second =
            crate::arena_settings::arena_ship_initial_top_speed_pixels_per_second(ship);
        self.rotation_per_second =
            crate::arena_settings::arena_ship_initial_rotation_per_second(ship);
        self.weapons =
            WeaponInventory::initialized(super::client_runtime::weapon_inventory_tuning(ship));
        self.status =
            StatusInventory::initialized(super::client_runtime::status_inventory_tuning(ship));
        if self.maxed_spawn_loadout {
            self.max_energy = crate::arena_settings::arena_ship_max_energy(ship);
            self.energy = self.max_energy;
            self.recharge_per_second =
                crate::arena_settings::arena_ship_max_recharge_per_second(ship);
            self.thrust_pixels_per_second_squared =
                crate::arena_settings::arena_ship_max_thrust_pixels_per_second_squared(ship);
            self.top_speed_pixels_per_second =
                crate::arena_settings::arena_ship_max_speed_pixels_per_second(ship);
            self.rotation_per_second =
                crate::arena_settings::arena_ship_max_rotation_per_second(ship);
            self.weapons =
                WeaponInventory::maximized(super::client_runtime::weapon_inventory_tuning(ship));
            self.status =
                StatusInventory::maximized(super::client_runtime::status_inventory_tuning(ship));
        }
        self.xradar = self.xradar_selected && self.status.xradar_capable;
        self.bounty = crate::arena_settings::arena_ship_initial_bounty(ship);
        self.last_step_tick = tick;
        self.last_fire_tick = None;
        self.last_fire_weapon_data = None;
        self.consecutive_gun_volleys = 0;
        self.super_until_tick = None;
        self.shield_until_tick = None;
        self.engine_shutdown_until_tick = None;
        self.emp_until_tick = None;
        self.repel_until_tick = None;
        self.player.flags = 0;
        self.safe_egress_path.clear();
        self.wall_follow_until_tick = None;
        self.wall_follow_x = 0;
        self.wall_follow_y = 0;
    }

    pub(crate) fn advance_life(&mut self) -> u32 {
        self.life_seq = self.life_seq.wrapping_add(1).max(1);
        self.begin_actor_motion_segment();
        self.life_seq
    }

    pub(crate) fn maintain_timers(&mut self, tick: u32) {
        for deadline in [
            &mut self.super_until_tick,
            &mut self.shield_until_tick,
            &mut self.engine_shutdown_until_tick,
            &mut self.repel_until_tick,
        ] {
            if deadline
                .is_some_and(|deadline| crate::continuum::continuum_tick_reached(tick, deadline))
            {
                *deadline = None;
            }
        }
    }

    pub(crate) fn shutdown_active(&self, tick: u32) -> bool {
        self.engine_shutdown_until_tick
            .is_some_and(|deadline| !crate::continuum::continuum_tick_reached(tick, deadline))
    }

    pub(crate) fn emp_active(&self, tick: u32) -> bool {
        self.emp_until_tick
            .is_some_and(|deadline| !crate::continuum::continuum_tick_reached(tick, deadline))
    }

    pub(crate) fn repel_active(&self, tick: u32) -> bool {
        self.repel_until_tick
            .is_some_and(|deadline| !crate::continuum::continuum_tick_reached(tick, deadline))
    }

    pub(crate) fn gun_weapon_data(&self) -> Option<u16> {
        (self.weapons.gun_level > 0).then(|| {
            let weapon_type = if self.weapons.bouncing_bullets { 2 } else { 1 };
            let level = u16::from(crate::continuum_weapon::weapon_level_to_bits(
                self.weapons.gun_level,
            ));
            // Bots retain their upgraded inventory, but never activate multifire:
            // the extra spread overwhelms the owner-requested free-for-all.
            // Keep this decision at firing time so prize pickups cannot enable it.
            weapon_type | (level << 5)
        })
    }

    pub(crate) fn bomb_weapon_data(&self) -> Option<u16> {
        (self.weapons.bomb_level > 0).then(|| {
            crate::gameplay_rules::encode_weapon_data(
                if self.weapons.proximity_bombs { 4 } else { 3 },
                self.weapons.bomb_level,
                self.weapons.bouncing_bullets,
                self.weapons.bomb_level,
                self.weapons.shrapnel,
                false,
            )
        })
    }

    /// Applies the bot-relevant portion of the reference game's signed prize state.
    /// Returns true for a positive Warp, whose map-dependent relocation is
    /// completed by the server bot tick.
    pub(crate) fn apply_prize_at_tick(&mut self, prize_id: i16, tick: u32) -> bool {
        let negative = prize_id < 0;
        if !negative {
            self.bounty = self.bounty.saturating_add(1);
        }
        let warp = self.apply_one_prize_at_tick(prize_id, tick);
        if prize_id == 25 {
            let weights = crate::arena_settings::arena_prize_weights();
            let weight_total = crate::arena_settings::arena_prize_weight_total();
            let count = crate::arena_settings::arena_multiprize_count();
            for sub_prize in crate::authority_prizes::multiprize_sub_prize_ids(
                &weights,
                weight_total,
                count,
                crate::authority_prizes::SECURITY_PRIZE_SEED,
            ) {
                self.apply_one_prize_at_tick(sub_prize, tick);
            }
        }
        warp
    }

    fn apply_one_prize_at_tick(&mut self, prize_id: i16, tick: u32) -> bool {
        let negative = prize_id < 0;
        let Some(kind) = prize_id.checked_abs() else {
            return false;
        };
        let ship = self.player.ship;
        let status_tuning = super::client_runtime::status_inventory_tuning(ship);
        let status_fallback = match kind {
            4 => !status_tuning.stealth_available && (!negative || !self.status.stealth_capable),
            5 => !status_tuning.cloak_available && (!negative || !self.status.cloak_capable),
            6 => !status_tuning.xradar_available && (!negative || !self.status.xradar_capable),
            20 => !status_tuning.antiwarp_available && (!negative || !self.status.antiwarp_capable),
            _ => false,
        };
        if status_fallback {
            self.energy = if negative { 1 } else { self.max_energy };
            self.energy_remainder = 0.0;
        } else {
            self.status.apply_prize_effect(status_tuning, prize_id);
            if !self.status.xradar_capable {
                self.xradar = false;
            }
        }
        match kind {
            1 => {
                self.recharge_per_second = crate::gameplay_rules::stat_value_after_prize(
                    self.recharge_per_second,
                    crate::arena_settings::arena_ship_initial_recharge_per_second(ship),
                    crate::arena_settings::arena_ship_recharge_upgrade_per_second(ship),
                    crate::arena_settings::arena_ship_max_recharge_per_second(ship),
                    negative,
                );
            }
            2 => {
                let base = crate::arena_settings::arena_ship_initial_energy(ship);
                let maximum = crate::arena_settings::arena_ship_max_energy(ship);
                let step = crate::arena_settings::arena_ship_energy_upgrade(ship);
                self.max_energy = if negative {
                    self.max_energy.saturating_sub(step).max(base)
                } else {
                    self.max_energy.saturating_add(step).min(maximum)
                };
            }
            3 => {
                self.rotation_per_second = crate::gameplay_rules::stat_value_after_prize(
                    self.rotation_per_second,
                    crate::arena_settings::arena_ship_initial_rotation_per_second(ship),
                    crate::arena_settings::arena_ship_rotation_upgrade_per_second(ship),
                    crate::arena_settings::arena_ship_max_rotation_per_second(ship),
                    negative,
                );
            }
            7 => {
                if negative {
                    self.energy = 1;
                    self.energy_remainder = 0.0;
                }
                return !negative;
            }
            11 => {
                self.thrust_pixels_per_second_squared =
                    crate::gameplay_rules::stat_value_after_prize(
                        self.thrust_pixels_per_second_squared,
                        crate::arena_settings::arena_ship_initial_thrust_pixels_per_second_squared(
                            ship,
                        ),
                        crate::arena_settings::arena_ship_thrust_upgrade_pixels_per_second_squared(
                            ship,
                        ),
                        crate::arena_settings::arena_ship_max_thrust_pixels_per_second_squared(
                            ship,
                        ),
                        negative,
                    );
            }
            12 => {
                self.top_speed_pixels_per_second = crate::gameplay_rules::stat_value_after_prize(
                    self.top_speed_pixels_per_second,
                    crate::arena_settings::arena_ship_initial_top_speed_pixels_per_second(ship),
                    crate::arena_settings::arena_ship_top_speed_upgrade_pixels_per_second(ship),
                    crate::arena_settings::arena_ship_max_speed_pixels_per_second(ship),
                    negative,
                );
            }
            13 => {
                self.energy = if negative { 1 } else { self.max_energy };
                self.energy_remainder = 0.0;
            }
            14 => {
                let base = crate::arena_settings::arena_engine_shutdown_ticks();
                let duration = if negative {
                    base.saturating_mul(3)
                } else {
                    base
                };
                self.engine_shutdown_until_tick =
                    (duration > 0).then(|| crate::continuum::continuum_tick_add(tick, duration));
            }
            17 => {
                let duration = crate::arena_settings::arena_ship_super_lifetime_ticks(ship);
                if duration > 0 {
                    let rolled =
                        crate::vie::vie_rng_next(&mut self.prize_effect_rng_seed) % duration;
                    let current_remaining = self.super_until_tick.map_or(0, |deadline| {
                        crate::continuum::continuum_tick_diff(deadline, tick).max(0) as u32
                    });
                    if rolled > current_remaining {
                        self.super_until_tick =
                            Some(crate::continuum::continuum_tick_add(tick, rolled));
                    }
                }
            }
            18 => {
                let duration = crate::arena_settings::arena_ship_shield_lifetime_ticks(ship);
                self.shield_until_tick =
                    (duration > 0).then(|| crate::continuum::continuum_tick_add(tick, duration));
            }
            _ => {}
        }
        self.weapons.apply_prize_effect(
            super::client_runtime::weapon_inventory_tuning(ship),
            prize_id,
        );
        false
    }

    pub(crate) fn position_status_bits(&self) -> u8 {
        if self.xradar {
            crate::authority::POSITION_STATUS_XRADAR
        } else {
            0
        }
    }

    pub(crate) fn advance_energy_at_tick(&mut self, elapsed_ticks: u32, tick: u32) {
        let mut pool =
            crate::authority::EnergyPool::new(self.energy, self.max_energy, self.energy_remainder);
        // Keep an expired EMP deadline until this energy step so a delayed
        // callback cannot recharge ticks that were still inside its interval.
        let rechargeable_ticks = self
            .emp_until_tick
            .map_or(elapsed_ticks, |deadline| {
                crate::continuum::continuum_tick_diff(tick, deadline).max(0) as u32
            })
            .min(elapsed_ticks);
        pool.recharge_for_position_ticks(rechargeable_ticks, self.recharge_per_second);
        if !self.emp_active(tick) {
            self.emp_until_tick = None;
        }
        if self.xradar
            && (!self.status.xradar_capable
                || !pool.spend_continuous_energy(
                    crate::arena_settings::arena_ship_xradar_energy_per_second(self.player.ship)
                        * crate::continuum::ticks_to_seconds_u32(elapsed_ticks),
                ))
        {
            self.xradar = false;
        }
        self.energy = pool.current();
        self.energy_remainder = pool.recharge_remainder();
    }

    pub(crate) fn large_position_packet(&self) -> Vec<u8> {
        use crate::continuum_position::protocol_velocity_component;
        let x = self.x.round().clamp(0.0, u16::MAX as f32) as u16;
        let y = self.y.round().clamp(0.0, u16::MAX as f32) as u16;
        let velocity_x = protocol_velocity_component(self.velocity_x);
        let velocity_y = protocol_velocity_component(self.velocity_y);

        let mut payload = vec![0; 23];
        payload[0] = crate::arena::LARGE_POSITION_PACKET;
        payload[1] = self.direction;
        // Legacy position encoding and snapshots share the canonical server
        // clock. A separately accumulated step counter drifts and can make
        // fresh snapshots fail the client's half-range timestamp admission.
        payload[2..4].copy_from_slice(&(self.last_step_tick as u16).to_le_bytes());
        payload[4..6].copy_from_slice(&x.to_le_bytes());
        payload[6..8].copy_from_slice(&velocity_y.to_le_bytes());
        payload[8..10].copy_from_slice(&self.player.id.to_le_bytes());
        payload[10..12].copy_from_slice(&velocity_x.to_le_bytes());
        payload[13] = self.position_status_bits();
        payload[15..17].copy_from_slice(&y.to_le_bytes());
        payload[17..19].copy_from_slice(&self.bounty.to_le_bytes());
        payload[19..21].copy_from_slice(&0_u16.to_le_bytes());
        payload[21..23].copy_from_slice(&self.energy.to_le_bytes());
        payload
    }
}

impl ServerState {
    pub(crate) fn initialize_configured_bots(&mut self) {
        self.bots.clear();
        let available = crate::identity::arena_player_limit(self)
            .saturating_sub(crate::identity::reserved_human_count(self, None));
        let count = self.config.bot_count.min(MAX_SERVER_BOTS).min(available);
        let reserved_names = self
            .clients
            .values()
            .filter(|client| client.joined || client.arena_admission_reserved)
            .map(|client| crate::identity::name_key(&client.player.name))
            .collect();
        if count == 0 {
            return;
        }
        let map = self.zone_map();
        let (spawn_x, spawn_y) = server_spawn_coordinates(map);
        #[cfg(debug_assertions)]
        let e2e_stationary_from_life_seq = debug_e2e_stationary_bot_from_life_seq();
        #[cfg(debug_assertions)]
        let e2e_stationary_static_life_seqs = debug_e2e_stationary_bot_static_life_seqs();
        #[cfg(debug_assertions)]
        let e2e_stationary_after_first_human_damage =
            debug_e2e_stationary_bot_after_first_human_damage();
        #[cfg(debug_assertions)]
        let e2e_passive_fire_life_seqs = debug_e2e_passive_bot_life_seqs();
        let mut detector_entropy = vec![0u8; count * 8];
        if getrandom::fill(&mut detector_entropy).is_err() {
            tracing::warn!(
                "bot X-radar shuffle unavailable; using deterministic half-roster assignment"
            );
        }
        let detector_assignments = shuffled_xradar_assignments(count, &detector_entropy);
        for (index, name) in names::bot_names_avoiding(count, &reserved_names)
            .into_iter()
            .enumerate()
        {
            let id = SERVER_BOT_ID_BASE + index as u16;
            let angle = index as f32 * 0.9;
            let radius = 96.0 + (index % 5) as f32 * 32.0;
            let tangent = angle + std::f32::consts::FRAC_PI_2;
            let mut bot = ServerBot {
                maxed_spawn_loadout: self.config.maxed_spawn_loadout,
                player: PlayerSummary {
                    id,
                    name,
                    ship: self
                        .config
                        .bot_ship_types
                        .get(index % self.config.bot_ship_types.len().max(1))
                        .copied()
                        .unwrap_or(0),
                    frequency: id,
                    assists: 0,
                    kill_points: 0,
                    flag_points: 0,
                    wins: 0,
                    losses: 0,
                    flags: 0,
                    banner: [0; PLAYER_BANNER_BYTES],
                },
                session_id: PlayerSessionId::new(BOT_SESSION_ID_BASE + index as u64 + 1),
                life_seq: 1,
                x: spawn_x as f32 + angle.cos() * radius,
                y: spawn_y as f32 + angle.sin() * radius,
                velocity_x: tangent.cos() * 60.0,
                velocity_y: tangent.sin() * 60.0,
                orientation: 0.0,
                direction: 0,
                energy: 0,
                max_energy: 0,
                energy_remainder: 0.0,
                recharge_per_second: 0.0,
                thrust_pixels_per_second_squared: 0.0,
                top_speed_pixels_per_second: 0.0,
                rotation_per_second: 0.0,
                weapons: WeaponInventory::cleared(),
                status: StatusInventory::cleared(),
                xradar_selected: detector_assignments[index],
                xradar: false,
                bounty: 0,
                last_step_tick: 0,
                last_fire_tick: None,
                last_fire_weapon_data: None,
                consecutive_gun_volleys: 0,
                respawn_at_tick: None,
                super_until_tick: None,
                shield_until_tick: None,
                engine_shutdown_until_tick: None,
                emp_until_tick: None,
                repel_until_tick: None,
                objective_position_sequence: 0,
                prize_effect_rng_seed: crate::authority_prizes::SECURITY_PRIZE_SEED,
                safe_egress_path: VecDeque::new(),
                wall_follow_until_tick: None,
                wall_follow_x: 0,
                wall_follow_y: 0,
                actor_motion_history: crate::combat_timeline::ActorMotionHistory::new(),
                #[cfg(debug_assertions)]
                e2e_stationary_from_life_seq,
                #[cfg(debug_assertions)]
                e2e_stationary_static_life_seqs: e2e_stationary_static_life_seqs.clone(),
                #[cfg(debug_assertions)]
                e2e_stationary_not_before_tick: None,
                #[cfg(debug_assertions)]
                e2e_stationary_after_first_human_damage,
                #[cfg(debug_assertions)]
                e2e_passive_fire_life_seqs: e2e_passive_fire_life_seqs.clone(),
            };
            bot.reset_loadout_at_tick(0);
            self.bots.push(bot);
        }
        tracing::info!(
            bot_count = self.bots.len(),
            xradar_bot_ids = ?self.bots.iter().filter(|bot| bot.xradar).map(|bot| bot.player.id).collect::<Vec<_>>(),
            "bot detector roster initialized"
        );
    }

    pub(crate) fn bot(&self, player_id: u16) -> Option<&ServerBot> {
        self.bots.iter().find(|bot| bot.player.id == player_id)
    }

    pub(crate) fn bot_mut(&mut self, player_id: u16) -> Option<&mut ServerBot> {
        self.bots.iter_mut().find(|bot| bot.player.id == player_id)
    }

    pub(crate) fn active_actor_runtime_identity(
        &self,
        player_id: u16,
    ) -> Option<PlayerRuntimeIdentity> {
        self.active_player_runtime_identity(player_id).or_else(|| {
            self.bot(player_id)
                .filter(|bot| bot.alive())
                .map(ServerBot::runtime_identity)
        })
    }

    pub(crate) fn active_actor_motion_segment(
        &self,
        player_id: u16,
    ) -> Option<crate::combat_timeline::MotionSegmentIdentity> {
        let runtime = self.active_actor_runtime_identity(player_id)?;
        if let Some(client) = self.clients.get(&player_id) {
            return client
                .actor_motion_history
                .current_segment()
                .filter(|segment| segment.actor == runtime);
        }
        self.bot(player_id)?
            .actor_motion_history
            .current_segment()
            .filter(|segment| segment.actor == runtime)
    }

    pub(crate) fn actor_runtime_matches(&self, identity: PlayerRuntimeIdentity) -> bool {
        self.player_runtime_matches(identity)
            || self
                .bot(identity.player_id())
                .is_some_and(|bot| bot.alive() && bot.runtime_identity() == identity)
    }
}

#[cfg(debug_assertions)]
fn debug_e2e_stationary_bot_from_life_seq() -> Option<u32> {
    let value = std::env::var("ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ").ok()?;
    let life_seq = value.parse::<u32>().unwrap_or_else(|_| {
        panic!(
            "ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ must be a nonzero u32, got {value:?}"
        )
    });
    assert!(
        life_seq != 0,
        "ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ must be nonzero"
    );
    Some(life_seq)
}

#[cfg(debug_assertions)]
fn debug_e2e_stationary_bot_static_life_seqs() -> Vec<u32> {
    debug_e2e_life_seq_list("ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ")
}

#[cfg(debug_assertions)]
fn debug_e2e_stationary_bot_after_first_human_damage() -> bool {
    let Ok(value) = std::env::var("ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE") else {
        return false;
    };
    match value.as_str() {
        "0" | "false" => false,
        "1" | "true" => true,
        _ => panic!(
            "ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE must be 0, 1, false, or true, got {value:?}"
        ),
    }
}

#[cfg(debug_assertions)]
fn debug_e2e_passive_bot_life_seqs() -> Vec<u32> {
    debug_e2e_life_seq_list("ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ")
}

#[cfg(debug_assertions)]
fn debug_e2e_life_seq_list(variable: &str) -> Vec<u32> {
    let Some(value) = std::env::var(variable).ok() else {
        return Vec::new();
    };
    let life_seqs = value
        .split(',')
        .map(|component| {
            component.trim().parse::<u32>().unwrap_or_else(|_| {
                panic!("{variable} must contain comma-separated nonzero u32 values, got {value:?}")
            })
        })
        .collect::<Vec<_>>();
    assert!(
        !life_seqs.is_empty() && life_seqs.len() <= 8,
        "{variable} must contain 1..=8 values"
    );
    assert!(
        life_seqs
            .windows(2)
            .all(|pair| pair[0] != 0 && pair[0] < pair[1])
            && life_seqs.last().is_some_and(|life_seq| *life_seq != 0),
        "{variable} values must be nonzero, unique, and increasing"
    );
    life_seqs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_xradar_assignment_is_balanced_randomized_and_stable_across_respawn() {
        for count in 0..=MAX_SERVER_BOTS {
            for byte in [0, 91, 255] {
                let selected = shuffled_xradar_assignments(count, &vec![byte; count * 8]);
                assert_eq!(selected.len(), count);
                assert_eq!(
                    selected.iter().filter(|active| **active).count(),
                    count.div_ceil(2)
                );
            }
        }
        assert_ne!(
            shuffled_xradar_assignments(15, &[0; 120]),
            shuffled_xradar_assignments(15, &[255; 120])
        );
        let mut state = ServerState {
            config: crate::config::ServerConfig {
                bot_count: 15,
                maxed_spawn_loadout: true,
                ..Default::default()
            },
            ..Default::default()
        };
        state.initialize_configured_bots();
        assert_eq!(state.bots.iter().filter(|bot| bot.xradar).count(), 8);
        for bot in &mut state.bots {
            let selected = bot.xradar_selected;
            assert_eq!(
                bot.large_position_packet()[13] & crate::authority::POSITION_STATUS_XRADAR != 0,
                selected
            );
            bot.advance_life();
            bot.reset_loadout_at_tick(100);
            assert_eq!(bot.xradar_selected, selected);
            assert_eq!(bot.xradar, selected);
        }
    }

    #[test]
    fn bot_xradar_spends_energy_after_recharge_and_shuts_off_when_unaffordable() {
        let mut bot = configured_bot();
        bot.xradar_selected = true;
        bot.status.xradar_capable = true;
        bot.xradar = true;
        let cost = crate::arena_settings::arena_ship_xradar_energy_per_second(bot.player.ship);
        assert!(cost > 0.0);
        bot.recharge_per_second = 0.0;
        bot.energy = 100;
        bot.energy_remainder = 0.0;
        bot.advance_energy_at_tick(100, 100);
        assert!((bot.energy as f32 + bot.energy_remainder - (100.0 - cost)).abs() < 0.001);
        bot.energy = 1;
        bot.energy_remainder = 0.0;
        bot.advance_energy_at_tick(100, 200);
        assert!(!bot.xradar);
        assert_eq!(bot.energy, 1);
        assert_eq!(
            bot.position_status_bits() & crate::authority::POSITION_STATUS_XRADAR,
            0
        );
        bot.reset_loadout_at_tick(300);
        assert!(bot.xradar);
        bot.emp_until_tick = Some(1000);
        bot.energy = 100;
        bot.advance_energy_at_tick(100, 400);
        assert!((bot.energy as f32 + bot.energy_remainder - (100.0 - cost)).abs() < 0.001);
        bot.status.xradar_capable = false;
        let energy = bot.energy;
        bot.advance_energy_at_tick(1, 401);
        assert!(!bot.xradar);
        assert_eq!(bot.energy, energy);
    }

    fn configured_bot() -> ServerBot {
        let mut state = ServerState {
            config: crate::config::ServerConfig {
                bot_count: 1,
                ..crate::config::ServerConfig::default()
            },
            ..ServerState::default()
        };
        state.initialize_configured_bots();
        state.bots.remove(0)
    }

    #[test]
    fn bot_emp_pauses_recharge_independently_of_prize_shutdown_across_wrap() {
        for tick in [100, crate::continuum::CONTINUUM_TICK_MASK - 150] {
            let mut bot = configured_bot();
            bot.xradar = false;
            bot.recharge_per_second = 100.0;
            bot.energy = 500;
            bot.energy_remainder = 0.0;
            bot.engine_shutdown_until_tick = Some(crate::continuum::continuum_tick_add(tick, 400));
            bot.advance_energy_at_tick(100, crate::continuum::continuum_tick_add(tick, 100));
            assert_eq!(bot.energy, 600, "prize Shutdown permits recharge");
            bot.engine_shutdown_until_tick = None;
            bot.emp_until_tick = Some(crate::continuum::continuum_tick_add(tick, 250));
            bot.energy = 500;
            let before = crate::continuum::continuum_tick_add(tick, 200);
            assert!(bot.emp_active(before));
            assert!(!bot.shutdown_active(before));
            bot.advance_energy_at_tick(100, before);
            assert_eq!(bot.energy, 500);
            let after = crate::continuum::continuum_tick_add(tick, 300);
            bot.maintain_timers(after);
            bot.advance_energy_at_tick(100, after);
            assert_eq!(bot.energy, 550, "only the 50 post-expiry ticks recharge");
            assert_eq!(bot.emp_until_tick, None);
            bot.emp_until_tick = Some(crate::continuum::continuum_tick_add(after, 100));
            bot.reset_loadout_at_tick(after);
            assert_eq!(bot.emp_until_tick, None);
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    fn stationary_e2e_fixture_is_life_scoped_and_never_holds_safety_egress() {
        let mut bot = configured_bot();
        bot.e2e_stationary_from_life_seq = Some(2);

        assert!(!bot.e2e_stationary_motion_active(false, 10));
        bot.advance_life();
        assert!(bot.e2e_stationary_motion_active(false, 10));
        assert!(
            !bot.e2e_stationary_motion_active(true, 10),
            "a fixture bot must leave authored safety before it becomes stationary"
        );
        bot.life_seq = 3;
        assert!(bot.e2e_stationary_motion_active(false, 10));
        bot.life_seq = 1;
        assert!(
            !bot.e2e_stationary_motion_active(false, 10),
            "a predecessor life must remain outside the configured boundary"
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    fn stationary_e2e_fixture_can_arm_from_real_human_damage_on_the_same_life() {
        let mut bot = configured_bot();
        bot.e2e_stationary_after_first_human_damage = true;

        let hit_tick = 0x7fff_ffc0;
        assert!(bot.arm_e2e_stationary_after_human_damage(hit_tick));
        assert_eq!(bot.e2e_stationary_from_life_seq, Some(1));
        let deadline = crate::continuum::continuum_tick_add(
            hit_tick,
            E2E_STATIONARY_AFTER_HUMAN_DAMAGE_LEAD_TICKS,
        );
        assert_eq!(bot.e2e_stationary_not_before_tick, Some(deadline));
        assert!(!bot.e2e_stationary_motion_active(false, hit_tick));
        assert!(bot.e2e_stationary_motion_active(false, deadline));
        assert!(!bot.e2e_stationary_motion_active(true, deadline));
        assert!(!bot.arm_e2e_stationary_after_human_damage(deadline));

        bot.advance_life();
        assert!(!bot.e2e_stationary_motion_active(false, deadline));
        let successor_hit_tick = crate::continuum::continuum_tick_add(deadline, 10);
        assert!(bot.arm_e2e_stationary_after_human_damage(successor_hit_tick));
        let successor_deadline = crate::continuum::continuum_tick_add(
            successor_hit_tick,
            E2E_STATIONARY_AFTER_HUMAN_DAMAGE_LEAD_TICKS,
        );
        assert!(!bot.e2e_stationary_motion_active(false, successor_hit_tick));
        assert!(bot.e2e_stationary_motion_active(false, successor_deadline));
    }

    #[test]
    #[cfg(debug_assertions)]
    fn stationary_after_damage_e2e_fixture_respects_configured_first_life() {
        let mut bot = configured_bot();
        bot.e2e_stationary_after_first_human_damage = true;
        bot.e2e_stationary_from_life_seq = Some(2);

        assert!(!bot.arm_e2e_stationary_after_human_damage(10));
        assert!(!bot.e2e_stationary_motion_active(false, 100));

        bot.advance_life();
        assert!(
            !bot.e2e_stationary_motion_active(false, 100),
            "an eligible life must keep moving until authoritative human damage arms it"
        );
        assert!(bot.arm_e2e_stationary_after_human_damage(100));
        let deadline =
            crate::continuum::continuum_tick_add(100, E2E_STATIONARY_AFTER_HUMAN_DAMAGE_LEAD_TICKS);
        assert!(!bot.e2e_stationary_motion_active(false, 100));
        assert!(bot.e2e_stationary_motion_active(false, deadline));
    }

    #[test]
    #[cfg(debug_assertions)]
    fn stationary_e2e_fixture_combines_damage_armed_predecessor_and_exact_static_lives() {
        let mut bot = configured_bot();
        bot.e2e_stationary_after_first_human_damage = true;
        bot.e2e_stationary_from_life_seq = Some(1);
        bot.e2e_stationary_static_life_seqs = vec![2, 3];

        let hit_tick = 100;
        assert!(bot.arm_e2e_stationary_after_human_damage(hit_tick));
        let deadline = crate::continuum::continuum_tick_add(
            hit_tick,
            E2E_STATIONARY_AFTER_HUMAN_DAMAGE_LEAD_TICKS,
        );
        assert!(!bot.e2e_stationary_motion_active(false, hit_tick));
        assert!(bot.e2e_stationary_motion_active(false, deadline));

        bot.advance_life();
        assert!(bot.e2e_stationary_motion_active(false, deadline));
        assert!(
            !bot.arm_e2e_stationary_after_human_damage(deadline),
            "the static successor boundary must not emit a redundant damage-arm transition"
        );
        assert!(
            !bot.e2e_stationary_motion_active(true, deadline),
            "even a static successor must leave authored safety"
        );

        bot.advance_life();
        assert!(bot.e2e_stationary_motion_active(false, deadline));
        bot.advance_life();
        assert!(
            !bot.e2e_stationary_motion_active(false, deadline),
            "an unnamed hostile successor must resume ordinary pursuit"
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    fn passive_fire_e2e_fixture_is_one_exact_life_only() {
        let mut bot = configured_bot();
        bot.e2e_passive_fire_life_seqs = vec![2];

        assert!(!bot.e2e_passive_fire_active());
        bot.advance_life();
        assert!(bot.e2e_passive_fire_active());
        bot.advance_life();
        assert!(
            !bot.e2e_passive_fire_active(),
            "the successor must resume ordinary hostile fire"
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    fn passive_fire_e2e_fixture_can_isolate_multiple_exact_lives() {
        let mut bot = configured_bot();
        bot.e2e_passive_fire_life_seqs = vec![1, 3];

        assert!(bot.e2e_passive_fire_active());
        bot.advance_life();
        assert!(!bot.e2e_passive_fire_active());
        bot.advance_life();
        assert!(bot.e2e_passive_fire_active());
        bot.advance_life();
        assert!(
            !bot.e2e_passive_fire_active(),
            "the newer successor must resume ordinary hostile fire"
        );
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn release_bot_e2e_controls_remain_inert_across_lives() {
        let mut bot = configured_bot();
        for _ in 0..3 {
            assert!(!bot.arm_e2e_stationary_after_human_damage(100));
            assert!(!bot.e2e_stationary_motion_active(false, 100));
            assert!(!bot.e2e_stationary_motion_active(true, 100));
            assert!(!bot.e2e_passive_fire_active());
            bot.advance_life();
        }
    }

    #[test]
    fn bot_prizes_preserve_signed_warp_energy_and_unavailable_status_fallback() {
        let mut bot = configured_bot();
        bot.energy = 333;
        let bounty = bot.bounty;

        assert!(bot.apply_prize_at_tick(7, 10));
        assert_eq!(bot.energy, 333, "positive Warp preserves live energy");
        assert_eq!(bot.bounty, bounty + 1);
        assert!(!bot.apply_prize_at_tick(-7, 11));
        assert_eq!(bot.energy, 1, "negative Warp is negative FullCharge");
        assert_eq!(bot.bounty, bounty + 1, "negative prizes add no bounty");

        bot.energy = 5;
        assert!(!bot.status.cloak_capable);
        bot.apply_prize_at_tick(5, 12);
        assert_eq!(bot.energy, bot.max_energy);
        assert!(!bot.status.cloak_capable);
    }

    #[test]
    fn bot_lifecycle_reset_retires_repel_speed_window() {
        let mut bot = configured_bot();
        bot.repel_until_tick = Some(u32::MAX);
        bot.velocity_x = crate::arena_settings::arena_repel_speed_pixels_per_second();

        bot.reset_loadout_at_tick(17);

        assert_eq!(bot.repel_until_tick, None);
        assert!(!bot.repel_active(17));
    }

    #[test]
    fn bot_motion_history_uses_exact_life_and_discontinuity_segments() {
        let mut bot = configured_bot();
        assert_eq!(
            bot.append_actor_motion_sample(10, false),
            crate::combat_timeline::HistoryAppendDisposition::Applied
        );
        let first = bot
            .actor_motion_history
            .current_segment()
            .expect("first bot segment");
        bot.velocity_x = 999.0;
        let last_relocation = bot.actor_motion_history.relocation_generation();
        bot.begin_actor_impulse_segment();
        assert_eq!(
            bot.append_actor_motion_sample(11, false),
            crate::combat_timeline::HistoryAppendDisposition::Applied
        );
        let repelled = bot
            .actor_motion_history
            .current_segment()
            .expect("repel segment");
        assert_eq!(repelled.actor, first.actor);
        assert_eq!(
            repelled.discontinuity_generation,
            first.discontinuity_generation + 1
        );
        assert_eq!(
            bot.actor_motion_history.relocation_generation(),
            last_relocation
        );

        bot.advance_life();
        let replacement = bot
            .actor_motion_history
            .current_segment()
            .expect("replacement life segment");
        assert_ne!(replacement.actor, first.actor);
        assert_eq!(replacement.discontinuity_generation, 1);
        assert_eq!(
            bot.actor_motion_history
                .at_or_before(repelled, 11)
                .map(|sample| sample.velocity_x_q8),
            Ok((999.0_f32 * 256.0) as i32)
        );
        assert_eq!(
            bot.actor_motion_history.at_or_before(replacement, 11),
            Err(crate::combat_timeline::HistoryUnavailable::WrongSegment)
        );
    }

    #[test]
    fn bot_prizes_preserve_capacity_transient_super_rng_and_multiprize_bounty() {
        let mut bot = configured_bot();
        let initial = bot.max_energy;
        let step = crate::arena_settings::arena_ship_energy_upgrade(bot.player.ship);
        bot.max_energy = initial.saturating_add(step);
        bot.energy = bot.max_energy;
        bot.apply_prize_at_tick(-2, 20);
        assert_eq!(bot.max_energy, initial);
        assert_eq!(bot.energy, initial.saturating_add(step));

        let mut expected_seed = bot.prize_effect_rng_seed;
        let max_super = crate::arena_settings::arena_ship_super_lifetime_ticks(bot.player.ship);
        let expected = crate::vie::vie_rng_next(&mut expected_seed) % max_super;
        bot.apply_prize_at_tick(17, 21);
        assert_eq!(bot.prize_effect_rng_seed, expected_seed);
        assert_eq!(
            bot.super_until_tick,
            (expected > 0).then(|| crate::continuum::continuum_tick_add(21, expected))
        );

        let bounty = bot.bounty;
        bot.apply_prize_at_tick(25, 22);
        assert_eq!(
            bot.bounty,
            bounty + 1,
            "Multiprize sub-prizes add no bounty"
        );
    }
}
