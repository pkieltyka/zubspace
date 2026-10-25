use super::*;

fn runtime(
    player_id: u16,
    session_id: u64,
    life_seq: u32,
) -> crate::authority_identity::PlayerRuntimeIdentity {
    crate::authority_identity::PlayerRuntimeIdentity::new(
        crate::authority_identity::PlayerSessionIdentity::new(
            player_id,
            crate::authority_identity::PlayerSessionId::new(session_id),
        ),
        crate::authority_identity::LifeIdentity::new(player_id, life_seq),
    )
}

fn player(
    player_id: u16,
    session_id: u64,
    life_seq: u32,
    frequency: u16,
    x: u16,
    y: u16,
) -> ObjectivePlayer {
    ObjectivePlayer {
        runtime: runtime(player_id, session_id, life_seq),
        arena_stream_id: 77,
        frequency,
        ship: 0,
        alive: true,
        safe: false,
        attached: false,
        position_x: x,
        position_y: y,
        velocity_x: 50,
        velocity_y: -25,
        direction: 0,
        exact_position_x: f32::from(x),
        exact_position_y: f32::from(y),
        exact_velocity_x: 5.0,
        exact_velocity_y: -2.5,
        exact_orientation: 0.0,
        position_sequence: 90,
    }
}

fn apply_lifecycle_without_goal(
    registry: &mut ObjectiveRegistry,
    player: ObjectivePlayer,
    reason: ObjectiveLifecycleReason,
    tick: u32,
    flag_drop_tile: impl FnMut(u16, u16) -> FlagDropTile,
) -> Result<ObjectiveMutation, ObjectiveMutationError> {
    apply_player_objective_lifecycle(
        registry,
        player,
        reason,
        tick,
        &[],
        flag_drop_tile,
        |_, _| false,
        |_, _| false,
    )
}

fn complete_profile(
    flag_count: u16,
    ball_count: u8,
    flag_policy: FlagPolicy,
    ball_policy: PowerballPolicy,
    ball_win_by: u32,
    team_limits: ObjectiveTeamLimits,
) -> crate::objectives::ServerObjectiveProfile {
    crate::objectives::ServerObjectiveProfile {
        flags: ServerFlagProfile::validate(
            flag_count,
            (flag_count > 0).then_some(FlagProfileSettings {
                spawn: ObjectiveSpawnArea {
                    x: 1,
                    y: 1,
                    radius: 0,
                },
                carry_timeout_ticks: 10,
                reset_delay_ticks: 2,
                reward: 100_000,
                team_limits,
                policy: flag_policy,
            }),
        )
        .unwrap(),
        powerballs: ServerPowerballProfile::validate(
            ball_count,
            (ball_count > 0).then_some(PowerballProfileSettings {
                spawns: vec![ObjectiveSpawnArea {
                    x: 1,
                    y: 1,
                    radius: 0,
                }],
                send_cadence_ticks: 25,
                goal_delay_ticks: 2,
                reward: -100,
                win_by: ball_win_by,
                team_limits,
                policy: ball_policy,
            }),
        )
        .unwrap(),
    }
}

fn position_witness(
    sequence: u16,
    life_seq: u32,
    x: u16,
    y: u16,
) -> crate::state::CanonicalPositionWitness {
    crate::state::CanonicalPositionWitness {
        sequence,
        life_seq,
        state: crate::state::ServerSimMovement {
            position_x: f32::from(x),
            position_y: f32::from(y),
            orientation: 0.0,
            velocity_x: 0.0,
            velocity_y: 0.0,
            energy: 1000.0,
            max_energy: 1000.0,
        },
        wire_x: x,
        wire_y: y,
        wire_velocity_x: 0,
        wire_velocity_y: 0,
        direction: 0,
        door_seed: 0,
        door_phase_tick: 0,
    }
}

fn ball_fire_order_fixture(
    position_sequence: u16,
) -> (
    std::sync::Arc<std::sync::Mutex<ServerState>>,
    crate::objective_protocol::ObjectiveActionRequest,
    crate::continuum_position::ClientPositionSnapshot,
) {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, _rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    let gun = crate::gameplay_rules::encode_weapon_data(1, 1, false, 1, 0, false);
    let mut packet = client_position_packet_with_sequence(0, 320, 320, position_sequence);
    packet[20..22].copy_from_slice(&gun.to_le_bytes());
    let snapshot = crate::continuum_position::parse_client_position_snapshot(&packet).unwrap();
    let request = {
        let mut state = shared.lock().unwrap();
        state.objectives.reset(synthetic_profile(0, 1)).unwrap();
        state.clients.get_mut(&player_id).unwrap().last_position = Some(
            client_position_packet_with_sequence(0, 320, 320, position_sequence),
        );
        let context = crate::objectives::objective_player(&state, player_id).unwrap();
        let ball = state
            .objectives
            .create_powerball(PowerballState::Carried {
                carrier: context.runtime,
            })
            .unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.last_position_sequence = Some(position_sequence);
        client
            .canonical_position_witnesses
            .push_back(position_witness(
                position_sequence,
                context.runtime.life.life_seq,
                320,
                320,
            ));
        client.server_energy = 10_000;
        client.server_max_energy = 10_000;
        client.server_weapons.gun_level = 4;
        crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::BallFire,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: context.arena_stream_id,
            player_id,
            life_seq: context.runtime.life.life_seq,
            action_id: 1,
            position_sequence,
            objective_id: u16::from(ball.identity.id),
            objective_generation: ball.identity.generation,
            expected_version: ball.version.get(),
        }
    };
    (shared, request, snapshot)
}

#[test]
fn deterministic_spawns_use_configured_regions_and_nearest_empty_capacity() {
    let profile = synthetic_profile(2, 2);
    let mut first = ObjectiveRegistry::new(profile.clone());
    let mut second = ObjectiveRegistry::new(profile);
    first.set_rng_seed(12345);
    second.set_rng_seed(12345);
    let occupied = |x, y| x == 1 && y == 1;

    let first_step = initialize_objectives(&mut first, 7, occupied, |x, y| {
        FlagDropTile::from_map_tile(if occupied(x, y) { 20 } else { 0 }, false)
    })
    .unwrap();
    let second_step = initialize_objectives(&mut second, 7, occupied, |x, y| {
        FlagDropTile::from_map_tile(if occupied(x, y) { 20 } else { 0 }, false)
    })
    .unwrap();

    assert_eq!(first_step, second_step);
    assert_eq!(first_step.flags.len(), 2);
    assert_eq!(first_step.powerballs.len(), 2);
    assert!(
        first_step
            .flags
            .iter()
            .all(|flag| { !matches!(flag.state, FlagState::OnMap { x: 24, y: 24, .. }) })
    );
    assert_ne!(first_step.flags[0].state, first_step.flags[1].state);
    assert!(
        first_step
            .powerballs
            .iter()
            .all(|ball| { !matches!(ball.state, PowerballState::OnMap { x: 16, y: 16, .. }) })
    );
}

#[test]
fn nonzero_radius_spawns_match_vie_modulus_math_and_stay_in_region() {
    let area = ObjectiveSpawnArea {
        x: 512,
        y: 512,
        radius: 7,
    };
    let mut profile = synthetic_profile(1, 1);
    let ServerFlagProfile::Enabled(flags) = &mut profile.flags else {
        unreachable!()
    };
    flags.spawn = area;
    let ServerPowerballProfile::Enabled(powerballs) = &mut profile.powerballs else {
        unreachable!()
    };
    powerballs.spawns = vec![area];
    let mut registry = ObjectiveRegistry::new(profile);
    registry.set_rng_seed(12_345);

    let initialized = initialize_objectives(
        &mut registry,
        7,
        |_, _| false,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();

    let mut source_seed = 12_345;
    let unit = |seed: &mut i32| {
        f64::from(crate::vie::vie_rng_next(seed)) / f64::from(crate::vie::VIE_RNG_MODULUS)
    };
    let flag_radius = unit(&mut source_seed).sqrt() * f64::from(area.radius);
    let flag_angle = unit(&mut source_seed) * std::f64::consts::TAU;
    let flag_tile_x = (f64::from(area.x) + 0.5 + flag_radius * flag_angle.cos()) as u16;
    let flag_tile_y = (f64::from(area.y) + 0.5 + flag_radius * flag_angle.sin()) as u16;
    assert!(matches!(
        initialized.flags[0].state,
        FlagState::OnMap { x, y, .. }
            if (x, y) == (flag_tile_x * 16 + 8, flag_tile_y * 16 + 8)
    ));
    assert!(flag_tile_x.abs_diff(area.x) <= area.radius);
    assert!(flag_tile_y.abs_diff(area.y) <= area.radius);

    let ball_radius = unit(&mut source_seed) * f64::from(area.radius);
    let ball_angle = unit(&mut source_seed) * std::f64::consts::TAU;
    let ball_tile_x = (i32::from(area.x) + (ball_radius * ball_angle.cos()) as i32) as u16;
    let ball_tile_y = (i32::from(area.y) + (ball_radius * ball_angle.sin()) as i32) as u16;
    let sub_tile = crate::vie::vie_rng_next(&mut source_seed) as u8;
    let expected_ball = (
        ball_tile_x * 16 + u16::from(sub_tile >> 4),
        ball_tile_y * 16 + u16::from(sub_tile & 0x0f),
    );
    assert!(matches!(
        initialized.powerballs[0].state,
        PowerballState::OnMap { x, y, .. } if (x, y) == expected_ball
    ));
    assert!(ball_tile_x.abs_diff(area.x) <= area.radius);
    assert!(ball_tile_y.abs_diff(area.y) <= area.radius);
}

#[test]
fn primary_nearest_empty_walk_orders_borders_and_stops_after_35_for_both_spawns() {
    let reserved = std::collections::BTreeSet::new();
    let border = ObjectiveSpawnArea {
        x: 0,
        y: 0,
        radius: 0,
    };
    let mut flag_seed = 7;
    assert_eq!(
        flag_spawn_tile(&mut flag_seed, border, &reserved, &mut |x, y| {
            FlagDropTile::from_map_tile(
                if matches!((x, y), (1, 0) | (0, 1023)) {
                    0
                } else {
                    20
                },
                false,
            )
        }),
        Some((1, 0)),
        "candidate search must not wrap the out-of-bounds top tile"
    );
    let mut ball_seed = 7;
    let border_ball = ball_spawn_wire_position(&mut ball_seed, border, &mut |x, y| {
        !matches!((x, y), (1, 0) | (0, 1023))
    })
    .unwrap();
    assert_eq!((border_ball.0 / 16, border_ball.1 / 16), (1, 0));

    let center = ObjectiveSpawnArea {
        x: 100,
        y: 100,
        radius: 0,
    };
    let mut flag_seed = 11;
    assert_eq!(
        flag_spawn_tile(&mut flag_seed, center, &reserved, &mut |x, y| {
            FlagDropTile::from_map_tile(if (x, y) == (100, 100) { 20 } else { 0 }, false)
        }),
        Some((100, 99)),
        "the equal-distance order starts at the top and proceeds clockwise"
    );
    let mut ball_seed = 11;
    let center_ball =
        ball_spawn_wire_position(&mut ball_seed, center, &mut |x, y| (x, y) == (100, 100)).unwrap();
    assert_eq!((center_ball.0 / 16, center_ball.1 / 16), (100, 99));

    let literal_only_up_growth = |x, y| !matches!((x, y), (99, 100) | (100, 101) | (100, 98));
    let mut flag_seed = 11;
    assert_eq!(
        flag_spawn_tile(&mut flag_seed, center, &reserved, &mut |x, y| {
            FlagDropTile::from_map_tile(u8::from(literal_only_up_growth(x, y)) * 20, false)
        }),
        Some((100, 98)),
        "the selected primary's literal Up-or-Up growth passes the left and bottom neighbors"
    );
    let mut ball_seed = 11;
    let literal_ball = ball_spawn_wire_position(&mut ball_seed, center, &mut |x, y| {
        literal_only_up_growth(x, y)
    })
    .unwrap();
    assert_eq!((literal_ball.0 / 16, literal_ball.1 / 16), (100, 98));

    let mut seen = Vec::new();
    let mut flag_seed = 12;
    assert!(
        flag_spawn_tile(&mut flag_seed, center, &reserved, &mut |x, y| {
            seen.push((x, y));
            FlagDropTile::from_map_tile(0, (x, y) == (100, 100))
        })
        .is_some()
    );
    assert_eq!(
        &seen[..2],
        &[(100, 100), (100, 100)],
        "a nearest empty no-flag tile is rejected after relocation, then the next random retry begins"
    );

    let mut flag_seed = 13;
    assert_eq!(
        flag_spawn_tile(&mut flag_seed, center, &reserved, &mut |x, y| {
            FlagDropTile::from_map_tile(if (x, y) == (200, 100) { 0 } else { 20 }, false)
        }),
        None
    );
    let mut ball_seed = 13;
    assert_eq!(
        ball_spawn_wire_position(&mut ball_seed, center, &mut |x, y| { (x, y) != (200, 100) }),
        None
    );

    let mut first_search_calls = 0_usize;
    let mut ball_seed = 17;
    assert_eq!(
        ball_spawn_wire_position(&mut ball_seed, center, &mut |_, _| {
            first_search_calls += 1;
            true
        }),
        None
    );
    let mut flag_calls = 0_usize;
    let mut flag_seed = 17;
    assert!(
        flag_spawn_tile(&mut flag_seed, center, &reserved, &mut |_, _| {
            flag_calls += 1;
            FlagDropTile::from_map_tile(
                if flag_calls <= first_search_calls {
                    20
                } else {
                    0
                },
                false,
            )
        })
        .is_some(),
        "a failed nearest-empty search must advance to the next flag random retry"
    );
}

#[test]
fn unavailable_spawn_fails_atomically_without_partial_registry() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(1, 1));
    assert_eq!(
        initialize_objectives(
            &mut registry,
            1,
            |_, _| true,
            |_, _| { FlagDropTile::from_map_tile(20, false) }
        ),
        Err(ObjectiveRegistryError::SpawnUnavailable)
    );
    assert!(registry.is_empty());
}

#[test]
fn flag_initialization_excludes_no_flag_regions_without_blocking_ball_spawn() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(1, 1));

    let initialized = initialize_objectives(
        &mut registry,
        1,
        |_, _| false,
        |x, y| FlagDropTile::from_map_tile(0, (x, y) == (1, 1)),
    )
    .unwrap();

    assert!(!matches!(
        initialized.flags[0].state,
        FlagState::OnMap { x: 24, y: 24, .. }
    ));
    assert!(matches!(
        initialized.powerballs[0].state,
        PowerballState::OnMap { x, y, .. } if x / 16 == 1 && y / 16 == 1
    ));
}

#[test]
fn deadline_flag_and_ball_spawns_ignore_temporary_bricks_on_static_empty_tiles() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let tick = crate::wire::current_tick();
    let spawn = {
        let state = shared.lock().unwrap();
        let map = state.zone_map();
        map.tiles
            .iter()
            .enumerate()
            .find_map(|(index, tile)| {
                let x = (index % crate::zone::MAP_TILES) as u16;
                let y = (index / crate::zone::MAP_TILES) as u16;
                (*tile == 0 && !map.no_flag_drops_at(x, y) && x > 1 && y > 1).then_some((x, y))
            })
            .expect("test zone needs a static empty flag-capable tile")
    };
    {
        let mut state = shared.lock().unwrap();
        let mut profile = complete_profile(
            1,
            1,
            crate::objectives::synthetic_flag_policy(),
            crate::objectives::synthetic_powerball_policy(),
            1,
            ObjectiveTeamLimits::default(),
        );
        let ServerFlagProfile::Enabled(flags) = &mut profile.flags else {
            unreachable!()
        };
        flags.spawn = ObjectiveSpawnArea {
            x: spawn.0,
            y: spawn.1,
            radius: 0,
        };
        let ServerPowerballProfile::Enabled(powerballs) = &mut profile.powerballs else {
            unreachable!()
        };
        powerballs.spawns = vec![ObjectiveSpawnArea {
            x: spawn.0,
            y: spawn.1,
            radius: 0,
        }];
        state.objectives.reset(profile).unwrap();
        state
            .objectives
            .create_flag(FlagState::Waiting {
                respawn_at_tick: Some(tick),
            })
            .unwrap();
        state
            .objectives
            .create_powerball(PowerballState::Waiting {
                respawn_at_tick: Some(tick),
            })
            .unwrap();
        state.objectives.set_rng_seed(23);
        state.active_bricks.push(ServerBrick {
            tiles: vec![spawn],
            packet: vec![BRICK_DROPPED, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            expires_at_tick: crate::continuum::continuum_tick_add(tick, 1_000),
        });
    }

    crate::server_tick::run_server_tick(&shared, Instant::now());

    let mut state = shared.lock().unwrap();
    assert_eq!(state.active_bricks[0].tiles, vec![spawn]);
    assert!(matches!(
        state.objectives.replay().flags[0].state,
        FlagState::OnMap { x, y, .. }
            if (x, y) == (spawn.0 * 16 + 8, spawn.1 * 16 + 8)
    ));
    assert!(matches!(
        state.objectives.replay().powerballs[0].state,
        PowerballState::OnMap { x, y, .. }
            if (x, y) == (spawn.0 * 16 + 5, spawn.1 * 16 + 3)
    ));
    assert_eq!(*state.objectives.rng_seed_mut(), 1_690_451_795);
}

#[test]
fn fixed_step_uses_x_then_y_revert_bounce_and_integer_friction() {
    let mut motion = PowerballMotion {
        position_x: 31_500,
        position_y: 16_000,
        velocity_x: 1_000,
        velocity_y: 999,
        friction: 999_000,
        friction_delta: 1_000,
        last_step_tick: 1,
        last_publish_tick: 1,
        last_touch: None,
        last_touch_frequency: None,
    };
    simulate_powerball_step(&mut motion, |x, y| x == 2 && y == 1);

    assert_eq!(motion.position_x, 31_500, "solid x reverts before y");
    assert_eq!(motion.position_y, 16_999, "y observes the reverted x axis");
    assert_eq!(motion.velocity_x, -999);
    assert_eq!(motion.velocity_y, 998);
    assert_eq!(motion.friction, 998_000);

    motion.velocity_x = 0;
    motion.velocity_y = 0;
    motion.friction_delta = -2_000;
    simulate_powerball_step(&mut motion, |_, _| false);
    assert_eq!(
        motion.friction, 1_000_000,
        "negative delta increases friction"
    );
}

#[test]
fn presentation_lookahead_does_not_reverse_live_velocity_before_actual_collision() {
    let mut motion = PowerballMotion {
        position_x: 15_000,
        position_y: 8_000,
        velocity_x: 800,
        velocity_y: 0,
        friction: 1_000_000,
        friction_delta: 0,
        last_step_tick: 1,
        last_publish_tick: 1,
        last_touch: None,
        last_touch_frequency: None,
    };

    simulate_powerball_step(&mut motion, |x, _| x == 1);
    assert_eq!(motion.position_x, 15_800);
    assert_eq!(
        motion.velocity_x, 800,
        "lookahead into tile 1 is presentation-only"
    );

    simulate_powerball_step(&mut motion, |x, _| x == 1);
    assert_eq!(
        motion.position_x, 15_800,
        "the actual collision reverts its step"
    );
    assert_eq!(motion.velocity_x, -800);
}

#[test]
fn fixed_step_threads_last_touch_frequency_for_friendly_brick_passage() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(0, 1));
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 31,
            y: 16,
            velocity_x: 1_000,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let mut motion = PowerballMotion::from_wire_position(31, 16, 1_000, 0, 0, 0);
    motion.position_x = 31_500;
    motion.last_touch_frequency = Some(7);
    registry
        .set_powerball_motion(ball.identity, motion)
        .unwrap();
    let mut observed_frequency = None;

    step_powerball_motion(
        &mut registry,
        1,
        |frequency, x, y| {
            observed_frequency = frequency;
            frequency != Some(7) && x == 2 && y == 1
        },
        |_, _, _| false,
    )
    .unwrap();

    assert_eq!(observed_frequency, Some(7));
    assert_eq!(
        registry
            .powerball_motion(ball.identity.id)
            .unwrap()
            .position_x,
        32_500
    );
}

#[test]
fn authenticated_actions_bind_every_runtime_object_and_position_identity() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(1, 0));
    let flag = registry
        .create_flag(FlagState::OnMap {
            x: 24,
            y: 24,
            owner: None,
        })
        .unwrap();
    let actor = player(4, 9, 3, 0, 24, 24);
    let request = crate::objective_protocol::ObjectiveActionRequest {
        kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
        server_session_id: crate::wire::server_session_id(),
        arena_stream_id: actor.arena_stream_id,
        player_id: actor.runtime.player_id(),
        life_seq: actor.runtime.life.life_seq,
        action_id: u32::MAX,
        position_sequence: actor.position_sequence,
        objective_id: flag.identity.id,
        objective_generation: flag.identity.generation,
        expected_version: flag.version.get(),
    };

    assert_eq!(
        authenticate_objective_action(&mut registry, actor, request),
        Ok(())
    );
    assert_eq!(
        authenticate_objective_action(&mut registry, actor, request),
        Err(ObjectiveMutationError::DuplicateAction)
    );
    let mut wrapped_action = request;
    wrapped_action.action_id = 1;
    assert_eq!(
        authenticate_objective_action(&mut registry, actor, wrapped_action),
        Ok(()),
        "the action replay window must advance across u32 wrap"
    );
    let mut stale_position = request;
    stale_position.action_id = 2;
    stale_position.position_sequence = actor.position_sequence.wrapping_sub(1);
    assert_eq!(
        authenticate_objective_action(&mut registry, actor, stale_position),
        Err(ObjectiveMutationError::StalePosition)
    );
    let mut stale_generation = flag.identity;
    stale_generation.generation += 1;
    assert_eq!(
        claim_flag(&mut registry, actor, stale_generation, flag.version, 5,),
        Err(ObjectiveMutationError::UnknownObjective)
    );
}

#[test]
fn authenticated_flag_claim_runs_through_server_routing_and_duplicate_is_inert() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    drain_available(&rx);
    let position_sequence = 41;
    let position = client_position_packet_with_sequence(0, 320, 320, position_sequence);
    let (request, witness) = {
        let mut state = shared.lock().unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.last_position = Some(position);
        client.last_position_sequence = Some(position_sequence.wrapping_sub(1));
        state.objectives.reset(synthetic_profile(1, 0)).unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::OnMap {
                x: 320,
                y: 320,
                owner: None,
            })
            .unwrap();
        let context = crate::objectives::objective_player(&state, player_id).unwrap();
        let request = crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: context.arena_stream_id,
            player_id,
            life_seq: context.runtime.life.life_seq,
            action_id: 77,
            position_sequence: context.position_sequence,
            objective_id: flag.identity.id,
            objective_generation: flag.identity.generation,
            expected_version: flag.version.get(),
        };
        let witness = crate::state::CanonicalPositionWitness {
            sequence: position_sequence,
            life_seq: context.runtime.life.life_seq,
            state: crate::state::ServerSimMovement {
                position_x: 320.0,
                position_y: 320.0,
                orientation: 0.0,
                velocity_x: 0.0,
                velocity_y: 0.0,
                energy: 1000.0,
                max_energy: 1000.0,
            },
            wire_x: 320,
            wire_y: 320,
            wire_velocity_x: 0,
            wire_velocity_y: 0,
            direction: 0,
            door_seed: 0,
            door_phase_tick: 0,
        };
        (request, witness)
    };

    crate::objectives::relay_objective_action(&shared, player_id, &request.encode());
    crate::objectives::process_pending_objective_actions(&shared, crate::wire::current_tick());
    assert!(matches!(
        shared
            .lock()
            .unwrap()
            .objectives
            .flag(request.objective_id)
            .unwrap()
            .state,
        FlagState::OnMap { .. }
    ));
    assert_eq!(shared.lock().unwrap().pending_reliable_gameplay.len(), 1);
    {
        let mut state = shared.lock().unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.canonical_position_witnesses.push_back(witness);
        client.last_position_sequence = Some(position_sequence.wrapping_add(1));
    }
    crate::objectives::process_pending_objective_actions(&shared, crate::wire::current_tick());
    let after_claim = shared
        .lock()
        .unwrap()
        .objectives
        .flag(request.objective_id)
        .unwrap();
    assert!(
        matches!(
            after_claim.state,
            FlagState::Carried { carrier, .. } if carrier.player_id() == player_id
        ),
        "after claim: {after_claim:?}"
    );
    assert_eq!(after_claim.version.get(), request.expected_version + 1);

    crate::objectives::relay_objective_action(&shared, player_id, &request.encode());
    crate::objectives::process_pending_objective_actions(&shared, crate::wire::current_tick());
    assert_eq!(
        shared.lock().unwrap().objectives.flag(request.objective_id),
        Some(after_claim)
    );
}

#[test]
fn a_versioned_drop_from_a_player_not_carrying_the_flag_is_rejected_and_the_carrier_keeps_it() {
    // `apply_request` gates `FlagDrop` on the caller being the carrier before it asks the engine for the
    // authored requested-drop, so this pins the gate through the same reliable routing a client uses: a
    // second peer holding the flag's current identity and version must not be able to hand it back.
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx_carrier, rx_carrier) = std::sync::mpsc::channel();
    let (tx_other, rx_other) = std::sync::mpsc::channel();
    let carrier = register_connection(&shared, tx_carrier);
    let other = register_connection(&shared, tx_other);
    join_arena(&shared, carrier, 0);
    join_arena(&shared, other, 0);
    drain_available(&rx_carrier);
    drain_available(&rx_other);

    let position_sequence = 61_u16;
    let carrier_witness = {
        let mut state = shared.lock().unwrap();
        let carrier_position = client_position_packet_with_sequence(0, 320, 320, position_sequence);
        let client = state.clients.get_mut(&carrier).unwrap();
        client.last_position = Some(carrier_position);
        client.last_position_sequence = Some(position_sequence.wrapping_add(1));
        let other_position = client_position_packet_with_sequence(0, 320, 320, position_sequence);
        let client = state.clients.get_mut(&other).unwrap();
        client.last_position = Some(other_position);
        client.last_position_sequence = Some(position_sequence.wrapping_sub(1));
        state.objectives.reset(synthetic_profile(1, 0)).unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::OnMap {
                x: 320,
                y: 320,
                owner: None,
            })
            .unwrap();
        let context = crate::objectives::objective_player(&state, carrier).unwrap();
        state
            .clients
            .get_mut(&carrier)
            .unwrap()
            .canonical_position_witnesses
            .push_back(position_witness(
                position_sequence,
                context.runtime.life.life_seq,
                320,
                320,
            ));
        let claim = crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: context.arena_stream_id,
            player_id: carrier,
            life_seq: context.runtime.life.life_seq,
            action_id: 11,
            position_sequence: context.position_sequence,
            objective_id: flag.identity.id,
            objective_generation: flag.identity.generation,
            expected_version: flag.version.get(),
        };
        drop(state);
        crate::objectives::relay_objective_action(&shared, carrier, &claim.encode());
        crate::objectives::process_pending_objective_actions(&shared, crate::wire::current_tick());
        let state = shared.lock().unwrap();
        let flag = state.objectives.flag(claim.objective_id).unwrap();
        assert!(
            matches!(flag.state, FlagState::Carried { carrier: runtime, .. }
                if runtime.player_id() == carrier),
            "the carrier must hold the flag before the stranger acts: {flag:?}"
        );
        let other_context = crate::objectives::objective_player(&state, other).unwrap();
        (
            position_witness(
                position_sequence,
                other_context.runtime.life.life_seq,
                320,
                320,
            ),
            crate::objective_protocol::ObjectiveActionRequest {
                kind: crate::objective_protocol::ObjectiveActionKind::FlagDrop,
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: other_context.arena_stream_id,
                player_id: other,
                life_seq: other_context.runtime.life.life_seq,
                action_id: 12,
                position_sequence: other_context.position_sequence,
                objective_id: claim.objective_id,
                objective_generation: flag.identity.generation,
                expected_version: flag.version.get(),
            },
            flag.version,
        )
    };
    let (other_witness, drop_request, carried_version) = carrier_witness;
    {
        let mut state = shared.lock().unwrap();
        let client = state.clients.get_mut(&other).unwrap();
        client.last_position_sequence = Some(position_sequence.wrapping_add(1));
        client.canonical_position_witnesses.push_back(other_witness);
    }

    crate::objectives::relay_objective_action(&shared, other, &drop_request.encode());
    crate::objectives::process_pending_objective_actions(&shared, crate::wire::current_tick());

    let state = shared.lock().unwrap();
    let flag = state.objectives.flag(drop_request.objective_id).unwrap();
    assert!(
        matches!(flag.state, FlagState::Carried { carrier: runtime, .. }
            if runtime.player_id() == carrier),
        "a non-carrier drop must leave the flag carried: {flag:?}"
    );
    assert_eq!(
        flag.version, carried_version,
        "a rejected drop must not advance the flag version"
    );
    assert!(
        state.clients.get(&carrier).unwrap().player.flags == 1,
        "the carrier's flag count must stay intact"
    );
    drop(state);
    assert!(
        shared.lock().unwrap().pending_reliable_gameplay.is_empty(),
        "the rejected action must resolve rather than stay resident"
    );
    drop(rx_other);
}

