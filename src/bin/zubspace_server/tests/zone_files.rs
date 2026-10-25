use super::*;

#[test]
fn server_advertises_and_serves_chaos_zone_map_and_lvz_files() {
    let map = zone_map();
    let info = map_information(map);
    assert_eq!(info[0], MAP_INFORMATION);
    assert_eq!(
        info.len(),
        1 + crate::zone_wire::AUTHENTICATED_MAP_INFO_HEADER_BYTES
            + SERVER_LVZ_FILES.len() * crate::zone_wire::AUTHENTICATED_MAP_INFO_FILE_BYTES
    );
    assert_eq!(&info[1..5], b"ZMAP");
    assert_eq!(
        read_fixed_test_string(&info[6..22]),
        crate::zone::CHAOS_ZONE_ID
    );
    assert_eq!(read_fixed_test_string(&info[22..38]), SERVER_MAP_FILENAME);
    assert_eq!(
        u32::from_le_bytes(info[38..42].try_into().unwrap()),
        map.checksum
    );
    assert_eq!(
        u32::from_le_bytes(info[42..46].try_into().unwrap()),
        map.compressed.len() as u32
    );
    assert_eq!(&info[46..78], &map.sha256);
    for (index, lvz) in map.lvz_files.iter().enumerate() {
        let offset = 80 + index * crate::zone_wire::AUTHENTICATED_MAP_INFO_FILE_BYTES;
        assert_eq!(
            read_fixed_test_string(&info[offset..offset + 16]),
            lvz.filename
        );
        assert_eq!(
            u32::from_le_bytes(info[offset + 16..offset + 20].try_into().unwrap()),
            lvz.checksum
        );
        assert_eq!(
            u32::from_le_bytes(info[offset + 20..offset + 24].try_into().unwrap()),
            lvz.bytes.len() as u32
        );
        assert_eq!(&info[offset + 24..offset + 56], &lvz.sha256);
    }

    let packet = compressed_map(map);
    assert_eq!(packet[0], COMPRESSED_MAP);
    assert_eq!(
        packet.len(),
        COMPRESSED_MAP_HEADER_SIZE + map.compressed.len()
    );
    assert_eq!(decompress_map_payload(&packet), SERVER_MAP_BYTES);

    let first_lvz = zone_file_download(1, map).expect("first LVZ should download");
    assert_eq!(download_payload(&first_lvz), SERVER_LVZ_FILES[0].1);
    let last_lvz =
        zone_file_download(SERVER_LVZ_FILES.len() as u16, map).expect("last LVZ should exist");
    assert_eq!(
        download_payload(&last_lvz),
        SERVER_LVZ_FILES.last().unwrap().1
    );
    assert!(zone_file_download(SERVER_LVZ_FILES.len() as u16 + 1, map).is_none());
}

#[test]
fn cached_map_check_opens_security_only_for_authenticated_joined_active_zone_digest() {
    let canonical_door_state = crate::doors::ClientDoorState::for_challenge(0x1234_5678);
    let state = Arc::new(Mutex::new(ServerState {
        server_door_challenge_state: canonical_door_state,
        server_door_state: canonical_door_state,
        ..ServerState::default()
    }));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    set_player_identity_and_name(
        &state,
        player_id,
        "000000000001".to_string(),
        "pilot".to_string(),
    );
    join_arena(&state, player_id, 0);
    while rx.try_recv().is_ok() {}
    let (identity, zone_id, sha256) = {
        let state = state.lock().expect("state");
        let map = state.zone_map();
        (
            state.active_player_session_identity(player_id).unwrap(),
            map.zone_id.as_ref(),
            map.sha256,
        )
    };
    let mut check = vec![
        crate::session::CACHED_MAP_CHECK,
        crate::zone_wire::CACHED_MAP_CHECK_VERSION,
    ];
    check.resize(2 + crate::zone_wire::ZONE_ID_BYTES, 0);
    check[2..2 + zone_id.len()].copy_from_slice(zone_id.as_bytes());
    check.extend_from_slice(&sha256);

    assert!(crate::session::serve_cached_map_check(
        &state, identity, &check
    ));
    let challenge = read_reliable_payload_from_channel(&rx);
    assert_eq!(challenge[0], SECURITY_CHALLENGE);
    assert_eq!(
        u32::from_le_bytes(challenge[9..13].try_into().expect("door anchor bytes")),
        canonical_door_state.timestamp
    );
    assert_eq!(
        state.lock().expect("state").clients[&player_id].door_state,
        Some(canonical_door_state),
        "the production map session must retain the same arena anchor it sent to the browser"
    );

    let mut invalid = check.clone();
    *invalid.last_mut().unwrap() ^= 1;
    assert!(!crate::session::serve_cached_map_check(
        &state, identity, &invalid
    ));
    assert!(rx.try_recv().is_err());
    assert!(!crate::session::serve_cached_map_check(
        &state,
        identity,
        &[crate::session::CACHED_MAP_CHECK]
    ));
}

