use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::super::reliable::{broadcast_reliable_to_joined, send_reliable_payload};
use super::super::state::ServerState;
use super::super::wire::read_capped_c_string;

pub(crate) const CHAT_MESSAGE_REQUEST: u8 = 0x06;
pub(crate) const CHAT_PACKET: u8 = 0x07;
pub(crate) const CHAT_TYPE_PUBLIC: u8 = 2;
pub(crate) const CHAT_TYPE_TEAM: u8 = 3;
pub(crate) const CHAT_TYPE_PRIVATE: u8 = 5;
const CHAT_RATE_LIMIT: Duration = Duration::from_millis(250);
// Four messages of credit accommodate a small reliable recovery batch while
// retaining the existing sustained limit of four messages per second.
const CHAT_BURST_CAPACITY: u32 = 4;

pub(crate) fn relay_chat(state: &Arc<Mutex<ServerState>>, sender_id: u16, request: &[u8]) {
    relay_chat_at(state, sender_id, request, Instant::now());
}

pub(crate) fn relay_chat_at(
    state: &Arc<Mutex<ServerState>>,
    sender_id: u16,
    request: &[u8],
    now: Instant,
) {
    let Some(packet) = chat_packet_from_request(sender_id, request) else {
        return;
    };
    if !allow_chat_relay(state, sender_id, now) {
        return;
    }

    let chat_type = request[1];
    let target = u16::from_le_bytes([request[3], request[4]]);
    match chat_type {
        CHAT_TYPE_PUBLIC => broadcast_reliable_to_joined(state, packet),
        CHAT_TYPE_TEAM => relay_team_chat(state, sender_id, &packet),
        CHAT_TYPE_PRIVATE => relay_private_chat(state, sender_id, target, &packet),
        _ => broadcast_reliable_to_joined(state, packet),
    }
}

pub(crate) fn chat_packet_from_request(sender_id: u16, request: &[u8]) -> Option<Vec<u8>> {
    if request.len() < 6 || request[0] != CHAT_MESSAGE_REQUEST {
        return None;
    }
    let message = read_capped_c_string(&request[5..], 200);
    if message.is_empty() {
        return None;
    }
    let mut packet = Vec::with_capacity(6 + message.len());
    packet.push(CHAT_PACKET);
    packet.push(request[1]);
    packet.push(request[2]);
    packet.extend(sender_id.to_le_bytes());
    packet.extend(message.as_bytes());
    packet.push(0);
    Some(packet)
}

fn relay_team_chat(state: &Arc<Mutex<ServerState>>, sender_id: u16, packet: &[u8]) {
    let mut state = state.lock().expect("server state should not be poisoned");
    let Some(sender) = state.clients.get(&sender_id) else {
        return;
    };
    if !sender.joined {
        return;
    }
    let frequency = sender.player.frequency;
    for client in state
        .clients
        .values_mut()
        .filter(|client| client.joined && client.player.frequency == frequency)
    {
        send_reliable_payload(client, packet);
    }
}

fn relay_private_chat(
    state: &Arc<Mutex<ServerState>>,
    sender_id: u16,
    target_id: u16,
    packet: &[u8],
) {
    let mut state = state.lock().expect("server state should not be poisoned");
    if !state
        .clients
        .get(&sender_id)
        .is_some_and(|client| client.joined)
    {
        return;
    }

    for player_id in [sender_id, target_id] {
        if let Some(client) = state.clients.get_mut(&player_id) {
            if client.joined {
                send_reliable_payload(client, packet);
            }
        }
        if target_id == sender_id {
            break;
        }
    }
}

fn allow_chat_relay(state: &Arc<Mutex<ServerState>>, sender_id: u16, now: Instant) -> bool {
    let mut state = state.lock().expect("server state should not be poisoned");
    let Some(client) = state.clients.get_mut(&sender_id) else {
        return false;
    };
    if !client.joined {
        return false;
    }
    let debt_until = client.chat_rate_debt_until.unwrap_or(now).max(now);
    let next_debt_until = debt_until + CHAT_RATE_LIMIT;
    if next_debt_until.saturating_duration_since(now) > CHAT_RATE_LIMIT * CHAT_BURST_CAPACITY {
        return false;
    }
    client.chat_rate_debt_until = Some(next_debt_until);
    true
}
