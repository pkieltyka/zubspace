use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RocketActionSequencer {
    next_input_sequence: u16,
}

#[inline]
fn weapon_phase_due_on_input_step(published_input_step: bool) -> bool {
    published_input_step
}

/// `Some` means an exact server-authored logical window is installed and
/// therefore overrides the presentation timer. Retire it on the first
/// exclusive-end step; otherwise the u16 sequence would reactivate it after a
/// complete epoch.
fn repel_movement_sequence_active_for_input(
    window: &mut Option<(u16, u16)>,
    current_input_sequence: u16,
) -> Option<bool> {
    let installed = (*window)?;
    if crate::gameplay_rules::repel_sequence_window_end_reached(installed, current_input_sequence) {
        *window = None;
        return Some(false);
    }
    Some(crate::gameplay_rules::repel_sequence_window_contains(
        installed,
        current_input_sequence,
    ))
}

impl RocketActionSequencer {
    fn after_movement(current_input_sequence: u16) -> Self {
        Self {
            next_input_sequence: crate::gameplay_rules::rocket_activation_sequence_after_movement(
                current_input_sequence,
            ),
        }
    }

    fn activation_for_action(
        self,
        kind: SemanticInputActionKind,
        activation_available: bool,
    ) -> Option<RocketActivation> {
        if kind != SemanticInputActionKind::Rocket {
            return None;
        }
        activation_available.then(|| RocketActivation::on_next_input(self.next_input_sequence))
    }

    fn completed_movement_sequence(self) -> u16 {
        self.next_input_sequence.wrapping_sub(1)
    }
}

impl Game {
    pub(in crate::game) fn local_spawn_pose_pending(&self) -> bool {
        if !self.networked_play || self.spectator_mode {
            return false;
        }
        let authority = self.client_authority.view();
        authority.initial_baseline_pending
            || authority.transition_kind == Some(ClientAuthorityTransitionKind::Respawn)
            || (authority.transition_kind == Some(ClientAuthorityTransitionKind::Ship)
                && authority.lifecycle_baseline_life_seq.is_some())
    }

    pub(in crate::game) fn local_ship_pose_presentable(&self) -> bool {
        !self.spectator_mode && self.player_respawn_timer <= 0.0 && !self.local_spawn_pose_pending()
    }

    pub(in crate::game) fn local_render_camera_target(&self) -> Vec2 {
        if self.local_spawn_pose_pending() {
            // The canonical neutral request is at the static map centre;
            // it is not the chosen spawn. Keep the prior drawn view until
            // the exact application receipt installs the server's position.
            self.camera.target
        } else {
            self.ship.pos
        }
    }

