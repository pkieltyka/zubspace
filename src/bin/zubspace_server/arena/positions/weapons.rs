use crate::arena_settings::{
    arena_bomb_safety_enabled, arena_flagger_bomb_upgrade, arena_flagger_fire_cost_multiplier,
    arena_flagger_gun_upgrade, arena_proximity_distance, arena_ship_max_mines,
    arena_ship_weapon_energy_cost, arena_ship_weapon_fire_delay_ticks, arena_team_max_mines,
};
use crate::authority::{
    EnergyPool, WeaponCooldownProof, WeaponFireOutcome, WeaponFireRejectReason, WeaponFireRequest,
    WeaponFireState, accept_weapon_fire_with_levels_and_charge_and_logical_cooldown,
};
use crate::continuum_position::{ClientPositionSnapshot, large_position_world_coordinates};
use crate::continuum_weapon::{
    continuum_proximity_bomb_firing_safety_radius, continuum_weapon_is_mine,
    continuum_weapon_is_proximity_bomb, continuum_weapon_level, continuum_weapon_type,
};
use crate::state::{ClientHandle, ServerState};
use tracing::info;

// Per-event lines log under this module; see `log_targets`.
const _: () = assert!(crate::log_targets::is_child_of(
    crate::log_targets::WEAPONS_COMBAT,
    module_path!()
));

/// Why a position's weapon is blocked before the shared fire rules run. The
/// fire rules only need to know that it is; the rejection log names the
/// cause, so a shot without a verified combat view is not reported as an
/// arena rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct WeaponBlock {
    pub(super) bomb_safety: bool,
    pub(super) mine_limit: bool,
    pub(super) objective_ball: bool,
    /// Why the shot could not be bound to the shooter's combat view.
    pub(super) combat_view: Option<&'static str>,
}

impl WeaponBlock {
    #[cfg(test)]
    pub(super) fn objective_ball(blocked: bool) -> Self {
        Self {
            objective_ball: blocked,
            ..Self::default()
        }
    }

    fn cause(self) -> Option<&'static str> {
        self.combat_view
            .or(self.bomb_safety.then_some("bomb_safety"))
            .or(self.mine_limit.then_some("mine_limit"))
            .or(self.objective_ball.then_some("objective_ball"))
    }
}

