/// Debug/QA-only correlation context for joining the bounded browser and
/// server trace rings. This is diagnostics metadata, never gameplay authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QaTraceContext {
    pub(crate) server_session_id: u64,
    pub(crate) arena_stream_id: u64,
    pub(crate) player_id: u16,
    pub(crate) bundle_id: [u8; 16],
    pub(crate) upload_token: [u8; 32],
    pub(crate) server_binary_sha256: [u8; 32],
    pub(crate) server_binary_bytes: u64,
}

impl QaTraceContext {
    /// Private zubspace server-to-client packet. It rides the existing ordered
    /// reliable lane as part of join bootstrap; it is not a fourth transport.
    pub(crate) const TYPE: u8 = 0x6c;
    pub(crate) const VERSION: u8 = 2;
    pub(crate) const PACKET_LEN: usize = 1 + 1 + 8 + 8 + 2 + 16 + 32 + 32 + 8;

    pub(crate) fn encode(self) -> Vec<u8> {
        let mut packet = Vec::with_capacity(Self::PACKET_LEN);
        packet.push(Self::TYPE);
        packet.push(Self::VERSION);
        packet.extend_from_slice(&self.server_session_id.to_le_bytes());
        packet.extend_from_slice(&self.arena_stream_id.to_le_bytes());
        packet.extend_from_slice(&self.player_id.to_le_bytes());
        packet.extend_from_slice(&self.bundle_id);
        packet.extend_from_slice(&self.upload_token);
        packet.extend_from_slice(&self.server_binary_sha256);
        packet.extend_from_slice(&self.server_binary_bytes.to_le_bytes());
        packet
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN || packet.first().copied() != Some(Self::TYPE) {
            return Err("invalid QA trace context packet length or type".to_string());
        }
        if packet[1] != Self::VERSION {
            return Err(format!(
                "unsupported QA trace context version {}",
                packet[1]
            ));
        }
        let mut bundle_id = [0; 16];
        bundle_id.copy_from_slice(&packet[20..36]);
        let mut upload_token = [0; 32];
        upload_token.copy_from_slice(&packet[36..68]);
        let mut server_binary_sha256 = [0; 32];
        server_binary_sha256.copy_from_slice(&packet[68..100]);
        let server_binary_bytes = u64::from_le_bytes(packet[100..108].try_into().unwrap());
        if (server_binary_bytes == 0) != (server_binary_sha256 == [0; 32]) {
            return Err("inconsistent QA server binary identity".to_string());
        }
        Ok(Self {
            server_session_id: u64::from_le_bytes(packet[2..10].try_into().unwrap()),
            arena_stream_id: u64::from_le_bytes(packet[10..18].try_into().unwrap()),
            player_id: u16::from_le_bytes(packet[18..20].try_into().unwrap()),
            bundle_id,
            upload_token,
            server_binary_sha256,
            server_binary_bytes,
        })
    }

    pub(crate) fn bundle_id_hex(self) -> String {
        hex(&self.bundle_id)
    }

    pub(crate) fn upload_token_hex(self) -> String {
        hex(&self.upload_token)
    }

    pub(crate) fn server_binary_sha256_hex(self) -> String {
        hex(&self.server_binary_sha256)
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qa_trace_context_round_trips_exact_scope_and_credentials() {
        let context = QaTraceContext {
            server_session_id: 0x0102_0304_0506_0708,
            arena_stream_id: 0x1112_1314_1516_1718,
            player_id: 0x2122,
            bundle_id: [0x34; 16],
            upload_token: [0x56; 32],
            server_binary_sha256: [0x78; 32],
            server_binary_bytes: 123_456,
        };
        let packet = context.encode();
        assert_eq!(packet.len(), QaTraceContext::PACKET_LEN);
        assert_eq!(QaTraceContext::parse(&packet).unwrap(), context);
        assert_eq!(context.bundle_id_hex(), "34".repeat(16));
        assert_eq!(context.upload_token_hex(), "56".repeat(32));
        assert_eq!(context.server_binary_sha256_hex(), "78".repeat(32));
    }

    #[test]
    fn qa_trace_context_rejects_wrong_version_and_trailing_bytes() {
        let mut packet = QaTraceContext {
            server_session_id: 1,
            arena_stream_id: 2,
            player_id: 3,
            bundle_id: [4; 16],
            upload_token: [5; 32],
            server_binary_sha256: [0; 32],
            server_binary_bytes: 0,
        }
        .encode();
        packet[1] = 1;
        assert!(QaTraceContext::parse(&packet).is_err());
        packet[1] = QaTraceContext::VERSION;
        packet.push(0);
        assert!(QaTraceContext::parse(&packet).is_err());
    }

    #[test]
    fn qa_trace_context_rejects_partial_binary_identity() {
        let context = QaTraceContext {
            server_session_id: 1,
            arena_stream_id: 2,
            player_id: 3,
            bundle_id: [4; 16],
            upload_token: [5; 32],
            server_binary_sha256: [0; 32],
            server_binary_bytes: 0,
        };
        assert_eq!(QaTraceContext::parse(&context.encode()).unwrap(), context);
        assert!(
            QaTraceContext::parse(
                &QaTraceContext {
                    server_binary_bytes: 10,
                    ..context
                }
                .encode()
            )
            .is_err()
        );
        assert!(
            QaTraceContext::parse(
                &QaTraceContext {
                    server_binary_sha256: [7; 32],
                    ..context
                }
                .encode()
            )
            .is_err()
        );
    }
}