#[test]
fn file_requests_are_exact_authenticated_ordered_idempotent_and_lifecycle_scoped() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    let identity = state
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();
    let map_request = [crate::session::FILE_REQUEST, 0, 0];
    assert!(!serve_file_request(&state, identity, &map_request));
    set_player_identity_and_name(
        &state,
        player_id,
        "000000000002".to_string(),
        "pilot".to_string(),
    );
    assert!(!serve_file_request(&state, identity, &map_request));
    join_arena(&state, player_id, 0);
    while rx.try_recv().is_ok() {}

    for malformed in [
        vec![crate::session::FILE_REQUEST],
        vec![crate::session::FILE_REQUEST, 0, 0, 0],
        vec![0xff, 0, 0],
    ] {
        assert!(!serve_file_request(&state, identity, &malformed));
    }
    assert!(!serve_file_request(
        &state,
        identity,
        &[crate::session::FILE_REQUEST, 1, 0]
    ));

    assert!(serve_file_request(&state, identity, &map_request));
    let pending_after_first = state.lock().unwrap().clients[&player_id]
        .outgoing_reliable
        .pending_len();
    for _ in 0..100 {
        assert!(serve_file_request(&state, identity, &map_request));
    }
    assert_eq!(
        state.lock().unwrap().clients[&player_id]
            .outgoing_reliable
            .pending_len(),
        pending_after_first,
        "duplicate/replayed application requests must not enqueue another transfer"
    );
    assert!(!serve_file_request(
        &state,
        identity,
        &[crate::session::FILE_REQUEST, 2, 0]
    ));
    assert!(!serve_file_request(
        &state,
        identity,
        &[crate::session::FILE_REQUEST, 1, 0]
    ));

    leave_arena(&state, player_id);
    assert!(!serve_file_request(&state, identity, &map_request));
    assert!(
        state.lock().unwrap().clients[&player_id]
            .map_transfer
            .is_none()
    );
}

/// Fill an active transfer's window without acknowledging anything. The recovery reads only two facts - the window is
/// full and `next_offset` has not moved - so topping the window up here is a direct statement of the state it must act
/// on rather than a replay of a hundred send calls.
fn fill_active_map_transfer_window(state: &Arc<Mutex<ServerState>>, player_id: u16) {
    let mut locked = state.lock().expect("test state should not be poisoned");
    let active = locked
        .clients
        .get_mut(&player_id)
        .expect("registered client")
        .map_transfer
        .as_mut()
        .expect("transfer must be active")
        .active
        .as_mut()
        .expect("file transfer must be active");
    while active.assigned_ids.len() < crate::reliable::APPLICATION_TRANSFER_IN_FLIGHT_FRAMES {
        let next = active.assigned_ids.back().copied().unwrap_or(0) + 1;
        active.assigned_ids.push_back(next);
    }
    assert_eq!(
        active.assigned_ids.len(),
        crate::reliable::APPLICATION_TRANSFER_IN_FLIGHT_FRAMES
    );
}

fn active_map_window(state: &Arc<Mutex<ServerState>>, player_id: u16) -> (usize, usize) {
    let locked = state.lock().expect("test state should not be poisoned");
    let active = locked.clients[&player_id]
        .map_transfer
        .as_ref()
        .and_then(|transfer| transfer.active.as_ref())
        .expect("file transfer must be active");
    (active.next_offset, active.assigned_ids.len())
}

#[test]
fn a_full_map_window_without_progress_reports_the_session_once_progress_has_stopped() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    set_player_identity_and_name(
        &state,
        player_id,
        "000000000031".to_string(),
        "pilot".to_string(),
    );
    join_arena(&state, player_id, 0);
    let identity = state
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();
    assert!(serve_file_request(
        &state,
        identity,
        &[crate::session::FILE_REQUEST, 0, 0],
    ));
    fill_active_map_transfer_window(&state, player_id);

    let report = |state: &Arc<Mutex<ServerState>>| {
        let mut locked = state.lock().expect("test state should not be poisoned");
        crate::reliable::note_map_transfer_stall_with_timeout(
            player_id,
            locked
                .clients
                .get_mut(&player_id)
                .expect("registered client"),
            0,
        )
    };

    // The first sight of a full window only starts the clock: a transfer that has just filled its window is normal, and
    // retiring it there would disconnect every healthy join.
    assert_eq!(
        report(&state),
        None,
        "first observation of a full window must not report the session"
    );
    assert_eq!(
        report(&state),
        Some(identity),
        "a full window with no observed progress past the timeout must report the session"
    );
}