    pub(in crate::game) fn local_render_prediction(
        &self,
        partial_seconds: f32,
        render_input: &RenderInputContext,
    ) -> LocalRenderPrediction {
        if self.local_spawn_pose_pending() {
            return LocalRenderPrediction {
                position: self.local_render_camera_target(),
                velocity: Vec2::ZERO,
                orientation: self.ship.orientation,
                input_bits: 0,
                partial_seconds: 0.0,
            };
        }
        let canonical = LocalRenderPrediction {
            position: self.ship.pos,
            velocity: self.ship.vel,
            orientation: self.ship.orientation,
            input_bits: self.last_input.bits(),
            partial_seconds: 0.0,
        };
        if !partial_seconds.is_finite()
            || partial_seconds <= 0.0
            || !self.gameplay_step_ready()
            || self.pre_step_network_rebase_suppressed()
            || self.spectator_mode
            || self.player_respawn_timer > 0.0
        {
            return canonical;
        }
        let dt = partial_seconds.min(crate::fixed_step::GAMEPLAY_STEP_SECONDS);
        let enabled = !self.chat.active
            && !self.menu_visible
            && !self.scoreboard_visible
            && !self.main_menu_requested;
        let input = read_local_player_input(enabled, render_input);
        if self.respawn_input_latch.armed {
            // Read the same latch policy without consuming a physical edge or
            // releasing the actual lifecycle latch before a canonical update.
            let mut latch = self.respawn_input_latch;
            let mut sampled_input = render_input.clone();
            let shift = enabled
                && (render_input.is_down(KeyCode::LeftShift)
                    || render_input.is_down(KeyCode::RightShift));
            let ctrl = enabled
                && (render_input.is_down(KeyCode::LeftControl)
                    || render_input.is_down(KeyCode::RightControl));
            let fresh = sampled_input.take_gameplay_key_press();
            let has_actions = !sampled_input.drain_actions().is_empty();
            if latch.should_suppress(
                input.bits(),
                held_respawn_action_bits(shift, ctrl, render_input),
                fresh,
                has_actions,
            ) {
                return canonical;
            }
        }
        let tuning = self.ship_tuning();
        let mut position = self.ship.pos;
        let mut velocity = self.ship.vel;
        let frequency = self.player_frequency();
        let passable_bricks: Vec<_> = self
            .bricks
            .iter()
            .filter(|brick| brick.team == frequency)
            .map(|brick| brick.tile)
            .collect();
        move_entity_with_collision_and_passable_tiles(
            self.map(),
            &mut position,
            &mut velocity,
            tuning.radius_pixels,
            dt,
            self.tuning.bounce_factor,
            &passable_bricks,
        );
        let (gravity_velocity, gravity_top_speed) = wormhole_gravity_velocity(
            self.map(),
            position,
            velocity,
            tuning.wormhole_gravity,
            tuning.wormhole_top_speed,
            dt,
        );
        let mut rocket_timer = self.ship.rocket_timer;
        let rocket_step = crate::gameplay_rules::begin_rocket_movement_step(
            &mut rocket_timer,
            dt,
            self.pending_rocket_activation_sequence,
            self.next_input_seq,
        );
        let rocket_active = crate::gameplay_rules::rocket_movement_is_active(
            rocket_step.active,
            self.last_self_server_rocket_activation_sequence,
            crate::continuum::seconds_to_ticks_u32(tuning.rocket_lifetime),
            self.next_input_seq,
        );
        let mut repel_window = self.repel_movement_sequence_window;
        let repel_active =
            repel_movement_sequence_active_for_input(&mut repel_window, self.next_input_seq)
                .unwrap_or(self.ship.repel_timer > 0.0);
        let movement = step_player_movement(
            PlayerMovementState {
                orientation: self.ship.orientation,
                velocity: MovementVector::new(velocity.x, velocity.y),
                energy: self.ship.energy,
                max_energy: self.ship.max_energy,
            },
            PlayerMovementInput {
                thrust: input.thrust,
                reverse: input.reverse,
                rotate_left: input.rotate_left,
                rotate_right: input.rotate_right,
                afterburner_held: input.afterburner_held,
            },
            self.local_movement_tuning(),
            PlayerMovementEnvironment {
                engine_shutdown: self.ship.engine_shutdown_timer > 0.0,
                emp_shutdown: self.ship.emp_shutdown_timer > 0.0,
                rocket_active,
                repel_active,
                flagger_active: self.flagger_active(),
                has_attached_riders: self.local_ship_has_attached_turrets(),
                wormhole_velocity_delta: MovementVector::new(
                    gravity_velocity.x - velocity.x,
                    gravity_velocity.y - velocity.y,
                ),
                gravity_top_speed,
            },
            dt,
        );
        // Deliberately discard energy, timers, effects and all semantic actions.
        // The next draw starts from canonical state again, never this result.
        LocalRenderPrediction {
            position,
            velocity: vec2(movement.state.velocity.x, movement.state.velocity.y),
            orientation: movement.state.orientation,
            input_bits: input.bits(),
            partial_seconds: dt,
        }
    }

    // Shared by canonical movement and disposable render prediction. Keep
    // authored/upgraded movement tuning in one place; a renderer must not grow
    // a second set of thrust, speed, rotation or temporary-effect constants.
    pub(in crate::game) fn local_movement_tuning(&self) -> PlayerMovementTuning {
        let tuning = self.ship_tuning();
        PlayerMovementTuning {
            rotation_per_second: self.effective_rotation_per_second(),
            shutdown_rotation_per_second: ENGINE_SHUTDOWN_ROTATION_PER_SECOND,
            base_thrust_acceleration: self.effective_thrust_acceleration(),
            afterburner_thrust_acceleration: tuning.afterburner_thrust_acceleration,
            rocket_thrust_acceleration: tuning.rocket_thrust_acceleration,
            base_top_speed: self.effective_top_speed(),
            afterburner_top_speed: tuning.afterburner_top_speed,
            rocket_top_speed: tuning.rocket_top_speed,
            afterburner_energy_per_second: tuning.afterburner_energy_per_second,
            recharge_per_second: self.effective_energy_recharge(),
            turret_thrust_penalty: tuning.turret_thrust_penalty,
            turret_speed_penalty: tuning.turret_speed_penalty,
            repel_top_speed: self.tuning.repel_speed,
            flagger_thrust_adjustment: self.tuning.flagger_thrust_adjustment,
            flagger_speed_adjustment: self.tuning.flagger_speed_adjustment,
        }
    }

