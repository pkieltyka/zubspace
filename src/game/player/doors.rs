use super::*;

/// Bound local extrapolation from the newest authenticated server tick. A
/// browser does not receive a fresh server tick every render frame, so freezing
/// at the last sample eventually freezes its door phase while the server keeps
/// advancing. The receipt time is a conservative (network-delayed) anchor;
/// stale/reordered samples cannot refresh or rewind it, and a full second
/// without advancing authority fails closed before the validation window can
/// be exhausted.
const DOOR_CLOCK_MAX_EXTRAPOLATION_SECONDS: f64 =
    crate::gameplay_rules::DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS as f64 / 100.0;

impl Game {
    pub(in crate::game) fn in_safe_zone(&self) -> bool {
        self.map().tile_at_world(self.ship.pos) == SAFE_TILE_ID
    }

    pub(crate) fn update_doors(&mut self, render_dt: f32) {
        // A correction application marker already names the current lineage.
        // Defer any local same-life door relocation until its terminal is
        // applied/cancelled so a late terminal can never overwrite the newer
        // boundary. The synchronized clock catches up all elapsed phases on
        // the first frame after the barrier opens.
        if self.client_authority.view().transition_kind
            == Some(ClientAuthorityTransitionKind::Correction)
        {
            return;
        }
        if self.networked_play && !self.network_security_synchronized {
            self.door_clock_fresh = false;
            return;
        }
        #[cfg(target_arch = "wasm32")]
        let observed_server_tick = (!browser_gameplay_clock_stalled())
            .then(|| self.browser_authenticated_server_tick())
            .flatten();
        #[cfg(not(target_arch = "wasm32"))]
        let observed_server_tick = self.browser_server_tick();
        let dt = if self.network_security_synchronized {
            let step = synchronized_door_clock_step(
                &mut self.door_clock_server_tick,
                &mut self.door_clock_observed_server_tick,
                &mut self.door_clock_anchor_seconds,
                &mut self.door_clock_last_observed_seconds,
                observed_server_tick,
                get_time(),
                DOOR_CLOCK_MAX_EXTRAPOLATION_SECONDS,
            );
            self.door_clock_fresh = step.fresh;
            step.elapsed_seconds
        } else {
            render_dt
        };
        if dt <= 0.0 {
            return;
        }
        let (update_count, next_timer) =
            door_update_count_and_next_timer(self.door_timer, dt, self.tuning.door_delay_seconds);
        self.door_timer = next_timer;
        if update_count == 0 {
            return;
        }

        for _ in 0..update_count {
            let mut warp_positions = Vec::new();
            let reset_powerball;
            let ship_radius = self.ship_tuning().radius_pixels;
            let tuning = self.tuning;

            {
                let map = &mut self.maps[self.map_index];
                let before_seed = map.door_seed;
                advance_map_door_seed_steps(map, self.tuning.door_mode, 1);
                let after_seed = map.door_seed;

                if entity_needs_door_relocation(
                    map,
                    self.ship.pos,
                    ship_radius,
                    before_seed,
                    after_seed,
                ) {
                    self.ship.pos = map.spawn_position();
                    self.ship.vel = Vec2::ZERO;
                    self.pending_same_life_movement_boundary_position = true;
                    warp_positions.push(self.ship.pos);
                }

                for bot in &mut self.bots {
                    let bot_radius = tuning.ship(bot.ship_type).radius_pixels;
                    if bot.respawn_timer <= 0.0
                        && entity_needs_door_relocation(
                            map,
                            bot.pos,
                            bot_radius,
                            before_seed,
                            after_seed,
                        )
                    {
                        bot.pos = map
                            .random_open_position_near(self.ship.pos, 12, 46, 80)
                            .unwrap_or_else(|| map.spawn_position());
                        bot.vel = Vec2::ZERO;
                        bot.fire_cooldown = bot_initial_fire_cooldown(
                            self.tuning.ship(bot.ship_type).bullet_cooldown,
                            bot.id,
                        );
                        warp_positions.push(bot.pos);
                    }
                }

                reset_powerball = !self.powerball.carried
                    && entity_needs_door_relocation(
                        map,
                        self.powerball.pos,
                        8.0,
                        before_seed,
                        after_seed,
                    );
            }

            if reset_powerball {
                self.reset_powerball();
                warp_positions.push(self.powerball.pos);
            }

            for pos in warp_positions {
                self.warps.push(WarpEffect { pos, age: 0.0 });
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SynchronizedDoorClockStep {
    elapsed_seconds: f32,
    fresh: bool,
}

#[allow(clippy::too_many_arguments)]
fn synchronized_door_clock_step(
    estimated_tick: &mut Option<u32>,
    last_observed_tick: &mut Option<u32>,
    estimate_anchor_seconds: &mut f64,
    last_observed_seconds: &mut f64,
    observed_tick: Option<u32>,
    now_seconds: f64,
    max_extrapolation_seconds: f64,
) -> SynchronizedDoorClockStep {
    if !now_seconds.is_finite() || now_seconds < *estimate_anchor_seconds {
        return SynchronizedDoorClockStep::default();
    }

    let authoritative_advanced = observed_tick.is_some_and(|observed| {
        last_observed_tick
            .is_none_or(|previous| crate::continuum::continuum_tick_diff(observed, previous) > 0)
    });
    if authoritative_advanced {
        *last_observed_tick = observed_tick;
        *last_observed_seconds = now_seconds;
    }

    let freshness_age = (now_seconds - *last_observed_seconds).max(0.0);
    if freshness_age > max_extrapolation_seconds.max(0.0) {
        return SynchronizedDoorClockStep::default();
    }

    let Some(previous_estimate) = *estimated_tick else {
        let Some(observed) = observed_tick else {
            return SynchronizedDoorClockStep::default();
        };
        *estimated_tick = Some(observed);
        *last_observed_tick = Some(observed);
        *estimate_anchor_seconds = now_seconds;
        *last_observed_seconds = now_seconds;
        return SynchronizedDoorClockStep {
            elapsed_seconds: 0.0,
            fresh: true,
        };
    };

    let local_elapsed_ticks = ((now_seconds - *estimate_anchor_seconds) * 100.0)
        .floor()
        .clamp(0.0, i32::MAX as f64) as u32;
    let local_estimate =
        crate::continuum::continuum_tick_add(previous_estimate, local_elapsed_ticks);
    let corrected_estimate = observed_tick
        .filter(|observed| crate::continuum::continuum_tick_diff(*observed, local_estimate) > 0)
        .unwrap_or(local_estimate);
    let elapsed_ticks =
        crate::continuum::continuum_tick_diff(corrected_estimate, previous_estimate).max(0);
    if elapsed_ticks > 0 {
        *estimated_tick = Some(corrected_estimate);
        if corrected_estimate == local_estimate {
            *estimate_anchor_seconds += elapsed_ticks as f64 / 100.0;
        } else {
            // A newer authoritative tick corrected the estimate forward. Its
            // receipt becomes the new conservative local anchor;
            // older/reordered samples cannot refresh or rewind it.
            *estimate_anchor_seconds = now_seconds;
        }
    }
    SynchronizedDoorClockStep {
        elapsed_seconds: crate::continuum::ticks_to_seconds_i32(elapsed_ticks),
        fresh: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{DOOR_CLOCK_MAX_EXTRAPOLATION_SECONDS, synchronized_door_clock_step};

    fn step(
        estimated: &mut Option<u32>,
        observed: &mut Option<u32>,
        anchor: &mut f64,
        last_observed: &mut f64,
        sample: Option<u32>,
        now: f64,
    ) -> super::SynchronizedDoorClockStep {
        synchronized_door_clock_step(
            estimated,
            observed,
            anchor,
            last_observed,
            sample,
            now,
            DOOR_CLOCK_MAX_EXTRAPOLATION_SECONDS,
        )
    }

    #[test]
    fn security_rebind_waits_for_a_strictly_forward_authoritative_clock_sample() {
        let mut estimated = Some(2_310);
        let mut observed = Some(2_310);
        let mut anchor = 10.0;
        let mut last_observed_seconds = f64::NEG_INFINITY;

        let same = step(
            &mut estimated,
            &mut observed,
            &mut anchor,
            &mut last_observed_seconds,
            Some(2_310),
            10.1,
        );
        assert_eq!(same, super::SynchronizedDoorClockStep::default());
        assert_eq!(estimated, Some(2_310));

        let forward = step(
            &mut estimated,
            &mut observed,
            &mut anchor,
            &mut last_observed_seconds,
            Some(2_641),
            10.2,
        );
        assert!(forward.fresh);
        assert_eq!(forward.elapsed_seconds, 3.31);
        assert_eq!(estimated, Some(2_641));
    }

    #[test]
    fn synchronized_door_clock_extrapolates_from_authority_and_never_rewinds() {
        let mut estimated = Some(1_000);
        let mut observed = Some(1_000);
        let mut anchor = 10.0;
        let mut last_observed = 10.0;

        let extrapolated = step(
            &mut estimated,
            &mut observed,
            &mut anchor,
            &mut last_observed,
            None,
            10.25,
        );
        assert_eq!(extrapolated.elapsed_seconds, 0.25);
        assert!(extrapolated.fresh);
        assert_eq!(estimated, Some(1_025));

        let reordered = step(
            &mut estimated,
            &mut observed,
            &mut anchor,
            &mut last_observed,
            Some(999),
            10.30,
        );
        assert_eq!(reordered.elapsed_seconds, 0.05);
        assert_eq!(estimated, Some(1_030));
        assert_eq!(observed, Some(1_000));

        let corrected = step(
            &mut estimated,
            &mut observed,
            &mut anchor,
            &mut last_observed,
            Some(1_050),
            10.31,
        );
        assert_eq!(corrected.elapsed_seconds, 0.20);
        assert_eq!(estimated, Some(1_050));
        assert_eq!(anchor, 10.31);
    }

    #[test]
    fn synchronized_door_clock_handles_wrap_and_gates_stale_extrapolation() {
        let mask = crate::continuum::CONTINUUM_TICK_MASK;
        let mut estimated = Some(mask - 3);
        let mut observed = Some(mask - 3);
        let mut anchor = 20.0;
        let mut last_observed = 20.0;
        let wrapped = step(
            &mut estimated,
            &mut observed,
            &mut anchor,
            &mut last_observed,
            Some(2),
            20.01,
        );
        assert_eq!(wrapped.elapsed_seconds, 0.06);
        assert_eq!(estimated, Some(2));

        let stale = step(
            &mut estimated,
            &mut observed,
            &mut anchor,
            &mut last_observed,
            None,
            21.02,
        );
        assert!(!stale.fresh);
        assert_eq!(stale.elapsed_seconds, 0.0);
        assert_eq!(estimated, Some(2), "stale extrapolation must freeze");

        let recovered = step(
            &mut estimated,
            &mut observed,
            &mut anchor,
            &mut last_observed,
            Some(242),
            21.03,
        );
        assert!(recovered.fresh);
        assert_eq!(recovered.elapsed_seconds, 2.4);
        assert_eq!(estimated, Some(242));
    }

    #[test]
    fn synchronized_door_clock_stays_exact_through_loss_reorder_and_render_stalls() {
        let mut estimated = Some(1_000);
        let mut observed = Some(1_000);
        let mut anchor = 0.0;
        let mut last_observed = 0.0;
        let schedule = [
            (0.25, None),
            (0.50, Some(1_020)),
            (1.00, None),
            (1.50, Some(1_140)),
            (2.25, Some(1_220)),
            (2.50, None),
            (3.25, Some(1_300)),
            (3.50, Some(1_100)), // heavily reordered, never a rewind
        ];
        for (now, sample) in schedule {
            let result = step(
                &mut estimated,
                &mut observed,
                &mut anchor,
                &mut last_observed,
                sample,
                now,
            );
            assert!(result.fresh, "schedule must stay inside freshness bound");
        }
        assert_eq!(
            estimated,
            Some(1_350),
            "erratic browser pacing must equal the bounded absolute estimate"
        );
        assert_eq!(
            observed,
            Some(1_300),
            "the reordered sample must not replace the newest authority"
        );

        let stale = step(
            &mut estimated,
            &mut observed,
            &mut anchor,
            &mut last_observed,
            None,
            4.375,
        );
        assert!(!stale.fresh);
        assert_eq!(estimated, Some(1_350));

        let recovered = step(
            &mut estimated,
            &mut observed,
            &mut anchor,
            &mut last_observed,
            Some(1_450),
            4.50,
        );
        assert!(recovered.fresh);
        assert_eq!(estimated, Some(1_450));
    }
}
