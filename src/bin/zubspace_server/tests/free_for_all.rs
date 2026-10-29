use super::*;

#[test]
fn shipped_free_for_all_assigns_unique_human_and_bot_frequencies_and_rejects_changes() {
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        bot_count: 4,
        ..ServerConfig::default()
    })));
    state.lock().unwrap().initialize_configured_bots();
    let mut humans = Vec::new();
    let mut receivers = Vec::new();
    for _ in 0..8 {
        let (tx, rx) = mpsc::channel();
        let id = register_connection(&state, tx);
        join_arena(&state, id, 0);
        humans.push(id);
        receivers.push(rx);
    }
    let before = {
        let state = state.lock().unwrap();
        assert!(state.config.free_for_all);
        let all = state
            .clients
            .values()
            .map(|client| client.player.frequency)
            .chain(state.bots.iter().map(|bot| bot.player.frequency))
            .collect::<Vec<_>>();
        assert_eq!(all.len(), 12);
        assert_eq!(
            all.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            all.len()
        );
        state.clients[&humans[0]].player.frequency
    };
    consume_pending_lifecycle_baseline(&state, humans[0]);
    let request = frequency_change_request(&state, humans[0], humans[1]);
    let baseline_before = state.lock().unwrap().clients[&humans[0]].pending_frequency_baseline;
    update_frequency(&state, humans[0], &request);
    let guard = state.lock().unwrap();
    assert_eq!(guard.clients[&humans[0]].player.frequency, before);
    assert_eq!(
        guard.clients[&humans[0]].pending_frequency_baseline,
        baseline_before
    );
}

#[test]
fn raw_spawn_loadouts_match_reference_capabilities_for_every_chaos_ship() {
    for ship in 0..8 {
        let tuning = crate::state::weapon_inventory_tuning(ship);
        let weapons = crate::authority::WeaponInventory::initialized(tuning);
        assert_eq!(weapons.gun_level, 3);
        assert_eq!(weapons.bomb_level, 0);
        assert!(!weapons.bouncing_bullets);
        assert_eq!(weapons.bricks, 0);
        let statuses = crate::authority::StatusInventory::initialized(
            crate::state::status_inventory_tuning(ship),
        );
        assert_eq!(statuses.cloak_capable, ship == 2 || ship == 7);
        assert!(!statuses.stealth_capable);
        assert!(!statuses.xradar_capable);
        assert!(!statuses.antiwarp_capable);
    }
}

#[test]
fn reference_bounty_thirty_chaos_status_capabilities_and_costs_match_all_eight_ships() {
    // Explicit reference-profile fixture, distinct from the full-spawn product policy.
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        maxed_spawn_loadout: false,
        ..ServerConfig::default()
    })));
    let mut receivers = Vec::new();
    for ship in 0..8 {
        let (tx, rx) = mpsc::channel();
        receivers.push(rx);
        let id = register_connection(&state, tx);
        join_arena(&state, id, ship);
        let guard = state.lock().unwrap();
        let client = &guard.clients[&id];
        assert_eq!(client.player.ship, ship);
        assert_eq!(client.server_bounty, 30);
        assert!(client.server_status.stealth_capable, "ship {ship}");
        assert!(client.server_status.xradar_capable, "ship {ship}");
        assert_eq!(client.server_status.cloak_capable, ship == 2 || ship == 7);
        assert!(
            !client.server_status.antiwarp_capable,
            "Antiwarp remains a green acquisition at bounty 30"
        );
        assert!(
            !client.server_weapons.multifire_capable,
            "zero MultiFire weight excludes starting grants"
        );
        // Join has not submitted its first position yet. Active toggles are
        // independently observed through genuine browser keys; this native
        // table audits owned capability and tuning, not a fabricated position.
        assert_eq!(client.server_max_energy, 1700);
        assert!((client.server_recharge_per_second - 89.8).abs() < 0.01);
        assert!(
            (crate::arena_settings::arena_ship_stealth_energy_per_second(ship) - 33.3).abs() < 0.01
        );
        assert_eq!(
            crate::arena_settings::arena_ship_xradar_energy_per_second(ship),
            25.0
        );
        assert_eq!(
            crate::arena_settings::arena_ship_antiwarp_energy_per_second(ship),
            10.0
        );
        assert_eq!(
            crate::arena_settings::arena_ship_cloak_energy_per_second(ship),
            if ship == 2 || ship == 7 { 10.0 } else { 0.0 }
        );
    }
}

