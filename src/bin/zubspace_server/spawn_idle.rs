//! Per-connection spawn grace, never a general AFK timeout.
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub(crate) struct SpawnIdleTracker {
    scope: Option<(u64, u32)>,
    active: bool,
    ready: bool,
    started: bool,
    elapsed: Duration,
    last_poll: Option<Instant>,
}

impl SpawnIdleTracker {
    fn scope(&mut self, stream: u64, life: u32) {
        if self.scope == Some((stream, life)) {
            return;
        }
        if self.scope.is_none_or(|(old, _)| old != stream) || self.active {
            self.elapsed = Duration::ZERO;
            self.started = false;
            self.last_poll = None;
        }
        self.scope = Some((stream, life));
        self.active = false;
        self.ready = false;
    }

    pub(crate) fn accepted_input(
        &mut self,
        stream: u64,
        life: u32,
        now: Instant,
        ordinary: bool,
        activity: bool,
    ) {
        self.scope(stream, life);
        if !ordinary {
            return;
        }
        if activity {
            self.active = true;
            self.elapsed = Duration::ZERO;
        }
        self.ready = true;
        self.started = true;
        self.last_poll.get_or_insert(now);
    }

    pub(crate) fn pause(&mut self, now: Instant) {
        self.last_poll = self.started.then_some(now);
    }

    pub(crate) fn due(&mut self, stream: u64, life: u32, now: Instant, alive: bool) -> bool {
        self.scope(stream, life);
        let counting = self.started && !self.active && (!alive || self.ready);
        if counting {
            if let Some(last) = self.last_poll {
                self.elapsed = self
                    .elapsed
                    .saturating_add(now.saturating_duration_since(last));
            }
        }
        self.last_poll = self.started.then_some(now);
        counting && self.elapsed > Duration::from_secs(120)
    }
}