    pub(super) fn update_ship(
        &mut self,
        dt: f32,
        render_input: &mut RenderInputContext,
        actions: &[SemanticInputAction],
        published_input_step: bool,
        weapon_submission_ready: bool,
    ) {
        // A menu action can submit a ship request after the frame loop
        // admitted this step. Publication then pauses at sequence N. Advancing
        // here would leave one unrecorded movement/energy step; a rejected
        // request resumes at N and would simulate it again before emission.
        // Frequency intents instead submit their embedded post-movement
        // baseline in the network tail, so their current step still belongs.
        if matches!(
            self.client_authority.view().lifecycle_intent,
            Some(PendingLifecycleRequest::Ship(_))
        ) {
            self.thrust_sound_started = false;
            self.exhaust_timer = 0.0;
            return;
        }
        let tuning = self.ship_tuning();
        self.last_afterburning = false;
        self.last_movement_thrust_acceleration = self.effective_thrust_acceleration();
        self.last_movement_top_speed = self.effective_top_speed();
        let gameplay_input_enabled = !self.chat.active
            && !self.menu_visible
            && !self.scoreboard_visible
            && !self.main_menu_requested;
        // Canonical input record. Phase 1 establishes this boundary for
        // the movement bits so the same record can later be sent to the
        // server (Phase 1b) and consumed by an authoritative physics
        // step (Phase 1c). See `docs/authoritative_server.md` §4.
        let mut input = read_local_player_input(gameplay_input_enabled, render_input);
        let shift_down = gameplay_input_enabled
            && (render_input.is_down(KeyCode::LeftShift)
                || render_input.is_down(KeyCode::RightShift));
        let ctrl_down = gameplay_input_enabled
            && (render_input.is_down(KeyCode::LeftControl)
                || render_input.is_down(KeyCode::RightControl));
        let fresh_gameplay_key_press = render_input.take_gameplay_key_press();
        if self.respawn_input_latch.armed {
            let action_bits = held_respawn_action_bits(shift_down, ctrl_down, render_input);
            if self.respawn_input_latch.should_suppress(
                input.bits(),
                action_bits,
                fresh_gameplay_key_press,
                !actions.is_empty(),
            ) {
                self.last_input = PlayerInput::default();
                self.thrust_sound_started = false;
                self.exhaust_timer = 0.0;
                return;
            }
        }
        // `shift_down` is *also* a UI modifier for non-movement actions
        // (cloak/antiwarp toggles, weapon variants). Phase 2 will add
        // explicit bits for those actions; for now the raw key state is
        // still read locally since the input record only covers
        // movement.
        debug_assert_eq!(
            shift_down, input.afterburner_held,
            "afterburner_held must mirror raw shift state"
        );
        self.ship.rocket_cooldown -= dt;
        self.ship.bomb_cooldown -= dt;
        // The reference game samples RocketTime for movement before it processes the
        // ordered weapon/action phase. Every successful F3 therefore begins
        // on the following logical input, regardless of its FIFO position.
        let current_input_sequence = self.next_input_seq;
        let rocket_actions = RocketActionSequencer::after_movement(current_input_sequence);
        let rocket_step = crate::gameplay_rules::begin_rocket_movement_step(
            &mut self.ship.rocket_timer,
            dt,
            self.pending_rocket_activation_sequence,
            current_input_sequence,
        );
        let engine_shutdown = self.ship.engine_shutdown_timer > 0.0;
        let rocket_active = crate::gameplay_rules::rocket_movement_is_active(
            rocket_step.active,
            self.last_self_server_rocket_activation_sequence,
            crate::continuum::seconds_to_ticks_u32(tuning.rocket_lifetime),
            current_input_sequence,
        );
        input.rocket_activation_pulse = rocket_step.activation_pulse;
        // Stash the exact movement record consumed below. Bit 5 is only the
        // activation pulse; `rocket_active` below remains the local predicted
        // or acknowledged logical interval and never leaks onto the wire as
        // held state.
        self.last_input = input;

        // The reference game's game loop runs PlayerManager before ShipController. The
        // current input must not affect this frame's displacement/collision;
        // thrust, rotation, and speed clamp below become next-frame velocity.
        self.move_ship_before_current_input(dt);

        let had_repel_movement_window = self.repel_movement_sequence_window.is_some();
        let exact_repel_active = repel_movement_sequence_active_for_input(
            &mut self.repel_movement_sequence_window,
            current_input_sequence,
        );
        if had_repel_movement_window && self.repel_movement_sequence_window.is_none() {
            self.ship.repel_timer = 0.0;
        }

        let (gravity_vel, gravity_top_speed) = wormhole_gravity_velocity(
            self.map(),
            self.ship.pos,
            self.ship.vel,
            tuning.wormhole_gravity,
            tuning.wormhole_top_speed,
            dt,
        );
        let attached_turrets = self.local_ship_has_attached_turrets();
        let movement = step_player_movement(
            PlayerMovementState {
                orientation: self.ship.orientation,
                velocity: MovementVector::new(self.ship.vel.x, self.ship.vel.y),
                energy: self.ship.energy,
                max_energy: self.ship.max_energy,
            },
            PlayerMovementInput {
                thrust: input.thrust,
                reverse: input.reverse,
                rotate_left: input.rotate_left,
                rotate_right: input.rotate_right,
                afterburner_held: input.afterburner_held,
            },
            self.local_movement_tuning(),
            PlayerMovementEnvironment {
                engine_shutdown,
                emp_shutdown: self.ship.emp_shutdown_timer > 0.0,
                rocket_active,
                repel_active: exact_repel_active.unwrap_or(self.ship.repel_timer > 0.0),
                flagger_active: self.flagger_active(),
                has_attached_riders: attached_turrets,
                wormhole_velocity_delta: MovementVector::new(
                    gravity_vel.x - self.ship.vel.x,
                    gravity_vel.y - self.ship.vel.y,
                ),
                gravity_top_speed,
            },
            dt,
        );
        self.ship.orientation = movement.state.orientation;
        self.ship.vel = vec2(movement.state.velocity.x, movement.state.velocity.y);
        self.ship.energy = movement.state.energy;
        let afterburning = movement.afterburning;
        self.last_afterburning = afterburning;
        self.last_movement_thrust_acceleration = movement.selected_thrust_acceleration;
        self.last_movement_top_speed = movement.top_speed;
        let display_heading = vec2(movement.display_heading.x, movement.display_heading.y);
        let (heading_x, heading_y) =
            crate::continuum_position::canonical_discrete_heading_components(
                movement.state.orientation,
            );
        let heading = vec2(heading_x, heading_y);
        match movement.applied_thrust {
            AppliedThrust::Rocket => {
                self.start_thrust_sound(false, false);
                self.spawn_rocket_exhaust(display_heading, dt);
            }
            AppliedThrust::Forward => {
                self.start_thrust_sound(false, afterburning);
                self.spawn_exhaust(display_heading, false, dt);
            }
            AppliedThrust::Reverse => {
                self.start_thrust_sound(true, afterburning);
                self.spawn_exhaust(display_heading, true, dt);
            }
            AppliedThrust::None => {
                self.thrust_sound_started = false;
                self.thrust_sound_afterburning = false;
                self.exhaust_timer = 0.0;
            }
        }
        drain_ship_status_energy(
            &mut self.ship.stealth,
            &mut self.ship.cloak,
            &mut self.ship.xradar,
            &mut self.ship.antiwarp,
            &mut self.ship.energy,
            tuning.stealth_energy_per_second,
            tuning.cloak_energy_per_second,
            tuning.xradar_energy_per_second,
            tuning.antiwarp_energy_per_second,
            dt,
        );
        self.ship.bullet_cooldown -= dt;
        self.ship.burst_cooldown -= dt;
        self.ship.repel_cooldown -= dt;
        self.ship.wormhole_cooldown -= dt;
        self.ship.fake_antiwarp_timer -= dt;

        if self.try_wormhole_warp() {
            return;
        }

        if !gameplay_input_enabled {
            return;
        }

        // A render catch-up can execute multiple authenticated 60 Hz inputs,
        // but only its tail publishes a client position.  Raw held weapon
        // keys must follow the same boundary as press-time semantic actions:
        // firing on an earlier unpublished step would apply recoil at N while
        // the retained weapon field and exact canonical tail are stamped N+1.
        // Deferring the complete weapon/action phase keeps the action,
        // server preview, recoil, and mandatory f32 provenance atomic.
        if !weapon_phase_due_on_input_step(published_input_step) {
            return;
        }

        let in_safe = self.in_safe_zone();
        let can_fastshoot = fast_weapon_available(afterburning, tuning.disable_fast_shooting);
        self.handle_semantic_ship_actions(
            actions,
            in_safe,
            can_fastshoot,
            heading,
            tuning,
            rocket_actions,
            weapon_submission_ready,
        );
        let gun_level = self.effective_gun_level();
        let bomb_level = self.effective_bomb_level();

        if in_safe {
            if weapon_submission_ready {
                self.handle_safe_zone_normal_weapon_inputs(
                    shift_down,
                    ctrl_down,
                    can_fastshoot,
                    heading,
                    tuning,
                    render_input,
                );
            }
            return;
        }

        // Movement, recharge and non-weapon semantic actions remain live when
        // the drawn combat view is temporarily held. Weapon prediction does
        // not spend energy or create a projectile until the exact prepared
        // receipt can be admitted by the authoritative server. Only unshifted
        // Control is a level-triggered primary-fire input; its press edge is
        // handled above so brief taps still survive zero-step render frames.
        if !weapon_submission_ready {
            return;
        }

        if can_fastshoot
            && held_primary_fire_down(shift_down, ctrl_down, shift_free_fire_down(render_input))
            && self.ship.bullet_cooldown <= 0.0
        {
            let multifire_active = self.ship.multifire_capable && self.ship.multifire;
            if ball_carrier_should_kick(self.powerball.carried, self.tuning.allow_guns) {
                self.kick_powerball_with_weapon_delay(heading);
            } else if multifire_active
                && gun_level > 0
                && weapon_energy_available(
                    self.ship.energy,
                    self.effective_multifire_energy_cost(tuning).required,
                )
            {
                self.fire_multifire();
            } else if !multifire_active
                && gun_level > 0
                && weapon_energy_available(
                    self.ship.energy,
                    self.effective_bullet_energy_cost(tuning).required,
                )
            {
                self.fire_bullet(heading);
            }
        }

        if mine_input_down(shift_down, render_input) && self.ship.bomb_cooldown <= 0.0 {
            if ball_carrier_should_kick(self.powerball.carried, self.tuning.allow_bombs) {
                self.kick_powerball_with_weapon_delay(heading);
            } else if bomb_level > 0
                && weapon_energy_available(
                    self.ship.energy,
                    self.effective_mine_energy_cost(tuning).required,
                )
                && self.mine_drop_allowed_for_heading(heading, tuning)
            {
                self.drop_mine(heading);
            }
        }

        if bomb_input_down(shift_down, render_input)
            && self.ship.bomb_cooldown <= 0.0
            && can_fastshoot
        {
            if ball_carrier_should_kick(self.powerball.carried, self.tuning.allow_bombs) {
                self.kick_powerball_with_weapon_delay(heading);
            } else if bomb_level > 0
                && weapon_energy_available(
                    self.ship.energy,
                    self.effective_bomb_energy_cost(tuning).required,
                )
                && self.proximity_bomb_safe_for_step(bomb_level)
            {
                self.fire_bomb(heading);
            }
        }
    }