#[test]
fn mixed_bot_ship_pool_distributes_total_evenly_without_changing_central_spawns() {
    let pool = vec![0, 1, 3, 4, 6];
    for requested in [0, 1, 3, 5, 12, 15, 17] {
        let mut state = ServerState::new(ServerConfig {
            bot_count: requested,
            bot_ship_types: pool.clone(),
            maxed_spawn_loadout: false, // This allocation fixture pins raw authored loadouts.
            ..ServerConfig::default()
        });
        state.initialize_configured_bots();
        assert_eq!(state.bots.len(), requested);
        let mut counts = [0_usize; 5];
        let (center_x, center_y) = crate::zone::server_spawn_coordinates(state.zone_map());
        for (index, bot) in state.bots.iter().enumerate() {
            let slot = pool
                .iter()
                .position(|ship| *ship == bot.player.ship)
                .unwrap();
            counts[slot] += 1;
            assert_eq!(slot, index % pool.len());
            assert_eq!(bot.player.frequency, bot.player.id);
            assert!(bot.player.id >= crate::state::SERVER_BOT_ID_BASE);
            let distance =
                ((bot.x - center_x as f32).powi(2) + (bot.y - center_y as f32).powi(2)).sqrt();
            assert!((distance - (96.0 + (index % 5) as f32 * 32.0)).abs() < 0.01);
            assert_eq!(
                bot.max_energy,
                crate::arena_settings::arena_ship_initial_energy(bot.player.ship)
            );
            assert_eq!(
                bot.weapons,
                crate::authority::WeaponInventory::initialized(
                    crate::state::weapon_inventory_tuning(bot.player.ship)
                )
            );
        }
        for (slot, count) in counts.iter().enumerate() {
            assert_eq!(
                *count,
                requested / pool.len() + usize::from(slot < requested % pool.len())
            );
        }
    }
}

fn assert_maxed_bomb_fragment_power(weapons: crate::authority::WeaponInventory, ship: u8) {
    let expected_level = if ship == 3 { 3 } else { 2 };
    for kind in [3, 4] {
        for mine in [false, true] {
            let requested =
                crate::gameplay_rules::encode_weapon_data(kind, expected_level, true, 4, 31, mine);
            let accepted = weapons.accepted_weapon_data(requested).unwrap();
            assert_eq!(
                crate::continuum_weapon::continuum_weapon_shrapnel_level(accepted),
                expected_level
            );
            assert_eq!(
                crate::continuum_weapon::continuum_weapon_shrapnel_count(accepted),
                if ship == 5 { 0 } else { 8 }
            );
        }
    }
}