#[test]
fn an_objective_action_without_a_position_witness_resolves_instead_of_staying_resident() {
    // A live flag session showed the shape this pins: the client's
    // FlagClaim arrived, deferred once for a missing canonical position
    // witness, and was then never evaluated again - no grant, no rejection and
    // no release across the next 40 seconds (`/tmp/flag-arrival.log`). A claim
    // whose witness never arrives must exhaust the recovery window
    // (`objectives/actions.rs:27`) and leave the reliable queue drained, so the
    // client can retry with a fresh action instead of being silently ignored.
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    drain_available(&rx);
    let position_sequence = 41;
    let request = {
        let mut state = shared.lock().unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.last_position = Some(client_position_packet_with_sequence(
            0,
            320,
            320,
            position_sequence,
        ));
        client.last_position_sequence = Some(position_sequence);
        state.objectives.reset(synthetic_profile(1, 0)).unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::OnMap {
                x: 320,
                y: 320,
                owner: None,
            })
            .unwrap();
        let context = crate::objectives::objective_player(&state, player_id).unwrap();
        crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: context.arena_stream_id,
            player_id,
            life_seq: context.runtime.life.life_seq,
            action_id: 91,
            // No canonical witness is ever established for this sequence, so
            // the head defers until the recovery window closes.
            position_sequence: position_sequence.wrapping_add(4_000),
            objective_id: flag.identity.id,
            objective_generation: flag.identity.generation,
            expected_version: flag.version.get(),
        }
    };

    crate::objectives::relay_objective_action(&shared, player_id, &request.encode());
    assert_eq!(shared.lock().unwrap().pending_reliable_gameplay.len(), 1);

    let start_tick = crate::wire::current_tick();
    for step in 1..=24 {
        crate::server_tick::run_server_tick_at_tick(
            &shared,
            Instant::now(),
            crate::continuum::continuum_tick_add(start_tick, step),
        );
    }

    let state = shared.lock().unwrap();
    assert_eq!(
        state.pending_reliable_gameplay.len(),
        0,
        "the unwitnessed claim stayed resident past the recovery window; a client \
         that asked once is never answered and never retried"
    );
    assert!(
        matches!(
            state.objectives.flag(request.objective_id).unwrap().state,
            FlagState::OnMap { .. }
        ),
        "an unwitnessed claim must not move the flag"
    );
}

#[test]
fn retained_position_outside_causal_tail_cannot_replay_objective_proximity() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    drain_available(&rx);
    let request = {
        let mut state = shared.lock().unwrap();
        state.objectives.reset(synthetic_profile(1, 0)).unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::OnMap {
                x: 320,
                y: 320,
                owner: None,
            })
            .unwrap();
        state.clients.get_mut(&player_id).unwrap().last_position =
            Some(client_position_packet_with_sequence(0, 320, 320, 50));
        let context = crate::objectives::objective_player(&state, player_id).unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.last_position_sequence = Some(50);
        client
            .canonical_position_witnesses
            .push_back(position_witness(
                20,
                context.runtime.life.life_seq,
                320,
                320,
            ));
        crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: context.arena_stream_id,
            player_id,
            life_seq: context.runtime.life.life_seq,
            action_id: 1,
            position_sequence: 20,
            objective_id: flag.identity.id,
            objective_generation: flag.identity.generation,
            expected_version: flag.version.get(),
        }
    };

    crate::objectives::relay_objective_action(&shared, player_id, &request.encode());
    crate::objectives::process_pending_objective_actions(&shared, crate::wire::current_tick());

    let state = shared.lock().unwrap();
    assert!(state.pending_reliable_gameplay.is_empty());
    assert!(matches!(
        state.objectives.flag(request.objective_id).unwrap().state,
        FlagState::OnMap { .. }
    ));
}

#[test]
fn hard_lifecycle_retires_old_runtime_objective_actions_and_releases_capacity() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    drain_available(&rx);
    let mut state = shared.lock().unwrap();
    let old_runtime = state.active_player_runtime_identity(player_id).unwrap();
    let arena_stream_id = state.clients[&player_id].arena_stream_id;
    let request = crate::objective_protocol::ObjectiveActionRequest {
        kind: crate::objective_protocol::ObjectiveActionKind::FlagDrop,
        server_session_id: crate::wire::server_session_id(),
        arena_stream_id,
        player_id,
        life_seq: old_runtime.life.life_seq,
        action_id: 0,
        position_sequence: 0,
        objective_id: 0,
        objective_generation: 1,
        expected_version: 1,
    };
    for action_id in 0..crate::state::MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME as u32 {
        assert!(state.queue_objective_action_for_runtime(
            old_runtime,
            action_id,
            crate::objective_protocol::ObjectiveActionRequest {
                action_id,
                ..request
            },
            10,
        ));
    }
    state.clear_pending_position_inputs_for_player(player_id);
    assert_eq!(
        state.pending_reliable_gameplay.len(),
        0,
        "the deferred old-life actions retire in the hard-boundary transaction"
    );

    state.advance_client_life_at_tick(player_id, 11).unwrap();
    let new_runtime = state.active_player_runtime_identity(player_id).unwrap();
    for action_id in 100..100 + crate::state::MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME as u32 {
        assert!(state.queue_objective_action_for_runtime(
            new_runtime,
            action_id,
            crate::objective_protocol::ObjectiveActionRequest {
                life_seq: new_runtime.life.life_seq,
                action_id,
                ..request
            },
            11,
        ));
    }
    assert_eq!(
        state.pending_reliable_gameplay.len(),
        crate::state::MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME,
        "hard cleanup releases global capacity for the new life"
    );
    assert!(!state.queue_objective_action_for_runtime(
        new_runtime,
        999,
        crate::objective_protocol::ObjectiveActionRequest {
            life_seq: new_runtime.life.life_seq,
            action_id: 999,
            ..request
        },
        11,
    ));
    assert_eq!(
        state.pending_reliable_gameplay.runtime_len(new_runtime),
        crate::state::MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME,
        "the per-runtime cap remains independent"
    );
}

#[test]
fn authoritative_powerball_pickup_and_manual_fire_anchor_weapon_delays() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    drain_available(&rx);
    let pickup = {
        let mut state = shared.lock().unwrap();
        let mut profile = synthetic_profile(0, 1);
        let ServerPowerballProfile::Enabled(powerballs) = &mut profile.powerballs else {
            unreachable!()
        };
        powerballs.policy.require_line_of_sight = false;
        powerballs.win_by = 2;
        let soccer_mode = powerballs.policy.soccer_mode;
        state.objectives.reset(profile).unwrap();
        let (frequency, goal_x, goal_y) = state
            .zone_map()
            .tiles
            .iter()
            .enumerate()
            .filter(|(_, tile)| **tile == 172)
            .find_map(|(index, _)| {
                let x = ((index % crate::zone::MAP_TILES) as u16) * 16 + 8;
                let y = ((index / crate::zone::MAP_TILES) as u16) * 16 + 8;
                (0..8).find_map(|frequency| {
                    soccer_goal_counts_world(x, y, frequency, soccer_mode)
                        .then_some((frequency, x, y))
                })
            })
            .expect("synthetic goal action test requires one map goal tile");
        state.clients.get_mut(&player_id).unwrap().player.frequency = frequency;
        let ball = state
            .objectives
            .create_powerball(PowerballState::OnMap {
                x: goal_x,
                y: goal_y,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: None,
            })
            .unwrap();
        state.clients.get_mut(&player_id).unwrap().last_position =
            Some(client_position_packet_with_sequence(0, goal_x, goal_y, 41));
        let context = crate::objectives::objective_player(&state, player_id).unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.last_position_sequence = Some(41);
        client
            .canonical_position_witnesses
            .push_back(position_witness(
                41,
                context.runtime.life.life_seq,
                goal_x,
                goal_y,
            ));
        crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::BallPickup,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: context.arena_stream_id,
            player_id,
            life_seq: context.runtime.life.life_seq,
            action_id: 1,
            position_sequence: 41,
            objective_id: u16::from(ball.identity.id),
            objective_generation: ball.identity.generation,
            expected_version: ball.version.get(),
        }
    };

    crate::objectives::relay_objective_action(&shared, player_id, &pickup.encode());
    crate::objectives::process_pending_objective_actions(&shared, 100);
    let early_fire = {
        let state = shared.lock().unwrap();
        let ball = state
            .objectives
            .powerball(pickup.objective_id as u8)
            .unwrap();
        assert_eq!(
            state.clients[&player_id].objective_weapon_lock_until_tick,
            Some(crate::continuum::continuum_tick_add(
                100,
                arena_ship_weapon_fire_delay_ticks(0, 3),
            ))
        );
        crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::BallFire,
            action_id: 2,
            expected_version: ball.version.get(),
            ..pickup
        }
    };
    crate::objectives::relay_objective_action(&shared, player_id, &early_fire.encode());
    crate::objectives::process_pending_objective_actions(&shared, 101);
    assert!(matches!(
        shared
            .lock()
            .unwrap()
            .objectives
            .powerball(pickup.objective_id as u8)
            .unwrap()
            .state,
        PowerballState::Carried { .. }
    ));
    let fire = crate::objective_protocol::ObjectiveActionRequest {
        action_id: 3,
        ..early_fire
    };
    crate::objectives::relay_objective_action(&shared, player_id, &fire.encode());
    crate::objectives::process_pending_objective_actions(&shared, 251);

    let state = shared.lock().unwrap();
    assert_eq!(
        state.clients[&player_id].objective_weapon_lock_until_tick,
        Some(crate::continuum::continuum_tick_add(251, 50))
    );
    assert!(matches!(
        state
            .objectives
            .powerball(pickup.objective_id as u8)
            .unwrap()
            .state,
        PowerballState::OnMap { .. }
    ));
    assert_eq!(
        state
            .objectives
            .powerball_score(state.clients[&player_id].player.frequency),
        1,
        "an accepted fire on canonical goal geometry must score in that transaction"
    );
}

#[test]
fn production_manual_and_timed_release_quantize_exact_canonical_geometry_once() {
    fn exact_release_fixture(
        carry_timeout: u32,
    ) -> (
        std::sync::Arc<std::sync::Mutex<ServerState>>,
        std::sync::mpsc::Receiver<Vec<u8>>,
        u16,
        crate::objective_protocol::ObjectiveActionRequest,
    ) {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
        let (tx, rx) = std::sync::mpsc::channel();
        let player_id = register_connection(&shared, tx);
        join_arena(&shared, player_id, 0);
        drain_available(&rx);
        let request = {
            let mut state = shared.lock().unwrap();
            let mut profile = synthetic_profile(0, 1);
            let ServerPowerballProfile::Enabled(powerballs) = &mut profile.powerballs else {
                unreachable!()
            };
            powerballs.policy.carry_timeout_ticks[0] = carry_timeout;
            state.objectives.reset(profile).unwrap();
            let sequence = 410;
            state.clients.get_mut(&player_id).unwrap().last_position =
                Some(client_position_packet_with_sequence(0, 101, 201, sequence));
            let context = crate::objectives::objective_player(&state, player_id).unwrap();
            let ball = state
                .objectives
                .create_powerball(PowerballState::Carried {
                    carrier: context.runtime,
                })
                .unwrap();
            let mut witness = position_witness(sequence, context.runtime.life.life_seq, 101, 201);
            witness.state.position_x = 100.75;
            witness.state.position_y = 200.75;
            witness.state.velocity_x = 10.075;
            witness.state.velocity_y = -10.075;
            witness.wire_velocity_x = 101;
            witness.wire_velocity_y = -101;
            let client = state.clients.get_mut(&player_id).unwrap();
            client.last_position_sequence = Some(sequence);
            client.canonical_movement_witness = Some(witness);
            client.canonical_position_witnesses.push_back(witness);
            crate::objective_protocol::ObjectiveActionRequest {
                kind: crate::objective_protocol::ObjectiveActionKind::BallFire,
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: context.arena_stream_id,
                player_id,
                life_seq: context.runtime.life.life_seq,
                action_id: 91,
                position_sequence: sequence,
                objective_id: u16::from(ball.identity.id),
                objective_generation: ball.identity.generation,
                expected_version: ball.version.get(),
            }
        };
        (shared, rx, player_id, request)
    }

    let (manual, manual_rx, player_id, request) = exact_release_fixture(100);
    crate::objectives::relay_objective_action_at_delivery(
        &manual,
        player_id,
        70,
        &request.encode(),
    );
    crate::objectives::process_pending_objective_actions(&manual, 500);
    let manual_state = manual
        .lock()
        .unwrap()
        .objectives
        .powerball(request.objective_id as u8)
        .unwrap();
    assert!(matches!(
        manual_state.state,
        PowerballState::OnMap {
            x: 100,
            y: 200,
            velocity_x: 100,
            velocity_y: -260,
            ..
        }
    ));
    let manual_wire = crate::objective_protocol::ObjectiveStateUpdate::parse(
        &read_reliable_payload_from_channel(&manual_rx),
    )
    .unwrap();
    assert_eq!(
        (
            manual_wire.x,
            manual_wire.y,
            manual_wire.velocity_x,
            manual_wire.velocity_y,
        ),
        (100, 200, 100, -260)
    );

    let (timed, timed_rx, _player_id, request) = exact_release_fixture(1);
    let tick = crate::wire::current_tick();
    {
        let mut state = timed.lock().unwrap();
        let identity = crate::objectives::PowerballIdentity {
            id: request.objective_id as u8,
            generation: request.objective_generation,
        };
        state.objectives.set_powerball_carried_since(
            identity,
            crate::continuum::continuum_tick_add(tick, u32::MAX),
        );
    }
    crate::server_tick::run_server_tick(&timed, Instant::now());
    let timed_state = timed
        .lock()
        .unwrap()
        .objectives
        .powerball(request.objective_id as u8)
        .unwrap();
    let extension = (arena_ship_radius_pixels(0) - 4.0).max(0.0);
    assert!(matches!(
        timed_state.state,
        PowerballState::OnMap { x: 100, y, velocity_x: 100, velocity_y: 59, .. }
            if y == (200.0 - extension) as u16
    ));
    let timed_wire = crate::objective_protocol::ObjectiveStateUpdate::parse(
        &read_reliable_payload_from_channel(&timed_rx),
    )
    .unwrap();
    assert_eq!(
        (
            timed_wire.x,
            timed_wire.y,
            timed_wire.velocity_x,
            timed_wire.velocity_y,
        ),
        (100, (200.0 - extension) as u16, 100, 59)
    );
}

#[test]
fn due_objective_deadlines_use_current_tick_exact_position_before_ball_motion() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    drain_available(&rx);
    let due_tick = 20_000;
    let sequence = 700;
    let (runtime, packet, expected_flag_position, expected_ball_state) = {
        let mut state = shared.lock().unwrap();
        let map = state.zone_map();
        let (tile_x, tile_y) = (3..crate::zone::MAP_TILES - 3)
            .flat_map(|y| (3..crate::zone::MAP_TILES - 3).map(move |x| (x, y)))
            .find(|&(x, y)| {
                (-2_i32..=2).all(|dy| {
                    (-2_i32..=2).all(|dx| {
                        let tx = (x as i32 + dx) as usize;
                        let ty = (y as i32 + dy) as usize;
                        map.tiles[ty * crate::zone::MAP_TILES + tx] == 0
                            && !map.no_flag_drops_at(tx as u16, ty as u16)
                    })
                })
            })
            .expect("test map needs a clear static deadline region");
        let exact_x = tile_x as f32 * 16.0 + 8.75;
        let exact_y = tile_y as f32 * 16.0 + 8.75;
        let wire_x = crate::continuum_position::protocol_position_coord(exact_x);
        let wire_y = crate::continuum_position::protocol_position_coord(exact_y);
        let exact_velocity_x = 10.075;
        let exact_velocity_y = -10.075;
        let wire_velocity_x =
            crate::continuum_position::protocol_velocity_component(exact_velocity_x);
        let wire_velocity_y =
            crate::continuum_position::protocol_velocity_component(exact_velocity_y);
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        let snapshot = crate::continuum_position::ClientPositionSnapshot {
            direction: 0,
            timestamp: 0,
            client_sequence: Some(sequence),
            x: wire_x,
            y: wire_y,
            velocity_x: wire_velocity_x,
            velocity_y: wire_velocity_y,
            togglables: 0,
            bounty: client.server_bounty,
            energy: client.server_energy,
            weapon: 0,
            canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_f32(
                exact_x,
                exact_y,
                exact_velocity_x,
                exact_velocity_y,
                0.0,
            ),
            repel_snapshot_id: None,
            repel_application_sequence: None,
            repel_delivery_generation: None,
            warp_collectible_id: None,
        };
        client.life_position_sequence_base = Some(sequence);
        client.pending_frequency_baseline = Some(snapshot);
        let packet = frequency_baseline_packet(snapshot, sequence);

        let mut profile = synthetic_profile(1, 1);
        let ServerFlagProfile::Enabled(flags) = &mut profile.flags else {
            unreachable!()
        };
        flags.carry_timeout_ticks = 1;
        flags.policy.drop_radius_tiles = 0;
        flags.policy.timeout_drop = FlagDropBehavior::NeutralAtCarrier;
        let ServerPowerballProfile::Enabled(powerballs) = &mut profile.powerballs else {
            unreachable!()
        };
        powerballs.policy.carry_timeout_ticks[0] = 1;
        state.objectives.reset(profile).unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::Carried {
                carrier: runtime,
                x: 16,
                y: 16,
                owner: None,
            })
            .unwrap();
        state.objectives.set_flag_carried_since(
            flag.identity,
            crate::continuum::continuum_tick_add(due_tick, u32::MAX),
        );
        let ball = state
            .objectives
            .create_powerball(PowerballState::Carried { carrier: runtime })
            .unwrap();
        state.objectives.set_powerball_carried_since(
            ball.identity,
            crate::continuum::continuum_tick_add(due_tick, u32::MAX),
        );
        let extension = (arena_ship_radius_pixels(0) - 4.0).max(0.0);
        let expected_ball_state = (
            exact_x.trunc() as u16,
            (exact_y.trunc() - extension) as u16,
            100,
            59,
        );
        (
            runtime,
            packet,
            (tile_x as u16 * 16 + 8, tile_y as u16 * 16 + 8),
            expected_ball_state,
        )
    };
    assert!(
        crate::connection::record_client_frequency_input_baseline_at_tick(
            &shared,
            runtime,
            &packet[crate::continuum_position::CLIENT_POSITION_EMBEDDED_INPUT_OFFSET..],
            due_tick,
        )
    );
    assert!(
        shared
            .lock()
            .unwrap()
            .queue_position_input_for_runtime(runtime, packet)
    );

    crate::server_tick::run_server_tick_at_tick(&shared, Instant::now(), due_tick);

    let state = shared.lock().unwrap();
    let witness = state.clients[&player_id]
        .canonical_movement_witness
        .expect("current position admitted before deadlines");
    assert_eq!(witness.state.position_x.fract(), 0.75);
    let deadline_player = crate::objectives::objective_player(&state, player_id)
        .expect("admitted current position remains available to objective deadlines");
    assert_eq!(
        (
            deadline_player.position_x / 16,
            deadline_player.position_y / 16
        ),
        (expected_flag_position.0 / 16, expected_flag_position.1 / 16)
    );
    let flag_state = state.objectives.replay().flags[0].state;
    assert!(
        matches!(
            flag_state,
            FlagState::OnMap { x, y, .. } if (x, y) == expected_flag_position
        ),
        "flag state {flag_state:?}, expected {expected_flag_position:?}"
    );
    assert!(matches!(
        state.objectives.replay().powerballs[0].state,
        PowerballState::OnMap { x, y, velocity_x, velocity_y, .. }
            if (x, y, velocity_x, velocity_y) == expected_ball_state
    ));
    let motion = state.objectives.powerball_motion(0).unwrap();
    assert_eq!(
        motion.wire_position(),
        (expected_ball_state.0, expected_ball_state.1)
    );
    assert_eq!(
        motion.last_step_tick, due_tick,
        "release is not integrated twice"
    );
    drop(state);

    let updates = rx
        .try_iter()
        .filter_map(|packet| {
            reliable_payload(&packet).and_then(|payload| {
                crate::objective_protocol::ObjectiveStateUpdate::parse(payload).ok()
            })
        })
        .collect::<Vec<_>>();
    let ball_update = updates
        .iter()
        .find(|update| {
            update.kind == crate::objective_protocol::ObjectiveStateUpdate::KIND_POWERBALL
        })
        .expect("timed release state emitted");
    assert_eq!(
        (
            ball_update.x,
            ball_update.y,
            ball_update.velocity_x,
            ball_update.velocity_y,
        ),
        expected_ball_state
    );
    let flag_update = updates
        .iter()
        .find(|update| update.kind == crate::objective_protocol::ObjectiveStateUpdate::KIND_FLAG)
        .expect("flag timeout state emitted");
    assert_eq!((flag_update.x, flag_update.y), expected_flag_position);
}

#[test]
fn weapon_and_ball_fire_are_mutually_exclusive_for_both_delivery_orders_and_retries() {
    let (weapon_first, fire, snapshot) = ball_fire_order_fixture(410);
    {
        let mut state = weapon_first.lock().unwrap();
        let (previewed, committed) =
            crate::arena::preview_and_commit_position_weapon_with_ball_policy_for_test(
                &mut state,
                fire.player_id,
                snapshot,
                500,
                true,
                true,
            )
            .unwrap();
        assert_ne!(previewed, 0);
        assert_eq!(committed, previewed);
        assert!(
            state.clients[&fire.player_id].accepted_weapon_witness_consumed(fire.position_sequence)
        );
        let mut intervening = snapshot;
        intervening.client_sequence = Some(fire.position_sequence.wrapping_add(1));
        let (previewed, committed) =
            crate::arena::preview_and_commit_position_weapon_with_ball_policy_for_test(
                &mut state,
                fire.player_id,
                intervening,
                1_500,
                true,
                true,
            )
            .unwrap();
        assert_ne!(previewed, 0);
        assert_eq!(committed, previewed);
        assert!(
            state.clients[&fire.player_id]
                .accepted_weapon_witness_consumed(fire.position_sequence.wrapping_add(1))
        );
        assert!(
            state.clients[&fire.player_id].accepted_weapon_witness_consumed(fire.position_sequence)
        );
    }
    for _ in 0..2 {
        crate::objectives::relay_objective_action(&weapon_first, fire.player_id, &fire.encode());
        crate::objectives::process_pending_objective_actions(&weapon_first, 501);
    }
    assert!(matches!(
        weapon_first
            .lock()
            .unwrap()
            .objectives
            .powerball(fire.objective_id as u8)
            .unwrap()
            .state,
        PowerballState::Carried { .. }
    ));

    let (ball_first, fire, snapshot) = ball_fire_order_fixture(420);
    crate::objectives::relay_objective_action(&ball_first, fire.player_id, &fire.encode());
    crate::objectives::process_pending_objective_actions(&ball_first, 600);
    {
        let mut state = ball_first.lock().unwrap();
        assert!(
            state.clients[&fire.player_id]
                .objective_ball_fire_witness_consumed(fire.position_sequence)
        );
        assert!(matches!(
            state
                .objectives
                .powerball(fire.objective_id as u8)
                .unwrap()
                .state,
            PowerballState::OnMap { .. }
        ));
        let mut intervening = snapshot;
        intervening.client_sequence = Some(fire.position_sequence.wrapping_add(1));
        let (previewed, committed) =
            crate::arena::preview_and_commit_position_weapon_with_ball_policy_for_test(
                &mut state,
                fire.player_id,
                intervening,
                1_500,
                true,
                true,
            )
            .unwrap();
        assert_ne!(previewed, 0);
        assert_eq!(committed, previewed);
        for tick in [2_500, 2_501] {
            let (previewed, committed) =
                crate::arena::preview_and_commit_position_weapon_with_ball_policy_for_test(
                    &mut state,
                    fire.player_id,
                    snapshot,
                    tick,
                    true,
                    true,
                )
                .unwrap();
            assert_eq!((previewed, committed), (0, 0));
        }
    }
}

#[test]
fn production_position_and_objective_queues_keep_full_witness_exclusion_history() {
    fn queue_weapon_witness(
        shared: &std::sync::Arc<std::sync::Mutex<ServerState>>,
        player_id: u16,
        sequence: u16,
        server_tick: u32,
    ) {
        let gun = crate::gameplay_rules::encode_weapon_data(1, 1, false, 1, 0, false);
        let mut packet = client_position_packet_with_sequence(0, 320, 320, sequence);
        packet[20..22].copy_from_slice(&gun.to_le_bytes());
        let mut state = shared.lock().unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        let life_seq = runtime.life.life_seq;
        let client = state.clients.get_mut(&player_id).unwrap();
        if !client
            .canonical_position_witnesses
            .iter()
            .any(|witness| witness.sequence == sequence)
        {
            client
                .canonical_position_witnesses
                .push_back(position_witness(sequence, life_seq, 320, 320));
        }
        if !client
            .recent_input_records
            .iter()
            .any(|record| record.sequence == sequence)
        {
            client
                .recent_input_records
                .push_back(crate::state::SequencedInputObservation {
                    sequence,
                    input: crate::state::ClientInputObservation {
                        bits: 0,
                        direction: 0,
                    },
                    door_seed: 0,
                    door_phase_tick: 0,
                });
        }
        client.last_position_sequence = Some(sequence);
        client.last_input_seq = Some(sequence);
        client
            .observer_combat_timeline
            .seed_input_decision_for_test(sequence, server_tick)
            .expect("objective weapon fixture owns an exact combat-view decision");
        state.pending_position_inputs.queue(runtime, packet);
    }

    let (weapon_first, fire, _) = ball_fire_order_fixture(500);
    queue_weapon_witness(&weapon_first, fire.player_id, fire.position_sequence, 1_000);
    crate::server_tick::run_server_tick_at_tick(&weapon_first, Instant::now(), 1_000);
    queue_weapon_witness(
        &weapon_first,
        fire.player_id,
        fire.position_sequence.wrapping_add(1),
        2_000,
    );
    crate::server_tick::run_server_tick_at_tick(&weapon_first, Instant::now(), 2_000);
    crate::objectives::relay_objective_action_at_delivery(
        &weapon_first,
        fire.player_id,
        81,
        &fire.encode(),
    );
    crate::server_tick::run_server_tick_at_tick(&weapon_first, Instant::now(), 2_001);
    {
        let state = weapon_first.lock().unwrap();
        assert!(
            state.clients[&fire.player_id].accepted_weapon_witness_consumed(fire.position_sequence)
        );
        assert!(
            state.clients[&fire.player_id]
                .accepted_weapon_witness_consumed(fire.position_sequence.wrapping_add(1))
        );
        assert!(matches!(
            state
                .objectives
                .powerball(fire.objective_id as u8)
                .unwrap()
                .state,
            PowerballState::Carried { .. }
        ));
    }

    let (ball_first, fire, _) = ball_fire_order_fixture(u16::MAX);
    crate::objectives::relay_objective_action_at_delivery(
        &ball_first,
        fire.player_id,
        u32::MAX,
        &fire.encode(),
    );
    crate::server_tick::run_server_tick_at_tick(&ball_first, Instant::now(), 3_000);
    queue_weapon_witness(
        &ball_first,
        fire.player_id,
        fire.position_sequence.wrapping_add(1),
        4_000,
    );
    crate::server_tick::run_server_tick_at_tick(&ball_first, Instant::now(), 4_000);
    queue_weapon_witness(&ball_first, fire.player_id, fire.position_sequence, 5_000);
    crate::server_tick::run_server_tick_at_tick(&ball_first, Instant::now(), 5_000);
    {
        let state = ball_first.lock().unwrap();
        assert!(
            state.clients[&fire.player_id]
                .objective_ball_fire_witness_consumed(fire.position_sequence)
        );
        assert!(
            state.clients[&fire.player_id]
                .accepted_weapon_witness_consumed(fire.position_sequence.wrapping_add(1))
        );
        assert!(
            !state.clients[&fire.player_id]
                .accepted_weapon_witness_consumed(fire.position_sequence)
        );
    }
}

