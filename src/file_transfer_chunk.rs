/// Zubspace's bounded, interleavable server file-transfer fragment.
///
/// Continuum's generic `0x00/0x08..0x09` small-chunk stream requires every
/// fragment of one logical payload to remain contiguous. A map or LVZ can be
/// hundreds of reliable wrappers long, which lets one lost asset wrapper
/// head-of-line block later roster and lifecycle controls. Current zubspace
/// peers therefore carry map/LVZ responses as independently framed `0x7e`
/// fragments. The reliable wrapper still supplies ordering/retry; the explicit
/// file index and offset make unrelated control packets legal between
/// fragments without weakening reassembly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerFileTransferChunk<'a> {
    pub(crate) file_index: u16,
    pub(crate) total_len: u32,
    pub(crate) offset: u32,
    pub(crate) data: &'a [u8],
}

impl<'a> ServerFileTransferChunk<'a> {
    pub(crate) const TYPE: u8 = 0x7e;
    pub(crate) const VERSION: u8 = 1;
    pub(crate) const HEADER_LEN: usize = 1 + 1 + 2 + 4 + 4;
    /// One fragment plus the six-byte application-reliable wrapper remains
    /// within the established 520-byte packet ceiling.
    pub(crate) const MAX_DATA_LEN: usize = 520 - 6 - Self::HEADER_LEN;

