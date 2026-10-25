use macroquad::prelude::*;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, OnceLock};

use crate::continuum::continuum_map_crc32;
use crate::door::seeded_door_tile;
use crate::lvz::LvzArchive;
use crate::packet_io::read_u32;
use crate::radar::{RadarPoint, RadarPointIndex, radar_tile_color};
use crate::render_limits::{checked_map_representation_bytes, validate_lvl_source};
use crate::tile::{
    animated_tile_size, circle_overlaps_tile, is_door_tile, is_solid_tile_with_doors,
    should_draw_base_tile, tile_center,
};
use crate::{BRICK_TILE_ID, MAP_TILES, SAFE_TILE_ID, TILE_SIZE};

pub(crate) struct ArenaMap {
    pub(crate) name: String,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) tiles: MapTiles,
    pub(crate) objects: Vec<MapObject>,
    pub(crate) doors: Vec<DoorTile>,
    pub(crate) radar_points: Vec<RadarPoint>,
    /// Cell bucketing of [`Self::radar_points`], built on the first radar frame.
    /// The initializer needs this map's own points and extent, so it cannot be
    /// a `LazyLock`.
    pub(crate) radar_index: OnceLock<RadarPointIndex>,
    pub(crate) embedded_tileset_pixel_bytes: usize,
    pub(crate) door_seed: u8,
    pub(crate) door_rng_seed: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct MapTiles {
    data: Arc<Vec<u8>>,
    nonempty_count: OnceLock<usize>,
}

impl PartialEq for MapTiles {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl Eq for MapTiles {}

impl MapTiles {
    pub(crate) fn shared(&self) -> Arc<Vec<u8>> {
        Arc::clone(&self.data)
    }

    fn nonempty_count(&self) -> usize {
        *self
            .nonempty_count
            .get_or_init(|| self.data.iter().filter(|tile| **tile != 0).count())
    }

    fn set(&mut self, index: usize, id: u8) {
        let previous = self.data[index];
        if previous == id {
            return;
        }
        Arc::make_mut(&mut self.data)[index] = id;
        if let Some(count) = self.nonempty_count.get_mut() {
            *count = *count - usize::from(previous != 0) + usize::from(id != 0);
        }
    }
}

impl From<Vec<u8>> for MapTiles {
    fn from(tiles: Vec<u8>) -> Self {
        Self {
            data: Arc::new(tiles),
            nonempty_count: OnceLock::new(),
        }
    }
}

impl Deref for MapTiles {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.data.as_slice()
    }
}

impl DerefMut for MapTiles {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // General slice edits must invalidate the cached count. The normal
        // single-tile path updates it exactly, including brick add/removal.
        self.nonempty_count.take();
        Arc::make_mut(&mut self.data).as_mut_slice()
    }
}

pub(crate) struct MapObject {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) id: u8,
}

pub(crate) struct DoorTile {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) id: u8,
}

pub(crate) enum DownloadedFile {
    Level(ArenaMap),
    Lvz(LvzArchive),
}

pub(crate) struct ValidatedDownloadedFile {
    filename: String,
    data: Arc<[u8]>,
    parsed: Mutex<Option<DownloadedFile>>,
}

impl ValidatedDownloadedFile {
    pub(crate) fn new(filename: String, data: Vec<u8>, parsed: DownloadedFile) -> Self {
        Self {
            filename,
            data: Arc::from(data),
            parsed: Mutex::new(Some(parsed)),
        }
    }

    pub(crate) fn filename(&self) -> &str {
        &self.filename
    }

    pub(crate) fn data(&self) -> &[u8] {
        &self.data
    }

    pub(crate) fn take_parsed(&self) -> Option<DownloadedFile> {
        self.parsed
            .lock()
            .expect("validated download lock should not be poisoned")
            .take()
    }

    pub(crate) fn seal_full_renderability(&self) -> Result<(), String> {
        self.seal_with_prepared_images(&mut crate::render_limits::ImagePreparationBudget::default())
    }

    pub(crate) fn seal_with_prepared_images(
        &self,
        budget: &mut crate::render_limits::ImagePreparationBudget,
    ) -> Result<(), String> {
        let mut parsed = self
            .parsed
            .lock()
            .expect("validated download lock should not be poisoned");
        if let Some(DownloadedFile::Lvz(archive)) = parsed.as_mut() {
            archive.seal_with_prepared_images(budget)?;
        }
        Ok(())
    }
}

