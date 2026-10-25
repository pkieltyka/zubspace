use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use super::security::SECURITY_CHECKSUM_KEY;
use super::wire::{continuum_file_crc32, copy_fixed};
use crate::OPEN_DOOR_TILE_ID;
use crate::continuum::continuum_map_crc32;
use crate::door::seeded_door_tile;

#[path = "zone/assets.rs"]
mod assets;
#[path = "zone/bundles.rs"]
mod bundles;

#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use assets::{
    CHAOS_ZONE_ID, EXTREME_ZONE_ID, SERVER_LVZ_FILES, SERVER_MAP_BYTES, SERVER_MAP_FILENAME,
    SERVER_NEWS_BYTES, SERVER_NEWS_FILENAME, SERVER_ZONE_IDS,
};
use assets::{ZONE_DEFINITIONS, ZoneDefinition};

pub(crate) const MAP_INFORMATION: u8 = 0x29;
pub(crate) const COMPRESSED_MAP: u8 = 0x2a;
pub(crate) const SERVER_FILE_TRANSFER: u8 = 0x10;
pub(crate) const COMPRESSED_MAP_HEADER_SIZE: usize = 17;
// These constants are intentionally duplicated from `src/constants.rs`
// (client side) because the types differ (usize vs i32, no name collision).
// The compile-time assertions below pin the values together so a stray
// edit on one side fails the build instead of silently desyncing.
pub(crate) const MAP_TILES: usize = 1024;
const FLAG_TILE_ID: u8 = 170;
pub(crate) const SAFE_TILE_ID: u8 = 171;
pub(crate) const GOAL_TILE_ID: u8 = 172;
const DOOR_FIRST_TILE_ID: u8 = 162;
const DOOR_LAST_TILE_ID: u8 = 169;
const WORMHOLE_TILE_ID: u8 = 220;
const BRICK_TILE_ID: u8 = 250;
pub(crate) const TILE_SIZE_PX: f32 = 16.0;

#[cfg(test)]
#[test]
fn server_zone_constants_match_client_constants() {
    // Independent reference: the client `src/constants.rs` definitions.
    // If anyone changes `MAP_TILES` or `TILE_SIZE` on either side, this
    // test fails until both are updated.
    assert_eq!(MAP_TILES as i32, 1024, "MAP_TILES drifted from client");
    assert!(
        (TILE_SIZE_PX - 16.0).abs() < f32::EPSILON,
        "TILE_SIZE_PX drifted from client"
    );
}
const GOAL_TOUCH_RADIUS_PX: f32 = 18.0;

static ZONE_MAPS: OnceLock<Vec<ZoneMap>> = OnceLock::new();
#[cfg(test)]
static ZONE_IDS: OnceLock<Box<[&'static str]>> = OnceLock::new();

pub(crate) struct ZoneMap {
    pub(crate) zone_id: Cow<'static, str>,
    pub(crate) display_name: Cow<'static, str>,
    pub(crate) filename: Cow<'static, str>,
    pub(crate) news_filename: Cow<'static, str>,
    pub(crate) news_bytes: Cow<'static, [u8]>,
    pub(crate) tiles: Vec<u8>,
    pub(crate) no_flag_drop_tiles: NoFlagDropTiles,
    pub(crate) checksum: u32,
    pub(crate) sha256: crate::zone_wire::Sha256Digest,
    pub(crate) compressed: Vec<u8>,
    pub(crate) map_checksum: u32,
    pub(crate) continuum_crc32: u32,
    // Flag tiles remain parsed map metadata, not server objective instances.
    #[allow(dead_code)]
    pub(crate) flags: Vec<ZoneFlag>,
    /// Server-only objectives are deliberately separate from map tile metadata
    /// and packet-backed client tuning. A map flag tile or soccer goal never
    /// activates a multiplayer objective without an explicit validated profile.
    pub(crate) objective_profile: crate::objectives::ServerObjectiveProfile,
    pub(crate) lvz_files: Vec<ZoneFile>,
    pub(crate) settings_packet: Cow<'static, [u8]>,
    pub(crate) brick_span: u8,
    pub(crate) resource_usage: ZoneCatalogUsage,
}

impl ZoneMap {
    pub(crate) fn no_flag_drops_at(&self, x: u16, y: u16) -> bool {
        self.no_flag_drop_tiles.contains(x, y)
    }
}

#[derive(Clone, Default)]
pub(crate) struct NoFlagDropTiles {
    bits: Box<[u64]>,
}

impl NoFlagDropTiles {
    const WORDS: usize = MAP_TILES * MAP_TILES / u64::BITS as usize;

    pub(crate) fn contains(&self, x: u16, y: u16) -> bool {
        let index = usize::from(y) * MAP_TILES + usize::from(x);
        self.bits
            .get(index / u64::BITS as usize)
            .is_some_and(|word| word & (1_u64 << (index % u64::BITS as usize)) != 0)
    }

    fn insert(&mut self, x: usize, y: usize) {
        if self.bits.is_empty() {
            self.bits = vec![0; Self::WORDS].into_boxed_slice();
        }
        let index = y * MAP_TILES + x;
        self.bits[index / u64::BITS as usize] |= 1_u64 << (index % u64::BITS as usize);
    }

    fn retained_bytes(&self) -> usize {
        self.bits.len() * std::mem::size_of::<u64>()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ZoneFlag {
    pub(crate) id: u16,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) owner: u16,
}

pub(crate) struct ZoneFile {
    pub(crate) filename: Cow<'static, str>,
    pub(crate) checksum: u32,
    pub(crate) sha256: crate::zone_wire::Sha256Digest,
    pub(crate) bytes: Cow<'static, [u8]>,
}

pub(crate) struct ServerZoneCatalog {
    maps: Vec<ZoneMap>,
    usage: ZoneCatalogUsage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ZoneCatalogUsage {
    source: usize,
    encoded: usize,
    decoded: usize,
}

impl ServerZoneCatalog {
    pub(crate) fn zone_ids(&self) -> impl Iterator<Item = &str> {
        self.maps.iter().map(|map| map.zone_id.as_ref())
    }

    pub(crate) fn maps(&self) -> &[ZoneMap] {
        &self.maps
    }
}

pub(crate) fn zone_map() -> &'static ZoneMap {
    zone_map_for_id(CHAOS_ZONE_ID).expect("chaos-zone should be bundled server-side")
}

pub(crate) fn zone_map_for_id(zone_id: &str) -> Option<&'static ZoneMap> {
    zone_maps()
        .iter()
        .find(|map| map.zone_id.as_ref() == zone_id)
}

/// [REVIEW H9] Test-only manifest of the embedded zone assets so a test
/// can cross-check the hand-maintained `include_bytes!` table in
/// `zone/assets.rs` against the on-disk `assets/zones/` tree — the two
/// are synchronized by hand (the client's build.rs manifest is generated;
/// the server's embed table is not), and this catches the drift.
#[cfg(test)]
pub(crate) fn zone_asset_manifest() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    ZONE_DEFINITIONS
        .iter()
        .map(|zone| {
            (
                zone.id,
                zone.map_filename,
                zone.lvz_files.iter().map(|(name, _)| *name).collect(),
            )
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn supported_server_zone_ids() -> &'static [&'static str] {
    ZONE_IDS.get_or_init(|| {
        server_zone_maps()
            .iter()
            .map(|zone| zone.zone_id.as_ref())
            .collect::<Vec<_>>()
            .into_boxed_slice()
    })
}

pub(crate) fn server_zone_maps() -> &'static [ZoneMap] {
    zone_maps()
}

pub(crate) fn zone_id_for_arena_name(name: &str) -> Option<&'static str> {
    let normalized = normalized_zone_lookup_name(name);
    if normalized.is_empty() {
        return None;
    }
    server_zone_maps()
        .iter()
        .find(|map| {
            normalized == normalized_zone_lookup_name(map.zone_id.as_ref())
                || normalized == normalized_zone_lookup_name(map.display_name.as_ref())
        })
        .map(|map| map.zone_id.as_ref())
}

pub(super) fn normalized_zone_lookup_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalized_zone_identity_aliases(
    zone_id: &str,
    display_name: &str,
) -> Result<BTreeSet<String>, String> {
    let mut aliases = BTreeSet::new();
    for (kind, value) in [("id", zone_id), ("display_name", display_name)] {
        let normalized = normalized_zone_lookup_name(value);
        if normalized.is_empty() {
            return Err(format!(
                "zone {zone_id:?} has an empty normalized {kind} identity"
            ));
        }
        // An id and display label belonging to the same zone may intentionally
        // normalize alike (for example `extreme` / `Extreme`). The browser
        // coalesces those local aliases before catalog-wide uniqueness checks.
        aliases.insert(normalized);
    }
    Ok(aliases)
}

fn validate_zone_catalog_identities(maps: &[ZoneMap]) -> Result<(), String> {
    let mut aliases = BTreeSet::new();
    for map in maps {
        for normalized in
            normalized_zone_identity_aliases(map.zone_id.as_ref(), map.display_name.as_ref())?
        {
            if !aliases.insert(normalized.clone()) {
                return Err(format!(
                    "zone {:?} identity {normalized:?} conflicts with another loaded zone after normalization",
                    map.zone_id
                ));
            }
        }
    }
    Ok(())
}

fn zone_maps() -> &'static [ZoneMap] {
    ZONE_MAPS.get_or_init(embedded_zone_maps)
}

fn build_zone_map(definition: &ZoneDefinition) -> ZoneMap {
    build_zone_map_parts(
        Cow::Borrowed(definition.id),
        Cow::Borrowed(definition.display_name),
        Cow::Borrowed(definition.map_filename),
        Cow::Borrowed(definition.map_bytes),
        Cow::Borrowed(definition.news_filename),
        Cow::Borrowed(definition.news_bytes),
        definition.settings_bytes,
        definition
            .lvz_files
            .iter()
            .map(|(filename, bytes)| (Cow::Borrowed(*filename), Cow::Borrowed(*bytes))),
    )
    .expect("embedded server zone should be valid")
}

