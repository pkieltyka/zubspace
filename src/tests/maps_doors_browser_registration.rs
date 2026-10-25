use super::*;
use crate::lvz::LvzImageDefinition;

#[test]
fn parses_plain_lvl_tile_records() {
    let data = [
        pack_lvl_tile(12, 34, 1),
        pack_lvl_tile(512, 511, 220),
        pack_lvl_tile(3, 4, 219),
        pack_lvl_tile(20, 21, DOOR_FIRST_TILE_ID),
        pack_lvl_tile(22, 21, DOOR_LAST_TILE_ID),
    ]
    .concat();
    let map = ArenaMap::from_lvl("test.lvl", &data).expect("plain lvl records should parse");

    assert_eq!(map.tile(12, 34), 1);
    assert!(map.is_solid_world(vec2(12.5 * TILE_SIZE, 34.5 * TILE_SIZE)));
    assert_eq!(map.tile(512, 511), 220);
    assert_eq!(map.tile(516, 515), 220);
    assert!(!map.is_solid_world(vec2(512.5 * TILE_SIZE, 511.5 * TILE_SIZE)));
    assert_eq!(map.tile(8, 9), 219);
    assert_eq!(map.doors.len(), 2);
    assert_eq!(
        (map.doors[0].x, map.doors[0].y, map.doors[0].id),
        (20, 21, DOOR_FIRST_TILE_ID)
    );
    assert_eq!(
        (map.doors[1].x, map.doors[1].y, map.doors[1].id),
        (22, 21, DOOR_LAST_TILE_ID)
    );
}

#[test]
fn parses_lvl_records_after_bitmap_prefix() {
    let tile = pack_lvl_tile(7, 9, 5);
    let mut data = minimal_test_bm2();
    data.extend_from_slice(&tile);

    let map = ArenaMap::from_lvl("prefixed.lvl", &data).expect("BM-prefixed lvl should parse");

    assert_eq!(map.tile(7, 9), 5);
}

#[test]
fn tracks_flag_and_goal_objects_for_pickups() {
    let data = [
        pack_lvl_tile(100, 101, FLAG_TILE_ID),
        pack_lvl_tile(104, 101, GOAL_TILE_ID),
    ]
    .concat();
    let map = ArenaMap::from_lvl("flag_goal.lvl", &data).expect("flag and goal should parse");

    assert!(map.is_touching_object(
        vec2(100.5 * TILE_SIZE, 101.5 * TILE_SIZE),
        FLAG_TILE_ID,
        FLAG_PICKUP_RADIUS
    ));
    assert!(map.is_touching_object(
        vec2(104.5 * TILE_SIZE, 101.5 * TILE_SIZE),
        GOAL_TILE_ID,
        FLAG_PICKUP_RADIUS
    ));
}

#[test]
fn dropped_flags_hide_source_and_fall_back_to_spawn_when_solid() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.tiles[10 * map.width + 10] = 1;

    assert!(flag_source_hidden(true, 0.0, false));
    assert!(flag_source_hidden(false, 0.5, false));
    assert!(flag_source_hidden(false, 0.0, true));
    assert!(!flag_source_hidden(false, 0.0, false));

    let open = vec2(11.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    assert_eq!(dropped_flag_position(&map, open), open);

    let blocked = vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    assert_ne!(dropped_flag_position(&map, blocked), blocked);
    assert!(!map.is_solid_world(dropped_flag_position(&map, blocked)));
}

#[test]
fn carry_flags_limit_matches_reference_semantics() {
    assert_eq!(carryable_flag_limit(0), None);
    assert_eq!(carryable_flag_limit(1), Some(u16::MAX));
    assert_eq!(carryable_flag_limit(2), Some(1));
    assert!(!can_pickup_carryable_flag(0, 0));
    assert!(can_pickup_carryable_flag(1, 32));
    assert!(can_pickup_carryable_flag(2, 0));
    assert!(!can_pickup_carryable_flag(2, 1));
}

#[test]
fn flagger_adjustments_apply_only_to_active_carriers() {
    assert_eq!(flagger_damage_received(100.0, 0.75, true), 75.0);
    assert_eq!(flagger_damage_received(101.0, 0.5, true), 50.0);
    assert_eq!(flagger_damage_received(100.0, 0.75, false), 100.0);
    assert_eq!(flagger_bomb_cooldown(0.65, 0.35, true), 1.0);
    assert_eq!(flagger_bomb_cooldown(0.65, 0.35, false), 0.65);
}

#[test]
fn matches_reference_game_solid_tile_rules() {
    assert!(!is_solid_tile(0));
    assert!(is_solid_tile(1));
    assert!(is_solid_tile(169));
    assert!(!is_solid_tile(170));
    assert!(!is_solid_tile(220));
    assert!(is_solid_tile(240));
    assert!(!is_solid_tile(241));
    assert!(is_solid_tile(252));
}

#[test]
fn seeded_door_tiles_open_and_close_by_reference_bits() {
    assert_eq!(reference_game_seeded_door_table(0), [OPEN_DOOR_TILE_ID; 8]);
    assert_eq!(
        reference_game_seeded_door_table(0xff),
        [162, 163, 164, 165, 166, 167, 168, 169]
    );
    assert_eq!(
        reference_game_seeded_door_table(0b0101_1010),
        [
            OPEN_DOOR_TILE_ID,
            163,
            OPEN_DOOR_TILE_ID,
            165,
            166,
            OPEN_DOOR_TILE_ID,
            168,
            OPEN_DOOR_TILE_ID,
        ]
    );
    assert_eq!(
        seeded_door_tile(DOOR_FIRST_TILE_ID, 0),
        Some(OPEN_DOOR_TILE_ID)
    );
    assert_eq!(
        seeded_door_tile(DOOR_FIRST_TILE_ID, 1),
        Some(DOOR_FIRST_TILE_ID)
    );
    assert_eq!(
        seeded_door_tile(DOOR_LAST_TILE_ID, 0),
        Some(OPEN_DOOR_TILE_ID)
    );
    assert_eq!(
        seeded_door_tile(DOOR_LAST_TILE_ID, 0x80),
        Some(DOOR_LAST_TILE_ID)
    );
    assert_eq!(seeded_door_tile(42, 0), None);
}

#[test]
fn door_seed_modes_follow_reference_game_rules() {
    let mut rng = 1;
    assert_eq!(vie_rng_next(&mut rng), 16_930);
    assert_eq!(vie_rng_next(&mut rng), 284_542_633);

    let mut rng = 1;
    assert_eq!(
        next_door_seed(0xfe, &mut rng, DOOR_MODE_LOCAL_INCREMENT),
        0xff
    );
    assert_eq!(
        next_door_seed(0xff, &mut rng, DOOR_MODE_LOCAL_INCREMENT),
        0x00
    );
    assert_eq!(next_door_seed(0, &mut rng, -2), 34);
    assert_eq!(rng, 16_930);
    assert_eq!(next_door_seed(0, &mut rng, 0x5a), 0x5a);
}

