use super::*;
use crate::game::network::rocket_state_resolves_pending_activation;
use crate::input::RenderInputContext;
use crate::protocol::SnapshotRocketState;

fn render_prediction_test_game() -> Game {
    let mut game = Game::new_network_authority_test();
    game.networked_play = false;
    game.spectator_mode = false;
    game.player_respawn_timer = 0.0;
    game.respawn_input_latch = RespawnInputLatch::default();
    game
}

fn lifecycle_request_coasting_game() -> Game {
    use crate::game::lifecycle_sequencer::ClientAuthorityReadiness;
    use crate::protocol::AuthorityScopeIdentity;
    let mut game = render_prediction_test_game();
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(7),
        arena_stream_id: Some(11),
        player_id: Some(0),
        life_seq: Some(2),
        alive: Some(true),
    };
    game.client_authority.install_test_scope(
        1,
        scope,
        ClientAuthorityReadiness {
            joined: true,
            arena_epoch: Some(11),
            settings: true,
            security: true,
            map: true,
            files: true,
        },
    );
    game.networked_play = true;
    game.network_prize_baseline_pending = false;
    game.door_clock_fresh = true;
    game.ship.ship_type = 6;
    game.ship.vel = vec2(0.47771907, -0.92877483);
    game.ship.orientation = 0.4824993;
    game.ship.energy = game.ship.max_energy - 100.0;
    game.next_input_seq = 26629;
    game
}

#[test]
fn rejected_ship_request_cannot_leave_an_unrecorded_movement_step() {
    use crate::game::lifecycle_sequencer::ClientAuthorityDisposition;
    use crate::protocol::ShipChangeRejected;

    let mut game = lifecycle_request_coasting_game();
    let before = (
        game.ship.pos,
        game.ship.vel,
        game.ship.orientation,
        game.ship.energy,
    );
    // The frame was admitted while Live. Its menu action submits the ship
    // request before update_ship runs; this closes publication for sequence N.
    assert!(game.gameplay_step_ready());
    game.client_authority
        .queue_lifecycle_intent(PendingLifecycleRequest::Ship(0));
    game.client_authority
        .record_ship_request_submitted(0, 26629);
    assert!(!game.gameplay_step_ready());
    game.update_ship(
        GAMEPLAY_STEP_SECONDS,
        &mut RenderInputContext::default(),
        &[],
        true,
        true,
    );
    assert_eq!(
        (
            game.ship.pos,
            game.ship.vel,
            game.ship.orientation,
            game.ship.energy
        ),
        before,
        "the menu's unpublished boundary step cannot advance canonical motion or energy"
    );
    assert_eq!(game.next_input_seq, 26629);

    assert_eq!(
        game.client_authority
            .observe_ship_rejection(ShipChangeRejected {
                ship: 0,
                server_session_id: 7,
                arena_stream_id: 11,
                player_id: 0,
                life_seq: 2,
                position_sequence: 26629,
            }),
        ClientAuthorityDisposition::Applied
    );
    assert!(game.gameplay_step_ready());
    game.update_ship(
        GAMEPLAY_STEP_SECONDS,
        &mut RenderInputContext::default(),
        &[],
        true,
        true,
    );
    assert_eq!(
        game.ship.pos,
        before.0 + before.1 * GAMEPLAY_STEP_SECONDS,
        "resumed sequence N consumes exactly one movement step, including retained coasting velocity"
    );
    assert_eq!(
        game.ship.ship_type, 6,
        "rejection keeps the server-confirmed ship"
    );
    assert_eq!(game.client_authority.view().scope.life_seq, Some(2));
}

