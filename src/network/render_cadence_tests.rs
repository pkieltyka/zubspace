//! Isolate the render-only-frame hypothesis using the production fixed clock,
//! observer clock policy, snapshot history, and projection. This is a paired
//! native diagnostic, not browser acceptance or a substitute for Game admission.
use super::*;
use crate::combat_timeline::{
    ClientObserverCombatTimeline, OBSERVER_DELAY_SLEW_INTERVAL_TICKS,
    OBSERVER_PRESENTATION_HOLD_GRACE_TICKS, ObserverNetworkProfile, ObserverTimelineDirectiveV1,
};
use crate::fixed_step::{FixedGameplayClock, bounded_presentation_step_seconds};

fn player() -> NetworkPlayer {
    let mut player = NetworkPlayer::from_roster(&PlayerRosterEntry {
        ship: 0,
        audio: 0,
        name: "cadence".into(),
        squad: String::new(),
        kill_points: 0,
        flag_points: 0,
        player_id: 9,
        frequency: 7,
        wins: 0,
        losses: 0,
        attach_parent: INVALID_PLAYER_ID,
        flags: 0,
        koth: 0,
    });
    player.set_life_seq(Some(4));
    player
}

fn timeline() -> ClientObserverCombatTimeline {
    let envelope = ObserverNetworkProfile::CleanLan.delay_envelope();
    let mut timeline = ClientObserverCombatTimeline::default();
    timeline.observe_directive(
        ObserverTimelineDirectiveV1 {
            generation: 1,
            issued_server_tick: 1_000,
            effective_server_tick: 1_000,
            effective_from_input_sequence: 0,
            from_delay_ticks: envelope.target_ticks,
            target_delay_ticks: envelope.target_ticks,
            ceiling_delay_ticks: envelope.ceiling_ticks,
            slew_interval_ticks: OBSERVER_DELAY_SLEW_INTERVAL_TICKS as u8,
            hold_grace_ticks: OBSERVER_PRESENTATION_HOLD_GRACE_TICKS,
            profile: ObserverNetworkProfile::CleanLan,
        },
        0,
    );
    timeline
}

fn apply_sample(player: &mut NetworkPlayer, id: u16, tick: u32, speed: f32, turns: bool) {
    // The analytically specified path is independent of the interpolator.
    // Reversals represent a wall contact, not an explicit relocation/Flash.
    let travel = (tick - 1_000) as f32 * speed / 100.0;
    let phase = travel % 800.0;
    let (x, velocity) = if !turns {
        (6_000.0 + travel, speed)
    } else if phase < 400.0 {
        (6_000.0 + phase, speed)
    } else {
        (6_800.0 - phase, -speed)
    };
    let update = PlayerPositionUpdate {
        player_id: 9,
        direction: 10,
        timestamp: tick as u16,
        x: x.round() as u16,
        y: 1_000,
        velocity_x: (velocity * 10.0) as i16,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon: 0,
        checksum: None,
        energy: Some(1_000),
        s2c_latency: None,
        flag_timer: None,
        items: None,
    };
    let NetworkPositionApply::Applied { snap_reason, .. } = player.apply_position(&update) else {
        panic!("monotonic received sample must apply");
    };
    player.record_presentation_snap(snap_reason);
    player.record_directed_snapshot(id, tick, snap_reason);
}