#[test]
fn observed_progress_in_a_full_map_window_restarts_the_stall_clock() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    set_player_identity_and_name(
        &state,
        player_id,
        "000000000032".to_string(),
        "pilot".to_string(),
    );
    join_arena(&state, player_id, 0);
    let identity = state
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();
    assert!(serve_file_request(
        &state,
        identity,
        &[crate::session::FILE_REQUEST, 0, 0],
    ));
    fill_active_map_transfer_window(&state, player_id);

    let (offset_before, _) = active_map_window(&state, player_id);
    // Start the clock, then move the transfer forward: a slow client on a real network shows exactly this shape, and it
    // must never be retired by the stall recovery.
    {
        let mut locked = state.lock().expect("test state should not be poisoned");
        crate::reliable::note_map_transfer_stall_with_timeout(
            player_id,
            locked
                .clients
                .get_mut(&player_id)
                .expect("registered client"),
            0,
        );
        locked
            .clients
            .get_mut(&player_id)
            .expect("registered client")
            .map_transfer
            .as_mut()
            .expect("transfer must be active")
            .active
            .as_mut()
            .expect("file transfer must be active")
            .next_offset = offset_before + 1;
    }
    {
        let mut locked = state.lock().expect("test state should not be poisoned");
        assert_eq!(
            crate::reliable::note_map_transfer_stall_with_timeout(
                player_id,
                locked
                    .clients
                    .get_mut(&player_id)
                    .expect("registered client"),
                0,
            ),
            None,
            "progress inside the window must restart the stall clock"
        );
    }
}

#[test]
fn the_tick_pump_retires_a_session_whose_map_window_has_stalled() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    set_player_identity_and_name(
        &state,
        player_id,
        "000000000033".to_string(),
        "pilot".to_string(),
    );
    join_arena(&state, player_id, 0);
    let identity = state
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();
    assert!(serve_file_request(
        &state,
        identity,
        &[crate::session::FILE_REQUEST, 0, 0],
    ));
    fill_active_map_transfer_window(&state, player_id);

    // Backdate the tracker past the production timeout rather than lowering the timeout: the timeout is process-wide and
    // other suites run in the same process, so a test that edits it would retire their sessions too.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_millis() as u64;
    {
        let mut locked = state.lock().expect("test state should not be poisoned");
        let active = locked
            .clients
            .get_mut(&player_id)
            .expect("registered client")
            .map_transfer
            .as_mut()
            .expect("transfer must be active")
            .active
            .as_mut()
            .expect("file transfer must be active");
        active.stall_offset = active.next_offset;
        active.stall_since_ms = now_ms - 9_000;
    }

    let _ = crate::reliable::resend_due_reliable_to_clients(&state, Instant::now());

    assert!(
        !state.lock().unwrap().clients.contains_key(&player_id),
        "a session whose map window stalled past the terminal timeout must be disconnected, not held invisible"
    );
}