#[test]
fn frequency_intent_keeps_the_step_owned_by_its_post_movement_baseline() {
    let mut game = lifecycle_request_coasting_game();
    let before = game.ship.pos;
    let velocity = game.ship.vel;
    assert!(game.gameplay_step_ready());
    // Frequency submission intentionally belongs to the network tail after
    // movement, unlike the ship request submitted by the menu action itself.
    game.client_authority
        .queue_lifecycle_intent(PendingLifecycleRequest::Frequency(9));
    game.update_ship(
        GAMEPLAY_STEP_SECONDS,
        &mut RenderInputContext::default(),
        &[],
        true,
        true,
    );
    assert_eq!(game.ship.pos, before + velocity * GAMEPLAY_STEP_SECONDS);
    assert_eq!(
        game.client_authority.view().lifecycle_request_sequence,
        None
    );
}

#[test]
fn local_render_prediction_obeys_current_wall_door_and_brick_geometry() {
    for (tile, seed, friendly, blocked) in [
        (1, 0, false, true),
        (DOOR_FIRST_TILE_ID, 255, false, true),
        (DOOR_FIRST_TILE_ID, 0, false, false),
        (BRICK_TILE_ID, 0, false, true),
        (BRICK_TILE_ID, 0, true, false),
    ] {
        let mut game = render_prediction_test_game();
        let frequency = game.player_frequency();
        for y in 95..=105 {
            game.maps[game.map_index].set_tile(100, y, tile);
            if tile == BRICK_TILE_ID {
                game.bricks.push(Brick {
                    tile: ivec2(100, y),
                    previous_tile: 0,
                    team: if friendly {
                        frequency
                    } else {
                        frequency.wrapping_add(1)
                    },
                    ttl: 10.0,
                    age: 0.0,
                    server_timestamp: None,
                });
            }
        }
        game.maps[game.map_index].door_seed = seed;
        let wall_edge = 100.0 * TILE_SIZE;
        let radius = game.ship_tuning().radius_pixels;
        game.ship.pos = vec2(wall_edge - radius - 2.0, 100.5 * TILE_SIZE);
        game.ship.vel = vec2(600.0, 0.0);
        let origin = game.ship.pos;
        let input = RenderInputContext::default();
        let preview = game.local_render_prediction(0.01, &input);
        if blocked {
            assert!(
                preview.position.x + radius <= wall_edge,
                "tile={tile}, seed={seed}"
            );
            assert!(preview.velocity.x < 0.0);
        } else {
            assert!(preview.position.x > origin.x + 5.9);
            assert!(preview.velocity.x > 0.0);
        }
        assert_eq!(game.ship.pos, origin);
        assert_eq!(game.ship.vel, vec2(600.0, 0.0));
    }
}

#[test]
fn local_render_prediction_respects_respawn_latch_and_recomputes_after_relocation() {
    let mut game = render_prediction_test_game();
    let mut input = RenderInputContext::default();
    game.respawn_input_latch.arm();
    input.merge_keys([KeyCode::Left], []);
    let held = game.local_render_prediction(0.01, &input);
    assert_eq!(held.partial_seconds, 0.0);
    let latch = game.respawn_input_latch;
    input.merge_keys([KeyCode::Left, KeyCode::Up], [KeyCode::Up]);
    assert!(game.local_render_prediction(0.01, &input).partial_seconds > 0.0);
    assert_eq!(
        game.respawn_input_latch, latch,
        "draw cannot release the real latch"
    );
    game.player_respawn_timer = 1.0;
    assert_eq!(
        game.local_render_prediction(0.01, &input).partial_seconds,
        0.0
    );
    game.player_respawn_timer = 0.0;
    game.ship.pos = vec2(1500.0, 1500.0);
    game.ship.vel = Vec2::ZERO;
    game.ship.orientation = 0.5;
    let relocated = game.local_render_prediction(0.01, &input);
    assert_eq!(relocated.position, game.ship.pos);
    assert!(relocated.orientation < 0.5);
    game.chat.active = true;
    let chat = game.local_render_prediction(0.01, &input);
    assert_eq!(chat.orientation, game.ship.orientation);
    assert_eq!(chat.input_bits, 0);
    game.spectator_mode = true;
    assert_eq!(
        game.local_render_prediction(0.01, &input).partial_seconds,
        0.0
    );
}

