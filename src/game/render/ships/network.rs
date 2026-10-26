use super::*;

// Fifteen ordinary bots can exceed64 live projectiles without any overload.
// Keep QA capture bounded while retaining complete combat windows for that rig.
// This path is disabled unless sprite tracing was explicitly requested.
const MAX_REMOTE_PROJECTILE_DRAW_TRACE_ENTRIES: usize = 512;
const MAX_PREDICTED_BOMB_DRAW_TRACE_ENTRIES: usize = 64;

impl Game {
    pub(in crate::game::render) fn draw_network_players(&self, projectile_partial: f32) {
        let leader_id = self.arena_score_leader_id();
        let viewer_frequency = self.viewer_frequency();
        let viewer_xradar = self.ship.xradar;
        // Sprite/projectile draw evidence is a trace-only diagnostic. It must
        // not consume the separately throttled HUD-metrics request or build a
        // discarded formatted trace every 250 ms during ordinary gameplay.
        let trace_requested = remote_sprite_trace_requested();
        let mut trace_entries = Vec::new();
        let attached_children = self.network_world.attached_children_by_parent();
        for player in self.network_world.players.values() {
            let suppressed =
                if Some(player.player_id) == self.client_authority.view().scope.player_id {
                    Some("self")
                } else if player.ship >= 8 {
                    Some("spectator")
                } else if player.attach_parent != INVALID_PLAYER_ID {
                    Some("attached")
                } else if player.position.is_none() {
                    Some("no-position")
                } else {
                    None
                };
            if let Some(reason) = suppressed {
                if trace_requested {
                    trace_entries.push(remote_sprite_trace_entry(
                        self, player, None, false, reason, 0.0,
                    ));
                }
                continue;
            }
            let Some(pos) = player.display_position() else {
                if trace_requested {
                    trace_entries.push(remote_sprite_trace_entry(
                        self,
                        player,
                        None,
                        false,
                        "no-display-position",
                        0.0,
                    ));
                }
                continue;
            };
            let player_visible = network_player_world_visible(
                player.togglables,
                player.frequency,
                viewer_frequency,
                viewer_xradar,
            );
            if player_visible {
                let tint = WHITE;
                self.draw_ship_sprite(
                    pos,
                    player.display_orientation(),
                    player.ship as usize,
                    tint,
                );
                if trace_requested {
                    // Record only after the renderer enqueued the actual ship
                    // texture draw. This is intentionally stronger than the
                    // actor/presentation-state metrics used by authority E2E.
                    trace_entries.push(remote_sprite_trace_entry(
                        self,
                        player,
                        Some(pos),
                        true,
                        "none",
                        tint.a,
                    ));
                }
            } else if trace_requested {
                trace_entries.push(remote_sprite_trace_entry(
                    self,
                    player,
                    Some(pos),
                    false,
                    "cloak",
                    0.0,
                ));
            }
            let children: &[&NetworkPlayer] = attached_children
                .get(&player.player_id)
                .map_or(&[], Vec::as_slice);
            for child in children.iter().copied().filter(|child| {
                network_player_world_visible(
                    child.togglables,
                    child.frequency,
                    viewer_frequency,
                    viewer_xradar,
                )
            }) {
                self.draw_turret_sprite(pos, child.display_orientation(), WHITE);
            }
            if player_visible {
                if player.flags > 0 {
                    self.draw_flag_sprite(
                        pos + vec2(0.0, -22.0),
                        get_time() as f32,
                        player.frequency == viewer_frequency,
                    );
                }
                let label = ship_label(&player.name, player.bounty as u32, player.flags > 0, false);
                self.draw_ship_label_text(
                    pos + vec2(0.0, -34.0),
                    &label,
                    ship_label_color_for_frequency(
                        player.frequency,
                        viewer_frequency,
                        player.flags > 0,
                    ),
                );
                if leader_id == Some(player.player_id) {
                    self.draw_camera_relative_leader_badge(
                        pos + vec2(0.0, -34.0) - self.camera.target,
                        &label,
                    );
                }
                if crate::browser_metrics::debug_mode()
                    && let Some(energy) = player.display_energy()
                {
                    let initial_energy = self.tuning.ship(player.ship as usize).energy_initial;
                    if let Some((text, color)) = energy_label(energy, initial_energy) {
                        self.draw_ship_label(pos + vec2(0.0, -48.0), &text, color);
                    }
                }
            }
            for (index, child) in children
                .iter()
                .copied()
                .filter(|child| {
                    network_player_world_visible(
                        child.togglables,
                        child.frequency,
                        viewer_frequency,
                        viewer_xradar,
                    )
                })
                .enumerate()
            {
                let label = ship_label(&child.name, child.bounty as u32, child.flags > 0, false);
                self.draw_ship_label_text(
                    pos + vec2(0.0, -50.0 - index as f32 * 16.0),
                    &label,
                    ship_label_color_for_frequency(
                        child.frequency,
                        viewer_frequency,
                        child.flags > 0,
                    ),
                );
                if leader_id == Some(child.player_id) {
                    self.draw_camera_relative_leader_badge(
                        pos + vec2(0.0, -50.0 - index as f32 * 16.0) - self.camera.target,
                        &label,
                    );
                }
            }
        }
        if trace_requested {
            let (projectile_entries, projectile_total) =
                remote_projectile_draw_trace(self, projectile_partial);
            let predicted_bomb_entries = predicted_bomb_draw_trace(self, projectile_partial);
            let scope = self.client_authority.view().scope;
            publish_remote_sprite_frame(&format!(
                "{{\"screenWidth\":{},\"screenHeight\":{},\"snapshotId\":{},\"serverTick\":{},\"presentationRenderTick\":{},\"serverSessionId\":{},\"arenaStreamId\":{},\"viewerPlayerId\":{},\"viewerLifeSeq\":{},\"renderPartialSeconds\":{},\"viewerFrequency\":{},\"viewerXradar\":{},\"entries\":[{}],\"projectiles\":{{\"total\":{},\"overflow\":{},\"entries\":[{}]}},\"predictedBombs\":{{\"total\":{},\"overflow\":{},\"entries\":[{}]}}}}",
                screen_width(),
                screen_height(),
                option_u16_json(self.network_world.authoritative_snapshot_id),
                option_u32_json(self.browser_server_tick()),
                option_u32_json(self.observer_combat_timeline.presentation_render_tick()),
                option_u64_json(scope.server_session_id),
                option_u64_json(scope.arena_stream_id),
                option_u16_json(scope.player_id),
                option_u32_json(scope.life_seq),
                projectile_partial,
                viewer_frequency,
                viewer_xradar,
                trace_entries.join(","),
                projectile_total,
                projectile_total.saturating_sub(projectile_entries.len()),
                projectile_entries.join(","),
                self.bombs.len(),
                self.bombs
                    .len()
                    .saturating_sub(predicted_bomb_entries.len()),
                predicted_bomb_entries.join(","),
            ));
        }
    }
}