fn build_zone_map_parts(
    id: Cow<'static, str>,
    display_name: Cow<'static, str>,
    map_filename: Cow<'static, str>,
    map_bytes: Cow<'static, [u8]>,
    news_filename: Cow<'static, str>,
    news_bytes: Cow<'static, [u8]>,
    settings_bytes: &[u8],
    lvz_files: impl IntoIterator<Item = (Cow<'static, str>, Cow<'static, [u8]>)>,
) -> Result<ZoneMap, String> {
    crate::zone_wire::validate_wire_asset_filename(map_filename.as_ref(), "lvl")?;
    if map_bytes.len() > crate::zone_wire::MAX_MAP_SOURCE_BYTES {
        return Err(format!(
            "zone {id:?} map source is {} bytes; maximum is {}",
            map_bytes.len(),
            crate::zone_wire::MAX_MAP_SOURCE_BYTES
        ));
    }
    let compressed = compress_map_for_transfer(id.as_ref(), map_bytes.as_ref())?;
    let map_layout = crate::render_limits::validate_lvl_source(map_bytes.as_ref())
        .map_err(|err| format!("zone {id:?} map representation is invalid: {err}"))?;
    let tiles = parse_lvl_tiles(map_bytes.as_ref())
        .map_err(|err| format!("zone {id:?} map is invalid: {err}"))?;
    let no_flag_drop_tiles = parse_no_flag_drop_tiles(map_bytes.as_ref())
        .map_err(|err| format!("zone {id:?} map regions are invalid: {err}"))?;
    let settings_packet =
        crate::arena_settings::settings_packet_from_bundle(id.as_ref(), settings_bytes)?;
    let brick_span = crate::arena_settings::brick_span_from_bundle(id.as_ref(), settings_bytes)?;

    let mut lvz_render_usage = crate::lvz_archive::LvzRenderUsage::default();
    let mut lvz_render_images = BTreeMap::<std::sync::Arc<str>, usize>::new();
    let mut pending_lvz = Vec::new();
    for (filename, bytes) in lvz_files {
        crate::zone_wire::validate_wire_asset_filename(filename.as_ref(), "lvz")?;
        if bytes.len() > crate::zone_wire::MAX_TRANSFER_FILE_BYTES {
            return Err(format!(
                "zone {id:?} LVZ {filename:?} is {} bytes; maximum is {}",
                bytes.len(),
                crate::zone_wire::MAX_TRANSFER_FILE_BYTES
            ));
        }
        let archive = crate::lvz_archive::LvzArchive::parse_structural(bytes.as_ref())
            .map_err(|err| format!("zone {id:?} LVZ {filename:?} is malformed: {err}"))?;
        let mut archive_usage = archive.render_usage()?;
        for (image_name, rgba_bytes) in archive.render_image_allocations()? {
            lvz_render_images.insert(image_name, rgba_bytes);
        }
        archive_usage.pixel_bytes = 0;
        archive_usage.texture_count = 0;
        lvz_render_usage = lvz_render_usage
            .checked_add(archive_usage)
            .map_err(|err| format!("zone {id:?} LVZ representation is invalid: {err}"))?;
        if lvz_render_usage.decoded_source_bytes > crate::zone_wire::MAX_ZONE_DECODED_LVZ_BYTES {
            return Err(format!(
                "zone {id:?} LVZ files expand to {} bytes; maximum is {}",
                lvz_render_usage.decoded_source_bytes,
                crate::zone_wire::MAX_ZONE_DECODED_LVZ_BYTES
            ));
        }
        pending_lvz.push((filename, bytes, archive));
    }
    // The exact cumulative work budget for every archive has succeeded.
    // Only now perform bounded full decodes, so a rejected late aggregate can
    // never spend decoder work on the first or second archive.
    let mut lvz_files = Vec::with_capacity(pending_lvz.len());
    for (filename, bytes, mut archive) in pending_lvz {
        archive.seal_full_renderability().map_err(|err| {
            format!("zone {id:?} LVZ {filename:?} failed full render validation: {err}")
        })?;
        lvz_files.push(ZoneFile {
            checksum: continuum_file_crc32(bytes.as_ref()),
            sha256: crate::zone_wire::sha256_digest(bytes.as_ref()),
            filename,
            bytes,
        });
    }

    let transfer_bytes = lvz_files
        .iter()
        .try_fold(compressed.len(), |total, file| {
            total.checked_add(file.bytes.len())
        })
        .ok_or_else(|| format!("zone {id:?} transfer size overflow"))?;
    if transfer_bytes > crate::zone_wire::MAX_ZONE_TRANSFER_BYTES {
        return Err(format!(
            "zone {id:?} advertises {transfer_bytes} encoded transfer bytes; maximum is {}",
            crate::zone_wire::MAX_ZONE_TRANSFER_BYTES
        ));
    }
    let source_bytes = map_bytes
        .len()
        .checked_add(settings_bytes.len())
        .and_then(|total| total.checked_add(news_bytes.len()))
        .and_then(|total| {
            lvz_files
                .iter()
                .try_fold(total, |sum, file| sum.checked_add(file.bytes.len()))
        })
        .ok_or_else(|| format!("zone {id:?} retained source size overflow"))?;
    let map_render_bytes = crate::render_limits::checked_map_representation_bytes(
        map_filename.len(),
        tiles.len(),
        map_layout.animated_record_count,
        map_layout.door_record_count,
        tiles.iter().filter(|tile| **tile != 0).count(),
        map_layout.embedded_pixel_bytes,
    )
    .and_then(|bytes| {
        bytes
            .checked_add(no_flag_drop_tiles.retained_bytes())
            .ok_or_else(|| "map region representation size overflow".to_string())
    })
    .map_err(|err| format!("zone {id:?} map representation is invalid: {err}"))?;
    lvz_render_usage
        .validate()
        .map_err(|err| format!("zone {id:?} LVZ representation is invalid: {err}"))?;
    let nonpixel_bytes = map_render_bytes
        .checked_sub(map_layout.embedded_pixel_bytes)
        .and_then(|total| total.checked_add(settings_packet.len()))
        .and_then(|total| {
            lvz_render_usage
                .retained_bytes()
                .ok()
                .and_then(|bytes| total.checked_add(bytes))
        })
        .ok_or_else(|| format!("zone {id:?} render representation size overflow"))?;
    let (decoded_bytes, _) = crate::render_limits::checked_zone_render_totals(
        nonpixel_bytes,
        map_layout.embedded_pixel_bytes,
        &lvz_render_images,
    )
    .map_err(|err| format!("zone {id:?} render representation is invalid: {err}"))?;
    crate::render_limits::checked_zone_render_peak(
        crate::zone_wire::MAX_ZONE_RENDER_BYTES,
        decoded_bytes,
        lvz_render_images.values().copied().max().unwrap_or(0),
    )
    .map_err(|err| format!("zone {id:?} render peak is invalid: {err}"))?;
    let (tiles, map_checksum, continuum_crc32, flags) = derive_zone_map_tiles(tiles);
    // Derived before the literal because `id` and `settings_packet` move into it.
    let objective_profile = crate::arena_settings::objective_profile_from_bundle(
        id.as_ref(),
        settings_bytes,
        &settings_packet,
    )?;
    Ok(ZoneMap {
        zone_id: id,
        display_name,
        filename: map_filename,
        news_filename,
        news_bytes,
        tiles,
        no_flag_drop_tiles,
        checksum: continuum_file_crc32(map_bytes.as_ref()),
        sha256: crate::zone_wire::sha256_digest(map_bytes.as_ref()),
        compressed,
        map_checksum,
        continuum_crc32,
        flags,
        objective_profile,
        lvz_files,
        settings_packet: Cow::Owned(settings_packet),
        brick_span,
        resource_usage: ZoneCatalogUsage {
            source: source_bytes,
            encoded: transfer_bytes,
            decoded: decoded_bytes,
        },
    })
}

fn derive_zone_map_tiles(tiles: Vec<u8>) -> (Vec<u8>, u32, u32, Vec<ZoneFlag>) {
    let map_checksum = map_security_checksum_for_tiles(&tiles, SECURITY_CHECKSUM_KEY);
    let continuum_crc32 = continuum_map_crc32(&tiles);
    let flags = flag_positions_from_tiles(&tiles);
    // All derived consumers are complete; transfer the sole 1 MiB tile
    // allocation into ZoneMap rather than retaining a checksum copy.
    (tiles, map_checksum, continuum_crc32, flags)
}

