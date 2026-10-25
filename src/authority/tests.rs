use super::*;
use crate::continuum_weapon::{
    continuum_weapon_level, continuum_weapon_shrapnel_count, continuum_weapon_shrapnel_level,
};

fn test_weapon_tuning() -> WeaponInventoryTuning {
    WeaponInventoryTuning {
        initial_gun_level: 1,
        max_gun_level: 3,
        initial_bomb_level: 1,
        max_bomb_level: 2,
        shrapnel_rate: 2,
        max_shrapnel: 4,
        initial_repels: 0,
        max_repels: 2,
        initial_bursts: 0,
        max_bursts: 2,
        initial_decoys: 0,
        max_decoys: 2,
        initial_thors: 0,
        max_thors: 2,
        initial_bricks: 0,
        max_bricks: 3,
        initial_rockets: 0,
        max_rockets: 2,
        initial_portals: 0,
        max_portals: 2,
    }
}

fn inventory_with_bouncing_prize() -> WeaponInventory {
    let tuning = test_weapon_tuning();
    let mut inventory = WeaponInventory::initialized(tuning);
    inventory.apply_prize_effect(tuning, 10);
    inventory
}

fn test_status_tuning() -> StatusInventoryTuning {
    StatusInventoryTuning {
        stealth_available: true,
        initial_stealth: false,
        cloak_available: true,
        initial_cloak: true,
        xradar_available: false,
        initial_xradar: false,
        antiwarp_available: true,
        initial_antiwarp: false,
    }
}

#[test]
fn recharge_accumulates_fractional_remainder() {
    let mut energy = EnergyPool::new(100, 200, 0.0);

    energy.recharge_for_position_ticks(1, 12.5);
    assert_eq!(energy.current(), 100);
    assert_eq!(energy.recharge_remainder(), 0.125);

    energy.recharge_for_position_ticks(7, 12.5);
    assert_eq!(energy.current(), 101);
    assert!((energy.recharge_remainder() - 0.0).abs() < f32::EPSILON);
}

#[test]
fn recharge_clamps_at_max_and_clears_remainder() {
    let mut energy = EnergyPool::new(198, 200, 0.75);

    energy.recharge_for_position_ticks(100, 50.0);
    assert_eq!(energy.current(), 200);
    assert_eq!(energy.recharge_remainder(), 0.0);
}

#[test]
fn lowered_capacity_preserves_transient_through_spend_then_recharge_converges() {
    let mut energy = EnergyPool::new(1_900, 1_700, 0.75);
    assert_eq!(energy.current(), 1_900);
    assert_eq!(energy.recharge_remainder(), 0.0);

    energy.spend_weapon_energy(75.0);
    assert_eq!(energy.current(), 1_825);
    assert_eq!(energy.recharge_remainder(), 0.0);

    energy.recharge_for_position_ticks(1, 100.0);
    assert_eq!(energy.current(), 1_700);
    assert_eq!(energy.recharge_remainder(), 0.0);
}

#[test]
fn weapon_cost_requires_strictly_more_than_cost() {
    assert!(EnergyPool::new(10, 100, 0.0).can_spend_weapon_energy(0.0));
    assert!(EnergyPool::new(11, 100, 0.0).can_spend_weapon_energy(10.0));
    assert!(!EnergyPool::new(10, 100, 0.0).can_spend_weapon_energy(10.0));
    assert!(EnergyPool::new(10, 100, 0.25).can_spend_weapon_energy(10.0));
}

#[test]
fn weapon_spend_saturates_current_energy() {
    let mut energy = EnergyPool::new(5, 100, 0.25);

    energy.spend_weapon_energy(10.0);
    assert_eq!(energy.current(), 0);
    assert_eq!(energy.recharge_remainder(), 0.0);
}

#[test]
fn continuous_energy_spend_preserves_fractional_remainder() {
    let mut energy = EnergyPool::new(10, 100, 0.75);

    assert!(energy.spend_continuous_energy(3.25));
    assert_eq!(energy.current(), 7);
    assert_eq!(energy.recharge_remainder(), 0.5);

    assert!(!energy.spend_continuous_energy(7.5));
    assert_eq!(energy.current(), 7);
    assert_eq!(energy.recharge_remainder(), 0.5);
}

