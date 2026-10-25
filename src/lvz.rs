use macroquad::prelude::*;
use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::sync::Arc;

mod archive;
mod render;
mod state;

pub(crate) use archive::{
    LvzArchive, LvzImageDefinition, LvzObjectDefinition, LvzRenderUsage, LvzSectionKind,
};
#[cfg(test)]
pub(crate) use archive::{LvzObjectSection, LvzSection};
pub(crate) use render::{
    LVZ_RESOLVE_OBJECTS_PER_FRAME, LvzRenderCache, LvzRenderableObject, LvzTexture,
};
pub(crate) use state::NetworkLvzState;

pub(crate) const LVZ_DISPLAY_SHOW_ALWAYS: u8 = 0;
pub(crate) const LVZ_DISPLAY_ENTER_ZONE: u8 = 1;
pub(crate) const LVZ_DISPLAY_ENTER_ARENA: u8 = 2;
pub(crate) const LVZ_DISPLAY_KILL: u8 = 3;
pub(crate) const LVZ_DISPLAY_DEATH: u8 = 4;
pub(crate) const LVZ_DISPLAY_SERVER_CONTROLLED: u8 = 5;
pub(crate) const LVZ_SCREEN_REFERENCE_POINT_COUNT: u8 = 12;
pub(crate) const LVZ_LAYER_BELOW_ALL: u8 = 0;
pub(crate) const LVZ_LAYER_AFTER_BACKGROUND: u8 = 1;
pub(crate) const LVZ_LAYER_AFTER_TILES: u8 = 2;
pub(crate) const LVZ_LAYER_AFTER_WEAPONS: u8 = 3;
pub(crate) const LVZ_LAYER_AFTER_SHIPS: u8 = 4;
pub(crate) const LVZ_LAYER_AFTER_GAUGES: u8 = 5;
pub(crate) const LVZ_LAYER_AFTER_CHAT: u8 = 6;
pub(crate) const LVZ_LAYER_TOPMOST: u8 = 7;

#[cfg(test)]
pub(crate) fn decode_lvz_raster_image(data: &[u8]) -> Result<Image, String> {
    let (rgba, allocation) = crate::render_limits::decode_raster_rgba_bounded(data)?;
    let width = u16::try_from(allocation.width)
        .map_err(|_| format!("LVZ raster width {} exceeds u16", allocation.width))?;
    let height = u16::try_from(allocation.height)
        .map_err(|_| format!("LVZ raster height {} exceeds u16", allocation.height))?;
    Image::from_rgba8(width, height, rgba.into_raw()).map_err(|error| error.to_string())
}

pub(crate) fn lvz_animation_frame(time: f32, animation_time: i16, frame_count: usize) -> usize {
    let frame_count = frame_count.max(1);
    if frame_count == 1 || animation_time <= 0 {
        return 0;
    }
    let duration = (animation_time as f32 / 100.0).max(0.001);
    ((time.rem_euclid(duration) / duration) * frame_count as f32) as usize % frame_count
}

pub(crate) fn lvz_object_animation_frame(
    time: f32,
    animation_time: i16,
    frame_count: usize,
) -> usize {
    let frame_count = frame_count.max(1);
    if frame_count == 1 || animation_time <= 0 || !time.is_finite() {
        return 0;
    }
    let duration = (animation_time as f32 / 100.0).max(0.001);
    if time <= 0.0 {
        return 0;
    }
    if time >= duration {
        return frame_count - 1;
    }
    ((time / duration) * frame_count as f32) as usize
}

pub(crate) fn lvz_display_time_seconds(display_time: u16) -> f32 {
    display_time as f32 / 10.0
}

pub(crate) fn lvz_source_for_frame(frame_size: Vec2, frame: usize, columns: usize) -> Rect {
    let columns = columns.max(1);
    Rect::new(
        (frame % columns) as f32 * frame_size.x,
        (frame / columns) as f32 * frame_size.y,
        frame_size.x,
        frame_size.y,
    )
}

pub(crate) fn lvz_square_sheet_frame_size(texture_width: f32, columns: usize) -> Vec2 {
    let columns = columns.max(1) as f32;
    let dim = (texture_width / columns).max(1.0);
    vec2(dim, dim)
}

pub(crate) fn lvz_single_ship_frame_size(texture_width: f32, texture_height: f32) -> Vec2 {
    let dim = ((texture_width * texture_height) / 40.0).sqrt().max(1.0);
    vec2(dim, dim)
}

pub(crate) fn lvz_map_object_world_position(x_map: i16, y_map: i16) -> Vec2 {
    vec2(x_map as f32, y_map as f32)
}

pub(crate) fn lvz_layer(layer: u8) -> u8 {
    layer.min(LVZ_LAYER_TOPMOST)
}