#[test]
fn door_update_timer_catches_up_elapsed_intervals() {
    let (count, timer) = door_update_count_and_next_timer(0.3, 1.2, 0.5);
    assert_eq!(count, 2);
    assert!((timer - 0.1).abs() < 0.0001);

    let (count, timer) = door_update_count_and_next_timer(0.3, 0.1, 0.5);
    assert_eq!(count, 0);
    assert!((timer - 0.2).abs() < 0.0001);
}

#[test]
fn security_challenge_seeds_map_door_rng() {
    let mut map = ArenaMap::fallback_skirmish();
    map.door_rng_seed = 99;

    seed_map_doors_from_security(
        &mut map,
        SecurityChallenge {
            prize_seed: 1,
            door_seed: 0x8000_0001,
            timestamp: 3,
            checksum_key: 4,
        },
    );

    assert_eq!(map.door_rng_seed, 0x8000_0001_u32 as i32);
}

#[test]
fn security_challenge_aligns_doors_to_server_timestamp() {
    let mut map = ArenaMap::fallback_skirmish();
    map.door_seed = 0xff;
    map.door_rng_seed = 99;

    let timer = synchronize_map_doors_from_security(
        &mut map,
        SecurityChallenge {
            prize_seed: 1,
            door_seed: 1,
            timestamp: 900,
            checksum_key: 4,
        },
        -2,
        0.5,
        1020,
    );

    assert_eq!(map.door_seed, 169);
    assert_eq!(map.door_rng_seed, 284_542_633);
    assert!((timer - 0.30).abs() < 0.0001);
}

#[test]
fn security_rebind_after_level_install_restores_exact_phase() {
    let challenge = SecurityChallenge {
        prize_seed: 1,
        door_seed: 2,
        timestamp: 2_436,
        checksum_key: 4,
    };
    let server_tick = 2_644;
    let mut original = ArenaMap::fallback_skirmish();
    synchronize_map_doors_from_security(&mut original, challenge, -2, 1.2, server_tick);

    // A downloaded level constructs a fresh map after the challenge and
    // therefore resets its local RNG until the cached challenge is replayed.
    let mut downloaded = ArenaMap::fallback_skirmish();
    advance_map_door_seed_steps(&mut downloaded, -2, 2);
    assert_ne!(downloaded.door_seed, original.door_seed);
    synchronize_map_doors_from_security(&mut downloaded, challenge, -2, 1.2, server_tick);

    assert_eq!(downloaded.door_seed, original.door_seed);
    assert_eq!(downloaded.door_rng_seed, original.door_rng_seed);
}

#[test]
fn security_rebind_after_settings_order_restores_exact_phase() {
    let challenge = SecurityChallenge {
        prize_seed: 1,
        door_seed: 2,
        timestamp: 2_436,
        checksum_key: 4,
    };
    let server_tick = 2_644;
    let mut map = ArenaMap::fallback_skirmish();

    // A challenge observed with provisional tuning is invalidated when the
    // reliable settings arrive later. Replaying the cached challenge against
    // the final settings must equal a direct final-settings bind.
    synchronize_map_doors_from_security(&mut map, challenge, 0, 0.5, server_tick);
    let provisional_seed = map.door_seed;
    synchronize_map_doors_from_security(&mut map, challenge, -2, 1.2, server_tick);

    let mut direct = ArenaMap::fallback_skirmish();
    synchronize_map_doors_from_security(&mut direct, challenge, -2, 1.2, server_tick);
    assert_ne!(provisional_seed, direct.door_seed);
    assert_eq!(map.door_seed, direct.door_seed);
    assert_eq!(map.door_rng_seed, direct.door_rng_seed);
}

#[test]
fn repeated_security_rebind_is_an_absolute_idempotent_door_phase() {
    let challenge = SecurityChallenge {
        prize_seed: 1,
        door_seed: 2,
        timestamp: 7_200,
        checksum_key: 4,
    };
    let server_tick = 8_411;
    let mut map = ArenaMap::fallback_skirmish();
    synchronize_map_doors_from_security(&mut map, challenge, -2, 1.2, server_tick);
    let expected_seed = map.door_seed;
    let expected_rng = map.door_rng_seed;

    // Model an already-running browser plus a settings/security reorder: both
    // the visible seed and RNG have moved before the cached challenge replays.
    advance_map_door_seed_steps(&mut map, -2, 7);
    assert_ne!(
        (map.door_seed, map.door_rng_seed),
        (expected_seed, expected_rng)
    );
    synchronize_map_doors_from_security(&mut map, challenge, -2, 1.2, server_tick);

    assert_eq!(map.door_seed, expected_seed);
    assert_eq!(map.door_rng_seed, expected_rng);
}

#[test]
fn door_solidity_tracks_seeded_open_state() {
    assert!(!is_solid_tile_with_doors(DOOR_FIRST_TILE_ID, 0));
    assert!(is_solid_tile_with_doors(DOOR_FIRST_TILE_ID, 1));
    assert!(!is_solid_tile_with_doors(DOOR_LAST_TILE_ID, 0));
    assert!(is_solid_tile_with_doors(DOOR_LAST_TILE_ID, 0x80));
}

#[test]
fn door_relocation_detects_entities_inside_newly_closed_doors() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.tiles[10 * map.width + 10] = DOOR_FIRST_TILE_ID;
    let pos = tile_center(ivec2(10, 10));

    map.door_seed = 0;
    assert!(!entity_needs_door_relocation(&map, pos, 1.0, 0x00, 0x00));

    let door_min = 10.0 * TILE_SIZE;
    let door_max = door_min + TILE_SIZE;
    let center = door_min + TILE_SIZE * 0.5;
    let radius = 4.0;
    assert!(!ship_axis_collides_with_solid_or_bounds(
        &map,
        vec2(door_min - radius, center),
        radius,
        &[],
        CollisionAxis::X,
        1.0,
    ));

    map.door_seed = 1;
    assert!(entity_needs_door_relocation(&map, pos, 1.0, 0x00, 0xff));
    assert!(ship_axis_collides_with_solid_or_bounds(
        &map,
        vec2(door_min - radius, center),
        radius,
        &[],
        CollisionAxis::X,
        1.0,
    ));
    assert!(entity_needs_door_relocation(
        &map,
        vec2(door_min - radius, center),
        radius,
        0x00,
        0xff,
    ));
    assert!(!entity_needs_door_relocation(
        &map,
        vec2(door_max + radius, center),
        radius,
        0x00,
        0xff,
    ));
    assert!(entity_needs_door_relocation(
        &map,
        vec2(door_min - radius + 0.001, center),
        radius,
        0x00,
        0xff,
    ));
    assert!(entity_needs_door_relocation(
        &map,
        vec2(door_max + radius - 0.001, center),
        radius,
        0x00,
        0xff,
    ));
}

