use super::*;
use crate::lvz::{
    LVZ_DISPLAY_DEATH, LVZ_DISPLAY_ENTER_ARENA, LVZ_DISPLAY_ENTER_ZONE, LVZ_DISPLAY_KILL,
    LVZ_DISPLAY_SERVER_CONTROLLED, LvzImageDefinition, LvzObjectSection, LvzSection,
    lvz_map_object_world_position, lvz_object_animation_frame,
};

fn test_network_player(
    player_id: u16,
    ship: u8,
    frequency: u16,
    position: Option<Vec2>,
) -> NetworkPlayer {
    let mut player = NetworkPlayer::from_roster(
        &PlayerRosterEntry::parse(&sample_player_enter_packet(player_id, b"pilot")).unwrap(),
    );
    player.ship = ship;
    player.frequency = frequency;
    player.position = position;
    player
}

fn test_attach_world() -> NetworkWorld {
    let mut world = NetworkWorld::default();
    world.install_test_viewer_player_id(Some(7));
    world
        .players
        .insert(7, test_network_player(7, 0, 3, Some(vec2(100.0, 100.0))));
    world
}

#[test]
fn attach_action_detaches_when_self_is_already_attached() {
    let mut world = test_attach_world();
    world.players.get_mut(&7).unwrap().attach_parent = 9;
    let tuning = ShipTuning::continuum_skirmish_default();
    let arena = ArenaTuning::continuum_skirmish_default();

    assert_eq!(
        attach_action_for_target(&world, None, 0, tuning.energy_max, tuning, &arena),
        Ok(AttachAction::Detach)
    );
}

#[test]
fn attach_action_drops_existing_children_before_attaching() {
    let mut world = test_attach_world();
    let mut child = test_network_player(8, 0, 3, Some(vec2(110.0, 100.0)));
    child.attach_parent = 7;
    world.players.insert(8, child);
    let tuning = ShipTuning::continuum_skirmish_default();
    let arena = ArenaTuning::continuum_skirmish_default();

    assert_eq!(
        attach_action_for_target(&world, None, 0, tuning.energy_max, tuning, &arena),
        Ok(AttachAction::DropAttached)
    );
}

#[test]
fn attached_children_grouping_matches_per_parent_visibility_scans() {
    let mut world = test_attach_world();
    world
        .players
        .insert(9, test_network_player(9, 1, 3, Some(vec2(120.0, 100.0))));
    world
        .players
        .insert(10, test_network_player(10, 1, 3, Some(vec2(400.0, 100.0))));
    for (child_id, parent_id, ship) in [(11_u16, 9_u16, 0_u8), (12, 9, 1), (14, 9, 0), (13, 10, 8)]
    {
        let mut child = test_network_player(child_id, ship, 3, Some(vec2(120.0, 100.0)));
        child.attach_parent = parent_id;
        world.players.insert(child_id, child);
    }
    // The viewer's own attached child stays out of every remote presentation
    // path, exactly as the per-parent scan already excludes it.
    world.players.get_mut(&7).unwrap().attach_parent = 10;

    let grouped = world.attached_children_by_parent();
    assert_eq!(
        grouped
            .values()
            .flatten()
            .map(|p| p.player_id)
            .collect::<Vec<_>>(),
        vec![11, 12, 14],
        "one pass must keep spectator ship 8 and the viewer child excluded"
    );
    for parent_id in [9_u16, 10, 11] {
        assert_eq!(
            grouped
                .get(&parent_id)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|player| player.player_id)
                .collect::<Vec<_>>(),
            world
                .visible_attached_players(parent_id)
                .map(|player| player.player_id)
                .collect::<Vec<_>>(),
            "grouped children must equal the per-parent scan for parent {parent_id}"
        );
    }
    assert_eq!(grouped.keys().copied().collect::<Vec<_>>(), vec![9]);
}

#[test]
fn attach_action_requires_full_energy_and_attach_bounty() {
    let mut world = test_attach_world();
    world
        .players
        .insert(9, test_network_player(9, 1, 3, Some(vec2(120.0, 100.0))));
    let mut arena = ArenaTuning::continuum_skirmish_default();
    arena.ships[0].attach_bounty = 5;
    arena.ships[1].turret_limit = 1;
    let tuning = arena.ship(0);

    assert_eq!(
        attach_action_for_target(&world, Some(9), 4, tuning.energy_max, tuning, &arena),
        Err("Bounty not high enough to attach.")
    );
    assert_eq!(
        attach_action_for_target(&world, Some(9), 5, tuning.energy_max - 1.0, tuning, &arena),
        Err("Not enough energy to attach.")
    );
}

#[test]
fn nearest_attach_target_picks_closest_eligible_teammate() {
    let mut world = test_attach_world();
    world
        .players
        .insert(9, test_network_player(9, 1, 3, Some(vec2(160.0, 100.0))));
    world
        .players
        .insert(10, test_network_player(10, 1, 3, Some(vec2(120.0, 100.0))));
    let mut arena = ArenaTuning::continuum_skirmish_default();
    arena.ships[1].turret_limit = 2;
    let tuning = arena.ship(0);

    let target = nearest_attach_target(
        &world,
        vec2(100.0, 100.0),
        0,
        tuning.energy_max,
        tuning,
        &arena,
    );

    assert_eq!(target, Some(10));
    assert_eq!(
        attach_action_for_target(&world, target, 0, tuning.energy_max, tuning, &arena),
        Ok(AttachAction::Attach(10))
    );
}

#[test]
fn nearest_spectate_target_uses_the_drawn_position() {
    let mut world = test_attach_world();
    let mut near = test_network_player(9, 1, 3, Some(vec2(500.0, 100.0)));
    near.directed_presentation = true;
    near.render_position = Some(vec2(105.0, 100.0));
    world.players.insert(9, near);
    world
        .players
        .insert(11, test_network_player(11, 2, 4, Some(vec2(120.0, 100.0))));
    assert_eq!(
        nearest_spectate_target(&world, vec2(100.0, 100.0), 3, false),
        Some(9)
    );
}

#[test]
fn nearest_spectate_target_respects_cloak_and_excludes_unpresented_or_attached_actors() {
    let mut world = test_attach_world();
    let mut cloaked = test_network_player(9, 1, 4, Some(vec2(105.0, 100.0)));
    cloaked.togglables = STATUS_CLOAK;
    world.players.insert(9, cloaked);
    let mut unpresented = test_network_player(10, 1, 4, Some(vec2(101.0, 100.0)));
    unpresented.directed_presentation = true;
    unpresented.render_position = None;
    world.players.insert(10, unpresented);
    let mut attached = test_network_player(12, 1, 4, Some(vec2(102.0, 100.0)));
    attached.attach_parent = 11;
    world.players.insert(12, attached);
    world
        .players
        .insert(11, test_network_player(11, 1, 4, Some(vec2(150.0, 100.0))));
    assert_eq!(
        nearest_spectate_target(&world, vec2(100.0, 100.0), 3, false),
        Some(11)
    );
    assert_eq!(
        nearest_spectate_target(&world, vec2(100.0, 100.0), 3, true),
        Some(9)
    );
}

#[test]
fn nearest_spectate_target_ignores_spectators_and_self() {
    let mut world = test_attach_world();
    world
        .players
        .insert(9, test_network_player(9, 1, 3, Some(vec2(200.0, 100.0))));
    world
        .players
        .insert(10, test_network_player(10, 8, 3, Some(vec2(110.0, 100.0))));
    world
        .players
        .insert(11, test_network_player(11, 2, 4, Some(vec2(130.0, 100.0))));
    world
        .players
        .insert(12, test_network_player(12, 2, 4, None));

    assert_eq!(
        nearest_spectate_target(&world, vec2(100.0, 100.0), 3, false),
        Some(11)
    );
}

#[test]
fn attach_action_rejects_opponents_spectators_and_full_turrets() {
    let mut world = test_attach_world();
    world
        .players
        .insert(9, test_network_player(9, 1, 4, Some(vec2(120.0, 100.0))));
    world
        .players
        .insert(10, test_network_player(10, 8, 3, Some(vec2(130.0, 100.0))));
    world
        .players
        .insert(11, test_network_player(11, 1, 3, Some(vec2(140.0, 100.0))));
    let mut child = test_network_player(12, 0, 3, Some(vec2(140.0, 100.0)));
    child.attach_parent = 11;
    world.players.insert(12, child);
    let mut arena = ArenaTuning::continuum_skirmish_default();
    arena.ships[1].turret_limit = 1;
    let tuning = arena.ship(0);

    assert_eq!(
        attach_action_for_target(&world, Some(9), 0, tuning.energy_max, tuning, &arena),
        Err("Must attach to somebody of same frequency.")
    );
    assert_eq!(
        attach_action_for_target(&world, Some(10), 0, tuning.energy_max, tuning, &arena),
        Err("Cannot attach to spectator.")
    );
    assert_eq!(
        attach_action_for_target(&world, Some(11), 0, tuning.energy_max, tuning, &arena),
        Err("Too many turrets already attached.")
    );
}

#[test]
fn network_world_tracks_lvz_toggle_and_modify_state() {
    let mut world = NetworkWorld::default();
    let always_object = LvzObjectDefinition {
        map_object: true,
        object_id: 10,
        x_map: 0,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 0,
        display_mode: LVZ_DISPLAY_SHOW_ALWAYS,
    };

    assert!(world.lvz.object_is_active(&always_object));

    world.apply_event(&ClientProtocolEvent::LvzToggled(LvzTogglePacket {
        toggles: vec![
            LvzObjectToggle {
                object_id: 10,
                enabled: true,
            },
            LvzObjectToggle {
                object_id: 11,
                enabled: true,
            },
            LvzObjectToggle {
                object_id: 10,
                enabled: false,
            },
        ],
    }));

    assert_eq!(world.lvz.enabled_objects, BTreeSet::from([11]));
    assert_eq!(world.lvz.disabled_objects, BTreeSet::from([10]));
    assert!(!world.lvz.object_is_active(&always_object));

    let modify = LvzModifyPacket {
        changed: LvzModificationFlags {
            xy: true,
            image: true,
            layer: false,
            time: false,
            mode: true,
        },
        map_object: false,
        object_id: 11,
        map_x: 160,
        map_y: 320,
        screen_x: 10,
        screen_y: 20,
        x_type: 3,
        y_type: 4,
        image_id: 5,
        layer: 6,
        display_time: 70,
        display_mode: 2,
    };
    world.apply_event(&ClientProtocolEvent::LvzModified(modify));

    assert_eq!(world.lvz.modified_objects.get(&11), Some(&modify));

    world.apply_event(&ClientProtocolEvent::PlayerIdAssigned(7));
    assert!(world.lvz.enabled_objects.is_empty());
    assert!(world.lvz.disabled_objects.is_empty());
    assert!(world.lvz.enabled_object_ages.is_empty());
    assert!(world.lvz.modified_objects.is_empty());
}

#[test]
fn network_lvz_state_expires_timed_toggles() {
    let mut state = NetworkLvzState::default();
    let timed_object = LvzObjectDefinition {
        map_object: true,
        object_id: 21,
        x_map: 0,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 25,
        display_mode: 5,
    };

    state.apply_toggle(&LvzTogglePacket {
        toggles: vec![LvzObjectToggle {
            object_id: 21,
            enabled: true,
        }],
    });
    assert!(state.object_is_active(&timed_object));

    state.update_with_objects(2.49, [&timed_object]);
    assert!(state.object_is_active(&timed_object));
    assert_eq!(state.enabled_objects, BTreeSet::from([21]));

    state.update_with_objects(0.01, [&timed_object]);
    assert!(!state.object_is_active(&timed_object));
    assert!(state.enabled_objects.is_empty());
    assert_eq!(state.disabled_objects, BTreeSet::from([21]));
}

#[test]
fn network_lvz_state_uses_modified_display_time_for_expiration() {
    let mut state = NetworkLvzState::default();
    let object = LvzObjectDefinition {
        map_object: true,
        object_id: 22,
        x_map: 0,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 0,
        display_mode: 5,
    };
    state.apply_toggle(&LvzTogglePacket {
        toggles: vec![LvzObjectToggle {
            object_id: 22,
            enabled: true,
        }],
    });
    state.apply_modify(LvzModifyPacket {
        changed: LvzModificationFlags {
            xy: false,
            image: false,
            layer: false,
            time: true,
            mode: false,
        },
        map_object: true,
        object_id: 22,
        map_x: 0,
        map_y: 0,
        screen_x: 0,
        screen_y: 0,
        x_type: 0,
        y_type: 0,
        image_id: 0,
        layer: 0,
        display_time: 10,
        display_mode: 5,
    });

    state.update_with_objects(1.0, [&object]);

    assert!(!state.object_is_active(&object));
    assert_eq!(lvz_display_time_seconds(10), 1.0);
}