#[test]
fn damage_spend_saturates_energy_and_clears_recharge_remainder() {
    let mut energy = EnergyPool::new(75, 100, 0.25);

    energy.apply_damage(20);
    assert_eq!(energy.current(), 55);
    assert_eq!(energy.recharge_remainder(), 0.0);

    energy.apply_damage(80);
    assert_eq!(energy.current(), 0);
    assert_eq!(energy.recharge_remainder(), 0.0);
}

#[test]
fn proportional_world_drain_preserves_authoritative_fraction() {
    let mut energy = EnergyPool::new(1_699, 1_700, 0.75);

    energy.retain_fraction(0.2, 1.0);

    assert_eq!(energy.current(), 339);
    assert!((energy.recharge_remainder() - 0.95).abs() < 0.000_1);
}

#[test]
fn position_status_bits_strip_high_bits_and_own_safety() {
    assert_eq!(
        PositionStatusBits::from_reported_position(0xff, false).bits(),
        POSITION_STATUS_RELAY_MASK & !POSITION_STATUS_SAFETY
    );
    assert_eq!(
        PositionStatusBits::from_reported_position(POSITION_STATUS_STEALTH, true).bits(),
        POSITION_STATUS_STEALTH | POSITION_STATUS_SAFETY
    );
}

#[test]
fn status_inventory_initializes_from_tuning_and_applies_prizes() {
    let tuning = test_status_tuning();
    let mut inventory = StatusInventory::initialized(tuning);

    assert!(!inventory.stealth_capable);
    assert!(inventory.cloak_capable);
    assert!(!inventory.xradar_capable);

    inventory.apply_prize_effect(tuning, 4);
    assert!(inventory.stealth_capable);
    inventory.apply_prize_effect(tuning, -4);
    assert!(!inventory.stealth_capable);

    inventory.apply_prize_effect(tuning, 6);
    assert!(!inventory.xradar_capable);
    inventory.xradar_capable = true;
    inventory.apply_prize_effect(tuning, -6);
    assert!(!inventory.xradar_capable);
}

#[test]
fn status_inventory_strips_reported_bits_without_capability() {
    let mut inventory = StatusInventory::cleared();
    inventory.stealth_capable = true;
    inventory.antiwarp_capable = true;
    let reported = POSITION_STATUS_STEALTH
        | POSITION_STATUS_CLOAK
        | POSITION_STATUS_XRADAR
        | POSITION_STATUS_ANTIWARP
        | POSITION_STATUS_SAFETY
        | 0x80;

    assert_eq!(
        inventory.position_status_bits(reported, false).bits(),
        POSITION_STATUS_STEALTH | POSITION_STATUS_ANTIWARP
    );
    assert_eq!(
        inventory.position_status_bits(reported, true).bits(),
        POSITION_STATUS_STEALTH | POSITION_STATUS_ANTIWARP | POSITION_STATUS_SAFETY
    );
}

#[test]
fn status_bits_drain_energy_and_strip_unaffordable_statuses() {
    let mut energy = EnergyPool::new(20, 100, 0.0);
    let bits = PositionStatusBits::from_reported_position(
        POSITION_STATUS_XRADAR | POSITION_STATUS_STEALTH | POSITION_STATUS_CLOAK,
        false,
    )
    .drain_energy_for_statuses(
        &mut energy,
        StatusEnergyCosts {
            xradar_per_second: 300.0,
            stealth_per_second: 100.0,
            cloak_per_second: 0.0,
            antiwarp_per_second: 0.0,
        },
        5,
    );

    assert_eq!(energy.current(), 5);
    assert_ne!(bits.bits() & POSITION_STATUS_XRADAR, 0);
    assert_eq!(bits.bits() & POSITION_STATUS_STEALTH, 0);
    assert_ne!(bits.bits() & POSITION_STATUS_CLOAK, 0);
}

#[test]
fn weapon_inventory_initializes_and_clears_from_tuning() {
    let inventory = WeaponInventory::initialized(test_weapon_tuning());
    assert_eq!(inventory.gun_level, 1);
    assert_eq!(inventory.bomb_level, 1);
    assert!(!inventory.bouncing_bullets);
    assert_eq!(inventory.repels, 0);
    assert_eq!(inventory.rockets, 0);

    assert_eq!(WeaponInventory::cleared(), WeaponInventory::default());
}

