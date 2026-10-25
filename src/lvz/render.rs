use macroquad::prelude::*;
use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use super::{
    LvzArchive, LvzImageDefinition, LvzObjectDefinition, LvzRenderUsage, LvzSectionKind,
    NetworkLvzState, lvz_display_time_seconds, lvz_graphic_override_stem_for_file, lvz_layer,
    lvz_layer_in_range, lvz_map_object_world_position, lvz_texture_name_for_image_filename,
    normalize_asset_name,
};

/// Objects resolved per frame by the budgeted join finalize. One Chaos LVZ carries ~27k of the zone's
/// 28.9k objects and the per-object image resolve is a measured ~54 ms, so this bound is what keeps a
/// join from paying the whole resolve in one frame. A zone that fits the bound is finalized inline.
pub(crate) const LVZ_RESOLVE_OBJECTS_PER_FRAME: usize = 2_048;

#[derive(Default)]
pub(crate) struct LvzRenderCache {
    pub(crate) textures: BTreeMap<Arc<str>, LvzTexture>,
    pub(crate) graphic_overrides: BTreeMap<Arc<str>, Arc<str>>,
    pub(crate) objects: Vec<LvzRenderableObject>,
    image: LvzImageResolution,
    objects_by_id: BTreeMap<u16, Vec<usize>>,
    render_usage: LvzRenderUsage,
    render_images: BTreeMap<Arc<str>, usize>,
    // `objects` indices per `LVZ_MAP_BANDS` entry, in table order.
    map_bands: Vec<Vec<u32>>,
    // `objects` indices per `LVZ_SCREEN_BANDS` entry, in table order.
    screen_bands: Vec<Vec<u32>>,
    // Objects no band claims; every pass walks these on top of its band.
    unbanded_objects: Vec<u32>,
    // Per map band, the objects filed into each cell their merged frame
    // rectangle covers. See `LVZ_MAP_CELL_SIZE`.
    map_band_cells: Vec<BTreeMap<(i32, i32), Vec<u32>>>,
    // Map objects whose image never resolved, so their frame extent - and
    // therefore their cells - is unknown. Every map pass walks these.
    map_unresolved_objects: Vec<u32>,
    // True while a zone's archives are merged with the whole-cache image resolve and spatial banding
    // postponed to one finalize. See `merge_archive_deferred`.
    merge_deferred: bool,
}