#[test]
fn network_lvz_state_shows_enter_zone_and_arena_objects_temporarily() {
    let zone_object = LvzObjectDefinition {
        map_object: false,
        object_id: 31,
        x_map: 0,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 25,
        display_mode: LVZ_DISPLAY_ENTER_ZONE,
    };
    let arena_object = LvzObjectDefinition {
        object_id: 32,
        display_mode: LVZ_DISPLAY_ENTER_ARENA,
        ..zone_object.clone()
    };
    let persistent_zone_object = LvzObjectDefinition {
        object_id: 33,
        display_time: 0,
        ..zone_object.clone()
    };
    let mut state = NetworkLvzState::default();

    assert!(state.object_is_active(&zone_object));
    assert!(state.object_is_active(&arena_object));
    assert!(state.object_is_active(&persistent_zone_object));

    state.update_with_objects(2.49, [&zone_object, &arena_object, &persistent_zone_object]);
    assert!(state.object_is_active(&zone_object));
    assert!(state.object_is_active(&arena_object));

    state.update_with_objects(0.01, [&zone_object, &arena_object, &persistent_zone_object]);
    assert!(!state.object_is_active(&zone_object));
    assert!(!state.object_is_active(&arena_object));
    assert!(state.object_is_active(&persistent_zone_object));
}

#[test]
fn joined_arena_restarts_lvz_enter_arena_timing() {
    let mut world = NetworkWorld::default();
    let arena_object = LvzObjectDefinition {
        map_object: false,
        object_id: 34,
        x_map: 0,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 25,
        display_mode: LVZ_DISPLAY_ENTER_ARENA,
    };

    world.lvz.update_with_objects(2.5, [&arena_object]);
    assert!(!world.lvz.object_is_active(&arena_object));

    world.apply_event(&ClientProtocolEvent::JoinedArena(1));
    assert!(world.lvz.object_is_active(&arena_object));
}

#[test]
fn lvz_kill_and_death_display_modes_are_event_triggered() {
    let kill_object = LvzObjectDefinition {
        map_object: false,
        object_id: 35,
        x_map: 0,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 25,
        display_mode: LVZ_DISPLAY_KILL,
    };
    let death_object = LvzObjectDefinition {
        object_id: 36,
        display_mode: LVZ_DISPLAY_DEATH,
        ..kill_object.clone()
    };
    let mut state = NetworkLvzState::default();

    assert!(!state.object_is_active(&kill_object));
    assert!(!state.object_is_active(&death_object));

    state.trigger_kill();
    assert!(state.object_is_active(&kill_object));
    assert!(!state.object_is_active(&death_object));

    state.update_with_objects(2.5, [&kill_object, &death_object]);
    assert!(!state.object_is_active(&kill_object));

    state.trigger_death();
    assert!(state.object_is_active(&death_object));
    state.update_with_objects(2.49, [&kill_object, &death_object]);
    assert!(state.object_is_active(&death_object));
    state.update_with_objects(0.01, [&kill_object, &death_object]);
    assert!(!state.object_is_active(&death_object));
}

#[test]
fn lvz_object_definition_applies_server_modifications() {
    let object = LvzObjectDefinition {
        map_object: true,
        object_id: 44,
        x_map: 10,
        y_map: 20,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 1,
        layer: 2,
        display_time: 30,
        display_mode: LVZ_DISPLAY_SHOW_ALWAYS,
    };
    let modification = LvzModifyPacket {
        changed: LvzModificationFlags {
            xy: true,
            image: true,
            layer: true,
            time: true,
            mode: true,
        },
        map_object: false,
        object_id: 44,
        map_x: 100,
        map_y: 200,
        screen_x: -12,
        screen_y: 24,
        x_type: 3,
        y_type: 4,
        image_id: 5,
        layer: 6,
        display_time: 70,
        display_mode: 5,
    };

    assert_eq!(
        object.modified(Some(&modification)),
        LvzObjectDefinition {
            map_object: false,
            object_id: 44,
            x_map: 10,
            y_map: 20,
            x_screen: -12,
            y_screen: 24,
            x_type: 3,
            y_type: 4,
            image_number: 5,
            layer: 6,
            display_time: 70,
            display_mode: 5,
        }
    );
    assert_eq!(object.modified(None), object);
}

#[test]
fn lvz_render_cache_preserves_duplicate_object_ids() {
    let image = LvzImageDefinition {
        x_count: 1,
        y_count: 1,
        animation_time: 0,
        filename: "sprite.bmp".into(),
    };
    let first = LvzObjectDefinition {
        map_object: true,
        object_id: 77,
        x_map: 100,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 20,
        display_mode: 5,
    };
    let second = LvzObjectDefinition {
        x_map: 200,
        display_time: 50,
        ..first.clone()
    };
    let archive = LvzArchive::validated_for_test(vec![LvzSection {
        filename: "".into(),
        timestamp: 0,
        data: Arc::from([]),
        kind: LvzSectionKind::Objects(LvzObjectSection {
            version: LvzArchive::MAGIC_CLV1,
            objects: vec![first.clone(), second.clone()],
            images: vec![image],
        }),
    }])
    .unwrap();
    let mut cache = LvzRenderCache::default();

    cache.merge_archive(&archive).unwrap();

    assert_eq!(cache.objects.len(), 2);
    assert!(std::sync::Arc::ptr_eq(
        &cache.objects[0].images,
        &cache.objects[1].images
    ));
    assert_eq!(cache.objects[0].object, first);
    assert_eq!(cache.objects[1].object, second);

    let mut state = NetworkLvzState::default();
    state.apply_toggle(&LvzTogglePacket {
        toggles: vec![LvzObjectToggle {
            object_id: 77,
            enabled: true,
        }],
    });
    state.update(1.99, &cache);
    let first = state.effective_renderable_object(&cache.objects[0]);
    let second = state.effective_renderable_object(&cache.objects[1]);
    assert!(state.renderable_is_active(&cache.objects[0], &first));
    assert!(!state.renderable_is_active(&cache.objects[1], &second));

    state.update(0.01, &cache);
    let first = state.effective_renderable_object(&cache.objects[0]);
    let second = state.effective_renderable_object(&cache.objects[1]);
    assert!(!state.renderable_is_active(&cache.objects[0], &first));
    assert!(!state.renderable_is_active(&cache.objects[1], &second));
}

#[test]
fn lvz_modifications_apply_only_to_first_duplicate_object_id() {
    let image = LvzImageDefinition {
        x_count: 1,
        y_count: 1,
        animation_time: 0,
        filename: "sprite.bmp".into(),
    };
    let object = LvzObjectDefinition {
        map_object: true,
        object_id: 88,
        x_map: 100,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 0,
        display_mode: LVZ_DISPLAY_SHOW_ALWAYS,
    };
    let archive = LvzArchive::validated_for_test(vec![LvzSection {
        filename: "".into(),
        timestamp: 0,
        data: Arc::from([]),
        kind: LvzSectionKind::Objects(LvzObjectSection {
            version: LvzArchive::MAGIC_CLV1,
            objects: vec![object.clone(), object.clone()],
            images: vec![image],
        }),
    }])
    .unwrap();
    let mut cache = LvzRenderCache::default();
    cache.merge_archive(&archive).unwrap();
    let mut state = NetworkLvzState::default();
    state.apply_modify(LvzModifyPacket {
        changed: LvzModificationFlags {
            xy: true,
            image: false,
            layer: false,
            time: false,
            mode: false,
        },
        map_object: true,
        object_id: 88,
        map_x: 320,
        map_y: 640,
        screen_x: 0,
        screen_y: 0,
        x_type: 0,
        y_type: 0,
        image_id: 0,
        layer: 0,
        display_time: 0,
        display_mode: LVZ_DISPLAY_SHOW_ALWAYS,
    });

    assert_eq!(
        state.effective_renderable_object(&cache.objects[0]).x_map,
        320
    );
    assert_eq!(
        state.effective_renderable_object(&cache.objects[1]).x_map,
        100
    );
}

#[test]
fn lvz_active_check_uses_modified_display_mode() {
    let image = LvzImageDefinition {
        x_count: 1,
        y_count: 1,
        animation_time: 0,
        filename: "sprite.bmp".into(),
    };
    let object = LvzObjectDefinition {
        map_object: false,
        object_id: 89,
        x_map: 0,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 0,
        display_mode: 5,
    };
    let archive = LvzArchive::validated_for_test(vec![LvzSection {
        filename: "".into(),
        timestamp: 0,
        data: Arc::from([]),
        kind: LvzSectionKind::Objects(LvzObjectSection {
            version: LvzArchive::MAGIC_CLV1,
            objects: vec![object],
            images: vec![image],
        }),
    }])
    .unwrap();
    let mut cache = LvzRenderCache::default();
    cache.merge_archive(&archive).unwrap();
    let mut state = NetworkLvzState::default();
    let object = state.effective_renderable_object(&cache.objects[0]);
    assert!(!state.renderable_is_active(&cache.objects[0], &object));

    state.apply_modify(LvzModifyPacket {
        changed: LvzModificationFlags {
            xy: false,
            image: false,
            layer: false,
            time: false,
            mode: true,
        },
        map_object: false,
        object_id: 89,
        map_x: 0,
        map_y: 0,
        screen_x: 0,
        screen_y: 0,
        x_type: 0,
        y_type: 0,
        image_id: 0,
        layer: 0,
        display_time: 0,
        display_mode: LVZ_DISPLAY_SHOW_ALWAYS,
    });

    let object = state.effective_renderable_object(&cache.objects[0]);
    assert!(state.renderable_is_active(&cache.objects[0], &object));
}

#[test]
fn lvz_effective_object_borrows_the_merged_definition_until_a_modify_applies() {
    let image = LvzImageDefinition {
        x_count: 1,
        y_count: 1,
        animation_time: 0,
        filename: "sprite.bmp".into(),
    };
    let object = LvzObjectDefinition {
        map_object: true,
        object_id: 91,
        x_map: 120,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 0,
        display_mode: LVZ_DISPLAY_SHOW_ALWAYS,
    };
    let archive = LvzArchive::validated_for_test(vec![LvzSection {
        filename: "".into(),
        timestamp: 0,
        data: Arc::from([]),
        kind: LvzSectionKind::Objects(LvzObjectSection {
            version: LvzArchive::MAGIC_CLV1,
            objects: vec![object],
            images: vec![image],
        }),
    }])
    .unwrap();
    let mut cache = LvzRenderCache::default();
    cache.merge_archive(&archive).unwrap();
    let mut state = NetworkLvzState::default();

    // The eight LVZ passes call this for every object they visit every frame, so
    // the overwhelming case - nothing has modified the object - must hand back
    // the merged definition itself rather than a copy of it.
    let untouched = state.effective_renderable_object(&cache.objects[0]);
    assert!(matches!(untouched, std::borrow::Cow::Borrowed(_)));
    assert!(std::ptr::eq(&*untouched, &cache.objects[0].object));

    state.apply_modify(LvzModifyPacket {
        changed: LvzModificationFlags {
            xy: true,
            image: false,
            layer: false,
            time: false,
            mode: false,
        },
        map_object: true,
        object_id: 91,
        map_x: 480,
        map_y: 0,
        screen_x: 0,
        screen_y: 0,
        x_type: 0,
        y_type: 0,
        image_id: 0,
        layer: 0,
        display_time: 0,
        display_mode: LVZ_DISPLAY_SHOW_ALWAYS,
    });

    let moved = state.effective_renderable_object(&cache.objects[0]);
    assert!(matches!(moved, std::borrow::Cow::Owned(_)));
    assert_eq!(moved.x_map, 480);
}

#[test]
fn lvz_server_toggles_restart_animation_clock() {
    let image = LvzImageDefinition {
        x_count: 4,
        y_count: 1,
        animation_time: 100,
        filename: "sprite.bmp".into(),
    };
    let object = LvzObjectDefinition {
        map_object: false,
        object_id: 90,
        x_map: 0,
        y_map: 0,
        x_screen: 0,
        y_screen: 0,
        x_type: 0,
        y_type: 0,
        image_number: 0,
        layer: 0,
        display_time: 0,
        display_mode: 5,
    };
    let archive = LvzArchive::validated_for_test(vec![LvzSection {
        filename: "".into(),
        timestamp: 0,
        data: Arc::from([]),
        kind: LvzSectionKind::Objects(LvzObjectSection {
            version: LvzArchive::MAGIC_CLV1,
            objects: vec![object],
            images: vec![image],
        }),
    }])
    .unwrap();
    let mut cache = LvzRenderCache::default();
    cache.merge_archive(&archive).unwrap();
    let mut state = NetworkLvzState::default();

    assert_eq!(
        state.renderable_animation_time(&cache.objects[0], 123.0),
        123.0
    );

    state.apply_toggle(&LvzTogglePacket {
        toggles: vec![LvzObjectToggle {
            object_id: 90,
            enabled: true,
        }],
    });
    assert_eq!(
        state.renderable_animation_time(&cache.objects[0], 123.0),
        0.0
    );
    assert_eq!(lvz_animation_frame(0.0, 100, 4), 0);

    state.update(0.26, &cache);
    assert_eq!(
        state.renderable_animation_time(&cache.objects[0], 123.0),
        0.26
    );
    assert_eq!(lvz_animation_frame(0.26, 100, 4), 1);

    state.apply_toggle(&LvzTogglePacket {
        toggles: vec![LvzObjectToggle {
            object_id: 90,
            enabled: true,
        }],
    });
    assert_eq!(
        state.renderable_animation_time(&cache.objects[0], 123.0),
        0.0
    );
}