#[test]
fn exact_successor_request_waits_for_the_current_files_final_transport_ack() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    set_player_identity_and_name(
        &state,
        player_id,
        "000000000015".to_string(),
        "pilot".to_string(),
    );
    join_arena(&state, player_id, 0);
    while rx.try_recv().is_ok() {}
    let identity = state
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();

    assert!(serve_file_request(
        &state,
        identity,
        &[crate::session::FILE_REQUEST, 0, 0],
    ));
    for _ in 0..crate::zone_wire::MAX_TRANSFER_FRAGMENTS {
        let (fully_assigned, next_reliable_id) = {
            let locked = state.lock().unwrap();
            let active = locked.clients[&player_id]
                .map_transfer
                .as_ref()
                .and_then(|transfer| transfer.active.as_ref())
                .unwrap();
            (
                active.next_offset == active.payload.len(),
                active.assigned_ids.front().copied(),
            )
        };
        if fully_assigned {
            break;
        }
        assert!(crate::reliable::receive_reliable_acks(
            &state,
            identity,
            &crate::reliable::reliable_ack(next_reliable_id.unwrap()),
        ));
    }

    let successor = [crate::session::FILE_REQUEST, 1, 0];
    assert!(serve_file_request(&state, identity, &successor));
    assert!(serve_file_request(&state, identity, &successor));
    assert!(!serve_file_request(
        &state,
        identity,
        &[crate::session::FILE_REQUEST, 2, 0],
    ));
    let current_ids = {
        let locked = state.lock().unwrap();
        let transfer = locked.clients[&player_id].map_transfer.as_ref().unwrap();
        assert_eq!(transfer.next_index, 0);
        assert_eq!(transfer.pending_index, Some(0));
        assert_eq!(transfer.deferred_successor_index, Some(1));
        transfer
            .active
            .as_ref()
            .unwrap()
            .assigned_ids
            .iter()
            .copied()
            .collect::<Vec<_>>()
    };
    for reliable_id in current_ids {
        assert!(crate::reliable::receive_reliable_acks(
            &state,
            identity,
            &crate::reliable::reliable_ack(reliable_id),
        ));
    }

    let locked = state.lock().unwrap();
    let transfer = locked.clients[&player_id].map_transfer.as_ref().unwrap();
    assert_eq!(transfer.next_index, 1);
    assert_eq!(transfer.pending_index, Some(1));
    assert_eq!(transfer.deferred_successor_index, None);
    assert_eq!(
        transfer.active.as_ref().map(|active| active.file_index),
        Some(1)
    );
}

#[test]
fn valid_file_request_defers_at_sender_capacity_and_ack_flushes_it_once() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    set_player_identity_and_name(
        &state,
        player_id,
        "000000000004".to_string(),
        "pilot".to_string(),
    );
    join_arena(&state, player_id, 0);
    while rx.try_recv().is_ok() {}
    let identity = state
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();

    {
        let mut locked = state.lock().unwrap();
        let client = locked.clients.get_mut(&player_id).unwrap();
        client.outgoing_reliable = Default::default();
        client.bootstrap_reliable_barrier = None;
        // Fill every application-transfer slot while retaining the explicit
        // control reserve. A valid request must be remembered, not dropped.
        for _ in 0..3584 {
            crate::reliable::send_reliable_payload(client, &[0x42]);
        }
    }
    while rx.try_recv().is_ok() {}

    let request = [crate::session::FILE_REQUEST, 0, 0];
    assert!(serve_file_request(&state, identity, &request));
    {
        let locked = state.lock().unwrap();
        let transfer = locked.clients[&player_id].map_transfer.as_ref().unwrap();
        assert_eq!(transfer.pending_index, Some(0));
        assert_eq!(transfer.next_index, 0);
    }
    // Retries while capacity is unavailable are idempotent.
    assert!(serve_file_request(&state, identity, &request));

    for reliable_id in 0..3584 {
        assert!(crate::reliable::receive_reliable_acks(
            &state,
            identity,
            &crate::reliable::reliable_ack(reliable_id),
        ));
    }
    // Capacity opening assigns only the bounded first fragment window. Drive
    // its exact ordered ACK frontier until the complete map packet retires.
    for _ in 0..crate::zone_wire::MAX_TRANSFER_FRAGMENTS {
        let next_fragment_id = {
            let locked = state.lock().unwrap();
            let transfer = locked.clients[&player_id].map_transfer.as_ref().unwrap();
            if transfer.pending_index.is_none() {
                None
            } else {
                transfer
                    .active
                    .as_ref()
                    .and_then(|active| active.assigned_ids.front().copied())
            }
        };
        let Some(reliable_id) = next_fragment_id else {
            break;
        };
        assert!(crate::reliable::receive_reliable_acks(
            &state,
            identity,
            &crate::reliable::reliable_ack(reliable_id),
        ));
    }
    let pending_after_flush = {
        let locked = state.lock().unwrap();
        let client = &locked.clients[&player_id];
        let transfer = client.map_transfer.as_ref().unwrap();
        assert_eq!(transfer.pending_index, None);
        assert_eq!(transfer.next_index, 1);
        assert!(client.door_state.is_some());
        client.outgoing_reliable.pending_len()
    };
    // Re-requesting a file the server already delivered used to be suppressed and reported as handled, which is what made a
    // strand permanent: the client had already discarded the one copy it was given, and nothing would ever send another.
    // It is now a bounded re-drive, so the file goes out again.
    assert!(serve_file_request(&state, identity, &request));
    let pending_after_redrive = state.lock().unwrap().clients[&player_id]
        .outgoing_reliable
        .pending_len();
    assert!(
        pending_after_redrive > pending_after_flush,
        "a re-requested file must be re-served, not silently dropped"
    );
    {
        let locked = state.lock().unwrap();
        let transfer = locked.clients[&player_id].map_transfer.as_ref().unwrap();
        assert_eq!(
            transfer.next_index, 0,
            "a re-drive rewinds delivery to the file the client could not use"
        );
        assert_eq!(transfer.redrive_count, 1);
    }
    // The re-drive is bounded, because an asset that is genuinely unusable must not let a client drive the same transfer
    // forever. At the budget the request is reported handled without enqueuing again.
    {
        let mut locked = state.lock().unwrap();
        let client = locked.clients.get_mut(&player_id).unwrap();
        client.map_transfer.as_mut().unwrap().redrive_count = 4;
    }
    assert!(serve_file_request(&state, identity, &request));
    assert_eq!(
        state.lock().unwrap().clients[&player_id]
            .outgoing_reliable
            .pending_len(),
        pending_after_redrive,
        "past the re-drive budget the server must stop re-serving"
    );
}