    /// BombSafety exactly as the server judges this step's bomb: the reported,
    /// rounded ship position against the remote ships of the snapshot named by
    /// the step's combat-view receipt. Offline play, or a step without that
    /// recorded view, uses the latest positions.
    fn proximity_bomb_safe_for_step(&self, bomb_level: u8) -> bool {
        let radius = continuum_proximity_bomb_firing_safety_radius(
            self.tuning.proximity_distance,
            bomb_level,
        );
        let shooter = if self.networked_play {
            vec2(
                crate::continuum_position::protocol_position_coord(self.ship.pos.x) as f32,
                crate::continuum_position::protocol_position_coord(self.ship.pos.y) as f32,
            )
        } else {
            self.ship.pos
        };
        let view = self
            .prepared_combat_view_receipt
            .filter(|prepared| {
                self.networked_play
                    && prepared.authority_segment == self.client_authority.view().segment
                    && prepared.receipt.input_sequence == self.next_input_seq
                    && prepared.receipt.weapon_submission_ready()
            })
            .and_then(|prepared| {
                self.combat_view_ship_positions
                    .at(prepared.receipt.latest_game_applied_snapshot_id)
            });
        match view {
            Some(view) => {
                !self.tuning.bomb_safety
                    || !self.ship.proximity_bombs
                    || !proximity_bomb_safety_blocked_by_view(
                        shooter,
                        view,
                        &self.network_world,
                        self.player_frequency(),
                        radius,
                    )
            }
            None => proximity_bomb_safe_to_fire(
                self.tuning.bomb_safety,
                self.ship.proximity_bombs,
                shooter,
                &self.bots,
                &self.network_world,
                self.player_frequency(),
                self.tuning,
                radius,
            ),
        }
    }

