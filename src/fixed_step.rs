/// Deterministic logical gameplay cadence shared by client prediction and the
/// server's input-sequence replay model.
pub(crate) use crate::gameplay_rules::{GAMEPLAY_STEP_SECONDS, MAX_GAMEPLAY_CATCH_UP_STEPS};
/// Presentation follows real render time, but a scheduler hitch should be
/// smoothed over subsequent frames instead of collapsing an entire buffered
/// remote interpolation segment into one visible jump. Regular frames inside
/// the gameplay catch-up budget retain their elapsed time; only longer stalls
/// fall back to the existing bounded recovery interval.
pub(crate) fn bounded_presentation_step_seconds(render_dt_seconds: f32) -> f32 {
    crate::combat_timeline::bounded_observer_frame_seconds(render_dt_seconds)
}

/// Render frames may arrive at 60, 40, 30 Hz or after a short scheduler stall.
/// Accumulate wall time and expose only bounded 60 Hz logical steps. Excess
/// stall time is deliberately discarded instead of creating an unbounded
/// physics/packet burst.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FixedGameplayClock {
    accumulator_seconds: f64,
}

impl FixedGameplayClock {
    pub(crate) fn advance_when_ready(&mut self, render_dt_seconds: f32, ready: bool) -> u8 {
        if ready {
            self.advance(render_dt_seconds)
        } else {
            self.discard_elapsed();
            0
        }
    }

    pub(crate) fn advance(&mut self, render_dt_seconds: f32) -> u8 {
        let step = f64::from(GAMEPLAY_STEP_SECONDS);
        let max_accumulator = step * f64::from(MAX_GAMEPLAY_CATCH_UP_STEPS);
        let elapsed = f64::from(render_dt_seconds.max(0.0)).min(max_accumulator);
        self.accumulator_seconds = (self.accumulator_seconds + elapsed).min(max_accumulator);
        // Render deltas arrive as f32; tolerate sub-microsecond accumulation
        // error so repeated `1.0 / 60.0` frames do not lose a logical step.
        let steps = ((self.accumulator_seconds + step * 1.0e-5) / step).floor() as u8;
        let steps = steps.min(MAX_GAMEPLAY_CATCH_UP_STEPS);
        self.accumulator_seconds = (self.accumulator_seconds - step * f64::from(steps)).max(0.0);
        steps
    }

    /// Readiness gaps are not scheduler stalls. Discard all partial gameplay
    /// time so recovery begins with a fresh canonical 60 Hz sample instead of
    /// replaying wall time accumulated while local simulation was forbidden.
    pub(crate) fn discard_elapsed(&mut self) {
        self.accumulator_seconds = 0.0;
    }

