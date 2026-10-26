use super::*;

#[test]
fn roster_notices_hide_initial_list_then_announce_only_named_changes() {
    let mut game = Game::new_network_authority_test();
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(10),
        arena_stream_id: Some(20),
        player_id: Some(1),
        life_seq: None,
        alive: None,
    };
    let readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(40),
        settings: true,
        security: true,
        map: true,
    };
    game.install_network_authority_test_scope(scope, readiness, 40);
    let roster = |id, name: &str| PlayerRosterEntry {
        ship: 0,
        audio: 0,
        name: name.into(),
        squad: String::new(),
        kill_points: 0,
        flag_points: 0,
        player_id: id,
        frequency: id,
        wins: 0,
        losses: 0,
        attach_parent: INVALID_PLAYER_ID,
        flags: 0,
        koth: 0,
    };
    let apply = |game: &mut Game, events| {
        game.apply_network_actions(&ClientProtocolActions {
            events,
            outbound_packets: Vec::new(),
            authority_admission: Vec::new(),
        });
    };
    apply(
        &mut game,
        vec![
            ClientProtocolEvent::PlayerEntered(roster(6, "Stardust")),
            ClientProtocolEvent::PlayerEntered(roster(7, "Mad Hatter")),
        ],
    );
    assert!(game.notifications.is_empty(), "initial roster is silent");
    assert_eq!(game.network_world.players.len(), 2);
    game.client_authority.install_test_scope(
        0,
        AuthorityScopeIdentity {
            life_seq: Some(30),
            alive: Some(true),
            ..scope
        },
        ClientAuthorityReadiness {
            joined: true,
            arena_epoch: Some(40),
            settings: true,
            security: true,
            map: true,
            files: true,
        },
    );
    apply(
        &mut game,
        vec![
            ClientProtocolEvent::PlayerEntered(roster(6, "Stardust")),
            ClientProtocolEvent::PlayerEntered(roster(8, "Ace")),
            ClientProtocolEvent::PlayerEntered(roster(1, "self")),
            ClientProtocolEvent::PlayerLeft(7),
            ClientProtocolEvent::PlayerLeft(99),
            ClientProtocolEvent::PlayerEntered(roster(7, "Puck")),
        ],
    );
    let notices = game
        .notifications
        .iter()
        .map(|n| n.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        notices,
        [
            "Player Ace joined.",
            "Player Mad Hatter left.",
            "Player Puck joined."
        ]
    );
    assert_eq!(game.network_world.players[&7].name, "Puck");
}

#[test]
fn player_leave_notice_keeps_departing_name_before_roster_removal_and_id_reuse() {
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        AuthorityScopeIdentity {
            server_session_id: Some(10),
            arena_stream_id: Some(20),
            player_id: Some(1),
            life_seq: Some(30),
            alive: Some(true),
        },
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(40),
            settings: true,
            security: true,
            map: true,
        },
        40,
    );
    let roster = |name: &str| PlayerRosterEntry {
        ship: 0,
        audio: 0,
        name: name.into(),
        squad: String::new(),
        kill_points: 0,
        flag_points: 0,
        player_id: 6,
        frequency: 6,
        wins: 0,
        losses: 0,
        attach_parent: INVALID_PLAYER_ID,
        flags: 0,
        koth: 0,
    };
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::PlayerEntered(roster("Mad Hatter")),
            ClientProtocolEvent::PlayerLeft(6),
            ClientProtocolEvent::PlayerEntered(roster("Stardust")),
            ClientProtocolEvent::PlayerLeft(6),
            ClientProtocolEvent::PlayerLeft(99),
        ],
        authority_admission: Vec::new(),
    });
    let departures = game
        .notifications
        .iter()
        .filter(|notice| notice.message.ends_with(" left."))
        .map(|notice| notice.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        departures,
        ["Player Mad Hatter left.", "Player Stardust left.",]
    );
    assert!(!game.network_world.players.contains_key(&6));
}

fn complete_initial_join_sequencer_for_test(game: &mut Game) {
    let authority = game.client_authority.view();
    assert_eq!(
        authority.phase,
        ClientAuthorityPhase::AwaitingDurableSubmission,
        "fixture must supply the complete exact initial scope/readiness first"
    );
    let life_seq = authority
        .initial_baseline_life_seq
        .expect("initial lifecycle identity");
    assert_eq!(
        game.client_authority
            .record_initial_baseline_submitted(700, u16::MAX),
        ClientAuthorityDisposition::Applied
    );
    assert_eq!(
        game.client_authority
            .observe_initial_receipt(LifecycleBaselineAccepted {
                client_reliable_delivery_id: 700,
                life_seq,
                position_sequence: u16::MAX,
                accepted_server_tick: 1_000,
                accepted_position: None,
            }),
        InitialReceiptOutcome::Applied
    );
}

#[test]
fn private_prize_revision_accepts_duplicate_and_wrap_forward_but_rejects_reorder() {
    assert!(prize_revision_is_admissible(None, 9));
    assert!(prize_revision_is_admissible(Some(9), 9));
    assert!(prize_revision_is_admissible(Some(u32::MAX), 1));
    assert!(!prize_revision_is_admissible(Some(10), 9));
    assert!(!prize_revision_is_admissible(Some(1), 0));
}

#[test]
fn combat_view_receipt_advances_only_after_exact_game_applied_snapshot() {
    use crate::combat_timeline::{
        OBSERVER_DELAY_SLEW_INTERVAL_TICKS, OBSERVER_PRESENTATION_HOLD_GRACE_TICKS,
        ObserverNetworkProfile, ObserverTimelineDirectiveV1,
    };

    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(91),
        life_seq: Some(101),
        alive: Some(true),
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(111),
            settings: true,
            security: true,
            map: true,
        },
        111,
    );
    game.last_observed_browser_server_tick = Some(5_010);
    game.next_input_seq = 40;

    let envelope = ObserverNetworkProfile::CleanLan.delay_envelope();
    let directive = ObserverTimelineDirectiveV1 {
        generation: 12,
        issued_server_tick: 5_000,
        effective_server_tick: 5_000,
        effective_from_input_sequence: 40,
        from_delay_ticks: envelope.target_ticks,
        target_delay_ticks: envelope.target_ticks,
        ceiling_delay_ticks: envelope.ceiling_ticks,
        slew_interval_ticks: OBSERVER_DELAY_SLEW_INTERVAL_TICKS as u8,
        hold_grace_ticks: OBSERVER_PRESENTATION_HOLD_GRACE_TICKS,
        profile: ObserverNetworkProfile::CleanLan,
    };
    let snapshot = |snapshot_id, tick| {
        ClientProtocolEvent::SnapshotReceived(SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id,
                tick,
                arena_stream_id: 81,
                ..SnapshotHeader::default()
            },
            events: vec![SnapshotEvent::ObserverTimeline(directive)],
            ..SnapshotFrame::default()
        })
    };

    assert_eq!(
        game.apply_network_actions(&ClientProtocolActions {
            outbound_packets: Vec::new(),
            events: vec![snapshot(9, 5_005)],
            authority_admission: Vec::new(),
        }),
        vec![true]
    );
    let applied = game.combat_view_receipt_for_input(40);
    assert_eq!(applied.observer_life_seq, 101);
    assert_eq!(applied.timeline_generation, 12);
    assert_eq!(applied.latest_game_applied_snapshot_id, 9);

    // The carrier was decoded into a SnapshotReceived action, but its stale
    // delta-base id cannot cross NetworkWorld. The Game receipt must therefore
    // remain at the prior exact application rather than following codec input.
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![snapshot(8, 5_006)],
        authority_admission: Vec::new(),
    });
    let after_stale = game.combat_view_receipt_for_input(41);
    assert_eq!(after_stale.timeline_generation, 12);
    assert_eq!(after_stale.latest_game_applied_snapshot_id, 9);
    assert_eq!(
        game.network_world.authoritative_snapshot_server_tick,
        Some(5_005)
    );

    // The next local action captures snapshot 9 before the staged snapshot
    // lane advances Game to snapshot 10. Packet assembly must consume that
    // exact pre-action receipt rather than silently regenerating a different
    // view for the same logical input.
    let prepared = game.prepare_combat_view_receipt_for_input(42);
    assert_eq!(prepared.latest_game_applied_snapshot_id, 9);
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![snapshot(10, 5_007)],
        authority_admission: Vec::new(),
    });
    assert_eq!(
        game.network_world.authoritative_snapshot_server_tick,
        Some(5_007)
    );
    let transmitted = game.take_combat_view_receipt_for_input(42);
    assert_eq!(transmitted, prepared);
    assert_eq!(
        game.combat_view_receipt_for_input(43)
            .latest_game_applied_snapshot_id,
        10
    );

    // A semantic weapon edge is sampled at the render boundary even when a
    // high-refresh frame executes no fixed step. Freeze snapshot 10 there;
    // the later logical step must not silently bind Q to snapshot 11.
    game.next_input_seq = 44;
    let weapon_input = RenderInputContext::from_keys([KeyCode::Q], [KeyCode::Q]);
    game.prepare_pending_semantic_weapon_receipt(&weapon_input, 0);
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![snapshot(11, 5_008)],
        authority_admission: Vec::new(),
    });
    let captured_at_render_boundary = game.take_combat_view_receipt_for_input(44);
    assert_eq!(
        captured_at_render_boundary.latest_game_applied_snapshot_id,
        10
    );

    // Rocket is a lifecycle/movement pulse rather than a combat-view-gated
    // projectile. It must not leave a stale prepared receipt behind.
    game.next_input_seq = 45;
    let rocket_input = RenderInputContext::from_keys([KeyCode::F3], [KeyCode::F3]);
    game.prepare_pending_semantic_weapon_receipt(&rocket_input, 0);
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![snapshot(12, 5_009)],
        authority_admission: Vec::new(),
    });
    assert_eq!(
        game.take_combat_view_receipt_for_input(45)
            .latest_game_applied_snapshot_id,
        12
    );

    // A slow render frame drains semantic edges only on the final catch-up
    // step. Reserve the drawn receipt for that exact future sequence while
    // earlier movement-only records receive current receipts and cannot
    // consume or replace it.
    game.next_input_seq = 46;
    let burst_input =
        RenderInputContext::from_keys([KeyCode::LeftShift, KeyCode::Delete], [KeyCode::Delete]);
    game.prepare_pending_semantic_weapon_receipt(&burst_input, 0);
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![snapshot(13, 5_009)],
        authority_admission: Vec::new(),
    });
    game.prepare_pending_semantic_weapon_receipt(&burst_input, 3);
    let catch_up_first = game.take_combat_view_receipt_for_input(46);
    let catch_up_second = game.take_combat_view_receipt_for_input(47);
    let catch_up_action = game.take_combat_view_receipt_for_input(48);
    assert_eq!(catch_up_first.input_sequence, 46);
    assert_eq!(catch_up_first.latest_game_applied_snapshot_id, 13);
    assert_eq!(catch_up_second.input_sequence, 47);
    assert_eq!(catch_up_second.latest_game_applied_snapshot_id, 13);
    assert_eq!(catch_up_action.input_sequence, 48);
    assert_eq!(catch_up_action.latest_game_applied_snapshot_id, 12);

    // The future reservation comparison is wrap-safe across the u16 input
    // sequence boundary used by long-lived browser sessions.
    game.next_input_seq = u16::MAX - 1;
    game.prepare_pending_semantic_weapon_receipt(&weapon_input, 3);
    assert_eq!(
        game.take_combat_view_receipt_for_input(u16::MAX - 1)
            .input_sequence,
        u16::MAX - 1
    );
    assert_eq!(
        game.take_combat_view_receipt_for_input(u16::MAX)
            .input_sequence,
        u16::MAX
    );
    assert_eq!(game.take_combat_view_receipt_for_input(0).input_sequence, 0);

    // Browser key events are delivered between animation callbacks against
    // the frame drawn by the preceding iteration. Freeze that press-time view
    // before the next inbound poll/presentation update: a forward clock
    // correction may legitimately put the upcoming frame into recovery, but
    // it cannot retroactively turn the already-observed physical edge into a
    // held-view edge.
    game.next_input_seq = 100;
    game.last_observed_browser_server_tick = Some(5_010);
    game.update_remote_presentation(1.0 / 60.0);
    let decoy_input = RenderInputContext::from_keys([KeyCode::F5], [KeyCode::F5]);
    game.prepare_pending_semantic_weapon_receipt(&decoy_input, 0);
    let drawn_boundary = game
        .prepared_combat_view_receipt
        .expect("press-time drawn receipt")
        .receipt;
    assert!(drawn_boundary.weapon_submission_ready());

    game.last_observed_browser_server_tick = Some(5_014);
    game.update_remote_presentation(0.009);
    assert!(
        game.observer_combat_timeline.input_hold_or_stall_at(5_014),
        "the upcoming frame should be recovering from the clock correction"
    );
    game.prepare_pending_semantic_weapon_receipt(&decoy_input, 1);
    assert_eq!(
        game.take_combat_view_receipt_for_input(100),
        drawn_boundary,
        "the edge must retain the preceding actually drawn view"
    );

    // The same boundary is fail-closed across a hard authority replacement.
    // It cannot be regenerated with a successor life merely because inbound
    // lifecycle work completed before the logical step consumed the edge.
    game.update_remote_presentation(0.025);
    game.next_input_seq = 101;
    game.prepare_pending_semantic_weapon_receipt(&decoy_input, 0);
    assert!(
        game.prepared_combat_view_receipt
            .expect("replacement boundary")
            .receipt
            .weapon_submission_ready()
    );
    let authority = game.client_authority.view();
    game.client_authority.install_test_scope(
        authority.segment + 1,
        AuthorityScopeIdentity {
            life_seq: Some(102),
            ..authority.scope
        },
        authority.readiness,
    );
    game.prepare_pending_semantic_weapon_receipt(&decoy_input, 1);
    let replacement = game.take_combat_view_receipt_for_input(101);
    assert!(!replacement.weapon_submission_ready());
    assert_eq!(replacement.observer_life_seq, 0);
    assert_eq!(replacement.timeline_generation, 0);
}

fn portal_reconciliation_game(portals_after_prediction: u32) -> (Game, PortalPlacementRequest) {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(91),
        life_seq: Some(101),
        alive: Some(true),
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(111),
            settings: true,
            security: true,
            map: true,
        },
        111,
    );
    game.network_prize_baseline_pending = false;
    game.last_self_server_prize_life_seq = scope.life_seq;
    game.last_self_server_prize_revision = Some(10);
    game.last_self_server_portals = Some((101, 10, 2));
    game.ship.portals = portals_after_prediction;
    let request = PortalPlacementRequest {
        server_session_id: 71,
        arena_stream_id: 81,
        player_id: 91,
        life_seq: 101,
        action_id: u32::MAX,
        position_sequence: u16::MAX,
    };
    game.pending_portal_placements
        .push_back(ClientPortalPlacement { request });
    (game, request)
}

/// A wormhole relocation changes the server's energy by 80% and the client caused none of it, so the current energy
/// now carried by the per-recipient prize/state authority is the only way its prediction can learn the new value.
/// Before that field existed the client predicted straight through the warp: in one measured run the server signed
/// a relocation at 330 (`retain_fraction(0.2, 1.0)` of a full 1650) while the client's own energy read 911.25, and
/// the E2E authority check refuses anything at or above half the pre-warp energy.
#[test]
fn authoritative_energy_replaces_a_prediction_the_client_did_not_cause() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(91),
        life_seq: Some(101),
        alive: Some(true),
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
        },
        81,
    );
    game.network_prize_baseline_pending = false;
    game.last_self_server_prize_life_seq = scope.life_seq;
    game.last_self_server_prize_revision = Some(9);

    // What the pilot's own prediction holds after flying into a map wormhole.
    game.ship.max_energy = 1_650.0;
    game.ship.energy = 911.25;
    game.last_self_server_energy = Some(911.25);

    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![private_prize_snapshot(
            12,
            81,
            SnapshotPrizeState {
                life_seq: 101,
                // The relocation is what bumps this revision, which is how the new energy gets published promptly.
                revision: 11,
                max_energy: 1_650,
                energy: Some(330),
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: 101,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        )],
        authority_admission: Vec::new(),
    });

    assert_eq!(
        game.ship.energy, 330.0,
        "the prediction must adopt the server's post-relocation energy"
    );
    assert_eq!(
        game.last_self_server_energy,
        Some(330.0),
        "and the authoritative baseline the HUD blends with must move with it"
    );
}

#[test]
fn each_accepted_snapshot_sets_the_inputs_the_server_applied() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(91),
        life_seq: Some(101),
        alive: Some(true),
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
        },
        81,
    );
    let snapshot = |snapshot_id: u16, applied: Option<u16>| {
        ClientProtocolEvent::SnapshotReceived(SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id,
                tick: 4_900,
                arena_stream_id: 81,
                ..SnapshotHeader::default()
            },
            events: applied
                .map(|through_sequence| {
                    SnapshotEvent::InputApplied(crate::protocol::SnapshotInputApplied {
                        life_seq: 101,
                        through_sequence,
                    })
                })
                .into_iter()
                .collect(),
            ..SnapshotFrame::default()
        })
    };
    for (snapshot_id, applied) in [(12, Some(40)), (13, Some(39)), (14, None)] {
        game.apply_network_actions(&ClientProtocolActions {
            outbound_packets: Vec::new(),
            events: vec![snapshot(snapshot_id, applied)],
            authority_admission: Vec::new(),
        });
        // The newest frame wins, even moving back after a server rollback,
        // and a frame without it restores whole windows.
        assert_eq!(game.server_input_applied_through, applied);
    }
}

#[test]
fn movement_prize_snapshot_waits_for_reliable_ack_boundary_before_changing_simulation() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(91),
        life_seq: Some(101),
        alive: Some(true),
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
        },
        81,
    );
    game.network_prize_baseline_pending = false;
    game.last_self_server_prize_life_seq = scope.life_seq;
    game.last_self_server_prize_revision = Some(9);
    let tuning = game.ship_tuning();
    let before = tuning.rotation_per_second;
    let after = stat_value_after_prize(
        before,
        tuning.rotation_per_second,
        tuning.rotation_upgrade_step,
        tuning.rotation_max_per_second,
        false,
    );
    game.ship.rotation_per_second = before;

    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![private_prize_snapshot(
            12,
            81,
            SnapshotPrizeState {
                life_seq: 101,
                revision: 10,
                flags: 1 << 12,
                rotation_bits: after.to_bits(),
                thrust_bits: game.ship.thrust_acceleration.to_bits(),
                top_speed_bits: game.ship.top_speed.to_bits(),
                recharge_bits: game.ship.recharge_per_second.to_bits(),
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: 101,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        )],
        authority_admission: Vec::new(),
    });
    assert_eq!(game.last_self_server_prize_revision, Some(10));
    assert_eq!(
        game.ship.rotation_per_second.to_bits(),
        before.to_bits(),
        "overtaking PrizeState cannot move the local simulator before the exact ACK boundary"
    );

    let ack = PrizePickupAck {
        server_session_id: 71,
        arena_stream_id: 81,
        player_id: 91,
        collectible_id: 0x4455,
        x: 800,
        y: 800,
        prize_id: PrizeKind::Rotation.protocol_id(),
        authoritative_energy: 1_700,
        life_seq: 101,
        prize_state_revision: 10,
        engine_shutdown_until_tick: None,
        engine_shutdown_application_required: true,
    };
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::PrizePickupAck(
            scoped_prize_pickup_ack(ack, 71, 81, 91),
        )],
        authority_admission: Vec::new(),
    });
    assert_eq!(game.ship.rotation_per_second.to_bits(), after.to_bits());
    assert_eq!(game.prize_pickup_notification_count, 1);
    assert_eq!(
        game.last_prize_pickup_notification.as_deref(),
        Some("Rotation speed increased.")
    );
    assert_eq!(
        game.notifications
            .iter()
            .filter(|entry| entry.message == "Rotation speed increased.")
            .count(),
        1,
        "an overtaking PrizeState cannot consume the exact pickup presentation"
    );
    let application = game
        .client_authority
        .pending_prize_effect_application()
        .expect("movement application retained")
        .application;
    assert_eq!(application.prize_id, PrizeKind::Rotation.protocol_id());

    for offset in (1..=4).rev() {
        let sequence = application.application_sequence.wrapping_sub(offset);
        game.outbound_input_ring.record(
            sequence,
            PlayerInputRecord {
                bits: 1,
                direction: 7,
                door_seed: 238,
                combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(
                    sequence,
                ),
            },
        );
    }
    assert_eq!(game.outbound_input_ring.len(), 4);

    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::PrizePickupAck(
            scoped_prize_pickup_ack(ack, 71, 81, 91),
        )],
        authority_admission: Vec::new(),
    });
    assert_eq!(game.prize_pickup_notification_count, 1);
    assert_eq!(
        game.notifications
            .iter()
            .filter(|entry| entry.message == "Rotation speed increased.")
            .count(),
        1,
        "a reliable duplicate cannot replay text or sound"
    );

    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::PrizeEffectAccepted(
            PrizeEffectAccepted::from(application),
        )],
        authority_admission: Vec::new(),
    });
    assert_eq!(
        game.client_authority
            .view()
            .prize_effect_application_barriers,
        0
    );
    assert_eq!(
        game.outbound_input_ring.len(),
        0,
        "the exact movement-prize terminal retires all predecessor-semantics redundancy",
    );
    assert_eq!(game.ship.rotation_per_second.to_bits(), after.to_bits());
}

fn portal_terminal(
    request: PortalPlacementRequest,
    revision: u32,
    portals: u8,
    outcome: u8,
) -> ClientProtocolActions {
    portal_terminal_at(
        request,
        revision,
        portals,
        outcome,
        vec2(4_096.25, 8_192.5),
        100,
        2_100,
    )
}

#[allow(clippy::too_many_arguments)]
fn portal_terminal_at(
    request: PortalPlacementRequest,
    revision: u32,
    portals: u8,
    outcome: u8,
    position: Vec2,
    placed_at_tick: u32,
    expires_at_tick: u32,
) -> ClientProtocolActions {
    ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::PortalPlacementAccepted(
            PortalPlacementAccepted {
                server_session_id: request.server_session_id,
                arena_stream_id: request.arena_stream_id,
                player_id: request.player_id,
                life_seq: request.life_seq,
                action_id: request.action_id,
                position_sequence: request.position_sequence,
                prize_state_revision: revision,
                portals,
                outcome,
                position_x_bits: position.x.to_bits(),
                position_y_bits: position.y.to_bits(),
                placed_at_tick,
                expires_at_tick,
            },
        )],
        authority_admission: Vec::new(),
    }
}

#[test]
fn portal_prediction_survives_same_revision_snapshot_then_converges_on_terminal() {
    let (mut game, request) = portal_reconciliation_game(1);
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![private_prize_snapshot(
            12,
            81,
            SnapshotPrizeState {
                life_seq: 101,
                revision: 10,
                portals: 2,
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: 101,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        )],
        authority_admission: Vec::new(),
    });
    assert_eq!(
        game.ship.portals, 1,
        "snapshot cannot resurrect a pending spend"
    );
    assert_eq!(game.pending_portal_placements.len(), 1);

    let terminal = portal_terminal(request, 11, 1, PortalPlacementAccepted::OUTCOME_APPLIED);
    game.apply_network_actions(&terminal);
    game.apply_network_actions(&terminal);
    assert!(game.pending_portal_placements.is_empty());
    assert_eq!(game.ship.portals, 1);
    assert_eq!(game.last_self_server_prize_revision, Some(11));
    assert_eq!(game.last_self_server_portals, Some((101, 11, 1)));
    let portal = game
        .portal
        .expect("accepted prediction converges to server endpoint");
    assert_eq!(portal.pos, vec2(4_096.25, 8_192.5));
    assert_eq!(portal.ttl, 20.0);
}

#[test]
fn newer_prize_snapshot_cannot_hide_matching_applied_portal_terminal() {
    let (mut game, request) = portal_reconciliation_game(1);
    game.last_observed_browser_server_tick = Some(150);
    game.door_clock_fresh = true;

    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![private_prize_snapshot(
            12,
            request.arena_stream_id,
            SnapshotPrizeState {
                life_seq: request.life_seq,
                revision: 12,
                portals: 1,
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: request.life_seq,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        )],
        authority_admission: Vec::new(),
    });
    assert_eq!(game.last_self_server_prize_revision, Some(12));

    game.apply_network_actions(&portal_terminal(
        request,
        11,
        1,
        PortalPlacementAccepted::OUTCOME_APPLIED,
    ));

    let portal = game
        .active_portal()
        .expect("the matching terminal independently installs its endpoint");
    assert_eq!(portal.pos, vec2(4_096.25, 8_192.5));
    assert_eq!(portal.server_action_id, Some(request.action_id));
    assert_eq!(game.last_self_server_prize_revision, Some(12));
    assert_eq!(
        game.last_self_server_portals,
        Some((request.life_seq, 12, 1))
    );
}

fn portal_warp_reconciliation_game() -> (Game, PortalWarpRequest) {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(91),
        life_seq: Some(101),
        alive: Some(true),
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(111),
            settings: true,
            security: true,
            map: true,
        },
        111,
    );
    game.network_prize_baseline_pending = false;
    game.ship.pos = vec2(1_000.0, 2_000.0);
    game.ship.vel = vec2(12.0, -34.0);
    game.ship.orientation = 0.25;
    game.ship.energy = 500.0;
    game.portal = Some(Portal {
        pos: vec2(4_096.0, 8_192.0),
        ttl: 10.0,
        age: 0.0,
        server_action_id: Some(u32::MAX - 1),
        server_placed_at_tick: Some(100),
        server_expires_at_tick: Some(1_100),
    });
    game.portal_endpoint_authority = Some(PortalEndpointAuthority::Active(u32::MAX - 1));
    let request = PortalWarpRequest {
        force_random: false,
        server_session_id: 71,
        arena_stream_id: 81,
        player_id: 91,
        life_seq: 101,
        action_id: u32::MAX,
        position_sequence: u16::MAX,
    };
    assert_eq!(
        game.client_authority.arm_portal_warp(request, 1),
        ClientAuthorityDisposition::Applied
    );
    (game, request)
}

fn portal_warp_terminal(
    request: PortalWarpRequest,
    outcome: u8,
    used_portal: bool,
) -> ClientProtocolActions {
    ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::PortalWarpAccepted(
            PortalWarpAccepted {
                server_session_id: request.server_session_id,
                arena_stream_id: request.arena_stream_id,
                player_id: request.player_id,
                life_seq: request.life_seq,
                action_id: request.action_id,
                position_sequence: request.position_sequence,
                server_tick: 200,
                position_x_bits: 4_000.25_f32.to_bits(),
                position_y_bits: 5_000.5_f32.to_bits(),
                velocity_x_bits: 123.5_f32.to_bits(),
                velocity_y_bits: (-45.25_f32).to_bits(),
                orientation_bits: 0.375_f32.to_bits(),
                energy: 321,
                used_portal,
                outcome,
            },
        )],
        authority_admission: Vec::new(),
    }
}

#[test]
fn portal_warp_terminal_applies_exact_server_rebase_once_and_unfreezes_gameplay() {
    let (mut game, request) = portal_warp_reconciliation_game();
    assert!(!game.local_gameplay_ready());
    let terminal = portal_warp_terminal(request, PortalWarpAccepted::OUTCOME_APPLIED, true);
    game.apply_network_actions(&terminal);
    assert!(!game.client_authority.view().portal_warp_barrier);
    assert!(game.local_gameplay_ready());
    assert_eq!(game.ship.pos, vec2(4_000.25, 5_000.5));
    assert_eq!(game.ship.vel, vec2(123.5, -45.25));
    assert_eq!(game.ship.orientation, 0.375);
    assert_eq!(game.ship.energy, 321.0);
    assert!(game.portal.is_none());
    assert!(game.pending_same_life_movement_boundary_position);
    assert!(game.suppress_gameplay_step_after_network_rebase);
    assert_eq!(game.warps.len(), 2);

    game.apply_network_actions(&terminal);
    assert_eq!(game.warps.len(), 2, "a replayed terminal is inert");
}

#[test]
fn rejected_or_wrong_scope_portal_warp_cannot_mutate_canonical_client_state() {
    let (mut game, request) = portal_warp_reconciliation_game();
    let original = (game.ship.pos, game.ship.vel, game.ship.energy);
    let original_portal = game
        .portal
        .as_ref()
        .map(|portal| (portal.pos, portal.ttl, portal.age));
    let mut forged = request;
    forged.life_seq = forged.life_seq.wrapping_add(1);
    game.apply_network_actions(&portal_warp_terminal(
        forged,
        PortalWarpAccepted::OUTCOME_APPLIED,
        true,
    ));
    assert!(game.client_authority.view().portal_warp_barrier);
    assert_eq!((game.ship.pos, game.ship.vel, game.ship.energy), original);
    assert_eq!(
        game.portal
            .as_ref()
            .map(|portal| (portal.pos, portal.ttl, portal.age)),
        original_portal,
    );

    game.apply_network_actions(&portal_warp_terminal(
        request,
        PortalWarpAccepted::OUTCOME_REJECTED,
        false,
    ));
    assert!(!game.client_authority.view().portal_warp_barrier);
    assert_eq!((game.ship.pos, game.ship.vel, game.ship.energy), original);
    assert_eq!(
        game.portal
            .as_ref()
            .map(|portal| (portal.pos, portal.ttl, portal.age)),
        original_portal,
    );
}

#[test]
fn portal_rejection_refunds_prediction_exactly_once() {
    let (mut game, request) = portal_reconciliation_game(1);
    let terminal = portal_terminal(request, 10, 2, PortalPlacementAccepted::OUTCOME_REJECTED);
    game.apply_network_actions(&terminal);
    game.apply_network_actions(&terminal);
    assert!(game.pending_portal_placements.is_empty());
    assert_eq!(game.ship.portals, 2);
    assert_eq!(game.last_self_server_portals, Some((101, 10, 2)));
}

#[test]
fn rejected_portal_replacement_preserves_existing_authoritative_endpoint() {
    let (mut game, mut request) = portal_reconciliation_game(1);
    request.action_id = 6;
    request.position_sequence = 6;
    game.pending_portal_placements.clear();
    game.pending_portal_placements
        .push_back(ClientPortalPlacement { request });
    game.last_observed_browser_server_tick = Some(150);
    assert!(game.install_authoritative_portal(5, vec2(1_024.0, 2_048.0), 100, 2_100,));

    game.apply_network_actions(&portal_terminal_at(
        request,
        11,
        2,
        PortalPlacementAccepted::OUTCOME_REJECTED,
        vec2(9_000.0, 9_000.0),
        151,
        2_151,
    ));

    let portal = game
        .active_portal()
        .expect("a rejected replacement keeps the existing endpoint");
    assert_eq!(portal.pos, vec2(1_024.0, 2_048.0));
    assert_eq!(portal.server_action_id, Some(5));
    assert_eq!(
        game.portal_endpoint_authority,
        Some(PortalEndpointAuthority::Active(5))
    );
}

#[test]
fn newer_portal_placement_terminal_before_older_terminal_cannot_roll_back_endpoint() {
    let (mut game, mut older) = portal_reconciliation_game(0);
    older.action_id = 10;
    older.position_sequence = 10;
    let newer = PortalPlacementRequest {
        action_id: 11,
        position_sequence: 11,
        ..older
    };
    game.pending_portal_placements.clear();
    game.pending_portal_placements
        .push_back(ClientPortalPlacement { request: older });
    game.pending_portal_placements
        .push_back(ClientPortalPlacement { request: newer });
    game.last_observed_browser_server_tick = Some(150);

    game.apply_network_actions(&portal_terminal_at(
        newer,
        12,
        0,
        PortalPlacementAccepted::OUTCOME_APPLIED,
        vec2(3_000.0, 4_000.0),
        120,
        2_120,
    ));
    game.apply_network_actions(&portal_terminal_at(
        older,
        11,
        1,
        PortalPlacementAccepted::OUTCOME_APPLIED,
        vec2(1_000.0, 2_000.0),
        110,
        2_110,
    ));

    let portal = game
        .active_portal()
        .expect("newer endpoint remains authoritative");
    assert_eq!(portal.pos, vec2(3_000.0, 4_000.0));
    assert_eq!(portal.server_action_id, Some(11));
    assert_eq!(game.last_self_server_prize_revision, Some(12));
    assert_eq!(game.last_self_server_portals, Some((101, 12, 0)));
}

#[test]
fn portal_warp_terminal_before_older_placement_leaves_consumption_tombstone() {
    let (mut game, mut placement) = portal_reconciliation_game(0);
    placement.action_id = 20;
    placement.position_sequence = 20;
    game.pending_portal_placements.clear();
    game.pending_portal_placements
        .push_back(ClientPortalPlacement { request: placement });
    let warp = PortalWarpRequest {
        force_random: false,
        server_session_id: placement.server_session_id,
        arena_stream_id: placement.arena_stream_id,
        player_id: placement.player_id,
        life_seq: placement.life_seq,
        action_id: 21,
        position_sequence: 21,
    };
    assert_eq!(
        game.client_authority.arm_portal_warp(warp, 2),
        ClientAuthorityDisposition::Applied
    );
    game.last_observed_browser_server_tick = Some(200);

    game.apply_network_actions(&portal_warp_terminal(
        warp,
        PortalWarpAccepted::OUTCOME_APPLIED,
        true,
    ));
    game.apply_network_actions(&portal_terminal_at(
        placement,
        11,
        0,
        PortalPlacementAccepted::OUTCOME_APPLIED,
        vec2(7_000.0, 8_000.0),
        100,
        2_100,
    ));

    assert!(game.portal.is_none());
    assert!(game.active_portal().is_none());
    assert_eq!(
        game.portal_endpoint_authority,
        Some(PortalEndpointAuthority::Consumed(21))
    );
}

#[test]
fn portal_pending_and_terminal_cannot_cross_disconnect_cutover() {
    let (mut game, request) = portal_reconciliation_game(1);
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::Disconnected],
        authority_admission: Vec::new(),
    });
    assert!(game.pending_portal_placements.is_empty());
    assert_eq!(game.last_self_server_portals, None);

    game.apply_network_actions(&portal_terminal(
        request,
        11,
        1,
        PortalPlacementAccepted::OUTCOME_APPLIED,
    ));
    assert_eq!(game.last_self_server_portals, None);
    assert_eq!(game.ship.portals, 0);
}

#[test]
fn authoritative_death_invalidates_pending_portal_before_a_terminal_can_arrive() {
    let (mut game, request) = portal_reconciliation_game(1);
    let replacement_life = request.life_seq.wrapping_add(1).max(1);
    game.portal = Some(Portal {
        pos: vec2(100.0, 200.0),
        ttl: 10.0,
        age: 0.0,
        server_action_id: Some(request.action_id.wrapping_sub(1)),
        server_placed_at_tick: Some(100),
        server_expires_at_tick: Some(1_100),
    });
    game.portal_endpoint_authority = Some(PortalEndpointAuthority::Active(
        request.action_id.wrapping_sub(1),
    ));
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::PlayerDeath(PlayerDeath {
            green_id: -1,
            green_collectible_id: 0,
            killer_id: 92,
            killed_id: request.player_id,
            bounty: 10,
            flag_transfer: 0,
            life_seq: replacement_life,
            respawn_at_tick: 500,
        })],
        authority_admission: Vec::new(),
    });

    assert!(game.pending_portal_placements.is_empty());
    assert_eq!(game.last_self_server_portals, None);
    assert!(game.portal.is_none());
    assert_eq!(game.client_authority.view().scope.alive, Some(false));

    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![private_prize_snapshot(
            12,
            request.arena_stream_id,
            SnapshotPrizeState {
                life_seq: replacement_life,
                revision: 1,
                portals: 3,
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: replacement_life,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        )],
        authority_admission: Vec::new(),
    });
    assert_eq!(
        game.ship.portals, 3,
        "new-life snapshot is no longer blocked"
    );
    assert_eq!(
        game.last_self_server_portals,
        Some((replacement_life, 1, 3))
    );
}

#[test]
fn zero_delay_respawn_clears_portal_transaction_before_arming_replacement_input() {
    let (mut game, _) = portal_reconciliation_game(1);
    game.portal = Some(Portal {
        pos: vec2(100.0, 200.0),
        ttl: 10.0,
        age: 0.0,
        server_action_id: Some(u32::MAX - 1),
        server_placed_at_tick: Some(100),
        server_expires_at_tick: Some(1_100),
    });
    game.portal_endpoint_authority = Some(PortalEndpointAuthority::Active(u32::MAX - 1));

    let mut dead_scope = game.client_authority.view().scope;
    dead_scope.life_seq = Some(dead_scope.life_seq.unwrap().wrapping_add(1).max(1));
    dead_scope.alive = Some(false);
    assert_eq!(
        game.client_authority.observe_private_lifecycle(
            ClientAuthoritySource::Event,
            dead_scope,
            Some(100),
            0.0,
        ),
        ClientAuthorityDisposition::Replacement
    );

    game.start_player_respawn_with_delay(vec2(300.0, 400.0), 0.0);

    assert!(game.pending_portal_placements.is_empty());
    assert_eq!(game.last_self_server_portals, None);
    assert!(game.portal.is_none());
    assert_eq!(
        game.client_authority.view().transition_kind,
        Some(ClientAuthorityTransitionKind::Respawn)
    );
    assert!(
        game.client_authority
            .view()
            .lifecycle_baseline_ready_to_submit
    );
    assert_eq!(game.client_authority.view().scope.alive, Some(false));
}

#[test]
fn initial_arena_life_arms_neutral_baseline_and_binds_first_lifecycle() {
    let mut game = Game::new_network_authority_test();
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
                server_session_id: 41,
                arena_stream_id: 51,
                input_sequence_base: u16::MAX,
                ..ServerClockBootstrap::default()
            }),
            ClientProtocolEvent::PlayerIdAssigned(7),
            ClientProtocolEvent::JoinedArena(61),
        ],
        authority_admission: Vec::new(),
    });
    let authority = game.client_authority.view();
    assert!(authority.initial_baseline_pending);
    assert_eq!(authority.initial_baseline_life_seq, None);

    assert_eq!(
        game.apply_authoritative_lifecycle_state(SnapshotLifecycleState {
            life_seq: 23,
            alive: true,
            respawn_at_tick: None,
            repel_until_tick: None,
            repel_delivery: None,
        }),
        LifecycleAction::Observe,
    );
    let authority = game.client_authority.view();
    assert_eq!(authority.initial_baseline_life_seq, Some(23));
    assert!(authority.initial_baseline_pending);
}

