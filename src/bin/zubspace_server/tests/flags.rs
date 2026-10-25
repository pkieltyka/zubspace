use super::*;
use crate::objective_protocol::ObjectiveStateUpdate;

#[test]
fn disabled_profile_rejects_unknown_and_malformed_flag_requests_without_replay() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let _beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    relay_flag_request(&state, alpha, &flag_request_payload(0));
    relay_flag_request(&state, alpha, &[FLAG_REQUEST, 0]);
    relay_flag_request(&state, alpha, &[FLAG_REQUEST, 0, 0, 0]);
    relay_flag_request(&state, alpha, &flag_request_payload(u16::MAX));
    relay_drop_flags(&state, alpha, &[DROP_FLAGS_REQUEST]);
    relay_drop_flags(&state, alpha, &[DROP_FLAGS_REQUEST, 0]);

    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    let locked = state.lock().expect("server state should not be poisoned");
    assert!(locked.objectives.profile().is_disabled());
    assert!(locked.objectives.is_empty());
    assert_eq!(locked.clients[&alpha].player.flags, 0);
    drop(locked);
    assert!(current_flag_packets(&state).is_empty());
}

#[test]
fn legacy_drop_distinguishes_a_closed_policy_from_a_carrier_holding_nothing() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    // The enabled half has to reach the carry test itself, so give the player
    // the position witness the objective engine requires. Without it both calls
    // would fail as `inactive_drop` and measure the player, not the profile.
    seed_client_position(&state, alpha, 640, 640);

    assert_eq!(
        legacy_flag_drop_packets(
            &mut state.lock().expect("state"),
            alpha,
            crate::wire::current_tick(),
        )
        .expect_err("a zone that authored no flags must stay policy-closed, not an empty carry"),
        "drop_policy_disabled",
    );

    // An authored zone with a flag on the map, and a player that never touched
    // it: the same call now has to report the empty carry.
    spawn_map_flag(&state, 640, 640);
    assert_eq!(
        legacy_flag_drop_packets(
            &mut state.lock().expect("state"),
            alpha,
            crate::wire::current_tick(),
        )
        .expect_err("an enabled profile with an idle carrier is the only empty-carry case"),
        "drop_nothing_carried",
    );
    assert!(rx_a.try_recv().is_err());
}

#[test]
fn legacy_claim_without_an_authoritative_position_cannot_mutate_the_flag() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let _beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    let original = {
        let mut locked = state.lock().expect("state");
        locked
            .objectives
            .reset(synthetic_profile(1, 0))
            .expect("synthetic objective generation should advance");
        locked
            .objectives
            .create_flag(FlagState::OnMap {
                x: 640,
                y: 640,
                owner: None,
            })
            .expect("synthetic flag should fit profile")
    };

    relay_flag_request(&state, alpha, &flag_request_payload(original.identity.id));
    relay_drop_flags(&state, alpha, &[DROP_FLAGS_REQUEST]);
    assert_eq!(
        state
            .lock()
            .expect("state")
            .objectives
            .flag(original.identity.id),
        Some(original)
    );
    assert_eq!(
        current_flag_packets(&state),
        vec![flag_position_packet(ZoneFlag {
            id: original.identity.id,
            x: 640,
            y: 640,
            owner: u16::MAX,
        })]
    );
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());

    let replacement = {
        let mut locked = state.lock().expect("state");
        locked
            .objectives
            .reset(synthetic_profile(1, 0))
            .expect("synthetic objective generation should advance");
        locked
            .objectives
            .create_flag(FlagState::OnMap {
                x: 800,
                y: 800,
                owner: None,
            })
            .expect("replacement flag should fit profile")
    };
    assert_eq!(replacement.identity.id, original.identity.id);
    assert_ne!(
        replacement.identity.generation,
        original.identity.generation
    );

    // A claim is evaluated against the server's own view. With no authoritative
    // position for this player, neither the original nor the indistinguishable
    // replayed request may touch the replacement generation.
    relay_flag_request(&state, alpha, &flag_request_payload(original.identity.id));
    assert_eq!(
        state
            .lock()
            .expect("state")
            .objectives
            .flag(replacement.identity.id),
        Some(replacement)
    );
    assert_eq!(state.lock().expect("state").clients[&alpha].player.flags, 0);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn carried_flag_replay_positions_identity_before_claim_for_late_joiner() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    let flag = {
        let mut locked = state.lock().expect("state");
        locked
            .objectives
            .reset(synthetic_profile(1, 0))
            .expect("synthetic objective generation should advance");
        let carrier = locked
            .active_player_runtime_identity(alpha)
            .expect("carrier runtime");
        locked
            .objectives
            .create_flag(FlagState::Carried {
                carrier,
                x: 704,
                y: 832,
                owner: Some(9),
            })
            .expect("synthetic carried flag should fit profile")
    };

    let (tx_b, rx_b) = mpsc::channel();
    let beta = register_connection(&state, tx_b);
    join_arena(&state, beta, 0);

    assert_eq!(
        read_reliable_payload_with_type(&rx_b, 0x12),
        flag_position_packet(ZoneFlag {
            id: flag.identity.id,
            x: 704,
            y: 832,
            owner: 9,
        })
    );
    assert_eq!(
        read_reliable_payload_with_type(&rx_b, FLAG_REQUEST),
        flag_claim_packet(flag.identity.id, alpha)
    );
}