#[test]
fn maxed_spawn_policy_keeps_bounty_thirty_and_maximizes_all_human_and_bot_ships() {
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        bot_count: 8,
        bot_ship_types: (0..8).collect(),
        ..ServerConfig::default()
    })));
    assert!(state.lock().unwrap().config.maxed_spawn_loadout);
    state.lock().unwrap().initialize_configured_bots();
    let mut receivers = Vec::new();
    for ship in 0..8 {
        let (tx, rx) = mpsc::channel();
        receivers.push(rx);
        let id = register_connection(&state, tx);
        join_arena(&state, id, ship);
        let mut guard = state.lock().unwrap();
        let client = guard.clients.get_mut(&id).unwrap();
        for _ in 0..2 {
            assert_eq!(client.server_bounty, 30);
            assert_eq!(client.server_max_energy, 1700);
            assert_eq!(client.server_energy, 1700);
            assert_eq!(
                client.server_top_speed,
                if ship == 1 { 375.0 } else { 325.0 }
            );
            assert_eq!(
                client.server_thrust_acceleration,
                match ship {
                    0 => 190.0,
                    1 => 170.0,
                    _ => 180.0,
                }
            );
            assert_eq!(client.server_recharge_per_second, 115.0);
            assert_eq!(
                client.server_rotation_per_second,
                match ship {
                    0 => 0.725,
                    7 => 0.65,
                    _ => 0.575,
                }
            );
            assert_eq!(
                client.server_weapons.shrapnel,
                if ship == 5 { 0 } else { 8 }
            );
            assert_eq!(client.server_weapons.gun_level, 3);
            assert_maxed_bomb_fragment_power(client.server_weapons, ship);
            assert_eq!(
                client.server_weapons.bomb_level,
                if ship == 3 { 3 } else { 2 }
            );
            assert!(client.server_weapons.multifire_capable);
            assert!(client.server_weapons.proximity_bombs);
            assert!(client.server_weapons.bouncing_bullets);
            assert_eq!(client.server_weapons.bricks, 3);
            for remaining in (0..3).rev() {
                assert!(client.server_weapons.can_spend_brick());
                client.server_weapons.spend_brick();
                assert_eq!(client.server_weapons.bricks, remaining);
            }
            assert!(!client.server_weapons.can_spend_brick());
            assert_eq!(client.server_weapons.repels, 3);
            assert_eq!(client.server_weapons.bursts, 3);
            assert_eq!(client.server_weapons.decoys, 3);
            assert_eq!(client.server_weapons.thors, 3);
            assert_eq!(client.server_weapons.rockets, 3);
            assert_eq!(client.server_weapons.portals, 3);
            assert!(client.server_status.stealth_capable);
            assert!(client.server_status.xradar_capable);
            assert!(client.server_status.antiwarp_capable);
            assert_eq!(client.server_status.cloak_capable, ship == 2 || ship == 7);
            client.server_top_speed = 1.0;
            client.server_weapons = crate::authority::WeaponInventory::cleared();
            client.reset_server_runtime_for_ship_at_tick(100);
        }
        let bot = &mut guard.bots[ship as usize];
        for _ in 0..2 {
            assert_eq!(bot.bounty, 30);
            assert_eq!(
                bot.top_speed_pixels_per_second,
                if ship == 1 { 375.0 } else { 325.0 }
            );
            assert_eq!(bot.recharge_per_second, 115.0);
            assert_eq!(
                bot.rotation_per_second,
                match ship {
                    0 => 0.725,
                    7 => 0.65,
                    _ => 0.575,
                }
            );
            assert_eq!(bot.weapons.shrapnel, if ship == 5 { 0 } else { 8 });
            assert_eq!(bot.weapons.gun_level, 3);
            assert_maxed_bomb_fragment_power(bot.weapons, ship);
            assert_eq!(bot.weapons.bomb_level, if ship == 3 { 3 } else { 2 });
            assert!(
                bot.weapons.multifire_capable
                    && bot.weapons.proximity_bombs
                    && bot.weapons.bouncing_bullets
            );
            assert!(
                bot.status.stealth_capable
                    && bot.status.xradar_capable
                    && bot.status.antiwarp_capable
            );
            assert_eq!(bot.status.cloak_capable, ship == 2 || ship == 7);
            assert_eq!(bot.player.flags, 0, "capability is not activation");
            assert_eq!(
                bot.gun_weapon_data().unwrap() & 0x8000,
                0,
                "bots must keep multifire off, including after loadout resets"
            );
            bot.apply_prize_at_tick(15, 99);
            assert_eq!(
                bot.gun_weapon_data().unwrap() & 0x8000,
                0,
                "a multifire prize must not activate bot spread"
            );
            bot.top_speed_pixels_per_second = 1.0;
            bot.weapons = crate::authority::WeaponInventory::cleared();
            bot.reset_loadout_at_tick(100);
        }
    }
}