#[test]
fn production_frequency_cutover_clears_objective_weapon_life_state_across_wrap() {
    let (shared, old_fire, old_weapon_snapshot) = ball_fire_order_fixture(u16::MAX);
    let player_id = old_fire.player_id;
    let old_runtime = {
        let mut state = shared.lock().unwrap();
        let old_runtime = state.active_player_runtime_identity(player_id).unwrap();
        let (previewed, committed) =
            crate::arena::preview_and_commit_position_weapon_with_ball_policy_for_test(
                &mut state,
                player_id,
                old_weapon_snapshot,
                1_000,
                true,
                true,
            )
            .unwrap();
        assert_ne!(previewed, 0);
        assert_eq!(committed, previewed);
        let mut wrapped = position_witness(0, old_runtime.life.life_seq, 320, 320);
        wrapped.state.energy = f32::from(state.clients[&player_id].server_energy);
        wrapped.state.max_energy = f32::from(state.clients[&player_id].server_max_energy);
        let mut wrapped_packet = client_position_packet_with_sequence(0, 320, 320, 0);
        wrapped_packet[18..20]
            .copy_from_slice(&state.clients[&player_id].server_energy.to_le_bytes());
        set_client_position_canonical_state(
            &mut wrapped_packet,
            wrapped.state.position_x,
            wrapped.state.position_y,
            wrapped.state.velocity_x,
            wrapped.state.velocity_y,
            wrapped.state.orientation,
        );
        let wrapped_position = crate::arena::large_position_from_client(player_id, &wrapped_packet)
            .expect("wrapped old-life position");
        let client = state.clients.get_mut(&player_id).unwrap();
        client.canonical_position_witnesses.push_back(wrapped);
        client.canonical_movement_witness = Some(wrapped);
        client.begin_actor_motion_segment();
        client.append_actor_motion_witness(1_000, wrapped).unwrap();
        client.last_position_sequence = Some(0);
        client.last_position = Some(wrapped_position);
        client.last_input_seq = Some(0);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: 0,
        });
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: 0,
                input: crate::state::ClientInputObservation {
                    bits: 0,
                    direction: 0,
                },
                door_seed: 0,
                door_phase_tick: 0,
            });
        client.server_sim_state = Some(wrapped.state);
        client.last_position_server_tick = Some(1_000);
        old_runtime
    };
    let wrapped_fire = crate::objective_protocol::ObjectiveActionRequest {
        action_id: 2,
        position_sequence: 0,
        ..old_fire
    };
    crate::objectives::relay_objective_action_at_delivery(
        &shared,
        player_id,
        u32::MAX,
        &wrapped_fire.encode(),
    );
    crate::objectives::process_pending_objective_actions(&shared, 1_100);
    {
        let state = shared.lock().unwrap();
        let client = &state.clients[&player_id];
        assert!(client.accepted_weapon_witness_consumed(u16::MAX));
        assert!(client.objective_ball_fire_witness_consumed(0));
        assert!(client.objective_weapon_lock_active(1_101));
    }

    let frequency = frequency_change_request(&shared, player_id, 7);
    crate::arena::update_frequency_at_tick(&shared, player_id, &frequency, 1_101);
    let new_runtime = shared
        .lock()
        .unwrap()
        .active_player_runtime_identity(player_id)
        .unwrap();
    assert_ne!(new_runtime, old_runtime, "production cutover advances life");
    {
        let state = shared.lock().unwrap();
        let client = &state.clients[&player_id];
        assert_eq!(client.player.frequency, 7);
        assert_eq!(client.objective_weapon_lock_until_tick, None);
        assert!(!client.accepted_weapon_witness_consumed(u16::MAX));
        assert!(!client.objective_ball_fire_witness_consumed(0));
    }
    consume_pending_lifecycle_baseline_at_tick(&shared, player_id, 1_102);

    let (new_ball, new_witness) = {
        let mut state = shared.lock().unwrap();
        state.objectives.reset(synthetic_profile(0, 1)).unwrap();
        let ball = state
            .objectives
            .create_powerball(PowerballState::Carried {
                carrier: new_runtime,
            })
            .unwrap();
        let witness = position_witness(0, new_runtime.life.life_seq, 320, 320);
        let client = state.clients.get_mut(&player_id).unwrap();
        client.life_position_sequence_base = None;
        client.canonical_position_witnesses.push_back(witness);
        client.canonical_movement_witness = Some(witness);
        client.last_position_sequence = Some(0);
        client.last_position = Some(client_position_packet_with_sequence(0, 320, 320, 0));
        client.server_energy = 10_000;
        client.server_max_energy = 10_000;
        client.server_weapons.gun_level = 4;
        (ball, witness)
    };
    {
        let mut state = shared.lock().unwrap();
        let mut reused_weapon = old_weapon_snapshot;
        reused_weapon.client_sequence = Some(u16::MAX);
        let (previewed, committed) =
            crate::arena::preview_and_commit_position_weapon_with_ball_policy_for_test(
                &mut state,
                player_id,
                reused_weapon,
                2_000,
                true,
                true,
            )
            .unwrap();
        assert_ne!(previewed, 0, "old-life weapon witness is reusable");
        assert_eq!(committed, previewed);
        assert_eq!(new_witness.sequence, 0);
    }
    let new_fire = crate::objective_protocol::ObjectiveActionRequest {
        kind: crate::objective_protocol::ObjectiveActionKind::BallFire,
        server_session_id: crate::wire::server_session_id(),
        arena_stream_id: shared.lock().unwrap().clients[&player_id].arena_stream_id,
        player_id,
        life_seq: new_runtime.life.life_seq,
        action_id: 3,
        position_sequence: 0,
        objective_id: u16::from(new_ball.identity.id),
        objective_generation: new_ball.identity.generation,
        expected_version: new_ball.version.get(),
    };
    crate::objectives::relay_objective_action_at_delivery(
        &shared,
        player_id,
        0,
        &new_fire.encode(),
    );
    crate::objectives::process_pending_objective_actions(&shared, 2_001);
    let state = shared.lock().unwrap();
    assert!(matches!(
        state
            .objectives
            .powerball(new_ball.identity.id)
            .unwrap()
            .state,
        PowerballState::OnMap { .. }
    ));
    assert!(state.clients[&player_id].objective_ball_fire_witness_consumed(0));
}

#[test]
fn reliable_objective_frequency_barrier_preserves_delivery_order_defer_and_wrap() {
    fn fixture(
        frequency: u16,
    ) -> (
        std::sync::Arc<std::sync::Mutex<ServerState>>,
        u16,
        Vec<u8>,
        crate::objective_protocol::ObjectiveActionRequest,
        crate::state::CanonicalPositionWitness,
    ) {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
        let (tx, rx) = std::sync::mpsc::channel();
        let player_id = register_connection(&shared, tx);
        join_arena(&shared, player_id, 0);
        drain_available(&rx);
        let frequency_payload = frequency_change_request(&shared, player_id, frequency);
        let (request, next_witness) = {
            let mut state = shared.lock().unwrap();
            let mut profile = synthetic_profile(1, 0);
            let ServerFlagProfile::Enabled(flags) = &mut profile.flags else {
                unreachable!()
            };
            flags.policy.win_delay_ticks = 0;
            state.objectives.reset(profile).unwrap();
            let runtime = state.active_player_runtime_identity(player_id).unwrap();
            let client = &state.clients[&player_id];
            let arena_stream_id = client.arena_stream_id;
            let baseline =
                crate::continuum_position::parse_client_position_snapshot(&frequency_payload[5..])
                    .expect("frequency request has canonical baseline");
            let next_sequence = baseline.client_sequence.unwrap().wrapping_sub(1);
            let mut next_witness =
                position_witness(next_sequence, runtime.life.life_seq, baseline.x, baseline.y);
            next_witness.direction = baseline.direction;
            next_witness.state.orientation =
                crate::continuum_position::protocol_direction_to_orientation(baseline.direction);
            next_witness.state.energy = f32::from(client.server_energy);
            next_witness.state.max_energy = f32::from(client.server_max_energy);
            let flag = state
                .objectives
                .create_flag(FlagState::OnMap {
                    x: next_witness.wire_x,
                    y: next_witness.wire_y,
                    owner: None,
                })
                .unwrap();
            (
                crate::objective_protocol::ObjectiveActionRequest {
                    kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
                    server_session_id: crate::wire::server_session_id(),
                    arena_stream_id,
                    player_id,
                    life_seq: runtime.life.life_seq,
                    action_id: 200,
                    position_sequence: next_sequence,
                    objective_id: flag.identity.id,
                    objective_generation: flag.identity.generation,
                    expected_version: flag.version.get(),
                },
                next_witness,
            )
        };
        (shared, player_id, frequency_payload, request, next_witness)
    }

    fn establish_objective_witness(
        shared: &std::sync::Arc<std::sync::Mutex<ServerState>>,
        player_id: u16,
        witness: crate::state::CanonicalPositionWitness,
    ) {
        let mut state = shared.lock().unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.canonical_position_witnesses.push_back(witness);
        client.canonical_movement_witness = Some(witness);
        client.last_position_sequence = Some(witness.sequence);
        client.last_input_seq = Some(witness.sequence);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: witness.direction,
        });
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: witness.sequence,
                input: crate::state::ClientInputObservation {
                    bits: 0,
                    direction: witness.direction,
                },
                door_seed: 0,
                door_phase_tick: 0,
            });
        client.server_sim_state = Some(witness.state);
        client.last_position = Some(client_position_packet_with_sequence(
            0,
            witness.wire_x,
            witness.wire_y,
            witness.sequence,
        ));
    }

    let (objective_first, player_id, frequency, request, witness) = fixture(7);
    let objective_first_tick = crate::wire::current_tick();
    crate::objectives::relay_objective_action_at_delivery(
        &objective_first,
        player_id,
        u32::MAX,
        &request.encode(),
    );
    crate::objectives::relay_objective_action_at_delivery(
        &objective_first,
        player_id,
        u32::MAX,
        &request.encode(),
    );
    {
        let mut state = objective_first.lock().unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        assert!(state.queue_frequency_change_for_runtime(runtime, 0, frequency));
        assert_eq!(
            state.pending_reliable_gameplay.len(),
            2,
            "retry is deduplicated"
        );
    }
    crate::server_tick::run_server_tick_at_tick(
        &objective_first,
        Instant::now(),
        objective_first_tick,
    );
    {
        let state = objective_first.lock().unwrap();
        assert_eq!(state.clients[&player_id].player.frequency, 0);
        assert_eq!(state.pending_reliable_gameplay.len(), 2);
    }
    establish_objective_witness(&objective_first, player_id, witness);
    crate::server_tick::run_server_tick_at_tick(
        &objective_first,
        Instant::now(),
        crate::continuum::continuum_tick_add(objective_first_tick, 1),
    );
    {
        let state = objective_first.lock().unwrap();
        assert_eq!(state.clients[&player_id].player.frequency, 7);
        assert!(state.pending_reliable_gameplay.is_empty());
        assert!(
            state
                .objectives
                .flag(request.objective_id)
                .unwrap()
                .version
                .get()
                > request.expected_version,
            "the earlier objective terminals before the cutover lifecycle"
        );
        assert!(matches!(
            state.objectives.flag(request.objective_id).unwrap().state,
            FlagState::Waiting { .. }
        ));
    }

    let (frequency_first, player_id, frequency, request, _witness) = fixture(8);
    {
        let mut state = frequency_first.lock().unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        assert!(state.queue_frequency_change_for_runtime(runtime, 100, frequency));
    }
    crate::objectives::relay_objective_action_at_delivery(
        &frequency_first,
        player_id,
        101,
        &request.encode(),
    );
    crate::server_tick::run_server_tick_at_tick(
        &frequency_first,
        Instant::now(),
        crate::wire::current_tick(),
    );
    {
        let state = frequency_first.lock().unwrap();
        assert_eq!(state.clients[&player_id].player.frequency, 8);
        assert_eq!(
            state
                .objectives
                .flag(request.objective_id)
                .unwrap()
                .version
                .get(),
            request.expected_version,
            "later old-scope objective is retired without mutation"
        );
        assert!(state.pending_reliable_gameplay.is_empty());
    }

    let (rejected_first, player_id, frequency, mut request, witness) = fixture(9);
    request.expected_version = request.expected_version.wrapping_add(1);
    establish_objective_witness(&rejected_first, player_id, witness);
    {
        let mut state = rejected_first.lock().unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        assert!(state.queue_frequency_change_for_runtime(runtime, 301, frequency));
    }
    crate::objectives::relay_objective_action_at_delivery(
        &rejected_first,
        player_id,
        300,
        &request.encode(),
    );
    crate::server_tick::run_server_tick_at_tick(
        &rejected_first,
        Instant::now(),
        crate::wire::current_tick(),
    );
    assert_eq!(
        rejected_first.lock().unwrap().clients[&player_id]
            .player
            .frequency,
        9,
        "terminal rejection releases the later reliable cutover"
    );
}

#[test]
fn production_reliable_objective_barrier_orders_all_lifecycle_and_attachment_mutations() {
    struct Fixture {
        shared: std::sync::Arc<std::sync::Mutex<ServerState>>,
        actor: u16,
        target: u16,
        actor_rx: std::sync::mpsc::Receiver<Vec<u8>>,
        request: crate::objective_protocol::ObjectiveActionRequest,
        witness: crate::state::CanonicalPositionWitness,
        joined: bool,
    }

    fn fixture() -> Fixture {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
        let (actor_tx, actor_rx) = std::sync::mpsc::channel();
        let actor = register_connection(&shared, actor_tx);
        join_arena(&shared, actor, 0);
        let (target_tx, target_rx) = std::sync::mpsc::channel();
        let target = register_connection(&shared, target_tx);
        join_arena(&shared, target, 0);
        drain_available(&actor_rx);
        drain_available(&target_rx);
        let (request, witness) = {
            let mut state = shared.lock().unwrap();
            state.clients.get_mut(&target).unwrap().player.frequency = 0;
            state.objectives.reset(synthetic_profile(1, 1)).unwrap();
            let runtime = state.active_player_runtime_identity(actor).unwrap();
            let mut witness = position_witness(42, runtime.life.life_seq, 320, 320);
            // An eligible lifecycle transition needs a simulation witness
            // whose energy agrees with the authoritative full pool.
            witness.state.energy = f32::from(state.clients[&actor].server_energy);
            witness.state.max_energy = f32::from(state.clients[&actor].server_max_energy);
            let ball = state
                .objectives
                .create_powerball(PowerballState::OnMap {
                    x: witness.wire_x,
                    y: witness.wire_y,
                    velocity_x: 0,
                    velocity_y: 0,
                    pickup_unlock_tick: None,
                })
                .unwrap();
            (
                crate::objective_protocol::ObjectiveActionRequest {
                    kind: crate::objective_protocol::ObjectiveActionKind::BallPickup,
                    server_session_id: crate::wire::server_session_id(),
                    arena_stream_id: state.clients[&actor].arena_stream_id,
                    player_id: actor,
                    life_seq: runtime.life.life_seq,
                    action_id: 700,
                    position_sequence: witness.sequence,
                    objective_id: u16::from(ball.identity.id),
                    objective_generation: ball.identity.generation,
                    expected_version: ball.version.get(),
                },
                witness,
            )
        };
        Fixture {
            shared,
            actor,
            target,
            actor_rx,
            request,
            witness,
            joined: true,
        }
    }

    fn ingress(fixture: &mut Fixture, delivery_id: u32, payload: &[u8]) {
        crate::connection::handle_client_packet(
            &fixture.shared,
            fixture.actor,
            reliable(delivery_id, payload),
            "objective-reliable-order",
            &mut fixture.joined,
        );
    }

    fn establish_witness(fixture: &Fixture) {
        let mut state = fixture.shared.lock().unwrap();
        let client = state.clients.get_mut(&fixture.actor).unwrap();
        client
            .canonical_position_witnesses
            .push_back(fixture.witness);
        client.canonical_movement_witness = Some(fixture.witness);
        client.last_position_sequence = Some(fixture.witness.sequence);
        client.server_sim_state = Some(fixture.witness.state);
        // last_position stores an admitted S2C position, not a C2S packet:
        // their status offsets differ, which otherwise invents a status drain.
        client.last_position = crate::arena::large_position_from_client(
            fixture.actor,
            &client_position_packet_with_sequence(
                0,
                fixture.witness.wire_x,
                fixture.witness.wire_y,
                fixture.witness.sequence,
            ),
        );
    }

    fn ball_state(fixture: &Fixture) -> PowerballState {
        fixture
            .shared
            .lock()
            .unwrap()
            .objectives
            .powerball(fixture.request.objective_id as u8)
            .unwrap()
            .state
    }

    fn received_rejection(fixture: &Fixture) -> bool {
        fixture.actor_rx.try_iter().any(|packet| {
            reliable_payload(&packet)
                .and_then(|payload| {
                    crate::objective_protocol::ObjectiveStateUpdate::parse(payload).ok()
                })
                .is_some_and(|update| {
                    update.action_id == fixture.request.action_id
                        && update.outcome
                            == crate::objective_protocol::ObjectiveStateUpdate::OUTCOME_REJECTED
                })
        })
    }

    // Earlier action -> ship: the missing witness holds the later mutation;
    // a transport retry is deduplicated, then the action terminals before the
    // ship advances life and releases its newly carried ball.
    let mut action_ship = fixture();
    let tick = crate::wire::current_tick();
    let action = action_ship.request.encode();
    ingress(&mut action_ship, 0, &action);
    ingress(&mut action_ship, 0, &action);
    let ship = [crate::arena::SHIP_CHANGE_REQUEST, 1, 43, 0];
    ingress(&mut action_ship, 1, &ship);
    crate::server_tick::run_server_tick_at_tick(&action_ship.shared, Instant::now(), tick);
    {
        let state = action_ship.shared.lock().unwrap();
        assert_eq!(state.pending_reliable_gameplay.len(), 2);
        assert_eq!(state.clients[&action_ship.actor].player.ship, 0);
    }
    establish_witness(&action_ship);
    crate::server_tick::run_server_tick_at_tick(
        &action_ship.shared,
        Instant::now(),
        crate::continuum::continuum_tick_add(tick, 1),
    );
    assert_eq!(
        action_ship.shared.lock().unwrap().clients[&action_ship.actor]
            .player
            .ship,
        1
    );
    assert!(!matches!(
        ball_state(&action_ship),
        PowerballState::Carried { .. }
    ));

    // Reverse ship -> action: the old-life action never mutates and receives
    // a retained rejection before the hard boundary clears its queue.
    let mut ship_action = fixture();
    establish_witness(&ship_action);
    let old_life = ship_action.request.life_seq;
    ingress(&mut ship_action, 0, &ship);
    let action = ship_action.request.encode();
    ingress(&mut ship_action, 1, &action);
    crate::server_tick::run_server_tick_at_tick(
        &ship_action.shared,
        Instant::now(),
        crate::wire::current_tick(),
    );
    assert_ne!(
        ship_action.shared.lock().unwrap().clients[&ship_action.actor].life_seq,
        old_life
    );
    assert!(matches!(
        ball_state(&ship_action),
        PowerballState::OnMap { .. }
    ));
    assert!(received_rejection(&ship_action));

    let mut noop_ship_action = fixture();
    establish_witness(&noop_ship_action);
    let life = noop_ship_action.request.life_seq;
    let noop_ship = [crate::arena::SHIP_CHANGE_REQUEST, 0, 43, 0];
    ingress(&mut noop_ship_action, 0, &noop_ship);
    let action = noop_ship_action.request.encode();
    ingress(&mut noop_ship_action, 1, &action);
    crate::server_tick::run_server_tick_at_tick(
        &noop_ship_action.shared,
        Instant::now(),
        crate::wire::current_tick(),
    );
    assert_eq!(
        noop_ship_action.shared.lock().unwrap().clients[&noop_ship_action.actor].life_seq,
        life,
        "a no-op lifecycle terminals without manufacturing a cutover"
    );
    assert!(matches!(
        ball_state(&noop_ship_action),
        PowerballState::Carried { .. }
    ));
    assert!(!received_rejection(&noop_ship_action));

    let mut deferred_ship_action = fixture();
    establish_witness(&deferred_ship_action);
    {
        let mut state = deferred_ship_action.shared.lock().unwrap();
        let runtime = state
            .active_player_runtime_identity(deferred_ship_action.actor)
            .unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::Carried {
                carrier: runtime,
                x: 320,
                y: 320,
                owner: Some(0),
            })
            .unwrap();
        assert!(
            state
                .objectives
                .force_flag_version(flag.identity.id, crate::continuum::CONTINUUM_TICK_MASK)
        );
        state.objectives.force_generation(u64::MAX);
    }
    ingress(&mut deferred_ship_action, 0, &ship);
    let action = deferred_ship_action.request.encode();
    ingress(&mut deferred_ship_action, 1, &action);
    let tick = crate::wire::current_tick();
    crate::server_tick::run_server_tick_at_tick(&deferred_ship_action.shared, Instant::now(), tick);
    {
        let state = deferred_ship_action.shared.lock().unwrap();
        assert_eq!(state.clients[&deferred_ship_action.actor].player.ship, 0);
        assert_eq!(state.pending_reliable_gameplay.len(), 2);
        assert!(matches!(
            state
                .objectives
                .powerball(deferred_ship_action.request.objective_id as u8)
                .unwrap()
                .state,
            PowerballState::OnMap { .. }
        ));
    }
    deferred_ship_action
        .shared
        .lock()
        .unwrap()
        .objectives
        .force_generation(100);
    crate::server_tick::run_server_tick_at_tick(
        &deferred_ship_action.shared,
        Instant::now(),
        crate::continuum::continuum_tick_add(tick, 1),
    );
    {
        let state = deferred_ship_action.shared.lock().unwrap();
        assert_eq!(state.clients[&deferred_ship_action.actor].player.ship, 1);
        assert!(state.pending_reliable_gameplay.is_empty());
    }
    assert!(received_rejection(&deferred_ship_action));

    // Earlier action -> leave and reverse leave -> action follow the same
    // terminal-before-cutover rule while the connection-local joined gate is
    // closed as soon as the leave is retained.
    let mut action_leave = fixture();
    let tick = crate::wire::current_tick();
    let action = action_leave.request.encode();
    ingress(&mut action_leave, 0, &action);
    ingress(&mut action_leave, 1, &[crate::arena::LEAVE_ARENA_REQUEST]);
    assert!(!action_leave.joined);
    crate::server_tick::run_server_tick_at_tick(&action_leave.shared, Instant::now(), tick);
    assert!(action_leave.shared.lock().unwrap().clients[&action_leave.actor].joined);
    establish_witness(&action_leave);
    crate::server_tick::run_server_tick_at_tick(
        &action_leave.shared,
        Instant::now(),
        crate::continuum::continuum_tick_add(tick, 1),
    );
    assert!(!action_leave.shared.lock().unwrap().clients[&action_leave.actor].joined);

    let mut leave_action = fixture();
    establish_witness(&leave_action);
    ingress(&mut leave_action, 0, &[crate::arena::LEAVE_ARENA_REQUEST]);
    let action = leave_action.request.encode();
    ingress(&mut leave_action, 1, &action);
    crate::server_tick::run_server_tick_at_tick(
        &leave_action.shared,
        Instant::now(),
        crate::wire::current_tick(),
    );
    assert!(!leave_action.shared.lock().unwrap().clients[&leave_action.actor].joined);
    assert!(matches!(
        ball_state(&leave_action),
        PowerballState::OnMap { .. }
    ));
    assert!(received_rejection(&leave_action));

    // Earlier action -> attach waits; reverse attach -> action applies the
    // new attached eligibility first and rejects pickup. Use max -> zero to
    // pin both the transport sequencer and application comparator at wrap.
    let mut action_attach = fixture();
    let tick = crate::wire::current_tick();
    let action = action_attach.request.encode();
    ingress(&mut action_attach, 0, &action);
    let attach = attach_request_payload(action_attach.target);
    ingress(&mut action_attach, 1, &attach);
    crate::server_tick::run_server_tick_at_tick(&action_attach.shared, Instant::now(), tick);
    assert!(
        !action_attach
            .shared
            .lock()
            .unwrap()
            .turret_links
            .contains_key(&action_attach.actor)
    );
    establish_witness(&action_attach);
    crate::server_tick::run_server_tick_at_tick(
        &action_attach.shared,
        Instant::now(),
        crate::continuum::continuum_tick_add(tick, 1),
    );
    assert_eq!(
        action_attach.shared.lock().unwrap().turret_links[&action_attach.actor],
        action_attach.target
    );
    assert!(matches!(
        ball_state(&action_attach),
        PowerballState::Carried { .. }
    ));

    let mut attach_action = fixture();
    establish_witness(&attach_action);
    attach_action
        .shared
        .lock()
        .unwrap()
        .clients
        .get_mut(&attach_action.actor)
        .unwrap()
        .incoming_reliable
        .set_next_process_id_for_test(u32::MAX);
    let attach = attach_request_payload(attach_action.target);
    ingress(&mut attach_action, u32::MAX, &attach);
    ingress(&mut attach_action, u32::MAX, &attach);
    let action = attach_action.request.encode();
    ingress(&mut attach_action, 0, &action);
    crate::server_tick::run_server_tick_at_tick(
        &attach_action.shared,
        Instant::now(),
        crate::wire::current_tick(),
    );
    assert_eq!(
        attach_action.shared.lock().unwrap().turret_links[&attach_action.actor],
        attach_action.target
    );
    assert!(matches!(
        ball_state(&attach_action),
        PowerballState::OnMap { .. }
    ));

    // DROP_ATTACHED is sequenced too: an earlier missing action holds the
    // parent's child-drop, while the reverse order drops first and then lets
    // the still-eligible parent complete pickup.
    let mut action_drop = fixture();
    action_drop
        .shared
        .lock()
        .unwrap()
        .turret_links
        .insert(action_drop.target, action_drop.actor);
    let tick = crate::wire::current_tick();
    let action = action_drop.request.encode();
    ingress(&mut action_drop, 0, &action);
    ingress(
        &mut action_drop,
        1,
        &[crate::objectives::DROP_ATTACHED_REQUEST],
    );
    crate::server_tick::run_server_tick_at_tick(&action_drop.shared, Instant::now(), tick);
    assert!(
        action_drop
            .shared
            .lock()
            .unwrap()
            .turret_links
            .contains_key(&action_drop.target)
    );
    establish_witness(&action_drop);
    crate::server_tick::run_server_tick_at_tick(
        &action_drop.shared,
        Instant::now(),
        crate::continuum::continuum_tick_add(tick, 1),
    );
    assert!(
        !action_drop
            .shared
            .lock()
            .unwrap()
            .turret_links
            .contains_key(&action_drop.target)
    );
    assert!(matches!(
        ball_state(&action_drop),
        PowerballState::Carried { .. }
    ));

    let mut drop_action = fixture();
    drop_action
        .shared
        .lock()
        .unwrap()
        .turret_links
        .insert(drop_action.target, drop_action.actor);
    establish_witness(&drop_action);
    ingress(
        &mut drop_action,
        0,
        &[crate::objectives::DROP_ATTACHED_REQUEST],
    );
    let action = drop_action.request.encode();
    ingress(&mut drop_action, 1, &action);
    crate::server_tick::run_server_tick_at_tick(
        &drop_action.shared,
        Instant::now(),
        crate::wire::current_tick(),
    );
    assert!(
        !drop_action
            .shared
            .lock()
            .unwrap()
            .turret_links
            .contains_key(&drop_action.target)
    );
    assert!(matches!(
        ball_state(&drop_action),
        PowerballState::Carried { .. }
    ));

    // Brick participates in the same runtime sequence. The missing action
    // retains the Brick without spending inventory. Because this revealed
    // prefix had no canonical source at its original boundary, the Brick is
    // terminally unavailable and a later witness may not resurrect it.
    let mut action_brick = fixture();
    action_brick
        .shared
        .lock()
        .unwrap()
        .clients
        .get_mut(&action_brick.actor)
        .unwrap()
        .server_weapons
        .bricks = 1;
    let tick = crate::wire::current_tick();
    let action = action_brick.request.encode();
    ingress(&mut action_brick, 0, &action);
    ingress(&mut action_brick, 1, &drop_brick_request_payload(320, 320));
    crate::server_tick::run_server_tick_at_tick(&action_brick.shared, Instant::now(), tick);
    assert!(action_brick.shared.lock().unwrap().active_bricks.is_empty());
    establish_witness(&action_brick);
    action_brick
        .shared
        .lock()
        .unwrap()
        .clients
        .get_mut(&action_brick.actor)
        .unwrap()
        .last_position = Some(
        crate::arena::large_position_from_client(
            action_brick.actor,
            &client_position_packet(0, 320, 320),
        )
        .unwrap(),
    );
    crate::server_tick::run_server_tick_at_tick(
        &action_brick.shared,
        Instant::now(),
        crate::continuum::continuum_tick_add(tick, 1),
    );
    {
        let state = action_brick.shared.lock().unwrap();
        assert!(matches!(
            state
                .objectives
                .powerball(action_brick.request.objective_id as u8)
                .unwrap()
                .state,
            PowerballState::Carried { .. }
        ));
        assert_eq!(state.active_bricks.len(), 0);
        assert_eq!(state.clients[&action_brick.actor].server_weapons.bricks, 1);
    }

    // Reverse order, retry, and delivery wrap publish the Brick first. The
    // actor's own wall is passable for pickup LOS, so the later action still
    // succeeds after the ordered Brick terminal.
    let mut brick_action = fixture();
    establish_witness(&brick_action);
    {
        let mut state = brick_action.shared.lock().unwrap();
        let client = state.clients.get_mut(&brick_action.actor).unwrap();
        client.last_position = Some(
            crate::arena::large_position_from_client(
                brick_action.actor,
                &client_position_packet(0, 320, 320),
            )
            .unwrap(),
        );
        client.server_weapons.bricks = 1;
        client
            .incoming_reliable
            .set_next_process_id_for_test(u32::MAX);
    }
    let brick = drop_brick_request_payload(320, 320);
    ingress(&mut brick_action, u32::MAX, &brick);
    ingress(&mut brick_action, u32::MAX, &brick);
    let action = brick_action.request.encode();
    ingress(&mut brick_action, 0, &action);
    crate::server_tick::run_server_tick_at_tick(
        &brick_action.shared,
        Instant::now(),
        crate::wire::current_tick(),
    );
    {
        let state = brick_action.shared.lock().unwrap();
        assert_eq!(state.active_bricks.len(), 1);
        assert!(matches!(
            state
                .objectives
                .powerball(brick_action.request.objective_id as u8)
                .unwrap()
                .state,
            PowerballState::Carried { .. }
        ));
        let stale = brick_action.request.life_seq;
        drop(state);
        let mut state = brick_action.shared.lock().unwrap();
        let old_runtime = state
            .active_player_runtime_identity(brick_action.actor)
            .unwrap();
        assert_eq!(old_runtime.life.life_seq, stale);
        assert!(
            state
                .advance_client_life_at_tick(brick_action.actor, 10)
                .is_some()
        );
        assert!(!state.queue_reliable_gameplay_for_runtime(
            old_runtime,
            1,
            crate::state::PendingReliableGameplayMutationKind::Brick {
                request: crate::state::ReliableBrickRequest::parse(&brick).unwrap(),
                source: crate::state::BrickSourceBinding::Pending,
            },
        ));
        let current = state
            .active_player_runtime_identity(brick_action.actor)
            .unwrap();
        assert!(!state.queue_reliable_gameplay_for_runtime(
            runtime(brick_action.actor, u64::MAX - 1, current.life.life_seq),
            2,
            crate::state::PendingReliableGameplayMutationKind::Brick {
                request: crate::state::ReliableBrickRequest::parse(&brick).unwrap(),
                source: crate::state::BrickSourceBinding::Pending,
            },
        ));
    }
}