#[test]
fn lvz_screen_reference_points_cover_reference_game_named_anchors() {
    let screen = vec2(800.0, 600.0);

    assert_eq!(lvz_screen_reference_point_for_size(0, screen), Vec2::ZERO);
    assert_eq!(
        lvz_screen_reference_point_for_size(1, screen),
        vec2(400.0, 300.0)
    );
    assert_eq!(
        lvz_screen_reference_point_for_size(2, screen),
        vec2(800.0, 600.0)
    );
    assert_eq!(
        lvz_screen_reference_point_for_size(4, screen),
        vec2(800.0, 0.0)
    );
    assert_eq!(
        lvz_screen_reference_point_for_size(6, screen),
        vec2(400.0, 36.0)
    );
    assert_eq!(
        lvz_screen_reference_point_for_size(7, screen),
        vec2(16.0, 480.0)
    );
    assert_eq!(
        lvz_screen_reference_point_for_size(8, screen),
        vec2(656.0, 12.0)
    );
    assert_eq!(
        lvz_screen_reference_point_for_size(9, screen),
        vec2(656.0, 156.0)
    );
    assert_eq!(
        lvz_screen_reference_point_for_size(10, screen),
        vec2(776.0, 243.0)
    );
    assert_eq!(
        lvz_screen_reference_point_for_size(11, screen),
        vec2(0.0, 418.0)
    );
    assert_eq!(lvz_screen_reference_point_for_size(15, screen), Vec2::ZERO);
}

#[test]
fn lvz_static_or_single_frame_images_stay_on_first_frame() {
    assert_eq!(lvz_animation_frame(123.0, 0, 4), 0);
    assert_eq!(lvz_animation_frame(123.0, -1, 4), 0);
    assert_eq!(lvz_animation_frame(123.0, 25, 1), 0);
    assert_eq!(lvz_object_animation_frame(123.0, 0, 4), 0);
    assert_eq!(lvz_object_animation_frame(123.0, -1, 4), 0);
    assert_eq!(lvz_object_animation_frame(123.0, 25, 1), 0);
}

#[test]
fn lvz_object_animations_stop_on_last_frame_like_reference_game() {
    assert_eq!(lvz_animation_frame(1.25, 100, 4), 1);
    assert_eq!(lvz_object_animation_frame(0.00, 100, 4), 0);
    assert_eq!(lvz_object_animation_frame(0.24, 100, 4), 0);
    assert_eq!(lvz_object_animation_frame(0.25, 100, 4), 1);
    assert_eq!(lvz_object_animation_frame(0.50, 100, 4), 2);
    assert_eq!(lvz_object_animation_frame(0.75, 100, 4), 3);
    assert_eq!(lvz_object_animation_frame(1.00, 100, 4), 3);
    assert_eq!(lvz_object_animation_frame(123.0, 100, 4), 3);
}

#[test]
fn lvz_square_sheet_frame_size_matches_reference_game_column_split() {
    let frame_size = lvz_square_sheet_frame_size(160.0, FLAG_FRAMES_PER_TEAM);

    assert_eq!(frame_size, vec2(16.0, 16.0));
    assert_eq!(
        lvz_source_for_frame(frame_size, FLAG_TEAM_FRAME_START, FLAG_FRAMES_PER_TEAM),
        Rect::new(0.0, 16.0, 16.0, 16.0)
    );
}

#[test]
fn lvz_single_ship_frame_size_matches_reference_game_sheet_formula() {
    let frame_size = lvz_single_ship_frame_size(360.0, 144.0);

    assert_eq!(frame_size, vec2(36.0, 36.0));
    assert_eq!(
        lvz_source_for_frame(frame_size, 39, 10),
        Rect::new(324.0, 108.0, 36.0, 36.0)
    );
}

#[test]
fn lvz_map_objects_render_in_client_pixel_world_coordinates() {
    assert_eq!(
        lvz_map_object_world_position(9622, 7504),
        vec2(9622.0, 7504.0)
    );
}

#[test]
fn lvz_asset_stem_matching_ignores_paths_and_extensions() {
    assert!(lvz_asset_stem_matches("ship6.bm2", "ship6"));
    assert!(lvz_asset_stem_matches("folder/flag.png", "flag"));
    assert!(!lvz_asset_stem_matches("folder/ship7.bmp", "ship6"));
}

#[test]
fn lvz_graphic_overrides_ignore_object_only_images() {
    let object_images = BTreeSet::from(["effects/ship6.png".to_string()]);

    assert_eq!(lvz_graphic_override_stem("folder/ship6.bm2"), Some("ship6"));
    assert_eq!(
        lvz_graphic_override_stem_for_file("ship6.bm2", &object_images),
        Some("ship6")
    );
    assert_eq!(
        lvz_graphic_override_stem_for_file("effects/ship6.png", &object_images),
        None
    );
    assert_eq!(
        lvz_graphic_override_stem_for_file("effects/readme.txt", &object_images),
        None
    );
}

#[test]
fn lvz_layers_match_reference_game_order_and_clamp_topmost() {
    assert_eq!(lvz_layer(0), LVZ_LAYER_BELOW_ALL);
    assert_eq!(lvz_layer(1), LVZ_LAYER_AFTER_BACKGROUND);
    assert_eq!(lvz_layer(2), LVZ_LAYER_AFTER_TILES);
    assert_eq!(lvz_layer(3), LVZ_LAYER_AFTER_WEAPONS);
    assert_eq!(lvz_layer(4), LVZ_LAYER_AFTER_SHIPS);
    assert_eq!(lvz_layer(5), LVZ_LAYER_AFTER_GAUGES);
    assert_eq!(lvz_layer(6), LVZ_LAYER_AFTER_CHAT);
    assert_eq!(lvz_layer(7), LVZ_LAYER_TOPMOST);
    assert_eq!(lvz_layer(99), LVZ_LAYER_TOPMOST);

    assert!(lvz_layer_in_range(
        3,
        LVZ_LAYER_AFTER_TILES,
        LVZ_LAYER_AFTER_SHIPS
    ));
    assert!(!lvz_layer_in_range(
        5,
        LVZ_LAYER_AFTER_TILES,
        LVZ_LAYER_AFTER_SHIPS
    ));
    assert!(lvz_layer_in_range(
        99,
        LVZ_LAYER_AFTER_CHAT,
        LVZ_LAYER_TOPMOST
    ));
}

#[test]
fn network_world_applies_action_batches_and_trims_chat_history() {
    let mut world = NetworkWorld::default();
    let entry = PlayerRosterEntry::parse(&sample_player_enter_packet(9, b"remote")).unwrap();
    let mut actions = ClientProtocolActions::default();
    actions
        .events
        .push(ClientProtocolEvent::PlayerEntered(entry));
    for index in 0..NetworkWorld::CHAT_HISTORY_LIMIT + 3 {
        actions.events.push(ClientProtocolEvent::Chat(ChatPacket {
            chat_type: 2,
            sound: 0,
            sender_id: 9,
            message: format!("msg {index}"),
        }));
    }

    world.apply_actions(&actions);

    assert_eq!(world.chat.len(), NetworkWorld::CHAT_HISTORY_LIMIT);
    assert_eq!(
        world.chat.first().map(|entry| entry.message.as_str()),
        Some("msg 3")
    );
    assert_eq!(
        world.chat.last().map(|entry| entry.message.as_str()),
        Some("msg 66")
    );
}

#[test]
fn scoreboard_entries_include_network_players_and_skip_self() {
    let map = ArenaMap::fallback_skirmish();
    let bots = spawn_initial_bots(
        &map,
        map.spawn_position(),
        ArenaTuning::continuum_skirmish_default(),
    );
    let mut world = NetworkWorld::default();
    world.install_test_viewer_player_id(Some(7));
    world.players.insert(
        7,
        NetworkPlayer {
            player_id: 7,
            life_seq: Some(1),
            source_session_id: None,
            name: "self".to_string(),
            squad: String::new(),
            ship: 1,
            frequency: 0,
            position: Some(vec2(1.0, 1.0)),
            previous_position: None,
            render_position: Some(vec2(1.0, 1.0)),
            directed_presentation: false,
            directed_render_orientation: None,
            directed_render_time: None,
            directed_firing_heading: None,
            directed_presentation_history: std::collections::VecDeque::new(),
            interpolation_start_position: None,
            interpolation_target_position: None,
            interpolation_elapsed: 0.0,
            interpolation_duration: 0.0,
            interpolation_start_velocity: Vec2::ZERO,
            interpolation_sample_interval: 0.0,
            interp_mean_gap_seconds: 0.0,
            interp_jitter_seconds: 0.0,
            velocity: Vec2::ZERO,
            orientation: 0.0,
            previous_authoritative_orientation: None,
            previous_orientation: None,
            orientation_interpolation_elapsed: 0.0,
            orientation_interpolation_duration: 0.0,
            bounty: 1,
            kill_points: 0,
            assists: 0,
            flag_points: 0,
            wins: 5,
            losses: 2,
            attach_parent: INVALID_PLAYER_ID,
            flags: 0,
            flag_timer: None,
            banner: None,
            ping: 0,
            energy: Some(100.0),
            togglables: 0,
            weapon: 0,
            presentation_snap_count: 0,
            presentation_initial_count: 0,
            last_presentation_snap_reason: 0,
            last_server_position_tick: None,
            last_seen_snapshot_id: None,
            snapshot_position_owned: false,
            snapshot_motion_segment: None,
            snapshot_relocation_generation: None,
        },
    );
    world.players.insert(
        9,
        NetworkPlayer {
            player_id: 9,
            life_seq: Some(1),
            source_session_id: None,
            name: "remote".to_string(),
            squad: String::new(),
            ship: 2,
            frequency: 0,
            position: Some(vec2(2.0, 2.0)),
            previous_position: None,
            render_position: Some(vec2(2.0, 2.0)),
            directed_presentation: false,
            directed_render_orientation: None,
            directed_render_time: None,
            directed_firing_heading: None,
            directed_presentation_history: std::collections::VecDeque::new(),
            interpolation_start_position: None,
            interpolation_target_position: None,
            interpolation_elapsed: 0.0,
            interpolation_duration: 0.0,
            interpolation_start_velocity: Vec2::ZERO,
            interpolation_sample_interval: 0.0,
            interp_mean_gap_seconds: 0.0,
            interp_jitter_seconds: 0.0,
            velocity: Vec2::ZERO,
            orientation: 0.0,
            previous_authoritative_orientation: None,
            previous_orientation: None,
            orientation_interpolation_elapsed: 0.0,
            orientation_interpolation_duration: 0.0,
            bounty: 6,
            kill_points: 610,
            assists: 40,
            flag_points: 20,
            wins: 4,
            losses: 3,
            attach_parent: INVALID_PLAYER_ID,
            flags: 2,
            flag_timer: None,
            banner: Some({
                let mut banner = [0; PLAYER_BANNER_BYTES];
                banner[0] = 11;
                banner
            }),
            ping: 0,
            energy: Some(750.0),
            togglables: 0,
            weapon: 0,
            presentation_snap_count: 0,
            presentation_initial_count: 0,
            last_presentation_snap_reason: 0,
            last_server_position_tick: None,
            last_seen_snapshot_id: None,
            snapshot_position_owned: false,
            snapshot_motion_segment: None,
            snapshot_relocation_generation: None,
        },
    );

    world.personal_banner = Some({
        let mut banner = [0; PLAYER_BANNER_BYTES];
        banner[0] = 5;
        banner
    });

    let rows = scoreboard_entries("pilot", 0, 0, 0, 0, 0, 0, 1000.0, &bots, &world);

    let local = rows
        .iter()
        .find(|row| row.local)
        .expect("local scoreboard row should exist");
    assert_eq!(local.name, "self");
    assert_eq!(local.kills, 5);
    assert_eq!(local.assists, 0);
    assert_eq!(local.deaths, 2);
    assert_eq!(local.banner.map(|banner| banner[0]), Some(5));
    let remote = rows
        .iter()
        .find(|row| row.name == "remote")
        .expect("remote scoreboard row should exist");
    assert_eq!(remote.ship_type, 2);
    assert_eq!(remote.kills, 4);
    assert_eq!(remote.assists, 40);
    assert_eq!(remote.total_score(), 630);
    assert_eq!(remote.average_bounty(), 152.5);
    assert_eq!(remote.rating(), 59);
    assert_eq!(remote.deaths, 3);
    assert_eq!(remote.bounty, 6);
    assert_eq!(remote.flag_goals, 20);
    assert_eq!(remote.energy, 750.0);
    assert_eq!(remote.banner.map(|banner| banner[0]), Some(11));
    assert!(remote.active);
}

