use std::sync::{Arc, Mutex};

use super::reliable::{
    begin_bounded_map_application_transfer, flush_bounded_map_application_transfer,
    send_reliable_payload, send_reliable_to,
};
use super::security::security_challenge_for_door_state;
use super::state::ServerState;
use super::zone::{server_file_transfer, zone_file_download};
use crate::authority_identity::PlayerSessionIdentity;
pub(crate) const PASSWORD_LOGIN_CONTINUUM: u8 = 0x24;
pub(crate) const LOGIN_FAILURE_MESSAGE: u8 = 0x33;
pub(crate) const FILE_REQUEST: u8 = 0x0c;
pub(crate) const NEWS_REQUEST: u8 = 0x0d;
pub(crate) const CACHED_MAP_CHECK: u8 = 0x7c;

pub(crate) fn serve_file_request(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerSessionIdentity,
    request: &[u8],
) -> bool {
    // These first three exits returned false with no trace at all. They are the refusals that matter most for a stranded
    // session, because a client that re-requests a file forever and is dropped here produces neither an accepted log nor a
    // refusal log, which is exactly how "the server received nothing" was being concluded from server-side silence alone.
    if request.len() != 3 || request.first() != Some(&FILE_REQUEST) {
        tracing::info!(
            player_id = identity.player_id,
            session = ?identity.session_id,
            len = request.len(),
            first = ?request.first(),
            reason = "malformed_request_shape",
            reports_handled = false,
            "server zone_file_request_refused"
        );
        return false;
    }
    let index = u16::from_le_bytes([request[1], request[2]]);
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state.player_session_matches(identity) {
        tracing::info!(
            player_id = identity.player_id,
            session = ?identity.session_id,
            reason = "session_mismatch",
            reports_handled = false,
            "server zone_file_request_refused"
        );
        return false;
    }
    let map = state.zone_map();
    let Some((
        has_player_identity,
        joined,
        zone_id,
        next_index,
        pending_index,
        has_active_transfer,
        active_file_index,
        active_payload_assigned,
        deferred_successor_index,
        redrive_count,
        file_count,
        map_sha256,
    )) = state.clients.get(&identity.player_id).and_then(|client| {
        client.map_transfer.as_ref().map(|transfer| {
            (
                client.player_identity.is_some(),
                client.joined,
                transfer.zone_id,
                transfer.next_index,
                transfer.pending_index,
                transfer.active.is_some(),
                transfer.active.as_ref().map(|active| active.file_index),
                transfer
                    .active
                    .as_ref()
                    .is_some_and(|active| active.next_offset == active.payload.len()),
                transfer.deferred_successor_index,
                transfer.redrive_count,
                transfer.file_count,
                transfer.map_sha256,
            )
        })
    })
    else {
        tracing::info!(
            player_id = identity.player_id,
            session = ?identity.session_id,
            reason = "no_map_transfer_state",
            reports_handled = false,
            "server zone_file_request_refused"
        );
        return false;
    };
    // Every refusal in this function used to be silent, which made a stranded transfer indistinguishable from a healthy
    // one: a client that stops receiving zone files never reaches `zoneFilesReady`, never enters gameplay scope and is
    // invisible to peers, while transport, acknowledgements and counters all continue to look correct. The whole admission
    // snapshot is logged with the reason and with whether the caller reports the packet as handled, because a refusal that
    // reports "handled" also suppresses the client's own retry, and those two facts together are what make the stall
    // permanent. Requests are a handful per session, so this is not a hot path.
    let log_refusal = |reason: &'static str, reports_handled: bool| {
        tracing::info!(
            player_id = identity.player_id,
            session = ?identity.session_id,
            requested_index = index,
            next_index,
            pending_index = ?pending_index,
            deferred_successor_index = ?deferred_successor_index,
            has_active_transfer,
            active_file_index = ?active_file_index,
            active_payload_assigned,
            file_count,
            zone_matches = zone_id == map.zone_id,
            map_sha_matches = map_sha256 == map.sha256,
            reason,
            reports_handled,
            "server zone_file_request_refused"
        );
    };
    if !has_player_identity
        || !joined
        || zone_id != map.zone_id
        || file_count != (1 + map.lvz_files.len()) as u16
        || map_sha256 != map.sha256
        || index >= file_count
    {
        log_refusal("admission_mismatch", false);
        return false;
    }
    if pending_index == Some(index) || deferred_successor_index == Some(index) {
        log_refusal("already_delivered_or_already_pending", true);
        return true;
    }
    // A client that re-requests a file the server already delivered could not use it: the completed-file packet failed to
    // decode or failed an integrity check on the client, and the client's request queue used to walk forward exactly once,
    // so the file was never asked for again. Refusing that re-request while reporting it as handled was what made a strand
    // permanent - no requests, no refusals, `zoneFilesReady` false forever, and the client invisible to peers. Re-serve
    // from that index instead. Bounded, because an asset that is genuinely unusable must not drive this loop forever.
    let redrive = index < next_index;
    if redrive {
        const MAP_TRANSFER_REDIVE_BUDGET: u8 = 4;
        if redrive_count >= MAP_TRANSFER_REDIVE_BUDGET {
            log_refusal("redrive_budget_exhausted", true);
            return true;
        }
        let Some(client) = state
            .clients
            .get_mut(&identity.player_id)
            .and_then(|client| client.map_transfer.as_mut())
        else {
            return false;
        };
        client.redrive_count = client.redrive_count.saturating_add(1);
        client.next_index = index;
        client.held_files.remove(&index);
        client.pending_index = None;
        client.deferred_successor_index = None;
        client.active = None;
        tracing::info!(
            player_id = identity.player_id,
            session = ?identity.session_id,
            file_index = index,
            redrive_count = client.redrive_count,
            "server zone_file_request_redrive"
        );
    }
    let exact_successor = state.clients[&identity.player_id]
        .map_transfer
        .as_ref()
        .is_some_and(|transfer| {
            first_missing_file(transfer, next_index.saturating_add(1)) == index
        });
    if !redrive
        && exact_successor
        && pending_index == Some(next_index)
        && has_active_transfer
        && active_file_index == Some(next_index)
        && active_payload_assigned
        && deferred_successor_index.is_none()
    {
        state
            .clients
            .get_mut(&identity.player_id)
            .and_then(|client| client.map_transfer.as_mut())
            .expect("validated map transfer must remain registered")
            .deferred_successor_index = Some(index);
        return true;
    }
    if !redrive && (pending_index.is_some() || has_active_transfer || index != next_index) {
        log_refusal("stale_pending_or_active_or_out_of_order", false);
        return false;
    }
    let Some(packet) = zone_file_download(index, map) else {
        log_refusal("file_payload_unavailable", false);
        return false;
    };
    tracing::info!(
        player_id = identity.player_id,
        session = ?identity.session_id,
        file_index = index,
        file_count,
        "server zone_file_request_accepted"
    );
    let client = state
        .clients
        .get_mut(&identity.player_id)
        .expect("matching map-transfer session must remain registered");
    client
        .map_transfer
        .as_mut()
        .expect("validated transfer must remain active")
        .pending_index = Some(index);
    let started = begin_bounded_map_application_transfer(client, index, packet);
    if !started {
        client
            .map_transfer
            .as_mut()
            .expect("validated transfer must remain active")
            .pending_index = None;
    }
    started
}

