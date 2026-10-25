use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::AcceptedPositionRelay;
use crate::state::ServerState;

const BATCHED_LARGE_POSITION: u8 = 0x3a;
const BATCHED_LARGE_ENTRY_LEN: usize = 11;
const MAX_BATCHED_POSITION_ENTRIES: usize = 40;

pub(crate) fn send_position_relays_batched(
    state: &Arc<Mutex<ServerState>>,
    relays: Vec<AcceptedPositionRelay>,
) {
    if relays.is_empty() {
        return;
    }
    let mut by_recipient: BTreeMap<u16, Vec<Vec<u8>>> = BTreeMap::new();
    for relay in relays {
        if relay.public_relay_suppressed {
            continue;
        }
        for recipient_id in relay.recipient_ids {
            by_recipient
                .entry(recipient_id)
                .or_default()
                .push(relay.position.clone());
        }
    }
    let deliveries = {
        let state = state.lock().expect("server state should not be poisoned");
        by_recipient
            .into_iter()
            .filter_map(|(id, positions)| {
                state
                    .clients
                    .get(&id)
                    .map(|client| (client.tx.clone(), positions))
            })
            .collect::<Vec<_>>()
    };
    for (tx, positions) in deliveries {
        for packet in batched_position_packets_for_recipient(&positions) {
            let _ = tx.send(packet);
        }
    }
}

fn batched_position_packets_for_recipient(positions: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    let mut batch = Vec::new();
    for position in positions {
        if let Some(entry) = batched_large_position_entry(position) {
            batch.push(entry);
            if batch.len() >= MAX_BATCHED_POSITION_ENTRIES {
                flush_batched_large_positions(&mut packets, &mut batch);
            }
        } else {
            flush_batched_large_positions(&mut packets, &mut batch);
            packets.push(position.clone());
        }
    }
    flush_batched_large_positions(&mut packets, &mut batch);
    packets
}

fn flush_batched_large_positions(packets: &mut Vec<Vec<u8>>, batch: &mut Vec<[u8; 11]>) {
    if batch.is_empty() {
        return;
    }
    let mut packet = Vec::with_capacity(1 + batch.len() * BATCHED_LARGE_ENTRY_LEN);
    packet.push(BATCHED_LARGE_POSITION);
    for entry in batch.drain(..) {
        packet.extend(entry);
    }
    packets.push(packet);
}

fn batched_large_position_entry(position: &[u8]) -> Option<[u8; 11]> {
    if position.len() < 21
        || position[0] != crate::arena::LARGE_POSITION_PACKET
        || u16_from(position, 19)? != 0
    {
        return None;
    }
    let player_id = u16_from(position, 8)?;
    let x = u16_from(position, 4)?;
    let y = u16_from(position, 15)?;
    if player_id > 0x03ff || x > 0x3fff || y > 0x3fff {
        return None;
    }

    let direction = position[1] & 0x3f;
    let timestamp = u16_from(position, 2)? & 0x03ff;
    let togglables = position[13] & 0x3f;
    let velocity_x = i16_from(position, 10)? as i32;
    let velocity_y = i16_from(position, 6)? as i32;
    if !(-8192..=8191).contains(&velocity_y) {
        return None;
    }

    let velocity_x_low = velocity_x.rem_euclid(16);
    let velocity_x_high = (velocity_x - velocity_x_low) / 16;
    let velocity_x_base = velocity_x_high.rem_euclid(4);
    let velocity_x_multiplier = (velocity_x_high - velocity_x_base) / 4;
    if !(i8::MIN as i32..=i8::MAX as i32).contains(&velocity_x_multiplier) {
        return None;
    }

    let packed_player = ((togglables as u16) << 10) | player_id;
    let packed_heading = ((direction as u16) << 10) | timestamp;
    let packed_position = ((velocity_x_low as u32) << 28) | ((y as u32) << 14) | x as u32;
    let packed_velocity = ((velocity_x_base as u16) << 14) | ((velocity_y as u16) & 0x3fff);

    let mut entry = [0; 11];
    entry[0..2].copy_from_slice(&packed_player.to_le_bytes());
    entry[2..4].copy_from_slice(&packed_heading.to_le_bytes());
    entry[4..8].copy_from_slice(&packed_position.to_le_bytes());
    entry[8..10].copy_from_slice(&packed_velocity.to_le_bytes());
    entry[10] = velocity_x_multiplier as i8 as u8;
    Some(entry)
}

fn u16_from(packet: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        packet.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn i16_from(packet: &[u8], offset: usize) -> Option<i16> {
    Some(i16::from_le_bytes(
        packet.get(offset..offset + 2)?.try_into().ok()?,
    ))
}