    fn handle_semantic_ship_actions(
        &mut self,
        actions: &[SemanticInputAction],
        in_safe: bool,
        can_fastshoot: bool,
        heading: Vec2,
        tuning: ShipTuning,
        rocket_actions: RocketActionSequencer,
        weapon_submission_ready: bool,
    ) {
        for action in actions {
            if let Some(index) = semantic_input_metric_index(action.kind) {
                self.semantic_input_counts[index] =
                    self.semantic_input_counts[index].saturating_add(1);
            }
            match action.kind {
                SemanticInputActionKind::ToggleMultifire => self.toggle_multifire(),
                SemanticInputActionKind::ToggleCloak => self.toggle_cloak(),
                SemanticInputActionKind::ToggleStealth => self.toggle_stealth(),
                SemanticInputActionKind::ToggleAntiwarp => self.toggle_antiwarp(),
                SemanticInputActionKind::ToggleXradar => self.toggle_xradar(),
                SemanticInputActionKind::Attach => self.handle_attach_action(),
                SemanticInputActionKind::PrimaryFireOrSpectate => {
                    if weapon_submission_ready {
                        self.try_semantic_gun(in_safe, can_fastshoot, heading, tuning);
                    }
                }
                SemanticInputActionKind::ChatAutocompleteOrBomb
                | SemanticInputActionKind::BombOrSpectate => {
                    if weapon_submission_ready {
                        self.try_semantic_bomb(in_safe, can_fastshoot, heading, tuning);
                    }
                }
                SemanticInputActionKind::Mine => {
                    if weapon_submission_ready {
                        self.try_semantic_mine(in_safe, heading, tuning);
                    }
                }
                SemanticInputActionKind::Repel => {
                    if weapon_submission_ready
                        && repel_weapon_available(
                            self.ship.repel_cooldown,
                            in_safe,
                            self.ship.energy,
                            0.0,
                            self.ship.repels,
                        )
                    {
                        self.fire_repel();
                    }
                }
                SemanticInputActionKind::Burst => {
                    if weapon_submission_ready
                        && self.ship.bomb_cooldown <= 0.0
                        && self.ship.burst_cooldown <= 0.0
                        && special_weapon_energy_available(in_safe, self.ship.energy, 0.0)
                        && self.ship.bursts > 0
                    {
                        self.fire_burst();
                    }
                }
                SemanticInputActionKind::Thor => {
                    if weapon_submission_ready
                        && can_fastshoot
                        && self.ship.bomb_cooldown <= 0.0
                        && special_weapon_energy_available(in_safe, self.ship.energy, 0.0)
                        && self.ship.thors > 0
                    {
                        self.fire_thor(heading);
                    }
                }
                SemanticInputActionKind::Decoy => {
                    if weapon_submission_ready
                        && self.ship.bomb_cooldown <= 0.0
                        && special_weapon_energy_available(in_safe, self.ship.energy, 0.0)
                        && self.ship.decoys > 0
                    {
                        self.launch_decoy();
                    }
                }
                SemanticInputActionKind::Brick => {
                    if weapon_submission_ready
                        && !in_safe
                        && self.ship.bomb_cooldown <= 0.0
                        && self.ship.bricks > 0
                    {
                        self.drop_brick(heading);
                    }
                }
                SemanticInputActionKind::Rocket => {
                    let activation_available =
                        rocket_available(self.ship.rocket_timer, self.ship.bomb_cooldown)
                            && self.ship.rockets > 0;
                    if let Some(activation) =
                        rocket_actions.activation_for_action(action.kind, activation_available)
                    {
                        self.activate_rocket(activation);
                    }
                }
                SemanticInputActionKind::PlacePortal => {
                    // Portal is placed after this frame's movement. Bind its
                    // spend to that exact completed canonical tail; Rocket's
                    // adjacent action deliberately begins on the next tail.
                    self.place_portal(rocket_actions.completed_movement_sequence())
                }
                SemanticInputActionKind::Warp | SemanticInputActionKind::RandomWarp => {
                    if ball_carrier_warp_should_kick(self.powerball.carried) {
                        self.kick_powerball_with_weapon_delay(heading);
                    } else {
                        self.activate_warp_mode(
                            true,
                            rocket_actions.completed_movement_sequence(),
                            action.kind == SemanticInputActionKind::RandomWarp,
                        );
                    }
                }
                _ => {}
            }
        }
    }

