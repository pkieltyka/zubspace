use super::*;

impl NetworkWorld {
    pub(super) fn apply_objective_score_event(&mut self, update: ObjectiveScoreUpdate) {
        if self
            .objective_score_versions
            .get(&update.frequency)
            .is_some_and(|current| {
                update.generation < current.generation
                    || update.generation == current.generation && update.version < current.version
            })
        {
            return;
        }
        self.objective_score_versions.insert(
            update.frequency,
            NetworkObjectiveScoreIdentity {
                generation: update.generation,
                version: update.version,
            },
        );
        self.soccer_scores.insert(update.frequency, update.score);
    }

    pub(super) fn apply_objective_state_event(&mut self, update: ObjectiveStateUpdate) {
        let key = (update.kind, update.objective_id);
        let previous = self.objective_versions.get(&key).copied();
        if previous.is_some_and(|current| {
            update.generation < current.generation
                || update.generation == current.generation
                    && (update.version < current.version
                        || update.version == current.version
                            && crate::continuum::continuum_tick_diff(
                                update.server_tick,
                                current.last_update_tick,
                            ) < 0)
        }) {
            if update.kind == ObjectiveStateUpdate::KIND_FLAG {
                self.flag_authoritative_stale_rejects += 1;
            }
            return;
        }
        let same_semantic_state = previous.is_some_and(|current| {
            current.generation == update.generation && current.version == update.version
        });
        let (last_action_id, last_outcome) = if update.action_id == 0 && same_semantic_state {
            previous
                .map(|current| (current.last_action_id, current.last_outcome))
                .unwrap_or((0, update.outcome))
        } else {
            (update.action_id, update.outcome)
        };
        self.objective_versions.insert(
            key,
            NetworkObjectiveIdentity {
                generation: update.generation,
                version: update.version,
                last_update_tick: update.server_tick,
                last_action_id,
                last_outcome,
            },
        );
        // Instrumentation for the flag presentation defect: how many authoritative
        // flag states arrived, and how many declared a carrier. Compare against the
        // legacy `FlagPosition` counters in the same payload to see which path a
        // client's presentation actually came from.
        if update.kind == ObjectiveStateUpdate::KIND_FLAG {
            self.flag_authoritative_state_events += 1;
            if update.owner_id.is_some() {
                self.flag_authoritative_carried_events += 1;
            }
            self.flag_last_authoritative_version = update.version;
            self.flag_last_authoritative_state = update.state;
            self.flag_last_authoritative_x = update.x;
            self.flag_last_authoritative_y = update.y;
            self.flag_last_authoritative_tick = update.server_tick;
        }
        match update.kind {
            ObjectiveStateUpdate::KIND_FLAG => self.apply_authoritative_flag_state(update),
            ObjectiveStateUpdate::KIND_POWERBALL => self.apply_authoritative_ball_state(update),
            _ => {}
        }
    }

    fn apply_authoritative_flag_state(&mut self, update: ObjectiveStateUpdate) {
        if update.state == ObjectiveStateUpdate::STATE_WAITING {
            self.flags.remove(&update.objective_id);
            self.reconcile_authoritative_flag_counts();
            return;
        }
        let owner = update.owner_frequency.unwrap_or(u16::MAX);
        self.flags
            .entry(update.objective_id)
            .and_modify(|flag| {
                flag.owner = owner;
                flag.position = vec2(update.x as f32, update.y as f32);
                flag.dropped = update.state == ObjectiveStateUpdate::STATE_ON_MAP;
                flag.turf = false;
                flag.carried_by = update.owner_id;
            })
            .or_insert(NetworkFlag {
                flag_id: update.objective_id,
                owner,
                position: vec2(update.x as f32, update.y as f32),
                dropped: update.state == ObjectiveStateUpdate::STATE_ON_MAP,
                turf: false,
                carried_by: update.owner_id,
            });
        self.reconcile_authoritative_flag_counts();
    }

    fn reconcile_authoritative_flag_counts(&mut self) {
        let mut counts = BTreeMap::<u16, u16>::new();
        for carrier in self.flags.values().filter_map(|flag| flag.carried_by) {
            let count = counts.entry(carrier).or_default();
            *count = count.saturating_add(1);
        }
        for (&player_id, player) in &mut self.players {
            player.flags = counts.get(&player_id).copied().unwrap_or(0);
        }
    }

