use super::*;
use crate::arena::accept_position_relay;

// Tests that install a synthetic canonical predecessor must also install its
// accepted motion baseline. Production establishes both through admission.
fn seed_fixture_motion_baseline(client: &mut crate::state::ClientHandle) {
    let witness = client
        .canonical_movement_witness
        .expect("fixture predecessor");
    let tick = client
        .last_position_server_tick
        .expect("fixture acceptance tick");
    client.begin_actor_motion_segment();
    client.append_actor_motion_witness(tick, witness).unwrap();
}

#[test]
fn only_accepted_non_mine_bombs_apply_post_movement_recoil() {
    let bomb = crate::gameplay_rules::encode_weapon_data(3, 2, false, 1, 0, false);
    let proximity_bomb = crate::gameplay_rules::encode_weapon_data(4, 2, false, 1, 0, false);
    let mine = crate::gameplay_rules::encode_weapon_data(3, 2, false, 1, 0, true);
    let proximity_mine = crate::gameplay_rules::encode_weapon_data(4, 2, false, 1, 0, true);
    let bullet = crate::gameplay_rules::encode_weapon_data(1, 2, false, 1, 0, false);
    assert!(crate::arena::accepted_position_weapon_applies_recoil(bomb));
    assert!(crate::arena::accepted_position_weapon_applies_recoil(
        proximity_bomb
    ));
    assert!(!crate::arena::accepted_position_weapon_applies_recoil(mine));
    assert!(!crate::arena::accepted_position_weapon_applies_recoil(
        proximity_mine
    ));
    assert!(!crate::arena::accepted_position_weapon_applies_recoil(
        bullet
    ));
    assert!(!crate::arena::accepted_position_weapon_applies_recoil(0));
}

#[test]
fn cross_class_gun_deadline_accepts_only_proven_final_quantization_tick() {
    // Reference gun fire sets next_bomb_tick = next_bullet_tick, including
    // multifire. Exercise the production preview/commit path, not a new rule.
    for ship in [0, 5] {
        for multi in [false, true] {
            for kind in [3, 4] {
                for mine in [false, true] {
                    for (wall_early, logical_early, allowed) in
                        [(1, false, true), (2, false, false), (1, true, false)]
                    {
                        let state = Arc::new(Mutex::new(ServerState::default()));
                        let (tx, _rx) = mpsc::channel();
                        let id = register_connection(&state, tx);
                        join_arena(&state, id, ship);
                        let mut state = state.lock().unwrap();
                        let client = state.clients.get_mut(&id).unwrap();
                        client.server_energy = 1700;
                        client.server_max_energy = 1700;
                        client.server_weapons.gun_level = 3;
                        client.server_weapons.multifire_capable = true;
                        client.server_weapons.bomb_level = 2;
                        client.server_weapons.proximity_bombs = true;
                        let gun =
                            crate::gameplay_rules::encode_weapon_data(1, 3, false, 1, 0, multi);
                        let mut first = crate::continuum_position::parse_client_position_snapshot(
                            &client_position_packet_with_weapon(0, 8200, 8200, gun),
                        )
                        .unwrap();
                        first.client_sequence = Some(100);
                        let (_, preview, committed) =
                            crate::arena::preview_and_commit_position_weapon_for_test(
                                &mut state, id, first, 1000, false,
                            )
                            .unwrap();
                        assert_eq!(preview, committed);
                        assert_ne!(committed, 0);
                        let delay = if multi {
                            crate::arena_settings::arena_ship_multifire_delay_ticks(ship)
                        } else {
                            crate::arena_settings::arena_ship_weapon_fire_delay_ticks(ship, 1)
                        };
                        let steps = (delay * 3).div_ceil(5) as u16;
                        let weapon =
                            crate::gameplay_rules::encode_weapon_data(kind, 2, false, 1, 0, mine);
                        let mut next = first;
                        next.weapon = weapon;
                        next.client_sequence = Some(100 + steps - u16::from(logical_early));
                        let before = state.clients[&id].server_energy;
                        let (_, preview, committed) =
                            crate::arena::preview_and_commit_position_weapon_for_test(
                                &mut state,
                                id,
                                next,
                                1000 + delay - wall_early,
                                false,
                            )
                            .unwrap();
                        assert_eq!(preview, committed);
                        assert_eq!(
                            committed != 0,
                            allowed,
                            "ship={ship} multi={multi} kind={kind} mine={mine} wall_early={wall_early} logical_early={logical_early}"
                        );
                        assert_eq!(state.clients[&id].server_energy < before, allowed);
                    }
                }
            }
        }
    }
}

#[test]
fn an_older_emp_bomb_deadline_cannot_borrow_a_newer_gun_sequence_proof() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let id = register_connection(&state, tx);
    join_arena(&state, id, 5);
    let mut state = state.lock().unwrap();
    let client = state.clients.get_mut(&id).unwrap();
    client.server_energy = 1700;
    client.server_max_energy = 1700;
    client.server_weapons.bomb_level = 2;
    client.last_bomb_fire_tick = Some(100);
    client.last_gun_fire_tick = Some(225);
    client.last_weapon_fire_tick = Some(225);
    client.last_accepted_weapon_tick = Some(225);
    client.last_accepted_weapon_data = Some(1);
    client.remember_accepted_weapon_witness(100);
    let weapon = crate::gameplay_rules::encode_weapon_data(3, 2, false, 0, 0, false);
    let mut snapshot = crate::continuum_position::parse_client_position_snapshot(
        &client_position_packet_with_weapon(0, 8200, 8200, weapon),
    )
    .unwrap();
    snapshot.client_sequence = Some(115);
    let (_, preview, committed) = crate::arena::preview_and_commit_position_weapon_for_test(
        &mut state, id, snapshot, 249, false,
    )
    .unwrap();
    assert_eq!((preview, committed), (0, 0));
    assert_eq!(state.clients[&id].server_energy, 1700);
}

#[test]
fn reference_bomb_cooldown_is_shared_by_human_bombs_proximity_and_dropped_mines() {
    for kind in [3, 4] {
        for mine in [false, true] {
            let state = Arc::new(Mutex::new(ServerState::default()));
            let (tx, _rx) = mpsc::channel();
            let player = register_connection(&state, tx);
            join_arena(&state, player, 0);
            let mut locked = state.lock().unwrap();
            let client = locked.clients.get_mut(&player).unwrap();
            client.server_energy = 1700;
            client.server_max_energy = 1700;
            client.server_energy_remainder = 0.0;
            client.server_weapons.bomb_level = 2;
            client.server_weapons.proximity_bombs = true;
            client.last_bomb_fire_tick = Some(100);
            client.last_weapon_fire_tick = Some(100);
            let weapon = crate::gameplay_rules::encode_weapon_data(kind, 2, false, 1, 0, mine);
            let packet = client_position_packet_with_weapon(0, 8200, 8200, weapon);
            let snapshot =
                crate::continuum_position::parse_client_position_snapshot(&packet).unwrap();
            // The reference game uses BombFireDelay150 even for proximity and mines;
            // LandmineFireDelay125 is not selected by weapon type4.
            for tick in [225, 249, 250] {
                let (_, preview, committed) =
                    crate::arena::preview_and_commit_position_weapon_for_test(
                        &mut locked,
                        player,
                        snapshot,
                        tick,
                        false,
                    )
                    .unwrap();
                assert_eq!(preview, committed);
                assert_eq!(
                    committed != 0,
                    tick == 250,
                    "kind={kind} mine={mine} tick={tick}"
                );
                assert_eq!(
                    locked.clients[&player].server_energy,
                    if tick == 250 {
                        1700 - if mine { 420 } else { 350 }
                    } else {
                        1700
                    }
                );
            }
        }
    }
}

#[test]
fn reference_weapon_cooldowns_preserve_preceding_multifire_and_emp_cross_fire_rules() {
    let gun = crate::gameplay_rules::encode_weapon_data(1, 3, false, 1, 0, false);
    let multi = crate::gameplay_rules::encode_weapon_data(1, 3, false, 1, 0, true);
    let bomb = crate::gameplay_rules::encode_weapon_data(4, 2, false, 1, 0, false);
    let cases = [
        (5, vec![(1, bomb, true), (0, gun, false), (2, gun, true)]),
        (
            0,
            vec![
                (0, multi, true),
                (24, gun, false),
                (49, gun, false),
                (50, gun, true),
            ],
        ),
        (
            0,
            vec![
                (0, gun, true),
                (24, multi, true),
                (48, gun, false),
                (74, gun, true),
            ],
        ),
        (
            0,
            vec![
                (0, multi, true),
                (24, bomb, false),
                (49, bomb, false),
                (50, bomb, true),
            ],
        ),
        (
            5,
            vec![
                (0, bomb, true),
                (1, gun, true),
                (25, gun, true),
                (125, bomb, false),
                (149, bomb, false),
                (150, bomb, true),
            ],
        ),
        (
            5,
            vec![
                (0, 8, true),
                (1, gun, false),
                (149, gun, false),
                (150, gun, true),
            ],
        ),
    ];
    for (ship, requests) in cases {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player = register_connection(&state, tx);
        join_arena(&state, player, ship);
        let mut locked = state.lock().unwrap();
        let client = locked.clients.get_mut(&player).unwrap();
        client.server_energy = 1700;
        client.server_max_energy = 1700;
        client.server_energy_remainder = 0.0;
        client.server_weapons.gun_level = 3;
        client.server_weapons.multifire_capable = true;
        client.server_weapons.bomb_level = 2;
        client.server_weapons.proximity_bombs = true;
        client.server_weapons.thors = 1;
        for (elapsed, weapon, allowed) in requests {
            locked
                .clients
                .get_mut(&player)
                .unwrap()
                .maintain_short_tick_state(
                    100 + elapsed,
                    crate::arena_settings::arena_ship_weapon_fire_delay_ticks(ship, 2).max(
                        crate::arena_settings::arena_ship_multifire_delay_ticks(ship),
                    ),
                    150,
                    150,
                    550,
                );
            let before = locked.clients[&player].server_energy;
            let packet = client_position_packet_with_weapon(0, 8200, 8200, weapon);
            let snapshot =
                crate::continuum_position::parse_client_position_snapshot(&packet).unwrap();
            let (_, preview, committed) =
                crate::arena::preview_and_commit_position_weapon_for_test(
                    &mut locked,
                    player,
                    snapshot,
                    100 + elapsed,
                    false,
                )
                .unwrap();
            assert_eq!(preview, committed);
            assert_eq!(
                committed != 0,
                allowed,
                "ship={ship} weapon={weapon} elapsed={elapsed}"
            );
            if !allowed {
                assert_eq!(
                    locked.clients[&player].server_energy, before,
                    "rejected shots cannot debit energy"
                );
            }
        }
    }
}

#[test]
fn objective_flagger_weapon_preview_and_commit_use_exact_runtime_across_life_and_reconnect() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (first_tx, _first_rx) = mpsc::channel();
    let player_id = register_connection(&state, first_tx);
    join_arena(&state, player_id, 0);
    let first_runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(player_id)
        .expect("first runtime");
    {
        let mut locked = state.lock().expect("state");
        locked
            .objectives
            .reset(synthetic_profile(1, 1))
            .expect("synthetic objective generation should advance");
        locked
            .objectives
            .create_flag(FlagState::Carried {
                carrier: first_runtime,
                x: 640,
                y: 640,
                owner: None,
            })
            .expect("synthetic carried flag should fit profile");
        locked
            .objectives
            .create_powerball(PowerballState::Carried {
                carrier: first_runtime,
            })
            .expect("synthetic carried powerball should fit profile");
    }

    let weapon = crate::gameplay_rules::encode_weapon_data(1, 1, false, 1, 0, false);
    let snapshot = crate::continuum_position::parse_client_position_snapshot(
        &client_position_packet_with_weapon(0, 8200, 8200, weapon),
    )
    .expect("weapon snapshot");
    let prepare_weapon = |state: &mut ServerState, denormalized_flags| {
        let client = state.clients.get_mut(&player_id).expect("client");
        client.player.flags = denormalized_flags;
        client.server_energy = 10_000;
        client.server_max_energy = 10_000;
        client.server_weapons.gun_level = 4;
        client.last_weapon_fire_tick = None;
        client.last_gun_fire_tick = None;
        client.last_bomb_fire_tick = None;
    };
    {
        let mut locked = state.lock().expect("state");
        prepare_weapon(&mut locked, 0);
        let (flagger, previewed, committed) =
            crate::arena::preview_and_commit_position_weapon_for_test(
                &mut locked,
                player_id,
                snapshot,
                100,
                true,
            )
            .expect("first-life weapon admission");
        assert!(
            flagger,
            "the exact carried flag/ball activates flagger policy"
        );
        assert_ne!(previewed, 0);
        assert_eq!(previewed, committed, "preview and commit must agree");
        locked
            .advance_client_life(player_id)
            .expect("replacement life should advance");
    }

    let replacement_runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(player_id)
        .expect("replacement runtime");
    assert_ne!(replacement_runtime, first_runtime);
    {
        let mut locked = state.lock().expect("state");
        prepare_weapon(&mut locked, u16::MAX);
        let (flagger, previewed, committed) =
            crate::arena::preview_and_commit_position_weapon_for_test(
                &mut locked,
                player_id,
                snapshot,
                200,
                true,
            )
            .expect("replacement-life weapon admission");
        assert!(
            !flagger,
            "a reused player id and poisoned denormalized count cannot inherit old-life objectives"
        );
        assert_ne!(previewed, 0);
        assert_eq!(previewed, committed, "preview and commit must agree");

        let ball = locked.objectives.powerball(0).expect("powerball");
        locked
            .objectives
            .transition_powerball(
                ball.identity,
                ball.version,
                PowerballState::Carried {
                    carrier: replacement_runtime,
                },
            )
            .expect("test policy transfers the ball to the replacement runtime");
        prepare_weapon(&mut locked, 0);
        let (flagger, previewed, committed) =
            crate::arena::preview_and_commit_position_weapon_for_test(
                &mut locked,
                player_id,
                snapshot,
                300,
                true,
            )
            .expect("replacement-life ball weapon admission");
        assert!(
            flagger,
            "the exact replacement-life ball activates flagger policy"
        );
        assert_ne!(previewed, 0);
        assert_eq!(previewed, committed, "preview and commit must agree");
    }

    disconnect_player(&state, player_id);
    state.lock().expect("state").next_player_id = player_id;
    let (replacement_tx, _replacement_rx) = mpsc::channel();
    let reused_player_id = register_connection(&state, replacement_tx);
    assert_eq!(
        reused_player_id, player_id,
        "fixture must reuse the wire player id"
    );
    join_arena(&state, reused_player_id, 0);
    let reconnect_runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(reused_player_id)
        .expect("reconnect runtime");
    assert_ne!(reconnect_runtime, replacement_runtime);
    {
        let mut locked = state.lock().expect("state");
        prepare_weapon(&mut locked, u16::MAX);
        let (flagger, previewed, committed) =
            crate::arena::preview_and_commit_position_weapon_for_test(
                &mut locked,
                reused_player_id,
                snapshot,
                400,
                true,
            )
            .expect("reconnect weapon admission");
        assert!(
            !flagger,
            "a reconnected session cannot inherit the prior session's carried ball or flag"
        );
        assert_ne!(previewed, 0);
        assert_eq!(previewed, committed, "preview and commit must agree");
    }
}

#[test]
fn carried_ball_weapon_substitution_blocks_guns_bombs_mines_and_same_witness_mirrors() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let runtime = state
        .lock()
        .unwrap()
        .active_player_runtime_identity(player_id)
        .unwrap();
    {
        let mut locked = state.lock().unwrap();
        locked.objectives.reset(synthetic_profile(0, 1)).unwrap();
        locked
            .objectives
            .create_powerball(PowerballState::Carried { carrier: runtime })
            .unwrap();
    }

    let gun = crate::gameplay_rules::encode_weapon_data(1, 1, false, 1, 0, false);
    let bomb = crate::gameplay_rules::encode_weapon_data(3, 1, false, 1, 0, false);
    let mine = crate::gameplay_rules::encode_weapon_data(3, 1, false, 1, 0, true);
    for (offset, weapon) in [gun, bomb, mine].into_iter().enumerate() {
        let sequence = 100 + offset as u16;
        let mut packet = client_position_packet_with_sequence(0, 8200, 8200, sequence);
        packet[20..22].copy_from_slice(&weapon.to_le_bytes());
        let snapshot = crate::continuum_position::parse_client_position_snapshot(&packet).unwrap();
        let mut locked = state.lock().unwrap();
        let client = locked.clients.get_mut(&player_id).unwrap();
        client.server_energy = 10_000;
        client.server_max_energy = 10_000;
        client.server_weapons.gun_level = 4;
        client.server_weapons.bomb_level = 4;
        client.clear_server_weapon_latches();
        let before_energy = client.server_energy;
        let (previewed, committed) =
            crate::arena::preview_and_commit_position_weapon_with_ball_policy_for_test(
                &mut locked,
                player_id,
                snapshot,
                1000 + offset as u32,
                false,
                false,
            )
            .unwrap();
        assert_eq!((previewed, committed), (0, 0));
        assert_eq!(locked.clients[&player_id].server_energy, before_energy);
        assert!(locked.active_projectiles.is_empty());
    }

    let sequence = 200;
    let mut packet = client_position_packet_with_sequence(0, 8200, 8200, sequence);
    packet[20..22].copy_from_slice(&gun.to_le_bytes());
    let snapshot = crate::continuum_position::parse_client_position_snapshot(&packet).unwrap();
    let mut locked = state.lock().unwrap();
    let client = locked.clients.get_mut(&player_id).unwrap();
    client.server_energy = 10_000;
    client.server_max_energy = 10_000;
    client.server_weapons.gun_level = 4;
    client.clear_server_weapon_latches();
    let (previewed, committed) =
        crate::arena::preview_and_commit_position_weapon_with_ball_policy_for_test(
            &mut locked,
            player_id,
            snapshot,
            1100,
            true,
            true,
        )
        .unwrap();
    assert_ne!(
        previewed, 0,
        "AllowGuns preserves the normal carried-ball gun"
    );
    assert_eq!(committed, previewed);

    locked
        .clients
        .get_mut(&player_id)
        .unwrap()
        .clear_server_weapon_latches();
    locked
        .clients
        .get_mut(&player_id)
        .unwrap()
        .remember_objective_ball_fire_witness(sequence);
    let (previewed, committed) =
        crate::arena::preview_and_commit_position_weapon_with_ball_policy_for_test(
            &mut locked,
            player_id,
            snapshot,
            1200,
            true,
            true,
        )
        .unwrap();
    assert_eq!(
        (previewed, committed),
        (0, 0),
        "a delayed reliable weapon mirror cannot follow a kick on the same witness"
    );
}

#[test]
fn headed_loss_tail_17138_geometry_matches_shared_client_collision() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let start = (8_193, 8_183);
    let exact_start = (8_192.5_f32, 8_183.0_f32);
    let end = (8_168, 8_165);
    let accepted_tick = 20_039;
    let previous_sequence = 17_116;
    let current_sequence = 17_138;
    let mut previous = client_position_packet_with_sequence(0, start.0, start.1, previous_sequence);
    previous[1] = 30;
    previous[6..8].copy_from_slice(&(-426_i16).to_le_bytes());
    previous[14..16].copy_from_slice(&(-568_i16).to_le_bytes());
    set_client_position_canonical_state(
        &mut previous,
        exact_start.0,
        exact_start.1,
        -42.6,
        -56.8,
        0.75,
    );
    let previous = large_position_from_client(player_id, &previous).expect("previous position");
    let identity = {
        let mut locked = state.lock().expect("state");
        let map = locked.zone_map();
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.player.ship = 1;
        client.reset_server_runtime_for_ship_at_tick(accepted_tick - 39);
        grant_legacy_capture_loadout(client, accepted_tick - 39);
        let door_seeds = crate::doors::client_door_validation_seeds_at_tick(
            client.door_state,
            &mut client.door_seed_cache,
            accepted_tick,
        );
        let blocks = |tile| position_blocking_tile_for_door_seeds(tile, &door_seeds);
        let radius = (arena_ship_radius_pixels(1) - 1.0).max(0.0);
        assert!(!blocks(tile_at_world(map, end.0, end.1)), "end center");
        assert!(
            !crate::continuum_position::reported_position_hull_overlaps_blocked_tile(
                end.0,
                end.1,
                radius,
                MAP_TILES,
                TILE_SIZE_PX,
                |tile_x, tile_y| map
                    .tiles
                    .get(tile_y * MAP_TILES + tile_x)
                    .is_none_or(|&tile| blocks(tile)),
            ),
            "end hull"
        );
        assert!(
            !crate::continuum_position::reported_position_path_hull_overlaps_blocked_tile(
                start.0,
                start.1,
                end.0,
                end.1,
                radius,
                MAP_TILES,
                TILE_SIZE_PX,
                |tile_x, tile_y| map
                    .tiles
                    .get(tile_y * MAP_TILES + tile_x)
                    .is_none_or(|&tile| blocks(tile)),
            ),
            "shared client-legal segment must remain server-legal"
        );

        client.last_position = Some(previous);
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(accepted_tick - 39);
        let exact_initial = crate::state::ServerSimMovement {
            position_x: exact_start.0,
            position_y: exact_start.1,
            orientation: 30.0 / 40.0,
            velocity_x: -42.6,
            velocity_y: -56.8,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let witness = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: exact_initial,
            wire_x: start.0,
            wire_y: start.1,
            wire_velocity_x: -426,
            wire_velocity_y: -568,
            direction: 30,
            door_seed: 0,
            door_phase_tick: 0,
        };
        client.canonical_movement_witness = Some(witness);
        seed_fixture_motion_baseline(client);
        client.canonical_position_witnesses.push_back(witness);
        client.server_sim_state = Some(exact_initial);
        client.server_sim_anchor_tick = Some(accepted_tick - 39);
        client.server_sim_input_bits = Some(5);
        let inputs = [
            5, 5, 5, 5, 5, 5, 5, 5, 1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 1, 1, 9, 9,
        ];
        let mut exact_orientation = exact_initial.orientation;
        for (offset, bits) in (1_u16..).zip(inputs) {
            exact_orientation = match (bits & (1 << 2) != 0, bits & (1 << 3) != 0) {
                (true, false) => {
                    exact_orientation - client.server_rotation_per_second * (1.0 / 60.0)
                }
                (false, true) => {
                    exact_orientation + client.server_rotation_per_second * (1.0 / 60.0)
                }
                _ => exact_orientation,
            }
            .rem_euclid(1.0);
            let direction = crate::continuum_position::protocol_direction(exact_orientation);
            let input = crate::state::ClientInputObservation { bits, direction };
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: previous_sequence.wrapping_add(offset),
                    input,
                    door_seed: 0,
                    door_phase_tick: 0,
                });
            client.last_input_seq = Some(previous_sequence.wrapping_add(offset));
            client.last_input_record = Some(input);
        }
        locked.active_life_identity(player_id).expect("life")
    };
    let (bounty, energy) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        (client.server_bounty, client.server_energy)
    };
    let exact_tail = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        crate::sim_observation::replay_canonical_position_gap(
            client,
            locked.zone_map(),
            false,
            false,
            false,
            crate::sim_observation::PlayerMovementUpgrades {
                thrust: client.server_thrust_acceleration,
                top_speed: client.server_top_speed,
                rotation: client.server_rotation_per_second,
                recharge: client.server_recharge_per_second,
                live_values: true,
            },
            &[],
            current_sequence,
            accepted_tick,
            server_spawn_coordinates(locked.zone_map()).0,
            server_spawn_coordinates(locked.zone_map()).1,
        )
        .expect("captured headed-loss exact replay")
        .witnesses
        .last()
        .copied()
        .expect("captured tail")
    };
    assert_eq!((exact_tail.wire_x, exact_tail.wire_y), end);
    assert_eq!(exact_tail.direction, 27);
    let mut current = client_position_packet_with_sequence(0, end.0, end.1, current_sequence);
    current[1] = exact_tail.direction;
    current[6..8].copy_from_slice(&exact_tail.wire_velocity_x.to_le_bytes());
    current[14..16].copy_from_slice(&exact_tail.wire_velocity_y.to_le_bytes());
    current[16..18].copy_from_slice(&bounty.to_le_bytes());
    current[18..20].copy_from_slice(&energy.to_le_bytes());
    set_client_position_canonical_state(
        &mut current,
        exact_tail.state.position_x,
        exact_tail.state.position_y,
        exact_tail.state.velocity_x,
        exact_tail.state.velocity_y,
        exact_tail.state.orientation,
    );
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &current,
            accepted_tick,
        )
        .is_some(),
        "the exact headed-loss packet is valid through full admission; its artifact non-arrival was transport omission, not a geometry false positive",
    );
}

#[test]
fn real_wall_bounce_waives_acceleration_only_for_the_exact_canonical_tail() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 1);
    let previous_sequence = 21_107_u16;
    let current_sequence = previous_sequence + 3;
    let anchor_tick = 1_518_u32;
    let accepted_tick = 1_524_u32;
    let radius = arena_ship_radius_pixels(1);
    let (start_x, start_y) = {
        let map = zone_map();
        (2..MAP_TILES - 2)
            .find_map(|tile_y| {
                (2..MAP_TILES - 2).find_map(|tile_x| {
                    let tile = map.tiles[tile_y * MAP_TILES + tile_x];
                    if !solid_tile(tile) || (162..=169).contains(&tile) {
                        return None;
                    }
                    let start_x = (tile_x as f32 * TILE_SIZE_PX - radius - 2.0).round() as u16;
                    let start_y =
                        (tile_y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5).round() as u16;
                    (!reported_position_hull_overlaps_blocked_tile(
                        start_x,
                        start_y,
                        radius,
                        MAP_TILES,
                        TILE_SIZE_PX,
                        |x, y| {
                            map.tiles
                                .get(y * MAP_TILES + x)
                                .is_none_or(|&id| position_blocking_tile_for_door_seeds(id, &[0]))
                        },
                    ))
                    .then_some((start_x, start_y))
                })
            })
            .expect("Chaos Zone contains a clear approach to a static wall")
    };
    let initial_orientation = 0.25_f32;
    let initial_velocity_x = 500.0_f32;
    let initial_velocity_y = 0.0_f32;
    let direction = crate::continuum_position::protocol_direction(initial_orientation);
    let identity = {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.reset_server_runtime_for_ship_at_tick(anchor_tick);
        grant_legacy_capture_loadout(client, anchor_tick);
        client.life_position_sequence_base = None;
        client.alive = true;
        client.respawn_at_tick = None;
        client.server_repel_until_tick = Some(1_743);

        let mut previous =
            client_position_packet_with_sequence(0, start_x, start_y, previous_sequence);
        previous[1] = direction;
        previous[6..8].copy_from_slice(
            &crate::continuum_position::protocol_velocity_component(initial_velocity_x)
                .to_le_bytes(),
        );
        previous[14..16].copy_from_slice(
            &crate::continuum_position::protocol_velocity_component(initial_velocity_y)
                .to_le_bytes(),
        );
        previous[16..18].copy_from_slice(&client.server_bounty.to_le_bytes());
        previous[18..20].copy_from_slice(&client.server_energy.to_le_bytes());
        client.last_position =
            Some(large_position_from_client(player_id, &previous).expect("previous position"));
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(anchor_tick);

        let initial_state = crate::state::ServerSimMovement {
            position_x: start_x as f32,
            position_y: start_y as f32,
            orientation: initial_orientation,
            velocity_x: initial_velocity_x,
            velocity_y: initial_velocity_y,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let anchor = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: initial_state,
            wire_x: start_x,
            wire_y: start_y,
            wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                initial_velocity_x,
            ),
            wire_velocity_y: crate::continuum_position::protocol_velocity_component(
                initial_velocity_y,
            ),
            direction,
            door_seed: 0,
            door_phase_tick: 0,
        };
        client.canonical_position_witnesses.clear();
        client.canonical_position_witnesses.push_back(anchor);
        client.canonical_movement_witness = Some(anchor);
        seed_fixture_motion_baseline(client);
        client.server_sim_state = Some(initial_state);
        client.server_sim_anchor_tick = Some(anchor_tick);
        client.server_sim_input_bits = Some(0);
        client.recent_input_records.clear();
        for offset in 1..=3 {
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: previous_sequence + offset,
                    input: crate::state::ClientInputObservation { bits: 0, direction },
                    door_seed: 0,
                    door_phase_tick: 0,
                });
        }
        client.last_input_seq = Some(current_sequence);
        client.last_input_record =
            Some(crate::state::ClientInputObservation { bits: 0, direction });
        locked.active_life_identity(player_id).expect("life")
    };

    let (tail, bounty, energy) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        let replay = crate::sim_observation::replay_canonical_position_gap(
            client,
            locked.zone_map(),
            false,
            false,
            true,
            crate::sim_observation::PlayerMovementUpgrades {
                thrust: client.server_thrust_acceleration,
                top_speed: client.server_top_speed,
                rotation: client.server_rotation_per_second,
                recharge: client.server_recharge_per_second,
                live_values: true,
            },
            &[],
            current_sequence,
            accepted_tick,
            server_spawn_coordinates(locked.zone_map()).0,
            server_spawn_coordinates(locked.zone_map()).1,
        )
        .expect("real Chaos Zone wall path replays");
        assert!(
            replay.any_bounced(),
            "fixture must collide with a real map wall"
        );
        assert!(
            !replay.relocated,
            "fixture is a wall bounce, not a door relocation"
        );
        let tail = replay
            .bounced_tail_witnesses()
            .into_iter()
            .next()
            .expect("exact replay has one bounced tail");
        assert!(replay.final_matches_report(
            tail.direction,
            tail.wire_x,
            tail.wire_y,
            tail.wire_velocity_x,
            tail.wire_velocity_y,
        ));
        assert_eq!(replay.branch_count(), 1);
        (tail, client.server_bounty, client.server_energy)
    };
    let mut exact =
        client_position_packet_with_sequence(0, tail.wire_x, tail.wire_y, current_sequence);
    exact[1] = tail.direction;
    exact[6..8].copy_from_slice(&tail.wire_velocity_x.to_le_bytes());
    exact[14..16].copy_from_slice(&tail.wire_velocity_y.to_le_bytes());
    exact[16..18].copy_from_slice(&bounty.to_le_bytes());
    exact[18..20].copy_from_slice(&energy.to_le_bytes());
    set_client_position_canonical_state(
        &mut exact,
        tail.state.position_x,
        tail.state.position_y,
        tail.state.velocity_x,
        tail.state.velocity_y,
        tail.state.orientation,
    );

    for field in ["x", "y", "direction", "velocity_x", "velocity_y"] {
        let mut forged = exact.clone();
        match field {
            "x" => forged[12..14].copy_from_slice(&tail.wire_x.wrapping_add(7).to_le_bytes()),
            "y" => forged[8..10].copy_from_slice(&tail.wire_y.wrapping_add(7).to_le_bytes()),
            "direction" => forged[1] = (tail.direction + 1) % 40,
            "velocity_x" => {
                forged[6..8].copy_from_slice(&tail.wire_velocity_x.wrapping_add(17).to_le_bytes())
            }
            "velocity_y" => {
                forged[14..16].copy_from_slice(&tail.wire_velocity_y.wrapping_add(17).to_le_bytes())
            }
            _ => unreachable!(),
        }
        sync_client_position_canonical_state_from_wire(&mut forged);
        let before = witness_admission_state(&state, player_id);
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &forged,
                accepted_tick,
            )
            .is_none(),
            "mutated {field} cannot borrow the canonical bounce waiver",
        );
        assert_eq!(
            witness_admission_state(&state, player_id),
            before,
            "rejected {field} mutation must be atomic",
        );
    }

    for field in ["raw_x", "raw_y", "raw_vx", "raw_vy", "raw_orientation"] {
        let mut forged = exact.clone();
        let (x, y, vx, vy, orientation) = match field {
            "raw_x" => (
                tail.state.position_x.next_up(),
                tail.state.position_y,
                tail.state.velocity_x,
                tail.state.velocity_y,
                tail.state.orientation,
            ),
            "raw_y" => (
                tail.state.position_x,
                tail.state.position_y.next_up(),
                tail.state.velocity_x,
                tail.state.velocity_y,
                tail.state.orientation,
            ),
            "raw_vx" => (
                tail.state.position_x,
                tail.state.position_y,
                tail.state.velocity_x.next_up(),
                tail.state.velocity_y,
                tail.state.orientation,
            ),
            "raw_vy" => (
                tail.state.position_x,
                tail.state.position_y,
                tail.state.velocity_x,
                tail.state.velocity_y.next_up(),
                tail.state.orientation,
            ),
            "raw_orientation" => (
                tail.state.position_x,
                tail.state.position_y,
                tail.state.velocity_x,
                tail.state.velocity_y,
                tail.state.orientation.next_up(),
            ),
            _ => unreachable!(),
        };
        set_client_position_canonical_state(&mut forged, x, y, vx, vy, orientation);
        assert!(
            crate::continuum_position::parse_client_position_snapshot(&forged).is_some(),
            "one-ULP {field} mutation remains inside the legacy wire encoding"
        );
        let before = witness_admission_state(&state, player_id);
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &forged,
                accepted_tick,
            )
            .is_none(),
            "mutated {field} cannot borrow the exact bounce witness",
        );
        assert_eq!(
            witness_admission_state(&state, player_id),
            before,
            "rejected {field} mutation must be atomic",
        );
    }

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &exact,
            accepted_tick,
        )
        .is_some(),
        "the exact real-wall replay tail must receive the bounce waiver",
    );
    let locked = state.lock().expect("state");
    let client = &locked.clients[&player_id];
    assert_eq!(client.last_position_sequence, Some(current_sequence));
    let committed = client
        .canonical_movement_witness
        .expect("committed witness");
    assert_eq!(committed.sequence, tail.sequence);
    assert_eq!(committed.wire_x, tail.wire_x);
    assert_eq!(committed.wire_y, tail.wire_y);
}

#[test]
fn exact_wormhole_replay_commits_spawn_and_authoritative_energy_atomically() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let previous_sequence = 31_000_u16;
    let current_sequence = previous_sequence + 1;
    let anchor_tick = 9_000_u32;
    let accepted_tick = anchor_tick + 1;
    let direction = 10;
    let orientation = 0.25_f32;
    let (start_x, start_y) = {
        let map = zone_map();
        map.tiles
            .iter()
            .enumerate()
            .find_map(|(index, &tile)| {
                let tile_x = index % MAP_TILES;
                let tile_y = index / MAP_TILES;
                (tile == 220 && tile_x > 0 && !solid_tile(map.tiles[index - 1])).then_some((
                    tile_x as f32 * TILE_SIZE_PX - 1.0,
                    tile_y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5,
                ))
            })
            .expect("Chaos Zone has a passable horizontal wormhole approach")
    };
    let identity = {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.reset_server_runtime_for_ship_at_tick(anchor_tick);
        grant_legacy_capture_loadout(client, anchor_tick);
        client.life_position_sequence_base = None;
        client.alive = true;
        client.respawn_at_tick = None;
        client.server_energy = 1_699;
        client.server_max_energy = 1_700;
        client.server_energy_remainder = 0.75;
        client.server_thrust_acceleration = 0.0;
        client.server_top_speed = 1_000.0;
        client.server_rotation_per_second = 0.0;
        client.server_recharge_per_second = 0.0;

        let initial_state = crate::state::ServerSimMovement {
            position_x: start_x,
            position_y: start_y,
            orientation,
            velocity_x: 120.0,
            velocity_y: 0.0,
            energy: 1_699.75,
            max_energy: 1_700.0,
        };
        let mut previous = client_position_packet_with_sequence(
            0,
            start_x.round() as u16,
            start_y.round() as u16,
            previous_sequence,
        );
        previous[1] = direction;
        previous[6..8].copy_from_slice(
            &crate::continuum_position::protocol_velocity_component(initial_state.velocity_x)
                .to_le_bytes(),
        );
        previous[14..16].copy_from_slice(&0_i16.to_le_bytes());
        previous[16..18].copy_from_slice(&client.server_bounty.to_le_bytes());
        previous[18..20].copy_from_slice(&client.server_energy.to_le_bytes());
        set_client_position_canonical_state(
            &mut previous,
            initial_state.position_x,
            initial_state.position_y,
            initial_state.velocity_x,
            initial_state.velocity_y,
            initial_state.orientation,
        );
        client.last_position =
            Some(large_position_from_client(player_id, &previous).expect("previous position"));
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(anchor_tick);
        let witness = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: initial_state,
            wire_x: start_x.round() as u16,
            wire_y: start_y.round() as u16,
            wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                initial_state.velocity_x,
            ),
            wire_velocity_y: 0,
            direction,
            door_seed: 0,
            door_phase_tick: anchor_tick,
        };
        client.canonical_position_witnesses.clear();
        client.canonical_position_witnesses.push_back(witness);
        client.canonical_movement_witness = Some(witness);
        seed_fixture_motion_baseline(client);
        client.server_sim_state = Some(initial_state);
        client.server_sim_anchor_tick = Some(anchor_tick);
        client.server_sim_input_bits = Some(0);
        let input = crate::state::ClientInputObservation { bits: 0, direction };
        client.recent_input_records.clear();
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: current_sequence,
                input,
                door_seed: 0,
                door_phase_tick: anchor_tick,
            });
        client.last_input_seq = Some(current_sequence);
        client.last_input_record = Some(input);
        locked.active_life_identity(player_id).expect("life")
    };

    {
        let mut locked = state.lock().expect("state");
        locked.record_projectile(ServerProjectile {
            id: 990_001,
            shot_group_id: 990_001,
            shot_index: 0,
            owner_id: player_id,
            owner_life_seq: identity.life_seq,
            combat_timeline: None,
            presentation_source: None,
            weapon_data: crate::gameplay_rules::encode_weapon_data(1, 1, false, 1, 0, false),
            x: start_x.round() as u16,
            y: start_y.round() as u16,
            timestamp: anchor_tick,
            created_tick: anchor_tick,
            velocity_x: 0,
            velocity_y: 0,
            expiry_tick: None,
            pos_x: start_x,
            source_server_tick: None,
            precise_velocity: None,
            pos_y: start_y,
            bounces_remaining: 0,
            behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        });
    }

    let tail = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        let (spawn_x, spawn_y) = server_spawn_coordinates(locked.zone_map());
        let replay = crate::sim_observation::replay_canonical_position_gap(
            client,
            locked.zone_map(),
            false,
            false,
            false,
            crate::sim_observation::movement_upgrades_for_client(client),
            &[],
            current_sequence,
            accepted_tick,
            spawn_x,
            spawn_y,
        )
        .expect("exact entering input replays");
        assert!(replay.wormhole_relocated);
        *replay.witnesses.last().expect("wormhole tail")
    };
    let mut packet =
        client_position_packet_with_sequence(0, tail.wire_x, tail.wire_y, current_sequence);
    packet[1] = tail.direction;
    packet[6..8].copy_from_slice(&tail.wire_velocity_x.to_le_bytes());
    packet[14..16].copy_from_slice(&tail.wire_velocity_y.to_le_bytes());
    packet[18..20].copy_from_slice(&340_u16.to_le_bytes());
    set_client_position_canonical_state(
        &mut packet,
        tail.state.position_x,
        tail.state.position_y,
        tail.state.velocity_x,
        tail.state.velocity_y,
        tail.state.orientation,
    );

    let mut combined_side_effect = packet.clone();
    combined_side_effect[20..22].copy_from_slice(
        &crate::gameplay_rules::encode_weapon_data(1, 1, false, 1, 0, false).to_le_bytes(),
    );
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &combined_side_effect,
            accepted_tick,
        )
        .is_none(),
        "a wormhole terminal cannot smuggle a same-step weapon side effect",
    );
    {
        let rejected = state.lock().expect("state");
        assert_eq!(rejected.clients[&player_id].server_energy, 1_699);
        assert_eq!(
            rejected.clients[&player_id].last_position_sequence,
            Some(previous_sequence)
        );
        assert_eq!(rejected.active_projectiles.len(), 1);
    }

    let accepted = crate::arena::accept_position_relay_for_life_at_tick(
        &state,
        identity,
        &packet,
        accepted_tick,
    )
    .expect("only the exact server-replayed wormhole terminal is accepted");
    let committed = state.lock().expect("state");
    let client = &committed.clients[&player_id];
    assert_eq!(client.server_energy, 339);
    assert!((client.server_energy_remainder - 0.95).abs() < 0.000_1);
    assert_eq!(
        u16::from_le_bytes(
            accepted.position[21..23]
                .try_into()
                .expect("accepted energy"),
        ),
        339,
    );
    assert_eq!(client.last_position_sequence, Some(current_sequence));
    let canonical = client
        .canonical_movement_witness
        .expect("committed wormhole witness");
    assert_eq!(
        canonical.state.position_x.to_bits(),
        tail.state.position_x.to_bits()
    );
    assert_eq!(
        canonical.state.position_y.to_bits(),
        tail.state.position_y.to_bits()
    );
    assert_eq!(canonical.state.velocity_x.to_bits(), 0.0_f32.to_bits());
    assert_eq!(canonical.state.velocity_y.to_bits(), 0.0_f32.to_bits());
    assert_eq!(canonical.state.energy.to_bits(), 339.0_f32.to_bits());
    assert!(
        committed.active_projectiles.is_empty(),
        "the accepted terminal retires every pre-relocation owner projectile",
    );
}

#[test]
fn accepted_bomb_recoil_preserves_exact_float_lineage_for_a_later_wall_bounce() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 1);
    let previous_sequence = 30_100_u16;
    let bomb_sequence = previous_sequence + 1;
    let anchor_tick = 4_000_u32;
    let radius = arena_ship_radius_pixels(1);
    let (start_x, start_y) = {
        let map = zone_map();
        (2..MAP_TILES - 2)
            .find_map(|tile_y| {
                (2..MAP_TILES - 2).find_map(|tile_x| {
                    let tile = map.tiles[tile_y * MAP_TILES + tile_x];
                    if !solid_tile(tile) || (162..=169).contains(&tile) {
                        return None;
                    }
                    let start_x = (tile_x as f32 * TILE_SIZE_PX - radius - 14.0).round() as u16;
                    let start_y =
                        (tile_y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5).round() as u16;
                    (!reported_position_hull_overlaps_blocked_tile(
                        start_x,
                        start_y,
                        radius,
                        MAP_TILES,
                        TILE_SIZE_PX,
                        |x, y| {
                            map.tiles
                                .get(y * MAP_TILES + x)
                                .is_none_or(|&id| position_blocking_tile_for_door_seeds(id, &[0]))
                        },
                    ))
                    .then_some((start_x, start_y))
                })
            })
            .expect("Chaos Zone contains a clear bomb-recoil approach to a static wall")
    };
    let orientation = 0.25_f32;
    let direction = crate::continuum_position::protocol_direction(orientation);
    let initial_velocity_x = 500.0_f32;
    let bomb_weapon = crate::gameplay_rules::encode_weapon_data(4, 2, false, 3, 8, false);
    let identity = {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.reset_server_runtime_for_ship_at_tick(anchor_tick);
        grant_legacy_capture_loadout(client, anchor_tick);
        client.life_position_sequence_base = None;
        client.alive = true;
        client.respawn_at_tick = None;
        client.server_repel_until_tick = Some(anchor_tick + 1_000);

        let mut previous =
            client_position_packet_with_sequence(0, start_x, start_y, previous_sequence);
        previous[1] = direction;
        previous[6..8].copy_from_slice(
            &crate::continuum_position::protocol_velocity_component(initial_velocity_x)
                .to_le_bytes(),
        );
        previous[16..18].copy_from_slice(&client.server_bounty.to_le_bytes());
        previous[18..20].copy_from_slice(&client.server_energy.to_le_bytes());
        client.last_position =
            Some(large_position_from_client(player_id, &previous).expect("previous position"));
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(anchor_tick);

        let initial_state = crate::state::ServerSimMovement {
            position_x: start_x as f32,
            position_y: start_y as f32,
            orientation,
            velocity_x: initial_velocity_x,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let anchor = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: initial_state,
            wire_x: start_x,
            wire_y: start_y,
            wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                initial_velocity_x,
            ),
            wire_velocity_y: 0,
            direction,
            door_seed: 0,
            door_phase_tick: 0,
        };
        client.canonical_position_witnesses.clear();
        client.canonical_position_witnesses.push_back(anchor);
        client.canonical_movement_witness = Some(anchor);
        seed_fixture_motion_baseline(client);
        client.server_sim_state = Some(initial_state);
        client.server_sim_anchor_tick = Some(anchor_tick);
        client.server_sim_input_bits = Some(0);
        client.recent_input_records.clear();
        for offset in 1..=8_u16 {
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: previous_sequence + offset,
                    input: crate::state::ClientInputObservation { bits: 0, direction },
                    door_seed: 0,
                    door_phase_tick: 0,
                });
        }
        client.last_input_seq = Some(previous_sequence + 8);
        client.last_input_record =
            Some(crate::state::ClientInputObservation { bits: 0, direction });
        locked.active_life_identity(player_id).expect("life")
    };
    for offset in 1..=8_u16 {
        seed_combat_view_decision_for_test(
            &state,
            player_id,
            previous_sequence.wrapping_add(offset),
            anchor_tick + 2,
        );
    }

    let (bomb_tail, bounty, energy) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        let mut replay = crate::sim_observation::replay_canonical_position_gap(
            client,
            locked.zone_map(),
            false,
            false,
            true,
            crate::sim_observation::PlayerMovementUpgrades {
                thrust: client.server_thrust_acceleration,
                top_speed: client.server_top_speed,
                rotation: client.server_rotation_per_second,
                recharge: client.server_recharge_per_second,
                live_values: true,
            },
            &[],
            bomb_sequence,
            anchor_tick + 2,
            server_spawn_coordinates(locked.zone_map()).0,
            server_spawn_coordinates(locked.zone_map()).1,
        )
        .expect("bomb movement record replays");
        assert!(!replay.bounced, "recoil fixture must fire before the wall");
        assert!(replay.apply_bomb_recoil_to_tail(
            crate::arena_settings::arena_ship_bomb_recoil_speed(client.player.ship)
        ));
        (
            replay.witnesses.last().copied().expect("bomb tail"),
            client.server_bounty,
            client.server_energy,
        )
    };
    let mut bomb =
        client_position_packet_with_sequence(0, bomb_tail.wire_x, bomb_tail.wire_y, bomb_sequence);
    bomb[1] = bomb_tail.direction;
    bomb[6..8].copy_from_slice(&bomb_tail.wire_velocity_x.to_le_bytes());
    bomb[14..16].copy_from_slice(&bomb_tail.wire_velocity_y.to_le_bytes());
    bomb[16..18].copy_from_slice(&bounty.to_le_bytes());
    bomb[18..20].copy_from_slice(&energy.to_le_bytes());
    bomb[20..22].copy_from_slice(&bomb_weapon.to_le_bytes());
    set_client_position_canonical_state(
        &mut bomb,
        bomb_tail.state.position_x,
        bomb_tail.state.position_y,
        bomb_tail.state.velocity_x,
        bomb_tail.state.velocity_y,
        bomb_tail.state.orientation,
    );
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &bomb,
            anchor_tick + 2,
        )
        .is_some(),
        "an accepted bomb position must match the exact post-movement recoil tail",
    );
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        let committed = client.canonical_movement_witness.expect("recoil witness");
        assert_eq!(committed.sequence, bomb_sequence);
        assert_eq!(committed.state.position_x, bomb_tail.state.position_x);
        assert_eq!(committed.state.position_y, bomb_tail.state.position_y);
        assert_eq!(committed.state.velocity_x, bomb_tail.state.velocity_x);
        assert_eq!(committed.state.velocity_y, bomb_tail.state.velocity_y);
    }

    let (bounce_sequence, bounce_tick, bounce_tail, bounty, energy) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        let mut found = None;
        for offset in 2..=8_u16 {
            let sequence = previous_sequence + offset;
            let tick = anchor_tick + u32::from(offset) * 2;
            let replay = crate::sim_observation::replay_canonical_position_gap(
                client,
                locked.zone_map(),
                false,
                false,
                true,
                crate::sim_observation::PlayerMovementUpgrades {
                    thrust: client.server_thrust_acceleration,
                    top_speed: client.server_top_speed,
                    rotation: client.server_rotation_per_second,
                    recharge: client.server_recharge_per_second,
                    live_values: true,
                },
                &[],
                sequence,
                tick,
                server_spawn_coordinates(locked.zone_map()).0,
                server_spawn_coordinates(locked.zone_map()).1,
            )
            .expect("post-recoil movement replays");
            if replay.bounced {
                found = Some((
                    sequence,
                    tick,
                    replay.witnesses.last().copied().expect("bounce tail"),
                ));
                break;
            }
        }
        let (sequence, tick, tail) = found.expect("post-recoil path reaches the real wall");
        (
            sequence,
            tick,
            tail,
            client.server_bounty,
            client.server_energy,
        )
    };
    let mut exact = client_position_packet_with_sequence(
        0,
        bounce_tail.wire_x,
        bounce_tail.wire_y,
        bounce_sequence,
    );
    exact[1] = bounce_tail.direction;
    exact[6..8].copy_from_slice(&bounce_tail.wire_velocity_x.to_le_bytes());
    exact[14..16].copy_from_slice(&bounce_tail.wire_velocity_y.to_le_bytes());
    exact[16..18].copy_from_slice(&bounty.to_le_bytes());
    exact[18..20].copy_from_slice(&energy.to_le_bytes());
    set_client_position_canonical_state(
        &mut exact,
        bounce_tail.state.position_x,
        bounce_tail.state.position_y,
        bounce_tail.state.velocity_x,
        bounce_tail.state.velocity_y,
        bounce_tail.state.orientation,
    );

    for field in ["x", "y", "direction", "velocity_x", "velocity_y"] {
        let mut forged = exact.clone();
        match field {
            "x" => {
                forged[12..14].copy_from_slice(&bounce_tail.wire_x.wrapping_add(1).to_le_bytes())
            }
            "y" => forged[8..10].copy_from_slice(&bounce_tail.wire_y.wrapping_add(1).to_le_bytes()),
            "direction" => forged[1] = (bounce_tail.direction + 1) % 40,
            "velocity_x" => forged[6..8]
                .copy_from_slice(&bounce_tail.wire_velocity_x.wrapping_add(1).to_le_bytes()),
            "velocity_y" => forged[14..16]
                .copy_from_slice(&bounce_tail.wire_velocity_y.wrapping_add(1).to_le_bytes()),
            _ => unreachable!(),
        }
        sync_client_position_canonical_state_from_wire(&mut forged);
        let before = witness_admission_state(&state, player_id);
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &forged,
                bounce_tick,
            )
            .is_none(),
            "post-recoil bounce mutation {field} must fail closed",
        );
        assert_eq!(
            witness_admission_state(&state, player_id),
            before,
            "post-recoil bounce mutation {field} must be atomic",
        );
    }
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &exact,
            bounce_tick,
        )
        .is_some(),
        "the later exact wall bounce must retain the pre-bounce recoil lineage",
    );
    let locked = state.lock().expect("state");
    let committed = locked.clients[&player_id]
        .canonical_movement_witness
        .expect("bounce witness committed");
    assert_eq!(committed.sequence, bounce_tail.sequence);
    assert_eq!(committed.state.position_x, bounce_tail.state.position_x);
    assert_eq!(committed.state.position_y, bounce_tail.state.position_y);
    assert_eq!(committed.state.orientation, bounce_tail.state.orientation);
    assert_eq!(committed.state.velocity_x, bounce_tail.state.velocity_x);
    assert_eq!(committed.state.velocity_y, bounce_tail.state.velocity_y);
    assert_eq!(committed.wire_x, bounce_tail.wire_x);
    assert_eq!(committed.wire_y, bounce_tail.wire_y);
    assert_eq!(committed.wire_velocity_x, bounce_tail.wire_velocity_x);
    assert_eq!(committed.wire_velocity_y, bounce_tail.wire_velocity_y);
    assert_eq!(committed.direction, bounce_tail.direction);
}

#[test]
fn server_translates_client_position_to_large_player_position() {
    let mut packet = vec![0; 22];
    packet[0] = CLIENT_POSITION;
    packet[1] = 7;
    packet[2..6].copy_from_slice(&0x1122_3344_u32.to_le_bytes());
    packet[6..8].copy_from_slice(&(-32_i16).to_le_bytes());
    packet[8..10].copy_from_slice(&700_u16.to_le_bytes());
    packet[11] = 0x55;
    packet[12..14].copy_from_slice(&500_u16.to_le_bytes());
    packet[14..16].copy_from_slice(&64_i16.to_le_bytes());
    packet[16..18].copy_from_slice(&3_u16.to_le_bytes());
    packet[18..20].copy_from_slice(&900_u16.to_le_bytes());
    packet[20..22].copy_from_slice(&0x83_u16.to_le_bytes());
    packet.resize(crate::continuum_position::CLIENT_POSITION_EXTRA_LEN, 0);
    crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(7, 500, 700, -32, 64)
        .encode(&mut packet);

    let position = large_position_from_client(9, &packet).expect("position should translate");

    assert_eq!(position[0], 0x05);
    assert_eq!(position[1], 7);
    assert_eq!(u16::from_le_bytes(position[2..4].try_into().unwrap()), 0);
    assert_eq!(u16::from_le_bytes(position[4..6].try_into().unwrap()), 500);
    assert_eq!(i16::from_le_bytes(position[6..8].try_into().unwrap()), 64);
    assert_eq!(u16::from_le_bytes(position[8..10].try_into().unwrap()), 9);
    assert_eq!(
        i16::from_le_bytes(position[10..12].try_into().unwrap()),
        -32
    );
    assert_eq!(position[13], 0x55);
    assert_eq!(
        u16::from_le_bytes(position[15..17].try_into().unwrap()),
        700
    );
    assert_eq!(u16::from_le_bytes(position[17..19].try_into().unwrap()), 3);
    assert_eq!(
        u16::from_le_bytes(position[19..21].try_into().unwrap()),
        0x83
    );
    assert_eq!(
        u16::from_le_bytes(position[21..23].try_into().unwrap()),
        900
    );

    let max_world = (MAP_TILES as f32 * TILE_SIZE_PX) as u16;
    let mut out_of_bounds_x = packet.clone();
    out_of_bounds_x[12..14].copy_from_slice(&max_world.to_le_bytes());
    assert!(large_position_from_client(9, &out_of_bounds_x).is_none());

    let mut out_of_bounds_y = packet.clone();
    out_of_bounds_y[8..10].copy_from_slice(&max_world.to_le_bytes());
    assert!(large_position_from_client(9, &out_of_bounds_y).is_none());
}

#[test]
fn server_caches_positions_and_replays_them_to_late_joiners() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let a = register_connection(&state, tx_a);
    let b = register_connection(&state, tx_b);
    join_arena(&state, a, 0);
    drain_available(&rx_a);

    let position = client_position_packet(0x1122_3344, 500, 700);
    broadcast_position_observable(&state, a, &position);

    {
        let state = state.lock().expect("server state should not be poisoned");
        let cached = state
            .clients
            .get(&a)
            .and_then(|client| client.last_position.as_ref())
            .expect("position should cache");
        assert_eq!(cached[0], 0x05);
        assert_eq!(u16::from_le_bytes(cached[8..10].try_into().unwrap()), a);
        assert_eq!(u16::from_le_bytes(cached[4..6].try_into().unwrap()), 500);
        assert_eq!(u16::from_le_bytes(cached[15..17].try_into().unwrap()), 700);
    }

    join_arena(&state, b, 0);
    let replayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(replayed[0], 0x05);
    assert_eq!(u16::from_le_bytes(replayed[8..10].try_into().unwrap()), a);
    assert_eq!(u16::from_le_bytes(replayed[4..6].try_into().unwrap()), 500);
    assert_eq!(
        u16::from_le_bytes(replayed[15..17].try_into().unwrap()),
        700
    );
}

#[test]
fn server_skips_high_rate_position_fanout_to_far_known_recipients() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let beta_position =
            large_position_from_client(beta, &client_position_packet(0x1122_3344, 8_000, 8_000))
                .expect("test position should translate");
        state.set_client_position(beta, beta_position);
    }

    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 500, 700),
    );

    assert!(rx_b.try_recv().is_err());
    let state = state.lock().expect("server state should not be poisoned");
    assert!(
        state
            .clients
            .get(&alpha)
            .and_then(|client| client.last_position.as_ref())
            .is_some()
    );
}

#[test]
fn server_uses_server_position_ticks_instead_of_client_timestamps() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let position = client_position_packet(0x1122_3344, 500, 700);
    broadcast_position_observable(&state, alpha, &position);
    let first = read_non_reliable_from_channel(&rx_b);
    assert_eq!(first[0], 0x05);

    broadcast_position_observable(&state, alpha, &position);
    let _ = rx_b.try_recv();

    let older_position = client_position_packet(0x1122_3343, 499, 699);
    broadcast_position_observable(&state, alpha, &older_position);
    let _ = rx_b.try_recv();

    {
        let state = state.lock().expect("server state should not be poisoned");
        let last = state
            .clients
            .get(&alpha)
            .and_then(|client| client.last_position_server_tick);
        assert!(last.is_some());
        assert_ne!(last, Some(0x1122_3344));
    }
    reset_position_rate_limit(&state, alpha);

    let next_position = client_position_packet(0x1122_3345, 501, 701);
    broadcast_position_observable(&state, alpha, &next_position);
    let next = read_non_reliable_from_channel(&rx_b);
    assert_eq!(first[8..10], next[8..10]);
    assert_eq!(u16::from_le_bytes(next[4..6].try_into().unwrap()), 501);

    broadcast_position_observable(&state, alpha, &[CLIENT_POSITION, 7]);
    assert!(rx_b.try_recv().is_err());

    let mut invalid_weapon = client_position_packet(0x1122_3346, 502, 702);
    invalid_weapon[20..22].copy_from_slice(&31_u16.to_le_bytes());
    assert!(large_position_from_client(alpha, &invalid_weapon).is_none());
    broadcast_position_observable(&state, alpha, &invalid_weapon);
    assert!(rx_b.try_recv().is_err());

    let allowed_burst_position = client_position_packet(0x1122_3347, 503, 703);
    broadcast_position_observable(&state, alpha, &allowed_burst_position);
    let allowed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(allowed[4..6].try_into().unwrap()), 503);

    let rate_limited_position = client_position_packet(0x1122_3348, 504, 704);
    broadcast_position_observable(&state, alpha, &rate_limited_position);
    let _ = rx_b.try_recv();
    reset_position_rate_limit(&state, alpha);

    let mut weapon_during_movement_burst = client_position_packet(0x1122_3349, 505, 705);
    weapon_during_movement_burst[20..22].copy_from_slice(&3_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &weapon_during_movement_burst);
    let relayed_weapon = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed_weapon[19..21].try_into().unwrap()),
        3
    );
}

#[test]
fn server_rejects_nonzero_legacy_position_timestamps() {
    // Delta, not absolute: `cargo test` runs these in parallel threads in one
    // process and the reject counters are process-global, so an absolute count
    // depends on which other test reset them last.
    let rejects_before = crate::security::peek_security_reject_count_for_test(
        "position",
        "client_timestamp_nonzero",
    );
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_b);

    let mut forged = client_position_packet(0, 500, 700);
    forged[2..6].copy_from_slice(&0x1122_3344_u32.to_le_bytes());
    broadcast_position_observable(&state, alpha, &forged);

    assert!(rx_b.try_recv().is_err());
    assert!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&alpha)
            .and_then(|client| client.last_position.as_ref())
            .is_none()
    );
    assert_eq!(
        crate::security::peek_security_reject_count_for_test(
            "position",
            "client_timestamp_nonzero"
        ) - rejects_before,
        1
    );
}

#[test]
fn server_rejects_impossible_reported_position_velocity() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let mut impossible = client_position_packet(0x1122_3344, 500, 700);
    impossible[6..8].copy_from_slice(&30_000_i16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &impossible);
    assert!(rx_b.try_recv().is_err());
    {
        let state = state.lock().expect("server state should not be poisoned");
        assert!(
            state
                .clients
                .get(&alpha)
                .and_then(|client| client.last_position_server_tick)
                .is_none()
        );
    }

    let valid = client_position_packet(0x1122_3345, 501, 701);
    broadcast_position_observable(&state, alpha, &valid);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(relayed[0], 0x05);
    assert_eq!(u16::from_le_bytes(relayed[4..6].try_into().unwrap()), 501);
}

#[test]
fn server_rejects_impossible_reported_position_acceleration() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let first = client_position_packet(0x1122_3344, 500, 700);
    broadcast_position_observable(&state, alpha, &first);
    assert_eq!(read_non_reliable_from_channel(&rx_b)[0], 0x05);

    let mut impossible = client_position_packet(0x1122_3345, 501, 701);
    impossible[6..8].copy_from_slice(&10_000_i16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &impossible);
    assert!(rx_b.try_recv().is_err());
    {
        let state = state.lock().expect("server state should not be poisoned");
        let last = state
            .clients
            .get(&alpha)
            .and_then(|client| client.last_position_server_tick);
        assert!(last.is_some());
        assert_ne!(last, Some(0x1122_3344));
    }

    let valid = client_position_packet(0x1122_3346, 502, 702);
    broadcast_position_observable(&state, alpha, &valid);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(relayed[0], 0x05);
    assert_eq!(u16::from_le_bytes(relayed[4..6].try_into().unwrap()), 502);
}

#[test]
fn server_rejects_impossible_reported_position_energy() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let mut impossible = client_position_packet(0x1122_3344, 500, 700);
    impossible[18..20].copy_from_slice(&1_701_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &impossible);
    assert!(rx_b.try_recv().is_err());

    let mut valid = client_position_packet(0x1122_3345, 501, 701);
    valid[18..20].copy_from_slice(&1_700_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &valid);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(relayed[0], 0x05);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        1_700
    );
}

#[test]
fn server_rejects_position_centers_inside_solid_tiles() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let (solid_x, solid_y) = first_solid_tile_position();
    let inside_wall = client_position_packet(0x1122_3344, solid_x, solid_y);
    broadcast_position_observable(&state, alpha, &inside_wall);
    assert!(rx_b.try_recv().is_err());
    {
        let state = state.lock().expect("server state should not be poisoned");
        assert!(
            state
                .clients
                .get(&alpha)
                .and_then(|client| client.last_position_server_tick)
                .is_none()
        );
    }

    let valid = client_position_packet(0x1122_3345, 501, 701);
    broadcast_position_observable(&state, alpha, &valid);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(relayed[0], 0x05);
    assert_eq!(u16::from_le_bytes(relayed[4..6].try_into().unwrap()), 501);
}

#[test]
fn exact_record_phase_blocks_early_next_door_passage_despite_adjacent_open_seed() {
    let accepted_tick = 40_000_u32;
    let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
    let (door_state, current, next) = (2_u32..=512)
        .find_map(|current_count| {
            let state =
                crate::doors::ClientDoorState::for_challenge(crate::continuum::continuum_tick_sub(
                    accepted_tick,
                    current_count.saturating_mul(delay),
                ));
            let phases =
                crate::doors::client_door_phase_candidates_at_tick(Some(state), accepted_tick);
            let current = phases.get(1).copied()?;
            let next = phases.get(2).copied()?;
            (crate::zone::position_blocking_tile_with_door_seed(162, current.seed)
                && !crate::zone::position_blocking_tile_with_door_seed(162, next.seed))
            .then_some((state, current, next))
        })
        .expect("canonical stream reaches closed-current/open-next door 162");
    assert!(
        !position_blocking_tile_for_door_seeds(162, &[current.seed, next.seed]),
        "the removed union-any admission would have failed open"
    );

    let (door_x, door_y) = zone_map()
        .tiles
        .iter()
        .enumerate()
        .find_map(|(index, &tile)| {
            if tile != 162 {
                return None;
            }
            let x = ((index % MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5) as u16;
            let y = ((index / MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5) as u16;
            let radius = (arena_ship_radius_pixels(0) - 1.0).max(0.0);
            (!crate::continuum_position::reported_position_hull_overlaps_blocked_tile(
                x,
                y,
                radius,
                MAP_TILES,
                TILE_SIZE_PX,
                |tile_x, tile_y| {
                    zone_map()
                        .tiles
                        .get(tile_y * MAP_TILES + tile_x)
                        .is_none_or(|&candidate| {
                            crate::zone::position_blocking_tile_with_door_seed(candidate, next.seed)
                        })
                },
            ))
            .then_some((x, y))
        })
        .expect("real map has a door-162 hull passable under the next seed");

    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let sequence = 77_u16;
    let identity = {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("client");
        client.life_position_sequence_base = None;
        client.door_state = Some(door_state);
        let input = crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        };
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence,
                input,
                door_seed: current.seed,
                door_phase_tick: current.started_tick,
            });
        client.last_input_seq = Some(sequence);
        client.last_input_record = Some(input);
        state.active_life_identity(player_id).expect("life")
    };
    let (energy, bounty) = {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        (client.server_energy, client.server_bounty)
    };
    let mut packet = client_position_packet_with_sequence(0, door_x, door_y, sequence);
    packet[1] = 0;
    packet[16..18].copy_from_slice(&bounty.to_le_bytes());
    packet[18..20].copy_from_slice(&energy.to_le_bytes());

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            accepted_tick,
        )
        .is_none(),
        "exact current closed phase must win over the adjacent open seed"
    );
    let client = &state.lock().expect("state").clients[&player_id];
    assert!(client.last_position.is_none());
    assert!(client.last_position_server_tick.is_none());
}

#[test]
fn admission_replays_open_crossing_before_closed_tail_and_rolls_back_closed_crossing() {
    let anchor = 0x7fff_ff00;
    let door_state = crate::doors::ClientDoorState::for_challenge(anchor);
    let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
    let (door_tile, open_phase, closed_phase) = (162_u8..=169)
        .find_map(|door_tile| {
            (0_u32..512).find_map(|count| {
                let before_tick =
                    crate::continuum::continuum_tick_add(anchor, count.saturating_mul(delay));
                let after_tick = crate::continuum::continuum_tick_add(before_tick, delay);
                let phases = crate::doors::client_door_phase_sequence(
                    Some(door_state),
                    before_tick,
                    after_tick,
                )?;
                let before = *phases.first()?;
                let after = *phases.last()?;
                (!crate::zone::position_blocking_tile_with_door_seed(door_tile, before.seed)
                    && crate::zone::position_blocking_tile_with_door_seed(door_tile, after.seed))
                .then_some((door_tile, before, after))
            })
        })
        .expect("wrapped canonical phases include an open-to-closed dynamic tile");
    let ship = 0;
    let radius = arena_ship_radius_pixels(ship);
    let speed = crate::arena_settings::arena_ship_initial_top_speed_pixels_per_second(ship);
    let crossing_distance = TILE_SIZE_PX + radius * 2.0 + 4.0;
    let open_steps = (crossing_distance / (speed / 60.0)).ceil() as u16 + 1;
    assert!(open_steps + 1 < crate::state::MAX_RECENT_INPUT_OBSERVATIONS as u16);
    let total_steps = open_steps + 1;
    let map = zone_map();
    let (start_x, start_y, velocity_x) = map
        .tiles
        .iter()
        .enumerate()
        .filter(|(_, tile)| **tile == door_tile)
        .find_map(|(index, _)| {
            let tile_x = index % MAP_TILES;
            let tile_y = index / MAP_TILES;
            let center_y = (tile_y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5).round();
            [1.0_f32, -1.0].into_iter().find_map(|sign| {
                let start = if sign > 0.0 {
                    tile_x as f32 * TILE_SIZE_PX - radius - 2.0
                } else {
                    (tile_x + 1) as f32 * TILE_SIZE_PX + radius + 2.0
                };
                let end = start + sign * speed * total_steps as f32 / 60.0;
                if !(0.0..u16::MAX as f32).contains(&start)
                    || !(0.0..u16::MAX as f32).contains(&end)
                    || !(0.0..u16::MAX as f32).contains(&center_y)
                {
                    return None;
                }
                let start = start.round() as u16;
                let end = end.round() as u16;
                let y = center_y as u16;
                let path_blocked = |seed| {
                    reported_position_path_hull_overlaps_blocked_tile(
                        start,
                        y,
                        end,
                        y,
                        radius,
                        MAP_TILES,
                        TILE_SIZE_PX,
                        |candidate_x, candidate_y| {
                            map.tiles
                                .get(candidate_y * MAP_TILES + candidate_x)
                                .is_none_or(|&tile| {
                                    crate::zone::position_blocking_tile_with_door_seed(tile, seed)
                                })
                        },
                    )
                };
                (!path_blocked(open_phase.seed) && path_blocked(closed_phase.seed)).then_some((
                    start,
                    y,
                    sign * speed,
                ))
            })
        })
        .expect("real map has a clear approach through the selected dynamic door");
    let previous_sequence = u16::MAX - 10;
    let current_sequence = previous_sequence.wrapping_add(total_steps);
    let velocity_wire = crate::continuum_position::protocol_velocity_component(velocity_x);
    let coast = crate::state::ClientInputObservation {
        bits: 0,
        direction: 0,
    };

    let prepare_baseline = |all_closed: bool| {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, ship);
        drain_available(&rx);
        let identity = {
            let mut locked = state.lock().expect("state");
            let identity = locked.active_life_identity(player_id).expect("life");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.life_position_sequence_base = None;
            client.door_state = Some(door_state);
            let baseline_phase = if all_closed { closed_phase } else { open_phase };
            let mut baseline =
                client_position_packet_with_sequence(0, start_x, start_y, previous_sequence);
            baseline[1] = 0;
            baseline[6..8].copy_from_slice(&velocity_wire.to_le_bytes());
            baseline[16..18].copy_from_slice(&client.server_bounty.to_le_bytes());
            baseline[18..20].copy_from_slice(&client.server_energy.to_le_bytes());
            client.last_position =
                Some(large_position_from_client(player_id, &baseline).expect("baseline relay"));
            client.last_position_sequence = Some(previous_sequence);
            client.last_position_server_tick = Some(baseline_phase.started_tick);
            client.alive = true;
            client.respawn_at_tick = None;
            let baseline_state = crate::state::ServerSimMovement {
                position_x: start_x as f32,
                position_y: start_y as f32,
                orientation: 0.0,
                velocity_x: crate::continuum_position::protocol_velocity_pixels_per_second(
                    velocity_wire,
                ),
                velocity_y: 0.0,
                energy: client.server_energy as f32,
                max_energy: client.server_max_energy as f32,
            };
            client.server_sim_state = Some(baseline_state);
            client.server_sim_anchor_tick = Some(baseline_phase.started_tick);
            client.server_sim_input_bits = Some(0);
            let baseline_witness = crate::state::CanonicalPositionWitness {
                sequence: previous_sequence,
                life_seq: client.life_seq,
                state: baseline_state,
                wire_x: start_x,
                wire_y: start_y,
                wire_velocity_x: velocity_wire,
                wire_velocity_y: 0,
                direction: 0,
                door_seed: baseline_phase.seed,
                door_phase_tick: baseline_phase.started_tick,
            };
            client
                .canonical_position_witnesses
                .push_back(baseline_witness);
            client.canonical_movement_witness = Some(baseline_witness);
            seed_fixture_motion_baseline(client);
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence: previous_sequence,
                    input: coast,
                    door_seed: baseline_phase.seed,
                    door_phase_tick: baseline_phase.started_tick,
                });
            client.last_input_seq = Some(previous_sequence);
            client.last_input_record = Some(coast);
            identity
        };
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            for offset in 1..=total_steps {
                let phase = if all_closed || offset > open_steps {
                    closed_phase
                } else {
                    open_phase
                };
                client
                    .recent_input_records
                    .push_back(crate::state::SequencedInputObservation {
                        sequence: previous_sequence.wrapping_add(offset),
                        input: coast,
                        door_seed: phase.seed,
                        door_phase_tick: phase.started_tick,
                    });
            }
            client.last_input_seq = Some(current_sequence);
            client.last_input_record = Some(coast);
        }
        (state, identity, rx)
    };

    let (open_state, open_identity, _open_rx) = prepare_baseline(false);
    {
        let mut locked = open_state.lock().expect("state");
        let client = locked
            .clients
            .get_mut(&open_identity.player_id)
            .expect("client");
        client
            .pending_warp_prize_relocations
            .push_back(crate::state::PendingWarpPrizeRelocation {
                collectible_id: 0x0d00_0042,
                life_seq: client.life_seq,
                expected_energy: client.server_energy,
                expected_bounty: client.server_bounty,
                prize_state_revision: client.server_prize_state_revision,
                claim: crate::state::ServerPrizeClaim {
                    collectible_id: 0x0d00_0042,
                    x: 800,
                    y: 800,
                    prize_id: 7,
                },
                pickup_ack_sent: true,
            });
    }
    let exact_tail = {
        let locked = open_state.lock().expect("state");
        let client = &locked.clients[&open_identity.player_id];
        crate::sim_observation::replay_canonical_position_gap(
            client,
            locked.zone_map(),
            false,
            false,
            false,
            crate::sim_observation::PlayerMovementUpgrades {
                thrust: client.server_thrust_acceleration,
                top_speed: client.server_top_speed,
                rotation: client.server_rotation_per_second,
                recharge: client.server_recharge_per_second,
                live_values: true,
            },
            &[],
            current_sequence,
            closed_phase.started_tick,
            server_spawn_coordinates(locked.zone_map()).0,
            server_spawn_coordinates(locked.zone_map()).1,
        )
        .expect("open crossing reconstructs")
        .witnesses
        .last()
        .copied()
        .expect("tail")
    };
    let (bounty, energy) = {
        let locked = open_state.lock().expect("state");
        let client = &locked.clients[&open_identity.player_id];
        (client.server_bounty, client.server_energy)
    };
    let mut tail = client_position_packet_with_sequence(
        0,
        exact_tail.wire_x,
        exact_tail.wire_y,
        current_sequence,
    );
    tail[1] = exact_tail.direction;
    tail[6..8].copy_from_slice(&exact_tail.wire_velocity_x.to_le_bytes());
    tail[14..16].copy_from_slice(&exact_tail.wire_velocity_y.to_le_bytes());
    tail[16..18].copy_from_slice(&bounty.to_le_bytes());
    tail[18..20].copy_from_slice(&energy.to_le_bytes());
    set_client_position_canonical_state(
        &mut tail,
        exact_tail.state.position_x,
        exact_tail.state.position_y,
        exact_tail.state.velocity_x,
        exact_tail.state.velocity_y,
        exact_tail.state.orientation,
    );
    let open_result = crate::arena::accept_position_relay_for_life_at_tick(
        &open_state,
        open_identity,
        &tail,
        closed_phase.started_tick,
    );
    assert!(
        open_result.is_some(),
        "the exact open-record crossing must admit after the door closes: {:?}",
        witness_admission_state(&open_state, open_identity.player_id)
    );
    assert_eq!(
        open_result
            .as_ref()
            .expect("accepted canonical door relocation")
            .warp_relocation_receipt,
        None,
        "an authenticated door relocation is not a Warp application receipt",
    );
    assert_eq!(
        open_state.lock().expect("state").clients[&open_identity.player_id]
            .pending_warp_prize_relocations
            .front()
            .map(|authorization| authorization.collectible_id),
        Some(0x0d00_0042),
        "an authenticated door relocation cannot consume an outstanding Warp marker",
    );
    let (closed_state, closed_identity, closed_rx) = prepare_baseline(true);
    let before = witness_admission_state(&closed_state, closed_identity.player_id);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &closed_state,
            closed_identity,
            &tail,
            closed_phase.started_tick,
        )
        .is_none(),
        "the same across-door tail must reject when every record is closed"
    );
    assert_eq!(
        witness_admission_state(&closed_state, closed_identity.player_id),
        before,
        "rejected incremental replay must roll back all authority state"
    );
    assert!(closed_rx.try_recv().is_err(), "rejection emits no packet");
}

#[test]
fn canonical_door_close_relocation_cancels_inflight_snapback_transaction() {
    let anchor = 0x6fff_ff00;
    let door_state = crate::doors::ClientDoorState::for_challenge(anchor);
    let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
    let ship = 0;
    let radius = arena_ship_radius_pixels(ship);
    let map = zone_map();
    let (door_x, door_y, open_phase, closed_phase) = map
        .tiles
        .iter()
        .enumerate()
        .filter(|(_, tile)| (162_u8..=169).contains(*tile))
        .find_map(|(index, _)| {
            let x = ((index % MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5).round() as u16;
            let y = ((index / MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5).round() as u16;
            (0_u32..512).find_map(|count| {
                let before_tick =
                    crate::continuum::continuum_tick_add(anchor, count.saturating_mul(delay));
                let after_tick = crate::continuum::continuum_tick_add(before_tick, delay);
                let phases = crate::doors::client_door_phase_sequence(
                    Some(door_state),
                    before_tick,
                    after_tick,
                )?;
                let open = *phases.first()?;
                let closed = *phases.last()?;
                let hull_blocked = |seed| {
                    reported_position_hull_overlaps_blocked_tile(
                        x,
                        y,
                        radius,
                        MAP_TILES,
                        TILE_SIZE_PX,
                        |candidate_x, candidate_y| {
                            map.tiles
                                .get(candidate_y * MAP_TILES + candidate_x)
                                .is_none_or(|&tile| {
                                    crate::zone::position_blocking_tile_with_door_seed(tile, seed)
                                })
                        },
                    )
                };
                (!hull_blocked(open.seed) && hull_blocked(closed.seed))
                    .then_some((x, y, open, closed))
            })
        })
        .expect("real map contains an open-to-closed door hull");

    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, ship);
    drain_available(&rx);
    let previous_sequence = u16::MAX;
    let current_sequence = 0;
    let coast = crate::state::ClientInputObservation {
        bits: 0,
        direction: 0,
    };
    let identity = {
        let mut locked = state.lock().expect("state");
        let identity = locked.active_life_identity(player_id).expect("life");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.life_position_sequence_base = None;
        client.door_state = Some(door_state);
        client.alive = true;
        client.respawn_at_tick = None;
        let baseline_state = crate::state::ServerSimMovement {
            position_x: door_x as f32,
            position_y: door_y as f32,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let mut baseline =
            client_position_packet_with_sequence(0, door_x, door_y, previous_sequence);
        baseline[16..18].copy_from_slice(&client.server_bounty.to_le_bytes());
        baseline[18..20].copy_from_slice(&client.server_energy.to_le_bytes());
        set_client_position_canonical_state(
            &mut baseline,
            door_x as f32,
            door_y as f32,
            0.0,
            0.0,
            0.0,
        );
        client.last_position =
            Some(large_position_from_client(player_id, &baseline).expect("baseline"));
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(open_phase.started_tick);
        client.server_sim_state = Some(baseline_state);
        client.server_sim_anchor_tick = Some(open_phase.started_tick);
        client.server_sim_input_bits = Some(0);
        let witness = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: baseline_state,
            wire_x: door_x,
            wire_y: door_y,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 0,
            door_seed: open_phase.seed,
            door_phase_tick: open_phase.started_tick,
        };
        client.canonical_movement_witness = Some(witness);
        seed_fixture_motion_baseline(client);
        client.canonical_position_witnesses.push_back(witness);
        for (sequence, phase) in [
            (previous_sequence, open_phase),
            (current_sequence, closed_phase),
        ] {
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence,
                    input: coast,
                    door_seed: phase.seed,
                    door_phase_tick: phase.started_tick,
                });
        }
        client.last_input_seq = Some(current_sequence);
        client.last_input_record = Some(coast);
        identity
    };

    let expected = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        let replay = crate::sim_observation::replay_canonical_position_gap(
            client,
            locked.zone_map(),
            false,
            false,
            false,
            crate::sim_observation::PlayerMovementUpgrades {
                thrust: client.server_thrust_acceleration,
                top_speed: client.server_top_speed,
                rotation: client.server_rotation_per_second,
                recharge: client.server_recharge_per_second,
                live_values: true,
            },
            &[],
            current_sequence,
            closed_phase.started_tick,
            server_spawn_coordinates(locked.zone_map()).0,
            server_spawn_coordinates(locked.zone_map()).1,
        )
        .expect("door-close replay");
        assert!(replay.any_relocated());
        replay.witnesses.last().copied().expect("relocated tail")
    };
    let correction =
        arm_snapback_transaction_for_discontinuity(&state, player_id, 0xd001, previous_sequence);
    let (bounty, energy) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        (client.server_bounty, client.server_energy)
    };
    let mut packet =
        client_position_packet_with_sequence(0, expected.wire_x, expected.wire_y, current_sequence);
    packet[1] = expected.direction;
    packet[6..8].copy_from_slice(&expected.wire_velocity_x.to_le_bytes());
    packet[14..16].copy_from_slice(&expected.wire_velocity_y.to_le_bytes());
    packet[16..18].copy_from_slice(&bounty.to_le_bytes());
    packet[18..20].copy_from_slice(&energy.to_le_bytes());
    set_client_position_canonical_state(
        &mut packet,
        expected.state.position_x,
        expected.state.position_y,
        expected.state.velocity_x,
        expected.state.velocity_y,
        expected.state.orientation,
    );

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            closed_phase.started_tick,
        )
        .is_some()
    );
    assert_snapback_transaction_cancelled(&state, player_id, correction);
}

#[test]
fn repeated_door_seed_receipt_relocation_reanchors_the_next_input_tail() {
    let anchor = 0x5fff_ff00;
    let door_state = crate::doors::ClientDoorState::for_challenge(anchor);
    let delay = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
    let map = zone_map();
    let ship = 0;
    let radius = arena_ship_radius_pixels(ship);
    let through_tick = crate::continuum::continuum_tick_add(
        anchor,
        delay.saturating_mul(crate::doors::MAX_DOOR_PHASE_RECONSTRUCTION_STEPS as u32),
    );
    let phases = crate::doors::client_door_phase_sequence(Some(door_state), anchor, through_tick)
        .expect("bounded authenticated door phase stream");
    let (door_x, door_y, predecessor_phase, receipt_phase) = map
        .tiles
        .iter()
        .enumerate()
        .filter(|(_, tile)| (162_u8..=169).contains(*tile))
        .find_map(|(index, _)| {
            let x = ((index % MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5).round() as u16;
            let y = ((index / MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5).round() as u16;
            let hull_blocked = |seed| {
                reported_position_hull_overlaps_blocked_tile(
                    x,
                    y,
                    radius,
                    MAP_TILES,
                    TILE_SIZE_PX,
                    |candidate_x, candidate_y| {
                        map.tiles
                            .get(candidate_y * MAP_TILES + candidate_x)
                            .is_none_or(|&tile| {
                                crate::zone::position_blocking_tile_with_door_seed(tile, seed)
                            })
                    },
                )
            };
            let mut first_open = [None; 256];
            let mut last_close_index = None;
            for (phase_index, phase) in phases.iter().copied().enumerate() {
                let blocked = hull_blocked(phase.seed);
                if phase_index > 0 && !hull_blocked(phases[phase_index - 1].seed) && blocked {
                    last_close_index = Some(phase_index);
                }
                if blocked {
                    continue;
                }
                if let Some((predecessor_index, predecessor)) = first_open[phase.seed as usize]
                    && last_close_index.is_some_and(|close| close > predecessor_index)
                {
                    return Some((x, y, predecessor, phase));
                }
                first_open[phase.seed as usize].get_or_insert((phase_index, phase));
            }
            None
        })
        .expect("real map door sees an open repeated seed after an intervening close");
    assert_eq!(predecessor_phase.seed, receipt_phase.seed);
    assert!(
        crate::continuum::continuum_tick_diff(
            receipt_phase.started_tick,
            predecessor_phase.started_tick,
        ) > 0
    );

    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, ship);
    let previous_sequence = u16::MAX;
    let relocation_sequence = 0_u16;
    let next_sequence = 1_u16;
    let coast = crate::state::ClientInputObservation {
        bits: 0,
        direction: 0,
    };
    let identity = {
        let mut locked = state.lock().expect("state");
        let identity = locked.active_life_identity(player_id).expect("life");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.life_position_sequence_base = None;
        client.door_state = Some(door_state);
        client.alive = true;
        client.respawn_at_tick = None;
        let baseline_state = crate::state::ServerSimMovement {
            position_x: door_x as f32,
            position_y: door_y as f32,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let mut baseline =
            client_position_packet_with_sequence(0, door_x, door_y, previous_sequence);
        baseline[16..18].copy_from_slice(&client.server_bounty.to_le_bytes());
        baseline[18..20].copy_from_slice(&client.server_energy.to_le_bytes());
        set_client_position_canonical_state(
            &mut baseline,
            door_x as f32,
            door_y as f32,
            0.0,
            0.0,
            0.0,
        );
        client.last_position =
            Some(large_position_from_client(player_id, &baseline).expect("baseline"));
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(predecessor_phase.started_tick);
        client.server_sim_state = Some(baseline_state);
        client.server_sim_anchor_tick = Some(predecessor_phase.started_tick);
        client.server_sim_input_bits = Some(0);
        let witness = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: baseline_state,
            wire_x: door_x,
            wire_y: door_y,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 0,
            door_seed: predecessor_phase.seed,
            door_phase_tick: predecessor_phase.started_tick,
        };
        client.canonical_movement_witness = Some(witness);
        seed_fixture_motion_baseline(client);
        client.canonical_position_witnesses.push_back(witness);
        for sequence in [previous_sequence, relocation_sequence] {
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence,
                    input: coast,
                    // The compact input lane has resolved this repeated byte
                    // to its earlier authenticated occurrence.
                    door_seed: predecessor_phase.seed,
                    door_phase_tick: predecessor_phase.started_tick,
                });
        }
        client.last_input_seq = Some(relocation_sequence);
        client.last_input_record = Some(coast);
        identity
    };
    let (spawn_x, spawn_y, bounty, energy) = {
        let locked = state.lock().expect("state");
        let (spawn_x, spawn_y) = server_spawn_coordinates(locked.zone_map());
        let client = &locked.clients[&player_id];
        (spawn_x, spawn_y, client.server_bounty, client.server_energy)
    };
    let mut relocation =
        client_position_packet_with_sequence(0, spawn_x, spawn_y, relocation_sequence);
    relocation[16..18].copy_from_slice(&bounty.to_le_bytes());
    relocation[18..20].copy_from_slice(&energy.to_le_bytes());
    set_client_position_canonical_state(
        &mut relocation,
        spawn_x as f32,
        spawn_y as f32,
        0.0,
        0.0,
        0.0,
    );
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &relocation,
            receipt_phase.started_tick,
        )
        .is_some(),
        "the exact receipt-bounded close relocation must admit"
    );
    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        let reanchored = client
            .recent_input_records
            .iter()
            .find(|record| record.sequence == relocation_sequence)
            .expect("relocation input retained");
        assert_eq!(reanchored.door_seed, receipt_phase.seed);
        assert_eq!(reanchored.door_phase_tick, receipt_phase.started_tick);
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: next_sequence,
                input: coast,
                door_seed: receipt_phase.seed,
                door_phase_tick: receipt_phase.started_tick,
            });
        client.last_input_seq = Some(next_sequence);
        client.last_input_record = Some(coast);
    }
    let next_tick = crate::continuum::continuum_tick_add(receipt_phase.started_tick, 1);
    let next_tail = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        crate::sim_observation::replay_canonical_position_gap(
            client,
            locked.zone_map(),
            false,
            false,
            false,
            crate::sim_observation::PlayerMovementUpgrades {
                thrust: client.server_thrust_acceleration,
                top_speed: client.server_top_speed,
                rotation: client.server_rotation_per_second,
                recharge: client.server_recharge_per_second,
                live_values: true,
            },
            &[],
            next_sequence,
            next_tick,
            spawn_x,
            spawn_y,
        )
        .and_then(|replay| replay.witnesses.last().copied())
        .expect("ordinary post-relocation tail replays")
    };
    let mut next =
        client_position_packet_with_sequence(0, next_tail.wire_x, next_tail.wire_y, next_sequence);
    next[1] = next_tail.direction;
    next[6..8].copy_from_slice(&next_tail.wire_velocity_x.to_le_bytes());
    next[14..16].copy_from_slice(&next_tail.wire_velocity_y.to_le_bytes());
    next[16..18].copy_from_slice(&bounty.to_le_bytes());
    next[18..20].copy_from_slice(&energy.to_le_bytes());
    set_client_position_canonical_state(
        &mut next,
        next_tail.state.position_x,
        next_tail.state.position_y,
        next_tail.state.velocity_x,
        next_tail.state.velocity_y,
        next_tail.state.orientation,
    );
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(&state, identity, &next, next_tick)
            .is_some(),
        "the next ordinary tail must stay on the reanchored phase"
    );
}

#[test]
fn server_rejects_position_hulls_that_overlap_solid_tiles() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let (near_wall_x, near_wall_y) = first_clear_position_touching_solid_tile();
    assert!(!solid_tile(tile_at_world(
        zone_map(),
        near_wall_x,
        near_wall_y
    )));
    assert_eq!(arena_ship_radius_pixels(0), 14.0);
    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, near_wall_x, near_wall_y),
    );
    assert!(rx_b.try_recv().is_err());
    {
        let state = state.lock().expect("server state should not be poisoned");
        assert!(
            state
                .clients
                .get(&alpha)
                .and_then(|client| client.last_position_server_tick)
                .is_none()
        );
    }

    let spawn = server_spawn_coordinates(zone_map());
    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_3345, spawn.0, spawn.1),
    );
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(relayed[0], 0x05);
    assert_eq!(
        u16::from_le_bytes(relayed[4..6].try_into().unwrap()),
        spawn.0
    );
}

#[test]
fn server_overwrites_forged_position_bounty_with_server_owned_bounty() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let mut forged = client_position_packet(0x1122_3344, 500, 700);
    forged[16..18].copy_from_slice(&999_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &forged);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed[17..19].try_into().unwrap()),
        crate::arena_settings::arena_ship_initial_bounty(0)
    );
    reset_position_rate_limit(&state, alpha);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_bounty = 7;
    }
    let mut forged_after_server_bounty = client_position_packet(0x1122_3345, 501, 701);
    forged_after_server_bounty[16..18].copy_from_slice(&999_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &forged_after_server_bounty);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[17..19].try_into().unwrap()), 7);
}

#[test]
fn server_overwrites_forged_position_energy_with_server_owned_energy() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let mut forged = client_position_packet(0x1122_3344, 500, 700);
    forged[18..20].copy_from_slice(&900_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &forged);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        1_700
    );
    reset_position_rate_limit(&state, alpha);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_energy = 840;
    }
    let mut forged_after_server_damage = client_position_packet(0x1122_3345, 501, 701);
    forged_after_server_damage[18..20].copy_from_slice(&999_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &forged_after_server_damage);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[21..23].try_into().unwrap()), 840);
}

#[test]
fn server_strips_position_weapon_bits_that_exceed_fire_cadence() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&alpha)
        .unwrap()
        .apply_server_prize_effect(10);
    reset_server_runtime_without_initial_bounty(&state, beta);

    let mut first_weapon = client_position_packet(0x1122_3344, 500, 700);
    first_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &first_weapon);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 2);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        1680
    );
    {
        let state = state.lock().expect("server state should not be poisoned");
        assert_eq!(state.active_projectiles.len(), 1);
        let projectile = state
            .active_projectiles
            .last()
            .expect("accepted weapon should record a server projectile");
        assert_eq!(projectile.owner_id, alpha);
        assert_eq!(projectile.weapon_data, 2);
        assert_eq!(projectile.x, 500);
        assert_eq!(projectile.y, 700);
        assert_ne!(projectile.timestamp, 0x1122_3344);
        assert_eq!(
            projectile.timestamp as u16,
            u16::from_le_bytes(relayed[2..4].try_into().unwrap()),
            "projectile spawn tick and relayed position tick must come from the same server clock sample"
        );
    }
    reset_position_rate_limit(&state, alpha);

    let mut spam_weapon = client_position_packet(0x1122_3345, 501, 701);
    spam_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &spam_weapon);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        1680
    );
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .active_projectiles
            .len(),
        1
    );
    reset_position_rate_limit(&state, alpha);
    rewind_last_gun_fire_tick(&state, alpha, arena_ship_weapon_fire_delay_ticks(0, 2));
    rewind_last_position_server_tick(&state, alpha, arena_ship_weapon_fire_delay_ticks(0, 2));
    advance_recharge_by_elapsed_ticks(&state, alpha, arena_ship_weapon_fire_delay_ticks(0, 2));

    let mut cooled_down_weapon = client_position_packet(0x1122_335c, 502, 702);
    cooled_down_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &cooled_down_weapon);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 2);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        1669
    );
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .active_projectiles
            .len(),
        2
    );
}

#[test]
fn server_strips_position_weapon_bits_that_repeat_before_server_cooldown() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&alpha)
        .unwrap()
        .apply_server_prize_effect(10);
    reset_server_runtime_without_initial_bounty(&state, beta);

    let mut first_weapon = client_position_packet(0x1122_3344, 500, 700);
    first_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &first_weapon);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 2);
    reset_position_rate_limit(&state, alpha);

    // This packet arrives immediately after the previous accepted bullet.
    // The authoritative server must use its own cooldown tick, not any
    // client-authored compatibility timestamp, to prevent duplicate spawns.
    let mut repeated_weapon = client_position_packet(0, 501, 701);
    repeated_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &repeated_weapon);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .active_projectiles
            .len(),
        1
    );
}

#[test]
fn server_rejects_position_without_zubspace_sequence() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    reset_server_runtime_without_initial_bounty(&state, beta);

    let mut missing_sequence = client_position_packet(0, 500, 700);
    missing_sequence.truncate(22);
    missing_sequence[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &missing_sequence);

    assert!(rx_b.try_recv().is_err());
    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert_eq!(client.last_position_sequence, None);
    assert_eq!(state.active_projectiles.len(), 0);
}

#[test]
fn server_rejects_replayed_position_sequence_for_movement_and_weapons() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&alpha)
        .unwrap()
        .apply_server_prize_effect(10);
    reset_server_runtime_without_initial_bounty(&state, beta);

    let mut first_weapon = client_position_packet_with_sequence(0, 500, 700, 7);
    first_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &first_weapon);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 2);
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .active_projectiles
            .len(),
        1
    );

    reset_position_rate_limit(&state, alpha);
    rewind_last_gun_fire_tick(&state, alpha, arena_ship_weapon_fire_delay_ticks(0, 2));
    let mut replayed_weapon = client_position_packet_with_sequence(0xffff_ffff, 501, 701, 7);
    replayed_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &replayed_weapon);
    assert!(rx_b.try_recv().is_err());
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .active_projectiles
            .len(),
        1,
        "same logical sequence must not spawn a second projectile"
    );

    reset_position_rate_limit(&state, alpha);
    rewind_last_gun_fire_tick(&state, alpha, arena_ship_weapon_fire_delay_ticks(0, 2));
    let mut next_weapon = client_position_packet_with_sequence(1, 502, 702, 8);
    next_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &next_weapon);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 2);
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .active_projectiles
            .len(),
        2
    );

    reset_position_rate_limit(&state, alpha);
    let missing_input_rejects_before =
        crate::security::peek_security_reject_count_for_test("position", "missing_input_record");
    let older_movement = client_position_packet_with_sequence(2, 503, 703, 6);
    assert!(
        accept_position_relay(&state, alpha, &older_movement).is_none(),
        "a reordered movement-only datagram remains inert even when its independently omitted input record is unavailable",
    );
    assert!(rx_b.try_recv().is_err());
    assert_eq!(
        crate::security::peek_security_reject_count_for_test("position", "missing_input_record"),
        missing_input_rejects_before,
        "ordinary unordered movement omission is transport behavior, not a security rejection",
    );
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&alpha)
            .and_then(|client| client.last_position_sequence),
        Some(8)
    );
}

#[test]
fn server_accepts_unseen_stale_weapon_sequence_after_later_movement() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&alpha)
        .unwrap()
        .apply_server_prize_effect(10);
    reset_server_runtime_without_initial_bounty(&state, beta);

    let baseline = client_position_packet_with_sequence(0, 8200, 8200, 5);
    broadcast_position_observable(&state, alpha, &baseline);
    let _ = read_non_reliable_from_channel(&rx_b);
    reset_position_rate_limit(&state, alpha);

    // Advance the datagram sequence while retaining a replay-exact stationary
    // tail. That proves the omitted sequence-7 origin; an arbitrary moved
    // tail may not manufacture stale-weapon provenance.
    let later_movement = client_position_packet_with_sequence(1, 8200, 8200, 8);
    broadcast_position_observable(&state, alpha, &later_movement);
    let _ = read_non_reliable_from_channel(&rx_b);
    reset_position_rate_limit(&state, alpha);

    let mut delayed_reliable_weapon = client_position_packet_with_sequence(2, 8200, 8200, 7);
    delayed_reliable_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &delayed_reliable_weapon);

    assert!(
        rx_b.try_recv().is_err(),
        "stale weapon mirror must not rewind movement"
    );
    let state = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        state
            .clients
            .get(&alpha)
            .and_then(|client| client.last_position_sequence),
        Some(8),
        "movement sequence should stay on the later movement-only frame"
    );
    assert_eq!(state.active_projectiles.len(), 1);
    let projectile = state
        .active_projectiles
        .last()
        .expect("stale reliable weapon mirror should spawn one projectile");
    assert_eq!(projectile.owner_id, alpha);
    assert_eq!(projectile.weapon_data, 2);
    assert_eq!(
        (projectile.x, projectile.y),
        (8200, 8200),
        "stale weapon mirror should preserve the original firing position without rewinding movement"
    );
}

#[test]
fn stale_weapon_mirror_with_impossible_velocity_is_rejected() {
    // [REVIEW H1] The mirror path has no previous frame to delta the
    // velocity against, so it must bound the claimed firing velocity
    // absolutely — otherwise a forged "stale mirror" spawns a projectile
    // with arbitrary inherited speed straight off the wire.
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    reset_server_runtime_without_initial_bounty(&state, beta);

    let baseline = client_position_packet_with_sequence(0, 8200, 8200, 5);
    broadcast_position_observable(&state, alpha, &baseline);
    let _ = read_non_reliable_from_channel(&rx_b);
    reset_position_rate_limit(&state, alpha);

    let later_movement = client_position_packet_with_sequence(1, 8216, 8200, 8);
    broadcast_position_observable(&state, alpha, &later_movement);
    let _ = read_non_reliable_from_channel(&rx_b);
    reset_position_rate_limit(&state, alpha);

    // Same shape as the legitimate delayed mirror, but claiming a firing
    // velocity of 3000 px/s — far past any ship's speed envelope.
    let mut forged_mirror = client_position_packet_with_sequence(2, 8200, 8200, 7);
    forged_mirror[20..22].copy_from_slice(&2_u16.to_le_bytes());
    // Client position packet velocity_x lives at [6..8] (tenths of px/s).
    forged_mirror[6..8].copy_from_slice(&30_000_i16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &forged_mirror);

    let state = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        state.active_projectiles.len(),
        0,
        "an impossible-velocity mirror must not spawn a projectile"
    );
}

#[test]
fn stale_weapon_requires_exact_canonical_origin_vector_life_and_is_atomic() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&alpha)
        .unwrap()
        .apply_server_prize_effect(10);
    reset_server_runtime_without_initial_bounty(&state, beta);

    let baseline = client_position_packet_with_sequence(0, 8_200, 8_200, 5);
    broadcast_position_observable(&state, alpha, &baseline);
    let _ = read_non_reliable_from_channel(&rx_b);
    reset_position_rate_limit(&state, alpha);
    let later = client_position_packet_with_sequence(0, 8_200, 8_200, 8);
    broadcast_position_observable(&state, alpha, &later);
    let _ = read_non_reliable_from_channel(&rx_b);
    reset_position_rate_limit(&state, alpha);

    let weapon_mirror = || {
        let mut packet = client_position_packet_with_sequence(0, 8_200, 8_200, 7);
        packet[20..22].copy_from_slice(&2_u16.to_le_bytes());
        packet
    };
    let solid_center = {
        let locked = state.lock().expect("state");
        let tile_index = locked
            .zone_map()
            .tiles
            .iter()
            .position(|&tile| position_blocking_tile(tile))
            .expect("test map has solid geometry");
        tile_center_position(tile_index % MAP_TILES, tile_index / MAP_TILES)
    };

    let mut forged = Vec::new();
    let mut displaced_511 = weapon_mirror();
    displaced_511[12..14].copy_from_slice(&8_711_u16.to_le_bytes());
    forged.push(("511px-current-position grace", displaced_511));
    let mut across_wall = weapon_mirror();
    across_wall[12..14].copy_from_slice(&solid_center.0.to_le_bytes());
    across_wall[8..10].copy_from_slice(&solid_center.1.to_le_bytes());
    forged.push(("across-wall/solid-hull origin", across_wall));
    let mut wrong_velocity = weapon_mirror();
    wrong_velocity[6..8].copy_from_slice(&1_i16.to_le_bytes());
    forged.push(("wrong velocity", wrong_velocity));
    let mut wrong_direction = weapon_mirror();
    wrong_direction[1] = 1;
    forged.push(("wrong direction", wrong_direction));

    for (case, packet) in forged {
        let before = witness_admission_state(&state, alpha);
        broadcast_position_observable(&state, alpha, &packet);
        let after = witness_admission_state(&state, alpha);
        assert_eq!(after, before, "{case} must be mutation-free");
    }

    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&alpha).expect("client");
        let current_life = client.life_seq;
        client
            .canonical_position_witnesses
            .iter_mut()
            .find(|witness| witness.sequence == 7)
            .expect("reconstructed stale sequence")
            .life_seq = current_life.wrapping_sub(1);
    }
    let before_wrong_life = witness_admission_state(&state, alpha);
    broadcast_position_observable(&state, alpha, &weapon_mirror());
    assert_eq!(
        witness_admission_state(&state, alpha),
        before_wrong_life,
        "wrong-life witness must not mutate weapon, energy, cooldown, or movement authority"
    );
    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&alpha).expect("client");
        let current_life = client.life_seq;
        client
            .canonical_position_witnesses
            .iter_mut()
            .find(|witness| witness.sequence == 7)
            .expect("reconstructed stale sequence")
            .life_seq = current_life;
    }

    // Capacity pressure must defer the exact redundant weapon mirror before
    // energy, cooldown, sequence, projectile, or public position mutation.
    {
        let mut locked = state.lock().expect("state");
        let recipient = locked.active_player_runtime_identity(beta).unwrap();
        let template = crate::snapshot::PendingRealtimeEventEnvelope {
            flight_transition: false,
            recipient,
            event_id: 77,
            server_tick: 77,
            payload: crate::realtime_event::RealtimeEventPayload::Weapon(
                crate::realtime_event::RealtimeWeaponEvent {
                    source_player_id: beta,
                    source_session_id: recipient.session.session_id.get(),
                    source_life_seq: recipient.life.life_seq,
                    shot_group_id: 1,
                    x: 8_200.0,
                    y: 8_200.0,
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    orientation: 0.0,
                    frequency: 2,
                    ship: 1,
                    weapon_data: 2,
                },
            ),
        };
        locked
            .clients
            .get_mut(&beta)
            .unwrap()
            .pending_realtime_events
            .queue
            .extend(std::iter::repeat_n(
                template,
                crate::snapshot::DEDICATED_REALTIME_EVENT_QUEUE_CAPACITY,
            ));
    }
    let before_pressure = witness_admission_state(&state, alpha);
    broadcast_position_observable(&state, alpha, &weapon_mirror());
    assert_eq!(
        witness_admission_state(&state, alpha),
        before_pressure,
        "a full dedicated queue must leave the authoritative weapon transaction untouched"
    );
    {
        let mut locked = state.lock().expect("state");
        assert_eq!(locked.active_projectiles.len(), 0);
        assert_eq!(locked.realtime_event_queue_overflows, 1);
        assert_eq!(locked.realtime_event_actions_deferred, 1);
        assert_eq!(
            locked.clients[&beta]
                .pending_realtime_events
                .queue
                .front()
                .unwrap()
                .event_id,
            77,
            "pressure never evicts an already-authoritative fact"
        );
        locked
            .clients
            .get_mut(&beta)
            .unwrap()
            .pending_realtime_events
            .queue
            .clear();
    }

    broadcast_position_observable(&state, alpha, &weapon_mirror());
    assert_eq!(
        state.lock().expect("state").active_projectiles.len(),
        1,
        "the exact genuinely reordered shot remains accepted"
    );
    let after_first = witness_admission_state(&state, alpha);
    broadcast_position_observable(&state, alpha, &weapon_mirror());
    assert_eq!(
        witness_admission_state(&state, alpha),
        after_first,
        "exact replay must not duplicate projectile or mutate fire state"
    );
    let locked = state.lock().expect("state");
    let queued = &locked.clients[&beta].pending_realtime_events.queue;
    assert_eq!(
        queued.len(),
        1,
        "accepted/replayed shot queues exactly once"
    );
    let crate::realtime_event::RealtimeEventPayload::Weapon(event) = queued[0].payload else {
        panic!("accepted weapon must use the dedicated event carrier");
    };
    assert_eq!(event.source_player_id, alpha);
    assert_eq!(event.source_life_seq, locked.clients[&alpha].life_seq);
    assert_eq!(
        event.source_session_id,
        locked.clients[&alpha].session_id.get()
    );
    assert_eq!((event.x, event.y), (8_200.0, 8_200.0));
    assert_eq!((event.velocity_x, event.velocity_y), (0.0, 0.0));
    assert_eq!(event.orientation, 0.0);
    assert_eq!(event.frequency, locked.clients[&alpha].player.frequency);
    assert_eq!(event.ship, locked.clients[&alpha].player.ship);
    assert_eq!(event.weapon_data, 2);
}

#[test]
fn stale_weapon_canonical_history_is_wrap_safe() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&alpha)
        .unwrap()
        .apply_server_prize_effect(10);

    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet_with_sequence(0, 8_200, 8_200, u16::MAX - 2),
    );
    let _ = read_non_reliable_from_channel(&rx_b);
    reset_position_rate_limit(&state, alpha);
    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet_with_sequence(0, 8_200, 8_200, 0),
    );
    let _ = read_non_reliable_from_channel(&rx_b);
    reset_position_rate_limit(&state, alpha);

    let mut delayed = client_position_packet_with_sequence(0, 8_200, 8_200, u16::MAX);
    delayed[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &delayed);
    let locked = state.lock().expect("state");
    assert_eq!(locked.active_projectiles.len(), 1);
    assert_eq!(
        locked.active_projectiles.last().map(|p| (p.x, p.y)),
        Some((8_200, 8_200))
    );
}

#[test]
fn server_allows_bomb_after_recent_gun_delay_instead_of_full_bomb_delay() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let gun_delay = arena_ship_weapon_fire_delay_ticks(0, 2);
    let bomb_delay = arena_ship_weapon_fire_delay_ticks(0, 3);
    assert!(bomb_delay > gun_delay);

    let mut bullet = client_position_packet(0x1122_3344, 500, 700);
    bullet[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &bullet);
    let relayed_bullet = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed_bullet[19..21].try_into().unwrap()),
        2
    );

    reset_position_rate_limit(&state, alpha);
    rewind_last_gun_fire_tick(&state, alpha, gun_delay);
    let mut bomb = client_position_packet(0x1122_3344 + gun_delay, 501, 701);
    bomb[20..22].copy_from_slice(&3_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &bomb);
    let relayed_bomb = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed_bomb[19..21].try_into().unwrap()),
        3
    );
}

#[test]
fn server_strips_position_weapon_bits_without_enough_server_energy() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_energy = 20;
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .last_energy_recharge_tick = Some(crate::wire::current_tick());
    }

    let mut equal_energy_weapon = client_position_packet(0x1122_3344, 500, 700);
    equal_energy_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &equal_energy_weapon);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert_eq!(u16::from_le_bytes(relayed[21..23].try_into().unwrap()), 20);

    reset_position_rate_limit(&state, alpha);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_energy_remainder = 0.25;
        client.last_energy_recharge_tick = Some(crate::wire::current_tick());
    }
    let mut fractional_energy_weapon = client_position_packet(0x1122_3345, 501, 701);
    fractional_energy_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &fractional_energy_weapon);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 2);
    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert_eq!(client.server_energy, 0);
    assert!((client.server_energy_remainder - 0.25).abs() < 0.001);
}

#[test]
fn server_super_fire_requires_normal_cost_but_charges_zero() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_energy = 20;
        client.server_energy_remainder = 0.0;
        client.server_super_until_tick = Some(crate::continuum::continuum_tick_add(
            crate::wire::current_tick(),
            1_000,
        ));
        client.last_energy_recharge_tick = Some(crate::wire::current_tick());
    }

    let mut fire = client_position_packet(0x1122_3344, 500, 700);
    fire[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &fire);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert_eq!(u16::from_le_bytes(relayed[21..23].try_into().unwrap()), 20);

    reset_position_rate_limit(&state, alpha);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_energy_remainder = 0.25;
        client.last_energy_recharge_tick = Some(crate::wire::current_tick());
    }
    let mut fractional_fire = client_position_packet(0x1122_3345, 501, 701);
    fractional_fire[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &fractional_fire);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 2);
    assert_eq!(u16::from_le_bytes(relayed[21..23].try_into().unwrap()), 20);
    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert_eq!(client.server_energy, 20);
    assert!((client.server_energy_remainder - 0.25).abs() < 0.001);
}

#[test]
fn server_rate_limits_rejected_weapon_bearing_positions_like_movement() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_energy = 20;
        client.last_energy_recharge_tick = Some(crate::wire::current_tick());
        client.last_position_relay_at = Some(Instant::now());
        client.position_relay_tokens = -100.0;
    }

    let mut rejected_weapon = client_position_packet(0x1122_3344, 500, 700);
    rejected_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &rejected_weapon);
    assert!(rx_b.try_recv().is_err());

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert_eq!(client.server_energy, 20);
    assert!(client.last_position_server_tick.is_some());
    assert!(client.last_weapon_fire_tick.is_none());
    let cached = client
        .last_position
        .as_ref()
        .expect("rate-limited movement should still update authoritative position");
    assert_eq!(u16::from_le_bytes(cached[19..21].try_into().unwrap()), 0);
    assert_eq!(u16::from_le_bytes(cached[21..23].try_into().unwrap()), 20);
}

#[test]
fn server_keeps_authoritative_state_when_movement_fanout_is_rate_limited() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    drain_available(&rx_b);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_energy = 1_300;
        client.server_energy_remainder = 0.0;
        client.server_recharge_per_second =
            crate::arena_settings::arena_ship_initial_recharge_per_second(client.player.ship);
        let anchor_tick = crate::continuum::continuum_tick_sub(crate::wire::current_tick(), 100);
        client.last_position_server_tick = Some(anchor_tick);
        client.last_energy_recharge_tick = Some(anchor_tick);
        client.last_position_relay_at = Some(Instant::now());
        client.position_relay_tokens = 0.0;
    }

    advance_recharge_by_elapsed_ticks(&state, alpha, 100);
    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_33a8, 501, 701),
    );
    assert!(rx_b.try_recv().is_err());

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert!(
        client.server_energy >= 1_319,
        "rate-limited movement should still recharge server energy, got {}",
        client.server_energy
    );
    let cached = client
        .last_position
        .as_ref()
        .expect("rate-limited movement should still update cached position");
    assert_eq!(u16::from_le_bytes(cached[4..6].try_into().unwrap()), 501);
    assert_eq!(u16::from_le_bytes(cached[15..17].try_into().unwrap()), 701);
}

#[test]
fn server_strips_position_weapon_bits_from_safe_tiles() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    reset_server_runtime_without_initial_bounty(&state, beta);

    let (safe_x, safe_y) = first_safe_zone_position();
    let mut safe_weapon = client_position_packet(0x1122_3344, safe_x, safe_y);
    safe_weapon[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &safe_weapon);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        1_700
    );
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert!(client.last_weapon_fire_tick.is_none());
        assert_eq!(client.server_energy, 1_700);
    }
}

#[test]
fn lost_intermediate_safe_zone_weapon_stop_replays_into_later_thrust_tail() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    state.lock().expect("state").config.movement_snapback = true;
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let (safe_x, safe_y) = first_safe_zone_position();
    let anchor_tick = 6_708;
    let accepted_tick = 6_710;
    let anchor_sequence = 29_501;
    let stop_sequence = anchor_sequence + 1;
    let current_sequence = anchor_sequence + 2;
    let orientation = 18.0 / 40.0;
    let velocity_x = 60.0;

    let identity = {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.reset_server_runtime_for_ship_at_tick(anchor_tick);
        grant_legacy_capture_loadout(client, anchor_tick);
        client.life_position_sequence_base = None;
        client.alive = true;
        client.respawn_at_tick = None;

        let mut prior = client_position_packet_with_sequence(0, safe_x, safe_y, anchor_sequence);
        prior[1] = crate::continuum_position::protocol_direction(orientation);
        prior[6..8].copy_from_slice(
            &crate::continuum_position::protocol_velocity_component(velocity_x).to_le_bytes(),
        );
        prior[16..18].copy_from_slice(&client.server_bounty.to_le_bytes());
        prior[18..20].copy_from_slice(&client.server_energy.to_le_bytes());
        set_client_position_canonical_state(
            &mut prior,
            safe_x as f32,
            safe_y as f32,
            velocity_x,
            0.0,
            orientation,
        );
        client.last_position =
            Some(large_position_from_client(player_id, &prior).expect("prior position"));
        client.last_position_sequence = Some(anchor_sequence);
        client.last_position_server_tick = Some(anchor_tick);
        let anchor = crate::state::CanonicalPositionWitness {
            sequence: anchor_sequence,
            life_seq: client.life_seq,
            state: crate::state::ServerSimMovement {
                position_x: safe_x as f32,
                position_y: safe_y as f32,
                orientation,
                velocity_x,
                velocity_y: 0.0,
                energy: client.server_energy as f32,
                max_energy: client.server_max_energy as f32,
            },
            wire_x: safe_x,
            wire_y: safe_y,
            wire_velocity_x: crate::continuum_position::protocol_velocity_component(velocity_x),
            wire_velocity_y: 0,
            direction: crate::continuum_position::protocol_direction(orientation),
            door_seed: 0,
            door_phase_tick: 0,
        };
        client.canonical_position_witnesses.clear();
        client.canonical_position_witnesses.push_back(anchor);
        client.canonical_movement_witness = Some(anchor);
        seed_fixture_motion_baseline(client);
        client.server_sim_state = Some(anchor.state);
        client.server_sim_anchor_tick = Some(anchor_tick);
        client.server_sim_input_bits = Some(1);
        client.recent_input_records.clear();
        let stopped_input = crate::state::ClientInputObservation {
            bits: crate::gameplay_rules::SAFE_ZONE_WEAPON_STOP_BIT | 1,
            direction: anchor.direction,
        };
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: stop_sequence,
                input: stopped_input,
                door_seed: 0,
                door_phase_tick: 0,
            });
        let tail_input = crate::state::ClientInputObservation {
            bits: 1,
            direction: anchor.direction,
        };
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: current_sequence,
                input: tail_input,
                door_seed: 0,
                door_phase_tick: 0,
            });
        client.last_input_seq = Some(current_sequence);
        client.last_input_record = Some(tail_input);
        locked.active_life_identity(player_id).expect("life")
    };

    let (stopped_tail, bounty, energy) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        let replay = crate::sim_observation::replay_canonical_position_gap(
            client,
            locked.zone_map(),
            false,
            false,
            false,
            crate::sim_observation::PlayerMovementUpgrades {
                thrust: client.server_thrust_acceleration,
                top_speed: client.server_top_speed,
                rotation: client.server_rotation_per_second,
                recharge: client.server_recharge_per_second,
                live_values: true,
            },
            &[],
            current_sequence,
            accepted_tick,
            server_spawn_coordinates(locked.zone_map()).0,
            server_spawn_coordinates(locked.zone_map()).1,
        )
        .expect("safe movement tail replays");
        assert!(crate::zone::world_position_in_safe_tile_for_map(
            locked.zone_map(),
            replay.witnesses.last().expect("tail").wire_x,
            replay.witnesses.last().expect("tail").wire_y,
        ));
        (
            replay.witnesses.last().copied().expect("stopped tail"),
            client.server_bounty,
            client.server_energy,
        )
    };
    assert_ne!(stopped_tail.state.velocity_x.to_bits(), 0.0_f32.to_bits());
    let mut stopped = client_position_packet_with_sequence(
        0,
        stopped_tail.wire_x,
        stopped_tail.wire_y,
        current_sequence,
    );
    stopped[1] = stopped_tail.direction;
    stopped[6..8].copy_from_slice(&stopped_tail.wire_velocity_x.to_le_bytes());
    stopped[14..16].copy_from_slice(&stopped_tail.wire_velocity_y.to_le_bytes());
    stopped[16..18].copy_from_slice(&bounty.to_le_bytes());
    stopped[18..20].copy_from_slice(&energy.to_le_bytes());
    set_client_position_canonical_state(
        &mut stopped,
        stopped_tail.state.position_x,
        stopped_tail.state.position_y,
        stopped_tail.state.velocity_x,
        stopped_tail.state.velocity_y,
        stopped_tail.state.orientation,
    );

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &stopped,
            accepted_tick,
        )
        .is_some(),
        "a lost intermediate safe-zone stop must replay into the later authenticated thrust tail",
    );
    let locked = state.lock().expect("state");
    let committed = locked.clients[&player_id]
        .canonical_movement_witness
        .expect("stopped witness");
    assert_eq!(committed.sequence, current_sequence);
    assert_eq!(
        committed.state.velocity_x.to_bits(),
        stopped_tail.state.velocity_x.to_bits()
    );
    assert_eq!(
        committed.state.velocity_y.to_bits(),
        stopped_tail.state.velocity_y.to_bits()
    );
    let private_tail = locked.clients[&player_id]
        .server_sim_state
        .expect("accepted movement retains a private observer tail");
    // The existing observer blends toward the quantized wire velocity, so
    // bitwise equality is not its contract. Even allowing a whole wire unit
    // (0.1 px/s), it must not carry the pre-stop 60 px/s into the next step.
    assert!(
        (private_tail.velocity_x - stopped_tail.state.velocity_x).abs() <= 0.1
            && (private_tail.velocity_y - stopped_tail.state.velocity_y).abs() <= 0.1,
        "safe-stop replay must agree within one wire velocity unit: private={private_tail:?}, canonical={:?}",
        stopped_tail.state,
    );
    drop(locked);
    // Repeated Ctrl+Forward requests in safety must not accumulate fictitious
    // drift and open a correction after initially accepting honest movement.
    for cycle in 1..=122_u16 {
        let sequence = current_sequence.wrapping_add(cycle * 2);
        let tick = accepted_tick + (u32::from(cycle) * 10 / 3);
        let (expected, energy, bounty) = {
            let mut locked = state.lock().expect("state");
            let observer_fault_position = (cycle == 121).then(|| {
                let index = locked
                    .zone_map()
                    .tiles
                    .iter()
                    .enumerate()
                    .find_map(|(index, tile)| {
                        let x = index % MAP_TILES;
                        let y = index / MAP_TILES;
                        (*tile == 0
                            && (4..MAP_TILES - 4).contains(&x)
                            && (4..MAP_TILES - 4).contains(&y))
                        .then_some(index)
                    })
                    .expect("authored map has an ordinary non-safe tile");
                (
                    (index % MAP_TILES) as f32 * 16.0 + 8.0,
                    (index / MAP_TILES) as f32 * 16.0 + 8.0,
                )
            });
            let client = locked.clients.get_mut(&player_id).expect("client");
            if let Some((x, y)) = observer_fault_position {
                // Fault only the diagnostic prediction, never the canonical
                // witness. Its invalid safe-stop comparison must rebase and
                // allow the next ordinary comparison to resume.
                let private = client.server_sim_state.as_mut().expect("observer");
                private.position_x = x;
                private.position_y = y;
                private.velocity_x = 0.0;
                private.velocity_y = 0.0;
            }
            for (offset, bits) in [
                (1, crate::gameplay_rules::SAFE_ZONE_WEAPON_STOP_BIT | 1),
                (0, 1),
            ] {
                client
                    .recent_input_records
                    .push_back(crate::state::SequencedInputObservation {
                        sequence: sequence.wrapping_sub(offset),
                        input: crate::state::ClientInputObservation {
                            bits,
                            direction: stopped_tail.direction,
                        },
                        door_seed: 0,
                        door_phase_tick: 0,
                    });
            }
            while client.recent_input_records.len() > crate::state::MAX_RECENT_INPUT_OBSERVATIONS {
                client.recent_input_records.pop_front();
            }
            client.last_input_seq = Some(sequence);
            client.last_input_record = client
                .recent_input_records
                .back()
                .map(|record| record.input);
            let client = &locked.clients[&player_id];
            let (spawn_x, spawn_y) = server_spawn_coordinates(locked.zone_map());
            let replay = crate::sim_observation::replay_canonical_position_gap(
                client,
                locked.zone_map(),
                false,
                false,
                false,
                crate::sim_observation::PlayerMovementUpgrades {
                    thrust: client.server_thrust_acceleration,
                    top_speed: client.server_top_speed,
                    rotation: client.server_rotation_per_second,
                    recharge: client.server_recharge_per_second,
                    live_values: true,
                },
                &[],
                sequence,
                tick,
                spawn_x,
                spawn_y,
            )
            .expect("repeated safe-zone movement replays");
            (
                *replay.witnesses.last().expect("tail"),
                client.server_energy,
                client.server_bounty,
            )
        };
        let mut packet =
            client_position_packet_with_sequence(0, expected.wire_x, expected.wire_y, sequence);
        packet[1] = expected.direction;
        packet[6..8].copy_from_slice(&expected.wire_velocity_x.to_le_bytes());
        packet[14..16].copy_from_slice(&expected.wire_velocity_y.to_le_bytes());
        packet[16..18].copy_from_slice(&bounty.to_le_bytes());
        packet[18..20].copy_from_slice(&energy.to_le_bytes());
        set_client_position_canonical_state(
            &mut packet,
            expected.state.position_x,
            expected.state.position_y,
            expected.state.velocity_x,
            expected.state.velocity_y,
            expected.state.orientation,
        );
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(&state, identity, &packet, tick)
                .is_some()
        );
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert!(
            client.pending_movement_correction.is_none(),
            "false correction in stop/thrust cycle {cycle}"
        );
        if cycle == 121 {
            let private = client.server_sim_state.expect("rebased observer");
            assert_eq!(private.position_x, expected.wire_x as f32);
            assert_eq!(private.position_y, expected.wire_y as f32);
            assert_eq!(
                client.canonical_movement_witness.unwrap().state.position_x,
                expected.state.position_x
            );
        }
        assert!(
            client.movement_drift_x.hypot(client.movement_drift_y)
                < crate::sim_observation::MOVEMENT_DRIFT_SNAP_PX,
            "safe-stop observer accumulated false drift in cycle {cycle}"
        );
    }
}

#[test]
fn server_sanitizes_position_togglables_and_owns_safety_bit() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    reset_server_runtime_without_initial_bounty(&state, beta);

    let mut forged_outside_safe = client_position_packet(0x1122_3344, 500, 700);
    forged_outside_safe[11] = 0xff;
    broadcast_position_observable(&state, alpha, &forged_outside_safe);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(relayed[13] & 0xc0, 0);
    assert_eq!(relayed[13] & POSITION_STATUS_SAFETY, 0);
    assert_eq!(relayed[13] & POSITION_STATUS_STEALTH, 0);
    assert_eq!(relayed[13] & POSITION_STATUS_CLOAK, 0);
    assert_eq!(relayed[13] & POSITION_STATUS_XRADAR, 0);
    assert_eq!(relayed[13] & POSITION_STATUS_ANTIWARP, 0);
    assert_ne!(relayed[13] & POSITION_STATUS_FLASH, 0);
    reset_position_rate_limit(&state, alpha);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_status.stealth_capable = true;
        state.clear_client_position(alpha);
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.last_position_sequence = None;
        client.canonical_movement_witness = None;
        client.canonical_position_witnesses.clear();
    }

    let (safe_x, safe_y) = first_safe_zone_position();
    let mut forged_inside_safe = client_position_packet(0x1122_3345, safe_x, safe_y);
    forged_inside_safe[11] = POSITION_STATUS_STEALTH;
    broadcast_position_observable(&state, alpha, &forged_inside_safe);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(relayed[13] & 0xc0, 0);
    assert_ne!(relayed[13] & POSITION_STATUS_SAFETY, 0);
    assert_ne!(relayed[13] & POSITION_STATUS_STEALTH, 0);
}

#[test]
fn server_drains_prior_status_energy_before_accepting_the_next_status_state() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    let _ = read_non_reliable_from_channel(&rx_b);
    reset_position_rate_limit(&state, alpha);
    rewind_last_position_server_tick(&state, alpha, 10);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_energy = 1;
        client.server_energy_remainder = 0.0;
        client.server_recharge_per_second =
            crate::arena_settings::arena_ship_initial_recharge_per_second(client.player.ship);
        client.server_status.xradar_capable = true;
        client.server_status.stealth_capable = true;
        client
            .last_position
            .as_mut()
            .expect("prior position should exist")[13] =
            POSITION_STATUS_XRADAR | POSITION_STATUS_STEALTH;
    }
    advance_gameplay_energy_by_elapsed_ticks(&state, alpha, 10);
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        let prior_status = client.last_position.as_ref().expect("prior position")[13];
        assert_ne!(prior_status & POSITION_STATUS_XRADAR, 0);
        assert_eq!(prior_status & POSITION_STATUS_STEALTH, 0);
        assert_eq!(client.server_energy, 2);
        assert!((client.server_energy_remainder - 0.5).abs() < 0.01);
    }

    let mut status_position = client_position_packet(0x1122_334e, 501, 700);
    status_position[11] = POSITION_STATUS_XRADAR | POSITION_STATUS_STEALTH;
    broadcast_position_observable(&state, alpha, &status_position);

    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_ne!(relayed[13] & POSITION_STATUS_XRADAR, 0);
    assert_ne!(
        relayed[13] & POSITION_STATUS_STEALTH,
        0,
        "the packet selects the status state charged on the next batch"
    );
    assert_eq!(u16::from_le_bytes(relayed[21..23].try_into().unwrap()), 2);
    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert_eq!(client.server_energy, 2);
    assert!((client.server_energy_remainder - 0.5).abs() < 0.01);
}

#[test]
fn upgraded_top_speed_coasting_never_infers_hidden_afterburner_drain() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    reset_server_runtime_without_initial_bounty(&state, player_id);
    let tick = 10_000;
    let anchor = crate::continuum::continuum_tick_sub(tick, 100);

    {
        let mut state = state.lock().expect("server state");
        let client = state.clients.get_mut(&player_id).expect("client");
        client.server_energy = 200;
        client.server_energy_remainder = 0.0;
        client.server_recharge_per_second = 115.0;
        client.server_top_speed = 325.0;
        client.last_energy_recharge_tick = Some(anchor);
        client.last_continuous_energy_drain_tick = Some(anchor);
        client.server_sim_input_bits = Some(0);
        let mut position = client_position_packet(1, 8_200, 8_200);
        position[11] = 0;
        // 320 px/s is above the initial 201 px/s cap but below the legitimate
        // upgraded Chaos cap. The retired heuristic incorrectly treated this
        // velocity alone as a held Shift+thrust input.
        position[10..12].copy_from_slice(&3_200_i16.to_le_bytes());
        client.last_position = large_position_from_client(player_id, &position);

        crate::arena::advance_server_gameplay_energy(client, tick);
        assert_eq!(client.server_energy, 315);
        assert_eq!(client.server_energy_remainder, 0.0);
    }

    {
        let mut state = state.lock().expect("server state");
        let client = state.clients.get_mut(&player_id).expect("client");
        client.server_energy = 200;
        client.server_energy_remainder = 0.0;
        client.last_energy_recharge_tick = Some(anchor);
        client.last_continuous_energy_drain_tick = Some(anchor);
        client.server_sim_input_bits = Some(0x11); // Shift + Forward.

        crate::arena::advance_server_gameplay_energy(client, tick);
        assert_eq!(
            client.server_energy, 195,
            "authenticated Afterburner costs 120/s before authored 115/s recharge"
        );
        assert_eq!(client.server_energy_remainder, 0.0);
    }
}

#[test]
fn server_rejects_unowned_bomb_level_and_clamps_shrapnel_when_owned() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    // Client-requested weapon levels are not authority. A level-2 bomb is
    // stripped while the server-owned loadout is level 1. After the server
    // owns level 2, the same weapon is accepted but shrapnel still clamps to
    // the server-tracked shrapnel inventory.
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_weapons.bomb_level = 1;
        client.server_weapons.shrapnel = 4;
    }

    let mut clamped_shrapnel = client_position_packet(0x1122_33dc, 501, 701);
    clamped_shrapnel[20..22]
        .copy_from_slice(&(3_u16 | (1 << 5) | (3 << 8) | (31 << 10)).to_le_bytes());
    broadcast_position_observable(&state, alpha, &clamped_shrapnel);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&alpha)
            .expect("alpha should exist")
            .server_weapons
            .bomb_level,
        1
    );

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_weapons.bomb_level = 2;
    }
    let mut owned_bomb = client_position_packet(0x1122_33dd, 501, 701);
    owned_bomb[20..22].copy_from_slice(&(3_u16 | (1 << 5) | (3 << 8) | (31 << 10)).to_le_bytes());
    broadcast_position_observable(&state, alpha, &owned_bomb);
    let relayed = read_non_reliable_from_channel(&rx_b);
    // Project override: accepted fragment power follows the level-two parent
    // bomb, despite level-three guns and the forged level-four fragment request.
    assert_eq!(
        u16::from_le_bytes(relayed[19..21].try_into().unwrap()),
        3 | (1 << 5) | (1 << 8) | (4 << 10)
    );
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&alpha)
            .expect("alpha should exist")
            .server_weapons
            .bomb_level,
        2
    );
}

#[test]
fn server_strips_proximity_bombs_blocked_by_bomb_safety() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    broadcast_position_observable(&state, beta, &client_position_packet(0x1122_3344, 520, 700));
    drain_available(&rx_a);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let alpha_client = state.clients.get_mut(&alpha).expect("alpha should exist");
        alpha_client.server_weapons.proximity_bombs = true;
        alpha_client.server_weapons.bomb_level = 1;
    }

    let mut blocked_prox = client_position_packet(0x1122_3344, 500, 700);
    blocked_prox[20..22].copy_from_slice(&4_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &blocked_prox);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .active_projectiles
            .len(),
        0
    );
    reset_position_rate_limit(&state, alpha);
    reset_position_rate_limit(&state, beta);

    broadcast_position_observable(&state, beta, &client_position_packet(0x1122_3380, 800, 700));
    drain_available(&rx_a);

    let mut clear_prox = client_position_packet(0x1122_33dc, 501, 701);
    clear_prox[20..22].copy_from_slice(&4_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &clear_prox);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 4);
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .active_projectiles
            .len(),
        1
    );
}

#[test]
fn server_strips_mines_blocked_by_player_cap_or_stacked_position() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    reset_server_runtime_without_initial_bounty(&state, beta);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        for index in 0..arena_ship_max_mines(0) {
            state.record_projectile(ServerProjectile {
                id: 1_000 + index as u32,
                shot_group_id: 1_000 + index as u64,
                shot_index: 0,
                owner_id: alpha,
                owner_life_seq: 1,
                combat_timeline: None,
                presentation_source: None,
                weapon_data: 3 | (1 << 15),
                x: 600 + index as u16,
                y: 700,
                timestamp: 0x1122_3300 + index as u32,
                created_tick: crate::wire::current_tick(),
                velocity_x: 0,
                velocity_y: 0,
                expiry_tick: None,
                pos_x: (600 + index as u16) as f32,
                source_server_tick: None,
                precise_velocity: None,
                pos_y: 700.0,
                bounces_remaining: 0,
                behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
                proximity_target: None,
                proximity_sensor_end_tick: None,
                proximity_highest_offset: 0.0,
            });
        }
    }

    let mut over_player_cap = client_position_packet(0x1122_3344, 500, 700);
    over_player_cap[20..22].copy_from_slice(&(3_u16 | (1 << 15)).to_le_bytes());
    broadcast_position_observable(&state, alpha, &over_player_cap);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        1_700
    );

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.active_projectiles.clear();
        state.record_projectile(ServerProjectile {
            id: 2_000,
            shot_group_id: 2_000,
            shot_index: 0,
            owner_id: beta,
            owner_life_seq: 1,
            combat_timeline: None,
            presentation_source: None,
            weapon_data: 3 | (1 << 15),
            x: 501,
            y: 701,
            timestamp: 0x1122_3345,
            created_tick: crate::wire::current_tick(),
            velocity_x: 0,
            velocity_y: 0,
            expiry_tick: None,
            pos_x: 501.0,
            source_server_tick: None,
            precise_velocity: None,
            pos_y: 701.0,
            bounces_remaining: 0,
            behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
            proximity_target: None,
            proximity_sensor_end_tick: None,
            proximity_highest_offset: 0.0,
        });
    }

    let mut stacked_mine = client_position_packet(0x1122_3380, 501, 701);
    stacked_mine[20..22].copy_from_slice(&(3_u16 | (1 << 15)).to_le_bytes());
    broadcast_position_observable(&state, alpha, &stacked_mine);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert!(u16::from_le_bytes(relayed[21..23].try_into().unwrap()) >= 1_000);
}

#[test]
fn server_strips_mines_blocked_by_team_cap() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let (tx_c, rx_c) = mpsc::channel();
    let (tx_d, rx_d) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    let gamma = register_connection(&state, tx_c);
    let delta = register_connection(&state, tx_d);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    join_arena(&state, gamma, 0);
    join_arena(&state, delta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    drain_available(&rx_c);
    drain_available(&rx_d);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    reset_server_runtime_without_initial_bounty(&state, beta);
    reset_server_runtime_without_initial_bounty(&state, gamma);
    reset_server_runtime_without_initial_bounty(&state, delta);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        for id in [alpha, beta, gamma, delta] {
            state
                .clients
                .get_mut(&id)
                .expect("client should exist")
                .player
                .frequency = 0;
        }
        let teammates = [beta, gamma, delta];
        for index in 0..arena_team_max_mines() {
            let owner_id = teammates[index % teammates.len()];
            state.record_projectile(ServerProjectile {
                id: 3_000 + index as u32,
                shot_group_id: 3_000 + index as u64,
                shot_index: 0,
                owner_id,
                owner_life_seq: 1,
                combat_timeline: None,
                presentation_source: None,
                weapon_data: 3 | (1 << 15),
                x: 700 + index as u16,
                y: 700,
                timestamp: 0x1122_3300 + index as u32,
                created_tick: crate::wire::current_tick(),
                velocity_x: 0,
                velocity_y: 0,
                expiry_tick: None,
                pos_x: (700 + index as u16) as f32,
                source_server_tick: None,
                precise_velocity: None,
                pos_y: 700.0,
                bounces_remaining: 0,
                behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
                proximity_target: None,
                proximity_sensor_end_tick: None,
                proximity_highest_offset: 0.0,
            });
        }
    }

    let mut over_team_cap = client_position_packet(0x1122_3344, 500, 700);
    over_team_cap[20..22].copy_from_slice(&(3_u16 | (1 << 15)).to_le_bytes());
    broadcast_position_observable(&state, alpha, &over_team_cap);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        1_700
    );
}

#[test]
fn projectile_group_ids_never_wrap_or_reuse_with_projectile_ids() {
    let mut state = ServerState::default();
    state.next_projectile_id = u32::MAX;
    state.next_projectile_group_id = u64::MAX - 1;

    assert_eq!(state.allocate_projectile_id(), 1, "wire id wraps safely");
    assert_eq!(state.allocate_projectile_group_id(), Some(u64::MAX));
    assert_eq!(state.allocate_projectile_group_id(), None);
    assert_eq!(state.next_projectile_group_id, u64::MAX);
}

#[test]
fn server_requires_owned_multifire_capability_for_alternate_bullet_fire() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&alpha)
        .unwrap()
        .server_weapons
        .multifire_capable = false;
    drain_available(&rx_a);
    drain_available(&rx_b);

    let mut multifire = client_position_packet(0x1122_3344, 500, 700);
    multifire[20..22].copy_from_slice(&(2_u16 | (1 << 15)).to_le_bytes());
    broadcast_position_observable(&state, alpha, &multifire);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert!(
        !state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&alpha)
            .expect("alpha should exist")
            .server_weapons
            .multifire_capable
    );

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_weapons
            .multifire_capable = true;
    }
    let mut owned_multifire = client_position_packet(0x1122_3345, 500, 700);
    owned_multifire[20..22].copy_from_slice(&(2_u16 | (1 << 15)).to_le_bytes());
    broadcast_position_observable(&state, alpha, &owned_multifire);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed[19..21].try_into().unwrap()),
        2 | (1 << 15)
    );
    assert!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .clients
            .get(&alpha)
            .expect("alpha should exist")
            .server_weapons
            .multifire_capable
    );
    assert_eq!(
        state
            .lock()
            .expect("server state should not be poisoned")
            .active_projectiles
            .len(),
        3,
        "one accepted multifire packet must register all three projectiles"
    );
    let locked = state.lock().expect("server state should not be poisoned");
    let owner = locked
        .active_player_runtime_identity(alpha)
        .expect("multifire owner runtime");
    let sequence = crate::continuum_position::parse_client_position_snapshot(&owned_multifire)
        .and_then(|snapshot| snapshot.client_sequence)
        .expect("multifire sequence");
    let bindings = locked
        .active_projectiles
        .iter()
        .map(|projectile| {
            projectile
                .combat_timeline
                .expect("every human projectile child owns a combat timeline")
        })
        .collect::<Vec<_>>();
    assert!(bindings.iter().all(|binding| {
        *binding == bindings[0]
            && binding.owner == owner
            && binding.owner_input_sequence == sequence
    }));
}

#[test]
fn spawn_idle_tracks_accepted_neutral_frames_and_cancels_on_an_accepted_shot() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let id = register_connection(&state, tx);
    join_arena(&state, id, 0);
    // Install one test baseline, then use ordinary production admission.
    // The legacy relay adapter deliberately marks EVERY call as a frequency
    // baseline, so it cannot establish post-loading gameplay readiness.
    let baseline = client_position_packet(0, 501, 701);
    broadcast_position_observable(&state, id, &baseline);
    reset_position_rate_limit(&state, id);
    let sequence = state.lock().unwrap().clients[&id]
        .last_position_sequence
        .unwrap()
        .wrapping_add(1);
    record_position_witness(&state, id, sequence, 0);
    let neutral = client_position_packet_with_sequence(0, 501, 701, sequence);
    assert!(accept_position_relay(&state, id, &neutral).is_some());
    reset_position_rate_limit(&state, id);
    let now = Instant::now();
    {
        let guard = state.lock().unwrap();
        let client = &guard.clients[&id];
        let mut probe = client.spawn_idle.clone();
        assert!(
            probe.due(
                client.arena_stream_id,
                client.life_seq,
                now + Duration::from_secs(121),
                true
            ),
            "ordinary neutral positions must not keep an inactive spawn alive forever"
        );
    }
    let next = sequence.wrapping_add(1);
    // Give the next 60Hz sample enough server-clock room; this fixture emits
    // synchronously and must not manufacture a future authoritative shot.
    let tick = state.lock().unwrap().clients[&id]
        .last_position_server_tick
        .unwrap()
        + 4;
    let (runtime, input) =
        build_input_record_packet_with_valid_combat_view(&state, id, next, &[(0, 0, 0)], tick);
    assert!(crate::connection::record_client_input_observation_at_tick(
        &state, runtime, &input, tick
    ));
    let mut shot = client_position_packet_with_sequence(0, 501, 701, next);
    shot[20..22].copy_from_slice(&1_u16.to_le_bytes());
    let outcome = crate::arena::accept_position_relay_for_runtime_at_tick_outcome(
        &state, runtime, &shot, tick,
    );
    assert!(
        matches!(outcome, crate::arena::RuntimePositionAdmission::Accepted(_)),
        "{outcome:?}"
    );
    let mut guard = state.lock().unwrap();
    let client = guard.clients.get_mut(&id).unwrap();
    assert_eq!(client.last_accepted_weapon_data, Some(1));
    assert!(!client.spawn_idle.due(
        client.arena_stream_id,
        client.life_seq,
        now + Duration::from_secs(3600),
        true
    ));
}

#[test]
fn server_canonicalizes_forged_thor_power_before_relay_and_inventory_spend() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    {
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&alpha).unwrap();
        client.server_weapons.thors = 2;
        client.server_weapons.bomb_level = 4;
        client.server_weapons.shrapnel = 31;
    }
    let mut packet = client_position_packet(0x1122_3344, 501, 701);
    packet[20..22].copy_from_slice(&0xffe8_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &packet);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 8);
    let state = state.lock().unwrap();
    let client = &state.clients[&alpha];
    assert_eq!(client.server_weapons.thors, 1);
    assert_eq!(client.server_weapons.shrapnel, 31);
    assert_eq!(client.last_accepted_weapon_data, Some(8));
}

#[test]
fn server_requires_positive_energy_for_special_position_weapon_bits() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_energy = 0;
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .last_energy_recharge_tick = Some(crate::wire::current_tick());
    }

    let mut zero_energy_repel = client_position_packet(0x1122_3344, 500, 700);
    zero_energy_repel[20..22].copy_from_slice(&5_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &zero_energy_repel);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    assert_eq!(u16::from_le_bytes(relayed[21..23].try_into().unwrap()), 0);
    reset_position_rate_limit(&state, alpha);

    let release_after_zero_energy = client_position_packet(0x1122_3350, 501, 700);
    broadcast_position_observable(&state, alpha, &release_after_zero_energy);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    reset_position_rate_limit(&state, alpha);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_energy = 1;
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_recharge_per_second =
            crate::arena_settings::arena_ship_initial_recharge_per_second(0);
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .last_energy_recharge_tick = Some(crate::wire::current_tick());
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_weapons
            .repels = 2;
    }
    rewind_last_position_server_tick(&state, alpha, 65);
    advance_recharge_by_elapsed_ticks(&state, alpha, 65);

    let mut positive_energy_repel = client_position_packet(0x1122_3390, 501, 701);
    positive_energy_repel[20..22].copy_from_slice(&5_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &positive_energy_repel);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 5);
    assert_eq!(u16::from_le_bytes(relayed[21..23].try_into().unwrap()), 27);
    let guard = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        guard
            .clients
            .get(&alpha)
            .expect("alpha should exist")
            .server_repel_until_tick,
        None,
        "firing a repel must not give the shooter the target-only speed window"
    );
    assert_eq!(
        guard
            .clients
            .get(&alpha)
            .expect("alpha should exist")
            .server_weapons
            .repels,
        1
    );
    drop(guard);
    reset_position_rate_limit(&state, alpha);

    let mut held_repel = client_position_packet(0x1122_33f0, 502, 702);
    held_repel[20..22].copy_from_slice(&5_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &held_repel);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    {
        let state = state.lock().expect("server state should not be poisoned");
        assert_eq!(
            state
                .clients
                .get(&alpha)
                .expect("alpha should exist")
                .server_weapons
                .repels,
            1
        );
    }
    reset_position_rate_limit(&state, alpha);

    let release_repel = client_position_packet(0x1122_3450, 503, 703);
    broadcast_position_observable(&state, alpha, &release_repel);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 0);
    reset_position_rate_limit(&state, alpha);
    rewind_last_bomb_fire_tick(&state, alpha, arena_ship_weapon_fire_delay_ticks(0, 5));

    let mut pressed_again_repel = client_position_packet(0x1122_34b0, 504, 704);
    pressed_again_repel[20..22].copy_from_slice(&5_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &pressed_again_repel);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 5);
    let state = state.lock().expect("server state should not be poisoned");
    assert_eq!(
        state
            .clients
            .get(&alpha)
            .expect("alpha should exist")
            .server_weapons
            .repels,
        0
    );
}

#[test]
fn server_recharges_position_energy_from_server_ticks() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_energy = 900;
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .last_energy_recharge_tick = Some(crate::wire::current_tick());
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .server_recharge_per_second =
            crate::arena_settings::arena_ship_initial_recharge_per_second(0);
    }

    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    let first = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(first[21..23].try_into().unwrap()), 900);
    reset_position_rate_limit(&state, alpha);
    rewind_last_position_server_tick(&state, alpha, 100);
    advance_recharge_by_elapsed_ticks(&state, alpha, 100);

    let mut fired_after_one_second = client_position_packet(0x1122_33a8, 501, 701);
    fired_after_one_second[20..22].copy_from_slice(&2_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &fired_after_one_second);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(relayed[19..21].try_into().unwrap()), 2);
    let relayed_energy = u16::from_le_bytes(relayed[21..23].try_into().unwrap());
    assert!(
        (920..=922).contains(&relayed_energy),
        "expected recharge near 920, got {relayed_energy}"
    );
}

#[test]
fn server_recharges_position_energy_with_server_owned_recharge_prizes() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_energy = 900;
        client.server_energy_remainder = 0.0;
        client.server_recharge_per_second = crate::gameplay_rules::stat_value_after_prize(
            crate::arena_settings::arena_ship_initial_recharge_per_second(client.player.ship),
            crate::arena_settings::arena_ship_initial_recharge_per_second(client.player.ship),
            crate::arena_settings::arena_ship_recharge_upgrade_per_second(client.player.ship),
            crate::arena_settings::arena_ship_max_recharge_per_second(client.player.ship),
            false,
        );
        client.last_energy_recharge_tick = Some(crate::wire::current_tick());
    }

    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    drain_available(&rx_b);
    reset_position_rate_limit(&state, alpha);
    rewind_last_position_server_tick(&state, alpha, 100);
    advance_recharge_by_elapsed_ticks(&state, alpha, 100);

    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_33a8, 501, 701),
    );
    let relayed = read_non_reliable_from_channel(&rx_b);
    let expected = 900
        + crate::gameplay_rules::stat_value_after_prize(
            crate::arena_settings::arena_ship_initial_recharge_per_second(0),
            crate::arena_settings::arena_ship_initial_recharge_per_second(0),
            crate::arena_settings::arena_ship_recharge_upgrade_per_second(0),
            crate::arena_settings::arena_ship_max_recharge_per_second(0),
            false,
        )
        .floor()
        .max(0.0) as u16;
    let relayed_energy = u16::from_le_bytes(relayed[21..23].try_into().unwrap());
    assert!(
        (expected..=expected + 2).contains(&relayed_energy),
        "expected recharge near {expected}, got {relayed_energy}"
    );
}

#[test]
fn server_recharge_caps_at_server_owned_energy_capacity() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_energy = 990;
        client.server_max_energy = 1_000;
        client.last_energy_recharge_tick = Some(crate::wire::current_tick());
    }

    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    let first = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(first[21..23].try_into().unwrap()), 990);
    reset_position_rate_limit(&state, alpha);
    rewind_last_position_server_tick(&state, alpha, 100);
    advance_recharge_by_elapsed_ticks(&state, alpha, 100);

    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_33a8, 501, 701),
    );
    let relayed = read_non_reliable_from_channel(&rx_b);
    // server_max_energy was manually pinned to 1_000 above; energy
    // must cap at 1_000 even after recharge.
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        1_000
    );

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_energy = 1_090;
        client.server_max_energy = 1_100;
        client.server_energy_remainder = 0.0;
        client.last_energy_recharge_tick = Some(crate::wire::current_tick());
    }
    reset_position_rate_limit(&state, alpha);
    rewind_last_position_server_tick(&state, alpha, 100);
    advance_recharge_by_elapsed_ticks(&state, alpha, 100);
    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_340c, 502, 702),
    );
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        1_100
    );
}

#[test]
fn lowered_capacity_admits_only_the_bounded_private_ack_tail_until_client_converges() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let (authorized_overcap, lowered_max) = {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&alpha).expect("alpha");
        let initial = crate::arena_settings::arena_ship_initial_energy(client.player.ship);
        let step = crate::arena_settings::arena_ship_energy_upgrade(client.player.ship);
        let authorized_overcap = initial.saturating_add(step);
        client.server_max_energy = authorized_overcap;
        client.server_energy = authorized_overcap;
        client.last_reported_energy = initial.saturating_sub(100);
        client.apply_server_prize_effect(-2);
        let lowered_max = client.server_max_energy;
        assert!(authorized_overcap > lowered_max);
        assert_eq!(client.server_energy, authorized_overcap);
        assert_eq!(client.last_reported_energy, authorized_overcap);
        // Model the next natural recharge step occurring before the browser
        // receives the private ACK. Its admitted delivery tail remains bound
        // to the exact prior authoritative value.
        client.server_energy = lowered_max;
        (authorized_overcap, lowered_max)
    };

    let mut delayed_ack_tail = client_position_packet(0, 500, 700);
    delayed_ack_tail[18..20].copy_from_slice(&authorized_overcap.to_le_bytes());
    broadcast_position_observable(&state, alpha, &delayed_ack_tail);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        lowered_max
    );
    assert_eq!(
        state.lock().expect("state").clients[&alpha].last_reported_energy,
        authorized_overcap
    );

    reset_position_rate_limit(&state, alpha);
    let mut converged = client_position_packet(0, 501, 701);
    converged[18..20].copy_from_slice(&lowered_max.to_le_bytes());
    broadcast_position_observable(&state, alpha, &converged);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed[21..23].try_into().unwrap()),
        lowered_max
    );
    assert_eq!(
        state.lock().expect("state").clients[&alpha].last_reported_energy,
        0
    );

    reset_position_rate_limit(&state, alpha);
    let mut forged_regression = client_position_packet(0, 502, 702);
    forged_regression[18..20].copy_from_slice(&authorized_overcap.to_le_bytes());
    let forged_snapshot =
        crate::continuum_position::parse_client_position_snapshot(&forged_regression)
            .expect("position snapshot");
    let locked = state.lock().expect("state");
    let client = &locked.clients[&alpha];
    assert_eq!(client.last_reported_energy, 0);
    assert!(!crate::continuum_position::reported_energy_within_limit(
        forged_snapshot,
        client.server_max_energy,
        client.server_energy.max(client.last_reported_energy),
    ));
}

#[test]
fn server_rejects_obvious_position_teleports_between_updates() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_3344, 500, 700),
    );
    let first = read_non_reliable_from_channel(&rx_b);
    assert_eq!(u16::from_le_bytes(first[4..6].try_into().unwrap()), 500);
    reset_position_rate_limit(&state, alpha);

    let forged_jump = client_position_packet(0x1122_3345, 5_000, 700);
    broadcast_position_observable(&state, alpha, &forged_jump);
    assert!(rx_b.try_recv().is_err());
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        let cached = client
            .last_position
            .as_ref()
            .expect("first position should still be cached");
        assert_eq!(u16::from_le_bytes(cached[4..6].try_into().unwrap()), 500);
        assert_ne!(client.last_position_server_tick, Some(0x1122_3344));
    }
    reset_position_rate_limit(&state, alpha);

    let normal_move = client_position_packet(0x1122_3346, 540, 700);
    broadcast_position_observable(&state, alpha, &normal_move);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(relayed[0], 0x05);
    assert_eq!(u16::from_le_bytes(relayed[4..6].try_into().unwrap()), 540);
}

#[test]
fn server_rejects_position_paths_that_cross_solid_tiles() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let ((start_x, start_y), (end_x, end_y)) = first_clear_path_through_solid_tile();
    let first = client_position_packet(0x1122_3344, start_x, start_y);
    broadcast_position_observable(&state, alpha, &first);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed[4..6].try_into().unwrap()),
        start_x
    );
    assert_eq!(
        u16::from_le_bytes(relayed[15..17].try_into().unwrap()),
        start_y
    );

    reset_position_rate_limit(&state, alpha);
    let forged_wall_crossing = client_position_packet(0x1122_3358, end_x, end_y);
    broadcast_position_observable(&state, alpha, &forged_wall_crossing);
    assert!(rx_b.try_recv().is_err());
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("player should exist");
        let cached = client
            .last_position
            .as_ref()
            .expect("first position should remain cached");
        assert_eq!(
            u16::from_le_bytes(cached[4..6].try_into().unwrap()),
            start_x
        );
        assert_eq!(
            u16::from_le_bytes(cached[15..17].try_into().unwrap()),
            start_y
        );
        assert_ne!(client.last_position_server_tick, Some(0x1122_3344));
    }

    reset_position_rate_limit(&state, alpha);
    let normal_move = client_position_packet(0x1122_336c, start_x.saturating_add(4), start_y);
    broadcast_position_observable(&state, alpha, &normal_move);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed[4..6].try_into().unwrap()),
        start_x.saturating_add(4)
    );
}

#[test]
fn server_rejects_position_path_hulls_that_sweep_solid_tiles() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let ((start_x, start_y), (end_x, end_y)) = first_clear_swept_hull_path_near_solid_tile();
    let first = client_position_packet(0x1122_3344, start_x, start_y);
    broadcast_position_observable(&state, alpha, &first);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed[4..6].try_into().unwrap()),
        start_x
    );

    reset_position_rate_limit(&state, alpha);
    let forged_sweep = client_position_packet(0x1122_3358, end_x, end_y);
    broadcast_position_observable(&state, alpha, &forged_sweep);
    assert!(rx_b.try_recv().is_err());
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("player should exist");
        let cached = client
            .last_position
            .as_ref()
            .expect("first position should remain cached");
        assert_eq!(
            u16::from_le_bytes(cached[4..6].try_into().unwrap()),
            start_x
        );
        assert_ne!(client.last_position_server_tick, Some(0x1122_3344));
    }
}

#[test]
fn server_gates_position_relay_until_respawn_window_expires() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    broadcast_position_observable(
        &state,
        alpha,
        &client_position_packet(0x1122_3343, 500, 700),
    );
    drain_available(&rx_b);

    relay_death(
        &state,
        alpha,
        &[
            DEATH_REQUEST,
            (alpha & 0xff) as u8,
            (alpha >> 8) as u8,
            0,
            0,
        ],
    );
    drain_available(&rx_a);
    drain_available(&rx_b);

    // Force a future respawn_at_tick so we can assert the gate independently of
    // the arena's configured respawn delay.
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .respawn_at_tick = Some(crate::continuum::continuum_tick_add(
            crate::wire::current_tick(),
            500,
        ));
    }
    let position = client_position_packet(0x1122_3344, 500, 700);
    broadcast_position_observable(&state, alpha, &position);
    assert!(rx_b.try_recv().is_err());
    {
        let state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get(&alpha).expect("alpha should exist");
        assert!(!client.alive);
        assert!(client.respawn_at_tick.is_some());
    }

    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state
            .clients
            .get_mut(&alpha)
            .expect("alpha should exist")
            .respawn_at_tick = Some(crate::continuum::continuum_tick_sub(
            crate::wire::current_tick(),
            1,
        ));
    }
    let (spawn_x, spawn_y, respawn_bounty, respawn_energy) = {
        let state = state.lock().expect("server state should not be poisoned");
        let mut prospective = state.clients[&alpha].clone();
        prospective.alive = true;
        prospective.reset_server_runtime_for_ship_at_tick(crate::wire::current_tick());
        let (spawn_x, spawn_y) = server_spawn_coordinates(state.zone_map());
        (
            spawn_x,
            spawn_y,
            prospective.server_bounty,
            prospective.server_energy,
        )
    };
    let mut respawn_position = client_position_packet(0x1122_3345, spawn_x, spawn_y);
    respawn_position[1] = 0;
    respawn_position[16..18].copy_from_slice(&respawn_bounty.to_le_bytes());
    respawn_position[18..20].copy_from_slice(&respawn_energy.to_le_bytes());
    broadcast_position_observable(&state, alpha, &respawn_position);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(relayed[0], 0x05);
    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    assert_eq!(
        crate::arena::large_position_coordinates(&relayed),
        client
            .canonical_movement_witness
            .map(|w| (w.wire_x, w.wire_y)),
        "public relay and replay anchor use the same server-selected spawn"
    );
    assert!(client.alive);
    assert!(client.respawn_at_tick.is_none());
    assert_eq!(
        client.server_bounty,
        crate::arena_settings::arena_ship_initial_bounty(0)
    );
    assert_eq!(
        client.server_energy,
        crate::arena_settings::arena_ship_max_energy(0)
    );
    assert!(client.server_weapons.proximity_bombs);
}

#[test]
fn rejected_first_respawn_reports_are_fully_atomic_then_exact_spawn_commits_once() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    broadcast_position_observable(&state, alpha, &client_position_packet(0, 500, 700));
    drain_available(&rx_a);
    drain_available(&rx_b);
    relay_death(
        &state,
        alpha,
        &[
            DEATH_REQUEST,
            (alpha & 0xff) as u8,
            (alpha >> 8) as u8,
            0,
            0,
        ],
    );
    drain_available(&rx_a);
    drain_available(&rx_b);

    let accepted_tick = crate::wire::current_tick().wrapping_add(10);
    let sequence = 4_120_u16;
    let (identity, spawn_x, spawn_y, respawn_bounty, respawn_energy, blocked_x, blocked_y) = {
        let mut locked = state.lock().expect("server state");
        let map = locked.zone_map();
        let blocked_index = map
            .tiles
            .iter()
            .position(|&tile| tile == 1)
            .expect("real zone contains a static wall tile");
        let blocked_x =
            ((blocked_index % MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5) as u16;
        let blocked_y =
            ((blocked_index / MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5) as u16;
        let (spawn_x, spawn_y) = server_spawn_coordinates(map);
        let identity = locked
            .active_life_identity(alpha)
            .expect("dead life identity");
        let client = locked.clients.get_mut(&alpha).expect("alpha");
        client.respawn_at_tick = Some(accepted_tick.wrapping_sub(1));
        client.server_sim_state = Some(crate::state::ServerSimMovement {
            position_x: 500.25,
            position_y: 700.75,
            orientation: 0.125,
            velocity_x: 11.25,
            velocity_y: -9.75,
            energy: 321.5,
            max_energy: 654.5,
        });
        client.server_sim_anchor_tick = Some(accepted_tick.wrapping_sub(20));
        client.server_sim_input_bits = Some(9);
        client.movement_drift_x = 7.25;
        client.movement_drift_y = -4.5;
        client
            .position_history
            .push_back(crate::state::RecentPositionSample {
                tick: accepted_tick.wrapping_sub(5),
                x: 500.25,
                y: 700.75,
            });
        client
            .aoi_tier_memory
            .insert(beta, crate::visibility::AoiTier::Mid);
        client
            .recent_damagers
            .push((identity, accepted_tick.wrapping_sub(2)));
        client.pending_snapshot_weapon = 0x1234;
        let mut prospective = client.clone();
        prospective.alive = true;
        prospective.reset_server_runtime_for_ship_at_tick(accepted_tick);
        (
            identity,
            spawn_x,
            spawn_y,
            prospective.server_bounty,
            prospective.server_energy,
            blocked_x,
            blocked_y,
        )
    };
    let runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(alpha)
        .expect("runtime");
    assert!(
        crate::connection::record_client_lifecycle_input_baseline_at_tick(
            &state,
            runtime,
            &build_input_record_packet(sequence, &[(0, 0, 0)]),
            accepted_tick,
        )
    );
    let beta_life_seq = state.lock().expect("state").clients[&beta].life_seq;
    crate::snapshot::queue_watch_damage_event_to_recipients(
        &state,
        &[runtime],
        crate::snapshot::PendingWatchDamage {
            victim_id: alpha,
            shooter_id: beta,
            victim_life_seq: identity.life_seq,
            shooter_life_seq: beta_life_seq,
            weapon_data: 1,
            damage: 2,
            energy_before: 3,
            energy_after: 1,
            server_tick: accepted_tick,
            damage_seq: 4,
            projectile_id: 5,
            lethal: false,
        },
    );

    let packet_at = |x: u16, y: u16| {
        let mut packet = client_position_packet_with_sequence(0, x, y, sequence);
        packet[1] = 0;
        packet[16..18].copy_from_slice(&respawn_bounty.to_le_bytes());
        packet[18..20].copy_from_slice(&respawn_energy.to_le_bytes());
        sync_client_position_canonical_state_from_wire(&mut packet);
        packet
    };
    let teleport_x = if spawn_x.abs_diff(500) > 512 {
        500
    } else {
        spawn_x.saturating_add(1_024)
    };
    let noncanonical_before =
        crate::security::peek_security_reject_count_for_test("position", "noncanonical_respawn");
    // A late old-source kill must not make the browser infer a bounty
    // benefit for this successor. Even at the exact spawn and due tick, that
    // one-field mismatch must remain an atomic rejection, not a relaxed
    // lifecycle baseline. The client-side regression covers preventing it.
    let mut wrong_bounty = packet_at(spawn_x, spawn_y);
    wrong_bounty[16..18].copy_from_slice(
        &respawn_bounty
            .checked_add(1)
            .expect("fixture bounty headroom")
            .to_le_bytes(),
    );
    let mut wrong_energy = packet_at(spawn_x, spawn_y);
    wrong_energy[18..20].copy_from_slice(
        &respawn_energy
            .checked_add(1)
            .expect("fixture energy headroom")
            .to_le_bytes(),
    );
    for (label, packet) in [
        ("blocked-tile", packet_at(blocked_x, blocked_y)),
        ("teleport", packet_at(teleport_x, 700)),
        ("retired-kill-bounty-mismatch", wrong_bounty),
        ("energy-mismatch", wrong_energy),
    ] {
        assert!(
            crate::continuum_position::parse_client_position_snapshot(&packet).is_some(),
            "{label} must be well-framed so rejection exercises respawn policy"
        );
        let before = witness_admission_state(&state, alpha);
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &packet,
                accepted_tick,
            )
            .is_none(),
            "{label} first-respawn report must reject"
        );
        assert_eq!(
            witness_admission_state(&state, alpha),
            before,
            "{label} rejection must preserve the complete dead-life state"
        );
        assert!(
            rx_a.try_recv().is_err(),
            "{label} rejection emits no owner packet"
        );
        assert!(
            rx_b.try_recv().is_err(),
            "{label} rejection emits no peer packet"
        );
    }
    assert_eq!(
        crate::security::peek_security_reject_count_for_test("position", "noncanonical_respawn"),
        noncanonical_before,
        "stale dead-life positions are lifecycle desynchronization, not attack telemetry"
    );

    let canonical = packet_at(spawn_x, spawn_y);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &canonical,
            accepted_tick,
        )
        .is_some(),
        "the exact neutral spawn must commit the due respawn"
    );
    let after_commit = witness_admission_state(&state, alpha);
    assert!(after_commit.alive);
    assert_eq!(after_commit.respawn_at_tick, None);
    assert_eq!(after_commit.server_bounty, respawn_bounty);
    assert_eq!(after_commit.server_energy, respawn_energy);
    assert_eq!(
        after_commit
            .last_position
            .as_deref()
            .and_then(crate::arena::large_position_coordinates),
        after_commit
            .canonical_movement_witness
            .map(|w| (w.wire_x, w.wire_y))
    );
    assert_eq!(
        after_commit.pending_snapshot_events.len(),
        1,
        "the exact respawn commits private authority without erasing the queued public damage fact"
    );
    assert_eq!(after_commit.aoi_tier_memory.len(), 0);
    assert_eq!(after_commit.position_history.len(), 1);
    assert_eq!(
        after_commit.actor_motion_history.current_samples().count(),
        1
    );
    let motion_segment = after_commit
        .actor_motion_history
        .current_segment()
        .expect("respawn establishes an exact motion segment");
    assert_eq!(motion_segment.actor.life, identity);

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &canonical,
            accepted_tick,
        )
        .is_none(),
        "the committed first position cannot be consumed twice"
    );
    assert_eq!(witness_admission_state(&state, alpha), after_commit);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn respawn_boundary_preserves_current_neutral_input_and_drops_old_history_across_wrap() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    broadcast_position_observable(&state, player_id, &client_position_packet(0, 500, 700));
    drain_available(&rx);
    relay_death(
        &state,
        player_id,
        &[DEATH_REQUEST, player_id as u8, (player_id >> 8) as u8, 0, 0],
    );
    drain_available(&rx);

    let due_tick = crate::continuum::CONTINUUM_TICK_MASK;
    let (identity, runtime, spawn_x, spawn_y, bounty, energy) = {
        let mut locked = state.lock().expect("state");
        let (spawn_x, spawn_y) = server_spawn_coordinates(locked.zone_map());
        let identity = locked.active_life_identity(player_id).expect("dead life");
        let runtime = locked
            .active_player_runtime_identity(player_id)
            .expect("runtime");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.respawn_at_tick = Some(due_tick.wrapping_sub(1));
        // This fixture moves the server to a synthetic tick wrap. Move the
        // pre-death time account with it instead of going backward from the
        // process's real uptime to zero.
        client.movement_time_budget = crate::state::MovementTimeBudget::default()
            .admit(crate::continuum::continuum_tick_sub(due_tick, 10), None, 0)
            .unwrap();
        let mut prospective = client.clone();
        prospective.alive = true;
        prospective.reset_server_runtime_for_ship_at_tick(due_tick);
        (
            identity,
            runtime,
            spawn_x,
            spawn_y,
            prospective.server_bounty,
            prospective.server_energy,
        )
    };
    let position_at = |sequence| {
        let mut packet = client_position_packet_with_sequence(0, spawn_x, spawn_y, sequence);
        packet[1] = 0;
        packet[16..18].copy_from_slice(&bounty.to_le_bytes());
        packet[18..20].copy_from_slice(&energy.to_le_bytes());
        sync_client_position_canonical_state_from_wire(&mut packet);
        packet
    };

    let before_non_neutral_input = witness_admission_state(&state, player_id);
    assert!(!crate::connection::record_client_input_observation_at_tick(
        &state,
        runtime,
        &build_input_record_packet(u16::MAX, &[(1 << 3, 0, 0)]),
        due_tick.wrapping_sub(1),
    ));
    assert_eq!(
        witness_admission_state(&state, player_id),
        before_non_neutral_input,
        "ordinary non-neutral dead-life input is inert before the reliable baseline"
    );
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &position_at(u16::MAX),
            due_tick,
        )
        .is_none(),
        "neutral geometry cannot commit with non-neutral input bits"
    );
    assert_eq!(
        witness_admission_state(&state, player_id),
        before_non_neutral_input,
        "rejected position leaves the dead runtime intact"
    );

    assert!(
        crate::connection::record_client_lifecycle_input_baseline_at_tick(
            &state,
            runtime,
            &build_input_record_packet(0, &[(0, 0, 0)]),
            0,
        )
    );
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(&state, identity, &position_at(0), 0,)
            .is_some()
    );
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert!(client.alive);
        assert_eq!(client.life_position_sequence_base, Some(0));
        assert_eq!(client.last_position_sequence, Some(0));
        assert_eq!(client.last_input_seq, Some(0));
        assert_eq!(client.last_input_record.unwrap().bits, 0);
        assert_eq!(client.recent_input_records.len(), 1);
        assert_eq!(client.recent_input_records[0].sequence, 0);
        assert_eq!(client.recent_input_records[0].input.bits, 0);
        assert_eq!(client.recent_input_records[0].door_phase_tick, 0);
    }

    let after_commit = witness_admission_state(&state, player_id);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(&state, identity, &position_at(0), 1,)
            .is_none()
    );
    assert_eq!(witness_admission_state(&state, player_id), after_commit);

    assert!(crate::connection::record_client_input_observation_at_tick(
        &state,
        runtime,
        &build_input_record_packet(1, &[(0, 0, 0)]),
        1,
    ));
    let mut successor = position_at(1);
    let point = after_commit
        .canonical_movement_witness
        .map(|w| (w.wire_x, w.wire_y))
        .unwrap();
    successor[12..14].copy_from_slice(&point.0.to_le_bytes());
    successor[8..10].copy_from_slice(&point.1.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut successor);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(&state, identity, &successor, 1,)
            .is_some(),
        "the first ordinary new-life input and movement frame has no blackout"
    );
    assert_eq!(
        state.lock().expect("state").clients[&player_id].last_position_sequence,
        Some(1)
    );
}

fn reset_position_rate_limit(state: &Arc<Mutex<ServerState>>, player_id: u16) {
    let mut state = state.lock().expect("server state should not be poisoned");
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("player should exist");
    client.last_position_relay_at = Some(Instant::now() - Duration::from_millis(100));
    client.position_relay_tokens = 3.0;
    client.position_relay_burst_count = 0;
}

fn rewind_last_gun_fire_tick(state: &Arc<Mutex<ServerState>>, player_id: u16, delay_ticks: u32) {
    let mut state = state.lock().expect("server state should not be poisoned");
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("player should exist");
    let timestamp = server_tick_before_delay(delay_ticks);
    client.last_weapon_fire_tick = Some(timestamp);
    client.last_gun_fire_tick = Some(timestamp);
}

fn rewind_last_bomb_fire_tick(state: &Arc<Mutex<ServerState>>, player_id: u16, delay_ticks: u32) {
    let mut state = state.lock().expect("server state should not be poisoned");
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("player should exist");
    let timestamp = server_tick_before_delay(delay_ticks);
    client.last_weapon_fire_tick = Some(timestamp);
    client.last_bomb_fire_tick = Some(timestamp);
}

fn rewind_last_position_server_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    elapsed_ticks: u32,
) {
    let mut state = state.lock().expect("server state should not be poisoned");
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("player should exist");
    let tick = crate::continuum::continuum_tick_sub(crate::wire::current_tick(), elapsed_ticks);
    client.last_position_server_tick = Some(tick);
    client.last_continuous_energy_drain_tick = Some(tick);
}

fn server_tick_before_delay(delay_ticks: u32) -> u32 {
    crate::continuum::continuum_tick_sub(crate::wire::current_tick(), delay_ticks.saturating_add(1))
}

fn advance_recharge_by_elapsed_ticks(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    elapsed_ticks: u32,
) {
    let tick = crate::wire::current_tick();
    let mut state = state.lock().expect("server state should not be poisoned");
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("player should exist");
    client.last_energy_recharge_tick =
        Some(crate::continuum::continuum_tick_sub(tick, elapsed_ticks));
    crate::arena::advance_server_recharge(client, tick);
}

fn advance_gameplay_energy_by_elapsed_ticks(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    elapsed_ticks: u32,
) {
    let tick = crate::wire::current_tick();
    let mut state = state.lock().expect("server state should not be poisoned");
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("player should exist");
    let anchor = crate::continuum::continuum_tick_sub(tick, elapsed_ticks);
    client.last_energy_recharge_tick = Some(anchor);
    client.last_continuous_energy_drain_tick = Some(anchor);
    crate::arena::advance_server_gameplay_energy(client, tick);
}

/// Recharge is gated on EMP alone: the EngineShutdown *prize* zeroes thrust but
/// still recharges (`ShipController:244-259`). Both sources share the
/// movement union, so this pins the distinction the browser now receives as two
/// separate deadlines.
#[test]
fn recharge_freezes_for_emp_but_not_for_the_shutdown_prize() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);

    let tick = crate::wire::current_tick();
    let rate = crate::arena_settings::arena_ship_initial_recharge_per_second(0);
    assert!(rate > 0.0, "fixture needs a nonzero Chaos recharge rate");

    let energy_after = |shutdown_prize: bool, emp: bool| -> u16 {
        let anchor = crate::continuum::continuum_tick_sub(tick, 100);
        let mut guard = state.lock().expect("server state should not be poisoned");
        let client = guard.clients.get_mut(&alpha).expect("alpha should exist");
        client.server_energy = 500;
        client.server_energy_remainder = 0.0;
        client.server_recharge_per_second = rate;
        client.last_energy_recharge_tick = Some(anchor);
        client.server_engine_shutdown_until_tick = shutdown_prize.then_some(tick + 1_000);
        client.server_engine_shutdown_start_sequence = shutdown_prize.then_some(1);
        client.server_emp_until_tick = emp.then_some(tick + 1_000);
        client.server_emp_start_sequence = None;
        crate::arena::advance_server_recharge(client, tick);
        client.server_energy
    };

    let recharged = energy_after(false, false);
    assert!(recharged > 500, "plain recharge should gain energy");
    assert_eq!(
        energy_after(true, false),
        recharged,
        "a Shutdown prize must not freeze recharge"
    );
    assert_eq!(energy_after(false, true), 500, "EMP must freeze recharge");
}

fn first_solid_tile_position() -> (u16, u16) {
    let map = zone_map();
    let index = map
        .tiles
        .iter()
        .position(|&tile| position_blocking_tile(tile))
        .expect("Chaos Zone should include solid tiles");
    let x = (((index % MAP_TILES) as f32 + 0.5) * TILE_SIZE_PX)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16;
    let y = (((index / MAP_TILES) as f32 + 0.5) * TILE_SIZE_PX)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16;
    (x, y)
}

fn first_clear_path_through_solid_tile() -> ((u16, u16), (u16, u16)) {
    let map = zone_map();
    for y in 2..MAP_TILES - 2 {
        for x in 2..MAP_TILES - 2 {
            if !position_blocking_tile(map.tiles[y * MAP_TILES + x]) {
                continue;
            }
            if !position_blocking_tile(map.tiles[y * MAP_TILES + x - 2])
                && !position_blocking_tile(map.tiles[y * MAP_TILES + x - 1])
                && !position_blocking_tile(map.tiles[y * MAP_TILES + x + 1])
                && !position_blocking_tile(map.tiles[y * MAP_TILES + x + 2])
            {
                return (
                    tile_center_position(x - 2, y),
                    tile_center_position(x + 2, y),
                );
            }
            if !position_blocking_tile(map.tiles[(y - 2) * MAP_TILES + x])
                && !position_blocking_tile(map.tiles[(y - 1) * MAP_TILES + x])
                && !position_blocking_tile(map.tiles[(y + 1) * MAP_TILES + x])
                && !position_blocking_tile(map.tiles[(y + 2) * MAP_TILES + x])
            {
                return (
                    tile_center_position(x, y - 2),
                    tile_center_position(x, y + 2),
                );
            }
        }
    }
    panic!("Chaos Zone should include a solid tile with clear opposite neighbors");
}

fn first_clear_position_touching_solid_tile() -> (u16, u16) {
    let map = zone_map();
    for y in 1..MAP_TILES - 1 {
        for x in 1..MAP_TILES - 1 {
            if position_blocking_tile(map.tiles[y * MAP_TILES + x]) {
                continue;
            }
            let touches_solid = position_blocking_tile(map.tiles[y * MAP_TILES + x - 1])
                || position_blocking_tile(map.tiles[y * MAP_TILES + x + 1])
                || position_blocking_tile(map.tiles[(y - 1) * MAP_TILES + x])
                || position_blocking_tile(map.tiles[(y + 1) * MAP_TILES + x]);
            if touches_solid {
                return tile_center_position(x, y);
            }
        }
    }
    panic!("Chaos Zone should include a clear tile touching a solid tile");
}

fn first_clear_swept_hull_path_near_solid_tile() -> ((u16, u16), (u16, u16)) {
    let map = zone_map();
    for y in 1..MAP_TILES - 2 {
        for x in 2..MAP_TILES - 2 {
            if !position_blocking_tile(map.tiles[y * MAP_TILES + x]) {
                continue;
            }
            for path_y in [y.saturating_sub(1), y + 1] {
                if path_y >= MAP_TILES {
                    continue;
                }
                let start = tile_center_position(x - 2, path_y);
                let end = tile_center_position(x + 2, path_y);
                if path_is_clear_for_center_but_not_hull(map, start, end) {
                    return (start, end);
                }
            }
        }
    }
    panic!("Chaos Zone should include a clear swept-hull path near a solid tile");
}

fn path_is_clear_for_center_but_not_hull(
    map: &ZoneMap,
    start: (u16, u16),
    end: (u16, u16),
) -> bool {
    let blocked_at_world = |x, y| position_blocking_tile(tile_at_world(map, x, y));
    let blocked_tile = |tile_x: usize, tile_y: usize| {
        map.tiles
            .get(tile_y * MAP_TILES + tile_x)
            .is_none_or(|&tile| position_blocking_tile(tile))
    };

    !reported_position_path_crosses_blocked_tile(
        start.0,
        start.1,
        end.0,
        end.1,
        TILE_SIZE_PX,
        blocked_at_world,
    ) && !reported_position_hull_overlaps_blocked_tile(
        start.0,
        start.1,
        arena_ship_radius_pixels(0),
        MAP_TILES,
        TILE_SIZE_PX,
        blocked_tile,
    ) && !reported_position_hull_overlaps_blocked_tile(
        end.0,
        end.1,
        arena_ship_radius_pixels(0),
        MAP_TILES,
        TILE_SIZE_PX,
        blocked_tile,
    ) && reported_position_path_hull_overlaps_blocked_tile(
        start.0,
        start.1,
        end.0,
        end.1,
        arena_ship_radius_pixels(0),
        MAP_TILES,
        TILE_SIZE_PX,
        blocked_tile,
    )
}

fn tile_center_position(x: usize, y: usize) -> (u16, u16) {
    let world_x = ((x as f32 + 0.5) * TILE_SIZE_PX)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16;
    let world_y = ((y as f32 + 0.5) * TILE_SIZE_PX)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16;
    (world_x, world_y)
}

fn build_input_record_packet(base_seq: u16, entries: &[(u16, u8, u8)]) -> Vec<u8> {
    // Mirrors `ClientInputRecordPacket::encode` so server tests can
    // exercise the wire format without pulling the protocol crate's
    // helpers into the server bin's `#[path]` tree.
    let mut bytes = Vec::with_capacity(
        4 + entries.len() * (4 + crate::combat_timeline::CombatViewReceiptV1::ENCODED_LEN),
    );
    bytes.push(0x62);
    bytes.extend_from_slice(&base_seq.to_le_bytes());
    bytes.push(entries.len() as u8);
    for (index, &(bits, direction, reserved)) in entries.iter().enumerate() {
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.push(direction);
        bytes.push(reserved);
        bytes.extend_from_slice(&unavailable_combat_view_receipt(
            base_seq.wrapping_add(index as u16),
        ));
    }
    bytes
}

fn build_input_record_packet_with_valid_combat_view(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    base_seq: u16,
    entries: &[(u16, u8, u8)],
    server_tick: u32,
) -> (crate::authority_identity::PlayerRuntimeIdentity, Vec<u8>) {
    let (runtime, receipts) = {
        let mut locked = state.lock().expect("server state should not be poisoned");
        let runtime = locked
            .active_player_runtime_identity(player_id)
            .expect("verified input fixture requires an active runtime");
        let client = locked.clients.get_mut(&player_id).expect("client");
        let receipts = (0..entries.len())
            .map(|index| {
                let sequence = base_seq.wrapping_add(index as u16);
                let decision = client
                    .observer_combat_timeline
                    .seed_input_decision_for_test(sequence, server_tick)
                    .expect("joined client owns a test combat-view decision");
                crate::combat_timeline::CombatViewReceiptV1 {
                    observer_life_seq: runtime.life.life_seq,
                    input_sequence: sequence,
                    timeline_generation: decision.generation,
                    latest_game_applied_snapshot_id: decision.applied_snapshot_id,
                    flags: 0,
                }
            })
            .collect::<Vec<_>>();
        (runtime, receipts)
    };
    let mut bytes = Vec::with_capacity(
        4 + entries.len() * (4 + crate::combat_timeline::CombatViewReceiptV1::ENCODED_LEN),
    );
    bytes.push(0x62);
    bytes.extend_from_slice(&base_seq.to_le_bytes());
    bytes.push(entries.len() as u8);
    for (&(bits, direction, reserved), receipt) in entries.iter().zip(receipts) {
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.push(direction);
        bytes.push(reserved);
        bytes.extend_from_slice(&receipt.encode());
    }
    (runtime, bytes)
}

fn seed_combat_view_decision_for_test(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    sequence: u16,
    server_tick: u32,
) {
    state
        .lock()
        .expect("server state should not be poisoned")
        .clients
        .get_mut(&player_id)
        .expect("combat-view fixture client")
        .observer_combat_timeline
        .seed_input_decision_for_test(sequence, server_tick)
        .expect("joined fixture owns a combat-view timeline");
}

#[derive(Debug, PartialEq)]
struct WitnessAdmissionState {
    alive: bool,
    respawn_at_tick: Option<u32>,
    server_bounty: u16,
    server_energy: u16,
    server_max_energy: u16,
    server_energy_remainder: f32,
    last_energy_recharge_tick: Option<u32>,
    last_continuous_energy_drain_tick: Option<u32>,
    server_super_until_tick: Option<u32>,
    server_shield_until_tick: Option<u32>,
    server_emp_until_tick: Option<u32>,
    server_rocket_until_tick: Option<u32>,
    server_rocket_input_asserted: bool,
    server_rocket_authorizations: Vec<crate::state::ServerRocketAuthorization>,
    server_status: crate::authority::StatusInventory,
    server_weapons: crate::authority::WeaponInventory,
    last_reported_energy: u16,
    life_started_tick: Option<u32>,
    last_position: Option<Vec<u8>>,
    visibility_cell: Option<crate::visibility::VisibilityCell>,
    last_position_sequence: Option<u16>,
    life_position_sequence_base: Option<u16>,
    last_position_server_tick: Option<u32>,
    last_position_relay_at: Option<Instant>,
    position_relay_tokens: f32,
    position_relay_burst_count: u8,
    latest_weapon_seq: Option<u16>,
    weapon_seq_mask: u64,
    last_weapon_fire_tick: Option<u32>,
    last_gun_fire_tick: Option<u32>,
    last_bomb_fire_tick: Option<u32>,
    last_accepted_weapon_data: Option<u16>,
    last_accepted_weapon_tick: Option<u32>,
    last_input_seq: Option<u16>,
    last_input_record: Option<crate::state::ClientInputObservation>,
    recent_input_records: Vec<crate::state::SequencedInputObservation>,
    recent_input_rocket_assertions: Vec<(u16, bool)>,
    canonical_position_witnesses: Vec<crate::state::CanonicalPositionWitness>,
    canonical_movement_witness: Option<crate::state::CanonicalPositionWitness>,
    pending_warp_prize_relocations: Vec<crate::state::PendingWarpPrizeRelocation>,
    server_sim_state: Option<crate::state::ServerSimMovement>,
    server_sim_anchor_tick: Option<u32>,
    server_sim_input_bits: Option<u16>,
    movement_drift_x: f32,
    movement_drift_y: f32,
    pending_repel_observations: Vec<crate::state::ServerRepelObservation>,
    retired_repel_applications: Vec<crate::state::ServerRepelRetiredApplication>,
    position_history: Vec<crate::state::RecentPositionSample>,
    actor_motion_history: crate::combat_timeline::ActorMotionHistory,
    aoi_tier_memory: std::collections::HashMap<u16, crate::visibility::AoiTier>,
    recent_damagers: Vec<(crate::authority_identity::LifeIdentity, u32)>,
    pending_snapshot_events: Vec<crate::snapshot::PendingWatchDamageEnvelope>,
    pending_snapshot_weapon: u16,
    active_projectiles: crate::authority_projectiles::ProjectileRegistry,
}

fn witness_admission_state(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
) -> WitnessAdmissionState {
    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client should exist");
    WitnessAdmissionState {
        alive: client.alive,
        respawn_at_tick: client.respawn_at_tick,
        server_bounty: client.server_bounty,
        server_energy: client.server_energy,
        server_max_energy: client.server_max_energy,
        server_energy_remainder: client.server_energy_remainder,
        last_energy_recharge_tick: client.last_energy_recharge_tick,
        last_continuous_energy_drain_tick: client.last_continuous_energy_drain_tick,
        server_super_until_tick: client.server_super_until_tick,
        server_shield_until_tick: client.server_shield_until_tick,
        server_emp_until_tick: client.server_emp_until_tick,
        server_rocket_until_tick: client.server_rocket_until_tick,
        server_rocket_input_asserted: client.server_rocket_input_asserted,
        server_rocket_authorizations: client
            .server_rocket_authorizations
            .iter()
            .copied()
            .collect(),
        server_status: client.server_status,
        server_weapons: client.server_weapons,
        last_reported_energy: client.last_reported_energy,
        life_started_tick: client.life_started_tick,
        last_position: client.last_position.clone(),
        visibility_cell: client.visibility_cell,
        last_position_sequence: client.last_position_sequence,
        life_position_sequence_base: client.life_position_sequence_base,
        last_position_server_tick: client.last_position_server_tick,
        last_position_relay_at: client.last_position_relay_at,
        position_relay_tokens: client.position_relay_tokens,
        position_relay_burst_count: client.position_relay_burst_count,
        latest_weapon_seq: client.latest_weapon_seq,
        weapon_seq_mask: client.weapon_seq_mask,
        last_weapon_fire_tick: client.last_weapon_fire_tick,
        last_gun_fire_tick: client.last_gun_fire_tick,
        last_bomb_fire_tick: client.last_bomb_fire_tick,
        last_accepted_weapon_data: client.last_accepted_weapon_data,
        last_accepted_weapon_tick: client.last_accepted_weapon_tick,
        last_input_seq: client.last_input_seq,
        last_input_record: client.last_input_record,
        recent_input_records: client.recent_input_records.iter().copied().collect(),
        recent_input_rocket_assertions: client
            .recent_input_rocket_assertions
            .iter()
            .copied()
            .collect(),
        canonical_position_witnesses: client
            .canonical_position_witnesses
            .iter()
            .copied()
            .collect(),
        canonical_movement_witness: client.canonical_movement_witness,
        pending_warp_prize_relocations: client
            .pending_warp_prize_relocations
            .iter()
            .copied()
            .collect(),
        server_sim_state: client.server_sim_state,
        server_sim_anchor_tick: client.server_sim_anchor_tick,
        server_sim_input_bits: client.server_sim_input_bits,
        movement_drift_x: client.movement_drift_x,
        movement_drift_y: client.movement_drift_y,
        pending_repel_observations: client.pending_repel_observations.iter().copied().collect(),
        retired_repel_applications: client.retired_repel_applications.iter().copied().collect(),
        position_history: client.position_history.iter().copied().collect(),
        actor_motion_history: client.actor_motion_history.clone(),
        aoi_tier_memory: client.aoi_tier_memory.clone(),
        recent_damagers: client.recent_damagers.clone(),
        pending_snapshot_events: client
            .pending_snapshot_events
            .watch_damage
            .iter()
            .copied()
            .collect(),
        pending_snapshot_weapon: client.pending_snapshot_weapon,
        active_projectiles: state.active_projectiles.clone(),
    }
}

#[test]
fn ship_change_sequence_barrier_drops_old_life_before_input_or_position_mutation() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);

    let old_position = large_position_from_client(
        player_id,
        &client_position_packet_with_sequence(0, 7_500, 8_200, 99),
    )
    .expect("old-life position");
    {
        let mut state = state.lock().expect("state");
        state.set_client_position(player_id, old_position);
        state
            .clients
            .get_mut(&player_id)
            .expect("client")
            .last_position_server_tick = Some(10_000);
    }

    update_ship_at_tick(&state, player_id, &[SHIP_CHANGE_REQUEST, 1, 100, 0], 10_001);
    drain_available(&rx);
    let (spawn_x, spawn_y, reset_bounty, reset_energy) = {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        assert!(client.last_position.is_none());
        assert!(client.last_position_server_tick.is_none());
        assert_eq!(client.life_position_sequence_base, Some(100));
        let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(state.zone_map());
        (spawn_x, spawn_y, client.server_bounty, client.server_energy)
    };

    let mut stale = client_position_packet_with_sequence(0, 7_505, 8_200, 99);
    stale[1] = 0;
    sync_client_position_canonical_state_from_wire(&mut stale);
    stale.extend_from_slice(&build_input_record_packet(99, &[(0, 0, 0)]));
    let after_change = witness_admission_state(&state, player_id);
    let teleport_before =
        crate::security::peek_security_reject_count_for_test("position", "teleport");
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        stale,
        "late-old-life-peer",
        &mut joined,
    );
    assert_eq!(witness_admission_state(&state, player_id), after_change);
    assert_eq!(
        state
            .lock()
            .expect("state")
            .pending_position_inputs
            .queued_count_for_player(player_id),
        0
    );
    assert_eq!(
        crate::security::peek_security_reject_count_for_test("position", "teleport"),
        teleport_before
    );

    // The lifecycle request can also overtake an already-sent realtime
    // position carrying the same sequence the request declares as its new
    // base. Channel provenance, not attacker-controlled position contents,
    // distinguishes that stale unordered packet from the required reliable
    // baseline.
    let mut overtaken_same_sequence =
        client_position_packet_with_sequence(0, spawn_x, spawn_y, 100);
    overtaken_same_sequence[11] = 32;
    overtaken_same_sequence[16..18].copy_from_slice(&reset_bounty.to_le_bytes());
    overtaken_same_sequence[18..20].copy_from_slice(&reset_energy.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut overtaken_same_sequence);
    overtaken_same_sequence.extend_from_slice(&build_input_record_packet(100, &[(0, 0, 0)]));
    let noncanonical_before = crate::security::peek_security_reject_count_for_test(
        "position",
        "noncanonical_ship_life_reset",
    );
    crate::connection::handle_client_packet(
        &state,
        player_id,
        overtaken_same_sequence,
        "same-sequence-old-life-peer",
        &mut joined,
    );
    assert_eq!(witness_admission_state(&state, player_id), after_change);
    assert_eq!(
        crate::security::peek_security_reject_count_for_test(
            "position",
            "noncanonical_ship_life_reset"
        ),
        noncanonical_before
    );

    // The same cross-lane overtaking applies to the standalone rolling input
    // packet sent beside that old-life position. The reliable request has
    // already reserved sequence 100 for the new neutral baseline, so neither
    // neutral nor non-neutral unordered input may claim or rewrite it first.
    let input_rewrite_before =
        crate::security::peek_security_reject_count_for_test("input_record", "sequence_rewrite");
    crate::connection::handle_client_packet(
        &state,
        player_id,
        build_input_record_packet(100, &[(0x09, 17, 0)]),
        "same-sequence-old-life-peer",
        &mut joined,
    );
    assert_eq!(witness_admission_state(&state, player_id), after_change);
    assert_eq!(
        crate::security::peek_security_reject_count_for_test("input_record", "sequence_rewrite"),
        input_rewrite_before
    );

    let mut first_new_life = client_position_packet_with_sequence(0, spawn_x, spawn_y, 100);
    first_new_life[1] = 0;
    first_new_life[16..18].copy_from_slice(&reset_bounty.to_le_bytes());
    first_new_life[18..20].copy_from_slice(&reset_energy.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut first_new_life);
    first_new_life.extend_from_slice(&build_input_record_packet(100, &[(0, 0, 0)]));
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable(0, &first_new_life),
        "new-life-peer",
        &mut joined,
    );
    crate::server_tick::run_server_tick(&state, Instant::now());
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert_eq!(client.last_position_sequence, Some(100));
        assert_eq!(client.life_position_sequence_base, Some(100));
        assert_eq!(
            client
                .last_position
                .as_deref()
                .and_then(crate::arena::large_position_coordinates),
            client
                .canonical_movement_witness
                .map(|w| (w.wire_x, w.wire_y))
        );
        let witness = client
            .canonical_movement_witness
            .expect("accepted lifecycle baseline seeds an exact replay anchor");
        assert_eq!(witness.sequence, 100);
        assert_eq!(witness.life_seq, client.life_seq);
        assert_eq!(witness.direction, 0);
        assert_eq!(
            (witness.state.position_x, witness.state.position_y),
            (witness.wire_x as f32, witness.wire_y as f32)
        );
        assert_eq!((witness.wire_velocity_x, witness.wire_velocity_y), (0, 0));
        assert_eq!(client.canonical_position_witnesses.back(), Some(&witness));
    }

    // The old packet is still rejected after the first new-life position;
    // reordering cannot install its embedded input after the cutover.
    state
        .lock()
        .expect("state")
        .clear_pending_position_inputs_for_player(player_id);
    let after_accept = witness_admission_state(&state, player_id);
    let mut stale_after_accept = client_position_packet_with_sequence(0, 7_505, 8_200, 99);
    stale_after_accept[1] = 0;
    stale_after_accept.extend_from_slice(&build_input_record_packet(99, &[(0, 0, 0)]));
    crate::connection::handle_client_packet(
        &state,
        player_id,
        stale_after_accept,
        "reordered-old-life-peer",
        &mut joined,
    );
    assert_eq!(witness_admission_state(&state, player_id), after_accept);
    assert_eq!(
        state
            .lock()
            .expect("state")
            .pending_position_inputs
            .queued_count_for_player(player_id),
        0
    );
}

#[test]
fn initial_join_reliable_neutral_baseline_establishes_position_authority() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let sequence = 0x5a5a;
    let (spawn_x, spawn_y, bounty, energy) = {
        let mut guard = state.lock().expect("state");
        let (x, y) = crate::zone::server_spawn_coordinates(guard.zone_map());
        let client = guard.clients.get_mut(&player_id).expect("client");
        client.life_position_sequence_base = Some(sequence);
        (x, y, client.server_bounty, client.server_energy)
    };
    let mut baseline = client_position_packet_with_sequence(0, spawn_x, spawn_y, sequence);
    baseline[1] = 0;
    baseline[16..18].copy_from_slice(&bounty.to_le_bytes());
    baseline[18..20].copy_from_slice(&energy.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut baseline);
    baseline.extend_from_slice(&build_input_record_packet(sequence, &[(0, 0, 0)]));
    let delivery_id = 0;
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable(delivery_id, &baseline),
        "initial-neutral-baseline-peer",
        &mut joined,
    );
    crate::server_tick::run_server_tick(&state, Instant::now());

    let guard = state.lock().expect("state");
    let client = &guard.clients[&player_id];
    assert_eq!(client.last_position_sequence, Some(sequence));
    assert_eq!(client.last_input_seq, Some(sequence));
    assert_eq!(client.last_input_record.map(|input| input.bits), Some(0));
    let baseline_samples: Vec<_> = client.actor_motion_history.current_samples().collect();
    assert_eq!(baseline_samples.len(), 1);
    assert_eq!(
        baseline_samples[0].sample_time.parts(),
        (
            client
                .last_position_server_tick
                .expect("baseline admission tick"),
            0
        ),
        "the neutral life baseline consumes no simulated movement duration"
    );
    assert_eq!(
        client
            .last_position
            .as_deref()
            .and_then(crate::arena::large_position_coordinates),
        client
            .canonical_movement_witness
            .map(|w| (w.wire_x, w.wire_y))
    );
    drop(guard);
    assert!(rx.try_iter().any(|packet| {
        crate::reliable::reliable_payload(&packet).is_some_and(|payload| {
            payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)
        })
    }));
}

#[test]
fn initial_join_half_pixel_roundup_rejects_without_retiring_exact_spawn_retry() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let sequence = 0xfffe;
    let (spawn_x, spawn_y, bounty, energy) = {
        let mut guard = state.lock().expect("state");
        let (x, y) = crate::zone::server_spawn_coordinates(guard.zone_map());
        let client = guard.clients.get_mut(&player_id).expect("client");
        client.life_position_sequence_base = Some(sequence);
        (x, y, client.server_bounty, client.server_energy)
    };
    let packet_at = |x, y| {
        let mut packet = client_position_packet_with_sequence(0, x, y, sequence);
        packet[1] = 0;
        packet[16..18].copy_from_slice(&bounty.to_le_bytes());
        packet[18..20].copy_from_slice(&energy.to_le_bytes());
        sync_client_position_canonical_state_from_wire(&mut packet);
        packet.extend_from_slice(&build_input_record_packet(sequence, &[(0, 0, 0)]));
        packet
    };
    let mut joined = true;

    // Legacy SetCoordinates presents integer N at N+0.5, which rounds to
    // N+1 on the u16 position wire. It must not consume the one-shot life
    // boundary or prevent an exact canonical retry.
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable(0, &packet_at(spawn_x + 1, spawn_y + 1)),
        "initial-half-pixel-roundup-peer",
        &mut joined,
    );
    {
        let guard = state.lock().expect("state");
        let client = &guard.clients[&player_id];
        assert_eq!(client.life_position_sequence_base, Some(sequence));
        assert_eq!(client.last_position_sequence, None);
        assert!(client.last_position.is_none());
    }

    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable(1, &packet_at(spawn_x, spawn_y)),
        "initial-exact-spawn-retry-peer",
        &mut joined,
    );
    crate::server_tick::run_server_tick(&state, Instant::now());

    let guard = state.lock().expect("state");
    let client = &guard.clients[&player_id];
    assert_eq!(client.last_position_sequence, Some(sequence));
    assert_eq!(
        client
            .last_position
            .as_deref()
            .and_then(crate::arena::large_position_coordinates),
        client
            .canonical_movement_witness
            .map(|w| (w.wire_x, w.wire_y))
    );
    drop(guard);
    assert!(rx.try_iter().any(|packet| {
        crate::reliable::reliable_payload(&packet).is_some_and(|payload| {
            payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)
        })
    }));
}

#[test]
fn ship_change_sequence_barrier_is_wrap_aware() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let mut state = state.lock().expect("state");
    let client = state.clients.get_mut(&player_id).expect("client");
    client.life_position_sequence_base = Some(0);
    assert!(!client.position_sequence_belongs_to_active_life(u16::MAX));
    assert!(client.position_sequence_belongs_to_active_life(0));
    assert!(!client.position_sequence_belongs_to_active_life(1));

    client.last_position_sequence = Some(1);
    assert!(!client.position_sequence_belongs_to_active_life(u16::MAX));
    client.last_position_sequence = Some(40);
    assert!(!client.position_sequence_belongs_to_active_life(u16::MAX));
    client.last_position_sequence = Some(u16::MAX);
    assert!(client.position_sequence_belongs_to_active_life(0));
}

#[test]
fn ship_change_lied_boundary_or_old_high_sequence_cannot_seed_new_life() {
    for sequence in [99_u16, 101] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        drain_available(&rx);
        update_ship_at_tick(&state, player_id, &[SHIP_CHANGE_REQUEST, 1, 100, 0], 10_001);
        drain_available(&rx);
        let before = witness_admission_state(&state, player_id);
        let mut old = client_position_packet_with_sequence(0, 7_500, 8_200, sequence);
        old[1] = 0;
        old.extend_from_slice(&build_input_record_packet(sequence, &[(0, 0, 0)]));
        let mut joined = true;

        crate::connection::handle_client_packet(
            &state,
            player_id,
            old,
            "lied-boundary-peer",
            &mut joined,
        );

        assert_eq!(witness_admission_state(&state, player_id), before);
        assert_eq!(
            state
                .lock()
                .expect("state")
                .pending_position_inputs
                .queued_count_for_player(player_id),
            0
        );
    }
}

#[test]
fn first_ship_life_position_and_input_must_be_canonical_reset_atomically() {
    for variant in 0..4 {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        drain_available(&rx);
        update_ship_at_tick(&state, player_id, &[SHIP_CHANGE_REQUEST, 1, 100, 0], 10_001);
        drain_available(&rx);
        let (spawn_x, spawn_y, bounty, energy) = {
            let state = state.lock().expect("state");
            let client = &state.clients[&player_id];
            let (x, y) = crate::zone::server_spawn_coordinates(state.zone_map());
            (x, y, client.server_bounty, client.server_energy)
        };
        let mut packet = client_position_packet_with_sequence(0, spawn_x, spawn_y, 100);
        packet[1] = 0;
        packet[16..18].copy_from_slice(&bounty.to_le_bytes());
        packet[18..20].copy_from_slice(&energy.to_le_bytes());
        match variant {
            0 => packet[12..14].copy_from_slice(&(spawn_x + 32).to_le_bytes()),
            1 => packet[6..8].copy_from_slice(&1_i16.to_le_bytes()),
            2 => packet[20..22].copy_from_slice(&1_u16.to_le_bytes()),
            3 => {}
            _ => unreachable!(),
        }
        sync_client_position_canonical_state_from_wire(&mut packet);
        if variant == 3 {
            packet.extend_from_slice(&build_input_record_packet(99, &[(0, 0, 0), (0, 0, 0)]));
        } else {
            packet.extend_from_slice(&build_input_record_packet(100, &[(0, 0, 0)]));
        }
        let before = witness_admission_state(&state, player_id);
        let count_before = crate::security::peek_security_reject_count_for_test(
            "position",
            "noncanonical_ship_life_reset",
        );
        let mut joined = true;

        crate::connection::handle_client_packet(
            &state,
            player_id,
            crate::reliable::reliable(0, &packet),
            "forged-reset-peer",
            &mut joined,
        );

        assert_eq!(witness_admission_state(&state, player_id), before);
        assert_eq!(
            state
                .lock()
                .expect("state")
                .pending_position_inputs
                .queued_count_for_player(player_id),
            0
        );
        assert_eq!(
            crate::security::peek_security_reject_count_for_test(
                "position",
                "noncanonical_ship_life_reset"
            ),
            count_before + 1
        );
    }
}

#[test]
fn lifecycle_baseline_commits_before_wrapped_realtime_and_only_then_emits_app_receipt() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    // This queue-ordering fixture deliberately knows the neutral successor
    // before receiving the application receipt. Use an authored point; the
    // separate random-spawn/wrapped-replay regressions exercise the shipped
    // FFA profile without this controlled spatial setup.
    use_fixed_authored_spawn_fixture(&mut state.lock().unwrap());
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let death_tick = crate::wire::current_tick();
    {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("client");
        client.alive = false;
        client.respawn_at_tick = Some(death_tick);
        client.clear_server_runtime();
        state
            .advance_client_life_at_tick(player_id, death_tick)
            .expect("advance dead life");
        let client = state.clients.get_mut(&player_id).expect("client");
        client.last_position_relay_at = Some(Instant::now());
        client.position_relay_tokens = -100.0;
    }
    let (spawn_x, spawn_y, bounty, energy, life_seq) = {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        let (x, y) = crate::zone::server_spawn_coordinates(state.zone_map());
        let mut prospective = client.clone();
        prospective.alive = true;
        prospective.respawn_at_tick = None;
        prospective.reset_server_runtime_for_ship_at_tick(death_tick);
        (
            x,
            y,
            prospective.server_bounty,
            prospective.server_energy,
            client.life_seq,
        )
    };
    let packet = |sequence: u16| {
        let mut packet = client_position_packet_with_sequence(0, spawn_x, spawn_y, sequence);
        packet[1] = 0;
        packet[16..18].copy_from_slice(&bounty.to_le_bytes());
        packet[18..20].copy_from_slice(&energy.to_le_bytes());
        sync_client_position_canonical_state_from_wire(&mut packet);
        packet.extend_from_slice(&build_input_record_packet(sequence, &[(0, 0, 0)]));
        packet
    };
    let baseline_delivery_id = 0;
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable(baseline_delivery_id, &packet(u16::MAX)),
        "lifecycle-app-receipt-peer",
        &mut joined,
    );
    crate::connection::handle_client_packet(
        &state,
        player_id,
        packet(0),
        "lifecycle-app-receipt-peer",
        &mut joined,
    );

    // The transport ACK exists before authoritative admission, but no
    // application acceptance receipt may exist yet.
    let before_tick = rx.try_iter().collect::<Vec<_>>();
    assert!(before_tick.iter().any(|packet| {
        crate::reliable::reliable_ack_ids(packet).is_some_and(|ids| ids == [baseline_delivery_id])
    }));
    assert!(!before_tick.iter().any(|packet| {
        crate::reliable::reliable_payload(packet).is_some_and(|payload| {
            payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)
        })
    }));

    let receipt_tick = crate::wire::current_tick();
    crate::server_tick::run_server_tick_at_tick(&state, Instant::now(), receipt_tick);

    let after_tick = rx.try_iter().collect::<Vec<_>>();
    let receipt_packet = after_tick
        .iter()
        .find(|packet| {
            crate::reliable::reliable_payload(packet).is_some_and(|payload| {
                payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)
            })
        })
        .cloned()
        .expect("accepted baseline must emit one application receipt");
    let (receipt_id, receipt) = crate::reliable::reliable_parts(&receipt_packet)
        .expect("application receipt must use the reliable transport");
    assert_eq!(
        receipt,
        crate::roster::lifecycle_baseline_accepted(
            baseline_delivery_id,
            life_seq,
            u16::MAX,
            receipt_tick,
            (
                u16::from_le_bytes(receipt[16..18].try_into().unwrap()),
                u16::from_le_bytes(receipt[18..20].try_into().unwrap())
            ),
        )
    );
    {
        let state = state.lock().expect("state");
        assert_eq!(state.clients[&player_id].last_position_sequence, Some(0));
        assert!(state.clients[&player_id].alive);
        assert!(
            state.clients[&player_id].position_relay_tokens < 1.0,
            "private admission cannot refill the suppressed public relay bucket"
        );
    }

    crate::reliable::resend_due_reliable_to_clients(
        &state,
        Instant::now() + Duration::from_secs(4),
    );
    assert!(
        rx.try_iter().any(|packet| packet == receipt_packet),
        "a lost application receipt must remain retained for resend"
    );
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable_ack(receipt_id),
        "lifecycle-app-receipt-peer",
        &mut joined,
    );
}

#[test]
fn blocked_lifecycle_spawn_retains_the_exact_baseline_until_geometry_clears() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    use_fixed_authored_spawn_fixture(&mut state.lock().unwrap());
    let (tx, rx) = mpsc::channel();
    let id = register_connection(&state, tx);
    join_arena(&state, id, 0);
    drain_available(&rx);
    let tick = crate::wire::current_tick();
    let expires = crate::continuum::continuum_tick_add(tick, 100);
    let (x, y, bounty, energy) = {
        let mut locked = state.lock().unwrap();
        let c = locked.clients.get_mut(&id).unwrap();
        c.alive = false;
        c.respawn_at_tick = Some(tick);
        c.clear_server_runtime();
        locked.advance_client_life_at_tick(id, tick).unwrap();
        let mut prospective = locked.clients[&id].clone();
        prospective.alive = true;
        prospective.reset_server_runtime_for_ship_at_tick(tick);
        let (x, y) = crate::zone::server_spawn_coordinates(locked.zone_map());
        let mut brick = vec![0; 11];
        brick[9..11].copy_from_slice(&prospective.player.frequency.wrapping_add(1).to_le_bytes());
        locked.active_bricks.push(crate::state::ServerBrick {
            tiles: vec![(x / 16, y / 16)],
            packet: brick,
            expires_at_tick: expires,
        });
        (x, y, prospective.server_bounty, prospective.server_energy)
    };
    let mut packet = client_position_packet_with_sequence(0, x, y, 100);
    packet[1] = 0;
    packet[16..18].copy_from_slice(&bounty.to_le_bytes());
    packet[18..20].copy_from_slice(&energy.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut packet);
    packet.extend_from_slice(&build_input_record_packet(100, &[(0, 0, 0)]));
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        id,
        crate::reliable::reliable(0, &packet),
        "blocked-spawn-peer",
        &mut joined,
    );
    for delta in [0, 1, 99] {
        crate::server_tick::run_server_tick_at_tick(
            &state,
            Instant::now(),
            crate::continuum::continuum_tick_add(tick, delta),
        );
        let locked = state.lock().unwrap();
        let c = &locked.clients[&id];
        assert!(!c.alive && c.respawn_at_tick.is_some());
        assert!(c.last_position.is_none() && c.canonical_movement_witness.is_none());
        assert_eq!(
            locked.pending_position_inputs.queued_count_for_player(id),
            1,
            "temporary geometry must not consume a transport-ACKed lifecycle baseline without its application receipt"
        );
        assert!(!rx.try_iter().any(|p| {
            crate::reliable::reliable_payload(&p)
                .is_some_and(|p| p.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED))
        }));
    }
    crate::server_tick::run_server_tick_at_tick(&state, Instant::now(), expires);
    let locked = state.lock().unwrap();
    let c = &locked.clients[&id];
    assert!(c.alive && c.respawn_at_tick.is_none());
    assert_eq!(
        crate::arena::large_position_coordinates(c.last_position.as_ref().unwrap()),
        Some((x, y))
    );
    assert!(locked.pending_position_inputs.is_empty());
    assert_eq!(
        rx.try_iter()
            .filter(|p| crate::reliable::reliable_payload(p)
                .is_some_and(|p| p.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)))
            .count(),
        1
    );
}

#[test]
fn lifecycle_respawns_choose_diverse_server_positions_and_receipt_exact_committed_state() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let mut destinations = std::collections::BTreeSet::new();
    let mut joined = true;
    for attempt in 0..16_u32 {
        let tick = crate::wire::current_tick();
        let sequence = (attempt as u16).wrapping_mul(10);
        let (spawn_x, spawn_y, bounty, energy, life) = {
            let mut locked = state.lock().unwrap();
            let c = locked.clients.get_mut(&player_id).unwrap();
            c.alive = false;
            c.respawn_at_tick = Some(tick);
            c.clear_server_runtime();
            locked.advance_client_life_at_tick(player_id, tick).unwrap();
            let mut prospective = locked.clients[&player_id].clone();
            prospective.alive = true;
            prospective.respawn_at_tick = None;
            prospective.reset_server_runtime_for_ship_at_tick(tick);
            let (x, y) = crate::zone::server_spawn_coordinates(locked.zone_map());
            (
                x,
                y,
                prospective.server_bounty,
                prospective.server_energy,
                prospective.life_seq,
            )
        };
        let mut packet = client_position_packet_with_sequence(0, spawn_x, spawn_y, sequence);
        packet[1] = 0;
        packet[16..18].copy_from_slice(&bounty.to_le_bytes());
        packet[18..20].copy_from_slice(&energy.to_le_bytes());
        sync_client_position_canonical_state_from_wire(&mut packet);
        packet.extend_from_slice(&build_input_record_packet(sequence, &[(0, 0, 0)]));
        let reliable = crate::reliable::reliable(attempt, &packet);
        if attempt == 0 {
            // Exercise a delayed ingress explicitly. The handler authenticates
            // its neutral input at receipt time, so the committing tick below
            // must follow ingress rather than reuse this earlier life tick.
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        crate::connection::handle_client_packet(
            &state,
            player_id,
            reliable.clone(),
            "random-respawn-peer",
            &mut joined,
        );
        let tick = crate::wire::current_tick();
        crate::server_tick::run_server_tick_at_tick(&state, Instant::now(), tick);
        let receipt = rx
            .try_iter()
            .find_map(|packet| {
                crate::reliable::reliable_parts(&packet)
                    .filter(|(_, payload)| {
                        payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)
                    })
                    .map(|(id, payload)| (id, payload.to_vec()))
            })
            .unwrap_or_else(|| {
                let locked = state.lock().unwrap();
                let client = &locked.clients[&player_id];
                panic!(
                    "the accepted neutral baseline must receive an exact application receipt; attempt={attempt} tick={tick} current_tick={} queued={} alive={} respawn={:?} last_input={:?} inputs={:?}",
                    crate::wire::current_tick(),
                    !locked.pending_position_inputs.is_empty(),
                    client.alive,
                    client.respawn_at_tick,
                    client.last_input_seq,
                    client.recent_input_records,
                );
            });
        let accepted_point = {
            let locked = state.lock().unwrap();
            let c = &locked.clients[&player_id];
            assert!(c.alive);
            let point = crate::arena::large_position_coordinates(c.last_position.as_ref().unwrap())
                .unwrap();
            assert_eq!(
                receipt.1,
                crate::roster::lifecycle_baseline_accepted(attempt, life, sequence, tick, point)
            );
            assert!(
                (5928..=10440).contains(&point.0) && (5928..=10440).contains(&point.1),
                "a one-player FFA respawn uses the reference population-sized centre, not RadarMode=4 frequency corners: {point:?}"
            );
            let witness = c.canonical_movement_witness.unwrap();
            assert_eq!((witness.wire_x, witness.wire_y), point);
            assert_eq!(
                (witness.state.position_x, witness.state.position_y),
                (point.0 as f32, point.1 as f32)
            );
            assert_eq!(c.server_sim_state, Some(witness.state));
            assert_eq!(c.server_sim_anchor_tick, Some(tick));
            assert_eq!(c.last_position_sequence, Some(sequence));
            assert_eq!(
                (
                    witness.state.velocity_x,
                    witness.state.velocity_y,
                    witness.state.orientation
                ),
                (0.0, 0.0, 0.0)
            );
            assert!(
                !crate::continuum_position::reported_position_hull_overlaps_blocked_tile(
                    point.0,
                    point.1,
                    crate::arena_settings::arena_ship_radius_pixels(0),
                    crate::zone::MAP_TILES,
                    crate::zone::TILE_SIZE_PX,
                    |x, y| crate::zone::position_blocking_tile(
                        locked.zone_map().tiles[y * crate::zone::MAP_TILES + x]
                    ),
                )
            );
            point
        };
        destinations.insert(accepted_point);
        // The transport and application each keep their own exact retry
        // identity. Replaying the request cannot roll another destination.
        crate::connection::handle_client_packet(
            &state,
            player_id,
            reliable,
            "random-respawn-peer",
            &mut joined,
        );
        crate::server_tick::run_server_tick_at_tick(
            &state,
            Instant::now(),
            crate::wire::current_tick(),
        );
        assert_eq!(
            crate::arena::large_position_coordinates(
                state.lock().unwrap().clients[&player_id]
                    .last_position
                    .as_ref()
                    .unwrap()
            ),
            Some(accepted_point)
        );
        crate::connection::handle_client_packet(
            &state,
            player_id,
            crate::reliable::reliable_ack(receipt.0),
            "random-respawn-peer",
            &mut joined,
        );
        drain_available(&rx);
    }
    assert!(
        destinations.len() >= 12,
        "16 real lifecycle resets must not keep using the fixed map centre: {destinations:?}"
    );
}

#[test]
fn early_lifecycle_baseline_waits_for_authoritative_respawn_deadline() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let death_tick = crate::wire::current_tick();
    let respawn_tick = crate::continuum::continuum_tick_add(death_tick, 200);
    {
        let mut state = state.lock().expect("state");
        let client = state.clients.get_mut(&player_id).expect("client");
        client.alive = false;
        client.respawn_at_tick = Some(respawn_tick);
        client.clear_server_runtime();
        state
            .advance_client_life_at_tick(player_id, death_tick)
            .expect("advance dead life");
    }
    let (spawn_x, spawn_y, bounty, energy, life_seq) = {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        let (x, y) = crate::zone::server_spawn_coordinates(state.zone_map());
        let mut prospective = client.clone();
        prospective.alive = true;
        prospective.respawn_at_tick = None;
        prospective.reset_server_runtime_for_ship_at_tick(respawn_tick);
        (
            x,
            y,
            prospective.server_bounty,
            prospective.server_energy,
            client.life_seq,
        )
    };
    let sequence = u16::MAX;
    let mut packet = client_position_packet_with_sequence(0, spawn_x, spawn_y, sequence);
    packet[1] = 0;
    packet[16..18].copy_from_slice(&bounty.to_le_bytes());
    packet[18..20].copy_from_slice(&energy.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut packet);
    packet.extend_from_slice(&build_input_record_packet(sequence, &[(0, 0, 0)]));
    let baseline_delivery_id = 0;
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable(baseline_delivery_id, &packet),
        "early-lifecycle-baseline-peer",
        &mut joined,
    );
    let transport_packets = rx.try_iter().collect::<Vec<_>>();
    assert!(transport_packets.iter().any(|packet| {
        crate::reliable::reliable_ack_ids(packet).is_some_and(|ids| ids == [baseline_delivery_id])
    }));

    let before_deadline = crate::continuum::continuum_tick_add(respawn_tick, u32::MAX);
    crate::server_tick::run_server_tick_at_tick(&state, Instant::now(), before_deadline);
    {
        let state = state.lock().expect("state");
        assert!(!state.clients[&player_id].alive);
        assert_eq!(
            state
                .pending_position_inputs
                .queued_count_for_player(player_id),
            1,
            "the exact lifecycle baseline remains protected until the deadline"
        );
    }
    assert!(!rx.try_iter().any(|packet| {
        crate::reliable::reliable_payload(&packet).is_some_and(|payload| {
            payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)
        })
    }));

    crate::server_tick::run_server_tick_at_tick(&state, Instant::now(), respawn_tick);
    let receipt = rx
        .try_iter()
        .find(|packet| {
            crate::reliable::reliable_payload(packet).is_some_and(|payload| {
                payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)
            })
        })
        .expect("the due exact baseline must receive application acceptance");
    assert_eq!(
        crate::reliable::reliable_parts(&receipt).map(|(_, payload)| payload),
        Some(
            crate::roster::lifecycle_baseline_accepted(
                baseline_delivery_id,
                life_seq,
                sequence,
                respawn_tick,
                crate::arena::large_position_coordinates(
                    state.lock().unwrap().clients[&player_id]
                        .last_position
                        .as_ref()
                        .unwrap()
                )
                .unwrap(),
            )
            .as_slice(),
        )
    );
    let state = state.lock().expect("state");
    assert!(state.clients[&player_id].alive);
    assert_eq!(
        state.clients[&player_id].last_position_sequence,
        Some(sequence)
    );
    assert!(state.pending_position_inputs.is_empty());
}

#[test]
fn successive_reliable_ship_cutovers_queue_and_receipt_each_wrapped_baseline() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let mut joined = true;

    let baseline_packet = |sequence: u16| {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert_eq!(client.life_position_sequence_base, Some(sequence));
        assert!(client.last_position_sequence.is_none());
        let (x, y) = crate::zone::server_spawn_coordinates(locked.zone_map());
        frequency_baseline_packet(
            crate::continuum_position::ClientPositionSnapshot {
                direction: 0,
                timestamp: 0,
                client_sequence: Some(sequence),
                x,
                y,
                velocity_x: 0,
                velocity_y: 0,
                togglables: 0,
                bounty: client.server_bounty,
                energy: client.server_energy,
                weapon: 0,
                canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_f32(
                    x as f32, y as f32, 0.0, 0.0, 0.0,
                ),
                repel_snapshot_id: None,
                repel_application_sequence: None,
                repel_delivery_generation: None,
                warp_collectible_id: None,
            },
            sequence,
        )
    };

    for (index, (ship, sequence)) in [(1_u8, u16::MAX), (5_u8, 0_u16)].into_iter().enumerate() {
        let request_delivery_id = (index as u32) * 2;
        let baseline_delivery_id = request_delivery_id + 1;
        let request = [
            SHIP_CHANGE_REQUEST,
            ship,
            sequence.to_le_bytes()[0],
            sequence.to_le_bytes()[1],
        ];
        crate::connection::handle_client_packet(
            &state,
            player_id,
            crate::reliable::reliable(request_delivery_id, &request),
            "successive-ship-cutover-peer",
            &mut joined,
        );
        assert_eq!(
            state.lock().expect("state").clients[&player_id].player.ship,
            if index == 0 { 0 } else { 1 },
            "reliable ship mutation waits for the server application stage"
        );
        crate::server_tick::run_server_tick(&state, Instant::now());
        let life_seq = state.lock().expect("state").clients[&player_id].life_seq;
        let baseline = baseline_packet(sequence);

        crate::connection::handle_client_packet(
            &state,
            player_id,
            crate::reliable::reliable(baseline_delivery_id, &baseline),
            "successive-ship-cutover-peer",
            &mut joined,
        );
        let receipt_tick = crate::wire::current_tick();
        crate::server_tick::run_server_tick_at_tick(&state, Instant::now(), receipt_tick);

        let packets = rx.try_iter().collect::<Vec<_>>();
        let receipts = packets
            .iter()
            .filter_map(|packet| crate::reliable::reliable_payload(packet))
            .filter(|payload| payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED))
            .collect::<Vec<_>>();
        assert_eq!(
            receipts,
            [crate::roster::lifecycle_baseline_accepted(
                baseline_delivery_id,
                life_seq,
                sequence,
                receipt_tick,
                crate::arena::large_position_coordinates(
                    state.lock().unwrap().clients[&player_id]
                        .last_position
                        .as_ref()
                        .unwrap()
                )
                .unwrap(),
            )
            .as_slice()],
            "each exact reliable baseline must receive its own application receipt"
        );
        let locked = state.lock().expect("state");
        assert_eq!(
            locked.clients[&player_id].last_position_sequence,
            Some(sequence)
        );
        assert_eq!(locked.clients[&player_id].player.ship, ship);
    }
}

#[test]
fn warp_receipts_survive_zero_public_tokens_duplicate_markers_loss_and_id_wrap() {
    let tick = crate::wire::current_tick();
    let sequence = u16::MAX;
    let (state, player_id, identity, mut packet) =
        warp_relocation_fixture(tick, tick + 500, sequence);
    acknowledge_join_bootstrap(&state, player_id);
    let (tx, rx) = mpsc::channel();
    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.tx = tx.into();
        client.last_position_relay_at = Some(Instant::now());
        client.position_relay_tokens = -100.0;
        client.outgoing_reliable.set_next_id_for_test(u32::MAX);
    }
    record_position_witness(&state, player_id, sequence, 7);
    state
        .lock()
        .expect("state")
        .queue_position_input(player_id, packet.clone());

    crate::server_tick::run_server_tick(&state, Instant::now());
    let first = rx
        .try_iter()
        .find(|candidate| {
            crate::reliable::reliable_parts(candidate).is_some_and(|(_, payload)| {
                payload.first() == Some(&crate::roster::WARP_RELOCATION_ACCEPTED)
            })
        })
        .expect("zero public tokens must not suppress a private Warp receipt");
    assert_eq!(
        crate::reliable::reliable_parts(&first),
        Some((
            u32::MAX,
            crate::roster::warp_relocation_accepted(42, identity.life_seq, sequence).as_slice(),
        ))
    );
    crate::reliable::resend_due_reliable_to_clients(
        &state,
        Instant::now() + Duration::from_secs(4),
    );
    assert!(
        rx.try_iter().any(|packet| packet == first),
        "a lost Warp receipt must be retried exactly"
    );
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable_ack(u32::MAX),
        "warp-app-receipt-peer",
        &mut joined,
    );

    let duplicate_sequence = sequence.wrapping_add(1);
    packet[28..32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(duplicate_sequence)
            .to_le_bytes(),
    );
    record_position_witness(&state, player_id, duplicate_sequence, 7);
    state
        .lock()
        .expect("state")
        .queue_position_input(player_id, packet);
    crate::server_tick::run_server_tick(&state, Instant::now());
    let duplicate = rx
        .try_iter()
        .find(|candidate| {
            crate::reliable::reliable_parts(candidate).is_some_and(|(_, payload)| {
                payload.first() == Some(&crate::roster::WARP_RELOCATION_ACCEPTED)
            })
        })
        .expect("a repeated exact marker must recover its application receipt");
    assert_eq!(
        crate::reliable::reliable_parts(&duplicate),
        Some((
            0,
            crate::roster::warp_relocation_accepted(42, identity.life_seq, duplicate_sequence,)
                .as_slice(),
        ))
    );
    let locked = state.lock().expect("state");
    assert!(locked.clients[&player_id].position_relay_tokens < 1.0);
    assert!(
        locked.clients[&player_id]
            .pending_warp_prize_relocations
            .is_empty()
    );
}

#[test]
fn rejected_lifecycle_baseline_never_emits_application_acceptance() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    update_ship_at_tick(&state, player_id, &[SHIP_CHANGE_REQUEST, 1, 100, 0], 10_001);
    drain_available(&rx);
    let (spawn_x, spawn_y, bounty, energy) = {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        let (x, y) = crate::zone::server_spawn_coordinates(state.zone_map());
        (x, y, client.server_bounty, client.server_energy)
    };
    let mut forged = client_position_packet_with_sequence(0, spawn_x + 32, spawn_y, 100);
    forged[16..18].copy_from_slice(&bounty.to_le_bytes());
    forged[18..20].copy_from_slice(&energy.to_le_bytes());
    forged.extend_from_slice(&build_input_record_packet(100, &[(0, 0, 0)]));
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable(0, &forged),
        "rejected-lifecycle-app-receipt-peer",
        &mut joined,
    );
    crate::server_tick::run_server_tick(&state, Instant::now());
    assert!(!rx.try_iter().any(|packet| {
        crate::reliable::reliable_payload(&packet).is_some_and(|payload| {
            payload.first() == Some(&crate::roster::LIFECYCLE_BASELINE_ACCEPTED)
        })
    }));
}

fn record_position_witness(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    sequence: u16,
    direction: u8,
) {
    let server_tick = crate::wire::current_tick();
    let (runtime, packet) = build_input_record_packet_with_valid_combat_view(
        state,
        player_id,
        sequence,
        &[(0, direction, 0)],
        server_tick,
    );
    let _ = crate::connection::record_client_input_observation_at_tick(
        state,
        runtime,
        &packet,
        server_tick,
    );
}

fn seed_exact_canonical_orientation_anchor(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    sequence: u16,
    tick: u32,
    x: u16,
    y: u16,
    orientation: f32,
) {
    let direction = crate::continuum_position::protocol_direction(orientation);
    let mut packet = client_position_packet_with_sequence(0, x, y, sequence);
    packet[1] = direction;
    let position = large_position_from_client(player_id, &packet).expect("canonical anchor");
    let input = crate::state::ClientInputObservation { bits: 0, direction };
    let mut locked = state.lock().expect("state");
    locked.set_client_position(player_id, position);
    let client = locked.clients.get_mut(&player_id).expect("client");
    client.last_position_sequence = Some(sequence);
    client.last_position_server_tick = Some(tick);
    client.last_input_seq = Some(sequence);
    client.last_input_record = Some(input);
    client.recent_input_records.clear();
    client
        .recent_input_records
        .push_back(crate::state::SequencedInputObservation {
            sequence,
            input,
            door_seed: 0,
            door_phase_tick: tick,
        });
    let witness_state = crate::state::ServerSimMovement {
        position_x: x as f32,
        position_y: y as f32,
        orientation,
        velocity_x: 0.0,
        velocity_y: 0.0,
        energy: client.server_energy as f32,
        max_energy: client.server_max_energy as f32,
    };
    let witness = crate::state::CanonicalPositionWitness {
        sequence,
        life_seq: client.life_seq,
        state: witness_state,
        wire_x: x,
        wire_y: y,
        wire_velocity_x: 0,
        wire_velocity_y: 0,
        direction,
        door_seed: 0,
        door_phase_tick: tick,
    };
    client.canonical_position_witnesses.clear();
    client.canonical_position_witnesses.push_back(witness);
    client.canonical_movement_witness = Some(witness);
    seed_fixture_motion_baseline(client);
    // Canonical replay must not borrow a same-bin private preference.
    client.server_sim_state = Some(crate::state::ServerSimMovement {
        orientation: direction as f32 / 40.0,
        ..witness.state
    });
    client.server_sim_anchor_tick = Some(tick);
    client.server_sim_input_bits = Some(0);
}

fn rotating_input_history(
    initial_orientation: f32,
    steps: usize,
    rotation_per_second: f32,
) -> (Vec<(u16, u8, u8)>, f32) {
    let mut orientation = initial_orientation;
    let mut records = Vec::with_capacity(steps);
    for _ in 0..steps {
        orientation = (orientation + rotation_per_second / 60.0).rem_euclid(1.0);
        records.push((
            1 << 3,
            crate::continuum_position::protocol_direction(orientation),
            0,
        ));
    }
    (records, orientation)
}

#[test]
fn missing_motion_clock_cannot_invent_a_baseline_or_partially_commit_admission() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    seed_exact_canonical_orientation_anchor(&state, player_id, 100, 1_000, 8_200, 8_200, 0.0);
    let runtime = state
        .lock()
        .unwrap()
        .active_player_runtime_identity(player_id)
        .unwrap();
    crate::connection::record_client_input_observation_at_tick(
        &state,
        runtime,
        &build_input_record_packet(101, &[(0, 0, 0)]),
        1_002,
    );
    // Deliberately interrupt history establishment without granting a life or
    // movement baseline. A valid next input cannot repair that missing proof.
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&player_id)
        .unwrap()
        .begin_actor_motion_segment();
    let before = witness_admission_state(&state, player_id);
    let mut packet = client_position_packet_with_sequence(0, 8_200, 8_200, 101);
    packet[1] = 0;
    set_client_position_canonical_state(&mut packet, 8_200.0, 8_200.0, 0.0, 0.0, 0.0);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(&state, runtime.life, &packet, 1_002,)
            .is_none()
    );
    assert_eq!(witness_admission_state(&state, player_id), before);
}

#[test]
fn overlapping_32_input_bundles_reconstruct_exact_64_step_history_across_wrap_and_reset() {
    for previous_sequence in [100_u16, u16::MAX - 32] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        seed_exact_canonical_orientation_anchor(
            &state,
            player_id,
            previous_sequence,
            80_000 - 107,
            8_200,
            8_200,
            0.0,
        );

        let first = previous_sequence.wrapping_add(1);
        let second = previous_sequence.wrapping_add(33);
        let neutral = vec![(0_u16, 0_u8, 0_u8); 32];
        let mut joined = true;
        crate::connection::handle_client_packet(
            &state,
            player_id,
            build_input_record_packet(first, &neutral),
            "rolling-history-peer",
            &mut joined,
        );
        crate::connection::handle_client_packet(
            &state,
            player_id,
            build_input_record_packet(second, &neutral),
            "rolling-history-peer",
            &mut joined,
        );

        let tail = previous_sequence.wrapping_add(64);
        {
            let locked = state.lock().expect("state");
            let client = &locked.clients[&player_id];
            assert_eq!(
                client.recent_input_records.len(),
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS
            );
            assert_eq!(client.recent_input_records.front().unwrap().sequence, first);
            assert_eq!(client.recent_input_records.back().unwrap().sequence, tail);
            assert_eq!(
                crate::sim_observation::first_missing_position_input_sequence(client, tail),
                None,
                "a complete64-step history has no missing input",
            );
            assert_eq!(
                crate::sim_observation::first_missing_position_input_sequence(
                    client,
                    tail.wrapping_add(1)
                ),
                Some(tail.wrapping_add(1)),
                "65 steps still require the next input beyond the finite retained interval",
            );
            assert_eq!(
                crate::sim_observation::validate_position_input_witness(client, tail, 0, 80_000,),
                Ok(crate::state::ClientInputObservation {
                    bits: 0,
                    direction: 0,
                }),
                "the full exact 64-step gap is admissible"
            );
            assert_eq!(
                crate::sim_observation::validate_position_input_witness(
                    client,
                    tail.wrapping_add(1),
                    0,
                    80_000,
                ),
                Err(crate::sim_observation::PositionInputWitnessError::Missing),
                "65 steps remain outside the finite replay/retention bound"
            );
        }
        let identity = state
            .lock()
            .expect("state")
            .active_life_identity(player_id)
            .expect("life");
        let mut exact_tail = client_position_packet_with_sequence(0, 8_200, 8_200, tail);
        exact_tail[1] = 0;
        set_client_position_canonical_state(&mut exact_tail, 8_200.0, 8_200.0, 0.0, 0.0, 0.0);
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &exact_tail,
                80_000,
            )
            .is_some(),
            "64 exact reconstructed steps must reach full position admission"
        );
        {
            let locked = state.lock().expect("state");
            let samples: Vec<_> = locked.clients[&player_id]
                .actor_motion_history
                .current_samples()
                .collect();
            assert_eq!(samples.len(), 65, "retain baseline plus every proven step");
            for (index, sample) in samples.iter().enumerate() {
                let quanta = index as u32 * 5;
                assert_eq!(
                    sample.sample_time.parts(),
                    (80_000 - 107 + quanta / 3, (quanta % 3) as u8)
                );
                assert_eq!(
                    sample.server_tick,
                    if index == 0 { 80_000 - 107 } else { 80_000 }
                );
                assert_eq!(sample.position(), (8_200.0, 8_200.0));
            }
        }
        let before_missing = witness_admission_state(&state, player_id);
        let mut beyond =
            client_position_packet_with_sequence(0, 8_200, 8_200, tail.wrapping_add(1));
        beyond[1] = 0;
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &beyond,
                80_001,
            )
            .is_none(),
            "65th step without an exact record rejects"
        );
        assert_eq!(witness_admission_state(&state, player_id), before_missing);

        // Additional overlapping bundles stay capacity-bounded, and a life
        // transition clears the reconstructed history rather than carrying
        // it into the next ship/life sequence namespace.
        crate::connection::handle_client_packet(
            &state,
            player_id,
            build_input_record_packet(tail.wrapping_add(1), &neutral),
            "rolling-history-peer",
            &mut joined,
        );
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            assert_eq!(
                client.recent_input_records.len(),
                crate::state::MAX_RECENT_INPUT_OBSERVATIONS
            );
            client.clear_life_scoped_observations();
            assert!(client.recent_input_records.is_empty());
            assert!(client.last_input_seq.is_none());
            assert!(client.last_position_sequence.is_none());
        }
    }
}

fn warp_relocation_fixture(
    accepted_tick: u32,
    _formerly_expires_at_tick: u32,
    sequence: u16,
) -> (
    Arc<Mutex<ServerState>>,
    u16,
    crate::authority_identity::LifeIdentity,
    Vec<u8>,
) {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let (identity, spawn_x, spawn_y, spawn_status, bounty, energy) = {
        let mut locked = state.lock().expect("state");
        let identity = locked.active_life_identity(player_id).expect("active life");
        let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(locked.zone_map());
        let in_safe =
            crate::zone::world_position_in_safe_tile_for_map(locked.zone_map(), spawn_x, spawn_y);
        let client = locked.clients.get_mut(&player_id).expect("client");
        let previous_sequence = sequence.wrapping_sub(1);
        let previous_x = spawn_x.saturating_add(32);
        let previous_y = spawn_y;
        let previous_orientation = 0.18125_f32;
        let previous_input = crate::state::ClientInputObservation {
            bits: 0,
            direction: 7,
        };
        let mut previous_packet =
            client_position_packet_with_sequence(0, previous_x, previous_y, previous_sequence);
        previous_packet[1] = 7;
        client.last_position = large_position_from_client(player_id, &previous_packet);
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(accepted_tick.wrapping_sub(1));
        client.last_input_seq = Some(previous_sequence);
        client.last_input_record = Some(previous_input);
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: previous_sequence,
                input: previous_input,
                door_seed: 0,
                door_phase_tick: 0,
            });
        let previous_state = crate::state::ServerSimMovement {
            position_x: previous_x as f32,
            position_y: previous_y as f32,
            orientation: previous_orientation,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let previous_witness = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: previous_state,
            wire_x: previous_x,
            wire_y: previous_y,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 7,
            door_seed: 0,
            door_phase_tick: 0,
        };
        client.canonical_movement_witness = Some(previous_witness);
        seed_fixture_motion_baseline(client);
        client
            .canonical_position_witnesses
            .push_back(previous_witness);
        client
            .pending_warp_prize_relocations
            .push_back(crate::state::PendingWarpPrizeRelocation {
                collectible_id: 42,
                life_seq: client.life_seq,
                expected_energy: client.server_energy,
                expected_bounty: client.server_bounty,
                prize_state_revision: client.server_prize_state_revision,
                claim: crate::state::ServerPrizeClaim {
                    collectible_id: 42,
                    x: 800,
                    y: 800,
                    prize_id: 7,
                },
                pickup_ack_sent: true,
            });
        client.server_sim_state = Some(crate::state::ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.175,
            velocity_x: 90.0,
            velocity_y: -45.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        });
        client.server_sim_anchor_tick = Some(crate::continuum::continuum_tick_add(
            accepted_tick,
            crate::continuum::CONTINUUM_TICK_MASK,
        ));
        client.server_sim_input_bits = Some(0);
        client.movement_drift_x = 19.0;
        client.movement_drift_y = -11.0;
        client
            .position_history
            .push_back(crate::state::RecentPositionSample {
                tick: accepted_tick.saturating_sub(2),
                x: spawn_x as f32,
                y: spawn_y as f32,
            });
        client
            .position_history
            .push_back(crate::state::RecentPositionSample {
                tick: accepted_tick.saturating_sub(1),
                x: spawn_x as f32,
                y: spawn_y as f32,
            });
        let status = client.server_status.position_status_bits(0, in_safe).bits();
        (
            identity,
            spawn_x,
            spawn_y,
            status,
            client.server_bounty,
            client.server_energy,
        )
    };
    let mut packet = client_position_packet_with_sequence(0, spawn_x, spawn_y, sequence);
    packet[1] = 7;
    packet[11] = spawn_status;
    packet[16..18].copy_from_slice(&bounty.to_le_bytes());
    packet[18..20].copy_from_slice(&energy.to_le_bytes());
    set_client_position_canonical_state(
        &mut packet,
        spawn_x as f32,
        spawn_y as f32,
        0.0,
        0.0,
        0.18125_f32,
    );
    (
        state,
        player_id,
        identity,
        append_warp_collectible_marker(packet, 42),
    )
}

#[test]
fn warp_relocation_spawn_mismatches_reject_before_authoritative_mutation() {
    let accepted_tick = 40_000;
    let expires_at_tick = accepted_tick + 500;
    for case in [
        "direction",
        "velocity_x",
        "velocity_y",
        "weapon",
        "togglables",
        "bounty",
        "repel_marker",
        "timestamp",
    ] {
        let sequence = 900;
        let (state, player_id, identity, mut packet) =
            warp_relocation_fixture(accepted_tick, expires_at_tick, sequence);
        let witness_direction = match case {
            "direction" => {
                packet[1] = 8;
                7
            }
            "velocity_x" => {
                packet[6..8].copy_from_slice(&1_i16.to_le_bytes());
                7
            }
            "velocity_y" => {
                packet[14..16].copy_from_slice(&(-1_i16).to_le_bytes());
                7
            }
            "weapon" => {
                packet[20..22].copy_from_slice(&1_u16.to_le_bytes());
                7
            }
            "togglables" => {
                packet[11] ^= 0x80;
                7
            }
            "bounty" => {
                let bounty = u16::from_le_bytes(packet[16..18].try_into().expect("bounty"));
                packet[16..18].copy_from_slice(&bounty.wrapping_add(1).to_le_bytes());
                7
            }
            "repel_marker" => {
                packet[24..26].copy_from_slice(&77_u16.to_le_bytes());
                packet[26..28].copy_from_slice(
                    &crate::continuum_position::CLIENT_POSITION_REPEL_MARKER_MAGIC.to_le_bytes(),
                );
                7
            }
            "timestamp" => {
                packet[2..6].copy_from_slice(&1_u32.to_le_bytes());
                7
            }
            _ => unreachable!(),
        };
        record_position_witness(&state, player_id, sequence, witness_direction);
        let before = witness_admission_state(&state, player_id);

        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &packet,
                accepted_tick,
            )
            .is_none(),
            "{case} must reject",
        );
        assert_eq!(
            witness_admission_state(&state, player_id),
            before,
            "{case} must not mutate energy, weapons/projectiles, input history, position, or movement tracking",
        );
    }
}

#[test]
fn warp_relocation_ignores_predicted_energy_and_stamps_server_energy() {
    let accepted_tick = 45_000;
    for (label, reported_energy, fractional_remainder) in [
        ("zero", 0_u16, 0.0_f32),
        ("lower", 799, 0.0),
        ("higher", 801, 0.0),
        ("fractional-remainder", 799, 0.625),
    ] {
        let sequence = 1_000;
        let (state, player_id, identity, mut packet) =
            warp_relocation_fixture(accepted_tick, accepted_tick + 500, sequence);
        {
            let mut locked = state.lock().expect("state");
            let client = locked.clients.get_mut(&player_id).expect("client");
            client.server_energy = 800;
            client.server_energy_remainder = fractional_remainder;
            client
                .pending_warp_prize_relocations
                .front_mut()
                .expect("Warp authorization")
                .expected_energy = 800;
            client.server_sim_state.as_mut().expect("sim state").energy = 800.0;
        }
        packet[18..20].copy_from_slice(&reported_energy.to_le_bytes());
        record_position_witness(&state, player_id, sequence, 7);
        let relay = crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            accepted_tick,
        )
        .unwrap_or_else(|| {
            panic!("{label} predicted-energy drift must not invalidate Warp authority")
        });
        assert_eq!(
            u16::from_le_bytes(relay.position[21..23].try_into().expect("energy")),
            800,
            "{label} relay must publish server energy",
        );
        {
            let locked = state.lock().expect("state");
            let client = &locked.clients[&player_id];
            assert_eq!(client.server_energy, 800, "{label}");
            assert_eq!(
                client.server_energy_remainder, fractional_remainder,
                "{label}"
            );
            assert!(
                client.pending_warp_prize_relocations.is_empty(),
                "{label} accepted relocation consumes once",
            );
        }
        let after_accept = witness_admission_state(&state, player_id);
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &packet,
                accepted_tick + 1,
            )
            .is_none(),
            "{label} replay rejects",
        );
        assert_eq!(witness_admission_state(&state, player_id), after_accept);
    }
}

#[test]
fn warp_relocation_accepts_current_bounty_after_a_later_ordered_prize_ack() {
    let accepted_tick = 47_500;
    let sequence = 1_100;
    let (state, player_id, identity, mut packet) =
        warp_relocation_fixture(accepted_tick, accepted_tick + 500, sequence);
    let current_bounty = {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        let authorization = client
            .pending_warp_prize_relocations
            .front()
            .copied()
            .expect("Warp authorization");
        client.server_bounty = authorization.expected_bounty.saturating_add(1);
        assert_ne!(client.server_bounty, authorization.expected_bounty);
        client.server_bounty
    };
    packet[16..18].copy_from_slice(&current_bounty.to_le_bytes());
    record_position_witness(&state, player_id, sequence, 7);

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            accepted_tick,
        )
        .is_some(),
        "a later reliable prize ACK may advance server-owned bounty before the Warp relocation is reported",
    );
    let locked = state.lock().expect("state");
    let client = &locked.clients[&player_id];
    assert!(client.pending_warp_prize_relocations.is_empty());
    assert_eq!(client.server_bounty, current_bounty);
}

#[test]
fn warp_relocation_ignores_old_position_then_consumes_exactly_once() {
    let accepted_tick = 50_000;
    let expires_at_tick = accepted_tick + 500;
    let sequence = 1_200;
    let (state, player_id, identity, exact) =
        warp_relocation_fixture(accepted_tick, expires_at_tick, sequence);

    let mut old_in_flight = exact.clone();
    old_in_flight.truncate(crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET);
    let old_x = u16::from_le_bytes(old_in_flight[12..14].try_into().expect("x")) + 32;
    let old_y = u16::from_le_bytes(old_in_flight[8..10].try_into().expect("y"));
    old_in_flight[12..14].copy_from_slice(&old_x.to_le_bytes());
    set_client_position_canonical_state(
        &mut old_in_flight,
        old_x as f32,
        old_y as f32,
        0.0,
        0.0,
        0.18125_f32,
    );
    record_position_witness(&state, player_id, sequence, 7);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &old_in_flight,
            accepted_tick,
        )
        .is_some(),
        "an ordinary pre-ACK position remains admissible",
    );
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert!(!client.pending_warp_prize_relocations.is_empty());
        assert_eq!(
            client
                .last_position
                .as_deref()
                .and_then(crate::arena::large_position_coordinates),
            Some((
                old_x,
                u16::from_le_bytes(exact[8..10].try_into().expect("y"))
            ))
        );
    }

    let exact_sequence = sequence + 1;
    let warp_correction =
        arm_snapback_transaction_for_discontinuity(&state, player_id, 0xa001, sequence);
    let mut exact = exact;
    exact[28..32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(exact_sequence).to_le_bytes(),
    );
    record_position_witness(&state, player_id, exact_sequence, 7);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &exact,
            accepted_tick + 1,
        )
        .is_some()
    );
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert!(client.pending_warp_prize_relocations.is_empty());
        assert_eq!(client.movement_drift_x, 0.0);
        assert_eq!(client.movement_drift_y, 0.0);
        assert_eq!(client.position_history.len(), 1);
        assert!(
            client
                .canonical_position_witnesses
                .iter()
                .any(|witness| witness.sequence == sequence),
            "movement rebasing must preserve prior canonical weapon witnesses"
        );
    }
    assert_snapback_transaction_cancelled(&state, player_id, warp_correction);

    let projectiles_before = state.lock().expect("state").active_projectiles.len();
    let mut delayed_weapon = old_in_flight;
    delayed_weapon[20..22].copy_from_slice(&1_u16.to_le_bytes());
    let mut forged = delayed_weapon.clone();
    forged[12..14].copy_from_slice(&old_x.saturating_add(1).to_le_bytes());
    set_client_position_canonical_state(
        &mut forged,
        old_x.saturating_add(1) as f32,
        old_y as f32,
        0.0,
        0.0,
        0.18125_f32,
    );
    let mismatch_before =
        crate::security::peek_security_reject_count_for_test("weapon", "mirror_witness_mismatch");
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &forged,
            accepted_tick + 2,
        )
        .is_none()
    );
    assert_eq!(
        state.lock().expect("state").active_projectiles.len(),
        projectiles_before,
        "a forged delayed mirror cannot spawn after relocation"
    );
    assert!(
        crate::security::peek_security_reject_count_for_test("weapon", "mirror_witness_mismatch")
            >= mismatch_before + 1
    );

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &delayed_weapon,
            accepted_tick + 3,
        )
        .is_none(),
        "a stale weapon mirror has a weapon-only side effect, never movement relay"
    );
    {
        let locked = state.lock().expect("state");
        assert_eq!(locked.active_projectiles.len(), projectiles_before + 1);
        let projectile = locked.active_projectiles.last().expect("projectile");
        assert_eq!((projectile.x, projectile.y), (old_x, old_y));
    }
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &delayed_weapon,
            accepted_tick + 4,
        )
        .is_none()
    );
    assert_eq!(
        state.lock().expect("state").active_projectiles.len(),
        projectiles_before + 1,
        "the delayed mirror sequence spawns exactly once"
    );

    let after_exact = witness_admission_state(&state, player_id);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &exact,
            accepted_tick + 5,
        )
        .is_none(),
        "duplicate exact frame must replay-drop",
    );
    assert_eq!(witness_admission_state(&state, player_id), after_exact);

    let later_sequence = exact_sequence + 1;
    exact[28..32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(later_sequence).to_le_bytes(),
    );
    record_position_witness(&state, player_id, later_sequence, 7);
    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.movement_drift_x = 23.0;
        client.movement_drift_y = -17.0;
    }
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &exact,
            accepted_tick + 6,
        )
        .is_some()
    );
    let locked = state.lock().expect("state");
    let client = &locked.clients[&player_id];
    assert!(client.pending_warp_prize_relocations.is_empty());
    assert_eq!(
        client.position_history.len(),
        2,
        "a later ordinary spawn report must append normally instead of invoking the one-shot Warp history reset",
    );
}

#[test]
fn warp_marker_is_required_after_stationary_spawn_and_forgery_cannot_consume_it() {
    let accepted_tick = 55_000;
    let sequence = 1_350;
    let (state, player_id, identity, mut exact) =
        warp_relocation_fixture(accepted_tick, accepted_tick + 500, sequence);
    let (spawn_x, spawn_y) = {
        let locked = state.lock().expect("state");
        server_spawn_coordinates(locked.zone_map())
    };
    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        let previous_sequence = sequence.wrapping_sub(1);
        let mut previous =
            client_position_packet_with_sequence(0, spawn_x, spawn_y, previous_sequence);
        previous[1] = 7;
        client.last_position = large_position_from_client(player_id, &previous);
        client.last_position_sequence = Some(previous_sequence);
        for witness in &mut client.canonical_position_witnesses {
            witness.state.position_x = spawn_x as f32;
            witness.state.position_y = spawn_y as f32;
            witness.wire_x = spawn_x;
            witness.wire_y = spawn_y;
        }
        let witness = client
            .canonical_movement_witness
            .as_mut()
            .expect("canonical witness");
        witness.state.position_x = spawn_x as f32;
        witness.state.position_y = spawn_y as f32;
        witness.wire_x = spawn_x;
        witness.wire_y = spawn_y;
    }

    let mut ordinary_spawn = exact.clone();
    ordinary_spawn.truncate(crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET);
    record_position_witness(&state, player_id, sequence, 7);
    let ordinary = crate::arena::accept_position_relay_for_life_at_tick(
        &state,
        identity,
        &ordinary_spawn,
        accepted_tick,
    )
    .expect("ordinary stationary spawn must remain admissible");
    assert_eq!(ordinary.warp_relocation_receipt, None);
    assert_eq!(
        state.lock().expect("state").clients[&player_id]
            .pending_warp_prize_relocations
            .front()
            .map(|authorization| authorization.collectible_id),
        Some(42),
        "shared stationary spawn fields alone cannot consume Warp authority",
    );

    let exact_sequence = sequence.wrapping_add(1);
    exact[28..32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(exact_sequence).to_le_bytes(),
    );
    record_position_witness(&state, player_id, exact_sequence, 7);
    let mut forged = exact.clone();
    let marker_id_offset = forged.len() - std::mem::size_of::<u32>();
    forged[marker_id_offset..].copy_from_slice(&43_u32.to_le_bytes());
    let forged_before = crate::security::peek_security_reject_count_for_test(
        "position",
        "invalid_warp_prize_marker",
    );
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &forged,
            accepted_tick + 1,
        )
        .is_none(),
        "a forged collectible marker must fail closed",
    );
    assert_eq!(
        crate::security::peek_security_reject_count_for_test(
            "position",
            "invalid_warp_prize_marker"
        ),
        forged_before + 1,
    );
    assert_eq!(
        state.lock().expect("state").clients[&player_id]
            .pending_warp_prize_relocations
            .front()
            .map(|authorization| authorization.collectible_id),
        Some(42),
        "a forged marker cannot consume the exact outstanding authority",
    );

    let accepted = crate::arena::accept_position_relay_for_life_at_tick(
        &state,
        identity,
        &exact,
        accepted_tick + 1,
    )
    .expect("the exact marker remains admissible after the forgery");
    assert_eq!(
        accepted.warp_relocation_receipt,
        Some((42, identity.life_seq, exact_sequence)),
    );
    assert!(
        state.lock().expect("state").clients[&player_id]
            .pending_warp_prize_relocations
            .is_empty()
    );

    let duplicate_sequence = exact_sequence.wrapping_add(1);
    exact[28..32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(duplicate_sequence)
            .to_le_bytes(),
    );
    record_position_witness(&state, player_id, duplicate_sequence, 7);
    let duplicate = crate::arena::accept_position_relay_for_life_at_tick(
        &state,
        identity,
        &exact,
        accepted_tick + 2,
    )
    .expect("a repeated consumed marker is an idempotent ordinary position");
    assert_eq!(
        duplicate.warp_relocation_receipt,
        Some((42, identity.life_seq, duplicate_sequence)),
        "the exact duplicate receipt lets a client recover a lost app acknowledgement",
    );
    assert!(
        state.lock().expect("state").clients[&player_id]
            .pending_warp_prize_relocations
            .is_empty()
    );
}

#[test]
fn warp_discontinuity_preserves_exact_orientation_across_wrap_and_long_gap() {
    let accepted_tick = 57_000;
    let anchor_sequence = u16::MAX - 5;
    let gap_steps = 12_usize;
    let warp_sequence = anchor_sequence.wrapping_add(gap_steps as u16);
    let (state, player_id, identity, mut warp_packet) =
        warp_relocation_fixture(accepted_tick, accepted_tick + 500, warp_sequence);
    let (spawn_x, spawn_y, rotation_per_second, runtime) = {
        let locked = state.lock().expect("state");
        let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(locked.zone_map());
        let client = &locked.clients[&player_id];
        (
            spawn_x,
            spawn_y,
            client.server_rotation_per_second,
            locked
                .active_player_runtime_identity(player_id)
                .expect("runtime"),
        )
    };
    let initial_orientation = 0.987_f32;
    seed_exact_canonical_orientation_anchor(
        &state,
        player_id,
        anchor_sequence,
        accepted_tick - 20,
        spawn_x + 32,
        spawn_y,
        initial_orientation,
    );
    let (records, warp_orientation) =
        rotating_input_history(initial_orientation, gap_steps, rotation_per_second);
    assert_eq!(
        crate::continuum_position::protocol_direction(initial_orientation),
        39
    );
    assert!(records.iter().any(|record| record.1 == 0));
    let (_, input_packet) = build_input_record_packet_with_valid_combat_view(
        &state,
        player_id,
        anchor_sequence.wrapping_add(1),
        &records,
        accepted_tick,
    );
    assert!(crate::connection::record_client_input_observation_at_tick(
        &state,
        runtime,
        &input_packet,
        accepted_tick,
    ));
    let warp_direction = crate::continuum_position::protocol_direction(warp_orientation);
    warp_packet[1] = warp_direction;
    set_client_position_canonical_state(
        &mut warp_packet,
        spawn_x as f32,
        spawn_y as f32,
        0.0,
        0.0,
        warp_orientation,
    );

    let mut forged_heading = warp_packet.clone();
    forged_heading[1] = (warp_direction + 1) % 40;
    let before_forged = witness_admission_state(&state, player_id);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &forged_heading,
            accepted_tick,
        )
        .is_none()
    );
    assert_eq!(witness_admission_state(&state, player_id), before_forged);

    let mut forged_origin = warp_packet.clone();
    forged_origin[12..14].copy_from_slice(&spawn_x.saturating_add(1_000).to_le_bytes());
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &forged_origin,
            accepted_tick,
        )
        .is_none()
    );
    assert_eq!(witness_admission_state(&state, player_id), before_forged);

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &warp_packet,
            accepted_tick,
        )
        .is_some()
    );
    {
        let locked = state.lock().expect("state");
        let witness = locked.clients[&player_id]
            .canonical_movement_witness
            .expect("Warp tail witness");
        assert_eq!(witness.state.orientation, warp_orientation);
        assert_eq!(witness.state.orientation, warp_orientation);
        assert_eq!((witness.wire_x, witness.wire_y), (spawn_x, spawn_y));
        assert_eq!((witness.wire_velocity_x, witness.wire_velocity_y), (0, 0));
    }

    let follow_steps = (1_usize..=20)
        .find(|steps| {
            let exact =
                (warp_orientation + *steps as f32 * rotation_per_second / 60.0).rem_euclid(1.0);
            let midpoint = (warp_direction as f32 / 40.0
                + *steps as f32 * rotation_per_second / 60.0)
                .rem_euclid(1.0);
            crate::continuum_position::protocol_direction(exact)
                != crate::continuum_position::protocol_direction(midpoint)
        })
        .expect("sub-bin provenance must diverge from midpoint within 20 turn steps");
    let follow_tick = accepted_tick + (follow_steps as u32 * 5).div_ceil(3);
    let follow_first = warp_sequence.wrapping_add(1);
    let follow_tail = warp_sequence.wrapping_add(follow_steps as u16);
    let (follow_records, follow_orientation) =
        rotating_input_history(warp_orientation, follow_steps, rotation_per_second);
    let midpoint_follow_orientation = (warp_direction as f32 / 40.0
        + follow_steps as f32 * rotation_per_second / 60.0)
        .rem_euclid(1.0);
    assert_ne!(
        crate::continuum_position::protocol_direction(midpoint_follow_orientation),
        crate::continuum_position::protocol_direction(follow_orientation),
        "this fixture must distinguish exact provenance from a direction-bin midpoint reseed",
    );
    let (_, follow_input_packet) = build_input_record_packet_with_valid_combat_view(
        &state,
        player_id,
        follow_first,
        &follow_records,
        follow_tick,
    );
    assert!(crate::connection::record_client_input_observation_at_tick(
        &state,
        runtime,
        &follow_input_packet,
        follow_tick,
    ));
    let mut projectile_packet = warp_packet;
    projectile_packet[1] = crate::continuum_position::protocol_direction(follow_orientation);
    projectile_packet[20..22].copy_from_slice(&1_u16.to_le_bytes());
    projectile_packet[28..32].copy_from_slice(
        &crate::continuum_position::zubspace_position_sequence_items(follow_tail).to_le_bytes(),
    );
    set_client_position_canonical_state(
        &mut projectile_packet,
        spawn_x as f32,
        spawn_y as f32,
        0.0,
        0.0,
        follow_orientation,
    );
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &projectile_packet,
            follow_tick,
        )
        .is_some()
    );
    let locked = state.lock().expect("state");
    let witness = locked.clients[&player_id]
        .canonical_movement_witness
        .expect("post-Warp witness");
    assert_eq!(witness.state.orientation, follow_orientation);
    assert_eq!(witness.state.orientation, follow_orientation);
    let projectile = locked.active_projectiles.last().expect("projectile");
    assert_eq!((projectile.x, projectile.y), (spawn_x, spawn_y));
    let expected_projectile = crate::gameplay_rules::initialize_projectiles_for_protocol_direction(
        1,
        crate::gameplay_rules::RulesVector::new(spawn_x as f32, spawn_y as f32),
        crate::gameplay_rules::RulesVector::new(0.0, 0.0),
        crate::continuum_position::protocol_direction(follow_orientation),
        crate::arena_settings::arena_projectile_initialization_tuning(0),
    )
    .into_iter()
    .next()
    .expect("Warbird bullet initialization");
    assert_eq!(
        (projectile.velocity_x, projectile.velocity_y),
        (
            crate::continuum_position::protocol_velocity_component(expected_projectile.velocity.x,),
            crate::continuum_position::protocol_velocity_component(expected_projectile.velocity.y,),
        ),
        "the first post-Warp projectile must use the heading evolved from the exact canonical orientation",
    );
}

#[test]
fn warp_relocation_persists_past_old_expiry_and_is_life_scoped() {
    for (label, accepted_tick) in [
        ("over five seconds and three retry windows", 61_501),
        ("after u31 wrap", 1_100),
    ] {
        let sequence = 1_500;
        let (state, player_id, identity, packet) =
            warp_relocation_fixture(accepted_tick, accepted_tick.wrapping_sub(1_501), sequence);
        record_position_witness(&state, player_id, sequence, 7);
        assert!(
            crate::arena::accept_position_relay_for_life_at_tick(
                &state,
                identity,
                &packet,
                accepted_tick,
            )
            .is_some(),
            "{label}",
        );
        let client = &state.lock().expect("state").clients[&player_id];
        assert!(client.pending_warp_prize_relocations.is_empty(), "{label}");
        assert_eq!(client.movement_drift_x, 0.0, "{label}");
        assert_eq!(client.movement_drift_y, 0.0, "{label}");
        assert_eq!(client.position_history.len(), 1, "{label}");
    }

    let accepted_tick = 70_000;
    let (state, player_id, identity, packet) =
        warp_relocation_fixture(accepted_tick, accepted_tick + 500, 1_600);
    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.life_seq = client.life_seq.wrapping_add(1);
    }
    record_position_witness(&state, player_id, 1_600, 7);
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            accepted_tick,
        )
        .is_none(),
        "the captured source life identity is stale",
    );
    assert!(
        !state.lock().expect("state").clients[&player_id]
            .pending_warp_prize_relocations
            .is_empty(),
        "a stale async callback cannot mutate the new life"
    );
    state
        .lock()
        .expect("state")
        .clients
        .get_mut(&player_id)
        .expect("client")
        .clear_life_scoped_observations();
    assert!(
        state.lock().expect("state").clients[&player_id]
            .pending_warp_prize_relocations
            .is_empty(),
        "life/zone runtime reset clears the authorization"
    );
}

#[test]
fn position_witness_rejects_range_and_exact_direction_mismatch_atomically() {
    for (snapshot_direction, input_direction, reason) in [
        (0, None, "missing_input_record"),
        (40, Some(0), "malformed"),
        (1, Some(0), "input_direction_mismatch"),
    ] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let sequence = 500;
        if let Some(input_direction) = input_direction {
            record_position_witness(&state, player_id, sequence, input_direction);
        }
        {
            let mut state = state.lock().expect("server state should not be poisoned");
            let client = state
                .clients
                .get_mut(&player_id)
                .expect("client should exist");
            client.server_energy = client.server_max_energy;
            client.movement_drift_x = 12.5;
            client.movement_drift_y = -7.25;
        }
        let mut packet = client_position_packet_with_sequence(0, 8200, 8200, sequence);
        packet[1] = snapshot_direction;
        packet[20..22].copy_from_slice(&1_u16.to_le_bytes());
        if snapshot_direction < 40 {
            sync_client_position_canonical_state_from_wire(&mut packet);
        }
        let before = witness_admission_state(&state, player_id);
        let count_before = crate::security::peek_security_reject_count_for_test("position", reason);

        assert!(accept_position_relay(&state, player_id, &packet).is_none());

        assert_eq!(witness_admission_state(&state, player_id), before);
        let count_after = crate::security::peek_security_reject_count_for_test("position", reason);
        assert!(
            count_after >= count_before + 1,
            "the exact reject reason must increment at least once: {count_before} -> {count_after}"
        );
    }
}

#[test]
fn impossible_complete_direction_history_rejects_before_any_authoritative_mutation() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let previous_sequence = 100;
    let accepted_tick = 20_000;
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state
            .clients
            .get_mut(&player_id)
            .expect("client should exist");
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(accepted_tick - 32);
        let exact_anchor = crate::state::ServerSimMovement {
            position_x: 8_200.0,
            position_y: 8_200.0,
            orientation: 0.987_49,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let witness = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: exact_anchor,
            wire_x: 8_200,
            wire_y: 8_200,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: crate::continuum_position::protocol_direction(exact_anchor.orientation),
            door_seed: 0,
            door_phase_tick: 0,
        };
        client.canonical_movement_witness = Some(witness);
        seed_fixture_motion_baseline(client);
        client.canonical_position_witnesses.push_back(witness);
        client.server_sim_state = Some(crate::state::ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.987_49,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        });
        client.server_sim_anchor_tick = Some(accepted_tick - 32);
        client.server_sim_input_bits = Some(0);
        client.movement_drift_x = 19.0;
        client.movement_drift_y = -3.0;
        for offset in 1..=32_u16 {
            let input = crate::state::ClientInputObservation {
                bits: 0,
                direction: ((38 + offset) % 40) as u8,
            };
            let sequence = previous_sequence.wrapping_add(offset);
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence,
                    input,
                    door_seed: 0,
                    door_phase_tick: 0,
                });
            client.last_input_seq = Some(sequence);
            client.last_input_record = Some(input);
        }
    }
    let sequence = previous_sequence + 32;
    let mut packet = client_position_packet_with_sequence(0, 8200, 8200, sequence);
    packet[1] = 30;
    packet[20..22].copy_from_slice(&1_u16.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut packet);
    let before = witness_admission_state(&state, player_id);
    let count_before = crate::security::peek_security_reject_count_for_test(
        "position",
        "invalid_direction_history",
    );
    let identity = state
        .lock()
        .expect("server state")
        .active_life_identity(player_id)
        .expect("life");

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            accepted_tick,
        )
        .is_none()
    );

    assert_eq!(witness_admission_state(&state, player_id), before);
    assert!(
        crate::security::peek_security_reject_count_for_test(
            "position",
            "invalid_direction_history"
        ) >= count_before + 1
    );
}

#[test]
fn attacker_selected_adjacent_sub_bin_rejects_weapon_before_projectile_spawn() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let previous_sequence = 100_u16;
    let sequence = previous_sequence + 1;
    let accepted_tick = 20_000;
    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        let mut previous = client_position_packet_with_sequence(0, 8_200, 8_200, previous_sequence);
        previous[1] = 39;
        set_client_position_canonical_state(&mut previous, 8_200.0, 8_200.0, 0.0, 0.0, 0.98);
        client.last_position =
            Some(large_position_from_client(player_id, &previous).expect("previous position"));
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(accepted_tick - 1);
        let movement = crate::state::ServerSimMovement {
            position_x: 8_200.0,
            position_y: 8_200.0,
            orientation: 0.98,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let witness = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: movement,
            wire_x: 8_200,
            wire_y: 8_200,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 39,
            door_seed: 0,
            door_phase_tick: 0,
        };
        client.canonical_position_witnesses.push_back(witness);
        client.canonical_movement_witness = Some(witness);
        seed_fixture_motion_baseline(client);
        client.server_sim_state = Some(movement);
        client.server_sim_anchor_tick = Some(accepted_tick - 1);
        client.server_sim_input_bits = Some(0);
        let attack = crate::state::ClientInputObservation {
            bits: 8,
            direction: 1,
        };
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence,
                input: attack,
                door_seed: 0,
                door_phase_tick: 0,
            });
        client.last_input_seq = Some(sequence);
        client.last_input_record = Some(attack);
    }
    let mut packet = client_position_packet_with_sequence(0, 8_200, 8_200, sequence);
    packet[1] = 1;
    packet[20..22].copy_from_slice(&1_u16.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut packet);
    let before = witness_admission_state(&state, player_id);
    let count_before = crate::security::peek_security_reject_count_for_test(
        "position",
        "invalid_direction_history",
    );
    let identity = state
        .lock()
        .expect("state")
        .active_life_identity(player_id)
        .expect("life");

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            accepted_tick,
        )
        .is_none()
    );
    assert_eq!(witness_admission_state(&state, player_id), before);
    assert_eq!(
        crate::security::peek_security_reject_count_for_test(
            "position",
            "invalid_direction_history"
        ),
        count_before + 1
    );
    assert!(state.lock().expect("state").active_projectiles.is_empty());
}

#[test]
fn canonical_rejection_retains_one_structured_first_tail_difference_without_changing_authority() {
    #[derive(Clone)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player = register_connection(&state, tx);
    join_arena(&state, player, 0);
    consume_pending_lifecycle_baseline(&state, player);
    let sequence = 77;
    let tick = crate::wire::current_tick();
    seed_exact_canonical_orientation_anchor(
        &state,
        player,
        sequence - 1,
        tick - 1,
        8200,
        8200,
        0.25,
    );
    record_position_witness(&state, player, sequence, 10);
    let mut packet = client_position_packet_with_sequence(0, 8200, 8200, sequence);
    packet[1] = 10;
    set_client_position_canonical_state(&mut packet, 8200.125, 8200.0, 0.0, 0.0, 0.25);
    let identity = state.lock().unwrap().active_life_identity(player).unwrap();
    let before = witness_admission_state(&state, player);
    let buffer = LogBuffer(Arc::new(Mutex::new(Vec::new())));
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..3 {
            assert!(
                crate::arena::accept_position_relay_for_life_at_tick(
                    &state, identity, &packet, tick
                )
                .is_none()
            );
        }
    });
    assert_eq!(witness_admission_state(&state, player), before);
    assert!(state.lock().unwrap().active_projectiles.is_empty());
    let raw = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    let details = raw
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|row| row["fields"]["event"] == "canonical_mismatch_detail")
        .collect::<Vec<_>>();
    assert_eq!(
        details.len(),
        1,
        "a repeated invalid tail cannot flood detailed logging: {raw}"
    );
    let fields = &details[0]["fields"];
    assert_eq!(details[0]["level"], "WARN");
    assert_eq!(fields["first_differing_tail_field"], "position_x");
    assert_eq!(fields["reported_x_bits"], 8200.125_f32.to_bits());
    assert_eq!(fields["expected_x_bits"], 8200_f32.to_bits());
    assert_eq!(fields["client_sequence"], sequence);
    assert_eq!(fields["life_seq"], identity.life_seq);
    assert_eq!(fields["server_tick"], tick);
}

#[test]
fn matching_exact_witness_is_canonical_for_position_and_projectile() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let sequence = 77;
    let direction = 10;
    seed_exact_canonical_orientation_anchor(&state, player_id, sequence - 1, 0, 8200, 8200, 0.25);
    record_position_witness(&state, player_id, sequence, direction);
    let mut packet = client_position_packet_with_sequence(0, 8200, 8200, sequence);
    packet[1] = direction;
    packet[20..22].copy_from_slice(&1_u16.to_le_bytes());
    set_client_position_canonical_state(&mut packet, 8200.0, 8200.0, 0.0, 0.0, 0.25);

    let relay = accept_position_relay(&state, player_id, &packet)
        .expect("matching exact witness should be accepted");
    assert_eq!(relay.position[1], direction);
    let state = state.lock().expect("server state should not be poisoned");
    let projectile = state
        .active_projectiles
        .last()
        .expect("matching weapon witness should spawn a projectile");
    let binding = projectile
        .combat_timeline
        .expect("accepted human projectile must freeze its combat timeline");
    assert_eq!(binding.owner_input_sequence, sequence);
    assert_eq!(
        binding.owner,
        state.active_player_runtime_identity(player_id).unwrap()
    );
    assert!(
        projectile.velocity_x > projectile.velocity_y.abs(),
        "direction 10 is east and must be canonical for projectile initialization"
    );
}

#[test]
fn unavailable_combat_view_strips_weapon_without_discarding_movement() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let sequence = 177;
    let direction = 10;
    let tick = crate::wire::current_tick();
    seed_exact_canonical_orientation_anchor(
        &state,
        player_id,
        sequence - 1,
        tick,
        8_200,
        8_200,
        0.25,
    );
    let runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(player_id)
        .expect("runtime");
    assert!(crate::connection::record_client_input_observation_at_tick(
        &state,
        runtime,
        &build_input_record_packet(sequence, &[(0, direction, 0)]),
        tick,
    ));
    assert_eq!(
        state.lock().expect("state").clients[&player_id]
            .observer_combat_timeline
            .decision_for_sequence(sequence),
        None
    );

    let mut packet = client_position_packet_with_sequence(0, 8_200, 8_200, sequence);
    packet[1] = direction;
    packet[20..22].copy_from_slice(&1_u16.to_le_bytes());
    set_client_position_canonical_state(&mut packet, 8_200.0, 8_200.0, 0.0, 0.0, 0.25);
    let (_, relay) =
        crate::arena::accept_position_relay_for_runtime_at_tick(&state, runtime, &packet, tick)
            .expect("missing combat proof must not discard valid movement");
    assert_eq!(
        u16::from_le_bytes(relay.position[19..21].try_into().unwrap()),
        0
    );
    let locked = state.lock().expect("state");
    assert_eq!(
        locked.clients[&player_id].last_position_sequence,
        Some(sequence)
    );
    assert!(locked.active_projectiles.is_empty());
}

fn seed_authoritative_repel_position(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    frequency: u16,
    x: u16,
    y: u16,
    sequence: u16,
    tick: u32,
) {
    let mut packet = client_position_packet_with_sequence(0, x, y, sequence);
    packet[1] = 0;
    let position = large_position_from_client(player_id, &packet)
        .expect("repel fixture position should translate");
    let input = crate::state::ClientInputObservation {
        bits: 0,
        direction: 0,
    };
    let mut state = state.lock().expect("server state should not be poisoned");
    state.set_client_position(player_id, position);
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("repel fixture client should exist");
    client.player.frequency = frequency;
    client.last_position_sequence = Some(sequence);
    client.last_position_server_tick = Some(tick);
    client.last_input_seq = Some(sequence);
    client.last_input_record = Some(input);
    client.recent_input_records.clear();
    client
        .recent_input_records
        .push_back(crate::state::SequencedInputObservation {
            sequence,
            input,
            door_seed: 0,
            door_phase_tick: 0,
        });
    let exact_orientation = 0.01_f32;
    let exact_state = crate::state::ServerSimMovement {
        position_x: x as f32,
        position_y: y as f32,
        orientation: exact_orientation,
        velocity_x: 0.0,
        velocity_y: 0.0,
        energy: client.server_energy as f32,
        max_energy: client.server_max_energy as f32,
    };
    let exact_witness = crate::state::CanonicalPositionWitness {
        sequence,
        life_seq: client.life_seq,
        state: exact_state,
        wire_x: x,
        wire_y: y,
        wire_velocity_x: 0,
        wire_velocity_y: 0,
        direction: 0,
        door_seed: 0,
        door_phase_tick: 0,
    };
    client.canonical_movement_witness = Some(exact_witness);
    seed_fixture_motion_baseline(client);
    client.canonical_position_witnesses.push_back(exact_witness);
    client.server_sim_state = Some(crate::state::ServerSimMovement {
        position_x: f32::NAN,
        position_y: f32::NAN,
        orientation: 0.0,
        velocity_x: 0.0,
        velocity_y: 0.0,
        energy: client.server_energy as f32,
        max_energy: client.server_max_energy as f32,
    });
    client.server_sim_anchor_tick = Some(tick);
    client.server_sim_input_bits = Some(0);
}

fn record_neutral_repel_input(state: &Arc<Mutex<ServerState>>, player_id: u16, sequence: u16) {
    let server_tick = state
        .lock()
        .expect("server state should not be poisoned")
        .clients
        .get(&player_id)
        .and_then(|client| client.last_position_server_tick)
        .map_or_else(crate::wire::current_tick, |tick| {
            crate::continuum::continuum_tick_add(tick, 1)
        });
    let (runtime, packet) = build_input_record_packet_with_valid_combat_view(
        state,
        player_id,
        sequence,
        &[(0, 0, 0)],
        server_tick,
    );
    assert!(crate::connection::record_client_input_observation_at_tick(
        state,
        runtime,
        &packet,
        server_tick,
    ));
}

fn fire_authoritative_repel_at_tick(
    state: &Arc<Mutex<ServerState>>,
    source_player_id: u16,
    x: u16,
    y: u16,
    sequence: u16,
    tick: u32,
) {
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let source = state
            .clients
            .get_mut(&source_player_id)
            .expect("repel source should exist");
        source.server_energy = source.server_max_energy;
        source.server_weapons.repels = source.server_weapons.repels.max(1);
        source.last_weapon_fire_tick = None;
        source.last_bomb_fire_tick = None;
        source.last_special_weapon_request = None;
    }
    record_neutral_repel_input(state, source_player_id, sequence);
    let mut packet = client_position_packet_with_sequence(0, x, y, sequence);
    packet[1] = 0;
    packet[20..22].copy_from_slice(&5_u16.to_le_bytes());
    set_client_position_canonical_state(&mut packet, x as f32, y as f32, 0.0, 0.0, 0.01);
    let identity = state
        .lock()
        .expect("server state should not be poisoned")
        .active_life_identity(source_player_id)
        .expect("repel source life should exist");
    let relay =
        crate::arena::accept_position_relay_for_life_at_tick(state, identity, &packet, tick)
            .expect("authoritative repel position should be accepted");
    assert_eq!(
        u16::from_le_bytes(relay.position[19..21].try_into().unwrap()),
        5,
        "fixture must exercise an accepted repel, not merely a reported one"
    );
}

fn assert_repel_mutates_captured_bot_projectile(retired_life: bool, weapon_data: u16) {
    let state = Arc::new(Mutex::new(ServerState {
        config: crate::config::ServerConfig {
            bot_count: 1,
            ..crate::config::ServerConfig::default()
        },
        ..ServerState::default()
    }));
    state.lock().expect("state").initialize_configured_bots();
    let (source_tx, _source_rx) = mpsc::channel();
    let source = register_connection(&state, source_tx);
    join_arena(&state, source, 0);
    let tick = 24_000;
    seed_authoritative_repel_position(&state, source, 0, 500, 700, 10, tick - 1);
    let (projectile_id, captured) = {
        let mut locked = state.lock().expect("state");
        let projectile_id = locked.allocate_projectile_id();
        let shot_group_id = locked.allocate_projectile_group_id().expect("group");
        let bot = &mut locked.bots[0];
        bot.player.frequency = 1;
        bot.x = 600.0;
        bot.y = 700.0;
        let captured = crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: bot.player.id,
            source_session_id: bot.session_id.get(),
            source_life_seq: bot.life_seq,
            shot_group_id,
            x: 600.0,
            y: 700.0,
            velocity_x: -100.0,
            velocity_y: 0.0,
            orientation: 0.75,
            frequency: 1,
            ship: 6, // Lancaster: old-life mine conversion retains this ship.
            weapon_data,
        };
        if retired_life {
            // Committed flight keeps its original identity and frequency even
            // after the owner respawns on the Repel user's frequency.
            bot.life_seq += 1;
            bot.player.frequency = 0;
            bot.player.ship = 0;
        }
        locked.active_projectiles.record(
            ServerProjectile {
                id: projectile_id,
                shot_group_id,
                shot_index: 0,
                owner_id: captured.source_player_id,
                owner_life_seq: captured.source_life_seq,
                combat_timeline: None,
                presentation_source: Some(captured),
                weapon_data,
                x: 600,
                y: 700,
                timestamp: tick - 5,
                created_tick: tick - 5,
                velocity_x: -1000,
                velocity_y: 0,
                expiry_tick: Some(tick + 100),
                pos_x: 600.0,
                pos_y: 700.0,
                source_server_tick: Some(tick - 5),
                precise_velocity: Some(crate::gameplay_rules::RulesVector::new(-100.0, 0.0)),
                bounces_remaining: 0,
                behavior: crate::gameplay_rules::ProjectileBehavior::Standard,
                proximity_target: None,
                proximity_sensor_end_tick: None,
                proximity_highest_offset: 0.0,
            },
            128,
            1024,
        );
        (projectile_id, captured)
    };

    fire_authoritative_repel_at_tick(&state, source, 500, 700, 11, tick);

    let locked = state.lock().expect("state");
    let projectile = locked
        .active_projectiles
        .iter()
        .find(|projectile| projectile.id == projectile_id)
        .expect("surviving shot");
    assert!(
        (projectile.motion_velocity().x - 500.0).abs() < 0.01,
        "accepted Repel must mutate bot-owned flight regardless of current owner life"
    );
    assert!(projectile.motion_velocity().y.abs() < 0.01);
    assert_eq!(
        projectile.expiry_tick,
        crate::arena_settings::arena_weapon_lifetime_ticks(weapon_data & !0x8000)
            .map(|lifetime| crate::continuum::continuum_tick_add(tick, lifetime))
    );
    let mutations = locked.clients[&source]
        .pending_realtime_events
        .queue
        .iter()
        .filter_map(|event| match &event.payload {
            crate::realtime_event::RealtimeEventPayload::RepelMutations(batch) => {
                Some(batch.entries())
            }
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    let mutation = mutations
        .iter()
        .find(|mutation| {
            mutation.source_player_id == captured.source_player_id
                && mutation.source_life_seq == captured.source_life_seq
        })
        .expect("exact original-life mutation must be published");
    assert_eq!(mutation.source_session_id, captured.source_session_id);
    assert_eq!(mutation.source_server_tick, tick - 5);
    assert_eq!(mutation.source_weapon_data, weapon_data);
    assert_eq!(mutation.source_projectile_index, 0);
    assert_eq!(mutation.result_weapon_data, weapon_data & !0x8000);
    if weapon_data & 0x8000 != 0 {
        assert_eq!(
            projectile.bounces_remaining,
            crate::arena_settings::arena_ship_bomb_bounce_count(captured.ship)
        );
        assert_eq!(mutation.bounces_remaining, projectile.bounces_remaining);
    }
    assert!((mutation.velocity_x - 500.0).abs() < 0.01);
}

#[test]
fn accepted_repel_mutates_live_bot_owned_projectile() {
    assert_repel_mutates_captured_bot_projectile(false, 2);
}

#[test]
fn accepted_repel_mutates_retired_life_bot_owned_projectile() {
    assert_repel_mutates_captured_bot_projectile(true, 2);
}

#[test]
fn accepted_repel_mutates_retired_life_bot_owned_mine_using_captured_ship() {
    assert_repel_mutates_captured_bot_projectile(true, 3 | 0x8000);
}

#[test]
fn hostile_repel_applies_authoritative_radial_impulse_and_lifetime_to_bots() {
    let state = Arc::new(Mutex::new(ServerState {
        config: crate::config::ServerConfig {
            bot_count: 2,
            ..crate::config::ServerConfig::default()
        },
        ..ServerState::default()
    }));
    state.lock().expect("state").initialize_configured_bots();
    let (source_tx, _source_rx) = mpsc::channel();
    let source = register_connection(&state, source_tx);
    join_arena(&state, source, 0);
    let tick = 24_000;
    seed_authoritative_repel_position(&state, source, 0, 500, 700, 10, tick - 1);
    let (hostile_id, hostile_life, friendly_id, friendly_velocity) = {
        let mut locked = state.lock().expect("state");
        let [hostile, friendly] = locked.bots.as_mut_slice() else {
            panic!("fixture requires exactly two configured bots");
        };
        hostile.x = 600.0;
        hostile.y = 700.0;
        hostile.velocity_x = -17.0;
        hostile.velocity_y = 23.0;
        hostile.wall_follow_until_tick = Some(tick + 100);
        hostile.wall_follow_x = -1;
        hostile.wall_follow_y = 1;
        hostile.player.frequency = 1;
        friendly.x = 600.0;
        friendly.y = 700.0;
        friendly.velocity_x = 31.0;
        friendly.velocity_y = -29.0;
        friendly.player.frequency = 0;
        assert_eq!(
            hostile.append_actor_motion_sample(tick - 1, false),
            crate::combat_timeline::HistoryAppendDisposition::Applied
        );
        (
            hostile.player.id,
            hostile.life_seq,
            friendly.player.id,
            (friendly.velocity_x, friendly.velocity_y),
        )
    };

    let (motion_before, relocation_before) = {
        let locked = state.lock().expect("state");
        let history = &locked.bot(hostile_id).unwrap().actor_motion_history;
        (
            history.current_segment().unwrap(),
            history.relocation_generation(),
        )
    };
    fire_authoritative_repel_at_tick(&state, source, 500, 700, 11, tick);

    let locked = state.lock().expect("state");
    let hostile = locked
        .bot(hostile_id)
        .expect("hostile bot remains rostered");
    assert_eq!(hostile.life_seq, hostile_life);
    assert_eq!(
        hostile
            .actor_motion_history
            .current_segment()
            .unwrap()
            .discontinuity_generation,
        motion_before
            .discontinuity_generation
            .wrapping_add(1)
            .max(1)
    );
    assert_eq!(
        hostile.actor_motion_history.relocation_generation(),
        relocation_before,
        "an authoritative Repel must retire motion without inventing a relocation"
    );
    assert!((hostile.velocity_x - 500.0).abs() < 0.01);
    assert!(hostile.velocity_y.abs() < 0.01);
    assert_eq!(hostile.wall_follow_until_tick, None);
    assert_eq!((hostile.wall_follow_x, hostile.wall_follow_y), (0, 0));
    assert_eq!(
        hostile.repel_until_tick,
        Some(crate::continuum::continuum_tick_add(
            tick,
            crate::arena_settings::arena_repel_lifetime_ticks(),
        )),
        "the authoritative bot must retain the reference game's RepelTime window"
    );
    let friendly = locked
        .bot(friendly_id)
        .expect("same-frequency bot remains rostered");
    assert_eq!(
        (friendly.velocity_x, friendly.velocity_y),
        friendly_velocity,
        "same-frequency actors are excluded from Repel"
    );
    assert_eq!(friendly.repel_until_tick, None);
}

fn acknowledge_pending_repel_delivery(
    state: &Arc<Mutex<ServerState>>,
    target_player_id: u16,
    snapshot_id: u16,
) {
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let target = state
            .clients
            .get_mut(&target_player_id)
            .expect("repel target should exist");
        let observation = target
            .pending_repel_observations
            .front_mut()
            .expect("repel observation should be pending");
        observation.delivered_snapshot_id = Some(snapshot_id);
        observation.delivered_snapshot_tick = Some(observation.event_tick);
        observation.delivered_snapshot_cursor = 1;
        let remaining_ticks = crate::continuum::continuum_tick_diff(
            observation.expires_at_tick,
            observation.event_tick,
        )
        .max(0) as u32;
        let window = crate::gameplay_rules::repel_sequence_window(0, remaining_ticks);
        let active_steps = window.1.wrapping_sub(window.0).saturating_sub(1);
        observation.delivered_active_steps = Some(active_steps);
        observation.delivered_snapshots[0] = Some(crate::state::ServerRepelDelivery {
            snapshot_id,
            snapshot_tick: observation.event_tick,
            active_steps,
            acknowledged_at_tick: Some(observation.event_tick),
        });
    }
    let ack = crate::snapshot_wire::SnapshotAckPacket { snapshot_id }.encode();
    crate::connection::record_snapshot_ack_at(
        state,
        target_player_id,
        &ack,
        std::time::Instant::now(),
    );
}

fn accept_repel_target_position_at_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    x: u16,
    y: u16,
    velocity_x: i16,
    velocity_y: i16,
    sequence: u16,
    tick: u32,
) -> crate::arena::AcceptedPositionRelay {
    record_neutral_repel_input(state, player_id, sequence);
    accept_repel_target_position_after_input_at_tick(
        state, player_id, x, y, velocity_x, velocity_y, sequence, tick,
    )
}

#[allow(clippy::too_many_arguments)]
fn try_repel_target_position_after_input_at_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    x: u16,
    y: u16,
    velocity_x: i16,
    velocity_y: i16,
    sequence: u16,
    tick: u32,
) -> Option<crate::arena::AcceptedPositionRelay> {
    let packet = repel_target_position_packet(
        state, player_id, x, y, velocity_x, velocity_y, sequence, tick, false,
    );
    let identity = state
        .lock()
        .expect("server state should not be poisoned")
        .active_life_identity(player_id)
        .expect("repel target life should exist");
    crate::arena::accept_position_relay_for_life_at_tick(state, identity, &packet, tick)
}

fn accept_repel_target_position_after_input_at_tick(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    x: u16,
    y: u16,
    velocity_x: i16,
    velocity_y: i16,
    sequence: u16,
    tick: u32,
) -> crate::arena::AcceptedPositionRelay {
    let packet = repel_target_position_packet(
        state, player_id, x, y, velocity_x, velocity_y, sequence, tick, true,
    );
    let identity = state
        .lock()
        .expect("server state should not be poisoned")
        .active_life_identity(player_id)
        .expect("repel target life should exist");
    crate::arena::accept_position_relay_for_life_at_tick(state, identity, &packet, tick)
        .expect("repel target position should be accepted")
}

#[allow(clippy::too_many_arguments)]
fn repel_target_position_packet(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    x: u16,
    y: u16,
    velocity_x: i16,
    velocity_y: i16,
    sequence: u16,
    tick: u32,
    use_exact_replay_tail: bool,
) -> Vec<u8> {
    repel_target_position_packet_at_application_sequence(
        state,
        player_id,
        x,
        y,
        velocity_x,
        velocity_y,
        sequence,
        sequence,
        tick,
        use_exact_replay_tail,
    )
}

#[allow(clippy::too_many_arguments)]
fn repel_target_position_packet_at_application_sequence(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    x: u16,
    y: u16,
    velocity_x: i16,
    velocity_y: i16,
    sequence: u16,
    application_sequence: u16,
    tick: u32,
    use_exact_replay_tail: bool,
) -> Vec<u8> {
    let mut packet = client_position_packet_with_sequence(0, x, y, sequence);
    packet[1] = 0;
    packet[6..8].copy_from_slice(&velocity_x.to_le_bytes());
    packet[14..16].copy_from_slice(&velocity_y.to_le_bytes());
    let exact_replay_tail = repel_target_replay_tail_at_application_sequence(
        state,
        player_id,
        sequence,
        application_sequence,
        tick,
    );
    if let Some(tail) = exact_replay_tail
        && use_exact_replay_tail
        && tail.wire_x == x
        && tail.wire_y == y
        && tail.wire_velocity_x == velocity_x
        && tail.wire_velocity_y == velocity_y
    {
        set_client_position_canonical_state(
            &mut packet,
            tail.state.position_x,
            tail.state.position_y,
            tail.state.velocity_x,
            tail.state.velocity_y,
            tail.state.orientation,
        );
    } else {
        sync_client_position_canonical_state_from_wire(&mut packet);
    }
    let delivered = state.lock().expect("state").clients[&player_id]
        .pending_repel_observations
        .front()
        .copied()
        .and_then(|observation| {
            observation
                .delivered_snapshot_id
                .map(|snapshot_id| (snapshot_id, observation.delivery_generation))
        });
    if let Some((snapshot_id, generation)) = delivered {
        append_repel_application_marker(&mut packet, snapshot_id, application_sequence, generation);
    }
    packet
}

fn append_repel_application_marker(
    packet: &mut Vec<u8>,
    snapshot_id: u16,
    sequence: u16,
    generation: u32,
) {
    packet[24..26].copy_from_slice(&snapshot_id.to_le_bytes());
    packet[26..28].copy_from_slice(
        &crate::continuum_position::CLIENT_POSITION_REPEL_MARKER_MAGIC.to_le_bytes(),
    );
    packet.push(crate::continuum_position::CLIENT_POSITION_REPEL_APPLICATION_MARKER_TYPE);
    packet.push(crate::continuum_position::CLIENT_POSITION_REPEL_APPLICATION_MARKER_VERSION);
    packet.extend_from_slice(&snapshot_id.to_le_bytes());
    packet.extend_from_slice(&sequence.to_le_bytes());
    packet.extend_from_slice(&generation.to_le_bytes());
}

fn repel_target_replay_tail(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    sequence: u16,
    tick: u32,
) -> Option<crate::state::CanonicalPositionWitness> {
    repel_target_replay_tail_at_application_sequence(state, player_id, sequence, sequence, tick)
}

fn repel_target_replay_tail_at_application_sequence(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    sequence: u16,
    application_sequence: u16,
    tick: u32,
) -> Option<crate::state::CanonicalPositionWitness> {
    let locked = state.lock().expect("state");
    let client = &locked.clients[&player_id];
    let repel_application = client
        .pending_repel_observations
        .front()
        .copied()
        .filter(|observation| {
            (observation.delivery_acked || observation.delivered_snapshot_id.is_some())
                && !crate::continuum::continuum_tick_reached(tick, observation.expires_at_tick)
        })
        .and_then(|observation| {
            Some(crate::sim_observation::CanonicalRepelApplication {
                sequence: application_sequence,
                origin_x: observation.origin_x,
                origin_y: observation.origin_y,
                speed: crate::arena_settings::arena_repel_speed_pixels_per_second(),
                radius: crate::arena_settings::arena_repel_radius_pixels(),
                expected_velocity_x: observation.expected_velocity_x,
                expected_velocity_y: observation.expected_velocity_y,
                delivered_snapshot_tick: observation.delivered_snapshot_tick?,
                active_steps: observation.delivered_active_steps?,
            })
        });
    let upgrades = crate::sim_observation::PlayerMovementUpgrades {
        thrust: client.server_thrust_acceleration,
        top_speed: client.server_top_speed,
        rotation: client.server_rotation_per_second,
        recharge: client.server_recharge_per_second,
        live_values: true,
    };
    let existing_repel_active = client
        .server_repel_until_tick
        .is_some_and(|until| !crate::continuum::continuum_tick_reached(tick, until));
    let has_attached_riders = locked
        .turret_links
        .values()
        .any(|host_id| *host_id == player_id);
    let is_flagger = client.player.flags > 0;
    let map = locked.zone_map();
    let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(map);
    crate::sim_observation::replay_canonical_position_gap_with_repel_application(
        client,
        map,
        is_flagger,
        has_attached_riders,
        existing_repel_active,
        upgrades,
        &[],
        sequence,
        tick,
        spawn_x,
        spawn_y,
        None,
        repel_application,
    )
    .and_then(|replay| replay.witnesses.last().copied())
}

#[test]
fn hostile_repel_rebases_exact_target_event_and_preserves_unrelated_runtime_state() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (source_tx, _source_rx) = mpsc::channel();
    let (target_tx, _target_rx) = mpsc::channel();
    let source = register_connection(&state, source_tx);
    let target = register_connection(&state, target_tx);
    join_arena(&state, source, 0);
    join_arena(&state, target, 0);
    let tick = 20_000;
    seed_authoritative_repel_position(&state, source, 0, 500, 700, 10, tick - 1);
    seed_authoritative_repel_position(&state, target, 1, 600, 700, 20, tick - 1);
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.config.sim_observe = true;
        state.config.movement_snapback = true;
        state.turret_links.insert(source, target);
        let target_client = state.clients.get_mut(&target).unwrap();
        target_client.player.flags = 1;
        target_client.server_emp_until_tick = Some(tick + 100);
        target_client.movement_drift_x = 25.0;
        target_client.movement_drift_y = -17.0;
        target_client.server_sim_state.as_mut().unwrap().velocity_x = -325.0;
    }

    fire_authoritative_repel_at_tick(&state, source, 500, 700, 11, tick);
    let pending = state.lock().unwrap().clients[&target]
        .pending_repel_observations
        .front()
        .copied()
        .expect("enemy in range should receive a life-bound pending repel");
    assert_eq!(pending.source.player_id(), source);
    assert!((pending.expected_velocity_x - 500.0).abs() < 0.01);
    assert!(pending.expected_velocity_y.abs() < 0.01);
    assert_eq!(
        state.lock().unwrap().clients[&source].server_repel_until_tick,
        None
    );
    acknowledge_pending_repel_delivery(&state, target, 71);

    let (motion_before, relocation_before) = {
        let locked = state.lock().expect("state");
        let history = &locked.clients[&target].actor_motion_history;
        (
            history.current_segment().unwrap(),
            history.relocation_generation(),
        )
    };
    let repel_correction = arm_snapback_transaction_for_discontinuity(&state, target, 0xe001, 20);
    let relay =
        accept_repel_target_position_at_tick(&state, target, 600, 700, 5_000, 0, 21, tick + 1);
    assert_eq!(
        u16::from_le_bytes(relay.position[4..6].try_into().unwrap()),
        600,
        "the exact repel event applies after movement for the marked tail"
    );
    let receipt = relay
        .repel_application_receipt
        .expect("exact application emits a private 0x7a receipt");
    assert_eq!(receipt.0, crate::wire::server_session_id());
    assert_eq!(receipt.1, 1);
    assert_eq!(receipt.2, pending.delivery_generation);
    assert_eq!((receipt.3, receipt.4), (71, 21));
    assert_eq!((receipt.5, receipt.6), (156, 135));

    let locked = state.lock().expect("server state should not be poisoned");
    let target_client = &locked.clients[&target];
    assert_eq!(
        target_client
            .actor_motion_history
            .current_segment()
            .unwrap()
            .discontinuity_generation,
        motion_before
            .discontinuity_generation
            .wrapping_add(1)
            .max(1)
    );
    assert_eq!(
        target_client.actor_motion_history.relocation_generation(),
        relocation_before,
        "a human Repel application also preserves its last actual relocation"
    );
    assert!(
        target_client
            .pending_repel_observations
            .front()
            .is_some_and(|observation| observation.accepted_application.is_some())
    );
    assert_eq!(
        target_client.server_repel_until_tick,
        Some(crate::continuum::continuum_tick_add(
            tick,
            crate::arena_settings::arena_repel_lifetime_ticks(),
        )),
        "observing the velocity must retain the server-owned event deadline"
    );
    assert_eq!(target_client.movement_drift_x, 0.0);
    assert_eq!(target_client.movement_drift_y, 0.0);
    assert!((target_client.server_sim_state.unwrap().velocity_x - 500.0).abs() < 0.01);
    assert_eq!(
        target_client.player.flags, 1,
        "flagger state must survive rebase"
    );
    assert_eq!(target_client.server_emp_until_tick, Some(tick + 100));
    assert_eq!(locked.turret_links.get(&source), Some(&target));
    drop(locked);
    assert_snapback_transaction_cancelled(&state, target, repel_correction);
}

#[test]
fn hostile_repel_marker_is_narrowed_by_server_receipt_time_without_client_timing_authority() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (source_tx, _source_rx) = mpsc::channel();
    let (target_tx, _target_rx) = mpsc::channel();
    let source = register_connection(&state, source_tx);
    let target = register_connection(&state, target_tx);
    join_arena(&state, source, 0);
    join_arena(&state, target, 0);
    let event_tick = 32_000;
    seed_authoritative_repel_position(&state, source, 0, 500, 700, 10, event_tick - 1);
    seed_authoritative_repel_position(&state, target, 1, 600, 700, 20, event_tick - 1);
    fire_authoritative_repel_at_tick(&state, source, 500, 700, 11, event_tick);
    acknowledge_pending_repel_delivery(&state, target, 73);

    let deadline = state.lock().unwrap().clients[&target]
        .pending_repel_observations
        .front()
        .expect("Repel event")
        .expires_at_tick;
    let relay = accept_repel_target_position_at_tick(
        &state,
        target,
        600,
        700,
        5_000,
        0,
        21,
        deadline.wrapping_sub(1),
    );
    let receipt = relay
        .repel_application_receipt
        .expect("predeadline exact marker is admitted");
    assert_eq!((receipt.3, receipt.4), (73, 21));
    assert_eq!(
        (receipt.5, receipt.6),
        (22, 1),
        "one remaining server tick authorizes one 60 Hz suffix step, never the original 135-step offer"
    );
    let locked = state.lock().unwrap();
    let target_client = &locked.clients[&target];
    assert_eq!(target_client.server_repel_sequence_window, Some((21, 23)));
    assert_eq!(
        target_client
            .pending_repel_observations
            .front()
            .and_then(|event| event.accepted_application)
            .map(|application| { (application.accepted_end_sequence, application.active_steps,) }),
        Some((22, 1)),
        "the immutable queued event retains the exact receipt suffix for every duplicate"
    );
}

#[test]
fn server_repel_window_retires_after_committed_gap_before_full_sequence_epoch_reuse() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let anchor_tick = 52_000;
    let previous_sequence = u16::MAX;
    let accepted_sequence = 1;
    let application_sequence = u16::MAX - 2;
    let original_window =
        crate::gameplay_rules::repel_sequence_window_from_active_steps(application_sequence, 3);
    assert_eq!(original_window, (u16::MAX - 2, 1));
    seed_exact_canonical_orientation_anchor(
        &state,
        player_id,
        previous_sequence,
        anchor_tick,
        8_200,
        8_200,
        0.0,
    );
    let identity = {
        let mut locked = state.lock().expect("state");
        let identity = locked.active_life_identity(player_id).expect("life");
        let client = locked.clients.get_mut(&player_id).expect("client");
        for sequence in [0, accepted_sequence] {
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: 0,
                    door_phase_tick: anchor_tick,
                });
        }
        client.last_input_seq = Some(accepted_sequence);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        });
        client.server_repel_sequence_window = Some(original_window);
        client.server_repel_until_tick = Some(anchor_tick + 225);
        client.server_repel_observer_until_tick = Some(anchor_tick + 225);
        identity
    };
    let (bounty, energy) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        (client.server_bounty, client.server_energy)
    };
    let mut packet = client_position_packet_with_sequence(0, 8_200, 8_200, accepted_sequence);
    packet[16..18].copy_from_slice(&bounty.to_le_bytes());
    packet[18..20].copy_from_slice(&energy.to_le_bytes());
    set_client_position_canonical_state(&mut packet, 8_200.0, 8_200.0, 0.0, 0.0, 0.0);

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &packet,
            anchor_tick + 2,
        )
        .is_some(),
        "the exact two-record replay crossing the wrapped exclusive end commits",
    );
    let locked = state.lock().expect("state");
    let client = &locked.clients[&player_id];
    assert_eq!(client.server_repel_sequence_window, None);
    assert_eq!(client.server_repel_until_tick, None);
    assert_eq!(client.server_repel_observer_until_tick, None);

    let first_active_sequence_one_full_epoch_later = application_sequence.wrapping_add(1);
    assert!(
        crate::gameplay_rules::repel_sequence_window_contains(
            original_window,
            first_active_sequence_one_full_epoch_later,
        ),
        "retaining the old tuple would grant the recurring wire value RepelSpeed",
    );
    assert!(!client.server_repel_sequence_window.is_some_and(|window| {
        crate::gameplay_rules::repel_sequence_window_contains(
            window,
            first_active_sequence_one_full_epoch_later,
        )
    }));
}

#[test]
fn hostile_repel_preserves_exact_orientation_across_wrap_and_long_gap() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (source_tx, _source_rx) = mpsc::channel();
    let (target_tx, _target_rx) = mpsc::channel();
    let source = register_connection(&state, source_tx);
    let target = register_connection(&state, target_tx);
    join_arena(&state, source, 0);
    join_arena(&state, target, 0);
    let tick = 71_000;
    seed_authoritative_repel_position(&state, source, 0, 500, 700, 10, tick - 1);
    seed_authoritative_repel_position(&state, target, 1, 600, 700, 20, tick - 1);
    fire_authoritative_repel_at_tick(&state, source, 500, 700, 11, tick);
    acknowledge_pending_repel_delivery(&state, target, 79);

    let anchor_sequence = u16::MAX - 5;
    let steps = 12_usize;
    let tail_sequence = anchor_sequence.wrapping_add(steps as u16);
    let initial_orientation = 0.98_f32;
    let (rotation_per_second, runtime, identity) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&target];
        (
            client.server_rotation_per_second,
            locked
                .active_player_runtime_identity(target)
                .expect("runtime"),
            locked.active_life_identity(target).expect("life"),
        )
    };
    seed_exact_canonical_orientation_anchor(
        &state,
        target,
        anchor_sequence,
        tick - 20,
        600,
        700,
        initial_orientation,
    );
    let (records, final_orientation) =
        rotating_input_history(initial_orientation, steps, rotation_per_second);
    assert_eq!(
        crate::continuum_position::protocol_direction(initial_orientation),
        39
    );
    assert!(records.iter().any(|record| record.1 == 0));
    let (_, input_packet) = build_input_record_packet_with_valid_combat_view(
        &state,
        target,
        anchor_sequence.wrapping_add(1),
        &records,
        tick + 1,
    );
    assert!(crate::connection::record_client_input_observation_at_tick(
        &state,
        runtime,
        &input_packet,
        tick + 1,
    ));
    let final_direction = crate::continuum_position::protocol_direction(final_orientation);
    let tail = repel_target_replay_tail(&state, target, tail_sequence, tick + 1)
        .expect("exact hostile-repel replay tail");
    assert_eq!(
        tail.state.orientation.to_bits(),
        final_orientation.to_bits()
    );
    let mut exact =
        client_position_packet_with_sequence(0, tail.wire_x, tail.wire_y, tail_sequence);
    exact[1] = final_direction;
    exact[6..8].copy_from_slice(&tail.wire_velocity_x.to_le_bytes());
    exact[14..16].copy_from_slice(&tail.wire_velocity_y.to_le_bytes());
    exact[20..22].copy_from_slice(&1_u16.to_le_bytes());
    set_client_position_canonical_state(
        &mut exact,
        tail.state.position_x,
        tail.state.position_y,
        tail.state.velocity_x,
        tail.state.velocity_y,
        tail.state.orientation,
    );
    let generation = state.lock().unwrap().clients[&target]
        .pending_repel_observations
        .front()
        .copied()
        .expect("pending Repel")
        .delivery_generation;
    append_repel_application_marker(&mut exact, 79, tail_sequence, generation);

    let before_forged = witness_admission_state(&state, target);
    let mut forged_heading = exact.clone();
    forged_heading[1] = (final_direction + 1) % 40;
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &forged_heading,
            tick + 1,
        )
        .is_none()
    );
    assert_eq!(witness_admission_state(&state, target), before_forged);

    let mut forged_origin = exact.clone();
    forged_origin[12..14].copy_from_slice(&2_000_u16.to_le_bytes());
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &forged_origin,
            tick + 1,
        )
        .is_none()
    );
    assert_eq!(witness_admission_state(&state, target), before_forged);

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(&state, identity, &exact, tick + 1,)
            .is_some()
    );
    let locked = state.lock().expect("state");
    let witness = locked.clients[&target]
        .canonical_movement_witness
        .expect("repel witness");
    assert_eq!(witness.state.orientation, final_orientation);
    assert_eq!(witness.state.orientation, final_orientation);
    assert_eq!((witness.wire_x, witness.wire_y), (tail.wire_x, tail.wire_y));
    assert_eq!(
        (witness.wire_velocity_x, witness.wire_velocity_y),
        (tail.wire_velocity_x, tail.wire_velocity_y)
    );
    let projectile = locked.active_projectiles.last().expect("projectile");
    assert_eq!((projectile.x, projectile.y), (tail.wire_x, tail.wire_y));
}

#[test]
fn hostile_repel_waits_through_pre_delivery_position_then_rebases_once() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (source_tx, _source_rx) = mpsc::channel();
    let (target_tx, _target_rx) = mpsc::channel();
    let source = register_connection(&state, source_tx);
    let target = register_connection(&state, target_tx);
    join_arena(&state, source, 0);
    join_arena(&state, target, 0);
    let tick = 30_000;
    seed_authoritative_repel_position(&state, source, 0, 500, 700, 10, tick - 1);
    seed_authoritative_repel_position(&state, target, 1, 600, 700, 20, tick - 1);

    fire_authoritative_repel_at_tick(&state, source, 500, 700, 11, tick);
    record_neutral_repel_input(&state, target, 21);
    {
        let mut state = state.lock().unwrap();
        state.clients.get_mut(&target).unwrap().movement_drift_x = 19.0;
        state.clients.get_mut(&target).unwrap().movement_drift_y = -7.0;
    }
    accept_repel_target_position_after_input_at_tick(&state, target, 600, 700, 0, 0, 21, tick + 1);
    assert!(
        !state.lock().unwrap().clients[&target]
            .pending_repel_observations
            .is_empty(),
        "an aligned maximum-speed position before snapshot ACK must not consume the event"
    );
    assert_eq!(
        state.lock().unwrap().clients[&target].server_repel_until_tick,
        None,
        "a position before exact snapshot delivery cannot arm RepelSpeed"
    );

    acknowledge_pending_repel_delivery(&state, target, 72);
    assert_eq!(
        state.lock().unwrap().clients[&target].server_repel_until_tick,
        None,
        "transport ACK alone cannot arm movement before the exact application boundary"
    );
    record_neutral_repel_input(&state, target, 22);
    let before_near_match = witness_admission_state(&state, target);
    assert!(
        try_repel_target_position_after_input_at_tick(
            &state,
            target,
            611,
            700,
            4_400,
            0,
            22,
            tick + 3,
        )
        .is_none()
    );
    assert_eq!(witness_admission_state(&state, target), before_near_match);
    assert!(
        !state.lock().unwrap().clients[&target]
            .pending_repel_observations
            .is_empty(),
        "an ACKed but malicious near-match outside the tight impulse vector must not consume"
    );

    record_neutral_repel_input(&state, target, 23);
    let application_sequence = 22;
    let tail = repel_target_replay_tail_at_application_sequence(
        &state,
        target,
        23,
        application_sequence,
        tick + 6,
    )
    .expect("lost first report must retain the exact earlier Repel boundary");
    let exact = repel_target_position_packet_at_application_sequence(
        &state,
        target,
        tail.wire_x,
        tail.wire_y,
        tail.wire_velocity_x,
        tail.wire_velocity_y,
        23,
        application_sequence,
        tick + 6,
        true,
    );
    let identity = state.lock().unwrap().active_life_identity(target).unwrap();

    let before_wrong_boundary = witness_admission_state(&state, target);
    let mut wrong_boundary = exact.clone();
    let marker_offset = wrong_boundary.len()
        - crate::continuum_position::CLIENT_POSITION_REPEL_APPLICATION_MARKER_LEN;
    wrong_boundary[marker_offset + 4..marker_offset + 6].copy_from_slice(&23_u16.to_le_bytes());
    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(
            &state,
            identity,
            &wrong_boundary,
            tick + 6,
        )
        .is_none(),
        "a forged later application boundary cannot select a different exact tail"
    );
    assert_eq!(
        witness_admission_state(&state, target),
        before_wrong_boundary
    );

    assert!(
        crate::arena::accept_position_relay_for_life_at_tick(&state, identity, &exact, tick + 6,)
            .is_some()
    );
    let state = state.lock().unwrap();
    let delayed_observation = state.clients[&target]
        .pending_repel_observations
        .front()
        .copied();
    assert!(
        delayed_observation.is_some_and(|observation| observation.accepted_application.is_some()),
        "delayed observation after exact acceptance: {delayed_observation:?}"
    );
    assert!(state.clients[&target].server_repel_until_tick.is_some());
}

#[test]
fn hostile_repel_delivery_gate_rejects_every_legal_pre_delivery_speed_modifier() {
    let base = crate::arena_settings::arena_ship_max_speed_pixels_per_second(0);
    let rocket = crate::arena_settings::arena_rocket_speed_pixels_per_second();
    let gravity = crate::arena_settings::arena_ship_wormhole_top_speed(0);
    let turret = (base - crate::arena_settings::arena_ship_turret_speed_penalty(0)).max(0.0);
    let flagger = (base + crate::arena_settings::arena_flagger_speed_adjustment()).max(0.0);
    let existing_repel = base.max(crate::arena_settings::arena_repel_speed_pixels_per_second())
        + crate::arena_settings::arena_flagger_speed_adjustment();
    let legal_caps = [
        ("afterburner", base),
        ("rocket", rocket),
        ("wormhole_gravity", gravity),
        ("turret_host", turret),
        ("flagger", flagger),
        ("existing_repel_window", existing_repel.max(0.0)),
    ];
    assert_eq!(base, 325.0);
    assert_eq!(rocket, 550.0);
    assert_eq!(
        crate::arena_settings::arena_repel_speed_pixels_per_second(),
        500.0
    );

    for (index, (name, speed)) in legal_caps.into_iter().enumerate() {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (source_tx, _source_rx) = mpsc::channel();
        let (target_tx, _target_rx) = mpsc::channel();
        let source = register_connection(&state, source_tx);
        let target = register_connection(&state, target_tx);
        join_arena(&state, source, 0);
        join_arena(&state, target, 0);
        let tick = 31_000 + index as u32 * 100;
        seed_authoritative_repel_position(&state, source, 0, 500, 700, 10, tick - 1);
        seed_authoritative_repel_position(&state, target, 1, 600, 700, 20, tick - 1);
        fire_authoritative_repel_at_tick(&state, source, 500, 700, 11, tick);

        record_neutral_repel_input(&state, target, 21);
        let before = witness_admission_state(&state, target);
        assert!(
            try_repel_target_position_after_input_at_tick(
                &state,
                target,
                604,
                700,
                (speed * 10.0).round() as i16,
                0,
                21,
                tick + 1,
            )
            .is_none(),
            "pre-delivery {name} tail must fail exact canonical replay"
        );
        assert_eq!(witness_admission_state(&state, target), before);
        assert!(
            !state.lock().unwrap().clients[&target]
                .pending_repel_observations
                .is_empty(),
            "legal {name} speed {speed} must not consume an unacknowledged repel delivery"
        );
    }
}

#[test]
fn multiple_hostile_repels_queue_without_overwriting_the_head() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (left_tx, _left_rx) = mpsc::channel();
    let (target_tx, _target_rx) = mpsc::channel();
    let (right_tx, _right_rx) = mpsc::channel();
    let left = register_connection(&state, left_tx);
    let target = register_connection(&state, target_tx);
    let right = register_connection(&state, right_tx);
    join_arena(&state, left, 0);
    join_arena(&state, target, 0);
    join_arena(&state, right, 0);
    let tick = 40_000;
    seed_authoritative_repel_position(&state, left, 0, 500, 700, 10, tick - 1);
    seed_authoritative_repel_position(&state, target, 1, 600, 700, 20, tick - 1);
    seed_authoritative_repel_position(&state, right, 2, 700, 700, 30, tick - 1);

    fire_authoritative_repel_at_tick(&state, left, 500, 700, 11, tick);
    fire_authoritative_repel_at_tick(&state, right, 700, 700, 31, tick + 1);
    {
        let state = state.lock().unwrap();
        let queue = &state.clients[&target].pending_repel_observations;
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].source.player_id(), left);
        assert_eq!(queue[1].source.player_id(), right);
        assert!((queue[0].expected_velocity_x - 500.0).abs() < 0.01);
        assert!((queue[1].expected_velocity_x + 500.0).abs() < 0.01);
    }

    acknowledge_pending_repel_delivery(&state, target, 81);
    record_neutral_repel_input(&state, target, 21);
    let before_old_impulse = witness_admission_state(&state, target);
    assert!(
        try_repel_target_position_after_input_at_tick(
            &state,
            target,
            608,
            700,
            5_000,
            0,
            21,
            tick + 2,
        )
        .is_none()
    );
    assert_eq!(witness_admission_state(&state, target), before_old_impulse);
    assert_eq!(
        state.lock().unwrap().clients[&target]
            .pending_repel_observations
            .front()
            .copied()
            .unwrap()
            .source
            .player_id(),
        left,
        "a later event must not overwrite or overtake the queue head"
    );
    let tail = repel_target_replay_tail(&state, target, 21, tick + 3)
        .expect("first exact hostile-repel tail");
    accept_repel_target_position_after_input_at_tick(
        &state,
        target,
        tail.wire_x,
        tail.wire_y,
        tail.wire_velocity_x,
        tail.wire_velocity_y,
        21,
        tick + 3,
    );
    assert!(
        state.lock().unwrap().clients[&target]
            .pending_repel_observations
            .front()
            .is_some_and(|observation| observation.accepted_application.is_some())
    );
}

#[test]
fn hostile_repel_observation_is_wrap_safe_and_life_scoped() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (source_tx, _source_rx) = mpsc::channel();
    let (target_tx, _target_rx) = mpsc::channel();
    let (other_tx, _other_rx) = mpsc::channel();
    let source = register_connection(&state, source_tx);
    let target = register_connection(&state, target_tx);
    let other = register_connection(&state, other_tx);
    join_arena(&state, source, 0);
    join_arena(&state, target, 0);
    join_arena(&state, other, 0);
    let tick = crate::continuum::CONTINUUM_TICK_MASK - 2;
    seed_authoritative_repel_position(
        &state,
        source,
        0,
        500,
        700,
        10,
        crate::continuum::continuum_tick_sub(tick, 1),
    );
    seed_authoritative_repel_position(
        &state,
        target,
        1,
        600,
        700,
        20,
        crate::continuum::continuum_tick_sub(tick, 1),
    );
    seed_authoritative_repel_position(
        &state,
        other,
        2,
        650,
        700,
        30,
        crate::continuum::continuum_tick_sub(tick, 1),
    );

    fire_authoritative_repel_at_tick(&state, source, 500, 700, 11, tick);
    let source_event = state.lock().unwrap().clients[&target]
        .pending_repel_observations
        .front()
        .copied()
        .unwrap();
    {
        let mut state = state.lock().unwrap();
        state.advance_client_life_at_tick(source, 0);
    }
    assert_eq!(
        state.lock().unwrap().clients[&target]
            .pending_repel_observations
            .front()
            .copied(),
        Some(source_event),
        "an already-emitted event remains valid if its source changes life"
    );
    acknowledge_pending_repel_delivery(&state, target, u16::MAX - 1);
    state
        .lock()
        .unwrap()
        .clients
        .get_mut(&target)
        .unwrap()
        .last_acked_snapshot_id = Some(2);
    accept_repel_target_position_at_tick(&state, target, 600, 700, 5_000, 0, 21, 1);
    assert!(
        state.lock().unwrap().clients[&target]
            .pending_repel_observations
            .front()
            .is_some_and(|observation| observation.accepted_application.is_some())
    );

    {
        let mut state = state.lock().unwrap();
        assert!(!state.clients[&other].pending_repel_observations.is_empty());
        state.advance_client_life_at_tick(other, 2);
        assert!(state.clients[&other].pending_repel_observations.is_empty());
        assert!(state.clients[&other].server_repel_until_tick.is_none());
    }
}

#[test]
fn hostile_repel_targets_only_live_enemy_ships_in_range_and_expires_without_a_waiver() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let mut players = Vec::new();
    for _ in 0..6 {
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        players.push(player_id);
    }
    let source = players[0];
    let target = players[1];
    let same_frequency = players[2];
    let outside = players[3];
    let dead = players[4];
    let spectator = players[5];
    let tick = 50_000;
    seed_authoritative_repel_position(&state, source, 0, 500, 700, 10, tick - 1);
    seed_authoritative_repel_position(&state, target, 1, 600, 700, 20, tick - 1);
    seed_authoritative_repel_position(&state, same_frequency, 0, 550, 700, 30, tick - 1);
    seed_authoritative_repel_position(&state, outside, 2, 1_100, 700, 40, tick - 1);
    seed_authoritative_repel_position(&state, dead, 3, 575, 700, 50, tick - 1);
    seed_authoritative_repel_position(&state, spectator, 4, 625, 700, 60, tick - 1);
    {
        let mut state = state.lock().unwrap();
        state.clients.get_mut(&dead).unwrap().alive = false;
        state.clients.get_mut(&spectator).unwrap().player.ship = 8;
    }

    fire_authoritative_repel_at_tick(&state, source, 500, 700, 11, tick);
    {
        let state = state.lock().unwrap();
        assert!(!state.clients[&target].pending_repel_observations.is_empty());
        for unaffected in [source, same_frequency, outside, dead, spectator] {
            assert!(
                state.clients[&unaffected]
                    .pending_repel_observations
                    .is_empty(),
                "player {unaffected} must not receive a hostile-repel discontinuity event"
            );
        }
    }

    let expiry = crate::continuum::continuum_tick_add(
        tick,
        crate::arena_settings::arena_repel_lifetime_ticks(),
    );
    record_neutral_repel_input(&state, target, 21);
    let canonical_before_expired_forgery =
        state.lock().unwrap().clients[&target].canonical_movement_witness;
    assert!(
        try_repel_target_position_after_input_at_tick(
            &state, target, 608, 700, 5_000, 0, 21, expiry,
        )
        .is_none(),
        "an expired discontinuity cannot authorize the old impulse tail"
    );
    assert_eq!(
        state.lock().unwrap().clients[&target].canonical_movement_witness,
        canonical_before_expired_forgery,
        "rejecting the expired impulse must not move the canonical anchor"
    );
    accept_repel_target_position_after_input_at_tick(&state, target, 600, 700, 0, 0, 21, expiry);
    let state = state.lock().unwrap();
    let delayed_observation = state.clients[&target]
        .pending_repel_observations
        .front()
        .copied();
    assert!(
        delayed_observation.is_none(),
        "an expired undelivered private Repel must be cancelled: {delayed_observation:?}"
    );
    assert_eq!(
        state.clients[&target].server_repel_until_tick, None,
        "an expired event must not grant a rebase or repel-speed window"
    );
}

#[test]
fn server_records_phase_1b_client_input_observation() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    let packet = build_input_record_packet(7, &[(0x1f, 23, 0)]);
    let mut joined = true;
    crate::connection::handle_client_packet(&state, player_id, packet, "test-peer", &mut joined);

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client present");
    assert_eq!(client.last_input_seq, Some(7));
    let observation = client
        .last_input_record
        .as_ref()
        .expect("input observation stored");
    assert_eq!(observation.bits, 0x1f);
    assert_eq!(observation.direction, 23);
}

#[test]
fn server_records_embedded_input_history_before_queueing_its_position() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let sequence = u16::MAX;
    let mut packet = client_position_packet_with_sequence(0, 500, 700, sequence);
    packet[1] = 23;
    sync_client_position_canonical_state_from_wire(&mut packet);
    packet.extend_from_slice(&build_input_record_packet(sequence, &[(0x1f, 23, 0)]));
    let mut joined = true;

    crate::connection::handle_client_packet(&state, player_id, packet, "test-peer", &mut joined);

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client present");
    assert_eq!(client.last_input_seq, Some(sequence));
    assert_eq!(client.last_input_record.map(|input| input.bits), Some(0x1f));
    assert_eq!(
        state
            .pending_position_inputs
            .queued_count_for_player(player_id),
        1
    );
}

#[test]
fn embedded_tail_sequence_and_direction_mismatches_are_atomic() {
    for (tail_sequence_offset, tail_direction, reason) in [
        (1_u16, 0_u8, "embedded_tail_sequence_mismatch"),
        (0, 1, "embedded_tail_direction_mismatch"),
    ] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let sequence = 900;
        let mut packet = client_position_packet_with_sequence(0, 500, 700, sequence);
        packet[1] = 0;
        packet[20..22].copy_from_slice(&1_u16.to_le_bytes());
        sync_client_position_canonical_state_from_wire(&mut packet);
        packet.extend_from_slice(&build_input_record_packet(
            sequence.wrapping_add(tail_sequence_offset),
            &[(0, tail_direction, 0)],
        ));
        let before = witness_admission_state(&state, player_id);
        let queued_before = state
            .lock()
            .expect("server state")
            .pending_position_inputs
            .queued_count_for_player(player_id);
        let count_before = crate::security::peek_security_reject_count_for_test("position", reason);
        let mut joined = true;

        crate::connection::handle_client_packet(
            &state,
            player_id,
            packet,
            "forged-embedded-peer",
            &mut joined,
        );

        assert_eq!(witness_admission_state(&state, player_id), before);
        assert_eq!(
            state
                .lock()
                .expect("server state")
                .pending_position_inputs
                .queued_count_for_player(player_id),
            queued_before
        );
        assert_eq!(
            crate::security::peek_security_reject_count_for_test("position", reason),
            count_before + 1
        );
    }
}

#[test]
fn server_rejects_malformed_or_rewriting_embedded_history_before_position_queue() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        build_input_record_packet(7, &[(0x01, 3, 0)]),
        "test-peer",
        &mut joined,
    );

    let mut rewrite = client_position_packet_with_sequence(0, 500, 700, 7);
    rewrite.extend_from_slice(&build_input_record_packet(7, &[(0x02, 3, 0)]));
    crate::connection::handle_client_packet(&state, player_id, rewrite, "test-peer", &mut joined);

    let mut malformed = client_position_packet_with_sequence(0, 500, 700, 8);
    malformed.extend_from_slice(&[0x62, 8, 0, 2, 0x01, 0, 3, 0]);
    crate::connection::handle_client_packet(&state, player_id, malformed, "test-peer", &mut joined);

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client present");
    assert_eq!(client.last_input_seq, Some(7));
    assert_eq!(client.last_input_record.map(|input| input.bits), Some(0x01));
    assert!(state.pending_position_inputs.is_empty());
}

#[test]
fn server_retains_each_sequence_and_keeps_latest_when_input_packet_carries_a_ring() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    // 3-entry ring; the server-side mirror should reflect the last
    // entry (the most recent input the client applied).
    let packet = build_input_record_packet(100, &[(0x01, 0, 0), (0x05, 10, 0), (0x1f, 20, 0)]);
    let mut joined = true;
    crate::connection::handle_client_packet(&state, player_id, packet, "test-peer", &mut joined);

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client present");
    assert_eq!(client.last_input_seq, Some(102));
    let observation = client
        .last_input_record
        .as_ref()
        .expect("input observation stored");
    assert_eq!(observation.bits, 0x1f);
    assert_eq!(observation.direction, 20);
    assert_eq!(client.recent_input_records.len(), 3);
    assert_eq!(
        client
            .recent_input_records
            .iter()
            .map(|record| (record.sequence, record.input.bits))
            .collect::<Vec<_>>(),
        vec![(100, 0x01), (101, 0x05), (102, 0x1f)]
    );
}

#[test]
fn server_retains_exact_input_sequences_across_newer_unordered_arrivals() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    let mut joined = true;
    for (sequence, bits) in [(40, 0x01), (41, 0x02), (42, 0x05)] {
        crate::connection::handle_client_packet(
            &state,
            player_id,
            build_input_record_packet(sequence, &[(bits, 0, 0)]),
            "test-peer",
            &mut joined,
        );
    }

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client present");
    assert_eq!(client.last_input_seq, Some(42));
    assert_eq!(
        client
            .recent_input_records
            .iter()
            .find(|record| record.sequence == 40)
            .map(|record| record.input.bits),
        Some(0x01),
        "a queued position with sequence 40 must still resolve its own input"
    );
    assert_eq!(client.recent_input_records.len(), 3);
}

#[test]
fn server_accepts_late_missing_entries_without_rewinding_latest_input() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    let mut joined = true;
    let first = build_input_record_packet(10, &[(0x01, 4, 0)]);
    crate::connection::handle_client_packet(&state, player_id, first, "test-peer", &mut joined);
    let skipped_gap = build_input_record_packet(12, &[(0x1f, 20, 0)]);
    crate::connection::handle_client_packet(
        &state,
        player_id,
        skipped_gap,
        "test-peer",
        &mut joined,
    );
    let late_missing = build_input_record_packet(11, &[(0x02, 8, 0)]);
    crate::connection::handle_client_packet(
        &state,
        player_id,
        late_missing,
        "test-peer",
        &mut joined,
    );

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client present");
    assert_eq!(client.last_input_seq, Some(12));
    let observation = client
        .last_input_record
        .as_ref()
        .expect("input observation stored");
    assert_eq!(observation.bits, 0x1f);
    assert_eq!(observation.direction, 20);
    assert_eq!(
        client
            .recent_input_records
            .iter()
            .map(|record| (record.sequence, record.input.bits))
            .collect::<Vec<_>>(),
        vec![(10, 0x01), (11, 0x02), (12, 0x1f)]
    );
}

#[test]
fn server_recovers_reordered_wrapped_ring_without_allowing_sequence_rewrite() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let mut joined = true;

    crate::connection::handle_client_packet(
        &state,
        player_id,
        build_input_record_packet(0xffff, &[(1, 39, 0), (2, 0, 0), (4, 1, 0)]),
        "test-peer",
        &mut joined,
    );
    crate::connection::handle_client_packet(
        &state,
        player_id,
        build_input_record_packet(0xfffe, &[(8, 38, 0), (1, 39, 0)]),
        "test-peer",
        &mut joined,
    );
    let before = {
        let state = state.lock().expect("state");
        let client = &state.clients[&player_id];
        assert_eq!(client.last_input_seq, Some(1));
        client
            .recent_input_records
            .iter()
            .copied()
            .collect::<Vec<_>>()
    };
    crate::connection::handle_client_packet(
        &state,
        player_id,
        build_input_record_packet(0, &[(0x20, 0, 0)]),
        "test-peer",
        &mut joined,
    );
    let state = state.lock().expect("state");
    let client = &state.clients[&player_id];
    assert_eq!(
        client
            .recent_input_records
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(
        client
            .recent_input_records
            .iter()
            .map(|record| (record.sequence, record.input.bits))
            .collect::<Vec<_>>(),
        vec![(0xfffe, 8), (0xffff, 1), (0, 2), (1, 4)]
    );
}

#[test]
fn server_uses_input_ring_tail_sequence_for_replay_rejection() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    let mut joined = true;
    let first_ring = build_input_record_packet(100, &[(0x01, 0, 0), (0x05, 10, 0), (0x1f, 20, 0)]);
    crate::connection::handle_client_packet(
        &state,
        player_id,
        first_ring,
        "test-peer",
        &mut joined,
    );
    let replayed_tail = build_input_record_packet(102, &[(0x02, 30, 0)]);
    crate::connection::handle_client_packet(
        &state,
        player_id,
        replayed_tail,
        "test-peer",
        &mut joined,
    );

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client present");
    assert_eq!(client.last_input_seq, Some(102));
    let observation = client
        .last_input_record
        .as_ref()
        .expect("input observation stored");
    assert_eq!(observation.bits, 0x1f);
    assert_eq!(observation.direction, 20);
}

#[test]
fn server_drops_out_of_order_input_packets_with_older_seq() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    let mut joined = true;
    let newer = build_input_record_packet(50, &[(0x1f, 30, 0)]);
    crate::connection::handle_client_packet(&state, player_id, newer, "test-peer", &mut joined);
    let older = build_input_record_packet(10, &[(0x01, 0, 0)]);
    crate::connection::handle_client_packet(&state, player_id, older, "test-peer", &mut joined);

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client present");
    // Newer packet stays — older one was rejected as out-of-order.
    assert_eq!(client.last_input_seq, Some(50));
    let observation = client
        .last_input_record
        .as_ref()
        .expect("input observation stored");
    assert_eq!(observation.bits, 0x1f);
    assert_eq!(observation.direction, 30);
}

#[test]
fn server_rejects_input_record_with_forged_door_seed() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    let mut joined = true;
    let packet = build_input_record_packet(77, &[(0x1f, 23, 9)]);
    crate::connection::handle_client_packet(&state, player_id, packet, "test-peer", &mut joined);

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client present");
    assert_eq!(client.last_input_seq, None);
    assert_eq!(client.last_input_record, None);
}

#[test]
fn recovered_fixed_step_batch_accepts_receipt_current_door_phase_but_not_future_phase() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let identity = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(player_id)
        .expect("runtime");
    let door_state = crate::doors::ClientDoorState::for_challenge(24);
    state
        .lock()
        .expect("state")
        .clients
        .get_mut(&player_id)
        .expect("client")
        .door_state = Some(door_state);

    let prior_tick = 2_904;
    let receipt_tick = 3_042;
    let prior_phase =
        crate::doors::client_door_phase_candidates_at_tick(Some(door_state), prior_tick)
            .into_iter()
            .find(|phase| phase.started_tick == prior_tick)
            .expect("prior phase");
    let phases = crate::doors::client_door_phase_candidates_at_tick(Some(door_state), receipt_tick);
    let current_phase = phases
        .iter()
        .copied()
        .find(|phase| phase.started_tick == 3_024)
        .expect("receipt-current phase");
    let future_phase = phases
        .iter()
        .copied()
        .find(|phase| phase.started_tick == 3_144)
        .expect("future phase");

    assert!(crate::connection::record_client_input_observation_at_tick(
        &state,
        identity,
        &build_input_record_packet(17_028, &[(0, 0, prior_phase.seed)]),
        prior_tick,
    ));
    let catch_up = vec![(0, 0, current_phase.seed); 15];
    assert!(crate::connection::record_client_input_observation_at_tick(
        &state,
        identity,
        &build_input_record_packet(17_029, &catch_up),
        receipt_tick,
    ));
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert_eq!(client.last_input_seq, Some(17_043));
        assert_eq!(client.recent_input_records.len(), 16);
        assert_eq!(client.recent_input_records[0].door_phase_tick, prior_tick);
        assert!(
            client
                .recent_input_records
                .iter()
                .skip(1)
                .all(|record| record.door_phase_tick == current_phase.started_tick)
        );
    }

    let before = witness_admission_state(&state, player_id);
    assert!(!crate::connection::record_client_input_observation_at_tick(
        &state,
        identity,
        &build_input_record_packet(17_044, &[(0, 0, future_phase.seed)]),
        receipt_tick,
    ));
    assert_eq!(witness_admission_state(&state, player_id), before);
}

#[test]
fn server_records_projectile_with_spawn_velocity_and_expiry_tick() {
    // Phase 2a: when the server accepts a fire, the recorded
    // projectile carries the shooter's velocity plus the weapon's
    // configured speed projected along the firing heading. The
    // expiry_tick is `spawn_tick + arena_weapon_lifetime_ticks`.
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    reset_server_runtime_without_initial_bounty(&state, beta);

    // Pre-warm a baseline position so the validator accepts subsequent
    // fires (tile validation needs a previous position).
    let mut baseline = client_position_packet(0x1122_3300, 8200, 8200);
    // Direction 10 (orientation 0.25 → heading east on the unit
    // circle: cos(0)=1, sin(0)=0). Picking a heading whose x and y
    // components are clearly distinguishable lets the test assert
    // both axes meaningfully.
    baseline[1] = 10;
    broadcast_position_observable(&state, alpha, &baseline);

    let mut fire = client_position_packet(0x1122_3340, 8200, 8200);
    fire[1] = 10; // same heading (east)
    // Wire layout: velocity_x at bytes 6..8, velocity_y at 14..16.
    fire[6..8].copy_from_slice(&50_i16.to_le_bytes()); // shooter vx = 5 px/s
    fire[14..16].copy_from_slice(&0_i16.to_le_bytes()); // shooter vy = 0
    fire[20..22].copy_from_slice(&1_u16.to_le_bytes()); // bullet (type 1)
    broadcast_position_observable(&state, alpha, &fire);

    let state = state.lock().expect("server state should not be poisoned");
    let projectile = state
        .active_projectiles
        .last()
        .expect("a projectile should have been recorded");
    assert_eq!(projectile.owner_id, alpha);
    assert_eq!(projectile.weapon_data, 1);
    // velocity_x should be > 50 (shooter vx) because the bullet adds
    // the weapon's spawn-speed in the heading direction (+x = east).
    assert!(
        projectile.velocity_x > 50,
        "expected spawn vx > shooter vx, got {}",
        projectile.velocity_x
    );
    // velocity_y should be ~0 since the heading is pure east.
    assert!(
        projectile.velocity_y.abs() < 50,
        "expected spawn vy ≈ 0 for east-firing bullet, got {}",
        projectile.velocity_y
    );
    // expiry_tick is anchored to server-tick at spawn (Phase 2b
    // changed the reference frame from snapshot.timestamp to
    // wire::current_tick), so it doesn't track the wire timestamp.
    // Just verify the lifetime was applied — the value should be
    // larger than the server's current tick at observation time.
    assert!(
        projectile.expiry_tick.is_some(),
        "expiry_tick should be set from BulletAliveTime"
    );
    let lifetime_ticks = crate::arena_settings::arena_weapon_lifetime_ticks(projectile.weapon_data)
        .expect("bullet lifetime should be configured");
    assert_eq!(
        projectile.expiry_tick,
        Some(crate::continuum::continuum_tick_add(
            projectile.timestamp,
            lifetime_ticks,
        )),
        "projectile lifetime should be anchored to the server-owned spawn tick"
    );
}

#[test]
fn server_accepted_position_feeds_history_and_projectile_spawn_without_sim_override() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);

    let mut previous = client_position_packet(100, 8200, 8200);
    previous[6..8].copy_from_slice(&1000_i16.to_le_bytes());
    sync_client_position_canonical_state_from_wire(&mut previous);
    let previous_large =
        large_position_from_client(alpha, &previous).expect("previous position should translate");
    let previous_sequence = crate::continuum_position::parse_client_position_snapshot(&previous)
        .and_then(|snapshot| snapshot.client_sequence)
        .expect("previous fixture sequence");
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        state.set_client_position(alpha, previous_large);
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        let anchor_tick = server_tick_before_delay(10);
        client.last_position_server_tick = Some(anchor_tick);
        client.last_position_sequence = Some(previous_sequence);
        client.last_energy_recharge_tick = Some(anchor_tick);
        client.server_sim_state = Some(crate::state::ServerSimMovement {
            position_x: f32::NAN,
            position_y: f32::NAN,
            orientation: 0.0,
            velocity_x: 40.0,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        });
        client.server_sim_anchor_tick = Some(anchor_tick);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: 7,
        });
        let exact_state = crate::state::ServerSimMovement {
            position_x: 8_200.0,
            position_y: 8_200.0,
            orientation: 7.0 / 40.0,
            velocity_x: 100.0,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let witness = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state: exact_state,
            wire_x: 8_200,
            wire_y: 8_200,
            wire_velocity_x: 1_000,
            wire_velocity_y: 0,
            direction: 7,
            door_seed: 0,
            door_phase_tick: 0,
        };
        client.canonical_movement_witness = Some(witness);
        seed_fixture_motion_baseline(client);
        client.canonical_position_witnesses.push_back(witness);
    }

    let mut fire = client_position_packet(110, 8202, 8200);
    fire[1] = 7;
    fire[6..8].copy_from_slice(&1000_i16.to_le_bytes());
    fire[20..22].copy_from_slice(&1_u16.to_le_bytes());
    set_client_position_canonical_state(
        &mut fire,
        8_200.0 + 100.0 / 60.0,
        8_200.0,
        100.0,
        0.0,
        7.0 / 40.0,
    );
    let fire_sequence = crate::continuum_position::parse_client_position_snapshot(&fire)
        .and_then(|snapshot| snapshot.client_sequence)
        .expect("fire fixture should carry a sequence");
    {
        let mut state = state.lock().expect("server state should not be poisoned");
        let client = state.clients.get_mut(&alpha).expect("alpha should exist");
        let input = crate::state::ClientInputObservation {
            bits: 0,
            direction: fire[1],
        };
        client.last_input_seq = Some(fire_sequence);
        client.last_input_record = Some(input);
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: fire_sequence,
                input,
                door_seed: 0,
                door_phase_tick: 0,
            });
    }
    seed_combat_view_decision_for_test(&state, alpha, fire_sequence, crate::wire::current_tick());
    let relay = accept_position_relay(&state, alpha, &fire).expect("position should be accepted");

    let relayed_x = u16::from_le_bytes(relay.position[4..6].try_into().unwrap());
    assert_eq!(
        relayed_x, 8202,
        "accepted snapshots should publish validated reported x, not private sim x"
    );

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&alpha).expect("alpha should exist");
    let cached = client
        .last_position
        .as_ref()
        .expect("accepted position should be cached");
    assert_eq!(
        u16::from_le_bytes(cached[4..6].try_into().unwrap()),
        relayed_x
    );
    let sample = client
        .position_history
        .back()
        .expect("accepted position should be in lag-comp history");
    assert_eq!(sample.x, relayed_x as f32);

    let projectile = state
        .active_projectiles
        .last()
        .expect("accepted weapon should record projectile");
    assert_eq!(projectile.x, relayed_x);
    assert_eq!(projectile.pos_x, relayed_x as f32);
    assert!(
        client.server_sim_state.is_some(),
        "private sim should still advance for validation/correction"
    );
}

fn arm_snapback_transaction_for_discontinuity(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    generation: u32,
    detected_sequence: u16,
) -> crate::movement_correction::MovementCorrection {
    let mut locked = state.lock().expect("state");
    let life_seq = locked.clients[&player_id].life_seq;
    let proposal = crate::movement_correction::MovementCorrection {
        reason: crate::movement_correction::MovementCorrection::REASON_SUSTAINED_DRIFT,
        repel_window: None,
        server_session_id: crate::wire::server_session_id(),
        player_id,
        life_seq,
        correction_generation: generation,
        position_sequence: detected_sequence,
        server_tick: crate::wire::current_tick(),
        position_x: 8_000.25,
        position_y: 8_100.5,
        velocity_x: 12.5,
        velocity_y: -3.25,
        orientation: 0.5,
    };
    locked
        .clients
        .get_mut(&player_id)
        .expect("client")
        .pending_movement_correction = Some(crate::state::ServerMovementCorrection {
        proposal,
        detected_state: crate::state::ServerSimMovement {
            position_x: proposal.position_x,
            position_y: proposal.position_y,
            velocity_x: proposal.velocity_x,
            velocity_y: proposal.velocity_y,
            orientation: proposal.orientation,
            energy: 0.0,
            max_energy: 0.0,
        },
        replay_witness: None,
        replay_server_tick: proposal.server_tick,
        predicted_repel_window: None,
        offer_reliable_id: Some(u32::MAX),
        application_sequence: None,
        terminal_state: None,
        terminal_server_tick: None,
        cancelled: false,
        receipt_reliable_id: None,
    });
    proposal
}

#[test]
fn exact_snapback_application_in_flight_across_death_is_inert_and_narrowly_scoped() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    drain_available(&rx);
    let proposal = arm_snapback_transaction_for_discontinuity(&state, player_id, 41, 700);
    let old_runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(player_id)
        .expect("old runtime");
    let successor = state
        .lock()
        .expect("state")
        .advance_client_life_at_tick(player_id, proposal.server_tick.wrapping_add(1))
        .expect("death advances life");
    assert_ne!(successor, old_runtime.life);

    let application = crate::movement_correction::MovementCorrectionApplied {
        intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
        server_session_id: proposal.server_session_id,
        player_id,
        life_seq: proposal.life_seq,
        correction_generation: proposal.correction_generation,
        detected_sequence: proposal.position_sequence,
        application_sequence: proposal.position_sequence.wrapping_add(1),
    };
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert!(client.pending_movement_correction.is_none());
        assert_eq!(client.retired_movement_corrections.len(), 1);
        assert_eq!(
            crate::connection::movement_correction_application_scope(
                &locked,
                player_id,
                application,
            ),
            crate::connection::MovementCorrectionApplicationScope::ExactRetired
        );
        for forged in [
            crate::movement_correction::MovementCorrectionApplied {
                server_session_id: application.server_session_id.wrapping_add(1).max(1),
                ..application
            },
            crate::movement_correction::MovementCorrectionApplied {
                player_id: player_id.wrapping_add(1),
                ..application
            },
            crate::movement_correction::MovementCorrectionApplied {
                life_seq: application.life_seq.wrapping_add(2).max(1),
                ..application
            },
            crate::movement_correction::MovementCorrectionApplied {
                correction_generation: application.correction_generation.wrapping_add(1).max(1),
                ..application
            },
            crate::movement_correction::MovementCorrectionApplied {
                detected_sequence: application.detected_sequence.wrapping_add(1),
                ..application
            },
        ] {
            assert_eq!(
                crate::connection::movement_correction_application_scope(
                    &locked, player_id, forged,
                ),
                crate::connection::MovementCorrectionApplicationScope::Invalid
            );
        }
    }

    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable(0, &application.encode()),
        "late-correction-after-death",
        &mut joined,
    );
    let locked = state.lock().expect("state");
    assert_eq!(locked.clients[&player_id].life_seq, successor.life_seq);
    assert!(
        locked.clients[&player_id]
            .pending_movement_correction
            .is_none()
    );
    assert!(locked.pending_reliable_gameplay.is_empty());
    assert_eq!(locked.movement_snapbacks_total, 0);
    drop(locked);
    assert!(
        rx.try_iter()
            .any(|packet| crate::reliable::reliable_ack_ids(&packet) == Some(vec![0]))
    );
}

fn assert_snapback_transaction_cancelled(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    proposal: crate::movement_correction::MovementCorrection,
) {
    let locked = state.lock().expect("state");
    let transaction = locked.clients[&player_id]
        .pending_movement_correction
        .as_ref()
        .expect("discontinuity keeps a terminal transaction");
    assert_eq!(transaction.proposal, proposal);
    assert!(transaction.cancelled);
    assert_eq!(
        transaction
            .terminal_receipt()
            .expect("cancel terminal")
            .outcome,
        crate::movement_correction::MovementCorrectionAccepted::OUTCOME_CANCELLED
    );
}

fn seed_snapback_position_fixture(
    state: &Arc<Mutex<ServerState>>,
    player_id: u16,
    accumulated_drift_x: f32,
) -> (u16, u16) {
    let mut state = state.lock().expect("server state should not be poisoned");
    state.config.movement_snapback = true;
    state.config.sim_observe = true;
    let (x, y) = crate::zone::server_spawn_coordinates(state.zone_map());
    let initial_sequence = 1;
    let mut initial_packet = client_position_packet_with_sequence(0, x, y, initial_sequence);
    initial_packet[1] = 0;
    let position = large_position_from_client(player_id, &initial_packet)
        .expect("fixture position should translate");
    state.set_client_position(player_id, position);
    let client = state
        .clients
        .get_mut(&player_id)
        .expect("player should exist");
    let anchor_tick = crate::wire::current_tick();
    client.last_position_server_tick = Some(anchor_tick);
    client.last_energy_recharge_tick = Some(anchor_tick);
    client.server_sim_state = Some(crate::state::ServerSimMovement {
        position_x: x as f32,
        position_y: y as f32,
        orientation: 0.0,
        velocity_x: 0.0,
        velocity_y: 0.0,
        energy: client.server_energy as f32,
        max_energy: client.server_max_energy as f32,
    });
    client.server_sim_anchor_tick = Some(crate::continuum::continuum_tick_sub(anchor_tick, 1));
    let input = crate::state::ClientInputObservation {
        bits: 0,
        direction: 0,
    };
    client.last_input_seq = Some(initial_sequence);
    client.last_input_record = Some(input);
    client
        .recent_input_records
        .push_back(crate::state::SequencedInputObservation {
            sequence: initial_sequence,
            input,
            door_seed: 0,
            door_phase_tick: 0,
        });
    client.last_position_sequence = Some(initial_sequence);
    let exact_state = crate::state::ServerSimMovement {
        position_x: x as f32,
        position_y: y as f32,
        orientation: 0.0,
        velocity_x: 0.0,
        velocity_y: 0.0,
        energy: client.server_energy as f32,
        max_energy: client.server_max_energy as f32,
    };
    let witness = crate::state::CanonicalPositionWitness {
        sequence: initial_sequence,
        life_seq: client.life_seq,
        state: exact_state,
        wire_x: x,
        wire_y: y,
        wire_velocity_x: 0,
        wire_velocity_y: 0,
        direction: 0,
        door_seed: 0,
        door_phase_tick: 0,
    };
    client.canonical_movement_witness = Some(witness);
    seed_fixture_motion_baseline(client);
    client.canonical_position_witnesses.push_back(witness);
    client.server_sim_input_bits = Some(0);
    client.movement_drift_x = accumulated_drift_x;
    client.movement_drift_y = 0.0;
    (x, y)
}

#[test]
fn snapback_offer_survives_delay_and_commits_at_later_fire_tail_exactly_once() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    let (base_x, base_y) = seed_snapback_position_fixture(
        &state,
        alpha,
        crate::sim_observation::MOVEMENT_DRIFT_SNAP_PX + 2.0,
    );

    let mut joined = true;
    let sequence = 2;
    crate::connection::handle_client_packet(
        &state,
        alpha,
        build_input_record_packet(sequence, &[(0, 0, 0)]),
        "test-peer",
        &mut joined,
    );
    let mut packet = client_position_packet_with_sequence(0, base_x, base_y, sequence);
    packet[1] = 0;
    sync_client_position_canonical_state_from_wire(&mut packet);
    let runtime = state
        .lock()
        .expect("server state")
        .active_player_runtime_identity(alpha)
        .expect("joined runtime");
    let relay = crate::arena::accept_position_relay_for_runtime_at_tick(
        &state,
        runtime,
        &packet,
        crate::wire::current_tick(),
    )
    .expect("exact legal tail should be admitted")
    .1;
    let correction = relay
        .movement_correction
        .expect("sustained accumulated drift should emit an owner correction");
    assert_eq!(correction.player_id, alpha);
    assert_eq!(correction.life_seq, runtime.life.life_seq);
    assert_eq!(correction.position_sequence, sequence);
    assert_eq!(
        correction.server_session_id,
        crate::wire::server_session_id()
    );
    assert_ne!(correction.correction_generation, 0);
    assert_eq!(
        u16::from_le_bytes(relay.position[4..6].try_into().unwrap()),
        base_x,
        "detection must leave the old canonical lineage committed",
    );
    {
        let locked = state.lock().expect("server state");
        assert_eq!(locked.movement_snapbacks_total, 0);
        assert_eq!(locked.movement_snapbacks_since_summary, 0);
        assert_eq!(
            locked.clients[&alpha]
                .pending_movement_correction
                .as_ref()
                .map(|transaction| transaction.proposal),
            Some(correction)
        );
    }

    assert!(crate::reliable::send_movement_correction_offer_to_runtime(
        &state, runtime, correction,
    ));
    let offer_packet = loop {
        let packet = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("expected correction offer");
        if crate::reliable::reliable_parts(&packet).is_some_and(|(_, payload)| {
            payload.first() == Some(&crate::movement_correction::MovementCorrection::TYPE)
        }) {
            break packet;
        }
    };
    let (offer_id, delivered) =
        crate::reliable::reliable_parts(&offer_packet).expect("offer is reliable");
    assert_eq!(
        crate::movement_correction::MovementCorrection::parse(&delivered),
        Ok(correction),
    );

    // Lose the first offer copy. The same reliable identity must survive and
    // recover after ordinary retransmission while old-lineage movement keeps
    // being admitted.
    crate::reliable::resend_due_reliable_to_clients(
        &state,
        Instant::now() + Duration::from_secs(4),
    );
    assert!(
        rx.try_iter().any(|packet| packet == offer_packet),
        "the exact correction offer must be retained for retransmission"
    );
    crate::connection::handle_client_packet(
        &state,
        alpha,
        crate::reliable::reliable_ack(offer_id),
        "snapback-offer-ack-peer",
        &mut joined,
    );

    // Advance seven normal tails after detection and consume a weapon on the
    // final old-lineage tail. Committing the correction must not erase the
    // projectile, energy drain, or inventory mutation that authority already
    // accepted before the ordered application marker.
    let tick = crate::wire::current_tick();
    let projectiles_before = state.lock().expect("state").active_projectiles.len();
    let mut fire_energy = None;
    for delayed_sequence in 3..=9 {
        let input_tick =
            crate::continuum::continuum_tick_add(tick, u32::from(delayed_sequence - 2));
        let (_, input_packet) = build_input_record_packet_with_valid_combat_view(
            &state,
            alpha,
            delayed_sequence,
            &[(0, 0, 0)],
            input_tick,
        );
        assert!(crate::connection::record_client_input_observation_at_tick(
            &state,
            runtime,
            &input_packet,
            input_tick,
        ));
        let mut delayed = client_position_packet_with_sequence(0, base_x, base_y, delayed_sequence);
        delayed[1] = 0;
        if delayed_sequence == 9 {
            delayed[20..22].copy_from_slice(&2_u16.to_le_bytes());
        }
        sync_client_position_canonical_state_from_wire(&mut delayed);
        crate::arena::accept_position_relay_for_runtime_at_tick(
            &state, runtime, &delayed, input_tick,
        )
        .expect("delayed old-lineage tail should remain admissible");
        if delayed_sequence == 9 {
            fire_energy = Some(state.lock().expect("state").clients[&alpha].server_energy);
        }
    }
    assert_eq!(
        state.lock().expect("state").active_projectiles.len(),
        projectiles_before + 1,
        "the correction barrier must not consume the authoritative fire edge"
    );

    let application = crate::movement_correction::MovementCorrectionApplied {
        intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
        server_session_id: correction.server_session_id,
        player_id: correction.player_id,
        life_seq: correction.life_seq,
        correction_generation: correction.correction_generation,
        detected_sequence: correction.position_sequence,
        application_sequence: 9,
    };
    assert!(
        crate::arena::apply_movement_correction_application_at_tick(
            &mut state.lock().expect("state"),
            runtime,
            crate::movement_correction::MovementCorrectionApplied {
                correction_generation: correction.correction_generation.wrapping_add(1),
                ..application
            },
            crate::continuum::continuum_tick_add(tick, 8),
        )
        .is_none()
    );
    let receipt = crate::arena::apply_movement_correction_application_at_tick(
        &mut state.lock().expect("state"),
        runtime,
        application,
        crate::continuum::continuum_tick_add(tick, 8),
    )
    .expect("exact delayed application should commit");
    assert_eq!(receipt.application_sequence, 9);
    assert_eq!(
        crate::arena::apply_movement_correction_application_at_tick(
            &mut state.lock().expect("state"),
            runtime,
            application,
            crate::continuum::continuum_tick_add(tick, 9),
        ),
        Some(receipt),
        "duplicate application is idempotent"
    );
    {
        let locked = state.lock().expect("server state");
        assert_eq!(locked.movement_snapbacks_total, 1);
        assert_eq!(locked.movement_snapbacks_since_summary, 1);
        assert_eq!(locked.active_projectiles.len(), projectiles_before + 1);
        assert_eq!(locked.clients[&alpha].server_energy, fire_energy.unwrap());
        let witness = locked.clients[&alpha]
            .canonical_movement_witness
            .expect("application must establish the exact authority anchor");
        assert_eq!(witness.sequence, 9);
        assert_eq!(
            witness.state.position_x.to_bits(),
            correction.position_x.to_bits()
        );
        assert_eq!(
            witness.state.velocity_x.to_bits(),
            correction.velocity_x.to_bits()
        );
        assert_eq!(witness.state.energy, fire_energy.unwrap() as f32);
    }

    assert!(
        crate::reliable::send_movement_correction_receipt_to_runtime(&state, runtime, receipt,)
    );
    let receipt_packet = loop {
        let packet = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("expected correction receipt");
        if crate::reliable::reliable_parts(&packet).is_some_and(|(_, payload)| {
            payload.first() == Some(&crate::movement_correction::MovementCorrectionAccepted::TYPE)
        }) {
            break packet;
        }
    };
    let (receipt_id, receipt_payload) =
        crate::reliable::reliable_parts(&receipt_packet).expect("receipt is reliable");
    assert_eq!(
        crate::movement_correction::MovementCorrectionAccepted::parse(receipt_payload),
        Ok(receipt)
    );
    crate::reliable::resend_due_reliable_to_clients(
        &state,
        Instant::now() + Duration::from_secs(4),
    );
    assert!(
        rx.try_iter().any(|packet| packet == receipt_packet),
        "a lost application receipt must be retained exactly"
    );
    crate::connection::handle_client_packet(
        &state,
        alpha,
        crate::reliable::reliable_ack(receipt_id),
        "snapback-receipt-ack-peer",
        &mut joined,
    );
    assert!(
        state.lock().expect("state").clients[&alpha]
            .pending_movement_correction
            .is_none(),
        "only the exact receipt transport ACK retires the transaction"
    );
}

#[test]
fn delayed_sustained_drift_application_reanchors_door_phase_before_resume() {
    assert_correction_door_rebase(false, true);
}

#[test]
fn correction_application_relocates_a_hull_when_its_door_closes_while_pending() {
    assert_correction_door_rebase(true, false);
}

#[test]
fn correction_door_relocation_survives_pause_beyond_phase_history() {
    assert_correction_door_rebase(true, true);
}

fn assert_correction_door_rebase(close_over_hull: bool, long_pause: bool) {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    let challenge_tick = 100_u32;
    let delay_ticks = crate::arena_settings::arena_door_delay_ticks().max(1) as u32;
    let door_state = crate::doors::ClientDoorState::for_challenge(challenge_tick);
    let (anchor_tick, application_tick, anchor_position) = if close_over_hull {
        let map = zone_map();
        (1..512_u32)
            .find_map(|count| {
                let before =
                    crate::continuum::continuum_tick_add(challenge_tick, count * delay_ticks);
                let elapsed_phases = if long_pause {
                    crate::doors::MAX_DOOR_PHASE_RECONSTRUCTION_STEPS as u32 + 8
                } else {
                    1
                };
                let after =
                    crate::continuum::continuum_tick_add(before, delay_ticks * elapsed_phases);
                let phase_at = |tick| {
                    crate::doors::client_door_phase_candidates_at_tick(Some(door_state), tick)
                        .into_iter()
                        .find(|phase| phase.started_tick == tick)
                };
                let phases = [phase_at(before)?, phase_at(after)?];
                map.tiles.iter().enumerate().find_map(|(index, tile)| {
                    if !(162..=169).contains(tile)
                        || crate::zone::position_blocking_tile_with_door_seed(*tile, phases[0].seed)
                        || !crate::zone::position_blocking_tile_with_door_seed(
                            *tile,
                            phases[1].seed,
                        )
                    {
                        return None;
                    }
                    let x = ((index % MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5) as u16;
                    let y = ((index / MAP_TILES) as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5) as u16;
                    let blocked = reported_position_hull_overlaps_blocked_tile(
                        x,
                        y,
                        arena_ship_radius_pixels(0),
                        MAP_TILES,
                        TILE_SIZE_PX,
                        |tx, ty| {
                            map.tiles.get(ty * MAP_TILES + tx).is_none_or(|tile| {
                                crate::zone::position_blocking_tile_with_door_seed(
                                    *tile,
                                    phases[0].seed,
                                )
                            })
                        },
                    );
                    (!blocked).then_some((before, after, Some((x, y))))
                })
            })
            .expect("authored map has a fitting hull in a door that next closes")
    } else {
        (
            challenge_tick,
            crate::continuum::continuum_tick_add(
                challenge_tick,
                delay_ticks
                    .saturating_mul(crate::doors::MAX_DOOR_PHASE_RECONSTRUCTION_STEPS as u32 + 8),
            ),
            None,
        )
    };
    let anchor_phase =
        crate::doors::client_door_phase_candidates_at_tick(Some(door_state), anchor_tick)
            .into_iter()
            .find(|phase| phase.started_tick == anchor_tick)
            .expect("challenge tick is an exact authenticated door phase");
    let application_phase =
        crate::doors::client_door_phase_candidates_at_tick(Some(door_state), application_tick)
            .into_iter()
            .filter(|phase| {
                crate::continuum::continuum_tick_diff(application_tick, phase.started_tick) >= 0
            })
            .min_by_key(|phase| {
                crate::continuum::continuum_tick_elapsed(application_tick, phase.started_tick)
            })
            .expect("application tick has a current authenticated door phase");
    assert!(
        crate::doors::client_door_phase_sequence(
            Some(door_state),
            anchor_phase.started_tick,
            application_phase.started_tick,
        )
        .is_none()
            == long_pause,
        "fixture must exceed the bounded predecessor-phase reconstruction horizon"
    );

    let detected_sequence = 500_u16;
    let application_sequence = detected_sequence.wrapping_add(1);
    let runtime;
    let correction;
    {
        let mut locked = state.lock().expect("state");
        runtime = locked
            .active_player_runtime_identity(player_id)
            .expect("runtime");
        let (spawn_x, spawn_y) = anchor_position
            .unwrap_or_else(|| crate::zone::server_spawn_coordinates(locked.zone_map()));
        let mut position =
            client_position_packet_with_sequence(0, spawn_x, spawn_y, detected_sequence);
        sync_client_position_canonical_state_from_wire(&mut position);
        locked.set_client_position(
            player_id,
            large_position_from_client(player_id, &position).expect("position"),
        );
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.door_state = Some(door_state);
        client.last_position_sequence = Some(detected_sequence);
        client.last_position_server_tick = Some(anchor_tick);
        let movement = crate::state::ServerSimMovement {
            position_x: spawn_x as f32,
            position_y: spawn_y as f32,
            velocity_x: 0.0,
            velocity_y: 0.0,
            orientation: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let witness = crate::state::CanonicalPositionWitness {
            sequence: detected_sequence,
            life_seq: client.life_seq,
            state: movement,
            wire_x: spawn_x,
            wire_y: spawn_y,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 0,
            door_seed: anchor_phase.seed,
            door_phase_tick: anchor_phase.started_tick,
        };
        client.canonical_position_witnesses.clear();
        client.canonical_position_witnesses.push_back(witness);
        client.canonical_movement_witness = Some(witness);
        seed_fixture_motion_baseline(client);
        client.server_sim_state = Some(movement);
        client.server_sim_anchor_tick = Some(anchor_tick);
        let input = crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        };
        client.last_input_seq = Some(application_sequence);
        client.last_input_record = Some(input);
        client.recent_input_records.clear();
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: application_sequence,
                input,
                door_seed: anchor_phase.seed,
                door_phase_tick: anchor_phase.started_tick,
            });
        correction = crate::movement_correction::MovementCorrection {
            reason: crate::movement_correction::MovementCorrection::REASON_SUSTAINED_DRIFT,
            repel_window: None,
            server_session_id: crate::wire::server_session_id(),
            player_id,
            life_seq: client.life_seq,
            correction_generation: 1,
            position_sequence: detected_sequence,
            server_tick: anchor_tick,
            position_x: movement.position_x,
            position_y: movement.position_y,
            velocity_x: movement.velocity_x,
            velocity_y: movement.velocity_y,
            orientation: movement.orientation,
        };
        client.pending_movement_correction = Some(crate::state::ServerMovementCorrection {
            proposal: correction,
            detected_state: movement,
            replay_witness: Some(witness),
            replay_server_tick: anchor_tick,
            predicted_repel_window: None,
            offer_reliable_id: Some(1),
            application_sequence: None,
            terminal_state: None,
            terminal_server_tick: None,
            cancelled: false,
            receipt_reliable_id: None,
        });
    }

    let receipt = crate::arena::apply_movement_correction_application_at_tick(
        &mut state.lock().expect("state"),
        runtime,
        crate::movement_correction::MovementCorrectionApplied {
            intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
            server_session_id: correction.server_session_id,
            player_id,
            life_seq: runtime.life.life_seq,
            correction_generation: correction.correction_generation,
            detected_sequence,
            application_sequence,
        },
        application_tick,
    )
    .expect("delayed exact application commits");
    if close_over_hull {
        let locked = state.lock().expect("state");
        let (spawn_x, spawn_y) = crate::zone::server_spawn_coordinates(locked.zone_map());
        assert_eq!(
            (receipt.position_x, receipt.position_y),
            (spawn_x as f32, spawn_y as f32),
            "a current-phase correction cannot commit a hull inside a newly closed door"
        );
        assert_eq!((receipt.velocity_x, receipt.velocity_y), (0.0, 0.0));
    }
    assert_eq!(
        receipt.outcome,
        crate::movement_correction::MovementCorrectionAccepted::OUTCOME_APPLIED
    );
    {
        let locked = state.lock().expect("state");
        let witness = locked.clients[&player_id]
            .canonical_movement_witness
            .expect("correction witness");
        assert_eq!(witness.door_seed, application_phase.seed);
        assert_eq!(witness.door_phase_tick, application_phase.started_tick);
    }

    let next_sequence = application_sequence.wrapping_add(1);
    let next_tick = crate::continuum::continuum_tick_add(application_tick, 1);
    let next_phase =
        crate::doors::client_door_phase_candidates_at_tick(Some(door_state), next_tick)
            .into_iter()
            .filter(|phase| {
                crate::continuum::continuum_tick_diff(next_tick, phase.started_tick) >= 0
            })
            .min_by_key(|phase| {
                crate::continuum::continuum_tick_elapsed(next_tick, phase.started_tick)
            })
            .expect("next input has a current authenticated phase");
    assert!(crate::connection::record_client_input_observation_at_tick(
        &state,
        runtime,
        &build_input_record_packet(next_sequence, &[(0, 0, next_phase.seed)]),
        next_tick,
    ));
    let locked = state.lock().expect("state");
    assert!(
        crate::sim_observation::replay_canonical_position_gap(
            &locked.clients[&player_id],
            locked.zone_map(),
            false,
            false,
            false,
            crate::sim_observation::movement_upgrades_for_client(&locked.clients[&player_id],),
            &[],
            next_sequence,
            next_tick,
            crate::zone::server_spawn_coordinates(locked.zone_map()).0,
            crate::zone::server_spawn_coordinates(locked.zone_map()).1,
        )
        .is_some(),
        "the first current-phase post-receipt input must replay from the corrected lineage"
    );
}

#[test]
fn snapback_application_sequence_wrap_is_forward_and_identity_bound() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    let (base_x, base_y) = seed_snapback_position_fixture(
        &state,
        alpha,
        crate::sim_observation::MOVEMENT_DRIFT_SNAP_PX + 2.0,
    );
    let runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(alpha)
        .expect("runtime");
    let proposal = crate::movement_correction::MovementCorrection {
        reason: crate::movement_correction::MovementCorrection::REASON_SUSTAINED_DRIFT,
        repel_window: None,
        server_session_id: crate::wire::server_session_id(),
        player_id: alpha,
        life_seq: runtime.life.life_seq,
        correction_generation: u32::MAX,
        position_sequence: u16::MAX,
        server_tick: crate::wire::current_tick(),
        position_x: base_x as f32 + 0.25,
        position_y: base_y as f32 + 0.5,
        velocity_x: 12.5,
        velocity_y: -3.25,
        orientation: 0.5,
    };
    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&alpha).expect("client");
        let mut current_tail = client
            .canonical_movement_witness
            .expect("seeded exact witness");
        current_tail.sequence = 0;
        client.last_position_sequence = Some(0);
        client.canonical_movement_witness = Some(current_tail);
        seed_fixture_motion_baseline(client);
        client.canonical_position_witnesses.push_back(current_tail);
        client.pending_movement_correction = Some(crate::state::ServerMovementCorrection {
            proposal,
            detected_state: crate::state::ServerSimMovement {
                position_x: base_x as f32,
                position_y: base_y as f32,
                velocity_x: 0.0,
                velocity_y: 0.0,
                orientation: 0.0,
                energy: 0.0,
                max_energy: 0.0,
            },
            replay_witness: Some(crate::state::CanonicalPositionWitness {
                sequence: 0,
                life_seq: proposal.life_seq,
                state: crate::state::ServerSimMovement {
                    position_x: proposal.position_x,
                    position_y: proposal.position_y,
                    velocity_x: proposal.velocity_x,
                    velocity_y: proposal.velocity_y,
                    orientation: proposal.orientation,
                    energy: 0.0,
                    max_energy: 0.0,
                },
                wire_x: proposal.position_x.round() as u16,
                wire_y: proposal.position_y.round() as u16,
                wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                    proposal.velocity_x,
                ),
                wire_velocity_y: crate::continuum_position::protocol_velocity_component(
                    proposal.velocity_y,
                ),
                direction: crate::continuum_position::protocol_direction(proposal.orientation),
                door_seed: current_tail.door_seed,
                door_phase_tick: current_tail.door_phase_tick,
            }),
            replay_server_tick: crate::wire::current_tick(),
            predicted_repel_window: None,
            offer_reliable_id: Some(u32::MAX),
            application_sequence: None,
            terminal_state: None,
            terminal_server_tick: None,
            cancelled: false,
            receipt_reliable_id: None,
        });
    }
    let application = crate::movement_correction::MovementCorrectionApplied {
        intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
        server_session_id: proposal.server_session_id,
        player_id: proposal.player_id,
        life_seq: proposal.life_seq,
        correction_generation: proposal.correction_generation,
        detected_sequence: u16::MAX,
        application_sequence: 0,
    };
    let receipt = crate::arena::apply_movement_correction_application_at_tick(
        &mut state.lock().expect("state"),
        runtime,
        application,
        crate::wire::current_tick(),
    )
    .expect("wrapped application sequence is one forward step");
    assert_eq!(
        receipt.outcome,
        crate::movement_correction::MovementCorrectionAccepted::OUTCOME_APPLIED
    );
    assert_eq!(receipt.application_sequence, 0);
    assert_eq!(
        state.lock().expect("state").clients[&alpha].last_position_sequence,
        Some(0)
    );
}

#[test]
fn half_range_snapback_application_gets_a_durable_cancel_terminal() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(player_id)
        .expect("runtime");
    let proposal = arm_snapback_transaction_for_discontinuity(&state, player_id, 17, 9);
    let application = crate::movement_correction::MovementCorrectionApplied {
        intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
        server_session_id: proposal.server_session_id,
        player_id,
        life_seq: proposal.life_seq,
        correction_generation: proposal.correction_generation,
        detected_sequence: proposal.position_sequence,
        application_sequence: proposal.position_sequence.wrapping_add(1 << 15),
    };
    let terminal = crate::arena::apply_movement_correction_application_at_tick(
        &mut state.lock().expect("state"),
        runtime,
        application,
        crate::wire::current_tick(),
    )
    .expect("half-range ambiguity must terminate the browser barrier");
    assert_eq!(
        terminal.outcome,
        crate::movement_correction::MovementCorrectionAccepted::OUTCOME_CANCELLED
    );
    assert_eq!(
        crate::arena::apply_movement_correction_application_at_tick(
            &mut state.lock().expect("state"),
            runtime,
            application,
            crate::wire::current_tick(),
        ),
        Some(terminal),
        "the cancellation outcome is idempotent"
    );
}

#[test]
fn client_newer_boundary_cancels_snapback_without_mutating_lineage() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    let runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(player_id)
        .expect("runtime");
    let proposal = arm_snapback_transaction_for_discontinuity(&state, player_id, 18, 90);
    let before = state.lock().expect("state").clients[&player_id]
        .last_position
        .clone();
    let application = crate::movement_correction::MovementCorrectionApplied {
        intent: crate::movement_correction::MovementCorrectionApplied::INTENT_CANCEL_NEWER_BOUNDARY,
        server_session_id: proposal.server_session_id,
        player_id,
        life_seq: proposal.life_seq,
        correction_generation: proposal.correction_generation,
        detected_sequence: proposal.position_sequence,
        application_sequence: proposal.position_sequence.wrapping_add(7),
    };
    let terminal = crate::arena::apply_movement_correction_application_at_tick(
        &mut state.lock().expect("state"),
        runtime,
        application,
        crate::wire::current_tick(),
    )
    .expect("a newer local boundary must receive a durable cancel terminal");
    assert_eq!(
        terminal.outcome,
        crate::movement_correction::MovementCorrectionAccepted::OUTCOME_CANCELLED
    );
    assert_eq!(
        terminal.application_sequence,
        application.application_sequence
    );
    let locked = state.lock().expect("state");
    assert_eq!(locked.clients[&player_id].last_position, before);
    assert_eq!(locked.movement_snapbacks_total, 0);
    drop(locked);
    assert_eq!(
        crate::arena::apply_movement_correction_application_at_tick(
            &mut state.lock().expect("state"),
            runtime,
            application,
            crate::wire::current_tick(),
        ),
        Some(terminal),
        "a duplicate boundary cancellation is idempotent"
    );
}

#[test]
fn cancelled_snapback_terminal_survives_loss_reorder_duplicate_and_id_wrap() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);
    acknowledge_join_bootstrap(&state, player_id);
    drain_available(&rx);
    let runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(player_id)
        .expect("runtime");
    let proposal = crate::movement_correction::MovementCorrection {
        reason: crate::movement_correction::MovementCorrection::REASON_SUSTAINED_DRIFT,
        repel_window: None,
        server_session_id: crate::wire::server_session_id(),
        player_id,
        life_seq: runtime.life.life_seq,
        correction_generation: u32::MAX,
        position_sequence: u16::MAX,
        server_tick: crate::wire::current_tick(),
        position_x: 8_000.25,
        position_y: 8_100.5,
        velocity_x: 12.5,
        velocity_y: -3.25,
        orientation: 0.5,
    };
    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.outgoing_reliable.set_next_id_for_test(u32::MAX);
        client.pending_movement_correction = Some(crate::state::ServerMovementCorrection {
            proposal,
            detected_state: crate::state::ServerSimMovement {
                position_x: proposal.position_x,
                position_y: proposal.position_y,
                velocity_x: proposal.velocity_x,
                velocity_y: proposal.velocity_y,
                orientation: proposal.orientation,
                energy: 0.0,
                max_energy: 0.0,
            },
            replay_witness: None,
            replay_server_tick: proposal.server_tick,
            predicted_repel_window: None,
            offer_reliable_id: None,
            application_sequence: None,
            terminal_state: None,
            terminal_server_tick: None,
            cancelled: false,
            receipt_reliable_id: None,
        });
    }

    assert!(crate::reliable::send_movement_correction_offer_to_runtime(
        &state, runtime, proposal,
    ));
    let offer = rx.recv_timeout(Duration::from_secs(1)).expect("offer");
    assert_eq!(
        crate::reliable::reliable_parts(&offer).map(|parts| parts.0),
        Some(u32::MAX)
    );

    {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.reset_server_movement_tracking_for_discontinuity();
        client.reset_server_movement_tracking_for_discontinuity();
        assert!(
            client
                .pending_movement_correction
                .as_ref()
                .is_some_and(|transaction| transaction.cancelled)
        );
    }
    crate::reliable::retry_pending_movement_correction_delivery(&state);
    let terminal_packet = loop {
        let packet = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("cancel terminal");
        if crate::reliable::reliable_parts(&packet).is_some_and(|(_, payload)| {
            payload.first() == Some(&crate::movement_correction::MovementCorrectionAccepted::TYPE)
        }) {
            break packet;
        }
    };
    let (terminal_id, terminal_payload) =
        crate::reliable::reliable_parts(&terminal_packet).expect("reliable terminal");
    assert_eq!(terminal_id, 0, "receipt allocation wraps after the offer");
    let terminal = crate::movement_correction::MovementCorrectionAccepted::parse(terminal_payload)
        .expect("cancel terminal parses");
    assert_eq!(
        terminal.outcome,
        crate::movement_correction::MovementCorrectionAccepted::OUTCOME_CANCELLED
    );
    assert_eq!(terminal.detected_sequence, u16::MAX);

    let application = crate::movement_correction::MovementCorrectionApplied {
        intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
        server_session_id: proposal.server_session_id,
        player_id,
        life_seq: proposal.life_seq,
        correction_generation: proposal.correction_generation,
        detected_sequence: proposal.position_sequence,
        application_sequence: 0,
    };
    assert_eq!(
        crate::arena::apply_movement_correction_application_at_tick(
            &mut state.lock().expect("state"),
            runtime,
            application,
            crate::wire::current_tick(),
        ),
        Some(terminal),
        "an overtaken application receives the same idempotent terminal"
    );

    crate::reliable::resend_due_reliable_to_clients(
        &state,
        Instant::now() + Duration::from_secs(4),
    );
    assert!(
        rx.try_iter().any(|packet| packet == terminal_packet),
        "a lost cancellation terminal remains recoverable"
    );
    let mut joined = true;
    crate::connection::handle_client_packet(
        &state,
        player_id,
        crate::reliable::reliable_ack(terminal_id),
        "cancelled-snapback-peer",
        &mut joined,
    );
    assert!(
        state.lock().expect("state").clients[&player_id]
            .pending_movement_correction
            .is_none()
    );
}

#[test]
fn enabled_snapback_leaves_honest_jitter_coordinates_untouched() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    let (base_x, base_y) = seed_snapback_position_fixture(&state, alpha, 0.0);

    let mut joined = true;
    let sequence = 2;
    crate::connection::handle_client_packet(
        &state,
        alpha,
        build_input_record_packet(sequence, &[(0, 0, 0)]),
        "test-peer",
        &mut joined,
    );
    let mut packet = client_position_packet_with_sequence(0, base_x + 1, base_y - 1, sequence);
    packet[1] = 0;
    sync_client_position_canonical_state_from_wire(&mut packet);
    let before = witness_admission_state(&state, alpha);
    assert!(
        accept_position_relay(&state, alpha, &packet).is_none(),
        "exact provenance rejects even a one-pixel attacker-selected drift tail"
    );
    assert_eq!(witness_admission_state(&state, alpha), before);
}

#[test]
fn insufficient_energy_bomb_recoil_is_rejected_and_exactly_rebased_without_a_projectile() {
    predicted_bomb_recoil_rejection_fixture(false, false);
}

#[test]
fn proximity_safety_race_bomb_recoil_is_rejected_and_exactly_rebased_without_a_projectile() {
    predicted_bomb_recoil_rejection_fixture(true, false);
}

#[test]
fn cooldown_bomb_recoil_is_rejected_and_exactly_rebased_without_a_projectile() {
    predicted_bomb_recoil_rejection_fixture(false, true);
}

fn predicted_bomb_recoil_rejection_fixture(safety_race: bool, cooldown_race: bool) {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 1);
    let previous_sequence = 24_000_u16;
    let late_predecessor_sequence = previous_sequence + 1;
    let rejected_sequence = previous_sequence + 2;
    let suffix_sequence = previous_sequence + 3;
    let anchor_tick = 9_000_u32;
    let (start_x, start_y) = {
        let map = zone_map();
        (2..MAP_TILES - 2)
            .find_map(|tile_y| {
                (2..MAP_TILES - 2).find_map(|tile_x| {
                    let x = (tile_x as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5).round() as u16;
                    let y = (tile_y as f32 * TILE_SIZE_PX + TILE_SIZE_PX * 0.5).round() as u16;
                    (!crate::zone::world_position_in_safe_tile_for_map(map, x, y)
                        && !reported_position_hull_overlaps_blocked_tile(
                            x,
                            y,
                            arena_ship_radius_pixels(1),
                            MAP_TILES,
                            TILE_SIZE_PX,
                            |hull_x, hull_y| {
                                map.tiles
                                    .get(hull_y * MAP_TILES + hull_x)
                                    .is_none_or(|&tile| position_blocking_tile(tile))
                            },
                        ))
                    .then_some((x, y))
                })
            })
            .expect("Chaos Zone contains a clear non-safe bomb fixture")
    };
    let runtime = {
        let mut locked = state.lock().expect("state");
        let client = locked.clients.get_mut(&player_id).expect("client");
        client.reset_server_runtime_for_ship_at_tick(anchor_tick);
        grant_legacy_capture_loadout(client, anchor_tick);
        client.life_position_sequence_base = None;
        client.alive = true;
        client.respawn_at_tick = None;
        client.server_energy = if safety_race || cooldown_race {
            1_700
        } else {
            1
        };
        client.server_energy_remainder = 0.0;
        client.last_energy_recharge_tick = Some(anchor_tick);
        client.server_weapons.bomb_level = 4;
        client.server_weapons.proximity_bombs = safety_race;
        if cooldown_race {
            client.last_gun_fire_tick = Some(anchor_tick);
            client.last_weapon_fire_tick = Some(anchor_tick);
            client.last_accepted_weapon_data = Some(1);
            client.last_accepted_weapon_tick = Some(anchor_tick);
            client.remember_accepted_weapon_witness(previous_sequence);
        }

        let mut previous =
            client_position_packet_with_sequence(0, start_x, start_y, previous_sequence);
        previous[16..18].copy_from_slice(&client.server_bounty.to_le_bytes());
        previous[18..20].copy_from_slice(&client.server_energy.to_le_bytes());
        client.last_position =
            Some(large_position_from_client(player_id, &previous).expect("previous position"));
        client.last_position_sequence = Some(previous_sequence);
        client.last_position_server_tick = Some(anchor_tick);
        let state = crate::state::ServerSimMovement {
            position_x: start_x as f32,
            position_y: start_y as f32,
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: client.server_energy as f32,
            max_energy: client.server_max_energy as f32,
        };
        let witness = crate::state::CanonicalPositionWitness {
            sequence: previous_sequence,
            life_seq: client.life_seq,
            state,
            wire_x: start_x,
            wire_y: start_y,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 0,
            door_seed: 0,
            door_phase_tick: anchor_tick,
        };
        client.canonical_position_witnesses.clear();
        client.canonical_position_witnesses.push_back(witness);
        client.canonical_movement_witness = Some(witness);
        seed_fixture_motion_baseline(client);
        client.server_sim_state = Some(state);
        client.server_sim_anchor_tick = Some(anchor_tick);
        client.server_sim_input_bits = Some(0);
        let input = crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        };
        client.last_input_seq = Some(suffix_sequence);
        client.last_input_record = Some(input);
        client.recent_input_records.clear();
        for sequence in [
            late_predecessor_sequence,
            rejected_sequence,
            suffix_sequence,
        ] {
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence,
                    input,
                    door_seed: 0,
                    door_phase_tick: anchor_tick,
                });
        }
        locked
            .active_player_runtime_identity(player_id)
            .expect("runtime")
    };

    let (
        authoritative_predecessor,
        authoritative_tail,
        authoritative_suffix,
        predicted_recoil_tail,
        predicted_recoil_suffix,
        bounty,
        energy_before,
    ) = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        let upgrades = crate::sim_observation::PlayerMovementUpgrades {
            thrust: client.server_thrust_acceleration,
            top_speed: client.server_top_speed,
            rotation: client.server_rotation_per_second,
            recharge: client.server_recharge_per_second,
            live_values: true,
        };
        let replay_to_rejected = crate::sim_observation::replay_canonical_position_gap(
            client,
            locked.zone_map(),
            false,
            false,
            false,
            upgrades,
            &[],
            rejected_sequence,
            anchor_tick + 2,
            server_spawn_coordinates(locked.zone_map()).0,
            server_spawn_coordinates(locked.zone_map()).1,
        )
        .expect("authenticated movement replays");
        let authoritative_predecessor = replay_to_rejected
            .witnesses
            .iter()
            .find(|witness| witness.sequence == late_predecessor_sequence)
            .copied()
            .expect("reordered predecessor witness");
        let authoritative_tail = replay_to_rejected
            .witnesses
            .last()
            .copied()
            .expect("rejected authoritative tail");
        let mut predicted = replay_to_rejected;
        assert!(predicted.apply_bomb_recoil_to_tail(
            crate::arena_settings::arena_ship_bomb_recoil_speed(client.player.ship)
        ));
        let predicted_recoil_tail = predicted
            .witnesses
            .last()
            .copied()
            .expect("rejected recoil tail");
        let predicted_recoil_suffix =
            crate::sim_observation::replay_canonical_position_gap_from_witness(
                client,
                predicted_recoil_tail,
                anchor_tick + 2,
                locked.zone_map(),
                false,
                false,
                false,
                upgrades,
                &[],
                suffix_sequence,
                anchor_tick + 3,
                server_spawn_coordinates(locked.zone_map()).0,
                server_spawn_coordinates(locked.zone_map()).1,
                None,
                None,
            )
            .and_then(|replay| replay.witnesses.last().copied())
            .expect("predicted recoil lineage replays through its suffix");
        let authoritative_suffix = crate::sim_observation::replay_canonical_position_gap(
            client,
            locked.zone_map(),
            false,
            false,
            false,
            upgrades,
            &[],
            suffix_sequence,
            anchor_tick + 3,
            server_spawn_coordinates(locked.zone_map()).0,
            server_spawn_coordinates(locked.zone_map()).1,
        )
        .and_then(|replay| replay.witnesses.last().copied())
        .expect("authoritative no-recoil suffix");
        (
            authoritative_predecessor,
            authoritative_tail,
            authoritative_suffix,
            predicted_recoil_tail,
            predicted_recoil_suffix,
            client.server_bounty,
            client.server_energy,
        )
    };
    assert_ne!(
        authoritative_tail.state.velocity_y.to_bits(),
        predicted_recoil_tail.state.velocity_y.to_bits(),
        "fixture must isolate the rejected recoil discontinuity"
    );
    let bomb = crate::gameplay_rules::encode_weapon_data(
        if safety_race { 4 } else { 3 },
        2,
        false,
        0,
        0,
        false,
    );
    if safety_race {
        assert!(crate::arena_settings::arena_bomb_safety_enabled());
        assert!(energy_before > crate::arena_settings::arena_ship_weapon_energy_cost(1, bomb));
        // The client's older view was clear; authoritative arrival sees a
        // different-frequency enemy inside the proximity firing radius.
        let (target_tx, _target_rx) = mpsc::channel();
        let target = register_connection(&state, target_tx);
        join_arena(&state, target, 0);
        let mut locked = state.lock().expect("state");
        let source_frequency = locked.clients[&player_id].player.frequency;
        let target_client = locked.clients.get_mut(&target).expect("target");
        target_client.player.frequency = source_frequency.wrapping_add(1);
        let position = client_position_packet_with_sequence(0, start_x + 8, start_y, 1);
        target_client.last_position = Some(large_position_from_client(target, &position).unwrap());
    }
    let packet_for = |witness: crate::state::CanonicalPositionWitness, weapon: u16| {
        let mut packet = client_position_packet_with_sequence(
            0,
            witness.wire_x,
            witness.wire_y,
            witness.sequence,
        );
        packet[1] = witness.direction;
        packet[6..8].copy_from_slice(&witness.wire_velocity_x.to_le_bytes());
        packet[14..16].copy_from_slice(&witness.wire_velocity_y.to_le_bytes());
        packet[16..18].copy_from_slice(&bounty.to_le_bytes());
        packet[18..20].copy_from_slice(&0_u16.to_le_bytes());
        packet[20..22].copy_from_slice(&weapon.to_le_bytes());
        set_client_position_canonical_state(
            &mut packet,
            witness.state.position_x,
            witness.state.position_y,
            witness.state.velocity_x,
            witness.state.velocity_y,
            witness.state.orientation,
        );
        packet
    };
    let packet = packet_for(predicted_recoil_tail, bomb);
    // Isolate the rejection reason: missing observer provenance has its own
    // supported recovery and must not satisfy either energy or safety test.
    seed_combat_view_decision_for_test(&state, player_id, rejected_sequence, anchor_tick + 1);
    {
        let locked = state.lock().expect("state");
        assert!(
            locked.clients[&player_id]
                .observer_combat_timeline
                .bind_projectile(runtime, rejected_sequence, anchor_tick + 1,)
                .is_ok(),
            "the fixture must have admissible observer provenance"
        );
    }
    if safety_race || cooldown_race {
        let mut drifted = predicted_recoil_tail;
        drifted.state.position_x += 0.25;
        for forged in [
            packet_for(predicted_recoil_tail, 0),
            packet_for(drifted, bomb),
        ] {
            let rejects = crate::security::peek_security_reject_count_for_test(
                "position",
                "canonical_state_mismatch",
            );
            assert!(
                crate::arena::accept_position_relay_for_runtime_at_tick(
                    &state,
                    runtime,
                    &forged,
                    anchor_tick + 1,
                )
                .is_none()
            );
            let locked = state.lock().expect("state");
            assert!(
                locked.clients[&player_id]
                    .pending_movement_correction
                    .is_none(),
                "an unclaimed recoil or extra drift is not the exact rejected-bomb lineage"
            );
            assert_eq!(
                locked.clients[&player_id].last_position_sequence,
                Some(previous_sequence)
            );
            assert_eq!(locked.clients[&player_id].server_energy, energy_before);
            assert_eq!(
                crate::security::peek_security_reject_count_for_test(
                    "position",
                    "canonical_state_mismatch",
                ),
                rejects + 1,
                "forged motion must remain a security reject"
            );
        }
    }
    let canonical_rejects_before = crate::security::peek_security_reject_count_for_test(
        "position",
        "canonical_state_mismatch",
    );
    let projectiles_before = state.lock().expect("state").active_projectiles.len();
    assert!(
        crate::arena::accept_position_relay_for_runtime_at_tick(
            &state,
            runtime,
            &packet,
            anchor_tick + 1,
        )
        .is_none(),
        "the unaffordable or unsafe bomb and its movement packet stay rejected"
    );
    let proposal = {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert_eq!(client.server_energy, energy_before);
        assert_eq!(client.last_position_sequence, Some(previous_sequence));
        assert_eq!(locked.active_projectiles.len(), projectiles_before);
        let transaction = client
            .pending_movement_correction
            .as_ref()
            .expect("exact rejected recoil starts one bounded correction");
        assert_eq!(
            transaction.proposal.reason,
            crate::movement_correction::MovementCorrection::REASON_REJECTED_WEAPON_RECOIL
        );
        assert_eq!(transaction.proposal.position_sequence, rejected_sequence);
        assert_eq!(
            transaction.proposal.velocity_y.to_bits(),
            authoritative_tail.state.velocity_y.to_bits()
        );
        transaction.proposal
    };
    assert!(
        crate::arena::accept_position_relay_for_runtime_at_tick(
            &state,
            runtime,
            &packet_for(authoritative_predecessor, 0),
            anchor_tick + 2,
        )
        .is_some(),
        "an exact reordered predecessor remains ordinary accepted lineage"
    );
    {
        let locked = state.lock().expect("state");
        let client = &locked.clients[&player_id];
        assert_eq!(
            client.last_position_sequence,
            Some(late_predecessor_sequence)
        );
        let transaction = client
            .pending_movement_correction
            .as_ref()
            .expect("the predecessor cannot retire the correction");
        assert!(transaction.awaiting_application());
        assert_eq!(
            transaction
                .replay_witness
                .expect("corrected shadow")
                .sequence,
            rejected_sequence,
            "a predecessor cannot replay the corrected shadow backward"
        );
    }
    assert!(
        crate::arena::accept_position_relay_for_runtime_at_tick(
            &state,
            runtime,
            &packet,
            anchor_tick + 2,
        )
        .is_none(),
        "the duplicated rejected edge remains inert behind its correction"
    );
    assert!(
        crate::arena::accept_position_relay_for_runtime_at_tick(
            &state,
            runtime,
            &packet_for(predicted_recoil_suffix, 0),
            anchor_tick + 3,
        )
        .is_none(),
        "an exact predicted-recoil suffix remains inert behind its correction"
    );
    assert!(
        state.lock().expect("state").clients[&player_id]
            .pending_movement_correction
            .as_ref()
            .is_some_and(crate::state::ServerMovementCorrection::awaiting_application),
        "reordered predecessor, edge, and suffix cannot cancel the barrier"
    );
    let terminal = crate::arena::apply_movement_correction_application_at_tick(
        &mut state.lock().expect("state"),
        runtime,
        crate::movement_correction::MovementCorrectionApplied {
            intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
            server_session_id: proposal.server_session_id,
            player_id,
            life_seq: proposal.life_seq,
            correction_generation: proposal.correction_generation,
            detected_sequence: rejected_sequence,
            application_sequence: suffix_sequence,
        },
        anchor_tick + 3,
    )
    .expect("exact application commits the no-recoil lineage");
    assert_eq!(
        terminal.velocity_y.to_bits(),
        authoritative_suffix.state.velocity_y.to_bits()
    );
    let locked = state.lock().expect("state");
    assert_eq!(
        crate::security::peek_security_reject_count_for_test(
            "position",
            "canonical_state_mismatch",
        ),
        canonical_rejects_before,
        "the exact rejected prediction is recovery, not an attack"
    );
    assert_eq!(locked.active_projectiles.len(), projectiles_before);
    assert_eq!(locked.clients[&player_id].server_energy, energy_before);
    assert_eq!(
        locked.clients[&player_id]
            .canonical_movement_witness
            .expect("corrected witness")
            .state
            .velocity_y
            .to_bits(),
        authoritative_suffix.state.velocity_y.to_bits()
    );
}

#[test]
fn server_ignores_truncated_input_packet() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    join_arena(&state, player_id, 0);

    // Header claims 2 entries but body has only 1.
    let mut packet = vec![0x62, 0x05, 0x00, 0x02];
    packet.extend_from_slice(&[0x1f, 0x00, 5, 1]);
    let mut joined = true;
    crate::connection::handle_client_packet(&state, player_id, packet, "test-peer", &mut joined);

    let state = state.lock().expect("server state should not be poisoned");
    let client = state.clients.get(&player_id).expect("client present");
    assert!(client.last_input_seq.is_none());
    assert!(client.last_input_record.is_none());
}

#[test]
fn stale_weapon_position_with_input_hole_inside_window_is_transport_loss() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    reset_server_runtime_without_initial_bounty(&state, alpha);

    // Retain witnesses for 1, 2, 4 and 5 so sequence 3 is a hole strictly inside
    // the retained input window: exactly the shape one dropped C2S input record
    // leaves on the unordered movement channel.
    for sequence in [1_u16, 2, 4, 5] {
        record_position_witness(&state, alpha, sequence, 0);
    }

    let fresh = client_position_packet_with_sequence(0, 500, 700, 5);
    state
        .lock()
        .expect("state")
        .queue_position_input(alpha, fresh);
    crate::server_tick::run_server_tick(&state, Instant::now());
    assert_eq!(
        state.lock().expect("state").clients[&alpha].last_position_sequence,
        Some(5),
        "the fresh sequence carrying its own witness must be admitted"
    );

    let rejects_before =
        crate::security::peek_security_reject_count_for_test("position", "missing_input_record");
    reset_position_rate_limit(&state, alpha);

    // Sequence 3 is stale behind the admitted 5 and weapon-bearing, and its
    // input record cannot have aged out because newer records are still retained.
    let mut stale_hole = client_position_packet_with_sequence(0, 501, 701, 3);
    stale_hole[20..22].copy_from_slice(&2_u16.to_le_bytes());
    state
        .lock()
        .expect("state")
        .queue_position_input(alpha, stale_hole);
    crate::server_tick::run_server_tick(&state, Instant::now());

    let locked = state.lock().expect("state");
    assert_eq!(
        locked.clients[&alpha].last_position_sequence,
        Some(5),
        "the provably lost stale frame must stay inert"
    );
    assert_eq!(
        locked.active_projectiles.len(),
        0,
        "a dropped stale weapon mirror must not fire"
    );
    drop(locked);
    assert_eq!(
        crate::security::peek_security_reject_count_for_test("position", "missing_input_record"),
        rejects_before,
        "an input hole inside the retained window is transport loss, not an attack",
    );

    // A weapon-bearing stale frame behind the oldest retained input stays a
    // security reject: it cannot be told apart from a captured replay.
    let rejects_after_hole =
        crate::security::peek_security_reject_count_for_test("position", "missing_input_record");
    reset_position_rate_limit(&state, alpha);
    let mut aged_replay = client_position_packet_with_sequence(0, 502, 702, 0);
    aged_replay[20..22].copy_from_slice(&2_u16.to_le_bytes());
    state
        .lock()
        .expect("state")
        .queue_position_input(alpha, aged_replay);
    crate::server_tick::run_server_tick(&state, Instant::now());

    assert_eq!(
        crate::security::peek_security_reject_count_for_test("position", "missing_input_record"),
        rejects_after_hole + 1,
        "a stale weapon frame older than the retained window must stay rejected",
    );
    assert_eq!(
        state.lock().expect("state").clients[&alpha].last_position_sequence,
        Some(5),
        "the aged replay must also stay inert"
    );
}

#[test]
fn accepted_weapon_use_reveals_owned_cloak_but_preserves_stealth() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    reset_server_runtime_without_initial_bounty(&state, alpha);
    {
        let mut state = state.lock().unwrap();
        let client = state.clients.get_mut(&alpha).unwrap();
        client.server_status.cloak_capable = true;
        client.server_status.stealth_capable = true;
    }
    drain_available(&rx_a);
    drain_available(&rx_b);
    let mut shot = client_position_packet(0, 500, 700);
    shot[11] = POSITION_STATUS_CLOAK | POSITION_STATUS_STEALTH;
    shot[20..22].copy_from_slice(&1_u16.to_le_bytes());
    broadcast_position_observable(&state, alpha, &shot);
    let relayed = read_non_reliable_from_channel(&rx_b);
    assert_eq!(
        u16::from_le_bytes(relayed[19..21].try_into().unwrap()),
        1,
        "fixture must fire an accepted bullet"
    );
    assert_eq!(
        relayed[13] & POSITION_STATUS_CLOAK,
        0,
        "the server must reveal an accepted shot even if the client retains cloak"
    );
    assert_ne!(relayed[13] & POSITION_STATUS_FLASH, 0);
    assert_ne!(relayed[13] & POSITION_STATUS_STEALTH, 0);
    assert_eq!(
        state.lock().unwrap().clients[&alpha]
            .last_position
            .as_ref()
            .unwrap()[13],
        relayed[13]
    );
}

#[test]
fn bomb_safety_blocks_hostile_live_bots_but_not_friends_dead_or_spectators() {
    for (friendly, dead, spectator, distance, blocked) in [
        (false, false, false, 20.0, true),
        (true, false, false, 20.0, false),
        (false, true, false, 20.0, false),
        (false, false, true, 20.0, false),
        (false, false, false, 200.0, false),
    ] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let alpha = register_connection(&state, tx);
        join_arena(&state, alpha, 0);
        {
            let mut state = state.lock().unwrap();
            state.config.bot_count = 1;
            state.initialize_configured_bots();
            let sender = state.clients.get_mut(&alpha).unwrap();
            sender.server_weapons.proximity_bombs = true;
            sender.server_weapons.bomb_level = 1;
            let frequency = sender.player.frequency;
            let bot = &mut state.bots[0];
            bot.x = 500.0 + distance;
            bot.y = 700.0;
            bot.player.frequency = if friendly {
                frequency
            } else {
                frequency.wrapping_add(1)
            };
            if dead {
                bot.respawn_at_tick = Some(100);
            }
            if spectator {
                bot.player.ship = 8;
            }
        }
        let mut packet = client_position_packet(0x1122_3344, 500, 700);
        packet[20..22].copy_from_slice(&4_u16.to_le_bytes());
        broadcast_position_observable(&state, alpha, &packet);
        let locked = state.lock().unwrap();
        assert_eq!(
            locked.active_projectiles.is_empty(),
            blocked,
            "friendly={friendly}, dead={dead}, spectator={spectator}, distance={distance}"
        );
    }
}

#[test]
fn bomb_safety_does_not_exclude_a_proximity_mine_beside_an_enemy() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _rx_a) = mpsc::channel();
    let (tx_b, _rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    broadcast_position_observable(&state, beta, &client_position_packet(0x1122_3344, 520, 700));
    {
        let mut locked = state.lock().unwrap();
        let sender = locked.clients.get_mut(&alpha).unwrap();
        sender.server_weapons.proximity_bombs = true;
        sender.server_weapons.bomb_level = 1;
    }
    let mut packet = client_position_packet(0x1122_3344, 500, 700);
    packet[20..22].copy_from_slice(&(4_u16 | 0x8000).to_le_bytes());
    broadcast_position_observable(&state, alpha, &packet);
    let locked = state.lock().unwrap();
    assert_eq!(
        locked.active_projectiles.len(),
        1,
        "the reference game's mine input does not run the fire-only BombSafety exclusion"
    );
}

#[test]
fn accelerated_input_clock_cannot_commit_legal_future_movement() {
    for elapsed in [0, 100] {
        let state = Arc::new(Mutex::new(ServerState::default()));
        let (tx, _rx) = mpsc::channel();
        let player_id = register_connection(&state, tx);
        join_arena(&state, player_id, 0);
        let baseline_tick = 1000;
        let tick = baseline_tick + elapsed;
        let sequence = 100;
        let door_state = crate::doors::ClientDoorState::for_challenge(900);
        let phase =
            crate::doors::client_door_phase_candidates_at_tick(Some(door_state), baseline_tick)
                .into_iter()
                .filter(|p| {
                    crate::continuum::continuum_tick_diff(baseline_tick, p.started_tick) >= 0
                })
                .min_by_key(|p| {
                    crate::continuum::continuum_tick_elapsed(baseline_tick, p.started_tick)
                })
                .unwrap();
        seed_exact_canonical_orientation_anchor(
            &state,
            player_id,
            sequence,
            baseline_tick,
            8200,
            8200,
            0.25,
        );
        let runtime = {
            let mut locked = state.lock().unwrap();
            let c = locked.clients.get_mut(&player_id).unwrap();
            c.door_state = Some(door_state);
            for record in &mut c.recent_input_records {
                record.door_seed = phase.seed;
                record.door_phase_tick = phase.started_tick;
            }
            let mut w = c.canonical_movement_witness.unwrap();
            w.door_seed = phase.seed;
            w.door_phase_tick = phase.started_tick;
            c.canonical_movement_witness = Some(w);
            c.canonical_position_witnesses.clear();
            c.canonical_position_witnesses.push_back(w);
            locked.active_player_runtime_identity(player_id).unwrap()
        };
        let mut admitted = 0;
        for batch in 0..3u16 {
            let base = sequence + 1 + batch * 16;
            let entries = vec![(1u16, 10, phase.seed); 16];
            assert!(crate::connection::record_client_input_observation_at_tick(
                &state,
                runtime,
                &build_input_record_packet(base, &entries),
                tick
            ));
            let tail = {
                let locked = state.lock().unwrap();
                let c = &locked.clients[&player_id];
                crate::sim_observation::replay_canonical_position_gap(
                    c,
                    locked.zone_map(),
                    false,
                    false,
                    false,
                    crate::sim_observation::movement_upgrades_for_client(c),
                    &[],
                    base + 15,
                    tick,
                    server_spawn_coordinates(locked.zone_map()).0,
                    server_spawn_coordinates(locked.zone_map()).1,
                )
                .unwrap()
                .witnesses
                .last()
                .copied()
                .unwrap()
            };
            let mut packet =
                client_position_packet_with_sequence(0, tail.wire_x, tail.wire_y, tail.sequence);
            packet[1] = tail.direction;
            packet[6..8].copy_from_slice(&tail.wire_velocity_x.to_le_bytes());
            packet[14..16].copy_from_slice(&tail.wire_velocity_y.to_le_bytes());
            let (bounty, energy) = {
                let locked = state.lock().unwrap();
                let c = &locked.clients[&player_id];
                (c.server_bounty, c.server_energy)
            };
            packet[16..18].copy_from_slice(&bounty.to_le_bytes());
            packet[18..20].copy_from_slice(&energy.to_le_bytes());
            set_client_position_canonical_state(
                &mut packet,
                tail.state.position_x,
                tail.state.position_y,
                tail.state.velocity_x,
                tail.state.velocity_y,
                tail.state.orientation,
            );
            let accepted = crate::arena::accept_position_relay_for_runtime_at_tick(
                &state, runtime, &packet, tick,
            )
            .is_some();
            eprintln!(
                "CLOCK_PROBE batch={batch} accepted={accepted} sequence={} position=({}, {}) server_tick={tick}",
                tail.sequence, tail.state.position_x, tail.state.position_y
            );
            if !accepted {
                break;
            }
            admitted += 16;
        }
        let locked = state.lock().unwrap();
        let c = &locked.clients[&player_id];
        eprintln!(
            "CLOCK_PROBE admitted_steps={admitted} real_elapsed_ticks={elapsed} last_position_sequence={:?} canonical={:?}",
            c.last_position_sequence, c.canonical_movement_witness
        );
        assert_eq!(
            admitted,
            if elapsed == 0 { 0 } else { 48 },
            "reject accelerated time but retain a legitimate delayed 800ms history"
        );
        assert_eq!(c.last_position_sequence, Some(sequence + admitted));
        if elapsed == 0 {
            assert_eq!(
                c.canonical_movement_witness.unwrap().state.position_x,
                8200.0
            );
            assert_eq!(
                c.movement_time_budget,
                Default::default(),
                "rejection is atomic"
            );
        }
    }
}