    fn try_semantic_gun(
        &mut self,
        in_safe: bool,
        can_fastshoot: bool,
        heading: Vec2,
        tuning: ShipTuning,
    ) {
        if !can_fastshoot || self.ship.bullet_cooldown > 0.0 {
            return;
        }
        let gun_level = self.effective_gun_level();
        let multifire_active = self.ship.multifire_capable && self.ship.multifire;
        if ball_carrier_should_kick(self.powerball.carried, self.tuning.allow_guns) {
            self.kick_powerball_with_weapon_delay(heading);
        } else if multifire_active
            && gun_level > 0
            && weapon_energy_available(
                self.ship.energy,
                self.effective_multifire_energy_cost(tuning).required,
            )
        {
            if in_safe {
                self.apply_safe_zone_gun_delay(tuning.multifire_cooldown);
            } else {
                self.fire_multifire();
            }
        } else if !multifire_active
            && gun_level > 0
            && weapon_energy_available(
                self.ship.energy,
                self.effective_bullet_energy_cost(tuning).required,
            )
        {
            if in_safe {
                self.apply_safe_zone_gun_delay(tuning.bullet_cooldown);
            } else {
                self.fire_bullet(heading);
            }
        }
    }

    fn try_semantic_mine(&mut self, in_safe: bool, heading: Vec2, tuning: ShipTuning) {
        if self.ship.bomb_cooldown > 0.0 {
            return;
        }
        let bomb_level = self.effective_bomb_level();
        if ball_carrier_should_kick(self.powerball.carried, self.tuning.allow_bombs) {
            self.kick_powerball_with_weapon_delay(heading);
        } else if bomb_level > 0
            && weapon_energy_available(
                self.ship.energy,
                self.effective_mine_energy_cost(tuning).required,
            )
            && self.mine_drop_allowed_for_heading(heading, tuning)
        {
            if in_safe {
                self.apply_safe_zone_bomb_delay(tuning, tuning.bomb_cooldown);
            } else {
                self.drop_mine(heading);
            }
        }
    }