#[test]
fn browser_phase_catch_up_checks_every_intermediate_rng_transition() {
    let pos = tile_center(ivec2(10, 10));
    let mut fixture = None;
    for start_count in 0..512 {
        let mut candidate = ArenaMap::fallback_skirmish();
        candidate.tiles.fill(0);
        candidate.tiles[10 * candidate.width + 10] = DOOR_FIRST_TILE_ID;
        candidate.door_seed = u8::MAX;
        candidate.door_rng_seed = 1;
        advance_map_door_seed_steps(&mut candidate, -2, start_count);
        let initial_seed = candidate.door_seed;
        let mut caught_intermediate_close = false;
        for _ in 0..3 {
            let before = candidate.door_seed;
            advance_map_door_seed_steps(&mut candidate, -2, 1);
            let after = candidate.door_seed;
            caught_intermediate_close |=
                entity_needs_door_relocation(&candidate, pos, 1.0, before, after);
        }
        if caught_intermediate_close
            && !entity_needs_door_relocation(
                &candidate,
                pos,
                1.0,
                initial_seed,
                candidate.door_seed,
            )
        {
            fixture = Some((start_count, candidate.door_seed, candidate.door_rng_seed));
            break;
        }
    }
    let (start_count, final_seed, final_rng_seed) =
        fixture.expect("canonical Chaos RNG has an intermediate close hidden by its final phase");
    let mut stepped = ArenaMap::fallback_skirmish();
    stepped.tiles.fill(0);
    stepped.tiles[10 * stepped.width + 10] = DOOR_FIRST_TILE_ID;
    stepped.door_seed = u8::MAX;
    stepped.door_rng_seed = 1;
    advance_map_door_seed_steps(&mut stepped, -2, start_count);
    let mut relocations = 0;
    for _ in 0..3 {
        let before = stepped.door_seed;
        advance_map_door_seed_steps(&mut stepped, -2, 1);
        let after = stepped.door_seed;
        relocations += usize::from(entity_needs_door_relocation(
            &stepped, pos, 1.0, before, after,
        ));
    }
    assert!(relocations >= 1);
    assert_eq!(stepped.door_seed, final_seed);
    assert_eq!(stepped.door_rng_seed, final_rng_seed);
}

#[test]
fn matches_reference_game_base_tile_render_filter() {
    assert!(!should_draw_base_tile(0));
    assert!(should_draw_base_tile(1));
    assert!(!should_draw_base_tile(162));
    assert!(!should_draw_base_tile(169));
    assert!(!should_draw_base_tile(170));
    assert!(should_draw_base_tile(171));
    assert!(!should_draw_base_tile(172));
    assert!(should_draw_base_tile(190));
    assert!(!should_draw_base_tile(191));
    assert!(!should_draw_base_tile(220));
}

#[test]
fn door_animation_uses_reference_game_tile_groups() {
    assert_eq!(
        door_animation_source(DOOR_FIRST_TILE_ID, 0.0),
        tile_source(162)
    );
    assert_eq!(
        door_animation_source(DOOR_FIRST_TILE_ID + 3, DOOR_ANIMATION_DURATION * 0.99),
        tile_source(165)
    );
    assert_eq!(
        door_animation_source(DOOR_FIRST_TILE_ID + 4, 0.0),
        tile_source(166)
    );
    assert_eq!(
        door_animation_source(DOOR_LAST_TILE_ID, DOOR_ANIMATION_DURATION * 0.99),
        tile_source(169)
    );
}

#[test]
fn parses_all_repository_levels() {
    let mut parsed_count = 0;

    for path in repository_level_paths() {
        let data = std::fs::read(&path).expect("level file should be readable");
        ArenaMap::from_lvl(&path, &data).unwrap_or_else(|err| panic!("{path} should parse: {err}"));
        parsed_count += 1;
    }

    assert_eq!(parsed_count, LEVEL_PATHS.len());
}

#[test]
fn chaos_zone_contains_solid_tiles_without_base_art() {
    let path = "assets/zones/chaos-zone/bdhadar.lvl";
    let data = std::fs::read(path).expect("Chaos level should be readable");
    let map = ArenaMap::from_lvl(path, &data).expect("Chaos level should parse");
    let mut counts = std::collections::BTreeMap::new();

    for id in map.tiles.iter().copied() {
        if id > 190 && is_solid_tile(id) && map.render_tile(id).is_none() {
            *counts.entry(id).or_insert(0usize) += 1;
        }
    }

    assert_eq!(counts.get(&242), Some(&486));
    assert_eq!(counts.get(&243), Some(&14_230));
    assert_eq!(counts.get(&244), Some(&1_200));
    assert!(!counts.contains_key(&241));
}

#[test]
fn chaos_radar_index_narrows_a_one_viewport_scan() {
    let path = "assets/zones/chaos-zone/bdhadar.lvl";
    let data = std::fs::read(path).expect("Chaos level should be readable");
    let map = ArenaMap::from_lvl(path, &data).expect("Chaos level should parse");
    let points = &map.radar_points;
    assert_eq!(points.len(), 26_390, "bundled Chaos radar density");

    // The small radar at a 1440x1000 screen spans at most 270x187 tiles of the
    // 1024x1024-tile map, so one frame must not walk every radar point.
    let view = crate::radar::RadarView {
        dim_px: 480.0,
        min_tile: macroquad::prelude::vec2(400.0, 400.0),
        tile_span: macroquad::prelude::vec2(270.0, 187.0),
    };
    let mut candidates = 0usize;
    let mut visible = 0usize;
    map.radar_point_index()
        .visit_view_candidates(points, view, |point| {
            candidates += 1;
            if view.contains_tile(point.x, point.y) {
                visible += 1;
            }
        });

    assert!(visible > 0, "the viewport covers radar tiles");
    assert_eq!(
        visible,
        points
            .iter()
            .filter(|point| view.contains_tile(point.x, point.y))
            .count(),
        "the index must not drop or duplicate a visible point"
    );
    assert!(
        candidates * 4 < points.len(),
        "the index visited {candidates} of {} radar points for one viewport",
        points.len()
    );
}