#[test]
fn scoreboard_rows_group_players_by_frequency_and_team_scores() {
    let mut world = NetworkWorld::default();
    world.install_test_viewer_player_id(Some(7));
    world.soccer_scores.insert(0, 3);
    world.players.insert(
        7,
        NetworkPlayer {
            player_id: 7,
            life_seq: Some(1),
            source_session_id: None,
            name: "self".to_string(),
            squad: String::new(),
            ship: 1,
            frequency: 0,
            position: Some(vec2(1.0, 1.0)),
            previous_position: None,
            render_position: Some(vec2(1.0, 1.0)),
            directed_presentation: false,
            directed_render_orientation: None,
            directed_render_time: None,
            directed_firing_heading: None,
            directed_presentation_history: std::collections::VecDeque::new(),
            interpolation_start_position: None,
            interpolation_target_position: None,
            interpolation_elapsed: 0.0,
            interpolation_duration: 0.0,
            interpolation_start_velocity: Vec2::ZERO,
            interpolation_sample_interval: 0.0,
            interp_mean_gap_seconds: 0.0,
            interp_jitter_seconds: 0.0,
            velocity: Vec2::ZERO,
            orientation: 0.0,
            previous_authoritative_orientation: None,
            previous_orientation: None,
            orientation_interpolation_elapsed: 0.0,
            orientation_interpolation_duration: 0.0,
            bounty: 1,
            kill_points: 0,
            assists: 0,
            flag_points: 11,
            wins: 5,
            losses: 2,
            attach_parent: INVALID_PLAYER_ID,
            flags: 0,
            flag_timer: None,
            banner: None,
            ping: 0,
            energy: Some(100.0),
            togglables: 0,
            weapon: 0,
            presentation_snap_count: 0,
            presentation_initial_count: 0,
            last_presentation_snap_reason: 0,
            last_server_position_tick: None,
            last_seen_snapshot_id: None,
            snapshot_position_owned: false,
            snapshot_motion_segment: None,
            snapshot_relocation_generation: None,
        },
    );
    world.players.insert(
        9,
        NetworkPlayer {
            player_id: 9,
            life_seq: Some(1),
            source_session_id: None,
            name: "remote".to_string(),
            squad: String::new(),
            ship: 2,
            frequency: 0,
            position: Some(vec2(2.0, 2.0)),
            previous_position: None,
            render_position: Some(vec2(2.0, 2.0)),
            directed_presentation: false,
            directed_render_orientation: None,
            directed_render_time: None,
            directed_firing_heading: None,
            directed_presentation_history: std::collections::VecDeque::new(),
            interpolation_start_position: None,
            interpolation_target_position: None,
            interpolation_elapsed: 0.0,
            interpolation_duration: 0.0,
            interpolation_start_velocity: Vec2::ZERO,
            interpolation_sample_interval: 0.0,
            interp_mean_gap_seconds: 0.0,
            interp_jitter_seconds: 0.0,
            velocity: Vec2::ZERO,
            orientation: 0.0,
            previous_authoritative_orientation: None,
            previous_orientation: None,
            orientation_interpolation_elapsed: 0.0,
            orientation_interpolation_duration: 0.0,
            bounty: 6,
            kill_points: 610,
            assists: 40,
            flag_points: 20,
            wins: 4,
            losses: 3,
            attach_parent: INVALID_PLAYER_ID,
            flags: 2,
            flag_timer: None,
            banner: None,
            ping: 0,
            energy: Some(750.0),
            togglables: 0,
            weapon: 0,
            presentation_snap_count: 0,
            presentation_initial_count: 0,
            last_presentation_snap_reason: 0,
            last_server_position_tick: None,
            last_seen_snapshot_id: None,
            snapshot_position_owned: false,
            snapshot_motion_segment: None,
            snapshot_relocation_generation: None,
        },
    );

    let rows = scoreboard_rows("pilot", 0, 0, 0, 0, 0, 0, 1000.0, &[], &world);

    assert!(matches!(
        rows.first(),
        Some(ScoreboardRow::Frequency(group))
            if group.frequency == Some(0)
                && group.players == 2
                && group.kills == 9
                && group.assists == 40
                && group.deaths == 5
                && group.flag_points == 31
                && group.ball_goals == 3
    ));
    assert!(matches!(
        rows.get(1),
        Some(ScoreboardRow::Player(row)) if row.name == "self" && row.local
    ));
}

#[test]
fn continuum_file_crc32_matches_standard_cached_file_checksum() {
    assert_eq!(continuum_file_crc32(b""), 0);
    assert_eq!(continuum_file_crc32(b"123456789"), 0xcbf4_3926);
    assert_eq!(continuum_file_crc32(b"abc"), 0x3524_41c2);
    assert_eq!(continuum_file_crc32(&[0, 1, 2, 3, 4, 5, 6, 7]), 0x88aa_689f);
}

#[test]
fn file_request_checksum_uses_reference_game_file_crc() {
    let data = b"downloaded map bytes";
    let request = FileRequestSpec {
        zone_id: None,
        filename: "arena.lvl".to_string(),
        index: 0,
        size: data.len() as u32,
        size_is_known: true,
        checksum: continuum_file_crc32(data),
        sha256: None,
        decompress: true,
    };

    assert!(request.matches_download_checksum(data));
    assert!(!request.matches_download_checksum(b"different bytes"));
}

#[test]
fn downloaded_file_parser_routes_lvl_and_lvz_by_extension() {
    let lvl_data = [pack_lvl_tile(12, 34, 1)].concat();
    match parse_downloaded_file("arena.LVL", &lvl_data).expect("downloaded lvl should parse") {
        DownloadedFile::Level(map) => assert_eq!(map.tile(12, 34), 1),
        DownloadedFile::Lvz(_) => panic!("lvl download should not parse as lvz"),
    }

    let mut lvz_data = Vec::new();
    push_u32(&mut lvz_data, LvzArchive::MAGIC_CONT);
    push_u32(&mut lvz_data, 0);
    match parse_downloaded_file("zone.LVZ", &lvz_data).expect("downloaded lvz should parse") {
        DownloadedFile::Lvz(archive) => assert!(archive.sections().is_empty()),
        DownloadedFile::Level(_) => panic!("lvz download should not parse as lvl"),
    }

    assert!(parse_downloaded_file("image.bm2", b"data").is_err());
    assert!(parse_downloaded_file("bad.lvz", b"bad").is_err());
}

#[test]
fn compressed_map_packet_skips_reference_game_header() {
    let mut packet = vec![0xaa; CompressedMapPacket::HEADER_SIZE];
    packet.extend_from_slice(b"compressed-or-plain-map-data");

    let parsed = CompressedMapPacket::parse(&packet).expect("packet should parse");
    assert_eq!(parsed.payload, b"compressed-or-plain-map-data");
    assert!(CompressedMapPacket::parse(&packet[..CompressedMapPacket::HEADER_SIZE - 1]).is_err());
}

#[test]
fn compressed_map_packet_decodes_plain_payload_when_not_decompressing() {
    let mut packet = vec![0xaa; CompressedMapPacket::HEADER_SIZE];
    packet.extend_from_slice(b"plain-lvz-payload");
    let parsed = CompressedMapPacket::parse(&packet).expect("packet should parse");
    let request = FileRequestSpec {
        zone_id: None,
        filename: "zone.lvz".to_string(),
        index: 1,
        size: b"plain-lvz-payload".len() as u32,
        size_is_known: true,
        checksum: 0,
        sha256: None,
        decompress: false,
    };

    assert_eq!(
        parsed
            .decode_for_request(&request)
            .expect("plain payload should pass through"),
        b"plain-lvz-payload"
    );

    let mismatched = FileRequestSpec {
        size: request.size + 1,
        ..request
    };
    assert!(parsed.decode_for_request(&mismatched).is_err());
}

#[test]
fn compressed_map_packet_inflates_zlib_payload_for_map_requests() {
    let original = b"downloaded zlib-compressed map bytes";
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(original)
        .expect("test zlib write should succeed");
    let compressed = encoder.finish().expect("test zlib finish should succeed");
    let mut packet = vec![0xaa; CompressedMapPacket::HEADER_SIZE];
    packet.extend_from_slice(&compressed);
    let parsed = CompressedMapPacket::parse(&packet).expect("packet should parse");
    let request = FileRequestSpec {
        zone_id: None,
        filename: "arena.lvl".to_string(),
        index: 0,
        size: compressed.len() as u32,
        size_is_known: true,
        checksum: continuum_file_crc32(original),
        sha256: None,
        decompress: true,
    };
    let decoded = parsed
        .decode_for_request(&request)
        .expect("compressed payload should inflate");

    assert_eq!(decoded, original);
    assert!(request.matches_download_checksum(&decoded));
    assert!(decompress_zlib_payload(b"not zlib data").is_err());
}

#[test]
fn vie_unknown_compressed_size_accepts_bounded_payload_but_continuum_remains_exact() {
    let original = b"legacy VIE map bytes";
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(original).expect("zlib input");
    let compressed = encoder.finish().expect("zlib output");

    let mut vie_info = vec![0; 20];
    vie_info[..9].copy_from_slice(b"arena.lvl");
    let vie_request = MapInformation::parse_payload(&vie_info, false)
        .expect("VIE map information")
        .file_request();
    assert!(!vie_request.size_is_known);
    assert_eq!(
        CompressedMapPacket {
            payload: &compressed
        }
        .decode_for_request(&vie_request)
        .expect("VIE's zero size means unknown, not empty"),
        original
    );
    assert!(
        CompressedMapPacket {
            payload: b"not zlib"
        }
        .decode_for_request(&vie_request)
        .is_err(),
        "unknown size must not weaken map decompression validation"
    );

    let mut continuum_info = vec![0; 24];
    continuum_info[..9].copy_from_slice(b"arena.lvl");
    let continuum_request = MapInformation::parse_payload(&continuum_info, true)
        .expect("Continuum map information")
        .file_request();
    assert!(continuum_request.size_is_known);
    assert!(
        CompressedMapPacket {
            payload: &compressed
        }
        .decode_for_request(&continuum_request)
        .is_err(),
        "Continuum's advertised zero is an exact size"
    );

    let oversized = vec![0; crate::zone_wire::MAX_TRANSFER_FILE_BYTES + 1];
    assert!(
        CompressedMapPacket {
            payload: &oversized
        }
        .decode_for_request(&vie_request)
        .is_err(),
        "VIE unknown-size payloads retain the encoded transfer ceiling"
    );
}

#[test]
fn parses_lvz_archive_file_and_object_sections() {
    let mut object_data = Vec::new();
    push_u32(&mut object_data, LvzArchive::MAGIC_CLV1);
    push_u32(&mut object_data, 1);
    push_u32(&mut object_data, 1);
    push_u16(&mut object_data, (42 << 1) | 1);
    push_u16(&mut object_data, 123);
    push_u16(&mut object_data, 456);
    push_u8(&mut object_data, 0);
    push_u8(&mut object_data, 4);
    push_u16(&mut object_data, (2 << 12) | 300);
    push_u16(&mut object_data, 2);
    push_u16(&mut object_data, 3);
    push_u16(&mut object_data, 75);
    push_test_c_string(&mut object_data, "sprite.bmp");

    let raw_file = minimal_test_bm2();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw_file).unwrap();
    let compressed_file = encoder.finish().unwrap();

    let mut archive_data = Vec::new();
    push_u32(&mut archive_data, LvzArchive::MAGIC_CONT);
    push_u32(&mut archive_data, 2);
    push_lvz_section(&mut archive_data, "", 0, &object_data, None);
    push_lvz_section(
        &mut archive_data,
        "ship1.bm2",
        1234,
        &raw_file,
        Some(&compressed_file),
    );

    let archive = LvzArchive::parse(&archive_data).expect("LVZ archive should parse");

    assert_eq!(archive.sections().len(), 2);
    let LvzSectionKind::Objects(objects) = &archive.sections()[0].kind else {
        panic!("first section should contain objects");
    };
    assert_eq!(objects.version, LvzArchive::MAGIC_CLV1);
    assert_eq!(objects.images.len(), 1);
    assert_eq!(objects.images[0].filename.as_ref(), "sprite.bmp");
    assert_eq!(objects.images[0].x_count, 2);
    assert_eq!(objects.images[0].y_count, 3);
    assert_eq!(objects.images[0].animation_time, 75);
    assert_eq!(objects.objects.len(), 1);
    assert_eq!(
        objects.objects[0],
        LvzObjectDefinition {
            map_object: true,
            object_id: 42,
            x_map: 123,
            y_map: 456,
            x_screen: 7,
            y_screen: 28,
            x_type: 11,
            y_type: 8,
            image_number: 0,
            layer: 4,
            display_time: 300,
            display_mode: 2,
        }
    );
    assert_eq!(archive.sections()[1].filename.as_ref(), "ship1.bm2");
    assert_eq!(archive.sections()[1].timestamp, 1234);
    assert_eq!(archive.sections()[1].data.as_ref(), raw_file);
    assert_eq!(archive.sections()[1].kind, LvzSectionKind::File);
}

