use super::*;

mod graphics;
mod progress;
mod sounds;

use progress::{draw_loading_frame, should_present_loading_progress};

pub(crate) use progress::hide_loading_overlay;
#[cfg(target_arch = "wasm32")]
pub(crate) use progress::set_loading_status;

impl Assets {
    pub(crate) async fn load() -> Self {
        #[cfg(target_arch = "wasm32")]
        let target = crate::transport::configured_browser_network_config()
            .map(|config| config.arena_name)
            .unwrap_or_else(|_| "chaos-zone".to_owned());
        #[cfg(target_arch = "wasm32")]
        let paths = startup_zone_paths(Some(&target));
        #[cfg(not(target_arch = "wasm32"))]
        let paths = startup_zone_paths(None);
        let zone_asset_count = paths.levels.len() + paths.lvz.len() + paths.settings.len();
        draw_loading_frame("Loading assets", 0, zone_asset_count).await;

        let graphics::LoadedGraphics {
            ships,
            turrets,
            tiles,
            backgrounds,
            stars,
            radar_h,
            radar_v,
            health_bar,
            tall_font,
            energy_font,
            icons,
            icon_counts,
            bullets,
            bullet_trail,
            bullet_impact,
            bombs,
            bomb_flash,
            bomb_trail,
            mines,
            shrapnel,
            bricks,
            repel,
            portal,
            warp,
            ship_explosion,
            bomb_explosion,
            emp_burst,
            emp_spark,
            exhaust,
            rocket,
            super_effect,
            shield_effect,
            powerball,
            goal,
            flag,
            dropped_flag,
            asteroid_small1,
            asteroid_small2,
            asteroid_large,
            station,
            wormhole,
            prizes,
        } = graphics::load_original_graphics();
        draw_loading_frame("Loading zones", 0, zone_asset_count).await;

        // Start a bounded window together, then parse in stable manifest order.
        // In browsers the menu prefetch and this loader share the same bytes.
        let mut files = load_zone_files(paths).await;
        let mut maps = Vec::new();
        let mut map_tilesets = Vec::new();
        let mut loaded_zone_assets = 0;
        for path in paths.levels {
            match files
                .remove(*path)
                .expect("every startup path has a result")
            {
                Ok(bytes) => match ArenaMap::from_lvl(path, &bytes) {
                    Ok(map) => match texture_from_lvl_embedded_tileset(&bytes) {
                        Ok(tileset) => {
                            map_tilesets.push(tileset);
                            maps.push(map);
                        }
                        Err(err) => {
                            tracing::warn!(path, error = %err, "failed to decode level tileset")
                        }
                    },
                    Err(err) => {
                        tracing::warn!(path, error = %err, "failed to parse level asset")
                    }
                },
                Err(err) => tracing::warn!(path, error = ?err, "failed to load level asset"),
            }
            loaded_zone_assets += 1;
            if should_present_loading_progress(loaded_zone_assets, zone_asset_count) {
                draw_loading_frame("Loading zones", loaded_zone_assets, zone_asset_count).await;
            }
        }
        let mut lvz_archives = Vec::new();
        let mut lvz_chat_sounds = Vec::new();
        for path in paths.lvz {
            match files
                .remove(*path)
                .expect("every startup path has a result")
            {
                Ok(bytes) => match LvzArchive::parse(&bytes) {
                    Ok(archive) => {
                        if should_decode_lvz_chat_sounds(path) {
                            for section in archive.sections() {
                                let Some(sound_id) = lvz_chat_sound_id(&section.filename) else {
                                    continue;
                                };
                                if let Ok(sound) = load_sound_from_bytes(&section.data).await {
                                    lvz_chat_sounds.push((path.to_string(), sound_id, sound));
                                }
                            }
                        }
                        lvz_archives.push((path.to_string(), archive));
                    }
                    Err(err) => tracing::warn!(path, error = %err, "failed to parse lvz asset"),
                },
                Err(err) => tracing::warn!(path, error = ?err, "failed to load lvz asset"),
            }
            loaded_zone_assets += 1;
            if should_present_loading_progress(loaded_zone_assets, zone_asset_count) {
                draw_loading_frame("Loading zones", loaded_zone_assets, zone_asset_count).await;
            }
        }
        let mut zone_tunings = Vec::new();
        for path in paths.settings {
            match files
                .remove(*path)
                .expect("every startup path has a result")
            {
                Ok(bytes) => match std::str::from_utf8(&bytes) {
                    Ok(text) => match arena_tuning_from_subgame_settings_text(text) {
                        Ok(tuning) => zone_tunings.push((path.to_string(), tuning)),
                        Err(err) => {
                            tracing::warn!(path, error = %err, "failed to parse settings asset")
                        }
                    },
                    Err(err) => {
                        tracing::warn!(path, error = %err, "failed to decode settings asset")
                    }
                },
                Err(err) => tracing::warn!(path, error = ?err, "failed to load settings asset"),
            }
            loaded_zone_assets += 1;
            if should_present_loading_progress(loaded_zone_assets, zone_asset_count) {
                draw_loading_frame("Loading zones", loaded_zone_assets, zone_asset_count).await;
            }
        }

        let sounds::LoadedSounds {
            thrust,
            reverse_thrust,
            rocket_sound,
            rocket_sustain_sound,
            gun_fire,
            bounce_sound,
            multifire_on,
            multifire_off,
            burst_sound,
            stealth_sound,
            cloak_sound,
            xradar_sound,
            antiwarp_sound,
            toggle_off_sound,
            bullet_impact_sound,
            prize_sound,
            energy_alarm_sound,
            flag_sound,
            goal_sound,
            ball_bounce_sound,
            ball_catch_sound,
            ball_throw_sound,
            bomb_fire,
            emp_bomb_fire,
            mine_fire,
            decoy_sound,
            thor_sound,
            repel_sound,
            wall_sound,
            portal_sound,
            warp_sound,
            victory_music,
            victory_music_long,
            ship_explode,
            bomb_explode,
            emp_bomb_explode,
            chat_sounds,
        } = sounds::load_original_sounds().await;

        Self {
            ships,
            turrets,
            tiles,
            _backgrounds: backgrounds,
            stars,
            radar_h,
            radar_v,
            health_bar,
            tall_font,
            energy_font,
            icons,
            icon_counts,
            bullets,
            bullet_trail,
            bullet_impact,
            bombs,
            bomb_flash,
            bomb_trail,
            mines,
            shrapnel,
            bricks,
            repel,
            portal,
            warp,
            ship_explosion,
            bomb_explosion,
            emp_burst,
            emp_spark,
            exhaust,
            rocket,
            super_effect,
            shield_effect,
            powerball,
            goal,
            flag,
            dropped_flag,
            asteroid_small1,
            asteroid_small2,
            asteroid_large,
            station,
            wormhole,
            prizes,
            maps,
            map_tilesets,
            lvz_archives,
            lvz_chat_sounds,
            zone_tunings,
            thrust,
            reverse_thrust,
            rocket_sound,
            rocket_sustain_sound,
            gun_fire,
            bounce_sound,
            multifire_on,
            multifire_off,
            burst_sound,
            stealth_sound,
            cloak_sound,
            xradar_sound,
            antiwarp_sound,
            toggle_off_sound,
            bullet_impact_sound,
            prize_sound,
            energy_alarm_sound,
            flag_sound,
            goal_sound,
            ball_bounce_sound,
            ball_catch_sound,
            ball_throw_sound,
            bomb_fire,
            emp_bomb_fire,
            mine_fire,
            decoy_sound,
            thor_sound,
            repel_sound,
            wall_sound,
            portal_sound,
            warp_sound,
            victory_music,
            victory_music_long,
            ship_explode,
            bomb_explode,
            emp_bomb_explode,
            chat_sounds,
        }
    }
}

