use super::*;
use crate::identity::{LoginAdmissionError, admit_player_login, reserved_human_count};
use std::collections::BTreeSet;
use std::sync::Barrier;

fn client(state: &Arc<Mutex<ServerState>>) -> (u16, mpsc::Receiver<Vec<u8>>) {
    let (tx, rx) = mpsc::channel();
    (register_connection(state, tx), rx)
}

fn admitted(
    state: &Arc<Mutex<ServerState>>,
    identity: &str,
    name: &str,
) -> (u16, mpsc::Receiver<Vec<u8>>) {
    let (id, rx) = client(state);
    admit_player_login(state, id, identity.into(), name.into()).unwrap();
    (id, rx)
}

#[test]
fn concurrent_normalized_names_have_exactly_one_owner() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (a, _a) = client(&state);
    let (b, _b) = client(&state);
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for (id, identity, name) in [(a, "100000000001", "  Ace "), (b, "100000000002", "aCe")] {
        let state = Arc::clone(&state);
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            barrier.wait();
            admit_player_login(&state, id, identity.into(), name.into())
        }));
    }
    barrier.wait();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| **r == Err(LoginAdmissionError::NameInUse))
            .count(),
        1
    );
    let state = state.lock().unwrap();
    assert_eq!(reserved_human_count(&state, None), 1);
    assert!(
        state
            .clients
            .values()
            .filter(|c| c.arena_admission_reserved)
            .all(|c| c.player.name == c.player.name.trim())
    );
    assert_eq!(
        state
            .clients
            .values()
            .filter(|c| c.player_identity.is_some())
            .count(),
        1
    );
}

#[test]
fn replayed_login_preserves_score_and_display_name_and_rejects_identity_change() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (id, _rx) = admitted(&state, "100000000001", "Ace");
    {
        let mut guard = state.lock().unwrap();
        let c = guard.clients.get_mut(&id).unwrap();
        c.player.kill_points = 91;
        c.player.wins = 3;
    }
    admit_player_login(&state, id, "100000000001".into(), "ACE".into()).unwrap();
    assert_eq!(
        admit_player_login(&state, id, "100000000002".into(), "Ace".into()),
        Err(LoginAdmissionError::InvalidIdentity)
    );
    assert_eq!(
        admit_player_login(&state, id, "100000000001".into(), "Other".into()),
        Err(LoginAdmissionError::InvalidIdentity)
    );
    let state = state.lock().unwrap();
    let c = &state.clients[&id];
    assert_eq!(
        (&*c.player.name, c.player.kill_points, c.player.wins),
        ("Ace", 91, 3)
    );
    assert_eq!(reserved_human_count(&state, None), 1);
}

#[test]
fn duplicate_private_identity_cannot_reserve_a_different_name() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (_a, _rx_a) = admitted(&state, "100000000001", "Alpha");
    let (b, _rx_b) = client(&state);
    assert_eq!(
        admit_player_login(&state, b, "100000000001".into(), "Beta".into()),
        Err(LoginAdmissionError::IdentityInUse)
    );
    assert!(!state.lock().unwrap().clients[&b].arena_admission_reserved);
}

#[test]
fn bots_and_pending_logins_share_the_500_member_boundary_atomically() {
    let mut initial = ServerState::new(ServerConfig {
        bot_count: 2,
        ..ServerConfig::default()
    });
    initial.initialize_configured_bots();
    let state = Arc::new(Mutex::new(initial));
    let mut receivers = Vec::new();
    for index in 0..497 {
        receivers.push(
            admitted(
                &state,
                &format!("{:012}", index + 1),
                &format!("Pilot{index}"),
            )
            .1,
        );
    }
    assert_eq!(reserved_human_count(&state.lock().unwrap(), None) + 2, 499);
    let (a, _a) = client(&state);
    let (b, _b) = client(&state);
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for (id, identity, name) in [(a, "900000000001", "LastA"), (b, "900000000002", "LastB")] {
        let state = Arc::clone(&state);
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            barrier.wait();
            (
                id,
                identity,
                name,
                admit_player_login(&state, id, identity.into(), name.into()),
            )
        }));
    }
    barrier.wait();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.3.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| r.3 == Err(LoginAdmissionError::Full))
            .count(),
        1
    );
    assert_eq!(reserved_human_count(&state.lock().unwrap(), None) + 2, 500);
    let winner = results.iter().find(|r| r.3.is_ok()).unwrap();
    let loser = results.iter().find(|r| r.3.is_err()).unwrap();
    disconnect_player(&state, winner.0);
    admit_player_login(&state, loser.0, loser.1.into(), loser.2.into()).unwrap();
    assert_eq!(reserved_human_count(&state.lock().unwrap(), None) + 2, 500);
}

