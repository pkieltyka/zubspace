use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use image::{DynamicImage, ImageDecoder, ImageReader, Limits, RgbaImage};

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    // Validation is synchronous. A thread-local probe keeps the exact-pass
    // regression isolated from the rest of Rust's parallel test runner.
    static FULL_IMAGE_VALIDATION_CALLS: Cell<usize> = const { Cell::new(0) };
}

use crate::zone_wire::{
    MAP_DOOR_RETAINED_BYTES, MAP_OBJECT_RETAINED_BYTES, MAP_RADAR_POINT_RETAINED_BYTES,
    MAX_IMAGE_DECODER_ALLOC_BYTES, MAX_LVZ_CHAT_SOUND_PCM_BYTES_PER_FILE,
    MAX_MAP_REPRESENTATION_BYTES, MAX_MAP_TILE_RECORDS, MAX_RENDER_IMAGE_DIMENSION,
    MAX_RENDER_IMAGE_PIXELS, MAX_RENDER_IMAGE_RGBA_BYTES, MAX_ZONE_RENDER_BYTES,
    MAX_ZONE_RENDER_PEAK_BYTES, MAX_ZONE_RENDER_PIXEL_BYTES,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImageAllocation {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rgba_bytes: usize,
}

/// Shared by the complete admitted zone, never renewed per archive. Prepared
/// pixels are optional: this reserves space in addition to the largest retained
/// manifest prefix, a full decoder workspace, and sequential CPU/GPU upload.
/// Exhaustion falls back to validation followed by a later bounded decode.
#[derive(Default)]
pub(crate) struct ImagePreparationBudget {
    remaining: usize,
}

impl ImagePreparationBudget {
    #[allow(dead_code)] // server validates sequentially without preparing client upload buffers
    pub(crate) fn for_zone_high_water(retained_bytes: usize) -> Self {
        let decoder_and_upload =
            MAX_IMAGE_DECODER_ALLOC_BYTES as usize + 2 * MAX_RENDER_IMAGE_RGBA_BYTES;
        Self {
            remaining: MAX_ZONE_RENDER_PEAK_BYTES
                .saturating_sub(retained_bytes)
                .saturating_sub(decoder_and_upload),
        }
    }

    pub(crate) fn available(&self) -> bool {
        self.remaining != 0
    }

    pub(crate) fn try_reserve(&mut self, bytes: usize) -> bool {
        let Some(remaining) = self.remaining.checked_sub(bytes) else {
            return false;
        };
        self.remaining = remaining;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LvlSourceLayout {
    pub(crate) tile_offset: usize,
    pub(crate) record_count: usize,
    pub(crate) animated_record_count: usize,
    pub(crate) door_record_count: usize,
    pub(crate) embedded_pixel_bytes: usize,
}

pub(crate) fn checked_image_allocation(
    width: usize,
    height: usize,
) -> Result<ImageAllocation, String> {
    if width == 0 || height == 0 {
        return Err("render image dimensions must be nonzero".to_string());
    }
    if width > MAX_RENDER_IMAGE_DIMENSION || height > MAX_RENDER_IMAGE_DIMENSION {
        return Err(format!(
            "render image dimensions {width}x{height} exceed {} per axis",
            MAX_RENDER_IMAGE_DIMENSION
        ));
    }
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| "render image pixel product overflow".to_string())?;
    if pixels > MAX_RENDER_IMAGE_PIXELS {
        return Err(format!(
            "render image has {pixels} pixels; maximum is {MAX_RENDER_IMAGE_PIXELS}"
        ));
    }
    let rgba_bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| "render RGBA byte count overflow".to_string())?;
    if rgba_bytes > MAX_RENDER_IMAGE_RGBA_BYTES {
        return Err(format!(
            "render image expands to {rgba_bytes} RGBA bytes; maximum is {MAX_RENDER_IMAGE_RGBA_BYTES}"
        ));
    }
    Ok(ImageAllocation {
        width,
        height,
        rgba_bytes,
    })
}

pub(crate) fn validate_bm2_header(data: &[u8]) -> Result<ImageAllocation, String> {
    if data.len() < 54 || &data[..2] != b"BM" {
        return Err("not a BMP/BM2 file".to_string());
    }
    let i32_at = |offset: usize| {
        i32::from_le_bytes(
            data[offset..offset + 4]
                .try_into()
                .expect("checked BM2 range"),
        )
    };
    let u16_at = |offset: usize| {
        u16::from_le_bytes(
            data[offset..offset + 2]
                .try_into()
                .expect("checked BM2 range"),
        )
    };
    let u32_at = |offset: usize| {
        u32::from_le_bytes(
            data[offset..offset + 4]
                .try_into()
                .expect("checked BM2 range"),
        )
    };

    let width = i32_at(18);
    let height = i32_at(22);
    let planes = u16_at(26);
    let bpp = u16_at(28);
    let compression = u32_at(30);
    if planes != 1
        || !((bpp == 8 && (compression == 0 || compression == 1))
            || (bpp == 24 && compression == 0)
            || (bpp == 32 && compression == 0))
    {
        return Err(format!(
            "unsupported BM2 format: planes={planes}, bpp={bpp}, compression={compression}"
        ));
    }
    checked_image_allocation(
        width.unsigned_abs() as usize,
        height.unsigned_abs() as usize,
    )
}

fn bounded_decoder(data: &[u8]) -> Result<impl ImageDecoder + '_, String> {
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|err| format!("raster format detection failed: {err}"))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_RENDER_IMAGE_DIMENSION as u32);
    limits.max_image_height = Some(MAX_RENDER_IMAGE_DIMENSION as u32);
    limits.max_alloc = Some(MAX_IMAGE_DECODER_ALLOC_BYTES);
    // PNG consumes limits during decoder construction; other formats also
    // receive them through ImageReader::into_decoder.
    reader.limits(limits);
    reader
        .into_decoder()
        .map_err(|err| format!("raster header/limit validation failed: {err}"))
}

