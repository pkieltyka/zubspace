use super::*;

impl Game {
    /// One correlated fresh-shot/owner draw intent, sampled at most at 10 Hz
    /// by the JS collector. Never enable the expensive HUD trace to obtain it.
    pub(super) fn capture_client_debug_muzzle(&self, partial: f32) {
        let viewer = self.client_authority.view().scope.player_id;
        let viewport = vec2(screen_width(), screen_height());
        let mut candidates = 0;
        for projectile in self.prepared_remote_projectiles.iter().rev().take(64) {
            if !matches!(projectile.kind, RemoteProjectileKind::Bullet { .. })
                || !(0.0..=0.25).contains(&projectile.age)
            {
                continue;
            }
            let RemoteWeaponSourceIdentity::Authenticated {
                player_id,
                session_id,
                life_seq,
            } = projectile.source.identity
            else {
                continue;
            };
            if viewer == Some(player_id) {
                continue;
            }
            let Some(owner) = self.network_world.players.get(&player_id) else {
                continue;
            };
            if owner.ship >= 8
                || owner.attach_parent != INVALID_PLAYER_ID
                || !projectile.source.identity.matches_authenticated_runtime(
                    owner.player_id,
                    owner.source_session_id,
                    owner.life_seq,
                )
                || !network_player_world_visible(
                    owner.togglables,
                    owner.frequency,
                    self.viewer_frequency(),
                    self.ship.xradar,
                )
            {
                continue;
            }
            let (Some(owner_pos), Some(authority_pos), Some(group), Some(source_tick)) = (
                owner.display_position(),
                owner.position,
                projectile.source.shot_group_id,
                projectile.source.realtime_server_tick,
            ) else {
                continue;
            };
            let presented = self.remote_projectile_draw_presentation(projectile, partial);
            let screen = self.camera.world_to_screen(presented.position);
            if !remote_projectile_is_visible(
                screen,
                viewport.x,
                viewport.y,
                REMOTE_PROJECTILE_CULL_MARGIN_PX,
            ) || !remote_projectile_is_visible(
                self.camera.world_to_screen(owner_pos),
                viewport.x,
                viewport.y,
                REMOTE_PROJECTILE_CULL_MARGIN_PX,
            ) {
                continue;
            }
            let orientation = owner.display_orientation();
            let row = [
                f64::from(player_id),
                f64::from(life_seq),
                (session_id >> 32) as f64,
                (session_id as u32) as f64,
                (group >> 32) as f64,
                (group as u32) as f64,
                f64::from(projectile.source_projectile_index),
                f64::from(owner.ship),
                f64::from(source_tick),
                self.browser_server_tick().map_or(-1.0, f64::from),
                self.observer_combat_timeline
                    .presentation_render_tick()
                    .map_or(-1.0, f64::from),
                projectile
                    .presentation_clock
                    .map_or(-1.0, |clock| f64::from(clock.pose_tick)),
                f64::from(projectile.age),
                f64::from(projectile.source.fire_position.x),
                f64::from(projectile.source.fire_position.y),
                f64::from(projectile.source.fire_velocity.x),
                f64::from(projectile.source.fire_velocity.y),
                f64::from(projectile.source.fire_orientation),
                f64::from(owner_pos.x),
                f64::from(owner_pos.y),
                f64::from(orientation),
                f64::from(authority_pos.x),
                f64::from(authority_pos.y),
                f64::from(owner.orientation),
                f64::from(presented.position.x),
                f64::from(presented.position.y),
                f64::from(projectile.vel.x),
                f64::from(projectile.vel.y),
                ships::ship_sprite_frame(orientation) as f64,
                f64::from(projectile.weapon_data),
                owner.last_server_position_tick.map_or(-1.0, f64::from),
                f64::from(self.weapon_render_partial_seconds()),
            ];
            if crate::browser_metrics::publish_client_debug_muzzle(&row) {
                break;
            }
            candidates += 1;
            if candidates == 4 {
                break;
            }
        }
    }