#[test]
fn map_transfer_loss_cannot_allocate_control_beyond_client_reorder_window() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    set_player_identity_and_name(
        &state,
        player_id,
        "000000000014".to_string(),
        "pilot".to_string(),
    );
    join_arena(&state, player_id, 0);
    while rx.try_recv().is_ok() {}
    let identity = state
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();
    {
        let mut locked = state.lock().unwrap();
        let client = locked.clients.get_mut(&player_id).unwrap();
        client.outgoing_reliable = Default::default();
        client.outgoing_reliable.set_next_id_for_test(u32::MAX - 31);
        client.bootstrap_reliable_barrier = None;
    }

    assert!(serve_file_request(
        &state,
        identity,
        &[crate::session::FILE_REQUEST, 0, 0],
    ));
    let (first_fragment, later_fragments) = {
        let mut locked = state.lock().unwrap();
        let client = locked.clients.get_mut(&player_id).unwrap();
        let active = client
            .map_transfer
            .as_ref()
            .and_then(|transfer| transfer.active.as_ref())
            .unwrap();
        assert_eq!(
            active.assigned_ids.len(),
            crate::reliable::APPLICATION_TRANSFER_IN_FLIGHT_FRAMES
        );
        let ids = active.assigned_ids.iter().copied().collect::<Vec<_>>();
        crate::reliable::send_reliable_payload(client, &[0x42]);
        (ids[0], ids[1..].to_vec())
    };

    // Every later file fragment is received, but the first is lost. Their
    // out-of-order ACKs must not open new file slots beyond the 64-id window.
    for reliable_id in later_fragments {
        assert!(crate::reliable::receive_reliable_acks(
            &state,
            identity,
            &crate::reliable::reliable_ack(reliable_id),
        ));
    }
    {
        let locked = state.lock().unwrap();
        let client = &locked.clients[&player_id];
        let active = client
            .map_transfer
            .as_ref()
            .and_then(|transfer| transfer.active.as_ref())
            .unwrap();
        assert_eq!(
            active.assigned_ids.len(),
            crate::reliable::APPLICATION_TRANSFER_IN_FLIGHT_FRAMES
        );
        assert_eq!(active.assigned_ids.front().copied(), Some(first_fragment));
        let ids = client.outgoing_reliable.pending_ids();
        let control_id = first_fragment
            .wrapping_add(crate::reliable::APPLICATION_TRANSFER_IN_FLIGHT_FRAMES as u32);
        assert!(ids.contains(&control_id));
        assert_eq!(
            control_id.wrapping_sub(first_fragment) as usize,
            crate::reliable::APPLICATION_TRANSFER_IN_FLIGHT_FRAMES,
            "the control immediately after the bounded burst stays inside the client window"
        );
    }

    // Once the missing frontier lands, all already-ACKed successors retire at
    // once and the next bounded burst may progress.
    assert!(crate::reliable::receive_reliable_acks(
        &state,
        identity,
        &crate::reliable::reliable_ack(first_fragment),
    ));
    let locked = state.lock().unwrap();
    let active = locked.clients[&player_id]
        .map_transfer
        .as_ref()
        .and_then(|transfer| transfer.active.as_ref())
        .unwrap();
    assert_eq!(
        active.assigned_ids.len(),
        crate::reliable::APPLICATION_TRANSFER_IN_FLIGHT_FRAMES
    );
    assert_eq!(
        active.assigned_ids.front().copied(),
        Some(
            first_fragment
                .wrapping_add(crate::reliable::APPLICATION_TRANSFER_IN_FLIGHT_FRAMES as u32 + 1,)
        ),
        "the next file window advances across u32 wrap without reusing the interleaved control id"
    );
}

