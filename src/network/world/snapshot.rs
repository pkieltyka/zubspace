use super::*;

/// How many consecutive snapshots a player can be absent from before the
/// client clears their render proxy. [REVIEW M6] One unified lifetime:
/// 12 frames at the 60 Hz near cadence ≈ 200 ms. Mid-tier players appear
/// only in every other frame, and a near→mid demotion under mid-frame loss
/// leaves the client unable to tell which tier the entity is in — a
/// per-tier lifetime keyed on the *last sighting* therefore cleared
/// demoted radar dots prematurely. A single generous lifetime covers both
/// tiers; the cost (a leaving ship's proxy persists ~100 ms longer) is
/// invisible because the near boundary is already ~one screen off-camera.
/// Covers the deliberate ~12-second stale-door scheduler freeze plus normal
/// headed-browser jitter while keeping diagnostics bounded. Overflow remains
/// explicit and fail-closed; the larger ring prevents the known release-mode
/// seam itself from manufacturing stale-artifact gaps.
const AUTHORITATIVE_ACTOR_TRACE_SNAPSHOTS: usize = 2_048;

impl NetworkWorld {
    /// Phase 3e-3: drive client render proxy lifecycle from the
    /// snapshot's entity list. Each snapshot lists the players currently in
    /// the recipient's near or mid AOI. Raw authoritative membership is
    /// retained immediately for actor-bound diagnostics; render position is
    /// cleared only after the bounded proxy lifetime below, so one lost frame
    /// or tier transition does not flicker the presentation.
    /// Roster membership (scoreboard, kill feed) is unaffected — that
    /// stays driven by reliable `PlayerEntered` / `PlayerLeft` events
    /// per spec §5.1 (global events bypass AOI).
    ///
    /// Folds the originally-planned Phase 3d "entity enter/leave"
    /// events into the snapshot diff: every snapshot self-corrects
    /// the AOI membership, so a dropped snapshot causes a momentary
    /// glitch but never persistent state loss.
    pub(super) fn apply_snapshot_received_event(&mut self, frame: &SnapshotFrame) {
        let snapshot_id = frame.header.snapshot_id;
        if !snapshot_id_advances(self.last_received_snapshot_id, snapshot_id) {
            return;
        }
        self.last_received_snapshot_id = Some(snapshot_id);
        self.authoritative_snapshot_id = Some(snapshot_id);
        self.authoritative_snapshot_server_tick = Some(frame.header.tick);
        self.authoritative_snapshot_stream_id = Some(frame.header.arena_stream_id);
        self.pending_snapshot_position_provenance.clear();
        self.pending_snapshot_position_provenance.extend(
            frame
                .near_entities
                .iter()
                .chain(&frame.mid_entities)
                .map(|entity| {
                    (
                        entity.entity_id,
                        (
                            snapshot_id,
                            frame.header.tick,
                            entity.sample_time,
                            entity.motion_segment,
                            entity.relocation_generation,
                        ),
                    )
                }),
        );
        // The server deliberately includes the recipient's own entity in the
        // snapshot for authoritative HUD reconciliation. Its exact life comes
        // only from the same-frame private lifecycle event. Remote life comes
        // only from that entity's wire delta. A stale roster generation is not
        // authoritative membership provenance for either case.
        let self_life_seq = frame.events.iter().find_map(|event| match event {
            SnapshotEvent::LifecycleState(lifecycle) if lifecycle.life_seq != 0 => {
                Some(lifecycle.life_seq)
            }
            _ => None,
        });
        let viewer_player_id = self.viewer_player_id;
        let authoritative_snapshot_actors: Vec<_> = frame
            .near_entities
            .iter()
            .chain(frame.mid_entities.iter())
            .map(|entity| AuthoritativeSnapshotActor {
                player_id: entity.entity_id,
                life_seq: if Some(entity.entity_id) == viewer_player_id {
                    self_life_seq
                } else {
                    entity.life_seq.filter(|life_seq| *life_seq != 0)
                },
            })
            .collect();
        self.authoritative_snapshot_actors = authoritative_snapshot_actors.clone();
        self.authoritative_snapshot_actor_serial =
            self.authoritative_snapshot_actor_serial.wrapping_add(1);
        self.authoritative_snapshot_actor_trace
            .push_back(AuthoritativeSnapshotActorMembership {
                epoch: self.snapshot_epoch.unwrap_or(0),
                serial: self.authoritative_snapshot_actor_serial,
                snapshot_id,
                actors: authoritative_snapshot_actors,
            });
        while self.authoritative_snapshot_actor_trace.len() > AUTHORITATIVE_ACTOR_TRACE_SNAPSHOTS {
            if let Some(evicted) = self.authoritative_snapshot_actor_trace.pop_front() {
                let already_published = self
                    .authoritative_snapshot_actor_metrics_cursor
                    .get()
                    .is_some_and(|(epoch, serial)| {
                        epoch == evicted.epoch
                            && serial.wrapping_sub(evicted.serial) < (1_u32 << 31)
                    });
                if !already_published {
                    self.authoritative_snapshot_actor_overflow =
                        self.authoritative_snapshot_actor_overflow.saturating_add(1);
                }
            }
        }

        // Bind presentation state to the actor generation carried in this
        // exact wire frame before its synthesized position is applied.
        // This prevents a reusable roster id (or delayed reliable roster
        // state) from aliasing per-life snap/Initial counters.
        for actor in &self.authoritative_snapshot_actors {
            if actor.life_seq.is_some_and(|life_seq| life_seq != 0)
                && let Some(player) = self.players.get_mut(&actor.player_id)
            {
                player.set_life_seq(actor.life_seq);
            }
        }

        // Maintain a current snapshot entity state map by applying
        // deltas (or replacing on full snapshots). Near-tier snapshot
        // entities synthesize `PlayerPositionUpdate` immediately after this
        // event is applied, so the provenance marker above can bind only that
        // exact generated render sample.
        let is_full_snapshot = frame.header.is_full();
        if is_full_snapshot {
            self.snapshot_entity_state.clear();
        }

        for entity in frame.near_entities.iter().chain(frame.mid_entities.iter()) {
            self.mark_snapshot_entity(entity, snapshot_id);
        }

        // Was a remote peer offered to this recipient at all in this frame, and in which tier? Every
        // recipient-independent explanation for the permanently blind observer has been retired on evidence -
        // wire delta omission, session carry, field masks, provenance gating - so the remaining candidates are
        // all per-recipient delivery facts, and none of them were observable. Integer math only, and these
        // publish through the opt-in metrics block rather than the always-on lifecycle channel: a counter moves
        // on almost every frame, and that channel is edge-triggered by design.
        let listing_self_id = self.viewer_player_id;
        let lists_remote = |entities: &[SnapshotEntity]| {
            entities.iter().any(|entity| {
                Some(entity.entity_id) != listing_self_id
                    && self.players.contains_key(&entity.entity_id)
            })
        };
        let near_remote = lists_remote(&frame.near_entities);
        let mid_remote = lists_remote(&frame.mid_entities);
        if near_remote {
            self.stats.remote_listing_near_frames =
                self.stats.remote_listing_near_frames.saturating_add(1);
            self.stats.remote_last_listing_tier = 1;
        } else if mid_remote {
            self.stats.remote_listing_mid_frames =
                self.stats.remote_listing_mid_frames.saturating_add(1);
            self.stats.remote_last_listing_tier = 2;
        } else if self
            .players
            .keys()
            .any(|player_id| Some(*player_id) != listing_self_id)
        {
            self.stats.remote_listing_absent_frames =
                self.stats.remote_listing_absent_frames.saturating_add(1);
            self.stats.remote_last_listing_tier = 3;
        }

        // Sweep proxies that have aged out of the recipient's AOI.
        // Self is exempt (the local player never appears in their
        // own snapshot — that's by design, the local sim drives
        // their position). Attached players ride their host's
        // position via the existing turret-link logic, so their
        // position is already None by other rules.
        let self_id = self.viewer_player_id;
        for (&player_id, player) in self.players.iter_mut() {
            if Some(player_id) == self_id {
                continue;
            }
            if player.attach_parent != INVALID_PLAYER_ID {
                continue;
            }
            if player.position.is_none() {
                continue;
            }
            let stale = match player.last_seen_snapshot_id {
                None => true,
                Some(seen) => snapshot_id.wrapping_sub(seen) > AOI_PROXY_LIFETIME_SNAPSHOTS,
            };
            if stale {
                player.clear_position();
                player.velocity = Vec2::ZERO;
            }
        }
    }