#[test]
fn fresh_loadout_requires_bouncing_bullet_prize_and_reset_removes_it() {
    let tuning = test_weapon_tuning();
    let mut inventory = WeaponInventory::initialized(tuning);
    assert_eq!(inventory.accepted_weapon_data(1), Some(1));
    assert_eq!(inventory.accepted_weapon_data(2), None);
    inventory.apply_prize_effect(tuning, 10);
    assert_eq!(inventory.accepted_weapon_data(2), Some(2));
    inventory.apply_prize_effect(tuning, -10);
    assert_eq!(inventory.accepted_weapon_data(2), None);
    inventory.apply_prize_effect(tuning, 10);
    inventory = WeaponInventory::initialized(tuning);
    assert_eq!(inventory.accepted_weapon_data(2), None);
}

#[test]
fn initial_weapon_inventory_clamps_all_initial_counts_like_reference_game_reset_ship() {
    let tuning = WeaponInventoryTuning {
        initial_gun_level: 3,
        initial_bomb_level: 3,
        max_gun_level: 0,
        max_bomb_level: 0,
        initial_repels: 255,
        initial_bursts: 255,
        initial_decoys: 255,
        initial_thors: 255,
        initial_bricks: 255,
        initial_rockets: 255,
        initial_portals: 255,
        ..test_weapon_tuning()
    };
    let inventory = WeaponInventory::initialized(tuning);
    assert_eq!(inventory.gun_level, 0);
    assert_eq!(inventory.bomb_level, 0);
    assert_eq!(inventory.repels, tuning.max_repels);
    assert_eq!(inventory.bursts, tuning.max_bursts);
    assert_eq!(inventory.decoys, tuning.max_decoys);
    assert_eq!(inventory.thors, tuning.max_thors);
    assert_eq!(inventory.bricks, tuning.max_bricks);
    assert_eq!(inventory.rockets, tuning.max_rockets);
    assert_eq!(inventory.portals, tuning.max_portals);
}

#[test]
fn weapon_inventory_rejects_bombs_when_settings_level_is_zero() {
    let tuning = WeaponInventoryTuning {
        initial_bomb_level: 0,
        max_bomb_level: 2,
        ..test_weapon_tuning()
    };
    let mut inventory = WeaponInventory::initialized(tuning);

    assert_eq!(inventory.bomb_level, 0);
    assert_eq!(inventory.accepted_weapon_data(3), None);
    assert_eq!(inventory.accepted_weapon_data(4), None);

    inventory.apply_prize_effect(tuning, 9);
    assert_eq!(inventory.bomb_level, 1);
    assert_eq!(inventory.accepted_weapon_data(3), Some(3));
}

#[test]
fn rocket_prizes_are_server_inventory_clamped() {
    let tuning = test_weapon_tuning();
    let mut inventory = WeaponInventory::initialized(tuning);

    inventory.apply_prize_effect(tuning, 27);
    inventory.apply_prize_effect(tuning, 27);
    inventory.apply_prize_effect(tuning, 27);
    assert_eq!(inventory.rockets, tuning.max_rockets);

    inventory.apply_prize_effect(tuning, -27);
    inventory.apply_prize_effect(tuning, -27);
    inventory.apply_prize_effect(tuning, -27);
    assert_eq!(inventory.rockets, 0);
}

#[test]
fn weapon_inventory_prizes_clamp_levels_and_consumables() {
    let tuning = test_weapon_tuning();
    let mut inventory = WeaponInventory::initialized(tuning);

    inventory.apply_prize_effect(tuning, 8);
    inventory.apply_prize_effect(tuning, 8);
    inventory.apply_prize_effect(tuning, 8);
    assert_eq!(inventory.gun_level, 3);
    inventory.apply_prize_effect(tuning, -8);
    inventory.apply_prize_effect(tuning, -8);
    inventory.apply_prize_effect(tuning, -8);
    assert_eq!(inventory.gun_level, 1);

    inventory.apply_prize_effect(tuning, 19);
    inventory.apply_prize_effect(tuning, 19);
    inventory.apply_prize_effect(tuning, 19);
    assert_eq!(inventory.shrapnel, 4);
    inventory.apply_prize_effect(tuning, -19);
    assert_eq!(inventory.shrapnel, 2);

    inventory.apply_prize_effect(tuning, 21);
    inventory.apply_prize_effect(tuning, 21);
    inventory.apply_prize_effect(tuning, 21);
    assert_eq!(inventory.repels, 2);
    inventory.apply_prize_effect(tuning, -21);
    assert_eq!(inventory.repels, 1);

    inventory.apply_prize_effect(tuning, 26);
    inventory.apply_prize_effect(tuning, 26);
    inventory.apply_prize_effect(tuning, 26);
    inventory.apply_prize_effect(tuning, 26);
    assert_eq!(inventory.bricks, 3);
    inventory.apply_prize_effect(tuning, -26);
    assert_eq!(inventory.bricks, 2);
}