#[test]
fn local_render_prediction_is_disposable_and_never_spends_or_records_input() {
    let mut game = Game::new_network_authority_test();
    game.networked_play = false;
    game.spectator_mode = false;
    game.player_respawn_timer = 0.0;
    game.respawn_input_latch = RespawnInputLatch::default();
    game.ship.orientation = 0.5;
    game.ship.vel = Vec2::ZERO;
    game.ship.energy = 1_000.0;
    let mut input = RenderInputContext::default();
    input.merge_keys(
        [KeyCode::Left, KeyCode::Up, KeyCode::LeftShift],
        [KeyCode::Left],
    );
    let canonical = (
        game.ship.pos,
        game.ship.vel,
        game.ship.orientation,
        game.ship.energy,
        game.next_input_seq,
        game.ship.rocket_timer,
        game.repel_movement_sequence_window,
    );
    let preview = game.local_render_prediction(0.0042, &input);
    assert!(preview.orientation < game.ship.orientation);
    assert_ne!(preview.velocity, game.ship.vel);
    assert_eq!(preview.input_bits & 5, 5);
    for _ in 0..240 {
        assert_eq!(game.local_render_prediction(0.0042, &input), preview);
    }
    assert_eq!(
        canonical,
        (
            game.ship.pos,
            game.ship.vel,
            game.ship.orientation,
            game.ship.energy,
            game.next_input_seq,
            game.ship.rocket_timer,
            game.repel_movement_sequence_window
        )
    );
    assert!(!input.pending_actions().is_empty() || input.take_gameplay_key_press());
    input.merge_keys([], []);
    let released = game.local_render_prediction(0.0042, &input);
    assert_eq!(released.orientation, game.ship.orientation);
    assert_eq!(released.input_bits, 0);
    assert_eq!(
        game.local_render_prediction(1.0, &input).partial_seconds,
        crate::fixed_step::GAMEPLAY_STEP_SECONDS
    );
    for invalid in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
        assert_eq!(
            game.local_render_prediction(invalid, &input)
                .partial_seconds,
            0.0
        );
    }
    game.networked_play = true;
    assert!(!game.local_gameplay_ready());
    assert_eq!(
        game.local_render_prediction(0.0042, &input).partial_seconds,
        0.0
    );
    game.networked_play = false;
    game.suppress_gameplay_step_after_network_rebase = true;
    assert_eq!(
        game.local_render_prediction(0.0042, &input).partial_seconds,
        0.0
    );
}

#[test]
fn safe_zone_weapon_use_stops_ship_like_reference() {
    assert_eq!(safe_zone_weapon_velocity(), Vec2::ZERO);
}

#[test]
fn client_repel_window_retires_before_a_complete_sequence_epoch_can_rearm_it() {
    let application_sequence = u16::MAX - 2;
    let original =
        crate::gameplay_rules::repel_sequence_window_from_active_steps(application_sequence, 3);
    let mut installed = Some(original);
    let mut active_sequences = Vec::new();

    // Drive through the original wrapped interval and then through the same
    // wire values one complete u16 epoch later. Only the first epoch may own
    // RepelSpeed.
    for logical_offset in 0..=(u32::from(u16::MAX) + 2) {
        let sequence = application_sequence.wrapping_add(logical_offset as u16);
        if repel_movement_sequence_active_for_input(&mut installed, sequence).unwrap_or(false) {
            active_sequences.push(logical_offset);
        }
    }

    assert_eq!(active_sequences, [1, 2, 3]);
    assert_eq!(installed, None);
    assert!(crate::gameplay_rules::repel_sequence_window_contains(
        original,
        application_sequence.wrapping_add(1),
    ));
    assert_eq!(
        repel_movement_sequence_active_for_input(
            &mut installed,
            application_sequence.wrapping_add(1),
        ),
        None,
        "the recurring u16 value has no retained authority"
    );
}