#[test]
fn clean_high_refresh_deferred_application_matches_eager_projection_with_stalls() {
    let mut schedules = 0;
    let mut total_frames = 0;
    let mut zero_step_frames_with_pending = 0;
    let mut maximum_application_delay_micros = 0;
    let mut maximum_active_application_delay_micros = 0;
    let mut maximum_display_difference = 0.0_f32;
    let mut maximum_display_step = 0.0_f32;
    for hz in [60_u64, 120, 144, 240] {
        for phase_micros in [0_u64, 1_000, 4_000, 8_000, 12_000, 16_000] {
            for stall_micros in [0_u64, 93_000, 150_000] {
                for speed in [300.0_f32, 1_200.0] {
                    for turns in [false, true] {
                        schedules += 1;
                        let mut eager = player();
                        let mut deferred = player();
                        let mut eager_timeline = timeline();
                        let mut deferred_timeline = timeline();
                        let mut clock = FixedGameplayClock::default();
                        let mut pending = Vec::new();
                        let mut next_snapshot = 1_u16;
                        let mut previous_micros = 0;
                        let mut previous_position: Option<Vec2> = None;
                        for frame in 1..=hz * 3 {
                            let now =
                                frame * 1_000_000 / hz + if frame >= hz { stall_micros } else { 0 };
                            let dt = (now - previous_micros) as f32 / 1_000_000.0;
                            previous_micros = now;
                            // Near snapshots originate at 60 Hz. Arrival phase
                            // is fixed per run; no loss, injected jitter, or
                            // invented future sample can hide this boundary.
                            while u64::from(next_snapshot) * 1_000_000 / 60 + phase_micros <= now {
                                let emitted = u64::from(next_snapshot) * 1_000_000 / 60;
                                let tick = 1_000 + (emitted / 10_000) as u32;
                                apply_sample(&mut eager, next_snapshot, tick, speed, turns);
                                eager_timeline.observe_game_applied_snapshot(next_snapshot, tick);
                                pending.push((next_snapshot, tick, now, frame));
                                next_snapshot += 1;
                            }
                            let steps = clock.advance(dt);
                            if steps > 0 {
                                for (id, tick, received, received_frame) in pending.drain(..) {
                                    maximum_application_delay_micros =
                                        maximum_application_delay_micros.max(now - received);
                                    // No application can run inside the deliberately
                                    // suspended browser. Report that wall interval
                                    // separately from active zero-step deferral.
                                    let suspended = if received_frame < hz && frame >= hz {
                                        stall_micros
                                    } else {
                                        0
                                    };
                                    maximum_active_application_delay_micros =
                                        maximum_active_application_delay_micros
                                            .max(now - received - suspended);
                                    apply_sample(&mut deferred, id, tick, speed, turns);
                                    deferred_timeline.observe_game_applied_snapshot(id, tick);
                                }
                            } else if !pending.is_empty() {
                                zero_step_frames_with_pending += 1;
                            }
                            let server_tick = 1_000 + (now / 10_000) as u32;
                            let presentation_dt = bounded_presentation_step_seconds(dt);
                            let eager_tick = eager_timeline
                                .advance_presentation_render_tick(server_tick, presentation_dt)
                                .unwrap();
                            let deferred_tick = deferred_timeline
                                .advance_presentation_render_tick(server_tick, presentation_dt)
                                .unwrap();
                            assert_eq!(eager_tick, deferred_tick);
                            eager.update_directed_render_position(eager_tick, presentation_dt);
                            deferred
                                .update_directed_render_position(deferred_tick, presentation_dt);
                            assert_eq!(
                                eager.display_position().is_some(),
                                deferred.display_position().is_some()
                            );
                            if eager_tick >= 1_002 {
                                assert!(
                                    eager.display_position().is_some(),
                                    "both paths returning no sprite cannot satisfy the comparison"
                                );
                            }
                            if let (Some(control), Some(actual)) =
                                (eager.display_position(), deferred.display_position())
                            {
                                if !turns {
                                    let expected_x =
                                        6_000.0 + (eager_tick - 1_000) as f32 * speed / 100.0;
                                    assert!(
                                        (control.x - expected_x).abs() <= 0.5,
                                        "straight projection must follow the independent path within one wire rounding half-pixel"
                                    );
                                }
                                let difference = control.distance(actual);
                                maximum_display_difference =
                                    maximum_display_difference.max(difference);
                                assert_eq!(
                                    difference, 0.0,
                                    "hz={hz} phase={phase_micros} stall={stall_micros} speed={speed} turns={turns} frame={frame}"
                                );
                                if let Some(previous) = previous_position {
                                    let step = previous.distance(actual);
                                    maximum_display_step = maximum_display_step.max(step);
                                    // Recovery is bounded at 200 clock ticks/s;
                                    // add one quantization tick and wire pixel.
                                    let bound = speed * (2.0 * presentation_dt + 0.01) + 1.0;
                                    assert!(
                                        step <= bound,
                                        "unexplained projection jump: {step} > {bound}"
                                    );
                                }
                                previous_position = Some(actual);
                            }
                            assert_eq!(deferred.presentation_snap_count, 0);
                            total_frames += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(schedules, 288);
    assert!(
        zero_step_frames_with_pending > 0,
        "must realize the suspected boundary"
    );
    assert!(maximum_active_application_delay_micros <= 16_667);
    println!(
        "{}",
        serde_json::json!({
            "schedules": schedules, "frames": total_frames,
            "zeroStepFramesWithPending": zero_step_frames_with_pending,
            "maximumApplicationDelayMicros": maximum_application_delay_micros,
            "maximumActiveApplicationDelayMicros": maximum_active_application_delay_micros,
            "maximumDisplayDifference": maximum_display_difference,
            "maximumDisplayStep": maximum_display_step,
        })
    );
}
