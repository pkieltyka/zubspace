use super::*;

fn flag_settings() -> FlagProfileSettings {
    FlagProfileSettings {
        spawn: ObjectiveSpawnArea {
            x: 512,
            y: 512,
            radius: 20,
        },
        carry_timeout_ticks: 100,
        reset_delay_ticks: 100,
        reward: 0,
        team_limits: ObjectiveTeamLimits::default(),
        policy: crate::objectives::synthetic_flag_policy(),
    }
}

fn powerball_settings() -> PowerballProfileSettings {
    PowerballProfileSettings {
        spawns: vec![ObjectiveSpawnArea {
            x: 512,
            y: 512,
            radius: 20,
        }],
        send_cadence_ticks: 25,
        goal_delay_ticks: 100,
        reward: 0,
        win_by: 0,
        team_limits: ObjectiveTeamLimits::default(),
        policy: crate::objectives::synthetic_powerball_policy(),
    }
}

#[test]
fn zone_objective_profiles_come_from_the_authored_section_not_map_tiles() {
    // Authored map flag tiles stay inert metadata: a zone only runs objectives
    // when its own arena.conf ships an [Objective] section.
    for zone_id in ["chaos-zone", "extreme"] {
        let map = zone_map_for_id(zone_id).expect("bundled zone");
        assert!(map.objective_profile.is_disabled(), "{zone_id}");
        assert_eq!(map.objective_profile.flags.count(), 0, "{zone_id}");
        assert_eq!(map.objective_profile.powerballs.count(), 0, "{zone_id}");
    }

    // The repo's only authored `[Objective]` profile text now lives in the e2e
    // fixture `tests/fixtures/zones/flag-test/arena.conf` (shipped Extreme
    // settings plus an authored flag section), assembled at run time behind the
    // browser gates `make test-e2e-flag-carry-badge` / `test-e2e-flag-carrier-ride`.
    // Parse it through the same entry point zone building uses so the native
    // suite keeps a real authored profile, not only synthetic settings.
    let fixture = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/zones/flag-test/arena.conf"
    ))
    .expect("flag-test fixture profile should exist");
    let extreme = zone_map_for_id("extreme").expect("extreme zone should be bundled");
    let profile = crate::arena_settings::objective_profile_from_bundle(
        "flag-test",
        &fixture,
        extreme.settings_packet.as_ref(),
    )
    .expect("fixture profile must parse through the zone-build entry point");
    let crate::objectives::ServerFlagProfile::Enabled(settings) = &profile.flags else {
        panic!(
            "the flag-test fixture authors an enabled flag profile, got {:?}",
            profile.flags
        );
    };
    assert_eq!(profile.flags.count(), 1);
    assert_eq!(profile.powerballs.count(), 0);
    // The seat is authored off the centre-core tile (512,512) that a local join spawns on. Seeded
    // there the flag sat under the ship, every join auto-claimed it within a second, and the badge
    // gate's "dark before the capture" premise became a race the gate usually lost.
    assert_eq!(
        (settings.spawn.x, settings.spawn.y, settings.spawn.radius),
        (540, 540, 64),
        "the fixture authors its spawn area in map tiles, off the join spawn tile"
    );
    assert_ne!(
        (settings.spawn.x, settings.spawn.y),
        (512, 512),
        "the authored seat must not land on the centre-core tile a local join spawns on"
    );
    // 0 means "no carry-expiry rule"; the badge and the deadline step both read
    // it that way, so an authored profile must not silently acquire a timer.
    assert_eq!(settings.carry_timeout_ticks, 0);
    assert!(
        settings
            .policy
            .collision_radius_pixels
            .iter()
            .all(|radius| *radius > 0),
        "claim radii must be derived from the fixture's own ship hulls: {:?}",
        settings.policy.collision_radius_pixels
    );
}