#[test]
fn reliable_bricks_precede_enemy_ball_motion_and_pickup_los_in_the_server_tick() {
    fn empty_brick_fixture_position(state: &ServerState) -> (u16, u16, (u16, u16)) {
        let map = state.zone_map();
        for tile_y in 4..crate::zone::MAP_TILES as u16 - 4 {
            for tile_x in 4..crate::zone::MAP_TILES as u16 - 4 {
                let ship_x = tile_x * 16 + 8;
                let ship_y = tile_y * 16 + 8;
                let center = crate::objectives::brick_tile_from_ship_position(ship_x, ship_y, 0);
                let previous = (center.0.saturating_sub(1), center.1);
                let tile = |(x, y): (u16, u16)| {
                    map.tiles[usize::from(y) * crate::zone::MAP_TILES + usize::from(x)]
                };
                if center.0 > 1
                    && tile(center) == 0
                    && tile(previous) == 0
                    && !crate::zone::world_position_in_safe_tile_for_map(map, ship_x, ship_y)
                {
                    return (ship_x, ship_y, center);
                }
            }
        }
        panic!("test zone needs adjacent static-empty Brick tiles")
    }

    // A runtime-head Brick is applied before the fixed ball step. An
    // enemy-lineage ball that would enter its static-empty center tile bounces
    // this tick, proving the wall is not delayed until the later lifecycle
    // stage.
    let motion_state = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, _rx) = std::sync::mpsc::channel();
    let dropper = register_connection(&motion_state, tx);
    join_arena(&motion_state, dropper, 0);
    let (ship_x, ship_y, center) = {
        let state = motion_state.lock().unwrap();
        empty_brick_fixture_position(&state)
    };
    let tick = crate::wire::current_tick();
    {
        let mut state = motion_state.lock().unwrap();
        let client = state.clients.get_mut(&dropper).unwrap();
        client.last_position = Some(
            crate::arena::large_position_from_client(
                dropper,
                &client_position_packet(0, ship_x, ship_y),
            )
            .unwrap(),
        );
        client.server_weapons.bricks = 1;
        state.objectives.reset(synthetic_profile(0, 1)).unwrap();
        let ball_x = center.0 * 16 - 1;
        let ball_y = center.1 * 16 + 8;
        let ball = state
            .objectives
            .create_powerball(PowerballState::OnMap {
                x: ball_x,
                y: ball_y,
                velocity_x: 1_000,
                velocity_y: 0,
                pickup_unlock_tick: None,
            })
            .unwrap();
        let mut motion = PowerballMotion::from_wire_position(
            ball_x,
            ball_y,
            1_000,
            0,
            0,
            crate::continuum::continuum_tick_sub(tick, 1),
        );
        motion.last_touch_frequency = Some(77);
        state
            .objectives
            .set_powerball_motion(ball.identity, motion)
            .unwrap();
    }
    let mut joined = true;
    crate::connection::handle_client_packet(
        &motion_state,
        dropper,
        reliable(0, &drop_brick_request_payload(ship_x, ship_y)),
        "brick-motion-order",
        &mut joined,
    );
    crate::server_tick::run_server_tick_at_tick(&motion_state, Instant::now(), tick);
    {
        let state = motion_state.lock().unwrap();
        assert_eq!(state.active_bricks.len(), 1);
        assert!(
            state.objectives.powerball_motion(0).unwrap().velocity_x < 0,
            "the enemy-lineage ball collides with the newly ordered Brick"
        );
    }

    // A Brick from another frequency is likewise visible to a later pickup
    // evaluation in this tick. Put the ball inside the new wall and the
    // claimant one adjacent static-empty tile away; proximity passes but LOS
    // rejects, leaving the ball on-map.
    let pickup_state = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (actor_tx, _actor_rx) = std::sync::mpsc::channel();
    let actor = register_connection(&pickup_state, actor_tx);
    join_arena(&pickup_state, actor, 0);
    let (dropper_tx, _dropper_rx) = std::sync::mpsc::channel();
    let enemy = register_connection(&pickup_state, dropper_tx);
    join_arena(&pickup_state, enemy, 0);
    let (ship_x, ship_y, center) = {
        let state = pickup_state.lock().unwrap();
        empty_brick_fixture_position(&state)
    };
    let request = {
        let mut state = pickup_state.lock().unwrap();
        state.clients.get_mut(&enemy).unwrap().player.frequency = 1;
        let enemy_client = state.clients.get_mut(&enemy).unwrap();
        enemy_client.last_position = Some(
            crate::arena::large_position_from_client(
                enemy,
                &client_position_packet(0, ship_x, ship_y),
            )
            .unwrap(),
        );
        enemy_client.server_weapons.bricks = 1;
        state.objectives.reset(synthetic_profile(0, 1)).unwrap();
        let actor_runtime = state.active_player_runtime_identity(actor).unwrap();
        let actor_x = center.0.saturating_sub(1) * 16 + 8;
        let actor_y = center.1 * 16 + 8;
        let witness = position_witness(42, actor_runtime.life.life_seq, actor_x, actor_y);
        let actor_client = state.clients.get_mut(&actor).unwrap();
        actor_client.canonical_position_witnesses.push_back(witness);
        actor_client.canonical_movement_witness = Some(witness);
        actor_client.last_position_sequence = Some(witness.sequence);
        actor_client.server_sim_state = Some(witness.state);
        actor_client.last_position = Some(client_position_packet_with_sequence(
            0,
            witness.wire_x,
            witness.wire_y,
            witness.sequence,
        ));
        let ball = state
            .objectives
            .create_powerball(PowerballState::OnMap {
                x: center.0 * 16 + 8,
                y: center.1 * 16 + 8,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: None,
            })
            .unwrap();
        crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::BallPickup,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: state.clients[&actor].arena_stream_id,
            player_id: actor,
            life_seq: actor_runtime.life.life_seq,
            action_id: 91,
            position_sequence: witness.sequence,
            objective_id: u16::from(ball.identity.id),
            objective_generation: ball.identity.generation,
            expected_version: ball.version.get(),
        }
    };
    let mut enemy_joined = true;
    crate::connection::handle_client_packet(
        &pickup_state,
        enemy,
        reliable(0, &drop_brick_request_payload(ship_x, ship_y)),
        "brick-los-order",
        &mut enemy_joined,
    );
    crate::objectives::relay_objective_action_at_delivery(
        &pickup_state,
        actor,
        0,
        &request.encode(),
    );
    crate::server_tick::run_server_tick_at_tick(
        &pickup_state,
        Instant::now(),
        crate::wire::current_tick(),
    );
    let state = pickup_state.lock().unwrap();
    assert_eq!(state.active_bricks.len(), 1);
    assert!(matches!(
        state.objectives.powerball(0).unwrap().state,
        PowerballState::OnMap { .. }
    ));
}

#[test]
fn deferred_reliable_heads_remain_capacity_resident_and_fair_across_all_hard_boundaries() {
    use crate::state::{
        MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME, MAX_PENDING_RELIABLE_GAMEPLAY_TOTAL,
        PendingReliableGameplayMutation, PendingReliableGameplayMutationKind,
    };

    for head_name in ["ship", "frequency", "leave"] {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
        let (actor_tx, _actor_rx) = std::sync::mpsc::channel();
        let actor = register_connection(&shared, actor_tx);
        join_arena(&shared, actor, 0);
        let (other_tx, _other_rx) = std::sync::mpsc::channel();
        let other = register_connection(&shared, other_tx);
        join_arena(&shared, other, 0);
        {
            let mut state = shared.lock().unwrap();
            let runtime = state.active_player_runtime_identity(actor).unwrap();
            let witness = position_witness(42, runtime.life.life_seq, 320, 320);
            let client = state.clients.get_mut(&actor).unwrap();
            client.canonical_movement_witness = Some(witness);
            client.canonical_position_witnesses.push_back(witness);
            client.server_sim_state = Some(witness.state);
        }
        let frequency = frequency_change_request(&shared, actor, 19);
        let runtime = {
            let mut state = shared.lock().unwrap();
            state.objectives.reset(synthetic_profile(1, 1)).unwrap();
            let runtime = state.active_player_runtime_identity(actor).unwrap();
            let flag = state
                .objectives
                .create_flag(FlagState::Carried {
                    carrier: runtime,
                    x: 320,
                    y: 320,
                    owner: Some(0),
                })
                .unwrap();
            state
                .objectives
                .create_powerball(PowerballState::OnMap {
                    x: 640,
                    y: 640,
                    velocity_x: 0,
                    velocity_y: 0,
                    pickup_unlock_tick: None,
                })
                .unwrap();
            assert!(
                state
                    .objectives
                    .force_flag_version(flag.identity.id, crate::continuum::CONTINUUM_TICK_MASK,)
            );
            state.objectives.force_generation(u64::MAX);
            let head = match head_name {
                "ship" => PendingReliableGameplayMutationKind::Ship(
                    crate::state::ReliableShipChange::parse(&[
                        crate::arena::SHIP_CHANGE_REQUEST,
                        1,
                        43,
                        0,
                    ])
                    .unwrap(),
                ),
                "frequency" => PendingReliableGameplayMutationKind::Frequency(
                    crate::state::ReliableFrequencyChange::parse(&frequency).unwrap(),
                ),
                "leave" => PendingReliableGameplayMutationKind::Leave,
                _ => unreachable!(),
            };
            assert!(state.queue_reliable_gameplay_for_runtime(runtime, u32::MAX, head,));
            let suffixes = [
                PendingReliableGameplayMutationKind::Ship(
                    crate::state::ReliableShipChange::parse(&[
                        crate::arena::SHIP_CHANGE_REQUEST,
                        2,
                        44,
                        0,
                    ])
                    .unwrap(),
                ),
                PendingReliableGameplayMutationKind::Frequency(
                    crate::state::ReliableFrequencyChange::parse(&frequency).unwrap(),
                ),
                PendingReliableGameplayMutationKind::Leave,
                PendingReliableGameplayMutationKind::Attach(
                    crate::state::ReliableAttachRequest::parse(&attach_request_payload(other))
                        .unwrap(),
                ),
                PendingReliableGameplayMutationKind::DropAttached,
                PendingReliableGameplayMutationKind::Brick {
                    request: crate::state::ReliableBrickRequest::parse(
                        &drop_brick_request_payload(320, 320),
                    )
                    .unwrap(),
                    source: crate::state::BrickSourceBinding::Pending,
                },
            ];
            for index in 0..MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME - 1 {
                assert!(state.queue_reliable_gameplay_for_runtime(
                    runtime,
                    index as u32,
                    suffixes[index % suffixes.len()].clone(),
                ));
            }
            let other_runtime = state.active_player_runtime_identity(other).unwrap();
            assert!(state.queue_reliable_gameplay_for_runtime(
                other_runtime,
                0,
                PendingReliableGameplayMutationKind::Brick {
                    request: crate::state::ReliableBrickRequest { x: 320, y: 320 },
                    source: crate::state::BrickSourceBinding::Pending,
                },
            ));
            runtime
        };

        crate::server_tick::run_server_tick_at_tick(
            &shared,
            Instant::now(),
            crate::wire::current_tick(),
        );
        let mut state = shared.lock().unwrap();
        assert_eq!(
            state.pending_reliable_gameplay.runtime_len(runtime),
            MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME,
            "{head_name} and its complete suffix remain capacity-resident"
        );
        assert_eq!(
            state.pending_reliable_gameplay.len(),
            MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME,
            "the independent runtime terminals despite the deferred head"
        );
        assert!(!state.queue_reliable_gameplay_for_runtime(
            runtime,
            100,
            PendingReliableGameplayMutationKind::DropAttached,
        ));
    }

    // Pin the global bound independently from the per-runtime bound. A head
    // snapshot is read-only, so concurrent ingress never observes a drained
    // slot and cannot force a later restore to exceed this capacity.
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, _rx) = std::sync::mpsc::channel();
    let actor = register_connection(&shared, tx);
    join_arena(&shared, actor, 0);
    let mut state = shared.lock().unwrap();
    let actor_runtime = state.active_player_runtime_identity(actor).unwrap();
    for runtime_index in 0..768_u64 {
        let filler_runtime = runtime(runtime_index as u16, 10_000 + runtime_index, 1);
        for delivery_id in 0..MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME as u32 {
            assert!(
                state
                    .pending_reliable_gameplay
                    .enqueue(PendingReliableGameplayMutation {
                        identity: filler_runtime,
                        reliable_delivery_id: delivery_id,
                        kind: PendingReliableGameplayMutationKind::DropAttached,
                    })
            );
        }
    }
    assert!(!state.queue_reliable_gameplay_for_runtime(
        actor_runtime,
        0,
        PendingReliableGameplayMutationKind::Leave,
    ));
    assert_eq!(
        state.pending_reliable_gameplay.len(),
        MAX_PENDING_RELIABLE_GAMEPLAY_TOTAL
    );
}

#[test]
fn reliable_prize_claims_order_inventory_and_timed_effects_with_bricks_across_wrap() {
    fn fixture(
        prize_id: i16,
        next_delivery_id: u32,
    ) -> (
        std::sync::Arc<std::sync::Mutex<ServerState>>,
        u16,
        std::sync::mpsc::Receiver<Vec<u8>>,
        bool,
    ) {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
        let (tx, rx) = std::sync::mpsc::channel();
        let player_id = register_connection(&shared, tx);
        join_arena(&shared, player_id, 0);
        acknowledge_join_bootstrap(&shared, player_id);
        drain_available(&rx);
        let tick = crate::wire::current_tick();
        let mut state = shared.lock().unwrap();
        state.active_hidden_prizes.push(ServerHiddenPrize {
            collectible_id: 900,
            x: 320,
            y: 320,
            prize_id,
            expires_at_tick: crate::continuum::continuum_tick_add(tick, 10_000),
        });
        let client = state.clients.get_mut(&player_id).unwrap();
        client.last_position = Some(
            crate::arena::large_position_from_client(
                player_id,
                &client_position_packet(0, 320, 320),
            )
            .unwrap(),
        );
        client.server_weapons.bricks = 0;
        client
            .incoming_reliable
            .set_next_process_id_for_test(next_delivery_id);
        drop(state);
        (shared, player_id, rx, true)
    }

    fn ingress(
        shared: &std::sync::Arc<std::sync::Mutex<ServerState>>,
        player_id: u16,
        joined: &mut bool,
        delivery_id: u32,
        payload: &[u8],
    ) {
        crate::connection::handle_client_packet(
            shared,
            player_id,
            reliable(delivery_id, payload),
            "reliable-prize-sequencer",
            joined,
        );
    }

    let brick = drop_brick_request_payload(320, 320);
    let prize = take_prize_request_payload(900, 320, 320, 26);

    // The failed zero-inventory Brick is terminal before the later Brick
    // prize. Retrying either delivery cannot reorder or duplicate the claim.
    let (brick_first, player_id, _rx, mut joined) = fixture(26, 0);
    ingress(&brick_first, player_id, &mut joined, 0, &brick);
    ingress(&brick_first, player_id, &mut joined, 0, &brick);
    ingress(&brick_first, player_id, &mut joined, 1, &prize);
    crate::server_tick::run_server_tick_at_tick(
        &brick_first,
        Instant::now(),
        crate::wire::current_tick(),
    );
    {
        let state = brick_first.lock().unwrap();
        assert!(state.active_bricks.is_empty());
        assert_eq!(state.clients[&player_id].server_weapons.bricks, 1);
        assert_eq!(state.claimed_prizes.len(), 1);
        assert!(state.pending_reliable_gameplay.is_empty());
    }

    // Reverse order at the u32 delivery wrap grants inventory before Brick,
    // which then consumes it exactly once at the immutable source geometry.
    let (prize_first, player_id, _rx, mut joined) = fixture(26, u32::MAX);
    ingress(&prize_first, player_id, &mut joined, u32::MAX, &prize);
    ingress(&prize_first, player_id, &mut joined, 0, &brick);
    ingress(&prize_first, player_id, &mut joined, 0, &brick);
    crate::server_tick::run_server_tick_at_tick(
        &prize_first,
        Instant::now(),
        crate::wire::current_tick(),
    );
    {
        let state = prize_first.lock().unwrap();
        assert_eq!(state.active_bricks.len(), 1);
        assert_eq!(state.clients[&player_id].server_weapons.bricks, 0);
        assert_eq!(state.claimed_prizes.len(), 1);
        assert!(state.pending_reliable_gameplay.is_empty());
    }

    // Timed prizes use the same ordered claim transaction, not an immediate
    // reliable-handler side path.
    let (timed, player_id, _rx, mut joined) = fixture(18, 0);
    let timed_prize = take_prize_request_payload(900, 320, 320, 18);
    ingress(&timed, player_id, &mut joined, 0, &timed_prize);
    crate::server_tick::run_server_tick_at_tick(
        &timed,
        Instant::now(),
        crate::wire::current_tick(),
    );
    let state = timed.lock().unwrap();
    assert!(state.clients[&player_id].server_shield_until_tick.is_some());
    assert!(state.pending_reliable_gameplay.is_empty());
    drop(state);

    // Hard life cleanup retires the captured old-runtime claim. The same
    // connection can then submit a fresh delivery under the replacement life.
    let (new_life, player_id, _rx, mut joined) = fixture(26, 0);
    ingress(&new_life, player_id, &mut joined, 0, &prize);
    {
        let mut state = new_life.lock().unwrap();
        let old = state.active_player_runtime_identity(player_id).unwrap();
        state.clear_pending_position_inputs_for_player(player_id);
        state
            .advance_client_life_at_tick(player_id, crate::wire::current_tick())
            .unwrap();
        assert_ne!(state.active_player_runtime_identity(player_id), Some(old));
        assert!(state.pending_reliable_gameplay.is_empty());
    }
    crate::server_tick::run_server_tick_at_tick(
        &new_life,
        Instant::now(),
        crate::wire::current_tick(),
    );
    assert!(new_life.lock().unwrap().claimed_prizes.is_empty());
    ingress(&new_life, player_id, &mut joined, 1, &prize);
    crate::server_tick::run_server_tick_at_tick(
        &new_life,
        Instant::now(),
        crate::wire::current_tick(),
    );
    assert_eq!(new_life.lock().unwrap().claimed_prizes.len(), 1);

    // Session retirement likewise removes the resident request. Reusing the
    // wire player id establishes a distinct session and claims exactly once.
    let (new_session, player_id, _old_rx, mut joined) = fixture(26, 0);
    ingress(&new_session, player_id, &mut joined, 0, &prize);
    let old_session = new_session
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();
    {
        let mut state = new_session.lock().unwrap();
        state.remove_client(player_id).unwrap();
        state.next_player_id = player_id;
    }
    assert!(
        new_session
            .lock()
            .unwrap()
            .pending_reliable_gameplay
            .is_empty()
    );
    let (replacement_tx, replacement_rx) = std::sync::mpsc::channel();
    assert_eq!(register_connection(&new_session, replacement_tx), player_id);
    join_arena(&new_session, player_id, 0);
    acknowledge_join_bootstrap(&new_session, player_id);
    drain_available(&replacement_rx);
    {
        let mut state = new_session.lock().unwrap();
        assert_ne!(
            state.active_player_session_identity(player_id),
            Some(old_session)
        );
        let client = state.clients.get_mut(&player_id).unwrap();
        client.last_position = Some(
            crate::arena::large_position_from_client(
                player_id,
                &client_position_packet(0, 320, 320),
            )
            .unwrap(),
        );
    }
    let mut replacement_joined = true;
    ingress(&new_session, player_id, &mut replacement_joined, 0, &prize);
    crate::server_tick::run_server_tick_at_tick(
        &new_session,
        Instant::now(),
        crate::wire::current_tick(),
    );
    assert_eq!(new_session.lock().unwrap().claimed_prizes.len(), 1);
}

#[test]
fn reliable_correction_orders_exact_lineage_before_or_after_brick_and_objective() {
    struct Fixture {
        shared: std::sync::Arc<std::sync::Mutex<ServerState>>,
        player_id: u16,
        joined: bool,
        application: crate::movement_correction::MovementCorrectionApplied,
        request: crate::objective_protocol::ObjectiveActionRequest,
        corrected: crate::state::CanonicalPositionWitness,
    }

    fn fixture(flag_x: u16, flag_y: u16, application_sequence: u16) -> Fixture {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
        let (tx, rx) = std::sync::mpsc::channel();
        let player_id = register_connection(&shared, tx);
        join_arena(&shared, player_id, 0);
        acknowledge_join_bootstrap(&shared, player_id);
        drain_available(&rx);
        let mut state = shared.lock().unwrap();
        state.objectives.reset(synthetic_profile(1, 0)).unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::OnMap {
                x: flag_x,
                y: flag_y,
                owner: None,
            })
            .unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        let old = position_witness(11, runtime.life.life_seq, 320, 320);
        let corrected_state = crate::state::ServerSimMovement {
            position_x: 640.75,
            position_y: 672.25,
            orientation: 0.25,
            velocity_x: 12.5,
            velocity_y: -3.25,
            energy: 1_000.0,
            max_energy: 1_000.0,
        };
        let corrected = crate::state::CanonicalPositionWitness {
            sequence: 11,
            life_seq: runtime.life.life_seq,
            state: corrected_state,
            wire_x: corrected_state.position_x.round() as u16,
            wire_y: corrected_state.position_y.round() as u16,
            wire_velocity_x: crate::continuum_position::protocol_velocity_component(
                corrected_state.velocity_x,
            ),
            wire_velocity_y: crate::continuum_position::protocol_velocity_component(
                corrected_state.velocity_y,
            ),
            direction: crate::continuum_position::protocol_direction(corrected_state.orientation),
            door_seed: 0,
            door_phase_tick: 0,
        };
        let proposal = crate::movement_correction::MovementCorrection {
            reason: crate::movement_correction::MovementCorrection::REASON_SUSTAINED_DRIFT,
            repel_window: None,
            server_session_id: crate::wire::server_session_id(),
            player_id,
            life_seq: runtime.life.life_seq,
            correction_generation: u32::MAX,
            position_sequence: 10,
            server_tick: crate::wire::current_tick(),
            position_x: corrected_state.position_x,
            position_y: corrected_state.position_y,
            velocity_x: corrected_state.velocity_x,
            velocity_y: corrected_state.velocity_y,
            orientation: corrected_state.orientation,
        };
        let application = crate::movement_correction::MovementCorrectionApplied {
            intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
            server_session_id: proposal.server_session_id,
            player_id,
            life_seq: proposal.life_seq,
            correction_generation: proposal.correction_generation,
            detected_sequence: proposal.position_sequence,
            application_sequence,
        };
        let stream = state.clients[&player_id].arena_stream_id;
        let client = state.clients.get_mut(&player_id).unwrap();
        client.canonical_movement_witness = Some(old);
        client.canonical_position_witnesses.push_back(old);
        client.last_position_sequence = Some(old.sequence);
        client.life_position_sequence_base = Some(old.sequence);
        client.last_input_seq = Some(old.sequence);
        client.last_input_record = Some(crate::state::ClientInputObservation {
            bits: 0,
            direction: old.direction,
        });
        client
            .recent_input_records
            .push_back(crate::state::SequencedInputObservation {
                sequence: old.sequence,
                input: crate::state::ClientInputObservation {
                    bits: 0,
                    direction: old.direction,
                },
                door_seed: 0,
                door_phase_tick: crate::wire::current_tick(),
            });
        client.server_sim_state = Some(old.state);
        client.server_sim_anchor_tick = Some(crate::wire::current_tick());
        client.last_position_server_tick = Some(crate::wire::current_tick());
        client.last_position = Some(client_position_packet_with_sequence(
            0,
            old.wire_x,
            old.wire_y,
            old.sequence,
        ));
        client.server_weapons.bricks = 1;
        client
            .incoming_reliable
            .set_next_process_id_for_test(u32::MAX);
        client.pending_movement_correction = Some(crate::state::ServerMovementCorrection {
            proposal,
            detected_state: old.state,
            replay_witness: Some(corrected),
            replay_server_tick: crate::wire::current_tick(),
            predicted_repel_window: None,
            offer_reliable_id: Some(9),
            application_sequence: None,
            terminal_state: None,
            terminal_server_tick: None,
            cancelled: false,
            receipt_reliable_id: None,
        });
        drop(state);
        Fixture {
            shared,
            player_id,
            joined: true,
            application,
            request: crate::objective_protocol::ObjectiveActionRequest {
                kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: stream,
                player_id,
                life_seq: runtime.life.life_seq,
                action_id: 77,
                position_sequence: application_sequence,
                objective_id: flag.identity.id,
                objective_generation: flag.identity.generation,
                expected_version: flag.version.get(),
            },
            corrected,
        }
    }

    fn ingress(fixture: &mut Fixture, delivery_id: u32, payload: &[u8]) {
        crate::connection::handle_client_packet(
            &fixture.shared,
            fixture.player_id,
            reliable(delivery_id, payload),
            "reliable-correction-sequencer",
            &mut fixture.joined,
        );
    }

    // Correction first: old-lineage positions have already drained, the
    // exact corrected tail commits, then Brick geometry and objective
    // proximity both bind to that corrected witness.
    let mut correction_first = fixture(641, 672, 11);
    let corrected_brick = drop_brick_request_payload(
        correction_first.corrected.wire_x,
        correction_first.corrected.wire_y,
    );
    // The wrapped suffix arrives first and is buffered by reliable ingress;
    // recovering the missing u32::MAX head releases the exact FIFO once.
    ingress(&mut correction_first, 0, &corrected_brick);
    let action = correction_first.request.encode();
    ingress(&mut correction_first, 1, &action);
    let application = correction_first.application.encode();
    ingress(&mut correction_first, u32::MAX, &application);
    ingress(&mut correction_first, u32::MAX, &application);
    crate::server_tick::run_server_tick_at_tick(
        &correction_first.shared,
        Instant::now(),
        crate::wire::current_tick(),
    );
    {
        let state = correction_first.shared.lock().unwrap();
        let witness = state.clients[&correction_first.player_id]
            .canonical_movement_witness
            .unwrap();
        assert_eq!(witness.sequence, 11);
        assert_eq!(
            witness.state.position_x.to_bits(),
            correction_first.corrected.state.position_x.to_bits()
        );
        let center = crate::objectives::brick_tile_from_ship_position(
            correction_first.corrected.wire_x,
            correction_first.corrected.wire_y,
            correction_first.corrected.direction,
        );
        assert!(state.active_bricks[0].tiles.contains(&center));
        assert!(matches!(
            state
                .objectives
                .flag(correction_first.request.objective_id)
                .unwrap()
                .state,
            FlagState::Carried { carrier, .. } if carrier.player_id() == correction_first.player_id
        ));
        assert!(state.pending_reliable_gameplay.is_empty());
    }

    // Reverse order: objective and Brick consume the old source before the
    // correction, even though the final canonical lineage is corrected.
    let mut correction_last = fixture(320, 320, 11);
    let action = correction_last.request.encode();
    ingress(&mut correction_last, u32::MAX, &action);
    let old_brick = drop_brick_request_payload(320, 320);
    ingress(&mut correction_last, 0, &old_brick);
    let application = correction_last.application.encode();
    ingress(&mut correction_last, 1, &application);
    crate::server_tick::run_server_tick_at_tick(
        &correction_last.shared,
        Instant::now(),
        crate::wire::current_tick(),
    );
    {
        let state = correction_last.shared.lock().unwrap();
        let old_center = crate::objectives::brick_tile_from_ship_position(320, 320, 0);
        assert!(state.active_bricks[0].tiles.contains(&old_center));
        assert!(matches!(
            state
                .objectives
                .flag(correction_last.request.objective_id)
                .unwrap()
                .state,
            FlagState::Carried { .. }
        ));
        assert_eq!(
            state.clients[&correction_last.player_id]
                .canonical_movement_witness
                .unwrap()
                .state
                .position_x
                .to_bits(),
            correction_last.corrected.state.position_x.to_bits()
        );
    }

    // A missing earlier objective witness holds the correction. The hostile
    // client can continue one old-lineage input; the position drain admits it
    // first, the objective terminals on that exact witness, and only then is
    // the corrected replay tail committed.
    let mut deferred = fixture(320, 320, 12);
    let action = deferred.request.encode();
    ingress(&mut deferred, u32::MAX, &action);
    let application = deferred.application.encode();
    ingress(&mut deferred, 0, &application);
    let tick = crate::wire::current_tick();
    crate::server_tick::run_server_tick_at_tick(&deferred.shared, Instant::now(), tick);
    assert_eq!(
        deferred
            .shared
            .lock()
            .unwrap()
            .pending_reliable_gameplay
            .len(),
        2
    );
    {
        let mut state = deferred.shared.lock().unwrap();
        let mut position = client_position_packet_with_sequence(0, 320, 320, 12);
        position[18..20].copy_from_slice(&1_000_u16.to_le_bytes());
        state.queue_position_input(deferred.player_id, position);
        let runtime = state
            .active_player_runtime_identity(deferred.player_id)
            .unwrap();
        state.input_recovery_deferred_at.insert(
            runtime.life,
            crate::continuum::continuum_tick_add(tick, crate::continuum::CONTINUUM_TICK_MASK - 11),
        );
    }
    crate::server_tick::run_server_tick_at_tick(
        &deferred.shared,
        Instant::now(),
        crate::continuum::continuum_tick_add(tick, 1),
    );
    let state = deferred.shared.lock().unwrap();
    let witness = state.clients[&deferred.player_id]
        .canonical_movement_witness
        .unwrap();
    assert_eq!(
        witness.sequence,
        12,
        "last={:?} recovery={:?} queue={}",
        state.clients[&deferred.player_id].last_position_sequence,
        state.input_recovery_deferred_at,
        state.pending_reliable_gameplay.len(),
    );
    let terminal_state = state.clients[&deferred.player_id]
        .pending_movement_correction
        .as_ref()
        .and_then(|transaction| transaction.terminal_state)
        .expect("correction application retains its exact terminal witness");
    assert_eq!(
        witness.state.position_x.to_bits(),
        terminal_state.position_x.to_bits()
    );
    assert_ne!(witness.state.position_x.to_bits(), 320.0_f32.to_bits());
    assert!(state.pending_reliable_gameplay.is_empty());
}

