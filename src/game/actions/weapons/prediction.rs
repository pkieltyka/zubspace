use super::*;

impl Game {
    /// Counts ownership independently of predicted versus authoritative storage.
    pub(in crate::game) fn owned_weapon_counts(&self) -> [usize; 5] {
        let mut counts = [
            self.bullets.len(),
            self.bombs.len(),
            self.mines.len(),
            self.shrapnel.len(),
            self.thors.len(),
        ];
        let owner = self.client_authority.view().scope.player_id;
        for projectile in &self.remote_projectiles {
            if Some(projectile.source.identity.player_id()) != owner {
                continue;
            }
            let index = match projectile.kind {
                RemoteProjectileKind::Bullet { .. } | RemoteProjectileKind::Burst { .. } => 0,
                RemoteProjectileKind::Bomb { .. } => 1,
                RemoteProjectileKind::Mine { .. } => 2,
                RemoteProjectileKind::Shrapnel { .. } => 3,
                RemoteProjectileKind::Thor => 4,
            };
            counts[index] += 1;
        }
        counts
    }

    pub(in crate::game) fn local_weapon_prediction(
        &self,
        projectile_index: u8,
    ) -> Option<crate::model::OwnedProjectilePrediction> {
        Some(crate::model::OwnedProjectilePrediction {
            activation: self.pending_weapon_prediction?,
            life_seq: self.client_authority.view().scope.life_seq?,
            input_sequence: None,
            projectile_index,
        })
    }

    pub(in crate::game) fn bind_pending_weapon_prediction(&mut self, sequence: u16) {
        let Some(activation) = self.pending_weapon_prediction.take() else {
            return;
        };
        // A prior input-sequence cycle cannot suppress a newly submitted shot.
        let life = self.client_authority.view().scope.life_seq;
        self.predicted_explosion_receipts.retain(|p| {
            p.activation == activation
                || Some(p.life_seq) != life
                || p.input_sequence != Some(sequence)
        });
        for prediction in &mut self.predicted_explosion_receipts {
            if prediction.activation == activation {
                prediction.input_sequence = Some(sequence);
            }
        }
        for prediction in self
            .bullets
            .iter_mut()
            .map(|p| &mut p.prediction)
            .chain(self.bombs.iter_mut().map(|p| &mut p.prediction))
            .chain(self.mines.iter_mut().map(|p| &mut p.prediction))
            .chain(self.thors.iter_mut().map(|p| &mut p.prediction))
            .chain(self.decoys.iter_mut().map(|p| &mut p.prediction))
            .chain(self.explosions.iter_mut().map(|p| &mut p.prediction))
            .flatten()
        {
            if prediction.activation == activation {
                prediction.input_sequence = Some(sequence);
            }
        }
    }

    pub(in crate::game) fn cancel_pending_weapon_prediction(&mut self) {
        if let Some(activation) = self.pending_weapon_prediction.take() {
            self.remove_unsubmitted_weapon_prediction(activation);
        }
    }

    pub(in crate::game) fn remove_unsubmitted_weapon_prediction(&mut self, activation: u64) {
        let keep = |prediction: Option<crate::model::OwnedProjectilePrediction>| {
            prediction.is_none_or(|p| p.activation != activation || p.input_sequence.is_some())
        };
        self.bullets.retain(|p| keep(p.prediction));
        self.bombs.retain(|p| keep(p.prediction));
        self.mines.retain(|p| keep(p.prediction));
        self.thors.retain(|p| keep(p.prediction));
        self.explosions.retain(|p| keep(p.prediction));
        self.decoys.retain(|p| keep(p.prediction));
        self.predicted_explosion_receipts.retain(|p| keep(Some(*p)));
    }

