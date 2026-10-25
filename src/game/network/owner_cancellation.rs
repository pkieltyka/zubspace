use super::*;

impl Game {
    pub(in crate::game) fn apply_owner_weapon_cancellation(
        &mut self,
        server: u64,
        arena: u64,
        event_id: u32,
        cancellation: crate::realtime_event::RealtimeOwnerWeaponCancellation,
    ) -> bool {
        let scope = self.client_authority.view().scope;
        if event_id == 0
            || scope.server_session_id != Some(server)
            || scope.arena_stream_id != Some(arena)
            || cancellation.source_session_id == 0
            || cancellation.owner_life_seq == 0
        {
            return false;
        }
        let Some(arena_scope) = RemoteWeaponArenaScope::authenticated(server, arena) else {
            return false;
        };
        let expired = self.latest_remote_realtime_event_id.is_some_and(|latest| {
            let distance = latest.wrapping_sub(event_id);
            distance >= crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS as u32
                && distance < (1_u32 << 31)
        });
        if expired && scope.player_id == Some(cancellation.source_player_id) {
            return false;
        }
        self.observe_remote_realtime_event(event_id);
        if self
            .remote_owner_weapon_cancellations
            .iter()
            .any(|fact| fact.event_id == event_id && fact.arena_scope == arena_scope)
        {
            return false;
        }
        // An ordered remote departure can arrive after its realtime horizon.
        // Clear existing copies, but do not retain an expired fact: original
        // carriers before that cutoff are already inadmissible at the session.
        if !expired {
            self.remote_owner_weapon_cancellations
                .push_back(OwnerWeaponCancellationFact {
                    event_id,
                    arena_scope,
                    cancellation,
                });
        }
        debug_assert!(
            self.remote_owner_weapon_cancellations.len()
                <= crate::realtime_event::REALTIME_EVENT_REORDER_WINDOW_BITS
        );
        let cancelled =
            |source: RemoteWeaponSource| source_cancelled(source, arena_scope, cancellation);
        self.remote_projectiles.retain(|p| !cancelled(p.source));
        self.prepared_remote_projectiles
            .retain(|p| !cancelled(p.source));
        self.pending_remote_birth_effects
            .retain(|effect| !cancelled(effect.source));
        self.remote_weapon_trails
            .retain(|trail| !cancelled(trail.source));
        self.decoys
            .retain(|d| d.remote_source.is_none_or(|source| !cancelled(source)));
        let pending_count = self.remote_weapon_presentations.len();
        for _ in 0..pending_count {
            let Some(pending) = self.remote_weapon_presentations.pop_front() else {
                break;
            };
            if pending.key.arena_scope == arena_scope
                && pending.source.weapon_data & 0x1f != 5
                && realtime_source_matches_cut(pending.source, cancellation)
            {
                self.remote_weapon_presentations
                    .mark_terminal(RemoteWeaponPresentationSuppressionReason::ScopeCancelled);
            } else {
                let reason = pending.suppression_reason;
                self.remote_weapon_presentations.requeue(pending, reason);
            }
        }
        if scope.player_id == Some(cancellation.source_player_id) {
            let keep = |p: Option<crate::model::OwnedProjectilePrediction>| {
                p.is_none_or(|prediction| {
                    let life_delta = prediction
                        .life_seq
                        .wrapping_sub(cancellation.owner_life_seq)
                        as i32;
                    !(life_delta < 0
                        || (life_delta == 0
                            && prediction
                                .input_sequence
                                .zip(cancellation.owner_input_sequence)
                                .is_some_and(|(seq, through)| {
                                    through.wrapping_sub(seq) < (1 << 15)
                                })))
                })
            };
            self.bullets.retain(|p| keep(p.prediction));
            self.bombs.retain(|p| keep(p.prediction));
            self.mines.retain(|p| keep(p.prediction));
            self.thors.retain(|p| keep(p.prediction));
            self.decoys.retain(|p| keep(p.prediction));
        }
        true
    }

    pub(in crate::game) fn remote_source_cancelled_by_owner(
        &self,
        source: RemoteWeaponSource,
    ) -> bool {
        self.remote_owner_weapon_cancellations
            .iter()
            .any(|fact| source_cancelled(source, fact.arena_scope, fact.cancellation))
    }

    pub(in crate::game) fn realtime_source_cancelled_by_owner(
        &self,
        arena: RemoteWeaponArenaScope,
        source: crate::realtime_event::RealtimeWeaponEvent,
    ) -> bool {
        self.remote_owner_weapon_cancellations.iter().any(|fact| {
            fact.arena_scope == arena && realtime_source_matches_cut(source, fact.cancellation)
        })
    }
}

fn realtime_source_matches_cut(
    source: crate::realtime_event::RealtimeWeaponEvent,
    cancellation: crate::realtime_event::RealtimeOwnerWeaponCancellation,
) -> bool {
    source.source_player_id == cancellation.source_player_id
        && source.source_session_id == cancellation.source_session_id
        && source.shot_group_id <= cancellation.through_shot_group_id
}

fn source_cancelled(
    source: RemoteWeaponSource,
    arena: RemoteWeaponArenaScope,
    cancellation: crate::realtime_event::RealtimeOwnerWeaponCancellation,
) -> bool {
    source.arena_scope == Some(arena)
        && matches!(source.identity, RemoteWeaponSourceIdentity::Authenticated { player_id, session_id, .. }
            if player_id == cancellation.source_player_id && session_id == cancellation.source_session_id)
        && source
            .shot_group_id
            .is_some_and(|group| group <= cancellation.through_shot_group_id)
}