#[test]
fn initial_join_exact_receipt_is_terminal_and_same_life_snapshot_cannot_rearm_it() {
    let session = 41;
    let stream = 51;
    let player = 7;
    let life = 23;
    let epoch = 61;
    let lifecycle_snapshot = || {
        private_prize_snapshot(
            1,
            stream,
            SnapshotPrizeState {
                life_seq: life,
                revision: 1,
                bounty: 10,
                max_energy: 1_000,
                rotation_bits: 1.0_f32.to_bits(),
                thrust_bits: 300.0_f32.to_bits(),
                top_speed_bits: 400.0_f32.to_bits(),
                recharge_bits: 50.0_f32.to_bits(),
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: life,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        )
    };
    let mut game = Game::new_network_authority_test();
    game.networked_play = true;
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
                server_session_id: session,
                arena_stream_id: stream,
                input_sequence_base: u16::MAX,
                ..ServerClockBootstrap::default()
            }),
            ClientProtocolEvent::PlayerIdAssigned(player),
            ClientProtocolEvent::JoinedArena(epoch),
            ClientProtocolEvent::ArenaSettingsUpdated(round9_test_settings()),
            ClientProtocolEvent::SecurityChallengeReceived {
                challenge: SecurityChallenge::default(),
                server_session_id: Some(session),
                arena_epoch: epoch,
            },
            ClientProtocolEvent::ZoneMapPublished,
            ClientProtocolEvent::ZoneFilesPublished,
            lifecycle_snapshot(),
        ],
        authority_admission: Vec::new(),
    });
    game.network_prize_baseline_pending = false;
    game.door_clock_fresh = true;
    assert_eq!(
        game.client_authority.view().phase,
        ClientAuthorityPhase::AwaitingDurableSubmission
    );
    assert!(!game.local_gameplay_ready());

    assert_eq!(
        game.client_authority
            .record_initial_baseline_submitted(28, 25_044),
        ClientAuthorityDisposition::Applied
    );
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::LifecycleBaselineAccepted(
            LifecycleBaselineAccepted {
                client_reliable_delivery_id: 28,
                life_seq: life,
                position_sequence: 25_044,
                accepted_server_tick: 500,
                accepted_position: Some((8104, 8312)),
            },
        )],
        authority_admission: Vec::new(),
    });
    let completed = game.client_authority.view();
    assert_eq!(completed.phase, ClientAuthorityPhase::Live);
    assert!(!completed.initial_baseline_pending);
    assert_eq!(completed.lifecycle_receipts_seen, 1);
    assert_eq!(completed.lifecycle_receipts_matched, 1);
    assert!(game.local_gameplay_ready());
    assert_eq!(game.ship.pos, vec2(8104.0, 8312.0));
    assert_eq!(
        game.ship.thrust_acceleration, 300.0,
        "receipt must preserve the installed private loadout"
    );

    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![lifecycle_snapshot()],
        authority_admission: Vec::new(),
    });
    assert_eq!(
        game.client_authority.view().phase,
        ClientAuthorityPhase::Live
    );
    assert!(!game.client_authority.view().initial_baseline_pending);
}

#[test]
fn pending_respawn_holds_the_death_view_past_eligibility_until_receipt() {
    let mut game = Game::new_network_authority_test();
    let live = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(23),
        life_seq: Some(41),
        alive: Some(true),
    };
    game.install_network_authority_test_scope(
        live,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
        },
        81,
    );
    let prior_view = vec2(9000.25, 9099.75);
    game.ship.pos = vec2(9000.0, 9100.0);
    game.camera.target = prior_view;
    assert_eq!(
        game.client_authority.observe_private_lifecycle(
            ClientAuthoritySource::Event,
            AuthorityScopeIdentity {
                life_seq: Some(42),
                alive: Some(false),
                ..live
            },
            Some(500),
            1.0,
        ),
        ClientAuthorityDisposition::Replacement
    );
    game.player_respawn_timer = 1.0;
    assert!(!game.local_ship_pose_presentable());
    assert_eq!(game.local_render_camera_target(), prior_view);
    assert_eq!(
        game.client_authority.advance_respawn_eligibility(1.0),
        ClientAuthorityDisposition::Applied
    );
    // This is the same reset invoked by update_player_respawn at eligibility.
    // The native test deliberately avoids its GL-backed sound/frame machinery.
    game.reset_ship_after_death();
    assert_eq!(game.player_respawn_timer, 0.0);
    assert_eq!(game.ship.pos, game.player_spawn_position());
    assert!(
        !game.local_ship_pose_presentable(),
        "timer expiry alone cannot reveal the new ship"
    );
    let draw = game.local_render_prediction(0.01, &RenderInputContext::default());
    assert_eq!(draw.position, prior_view);
    assert_eq!(draw.velocity, Vec2::ZERO);
    assert_eq!(draw.input_bits, 0);
    assert_eq!(
        game.client_authority
            .record_lifecycle_baseline_submitted(100, 80, None),
        ClientAuthorityDisposition::Applied
    );
    let receipt = LifecycleBaselineAccepted {
        client_reliable_delivery_id: 100,
        life_seq: 42,
        position_sequence: 80,
        accepted_server_tick: 600,
        accepted_position: Some((8104, 8312)),
    };
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![ClientProtocolEvent::LifecycleBaselineAccepted(receipt)],
        outbound_packets: vec![],
        authority_admission: vec![],
    });
    assert!(game.local_ship_pose_presentable());
    assert_eq!(game.local_render_camera_target(), vec2(8104.0, 8312.0));
    assert_eq!(
        game.local_render_prediction(0.01, &RenderInputContext::default())
            .position,
        vec2(8104.0, 8312.0)
    );
}

#[test]
fn spawn_render_hold_does_not_freeze_offline_live_frequency_or_spectator_views() {
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        AuthorityScopeIdentity {
            server_session_id: Some(71),
            arena_stream_id: Some(81),
            player_id: Some(23),
            life_seq: Some(41),
            alive: Some(true),
        },
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
        },
        81,
    );
    game.ship.pos = vec2(9000.0, 9100.0);
    game.camera.target = vec2(8999.0, 9098.0);
    assert!(!game.local_spawn_pose_pending());
    assert!(game.local_ship_pose_presentable());
    assert_eq!(game.local_render_camera_target(), game.ship.pos);
    assert_eq!(
        game.client_authority
            .queue_lifecycle_intent(PendingLifecycleRequest::Frequency(27)),
        ClientAuthorityDisposition::Applied
    );
    assert_eq!(
        game.client_authority.view().transition_kind,
        Some(ClientAuthorityTransitionKind::Frequency)
    );
    assert!(
        !game.local_spawn_pose_pending(),
        "frequency retains its current pose"
    );
    assert!(game.local_ship_pose_presentable());
    assert_eq!(game.local_render_camera_target(), game.ship.pos);
    game.spectator_mode = true;
    assert!(!game.local_spawn_pose_pending());
    assert!(!game.local_ship_pose_presentable());
    assert_eq!(game.local_render_camera_target(), game.ship.pos);

    let mut offline = Game::new_network_authority_test();
    offline.networked_play = false;
    offline.ship.pos = vec2(7000.0, 7100.0);
    offline.camera.target = vec2(6999.0, 7098.0);
    assert!(!offline.local_spawn_pose_pending());
    assert!(offline.local_ship_pose_presentable());
    assert_eq!(offline.local_render_camera_target(), offline.ship.pos);
}

#[test]
fn pending_random_spawn_keeps_the_prior_render_view_until_the_matched_receipt() {
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        AuthorityScopeIdentity {
            server_session_id: Some(71),
            arena_stream_id: Some(81),
            player_id: Some(23),
            life_seq: Some(41),
            alive: Some(true),
        },
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
        },
        81,
    );
    game.network_zone_files_ready = true;
    game.last_self_server_prize_life_seq = Some(41);
    game.last_self_server_prize_revision = Some(1);
    game.ship.pos = vec2(9000.0, 9100.0);
    // The prior actual drawn pose can contain a disposable subpixel lead.
    let prior_view = vec2(9000.25, 9099.75);
    game.camera.target = prior_view;
    assert_eq!(
        game.client_authority
            .queue_lifecycle_intent(PendingLifecycleRequest::Ship(1)),
        ClientAuthorityDisposition::Applied
    );
    assert_eq!(
        game.client_authority.record_ship_request_submitted(1, 80),
        ClientAuthorityDisposition::Applied
    );
    assert!(
        game.local_ship_pose_presentable(),
        "the old ship remains visible until the server acknowledges replacement"
    );
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![ClientProtocolEvent::ShipFrequencyChanged(
            crate::protocol::ShipFrequencyChange {
                player_id: 23,
                ship: 1,
                frequency: 23,
                life_seq: Some(42),
            },
        )],
        outbound_packets: vec![],
        authority_admission: vec![],
    });
    assert!(game.client_authority.view().neutral_ship_baseline_ready());
    game.normalize_ship_life_reset_pose();
    let canonical_anchor = game.player_spawn_position();
    assert_eq!(
        game.ship.pos, canonical_anchor,
        "the reliable neutral request still proves the exact static anchor"
    );
    let draw = game.local_render_prediction(0.01, &RenderInputContext::default());
    assert_eq!(
        draw.position, prior_view,
        "the held view must not jump to the uncommitted neutral-request centre"
    );
    assert_eq!(draw.partial_seconds, 0.0);
    assert_eq!(
        game.ship.pos, canonical_anchor,
        "drawing cannot change the canonical request"
    );
    assert!(!game.local_ship_pose_presentable());
    assert_eq!(game.local_render_camera_target(), prior_view);
    assert_eq!(
        game.client_authority
            .record_lifecycle_baseline_submitted(100, 80, None),
        ClientAuthorityDisposition::Applied
    );
    let receipt = LifecycleBaselineAccepted {
        client_reliable_delivery_id: 100,
        life_seq: 42,
        position_sequence: 80,
        accepted_server_tick: 500,
        accepted_position: Some((8104, 8312)),
    };
    for wrong in [
        LifecycleBaselineAccepted {
            client_reliable_delivery_id: 99,
            ..receipt
        },
        LifecycleBaselineAccepted {
            life_seq: 41,
            ..receipt
        },
        LifecycleBaselineAccepted {
            position_sequence: 79,
            ..receipt
        },
    ] {
        game.apply_network_actions(&ClientProtocolActions {
            events: vec![ClientProtocolEvent::LifecycleBaselineAccepted(wrong)],
            outbound_packets: vec![],
            authority_admission: vec![],
        });
        assert!(!game.local_ship_pose_presentable());
        assert_eq!(
            game.local_render_prediction(0.01, &RenderInputContext::default())
                .position,
            prior_view
        );
        assert_eq!(game.ship.pos, canonical_anchor);
    }
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![ClientProtocolEvent::LifecycleBaselineAccepted(receipt)],
        outbound_packets: vec![],
        authority_admission: vec![],
    });
    assert!(game.local_ship_pose_presentable());
    assert_eq!(game.local_render_camera_target(), vec2(8104.0, 8312.0));
    assert_eq!(
        game.local_render_prediction(0.01, &RenderInputContext::default())
            .position,
        vec2(8104.0, 8312.0)
    );
    game.ship.pos += vec2(12.0, 6.0);
    let moved = game.ship.pos;
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![ClientProtocolEvent::LifecycleBaselineAccepted(receipt)],
        outbound_packets: vec![],
        authority_admission: vec![],
    });
    assert_eq!(
        game.local_render_camera_target(),
        moved,
        "duplicate acceptance cannot freeze or rebase the view"
    );
}

#[test]
fn initial_join_baseline_normalizes_legacy_half_pixel_coordinates_to_exact_spawn() {
    let mut game = Game::new_network_authority_test();
    let spawn = game.player_spawn_position();
    game.ship.pos = spawn + vec2(0.5, 0.5);
    game.ship.vel = vec2(17.0, -23.0);
    game.ship.orientation = 0.75;

    game.normalize_ship_life_reset_pose();

    assert_eq!(game.ship.pos, spawn);
    assert_eq!(game.ship.vel, Vec2::ZERO);
    assert_eq!(game.ship.orientation, 0.0);
    assert_eq!(protocol_position_coord(game.ship.pos.x), spawn.x as u16);
    assert_eq!(protocol_position_coord(game.ship.pos.y), spawn.y as u16);
}

#[test]
fn network_spawn_ignores_replayed_brick_overlays_but_not_authored_brick_tiles() {
    let mut game = Game::new_network_authority_test();
    game.networked_play = true;
    let map_index = game.map_index;
    let center = (game.maps[map_index].width / 2) as i32;
    for y in center - 2..=center + 2 {
        for x in center - 2..=center + 2 {
            game.maps[map_index].set_tile(x, y, 0);
        }
    }
    let canonical = game.player_spawn_position();
    let neighbor = ivec2(center, center + 1);
    assert!(game.install_brick_tile(neighbor, 120.0, 7));
    assert_ne!(game.map().spawn_position(), canonical);
    assert_eq!(
        game.player_spawn_position(),
        canonical,
        "accepted replay must not change the server's static initial baseline"
    );
    game.networked_play = false;
    assert_ne!(game.player_spawn_position(), canonical);
    game.networked_play = true;
    game.clear_bricks();
    game.maps[map_index].set_tile(neighbor.x, neighbor.y, BRICK_TILE_ID);
    assert_ne!(
        game.player_spawn_position(),
        canonical,
        "an authored tile with the brick ID is still part of the static map"
    );
    game.maps[map_index].set_tile(neighbor.x, neighbor.y, 162);
    let static_spawn = game.player_spawn_position();
    assert_ne!(static_spawn, canonical);
    for seed in [0, 1, 255] {
        game.maps[map_index].door_seed = seed;
        assert_eq!(game.player_spawn_position(), static_spawn);
    }
    game.networked_play = false;
    game.maps[map_index].door_seed = 0;
    assert_eq!(game.player_spawn_position(), canonical);
}

#[test]
fn door_close_relocation_uses_the_servers_static_spawn_not_the_live_brick_map() {
    let mut game = Game::new_network_authority_test();
    game.networked_play = true;
    let map_index = game.map_index;
    let center = (game.maps[map_index].width / 2) as i32;
    for y in center - 2..=center + 2 {
        for x in center - 2..=center + 2 {
            game.maps[map_index].set_tile(x, y, 0);
        }
    }
    let canonical = game.player_spawn_position();
    assert!(game.install_brick_tile(ivec2(center, center - 1), 120.0, 7));
    assert_ne!(
        game.map().spawn_position(),
        canonical,
        "the brick moves the live map's spawn to a neighbouring tile"
    );
    game.ship.pos = canonical + vec2(900.0, -400.0);
    game.ship.vel = vec2(40.0, -12.0);
    game.pending_same_life_movement_boundary_position = false;

    let relocated = game.relocate_ship_for_closed_door();

    assert_eq!(relocated, canonical, "the server replays the static spawn");
    assert_eq!(game.ship.pos, canonical);
    assert_eq!(game.ship.vel, Vec2::ZERO);
    assert!(game.pending_same_life_movement_boundary_position);
}

#[test]
fn initial_brick_replay_survives_map_geometry_publication_without_restarting_age() {
    for clear_before_map in [false, true] {
        let mut game = Game::new_network_authority_test();
        game.install_network_authority_test_scope(
            AuthorityScopeIdentity {
                server_session_id: Some(71),
                arena_stream_id: Some(81),
                player_id: Some(23),
                life_seq: Some(41),
                alive: Some(true),
            },
            AuthorityReadiness {
                joined: true,
                arena_epoch: Some(81),
                settings: true,
                security: true,
                map: false,
            },
            81,
        );
        game.last_observed_browser_server_tick = Some(6000);
        game.tuning.brick_lifetime = 120.0;
        let drop = BrickDrop {
            x1: 7,
            y1: 8,
            x2: 16,
            y2: 8,
            team: 5,
            brick_id: 3,
            timestamp: 1000,
        };
        // The placeholder can have terrain where the downloaded arena is empty.
        game.maps[game.map_index].set_tile(7, 8, 1);
        let mut events = vec![ClientProtocolEvent::BrickDropped(BrickDropped::Drop(drop))];
        if clear_before_map {
            events.push(ClientProtocolEvent::BrickDropped(BrickDropped::Clear));
        }
        game.apply_network_actions(&ClientProtocolActions {
            events,
            outbound_packets: vec![],
            authority_admission: vec![],
        });
        assert!(
            game.bricks.is_empty(),
            "pre-map replay must not mutate placeholder collision"
        );
        assert_eq!(
            game.network_world.brick_lines.len(),
            if clear_before_map { 0 } else { 1 }
        );
        let mut map = ArenaMap::fallback_skirmish();
        for x in 7..17 {
            map.set_tile(x, 8, 0);
        }
        // Exercise the actual geometry/cache invalidation and replacement
        // helper without the GPU/audio work in switch_to_map. The browser
        // lane covers the complete downloaded-level/render pipeline.
        game.clear_bricks();
        let old_index = game.map_index;
        game.map_index =
            crate::game::events::replace_current_downloaded_map(&mut game.maps, old_index, map);
        game.apply_network_actions(&ClientProtocolActions {
            events: vec![ClientProtocolEvent::ZoneMapPublished],
            outbound_packets: vec![],
            authority_admission: vec![],
        });
        assert_eq!(game.bricks.len(), if clear_before_map { 0 } else { 10 });
        assert!(
            game.bricks
                .iter()
                .all(|b| b.age == 50.0 && b.ttl == 70.0 && b.team == 5)
        );
        game.apply_network_actions(&ClientProtocolActions {
            events: vec![ClientProtocolEvent::JoinedArena(82)],
            outbound_packets: vec![],
            authority_admission: vec![],
        });
        assert!(
            game.bricks.is_empty(),
            "an actual arena replacement clears the old tiles"
        );
        assert!(game.network_world.brick_lines.is_empty());
    }
}

#[test]
fn ship_receipt_waits_for_exact_successor_loadout_before_canonical_thrust() {
    let mut game = Game::new_network_authority_test();
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(23),
        life_seq: Some(41),
        alive: Some(true),
    };
    game.install_network_authority_test_scope(
        scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
        },
        81,
    );
    game.network_zone_files_ready = true;
    game.last_self_server_prize_life_seq = Some(41);
    game.last_self_server_prize_revision = Some(1);
    game.ship.thrust_acceleration = 190.0;
    assert_eq!(
        game.client_authority
            .queue_lifecycle_intent(PendingLifecycleRequest::Ship(8)),
        ClientAuthorityDisposition::Applied
    );
    assert_eq!(
        game.client_authority.record_ship_request_submitted(8, 79),
        ClientAuthorityDisposition::Applied
    );
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![ClientProtocolEvent::ShipFrequencyChanged(
            crate::protocol::ShipFrequencyChange {
                player_id: 23,
                ship: 8,
                frequency: 23,
                life_seq: Some(42),
            },
        )],
        outbound_packets: vec![],
        authority_admission: vec![],
    });
    assert!(game.spectator_mode);
    assert_eq!(
        game.client_authority
            .queue_lifecycle_intent(PendingLifecycleRequest::Ship(0)),
        ClientAuthorityDisposition::Applied
    );
    assert_eq!(
        game.client_authority.record_ship_request_submitted(0, 80),
        ClientAuthorityDisposition::Applied
    );
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![ClientProtocolEvent::ShipFrequencyChanged(
            crate::protocol::ShipFrequencyChange {
                player_id: 23,
                ship: 0,
                frequency: 23,
                life_seq: Some(43),
            },
        )],
        outbound_packets: vec![],
        authority_admission: vec![],
    });
    assert!(game.client_authority.view().neutral_ship_baseline_ready());
    assert_eq!(
        game.client_authority
            .record_lifecycle_baseline_submitted(100, 80, None),
        ClientAuthorityDisposition::Applied
    );
    let receipt = ClientProtocolEvent::LifecycleBaselineAccepted(LifecycleBaselineAccepted {
        client_reliable_delivery_id: 100,
        life_seq: 43,
        position_sequence: 80,
        accepted_server_tick: 500,
        accepted_position: None,
    });
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![receipt.clone()],
        outbound_packets: vec![],
        authority_admission: vec![],
    });
    assert_eq!(
        game.client_authority.view().phase,
        ClientAuthorityPhase::Live
    );
    game.suppress_gameplay_step_after_network_rebase = false;
    assert!(
        !game.gameplay_step_ready(),
        "receipt alone cannot integrate reset thrust as authoritative input"
    );
    assert!(
        game.network_prize_baseline_pending,
        "outbound input must also await the exact loadout"
    );
    let prize = |life, revision| {
        private_prize_snapshot(
            revision as u16,
            81,
            SnapshotPrizeState {
                life_seq: life,
                revision,
                bounty: 30,
                max_energy: 1700,
                energy: Some(1700),
                rotation_bits: 1.0_f32.to_bits(),
                thrust_bits: 190.0_f32.to_bits(),
                top_speed_bits: 325.0_f32.to_bits(),
                recharge_bits: 115.0_f32.to_bits(),
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: life,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        )
    };
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![prize(41, 2)],
        outbound_packets: vec![],
        authority_admission: vec![],
    });
    assert!(
        !game.gameplay_step_ready(),
        "the previous life's loadout cannot release movement"
    );
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![prize(43, 3)],
        outbound_packets: vec![],
        authority_admission: vec![],
    });
    assert!(game.gameplay_step_ready());
    assert!(!game.network_prize_baseline_pending);
    assert_eq!(game.ship.thrust_acceleration, 190.0);
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![receipt],
        outbound_packets: vec![],
        authority_admission: vec![],
    });
    assert_eq!(
        game.ship.thrust_acceleration, 190.0,
        "duplicate receipt cannot erase the loadout"
    );
    assert!(game.gameplay_step_ready());
}

#[test]
fn stale_old_life_portal_rejection_cannot_refund_replacement_life() {
    let (mut game, old_request) = portal_reconciliation_game(1);
    let replacement_life = old_request.life_seq.wrapping_add(1).max(1);
    let authority = game.client_authority.view();
    game.client_authority.install_test_scope(
        authority.segment,
        AuthorityScopeIdentity {
            life_seq: Some(replacement_life),
            alive: Some(true),
            ..authority.scope
        },
        authority.readiness,
    );
    game.last_self_server_prize_life_seq = Some(replacement_life);
    game.last_self_server_prize_revision = Some(20);
    game.last_self_server_portals = Some((replacement_life, 20, 3));
    game.ship.portals = 3;

    let actions = portal_terminal(
        old_request,
        11,
        2,
        PortalPlacementAccepted::OUTCOME_REJECTED,
    );
    let plan = build_authority_admission_plan(
        &actions,
        AuthorityAdmissionSeed {
            scope: AuthorityScopeIdentity {
                server_session_id: Some(old_request.server_session_id),
                arena_stream_id: Some(old_request.arena_stream_id),
                player_id: Some(old_request.player_id),
                life_seq: Some(replacement_life),
                alive: Some(true),
            },
            readiness: AuthorityReadiness {
                joined: true,
                arena_epoch: Some(111),
                settings: true,
                security: true,
                map: true,
            },
            segment: 0,
        },
    );
    assert_eq!(plan[0].private_fact, PrivateAuthorityAdmission::Reject);
    game.apply_network_actions(&actions);

    assert_eq!(game.ship.portals, 3);
    assert_eq!(
        game.last_self_server_portals,
        Some((replacement_life, 20, 3))
    );
    assert_eq!(game.pending_portal_placements.len(), 1);
}

#[test]
fn lifecycle_intent_waits_for_complete_zone_publication_before_timeout_ownership() {
    let mut intent = None;
    assert!(preserve_lifecycle_request_intent(
        &mut intent,
        true,
        PendingLifecycleRequest::Ship(1),
    ));
    assert!(!lifecycle_request_emission_ready(false));
    assert_eq!(intent, Some(PendingLifecycleRequest::Ship(1)));

    // The exact authenticated publication boundary releases the same intent;
    // no wall-clock delay or rewritten sequence is involved.
    assert!(lifecycle_request_emission_ready(true));
    assert_eq!(intent, Some(PendingLifecycleRequest::Ship(1)));

    let mut frequency_intent = None;
    assert!(queue_frequency_change_for_network_tail(
        &mut frequency_intent,
        true,
        u16::MAX,
    ));
    assert_eq!(
        frequency_intent,
        Some(PendingLifecycleRequest::Frequency(u16::MAX))
    );
    assert!(!lifecycle_request_emission_ready(false));
    assert!(lifecycle_request_emission_ready(true));
}

#[test]
fn damage_prize_notification_identity_is_exactly_once_across_duplicate_and_newer_private_revisions()
{
    assert!(damage_prize_notification_is_new(
        None,
        None,
        None,
        7,
        u32::MAX,
        27,
    ));
    assert!(
        !damage_prize_notification_is_new(Some(7), Some(u32::MAX), Some(27), 7, u32::MAX, 27),
        "the same loss marker remains one notification when a later private prize mutation advances the frame revision"
    );
    assert!(damage_prize_notification_is_new(
        Some(7),
        Some(u32::MAX),
        Some(27),
        8,
        1,
        27,
    ));
    assert!(damage_prize_notification_is_new(
        Some(8),
        Some(1),
        Some(27),
        8,
        2,
        27,
    ));
}

#[test]
fn private_prize_timer_uses_absolute_wrap_safe_deadline() {
    let frame_tick = u32::MAX - 4;
    let deadline = crate::continuum::continuum_tick_add(frame_tick, 10);
    assert!((authoritative_prize_timer_seconds(Some(deadline), frame_tick) - 0.1).abs() < 0.0001);
    assert_eq!(
        authoritative_prize_timer_seconds(Some(deadline), deadline),
        0.0
    );
    assert_eq!(authoritative_prize_timer_seconds(None, frame_tick), 0.0);
}

#[test]
fn shutdown_source_replacement_uses_revision_not_longest_deadline() {
    let life = 7;
    let negative_deadline = 9_000;
    let positive_deadline = 3_000;
    let newer_positive = AcceptedPrizeEngineShutdown {
        life_seq: life,
        prize_state_revision: 11,
        until_tick: Some(positive_deadline),
    };
    assert_eq!(
        authoritative_engine_shutdown_deadline_by_revision(
            Some((life, 10, Some(negative_deadline), false)),
            Some(newer_positive),
            [],
        ),
        Some(positive_deadline),
        "a newer positive Shutdown replaces the longer negative deadline",
    );
    assert_eq!(
        authoritative_engine_shutdown_deadline_by_revision(
            Some((life, 12, Some(negative_deadline), false)),
            Some(newer_positive),
            [],
        ),
        Some(negative_deadline),
        "a reordered older accepted source cannot re-extend a newer PrizeState",
    );
    assert_eq!(
        authoritative_engine_shutdown_deadline_by_revision(
            Some((life, u32::MAX, Some(positive_deadline), false)),
            None,
            [(life, 0, Some(negative_deadline))],
        ),
        Some(negative_deadline),
        "positive-to-negative replacement advances across revision wrap",
    );
    assert_eq!(
        authoritative_engine_shutdown_deadline_by_revision(
            Some((life, 20, Some(negative_deadline), false)),
            Some(AcceptedPrizeEngineShutdown {
                life_seq: life.wrapping_add(1),
                prize_state_revision: 21,
                until_tick: Some(positive_deadline),
            }),
            [],
        ),
        Some(negative_deadline),
        "a different-life source never replaces the current life",
    );
    assert_eq!(authoritative_prize_timer_seconds(Some(50), 50), 0.0);
}

#[test]
fn pending_shutdown_snapshot_preserves_source_without_pre_ack_activation() {
    let life = 7;
    let deadline = 9_000;
    assert_eq!(
        authoritative_engine_shutdown_deadline_by_revision(
            Some((life, 12, Some(deadline), true)),
            None,
            [],
        ),
        None,
        "a pending snapshot source is continuity evidence, not activation",
    );
    assert_eq!(
        authoritative_engine_shutdown_deadline_by_revision(
            Some((life, 12, Some(deadline), true)),
            None,
            [(life, 11, Some(deadline))],
        ),
        Some(deadline),
        "the exact older ACK/application source survives an unrelated aggregate revision",
    );
    assert_eq!(
        authoritative_engine_shutdown_deadline_by_revision(
            Some((life, 13, Some(deadline), false)),
            Some(AcceptedPrizeEngineShutdown {
                life_seq: life,
                prize_state_revision: 11,
                until_tick: Some(deadline.wrapping_add(500)),
            }),
            [],
        ),
        Some(deadline),
        "an applied aggregate snapshot retires the older accepted source",
    );
}

#[test]
fn equal_revision_shutdown_replacement_preserves_independent_emp_recharge_gate() {
    let life = 7;
    let revision = 12;
    let accepted = AcceptedPrizeEngineShutdown {
        life_seq: life,
        prize_state_revision: revision,
        until_tick: Some(9_000),
    };
    for authoritative_deadline in [Some(7_000), Some(11_000), None] {
        let selected = authoritative_engine_shutdown_deadline_by_revision(
            Some((life, revision, authoritative_deadline, false)),
            Some(accepted),
            [(life, revision, Some(accepted.until_tick.unwrap()))],
        );
        assert_eq!(selected, authoritative_deadline);
    }

    let tick = 7_000;
    let expired = authoritative_engine_shutdown_deadline_by_revision(
        Some((life, revision, Some(tick), false)),
        Some(accepted),
        [],
    );
    assert_eq!(authoritative_prize_timer_seconds(expired, tick), 0.0);

    fn movement_for_deadline(deadline: Option<u32>, tick: u32) -> PlayerMovementOutcome {
        step_player_movement(
            PlayerMovementState {
                orientation: 0.0,
                velocity: MovementVector::ZERO,
                energy: 1_000.0,
                max_energy: 1_700.0,
            },
            PlayerMovementInput {
                thrust: true,
                rotate_right: true,
                ..Default::default()
            },
            PlayerMovementTuning {
                rotation_per_second: 0.5,
                shutdown_rotation_per_second: 0.1,
                base_thrust_acceleration: 150.0,
                afterburner_thrust_acceleration: 300.0,
                rocket_thrust_acceleration: 500.0,
                base_top_speed: 200.0,
                afterburner_top_speed: 400.0,
                rocket_top_speed: 600.0,
                afterburner_energy_per_second: 120.0,
                recharge_per_second: 200.0,
                turret_thrust_penalty: 40.0,
                turret_speed_penalty: 50.0,
                repel_top_speed: 800.0,
                flagger_thrust_adjustment: 25.0,
                flagger_speed_adjustment: 20.0,
            },
            PlayerMovementEnvironment {
                // An EMP-only private state has no prize movement deadline.
                engine_shutdown: false,
                emp_shutdown: authoritative_prize_timer_seconds(deadline, tick) > 0.0,
                ..Default::default()
            },
            0.1,
        )
    }

    let active = movement_for_deadline(Some(tick + 1), tick);
    assert_eq!(active.state.energy, 1_000.0, "active EMP blocks recharge");
    assert!((active.state.orientation - 0.05).abs() < f32::EPSILON);

    let expired = movement_for_deadline(Some(tick), tick);
    assert_eq!(
        active.state.velocity, expired.state.velocity,
        "EMP must not change the canonical thrust result"
    );
    assert_eq!(
        expired.state.energy, 1_020.0,
        "expired EMP recharges immediately"
    );
    assert!(expired.state.velocity.x.abs() < 0.001);
    assert!((expired.state.velocity.y + 15.0).abs() < 0.001);
    assert!((expired.state.orientation - 0.05).abs() < f32::EPSILON);
}

#[test]
fn pickup_ack_scope_rejects_lifecycle_overtake_reorder_duplicates_and_wrap() {
    let ack = PrizePickupAck {
        server_session_id: 77,
        arena_stream_id: 3,
        player_id: 2,
        collectible_id: 88,
        x: 8_136,
        y: 8_280,
        prize_id: PrizeKind::Warp.protocol_id(),
        authoritative_energy: 1_234,
        life_seq: u32::MAX,
        prize_state_revision: 17,
        engine_shutdown_until_tick: None,
        engine_shutdown_application_required: false,
    };
    let scoped = scoped_prize_pickup_ack(ack, 77, 3, 2);
    assert!(prize_pickup_ack_matches_current_runtime(
        Some(77),
        Some(3),
        Some(2),
        Some(u32::MAX),
        Some(true),
        false,
        scoped,
    ));
    assert!(
        prize_pickup_ack_matches_current_runtime(
            Some(77),
            Some(3),
            Some(2),
            Some(u32::MAX),
            Some(true),
            false,
            scoped,
        ),
        "a reliable duplicate remains the same admissible identity for the dedupe layer"
    );
    assert!(
        !prize_pickup_ack_matches_current_runtime(
            Some(77),
            Some(3),
            Some(2),
            Some(1),
            Some(true),
            false,
            scoped,
        ),
        "wrapped replacement life rejects the delayed old-life Warp ACK"
    );
    assert!(!prize_pickup_ack_matches_current_runtime(
        Some(78),
        Some(3),
        Some(2),
        Some(u32::MAX),
        Some(true),
        false,
        scoped,
    ));
    assert!(!prize_pickup_ack_matches_current_runtime(
        Some(77),
        Some(4),
        Some(2),
        Some(u32::MAX),
        Some(true),
        false,
        scoped,
    ));
    assert!(!prize_pickup_ack_matches_current_runtime(
        Some(77),
        Some(3),
        Some(3),
        Some(u32::MAX),
        Some(true),
        false,
        scoped,
    ));
    assert!(!prize_pickup_ack_matches_current_runtime(
        Some(77),
        Some(3),
        Some(2),
        Some(u32::MAX),
        Some(false),
        false,
        scoped,
    ));
    assert!(!prize_pickup_ack_matches_current_runtime(
        Some(77),
        Some(3),
        Some(2),
        Some(u32::MAX),
        Some(true),
        true,
        scoped,
    ));
    for body_mismatch in [
        ScopedPrizePickupAck {
            ack: PrizePickupAck {
                server_session_id: 78,
                ..ack
            },
            ..scoped
        },
        ScopedPrizePickupAck {
            ack: PrizePickupAck {
                arena_stream_id: 4,
                ..ack
            },
            ..scoped
        },
        ScopedPrizePickupAck {
            ack: PrizePickupAck {
                player_id: 3,
                ..ack
            },
            ..scoped
        },
    ] {
        assert!(
            !prize_pickup_ack_matches_current_runtime(
                Some(77),
                Some(3),
                Some(2),
                Some(u32::MAX),
                Some(true),
                false,
                body_mismatch,
            ),
            "packet identity cannot be rebound to its decode-time scope"
        );
    }

    let ordinary = scoped_prize_pickup_ack(
        PrizePickupAck {
            server_session_id: 77,
            arena_stream_id: 3,
            player_id: 2,
            prize_id: PrizeKind::Energy.protocol_id(),
            ..ack
        },
        77,
        3,
        2,
    );
    assert!(
        !prize_pickup_ack_matches_current_runtime(
            Some(77),
            Some(3),
            Some(2),
            Some(1),
            Some(true),
            false,
            ordinary,
        ),
        "ordinary prize ACKs use the identical pre-mutation life gate"
    );

    let delayed_warp = ClientProtocolEvent::PrizePickupAck(scoped);
    let replacement_lifecycle = ClientProtocolEvent::SnapshotReceived(SnapshotFrame {
        header: SnapshotHeader {
            arena_stream_id: 3,
            ..SnapshotHeader::default()
        },
        events: vec![SnapshotEvent::LifecycleState(SnapshotLifecycleState {
            life_seq: 1,
            alive: true,
            respawn_at_tick: None,
            repel_until_tick: None,
            repel_delivery: None,
        })],
        ..SnapshotFrame::default()
    });
    let reordered = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            delayed_warp.clone(),
            replacement_lifecycle.clone(),
            delayed_warp,
        ],
        authority_admission: Vec::new(),
    };
    assert!(prize_pickup_ack_is_overtaken_in_action_batch(
        &reordered,
        Some(2),
        Some(u32::MAX),
        Some(true),
        ack,
    ));
    assert!(prize_pickup_ack_is_overtaken_in_action_batch(
        &ClientProtocolActions {
            outbound_packets: Vec::new(),
            events: vec![
                ClientProtocolEvent::PrizePickupAck(ordinary),
                replacement_lifecycle,
            ],
            authority_admission: Vec::new(),
        },
        Some(2),
        Some(u32::MAX),
        Some(true),
        ordinary.ack,
    ));

    let replacement_bootstrap =
        ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
            server_session_id: 78,
            arena_stream_id: 4,
            ..ServerClockBootstrap::default()
        });
    for stale in [scoped, ordinary] {
        assert!(prize_pickup_ack_is_overtaken_in_action_batch(
            &ClientProtocolActions {
                outbound_packets: Vec::new(),
                events: vec![
                    ClientProtocolEvent::PrizePickupAck(stale),
                    replacement_bootstrap.clone(),
                    ClientProtocolEvent::PrizePickupAck(stale),
                ],
                authority_admission: Vec::new(),
            },
            Some(2),
            Some(u32::MAX),
            Some(true),
            stale.ack,
        ));
    }
    assert!(prize_pickup_ack_is_overtaken_in_action_batch(
        &ClientProtocolActions {
            outbound_packets: Vec::new(),
            events: vec![
                ClientProtocolEvent::PrizePickupAck(scoped),
                ClientProtocolEvent::Disconnected,
            ],
            authority_admission: Vec::new(),
        },
        Some(2),
        Some(u32::MAX),
        Some(true),
        scoped.ack,
    ));
}

fn movement_correction(position_sequence: u16) -> MovementCorrection {
    MovementCorrection {
        reason: MovementCorrection::REASON_SUSTAINED_DRIFT,
        repel_window: None,
        server_session_id: 77,
        player_id: 9,
        life_seq: 41,
        correction_generation: 3,
        position_sequence,
        server_tick: 1_234,
        position_x: 10_500.375,
        position_y: 8_191.75,
        velocity_x: -321.125,
        velocity_y: 47.5,
        orientation: 0.9875,
    }
}

fn scoped_prize_pickup_ack(
    mut ack: PrizePickupAck,
    server_session_id: u64,
    arena_epoch: u32,
    player_id: u16,
) -> ScopedPrizePickupAck {
    ack.server_session_id = server_session_id;
    ack.arena_stream_id = u64::from(arena_epoch);
    ack.player_id = player_id;
    ScopedPrizePickupAck {
        ack,
        server_session_id: Some(server_session_id),
        arena_stream_id: Some(u64::from(arena_epoch)),
        player_id: Some(player_id),
    }
}

#[test]
fn applied_movement_correction_terminal_installs_exact_raw_f32_current_tail() {
    let correction = movement_correction(17);
    let receipt = MovementCorrectionAccepted {
        server_tick: correction.server_tick,
        outcome: MovementCorrectionAccepted::OUTCOME_APPLIED,
        server_session_id: correction.server_session_id,
        player_id: correction.player_id,
        life_seq: correction.life_seq,
        correction_generation: correction.correction_generation,
        detected_sequence: correction.position_sequence,
        application_sequence: 40,
        position_x: 12_000.375,
        position_y: 9_000.75,
        velocity_x: -201.125,
        velocity_y: 91.5,
        orientation: 0.75,
    };
    let mut ship = Ship {
        pos: vec2(1.0, 2.0),
        vel: vec2(3.0, 4.0),
        orientation: 0.25,
        ..Ship::default()
    };
    apply_exact_movement_correction_terminal(&mut ship, receipt);
    assert_eq!(ship.pos.x.to_bits(), receipt.position_x.to_bits());
    assert_eq!(ship.pos.y.to_bits(), receipt.position_y.to_bits());
    assert_eq!(ship.vel.x.to_bits(), receipt.velocity_x.to_bits());
    assert_eq!(ship.vel.y.to_bits(), receipt.velocity_y.to_bits());
    assert_eq!(ship.orientation.to_bits(), receipt.orientation.to_bits());
}

