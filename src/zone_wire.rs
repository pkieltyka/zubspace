use sha2::{Digest, Sha256};

pub(crate) const ZONE_ID_BYTES: usize = 16;
// The server binary enforces this during merged-catalog construction; the
// separately-built browser helper mirrors it for streamed `/zones` admission.
#[allow(dead_code)]
pub(crate) const MAX_ZONE_COUNT: usize = 64;
pub(crate) const WIRE_FILENAME_BYTES: usize = 16;
pub(crate) const SHA256_BYTES: usize = 32;
pub(crate) const AUTHENTICATED_MAP_INFO_MAGIC: [u8; 4] = *b"ZMAP";
pub(crate) const AUTHENTICATED_MAP_INFO_VERSION: u8 = 1;
pub(crate) const AUTHENTICATED_MAP_INFO_HEADER_BYTES: usize = 4 + 1 + 16 + 16 + 4 + 4 + 32 + 2;
pub(crate) const AUTHENTICATED_MAP_INFO_FILE_BYTES: usize = 16 + 4 + 4 + 32;
pub(crate) const CACHED_MAP_CHECK_VERSION: u8 = 1;

// One decoded logical transfer remains bounded at 1 MiB. Current map/LVZ
// delivery retains only a 64-fragment application window and reassembles to
// this bound; legacy generic small/huge chunks share the same decoded limit.
pub(crate) const MAX_TRANSFER_LOGICAL_BYTES: usize = 1024 * 1024;
// Every transfer representation also bounds fragment bookkeeping. Legitimate
// roughly-500-byte payload fragments need fewer than 2,100 entries at the
// logical maximum, leaving headroom without making processing attacker-owned.
#[allow(dead_code)]
pub(crate) const MAX_TRANSFER_FRAGMENTS: usize = 4_096;
pub(crate) const COMPRESSED_MAP_HEADER_BYTES: usize = 17;
pub(crate) const MAX_TRANSFER_FILE_BYTES: usize =
    MAX_TRANSFER_LOGICAL_BYTES - COMPRESSED_MAP_HEADER_BYTES;
pub(crate) const MAX_MAP_SOURCE_BYTES: usize = MAX_TRANSFER_FILE_BYTES;
pub(crate) const MAX_ZONE_FILES: usize = 65;
pub(crate) const MAX_ZONE_TRANSFER_BYTES: usize = 32 * 1024 * 1024;
// Server catalog construction and the client runtime use these same bounds.
// They cap both retained source/encoded data and the expansion represented by
// parsed LVZ archives across a whole active zone/catalog.
#[allow(dead_code)]
pub(crate) const MAX_ZONE_DECODED_LVZ_BYTES: usize = 64 * 1024 * 1024;
#[allow(dead_code)]
pub(crate) const MAX_ZONE_CATALOG_SOURCE_BYTES: usize = 64 * 1024 * 1024;
#[allow(dead_code)]
pub(crate) const MAX_ZONE_CATALOG_ENCODED_BYTES: usize = 64 * 1024 * 1024;
#[allow(dead_code)]
pub(crate) const MAX_ZONE_CATALOG_DECODED_BYTES: usize = 512 * 1024 * 1024;