#[test]
fn weapon_inventory_accepts_and_clamps_owned_weapon_state() {
    let mut inventory = inventory_with_bouncing_prize();
    inventory.shrapnel = 2;
    let level_three_bouncing_bullet = 2 | (2 << 5) | (3 << 8) | (5 << 10);

    assert_eq!(
        inventory.accepted_weapon_data(level_three_bouncing_bullet),
        None
    );

    inventory.gun_level = 3;
    let accepted = inventory
        .accepted_weapon_data(level_three_bouncing_bullet)
        .expect("owned bouncing bullet should be accepted");
    assert_eq!(continuum_weapon_level(accepted), 3);
    assert_eq!(continuum_weapon_shrapnel_level(accepted), 3);
    assert_eq!(continuum_weapon_shrapnel_count(accepted), 2);
}

#[test]
fn shrapnel_prize_uses_authored_zero_rate_and_dynamic_tuning_verbatim() {
    let mut tuning = test_weapon_tuning();
    tuning.shrapnel_rate = 0;
    tuning.max_shrapnel = 5;
    let mut inventory = WeaponInventory::initialized(tuning);
    inventory.shrapnel = 4;

    inventory.apply_prize_effect(tuning, 19);
    assert_eq!(inventory.shrapnel, 4);
    inventory.apply_prize_effect(tuning, -19);
    assert_eq!(inventory.shrapnel, 4);

    tuning.shrapnel_rate = 3;
    inventory.apply_prize_effect(tuning, 19);
    assert_eq!(
        inventory.shrapnel, 5,
        "positive clamps at authored capacity"
    );
    inventory.apply_prize_effect(tuning, -19);
    assert_eq!(
        inventory.shrapnel, 2,
        "the next claim observes new zone tuning"
    );
    inventory.apply_prize_effect(tuning, -19);
    assert_eq!(
        inventory.shrapnel, 2,
        "negative below rate is an exact no-op"
    );

    tuning.shrapnel_rate = u8::MAX;
    tuning.max_shrapnel = u8::MAX;
    inventory.apply_prize_effect(tuning, 19);
    assert_eq!(inventory.shrapnel, u8::MAX);
    inventory.apply_prize_effect(tuning, -19);
    assert_eq!(inventory.shrapnel, 0);
}

#[test]
fn weapon_inventory_requires_consumable_inventory_before_spending() {
    let mut inventory = WeaponInventory::initialized(test_weapon_tuning());
    let repel_weapon = 5;

    assert_eq!(inventory.accepted_weapon_data(repel_weapon), None);
    inventory.repels = 1;
    assert_eq!(
        inventory.accepted_weapon_data(repel_weapon),
        Some(repel_weapon)
    );
    inventory.spend_consumable(5);
    assert_eq!(inventory.repels, 0);
    assert_eq!(inventory.accepted_weapon_data(repel_weapon), None);

    assert!(!inventory.can_spend_brick());
    inventory.apply_prize_effect(test_weapon_tuning(), 26);
    assert!(inventory.can_spend_brick());
    inventory.spend_brick();
    assert_eq!(inventory.bricks, 0);
}

#[test]
fn weapon_fire_acceptance_spends_energy_and_consumables() {
    let mut weapons = WeaponInventory::initialized(test_weapon_tuning());
    weapons.repels = 1;
    let state = WeaponFireState {
        energy: EnergyPool::new(100, 200, 0.25),
        weapons,
        last_fire_tick: None,
        last_gun_fire_tick: None,
        last_bomb_fire_tick: None,
        last_special_weapon_request: None,
    };

    let outcome = accept_weapon_fire(
        state,
        WeaponFireRequest {
            weapon_data: 5,
            server_tick: 100,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 20.0,
            fire_delay_ticks: 50,
            cross_fire_delay_ticks: 0,
        },
    );

    assert_eq!(outcome.accepted_weapon_data, 5);
    assert_eq!(outcome.state.energy.current(), 80);
    assert_eq!(outcome.state.energy.recharge_remainder(), 0.25);
    assert_eq!(outcome.state.weapons.repels, 0);
    assert_eq!(outcome.state.last_fire_tick, Some(100));
    assert_eq!(outcome.state.last_bomb_fire_tick, Some(100));
    assert_eq!(outcome.state.last_special_weapon_request, Some(5));
}