#[test]
fn repel_correction_installs_the_sealed_window_without_consuming_the_held_receipt() {
    for (next_sequence, pending_generation) in [(25_u16, 7), (41, 7), (25, 8), (41, 8)] {
        let mut game = Game::new_network_authority_test();
        game.install_network_authority_test_scope(
            AuthorityScopeIdentity {
                server_session_id: Some(77),
                arena_stream_id: Some(88),
                player_id: Some(9),
                life_seq: Some(41),
                alive: Some(true),
            },
            AuthorityReadiness {
                joined: true,
                arena_epoch: Some(88),
                settings: true,
                security: true,
                map: true,
            },
            88,
        );
        game.next_input_seq = next_sequence;
        let marker = crate::continuum_position::ClientRepelApplicationMarker {
            snapshot_id: 933,
            application_sequence: 10,
            delivery_generation: pending_generation,
        };
        game.pending_repel_application = Some(marker);
        game.repel_movement_sequence_window = Some((10, 26));
        game.ship.repel_timer = 10.0;
        let proposal = MovementCorrection {
            reason: MovementCorrection::REASON_REPEL_WINDOW,
            repel_window: Some((10, 40, 7)),
            ..movement_correction(24)
        };
        game.apply_network_actions(&ClientProtocolActions {
            events: vec![ClientProtocolEvent::MovementCorrected(proposal)],
            ..Default::default()
        });
        let application = game
            .client_authority
            .correction_application_to_submit()
            .unwrap();
        if pending_generation != 7 {
            assert_eq!(
                application.intent,
                MovementCorrectionApplied::INTENT_CANCEL_NEWER_BOUNDARY
            );
            assert_eq!(game.pending_repel_application, Some(marker));
            continue;
        }
        assert_eq!(
            application.intent,
            MovementCorrectionApplied::INTENT_APPLY,
            "the server-sealed same Repel is not a newer uncrossed boundary"
        );
        game.client_authority
            .record_correction_application_submitted(42);
        let receipt = MovementCorrectionAccepted {
            outcome: MovementCorrectionAccepted::OUTCOME_APPLIED,
            server_tick: 1300,
            server_session_id: 77,
            player_id: 9,
            life_seq: 41,
            correction_generation: 3,
            detected_sequence: 24,
            application_sequence: application.application_sequence,
            position_x: 8000.0,
            position_y: 8100.0,
            velocity_x: 1.0,
            velocity_y: 2.0,
            orientation: 0.0,
        };
        game.apply_network_actions(&ClientProtocolActions {
            events: vec![ClientProtocolEvent::MovementCorrectionAccepted(receipt)],
            ..Default::default()
        });
        assert_eq!(
            game.repel_movement_sequence_window,
            (next_sequence < 40).then_some((10, 40))
        );
        assert_eq!(game.ship.repel_timer, 0.0);
        assert_eq!(game.pending_repel_application, Some(marker));
        assert!(game.consumed_repel_delivery_generations.is_empty());
        assert_eq!(game.ship.pos, vec2(8000.0, 8100.0));
    }
}

#[test]
fn dropped_warp_boundary_position_still_cancels_an_older_correction() {
    let marker_was_enqueued_but_not_admitted = false;
    assert!(newer_same_life_boundary_requires_correction_cancellation(
        marker_was_enqueued_but_not_admitted,
        Some(0x1020_3040),
        None,
    ));
}

#[test]
fn dropped_repel_boundary_position_still_cancels_an_older_correction() {
    let marker = crate::continuum_position::ClientRepelApplicationMarker {
        snapshot_id: 0xfffe,
        application_sequence: u16::MAX,
        delivery_generation: u32::MAX,
    };
    let marker_was_enqueued_but_not_admitted = false;
    assert!(newer_same_life_boundary_requires_correction_cancellation(
        marker_was_enqueued_but_not_admitted,
        None,
        Some(marker),
    ));
}

#[test]
fn receipted_warp_and_repel_do_not_leave_a_phantom_cancellation_boundary() {
    assert!(!newer_same_life_boundary_requires_correction_cancellation(
        false, None, None,
    ));
}

fn warp_prize_test_game() -> Game {
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        AuthorityScopeIdentity {
            server_session_id: Some(77),
            arena_stream_id: Some(88),
            player_id: Some(9),
            life_seq: Some(41),
            alive: Some(true),
        },
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(88),
            settings: true,
            security: true,
            map: true,
        },
        88,
    );
    game.network_prize_baseline_pending = false;
    game.ship.pos = vec2(7_136.5, 8_920.25);
    game.ship.vel = vec2(115.5, -63.0);
    game.ship.orientation = 0.325;
    game.ship.energy = 1_600.0;
    game.ship.max_energy = 1_700.0;
    game
}

fn warp_prize_ack(collectible_id: u32) -> ScopedPrizePickupAck {
    scoped_prize_pickup_ack(
        PrizePickupAck {
            server_session_id: 0,
            arena_stream_id: 0,
            player_id: 0,
            collectible_id,
            x: 7_136,
            y: 8_920,
            prize_id: PrizeKind::Warp.protocol_id(),
            authoritative_energy: 1_700,
            life_seq: 41,
            prize_state_revision: 37,
            engine_shutdown_until_tick: None,
            engine_shutdown_application_required: false,
        },
        77,
        88,
        9,
    )
}

fn neutral_input_packet(sequence: u16) -> ClientInputRecordPacket {
    ClientInputRecordPacket {
        base_seq: sequence,
        inputs: vec![crate::protocol::PlayerInputRecord {
            combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(sequence),
            ..Default::default()
        }],
    }
}

#[test]
fn warp_prize_relocates_on_the_report_that_first_carries_its_marker() {
    let mut game = warp_prize_test_game();
    let spawn = game.player_spawn_position();
    assert!(game.apply_network_prize_pickup_ack(warp_prize_ack(7_807)));
    // The ACK stages the relocation: no position may carry the marker
    // with a state that is not the spawn report itself.
    assert_eq!(game.ship.pos, vec2(7_136.5, 8_920.25));
    assert!(game.staged_warp_relocation_pending());
    assert!(game.pending_same_life_movement_boundary_position);
    assert_eq!(
        game.client_position_update(40_188, 0, neutral_input_packet(40_188), None)
            .warp_collectible_id,
        None
    );

    // On prod a gameplay step ran between the ACK and the next emitted
    // position, so the first marked report already had thrust on it and the
    // server rejected every report after it.
    game.ship.pos += vec2(0.05, 0.02);
    game.ship.vel = vec2(2.8, 1.4);
    assert!(
        !game.apply_staged_warp_relocation_for_emission(1, false),
        "a weapon report cannot also be the relocation report"
    );
    assert!(!game.apply_staged_warp_relocation_for_emission(0, true));
    assert!(game.staged_warp_relocation_pending());
    assert!(game.staged_warp_relocation_forces_next_report());
    game.pending_repel_application =
        Some(crate::continuum_position::ClientRepelApplicationMarker {
            snapshot_id: 933,
            application_sequence: 40_188,
            delivery_generation: 7,
        });
    assert!(
        !game.staged_warp_relocation_forces_next_report(),
        "a Warp waiting on a Repel receipt does not force every step's report"
    );
    game.pending_repel_application = None;

    assert!(game.apply_staged_warp_relocation_for_emission(0, false));
    let report = game.client_position_update(40_189, 0, neutral_input_packet(40_189), None);
    assert_eq!(report.warp_collectible_id, Some(7_807));
    assert_eq!((report.velocity_x, report.velocity_y), (0, 0));
    let (x, y, velocity_x, velocity_y, _) = report.canonical_state.values();
    assert_eq!(x.to_bits(), spawn.x.to_bits());
    assert_eq!(y.to_bits(), spawn.y.to_bits());
    assert_eq!(velocity_x.to_bits(), 0.0_f32.to_bits());
    assert_eq!(velocity_y.to_bits(), 0.0_f32.to_bits());
    assert!(!game.staged_warp_relocation_pending());
    assert!(
        !game.apply_staged_warp_relocation_for_emission(0, false),
        "the relocation happens once"
    );
}

#[test]
fn a_correction_detected_at_or_after_the_warp_report_overtakes_its_marker() {
    let published = std::collections::VecDeque::from([u16::MAX - 1, u16::MAX, 0]);
    assert!(warp_report_overtaken_by_correction(
        true,
        Some(41),
        &published,
        41,
        60
    ));
    assert!(warp_report_overtaken_by_correction(
        true,
        Some(41),
        &published,
        41,
        u16::MAX - 1
    ));
    // A correction detected before the report can still be overtaken by it.
    assert!(!warp_report_overtaken_by_correction(
        true,
        Some(41),
        &published,
        41,
        u16::MAX - 2
    ));
    assert!(!warp_report_overtaken_by_correction(
        false,
        Some(41),
        &published,
        41,
        60
    ));
    assert!(!warp_report_overtaken_by_correction(
        true,
        Some(41),
        &published,
        42,
        60
    ));
    assert!(!warp_report_overtaken_by_correction(
        true,
        Some(41),
        &std::collections::VecDeque::new(),
        41,
        60
    ));
}

#[test]
fn an_unadmitted_warp_report_no_longer_cancels_every_later_correction() {
    // Prod: the last admitted position was 40187, the Warp report 40189 was
    // never admitted, and history-gap corrections detected at 40252 and later
    // were each cancelled for the pending Warp until the player left.
    for (detected_sequence, overtaken) in [(40_252_u16, true), (40_150, false)] {
        let mut game = warp_prize_test_game();
        assert!(game.apply_network_prize_pickup_ack(warp_prize_ack(7_807)));
        assert!(game.apply_staged_warp_relocation_for_emission(0, false));
        game.pending_same_life_movement_boundary_position = false;
        game.pending_warp_relocation_sequences.push_back(40_189);
        game.next_input_seq = 40_260;
        game.apply_network_actions(&ClientProtocolActions {
            events: vec![ClientProtocolEvent::MovementCorrected(MovementCorrection {
                reason: MovementCorrection::REASON_INPUT_HISTORY_GAP,
                correction_generation: 1,
                ..movement_correction(detected_sequence)
            })],
            ..Default::default()
        });
        let application = game
            .client_authority
            .correction_application_to_submit()
            .expect("the correction is answered either way");
        if overtaken {
            assert_eq!(application.intent, MovementCorrectionApplied::INTENT_APPLY);
            assert_eq!(game.pending_warp_relocation_collectible_id, None);
            assert!(game.pending_warp_relocation_sequences.is_empty());
        } else {
            assert_eq!(
                application.intent,
                MovementCorrectionApplied::INTENT_CANCEL_NEWER_BOUNDARY
            );
            assert_eq!(game.pending_warp_relocation_collectible_id, Some(7_807));
        }
    }
}

#[test]
fn accepted_frequency_baseline_rebases_exact_physics_before_realtime_reopens() {
    let exact_position_x = 12_345.375_f32;
    let exact_position_y = 54_320.75_f32;
    let exact_velocity_x = -432.125_f32;
    let exact_velocity_y = 123.4375_f32;
    let exact_orientation = 0.998_75_f32;
    let baseline = ClientPositionUpdate {
        direction: 39,
        timestamp: 0,
        x: 12_345,
        y: 54_321,
        velocity_x: -4_321,
        velocity_y: 1_234,
        togglables: STATUS_STEALTH | STATUS_XRADAR,
        bounty: 777,
        energy: 1_234,
        weapon: 0,
        extra: None,
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_f32(
            exact_position_x,
            exact_position_y,
            exact_velocity_x,
            exact_velocity_y,
            exact_orientation,
        ),
        input_history: None,
        warp_collectible_id: None,
        repel_application: None,
    };
    let mut ship = Ship {
        pos: vec2(1.25, 2.5),
        vel: vec2(3.75, 4.5),
        orientation: 0.5,
        stealth: false,
        cloak: true,
        xradar: false,
        antiwarp: true,
        flash: true,
        energy: 99.5,
        ..Ship::default()
    };
    let mut bounty = 9_u32;
    let mut last_server_energy = Some(88.0);
    apply_frequency_baseline_rebase(&mut ship, &mut bounty, &mut last_server_energy, &baseline);
    assert_eq!(ship.pos, vec2(exact_position_x, exact_position_y));
    assert_eq!(ship.vel, vec2(exact_velocity_x, exact_velocity_y));
    assert_eq!(ship.orientation, exact_orientation);
    assert!(ship.stealth);
    assert!(!ship.cloak);
    assert!(ship.xradar);
    assert!(!ship.antiwarp);
    assert!(!ship.flash);
    assert_eq!(ship.energy, 1_234.0);
    assert_eq!(last_server_energy, Some(1_234.0));
    assert_eq!(bounty, 777);
}

#[test]
fn repel_application_receipt_requires_exact_session_life_generation_snapshot_and_sequence() {
    let marker = crate::continuum_position::ClientRepelApplicationMarker {
        snapshot_id: u16::MAX,
        application_sequence: 0,
        delivery_generation: u32::MAX,
    };
    let receipt = RepelApplicationAccepted {
        server_session_id: 77,
        life_seq: 9,
        delivery_generation: u32::MAX,
        snapshot_id: u16::MAX,
        application_sequence: 0,
        accepted_end_sequence: 135,
        active_steps: 135,
    };
    assert!(repel_application_receipt_matches(
        Some(77),
        Some(9),
        Some(marker),
        receipt
    ));
    assert!(!repel_application_receipt_matches(
        Some(78),
        Some(9),
        Some(marker),
        receipt
    ));
    assert!(!repel_application_receipt_matches(
        Some(77),
        Some(10),
        Some(marker),
        receipt
    ));
    for mismatched in [
        RepelApplicationAccepted {
            delivery_generation: 1,
            ..receipt
        },
        RepelApplicationAccepted {
            snapshot_id: 0,
            ..receipt
        },
        RepelApplicationAccepted {
            application_sequence: 1,
            ..receipt
        },
        RepelApplicationAccepted {
            accepted_end_sequence: 134,
            ..receipt
        },
        RepelApplicationAccepted {
            active_steps: 134,
            ..receipt
        },
    ] {
        assert!(!repel_application_receipt_matches(
            Some(77),
            Some(9),
            Some(marker),
            mismatched,
        ));
    }
}

#[test]
fn held_repel_wall_expiry_preserves_the_already_applied_sequence_movement() {
    let mut game = Game::new_network_authority_test();
    game.networked_play = false;
    game.spectator_mode = false;
    game.player_respawn_timer = 0.0;
    game.ship.vel = vec2(-365.4568, -14.041289);
    game.next_input_seq = 16287;
    game.repel_movement_sequence_window = Some((16177, 16313));
    game.ship.repel_timer = 0.4;
    let marker = crate::continuum_position::ClientRepelApplicationMarker {
        snapshot_id: 959,
        application_sequence: 16177,
        delivery_generation: 1,
    };
    game.pending_repel_application = Some(marker);
    game.pending_repel_application_expiry_tick = Some(2806);
    let keys = crate::input::RenderInputContext::default();
    let before = game.local_render_prediction(0.0042, &keys);
    game.expire_pending_repel_application_at_tick(2810);
    let after = game.local_render_prediction(0.0042, &keys);
    assert_eq!(
        after.velocity, before.velocity,
        "wall time must not clamp an already committed sequence early"
    );
    assert_eq!(game.repel_movement_sequence_window, Some((16177, 16313)));
    assert_eq!(game.pending_repel_application, None);
    assert_eq!(game.pending_repel_application_expiry_tick, None);
    assert_eq!(game.expired_repel_applications.front(), Some(&marker));
    assert_eq!(game.repel_delivery_expired_application_cancellations, 1);
    game.expire_pending_repel_application_at_tick(2811);
    assert_eq!(game.repel_delivery_expired_application_cancellations, 1);
}

#[test]
fn held_repel_expired_receipt_narrows_only_its_existing_logical_window() {
    for (start, next, offered_end, accepted_end) in
        [(16177, 16287, 16313, 16310), (u16::MAX - 4, 2, 100, 50)]
    {
        for current in [Some((start, offered_end)), None, Some((200, 220))] {
            let mut game = Game::new_network_authority_test();
            game.install_network_authority_test_scope(
                AuthorityScopeIdentity {
                    server_session_id: Some(77),
                    arena_stream_id: Some(3),
                    player_id: Some(1),
                    life_seq: Some(9),
                    alive: Some(true),
                },
                AuthorityReadiness {
                    joined: true,
                    arena_epoch: Some(3),
                    settings: true,
                    security: true,
                    map: true,
                },
                3,
            );
            game.next_input_seq = next;
            game.repel_movement_sequence_window = current;
            game.expired_repel_applications.push_back(
                crate::continuum_position::ClientRepelApplicationMarker {
                    snapshot_id: 959,
                    application_sequence: start,
                    delivery_generation: 1,
                },
            );
            let receipt = RepelApplicationAccepted {
                server_session_id: 77,
                life_seq: 9,
                delivery_generation: 1,
                snapshot_id: 959,
                application_sequence: start,
                accepted_end_sequence: accepted_end,
                active_steps: accepted_end.wrapping_sub(start),
            };
            game.apply_network_actions(&ClientProtocolActions {
                events: vec![ClientProtocolEvent::RepelApplicationAccepted(receipt)],
                outbound_packets: vec![],
                authority_admission: vec![],
            });
            let expected = if current == Some((start, offered_end)) {
                repel_receipt_movement_window(next, start, accepted_end, receipt.active_steps)
            } else {
                current
            };
            assert_eq!(game.repel_movement_sequence_window, expected);
            assert!(game.expired_repel_applications.is_empty());
            assert_eq!(game.repel_delivery_expired_receipts_recovered, 1);
            assert_eq!(game.consumed_repel_delivery_generations.len(), 1);
        }
    }
}

#[test]
fn expired_repel_marker_matches_a_lost_receipt_without_rearming_an_unrelated_generation() {
    let expired = std::collections::VecDeque::from([
        crate::continuum_position::ClientRepelApplicationMarker {
            snapshot_id: u16::MAX,
            application_sequence: u16::MAX,
            delivery_generation: u32::MAX,
        },
        crate::continuum_position::ClientRepelApplicationMarker {
            snapshot_id: 0,
            application_sequence: 7,
            delivery_generation: 1,
        },
    ]);
    let receipt = RepelApplicationAccepted {
        server_session_id: 77,
        life_seq: 9,
        delivery_generation: u32::MAX,
        snapshot_id: u16::MAX,
        application_sequence: u16::MAX,
        accepted_end_sequence: 0,
        active_steps: 1,
    };
    assert_eq!(
        expired_repel_application_receipt_index(Some(77), Some(9), &expired, receipt),
        Some(0),
    );
    assert_eq!(
        expired_repel_application_receipt_index(Some(77), Some(10), &expired, receipt),
        None,
        "a prior-life receipt cannot retire a current-life marker after wrap"
    );
    assert_eq!(
        expired_repel_application_receipt_index(
            Some(77),
            Some(9),
            &expired,
            RepelApplicationAccepted {
                delivery_generation: 1,
                ..receipt
            },
        ),
        None,
        "a different queued generation is never retired by the old receipt"
    );
}

#[test]
fn private_repel_provisional_window_clamps_only_a_lagging_clock_and_never_extends_expiry() {
    assert_eq!(
        private_repel_provisional_steps(100, 325, 135, 99),
        Some(135)
    );
    assert_eq!(
        private_repel_provisional_steps(100, 325, 135, 101),
        Some(135)
    );
    assert_eq!(private_repel_provisional_steps(100, 325, 135, 324), Some(1));
    assert_eq!(private_repel_provisional_steps(100, 325, 135, 325), None);
    assert_eq!(private_repel_provisional_steps(325, 325, 135, 324), None);
    assert_eq!(private_repel_provisional_steps(100, 325, 134, 100), None);

    let snapshot = crate::continuum::CONTINUUM_TICK_MASK - 2;
    let expiry = crate::continuum::continuum_tick_add(snapshot, 225);
    assert_eq!(
        private_repel_provisional_steps(
            snapshot,
            expiry,
            135,
            crate::continuum::continuum_tick_sub(snapshot, 1),
        ),
        Some(135),
    );
    assert_eq!(
        private_repel_provisional_steps(snapshot, expiry, 135, expiry),
        None
    );
    assert_eq!(repel_receipt_remaining_steps(101, 135), 35);
    assert_eq!(repel_receipt_remaining_steps(136, 135), 0);
    assert_eq!(repel_receipt_remaining_steps(u16::MAX, 1), 3);
    assert_eq!(
        repel_receipt_movement_window(101, 100, 135, 35),
        Some((100, 136)),
    );
    assert_eq!(
        repel_receipt_movement_window(136, 100, 135, 35),
        None,
        "a receipt at the exclusive end retires identity without reinstalling movement authority",
    );
    assert_eq!(
        repel_receipt_movement_window(137, 100, 135, 35),
        None,
        "an endpoint already behind next_input_seq cannot install a window",
    );
}

#[test]
fn browser_server_tick_remains_exact_while_network_owner_is_moved_for_poll() {
    assert_eq!(
        browser_server_tick_across_owner_poll(Some(101), Some(100)),
        Some(101)
    );
    assert_eq!(
        browser_server_tick_across_owner_poll(None, Some(u32::MAX)),
        Some(u32::MAX),
        "temporarily moving BrowserNetworkClient out of Game cannot erase its clock"
    );
    assert_eq!(browser_server_tick_across_owner_poll(None, None), None);
}

#[test]
fn delayed_frequency_receipt_rebases_before_next_wrapped_fixed_step() {
    let baseline = ClientPositionUpdate {
        direction: 10,
        timestamp: 0,
        x: 10_521,
        y: 8_192,
        velocity_x: 3_525,
        velocity_y: 0,
        togglables: 0,
        bounty: 17,
        energy: 1_700,
        weapon: 0,
        extra: None,
        canonical_state: crate::continuum_position::ClientPositionCanonicalState::from_legacy_wire(
            10, 10_521, 8_192, 3_525, 0,
        ),
        input_history: None,
        warp_collectible_id: None,
        repel_application: None,
    };
    let retained_accepted_baseline = baseline.clone();
    let mut ship = Ship {
        pos: vec2(baseline.x as f32, baseline.y as f32),
        vel: vec2(352.5, 0.0),
        orientation: crate::continuum_position::protocol_direction_to_orientation(10),
        energy: 1_700.0,
        ..Ship::default()
    };

    // Transport ACK arrives, but the application receipt is deliberately
    // delayed while held thrust continues render simulation behind the
    // realtime barrier.
    let mut next_input_sequence = u16::MAX;
    for _ in 0..3 {
        ship.pos.x += ship.vel.x * crate::fixed_step::GAMEPLAY_STEP_SECONDS;
        ship.vel.x += 2.5;
    }
    assert!(ship.pos.x > baseline.x as f32);
    assert_eq!(
        next_input_sequence,
        u16::MAX,
        "the barrier emitted no input"
    );

    // The exact application receipt discards every speculative held-motion
    // step. Browser authority is polled before gameplay and suppresses this
    // render frame, so no input is consumed beside the accepted boundary.
    let mut bounty = 0;
    let mut last_server_energy = None;
    apply_frequency_baseline_rebase(
        &mut ship,
        &mut bounty,
        &mut last_server_energy,
        &retained_accepted_baseline,
    );
    assert_eq!(ship.pos, vec2(10_521.0, 8_192.0));
    assert_eq!(ship.vel, vec2(352.5, 0.0));
    assert_eq!(next_input_sequence, u16::MAX);

    let mut suppress_receipt_frame = true;
    if !std::mem::take(&mut suppress_receipt_frame) {
        ship.pos.x += ship.vel.x * crate::fixed_step::GAMEPLAY_STEP_SECONDS;
        next_input_sequence = next_input_sequence.wrapping_add(1);
    }
    assert_eq!(next_input_sequence, u16::MAX);
    assert_eq!(ship.pos.x, 10_521.0);

    // The following render's fixed step advances once from that exact
    // baseline and the sequence wraps normally.
    ship.pos.x += ship.vel.x * crate::fixed_step::GAMEPLAY_STEP_SECONDS;
    next_input_sequence = next_input_sequence.wrapping_add(1);
    assert_eq!(next_input_sequence, 0);
    assert_eq!(
        ship.pos.x,
        10_521.0 + 352.5 * crate::fixed_step::GAMEPLAY_STEP_SECONDS
    );
}

#[test]
fn warp_acceptance_requires_exact_collectible_life_and_an_emitted_wrapped_sequence() {
    let emitted = std::collections::VecDeque::from([u16::MAX, 0]);
    let receipt = WarpRelocationAccepted {
        collectible_id: 91,
        life_seq: 41,
        position_sequence: 0,
    };
    assert!(warp_relocation_receipt_matches(
        Some(91),
        Some(41),
        &emitted,
        receipt,
    ));
    for forged in [
        WarpRelocationAccepted {
            collectible_id: 90,
            ..receipt
        },
        WarpRelocationAccepted {
            life_seq: 40,
            ..receipt
        },
        WarpRelocationAccepted {
            position_sequence: 1,
            ..receipt
        },
    ] {
        assert!(!warp_relocation_receipt_matches(
            Some(91),
            Some(41),
            &emitted,
            forged,
        ));
    }
}

#[test]
fn security_door_sync_never_rewinds_to_an_older_bootstrap_tick() {
    assert_eq!(security_door_sync_tick(None, 1_000), 1_000);
    assert_eq!(security_door_sync_tick(Some(999), 1_000), 1_000);
    assert_eq!(security_door_sync_tick(Some(1_001), 1_000), 1_001);

    let mask = 0x7fff_ffff;
    assert_eq!(
        security_door_sync_tick(Some(2), mask - 3),
        2,
        "a wrapped observed tick is newer than the challenge"
    );
    assert_eq!(
        security_door_sync_tick(Some(mask - 3), 2),
        2,
        "a pre-wrap cached bootstrap cannot rewind a post-wrap challenge"
    );
}

#[test]
fn security_challenge_is_exact_session_arena_generation_scoped_and_wrap_safe() {
    let generation = NetworkSecurityGeneration {
        server_session_id: 0x0102_0304_0506_0708,
        arena_epoch: 17,
    };
    let current = SecurityChallenge {
        timestamp: u32::MAX - 1,
        door_seed: 11,
        prize_seed: 12,
        checksum_key: 13,
    };
    let wrapped_new = SecurityChallenge {
        timestamp: 2,
        door_seed: 21,
        ..current
    };
    assert!(generation.admits_challenge(
        Some(generation.server_session_id),
        generation.arena_epoch,
        current,
        None,
    ));
    assert!(generation.admits_challenge(
        Some(generation.server_session_id),
        generation.arena_epoch,
        wrapped_new,
        Some(current),
    ));
    assert!(
        generation.admits_challenge(
            Some(generation.server_session_id),
            generation.arena_epoch,
            SecurityChallenge {
                timestamp: u32::MAX - 4,
                ..current
            },
            None,
        ),
        "the timestamp is the stable arena door anchor, not packet issue time; an early snapshot must not make a current-scope challenge stale"
    );

    for (session, epoch, challenge, prior) in [
        (Some(generation.server_session_id + 1), 17, current, None),
        (Some(generation.server_session_id), 18, current, None),
        (None, 17, current, None),
        (
            Some(generation.server_session_id),
            17,
            SecurityChallenge {
                timestamp: u32::MAX - 2,
                ..current
            },
            Some(current),
        ),
    ] {
        assert!(
            !generation.admits_challenge(session, epoch, challenge, prior),
            "old-session, old-arena, pre-generation, and reordered challenges stay closed",
        );
    }

    let rejoined = NetworkSecurityGeneration {
        arena_epoch: generation.arena_epoch + 1,
        ..generation
    };
    assert!(
        !rejoined.admits_challenge(
            Some(generation.server_session_id),
            generation.arena_epoch,
            wrapped_new,
            None,
        ),
        "a same-RTC leave/rejoin cannot reuse the prior arena challenge",
    );
    let reconnected = NetworkSecurityGeneration {
        server_session_id: generation.server_session_id + 1,
        arena_epoch: 1,
    };
    assert!(
        !reconnected.admits_challenge(
            Some(generation.server_session_id),
            reconnected.arena_epoch,
            wrapped_new,
            None,
        ),
        "a reused Game cannot bind an old RTC session challenge",
    );
}

#[test]
fn realtime_gameplay_waits_for_the_whole_authenticated_zone_including_the_lvz_tail() {
    // The LVZ tail used to be optional for gameplay, and the predicate was
    // named for that contract. A retained Chrome/Chaos capture of the ordinary
    // client URL then recorded a deterministic 158 ms frame gap about 1.1 s
    // after the first accepted input, profiled to `LvzRenderCache::merge_archive`
    // resolving the arena's 28,922 map objects while the ship was already
    // flying. Gameplay now waits for the published file set as well as the map.
    assert!(!arena_gameplay_ready(false, false, false, false, false));
    assert!(!arena_gameplay_ready(true, false, false, true, true));
    assert!(!arena_gameplay_ready(false, true, true, true, true));
    assert!(!arena_gameplay_ready(true, true, false, true, true));
    assert!(!arena_gameplay_ready(true, true, true, false, true));
    assert!(
        !arena_gameplay_ready(true, true, true, true, false),
        "a published map with the authored archives still streaming is not an arena to fly in",
    );
    assert!(arena_gameplay_ready(true, true, true, true, true));
}

fn death(killer_id: u16, killed_id: u16, bounty: u16) -> PlayerDeath {
    PlayerDeath {
        green_id: -1,
        green_collectible_id: 0,
        killer_id,
        killed_id,
        bounty,
        flag_transfer: 0,
        life_seq: 1,
        respawn_at_tick: 100,
    }
}

#[test]
fn arena_kill_notice_reaches_uninvolved_player_and_spectator_exactly_once() {
    for spectator in [false, true] {
        for killer_id in [7, 12, 768] {
            let mut game = bounty_authority_test_game();
            game.spectator_mode = spectator;
            let roster = |id, name: &str| PlayerRosterEntry {
                ship: 0,
                audio: 0,
                name: name.into(),
                squad: String::new(),
                kill_points: 0,
                flag_points: 0,
                player_id: id,
                frequency: id,
                wins: 0,
                losses: 0,
                attach_parent: INVALID_PLAYER_ID,
                flags: 0,
                koth: 0,
            };
            let mut notice = death(killer_id, 13, 30);
            notice.life_seq = 2;
            let events = vec![
                ClientProtocolEvent::PlayerEntered(roster(killer_id, "killer")),
                ClientProtocolEvent::PlayerEntered(roster(13, "victim")),
                ClientProtocolEvent::PlayerDeath(notice),
                ClientProtocolEvent::PlayerDeath(notice),
            ];
            game.apply_network_actions(&ClientProtocolActions {
                outbound_packets: Vec::new(),
                events,
                authority_admission: Vec::new(),
            });
            assert_eq!(game.combat_kill_notification_count, 1);
            assert_eq!(
                game.last_combat_kill_notification.as_deref(),
                Some("victim(30) killed by: killer")
            );
            let notices = game
                .notifications
                .iter()
                .filter(|n| n.message == "victim(30) killed by: killer")
                .collect::<Vec<_>>();
            assert_eq!(notices.len(), 1);
            assert_eq!(
                notices[0].color,
                if killer_id == 7 {
                    Color::from_rgba(255, 220, 95, 255)
                } else {
                    Color::from_rgba(120, 220, 150, 255)
                }
            );
            assert_eq!(
                game.player_bounty, 30,
                "public notices cannot mutate private bounty"
            );
        }
    }
}

#[test]
fn global_death_notices_do_not_infer_private_bounty_for_any_killer_role() {
    for (killer, victim, victim_bounty) in [(7, 9, 500), (7, 9, 0), (8, 9, 500)] {
        let mut game = bounty_authority_test_game();
        game.player_bounty = 505;
        game.apply_authoritative_player_death(death(killer, victim, victim_bounty));
        assert_eq!(
            game.player_bounty, 505,
            "a global notice cannot infer the killer's current-life benefit"
        );
    }
}

#[test]
fn lifecycle_authority_is_exact_next_wrap_safe_and_idempotent() {
    let dead = |life_seq| SnapshotLifecycleState {
        life_seq,
        alive: false,
        respawn_at_tick: Some(42),
        repel_until_tick: None,
        repel_delivery: None,
    };
    let alive = |life_seq| SnapshotLifecycleState {
        life_seq,
        alive: true,
        respawn_at_tick: None,
        repel_until_tick: None,
        repel_delivery: None,
    };

    assert_eq!(
        authoritative_lifecycle_action(None, None, dead(u32::MAX)),
        LifecycleAction::StartDeath
    );
    assert_eq!(
        authoritative_lifecycle_action(Some(u32::MAX), Some(false), dead(u32::MAX)),
        LifecycleAction::Ignore,
        "snapshot/reliable duplicate cannot replay death"
    );
    assert_eq!(
        authoritative_lifecycle_action(Some(u32::MAX), Some(false), alive(u32::MAX)),
        LifecycleAction::Observe,
        "the exact same generation may transition dead -> alive"
    );
    assert_eq!(
        authoritative_lifecycle_action(Some(u32::MAX), Some(true), dead(1)),
        LifecycleAction::StartDeath,
        "reserved-zero life sequencing wraps MAX -> 1"
    );
    assert_eq!(
        authoritative_lifecycle_action(Some(u32::MAX), Some(true), dead(2)),
        LifecycleAction::Ignore,
        "a reordered/forged lifecycle update cannot skip a generation"
    );
    assert_eq!(
        authoritative_lifecycle_action(Some(1), Some(true), dead(u32::MAX)),
        LifecycleAction::Ignore,
        "an old pre-wrap death cannot rewind a live generation"
    );
}

#[test]
fn authoritative_respawn_deadline_is_wrap_safe_and_never_extends() {
    assert_eq!(
        authoritative_respawn_delay_seconds(Some(5), u32::MAX - 4),
        0.1
    );
    assert_eq!(authoritative_respawn_delay_seconds(Some(5), 7), 0.0);
    assert_eq!(authoritative_respawn_delay_seconds(None, 7), 0.0);
}

#[test]
fn saturated_private_bounty_is_not_incremented_by_a_later_global_notice() {
    let mut game = bounty_authority_test_game();
    let prize = SnapshotPrizeState {
        life_seq: 5,
        revision: 8,
        bounty: u16::MAX,
        max_energy: 1700,
        energy: Some(1700),
        ..SnapshotPrizeState::default()
    };
    assert!(game.apply_authoritative_prize_state(prize, 5491));
    game.apply_authoritative_player_death(death(7, 9, 1));
    assert_eq!(
        game.player_bounty,
        u32::from(u16::MAX),
        "the server's saturated wire bounty remains exact"
    );
    assert!(game.apply_authoritative_prize_state(prize, 5492));
    assert_eq!(
        game.player_bounty,
        u32::from(u16::MAX),
        "repeated private authority cannot award the kill again"
    );
}

#[test]
fn arena_settings_update_raises_ship_max_energy_floor() {
    let mut tuning = ShipTuning::continuum_skirmish_default();
    tuning.energy_initial = 1500.0;
    tuning.energy_max = 2000.0;
    assert_eq!(
        max_energy_after_settings_update(1000.0, tuning, false),
        1500.0
    );
}

#[test]
fn arena_settings_update_does_not_lower_existing_ship_max_energy() {
    let mut tuning = ShipTuning::continuum_skirmish_default();
    tuning.energy_initial = 900.0;
    tuning.energy_max = 1200.0;
    assert_eq!(
        max_energy_after_settings_update(1500.0, tuning, false),
        1500.0
    );
}

#[test]
fn first_network_arena_settings_adopt_server_initial_energy() {
    let mut tuning = ShipTuning::continuum_skirmish_default();
    tuning.energy_initial = 1000.0;
    tuning.energy_max = 1700.0;
    assert_eq!(
        max_energy_after_settings_update(1700.0, tuning, true),
        1000.0
    );
}

#[test]
fn original_special_chat_sound_maps_continuum_music_ids() {
    assert_eq!(
        original_special_chat_sound(100),
        Some(OriginalSpecialChatSound::MusicLoop)
    );
    assert_eq!(
        original_special_chat_sound(101),
        Some(OriginalSpecialChatSound::MusicStop)
    );
    assert_eq!(
        original_special_chat_sound(102),
        Some(OriginalSpecialChatSound::MusicOnce)
    );
    assert_eq!(
        original_special_chat_sound(104),
        Some(OriginalSpecialChatSound::Goal)
    );
    assert_eq!(original_special_chat_sound(3), None);
}

#[test]
fn network_position_timer_preserves_elapsed_remainder() {
    let interval = 1.0 / 60.0;
    let frame_dt = 0.016;
    let mut timer = 0.0;
    let mut sends = 0;

    for _ in 0..120 {
        if advance_network_position_timer(&mut timer, frame_dt, interval) {
            sends += 1;
        }
    }

    assert_eq!(sends, 116);
}

#[test]
fn network_position_timer_does_not_catch_up_after_long_pause() {
    let interval = 1.0 / 60.0;
    let mut timer = interval;

    assert!(advance_network_position_timer(&mut timer, 1.0, interval));
    assert_eq!(timer, 0.0);
    assert!(advance_network_position_timer(&mut timer, 0.0, interval));
}

#[test]
fn correction_and_lifecycle_transaction_events_apply_before_gameplay() {
    let correction = MovementCorrection {
        reason: MovementCorrection::REASON_SUSTAINED_DRIFT,
        repel_window: None,
        server_session_id: 7,
        player_id: 3,
        life_seq: 11,
        correction_generation: 13,
        position_sequence: 17,
        server_tick: 19,
        position_x: 1_024.5,
        position_y: 2_048.25,
        velocity_x: 3.0,
        velocity_y: -4.0,
        orientation: 0.5,
    };
    let receipt = MovementCorrectionAccepted {
        server_tick: correction.server_tick,
        outcome: MovementCorrectionAccepted::OUTCOME_APPLIED,
        server_session_id: correction.server_session_id,
        player_id: correction.player_id,
        life_seq: correction.life_seq,
        correction_generation: correction.correction_generation,
        detected_sequence: correction.position_sequence,
        application_sequence: correction.position_sequence.wrapping_add(9),
        position_x: correction.position_x + 3.0,
        position_y: correction.position_y + 4.0,
        velocity_x: correction.velocity_x + 5.0,
        velocity_y: correction.velocity_y + 6.0,
        orientation: 0.75,
    };

    assert!(network_event_applies_before_gameplay(
        &ClientProtocolEvent::MovementCorrected(correction)
    ));
    assert!(network_event_applies_before_gameplay(
        &ClientProtocolEvent::MovementCorrectionAccepted(receipt)
    ));
    assert!(network_event_applies_before_gameplay(
        &ClientProtocolEvent::LifecycleBaselineAccepted(LifecycleBaselineAccepted {
            client_reliable_delivery_id: u32::MAX,
            life_seq: 41,
            position_sequence: 0,
            accepted_server_tick: 700,
            accepted_position: None,
        })
    ));
    assert!(!network_event_applies_before_gameplay(
        &ClientProtocolEvent::KeepAlive
    ));
}