#[test]
fn bot_creation_avoids_reserved_names_and_cannot_overbook_custom_capacity() {
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        max_players: 3,
        bot_count: 15,
        ..ServerConfig::default()
    })));
    let (_a, _a_rx) = admitted(&state, "100000000001", "Ace");
    let (_b, _b_rx) = admitted(&state, "100000000002", "Hyde");
    state.lock().unwrap().initialize_configured_bots();
    {
        let guard = state.lock().unwrap();
        assert_eq!(guard.bots.len(), 1);
        assert!(
            !["ace", "hyde"]
                .contains(&crate::identity::name_key(&guard.bots[0].player.name).as_str())
        );
    }
    let (c, _c) = client(&state);
    assert_eq!(
        admit_player_login(&state, c, "100000000003".into(), "Third".into()),
        Err(LoginAdmissionError::Full)
    );
    state.lock().unwrap().config.max_players = 500;
    state.lock().unwrap().config.bot_count = 43;
    state.lock().unwrap().initialize_configured_bots();
    let guard = state.lock().unwrap();
    let names: BTreeSet<_> = guard
        .bots
        .iter()
        .map(|b| crate::identity::name_key(&b.player.name))
        .collect();
    assert_eq!(names.len(), 43);
    assert!(!names.contains("ace") && !names.contains("hyde"));
}

#[test]
fn a_bot_name_is_reserved_even_while_the_bot_is_dead() {
    let mut initial = ServerState::new(ServerConfig {
        bot_count: 1,
        ..ServerConfig::default()
    });
    initial.initialize_configured_bots();
    // Use a valid human-entered name for the ownership collision. Many
    // intentionally preserved server bot display names contain spaces.
    initial.bots[0].player.name = "Ace".into();
    let name = initial.bots[0].player.name.to_ascii_uppercase();
    initial.bots[0].respawn_at_tick = Some(1000);
    assert!(!initial.bots[0].alive());
    let state = Arc::new(Mutex::new(initial));
    let (id, _rx) = client(&state);
    assert_eq!(
        admit_player_login(&state, id, "100000000001".into(), name),
        Err(LoginAdmissionError::NameInUse)
    );
}

#[test]
fn failed_zone_join_releases_reservation_and_rejoin_rechecks_new_owner() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (a, _a_rx) = admitted(&state, "100000000001", "Retry");
    let mut unknown = vec![0_u8; 10];
    unknown[0] = ARENA_LOGIN;
    unknown.extend_from_slice(b"missing-zone\0");
    assert!(crate::arena::join_arena_for_login(&state, a, 0, &unknown).is_err());
    assert!(!state.lock().unwrap().clients[&a].arena_admission_reserved);
    let (_b, _b_rx) = admitted(&state, "100000000002", "retry");
    assert_eq!(
        crate::arena::join_arena_for_login(&state, a, 0, &[ARENA_LOGIN, 0]),
        Err("ZUBSPACE_NAME_IN_USE")
    );
    assert!(!state.lock().unwrap().clients[&a].joined);
}