#[test]
fn held_primary_fire_is_unshifted_control_comma_or_x() {
    assert!(held_primary_fire_down(false, true, false));
    assert!(!held_primary_fire_down(true, true, false));
    assert!(!held_primary_fire_down(false, false, false));
    assert!(!held_primary_fire_down(true, false, false));
    // `,` and X keep firing while Shift runs the afterburner.
    assert!(held_primary_fire_down(false, false, true));
    assert!(held_primary_fire_down(true, false, true));
    for key in [KeyCode::Comma, KeyCode::X] {
        assert!(shift_free_fire_down(&RenderInputContext::from_keys(
            [key],
            []
        )));
    }
    let other_keys =
        RenderInputContext::from_keys([KeyCode::LeftControl, KeyCode::Z, KeyCode::Period], []);
    assert!(!shift_free_fire_down(&other_keys));
}

#[test]
fn held_bomb_and_bullet_keys_of_every_layout_hold_across_respawn() {
    for key in [KeyCode::Tab, KeyCode::Period, KeyCode::Z] {
        let held = RenderInputContext::from_keys([key], []);
        assert!(bomb_input_down(false, &held));
        assert!(!mine_input_down(false, &held));
        assert!(
            !bomb_input_down(true, &held),
            "{key:?} with Shift is a mine"
        );
        assert!(mine_input_down(true, &held));
        assert_ne!(held_respawn_action_bits(false, false, &held), 0);
    }
    assert!(!bomb_input_down(
        false,
        &RenderInputContext::from_keys([KeyCode::X], [])
    ));

    for key in [KeyCode::Comma, KeyCode::X] {
        assert_eq!(
            held_respawn_action_bits(false, false, &RenderInputContext::from_keys([key], [])),
            held_respawn_action_bits(false, true, &RenderInputContext::default()),
            "held {key:?} is the same primary-fire hold as Control"
        );
    }
}

#[test]
fn held_fire_waits_for_prepared_combat_view_without_blocking_movement() {
    let mut game = Game::new_network_authority_test();
    game.ship.energy = game.ship.max_energy;
    game.ship.bullet_cooldown = 0.0;
    game.ship.bomb_cooldown = 0.0;
    let mut input = RenderInputContext::default();
    input.merge_keys(
        [KeyCode::Left, KeyCode::LeftControl],
        [KeyCode::Left, KeyCode::LeftControl],
    );

    let energy_before = game.ship.energy;
    let orientation_before = game.ship.orientation;
    game.update_ship(GAMEPLAY_STEP_SECONDS, &mut input, &[], true, false);
    assert_ne!(
        game.ship.orientation, orientation_before,
        "movement remains live while held"
    );
    assert!(game.bullets.is_empty());
    assert_eq!(game.pending_weapon, 0);
    assert_eq!(game.ship.energy, energy_before);

    assert!(
        input.is_down(KeyCode::LeftControl),
        "the physical level remains available to a later ready input"
    );
}

#[test]
fn respawn_latch_does_not_block_new_rotation_while_old_fire_remains_held() {
    let mut latch = RespawnInputLatch::default();
    latch.arm();
    let held_space = 1_u16;
    assert!(latch.should_suppress(0, held_space, false, false));
    assert!(
        !latch.should_suppress(PlayerInput::BIT_ROTATE_LEFT, held_space, true, false,),
        "a newly pressed turn is intentional post-respawn input"
    );
    assert!(!latch.armed);
}

#[test]
fn respawn_latch_accepts_first_fresh_fire_or_turn_even_before_a_neutral_sample() {
    for (movement_bits, action_bits) in [
        (PlayerInput::BIT_ROTATE_LEFT, 0),
        (PlayerInput::BIT_ROTATE_RIGHT, 0),
        (0, 1),
    ] {
        let mut latch = RespawnInputLatch::default();
        latch.arm();
        assert!(!latch.should_suppress(movement_bits, action_bits, true, false));
        assert!(!latch.armed);
    }
}