pub(crate) fn lvz_layer_in_range(layer: u8, start: u8, end: u8) -> bool {
    let layer = lvz_layer(layer);
    layer >= start && layer <= end
}

pub(crate) fn lvz_screen_reference_point(point: u8) -> Vec2 {
    lvz_screen_reference_point_for_size(point, vec2(screen_width(), screen_height()))
}

pub(crate) fn lvz_screen_reference_point_for_size(point: u8, screen_size: Vec2) -> Vec2 {
    if point >= LVZ_SCREEN_REFERENCE_POINT_COUNT {
        return Vec2::ZERO;
    }

    match point {
        0 => Vec2::ZERO,
        1 => screen_size * 0.5,
        2 => screen_size,
        3 => screen_size,
        4 => vec2(screen_size.x, 0.0),
        5 => screen_size,
        6 => vec2(screen_size.x * 0.5, 36.0),
        7 => vec2(16.0, (screen_size.y - 120.0).max(100.0)),
        8 => {
            let dim = lvz_reference_radar_dim(screen_size.x);
            vec2(screen_size.x - dim - 12.0, 12.0)
        }
        9 => {
            let dim = lvz_reference_radar_dim(screen_size.x);
            vec2(screen_size.x - dim - 12.0, dim + 24.0)
        }
        10 => vec2(screen_size.x - 24.0, screen_size.y * 0.57 - 99.0),
        11 => vec2(0.0, screen_size.y * 0.57 + 76.0),
        _ => Vec2::ZERO,
    }
}

fn lvz_reference_radar_dim(screen_width: f32) -> f32 {
    let surface_width = screen_width.max(1.0) as u32;
    ((((surface_width / 6) / 4) * 8) / 2).max(32) as f32
}

pub(crate) fn normalize_asset_name(filename: &str) -> String {
    filename.replace('\\', "/").to_ascii_lowercase()
}

impl LvzArchive {
    pub(crate) fn object_image_filenames(&self) -> std::collections::BTreeSet<Arc<str>> {
        self.object_sections()
            .flat_map(|objects| objects.images.iter())
            .map(|image| Arc::clone(&image.filename))
            .collect()
    }
}

impl LvzObjectDefinition {
    pub(crate) fn modified(&self, modification: Option<&crate::LvzModifyPacket>) -> Self {
        let Some(modification) = modification else {
            return self.clone();
        };
        if modification.object_id != self.object_id {
            return self.clone();
        }

        let mut object = self.clone();
        object.map_object = modification.map_object;
        if modification.changed.mode {
            object.display_mode = modification.display_mode;
        }
        if modification.changed.xy {
            if modification.map_object {
                object.x_map = modification.map_x;
                object.y_map = modification.map_y;
            } else {
                object.x_screen = modification.screen_x;
                object.y_screen = modification.screen_y;
                object.x_type = modification.x_type;
                object.y_type = modification.y_type;
            }
        }
        if modification.changed.image {
            object.image_number = modification.image_id;
        }
        if modification.changed.layer {
            object.layer = modification.layer;
        }
        if modification.changed.time {
            object.display_time = modification.display_time;
        }

        object
    }
}

pub(crate) fn lvz_asset_stem_matches(name: &str, expected_stem: &str) -> bool {
    let basename = name.rsplit('/').next().unwrap_or(name);
    basename
        .rsplit_once('.')
        .map(|(stem, _)| stem == expected_stem)
        .unwrap_or(false)
}

pub(crate) fn lvz_texture_name_for_image_filename<'a, K, T>(
    textures: &'a BTreeMap<K, T>,
    filename: &str,
) -> Option<&'a str>
where
    K: Ord + Borrow<str>,
{
    let normalized = normalize_asset_name(filename);
    if let Some((name, _)) = textures.get_key_value(normalized.as_str()) {
        return Some(name.borrow());
    }

    let expected_stem = normalized.rsplit('/').next()?.rsplit_once('.')?.0;
    let mut matches = textures
        .keys()
        .filter(|name| lvz_asset_stem_matches((*name).borrow(), expected_stem));
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first.borrow())
}

pub(crate) fn lvz_graphic_override_stem(filename: &str) -> Option<&str> {
    let basename = filename.rsplit('/').next().unwrap_or(filename);
    let (stem, extension) = basename.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "bm2" | "bmp" | "png" | "gif" | "jpg" | "jpeg"
    )
    .then_some(stem)
}

pub(crate) fn lvz_graphic_override_stem_for_file<'a, K>(
    normalized_name: &'a str,
    object_image_filenames: &std::collections::BTreeSet<K>,
) -> Option<&'a str>
where
    K: Ord + Borrow<str>,
{
    if object_image_filenames.contains(normalized_name) {
        return None;
    }
    lvz_graphic_override_stem(normalized_name)
}