pub(crate) fn validate_raster_header(data: &[u8]) -> Result<ImageAllocation, String> {
    let decoder = bounded_decoder(data)?;
    let (width, height) = decoder.dimensions();
    checked_image_allocation(width as usize, height as usize)
}

#[allow(dead_code)]
pub(crate) fn decode_raster_rgba_bounded(
    data: &[u8],
) -> Result<(RgbaImage, ImageAllocation), String> {
    let decoder = bounded_decoder(data)?;
    let (width, height) = decoder.dimensions();
    let allocation = checked_image_allocation(width as usize, height as usize)?;
    let rgba = DynamicImage::from_decoder(decoder)
        .map_err(|err| format!("raster decode failed: {err}"))?
        .into_rgba8();
    if rgba.width() as usize != allocation.width
        || rgba.height() as usize != allocation.height
        || rgba.as_raw().len() != allocation.rgba_bytes
    {
        return Err("raster decoder returned an inconsistent RGBA allocation".to_string());
    }
    Ok((rgba, allocation))
}

pub(crate) fn image_allocation_for_filename(
    filename: &str,
    data: &[u8],
) -> Result<Option<ImageAllocation>, String> {
    let extension = filename.rsplit_once('.').map(|(_, extension)| extension);
    if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("bm2")) {
        return validate_bm2_header(data).map(Some);
    }
    if extension.is_some_and(|extension| {
        ["bmp", "png", "gif", "jpg", "jpeg"]
            .iter()
            .any(|expected| extension.eq_ignore_ascii_case(expected))
    }) {
        return validate_raster_header(data).map(Some);
    }
    Ok(None)
}

#[cfg(test)]
pub(crate) fn fully_decode_image_for_filename(
    filename: &str,
    data: &[u8],
) -> Result<Option<ImageAllocation>, String> {
    Ok(
        decode_image_for_filename(filename, data)?.map(|decoded| ImageAllocation {
            width: decoded.width,
            height: decoded.height,
            rgba_bytes: decoded.rgba.len(),
        }),
    )
}