#[test]
fn browser_partition_extracts_only_private_prize_state_in_exact_ack_terminal_order() {
    let ack = PrizePickupAck {
        server_session_id: 77,
        arena_stream_id: 3,
        player_id: 2,
        collectible_id: 88,
        x: 8_136,
        y: 8_280,
        prize_id: PrizeKind::EngineShutdown.protocol_id(),
        authoritative_energy: 1_234,
        life_seq: 9,
        prize_state_revision: 17,
        engine_shutdown_until_tick: Some(5_000),
        engine_shutdown_application_required: true,
    };
    let application = PrizeEffectApplied {
        server_session_id: ack.server_session_id,
        player_id: ack.player_id,
        life_seq: ack.life_seq,
        collectible_id: ack.collectible_id,
        prize_id: ack.prize_id,
        prize_state_revision: ack.prize_state_revision,
        application_sequence: 41,
    };
    let terminal = PrizeEffectAccepted::from(application);
    let prize = SnapshotPrizeState {
        life_seq: ack.life_seq,
        revision: ack.prize_state_revision,
        max_energy: 1_700,
        rotation_bits: 0.8_f32.to_bits(),
        thrust_bits: 240.0_f32.to_bits(),
        top_speed_bits: 400.0_f32.to_bits(),
        recharge_bits: 300.0_f32.to_bits(),
        engine_shutdown_until_tick: Some(5_500),
        ..SnapshotPrizeState::default()
    };
    let lifecycle = SnapshotLifecycleState {
        life_seq: ack.life_seq,
        alive: true,
        respawn_at_tick: None,
        repel_until_tick: Some(5_600),
        repel_delivery: Some(SnapshotRepelDelivery {
            active_steps: 10,
            delivery_generation: 3,
            source_player_id: 7,
            source_life_seq: 5,
            origin_x: 8_000,
            origin_y: 8_000,
            source_frequency: 1,
            expected_velocity_x_bits: 500.0_f32.to_bits(),
            expected_velocity_y_bits: 0.0_f32.to_bits(),
            weapon_data: 1,
            source_ship: 0,
        }),
    };
    let snapshot = SnapshotFrame {
        header: SnapshotHeader {
            snapshot_id: 123,
            tick: 4_900,
            arena_stream_id: ack.arena_stream_id,
            ..SnapshotHeader::default()
        },
        near_entities: vec![SnapshotEntity {
            entity_id: 2,
            field_mask: SnapshotEntity::FIELD_WEAPON,
            weapon: Some(66),
            ..SnapshotEntity::default()
        }],
        events: vec![
            SnapshotEvent::PrizeState(prize),
            SnapshotEvent::RocketState(SnapshotRocketState {
                life_seq: ack.life_seq,
                active: true,
                rockets: 2,
                ..SnapshotRocketState::default()
            }),
            SnapshotEvent::LifecycleState(lifecycle),
        ],
        ..SnapshotFrame::default()
    };
    let actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::PrizePickupAck(scoped_prize_pickup_ack(ack, 77, 3, 2)),
            ClientProtocolEvent::SnapshotReceived(snapshot.clone()),
            ClientProtocolEvent::PrizeEffectAccepted(terminal),
        ],
        authority_admission: Vec::new(),
    };
    let (pre, deferred) = partition_browser_network_actions(
        actions,
        Some(ack.server_session_id),
        Some(ack.arena_stream_id),
        Some(ack.player_id),
        Some(ack.life_seq),
        Some(true),
        true,
        Some(1),
        true,
        true,
        true,
        false,
        false,
    );
    assert_eq!(pre.events.len(), 3);
    assert!(matches!(
        pre.events[0],
        ClientProtocolEvent::PrizePickupAck(_)
    ));
    assert!(matches!(
        pre.events[1],
        ClientProtocolEvent::PreGameplayPrizeState {
            snapshot_id: 123,
            snapshot_tick: 4_900,
            state,
        } if state == prize
    ));
    assert!(matches!(
        pre.events[2],
        ClientProtocolEvent::PrizeEffectAccepted(candidate) if candidate == terminal
    ));
    assert_eq!(deferred.events.len(), 1);
    let ClientProtocolEvent::SnapshotReceived(deferred_snapshot) = &deferred.events[0] else {
        panic!("the original snapshot remains the sole deferred event");
    };
    assert_eq!(deferred_snapshot.header, snapshot.header);
    assert_eq!(deferred_snapshot.near_entities, snapshot.near_entities);
    assert_eq!(deferred_snapshot.near_entities[0].weapon, Some(66));
    assert_eq!(
        deferred_snapshot.events,
        vec![
            SnapshotEvent::RocketState(SnapshotRocketState {
                life_seq: ack.life_seq,
                active: true,
                rockets: 2,
                ..SnapshotRocketState::default()
            }),
            SnapshotEvent::LifecycleState(lifecycle),
        ],
        "Rocket, lifecycle, Repel, weapon, and entity state remain post-step",
    );
    assert!(
        deferred_snapshot
            .events
            .iter()
            .all(|event| !matches!(event, SnapshotEvent::PrizeState(_))),
        "the private state has exactly one application carrier"
    );

    let reordered = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::PrizeEffectAccepted(terminal),
            ClientProtocolEvent::SnapshotReceived(snapshot.clone()),
            ClientProtocolEvent::PrizePickupAck(scoped_prize_pickup_ack(ack, 77, 3, 2)),
        ],
        authority_admission: Vec::new(),
    };
    let (reordered_pre, reordered_deferred) = partition_browser_network_actions(
        reordered,
        Some(ack.server_session_id),
        Some(ack.arena_stream_id),
        Some(ack.player_id),
        Some(ack.life_seq),
        Some(true),
        true,
        Some(1),
        true,
        true,
        true,
        false,
        false,
    );
    assert!(matches!(
        reordered_pre.events.as_slice(),
        [
            ClientProtocolEvent::PrizeEffectAccepted(candidate),
            ClientProtocolEvent::PreGameplayPrizeState {
                snapshot_id: 123,
                snapshot_tick: 4_900,
                state,
            },
            ClientProtocolEvent::PrizePickupAck(_),
        ] if *candidate == terminal && *state == prize
    ));
    assert_eq!(reordered_deferred.events.len(), 1);

    let forward_life = ack.life_seq.wrapping_add(1).max(1);
    let forward_prize = SnapshotPrizeState {
        life_seq: forward_life,
        revision: 1,
        ..prize
    };
    let forward_lifecycle = SnapshotLifecycleState {
        life_seq: forward_life,
        alive: true,
        ..SnapshotLifecycleState::default()
    };
    let forward_snapshot = SnapshotFrame {
        header: SnapshotHeader {
            snapshot_id: 124,
            tick: 4_901,
            arena_stream_id: ack.arena_stream_id,
            ..SnapshotHeader::default()
        },
        events: vec![
            SnapshotEvent::LifecycleState(forward_lifecycle),
            SnapshotEvent::PrizeState(forward_prize),
        ],
        ..SnapshotFrame::default()
    };
    let (forward_pre, forward_deferred) = partition_browser_network_actions(
        ClientProtocolActions {
            outbound_packets: Vec::new(),
            events: vec![ClientProtocolEvent::SnapshotReceived(
                forward_snapshot.clone(),
            )],
            authority_admission: Vec::new(),
        },
        Some(ack.server_session_id),
        Some(ack.arena_stream_id),
        Some(ack.player_id),
        Some(ack.life_seq),
        Some(true),
        true,
        Some(1),
        true,
        true,
        true,
        false,
        false,
    );
    assert!(forward_pre.events.is_empty());
    assert!(
        matches!(
            forward_deferred.events.as_slice(),
            [
                ClientProtocolEvent::SnapshotReceived(candidate),
                ClientProtocolEvent::PreGameplayPrizeState { state, .. },
            ] if candidate.events == vec![SnapshotEvent::LifecycleState(forward_lifecycle)]
                && *state == forward_prize
        ),
        "a forward-life PrizeState is staged exactly once behind its post-step lifecycle boundary"
    );

    for (alive, respawn_pending, cutover_pending) in
        [(Some(true), false, true), (Some(false), true, false)]
    {
        let current_snapshot = SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id: 125,
                tick: 4_902,
                arena_stream_id: ack.arena_stream_id,
                ..SnapshotHeader::default()
            },
            events: vec![SnapshotEvent::PrizeState(prize)],
            ..SnapshotFrame::default()
        };
        let (blocked_pre, blocked_deferred) = partition_browser_network_actions(
            ClientProtocolActions {
                outbound_packets: Vec::new(),
                events: vec![ClientProtocolEvent::SnapshotReceived(
                    current_snapshot.clone(),
                )],
                authority_admission: Vec::new(),
            },
            Some(ack.server_session_id),
            Some(ack.arena_stream_id),
            Some(ack.player_id),
            Some(ack.life_seq),
            alive,
            true,
            Some(1),
            true,
            true,
            true,
            respawn_pending,
            cutover_pending,
        );
        assert!(blocked_pre.events.is_empty());
        assert_eq!(prize_state_carrier_count(&blocked_deferred), 0);
        assert!(matches!(
            blocked_deferred.events.as_slice(),
            [ClientProtocolEvent::SnapshotReceived(candidate)] if candidate.events.is_empty()
        ));
    }
}

fn private_prize_snapshot(
    snapshot_id: u16,
    stream_id: u64,
    prize: SnapshotPrizeState,
    lifecycle: Option<SnapshotLifecycleState>,
) -> ClientProtocolEvent {
    let mut events = vec![SnapshotEvent::PrizeState(prize)];
    if let Some(lifecycle) = lifecycle {
        events.push(SnapshotEvent::LifecycleState(lifecycle));
    }
    ClientProtocolEvent::SnapshotReceived(SnapshotFrame {
        header: SnapshotHeader {
            snapshot_id,
            tick: 4_900,
            arena_stream_id: stream_id,
            ..SnapshotHeader::default()
        },
        events,
        ..SnapshotFrame::default()
    })
}

fn partition_for_private_prize_scope(
    events: Vec<ClientProtocolEvent>,
    session_id: u64,
    stream_id: u64,
    player_id: u16,
    life_seq: u32,
    alive: bool,
) -> (ClientProtocolActions, ClientProtocolActions) {
    partition_browser_network_actions(
        ClientProtocolActions {
            outbound_packets: Vec::new(),
            events,
            authority_admission: Vec::new(),
        },
        Some(session_id),
        Some(stream_id),
        Some(player_id),
        Some(life_seq),
        Some(alive),
        true,
        Some(1),
        true,
        true,
        true,
        false,
        false,
    )
}

fn prize_state_carrier_count(actions: &ClientProtocolActions) -> usize {
    actions
        .events
        .iter()
        .map(|event| match event {
            ClientProtocolEvent::PreGameplayPrizeState { .. } => 1,
            ClientProtocolEvent::SnapshotReceived(frame) => frame
                .events
                .iter()
                .filter(|event| matches!(event, SnapshotEvent::PrizeState(_)))
                .count(),
            _ => 0,
        })
        .sum()
}

#[test]
fn whole_batch_lifecycle_and_stream_overtake_defers_private_prize_in_both_orders() {
    let session = u64::MAX;
    let stream = u64::MAX;
    let player = 9;
    let life = u32::MAX;
    let prize = SnapshotPrizeState {
        life_seq: life,
        revision: 17,
        max_energy: 1_700,
        ..SnapshotPrizeState::default()
    };
    let current_snapshot = private_prize_snapshot(100, stream, prize, None);
    let next_life = life.wrapping_add(1).max(1);
    let replacement_lifecycle = |candidate_life, alive| {
        ClientProtocolEvent::SnapshotReceived(SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id: 101,
                tick: 4_901,
                arena_stream_id: stream,
                ..SnapshotHeader::default()
            },
            events: vec![SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                life_seq: candidate_life,
                alive,
                respawn_at_tick: (!alive).then_some(5_200),
                repel_until_tick: None,
                repel_delivery: None,
            })],
            ..SnapshotFrame::default()
        })
    };
    let death = ClientProtocolEvent::PlayerDeath(PlayerDeath {
        green_id: 0,
        green_collectible_id: 0,
        killer_id: 7,
        killed_id: player,
        bounty: 500,
        flag_transfer: 0,
        life_seq: next_life,
        respawn_at_tick: 5_200,
    });
    let boundaries = [
        ("same-player death", death, false),
        (
            "forward-life snapshot across life wrap",
            replacement_lifecycle(next_life, true),
            false,
        ),
        (
            "joined arena",
            ClientProtocolEvent::JoinedArena(u32::MAX),
            true,
        ),
        ("left arena", ClientProtocolEvent::PlayerLeft(player), true),
        ("disconnect", ClientProtocolEvent::Disconnected, true),
        (
            "session replacement across wrap",
            ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
                server_session_id: 1,
                arena_stream_id: stream,
                ..ServerClockBootstrap::default()
            }),
            true,
        ),
        (
            "arena-stream replacement across wrap",
            ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
                server_session_id: session,
                arena_stream_id: 1,
                ..ServerClockBootstrap::default()
            }),
            true,
        ),
        (
            "replacement player/bootstrap scope",
            ClientProtocolEvent::PlayerIdAssigned(player.wrapping_add(1)),
            true,
        ),
    ];

    for (label, boundary, hard_replacement) in boundaries {
        for events in [
            vec![current_snapshot.clone(), boundary.clone()],
            vec![boundary.clone(), current_snapshot.clone()],
        ] {
            let actions = ClientProtocolActions {
                outbound_packets: Vec::new(),
                events: events.clone(),
                authority_admission: Vec::new(),
            };
            let admission = build_authority_admission_plan(
                &actions,
                AuthorityAdmissionSeed {
                    scope: AuthorityScopeIdentity {
                        server_session_id: Some(session),
                        arena_stream_id: Some(stream),
                        player_id: Some(player),
                        life_seq: Some(life),
                        alive: Some(true),
                    },
                    readiness: AuthorityReadiness::default(),
                    segment: 0,
                },
            );
            assert!(
                admission
                    .iter()
                    .any(|entry| entry.boundary != AuthorityBoundaryAdmission::None),
                "{label}: scope is overtaken"
            );
            assert_eq!(
                admission
                    .iter()
                    .any(|entry| matches!(entry.boundary, AuthorityBoundaryAdmission::Hard { .. })),
                hard_replacement,
                "{label}: lifecycle transitions defer canonically, while stream/player replacements cancel old private state post-step",
            );
            let (pre, deferred) =
                partition_for_private_prize_scope(events, session, stream, player, life, true);
            assert!(
                pre.events.iter().all(|event| !matches!(
                    event,
                    ClientProtocolEvent::PreGameplayPrizeState { .. }
                )),
                "{label} must overtake private state in either merged-channel order",
            );
            assert_eq!(
                prize_state_carrier_count(&pre),
                0,
                "{label}: no pre carrier"
            );
            assert_eq!(
                prize_state_carrier_count(&deferred),
                0,
                "{label}: old-scope state is rejected"
            );
        }
    }

    let invalid_unbootstrapped_stream = private_prize_snapshot(102, 1, prize, None);
    let (pre, deferred) = partition_for_private_prize_scope(
        vec![invalid_unbootstrapped_stream],
        session,
        stream,
        player,
        life,
        true,
    );
    assert_eq!(prize_state_carrier_count(&pre), 0);
    assert_eq!(prize_state_carrier_count(&deferred), 0);

    let current_life_respawn = replacement_lifecycle(life, true);
    for events in [
        vec![current_snapshot.clone(), current_life_respawn.clone()],
        vec![current_life_respawn, current_snapshot.clone()],
    ] {
        let actions = ClientProtocolActions {
            outbound_packets: Vec::new(),
            events: events.clone(),
            authority_admission: Vec::new(),
        };
        let admission = build_authority_admission_plan(
            &actions,
            AuthorityAdmissionSeed {
                scope: AuthorityScopeIdentity {
                    server_session_id: Some(session),
                    arena_stream_id: Some(stream),
                    player_id: Some(player),
                    life_seq: Some(life),
                    alive: Some(false),
                },
                readiness: AuthorityReadiness::default(),
                segment: 0,
            },
        );
        assert!(
            admission
                .iter()
                .any(|entry| entry.boundary == AuthorityBoundaryAdmission::Soft)
        );
        assert!(
            !admission
                .iter()
                .any(|entry| matches!(entry.boundary, AuthorityBoundaryAdmission::Hard { .. }))
        );
        let (pre, deferred) =
            partition_for_private_prize_scope(events, session, stream, player, life, false);
        assert_eq!(prize_state_carrier_count(&pre), 0);
        assert_eq!(
            prize_state_carrier_count(&deferred),
            1,
            "the current-life respawn baseline is staged after its soft lifecycle boundary",
        );
    }
}

#[test]
fn whole_batch_overtake_is_self_scoped_and_preserves_unaffected_exact_order_once() {
    let session = 77;
    let stream = 3;
    let player = 9;
    let life = 41;
    let prize = SnapshotPrizeState {
        life_seq: life,
        revision: 17,
        max_energy: 1_700,
        ..SnapshotPrizeState::default()
    };
    let ack = PrizePickupAck {
        server_session_id: session,
        arena_stream_id: stream,
        player_id: player,
        collectible_id: 88,
        x: 8_136,
        y: 8_280,
        prize_id: PrizeKind::EngineShutdown.protocol_id(),
        authoritative_energy: 1_234,
        life_seq: life,
        prize_state_revision: prize.revision,
        engine_shutdown_until_tick: Some(5_000),
        engine_shutdown_application_required: true,
    };
    let application = PrizeEffectApplied {
        server_session_id: session,
        player_id: player,
        life_seq: life,
        collectible_id: ack.collectible_id,
        prize_id: ack.prize_id,
        prize_state_revision: ack.prize_state_revision,
        application_sequence: u16::MAX,
    };
    let events = vec![
        ClientProtocolEvent::PrizePickupAck(scoped_prize_pickup_ack(
            ack,
            session,
            u32::try_from(stream).expect("test stream fits"),
            player,
        )),
        ClientProtocolEvent::PlayerDeath(PlayerDeath {
            green_id: 0,
            green_collectible_id: 0,
            killer_id: player,
            killed_id: player.wrapping_add(1),
            bounty: 1,
            flag_transfer: 0,
            life_seq: life.wrapping_add(1),
            respawn_at_tick: 5_200,
        }),
        private_prize_snapshot(u16::MAX, stream, prize, None),
        ClientProtocolEvent::PlayerLeft(player.wrapping_add(2)),
        ClientProtocolEvent::PrizeEffectAccepted(PrizeEffectAccepted::from(application)),
    ];
    let (pre, deferred) =
        partition_for_private_prize_scope(events, session, stream, player, life, true);
    assert!(matches!(
        pre.events.as_slice(),
        [
            ClientProtocolEvent::PrizePickupAck(_),
            ClientProtocolEvent::PreGameplayPrizeState {
                snapshot_id: u16::MAX,
                state,
                ..
            },
            ClientProtocolEvent::PrizeEffectAccepted(_),
        ] if *state == prize
    ));
    assert_eq!(prize_state_carrier_count(&pre), 1);
    assert_eq!(prize_state_carrier_count(&deferred), 0);
    assert_eq!(
        deferred.events.len(),
        3,
        "unrelated death/leave and the stripped snapshot stay post-step",
    );

    let overtaken = vec![
        ClientProtocolEvent::PrizePickupAck(scoped_prize_pickup_ack(
            ack,
            session,
            u32::try_from(stream).expect("test stream fits"),
            player,
        )),
        private_prize_snapshot(u16::MAX, stream, prize, None),
        ClientProtocolEvent::PrizeEffectAccepted(PrizeEffectAccepted::from(application)),
        ClientProtocolEvent::JoinedArena(1),
    ];
    let (overtaken_pre, overtaken_deferred) =
        partition_for_private_prize_scope(overtaken, session, stream, player, life, true);
    assert_eq!(
        overtaken_pre.events,
        vec![ClientProtocolEvent::PreGameplayAuthorityReset],
    );
    assert!(matches!(
        overtaken_deferred.events.as_slice(),
        [
            ClientProtocolEvent::PrizePickupAck(_),
            ClientProtocolEvent::SnapshotReceived(frame),
            ClientProtocolEvent::PrizeEffectAccepted(_),
            ClientProtocolEvent::JoinedArena(1),
        ] if frame.events.is_empty()
    ));
    assert_eq!(
        prize_state_carrier_count(&overtaken_deferred),
        0,
        "old-scope private state is rejected instead of rebound after the join",
    );
}

#[test]
fn overtaken_private_prize_cannot_change_the_current_fixed_step_or_weapon_inventory() {
    let session = 77;
    let stream = 3;
    let player = 9;
    let life = 41;
    let tick = 5_000;
    let stale = SnapshotPrizeState {
        life_seq: life,
        revision: 18,
        max_energy: 1_700,
        rotation_bits: 2.0_f32.to_bits(),
        thrust_bits: 900.0_f32.to_bits(),
        top_speed_bits: 900.0_f32.to_bits(),
        recharge_bits: 900.0_f32.to_bits(),
        gun_level: 3,
        engine_shutdown_until_tick: None,
        ..SnapshotPrizeState::default()
    };
    let (pre, deferred) = partition_for_private_prize_scope(
        vec![
            private_prize_snapshot(100, stream, stale, None),
            ClientProtocolEvent::PlayerDeath(PlayerDeath {
                green_id: 0,
                green_collectible_id: 0,
                killer_id: 7,
                killed_id: player,
                bounty: 500,
                flag_transfer: 0,
                life_seq: life.wrapping_add(1),
                respawn_at_tick: 5_200,
            }),
        ],
        session,
        stream,
        player,
        life,
        true,
    );

    let mut deadline = Some(6_000);
    let mut rotation = 0.2;
    let mut thrust = 100.0;
    let mut recharge = 50.0;
    let mut gun_level = 1;
    for event in &pre.events {
        if let ClientProtocolEvent::PreGameplayPrizeState { state, .. } = event {
            deadline = state.engine_shutdown_until_tick;
            rotation = f32::from_bits(state.rotation_bits);
            thrust = f32::from_bits(state.thrust_bits);
            recharge = f32::from_bits(state.recharge_bits);
            gun_level = state.gun_level;
        }
    }
    let step = step_player_movement(
        PlayerMovementState {
            orientation: 0.0,
            velocity: MovementVector::ZERO,
            energy: 1_000.0,
            max_energy: 1_700.0,
        },
        PlayerMovementInput {
            thrust: true,
            rotate_right: true,
            ..PlayerMovementInput::default()
        },
        PlayerMovementTuning {
            rotation_per_second: rotation,
            shutdown_rotation_per_second: 0.1,
            base_thrust_acceleration: thrust,
            afterburner_thrust_acceleration: 300.0,
            rocket_thrust_acceleration: 500.0,
            base_top_speed: 400.0,
            afterburner_top_speed: 500.0,
            rocket_top_speed: 600.0,
            afterburner_energy_per_second: 120.0,
            recharge_per_second: recharge,
            turret_thrust_penalty: 40.0,
            turret_speed_penalty: 50.0,
            repel_top_speed: 800.0,
            flagger_thrust_adjustment: 25.0,
            flagger_speed_adjustment: 20.0,
        },
        PlayerMovementEnvironment {
            engine_shutdown: authoritative_prize_timer_seconds(deadline, tick) > 0.0,
            ..PlayerMovementEnvironment::default()
        },
        0.1,
    );
    assert_eq!(
        step.state.energy, 1_005.0,
        "the current recharge rate applies; the union only gates movement"
    );
    assert_eq!(
        step.state.velocity,
        MovementVector::ZERO,
        "stale thrust was not consumed"
    );
    assert!((step.state.orientation - 0.01).abs() < f32::EPSILON);
    assert_eq!(gun_level, 1, "stale weapon inventory was not consumed");
    assert_eq!(prize_state_carrier_count(&pre), 0);
    assert_eq!(prize_state_carrier_count(&deferred), 0);
}

#[test]
fn pre_gameplay_equal_revision_union_controls_the_immediately_ready_fixed_step() {
    let life = 9;
    let revision = 17;
    let tick = 5_000;
    let accepted = AcceptedPrizeEngineShutdown {
        life_seq: life,
        prize_state_revision: revision,
        until_tick: Some(9_000),
    };
    let authoritative_prize = |deadline| SnapshotPrizeState {
        life_seq: life,
        revision,
        max_energy: 1_700,
        rotation_bits: 0.8_f32.to_bits(),
        thrust_bits: 240.0_f32.to_bits(),
        top_speed_bits: 400.0_f32.to_bits(),
        recharge_bits: 300.0_f32.to_bits(),
        engine_shutdown_until_tick: deadline,
        ..SnapshotPrizeState::default()
    };
    let fixed_step = |prize: SnapshotPrizeState| {
        let deadline = authoritative_engine_shutdown_deadline_by_revision(
            Some((
                prize.life_seq,
                prize.revision,
                prize.engine_shutdown_until_tick,
                false,
            )),
            Some(accepted),
            [],
        );
        let outcome = step_player_movement(
            PlayerMovementState {
                orientation: 0.0,
                velocity: MovementVector::ZERO,
                energy: 1_000.0,
                max_energy: f32::from(prize.max_energy),
            },
            PlayerMovementInput {
                thrust: true,
                rotate_right: true,
                ..PlayerMovementInput::default()
            },
            PlayerMovementTuning {
                rotation_per_second: f32::from_bits(prize.rotation_bits),
                shutdown_rotation_per_second: 0.1,
                base_thrust_acceleration: f32::from_bits(prize.thrust_bits),
                afterburner_thrust_acceleration: 300.0,
                rocket_thrust_acceleration: 500.0,
                base_top_speed: f32::from_bits(prize.top_speed_bits),
                afterburner_top_speed: 500.0,
                rocket_top_speed: 600.0,
                afterburner_energy_per_second: 120.0,
                recharge_per_second: f32::from_bits(prize.recharge_bits),
                turret_thrust_penalty: 40.0,
                turret_speed_penalty: 50.0,
                repel_top_speed: 800.0,
                flagger_thrust_adjustment: 25.0,
                flagger_speed_adjustment: 20.0,
            },
            PlayerMovementEnvironment {
                engine_shutdown: authoritative_prize_timer_seconds(deadline, tick) > 0.0,
                ..PlayerMovementEnvironment::default()
            },
            0.1,
        );
        (deadline, outcome)
    };

    let (absent, unblocked) = fixed_step(authoritative_prize(None));
    assert_eq!(absent, None, "equal-revision absence retires continuity");
    assert_eq!(unblocked.state.energy, 1_030.0);
    assert!((unblocked.state.orientation - 0.08).abs() < f32::EPSILON);
    assert!((unblocked.state.velocity.y + 24.0).abs() < 0.001);

    for deadline in [Some(6_000), Some(11_000)] {
        let (selected, blocked) = fixed_step(authoritative_prize(deadline));
        assert_eq!(
            selected, deadline,
            "shorter or later Shutdown union wins now"
        );
        // The union blocks thrust, but a Shutdown *prize* still recharges
        // (`ShipController:244-259`); only EMP freezes recharge.
        assert_eq!(blocked.state.energy, 1_030.0);
        assert_eq!(blocked.state.velocity, MovementVector::ZERO);
        assert!((blocked.state.orientation - 0.01).abs() < f32::EPSILON);
    }

    let (_, expired) = fixed_step(authoritative_prize(Some(tick)));
    assert_eq!(expired.state.energy, 1_030.0);
    assert!((expired.state.velocity.y + 24.0).abs() < 0.001);

    assert!(prize_state_is_admissible_across_lifecycle_cutover(
        false,
        Some(life),
        life,
    ));
    assert!(!prize_state_is_admissible_across_lifecycle_cutover(
        false,
        Some(life),
        life.wrapping_add(1),
    ));
    assert!(!prize_state_is_admissible_across_lifecycle_cutover(
        true,
        Some(life),
        life,
    ));
}

#[test]
fn warp_and_private_repel_wait_for_the_movement_correction_terminal() {
    let warp = ClientProtocolEvent::PrizePickupAck(scoped_prize_pickup_ack(
        PrizePickupAck {
            server_session_id: 77,
            arena_stream_id: 3,
            player_id: 2,
            collectible_id: 7,
            x: 8_200,
            y: 8_200,
            prize_id: PrizeKind::Warp.protocol_id(),
            authoritative_energy: 1_700,
            life_seq: 9,
            prize_state_revision: 1,
            engine_shutdown_until_tick: None,
            engine_shutdown_application_required: false,
        },
        77,
        3,
        2,
    ));
    assert!(network_event_waits_for_movement_correction_terminal(&warp));

    let private_repel = ClientProtocolEvent::SnapshotReceived(SnapshotFrame {
        events: vec![SnapshotEvent::LifecycleState(SnapshotLifecycleState {
            life_seq: 9,
            alive: true,
            respawn_at_tick: None,
            repel_until_tick: Some(1_234),
            repel_delivery: Some(SnapshotRepelDelivery {
                active_steps: 60,
                delivery_generation: 3,
                source_player_id: 2,
                source_life_seq: 5,
                origin_x: 8_100,
                origin_y: 8_100,
                source_frequency: 1,
                expected_velocity_x_bits: 400.0_f32.to_bits(),
                expected_velocity_y_bits: 0.0_f32.to_bits(),
                weapon_data: 1,
                source_ship: 0,
            }),
        })],
        ..SnapshotFrame::default()
    });
    assert!(network_event_waits_for_movement_correction_terminal(
        &private_repel
    ));
    assert!(!network_event_waits_for_movement_correction_terminal(
        &ClientProtocolEvent::SnapshotReceived(SnapshotFrame::default())
    ));
    assert!(network_event_waits_for_movement_correction_terminal(
        &ClientProtocolEvent::SetCoordinates(SetCoordinates { x: 1, y: 2 })
    ));
}

#[test]
fn shutdown_ack_and_terminal_are_pre_gameplay_exact_identity_boundaries() {
    let ack = PrizePickupAck {
        server_session_id: 77,
        arena_stream_id: 3,
        player_id: 2,
        collectible_id: 88,
        x: 8_136,
        y: 8_280,
        prize_id: -14,
        authoritative_energy: 1_234,
        life_seq: 9,
        prize_state_revision: 17,
        engine_shutdown_until_tick: Some(5_000),
        engine_shutdown_application_required: true,
    };
    let application = PrizeEffectApplied {
        server_session_id: 77,
        player_id: 2,
        life_seq: ack.life_seq,
        collectible_id: ack.collectible_id,
        prize_id: ack.prize_id,
        prize_state_revision: ack.prize_state_revision,
        application_sequence: 0,
    };
    let terminal = PrizeEffectAccepted::from(application);
    assert!(network_event_applies_before_gameplay(
        &ClientProtocolEvent::PrizePickupAck(scoped_prize_pickup_ack(ack, 77, 3, 2))
    ));
    assert!(network_event_applies_before_gameplay(
        &ClientProtocolEvent::PrizeEffectAccepted(terminal)
    ));
    assert!(prize_effect_acceptance_matches(application, terminal));
    assert!(prize_effect_rebase_moves_forward(
        application,
        PrizeEffectAccepted {
            outcome: PrizeEffectAccepted::OUTCOME_REBASE,
            requested_application_sequence: 0,
            application_sequence: 1,
            ..terminal
        },
    ));
    assert!(prize_effect_rebase_moves_forward(
        PrizeEffectApplied {
            application_sequence: u16::MAX,
            ..application
        },
        PrizeEffectAccepted {
            outcome: PrizeEffectAccepted::OUTCOME_REBASE,
            requested_application_sequence: u16::MAX,
            application_sequence: 0,
            ..terminal
        },
    ));
    assert!(
        !prize_effect_rebase_moves_forward(
            application,
            PrizeEffectAccepted {
                outcome: PrizeEffectAccepted::OUTCOME_REBASE,
                requested_application_sequence: 0,
                application_sequence: u16::MAX,
                ..terminal
            },
        ),
        "a delayed terminal can never rebase behind the client-declared boundary",
    );
    for stale in [
        PrizeEffectAccepted {
            collectible_id: 89,
            ..terminal
        },
        PrizeEffectAccepted {
            life_seq: 10,
            ..terminal
        },
        PrizeEffectAccepted {
            application_sequence: u16::MAX,
            ..terminal
        },
        PrizeEffectAccepted {
            prize_state_revision: 18,
            ..terminal
        },
    ] {
        assert!(!prize_effect_acceptance_matches(application, stale));
    }
    let first_rebase = PrizeEffectAccepted {
        outcome: PrizeEffectAccepted::OUTCOME_REBASE,
        requested_application_sequence: 0,
        application_sequence: 1,
        ..terminal
    };
    let rebased_application = PrizeEffectApplied {
        application_sequence: 1,
        ..application
    };
    assert!(!prize_effect_acceptance_identity_matches(
        rebased_application,
        first_rebase,
    ));
    assert!(prize_effect_rebase_moves_forward(
        rebased_application,
        PrizeEffectAccepted {
            requested_application_sequence: 1,
            application_sequence: 2,
            ..first_rebase
        },
    ));
}

#[test]
fn every_application_required_movement_prize_ack_is_a_pre_gameplay_boundary() {
    for prize_id in [1, 3, 11, 12, 14, 25, -1, -3, -11, -12, -14] {
        let ack = PrizePickupAck {
            server_session_id: 77,
            arena_stream_id: 3,
            player_id: 2,
            collectible_id: 88,
            x: 8_136,
            y: 8_280,
            prize_id,
            authoritative_energy: 1_234,
            life_seq: 9,
            prize_state_revision: 17,
            engine_shutdown_until_tick: (prize_id.abs() == 14).then_some(5_000),
            engine_shutdown_application_required: true,
        };
        assert!(
            network_event_applies_before_gameplay(&ClientProtocolEvent::PrizePickupAck(
                scoped_prize_pickup_ack(ack, 77, 3, 2),
            )),
            "movement-changing prize {prize_id} must close gameplay before the fixed step",
        );
    }

    let inventory_only = PrizePickupAck {
        server_session_id: 77,
        arena_stream_id: 3,
        player_id: 2,
        collectible_id: 89,
        x: 8_136,
        y: 8_280,
        prize_id: 5,
        authoritative_energy: 1_234,
        life_seq: 9,
        prize_state_revision: 18,
        engine_shutdown_until_tick: None,
        engine_shutdown_application_required: false,
    };
    assert!(!network_event_applies_before_gameplay(
        &ClientProtocolEvent::PrizePickupAck(scoped_prize_pickup_ack(inventory_only, 77, 3, 2,)),
    ));

    assert!(network_event_applies_before_gameplay(
        &ClientProtocolEvent::RepelApplicationAccepted(RepelApplicationAccepted {
            server_session_id: 77,
            life_seq: 9,
            delivery_generation: 4,
            snapshot_id: 12,
            application_sequence: 44,
            accepted_end_sequence: 62,
            active_steps: 19,
        },),
    ));
}

#[test]
fn lost_terminal_keeps_inverse_warp_repel_and_door_boundaries_serialized() {
    let proposal = movement_correction(70);
    let application = MovementCorrectionApplied {
        intent: MovementCorrectionApplied::INTENT_APPLY,
        server_session_id: proposal.server_session_id,
        player_id: proposal.player_id,
        life_seq: proposal.life_seq,
        correction_generation: proposal.correction_generation,
        detected_sequence: proposal.position_sequence,
        application_sequence: 74,
    };
    let warp = ClientProtocolEvent::PrizePickupAck(scoped_prize_pickup_ack(
        PrizePickupAck {
            server_session_id: proposal.server_session_id,
            arena_stream_id: 3,
            player_id: proposal.player_id,
            collectible_id: 8,
            x: 8_200,
            y: 8_200,
            prize_id: PrizeKind::Warp.protocol_id(),
            authoritative_energy: 1_700,
            life_seq: proposal.life_seq,
            prize_state_revision: 1,
            engine_shutdown_until_tick: None,
            engine_shutdown_application_required: false,
        },
        proposal.server_session_id,
        3,
        proposal.player_id,
    ));
    let repel = ClientProtocolEvent::SnapshotReceived(SnapshotFrame {
        events: vec![SnapshotEvent::LifecycleState(SnapshotLifecycleState {
            life_seq: proposal.life_seq,
            alive: true,
            respawn_at_tick: None,
            repel_until_tick: Some(2_000),
            repel_delivery: Some(SnapshotRepelDelivery {
                active_steps: 30,
                delivery_generation: 4,
                source_player_id: 2,
                source_life_seq: 3,
                origin_x: 8_000,
                origin_y: 8_000,
                source_frequency: 1,
                expected_velocity_x_bits: 500.0_f32.to_bits(),
                expected_velocity_y_bits: 0.0_f32.to_bits(),
                weapon_data: 1,
                source_ship: 0,
            }),
        })],
        ..SnapshotFrame::default()
    });
    let actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![warp, repel],
        authority_admission: Vec::new(),
    };

    let (ready, held) = partition_network_actions_behind_movement_correction(actions, true);
    assert!(ready.events.is_empty());
    assert_eq!(held.events.len(), 2);
    assert!(movement_correction_blocks_local_discontinuity(Some(
        application
    )));

    // The first terminal copy is lost: polling again with the same barrier
    // retains both network boundaries and keeps the local door clock from
    // applying a relocation.
    let (ready, held) = partition_network_actions_behind_movement_correction(held, true);
    assert!(ready.events.is_empty());
    assert_eq!(held.events.len(), 2);
    assert!(movement_correction_blocks_local_discontinuity(Some(
        application
    )));

    // Only after the matching terminal clears the transaction may the held
    // Warp/Repel actions and the caught-up door phase mutate local movement.
    let (ready, held) = partition_network_actions_behind_movement_correction(held, false);
    assert_eq!(ready.events.len(), 2);
    assert!(held.events.is_empty());
    assert!(!movement_correction_blocks_local_discontinuity(None));
}