/// `draw_bombs` enqueues every local bomb earlier in this immutable pass.
/// Use its identical terrain-aware presentation, including animation age.
/// Identity may be unbound on the first frame; activation still links it to
/// the later submission and exact accepted-state retirement event.
fn predicted_bomb_draw_trace(game: &Game, partial_seconds: f32) -> Vec<String> {
    let gravity = game
        .tuning
        .gravity_bombs
        .then_some(game.ship_tuning().wormhole_gravity);
    game.bombs
        .iter()
        .take(MAX_PREDICTED_BOMB_DRAW_TRACE_ENTRIES)
        .map(|bomb| {
            let presented = local_bomb_presentation(
                bomb,
                partial_seconds,
                game.map(),
                &game.bricks,
                &game.tuning,
                gravity,
            );
            predicted_bomb_trace_entry(
                bomb,
                presented,
                game.camera.world_to_screen(presented.position),
            )
        })
        .collect()
}

fn predicted_bomb_trace_entry(
    bomb: &Bomb,
    presented: crate::gameplay::WeaponPresentation,
    screen: Vec2,
) -> String {
    format!(
        "{{\"activation\":{},\"sourceLifeSeq\":{},\"inputSequence\":{},\"sourceProjectileIndex\":{},\"drawn\":true,\"x\":{},\"y\":{},\"screenX\":{},\"screenY\":{},\"velocityX\":{},\"velocityY\":{},\"age\":{},\"animationSeconds\":{},\"ttl\":{},\"level\":{},\"proximity\":{},\"bouncesRemaining\":{}}}",
        option_u64_json(bomb.prediction.map(|prediction| prediction.activation)),
        option_u32_json(bomb.prediction.map(|prediction| prediction.life_seq)),
        option_u16_json(
            bomb.prediction
                .and_then(|prediction| prediction.input_sequence)
        ),
        bomb.prediction.map_or_else(
            || "null".to_owned(),
            |prediction| prediction.projectile_index.to_string()
        ),
        presented.position.x,
        presented.position.y,
        screen.x,
        screen.y,
        bomb.vel.x,
        bomb.vel.y,
        bomb.age,
        presented.animation_seconds,
        bomb.ttl,
        bomb.level,
        bomb.proximity,
        presented.bounces_remaining,
    )
}