#[test]
fn clv1_screen_objects_use_full_signed_coordinates_without_reference_bits() {
    let mut object_data = Vec::new();
    push_u32(&mut object_data, LvzArchive::MAGIC_CLV1);
    push_u32(&mut object_data, 1);
    push_u32(&mut object_data, 1);
    push_u16(&mut object_data, 42 << 1);
    push_u16(&mut object_data, (-194_i16) as u16);
    push_u16(&mut object_data, 5);
    push_u8(&mut object_data, 0);
    push_u8(&mut object_data, 7);
    push_u16(
        &mut object_data,
        (LVZ_DISPLAY_SERVER_CONTROLLED as u16) << 12,
    );
    push_u16(&mut object_data, 1);
    push_u16(&mut object_data, 1);
    push_u16(&mut object_data, 100);
    push_test_c_string(&mut object_data, "score.bmp");

    let mut archive_data = Vec::new();
    push_u32(&mut archive_data, LvzArchive::MAGIC_CONT);
    push_u32(&mut archive_data, 1);
    push_lvz_section(&mut archive_data, "", 0, &object_data, None);

    let archive = LvzArchive::parse(&archive_data).expect("LVZ archive should parse");
    let LvzSectionKind::Objects(objects) = &archive.sections()[0].kind else {
        panic!("first section should contain objects");
    };

    assert_eq!(objects.objects.len(), 1);
    assert_eq!(objects.objects[0].x_screen, -194);
    assert_eq!(objects.objects[0].y_screen, 5);
    assert_eq!(objects.objects[0].x_type, 0);
    assert_eq!(objects.objects[0].y_type, 0);
    assert_eq!(
        objects.objects[0].display_mode,
        LVZ_DISPLAY_SERVER_CONTROLLED
    );
}

#[test]
fn lvz_archive_rejects_bad_magic_and_truncated_sections() {
    assert!(LvzArchive::parse(b"bad").is_err());

    let mut truncated = Vec::new();
    push_u32(&mut truncated, LvzArchive::MAGIC_CONT);
    push_u32(&mut truncated, 1);
    push_u32(&mut truncated, LvzArchive::MAGIC_CONT);

    assert!(LvzArchive::parse(&truncated).is_err());
}

#[test]
fn lvz_archive_enforces_archive_wide_decoded_budget_before_section_decode() {
    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    let half = LvzArchive::MAX_ARCHIVE_DECODED_BYTES / 2;
    let half_data = vec![0_u8; half];
    let half_zlib = zlib(&half_data);
    let mut exact = Vec::new();
    push_u32(&mut exact, LvzArchive::MAGIC_CONT);
    push_u32(&mut exact, 2);
    push_lvz_section(&mut exact, "a.bin", 1, &half_data, Some(&half_zlib));
    push_lvz_section(&mut exact, "b.bin", 1, &half_data, Some(&half_zlib));
    let parsed = LvzArchive::parse(&exact).expect("exact aggregate decoded cap");
    assert_eq!(
        parsed.decoded_bytes(),
        LvzArchive::MAX_ARCHIVE_DECODED_BYTES
    );

    let one = [0_u8];
    let one_zlib = zlib(&one);
    let mut over = exact;
    over[4..8].copy_from_slice(&3_u32.to_le_bytes());
    push_lvz_section(&mut over, "c.bin", 1, &one, Some(&one_zlib));
    assert!(LvzArchive::parse(&over).is_err());

    let per_section = LvzArchive::MAX_ARCHIVE_DECODED_BYTES / 64 + 1;
    let section = vec![0_u8; per_section];
    let compressed = zlib(&section);
    let mut many_section_bomb = Vec::new();
    push_u32(&mut many_section_bomb, LvzArchive::MAGIC_CONT);
    push_u32(&mut many_section_bomb, 64);
    for index in 0..64 {
        push_lvz_section(
            &mut many_section_bomb,
            &format!("s{index}.bin"),
            1,
            &section,
            Some(&compressed),
        );
    }
    assert!(
        many_section_bomb.len() < crate::zone_wire::MAX_TRANSFER_FILE_BYTES,
        "compressed expansion fixture must itself pass the transfer-file cap"
    );
    assert!(LvzArchive::parse(&many_section_bomb).is_err());
}

#[test]
fn packet_chunker_reassembles_small_chunks_on_tail() {
    let mut chunker = PacketChunker::default();

    assert_eq!(
        chunker.push_small_chunk(&[0x00, 0x08, b'a', b'b']).unwrap(),
        None
    );
    assert_eq!(chunker.push_small_chunk(&[0x00, 0x08, b'c']).unwrap(), None);
    assert_eq!(
        chunker.push_small_chunk(&[0x00, 0x09, b'd', b'e']).unwrap(),
        Some(b"abcde".to_vec())
    );
    assert!(chunker.push_small_chunk(&[0x00, 0x09, b'z']).is_err());
    assert_eq!(chunker.small.len, 0);
    assert!(chunker.push_small_chunk(&[0x00, 0x0a]).is_err());
}

#[test]
fn packet_chunker_reassembles_huge_chunks_at_advertised_length() {
    let mut chunker = PacketChunker::default();
    let mut first = vec![0x00, 0x0a];
    first.extend_from_slice(&5_u32.to_le_bytes());
    first.extend_from_slice(b"ab");
    let mut second = vec![0x00, 0x0a];
    second.extend_from_slice(&5_u32.to_le_bytes());
    second.extend_from_slice(b"cde");

    assert_eq!(chunker.push_huge_chunk(&first).unwrap(), None);
    assert_eq!(
        chunker.push_huge_chunk(&second).unwrap(),
        Some(b"abcde".to_vec())
    );

    let mut extra = vec![0x00, 0x0a];
    extra.extend_from_slice(&2_u32.to_le_bytes());
    extra.extend_from_slice(b"xyz");
    assert!(chunker.push_huge_chunk(&extra).is_err());
    assert_eq!(chunker.huge.len, 0);
}

#[test]
fn packet_chunker_cancel_clears_huge_chunks_and_returns_ack() {
    let mut chunker = PacketChunker::default();
    let mut partial = vec![0x00, 0x0a];
    partial.extend_from_slice(&10_u32.to_le_bytes());
    partial.extend_from_slice(b"abc");
    assert_eq!(chunker.push_huge_chunk(&partial).unwrap(), None);

    assert_eq!(chunker.cancel_huge_chunk(), vec![0x00, 0x0c]);
    assert_eq!(chunker.huge.data.capacity(), 0);

    let mut complete = vec![0x00, 0x0a];
    complete.extend_from_slice(&2_u32.to_le_bytes());
    complete.extend_from_slice(b"de");
    assert_eq!(
        chunker.push_huge_chunk(&complete).unwrap(),
        Some(b"de".to_vec())
    );
}

#[test]
fn packet_chunker_enforces_exact_transfer_bound_and_cleans_failed_state() {
    let mut assembler = ChunkAssembler::default();
    assembler
        .push(&vec![0xaa; crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES])
        .expect("exact logical transfer maximum is admitted");
    assert_eq!(assembler.len, crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES);
    assert!(assembler.push(&[0xbb]).is_err());
    assert_eq!(assembler.len, 0);
    assert!(assembler.data.is_empty());
    assert_eq!(assembler.data.capacity(), 0);

    let mut chunker = PacketChunker::default();
    let mut oversized_target = vec![PacketChunker::CORE_BYTE, PacketChunker::HUGE_TYPE];
    oversized_target.extend_from_slice(
        &(crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES as u32 + 1).to_le_bytes(),
    );
    assert!(chunker.push_huge_chunk(&oversized_target).is_err());
    assert_eq!(chunker.huge.target_len, None);

    let mut first = vec![PacketChunker::CORE_BYTE, PacketChunker::HUGE_TYPE];
    first.extend_from_slice(&10_u32.to_le_bytes());
    first.extend_from_slice(b"abc");
    assert_eq!(chunker.push_huge_chunk(&first).unwrap(), None);
    let mut changed = vec![PacketChunker::CORE_BYTE, PacketChunker::HUGE_TYPE];
    changed.extend_from_slice(&9_u32.to_le_bytes());
    changed.extend_from_slice(b"def");
    assert!(chunker.push_huge_chunk(&changed).is_err());
    assert_eq!(chunker.huge.len, 0);
    assert_eq!(chunker.huge.target_len, None);
    assert_eq!(chunker.huge.data.capacity(), 0);
}

#[test]
fn packet_chunker_rejects_empty_huge_fragments_and_bounds_fragment_count() {
    let mut chunker = PacketChunker::default();
    let mut empty_huge = vec![PacketChunker::CORE_BYTE, PacketChunker::HUGE_TYPE];
    empty_huge.extend_from_slice(&10_u32.to_le_bytes());
    for _ in 0..3 {
        assert!(chunker.push_huge_chunk(&empty_huge).is_err());
        assert_eq!(chunker.huge.len, 0);
        assert_eq!(chunker.huge.fragments, 0);
        assert_eq!(chunker.huge.data.capacity(), 0);
        assert_eq!(chunker.huge.target_len, None);
    }

    let mut assembler = ChunkAssembler::default();
    for _ in 0..crate::zone_wire::MAX_TRANSFER_FRAGMENTS {
        assembler.push(b"x").expect("exact fragment cap");
    }
    assert_eq!(
        assembler.fragments,
        crate::zone_wire::MAX_TRANSFER_FRAGMENTS
    );
    assert!(assembler.push(b"x").is_err());
    assert_eq!(assembler.len, 0);
    assert_eq!(assembler.fragments, 0);

    let mut huge = PacketChunker::default();
    let target = (crate::zone_wire::MAX_TRANSFER_FRAGMENTS + 1) as u32;
    for _ in 0..crate::zone_wire::MAX_TRANSFER_FRAGMENTS {
        let mut fragment = vec![PacketChunker::CORE_BYTE, PacketChunker::HUGE_TYPE];
        fragment.extend_from_slice(&target.to_le_bytes());
        fragment.push(b'x');
        assert_eq!(huge.push_huge_chunk(&fragment).unwrap(), None);
    }
    let mut over = vec![PacketChunker::CORE_BYTE, PacketChunker::HUGE_TYPE];
    over.extend_from_slice(&target.to_le_bytes());
    over.push(b'x');
    assert!(huge.push_huge_chunk(&over).is_err());
    assert_eq!(huge.huge.len, 0);
    assert_eq!(huge.huge.fragments, 0);
}

#[test]
fn packet_chunker_discards_incomplete_assembly_at_next_logical_packet() {
    let mut chunker = PacketChunker::default();
    assert_eq!(
        chunker
            .push_small_chunk(&[
                PacketChunker::CORE_BYTE,
                PacketChunker::SMALL_BODY_TYPE,
                b'a'
            ])
            .unwrap(),
        None
    );
    let mut huge = vec![PacketChunker::CORE_BYTE, PacketChunker::HUGE_TYPE];
    huge.extend_from_slice(&5_u32.to_le_bytes());
    huge.extend_from_slice(b"ab");
    assert_eq!(chunker.push_huge_chunk(&huge).unwrap(), None);
    chunker.abort_incomplete_for_packet(&[ServerPacket::JOIN_GAME]);
    assert_eq!(chunker.small.len, 0);
    assert_eq!(chunker.huge.len, 0);
    assert_eq!(chunker.huge.target_len, None);
    assert_eq!(chunker.small.data.capacity(), 0);
    assert_eq!(chunker.huge.data.capacity(), 0);
    assert!(
        chunker
            .push_small_chunk(&[
                PacketChunker::CORE_BYTE,
                PacketChunker::SMALL_TAIL_TYPE,
                b'z'
            ])
            .is_err()
    );
}