#[test]
fn generated_level_manifest_matches_repository_levels() {
    let mut paths = repository_level_paths();
    sort_manifest_paths(&mut paths);

    assert_eq!(LEVEL_PATHS, paths.as_slice());
    assert_eq!(
        LEVEL_PATHS.first(),
        Some(&"assets/zones/chaos-zone/bdhadar.lvl")
    );
    assert!(LEVEL_PATHS.contains(&"assets/zones/chaos-zone/bdhadar.lvl"));
    assert!(!LEVEL_PATHS.contains(&"levels/Ana-Armageddon.lvl"));
    assert!(!LEVEL_PATHS.contains(&"levels/Ana-DBGT.lvl"));
}

#[test]
fn decodes_all_repository_lvl_embedded_tilesets() {
    let mut decoded_count = 0;

    for path in repository_level_paths() {
        let data = std::fs::read(&path).expect("level file should be readable");
        let Some(bitmap) =
            lvl_embedded_bitmap(&data).unwrap_or_else(|err| panic!("{path} should scan: {err}"))
        else {
            continue;
        };
        let decoded = decode_bm2(bitmap, Transparency::Opaque)
            .unwrap_or_else(|err| panic!("{path} embedded tileset should decode: {err}"));

        assert_eq!(decoded.width, 304, "{path} tileset width");
        assert_eq!(decoded.height, 160, "{path} tileset height");
        decoded_count += 1;
    }

    assert_eq!(
        decoded_count,
        LEVEL_PATHS.len(),
        "all repository levels should include embedded tilesets"
    );
}

#[test]
fn parses_all_repository_lvz_archives() {
    let mut parsed_count = 0;

    for path in repository_lvz_paths() {
        let data = std::fs::read(&path).expect("LVZ file should be readable");
        LvzArchive::parse(&data).unwrap_or_else(|err| panic!("{path} should parse: {err}"));
        parsed_count += 1;
    }

    assert_eq!(parsed_count, LVZ_PATHS.len());
}

// There used to be a `decodes_all_repository_lvz_bm2_file_sections` gate here, sweeping every
// `.bm2` File section out of every shipped archive. It retired with the last archive that carried
// one, and its own `decoded_count > 0` pin is what said so: no shipped zone archive contains a BM2
// section any more, so the sweep had no subject. BM2 decoding itself stays pinned twice over -
// against the shipped `assets/graphics/*.bm2` sheets in `tests/assets_visuals.rs`, and through a
// real archive File section in `tests/attach_lvz_reliable_chat.rs`.

#[test]
fn decodes_all_repository_lvz_raster_file_sections() {
    let mut decoded_count = 0;
    let mut regular_raster_count = 0;
    let mut regular_alpha_count = 0;

    for path in repository_lvz_paths() {
        let data = std::fs::read(&path).expect("LVZ file should be readable");
        let archive =
            LvzArchive::parse(&data).unwrap_or_else(|err| panic!("{path} should parse: {err}"));
        for section in archive.into_sections() {
            if !matches!(section.kind, LvzSectionKind::File) {
                continue;
            }

            let name = normalize_asset_name(&section.filename);
            if name.ends_with(".bm2") {
                let decoded =
                    decode_bm2(&section.data, Transparency::Black).unwrap_or_else(|err| {
                        panic!("{}:{} should decode: {err}", path, section.filename)
                    });
                assert!(
                    decoded.width > 0 && decoded.height > 0,
                    "{}:{} should have non-empty dimensions",
                    path,
                    section.filename
                );
                decoded_count += 1;
            } else if lvz_raster_extension(&name) {
                let image = decode_lvz_raster_image(&section.data).unwrap_or_else(|err| {
                    panic!("{}:{} should decode: {err}", path, section.filename)
                });
                assert!(
                    image.width() > 0 && image.height() > 0,
                    "{}:{} should have non-empty dimensions",
                    path,
                    section.filename
                );
                assert_eq!(
                    image.bytes().len(),
                    image.width() as usize * image.height() as usize * 4,
                    "{}:{} should decode to a complete RGBA buffer",
                    path,
                    section.filename
                );
                if image.bytes().chunks_exact(4).any(|pixel| pixel[3] != 255) {
                    regular_alpha_count += 1;
                }
                regular_raster_count += 1;
                decoded_count += 1;
            }
        }
    }

    assert!(
        decoded_count > 0,
        "repository should include LVZ raster sections"
    );
    assert!(
        regular_raster_count > 0,
        "repository should include non-BM2 LVZ raster sections"
    );
    assert!(
        regular_alpha_count > 0,
        "repository should include non-BM2 LVZ rasters that preserve decoded alpha"
    );
}

#[test]
fn repository_lvz_object_image_references_resolve_within_zone() {
    let mut zone_textures: BTreeMap<String, BTreeMap<String, ()>> = BTreeMap::new();
    let mut zone_image_references: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();

    for path in repository_lvz_paths() {
        let zone = lvz_zone_key(&path);
        let data = std::fs::read(&path).expect("LVZ file should be readable");
        let archive =
            LvzArchive::parse(&data).unwrap_or_else(|err| panic!("{path} should parse: {err}"));
        for section in archive.into_sections() {
            match section.kind {
                LvzSectionKind::File => {
                    let name = normalize_asset_name(&section.filename);
                    if name.ends_with(".bm2") || lvz_raster_extension(&name) {
                        zone_textures
                            .entry(zone.clone())
                            .or_default()
                            .insert(name, ());
                    }
                }
                LvzSectionKind::Objects(objects) => {
                    for image in objects.images {
                        let name = normalize_asset_name(&image.filename);
                        if name.ends_with(".bm2") || lvz_raster_extension(&name) {
                            zone_image_references
                                .entry(zone.clone())
                                .or_default()
                                .push((path.clone(), name));
                        }
                    }
                }
            }
        }
    }

    let mut checked_count = 0;
    let mut unresolved = Vec::new();
    for (zone, references) in zone_image_references {
        let textures = zone_textures
            .get(&zone)
            .unwrap_or_else(|| panic!("{zone} should have LVZ texture files"));
        for (path, image) in references {
            if lvz_texture_name_for_image_filename(textures, &image).is_none() {
                unresolved.push(format!("{path} references {image} in {zone}"));
            }
            checked_count += 1;
        }
    }

    assert!(
        unresolved.is_empty(),
        "unresolved LVZ image references:\n{}",
        unresolved.join("\n")
    );
    assert!(
        checked_count > 0,
        "repository should include LVZ object image references"
    );
}

