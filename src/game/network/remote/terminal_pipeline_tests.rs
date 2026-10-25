use super::*;

#[test]
fn spectator_same_tick_hit_keeps_one_recent_firing_sound_without_reviving_flight() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/spectator_instant_hit_sound.json"
    ))
    .unwrap();
    let events = fixture["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let hex = row["hex"].as_str().unwrap();
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect::<Vec<_>>();
            crate::realtime_event::AuthenticatedRealtimeEvent::parse(&bytes).unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 2);
    let firing = events[0];
    let crate::realtime_event::RealtimeEventPayload::Weapon(source) = firing.payload else {
        panic!("original firing packet required")
    };
    let terminal = events[1];
    let crate::realtime_event::RealtimeEventPayload::ProjectileState(state) = terminal.payload
    else {
        panic!("terminal packet required")
    };
    assert!(state.terminal);
    assert_eq!(firing.server_tick, 2_355);
    assert_eq!(terminal.server_tick, firing.server_tick);
    for (age, cancelled) in [
        (0, false),
        (7, false),
        (27, false),
        (28, false),
        (50, false),
        (7, true),
    ] {
        let mut game = Game::new_network_authority_test();
        game.install_network_authority_test_scope(
            AuthorityScopeIdentity {
                server_session_id: Some(firing.server_session_id),
                arena_stream_id: Some(firing.arena_stream_id),
                player_id: Some(1),
                life_seq: None,
                alive: None,
            },
            AuthorityReadiness {
                joined: true,
                arena_epoch: Some(2),
                settings: true,
                security: true,
                map: true,
            },
            2,
        );
        game.networked_play = true;
        game.spectator_mode = true;
        let mut actor = NetworkPlayer::from_roster(&PlayerRosterEntry {
            ship: source.ship,
            audio: 0,
            name: "spectatorbeta".to_owned(),
            squad: String::new(),
            kill_points: 0,
            flag_points: 0,
            player_id: source.source_player_id,
            frequency: source.frequency,
            wins: 0,
            losses: 0,
            attach_parent: INVALID_PLAYER_ID,
            flags: 0,
            koth: 0,
        });
        actor.set_life_seq(Some(source.source_life_seq));
        actor.position = Some(vec2(source.x, source.y));
        actor.render_position = actor.position;
        actor.last_seen_snapshot_id = Some(1_063);
        game.network_world
            .players
            .insert(source.source_player_id, actor);
        game.maps[game.map_index].tiles.fill(0);
        game.door_clock_fresh = true;
        game.client_authority.observe_initial_ship(SPECTATOR_SHIP);
        let scope = AuthorityScopeIdentity {
            life_seq: Some(2),
            alive: Some(false),
            ..game.client_authority.view().scope
        };
        game.client_authority.observe_private_lifecycle(
            crate::game::lifecycle_sequencer::ClientAuthoritySource::Event,
            scope,
            None,
            0.0,
        );
        assert!(game.observer_controls_ready());
        assert!(!game.gameplay_step_ready());
        game.last_observed_browser_server_tick = Some(firing.server_tick + age);
        game.apply_realtime_weapon_visual(
            firing.server_session_id,
            firing.arena_stream_id,
            firing.event_id,
            firing.server_tick,
            source,
        );
        assert_eq!(game.remote_projectile_spawns, 1);
        assert!(game.apply_realtime_projectile_state(
            terminal.server_session_id,
            terminal.arena_stream_id,
            terminal.event_id,
            terminal.server_tick,
            state,
        ));
        assert!(game.remote_projectiles.is_empty());
        if cancelled {
            game.apply_owner_weapon_cancellation(
                firing.server_session_id,
                firing.arena_stream_id,
                terminal.event_id + 1,
                crate::realtime_event::RealtimeOwnerWeaponCancellation {
                    source_player_id: source.source_player_id,
                    source_session_id: source.source_session_id,
                    through_shot_group_id: source.shot_group_id,
                    owner_life_seq: source.source_life_seq,
                    owner_input_sequence: None,
                },
            );
        }
        let cues = game.prepare_remote_weapon_views(Some(firing.server_tick + age - 7));
        assert_eq!(
            cues,
            u32::from(age < 28 && !cancelled),
            "receipt age {age}, cancelled={cancelled}"
        );
        assert!(game.pending_remote_birth_effects.is_empty());
        assert!(game.prepared_remote_projectiles.is_empty());
        assert!(game.prepared_remote_bullet_trails.is_empty());
        assert_eq!(
            game.prepare_remote_weapon_views(Some(firing.server_tick)),
            0
        );
    }
}