    fn apply_authoritative_ball_state(&mut self, update: ObjectiveStateUpdate) {
        let Ok(ball_id) = u8::try_from(update.objective_id) else {
            return;
        };
        if update.state == ObjectiveStateUpdate::STATE_WAITING {
            self.powerballs.remove(&ball_id);
            return;
        }
        let carried = update.state == ObjectiveStateUpdate::STATE_CARRIED;
        let age_since_update = self
            .powerballs
            .get(&ball_id)
            .filter(|existing| {
                !existing.carried
                    && !carried
                    && existing.pickup_unlock_tick == update.pickup_unlock_tick
                    && existing.killer_id == update.killer_id
                    && existing.killer_pickup_unlock_tick == update.killer_pickup_unlock_tick
            })
            .map_or(0.0, |existing| existing.age_since_update);
        self.powerballs.insert(
            ball_id,
            NetworkPowerball {
                ball_id,
                position: vec2(update.x as f32, update.y as f32),
                velocity: vec2(
                    update.velocity_x as f32 / 10.0,
                    update.velocity_y as f32 / 10.0,
                ),
                owner_id: update.owner_id,
                timestamp: update.version,
                carried,
                pickup_unlock_tick: update.pickup_unlock_tick,
                killer_id: update.killer_id,
                killer_pickup_unlock_tick: update.killer_pickup_unlock_tick,
                authoritative_unlock_provenance: true,
                age_since_update,
            },
        );
    }

    pub(super) fn apply_soccer_goal_event(&mut self, goal: &SoccerGoal) {
        self.soccer_scores.insert(goal.frequency, goal.team_points);
    }

    pub(super) fn apply_ufo_toggled_event(&mut self, packet: &ToggleUfoPacket) {
        self.ufo_available = packet.available;
    }

    pub(super) fn apply_personal_koth_timer_set_event(&mut self, packet: &SetPersonalKothTimer) {
        self.personal_koth_timer = Some(packet.timer);
        self.personal_koth_remainder = 0.0;
    }

    pub(super) fn apply_koth_game_reset_event(&mut self, packet: &KothGameReset) {
        self.koth_timer_adding = packet.adding_timer;
        self.koth_timer = Some(packet.timer);
        self.koth_player_id = Some(packet.player_id);
        self.koth_remainder = 0.0;
    }

    pub(super) fn apply_koth_time_added_event(&mut self, packet: &AddKothTime) {
        self.koth_timer = Some(
            self.koth_timer
                .unwrap_or(0)
                .saturating_add(packet.additional_time),
        );
    }

    pub(super) fn apply_speed_game_over_event(&mut self, result: &SpeedGameOver) {
        self.last_speed_game_over = Some(*result);
    }

    pub(super) fn apply_powerball_position_event(&mut self, update: &PowerballPositionUpdate) {
        if let Some(powerball) = NetworkPowerball::from_update(*update) {
            if self
                .powerballs
                .get(&update.ball_id)
                .is_none_or(|existing| existing.accepts_update(update))
            {
                self.powerballs.insert(update.ball_id, powerball);
            }
        } else {
            self.powerballs.remove(&update.ball_id);
        }
    }

    pub(crate) fn update_powerballs(&mut self, dt: f32) {
        for powerball in self.powerballs.values_mut() {
            powerball.update(dt);
        }
    }

    pub(crate) fn update_koth_timers(&mut self, dt: f32) -> bool {
        let personal_ended = tick_optional_centisecond_timer_down(
            &mut self.personal_koth_timer,
            &mut self.personal_koth_remainder,
            dt,
        );

        if self.koth_timer.is_some() {
            let elapsed = elapsed_centiseconds(dt, &mut self.koth_remainder);
            if elapsed > 0 {
                if let Some(timer) = self.koth_timer.as_mut() {
                    if self.koth_timer_adding {
                        *timer = timer.saturating_add(elapsed);
                    } else {
                        *timer = timer.saturating_sub(elapsed);
                    }
                }
            }
        } else {
            self.koth_remainder = 0.0;
        }

        personal_ended
    }
}

fn elapsed_centiseconds(dt: f32, remainder: &mut f32) -> u32 {
    if dt <= 0.0 {
        return 0;
    }

    *remainder += dt * 100.0;
    let elapsed = remainder.floor() as u32;
    *remainder -= elapsed as f32;
    elapsed
}

fn tick_optional_centisecond_timer_down(
    timer: &mut Option<u32>,
    remainder: &mut f32,
    dt: f32,
) -> bool {
    let Some(current) = timer.as_mut() else {
        *remainder = 0.0;
        return false;
    };

    let elapsed = elapsed_centiseconds(dt, remainder);
    if elapsed == 0 {
        return false;
    }
    if elapsed < *current {
        *current -= elapsed;
        return false;
    }

    *timer = None;
    *remainder = 0.0;
    true
}