#[test]
fn objective_profile_is_disabled_without_the_operator_opt_in() {
    // Flags and powerballs are deferred for this iteration: an authored
    // [Objective] profile must stay inert unless the operator explicitly
    // enables objectives, and the enabling flag must not invent a profile for a
    // zone that authored none.
    let fixture = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/zones/flag-test/arena.conf"
    ))
    .expect("flag-test fixture profile should exist");
    let extreme = zone_map_for_id("extreme").expect("extreme zone should be bundled");
    let authored = crate::arena_settings::objective_profile_from_bundle(
        "flag-test",
        &fixture,
        extreme.settings_packet.as_ref(),
    )
    .expect("fixture profile must parse through the zone-build entry point");
    assert_eq!(authored.flags.count(), 1, "the fixture authors one flag");

    let disabled = crate::objectives::gated_objective_profile(false, Some(&authored));
    assert!(
        disabled.is_disabled(),
        "without the operator opt-in an authored objective profile must be disabled"
    );
    assert_eq!(disabled.flags.count(), 0);
    assert_eq!(disabled.powerballs.count(), 0);

    let enabled = crate::objectives::gated_objective_profile(true, Some(&authored));
    assert_eq!(
        enabled.flags.count(),
        1,
        "the opt-in activates the authored profile"
    );

    assert!(
        crate::objectives::gated_objective_profile(true, None).is_disabled(),
        "the opt-in cannot invent objectives for a zone that authors none"
    );
}

#[test]
fn zero_count_has_one_unambiguous_disabled_representation() {
    assert_eq!(
        ServerFlagProfile::validate(0, None).unwrap(),
        ServerFlagProfile::Disabled
    );
    assert_eq!(
        ServerPowerballProfile::validate(0, None).unwrap(),
        ServerPowerballProfile::Disabled
    );
    assert!(ServerFlagProfile::validate(0, Some(flag_settings())).is_err());
    assert!(ServerFlagProfile::validate(1, None).is_err());
    assert!(ServerPowerballProfile::validate(0, Some(powerball_settings())).is_err());
    assert!(ServerPowerballProfile::validate(1, None).is_err());
}

#[test]
fn profile_validation_enforces_global_counts_geometry_timing_and_team_caps() {
    assert!(ServerFlagProfile::validate(256, Some(flag_settings())).is_ok());
    assert!(ServerFlagProfile::validate(257, Some(flag_settings())).is_err());
    assert!(ServerPowerballProfile::validate(8, Some(powerball_settings())).is_ok());
    assert!(ServerPowerballProfile::validate(9, Some(powerball_settings())).is_err());

    let mut flags = flag_settings();
    flags.spawn.x = 1024;
    assert!(ServerFlagProfile::validate(1, Some(flags)).is_err());
    let mut flags = flag_settings();
    flags.spawn.radius = 1025;
    assert!(ServerFlagProfile::validate(1, Some(flags)).is_err());
    let mut flags = flag_settings();
    flags.carry_timeout_ticks = 1 << 30;
    assert!(ServerFlagProfile::validate(1, Some(flags)).is_err());
    let mut flags = flag_settings();
    flags.team_limits.min_players = 769;
    assert!(ServerFlagProfile::validate(1, Some(flags)).is_err());

    let mut balls = powerball_settings();
    balls.spawns.clear();
    assert!(ServerPowerballProfile::validate(1, Some(balls)).is_err());
    let mut balls = powerball_settings();
    balls.spawns.push(balls.spawns[0]);
    assert!(ServerPowerballProfile::validate(1, Some(balls)).is_err());
    let mut balls = powerball_settings();
    balls.send_cadence_ticks = 24;
    assert!(ServerPowerballProfile::validate(1, Some(balls)).is_err());
    let mut balls = powerball_settings();
    balls.send_cadence_ticks = 501;
    assert!(ServerPowerballProfile::validate(1, Some(balls)).is_err());
    let mut balls = powerball_settings();
    balls.goal_delay_ticks = 1 << 30;
    assert!(ServerPowerballProfile::validate(1, Some(balls)).is_err());
    let mut balls = powerball_settings();
    balls.win_by = i32::MAX as u32 + 1;
    assert!(ServerPowerballProfile::validate(1, Some(balls)).is_err());
    let mut balls = powerball_settings();
    balls.team_limits.max_players_per_team = 769;
    assert!(ServerPowerballProfile::validate(1, Some(balls)).is_err());
    let mut balls = powerball_settings();
    balls.policy.bounce = false;
    assert!(ServerPowerballProfile::validate(1, Some(balls)).is_err());
    let mut balls = powerball_settings();
    balls.policy.soccer_mode = 7;
    assert!(ServerPowerballProfile::validate(1, Some(balls)).is_err());
    let mut flags = flag_settings();
    flags.policy.carry_limit = 0;
    assert!(ServerFlagProfile::validate(1, Some(flags)).is_err());

    let mut flags = flag_settings();
    flags.reward = i32::MIN;
    assert!(ServerFlagProfile::validate(1, Some(flags)).is_ok());
    let mut balls = powerball_settings();
    balls.reward = i32::MAX;
    assert!(ServerPowerballProfile::validate(1, Some(balls)).is_ok());
}