pub(crate) fn decode_image_for_filename(
    filename: &str,
    data: &[u8],
) -> Result<Option<crate::bm2::DecodedImage>, String> {
    let extension = filename.rsplit_once('.').map(|(_, extension)| extension);
    if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("bm2")) {
        #[cfg(test)]
        FULL_IMAGE_VALIDATION_CALLS.with(|calls| calls.set(calls.get().saturating_add(1)));
        let decoded = crate::bm2::decode_bm2(data, crate::bm2::Transparency::Black)?;
        let allocation = checked_image_allocation(decoded.width, decoded.height)?;
        if decoded.rgba.len() != allocation.rgba_bytes {
            return Err("BM2 decoder returned an inconsistent RGBA allocation".to_string());
        }
        return Ok(Some(decoded));
    }
    if extension.is_some_and(|extension| {
        ["bmp", "png", "gif", "jpg", "jpeg"]
            .iter()
            .any(|expected| extension.eq_ignore_ascii_case(expected))
    }) {
        #[cfg(test)]
        FULL_IMAGE_VALIDATION_CALLS.with(|calls| calls.set(calls.get().saturating_add(1)));
        let (rgba, allocation) = decode_raster_rgba_bounded(data)?;
        return Ok(Some(crate::bm2::DecodedImage {
            width: allocation.width,
            height: allocation.height,
            rgba: rgba.into_raw(),
        }));
    }
    Ok(None)
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn reset_full_image_validation_calls() {
    FULL_IMAGE_VALIDATION_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn full_image_validation_calls() -> usize {
    FULL_IMAGE_VALIDATION_CALLS.with(Cell::get)
}

pub(crate) fn lvz_chat_sound_id(filename: &str) -> Option<u8> {
    let basename = filename.rsplit('/').next().unwrap_or(filename);
    let (stem, extension) = basename.rsplit_once('.')?;
    if !(extension.eq_ignore_ascii_case("wa2") || extension.eq_ignore_ascii_case("wav")) {
        return None;
    }
    let id = stem.strip_prefix("bong")?.parse::<u16>().ok()?;
    u8::try_from(id).ok().filter(|id| *id != 0)
}

pub(crate) fn validate_chat_sound_for_filename(
    filename: &str,
    data: &[u8],
) -> Result<Option<usize>, String> {
    if lvz_chat_sound_id(filename).is_none() {
        return Ok(None);
    }
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(format!(
            "LVZ chat sound {filename:?} is not a RIFF/WAVE file"
        ));
    }
    let declared = u32::from_le_bytes(data[4..8].try_into().expect("RIFF header")) as usize;
    if declared.checked_add(8) != Some(data.len()) {
        return Err(format!(
            "LVZ chat sound {filename:?} has an inconsistent RIFF size"
        ));
    }
    let mut offset = 12_usize;
    let mut format = None;
    let mut data_bytes = 0_usize;
    while offset < data.len() {
        if offset.checked_add(8).is_none_or(|end| end > data.len()) {
            return Err(format!(
                "LVZ chat sound {filename:?} has a truncated chunk header"
            ));
        }
        let id = &data[offset..offset + 4];
        let len = u32::from_le_bytes(
            data[offset + 4..offset + 8]
                .try_into()
                .expect("WAVE chunk size"),
        ) as usize;
        let start = offset + 8;
        let end = start
            .checked_add(len)
            .ok_or_else(|| format!("LVZ chat sound {filename:?} chunk size overflow"))?;
        if end > data.len() {
            return Err(format!("LVZ chat sound {filename:?} has a truncated chunk"));
        }
        if id == b"fmt " {
            if len < 16 || format.is_some() {
                return Err(format!(
                    "LVZ chat sound {filename:?} has an invalid fmt chunk"
                ));
            }
            let tag = u16::from_le_bytes(data[start..start + 2].try_into().unwrap());
            let channels = u16::from_le_bytes(data[start + 2..start + 4].try_into().unwrap());
            let sample_rate = u32::from_le_bytes(data[start + 4..start + 8].try_into().unwrap());
            let byte_rate = u32::from_le_bytes(data[start + 8..start + 12].try_into().unwrap());
            let block_align = u16::from_le_bytes(data[start + 12..start + 14].try_into().unwrap());
            let bits = u16::from_le_bytes(data[start + 14..start + 16].try_into().unwrap());
            if tag != 1
                || !(1..=2).contains(&channels)
                || sample_rate == 0
                || sample_rate > 192_000
                || !matches!(bits, 8 | 16 | 24 | 32)
                || block_align as u32 != channels as u32 * (bits as u32 / 8)
                || byte_rate != sample_rate.saturating_mul(block_align as u32)
            {
                return Err(format!(
                    "LVZ chat sound {filename:?} has unsupported PCM parameters"
                ));
            }
            format = Some((channels as usize, block_align as usize));
        } else if id == b"data" {
            data_bytes = data_bytes
                .checked_add(len)
                .ok_or_else(|| format!("LVZ chat sound {filename:?} data size overflow"))?;
        }
        offset = end
            .checked_add(len & 1)
            .ok_or_else(|| format!("LVZ chat sound {filename:?} chunk padding overflow"))?;
        if offset > data.len() {
            return Err(format!(
                "LVZ chat sound {filename:?} has truncated chunk padding"
            ));
        }
    }
    let Some((channels, block_align)) = format else {
        return Err(format!(
            "LVZ chat sound {filename:?} is missing a fmt chunk"
        ));
    };
    if data_bytes == 0 || !data_bytes.is_multiple_of(block_align) {
        return Err(format!(
            "LVZ chat sound {filename:?} has invalid PCM data alignment"
        ));
    }
    let pcm_bytes = (data_bytes / block_align)
        .checked_mul(channels)
        .and_then(|samples| samples.checked_mul(4))
        .ok_or_else(|| format!("LVZ chat sound {filename:?} decoded size overflow"))?;
    if pcm_bytes > MAX_LVZ_CHAT_SOUND_PCM_BYTES_PER_FILE {
        return Err(format!(
            "LVZ chat sound {filename:?} expands to {pcm_bytes} bytes; maximum is {MAX_LVZ_CHAT_SOUND_PCM_BYTES_PER_FILE}"
        ));
    }
    Ok(Some(pcm_bytes))
}