#[test]
fn deferred_brick_keeps_immutable_source_geometry_for_twelve_ticks_and_lifecycle() {
    fn fixture() -> (
        std::sync::Arc<std::sync::Mutex<ServerState>>,
        u16,
        bool,
        crate::objective_protocol::ObjectiveActionRequest,
    ) {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
        let (tx, rx) = std::sync::mpsc::channel();
        let player_id = register_connection(&shared, tx);
        join_arena(&shared, player_id, 0);
        acknowledge_join_bootstrap(&shared, player_id);
        drain_available(&rx);
        let mut state = shared.lock().unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        let stream = state.clients[&player_id].arena_stream_id;
        let witness = position_witness(1, runtime.life.life_seq, 320, 320);
        let client = state.clients.get_mut(&player_id).unwrap();
        client.canonical_movement_witness = Some(witness);
        client.canonical_position_witnesses.push_back(witness);
        client.last_position_sequence = Some(1);
        client.server_sim_state = Some(witness.state);
        client.last_position = Some(client_position_packet_with_sequence(0, 320, 320, 1));
        client.server_weapons.bricks = 1;
        client
            .incoming_reliable
            .set_next_process_id_for_test(u32::MAX);
        drop(state);
        (
            shared,
            player_id,
            true,
            crate::objective_protocol::ObjectiveActionRequest {
                kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: stream,
                player_id,
                life_seq: runtime.life.life_seq,
                action_id: 500,
                position_sequence: 2,
                objective_id: 0,
                objective_generation: 1,
                expected_version: 1,
            },
        )
    }

    fn ingress(
        shared: &std::sync::Arc<std::sync::Mutex<ServerState>>,
        player_id: u16,
        joined: &mut bool,
        id: u32,
        payload: &[u8],
    ) {
        crate::connection::handle_client_packet(
            shared,
            player_id,
            reliable(id, payload),
            "frozen-brick-source",
            joined,
        );
    }

    let (shared, player_id, mut joined, action) = fixture();
    ingress(&shared, player_id, &mut joined, u32::MAX, &action.encode());
    ingress(
        &shared,
        player_id,
        &mut joined,
        0,
        &drop_brick_request_payload(320, 320),
    );
    let tick = crate::wire::current_tick();
    crate::server_tick::run_server_tick_at_tick(&shared, Instant::now(), tick);
    for elapsed in 1..=12 {
        let mut state = shared.lock().unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        let mut moved = position_witness(1, runtime.life.life_seq, 800, 864);
        moved.direction = 10;
        moved.state.orientation = 0.25;
        let client = state.clients.get_mut(&player_id).unwrap();
        client.canonical_movement_witness = Some(moved);
        client.server_sim_state = Some(moved.state);
        client.last_position = Some(client_position_packet_with_sequence(
            0,
            moved.wire_x,
            moved.wire_y,
            moved.sequence,
        ));
        drop(state);
        crate::server_tick::run_server_tick_at_tick(
            &shared,
            Instant::now(),
            crate::continuum::continuum_tick_add(tick, elapsed),
        );
        assert!(shared.lock().unwrap().active_bricks.is_empty());
    }
    crate::server_tick::run_server_tick_at_tick(
        &shared,
        Instant::now(),
        crate::continuum::continuum_tick_add(tick, 13),
    );
    {
        let state = shared.lock().unwrap();
        let frozen_center = crate::objectives::brick_tile_from_ship_position(320, 320, 0);
        let rebound_center = crate::objectives::brick_tile_from_ship_position(800, 864, 10);
        assert!(state.active_bricks[0].tiles.contains(&frozen_center));
        assert!(!state.active_bricks[0].tiles.contains(&rebound_center));
        assert!(state.pending_reliable_gameplay.is_empty());
    }

    // The same retained prefix is retired wholesale by a hard life boundary;
    // an old-life frozen wall can never leak into the replacement runtime.
    let (lifecycle, player_id, mut joined, action) = fixture();
    ingress(
        &lifecycle,
        player_id,
        &mut joined,
        u32::MAX,
        &action.encode(),
    );
    ingress(
        &lifecycle,
        player_id,
        &mut joined,
        0,
        &drop_brick_request_payload(320, 320),
    );
    crate::server_tick::run_server_tick_at_tick(&lifecycle, Instant::now(), tick);
    {
        let mut state = lifecycle.lock().unwrap();
        state.clear_pending_position_inputs_for_player(player_id);
        state.advance_client_life_at_tick(player_id, tick).unwrap();
        assert!(state.pending_reliable_gameplay.is_empty());
    }
    crate::server_tick::run_server_tick_at_tick(
        &lifecycle,
        Instant::now(),
        crate::continuum::continuum_tick_add(tick, 20),
    );
    assert!(lifecycle.lock().unwrap().active_bricks.is_empty());
}

#[test]
fn reliable_gameplay_sequencer_has_one_mixed_aggregate_cap_and_linear_head_cost() {
    use crate::state::{
        BrickSourceBinding, BrickSourceGeometry, MAX_PENDING_RELIABLE_GAMEPLAY_BYTES,
        MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME, MAX_PENDING_RELIABLE_GAMEPLAY_TOTAL,
        PendingObjectiveAction, PendingReliableGameplayMutation,
        PendingReliableGameplayMutationKind, ReliableGameplaySequencer,
    };

    let started = Instant::now();
    let mut sequencer = ReliableGameplaySequencer::default();
    let mut sources = std::collections::BTreeMap::new();
    for runtime_index in 0..768_u64 {
        let identity = runtime(runtime_index as u16, 50_000 + runtime_index, 1);
        sources.insert(
            identity,
            Some(BrickSourceGeometry {
                x: 320,
                y: 320,
                direction: 0,
                ship: 0,
                frequency: runtime_index as u16,
            }),
        );
        for delivery_id in 0..MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME as u32 {
            let kind = if delivery_id as usize == MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME - 1 {
                match runtime_index % 4 {
                    0 => PendingReliableGameplayMutationKind::Frequency(
                        crate::state::ReliableFrequencyChange {
                            packet: [0; crate::state::RELIABLE_FREQUENCY_CHANGE_PACKET_LEN],
                        },
                    ),
                    1 => PendingReliableGameplayMutationKind::Ship(
                        crate::state::ReliableShipChange {
                            ship: 1,
                            position_sequence_base: 1,
                        },
                    ),
                    2 => PendingReliableGameplayMutationKind::Leave,
                    _ => PendingReliableGameplayMutationKind::MovementCorrection(
                        crate::movement_correction::MovementCorrectionApplied {
                            intent:
                                crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
                            server_session_id: crate::wire::server_session_id(),
                            player_id: identity.player_id(),
                            life_seq: identity.life.life_seq,
                            correction_generation: 1,
                            detected_sequence: 1,
                            application_sequence: 1,
                        },
                    ),
                }
            } else {
                match delivery_id % 5 {
                    0 => PendingReliableGameplayMutationKind::Objective(PendingObjectiveAction {
                        identity,
                        reliable_delivery_id: delivery_id,
                        request: crate::objective_protocol::ObjectiveActionRequest {
                            kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
                            server_session_id: crate::wire::server_session_id(),
                            arena_stream_id: 1,
                            player_id: identity.player_id(),
                            life_seq: identity.life.life_seq,
                            action_id: delivery_id,
                            position_sequence: delivery_id as u16,
                            objective_id: 0,
                            objective_generation: 1,
                            expected_version: 1,
                        },
                        queued_at_tick: 1,
                    }),
                    1 => PendingReliableGameplayMutationKind::Brick {
                        request: crate::state::ReliableBrickRequest { x: 320, y: 320 },
                        source: BrickSourceBinding::Pending,
                    },
                    2 => PendingReliableGameplayMutationKind::Prize(
                        crate::state::ReliablePrizeRequest {
                            collectible_id: delivery_id + 1,
                        },
                    ),
                    3 => PendingReliableGameplayMutationKind::Attach(
                        crate::state::ReliableAttachRequest { destination_id: 0 },
                    ),
                    _ => PendingReliableGameplayMutationKind::DropAttached,
                }
            };
            assert!(sequencer.enqueue(PendingReliableGameplayMutation {
                identity,
                reliable_delivery_id: delivery_id,
                kind,
            }));
        }
    }
    assert_eq!(sequencer.len(), MAX_PENDING_RELIABLE_GAMEPLAY_TOTAL);
    assert_eq!(
        MAX_PENDING_RELIABLE_GAMEPLAY_BYTES,
        MAX_PENDING_RELIABLE_GAMEPLAY_TOTAL
            * std::mem::size_of::<PendingReliableGameplayMutation>()
    );
    assert!(
        std::mem::size_of::<PendingReliableGameplayMutation>() <= 128,
        "fixed mutation storage unexpectedly grew: {} bytes",
        std::mem::size_of::<PendingReliableGameplayMutation>()
    );
    let first_identity = runtime(0, 50_000, 1);
    assert_eq!(
        sequencer.runtime_len(first_identity),
        MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME
    );
    let duplicate = sequencer.runtime_head(first_identity).unwrap();
    assert!(
        sequencer.enqueue(duplicate),
        "a resident retry stays idempotent even at the aggregate cap"
    );
    assert!(!sequencer.enqueue(PendingReliableGameplayMutation {
        identity: runtime(u16::MAX, u64::MAX, 1),
        reliable_delivery_id: 0,
        kind: PendingReliableGameplayMutationKind::DropAttached,
    }));

    sequencer.bind_brick_sources(&sources);
    let ready = sequencer.fair_runtime_identities();
    assert_eq!(ready.len(), 768);
    for identity in ready {
        let head = sequencer.runtime_head(identity).unwrap();
        assert!(sequencer.pop_terminal(&head));
    }
    assert_eq!(sequencer.len(), MAX_PENDING_RELIABLE_GAMEPLAY_TOTAL - 768);
    assert_eq!(
        sequencer.operation_counts(),
        (768, MAX_PENDING_RELIABLE_GAMEPLAY_TOTAL as u64),
        "source capture visits each newly admitted prefix item once and head scheduling visits one item per active runtime"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "49,152 mixed admissions, source bindings, and one fair head pass should stay comfortably bounded in debug tests"
    );
    sequencer.retain_runtime(|_| false);
    assert_eq!(sequencer.len(), 0);
    assert_eq!(
        sequencer.container_counts(),
        (0, 0, 0, 0),
        "retiring the saturated aggregate releases every per-runtime deque, delivery set, and ready-index entry"
    );
}

#[test]
fn production_tick_enforces_global_reliable_work_budget_and_rotates_fairly() {
    use crate::authority_identity::PlayerSessionId;
    use crate::state::{
        BrickSourceBinding, PendingObjectiveAction, PendingReliableGameplayMutationKind,
        ReliableAttachRequest, ReliableBrickRequest, ReliablePrizeRequest,
    };

    const RUNTIMES: u16 = 300;
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, _rx) = std::sync::mpsc::channel();
    let template_id = register_connection(&shared, tx);
    join_arena(&shared, template_id, 0);
    acknowledge_join_bootstrap(&shared, template_id);
    let tick = 20_000;
    {
        let mut state = shared.lock().unwrap();
        let template = state.clients[&template_id].clone();
        for player_id in 0..RUNTIMES {
            if player_id != template_id {
                let mut client = template.clone();
                client.player.id = player_id;
                client.player.name = format!("budget-{player_id}");
                client.session_id = PlayerSessionId::new(u64::from(player_id) + 10_000);
                client.life_seq = 1;
                client.arena_stream_id = u64::from(player_id) + 1;
                client.ingress_gate = std::sync::Arc::new(std::sync::Mutex::new(()));
                client.pending_reset_application_terminals.clear();
                state.clients.insert(player_id, client);
            }
            let identity = state.active_player_runtime_identity(player_id).unwrap();
            let kind = match player_id % 5 {
                0 => PendingReliableGameplayMutationKind::DropAttached,
                1 => PendingReliableGameplayMutationKind::Attach(ReliableAttachRequest {
                    destination_id: u16::MAX,
                }),
                2 => PendingReliableGameplayMutationKind::Brick {
                    request: ReliableBrickRequest { x: 320, y: 320 },
                    source: BrickSourceBinding::Pending,
                },
                3 => PendingReliableGameplayMutationKind::Prize(ReliablePrizeRequest {
                    collectible_id: u32::from(player_id) + 1,
                }),
                _ => PendingReliableGameplayMutationKind::Objective(PendingObjectiveAction {
                    identity,
                    reliable_delivery_id: u32::from(player_id),
                    request: crate::objective_protocol::ObjectiveActionRequest {
                        kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
                        server_session_id: crate::wire::server_session_id(),
                        arena_stream_id: state.clients[&player_id].arena_stream_id,
                        player_id,
                        life_seq: identity.life.life_seq,
                        action_id: u32::from(player_id) + 1,
                        position_sequence: 1,
                        objective_id: 0,
                        objective_generation: 1,
                        expected_version: 1,
                    },
                    queued_at_tick: tick - 100,
                }),
            };
            assert!(state.queue_reliable_gameplay_for_runtime(
                identity,
                u32::from(player_id),
                kind,
            ));
        }
    }

    let started = Instant::now();
    crate::server_tick::run_server_tick_at_tick(&shared, Instant::now(), tick);
    let representative_elapsed = started.elapsed();
    {
        let state = shared.lock().unwrap();
        assert_eq!(
            state.pending_reliable_gameplay.len(),
            usize::from(RUNTIMES) - crate::server_tick::MAX_RELIABLE_GAMEPLAY_WORK_PER_TICK
        );
        for player_id in 0..RUNTIMES {
            let identity = state.active_player_runtime_identity(player_id).unwrap();
            assert_eq!(
                state.pending_reliable_gameplay.runtime_len(identity),
                usize::from(
                    player_id >= crate::server_tick::MAX_RELIABLE_GAMEPLAY_WORK_PER_TICK as u16
                ),
                "the rotating index gives every earlier ready runtime one bounded work slot"
            );
        }
        assert_eq!(
            state.pending_reliable_gameplay.operation_counts().0,
            crate::server_tick::MAX_RELIABLE_GAMEPLAY_WORK_PER_TICK as u64
        );
    }

    crate::server_tick::run_server_tick_at_tick(&shared, Instant::now(), tick + 1);
    assert!(shared.lock().unwrap().pending_reliable_gameplay.is_empty());
    eprintln!(
        "representative mixed 300-runtime reliable budget tick: {:?}; deterministic work cap={} (timing informational only)",
        representative_elapsed,
        crate::server_tick::MAX_RELIABLE_GAMEPLAY_WORK_PER_TICK,
    );
}

#[test]
fn production_tick_charges_hard_lifecycle_suffix_cleanup_against_global_budget() {
    use crate::state::{
        MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME, PendingReliableGameplayMutationKind,
    };

    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let mut player_ids = Vec::new();
    for _ in 0..5 {
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = register_connection(&shared, tx);
        join_arena(&shared, player_id, 0);
        player_ids.push(player_id);
    }
    {
        let mut state = shared.lock().unwrap();
        for &player_id in &player_ids {
            let identity = state.active_player_runtime_identity(player_id).unwrap();
            assert!(state.queue_reliable_gameplay_for_runtime(
                identity,
                0,
                PendingReliableGameplayMutationKind::Leave,
            ));
            for delivery_id in 1..MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME as u32 {
                assert!(state.queue_reliable_gameplay_for_runtime(
                    identity,
                    delivery_id,
                    PendingReliableGameplayMutationKind::DropAttached,
                ));
            }
        }
    }

    crate::server_tick::run_server_tick_at_tick(&shared, Instant::now(), 30_000);
    {
        let state = shared.lock().unwrap();
        assert_eq!(
            state.pending_reliable_gameplay.len(),
            MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME,
            "four 64-entry hard boundaries consume the complete 256-work budget"
        );
        assert!(
            player_ids[..4]
                .iter()
                .all(|player_id| !state.clients[player_id].joined)
        );
        assert!(state.clients[&player_ids[4]].joined);
    }
    crate::server_tick::run_server_tick_at_tick(&shared, Instant::now(), 30_001);
    let state = shared.lock().unwrap();
    assert!(state.pending_reliable_gameplay.is_empty());
    assert!(
        player_ids
            .iter()
            .all(|player_id| !state.clients[player_id].joined)
    );
}

#[test]
fn brick_prefix_binding_reuses_one_boundary_and_absence_never_revives() {
    use crate::state::{
        BrickSourceBinding, BrickSourceGeometry, PendingReliableGameplayMutation,
        PendingReliableGameplayMutationKind, ReliableAttachRequest, ReliableBrickRequest,
        ReliableGameplaySequencer, ReliablePrizeRequest, ReliableShipChange,
    };

    let source = BrickSourceGeometry {
        x: 320,
        y: 336,
        direction: 7,
        ship: 2,
        frequency: 9,
    };
    let replacement = BrickSourceGeometry {
        x: 800,
        y: 864,
        direction: 21,
        ship: 4,
        frequency: 12,
    };
    let brick = || PendingReliableGameplayMutationKind::Brick {
        request: ReliableBrickRequest { x: 320, y: 336 },
        source: BrickSourceBinding::Pending,
    };
    let enqueue = |sequencer: &mut ReliableGameplaySequencer, identity, delivery_id, kind| {
        assert!(sequencer.enqueue(PendingReliableGameplayMutation {
            identity,
            reliable_delivery_id: delivery_id,
            kind,
        }));
    };

    // Two wrapped Bricks revealed at one boundary bind to exactly the same
    // immutable geometry even after the first terminal pops.
    let paired = runtime(10, 10, 1);
    let mut sequencer = ReliableGameplaySequencer::default();
    enqueue(&mut sequencer, paired, u32::MAX, brick());
    enqueue(&mut sequencer, paired, 0, brick());
    sequencer.bind_brick_sources(&std::collections::BTreeMap::from([(paired, Some(source))]));
    for delivery_id in [u32::MAX, 0] {
        let head = sequencer.runtime_head(paired).unwrap();
        assert_eq!(head.reliable_delivery_id, delivery_id);
        assert!(matches!(
            head.kind,
            PendingReliableGameplayMutationKind::Brick {
                source: BrickSourceBinding::Bound(bound), ..
            } if bound == source
        ));
        assert!(sequencer.pop_terminal(&head));
    }

    // A source boundary is a one-shot inspection epoch, not sticky runtime
    // state. A Brick admitted after an empty boundary waits for the next
    // explicit position/correction boundary and binds only that newer source.
    let empty_epoch = runtime(14, 40, 1);
    let mut sequencer = ReliableGameplaySequencer::default();
    sequencer.bind_brick_sources(&std::collections::BTreeMap::from([(
        empty_epoch,
        Some(source),
    )]));
    enqueue(&mut sequencer, empty_epoch, u32::MAX, brick());
    assert!(matches!(
        sequencer.runtime_head(empty_epoch).unwrap().kind,
        PendingReliableGameplayMutationKind::Brick {
            source: BrickSourceBinding::Pending,
            ..
        }
    ));
    sequencer.bind_brick_source_for_runtime(empty_epoch, Some(replacement));
    assert!(matches!(
        sequencer.runtime_head(empty_epoch).unwrap().kind,
        PendingReliableGameplayMutationKind::Brick {
            source: BrickSourceBinding::Bound(bound), ..
        } if bound == replacement
    ));

    // The same rule holds while an already-inspected non-Brick prefix stays
    // resident across positions A and B. A later Brick cannot consume either
    // completed epoch; authoritative no-position at C terminals it and a
    // later position cannot revive it.
    let retained_epoch = runtime(15, 41, 1);
    let mut sequencer = ReliableGameplaySequencer::default();
    enqueue(
        &mut sequencer,
        retained_epoch,
        1,
        PendingReliableGameplayMutationKind::Prize(ReliablePrizeRequest { collectible_id: 7 }),
    );
    sequencer.bind_brick_source_for_runtime(retained_epoch, Some(source));
    sequencer.bind_brick_source_for_runtime(retained_epoch, Some(replacement));
    enqueue(&mut sequencer, retained_epoch, 2, brick());
    assert!(matches!(
        sequencer.runtime_items(retained_epoch).nth(1).unwrap().kind,
        PendingReliableGameplayMutationKind::Brick {
            source: BrickSourceBinding::Pending,
            ..
        }
    ));
    sequencer.bind_brick_source_for_runtime(retained_epoch, None);
    let prize = sequencer.runtime_head(retained_epoch).unwrap();
    assert!(sequencer.pop_terminal(&prize));
    assert!(matches!(
        sequencer.runtime_head(retained_epoch).unwrap().kind,
        PendingReliableGameplayMutationKind::Brick {
            source: BrickSourceBinding::Unavailable,
            ..
        }
    ));
    sequencer.bind_brick_source_for_runtime(retained_epoch, Some(source));
    assert!(matches!(
        sequencer.runtime_head(retained_epoch).unwrap().kind,
        PendingReliableGameplayMutationKind::Brick {
            source: BrickSourceBinding::Unavailable,
            ..
        }
    ));

    // Prize/Attach/DropAttached are members of the same revealed prefix. An
    // authoritative source absence marks every following Brick unavailable;
    // a future position boundary cannot resurrect any of them.
    for (case, prefix) in [
        (
            "prize",
            vec![PendingReliableGameplayMutationKind::Prize(
                ReliablePrizeRequest { collectible_id: 1 },
            )],
        ),
        (
            "attach-drop",
            vec![
                PendingReliableGameplayMutationKind::Attach(ReliableAttachRequest {
                    destination_id: 22,
                }),
                PendingReliableGameplayMutationKind::DropAttached,
            ],
        ),
    ] {
        let identity = runtime(if case == "prize" { 11 } else { 12 }, 20, 1);
        let mut sequencer = ReliableGameplaySequencer::default();
        for (index, kind) in prefix.into_iter().chain([brick(), brick()]).enumerate() {
            enqueue(&mut sequencer, identity, index as u32, kind);
        }
        sequencer.bind_brick_sources(&std::collections::BTreeMap::from([(identity, None)]));
        while let Some(head) = sequencer.runtime_head(identity) {
            if matches!(head.kind, PendingReliableGameplayMutationKind::Brick { .. }) {
                assert!(matches!(
                    head.kind,
                    PendingReliableGameplayMutationKind::Brick {
                        source: BrickSourceBinding::Unavailable,
                        ..
                    }
                ));
            }
            assert!(sequencer.pop_terminal(&head));
            sequencer.bind_brick_source_for_runtime(identity, Some(replacement));
        }
    }

    // A hard lifecycle/correction prerequisite ends the old prefix. Only
    // after that exact head terminals may the suffix capture a new boundary.
    let barrier = runtime(13, 30, 1);
    let mut sequencer = ReliableGameplaySequencer::default();
    enqueue(
        &mut sequencer,
        barrier,
        1,
        PendingReliableGameplayMutationKind::Ship(ReliableShipChange {
            ship: 3,
            position_sequence_base: 9,
        }),
    );
    enqueue(&mut sequencer, barrier, 2, brick());
    sequencer.bind_brick_sources(&std::collections::BTreeMap::from([(barrier, Some(source))]));
    let lifecycle = sequencer.runtime_head(barrier).unwrap();
    assert!(sequencer.pop_terminal(&lifecycle));
    assert!(matches!(
        sequencer.runtime_head(barrier).unwrap().kind,
        PendingReliableGameplayMutationKind::Brick {
            source: BrickSourceBinding::Pending,
            ..
        }
    ));
    sequencer.bind_brick_source_for_runtime(barrier, Some(replacement));
    assert!(matches!(
        sequencer.runtime_head(barrier).unwrap().kind,
        PendingReliableGameplayMutationKind::Brick {
            source: BrickSourceBinding::Bound(bound), ..
        } if bound == replacement
    ));
}

#[test]
fn reliable_connection_saturation_withholds_ack_and_retries_every_gameplay_class() {
    use crate::state::{
        MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME, PendingReliableGameplayMutationKind,
    };

    for class in [
        "objective",
        "brick",
        "prize",
        "correction",
        "frequency",
        "ship",
        "leave",
        "attach",
        "drop-attached",
    ] {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
        let (tx, rx) = std::sync::mpsc::channel();
        let player_id = register_connection(&shared, tx);
        join_arena(&shared, player_id, 0);
        acknowledge_join_bootstrap(&shared, player_id);
        drain_available(&rx);
        let (runtime, stream) = {
            let mut state = shared.lock().unwrap();
            let runtime = state.active_player_runtime_identity(player_id).unwrap();
            let stream = state.clients[&player_id].arena_stream_id;
            state.clients.get_mut(&player_id).unwrap().alive = true;
            state.clients.get_mut(&player_id).unwrap().incoming_reliable = Default::default();
            state
                .clients
                .get_mut(&player_id)
                .unwrap()
                .incoming_reliable
                .set_next_process_id_for_test(u32::MAX);
            for delivery_id in 1_000..1_000 + MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME as u32 {
                assert!(state.queue_reliable_gameplay_for_runtime(
                    runtime,
                    delivery_id,
                    PendingReliableGameplayMutationKind::DropAttached,
                ));
            }
            (runtime, stream)
        };
        let payload = match class {
            "objective" => crate::objective_protocol::ObjectiveActionRequest {
                kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: stream,
                player_id,
                life_seq: runtime.life.life_seq,
                action_id: 900,
                position_sequence: 1,
                objective_id: 0,
                objective_generation: 1,
                expected_version: 1,
            }
            .encode(),
            "brick" => drop_brick_request_payload(320, 320),
            "prize" => take_prize_request_payload(44, 320, 320, 26),
            "correction" => crate::movement_correction::MovementCorrectionApplied {
                intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
                server_session_id: crate::wire::server_session_id(),
                player_id,
                life_seq: runtime.life.life_seq,
                correction_generation: 7,
                detected_sequence: 1,
                application_sequence: 2,
            }
            .encode(),
            "frequency" => frequency_change_request(&shared, player_id, 1),
            "ship" => vec![crate::arena::SHIP_CHANGE_REQUEST, 1, 1, 0],
            "leave" => vec![crate::arena::LEAVE_ARENA_REQUEST],
            "attach" => attach_request_payload(u16::MAX),
            "drop-attached" => vec![crate::objectives::DROP_ATTACHED_REQUEST],
            _ => unreachable!(),
        };
        let mut joined = true;

        // Retain the wrapped suffix first. A wrapper ahead of the exact
        // cursor is durable but not ACK-eligible and cannot enter gameplay
        // application order while the missing head is capacity-blocked.
        crate::connection::handle_client_packet(
            &shared,
            player_id,
            reliable(0, &[crate::objectives::DROP_ATTACHED_REQUEST]),
            "reliable-gap-suffix",
            &mut joined,
        );
        assert!(rx.try_iter().all(|packet| !is_reliable_ack(&packet)));

        for _ in 0..2 {
            crate::connection::handle_client_packet(
                &shared,
                player_id,
                reliable(u32::MAX, &payload),
                "reliable-capacity-retry",
                &mut joined,
            );
            assert!(
                rx.try_iter()
                    .collect::<Vec<_>>()
                    .iter()
                    .all(|packet| !is_reliable_ack(packet)),
                "{class} capacity rejection must remain unacknowledged"
            );
            let state = shared.lock().unwrap();
            assert_eq!(
                state.pending_reliable_gameplay.runtime_len(runtime),
                MAX_PENDING_RELIABLE_GAMEPLAY_PER_RUNTIME,
                "{class} must remain in the durable receive backlog"
            );
            assert_eq!(
                state.clients[&player_id]
                    .incoming_reliable
                    .dispatch_backlog_len(),
                2
            );
        }
        assert!(
            joined,
            "Leave closes the RTC gate only after durable admission"
        );

        {
            let mut state = shared.lock().unwrap();
            let head = state.reliable_gameplay_head_for_runtime(runtime).unwrap();
            assert!(state.pop_reliable_gameplay_terminal(&head));
        }
        crate::connection::handle_client_packet(
            &shared,
            player_id,
            reliable(u32::MAX, &payload),
            "reliable-capacity-retry",
            &mut joined,
        );
        let packets = rx.try_iter().collect::<Vec<_>>();
        assert!(
            packets.iter().any(|packet| is_reliable_ack(packet)),
            "{class}"
        );
        {
            let state = shared.lock().unwrap();
            assert_eq!(state.pending_reliable_gameplay.runtime_len(runtime), 64);
            assert_eq!(
                state.clients[&player_id]
                    .incoming_reliable
                    .dispatch_backlog_len(),
                1,
                "the admitted wrapped head cannot ACK or discard its capacity-blocked suffix"
            );
        }
        assert_eq!(joined, class != "leave");

        {
            let mut state = shared.lock().unwrap();
            let head = state.reliable_gameplay_head_for_runtime(runtime).unwrap();
            assert!(state.pop_reliable_gameplay_terminal(&head));
        }
        crate::connection::handle_client_packet(
            &shared,
            player_id,
            reliable(0, &[crate::objectives::DROP_ATTACHED_REQUEST]),
            "reliable-gap-suffix-retry",
            &mut joined,
        );
        assert!(rx.try_iter().any(|packet| is_reliable_ack(&packet)));
        {
            let state = shared.lock().unwrap();
            assert_eq!(state.pending_reliable_gameplay.runtime_len(runtime), 64);
            assert_eq!(
                state.clients[&player_id]
                    .incoming_reliable
                    .dispatch_backlog_len(),
                0
            );
        }

        // A transport retry after admission is a resident duplicate at full
        // capacity: it ACKs idempotently and cannot append a 65th mutation.
        crate::connection::handle_client_packet(
            &shared,
            player_id,
            reliable(u32::MAX, &payload),
            "reliable-capacity-duplicate",
            &mut joined,
        );
        assert!(rx.try_iter().any(|packet| is_reliable_ack(&packet)));
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .pending_reliable_gameplay
                .runtime_len(runtime),
            64
        );
    }
}