#[test]
fn respawn_latch_keeps_only_the_exact_across_respawn_holds_suppressed() {
    let mut latch = RespawnInputLatch::default();
    latch.arm();
    let stale = PlayerInput::BIT_THRUST;
    assert!(latch.should_suppress(stale, 0, false, false));
    assert!(latch.should_suppress(stale, 0, false, false));
    assert!(!latch.should_suppress(0, 0, false, false));
    assert!(!latch.armed);
}

fn queued_actions(frames: &[KeyCode]) -> Vec<SemanticInputAction> {
    let mut input = RenderInputContext::default();
    for key in frames {
        input.merge_keys([*key], [*key]);
        // A 144 Hz frame can execute no logical step. Releasing before the
        // next captured edge preserves both actions in chronological order.
        input.merge_keys([], []);
    }
    input.drain_actions()
}

fn rocket_state(
    observed_input_sequence: Option<u16>,
    activation_sequence: Option<u16>,
    input_sequence_watermark: Option<u16>,
) -> SnapshotRocketState {
    SnapshotRocketState {
        life_seq: 1,
        observed_input_sequence,
        input_sequence_watermark,
        activation_sequence,
        until_tick: activation_sequence.map(|_| 1_000),
        active: activation_sequence.is_some(),
        rockets: u8::from(activation_sequence.is_none()) + 2,
    }
}

#[test]
fn queued_primary_then_rocket_binds_pulse_to_following_input() {
    let actions = queued_actions(&[KeyCode::LeftControl, KeyCode::F3]);
    assert_eq!(
        actions.iter().map(|action| action.kind).collect::<Vec<_>>(),
        [
            SemanticInputActionKind::PrimaryFireOrSpectate,
            SemanticInputActionKind::Rocket,
        ]
    );

    let current_sequence = 42_u16;
    let sequencer = RocketActionSequencer::after_movement(current_sequence);
    let current_input = PlayerInput::default();
    assert_eq!(
        current_input.bits() & PlayerInput::BIT_ROCKET_ACTIVATION_PULSE,
        0,
        "record N is already formed before the later Rocket edge"
    );
    assert_eq!(sequencer.activation_for_action(actions[0].kind, true), None);
    let activation = sequencer
        .activation_for_action(actions[1].kind, true)
        .expect("queued Rocket should activate after movement");
    assert_eq!(activation.pulse_sequence, 43);

    let mut following_input = PlayerInput::default();
    following_input.rocket_activation_pulse = true;
    assert_ne!(
        following_input.bits() & PlayerInput::BIT_ROCKET_ACTIVATION_PULSE,
        0,
        "record N+1 carries the one-record Rocket activation pulse"
    );

    let accepted = rocket_state(Some(43), Some(43), Some(43));
    let rejected = rocket_state(Some(43), None, Some(43));
    assert!(rocket_state_resolves_pending_activation(
        Some(activation.pulse_sequence),
        accepted,
    ));
    assert!(rocket_state_resolves_pending_activation(
        Some(activation.pulse_sequence),
        rejected,
    ));
    assert!(!rocket_state_resolves_pending_activation(
        Some(current_sequence),
        accepted,
    ));
}

#[test]
fn catch_up_defers_semantic_chord_to_published_step_before_rocket_pulse() {
    let mut input = RenderInputContext::default();
    input.merge_keys([KeyCode::F3, KeyCode::Z], [KeyCode::F3, KeyCode::Z]);

    assert!(Game::drain_actions_for_gameplay_step(&mut input, false).is_empty());
    assert!(Game::drain_actions_for_gameplay_step(&mut input, false).is_empty());
    let actions = Game::drain_actions_for_gameplay_step(&mut input, true);
    assert_eq!(
        actions.iter().map(|action| action.kind).collect::<Vec<_>>(),
        [
            SemanticInputActionKind::Rocket,
            SemanticInputActionKind::BombOrSpectate,
        ]
    );

    // The final catch-up step publishes the bomb-bearing position at N. Its
    // Rocket action is processed after movement and therefore arms only N+1.
    let published_weapon_sequence = 700_u16;
    let activation = RocketActionSequencer::after_movement(published_weapon_sequence)
        .activation_for_action(actions[0].kind, true)
        .expect("Rocket action on the published step");
    assert_eq!(activation.pulse_sequence, 701);
    assert_ne!(activation.pulse_sequence, published_weapon_sequence);
}