fn compress_map_for_transfer(id: &str, map_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if map_bytes.len() > crate::zone_wire::MAX_MAP_SOURCE_BYTES {
        return Err(format!(
            "zone {id:?} map source is {} bytes; maximum is {}",
            map_bytes.len(),
            crate::zone_wire::MAX_MAP_SOURCE_BYTES
        ));
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(map_bytes)
        .map_err(|err| format!("zone {id:?} map compression failed: {err}"))?;
    let compressed = encoder
        .finish()
        .map_err(|err| format!("zone {id:?} map compression did not finish: {err}"))?;
    if compressed.len() > crate::zone_wire::MAX_TRANSFER_FILE_BYTES {
        return Err(format!(
            "zone {id:?} compressed map is {} bytes; maximum transfer payload is {}",
            compressed.len(),
            crate::zone_wire::MAX_TRANSFER_FILE_BYTES
        ));
    }
    Ok(compressed)
}

fn embedded_zone_maps() -> Vec<ZoneMap> {
    let maps = ZONE_DEFINITIONS
        .iter()
        .map(build_zone_map)
        .collect::<Vec<_>>();
    validate_zone_catalog_identities(&maps)
        .expect("embedded zone identities should satisfy the browser catalog contract");
    catalog_usage(&maps).expect("embedded zone catalog should fit resource budgets");
    maps
}

pub(crate) fn load_server_zone_catalog(
    root: Option<&str>,
    allow_overrides: bool,
) -> Result<ServerZoneCatalog, String> {
    let maps = load_configured_zone_maps(root, allow_overrides)?;
    validate_zone_catalog_identities(&maps)?;
    let usage = catalog_usage(&maps)?;
    Ok(ServerZoneCatalog { maps, usage })
}

pub(crate) fn install_server_zone_catalog(catalog: ServerZoneCatalog) -> Result<(), String> {
    let ServerZoneCatalog { maps, usage } = catalog;
    debug_assert_eq!(catalog_usage(&maps).ok(), Some(usage));
    ZONE_MAPS
        .set(maps)
        .map_err(|_| "server zone catalog was initialized before configuration".to_string())
}

fn load_configured_zone_maps(
    root: Option<&str>,
    allow_overrides: bool,
) -> Result<Vec<ZoneMap>, String> {
    let mut maps = embedded_zone_maps();
    let Some(root) = root else {
        return Ok(maps);
    };
    let embedded_source = catalog_usage(&maps)?.source;
    let disk_source_budget = crate::zone_wire::MAX_ZONE_CATALOG_SOURCE_BYTES
        .checked_sub(embedded_source)
        .ok_or_else(|| "embedded zone catalog exceeds the source budget".to_string())?;
    let definitions = bundles::load_disk_zone_definitions_with_source_budget(
        Path::new(root),
        disk_source_budget as u64,
    )?;
    validate_disk_identities(&definitions, allow_overrides)?;
    let new_zone_count = definitions
        .iter()
        .filter(|definition| !maps.iter().any(|map| map.zone_id.as_ref() == definition.id))
        .count();
    let merged_zone_count = maps
        .len()
        .checked_add(new_zone_count)
        .ok_or_else(|| "merged zone catalog count overflow".to_string())?;
    if merged_zone_count > crate::zone_wire::MAX_ZONE_COUNT {
        return Err(format!(
            "merged zone catalog contains {merged_zone_count} zones; maximum is {}",
            crate::zone_wire::MAX_ZONE_COUNT
        ));
    }
    for definition in definitions {
        let id = definition.id;
        let lvz_files = definition
            .lvz_files
            .into_iter()
            .map(|(filename, bytes)| (Cow::Owned(filename), Cow::Owned(bytes)))
            .collect::<Vec<_>>();
        let disk_map = build_zone_map_parts(
            Cow::Owned(id.clone()),
            Cow::Owned(definition.display_name),
            Cow::Owned(definition.map_filename),
            Cow::Owned(definition.map_bytes),
            Cow::Owned(definition.news_filename),
            Cow::Owned(definition.news_bytes),
            &definition.settings_bytes,
            lvz_files,
        )?;
        if let Some(index) = maps.iter().position(|map| map.zone_id.as_ref() == id) {
            maps[index] = disk_map;
        } else {
            maps.push(disk_map);
        }
    }
    maps.sort_by(|left, right| {
        embedded_zone_priority(left.zone_id.as_ref())
            .cmp(&embedded_zone_priority(right.zone_id.as_ref()))
            .then_with(|| left.zone_id.cmp(&right.zone_id))
    });
    debug_assert!(maps.len() <= crate::zone_wire::MAX_ZONE_COUNT);
    catalog_usage(&maps)?;
    Ok(maps)
}

fn catalog_usage(maps: &[ZoneMap]) -> Result<ZoneCatalogUsage, String> {
    let mut usage = ZoneCatalogUsage::default();
    for map in maps {
        usage.source = usage
            .source
            .checked_add(map.resource_usage.source)
            .ok_or_else(|| "zone catalog source size overflow".to_string())?;
        usage.encoded = usage
            .encoded
            .checked_add(map.resource_usage.encoded)
            .ok_or_else(|| "zone catalog encoded size overflow".to_string())?;
        usage.decoded = usage
            .decoded
            .checked_add(map.resource_usage.decoded)
            .ok_or_else(|| "zone catalog decoded size overflow".to_string())?;
    }
    for (label, actual, maximum) in [
        (
            "source",
            usage.source,
            crate::zone_wire::MAX_ZONE_CATALOG_SOURCE_BYTES,
        ),
        (
            "encoded",
            usage.encoded,
            crate::zone_wire::MAX_ZONE_CATALOG_ENCODED_BYTES,
        ),
        (
            "decoded",
            usage.decoded,
            crate::zone_wire::MAX_ZONE_CATALOG_DECODED_BYTES,
        ),
    ] {
        if actual > maximum {
            return Err(format!(
                "zone catalog {label} resource usage is {actual} bytes; maximum is {maximum}"
            ));
        }
    }
    Ok(usage)
}

fn validate_disk_identities(
    definitions: &[bundles::DiskZoneDefinition],
    allow_overrides: bool,
) -> Result<(), String> {
    let mut aliases = BTreeSet::new();
    for zone in ZONE_DEFINITIONS {
        for normalized in normalized_zone_identity_aliases(zone.id, zone.display_name)? {
            if !aliases.insert(normalized.clone()) {
                return Err(format!(
                    "embedded zone {:?} identity {normalized:?} conflicts after normalization",
                    zone.id
                ));
            }
        }
    }
    for definition in definitions {
        let embedded = ZONE_DEFINITIONS
            .iter()
            .find(|zone| zone.id == definition.id);
        match (embedded, definition.override_id.as_deref(), allow_overrides) {
            (Some(_), Some(override_id), true) if override_id == definition.id => {}
            (Some(_), _, _) => {
                return Err(format!(
                    "disk zone {:?} conflicts with an embedded identity; both allow_zone_overrides=true and override={} are required",
                    definition.id, definition.id
                ));
            }
            (None, Some(_), _) => {
                return Err(format!(
                    "new disk zone {:?} declares an override but no embedded identity exists",
                    definition.id
                ));
            }
            (None, None, _) => {}
        }
        if let Some(embedded) = embedded {
            for normalized in normalized_zone_identity_aliases(embedded.id, embedded.display_name)?
            {
                aliases.remove(&normalized);
            }
        }
        for normalized in
            normalized_zone_identity_aliases(&definition.id, &definition.display_name)?
        {
            if !aliases.insert(normalized) {
                return Err(format!(
                    "disk zone identity {:?} conflicts with another loaded zone after normalization",
                    definition.id
                ));
            }
        }
    }
    Ok(())
}

fn embedded_zone_priority(zone_id: &str) -> usize {
    SERVER_ZONE_IDS
        .iter()
        .position(|embedded| *embedded == zone_id)
        .unwrap_or(SERVER_ZONE_IDS.len())
}

pub(crate) fn tile_at_world(map: &ZoneMap, x: u16, y: u16) -> u8 {
    let tile_x = ((x as f32) / TILE_SIZE_PX)
        .floor()
        .clamp(0.0, (MAP_TILES - 1) as f32) as usize;
    let tile_y = ((y as f32) / TILE_SIZE_PX)
        .floor()
        .clamp(0.0, (MAP_TILES - 1) as f32) as usize;
    map.tiles
        .get(tile_y * MAP_TILES + tile_x)
        .copied()
        .unwrap_or(20)
}

pub(crate) fn server_spawn_coordinates(map: &ZoneMap) -> (u16, u16) {
    let center = (MAP_TILES / 2) as i32;
    if let Some((x, y)) = find_spawn_tile(map, center, center, true) {
        return tile_center_coordinates(x, y);
    }
    if let Some((x, y)) = find_spawn_tile(map, center, center, false) {
        return tile_center_coordinates(x, y);
    }

    tile_center_coordinates(center, center)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RandomWarpArea {
    /// Reference frequency regions and radar-mode quadrants.
    ArenaSettings,
    /// Owner-selected FFA policy: central population-sized combat area.
    CentralCombat,
    /// Lifecycle FFA: honor explicit spawn regions, otherwise central combat.
    AuthoredOrCentral,
}

/// The reference game's PlayerManager::Spawn(false): configured frequency regions or the
/// radar-mode/default population-dependent area. The server chooses the point;
/// callers supply entropy, never a client-selected destination.
pub(crate) fn server_random_warp_coordinates(
    map: &ZoneMap,
    frequency: u16,
    player_count: usize,
    ship_radius: f32,
    area: RandomWarpArea,
    seed: &mut i32,
    blocked_tiles: &BTreeSet<(u16, u16)>,
) -> Option<(u16, u16)> {
    server_random_spawn_coordinates_from_settings(
        map,
        &map.settings_packet,
        frequency,
        player_count,
        ship_radius,
        area,
        seed,
        blocked_tiles,
    )
}

/// Shared bounded spawn search. Geometry always comes from the actual map;
/// the settings packet defines the authored region and population scaling.
#[allow(clippy::too_many_arguments)]
pub(crate) fn server_random_spawn_coordinates_from_settings(
    map: &ZoneMap,
    packet: &[u8],
    frequency: u16,
    player_count: usize,
    ship_radius: f32,
    area: RandomWarpArea,
    seed: &mut i32,
    blocked_tiles: &BTreeSet<(u16, u16)>,
) -> Option<(u16, u16)> {
    use crate::arena_settings::{
        CONTINUUM_BYTE_SETTINGS_OFFSET, CONTINUUM_GLOBAL_INTS_OFFSET,
        CONTINUUM_SHORT_SETTINGS_OFFSET,
    };
    let spawn_offset = CONTINUUM_GLOBAL_INTS_OFFSET + 80;
    let regions = (0..4)
        .filter_map(|index| {
            let offset = spawn_offset + index * 4;
            let raw = u32::from_le_bytes(packet[offset..offset + 4].try_into().unwrap());
            let x = (((raw & 0x3ff) as i16) << 6) >> 6;
            let y = ((((raw >> 10) & 0x3ff) as i16) << 6) >> 6;
            let radius = ((raw >> 20) & 0x1ff) as i32;
            (x != 0 || y != 0 || radius != 0).then_some((x, y, radius))
        })
        .collect::<Vec<_>>();
    let region = (area != RandomWarpArea::CentralCombat && !regions.is_empty())
        .then(|| regions[frequency as usize % regions.len()]);
    let center_coord = |v: i16| {
        if v == 0 {
            512
        } else if v < 0 {
            i32::from(v) + 1024
        } else {
            i32::from(v)
        }
    };
    let radar_mode = if area != RandomWarpArea::ArenaSettings {
        0
    } else {
        packet[CONTINUUM_BYTE_SETTINGS_OFFSET + 18]
    };
    let limit = i16::from_le_bytes(
        packet[CONTINUUM_SHORT_SETTINGS_OFFSET + 68..CONTINUUM_SHORT_SETTINGS_OFFSET + 70]
            .try_into()
            .unwrap(),
    )
    .max(3) as i32;
    let radius = ((((player_count.min(1024) / 8) * 0x2000 + 0x400) / 0x60 + 0x100) as i32)
        .min(limit)
        .max(3);
    // Conservative full-tile clearance also excludes closed doors and map edges.
    let clearance = (ship_radius / TILE_SIZE_PX).ceil().max(1.0) as i32;
    let fits = |x: i32, y: i32| {
        tile_at(map, x, y) != WORMHOLE_TILE_ID
            && (-clearance..=clearance).all(|dy| {
                (-clearance..=clearance).all(|dx| {
                    !solid_tile(tile_at(map, x + dx, y + dy))
                        && !blocked_tiles.contains(&((x + dx) as u16, (y + dy) as u16))
                })
            })
    };
    for _ in 0..100 {
        let mut next = || crate::vie::vie_rng_next(seed);
        let (x, y) = if let Some((x, y, radius)) = region {
            let center = (center_coord(x), center_coord(y));
            if radius == 0 {
                center
            } else {
                (
                    center.0 + (next() % (radius as u32 * 2)) as i32 - radius,
                    center.1 + (next() % (radius as u32 * 2)) as i32 - radius,
                )
            }
        } else {
            match radar_mode {
                1 | 3 => (
                    (frequency & 1) as i32 * 0x300 + next() as u8 as i32,
                    next() as u8 as i32 + 0x100,
                ),
                2 | 4 => (
                    (frequency & 1) as i32 * 0x300 + next() as u8 as i32,
                    ((frequency / 2) & 1) as i32 * 0x300 + next() as u8 as i32,
                ),
                _ => {
                    let offset = (1024 - radius) / 2;
                    let x =
                        (next() % (radius - 2) as u32) as i32 - 9 + offset + (next() % 20) as i32;
                    let y =
                        (next() % (radius - 2) as u32) as i32 - 9 + offset + (next() % 20) as i32;
                    (x, y)
                }
            }
        };
        if fits(x, y) {
            return Some(tile_center_coordinates(x, y));
        }
    }
    // A bounded search may fail in a blocked authored region. Check the
    // ordinary fallback with the SAME hull/brick predicate; otherwise reject
    // rather than teleporting into terrain or a temporary hostile wall.
    let fallback = match region {
        Some((x, y, _)) => tile_center_coordinates(center_coord(x), center_coord(y)),
        None => server_spawn_coordinates(map),
    };
    fits(i32::from(fallback.0) / 16, i32::from(fallback.1) / 16).then_some(fallback)
}

/// Optional map-distribution measurement, not an acceptance gate. Use fresh,
/// reproducible seeds as real activations use fresh OS entropy.
#[test]
#[ignore = "diagnostic random-warp distribution measurement"]
fn random_warp_distribution_probe() {
    let map = server_zone_maps()
        .iter()
        .find(|map| map.zone_id == "chaos-zone")
        .unwrap();
    let fallback = server_spawn_coordinates(map);
    let blocked = BTreeSet::new();
    let mut rows = Vec::new();
    for area in [RandomWarpArea::ArenaSettings, RandomWarpArea::CentralCombat] {
        for frequency in 0..4 {
            let mut points = std::collections::BTreeMap::<(u16, u16), usize>::new();
            let mut rejected = 0;
            for index in 0_u32..1024 {
                let mut seed = 0x1357_9bdf_u32.wrapping_add(index.wrapping_mul(0x9e37_79b9)) as i32;
                if let Some(point) = server_random_warp_coordinates(
                    map, frequency, 16, 14.0, area, &mut seed, &blocked,
                ) {
                    *points.entry(point).or_default() += 1;
                } else {
                    rejected += 1;
                }
            }
            let mut popular = points
                .iter()
                .map(|(&point, &count)| (point, count))
                .collect::<Vec<_>>();
            popular.sort_unstable_by_key(|&(point, count)| (std::cmp::Reverse(count), point));
            rows.push(serde_json::json!({
            "area": format!("{area:?}"), "frequency": frequency, "samples": 1024, "distinct": points.len(), "rejected": rejected,
            "fallbackPoint": fallback, "fallbackOccurrences": points.get(&fallback).copied().unwrap_or(0),
            "boundsPixels": {
                "minX": points.keys().map(|point| point.0).min(), "maxX": points.keys().map(|point| point.0).max(),
                "minY": points.keys().map(|point| point.1).min(), "maxY": points.keys().map(|point| point.1).max(),
            },
            "mostCommon": popular.into_iter().take(5).collect::<Vec<_>>(),
        }));
        }
    }
    println!(
        "warp_distribution {}",
        serde_json::json!({"zone": map.zone_id, "playerCount": 16, "shipRadiusPixels": 14, "frequencies": rows})
    );
}

#[test]
fn chaos_shared_spawn_range_stays_close_to_k11_at_every_population_and_frequency() {
    let map = server_zone_maps()
        .iter()
        .find(|map| map.zone_id == "chaos-zone")
        .unwrap();
    let offset = crate::arena_settings::CONTINUUM_SHORT_SETTINGS_OFFSET + 68;
    assert_eq!(
        i16::from_le_bytes(map.settings_packet[offset..offset + 2].try_into().unwrap()),
        176,
        "one authored WarpRadiusLimit applies to all ships and populations"
    );
    let mut points = BTreeSet::new();
    let mut quadrants = BTreeSet::new();
    for index in 0_u32..128 {
        let initial_seed = 0x1357_9bdf_u32.wrapping_add(index.wrapping_mul(0x9e37_79b9)) as i32;
        let mut expected = None;
        for count in [0, 1, 15, 40, 500, 1024] {
            for frequency in [0, 1, 768, 65534] {
                for area in [
                    RandomWarpArea::CentralCombat,
                    RandomWarpArea::AuthoredOrCentral,
                ] {
                    let mut seed = initial_seed;
                    let point = server_random_warp_coordinates(
                        map,
                        frequency,
                        count,
                        14.0,
                        area,
                        &mut seed,
                        &BTreeSet::new(),
                    )
                    .expect("the central Chaos area has clear spawn positions");
                    if let Some(expected) = expected {
                        assert_eq!(
                            point, expected,
                            "neither population nor FFA frequency widens the range"
                        );
                    } else {
                        expected = Some(point);
                    }
                    // The reference game's 176-tile width plus its asymmetric jitter
                    // reaches tiles415..607. Use an independent world-distance
                    // bound, including tile centres, rather than just checking
                    // the same modulo arithmetic as the sampler.
                    let dx = f32::from(point.0) - 8192.0;
                    let dy = f32::from(point.1) - 8192.0;
                    assert!(dx.abs() <= 1552.0 && dy.abs() <= 1552.0, "{point:?}");
                    assert!(dx.hypot(dy) < 2200.0, "spawn too far from K11: {point:?}");
                    points.insert(point);
                    quadrants.insert((dx >= 0.0, dy >= 0.0));
                }
            }
        }
    }
    assert!(
        points.len() > 110,
        "keep varied destinations rather than one fixed spawn"
    );
    assert_eq!(quadrants.len(), 4, "sample in all directions around K11");
}

#[test]
fn central_ffa_random_warp_is_frequency_independent_and_covers_the_combat_area() {
    let map = server_zone_maps()
        .iter()
        .find(|map| map.zone_id == "chaos-zone")
        .unwrap();
    let blocked = BTreeSet::new();
    let mut points = BTreeSet::new();
    let mut quadrants = BTreeSet::new();
    for index in 0_u32..200 {
        let initial_seed = 0x1357_9bdf_u32.wrapping_add(index.wrapping_mul(0x9e37_79b9)) as i32;
        let mut expected = None;
        for frequency in [0, 1, 2, 3, 768, 65534] {
            let mut seed = initial_seed;
            let point = server_random_warp_coordinates(
                map,
                frequency,
                16,
                14.0,
                RandomWarpArea::CentralCombat,
                &mut seed,
                &blocked,
            )
            .unwrap();
            if let Some(expected) = expected {
                assert_eq!(
                    point, expected,
                    "FFA frequency must not choose a fixed corner"
                );
            } else {
                expected = Some(point);
            }
            assert!(
                (4096..12288).contains(&point.0) && (4096..12288).contains(&point.1),
                "central 16-player area: {point:?}"
            );
            points.insert(point);
            quadrants.insert((point.0 >= 8192, point.1 >= 8192));
        }
    }
    assert!(
        points.len() > 180,
        "fresh seeds must vary destinations, got {}",
        points.len()
    );
    assert_eq!(
        quadrants.len(),
        4,
        "warps must reach all four sides of center"
    );
}

#[test]
fn ffa_lifecycle_spawns_preserve_authored_regions_without_changing_random_warp() {
    let map = server_zone_maps()
        .iter()
        .find(|map| map.zone_id == "chaos-zone")
        .unwrap();
    let first = find_spawn_tile(map, 450, 450, true).unwrap();
    let second = find_spawn_tile(map, 570, 570, true).unwrap();
    assert_ne!(first, second);
    let mut settings = map.settings_packet.to_vec();
    let offset = crate::arena_settings::CONTINUUM_GLOBAL_INTS_OFFSET + 80;
    settings[offset..offset + 16].fill(0);
    for (index, (x, y)) in [first, second].into_iter().enumerate() {
        let raw = (x as u32 & 0x3ff) | ((y as u32 & 0x3ff) << 10);
        settings[offset + index * 4..offset + index * 4 + 4].copy_from_slice(&raw.to_le_bytes());
    }
    for frequency in [0, 1, 2, 3, 768, 65534] {
        let mut seed = 123456;
        let point = server_random_spawn_coordinates_from_settings(
            map,
            &settings,
            frequency,
            16,
            14.0,
            RandomWarpArea::AuthoredOrCentral,
            &mut seed,
            &BTreeSet::new(),
        )
        .unwrap();
        let expected = [first, second][frequency as usize % 2];
        assert_eq!(
            point,
            tile_center_coordinates(expected.0, expected.1),
            "explicit authored regions precede the FFA corner override"
        );
        // Shift+0 retains the owner's wider central policy even with regions.
        let mut actual_seed = 123456;
        let actual = server_random_spawn_coordinates_from_settings(
            map,
            &settings,
            frequency,
            16,
            14.0,
            RandomWarpArea::CentralCombat,
            &mut actual_seed,
            &BTreeSet::new(),
        );
        let mut original_seed = 123456;
        let original = server_random_warp_coordinates(
            map,
            frequency,
            16,
            14.0,
            RandomWarpArea::CentralCombat,
            &mut original_seed,
            &BTreeSet::new(),
        );
        assert_eq!(
            actual, original,
            "explicit lifecycle regions must not change random warp"
        );
    }
}

#[test]
fn zero_region_ffa_lifecycle_uses_the_existing_frequency_independent_central_sampler() {
    let map = server_zone_maps()
        .iter()
        .find(|map| map.zone_id == "chaos-zone")
        .unwrap();
    let offset = crate::arena_settings::CONTINUUM_GLOBAL_INTS_OFFSET + 80;
    assert!(
        map.settings_packet[offset..offset + 16]
            .iter()
            .all(|byte| *byte == 0)
    );
    for initial_seed in [1, 123456, -123456, i32::MAX, i32::MIN] {
        let mut central_seed = initial_seed;
        let expected = server_random_warp_coordinates(
            map,
            0,
            16,
            14.0,
            RandomWarpArea::CentralCombat,
            &mut central_seed,
            &BTreeSet::new(),
        );
        for frequency in [0, 1, 2, 3, 768, 65534] {
            let mut seed = initial_seed;
            let actual = server_random_warp_coordinates(
                map,
                frequency,
                16,
                14.0,
                RandomWarpArea::AuthoredOrCentral,
                &mut seed,
                &BTreeSet::new(),
            );
            assert_eq!(
                actual, expected,
                "zero-region lifecycle spawns ignore RadarMode4 frequency corners"
            );
            assert_eq!(
                seed, central_seed,
                "reuse the same bounded candidate stream"
            );
        }
    }
}

#[test]
fn blocked_authored_lifecycle_region_cannot_fall_back_outside_its_region() {
    let map = server_zone_maps()
        .iter()
        .find(|map| map.zone_id == "chaos-zone")
        .unwrap();
    let authored = find_spawn_tile(map, 450, 450, true).unwrap();
    assert_ne!(
        tile_center_coordinates(authored.0, authored.1),
        server_spawn_coordinates(map)
    );
    let mut settings = map.settings_packet.to_vec();
    let offset = crate::arena_settings::CONTINUUM_GLOBAL_INTS_OFFSET + 80;
    settings[offset..offset + 16].fill(0);
    let raw = (authored.0 as u32 & 0x3ff) | ((authored.1 as u32 & 0x3ff) << 10);
    settings[offset..offset + 4].copy_from_slice(&raw.to_le_bytes());
    let mut seed = 123456;
    assert_eq!(
        server_random_spawn_coordinates_from_settings(
            map,
            &settings,
            1,
            16,
            14.0,
            RandomWarpArea::AuthoredOrCentral,
            &mut seed,
            &BTreeSet::from([(authored.0 as u16, authored.1 as u16)]),
        ),
        None,
        "a clear global centre cannot override a blocked explicit region"
    );
}

#[test]
fn extreme_ffa_lifecycle_spawns_stay_inside_the_shipped_explicit_region() {
    let map = server_zone_maps()
        .iter()
        .find(|map| map.zone_id == EXTREME_ZONE_ID)
        .unwrap();
    // [Spawn] Team0..3: X=-512, Y=-512, Radius=2. The reference game interprets
    // negative coordinates relative to 1024 and samples [centre-r, centre+r).
    let offset = crate::arena_settings::CONTINUUM_GLOBAL_INTS_OFFSET + 80;
    let raw = 512_u32 | (512_u32 << 10) | (2_u32 << 20);
    for index in 0..4 {
        assert_eq!(
            &map.settings_packet[offset + index * 4..offset + index * 4 + 4],
            &raw.to_le_bytes()
        );
    }
    let mut destinations = BTreeSet::new();
    for index in 0_u32..100 {
        let mut seed = 0x1357_9bdf_u32.wrapping_add(index.wrapping_mul(0x9e37_79b9)) as i32;
        let point = server_random_warp_coordinates(
            map,
            index as u16,
            16,
            14.0,
            RandomWarpArea::AuthoredOrCentral,
            &mut seed,
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(
            (510..514).contains(&(point.0 / 16)) && (510..514).contains(&(point.1 / 16)),
            "explicit Extreme lifecycle region: {point:?}"
        );
        destinations.insert(point);
    }
    assert!(
        destinations.len() > 4,
        "the authored region remains randomized"
    );
}

#[test]
fn random_warp_destinations_vary_and_clear_the_ship_radius() {
    let map = &server_zone_maps()[0];
    let mut seed = 123456;
    let mut destinations = std::collections::BTreeSet::new();
    for _ in 0..200 {
        let (x, y) = server_random_warp_coordinates(
            map,
            0,
            8,
            14.0,
            RandomWarpArea::ArenaSettings,
            &mut seed,
            &BTreeSet::new(),
        )
        .expect("shipped map has clear spawn space");
        destinations.insert((x, y));
        let (tx, ty) = (i32::from(x) / 16, i32::from(y) / 16);
        for dy in -1..=1 {
            for dx in -1..=1 {
                assert!(
                    !solid_tile(tile_at(map, tx + dx, ty + dy)),
                    "blocked random warp at {x},{y}"
                );
            }
        }
        assert_ne!(tile_at(map, tx, ty), WORMHOLE_TILE_ID);
    }
    assert!(
        destinations.len() > 100,
        "warp must not always use the fixed spawn"
    );
}

#[test]
fn random_warp_avoids_brick_hulls_and_rejects_blocked_bounded_fallback() {
    for area in [RandomWarpArea::ArenaSettings, RandomWarpArea::CentralCombat] {
        let map = &server_zone_maps()[0];
        let mut seed = 123456;
        let first =
            server_random_warp_coordinates(map, 0, 8, 14.0, area, &mut seed, &BTreeSet::new())
                .unwrap();
        // A neighboring brick, not just a brick under the center, must exclude a
        // candidate whose ship hull would overlap the wall.
        let mut blocked = BTreeSet::from([(first.0 / 16 + 1, first.1 / 16)]);
        seed = 123456;
        let next =
            server_random_warp_coordinates(map, 0, 8, 14.0, area, &mut seed, &blocked).unwrap();
        assert_ne!(next, first);
        // Replay the same bounded candidate stream, blocking each accepted point.
        // This exhausts at most 100 candidates plus the ordinary fallback.
        for _ in 0..101 {
            seed = 123456;
            match server_random_warp_coordinates(map, 0, 8, 14.0, area, &mut seed, &blocked) {
                Some((x, y)) => {
                    blocked.insert((x / 16, y / 16));
                }
                None => break,
            }
        }
        seed = 123456;
        assert_eq!(
            server_random_warp_coordinates(map, 0, 8, 14.0, area, &mut seed, &blocked),
            None
        );
    }
}

fn find_spawn_tile(
    map: &ZoneMap,
    center_x: i32,
    center_y: i32,
    require_clearance: bool,
) -> Option<(i32, i32)> {
    let max_radius = (MAP_TILES / 2) as i32;
    // Shared outward-radius search algorithm — keeps client + server
    // spawn-tile selection order identical.
    crate::continuum_position::find_spawn_tile(center_x, center_y, max_radius, |x, y| {
        !can_spawn_at(map, x, y, require_clearance)
    })
}

fn can_spawn_at(map: &ZoneMap, x: i32, y: i32, require_clearance: bool) -> bool {
    if solid_tile(tile_at(map, x, y)) {
        return false;
    }
    if !require_clearance {
        return true;
    }

    for dy in -1..=1 {
        for dx in -1..=1 {
            if solid_tile(tile_at(map, x + dx, y + dy)) {
                return false;
            }
        }
    }

    true
}

fn tile_at(map: &ZoneMap, x: i32, y: i32) -> u8 {
    if x < 0 || y < 0 || x >= MAP_TILES as i32 || y >= MAP_TILES as i32 {
        return 20;
    }
    map.tiles
        .get(y as usize * MAP_TILES + x as usize)
        .copied()
        .unwrap_or(20)
}

pub(crate) fn solid_tile(id: u8) -> bool {
    // Mirrors `src/tile.rs::is_solid_tile` exactly. The two implementations
    // can't yet share code because `tile.rs` pulls in macroquad, but the
    // `solid_tile_matches_client_is_solid_tile` test below asserts byte-by-
    // byte parity over all 256 possible tile ids so they cannot drift.
    if id == 0 {
        return false;
    }
    if (DOOR_FIRST_TILE_ID..=DOOR_LAST_TILE_ID).contains(&id) {
        return true;
    }
    if id < 170 {
        return true;
    }
    if id == WORMHOLE_TILE_ID {
        return false;
    }
    if (192..=240).contains(&id) {
        return true;
    }
    if (242..=252).contains(&id) {
        return true;
    }

    false
}

#[cfg(test)]
#[test]
fn solid_tile_matches_client_is_solid_tile_for_all_inputs() {
    // Independent copy of the client's tile-classification rule. If either
    // side adds/removes a solid range (e.g. a new map feature), this test
    // will fail until both sides are updated, preventing the
    // collision-mismatch class of bug across server/client.
    fn client_is_solid_tile(id: u8) -> bool {
        if id == 0 {
            return false;
        }
        if (162..=169).contains(&id) {
            return true;
        }
        if id < 170 {
            return true;
        }
        if id == WORMHOLE_TILE_ID {
            return false;
        }
        if (192..=240).contains(&id) {
            return true;
        }
        if (242..=252).contains(&id) {
            return true;
        }
        false
    }
    for id in 0..=u8::MAX {
        assert_eq!(
            solid_tile(id),
            client_is_solid_tile(id),
            "solid_tile/is_solid_tile disagree for id {id}"
        );
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn position_blocking_tile(id: u8) -> bool {
    position_blocking_tile_for_door_seeds(id, &[])
}

pub(crate) fn position_blocking_tile_for_door_seeds(id: u8, door_seeds: &[u8]) -> bool {
    if (DOOR_FIRST_TILE_ID..=DOOR_LAST_TILE_ID).contains(&id) {
        // No seed context (e.g. pre-handshake or before the first map sync) must
        // not let a client walk through doors — fail safe by blocking.
        if door_seeds.is_empty() {
            return true;
        }
        return !door_seeds
            .iter()
            .any(|&seed| !position_blocking_tile_with_door_seed(id, seed));
    }
    solid_tile(id)
}

pub(crate) fn position_blocking_tile_with_door_seed(id: u8, door_seed: u8) -> bool {
    if let Some(seeded) = seeded_door_tile(id, door_seed) {
        return seeded != OPEN_DOOR_TILE_ID;
    }
    solid_tile(id)
}

fn tile_center_coordinates(x: i32, y: i32) -> (u16, u16) {
    // Mirrors `src/tile.rs::tile_center` — the formula `(t + 0.5) * TILE_SIZE`
    // is the canonical Continuum tile-center math. The parity test below
    // pins the formula so server and client cannot drift on tile geometry
    // (which would silently misplace prizes, bricks, and spawn coords).
    let world_x = ((x as f32 + 0.5) * TILE_SIZE_PX).round();
    let world_y = ((y as f32 + 0.5) * TILE_SIZE_PX).round();
    (
        world_x.clamp(0.0, u16::MAX as f32) as u16,
        world_y.clamp(0.0, u16::MAX as f32) as u16,
    )
}

#[cfg(test)]
#[test]
fn tile_center_coordinates_matches_canonical_formula() {
    // Independent reference: `(t + 0.5) * TILE_SIZE`, the same formula as
    // `src/tile.rs::tile_center`. Probe representative tile indices.
    let probes: [(i32, i32); 5] = [(0, 0), (1, 1), (15, 23), (511, 1023), (1023, 0)];
    for (tx, ty) in probes {
        let (cx, cy) = tile_center_coordinates(tx, ty);
        let expected_x = ((tx as f32 + 0.5) * 16.0)
            .round()
            .clamp(0.0, u16::MAX as f32) as u16;
        let expected_y = ((ty as f32 + 0.5) * 16.0)
            .round()
            .clamp(0.0, u16::MAX as f32) as u16;
        assert_eq!(
            cx, expected_x,
            "tile_center_coordinates x drift at {tx},{ty}"
        );
        assert_eq!(
            cy, expected_y,
            "tile_center_coordinates y drift at {tx},{ty}"
        );
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn world_position_in_safe_tile(x: u16, y: u16) -> bool {
    world_position_in_safe_tile_for_map(zone_map(), x, y)
}

pub(crate) fn world_position_in_safe_tile_for_map(map: &ZoneMap, x: u16, y: u16) -> bool {
    tile_at_world(map, x, y) == SAFE_TILE_ID
}

/// Match the browser's raw-f32 `Map::tile_at_world` lookup without first
/// rounding through the legacy u16 position wire format. Canonical movement
/// replay owns the exact f32 position, and a coordinate such as 8431.9 is
/// already inside tile 526 even though its serialized coordinate rounds to
/// 8432 (tile 527). Out-of-map/non-finite coordinates fail closed just as the
/// browser map lookup returns its blocking sentinel outside the map.
pub(crate) fn world_position_f32_in_safe_tile_for_map(map: &ZoneMap, x: f32, y: f32) -> bool {
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return false;
    }
    let tile_x = (x / TILE_SIZE_PX).floor() as usize;
    let tile_y = (y / TILE_SIZE_PX).floor() as usize;
    if tile_x >= MAP_TILES || tile_y >= MAP_TILES {
        return false;
    }
    map.tiles.get(tile_y * MAP_TILES + tile_x).copied() == Some(SAFE_TILE_ID)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn world_position_touches_goal(x: u16, y: u16) -> bool {
    world_position_touches_goal_for_map(zone_map(), x, y)
}

pub(crate) fn world_position_touches_goal_for_map(map: &ZoneMap, x: u16, y: u16) -> bool {
    let center_x = x as f32;
    let center_y = y as f32;
    let tile_x = (center_x / TILE_SIZE_PX).floor() as i32;
    let tile_y = (center_y / TILE_SIZE_PX).floor() as i32;
    let radius_tiles = (GOAL_TOUCH_RADIUS_PX / TILE_SIZE_PX).ceil() as i32 + 1;

    for y in (tile_y - radius_tiles)..=(tile_y + radius_tiles) {
        for x in (tile_x - radius_tiles)..=(tile_x + radius_tiles) {
            if x < 0 || y < 0 || x >= MAP_TILES as i32 || y >= MAP_TILES as i32 {
                continue;
            }
            let tile_index = y as usize * MAP_TILES + x as usize;
            if map.tiles.get(tile_index).copied() != Some(GOAL_TILE_ID) {
                continue;
            }
            let goal_center_x = (x as f32 + 0.5) * TILE_SIZE_PX;
            let goal_center_y = (y as f32 + 0.5) * TILE_SIZE_PX;
            if (goal_center_x - center_x).hypot(goal_center_y - center_y) <= GOAL_TOUCH_RADIUS_PX {
                return true;
            }
        }
    }

    false
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn large_position_packet_in_safe_tile(packet: &[u8]) -> bool {
    large_position_packet_in_safe_tile_for_map(zone_map(), packet)
}

pub(crate) fn large_position_packet_in_safe_tile_for_map(map: &ZoneMap, packet: &[u8]) -> bool {
    if packet.len() < 17 || packet[0] != crate::arena::LARGE_POSITION_PACKET {
        return false;
    }
    let x = u16::from_le_bytes([packet[4], packet[5]]);
    let y = u16::from_le_bytes([packet[15], packet[16]]);
    world_position_in_safe_tile_for_map(map, x, y)
}

pub(crate) fn map_information(map: &ZoneMap) -> Vec<u8> {
    let mut payload = vec![
        0;
        1 + crate::zone_wire::AUTHENTICATED_MAP_INFO_HEADER_BYTES
            + map.lvz_files.len()
                * crate::zone_wire::AUTHENTICATED_MAP_INFO_FILE_BYTES
    ];
    payload[0] = MAP_INFORMATION;
    payload[1..5].copy_from_slice(&crate::zone_wire::AUTHENTICATED_MAP_INFO_MAGIC);
    payload[5] = crate::zone_wire::AUTHENTICATED_MAP_INFO_VERSION;
    copy_fixed(&mut payload[6..22], map.zone_id.as_bytes());
    copy_fixed(&mut payload[22..38], map.filename.as_bytes());
    payload[38..42].copy_from_slice(&map.checksum.to_le_bytes());
    payload[42..46].copy_from_slice(&(map.compressed.len() as u32).to_le_bytes());
    payload[46..78].copy_from_slice(&map.sha256);
    payload[78..80].copy_from_slice(&((1 + map.lvz_files.len()) as u16).to_le_bytes());
    for (index, lvz) in map.lvz_files.iter().enumerate() {
        let offset = 80 + index * crate::zone_wire::AUTHENTICATED_MAP_INFO_FILE_BYTES;
        copy_fixed(&mut payload[offset..offset + 16], lvz.filename.as_bytes());
        payload[offset + 16..offset + 20].copy_from_slice(&lvz.checksum.to_le_bytes());
        payload[offset + 20..offset + 24].copy_from_slice(&(lvz.bytes.len() as u32).to_le_bytes());
        payload[offset + 24..offset + 56].copy_from_slice(&lvz.sha256);
    }
    payload
}

pub(crate) fn compressed_map(map: &ZoneMap) -> Vec<u8> {
    compressed_map_packet(&map.compressed)
}

pub(crate) fn zone_file_download(index: u16, map: &ZoneMap) -> Option<Vec<u8>> {
    if index == 0 {
        return Some(compressed_map(map));
    }
    map.lvz_files
        .get(index as usize - 1)
        .map(|file| compressed_map_packet(file.bytes.as_ref()))
}

pub(crate) fn server_file_transfer(filename: &str, data: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(17 + data.len());
    payload.push(SERVER_FILE_TRANSFER);
    payload.resize(17, 0);
    copy_fixed(&mut payload[1..17], filename.as_bytes());
    payload.extend_from_slice(data);
    payload
}

fn compressed_map_packet(bytes: &[u8]) -> Vec<u8> {
    let mut payload = vec![0; COMPRESSED_MAP_HEADER_SIZE];
    payload[0] = COMPRESSED_MAP;
    payload.extend_from_slice(bytes);
    payload
}

fn flag_positions_from_tiles(tiles: &[u8]) -> Vec<ZoneFlag> {
    tiles
        .iter()
        .enumerate()
        .filter(|&(_, &tile)| tile == FLAG_TILE_ID)
        .map(|(index, _)| {
            let x = (index % MAP_TILES) as f32;
            let y = (index / MAP_TILES) as f32;
            (x, y)
        })
        .enumerate()
        .map(|(id, (x, y))| ZoneFlag {
            id: id.min(u16::MAX as usize) as u16,
            x: ((x + 0.5) * TILE_SIZE_PX)
                .round()
                .clamp(0.0, u16::MAX as f32) as u16,
            y: ((y + 0.5) * TILE_SIZE_PX)
                .round()
                .clamp(0.0, u16::MAX as f32) as u16,
            owner: u16::MAX,
        })
        .collect()
}

// `continuum_map_crc32` lives in `src/continuum.rs` (the canonical) and is
// re-imported here via `wire::*`. Keeping a single implementation prevents
// server-computed checksums from drifting from the client's.

fn parse_lvl_tiles(data: &[u8]) -> Result<Vec<u8>, String> {
    let offset = lvl_tile_record_offset(data)?;
    let tile_bytes = &data[offset..];
    if !tile_bytes.len().is_multiple_of(4) {
        return Err("LVL tile data is not aligned to 4-byte tile records".to_string());
    }

    let mut tiles = vec![0; MAP_TILES * MAP_TILES];
    for record in tile_bytes.chunks_exact(4) {
        let packed = u32::from_le_bytes([record[0], record[1], record[2], record[3]]);
        let x = (packed & 0x0fff) as usize;
        let y = ((packed >> 12) & 0x0fff) as usize;
        let id = (packed >> 24) as u8;
        if x >= MAP_TILES || y >= MAP_TILES {
            return Err(format!("LVL tile coordinate out of bounds: {x},{y}"));
        }

        let size = animated_tile_size(id).unwrap_or(1);
        for dy in 0..size {
            for dx in 0..size {
                let ax = x + dx;
                let ay = y + dy;
                if ax < MAP_TILES && ay < MAP_TILES {
                    tiles[ay * MAP_TILES + ax] = id;
                }
            }
        }
    }

    Ok(tiles)
}

fn parse_no_flag_drop_tiles(data: &[u8]) -> Result<NoFlagDropTiles, String> {
    if data.len() < 10 || &data[..2] != b"BM" {
        return Ok(NoFlagDropTiles::default());
    }
    let metadata_offset =
        u32::from_le_bytes(data[6..10].try_into().expect("checked BMP header")) as usize;
    if metadata_offset == 0 {
        return Ok(NoFlagDropTiles::default());
    }
    let header_end = metadata_offset
        .checked_add(12)
        .ok_or_else(|| "ELVL metadata header offset overflow".to_string())?;
    if header_end > data.len() || &data[metadata_offset..metadata_offset + 4] != b"elvl" {
        return Err("BMP metadata pointer does not name an ELVL header".to_string());
    }
    let total_size = u32::from_le_bytes(
        data[metadata_offset + 4..metadata_offset + 8]
            .try_into()
            .expect("checked ELVL header"),
    ) as usize;
    if total_size < 12 {
        return Err("ELVL metadata size is shorter than its header".to_string());
    }
    let metadata_end = metadata_offset
        .checked_add(total_size)
        .filter(|end| *end <= data.len())
        .ok_or_else(|| "ELVL metadata extends past the map source".to_string())?;
    let mut result = NoFlagDropTiles::default();
    let mut offset = header_end;
    while offset < metadata_end {
        let (kind, payload, next) = lvl_chunk(data, offset, metadata_end)?;
        if kind == b"REGN" {
            parse_no_flag_region(payload, &mut result)?;
        }
        offset = next;
    }
    Ok(result)
}

fn lvl_chunk(data: &[u8], offset: usize, end: usize) -> Result<(&[u8], &[u8], usize), String> {
    let header_end = offset
        .checked_add(8)
        .filter(|header_end| *header_end <= end)
        .ok_or_else(|| "ELVL chunk has a truncated header".to_string())?;
    let size = u32::from_le_bytes(
        data[offset + 4..header_end]
            .try_into()
            .expect("checked chunk header"),
    ) as usize;
    let payload_end = header_end
        .checked_add(size)
        .filter(|payload_end| *payload_end <= end)
        .ok_or_else(|| "ELVL chunk payload exceeds its parent".to_string())?;
    let padded = size
        .checked_add(3)
        .map(|size| size & !3)
        .ok_or_else(|| "ELVL chunk padding overflow".to_string())?;
    let next = header_end
        .checked_add(padded)
        .filter(|next| *next <= end)
        .ok_or_else(|| "ELVL chunk padding exceeds its parent".to_string())?;
    Ok((
        &data[offset..offset + 4],
        &data[header_end..payload_end],
        next,
    ))
}

fn parse_no_flag_region(payload: &[u8], result: &mut NoFlagDropTiles) -> Result<(), String> {
    let mut has_name = false;
    let mut no_flag_drops = false;
    let mut tile_data = Vec::new();
    let mut offset = 0;
    while offset < payload.len() {
        let (kind, chunk, next) = lvl_chunk(payload, offset, payload.len())?;
        match kind {
            // ExtendedLvl only registers named regions. Keep the same boundary
            // so malformed anonymous metadata cannot silently create policy.
            b"rNAM" => has_name = !chunk.is_empty(),
            b"rNFL" => no_flag_drops = true,
            b"rTIL" => tile_data.push(chunk),
            _ => {}
        }
        offset = next;
    }
    if has_name && no_flag_drops {
        for encoded in tile_data {
            decode_no_flag_region_tiles(encoded, result)?;
        }
    }
    Ok(())
}

fn decode_no_flag_region_tiles(encoded: &[u8], result: &mut NoFlagDropTiles) -> Result<(), String> {
    let mut offset = 0;
    let mut x = 0_usize;
    let mut y = 0_usize;
    let mut last_row = Vec::<(usize, usize)>::new();
    let mut last_row_y = None;
    while offset < encoded.len() {
        if x > MAP_TILES || y > MAP_TILES {
            return Err("ELVL region RLE coordinate exceeds the map".to_string());
        }
        let first = encoded[offset];
        offset += 1;
        let operation = first >> 6;
        let count = if first & 0x20 == 0 {
            usize::from(first & 0x1f) + 1
        } else {
            let second = *encoded
                .get(offset)
                .ok_or_else(|| "ELVL region RLE has a truncated count".to_string())?;
            offset += 1;
            (usize::from(first & 0x03) << 8 | usize::from(second)) + 1
        };
        match operation {
            0 => x = x.saturating_add(count),
            1 => {
                if y >= MAP_TILES || x.checked_add(count).is_none_or(|end| end > MAP_TILES) {
                    return Err("ELVL region RLE present run exceeds the map".to_string());
                }
                if last_row_y != Some(y) {
                    last_row.clear();
                    last_row_y = Some(y);
                }
                last_row.push((x, count));
                for tile_x in x..x + count {
                    result.insert(tile_x, y);
                }
                x += count;
            }
            2 => {
                if x != 0 {
                    return Err("ELVL region RLE empty-row run starts mid-row".to_string());
                }
                y = y.saturating_add(count);
            }
            3 => {
                if x != 0 || y == 0 || y.checked_add(count).is_none_or(|end| end > MAP_TILES) {
                    return Err("ELVL region RLE row repeat is invalid".to_string());
                }
                for repeated_y in y..y + count {
                    for &(start_x, width) in &last_row {
                        for tile_x in start_x..start_x + width {
                            result.insert(tile_x, repeated_y);
                        }
                    }
                }
                y += count;
                last_row_y = Some(y - 1);
            }
            _ => unreachable!(),
        }
        if x == MAP_TILES {
            x = 0;
            y += 1;
        }
    }
    if x != 0 || y != MAP_TILES {
        return Err("ELVL region RLE does not cover exactly 1024 rows".to_string());
    }
    Ok(())
}

fn lvl_tile_record_offset(data: &[u8]) -> Result<usize, String> {
    if data.len() < 6 || &data[0..2] != b"BM" {
        return Ok(0);
    }

    let offset =
        u32::from_le_bytes(data[2..6].try_into().expect("slice length is checked")) as usize;
    if offset > data.len() {
        return Err("LVL tile offset points past end of file".to_string());
    }
    Ok(offset)
}

fn animated_tile_size(id: u8) -> Option<usize> {
    match id {
        GOAL_TILE_ID | 216 | 218 | FLAG_TILE_ID => Some(1),
        217 => Some(2),
        219 => Some(6),
        WORMHOLE_TILE_ID => Some(5),
        _ => None,
    }
}

fn map_security_checksum_for_tiles(tiles: &[u8], key: u32) -> u32 {
    let basekey = key as i32;
    let mut checksum = key;
    let mut y = basekey % 32;

    while y < MAP_TILES as i32 {
        let mut x = basekey % 31;
        while x < MAP_TILES as i32 {
            let index = y as usize * MAP_TILES + x as usize;
            let mut tile = tiles.get(index).copied().unwrap_or(20);
            if tile == BRICK_TILE_ID {
                tile = 0;
            }

            if (1..=160).contains(&tile) || tile == SAFE_TILE_ID {
                checksum = checksum.wrapping_add(key ^ tile as u32);
            }

            x += 31;
        }
        y += 32;
    }

    checksum
}

#[cfg(test)]
mod dynamic_zone_identity_tests {
    use super::bundles::tests::TempZoneRoot;
    use super::*;
    use image::ImageEncoder;

    fn identity_corpus() -> serde_json::Value {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/zone_identity_corpus.json"
        )))
        .expect("shared zone identity corpus")
    }

    fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(kind);
        bytes.extend((payload.len() as u32).to_le_bytes());
        bytes.extend(payload);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        bytes
    }

    #[test]
    fn elvl_no_flag_drop_region_decodes_exact_rle_membership() {
        // Row 0: five absent, one present, 1018 absent. Then 1023 empty
        // rows. This is the source rTIL RLE shape for exactly tile (5, 0).
        let tile_data = [4, 0x40, 0x23, 0xf9, 0xa3, 0xfe];
        let mut region = chunk(b"rNAM", b"no-flags");
        region.extend(chunk(b"rTIL", &tile_data));
        region.extend(chunk(b"rNFL", &[]));
        let region = chunk(b"REGN", &region);
        let metadata_size = 12 + region.len();
        let metadata_offset = 14_u32;
        let file_size = metadata_offset as usize + metadata_size;
        let mut lvl = vec![0; metadata_offset as usize];
        lvl[..2].copy_from_slice(b"BM");
        lvl[2..6].copy_from_slice(&(file_size as u32).to_le_bytes());
        lvl[6..10].copy_from_slice(&metadata_offset.to_le_bytes());
        lvl.extend(b"elvl");
        lvl.extend((metadata_size as u32).to_le_bytes());
        lvl.extend(0_u32.to_le_bytes());
        lvl.extend(region);

        let no_flags = parse_no_flag_drop_tiles(&lvl).unwrap();

        assert!(no_flags.contains(5, 0));
        assert!(!no_flags.contains(4, 0));
        assert!(!no_flags.contains(5, 1));

        let mut anonymous = chunk(b"rTIL", &tile_data);
        anonymous.extend(chunk(b"rNFL", &[]));
        let anonymous = chunk(b"REGN", &anonymous);
        let metadata_size = 12 + anonymous.len();
        let file_size = metadata_offset as usize + metadata_size;
        let mut lvl = vec![0; metadata_offset as usize];
        lvl[..2].copy_from_slice(b"BM");
        lvl[2..6].copy_from_slice(&(file_size as u32).to_le_bytes());
        lvl[6..10].copy_from_slice(&metadata_offset.to_le_bytes());
        lvl.extend(b"elvl");
        lvl.extend((metadata_size as u32).to_le_bytes());
        lvl.extend(0_u32.to_le_bytes());
        lvl.extend(anonymous);
        assert!(!parse_no_flag_drop_tiles(&lvl).unwrap().contains(5, 0));
    }

    #[test]
    fn derived_zone_metadata_moves_the_single_tile_allocation() {
        let mut tiles = vec![0_u8; MAP_TILES * MAP_TILES];
        tiles[17] = FLAG_TILE_ID;
        let source_ptr = tiles.as_ptr();
        let source_capacity = tiles.capacity();

        let (tiles, checksum, crc32, flags) = derive_zone_map_tiles(tiles);

        assert_eq!(tiles.as_ptr(), source_ptr);
        assert_eq!(tiles.capacity(), source_capacity);
        assert_eq!(
            checksum,
            map_security_checksum_for_tiles(&tiles, SECURITY_CHECKSUM_KEY)
        );
        assert_eq!(crc32, continuum_map_crc32(&tiles));
        assert_eq!(flags.len(), 1);
        assert_eq!(
            crate::render_limits::checked_map_representation_bytes(0, tiles.len(), 0, 0, 0, 0,)
                .unwrap(),
            256 + tiles.len(),
            "catalog usage charges the one moved tile allocation, not a checksum copy",
        );
    }

    #[test]
    fn embedded_overrides_require_both_operator_and_manifest_opt_in() {
        let root = TempZoneRoot::new();
        root.write_valid_bundle("chaos-zone", "Chaos Zone", Some("chaos-zone"));
        assert!(load_server_zone_catalog(root.path.to_str(), false).is_err());
        let catalog =
            load_server_zone_catalog(root.path.to_str(), true).expect("explicit override");
        assert!(catalog.zone_ids().any(|id| id == "chaos-zone"));

        let root = TempZoneRoot::new();
        root.write_valid_bundle("chaos-zone", "Chaos Zone", None);
        assert!(load_server_zone_catalog(root.path.to_str(), true).is_err());
    }

    #[test]
    fn new_zone_cannot_claim_an_existing_normalized_identity() {
        let root = TempZoneRoot::new();
        root.write_valid_bundle("new-zone", "Extreme", None);
        assert!(load_server_zone_catalog(root.path.to_str(), false).is_err());
    }

    #[test]
    fn shared_display_name_corpus_matches_browser_normalization_contract() {
        let corpus = identity_corpus();
        for descriptor in corpus["accepted_descriptors"]
            .as_array()
            .expect("accepted descriptor fixtures")
        {
            let id = descriptor["id"].as_str().expect("fixture id");
            let display_name = descriptor["display_name"]
                .as_str()
                .expect("fixture display_name");
            assert!(!normalized_zone_lookup_name(display_name).is_empty());
            let root = TempZoneRoot::new();
            root.write_valid_bundle(id, display_name, None);
            let catalog =
                load_server_zone_catalog(root.path.to_str(), false).unwrap_or_else(|error| {
                    panic!("server rejected browser-valid {descriptor}: {error}")
                });
            let zone = catalog
                .maps()
                .iter()
                .find(|zone| zone.zone_id == id)
                .expect("accepted fixture must reach the catalog");
            assert_eq!(zone.display_name, display_name);
        }

        for (index, display_name) in corpus["rejected_display_names"]
            .as_array()
            .expect("rejected display-name fixtures")
            .iter()
            .enumerate()
        {
            let display_name = display_name.as_str().expect("fixture display_name");
            let root = TempZoneRoot::new();
            root.write_valid_bundle(&format!("invalid-{index}"), display_name, None);
            assert!(
                load_server_zone_catalog(root.path.to_str(), false).is_err(),
                "server accepted browser-invalid display_name {display_name:?}"
            );
        }
    }

    #[test]
    fn shared_identity_collisions_reject_atomically_before_status_serialization() {
        let installed_before = server_zone_maps()
            .iter()
            .map(|zone| (zone.zone_id.to_string(), zone.display_name.to_string()))
            .collect::<Vec<_>>();
        for fixtures in identity_corpus()["collision_catalogs"]
            .as_array()
            .expect("collision fixtures")
        {
            let root = TempZoneRoot::new();
            for descriptor in fixtures.as_array().expect("collision catalog") {
                root.write_valid_bundle(
                    descriptor["id"].as_str().expect("fixture id"),
                    descriptor["display_name"]
                        .as_str()
                        .expect("fixture display_name"),
                    None,
                );
            }
            let error = load_server_zone_catalog(root.path.to_str(), false)
                .err()
                .expect("normalized id/label collision must reject the complete catalog");
            assert!(error.contains("conflict"), "unexpected error: {error}");
            let installed_after = server_zone_maps()
                .iter()
                .map(|zone| (zone.zone_id.to_string(), zone.display_name.to_string()))
                .collect::<Vec<_>>();
            assert_eq!(
                installed_after, installed_before,
                "a rejected identity corpus must not partially alter status state"
            );
        }
    }

    #[test]
    fn accepted_punctuation_display_name_has_exact_zones_serialization() {
        let root = TempZoneRoot::new();
        root.write_valid_bundle("punct-zone", "Zone! 42?", None);
        let catalog = load_server_zone_catalog(root.path.to_str(), false).unwrap();
        assert_eq!(
            crate::connection::server_zone_catalog_json(catalog.maps()),
            r#"{"ok":true,"zones":[{"id":"chaos-zone","display_name":"Chaos Zone","map":"bdhadar.lvl"},{"id":"extreme","display_name":"Extreme","map":"doomsy8.lvl"},{"id":"punct-zone","display_name":"Zone! 42?","map":"test.lvl"}]}"#
        );
    }

    fn write_count_fixture(root: &TempZoneRoot, new_zones: usize) {
        root.write_valid_bundle("chaos-zone", "Chaos Zone", Some("chaos-zone"));
        root.write_valid_bundle("extreme", "Extreme", Some("extreme"));
        write_new_zone_fixture(root, new_zones);
    }

    fn write_new_zone_fixture(root: &TempZoneRoot, new_zones: usize) {
        for index in 0..new_zones {
            root.write_valid_bundle(&format!("z{index:03}"), &format!("Zone {index:03}"), None);
        }
    }

    #[test]
    fn merged_catalog_count_accepts_exact_64_with_overrides_replacing_embedded() {
        let root = TempZoneRoot::new();
        // 2 embedded - 2 replaced + 2 overrides + 62 new = 64 installed: with the
        // fixture replacing both embedded zones, merged equals 2 + new_zones.
        write_count_fixture(&root, 62);
        let catalog = load_server_zone_catalog(root.path.to_str(), true)
            .expect("exact shared merged zone count");
        assert_eq!(catalog.zone_ids().count(), crate::zone_wire::MAX_ZONE_COUNT);
        assert_eq!(
            catalog.zone_ids().filter(|id| *id == "chaos-zone").count(),
            1
        );
        assert_eq!(catalog.zone_ids().filter(|id| *id == "extreme").count(), 1);
    }

    #[test]
    fn merged_catalog_count_rejects_65_atomically_before_catalog_installation() {
        let installed_before = server_zone_maps()
            .iter()
            .map(|zone| zone.zone_id.to_string())
            .collect::<Vec<_>>();
        let root = TempZoneRoot::new();
        // New-id bundles only, deliberately with no override rows. Overriding both surviving
        // embedded zones makes the merged count equal the root directory count, so the
        // 64-directory root cap rejects first and the merged-count cap becomes unreachable.
        // Without overrides, 63 new zones advertise 2 embedded + 63 = 65 merged zones (cap + 1)
        // from 63 directories, which is inside the root budget, so the merged cap is what rejects.
        write_new_zone_fixture(&root, 63);
        let error = load_server_zone_catalog(root.path.to_str(), false)
            .err()
            .expect("merged cap+1 must reject startup catalog construction");
        assert!(
            error.contains("merged zone catalog contains 65 zones; maximum is 64"),
            "unexpected merged-count error: {error}"
        );
        let installed_after = server_zone_maps()
            .iter()
            .map(|zone| zone.zone_id.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            installed_after, installed_before,
            "a rejected startup catalog must not partially change /v1/zones state"
        );
    }

    #[test]
    fn complete_new_zone_builds_map_settings_and_security_identity_from_disk() {
        let root = TempZoneRoot::new();
        root.write_valid_bundle("test-zone", "Test Zone", None);
        let maps = load_configured_zone_maps(root.path.to_str(), false).expect("disk zone catalog");
        let zone = maps
            .iter()
            .find(|zone| zone.zone_id == "test-zone")
            .expect("new disk zone");
        assert_eq!(zone.filename, "test.lvl");
        assert_eq!(
            zone.settings_packet.len(),
            crate::CONTINUUM_ARENA_SETTINGS_SIZE
        );
        assert_ne!(zone.checksum, 0);
        assert_ne!(zone.map_checksum, 0);
        assert_ne!(zone.continuum_crc32, 0);
    }

    #[test]
    fn map_transfer_compression_enforces_source_and_encoded_boundaries() {
        let exact_compressible = vec![0; crate::zone_wire::MAX_MAP_SOURCE_BYTES];
        assert!(compress_map_for_transfer("test-zone", &exact_compressible).is_ok());
        assert!(
            compress_map_for_transfer(
                "test-zone",
                &vec![0; crate::zone_wire::MAX_MAP_SOURCE_BYTES + 1]
            )
            .is_err()
        );

        let mut state = 0x91e1_0da5_u32;
        let mut incompressible = vec![0; crate::zone_wire::MAX_MAP_SOURCE_BYTES];
        for byte in &mut incompressible {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state as u8;
        }
        assert!(
            compress_map_for_transfer("test-zone", &incompressible).is_err(),
            "an incompressible exact-source-limit map must fail when zlib framing exceeds the logical transfer budget"
        );
    }

    #[test]
    fn catalog_construction_parses_every_declared_lvz_before_installation() {
        let root = TempZoneRoot::new();
        let directory = root.write_valid_bundle("test-zone", "Test Zone", None);
        let corrupt = b"CONT\x01\x00\x00\x00";
        std::fs::write(directory.join("broken.lvz"), corrupt).unwrap();
        let manifest_path = directory.join(bundles::BUNDLE_MANIFEST_FILENAME);
        let mut manifest = std::fs::read_to_string(&manifest_path).unwrap();
        manifest.push_str(&format!(
            "lvz=broken.lvz,{}\n",
            bundles::sha256_hex(corrupt)
        ));
        std::fs::write(manifest_path, manifest).unwrap();

        let error = load_server_zone_catalog(root.path.to_str(), false)
            .err()
            .expect("truncated LVZ must fail catalog construction");
        assert!(
            error.contains("LVZ") && error.contains("broken.lvz"),
            "got: {error}"
        );
    }

    #[test]
    fn catalog_rejects_header_valid_but_truncated_render_payload_before_installation() {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[0, 0, 0, 0], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        png.truncate(png.len() - 10);
        assert!(crate::render_limits::validate_raster_header(&png).is_ok());
        assert!(crate::render_limits::fully_decode_image_for_filename("bad.png", &png).is_err());

        let mut lvz = Vec::new();
        lvz.extend_from_slice(&crate::lvz_archive::LvzArchive::MAGIC_CONT.to_le_bytes());
        lvz.extend_from_slice(&1_u32.to_le_bytes());
        lvz.extend_from_slice(&crate::lvz_archive::LvzArchive::MAGIC_CONT.to_le_bytes());
        lvz.extend_from_slice(&(png.len() as u32).to_le_bytes());
        lvz.extend_from_slice(&1_u32.to_le_bytes());
        lvz.extend_from_slice(&(png.len() as u32).to_le_bytes());
        lvz.extend_from_slice(b"bad.png\0");
        lvz.extend_from_slice(&png);

        let definition = &ZONE_DEFINITIONS[0];
        let result = build_zone_map_parts(
            Cow::Borrowed("test-zone"),
            Cow::Borrowed("Test Zone"),
            Cow::Borrowed("test.lvl"),
            Cow::Borrowed(&[]),
            Cow::Borrowed("news.txt"),
            Cow::Borrowed(&[]),
            definition.settings_bytes,
            [(Cow::Borrowed("bad.lvz"), Cow::Owned(lvz))],
        );
        assert!(result.is_err());
    }

    fn synthetic_usage_map(index: usize, usage: ZoneCatalogUsage) -> ZoneMap {
        ZoneMap {
            zone_id: Cow::Owned(format!("z{index}")),
            display_name: Cow::Owned(format!("Zone {index}")),
            filename: Cow::Borrowed("test.lvl"),
            news_filename: Cow::Borrowed("news.txt"),
            news_bytes: Cow::Borrowed(&[]),
            tiles: Vec::new(),
            no_flag_drop_tiles: NoFlagDropTiles::default(),
            checksum: 0,
            sha256: [0; crate::zone_wire::SHA256_BYTES],
            compressed: Vec::new(),
            map_checksum: 0,
            continuum_crc32: 0,
            flags: Vec::new(),
            objective_profile: crate::objectives::ServerObjectiveProfile::disabled(),
            lvz_files: Vec::new(),
            settings_packet: Cow::Borrowed(&[]),
            brick_span: 1,
            resource_usage: usage,
        }
    }

    #[test]
    fn catalog_resource_budgets_accept_exact_64_bundle_total_and_reject_max_plus_one() {
        let per_bundle = ZoneCatalogUsage {
            source: crate::zone_wire::MAX_ZONE_CATALOG_SOURCE_BYTES / 64,
            encoded: crate::zone_wire::MAX_ZONE_CATALOG_ENCODED_BYTES / 64,
            decoded: crate::zone_wire::MAX_ZONE_CATALOG_DECODED_BYTES / 64,
        };
        let mut maps = (0..64)
            .map(|index| synthetic_usage_map(index, per_bundle))
            .collect::<Vec<_>>();
        assert_eq!(
            catalog_usage(&maps).unwrap(),
            ZoneCatalogUsage {
                source: crate::zone_wire::MAX_ZONE_CATALOG_SOURCE_BYTES,
                encoded: crate::zone_wire::MAX_ZONE_CATALOG_ENCODED_BYTES,
                decoded: crate::zone_wire::MAX_ZONE_CATALOG_DECODED_BYTES,
            }
        );
        maps[63].resource_usage.source += 1;
        assert!(catalog_usage(&maps).is_err());
        maps[63].resource_usage.source -= 1;
        maps[63].resource_usage.encoded += 1;
        assert!(catalog_usage(&maps).is_err());
        maps[63].resource_usage.encoded -= 1;
        maps[63].resource_usage.decoded += 1;
        assert!(catalog_usage(&maps).is_err());
    }

    #[test]
    fn generated_transfer_aggregate_must_fit_the_client_advertisement_budget() {
        fn lvz_with_total_size(size: usize) -> Vec<u8> {
            let filename = b"asset.bin\0";
            let overhead = 8 + 16 + filename.len();
            assert!(size > overhead);
            let payload_len = size - overhead;
            let mut bytes = Vec::with_capacity(size);
            bytes.extend_from_slice(&crate::lvz_archive::LvzArchive::MAGIC_CONT.to_le_bytes());
            bytes.extend_from_slice(&1_u32.to_le_bytes());
            bytes.extend_from_slice(&crate::lvz_archive::LvzArchive::MAGIC_CONT.to_le_bytes());
            bytes.extend_from_slice(&(payload_len as u32).to_le_bytes());
            bytes.extend_from_slice(&1_u32.to_le_bytes());
            bytes.extend_from_slice(&(payload_len as u32).to_le_bytes());
            bytes.extend_from_slice(filename);
            bytes.resize(size, 0xa5);
            bytes
        }

        let map_bytes = assets::SERVER_MAP_BYTES;
        let compressed = compress_map_for_transfer("aggregate-zone", map_bytes).unwrap();
        let file_count = 33;
        let needed = crate::zone_wire::MAX_ZONE_TRANSFER_BYTES - compressed.len() + 1;
        let per_file = needed.div_ceil(file_count);
        assert!(per_file <= crate::zone_wire::MAX_TRANSFER_FILE_BYTES);
        let lvz_files = (0..file_count)
            .map(|index| {
                (
                    Cow::Owned(format!("f{index:02}.lvz")),
                    Cow::Owned(lvz_with_total_size(per_file)),
                )
            })
            .collect::<Vec<_>>();
        let source_total = map_bytes.len()
            + include_bytes!("../../../assets/zones/extreme/arena.conf").len()
            + b"news".len()
            + per_file * file_count;
        assert!(
            source_total <= crate::zone_wire::MAX_ZONE_TRANSFER_BYTES + 512 * 1024,
            "fixture remains individually and bundle-loader valid-looking"
        );
        let error = build_zone_map_parts(
            Cow::Owned("aggregate-zone".to_string()),
            Cow::Owned("Aggregate Zone".to_string()),
            Cow::Owned("test.lvl".to_string()),
            Cow::Borrowed(map_bytes),
            Cow::Owned("news.txt".to_string()),
            Cow::Borrowed(b"news"),
            include_bytes!("../../../assets/zones/extreme/arena.conf"),
            lvz_files,
        )
        .err()
        .expect("server must reject what the client cannot advertise");
        assert!(error.contains("encoded transfer bytes"), "got: {error}");
    }

    #[test]
    fn disk_catalog_data_is_owned_and_sequential_catalogs_release_cleanly() {
        let first = TempZoneRoot::new();
        first.write_valid_bundle("first-zone", "First Zone", None);
        let catalog = load_server_zone_catalog(first.path.to_str(), false).unwrap();
        let first_map = catalog
            .maps
            .iter()
            .find(|map| map.zone_id == "first-zone")
            .unwrap();
        assert!(matches!(first_map.zone_id, Cow::Owned(_)));
        assert!(matches!(first_map.news_bytes, Cow::Owned(_)));
        assert!(matches!(first_map.settings_packet, Cow::Owned(_)));
        drop(catalog);

        let second = TempZoneRoot::new();
        second.write_valid_bundle("second-zone", "Second Zone", None);
        let reloaded = load_server_zone_catalog(second.path.to_str(), false).unwrap();
        assert!(reloaded.zone_ids().any(|id| id == "second-zone"));
        assert!(!reloaded.zone_ids().any(|id| id == "first-zone"));
    }
}