#[test]
fn registry_capacity_and_server_identity_are_profile_bounded() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(2, 2));
    let flag0 = registry
        .create_flag(FlagState::Waiting {
            respawn_at_tick: None,
        })
        .unwrap();
    let flag1 = registry
        .create_flag(FlagState::Waiting {
            respawn_at_tick: Some(10),
        })
        .unwrap();
    assert_eq!((flag0.identity.id, flag1.identity.id), (0, 1));
    assert_eq!(
        registry.create_flag(FlagState::Waiting {
            respawn_at_tick: None
        }),
        Err(ObjectiveRegistryError::Capacity)
    );

    let ball0 = registry
        .create_powerball(PowerballState::Waiting {
            respawn_at_tick: None,
        })
        .unwrap();
    let ball1 = registry
        .create_powerball(PowerballState::Waiting {
            respawn_at_tick: Some(10),
        })
        .unwrap();
    assert_eq!((ball0.identity.id, ball1.identity.id), (0, 1));
    assert_eq!(
        registry.create_powerball(PowerballState::Waiting {
            respawn_at_tick: None
        }),
        Err(ObjectiveRegistryError::Capacity)
    );

    let mut disabled = ObjectiveRegistry::default();
    assert_eq!(
        disabled.create_flag(FlagState::Waiting {
            respawn_at_tick: None
        }),
        Err(ObjectiveRegistryError::Disabled)
    );
    assert_eq!(
        disabled.create_powerball(PowerballState::Waiting {
            respawn_at_tick: None
        }),
        Err(ObjectiveRegistryError::Disabled)
    );
}

#[test]
fn explicit_transitions_reject_duplicate_reordered_and_old_generation_mutations() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(1, 1));
    let flag = registry
        .create_flag(FlagState::Waiting {
            respawn_at_tick: None,
        })
        .unwrap();
    let on_map = registry
        .transition_flag(
            flag.identity,
            flag.version,
            FlagState::OnMap {
                x: 100,
                y: 200,
                owner: None,
            },
        )
        .unwrap();
    assert_eq!(
        registry.transition_flag(
            flag.identity,
            flag.version,
            FlagState::Waiting {
                respawn_at_tick: None
            }
        ),
        Err(ObjectiveRegistryError::StaleVersion)
    );
    assert_eq!(registry.flag(0), Some(on_map));

    let old_identity = on_map.identity;
    registry
        .reset(synthetic_profile(1, 1))
        .expect("generation should advance");
    let replacement = registry
        .create_flag(FlagState::Waiting {
            respawn_at_tick: None,
        })
        .unwrap();
    assert_eq!(replacement.identity.id, old_identity.id);
    assert_ne!(replacement.identity.generation, old_identity.generation);
    assert_eq!(
        registry.transition_flag(
            old_identity,
            on_map.version,
            FlagState::OnMap {
                x: 1,
                y: 1,
                owner: None
            }
        ),
        Err(ObjectiveRegistryError::UnknownIdentity)
    );
}