    // This shared codec is compiled into both binaries: the client uses parse
    // and the server uses encode.
    #[allow(dead_code)]
    pub(crate) fn parse(packet: &'a [u8]) -> Result<Self, String> {
        if packet.len() <= Self::HEADER_LEN || packet.len() > Self::HEADER_LEN + Self::MAX_DATA_LEN
        {
            return Err(format!(
                "server file chunk length out of bounds: {}",
                packet.len()
            ));
        }
        if packet[0] != Self::TYPE || packet[1] != Self::VERSION {
            return Err("invalid server file chunk type/version".to_string());
        }
        let file_index = u16::from_le_bytes(
            packet[2..4]
                .try_into()
                .expect("validated file chunk index range"),
        );
        let total_len = u32::from_le_bytes(
            packet[4..8]
                .try_into()
                .expect("validated file chunk total range"),
        );
        let offset = u32::from_le_bytes(
            packet[8..12]
                .try_into()
                .expect("validated file chunk offset range"),
        );
        let data = &packet[Self::HEADER_LEN..];
        let total_len_usize = total_len as usize;
        let offset_usize = offset as usize;
        if total_len_usize == 0
            || total_len_usize > crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES
            || offset_usize >= total_len_usize
            || data.len() > total_len_usize.saturating_sub(offset_usize)
        {
            return Err("invalid server file chunk bounds".to_string());
        }
        Ok(Self {
            file_index,
            total_len,
            offset,
            data,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn encode(
        file_index: u16,
        total_len: usize,
        offset: usize,
        data: &[u8],
    ) -> Option<Vec<u8>> {
        if total_len == 0
            || total_len > crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES
            || data.is_empty()
            || data.len() > Self::MAX_DATA_LEN
            || offset >= total_len
            || data.len() > total_len.saturating_sub(offset)
        {
            return None;
        }
        let mut packet = Vec::with_capacity(Self::HEADER_LEN + data.len());
        packet.push(Self::TYPE);
        packet.push(Self::VERSION);
        packet.extend_from_slice(&file_index.to_le_bytes());
        packet.extend_from_slice(&(total_len as u32).to_le_bytes());
        packet.extend_from_slice(&(offset as u32).to_le_bytes());
        packet.extend_from_slice(data);
        Some(packet)
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServerFileTransferAssembler {
    file_index: Option<u16>,
    total_len: usize,
    next_offset: usize,
    fragments: usize,
    data: Vec<u8>,
}

#[allow(dead_code)]
impl ServerFileTransferAssembler {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn push(
        &mut self,
        chunk: ServerFileTransferChunk<'_>,
    ) -> Result<Option<Vec<u8>>, String> {
        let chunk_total = chunk.total_len as usize;
        let chunk_offset = chunk.offset as usize;
        if self.file_index.is_none() {
            if chunk_offset != 0 {
                return Err("orphan server file chunk".to_string());
            }
            self.file_index = Some(chunk.file_index);
            self.total_len = chunk_total;
            self.data = Vec::with_capacity(chunk_total);
        } else if self.file_index != Some(chunk.file_index)
            || self.total_len != chunk_total
            || self.next_offset != chunk_offset
        {
            self.clear();
            return Err("server file chunk identity/offset changed during assembly".to_string());
        }

        let Some(fragments) = self.fragments.checked_add(1) else {
            self.clear();
            return Err("server file chunk count overflow".to_string());
        };
        self.fragments = fragments;
        if self.fragments > crate::zone_wire::MAX_TRANSFER_FRAGMENTS {
            self.clear();
            return Err("server file chunk count exceeds limit".to_string());
        }
        self.data.extend_from_slice(chunk.data);
        self.next_offset = self.data.len();
        if self.next_offset == self.total_len {
            let complete = std::mem::take(&mut self.data);
            self.clear();
            return Ok(Some(complete));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(file_index: u16, total: usize, offset: usize, data: &[u8]) -> Vec<u8> {
        ServerFileTransferChunk::encode(file_index, total, offset, data)
            .expect("valid test fragment")
    }

    #[test]
    fn exact_max_fragment_round_trips_without_changing_payload() {
        let data = vec![0xa5; ServerFileTransferChunk::MAX_DATA_LEN];
        let packet = encoded(0xffff, data.len(), 0, &data);
        assert_eq!(packet.len() + 6, 520);
        let parsed = ServerFileTransferChunk::parse(&packet).unwrap();
        assert_eq!(parsed.file_index, 0xffff);
        assert_eq!(parsed.total_len, data.len() as u32);
        assert_eq!(parsed.offset, 0);
        assert_eq!(parsed.data, data);
    }

    #[test]
    fn malformed_or_out_of_bounds_fragments_fail_closed() {
        assert!(ServerFileTransferChunk::parse(&[]).is_err());
        assert!(ServerFileTransferChunk::encode(0, 1, 0, &[]).is_none());
        assert!(
            ServerFileTransferChunk::encode(
                0,
                crate::zone_wire::MAX_TRANSFER_LOGICAL_BYTES + 1,
                0,
                &[1],
            )
            .is_none()
        );
        assert!(ServerFileTransferChunk::encode(0, 4, 4, &[1]).is_none());

        let mut bad_version = encoded(0, 1, 0, &[1]);
        bad_version[1] = ServerFileTransferChunk::VERSION.wrapping_add(1);
        assert!(ServerFileTransferChunk::parse(&bad_version).is_err());

        let mut excessive = encoded(0, 1, 0, &[1]);
        excessive.extend(std::iter::repeat_n(
            0,
            ServerFileTransferChunk::MAX_DATA_LEN,
        ));
        assert!(ServerFileTransferChunk::parse(&excessive).is_err());
    }

    #[test]
    fn assembler_requires_one_exact_contiguous_identity_and_resets_after_completion() {
        let logical = (0..900)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let split = ServerFileTransferChunk::MAX_DATA_LEN;
        let first = encoded(7, logical.len(), 0, &logical[..split]);
        let second = encoded(7, logical.len(), split, &logical[split..]);
        let mut assembler = ServerFileTransferAssembler::default();

        assert_eq!(
            assembler
                .push(ServerFileTransferChunk::parse(&first).unwrap())
                .unwrap(),
            None
        );
        assert_eq!(
            assembler
                .push(ServerFileTransferChunk::parse(&second).unwrap())
                .unwrap(),
            Some(logical.clone())
        );
        assert_eq!(assembler, ServerFileTransferAssembler::default());

        let orphan = encoded(7, logical.len(), split, &logical[split..]);
        assert!(
            assembler
                .push(ServerFileTransferChunk::parse(&orphan).unwrap())
                .is_err()
        );

        assert!(
            assembler
                .push(ServerFileTransferChunk::parse(&first).unwrap())
                .unwrap()
                .is_none()
        );
        let wrong_identity = encoded(8, logical.len(), split, &logical[split..]);
        assert!(
            assembler
                .push(ServerFileTransferChunk::parse(&wrong_identity).unwrap())
                .is_err()
        );
        assert_eq!(assembler, ServerFileTransferAssembler::default());
    }
}