#[test]
fn cached_map_attestation_rejects_wrong_zone_digest_prejoin_and_stale_session() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, _rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    set_player_identity_and_name(
        &state,
        player_id,
        "000000000003".to_string(),
        "pilot".to_string(),
    );
    let identity = state
        .lock()
        .unwrap()
        .active_player_session_identity(player_id)
        .unwrap();
    let (zone_id, sha256) = {
        let locked = state.lock().unwrap();
        let map = locked.zone_map();
        (map.zone_id.as_ref(), map.sha256)
    };
    let make_check = |zone: &str, digest: crate::zone_wire::Sha256Digest| {
        let mut packet = vec![
            crate::session::CACHED_MAP_CHECK,
            crate::zone_wire::CACHED_MAP_CHECK_VERSION,
        ];
        packet.resize(2 + crate::zone_wire::ZONE_ID_BYTES, 0);
        packet[2..2 + zone.len()].copy_from_slice(zone.as_bytes());
        packet.extend_from_slice(&digest);
        packet
    };
    let valid = make_check(zone_id, sha256);
    assert!(!crate::session::serve_cached_map_check(
        &state, identity, &valid
    ));
    join_arena(&state, player_id, 0);
    assert!(!crate::session::serve_cached_map_check(
        &state,
        identity,
        &make_check("other-zone", sha256)
    ));
    let mut wrong_digest = sha256;
    wrong_digest[0] ^= 1;
    assert!(!crate::session::serve_cached_map_check(
        &state,
        identity,
        &make_check(zone_id, wrong_digest)
    ));
    let stale = crate::authority_identity::PlayerSessionIdentity::new(
        player_id,
        crate::authority_identity::PlayerSessionId::new(u64::MAX),
    );
    assert!(!crate::session::serve_cached_map_check(
        &state, stale, &valid
    ));
}

#[test]
fn server_can_select_bundled_non_chaos_zone_map() {
    let extreme = zone_map_for_id("extreme").expect("extreme zone should be bundled");
    assert_eq!(extreme.display_name, "Extreme");
    assert_eq!(extreme.filename, "doomsy8.lvl");
    assert!(!extreme.compressed.is_empty());
    assert!(!extreme.lvz_files.is_empty());

    // A ServerState configured with the selected zone's id serves that zone's
    // own assets: the map-information packet names doomsy8.lvl, and a news
    // request transfers the bundled Extreme news file verbatim.
    let state = Arc::new(Mutex::new(ServerState {
        config: ServerConfig {
            default_zone: "extreme".to_string(),
            ..ServerConfig::default()
        },
        ..ServerState::default()
    }));
    let info = map_information(state.lock().expect("state").zone_map());
    assert_eq!(read_fixed_test_string(&info[22..38]), "doomsy8.lvl");

    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(&state, tx);
    serve_news_request(&state, player_id);
    let news = read_reliable_payload_from_channel(&rx);
    assert_eq!(
        news,
        server_file_transfer(extreme.news_filename.as_ref(), extreme.news_bytes.as_ref())
    );
}

#[test]
fn server_detects_chaos_zone_safe_tiles_from_position_packets() {
    let (x, y) = first_safe_zone_position();
    assert_eq!(tile_at_world(zone_map(), x, y), SAFE_TILE_ID);
    assert!(world_position_in_safe_tile(x, y));

    let position = large_position_from_client(7, &client_position_packet(0x1122_3344, x, y))
        .expect("client position should encode");
    assert!(large_position_packet_in_safe_tile(&position));
}

#[test]
fn server_position_validation_tracks_dynamic_door_seed_solidity() {
    assert!(solid_tile(162));
    assert!(solid_tile(169));
    // Without door seed context, doors must be treated as blocked so an unhandshaked
    // client cannot walk through them.
    assert!(position_blocking_tile(162));
    assert!(position_blocking_tile(169));
    assert!(position_blocking_tile_for_door_seeds(162, &[1]));
    assert!(position_blocking_tile_for_door_seeds(169, &[0x80]));
    assert!(!position_blocking_tile_for_door_seeds(162, &[0]));
    assert!(!position_blocking_tile_for_door_seeds(169, &[0]));
    assert!(!position_blocking_tile_for_door_seeds(162, &[1, 0]));
}