#[test]
fn reliable_gap_terminals_stale_head_then_malformed_wrapped_suffix_in_order() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    acknowledge_join_bootstrap(&shared, player_id);
    drain_available(&rx);
    let runtime = {
        let mut state = shared.lock().unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        state.clients.get_mut(&player_id).unwrap().incoming_reliable = Default::default();
        state
            .clients
            .get_mut(&player_id)
            .unwrap()
            .incoming_reliable
            .set_next_process_id_for_test(u32::MAX);
        runtime
    };
    let mut malformed_suffix = crate::objective_protocol::ObjectiveActionRequest {
        kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
        server_session_id: crate::wire::server_session_id(),
        arena_stream_id: 1,
        player_id,
        life_seq: runtime.life.life_seq,
        action_id: 8,
        position_sequence: 1,
        objective_id: 0,
        objective_generation: 1,
        expected_version: 1,
    }
    .encode();
    malformed_suffix.push(0xff);
    let stale_head = crate::movement_correction::MovementCorrectionApplied {
        intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
        server_session_id: crate::wire::server_session_id(),
        player_id,
        life_seq: runtime.life.life_seq.wrapping_add(1),
        correction_generation: 1,
        detected_sequence: 1,
        application_sequence: 2,
    }
    .encode();
    let mut joined = true;
    crate::connection::handle_client_packet(
        &shared,
        player_id,
        reliable(0, &malformed_suffix),
        "reliable-terminal-gap",
        &mut joined,
    );
    assert!(rx.try_iter().all(|packet| !is_reliable_ack(&packet)));
    crate::connection::handle_client_packet(
        &shared,
        player_id,
        reliable(u32::MAX, &stale_head),
        "reliable-terminal-head",
        &mut joined,
    );
    let ack_ids = rx
        .try_iter()
        .filter(|packet| is_reliable_ack(packet))
        .map(|packet| u32::from_le_bytes(packet[2..6].try_into().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(ack_ids, vec![u32::MAX, 0]);
    let state = shared.lock().unwrap();
    assert!(state.pending_reliable_gameplay.is_empty());
    assert_eq!(
        state.clients[&player_id]
            .incoming_reliable
            .dispatch_backlog_len(),
        0
    );
}

#[test]
fn sequenced_gameplay_framing_rejects_trailing_and_chunk_oversize_for_every_kind() {
    for class in [
        "objective",
        "brick",
        "prize",
        "correction",
        "frequency",
        "ship",
        "leave",
        "attach",
        "drop-attached",
    ] {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
        let (tx, rx) = std::sync::mpsc::channel();
        let player_id = register_connection(&shared, tx);
        join_arena(&shared, player_id, 0);
        acknowledge_join_bootstrap(&shared, player_id);
        drain_available(&rx);
        let (runtime, stream) = {
            let state = shared.lock().unwrap();
            (
                state.active_player_runtime_identity(player_id).unwrap(),
                state.clients[&player_id].arena_stream_id,
            )
        };
        let exact = match class {
            "objective" => crate::objective_protocol::ObjectiveActionRequest {
                kind: crate::objective_protocol::ObjectiveActionKind::FlagClaim,
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: stream,
                player_id,
                life_seq: runtime.life.life_seq,
                action_id: 901,
                position_sequence: 1,
                objective_id: 0,
                objective_generation: 1,
                expected_version: 1,
            }
            .encode(),
            "brick" => drop_brick_request_payload(320, 320),
            "prize" => take_prize_request_payload(45, 320, 320, 26),
            "correction" => crate::movement_correction::MovementCorrectionApplied {
                intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
                server_session_id: crate::wire::server_session_id(),
                player_id,
                life_seq: runtime.life.life_seq,
                correction_generation: 8,
                detected_sequence: 1,
                application_sequence: 2,
            }
            .encode(),
            "frequency" => frequency_change_request(&shared, player_id, 1),
            "ship" => vec![crate::arena::SHIP_CHANGE_REQUEST, 1, 1, 0],
            "leave" => vec![crate::arena::LEAVE_ARENA_REQUEST],
            "attach" => attach_request_payload(u16::MAX),
            "drop-attached" => vec![crate::objectives::DROP_ATTACHED_REQUEST],
            _ => unreachable!(),
        };
        let mut joined = true;

        let mut trailing = exact.clone();
        trailing.push(0xa5);
        crate::connection::handle_client_packet(
            &shared,
            player_id,
            reliable(0, &trailing),
            "exact-framing",
            &mut joined,
        );
        assert!(rx.try_iter().any(|packet| is_reliable_ack(&packet)));
        assert!(shared.lock().unwrap().pending_reliable_gameplay.is_empty());

        let mut oversized = exact.clone();
        oversized.extend(std::iter::repeat_n(0x5a, 600));
        let split = 400;
        let mut body = vec![CORE_BYTE, crate::reliable::SMALL_CHUNK_BODY];
        body.extend_from_slice(&oversized[..split]);
        let mut tail = vec![CORE_BYTE, crate::reliable::SMALL_CHUNK_TAIL];
        tail.extend_from_slice(&oversized[split..]);
        crate::connection::handle_client_packet(
            &shared,
            player_id,
            reliable(1, &body),
            "chunk-framing",
            &mut joined,
        );
        crate::connection::handle_client_packet(
            &shared,
            player_id,
            reliable(2, &tail),
            "chunk-framing",
            &mut joined,
        );
        assert_eq!(
            rx.try_iter()
                .collect::<Vec<_>>()
                .iter()
                .filter(|packet| is_reliable_ack(packet))
                .count(),
            2,
            "{class} chunk frames terminal independently"
        );
        assert!(shared.lock().unwrap().pending_reliable_gameplay.is_empty());

        crate::connection::handle_client_packet(
            &shared,
            player_id,
            reliable(3, &exact),
            "exact-framing",
            &mut joined,
        );
        assert!(rx.try_iter().any(|packet| is_reliable_ack(&packet)));
        let state = shared.lock().unwrap();
        assert_eq!(
            state.pending_reliable_gameplay.runtime_len(runtime),
            1,
            "{class}"
        );
        assert!(std::mem::size_of::<crate::state::PendingReliableGameplayMutation>() <= 128);
    }
}

#[test]
fn already_retired_correction_application_gets_explicit_cancel_terminal() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    acknowledge_join_bootstrap(&shared, player_id);
    drain_available(&rx);
    let runtime = {
        let mut state = shared.lock().unwrap();
        let runtime = state.active_player_runtime_identity(player_id).unwrap();
        let witness = position_witness(5, runtime.life.life_seq, 320, 336);
        let client = state.clients.get_mut(&player_id).unwrap();
        client.canonical_movement_witness = Some(witness);
        client.canonical_position_witnesses.push_back(witness);
        runtime
    };
    let application = crate::movement_correction::MovementCorrectionApplied {
        intent: crate::movement_correction::MovementCorrectionApplied::INTENT_APPLY,
        server_session_id: crate::wire::server_session_id(),
        player_id,
        life_seq: runtime.life.life_seq,
        correction_generation: 19,
        detected_sequence: 4,
        application_sequence: 5,
    };
    let mut joined = true;
    crate::connection::handle_client_packet(
        &shared,
        player_id,
        reliable(0, &application.encode()),
        "retired-correction-terminal",
        &mut joined,
    );
    crate::server_tick::run_server_tick_at_tick(
        &shared,
        Instant::now(),
        crate::wire::current_tick(),
    );

    let terminal = rx
        .try_iter()
        .filter_map(|packet| reliable_payload(&packet).map(Vec::from))
        .find_map(|payload| {
            crate::movement_correction::MovementCorrectionAccepted::parse(&payload).ok()
        })
        .expect("valid current-life application receives a terminal");
    assert_eq!(
        terminal.outcome,
        crate::movement_correction::MovementCorrectionAccepted::OUTCOME_CANCELLED
    );
    assert_eq!(terminal.correction_generation, 19);
    assert_eq!(terminal.application_sequence, 5);
    assert!(shared.lock().unwrap().pending_reliable_gameplay.is_empty());
}

#[test]
fn safe_pickups_are_allowed_while_attachment_and_blocked_los_fail_closed() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(1, 1));
    let flag = registry
        .create_flag(FlagState::OnMap {
            x: 24,
            y: 24,
            owner: None,
        })
        .unwrap();
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 24,
            y: 24,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let mut actor = player(4, 9, 3, 7, 24, 24);
    actor.safe = true;
    assert!(claim_flag(&mut registry, actor, flag.identity, flag.version, 1).is_ok());

    actor.attached = true;
    assert_eq!(
        pickup_powerball(&mut registry, actor, ball.identity, ball.version, 1, true),
        Err(ObjectiveMutationError::InactivePlayer)
    );
    actor.attached = false;
    assert_eq!(
        pickup_powerball(&mut registry, actor, ball.identity, ball.version, 1, false),
        Err(ObjectiveMutationError::BlockedLineOfSight)
    );
    assert!(pickup_powerball(&mut registry, actor, ball.identity, ball.version, 1, true).is_ok());
}

#[test]
fn flag_claim_delayed_win_reward_reset_and_enemy_kill_transfer_are_profile_driven() {
    let mut policy = crate::objectives::synthetic_flag_policy();
    policy.win_delay_ticks = 2;
    policy.reward_split = FlagRewardSplit::SplitAmongTeam;
    let profile = complete_profile(
        2,
        0,
        policy,
        crate::objectives::synthetic_powerball_policy(),
        0,
        ObjectiveTeamLimits::default(),
    );
    let mut registry = ObjectiveRegistry::new(profile);
    let flags = initialize_objectives(
        &mut registry,
        10,
        |_, _| false,
        |_, _| FlagDropTile::available(),
    )
    .unwrap()
    .flags;
    let mut carrier = player(1, 10, 1, 5, 24, 24);
    for flag in flags {
        if let FlagState::OnMap { x, y, .. } = flag.state {
            carrier.position_x = x;
            carrier.position_y = y;
        }
        claim_flag(&mut registry, carrier, flag.identity, flag.version, 10).unwrap();
    }
    let participants = [ObjectiveParticipant {
        player_id: 1,
        frequency: 5,
        safe: false,
    }];
    assert!(
        evaluate_flag_win(&mut registry, &participants, 10)
            .unwrap()
            .flags
            .is_empty()
    );
    assert!(
        evaluate_flag_win(&mut registry, &participants, 11)
            .unwrap()
            .flags
            .is_empty()
    );
    let win = evaluate_flag_win(&mut registry, &participants, 12).unwrap();
    assert_eq!(win.winning_frequency, Some(5));
    assert_eq!(win.awards[0].points, 100);
    assert_eq!(registry.flag_score(5), 100);
    assert!(win.flags.iter().all(|flag| matches!(
        flag.state,
        FlagState::Waiting {
            respawn_at_tick: Some(14)
        }
    )));

    step_objective_deadlines(
        &mut registry,
        &[],
        14,
        |_, _| false,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    let respawned = registry.replay().flags[0];
    let mut victim = carrier;
    if let FlagState::OnMap { x, y, .. } = respawned.state {
        victim.position_x = x;
        victim.position_y = y;
    }
    claim_flag(
        &mut registry,
        victim,
        respawned.identity,
        respawned.version,
        14,
    )
    .unwrap();
    let killer = player(2, 20, 1, 7, 24, 24);
    apply_lifecycle_without_goal(
        &mut registry,
        victim,
        ObjectiveLifecycleReason::Death {
            killer: Some(killer),
        },
        15,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    assert!(matches!(
        registry.flag(respawned.identity.id).unwrap().state,
        FlagState::Carried { carrier, owner: Some(7), .. } if carrier == killer.runtime
    ));
}

#[test]
fn flag_team_limits_zero_cumulative_reward_below_each_threshold() {
    fn assert_unrewarded(limits: ObjectiveTeamLimits, participants: &[ObjectiveParticipant]) {
        let mut policy = crate::objectives::synthetic_flag_policy();
        policy.win_delay_ticks = 0;
        let mut registry = ObjectiveRegistry::new(complete_profile(
            1,
            0,
            policy,
            crate::objectives::synthetic_powerball_policy(),
            0,
            limits,
        ));
        registry
            .create_flag(FlagState::Carried {
                carrier: player(1, 10, 1, 5, 24, 24).runtime,
                x: 24,
                y: 24,
                owner: Some(5),
            })
            .unwrap();

        let win = evaluate_flag_win(&mut registry, participants, 1).unwrap();

        assert_eq!(win.winning_frequency, Some(5));
        assert_eq!(registry.flag_score(5), 0);
        assert!(win.awards.iter().all(|award| award.points == 0));
    }

    let alpha = ObjectiveParticipant {
        player_id: 1,
        frequency: 5,
        safe: false,
    };
    let same_team_alias = ObjectiveParticipant {
        player_id: 2,
        frequency: 9,
        safe: false,
    };
    let teammate = ObjectiveParticipant {
        player_id: 3,
        frequency: 5,
        safe: false,
    };
    assert_unrewarded(
        ObjectiveTeamLimits {
            min_players: 2,
            min_teams: 0,
            max_players_per_team: 0,
        },
        &[alpha],
    );
    assert_unrewarded(
        ObjectiveTeamLimits {
            min_players: 0,
            min_teams: 2,
            max_players_per_team: 0,
        },
        &[alpha, same_team_alias],
    );
    assert_unrewarded(
        ObjectiveTeamLimits {
            min_players: 0,
            min_teams: 0,
            max_players_per_team: 1,
        },
        &[alpha, teammate],
    );
}

#[test]
fn owned_drop_own_all_dropped_and_team_split_are_explicit_policy_modes() {
    let mut policy = crate::objectives::synthetic_flag_policy();
    policy.requested_drop = FlagDropBehavior::OwnedAtCarrier;
    policy.win_mode = FlagWinMode::OwnAllDropped;
    policy.win_delay_ticks = 0;
    policy.reward_split = FlagRewardSplit::FullPerPlayer;
    let profile = complete_profile(
        1,
        0,
        policy,
        crate::objectives::synthetic_powerball_policy(),
        0,
        ObjectiveTeamLimits::default(),
    );
    let mut registry = ObjectiveRegistry::new(profile);
    let flag = registry
        .create_flag(FlagState::OnMap {
            x: 10,
            y: 10,
            owner: None,
        })
        .unwrap();
    let carrier = player(3, 3, 1, 8, 10, 10);
    claim_flag(&mut registry, carrier, flag.identity, flag.version, 1).unwrap();
    apply_lifecycle_without_goal(
        &mut registry,
        carrier,
        ObjectiveLifecycleReason::Requested,
        2,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    assert!(matches!(
        registry.flag(flag.identity.id).unwrap().state,
        FlagState::OnMap { owner: Some(8), .. }
    ));
    let win = evaluate_flag_win(
        &mut registry,
        &[ObjectiveParticipant {
            player_id: 3,
            frequency: 8,
            safe: false,
        }],
        2,
    )
    .unwrap();
    assert_eq!(win.winning_frequency, Some(8));

    let mut split = crate::objectives::synthetic_flag_policy();
    split.reward_split = FlagRewardSplit::SplitAmongTeam;
    assert_eq!(split.reward_split, FlagRewardSplit::SplitAmongTeam);

    let mut centered_policy = crate::objectives::synthetic_flag_policy();
    centered_policy.requested_drop = FlagDropBehavior::OwnedAtCenter;
    let mut centered = ObjectiveRegistry::new(complete_profile(
        1,
        0,
        centered_policy,
        crate::objectives::synthetic_powerball_policy(),
        0,
        ObjectiveTeamLimits::default(),
    ));
    let flag = centered
        .create_flag(FlagState::OnMap {
            x: 10,
            y: 10,
            owner: None,
        })
        .unwrap();
    let carrier = player(4, 4, 1, 9, 10, 10);
    claim_flag(&mut centered, carrier, flag.identity, flag.version, 1).unwrap();
    let mut remote_carrier = carrier;
    remote_carrier.position_x = 100;
    remote_carrier.position_y = 100;
    apply_lifecycle_without_goal(
        &mut centered,
        remote_carrier,
        ObjectiveLifecycleReason::Requested,
        2,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    assert!(matches!(
        centered.flag(flag.identity.id).unwrap().state,
        FlagState::OnMap {
            x: 24,
            y: 24,
            owner: Some(9)
        }
    ));
}

#[test]
fn powerball_pickup_throw_goal_min_team_reward_and_new_game_delay_are_authoritative() {
    let limits = ObjectiveTeamLimits {
        min_players: 2,
        min_teams: 2,
        max_players_per_team: 2,
    };
    let profile = complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        crate::objectives::synthetic_powerball_policy(),
        1,
        limits,
    );
    let mut registry = ObjectiveRegistry::new(profile);
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 10,
            y: 10,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let mut scorer = player(1, 1, 1, 4, 10, 10);
    scorer.direction = 3;
    scorer.exact_orientation =
        crate::continuum_position::protocol_direction_to_orientation(scorer.direction);
    let carried =
        pickup_powerball(&mut registry, scorer, ball.identity, ball.version, 20, true).unwrap();
    let fired = fire_powerball(
        &mut registry,
        scorer,
        carried.identity,
        carried.version,
        21,
        false,
    )
    .unwrap();
    assert!(matches!(
        fired.state,
        PowerballState::OnMap {
            pickup_unlock_tick: Some(26),
            ..
        }
    ));
    let motion = registry.powerball_motion(fired.identity.id).unwrap();
    assert_eq!((motion.velocity_x, motion.velocity_y), (122, -167));
    assert_eq!(motion.last_touch, Some(scorer.runtime));
    assert_eq!(motion.last_touch_frequency, Some(4));

    let participants = [
        ObjectiveParticipant {
            player_id: 1,
            frequency: 4,
            safe: false,
        },
        ObjectiveParticipant {
            player_id: 2,
            frequency: 5,
            safe: false,
        },
    ];
    let goal = score_powerball_goal(
        &mut registry,
        fired.identity,
        fired.version,
        4,
        9_000,
        10,
        &participants,
        30,
        |_, _| false,
    )
    .unwrap();
    assert_eq!(goal.winning_frequency, Some(4));
    assert_eq!(
        goal.awards,
        vec![ObjectiveAward {
            player_id: 1,
            points: 200
        }]
    );
    assert_eq!(goal.score_events, vec![(4, 100)]);
    assert_eq!(
        goal.powerball_scores,
        vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
            (5, 0),
            (6, 0),
            (7, 0),
        ]
    );
    assert!(goal.powerball_score_version.is_some());
    assert!(matches!(
        goal.powerballs[0].state,
        PowerballState::Waiting {
            respawn_at_tick: Some(31)
        }
    ));
    assert_eq!(
        registry.powerball_score(4),
        0,
        "new game clears team scores"
    );
    let respawn = step_objective_deadlines(
        &mut registry,
        &[],
        31,
        |_, _| false,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    assert!(matches!(
        respawn.powerballs[0].state,
        PowerballState::OnMap { .. }
    ));

    let mut below_limits = ObjectiveRegistry::new(complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        crate::objectives::synthetic_powerball_policy(),
        1,
        limits,
    ));
    let ball = below_limits
        .create_powerball(PowerballState::OnMap {
            x: 10,
            y: 10,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let goal = score_powerball_goal(
        &mut below_limits,
        ball.identity,
        ball.version,
        4,
        9_000,
        10,
        &participants[..1],
        30,
        |_, _| false,
    )
    .unwrap();
    assert_eq!(goal.awards[0].points, 0);
}

#[test]
fn goal_zero_respawns_atomically_and_negative_new_game_delay_draws_deterministically() {
    let mut zero_profile = complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        crate::objectives::synthetic_powerball_policy(),
        2,
        ObjectiveTeamLimits::default(),
    );
    let ServerPowerballProfile::Enabled(zero) = &mut zero_profile.powerballs else {
        unreachable!()
    };
    zero.goal_delay_ticks = 0;
    let mut zero_registry = ObjectiveRegistry::new(zero_profile);
    let ball = zero_registry
        .create_powerball(PowerballState::OnMap {
            x: 9_000,
            y: 10,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let goal = score_powerball_goal(
        &mut zero_registry,
        ball.identity,
        ball.version,
        4,
        9_000,
        10,
        &[],
        100,
        |_, _| false,
    )
    .unwrap();
    assert!(matches!(
        goal.powerballs[0].state,
        PowerballState::OnMap { .. }
    ));
    assert!(zero_registry.powerball_motion(ball.identity.id).is_some());

    let mut random_policy = crate::objectives::synthetic_powerball_policy();
    random_policy.new_game_delay_ticks = -9;
    let mut random_registry = ObjectiveRegistry::new(complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        random_policy,
        1,
        ObjectiveTeamLimits::default(),
    ));
    random_registry.set_rng_seed(12_345);
    let ball = random_registry
        .create_powerball(PowerballState::OnMap {
            x: 9_000,
            y: 10,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let mut expected_seed = 12_345;
    let expected_delay = crate::vie::vie_rng_next(&mut expected_seed) as u32 % 10;
    let goal = score_powerball_goal(
        &mut random_registry,
        ball.identity,
        ball.version,
        4,
        9_000,
        10,
        &[],
        200,
        |_, _| false,
    )
    .unwrap();
    assert!(matches!(
        goal.powerballs[0].state,
        PowerballState::Waiting {
            respawn_at_tick: Some(deadline)
        } if deadline == crate::continuum::continuum_tick_add(200, expected_delay)
    ));
}

#[test]
fn soccer_scores_keep_exact_eight_frequency_buckets_and_reject_higher_frequencies() {
    let profile = complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        crate::objectives::synthetic_powerball_policy(),
        2,
        ObjectiveTeamLimits::default(),
    );
    let mut registry = ObjectiveRegistry::new(profile);
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 100,
            y: 10,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let goal = score_powerball_goal(
        &mut registry,
        ball.identity,
        ball.version,
        5,
        100,
        10,
        &[],
        30,
        |_, _| false,
    )
    .unwrap();
    assert_eq!(registry.powerball_score(5), 1);
    assert_eq!(registry.powerball_score(1), 0);
    assert_eq!(goal.powerball_scores.len(), 8);
    assert_eq!(goal.powerball_scores[5], (5, 1));

    let current = registry.powerball(ball.identity.id).unwrap();
    assert_eq!(
        score_powerball_goal(
            &mut registry,
            current.identity,
            current.version,
            8,
            100,
            10,
            &[],
            31,
            |_, _| false,
        ),
        Err(ObjectiveMutationError::UnsupportedFrequency)
    );

    let mut pickup_registry = ObjectiveRegistry::new(synthetic_profile(0, 1));
    let pickup_ball = pickup_registry
        .create_powerball(PowerballState::OnMap {
            x: 24,
            y: 24,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    assert_eq!(
        pickup_powerball(
            &mut pickup_registry,
            player(8, 8, 1, 8, 24, 24),
            pickup_ball.identity,
            pickup_ball.version,
            31,
            true,
        ),
        Err(ObjectiveMutationError::UnsupportedFrequency)
    );
}

#[test]
fn four_team_steal_game_over_requires_exactly_three_zero_buckets() {
    let mut policy = crate::objectives::synthetic_powerball_policy();
    policy.soccer_mode = 3;
    policy.capture_points = 1;
    let profile = complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        policy,
        0,
        ObjectiveTeamLimits::default(),
    );
    let mut registry = ObjectiveRegistry::new(profile);
    registry.set_powerball_score(1, 0);
    registry.set_powerball_score(2, 0);
    registry.set_powerball_score(3, 0);
    let ServerPowerballProfile::Enabled(profile) = registry.profile().powerballs.clone() else {
        unreachable!()
    };

    assert_eq!(
        crate::objectives::powerball_game_winner(&registry, &profile),
        Some(0)
    );
    assert_eq!(
        (4..8)
            .map(|frequency| registry.powerball_score(frequency))
            .collect::<Vec<_>>(),
        vec![1, 1, 1, 1],
        "the four higher exact score buckets are not opponents in the four-team zero count"
    );
    registry.set_powerball_score(4, 0);
    assert_eq!(
        crate::objectives::powerball_game_winner(&registry, &profile),
        None,
        "four zero buckets must not satisfy the primary exactly-three rule"
    );
}

#[test]
fn four_team_steal_rewards_the_highest_team_when_another_team_scores_the_final_goal() {
    let mut policy = crate::objectives::synthetic_powerball_policy();
    policy.soccer_mode = 3;
    policy.capture_points = 1;
    let mut registry = ObjectiveRegistry::new(complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        policy,
        0,
        ObjectiveTeamLimits::default(),
    ));
    for (frequency, score) in [(0, 5), (1, 0), (2, 1), (3, 1), (5, 0)] {
        registry.set_powerball_score(frequency, score);
    }
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 100,
            y: 9_000,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let participants = [
        ObjectiveParticipant {
            player_id: 10,
            frequency: 0,
            safe: false,
        },
        ObjectiveParticipant {
            player_id: 13,
            frequency: 3,
            safe: false,
        },
    ];

    let goal = score_powerball_goal(
        &mut registry,
        ball.identity,
        ball.version,
        3,
        100,
        9_000,
        &participants,
        50,
        |_, _| false,
    )
    .unwrap();

    assert_eq!(goal.winning_frequency, Some(0));
    assert_eq!(goal.score_events, vec![(3, 100)]);
    assert_eq!(
        goal.awards,
        vec![
            ObjectiveAward {
                player_id: 13,
                points: 100,
            },
            ObjectiveAward {
                player_id: 10,
                points: 100,
            },
        ],
        "the scorer keeps the goal reward while the highest team gets the game reward"
    );
    assert!(
        registry
            .powerball_score_snapshot()
            .iter()
            .all(|(_, score)| *score == 1),
        "game-over resets every exact score bucket"
    );
}

#[test]
fn absolute_game_over_counts_all_eight_buckets_and_zero_win_by_stays_inert() {
    let profile_for = |win_by| {
        let mut policy = crate::objectives::synthetic_powerball_policy();
        policy.capture_points = -1;
        complete_profile(
            0,
            1,
            crate::objectives::synthetic_flag_policy(),
            policy,
            win_by,
            ObjectiveTeamLimits::default(),
        )
    };
    let participant = ObjectiveParticipant {
        player_id: 10,
        frequency: 0,
        safe: false,
    };

    let mut zero = ObjectiveRegistry::new(profile_for(0));
    let ball = zero
        .create_powerball(PowerballState::OnMap {
            x: 9_000,
            y: 100,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let goal = score_powerball_goal(
        &mut zero,
        ball.identity,
        ball.version,
        0,
        9_000,
        100,
        &[participant],
        100,
        |_, _| false,
    )
    .unwrap();
    assert_eq!(goal.winning_frequency, None);
    assert_eq!(
        goal.awards,
        vec![ObjectiveAward {
            player_id: 10,
            points: 100
        }]
    );
    assert_eq!(
        zero.powerball_score(0),
        1,
        "zero WinBy must not reset scores"
    );
    assert!(matches!(
        zero.powerball(ball.identity.id).unwrap().state,
        PowerballState::Waiting {
            respawn_at_tick: Some(102)
        }
    ));

    let mut positive = ObjectiveRegistry::new(profile_for(1));
    let ball = positive
        .create_powerball(PowerballState::OnMap {
            x: 9_000,
            y: 100,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let goal = score_powerball_goal(
        &mut positive,
        ball.identity,
        ball.version,
        0,
        9_000,
        100,
        &[participant],
        100,
        |_, _| false,
    )
    .unwrap();
    assert_eq!(goal.winning_frequency, Some(0));
    assert_eq!(
        goal.awards,
        vec![ObjectiveAward {
            player_id: 10,
            points: 200
        }]
    );
    assert_eq!(positive.powerball_score(0), 0);
    assert!(matches!(
        positive.powerball(ball.identity.id).unwrap().state,
        PowerballState::Waiting {
            respawn_at_tick: Some(101)
        }
    ));
}

#[test]
fn pickup_version_exhaustion_preserves_the_authoritative_motion() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(0, 1));
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 24,
            y: 24,
            velocity_x: 10,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let motion = PowerballMotion::from_wire_position(24, 24, 10, 0, 0, 1);
    registry
        .set_powerball_motion(ball.identity, motion)
        .unwrap();
    assert!(
        registry.force_powerball_version(ball.identity.id, crate::continuum::CONTINUUM_TICK_MASK)
    );
    let exhausted = registry.powerball(ball.identity.id).unwrap();

    assert_eq!(
        pickup_powerball(
            &mut registry,
            player(2, 2, 1, 1, 24, 24),
            exhausted.identity,
            exhausted.version,
            2,
            true,
        ),
        Err(ObjectiveMutationError::Registry(
            ObjectiveRegistryError::VersionExhausted
        ))
    );
    assert_eq!(registry.powerball_motion(ball.identity.id), Some(motion));
    assert_eq!(registry.powerball(ball.identity.id), Some(exhausted));
}

#[test]
fn zero_carry_timeouts_are_disabled_while_positive_timeouts_expire_across_wrap() {
    let actor = player(3, 3, 1, 1, 24, 24);
    let start = crate::continuum::CONTINUUM_TICK_MASK - 5;
    let mut zero_profile = complete_profile(
        1,
        1,
        crate::objectives::synthetic_flag_policy(),
        crate::objectives::synthetic_powerball_policy(),
        1,
        ObjectiveTeamLimits::default(),
    );
    let ServerFlagProfile::Enabled(flags) = &mut zero_profile.flags else {
        unreachable!()
    };
    flags.carry_timeout_ticks = 0;
    let ServerPowerballProfile::Enabled(powerballs) = &mut zero_profile.powerballs else {
        unreachable!()
    };
    powerballs.policy.carry_timeout_ticks = [0; 8];
    let mut zero_registry = ObjectiveRegistry::new(zero_profile);
    let zero_flag = zero_registry
        .create_flag(FlagState::Carried {
            carrier: actor.runtime,
            x: 24,
            y: 24,
            owner: Some(actor.frequency),
        })
        .unwrap();
    zero_registry.set_flag_carried_since(zero_flag.identity, start);
    let zero_ball = zero_registry
        .create_powerball(PowerballState::Carried {
            carrier: actor.runtime,
        })
        .unwrap();
    zero_registry.set_powerball_carried_since(zero_ball.identity, start);
    let mutation = step_objective_deadlines(
        &mut zero_registry,
        &[actor],
        5,
        |_, _| false,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    assert!(mutation.flags.is_empty() && mutation.powerballs.is_empty());
    assert!(matches!(
        zero_registry.flag(zero_flag.identity.id).unwrap().state,
        FlagState::Carried { .. }
    ));
    assert!(matches!(
        zero_registry
            .powerball(zero_ball.identity.id)
            .unwrap()
            .state,
        PowerballState::Carried { .. }
    ));

    let mut positive_policy = crate::objectives::synthetic_powerball_policy();
    positive_policy.carry_timeout_ticks = [10; 8];
    let mut positive_registry = ObjectiveRegistry::new(complete_profile(
        1,
        1,
        crate::objectives::synthetic_flag_policy(),
        positive_policy,
        0,
        ObjectiveTeamLimits::default(),
    ));
    let flag = positive_registry
        .create_flag(FlagState::Carried {
            carrier: actor.runtime,
            x: 24,
            y: 24,
            owner: Some(actor.frequency),
        })
        .unwrap();
    positive_registry.set_flag_carried_since(flag.identity, start);
    let ball = positive_registry
        .create_powerball(PowerballState::Carried {
            carrier: actor.runtime,
        })
        .unwrap();
    positive_registry.set_powerball_carried_since(ball.identity, start);
    let mutation = step_objective_deadlines(
        &mut positive_registry,
        &[actor],
        5,
        |_, _| false,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    assert_eq!(mutation.flags.len(), 1);
    assert_eq!(mutation.powerballs.len(), 1);
}

#[test]
fn timed_powerball_release_uses_canonical_carried_ball_geometry() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(0, 1));
    let mut actor = player(1, 1, 1, 4, 1_000, 2_000);
    actor.direction = 10;
    actor.exact_orientation =
        crate::continuum_position::protocol_direction_to_orientation(actor.direction);
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 1_000,
            y: 2_000,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let carried =
        pickup_powerball(&mut registry, actor, ball.identity, ball.version, 1, true).unwrap();

    let released = fire_powerball(
        &mut registry,
        actor,
        carried.identity,
        carried.version,
        2,
        true,
    )
    .unwrap();

    let orientation = crate::continuum_position::protocol_direction_to_orientation(actor.direction);
    let (heading_x, heading_y) =
        crate::continuum_position::canonical_discrete_heading_components(orientation);
    let extension = (arena_ship_radius_pixels(actor.ship) - 4.0).max(0.0);
    let expected_x = (f32::from(actor.position_x) + heading_x * extension) as u16;
    let expected_y = (f32::from(actor.position_y) + heading_y * extension) as u16;
    assert!(matches!(
        released.state,
        PowerballState::OnMap { x, y, .. } if (x, y) == (expected_x, expected_y)
    ));
    assert_ne!(
        (expected_x, expected_y),
        (actor.position_x, actor.position_y)
    );
}

#[test]
fn every_lifecycle_reason_applies_configured_flag_and_ball_cleanup() {
    for reason in [
        ObjectiveLifecycleReason::ShipOrFrequency,
        ObjectiveLifecycleReason::Disconnect,
        ObjectiveLifecycleReason::SessionOrArena,
    ] {
        let profile = synthetic_profile(1, 1);
        let mut registry = ObjectiveRegistry::new(profile);
        let actor = player(1, 1, 1, 3, 24, 24);
        let flag = registry
            .create_flag(FlagState::OnMap {
                x: 24,
                y: 24,
                owner: None,
            })
            .unwrap();
        let flag = claim_flag(&mut registry, actor, flag.identity, flag.version, 1).unwrap();
        let ball = registry
            .create_powerball(PowerballState::OnMap {
                x: 24,
                y: 24,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: None,
            })
            .unwrap();
        pickup_powerball(&mut registry, actor, ball.identity, ball.version, 1, true).unwrap();

        let cleanup = apply_lifecycle_without_goal(&mut registry, actor, reason, 2, |_, _| {
            FlagDropTile::available()
        })
        .unwrap();
        assert_eq!(cleanup.flags.len(), 1);
        assert_eq!(cleanup.powerballs.len(), 1);
        assert!(matches!(
            registry.flag(flag.identity.id).unwrap().state,
            FlagState::OnMap {
                x: 24,
                y: 24,
                owner: None
            }
        ));
        assert!(matches!(
            registry.powerball(ball.identity.id).unwrap().state,
            PowerballState::OnMap {
                x: 24,
                y: 24,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: Some(7),
            }
        ));
    }
}

#[test]
fn joined_dead_players_count_for_objectives_and_keep_last_safe_reward_status() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx_a, _rx_a) = std::sync::mpsc::channel();
    let (tx_b, _rx_b) = std::sync::mpsc::channel();
    let (tx_c, _rx_c) = std::sync::mpsc::channel();
    let alpha = register_connection(&shared, tx_a);
    let beta = register_connection(&shared, tx_b);
    let spectator = register_connection(&shared, tx_c);
    join_arena(&shared, alpha, 0);
    join_arena(&shared, beta, 0);
    join_arena(&shared, spectator, 8);
    {
        let mut state = shared.lock().unwrap();
        state.clients.get_mut(&alpha).unwrap().player.frequency = 4;
        let beta_client = state.clients.get_mut(&beta).unwrap();
        beta_client.player.frequency = 4;
        beta_client.alive = false;
        beta_client.objective_last_safe = true;
        state.clear_client_position(beta);
    }

    let state = shared.lock().unwrap();
    let participants = crate::objectives::objective_participants(&state);
    assert_eq!(participants.len(), 2);
    assert!(
        participants
            .iter()
            .any(|participant| { participant.player_id == beta && participant.safe })
    );
    assert!(
        !participants
            .iter()
            .any(|participant| participant.player_id == spectator)
    );

    let mut registry = ObjectiveRegistry::new(complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        crate::objectives::synthetic_powerball_policy(),
        2,
        ObjectiveTeamLimits {
            min_players: 2,
            min_teams: 1,
            max_players_per_team: 0,
        },
    ));
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 9_000,
            y: 10,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let goal = score_powerball_goal(
        &mut registry,
        ball.identity,
        ball.version,
        4,
        9_000,
        10,
        &participants,
        20,
        |_, _| false,
    )
    .unwrap();
    assert_eq!(goal.score_events, vec![(4, 100)]);
    assert_eq!(
        goal.awards,
        vec![ObjectiveAward {
            player_id: alpha,
            points: 100,
        }],
        "the dead safe player counts toward MinPlayers but receives no reward"
    );
}

#[test]
fn objective_safe_status_survives_same_arena_death_but_not_leave_rejoin() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, _rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    let prior_stream = {
        let mut state = shared.lock().unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.objective_last_safe = true;
        let stream = client.arena_stream_id;
        state.clear_client_position(player_id);
        assert!(state.clients[&player_id].objective_last_safe);
        stream
    };

    leave_arena(&shared, player_id);
    assert!(!shared.lock().unwrap().clients[&player_id].objective_last_safe);

    join_arena(&shared, player_id, 0);
    let state = shared.lock().unwrap();
    assert_ne!(state.clients[&player_id].arena_stream_id, prior_stream);
    assert!(!state.clients[&player_id].objective_last_safe);
}

#[test]
fn stale_reused_session_disconnect_is_exactly_inert_before_objective_lifecycle() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (old_tx, _old_rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, old_tx);
    join_arena(&shared, player_id, 0);
    let old_session = shared
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();

    let (observer_tx, observer_rx) = std::sync::mpsc::channel();
    let observer_id = register_connection(&shared, observer_tx);
    join_arena(&shared, observer_id, 0);
    drain_available(&observer_rx);

    crate::arena::disconnect_player_session(&shared, old_session);
    drain_available(&observer_rx);
    {
        let mut state = shared.lock().unwrap();
        state.next_player_id = player_id;
    }
    let (replacement_tx, replacement_rx) = std::sync::mpsc::channel();
    assert_eq!(register_connection(&shared, replacement_tx), player_id);
    join_arena(&shared, player_id, 0);
    let replacement_session = shared
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();
    assert_ne!(old_session.session_id, replacement_session.session_id);
    drain_available(&replacement_rx);
    drain_available(&observer_rx);

    let (before_registry, before_player, before_identity_stats) = {
        let mut state = shared.lock().unwrap();
        state.objectives.reset(synthetic_profile(1, 1)).unwrap();
        let replacement = state.active_player_runtime_identity(player_id).unwrap();
        state
            .objectives
            .create_flag(FlagState::Carried {
                carrier: replacement,
                x: 320,
                y: 336,
                owner: Some(0),
            })
            .unwrap();
        state
            .objectives
            .create_powerball(PowerballState::Carried {
                carrier: replacement,
            })
            .unwrap();
        (
            state.objectives.clone(),
            state.clients[&player_id].player.clone(),
            state.identity_stats.clone(),
        )
    };

    let mut stale_after_removal_ran = false;
    crate::arena::disconnect_player_session_after_removal(&shared, old_session, || {
        stale_after_removal_ran = true;
    });
    assert!(
        !stale_after_removal_ran,
        "stale cleanup must not cross the authenticated removal boundary"
    );
    crate::arena::disconnect_player_session(&shared, old_session);
    {
        let state = shared.lock().unwrap();
        assert!(state.player_session_matches(replacement_session));
        assert_eq!(state.objectives, before_registry);
        assert_eq!(state.clients[&player_id].player, before_player);
        assert_eq!(state.identity_stats, before_identity_stats);
    }
    assert!(
        replacement_rx.try_iter().next().is_none(),
        "stale callback must emit no replacement-private bytes"
    );
    assert!(
        observer_rx.try_iter().next().is_none(),
        "stale callback must emit no replay, mutation, reward, or leave bytes"
    );

    crate::arena::disconnect_player_session(&shared, replacement_session);
    assert!(!shared.lock().unwrap().clients.contains_key(&player_id));
    assert!(
        observer_rx.try_iter().next().is_some(),
        "the matching current session performs lifecycle and leave fanout"
    );
    drain_available(&observer_rx);
    crate::arena::disconnect_player_session(&shared, replacement_session);
    assert!(
        observer_rx.try_iter().next().is_none(),
        "a retry after the current session retires is inert"
    );
}

#[test]
fn lifecycle_owned_drop_establishes_own_all_win_in_same_transaction() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, _rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    let mut policy = crate::objectives::synthetic_flag_policy();
    policy.win_mode = FlagWinMode::OwnAllDropped;
    policy.win_delay_ticks = 0;
    policy.ship_or_frequency_drop = FlagDropBehavior::OwnedAtCarrier;
    let mut state = shared.lock().unwrap();
    state
        .objectives
        .reset(complete_profile(
            1,
            0,
            policy,
            crate::objectives::synthetic_powerball_policy(),
            0,
            ObjectiveTeamLimits::default(),
        ))
        .unwrap();
    state.clients.get_mut(&player_id).unwrap().last_position =
        Some(client_position_packet_with_sequence(0, 320, 320, 41));
    let context = crate::objectives::objective_player(&state, player_id).unwrap();
    state
        .objectives
        .create_flag(FlagState::Carried {
            carrier: context.runtime,
            x: 320,
            y: 320,
            owner: None,
        })
        .unwrap();

    let packets = crate::objectives::player_lifecycle_packets_for_context(
        &mut state,
        context,
        None,
        ObjectiveLifecycleReason::ShipOrFrequency,
        10,
    )
    .unwrap();

    assert!(matches!(
        state.objectives.replay().flags[0].state,
        FlagState::Waiting { .. }
    ));
    assert!(packets.iter().any(|packet| packet.first() == Some(&0x14)));
}

#[test]
fn authenticated_flag_drop_establishes_win_without_releasing_a_carried_ball() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    drain_available(&rx);
    let (request, ball_identity, ball_version) = {
        let mut state = shared.lock().unwrap();
        let mut policy = crate::objectives::synthetic_flag_policy();
        policy.win_mode = FlagWinMode::OwnAllDropped;
        policy.win_delay_ticks = 0;
        policy.requested_drop = FlagDropBehavior::OwnedAtCarrier;
        state
            .objectives
            .reset(complete_profile(
                1,
                1,
                policy,
                crate::objectives::synthetic_powerball_policy(),
                0,
                ObjectiveTeamLimits::default(),
            ))
            .unwrap();
        state.clients.get_mut(&player_id).unwrap().last_position =
            Some(client_position_packet_with_sequence(0, 320, 320, 41));
        let context = crate::objectives::objective_player(&state, player_id).unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::Carried {
                carrier: context.runtime,
                x: 320,
                y: 320,
                owner: None,
            })
            .unwrap();
        let ball = state
            .objectives
            .create_powerball(PowerballState::Carried {
                carrier: context.runtime,
            })
            .unwrap();
        let client = state.clients.get_mut(&player_id).unwrap();
        client.last_position_sequence = Some(41);
        client
            .canonical_position_witnesses
            .push_back(position_witness(
                41,
                context.runtime.life.life_seq,
                320,
                320,
            ));
        let request = crate::objective_protocol::ObjectiveActionRequest {
            kind: crate::objective_protocol::ObjectiveActionKind::FlagDrop,
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: context.arena_stream_id,
            player_id,
            life_seq: context.runtime.life.life_seq,
            action_id: 1,
            position_sequence: 41,
            objective_id: flag.identity.id,
            objective_generation: flag.identity.generation,
            expected_version: flag.version.get(),
        };
        (request, ball.identity, ball.version)
    };

    crate::objectives::relay_objective_action(&shared, player_id, &request.encode());
    crate::objectives::process_pending_objective_actions(&shared, 10);

    let state = shared.lock().unwrap();
    assert!(matches!(
        state.objectives.flag(request.objective_id).unwrap().state,
        FlagState::Waiting { .. }
    ));
    let ball = state.objectives.powerball(ball_identity.id).unwrap();
    assert_eq!(ball.version, ball_version);
    assert!(matches!(
        ball.state,
        PowerballState::Carried { carrier } if carrier.player_id() == player_id
    ));
}