#[test]
fn exact_successor_death_supersedes_correction_and_retires_old_life_boundaries() {
    let session = 77;
    let stream = 3;
    let player = 9;
    let life = 41;
    let epoch = 5;
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(session),
        arena_stream_id: Some(stream),
        player_id: Some(player),
        life_seq: Some(life),
        alive: Some(true),
    };
    let readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(epoch),
        settings: true,
        security: true,
        map: true,
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(scope, readiness, epoch);
    game.ship.pos = vec2(8_192.0, 8_192.0);
    let proposal = MovementCorrection {
        reason: MovementCorrection::REASON_REJECTED_WEAPON_RECOIL,
        repel_window: None,
        server_session_id: session,
        player_id: player,
        life_seq: life,
        correction_generation: 1,
        position_sequence: 200,
        server_tick: 500,
        position_x: game.ship.pos.x,
        position_y: game.ship.pos.y,
        velocity_x: 0.0,
        velocity_y: 0.0,
        orientation: 0.0,
    };
    assert_eq!(
        game.client_authority
            .observe_movement_correction(proposal, 202, false),
        ClientAuthorityDisposition::Applied
    );

    let stale_coordinates = SetCoordinates { x: 100, y: 101 };
    let death = PlayerDeath {
        green_id: -1,
        green_collectible_id: 0,
        killer_id: 768,
        killed_id: player,
        bounty: 50,
        flag_transfer: 0,
        life_seq: life + 1,
        respawn_at_tick: 900,
    };
    let actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::SetCoordinates(stale_coordinates),
            ClientProtocolEvent::PlayerDeath(death),
        ],
        authority_admission: Vec::new(),
    };
    let (pre, deferred) = partition_browser_network_actions_at_segment(
        actions,
        Some(session),
        Some(stream),
        Some(player),
        Some(life),
        Some(true),
        true,
        Some(epoch),
        true,
        true,
        true,
        game.client_authority.view().segment,
        true,
        false,
        false,
    );
    assert!(pre.events.is_empty());
    let (ready, held) = partition_network_actions_behind_movement_correction_at_segment(
        deferred,
        true,
        game.client_authority.view().segment,
    );
    assert!(matches!(
        ready.events.as_slice(),
        [ClientProtocolEvent::PlayerDeath(observed)] if *observed == death
    ));
    assert!(matches!(
        held.events.as_slice(),
        [ClientProtocolEvent::SetCoordinates(observed)] if *observed == stale_coordinates
    ));

    game.last_observed_browser_server_tick = Some(500);
    game.apply_network_actions(&ready);
    let authority = game.client_authority.view();
    assert_eq!(authority.scope.life_seq, Some(life + 1));
    assert_eq!(authority.scope.alive, Some(false));
    assert_eq!(
        authority.transition_kind,
        Some(ClientAuthorityTransitionKind::Respawn)
    );
    assert!(authority.correction_application.is_none());

    // Releasing the old correction bucket after death cannot rebind its
    // same-life coordinate mutation to the successor life.
    let (stale, still_held) = partition_network_actions_behind_movement_correction_at_segment(
        held,
        false,
        game.client_authority.view().segment,
    );
    assert!(still_held.events.is_empty());
    let death_position = game.ship.pos;
    game.apply_network_actions(&stale);
    assert_eq!(game.ship.pos, death_position);
}

#[test]
fn ship_life_reset_position_emits_on_exact_ack_step_before_catch_up_tail() {
    assert!(network_position_emission_due(
        false, false, true, false, false, false
    ));
    assert!(network_position_emission_due(
        true, false, true, false, false, false
    ));
    assert!(!network_position_emission_due(
        true, false, false, false, false, false
    ));
    assert!(network_position_emission_due(
        true, true, false, false, false, false
    ));
    assert!(!network_position_emission_due(
        false, true, false, false, false, false
    ));
    assert!(!network_position_emission_due(
        true, true, false, true, false, false
    ));
    assert!(network_position_emission_due(
        false, false, false, false, true, false
    ));
    assert!(network_position_emission_due(
        false, true, false, true, true, false
    ));
}

#[test]
fn weapon_recoil_position_emits_on_its_exact_final_step_without_waiting_for_timer() {
    assert!(network_position_emission_due(
        false, true, false, false, false, true
    ));
    assert!(!network_position_emission_due(
        false, false, false, false, false, true
    ));
}

#[test]
fn embedded_position_replaces_only_its_redundant_standalone_input_copy() {
    assert!(standalone_input_emission_due(false));
    assert!(!standalone_input_emission_due(true));

    // A three-step catch-up batch retains the two intermediate standalone
    // histories, while its final position atomically carries the tail. The
    // normal one-step path emits one position and no duplicate `0x62` packet.
    let catch_up_positions = [false, false, true];
    assert_eq!(
        catch_up_positions
            .into_iter()
            .filter(|position_due| standalone_input_emission_due(*position_due))
            .count(),
        2
    );
    assert!(!standalone_input_emission_due(true));
}

fn simulate_ship_life_position_batch(
    next_sequence: &mut u16,
    logical_steps_after_ack: usize,
    pending_reset: &mut bool,
    suppress_tail: &mut bool,
) -> Vec<u16> {
    let mut emitted = Vec::new();
    for logical_step in 0..logical_steps_after_ack {
        let sequence = *next_sequence;
        *next_sequence = next_sequence.wrapping_add(1);
        let reset = *pending_reset;
        if network_position_emission_due(
            true,
            logical_step + 1 == logical_steps_after_ack,
            reset,
            *suppress_tail,
            false,
            false,
        ) {
            emitted.push(sequence);
            if reset {
                *pending_reset = false;
                *suppress_tail = true;
            }
        }
    }
    emitted
}

#[test]
fn ship_life_ack_in_two_and_eight_step_batches_emits_boundary_exactly_once() {
    for steps in [2, 8] {
        let base = 4_000;
        let mut next_sequence = base;
        let mut pending_reset = true;
        let mut suppress_tail = false;

        let ack_batch = simulate_ship_life_position_batch(
            &mut next_sequence,
            steps,
            &mut pending_reset,
            &mut suppress_tail,
        );
        assert_eq!(ack_batch, [base]);
        assert!(!pending_reset);
        assert!(suppress_tail);

        // The render-batch boundary retires only the tail suppression. A
        // later batch keeps advancing the new-life stream and may publish its
        // ordinary final position without producing a reset burst.
        suppress_tail = false;
        let later_batch = simulate_ship_life_position_batch(
            &mut next_sequence,
            steps,
            &mut pending_reset,
            &mut suppress_tail,
        );
        assert_eq!(later_batch, [base.wrapping_add((steps * 2 - 1) as u16)]);
        assert_eq!(
            ack_batch
                .iter()
                .chain(&later_batch)
                .filter(|&&sequence| sequence == base)
                .count(),
            1
        );
    }
}

#[test]
fn ship_life_ack_survives_zero_step_frame_and_wraps_without_reorder_wedge() {
    let base = u16::MAX;
    let mut next_sequence = base;
    let mut pending_reset = true;
    let mut suppress_tail = false;

    let zero_step = simulate_ship_life_position_batch(
        &mut next_sequence,
        0,
        &mut pending_reset,
        &mut suppress_tail,
    );
    assert!(zero_step.is_empty());
    assert_eq!(next_sequence, base);
    assert!(pending_reset);

    let catch_up = simulate_ship_life_position_batch(
        &mut next_sequence,
        8,
        &mut pending_reset,
        &mut suppress_tail,
    );
    assert_eq!(catch_up, [u16::MAX]);
    assert_eq!(next_sequence, 7);

    suppress_tail = false;
    let later_tail = simulate_ship_life_position_batch(
        &mut next_sequence,
        2,
        &mut pending_reset,
        &mut suppress_tail,
    );
    assert_eq!(later_tail, [8]);
}

#[test]
fn unsent_lifecycle_intent_is_preserved_and_does_not_start_timeout() {
    let mut pending = None;
    assert!(preserve_lifecycle_request_intent(
        &mut pending,
        true,
        PendingLifecycleRequest::Frequency(19),
    ));
    assert!(!preserve_lifecycle_request_intent(
        &mut pending,
        true,
        PendingLifecycleRequest::Frequency(19),
    ));
    assert!(!preserve_lifecycle_request_intent(
        &mut pending,
        true,
        PendingLifecycleRequest::Frequency(20),
    ));
    assert!(!preserve_lifecycle_request_intent(
        &mut pending,
        true,
        PendingLifecycleRequest::Ship(4),
    ));
    assert_eq!(pending, Some(PendingLifecycleRequest::Frequency(19)));
}

#[test]
fn frequency_action_defers_exact_baseline_until_after_current_movement_step() {
    let mut pending = None;
    let sequence = 16_860_u16;
    let position_before = 10_577.971_679_687_5_f32;
    let velocity = 550.0_f32;

    assert!(queue_frequency_change_for_network_tail(
        &mut pending,
        true,
        2,
    ));
    assert_eq!(pending, Some(PendingLifecycleRequest::Frequency(2)));

    // The reference game's position integration for N occurs after the semantic action
    // but before the network tail consumes the queued intent.
    let position_after = position_before + velocity * crate::fixed_step::GAMEPLAY_STEP_SECONDS;
    let baseline_sequence = sequence;
    let baseline_state = crate::continuum_position::ClientPositionCanonicalState::from_f32(
        position_after,
        8_200.0,
        velocity,
        0.0,
        0.268_333_38,
    );
    assert_eq!(baseline_sequence, sequence);
    assert_eq!(
        baseline_state.values().0.to_bits(),
        position_after.to_bits()
    );
    assert_ne!(
        baseline_state.values().0.to_bits(),
        position_before.to_bits(),
        "the reserved N baseline must never reuse the accepted N-1 tail",
    );
}

#[test]
fn browser_network_stats_track_realtime_backpressure_drops() {
    let mut stats = BrowserNetworkStats::default();

    stats.update_rates(1.0, 12, 20, 30, 4);
    assert_eq!(stats.dropped_realtime_packets, 4);
    assert_eq!(stats.dropped_realtime_packets_per_second, 4.0);

    stats.update_rates(1.0, 24, 36, 50, 9);
    assert_eq!(stats.dropped_realtime_packets, 9);
    assert_eq!(stats.dropped_realtime_packets_per_second, 5.0);
}

#[test]
fn reconciled_self_energy_lets_prediction_lead_recharge() {
    // [REVIEW M10] Within the recharge-lag tolerance, the stale server
    // snapshot must not drag the predicted energy (fire/warp gating) down.
    assert_eq!(reconciled_self_energy(1_700.0, 1_690.0), 1_700.0);
    assert_eq!(reconciled_self_energy(1_000.0, 990.0), 1_000.0);
}

#[test]
fn reconciled_self_energy_adopts_real_downward_divergence() {
    // Beyond tolerance the server is authoritative (mispredicted drain or
    // missed damage): adopt it exactly.
    assert_eq!(reconciled_self_energy(1_700.0, 1_300.0), 1_300.0);
    assert_eq!(reconciled_self_energy(500.0, 400.0), 400.0);
}

#[test]
fn reconciled_self_energy_converges_upward_smoothly() {
    // The server reporting MORE than predicted (e.g. recharge upgrade
    // applied server-side) converges without snapping.
    let next = reconciled_self_energy(1_000.0, 1_200.0);
    assert!(next > 1_000.0 && next < 1_200.0, "got {next}");
    // Repeated application converges toward the server value.
    let mut energy = 1_000.0;
    for _ in 0..32 {
        energy = reconciled_self_energy(energy, 1_200.0);
    }
    assert!((energy - 1_200.0).abs() < 2.0, "got {energy}");
}

#[test]
fn dead_private_runtime_cannot_poison_an_armed_respawn_baseline() {
    assert!(!self_runtime_snapshot_is_admissible(Some(false), true));
    assert!(self_runtime_snapshot_is_admissible(Some(false), false));
    assert!(self_runtime_snapshot_is_admissible(Some(true), true));
    assert!(self_runtime_snapshot_is_admissible(None, true));
}

#[test]
fn private_prize_reconciliation_opens_initial_join_but_not_replacement_cutovers() {
    assert!(!prize_state_transition_blocks_reconciliation(
        Some(ClientAuthorityTransitionKind::InitialJoin),
        Some(true),
    ));
    assert!(!prize_state_transition_blocks_reconciliation(
        None,
        Some(true)
    ));
    for kind in [
        ClientAuthorityTransitionKind::Ship,
        ClientAuthorityTransitionKind::Frequency,
        ClientAuthorityTransitionKind::Correction,
    ] {
        assert!(prize_state_transition_blocks_reconciliation(
            Some(kind),
            Some(true),
        ));
    }
    assert!(prize_state_transition_blocks_reconciliation(
        Some(ClientAuthorityTransitionKind::Respawn),
        Some(false),
    ));
    assert!(!prize_state_transition_blocks_reconciliation(
        Some(ClientAuthorityTransitionKind::Respawn),
        Some(true),
    ));
}

#[test]
fn authoritative_rocket_deadline_never_refreshes_from_delivery_delay() {
    let state = SnapshotRocketState {
        life_seq: 7,
        observed_input_sequence: Some(65_534),
        input_sequence_watermark: Some(65_534),
        activation_sequence: Some(65_534),
        until_tick: Some(10_005),
        active: true,
        rockets: 2,
    };
    assert_eq!(authoritative_rocket_timer_seconds(state, 10_004), 0.01);
    assert_eq!(authoritative_rocket_timer_seconds(state, 10_005), 0.0);
    assert_eq!(authoritative_rocket_timer_seconds(state, 10_900), 0.0);
    assert_eq!(
        current_rocket_reconciliation_tick(10_004, Some(10_900)),
        10_900,
        "a delayed frame uses the newer synchronized clock"
    );
    assert_eq!(
        current_rocket_reconciliation_tick(10_004, Some(10_003)),
        10_004,
        "a lagging local estimate cannot predate the snapshot itself"
    );
}

#[test]
fn matching_delayed_rocket_reconciliation_never_increases_prediction() {
    let delayed = SnapshotRocketState {
        life_seq: 7,
        observed_input_sequence: Some(100),
        input_sequence_watermark: Some(124),
        activation_sequence: Some(100),
        // Deliberately overstates receipt-relative authority. Sequence-relative
        // reconciliation and the existing prediction must both cap it.
        until_tick: Some(2_000),
        active: true,
        rockets: 2,
    };
    let predicted = 9.40;
    let reconciled = reconciled_self_rocket_timer_seconds(predicted, 10.0, delayed, 1_000, 125);
    assert_eq!(reconciled, predicted);

    let authoritative_shorter = reconciled_self_rocket_timer_seconds(
        predicted,
        10.0,
        SnapshotRocketState {
            until_tick: Some(1_900),
            ..delayed
        },
        1_000,
        125,
    );
    assert_eq!(authoritative_shorter, 9.0);

    let expired_or_reordered = reconciled_self_rocket_timer_seconds(0.0, 10.0, delayed, 1_000, 125);
    assert_eq!(expired_or_reordered, 0.0);
}

#[test]
fn sequence_relative_rocket_reconciliation_wraps_and_inactive_state_wins() {
    let activation = u16::MAX - 5;
    let last_consumed = activation.wrapping_add(24);
    let current_tick = crate::continuum::CONTINUUM_TICK_MASK - 10;
    let state = SnapshotRocketState {
        life_seq: u32::MAX,
        observed_input_sequence: Some(activation),
        input_sequence_watermark: Some(last_consumed),
        activation_sequence: Some(activation),
        until_tick: Some(crate::continuum::continuum_tick_add(current_tick, 2_000)),
        active: true,
        rockets: 0,
    };
    let predicted = 9.9;
    let reconciled = reconciled_self_rocket_timer_seconds(
        predicted,
        10.0,
        state,
        current_tick,
        last_consumed.wrapping_add(1),
    );
    let sequence_remaining = 10.0 - 25.0 * crate::fixed_step::GAMEPLAY_STEP_SECONDS;
    assert!((reconciled - sequence_remaining).abs() < 0.000_1);
    assert!(reconciled <= predicted);
    assert_eq!(
        reconciled_self_rocket_timer_seconds(
            reconciled,
            10.0,
            SnapshotRocketState {
                active: false,
                ..state
            },
            0,
            last_consumed.wrapping_add(1),
        ),
        0.0,
    );
}

#[test]
fn authoritative_rocket_deadline_and_life_sequence_are_wrap_safe() {
    let state = SnapshotRocketState {
        life_seq: u32::MAX,
        observed_input_sequence: Some(u16::MAX),
        input_sequence_watermark: Some(u16::MAX),
        activation_sequence: Some(u16::MAX),
        until_tick: Some(2),
        active: true,
        rockets: 0,
    };
    assert_eq!(
        authoritative_rocket_timer_seconds(state, crate::continuum::CONTINUUM_TICK_MASK - 1),
        0.04
    );
    assert!(life_sequence_is_current_or_forward(Some(u32::MAX), 0));
    assert!(!life_sequence_is_current_or_forward(Some(10), 9));
    assert_eq!(
        authoritative_rocket_timer_seconds(
            SnapshotRocketState {
                active: false,
                ..state
            },
            0
        ),
        0.0,
        "an authoritative inactive snapshot wins even before its old deadline"
    );
}

#[test]
fn rocket_pending_activation_waits_for_observation_then_accepts_or_rejects() {
    let pre_activation = SnapshotRocketState {
        life_seq: 1,
        observed_input_sequence: Some(99),
        input_sequence_watermark: Some(99),
        activation_sequence: None,
        until_tick: None,
        active: false,
        rockets: 3,
    };
    assert!(!rocket_state_resolves_pending_activation(
        Some(100),
        pre_activation
    ));

    assert!(!rocket_state_resolves_pending_activation(
        Some(100),
        SnapshotRocketState {
            observed_input_sequence: None,
            input_sequence_watermark: Some(163),
            ..pre_activation
        }
    ));

    let accepted = SnapshotRocketState {
        observed_input_sequence: Some(100),
        activation_sequence: Some(100),
        until_tick: Some(2_000),
        active: true,
        rockets: 2,
        ..pre_activation
    };
    assert!(rocket_state_resolves_pending_activation(
        Some(100),
        accepted
    ));

    assert!(rocket_state_resolves_pending_activation(
        Some(100),
        SnapshotRocketState {
            observed_input_sequence: Some(100),
            input_sequence_watermark: Some(101),
            ..pre_activation
        }
    ));

    let rejected_or_wholly_lost = SnapshotRocketState {
        observed_input_sequence: None,
        input_sequence_watermark: Some(164),
        ..pre_activation
    };
    assert!(rocket_state_resolves_pending_activation(
        Some(100),
        rejected_or_wholly_lost
    ));

    let wrapped_rejection = SnapshotRocketState {
        observed_input_sequence: None,
        input_sequence_watermark: Some(63),
        ..pre_activation
    };
    assert!(rocket_state_resolves_pending_activation(
        Some(u16::MAX),
        wrapped_rejection
    ));
    assert!(!rocket_state_resolves_pending_activation(
        Some(u16::MAX),
        SnapshotRocketState {
            observed_input_sequence: None,
            input_sequence_watermark: Some(62),
            ..pre_activation
        }
    ));
    assert!(!rocket_state_resolves_pending_activation(
        Some(1),
        SnapshotRocketState {
            observed_input_sequence: Some(u16::MAX),
            input_sequence_watermark: Some(u16::MAX),
            ..pre_activation
        }
    ));
}

#[test]
fn rocket_reset_requires_a_strictly_newer_life_before_state_can_mutate_again() {
    let previous = Some(u32::MAX);
    let mut rockets = 3_u8;
    let mut timer = 0.0_f32;
    let mut require_forward = true;

    let delayed_old_life = SnapshotRocketState {
        life_seq: u32::MAX,
        input_sequence_watermark: Some(100),
        activation_sequence: Some(100),
        until_tick: Some(2_000),
        active: true,
        rockets: 0,
        ..SnapshotRocketState::default()
    };
    if rocket_life_sequence_is_admissible(previous, delayed_old_life.life_seq, require_forward) {
        rockets = delayed_old_life.rockets;
        timer = authoritative_rocket_timer_seconds(delayed_old_life, 1_000);
    }
    assert_eq!((rockets, timer), (3, 0.0));
    assert!(require_forward);

    let forward_after_wrap = SnapshotRocketState {
        life_seq: 1,
        rockets: 2,
        ..SnapshotRocketState::default()
    };
    assert!(rocket_life_sequence_is_admissible(
        previous,
        forward_after_wrap.life_seq,
        require_forward
    ));
    rockets = forward_after_wrap.rockets;
    timer = authoritative_rocket_timer_seconds(forward_after_wrap, 1_000);
    require_forward = false;
    assert_eq!((rockets, timer), (2, 0.0));

    assert!(rocket_life_sequence_is_admissible(
        Some(1),
        1,
        require_forward
    ));
    assert!(!rocket_life_sequence_is_admissible(
        Some(1),
        u32::MAX,
        require_forward
    ));
}

#[test]
fn accepted_lifecycle_baseline_clears_predicted_rocket_and_establishes_wrapped_life() {
    let mut last_life = Some(u32::MAX);
    let mut require_forward = true;
    let mut last_activation = Some(u16::MAX);
    let mut last_until = Some(crate::continuum::CONTINUUM_TICK_MASK);
    let mut pending_activation = Some(u16::MAX);
    let mut timer = 8.5;
    let mut cooldown = 0.25;
    let mut last_input = PlayerInput {
        thrust: true,
        rocket_activation_pulse: true,
        ..PlayerInput::default()
    };

    commit_accepted_lifecycle_baseline_rocket_lifecycle(
        1,
        &mut last_life,
        &mut require_forward,
        &mut last_activation,
        &mut last_until,
        &mut pending_activation,
        &mut timer,
        &mut cooldown,
        &mut last_input,
    );

    assert_eq!(last_life, Some(1));
    assert!(!require_forward);
    assert_eq!(
        (last_activation, last_until, pending_activation),
        (None, None, None)
    );
    assert_eq!((timer, cooldown), (0.0, 0.0));
    assert!(
        last_input.thrust,
        "cutover preserves non-Rocket input state"
    );
    assert!(!last_input.rocket_activation_pulse);

    let delayed_old_life = SnapshotRocketState {
        life_seq: u32::MAX,
        activation_sequence: Some(u16::MAX),
        until_tick: Some(crate::continuum::CONTINUUM_TICK_MASK),
        active: true,
        rockets: 0,
        ..SnapshotRocketState::default()
    };
    assert!(!rocket_life_sequence_is_admissible(
        last_life,
        delayed_old_life.life_seq,
        require_forward,
    ));
    assert!(rocket_life_sequence_is_admissible(
        last_life,
        SnapshotRocketState {
            life_seq: 1,
            active: false,
            rockets: 2,
            ..SnapshotRocketState::default()
        }
        .life_seq,
        require_forward,
    ));
}

#[test]
fn accepted_respawn_baseline_reopens_same_life_rocket_receipts() {
    let life_seq = 41;
    let mut last_life = Some(life_seq);
    let mut require_forward = true;
    let mut last_activation = Some(700);
    let mut last_until = Some(9_000);
    let mut pending_activation = Some(701);
    let mut timer = 8.5;
    let mut cooldown = 0.25;
    let mut last_input = PlayerInput {
        thrust: true,
        rocket_activation_pulse: true,
        ..PlayerInput::default()
    };

    // A matched 0x75 is application authority for this exact life. The
    // client must not keep waiting for life 42 after rebasing its simulation.
    commit_accepted_lifecycle_baseline_rocket_lifecycle(
        life_seq,
        &mut last_life,
        &mut require_forward,
        &mut last_activation,
        &mut last_until,
        &mut pending_activation,
        &mut timer,
        &mut cooldown,
        &mut last_input,
    );

    assert_eq!(last_life, Some(life_seq));
    assert!(!require_forward);
    assert_eq!(
        (last_activation, last_until, pending_activation),
        (None, None, None)
    );
    assert_eq!((timer, cooldown), (0.0, 0.0));
    assert!(last_input.thrust);
    assert!(!last_input.rocket_activation_pulse);
    assert!(rocket_state_is_admissible_across_lifecycle_cutover(
        false,
        last_life,
        life_seq,
        require_forward,
    ));
    assert!(!rocket_state_is_admissible_across_lifecycle_cutover(
        false,
        last_life,
        life_seq.wrapping_sub(1),
        require_forward,
    ));
}

#[test]
fn accepted_respawn_baseline_atomically_retires_same_frame_rearmed_position_intent() {
    let live_observed_scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(91),
        life_seq: Some(101),
        alive: Some(true),
    };
    let predecessor_scope = AuthorityScopeIdentity {
        life_seq: Some(100),
        alive: Some(true),
        ..live_observed_scope
    };
    let held_dead_scope = AuthorityScopeIdentity {
        alive: Some(false),
        ..live_observed_scope
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        predecessor_scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(111),
            settings: true,
            security: true,
            map: true,
        },
        111,
    );
    game.network_prize_baseline_pending = false;

    assert_eq!(
        game.client_authority.observe_private_lifecycle(
            ClientAuthoritySource::Event,
            held_dead_scope,
            Some(500),
            0.0,
        ),
        ClientAuthorityDisposition::Replacement
    );
    assert_eq!(
        game.client_authority.advance_respawn_eligibility(0.0),
        ClientAuthorityDisposition::Applied
    );
    assert_eq!(
        game.client_authority
            .record_lifecycle_baseline_submitted(342, 31_707, None),
        ClientAuthorityDisposition::Applied
    );
    game.pending_weapon = 65;

    // Model the browser getting one deterministic door phase ahead while the
    // authoritative server tick stalls before committing the neutral respawn
    // baseline. The accepted receipt must reset both the visible seed and the
    // hidden RNG stream before successor-life rolling input can be emitted.
    let challenge = game
        .network_security_challenge
        .expect("complete test readiness installs SECURITY");
    let mut expected_map = ArenaMap::fallback_skirmish();
    let expected_timer = synchronize_map_doors_from_security(
        &mut expected_map,
        challenge,
        game.tuning.door_mode,
        game.tuning.door_delay_seconds,
        1_200,
    );
    let map_index = game.map_index;
    game.maps[map_index].door_seed = expected_map.door_seed.wrapping_add(1);
    game.maps[map_index].door_rng_seed = expected_map.door_rng_seed.wrapping_add(1);
    game.door_timer = 0.01;
    game.door_clock_server_tick = Some(1_440);
    game.door_clock_observed_server_tick = Some(1_440);
    game.last_observed_browser_server_tick = Some(1_199);

    let receipt = LifecycleBaselineAccepted {
        client_reliable_delivery_id: 342,
        life_seq: 101,
        position_sequence: 31_707,
        accepted_server_tick: 1_200,
        accepted_position: Some((8104, 8312)),
    };
    let admission = AuthorityEventAdmission {
        segment: 0,
        action: AuthorityActionAdmission::Bound(live_observed_scope),
        readiness_after: AuthorityReadiness {
            joined: true,
            arena_epoch: Some(111),
            settings: true,
            security: true,
            map: true,
        },
        ..AuthorityEventAdmission::default()
    };
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::LifecycleBaselineAccepted(receipt)],
        authority_admission: vec![admission],
    });

    let authority = game.client_authority.view();
    assert_eq!(
        game.ship.pos,
        vec2(8104.0, 8312.0),
        "only the matched receipt installs the server-selected spawn"
    );
    assert_eq!(game.ship.vel, Vec2::ZERO);
    assert_eq!(game.ship.orientation, 0.0);
    assert_eq!(authority.lifecycle_receipts_seen, 1);
    assert_eq!(authority.lifecycle_receipts_matched, 1);
    assert_eq!(authority.phase, ClientAuthorityPhase::Live);
    assert_eq!(authority.transition_kind, None);
    assert_eq!(game.pending_weapon, 0);
    assert!(game.suppress_gameplay_step_after_network_rebase);
    assert_eq!(game.maps[map_index].door_seed, expected_map.door_seed);
    assert_eq!(
        game.maps[map_index].door_rng_seed,
        expected_map.door_rng_seed
    );
    assert!((game.door_timer - expected_timer).abs() < 0.0001);
    assert_eq!(game.door_clock_server_tick, Some(1_200));
    assert_eq!(game.door_clock_observed_server_tick, Some(1_200));
    assert!(game.door_clock_fresh);

    let actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::LifecycleBaselineAccepted(receipt)],
        authority_admission: Vec::new(),
    };
    assert_eq!(
        admitted_authority_actions(&actions, &[admission], 0, held_dead_scope),
        vec![true],
        "the exact same-life receipt crosses only the transitional alive bit"
    );
    assert_eq!(
        admitted_authority_actions(
            &actions,
            &[AuthorityEventAdmission {
                segment: u64::MAX,
                ..admission
            }],
            0,
            held_dead_scope,
        ),
        vec![false],
        "an old persistent authority generation remains inert"
    );
    assert_eq!(
        admitted_authority_actions(
            &actions,
            &[AuthorityEventAdmission {
                action: AuthorityActionAdmission::Bound(AuthorityScopeIdentity {
                    life_seq: Some(100),
                    ..live_observed_scope
                }),
                ..admission
            }],
            0,
            held_dead_scope,
        ),
        vec![false],
        "an old numeric life cannot release the current boundary"
    );
    game.ship.pos = vec2(8110.0, 8318.0);
    game.ship.vel = vec2(10.0, 5.0);
    for stale in [
        receipt,
        LifecycleBaselineAccepted {
            life_seq: 100,
            ..receipt
        },
        LifecycleBaselineAccepted {
            client_reliable_delivery_id: 341,
            ..receipt
        },
        LifecycleBaselineAccepted {
            position_sequence: 31_706,
            ..receipt
        },
    ] {
        game.apply_network_actions(&ClientProtocolActions {
            outbound_packets: vec![],
            events: vec![ClientProtocolEvent::LifecycleBaselineAccepted(stale)],
            authority_admission: vec![admission],
        });
        assert_eq!(
            game.ship.pos,
            vec2(8110.0, 8318.0),
            "a duplicate or unmatched receipt cannot repeat the spawn relocation"
        );
        assert_eq!(game.ship.vel, vec2(10.0, 5.0));
    }
}

#[test]
fn lifecycle_ack_orders_private_rocket_life_across_independent_channels() {
    let old_life = u32::MAX;
    let new_life = 1;

    assert!(
        !rocket_state_is_admissible_across_lifecycle_cutover(true, Some(old_life), new_life, false),
        "a new-life realtime snapshot cannot mutate Rocket state before the tracked ACK"
    );
    assert!(
        rocket_state_is_admissible_across_lifecycle_cutover(false, Some(old_life), new_life, true,),
        "ACK-before-snapshot admits the exact wrapped next life"
    );
    assert!(
        !rocket_state_is_admissible_across_lifecycle_cutover(false, Some(old_life), old_life, true,),
        "the delayed old life remains rejected after the ACK"
    );
    assert!(
        !rocket_state_is_admissible_across_lifecycle_cutover(
            false,
            Some(old_life),
            new_life.wrapping_add(1),
            true,
        ),
        "an arbitrary forward life cannot unlatch the exact tracked boundary"
    );
    assert!(
        rocket_state_is_admissible_across_lifecycle_cutover(false, None, new_life, false),
        "a session with no prior private life accepts its first post-ACK state"
    );
}

#[test]
fn lifecycle_ack_buffers_private_prize_state_across_ship_and_frequency_cutovers() {
    let old_life = u32::MAX;
    let new_life = 1;

    assert!(!prize_state_is_admissible_across_lifecycle_cutover(
        true,
        Some(new_life),
        new_life,
    ));
    assert!(!prize_state_is_admissible_across_lifecycle_cutover(
        false,
        Some(old_life),
        new_life,
    ));
    assert!(prize_state_is_admissible_across_lifecycle_cutover(
        false,
        Some(new_life),
        new_life,
    ));
    assert!(!prize_state_is_admissible_across_lifecycle_cutover(
        false,
        Some(new_life),
        old_life,
    ));
}

fn round9_test_settings() -> ContinuumArenaSettings {
    ContinuumArenaSettings::parse(&vec![0; CONTINUUM_ARENA_SETTINGS_SIZE])
        .expect("zeroed settings retain the canonical wire layout")
}

fn authority_test_position(
    player_id: u16,
    x: u16,
    y: u16,
    energy: u16,
    weapon: u16,
) -> PlayerPositionUpdate {
    PlayerPositionUpdate {
        player_id,
        direction: 0,
        timestamp: 0,
        x,
        y,
        velocity_x: 0,
        velocity_y: 0,
        togglables: 0,
        ping: 0,
        bounty: 0,
        weapon,
        checksum: None,
        energy: Some(energy),
        s2c_latency: None,
        flag_timer: None,
        items: None,
    }
}

fn round9_replacement_actions(private_first: bool) -> ClientProtocolActions {
    let session = 1;
    let stream = 1;
    let player = 9;
    let life = 1;
    let epoch = 1;
    let ack = PrizePickupAck {
        server_session_id: session,
        arena_stream_id: stream,
        player_id: player,
        collectible_id: 700,
        x: 8_192,
        y: 8_192,
        prize_id: PrizeKind::Rocket.protocol_id(),
        authoritative_energy: 900,
        life_seq: life,
        prize_state_revision: 6,
        engine_shutdown_until_tick: None,
        engine_shutdown_application_required: false,
    };
    let private = vec![
        ClientProtocolEvent::PrizePickupAck(ScopedPrizePickupAck {
            ack,
            server_session_id: Some(session),
            arena_stream_id: Some(stream),
            player_id: Some(player),
        }),
        private_prize_snapshot(
            1,
            stream,
            SnapshotPrizeState {
                life_seq: life,
                revision: 7,
                bounty: 55,
                max_energy: 1_800,
                rotation_bits: 1.25_f32.to_bits(),
                thrust_bits: 425.0_f32.to_bits(),
                top_speed_bits: 515.0_f32.to_bits(),
                recharge_bits: 67.0_f32.to_bits(),
                gun_level: 2,
                bomb_level: 1,
                repels: 4,
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: life,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        ),
        ClientProtocolEvent::PlayerPosition(PlayerPositionUpdate {
            player_id: player,
            x: 8_192,
            y: 8_192,
            bounty: 55,
            energy: Some(900),
            ..PlayerPositionUpdate {
                player_id: player,
                direction: 0,
                timestamp: 0,
                x: 0,
                y: 0,
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
        }),
    ];
    let replacement = vec![
        ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
            server_session_id: session,
            arena_stream_id: stream,
            tick_rate_hz: 100,
            snapshot_rate_hz: 60,
            ..ServerClockBootstrap::default()
        }),
        ClientProtocolEvent::PlayerIdAssigned(player),
        ClientProtocolEvent::JoinedArena(epoch),
        ClientProtocolEvent::ArenaSettingsUpdated(round9_test_settings()),
        ClientProtocolEvent::SecurityChallengeReceived {
            challenge: SecurityChallenge::default(),
            server_session_id: Some(session),
            arena_epoch: epoch,
        },
        ClientProtocolEvent::ZoneMapPublished,
    ];
    let events = if private_first {
        private.into_iter().chain(replacement).collect()
    } else {
        replacement.into_iter().chain(private).collect()
    };
    ClientProtocolActions {
        outbound_packets: Vec::new(),
        events,
        authority_admission: Vec::new(),
    }
}

#[test]
fn production_game_apply_installs_replacement_private_baseline_in_both_channel_orders() {
    let old_scope = AuthorityScopeIdentity {
        server_session_id: Some(u64::MAX),
        arena_stream_id: Some(u64::MAX),
        // The replacement deliberately reuses numeric player id 9; the hard
        // session/stream generation, not the reusable id, owns admission.
        player_id: Some(9),
        life_seq: Some(u32::MAX),
        alive: Some(true),
    };
    let old_readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(u32::MAX),
        settings: true,
        security: true,
        map: true,
    };

    for private_first in [true, false] {
        for security_first in [true, false] {
            let mut game = Game::new_network_authority_test();
            game.install_network_authority_test_scope(old_scope, old_readiness, u32::MAX);
            game.player_bounty = 999;
            game.ship.energy = 1_500.0;
            game.ship.max_energy = 1_700.0;
            game.ship.rotation_per_second = 9.0;
            game.ship.thrust_acceleration = 9_000.0;
            game.ship.recharge_per_second = 9_000.0;
            game.ship.gun_level = 3;
            game.ship.rockets = 9;

            let mut actions = round9_replacement_actions(private_first);
            if security_first {
                let security_index = actions
                    .events
                    .iter()
                    .position(|event| {
                        matches!(event, ClientProtocolEvent::SecurityChallengeReceived { .. })
                    })
                    .expect("replacement fixture carries SECURITY");
                let security = actions.events.remove(security_index);
                let joined_index = actions
                    .events
                    .iter()
                    .position(|event| matches!(event, ClientProtocolEvent::JoinedArena(_)))
                    .expect("replacement fixture carries its hard arena boundary");
                // Cross-channel order is independent inside the replacement
                // segment, but SECURITY decoded before the hard arena
                // boundary belongs to the old segment and must not be rebound.
                actions.events.insert(joined_index + 1, security);
            }
            let (pre, deferred) = partition_browser_network_actions(
                actions,
                old_scope.server_session_id,
                old_scope.arena_stream_id,
                old_scope.player_id,
                old_scope.life_seq,
                old_scope.alive,
                true,
                Some(u32::MAX),
                true,
                true,
                true,
                false,
                false,
            );
            assert_eq!(
                pre.events,
                vec![ClientProtocolEvent::PreGameplayAuthorityReset],
                "only the private authority reset may cross the fixed-step boundary"
            );
            game.apply_network_actions(&pre);
            assert!(game.network_prize_baseline_pending);
            assert!(!game.local_gameplay_ready());
            assert_eq!(game.ship.energy, 0.0);
            // Movement tuning returns to the arena base rather than zero. Zeroing it
            // froze the ship for the whole baseline wait, because `top_speed` is
            // `effective_top_speed()` verbatim and clamps the physics.
            let base_tuning = game.ship_tuning();
            assert_eq!(
                game.ship.rotation_per_second,
                base_tuning.rotation_per_second
            );
            assert_eq!(
                game.ship.thrust_acceleration,
                base_tuning.thrust_acceleration
            );
            assert_eq!(game.ship.recharge_per_second, base_tuning.energy_recharge);
            assert_eq!(game.ship.gun_level, 0);
            assert_eq!(game.ship.rockets, 0);

            let duplicate_private: ClientProtocolActions = ClientProtocolActions {
                outbound_packets: Vec::new(),
                events: deferred
                    .events
                    .iter()
                    .cloned()
                    .zip(deferred.authority_admission.iter().copied())
                    .filter_map(|(event, admission)| {
                        matches!(
                            admission.private_fact,
                            PrivateAuthorityAdmission::Current(_)
                                | PrivateAuthorityAdmission::Replacement(_)
                        )
                        .then_some(event)
                    })
                    .collect(),
                authority_admission: deferred
                    .events
                    .iter()
                    .zip(deferred.authority_admission.iter().copied())
                    .filter_map(|(_, admission)| {
                        matches!(
                            admission.private_fact,
                            PrivateAuthorityAdmission::Current(_)
                                | PrivateAuthorityAdmission::Replacement(_)
                        )
                        .then_some(admission)
                    })
                    .collect(),
            };
            game.apply_network_actions(&deferred);
            if private_first {
                assert!(game.network_prize_baseline_pending);
                assert_eq!(game.client_authority.view().scope.life_seq, None);
                let retry = round9_replacement_actions(true);
                let retry_events = retry
                    .events
                    .into_iter()
                    .filter(|event| {
                        matches!(
                            event,
                            ClientProtocolEvent::SnapshotReceived(_)
                                | ClientProtocolEvent::PrizePickupAck(_)
                                | ClientProtocolEvent::PlayerPosition(_)
                        )
                    })
                    .collect();
                let (retry_pre, retry_deferred) = partition_browser_network_actions_at_segment(
                    ClientProtocolActions {
                        outbound_packets: Vec::new(),
                        events: retry_events,
                        authority_admission: Vec::new(),
                    },
                    Some(1),
                    Some(1),
                    Some(9),
                    None,
                    None,
                    true,
                    Some(1),
                    true,
                    true,
                    true,
                    game.client_authority.view().segment,
                    false,
                    false,
                    false,
                );
                game.apply_network_actions(&retry_pre);
                game.apply_network_actions(&retry_deferred);
            }
            assert_eq!(
                game.client_authority.view().scope.server_session_id,
                Some(1)
            );
            assert_eq!(game.client_authority.view().scope.arena_stream_id, Some(1));
            assert_eq!(game.client_authority.view().scope.player_id, Some(9));
            assert_eq!(game.client_authority.view().scope.life_seq, Some(1));
            assert_eq!(game.client_authority.view().scope.alive, Some(true));
            assert_eq!(game.last_self_server_prize_life_seq, Some(1));
            assert_eq!(game.last_self_server_prize_revision, Some(7));
            assert!(!game.network_prize_baseline_pending);
            assert!(game.client_authority.view().readiness.joined);
            assert!(game.network_settings_received);
            assert!(game.network_security_synchronized);
            assert!(game.network_zone_map_ready);
            // The fixture stops at the map. Gameplay now also waits for the
            // authored LVZ tail, so publish it through the same event the browser
            // client raises when the last archive installs.
            game.apply_network_actions(&ClientProtocolActions {
                outbound_packets: Vec::new(),
                events: vec![ClientProtocolEvent::ZoneFilesPublished],
                authority_admission: Vec::new(),
            });
            assert!(game.network_zone_files_ready);
            // SECURITY deliberately waits for one strictly-forward door-clock
            // observation, which is outside action application. Once that
            // independent freshness gate is met, no private baseline barrier
            // remains.
            game.door_clock_fresh = true;
            complete_initial_join_sequencer_for_test(&mut game);
            assert!(game.local_gameplay_ready());
            assert_eq!(game.player_bounty, 55);
            assert_eq!(game.ship.max_energy, 1_800.0);
            assert_eq!(game.ship.rotation_per_second, 1.25);
            assert_eq!(game.ship.thrust_acceleration, 425.0);
            assert_eq!(game.ship.top_speed, 515.0);
            assert_eq!(game.ship.recharge_per_second, 67.0);
            assert_eq!(game.ship.gun_level, 2);
            assert_eq!(game.ship.bomb_level, 1);
            assert!(game.applied_collectible_acks.contains(700));
            assert_eq!(game.ship.rockets, 0);
            assert_eq!(game.ship.repels, 4);

            game.apply_network_actions(&duplicate_private);
            assert_eq!(
                game.player_bounty, 55,
                "duplicate replacement batch is idempotent"
            );
            assert!(game.applied_collectible_acks.contains(700));
            assert_eq!(game.ship.rockets, 0);
            assert_eq!(game.ship.repels, 4);
        }
    }
}