#[test]
fn catch_up_weapon_phase_is_bound_to_the_published_input_tail() {
    // The real update loop passes this same final-step bit to both semantic
    // action draining and `update_ship`. A held weapon key therefore cannot
    // apply bomb recoil on either unpublished catch-up record.
    let published_steps = [false, false, true].map(weapon_phase_due_on_input_step);
    assert_eq!(
        published_steps
            .into_iter()
            .enumerate()
            .filter_map(|(index, published)| published.then_some(index))
            .collect::<Vec<_>>(),
        [2]
    );
}

#[test]
fn multiple_older_edges_keep_rocket_on_following_input() {
    let actions = queued_actions(&[KeyCode::LeftControl, KeyCode::F4, KeyCode::F5, KeyCode::F3]);
    let sequencer = RocketActionSequencer::after_movement(900);

    let activations = actions
        .iter()
        .filter_map(|action| sequencer.activation_for_action(action.kind, true))
        .collect::<Vec<_>>();
    assert_eq!(
        activations,
        [RocketActivation {
            pulse_sequence: 901,
        }]
    );
}

#[test]
fn leading_rocket_and_bomb_bind_rocket_to_next_input_in_action_order() {
    let actions = queued_actions(&[KeyCode::F3, KeyCode::Z]);
    assert_eq!(
        actions.iter().map(|action| action.kind).collect::<Vec<_>>(),
        [
            SemanticInputActionKind::Rocket,
            SemanticInputActionKind::BombOrSpectate,
        ],
        "the reference game's weapon phase processes F3 before the bomb"
    );
    let sequencer = RocketActionSequencer::after_movement(77);
    let current_input = PlayerInput::default();
    assert_eq!(
        current_input.bits() & PlayerInput::BIT_ROCKET_ACTIVATION_PULSE,
        0,
        "movement/input record N is formed before the weapon phase"
    );
    let activation = sequencer
        .activation_for_action(actions[0].kind, true)
        .expect("leading Rocket activates after movement");
    assert_eq!(activation.pulse_sequence, 78);
    assert_eq!(
        sequencer.activation_for_action(actions[1].kind, false),
        None
    );
}