fn first_missing_file(transfer: &super::state::MapTransferState, mut index: u16) -> u16 {
    while index < transfer.file_count && transfer.held_files.contains(&index) {
        index += 1;
    }
    index
}

fn log_manifest_resolution(state: &mut ServerState, player_id: u16) {
    let Some(transfer) = state
        .clients
        .get_mut(&player_id)
        .and_then(|c| c.map_transfer.as_mut())
    else {
        return;
    };
    if transfer.summary_logged
        || transfer.next_index != transfer.file_count
        || transfer.active.is_some()
        || transfer.pending_index.is_some()
    {
        return;
    }
    transfer.summary_logged = true;
    tracing::info!(
        player_id,
        zone = transfer.zone_id,
        held_files = transfer.counted_held_files.len(),
        held_bytes = transfer.held_bytes,
        sent_files = transfer.sent_files.len(),
        sent_bytes = transfer.sent_bytes,
        completed_transfers = transfer.completed_transfers,
        elapsed_ms = transfer.started_at.elapsed().as_millis() as u64,
        "server zone_file_manifest_resolved"
    );
}

pub(crate) fn serve_cached_map_check(
    state: &Arc<Mutex<ServerState>>,
    identity: PlayerSessionIdentity,
    request: &[u8],
) -> bool {
    let check = if request.len() == 50
        && request[..2] == [CACHED_MAP_CHECK, crate::zone_wire::CACHED_MAP_CHECK_VERSION]
    {
        let Ok(zone_id) = crate::zone_wire::parse_canonical_fixed_ascii(&request[2..18]) else {
            return false;
        };
        if !crate::zone_wire::valid_zone_id(zone_id) {
            return false;
        }
        let sha256 = request[18..50].try_into().unwrap();
        crate::zone_wire::HeldZoneFile {
            zone_id: zone_id.to_owned(),
            map_sha256: sha256,
            index: 0,
            sha256,
        }
    } else {
        let Ok(check) = crate::zone_wire::HeldZoneFile::parse(request) else {
            return false;
        };
        check
    };
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state.player_session_matches(identity) {
        return false;
    }
    let client = &state.clients[&identity.player_id];
    if client.player_identity.is_none() || !client.joined {
        return false;
    }
    let map = state.zone_map();
    let Some(transfer) = client.map_transfer.as_ref() else {
        return false;
    };
    let (digest, bytes) = if check.index == 0 {
        (map.sha256, map.compressed.len())
    } else if let Some(file) = map.lvz_files.get(usize::from(check.index - 1)) {
        (file.sha256, file.bytes.len())
    } else {
        return false;
    };
    if check.zone_id != map.zone_id
        || check.map_sha256 != map.sha256
        || check.sha256 != digest
        || transfer.zone_id != map.zone_id
        || transfer.map_sha256 != map.sha256
        || transfer.file_count != (1 + map.lvz_files.len()) as u16
    {
        return false;
    }
    // A late duplicate is idempotent. It cannot relabel a completed transfer
    // as held or cancel a live transfer whose fragments remain in flight.
    if check.index < transfer.next_index || transfer.held_files.contains(&check.index) {
        return true;
    }
    if transfer.pending_index == Some(check.index)
        || transfer
            .active
            .as_ref()
            .is_some_and(|a| a.file_index == check.index)
    {
        return false;
    }
    let transfer = state
        .clients
        .get_mut(&identity.player_id)
        .unwrap()
        .map_transfer
        .as_mut()
        .unwrap();
    transfer.held_files.insert(check.index);
    let first_claim = transfer.counted_held_files.insert(check.index);
    if first_claim {
        transfer.held_bytes = transfer.held_bytes.saturating_add(bytes as u64);
    }
    let previous = transfer.next_index;
    if transfer.pending_index.is_none() && transfer.active.is_none() {
        transfer.next_index = first_missing_file(transfer, transfer.next_index);
    }
    let needs_security = previous == 0 && transfer.next_index > 0;
    tracing::info!(player_id = identity.player_id, session = ?identity.session_id, zone = %check.zone_id,
        file_index = check.index, bytes, "server zone_file_held");
    if first_claim {
        state.telemetry.zone_files_held = state.telemetry.zone_files_held.saturating_add(1);
        state.telemetry.zone_bytes_held =
            state.telemetry.zone_bytes_held.saturating_add(bytes as u64);
    }
    if needs_security {
        let door_state = state.server_door_challenge_state;
        enqueue_map_security_challenge(
            state.clients.get_mut(&identity.player_id).unwrap(),
            door_state,
        );
    }
    log_manifest_resolution(&mut state, identity.player_id);
    true
}