// A control replay of exact captured bytes. This deliberately does not claim
// to reproduce the original browser's surrounding traffic or frame grouping.
#[test]
fn captured_frozen_bullet_terminal_crosses_poll_and_authority_application() {
    #[derive(Default)]
    struct WireTransport(std::collections::VecDeque<Vec<u8>>);
    impl PacketTransport for WireTransport {
        fn send_packet(&mut self, _packet: &[u8]) -> Result<(), String> {
            Ok(())
        }
        fn recv_packet(&mut self) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.pop_front())
        }
        fn last_received_lane(&self) -> crate::transport::PacketTransportLane {
            crate::transport::PacketTransportLane::Event
        }
    }
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/remote_terminal_frozen_713.json"
    ))
    .unwrap();
    let mut game = Game::new_network_authority_test();
    let scope = AuthorityScopeIdentity {
        server_session_id: Some(1_792_888_843_678_433_671),
        arena_stream_id: Some(1),
        player_id: Some(0),
        life_seq: Some(5),
        alive: Some(true),
    };
    let readiness = AuthorityReadiness {
        joined: true,
        arena_epoch: Some(1),
        settings: true,
        security: true,
        map: true,
    };
    game.install_network_authority_test_scope(scope, readiness, 1);
    game.networked_play = true;
    game.door_clock_fresh = true;
    let mut session =
        ClientProtocolSession::new(true, ArenaLoginRequest::spectator_default(), 0, 0);
    session.self_player_id = scope.player_id;
    session.latest_server_clock_bootstrap = Some(ServerClockBootstrap {
        server_session_id: scope.server_session_id.unwrap(),
        arena_stream_id: scope.arena_stream_id.unwrap(),
        ..ServerClockBootstrap::default()
    });
    let mut client = NetworkClient::new(session, WireTransport::default());
    for (index, row) in fixture["packets"].as_array().unwrap().iter().enumerate() {
        let hex = row["hex"].as_str().unwrap();
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let tick = u32::from_le_bytes(bytes[23..27].try_into().unwrap());
        game.last_observed_browser_server_tick = Some(tick);
        client.transport_mut().0.push_back(bytes.clone());
        client.transport_mut().0.push_back(bytes);
        let actions = client.poll_actions_at_tick(tick).unwrap();
        assert_eq!(actions.events.len(), 1, "two carriers decode only once");
        let view = game.client_authority.view();
        let (before, after) =
            crate::game::browser_state::partition_browser_network_actions_at_segment(
                actions,
                scope.server_session_id,
                scope.arena_stream_id,
                scope.player_id,
                scope.life_seq,
                scope.alive,
                true,
                Some(1),
                true,
                true,
                true,
                view.segment,
                false,
                false,
                false,
            );
        assert!(game.apply_network_actions(&before).into_iter().all(|x| x));
        assert!(game.apply_network_actions(&after).into_iter().all(|x| x));
        game.prepare_remote_weapon_views(Some(tick - 7));
        if index < 2 {
            assert_eq!(game.remote_projectiles.len(), 1);
            assert_eq!(game.prepared_remote_projectiles.len(), 1);
            assert_eq!(
                game.remote_projectiles[0]
                    .authority_binding
                    .unwrap()
                    .projectile_id,
                713
            );
            assert_eq!(
                game.remote_projectiles[0]
                    .presentation_clock
                    .unwrap()
                    .motion_frozen,
                index == 1
            );
        } else {
            assert!(game.remote_projectiles.is_empty());
            assert!(game.prepared_remote_projectiles.is_empty());
            game.prepare_remote_weapon_views(Some(5_960));
            assert!(game.prepared_remote_projectiles.is_empty());
        }
    }
}