    fn try_semantic_bomb(
        &mut self,
        in_safe: bool,
        can_fastshoot: bool,
        heading: Vec2,
        tuning: ShipTuning,
    ) {
        if self.ship.bomb_cooldown > 0.0 || !can_fastshoot {
            return;
        }
        let bomb_level = self.effective_bomb_level();
        if ball_carrier_should_kick(self.powerball.carried, self.tuning.allow_bombs) {
            self.kick_powerball_with_weapon_delay(heading);
        } else if bomb_level > 0
            && weapon_energy_available(
                self.ship.energy,
                self.effective_bomb_energy_cost(tuning).required,
            )
            && self.proximity_bomb_safe_for_step(bomb_level)
        {
            if in_safe {
                self.apply_safe_zone_bomb_delay(tuning, tuning.bomb_cooldown);
            } else {
                self.fire_bomb(heading);
            }
        }
    }

    fn handle_safe_zone_normal_weapon_inputs(
        &mut self,
        shift_down: bool,
        ctrl_down: bool,
        can_fastshoot: bool,
        heading: Vec2,
        tuning: ShipTuning,
        render_input: &RenderInputContext,
    ) {
        let gun_level = self.effective_gun_level();
        let bomb_level = self.effective_bomb_level();
        if can_fastshoot
            && held_primary_fire_down(shift_down, ctrl_down, shift_free_fire_down(render_input))
            && self.ship.bullet_cooldown <= 0.0
        {
            let multifire_active = self.ship.multifire_capable && self.ship.multifire;
            if ball_carrier_should_kick(self.powerball.carried, self.tuning.allow_guns) {
                self.kick_powerball_with_weapon_delay(heading);
            } else if multifire_active
                && gun_level > 0
                && weapon_energy_available(
                    self.ship.energy,
                    self.effective_multifire_energy_cost(tuning).required,
                )
            {
                self.apply_safe_zone_gun_delay(tuning.multifire_cooldown);
            } else if !multifire_active
                && gun_level > 0
                && weapon_energy_available(
                    self.ship.energy,
                    self.effective_bullet_energy_cost(tuning).required,
                )
            {
                self.apply_safe_zone_gun_delay(tuning.bullet_cooldown);
            }
        }

        if mine_input_down(shift_down, render_input) && self.ship.bomb_cooldown <= 0.0 {
            if ball_carrier_should_kick(self.powerball.carried, self.tuning.allow_bombs) {
                self.kick_powerball_with_weapon_delay(heading);
            } else if bomb_level > 0
                && weapon_energy_available(
                    self.ship.energy,
                    self.effective_mine_energy_cost(tuning).required,
                )
                && self.mine_drop_allowed_for_heading(heading, tuning)
            {
                self.apply_safe_zone_bomb_delay(tuning, tuning.bomb_cooldown);
            }
        }

        if bomb_input_down(shift_down, render_input)
            && self.ship.bomb_cooldown <= 0.0
            && can_fastshoot
        {
            if ball_carrier_should_kick(self.powerball.carried, self.tuning.allow_bombs) {
                self.kick_powerball_with_weapon_delay(heading);
            } else if bomb_level > 0
                && weapon_energy_available(
                    self.ship.energy,
                    self.effective_bomb_energy_cost(tuning).required,
                )
                && self.proximity_bomb_safe_for_step(bomb_level)
            {
                self.apply_safe_zone_bomb_delay(tuning, tuning.bomb_cooldown);
            }
        }
    }

    fn apply_safe_zone_gun_delay(&mut self, cooldown: f32) {
        self.ship.bullet_cooldown = cooldown;
        self.ship.bomb_cooldown = self.ship.bomb_cooldown.max(cooldown);
        self.ship.vel = safe_zone_weapon_velocity();
        self.last_input.safe_zone_weapon_stop = true;
    }

    fn apply_safe_zone_bomb_delay(&mut self, tuning: ShipTuning, base_cooldown: f32) {
        self.ship.bomb_cooldown = self.effective_bomb_cooldown(base_cooldown);
        if !tuning.emp_bomb {
            self.ship.bullet_cooldown = self.ship.bullet_cooldown.max(self.ship.bomb_cooldown);
            self.ship.repel_cooldown = repel_cooldown_after_weapon(self.ship.repel_cooldown);
        }
        self.ship.vel = safe_zone_weapon_velocity();
        self.last_input.safe_zone_weapon_stop = true;
    }

