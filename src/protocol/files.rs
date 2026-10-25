use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientFileRequest {
    pub(crate) local_filename: String,
    pub(crate) remote_filename: String,
}

impl ClientFileRequest {
    pub(crate) const TYPE: u8 = 0x19;
    pub(crate) const LOCAL_FILENAME_LEN: usize = 256;
    pub(crate) const REMOTE_FILENAME_LEN: usize = 17;
    pub(crate) const PACKET_LEN: usize = 1 + Self::LOCAL_FILENAME_LEN + Self::REMOTE_FILENAME_LEN;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN {
            return Err(format!(
                "client file request packet length mismatch: {} != {}",
                packet.len(),
                Self::PACKET_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected client file request packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            local_filename: read_fixed_c_string(packet, 1, Self::LOCAL_FILENAME_LEN)?,
            remote_filename: read_fixed_c_string(
                packet,
                1 + Self::LOCAL_FILENAME_LEN,
                Self::REMOTE_FILENAME_LEN,
            )?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerVoice {
    pub(crate) player_id: u16,
    pub(crate) wav_data: Vec<u8>,
}

impl PlayerVoice {
    pub(crate) const TYPE: u8 = 0x0c;
    pub(crate) const HEADER_LEN: usize = 3;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::HEADER_LEN {
            return Err(format!(
                "player voice packet too short: {} < {}",
                packet.len(),
                Self::HEADER_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected player voice packet type: {:#04x}",
                packet[0]
            ));
        }

        Ok(Self {
            player_id: read_u16(packet, 1)?,
            wav_data: packet[Self::HEADER_LEN..].to_vec(),
        })
    }

    #[allow(dead_code)]
    pub(crate) fn has_wav_header(&self) -> bool {
        player_voice_payload_has_wav_header(&self.wav_data)
    }
}

#[allow(dead_code)]
pub(crate) fn player_voice_payload_has_wav_header(data: &[u8]) -> bool {
    data.len() >= 12 && data.starts_with(b"RIFF") && data[8..12] == *b"WAVE"
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerFileTransfer {
    pub(crate) filename: String,
    pub(crate) data: Vec<u8>,
}

impl ServerFileTransfer {
    pub(crate) const TYPE: u8 = 0x10;
    pub(crate) const FILENAME_LEN: usize = 16;
    pub(crate) const HEADER_LEN: usize = 1 + Self::FILENAME_LEN;

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() < Self::HEADER_LEN {
            return Err(format!(
                "server file transfer packet too short: {} < {}",
                packet.len(),
                Self::HEADER_LEN
            ));
        }
        if packet[0] != Self::TYPE {
            return Err(format!(
                "unexpected server file transfer packet type: {:#04x}",
                packet[0]
            ));
        }

        let mut filename = read_fixed_c_string(packet, 1, Self::FILENAME_LEN)?;
        if filename.is_empty() {
            filename = "News.txt".to_string();
        }

        Ok(Self {
            filename,
            data: packet[Self::HEADER_LEN..].to_vec(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapInformation {
    pub(crate) zone_id: Option<String>,
    pub(crate) filename: String,
    pub(crate) checksum: u32,
    pub(crate) sha256: Option<crate::zone_wire::Sha256Digest>,
    pub(crate) compressed_size: u32,
    pub(crate) compressed_size_is_known: bool,
    pub(crate) lvz_files: Vec<FileRequestSpec>,
}

impl MapInformation {
    pub(crate) const AUTHENTICATED_HEADER_LEN: usize =
        crate::zone_wire::AUTHENTICATED_MAP_INFO_HEADER_BYTES;
    pub(crate) const AUTHENTICATED_FILE_LEN: usize =
        crate::zone_wire::AUTHENTICATED_MAP_INFO_FILE_BYTES;
    pub(crate) const MAX_FILES: usize = crate::zone_wire::MAX_ZONE_FILES;
    pub(crate) const MAX_COMPRESSED_MAP_BYTES: u32 =
        crate::zone_wire::MAX_TRANSFER_FILE_BYTES as u32;
    pub(crate) const MAX_LVZ_BYTES: u32 = crate::zone_wire::MAX_TRANSFER_FILE_BYTES as u32;
    pub(crate) const MAX_TOTAL_TRANSFER_BYTES: u64 =
        crate::zone_wire::MAX_ZONE_TRANSFER_BYTES as u64;

    #[allow(dead_code)]
    pub(crate) fn parse_payload(
        payload: &[u8],
        continuum_packet_format: bool,
    ) -> Result<Self, String> {
        if payload.starts_with(&crate::zone_wire::AUTHENTICATED_MAP_INFO_MAGIC) {
            return Self::parse_authenticated_payload(payload);
        }
        Self::parse_legacy_payload(payload, continuum_packet_format)
    }

    fn parse_authenticated_payload(payload: &[u8]) -> Result<Self, String> {
        if payload.len() < Self::AUTHENTICATED_HEADER_LEN {
            return Err(format!(
                "authenticated map information payload too short: {} < {}",
                payload.len(),
                Self::AUTHENTICATED_HEADER_LEN
            ));
        }
        if payload[4] != crate::zone_wire::AUTHENTICATED_MAP_INFO_VERSION {
            return Err(format!(
                "unsupported authenticated map information version {}",
                payload[4]
            ));
        }
        let zone_id = crate::zone_wire::parse_canonical_fixed_ascii(&payload[5..21])?.to_string();
        if !crate::zone_wire::valid_zone_id(&zone_id) {
            return Err(format!("invalid authenticated map zone id {zone_id:?}"));
        }
        let filename = crate::zone_wire::parse_canonical_fixed_ascii(&payload[21..37])?.to_string();
        let checksum = read_u32(payload, 37)?;
        let compressed_size = read_u32(payload, 41)?;
        let sha256 = payload[45..77]
            .try_into()
            .expect("authenticated map digest has fixed size");
        let file_count = read_u16(payload, 77)? as usize;
        if file_count == 0 || file_count > Self::MAX_FILES {
            return Err(format!(
                "authenticated map information advertises invalid file count {file_count}"
            ));
        }
        let expected_len = Self::AUTHENTICATED_HEADER_LEN
            .checked_add((file_count - 1).saturating_mul(Self::AUTHENTICATED_FILE_LEN))
            .ok_or_else(|| "authenticated map information length overflow".to_string())?;
        if payload.len() != expected_len {
            return Err(format!(
                "authenticated map information length mismatch: {} != {expected_len}",
                payload.len()
            ));
        }

        let mut lvz_files = Vec::with_capacity(file_count - 1);
        let mut offset = Self::AUTHENTICATED_HEADER_LEN;
        for index in 1..file_count {
            lvz_files.push(FileRequestSpec {
                zone_id: Some(zone_id.clone()),
                filename: crate::zone_wire::parse_canonical_fixed_ascii(
                    &payload[offset..offset + 16],
                )?
                .to_string(),
                index: index as u16,
                checksum: read_u32(payload, offset + 16)?,
                size: read_u32(payload, offset + 20)?,
                size_is_known: true,
                sha256: Some(
                    payload[offset + 24..offset + Self::AUTHENTICATED_FILE_LEN]
                        .try_into()
                        .expect("authenticated LVZ digest has fixed size"),
                ),
                decompress: false,
            });
            offset += Self::AUTHENTICATED_FILE_LEN;
        }
        let info = Self {
            zone_id: Some(zone_id),
            filename,
            checksum,
            sha256: Some(sha256),
            compressed_size,
            compressed_size_is_known: true,
            lvz_files,
        };
        info.validate()?;
        Ok(info)
    }

    fn parse_legacy_payload(payload: &[u8], continuum_packet_format: bool) -> Result<Self, String> {
        let min_len = 16 + 4 + if continuum_packet_format { 4 } else { 0 };
        if payload.len() < min_len {
            return Err(format!(
                "map information payload too short: {} < {}",
                payload.len(),
                min_len
            ));
        }
        let filename = read_fixed_c_string(payload, 0, 16)?;
        let checksum = read_u32(payload, 16)?;
        let compressed_size = if continuum_packet_format {
            read_u32(payload, 20)?
        } else {
            0
        };
        let mut offset = min_len;
        let remaining = payload.len() - offset;
        if !remaining.is_multiple_of(24) {
            return Err("map information LVZ file list is not aligned".to_string());
        }
        let mut lvz_files = Vec::with_capacity(remaining / 24);
        let mut index = 1;
        while offset < payload.len() {
            lvz_files.push(FileRequestSpec {
                zone_id: None,
                filename: read_fixed_c_string(payload, offset, 16)?,
                index,
                checksum: read_u32(payload, offset + 16)?,
                size: read_u32(payload, offset + 20)?,
                size_is_known: true,
                sha256: None,
                decompress: false,
            });
            index += 1;
            offset += 24;
        }
        let info = Self {
            zone_id: None,
            filename,
            checksum,
            sha256: None,
            compressed_size,
            compressed_size_is_known: continuum_packet_format,
            lvz_files,
        };
        info.validate()?;
        Ok(info)
    }

    fn validate(&self) -> Result<(), String> {
        crate::zone_wire::validate_wire_asset_filename(&self.filename, "lvl")?;
        if self.compressed_size > Self::MAX_COMPRESSED_MAP_BYTES {
            return Err(format!(
                "map information compressed map is too large: {}",
                self.compressed_size
            ));
        }
        if 1 + self.lvz_files.len() > Self::MAX_FILES {
            return Err(format!(
                "map information advertises too many files: {}",
                1 + self.lvz_files.len()
            ));
        }
        let mut total = u64::from(self.compressed_size);
        let mut filenames = std::collections::BTreeSet::new();
        filenames.insert(self.filename.to_ascii_lowercase());
        for file in &self.lvz_files {
            if crate::zone_wire::validate_wire_asset_filename(&file.filename, "lvz").is_err()
                || file.size > Self::MAX_LVZ_BYTES
                || file.zone_id != self.zone_id
                || self.sha256.is_some() != file.sha256.is_some()
                || !filenames.insert(file.filename.to_ascii_lowercase())
            {
                return Err(format!(
                    "map information has invalid LVZ entry {:?}",
                    file.filename
                ));
            }
            total = total.saturating_add(u64::from(file.size));
        }
        if total > Self::MAX_TOTAL_TRANSFER_BYTES {
            return Err(format!(
                "map information transfer budget is too large: {total}"
            ));
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn file_request(&self) -> FileRequestSpec {
        FileRequestSpec {
            zone_id: self.zone_id.clone(),
            filename: self.filename.clone(),
            index: 0,
            size: self.compressed_size,
            size_is_known: self.compressed_size_is_known,
            checksum: self.checksum,
            sha256: self.sha256,
            decompress: true,
        }
    }

    pub(crate) fn file_requests(&self) -> Vec<FileRequestSpec> {
        let mut requests = Vec::with_capacity(1 + self.lvz_files.len());
        requests.push(self.file_request());
        requests.extend(self.lvz_files.iter().cloned());
        requests
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRequestSpec {
    pub(crate) zone_id: Option<String>,
    pub(crate) filename: String,
    pub(crate) index: u16,
    pub(crate) size: u32,
    pub(crate) size_is_known: bool,
    pub(crate) checksum: u32,
    pub(crate) sha256: Option<crate::zone_wire::Sha256Digest>,
    pub(crate) decompress: bool,
}

impl FileRequestSpec {
    pub(crate) const MAP_REQUEST_TYPE: u8 = 0x0c;

    #[allow(dead_code)]
    pub(crate) fn encode_request_packet(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(3);
        push_u8(&mut bytes, Self::MAP_REQUEST_TYPE);
        push_u16(&mut bytes, self.index);
        bytes
    }

    #[allow(dead_code)]
    pub(crate) fn matches_download_checksum(&self, data: &[u8]) -> bool {
        continuum_file_crc32(data) == self.checksum
    }

    pub(crate) fn matches_download_digest(&self, data: &[u8]) -> bool {
        self.sha256
            .is_none_or(|expected| crate::zone_wire::sha256_digest(data) == expected)
    }

    #[allow(dead_code)]
    pub(crate) fn cache_identity(&self) -> Option<String> {
        crate::zone_wire::map_cache_identity(
            self.zone_id.as_deref()?,
            &self.filename,
            &self.sha256?,
        )
        .ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedMapCheck {
    pub(crate) zone_id: String,
    pub(crate) sha256: crate::zone_wire::Sha256Digest,
}

impl CachedMapCheck {
    pub(crate) const TYPE: u8 = 0x7c;
    pub(crate) const PACKET_LEN: usize = 1 + 1 + 16 + 32;

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::PACKET_LEN);
        push_u8(&mut bytes, Self::TYPE);
        push_u8(&mut bytes, crate::zone_wire::CACHED_MAP_CHECK_VERSION);
        bytes.resize(2 + crate::zone_wire::ZONE_ID_BYTES, 0);
        let zone_bytes = self.zone_id.as_bytes();
        let copied = zone_bytes.len().min(crate::zone_wire::ZONE_ID_BYTES - 1);
        bytes[2..2 + copied].copy_from_slice(&zone_bytes[..copied]);
        bytes.extend_from_slice(&self.sha256);
        bytes
    }

    #[cfg(test)]
    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN
            || packet.first() != Some(&Self::TYPE)
            || packet[1] != crate::zone_wire::CACHED_MAP_CHECK_VERSION
        {
            return Err("cached map check packet has invalid shape".to_string());
        }
        let zone_id = crate::zone_wire::parse_canonical_fixed_ascii(
            &packet[2..2 + crate::zone_wire::ZONE_ID_BYTES],
        )?
        .to_string();
        if !crate::zone_wire::valid_zone_id(&zone_id) {
            return Err("cached map check has invalid zone identity".to_string());
        }
        Ok(Self {
            zone_id,
            sha256: packet[18..50]
                .try_into()
                .expect("cached map SHA-256 has fixed size"),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressedMapPacket<'a> {
    pub(crate) payload: &'a [u8],
}

impl<'a> CompressedMapPacket<'a> {
    pub(crate) const HEADER_SIZE: usize = 17;

    #[allow(dead_code)]
    pub(crate) fn parse(packet: &'a [u8]) -> Result<Self, String> {
        if packet.len() < Self::HEADER_SIZE {
            return Err(format!(
                "compressed map packet too short: {} < {}",
                packet.len(),
                Self::HEADER_SIZE
            ));
        }

        Ok(Self {
            payload: &packet[Self::HEADER_SIZE..],
        })
    }

    #[allow(dead_code)]
    pub(crate) fn decode_for_request(&self, request: &FileRequestSpec) -> Result<Vec<u8>, String> {
        if self.payload.len() > crate::zone_wire::MAX_TRANSFER_FILE_BYTES {
            return Err(format!(
                "download payload is too large for {:?}: {} > {}",
                request.filename,
                self.payload.len(),
                crate::zone_wire::MAX_TRANSFER_FILE_BYTES
            ));
        }
        if request.size_is_known && self.payload.len() != request.size as usize {
            return Err(format!(
                "download payload size mismatch for {:?}: {} != {}",
                request.filename,
                self.payload.len(),
                request.size
            ));
        }
        if request.decompress {
            crate::continuum::decompress_zlib_payload_bounded(
                self.payload,
                crate::zone_wire::MAX_MAP_SOURCE_BYTES,
            )
        } else {
            Ok(self.payload.to_vec())
        }
    }
}