pub(crate) fn validate_lvl_source(data: &[u8]) -> Result<LvlSourceLayout, String> {
    let tile_offset = if data.len() >= 6 && &data[..2] == b"BM" {
        u32::from_le_bytes(data[2..6].try_into().expect("checked LVL header")) as usize
    } else {
        0
    };
    if tile_offset > data.len() {
        return Err("LVL tile offset points past end of file".to_string());
    }
    let embedded_pixel_bytes = if tile_offset == 0 {
        0
    } else {
        let decoded =
            crate::bm2::decode_bm2(&data[..tile_offset], crate::bm2::Transparency::Opaque)?;
        checked_image_allocation(decoded.width, decoded.height)?.rgba_bytes
    };
    let tile_bytes = &data[tile_offset..];
    if !tile_bytes.len().is_multiple_of(4) {
        return Err("LVL tile data is not aligned to 4-byte tile records".to_string());
    }
    let record_count = tile_bytes.len() / 4;
    if record_count > MAX_MAP_TILE_RECORDS {
        return Err(format!(
            "LVL declares {record_count} tile records; maximum is {MAX_MAP_TILE_RECORDS}"
        ));
    }
    let mut animated_record_count = 0_usize;
    let mut door_record_count = 0_usize;
    for record in tile_bytes.chunks_exact(4) {
        let packed = u32::from_le_bytes(record.try_into().expect("exact LVL record"));
        let x = (packed & 0x0fff) as usize;
        let y = ((packed >> 12) & 0x0fff) as usize;
        if x >= 1024 || y >= 1024 {
            return Err(format!("LVL tile coordinate out of bounds: {x},{y}"));
        }
        let id = (packed >> 24) as u8;
        if matches!(id, 170 | 172 | 216 | 217 | 218 | 219 | 220) {
            animated_record_count += 1;
        }
        if (162..=169).contains(&id) {
            door_record_count += 1;
        }
    }
    Ok(LvlSourceLayout {
        tile_offset,
        record_count,
        animated_record_count,
        door_record_count,
        embedded_pixel_bytes,
    })
}