/// `draw_remote_projectiles` runs earlier in the same immutable render pass and
/// applies the shared viewport cull, so this reports that same per-frame
/// decision: an entry says `"drawn": false` with `"suppression": "offscreen"`
/// exactly when the draw path skipped it. Keep only the nearest bounded subset so
/// trace collection cannot turn an extreme projectile field into a browser stall.
fn remote_projectile_draw_trace(game: &Game, partial_seconds: f32) -> (Vec<String>, usize) {
    let mut projectiles = game.prepared_remote_projectiles.iter().collect::<Vec<_>>();
    projectiles.sort_by(|left, right| {
        left.pos
            .distance_squared(game.ship.pos)
            .total_cmp(&right.pos.distance_squared(game.ship.pos))
            .then_with(|| {
                left.authority_binding
                    .map(|binding| binding.projectile_id)
                    .cmp(&right.authority_binding.map(|binding| binding.projectile_id))
            })
    });
    let total = projectiles.len();
    let entries = projectiles
        .into_iter()
        .take(MAX_REMOTE_PROJECTILE_DRAW_TRACE_ENTRIES)
        .map(|projectile| {
            let kind = match projectile.kind {
                RemoteProjectileKind::Bullet { .. } => "bullets",
                RemoteProjectileKind::Burst { .. } => "empburst",
                RemoteProjectileKind::Shrapnel { .. } => "shrapnel",
                RemoteProjectileKind::Bomb { .. } => "bombs",
                RemoteProjectileKind::Mine { .. } => "mines",
                RemoteProjectileKind::Thor => "thor",
            };
            let (source_session_id, source_life_seq) = match projectile.source.identity {
                RemoteWeaponSourceIdentity::Authenticated {
                    session_id,
                    life_seq,
                    ..
                } => (Some(session_id), Some(life_seq)),
                RemoteWeaponSourceIdentity::Legacy { life_seq, .. } => (None, life_seq),
            };
            let (authority_projectile_id, authority_state_tick) = projectile
                .authority_binding
                .map_or((None, None), |binding| {
                    (Some(binding.projectile_id), Some(binding.state_tick))
                });
            let presented = game.remote_projectile_draw_presentation(projectile, partial_seconds);
            let screen = game.camera.world_to_screen(presented.position);
            let drawn = remote_projectile_is_visible(
                screen,
                screen_width(),
                screen_height(),
                REMOTE_PROJECTILE_CULL_MARGIN_PX,
            );
            let suppression = if drawn { "" } else { "offscreen" };
            format!(
                "{{\"sourcePlayerId\":{},\"sourceSessionId\":{},\"sourceLifeSeq\":{},\"eventId\":{},\"shotGroupId\":{},\"sourceProjectileIndex\":{},\"authorityProjectileId\":{},\"authorityStateTick\":{},\"sourceServerTick\":{},\"poseTick\":{},\"posePartialSeconds\":{},\"drawn\":{},\"motionFrozen\":{},\"suppression\":\"{}\",\"texture\":\"{}\",\"x\":{:.2},\"y\":{:.2},\"screenX\":{:.2},\"screenY\":{:.2},\"velocityX\":{},\"velocityY\":{},\"age\":{},\"animationSeconds\":{},\"ownerPredictionLeadSeconds\":{},\"bouncesRemaining\":{},\"ttl\":{:.3}}}",
                projectile.source.identity.player_id(),
                option_u64_json(source_session_id),
                option_u32_json(source_life_seq),
                option_u32_json(projectile.source.realtime_event_id),
                option_u64_json(projectile.source.shot_group_id),
                projectile.source_projectile_index,
                option_u32_json(authority_projectile_id),
                option_u32_json(authority_state_tick),
                option_u32_json(projectile.source.realtime_server_tick),
                option_u32_json(projectile.presentation_clock.map(|clock| clock.pose_tick)),
                projectile.presentation_clock.map_or(0.0, |clock| clock.render_partial_seconds),
                drawn,
                projectile.presentation_clock.is_some_and(|clock| clock.motion_frozen),
                suppression,
                kind,
                presented.position.x,
                presented.position.y,
                screen.x,
                screen.y,
                projectile.vel.x,
                projectile.vel.y,
                projectile.age,
                presented.animation_seconds,
                presented.owner_prediction_lead_seconds,
                presented.bounces_remaining,
                projectile.ttl,
            )
        })
        .collect();
    (entries, total)
}

