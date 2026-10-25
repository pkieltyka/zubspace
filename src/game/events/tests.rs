use super::*;

fn test_chat_wave(data_len: usize, fill: u8) -> Arc<[u8]> {
    let mut wav = b"RIFF".to_vec();
    wav.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    wav.extend_from_slice(&22_050_u32.to_le_bytes());
    wav.extend_from_slice(&22_050_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&8_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_len as u32).to_le_bytes());
    wav.resize(wav.len() + data_len, fill);
    Arc::from(wav)
}

#[test]
fn runtime_lvz_aggregate_budget_is_checked_and_replacement_aware() {
    fn archive(size: usize) -> LvzArchive {
        LvzArchive::unvalidated_for_usage_test(vec![crate::lvz::LvzSection {
            filename: "asset.bin".into(),
            timestamp: 1,
            data: Arc::from(vec![0; size]),
            kind: crate::lvz::LvzSectionKind::File,
        }])
    }

    let mut archives = BTreeMap::new();
    archives.insert("a.lvz".to_string(), archive(1));
    let order = vec!["a.lvz".to_string(), "b.lvz".to_string()];
    let one_archive_bytes = archive(1).render_usage().unwrap().retained_bytes().unwrap();
    let two_byte_archive_bytes = archive(2).render_usage().unwrap().retained_bytes().unwrap();
    assert_eq!(
        admitted_zone_render_total_with_replacement(
            &archives,
            &order,
            "b.lvz",
            &archive(1),
            crate::zone_wire::MAX_ZONE_RENDER_BYTES - 2 * one_archive_bytes,
            0,
        ),
        Ok(crate::zone_wire::MAX_ZONE_RENDER_BYTES)
    );
    assert!(
        admitted_zone_render_total_with_replacement(
            &archives,
            &order,
            "b.lvz",
            &archive(2),
            crate::zone_wire::MAX_ZONE_RENDER_BYTES - 2 * one_archive_bytes,
            0,
        )
        .is_err()
    );
    assert_eq!(
        admitted_zone_render_total_with_replacement(
            &archives,
            &["a.lvz".to_string()],
            "a.lvz",
            &archive(2),
            crate::zone_wire::MAX_ZONE_RENDER_BYTES - two_byte_archive_bytes,
            0,
        ),
        Ok(crate::zone_wire::MAX_ZONE_RENDER_BYTES),
        "replacing an archive must not double-count the old decoded data"
    );
}

#[test]
fn cycling_sixty_four_downloaded_zones_replaces_one_runtime_map_slot() {
    let mut maps = vec![ArenaMap::from_lvl("embedded.lvl", &[]).unwrap()];
    for index in 0..64 {
        let replacement = ArenaMap::from_lvl(&format!("zone-{index}.lvl"), &[]).unwrap();
        assert_eq!(replace_current_downloaded_map(&mut maps, 0, replacement), 0);
        assert_eq!(maps.len(), 1);
    }
    assert_eq!(maps[0].name, "zone-63.lvl");
}