#[test]
fn repository_lvz_object_image_grids_match_texture_dimensions() {
    let mut zone_textures: BTreeMap<String, BTreeMap<String, (usize, usize)>> = BTreeMap::new();
    let mut zone_image_references: BTreeMap<String, Vec<(String, LvzImageDefinition)>> =
        BTreeMap::new();

    for path in repository_lvz_paths() {
        let zone = lvz_zone_key(&path);
        let data = std::fs::read(&path).expect("LVZ file should be readable");
        let archive =
            LvzArchive::parse(&data).unwrap_or_else(|err| panic!("{path} should parse: {err}"));
        for section in archive.into_sections() {
            match section.kind {
                LvzSectionKind::File => {
                    let name = normalize_asset_name(&section.filename);
                    if let Some(dimensions) = lvz_test_image_dimensions(&name, &section.data) {
                        zone_textures
                            .entry(zone.clone())
                            .or_default()
                            .insert(name, dimensions);
                    }
                }
                LvzSectionKind::Objects(objects) => {
                    for image in objects.images {
                        let name = normalize_asset_name(&image.filename);
                        if name.ends_with(".bm2") || lvz_raster_extension(&name) {
                            zone_image_references
                                .entry(zone.clone())
                                .or_default()
                                .push((path.clone(), image));
                        }
                    }
                }
            }
        }
    }

    let mut checked_count = 0;
    let mut invalid = Vec::new();
    for (zone, references) in zone_image_references {
        let textures = zone_textures
            .get(&zone)
            .unwrap_or_else(|| panic!("{zone} should have LVZ texture files"));
        for (path, image) in references {
            let name = normalize_asset_name(&image.filename);
            let Some(texture_name) = lvz_texture_name_for_image_filename(textures, &name) else {
                invalid.push(format!("{path} references missing image {name}"));
                continue;
            };
            let (width, height) = textures
                .get(texture_name)
                .copied()
                .expect("resolved texture should have dimensions");
            let x_count = image.x_count;
            let y_count = image.y_count;
            if x_count <= 0 || y_count <= 0 {
                invalid.push(format!(
                    "{path}:{name} uses non-positive LVZ frame grid {x_count}x{y_count}"
                ));
                continue;
            }
            if image.animation_time < 0 {
                invalid.push(format!(
                    "{path}:{name} uses negative LVZ animation time {}",
                    image.animation_time
                ));
            }
            if width % x_count as usize != 0 || height % y_count as usize != 0 {
                invalid.push(format!(
                    "{path}:{name} texture {width}x{height} is not divisible by LVZ frame grid {x_count}x{y_count}"
                ));
            }
            checked_count += 1;
        }
    }

    assert!(
        invalid.is_empty(),
        "invalid LVZ object image grid definitions:\n{}",
        invalid.join("\n")
    );
    assert!(
        checked_count > 0,
        "repository should include LVZ object image grids"
    );
}

#[test]
fn repository_lvz_screen_references_are_supported_after_version_decode() {
    let mut checked_count = 0;
    let mut unsupported = Vec::new();

    for path in repository_lvz_paths() {
        let data = std::fs::read(&path).expect("LVZ file should be readable");
        let archive =
            LvzArchive::parse(&data).unwrap_or_else(|err| panic!("{path} should parse: {err}"));
        for section in archive.into_sections() {
            let LvzSectionKind::Objects(objects) = section.kind else {
                continue;
            };
            for object in objects.objects {
                if object.map_object {
                    continue;
                }
                checked_count += 1;
                if object.x_type >= LVZ_SCREEN_REFERENCE_POINT_COUNT
                    || object.y_type >= LVZ_SCREEN_REFERENCE_POINT_COUNT
                {
                    unsupported.push(format!(
                        "{path} object {} uses unsupported screen refs ({}, {})",
                        object.object_id, object.x_type, object.y_type
                    ));
                }
            }
        }
    }

    assert!(
        unsupported.is_empty(),
        "unsupported LVZ screen reference points:\n{}",
        unsupported.join("\n")
    );
    assert!(
        checked_count > 0,
        "repository should include LVZ screen objects"
    );
}

#[test]
fn lvz_texture_lookup_falls_back_to_unique_matching_stem() {
    let textures = BTreeMap::from([
        ("tut1.jpg".to_string(), ()),
        ("folder/tut2.gif".to_string(), ()),
    ]);

    assert_eq!(
        lvz_texture_name_for_image_filename(&textures, "tut1.gif"),
        Some("tut1.jpg")
    );
    assert_eq!(
        lvz_texture_name_for_image_filename(&textures, "folder/tut2.gif"),
        Some("folder/tut2.gif")
    );
    assert_eq!(
        lvz_texture_name_for_image_filename(&textures, "missing.gif"),
        None
    );
}

#[test]
fn lvz_texture_lookup_rejects_ambiguous_matching_stems() {
    let textures = BTreeMap::from([
        ("banner.gif".to_string(), ()),
        ("folder/banner.jpg".to_string(), ()),
    ]);

    assert_eq!(
        lvz_texture_name_for_image_filename(&textures, "banner.png"),
        None
    );
}

#[test]
fn generated_lvz_manifest_matches_repository_archives() {
    let mut paths = repository_lvz_paths();
    sort_manifest_paths(&mut paths);

    assert_eq!(LVZ_PATHS, paths.as_slice());
    assert!(
        LVZ_PATHS
            .first()
            .is_some_and(|path| path.starts_with("assets/zones/chaos-zone/"))
    );
    assert!(LVZ_PATHS.contains(&"assets/zones/chaos-zone/aiq.lvz"));
}

#[test]
fn generated_zone_settings_manifest_matches_repository_settings_files() {
    let mut paths = repository_zone_settings_paths();
    sort_manifest_paths(&mut paths);

    assert_eq!(ZONE_SETTINGS_PATHS, paths.as_slice());
    assert!(
        ZONE_SETTINGS_PATHS
            .first()
            .is_some_and(|path| *path == "assets/zones/chaos-zone/arena.conf")
    );
}

#[test]
fn zone_dat_is_original_directory_listing_not_arena_settings() {
    let data = std::fs::read_to_string("assets/zones/zone.dat")
        .expect("zone directory file should be readable");

    assert!(data.starts_with("Battlestar Galactica,"));
    assert!(data.contains("ss://"));
}

#[test]
fn finds_open_spawn_for_all_repository_levels() {
    for path in repository_level_paths() {
        let data = std::fs::read(&path).expect("level file should be readable");
        let map = ArenaMap::from_lvl(&path, &data)
            .unwrap_or_else(|err| panic!("{path} should parse: {err}"));
        let spawn = map.spawn_position();

        assert!(
            !map.is_solid_world(spawn),
            "{} spawn should be open, got {spawn:?}",
            path
        );
    }
}

fn repository_level_paths() -> Vec<String> {
    let mut paths = Vec::new();
    collect_repository_paths(std::path::Path::new("levels"), "lvl", &mut paths);
    collect_repository_paths(std::path::Path::new("assets/zones"), "lvl", &mut paths);
    paths
}