#[test]
fn weapon_fire_acceptance_rejects_arena_rule_blocked_weapon() {
    let weapons = WeaponInventory::initialized(test_weapon_tuning());
    let outcome = accept_weapon_fire(
        WeaponFireState {
            energy: EnergyPool::new(100, 200, 0.25),
            weapons,
            last_fire_tick: None,
            last_gun_fire_tick: None,
            last_bomb_fire_tick: None,
            last_special_weapon_request: None,
        },
        WeaponFireRequest {
            weapon_data: 3,
            server_tick: 100,
            in_safe_tile: false,
            blocked_by_arena_rule: true,
            energy_cost: 20.0,
            fire_delay_ticks: 50,
            cross_fire_delay_ticks: 0,
        },
    );

    assert_eq!(outcome.accepted_weapon_data, 0);
    assert_eq!(outcome.state.energy.current(), 100);
    assert_eq!(outcome.state.last_fire_tick, None);
}

#[test]
fn weapon_fire_acceptance_does_not_grant_unowned_client_requested_capabilities() {
    let state = WeaponFireState {
        energy: EnergyPool::new(1000, 2000, 0.0),
        weapons: WeaponInventory::initialized(test_weapon_tuning()),
        last_fire_tick: None,
        last_gun_fire_tick: None,
        last_bomb_fire_tick: None,
        last_special_weapon_request: None,
    };

    let forged_proximity = accept_weapon_fire(
        state,
        WeaponFireRequest {
            weapon_data: 4,
            server_tick: 100,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 20.0,
            fire_delay_ticks: 0,
            cross_fire_delay_ticks: 0,
        },
    );
    assert_eq!(forged_proximity.accepted_weapon_data, 0);
    assert!(!forged_proximity.state.weapons.proximity_bombs);

    let forged_repel = accept_weapon_fire(
        forged_proximity.state,
        WeaponFireRequest {
            weapon_data: 5,
            server_tick: 120,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 0.0,
            fire_delay_ticks: 0,
            cross_fire_delay_ticks: 0,
        },
    );
    assert_eq!(forged_repel.accepted_weapon_data, 0);
    assert_eq!(forged_repel.state.weapons.repels, 0);

    let forged_gun_upgrade = accept_weapon_fire(
        forged_repel.state,
        WeaponFireRequest {
            weapon_data: 2 | (3 << 5),
            server_tick: 140,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 20.0,
            fire_delay_ticks: 0,
            cross_fire_delay_ticks: 0,
        },
    );
    assert_eq!(forged_gun_upgrade.accepted_weapon_data, 0);
    assert_eq!(
        forged_gun_upgrade.state.weapons.gun_level,
        test_weapon_tuning().initial_gun_level
    );
}

#[test]
fn weapon_fire_acceptance_allows_only_the_server_supplied_flagger_level() {
    let state = WeaponFireState {
        energy: EnergyPool::new(1_000, 2_000, 0.0),
        weapons: inventory_with_bouncing_prize(),
        last_fire_tick: None,
        last_gun_fire_tick: None,
        last_bomb_fire_tick: None,
        last_special_weapon_request: None,
    };
    let level_four_bullet = 2 | (3 << 5);
    let request = WeaponFireRequest {
        weapon_data: level_four_bullet,
        server_tick: 100,
        in_safe_tile: false,
        blocked_by_arena_rule: false,
        energy_cost: 80.0,
        fire_delay_ticks: 0,
        cross_fire_delay_ticks: 0,
    };

    assert_eq!(accept_weapon_fire(state, request).accepted_weapon_data, 0);
    assert_eq!(
        super::weapons::accept_weapon_fire_with_levels(state, request, 4, 1).accepted_weapon_data,
        level_four_bullet
    );
}

