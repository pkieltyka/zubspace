#![allow(clippy::large_enum_variant, clippy::too_many_arguments)]

use macroquad::audio::{
    PlaySoundParams, Sound, load_sound_from_bytes, play_sound, set_sound_volume, stop_sound,
};
#[cfg(target_arch = "wasm32")]
use macroquad::experimental::coroutines::{Coroutine, start_coroutine};
use macroquad::prelude::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

mod attach;
use attach::{AttachAction, attach_action_for_target, nearest_attach_target};
mod assets;
pub(crate) use assets::Assets;
#[cfg(test)]
pub(crate) use assets::chat_sound_index;
mod bm2;
#[cfg(test)]
use bm2::decode_bm2;
use bm2::{Transparency, texture_from_bm2, try_texture_from_bm2};
mod bot;
use bot::{
    bot_bullet_damage, bot_bullet_level, bot_bullet_range, bot_can_fire,
    bot_fire_cooldown_after_shot, bot_flag_objective_target, bot_initial_fire_cooldown,
    bot_jammed_by_antiwarp, bot_prize_target_position, bot_rotation_per_second, bot_spawn_energy,
    bot_spawn_orientation, bot_target_position, bot_touches_map_object, bot_touching_map_object,
    bot_touching_pos,
};
#[cfg(target_arch = "wasm32")]
mod browser_logging;
mod browser_metrics;
#[cfg(target_arch = "wasm32")]
use browser_metrics::publish_qa_trace_context;
use browser_metrics::{
    local_minutes_since_midnight, publish_damage_event, publish_remote_sprite_frame,
    publish_ui_metrics, publish_weapon_admission_event, remote_sprite_trace_requested,
    ui_metrics_requested,
};
mod chat;
#[cfg(test)]
use chat::CHAT_MAX_CHARS;
#[cfg(test)]
use chat::parse_local_chat_input;
#[cfg(test)]
use chat::push_chat_char;
use chat::{
    AudioVolumeCategory, ChatState, autocomplete_private_chat_target, format_network_chat_message,
    parse_audio_volume_command, parse_frequency_change_input, parse_local_chat_input_with_resolver,
    parse_status_command, ship_status_percent, status_command_message,
};
mod combat;
mod combat_timeline;
#[cfg(test)]
use combat::explosive_contact_radius;
#[cfg(test)]
use combat::inexact_weapon_damage_from_roll;
#[cfg(test)]
use combat::mine_limit;
#[cfg(test)]
use combat::thor_damage;
use combat::{
    active_shrapnel_damage, ball_carrier_should_kick, ball_carrier_warp_should_kick,
    bomb_damage_for_ship, bomb_explosion_radius, bomb_flash_position, bomb_recoil_velocity,
    bot_bullet_hits_ship, bullet_contact_overlaps_ship, bullet_or_burst_damage_after_exactness,
    damage_after_timed_protection, decoy_mirrored_orientation,
    early_shrapnel_wall_bounce_times_out, emp_bomb_shutdown_time, explosion_bot_damage_active,
    explosive_cleanup_creates_explosion, explosive_contact_overlaps_ship, explosive_shrapnel_level,
    fast_weapon_available, inactive_shrapnel_damage, leveled_gun_energy_cost,
    leveled_weapon_damage, leveled_weapon_energy_cost, local_bomb_self_damage_energy_after,
    local_visual_damage_applies, mine_drop_allowed, mine_positions_overlap, player_damage_is_fatal,
    projectile_cleanup_creates_impact, proximity_bomb_safety_blocks_ship,
    proximity_bomb_trigger_radius, proximity_sensor_overlaps_ship, radial_damage,
    radial_damage_with_shooter_proximity_reduction, repel_can_affect_frequency,
    repel_cooldown_after_weapon, repel_velocity, rocket_available, safe_zone_weapon_hit_drops_flag,
    shrapnel_cleanup_creates_impact, shrapnel_count, shrapnel_damage, shrapnel_direction,
    shrapnel_projectile_position, shrapnel_rng_seed, thor_damage_for_ship,
    thor_proximity_trigger_radius,
};
mod continuum;
#[cfg(test)]
use continuum::continuum_bounce_factor;
#[cfg(test)]
use continuum::continuum_map_crc32;
#[cfg(test)]
use continuum::decompress_zlib_payload;
use continuum::{
    continuum_file_crc32, continuum_settings_checksum, sign_extend_i16, weapon_checksum,
};
mod authority;
mod authority_identity;
mod authority_inputs;
mod authority_prizes;
mod authority_projectiles;
mod clock_bootstrap;
pub(crate) use clock_bootstrap::ServerClockBootstrap;
mod constants;
mod continuum_position;
mod continuum_weapon;
pub(crate) use constants::*;
use continuum_weapon::continuum_proximity_bomb_firing_safety_radius;
mod door;
#[cfg(test)]
use door::reference_game_seeded_door_table;
use door::{next_door_seed, seeded_door_tile};
mod flag;
#[cfg(test)]
use flag::carryable_flag_limit;
use flag::{
    can_pickup_carryable_flag, flag_source_hidden, flagger_bomb_cooldown, flagger_damage_received,
};
mod fixed_step;
use fixed_step::{FixedGameplayClock, GAMEPLAY_STEP_SECONDS, bounded_presentation_step_seconds};
mod font;
use font::{BitmapFont, TextAlign, TextColor};
mod game;
pub(crate) use game::Game;
mod gameplay;
#[path = "gameplay/shared_rules.rs"]
mod gameplay_rules;
pub(crate) use gameplay::*;
mod input;
#[cfg(test)]
use input::frequency_step_key;
#[cfg(test)]
use input::ship_number_key;
use input::{
    FrameInputEvents, RenderInputContext, SemanticInputAction, SemanticInputActionKind,
    semantic_action_requires_weapon_submission, semantic_input_metric_index,
};
mod lvz;
#[cfg(test)]
use lvz::{
    LVZ_DISPLAY_DEATH, LVZ_DISPLAY_KILL, LVZ_DISPLAY_SHOW_ALWAYS, LVZ_SCREEN_REFERENCE_POINT_COUNT,
    LvzObjectDefinition, LvzSectionKind, decode_lvz_raster_image, lvz_asset_stem_matches,
    lvz_display_time_seconds, lvz_graphic_override_stem, lvz_graphic_override_stem_for_file,
    lvz_layer, lvz_screen_reference_point_for_size, lvz_texture_name_for_image_filename,
    normalize_asset_name,
};
use lvz::{
    LVZ_LAYER_AFTER_BACKGROUND, LVZ_LAYER_AFTER_CHAT, LVZ_LAYER_AFTER_GAUGES,
    LVZ_LAYER_AFTER_SHIPS, LVZ_LAYER_AFTER_TILES, LVZ_LAYER_AFTER_WEAPONS, LVZ_LAYER_BELOW_ALL,
    LVZ_LAYER_TOPMOST, LvzArchive, LvzRenderCache, LvzTexture, NetworkLvzState,
    lvz_animation_frame, lvz_layer_in_range, lvz_screen_reference_point,
    lvz_single_ship_frame_size, lvz_source_for_frame, lvz_square_sheet_frame_size,
};
mod map;
use map::{
    ArenaMap, DownloadedFile, MapTiles, ValidatedDownloadedFile, lvl_embedded_bitmap,
    map_security_checksum_for_tiles, parse_downloaded_file, parse_downloaded_file_structural,
};
mod math;
use math::{
    hash_grid, heading_to_orientation, orientation_to_heading, protocol_direction,
    protocol_position_coord, protocol_velocity_component, weapon_level_index,
};
#[cfg(any(test, target_arch = "wasm32"))]
use math::{
    protocol_ball_coord, protocol_ball_velocity_component, protocol_signed_world_coord,
    protocol_world_coord,
};
mod model;
pub(crate) use model::{
    Bomb, BombFlash, BombTrail, BotBullet, BotShip, Brick, Bullet, BulletImpact, BulletTrail,
    BurstEffect, Decoy, DroppedFlag, ExhaustParticle, Explosion, ExplosionKind, ExplosiveShrapnel,
    Mine, Portal, PortalEndpointAuthority, Powerball, PowerballTouch, PowerballTrail, PrizeKind,
    PrizePickup, ProximitySensorState, RemoteRepelAuthoritativeMutation,
    RemoteRepelPresentationFact, RepelEffect, RocketParticle, Ship, ShipExplosion, Shrapnel, Thor,
    WarpEffect, WeaponLifetimeKind,
};
mod network;
pub(crate) use network::*;
mod packet_io;
mod zone_wire;
use packet_io::{
    push_fixed_string, push_u8, push_u16, push_u32, read_c_string_at, read_fixed_c_string,
    read_i16, read_lossy_c_string, read_optional_u16, read_optional_u32, read_u16, read_u32,
};
mod physics;
use physics::{perpendicular, truncate, turn_towards_orientation};
mod prize;
use prize::{
    DAMAGE_PRIZE_LOSS_REROLL_LIMIT, apply_status_prize, authoritative_multiprize_prizes,
    can_spawn_prize_at, capability_prize_notification, collected_prize_bounty_gain,
    continuum_prize_spawn_position, current_energy_after_capacity_prize, damage_can_remove_prize,
    damage_prize_loss_chance, damage_prize_loss_id_allowed, death_prize_from_player_death,
    drain_ship_status_energy, energy_capacity_after_prize, engine_shutdown_lifetime_after,
    generate_weighted_prize, initial_bounty_prizes, local_player_can_collect_prizes,
    max_hidden_prize_count, network_player_overlaps_prize, none_prize_replacement_prize,
    prize_lifetime_seconds, remove_prize_by_collectible_id, shield_prize_lifetime_after,
    ship_explosion_position_from_player_death, ship_overlaps_prize, shrapnel_prize_amount_after,
    stat_value_after_prize, status_prize_notification, super_prize_lifetime_after,
    tick_timed_status, warp_prize_energy_after, warp_prize_notification_after,
};
#[cfg(test)]
use prize::{
    drain_status_energy, initial_bounty_prize_allowed, multiprize_prize_allowed, multiprize_prizes,
    none_prize_replacement_prize_allowed, remove_prize_at_protocol_position,
    status_prize_capability_after,
};
mod radar;
#[cfg(test)]
use radar::radar_tile_color;
use radar::{
    draw_radar_view_dot, radar_bomb_visible, radar_goal_tile_color, radar_player_style,
    radar_time_sector_label, small_radar_origin, small_radar_view,
};
mod protocol;
#[path = "protocol/realtime_event.rs"]
mod realtime_event;
pub(crate) use protocol::*;
mod remote_weapon;
mod render_limits;
use remote_weapon::{
    RemoteProjectile, RemoteProjectileAuthorityBinding, RemoteProjectileKind,
    RemoteWeaponArenaScope, RemoteWeaponSoundKind, RemoteWeaponSource, RemoteWeaponSourceIdentity,
    RemoteWeaponVisual, encode_weapon_data, remote_mine_repel_bounces,
    remote_projectile_uses_solid_cleanup, remote_shrapnel_projectiles, remote_weapon_projectiles,
    remote_weapon_sound, remote_weapon_visual_position,
};
#[cfg(test)]
use remote_weapon::{RemoteWeaponData, remote_weapon_data, remote_weapon_visual};
mod soccer;
use soccer::{
    bot_powerball_kick, can_touch_powerball, local_player_can_attempt_powerball_pickup,
    manual_powerball_fire_request_position, manual_powerball_fire_velocity,
    powerball_pickup_weapon_cooldowns_after, soccer_goal_counts, soccer_team_goal,
    timed_powerball_release_velocity,
};
#[cfg(test)]
use soccer::{can_pickup_powerball, continuum_powerball_friction, solid_between_world_points};
mod scoreboard;
use scoreboard::{ScoreboardRow, scoreboard_entries, scoreboard_rows};
mod spatial_audio;
use spatial_audio::{positional_sound_radius_px, positional_sound_volume};
mod settings;
pub(crate) use settings::*;
mod spectate;
mod spectator_view;
#[cfg(test)]
use spectate::nearest_spectate_target;
mod sprite;
use sprite::SpriteSheet;
mod tile;
#[cfg(test)]
use tile::tile_center;
#[cfg(test)]
use tile::{
    DOOR_ANIMATION_DURATION, is_solid_tile, is_solid_tile_with_doors, should_draw_base_tile,
};
use tile::{brick_line_tiles, door_animation_source, tile_source, world_to_tile};
mod transport;
#[cfg(any(test, target_arch = "wasm32"))]
use transport::PacketTransport;
#[cfg(test)]
use transport::{BrowserNetworkConfig, browser_player_identity_parts};
#[cfg(target_arch = "wasm32")]
use transport::{
    BrowserRtcStatus, BrowserRtcTransport, browser_disconnect_requested,
    browser_dropped_realtime_packets, browser_gameplay_clock_stalled,
    browser_player_identity_parts, configured_browser_network_config, mark_browser_disconnect_sent,
    request_browser_join_retry, request_browser_main_menu, reset_browser_player_identity,
};
mod ui;
#[cfg(test)]
use ui::banner_palette_color;
#[cfg(test)]
use ui::koth_timer_label;
#[cfg(test)]
use ui::prize_notification;
use ui::{
    NOTIFICATION_ANCHOR_X_FRACTION, NOTIFICATION_ANCHOR_Y_FRACTION, NOTIFICATION_LIFETIME,
    NOTIFICATION_LIMIT, NOTIFICATION_TEXT_SCALE, Notification, arena_directory_notification,
    arena_directory_panel_position, arena_directory_rows, bot_destroyed_notification,
    client_file_request_notification, draw_player_banner, energy_alarm_active, energy_label,
    flag_reward_notification, flag_victory_notification, frequency_change_notification,
    koth_reset_notification, koth_time_added_notification, network_objective_status,
    personal_koth_timer_notification, player_destroyed_notification, player_entered_notification,
    player_killed_by_notification, player_left_notification, player_voice_notification,
    prize_notification_text, push_notification_entry, server_file_transfer_notification,
    server_version_notification, ship_frequency_change_notification, ship_label,
    soccer_goal_notification, speed_game_over_notification, ufo_toggle_notification,
    update_notification_entries, watched_damage_notification,
};
mod visibility;
use visibility::{
    flags_visible_to_viewer, network_child_reveals_parent_on_radar, network_player_radar_visible,
    network_player_world_visible, spectator_xradar_toggle_after, viewer_frequency_after_spectate,
    visibility_ship_type_after_spectate,
};
mod vie;
#[cfg(test)]
use vie::vie_rng_next;
mod respawn;
use respawn::respawn_delay_seconds;
mod warp;
#[cfg(test)]
use warp::continuum_default_spawn_radius;
use warp::{
    antiwarp_settle_active, apply_set_coordinates_to_ship, is_antiwarped_by_enemy,
    manual_warp_bomb_cooldown_after, portal_place_available, warp_available, warp_energy_after,
    warp_prize_destination, warp_prize_velocity_after, warp_velocity_after,
};