/// Refill the bounded file-fragment window after ACKs free slots. The ACK path
/// calls this while holding the server-state lock so reconnect/zone changes
/// cannot retarget the retained payload.
pub(crate) fn flush_pending_file_transfer_locked(
    state: &mut ServerState,
    identity: PlayerSessionIdentity,
) {
    if !state.player_session_matches(identity) {
        return;
    }
    let map = state.zone_map();
    let Some((
        has_player_identity,
        joined,
        zone_id,
        next_index,
        pending_index,
        file_count,
        map_sha256,
    )) = state.clients.get(&identity.player_id).and_then(|client| {
        client.map_transfer.as_ref().map(|transfer| {
            (
                client.player_identity.is_some(),
                client.joined,
                transfer.zone_id,
                transfer.next_index,
                transfer.pending_index,
                transfer.file_count,
                transfer.map_sha256,
            )
        })
    })
    else {
        return;
    };
    let Some(index) = pending_index else {
        return;
    };
    if !has_player_identity
        || !joined
        || zone_id != map.zone_id
        || file_count != (1 + map.lvz_files.len()) as u16
        || map_sha256 != map.sha256
        || index != next_index
        || index >= file_count
    {
        return;
    }
    let door_state = state.server_door_challenge_state;
    let deferred_successor_index = {
        let client = state
            .clients
            .get_mut(&identity.player_id)
            .expect("matching deferred transfer session must remain registered");
        let Some(completed_index) = flush_bounded_map_application_transfer(client) else {
            return;
        };
        if completed_index != index {
            return;
        }
        let transfer = client
            .map_transfer
            .as_mut()
            .expect("validated deferred transfer must remain active");
        transfer.pending_index = None;
        transfer.sent_files.insert(index);
        let bytes = if index == 0 {
            map.compressed.len()
        } else {
            map.lvz_files[usize::from(index - 1)].bytes.len()
        };
        transfer.sent_bytes = transfer.sent_bytes.saturating_add(bytes as u64);
        transfer.completed_transfers = transfer.completed_transfers.saturating_add(1);
        transfer.next_index = first_missing_file(transfer, transfer.next_index + 1);
        let deferred = transfer.deferred_successor_index.take();
        if index == 0 {
            enqueue_map_security_challenge(client, door_state);
        }
        deferred
    };

    let completed_bytes = if index == 0 {
        map.compressed.len()
    } else {
        map.lvz_files[usize::from(index - 1)].bytes.len()
    };
    state.telemetry.zone_files_sent = state.telemetry.zone_files_sent.saturating_add(1);
    state.telemetry.zone_bytes_sent = state
        .telemetry
        .zone_bytes_sent
        .saturating_add(completed_bytes as u64);
    log_manifest_resolution(state, identity.player_id);
    let Some(successor_index) = deferred_successor_index else {
        return;
    };
    let successor_packet = {
        let map = state.zone_map();
        let expected_file_count = (1 + map.lvz_files.len()) as u16;
        if successor_index >= expected_file_count {
            return;
        }
        let Some(packet) = zone_file_download(successor_index, map) else {
            return;
        };
        packet
    };
    let client = state
        .clients
        .get_mut(&identity.player_id)
        .expect("successor map transfer session must remain registered");
    let transfer = client
        .map_transfer
        .as_mut()
        .expect("successor map transfer state must remain active");
    if transfer.next_index != successor_index
        || transfer.pending_index.is_some()
        || transfer.active.is_some()
    {
        return;
    }
    transfer.pending_index = Some(successor_index);
    let started = begin_bounded_map_application_transfer(client, successor_index, successor_packet);
    if !started {
        client
            .map_transfer
            .as_mut()
            .expect("failed successor transfer must remain registered")
            .pending_index = None;
    }
}

fn enqueue_map_security_challenge(
    client: &mut super::state::ClientHandle,
    door_state: super::doors::ClientDoorState,
) {
    let (challenge, door_state) = security_challenge_for_door_state(door_state);
    client.door_state = Some(door_state);
    send_reliable_payload(client, &challenge);
}

pub(crate) fn serve_news_request(state: &Arc<Mutex<ServerState>>, player_id: u16) {
    let map = {
        let state = state.lock().expect("server state should not be poisoned");
        state.zone_map()
    };
    send_reliable_to(
        state,
        player_id,
        server_file_transfer(map.news_filename.as_ref(), map.news_bytes.as_ref()),
    );
}

pub(crate) fn password_response() -> Vec<u8> {
    let mut payload = vec![0; 20];
    payload[0] = 0x0a;
    payload[1] = 0x00;
    payload.extend_from_slice(crate::zone_wire::HELD_ZONE_FILES_LOGIN_TRAILER);
    payload
}

pub(crate) fn login_failure_message(reason: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + reason.len() + 1);
    payload.push(LOGIN_FAILURE_MESSAGE);
    payload.extend(reason.as_bytes());
    payload.push(0);
    payload
}