pub(super) fn accepted_position_weapon(
    sender: &mut ClientHandle,
    snapshot: ClientPositionSnapshot,
    in_safe_tile: bool,
    block: WeaponBlock,
    server_fire_tick: u32,
    flagger_active: bool,
) -> u16 {
    accepted_position_weapon_with_logging(
        sender,
        snapshot,
        in_safe_tile,
        block,
        server_fire_tick,
        flagger_active,
        true,
    )
    .accepted_weapon_data
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PositionWeaponPreview {
    pub(super) accepted_weapon_data: u16,
    pub(super) reject_reason: Option<WeaponFireRejectReason>,
}

/// Evaluate weapon authority against a private clone so movement admission can
/// reconstruct post-action position fields without spending energy,
/// inventory, or cooldown state. The real outcome is committed only after all
/// position checks succeed.
pub(super) fn preview_accepted_position_weapon(
    sender: &ClientHandle,
    snapshot: ClientPositionSnapshot,
    in_safe_tile: bool,
    block: WeaponBlock,
    server_fire_tick: u32,
    flagger_active: bool,
) -> u16 {
    preview_position_weapon(
        sender,
        snapshot,
        in_safe_tile,
        block,
        server_fire_tick,
        flagger_active,
    )
    .accepted_weapon_data
}

/// Return the exact prospective weapon outcome as well as the accepted wire
/// value. Position replay needs the rejection class to distinguish an honest
/// locally-predicted bomb recoil invalidated by later authoritative damage
/// from an arbitrary noncanonical movement report.
pub(super) fn preview_position_weapon(
    sender: &ClientHandle,
    snapshot: ClientPositionSnapshot,
    in_safe_tile: bool,
    block: WeaponBlock,
    server_fire_tick: u32,
    flagger_active: bool,
) -> PositionWeaponPreview {
    let mut prospective = sender.clone();
    let outcome = accepted_position_weapon_with_logging(
        &mut prospective,
        snapshot,
        in_safe_tile,
        block,
        server_fire_tick,
        flagger_active,
        false,
    );
    PositionWeaponPreview {
        accepted_weapon_data: outcome.accepted_weapon_data,
        reject_reason: outcome.reject_reason,
    }
}

#[allow(clippy::too_many_arguments)]
fn accepted_position_weapon_with_logging(
    sender: &mut ClientHandle,
    snapshot: ClientPositionSnapshot,
    in_safe_tile: bool,
    block: WeaponBlock,
    server_fire_tick: u32,
    flagger_active: bool,
    log_outcome: bool,
) -> WeaponFireOutcome {
    let effective_gun_level = crate::gameplay_rules::effective_weapon_level(
        sender.server_weapons.gun_level,
        flagger_active && arena_flagger_gun_upgrade(),
        4,
    );
    let effective_bomb_level = crate::gameplay_rules::effective_weapon_level(
        sender.server_weapons.bomb_level,
        flagger_active && arena_flagger_bomb_upgrade(),
        4,
    );
    let accepted_for_cost = sender
        .server_weapons
        .accepted_weapon_data_with_levels(
            snapshot.weapon,
            effective_gun_level,
            effective_bomb_level,
        )
        .unwrap_or(snapshot.weapon);
    let accepted_weapon_type = continuum_weapon_type(accepted_for_cost);
    let warp_bomb_lock_active = matches!(accepted_weapon_type, 3 | 4 | 6 | 7 | 8)
        && sender
            .server_warp_bomb_lock_until_tick
            .is_some_and(|until| {
                !crate::continuum::continuum_tick_reached(server_fire_tick, until)
            });
    let (fire_delay_ticks, cross_fire_delay_ticks) =
        position_weapon_cooldown_delays(sender, accepted_for_cost);
    let energy_cost = crate::gameplay_rules::effective_fire_energy_cost(
        arena_ship_weapon_energy_cost(sender.player.ship, accepted_for_cost) as f32,
        crate::gameplay_rules::FireEnergyStatus {
            flagger_active,
            flagger_cost_multiplier: arena_flagger_fire_cost_multiplier(),
            super_active: sender.server_super_until_tick.is_some_and(|until| {
                !crate::continuum::continuum_tick_reached(server_fire_tick, until)
            }),
        },
    );
    let block_cause = block
        .cause()
        .or(sender
            .objective_weapon_lock_active(server_fire_tick)
            .then_some("objective_lock"))
        .or(warp_bomb_lock_active.then_some("warp_bomb_lock"));
    let energy_before_fire = sender.server_energy;
    // A gun sets both reference deadlines to the same tick. The final
    // 100 Hz quantization allowance therefore belongs to the preceding gun,
    // even when the new request is a bomb/mine. Prove each owning class
    // independently; an older bomb deadline cannot borrow a newer gun proof.
    let mut logical_cooldown = WeaponCooldownProof::default();
    if let Some(sequence) = snapshot.client_sequence
        && let Some(previous_sequence) = sender.accepted_weapon_witnesses.latest()
        && let Some(previous_weapon) = sender.last_accepted_weapon_data
        && let Some(previous_tick) = sender.last_accepted_weapon_tick
        && sender.last_weapon_fire_tick == Some(previous_tick)
    {
        let previous_kind = continuum_weapon_type(previous_weapon);
        let request_is_gun = matches!(accepted_weapon_type, 1 | 2 | 15);
        let request_is_bomb = matches!(accepted_weapon_type, 3..=8);
        if request_is_gun || request_is_bomb {
            if matches!(previous_kind, 1 | 2 | 15)
                && sender.last_gun_fire_tick == Some(previous_tick)
            {
                logical_cooldown.gun = crate::gameplay_rules::weapon_sequence_cooldown_elapsed(
                    previous_sequence,
                    sequence,
                    if request_is_gun {
                        fire_delay_ticks
                    } else {
                        cross_fire_delay_ticks
                    },
                );
            } else if matches!(previous_kind, 3..=8)
                && sender.last_bomb_fire_tick == Some(previous_tick)
            {
                logical_cooldown.bomb = crate::gameplay_rules::weapon_sequence_cooldown_elapsed(
                    previous_sequence,
                    sequence,
                    if request_is_bomb {
                        fire_delay_ticks
                    } else {
                        cross_fire_delay_ticks
                    },
                );
            }
        }
    }
    let outcome = accept_weapon_fire_with_levels_and_charge_and_logical_cooldown(
        WeaponFireState {
            energy: EnergyPool::new(
                sender.server_energy,
                sender.server_max_energy,
                sender.server_energy_remainder,
            ),
            weapons: sender.server_weapons,
            last_fire_tick: sender.last_weapon_fire_tick,
            last_gun_fire_tick: sender.last_gun_fire_tick,
            last_bomb_fire_tick: sender.last_bomb_fire_tick,
            last_special_weapon_request: sender.last_special_weapon_request,
        },
        WeaponFireRequest {
            weapon_data: snapshot.weapon,
            server_tick: server_fire_tick,
            in_safe_tile,
            blocked_by_arena_rule: block_cause.is_some(),
            energy_cost: energy_cost.required,
            fire_delay_ticks,
            cross_fire_delay_ticks,
        },
        effective_gun_level,
        effective_bomb_level,
        energy_cost.charged,
        logical_cooldown,
    );
    sender.server_energy = outcome.state.energy.current();
    sender.server_energy_remainder = outcome.state.energy.recharge_remainder();
    sender.server_weapons = outcome.state.weapons;
    sender.last_weapon_fire_tick = outcome.state.last_fire_tick;
    sender.last_gun_fire_tick = outcome.state.last_gun_fire_tick;
    sender.last_bomb_fire_tick = outcome.state.last_bomb_fire_tick;
    sender.last_special_weapon_request = outcome.state.last_special_weapon_request;
    if outcome.accepted_weapon_data != 0 {
        sender.last_accepted_weapon_data = Some(outcome.accepted_weapon_data);
        sender.last_accepted_weapon_tick = Some(server_fire_tick);
        if let Some(sequence) = snapshot.client_sequence {
            sender.remember_accepted_weapon_witness(sequence);
        }
    }
    if log_outcome && snapshot.weapon != 0 {
        sender.session_summary.weapon(
            outcome.accepted_weapon_data != 0,
            block.combat_view.is_some(),
        );
        let accepted_weapon = outcome.accepted_weapon_data;
        let accepted_weapon_type = continuum_weapon_type(accepted_weapon);
        if accepted_weapon != 0 {
            info!(
                target: crate::log_targets::WEAPONS_COMBAT,
                player_id = sender.player.id,
                name = %sender.player.name,
                ship = sender.player.ship,
                frequency = sender.player.frequency,
                server_tick = server_fire_tick,
                client_sequence = ?snapshot.client_sequence,
                requested_weapon = snapshot.weapon,
                requested_weapon_type = continuum_weapon_type(snapshot.weapon),
                requested_weapon_level = continuum_weapon_level(snapshot.weapon),
                accepted_weapon,
                accepted_weapon_type,
                accepted_weapon_level = continuum_weapon_level(accepted_weapon),
                energy_required = energy_cost.required,
                energy_charged = energy_cost.charged,
                energy_before = energy_before_fire,
                energy_after = sender.server_energy,
                max_energy = sender.server_max_energy,
                fire_delay_ticks,
                cross_fire_delay_ticks,
                x = snapshot.x,
                y = snapshot.y,
                velocity_x = snapshot.velocity_x,
                velocity_y = snapshot.velocity_y,
                "server weapon accepted"
            );
        } else {
            info!(
                player_id = sender.player.id,
                name = %sender.player.name,
                ship = sender.player.ship,
                frequency = sender.player.frequency,
                server_tick = server_fire_tick,
                client_sequence = ?snapshot.client_sequence,
                requested_weapon = snapshot.weapon,
                requested_weapon_type = continuum_weapon_type(snapshot.weapon),
                requested_weapon_level = continuum_weapon_level(snapshot.weapon),
                accepted_for_cost,
                accepted_for_cost_type = continuum_weapon_type(accepted_for_cost),
                accepted_for_cost_level = continuum_weapon_level(accepted_for_cost),
                energy_required = energy_cost.required,
                prospective_energy_charge = energy_cost.charged,
                energy_charged = 0.0,
                energy_before = energy_before_fire,
                energy_after = sender.server_energy,
                max_energy = sender.server_max_energy,
                fire_delay_ticks,
                cross_fire_delay_ticks,
                in_safe_tile,
                block_cause = block_cause.unwrap_or("none"),
                reject_reason = outcome
                    .reject_reason
                    .map_or("unknown", |reason| reason.as_str()),
                x = snapshot.x,
                y = snapshot.y,
                velocity_x = snapshot.velocity_x,
                velocity_y = snapshot.velocity_y,
                "server weapon rejected"
            );
        }
    }
    outcome
}

pub(super) fn mine_blocked_by_mine_limits(
    state: &ServerState,
    player_id: u16,
    snapshot: ClientPositionSnapshot,
) -> bool {
    if !continuum_weapon_is_mine(snapshot.weapon) {
        return false;
    }
    let Some(sender) = state.clients.get(&player_id) else {
        return false;
    };
    if !sender.joined || !sender.alive || sender.player.ship >= 8 {
        return false;
    }

    let mut player_mines = 0usize;
    let mut team_mines = 0usize;
    let mut overlaps_existing_mine = false;
    for projectile in state.active_projectiles.iter() {
        if !continuum_weapon_is_mine(projectile.weapon_data) {
            continue;
        }
        let Some(owner) = state.clients.get(&projectile.owner_id).filter(|owner| {
            owner.joined && owner.alive && owner.life_seq == projectile.owner_life_seq
        }) else {
            continue;
        };
        if projectile.owner_id == player_id && projectile.owner_life_seq == sender.life_seq {
            player_mines += 1;
        }
        if owner.player.frequency == sender.player.frequency {
            team_mines += 1;
        }
        overlaps_existing_mine |= projectile.x == snapshot.x && projectile.y == snapshot.y;
    }

    overlaps_existing_mine
        || player_mines >= arena_ship_max_mines(sender.player.ship)
        || team_mines >= arena_team_max_mines()
}

pub(super) fn proximity_bomb_blocked_by_bomb_safety(
    state: &ServerState,
    player_id: u16,
    snapshot: ClientPositionSnapshot,
) -> bool {
    if !arena_bomb_safety_enabled()
        || !continuum_weapon_is_proximity_bomb(snapshot.weapon)
        || continuum_weapon_is_mine(snapshot.weapon)
    {
        // The reference game's separate dropped-mine input never runs this firing rule.
        // Mine stacking and player/team limits remain independently enforced.
        return false;
    }
    let Some(sender) = state.clients.get(&player_id) else {
        return false;
    };
    if !sender.joined || !sender.alive || sender.player.ship >= 8 {
        return false;
    }

    let safety_radius = continuum_proximity_bomb_firing_safety_radius(
        arena_proximity_distance(),
        continuum_weapon_level(snapshot.weapon),
    );
    let blocks = |target_x: f32, target_y: f32| {
        let dx = snapshot.x as f32 - target_x;
        let dy = snapshot.y as f32 - target_y;
        dx * dx + dy * dy <= safety_radius * safety_radius
    };
    state.clients.iter().any(|(&id, client)| {
        id != player_id
            && client.joined
            && client.alive
            && client.player.ship < 8
            && client.player.frequency != sender.player.frequency
            && client
                .last_position
                .as_deref()
                .and_then(large_position_world_coordinates)
                .is_some_and(|(target_x, target_y)| blocks(target_x as f32, target_y as f32))
    }) || state.bots.iter().any(|bot| {
        bot.alive() && bot.player.frequency != sender.player.frequency && blocks(bot.x, bot.y)
    })
}

fn position_weapon_cooldown_delays(sender: &ClientHandle, weapon: u16) -> (u32, u32) {
    // Each committed shot has already served its preceding cross-class lock.
    // Its canonical wire metadata owns the new lock, including multifire.
    // The separate bomb anchor preserves EMP's outstanding bomb deadline
    // while EMP deliberately permits intervening gun fire.
    let ship = sender.player.ship;
    let kind = continuum_weapon_type(weapon);
    let default = (
        arena_ship_weapon_fire_delay_ticks(ship, kind),
        arena_ship_weapon_cross_fire_delay_ticks(ship, kind),
    );
    let Some((previous, tick)) = sender
        .last_accepted_weapon_data
        .zip(sender.last_accepted_weapon_tick)
    else {
        return default;
    };
    let previous_kind = continuum_weapon_type(previous);
    let previous_is_gun = matches!(previous_kind, 1 | 2 | 15);
    let previous_is_bomb = matches!(previous_kind, 3..=8);
    let previous_class_tick = if previous_is_gun {
        sender.last_gun_fire_tick
    } else if previous_is_bomb {
        sender.last_bomb_fire_tick
    } else {
        return default;
    };
    if previous_class_tick != Some(tick) || sender.last_weapon_fire_tick != Some(tick) {
        return default;
    }
    let previous_gun_delay = if crate::continuum_weapon::continuum_weapon_alternate(previous) {
        crate::arena_settings::arena_ship_multifire_delay_ticks(ship)
    } else {
        arena_ship_weapon_fire_delay_ticks(ship, previous_kind)
    };
    match kind {
        1 | 2 | 15 if previous_is_gun => (previous_gun_delay, 0),
        1 | 2 | 15 => {
            // An accepted bomb already served the preceding gun deadline.
            // EMP bombs/mines alone leave guns unlocked; Thor and other
            // consumables still set both reference deadlines on EMP ships.
            let bomb_locks_guns = !(matches!(previous_kind, 3 | 4)
                && crate::arena_settings::arena_ship_emp_bomb(ship));
            (
                0,
                if bomb_locks_guns {
                    arena_ship_weapon_fire_delay_ticks(ship, previous_kind)
                } else {
                    0
                },
            )
        }
        3..=8 => {
            // A gun after an EMP bomb must not erase that bomb's outstanding
            // deadline, so keep the separate bomb anchor. Gun/bomb switches
            // use the actual preceding gun delay, including multifire.
            (
                arena_ship_weapon_fire_delay_ticks(ship, kind),
                if previous_is_gun {
                    previous_gun_delay
                } else {
                    0
                },
            )
        }
        _ => default,
    }
}

fn arena_ship_weapon_cross_fire_delay_ticks(ship: u8, weapon_type: u8) -> u32 {
    match weapon_type {
        1 | 2 | 15 => arena_ship_weapon_fire_delay_ticks(ship, 3),
        3..=8 => arena_ship_weapon_fire_delay_ticks(ship, 2),
        _ => 0,
    }
}