#[test]
fn packet_chunker_moves_exact_limit_completions_and_releases_internal_capacity() {
    let maximum = crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES;
    let mut small = PacketChunker::default();
    let mut body = vec![PacketChunker::CORE_BYTE, PacketChunker::SMALL_BODY_TYPE];
    body.resize(body.len() + maximum - 1, 0x5a);
    assert!(small.push_small_chunk(&body).unwrap().is_none());
    let completed = small
        .push_small_chunk(&[
            PacketChunker::CORE_BYTE,
            PacketChunker::SMALL_TAIL_TYPE,
            0x5b,
        ])
        .unwrap()
        .unwrap();
    assert_eq!(completed.len(), maximum);
    assert_eq!(small.small.data.capacity(), 0);

    let mut huge = PacketChunker::default();
    let mut packet = vec![PacketChunker::CORE_BYTE, PacketChunker::HUGE_TYPE];
    packet.extend_from_slice(&(maximum as u32).to_le_bytes());
    packet.resize(packet.len() + maximum, 0x6a);
    let completed = huge.push_huge_chunk(&packet).unwrap().unwrap();
    assert_eq!(completed.len(), maximum);
    assert_eq!(huge.huge.data.capacity(), 0);
}

#[test]
fn packet_cluster_splits_length_prefixed_packets() {
    let cluster = [0x00, 0x0e, 3, 0x00, 0x04, 0xaa, 2, 0x01, 0xbb];
    let packets = PacketChunker::split_packet_cluster(&cluster).expect("cluster should parse");

    assert_eq!(packets, vec![&[0x00, 0x04, 0xaa][..], &[0x01, 0xbb][..]]);
    assert!(PacketChunker::split_packet_cluster(&cluster[..cluster.len() - 1]).is_err());
    assert!(PacketChunker::split_packet_cluster(&[0x00, 0x03]).is_err());
}

#[test]
fn reliable_sequencer_encodes_outbound_messages_and_acks() {
    let mut sequencer = ReliableSequencer::default();

    assert_eq!(
        sequencer.send_reliable_at_tick(b"hello", 10),
        vec![0x00, 0x03, 0, 0, 0, 0, b'h', b'e', b'l', b'l', b'o']
    );
    assert_eq!(
        sequencer.send_reliable_at_tick(b"!", 20),
        vec![0x00, 0x03, 1, 0, 0, 0, b'!']
    );
    assert_eq!(
        sequencer.sent,
        vec![
            ReliableSentPacket {
                id: 0,
                payload: b"hello".to_vec(),
                last_sent_tick: 10,
            },
            ReliableSentPacket {
                id: 1,
                payload: b"!".to_vec(),
                last_sent_tick: 20,
            },
        ]
    );

    assert!(
        sequencer
            .receive_ack(&[0x00, 0x04, 0, 0, 0, 0])
            .expect("ack should parse")
    );
    assert!(
        !sequencer
            .receive_ack(&[0x00, 0x04, 9, 0, 0, 0])
            .expect("ack should parse")
    );
    assert_eq!(sequencer.sent.len(), 1);
    assert!(sequencer.receive_ack(&[0x00, 0x03, 0, 0, 0, 0]).is_err());
}

#[test]
fn reliable_sequencer_resends_timed_out_messages_like_reference_game() {
    let mut sequencer = ReliableSequencer::default();
    sequencer.send_reliable_at_tick(b"old", 10);
    sequencer.send_reliable_at_tick(b"fresh", 250);

    assert!(sequencer.encode_timed_out_reliable_at_tick(309).is_empty());
    assert_eq!(
        sequencer.encode_timed_out_reliable_at_tick(310),
        vec![ReliableSequencer::encode_reliable_message(0, b"old")]
    );
    assert_eq!(sequencer.sent[0].last_sent_tick, 310);
    assert_eq!(
        sequencer.encode_timed_out_reliable_at_tick(550),
        vec![ReliableSequencer::encode_reliable_message(1, b"fresh")]
    );
    assert!(
        sequencer
            .receive_ack(&ReliableSequencer::encode_ack(0))
            .expect("ack should parse")
    );
    assert_eq!(
        sequencer.encode_timed_out_reliable_at_tick(610),
        Vec::<Vec<u8>>::new()
    );
}

#[test]
fn reliable_sequencer_caps_tracked_sent_messages_like_reference_game() {
    let mut sequencer = ReliableSequencer::default();
    let mut last_packet = Vec::new();
    for id in 0..300 {
        last_packet = sequencer.send_reliable_at_tick(b"x", id);
    }

    assert_eq!(sequencer.next_outbound_id, 300);
    assert_eq!(sequencer.sent.len(), ReliableSequencer::MAX_SENT_RELIABLES);
    assert_eq!(sequencer.sent.first().map(|packet| packet.id), Some(0));
    assert_eq!(
        sequencer.sent.last().map(|packet| packet.id),
        Some(ReliableSequencer::MAX_SENT_RELIABLES as u32 - 1)
    );
    assert_eq!(
        last_packet,
        ReliableSequencer::encode_reliable_message(299, b"x")
    );

    assert!(
        !sequencer
            .receive_ack(&ReliableSequencer::encode_ack(299))
            .expect("ack should parse")
    );
    assert!(
        sequencer
            .receive_ack(&ReliableSequencer::encode_ack(255))
            .expect("ack should parse")
    );
    assert_eq!(
        sequencer.sent.len(),
        ReliableSequencer::MAX_SENT_RELIABLES - 1
    );
}

#[test]
fn lifecycle_reliable_waits_at_256_capacity_and_requires_its_exact_ack_across_wrap() {
    let mut sequencer = ReliableSequencer::default();
    sequencer.next_outbound_id = u32::MAX - 127;
    for _ in 0..ReliableSequencer::MAX_SENT_RELIABLES {
        assert!(
            sequencer
                .try_send_tracked_reliable_at_tick(b"ordinary", 10)
                .is_some()
        );
    }
    let lifecycle_id = sequencer.next_outbound_id;
    let before = sequencer.sent.clone();
    assert_eq!(
        sequencer.try_send_tracked_reliable_at_tick(b"lifecycle-baseline", 11),
        None,
        "a saturated table must retain intent without emitting an untracked id"
    );
    assert_eq!(sequencer.next_outbound_id, lifecycle_id);
    assert_eq!(sequencer.sent, before);

    let freed_id = before[73].id;
    assert!(
        sequencer
            .receive_ack(&ReliableSequencer::encode_ack(freed_id))
            .unwrap()
    );
    let (reserved_id, packet) = sequencer
        .try_send_tracked_reliable_at_tick(b"lifecycle-baseline", 12)
        .expect("one exact tracked slot became available");
    assert_eq!(reserved_id, lifecycle_id);
    assert_eq!(
        packet,
        ReliableSequencer::encode_reliable_message(lifecycle_id, b"lifecycle-baseline")
    );

    // Even after every earlier wrapped id is acknowledged, the barrier does
    // not release until the lifecycle packet's own ACK arrives.
    for sent in before.iter().filter(|sent| sent.id != freed_id) {
        assert!(
            sequencer
                .receive_ack(&ReliableSequencer::encode_ack(sent.id))
                .unwrap()
        );
    }
    assert!(!sequencer.outbound_prefix_acked_through(lifecycle_id));
    assert!(
        !sequencer
            .receive_ack(&ReliableSequencer::encode_ack(lifecycle_id.wrapping_add(1)))
            .unwrap()
    );
    assert!(!sequencer.outbound_prefix_acked_through(lifecycle_id));
    assert!(
        sequencer
            .receive_ack(&ReliableSequencer::encode_ack(lifecycle_id))
            .unwrap()
    );
    assert!(sequencer.outbound_prefix_acked_through(lifecycle_id));
}

#[test]
fn reliable_sequencer_drains_incoming_reliable_messages_in_order() {
    let mut sequencer = ReliableSequencer::default();
    let packet_two = ReliableSequencer::encode_reliable_message(2, b"two");
    let packet_zero = ReliableSequencer::encode_reliable_message(0, b"zero");
    let packet_one = ReliableSequencer::encode_reliable_message(1, b"one");

    sequencer
        .receive_reliable(&packet_two)
        .expect("packet 2 should parse");
    assert!(sequencer.drain_ordered().is_empty());
    sequencer
        .receive_reliable(&packet_zero)
        .expect("packet 0 should parse");
    assert_eq!(sequencer.drain_ordered(), vec![b"zero".to_vec()]);
    sequencer
        .receive_reliable(&packet_one)
        .expect("packet 1 should parse");
    assert_eq!(
        sequencer.drain_ordered(),
        vec![b"one".to_vec(), b"two".to_vec()]
    );

    sequencer
        .receive_reliable(&packet_one)
        .expect("old duplicate should still ack");
    assert!(sequencer.drain_ordered().is_empty());
    assert_eq!(sequencer.outbound_acks, vec![2, 0, 1, 1]);
    assert!(sequencer.receive_reliable(&[0x00, 0x03]).is_err());
}

#[test]
fn reliable_sequencer_encodes_single_and_clustered_outbound_acks() {
    let mut sequencer = ReliableSequencer::default();
    sequencer
        .receive_reliable(&ReliableSequencer::encode_reliable_message(7, b"a"))
        .expect("packet should parse");
    assert_eq!(
        sequencer.encode_outbound_acks(),
        vec![vec![0x00, 0x04, 7, 0, 0, 0]]
    );
    assert!(sequencer.encode_outbound_acks().is_empty());

    sequencer
        .receive_reliable(&ReliableSequencer::encode_reliable_message(9, b"b"))
        .expect("packet should parse");
    sequencer
        .receive_reliable(&ReliableSequencer::encode_reliable_message(10, b"c"))
        .expect("packet should parse");
    assert_eq!(
        sequencer.encode_outbound_acks(),
        vec![vec![
            0x00, 0x0e, 6, 0x00, 0x04, 9, 0, 0, 0, 6, 0x00, 0x04, 10, 0, 0, 0
        ]]
    );
}

#[test]
fn reliable_sequencer_never_drops_acks_beyond_the_flush_threshold() {
    // [REVIEW M8] A burst larger than MAX_OUTBOUND_ACKS (256) between
    // flushes must not silently drop acks — each dropped ack costs the
    // peer a 300-tick resend of a packet we already hold. The threshold
    // is a flush bound, not a drop-cap: every ack is recorded and the
    // encoder chunks the backlog into 520-byte-bounded clusters.
    let mut sequencer = ReliableSequencer::default();
    let burst = ReliableSequencer::MAX_OUTBOUND_ACKS + 44; // 300
    for id in 0..burst as u32 {
        sequencer
            .receive_reliable(&ReliableSequencer::encode_reliable_message(id, b"x"))
            .expect("packet should parse");
        assert_eq!(sequencer.drain_ordered(), vec![b"x".to_vec()]);
    }
    assert_eq!(
        sequencer.outbound_acks.len(),
        burst,
        "every received reliable must be recorded for ack"
    );

    let packets = sequencer.encode_outbound_acks();
    let mut acked = 0usize;
    for packet in &packets {
        assert!(packet.len() <= ReliableSequencer::MAX_PACKET_SIZE);
        if packet[1] == 0x04 {
            acked += 1;
            continue;
        }
        assert_eq!(packet[0..2], [0x00, 0x0e], "multi-ack packets are clusters");
        let mut offset = 2;
        while offset < packet.len() {
            assert_eq!(packet[offset] as usize, ReliableSequencer::ACK_SIZE);
            offset += 1 + ReliableSequencer::ACK_SIZE;
            acked += 1;
        }
    }
    assert_eq!(acked, burst, "all acks must reach the wire");
    assert!(sequencer.outbound_acks.is_empty());
}

#[test]
fn reliable_sequencer_splits_ack_clusters_at_reference_game_packet_size() {
    let mut sequencer = ReliableSequencer::default();
    for id in 0..75 {
        sequencer
            .receive_reliable(&ReliableSequencer::encode_reliable_message(id, b"x"))
            .expect("packet should parse");
    }

    let packets = sequencer.encode_outbound_acks();
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].len(), ReliableSequencer::MAX_PACKET_SIZE);
    assert_eq!(packets[0][0..2], [0x00, 0x0e]);
    assert_eq!(packets[0][2], ReliableSequencer::ACK_SIZE as u8);
    assert_eq!(
        &packets[0][3..3 + ReliableSequencer::ACK_SIZE],
        ReliableSequencer::encode_ack(0).as_slice()
    );
    assert_eq!(
        &packets[0][packets[0].len() - ReliableSequencer::ACK_SIZE..],
        ReliableSequencer::encode_ack(73).as_slice()
    );
    assert_eq!(packets[1], ReliableSequencer::encode_ack(74));
    assert!(sequencer.encode_outbound_acks().is_empty());
}