#[test]
fn version_and_generation_exhaustion_are_fail_closed_without_token_reuse() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(0, 1));
    let original = registry
        .create_powerball(PowerballState::Waiting {
            respawn_at_tick: None,
        })
        .unwrap();
    assert!(
        registry
            .force_powerball_version(original.identity.id, crate::continuum::CONTINUUM_TICK_MASK)
    );
    let exhausted = registry.powerball(original.identity.id).unwrap();
    assert_eq!(
        registry.transition_powerball(
            exhausted.identity,
            exhausted.version,
            PowerballState::OnMap {
                x: 1,
                y: 1,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: None,
            },
        ),
        Err(ObjectiveRegistryError::VersionExhausted)
    );
    assert_eq!(registry.powerball(original.identity.id), Some(exhausted));

    // The token issued for the object's original version 1 must never become
    // current again, even after the version space is exhausted.
    assert_eq!(
        registry.transition_powerball(
            original.identity,
            original.version,
            PowerballState::OnMap {
                x: 2,
                y: 2,
                velocity_x: 0,
                velocity_y: 0,
                pickup_unlock_tick: None,
            },
        ),
        Err(ObjectiveRegistryError::StaleVersion)
    );
    assert_eq!(registry.powerball(original.identity.id), Some(exhausted));

    registry.force_generation(u64::MAX);
    let before = registry.replay();
    assert_eq!(
        registry.reset(synthetic_profile(0, 1)),
        Err(ObjectiveRegistryError::GenerationExhausted)
    );
    assert_eq!(registry.replay(), before);
}

#[test]
fn pickup_lock_expiry_clears_only_release_metadata_without_consuming_a_version() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(0, 1));
    let ball = registry
        .create_powerball(PowerballState::OnMap {
            x: 10,
            y: 20,
            velocity_x: 30,
            velocity_y: 40,
            pickup_unlock_tick: Some(100),
        })
        .unwrap();
    assert!(
        registry.force_powerball_version(ball.identity.id, crate::continuum::CONTINUUM_TICK_MASK)
    );
    let exhausted = registry.powerball(ball.identity.id).unwrap();

    registry.expire_powerball_pickup_locks(99);
    assert_eq!(registry.powerball(ball.identity.id), Some(exhausted));

    registry.expire_powerball_pickup_locks(100);
    let unlocked = registry.powerball(ball.identity.id).unwrap();
    assert_eq!(unlocked.version, exhausted.version);
    assert!(matches!(
        unlocked.state,
        PowerballState::OnMap {
            pickup_unlock_tick: None,
            ..
        }
    ));
}

#[test]
fn replay_is_bounded_atomic_state_not_mutable_history() {
    let mut registry = ObjectiveRegistry::new(synthetic_profile(1, 1));
    let flag = registry
        .create_flag(FlagState::OnMap {
            x: 10,
            y: 20,
            owner: Some(7),
        })
        .unwrap();
    registry
        .create_powerball(PowerballState::Waiting {
            respawn_at_tick: Some(50),
        })
        .unwrap();
    let replay = registry.replay();
    assert_eq!(replay.flags.len(), 1);
    assert_eq!(replay.powerballs.len(), 1);

    registry
        .transition_flag(
            flag.identity,
            flag.version,
            FlagState::Waiting {
                respawn_at_tick: Some(60),
            },
        )
        .unwrap();
    assert!(matches!(replay.flags[0].state, FlagState::OnMap { .. }));
    assert!(matches!(
        registry.replay().flags[0].state,
        FlagState::Waiting {
            respawn_at_tick: Some(60)
        }
    ));
}