#[test]
fn real_fixed_steps_apply_leading_rocket_only_on_n_plus_one() {
    fn movement(rocket_active: bool) -> AppliedThrust {
        step_player_movement(
            PlayerMovementState {
                orientation: 0.0,
                velocity: MovementVector::ZERO,
                energy: 1_000.0,
                max_energy: 1_700.0,
            },
            PlayerMovementInput {
                thrust: true,
                ..Default::default()
            },
            PlayerMovementTuning {
                rotation_per_second: 0.5,
                shutdown_rotation_per_second: 0.1,
                base_thrust_acceleration: 150.0,
                afterburner_thrust_acceleration: 300.0,
                rocket_thrust_acceleration: 500.0,
                base_top_speed: 200.0,
                afterburner_top_speed: 400.0,
                rocket_top_speed: 600.0,
                afterburner_energy_per_second: 120.0,
                recharge_per_second: 200.0,
                turret_thrust_penalty: 40.0,
                turret_speed_penalty: 50.0,
                repel_top_speed: 800.0,
                flagger_thrust_adjustment: 25.0,
                flagger_speed_adjustment: 20.0,
            },
            PlayerMovementEnvironment {
                rocket_active,
                ..Default::default()
            },
            crate::fixed_step::GAMEPLAY_STEP_SECONDS,
        )
        .applied_thrust
    }

    let sequence_n = 500_u16;
    let mut timer = 0.0;
    let step_n = crate::gameplay_rules::begin_rocket_movement_step(
        &mut timer,
        crate::fixed_step::GAMEPLAY_STEP_SECONDS,
        None,
        sequence_n,
    );
    assert!(!step_n.active);
    assert!(!step_n.activation_pulse);
    assert_eq!(movement(step_n.active), AppliedThrust::Forward);

    // The real ordered weapon phase runs after N's movement/input sample.
    let activation = RocketActionSequencer::after_movement(sequence_n)
        .activation_for_action(SemanticInputActionKind::Rocket, true)
        .expect("F3 activates after movement N");
    timer = 10.0;
    let step_n_plus_one = crate::gameplay_rules::begin_rocket_movement_step(
        &mut timer,
        crate::fixed_step::GAMEPLAY_STEP_SECONDS,
        Some(activation.pulse_sequence),
        sequence_n.wrapping_add(1),
    );
    assert!(step_n_plus_one.active);
    assert!(step_n_plus_one.activation_pulse);
    assert_eq!(movement(step_n_plus_one.active), AppliedThrust::Rocket);
}

#[test]
fn unavailable_rocket_creates_no_activation_binding() {
    for actions in [
        queued_actions(&[KeyCode::F3]),
        queued_actions(&[KeyCode::LeftControl, KeyCode::F3]),
    ] {
        let sequencer = RocketActionSequencer::after_movement(123);
        let mut pending_sequence = None;
        for action in actions {
            if let Some(activation) = sequencer.activation_for_action(action.kind, false) {
                pending_sequence = Some(activation.pulse_sequence);
            }
        }
        assert_eq!(pending_sequence, None);
    }
}

#[test]
fn every_rocket_binding_wraps_from_max_to_zero() {
    let actions = queued_actions(&[KeyCode::F3]);
    let sequencer = RocketActionSequencer::after_movement(u16::MAX);
    let activation = actions
        .iter()
        .find_map(|action| sequencer.activation_for_action(action.kind, true))
        .expect("wrapped post-movement Rocket activation");
    assert_eq!(activation.pulse_sequence, 0);
    assert!(rocket_state_resolves_pending_activation(
        Some(0),
        rocket_state(Some(0), Some(0), Some(0)),
    ));
    assert!(rocket_state_resolves_pending_activation(
        Some(0),
        rocket_state(Some(0), None, Some(0)),
    ));
}

#[test]
fn portal_action_names_the_completed_movement_tail_across_wrap() {
    let current = u16::MAX;
    let sequencer = RocketActionSequencer::after_movement(current);
    assert_eq!(sequencer.next_input_sequence, 0);
    assert_eq!(sequencer.completed_movement_sequence(), current);
}

#[test]
fn activation_is_one_wire_record_and_rolling_recovery_preserves_that_exact_pulse() {
    let start = u16::MAX - 1;
    let activation = RocketActivation::on_next_input(start);
    let mut ring = ClientInputRing::default();
    let mut latest = None;
    for sequence in [start, u16::MAX, 0, 1] {
        let mut input = PlayerInput::default();
        input.rocket_activation_pulse = crate::gameplay_rules::rocket_activation_pulse_for_sequence(
            Some(activation.pulse_sequence),
            sequence,
        );
        latest = Some(ring.record(
            sequence,
            PlayerInputRecord {
                bits: input.bits(),
                direction: 0,
                door_seed: 0,
                combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(
                    sequence,
                ),
            },
        ));
    }
    let encoded = latest.expect("rolling packet").encode();
    let recovered = ClientInputRecordPacket::parse(&encoded).expect("wire round trip");
    let pulse_sequences = recovered
        .inputs
        .iter()
        .enumerate()
        .filter_map(|(index, input)| {
            (input.bits & PlayerInput::BIT_ROCKET_ACTIVATION_PULSE != 0)
                .then(|| recovered.base_seq.wrapping_add(index as u16))
        })
        .collect::<Vec<_>>();
    assert_eq!(pulse_sequences, [start]);

    let immediate_retrigger = RocketActivation::on_next_input(
        crate::gameplay_rules::rocket_activation_sequence_after_movement(u16::MAX),
    );
    assert_eq!(immediate_retrigger.pulse_sequence, 0);
    assert!(crate::gameplay_rules::rocket_activation_pulse_for_sequence(
        Some(immediate_retrigger.pulse_sequence),
        0,
    ));
    assert!(
        !crate::gameplay_rules::rocket_activation_pulse_for_sequence(
            Some(immediate_retrigger.pulse_sequence),
            1,
        )
    );
}