fn window_conf() -> Conf {
    Conf {
        window_title: "zubspace".to_string(),
        window_width: 1280,
        window_height: 720,
        high_dpi: true,
        // The fork honors this on WebGL; upstream always requested AA.
        // Preserve the existing browser appearance during the migration.
        sample_count: 4,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    init_client_logging();
    let assets = Assets::load().await;
    let mut game = Game::new(assets);
    let mut loading_overlay_visible = true;
    let mut gameplay_clock = FixedGameplayClock::default();
    let mut render_input = RenderInputContext::default();
    let mut input_events = FrameInputEvents::new();

    loop {
        let render_dt = get_frame_time();
        game.reset_weapon_recovery_budget();
        // Browser key events arrive between animation callbacks, against the
        // frame that was actually presented by the preceding iteration.
        // Capture that physical edge and freeze its combat-view receipt before
        // inbound polling can advance the server clock, applied watermark, or
        // next presentation cursor. Authority is still applied before the edge
        // is consumed; a hard scope change makes this prepared receipt
        // unavailable instead of rebinding the edge to a successor life.
        render_input.capture_frame(&mut input_events);
        game.update_player_list_input(
            render_dt,
            screen_width(),
            screen_height(),
            &mut render_input,
        );
        game.prepare_pending_semantic_weapon_receipt(&render_input, 0);
        // Inbound authority is applied before this frame consumes semantic
        // gameplay edges. A correction can therefore establish its
        // application barrier without first spawning a local projectile or
        // mutating weapon energy/inventory on the superseded movement state.
        #[cfg(target_arch = "wasm32")]
        {
            game.poll_browser_network_before_gameplay();
            // Spend one bounded chunk of the postponed LVZ image resolve each frame. The ready path
            // opens gameplay only after this completes, so a fourteen-archive join never pays the
            // whole per-object resolve in one frame.
            game.advance_deferred_lvz_merge();
        }
        // Multiplayer door seeds advance from synchronized server-tick deltas,
        // independently of capped gameplay catch-up and browser render pacing.
        // Offline play retains render-time doors. The helper applies every
        // elapsed phase and relocates newly trapped hulls.
        game.update_doors(render_dt);
        // Network brick expiry is server-clock state, including while a
        // lifecycle barrier pauses gameplay. Retire collision before stepping.
        game.update_bricks_at_server_time();
        let gameplay_ready = game.gameplay_step_ready();
        let logical_steps = gameplay_clock.advance_when_ready(render_dt, gameplay_ready);
        if gameplay_ready {
            // The press-time receipt was captured before inbound/presentation
            // advancement. A catch-up batch drains edges on its final step, so
            // retarget only that receipt's structural input sequence rather
            // than resampling a newer view or consuming it on step one.
            game.prepare_pending_semantic_weapon_receipt(&render_input, logical_steps);
            for logical_step in 0..logical_steps {
                // Each update consumes exactly one logical input, advances
                // prediction exactly once, and records that sequence in the
                // rolling input bundle. Only the final catch-up step emits a
                // position so unordered RTC delivery cannot expose intermediate
                // positions before their input burst arrives.
                let emit_network_position = logical_step + 1 == logical_steps;
                game.update(
                    GAMEPLAY_STEP_SECONDS,
                    emit_network_position,
                    &mut render_input,
                );
            }
            if logical_steps > 0 {
                render_input.finish_gameplay_batch();
                game.finish_gameplay_batch();
            }
        } else {
            game.update_while_gameplay_unready(render_dt, &mut render_input);
        }
        // Pre-step polling decodes snapshots, but stages their public actor
        // state for application in either branch above. Project only after
        // that Game application: advancing the clock against the predecessor
        // samples caused a held frame followed by a catch-up jump after a
        // scheduler pause. Press-time input receipts were already frozen
        // against the preceding draw and must not be resampled here.
        game.update_remote_presentation(bounded_presentation_step_seconds(render_dt));
        game.update_rocket_audio();
        game.draw(gameplay_clock.remainder_seconds() as f32, &render_input);
        if loading_overlay_visible && game.initial_presentation_ready() {
            assets::hide_loading_overlay();
            loading_overlay_visible = false;
        }
        next_frame().await;
    }
}

#[cfg(target_arch = "wasm32")]
fn init_client_logging() {
    browser_logging::init();
}

#[cfg(not(target_arch = "wasm32"))]
fn init_client_logging() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .with_ansi(false)
        .compact()
        .try_init();
}

#[cfg(test)]
mod tests;

#[cfg(test)]
use chat::{CHAT_HISTORY_LIMIT, push_chat_message};