impl fmt::Debug for ValidatedDownloadedFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedDownloadedFile")
            .field("filename", &self.filename)
            .field("data_len", &self.data.len())
            .finish_non_exhaustive()
    }
}

impl PartialEq for ValidatedDownloadedFile {
    fn eq(&self, other: &Self) -> bool {
        self.filename == other.filename && self.data == other.data
    }
}

impl Eq for ValidatedDownloadedFile {}

pub(crate) fn lvl_embedded_bitmap(data: &[u8]) -> Result<Option<&[u8]>, String> {
    let offset = lvl_tile_record_offset(data)?;
    if offset == 0 {
        Ok(None)
    } else {
        Ok(Some(&data[..offset]))
    }
}

pub(crate) fn parse_downloaded_file(filename: &str, data: &[u8]) -> Result<DownloadedFile, String> {
    let extension = filename.rsplit_once('.').map(|(_, extension)| extension);
    if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("lvl")) {
        ArenaMap::from_lvl(filename, data).map(DownloadedFile::Level)
    } else if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("lvz")) {
        LvzArchive::parse(data).map(DownloadedFile::Lvz)
    } else {
        Err(format!("unsupported downloaded file type: {filename}"))
    }
}

pub(crate) fn parse_downloaded_file_structural(
    filename: &str,
    data: &[u8],
) -> Result<DownloadedFile, String> {
    let extension = filename.rsplit_once('.').map(|(_, extension)| extension);
    if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("lvl")) {
        ArenaMap::from_lvl(filename, data).map(DownloadedFile::Level)
    } else if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("lvz")) {
        LvzArchive::parse_structural(data).map(DownloadedFile::Lvz)
    } else {
        Err(format!("unsupported downloaded file type: {filename}"))
    }
}

impl ArenaMap {
    pub(crate) fn fallback_skirmish() -> Self {
        let width = MAP_TILES as usize;
        let height = MAP_TILES as usize;
        let mut tiles = vec![0; width * height];
        let mut objects = Vec::new();
        let doors = Vec::new();
        let center = MAP_TILES / 2;
        let half = 32;
        let min = center - half;
        let max = center + half;

        for y in 0..MAP_TILES {
            for x in 0..MAP_TILES {
                let border = x == min || y == min || x == max || y == max;
                let station_ring =
                    (x == center - 8 || x == center + 7 || y == center - 8 || y == center + 7)
                        && x > center - 9
                        && x < center + 8
                        && y > center - 9
                        && y < center + 8;
                let corridors =
                    (x > center - 3 && x < center + 2) || (y > center - 3 && y < center + 2);
                let diagonal = (x - y).abs() < 2 && x > center - 24 && x < center - 10;

                if border || (station_ring && !corridors) || diagonal {
                    tiles[(y as usize * width) + x as usize] = 1;
                }
            }
        }

        for (x, y, id) in [
            (center - 18, center - 18, 216),
            (center + 18, center - 18, 218),
            (center - 20, center + 18, 217),
            (center + 14, center + 14, 220),
            (center + 1, center - 1, 170),
            (center - 1, center - 1, 172),
        ] {
            tiles[(y as usize * width) + x as usize] = id;
            objects.push(MapObject {
                x: x as usize,
                y: y as usize,
                id,
            });
        }

        Self {
            name: "fallback".to_string(),
            width,
            height,
            radar_points: Self::build_radar_points(&tiles, width, height),
            radar_index: OnceLock::new(),
            embedded_tileset_pixel_bytes: 0,
            tiles: tiles.into(),
            objects,
            doors,
            door_seed: u8::MAX,
            door_rng_seed: 1,
        }
    }