#[test]
fn authenticated_flag_drop_ignores_temporary_brick_and_preserves_assignment_rng() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    drain_available(&rx);
    let (request, drop_tile) = {
        let mut state = shared.lock().unwrap();
        let map = state.zone_map();
        let drop_tile = map
            .tiles
            .iter()
            .enumerate()
            .find_map(|(index, tile)| {
                let x = (index % crate::zone::MAP_TILES) as u16;
                let y = (index / crate::zone::MAP_TILES) as u16;
                (*tile == 0 && !map.no_flag_drops_at(x, y) && x > 1 && y > 1).then_some((x, y))
            })
            .unwrap();
        state.objectives.reset(synthetic_profile(1, 0)).unwrap();
        let world = (drop_tile.0 * 16 + 8, drop_tile.1 * 16 + 8);
        state.clients.get_mut(&player_id).unwrap().last_position = Some(
            client_position_packet_with_sequence(0, world.0, world.1, 41),
        );
        let context = crate::objectives::objective_player(&state, player_id).unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::Carried {
                carrier: context.runtime,
                x: world.0,
                y: world.1,
                owner: None,
            })
            .unwrap();
        state.objectives.set_rng_seed(29);
        state.active_bricks.push(ServerBrick {
            tiles: vec![drop_tile],
            packet: vec![BRICK_DROPPED, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            expires_at_tick: crate::continuum::continuum_tick_add(10, 1_000),
        });
        let client = state.clients.get_mut(&player_id).unwrap();
        client.last_position_sequence = Some(41);
        client
            .canonical_position_witnesses
            .push_back(position_witness(
                41,
                context.runtime.life.life_seq,
                world.0,
                world.1,
            ));
        (
            crate::objective_protocol::ObjectiveActionRequest {
                kind: crate::objective_protocol::ObjectiveActionKind::FlagDrop,
                server_session_id: crate::wire::server_session_id(),
                arena_stream_id: context.arena_stream_id,
                player_id,
                life_seq: context.runtime.life.life_seq,
                action_id: 1,
                position_sequence: 41,
                objective_id: flag.identity.id,
                objective_generation: flag.identity.generation,
                expected_version: flag.version.get(),
            },
            drop_tile,
        )
    };

    crate::objectives::relay_objective_action(&shared, player_id, &request.encode());
    crate::objectives::process_pending_objective_actions(&shared, 10);

    let mut state = shared.lock().unwrap();
    assert!(matches!(
        state.objectives.flag(request.objective_id).unwrap().state,
        FlagState::OnMap { x, y, .. }
            if (x, y) == (drop_tile.0 * 16 + 8, drop_tile.1 * 16 + 8)
    ));
    assert_eq!(*state.objectives.rng_seed_mut(), 29);
    assert_eq!(state.active_bricks[0].tiles, vec![drop_tile]);
}

#[test]
fn lifecycle_retires_fired_powerball_touch_lineage() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(0, 1));
    let actor = player(1, 1, 1, 3, 24, 24);
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 24,
            y: 24,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let carried =
        pickup_powerball(&mut registry, actor, ball.identity, ball.version, 1, true).unwrap();
    fire_powerball(
        &mut registry,
        actor,
        carried.identity,
        carried.version,
        2,
        false,
    )
    .unwrap();

    let current = registry.powerball(ball.identity.id).unwrap();
    let cleanup = apply_lifecycle_without_goal(
        &mut registry,
        actor,
        ObjectiveLifecycleReason::ShipOrFrequency,
        3,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();

    assert_eq!(cleanup.powerballs.len(), 1);
    assert_eq!(cleanup.powerballs[0].version, current.version);
    let motion = registry.powerball_motion(ball.identity.id).unwrap();
    assert_eq!(motion.last_touch, None);
    assert_eq!(motion.last_touch_frequency, None);
}

#[test]
fn lifecycle_metadata_cleanup_publishes_equal_version_newer_tick_wire_state() {
    let mut state = team_mode_state();
    state.objectives.reset(synthetic_profile(0, 2)).unwrap();
    let actor = player(1, 1, 1, 3, 24, 24);
    let lineage_ball = state
        .objectives
        .create_powerball(PowerballState::OnMap {
            x: 24,
            y: 24,
            velocity_x: 10,
            velocity_y: 0,
            pickup_unlock_tick: Some(40),
        })
        .unwrap();
    let mut motion = PowerballMotion::from_wire_position(24, 24, 10, 0, 0, 10);
    motion.last_touch = Some(actor.runtime);
    motion.last_touch_frequency = Some(actor.frequency);
    state
        .objectives
        .set_powerball_motion(lineage_ball.identity, motion)
        .unwrap();
    let killer_ball = state
        .objectives
        .create_powerball(PowerballState::OnMap {
            x: 40,
            y: 24,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: Some(40),
        })
        .unwrap();
    let other = runtime(2, 2, 1);
    let mut motion = PowerballMotion::from_wire_position(40, 24, 0, 0, 0, 10);
    motion.last_touch = Some(other);
    motion.last_touch_frequency = Some(4);
    state
        .objectives
        .set_powerball_motion(killer_ball.identity, motion)
        .unwrap();
    state
        .objectives
        .set_powerball_killer_pickup(killer_ball.identity, Some((actor.runtime, 35)))
        .unwrap();

    let mutation = apply_lifecycle_without_goal(
        &mut state.objectives,
        actor,
        ObjectiveLifecycleReason::ShipOrFrequency,
        20,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    let packets = mutation_packets(&mut state, &mutation, 20, 0);
    let mut updates = packets
        .iter()
        .filter_map(|packet| crate::objective_protocol::ObjectiveStateUpdate::parse(packet).ok())
        .collect::<Vec<_>>();
    updates.sort_by_key(|update| update.objective_id);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].version, lineage_ball.version.get());
    assert_eq!(updates[0].server_tick, 20);
    assert_eq!(updates[0].owner_id, None);
    assert_eq!(updates[0].owner_frequency, None);
    assert_eq!(updates[0].killer_id, None);
    assert_eq!(updates[0].killer_pickup_unlock_tick, None);
    assert_eq!(updates[0].pickup_unlock_tick, Some(40));
    assert_eq!(updates[1].version, killer_ball.version.get());
    assert_eq!(updates[1].server_tick, 20);
    assert_eq!(updates[1].owner_id, Some(other.player_id()));
    assert_eq!(updates[1].owner_frequency, Some(4));
    assert_eq!(updates[1].killer_id, None);
    assert_eq!(updates[1].killer_pickup_unlock_tick, None);
    assert_eq!(updates[1].pickup_unlock_tick, Some(40));
}