    /// Mark a snapshot entity as seen this frame: refresh the AOI sweep
    /// liveness on the matching player and patch the delta-tracking
    /// entity-state map (missing fields keep the base value, so an
    /// unchanged-position delta entry retains its position).
    fn mark_snapshot_entity(&mut self, entity: &SnapshotEntity, snapshot_id: u16) {
        if let Some(player) = self.players.get_mut(&entity.entity_id) {
            player.last_seen_snapshot_id = Some(snapshot_id);
        }
        let current = self
            .snapshot_entity_state
            .get(&entity.entity_id)
            .copied()
            .unwrap_or((0, 0));
        let next_x = entity.x.unwrap_or(current.0);
        let next_y = entity.y.unwrap_or(current.1);
        self.snapshot_entity_state
            .insert(entity.entity_id, (next_x, next_y));
    }
}

fn snapshot_id_advances(previous: Option<u16>, candidate: u16) -> bool {
    previous.map_or(true, |last| snapshot_id_is_forward(candidate, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster_entry(player_id: u16, name: &str) -> PlayerRosterEntry {
        PlayerRosterEntry {
            ship: 0,
            audio: 0,
            name: name.to_string(),
            squad: String::new(),
            kill_points: 0,
            flag_points: 0,
            player_id,
            frequency: 0,
            wins: 0,
            losses: 0,
            attach_parent: INVALID_PLAYER_ID,
            flags: 0,
            koth: 0,
        }
    }

    fn snapshot_with(
        snapshot_id: u16,
        near: &[(u16, u16, u16)],
        mid: &[(u16, u16, u16)],
    ) -> SnapshotFrame {
        SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_FULL,
                snapshot_id,
                base_id: 0,
                tick: 0,
                arena_stream_id: 0,
                near_count: near.len() as u8,
                mid_count: mid.len() as u8,
                event_count: 0,
            },
            near_entities: near
                .iter()
                .map(|&(id, x, y)| SnapshotEntity::full(id, x, y))
                .collect(),
            mid_entities: mid
                .iter()
                .map(|&(id, x, y)| SnapshotEntity::full(id, x, y))
                .collect(),
            events: vec![],
        }
    }

    fn world_with_self_and_others(self_id: u16, others: &[u16]) -> NetworkWorld {
        let mut world = NetworkWorld {
            viewer_player_id: Some(self_id),
            snapshot_epoch: Some(1),
            ..NetworkWorld::default()
        };
        world.apply_player_entered_event(&roster_entry(self_id, "self"));
        for (i, &id) in others.iter().enumerate() {
            world.apply_player_entered_event(&roster_entry(id, &format!("p{i}")));
            // Seed a known position so we can observe `clear_position()`.
            if let Some(player) = world.players.get_mut(&id) {
                player.position = Some(vec2(100.0, 100.0));
                player.render_position = Some(vec2(100.0, 100.0));
            }
        }
        world
    }

    #[test]
    fn repelled_snapshot_motion_boundary_does_not_manufacture_a_warp_flash() {
        let mut world = world_with_self_and_others(2, &[768]);
        world.players.get_mut(&768).unwrap().position = None;
        world.players.get_mut(&768).unwrap().render_position = None;
        // Exact before/after source geometry from the retained c4460ea
        // browser failure. The server's tick-1911 Repel changes velocity,
        // not position; snapshot 666 is the first observed new segment.
        for (id, tick, x, y, vx, vy, segment) in [
            (664, 1910, 8184, 8204, -364, -458, 1),
            (666, 1913, 8171, 8208, -4501, 1150, 2),
        ] {
            let mut frame = snapshot_with(id, &[(768, x, y)], &[]);
            frame.header.tick = tick;
            frame.header.arena_stream_id = 7;
            frame.near_entities[0].life_seq = Some(1);
            frame.near_entities[0].sample_time = Some(
                crate::combat_timeline::PresentationSampleTime::from_tick(tick),
            );
            frame.near_entities[0].motion_segment = Some(segment);
            frame.near_entities[0].relocation_generation = Some(1);
            world.apply_snapshot_received_event(&frame);
            world.apply_snapshot_player_position_event(
                &PlayerPositionUpdate {
                    player_id: 768,
                    timestamp: tick as u16,
                    x,
                    y,
                    direction: 0,
                    velocity_x: vx,
                    velocity_y: vy,
                    togglables: 0,
                    ping: 0,
                    bounty: 30,
                    weapon: 0,
                    checksum: None,
                    energy: None,
                    s2c_latency: None,
                    flag_timer: None,
                    items: None,
                },
                id,
                tick,
                7,
            );
        }
        assert!(
            world.pending_warp_effects.is_empty(),
            "a position-continuous Repel is not a warp"
        );
        assert_eq!(world.players[&768].presentation_snap_count, 0);
        assert_eq!(world.players[&768].snapshot_motion_segment, Some(2));
    }

    #[test]
    fn snapshot_motion_segment_snaps_near_and_far_relocations_once_despite_loss() {
        for displacement in [30, 2_000] {
            let mut world = world_with_self_and_others(2, &[768]);
            let apply = |world: &mut NetworkWorld, id, tick, x, life, (segment, relocation)| {
                let mut frame = snapshot_with(id, &[(768, x, 100)], &[]);
                frame.header.tick = tick;
                frame.header.arena_stream_id = 7;
                frame.near_entities[0].life_seq = Some(life);
                frame.near_entities[0].sample_time = Some(
                    crate::combat_timeline::PresentationSampleTime::from_tick(tick),
                );
                frame.near_entities[0].motion_segment = Some(segment);
                frame.near_entities[0].relocation_generation = Some(relocation);
                world.apply_snapshot_received_event(&frame);
                world.apply_snapshot_player_position_event(
                    &PlayerPositionUpdate {
                        player_id: 768,
                        timestamp: tick as u16,
                        x,
                        y: 100,
                        direction: 0,
                        velocity_x: 0,
                        velocity_y: 0,
                        togglables: 0,
                        ping: 0,
                        bounty: 30,
                        weapon: 0,
                        checksum: None,
                        energy: None,
                        s2c_latency: None,
                        flag_timer: None,
                        items: None,
                    },
                    id,
                    tick,
                    7,
                );
            };
            apply(&mut world, 1, 100, 100, 7, (1, 1));
            world
                .players
                .get_mut(&768)
                .unwrap()
                .update_directed_render_position(100, 0.01);
            assert_eq!(
                world.players[&768].render_position,
                Some(vec2(100.0, 100.0))
            );
            // Relocation segment 2 is lost, then Repel creates segment 3.
            // Its retained relocation generation 2 must still force the
            // original near/far relocation, despite the latest cause being
            // a velocity-only impulse.
            apply(&mut world, 3, 104, 100 + displacement, 7, (3, 2));
            assert_eq!(world.players[&768].presentation_snap_count, 1);
            assert_eq!(world.pending_warp_effects.len(), 1);
            world
                .players
                .get_mut(&768)
                .unwrap()
                .update_directed_render_position(102, 0.01);
            assert_eq!(
                world.players[&768].render_position,
                Some(vec2(100.0, 100.0))
            );
            world
                .players
                .get_mut(&768)
                .unwrap()
                .update_directed_render_position(104, 0.01);
            assert_eq!(
                world.players[&768].render_position,
                Some(vec2(f32::from(100 + displacement), 100.0))
            );
            apply(&mut world, 4, 106, 104 + displacement, 7, (4, 2));
            apply(&mut world, 5, 108, 108 + displacement, 7, (4, 2));
            apply(&mut world, 3, 104, 100 + displacement, 7, (3, 2));
            assert_eq!(world.players[&768].presentation_snap_count, 1);
            assert_eq!(
                world.pending_warp_effects.len(),
                1,
                "no repeated Flash effects"
            );
            world
                .players
                .get_mut(&768)
                .unwrap()
                .update_directed_render_position(107, 0.01);
            let displayed = world.players[&768].render_position.unwrap().x;
            assert!(
                displayed >= f32::from(104 + displacement)
                    && displayed <= f32::from(108 + displacement)
            );
            world.pending_warp_effects.clear();
            apply(&mut world, 6, 110, 108 + displacement, 8, (1, 1));
            assert_eq!(
                world.players[&768].presentation_snap_count, 0,
                "a new life does not inherit a prior motion generation"
            );
            assert!(world.pending_warp_effects.is_empty());
        }
    }

    #[test]
    fn fresh_snapshot_recovers_actor_after_long_aoi_absence() {
        for first_tick in [1000, 0x7fff_fff0] {
            for gap in [32_767, 32_768, 36_000, 65_536] {
                for mid in [false, true] {
                    for life in [1, 2] {
                        assert_long_aoi_reentry(first_tick, gap, mid, life);
                    }
                }
            }
        }
    }

    fn assert_long_aoi_reentry(first_tick: u32, gap: u32, mid: bool, life: u32) {
        let return_tick = crate::continuum::continuum_tick_add(first_tick, gap);
        let mut world = world_with_self_and_others(2, &[768]);
        let mut position = PlayerPositionUpdate {
            player_id: 768,
            direction: 0,
            timestamp: first_tick as u16,
            x: 7884,
            y: 8433,
            velocity_x: 0,
            velocity_y: 0,
            togglables: 0,
            ping: 0,
            bounty: 0,
            weapon: 0,
            checksum: None,
            energy: None,
            s2c_latency: None,
            flag_timer: None,
            items: None,
        };
        let mut first = snapshot_with(1, &[(768, position.x, position.y)], &[]);
        first.header.tick = first_tick;
        first.header.arena_stream_id = 7;
        first.near_entities[0].life_seq = Some(1);
        first.near_entities[0].sample_time = Some(
            crate::combat_timeline::PresentationSampleTime::from_tick(first_tick),
        );
        world.apply_snapshot_received_event(&first);
        world.apply_snapshot_player_position_event(&position, 1, first_tick, 7);
        let old_position = position.clone();
        for id in 2..=AOI_PROXY_LIFETIME_SNAPSHOTS + 2 {
            world.apply_snapshot_received_event(&snapshot_with(id, &[], &[]));
        }
        assert!(world.players[&768].position.is_none());
        position.timestamp = return_tick as u16;
        position.x = 8065;
        position.y = 8640;
        let mut returned = snapshot_with(100, &[(768, position.x, position.y)], &[]);
        returned.header.tick = return_tick;
        returned.header.arena_stream_id = 7;
        returned.near_entities[0].life_seq = Some(life);
        returned.near_entities[0].sample_time = Some(
            crate::combat_timeline::PresentationSampleTime::from_tick(return_tick),
        );
        if mid {
            returned.mid_entities = std::mem::take(&mut returned.near_entities);
            returned.mid_entities[0].sample_time = None;
            returned.header.near_count = 0;
            returned.header.mid_count = 1;
        }
        world.apply_snapshot_received_event(&returned);
        for (snapshot_id, tick, stream) in [
            (99, return_tick, 7),
            (
                100,
                crate::continuum::continuum_tick_add(return_tick, 65_536),
                7,
            ),
            (100, return_tick, 8),
        ] {
            world.apply_snapshot_player_position_event(&position, snapshot_id, tick, stream);
            assert!(
                world.players[&768].position.is_none(),
                "mismatched snapshot is inert"
            );
        }
        world.apply_snapshot_player_position_event(&position, 100, return_tick, 7);
        assert_eq!(world.players[&768].life_seq, Some(life));
        assert_eq!(world.players[&768].last_seen_snapshot_id, Some(100));
        assert_eq!(
            world.players[&768].position,
            Some(vec2(8065.0, 8640.0)),
            "a fresh snapshot must restore the actor, not just its membership"
        );
        // A legacy tick can alias forward after the long gap, but cannot
        // overwrite snapshot-owned geometry. The same snapshot is once-only.
        world.apply_player_position_event(&old_position);
        let duplicate = PlayerPositionUpdate {
            x: 999,
            ..position.clone()
        };
        world.apply_snapshot_player_position_event(&duplicate, 100, return_tick, 7);
        assert_eq!(world.players[&768].position, Some(vec2(8065.0, 8640.0)));
    }

    #[test]
    fn bootstrap_and_snapshot_positions_share_clock_without_weakening_stale_admission() {
        for join_tick in [53_808u32, 65_535, 65_536, 131_073] {
            for bootstrap_first in [true, false] {
                let mut world = world_with_self_and_others(2, &[768]);
                let bootstrap = PlayerPositionUpdate {
                    player_id: 768,
                    direction: 0,
                    timestamp: join_tick as u16,
                    x: 7884,
                    y: 8433,
                    velocity_x: 0,
                    velocity_y: 0,
                    togglables: 0,
                    ping: 0,
                    bounty: 0,
                    weapon: 0,
                    checksum: None,
                    energy: None,
                    s2c_latency: None,
                    flag_timer: None,
                    items: None,
                };
                let next = PlayerPositionUpdate {
                    timestamp: join_tick.wrapping_add(1) as u16,
                    x: 8065,
                    y: 8640,
                    ..bootstrap.clone()
                };
                if bootstrap_first {
                    world.apply_player_position_event(&bootstrap);
                }
                let mut frame = snapshot_with(207, &[(768, next.x, next.y)], &[]);
                frame.header.tick = join_tick + 1;
                world.apply_snapshot_received_event(&frame);
                world.apply_snapshot_player_position_event(&next, 207, join_tick + 1, 0);
                // A late initial projection or duplicate must not restore the
                // obsolete sprite anchor, including across 16-bit clock wrap.
                world.apply_player_position_event(&bootstrap);
                world.apply_player_position_event(&next);
                let remote = &world.players[&768];
                assert_eq!(remote.position, Some(vec2(8065.0, 8640.0)));
                assert_eq!(remote.last_server_position_tick, Some(next.timestamp));
                assert_eq!(remote.last_seen_snapshot_id, Some(207));
                assert_eq!(world.stats.remote_position_stale_drops, 2);
            }
        }
    }

    #[test]
    fn snapshot_marks_entities_as_seen_at_current_snapshot_id() {
        let mut world = world_with_self_and_others(0, &[1, 2]);
        let frame = snapshot_with(5, &[(1, 100, 100)], &[(2, 5_000, 5_000)]);
        world.apply_snapshot_received_event(&frame);
        assert_eq!(world.last_received_snapshot_id, Some(5));
        assert_eq!(world.players[&1].last_seen_snapshot_id, Some(5));
        assert_eq!(world.players[&2].last_seen_snapshot_id, Some(5));
    }

    #[test]
    fn game_applied_presentability_is_distinct_from_snapshot_membership() {
        let mut world = world_with_self_and_others(0, &[1]);
        let mut unresolved = snapshot_with(5, &[(1, 100, 100)], &[]);
        unresolved.header.tick = 700;
        world.apply_snapshot_received_event(&unresolved);
        assert_eq!(
            world.snapshot_presentability(),
            SnapshotPresentabilitySummary {
                snapshot_id: Some(5),
                server_tick: Some(700),
                remote_actor_count: 1,
                presentable_remote_actor_count: 0,
                unresolved_life_count: 1,
                missing_position_count: 0,
            }
        );

        let mut applied = snapshot_with(6, &[(1, 101, 100)], &[]);
        applied.header.tick = 701;
        applied.near_entities[0].field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
        applied.near_entities[0].life_seq = Some(3);
        world.apply_snapshot_received_event(&applied);
        let summary = world.snapshot_presentability();
        assert!(summary.snapshot_is_presentable());
        assert_eq!(summary.snapshot_id, Some(6));
        assert_eq!(summary.server_tick, Some(701));
        assert_eq!(summary.presentable_remote_actor_count, 1);
    }

    #[test]
    fn directed_history_consumes_only_exact_generated_snapshot_positions() {
        let mut world = world_with_self_and_others(0, &[1]);
        let position = |timestamp, x| PlayerPositionUpdate {
            player_id: 1,
            direction: 0,
            timestamp,
            x,
            y: 100,
            velocity_x: 0,
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

        let mut first = snapshot_with(5, &[(1, 100, 100)], &[]);
        first.header.tick = 700;
        first.near_entities[0].sample_time = Some(
            crate::combat_timeline::PresentationSampleTime::from_tick(700),
        );
        first.near_entities[0].field_mask |= SnapshotEntity::FIELD_SAMPLE_TIME;
        first.near_entities[0].field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
        first.near_entities[0].life_seq = Some(3);
        world.apply_snapshot_received_event(&first);
        world.apply_snapshot_player_position_event(&position(700, 100), 5, 700, 0);
        world.update_remote_players(0.0, Some(700));
        assert_eq!(
            world.players[&1].display_position(),
            Some(vec2(100.0, 100.0))
        );

        // Legacy datagrams cannot compete with snapshot-owned position or
        // enter directed presentation history, even when their tick is newer.
        world.apply_player_position_event(&position(701, 200));
        world.update_remote_players(0.0, Some(701));
        assert_eq!(world.players[&1].position, Some(vec2(100.0, 100.0)));
        assert_eq!(
            world.players[&1].display_position(),
            Some(vec2(100.0, 100.0))
        );

        let mut second = snapshot_with(6, &[(1, 200, 100)], &[]);
        second.header.tick = 710;
        second.near_entities[0].sample_time = Some(
            crate::combat_timeline::PresentationSampleTime::from_tick(710),
        );
        second.near_entities[0].field_mask |= SnapshotEntity::FIELD_SAMPLE_TIME;
        second.near_entities[0].field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
        second.near_entities[0].life_seq = Some(3);
        world.apply_snapshot_received_event(&second);
        world.apply_snapshot_player_position_event(&position(710, 200), 6, 710, 0);
        world.update_remote_players(0.0, Some(705));
        assert_eq!(
            world.players[&1].display_position(),
            Some(vec2(150.0, 100.0))
        );
    }

    #[test]
    fn authoritative_membership_uses_same_frame_private_lifecycle_for_self() {
        let mut world = world_with_self_and_others(7, &[8]);
        assert_eq!(world.players[&7].life_seq, None);
        world.players.get_mut(&8).unwrap().set_life_seq(Some(3));
        let mut frame = snapshot_with(5, &[(7, 90, 90), (8, 100, 100)], &[]);
        frame.near_entities[0].field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
        frame.near_entities[0].life_seq = Some(99);
        frame.near_entities[1].field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
        frame.near_entities[1].life_seq = Some(3);
        frame
            .events
            .push(SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                life_seq: 42,
                alive: true,
                respawn_at_tick: None,
                repel_until_tick: None,
                repel_delivery: None,
            }));

        world.apply_snapshot_received_event(&frame);

        assert_eq!(
            world.authoritative_snapshot_actors,
            vec![
                AuthoritativeSnapshotActor {
                    player_id: 7,
                    life_seq: Some(42),
                },
                AuthoritativeSnapshotActor {
                    player_id: 8,
                    life_seq: Some(3),
                },
            ]
        );
        assert_eq!(world.players[&7].life_seq, Some(42));
    }

    #[test]
    fn authoritative_membership_never_falls_back_to_stale_remote_roster_life() {
        let mut world = world_with_self_and_others(0, &[1]);
        world.players.get_mut(&1).unwrap().set_life_seq(Some(77));
        let frame = snapshot_with(5, &[(1, 100, 100)], &[]);

        world.apply_snapshot_received_event(&frame);

        assert_eq!(
            world.authoritative_snapshot_actors,
            vec![AuthoritativeSnapshotActor {
                player_id: 1,
                life_seq: None,
            }],
            "missing same-frame remote life stays explicitly unresolved",
        );
        assert_eq!(world.players[&1].life_seq, Some(77));
    }

    #[test]
    fn snapshot_ignores_stale_ids_for_aoi_lifecycle() {
        let mut world = world_with_self_and_others(0, &[1]);
        let fresh = snapshot_with(10, &[(1, 100, 100)], &[]);
        world.apply_snapshot_received_event(&fresh);
        assert_eq!(world.last_received_snapshot_id, Some(10));
        assert!(world.players[&1].position.is_some());

        let stale_empty = snapshot_with(9, &[], &[]);
        world.apply_snapshot_received_event(&stale_empty);
        assert_eq!(
            world.last_received_snapshot_id,
            Some(10),
            "stale snapshot must not move the AOI cursor backwards"
        );
        assert_eq!(world.players[&1].last_seen_snapshot_id, Some(10));
        assert!(
            world.players[&1].position.is_some(),
            "stale empty snapshot must not clear a visible remote proxy"
        );
    }

    #[test]
    fn snapshot_clears_position_for_entities_never_seen_in_snapshot() {
        // Player 2 was placed in the world (e.g. via PlayerEntered +
        // a stale LARGE_POSITION) but is not in the first snapshot.
        // Their position should be cleared so the render layer stops
        // drawing the proxy.
        let mut world = world_with_self_and_others(0, &[1, 2]);
        let frame = snapshot_with(5, &[(1, 100, 100)], &[]);
        world.apply_snapshot_received_event(&frame);
        assert!(
            world.players[&1].position.is_some(),
            "player 1 was in snapshot, keeps position"
        );
        assert!(
            world.players[&2].position.is_none(),
            "player 2 absent from first snapshot is cleared"
        );
    }

    #[test]
    fn proxy_survives_to_unified_lifetime_then_clears() {
        // [REVIEW M6] One unified lifetime for both tiers: a sighting (near
        // here) survives absence through the configured lifetime, then clears
        // on the following snapshot.
        let mut world = world_with_self_and_others(0, &[1]);

        let seen = snapshot_with(10, &[(1, 100, 100)], &[]);
        world.apply_snapshot_received_event(&seen);
        assert!(world.players[&1].position.is_some());

        for id in 11..=10 + AOI_PROXY_LIFETIME_SNAPSHOTS {
            world.apply_snapshot_received_event(&snapshot_with(id, &[], &[]));
            assert!(
                world.players[&1].position.is_some(),
                "proxy kept through delta {} (<= lifetime)",
                id - 10
            );
        }
        let expired_id = 10 + AOI_PROXY_LIFETIME_SNAPSHOTS + 1;
        world.apply_snapshot_received_event(&snapshot_with(expired_id, &[], &[]));
        assert!(
            world.players[&1].position.is_none(),
            "proxy cleared once delta exceeds the unified lifetime"
        );
    }

    #[test]
    fn mid_sighting_refreshes_proxy_lifetime() {
        // A demoted (near→mid) player's radar dot must be kept alive by
        // mid sightings alone, with the same unified lifetime.
        let mut world = world_with_self_and_others(0, &[1]);

        world.apply_snapshot_received_event(&snapshot_with(10, &[(1, 100, 100)], &[]));
        // Demoted: next sighting arrives in the mid list.
        world.apply_snapshot_received_event(&snapshot_with(14, &[], &[(1, 5_000, 5_000)]));
        assert!(world.players[&1].position.is_some());

        // Absence is measured from the mid sighting (id 14), not the old
        // near sighting: kept through the named lifetime, then cleared.
        for id in 15..=14 + AOI_PROXY_LIFETIME_SNAPSHOTS {
            world.apply_snapshot_received_event(&snapshot_with(id, &[], &[]));
            assert!(
                world.players[&1].position.is_some(),
                "mid-refreshed proxy kept through delta {} (<= lifetime)",
                id - 14
            );
        }
        let expired_id = 14 + AOI_PROXY_LIFETIME_SNAPSHOTS + 1;
        world.apply_snapshot_received_event(&snapshot_with(expired_id, &[], &[]));
        assert!(
            world.players[&1].position.is_none(),
            "mid-refreshed proxy cleared once delta exceeds the lifetime"
        );
    }

    #[test]
    fn snapshot_recovers_position_when_entity_reappears() {
        let mut world = world_with_self_and_others(0, &[1]);

        // Entity 1 starts in AOI.
        let f1 = snapshot_with(10, &[(1, 100, 100)], &[]);
        world.apply_snapshot_received_event(&f1);

        // Entity 1 disappears beyond the named unified lifetime — cleared.
        let expired_id = 10 + AOI_PROXY_LIFETIME_SNAPSHOTS + 1;
        let f2 = snapshot_with(expired_id, &[], &[]);
        world.apply_snapshot_received_event(&f2);
        assert!(world.players[&1].position.is_none());

        // Manually re-set the position (simulating a fresh
        // LARGE_POSITION arriving alongside the next snapshot).
        if let Some(player) = world.players.get_mut(&1) {
            player.position = Some(vec2(200.0, 200.0));
        }

        // The following snapshot places them back in near AOI — keep the
        // position since they're seen.
        let reentered_id = expired_id + 1;
        let f3 = snapshot_with(reentered_id, &[(1, 200, 200)], &[]);
        world.apply_snapshot_received_event(&f3);
        assert_eq!(world.players[&1].last_seen_snapshot_id, Some(reentered_id));
        assert!(world.players[&1].position.is_some());
    }

    #[test]
    fn snapshot_does_not_clear_self_player() {
        let mut world = world_with_self_and_others(0, &[]);
        // Local player has a position from their own simulation.
        if let Some(player) = world.players.get_mut(&0) {
            player.position = Some(vec2(500.0, 500.0));
        }
        // Empty snapshot — local player is never in their own snapshot.
        let frame = snapshot_with(5, &[], &[]);
        world.apply_snapshot_received_event(&frame);
        assert!(
            world.players[&0].position.is_some(),
            "self player should never be cleared by AOI sweep"
        );
    }

    #[test]
    fn snapshot_does_not_clear_attached_players() {
        // Attached players have their position driven by the host
        // turret-link logic; the AOI sweep should leave them alone.
        let mut world = world_with_self_and_others(0, &[1]);
        if let Some(player) = world.players.get_mut(&1) {
            player.attach_parent = 0;
            // Attached players typically have position == None set by
            // turret-link logic, but we test the carve-out
            // explicitly: even if they had a stale position, we do
            // not touch it via AOI sweep.
            player.position = Some(vec2(300.0, 300.0));
        }
        let frame = snapshot_with(5, &[], &[]);
        world.apply_snapshot_received_event(&frame);
        assert!(
            world.players[&1].position.is_some(),
            "attached player should be exempt from AOI sweep"
        );
    }

    #[test]
    fn full_snapshot_resets_entity_state_map() {
        let mut world = world_with_self_and_others(0, &[1, 2]);
        // Seed a stale entry so we can confirm the reset.
        world.snapshot_entity_state.insert(99, (1234, 5678));

        let frame = snapshot_with(1, &[(1, 100, 200)], &[(2, 300, 400)]);
        world.apply_snapshot_received_event(&frame);

        // Stale entry cleared by full reset.
        assert!(!world.snapshot_entity_state.contains_key(&99));
        // New entries set from the snapshot.
        assert_eq!(world.snapshot_entity_state.get(&1), Some(&(100, 200)));
        assert_eq!(world.snapshot_entity_state.get(&2), Some(&(300, 400)));
    }

    fn delta_frame_unchanged(snapshot_id: u16, base_id: u16, ids: &[u16]) -> SnapshotFrame {
        SnapshotFrame {
            header: SnapshotHeader {
                flags: 0,
                snapshot_id,
                base_id,
                tick: 0,
                arena_stream_id: 0,
                near_count: ids.len() as u8,
                mid_count: 0,
                event_count: 0,
            },
            near_entities: ids
                .iter()
                .map(|&id| SnapshotEntity::unchanged(id))
                .collect(),
            mid_entities: vec![],
            events: vec![],
        }
    }

    fn delta_frame_with_position(
        snapshot_id: u16,
        base_id: u16,
        entity_id: u16,
        x: u16,
        y: u16,
    ) -> SnapshotFrame {
        SnapshotFrame {
            header: SnapshotHeader {
                flags: 0,
                snapshot_id,
                base_id,
                tick: 0,
                arena_stream_id: 0,
                near_count: 1,
                mid_count: 0,
                event_count: 0,
            },
            near_entities: vec![SnapshotEntity::full(entity_id, x, y)],
            mid_entities: vec![],
            events: vec![],
        }
    }

    #[test]
    fn delta_snapshot_keeps_unchanged_entity_position() {
        let mut world = world_with_self_and_others(0, &[1]);
        let initial = snapshot_with(10, &[(1, 500, 500)], &[]);
        world.apply_snapshot_received_event(&initial);
        assert_eq!(world.snapshot_entity_state.get(&1), Some(&(500, 500)));

        // Delta against snapshot 10 with entity 1 unchanged.
        let delta = delta_frame_unchanged(11, 10, &[1]);
        world.apply_snapshot_received_event(&delta);
        assert_eq!(
            world.snapshot_entity_state.get(&1),
            Some(&(500, 500)),
            "unchanged entity should keep position from base"
        );
    }

    #[test]
    fn delta_snapshot_updates_changed_position_fields() {
        let mut world = world_with_self_and_others(0, &[1]);
        let initial = snapshot_with(10, &[(1, 500, 500)], &[]);
        world.apply_snapshot_received_event(&initial);

        // Delta with both fields changed.
        let delta = delta_frame_with_position(11, 10, 1, 700, 800);
        world.apply_snapshot_received_event(&delta);
        assert_eq!(world.snapshot_entity_state.get(&1), Some(&(700, 800)));
    }

    #[test]
    fn delta_snapshot_with_only_x_set_keeps_y() {
        let mut world = world_with_self_and_others(0, &[1]);
        let initial = snapshot_with(10, &[(1, 500, 500)], &[]);
        world.apply_snapshot_received_event(&initial);

        // Hand-craft a delta where only x is present.
        let delta = SnapshotFrame {
            header: SnapshotHeader {
                flags: 0,
                snapshot_id: 11,
                base_id: 10,
                tick: 0,
                arena_stream_id: 0,
                near_count: 1,
                mid_count: 0,
                event_count: 0,
            },
            near_entities: vec![SnapshotEntity {
                entity_id: 1,
                field_mask: SnapshotEntity::FIELD_X,
                x: Some(900),
                ..SnapshotEntity::default()
            }],
            mid_entities: vec![],
            events: vec![],
        };
        world.apply_snapshot_received_event(&delta);
        assert_eq!(
            world.snapshot_entity_state.get(&1),
            Some(&(900, 500)),
            "only-x delta should leave y from base"
        );
    }

    #[test]
    fn snapshot_handles_id_wraparound() {
        // last_seen near u16::MAX, current near 0 — wrapping_sub
        // gives a small forward delta. Should NOT be treated as stale.
        let mut world = world_with_self_and_others(0, &[1]);
        if let Some(player) = world.players.get_mut(&1) {
            player.last_seen_snapshot_id = Some(u16::MAX - 1);
            player.position = Some(vec2(100.0, 100.0));
        }
        // Current snapshot id has wrapped to 1. Delta = 1 - (u16::MAX - 1)
        // wrapping = 3. That's at the boundary — should NOT clear
        // (strict greater-than threshold).
        let frame = snapshot_with(1, &[], &[]);
        world.apply_snapshot_received_event(&frame);
        assert!(
            world.players[&1].position.is_some(),
            "delta == threshold across wrap should not clear"
        );
    }

    #[test]
    fn authoritative_membership_is_immediate_raw_actor_and_life_evidence() {
        let mut world = world_with_self_and_others(0, &[1, 2, 3]);
        for (player_id, life_seq) in [(1, 11), (2, 22), (3, 33)] {
            world
                .players
                .get_mut(&player_id)
                .unwrap()
                .set_life_seq(Some(life_seq));
        }

        let mut first = snapshot_with(100, &[(1, 100, 100), (2, 200, 200)], &[(3, 3_000, 3_000)]);
        for (entity, life_seq) in first
            .near_entities
            .iter_mut()
            .chain(first.mid_entities.iter_mut())
            .zip([11, 22, 33])
        {
            entity.field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
            entity.life_seq = Some(life_seq);
        }
        world.apply_snapshot_received_event(&first);
        assert_eq!(
            world.authoritative_snapshot_actors,
            vec![
                AuthoritativeSnapshotActor {
                    player_id: 1,
                    life_seq: Some(11),
                },
                AuthoritativeSnapshotActor {
                    player_id: 2,
                    life_seq: Some(22),
                },
                AuthoritativeSnapshotActor {
                    player_id: 3,
                    life_seq: Some(33),
                },
            ],
            "raw near+mid wire order and exact lifecycle identity are retained",
        );

        let mut second = snapshot_with(101, &[(2, 210, 210)], &[]);
        second.near_entities[0].field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
        second.near_entities[0].life_seq = Some(22);
        world.apply_snapshot_received_event(&second);
        assert_eq!(
            world.authoritative_snapshot_actors,
            vec![AuthoritativeSnapshotActor {
                player_id: 2,
                life_seq: Some(22),
            }],
            "wire omission is visible on the first forward applied snapshot",
        );
        assert!(
            world.players[&1].position.is_some(),
            "render proxy TTL is deliberately independent of raw wire membership",
        );

        world.apply_snapshot_received_event(&snapshot_with(100, &[], &[]));
        assert_eq!(world.authoritative_snapshot_id, Some(101));
        assert_eq!(world.authoritative_snapshot_actors[0].player_id, 2);

        world.last_received_snapshot_id = Some(u16::MAX);
        let mut wrapped = snapshot_with(1, &[(1, 120, 120)], &[]);
        wrapped.near_entities[0].field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
        wrapped.near_entities[0].life_seq = Some(11);
        world.apply_snapshot_received_event(&wrapped);
        assert_eq!(world.authoritative_snapshot_id, Some(1));
        assert_eq!(world.authoritative_snapshot_actors[0].life_seq, Some(11));
    }

    #[test]
    fn authoritative_remote_membership_prefers_same_frame_wire_life() {
        let mut world = world_with_self_and_others(0, &[1]);
        world.players.get_mut(&1).unwrap().set_life_seq(Some(11));
        let mut frame = snapshot_with(100, &[(1, 100, 100)], &[]);
        frame.near_entities[0].field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
        frame.near_entities[0].life_seq = Some(99);

        world.apply_snapshot_received_event(&frame);

        assert_eq!(
            world.authoritative_snapshot_actors,
            vec![AuthoritativeSnapshotActor {
                player_id: 1,
                life_seq: Some(99),
            }],
            "same-frame wire membership cannot be aliased by stale reliable life state",
        );
        assert_eq!(
            world.players[&1].life_seq,
            Some(99),
            "presentation counters are rebound to the exact wire generation",
        );
    }

    #[test]
    fn authoritative_membership_trace_retains_every_forward_frame_but_not_stale() {
        let mut world = world_with_self_and_others(0, &[1, 2]);
        world.players.get_mut(&1).unwrap().set_life_seq(Some(11));
        world.players.get_mut(&2).unwrap().set_life_seq(Some(22));

        let mut first = snapshot_with(100, &[(1, 100, 100)], &[]);
        first.near_entities[0].field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
        first.near_entities[0].life_seq = Some(11);
        world.apply_snapshot_received_event(&first);
        world.apply_snapshot_received_event(&snapshot_with(101, &[], &[]));
        let mut third = snapshot_with(102, &[(2, 200, 200)], &[]);
        third.near_entities[0].field_mask |= SnapshotEntity::FIELD_LIFE_SEQ;
        third.near_entities[0].life_seq = Some(22);
        world.apply_snapshot_received_event(&third);
        world.apply_snapshot_received_event(&snapshot_with(101, &[(1, 100, 100)], &[]));

        assert_eq!(world.authoritative_snapshot_actor_trace.len(), 3);
        assert_eq!(world.authoritative_snapshot_actor_trace[0].epoch, 1);
        assert_eq!(world.authoritative_snapshot_actor_trace[0].serial, 1);
        assert_eq!(world.authoritative_snapshot_actor_trace[0].snapshot_id, 100);
        assert_eq!(
            world.authoritative_snapshot_actor_trace[0].actors[0].life_seq,
            Some(11)
        );
        assert!(
            world.authoritative_snapshot_actor_trace[1]
                .actors
                .is_empty()
        );
        assert_eq!(world.authoritative_snapshot_actor_trace[2].snapshot_id, 102);
        assert_eq!(
            world.authoritative_snapshot_actor_trace[2].actors[0].life_seq,
            Some(22)
        );

        let final_snapshot_id = AUTHORITATIVE_ACTOR_TRACE_SNAPSHOTS as u16 + 108;
        for snapshot_id in 103..=final_snapshot_id {
            world.apply_snapshot_received_event(&snapshot_with(snapshot_id, &[], &[]));
        }
        assert_eq!(
            world.authoritative_snapshot_actor_trace.len(),
            AUTHORITATIVE_ACTOR_TRACE_SNAPSHOTS,
        );
        assert_eq!(
            world
                .authoritative_snapshot_actor_trace
                .front()
                .unwrap()
                .snapshot_id,
            109
        );
        assert_eq!(
            world
                .authoritative_snapshot_actor_trace
                .back()
                .unwrap()
                .snapshot_id,
            final_snapshot_id
        );
        assert_eq!(
            world.authoritative_snapshot_actor_serial,
            AUTHORITATIVE_ACTOR_TRACE_SNAPSHOTS as u32 + 9
        );
        assert_eq!(world.authoritative_snapshot_actor_overflow, 9);
        assert_eq!(
            world
                .authoritative_snapshot_actor_trace
                .front()
                .unwrap()
                .serial,
            10
        );
        assert_eq!(
            world
                .authoritative_snapshot_actor_trace
                .back()
                .unwrap()
                .serial,
            AUTHORITATIVE_ACTOR_TRACE_SNAPSHOTS as u32 + 9
        );
    }

    #[test]
    fn authoritative_membership_stream_serial_wrap_and_epoch_reset_are_explicit() {
        let mut world = world_with_self_and_others(0, &[]);
        world.authoritative_snapshot_actor_serial = u32::MAX;
        world.apply_snapshot_received_event(&snapshot_with(1, &[], &[]));
        assert_eq!(
            world
                .authoritative_snapshot_actor_trace
                .back()
                .unwrap()
                .serial,
            0
        );
        world.apply_arena_presentation_joined_event(9);
        world.apply_snapshot_received_event(&snapshot_with(1, &[], &[]));
        let membership = world.authoritative_snapshot_actor_trace.back().unwrap();
        assert_eq!((membership.epoch, membership.serial), (9, 1));
        assert_eq!(world.authoritative_snapshot_actor_overflow, 0);
    }

    #[test]
    fn authoritative_membership_stream_does_not_count_published_evictions_as_overflow() {
        let mut world = world_with_self_and_others(0, &[]);
        world.apply_arena_presentation_joined_event(7);

        for snapshot_id in 1..=(AUTHORITATIVE_ACTOR_TRACE_SNAPSHOTS as u16 + 8) {
            world.apply_snapshot_received_event(&snapshot_with(snapshot_id, &[], &[]));
            let latest = world.authoritative_snapshot_actor_trace.back().unwrap();
            world
                .authoritative_snapshot_actor_metrics_cursor
                .set(Some((latest.epoch, latest.serial)));
        }

        assert_eq!(
            world.authoritative_snapshot_actor_trace.len(),
            AUTHORITATIVE_ACTOR_TRACE_SNAPSHOTS,
        );
        assert_eq!(world.authoritative_snapshot_actor_overflow, 0);
        assert_eq!(
            world
                .authoritative_snapshot_actor_trace
                .front()
                .unwrap()
                .serial,
            9,
        );
    }

    #[test]
    fn presentation_discontinuity_counters_are_actor_and_life_scoped() {
        let mut world = world_with_self_and_others(0, &[1, 2]);
        world.players.get_mut(&1).unwrap().set_life_seq(Some(10));
        world.players.get_mut(&2).unwrap().set_life_seq(Some(20));
        world
            .players
            .get_mut(&1)
            .unwrap()
            .record_presentation_snap(RemoteSnapReason::Distance);
        world
            .players
            .get_mut(&2)
            .unwrap()
            .record_presentation_snap(RemoteSnapReason::Initial);

        assert_eq!(world.players[&1].presentation_snap_count, 1);
        assert_eq!(world.players[&1].presentation_initial_count, 0);
        assert_eq!(world.players[&2].presentation_snap_count, 0);
        assert_eq!(world.players[&2].presentation_initial_count, 1);

        world.players.get_mut(&1).unwrap().set_life_seq(Some(11));
        assert_eq!(world.players[&1].presentation_snap_count, 0);
        assert_eq!(world.players[&1].presentation_initial_count, 0);
        assert_eq!(world.players[&2].presentation_initial_count, 1);
    }

    #[test]
    fn a_late_death_from_an_older_life_does_not_blind_the_observer_to_a_respawned_peer() {
        // Why this test exists: `visible_players()` drops any entry whose position is `None`, so an
        // observer that clears a live peer's position renders that peer invisible while it keeps
        // appearing in every later snapshot. The retained field evidence is exactly that shape - a peer
        // with a healthy snapshot count and a null position - and it only appears in runs where a client
        // died, i.e. where a death event is in flight.
        fn position_update(x: u16, y: u16, tick: u32) -> PlayerPositionUpdate {
            PlayerPositionUpdate {
                player_id: 768,
                direction: 0,
                timestamp: tick as u16,
                x,
                y,
                velocity_x: 0,
                velocity_y: 0,
                togglables: 0,
                ping: 0,
                bounty: 0,
                weapon: 0,
                checksum: None,
                energy: None,
                s2c_latency: None,
                flag_timer: None,
                items: None,
            }
        }

        let mut world = world_with_self_and_others(2, &[768]);

        // First life: the peer is visible at its authored position.
        let first = position_update(7884, 8433, 1_000);
        let mut first_frame = snapshot_with(1, &[(768, 7884, 8433)], &[]);
        first_frame.header.tick = 1_000;
        first_frame.header.arena_stream_id = 7;
        first_frame.near_entities[0].life_seq = Some(1);
        first_frame.near_entities[0].sample_time = Some(
            crate::combat_timeline::PresentationSampleTime::from_tick(1_000),
        );
        world.apply_snapshot_received_event(&first_frame);
        world.apply_snapshot_player_position_event(&first, 1, 1_000, 7);
        assert_eq!(
            world.players[&768]
                .position
                .map(|position| (position.x, position.y)),
            Some((7884.0, 8433.0)),
        );

        // Second life: the peer respawns elsewhere and the stream carries the new coordinate. This is
        // the state the observer has to keep.
        let second = position_update(4000, 6000, 1_010);
        let mut second_frame = snapshot_with(2, &[(768, 4000, 6000)], &[]);
        second_frame.header.tick = 1_010;
        second_frame.header.arena_stream_id = 7;
        second_frame.near_entities[0].life_seq = Some(2);
        second_frame.near_entities[0].sample_time = Some(
            crate::combat_timeline::PresentationSampleTime::from_tick(1_010),
        );
        world.apply_snapshot_received_event(&second_frame);
        world.apply_snapshot_player_position_event(&second, 2, 1_010, 7);
        assert_eq!(
            world.players[&768]
                .position
                .map(|position| (position.x, position.y)),
            Some((4000.0, 6000.0)),
        );

        // The death event for the first life arrives late, after the respawn was applied. The supersede
        // guard in `apply_player_death_event` is what should make this a no-op.
        world.apply_player_death_event(&PlayerDeath {
            green_id: -1,
            green_collectible_id: 0,
            killer_id: 2,
            killed_id: 768,
            bounty: 0,
            flag_transfer: 0,
            life_seq: 1,
            respawn_at_tick: 0,
        });

        let rendered = world.visible_players().any(|player| {
            player.player_id == 768
                && player.position.map(|position| (position.x, position.y))
                    == Some((4000.0, 6000.0))
        });
        assert!(
            rendered,
            "a death from a superseded life cleared a live peer's position, which leaves that peer \
             invisible to this observer for as long as it stands still"
        );
    }
}