    pub(crate) fn from_lvl(name: &str, data: &[u8]) -> Result<Self, String> {
        let layout = validate_lvl_source(data)?;
        let offset = layout.tile_offset;

        let width = MAP_TILES as usize;
        let height = MAP_TILES as usize;
        let mut tiles = vec![0; width * height];
        let mut objects = Vec::with_capacity(layout.animated_record_count);
        let mut doors = Vec::with_capacity(layout.door_record_count);
        let tile_bytes = &data[offset..];

        if !tile_bytes.len().is_multiple_of(4) {
            return Err("LVL tile data is not aligned to 4-byte tile records".to_string());
        }

        for record in tile_bytes.chunks_exact(4) {
            let packed = u32::from_le_bytes([record[0], record[1], record[2], record[3]]);
            let x = (packed & 0x0fff) as usize;
            let y = ((packed >> 12) & 0x0fff) as usize;
            let id = (packed >> 24) as u8;

            if x >= width || y >= height {
                return Err(format!("LVL tile coordinate out of bounds: {x},{y}"));
            }

            tiles[y * width + x] = id;

            if let Some(animated_size) = animated_tile_size(id) {
                objects.push(MapObject { x, y, id });
                for dy in 0..animated_size {
                    for dx in 0..animated_size {
                        let ax = x + dx;
                        let ay = y + dy;
                        if ax < width && ay < height {
                            tiles[ay * width + ax] = id;
                        }
                    }
                }
            }

            if is_door_tile(id) {
                doors.push(DoorTile { x, y, id });
            }
        }

        let retained_radar_slots = tiles.iter().filter(|tile| **tile != 0).count();
        checked_map_representation_bytes(
            name.len(),
            tiles.len(),
            objects.len(),
            doors.len(),
            retained_radar_slots,
            layout.embedded_pixel_bytes,
        )?;
        let radar_points = Self::build_radar_points(&tiles, width, height);
        let tiles = MapTiles {
            data: Arc::new(tiles),
            nonempty_count: OnceLock::from(retained_radar_slots),
        };
        Ok(Self {
            name: name.to_string(),
            width,
            height,
            radar_points,
            radar_index: OnceLock::new(),
            embedded_tileset_pixel_bytes: layout.embedded_pixel_bytes,
            tiles,
            objects,
            doors,
            door_seed: u8::MAX,
            door_rng_seed: 1,
        })
    }

    pub(crate) fn retained_render_bytes(&self) -> Result<usize, String> {
        checked_map_representation_bytes(
            self.name.len(),
            self.tiles.len(),
            self.objects.len(),
            self.doors.len(),
            self.tiles.nonempty_count(),
            self.embedded_tileset_pixel_bytes,
        )
    }