#[test]
fn death_goal_lineage_and_killer_specific_pass_deadline_follow_policy() {
    let mut policy = crate::objectives::synthetic_powerball_policy();
    policy.death_scores_goal = true;
    policy.pass_delay_ticks = 5;
    policy.killer_ignore_pass_delay_ticks = 3;
    let profile = complete_profile(
        0,
        2,
        crate::objectives::synthetic_flag_policy(),
        policy,
        0,
        ObjectiveTeamLimits::default(),
    );
    let mut registry = ObjectiveRegistry::new(profile);
    let victim = player(1, 1, 1, 3, 24, 24);
    let killer = player(2, 2, 1, 4, 24, 24);
    let other = player(3, 3, 1, 5, 24, 24);

    let fired = registry
        .create_powerball(PowerballState::OnMap {
            x: 24,
            y: 24,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let carried = pickup_powerball(
        &mut registry,
        victim,
        fired.identity,
        fired.version,
        1,
        true,
    )
    .unwrap();
    fire_powerball(
        &mut registry,
        victim,
        carried.identity,
        carried.version,
        2,
        false,
    )
    .unwrap();
    let held = registry
        .create_powerball(PowerballState::Carried {
            carrier: victim.runtime,
        })
        .unwrap();

    let mutation = apply_lifecycle_without_goal(
        &mut registry,
        victim,
        ObjectiveLifecycleReason::Death {
            killer: Some(killer),
        },
        100,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();

    let fired_motion = registry.powerball_motion(fired.identity.id).unwrap();
    assert_eq!(fired_motion.last_touch, Some(victim.runtime));
    assert_eq!(fired_motion.last_touch_frequency, Some(victim.frequency));
    assert!(matches!(
        mutation.powerballs[0].state,
        PowerballState::OnMap {
            pickup_unlock_tick: Some(105),
            ..
        }
    ));
    assert_eq!(
        registry.powerball_killer_pickup(held.identity.id),
        Some((killer.runtime, 102))
    );
    let released = registry.powerball(held.identity.id).unwrap();
    assert_eq!(
        pickup_powerball(
            &mut registry,
            other,
            released.identity,
            released.version,
            102,
            true,
        ),
        Err(ObjectiveMutationError::PickupLocked)
    );
    assert!(
        pickup_powerball(
            &mut registry,
            killer,
            released.identity,
            released.version,
            102,
            true,
        )
        .is_ok()
    );
}

#[test]
fn carried_ball_death_scores_and_resets_without_publishing_a_loose_intermediate() {
    let mut policy = crate::objectives::synthetic_powerball_policy();
    policy.death_scores_goal = true;
    policy.new_game_delay_ticks = 1;
    let mut profile = complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        policy,
        1,
        ObjectiveTeamLimits::default(),
    );
    let ServerPowerballProfile::Enabled(powerballs) = &mut profile.powerballs else {
        unreachable!()
    };
    powerballs.goal_delay_ticks = 0;
    let mut registry = ObjectiveRegistry::new(profile);
    let victim = player(1, 1, 1, 0, 9_000, 10);
    let participant = ObjectiveParticipant {
        player_id: victim.runtime.player_id(),
        frequency: victim.frequency,
        safe: false,
    };
    let carried = registry
        .create_powerball(PowerballState::Carried {
            carrier: victim.runtime,
        })
        .unwrap();

    let mutation = apply_player_objective_lifecycle(
        &mut registry,
        victim,
        ObjectiveLifecycleReason::Death { killer: None },
        100,
        &[participant],
        |_, _| FlagDropTile::available(),
        |_, _| false,
        |x, y| (x, y) == (9_000, 10),
    )
    .unwrap();

    assert_eq!(mutation.winning_frequency, Some(0));
    assert_eq!(mutation.score_events, vec![(0, 100)]);
    assert_eq!(
        mutation
            .awards
            .iter()
            .map(|award| award.points)
            .sum::<i64>(),
        200,
        "the scoring player receives separate goal and game rewards"
    );
    assert_eq!(mutation.powerballs.len(), 1);
    assert_eq!(
        mutation.powerballs[0].version.get(),
        carried.version.get() + 2
    );
    assert!(matches!(
        mutation.powerballs[0].state,
        PowerballState::Waiting {
            respawn_at_tick: Some(101)
        }
    ));
    assert!(
        mutation.powerballs.iter().all(|ball| !matches!(
            ball.state,
            PowerballState::OnMap {
                x: 9_000,
                y: 10,
                ..
            }
        )),
        "the carried-ball release is never observable before the goal result"
    );
    assert_eq!(registry.powerball_score(0), 0, "the winning game resets");

    let respawn = step_objective_deadlines(
        &mut registry,
        &[victim],
        101,
        |_, _| false,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    assert!(matches!(
        respawn.powerballs[0].state,
        PowerballState::OnMap { .. }
    ));

    let mut nonwinning_profile = complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        policy,
        2,
        ObjectiveTeamLimits::default(),
    );
    let ServerPowerballProfile::Enabled(powerballs) = &mut nonwinning_profile.powerballs else {
        unreachable!()
    };
    powerballs.goal_delay_ticks = 0;
    let mut nonwinning = ObjectiveRegistry::new(nonwinning_profile);
    let carried = nonwinning
        .create_powerball(PowerballState::Carried {
            carrier: victim.runtime,
        })
        .unwrap();
    let mutation = apply_player_objective_lifecycle(
        &mut nonwinning,
        victim,
        ObjectiveLifecycleReason::Death { killer: None },
        200,
        &[participant],
        |_, _| FlagDropTile::available(),
        |_, _| false,
        |x, y| (x, y) == (9_000, 10),
    )
    .unwrap();
    assert_eq!(mutation.winning_frequency, None);
    assert_eq!(nonwinning.powerball_score(0), 1);
    assert_eq!(
        mutation.powerballs[0].version.get(),
        carried.version.get() + 2
    );
    assert!(matches!(
        mutation.powerballs[0].state,
        PowerballState::OnMap { x, y, .. } if (x, y) != (9_000, 10)
    ));
}

#[test]
fn hard_lifecycle_phases_unplaceable_flags_but_requested_drop_stays_atomic() {
    let actor = player(1, 1, 1, 3, 24, 24);
    let carried_registry = || {
        let mut registry = ObjectiveRegistry::new(synthetic_profile(1, 0));
        registry
            .create_flag(FlagState::Carried {
                carrier: actor.runtime,
                x: 24,
                y: 24,
                owner: Some(actor.frequency),
            })
            .unwrap();
        registry
    };
    let unavailable = |_, _| FlagDropTile::with_semantics(false, false);

    let mut hard = carried_registry();
    let mutation = apply_lifecycle_without_goal(
        &mut hard,
        actor,
        ObjectiveLifecycleReason::Disconnect,
        10,
        unavailable,
    )
    .unwrap();
    assert!(matches!(
        mutation.flags[0].state,
        FlagState::Waiting {
            respawn_at_tick: Some(10)
        }
    ));
    assert!(!matches!(
        hard.replay().flags[0].state,
        FlagState::Carried { .. }
    ));

    let mut requested = carried_registry();
    let before = requested.replay().flags[0];
    assert_eq!(
        apply_lifecycle_without_goal(
            &mut requested,
            actor,
            ObjectiveLifecycleReason::Requested,
            10,
            unavailable,
        ),
        Err(ObjectiveMutationError::Registry(
            ObjectiveRegistryError::SpawnUnavailable
        ))
    );
    assert_eq!(requested.replay().flags[0], before);
}

#[test]
fn self_kill_never_transfers_and_friendly_capacity_uses_overflow_policy() {
    let mut self_policy = crate::objectives::synthetic_flag_policy();
    self_policy.transfer_on_friendly_kill = true;
    let mut self_registry = ObjectiveRegistry::new(complete_profile(
        1,
        0,
        self_policy,
        crate::objectives::synthetic_powerball_policy(),
        0,
        ObjectiveTeamLimits::default(),
    ));
    let actor = player(1, 1, 1, 5, 160, 160);
    let flag = self_registry
        .create_flag(FlagState::OnMap {
            x: 160,
            y: 160,
            owner: None,
        })
        .unwrap();
    claim_flag(&mut self_registry, actor, flag.identity, flag.version, 1).unwrap();
    apply_lifecycle_without_goal(
        &mut self_registry,
        actor,
        ObjectiveLifecycleReason::Death {
            killer: Some(actor),
        },
        2,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    assert!(matches!(
        self_registry.flag(flag.identity.id).unwrap().state,
        FlagState::OnMap { .. }
    ));

    let mut overflow_policy = crate::objectives::synthetic_flag_policy();
    overflow_policy.carry_limit = 1;
    overflow_policy.transfer_on_friendly_kill = true;
    overflow_policy.friendly_kill_drop = FlagDropBehavior::NeutralAtCarrier;
    overflow_policy.transfer_overflow_drop = FlagDropBehavior::OwnedAtCenter;
    let mut overflow_registry = ObjectiveRegistry::new(complete_profile(
        2,
        0,
        overflow_policy,
        crate::objectives::synthetic_powerball_policy(),
        0,
        ObjectiveTeamLimits::default(),
    ));
    let killer = player(2, 2, 1, 5, 320, 320);
    let victim = player(3, 3, 1, 5, 160, 160);
    let killer_flag = overflow_registry
        .create_flag(FlagState::OnMap {
            x: 320,
            y: 320,
            owner: None,
        })
        .unwrap();
    let victim_flag = overflow_registry
        .create_flag(FlagState::OnMap {
            x: 160,
            y: 160,
            owner: None,
        })
        .unwrap();
    claim_flag(
        &mut overflow_registry,
        killer,
        killer_flag.identity,
        killer_flag.version,
        1,
    )
    .unwrap();
    claim_flag(
        &mut overflow_registry,
        victim,
        victim_flag.identity,
        victim_flag.version,
        1,
    )
    .unwrap();
    apply_lifecycle_without_goal(
        &mut overflow_registry,
        victim,
        ObjectiveLifecycleReason::Death {
            killer: Some(killer),
        },
        2,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    assert!(matches!(
        overflow_registry
            .flag(victim_flag.identity.id)
            .unwrap()
            .state,
        FlagState::OnMap {
            x: 24,
            y: 24,
            owner: Some(5)
        }
    ));
}

#[test]
fn flag_drop_search_uses_source_flythrough_single_width_and_no_flag_rules() {
    let reserved = std::collections::BTreeSet::new();
    let mut seed = 1;
    let through_door =
        flag_drop_tile_near(&mut seed, 10, 10, 0, &reserved, &mut |x, y| match (x, y) {
            (10, 10) | (11, 9) | (11, 10) | (11, 11) | (12, 9) | (12, 11) => {
                FlagDropTile::with_semantics(true, false)
            }
            (12, 10) => FlagDropTile::available(),
            _ => FlagDropTile::with_semantics(false, false),
        });
    assert_eq!(through_door, Some((12, 10)));

    let mut seed = 1;
    let single_width =
        flag_drop_tile_near(&mut seed, 10, 10, 0, &reserved, &mut |x, y| match (x, y) {
            (10, 10) => FlagDropTile::with_semantics(true, false),
            (11, 10) => FlagDropTile::available(),
            _ => FlagDropTile::with_semantics(false, false),
        });
    assert_eq!(single_width, None);

    let mut seed = 1;
    let no_flag_region =
        flag_drop_tile_near(&mut seed, 10, 10, 0, &reserved, &mut |x, y| match (x, y) {
            (10, 10) => FlagDropTile::with_semantics(true, false),
            _ => FlagDropTile::with_semantics(false, false),
        });
    assert_eq!(no_flag_region, None);

    let mut seed = 7;
    let selected = flag_drop_tile_near(&mut seed, 10, 10, 0, &reserved, &mut |x, y| {
        if (x, y) == (10, 10) {
            FlagDropTile::with_semantics(true, false)
        } else if x.abs_diff(10) + y.abs_diff(10) == 1 {
            FlagDropTile::available()
        } else if x.abs_diff(10) == 1 && y.abs_diff(10) == 1 {
            FlagDropTile::with_semantics(true, false)
        } else {
            FlagDropTile::with_semantics(false, false)
        }
    });
    assert!(selected.is_some());
    let mut expected_seed = 7;
    for _ in 0..3 {
        crate::vie::vie_rng_next(&mut expected_seed);
    }
    assert_eq!(
        seed, expected_seed,
        "all four coordinates in the completed frontier must enter the Fisher-Yates shuffle"
    );
}

#[test]
fn multi_flag_drop_shuffles_one_complete_zero_radius_coordinate_set() {
    let actor = player(1, 1, 1, 3, 168, 168);
    let mut policy = crate::objectives::synthetic_flag_policy();
    policy.drop_radius_tiles = 0;
    let mut registry = ObjectiveRegistry::new(complete_profile(
        3,
        0,
        policy,
        crate::objectives::synthetic_powerball_policy(),
        0,
        ObjectiveTeamLimits::default(),
    ));
    for _ in 0..3 {
        registry
            .create_flag(FlagState::Carried {
                carrier: actor.runtime,
                x: actor.position_x,
                y: actor.position_y,
                owner: None,
            })
            .unwrap();
    }
    registry.set_rng_seed(7);

    let mutation = apply_lifecycle_without_goal(
        &mut registry,
        actor,
        ObjectiveLifecycleReason::Requested,
        10,
        |x, y| {
            if (8..=12).contains(&x) && (8..=12).contains(&y) {
                if x.abs_diff(10) + y.abs_diff(10) <= 1 {
                    FlagDropTile::available()
                } else {
                    FlagDropTile::with_semantics(true, false)
                }
            } else {
                FlagDropTile::with_semantics(false, false)
            }
        },
    )
    .unwrap();

    assert_eq!(
        mutation
            .flags
            .iter()
            .map(|flag| match flag.state {
                FlagState::OnMap { x, y, .. } => (flag.identity.id, (x / 16, y / 16)),
                state => panic!("expected dropped flag, got {state:?}"),
            })
            .collect::<Vec<_>>(),
        vec![(0, (9, 10)), (1, (10, 10)), (2, (11, 10))],
        "stable flag IDs consume the single shuffled coordinate set in order"
    );
    assert_eq!(*registry.rng_seed_mut(), 708_919_132);
}

#[test]
fn insufficient_multi_flag_drop_shuffles_once_then_center_fallbacks_remainder() {
    let actor = player(1, 1, 1, 3, 168, 168);
    let mut policy = crate::objectives::synthetic_flag_policy();
    policy.drop_radius_tiles = 0;
    let mut profile = complete_profile(
        3,
        0,
        policy,
        crate::objectives::synthetic_powerball_policy(),
        0,
        ObjectiveTeamLimits::default(),
    );
    let ServerFlagProfile::Enabled(flags) = &mut profile.flags else {
        unreachable!()
    };
    flags.spawn = ObjectiveSpawnArea {
        x: 100,
        y: 100,
        radius: 0,
    };
    let mut registry = ObjectiveRegistry::new(profile);
    for _ in 0..3 {
        registry
            .create_flag(FlagState::Carried {
                carrier: actor.runtime,
                x: actor.position_x,
                y: actor.position_y,
                owner: None,
            })
            .unwrap();
    }
    registry.set_rng_seed(11);

    let mutation = apply_lifecycle_without_goal(
        &mut registry,
        actor,
        ObjectiveLifecycleReason::Requested,
        10,
        |x, y| {
            if (8..=12).contains(&x) && (8..=12).contains(&y) {
                if (x, y) == (10, 10) || (x, y) == (9, 10) {
                    FlagDropTile::available()
                } else {
                    FlagDropTile::with_semantics(true, false)
                }
            } else if (x, y) == (100, 100) {
                FlagDropTile::available()
            } else {
                FlagDropTile::with_semantics(false, false)
            }
        },
    )
    .unwrap();

    assert_eq!(
        mutation
            .flags
            .iter()
            .map(|flag| match flag.state {
                FlagState::OnMap { x, y, .. } => (flag.identity.id, (x / 16, y / 16)),
                state => panic!("expected dropped flag, got {state:?}"),
            })
            .collect::<Vec<_>>(),
        vec![(0, (9, 10)), (1, (10, 10)), (2, (100, 100))]
    );
    assert_eq!(*registry.rng_seed_mut(), 1_056_066_286);
}

#[test]
fn hard_lifecycle_version_exhaustion_quarantines_every_old_runtime_carrier() {
    for reason in [
        ObjectiveLifecycleReason::Death { killer: None },
        ObjectiveLifecycleReason::ShipOrFrequency,
        ObjectiveLifecycleReason::Disconnect,
        ObjectiveLifecycleReason::SessionOrArena,
    ] {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let player_id = register_connection(&shared, tx);
        join_arena(&shared, player_id, 0);
        let mut state = shared.lock().unwrap();
        state.objectives.reset(synthetic_profile(1, 1)).unwrap();
        state.clients.get_mut(&player_id).unwrap().last_position =
            Some(client_position_packet_with_sequence(0, 320, 320, 41));
        let context = crate::objectives::objective_player(&state, player_id).unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::Carried {
                carrier: context.runtime,
                x: context.position_x,
                y: context.position_y,
                owner: None,
            })
            .unwrap();
        let ball = state
            .objectives
            .create_powerball(PowerballState::Carried {
                carrier: context.runtime,
            })
            .unwrap();
        assert!(
            state
                .objectives
                .force_flag_version(flag.identity.id, crate::continuum::CONTINUUM_TICK_MASK)
        );
        assert!(
            state
                .objectives
                .force_powerball_version(ball.identity.id, crate::continuum::CONTINUUM_TICK_MASK)
        );
        let old_generation = state.objectives.replay().generation;

        let packets = crate::objectives::player_lifecycle_packets_for_context_with_tiles(
            &mut state,
            context,
            None,
            reason,
            50,
            |_, _| FlagDropTile::available(),
            |_, _| false,
            |_, _| false,
        )
        .unwrap();

        let replay = state.objectives.replay();
        assert_eq!(replay.generation, old_generation + 1);
        assert!(matches!(
            replay.flags[0].state,
            FlagState::Waiting {
                respawn_at_tick: Some(50)
            }
        ));
        assert!(matches!(
            replay.powerballs[0].state,
            PowerballState::Waiting {
                respawn_at_tick: Some(50)
            }
        ));
        assert_eq!(state.objectives.flag_carrier_count(context.runtime), 0);
        assert!(
            state
                .objectives
                .carried_powerball_ids(context.runtime)
                .is_empty()
        );
        let updates = packets
            .iter()
            .filter_map(|packet| {
                crate::objective_protocol::ObjectiveStateUpdate::parse(packet).ok()
            })
            .collect::<Vec<_>>();
        assert_eq!(updates.len(), 2);
        assert!(
            updates
                .iter()
                .all(|update| update.generation == replay.generation)
        );

        step_objective_deadlines(
            &mut state.objectives,
            &[],
            51,
            |_, _| false,
            |_, _| FlagDropTile::available(),
        )
        .unwrap();
        let replay = state.objectives.replay();
        assert!(matches!(replay.flags[0].state, FlagState::OnMap { .. }));
        assert!(matches!(
            replay.powerballs[0].state,
            PowerballState::OnMap { .. }
        ));
    }
}

#[test]
fn generation_exhaustion_is_typed_and_leaves_runtime_carriers_intact() {
    for reason in [
        ObjectiveLifecycleReason::Death { killer: None },
        ObjectiveLifecycleReason::ShipOrFrequency,
        ObjectiveLifecycleReason::Disconnect,
        ObjectiveLifecycleReason::SessionOrArena,
    ] {
        let actor = player(1, 1, 1, 3, 320, 320);
        let mut state = team_mode_state();
        state.objectives.reset(synthetic_profile(1, 1)).unwrap();
        let flag = state
            .objectives
            .create_flag(FlagState::Carried {
                carrier: actor.runtime,
                x: actor.position_x,
                y: actor.position_y,
                owner: None,
            })
            .unwrap();
        state
            .objectives
            .create_powerball(PowerballState::Carried {
                carrier: actor.runtime,
            })
            .unwrap();
        assert!(
            state
                .objectives
                .force_flag_version(flag.identity.id, crate::continuum::CONTINUUM_TICK_MASK)
        );
        state.objectives.force_generation(u64::MAX);

        assert_eq!(
            crate::objectives::player_lifecycle_packets_for_context_with_tiles(
                &mut state,
                actor,
                None,
                reason,
                50,
                |_, _| FlagDropTile::available(),
                |_, _| false,
                |_, _| false,
            ),
            Err(ObjectiveRegistryError::GenerationExhausted)
        );
        assert_eq!(state.objectives.flag_carrier_count(actor.runtime), 1);
        assert_eq!(
            state.objectives.carried_powerball_ids(actor.runtime),
            vec![0]
        );
    }
}

#[test]
fn zero_delay_death_goal_spawn_failure_quarantines_flag_and_ball_atomically() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, _rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    join_arena(&shared, player_id, 0);
    let mut state = shared.lock().unwrap();
    let mut ball_policy = crate::objectives::synthetic_powerball_policy();
    ball_policy.death_scores_goal = true;
    let mut profile = complete_profile(
        1,
        1,
        crate::objectives::synthetic_flag_policy(),
        ball_policy,
        0,
        ObjectiveTeamLimits::default(),
    );
    let ServerPowerballProfile::Enabled(ball_profile) = &mut profile.powerballs else {
        unreachable!()
    };
    ball_profile.goal_delay_ticks = 0;
    state.objectives.reset(profile).unwrap();
    state.clients.get_mut(&player_id).unwrap().player.frequency = 1;
    state.clients.get_mut(&player_id).unwrap().last_position =
        Some(client_position_packet_with_sequence(0, 320, 320, 41));
    let context = crate::objectives::objective_player(&state, player_id).unwrap();
    state
        .objectives
        .create_flag(FlagState::Carried {
            carrier: context.runtime,
            x: context.position_x,
            y: context.position_y,
            owner: None,
        })
        .unwrap();
    state
        .objectives
        .create_powerball(PowerballState::Carried {
            carrier: context.runtime,
        })
        .unwrap();
    let old_generation = state.objectives.replay().generation;

    let packets = crate::objectives::player_lifecycle_packets_for_context_with_tiles(
        &mut state,
        context,
        None,
        ObjectiveLifecycleReason::Death { killer: None },
        90,
        |_, _| FlagDropTile::with_semantics(true, true),
        |_, _| true,
        |_, _| true,
    )
    .unwrap();

    let replay = state.objectives.replay();
    assert_eq!(replay.generation, old_generation + 1);
    assert!(replay.flags.iter().all(|flag| matches!(
        flag.state,
        FlagState::Waiting {
            respawn_at_tick: Some(90)
        }
    )));
    assert!(replay.powerballs.iter().all(|ball| matches!(
        ball.state,
        PowerballState::Waiting {
            respawn_at_tick: Some(90)
        }
    )));
    assert_eq!(state.objectives.powerball_score(context.frequency), 0);
    assert!(
        packets
            .iter()
            .filter_map(
                |packet| crate::objective_protocol::ObjectiveStateUpdate::parse(packet).ok()
            )
            .all(|update| update.generation == replay.generation)
    );
}

#[test]
fn claimant_ship_selects_flag_hull_and_powerball_proximity() {
    let mut flag_policy = crate::objectives::synthetic_flag_policy();
    flag_policy.collision_radius_pixels[0] = 4;
    flag_policy.collision_radius_pixels[1] = 12;
    let mut registry = ObjectiveRegistry::new(complete_profile(
        1,
        0,
        flag_policy,
        crate::objectives::synthetic_powerball_policy(),
        0,
        ObjectiveTeamLimits::default(),
    ));
    let flag = registry
        .create_flag(FlagState::OnMap {
            x: 100,
            y: 100,
            owner: None,
        })
        .unwrap();
    let mut narrow = player(1, 1, 1, 3, 120, 100);
    narrow.ship = 0;
    assert_eq!(
        claim_flag(&mut registry, narrow, flag.identity, flag.version, 1),
        Err(ObjectiveMutationError::OutOfRange)
    );
    let mut wide = narrow;
    wide.ship = 1;
    assert!(claim_flag(&mut registry, wide, flag.identity, flag.version, 1).is_ok());

    let mut ball_policy = crate::objectives::synthetic_powerball_policy();
    ball_policy.pickup_radius_pixels[0] = 10;
    ball_policy.pickup_radius_pixels[1] = 30;
    let mut registry = ObjectiveRegistry::new(complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        ball_policy,
        0,
        ObjectiveTeamLimits::default(),
    ));
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 100,
            y: 100,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    assert_eq!(
        pickup_powerball(&mut registry, narrow, ball.identity, ball.version, 1, true),
        Err(ObjectiveMutationError::OutOfRange)
    );
    assert!(pickup_powerball(&mut registry, wide, ball.identity, ball.version, 1, true).is_ok());
}

#[test]
fn carrier_ship_selects_throw_timer_speed_and_persisted_friction() {
    let mut policy = crate::objectives::synthetic_powerball_policy();
    policy.carry_timeout_ticks[0] = 5;
    policy.carry_timeout_ticks[1] = 10;
    policy.throw_speed[0] = 100;
    policy.throw_speed[1] = 240;
    policy.friction_delta[0] = 111;
    policy.friction_delta[1] = 222;
    let profile = complete_profile(
        0,
        1,
        crate::objectives::synthetic_flag_policy(),
        policy,
        0,
        ObjectiveTeamLimits::default(),
    );

    for (ship, timeout, speed, friction) in [(0_u8, 5_u32, 100_i16, 111_i16), (1, 10, 240, 222)] {
        let mut manual_registry = ObjectiveRegistry::new(profile.clone());
        let mut thrower = player(1, 1, 1, 3, 320, 320);
        thrower.ship = ship;
        let manual_ball = manual_registry
            .create_powerball(PowerballState::Carried {
                carrier: thrower.runtime,
            })
            .unwrap();
        let released = fire_powerball(
            &mut manual_registry,
            thrower,
            manual_ball.identity,
            manual_ball.version,
            3,
            false,
        )
        .unwrap();
        assert!(
            matches!(released.state, PowerballState::OnMap { velocity_y, .. } if velocity_y == -25 - speed)
        );
        assert_eq!(
            manual_registry
                .powerball_motion(manual_ball.identity.id)
                .unwrap()
                .friction_delta,
            friction
        );

        let mut registry = ObjectiveRegistry::new(profile.clone());
        let mut carrier = player(2, 2, 1, 4, 320, 320);
        carrier.ship = ship;
        let ball = registry
            .create_powerball(PowerballState::Carried {
                carrier: carrier.runtime,
            })
            .unwrap();
        registry.set_powerball_carried_since(ball.identity, 0);
        assert!(
            step_objective_deadlines(
                &mut registry,
                &[carrier],
                timeout - 1,
                |_, _| false,
                |_, _| FlagDropTile::available(),
            )
            .unwrap()
            .powerballs
            .is_empty()
        );
        let mutation = step_objective_deadlines(
            &mut registry,
            &[carrier],
            timeout,
            |_, _| false,
            |_, _| FlagDropTile::available(),
        )
        .unwrap();
        assert_eq!(mutation.powerballs.len(), 1);
        assert!(
            matches!(mutation.powerballs[0].state, PowerballState::OnMap { velocity_y, .. } if velocity_y == -25 + speed)
        );
        assert_eq!(
            registry
                .powerball_motion(ball.identity.id)
                .unwrap()
                .friction_delta,
            friction
        );
    }
}

#[test]
fn catchup_stops_on_first_goal_crossing_instead_of_skipping_it() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(0, 1));
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 24,
            y: 24,
            velocity_x: 16_000,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let mut motion = PowerballMotion::from_wire_position(24, 24, 16_000, 0, 0, 0);
    motion.last_touch_frequency = Some(1);
    registry
        .set_powerball_motion(ball.identity, motion)
        .unwrap();

    step_powerball_motion(
        &mut registry,
        2,
        |_, _, _| false,
        |frequency, x, _| frequency == Some(1) && x / 16 == 2,
    )
    .unwrap();

    assert_eq!(
        registry
            .powerball_motion(ball.identity.id)
            .unwrap()
            .wire_position()
            .0,
        40
    );
}

#[test]
fn fixed_step_catchup_is_wrap_safe_and_bounded() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(0, 1));
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 16,
            y: 16,
            velocity_x: 10,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    let start = crate::continuum::CONTINUUM_TICK_MASK - 2;
    registry
        .set_powerball_motion(
            ball.identity,
            PowerballMotion::from_wire_position(16, 16, 10, 0, 0, start),
        )
        .unwrap();
    step_powerball_motion(&mut registry, 1, |_, _, _| false, |_, _, _| false).unwrap();
    let wrapped = registry.powerball_motion(ball.identity.id).unwrap();
    assert_eq!(wrapped.position_x, 16_040);
    assert_eq!(wrapped.last_step_tick, 1);

    let before = wrapped.position_x;
    step_powerball_motion(&mut registry, 7_001, |_, _, _| false, |_, _, _| false).unwrap();
    let bounded = registry.powerball_motion(ball.identity.id).unwrap();
    assert_eq!(bounded.position_x - before, 60_000);
}

#[test]
fn powerball_publication_cadence_uses_wrap_safe_elapsed_time_not_tick_modulus() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(0, 1));
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 16,
            y: 16,
            velocity_x: 10,
            velocity_y: 0,
            pickup_unlock_tick: None,
        })
        .unwrap();
    registry
        .set_powerball_motion(
            ball.identity,
            PowerballMotion::from_wire_position(16, 16, 10, 0, 0, 7),
        )
        .unwrap();
    assert!(
        step_powerball_motion(&mut registry, 25, |_, _, _| false, |_, _, _| false,)
            .unwrap()
            .is_empty(),
        "absolute tick divisibility must not publish before elapsed cadence"
    );
    let first_publication =
        step_powerball_motion(&mut registry, 32, |_, _, _| false, |_, _, _| false).unwrap();
    assert_eq!(first_publication.len(), 1);
    assert_eq!(first_publication[0].version, ball.version);
    assert_eq!(
        registry.powerball(ball.identity.id).unwrap().version,
        ball.version
    );

    let second_publication =
        step_powerball_motion(&mut registry, 57, |_, _, _| false, |_, _, _| false).unwrap();
    assert_eq!(second_publication.len(), 1);
    assert_eq!(second_publication[0].version, ball.version);

    let start = crate::continuum::CONTINUUM_TICK_MASK - 10;
    let current = registry.powerball(ball.identity.id).unwrap();
    registry
        .set_powerball_motion(
            current.identity,
            PowerballMotion::from_wire_position(16, 16, 10, 0, 0, start),
        )
        .unwrap();
    assert_eq!(
        step_powerball_motion(&mut registry, 14, |_, _, _| false, |_, _, _| false,)
            .unwrap()
            .len(),
        1,
        "elapsed cadence must publish across the 31-bit tick wrap"
    );
    assert!(
        pickup_powerball(
            &mut registry,
            player(9, 9, 1, 1, 16, 16),
            ball.identity,
            ball.version,
            15,
            true,
        )
        .is_ok()
    );
}

#[test]
fn late_join_replay_is_stable_complete_generation_version_state() {
    let mut state = team_mode_state();
    state.objectives.reset(synthetic_profile(1, 1)).unwrap();
    initialize_objectives(
        &mut state.objectives,
        9,
        |_, _| false,
        |_, _| FlagDropTile::available(),
    )
    .unwrap();
    let ball = state.objectives.replay().powerballs[0];
    let mut motion = state.objectives.powerball_motion(ball.identity.id).unwrap();
    motion.last_touch = Some(runtime(42, 4, 2));
    motion.last_touch_frequency = Some(3);
    state
        .objectives
        .set_powerball_motion(ball.identity, motion)
        .unwrap();
    let first = objective_state_replay_packets(&state, 10);
    let second = objective_state_replay_packets(&state, 10);
    assert_eq!(first, second);
    assert_eq!(first.len(), 10);
    let decoded = first[..2]
        .iter()
        .map(|packet| crate::objective_protocol::ObjectiveStateUpdate::parse(packet).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        decoded[0].kind,
        crate::objective_protocol::ObjectiveStateUpdate::KIND_FLAG
    );
    assert_eq!(
        decoded[1].kind,
        crate::objective_protocol::ObjectiveStateUpdate::KIND_POWERBALL
    );
    assert_eq!(decoded[1].owner_id, Some(42));
    assert_eq!(decoded[1].owner_frequency, Some(3));
    assert!(decoded.iter().all(|update| {
        update.generation > 0
            && update.version > 0
            && update.outcome == crate::objective_protocol::ObjectiveStateUpdate::OUTCOME_REPLAY
    }));
    let scores = first[2..]
        .iter()
        .map(|packet| crate::objective_protocol::ObjectiveScoreUpdate::parse(packet).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        scores
            .iter()
            .map(|score| (score.frequency, score.score))
            .collect::<Vec<_>>(),
        vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
            (5, 0),
            (6, 0),
            (7, 0),
        ]
    );
    assert!(scores.iter().all(|score| {
        score.generation == decoded[0].generation
            && score.outcome == crate::objective_protocol::ObjectiveScoreUpdate::OUTCOME_REPLAY
    }));
}

#[test]
fn objective_awards_update_roster_score_and_identity_persistence() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(team_mode_state()));
    let (tx, _rx) = std::sync::mpsc::channel();
    let player_id = register_connection(&shared, tx);
    set_player_identity_and_name(
        &shared,
        player_id,
        "000000000777".to_string(),
        "scorer".to_string(),
    );
    let mut state = shared.lock().unwrap();
    // Start from a nonzero earned score to keep checking accumulation;
    // fresh production identities now begin at zero.
    state
        .clients
        .get_mut(&player_id)
        .unwrap()
        .player
        .flag_points = 2_000;
    let mutation = crate::objectives::ObjectiveMutation {
        awards: vec![
            ObjectiveAward {
                player_id,
                points: 125,
            },
            ObjectiveAward {
                player_id,
                points: 25,
            },
        ],
        ..crate::objectives::ObjectiveMutation::default()
    };
    let packets = crate::objectives::mutation_packets(&mut state, &mutation, 10, 0);

    assert_eq!(state.clients[&player_id].player.flag_points, 2_150);
    assert_eq!(state.identity_stats["000000000777"].flag_points, 2_150);
    assert_eq!(packets.len(), 1);
    assert_eq!(
        u32::from_le_bytes(packets[0][7..11].try_into().unwrap()),
        2_150
    );
}

#[test]
fn mode_five_frequency_two_uses_primary_x_branch_and_complement() {
    let x_left_y_low = (400 * 16, 300 * 16);
    assert!(!soccer_goal_counts_world(
        x_left_y_low.0,
        x_left_y_low.1,
        2,
        5
    ));
    assert!(soccer_goal_counts_world(
        x_left_y_low.0,
        x_left_y_low.1,
        2,
        6
    ));

    let x_right_sum_high = (700 * 16, 500 * 16);
    assert!(soccer_goal_counts_world(
        x_right_sum_high.0,
        x_right_sum_high.1,
        2,
        5
    ));
    assert!(!soccer_goal_counts_world(
        x_right_sum_high.0,
        x_right_sum_high.1,
        2,
        6
    ));
}