fn texture_from_lvl_embedded_tileset(data: &[u8]) -> Result<Option<Texture2D>, String> {
    let Some(bitmap) = lvl_embedded_bitmap(data)? else {
        return Ok(None);
    };
    let texture = try_texture_from_bm2(bitmap, Transparency::Opaque)?;
    texture.set_filter(FilterMode::Nearest);
    Ok(Some(texture))
}

fn browser_asset_path(path: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        path.replace('#', "%23")
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        path.to_string()
    }
}

pub(crate) fn lvz_chat_sound_id(filename: &str) -> Option<u8> {
    let basename = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename)
        .to_ascii_lowercase();
    let stem = basename
        .strip_suffix(".wa2")
        .or_else(|| basename.strip_suffix(".wav"))?;
    let id = stem.strip_prefix("bong")?.parse::<u16>().ok()?;
    (1..=u8::MAX as u16).contains(&id).then_some(id as u8)
}

fn should_decode_lvz_chat_sounds(path: &str) -> bool {
    let Some(default_zone) = LEVEL_PATHS.first().and_then(|path| asset_directory(path)) else {
        return false;
    };

    asset_directory(path) == Some(default_zone)
}

fn asset_directory(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(directory, _)| directory)
}

#[cfg(test)]
mod tests {
    use super::{
        lvz_chat_sound_id, progress::should_present_loading_progress, should_decode_lvz_chat_sounds,
    };

    #[test]
    fn lvz_chat_sound_ids_parse_bong_filenames() {
        assert_eq!(lvz_chat_sound_id("bong1.wa2"), Some(1));
        assert_eq!(lvz_chat_sound_id("BONG150.WAV"), Some(150));
        assert_eq!(lvz_chat_sound_id("folder\\bong60.wa2"), Some(60));
        assert_eq!(lvz_chat_sound_id("bong0.wa2"), None);
        assert_eq!(lvz_chat_sound_id("bong300.wa2"), None);
        assert_eq!(lvz_chat_sound_id("explode2.wa2"), None);
    }