fn remote_sprite_trace_entry(
    game: &Game,
    player: &NetworkPlayer,
    pos: Option<Vec2>,
    drawn: bool,
    suppression: &str,
    alpha: f32,
) -> String {
    let screen = pos.map(|position| game.camera.world_to_screen(position));
    let authoritative_life_seq = game
        .network_world
        .authoritative_snapshot_actors
        .iter()
        .find(|actor| actor.player_id == player.player_id)
        .and_then(|actor| actor.life_seq);
    let authoritative_member =
        authoritative_life_seq.is_some() && authoritative_life_seq == player.life_seq;
    // The drawn bin comes from the interpolated display heading, so the record also carries the
    // continuous heading and its authoritative target: a browser gate can then separate "the
    // renderer drew the heading it was given" from "the heading it was given matched the shot".
    let display_orientation = player.display_orientation();
    let frame = ship_sprite_frame(display_orientation);
    let interpolation_phase = if player.interpolation_duration > 0.0 {
        (player.interpolation_elapsed / player.interpolation_duration).clamp(0.0, 1.0)
    } else {
        1.0
    };
    format!(
        "{{\"id\":{},\"lifeSeq\":{},\"sourceSessionId\":{},\"authoritativeLifeSeq\":{},\"authoritativeMember\":{},\"drawn\":{},\"suppression\":\"{}\",\"texture\":\"ships\",\"ship\":{},\"shipFrame\":{},\"togglables\":{},\"frequency\":{},\"positionPresent\":{},\"renderPositionPresent\":{},\"lastSeenSnapshotId\":{},\"lastServerPositionTick\":{},\"authoritativeX\":{},\"authoritativeY\":{},\"renderedX\":{},\"renderedY\":{},\"interpolationStartX\":{},\"interpolationStartY\":{},\"interpolationTargetX\":{},\"interpolationTargetY\":{},\"interpolationElapsed\":{},\"interpolationDuration\":{},\"interpolationPhase\":{},\"interpolationSampleInterval\":{},\"screenX\":{},\"screenY\":{},\"alpha\":{:.3},\"presentationInitialCount\":{},\"presentationSnapCount\":{},\"lastPresentationSnapReason\":{},\"displayOrientation\":{},\"authoritativeOrientation\":{},\"previousAuthoritativeOrientation\":{},\"directedPresentation\":{},\"orientationInterpolationStart\":{},\"orientationInterpolationElapsed\":{},\"orientationInterpolationDuration\":{}}}",
        player.player_id,
        option_u32_json(player.life_seq),
        option_u64_json(player.source_session_id),
        option_u32_json(authoritative_life_seq),
        authoritative_member,
        drawn,
        suppression,
        player.ship,
        frame,
        player.togglables,
        player.frequency,
        player.position.is_some(),
        player.render_position.is_some(),
        option_u16_json(player.last_seen_snapshot_id),
        option_u16_json(player.last_server_position_tick),
        option_f32_json(player.position.map(|position| position.x)),
        option_f32_json(player.position.map(|position| position.y)),
        option_f32_json(pos.map(|position| position.x)),
        option_f32_json(pos.map(|position| position.y)),
        option_f32_json(
            player
                .interpolation_start_position
                .map(|position| position.x)
        ),
        option_f32_json(
            player
                .interpolation_start_position
                .map(|position| position.y)
        ),
        option_f32_json(
            player
                .interpolation_target_position
                .map(|position| position.x)
        ),
        option_f32_json(
            player
                .interpolation_target_position
                .map(|position| position.y)
        ),
        option_f32_json(Some(player.interpolation_elapsed)),
        option_f32_json(Some(player.interpolation_duration)),
        option_f32_json(Some(interpolation_phase)),
        option_f32_json(Some(player.interpolation_sample_interval)),
        option_f32_json(screen.map(|point| point.x)),
        option_f32_json(screen.map(|point| point.y)),
        alpha,
        player.presentation_initial_count,
        player.presentation_snap_count,
        player.last_presentation_snap_reason,
        exact_f32_json(Some(display_orientation)),
        exact_f32_json(Some(player.orientation)),
        exact_f32_json(player.previous_authoritative_orientation),
        player.directed_presentation,
        exact_f32_json(player.previous_orientation),
        exact_f32_json(Some(player.orientation_interpolation_elapsed)),
        exact_f32_json(Some(player.orientation_interpolation_duration)),
    )
}