#[test]
fn server_spawn_coordinates_prefer_center_core_arena() {
    let map = zone_map();
    let spawn = server_spawn_coordinates(map);
    let center = (MAP_TILES as f32 * TILE_SIZE_PX * 0.5) as u16;
    assert!(spawn.0.abs_diff(center) <= TILE_SIZE_PX as u16);
    assert!(spawn.1.abs_diff(center) <= TILE_SIZE_PX as u16);
    assert!(!solid_tile(tile_at_world(map, spawn.0, spawn.1)));
}

#[test]
fn server_encodes_chaos_zone_news_file_transfer() {
    let packet = server_file_transfer(SERVER_NEWS_FILENAME, SERVER_NEWS_BYTES);

    assert_eq!(packet[0], SERVER_FILE_TRANSFER);
    assert_eq!(&packet[1..10], b"news.txt\0");
    assert_eq!(&packet[17..], SERVER_NEWS_BYTES);
    assert!(String::from_utf8_lossy(&packet[17..]).contains("Chaos Zone"));
}

#[test]
fn server_encodes_arena_directory_metadata() {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx_a, _) = mpsc::channel();
    let (tx_b, _) = mpsc::channel();
    let alpha = register_connection(&state, tx_a);
    let beta = register_connection(&state, tx_b);
    join_arena(&state, alpha, 0);
    join_arena(&state, beta, 0);

    let directory = arena_directory_listing(&state);
    assert_arena_directory_entries(
        &directory,
        &[("Chaos Zone".to_string(), 2), ("Extreme".to_string(), 0)],
    );
}

#[test]
fn join_bootstrap_does_not_advertise_a_placeholder_zone_banner() {
    for zone in ["chaos-zone", "extreme"] {
        let state = Arc::new(Mutex::new(ServerState::new(ServerConfig {
            default_zone: zone.to_string(),
            ..ServerConfig::default()
        })));
        let client = connect_login_and_join(&state, "pilot", 0);
        let payloads: Vec<_> = client
            .rx
            .try_iter()
            .filter_map(|packet| reliable_parts(&packet).map(|(_, payload)| payload.to_vec()))
            .collect();
        assert!(
            payloads.iter().any(|packet| packet == &[0x02]),
            "missing joined fact for {zone}"
        );
        assert!(
            payloads
                .iter()
                .any(|packet| packet.first() == Some(&ARENA_DIRECTORY)),
            "missing directory for {zone}"
        );
        assert!(
            payloads.iter().all(|packet| packet.first() != Some(&0x30)),
            "unexpected banner advertisement for {zone}"
        );
    }
}

fn held_zone_fixture() -> (
    Arc<Mutex<ServerState>>,
    crate::authority_identity::PlayerSessionIdentity,
    mpsc::Receiver<Vec<u8>>,
) {
    let state = Arc::new(Mutex::new(ServerState::default()));
    let (tx, rx) = mpsc::channel();
    let id = register_connection(&state, tx);
    assert!(set_player_identity_and_name(
        &state,
        id,
        "100000000001".into(),
        "Ace".into()
    ));
    join_arena(&state, id, 0);
    while rx.try_recv().is_ok() {}
    let identity = state
        .lock()
        .unwrap()
        .active_player_session_identity(id)
        .unwrap();
    (state, identity, rx)
}
fn held_zone_packet(state: &Arc<Mutex<ServerState>>, index: u16) -> Vec<u8> {
    let guard = state.lock().unwrap();
    let map = guard.zone_map();
    crate::zone_wire::HeldZoneFile {
        zone_id: map.zone_id.to_string(),
        map_sha256: map.sha256,
        index,
        sha256: if index == 0 {
            map.sha256
        } else {
            map.lvz_files[index as usize - 1].sha256
        },
    }
    .encode()
}