    #[test]
    fn loading_progress_presents_periodically_and_at_completion() {
        assert!(!should_present_loading_progress(1, 10));
        assert!(should_present_loading_progress(4, 10));
        assert!(!should_present_loading_progress(9, 10));
        assert!(should_present_loading_progress(10, 10));
    }

    #[test]
    fn lvz_chat_sound_decode_is_scoped_to_default_zone() {
        assert!(should_decode_lvz_chat_sounds(
            "assets/zones/chaos-zone/cpwavs.lvz"
        ));
        assert!(should_decode_lvz_chat_sounds(
            "assets/zones/chaos-zone/urkills.lvz"
        ));
        assert!(should_decode_lvz_chat_sounds(
            "assets/zones/chaos-zone/teamwavs.lvz"
        ));
        assert!(!should_decode_lvz_chat_sounds(
            "assets/zones/extreme/Epl.lvz"
        ));
        assert!(!should_decode_lvz_chat_sounds("standalone.lvz"));
    }
}

fn startup_zone_paths(target: Option<&str>) -> ZoneAssetPaths {
    match target {
        None => ZoneAssetPaths {
            id: "all",
            levels: LEVEL_PATHS,
            lvz: LVZ_PATHS,
            settings: ZONE_SETTINGS_PATHS,
        },
        Some(target) => {
            let target = if target == "extreme-games" {
                "extreme"
            } else {
                target
            };
            ZONE_ASSET_PATHS
                .iter()
                .find(|paths| paths.id == target)
                .copied()
                .unwrap_or(ZoneAssetPaths {
                    id: "dynamic",
                    levels: &[],
                    lvz: &[],
                    settings: &[],
                })
        }
    }
}

async fn load_zone_files(
    paths: ZoneAssetPaths,
) -> std::collections::BTreeMap<&'static str, Result<Vec<u8>, macroquad::Error>> {
    use std::future::{Future, poll_fn};
    use std::task::Poll;
    // Settings are small and needed before join; archives follow map/settings.
    let ordered: Vec<_> = paths
        .levels
        .iter()
        .chain(paths.settings)
        .chain(paths.lvz)
        .copied()
        .collect();
    let mut results = std::collections::BTreeMap::new();
    for chunk in ordered.chunks(6) {
        let mut pending: Vec<_> = chunk
            .iter()
            .map(|&path| {
                (
                    path,
                    Some(Box::pin(async move {
                        load_file(&browser_asset_path(path)).await
                    })),
                )
            })
            .collect();
        poll_fn(|cx| {
            for (path, future) in &mut pending {
                if let Some(future) = future {
                    if let Poll::Ready(result) = future.as_mut().poll(cx) {
                        results.insert(*path, result);
                    }
                }
                if results.contains_key(path) {
                    *future = None;
                }
            }
            if pending.iter().all(|(_, future)| future.is_none()) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
    results
}

#[cfg(test)]
mod startup_path_tests {
    use super::*;
    #[test]
    fn startup_is_zone_scoped_and_dynamic_zones_never_fetch_another_zone() {
        let chaos = startup_zone_paths(Some("chaos-zone"));
        assert!(!chaos.levels.is_empty());
        for path in chaos.levels.iter().chain(chaos.lvz).chain(chaos.settings) {
            assert!(path.starts_with("assets/zones/chaos-zone/"));
        }
        assert_eq!(
            startup_zone_paths(Some("extreme-games")).levels,
            startup_zone_paths(Some("extreme")).levels
        );
        let unknown = startup_zone_paths(Some("temporary"));
        assert!(unknown.levels.is_empty() && unknown.lvz.is_empty() && unknown.settings.is_empty());
        let native = startup_zone_paths(None);
        assert_eq!(native.levels, LEVEL_PATHS);
        assert_eq!(native.lvz, LVZ_PATHS);
    }
    #[test]
    fn generated_groups_cover_each_shipped_path_once() {
        for (all, grouped) in [
            (
                LEVEL_PATHS,
                ZONE_ASSET_PATHS
                    .iter()
                    .flat_map(|p| p.levels.iter().copied())
                    .collect::<Vec<_>>(),
            ),
            (
                LVZ_PATHS,
                ZONE_ASSET_PATHS
                    .iter()
                    .flat_map(|p| p.lvz.iter().copied())
                    .collect::<Vec<_>>(),
            ),
            (
                ZONE_SETTINGS_PATHS,
                ZONE_ASSET_PATHS
                    .iter()
                    .flat_map(|p| p.settings.iter().copied())
                    .collect::<Vec<_>>(),
            ),
        ] {
            let mut expected = all.to_vec();
            expected.sort();
            let mut actual = grouped;
            actual.sort();
            assert_eq!(actual, expected);
            assert!(actual.windows(2).all(|pair| pair[0] != pair[1]));
        }
    }
}
