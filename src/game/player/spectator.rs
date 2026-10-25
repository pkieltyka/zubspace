use super::*;

impl Game {
    pub(super) fn toggle_spectator_mode(&mut self) {
        if self.spectator_mode {
            self.leave_spectator_mode();
        } else {
            self.enter_spectator_mode();
        }
    }

    pub(super) fn enter_spectator_mode(&mut self) {
        if self.spectator_mode {
            return;
        }

        if !self.ship_change_energy_ready() {
            return;
        }
        if self.networked_play {
            // A hit can race the full-energy read. Keep the confirmed live
            // role, velocity and owned flight until the server admits it.
            self.send_network_ship_change(SPECTATOR_SHIP);
            return;
        }
        self.apply_admitted_spectator_mode();
    }

    pub(in crate::game) fn apply_admitted_spectator_mode(&mut self) {
        self.spectator_mode = true;
        self.spectating_player_id = None;
        self.ship.vel = Vec2::ZERO;
        self.last_input = PlayerInput::default();
        if self.tuning.no_xradar {
            self.ship.xradar = false;
        }
        self.pending_weapon = 0;
        self.cancel_pending_weapon_prediction();
        self.clear_local_weapons();
        self.push_notification(
            "Spectator: arrows pan; Ctrl next; Tab previous; Esc + 1-8 play.",
            Color::from_rgba(120, 220, 255, 255),
        );
    }

    pub(super) fn leave_spectator_mode(&mut self) {
        if !self.spectator_mode {
            return;
        }
        if self.networked_play {
            // Preserve the observer/follow camera through admission or a
            // connection-quality refusal. The accepted ship ack detaches it.
            self.send_network_ship_change(self.ship.ship_type as u8);
            return;
        }

        if self.spectating_player_id.take().is_some() {
            self.send_network_spectate_request(INVALID_PLAYER_ID);
        }
        self.spectator_mode = false;
        self.ship.vel = Vec2::ZERO;
        self.send_network_ship_change(self.ship.ship_type as u8);
        if !self.networked_play {
            self.push_notification("Playing.", Color::from_rgba(120, 220, 150, 255));
        }
    }

