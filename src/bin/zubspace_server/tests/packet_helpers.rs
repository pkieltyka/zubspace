use super::*;
use std::sync::mpsc::Receiver;

pub(super) struct TestPacketClient {
    pub(super) player_id: u16,
    pub(super) rx: Receiver<Vec<u8>>,
    joined: bool,
    peer: String,
}

impl TestPacketClient {
    pub(super) fn send_packet(&mut self, state: &Arc<Mutex<ServerState>>, packet: Vec<u8>) {
        crate::connection::handle_client_packet(
            state,
            self.player_id,
            packet,
            &self.peer,
            &mut self.joined,
        );
    }

    pub(super) fn send_reliable(
        &mut self,
        state: &Arc<Mutex<ServerState>>,
        id: u32,
        payload: &[u8],
    ) {
        self.send_packet(state, reliable(id, payload));
    }

    pub(super) fn read_reliable_payload(&self) -> Vec<u8> {
        let mut small_chunks = Vec::new();
        loop {
            let packet = self
                .rx
                .recv_timeout(Duration::from_secs(1))
                .expect("expected server packet");
            if is_reliable_ack(&packet) {
                continue;
            }
            let Some(payload) = reliable_payload(&packet) else {
                continue;
            };
            if payload.first().copied() == Some(crate::qa_trace_protocol::QaTraceContext::TYPE) {
                crate::qa_trace_protocol::QaTraceContext::parse(payload)
                    .expect("debug QA context must remain well formed");
                continue;
            }
            if payload.len() >= 2 && payload[0] == CORE_BYTE && payload[1] == SMALL_CHUNK_BODY {
                small_chunks.extend_from_slice(&payload[2..]);
                continue;
            }
            if payload.len() >= 2 && payload[0] == CORE_BYTE && payload[1] == SMALL_CHUNK_TAIL {
                small_chunks.extend_from_slice(&payload[2..]);
                return small_chunks;
            }
            return payload.to_vec();
        }
    }

    /// Read one current map/LVZ application transfer through its production
    /// bounded fragment window. ACK each exact wrapper so the server may
    /// replenish the window, and return the byte-identical legacy logical
    /// packet that the real client parser receives after reassembly.
    pub(super) fn read_file_transfer(&self, state: &Arc<Mutex<ServerState>>) -> Vec<u8> {
        let identity = state
            .lock()
            .expect("server state")
            .active_player_session_identity(self.player_id)
            .expect("file-transfer test client must retain its session");
        let mut assembler = crate::file_transfer_chunk::ServerFileTransferAssembler::default();
        loop {
            let packet = self
                .rx
                .recv_timeout(Duration::from_secs(1))
                .expect("expected server file fragment");
            if is_reliable_ack(&packet) {
                continue;
            }
            let Some(payload) = reliable_payload(&packet) else {
                continue;
            };
            if payload.first().copied() == Some(crate::qa_trace_protocol::QaTraceContext::TYPE) {
                crate::qa_trace_protocol::QaTraceContext::parse(payload)
                    .expect("debug QA context must remain well formed");
                continue;
            }
            let chunk = crate::file_transfer_chunk::ServerFileTransferChunk::parse(payload)
                .expect("expected bounded server file fragment");
            let complete = assembler.push(chunk).expect("valid contiguous file stream");
            let reliable_id = u32::from_le_bytes(
                packet[2..6]
                    .try_into()
                    .expect("validated reliable wrapper id"),
            );
            assert!(crate::reliable::receive_reliable_acks(
                state,
                identity,
                &reliable_ack(reliable_id),
            ));
            if let Some(complete) = complete {
                return complete;
            }
        }
    }

    pub(super) fn disconnect(self, state: &Arc<Mutex<ServerState>>) {
        disconnect_player(state, self.player_id);
    }
}

pub(super) fn set_login_identity(login: &mut [u8], identity: u64) {
    let low = identity as u32;
    let high = (identity >> 32) as u32;
    login[LOGIN_ID_LOW_OFFSET..LOGIN_ID_LOW_OFFSET + 4].copy_from_slice(&low.to_le_bytes());
    login[LOGIN_ID_HIGH_OFFSET..LOGIN_ID_HIGH_OFFSET + 4].copy_from_slice(&high.to_le_bytes());
}

pub(super) fn test_identity_for_name(name: &str) -> u64 {
    let mut hash = 100_000_000_000_u64;
    for byte in name.bytes() {
        hash = (hash * 131 + byte as u64) % 900_000_000_000;
    }
    100_000_000_000 + hash
}

pub(super) fn connect_login_and_join(
    state: &Arc<Mutex<ServerState>>,
    name: &str,
    ship: u8,
) -> TestPacketClient {
    connect_login_and_join_with_identity(state, name, ship, test_identity_for_name(name))
}

pub(super) fn connect_login_and_join_with_identity(
    state: &Arc<Mutex<ServerState>>,
    name: &str,
    ship: u8,
    identity: u64,
) -> TestPacketClient {
    let mut client = connect_and_login(state, name, identity);
    let password = client.read_reliable_payload();
    assert_eq!(password.first().copied(), Some(0x0a));
    assert_eq!(password.get(1).copied(), Some(0x00));

    client.send_reliable(state, 1, &[ARENA_LOGIN, ship]);
    client
}

pub(super) fn arena_login_payload(ship: u8, arena_name: &str) -> Vec<u8> {
    let mut payload = vec![0; 27];
    payload[0] = ARENA_LOGIN;
    payload[1] = ship;
    payload[8..10].copy_from_slice(&0xffff_u16.to_le_bytes());
    let arena = arena_name.as_bytes();
    let len = arena.len().min(16);
    payload[10..10 + len].copy_from_slice(&arena[..len]);
    payload[26] = 1;
    payload
}

pub(super) fn connect_and_login(
    state: &Arc<Mutex<ServerState>>,
    name: &str,
    identity: u64,
) -> TestPacketClient {
    let (tx, rx) = mpsc::channel();
    let player_id = register_connection(state, tx);
    let mut client = TestPacketClient {
        player_id,
        rx,
        joined: false,
        peer: format!("test-peer-{player_id}"),
    };

    let mut login = vec![0; 165];
    login[0] = PASSWORD_LOGIN_CONTINUUM;
    login[2..2 + name.len()].copy_from_slice(name.as_bytes());
    set_login_identity(&mut login, identity);
    client.send_reliable(state, 0, &login);
    client
}