    pub(crate) fn remainder_seconds(self) -> f64 {
        self.accumulator_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gameplay::{
        MovementVector, PlayerMovementEnvironment, PlayerMovementInput, PlayerMovementState,
        PlayerMovementTuning, door_update_count_and_next_timer, integrate_player_collision,
        sample_wormhole_gravity, step_player_movement,
    };
    use crate::input::RenderInputContext;
    use crate::protocol::{ClientInputRing, PlayerInputRecord};
    use macroquad::prelude::KeyCode;
    use std::collections::BTreeMap;

    fn logical_steps(schedule: &[f32]) -> u32 {
        let mut clock = FixedGameplayClock::default();
        schedule
            .iter()
            .map(|&dt| u32::from(clock.advance(dt)))
            .sum()
    }

    #[test]
    fn sixty_forty_thirty_and_high_refresh_schedules_produce_same_logical_steps() {
        assert_eq!(logical_steps(&vec![1.0 / 60.0; 180]), 180);
        assert_eq!(logical_steps(&vec![1.0 / 40.0; 120]), 180);
        assert_eq!(logical_steps(&vec![1.0 / 30.0; 90]), 180);
        assert_eq!(logical_steps(&vec![1.0 / 144.0; 432]), 180);
    }

    #[test]
    fn jitter_preserves_elapsed_steps_and_long_stalls_are_bounded() {
        let jitter: Vec<_> = [0.010, 0.030, 0.020, 0.040]
            .into_iter()
            .cycle()
            .take(40)
            .collect();
        assert_eq!(logical_steps(&jitter), 60);

        let mut clock = FixedGameplayClock::default();
        assert_eq!(clock.advance(10.0), MAX_GAMEPLAY_CATCH_UP_STEPS);
        assert!(clock.remainder_seconds() < f64::from(GAMEPLAY_STEP_SECONDS));
        assert_eq!(clock.advance(GAMEPLAY_STEP_SECONDS), 1);
    }

    #[test]
    fn physical_input_can_precede_a_render_with_no_canonical_gameplay_step() {
        // Chrome drew an unchanged frame after keydown before rotating on the
        // following frame. A short render interval pins the clock mechanism.
        // This is not next-draw acceptance: presentation must bridge this gap
        // without manufacturing canonical input records or extra physics steps.
        let mut clock = FixedGameplayClock::default();
        let mut input = RenderInputContext::default();
        assert_eq!(clock.advance(GAMEPLAY_STEP_SECONDS), 1);
        input.merge_keys([KeyCode::Left], [KeyCode::Left]);
        assert!(input.is_down(KeyCode::Left));
        assert_eq!(clock.advance(0.0042), 0);
        assert!(clock.remainder_seconds() > 0.0);
        assert!(clock.remainder_seconds() < f64::from(GAMEPLAY_STEP_SECONDS));
        assert_eq!(clock.advance(0.0164), 1);
        assert!(input.is_down(KeyCode::Left));
    }

    #[test]
    fn invalid_or_hostile_render_deltas_cannot_create_unbounded_simulation() {
        let mut clock = FixedGameplayClock::default();
        assert_eq!(clock.advance(f32::NAN), 0);
        assert_eq!(clock.advance(-1.0), 0);
        assert_eq!(clock.advance(f32::INFINITY), MAX_GAMEPLAY_CATCH_UP_STEPS);
        assert!(clock.remainder_seconds() < f64::from(GAMEPLAY_STEP_SECONDS));
    }

    #[test]
    fn stale_readiness_freezes_complete_local_state_and_discards_weapon_edges() {
        #[derive(Clone, Debug, PartialEq)]
        struct LocalGameplayState {
            position_x: f32,
            velocity_x: f32,
            energy: f32,
            weapon_cooldown: f32,
            status_timer: f32,
            bombs: u16,
            rockets: u16,
            projectile_count: usize,
            semantic_counts: [u32; 3],
            input_ring: ClientInputRing,
            next_input_sequence: u16,
        }

        let mut clock = FixedGameplayClock::default();
        let mut input = RenderInputContext::default();
        let mut local = LocalGameplayState {
            position_x: 1_000.0,
            velocity_x: 7.0,
            energy: 1_234.5,
            weapon_cooldown: 0.4,
            status_timer: 9.0,
            bombs: 3,
            rockets: 2,
            projectile_count: 4,
            semantic_counts: [0; 3],
            input_ring: ClientInputRing::default(),
            next_input_sequence: u16::MAX,
        };
        let frozen = local.clone();
        let mut network_polls = 0_u32;
        let mut presentation_frames = 0_u32;

        for frame in 0..90 {
            let pressed = (frame == 0)
                .then_some([KeyCode::F3, KeyCode::LeftControl, KeyCode::Z])
                .into_iter()
                .flatten();
            input.merge_keys([KeyCode::W], pressed);
            assert_eq!(clock.advance_when_ready(GAMEPLAY_STEP_SECONDS, false), 0);
            let discarded = input.drain_actions();
            if frame == 0 {
                assert_eq!(
                    discarded
                        .iter()
                        .map(|action| action.kind)
                        .collect::<Vec<_>>(),
                    [
                        crate::input::SemanticInputActionKind::Rocket,
                        crate::input::SemanticInputActionKind::PrimaryFireOrSpectate,
                        crate::input::SemanticInputActionKind::BombOrSpectate,
                    ]
                );
            }
            input.finish_gameplay_batch();
            network_polls += 1;
            presentation_frames += 1;
        }

        assert_eq!(
            local, frozen,
            ">1 s stale time must mutate no local gameplay state"
        );
        assert_eq!(
            network_polls, 90,
            "transport polling continues while frozen"
        );
        assert_eq!(
            presentation_frames, 90,
            "remote presentation continues while frozen"
        );
        assert!(
            input.is_down(KeyCode::W),
            "held state remains a live sample"
        );
        assert!(input.pending_actions().is_empty());

        input.merge_keys([KeyCode::W], []);
        assert_eq!(clock.advance_when_ready(GAMEPLAY_STEP_SECONDS, true), 1);
        assert!(input.drain_actions().is_empty(), "stale edges never replay");
        local.velocity_x += 1.0;
        local.position_x += local.velocity_x * GAMEPLAY_STEP_SECONDS;
        let packet = local.input_ring.record(
            local.next_input_sequence,
            PlayerInputRecord {
                bits: u16::from(input.is_down(KeyCode::W)),
                direction: 0,
                door_seed: 7,
                combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(
                    local.next_input_sequence,
                ),
            },
        );
        local.next_input_sequence = local.next_input_sequence.wrapping_add(1);
        assert_eq!(packet.base_seq, u16::MAX);
        assert_eq!(packet.inputs.len(), 1);
        assert_eq!(packet.inputs[0].bits, 1, "only held thrust resumes");
        assert_eq!(local.next_input_sequence, 0);
        assert_eq!(local.semantic_counts, [0; 3]);
    }

    #[test]
    fn pending_render_edges_fire_once_while_held_input_reaches_every_step() {
        fn run(schedule: &[f32]) -> (u32, u32, u32) {
            let mut clock = FixedGameplayClock::default();
            let mut input = RenderInputContext::default();
            let mut edge_actions = 0;
            let mut held_steps = 0;
            let mut logical_steps = 0;
            for (frame, &render_dt) in schedule.iter().enumerate() {
                input.merge_keys(
                    [KeyCode::W],
                    (frame == 0)
                        .then_some([KeyCode::Escape, KeyCode::Insert, KeyCode::F7])
                        .into_iter()
                        .flatten(),
                );
                let batch_steps = clock.advance(render_dt);
                for _ in 0..batch_steps {
                    logical_steps += 1;
                    held_steps += u32::from(input.is_down(KeyCode::W));
                    edge_actions += input.drain_actions().len() as u32;
                }
                if batch_steps > 0 {
                    input.finish_gameplay_batch();
                }
            }
            (logical_steps, held_steps, edge_actions)
        }

        for schedule in [
            vec![1.0 / 60.0; 60],
            vec![1.0 / 30.0; 30],
            vec![1.0 / 144.0; 144],
        ] {
            let (steps, held, edges) = run(&schedule);
            assert_eq!(steps, 60);
            assert_eq!(held, 60);
            assert_eq!(edges, 3);
        }
        let (steps, held, edges) = run(&[1.0 / 30.0, 0.5]);
        assert_eq!(steps, 10);
        assert_eq!(held, 10);
        assert_eq!(edges, 3, "three one-shot actions per input batch");
    }

    #[test]
    fn remote_presentation_uses_render_time_but_smooths_scheduler_hitches() {
        assert_eq!(bounded_presentation_step_seconds(1.0 / 60.0), 1.0 / 60.0);
        assert_eq!(bounded_presentation_step_seconds(1.0 / 40.0), 1.0 / 40.0);
        assert_eq!(bounded_presentation_step_seconds(0.050), 0.050);
        assert_eq!(bounded_presentation_step_seconds(0.100), 0.100);
        assert_eq!(
            bounded_presentation_step_seconds(0.150),
            crate::combat_timeline::OBSERVER_PRESENTATION_MAX_STEP_SECONDS
        );
        assert_eq!(bounded_presentation_step_seconds(-1.0), 0.0);
        assert_eq!(bounded_presentation_step_seconds(f32::NAN), 0.0);
    }

    #[test]
    fn offline_door_clock_uses_full_render_time_when_gameplay_catch_up_is_bounded() {
        fn door_updates(schedule: &[f32]) -> usize {
            let mut timer = 1.2;
            let mut updates = 0;
            for &render_dt in schedule {
                let (count, next_timer) = door_update_count_and_next_timer(timer, render_dt, 1.2);
                updates += count;
                timer = next_timer;
            }
            updates
        }

        let smooth = vec![1.0 / 60.0; 180];
        let stalled = vec![0.3; 10];
        assert_eq!(door_updates(&smooth), 2);
        assert_eq!(door_updates(&stalled), 2);
        assert_eq!(logical_steps(&smooth), 180);
        assert_eq!(logical_steps(&stalled), 80);
    }

    #[test]
    fn catch_up_steps_each_enter_the_rolling_ring_and_recover_dropped_packets() {
        let mut clock = FixedGameplayClock::default();
        let mut ring = ClientInputRing::default();
        let mut received = BTreeMap::new();
        let mut sequence = 100_u16;

        let schedule = [1.0 / 30.0, 0.2, 1.0 / 40.0, 1.0 / 60.0];
        for frame_dt in schedule {
            for _ in 0..clock.advance(frame_dt) {
                let packet = ring.record(
                    sequence,
                    PlayerInputRecord {
                        bits: sequence,
                        direction: (sequence % 40) as u8,
                        door_seed: 0,
                        combat_view_receipt:
                            crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence),
                    },
                );
                // Lose every fifth outbound datagram. Later rolling packets
                // recover its logical step, including the bounded 8-step
                // catch-up burst.
                if !sequence.is_multiple_of(5) {
                    for (offset, input) in packet.inputs.iter().copied().enumerate() {
                        received.insert(packet.base_seq.wrapping_add(offset as u16), input);
                    }
                }
                sequence = sequence.wrapping_add(1);
            }
        }
        // Flush the tail without impairment.
        for _ in 0..8 {
            let packet = ring.record(
                sequence,
                PlayerInputRecord {
                    bits: sequence,
                    direction: (sequence % 40) as u8,
                    door_seed: 0,
                    combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(
                        sequence,
                    ),
                },
            );
            for (offset, input) in packet.inputs.iter().copied().enumerate() {
                received.insert(packet.base_seq.wrapping_add(offset as u16), input);
            }
            sequence = sequence.wrapping_add(1);
        }

        for expected in 100..sequence {
            assert_eq!(
                received.get(&expected).map(|input| input.bits),
                Some(expected)
            );
        }
    }

