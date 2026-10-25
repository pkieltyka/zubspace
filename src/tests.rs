use super::*;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::collections::VecDeque;
use std::io::Write;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MemoryPacketTransport {
    inbound: VecDeque<Vec<u8>>,
    inbound_lanes: VecDeque<crate::transport::PacketTransportLane>,
    last_received_lane: crate::transport::PacketTransportLane,
    sent: Vec<Vec<u8>>,
    send_failures_remaining: usize,
}

impl MemoryPacketTransport {
    fn with_inbound(packets: Vec<Vec<u8>>) -> Self {
        Self {
            inbound: packets.into(),
            inbound_lanes: VecDeque::new(),
            last_received_lane: crate::transport::PacketTransportLane::Unknown,
            sent: Vec::new(),
            send_failures_remaining: 0,
        }
    }

    fn with_laned_inbound(packets: Vec<(Vec<u8>, crate::transport::PacketTransportLane)>) -> Self {
        let (inbound, inbound_lanes): (VecDeque<_>, VecDeque<_>) = packets.into_iter().unzip();
        Self {
            inbound,
            inbound_lanes,
            last_received_lane: crate::transport::PacketTransportLane::Unknown,
            sent: Vec::new(),
            send_failures_remaining: 0,
        }
    }
}

impl PacketTransport for MemoryPacketTransport {
    fn send_packet(&mut self, packet: &[u8]) -> Result<(), String> {
        if self.send_failures_remaining > 0 {
            self.send_failures_remaining -= 1;
            return Err("synthetic transport send failure".to_string());
        }
        self.sent.push(packet.to_vec());
        Ok(())
    }

    fn recv_packet(&mut self) -> Result<Option<Vec<u8>>, String> {
        let packet = self.inbound.pop_front();
        self.last_received_lane = if packet.is_some() {
            self.inbound_lanes.pop_front().unwrap_or_default()
        } else {
            crate::transport::PacketTransportLane::Unknown
        };
        Ok(packet)
    }

    fn last_received_lane(&self) -> crate::transport::PacketTransportLane {
        self.last_received_lane
    }
}