fn round9_old_private_events() -> Vec<ClientProtocolEvent> {
    let old_ack = PrizePickupAck {
        server_session_id: u64::MAX,
        arena_stream_id: u64::MAX,
        player_id: u16::MAX - 1,
        collectible_id: 701,
        x: 8_100,
        y: 8_100,
        prize_id: PrizeKind::FullCharge.protocol_id(),
        authoritative_energy: 1_700,
        life_seq: u32::MAX,
        prize_state_revision: u32::MAX,
        engine_shutdown_until_tick: None,
        engine_shutdown_application_required: false,
    };
    vec![
        ClientProtocolEvent::PrizePickupAck(ScopedPrizePickupAck {
            ack: old_ack,
            server_session_id: Some(old_ack.server_session_id),
            arena_stream_id: Some(old_ack.arena_stream_id),
            player_id: Some(old_ack.player_id),
        }),
        private_prize_snapshot(
            u16::MAX,
            u64::MAX,
            SnapshotPrizeState {
                life_seq: u32::MAX,
                revision: u32::MAX,
                bounty: u16::MAX,
                max_energy: u16::MAX,
                rotation_bits: 99.0_f32.to_bits(),
                thrust_bits: 99_000.0_f32.to_bits(),
                recharge_bits: 99_000.0_f32.to_bits(),
                gun_level: 3,
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: u32::MAX,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        ),
    ]
}

#[test]
fn production_game_apply_rejects_old_scope_facts_on_either_side_of_replacement() {
    let old_scope = AuthorityScopeIdentity {
        server_session_id: Some(u64::MAX),
        arena_stream_id: Some(u64::MAX),
        player_id: Some(u16::MAX - 1),
        life_seq: Some(u32::MAX),
        alive: Some(true),
    };
    for old_first in [true, false] {
        let mut actions = round9_replacement_actions(false);
        let mut old = round9_old_private_events();
        if old_first {
            old.append(&mut actions.events);
            actions.events = old;
        } else {
            actions.events.append(&mut old);
        }
        let (pre, deferred) = partition_browser_network_actions(
            actions,
            old_scope.server_session_id,
            old_scope.arena_stream_id,
            old_scope.player_id,
            old_scope.life_seq,
            old_scope.alive,
            true,
            Some(u32::MAX),
            true,
            true,
            true,
            false,
            false,
        );
        assert_eq!(prize_state_carrier_count(&pre), 0);
        assert_eq!(prize_state_carrier_count(&deferred), 1);

        let mut game = Game::new_network_authority_test();
        game.install_network_authority_test_scope(
            old_scope,
            AuthorityReadiness {
                joined: true,
                arena_epoch: Some(u32::MAX),
                settings: true,
                security: true,
                map: true,
            },
            u32::MAX,
        );
        game.apply_network_actions(&pre);
        game.apply_network_actions(&deferred);
        assert_eq!(game.last_self_server_prize_life_seq, Some(1));
        assert_eq!(game.last_self_server_prize_revision, Some(7));
        assert_eq!(game.player_bounty, 55);
        assert!(game.applied_collectible_acks.contains(700));
        assert!(!game.applied_collectible_acks.contains(701));
        assert_eq!(game.ship.rotation_per_second, 1.25);
        assert_eq!(game.ship.thrust_acceleration, 425.0);
    }
}

#[test]
fn production_game_apply_keeps_replacement_closed_until_retried_exact_baseline() {
    let old_scope = AuthorityScopeIdentity {
        server_session_id: Some(u64::MAX),
        arena_stream_id: Some(u64::MAX),
        player_id: Some(u16::MAX - 1),
        life_seq: Some(u32::MAX),
        alive: Some(true),
    };
    let mut initial = round9_replacement_actions(false);
    initial.events.retain(|event| {
        !matches!(
            event,
            ClientProtocolEvent::SnapshotReceived(_)
                | ClientProtocolEvent::PrizePickupAck(_)
                | ClientProtocolEvent::PlayerPosition(_)
        )
    });
    let (pre, deferred) = partition_browser_network_actions(
        initial,
        old_scope.server_session_id,
        old_scope.arena_stream_id,
        old_scope.player_id,
        old_scope.life_seq,
        old_scope.alive,
        true,
        Some(u32::MAX),
        true,
        true,
        true,
        false,
        false,
    );
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        old_scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(u32::MAX),
            settings: true,
            security: true,
            map: true,
        },
        u32::MAX,
    );
    game.apply_network_actions(&pre);
    game.apply_network_actions(&deferred);
    assert!(game.network_prize_baseline_pending);
    assert!(!game.local_gameplay_ready());
    assert_eq!(game.client_authority.view().scope.life_seq, None);
    assert_eq!(game.last_self_server_prize_revision, None);

    let full = round9_replacement_actions(true);
    let repair_events: Vec<_> = full
        .events
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                ClientProtocolEvent::SnapshotReceived(_)
                    | ClientProtocolEvent::PrizePickupAck(_)
                    | ClientProtocolEvent::PlayerPosition(_)
            )
        })
        .collect();
    let (repair_pre, repair_deferred) = partition_browser_network_actions_at_segment(
        ClientProtocolActions {
            outbound_packets: Vec::new(),
            events: repair_events,
            authority_admission: Vec::new(),
        },
        Some(1),
        Some(1),
        Some(9),
        None,
        None,
        true,
        Some(1),
        true,
        true,
        true,
        game.client_authority.view().segment,
        false,
        false,
        false,
    );
    assert!(repair_pre.events.is_empty());
    assert_eq!(prize_state_carrier_count(&repair_deferred), 1);
    game.apply_network_actions(&repair_deferred);
    assert_eq!(game.client_authority.view().scope.life_seq, Some(1));
    assert_eq!(game.last_self_server_prize_revision, Some(7));
    assert!(game.applied_collectible_acks.contains(700));
    assert!(!game.network_prize_baseline_pending);
}

#[test]
fn hard_disconnect_leave_join_player_and_stream_boundaries_reset_before_gameplay() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(77),
        arena_stream_id: Some(3),
        player_id: Some(9),
        life_seq: Some(41),
        alive: Some(true),
    };
    let stale = private_prize_snapshot(
        90,
        3,
        SnapshotPrizeState {
            life_seq: 41,
            revision: 12,
            bounty: 900,
            max_energy: 1_900,
            rotation_bits: 8.0_f32.to_bits(),
            thrust_bits: 8_000.0_f32.to_bits(),
            recharge_bits: 8_000.0_f32.to_bits(),
            gun_level: 3,
            ..SnapshotPrizeState::default()
        },
        Some(SnapshotLifecycleState {
            life_seq: 41,
            alive: true,
            ..SnapshotLifecycleState::default()
        }),
    );
    let boundaries = [
        ClientProtocolEvent::Disconnected,
        ClientProtocolEvent::PlayerLeft(9),
        ClientProtocolEvent::JoinedArena(8),
        ClientProtocolEvent::PlayerIdAssigned(9),
        ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
            server_session_id: 78,
            arena_stream_id: 4,
            ..ServerClockBootstrap::default()
        }),
    ];

    for boundary in boundaries {
        for stale_first in [true, false] {
            let events = if stale_first {
                vec![stale.clone(), boundary.clone()]
            } else {
                vec![boundary.clone(), stale.clone()]
            };
            let (pre, deferred) = partition_browser_network_actions(
                ClientProtocolActions {
                    outbound_packets: Vec::new(),
                    events,
                    authority_admission: Vec::new(),
                },
                scope.server_session_id,
                scope.arena_stream_id,
                scope.player_id,
                scope.life_seq,
                scope.alive,
                true,
                Some(7),
                true,
                true,
                true,
                false,
                false,
            );
            assert_eq!(
                pre.events,
                vec![ClientProtocolEvent::PreGameplayAuthorityReset]
            );
            assert_eq!(prize_state_carrier_count(&deferred), 0);

            let mut game = Game::new_network_authority_test();
            game.install_network_authority_test_scope(
                scope,
                AuthorityReadiness {
                    joined: true,
                    arena_epoch: Some(7),
                    settings: true,
                    security: true,
                    map: true,
                },
                7,
            );
            game.ship.energy = 1_500.0;
            game.ship.rotation_per_second = 8.0;
            game.ship.thrust_acceleration = 8_000.0;
            game.ship.recharge_per_second = 8_000.0;
            game.ship.gun_level = 3;
            game.apply_network_actions(&pre);
            assert!(game.network_prize_baseline_pending);
            assert!(!game.local_gameplay_ready());
            assert_eq!(game.ship.energy, 0.0);
            // Base movement tuning survives; see the cutover case above.
            let base_tuning = game.ship_tuning();
            assert_eq!(
                game.ship.rotation_per_second,
                base_tuning.rotation_per_second
            );
            assert_eq!(
                game.ship.thrust_acceleration,
                base_tuning.thrust_acceleration
            );
            assert_eq!(game.ship.recharge_per_second, base_tuning.energy_recharge);
            assert_eq!(game.ship.gun_level, 0);
            game.apply_network_actions(&deferred);
            assert!(game.network_prize_baseline_pending);
            assert_eq!(game.last_self_server_prize_revision, None);
        }
    }
}

#[test]
fn immutable_admission_segments_distinguish_identical_wrapped_scope_across_many_hard_boundaries() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(u64::MAX),
        arena_stream_id: Some(u64::MAX),
        player_id: Some(u16::MAX),
        life_seq: Some(u32::MAX),
        alive: Some(true),
    };
    let old = private_prize_snapshot(
        40,
        u64::MAX,
        SnapshotPrizeState {
            life_seq: u32::MAX,
            revision: 40,
            bounty: 4_000,
            max_energy: 4_000,
            rotation_bits: 40.0_f32.to_bits(),
            ..SnapshotPrizeState::default()
        },
        Some(SnapshotLifecycleState {
            life_seq: u32::MAX,
            alive: true,
            ..SnapshotLifecycleState::default()
        }),
    );
    let replacement = private_prize_snapshot(
        41,
        u64::MAX,
        SnapshotPrizeState {
            life_seq: u32::MAX,
            revision: 41,
            bounty: 41,
            max_energy: 1_741,
            rotation_bits: 1.41_f32.to_bits(),
            thrust_bits: 441.0_f32.to_bits(),
            top_speed_bits: 541.0_f32.to_bits(),
            recharge_bits: 71.0_f32.to_bits(),
            ..SnapshotPrizeState::default()
        },
        Some(SnapshotLifecycleState {
            life_seq: u32::MAX,
            alive: true,
            ..SnapshotLifecycleState::default()
        }),
    );
    let actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            old,
            ClientProtocolEvent::Disconnected,
            ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
                server_session_id: u64::MAX,
                arena_stream_id: u64::MAX,
                ..ServerClockBootstrap::default()
            }),
            ClientProtocolEvent::PlayerIdAssigned(u16::MAX),
            ClientProtocolEvent::JoinedArena(u32::MAX),
            ClientProtocolEvent::ArenaSettingsUpdated(round9_test_settings()),
            // Exercise the real cross-channel order: SECURITY may drain
            // independently, but it never changes the segment of a private
            // snapshot fact.
            ClientProtocolEvent::SecurityChallengeReceived {
                challenge: SecurityChallenge::default(),
                server_session_id: Some(u64::MAX),
                arena_epoch: u32::MAX,
            },
            ClientProtocolEvent::ZoneMapPublished,
            replacement,
            ClientProtocolEvent::PlayerPosition(authority_test_position(
                u16::MAX,
                8_210,
                8_211,
                941,
                0,
            )),
        ],
        authority_admission: Vec::new(),
    };
    let plan = build_authority_admission_plan(
        &actions,
        AuthorityAdmissionSeed {
            scope,
            readiness: AuthorityReadiness {
                joined: true,
                arena_epoch: Some(u32::MAX),
                settings: true,
                security: true,
                map: true,
            },
            segment: 0,
        },
    );
    assert_eq!(plan[0].segment, 0);
    assert_eq!(plan[0].private_fact, PrivateAuthorityAdmission::Reject);
    assert_eq!(plan[1].segment, 1);
    assert_eq!(plan[2].segment, 2);
    assert_eq!(plan[3].segment, 3);
    assert_eq!(plan[4].segment, 4);
    assert!(matches!(
        plan[4].boundary,
        AuthorityBoundaryAdmission::Hard { segment: 4, .. }
    ));
    assert!(matches!(
        plan[8].private_fact,
        PrivateAuthorityAdmission::Replacement(candidate) if candidate == scope
    ));
    assert!(matches!(
        plan[9].private_fact,
        PrivateAuthorityAdmission::Replacement(candidate) if candidate == scope
    ));

    let (pre, deferred) = partition_browser_network_actions(
        actions,
        scope.server_session_id,
        scope.arena_stream_id,
        scope.player_id,
        scope.life_seq,
        scope.alive,
        true,
        Some(u32::MAX),
        true,
        true,
        true,
        false,
        false,
    );
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(u32::MAX),
            settings: true,
            security: true,
            map: true,
        },
        u32::MAX,
    );
    game.apply_network_actions(&pre);
    assert_eq!(game.last_self_server_prize_revision, None);
    game.apply_network_actions(&deferred);
    assert_eq!(game.last_self_server_prize_life_seq, Some(u32::MAX));
    assert_eq!(game.last_self_server_prize_revision, Some(41));
    assert_eq!(game.player_bounty, 41);
    assert_eq!(game.ship.energy, 941.0);
    assert_eq!(game.ship.rotation_per_second, 1.41);
    assert!(!game.network_prize_baseline_pending);
}

#[test]
fn production_game_rejected_snapshot_is_wholly_inert_until_exact_replacement_baseline() {
    let old_scope = AuthorityScopeIdentity {
        server_session_id: Some(100),
        arena_stream_id: Some(200),
        player_id: Some(9),
        life_seq: Some(9),
        alive: Some(true),
    };
    let old_snapshot = SnapshotFrame {
        header: SnapshotHeader {
            flags: SNAPSHOT_FLAG_FULL,
            snapshot_id: 50,
            tick: 5_000,
            arena_stream_id: 200,
            ..SnapshotHeader::default()
        },
        near_entities: vec![SnapshotEntity::near_full(
            9,
            SnapshotEntityState {
                x: 7_777,
                y: 7_778,
                energy: 1_999,
                weapon: 0x1234,
                life_seq: 9,
                ..SnapshotEntityState::default()
            },
        )],
        events: vec![
            SnapshotEvent::WatchDamage(SnapshotWatchDamage {
                victim_id: 9,
                shooter_id: 77,
                victim_life_seq: Some(9),
                shooter_life_seq: Some(3),
                weapon_data: 0x1234,
                damage: 400,
                energy_before: Some(1_999),
                energy_after: 1_599,
                server_tick: Some(5_000),
                damage_seq: Some(90),
                projectile_id: Some(91),
                lethal: false,
            }),
            SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                life_seq: 9,
                alive: true,
                repel_until_tick: Some(5_200),
                repel_delivery: Some(SnapshotRepelDelivery {
                    active_steps: 12,
                    delivery_generation: 9,
                    source_player_id: 77,
                    source_life_seq: 3,
                    origin_x: 7_700,
                    origin_y: 7_700,
                    ..SnapshotRepelDelivery::default()
                }),
                ..SnapshotLifecycleState::default()
            }),
            SnapshotEvent::RocketState(SnapshotRocketState {
                life_seq: 9,
                activation_sequence: Some(80),
                until_tick: Some(5_900),
                active: true,
                rockets: 9,
                ..SnapshotRocketState::default()
            }),
            SnapshotEvent::PrizeState(SnapshotPrizeState {
                life_seq: 9,
                revision: 90,
                bounty: 999,
                max_energy: 1_999,
                rotation_bits: 9.0_f32.to_bits(),
                thrust_bits: 9_000.0_f32.to_bits(),
                recharge_bits: 9_000.0_f32.to_bits(),
                gun_level: 3,
                repels: 9,
                ..SnapshotPrizeState::default()
            }),
        ],
        ..SnapshotFrame::default()
    };
    let old_damage = ClientProtocolEvent::DamageWatched(ServerDamageReport {
        player_id: 9,
        timestamp: 5_000,
        entries: vec![ServerDamageReportEntry {
            attacker_id: 77,
            weapon_data: 0x1234,
            energy: 1_599,
            damage: 400,
            timestamp_offset: 0,
            energy_before: Some(1_999),
            damage_seq: Some(90),
            projectile_id: Some(91),
            victim_life_seq: Some(9),
            shooter_life_seq: Some(3),
            explicit_lethal: Some(false),
            source_snapshot_id: Some(50),
            source_snapshot_epoch: Some(2),
        }],
    });
    for old_first in [true, false] {
        let replacement = vec![
            ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
                server_session_id: 101,
                arena_stream_id: 201,
                ..ServerClockBootstrap::default()
            }),
            ClientProtocolEvent::PlayerIdAssigned(10),
            ClientProtocolEvent::JoinedArena(2),
            ClientProtocolEvent::ArenaSettingsUpdated(round9_test_settings()),
            ClientProtocolEvent::SecurityChallengeReceived {
                challenge: SecurityChallenge::default(),
                server_session_id: Some(101),
                arena_epoch: 2,
            },
            ClientProtocolEvent::ZoneMapPublished,
        ];
        let stale = vec![
            old_damage.clone(),
            ClientProtocolEvent::SnapshotReceived(old_snapshot.clone()),
            ClientProtocolEvent::PlayerPosition(authority_test_position(
                9, 7_777, 7_778, 1_599, 0x1234,
            )),
            ClientProtocolEvent::SnapshotPlayerPosition {
                update: PlayerPositionUpdate {
                    timestamp: old_snapshot.header.tick as u16,
                    ..authority_test_position(9, 7_777, 7_778, 1_599, 0x1234)
                },
                snapshot_id: old_snapshot.header.snapshot_id,
                server_tick: old_snapshot.header.tick,
                arena_stream_id: old_snapshot.header.arena_stream_id,
            },
        ];
        let events = if old_first {
            stale.into_iter().chain(replacement).collect()
        } else {
            replacement.into_iter().chain(stale).collect()
        };
        let replacement_context = ClientProtocolActions {
            outbound_packets: Vec::new(),
            events,
            authority_admission: Vec::new(),
        };
        let (pre, deferred) = partition_browser_network_actions(
            replacement_context,
            old_scope.server_session_id,
            old_scope.arena_stream_id,
            old_scope.player_id,
            old_scope.life_seq,
            old_scope.alive,
            true,
            Some(1),
            true,
            true,
            true,
            false,
            false,
        );
        assert!(
            deferred
                .authority_admission
                .iter()
                .any(|admission| { admission.private_fact == PrivateAuthorityAdmission::Reject })
        );
        assert!(
            deferred
                .events
                .iter()
                .zip(&deferred.authority_admission)
                .filter(|(event, _)| matches!(event, ClientProtocolEvent::PlayerPosition(_)))
                .all(|(_, admission)| {
                    admission.private_fact == PrivateAuthorityAdmission::Reject
                })
        );
        let mut game = Game::new_network_authority_test();
        game.install_network_authority_test_scope(
            old_scope,
            AuthorityReadiness {
                joined: true,
                arena_epoch: Some(1),
                settings: true,
                security: true,
                map: true,
            },
            1,
        );
        game.ship.energy = 1_700.0;
        game.ship.rockets = 7;
        game.ship.repels = 7;
        game.apply_network_actions(&pre);
        game.apply_network_actions(&deferred);
        game.apply_network_actions(&deferred);
        assert!(game.network_prize_baseline_pending);
        assert_eq!(game.client_authority.view().scope.life_seq, None);
        assert_eq!(game.last_self_server_prize_revision, None);
        assert_eq!(game.last_self_server_rocket_life_seq, None);
        assert_eq!(game.repel_delivery_frames_seen, 0);
        assert_eq!(game.last_network_damage_server_seq, None);
        assert_eq!(game.network_world.last_received_snapshot_id, None);
        assert!(game.network_world.snapshot_entity_state.is_empty());
        assert_eq!(game.ship.energy, 0.0);
        assert_eq!(game.ship.rockets, 0);
        assert_eq!(game.ship.repels, 0);

        let valid_snapshot = ClientProtocolEvent::SnapshotReceived(SnapshotFrame {
            header: SnapshotHeader {
                flags: SNAPSHOT_FLAG_FULL,
                snapshot_id: 51,
                tick: 5_100,
                arena_stream_id: 201,
                ..SnapshotHeader::default()
            },
            near_entities: vec![SnapshotEntity::near_full(
                10,
                SnapshotEntityState {
                    x: 8_200,
                    y: 8_201,
                    energy: 900,
                    life_seq: 10,
                    ..SnapshotEntityState::default()
                },
            )],
            events: vec![
                SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                    life_seq: 10,
                    alive: true,
                    ..SnapshotLifecycleState::default()
                }),
                SnapshotEvent::RocketState(SnapshotRocketState {
                    life_seq: 10,
                    until_tick: Some(5_900),
                    active: true,
                    rockets: 2,
                    ..SnapshotRocketState::default()
                }),
                SnapshotEvent::PrizeState(SnapshotPrizeState {
                    life_seq: 10,
                    revision: 2,
                    bounty: 52,
                    max_energy: 1_800,
                    rotation_bits: 1.2_f32.to_bits(),
                    thrust_bits: 420.0_f32.to_bits(),
                    top_speed_bits: 520.0_f32.to_bits(),
                    recharge_bits: 72.0_f32.to_bits(),
                    repels: 3,
                    ..SnapshotPrizeState::default()
                }),
            ],
            ..SnapshotFrame::default()
        });
        let (valid_pre, valid_deferred) = partition_browser_network_actions_at_segment(
            ClientProtocolActions {
                outbound_packets: Vec::new(),
                events: vec![
                    valid_snapshot,
                    ClientProtocolEvent::PlayerPosition(authority_test_position(
                        10, 8_200, 8_201, 900, 0,
                    )),
                ],
                authority_admission: Vec::new(),
            },
            Some(101),
            Some(201),
            Some(10),
            None,
            None,
            true,
            Some(2),
            true,
            true,
            true,
            game.client_authority.view().segment,
            false,
            false,
            false,
        );
        game.apply_network_actions(&valid_pre);
        game.apply_network_actions(&valid_deferred);
        game.apply_network_actions(&valid_deferred);
        assert_eq!(game.client_authority.view().scope.life_seq, Some(10));
        assert_eq!(game.last_self_server_prize_revision, Some(2));
        assert_eq!(game.last_self_server_rocket_life_seq, Some(10));
        assert_eq!(game.ship.rockets, 2);
        assert_eq!(game.ship.repels, 3);
        assert_eq!(game.ship.energy, 900.0);
        assert_eq!(game.network_world.last_received_snapshot_id, Some(51));
        assert_eq!(
            game.network_world.snapshot_entity_state.get(&10),
            Some(&(8_200, 8_201))
        );
        assert!(!game.network_prize_baseline_pending);
    }
}

#[test]
fn rejected_snapshot_damage_does_not_consume_later_realtime_copy() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(101),
        arena_stream_id: Some(201),
        player_id: Some(10),
        life_seq: Some(10),
        alive: Some(true),
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(2),
            settings: true,
            security: true,
            map: true,
        },
        2,
    );
    let snapshot_copy = ClientProtocolEvent::DamageWatched(ServerDamageReport {
        player_id: 768,
        timestamp: 5_000,
        entries: vec![ServerDamageReportEntry {
            attacker_id: 10,
            weapon_data: 0x42,
            energy: 0,
            damage: 383,
            timestamp_offset: 0,
            energy_before: Some(383),
            damage_seq: Some(56),
            projectile_id: Some(69),
            victim_life_seq: Some(2),
            shooter_life_seq: Some(8),
            explicit_lethal: Some(true),
            source_snapshot_id: Some(50),
            source_snapshot_epoch: Some(2),
        }],
    });
    let rejected = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![snapshot_copy.clone()],
        authority_admission: Vec::new(),
    };
    game.apply_network_actions(&rejected);
    assert!(game.recent_network_damage.is_empty());
    assert_eq!(game.last_network_damage_server_seq, None);

    let mut realtime_copy = snapshot_copy;
    let ClientProtocolEvent::DamageWatched(report) = &mut realtime_copy else {
        unreachable!();
    };
    report.entries[0].source_snapshot_id = None;
    report.entries[0].source_snapshot_epoch = None;
    let admitted = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![realtime_copy],
        authority_admission: Vec::new(),
    };
    assert_eq!(game.apply_network_actions(&admitted), vec![true]);
    assert_eq!(game.last_network_damage_server_seq, Some(56));
    assert_eq!(game.network_damage_event_seq, 1);
    assert_eq!(game.recent_network_damage.len(), 1);

    assert_eq!(game.apply_network_actions(&admitted), vec![false]);
    assert_eq!(game.network_damage_event_seq, 1);
    assert_eq!(game.recent_network_damage.len(), 1);
}

#[test]
fn production_game_security_readiness_is_segment_bound_across_reused_wrapped_scope() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(u64::MAX),
        arena_stream_id: Some(u64::MAX),
        player_id: Some(u16::MAX),
        life_seq: Some(u32::MAX),
        alive: Some(true),
    };
    let readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(u32::MAX),
        settings: true,
        security: true,
        map: true,
    };
    let old_challenge = SecurityChallenge {
        prize_seed: 0x1111_1111,
        door_seed: 0x2222_2222,
        timestamp: 10,
        checksum_key: 0x3333_3333,
    };
    let wrong_challenge = SecurityChallenge {
        prize_seed: 0xaaaa_aaaa,
        door_seed: 0xbbbb_bbbb,
        timestamp: 11,
        checksum_key: 0xcccc_cccc,
    };
    let final_challenge = SecurityChallenge {
        prize_seed: 0x4444_4444,
        door_seed: 0x5555_5555,
        timestamp: 12,
        checksum_key: 0x6666_6666,
    };
    let replacement_snapshot = || {
        private_prize_snapshot(
            u16::MAX,
            u64::MAX,
            SnapshotPrizeState {
                life_seq: u32::MAX,
                revision: u32::MAX,
                bounty: 77,
                max_energy: 1_777,
                rotation_bits: 1.77_f32.to_bits(),
                thrust_bits: 477.0_f32.to_bits(),
                top_speed_bits: 577.0_f32.to_bits(),
                recharge_bits: 77.0_f32.to_bits(),
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: u32::MAX,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        )
    };
    let replacement_position = || {
        ClientProtocolEvent::PlayerPosition(authority_test_position(u16::MAX, 8_100, 8_101, 977, 0))
    };

    // SECURITY and reliable old-generation facts may arrive in either merged
    // channel order, including duplicates and an identity collision. Every
    // one is still pre-Disconnect segment zero and must remain rejected after
    // the numerically identical replacement generation is established.
    for old_security_first in [true, false] {
        for retry_snapshot_first in [true, false] {
            let old_security = ClientProtocolEvent::SecurityChallengeReceived {
                challenge: old_challenge,
                server_session_id: Some(u64::MAX),
                arena_epoch: u32::MAX,
            };
            let wrong_security = ClientProtocolEvent::SecurityChallengeReceived {
                challenge: wrong_challenge,
                server_session_id: Some(u64::MAX - 1),
                arena_epoch: u32::MAX,
            };
            let mut old_segment = if old_security_first {
                vec![
                    old_security.clone(),
                    ClientProtocolEvent::ArenaSettingsUpdated(round9_test_settings()),
                    wrong_security,
                    old_security,
                ]
            } else {
                vec![
                    ClientProtocolEvent::ArenaSettingsUpdated(round9_test_settings()),
                    old_security.clone(),
                    old_security,
                    wrong_security,
                ]
            };
            old_segment.extend([
                ClientProtocolEvent::Disconnected,
                ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
                    server_session_id: u64::MAX,
                    arena_stream_id: u64::MAX,
                    ..ServerClockBootstrap::default()
                }),
                ClientProtocolEvent::PlayerIdAssigned(u16::MAX),
                ClientProtocolEvent::JoinedArena(u32::MAX),
                ClientProtocolEvent::ArenaSettingsUpdated(round9_test_settings()),
                ClientProtocolEvent::ZoneFilesPending,
                ClientProtocolEvent::ZoneMapPublished,
                ClientProtocolEvent::ZoneFilesPublished,
                replacement_snapshot(),
                replacement_position(),
            ]);
            let actions = ClientProtocolActions {
                outbound_packets: Vec::new(),
                events: old_segment,
                authority_admission: Vec::new(),
            };
            let plan = build_authority_admission_plan(
                &actions,
                AuthorityAdmissionSeed {
                    scope,
                    readiness,
                    segment: 0,
                },
            );
            let final_segment = plan
                .iter()
                .map(|admission| admission.segment)
                .max()
                .expect("fixture is nonempty");
            assert_eq!(final_segment, 4, "all hard boundaries are represented");
            for (event, admission) in actions.events.iter().zip(&plan) {
                if matches!(event, ClientProtocolEvent::SecurityChallengeReceived { .. }) {
                    assert_eq!(admission.segment, 0);
                    assert_eq!(
                        admission.readiness_fact,
                        AuthorityReadinessAdmission::Reject
                    );
                }
            }
            let replacement_snapshot_admission = actions
                .events
                .iter()
                .zip(&plan)
                .find_map(|(event, admission)| {
                    matches!(event, ClientProtocolEvent::SnapshotReceived(_)).then_some(*admission)
                })
                .expect("replacement snapshot admission");
            assert!(!replacement_snapshot_admission.readiness_after.complete());
            assert!(
                !plan
                    .iter()
                    .skip_while(|admission| admission.segment < 3)
                    .any(|admission| admission.segment == 3 && admission.readiness_after.security),
                "PlayerId hard boundary clears predecessor SECURITY before Join"
            );

            let (pre, deferred) = partition_browser_network_actions(
                actions,
                scope.server_session_id,
                scope.arena_stream_id,
                scope.player_id,
                scope.life_seq,
                scope.alive,
                true,
                Some(u32::MAX),
                true,
                true,
                true,
                false,
                false,
            );
            assert_eq!(
                pre.events,
                vec![ClientProtocolEvent::PreGameplayAuthorityReset]
            );
            assert!(deferred.events.iter().all(|event| !matches!(
                event,
                ClientProtocolEvent::SecurityChallengeReceived { .. }
            )));

            let mut game = Game::new_network_authority_test();
            game.install_network_authority_test_scope(scope, readiness, u32::MAX);
            game.prize_rng_seed = 7_777;
            game.maps[game.map_index].door_rng_seed = 8_888;
            game.apply_network_actions(&pre);
            game.apply_network_actions(&deferred);
            assert!(game.network_prize_baseline_pending);
            assert!(!game.local_gameplay_ready());
            assert_eq!(game.last_self_server_prize_revision, None);
            assert_eq!(game.network_security_challenge, None);
            assert!(!game.network_security_synchronized);
            assert!(!game.door_clock_fresh);
            assert_eq!(game.prize_rng_seed, 7_777);
            assert_eq!(game.maps[game.map_index].door_rng_seed, 8_888);

            let final_security = ClientProtocolEvent::SecurityChallengeReceived {
                challenge: final_challenge,
                server_session_id: Some(u64::MAX),
                arena_epoch: u32::MAX,
            };
            let final_wrong_identity = ClientProtocolEvent::SecurityChallengeReceived {
                challenge: wrong_challenge,
                server_session_id: Some(u64::MAX - 1),
                arena_epoch: u32::MAX,
            };
            let private = vec![replacement_snapshot(), replacement_position()];
            let mut retry_events: Vec<ClientProtocolEvent> = if retry_snapshot_first {
                private
                    .into_iter()
                    .chain([final_wrong_identity, final_security.clone(), final_security])
                    .collect()
            } else {
                [final_wrong_identity, final_security.clone(), final_security]
                    .into_iter()
                    .chain(private)
                    .collect()
            };
            // Preserve the snapshot-derived position adjacency when the
            // snapshot is ordered first; it is one carrier lineage.
            if retry_snapshot_first {
                assert!(matches!(
                    retry_events.first(),
                    Some(ClientProtocolEvent::SnapshotReceived(_))
                ));
            }
            let (retry_pre, retry_deferred) = partition_browser_network_actions_at_segment(
                ClientProtocolActions {
                    outbound_packets: Vec::new(),
                    events: std::mem::take(&mut retry_events),
                    authority_admission: Vec::new(),
                },
                Some(u64::MAX),
                Some(u64::MAX),
                Some(u16::MAX),
                Some(u32::MAX),
                Some(true),
                true,
                Some(u32::MAX),
                true,
                false,
                true,
                game.client_authority.view().segment,
                false,
                false,
                false,
            );
            assert!(
                retry_pre
                    .authority_admission
                    .iter()
                    .all(|admission| admission.readiness_after.complete())
            );
            assert_eq!(
                retry_deferred
                    .events
                    .iter()
                    .filter(|event| matches!(
                        event,
                        ClientProtocolEvent::SecurityChallengeReceived { .. }
                    ))
                    .count(),
                2,
                "wrong-identity SECURITY is dropped; exact retry+duplicate remain"
            );
            game.apply_network_actions(&retry_pre);
            assert!(!game.local_gameplay_ready());
            game.apply_network_actions(&retry_deferred);
            assert_eq!(game.network_security_challenge, Some(final_challenge));
            assert!(game.network_security_synchronized);
            assert_eq!(game.prize_rng_seed, final_challenge.prize_seed as i32);
            assert_eq!(
                game.maps[game.map_index].door_rng_seed,
                final_challenge.door_seed as i32
            );
            assert_eq!(game.last_self_server_prize_revision, Some(u32::MAX));
            assert_eq!(game.player_bounty, 77);
            assert_eq!(game.ship.energy, 977.0);
            assert!(!game.network_prize_baseline_pending);
            game.door_clock_fresh = true;
            complete_initial_join_sequencer_for_test(&mut game);
            assert!(game.local_gameplay_ready());

            // Reapplying an exact duplicate is idempotent: it cannot rewind
            // RNG progress or reinstall the private baseline.
            game.prize_rng_seed = game.prize_rng_seed.wrapping_add(1);
            game.maps[game.map_index].door_rng_seed =
                game.maps[game.map_index].door_rng_seed.wrapping_add(1);
            let advanced_prize_rng = game.prize_rng_seed;
            let advanced_door_rng = game.maps[game.map_index].door_rng_seed;
            game.apply_network_actions(&retry_deferred);
            assert_eq!(game.prize_rng_seed, advanced_prize_rng);
            assert_eq!(game.maps[game.map_index].door_rng_seed, advanced_door_rng);
            assert_eq!(game.last_self_server_prize_revision, Some(u32::MAX));
            assert_eq!(game.player_bounty, 77);
        }
    }
}

