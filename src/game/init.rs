use super::*;
use crate::authority_prizes::SECURITY_PRIZE_SEED;

impl Game {
    pub(crate) fn new(assets: Assets) -> Self {
        Self::new_with_optional_assets(Some(assets), false)
    }

    #[cfg(test)]
    pub(in crate::game) fn new_network_authority_test() -> Self {
        Self::new_with_optional_assets(None, true)
    }

    fn new_with_optional_assets(mut assets: Option<Assets>, headless_authority_test: bool) -> Self {
        let mut maps = assets
            .as_mut()
            .map(|assets| std::mem::take(&mut assets.maps))
            .unwrap_or_default();
        let mut map_tilesets = assets
            .as_mut()
            .map(|assets| std::mem::take(&mut assets.map_tilesets))
            .unwrap_or_default();
        if maps.is_empty() {
            maps.push(ArenaMap::fallback_skirmish());
            map_tilesets.push(None);
        }
        while map_tilesets.len() < maps.len() {
            map_tilesets.push(None);
        }

        let map_index = 0;
        let tuning = assets
            .as_ref()
            .and_then(|assets| local_zone_tuning(&assets.zone_tunings, &maps[map_index].name))
            .unwrap_or_else(ArenaTuning::continuum_skirmish_default);
        let spawn = maps[map_index].spawn_position();
        let powerball_pos = maps[map_index].powerball_spawn_position(spawn);
        let mut lvz_render_cache = LvzRenderCache::default();
        if let Some(assets) = assets.as_ref() {
            merge_local_zone_lvz(
                &mut lvz_render_cache,
                &assets.lvz_archives,
                &maps[map_index].name,
            );
        }
        let mut lvz_chat_sounds = BTreeMap::new();
        if let Some(assets) = assets.as_ref() {
            merge_local_zone_lvz_sounds(
                &mut lvz_chat_sounds,
                &assets.lvz_chat_sounds,
                &maps[map_index].name,
            );
        }
        #[cfg(target_arch = "wasm32")]
        let mut local_player_name = "You".to_string();
        #[cfg(not(target_arch = "wasm32"))]
        let local_player_name = "You".to_string();
        #[cfg(target_arch = "wasm32")]
        let browser_network = match browser_network_client_from_config(&maps[map_index]) {
            Ok(client) => {
                local_player_name = client.login_name.clone();
                Some(client)
            }
            Err(err) => {
                tracing::warn!(error = %err, "browser network setup failed");
                None
            }
        };
        #[cfg(target_arch = "wasm32")]
        let networked_play = browser_network.is_some();
        #[cfg(not(target_arch = "wasm32"))]
        let networked_play = false;
        let bots = if !networked_play && !headless_authority_test {
            spawn_initial_bots(&maps[map_index], spawn, tuning)
        } else {
            Vec::new()
        };

        #[cfg(target_arch = "wasm32")]
        let initial_ship = browser_network
            .as_ref()
            .map(|network| network.client.session.arena_login.ship as usize)
            .filter(|ship| *ship < 8)
            .unwrap_or(0);
        #[cfg(not(target_arch = "wasm32"))]
        let initial_ship = 0;
        let initial_ship_tuning = tuning.ship(initial_ship);
        let mut game = Self {
            #[cfg(not(test))]
            assets: assets.expect("production game requires loaded assets"),
            #[cfg(test)]
            assets: TestGameAssets(assets),
            tuning,
            chat: ChatState::default(),
            notifications: Vec::new(),
            ship: Ship {
                pos: spawn,
                vel: Vec2::ZERO,
                orientation: 0.0,
                ship_type: initial_ship,
                multifire_capable: false,
                multifire: false,
                stealth_capable: initial_ship_tuning.initial_stealth,
                stealth: false,
                cloak_capable: initial_ship_tuning.initial_cloak,
                cloak: false,
                xradar_capable: initial_ship_tuning.initial_xradar,
                xradar: false,
                antiwarp_capable: initial_ship_tuning.initial_antiwarp,
                antiwarp: false,
                flash: false,
                proximity_bombs: false,
                // BouncingBullets is a per-ship capability prize in
                // Continuum (default-off; the BouncingBullets prize
                // toggles it on). Initializing it to `true` makes the
                // client fire weapon-type=2 (bouncing bullets) instead
                // of the warbird's stock type=1 — visible to remote
                // clients via `apply_remote_weapon_visual` and to the
                // server as `weapon=0x42` damage events.
                bouncing_bullets: false,
                super_timer: 0.0,
                shield_timer: 0.0,
                engine_shutdown_timer: 0.0,
                emp_shutdown_timer: 0.0,
                repel_timer: 0.0,
                repels: initial_ship_tuning
                    .initial_repels
                    .min(initial_ship_tuning.max_repels),
                bursts: initial_ship_tuning
                    .initial_bursts
                    .min(initial_ship_tuning.max_bursts),
                bricks: initial_ship_tuning
                    .initial_bricks
                    .min(initial_ship_tuning.max_bricks),
                decoys: initial_ship_tuning
                    .initial_decoys
                    .min(initial_ship_tuning.max_decoys),
                thors: initial_ship_tuning
                    .initial_thors
                    .min(initial_ship_tuning.max_thors),
                rockets: initial_ship_tuning
                    .initial_rockets
                    .min(initial_ship_tuning.max_rockets),
                portals: initial_ship_tuning
                    .initial_portals
                    .min(initial_ship_tuning.max_portals),
                gun_level: initial_ship_tuning
                    .initial_gun_level
                    .min(initial_ship_tuning.max_gun_level),
                bomb_level: initial_ship_tuning
                    .initial_bomb_level
                    .min(initial_ship_tuning.max_bomb_level),
                rotation_per_second: initial_ship_tuning
                    .rotation_per_second
                    .min(initial_ship_tuning.rotation_max_per_second),
                thrust_acceleration: initial_ship_tuning
                    .thrust_acceleration
                    .min(initial_ship_tuning.thrust_max_acceleration),
                top_speed: initial_ship_tuning
                    .top_speed
                    .min(initial_ship_tuning.top_speed_max),
                recharge_per_second: initial_ship_tuning
                    .energy_recharge
                    .min(initial_ship_tuning.energy_recharge_max),
                shrapnel_upgrades: 0,
                bullet_cooldown: 0.0,
                burst_cooldown: 0.0,
                bomb_cooldown: 0.0,
                repel_cooldown: 0.0,
                wormhole_cooldown: 0.0,
                fake_antiwarp_timer: 0.0,
                rocket_timer: 0.0,
                rocket_cooldown: 0.0,
                max_energy: initial_ship_tuning
                    .energy_initial
                    .min(initial_ship_tuning.energy_max),
                energy: initial_ship_tuning
                    .energy_initial
                    .min(initial_ship_tuning.energy_max),
            },
            last_network_damage_player_id: None,
            last_network_damage_attacker_id: None,
            last_network_damage_energy: None,
            last_network_damage_energy_before: None,
            last_network_damage_amount: None,
            last_network_damage_weapon: None,
            last_network_damage_server_tick: None,
            last_network_damage_server_seq: None,
            last_network_damage_victim_life_seq: None,
            last_network_damage_shooter_life_seq: None,
            last_network_damage_for_self: false,
            network_damage_event_seq: 0,
            last_self_damage_attacker_id: None,
            last_self_damage_energy: None,
            last_self_damage_energy_before: None,
            last_self_damage_amount: None,
            last_self_damage_weapon: None,
            last_self_damage_server_tick: None,
            last_self_damage_server_seq: None,
            last_self_damage_victim_life_seq: None,
            last_self_damage_shooter_life_seq: None,
            self_damage_event_seq: 0,
            last_self_server_energy: None,
            last_self_server_rocket_life_seq: None,
            require_forward_self_server_rocket_life: false,
            last_self_server_rocket_activation_sequence: None,
            last_self_server_rocket_until_tick: None,
            pending_rocket_activation_sequence: None,
            last_self_server_prize_life_seq: None,
            last_self_server_prize_revision: None,
            server_maxed_spawn_loadout: None,
            last_self_server_portals: None,
            network_prize_baseline_pending: networked_play,
            last_self_server_engine_shutdown_until_tick: None,
            last_self_server_emp_until_tick: None,
            last_self_server_engine_shutdown_application_pending: false,
            last_self_server_damage_prize_loss_seq: None,
            last_self_server_damage_prize_loss_id: None,
            last_self_server_damage_prize_loss_life_seq: None,
            last_self_server_damage_prize_loss_revision: None,
            damage_prize_notification_count: 0,
            last_damage_prize_notification: None,
            prize_pickup_notification_count: 0,
            last_prize_pickup_notification: None,
            prize_pickup_sound_play_count: 0,
            combat_kill_notification_count: 0,
            last_combat_kill_notification: None,
            remote_ship_explosion_count: 0,
            last_remote_ship_explosion_position: None,
            remote_ship_explosion_witnesses: VecDeque::new(),
            bullets: Vec::new(),
            bullet_trails: Vec::new(),
            bullet_impacts: Vec::new(),
            bombs: Vec::new(),
            bomb_flashes: Vec::new(),
            bomb_trails: Vec::new(),
            mines: Vec::new(),
            shrapnel: Vec::new(),
            thors: Vec::new(),
            remote_projectiles: Vec::new(),
            owner_bomb_draw_bridges: VecDeque::new(),
            remote_weapon_view_history: crate::remote_weapon::RemoteWeaponViewHistory::default(),
            prepared_remote_projectiles: Vec::new(),
            prepared_remote_weapon_groups: Vec::new(),
            pending_remote_birth_effects: VecDeque::new(),
            remote_weapon_trails: VecDeque::new(),
            prepared_remote_bullet_trails: Vec::new(),
            prepared_remote_bomb_trails: Vec::new(),
            remote_weapon_view_effect_drops: 0,
            remote_projectile_spawns: 0,
            remote_bomb_spawns: 0,
            remote_bullet_spawns: 0,
            remote_mine_spawns: 0,
            remote_repel_spawns: 0,
            remote_repel_presentation_history: VecDeque::new(),
            remote_repel_authoritative_history: VecDeque::new(),
            latest_remote_realtime_event_id: None,
            remote_weapon_presentations: RemoteWeaponPresentationQueue::default(),
            bricks: Vec::new(),
            decoys: Vec::new(),
            decoy_ordinary_ship_presentations: std::cell::Cell::new(0),
            decoy_name_label_presentations: std::cell::Cell::new(0),
            decoy_radar_presentations: std::cell::Cell::new(0),
            repels: Vec::new(),
            warps: Vec::new(),
            portal: None,
            portal_endpoint_authority: None,
            explosions: Vec::new(),
            ship_explosions: Vec::new(),
            recent_network_deaths: VecDeque::new(),
            remote_projectile_terminals: VecDeque::new(),
            remote_owner_weapon_cancellations: VecDeque::new(),
            recent_network_damage: VecDeque::new(),
            latest_network_damage_sequence: None,
            burst_effects: Vec::new(),
            exhausts: Vec::new(),
            rocket_particles: Vec::new(),
            prizes: Vec::new(),
            applied_collectible_acks: AppliedCollectibleAckHistory::default(),
            bots,
            bot_bullets: Vec::new(),
            powerball: Powerball {
                ball_id: 0,
                pos: powerball_pos,
                vel: Vec2::ZERO,
                frequency: INVALID_PLAYER_ID,
                carried: false,
                age: 0.0,
                trail_timer: 0.0,
                pass_cooldown: 0.0,
                carry_timer: 0.0,
                friction: POWERBALL_FRICTION,
                last_touch: PowerballTouch::Player,
            },
            powerball_trails: Vec::new(),
            carrying_flag: false,
            dropped_flag: None,
            flag_score: 0,
            bot_flag_score: 0,
            ball_score: 0,
            bot_ball_score: 0,
            bot_score: 0,
            player_deaths: 0,
            player_bounty: 0,
            local_player_name,
            pending_weapon: 0,
            pending_weapon_prediction: None,
            next_weapon_prediction_id: 0,
            owned_projectile_reconciliations: 0,
            predicted_explosion_receipts: VecDeque::new(),
            owned_projectile_life_floor: None,
            pending_warp_relocation_collectible_id: None,
            pending_repel_application: None,
            pending_repel_application_expiry_tick: None,
            expired_repel_applications: std::collections::VecDeque::new(),
            consumed_repel_delivery_generations: std::collections::VecDeque::new(),
            repel_delivery_frames_seen: 0,
            repel_delivery_frames_applied: 0,
            repel_application_positions_sent: 0,
            repel_delivery_life_rejects: 0,
            repel_delivery_timing_rejects: 0,
            repel_delivery_application_rejects: 0,
            repel_delivery_expired_application_cancellations: 0,
            repel_delivery_expired_receipts_recovered: 0,
            repel_delivery_pending_retries: 0,
            repel_delivery_consumed_retries: 0,
            repel_delivery_last_snapshot_tick: None,
            repel_delivery_last_expiry_tick: None,
            repel_delivery_last_observed_tick: None,
            repel_delivery_last_offered_steps: None,
            repel_delivery_last_accepted_end_sequence: None,
            repel_delivery_last_accepted_steps: None,
            repel_movement_sequence_window: None,
            pending_warp_relocation_life_seq: None,
            pending_warp_relocation_sequences: std::collections::VecDeque::new(),
            pending_warp_relocation_staged: false,
            last_input: PlayerInput::default(),
            next_input_seq: 0,
            suppress_lifecycle_tail_position: false,
            outbound_input_ring: ClientInputRing::default(),
            server_input_applied_through: None,
            pending_portal_placements: VecDeque::new(),
            #[cfg(target_arch = "wasm32")]
            next_portal_placement_action_id: 1,
            #[cfg(target_arch = "wasm32")]
            next_objective_action_id: 1,
            #[cfg(any(test, target_arch = "wasm32"))]
            objective_action_attempts: BTreeMap::new(),
            accepted_prize_engine_shutdown: None,
            pending_same_life_movement_boundary_position: false,
            suppress_gameplay_step_after_network_rebase: false,
            self_correction_count: 0,
            last_self_correction_reason: 0,
            respawn_input_latch: RespawnInputLatch::default(),
            semantic_input_counts: [0; 12],
            spectator_mode: false,
            spectating_player_id: None,
            spectator_view_sequence: 0,
            arena_enter_time: if headless_authority_test {
                0.0
            } else {
                get_time() as f32
            },
            network_world: NetworkWorld::default(),
            poll_peak_events: 0,
            poll_peak_deferred_events: 0,
            lvz_merge_count: 0,
            lvz_merge_total_ms: 0.0,
            lvz_merge_max_ms: 0.0,
            flag_claim_eligible_frames: 0,
            flag_claim_attempts: 0,
            flag_claim_send_gated: 0,
            networked_play,
            network_settings_received: false,
            network_zone_files_ready: !networked_play,
            zone_files_batch_open: false,
            zone_files_resolve_pending: false,
            network_zone_map_ready: !networked_play,
            network_security_challenge: None,
            network_security_challenge_generation: None,
            network_security_generation: None,
            client_authority: ClientAuthoritySequencer::default(),
            observer_combat_timeline: crate::combat_timeline::ClientObserverCombatTimeline::default(
            ),
            prepared_combat_view_receipt: None,
            combat_view_ship_positions: Default::default(),
            last_observed_browser_server_tick: None,
            projectile_refresh_needed: false,
            weapon_recovery_steps_remaining: 4096,
            world_effects_advanced_before_presentation: false,
            last_projectile_refresh_request_tick: None,
            network_notice: None,
            #[cfg(any(test, target_arch = "wasm32"))]
            quality_return_recovery: None,
            menu_visible: false,
            frame_rate: Default::default(),
            main_menu_requested: false,
            scoreboard_visible: false,
            scoreboard_scroll: Default::default(),
            arena_directory_visible: false,
            developer_overlay: false,
            super_animation_age: 0.0,
            shield_animation_age: 0.0,
            super_visual_deadline: None,
            shield_visual_deadline: None,
            lvz_archives: BTreeMap::new(),
            lvz_archive_order: Vec::new(),
            lvz_render_cache,
            lvz_chat_sounds,
            dynamic_lvz_chat_sound_sources: BTreeMap::new(),
            dynamic_lvz_chat_sound_loaded_ids: BTreeSet::new(),
            dynamic_lvz_chat_sound_generation: 0,
            #[cfg(target_arch = "wasm32")]
            dynamic_lvz_sound_loads: Vec::new(),
            prize_rng_seed: SECURITY_PRIZE_SEED,
            #[cfg(target_arch = "wasm32")]
            browser_network,
            #[cfg(target_arch = "wasm32")]
            #[cfg(target_arch = "wasm32")]
            voice_loads: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            active_voice_sounds: Vec::new(),
            player_respawn_timer: 0.0,
            player_respawn_seq: 0,
            flag_respawn_timer: 0.0,
            flag_carry_timer: 0.0,
            audio_volumes: AudioVolumes::default(),
            rocket_audio: audio::RocketAudioLoop::default(),
            camera: Camera2D::default(),
            maps,
            map_tilesets,
            map_index,
            thrust_sound_started: false,
            thrust_sound_reversing: false,
            thrust_sound_afterburning: false,
            afterburner_sound_play_count: 0,
            last_afterburning: false,
            last_movement_thrust_acceleration: initial_ship_tuning.thrust_acceleration,
            last_movement_top_speed: initial_ship_tuning.top_speed,
            energy_alarm_timer: 0.0,
            victory_music_looping: false,
            exhaust_timer: 0.0,
            prize_spawn_timer: 0.0,
            door_timer: 0.0,
            network_security_synchronized: false,
            door_clock_server_tick: None,
            door_clock_observed_server_tick: None,
            door_clock_anchor_seconds: 0.0,
            door_clock_last_observed_seconds: 0.0,
            door_clock_fresh: false,
        };
        if !headless_authority_test {
            game.apply_initial_bounty_prizes();
        }
        game
    }
}

