use super::*;

mod prize_notifications;
mod prizes;

pub(crate) use prize_notifications::{PrizeNotification, prize_notification_text_for_entry};

pub(in crate::game) use prizes::AppliedCollectibleAckHistory;

#[cfg(test)]
use prize_notifications::*;
#[cfg(test)]
use prizes::apply_multifire_prize;

/// Page clock for LVZ merge accounting. The browser measures with the same clock
/// the render path already reads; a native test has no miniquad context, and
/// `macroquad::get_time` asserts on a thread without one.
fn lvz_merge_clock() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        macroquad::prelude::get_time()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs_f64())
            .unwrap_or_default()
    }
}

impl Game {
    pub(super) fn apply_network_brick_dropped(&mut self, brick: BrickDropped) {
        match brick {
            BrickDropped::Clear => self.clear_bricks(),
            BrickDropped::Drop(drop) => {
                if !self.networked_play || self.network_zone_map_ready {
                    self.install_server_brick_line(drop);
                }
            }
        }
    }

    pub(super) fn update_notifications(&mut self, dt: f32) {
        update_notification_entries(&mut self.notifications, dt);
    }

    pub(super) fn push_notification(&mut self, message: &str, color: Color) {
        push_notification_entry(
            &mut self.notifications,
            message.to_string(),
            color,
            NOTIFICATION_LIMIT,
            NOTIFICATION_LIFETIME,
        );
    }