#[test]
fn three_poll_hard_replacement_retires_every_held_gameplay_generation_before_release() {
    let session = u64::MAX;
    let stream = u64::MAX;
    let player = u16::MAX;
    let life = u32::MAX;
    let epoch = u32::MAX;
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(session),
        arena_stream_id: Some(stream),
        player_id: Some(player),
        life_seq: Some(life),
        alive: Some(true),
    };
    let readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(epoch),
        settings: true,
        security: true,
        map: true,
    };
    let scoped_warp = |collectible_id, x, y| {
        ClientProtocolEvent::PrizePickupAck(ScopedPrizePickupAck {
            ack: PrizePickupAck {
                server_session_id: session,
                arena_stream_id: stream,
                player_id: player,
                collectible_id,
                x,
                y,
                prize_id: PrizeKind::Warp.protocol_id(),
                authoritative_energy: 900,
                life_seq: life,
                prize_state_revision: 1,
                engine_shutdown_until_tick: None,
                engine_shutdown_application_required: false,
            },
            server_session_id: Some(session),
            arena_stream_id: Some(stream),
            player_id: Some(player),
        })
    };
    let repel_snapshot = |snapshot_id, generation| {
        ClientProtocolEvent::SnapshotReceived(SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id,
                tick: 500,
                arena_stream_id: stream,
                ..SnapshotHeader::default()
            },
            events: vec![SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                life_seq: life,
                alive: true,
                repel_until_tick: Some(550),
                repel_delivery: Some(SnapshotRepelDelivery {
                    active_steps: 30,
                    delivery_generation: generation,
                    source_player_id: 2,
                    source_life_seq: 3,
                    origin_x: 8_000,
                    origin_y: 8_000,
                    expected_velocity_x_bits: 300.0_f32.to_bits(),
                    expected_velocity_y_bits: 0.0_f32.to_bits(),
                    ..SnapshotRepelDelivery::default()
                }),
                ..SnapshotLifecycleState::default()
            })],
            ..SnapshotFrame::default()
        })
    };
    let hidden = |collectible_id, x| {
        ClientProtocolEvent::HiddenPrizeSpawn(HiddenPrizeSpawn {
            collectible_id,
            x,
            y: x,
            prize_id: PrizeKind::Recharge.protocol_id(),
            ttl_ticks: 500,
        })
    };
    let killed = |green_collectible_id, killed_id| {
        ClientProtocolEvent::PlayerDeath(PlayerDeath {
            green_id: -1,
            green_collectible_id,
            killer_id: player,
            killed_id,
            bounty: 50,
            flag_transfer: 0,
            life_seq: life,
            respawn_at_tick: 900,
        })
    };

    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(scope, readiness, epoch);
    game.ship.pos = vec2(8_192.0, 8_192.0);
    game.player_bounty = 20;

    // Poll 1: every gameplay carrier is held behind the correction barrier.
    let old_actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            scoped_warp(1_001, 7_000, 7_000),
            repel_snapshot(10, 10),
            ClientProtocolEvent::SetCoordinates(SetCoordinates { x: 100, y: 101 }),
            hidden(1_002, 7_100),
            ClientProtocolEvent::CollectedPrize(CollectedPrize {
                count: 2,
                prize_id: PrizeKind::Rocket.protocol_id(),
            }),
            killed(1_003, player),
        ],
        authority_admission: Vec::new(),
    };
    let (old_pre, old_deferred) = partition_browser_network_actions_at_segment(
        old_actions,
        Some(session),
        Some(stream),
        Some(player),
        Some(life),
        Some(true),
        true,
        Some(epoch),
        true,
        true,
        true,
        game.client_authority.view().segment,
        true,
        false,
        false,
    );
    assert!(old_pre.events.is_empty());
    let (ready, mut held) = partition_network_actions_behind_movement_correction_at_segment(
        old_deferred,
        true,
        game.client_authority.view().segment,
    );
    assert!(ready.events.is_empty());
    assert_eq!(held.events.len(), 6);

    // Poll 2: a hard replacement deliberately reuses every wrapped numeric
    // identity. Its valid gameplay facts remain held, while all poll-1 facts
    // are atomically retired before any terminal can release them.
    let baseline = private_prize_snapshot(
        20,
        stream,
        SnapshotPrizeState {
            life_seq: life,
            revision: 1,
            bounty: 30,
            max_energy: 1_700,
            rotation_bits: 1.0_f32.to_bits(),
            thrust_bits: 400.0_f32.to_bits(),
            top_speed_bits: 500.0_f32.to_bits(),
            recharge_bits: 60.0_f32.to_bits(),
            ..SnapshotPrizeState::default()
        },
        Some(SnapshotLifecycleState {
            life_seq: life,
            alive: true,
            ..SnapshotLifecycleState::default()
        }),
    );
    let replacement = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::Disconnected,
            ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
                server_session_id: session,
                arena_stream_id: stream,
                tick_rate_hz: 100,
                snapshot_rate_hz: 60,
                ..ServerClockBootstrap::default()
            }),
            ClientProtocolEvent::PlayerIdAssigned(player),
            ClientProtocolEvent::JoinedArena(epoch),
            ClientProtocolEvent::ArenaSettingsUpdated(round9_test_settings()),
            ClientProtocolEvent::SecurityChallengeReceived {
                challenge: SecurityChallenge::default(),
                server_session_id: Some(session),
                arena_epoch: epoch,
            },
            ClientProtocolEvent::ZoneMapPublished,
            baseline,
            ClientProtocolEvent::PlayerPosition(authority_test_position(
                player, 8_192, 8_192, 900, 0,
            )),
            scoped_warp(2_001, 8_300, 8_300),
            ClientProtocolEvent::SetCoordinates(SetCoordinates { x: 120, y: 121 }),
            repel_snapshot(21, 21),
            hidden(2_002, 8_400),
            ClientProtocolEvent::CollectedPrize(CollectedPrize {
                count: 1,
                prize_id: PrizeKind::Rocket.protocol_id(),
            }),
            killed(2_003, 2),
        ],
        authority_admission: Vec::new(),
    };
    let (replacement_pre, replacement_deferred) = partition_browser_network_actions_at_segment(
        replacement,
        Some(session),
        Some(stream),
        Some(player),
        Some(life),
        Some(true),
        true,
        Some(epoch),
        true,
        true,
        true,
        game.client_authority.view().segment,
        true,
        false,
        false,
    );
    game.apply_network_actions(&replacement_pre);
    held.append(replacement_deferred);
    let (replacement_boundaries, held) =
        partition_network_actions_behind_movement_correction_at_segment(
            held,
            true,
            game.client_authority.view().segment,
        );
    game.apply_network_actions(&replacement_boundaries);
    assert_eq!(
        held.authority_admission.iter().map(|a| a.segment).min(),
        Some(4)
    );
    assert!(!game.applied_collectible_acks.contains(1_001));
    assert!(
        !game
            .prizes
            .iter()
            .any(|prize| prize.collectible_id == 1_002)
    );
    assert_eq!(game.warps.len(), 0);
    assert_eq!(game.player_bounty, 0);
    assert_eq!(game.client_authority.view().scope.alive, None);
    assert!(game.pending_repel_application.is_none());
    assert!(!game.local_gameplay_ready());

    // Poll 3: the matching terminal releases only the replacement generation.
    let (ready, still_held) = partition_network_actions_behind_movement_correction_at_segment(
        held,
        false,
        game.client_authority.view().segment,
    );
    assert!(still_held.events.is_empty());
    game.last_observed_browser_server_tick = Some(500);
    assert!(ready.events.iter().any(|event| matches!(
        event,
        ClientProtocolEvent::HiddenPrizeSpawn(spawn) if spawn.collectible_id == 2_002
    )));
    let final_gate = admitted_authority_actions(
        &ready,
        &ready.authority_admission,
        game.client_authority.view().segment,
        AuthorityScopeIdentity {
            server_session_id: game.client_authority.view().scope.server_session_id,
            arena_stream_id: game.client_authority.view().scope.arena_stream_id,
            player_id: game.client_authority.view().scope.player_id,
            life_seq: game.client_authority.view().scope.life_seq,
            alive: game.client_authority.view().scope.alive,
        },
    );
    let gate_trace: Vec<_> = ready
        .events
        .iter()
        .zip(&ready.authority_admission)
        .zip(&final_gate)
        .map(|((event, admission), admitted)| (format!("{event:?}"), *admission, *admitted))
        .collect();
    assert!(ready
        .events
        .iter()
        .zip(final_gate)
        .any(|(event, admitted)| admitted
            && matches!(event, ClientProtocolEvent::HiddenPrizeSpawn(spawn) if spawn.collectible_id == 2_002)), "{gate_trace:#?}");
    game.apply_network_actions(&ready);
    assert!(!game.applied_collectible_acks.contains(1_001));
    assert!(game.applied_collectible_acks.contains(2_001));
    assert!(
        !game
            .prizes
            .iter()
            .any(|prize| prize.collectible_id == 1_002)
    );
    assert!(
        game.prizes
            .iter()
            .any(|prize| prize.collectible_id == 2_002)
    );
    assert_eq!(game.client_authority.view().scope.alive, Some(true));
    assert!(
        game.pending_repel_application.is_some(),
        "seen={} applied={} life_rejects={} timing_rejects={} app_rejects={}",
        game.repel_delivery_frames_seen,
        game.repel_delivery_frames_applied,
        game.repel_delivery_life_rejects,
        game.repel_delivery_timing_rejects,
        game.repel_delivery_application_rejects,
    );
    assert!(
        game.staged_warp_relocation_pending()
            && game.pending_warp_relocation_collectible_id == Some(2_001),
        "the new Warp is staged for its relocation report"
    );
    assert!(!game.warps.is_empty(), "SetCoordinates applies once");
    assert!(
        game.player_bounty > 0,
        "only the new collected prize presents"
    );
    assert_eq!(
        ready
            .events
            .iter()
            .filter(|event| matches!(event, ClientProtocolEvent::SetCoordinates(_)))
            .count(),
        1,
        "one held replacement carrier is released exactly once"
    );
}

#[test]
fn split_poll_player_assignment_resets_all_readiness_until_exact_final_retry() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(91),
        life_seq: Some(101),
        alive: Some(true),
    };
    let readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(111),
        settings: true,
        security: true,
        map: true,
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(scope, readiness, 111);
    game.network_zone_files_ready = true;

    // Poll 1 replaces the player generation with the exact same numeric id.
    // Every readiness component, including NetworkWorld's join/epoch, must be
    // reset before the next planner is seeded.
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::PlayerIdAssigned(91)],
        authority_admission: Vec::new(),
    });
    assert_eq!(game.client_authority.view().segment, 1);
    assert!(!game.client_authority.view().readiness.joined);
    assert_eq!(game.network_world.snapshot_epoch(), None);
    assert!(!game.network_settings_received);
    assert!(!game.network_security_synchronized);
    assert!(!game.network_zone_map_ready);
    assert!(!game.network_zone_files_ready);
    assert_eq!(game.network_security_challenge, None);
    assert!(!game.local_gameplay_ready());

    // Poll 2 joins with a reused epoch and carries a private baseline too
    // early. Join is retained, but no predecessor readiness or private state
    // leaks through the split boundary.
    let early = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::JoinedArena(111),
            private_prize_snapshot(
                31,
                81,
                SnapshotPrizeState {
                    life_seq: 101,
                    revision: 1,
                    bounty: 51,
                    max_energy: 1_700,
                    rotation_bits: 1.0_f32.to_bits(),
                    thrust_bits: 400.0_f32.to_bits(),
                    top_speed_bits: 500.0_f32.to_bits(),
                    recharge_bits: 60.0_f32.to_bits(),
                    ..SnapshotPrizeState::default()
                },
                Some(SnapshotLifecycleState {
                    life_seq: 101,
                    alive: true,
                    ..SnapshotLifecycleState::default()
                }),
            ),
        ],
        authority_admission: Vec::new(),
    };
    let (early_pre, early_deferred) = partition_browser_network_actions_at_segment(
        early,
        Some(71),
        Some(81),
        Some(91),
        game.client_authority.view().scope.life_seq,
        game.client_authority.view().scope.alive,
        false,
        None,
        false,
        false,
        false,
        game.client_authority.view().segment,
        false,
        false,
        false,
    );
    game.apply_network_actions(&early_pre);
    game.apply_network_actions(&early_deferred);
    assert_eq!(game.client_authority.view().segment, 2);
    assert!(game.client_authority.view().readiness.joined);
    assert_eq!(game.network_world.snapshot_epoch(), Some(111));
    assert!(!game.network_settings_received);
    assert!(!game.network_security_synchronized);
    assert!(!game.network_zone_map_ready);
    assert!(!game.network_zone_files_ready);
    assert_eq!(game.last_self_server_prize_revision, None);
    assert!(!game.local_gameplay_ready());

    // Poll 3 retries the complete readiness set and exact current-generation
    // baseline. The persistent generation seed admits it once.
    let final_actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::ArenaSettingsUpdated(round9_test_settings()),
            ClientProtocolEvent::SecurityChallengeReceived {
                challenge: SecurityChallenge::default(),
                server_session_id: Some(71),
                arena_epoch: 111,
            },
            ClientProtocolEvent::ZoneMapPublished,
            ClientProtocolEvent::ZoneFilesPublished,
            private_prize_snapshot(
                32,
                81,
                SnapshotPrizeState {
                    life_seq: 101,
                    revision: 2,
                    bounty: 52,
                    max_energy: 1_700,
                    rotation_bits: 1.0_f32.to_bits(),
                    thrust_bits: 400.0_f32.to_bits(),
                    top_speed_bits: 500.0_f32.to_bits(),
                    recharge_bits: 60.0_f32.to_bits(),
                    ..SnapshotPrizeState::default()
                },
                Some(SnapshotLifecycleState {
                    life_seq: 101,
                    alive: true,
                    ..SnapshotLifecycleState::default()
                }),
            ),
            ClientProtocolEvent::PlayerPosition(authority_test_position(91, 8_192, 8_192, 900, 0)),
        ],
        authority_admission: Vec::new(),
    };
    let (final_pre, final_deferred) = partition_browser_network_actions_at_segment(
        final_actions,
        Some(71),
        Some(81),
        Some(91),
        game.client_authority.view().scope.life_seq,
        game.client_authority.view().scope.alive,
        true,
        Some(111),
        false,
        false,
        false,
        game.client_authority.view().segment,
        false,
        false,
        false,
    );
    game.apply_network_actions(&final_pre);
    game.apply_network_actions(&final_deferred);
    assert_eq!(game.client_authority.view().segment, 2);
    assert!(game.network_settings_received);
    assert!(game.network_security_synchronized);
    assert!(game.network_zone_map_ready);
    assert!(game.network_zone_files_ready);
    assert_eq!(game.last_self_server_prize_revision, Some(2));
    assert_eq!(game.player_bounty, 52);
    assert!(!game.network_prize_baseline_pending);
    game.door_clock_fresh = true;
    complete_initial_join_sequencer_for_test(&mut game);
    assert!(game.local_gameplay_ready());
}

#[test]
fn same_poll_initial_life_discovery_does_not_retire_one_shot_zone_publication() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(401),
        arena_stream_id: Some(402),
        player_id: Some(7),
        life_seq: None,
        alive: None,
    };
    let readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(403),
        settings: true,
        security: true,
        map: false,
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(scope, readiness, 403);
    game.network_zone_files_ready = false;

    // The reliable map publication can drain in the same browser poll as the
    // first private snapshot that discovers life 1. Map/files are arena facts
    // and must survive that soft life cursor advance exactly once.
    let actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::ZoneMapPublished,
            ClientProtocolEvent::ZoneFilesPublished,
            private_prize_snapshot(
                1,
                402,
                SnapshotPrizeState {
                    life_seq: 1,
                    revision: 1,
                    bounty: 50,
                    max_energy: 1_700,
                    rotation_bits: 1.0_f32.to_bits(),
                    thrust_bits: 400.0_f32.to_bits(),
                    top_speed_bits: 500.0_f32.to_bits(),
                    recharge_bits: 60.0_f32.to_bits(),
                    ..SnapshotPrizeState::default()
                },
                Some(SnapshotLifecycleState {
                    life_seq: 1,
                    alive: true,
                    ..SnapshotLifecycleState::default()
                }),
            ),
        ],
        authority_admission: Vec::new(),
    };
    let (pre, deferred) = partition_browser_network_actions_at_segment(
        actions,
        scope.server_session_id,
        scope.arena_stream_id,
        scope.player_id,
        scope.life_seq,
        scope.alive,
        true,
        Some(403),
        true,
        true,
        false,
        game.client_authority.view().segment,
        false,
        false,
        false,
    );
    game.apply_network_actions(&pre);
    game.apply_network_actions(&deferred);

    assert_eq!(game.client_authority.view().scope.life_seq, Some(1));
    assert_eq!(game.client_authority.view().scope.alive, Some(true));
    assert!(game.network_zone_map_ready);
    assert!(game.network_zone_files_ready);
    assert_eq!(game.last_self_server_prize_revision, Some(1));
}

#[test]
fn split_poll_initial_lifecycle_uses_the_single_sequencer_scope() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(411),
        arena_stream_id: Some(412),
        player_id: Some(7),
        life_seq: None,
        alive: None,
    };
    let readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(413),
        settings: true,
        security: true,
        map: true,
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(scope, readiness, 413);
    game.network_zone_files_ready = true;

    // This is the real browser ordering which exposed the migration seam:
    // bootstrap/join/readiness completed in an earlier poll, then the first
    // private lifecycle arrived by itself. Its immutable admission is seeded
    // exclusively from the sequencer.
    let authority = game.client_authority.view();
    let (pre, deferred) = partition_browser_network_actions_at_segment(
        ClientProtocolActions {
            outbound_packets: Vec::new(),
            events: vec![private_prize_snapshot(
                41,
                412,
                SnapshotPrizeState {
                    life_seq: 1,
                    revision: 1,
                    bounty: 50,
                    max_energy: 1_700,
                    rotation_bits: 1.0_f32.to_bits(),
                    thrust_bits: 400.0_f32.to_bits(),
                    top_speed_bits: 500.0_f32.to_bits(),
                    recharge_bits: 60.0_f32.to_bits(),
                    ..SnapshotPrizeState::default()
                },
                Some(SnapshotLifecycleState {
                    life_seq: 1,
                    alive: true,
                    ..SnapshotLifecycleState::default()
                }),
            )],
            authority_admission: Vec::new(),
        },
        authority.scope.server_session_id,
        authority.scope.arena_stream_id,
        authority.scope.player_id,
        authority.scope.life_seq,
        authority.scope.alive,
        authority.readiness.joined,
        authority.readiness.arena_epoch,
        authority.readiness.settings,
        authority.readiness.security,
        authority.readiness.map,
        authority.segment,
        false,
        authority.initial_baseline_pending,
        true,
    );
    assert!(pre.events.is_empty());
    assert!(matches!(
        deferred.events.as_slice(),
        [
            ClientProtocolEvent::SnapshotReceived(_),
            ClientProtocolEvent::PreGameplayPrizeState { .. }
        ]
    ));

    // The former Game-level lifecycle mirrors have been deleted. Final
    // admission and lifecycle classification both consume this single typed
    // sequencer scope, including when the snapshot/private facts arrive in a
    // later poll.
    let admitted = game.apply_network_actions(&deferred);
    assert_eq!(admitted, vec![true, true]);
    assert_eq!(game.client_authority.view().scope.life_seq, Some(1));
    assert_eq!(game.client_authority.view().scope.alive, Some(true));
    assert!(
        game.client_authority
            .view()
            .initial_baseline_ready_to_submit
    );
    assert_eq!(game.client_authority.view().scope.life_seq, Some(1));
    assert_eq!(game.client_authority.view().scope.alive, Some(true));
    assert_eq!(game.network_world.authoritative_snapshot_id, Some(41));
    assert_eq!(game.last_self_server_prize_revision, Some(1));

    // Once the lifecycle fact is installed, later same-life snapshots are
    // classified as Current rather than Replacement. InitialJoin must not
    // block its own private PrizeState prerequisite or the reliable neutral
    // baseline can never be submitted. Ship/frequency/respawn/correction
    // cutovers remain closed by the shared predicate.
    game.network_prize_baseline_pending = true;
    game.last_self_server_prize_life_seq = None;
    game.last_self_server_prize_revision = None;
    game.last_self_server_energy = Some(1_700.0);
    let authority = game.client_authority.view();
    assert_eq!(
        authority.transition_kind,
        Some(ClientAuthorityTransitionKind::InitialJoin)
    );
    assert!(!prize_state_transition_blocks_reconciliation(
        authority.transition_kind,
        authority.scope.alive,
    ));
    let (pre, deferred) = partition_browser_network_actions_at_segment(
        ClientProtocolActions {
            outbound_packets: Vec::new(),
            events: vec![private_prize_snapshot(
                42,
                412,
                SnapshotPrizeState {
                    life_seq: 1,
                    revision: 2,
                    bounty: 51,
                    max_energy: 1_700,
                    rotation_bits: 1.0_f32.to_bits(),
                    thrust_bits: 400.0_f32.to_bits(),
                    top_speed_bits: 500.0_f32.to_bits(),
                    recharge_bits: 60.0_f32.to_bits(),
                    ..SnapshotPrizeState::default()
                },
                Some(SnapshotLifecycleState {
                    life_seq: 1,
                    alive: true,
                    ..SnapshotLifecycleState::default()
                }),
            )],
            authority_admission: Vec::new(),
        },
        authority.scope.server_session_id,
        authority.scope.arena_stream_id,
        authority.scope.player_id,
        authority.scope.life_seq,
        authority.scope.alive,
        authority.readiness.joined,
        authority.readiness.arena_epoch,
        authority.readiness.settings,
        authority.readiness.security,
        authority.readiness.map,
        authority.segment,
        false,
        authority.initial_baseline_pending,
        prize_state_transition_blocks_reconciliation(
            authority.transition_kind,
            authority.scope.alive,
        ),
    );
    assert!(matches!(
        pre.events.as_slice(),
        [ClientProtocolEvent::PreGameplayPrizeState { .. }]
    ));
    game.apply_network_actions(&pre);
    game.apply_network_actions(&deferred);
    assert_eq!(game.last_self_server_prize_life_seq, Some(1));
    assert_eq!(game.last_self_server_prize_revision, Some(2));
    assert!(!game.network_prize_baseline_pending);
}

#[test]
fn cross_channel_self_ship_transition_survives_soft_snapshot_partition_and_split_poll() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(501),
        arena_stream_id: Some(502),
        player_id: Some(0),
        life_seq: Some(1),
        alive: Some(true),
    };
    let readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(503),
        settings: true,
        security: true,
        map: true,
    };
    let snapshot = || {
        ClientProtocolEvent::SnapshotReceived(SnapshotFrame {
            header: SnapshotHeader {
                snapshot_id: 151,
                tick: 5_000,
                arena_stream_id: 502,
                ..SnapshotHeader::default()
            },
            events: vec![SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                life_seq: 2,
                alive: true,
                ..SnapshotLifecycleState::default()
            })],
            ..SnapshotFrame::default()
        })
    };
    let ship_change = || {
        ClientProtocolEvent::ShipFrequencyChanged(ShipFrequencyChange {
            ship: 6,
            player_id: 0,
            frequency: 0,
            life_seq: Some(2),
        })
    };
    let partition = |events| {
        partition_browser_network_actions_at_segment(
            ClientProtocolActions {
                outbound_packets: Vec::new(),
                events,
                authority_admission: Vec::new(),
            },
            scope.server_session_id,
            scope.arena_stream_id,
            scope.player_id,
            scope.life_seq,
            scope.alive,
            readiness.joined,
            readiness.arena_epoch,
            readiness.settings,
            readiness.security,
            readiness.map,
            17,
            false,
            false,
            true,
        )
    };
    let arm_ship_cutover = |game: &mut Game| {
        assert_eq!(
            game.client_authority
                .queue_lifecycle_intent(PendingLifecycleRequest::Ship(6)),
            ClientAuthorityDisposition::Applied
        );
        assert_eq!(
            game.client_authority.record_ship_request_submitted(6, 91),
            ClientAuthorityDisposition::Applied
        );
    };

    // A snapshot and reliable roster terminal can share one transport drain.
    // Same-segment soft lifecycle facts must retain callback order rather than
    // being grouped like a hard authority replacement.
    let (pre, deferred) = partition(vec![snapshot(), ship_change()]);
    assert!(pre.events.is_empty());
    assert!(matches!(
        deferred.events.as_slice(),
        [
            ClientProtocolEvent::SnapshotReceived(_),
            ClientProtocolEvent::ShipFrequencyChanged(_)
        ]
    ));
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope_at_segment(17, scope, readiness, 503);
    arm_ship_cutover(&mut game);
    game.apply_network_actions(&deferred);
    assert_eq!(game.client_authority.view().scope.life_seq, Some(2));
    assert_eq!(game.client_authority.view().scope.alive, Some(true));
    assert_eq!(game.ship.ship_type, 6);
    assert_eq!(
        game.client_authority.view().lifecycle_intent,
        Some(PendingLifecycleRequest::Ship(6))
    );
    assert!(
        game.client_authority
            .view()
            .lifecycle_baseline_ready_to_submit
    );
    assert_eq!(game.client_authority.view().scope.life_seq, Some(2));

    // A movement/correction barrier can retain separately planned callbacks.
    // Once the first immutable action reaches the exact target life, the
    // independently stamped transition may close that same soft boundary but
    // cannot rebind to a different life.
    let (_, mut snapshot_deferred) = partition(vec![snapshot()]);
    let (_, ship_deferred) = partition(vec![ship_change()]);
    snapshot_deferred.append(ship_deferred);
    let mut split_game = Game::new_network_authority_test();
    split_game.install_network_authority_test_scope_at_segment(17, scope, readiness, 503);
    arm_ship_cutover(&mut split_game);
    split_game.apply_network_actions(&snapshot_deferred);
    assert_eq!(split_game.ship.ship_type, 6);
    assert_eq!(
        split_game.client_authority.view().lifecycle_intent,
        Some(PendingLifecycleRequest::Ship(6))
    );
    assert_eq!(split_game.client_authority.view().scope.life_seq, Some(2));

    let stale_change = ClientProtocolEvent::ShipFrequencyChanged(ShipFrequencyChange {
        ship: 6,
        player_id: 0,
        frequency: 0,
        life_seq: Some(3),
    });
    let (_, mut snapshot_deferred) = partition(vec![snapshot()]);
    let (_, stale_deferred) = partition(vec![stale_change]);
    snapshot_deferred.append(stale_deferred);
    let mut stale_game = Game::new_network_authority_test();
    stale_game.install_network_authority_test_scope_at_segment(17, scope, readiness, 503);
    arm_ship_cutover(&mut stale_game);
    stale_game.apply_network_actions(&snapshot_deferred);
    assert_eq!(stale_game.client_authority.view().scope.life_seq, Some(2));
    assert_eq!(stale_game.ship.ship_type, 0);
    assert_eq!(
        stale_game.client_authority.view().lifecycle_intent,
        Some(PendingLifecycleRequest::Ship(6))
    );
    assert_eq!(
        stale_game
            .client_authority
            .view()
            .lifecycle_baseline_life_seq,
        None
    );
    assert!(
        !stale_game
            .client_authority
            .view()
            .lifecycle_baseline_ready_to_submit
    );
}

#[test]
fn bootstrap_and_self_left_persistently_clear_identity_join_epoch_and_readiness_until_retry() {
    let old_scope = AuthorityScopeIdentity {
        server_session_id: Some(u64::MAX),
        arena_stream_id: Some(u64::MAX),
        player_id: Some(u16::MAX),
        life_seq: Some(u32::MAX),
        alive: Some(true),
    };
    let ready = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(u32::MAX),
        settings: true,
        security: true,
        map: true,
    };

    for (replacement, session, stream) in [
        (
            ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
                server_session_id: 1,
                arena_stream_id: 1,
                server_tick: 7,
                ..ServerClockBootstrap::default()
            }),
            1,
            1,
        ),
        (
            ClientProtocolEvent::PlayerLeft(u16::MAX),
            u64::MAX,
            u64::MAX,
        ),
    ] {
        for stale_after_boundary in [false, true] {
            let stale = ClientProtocolEvent::PlayerPosition(authority_test_position(
                u16::MAX,
                123,
                124,
                1_600,
                1,
            ));
            let events = if stale_after_boundary {
                vec![replacement.clone(), stale]
            } else {
                vec![stale, replacement.clone()]
            };
            let mut game = Game::new_network_authority_test();
            game.install_network_authority_test_scope(old_scope, ready, u32::MAX);
            game.network_zone_files_ready = true;
            game.network_world.players.insert(
                u16::MAX,
                NetworkPlayer::from_roster(&PlayerRosterEntry {
                    ship: 0,
                    audio: 0,
                    name: "old-self".into(),
                    squad: String::new(),
                    kill_points: 0,
                    flag_points: 0,
                    player_id: u16::MAX,
                    frequency: 0,
                    wins: 0,
                    losses: 0,
                    attach_parent: INVALID_PLAYER_ID,
                    flags: 0,
                    koth: 0,
                }),
            );
            game.apply_network_actions(&ClientProtocolActions {
                outbound_packets: Vec::new(),
                events,
                authority_admission: Vec::new(),
            });
            assert_eq!(
                game.client_authority.view().scope.server_session_id,
                Some(session)
            );
            assert_eq!(
                game.client_authority.view().scope.arena_stream_id,
                Some(stream)
            );
            assert_eq!(game.client_authority.view().scope.player_id, None);
            assert!(!game.client_authority.view().readiness.joined);
            assert_eq!(game.network_world.snapshot_epoch(), None);
            assert!(game.network_world.players.is_empty());
            assert!(!game.network_settings_received);
            assert!(!game.network_security_synchronized);
            assert!(!game.network_zone_map_ready);
            assert!(!game.network_zone_files_ready);
            assert!(game.network_prize_baseline_pending);
            assert!(!game.local_gameplay_ready());

            // A split-poll bootstrap/leave without JOIN cannot leak readiness.
            game.apply_network_actions(&ClientProtocolActions::default());
            assert!(!game.local_gameplay_ready());

            let player = 0;
            let epoch = 0;
            let life = 0;
            let retry = ClientProtocolActions {
                outbound_packets: Vec::new(),
                events: vec![
                    ClientProtocolEvent::PlayerIdAssigned(player),
                    ClientProtocolEvent::JoinedArena(epoch),
                    ClientProtocolEvent::ArenaSettingsUpdated(round9_test_settings()),
                    ClientProtocolEvent::SecurityChallengeReceived {
                        challenge: SecurityChallenge::default(),
                        server_session_id: Some(session),
                        arena_epoch: epoch,
                    },
                    ClientProtocolEvent::ZoneMapPublished,
                    ClientProtocolEvent::ZoneFilesPublished,
                    private_prize_snapshot(
                        0,
                        stream,
                        SnapshotPrizeState {
                            life_seq: life,
                            revision: 1,
                            bounty: 77,
                            max_energy: 1_700,
                            rotation_bits: 1.0_f32.to_bits(),
                            thrust_bits: 400.0_f32.to_bits(),
                            top_speed_bits: 500.0_f32.to_bits(),
                            recharge_bits: 60.0_f32.to_bits(),
                            ..SnapshotPrizeState::default()
                        },
                        Some(SnapshotLifecycleState {
                            life_seq: life,
                            alive: true,
                            ..SnapshotLifecycleState::default()
                        }),
                    ),
                    ClientProtocolEvent::PlayerPosition(authority_test_position(
                        player, 8_192, 8_192, 900, 0,
                    )),
                ],
                authority_admission: Vec::new(),
            };
            let (pre, deferred) = partition_browser_network_actions_at_segment(
                retry,
                Some(session),
                Some(stream),
                None,
                None,
                None,
                false,
                None,
                false,
                false,
                false,
                game.client_authority.view().segment,
                false,
                false,
                false,
            );
            game.apply_network_actions(&pre);
            game.apply_network_actions(&deferred);
            assert_eq!(game.client_authority.view().scope.player_id, Some(player));
            assert!(game.client_authority.view().readiness.joined);
            assert_eq!(game.network_world.snapshot_epoch(), Some(epoch));
            assert_eq!(game.last_self_server_prize_revision, Some(1));
            assert!(!game.network_prize_baseline_pending);
            game.door_clock_fresh = true;
            complete_initial_join_sequencer_for_test(&mut game);
            assert!(game.local_gameplay_ready());
        }
    }
}

#[test]
fn replacement_death_presentation_uses_ordered_new_world_and_is_exactly_once() {
    let old_scope = AuthorityScopeIdentity {
        server_session_id: Some(10),
        arena_stream_id: Some(20),
        player_id: Some(1),
        life_seq: Some(30),
        alive: Some(true),
    };
    let ready = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(40),
        settings: true,
        security: true,
        map: true,
    };
    let roster = |player_id, name: &str| PlayerRosterEntry {
        ship: 0,
        audio: 0,
        name: name.into(),
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
    };
    let death = PlayerDeath {
        green_id: PrizeKind::Repel.protocol_id() as i8,
        green_collectible_id: 0xfeed_beef,
        killer_id: 1,
        killed_id: 3,
        bounty: 50,
        flag_transfer: 0,
        life_seq: 31,
        respawn_at_tick: 500,
    };
    let stale_death = ClientProtocolEvent::PlayerDeath(death);
    let mut killed_position = authority_test_position(3, 9_000, 9_100, 700, 0);
    killed_position.velocity_x = 100;

    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(old_scope, ready, 40);
    let actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            stale_death,
            ClientProtocolEvent::Disconnected,
            ClientProtocolEvent::ServerClockBootstrapped(ServerClockBootstrap {
                server_session_id: 10,
                arena_stream_id: 20,
                ..ServerClockBootstrap::default()
            }),
            ClientProtocolEvent::PlayerIdAssigned(1),
            ClientProtocolEvent::JoinedArena(40),
            ClientProtocolEvent::PlayerEntered(roster(1, "self")),
            ClientProtocolEvent::PlayerEntered(roster(3, "victim")),
            ClientProtocolEvent::PlayerPosition(killed_position.clone()),
            ClientProtocolEvent::PlayerDeath(death),
        ],
        authority_admission: Vec::new(),
    };
    game.apply_network_actions(&actions);
    assert_eq!(game.ship_explosions.len(), 1);
    assert_eq!(game.ship_explosions[0].pos, vec2(9_000.0, 9_100.0));
    assert_eq!(game.remote_ship_explosion_count, 1);
    assert_eq!(game.remote_ship_explosion_witnesses.len(), 1);
    assert_eq!(game.remote_ship_explosion_witnesses[0].death.killed_id, 3);
    assert_eq!(game.remote_ship_explosion_witnesses[0].death.life_seq, 31);
    assert_eq!(game.remote_ship_explosion_witnesses[0].ordinal, 1);
    assert_eq!(
        game.remote_ship_explosion_witnesses[0].position,
        vec2(9_000.0, 9_100.0)
    );
    assert_eq!(
        game.last_remote_ship_explosion_position,
        Some(vec2(9_000.0, 9_100.0))
    );
    assert_eq!(game.combat_kill_notification_count, 1);
    assert_eq!(
        game.last_combat_kill_notification.as_deref(),
        Some("victim(50) killed by: self")
    );
    assert_eq!(
        game.prizes
            .iter()
            .filter(|prize| prize.collectible_id == death.green_collectible_id)
            .count(),
        1
    );
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::PlayerDeath(death)],
        authority_admission: Vec::new(),
    });
    assert_eq!(game.ship_explosions.len(), 1);
    assert_eq!(game.remote_ship_explosion_count, 1);
    assert_eq!(game.remote_ship_explosion_witnesses.len(), 1);
    assert_eq!(game.combat_kill_notification_count, 1);
    assert_eq!(
        game.prizes
            .iter()
            .filter(|prize| prize.collectible_id == death.green_collectible_id)
            .count(),
        1
    );

    let second_death = PlayerDeath {
        green_id: 0,
        green_collectible_id: 0,
        killed_id: 4,
        life_seq: 41,
        ..death
    };
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![
            ClientProtocolEvent::PlayerEntered(roster(4, "other victim")),
            ClientProtocolEvent::PlayerPosition(authority_test_position(4, 9_300, 9_350, 700, 0)),
            ClientProtocolEvent::PlayerDeath(second_death),
        ],
        authority_admission: Vec::new(),
    });
    assert_eq!(game.remote_ship_explosion_count, 2);
    assert_eq!(game.remote_ship_explosion_witnesses.len(), 2);
    assert_eq!(game.remote_ship_explosion_witnesses[0].death.killed_id, 3);
    assert_eq!(game.remote_ship_explosion_witnesses[0].death.life_seq, 31);
    assert_eq!(game.remote_ship_explosion_witnesses[1].death.killed_id, 4);
    assert_eq!(game.remote_ship_explosion_witnesses[1].death.life_seq, 41);
    assert_eq!(
        game.remote_ship_explosion_witnesses[0].position,
        vec2(9_000.0, 9_100.0)
    );
    assert_eq!(
        game.last_remote_ship_explosion_position,
        Some(vec2(9_300.0, 9_350.0))
    );
}

#[test]
fn browser_clock_rejects_old_nonce_scope_and_resets_high_tick_before_new_session() {
    let mut clock = BrowserServerClock::default();
    let old = ServerClockBootstrap {
        server_session_id: u64::MAX,
        arena_stream_id: u64::MAX,
        server_tick: 0x7fff_ff00,
        ..ServerClockBootstrap::default()
    };
    assert!(clock.observe_bootstrap(old, u64::MAX));
    let old_nonce = clock.next_sync_nonce(u64::MAX);
    let replacement = ServerClockBootstrap {
        server_session_id: 1,
        arena_stream_id: 1,
        server_tick: 7,
        ..ServerClockBootstrap::default()
    };
    assert!(clock.observe_bootstrap(replacement, 0));
    assert_eq!(clock.current_server_tick(), Some(7));
    assert_eq!(clock.latest_authenticated_server_tick(), Some(7));
    assert!(!clock.observe_time_sync(
        ScopedTimeSyncResponse {
            response: TimeSyncResponse {
                received_timestamp: old_nonce,
                local_timestamp: 0x7fff_ff10,
                server_session_id: Some(u64::MAX),
                arena_stream_id: Some(u64::MAX),
            },
            server_session_id: Some(u64::MAX),
            arena_stream_id: Some(u64::MAX),
        },
        0,
    ));
    assert_eq!(clock.current_server_tick(), Some(7));

    let nonce = clock.next_sync_nonce(0);
    let current = ScopedTimeSyncResponse {
        response: TimeSyncResponse {
            received_timestamp: nonce,
            local_timestamp: 8,
            server_session_id: Some(1),
            arena_stream_id: Some(1),
        },
        server_session_id: Some(1),
        arena_stream_id: Some(1),
    };
    assert!(clock.observe_time_sync(current, 0));
    assert_eq!(clock.current_server_tick(), Some(8));
    assert_eq!(clock.latest_authenticated_server_tick(), Some(8));
    assert!(
        !clock.observe_time_sync(current, 0),
        "duplicate nonce is inert"
    );
    let mut wrapped = BrowserServerClock::default();
    assert!(wrapped.observe_bootstrap(
        ServerClockBootstrap {
            server_session_id: 9,
            arena_stream_id: 10,
            server_tick: 0x7fff_ffff,
            ..ServerClockBootstrap::default()
        },
        11,
    ));
    wrapped.observe_server_tick(0);
    assert_eq!(wrapped.current_server_tick(), Some(0));
    assert_eq!(wrapped.latest_authenticated_server_tick(), Some(0));
}

#[test]
fn browser_clock_advances_continuously_between_bursty_server_observations() {
    let mut clock = BrowserServerClock::default();
    clock.advance_local_monotonic_micros(1_000_000);
    assert!(clock.observe_bootstrap(
        ServerClockBootstrap {
            server_session_id: 7,
            arena_stream_id: 8,
            server_tick: 100,
            ..ServerClockBootstrap::default()
        },
        9,
    ));
    assert_eq!(clock.current_server_tick(), Some(100));
    assert_eq!(clock.latest_authenticated_server_tick(), Some(100));

    clock.advance_local_monotonic_micros(1_006_000);
    assert_eq!(clock.current_server_tick(), Some(100));

    // A 60 Hz snapshot can establish a later exact centisecond tick without
    // discarding the local sub-tick phase. Resetting that phase on every 60 Hz
    // observation systematically slowed a nominal 100 Hz estimate.
    clock.observe_server_tick(104);
    clock.advance_local_monotonic_micros(1_009_000);
    assert_eq!(clock.current_server_tick(), Some(104));
    clock.advance_local_monotonic_micros(1_010_000);
    assert_eq!(clock.current_server_tick(), Some(105));
    assert_eq!(clock.latest_authenticated_server_tick(), Some(104));

    // A backward browser clock sample is inert and resets partial elapsed
    // time rather than creating an unsigned jump.
    clock.advance_local_monotonic_micros(1_000_000);
    assert_eq!(clock.current_server_tick(), Some(105));
    clock.advance_local_monotonic_micros(1_010_000);
    assert_eq!(clock.current_server_tick(), Some(106));
    assert_eq!(clock.latest_authenticated_server_tick(), Some(104));
}