#[test]
fn reliable_sequencer_acks_every_retained_message_beyond_the_reference_game_set_size() {
    // [REVIEW M8] Previously this pinned the reference game's drop-at-256 outbound
    // ack-set behavior; zubspace deliberately diverges (a robustness
    // extension like the wrap-safe id compare): MAX_OUTBOUND_ACKS is a
    // flush threshold, and no ack is ever silently dropped — the encoder
    // chunks the whole backlog into 520-byte-bounded clusters.
    let mut sequencer = ReliableSequencer::default();
    for id in 0..300 {
        sequencer
            .receive_reliable(&ReliableSequencer::encode_reliable_message(id, b"x"))
            .expect("packet should parse");
        assert_eq!(sequencer.drain_ordered(), vec![b"x".to_vec()]);
    }

    let packets = sequencer.encode_outbound_acks();
    let mut acks = Vec::new();
    for packet in packets {
        if packet == ReliableSequencer::encode_ack(0) {
            acks.push(0);
            continue;
        }

        let entries = PacketChunker::split_packet_cluster(&packet).expect("cluster should parse");
        for entry in entries {
            assert_eq!(entry.len(), ReliableSequencer::ACK_SIZE);
            assert_eq!(entry[0..2], [0x00, 0x04]);
            acks.push(u32::from_le_bytes(
                entry[2..6].try_into().expect("ack id should fit"),
            ));
        }
    }

    assert_eq!(acks.len(), 300, "every received reliable is acked");
    assert_eq!(acks.first().copied(), Some(0));
    assert_eq!(acks.last().copied(), Some(299));
    assert!(sequencer.encode_outbound_acks().is_empty());
}

#[test]
fn reliable_sequencer_wraps_process_id_like_reference_game() {
    let mut sequencer = ReliableSequencer {
        next_process_id: u32::MAX,
        ..ReliableSequencer::default()
    };

    sequencer
        .receive_reliable(&ReliableSequencer::encode_reliable_message(0, b"early"))
        .expect("early wrapped packet should parse");
    assert!(sequencer.drain_ordered().is_empty());
    assert_eq!(sequencer.outbound_acks, vec![0]);
    assert!(sequencer.queued.contains_key(&0));

    sequencer
        .receive_reliable(&ReliableSequencer::encode_reliable_message(
            u32::MAX,
            b"last",
        ))
        .expect("last pre-wrap packet should parse");
    assert_eq!(
        sequencer.drain_ordered(),
        vec![b"last".to_vec(), b"early".to_vec()]
    );
    assert_eq!(sequencer.next_process_id, 1);
}

#[test]
fn reliable_sequencer_withholds_ack_when_full_then_accepts_retry_after_gap() {
    let mut sequencer = ReliableSequencer::default();
    for id in 1..=300 {
        sequencer
            .receive_reliable(&ReliableSequencer::encode_reliable_message(id, b"x"))
            .expect("packet should parse");
    }

    assert_eq!(
        sequencer.queued.len(),
        ReliableSequencer::MAX_QUEUED_RELIABLES
    );
    assert!(sequencer.queued.contains_key(&1));
    assert!(
        sequencer
            .queued
            .contains_key(&(ReliableSequencer::MAX_QUEUED_RELIABLES as u32))
    );
    assert!(
        !sequencer
            .queued
            .contains_key(&(ReliableSequencer::MAX_QUEUED_RELIABLES as u32 + 1))
    );

    let ack_packets = sequencer.encode_outbound_acks();
    let ack_count = ack_packets
        .iter()
        .flat_map(|packet| {
            if packet.len() == ReliableSequencer::ACK_SIZE {
                vec![packet.as_slice()]
            } else {
                PacketChunker::split_packet_cluster(packet).expect("cluster should parse")
            }
        })
        .count();
    assert_eq!(
        ack_count,
        ReliableSequencer::MAX_QUEUED_RELIABLES,
        "only payloads retained for later ordered dispatch may be ACKed"
    );

    sequencer
        .receive_reliable(&ReliableSequencer::encode_reliable_message(0, b"zero"))
        .expect("expected packet should parse");
    let packets = sequencer.drain_ordered();
    assert_eq!(packets.len(), 257);
    assert_eq!(packets.first(), Some(&b"zero".to_vec()));
    assert_eq!(packets.last(), Some(&b"x".to_vec()));
    assert_eq!(sequencer.next_process_id, 257);
    assert_eq!(
        sequencer.encode_outbound_acks(),
        vec![ReliableSequencer::encode_ack(0)]
    );

    sequencer
        .receive_reliable(&ReliableSequencer::encode_reliable_message(257, b"retried"))
        .expect("unacknowledged overflow should be retried");
    assert_eq!(
        sequencer.encode_outbound_acks(),
        vec![ReliableSequencer::encode_ack(257)]
    );
    assert_eq!(sequencer.drain_ordered(), vec![b"retried".to_vec()]);
    assert_eq!(sequencer.next_process_id, 258);
}

#[test]
fn chat_input_filters_control_chars_and_caps_length() {
    let mut input = String::new();
    push_chat_char(&mut input, 'o');
    push_chat_char(&mut input, '\n');
    push_chat_char(&mut input, 'k');

    assert_eq!(input, "ok");

    for _ in 0..CHAT_MAX_CHARS + 10 {
        push_chat_char(&mut input, 'x');
    }

    assert_eq!(input.chars().count(), CHAT_MAX_CHARS);
}

#[test]
fn chat_messages_trim_to_history_limit() {
    let mut messages = Vec::new();
    for index in 0..CHAT_HISTORY_LIMIT + 3 {
        push_chat_message(&mut messages, format!("msg {index}"), CHAT_HISTORY_LIMIT);
    }

    assert_eq!(messages.len(), CHAT_HISTORY_LIMIT);
    assert_eq!(messages.first().map(String::as_str), Some("msg 3"));
    assert_eq!(
        messages.last(),
        Some(&format!("msg {}", CHAT_HISTORY_LIMIT + 2))
    );
}

#[test]
fn chat_state_recalls_sent_messages_with_arrow_history() {
    let mut chat = ChatState::default();
    chat.begin_input();
    chat.input = "first".to_string();
    assert_eq!(chat.submit_input().as_deref(), Some("first"));

    chat.begin_input();
    chat.input = "second".to_string();
    assert_eq!(chat.submit_input().as_deref(), Some("second"));

    chat.begin_input();
    chat.recall_previous();
    assert_eq!(chat.input, "second");
    chat.recall_previous();
    assert_eq!(chat.input, "first");
    chat.recall_previous();
    assert_eq!(chat.input, "first");
    chat.recall_next();
    assert_eq!(chat.input, "second");
    chat.recall_next();
    assert_eq!(chat.input, "");
}

#[test]
fn chat_state_editing_clears_history_recall_cursor() {
    let mut chat = ChatState::default();
    chat.begin_input();
    chat.input = "old".to_string();
    assert_eq!(chat.submit_input().as_deref(), Some("old"));

    chat.begin_input();
    chat.recall_previous();
    assert_eq!(chat.input, "old");
    chat.push_char('!');
    chat.recall_next();

    assert_eq!(chat.input, "old!");
}

#[test]
fn network_chat_messages_include_sender_label() {
    assert_eq!(
        format_network_chat_message(ChatMessageRequest::PUBLIC_TYPE, Some("remote"), 9, "hello"),
        "remote: hello"
    );
    assert_eq!(
        format_network_chat_message(ChatMessageRequest::PUBLIC_TYPE, Some(""), 9, "hello"),
        "#9: hello"
    );
    assert_eq!(
        format_network_chat_message(ChatMessageRequest::PUBLIC_TYPE, None, 7, "arena"),
        "#7: arena"
    );
    assert_eq!(
        format_network_chat_message(ChatMessageRequest::TEAM_TYPE, Some("remote"), 9, "hello"),
        "remote (team): hello"
    );
    assert_eq!(
        format_network_chat_message(ChatMessageRequest::PRIVATE_TYPE, Some("remote"), 9, "hello"),
        "remote -> you: hello"
    );
}

#[test]
fn local_chat_input_parses_public_team_private_and_sound_commands() {
    let public = parse_local_chat_input("hello").expect("public chat should parse");
    assert_eq!(public.request, ChatMessageRequest::public("hello"));
    assert_eq!(public.echo_text(), "You: hello");

    let team = parse_local_chat_input("'cover me").expect("team chat should parse");
    assert_eq!(team.request, ChatMessageRequest::team("cover me"));
    assert_eq!(team.echo_text(), "You (team): cover me");

    let slash_team = parse_local_chat_input("/team cover me").expect("team chat should parse");
    assert_eq!(slash_team.request, ChatMessageRequest::team("cover me"));

    let private = parse_local_chat_input("/p 42 go").expect("private chat should parse");
    assert_eq!(private.request, ChatMessageRequest::private(42, "go"));
    assert_eq!(private.echo_text(), "You -> #42: go");

    let private_by_name =
        parse_local_chat_input_with_resolver("/msg beta go", |target| match target {
            "beta" => Ok(42),
            _ => Err("private chat player not found"),
        })
        .expect("private chat should resolve names");
    assert_eq!(
        private_by_name.request,
        ChatMessageRequest::private(42, "go")
    );

    let sound = parse_local_chat_input("/sound 5 hello").expect("sound chat should parse");
    assert_eq!(
        sound.request,
        ChatMessageRequest::public("hello").with_sound(5)
    );
}

#[test]
fn local_chat_input_parses_reference_game_frequency_command_separately() {
    assert_eq!(
        parse_frequency_change_input("=0").expect("frequency command should parse"),
        Some(0)
    );
    assert_eq!(
        parse_frequency_change_input(" =123 ").expect("trimmed frequency command should parse"),
        Some(123)
    );
    assert_eq!(
        parse_frequency_change_input("hello").expect("public chat should not be a frequency"),
        None
    );
}

#[test]
fn local_chat_input_parses_reference_game_status_command_separately() {
    assert!(parse_status_command("?status"));
    assert!(parse_status_command(" ?STATUS "));
    assert!(!parse_status_command("?status now"));
    assert_eq!(ship_status_percent(166.0, 1150.0, 400.0), 33);
    assert_eq!(ship_status_percent(0.0, 1150.0, 400.0), 100);
    assert_eq!(
        status_command_message(33, 50, 66, 100, 8),
        "Recharge:33%  Thruster:50%  Speed:66%  Rotation:100%  Shrapnel:8"
    );
}

#[test]
fn local_chat_input_parses_audio_volume_commands_separately() {
    assert_eq!(
        parse_audio_volume_command("?sound=7").expect("sound volume should parse"),
        Some(crate::chat::AudioVolumeCommand {
            category: AudioVolumeCategory::Effects,
            value: 7,
        })
    );
    assert_eq!(
        parse_audio_volume_command("?chat 3").expect("chat volume should parse"),
        Some(crate::chat::AudioVolumeCommand {
            category: AudioVolumeCategory::Chat,
            value: 3,
        })
    );
    assert_eq!(
        parse_audio_volume_command("?voice=0").expect("voice volume should parse"),
        Some(crate::chat::AudioVolumeCommand {
            category: AudioVolumeCategory::Voice,
            value: 0,
        })
    );
    assert_eq!(
        parse_audio_volume_command("?music=10").expect("music volume should parse"),
        Some(crate::chat::AudioVolumeCommand {
            category: AudioVolumeCategory::Music,
            value: 10,
        })
    );
    assert_eq!(
        parse_audio_volume_command("hello").expect("normal chat should not be volume command"),
        None
    );
    assert_eq!(
        parse_audio_volume_command("?unknown=5").expect("unknown question command is normal chat"),
        None
    );
    assert_eq!(
        parse_audio_volume_command("?sound=11").unwrap_err(),
        "volume command needs a value from 0 to 10"
    );
}

#[test]
fn local_chat_input_rejects_malformed_chat_commands() {
    assert_eq!(
        parse_local_chat_input("/p nope hi").unwrap_err(),
        "private chat target must be a player id"
    );
    assert_eq!(
        parse_local_chat_input("/p").unwrap_err(),
        "private chat needs a target"
    );
    assert_eq!(
        parse_local_chat_input_with_resolver("/p nope hi", |_| Err(
            "private chat player not found"
        ))
        .unwrap_err(),
        "private chat player not found"
    );
    assert_eq!(
        parse_local_chat_input("/sound 999 hi").unwrap_err(),
        "chat sound id must be 0-255"
    );
    assert_eq!(
        parse_local_chat_input("/team").unwrap_err(),
        "team chat needs a message"
    );
    assert_eq!(
        parse_frequency_change_input("=").unwrap_err(),
        "frequency change needs a number"
    );
    assert_eq!(
        parse_frequency_change_input("=abc").unwrap_err(),
        "frequency change needs a number"
    );
    assert_eq!(
        parse_frequency_change_input("=65536").unwrap_err(),
        "frequency is out of range"
    );
}