    pub(in crate::game) fn retire_authoritative_weapon_prediction(
        &mut self,
        state: crate::realtime_event::RealtimeProjectileStateEvent,
        capture_tick: u32,
    ) -> Option<Bomb> {
        if state.weapon_data & 0x1f == 15 {
            return None;
        }
        let Some(sequence) = state.source_input_sequence else {
            return None;
        };
        let before = self.bullets.len()
            + self.bombs.len()
            + self.mines.len()
            + self.thors.len()
            + self.decoys.len();
        let linked_terminal = state.terminal && matches!(state.weapon_data & 0x1f, 1 | 2);
        let keep = |prediction: Option<crate::model::OwnedProjectilePrediction>| {
            prediction.is_none_or(|p| {
                p.life_seq != state.source.source_life_seq
                    || p.input_sequence != Some(sequence)
                    || (!linked_terminal && p.projectile_index != state.source_projectile_index)
            })
        };
        let retired_bomb = (!state.terminal)
            .then(|| {
                self.bombs
                    .iter()
                    .find(|bomb| !keep(bomb.prediction))
                    .copied()
            })
            .flatten();
        if remote_sprite_trace_requested() {
            for bomb in self.bombs.iter().filter(|bomb| !keep(bomb.prediction)) {
                // Bind exact identities at retirement, never by nearest position.
                // Per-frame trace rows retain the actual final predicted draw;
                // this event contains the received authority pose, not that draw.
                let prediction = bomb.prediction.unwrap();
                publish_weapon_admission_event(&owner_bomb_handoff_trace(
                    self.client_authority.view().scope,
                    prediction,
                    state,
                    capture_tick,
                    self.browser_server_tick(),
                    bomb,
                ));
            }
        }
        self.bullets.retain(|p| keep(p.prediction));
        self.bombs.retain(|p| keep(p.prediction));
        self.mines.retain(|p| keep(p.prediction));
        self.thors.retain(|p| keep(p.prediction));
        if state.weapon_data & 0x1f == 6 {
            self.decoys
                .retain(|p| p.remote_source.is_some() || keep(p.prediction));
        }
        let after = self.bullets.len()
            + self.bombs.len()
            + self.mines.len()
            + self.thors.len()
            + self.decoys.len();
        self.owned_projectile_reconciliations = self
            .owned_projectile_reconciliations
            .saturating_add(before.saturating_sub(after) as u64);
        retired_bomb
    }
    pub(in crate::game) fn reconcile_predicted_explosion(
        &mut self,
        state: crate::realtime_event::RealtimeProjectileStateEvent,
        identity: RemoteWeaponSourceIdentity,
        shutdown_time: f32,
    ) -> bool {
        let Some(sequence) = state.source_input_sequence else {
            return false;
        };
        let matches = |p: crate::model::OwnedProjectilePrediction| {
            p.life_seq == state.source.source_life_seq
                && p.input_sequence == Some(sequence)
                && p.projectile_index == state.source_projectile_index
        };
        if !self
            .predicted_explosion_receipts
            .iter()
            .copied()
            .any(matches)
        {
            return false;
        }
        for explosion in &mut self.explosions {
            if explosion.prediction.is_some_and(matches) {
                explosion.pos = vec2(state.position_x, state.position_y);
                explosion.damage_pos = explosion.pos;
                explosion.kind = explosion_kind_for_emp_shutdown(shutdown_time);
                explosion.damages_bots = false;
                explosion.damage = 0.0;
                explosion.radius = 0.0;
                explosion.emp_shutdown_time = shutdown_time;
                explosion.remote_source = Some(identity);
                explosion.prediction = None;
                // Preserve age and the already-played sound, rather than
                // restart an animation or replay an expired cosmetic effect.
            }
        }
        true
    }
}