pub(crate) fn enforce(state: &mut crate::state::ServerState, tick: u32, now: Instant) {
    let candidates = state
        .clients
        .iter_mut()
        .filter_map(|(&id, client)| {
            if !client.joined || client.player.ship >= 8 {
                client.spawn_idle = Default::default();
                return None;
            }
            if !client.snapshot_ready {
                client.spawn_idle.pause(now);
                return None;
            }
            let due =
                client
                    .spawn_idle
                    .due(client.arena_stream_id, client.life_seq, now, client.alive);
            due.then_some(id)
        })
        .collect::<Vec<_>>();
    for id in candidates {
        let Some(runtime) = state.active_player_runtime_identity(id) else {
            continue;
        };
        if !state.clients[&id]
            .outgoing_reliable
            .can_retain_application_frames(2)
        {
            continue;
        }
        let crate::arena::RosterChangeOutcome::Applied(roster) =
            crate::arena::force_spawn_idle_spectator_in_state(state, runtime, tick, now)
        else {
            continue;
        };
        let client = state.clients.get_mut(&id).expect("forced session retained");
        let fact = crate::forced_spectator_protocol::SpawnIdleSpectator {
            server_session_id: crate::wire::server_session_id(),
            arena_stream_id: client.arena_stream_id,
            player_id: id,
            predecessor_life_seq: runtime.life.life_seq,
            life_seq: client.life_seq,
        };
        client.spawn_idle = Default::default();
        crate::reliable::send_reliable_payload(client, &fact.encode());
        tracing::info!(
            event = "spawn_idle_spectator",
            player_id = id,
            session_id = client.session_id.get(),
            life_seq = client.life_seq,
            predecessor_life_seq = runtime.life.life_seq,
            "moved inactive fresh spawn to spectator"
        );
        for recipient in state.clients.values_mut().filter(|client| client.joined) {
            crate::reliable::send_reliable_payload(recipient, &roster);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spawn_idle_enforcement_advances_life_cleans_up_sends_reason_and_allows_return() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        let id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, id, 0);
        while rx.try_recv().is_ok() {}
        let now = Instant::now();
        let (stream, life) = {
            let mut guard = state.lock().unwrap();
            let client = guard.clients.get_mut(&id).unwrap();
            client.snapshot_ready = true;
            client.server_energy = 1;
            client.last_position = Some(vec![0; 27]);
            client.spawn_idle.accepted_input(
                client.arena_stream_id,
                client.life_seq,
                now,
                true,
                false,
            );
            (client.arena_stream_id, client.life_seq)
        };
        // An unowned/damaged ship-change attempt is not genuine flight or
        // firing activity and may not restart this grace period.
        crate::arena::update_ship_at_tick(
            &state,
            id,
            &[crate::arena::SHIP_CHANGE_REQUEST, 1, 1, 0],
            100,
        );
        {
            let mut guard = state.lock().unwrap();
            enforce(&mut guard, 12100, now + Duration::from_secs(121));
            let client = &guard.clients[&id];
            assert_eq!(client.player.ship, 8);
            assert_eq!(client.life_seq, life + 1);
            assert!(client.last_position.is_none());
            assert!(!client.alive);
            assert!(client.respawn_at_tick.is_none());
            assert_eq!(guard.quality_holds.len(), 0);
            assert_eq!(guard.quality_holds.forced, 0);
            enforce(&mut guard, 12200, now + Duration::from_secs(122));
            assert_eq!(guard.clients[&id].life_seq, life + 1);
        }
        let facts = rx
            .try_iter()
            .filter_map(|packet| {
                crate::reliable::reliable_payload(&packet).and_then(|payload| {
                    crate::forced_spectator_protocol::SpawnIdleSpectator::parse(payload).ok()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].arena_stream_id, stream);
        assert_eq!(facts[0].predecessor_life_seq, life);
        crate::arena::update_ship_at_tick(
            &state,
            id,
            &[crate::arena::SHIP_CHANGE_REQUEST, 0, 1, 0],
            12300,
        );
        let guard = state.lock().unwrap();
        assert_eq!(
            guard.clients[&id].player.ship, 0,
            "idle spectating imposes no return hold"
        );
    }

    #[test]
    fn spawn_idle_reliable_pressure_defers_without_life_or_quality_mutation() {
        let state =
            std::sync::Arc::new(std::sync::Mutex::new(crate::state::ServerState::default()));
        let (tx, _rx) = std::sync::mpsc::channel();
        let id = crate::arena::register_connection(&state, tx);
        crate::arena::join_arena(&state, id, 0);
        let now = Instant::now();
        let mut guard = state.lock().unwrap();
        let client = guard.clients.get_mut(&id).unwrap();
        client.snapshot_ready = true;
        let life = client.life_seq;
        client
            .spawn_idle
            .accepted_input(client.arena_stream_id, life, now, true, false);
        while client.outgoing_reliable.can_retain_application_frames(2) {
            crate::reliable::send_reliable_payload(client, &[0x08]);
        }
        let retained = client.outgoing_reliable.pending_len();
        enforce(&mut guard, 12100, now + Duration::from_secs(121));
        assert_eq!(guard.clients[&id].player.ship, 0);
        assert_eq!(guard.clients[&id].life_seq, life);
        assert_eq!(guard.clients[&id].outgoing_reliable.pending_len(), retained);
        assert_eq!(guard.quality_holds.len(), 0);
    }
    #[test]
    fn spawn_idle_starts_after_controllable_input_and_uses_strict_two_minutes() {
        let t = Instant::now();
        let mut idle = SpawnIdleTracker::default();
        idle.accepted_input(1, 1, t, false, false);
        assert!(!idle.due(1, 1, t + Duration::from_secs(600), true));
        idle.accepted_input(1, 1, t + Duration::from_secs(600), true, false);
        assert!(!idle.due(1, 1, t + Duration::from_secs(720), true));
        assert!(idle.due(1, 1, t + Duration::from_millis(720001), true));
    }
    #[test]
    fn spawn_idle_activity_cancels_for_life_but_respawn_and_new_arena_get_grace() {
        let t = Instant::now();
        let mut idle = SpawnIdleTracker::default();
        idle.accepted_input(1, 1, t, true, true);
        assert!(!idle.due(1, 1, t + Duration::from_secs(3600), true));
        idle.accepted_input(1, 2, t + Duration::from_secs(3601), true, false);
        assert!(!idle.due(1, 2, t + Duration::from_secs(3721), true));
        assert!(idle.due(1, 2, t + Duration::from_secs(3722), true));
        idle.accepted_input(2, 1, t + Duration::from_secs(3723), true, false);
        assert!(!idle.due(2, 1, t + Duration::from_secs(3724), true));
    }
    #[test]
    fn spawn_idle_wall_clock_budget_survives_repeated_idle_deaths() {
        let t = Instant::now();
        let mut idle = SpawnIdleTracker::default();
        for life in 1..=4 {
            let start = Duration::from_secs(u64::from(life - 1) * 30);
            idle.accepted_input(1, life, t + start, true, false);
            assert!(!idle.due(1, life, t + start + Duration::from_secs(20), true));
            assert!(!idle.due(1, life, t + start + Duration::from_secs(30), false));
        }
        assert!(idle.due(1, 4, t + Duration::from_secs(121), false));
    }

    #[test]
    fn spawn_idle_pauses_loading_after_death_without_losing_the_idle_episode() {
        let t = Instant::now();
        let mut idle = SpawnIdleTracker::default();
        idle.accepted_input(1, 1, t, true, false);
        assert!(!idle.due(1, 1, t + Duration::from_secs(30), true));
        assert!(!idle.due(1, 2, t + Duration::from_secs(40), false));
        assert!(!idle.due(1, 2, t + Duration::from_secs(300), true));
        idle.accepted_input(1, 2, t + Duration::from_secs(300), true, false);
        assert!(!idle.due(1, 2, t + Duration::from_secs(380), true));
        assert!(idle.due(1, 2, t + Duration::from_secs(381), true));
    }
}