fn put_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_i16(data: &mut [u8], offset: usize, value: i16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_i32(data: &mut [u8], offset: usize, value: i32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn pack_continuum_spawn_region(x: i16, y: i16, radius: u16) -> u32 {
    ((x as i32 as u32) & 0x03ff)
        | (((y as i32 as u32) & 0x03ff) << 10)
        | (((radius as u32) & 0x01ff) << 20)
}

fn empty_test_map() -> ArenaMap {
    let width = MAP_TILES as usize;
    let height = MAP_TILES as usize;
    let tiles = vec![0; width * height];
    ArenaMap {
        name: "empty".to_string(),
        width,
        height,
        radar_points: ArenaMap::build_radar_points(&tiles, width, height),
        radar_index: std::sync::OnceLock::new(),
        embedded_tileset_pixel_bytes: 0,
        tiles: tiles.into(),
        objects: Vec::new(),
        doors: Vec::new(),
        door_seed: u8::MAX,
        door_rng_seed: 1,
    }
}

fn push_test_c_string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
}

fn minimal_test_bm2() -> Vec<u8> {
    let mut data = vec![0_u8; 58];
    data[..2].copy_from_slice(b"BM");
    data[2..6].copy_from_slice(&58_u32.to_le_bytes());
    data[10..14].copy_from_slice(&54_u32.to_le_bytes());
    data[14..18].copy_from_slice(&40_u32.to_le_bytes());
    data[18..22].copy_from_slice(&1_i32.to_le_bytes());
    data[22..26].copy_from_slice(&1_i32.to_le_bytes());
    data[26..28].copy_from_slice(&1_u16.to_le_bytes());
    data[28..30].copy_from_slice(&24_u16.to_le_bytes());
    data
}

fn push_lvz_section(
    archive: &mut Vec<u8>,
    filename: &str,
    timestamp: u32,
    data: &[u8],
    compressed: Option<&[u8]>,
) {
    let stored = compressed.unwrap_or(data);
    push_u32(archive, LvzArchive::MAGIC_CONT);
    push_u32(archive, data.len() as u32);
    push_u32(archive, timestamp);
    push_u32(archive, stored.len() as u32);
    push_test_c_string(archive, filename);
    archive.extend_from_slice(stored);
}

fn sample_continuum_settings_packet() -> Vec<u8> {
    let mut data = vec![0; CONTINUUM_ARENA_SETTINGS_SIZE];
    put_u32(
        &mut data,
        0,
        3 | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 19),
    );

    let ship = CONTINUUM_SHIP_SETTINGS_OFFSET;
    put_u32(&mut data, ship, 1200);
    put_u32(&mut data, ship + 4, 600);
    put_i16(&mut data, ship + 8, 40);
    put_i16(&mut data, ship + 10, 5600);
    put_u16(&mut data, ship + 12, 30);
    put_u16(&mut data, ship + 14, 45);
    put_u16(&mut data, ship + 16, 120);
    put_u16(&mut data, ship + 18, 15);
    put_u16(&mut data, ship + 20, 130);
    put_u16(&mut data, ship + 22, 20);
    put_u16(&mut data, ship + 24, 7600);
    put_u16(&mut data, ship + 26, 4300);
    put_u16(&mut data, ship + 28, 2 | (1 << 2) | (14 << 3));
    put_u16(&mut data, ship + 30, 2220);
    put_u16(&mut data, ship + 32, 700);
    put_u16(&mut data, ship + 34, 450);
    put_u16(&mut data, ship + 36, 550);
    put_u16(&mut data, ship + 38, 400);
    put_u16(&mut data, ship + 40, 400);
    put_u16(&mut data, ship + 42, 52);
    put_u16(&mut data, ship + 44, 5200);
    put_u16(&mut data, ship + 46, 1800);
    put_u16(&mut data, ship + 48, 1500);
    put_u16(&mut data, ship + 50, 200);
    put_u16(&mut data, ship + 52, 36);
    put_u16(&mut data, ship + 54, 3200);
    put_u16(&mut data, ship + 56, 1200);
    put_u16(&mut data, ship + 58, 1200);
    put_u16(&mut data, ship + 60, 40);
    put_u16(&mut data, ship + 62, 4);
    put_u16(&mut data, ship + 64, 400);
    put_u16(&mut data, ship + 66, 100);
    put_u16(&mut data, ship + 68, 100);
    put_u16(&mut data, ship + 70, 1800);
    put_u16(&mut data, ship + 72, 160);
    put_u16(&mut data, ship + 74, 6200);
    put_i16(&mut data, ship + 76, 8);
    put_i16(&mut data, ship + 78, 120);
    put_u16(&mut data, ship + 80, 8);
    put_u16(&mut data, ship + 82, 12);
    put_u16(&mut data, ship + 84, 65);
    put_u16(&mut data, ship + 86, 70);
    put_u16(&mut data, ship + 88, 250);
    put_u16(&mut data, ship + 90, 7);
    put_u16(&mut data, ship + 92, 5000);
    put_u16(&mut data, ship + 94, 12);
    put_u16(&mut data, ship + 96, 9);
    put_u16(&mut data, ship + 98, 450);
    put_u16(&mut data, ship + 100, 1500);
    put_u16(&mut data, ship + 102, 28);
    put_u16(&mut data, ship + 104, 9000);
    data[ship + 106] = 2;
    data[ship + 107] = 12;
    data[ship + 108] = 8;
    data[ship + 109] = 5;
    data[ship + 110] = 6;
    data[ship + 111] = 7;
    data[ship + 112] = 2;
    data[ship + 113] = 4;
    data[ship + 114] = 3;
    data[ship + 115] = 5;
    data[ship + 116] = 1;
    data[ship + 117] = 2;
    data[ship + 118] = 3;
    data[ship + 119] = 1;
    data[ship + 120] = 1;
    data[ship + 121] = 2;
    data[ship + 122] = 1;
    data[ship + 123] = 2;
    let capability = 13
        | (3 << 5)
        | (2 << 10)
        | (1 << 12)
        | (2 << 14)
        | (1 << 16)
        | (1 << 18)
        | (3 << 20)
        | (1 << 22)
        | (3 << 24)
        | (1 << 26)
        | (1 << 27)
        | (1 << 28);
    put_u32(&mut data, ship + 124, capability);

    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET, 90_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 4, 260_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 8, 125);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 12, 240);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 16, 1200);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 28, 1000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 32, 5200);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 36, 4500);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 40, 70_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 44, 10_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 48, 12_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 52, 500);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 56, 92);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 60, 7000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 64, 20_000);
    put_i32(&mut data, CONTINUUM_GLOBAL_INTS_OFFSET + 76, 4300);

    put_u32(
        &mut data,
        CONTINUUM_SPAWN_SETTINGS_OFFSET,
        pack_continuum_spawn_region(128, -12, 21),
    );
    put_u32(
        &mut data,
        CONTINUUM_SPAWN_SETTINGS_OFFSET + 4,
        pack_continuum_spawn_region(-128, 256, 0),
    );

    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 2, 25);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 8, 1500);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 6, 72);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 12, 150);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 14, 400);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 16, 3);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 18, 2);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 20, 16);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 22, 48);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 30, 50);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 32, 160);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 40, 1000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 42, 200);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 44, 800);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 46, 20);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 48, 3000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 50, 1000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 52, 32000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 54, 120);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 56, 640);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 58, -1);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 64, 5);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 66, 1200);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 68, 768);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 70, 300);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 72, 600);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 76, 2000);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 80, 750);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 82, 500);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 88, 800);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 90, 750);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 92, 35);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 94, 100);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 100, -7);
    put_i16(&mut data, CONTINUUM_SHORT_SETTINGS_OFFSET + 102, -40);

    data[CONTINUUM_BYTE_SETTINGS_OFFSET] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 1] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 2] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 3] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 4] = 3;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 7] = 4;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 8] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 9] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 11] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 13] = 3;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 16] = 2;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 18] = 4;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 19] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 20] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 21] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 22] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 23] = 1;
    data[CONTINUUM_BYTE_SETTINGS_OFFSET + 24] = 50;

    for index in 0..CONTINUUM_PRIZE_WEIGHT_COUNT {
        data[CONTINUUM_PRIZE_WEIGHTS_OFFSET + index] = index as u8 + 1;
    }

    data
}