fn repository_lvz_paths() -> Vec<String> {
    let mut paths = Vec::new();
    collect_repository_paths(std::path::Path::new("levels"), "lvz", &mut paths);
    collect_repository_paths(std::path::Path::new("assets/zones"), "lvz", &mut paths);
    paths
}

fn repository_zone_settings_paths() -> Vec<String> {
    let mut paths = Vec::new();
    for extension in ["cfg", "conf", "ini", "set"] {
        collect_repository_paths(std::path::Path::new("levels"), extension, &mut paths);
        collect_repository_paths(std::path::Path::new("assets/zones"), extension, &mut paths);
    }
    paths
}

fn lvz_raster_extension(name: &str) -> bool {
    [".bmp", ".png", ".gif", ".jpg", ".jpeg"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

fn lvz_test_image_dimensions(name: &str, data: &[u8]) -> Option<(usize, usize)> {
    if name.ends_with(".bm2") {
        return decode_bm2(data, Transparency::Black)
            .ok()
            .map(|decoded| (decoded.width, decoded.height));
    }
    if lvz_raster_extension(name) {
        return decode_lvz_raster_image(data)
            .ok()
            .map(|image| (image.width() as usize, image.height() as usize));
    }
    None
}

fn lvz_zone_key(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

fn collect_repository_paths(root: &std::path::Path, extension: &str, paths: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };

    for entry in entries {
        let path = entry.expect("level entry should be readable").path();
        if path.is_dir() {
            collect_repository_paths(&path, extension, paths);
        } else if path
            .extension()
            .and_then(|path_ext| path_ext.to_str())
            .is_some_and(|path_ext| path_ext.eq_ignore_ascii_case(extension))
        {
            paths.push(path.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn sort_manifest_paths(paths: &mut [String]) {
    paths.sort_by(|left, right| {
        manifest_path_priority(left)
            .cmp(&manifest_path_priority(right))
            .then_with(|| left.cmp(right))
    });
}

fn manifest_path_priority(path: &str) -> u8 {
    if path.starts_with("assets/zones/chaos-zone/") {
        0
    } else if path.starts_with("assets/zones/") {
        1
    } else {
        2
    }
}

#[test]
fn radius_collision_detects_nearby_solid_tiles() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.tiles[10 * map.width + 10] = 1;

    assert!(!map.can_fit_world(vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE), SHIP_RADIUS));
    assert!(!map.can_fit_world(vec2(9.5 * TILE_SIZE, 10.5 * TILE_SIZE), SHIP_RADIUS));
    assert!(map.can_fit_world(vec2(7.0 * TILE_SIZE, 10.5 * TILE_SIZE), SHIP_RADIUS));
}

#[test]
fn bouncing_point_reflects_velocity_on_solid_axis() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.tiles[10 * map.width + 11] = 1;

    let mut pos = vec2(10.5 * TILE_SIZE, 10.5 * TILE_SIZE);
    let mut vel = vec2(64.0, 0.0);

    assert!(move_bouncing_point_for_frequency(
        &map,
        &mut pos,
        &mut vel,
        0.25,
        0,
        &[],
    ));
    assert!(pos.x < 11.0 * TILE_SIZE);
    assert_eq!(pos.y, 10.5 * TILE_SIZE);
    assert_eq!(vel, vec2(-64.0, 0.0));

    let mut fast_pos = vec2(10.5 * TILE_SIZE, 12.5 * TILE_SIZE);
    let mut fast_vel = vec2(400.0, 0.0);
    map.tiles[12 * map.width + 11] = 1;

    assert!(move_bouncing_point_for_frequency(
        &map,
        &mut fast_pos,
        &mut fast_vel,
        0.08,
        0,
        &[],
    ));
    assert!(fast_pos.x < 11.0 * TILE_SIZE);
    assert_eq!(fast_pos.y, 12.5 * TILE_SIZE);
    assert_eq!(fast_vel, vec2(-400.0, 0.0));

    let mut linear_pos = vec2(10.5 * TILE_SIZE, 14.5 * TILE_SIZE);
    let linear_vel = vec2(400.0, 0.0);
    map.tiles[14 * map.width + 11] = 1;

    assert!(move_point_until_solid_for_frequency(
        &map,
        &mut linear_pos,
        linear_vel,
        0.08,
        0,
        &[],
    ));
    assert!((linear_pos.x - 10.75 * TILE_SIZE).abs() < 0.001);
    assert_eq!(linear_pos.y, 14.5 * TILE_SIZE);

    let mut diagonal_pos = vec2(10.9 * TILE_SIZE, 16.5 * TILE_SIZE);
    let diagonal_vel = vec2(400.0, 160.0);
    map.tiles[16 * map.width + 11] = 1;

    assert!(move_point_until_solid_for_frequency(
        &map,
        &mut diagonal_pos,
        diagonal_vel,
        0.01,
        0,
        &[],
    ));
    assert_eq!(diagonal_pos.x, 10.9 * TILE_SIZE);
    assert!((diagonal_pos.y - (16.5 * TILE_SIZE + 1.6)).abs() < 0.001);
}

#[test]
fn recognizes_safe_tile_at_world_position() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.tiles[12 * map.width + 11] = SAFE_TILE_ID;

    assert_eq!(
        map.tile_at_world(vec2(11.5 * TILE_SIZE, 12.5 * TILE_SIZE)),
        SAFE_TILE_ID
    );
    assert!(!map.is_solid_world(vec2(11.5 * TILE_SIZE, 12.5 * TILE_SIZE)));
}

#[test]
fn temporary_brick_tiles_are_solid_and_restorable() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.set_tile(7, 8, BRICK_TILE_ID);

    assert_eq!(map.tile(7, 8), BRICK_TILE_ID);
    assert!(map.is_solid_world(tile_center(ivec2(7, 8))));

    map.set_tile(7, 8, 0);
    assert_eq!(map.tile(7, 8), 0);
    assert!(!map.is_solid_world(tile_center(ivec2(7, 8))));
}

#[test]
fn team_brick_collision_can_be_treated_as_passable() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.set_tile(7, 8, BRICK_TILE_ID);
    let pos = tile_center(ivec2(7, 8));

    assert!(!map.can_fit_world(pos, 1.0));
    assert!(can_fit_world_with_passable_tiles(
        &map,
        pos,
        1.0,
        &[ivec2(7, 8)]
    ));
    assert!(!can_fit_world_with_passable_tiles(
        &map,
        pos,
        1.0,
        &[ivec2(6, 8)]
    ));

    let bricks = vec![Brick {
        tile: ivec2(7, 8),
        previous_tile: 0,
        team: 5,
        ttl: 10.0,
        age: 0.0,
        server_timestamp: None,
    }];
    assert!(!is_solid_world_for_frequency(&map, pos, 5, &bricks));
    assert!(is_solid_world_for_frequency(&map, pos, 6, &bricks));
    assert!(shrapnel_spawn_allowed(&map, pos, 5, &bricks));
    assert!(!shrapnel_spawn_allowed(&map, pos, 6, &bricks));

    let mut pass_pos = tile_center(ivec2(6, 8));
    let mut pass_vel = vec2(TILE_SIZE, 0.0);
    assert!(!move_bouncing_point_for_frequency(
        &map,
        &mut pass_pos,
        &mut pass_vel,
        1.0,
        5,
        &bricks,
    ));
    assert_eq!(world_to_tile(pass_pos), ivec2(7, 8));

    let mut bounce_pos = tile_center(ivec2(6, 8));
    let mut bounce_vel = vec2(TILE_SIZE, 0.0);
    assert!(move_bouncing_point_for_frequency(
        &map,
        &mut bounce_pos,
        &mut bounce_vel,
        1.0,
        6,
        &bricks,
    ));
    assert_eq!(world_to_tile(bounce_pos), ivec2(6, 8));
    assert_eq!(bounce_vel.x, -TILE_SIZE);

    assert!(brick_tile_overlaps_ship_aabb(
        ivec2(7, 8),
        tile_center(ivec2(7, 8)),
        SHIP_RADIUS,
    ));
    assert!(!brick_tile_overlaps_ship_aabb(
        ivec2(7, 8),
        tile_center(ivec2(9, 8)),
        1.0,
    ));

    let approach = vec2(7.0 * TILE_SIZE - SHIP_RADIUS - 1.0, 8.5 * TILE_SIZE);
    let mut enemy_pos = approach;
    let mut enemy_velocity = vec2(100.0, 0.0);
    assert!(move_entity_with_collision_and_passable_tiles(
        &map,
        &mut enemy_pos,
        &mut enemy_velocity,
        SHIP_RADIUS,
        0.01,
        0.5,
        &[],
    ));
    assert_eq!(enemy_pos, approach);
    assert_eq!(enemy_velocity, vec2(-50.0, 0.0));

    let mut team_pos = approach;
    let mut team_velocity = vec2(100.0, 0.0);
    assert!(!move_entity_with_collision_and_passable_tiles(
        &map,
        &mut team_pos,
        &mut team_velocity,
        SHIP_RADIUS,
        0.01,
        0.5,
        &[ivec2(7, 8)],
    ));
    assert_eq!(team_pos.x, approach.x + 1.0);
    assert_eq!(team_velocity, vec2(100.0, 0.0));
}