#[test]
fn held_zone_files_can_arrive_out_of_order_and_are_counted_once_without_transfers() {
    let (state, id, rx) = held_zone_fixture();
    let count = state.lock().unwrap().zone_map().lvz_files.len() + 1;
    for index in (0..count as u16).rev() {
        let packet = held_zone_packet(&state, index);
        assert!(crate::session::serve_cached_map_check(&state, id, &packet));
        assert!(crate::session::serve_cached_map_check(&state, id, &packet));
    }
    let guard = state.lock().unwrap();
    let transfer = guard.clients[&id.player_id].map_transfer.as_ref().unwrap();
    assert_eq!(transfer.next_index, count as u16);
    assert_eq!(transfer.held_files.len(), count);
    assert!(
        transfer.pending_index.is_none() && transfer.active.is_none() && transfer.summary_logged
    );
    assert_eq!(guard.telemetry.zone_files_held, count as u64);
    assert_eq!(guard.telemetry.zone_files_sent, 0);
    let metrics = crate::observability::metrics(&guard, std::time::Instant::now());
    assert!(metrics.contains(&format!("zubspace_zone_files_held_total {count}\n")));
    assert!(metrics.contains("zubspace_zone_files_sent_total 0\n"));
    assert!(metrics.contains("zubspace_zone_bytes_held_total "));
    assert!(metrics.contains("zubspace_zone_bytes_sent_total 0\n"));
    assert!(guard.telemetry.zone_bytes_held > 0);
    assert_eq!(guard.clients[&id.player_id].last_security_valid, None);
    drop(guard);
    assert_eq!(
        read_reliable_payload_from_channel(&rx)[0],
        SECURITY_CHALLENGE
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn held_zone_claims_fail_closed_for_wrong_digest_manifest_index_and_session() {
    let (state, id, _rx) = held_zone_fixture();
    let valid = held_zone_packet(&state, 1);
    for offset in [2, 18, 52] {
        let mut bad = valid.clone();
        bad[offset] ^= 1;
        assert!(!crate::session::serve_cached_map_check(&state, id, &bad));
    }
    let mut bad = valid.clone();
    bad[50..52].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(!crate::session::serve_cached_map_check(&state, id, &bad));
    assert!(!crate::session::serve_cached_map_check(
        &state,
        id,
        &valid[..83]
    ));
    assert_eq!(state.lock().unwrap().telemetry.zone_files_held, 0);
    crate::arena::disconnect_player_session(&state, id);
    assert!(!crate::session::serve_cached_map_check(&state, id, &valid));
}

#[test]
fn held_lvz_prefix_preserves_the_download_final_ack_and_skips_to_the_missing_successor() {
    let (state, id, _rx) = held_zone_fixture();
    assert!(serve_file_request(
        &state,
        id,
        &[crate::session::FILE_REQUEST, 0, 0]
    ));
    for index in [1, 2] {
        assert!(crate::session::serve_cached_map_check(
            &state,
            id,
            &held_zone_packet(&state, index)
        ));
    }
    for _ in 0..crate::zone_wire::MAX_TRANSFER_FRAGMENTS {
        let (complete, next) = {
            let guard = state.lock().unwrap();
            let active = guard.clients[&id.player_id]
                .map_transfer
                .as_ref()
                .unwrap()
                .active
                .as_ref()
                .unwrap();
            (
                active.next_offset == active.payload.len(),
                active.assigned_ids.front().copied(),
            )
        };
        if complete {
            break;
        }
        assert!(crate::reliable::receive_reliable_acks(
            &state,
            id,
            &crate::reliable::reliable_ack(next.unwrap())
        ));
    }
    assert!(serve_file_request(
        &state,
        id,
        &[crate::session::FILE_REQUEST, 3, 0]
    ));
    let ids = {
        let guard = state.lock().unwrap();
        let transfer = guard.clients[&id.player_id].map_transfer.as_ref().unwrap();
        assert_eq!(transfer.next_index, 0);
        assert_eq!(transfer.deferred_successor_index, Some(3));
        transfer
            .active
            .as_ref()
            .unwrap()
            .assigned_ids
            .iter()
            .copied()
            .collect::<Vec<_>>()
    };
    for ack in ids {
        assert!(crate::reliable::receive_reliable_acks(
            &state,
            id,
            &crate::reliable::reliable_ack(ack)
        ));
    }
    let guard = state.lock().unwrap();
    let transfer = guard.clients[&id.player_id].map_transfer.as_ref().unwrap();
    assert_eq!(transfer.next_index, 3);
    assert_eq!(transfer.pending_index, Some(3));
    assert_eq!(guard.telemetry.zone_files_sent, 1);
    assert_eq!(guard.telemetry.zone_files_held, 2);
}

#[test]
fn falsely_held_map_still_requires_the_original_security_proof() {
    let (state, id, rx) = held_zone_fixture();
    assert!(crate::session::serve_cached_map_check(
        &state,
        id,
        &held_zone_packet(&state, 0)
    ));
    assert_eq!(
        read_reliable_payload_from_channel(&rx)[0],
        SECURITY_CHALLENGE
    );
    let map = state.lock().unwrap().zone_map();
    let forged = security_response_packet(
        continuum_settings_checksum(SECURITY_CHECKSUM_KEY, arena_settings_packet()),
        map.map_checksum.wrapping_add(1),
        Some(map.continuum_crc32),
    );
    crate::security::update_security_report(&state, id.player_id, &forged);
    assert_eq!(
        state.lock().unwrap().clients[&id.player_id].last_security_valid,
        Some(false)
    );
}