/// Positions a joined client the way the position path would, so the objective
/// engine resolves it through the same fallback the live server uses.
fn seed_client_position(state: &Arc<Mutex<ServerState>>, player_id: u16, x: u16, y: u16) {
    let mut locked = state.lock().expect("state");
    let client = locked.clients.get_mut(&player_id).expect("joined client");
    client.alive = true;
    client.last_position_sequence = Some(1);
    client.last_position = Some(client_position_packet_with_sequence(0, x, y, 1));
}

fn spawn_map_flag(state: &Arc<Mutex<ServerState>>, x: u16, y: u16) -> u16 {
    let mut locked = state.lock().expect("state");
    locked
        .objectives
        .reset(synthetic_profile(1, 0))
        .expect("synthetic objective profile");
    locked
        .objectives
        .create_flag(FlagState::OnMap { x, y, owner: None })
        .expect("synthetic flag should fit profile")
        .identity
        .id
}

fn two_joined_peers() -> (
    Arc<Mutex<ServerState>>,
    u16,
    mpsc::Receiver<Vec<u8>>,
    u16,
    mpsc::Receiver<Vec<u8>>,
) {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    // Post-bootstrap reliable payloads wait on the join fence, which these
    // tests are not about.
    acknowledge_join_bootstrap(&state, alpha);
    acknowledge_join_bootstrap(&state, beta);
    drain_available(&rx_a);
    drain_available(&rx_b);
    (state, alpha, rx_a, beta, rx_b)
}

/// Reads the versioned objective-state broadcast that the mutation pipeline
/// publishes for every registry change.
fn flag_update(rx: &mpsc::Receiver<Vec<u8>>) -> ObjectiveStateUpdate {
    ObjectiveStateUpdate::parse(&read_reliable_payload_with_type(
        rx,
        ObjectiveStateUpdate::TYPE,
    ))
    .expect("versioned flag state update")
}

#[test]
fn legacy_claim_while_touching_the_flag_carries_it_and_broadcasts_the_claim() {
    let (state, alpha, rx_a, _beta, rx_b) = two_joined_peers();
    let flag_id = spawn_map_flag(&state, 640, 640);
    seed_client_position(&state, alpha, 640, 640);

    relay_flag_request(&state, alpha, &flag_request_payload(flag_id));

    let locked = state.lock().expect("state");
    let flag = locked.objectives.flag(flag_id).expect("flag remains live");
    assert!(
        matches!(flag.state, FlagState::Carried { carrier, .. } if carrier.player_id() == alpha),
        "the claimant's own hull must carry the flag: {:?}",
        flag.state
    );
    assert_eq!(locked.clients[&alpha].player.flags, 1);
    drop(locked);
    for rx in [&rx_a, &rx_b] {
        let update = flag_update(rx);
        assert_eq!(
            (
                update.kind,
                update.state,
                update.objective_id,
                update.owner_id,
                update.outcome,
            ),
            (
                ObjectiveStateUpdate::KIND_FLAG,
                ObjectiveStateUpdate::STATE_CARRIED,
                flag_id,
                Some(alpha),
                ObjectiveStateUpdate::OUTCOME_APPLIED,
            )
        );
    }
}