pub(crate) fn checked_map_representation_bytes(
    name_bytes: usize,
    tile_bytes: usize,
    object_count: usize,
    door_count: usize,
    retained_radar_slots: usize,
    embedded_pixel_bytes: usize,
) -> Result<usize, String> {
    let total = 256_usize
        .checked_add(name_bytes)
        .and_then(|value| value.checked_add(tile_bytes))
        .and_then(|value| {
            object_count
                .checked_mul(MAP_OBJECT_RETAINED_BYTES)
                .and_then(|bytes| value.checked_add(bytes))
        })
        .and_then(|value| {
            door_count
                .checked_mul(MAP_DOOR_RETAINED_BYTES)
                .and_then(|bytes| value.checked_add(bytes))
        })
        .and_then(|value| {
            retained_radar_slots
                .checked_mul(MAP_RADAR_POINT_RETAINED_BYTES)
                .and_then(|bytes| value.checked_add(bytes))
        })
        .and_then(|value| value.checked_add(embedded_pixel_bytes))
        .ok_or_else(|| "LVL retained representation size overflow".to_string())?;
    if total > MAX_MAP_REPRESENTATION_BYTES {
        return Err(format!(
            "LVL retained representation is {total} bytes; maximum is {MAX_MAP_REPRESENTATION_BYTES}"
        ));
    }
    Ok(total)
}

pub(crate) fn checked_zone_render_totals<K: Ord>(
    nonpixel_bytes: usize,
    map_pixel_bytes: usize,
    render_images: &BTreeMap<K, usize>,
) -> Result<(usize, usize), String> {
    if render_images.len() > crate::zone_wire::MAX_LVZ_TEXTURES_PER_ZONE {
        return Err(format!(
            "zone retains {} render textures; maximum is {}",
            render_images.len(),
            crate::zone_wire::MAX_LVZ_TEXTURES_PER_ZONE
        ));
    }
    let image_bytes = render_images
        .values()
        .try_fold(0_usize, |total, bytes| total.checked_add(*bytes))
        .ok_or_else(|| "zone render image size overflow".to_string())?;
    let pixel_bytes = map_pixel_bytes
        .checked_add(image_bytes)
        .ok_or_else(|| "zone render pixel size overflow".to_string())?;
    if pixel_bytes > MAX_ZONE_RENDER_PIXEL_BYTES {
        return Err(format!(
            "zone images expand to {pixel_bytes} RGBA bytes; maximum is {MAX_ZONE_RENDER_PIXEL_BYTES}"
        ));
    }
    let retained_bytes = nonpixel_bytes
        .checked_add(pixel_bytes)
        .ok_or_else(|| "zone retained render size overflow".to_string())?;
    if retained_bytes > MAX_ZONE_RENDER_BYTES {
        return Err(format!(
            "zone retains {retained_bytes} render bytes; maximum is {MAX_ZONE_RENDER_BYTES}"
        ));
    }
    Ok((retained_bytes, pixel_bytes))
}

#[allow(dead_code)] // server shares admission helpers but never mutates a render cache
pub(crate) fn checked_zone_render_totals_with_replacements(
    nonpixel_bytes: usize,
    map_pixel_bytes: usize,
    existing: &BTreeMap<Arc<str>, usize>,
    replacements: &BTreeMap<Arc<str>, usize>,
) -> Result<(usize, usize), String> {
    let retained_texture_count = existing
        .keys()
        .filter(|name| !replacements.contains_key(name.as_ref()))
        .count()
        .checked_add(replacements.len())
        .ok_or_else(|| "zone render texture count overflow".to_string())?;
    if retained_texture_count > crate::zone_wire::MAX_LVZ_TEXTURES_PER_ZONE {
        return Err(format!(
            "zone retains {retained_texture_count} render textures; maximum is {}",
            crate::zone_wire::MAX_LVZ_TEXTURES_PER_ZONE
        ));
    }
    let existing_bytes = existing.iter().try_fold(0_usize, |total, (name, bytes)| {
        if replacements.contains_key(name.as_ref()) {
            Ok(total)
        } else {
            total
                .checked_add(*bytes)
                .ok_or_else(|| "zone render image size overflow".to_string())
        }
    })?;
    let image_bytes = replacements
        .values()
        .try_fold(existing_bytes, |total, bytes| {
            total
                .checked_add(*bytes)
                .ok_or_else(|| "zone render image size overflow".to_string())
        })?;
    let pixel_bytes = map_pixel_bytes
        .checked_add(image_bytes)
        .ok_or_else(|| "zone render pixel size overflow".to_string())?;
    if pixel_bytes > MAX_ZONE_RENDER_PIXEL_BYTES {
        return Err(format!(
            "zone images expand to {pixel_bytes} RGBA bytes; maximum is {MAX_ZONE_RENDER_PIXEL_BYTES}"
        ));
    }
    let retained_bytes = nonpixel_bytes
        .checked_add(pixel_bytes)
        .ok_or_else(|| "zone retained render size overflow".to_string())?;
    if retained_bytes > MAX_ZONE_RENDER_BYTES {
        return Err(format!(
            "zone retains {retained_bytes} render bytes; maximum is {MAX_ZONE_RENDER_BYTES}"
        ));
    }
    Ok((retained_bytes, pixel_bytes))
}