    pub(super) fn handle_attach_action(&mut self) {
        let tuning = self.ship_tuning();
        let target_id = nearest_attach_target(
            &self.network_world,
            self.ship.pos,
            self.player_bounty,
            self.ship.energy,
            tuning,
            &self.tuning,
        );
        match attach_action_for_target(
            &self.network_world,
            target_id,
            self.player_bounty,
            self.ship.energy,
            tuning,
            &self.tuning,
        ) {
            Ok(AttachAction::Detach) => {
                self.send_network_attach_request(INVALID_PLAYER_ID);
                if let Some(self_id) = self.client_authority.view().scope.player_id {
                    self.network_world
                        .apply_event(&ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
                            requester_id: self_id,
                            destination_id: Some(INVALID_PLAYER_ID),
                        }));
                }
            }
            Ok(AttachAction::DropAttached) => {
                self.send_network_drop_attached();
                if let Some(self_id) = self.client_authority.view().scope.player_id {
                    self.network_world
                        .apply_event(&ClientProtocolEvent::TurretLinkDestroyed(
                            DestroyTurretLink { player_id: self_id },
                        ));
                }
            }
            Ok(AttachAction::Attach(player_id)) => {
                if antiwarp_settle_active(self.ship.fake_antiwarp_timer) {
                    self.push_notification(
                        "AntiWarp engaged.",
                        Color::from_rgba(255, 220, 95, 255),
                    );
                    return;
                }
                self.send_network_attach_request(player_id);
                self.ship.fake_antiwarp_timer = self.tuning.antiwarp_settle_seconds;
                if let Some(self_id) = self.client_authority.view().scope.player_id {
                    self.network_world
                        .apply_event(&ClientProtocolEvent::TurretLinkCreated(CreateTurretLink {
                            requester_id: self_id,
                            destination_id: Some(player_id),
                        }));
                }
            }
            Err(message) => {
                self.push_notification(message, Color::from_rgba(255, 220, 95, 255));
            }
        }
    }
}

/// Shift+Control is Repel, so Control fires only unshifted; `,` and X fire with
/// or without the afterburner.
fn held_primary_fire_down(shift_down: bool, ctrl_down: bool, shift_free_down: bool) -> bool {
    (!shift_down && ctrl_down) || shift_free_down
}

fn shift_free_fire_down(render_input: &RenderInputContext) -> bool {
    render_input.is_down(KeyCode::Comma) || render_input.is_down(KeyCode::X)
}

/// Tab, Z and `.` bomb, or lay a mine while Shift is held.
fn bomb_key_down(render_input: &RenderInputContext) -> bool {
    render_input.is_down(KeyCode::Tab)
        || render_input.is_down(KeyCode::Z)
        || render_input.is_down(KeyCode::Period)
}

fn bomb_input_down(shift_down: bool, render_input: &RenderInputContext) -> bool {
    !shift_down && bomb_key_down(render_input)
}

fn mine_input_down(shift_down: bool, render_input: &RenderInputContext) -> bool {
    (shift_down && bomb_key_down(render_input)) || render_input.is_down(KeyCode::M)
}

fn held_respawn_action_bits(
    shift_down: bool,
    ctrl_down: bool,
    render_input: &RenderInputContext,
) -> u16 {
    let keys = [
        KeyCode::Tab,
        KeyCode::Z,
        KeyCode::M,
        KeyCode::Delete,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Insert,
        KeyCode::F3,
        KeyCode::F4,
        KeyCode::F5,
        KeyCode::F6,
        KeyCode::F7,
        KeyCode::Period,
    ];
    let mut bits = 0_u16;
    for (index, key) in keys.into_iter().enumerate() {
        if render_input.is_down(key) {
            bits |= 1 << index;
        }
    }
    // Held primary fire from any layout.
    if ctrl_down || shift_free_fire_down(render_input) {
        bits |= 1 << 13;
    }
    if shift_down && ctrl_down {
        bits |= 1 << 14;
    }
    bits
}

impl RespawnInputLatch {
    pub(in crate::game) fn arm(&mut self) {
        *self = Self {
            armed: true,
            ..Self::default()
        };
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    fn should_suppress(
        &mut self,
        movement_bits: u16,
        action_bits: u16,
        has_new_gameplay_key_press: bool,
        has_new_semantic_action: bool,
    ) -> bool {
        debug_assert!(self.armed);
        if has_new_gameplay_key_press || has_new_semantic_action {
            self.clear();
            return false;
        }
        if !self.captured {
            self.captured = true;
            self.movement_bits = movement_bits;
            self.action_bits = action_bits;
            if movement_bits == 0 && action_bits == 0 {
                self.clear();
                return false;
            }
            return true;
        }

        let newly_pressed_movement = movement_bits & !self.movement_bits != 0;
        let newly_pressed_action = action_bits & !self.action_bits != 0;
        self.movement_bits &= movement_bits;
        self.action_bits &= action_bits;
        if newly_pressed_movement || newly_pressed_action || has_new_semantic_action {
            self.clear();
            return false;
        }
        if self.movement_bits == 0 && self.action_bits == 0 {
            self.clear();
            return false;
        }
        true
    }
}

fn safe_zone_weapon_velocity() -> Vec2 {
    Vec2::ZERO
}

#[cfg(test)]
mod tests;