pub(crate) struct LvzTexture {
    pub(crate) texture: Texture2D,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// One image-filename resolution shared by every object that references it.
///
/// The frame passes used to resolve a filename once per object visit - 28,922
/// objects walked eight times per frame in Chaos - while the shipped zones carry
/// 858 distinct image filenames, and each resolution allocated two `String`s for
/// the normalisation plus a stem scan whenever the exact key missed. Merge now
/// resolves each filename once, and an object reaches its resolution through a
/// slot index, which both keeps the per-visit work to an array index and lets a
/// pass cull against the resolved frame size before it touches the texture table.
pub(crate) struct ResolvedLvzImage {
    pub(crate) texture_name: Arc<str>,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// The slot an unresolvable image filename gets. An object pointing here draws
/// nothing, which is what a failed per-visit lookup already meant.
pub(crate) const UNRESOLVED_IMAGE_SLOT: u32 = u32::MAX;

/// The exact layer bands `src/game/render.rs` invokes each LVZ map pass with.
/// The five ranges partition the clamped layers 0..=7, so a map object belongs
/// to exactly one of them per frame.
const LVZ_MAP_BANDS: [(u8, u8); 5] = [
    (super::LVZ_LAYER_BELOW_ALL, super::LVZ_LAYER_BELOW_ALL),
    (
        super::LVZ_LAYER_AFTER_BACKGROUND,
        super::LVZ_LAYER_AFTER_BACKGROUND,
    ),
    (super::LVZ_LAYER_AFTER_TILES, super::LVZ_LAYER_AFTER_TILES),
    (
        super::LVZ_LAYER_AFTER_WEAPONS,
        super::LVZ_LAYER_AFTER_WEAPONS,
    ),
    (super::LVZ_LAYER_AFTER_SHIPS, super::LVZ_LAYER_TOPMOST),
];

/// The same for the three screen passes.
const LVZ_SCREEN_BANDS: [(u8, u8); 3] = [
    (super::LVZ_LAYER_BELOW_ALL, super::LVZ_LAYER_AFTER_SHIPS),
    (super::LVZ_LAYER_AFTER_GAUGES, super::LVZ_LAYER_AFTER_GAUGES),
    (super::LVZ_LAYER_AFTER_CHAT, super::LVZ_LAYER_TOPMOST),
];

/// The band that owns an object's merged layer. Same predicate the passes apply,
/// so a band member is an object the pass would draw were it not culled.
fn lvz_band_index(bands: &[(u8, u8)], layer: u8) -> Option<usize> {
    bands
        .iter()
        .position(|(start, end)| lvz_layer_in_range(layer, *start, *end))
}

fn push_band_index(bands: &mut Vec<Vec<u32>>, band: usize, index: u32) {
    if bands.len() <= band {
        bands.resize(band + 1, Vec::new());
    }
    bands[band].push(index);
}

/// Side length, in map pixels, of one cell in a map band's spatial index.
const LVZ_MAP_CELL_SIZE: f32 = 512.0;

/// A band smaller than this is walked directly: collecting the candidate cells
/// and sorting them costs more than the walk it would replace.
const LVZ_SPATIAL_MIN_BAND: usize = 256;

/// A query wider than this walks the band instead. It exists for a viewport
/// that is not a viewport (an uninitialised or saturated camera), not for
/// ordinary play, where the visible range is a few tens of cells.
const LVZ_SPATIAL_MAX_CELLS: i64 = 4_096;

fn lvz_map_cell(x: f32, y: f32) -> (i32, i32) {
    (
        (x / LVZ_MAP_CELL_SIZE).floor() as i32,
        (y / LVZ_MAP_CELL_SIZE).floor() as i32,
    )
}

/// The inclusive cell range a rectangle covers.
///
/// An object is filed into every cell its own frame rectangle covers, and a pass
/// queries every cell the viewport covers. So an object that overlaps the
/// viewport is always in a queried cell: the two rectangles share a point, and
/// that point lies inside a cell both ranges include.
fn lvz_map_cell_range(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> (i32, i32, i32, i32) {
    let (x0, y0) = lvz_map_cell(min_x, min_y);
    let (x1, y1) = lvz_map_cell(max_x, max_y);
    (x0, y0, x1, y1)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LvzRenderableObject {
    pub(crate) object: LvzObjectDefinition,
    // All objects in one object section share its immutable image table. A
    // hostile section can no longer multiply the table by its object count.
    pub(crate) images: Arc<[LvzImageDefinition]>,
    // Parallel to `images`: the `LvzRenderCache::resolved_images` slot for each
    // image filename, rebuilt with the memo after every merge.
    pub(crate) image_slots: Arc<[u32]>,
    pub(crate) first_for_id: bool,
}

impl LvzRenderableObject {
    pub(crate) fn image_slot(&self, image_number: u8) -> u32 {
        self.image_slots
            .get(image_number as usize)
            .copied()
            .unwrap_or(UNRESOLVED_IMAGE_SLOT)
    }
}

/// The reference rules, kept for the equivalence tests. [`LvzImageResolution`] maintains the same answers
/// incrementally; this recomputes them from scratch, which is exactly why it is no longer the production
/// path - it indexes the whole texture table and walks every object filename on every call.
///
/// Generic over the table value so the equivalence against `lvz_texture_name_for_image_filename` is testable
/// without a graphics context.
#[cfg(test)]
fn resolve_image_filenames<'a, K, T>(
    filenames: impl IntoIterator<Item = &'a str>,
    textures: &'a BTreeMap<K, T>,
) -> BTreeMap<Arc<str>, Arc<str>>
where
    K: Borrow<str>,
{
    let mut exact: HashMap<&str, &K> = HashMap::with_capacity(textures.len());
    let mut stems: HashMap<&str, (&K, bool)> = HashMap::with_capacity(textures.len());
    for key in textures.keys() {
        exact.insert(key.borrow(), key);
        let Some(stem) = lvz_asset_stem(key.borrow()) else {
            continue;
        };
        // A second key under the same stem is ambiguous, and an ambiguous stem resolves to nothing.
        match stems.get_mut(stem) {
            Some((_, ambiguous)) => *ambiguous = true,
            None => {
                stems.insert(stem, (key, false));
            }
        }
    }

    let mut resolved = BTreeMap::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for filename in filenames {
        if !seen.insert(filename) {
            continue;
        }
        let normalized = normalize_asset_name(filename);
        if let Some(name) = exact.get(normalized.as_str()) {
            resolved.insert(Arc::from(filename), Arc::from((*name).borrow()));
            continue;
        }
        let Some(stem) = lvz_asset_stem(normalized.as_str()) else {
            continue;
        };
        let Some((name, ambiguous)) = stems.get(stem) else {
            continue;
        };
        if !ambiguous {
            resolved.insert(Arc::from(filename), Arc::from((*name).borrow()));
        }
    }
    resolved
}

/// The basename-before-the-last-dot that an LVZ stem fallback compares against. A name with no
/// extension has no stem, which is how `lvz_asset_stem_matches` rejects it too.
fn lvz_asset_stem(name: &str) -> Option<&str> {
    name.rsplit('/')
        .next()
        .and_then(|basename| basename.rsplit_once('.'))
        .map(|(stem, _)| stem)
}

/// Incremental counterpart of [`resolve_image_filenames`], which stays as the reference the equivalence
/// tests check against.
///
/// The reference rebuilds both lookup indexes from the whole texture table and re-resolves every distinct
/// object filename. `rebuild_resolved_images` used to call it after every merge, and a CPU profile of the
/// authored zone's boot attributed 21% of the load window to it (`make bench-client-cpu`): a boot installs
/// dozens of archives while the object table is already near its final size, so the same filenames were
/// hashed and re-normalized dozens of times over.
///
/// Maintaining the state is exact rather than approximate because both rules are order-independent: an
/// exact hit depends only on the set of texture names, and a stem hit takes the alphabetically-first key
/// sharing the stem and refuses when two or more share it. `min` and `count` are commutative, so folding
/// names in as they arrive answers exactly what a rebuild from the ordered table would. Replacing a
/// texture's pixels under an existing name cannot change any resolution either, so a merge revisits only the
/// filenames its new names can affect - those whose normalized form equals a new name, and those whose stem
/// equals it - and rewrites only the image slot tables that contain them.
///
/// Slot numbering is append-only, which is what lets an untouched table keep the slots it already has: a
/// resolution that repoints overwrites its own slot instead of shifting the table.
#[derive(Default)]
struct LvzImageResolution {
    /// The texture names present, which is all an exact hit depends on.
    exact: HashSet<Arc<str>>,
    /// Stem -> (alphabetically-first texture name with that stem, how many names share it).
    stems: HashMap<Arc<str>, (Arc<str>, u32)>,
    filenames: Vec<Arc<str>>,
    filename_index: HashMap<Arc<str>, u32>,
    normalized: Vec<Arc<str>>,
    by_normalized: HashMap<Arc<str>, Vec<u32>>,
    by_stem: HashMap<Arc<str>, Vec<u32>>,
    /// Filename index -> the texture name it resolves to; `None` is an unresolvable filename.
    resolution: HashMap<u32, Option<Arc<str>>>,
    slot_of: HashMap<u32, u32>,
    /// A `LvzRenderableObject::image_slot` value indexes this.
    resolved: Vec<ResolvedLvzImage>,
    /// One image table per object-section buffer address, kept alive so an address cannot be recycled.
    table_images: HashMap<usize, Arc<[LvzImageDefinition]>>,
    table_objects: HashMap<usize, Vec<u32>>,
    tables_by_filename: HashMap<u32, Vec<usize>>,
    objects_indexed: usize,
}

/// The frame size a resolution slot reports for a texture name. Generic over the texture table value so
/// the incremental index is testable without macroquad's graphics context, exactly like
/// [`resolve_image_filenames`].
trait LvzFrameSize {
    fn frame_size(&self) -> (f32, f32);
}

impl LvzFrameSize for LvzTexture {
    fn frame_size(&self) -> (f32, f32) {
        (self.width, self.height)
    }
}

#[cfg(test)]
impl LvzFrameSize for (f32, f32) {
    fn frame_size(&self) -> (f32, f32) {
        *self
    }
}

impl LvzImageResolution {
    /// Folds texture names gained since the last call and returns the slot tables that can change.
    fn note_textures<T: LvzFrameSize>(&mut self, textures: &BTreeMap<Arc<str>, T>) -> Vec<usize> {
        let mut invalidated: Vec<u32> = Vec::new();
        for name in textures.keys() {
            if self.exact.contains(name.as_ref()) {
                continue;
            }
            self.exact.insert(Arc::clone(name));
            let stem = lvz_asset_stem(name);
            if let Some(stem) = stem {
                match self.stems.get_mut(stem) {
                    Some((first, count)) => {
                        *count += 1;
                        if name.as_ref() < first.as_ref() {
                            *first = Arc::clone(name);
                        }
                    }
                    None => {
                        self.stems.insert(Arc::from(stem), (Arc::clone(name), 1));
                    }
                }
            }
            if let Some(indices) = self.by_normalized.get(name.as_ref()) {
                invalidated.extend(indices.iter().copied());
            }
            if let Some(stem) = stem {
                if let Some(indices) = self.by_stem.get(stem) {
                    invalidated.extend(indices.iter().copied());
                }
            }
        }
        if invalidated.is_empty() {
            return Vec::new();
        }
        invalidated.sort_unstable();
        invalidated.dedup();
        let mut tables = Vec::new();
        for index in invalidated {
            let value = self.resolve(index);
            self.resolution.insert(index, value);
            if let Some(keys) = self.tables_by_filename.get(&index) {
                tables.extend(keys.iter().copied());
            }
        }
        tables
    }

    /// Indexes the filenames of objects appended since the last call and returns their tables.
    /// Folds up to `max_new` objects gained since the last call and returns the slot tables that can
    /// change. `objects_indexed` is the resume cursor, so a caller that passes a bound resolves a zone's
    /// objects across several frames instead of in one long task.
    fn note_objects(&mut self, objects: &[LvzRenderableObject], max_new: usize) -> Vec<usize> {
        if self.objects_indexed >= objects.len() || max_new == 0 {
            return Vec::new();
        }
        let end = objects
            .len()
            .min(self.objects_indexed.saturating_add(max_new));
        let mut tables = Vec::new();
        for (index, object) in objects
            .iter()
            .enumerate()
            .take(end)
            .skip(self.objects_indexed)
        {
            let key = object.images.as_ptr() as usize;
            let entry = self.table_objects.entry(key).or_default();
            if entry.is_empty() {
                self.table_images.insert(key, Arc::clone(&object.images));
                tables.push(key);
            }
            entry.push(index as u32);
            for image in object.images.iter() {
                let filename = self.index_filename(&image.filename);
                let tables_for = self.tables_by_filename.entry(filename).or_default();
                if !tables_for.contains(&key) {
                    tables_for.push(key);
                }
            }
        }
        self.objects_indexed = end;
        tables
    }

    /// Registers one object image filename, normalizing it once for as long as it is referenced.
    fn index_filename(&mut self, filename: &Arc<str>) -> u32 {
        if let Some(index) = self.filename_index.get(&**filename) {
            return *index;
        }
        let index = self.filenames.len() as u32;
        let normalized = Arc::from(normalize_asset_name(filename).as_str());
        if let Some(stem) = lvz_asset_stem(&normalized) {
            self.by_stem.entry(Arc::from(stem)).or_default().push(index);
        }
        self.by_normalized
            .entry(Arc::clone(&normalized))
            .or_default()
            .push(index);
        self.normalized.push(Arc::clone(&normalized));
        self.filenames.push(Arc::clone(filename));
        let value = self.resolve(index);
        self.resolution.insert(index, value);
        self.filename_index.insert(Arc::clone(filename), index);
        index
    }

    /// The texture name an indexed filename resolves to under the reference rules.
    fn resolve(&self, index: u32) -> Option<Arc<str>> {
        let normalized = self.normalized.get(index as usize)?;
        if self.exact.contains(normalized.as_ref()) {
            return Some(Arc::clone(normalized));
        }
        let stem = lvz_asset_stem(normalized)?;
        let (first, count) = self.stems.get(stem)?;
        if *count == 1 {
            Some(Arc::clone(first))
        } else {
            None
        }
    }

    /// The slot for one filename: allocated on first resolution, overwritten in place when it repoints.
    fn slot_for<T: LvzFrameSize>(
        &mut self,
        filename: &Arc<str>,
        textures: &BTreeMap<Arc<str>, T>,
    ) -> u32 {
        let Some(index) = self.filename_index.get(&**filename).copied() else {
            return UNRESOLVED_IMAGE_SLOT;
        };
        let Some(name) = self.resolution.get(&index).cloned().flatten() else {
            return UNRESOLVED_IMAGE_SLOT;
        };
        let Some(texture) = textures.get(&*name) else {
            return UNRESOLVED_IMAGE_SLOT;
        };
        let frame = texture.frame_size();
        match self.slot_of.get(&index).copied() {
            Some(slot) => {
                if let Some(entry) = self.resolved.get_mut(slot as usize) {
                    entry.texture_name = name;
                    entry.width = frame.0;
                    entry.height = frame.1;
                }
                slot
            }
            None => {
                let slot = self.resolved.len() as u32;
                self.resolved.push(ResolvedLvzImage {
                    texture_name: name,
                    width: frame.0,
                    height: frame.1,
                });
                self.slot_of.insert(index, slot);
                slot
            }
        }
    }

    /// A texture replaced under the same name changes no resolution, so it reaches no invalidation list - but
    /// last-wins pixels can change the frame size every slot pointing at that name reports.
    fn refresh_frames<T: LvzFrameSize>(&mut self, textures: &BTreeMap<Arc<str>, T>) {
        for slot in self.resolved.iter_mut() {
            if let Some(texture) = textures.get(&*slot.texture_name) {
                let (width, height) = texture.frame_size();
                slot.width = width;
                slot.height = height;
            }
        }
    }

    /// Recomputes the slot tables a merge can have changed. A table its invalidation cannot reach keeps the
    /// slots it already has, which is what makes the boot cheap - and what makes append-only slot numbering
    /// load-bearing rather than incidental.
    fn rewrite_tables<T: LvzFrameSize>(
        &mut self,
        tables: &[usize],
        objects: &mut [LvzRenderableObject],
        textures: &BTreeMap<Arc<str>, T>,
    ) {
        let mut keys = tables.to_vec();
        keys.sort_unstable();
        keys.dedup();
        for key in keys {
            let Some(images) = self.table_images.get(&key).cloned() else {
                continue;
            };
            let mut slots = Vec::with_capacity(images.len());
            for image in images.iter() {
                slots.push(self.slot_for(&image.filename, textures));
            }
            let slots: Arc<[u32]> = Arc::from(slots);
            if let Some(indexes) = self.table_objects.get(&key) {
                for index in indexes.iter() {
                    objects[*index as usize].image_slots = Arc::clone(&slots);
                }
            }
        }
    }
}

impl LvzRenderCache {
    /// The resolution for one `LvzRenderableObject::image_slot` value. `None`
    /// means exactly what an `lvz_texture_name_for_image_filename` miss meant to
    /// the caller: draw nothing.
    pub(crate) fn resolved_image(&self, slot: u32) -> Option<&ResolvedLvzImage> {
        if slot == UNRESOLVED_IMAGE_SLOT {
            return None;
        }
        self.image.resolved.get(slot as usize)
    }

    /// One table descent with no allocation, for a name already produced by
    /// `resolved_image`.
    pub(crate) fn texture_for_name(&self, name: &str) -> Option<&LvzTexture> {
        self.textures.get(name)
    }

    /// The objects the map pass for `layer_start..=layer_end` is the only owner
    /// of, in table order. `None` means the caller must walk the whole table.
    pub(crate) fn map_band_objects(
        &self,
        lvz: &NetworkLvzState,
        layer_start: u8,
        layer_end: u8,
    ) -> Option<&[u32]> {
        let band = self.map_band_index(lvz, layer_start, layer_end)?;
        self.map_bands.get(band).map(Vec::as_slice)
    }

    /// The screen-pass counterpart of [`Self::map_band_objects`].
    pub(crate) fn screen_band_objects(
        &self,
        lvz: &NetworkLvzState,
        layer_start: u8,
        layer_end: u8,
    ) -> Option<&[u32]> {
        let band = self.screen_band_index(lvz, layer_start, layer_end)?;
        self.screen_bands.get(band).map(Vec::as_slice)
    }

    /// The map-band objects whose merged frame rectangle can overlap
    /// `min..=max`, in table order, or `None` when the caller must walk the
    /// whole band.
    ///
    /// Declines under the same conditions as [`Self::map_band_objects`] - a
    /// server modification can also move an object to another cell - plus when
    /// the band is small enough to walk directly, and when the query is not a
    /// viewport at all.
    pub(crate) fn map_band_visible_objects(
        &self,
        lvz: &NetworkLvzState,
        layer_start: u8,
        layer_end: u8,
        min: Vec2,
        max: Vec2,
    ) -> Option<Vec<u32>> {
        let band = self.map_band_index(lvz, layer_start, layer_end)?;
        let band_size = self.map_bands.get(band)?.len();
        if band_size < LVZ_SPATIAL_MIN_BAND {
            return None;
        }
        let cells = self.map_band_cells.get(band)?;
        let (x0, y0, x1, y1) = lvz_map_cell_range(min.x, min.y, max.x, max.y);
        if x1 < x0
            || y1 < y0
            || (x1 as i64 - x0 as i64) * (y1 as i64 - y0 as i64) > LVZ_SPATIAL_MAX_CELLS
        {
            return None;
        }
        let mut found: Vec<u32> = Vec::with_capacity(LVZ_SPATIAL_MIN_BAND);
        for y in y0..=y1 {
            for x in x0..=x1 {
                if let Some(indices) = cells.get(&(x, y)) {
                    found.extend_from_slice(indices);
                }
            }
        }
        found.extend_from_slice(&self.map_unresolved_objects);
        // Cell order is not table order. Sorting restores the order the
        // whole-table walk drew in, and dedups an object whose rectangle covers
        // more than one queried cell.
        found.sort_unstable();
        found.dedup();
        Some(found)
    }

    fn map_band_index(
        &self,
        lvz: &NetworkLvzState,
        layer_start: u8,
        layer_end: u8,
    ) -> Option<usize> {
        // `LvzModifyPacket` carries a new `map_object`, layer and position, so a
        // modified object draws in another band - and another cell - than the
        // merged ones the bands and cells were built from.
        if lvz.has_modifications() {
            return None;
        }
        self.exact_band(&LVZ_MAP_BANDS, layer_start, layer_end)
    }

    fn screen_band_index(
        &self,
        lvz: &NetworkLvzState,
        layer_start: u8,
        layer_end: u8,
    ) -> Option<usize> {
        if lvz.has_modifications() {
            return None;
        }
        self.exact_band(&LVZ_SCREEN_BANDS, layer_start, layer_end)
    }

    /// The band whose range is exactly `layer_start..=layer_end`. A wider or
    /// narrower invocation is not that band's business and walks the table.
    fn exact_band(&self, bands: &[(u8, u8)], layer_start: u8, layer_end: u8) -> Option<usize> {
        let band = lvz_band_index(bands, layer_start)?;
        let (start, end) = bands[band];
        if lvz_layer(start) != layer_start || lvz_layer(end) != layer_end {
            return None;
        }
        Some(band)
    }

    /// Objects no band claims. Every pass walks these on top of its own band, so
    /// an unexpected layer value costs visits rather than dropping the object.
    pub(crate) fn unbanded_objects(&self) -> &[u32] {
        &self.unbanded_objects
    }

    pub(crate) fn renderable_at(&self, index: u32) -> Option<&LvzRenderableObject> {
        self.objects.get(index as usize)
    }

    /// Rebuilt with the memo, for the same reason: which cells an object covers
    /// depends on the frame extent its image resolves to, and a later archive
    /// can change that resolution.
    fn rebuild_spatial_bands(&mut self) {
        self.map_bands = vec![Vec::new(); LVZ_MAP_BANDS.len()];
        self.screen_bands = vec![Vec::new(); LVZ_SCREEN_BANDS.len()];
        self.map_band_cells = vec![BTreeMap::new(); LVZ_MAP_BANDS.len()];
        self.unbanded_objects.clear();
        self.map_unresolved_objects.clear();

        for index in 0..self.objects.len() {
            let (map_object, layer) = {
                let renderable = &self.objects[index];
                (renderable.object.map_object, renderable.object.layer)
            };
            let bands = if map_object {
                &LVZ_MAP_BANDS[..]
            } else {
                &LVZ_SCREEN_BANDS[..]
            };
            let Some(band) = lvz_band_index(bands, layer) else {
                self.unbanded_objects.push(index as u32);
                continue;
            };
            if map_object {
                push_band_index(&mut self.map_bands, band, index as u32);
                match self.map_object_frame_rect(&self.objects[index]) {
                    Some((pos, extent)) => {
                        self.file_map_object_in_cells(band, index as u32, pos, extent)
                    }
                    None => self.map_unresolved_objects.push(index as u32),
                }
            } else {
                push_band_index(&mut self.screen_bands, band, index as u32);
            }
        }
    }

    /// The merged position and frame rectangle of a map object, `None` when its
    /// image does not resolve. An object of unknown extent cannot be filed into
    /// cells, so it is walked by every map pass instead.
    fn map_object_frame_rect(&self, renderable: &LvzRenderableObject) -> Option<(Vec2, Vec2)> {
        let image = renderable
            .images
            .get(renderable.object.image_number as usize)?;
        let resolved =
            self.resolved_image(renderable.image_slot(renderable.object.image_number))?;
        Some((
            lvz_map_object_world_position(renderable.object.x_map, renderable.object.y_map),
            vec2(
                resolved.width / image.x_count.max(1) as f32,
                resolved.height / image.y_count.max(1) as f32,
            ),
        ))
    }

    /// Files a map object into every cell its frame rectangle covers.
    fn file_map_object_in_cells(&mut self, band: usize, index: u32, pos: Vec2, extent: Vec2) {
        let Some(cells) = self.map_band_cells.get_mut(band) else {
            return;
        };
        let (x0, y0, x1, y1) = lvz_map_cell_range(pos.x, pos.y, pos.x + extent.x, pos.y + extent.y);
        for y in y0..=y1 {
            for x in x0..=x1 {
                cells.entry((x, y)).or_default().push(index);
            }
        }
    }

    /// Keeps [`LvzImageResolution`] current rather than rebuilding resolution from scratch: a later archive
    /// can still supply the texture an earlier object filename only resolved by stem, and image textures are
    /// last-wins, so the filenames a merge's new names can affect are recomputed and their slot tables
    /// rewritten. Everything those rules cannot reach keeps its resolution.
    fn rebuild_resolved_images(&mut self) {
        let mut tables = self.image.note_textures(&self.textures);
        tables.extend(self.image.note_objects(&self.objects, usize::MAX));
        self.image
            .rewrite_tables(&tables, &mut self.objects, &self.textures);
        self.image.refresh_frames(&self.textures);
    }

    /// Merge one archive and finalize the whole-cache image resolve and spatial banding inline.
    /// Callers that merge a batch of archives (a zone join is fourteen) should use
    /// [`Self::merge_archive_deferred`] and call [`Self::finalize_deferred_merges`] once: banding
    /// walks every object in the cache, so finalizing per archive made a join
    /// O(archives x objects) - the measured 112-116 ms worst single install.
    pub(crate) fn merge_archive(&mut self, archive: &LvzArchive) -> Result<(), String> {
        self.merge_archive_inner(archive)?;
        self.rebuild_resolved_images();
        self.rebuild_spatial_bands();
        Ok(())
    }

    /// Merge one archive of a batch without the whole-cache resolve/band pass. Objects added this
    /// way have no spatial bands until [`Self::finalize_deferred_merges`], so they are not drawn; that
    /// is safe because every caller holds gameplay and the zone publication until the batch is done.
    pub(crate) fn merge_archive_deferred(&mut self, archive: &LvzArchive) -> Result<(), String> {
        self.merge_archive_inner(archive)?;
        self.merge_deferred = true;
        Ok(())
    }

    /// Run the postponed resolve/band pass once for a batch of deferred merges. Idempotent and cheap
    /// when nothing was deferred. Used by the startup/offline local merge, which has no frame budget to
    /// drive a resumable pass.
    pub(crate) fn finalize_deferred_merges(&mut self) {
        if !self.merge_deferred {
            return;
        }
        self.merge_deferred = false;
        self.rebuild_resolved_images();
        self.rebuild_spatial_bands();
    }

    /// Resumable finalize: resolve at most `max_new_objects` newly merged objects this call and band
    /// once when the resolve completes, returning true when nothing remains. The per-object image
    /// resolve is the join's measured hot task (one Chaos LVZ carries ~27k of the zone's 28.9k
    /// objects, a ~54 ms task), so a caller that drives this once per frame bounds the worst frame
    /// while gameplay and the zone publication are held. Cheap and inert when nothing was deferred.
    pub(crate) fn finalize_deferred_merges_budgeted(&mut self, max_new_objects: usize) -> bool {
        if !self.merge_deferred {
            return true;
        }
        let mut tables = self.image.note_textures(&self.textures);
        tables.extend(
            self.image
                .note_objects(&self.objects, max_new_objects.max(1)),
        );
        self.image
            .rewrite_tables(&tables, &mut self.objects, &self.textures);
        self.image.refresh_frames(&self.textures);
        if self.image.objects_indexed < self.objects.len() {
            return false;
        }
        self.rebuild_spatial_bands();
        self.merge_deferred = false;
        true
    }

    fn merge_archive_inner(&mut self, archive: &LvzArchive) -> Result<(), String> {
        // Parsing seals only after full CPU validation of every payload.
        // Consume prepared last-wins pixels when peak headroom permitted their
        // handoff; otherwise decode one bounded image at a time for upload.
        archive.require_full_validation_seal()?;
        let mut archive_usage = archive.render_usage()?;
        archive_usage.decoded_source_bytes = 0;
        archive_usage.pixel_bytes = 0;
        archive_usage.texture_count = 0;
        let candidate_usage = self.render_usage.checked_add(archive_usage)?;
        let mut replacements = BTreeMap::new();
        for (name, bytes) in archive.render_image_allocations()? {
            replacements.insert(name, bytes);
        }
        let before = crate::render_limits::checked_zone_render_totals(
            self.render_usage.metadata_bytes,
            0,
            &self.render_images,
        )?;
        let candidate = crate::render_limits::checked_zone_render_totals_with_replacements(
            candidate_usage.metadata_bytes,
            0,
            &self.render_images,
            &replacements,
        )?;
        let largest_upload = replacements.values().copied().max().unwrap_or(0);
        crate::render_limits::checked_zone_render_peak(before.0, candidate.0, largest_upload)?;
        let object_image_filenames = archive.object_image_filenames();
        // Coalesce normalized File sections before the renderer decodes for
        // GPU upload. Parsing already fully decoded every payload one at a
        // time, including overwritten duplicates, so malformed duplicates
        // still fail closed while only the last payload can consume retained
        // texture state.
        let last_image_sections = coalesced_render_image_sections(archive)?;

        for (normalized_name, (index, _)) in last_image_sections {
            let image = lvz_image_for_upload(archive, index)?;
            let texture =
                Texture2D::from_rgba8(image.width as u16, image.height as u16, &image.rgba);
            texture.set_filter(FilterMode::Nearest);
            self.record_graphic_override(&normalized_name, &object_image_filenames);
            self.textures.insert(
                Arc::clone(&normalized_name),
                LvzTexture {
                    texture,
                    width: image.width as f32,
                    height: image.height as f32,
                },
            );
            self.render_images.insert(
                Arc::clone(&normalized_name),
                *replacements
                    .get(normalized_name.as_ref())
                    .expect("coalesced image has an allocation"),
            );
        }
        archive.discard_prepared_images();

        for object_section in archive.object_sections() {
            let images: Arc<[LvzImageDefinition]> = Arc::from(object_section.images.clone());
            for object in &object_section.objects {
                let index = self.objects.len();
                let first_for_id = !self.objects_by_id.contains_key(&object.object_id);
                self.objects.push(LvzRenderableObject {
                    object: object.clone(),
                    images: Arc::clone(&images),
                    // Filled by `rebuild_resolved_images` before this merge returns.
                    image_slots: Arc::from(Vec::new()),
                    first_for_id,
                });
                self.objects_by_id
                    .entry(object.object_id)
                    .or_default()
                    .push(index);
            }
        }
        self.render_usage = candidate_usage;
        Ok(())
    }

    fn record_graphic_override(
        &mut self,
        normalized_name: &str,
        object_image_filenames: &BTreeSet<Arc<str>>,
    ) {
        let Some(stem) =
            lvz_graphic_override_stem_for_file(normalized_name, object_image_filenames)
        else {
            return;
        };
        self.graphic_overrides
            .entry(Arc::from(stem))
            .or_insert_with(|| Arc::from(normalized_name));
    }

    pub(crate) fn expiration_seconds_for_object(
        &self,
        object_id: u16,
        state: &NetworkLvzState,
    ) -> Option<f32> {
        let indexes = self.objects_by_id.get(&object_id)?;
        let first = self.objects.get(*indexes.first()?)?;
        let object = first
            .object
            .modified(state.modified_objects.get(&object_id));
        Some(lvz_display_time_seconds(object.display_time))
    }
}

fn coalesced_render_image_sections(
    archive: &LvzArchive,
) -> Result<BTreeMap<Arc<str>, (usize, &super::archive::LvzSection)>, String> {
    let mut sections = BTreeMap::new();
    for (index, section) in archive.sections().iter().enumerate() {
        if matches!(section.kind, LvzSectionKind::File)
            && crate::render_limits::image_allocation_for_filename(
                &section.filename,
                &section.data,
            )?
            .is_some()
        {
            sections.insert(Arc::clone(&section.filename), (index, section));
        }
    }
    Ok(sections)
}

fn lvz_image_for_upload(
    archive: &LvzArchive,
    index: usize,
) -> Result<crate::bm2::DecodedImage, String> {
    archive.require_full_validation_seal()?;
    if let Some(image) = archive.take_prepared_image(index) {
        return Ok(image);
    }
    let section = archive
        .sections()
        .get(index)
        .ok_or("missing LVZ upload section")?;
    crate::render_limits::decode_image_for_filename(&section.filename, &section.data)?
        .ok_or_else(|| "LVZ upload section is not an image".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    #[test]
    fn memoised_image_resolution_matches_the_per_visit_lookup_exactly() {
        let textures: BTreeMap<String, ()> = BTreeMap::from([
            ("maps/terrain.bmp".to_string(), ()),
            ("tut1.jpg".to_string(), ()),
            ("folder/tut2.gif".to_string(), ()),
            ("banner.gif".to_string(), ()),
            ("folder/banner.jpg".to_string(), ()),
        ]);
        let filenames = [
            "maps\\terrain.bmp", // windows separator normalises onto an exact key
            "tut1.gif",          // unique-stem fallback
            "folder/tut2.gif",   // exact key
            "banner.png",        // stem matches two keys, so nothing resolves it
            "missing.gif",       // no key and no stem match
            "tut1.gif",          // repeated input collapses to one entry
        ];

        let resolved = resolve_image_filenames(filenames.iter().copied(), &textures);

        for filename in filenames.iter().copied() {
            assert_eq!(
                resolved.get(filename).map(|name| name.as_ref()),
                lvz_texture_name_for_image_filename(&textures, filename),
                "memoised resolution differs for {filename}"
            );
        }
        // The draw passes hand the stored name straight back to the table, so a
        // resolution is only useful if it is a live key.
        for name in resolved.values() {
            assert!(
                textures.contains_key(name.as_ref()),
                "{name} is not a texture-table key"
            );
        }
        assert_eq!(resolved.len(), 3, "only resolvable filenames are stored");
    }

    #[test]
    fn a_later_merge_that_adds_a_texture_repoints_the_earlier_stem_resolution() {
        // The first archive ships the image under another name, so the object
        // filename only resolves through the unique-stem fallback.
        let mut textures: BTreeMap<String, ()> = BTreeMap::from([("alt/tut1.bmp".to_string(), ())]);
        let first = resolve_image_filenames(["tut1.gif"], &textures);
        assert_eq!(
            first.get("tut1.gif").map(|name| name.as_ref()),
            Some("alt/tut1.bmp")
        );

        // Image textures are last-wins, so the merge-time memo has to be recomputed
        // over the merged table: a resolution kept from the first merge would now
        // draw the superseded image.
        textures.insert("tut1.gif".to_string(), ());
        let merged = resolve_image_filenames(["tut1.gif"], &textures);
        assert_eq!(
            merged.get("tut1.gif").map(|name| name.as_ref()),
            Some("tut1.gif")
        );
    }

    #[test]
    fn the_incremental_index_answers_exactly_what_the_reference_computes() {
        // Chosen so the rules interact: `exact.png` hits exactly, `stem.bmp` resolves only through the
        // unique-stem fallback, `dupe.bmp` becomes ambiguous when the second `dupe.*` key arrives (so it has
        // to travel resolved -> unresolved), `never.bmp` matches the extensionless `never` key and must stay
        // unresolved, and `dir/art.png` is a pathed name that hits exactly.
        let filenames = [
            "exact.png",
            "stem.bmp",
            "dupe.bmp",
            "never.bmp",
            "dir/art.png",
        ];
        // Last-wins textures, shipped in the order the archives deliver them.
        let shipments: Vec<Vec<(Arc<str>, (f32, f32))>> = vec![
            vec![("dupe.gif".into(), (3.0, 4.0))],
            vec![
                ("stem.png".into(), (8.0, 9.0)),
                ("never".into(), (1.0, 1.0)),
            ],
            vec![
                ("exact.png".into(), (16.0, 20.0)),
                ("dupe.png".into(), (5.0, 6.0)),
            ],
            vec![("dir/art.png".into(), (2.0, 7.0))],
        ];

        // A boot knows the map's objects before most archives ship; a late zone archive ships its objects
        // with its textures. Order-independence is what licenses maintaining the state at all.
        for objects_first in [true, false] {
            let mut image = LvzImageResolution::default();
            let mut textures: BTreeMap<Arc<str>, (f32, f32)> = BTreeMap::new();
            let mut objects: Vec<LvzRenderableObject> = Vec::new();
            let mut merged_objects = false;
            let ship_objects = |objects: &mut Vec<LvzRenderableObject>| {
                let images: Arc<[LvzImageDefinition]> = Arc::from(
                    filenames
                        .iter()
                        .map(|name| LvzImageDefinition {
                            x_count: 1,
                            y_count: 1,
                            animation_time: 0,
                            filename: Arc::from(*name),
                        })
                        .collect::<Vec<_>>(),
                );
                objects.push(LvzRenderableObject {
                    object: band_object(7, true, 0),
                    images,
                    image_slots: Arc::from(Vec::new()),
                    first_for_id: true,
                });
            };
            if objects_first {
                ship_objects(&mut objects);
                merged_objects = true;
            }
            for shipment in &shipments {
                for (name, size) in shipment {
                    textures.insert(Arc::clone(name), *size);
                }
                if !objects_first && !merged_objects {
                    ship_objects(&mut objects);
                    merged_objects = true;
                }
                let mut tables = image.note_textures(&textures);
                tables.extend(image.note_objects(&objects, usize::MAX));
                image.rewrite_tables(&tables, &mut objects, &textures);
                image.refresh_frames(&textures);
            }

            let reference = resolve_image_filenames(filenames, &textures);
            assert_eq!(objects.len(), 1, "one object table ships in either order");
            for (position, filename) in filenames.iter().enumerate() {
                let slot = objects[0].image_slot(position as u8);
                match reference.get(*filename) {
                    Some(name) => {
                        let entry = image.resolved.get(slot as usize).unwrap_or_else(|| {
                            panic!("{filename} resolves in the reference but not live")
                        });
                        assert_eq!(
                            &*entry.texture_name, &**name,
                            "objects_first={objects_first}"
                        );
                        let (width, height) =
                            textures.get(name.as_ref()).expect("resolved name is a key");
                        assert_eq!((entry.width, entry.height), (*width, *height));
                    }
                    None => assert_eq!(
                        slot, UNRESOLVED_IMAGE_SLOT,
                        "objects_first={objects_first}: {filename} must stay unresolved"
                    ),
                }
            }
            assert_eq!(
                objects[0]
                    .image_slots
                    .iter()
                    .filter(|slot| **slot != UNRESOLVED_IMAGE_SLOT)
                    .count(),
                reference.len(),
                "the same filenames resolve in both implementations"
            );
        }
    }

    #[test]
    fn a_merge_that_cannot_affect_a_table_leaves_its_slots_alone() {
        // The cheap boot depends on this: a new texture must rewrite only the tables that reference it, which
        // only stays correct while slot numbering is append-only.
        let mut image = LvzImageResolution::default();
        let mut textures: BTreeMap<Arc<str>, (f32, f32)> = BTreeMap::new();
        let mut objects: Vec<LvzRenderableObject> = Vec::new();
        let mut ship_table = |objects: &mut Vec<LvzRenderableObject>, filename: &str| {
            let images: Arc<[LvzImageDefinition]> = Arc::from(vec![LvzImageDefinition {
                x_count: 1,
                y_count: 1,
                animation_time: 0,
                filename: Arc::from(filename),
            }]);
            objects.push(LvzRenderableObject {
                object: band_object(objects.len() as u16, true, 0),
                images,
                image_slots: Arc::from(Vec::new()),
                first_for_id: true,
            });
        };
        ship_table(&mut objects, "first.png");
        ship_table(&mut objects, "second.png");
        textures.insert(Arc::from("first.png"), (4.0, 6.0));
        let mut tables = image.note_textures(&textures);
        tables.extend(image.note_objects(&objects, usize::MAX));
        image.rewrite_tables(&tables, &mut objects, &textures);
        let first_slots = Arc::clone(&objects[0].image_slots);
        let first_slot = objects[0].image_slot(0);
        assert_eq!(first_slot, 0, "the first resolution takes the first slot");

        // A later archive ships the other table's texture.
        textures.insert(Arc::from("second.png"), (7.0, 8.0));
        let tables = image.note_textures(&textures);
        assert_eq!(
            tables.len(),
            1,
            "only the table referencing the new name can change"
        );
        image.rewrite_tables(&tables, &mut objects, &textures);
        image.refresh_frames(&textures);

        assert!(
            Arc::ptr_eq(&first_slots, &objects[0].image_slots),
            "an unaffected table keeps the array it already had"
        );
        assert_eq!(
            objects[0].image_slot(0),
            first_slot,
            "slot numbers do not shift"
        );
        assert_ne!(objects[1].image_slot(0), UNRESOLVED_IMAGE_SLOT);
        assert_eq!(
            image.resolved[objects[1].image_slot(0) as usize].width,
            7.0,
            "the new texture reports its own frame width"
        );

        // Last-wins pixels under the same name change no resolution, so they reach no invalidation list; the
        // frame sweep is what keeps the slot honest.
        textures.insert(Arc::from("first.png"), (11.0, 12.0));
        assert!(image.note_textures(&textures).is_empty());
        image.refresh_frames(&textures);
        assert_eq!(
            image.resolved[first_slot as usize].width, 11.0,
            "a replaced texture's new frame size reaches the slot that points at it"
        );
    }

    #[test]
    fn an_extensionless_key_shares_no_stem_with_an_image_filename() {
        // The stem index skips keys that have no extension. Were it to treat such a key as its own stem,
        // a `README` key would start matching `README.gif` and draw the wrong thing.
        let textures: BTreeMap<String, ()> =
            BTreeMap::from([("tut1".to_string(), ()), ("notes".to_string(), ())]);
        let resolved = resolve_image_filenames(["tut1.gif", "notes.png"], &textures);
        assert!(
            resolved.is_empty(),
            "a key with no extension has no stem to match"
        );
        for filename in ["tut1.gif", "notes.png"] {
            assert_eq!(
                lvz_texture_name_for_image_filename(&textures, filename),
                None,
                "the per-visit lookup agrees for {filename}"
            );
        }
    }

    fn band_object(id: u16, map_object: bool, layer: u8) -> LvzObjectDefinition {
        LvzObjectDefinition {
            map_object,
            object_id: id,
            x_map: 0,
            y_map: 0,
            x_screen: 0,
            y_screen: 0,
            x_type: 0,
            y_type: 0,
            image_number: 0,
            layer,
            display_time: 0,
            display_mode: crate::lvz::LVZ_DISPLAY_SHOW_ALWAYS,
        }
    }

    fn band_archive(objects: Vec<LvzObjectDefinition>) -> LvzArchive {
        LvzArchive::validated_for_test(vec![crate::lvz::archive::LvzSection {
            filename: "".into(),
            timestamp: 0,
            data: Arc::from([]),
            kind: LvzSectionKind::Objects(crate::lvz::archive::LvzObjectSection {
                version: LvzArchive::MAGIC_CLV1,
                objects,
                images: vec![LvzImageDefinition {
                    x_count: 1,
                    y_count: 1,
                    animation_time: 0,
                    filename: "sprite.bmp".into(),
                }],
            }),
        }])
        .unwrap()
    }

    /// One map and one screen object per layer, including two values above the
    /// topmost layer to exercise the clamp every band test relies on.
    fn band_probe_objects() -> Vec<LvzObjectDefinition> {
        let mut objects = Vec::new();
        for layer in [0u8, 1, 2, 3, 4, 5, 6, 7, 9, 200] {
            objects.push(band_object(objects.len() as u16, true, layer));
            objects.push(band_object(objects.len() as u16, false, layer));
        }
        objects
    }

    #[test]
    fn deferred_merge_finalizes_to_the_same_bands_as_inline() {
        // A zone join defers the whole-cache resolve/band pass to one finalize, so it must produce exactly
        // the state an inline per-archive merge would - otherwise a deferred join would draw a different zone.
        let first = band_probe_objects();
        let second = band_probe_objects();

        let mut inline = LvzRenderCache::default();
        inline.merge_archive(&band_archive(first.clone())).unwrap();
        inline.merge_archive(&band_archive(second.clone())).unwrap();

        let mut deferred = LvzRenderCache::default();
        deferred
            .merge_archive_deferred(&band_archive(first))
            .unwrap();
        deferred
            .merge_archive_deferred(&band_archive(second))
            .unwrap();
        // Before the finalize the new objects carry no band: that absence is exactly the postponed cost.
        assert!(
            deferred.map_bands.iter().all(|band| band.is_empty())
                && deferred.screen_bands.iter().all(|band| band.is_empty()),
            "a deferred merge must not band objects before the finalize"
        );
        deferred.finalize_deferred_merges();
        // Idempotent and cheap when nothing is pending.
        deferred.finalize_deferred_merges();

        assert_eq!(deferred.objects.len(), inline.objects.len());
        assert_eq!(deferred.map_bands, inline.map_bands);
        assert_eq!(deferred.screen_bands, inline.screen_bands);
        assert_eq!(deferred.unbanded_objects, inline.unbanded_objects);
        assert_eq!(
            deferred.map_unresolved_objects,
            inline.map_unresolved_objects
        );

        // The budgeted drive used by the join must reach the same state when driven one object at a
        // time, and must report completion exactly on the call that bands.
        let mut budgeted = LvzRenderCache::default();
        budgeted
            .merge_archive_deferred(&band_archive(band_probe_objects()))
            .unwrap();
        budgeted
            .merge_archive_deferred(&band_archive(band_probe_objects()))
            .unwrap();
        let mut calls = 0_usize;
        while !budgeted.finalize_deferred_merges_budgeted(1) {
            calls += 1;
            assert!(calls < 100_000, "budgeted finalize must terminate");
        }
        assert_eq!(budgeted.map_bands, inline.map_bands);
        assert_eq!(budgeted.screen_bands, inline.screen_bands);
        assert_eq!(budgeted.unbanded_objects, inline.unbanded_objects);
        assert_eq!(
            budgeted.map_unresolved_objects,
            inline.map_unresolved_objects
        );
        // Idempotent once complete.
        assert!(budgeted.finalize_deferred_merges_budgeted(1));
    }

    #[test]
    fn each_pass_band_walks_exactly_the_objects_the_full_table_walk_visits() {
        let lvz = NetworkLvzState::default();
        let first = band_probe_objects();
        let second = band_probe_objects();
        let mut all = first.clone();
        all.extend(second.iter().cloned());
        let mut cache = LvzRenderCache::default();
        // Two merges: a band must keep appending in table order across merges,
        // which is how a zone split over several archives draws.
        cache.merge_archive(&band_archive(first)).unwrap();
        cache.merge_archive(&band_archive(second)).unwrap();
        assert_eq!(cache.objects.len(), all.len());
        assert!(
            cache.unbanded_objects().is_empty(),
            "clamped layers 0..=7 are covered by the shipped bands, so nothing may fall outside them"
        );

        for &(start, end) in &LVZ_MAP_BANDS {
            let expected: Vec<u32> = all
                .iter()
                .enumerate()
                .filter(|(_, object)| {
                    object.map_object && lvz_layer_in_range(object.layer, start, end)
                })
                .map(|(index, _)| index as u32)
                .collect();
            let walked: Vec<u32> = cache
                .map_band_objects(&lvz, start, end)
                .expect("the shipped map bands must be recognised")
                .iter()
                .chain(cache.unbanded_objects())
                .copied()
                .collect();
            assert_eq!(
                walked, expected,
                "map band {start}..={end} must walk exactly the objects the full-table walk visits, in table order"
            );
        }
        for &(start, end) in &LVZ_SCREEN_BANDS {
            let expected: Vec<u32> = all
                .iter()
                .enumerate()
                .filter(|(_, object)| {
                    !object.map_object && lvz_layer_in_range(object.layer, start, end)
                })
                .map(|(index, _)| index as u32)
                .collect();
            let walked: Vec<u32> = cache
                .screen_band_objects(&lvz, start, end)
                .expect("the shipped screen bands must be recognised")
                .iter()
                .chain(cache.unbanded_objects())
                .copied()
                .collect();
            assert_eq!(
                walked, expected,
                "screen band {start}..={end} must walk exactly the objects the full-table walk visits, in table order"
            );
        }

        // Any range that is not a shipped band must fall back to the full walk.
        assert!(cache.map_band_objects(&lvz, 1, 3).is_none());
        assert!(cache.map_band_objects(&lvz, 0, 7).is_none());
        assert!(cache.screen_band_objects(&lvz, 0, 7).is_none());
        assert!(cache.screen_band_objects(&lvz, 2, 6).is_none());

        // The eight bands partition the table: one visit per object per frame.
        let mut visits = vec![0_u8; all.len()];
        for band in &cache.map_bands {
            for index in band {
                visits[*index as usize] += 1;
            }
        }
        for band in &cache.screen_bands {
            for index in band {
                visits[*index as usize] += 1;
            }
        }
        for index in cache.unbanded_objects() {
            visits[*index as usize] += 1;
        }
        assert!(
            visits.iter().all(|count| *count == 1),
            "every object must be owned by exactly one pass per frame, got {visits:?}"
        );

        // Fail-safe: an object no band claims must still be reached. Shipped
        // zones never land here - the bands cover the whole clamped range - so
        // this pins the behaviour if a band table ever stops covering a layer.
        cache.unbanded_objects.push(3);
        for &(start, end) in &LVZ_MAP_BANDS {
            let walked: Vec<u32> = cache
                .map_band_objects(&lvz, start, end)
                .expect("the shipped map bands must be recognised")
                .iter()
                .chain(cache.unbanded_objects())
                .copied()
                .collect();
            assert!(
                walked.contains(&3),
                "map band {start}..={end} must walk an object no band claims"
            );
        }
        for &(start, end) in &LVZ_SCREEN_BANDS {
            let walked: Vec<u32> = cache
                .screen_band_objects(&lvz, start, end)
                .expect("the shipped screen bands must be recognised")
                .iter()
                .chain(cache.unbanded_objects())
                .copied()
                .collect();
            assert!(
                walked.contains(&3),
                "screen band {start}..={end} must walk an object no band claims"
            );
        }

        // A modify packet can move an object to another class or layer, so the
        // bands must decline the moment one is in effect.
        let mut modified = NetworkLvzState::default();
        modified.apply_modify(crate::LvzModifyPacket {
            changed: crate::LvzModificationFlags {
                xy: false,
                image: false,
                layer: true,
                time: false,
                mode: false,
            },
            map_object: false,
            object_id: 0,
            map_x: 0,
            map_y: 0,
            screen_x: 0,
            screen_y: 0,
            x_type: 0,
            y_type: 0,
            image_id: 0,
            layer: crate::lvz::LVZ_LAYER_TOPMOST,
            display_time: 0,
            display_mode: crate::lvz::LVZ_DISPLAY_SHOW_ALWAYS,
        });
        for &(start, end) in &LVZ_MAP_BANDS {
            assert!(
                cache.map_band_objects(&modified, start, end).is_none(),
                "a modification must send the map pass back to the full table walk"
            );
        }
        for &(start, end) in &LVZ_SCREEN_BANDS {
            assert!(
                cache.screen_band_objects(&modified, start, end).is_none(),
                "a modification must send the screen pass back to the full table walk"
            );
        }
    }

    /// 300 map objects on one band - the size at which the spatial index engages
    /// - on cell edges, one pixel inside them, and in both sign halves.
    fn spatial_probe_objects() -> Vec<LvzObjectDefinition> {
        (0..300_u16)
            .map(|index| {
                let row = (index / 5) as i32;
                let x_map = match index % 5 {
                    0 => row * 512,
                    1 => row * 512 - 1,
                    2 => -(row * 512) - 1,
                    3 => row * 64 - 12_000,
                    _ => row * 512 - 4_000,
                };
                let y_map = match index % 3 {
                    0 => row * 256,
                    1 => -(row * 300),
                    _ => 1_000 - row,
                };
                LvzObjectDefinition {
                    map_object: true,
                    object_id: index,
                    x_map: i16::try_from(x_map).expect("probe position fits i16"),
                    y_map: i16::try_from(y_map).expect("probe position fits i16"),
                    x_screen: 0,
                    y_screen: 0,
                    x_type: 0,
                    y_type: 0,
                    image_number: 0,
                    layer: crate::lvz::LVZ_LAYER_AFTER_TILES,
                    display_time: 0,
                    display_mode: crate::lvz::LVZ_DISPLAY_SHOW_ALWAYS,
                }
            })
            .collect()
    }

    fn spatial_probe_archive(objects: Vec<LvzObjectDefinition>) -> LvzArchive {
        LvzArchive::validated_for_test(vec![crate::lvz::archive::LvzSection {
            filename: "".into(),
            timestamp: 0,
            data: Arc::from([]),
            kind: LvzSectionKind::Objects(crate::lvz::archive::LvzObjectSection {
                version: LvzArchive::MAGIC_CLV1,
                objects,
                images: vec![LvzImageDefinition {
                    x_count: 1,
                    y_count: 1,
                    animation_time: 0,
                    filename: "sprite.bmp".into(),
                }],
            }),
        }])
        .unwrap()
    }

    /// Cell ranges for rectangles whose edges sit on, one pixel inside, and one
    /// pixel outside a cell boundary, including the negative half where flooring
    /// rather than truncating is what keeps a straddling rectangle whole.
    #[test]
    fn cell_ranges_cover_exactly_the_cells_a_rectangle_touches() {
        for (min, max, expected) in [
            (vec2(0.0, 0.0), vec2(0.0, 0.0), (0, 0, 0, 0)),
            (vec2(0.0, 0.0), vec2(512.0, 512.0), (0, 0, 1, 1)),
            (vec2(-1.0, 0.0), vec2(0.0, 0.0), (-1, 0, 0, 0)),
            (vec2(-512.0, 512.0), vec2(-1.0, 512.0), (-1, 1, -1, 1)),
            (vec2(-64.0, 100.0), vec2(64.0, 100.0), (-1, 0, 0, 0)),
            (
                vec2(1_024.0, -1_024.0),
                vec2(1_200.0, -900.0),
                (2, -2, 2, -2),
            ),
        ] {
            assert_eq!(
                lvz_map_cell_range(min.x, min.y, max.x, max.y),
                expected,
                "{min:?}..={max:?} must cover {expected:?}"
            );
        }
    }

    /// The invariant the pixels depend on: every object the viewport overlap test
    /// would draw is in the cell query's candidates, in table order, and
    /// off-screen objects are dropped.
    #[test]
    fn the_cell_query_never_omits_a_map_object_the_viewport_walk_would_draw() {
        let objects = spatial_probe_objects();
        let mut cache = LvzRenderCache::default();
        cache
            .merge_archive(&spatial_probe_archive(objects.clone()))
            .unwrap();
        let lvz = NetworkLvzState::default();
        let (start, end) = LVZ_MAP_BANDS[2];
        assert_eq!(cache.map_bands[2].len(), objects.len());
        assert_eq!(cache.map_unresolved_objects.len(), objects.len());

        // A native test cannot upload a texture, so file the objects by hand with
        // the extent a resolved 64x64 single-frame image gives them.
        cache.map_unresolved_objects.clear();
        for (index, object) in objects.iter().enumerate() {
            let pos = lvz_map_object_world_position(object.x_map, object.y_map);
            cache.file_map_object_in_cells(2, index as u32, pos, vec2(64.0, 64.0));
        }

        let extent = vec2(64.0, 64.0);
        let mut filtered_any = false;
        for center in [
            vec2(0.0, 0.0),
            vec2(1_536.0, -1_024.0),
            vec2(-16_384.0, 8_192.0),
            vec2(26_000.0, 15_000.0),
        ] {
            let min = center - vec2(1_216.0, 796.0);
            let max = center + vec2(1_216.0, 796.0);
            let candidates = cache
                .map_band_visible_objects(&lvz, start, end, min, max)
                .expect("a band this size must use the spatial index");
            assert!(
                candidates.windows(2).all(|pair| pair[0] < pair[1]),
                "candidates must come back in table order with no repeats"
            );
            filtered_any |= candidates.len() < objects.len();
            for (index, object) in objects.iter().enumerate() {
                let pos = lvz_map_object_world_position(object.x_map, object.y_map);
                // Exactly the pass' own overlap test against the same box.
                let drawn = pos.x <= max.x
                    && pos.y <= max.y
                    && pos.x + extent.x >= min.x
                    && pos.y + extent.y >= min.y;
                assert!(
                    !drawn || candidates.contains(&(index as u32)),
                    "object {index} at {pos:?} overlaps {min:?}..={max:?} but the cell query omitted it"
                );
            }
        }
        assert!(
            filtered_any,
            "the cell query must actually omit off-screen objects, not restate the band"
        );

        // With no resolvable image the extent is unknown, so every map object has
        // to reach every map pass rather than risk dropping one.
        let mut unresolved = LvzRenderCache::default();
        unresolved
            .merge_archive(&spatial_probe_archive(objects.clone()))
            .unwrap();
        let candidates = unresolved
            .map_band_visible_objects(
                &lvz,
                start,
                end,
                vec2(-10_000.0, -10_000.0),
                vec2(-9_900.0, -9_800.0),
            )
            .expect("a band this size must use the spatial index");
        assert_eq!(
            candidates.len(),
            objects.len(),
            "objects of unknown extent must all be walked"
        );
    }

    #[test]
    fn renderer_consumes_validated_pixels_without_another_decode_and_falls_back_when_absent() {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[1, 2, 3, 4], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let mut archive = LvzArchive::unvalidated_for_usage_test(vec![super::super::LvzSection {
            filename: Arc::from("one.png"),
            timestamp: 1,
            data: Arc::from(png),
            kind: LvzSectionKind::File,
        }]);
        assert!(
            lvz_image_for_upload(&archive, 0).is_err(),
            "unsealed data is not uploadable"
        );
        crate::render_limits::reset_full_image_validation_calls();
        archive
            .seal_with_prepared_images(
                &mut crate::render_limits::ImagePreparationBudget::for_zone_high_water(0),
            )
            .unwrap();
        assert_eq!(crate::render_limits::full_image_validation_calls(), 1);
        assert_eq!(
            lvz_image_for_upload(&archive, 0).unwrap().rgba,
            [1, 2, 3, 4]
        );
        assert_eq!(crate::render_limits::full_image_validation_calls(), 1);
        assert_eq!(
            lvz_image_for_upload(&archive, 0).unwrap().rgba,
            [1, 2, 3, 4]
        );
        assert_eq!(
            crate::render_limits::full_image_validation_calls(),
            2,
            "a later rebuild uses the bounded fallback"
        );
        assert!(lvz_image_for_upload(&archive, 1).is_err());
    }

    fn compressed_2048_png() -> Vec<u8> {
        let pixels = vec![0_u8; 2_048 * 2_048 * 4];
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&pixels, 2_048, 2_048, image::ExtendedColorType::Rgba8)
            .unwrap();
        assert!(
            png.len() < 128 * 1024,
            "zero image should remain compressed"
        );
        png
    }

    fn duplicate_png_archive(png: &Arc<[u8]>, count: usize) -> LvzArchive {
        LvzArchive::unvalidated_for_usage_test(
            (0..count)
                .map(|index| super::super::LvzSection {
                    filename: Arc::from("duplicate.png"),
                    timestamp: index as u32,
                    data: Arc::clone(png),
                    kind: LvzSectionKind::File,
                })
                .collect(),
        )
    }

    #[test]
    fn multi_archive_decode_work_is_aggregated_before_any_full_decode() {
        let png: Arc<[u8]> = Arc::from(compressed_2048_png());

        crate::render_limits::reset_full_image_validation_calls();
        let mut exact_first = duplicate_png_archive(&png, 8);
        let mut exact_second = duplicate_png_archive(&png, 8);
        let exact_usage = exact_first
            .render_usage()
            .unwrap()
            .checked_add(exact_second.render_usage().unwrap())
            .unwrap();
        assert_eq!(
            exact_usage.image_decode_work_bytes,
            crate::zone_wire::MAX_ZONE_IMAGE_DECODE_WORK_BYTES
        );
        assert_eq!(crate::render_limits::full_image_validation_calls(), 0);
        exact_first.seal_full_renderability().unwrap();
        exact_second.seal_full_renderability().unwrap();
        assert_eq!(crate::render_limits::full_image_validation_calls(), 16);

        crate::render_limits::reset_full_image_validation_calls();
        let over_first = duplicate_png_archive(&png, 8);
        let over_second = duplicate_png_archive(&png, 9);
        assert!(
            over_first
                .render_usage()
                .unwrap()
                .checked_add(over_second.render_usage().unwrap())
                .is_err()
        );
        assert_eq!(
            crate::render_limits::full_image_validation_calls(),
            0,
            "an aggregate max+1 rejection must decode neither archive"
        );
    }

    #[test]
    fn duplicate_compressed_textures_obey_work_budget_validate_once_and_coalesce_last_wins() {
        let png = compressed_2048_png();
        let shared: Arc<[u8]> = Arc::from(png.clone());
        let last: Arc<[u8]> = Arc::from(png);
        let mut sections = (0..15)
            .map(|_| super::super::LvzSection {
                filename: Arc::from("duplicate.png"),
                timestamp: 1,
                data: Arc::clone(&shared),
                kind: LvzSectionKind::File,
            })
            .collect::<Vec<_>>();
        sections.push(super::super::LvzSection {
            filename: Arc::from("duplicate.png"),
            timestamp: 2,
            data: Arc::clone(&last),
            kind: LvzSectionKind::File,
        });
        let mut over_budget = sections.clone();
        over_budget.push(super::super::LvzSection {
            filename: Arc::from("duplicate.png"),
            timestamp: 3,
            data: Arc::clone(&last),
            kind: LvzSectionKind::File,
        });
        crate::render_limits::reset_full_image_validation_calls();
        assert!(
            LvzArchive::unvalidated_for_usage_test(over_budget)
                .render_usage()
                .is_err(),
            "seventeen compressed 2048² duplicates exceed cumulative decode work before full decode"
        );
        assert_eq!(
            crate::render_limits::full_image_validation_calls(),
            0,
            "work overage must reject before any full pixel decode"
        );

        crate::render_limits::reset_full_image_validation_calls();
        let archive = LvzArchive::validated_for_test(sections).unwrap();
        let validation_calls = crate::render_limits::full_image_validation_calls();
        assert_eq!(validation_calls, 16, "every payload validates exactly once");

        archive.require_full_validation_seal().unwrap();
        assert_eq!(
            crate::render_limits::full_image_validation_calls(),
            validation_calls,
            "the renderer's validation seal check must not repeat full CPU decodes"
        );
        let usage = archive.render_usage().unwrap();
        assert_eq!(usage.texture_count, 1);
        assert_eq!(
            usage.pixel_bytes,
            crate::zone_wire::MAX_RENDER_IMAGE_RGBA_BYTES
        );
        assert_eq!(
            usage.image_decode_work_bytes,
            crate::zone_wire::MAX_ZONE_IMAGE_DECODE_WORK_BYTES,
            "sixteen compressed 2048² duplicates are the exact work maximum"
        );
        let coalesced = coalesced_render_image_sections(&archive).unwrap();
        assert_eq!(coalesced.len(), 1);
        assert!(Arc::ptr_eq(
            &coalesced.get("duplicate.png").unwrap().1.data,
            &last
        ));

        let mut near_full = BTreeMap::new();
        for index in 0..16 {
            near_full.insert(
                Arc::<str>::from(format!("old-{index}.png")),
                crate::zone_wire::MAX_RENDER_IMAGE_RGBA_BYTES,
            );
        }
        let before = crate::render_limits::checked_zone_render_totals(
            crate::zone_wire::MAX_ZONE_RENDER_BYTES - crate::zone_wire::MAX_ZONE_RENDER_PIXEL_BYTES,
            0,
            &near_full,
        )
        .unwrap();
        assert_eq!(before.0, crate::zone_wire::MAX_ZONE_RENDER_BYTES);
        assert_eq!(
            crate::render_limits::checked_zone_render_peak(
                before.0,
                before.0,
                crate::zone_wire::MAX_RENDER_IMAGE_RGBA_BYTES,
            )
            .unwrap(),
            crate::zone_wire::MAX_ZONE_RENDER_PEAK_BYTES
        );
    }
}
#[cfg(test)]
mod lvz_scan_cost_tests {
    use super::super::LvzArchive;
    use super::*;
    #[test]
    #[ignore = "reads the shipped zone assets; run with `make bench-lvz-scan`"]
    fn shipped_lvz_object_tables_are_scanned_eight_times_per_frame() {
        // `src/game/render.rs` calls `draw_lvz_map_objects_for_layers` five times
        // and `draw_lvz_screen_objects_for_layers` three times per frame, and each
        // pass walks the whole merged object table. Every object is therefore
        // visited eight times per frame, and in each visit the cost is the object
        // clone, the active/layer/image checks, and the image-filename resolution -
        // not the draw. This measures the table sizes that decide how much that
        // ordering matters, and times the resolution over each zone's real lookup
        // set three ways: the per-visit `lvz_texture_name_for_image_filename`
        // lookup, the merge-time memo, and the slot index that memo now arrives by.
        let zones = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/zones");
        let mut total_objects = 0_usize;
        let mut total_map_objects = 0_usize;
        let mut total_files = 0_usize;
        let mut zone_dirs: Vec<std::path::PathBuf> = std::fs::read_dir(&zones)
            .expect("shipped zones directory")
            .map(|entry| entry.expect("zone entry").path())
            .filter(|path| path.is_dir())
            .collect();
        zone_dirs.sort();
        for zone in zone_dirs {
            let mut zone_objects = 0_usize;
            let mut zone_map_objects = 0_usize;
            let mut zone_files = 0_usize;
            let mut zone_visits: Vec<Arc<str>> = Vec::new();
            // (map object, clamped draw layer, referenced image filename, display mode)
            let mut zone_meta: Vec<(bool, u8, Option<Arc<str>>, u8)> = Vec::new();
            let mut zone_texture_keys: BTreeSet<Arc<str>> = BTreeSet::new();
            let mut stack = vec![zone.clone()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).expect("zone directory") {
                    let path = entry.expect("zone file entry").path();
                    if path.is_dir() {
                        stack.push(path);
                        continue;
                    }
                    if path.extension().and_then(|name| name.to_str()) != Some("lvz") {
                        continue;
                    }
                    let archive = LvzArchive::parse_structural(
                        &std::fs::read(&path).expect("readable shipped lvz"),
                    )
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                    for section in archive.sections() {
                        if matches!(section.kind, LvzSectionKind::File)
                            && crate::render_limits::image_allocation_for_filename(
                                &section.filename,
                                &section.data,
                            )
                            .expect("shipped image allocation check")
                            .is_some()
                        {
                            zone_texture_keys.insert(Arc::clone(&section.filename));
                        }
                    }
                    for section in archive.object_sections() {
                        for object in &section.objects {
                            zone_objects += 1;
                            if object.map_object {
                                zone_map_objects += 1;
                            }
                            let filename = section
                                .images
                                .get(object.image_number as usize)
                                .map(|image| Arc::clone(&image.filename));
                            if let Some(filename) = &filename {
                                zone_visits.push(Arc::clone(filename));
                            }
                            zone_meta.push((
                                object.map_object,
                                crate::lvz::lvz_layer(object.layer),
                                filename,
                                object.display_mode,
                            ));
                        }
                    }
                    zone_files += 1;
                }
            }
            println!(
                "zone {:<14} lvz files {:>3}  objects {:>6}  map objects {:>6}  frame visits {:>7}",
                zone.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("?"),
                zone_files,
                zone_objects,
                zone_map_objects,
                zone_objects * 8,
            );

            // What a frame actually reaches. `src/game/render.rs` runs the map pass
            // for layers 0, 1, 2, 3 and 4..=7 and the screen pass for 0..=4, 5 and
            // 6..=7, so a visit only resolves a filename when the object's class and
            // layer band match the pass; the runtime display-mode check filters this
            // set further, so it is an upper bound on the resolution volume. Timing
            // the every-visit model instead overstates the frame: a real joined Chaos
            // session profiles the whole map pass at well under a millisecond a
            // frame, which is only possible because so few visits get this far.
            const PASSES: [(bool, u8, u8); 8] = [
                (true, 0, 0),
                (true, 1, 1),
                (true, 2, 2),
                (true, 3, 3),
                (true, 4, 7),
                (false, 0, 4),
                (false, 5, 5),
                (false, 6, 7),
            ];
            let mut reachable: Vec<Arc<str>> = Vec::new();
            let mut always_visible = 0_usize;
            let mut per_pass = [0_usize; 8];
            for (is_map, layer, filename, display_mode) in &zone_meta {
                for (pass, &(pass_map, start, end)) in PASSES.iter().enumerate() {
                    if pass_map != *is_map || *layer < start || *layer > end {
                        continue;
                    }
                    // The size of the walk that pass performs each frame: its band.
                    per_pass[pass] += 1;
                    let Some(filename) = filename else {
                        continue;
                    };
                    reachable.push(Arc::clone(filename));
                    always_visible +=
                        usize::from(*display_mode == crate::lvz::LVZ_DISPLAY_SHOW_ALWAYS);
                }
            }
            println!(
                "  band sizes per frame: map {:>5} {:>5} {:>5} {:>5} {:>6}   screen {:>5} {:>5} \
                 {:>5}   walked {:>6} instead of {:>6}",
                per_pass[0],
                per_pass[1],
                per_pass[2],
                per_pass[3],
                per_pass[4],
                per_pass[5],
                per_pass[6],
                per_pass[7],
                per_pass.iter().sum::<usize>(),
                zone_objects * 8,
            );

            let texture_table: BTreeMap<Arc<str>, ()> = zone_texture_keys
                .iter()
                .map(|name| (Arc::clone(name), ()))
                .collect();
            let memo =
                resolve_image_filenames(reachable.iter().map(|name| name.as_ref()), &texture_table);
            let started = std::time::Instant::now();
            let mut resolved_old = 0_usize;
            for filename in &reachable {
                resolved_old += lvz_texture_name_for_image_filename(&texture_table, filename)
                    .is_some() as usize;
            }
            let old_elapsed = started.elapsed();
            let started = std::time::Instant::now();
            let mut resolved_memo = 0_usize;
            for filename in &reachable {
                resolved_memo += memo.get(filename.as_ref()).is_some() as usize;
            }
            let memo_elapsed = started.elapsed();
            // The shape the passes use now: an object arrives holding the memo slot
            // for its image filename, so a visit costs one array index.
            let mut slot_of: BTreeMap<Arc<str>, u32> = BTreeMap::new();
            let mut frames: Vec<(f32, f32)> = Vec::new();
            let mut slots: Vec<u32> = Vec::with_capacity(reachable.len());
            for filename in &reachable {
                let slot = match slot_of.get(&**filename) {
                    Some(slot) => *slot,
                    None if memo.contains_key(filename.as_ref()) => {
                        let slot = frames.len() as u32;
                        frames.push((64.0, 64.0));
                        slot_of.insert(Arc::clone(filename), slot);
                        slot
                    }
                    None => UNRESOLVED_IMAGE_SLOT,
                };
                slots.push(slot);
            }
            let started = std::time::Instant::now();
            let mut resolved_slots = 0_usize;
            for slot in &slots {
                if *slot == UNRESOLVED_IMAGE_SLOT {
                    continue;
                }
                if let Some((width, height)) = frames.get(*slot as usize) {
                    resolved_slots += 1;
                    std::hint::black_box(width + height);
                }
            }
            let slot_elapsed = started.elapsed();
            assert_eq!(
                resolved_old, resolved_memo,
                "the memo must resolve exactly the filenames the per-visit lookup resolves"
            );
            assert_eq!(
                resolved_old, resolved_slots,
                "slot indices must resolve exactly the filenames the per-visit lookup resolves"
            );
            let frame_resolutions = reachable.len();
            println!(
                "  {frame_resolutions:>7} resolutions per frame reach the lookup out of \
                 {:>7} visits ({always_visible:>6} show-always)",
                zone_visits.len() * 8,
            );
            println!(
                "  resolve cost for one frame: normalisation {:>6.1} ns/visit {:>7.3} ms   \
                 memo {:>6.1} ns/visit {:>7.3} ms   slot {:>6.1} ns/visit {:>7.4} ms   \
                 ({} texture keys)",
                old_elapsed.as_nanos() as f64 / frame_resolutions.max(1) as f64,
                old_elapsed.as_secs_f64() * 1e3,
                memo_elapsed.as_nanos() as f64 / frame_resolutions.max(1) as f64,
                memo_elapsed.as_secs_f64() * 1e3,
                slot_elapsed.as_nanos() as f64 / frame_resolutions.max(1) as f64,
                slot_elapsed.as_secs_f64() * 1e3,
                zone_texture_keys.len(),
            );
            total_objects += zone_objects;
            total_map_objects += zone_map_objects;
            total_files += zone_files;
        }
        assert!(total_objects > 0, "shipped zones carry no LVZ objects");
        println!(
            "total: {total_files} lvz files, {total_objects} objects \
             ({total_map_objects} map objects), {} object visits per rendered frame",
            total_objects * 8,
        );
    }
}

#[cfg(test)]
mod lvz_merge_cost_tests {
    use super::super::LvzArchive;
    use super::LvzRenderCache;
    use super::*;
    use std::time::Instant;