#[test]
fn weapon_fire_acceptance_rejects_held_special_until_released() {
    let mut weapons = WeaponInventory::initialized(test_weapon_tuning());
    weapons.repels = 2;
    let pressed = accept_weapon_fire(
        WeaponFireState {
            energy: EnergyPool::new(100, 200, 0.0),
            weapons,
            last_fire_tick: None,
            last_gun_fire_tick: None,
            last_bomb_fire_tick: None,
            last_special_weapon_request: None,
        },
        WeaponFireRequest {
            weapon_data: 5,
            server_tick: 100,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 0.0,
            fire_delay_ticks: 0,
            cross_fire_delay_ticks: 0,
        },
    );

    let held = accept_weapon_fire(
        pressed.state,
        WeaponFireRequest {
            weapon_data: 5,
            server_tick: 160,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 0.0,
            fire_delay_ticks: 0,
            cross_fire_delay_ticks: 0,
        },
    );
    assert_eq!(held.accepted_weapon_data, 0);
    assert_eq!(held.state.weapons.repels, 1);

    let released = accept_weapon_fire(
        held.state,
        WeaponFireRequest {
            weapon_data: 0,
            server_tick: 170,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 0.0,
            fire_delay_ticks: 0,
            cross_fire_delay_ticks: 0,
        },
    );
    assert_eq!(released.state.last_special_weapon_request, None);

    let pressed_again = accept_weapon_fire(
        released.state,
        WeaponFireRequest {
            weapon_data: 5,
            server_tick: 180,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 0.0,
            fire_delay_ticks: 0,
            cross_fire_delay_ticks: 0,
        },
    );
    assert_eq!(pressed_again.accepted_weapon_data, 5);
    assert_eq!(pressed_again.state.weapons.repels, 0);
}

#[test]
fn weapon_fire_acceptance_uses_gun_delay_not_bomb_delay_after_gun_for_bombs() {
    let weapons = WeaponInventory::initialized(test_weapon_tuning());
    let state = WeaponFireState {
        energy: EnergyPool::new(100, 200, 0.0),
        weapons,
        last_fire_tick: Some(100),
        last_gun_fire_tick: Some(100),
        last_bomb_fire_tick: None,
        last_special_weapon_request: None,
    };

    let accepted = accept_weapon_fire(
        state,
        WeaponFireRequest {
            weapon_data: 3,
            server_tick: 124,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 0.0,
            fire_delay_ticks: 150,
            cross_fire_delay_ticks: 24,
        },
    );

    assert_eq!(accepted.accepted_weapon_data, 3);
    assert_eq!(accepted.state.last_bomb_fire_tick, Some(124));
}

#[test]
fn weapon_fire_acceptance_keeps_bomb_delay_lockout_for_guns_after_bomb() {
    let weapons = inventory_with_bouncing_prize();
    let state = WeaponFireState {
        energy: EnergyPool::new(100, 200, 0.0),
        weapons,
        last_fire_tick: Some(100),
        last_gun_fire_tick: None,
        last_bomb_fire_tick: Some(100),
        last_special_weapon_request: None,
    };

    let rejected = accept_weapon_fire(
        state,
        WeaponFireRequest {
            weapon_data: 2,
            server_tick: 124,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 0.0,
            fire_delay_ticks: 24,
            cross_fire_delay_ticks: 150,
        },
    );
    assert_eq!(rejected.accepted_weapon_data, 0);

    let accepted = accept_weapon_fire(
        rejected.state,
        WeaponFireRequest {
            weapon_data: 2,
            server_tick: 250,
            in_safe_tile: false,
            blocked_by_arena_rule: false,
            energy_cost: 0.0,
            fire_delay_ticks: 24,
            cross_fire_delay_ticks: 150,
        },
    );
    assert_eq!(accepted.accepted_weapon_data, 2);
    assert_eq!(accepted.state.last_gun_fire_tick, Some(250));
}