    pub(crate) fn tile(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 20;
        }
        self.tiles[(y as usize * self.width) + x as usize]
    }

    pub(crate) fn set_tile(&mut self, x: i32, y: i32, id: u8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.tiles.set((y as usize * self.width) + x as usize, id);
    }

    pub(crate) fn render_tile(&self, id: u8) -> Option<u8> {
        let id = seeded_door_tile(id, self.door_seed).unwrap_or(id);
        if should_draw_base_tile(id) {
            Some(id)
        } else {
            None
        }
    }

    pub(crate) fn is_solid_world(&self, pos: Vec2) -> bool {
        is_solid_tile_with_doors(self.tile_at_world(pos), self.door_seed)
    }

    pub(crate) fn tile_at_world(&self, pos: Vec2) -> u8 {
        let x = (pos.x / TILE_SIZE).floor() as i32;
        let y = (pos.y / TILE_SIZE).floor() as i32;
        self.tile(x, y)
    }

    #[allow(dead_code)]
    pub(crate) fn security_checksum(&self, key: u32) -> u32 {
        map_security_checksum_for_tiles(&self.tiles, self.width, self.height, key)
    }

    #[allow(dead_code)]
    pub(crate) fn continuum_crc32(&self) -> u32 {
        continuum_map_crc32(&self.tiles)
    }

    pub(crate) fn is_touching_object(&self, pos: Vec2, id: u8, radius_px: f32) -> bool {
        self.objects.iter().any(|object| {
            object.id == id
                && pos.distance(tile_center(ivec2(object.x as i32, object.y as i32))) <= radius_px
        })
    }

    pub(crate) fn first_object_position(&self, id: u8) -> Option<Vec2> {
        self.objects
            .iter()
            .find(|object| object.id == id)
            .map(|object| tile_center(ivec2(object.x as i32, object.y as i32)))
    }

    pub(crate) fn object_positions(&self, id: u8) -> Vec<Vec2> {
        self.objects
            .iter()
            .filter(|object| object.id == id)
            .map(|object| tile_center(ivec2(object.x as i32, object.y as i32)))
            .collect()
    }

    pub(crate) fn can_fit_world(&self, pos: Vec2, radius_px: f32) -> bool {
        let min_tile_x = ((pos.x - radius_px) / TILE_SIZE).floor() as i32;
        let min_tile_y = ((pos.y - radius_px) / TILE_SIZE).floor() as i32;
        let max_tile_x = ((pos.x + radius_px) / TILE_SIZE).floor() as i32;
        let max_tile_y = ((pos.y + radius_px) / TILE_SIZE).floor() as i32;

        for y in min_tile_y..=max_tile_y {
            for x in min_tile_x..=max_tile_x {
                if !is_solid_tile_with_doors(self.tile(x, y), self.door_seed) {
                    continue;
                }

                if circle_overlaps_tile(pos, radius_px, x, y) {
                    return false;
                }
            }
        }

        true
    }

    pub(crate) fn world_size(&self) -> f32 {
        self.width as f32 * TILE_SIZE
    }

    pub(crate) fn spawn_position(&self) -> Vec2 {
        let center_x = (self.width / 2) as i32;
        let center_y = (self.height / 2) as i32;

        if let Some(tile) = self.find_spawn_tile(center_x, center_y, true) {
            return tile_center(tile);
        }

        if let Some(tile) = self.find_spawn_tile(center_x, center_y, false) {
            return tile_center(tile);
        }

        vec2(self.world_size() * 0.5, self.world_size() * 0.5)
    }

    /// Network lifecycle baselines use the server's static map, independent
    /// of current door state and temporary brick overlays.
    pub(crate) fn static_spawn_position(&self, tile_at: impl Fn(i32, i32) -> u8) -> Vec2 {
        let center_x = (self.width / 2) as i32;
        let center_y = (self.height / 2) as i32;
        let max_radius = (self.width.max(self.height) / 2) as i32;
        for require_clearance in [true, false] {
            let tile = crate::continuum_position::find_spawn_tile(
                center_x,
                center_y,
                max_radius,
                |x, y| {
                    if crate::tile::is_solid_tile(tile_at(x, y)) {
                        return true;
                    }
                    require_clearance
                        && (-1..=1).any(|dy| {
                            (-1..=1).any(|dx| crate::tile::is_solid_tile(tile_at(x + dx, y + dy)))
                        })
                },
            );
            if let Some((x, y)) = tile {
                return tile_center(ivec2(x, y));
            }
        }
        vec2(self.world_size() * 0.5, self.world_size() * 0.5)
    }

    pub(crate) fn powerball_spawn_position(&self, near: Vec2) -> Vec2 {
        self.random_open_position_near(near, 4, 18, 64)
            .unwrap_or_else(|| self.spawn_position())
    }

    pub(crate) fn random_open_position_near(
        &self,
        center: Vec2,
        min_radius: i32,
        max_radius: i32,
        attempts: usize,
    ) -> Option<Vec2> {
        let center_x = (center.x / TILE_SIZE).floor() as i32;
        let center_y = (center.y / TILE_SIZE).floor() as i32;

        for _ in 0..attempts {
            let dx = macroquad::rand::gen_range(-max_radius, max_radius + 1);
            let dy = macroquad::rand::gen_range(-max_radius, max_radius + 1);

            if dx.abs().max(dy.abs()) < min_radius {
                continue;
            }

            let x = center_x + dx;
            let y = center_y + dy;
            if !is_solid_tile_with_doors(self.tile(x, y), self.door_seed) {
                return Some(tile_center(ivec2(x, y)));
            }
        }

        self.find_spawn_tile(center_x, center_y, false)
            .map(tile_center)
    }

    fn find_spawn_tile(
        &self,
        center_x: i32,
        center_y: i32,
        require_clearance: bool,
    ) -> Option<IVec2> {
        let max_radius = (self.width.max(self.height) / 2) as i32;
        // Shared outward-radius search algorithm — keeps client + server
        // spawn-tile selection order identical so positions can't desync.
        crate::continuum_position::find_spawn_tile(center_x, center_y, max_radius, |x, y| {
            !self.can_spawn_at(x, y, require_clearance)
        })
        .map(|(x, y)| ivec2(x, y))
    }

    fn can_spawn_at(&self, x: i32, y: i32, require_clearance: bool) -> bool {
        if is_solid_tile_with_doors(self.tile(x, y), self.door_seed) {
            return false;
        }

        if !require_clearance {
            return true;
        }

        for dy in -1..=1 {
            for dx in -1..=1 {
                if is_solid_tile_with_doors(self.tile(x + dx, y + dy), self.door_seed) {
                    return false;
                }
            }
        }

        true
    }

    pub(crate) fn build_radar_points(tiles: &[u8], width: usize, height: usize) -> Vec<RadarPoint> {
        let mut points = Vec::new();

        for y in 0..height {
            for x in 0..width {
                let Some(color) = radar_tile_color(tiles[y * width + x]) else {
                    continue;
                };
                points.push(RadarPoint { x, y, color });
            }
        }

        points
    }

    /// Cell-bucketed [`Self::radar_points`] for the small radar's viewport scan.
    pub(crate) fn radar_point_index(&self) -> &RadarPointIndex {
        self.radar_index
            .get_or_init(|| RadarPointIndex::build(&self.radar_points, self.width, self.height))
    }
}