fn owner_bomb_handoff_trace(
    scope: AuthorityScopeIdentity,
    prediction: crate::model::OwnedProjectilePrediction,
    state: crate::realtime_event::RealtimeProjectileStateEvent,
    capture_tick: u32,
    observed_tick: Option<u32>,
    bomb: &Bomb,
) -> String {
    format!(
        "{{\"phase\":\"owner-bomb-handoff\",\"serverSessionId\":{},\"arenaStreamId\":{},\"sourcePlayerId\":{},\"sourceSessionId\":\"{}\",\"sourceLifeSeq\":{},\"activation\":\"{}\",\"inputSequence\":{},\"sourceProjectileIndex\":{},\"authorityProjectileId\":{},\"shotGroupId\":\"{}\",\"weaponData\":{},\"sourceServerTick\":{},\"captureTick\":{},\"observedTick\":{},\"expiryTick\":{},\"terminal\":{},\"motionFrozen\":{},\"authorityX\":{},\"authorityY\":{},\"authorityVelocityX\":{},\"authorityVelocityY\":{},\"predictedX\":{},\"predictedY\":{},\"predictedAge\":{},\"predictedTtl\":{}}}",
        scope
            .server_session_id
            .map_or_else(|| "null".to_owned(), |id| format!("\"{id}\"")),
        scope
            .arena_stream_id
            .map_or_else(|| "null".to_owned(), |id| format!("\"{id}\"")),
        state.source.source_player_id,
        state.source.source_session_id,
        state.source.source_life_seq,
        prediction.activation,
        prediction
            .input_sequence
            .map_or_else(|| "null".to_owned(), |id| id.to_string()),
        state.source_projectile_index,
        state.projectile_id,
        state.source.shot_group_id,
        state.weapon_data,
        state.source_server_tick,
        capture_tick,
        observed_tick.map_or_else(|| "null".to_owned(), |tick| tick.to_string()),
        state
            .expiry_tick
            .map_or_else(|| "null".to_owned(), |tick| tick.to_string()),
        state.terminal,
        state.motion_frozen,
        state.position_x,
        state.position_y,
        state.velocity_x,
        state.velocity_y,
        bomb.pos.x,
        bomb.pos.y,
        bomb.age,
        bomb.ttl,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_trace_preserves_exact_identity_clocks_and_terminal_state() {
        let prediction = crate::model::OwnedProjectilePrediction {
            activation: u64::MAX,
            life_seq: 7,
            input_sequence: Some(u16::MAX),
            projectile_index: 2,
        };
        let bomb = Bomb {
            prediction: Some(prediction),
            pos: vec2(100.25, 90.5),
            vel: vec2(0.0, -485.0),
            frequency: 1,
            ttl: 3.5,
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
        let source = crate::realtime_event::RealtimeWeaponEvent {
            source_player_id: 9,
            source_session_id: u64::MAX - 1,
            source_life_seq: 7,
            shot_group_id: u64::MAX - 2,
            x: 100.0,
            y: 110.0,
            velocity_x: 0.0,
            velocity_y: -325.0,
            orientation: 0.0,
            frequency: 1,
            ship: 0,
            weapon_data: 3,
        };
        let mut state = crate::realtime_event::RealtimeProjectileStateEvent {
            source_input_sequence: Some(u16::MAX),
            terminal: false,
            motion_frozen: false,
            source,
            projectile_id: u32::MAX,
            source_server_tick: u32::MAX - 3,
            source_projectile_index: 2,
            weapon_data: 3,
            position_x: 100.0,
            position_y: 105.0,
            velocity_x: 0.0,
            velocity_y: -485.0,
            expiry_tick: Some(200),
            behavior: crate::realtime_event::PROJECTILE_BEHAVIOR_STANDARD,
            bounces_remaining: 1,
            proximity: None,
        };
        let scope = AuthorityScopeIdentity {
            server_session_id: Some(u64::MAX - 3),
            arena_stream_id: Some(u64::MAX - 4),
            ..Default::default()
        };
        for terminal in [false, true] {
            state.terminal = terminal;
            state.motion_frozen = terminal;
            let row: serde_json::Value = serde_json::from_str(&owner_bomb_handoff_trace(
                scope,
                prediction,
                state,
                1,
                Some(4),
                &bomb,
            ))
            .unwrap();
            assert_eq!(row["activation"], u64::MAX.to_string());
            assert_eq!(row["sourceSessionId"], (u64::MAX - 1).to_string());
            assert_eq!(row["shotGroupId"], (u64::MAX - 2).to_string());
            assert_eq!(row["serverSessionId"], (u64::MAX - 3).to_string());
            assert_eq!(row["arenaStreamId"], (u64::MAX - 4).to_string());
            assert_eq!(row["sourceLifeSeq"], 7);
            assert_eq!(row["inputSequence"], u16::MAX);
            assert_eq!(row["sourceProjectileIndex"], 2);
            assert_eq!(row["authorityProjectileId"], u32::MAX);
            assert_eq!(row["sourceServerTick"], u32::MAX - 3);
            assert_eq!(row["captureTick"], 1);
            assert_eq!(row["observedTick"], 4);
            assert_eq!(row["expiryTick"], 200);
            assert_eq!(row["terminal"], terminal);
            assert_eq!(row["motionFrozen"], terminal);
            assert_eq!(row["predictedY"], 90.5);
            assert_eq!(row["authorityY"], 105.0);
            assert_eq!(row["predictedAge"], 0.125);
        }
    }
}