pub(crate) fn checked_zone_render_peak(
    retained_before_bytes: usize,
    retained_after_bytes: usize,
    largest_uploaded_rgba_bytes: usize,
) -> Result<usize, String> {
    // During Texture2D creation the old GPU texture, CPU RGBA buffer, and new
    // GPU texture can coexist. Uploads are sequential, so two image products
    // are the only transient pixel copies beyond retained state.
    let upload_peak = retained_before_bytes
        .checked_add(
            largest_uploaded_rgba_bytes
                .checked_mul(2)
                .ok_or_else(|| "zone render upload peak overflow".to_string())?,
        )
        .ok_or_else(|| "zone render upload peak overflow".to_string())?;
    let progressive_peak = retained_after_bytes
        .checked_add(largest_uploaded_rgba_bytes)
        .ok_or_else(|| "zone render progressive upload peak overflow".to_string())?;
    let peak = retained_after_bytes.max(upload_peak).max(progressive_peak);
    if peak > MAX_ZONE_RENDER_PEAK_BYTES {
        return Err(format!(
            "zone render peak is {peak} bytes; maximum is {MAX_ZONE_RENDER_PEAK_BYTES}"
        ));
    }
    Ok(peak)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    #[test]
    fn prepared_images_share_only_peak_headroom_and_reservations_are_atomic() {
        let retained = 128 * 1024 * 1024;
        let reserved_work =
            MAX_IMAGE_DECODER_ALLOC_BYTES as usize + 2 * MAX_RENDER_IMAGE_RGBA_BYTES;
        let headroom = MAX_ZONE_RENDER_PEAK_BYTES - retained - reserved_work;
        let mut budget = ImagePreparationBudget::for_zone_high_water(retained);
        assert!(!budget.try_reserve(headroom + 1));
        assert!(!budget.try_reserve(usize::MAX));
        assert!(budget.try_reserve(headroom / 2));
        assert!(budget.try_reserve(headroom - headroom / 2));
        assert!(!budget.available());
        assert!(!budget.try_reserve(1));
        assert!(!ImagePreparationBudget::default().try_reserve(1));
        for retained in [
            MAX_ZONE_RENDER_BYTES,
            MAX_ZONE_RENDER_PEAK_BYTES,
            usize::MAX,
        ] {
            assert!(!ImagePreparationBudget::for_zone_high_water(retained).available());
        }
    }

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13_u32.to_be_bytes());
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&crate::continuum::continuum_file_crc32(&ihdr).to_be_bytes());
        let mut idat = b"IDAT".to_vec();
        idat.extend_from_slice(&[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
        png.extend_from_slice(&8_u32.to_be_bytes());
        png.extend_from_slice(&idat);
        png.extend_from_slice(&crate::continuum::continuum_file_crc32(&idat).to_be_bytes());
        png.extend_from_slice(&0_u32.to_be_bytes());
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&crate::continuum::continuum_file_crc32(b"IEND").to_be_bytes());
        png
    }

    fn valid_png(width: u32, height: u32) -> Vec<u8> {
        let mut png = Vec::new();
        let pixels = vec![0_u8; width as usize * height as usize * 4];
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        png
    }

    fn minimal_bm2(width: usize, height: usize) -> Vec<u8> {
        let row_stride = (width * 3 + 3) / 4 * 4;
        let mut bm2 = vec![0_u8; 54 + row_stride * height];
        bm2[..2].copy_from_slice(b"BM");
        bm2[10..14].copy_from_slice(&54_u32.to_le_bytes());
        bm2[14..18].copy_from_slice(&40_u32.to_le_bytes());
        bm2[18..22].copy_from_slice(&(width as i32).to_le_bytes());
        bm2[22..26].copy_from_slice(&(height as i32).to_le_bytes());
        bm2[26..28].copy_from_slice(&1_u16.to_le_bytes());
        bm2[28..30].copy_from_slice(&24_u16.to_le_bytes());
        bm2
    }

    #[test]
    fn checked_image_product_accepts_exact_limit_and_rejects_axis_and_product_overage() {
        assert_eq!(
            checked_image_allocation(2_048, 2_048).unwrap().rgba_bytes,
            MAX_RENDER_IMAGE_RGBA_BYTES
        );
        assert!(checked_image_allocation(2_049, 2_048).is_err());
        assert!(checked_image_allocation(MAX_RENDER_IMAGE_DIMENSION + 1, 1).is_err());
        assert!(checked_image_allocation(usize::MAX, 2).is_err());
    }

    #[test]
    fn tiny_bm2_headers_cannot_claim_huge_or_over_limit_dimensions() {
        let mut header = vec![0_u8; 54];
        header[..2].copy_from_slice(b"BM");
        header[18..22].copy_from_slice(&(2_048_i32).to_le_bytes());
        header[22..26].copy_from_slice(&(2_048_i32).to_le_bytes());
        header[26..28].copy_from_slice(&1_u16.to_le_bytes());
        header[28..30].copy_from_slice(&24_u16.to_le_bytes());
        assert_eq!(
            validate_bm2_header(&header).unwrap().rgba_bytes,
            MAX_RENDER_IMAGE_RGBA_BYTES
        );
        header[18..22].copy_from_slice(&(2_049_i32).to_le_bytes());
        assert!(validate_bm2_header(&header).is_err());
        header[18..22].copy_from_slice(&i32::MIN.to_le_bytes());
        header[22..26].copy_from_slice(&i32::MIN.to_le_bytes());
        assert!(validate_bm2_header(&header).is_err());
    }

    #[test]
    fn full_decode_rejects_header_only_images_and_accepts_exact_pixel_limit() {
        let truncated_bm2 = minimal_bm2(2, 2)[..54].to_vec();
        assert!(validate_bm2_header(&truncated_bm2).is_ok());
        assert!(fully_decode_image_for_filename("bad.bm2", &truncated_bm2).is_err());

        let truncated_png = png_header(2, 2);
        assert!(validate_raster_header(&truncated_png).is_ok());
        assert!(fully_decode_image_for_filename("bad.png", &truncated_png).is_err());
        for (filename, bytes) in [
            ("bad.bmp", b"BM\0\0\0\0\0\0\0\0\0\0\0\0".as_slice()),
            ("bad.gif", b"GIF89a\x01\0\x01\0".as_slice()),
            ("bad.jpg", b"\xff\xd8\xff\xe0\0\x10JFIF".as_slice()),
        ] {
            assert!(
                fully_decode_image_for_filename(filename, bytes).is_err(),
                "{filename}"
            );
        }

        let exact = valid_png(2_048, 2_048);
        assert_eq!(
            fully_decode_image_for_filename("exact.png", &exact)
                .unwrap()
                .unwrap()
                .rgba_bytes,
            MAX_RENDER_IMAGE_RGBA_BYTES
        );
        assert!(fully_decode_image_for_filename("over.png", &png_header(2_049, 2_048)).is_err());
    }

    #[test]
    fn embedded_lvl_tileset_requires_a_complete_bm2_decode() {
        let mut truncated = minimal_bm2(2, 2)[..54].to_vec();
        truncated[2..6].copy_from_slice(&54_u32.to_le_bytes());
        assert!(validate_bm2_header(&truncated).is_ok());
        assert!(validate_lvl_source(&truncated).is_err());

        let mut valid = minimal_bm2(2, 2);
        let offset = valid.len() as u32;
        valid[2..6].copy_from_slice(&offset.to_le_bytes());
        assert!(validate_lvl_source(&valid).is_ok());
    }

    #[test]
    fn wave_validation_is_structural_and_pcm_bounded() {
        fn wave(data_len: usize) -> Vec<u8> {
            let mut wav = b"RIFF".to_vec();
            wav.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
            wav.extend_from_slice(&22_050_u32.to_le_bytes());
            wav.extend_from_slice(&22_050_u32.to_le_bytes());
            wav.extend_from_slice(&1_u16.to_le_bytes());
            wav.extend_from_slice(&8_u16.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&(data_len as u32).to_le_bytes());
            wav.resize(wav.len() + data_len, 0x80);
            wav
        }
        let exact = wave(MAX_LVZ_CHAT_SOUND_PCM_BYTES_PER_FILE / 4);
        assert_eq!(
            validate_chat_sound_for_filename("bong255.wa2", &exact).unwrap(),
            Some(MAX_LVZ_CHAT_SOUND_PCM_BYTES_PER_FILE)
        );
        assert!(
            validate_chat_sound_for_filename(
                "bong1.wa2",
                &wave(MAX_LVZ_CHAT_SOUND_PCM_BYTES_PER_FILE / 4 + 1)
            )
            .is_err()
        );
        let mut truncated = exact;
        truncated.pop();
        assert!(validate_chat_sound_for_filename("bong1.wa2", &truncated).is_err());
    }

    #[test]
    fn tiny_png_headers_obey_exact_product_axis_and_decoder_limits_before_decode() {
        assert_eq!(
            validate_raster_header(&png_header(2_048, 2_048))
                .unwrap()
                .rgba_bytes,
            MAX_RENDER_IMAGE_RGBA_BYTES
        );
        assert!(validate_raster_header(&png_header(2_049, 2_048)).is_err());
        assert!(
            validate_raster_header(&png_header((MAX_RENDER_IMAGE_DIMENSION + 1) as u32, 1))
                .is_err()
        );
        assert!(validate_raster_header(&png_header(u32::MAX, u32::MAX)).is_err());
    }

    #[test]
    fn lvl_record_stream_accepts_exact_limit_and_rejects_one_more() {
        let record = (1_u32 << 24).to_le_bytes();
        let exact = record.repeat(MAX_MAP_TILE_RECORDS);
        let layout = validate_lvl_source(&exact).unwrap();
        assert_eq!(layout.record_count, MAX_MAP_TILE_RECORDS);
        let over = record.repeat(MAX_MAP_TILE_RECORDS + 1);
        assert!(validate_lvl_source(&over).is_err());
    }

    #[test]
    fn map_representation_accepts_exact_limit_and_rejects_one_more() {
        assert_eq!(
            checked_map_representation_bytes(0, MAX_MAP_REPRESENTATION_BYTES - 256, 0, 0, 0, 0,)
                .unwrap(),
            MAX_MAP_REPRESENTATION_BYTES
        );
        assert!(
            checked_map_representation_bytes(0, MAX_MAP_REPRESENTATION_BYTES - 255, 0, 0, 0, 0,)
                .is_err()
        );
    }

    #[test]
    fn zone_render_budget_is_exact_and_normalized_texture_replacements_do_not_multiply() {
        let mut images = BTreeMap::new();
        images.insert("shared.png".to_string(), MAX_ZONE_RENDER_PIXEL_BYTES);
        assert_eq!(
            checked_zone_render_totals(
                MAX_ZONE_RENDER_BYTES - MAX_ZONE_RENDER_PIXEL_BYTES,
                0,
                &images,
            )
            .unwrap(),
            (MAX_ZONE_RENDER_BYTES, MAX_ZONE_RENDER_PIXEL_BYTES)
        );
        images.insert("shared.png".to_string(), MAX_ZONE_RENDER_PIXEL_BYTES);
        assert!(
            checked_zone_render_totals(
                MAX_ZONE_RENDER_BYTES - MAX_ZONE_RENDER_PIXEL_BYTES + 1,
                0,
                &images,
            )
            .is_err()
        );
        images.insert("extra.png".to_string(), 1);
        assert!(checked_zone_render_totals(0, 0, &images).is_err());
    }
}