/// Build a fresh browser network client from the page configuration and the map the client is
/// currently on. Shared by initial startup and the bounded transport rejoin, so both paths install
/// the same session shape (same login identity, same map-security shared tiles) and a recovery can
/// never drift from a first join.
#[cfg(target_arch = "wasm32")]
pub(in crate::game) fn browser_network_client_from_config(
    map: &ArenaMap,
) -> Result<BrowserNetworkClient, String> {
    let config = configured_browser_network_config()?;
    let Some(url) = config.rtc_url.clone() else {
        return Err("browser RTC transport requires ?rtc=http(s)://.../v1/rtc/offer".to_string());
    };
    let transport = BrowserRtcTransport::connect(&url)?;
    let mut session = ClientProtocolSession::new(
        true,
        ArenaLoginRequest::new(
            config.initial_ship,
            0,
            1920,
            1080,
            0xffff,
            config.arena_name,
            true,
        ),
        0,
        0,
    );
    session.set_map_security_shared_tiles(&map.tiles, map.width, map.height, map.continuum_crc32());
    Ok(BrowserNetworkClient {
        client: NetworkClient::new(session, transport),
        login_name: config.login_name,
        password: config.password,
        player_identity: config.player_identity,
        login_sent: false,
        identity_login_retry: IdentityLoginRetry::default(),
        registration_requested: false,
        registration_sent: false,
        disconnect_sent: false,
        position_timer: 0.0,
        time_sync_timer: NETWORK_TIME_SYNC_INTERVAL,
        server_clock: BrowserServerClock::default(),
        stats: BrowserNetworkStats::default(),
        rejoin_attempted: false,
        rejoin_count: 0,
    })
}
