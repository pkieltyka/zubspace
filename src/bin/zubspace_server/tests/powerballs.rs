use super::*;

fn install_ball(
    state: &Arc<Mutex<ServerState>>,
    ball_state: PowerballState,
) -> crate::objectives::PowerballSnapshot {
    let mut locked = state.lock().expect("state");
    locked
        .objectives
        .reset(synthetic_profile(0, 1))
        .expect("synthetic objective generation should advance");
    locked
        .objectives
        .create_powerball(ball_state)
        .expect("synthetic powerball should fit profile")
}

#[test]
fn disabled_profile_rejects_all_powerball_requests_without_state_or_score() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let _beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);

    relay_ball_pickup(&state, alpha, &ball_pickup_request_payload(0, 1));
    relay_ball_fire(
        &state,
        alpha,
        &ball_fire_request_payload(0, 320, 320, 0, 0, alpha, 0),
    );
    relay_ball_goal(&state, alpha, &ball_goal_request_payload(0, 20, 20));
    relay_ball_pickup(&state, alpha, &[BALL_PICKUP_REQUEST, 0]);
    relay_ball_fire(&state, alpha, &[BALL_FIRE_REQUEST, 0]);
    relay_ball_goal(&state, alpha, &[BALL_GOAL_REQUEST, 0]);

    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
    let locked = state.lock().expect("state");
    assert!(locked.objectives.profile().is_disabled());
    assert!(locked.objectives.is_empty());
    assert!(crate::objectives::current_powerball_packets_for_state(&locked).is_empty());
}

#[test]
fn exact_pickup_is_inert_and_old_request_cannot_claim_replacement_generation() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let _beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    let original = install_ball(
        &state,
        PowerballState::OnMap {
            x: 320,
            y: 320,
            velocity_x: 0,
            velocity_y: 0,
            pickup_unlock_tick: None,
        },
    );
    let request = ball_pickup_request_payload(original.identity.id, original.version.get());

    relay_ball_pickup(&state, alpha, &request);
    relay_ball_pickup(
        &state,
        alpha,
        &ball_pickup_request_payload(original.identity.id, 0),
    );
    relay_ball_pickup(
        &state,
        alpha,
        &ball_pickup_request_payload(original.identity.id, 0x8000_0001),
    );
    assert_eq!(
        state
            .lock()
            .expect("state")
            .objectives
            .powerball(original.identity.id),
        Some(original)
    );

    let replacement = install_ball(
        &state,
        PowerballState::OnMap {
            x: 640,
            y: 640,
            velocity_x: 10,
            velocity_y: -10,
            pickup_unlock_tick: None,
        },
    );
    assert_eq!(replacement.identity.id, original.identity.id);
    assert_ne!(
        replacement.identity.generation,
        original.identity.generation
    );
    relay_ball_pickup(&state, alpha, &request);
    assert_eq!(
        state
            .lock()
            .expect("state")
            .objectives
            .powerball(replacement.identity.id),
        Some(replacement)
    );
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn fire_is_inert_across_duplicate_life_reuse_and_client_authored_motion() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);
    drain_available(&rx_a);
    drain_available(&rx_b);
    let runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(alpha)
        .expect("runtime");
    let ball = install_ball(&state, PowerballState::Carried { carrier: runtime });
    assert!(player_carries_powerball(
        &state.lock().expect("state"),
        runtime
    ));
    let fire = ball_fire_request_payload(
        ball.identity.id,
        u16::MAX,
        u16::MAX,
        i16::MAX,
        i16::MIN,
        alpha,
        0,
    );

    relay_ball_fire(&state, alpha, &fire);
    relay_ball_fire(&state, alpha, &fire);
    relay_ball_fire(
        &state,
        alpha,
        &ball_fire_request_payload(ball.identity.id, 1, 1, 0, 0, beta, 0),
    );
    assert_eq!(
        state
            .lock()
            .expect("state")
            .objectives
            .powerball(ball.identity.id),
        Some(ball)
    );

    state
        .lock()
        .expect("state")
        .advance_client_life(alpha)
        .expect("life should advance");
    let replacement_runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(alpha)
        .expect("replacement runtime");
    assert_ne!(replacement_runtime, runtime);
    assert!(
        !player_carries_powerball(&state.lock().expect("state"), replacement_runtime),
        "the reusable player id must not transfer an old-life carried ball"
    );
    relay_ball_fire(&state, alpha, &fire);
    relay_ball_goal(
        &state,
        alpha,
        &ball_goal_request_payload(ball.identity.id, 0, 0),
    );
    assert_eq!(
        state
            .lock()
            .expect("state")
            .objectives
            .powerball(ball.identity.id),
        Some(ball)
    );
    assert!(rx_a.try_recv().is_err());
    assert!(rx_b.try_recv().is_err());
}

#[test]
fn goal_is_inert_for_an_existing_carried_ball() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let alpha = register_connection(&state, tx);
    join_arena(&state, alpha, 0);
    drain_available(&rx);
    let runtime = state
        .lock()
        .expect("state")
        .active_player_runtime_identity(alpha)
        .expect("runtime");
    let ball = install_ball(&state, PowerballState::Carried { carrier: runtime });

    relay_ball_goal(
        &state,
        alpha,
        &ball_goal_request_payload(ball.identity.id, i16::MAX, i16::MIN),
    );
    assert_eq!(
        state
            .lock()
            .expect("state")
            .objectives
            .powerball(ball.identity.id),
        Some(ball)
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn direct_server_state_replays_to_late_joiner() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, rx_a) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    join_arena(&state, alpha, 0);
    drain_available(&rx_a);
    let ball = install_ball(
        &state,
        PowerballState::OnMap {
            x: 320,
            y: 480,
            velocity_x: 25,
            velocity_y: -50,
            pickup_unlock_tick: None,
        },
    );
    let expected =
        crate::objectives::current_powerball_packets_for_state(&state.lock().expect("state"));
    assert_eq!(expected.len(), 1);
    assert_eq!(expected[0][1], ball.identity.id);

    let (tx_b, rx_b) = mpsc::channel();
    let beta = register_connection(&state, tx_b);
    join_arena(&state, beta, 0);
    assert_eq!(
        read_reliable_payload_with_type(&rx_b, POWERBALL_POSITION),
        expected[0]
    );
}

#[test]
fn server_goal_position_validation_matches_reference_soccer_modes() {
    let scoring_goal = server_goal_position_for_frequency(1, true);
    let own_team_goal = server_goal_position_for_frequency(0, false);
    assert!(world_position_touches_goal(
        scoring_goal.0 as u16,
        scoring_goal.1 as u16
    ));
    assert!(world_position_touches_goal(
        own_team_goal.0 as u16,
        own_team_goal.1 as u16
    ));
    assert!(soccer_goal_counts_world(
        scoring_goal.0 as u16,
        scoring_goal.1 as u16,
        1,
        arena_soccer_mode()
    ));
    assert!(!soccer_goal_counts_world(
        own_team_goal.0 as u16,
        own_team_goal.1 as u16,
        0,
        arena_soccer_mode()
    ));
}