    fn shipped_lvz_paths(zone: &str) -> Vec<std::path::PathBuf> {
        let mut paths = Vec::new();
        let mut stack = vec![
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/zones")
                .join(zone),
        ];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("shipped zone directory") {
                let path = entry.expect("zone entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|name| name.to_str()) == Some("lvz") {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        paths
    }

    #[test]
    #[ignore = "reads the shipped zone assets; run with `make bench-lvz-merge`"]
    fn shipped_zone_object_merge_cost_is_timed() {
        // Image sections are dropped because `Texture2D::from_rgba8` needs macroquad's graphics context,
        // which a native test does not have. What remains is the per-object work that dominates the
        // join: `rebuild_resolved_images` indexing every object's image filenames against the merged
        // name set. Both the inline per-archive merge and the deferred-batch budgeted finalize are
        // timed so the join improvement is measured rather than asserted. The budgeted run reports the
        // worst single call, which is the worst frame a join actually pays.
        const BUDGET_OBJECTS: usize = 2_048;
        for zone in ["chaos-zone", "extreme"] {
            let paths = shipped_lvz_paths(zone);
            let archives: Vec<LvzArchive> = paths
                .iter()
                .map(|path| {
                    let parsed =
                        LvzArchive::parse(&std::fs::read(path).expect("readable shipped lvz"))
                            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                    let sections = parsed
                        .sections()
                        .iter()
                        .filter(|section| matches!(section.kind, LvzSectionKind::Objects(_)))
                        .cloned()
                        .collect::<Vec<_>>();
                    LvzArchive::validated_for_test(sections).expect("object-only archive")
                })
                .collect();

            // Inline: one whole-cache resolve/band pass per archive, all in the install frames.
            let mut cache = LvzRenderCache::default();
            let mut inline_worst_ms = 0_u128;
            let inline_started = Instant::now();
            for archive in &archives {
                let started = Instant::now();
                cache
                    .merge_archive(archive)
                    .expect("object-only archive merges into the render cache");
                inline_worst_ms = inline_worst_ms.max(started.elapsed().as_millis());
            }
            let inline_ms = inline_started.elapsed().as_millis();
            let objects = cache.objects.len();

            // Deferred batch: merge every archive with no resolve/band, then finalize across budgeted calls.
            let mut deferred = LvzRenderCache::default();
            let batch_started = Instant::now();
            for archive in &archives {
                deferred
                    .merge_archive_deferred(archive)
                    .expect("object-only archive merges into the render cache");
            }
            let batch_ms = batch_started.elapsed().as_millis();
            let finalize_started = Instant::now();
            let mut finalize_worst_ms = 0_u128;
            let mut finalize_calls = 0usize;
            loop {
                let started = Instant::now();
                let done = deferred.finalize_deferred_merges_budgeted(BUDGET_OBJECTS);
                finalize_worst_ms = finalize_worst_ms.max(started.elapsed().as_millis());
                finalize_calls += 1;
                if done {
                    break;
                }
                assert!(finalize_calls < 100_000, "budgeted finalize must terminate");
            }
            let finalize_ms = finalize_started.elapsed().as_millis();
            assert_eq!(
                deferred.objects.len(),
                objects,
                "deferred and inline merges must hold the same objects"
            );

            println!(
                "OBJECT MERGE zone={zone} files={} objects={} budget_objects={BUDGET_OBJECTS} \
                 inline_ms={inline_ms} inline_worst_ms={inline_worst_ms} deferred_batch_ms={batch_ms} \
                 deferred_finalize_ms={finalize_ms} deferred_finalize_worst_ms={finalize_worst_ms} \
                 deferred_finalize_calls={finalize_calls}",
                paths.len(),
                objects,
            );
        }
    }
}