    #[derive(Clone, Copy, Debug)]
    struct ScheduledMovementResult {
        position: MovementVector,
        state: PlayerMovementState,
        bounce_count: u32,
        gravity_steps: u32,
        logical_steps: u32,
    }

    fn movement_tuning() -> PlayerMovementTuning {
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
        }
    }

    #[test]
    fn fractional_presentation_model_responds_without_advancing_canonical_movement() {
        // Independent render-only schedule model. Re-evaluate canonical state on
        // every draw; never accumulate a preview or feed it into input replay.
        // Production geometry and lifecycle admission have separate runtime
        // regressions; this model isolates elapsed-time/step-count invariants.
        for hz in [60, 120, 144, 240] {
            let mut clock = FixedGameplayClock::default();
            let mut canonical = PlayerMovementState {
                orientation: 0.5,
                velocity: MovementVector::ZERO,
                energy: 1_000.0,
                max_energy: 1_700.0,
            };
            let mut logical_steps = 0;
            let mut zero_step_responses = 0;
            for frame in 0..hz {
                let input = PlayerMovementInput {
                    thrust: true,
                    rotate_left: frame % 2 == 0,
                    rotate_right: frame % 2 != 0,
                    afterburner_held: true,
                    ..Default::default()
                };
                let steps = clock.advance(1.0 / hz as f32);
                for _ in 0..steps {
                    canonical = step_player_movement(
                        canonical,
                        input,
                        movement_tuning(),
                        PlayerMovementEnvironment::default(),
                        GAMEPLAY_STEP_SECONDS,
                    )
                    .state;
                    logical_steps += 1;
                }
                let frozen = canonical;
                let partial_dt = clock.remainder_seconds() as f32;
                assert!((0.0..GAMEPLAY_STEP_SECONDS).contains(&partial_dt));
                if partial_dt > 0.0 {
                    let preview = step_player_movement(
                        canonical,
                        input,
                        movement_tuning(),
                        PlayerMovementEnvironment::default(),
                        partial_dt,
                    );
                    // Independent angular oracle. None of these headings
                    // crosses wrap, and each frame reverses held direction.
                    let sign = if input.rotate_left { -1.0 } else { 1.0 };
                    let expected = canonical.orientation
                        + sign * movement_tuning().rotation_per_second * partial_dt;
                    assert!((preview.state.orientation - expected).abs() < 1.0e-6);
                    if steps == 0 {
                        assert_ne!(preview.state.orientation, canonical.orientation);
                        zero_step_responses += 1;
                    }
                    // Energy is deliberately NOT a presentation output: even
                    // afterburner previews cannot commit a drain or recharge.
                    assert_eq!(canonical, frozen);
                }
            }
            assert_eq!(logical_steps, 60);
            if hz > 60 {
                assert!(zero_step_responses > 0);
            }
        }
    }

    fn run_movement_schedule(schedule: &[f32]) -> ScheduledMovementResult {
        let mut clock = FixedGameplayClock::default();
        let mut position = MovementVector::new(44.0, 72.0);
        let mut state = PlayerMovementState {
            orientation: 0.0,
            velocity: MovementVector::ZERO,
            energy: 1_000.0,
            max_energy: 1_700.0,
        };
        let mut bounce_count = 0;
        let mut gravity_steps = 0;
        let mut logical_steps = 0;

        for &render_dt in schedule {
            for _ in 0..clock.advance(render_dt) {
                logical_steps += 1;
                let collision = integrate_player_collision(
                    position,
                    state.velocity,
                    GAMEPLAY_STEP_SECONDS,
                    0.5,
                    |candidate, _, _| {
                        candidate.x < 20.0
                            || candidate.x > 62.0
                            || candidate.y < 60.0
                            || candidate.y > 100.0
                    },
                );
                let gravity = sample_wormhole_gravity(
                    collision.position,
                    100,
                    500.0,
                    GAMEPLAY_STEP_SECONDS,
                    16.0,
                    |x, y| (2..=6).contains(&x) && (2..=6).contains(&y),
                );
                gravity_steps += u32::from(gravity.velocity_delta.length_squared() > 0.0);
                let movement = step_player_movement(
                    PlayerMovementState {
                        velocity: collision.velocity,
                        ..state
                    },
                    PlayerMovementInput {
                        thrust: true,
                        rotate_right: true,
                        afterburner_held: true,
                        ..Default::default()
                    },
                    movement_tuning(),
                    PlayerMovementEnvironment {
                        wormhole_velocity_delta: gravity.velocity_delta,
                        gravity_top_speed: gravity.top_speed,
                        ..Default::default()
                    },
                    GAMEPLAY_STEP_SECONDS,
                );
                bounce_count += u32::from(collision.bounced);
                position = collision.position;
                state = movement.state;
            }
        }

        ScheduledMovementResult {
            position,
            state,
            bounce_count,
            gravity_steps,
            logical_steps,
        }
    }

    fn assert_movement_close(actual: ScheduledMovementResult, expected: ScheduledMovementResult) {
        let close = |actual: f32, expected: f32| {
            assert!((actual - expected).abs() < 1.0e-4, "{actual} != {expected}");
        };
        close(actual.position.x, expected.position.x);
        close(actual.position.y, expected.position.y);
        close(actual.state.orientation, expected.state.orientation);
        close(actual.state.velocity.x, expected.state.velocity.x);
        close(actual.state.velocity.y, expected.state.velocity.y);
        close(actual.state.energy, expected.state.energy);
        assert_eq!(actual.bounce_count, expected.bounce_count);
        assert_eq!(actual.gravity_steps, expected.gravity_steps);
        assert_eq!(actual.logical_steps, expected.logical_steps);
    }

    #[test]
    fn render_cadence_does_not_change_shared_movement_energy_gravity_or_collision() {
        let sixty_hz = vec![1.0 / 60.0; 180];
        let forty_hz = vec![1.0 / 40.0; 120];
        let thirty_hz = vec![1.0 / 30.0; 90];
        let jitter: Vec<_> = [0.010, 0.030, 0.020, 0.040]
            .into_iter()
            .cycle()
            .take(120)
            .collect();
        let expected = run_movement_schedule(&sixty_hz);

        assert_eq!(expected.logical_steps, 180);
        assert!(expected.bounce_count > 0);
        assert!(expected.gravity_steps > 0);
        assert_ne!(expected.state.orientation, 0.0);
        assert_ne!(expected.state.energy, 1_000.0);
        assert_movement_close(run_movement_schedule(&forty_hz), expected);
        assert_movement_close(run_movement_schedule(&thirty_hz), expected);
        assert_movement_close(run_movement_schedule(&jitter), expected);
    }
}