#[test]
fn bomb_safety_semantic_fire_uses_tile_radius_not_the_contact_sensor() {
    for (distance, allowed) in [
        (47.99, false),
        (48.0, false),
        (48.01, true),
        (50.0, true),
        (54.0, true),
    ] {
        let mut game = render_prediction_test_game();
        game.networked_play = false;
        game.tuning.bomb_safety = true;
        game.tuning.proximity_distance = 1.0;
        game.ship.proximity_bombs = true;
        game.ship.bomb_level = 3;
        game.ship.bomb_cooldown = 0.0;
        game.ship.energy = game.ship.max_energy;
        let mut enemy =
            crate::network::NetworkPlayer::from_roster(&crate::protocol::PlayerRosterEntry {
                ship: 0,
                audio: 0,
                name: "safety-target".into(),
                squad: String::new(),
                kill_points: 0,
                flag_points: 0,
                player_id: 9,
                frequency: game.player_frequency().wrapping_add(1),
                wins: 0,
                losses: 0,
                attach_parent: INVALID_PLAYER_ID,
                flags: 0,
                koth: 0,
            });
        enemy.position = Some(game.ship.pos + vec2(distance, 0.0));
        game.network_world.players.insert(9, enemy);
        let tuning = game.ship_tuning();
        assert_eq!(game.effective_bomb_level(), 3);
        assert!(weapon_energy_available(
            game.ship.energy,
            game.effective_bomb_energy_cost(tuning).required
        ));
        // Safe-tile delay proves actual semantic fire admission without needing
        // graphics/audio. Normal/semantic/safe held fire share this rule.
        game.try_semantic_bomb(true, true, vec2(1.0, 0.0), tuning);
        assert_eq!(
            game.ship.bomb_cooldown > 0.0,
            allowed,
            "distance={distance}"
        );
        assert_eq!(
            proximity_bomb_trigger_radius(1.0, 3),
            54.0,
            "the independently expanded collision sensor must remain unchanged"
        );
    }
}

#[test]
fn held_repel_wall_expiry_is_wrap_safe_and_never_rearms_a_retired_window() {
    let mut game = Game::new_network_authority_test();
    let expiry = 3;
    game.pending_repel_application_expiry_tick = Some(expiry);
    game.repel_movement_sequence_window = Some((u16::MAX - 4, 7));
    game.ship.repel_timer = 1.0;
    game.expire_pending_repel_application_at_tick(crate::continuum::CONTINUUM_TICK_MASK);
    assert_eq!(game.pending_repel_application_expiry_tick, Some(expiry));
    game.expire_pending_repel_application_at_tick(expiry);
    assert_eq!(game.repel_movement_sequence_window, Some((u16::MAX - 4, 7)));
    let active =
        repel_movement_sequence_active_for_input(&mut game.repel_movement_sequence_window, 7);
    assert_eq!(active, Some(false));
    assert_eq!(game.repel_movement_sequence_window, None);
    game.expire_pending_repel_application_at_tick(4);
    assert_eq!(game.repel_movement_sequence_window, None);
}