fn sample_player_enter_packet(player_id: u16, name: &[u8]) -> Vec<u8> {
    let mut packet = vec![0; PlayerRosterEntry::PACKET_LEN];
    packet[0] = PlayerRosterEntry::TYPE;
    packet[1] = 3;
    packet[2] = 4;
    packet[3..3 + name.len()].copy_from_slice(name);
    packet[23..28].copy_from_slice(b"squad");
    put_u32(&mut packet, 43, 1000);
    put_u32(&mut packet, 47, 2000);
    put_u16(&mut packet, 51, player_id);
    put_u16(&mut packet, 53, 7);
    put_u16(&mut packet, 55, 8);
    put_u16(&mut packet, 57, 9);
    put_u16(&mut packet, 59, 0xffff);
    put_u16(&mut packet, 61, 2);
    packet[63] = 1;
    packet
}

fn reliable_small_chunk_packets(start_id: u32, payload: &[u8]) -> Vec<Vec<u8>> {
    let chunks: Vec<&[u8]> = payload
        .chunks(ReliableSequencer::MAX_SMALL_CHUNK_PAYLOAD_SIZE)
        .collect();

    chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            let mut chunk_payload = Vec::with_capacity(chunk.len() + 2);
            push_u8(&mut chunk_payload, PacketChunker::CORE_BYTE);
            push_u8(
                &mut chunk_payload,
                if index + 1 == chunks.len() {
                    PacketChunker::SMALL_TAIL_TYPE
                } else {
                    PacketChunker::SMALL_BODY_TYPE
                },
            );
            chunk_payload.extend_from_slice(chunk);
            ReliableSequencer::encode_reliable_message(start_id + index as u32, &chunk_payload)
        })
        .collect()
}

fn pack_lvl_tile(x: u16, y: u16, id: u8) -> [u8; 4] {
    ((x as u32 & 0x0fff) | ((y as u32 & 0x0fff) << 12) | ((id as u32) << 24)).to_le_bytes()
}

mod assets_visuals;
mod attach_lvz_reliable_chat;
mod combat_gameplay;
mod combat_remote_weapons;
mod maps_doors_browser_registration;
mod network_client;
mod network_world;
mod prizes_spawn;
mod projectile_presentation;
mod protocol_packets;
mod protocol_session;
mod settings_security;