fn option_u64_json(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| format!("\"{value}\""))
}

fn option_u16_json(value: Option<u16>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

fn option_u32_json(value: Option<u32>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

fn option_f32_json(value: Option<f32>) -> String {
    match value {
        Some(value) if value.is_finite() => format!("{value:.2}"),
        _ => "null".to_owned(),
    }
}

/// Headings and interpolation windows need the exact value: `option_f32_json` rounds positions to
/// hundredths, and a hundredth of a turn is 0.4 sprite bins - enough to land a published heading on
/// the far side of the boundary that decided the sprite it was drawn with.
fn exact_f32_json(value: Option<f32>) -> String {
    match value {
        Some(value) if value.is_finite() => format!("{value}"),
        _ => "null".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{exact_f32_json, option_f32_json, ship_sprite_frame};

    #[test]
    fn predicted_bomb_trace_uses_the_draw_pose_and_unrounded_animation_age() {
        use super::*;
        let mut bomb = Bomb {
            prediction: Some(crate::model::OwnedProjectilePrediction {
                activation: u64::MAX,
                life_seq: 2,
                input_sequence: None,
                projectile_index: 0,
            }),
            pos: vec2(100.0, 100.0),
            vel: vec2(0.0, -485.0),
            frequency: 1,
            ttl: 3.0,
            age: 0.125,
            trail_timer: 0.0,
            bounces_remaining: 1,
            level: 3,
            damage: 100.0,
            emp_shutdown_time: 0.0,
            shrapnel: ExplosiveShrapnel::none(),
            proximity: true,
            proximity_sensor: None,
        };
        // Deliberately differs from canonical pose/age/bounce count: the trace
        // must describe the terrain-aware draw and keep canonical storage intact.
        let presented = crate::gameplay::WeaponPresentation {
            position: vec2(100.0, 97.375),
            animation_seconds: 0.1328125,
            bounces_remaining: 0,
            burst_active: false,
            owner_prediction_lead_seconds: 0.0,
        };
        let before = bomb;
        let row: serde_json::Value = serde_json::from_str(&predicted_bomb_trace_entry(
            &bomb,
            presented,
            vec2(-1.0, 20.0),
        ))
        .unwrap();
        assert_eq!(row["activation"], u64::MAX.to_string());
        assert!(row["inputSequence"].is_null());
        assert_eq!(row["sourceLifeSeq"], 2);
        assert_eq!(row["y"], 97.375);
        assert_eq!(row["animationSeconds"], 0.1328125);
        assert_eq!(row["bouncesRemaining"], 0);
        assert_eq!(row["drawn"], true, "local draws have no viewport cull");
        assert_eq!(bomb, before);
        bomb.prediction.as_mut().unwrap().input_sequence = Some(42);
        let submitted: serde_json::Value =
            serde_json::from_str(&predicted_bomb_trace_entry(&bomb, presented, Vec2::ZERO))
                .unwrap();
        assert_eq!(submitted["activation"], row["activation"]);
        assert_eq!(submitted["inputSequence"], 42);
    }

    /// A published heading has to reproduce the sprite bin it was drawn with: `tests/e2e/bot_facing.mjs`
    /// re-derives the bin from `displayOrientation`, and the position-grade two-decimal format used to
    /// put the published value on the far side of that boundary.
    #[test]
    fn published_headings_round_trip_through_their_sprite_bin() {
        for step in 0..40_000_u32 {
            let orientation = (step as f64 / 40_000.0) as f32;
            let published: f32 = exact_f32_json(Some(orientation))
                .parse()
                .expect("heading json");
            let expected = ship_sprite_frame(orientation);
            assert_eq!(
                ship_sprite_frame(published),
                expected,
                "heading {orientation} published as {published}"
            );
        }
        // The position format is still two decimals; only the exact helper round-trips.
        let lossy: f32 = option_f32_json(Some(0.549_999_9)).parse().unwrap();
        assert_ne!(
            ship_sprite_frame(lossy),
            ship_sprite_frame(0.549_999_9),
            "two decimals must still be the lossy choice this helper exists to avoid"
        );
    }
}