    pub(super) fn install_downloaded_file(&mut self, download: &ValidatedDownloadedFile) {
        let filename = download.filename();
        let data = download.data();
        match download.take_parsed() {
            Some(DownloadedFile::Level(map)) => {
                if map.retained_render_bytes().is_err() {
                    self.push_notification("Arena map representation budget exceeded.", RED);
                    return;
                }
                self.clear_bricks();
                // Network LVZ archives belong to the downloaded arena. Drop
                // the previous arena's decoded data before accepting this
                // arena so repeated zone changes cannot accumulate it.
                self.lvz_archives.clear();
                self.lvz_archive_order.clear();
                self.clear_dynamic_lvz_chat_sounds();
                let tileset = match texture_from_lvl_embedded_tileset(data) {
                    Ok(tileset) => tileset,
                    Err(_) => {
                        self.push_notification("Arena tileset decode failed.", RED);
                        return;
                    }
                };
                let index = replace_current_downloaded_map(&mut self.maps, self.map_index, map);
                while self.map_tilesets.len() <= index {
                    self.map_tilesets.push(None);
                }
                self.map_tilesets[index] = tileset;
                self.switch_to_map(index);
                if crate::browser_metrics::debug_mode() {
                    self.push_notification("Downloaded arena map.", GREEN);
                }
            }
            Some(DownloadedFile::Lvz(archive)) => {
                let candidate_order =
                    archive_order_with_replacement(&self.lvz_archive_order, filename);
                let map_render_bytes = match self.maps[self.map_index].retained_render_bytes() {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        self.push_notification("Arena map representation budget exceeded.", RED);
                        return;
                    }
                };
                let render_total = match admitted_zone_render_total_with_replacement(
                    &self.lvz_archives,
                    &candidate_order,
                    filename,
                    &archive,
                    map_render_bytes,
                    self.maps[self.map_index].embedded_tileset_pixel_bytes,
                ) {
                    Ok(total) => total,
                    Err(()) => {
                        self.push_notification("Zone render budget exceeded.", RED);
                        return;
                    }
                };
                debug_assert!(render_total <= crate::zone_wire::MAX_ZONE_RENDER_BYTES);
                let dynamic_sound_sources = match dynamic_lvz_chat_sound_sources(
                    &self.lvz_archives,
                    &candidate_order,
                    Some((filename, &archive)),
                ) {
                    Ok(sources) => sources,
                    Err(_) => {
                        self.push_notification("LVZ chat audio validation failed.", RED);
                        return;
                    }
                };
                let merge_started = get_time();
                if self.lvz_archives.contains_key(filename) {
                    let mut replacement_cache = LvzRenderCache::default();
                    let mut merge_failed = false;
                    for archive_name in &candidate_order {
                        let candidate = if archive_name == filename {
                            &archive
                        } else {
                            self.lvz_archives
                                .get(archive_name)
                                .expect("candidate LVZ order must reference keyed storage")
                        };
                        if replacement_cache.merge_archive_deferred(candidate).is_err() {
                            merge_failed = true;
                            break;
                        }
                    }
                    // One whole-cache resolve/band pass for the whole replacement set, not one per
                    // archive: banding walks every object and a replacement re-merges the full order.
                    replacement_cache.finalize_deferred_merges();
                    self.account_lvz_merge(merge_started);
                    if merge_failed {
                        self.push_notification("LVZ render validation failed.", RED);
                        return;
                    }
                    self.lvz_render_cache = replacement_cache;
                } else {
                    // A zone download installs its archives one per poll; defer the whole-cache
                    // resolve/band pass to `ZoneFilesPublished` so a fourteen-archive join pays one
                    // band pass. A standalone install (no open batch) finalizes inline.
                    let merged = if self.zone_files_batch_open {
                        self.lvz_render_cache.merge_archive_deferred(&archive)
                    } else {
                        self.lvz_render_cache.merge_archive(&archive)
                    };
                    self.account_lvz_merge(merge_started);
                    if merged.is_err() {
                        self.push_notification("LVZ render validation failed.", RED);
                        return;
                    }
                }
                self.lvz_archives.insert(filename.to_string(), archive);
                self.lvz_archive_order = candidate_order;
                self.install_dynamic_lvz_chat_sounds(dynamic_sound_sources);
                if crate::browser_metrics::debug_mode() {
                    self.push_notification("Downloaded LVZ archive.", GREEN);
                }
            }
            None => {
                self.push_notification("Validated download was already consumed.", RED);
            }
        }
    }

    /// Record the wall cost one downloaded-LVZ install just charged the render
    /// path. The counters are cumulative for the page's life so a browser gate
    /// reads a window's cost by diffing a before/after pair, the same way the
    /// shared frame witness does; a replacement install is accounted as one
    /// merge because that is the single task the frame actually paid.
    pub(super) fn account_lvz_merge(&mut self, started_seconds: f64) {
        let elapsed_ms = ((lvz_merge_clock() - started_seconds) * 1_000.0).max(0.0) as f32;
        self.lvz_merge_count = self.lvz_merge_count.saturating_add(1);
        self.lvz_merge_total_ms += elapsed_ms;
        self.lvz_merge_max_ms = self.lvz_merge_max_ms.max(elapsed_ms);
    }

    /// Spend one bounded chunk of the postponed LVZ image resolve, called once per frame. When the
    /// last chunk completes it runs the one spatial band pass and marks zone files ready, so gameplay
    /// opens only on a fully resolved zone without a single long main-thread task. Inert unless a
    /// `ZoneFilesPublished` batch opened the pending resolve.
    pub(crate) fn advance_deferred_lvz_merge(&mut self) {
        if !self.zone_files_resolve_pending {
            return;
        }
        // ~2k objects is a few milliseconds: one Chaos LVZ carries ~27k of the zone's 28.9k objects, so
        // the unbudgeted per-object image resolve was a ~54 ms task inside one install frame.
        if self
            .lvz_render_cache
            .finalize_deferred_merges_budgeted(crate::lvz::LVZ_RESOLVE_OBJECTS_PER_FRAME)
        {
            self.zone_files_resolve_pending = false;
            self.network_zone_files_ready = true;
            self.client_authority.observe_files_ready();
        }
    }

    fn clear_dynamic_lvz_chat_sounds(&mut self) {
        self.dynamic_lvz_chat_sound_generation =
            self.dynamic_lvz_chat_sound_generation.wrapping_add(1);
        self.dynamic_lvz_chat_sound_sources.clear();
        self.dynamic_lvz_chat_sound_loaded_ids.clear();
        #[cfg(target_arch = "wasm32")]
        self.dynamic_lvz_sound_loads.clear();
    }

    fn install_dynamic_lvz_chat_sounds(&mut self, sources: BTreeMap<u8, Arc<[u8]>>) {
        let Some(generation) = self.replace_dynamic_lvz_chat_sound_sources(sources) else {
            return;
        };
        self.lvz_chat_sounds.clear();
        merge_local_zone_lvz_sounds(
            &mut self.lvz_chat_sounds,
            &self.assets.lvz_chat_sounds,
            &self.maps[self.map_index].name,
        );
        #[cfg(target_arch = "wasm32")]
        {
            for (sound_id, data) in &self.dynamic_lvz_chat_sound_sources {
                let data = Arc::clone(data);
                let sound_id = *sound_id;
                self.dynamic_lvz_sound_loads.push(DynamicLvzSoundLoad {
                    generation,
                    sound_id,
                    load: start_coroutine(async move {
                        load_sound_from_bytes(data.as_ref()).await.ok()
                    }),
                });
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = generation;
    }

    fn replace_dynamic_lvz_chat_sound_sources(
        &mut self,
        sources: BTreeMap<u8, Arc<[u8]>>,
    ) -> Option<u64> {
        // Most archives contain graphics only. Their admission cannot cancel
        // pending sound decodes, clear loaded audio, or restart every sound.
        // Compare content, not archive/Arc identity; a real replacement or
        // removal still retires the complete old generation before loading.
        if self.dynamic_lvz_chat_sound_sources == sources {
            return None;
        }
        self.dynamic_lvz_chat_sound_generation =
            self.dynamic_lvz_chat_sound_generation.wrapping_add(1);
        self.dynamic_lvz_chat_sound_sources = sources;
        self.dynamic_lvz_chat_sound_loaded_ids.clear();
        #[cfg(target_arch = "wasm32")]
        self.dynamic_lvz_sound_loads.clear();
        Some(self.dynamic_lvz_chat_sound_generation)
    }
}

fn archive_order_with_replacement(existing: &[String], filename: &str) -> Vec<String> {
    let mut order = existing
        .iter()
        .filter(|existing| existing.as_str() != filename)
        .cloned()
        .collect::<Vec<_>>();
    order.push(filename.to_string());
    order
}

fn dynamic_lvz_chat_sound_sources(
    archives: &BTreeMap<String, LvzArchive>,
    archive_order: &[String],
    replacement: Option<(&str, &LvzArchive)>,
) -> Result<BTreeMap<u8, Arc<[u8]>>, String> {
    let mut sources = BTreeMap::new();
    let mut pcm_bytes = 0_usize;
    for archive_name in archive_order {
        let archive = replacement
            .filter(|(filename, _)| *filename == archive_name)
            .map(|(_, archive)| archive)
            .or_else(|| archives.get(archive_name))
            .ok_or_else(|| format!("LVZ order references missing archive {archive_name:?}"))?;
        for section in archive.sections() {
            let Some(sound_id) = crate::render_limits::lvz_chat_sound_id(&section.filename) else {
                continue;
            };
            let Some(decoded_bytes) = crate::render_limits::validate_chat_sound_for_filename(
                &section.filename,
                &section.data,
            )?
            else {
                continue;
            };
            if let Some(previous) = sources.insert(sound_id, Arc::clone(&section.data)) {
                let previous_bytes = crate::render_limits::validate_chat_sound_for_filename(
                    &format!("bong{sound_id}.wa2"),
                    &previous,
                )?
                .unwrap_or(0);
                pcm_bytes = pcm_bytes.saturating_sub(previous_bytes);
            }
            pcm_bytes = pcm_bytes
                .checked_add(decoded_bytes)
                .ok_or_else(|| "dynamic LVZ chat sound PCM size overflow".to_string())?;
            if pcm_bytes > crate::zone_wire::MAX_LVZ_CHAT_SOUND_PCM_BYTES {
                return Err(format!(
                    "dynamic LVZ chat sounds expand to {pcm_bytes} bytes; maximum is {}",
                    crate::zone_wire::MAX_LVZ_CHAT_SOUND_PCM_BYTES
                ));
            }
        }
    }
    Ok(sources)
}

pub(in crate::game) fn replace_current_downloaded_map(
    maps: &mut Vec<ArenaMap>,
    current: usize,
    replacement: ArenaMap,
) -> usize {
    let index = maps
        .iter()
        .position(|existing| existing.name == replacement.name)
        .unwrap_or_else(|| current.min(maps.len().saturating_sub(1)));
    if maps.is_empty() {
        maps.push(replacement);
        0
    } else {
        maps[index] = replacement;
        index
    }
}

fn admitted_zone_render_total_with_replacement(
    archives: &BTreeMap<String, LvzArchive>,
    archive_order: &[String],
    filename: &str,
    replacement: &LvzArchive,
    map_render_bytes: usize,
    map_pixel_bytes: usize,
) -> Result<usize, ()> {
    let mut usage = crate::lvz::LvzRenderUsage::default();
    let mut images = BTreeMap::new();
    for archive_name in archive_order {
        let archive = if archive_name == filename {
            replacement
        } else {
            archives.get(archive_name).ok_or(())?
        };
        let mut existing_usage = archive.render_usage().map_err(|_| ())?;
        existing_usage.pixel_bytes = 0;
        existing_usage.texture_count = 0;
        usage = usage.checked_add(existing_usage).map_err(|_| ())?;
        for (name, bytes) in archive.render_image_allocations().map_err(|_| ())? {
            images.insert(name, bytes);
        }
    }
    usage.validate().map_err(|_| ())?;
    let nonpixel_bytes = map_render_bytes
        .checked_sub(map_pixel_bytes)
        .and_then(|bytes| bytes.checked_add(usage.retained_bytes().ok()?))
        .ok_or(())?;
    crate::render_limits::checked_zone_render_totals(nonpixel_bytes, map_pixel_bytes, &images)
        .map(|totals| totals.0)
        .map_err(|_| ())
}

fn texture_from_lvl_embedded_tileset(data: &[u8]) -> Result<Option<Texture2D>, String> {
    let Some(bitmap) = lvl_embedded_bitmap(data)? else {
        return Ok(None);
    };
    let texture = try_texture_from_bm2(bitmap, Transparency::Opaque)?;
    texture.set_filter(FilterMode::Nearest);
    Ok(Some(texture))
}

#[cfg(test)]
mod tests;