#[test]
fn client_live_frame_uses_one_full_dt_pair_for_wall_brick_door_and_bounds() {
    let run = |map: &ArenaMap, passable: &[IVec2]| {
        let mut position = vec2(31.0, 56.0);
        let mut velocity = vec2(200.0, 0.0);
        let bounced = move_entity_with_collision_and_passable_tiles(
            map,
            &mut position,
            &mut velocity,
            14.0,
            1.0 / 60.0,
            0.5,
            passable,
        );
        (position, velocity, bounced)
    };

    let mut wall = ArenaMap::fallback_skirmish();
    wall.tiles.fill(0);
    wall.set_tile(3, 3, 1);
    let (wall_position, wall_velocity, wall_bounced) = run(&wall, &[]);
    assert!(wall_bounced);
    assert_eq!(wall_position, vec2(31.0, 56.0));
    assert_eq!(wall_velocity, vec2(-100.0, 0.0));
    assert_eq!(
        crate::continuum_position::protocol_velocity_component(wall_velocity.x),
        -1_000,
    );

    let mut brick = ArenaMap::fallback_skirmish();
    brick.tiles.fill(0);
    brick.set_tile(3, 3, BRICK_TILE_ID);
    let (enemy_position, enemy_velocity, enemy_bounced) = run(&brick, &[]);
    assert!(enemy_bounced);
    assert_eq!(enemy_position, vec2(31.0, 56.0));
    assert_eq!(enemy_velocity, vec2(-100.0, 0.0));
    let (team_position, team_velocity, team_bounced) = run(&brick, &[ivec2(3, 3)]);
    assert!(!team_bounced);
    assert_eq!(team_position.x.to_bits(), 0x4209_5555);
    assert_eq!(team_velocity, vec2(200.0, 0.0));

    let mut door = ArenaMap::fallback_skirmish();
    door.tiles.fill(0);
    door.set_tile(3, 3, DOOR_FIRST_TILE_ID);
    door.door_seed = 1;
    let (closed_position, closed_velocity, closed_bounced) = run(&door, &[]);
    assert!(closed_bounced);
    assert_eq!(closed_position, vec2(31.0, 56.0));
    assert_eq!(closed_velocity, vec2(-100.0, 0.0));
    door.door_seed = 0;
    let (open_position, open_velocity, open_bounced) = run(&door, &[]);
    assert!(!open_bounced);
    assert_eq!(open_position.x.to_bits(), 0x4209_5555);
    assert_eq!(open_velocity, vec2(200.0, 0.0));

    let empty = ArenaMap::fallback_skirmish();
    let mut boundary_position = vec2(15.0, 56.0);
    let mut boundary_velocity = vec2(-200.0, 0.0);
    assert!(move_entity_with_collision(
        &empty,
        &mut boundary_position,
        &mut boundary_velocity,
        14.0,
        1.0 / 60.0,
        0.5,
    ));
    assert_eq!(boundary_position, vec2(15.0, 56.0));
    assert_eq!(boundary_velocity, vec2(100.0, 0.0));
}