#[test]
fn weapon_fire_acceptance_allows_only_one_server_tick_of_proven_sequence_quantization() {
    let weapons = inventory_with_bouncing_prize();
    let state = WeaponFireState {
        energy: EnergyPool::new(100, 200, 0.0),
        weapons,
        last_fire_tick: Some(100),
        last_gun_fire_tick: Some(100),
        last_bomb_fire_tick: None,
        last_special_weapon_request: None,
    };
    let request = WeaponFireRequest {
        weapon_data: 2,
        server_tick: 123,
        in_safe_tile: false,
        blocked_by_arena_rule: false,
        energy_cost: 0.0,
        fire_delay_ticks: 24,
        cross_fire_delay_ticks: 0,
    };

    assert_eq!(
        accept_weapon_fire(state, request).reject_reason,
        Some(super::weapons::WeaponFireRejectReason::Cooldown),
        "receipt time remains authoritative without a canonical logical proof"
    );
    assert_eq!(
        super::weapons::accept_weapon_fire_with_levels_and_charge_and_logical_cooldown(
            state,
            request,
            1,
            1,
            0.0,
            super::weapons::WeaponCooldownProof {
                gun: true,
                bomb: false
            },
        )
        .accepted_weapon_data,
        2,
        "one final 100 Hz quantization tick is accepted with exact logical proof"
    );

    let two_ticks_early = WeaponFireRequest {
        server_tick: 122,
        ..request
    };
    assert_eq!(
        super::weapons::accept_weapon_fire_with_levels_and_charge_and_logical_cooldown(
            state,
            two_ticks_early,
            1,
            1,
            0.0,
            super::weapons::WeaponCooldownProof {
                gun: true,
                bomb: false
            },
        )
        .reject_reason,
        Some(super::weapons::WeaponFireRejectReason::Cooldown),
        "logical input never becomes an independent or multi-tick time authority"
    );
}

#[test]
fn super_shots_require_normal_fractional_energy_but_charge_zero() {
    fn state(current: u16, remainder: f32) -> WeaponFireState {
        WeaponFireState {
            energy: EnergyPool::new(current, 200, remainder),
            weapons: inventory_with_bouncing_prize(),
            last_fire_tick: None,
            last_gun_fire_tick: None,
            last_bomb_fire_tick: None,
            last_special_weapon_request: None,
        }
    }
    let request = WeaponFireRequest {
        weapon_data: 2,
        server_tick: 100,
        in_safe_tile: false,
        blocked_by_arena_rule: false,
        energy_cost: 100.0,
        fire_delay_ticks: 0,
        cross_fire_delay_ticks: 0,
    };

    for (current, remainder) in [(99, 0.999), (100, 0.0)] {
        let outcome = super::weapons::accept_weapon_fire_with_levels_and_charge(
            state(current, remainder),
            request,
            1,
            1,
            0.0,
        );
        assert_eq!(outcome.accepted_weapon_data, 0);
        assert_eq!(
            outcome.reject_reason,
            Some(super::weapons::WeaponFireRejectReason::InsufficientEnergy)
        );
    }

    let accepted = super::weapons::accept_weapon_fire_with_levels_and_charge(
        state(100, 0.001),
        request,
        1,
        1,
        0.0,
    );
    assert_eq!(accepted.accepted_weapon_data, 2);
    assert_eq!(accepted.state.energy.current(), 100);
    assert!((accepted.state.energy.recharge_remainder() - 0.001).abs() < 0.000_01);
}

#[test]
fn thor_admission_canonicalizes_all_requested_power_and_fragment_bits() {
    let mut inventory = WeaponInventory::initialized(test_weapon_tuning());
    inventory.thors = 1;
    inventory.shrapnel = 31;
    for bomb_level in 0..=4 {
        inventory.bomb_level = bomb_level;
        for metadata in 0..=0x07ff_u16 {
            let request = 8 | (metadata << 5);
            assert_eq!(
                inventory.accepted_weapon_data(request),
                Some(8),
                "stock Thor is independent of bombs and client metadata: {request:#06x}"
            );
        }
    }
    inventory.thors = 0;
    assert_eq!(inventory.accepted_weapon_data(8), None);
    assert_eq!(inventory.accepted_weapon_data(0xffe8), None);
}

#[test]
fn explosive_shrapnel_power_is_authoritative_parent_level_independent_of_guns() {
    let mut inventory = WeaponInventory::initialized(test_weapon_tuning());
    inventory.bomb_level = 4;
    inventory.proximity_bombs = true;
    inventory.thors = 1;
    inventory.shrapnel = 5;
    for gun_level in 0..=4 {
        inventory.gun_level = gun_level;
        for weapon_type in [3, 4] {
            for parent_level in 1..=4 {
                for requested_fragment_level in 1..=4 {
                    let requested = crate::gameplay_rules::encode_weapon_data(
                        weapon_type,
                        parent_level,
                        true,
                        requested_fragment_level,
                        31,
                        false,
                    );
                    let accepted = inventory.accepted_weapon_data(requested).unwrap();
                    assert_eq!(continuum_weapon_level(accepted), parent_level);
                    assert_eq!(continuum_weapon_shrapnel_level(accepted), parent_level);
                    assert_eq!(continuum_weapon_shrapnel_count(accepted), 5);
                }
            }
        }
    }
}
