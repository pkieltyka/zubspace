use super::{
    local_zone_tuning, wrapped_map_index, zone_directory, zone_label, zone_settings_path_for_map,
    zone_start_index_for_names,
};
use crate::ArenaTuning;

#[test]
fn map_index_wraps_for_local_level_cycling() {
    assert_eq!(wrapped_map_index(0, 3, 1), 1);
    assert_eq!(wrapped_map_index(2, 3, 1), 0);
    assert_eq!(wrapped_map_index(0, 3, -1), 2);
    assert_eq!(wrapped_map_index(1, 3, -2), 2);
    assert_eq!(wrapped_map_index(0, 0, 1), 0);
}

#[test]
fn zone_directory_groups_maps_and_lvz_by_folder() {
    assert_eq!(
        zone_directory("assets/zones/chaos-zone/bdhadar.lvl"),
        Some("assets/zones/chaos-zone")
    );
    assert_eq!(
        zone_directory("assets/zones/chaos-zone/aiq.lvz"),
        Some("assets/zones/chaos-zone")
    );
    assert_eq!(zone_directory("arena.lvl"), None);
}

#[test]
fn zone_selection_jumps_between_first_map_in_each_zone() {
    let names = [
        "assets/zones/chaos-zone/bdhadar.lvl",
        "assets/zones/extreme/doomsy7.lvl",
        "assets/zones/extreme/doomsy8.lvl",
    ];

    assert_eq!(zone_start_index_for_names(&names, 0, 1), 1);
    assert_eq!(zone_start_index_for_names(&names, 1, 1), 0);
    assert_eq!(zone_start_index_for_names(&names, 2, 1), 0);
    assert_eq!(zone_start_index_for_names(&names, 0, -1), 1);
    assert_eq!(zone_start_index_for_names(&[], 0, 1), 0);
}

#[test]
fn zone_labels_use_folder_name_for_zone_assets() {
    assert_eq!(
        zone_label("assets/zones/chaos-zone/bdhadar.lvl"),
        "chaos-zone"
    );
    assert_eq!(zone_label("arena.lvl"), "arena.lvl");
}

#[test]
fn zone_settings_path_matches_current_map_zone() {
    let settings = [
        "assets/zones/extreme/arena.cfg",
        "assets/zones/chaos-zone/arena.conf",
    ];

    assert_eq!(
        zone_settings_path_for_map("assets/zones/chaos-zone/bdhadar.lvl", &settings),
        Some("assets/zones/chaos-zone/arena.conf")
    );
    assert_eq!(
        zone_settings_path_for_map("standalone.lvl", &settings),
        None
    );
}

#[test]
fn local_zone_tuning_matches_current_map_zone() {
    let mut chaos = ArenaTuning::continuum_skirmish_default();
    chaos.radar_mode = 7;
    let mut extreme = ArenaTuning::continuum_skirmish_default();
    extreme.radar_mode = 3;
    let tunings = vec![
        ("assets/zones/extreme/arena.conf".to_string(), extreme),
        ("assets/zones/chaos-zone/settings.cfg".to_string(), chaos),
    ];

    assert_eq!(
        local_zone_tuning(&tunings, "assets/zones/chaos-zone/bdhadar.lvl")
            .unwrap()
            .radar_mode,
        7
    );
    assert_eq!(
        local_zone_tuning(&tunings, "assets/zones/extreme/doomsy8.lvl")
            .unwrap()
            .radar_mode,
        3
    );
    assert!(local_zone_tuning(&tunings, "standalone.lvl").is_none());
    // A map under a zone directory nobody supplied tuning for must miss too. Both surviving shipped
    // zones carry tuning now, so this branch needs a directory name of its own rather than a zone.
    assert!(local_zone_tuning(&tunings, "assets/zones/punct-zone/bdhadar.lvl").is_none());
}