#[test]
fn browser_clock_keeps_authenticated_watermark_behind_a_local_estimate() {
    let mut clock = BrowserServerClock::default();
    clock.advance_local_monotonic_micros(1_000_000);
    assert!(clock.observe_bootstrap(
        ServerClockBootstrap {
            server_session_id: 7,
            arena_stream_id: 8,
            server_tick: 100,
            ..ServerClockBootstrap::default()
        },
        9,
    ));
    clock.advance_local_monotonic_micros(1_100_000);
    assert_eq!(clock.current_server_tick(), Some(110));
    assert_eq!(clock.latest_authenticated_server_tick(), Some(100));

    // A newly delivered authenticated sample can legitimately trail the
    // continuously extrapolated estimate. It refreshes the receipt watermark
    // without rewinding the smooth protocol clock.
    clock.observe_server_tick(105);
    assert_eq!(clock.current_server_tick(), Some(110));
    assert_eq!(clock.latest_authenticated_server_tick(), Some(105));
    clock.observe_server_tick(105);
    assert_eq!(clock.latest_authenticated_server_tick(), Some(105));
}

#[test]
fn login_registration_and_time_sync_are_never_neutral_across_hard_boundary() {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(1),
        arena_stream_id: Some(2),
        player_id: Some(3),
        life_seq: Some(4),
        alive: Some(true),
    };
    let events = vec![
        ClientProtocolEvent::PasswordAccepted,
        ClientProtocolEvent::RegistrationRequired,
        ClientProtocolEvent::RegistrationFormRequested,
        ClientProtocolEvent::LoginFailed(1),
        ClientProtocolEvent::TimeSyncReceived(ScopedTimeSyncResponse {
            response: TimeSyncResponse {
                received_timestamp: 5,
                local_timestamp: 6,
                server_session_id: Some(1),
                arena_stream_id: Some(2),
            },
            server_session_id: Some(1),
            arena_stream_id: Some(2),
        }),
        ClientProtocolEvent::Disconnected,
    ];
    let actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events,
        authority_admission: Vec::new(),
    };
    let plan = build_authority_admission_plan(
        &actions,
        AuthorityAdmissionSeed {
            scope,
            readiness: AuthorityReadiness::default(),
            segment: u64::MAX,
        },
    );
    assert!(
        plan[..5]
            .iter()
            .all(|admission| admission.action == AuthorityActionAdmission::Reject)
    );
    assert!(
        plan[..5]
            .iter()
            .all(|admission| admission.action != AuthorityActionAdmission::Neutral)
    );
}

#[test]
fn an_authority_boundary_resets_movement_tuning_instead_of_immobilising_the_ship() {
    let mut game = Game::new_network_authority_test();
    // What a collected prize leaves behind: tuning well above the arena base.
    game.ship.top_speed = 900.0;
    game.ship.thrust_acceleration = 900.0;
    game.ship.rotation_per_second = 9.0;
    game.ship.recharge_per_second = 900.0;
    game.ship.energy = 1_500.0;
    game.ship.gun_level = 4;
    game.ship.multifire_capable = true;

    game.invalidate_authoritative_prize_runtime();

    // Movement survives the prize-baseline wait. `ship.top_speed` is
    // `effective_top_speed()` verbatim and feeds the physics clamp, so the old zero
    // assignment froze the ship for the whole window whatever the frame gate said -
    // which is why dropping the gate alone would have changed nothing. The reset target
    // is the arena base tuning, the same source `reset_ship` uses, and the first
    // authoritative snapshot overwrites it from server prize bits.
    let tuning = game.ship_tuning();
    assert_eq!(game.ship.top_speed, tuning.top_speed);
    assert_eq!(game.ship.thrust_acceleration, tuning.thrust_acceleration);
    assert_eq!(game.ship.rotation_per_second, tuning.rotation_per_second);
    assert_eq!(game.ship.recharge_per_second, tuning.energy_recharge);

    // Everything the server has to re-grant is still stripped. The ship may fly; it may
    // not fire, collect, cloak or spend on the strength of a previous life.
    assert_eq!(game.ship.energy, 0.0);
    assert_eq!(game.ship.max_energy, 0.0);
    assert_eq!(game.ship.gun_level, 0);
    assert!(!game.ship.multifire_capable);
    assert!(game.network_prize_baseline_pending);
    assert!(game.last_self_server_prize_revision.is_none());
}

#[test]
fn energy_rejection_rolls_back_optimistic_spectator_without_resetting_the_live_ship() {
    let mut game = Game::new_network_authority_test();
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(23),
        life_seq: Some(41),
        alive: Some(true),
    };
    game.client_authority.install_test_scope(
        1,
        scope,
        ClientAuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
            files: true,
        },
    );
    game.ship.ship_type = 0;
    game.ship.energy = 800.0;
    let position = game.ship.pos;
    game.spectator_mode = true;
    game.client_authority
        .queue_lifecycle_intent(PendingLifecycleRequest::Ship(8));
    game.client_authority
        .record_ship_request_submitted(8, u16::MAX);
    let receipt = ShipChangeRejected {
        ship: 8,
        server_session_id: 71,
        arena_stream_id: 81,
        player_id: 23,
        life_seq: 41,
        position_sequence: u16::MAX,
    };
    game.apply_network_actions(&ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::ShipChangeRejected(receipt)],
        authority_admission: vec![AuthorityEventAdmission {
            segment: 1,
            action: AuthorityActionAdmission::Bound(scope),
            ..AuthorityEventAdmission::default()
        }],
    });
    assert_eq!(
        game.client_authority.view().phase,
        ClientAuthorityPhase::Live
    );
    assert_eq!(game.client_authority.view().scope, scope);
    assert_eq!(game.client_authority.view().lifecycle_intent, None);
    assert!(!game.spectator_mode);
    assert_eq!(game.ship.ship_type, 0);
    assert_eq!(game.ship.energy, 800.0);
    assert_eq!(game.ship.pos, position);
}

#[test]
fn spawn_warp_terminal_does_not_consume_an_unused_portal() {
    let (mut game, request) = portal_warp_reconciliation_game();
    let original = game.portal.as_ref().unwrap().pos;
    let endpoint = game.portal_endpoint_authority;
    game.apply_network_actions(&portal_warp_terminal(
        request,
        PortalWarpAccepted::OUTCOME_APPLIED,
        false,
    ));
    assert_eq!(game.portal.as_ref().unwrap().pos, original);
    assert_eq!(game.portal_endpoint_authority, endpoint);
    assert!(!game.client_authority.view().portal_warp_barrier);
}

#[test]
fn spawn_idle_spectator_notice_is_scoped_idempotent_and_has_no_quality_hold() {
    let mut game = Game::new_network_authority_test();
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(23),
        life_seq: Some(41),
        alive: Some(true),
    };
    let ready = ClientAuthorityReadiness {
        joined: true,
        arena_epoch: Some(81),
        settings: true,
        security: true,
        map: true,
        files: true,
    };
    game.client_authority.install_test_scope(1, scope, ready);
    game.ship.vel = vec2(10.0, 5.0);
    game.player_respawn_timer = 3.0;
    let fact = crate::protocol::SpawnIdleSpectator {
        server_session_id: 71,
        arena_stream_id: 81,
        player_id: 23,
        predecessor_life_seq: 41,
        life_seq: 42,
    };
    let apply = |game: &mut Game, fact| {
        let scope = game.client_authority.view().scope;
        game.apply_network_actions(&ClientProtocolActions {
            outbound_packets: vec![],
            events: vec![ClientProtocolEvent::SpawnIdleSpectator(fact)],
            authority_admission: vec![AuthorityEventAdmission {
                segment: 1,
                action: AuthorityActionAdmission::Bound(scope),
                ..AuthorityEventAdmission::default()
            }],
        });
    };
    for wrong in [
        crate::protocol::SpawnIdleSpectator {
            arena_stream_id: 82,
            ..fact
        },
        crate::protocol::SpawnIdleSpectator {
            server_session_id: 72,
            ..fact
        },
        crate::protocol::SpawnIdleSpectator {
            player_id: 24,
            ..fact
        },
        crate::protocol::SpawnIdleSpectator {
            predecessor_life_seq: 40,
            life_seq: 41,
            ..fact
        },
    ] {
        apply(&mut game, wrong);
        assert_eq!(
            game.client_authority.view().phase,
            ClientAuthorityPhase::Live
        );
    }
    apply(&mut game, fact);
    assert!(game.spectator_mode);
    assert_eq!(game.ship.vel, Vec2::ZERO);
    assert_eq!(game.player_respawn_timer, 0.0);
    assert_eq!(
        game.client_authority.view().phase,
        ClientAuthorityPhase::Spectating
    );
    assert!(
        game.client_authority
            .quality_hold_remaining(100.0)
            .is_none()
    );
    assert_eq!(game.notifications.len(), 1);
    game.spectating_player_id = Some(55);
    apply(&mut game, fact);
    assert_eq!(game.spectating_player_id, Some(55));
    assert_eq!(game.notifications.len(), 1);
    game.client_authority.install_test_scope(
        1,
        AuthorityScopeIdentity {
            life_seq: Some(43),
            ..scope
        },
        ready,
    );
    game.spectator_mode = false;
    apply(&mut game, fact);
    assert!(
        !game.spectator_mode,
        "late idle fact must not retire a returned player life"
    );
}

#[test]
fn forced_quality_spectator_notice_preserves_camera_on_duplicate_and_status_updates() {
    let mut game = Game::new_network_authority_test();
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(23),
        life_seq: Some(41),
        alive: Some(true),
    };
    game.client_authority.install_test_scope(
        1,
        scope,
        ClientAuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
            files: true,
        },
    );
    game.ship.energy = 1.0;
    game.ship.vel = vec2(10.0, 5.0);
    game.player_respawn_timer = 3.0;
    let fact = crate::protocol::ConnectionQualityDirective {
        kind: crate::protocol::ConnectionQualityDirectiveKind::Force,
        server_session_id: 71,
        arena_stream_id: 81,
        player_id: 23,
        predecessor_life_seq: 41,
        life_seq: 42,
        hold_id: 1,
        remaining_ms: 60_000,
        rejected_request: None,
    };
    let apply = |game: &mut Game, fact| {
        let bound = game.client_authority.view().scope;
        game.apply_network_actions(&ClientProtocolActions {
            outbound_packets: vec![],
            events: vec![ClientProtocolEvent::ConnectionQualityDirective(fact)],
            authority_admission: vec![AuthorityEventAdmission {
                segment: 1,
                action: AuthorityActionAdmission::Bound(bound),
                ..AuthorityEventAdmission::default()
            }],
        });
    };
    apply(&mut game, fact);
    assert!(game.spectator_mode);
    assert_eq!(
        game.client_authority.view().phase,
        ClientAuthorityPhase::Spectating
    );
    assert_eq!(game.ship.vel, Vec2::ZERO);
    assert_eq!(game.player_respawn_timer, 0.0);
    assert_eq!(game.notifications.len(), 1);
    assert!(game.notifications[0].message.contains("for one minute"));
    game.spectating_player_id = Some(55);
    let position = game.ship.pos;
    apply(&mut game, fact);
    apply(
        &mut game,
        crate::protocol::ConnectionQualityDirective {
            kind: crate::protocol::ConnectionQualityDirectiveKind::Hold,
            predecessor_life_seq: 42,
            remaining_ms: 118_000,
            ..fact
        },
    );
    assert_eq!(game.spectating_player_id, Some(55));
    assert_eq!(game.ship.pos, position);
    assert_eq!(
        game.notifications.len(),
        1,
        "status/replay must not repeat the forced notification"
    );
}

#[test]
fn quality_return_recovery_requires_the_exact_expired_ship_denial_and_is_not_restarted() {
    use crate::protocol::{ConnectionQualityDirective, ConnectionQualityDirectiveKind as Kind};
    for remaining_ms in [0, 30_000] {
        let mut game = Game::new_network_authority_test();
        game.client_authority.install_test_scope(
            1,
            AuthorityScopeIdentity {
                server_session_id: Some(71),
                arena_stream_id: Some(81),
                player_id: Some(23),
                life_seq: Some(41),
                alive: Some(true),
            },
            ClientAuthorityReadiness {
                joined: true,
                arena_epoch: Some(81),
                settings: true,
                security: true,
                map: true,
                files: true,
            },
        );
        let apply = |game: &mut Game, fact| {
            game.apply_network_actions(&ClientProtocolActions {
                events: vec![ClientProtocolEvent::ConnectionQualityDirective(fact)],
                authority_admission: vec![AuthorityEventAdmission {
                    segment: 1,
                    action: AuthorityActionAdmission::Bound(game.client_authority.view().scope),
                    ..AuthorityEventAdmission::default()
                }],
                ..ClientProtocolActions::default()
            });
        };
        let force = ConnectionQualityDirective {
            kind: Kind::Force,
            server_session_id: 71,
            arena_stream_id: 81,
            player_id: 23,
            predecessor_life_seq: 41,
            life_seq: 42,
            hold_id: 1,
            remaining_ms: 60_000,
            rejected_request: None,
        };
        apply(&mut game, force);
        assert!(game.quality_return_recovery.is_none());
        game.client_authority
            .queue_lifecycle_intent(PendingLifecycleRequest::Ship(5));
        game.client_authority.record_ship_request_submitted(5, 17);
        let denial = ConnectionQualityDirective {
            kind: Kind::Hold,
            predecessor_life_seq: 42,
            remaining_ms,
            rejected_request: Some((5, 17)),
            ..force
        };
        for invalid in [
            ConnectionQualityDirective {
                rejected_request: Some((5, 18)),
                ..denial
            },
            ConnectionQualityDirective {
                arena_stream_id: 82,
                ..denial
            },
        ] {
            apply(&mut game, invalid);
            assert!(game.quality_return_recovery.is_none());
            assert!(game.client_authority.view().transition_pending);
        }
        apply(&mut game, denial);
        assert!(!game.client_authority.view().transition_pending);
        assert_eq!(
            game.client_authority.view().phase,
            ClientAuthorityPhase::Spectating
        );
        assert_eq!(game.quality_return_recovery.is_some(), remaining_ms == 0);
        if remaining_ms == 0 {
            // A fresh menu selection on the recovered observer can still be
            // denied by server quality checks. Explain that without replacing
            // the connection or arming an automatic playable-ship retry.
            let now = macroquad::miniquad::date::now();
            let attempt = game.quality_return_recovery.as_mut().unwrap();
            assert_eq!(
                attempt.poll(now, false, false, false),
                quality_return::RecoveryAction::Reconnect
            );
            // In scope, but the server has not measured the fresh connection.
            assert_eq!(
                attempt.poll(now, false, true, false),
                quality_return::RecoveryAction::Wait
            );
            // A selection during that measurement is explained the same way,
            // without restarting recovery.
            let settling = game.quality_return_recovery;
            game.client_authority
                .queue_lifecycle_intent(PendingLifecycleRequest::Ship(5));
            game.client_authority.record_ship_request_submitted(5, 18);
            apply(
                &mut game,
                ConnectionQualityDirective {
                    rejected_request: Some((5, 18)),
                    ..denial
                },
            );
            assert_eq!(game.quality_return_recovery, settling);
            assert!(
                game.notifications
                    .last()
                    .unwrap()
                    .message
                    .contains("still being checked")
            );
            let attempt = game.quality_return_recovery.as_mut().unwrap();
            assert_eq!(
                attempt.poll(now + quality_return::FRESH_MEASUREMENT, false, true, false),
                quality_return::RecoveryAction::Ready
            );
            let ready = game.quality_return_recovery;
            game.client_authority
                .queue_lifecycle_intent(PendingLifecycleRequest::Ship(5));
            game.client_authority.record_ship_request_submitted(5, 19);
            apply(
                &mut game,
                ConnectionQualityDirective {
                    rejected_request: Some((5, 19)),
                    ..denial
                },
            );
            assert_eq!(game.quality_return_recovery, ready);
            assert!(
                game.notifications
                    .last()
                    .unwrap()
                    .message
                    .contains("still being checked")
            );
            assert_eq!(
                game.client_authority.view().phase,
                ClientAuthorityPhase::Spectating
            );
            assert!(!game.client_authority.view().transition_pending);
        }
        if let Some(attempt) = game.quality_return_recovery.as_mut() {
            assert_eq!(attempt.ship, 5);
            attempt.stop();
        }
        let stopped = game.quality_return_recovery;
        // A delayed terminal cannot erase the stopped latch and reconnect again.
        apply(&mut game, denial);
        assert_eq!(game.quality_return_recovery, stopped);
        assert!(game.client_authority.view().gameplay_scope().is_none());
    }
}

#[test]
fn quality_return_clear_ack_and_private_snapshot_keep_baseline_submission_ready() {
    use crate::protocol::{ConnectionQualityDirective, ConnectionQualityDirectiveKind as Kind};
    for order in [[0, 1, 2], [0, 2, 1], [2, 0, 1]] {
        for split in [false, true] {
            let mut game = Game::new_network_authority_test();
            let scope = AuthorityScopeIdentity {
                server_session_id: Some(71),
                arena_stream_id: Some(81),
                player_id: Some(23),
                life_seq: None,
                alive: None,
            };
            game.install_network_authority_test_scope(
                scope,
                AuthorityReadiness {
                    joined: true,
                    arena_epoch: Some(81),
                    settings: true,
                    security: true,
                    map: true,
                },
                81,
            );
            game.network_zone_files_ready = true;
            game.network_prize_baseline_pending = true;
            game.client_authority.observe_initial_ship(8);
            let initial_hold = ConnectionQualityDirective {
                kind: Kind::Hold,
                server_session_id: 71,
                arena_stream_id: 81,
                player_id: 23,
                predecessor_life_seq: 42,
                life_seq: 42,
                hold_id: 1,
                remaining_ms: 0,
                rejected_request: None,
            };
            game.apply_network_actions(&ClientProtocolActions {
                outbound_packets: vec![],
                events: vec![ClientProtocolEvent::ConnectionQualityDirective(
                    initial_hold,
                )],
                authority_admission: vec![],
            });
            assert!(game.client_authority.view().observer_scope().is_some());
            assert!(!game.client_authority.view().neutral_ship_baseline_ready());
            assert_eq!(
                game.client_authority
                    .queue_lifecycle_intent(PendingLifecycleRequest::Ship(0)),
                ClientAuthorityDisposition::Applied
            );
            assert_eq!(
                game.client_authority.record_ship_request_submitted(0, 80),
                ClientAuthorityDisposition::Applied
            );
            let events = [
                ClientProtocolEvent::ConnectionQualityDirective(ConnectionQualityDirective {
                    kind: Kind::Clear,
                    predecessor_life_seq: 42,
                    life_seq: 42,
                    ..initial_hold
                }),
                ClientProtocolEvent::ShipFrequencyChanged(crate::protocol::ShipFrequencyChange {
                    ship: 0,
                    player_id: 23,
                    frequency: 0,
                    life_seq: Some(43),
                }),
                private_prize_snapshot(
                    1,
                    81,
                    SnapshotPrizeState {
                        life_seq: 43,
                        revision: 1,
                        max_energy: 1700,
                        energy: Some(1700),
                        bounty: 30,
                        rotation_bits: 1.0_f32.to_bits(),
                        thrust_bits: 400.0_f32.to_bits(),
                        top_speed_bits: 500.0_f32.to_bits(),
                        recharge_bits: 60.0_f32.to_bits(),
                        ..SnapshotPrizeState::default()
                    },
                    Some(SnapshotLifecycleState {
                        life_seq: 43,
                        alive: true,
                        ..SnapshotLifecycleState::default()
                    }),
                ),
            ];
            let ordered = order
                .into_iter()
                .map(|index| events[index].clone())
                .collect::<Vec<_>>();
            let batches = if split {
                ordered
                    .into_iter()
                    .map(|event| vec![event])
                    .collect::<Vec<_>>()
            } else {
                vec![ordered]
            };
            for events in batches {
                let view = game.client_authority.view();
                let (pre, deferred) = partition_browser_network_actions_at_segment(
                    ClientProtocolActions {
                        events,
                        outbound_packets: vec![],
                        authority_admission: vec![],
                    },
                    view.scope.server_session_id,
                    view.scope.arena_stream_id,
                    view.scope.player_id,
                    view.scope.life_seq,
                    view.scope.alive,
                    view.readiness.joined,
                    view.readiness.arena_epoch,
                    view.readiness.settings,
                    view.readiness.security,
                    view.readiness.map,
                    view.segment,
                    false,
                    false,
                    game.prize_reconciliation_cutover_pending(),
                );
                game.apply_network_actions(&pre);
                game.apply_network_actions(&deferred);
            }
            assert!(
                game.client_authority
                    .view()
                    .lifecycle_baseline_ready_to_submit,
                "{order:?}, split={split}, {:?}",
                game.client_authority.view()
            );
            // An early private carrier may precede the reliable ACK and be
            // rejected. The next ordinary snapshot for the confirmed successor
            // supplies its loadout before the neutral position is emitted.
            let mut follow = events[2].clone();
            if let ClientProtocolEvent::SnapshotReceived(frame) = &mut follow {
                frame.header.snapshot_id = 2;
            }
            game.apply_network_actions(&ClientProtocolActions {
                events: vec![follow],
                outbound_packets: vec![],
                authority_admission: vec![],
            });
            assert!(
                game.current_life_loadout_baseline_ready(),
                "{order:?}, split={split}"
            );
            assert!(
                !game.network_prize_baseline_pending,
                "{order:?}, split={split}"
            );
            assert!(game.client_authority.view().neutral_ship_baseline_ready());
            assert!(game.client_authority.view().gameplay_scope().is_none());
            assert!(!game.local_gameplay_ready());
            assert!(
                game.client_authority
                    .quality_hold_remaining(100.0)
                    .is_none(),
                "{order:?}, split={split}"
            );
            assert!(!game.spectator_mode);
            assert_eq!(
                game.client_authority
                    .record_lifecycle_baseline_submitted(100, 80, None),
                ClientAuthorityDisposition::Applied
            );
            game.client_authority
                .observe_lifecycle_receipt(LifecycleBaselineAccepted {
                    client_reliable_delivery_id: 101,
                    life_seq: 43,
                    position_sequence: 80,
                    accepted_server_tick: 500,
                    accepted_position: None,
                });
            assert!(game.client_authority.view().gameplay_scope().is_none());
            game.client_authority
                .observe_lifecycle_receipt(LifecycleBaselineAccepted {
                    client_reliable_delivery_id: 100,
                    life_seq: 43,
                    position_sequence: 80,
                    accepted_server_tick: 500,
                    accepted_position: None,
                });
            assert!(
                game.local_gameplay_ready(),
                "the exact receipt plus its already-installed loadout release gameplay"
            );
            assert!(
                !game.network_prize_baseline_pending,
                "{order:?}, split={split}"
            );
            assert!(game.client_authority.view().gameplay_scope().is_some());
        }
    }
}

#[test]
fn brick_overlap_receipt_moves_and_animates_only_after_server_commit_once() {
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        AuthorityScopeIdentity {
            server_session_id: Some(77),
            arena_stream_id: Some(88),
            player_id: Some(9),
            life_seq: Some(3),
            alive: Some(true),
        },
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(88),
            settings: true,
            security: true,
            map: true,
        },
        88,
    );
    game.ship.pos = vec2(8000.0, 8100.0);
    let from = game.ship.pos;
    let proposal = MovementCorrection {
        reason: MovementCorrection::REASON_BRICK_OVERLAP,
        repel_window: None,
        server_session_id: 77,
        player_id: 9,
        life_seq: 3,
        correction_generation: 1,
        position_sequence: u16::MAX,
        server_tick: 100,
        position_x: 7000.0,
        position_y: 7100.0,
        velocity_x: 0.0,
        velocity_y: 0.0,
        orientation: 0.0,
    };
    game.client_authority
        .observe_movement_correction(proposal, 1, false);
    let application = game
        .client_authority
        .correction_application_to_submit()
        .unwrap();
    game.client_authority
        .record_correction_application_submitted(42);
    assert_eq!(game.ship.pos, from);
    assert!(game.warps.is_empty());
    let receipt = MovementCorrectionAccepted {
        outcome: MovementCorrectionAccepted::OUTCOME_APPLIED,
        server_tick: 101,
        server_session_id: 77,
        player_id: 9,
        life_seq: 3,
        correction_generation: 1,
        detected_sequence: proposal.position_sequence,
        application_sequence: application.application_sequence,
        position_x: 7000.0,
        position_y: 7100.0,
        velocity_x: 0.0,
        velocity_y: 0.0,
        orientation: 0.0,
    };
    let actions = ClientProtocolActions {
        outbound_packets: Vec::new(),
        events: vec![ClientProtocolEvent::MovementCorrectionAccepted(receipt)],
        authority_admission: Vec::new(),
    };
    game.apply_network_actions(&actions);
    assert_eq!(game.ship.pos, vec2(7000.0, 7100.0));
    assert!(game.ship.flash);
    assert_eq!(game.warps.len(), 2);
    assert_eq!(game.warps[0].pos, from);
    assert_eq!(game.warps[1].pos, game.ship.pos);
    game.apply_network_actions(&actions);
    assert_eq!(
        game.warps.len(),
        2,
        "a retried terminal must not replay the warp effect"
    );
}

#[test]
fn confirmed_ship_change_installs_exact_successor_energy_before_neutral_baseline() {
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        AuthorityScopeIdentity {
            server_session_id: Some(71),
            arena_stream_id: Some(81),
            player_id: Some(23),
            life_seq: Some(41),
            alive: Some(true),
        },
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
        },
        81,
    );
    game.network_zone_files_ready = true;
    game.last_self_server_prize_life_seq = Some(41);
    game.last_self_server_prize_revision = Some(1);
    game.last_self_server_energy = Some(2800.0);
    assert_eq!(
        game.client_authority
            .queue_lifecycle_intent(PendingLifecycleRequest::Ship(1)),
        ClientAuthorityDisposition::Applied
    );
    assert_eq!(
        game.client_authority.record_ship_request_submitted(1, 80),
        ClientAuthorityDisposition::Applied
    );
    let snapshot = |life, revision, energy| {
        private_prize_snapshot(
            revision as u16,
            81,
            SnapshotPrizeState {
                life_seq: life,
                revision,
                bounty: 3,
                max_energy: energy,
                energy: Some(energy),
                rotation_bits: 1.0_f32.to_bits(),
                thrust_bits: 190.0_f32.to_bits(),
                top_speed_bits: 350.0_f32.to_bits(),
                recharge_bits: 115.0_f32.to_bits(),
                ..SnapshotPrizeState::default()
            },
            Some(SnapshotLifecycleState {
                life_seq: life,
                alive: true,
                ..SnapshotLifecycleState::default()
            }),
        )
    };
    // A request alone cannot authorize a future-life loadout.
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![snapshot(42, 2, 2000)],
        ..Default::default()
    });
    assert_eq!(game.last_self_server_prize_life_seq, Some(41));
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![ClientProtocolEvent::ShipFrequencyChanged(
            crate::protocol::ShipFrequencyChange {
                player_id: 23,
                ship: 1,
                frequency: 23,
                life_seq: Some(42),
            },
        )],
        ..Default::default()
    });
    assert!(game.client_authority.view().neutral_ship_baseline_ready());
    // The previous life cannot restore its larger energy across the ACK.
    game.apply_network_actions(&ClientProtocolActions {
        events: vec![snapshot(41, 3, 2800)],
        ..Default::default()
    });
    assert_eq!(game.last_self_server_prize_life_seq, Some(41));
    assert!(!game.current_life_loadout_baseline_ready());
    let authority = game.client_authority.view();
    let (pre, deferred) = partition_browser_network_actions_at_segment(
        ClientProtocolActions {
            events: vec![snapshot(42, 4, 2000)],
            ..Default::default()
        },
        authority.scope.server_session_id,
        authority.scope.arena_stream_id,
        authority.scope.player_id,
        authority.scope.life_seq,
        authority.scope.alive,
        authority.readiness.joined,
        authority.readiness.arena_epoch,
        authority.readiness.settings,
        authority.readiness.security,
        authority.readiness.map,
        authority.segment,
        false,
        false,
        game.prize_reconciliation_cutover_pending(),
    );
    assert!(pre.events.iter().any(|event| matches!(event,
        ClientProtocolEvent::PreGameplayPrizeState { state, .. } if state.life_seq == 42)));
    game.apply_network_actions(&pre);
    game.apply_network_actions(&deferred);
    assert!(game.current_life_loadout_baseline_ready());
    assert_eq!(game.last_self_server_prize_life_seq, Some(42));
    assert_eq!(game.ship.energy, 2000.0);
    assert_eq!(game.ship.max_energy, 2000.0);
    assert_eq!(game.last_self_server_energy, Some(2000.0));
    assert!(game.client_authority.view().neutral_ship_baseline_ready());
    assert_ne!(
        game.client_authority.view().phase,
        ClientAuthorityPhase::Live,
        "a fresh loadout permits only the neutral transaction, never gameplay before its receipt"
    );
}

#[test]
fn respawn_energy_uses_explicit_server_spawn_policy_instead_of_earned_max_energy() {
    for maxed_policy in [false, true] {
        let mut game = Game::new_network_authority_test();
        game.install_network_authority_test_scope(
            AuthorityScopeIdentity {
                server_session_id: Some(71),
                arena_stream_id: Some(81),
                player_id: Some(23),
                life_seq: Some(41),
                alive: Some(true),
            },
            AuthorityReadiness {
                joined: true,
                arena_epoch: Some(81),
                settings: true,
                security: true,
                map: true,
            },
            81,
        );
        game.network_zone_files_ready = true;
        // Javelin's distinct Extreme energy values, with bounty grants excluded
        // only to isolate the reset policy from random initial prize selection.
        game.tuning.ships[0].energy_initial = 1200.0;
        game.tuning.ships[0].energy_max = 2000.0;
        game.tuning.ships[0].initial_bounty = 0;
        game.apply_network_actions(&ClientProtocolActions {
            events: vec![private_prize_snapshot(
                1,
                81,
                SnapshotPrizeState {
                    life_seq: 41,
                    revision: 1,
                    flags: if maxed_policy { 1 << 14 } else { 0 },
                    max_energy: 2000,
                    energy: Some(2000),
                    ..SnapshotPrizeState::default()
                },
                Some(SnapshotLifecycleState {
                    life_seq: 41,
                    alive: true,
                    ..SnapshotLifecycleState::default()
                }),
            )],
            ..Default::default()
        });
        assert_eq!(game.ship.energy, 2000.0);
        game.reset_ship_after_death();
        assert_eq!(
            game.ship.energy,
            if maxed_policy { 2000.0 } else { 1200.0 },
            "earned maximum energy is not evidence of the server's entry policy"
        );
        // Hard authority replacement must not inherit the previous policy.
        game.invalidate_authoritative_prize_runtime();
        game.reset_ship_after_death();
        assert_eq!(game.ship.energy, 1200.0);
    }
}

fn bounty_authority_test_game() -> Game {
    bounty_authority_test_game_with_alive(true)
}

#[test]
fn local_death_notice_uses_player_handle_and_keeps_duplicate_deaths_silent() {
    for known_killer in [false, true] {
        let mut game = bounty_authority_test_game();
        game.local_player_name = "Stardust".into();
        if known_killer {
            game.network_world
                .apply_event(&ClientProtocolEvent::PlayerEntered(PlayerRosterEntry {
                    ship: 0,
                    audio: 0,
                    name: "Mad Hatter".into(),
                    squad: String::new(),
                    kill_points: 0,
                    flag_points: 0,
                    player_id: 13,
                    frequency: 13,
                    wins: 0,
                    losses: 0,
                    attach_parent: INVALID_PLAYER_ID,
                    flags: 0,
                    koth: 0,
                }));
        }
        let notice = PlayerDeath {
            life_seq: 6,
            ..death(13, 7, 30)
        };
        game.apply_authoritative_player_death(notice);
        let expected = if known_killer {
            "Stardust(30) killed by: Mad Hatter"
        } else {
            "Stardust(30) destroyed."
        };
        assert!(
            game.notifications
                .iter()
                .any(|entry| entry.message == expected)
        );
        let count = game.notifications.len();
        game.apply_authoritative_player_death(notice);
        assert_eq!(game.notifications.len(), count);
    }
}

fn bounty_authority_test_game_with_alive(alive: bool) -> Game {
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(71),
        arena_stream_id: Some(81),
        player_id: Some(7),
        life_seq: Some(5),
        alive: Some(alive),
    };
    let mut game = Game::new_network_authority_test();
    game.install_network_authority_test_scope(
        scope,
        AuthorityReadiness {
            joined: true,
            arena_epoch: Some(81),
            settings: true,
            security: true,
            map: true,
        },
        81,
    );
    game.network_prize_baseline_pending = false;
    game.last_self_server_prize_life_seq = Some(5);
    game.last_self_server_prize_revision = Some(7);
    game.player_bounty = 30;
    game.tuning.bounty_increase_for_kill = 1;
    game.ship.energy = 1700.0;
    game.ship.max_energy = 1700.0;
    game
}

#[test]
fn late_kill_notice_cannot_corrupt_successor_neutral_baseline_bounty() {
    for alive in [false, true] {
        let mut game = bounty_authority_test_game_with_alive(alive);
        assert_eq!(game.client_authority.view().scope.alive, Some(alive));
        let sequence = 27181;
        let neutral = || ClientInputRecordPacket {
            base_seq: sequence,
            inputs: vec![crate::protocol::PlayerInputRecord {
                combat_view_receipt: crate::combat_timeline::CombatViewReceiptV1::unavailable(
                    sequence,
                ),
                ..Default::default()
            }],
        };
        let before = game
            .client_position_update(sequence, 0, neutral(), None)
            .encode();
        // The global death carrier has no killer-life identity. This same notice
        // can describe a retained life-4 projectile while the pilot is now life 5.
        // Cumulative score comes from the independent server ScoreUpdate; the
        // notice may show the kill, but cannot infer any life-5 bounty benefit.
        game.apply_authoritative_player_death(death(7, 9, 42));
        let after = game
            .client_position_update(sequence, 0, neutral(), None)
            .encode();
        let decoded = crate::continuum_position::parse_client_position_snapshot(&after)
            .expect("the actual outbound neutral carrier must decode");
        assert_eq!(
            decoded.bounty, 30,
            "late kill cannot change the encoded successor baseline bounty"
        );
        assert_eq!(
            game.player_bounty, 30,
            "old-source credit cannot alter successor bounty"
        );
        assert_eq!(
            after, before,
            "the exact respawn position carrier must stay canonical"
        );
        assert_eq!(
            game.combat_kill_notification_count, 1,
            "retain the kill notification"
        );
        assert_eq!(
            game.client_authority.view().scope.alive,
            Some(alive),
            "another player death cannot change our lifecycle"
        );
    }
}

#[test]
fn private_bounty_snapshot_and_global_death_do_not_double_award_in_either_order() {
    for death_first in [false, true] {
        let mut game = bounty_authority_test_game();
        let prize = SnapshotPrizeState {
            life_seq: 5,
            // Kill bounty is repeated with the existing private-state
            // revision; only forward snapshot frames reach this path.
            revision: 7,
            bounty: 31,
            max_energy: 1700,
            energy: Some(1700),
            ..SnapshotPrizeState::default()
        };
        if death_first {
            game.apply_authoritative_player_death(death(7, 9, 42));
        }
        assert!(game.apply_authoritative_prize_state(prize, 5491));
        if !death_first {
            game.apply_authoritative_player_death(death(7, 9, 42));
        }
        assert_eq!(
            game.player_bounty, 31,
            "private authority owns the exact award in either order"
        );
        assert_eq!(game.combat_kill_notification_count, 1);
        assert!(game.apply_authoritative_prize_state(prize, 5492));
        assert_eq!(
            game.player_bounty, 31,
            "a repeated private frame cannot award twice"
        );
        assert!(!game.apply_authoritative_prize_state(
            SnapshotPrizeState {
                revision: 6,
                bounty: 99,
                ..prize
            },
            5490
        ));
        assert_eq!(
            game.player_bounty, 31,
            "an older private revision cannot rewind the award"
        );
    }
}

#[test]
fn quality_warnings_explain_the_cause_only_to_the_current_scope_in_a_ship() {
    let mut game = Game::new_network_authority_test();
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(10),
        arena_stream_id: Some(20),
        player_id: Some(1),
        life_seq: None,
        alive: None,
    };
    let readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(40),
        settings: true,
        security: true,
        map: true,
    };
    game.install_network_authority_test_scope(scope, readiness, 40);
    game.spectator_mode = false;
    let warning = crate::protocol::ConnectionQualityWarning {
        server_session_id: 10,
        arena_stream_id: 20,
        reason: crate::protocol::ConnectionQualityWarningReason::ShotsRefused,
        rtt_ms: 311,
        loss_per_mille: 0,
    };
    let apply = |game: &mut Game, warning| {
        game.apply_network_actions(&ClientProtocolActions {
            events: vec![ClientProtocolEvent::ConnectionQualityWarning(warning)],
            outbound_packets: Vec::new(),
            authority_admission: Vec::new(),
        });
    };

    for stale in [
        crate::protocol::ConnectionQualityWarning {
            server_session_id: 11,
            ..warning
        },
        crate::protocol::ConnectionQualityWarning {
            arena_stream_id: 21,
            ..warning
        },
    ] {
        apply(&mut game, stale);
    }
    assert!(game.notifications.is_empty(), "another session or arena");
    game.spectator_mode = true;
    apply(&mut game, warning);
    assert!(game.notifications.is_empty(), "a spectator fires no shots");
    game.spectator_mode = false;
    apply(&mut game, warning);
    assert_eq!(game.notifications.len(), 1);
    assert_eq!(
        game.notifications[0].message,
        "Your shots aren't counting because your connection is too slow (311 ms ping)."
    );

    use crate::protocol::ConnectionQualityWarningReason::{HighLatency, PacketLoss};
    assert_eq!(
        connection_quality_warning_lines(crate::protocol::ConnectionQualityWarning {
            reason: HighLatency,
            rtt_ms: 0,
            ..warning
        }),
        [
            "Your connection is very slow: shots and hits may not count.",
            "If it doesn't improve, you may be moved to spectator.",
        ]
    );
    assert_eq!(
        connection_quality_warning_lines(crate::protocol::ConnectionQualityWarning {
            reason: PacketLoss,
            loss_per_mille: 125,
            ..warning
        }),
        [
            "Your connection is unstable (13% of packets lost).",
            "If it doesn't improve, you may be moved to spectator.",
        ]
    );
    // Notifications are single lines; 85 characters (7 px each) fit a 640 px window.
    for reason in [
        crate::protocol::ConnectionQualityWarningReason::ShotsRefused,
        HighLatency,
        PacketLoss,
    ] {
        for line in connection_quality_warning_lines(crate::protocol::ConnectionQualityWarning {
            reason,
            rtt_ms: u16::MAX,
            loss_per_mille: 1_000,
            ..warning
        }) {
            assert!(line.len() <= 85, "{line}");
        }
    }
}