#[test]
fn legacy_claim_outside_the_ship_hull_is_rejected_without_mutation() {
    let (state, alpha, rx_a, _beta, rx_b) = two_joined_peers();
    let flag_id = spawn_map_flag(&state, 640, 640);
    // Synthetic claim radius is 10 px; this hull is 1_360 px away.
    seed_client_position(&state, alpha, 2_000, 2_000);

    relay_flag_request(&state, alpha, &flag_request_payload(flag_id));

    let locked = state.lock().expect("state");
    let flag = locked.objectives.flag(flag_id).expect("flag remains live");
    assert!(
        matches!(
            flag.state,
            FlagState::OnMap {
                x: 640,
                y: 640,
                owner: None
            }
        ),
        "a distant hull must not move the flag: {:?}",
        flag.state
    );
    assert_eq!(locked.clients[&alpha].player.flags, 0);
    drop(locked);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn legacy_replayed_claim_cannot_double_claim_a_carried_flag() {
    let (state, alpha, rx_a, _beta, rx_b) = two_joined_peers();
    let flag_id = spawn_map_flag(&state, 640, 640);
    seed_client_position(&state, alpha, 640, 640);
    let request = flag_request_payload(flag_id);

    relay_flag_request(&state, alpha, &request);
    drain_available(&rx_a);
    drain_available(&rx_b);

    // The legacy packet is indistinguishable from its own replay, so the engine's
    // current-state check is what stops it.
    relay_flag_request(&state, alpha, &request);

    let locked = state.lock().expect("state");
    let flag = locked.objectives.flag(flag_id).expect("flag remains live");
    assert!(
        matches!(flag.state, FlagState::Carried { carrier, .. } if carrier.player_id() == alpha),
        "a replayed claim must leave the single carrier: {:?}",
        flag.state
    );
    assert_eq!(locked.clients[&alpha].player.flags, 1);
    drop(locked);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn legacy_drop_releases_the_carried_flag_and_a_second_drop_is_inert() {
    let (state, alpha, rx_a, _beta, rx_b) = two_joined_peers();
    let flag_id = spawn_map_flag(&state, 640, 640);
    seed_client_position(&state, alpha, 640, 640);
    relay_flag_request(&state, alpha, &flag_request_payload(flag_id));
    drain_available(&rx_a);
    drain_available(&rx_b);

    relay_drop_flags(&state, alpha, &[DROP_FLAGS_REQUEST]);

    let locked = state.lock().expect("state");
    let flag = locked.objectives.flag(flag_id).expect("flag remains live");
    assert!(
        matches!(flag.state, FlagState::OnMap { owner: None, .. }),
        "the requested drop must return the flag to the map: {:?}",
        flag.state
    );
    assert_eq!(locked.clients[&alpha].player.flags, 0);
    drop(locked);
    let update = flag_update(&rx_b);
    assert_eq!(
        (update.kind, update.state, update.objective_id),
        (
            ObjectiveStateUpdate::KIND_FLAG,
            ObjectiveStateUpdate::STATE_ON_MAP,
            flag_id
        ),
        "the released flag must be re-advertised to peers"
    );
    drain_available(&rx_a);
    drain_available(&rx_b);

    relay_drop_flags(&state, alpha, &[DROP_FLAGS_REQUEST]);
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

/// The authored flag profile sets `FlagCarryTimeoutTicks = 0`, which the engine
/// honours as "no expiry" (`objectives/engine.rs:966-969`), so nothing but the
/// carrier's own lifecycle can hand a carried flag back to the arena.
fn spawn_map_flag_without_carry_expiry(state: &Arc<Mutex<ServerState>>, x: u16, y: u16) -> u16 {
    let mut profile = synthetic_profile(1, 0);
    match &mut profile.flags {
        ServerFlagProfile::Enabled(settings) => settings.carry_timeout_ticks = 0,
        _ => panic!("the synthetic flag profile must be enabled"),
    }
    let mut locked = state.lock().expect("server state should not be poisoned");
    locked
        .objectives
        .reset(profile)
        .expect("no-expiry flag profile should validate");
    locked
        .objectives
        .create_flag(FlagState::OnMap { x, y, owner: None })
        .expect("synthetic flag should fit profile")
        .identity
        .id
}

#[test]
fn a_disconnected_carrier_cannot_hold_the_flag_against_later_joiners() {
    let (state, alpha, rx_a, beta, rx_b) = two_joined_peers();
    let flag_id = spawn_map_flag_without_carry_expiry(&state, 640, 640);
    seed_client_position(&state, alpha, 640, 640);
    relay_flag_request(&state, alpha, &flag_request_payload(flag_id));
    let _alpha_session = state
        .lock()
        .expect("server state should not be poisoned")
        .active_player_session_identity(alpha)
        .expect("alpha session identity");
    drain_available(&rx_a);
    drain_available(&rx_b);

    // The authored reset delay is applied by the arena tick loop, so hand the
    // arena the ticks the ended session would otherwise have generated.
    for offset in 1..=4 {
        crate::server_tick::run_server_tick_at_tick(
            &state,
            Instant::now(),
            crate::continuum::continuum_tick_add(crate::wire::current_tick(), offset),
        );
    }
    let after_disconnect = state
        .lock()
        .expect("server state should not be poisoned")
        .objectives
        .flag(flag_id)
        .expect("the flag stays live across its carrier's session end")
        .state;
    let FlagState::OnMap { x, y, .. } = after_disconnect else {
        panic!(
            "the end of a carrier's session must hand the flag back to the arena: {after_disconnect:?}"
        );
    };

    // The authored policy re-seats a released flag at its spawn area, so the
    // surviving peer has to meet it where the arena put it.
    seed_client_position(&state, beta, x, y);
    relay_flag_request(&state, beta, &flag_request_payload(flag_id));
    let claimed = state
        .lock()
        .expect("server state should not be poisoned")
        .objectives
        .flag(flag_id)
        .expect("the flag stays live")
        .state;
    assert!(
        matches!(&claimed, FlagState::Carried { carrier, .. } if carrier.player_id() == beta),
        "a later claimant must be able to carry what the ended session released: {claimed:?}",
    );
}