// These limits describe retained/runtime representation, not just bytes on
// the wire. They are shared by catalog construction, protocol admission, and
// the renderer so a small compressed asset cannot expand after validation.
pub(crate) const MAX_RENDER_IMAGE_DIMENSION: usize = 4_096;
pub(crate) const MAX_RENDER_IMAGE_PIXELS: usize = 4 * 1024 * 1024;
pub(crate) const MAX_RENDER_IMAGE_RGBA_BYTES: usize = MAX_RENDER_IMAGE_PIXELS * 4;
pub(crate) const MAX_IMAGE_DECODER_ALLOC_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_ZONE_RENDER_PIXEL_BYTES: usize = 256 * 1024 * 1024;
// Cumulative CPU decode work charges every renderable file payload, including
// overwritten duplicates. It is deliberately separate from the normalized
// last-wins retained texture budget above.
pub(crate) const MAX_ZONE_IMAGE_DECODE_WORK_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_ZONE_RENDER_BYTES: usize = 288 * 1024 * 1024;
// Peak includes retained decoded/GPU texture state plus the bounded CPU RGBA
// upload buffer and newly-created texture for one image. Archives are
// validated sequentially and normalized last-wins files are uploaded one at a
// time, so no second unbounded texture collection exists.
// Optional validated-pixel handoff uses only headroom left after the largest
// manifest prefix plus decoder/upload workspace; see ImagePreparationBudget.
pub(crate) const MAX_ZONE_RENDER_PEAK_BYTES: usize = 320 * 1024 * 1024;
pub(crate) const MAX_LVZ_OBJECTS_PER_ZONE: usize = 65_536;
pub(crate) const MAX_LVZ_IMAGES_PER_ZONE: usize = 4_096;
pub(crate) const MAX_LVZ_TEXTURES_PER_ZONE: usize = 4_096;
pub(crate) const MAX_LVZ_OBJECT_REFERENCES_PER_ZONE: usize = 65_536;
pub(crate) const MAX_LVZ_CHAT_SOUNDS_PER_ZONE: usize = 255;
pub(crate) const MAX_LVZ_CHAT_SOUND_PCM_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_LVZ_CHAT_SOUND_PCM_BYTES_PER_FILE: usize = 4 * 1024 * 1024;
pub(crate) const LVZ_OBJECT_RETAINED_BYTES: usize = 64;
pub(crate) const LVZ_IMAGE_RETAINED_BASE_BYTES: usize = 96;
pub(crate) const LVZ_RENDER_REFERENCE_RETAINED_BYTES: usize = 32;
pub(crate) const LVZ_TEXTURE_RETAINED_BASE_BYTES: usize = 64;
// LVZ names are legacy C strings. Keep parsing and canonicalization bounded,
// and charge the Arc plus tree/reference bookkeeping retained by parsed and
// render state independently of the string bytes themselves.
pub(crate) const MAX_LVZ_FILENAME_BYTES: usize = 255;
pub(crate) const LVZ_FILE_RETAINED_BASE_BYTES: usize = 80;
pub(crate) const LVZ_FILENAME_REFERENCE_RETAINED_BYTES: usize = 24;
pub(crate) const MAX_MAP_TILE_RECORDS: usize = 131_072;
pub(crate) const MAP_OBJECT_RETAINED_BYTES: usize = 24;
pub(crate) const MAP_DOOR_RETAINED_BYTES: usize = 24;
pub(crate) const MAP_RADAR_POINT_RETAINED_BYTES: usize = 32;
pub(crate) const MAX_MAP_REPRESENTATION_BYTES: usize = 16 * 1024 * 1024;

pub(crate) type Sha256Digest = [u8; SHA256_BYTES];

pub(crate) fn sha256_digest(bytes: &[u8]) -> Sha256Digest {
    Sha256::digest(bytes).into()
}

#[allow(dead_code)]
pub(crate) fn sha256_hex(digest: &Sha256Digest) -> String {
    let mut encoded = String::with_capacity(SHA256_BYTES * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[allow(dead_code)]
pub(crate) fn parse_sha256_hex(encoded: &str) -> Result<Sha256Digest, String> {
    if encoded.len() != SHA256_BYTES * 2
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("invalid lowercase SHA-256 digest {encoded:?}"));
    }
    let mut digest = [0_u8; SHA256_BYTES];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&encoded[index * 2..index * 2 + 2], 16)
            .map_err(|err| format!("invalid SHA-256 digest {encoded:?}: {err}"))?;
    }
    Ok(digest)
}

pub(crate) fn valid_zone_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() < ZONE_ID_BYTES
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(crate) fn parse_canonical_fixed_ascii(field: &[u8]) -> Result<&str, String> {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| "fixed wire string is missing its NUL terminator".to_string())?;
    if field[end..].iter().any(|byte| *byte != 0) {
        return Err("fixed wire string has nonzero bytes after its terminator".to_string());
    }
    let value = std::str::from_utf8(&field[..end])
        .map_err(|_| "fixed wire string is not UTF-8".to_string())?;
    if !value.is_ascii() {
        return Err("fixed wire string is not ASCII".to_string());
    }
    Ok(value)
}

pub(crate) fn validate_wire_asset_filename(filename: &str, extension: &str) -> Result<(), String> {
    let valid = !filename.is_empty()
        && filename.len() < WIRE_FILENAME_BYTES
        && filename.is_ascii()
        && filename.bytes().all(is_safe_asset_filename_byte)
        && !filename.contains("..")
        && filename.rsplit_once('.').is_some_and(|(stem, actual)| {
            !stem.is_empty() && actual.eq_ignore_ascii_case(extension)
        });
    if !valid {
        return Err(format!(
            "invalid .{extension} wire filename {filename:?}; expected a safe 1-15 byte ASCII basename"
        ));
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn validate_plain_bundle_filename(filename: &str) -> Result<(), String> {
    let valid = !filename.is_empty()
        && filename.is_ascii()
        && filename.bytes().all(is_safe_asset_filename_byte)
        && !filename.contains("..");
    if !valid {
        return Err(format!(
            "zone bundle paths must be safe ASCII basenames without whitespace or traversal: {filename:?}"
        ));
    }
    Ok(())
}

fn is_safe_asset_filename_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'#' | b'@')
}