#[test]
fn ffa_frequencies_remain_unique_across_roles_reconnect_and_player_id_wrap_in_both_zones() {
    for zone in [crate::zone::CHAOS_ZONE_ID, crate::zone::EXTREME_ZONE_ID] {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            default_zone: zone.to_string(),
            bot_count: 4,
            ..ServerConfig::default()
        })));
        state.lock().unwrap().initialize_configured_bots();
        let mut receivers = Vec::new();
        let mut humans = Vec::new();
        // Exercise the last human ID, wrap to zero, and occupancy skipping.
        state.lock().unwrap().next_player_id = crate::state::SERVER_BOT_ID_BASE - 1;
        for ship in [0, 4, 8] {
            let (tx, rx) = mpsc::channel();
            receivers.push(rx);
            let id = register_connection(&state, tx);
            join_arena(&state, id, ship);
            humans.push(id);
        }
        assert_eq!(humans, [crate::state::SERVER_BOT_ID_BASE - 1, 0, 1]);
        let assert_unique = || {
            let guard = state.lock().unwrap();
            let actors = guard
                .clients
                .values()
                .filter(|client| client.joined)
                .map(|client| (client.player.id, client.player.frequency))
                .chain(
                    guard
                        .bots
                        .iter()
                        .map(|bot| (bot.player.id, bot.player.frequency)),
                )
                .collect::<Vec<_>>();
            assert_eq!(actors.len(), 7, "{zone}: complete human and bot roster");
            let frequencies = actors
                .iter()
                .map(|(_, frequency)| *frequency)
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(frequencies.len(), actors.len(), "{zone}: no shared team");
            for (id, frequency) in actors {
                assert_eq!(frequency, id, "{zone}: stable actor-owned frequency");
                assert!(frequency < arena_max_frequency());
            }
        };
        assert_unique();
        let player = humans[0];
        consume_pending_lifecycle_baseline(&state, player);
        let starting_life = state.lock().unwrap().clients[&player].life_seq;
        update_ship(&state, player, &ship_change_request(8));
        assert_eq!(state.lock().unwrap().clients[&player].player.ship, 8);
        assert_unique();
        update_ship(&state, player, &ship_change_request(7));
        {
            let guard = state.lock().unwrap();
            assert_eq!(guard.clients[&player].player.ship, 7);
            assert_eq!(guard.clients[&player].life_seq, starting_life + 2);
        }
        consume_pending_lifecycle_baseline(&state, player);
        assert_unique();
        for forbidden in [humans[1], crate::state::SERVER_BOT_ID_BASE] {
            let request = frequency_change_request(&state, player, forbidden);
            update_frequency(&state, player, &request);
            assert_unique();
        }
        let departed = state
            .lock()
            .unwrap()
            .active_player_session_identity(player)
            .unwrap();
        crate::arena::disconnect_player_session(&state, departed);
        state.lock().unwrap().next_player_id = player;
        let (tx, rx) = mpsc::channel();
        receivers.push(rx);
        let replacement = register_connection(&state, tx);
        assert_eq!(replacement, player);
        join_arena(&state, replacement, 0);
        assert_ne!(
            state.lock().unwrap().clients[&replacement].session_id,
            departed.session_id
        );
        assert_unique();
        // A stale session's departure cannot remove or retag the replacement.
        crate::arena::disconnect_player_session(&state, departed);
        assert_unique();
    }
}

#[test]
fn ffa_zone_switch_reallocates_distinct_bot_and_human_frequencies_without_session_aliases() {
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        bot_count: 4,
        ..ServerConfig::default()
    })));
    state.lock().unwrap().initialize_configured_bots();
    let mut receivers = Vec::new();
    let mut humans = Vec::new();
    for _ in 0..2 {
        let (tx, rx) = mpsc::channel();
        receivers.push(rx);
        humans.push(register_connection(&state, tx));
    }
    let sessions = humans
        .iter()
        .map(|id| state.lock().unwrap().clients[id].session_id)
        .collect::<Vec<_>>();
    let mut streams = std::collections::BTreeSet::new();
    for (zone_name, zone_id) in [
        ("Chaos Zone", "chaos-zone"),
        ("Extreme", "extreme"),
        ("Chaos Zone", "chaos-zone"),
    ] {
        let request = arena_login_payload(0, zone_name);
        crate::arena::join_arena_for_login(&state, humans[0], 0, &request).unwrap();
        join_arena(&state, humans[1], 0);
        {
            let locked = state.lock().unwrap();
            assert_eq!(locked.config.default_zone, zone_id);
            assert_eq!(locked.bots.len(), 4);
            let frequencies = locked
                .clients
                .values()
                .filter(|c| c.joined)
                .map(|c| c.player.frequency)
                .chain(locked.bots.iter().map(|b| b.player.frequency))
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                frequencies.len(),
                6,
                "{zone_id}: every actor has its own team"
            );
            for (index, id) in humans.iter().enumerate() {
                assert_eq!(locked.clients[id].session_id, sessions[index]);
                assert_eq!(locked.clients[id].player.frequency, *id);
                assert!(
                    streams.insert(locked.clients[id].arena_stream_id),
                    "a new arena stream is required"
                );
            }
            for bot in &locked.bots {
                assert_eq!(bot.player.frequency, bot.player.id);
                assert!(!humans.contains(&bot.player.frequency));
            }
        }
        for id in &humans {
            crate::arena::leave_arena(&state, *id);
        }
    }
}