#[test]
fn ship_movement_collision_uses_reference_game_pixel_rounded_aabb() {
    let mut map = ArenaMap::fallback_skirmish();
    map.tiles.fill(0);
    map.set_tile(10, 10, 1);
    let diagonal_corner = vec2(147.0, 147.0);

    assert!(map.can_fit_world(diagonal_corner, SHIP_RADIUS));
    assert!(ship_axis_collides_with_solid_or_bounds(
        &map,
        diagonal_corner,
        SHIP_RADIUS,
        &[],
        CollisionAxis::X,
        1.0,
    ));

    let mut pos = vec2(144.0, 147.0);
    let mut vel = vec2(300.0, 20.0);
    assert!(move_entity_with_collision(
        &map,
        &mut pos,
        &mut vel,
        SHIP_RADIUS,
        0.01,
        0.5,
    ));
    assert_eq!(pos.x, 144.0);
    // The reference game runs X before Y, so the X bounce scales the perpendicular
    // velocity before the Y-axis displacement is computed.
    assert_eq!(pos.y, 147.1);
    assert_eq!(vel.x, -150.0);
    assert_eq!(vel.y, 10.0);

    // The real map predicate retains the reference game's player-first operand order:
    // touching the wall's minimum edge overlaps, touching its maximum edge
    // does not. One pixel of penetration overlaps from either side after the
    // 1px (reference 1/16 tile) center truncation.
    assert!(ship_axis_collides_with_solid_or_bounds(
        &map,
        vec2(156.0, 168.0),
        4.0,
        &[],
        CollisionAxis::X,
        1.0,
    ));
    assert!(!ship_axis_collides_with_solid_or_bounds(
        &map,
        vec2(180.0, 168.0),
        4.0,
        &[],
        CollisionAxis::X,
        -1.0,
    ));
    assert!(ship_axis_collides_with_solid_or_bounds(
        &map,
        vec2(157.0, 168.0),
        4.0,
        &[],
        CollisionAxis::X,
        1.0,
    ));
    assert!(ship_axis_collides_with_solid_or_bounds(
        &map,
        vec2(179.0, 168.0),
        4.0,
        &[],
        CollisionAxis::X,
        -1.0,
    ));

    assert!(ship_axis_collides_with_solid_or_bounds(
        &map,
        vec2(SHIP_RADIUS - 1.0, 128.0),
        SHIP_RADIUS,
        &[],
        CollisionAxis::X,
        -1.0,
    ));
    assert!(ship_axis_collides_with_solid_or_bounds(
        &map,
        vec2(map.width as f32 * TILE_SIZE - SHIP_RADIUS, 128.0),
        SHIP_RADIUS,
        &[],
        CollisionAxis::X,
        1.0,
    ));

    let chaos_path = "assets/zones/chaos-zone/bdhadar.lvl";
    let chaos_data = std::fs::read(chaos_path).expect("Chaos level should be readable");
    let chaos = ArenaMap::from_lvl(chaos_path, &chaos_data).expect("Chaos level should parse");
    let (wall_x, wall_y) = (1..chaos.width - 1)
        .find_map(|y| {
            (1..chaos.width - 1).find_map(|x| {
                (is_solid_tile_with_doors(chaos.tile(x as i32, y as i32), chaos.door_seed)
                    && !is_solid_tile_with_doors(
                        chaos.tile(x as i32 - 1, y as i32),
                        chaos.door_seed,
                    ))
                .then_some((x, y))
            })
        })
        .expect("Chaos should have a static wall with a clear left approach");
    let chaos_candidate = vec2(
        wall_x as f32 * TILE_SIZE - SHIP_RADIUS,
        (wall_y as f32 + 0.5) * TILE_SIZE,
    );
    assert!(ship_axis_collides_with_solid_or_bounds(
        &chaos,
        chaos_candidate,
        SHIP_RADIUS,
        &[],
        CollisionAxis::X,
        1.0,
    ));
}

#[test]
fn browser_network_config_defaults_to_local_play_values() {
    let config = BrowserNetworkConfig::from_values("", "", "", "", "");

    assert_eq!(config.rtc_url, None);
    assert_eq!(config.login_name, "zubspace");
    assert_eq!(config.password, "");
    assert_eq!(config.player_identity, "000000000001");
    assert_eq!(config.arena_name, "");
}

#[test]
fn browser_network_config_preserves_query_values() {
    let config = BrowserNetworkConfig::from_values(
        "http://127.0.0.1:9001/v1/rtc/offer",
        "pilot",
        "secret",
        "123456789012",
        "pub",
    );

    assert_eq!(
        config.rtc_url.as_deref(),
        Some("http://127.0.0.1:9001/v1/rtc/offer")
    );
    assert_eq!(config.login_name, "pilot");
    assert_eq!(config.password, "secret");
    assert_eq!(config.player_identity, "123456789012");
    assert_eq!(config.arena_name, "pub");
    assert_eq!(
        browser_player_identity_parts(&config.player_identity),
        Some((3_197_704_724, 28))
    );
}

#[test]
fn browser_network_config_replaces_invalid_player_identity() {
    let config = BrowserNetworkConfig::from_values("", "", "", "not-a-player-id", "");

    assert_eq!(config.player_identity, "000000000001");
}

#[test]
fn chat_sound_ids_map_to_original_bong_assets() {
    assert_eq!(chat_sound_index(0), None);
    assert_eq!(chat_sound_index(1), Some(0));
    assert_eq!(chat_sound_index(26), Some(25));
    assert_eq!(chat_sound_index(27), None);
}

#[test]
fn login_response_messages_match_reference_game_reference_table() {
    assert_eq!(login_response_message(0x00), Some("Ok"));
    assert_eq!(login_response_message(0x01), Some("Unregistered player"));
    assert_eq!(login_response_message(0x02), Some("Bad password"));
    assert_eq!(
        login_response_message(0x14),
        Some("Restricted zone, mod access required")
    );
    assert_eq!(login_response_message(0x15), None);
}

#[test]
fn minimal_registration_response_uses_placeholder_profile() {
    let response = RegistrationFormResponse::minimal();
    let payload = response.encode_payload();

    assert_eq!(payload.len(), 766);
    assert_eq!(payload[0], RegistrationFormResponse::TYPE);
    assert_eq!(response.email, "zubspace@example.invalid");
    assert_eq!(response.sex, b'M');
    assert_eq!(response.age, 0);
    assert!(response.connection_home);
    assert!(!response.connection_work);
    assert!(!response.connection_school);
}

#[test]
fn registration_response_packets_use_small_reliable_chunks() {
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    let response = RegistrationFormResponse::minimal();
    let payload = response.encode_payload();

    let packets = session.send_registration_response_packets(&response);

    assert_eq!(packets, reliable_small_chunk_packets(0, &payload));
    assert!(
        packets
            .iter()
            .all(|packet| packet.len() <= ReliableSequencer::MAX_PACKET_SIZE)
    );
    assert_eq!(read_u32(&packets[0], 2).unwrap(), 0);
    assert_eq!(read_u32(&packets[1], 2).unwrap(), 1);
    assert_eq!(
        &packets[0][ReliableSequencer::RELIABLE_HEADER_SIZE..][..2],
        &[PacketChunker::CORE_BYTE, PacketChunker::SMALL_BODY_TYPE]
    );
    assert_eq!(
        &packets[1][ReliableSequencer::RELIABLE_HEADER_SIZE..][..2],
        &[PacketChunker::CORE_BYTE, PacketChunker::SMALL_TAIL_TYPE]
    );

    let mut chunker = PacketChunker::default();
    assert_eq!(
        chunker
            .push_small_chunk(&packets[0][ReliableSequencer::RELIABLE_HEADER_SIZE..])
            .unwrap(),
        None
    );
    assert_eq!(
        chunker
            .push_small_chunk(&packets[1][ReliableSequencer::RELIABLE_HEADER_SIZE..])
            .unwrap(),
        Some(payload)
    );
}