#[allow(dead_code)]
pub(crate) fn map_cache_identity(
    zone_id: &str,
    filename: &str,
    digest: &Sha256Digest,
) -> Result<String, String> {
    if !valid_zone_id(zone_id) {
        return Err(format!("invalid cache zone identity {zone_id:?}"));
    }
    if validate_wire_asset_filename(filename, "lvl").is_err() {
        validate_wire_asset_filename(filename, "lvz")?;
    }
    Ok(format!("v2.{zone_id}.{}.{filename}", sha256_hex(digest)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_wire_filename_policy_rejects_cross_platform_traversal_shapes() {
        for invalid in [
            "../bad.lvl",
            "bad\\map.lvl",
            "bad/map.lvl",
            "bad map.lvl",
            "bad..map.lvl",
            ".lvl",
            "012345678901.lvl",
            "map.lvz",
        ] {
            assert!(
                validate_wire_asset_filename(invalid, "lvl").is_err(),
                "{invalid:?} must fail"
            );
        }
        assert!(validate_wire_asset_filename("@map_1.lvl", "lvl").is_ok());
        assert!(validate_wire_asset_filename("#fx-1.lvz", "lvz").is_ok());
        assert_eq!(
            parse_canonical_fixed_ascii(b"map.lvl\0\0").unwrap(),
            "map.lvl"
        );
        assert!(parse_canonical_fixed_ascii(b"map.lvl\0x").is_err());
        assert!(parse_canonical_fixed_ascii(b"0123456789abcdef").is_err());
    }

    #[test]
    fn cache_identity_separates_equal_crc_maps_by_zone_and_sha256() {
        // These distinct 16-byte payloads are a real CRC-32 collision. The
        // cache identity must still differ because it binds SHA-256.
        let first_bytes = b"\x34\xc4\x99\x54\x07\x0a\x6f\xcb\x46\x32\xb8\x2d\xc5\x17\x05\x1a";
        let second_bytes = b"\x28\x00\x19\x15\x4e\x1c\x9f\x1a\xc8\x22\x29\xc0\x2b\x8e\x11\x19";
        assert_eq!(
            crate::continuum::continuum_file_crc32(first_bytes),
            crate::continuum::continuum_file_crc32(second_bytes)
        );
        let first = sha256_digest(first_bytes);
        let second = sha256_digest(second_bytes);
        assert_ne!(
            map_cache_identity("alpha", "map.lvl", &first).unwrap(),
            map_cache_identity("beta", "map.lvl", &first).unwrap()
        );
        assert_ne!(
            map_cache_identity("alpha", "map.lvl", &first).unwrap(),
            map_cache_identity("alpha", "map.lvl", &second).unwrap()
        );
    }
}

/// Version two of the existing cached-map control packet also identifies a
/// manifest file. The map digest scopes LVZ indexes to the advertised catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HeldZoneFile {
    pub(crate) zone_id: String,
    pub(crate) map_sha256: Sha256Digest,
    pub(crate) index: u16,
    pub(crate) sha256: Sha256Digest,
}

impl HeldZoneFile {
    pub(crate) const TYPE: u8 = 0x7c;
    pub(crate) const VERSION: u8 = 2;
    pub(crate) const PACKET_LEN: usize = 84;

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![0; Self::PACKET_LEN];
        bytes[0] = Self::TYPE;
        bytes[1] = Self::VERSION;
        let zone = self.zone_id.as_bytes();
        let len = zone.len().min(ZONE_ID_BYTES - 1);
        bytes[2..2 + len].copy_from_slice(&zone[..len]);
        bytes[18..50].copy_from_slice(&self.map_sha256);
        bytes[50..52].copy_from_slice(&self.index.to_le_bytes());
        bytes[52..84].copy_from_slice(&self.sha256);
        bytes
    }

    pub(crate) fn parse(packet: &[u8]) -> Result<Self, String> {
        if packet.len() != Self::PACKET_LEN || packet[0] != Self::TYPE || packet[1] != Self::VERSION
        {
            return Err("invalid held-zone-file shape".into());
        }
        let zone_id = parse_canonical_fixed_ascii(&packet[2..18])?.to_owned();
        if !valid_zone_id(&zone_id) {
            return Err("invalid held-zone-file zone".into());
        }
        Ok(Self {
            zone_id,
            map_sha256: packet[18..50].try_into().unwrap(),
            index: u16::from_le_bytes(packet[50..52].try_into().unwrap()),
            sha256: packet[52..84].try_into().unwrap(),
        })
    }
}

// The existing password-response parser accepts trailing bytes. Older clients
// ignore this extension; new clients keep LVZ requests on 0x0c for old servers.
pub(crate) const HELD_ZONE_FILES_LOGIN_TRAILER: &[u8; 6] = b"ZCAP\x01\x01";