fn lvl_tile_record_offset(data: &[u8]) -> Result<usize, String> {
    if data.len() < 6 || &data[0..2] != b"BM" {
        return Ok(0);
    }

    let offset = read_u32(data, 2)? as usize;
    if offset > data.len() {
        return Err("LVL tile offset points past end of file".to_string());
    }
    Ok(offset)
}

pub(crate) fn map_security_checksum_for_tiles(
    tiles: &[u8],
    width: usize,
    height: usize,
    key: u32,
) -> u32 {
    let basekey = key as i32;
    let mut checksum = key;
    let mut y = basekey % 32;

    while y < height as i32 {
        let mut x = basekey % 31;
        while x < width as i32 {
            let index = y as usize * width + x as usize;
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
mod accounting_tests {
    use super::*;

    #[test]
    fn map_accounting_cache_tracks_single_and_bulk_edits_without_mutating_shared_tiles() {
        let mut tiles = MapTiles::from(vec![0, 1, 2, 0]);
        let uncounted = tiles.clone();
        assert_eq!(tiles.nonempty_count(), 2);
        assert_eq!(tiles.nonempty_count.get(), Some(&2));
        assert_eq!(tiles, uncounted, "cache population is not map identity");
        let shared = tiles.shared();
        let predecessor = tiles.clone();
        tiles.set(0, 3);
        tiles.set(1, 0);
        tiles.set(2, 4);
        tiles.set(2, 4);
        assert_eq!(tiles.nonempty_count.get(), Some(&2));
        assert_eq!(shared.as_slice(), &[0, 1, 2, 0]);
        assert_eq!(predecessor.nonempty_count(), 2);
        assert_eq!(&*predecessor, &[0, 1, 2, 0]);
        tiles[1..].fill(5);
        assert_eq!(
            tiles.nonempty_count.get(),
            None,
            "mutable slice invalidates cache"
        );
        assert_eq!(tiles.nonempty_count(), 4);
        assert_eq!(tiles.nonempty_count.get(), Some(&4));
        tiles.set(0, 0);
        assert_eq!(tiles.nonempty_count.get(), Some(&3));
    }

    #[test]
    fn parsed_map_seeds_exact_accounting_without_scanning_again_on_each_archive() {
        let mut records = Vec::new();
        for (x, y, id) in [(10, 20, 1), (11, 20, 0), (12, 20, 162), (14, 22, 217)] {
            let packed: u32 = x | (y << 12) | (id << 24);
            records.extend_from_slice(&packed.to_le_bytes());
        }
        let mut map = ArenaMap::from_lvl("accounting.lvl", &records).unwrap();
        let expected = map.tiles.iter().filter(|tile| **tile != 0).count();
        assert_eq!(map.tiles.nonempty_count.get(), Some(&expected));
        let bytes = map.retained_render_bytes().unwrap();
        for _ in 0..8 {
            assert_eq!(map.retained_render_bytes().unwrap(), bytes);
            assert_eq!(map.tiles.nonempty_count.get(), Some(&expected));
        }
        map.set_tile(0, 0, 20);
        assert_eq!(map.tiles.nonempty_count.get(), Some(&(expected + 1)));
        assert_eq!(
            map.retained_render_bytes().unwrap(),
            bytes + crate::zone_wire::MAP_RADAR_POINT_RETAINED_BYTES
        );
        map.set_tile(0, 0, 0);
        assert_eq!(map.retained_render_bytes().unwrap(), bytes);
        map.set_tile(-1, 0, 20);
        map.set_tile(map.width as i32, 0, 20);
        assert_eq!(map.retained_render_bytes().unwrap(), bytes);
    }
}