    /// Compact numeric capture, independent of the full HUD diagnostic serializer.
    /// Only two projectile samples are retained; scanning is bounded explicitly.
    pub(super) fn capture_client_debug_frame(&self, local: &LocalRenderPrediction, partial: f32) {
        let scope = self.client_authority.view().scope;
        let projectile_partial = if self.gameplay_step_ready() {
            partial
        } else {
            0.0
        };
        let followed = self
            .spectating_player_id
            .and_then(|id| self.network_world.players.get(&id))
            .and_then(|player| player.display_position());
        let mut frame = [-1.0; 32];
        frame[..14].copy_from_slice(&[
            f64::from(partial),
            f64::from(local.position.x),
            f64::from(local.position.y),
            f64::from(self.ship.pos.x),
            f64::from(self.ship.pos.y),
            if followed.is_some() {
                self.spectating_player_id.map_or(-1.0, f64::from)
            } else {
                -1.0
            },
            followed.map_or(0.0, |pos| f64::from(pos.x)),
            followed.map_or(0.0, |pos| f64::from(pos.y)),
            self.observer_combat_timeline
                .presentation_render_tick()
                .map_or(-1.0, f64::from),
            self.browser_server_tick().map_or(-1.0, f64::from),
            scope.life_seq.map_or(-1.0, f64::from),
            self.self_correction_count as f64,
            u8::from(self.step_authority_ready()) as f64,
            u8::from(self.spectator_mode) as f64,
        ]);
        let viewport = vec2(screen_width(), screen_height());
        let visible = |pos| {
            let screen = self.camera.world_to_screen(pos);
            remote_projectile_is_visible(
                screen,
                viewport.x,
                viewport.y,
                REMOTE_PROJECTILE_CULL_MARGIN_PX,
            )
        };
        let bomb_gravity = self
            .tuning
            .gravity_bombs
            .then_some(self.ship_tuning().wormhole_gravity);
        let local_bomb = self.bombs.iter().take(16).find_map(|bomb| {
            let presented = local_bomb_presentation(
                bomb,
                projectile_partial,
                self.map(),
                &self.bricks,
                &self.tuning,
                bomb_gravity,
            );
            visible(presented.position).then_some((bomb, presented))
        });
        if let Some((bomb, presented)) = local_bomb {
            let id = bomb
                .prediction
                .map_or(0, |prediction| prediction.activation);
            frame[14..23].copy_from_slice(&[
                1.0,
                scope.player_id.map_or(-1.0, f64::from),
                bomb.prediction
                    .map_or(-1.0, |prediction| f64::from(prediction.life_seq)),
                (id >> 32) as f64,
                (id as u32) as f64,
                bomb.prediction
                    .map_or(-1.0, |prediction| f64::from(prediction.projectile_index)),
                f64::from(presented.position.x),
                f64::from(presented.position.y),
                f64::from(presented.animation_seconds),
            ]);
        }
        let remote = self
            .prepared_remote_projectiles
            .iter()
            .take(64)
            .filter(|p| matches!(p.kind, RemoteProjectileKind::Bomb { .. }))
            .find_map(|projectile| {
                let presented =
                    self.remote_projectile_draw_presentation(projectile, projectile_partial);
                visible(presented.position).then_some((projectile, presented))
            })
            .or_else(|| {
                self.prepared_remote_projectiles
                    .iter()
                    .take(64)
                    .find_map(|p| {
                        if !matches!(p.kind, RemoteProjectileKind::Bullet { .. }) {
                            return None;
                        }
                        let presented =
                            self.remote_projectile_draw_presentation(p, projectile_partial);
                        visible(presented.position).then_some((p, presented))
                    })
            });
        if let Some((projectile, presented)) = remote {
            let id = projectile.source.shot_group_id.unwrap_or(0);
            let life = match projectile.source.identity {
                RemoteWeaponSourceIdentity::Authenticated { life_seq, .. } => f64::from(life_seq),
                RemoteWeaponSourceIdentity::Legacy { life_seq, .. } => {
                    life_seq.map_or(-1.0, f64::from)
                }
            };
            let bomb = matches!(projectile.kind, RemoteProjectileKind::Bomb { .. });
            frame[23..32].copy_from_slice(&[
                if bomb { 3.0 } else { 2.0 },
                f64::from(projectile.source.identity.player_id()),
                life,
                (id >> 32) as f64,
                (id as u32) as f64,
                f64::from(projectile.source_projectile_index),
                f64::from(presented.position.x),
                f64::from(presented.position.y),
                f64::from(presented.animation_seconds),
            ]);
        }
        crate::browser_metrics::publish_client_debug_frame(&frame);
    }
}