#[test]
fn committed_leave_releases_name_but_preserves_identity_stats_and_quality_hold() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (a, _a_rx) = admitted(&state, "100000000001", "LeaveAce");
    crate::arena::join_arena_for_login(&state, a, 0, &[ARENA_LOGIN, 0]).unwrap();
    {
        let mut guard = state.lock().unwrap();
        guard.clients.get_mut(&a).unwrap().player.kill_points = 73;
        guard
            .quality_holds
            .begin("100000000001", Instant::now())
            .unwrap();
    }
    crate::arena::leave_arena(&state, a);
    {
        let guard = state.lock().unwrap();
        assert!(!guard.clients[&a].joined && !guard.clients[&a].arena_admission_reserved);
        assert!(guard.quality_holds.get("100000000001").is_some());
    }
    let (b, _b_rx) = admitted(&state, "100000000002", "leaveace");
    assert_eq!(
        crate::arena::join_arena_for_login(&state, a, 0, &[ARENA_LOGIN, 0]),
        Err("ZUBSPACE_NAME_IN_USE")
    );
    disconnect_player(&state, b);
    crate::arena::join_arena_for_login(&state, a, 0, &[ARENA_LOGIN, 0]).unwrap();
    assert_eq!(
        state.lock().unwrap().clients[&a].player.ship,
        8,
        "the existing quality hold survives same-transport rejoin"
    );
    disconnect_player(&state, a);
    let (c, _c_rx) = admitted(&state, "100000000001", "LeaveAce");
    assert_eq!(state.lock().unwrap().clients[&c].player.kill_points, 73);
    assert!(
        state
            .lock()
            .unwrap()
            .quality_holds
            .get("100000000001")
            .is_some()
    );
}

#[test]
fn failed_bootstrap_sink_does_not_retain_an_arena_member_or_name() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (id, rx) = admitted(&state, "100000000001", "ClosedSink");
    drop(rx);
    assert!(crate::arena::join_arena_for_login(&state, id, 0, &[ARENA_LOGIN, 0]).is_err());
    {
        let guard = state.lock().unwrap();
        assert!(
            guard
                .clients
                .get(&id)
                .is_none_or(|c| !c.joined && !c.arena_admission_reserved)
        );
    }
    admitted(&state, "100000000002", "closedsink");
}

#[test]
fn successful_bootstrap_waiting_for_network_acks_keeps_its_slot_and_name() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (id, _rx) = admitted(&state, "100000000001", "WaitingAce");
    crate::arena::join_arena_for_login(&state, id, 0, &[ARENA_LOGIN, 0]).unwrap();
    {
        let guard = state.lock().unwrap();
        let client = &guard.clients[&id];
        assert!(client.joined && client.arena_admission_reserved && !client.snapshot_ready);
        assert!(
            client
                .bootstrap_reliable_barrier
                .as_ref()
                .unwrap()
                .initial_delivery_complete()
        );
    }
    let (other, _other_rx) = client(&state);
    assert_eq!(
        admit_player_login(&state, other, "100000000002".into(), "waitingace".into()),
        Err(LoginAdmissionError::NameInUse)
    );
}

#[test]
fn same_connection_login_retry_waits_for_old_session_and_preserves_identity_stats() {
    let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
        max_players: 1,
        ..ServerConfig::default()
    })));
    let (old, _old_rx) = admitted(&state, "100000000001", "Ace");
    let old_session = {
        let mut guard = state.lock().unwrap();
        guard.clients.get_mut(&old).unwrap().player.kill_points = 91;
        guard.clients.get_mut(&old).unwrap().player.wins = 3;
        guard.active_player_session_identity(old).unwrap()
    };
    let (new, _new_rx) = client(&state);
    for _ in 0..3 {
        assert_eq!(
            admit_player_login(&state, new, "100000000001".into(), "Ace".into()),
            Err(LoginAdmissionError::IdentityInUse)
        );
        let guard = state.lock().unwrap();
        assert_eq!(guard.clients[&old].player.kill_points, 91);
        assert!(!guard.clients[&new].arena_admission_reserved);
    }
    crate::arena::disconnect_player_session(&state, old_session);
    admit_player_login(&state, new, "100000000001".into(), "Ace".into()).unwrap();
    crate::arena::disconnect_player_session(&state, old_session);
    let guard = state.lock().unwrap();
    assert_eq!(guard.clients.len(), 1);
    assert!(guard.clients[&new].arena_admission_reserved);
    assert_eq!(guard.clients[&new].player.kill_points, 91);
    assert_eq!(guard.clients[&new].player.wins, 3);
    assert_eq!(reserved_human_count(&guard, None), 1);
}