#[test]
fn dynamic_chat_sound_sources_are_bounded_shared_and_last_wins_across_lifecycle_rebuilds() {
    fn archive(sound: Arc<[u8]>) -> LvzArchive {
        LvzArchive::validated_for_test(vec![crate::lvz::LvzSection {
            filename: "bong27.wa2".into(),
            timestamp: 1,
            data: sound,
            kind: crate::lvz::LvzSectionKind::File,
        }])
        .unwrap()
    }
    let first = test_chat_wave(16, 0x11);
    let second = test_chat_wave(16, 0x22);
    let mut archives = BTreeMap::new();
    archives.insert("z-first.lvz".to_string(), archive(Arc::clone(&first)));
    archives.insert("a-second.lvz".to_string(), archive(Arc::clone(&second)));
    let mut order = vec!["z-first.lvz".to_string(), "a-second.lvz".to_string()];
    let sources = dynamic_lvz_chat_sound_sources(&archives, &order, None).unwrap();
    assert_eq!(sources.len(), 1);
    assert!(Arc::ptr_eq(sources.get(&27).unwrap(), &second));

    archives.clear();
    order.clear();
    assert!(
        dynamic_lvz_chat_sound_sources(&archives, &order, None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn lvz_merge_accounting_is_cumulative_and_keeps_the_worst_install() {
    // A browser gate diffs these counters across an arena switch, so they must
    // accumulate across installs and retain the single worst one rather than the
    // latest or the average.
    let mut game = Game::new_network_authority_test();
    assert_eq!(game.lvz_merge_count, 0);
    assert_eq!(game.lvz_merge_max_ms, 0.0);

    let started = lvz_merge_clock();
    std::thread::sleep(std::time::Duration::from_millis(20));
    game.account_lvz_merge(started);
    assert_eq!(game.lvz_merge_count, 1);
    let first = game.lvz_merge_total_ms;
    assert!(
        first >= 15.0,
        "a 20ms install must be accounted, saw {first}"
    );

    game.account_lvz_merge(lvz_merge_clock());
    assert_eq!(game.lvz_merge_count, 2);
    assert!(
        game.lvz_merge_total_ms >= first,
        "a second install adds, it never replaces: {first} -> {}",
        game.lvz_merge_total_ms
    );

    let slower_start = lvz_merge_clock();
    std::thread::sleep(std::time::Duration::from_millis(30));
    game.account_lvz_merge(slower_start);
    assert_eq!(game.lvz_merge_count, 3);
    assert!(
        game.lvz_merge_max_ms >= 25.0,
        "a slower install must raise the worst case, saw {}",
        game.lvz_merge_max_ms
    );
    assert!(
        game.lvz_merge_max_ms < game.lvz_merge_total_ms,
        "the maximum stays the worst install, not the sum"
    );
}

#[test]
fn unchanged_dynamic_chat_sounds_do_not_restart_pending_or_loaded_audio() {
    let mut game = Game::new_network_authority_test();
    let wave = test_chat_wave(16, 0x11);
    game.dynamic_lvz_chat_sound_generation = 7;
    game.dynamic_lvz_chat_sound_sources = BTreeMap::from([(27, Arc::clone(&wave))]);
    game.dynamic_lvz_chat_sound_loaded_ids.insert(27);
    // Same bytes from a separately parsed archive are still the same sound.
    // The headless fixture also proves that this path never touches audio/GPU
    // assets merely because another (non-audio) archive was installed.
    game.install_dynamic_lvz_chat_sounds(BTreeMap::from([(27, Arc::from(wave.as_ref()))]));
    assert_eq!(game.dynamic_lvz_chat_sound_generation, 7);
    assert!(game.dynamic_lvz_chat_sound_loaded_ids.contains(&27));
    game.dynamic_lvz_chat_sound_loaded_ids.clear();
    game.install_dynamic_lvz_chat_sounds(BTreeMap::from([(27, wave)]));
    assert_eq!(game.dynamic_lvz_chat_sound_generation, 7);
}

#[test]
fn changed_dynamic_chat_sounds_retire_old_generation_on_replace_remove_and_map_reset() {
    let mut game = Game::new_network_authority_test();
    let first = test_chat_wave(16, 0x11);
    let second = test_chat_wave(16, 0x22);
    assert_eq!(
        game.replace_dynamic_lvz_chat_sound_sources(BTreeMap::new()),
        None
    );
    assert_eq!(
        game.replace_dynamic_lvz_chat_sound_sources(BTreeMap::from([(27, first)])),
        Some(1)
    );
    game.dynamic_lvz_chat_sound_loaded_ids.insert(27);
    assert_eq!(
        game.replace_dynamic_lvz_chat_sound_sources(BTreeMap::from([(27, Arc::clone(&second))])),
        Some(2)
    );
    assert!(game.dynamic_lvz_chat_sound_loaded_ids.is_empty());
    assert_eq!(game.dynamic_lvz_chat_sound_sources[&27], second);
    assert_eq!(
        game.replace_dynamic_lvz_chat_sound_sources(BTreeMap::new()),
        Some(3)
    );
    assert!(game.dynamic_lvz_chat_sound_sources.is_empty());
    assert_eq!(
        game.replace_dynamic_lvz_chat_sound_sources(BTreeMap::new()),
        None
    );
    game.clear_dynamic_lvz_chat_sounds();
    assert_eq!(game.dynamic_lvz_chat_sound_generation, 4);
    assert_eq!(
        game.replace_dynamic_lvz_chat_sound_sources(BTreeMap::from([(27, second)])),
        Some(5),
        "a new map must load its own sounds even if the previous map had the same bytes"
    );
}

#[test]
fn dynamic_chat_sound_aggregate_pcm_budget_rejects_max_plus_one_file() {
    let mut sections = Vec::new();
    for sound_id in 1..=9 {
        sections.push(crate::lvz::LvzSection {
            filename: Arc::from(format!("bong{sound_id}.wa2")),
            timestamp: 1,
            data: test_chat_wave(
                crate::zone_wire::MAX_LVZ_CHAT_SOUND_PCM_BYTES_PER_FILE / 4,
                sound_id,
            ),
            kind: crate::lvz::LvzSectionKind::File,
        });
    }
    let archive = LvzArchive::unvalidated_for_usage_test(sections);
    assert!(archive.render_usage().is_err());
}

#[test]
fn multifire_prize_grants_capability_without_toggling_active_state() {
    let mut capable = false;
    let mut active = false;

    apply_multifire_prize(&mut capable, &mut active, false);
    assert!(capable);
    assert!(!active);

    active = true;
    apply_multifire_prize(&mut capable, &mut active, true);
    assert!(!capable);
    assert!(!active);
}

#[test]
fn unchanged_numeric_prize_values_report_reference_game_min_max_notifications() {
    assert_eq!(
        changed_prize_notification(PrizeKind::Rocket, false, 4_u32, 5_u32),
        Some(PrizeNotification::new(PrizeKind::Rocket, false))
    );
    assert_eq!(
        changed_prize_notification(PrizeKind::Rocket, false, 5_u32, 5_u32),
        Some(PrizeNotification::limited(PrizeKind::Rocket, false))
    );
    assert_eq!(
        prize_notification_text_for_entry(
            PrizeNotification::limited(PrizeKind::Rocket, false),
            false,
        ),
        "Rocket Increased. MAX"
    );
    assert_eq!(
        prize_notification_text_for_entry(
            PrizeNotification::limited(PrizeKind::Rocket, true),
            true,
        ),
        "DAMAGE: Rocket lost. MIN"
    );
    assert_eq!(
        prize_notification_text_for_entry(PrizeNotification::new(PrizeKind::Rocket, true), true,),
        "DAMAGE: Rocket lost.",
        "an authoritative damage-loss notification carries the reference prefix exactly once"
    );
}

#[test]
fn unchanged_capability_prize_values_report_reference_game_min_max_notifications() {
    assert_eq!(
        capability_prize_notification_entry(PrizeKind::BouncingBullets, false, false),
        Some(PrizeNotification::new(PrizeKind::BouncingBullets, false))
    );
    assert_eq!(
        capability_prize_notification_entry(PrizeKind::BouncingBullets, true, false),
        Some(PrizeNotification::limited(
            PrizeKind::BouncingBullets,
            false
        ))
    );
    assert_eq!(
        capability_prize_notification_entry(PrizeKind::BouncingBullets, true, true),
        Some(PrizeNotification::new(PrizeKind::BouncingBullets, true))
    );
    assert_eq!(
        capability_prize_notification_entry(PrizeKind::BouncingBullets, false, true),
        Some(PrizeNotification::limited(PrizeKind::BouncingBullets, true))
    );
    assert_eq!(
        prize_notification_text_for_entry(
            PrizeNotification::limited(PrizeKind::BouncingBullets, false),
            false,
        ),
        "Bouncing bullets. MAX"
    );
    assert_eq!(
        prize_notification_text_for_entry(
            PrizeNotification::limited(PrizeKind::BouncingBullets, true),
            false,
        ),
        "Bouncing bullets lost. MIN"
    );
}

#[test]
fn unchanged_status_prize_values_report_reference_game_min_max_notifications() {
    assert_eq!(
        status_prize_notification_entry(PrizeKind::Stealth, true, false, false),
        Some(PrizeNotification::new(PrizeKind::Stealth, false))
    );
    assert_eq!(
        status_prize_notification_entry(PrizeKind::Stealth, true, true, false),
        Some(PrizeNotification::limited(PrizeKind::Stealth, false))
    );
    assert_eq!(
        status_prize_notification_entry(PrizeKind::Stealth, true, true, true),
        Some(PrizeNotification::new(PrizeKind::Stealth, true))
    );
    assert_eq!(
        status_prize_notification_entry(PrizeKind::Stealth, true, false, true),
        Some(PrizeNotification::limited(PrizeKind::Stealth, true))
    );
    assert_eq!(
        status_prize_notification_entry(PrizeKind::Stealth, false, false, false),
        Some(PrizeNotification::new(PrizeKind::FullCharge, false))
    );
    assert_eq!(
        prize_notification_text_for_entry(
            PrizeNotification::limited(PrizeKind::Stealth, false),
            false,
        ),
        "Stealth available. MAX"
    );
}