    pub(super) fn update_spectator(
        &mut self,
        dt: f32,
        render_input: &RenderInputContext,
        actions: &[SemanticInputAction],
    ) {
        let shift_down =
            render_input.is_down(KeyCode::LeftShift) || render_input.is_down(KeyCode::RightShift);
        let mut movement = Vec2::ZERO;
        if render_input.is_down(KeyCode::Left) || render_input.is_down(KeyCode::A) {
            movement.x -= 1.0;
        }
        if render_input.is_down(KeyCode::Right) || render_input.is_down(KeyCode::D) {
            movement.x += 1.0;
        }
        if render_input.is_down(KeyCode::Up) || render_input.is_down(KeyCode::W) {
            movement.y -= 1.0;
        }
        if render_input.is_down(KeyCode::Down) || render_input.is_down(KeyCode::S) {
            movement.y += 1.0;
        }

        if movement.length_squared() > 0.0 {
            if self.spectating_player_id.take().is_some() {
                self.send_network_spectate_request(INVALID_PLAYER_ID);
            }
            let speed = if shift_down { 900.0 } else { 360.0 };
            self.ship.pos += movement.normalize() * speed * dt;
            let bounds = vec2(self.map().width as f32, self.map().height as f32) * TILE_SIZE
                - Vec2::splat(0.1);
            self.ship.pos = self.ship.pos.clamp(Vec2::ZERO, bounds);
            self.ship.vel = Vec2::ZERO;
            return;
        }

        for action in actions {
            match action.kind {
                SemanticInputActionKind::PrimaryFireOrSpectate => self.spectate_next_player(false),
                SemanticInputActionKind::BombOrSpectate
                | SemanticInputActionKind::ChatAutocompleteOrBomb => {
                    self.spectate_next_player(true)
                }
                SemanticInputActionKind::ToggleXradar => {
                    if let Some(next_xradar) =
                        spectator_xradar_toggle_after(self.ship.xradar, self.tuning.no_xradar)
                    {
                        self.ship.xradar = next_xradar;
                        let sound = if self.ship.xradar {
                            self.assets.xradar_sound.as_ref()
                        } else {
                            self.assets.toggle_off_sound.as_ref()
                        };
                        if let Some(sound) = sound {
                            play_sound(
                                sound,
                                PlaySoundParams {
                                    looped: false,
                                    volume: self.effect_volume(0.32),
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        self.refresh_spectator_follow_position();
    }

    /// Follow the exact position used by this frame's ship and label drawing.
    /// Called again after remote interpolation advances, including zero-step
    /// and authority-unready observer frames. This is observer camera state;
    /// it never advances player physics, input, energy or network authority.
    pub(in crate::game) fn refresh_spectator_follow_position(&mut self) {
        if !self.spectator_mode {
            return;
        }
        if let Some(player_id) = self.spectating_player_id {
            match spectate_view_for_target(self.network_world.players.get(&player_id)) {
                SpectateView::Follow(pos, velocity) => {
                    self.ship.pos = pos;
                    self.ship.vel = velocity;
                }
                SpectateView::Hold => {
                    // [REVIEW M11] Transient AOI gap: the target is still in
                    // the arena but their render position was swept (e.g. a
                    // dropped snapshot run). Hold the latch and coast in
                    // place instead of silently dropping the spectate.
                    self.ship.vel = Vec2::ZERO;
                }
                SpectateView::Drop => {
                    self.spectating_player_id = None;
                    self.ship.vel = Vec2::ZERO;
                }
            }
        }
    }

    /// Observer rendering advances without running ship physics or contact/damage.
    pub(super) fn update_observer_presentation(&mut self, dt: f32) {
        let mut projectiles = std::mem::take(&mut self.remote_projectiles);
        for projectile in &mut projectiles {
            if projectile.presentation_clock.is_none() {
                self.catch_up_remote_projectile_presentation(projectile, dt);
            }
        }
        projectiles.retain(|projectile| projectile.ttl > 0.0);
        self.remote_projectiles = projectiles;
        self.update_bricks(dt);
        self.update_decoys(dt);
        self.update_repels(dt);
        self.update_warps(dt);
        self.update_explosions(dt);
        self.update_burst_effects(dt);
        self.update_exhausts(dt);
        self.update_rocket_particles(dt);
        self.update_bullet_trails(dt);
        self.update_bullet_impacts(dt);
        self.update_bomb_trails(dt);
        self.update_bomb_flashes(dt);
        self.network_world.lvz.update(dt, &self.lvz_render_cache);
        self.world_effects_advanced_before_presentation = true;
    }

    pub(super) fn spectate_next_player(&mut self, backwards: bool) {
        let Some(player_id) = crate::spectate::next_spectate_target(
            &self.network_world,
            self.spectating_player_id,
            self.ship.pos,
            backwards,
            self.viewer_frequency(),
            self.ship.xradar,
        ) else {
            self.push_notification("No spectate target.", Color::from_rgba(255, 220, 95, 255));
            return;
        };

        if self.spectating_player_id == Some(player_id) {
            return;
        }

        self.spectating_player_id = Some(player_id);
        self.send_network_spectate_request(player_id);
        let name = self
            .network_world
            .players
            .get(&player_id)
            .map(|player| player.name.as_str())
            .unwrap_or("player");
        self.push_notification(
            &format!("Spectating {name}."),
            Color::from_rgba(120, 220, 255, 255),
        );
    }
}

/// What the spectator camera should do for the current latch target this
/// frame. Pure decision so the latch policy is unit-testable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SpectateView {
    /// Target is flyable and positioned — follow them.
    Follow(Vec2, Vec2),
    /// Target is still in the arena but has no render position right now
    /// (transient AOI gap) — keep the latch, coast in place.
    Hold,
    /// Target left the arena or became a spectator — drop the latch.
    Drop,
}

pub(crate) fn spectate_view_for_target(
    target: Option<&crate::network::NetworkPlayer>,
) -> SpectateView {
    match target {
        None => SpectateView::Drop,
        Some(player) if player.ship >= SPECTATOR_SHIP => SpectateView::Drop,
        Some(player) => match player.display_position() {
            Some(pos) => SpectateView::Follow(pos, player.velocity),
            None => SpectateView::Hold,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PlayerRosterEntry;

    fn pending_spectator_test_game() -> (Game, crate::protocol::AuthorityScopeIdentity) {
        use crate::game::lifecycle_sequencer::ClientAuthorityReadiness;
        let mut game = Game::new_network_authority_test();
        let scope = crate::protocol::AuthorityScopeIdentity {
            server_session_id: Some(71),
            arena_stream_id: Some(81),
            player_id: Some(23),
            life_seq: Some(41),
            alive: Some(true),
        };
        game.client_authority.install_test_scope(
            1,
            scope,
            ClientAuthorityReadiness {
                joined: true,
                arena_epoch: Some(81),
                settings: true,
                security: true,
                map: true,
                files: true,
            },
        );
        game.networked_play = true;
        game.ship.energy = game.ship.max_energy;
        game.ship.vel = vec2(120.0, -45.0);
        game.bullets.push(Bullet {
            prediction: None,
            pos: game.ship.pos,
            vel: vec2(300.0, 0.0),
            frequency: 23,
            ttl: 1.25,
            age: 0.3,
            trail_timer: 0.0,
            bounces_remaining: 0,
            bouncing: false,
            activates_on_wall: false,
            active: true,
            level: 1,
            damage: 100.0,
        });
        let tuning = game.ship_tuning();
        let foreign = RemoteWeaponSource::capture(
            RemoteWeaponSourceIdentity::authenticated(9, 7, 1).unwrap(),
            RemoteWeaponArenaScope::authenticated(71, 81),
            game.ship.pos,
            Vec2::ZERO,
            0.25,
            7,
            0,
            tuning,
            false,
        )
        .unwrap();
        game.remote_projectiles = remote_weapon_projectiles(foreign, 1, tuning, &game.tuning);
        (game, scope)
    }

    fn submit_test_spectator_request(game: &mut Game) {
        // Native transport is intentionally a no-op. Supply only the delivery
        // facts that the browser submitter owns after the actual menu action.
        assert_eq!(
            game.client_authority
                .queue_lifecycle_intent(PendingLifecycleRequest::Ship(8)),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            game.client_authority.record_ship_request_submitted(8, 79),
            ClientAuthorityDisposition::Applied
        );
    }

    #[test]
    fn spectator_request_and_energy_rejection_preserve_live_motion_and_owned_flight() {
        let (mut game, scope) = pending_spectator_test_game();
        let velocity = game.ship.vel;
        let bullet = game.bullets[0];
        game.enter_spectator_mode();
        assert!(
            !game.spectator_mode,
            "role changes only after authoritative admission"
        );
        assert_eq!(
            game.ship.vel, velocity,
            "pending admission cannot destroy coasting state"
        );
        assert_eq!(
            game.bullets,
            vec![bullet],
            "pending admission cannot cancel owned flight"
        );
        submit_test_spectator_request(&mut game);
        // A real hit can race the full-health read and cause this exact rejection.
        game.ship.energy = 800.0;
        game.apply_network_actions(&ClientProtocolActions {
            outbound_packets: vec![],
            authority_admission: vec![],
            events: vec![ClientProtocolEvent::ShipChangeRejected(
                crate::protocol::ShipChangeRejected {
                    ship: 8,
                    server_session_id: 71,
                    arena_stream_id: 81,
                    player_id: 23,
                    life_seq: 41,
                    position_sequence: 79,
                },
            )],
        });
        assert!(!game.spectator_mode);
        assert_eq!(game.client_authority.view().scope, scope);
        assert_eq!(
            game.client_authority.view().phase,
            ClientAuthorityPhase::Live
        );
        assert_eq!(game.ship.vel, velocity);
        assert_eq!(
            game.ship.energy, 800.0,
            "rejection must not refund legitimate damage"
        );
        assert_eq!(game.bullets, vec![bullet]);
        assert!(!game.remote_projectiles.is_empty());
    }

    #[test]
    fn admitted_spectator_cancels_owned_flight_once_and_preserves_foreign_effects() {
        let (mut game, _) = pending_spectator_test_game();
        let foreign_count = game.remote_projectiles.len();
        assert!(foreign_count > 0);
        game.enter_spectator_mode();
        submit_test_spectator_request(&mut game);
        let actions = ClientProtocolActions {
            outbound_packets: vec![],
            authority_admission: vec![],
            events: vec![ClientProtocolEvent::ShipFrequencyChanged(
                crate::protocol::ShipFrequencyChange {
                    player_id: 23,
                    ship: 8,
                    frequency: 23,
                    life_seq: Some(42),
                },
            )],
        };
        game.apply_network_actions(&actions);
        assert!(game.spectator_mode);
        assert_eq!(game.ship.vel, Vec2::ZERO);
        assert!(
            game.bullets.is_empty(),
            "accepted spectator transition is the flight cutoff"
        );
        assert_eq!(game.remote_projectiles.len(), foreign_count);
        game.ship.pos += vec2(30.0, 40.0);
        let camera_position = game.ship.pos;
        game.apply_network_actions(&actions);
        assert_eq!(
            game.ship.pos, camera_position,
            "duplicate admission cannot reset a panned camera"
        );
        assert_eq!(game.remote_projectiles.len(), foreign_count);
    }

    #[test]
    fn spectator_return_keeps_the_observer_until_the_selected_ship_is_admitted() {
        let (mut game, _) = pending_spectator_test_game();
        submit_test_spectator_request(&mut game);
        game.apply_network_actions(&ClientProtocolActions {
            outbound_packets: vec![],
            authority_admission: vec![],
            events: vec![ClientProtocolEvent::ShipFrequencyChanged(
                crate::protocol::ShipFrequencyChange {
                    player_id: 23,
                    ship: 8,
                    frequency: 23,
                    life_seq: Some(42),
                },
            )],
        });
        game.spectating_player_id = Some(9);
        game.leave_spectator_mode();
        assert!(
            game.spectator_mode,
            "the server still owns the observer role while return is pending"
        );
        assert_eq!(game.spectating_player_id, Some(9));
        assert_eq!(game.client_authority.view().scope.life_seq, Some(42));
        assert_eq!(
            game.client_authority
                .queue_lifecycle_intent(PendingLifecycleRequest::Ship(0)),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            game.client_authority.record_ship_request_submitted(0, 80),
            ClientAuthorityDisposition::Applied
        );
        game.apply_network_actions(&ClientProtocolActions {
            outbound_packets: vec![],
            authority_admission: vec![],
            events: vec![ClientProtocolEvent::ShipFrequencyChanged(
                crate::protocol::ShipFrequencyChange {
                    player_id: 23,
                    ship: 0,
                    frequency: 23,
                    life_seq: Some(43),
                },
            )],
        });
        assert!(!game.spectator_mode);
        assert_eq!(game.spectating_player_id, None);
        assert_eq!(game.ship.ship_type, 0);
        assert_eq!(game.client_authority.view().scope.life_seq, Some(43));
        assert!(game.client_authority.view().neutral_ship_baseline_ready());
    }

    fn flyable_player(position: Option<Vec2>) -> crate::network::NetworkPlayer {
        let mut player = crate::network::NetworkPlayer::from_roster(&PlayerRosterEntry {
            ship: 0,
            audio: 0,
            name: "target".to_string(),
            squad: String::new(),
            kill_points: 0,
            flag_points: 0,
            player_id: 9,
            frequency: 0,
            wins: 0,
            losses: 0,
            attach_parent: crate::INVALID_PLAYER_ID,
            flags: 0,
            koth: 0,
        });
        player.position = position;
        player.velocity = vec2(10.0, -5.0);
        player
    }

    #[test]
    fn observer_projectiles_cross_the_camera_without_hit_or_damage() {
        let mut game = Game::new_network_authority_test();
        game.spectator_mode = true;
        let position = game.ship.pos;
        let tuning = game.ship_tuning();
        let source = RemoteWeaponSource::capture(
            RemoteWeaponSourceIdentity::authenticated(9, 7, 1).unwrap(),
            RemoteWeaponArenaScope::authenticated(1, 1),
            position - vec2(4.0, 0.0),
            Vec2::ZERO,
            0.25,
            7,
            0,
            tuning,
            false,
        )
        .unwrap();
        game.remote_projectiles = remote_weapon_projectiles(source, 1, tuning, &game.tuning);
        let ttl = game.remote_projectiles[0].ttl;
        let energy = game.ship.energy;
        game.update_observer_presentation(0.05);
        assert_eq!(game.remote_projectiles.len(), 1);
        assert!(game.remote_projectiles[0].pos.x > position.x);
        assert!((game.remote_projectiles[0].ttl - (ttl - 0.05)).abs() < 0.001);
        assert_eq!(game.ship.energy, energy);
        assert_eq!(game.ship.pos, position);
    }

    #[test]
    fn spectate_follows_a_positioned_target() {
        let player = flyable_player(Some(vec2(100.0, 200.0)));
        assert_eq!(
            spectate_view_for_target(Some(&player)),
            SpectateView::Follow(vec2(100.0, 200.0), vec2(10.0, -5.0))
        );
    }

    #[test]
    fn spectator_camera_follows_the_same_interpolated_position_as_the_ship_sprite() {
        let mut player = flyable_player(Some(vec2(100.0, 200.0)));
        player.render_position = Some(vec2(90.0, 180.0));
        assert_eq!(
            spectate_view_for_target(Some(&player)),
            SpectateView::Follow(vec2(90.0, 180.0), player.velocity)
        );
    }

    #[test]
    fn spectator_follow_refreshes_after_remote_presentation_even_without_a_gameplay_step() {
        for hz in [60, 120, 144, 240] {
            let mut game = Game::new_network_authority_test();
            game.networked_play = true;
            game.spectator_mode = true;
            assert!(!game.gameplay_step_ready());
            game.spectating_player_id = Some(9);
            game.ship.pos = vec2(100.0, 200.0);
            // A normal nearby interpolation segment, not a teleport that
            // intentionally activates the renderer's snap/settle policy.
            let mut player = flyable_player(Some(vec2(103.0, 201.5)));
            player.render_position = Some(game.ship.pos);
            player.interpolation_start_position = Some(game.ship.pos);
            player.interpolation_target_position = player.position;
            player.interpolation_duration = 1.0;
            game.network_world.players.insert(9, player);
            let input = RenderInputContext::default();
            let before = (game.ship.energy, game.next_input_seq, game.ship.orientation);
            for _ in 0..hz {
                let previous = game.ship.pos;
                // This is the production post-update ordering. In spectator
                // mode the normal gameplay authority gate may remain closed.
                game.update_remote_presentation(1.0 / hz as f32);
                let displayed = game.network_world.players[&9].display_position().unwrap();
                assert_eq!(game.ship.pos, displayed, "camera lag at {hz}Hz");
                assert_eq!(
                    game.local_render_prediction(0.0, &input).position,
                    displayed
                );
                assert_eq!(
                    game.local_render_prediction(0.004, &input).position,
                    displayed
                );
                assert!(game.ship.pos.distance(previous) > 0.0);
            }
            assert_eq!(
                before,
                (game.ship.energy, game.next_input_seq, game.ship.orientation)
            );
        }
    }

    #[test]
    fn spectator_follow_refresh_preserves_pan_hold_drop_and_target_change() {
        let mut game = Game::new_network_authority_test();
        game.spectator_mode = true;
        game.spectating_player_id = Some(9);
        game.network_world
            .players
            .insert(9, flyable_player(Some(vec2(100.0, 200.0))));
        game.update_remote_presentation(0.0);
        assert_eq!(game.ship.pos, vec2(100.0, 200.0));
        game.network_world
            .players
            .get_mut(&9)
            .unwrap()
            .clear_position();
        game.update_remote_presentation(0.016);
        assert_eq!(game.ship.pos, vec2(100.0, 200.0));
        assert_eq!(game.spectating_player_id, Some(9));
        let mut next = flyable_player(Some(vec2(800.0, 900.0)));
        next.player_id = 10;
        game.network_world.players.insert(10, next);
        game.spectating_player_id = Some(10);
        game.update_remote_presentation(0.0);
        assert_eq!(game.ship.pos, vec2(800.0, 900.0));
        game.network_world.players.get_mut(&10).unwrap().ship = SPECTATOR_SHIP;
        game.update_remote_presentation(0.0);
        assert_eq!(game.spectating_player_id, None);
        game.ship.pos = vec2(500.0, 600.0); // Free-pan pose.
        game.update_remote_presentation(0.016);
        assert_eq!(game.ship.pos, vec2(500.0, 600.0));
        game.spectator_mode = false;
        game.spectating_player_id = Some(9); // A stale latch cannot move a player.
        game.network_world.players.get_mut(&9).unwrap().position = Some(vec2(2.0, 3.0));
        game.update_remote_presentation(0.016);
        assert_eq!(game.ship.pos, vec2(500.0, 600.0));
    }

    #[test]
    fn spectate_holds_latch_through_transient_aoi_gap() {
        // [REVIEW M11] A target whose position was swept by the AOI
        // proxy-lifetime (dropped snapshots) is still in the arena; the
        // latch must hold instead of silently dropping the spectate.
        let player = flyable_player(None);
        assert_eq!(spectate_view_for_target(Some(&player)), SpectateView::Hold);
    }

    #[test]
    fn spectate_drops_when_target_leaves_or_goes_spectator() {
        assert_eq!(spectate_view_for_target(None), SpectateView::Drop);
        let mut player = flyable_player(Some(vec2(1.0, 1.0)));
        player.ship = SPECTATOR_SHIP;
        assert_eq!(spectate_view_for_target(Some(&player)), SpectateView::Drop);
    }
}