#[test]
fn private_chat_autocomplete_completes_unique_name_prefixes() {
    let names = ["alpha", "beta", "gamma"];

    assert_eq!(
        autocomplete_private_chat_target("/p be", names).unwrap(),
        Some("/p beta ".to_string())
    );
    assert_eq!(
        autocomplete_private_chat_target("/private BETA", names).unwrap(),
        Some("/private beta ".to_string())
    );
    assert_eq!(
        autocomplete_private_chat_target("/msg beta hello", names).unwrap(),
        None
    );
    assert_eq!(
        autocomplete_private_chat_target("/team be", names).unwrap(),
        None
    );
}

#[test]
fn private_chat_autocomplete_reports_ambiguous_or_missing_targets() {
    assert_eq!(
        autocomplete_private_chat_target("/p al", ["alpha", "alpine"]).unwrap_err(),
        "chat autocomplete player name is ambiguous"
    );
    assert_eq!(
        autocomplete_private_chat_target("/p zed", ["alpha", "beta"]).unwrap_err(),
        "chat autocomplete found no player"
    );
}

#[test]
fn status_energy_drain_turns_off_when_empty() {
    let mut active = true;
    let mut energy = 100.0;
    drain_status_energy(&mut active, &mut energy, 10.0, 2.0);
    assert!(active);
    assert_eq!(energy, 80.0);

    drain_status_energy(&mut active, &mut energy, 100.0, 1.0);
    assert!(!active);
    assert_eq!(energy, 80.0);
}

#[test]
fn energy_prizes_use_ship_upgrade_energy_step_for_capacity() {
    assert_eq!(
        energy_capacity_after_prize(500.0, 100.0, 300.0, 1500.0, false),
        600.0
    );
    assert_eq!(
        energy_capacity_after_prize(1460.0, 100.0, 300.0, 1500.0, false),
        1500.0
    );
    assert_eq!(
        energy_capacity_after_prize(350.0, 100.0, 300.0, 1500.0, true),
        300.0
    );
    assert_eq!(
        energy_capacity_after_prize(500.0, 100.0, 300.0, 1500.0, true),
        400.0
    );
    assert_eq!(current_energy_after_capacity_prize(700.0, 400.0), 700.0);
}

#[test]
fn stat_prizes_clamp_each_stat_to_its_own_maximum() {
    assert_eq!(
        stat_value_after_prize(140.0, 100.0, 20.0, 160.0, false),
        160.0
    );
    assert_eq!(
        stat_value_after_prize(160.0, 100.0, 20.0, 160.0, false),
        160.0
    );
    assert_eq!(
        stat_value_after_prize(280.0, 200.0, 40.0, 290.0, false),
        290.0
    );
    assert_eq!(
        stat_value_after_prize(290.0, 200.0, 40.0, 290.0, true),
        250.0
    );
    assert_eq!(
        stat_value_after_prize(100.0, 100.0, 20.0, 160.0, true),
        100.0
    );
}

#[test]
fn respawn_delay_has_two_second_minimum() {
    assert_eq!(respawn_delay_seconds(0.0), 2.0);
    assert_eq!(respawn_delay_seconds(-1.0), 2.0);
    assert_eq!(respawn_delay_seconds(0.5), 2.0);
    assert_eq!(respawn_delay_seconds(1.5), 1.5 + EXPLOSION_LIFETIME);
}

#[test]
fn status_prizes_respect_ship_capability_bits() {
    assert_eq!(
        status_prize_capability_after(true, false, false),
        Some(true)
    );
    assert_eq!(status_prize_capability_after(true, true, true), Some(false));
    assert_eq!(status_prize_capability_after(false, false, false), None);
    assert_eq!(
        status_prize_capability_after(false, true, true),
        Some(false)
    );
    assert_eq!(
        capability_prize_notification(PrizeKind::Multifire, false, false),
        Some((PrizeKind::Multifire, false))
    );
    assert_eq!(
        capability_prize_notification(PrizeKind::Multifire, true, false),
        None
    );
    assert_eq!(
        capability_prize_notification(PrizeKind::Multifire, true, true),
        Some((PrizeKind::Multifire, true))
    );
    assert_eq!(
        capability_prize_notification(PrizeKind::Multifire, false, true),
        None
    );
    assert_eq!(
        status_prize_notification(PrizeKind::Stealth, true, false, false),
        Some((PrizeKind::Stealth, false))
    );
    assert_eq!(
        status_prize_notification(PrizeKind::Stealth, true, true, false),
        None
    );
    assert_eq!(
        status_prize_notification(PrizeKind::Stealth, false, false, false),
        Some((PrizeKind::FullCharge, false))
    );
    assert_eq!(
        status_prize_notification(PrizeKind::Stealth, false, false, true),
        Some((PrizeKind::FullCharge, true))
    );
    assert_eq!(
        status_prize_notification(PrizeKind::Stealth, false, true, true),
        Some((PrizeKind::Stealth, true))
    );

    let mut capable = false;
    let mut active = false;
    let mut energy = 25.0;
    apply_status_prize(&mut capable, &mut active, &mut energy, 1500.0, false, false);
    assert!(!capable);
    assert!(!active);
    assert_eq!(energy, 1500.0);

    apply_status_prize(&mut capable, &mut active, &mut energy, 1500.0, false, true);
    assert!(!capable);
    assert!(!active);
    assert_eq!(energy, 1.0);

    capable = true;
    active = true;
    energy = 800.0;
    apply_status_prize(&mut capable, &mut active, &mut energy, 1500.0, false, true);
    assert!(!capable);
    assert!(!active);
    assert_eq!(energy, 800.0);

    apply_status_prize(&mut capable, &mut active, &mut energy, 1500.0, true, false);
    assert!(capable);
    assert!(!active);

    active = true;
    apply_status_prize(&mut capable, &mut active, &mut energy, 1500.0, true, true);
    assert!(!capable);
    assert!(!active);
}

#[test]
fn timed_prize_statuses_tick_down_to_zero() {
    let mut timer = 3.0;
    tick_timed_status(&mut timer, 1.25);
    assert_eq!(timer, 1.75);

    tick_timed_status(&mut timer, 5.0);
    assert_eq!(timer, 0.0);
}

#[test]
fn super_prize_uses_random_duration_without_shortening_current_timer() {
    assert_eq!(super_prize_lifetime_after(0.0, 12.0, 1_234), 0.34);
    assert_eq!(super_prize_lifetime_after(5.0, 12.0, 123), 5.0);
    assert_eq!(super_prize_lifetime_after(0.0, 0.0, 123), 0.0);
}

#[test]
fn timed_and_warp_prizes_match_reference_negative_behavior() {
    assert_eq!(shield_prize_lifetime_after(4.0), 4.0);
    assert_eq!(shield_prize_lifetime_after(-1.0), 0.0);
    assert_eq!(engine_shutdown_lifetime_after(4.0, false), 4.0);
    assert_eq!(engine_shutdown_lifetime_after(4.0, true), 12.0);
    assert_eq!(engine_shutdown_lifetime_after(-1.0, true), 0.0);
    assert_eq!(warp_prize_energy_after(500.0, 1700.0, false), 500.0);
    assert_eq!(warp_prize_energy_after(500.0, 1700.0, true), 1.0);
    assert_eq!(
        warp_prize_notification_after(false),
        (PrizeKind::Warp, false)
    );
    assert_eq!(
        warp_prize_notification_after(true),
        (PrizeKind::FullCharge, true)
    );
    let (kind, negative) = warp_prize_notification_after(true);
    assert_eq!(prize_notification(kind, negative), "Energy depleted.");
}

#[test]
fn shrapnel_upgrades_increase_fragment_count_until_cap() {
    assert_eq!(shrapnel_count(0, MAX_SHRAPNEL_UPGRADES), 0);
    assert_eq!(shrapnel_count(2, MAX_SHRAPNEL_UPGRADES), 2);
    assert_eq!(
        shrapnel_count(MAX_SHRAPNEL_UPGRADES + 3, MAX_SHRAPNEL_UPGRADES),
        MAX_SHRAPNEL_UPGRADES as usize
    );
    assert_eq!(shrapnel_count(13, 13), 13);
}

#[test]
fn shrapnel_prizes_use_ship_rate_and_cap() {
    assert_eq!(shrapnel_prize_amount_after(0, 3, 13, false), 3);
    assert_eq!(shrapnel_prize_amount_after(12, 3, 13, false), 13);
    assert_eq!(shrapnel_prize_amount_after(2, 3, 13, true), 2);
    assert_eq!(shrapnel_prize_amount_after(9, 3, 13, true), 6);
    assert_eq!(shrapnel_prize_amount_after(0, 0, 13, false), 0);
}

#[test]
fn shrapnel_direction_uses_circular_or_random_arena_setting() {
    let circular_first = shrapnel_direction(0, 8, false, 123);
    let circular_next = shrapnel_direction(1, 8, false, 123);
    let random_first = shrapnel_direction(0, 8, true, 123);
    let random_repeat = shrapnel_direction(0, 8, true, 123);
    let mut rng_seed = 123;
    let reference_game_roll = vie_rng_next(&mut rng_seed) % 40_000;
    let reference_game_angle = (reference_game_roll as f32 * 9.0 / 1000.0).to_radians();
    let reference_game_first = vec2(reference_game_angle.sin(), -reference_game_angle.cos());

    assert!((circular_first - vec2(0.0, -1.0)).length() < 0.001);
    assert!(
        (circular_next
            - vec2(
                std::f32::consts::FRAC_1_SQRT_2,
                -std::f32::consts::FRAC_1_SQRT_2
            ))
        .length()
            < 0.001
    );
    assert!((circular_next.length() - 1.0).abs() < 0.001);
    assert!((random_first - reference_game_first).length() < 0.001);
    assert!((random_first.length() - 1.0).abs() < 0.001);
    assert_eq!(random_first, random_repeat);
    assert_ne!(random_first, circular_first);
}

#[test]
fn shrapnel_damage_uses_inactive_window_then_scaled_bullet_damage() {
    let inactive = inactive_shrapnel_damage(20.0, 3);
    let active = active_shrapnel_damage(90.0, 10.0, 0.5, 3);

    assert_eq!(inactive, 40.0);
    assert_eq!(active, 55.0);
    assert_eq!(inactive_shrapnel_damage(20.75, 3), 40.0);
    assert_eq!(active_shrapnel_damage(91.0, 10.0, 0.5, 3), 55.0);
    assert_eq!(shrapnel_damage(0.25, inactive, active), inactive);
    assert_eq!(shrapnel_damage(0.251, inactive, active), active);
}

#[test]
fn early_bouncing_shrapnel_wall_hits_timeout_like_reference_game() {
    assert!(early_shrapnel_wall_bounce_times_out(0.25));
    assert!(!early_shrapnel_wall_bounce_times_out(0.251));
}

#[test]
fn weapon_level_index_clamps_to_original_four_levels() {
    assert_eq!(weapon_level_index(0), 0);
    assert_eq!(weapon_level_index(1), 0);
    assert_eq!(weapon_level_index(3), 2);
    assert_eq!(weapon_level_index(9), 3);
}

#[test]
fn starfield_hash_is_deterministic_and_salted() {
    assert_eq!(hash_grid(12, -7, 1), hash_grid(12, -7, 1));
    assert_ne!(hash_grid(12, -7, 0), hash_grid(12, -7, 1));
}

#[test]
fn engine_shutdown_uses_reference_slow_rotation() {
    assert_eq!(ENGINE_SHUTDOWN_ROTATION_PER_SECOND, 40.0 / 400.0);
}

#[test]
fn treats_black_as_transparent_for_sprites() {
    let decoded = decode_bm2(
        include_bytes!("../../assets/graphics/ships.bm2"),
        Transparency::Black,
    )
    .expect("ships.bm2 should decode");

    assert_eq!(decoded.rgba[3], 0);
}

#[test]
fn spectator_cycles_include_roster_actors_without_a_current_aoi_position() {
    let mut world = test_attach_world();
    world
        .players
        .insert(9, test_network_player(9, 1, 3, Some(vec2(200.0, 100.0))));
    world
        .players
        .insert(10, test_network_player(10, 8, 3, None));
    world
        .players
        .insert(11, test_network_player(11, 2, 4, None));
    assert_eq!(
        crate::spectate::next_spectate_target(&world, Some(9), vec2(100.0, 100.0), false, 3, false),
        Some(11)
    );
    assert_eq!(
        crate::spectate::next_spectate_target(
            &world,
            Some(11),
            vec2(100.0, 100.0),
            false,
            3,
            false
        ),
        Some(9)
    );
    assert_eq!(
        crate::spectate::next_spectate_target(&world, Some(9), vec2(100.0, 100.0), true, 3, false),
        Some(11)
    );
}
