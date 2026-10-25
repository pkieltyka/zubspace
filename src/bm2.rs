use crate::packet_io::{read_i32, read_u16, read_u32};
use crate::render_limits::validate_bm2_header;
use macroquad::prelude::*;

#[derive(Clone, Copy)]
pub(crate) enum Transparency {
    Opaque,
    Black,
}

#[allow(dead_code)] // shared server validation module does not construct GPU textures
pub(crate) fn texture_from_bm2(data: &[u8], transparency: Transparency) -> Texture2D {
    try_texture_from_bm2(data, transparency).expect("failed to decode Continuum BM2 asset")
}

#[allow(dead_code)] // shared server validation module does not construct GPU textures
pub(crate) fn try_texture_from_bm2(
    data: &[u8],
    transparency: Transparency,
) -> Result<Texture2D, String> {
    let decoded = decode_bm2(data, transparency)?;
    Ok(Texture2D::from_rgba8(
        decoded.width as u16,
        decoded.height as u16,
        &decoded.rgba,
    ))
}

pub(crate) struct DecodedImage {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rgba: Vec<u8>,
}

pub(crate) fn decode_bm2(data: &[u8], transparency: Transparency) -> Result<DecodedImage, String> {
    // Validate the complete allocation product before reading palette/RLE
    // metadata or allocating output. This is shared with server admission.
    let allocation = validate_bm2_header(data)?;

    let data_offset = read_u32(data, 10)? as usize;
    let dib_size = read_u32(data, 14)? as usize;
    let _width = read_i32(data, 18)?;
    let height = read_i32(data, 22)?;
    let planes = read_u16(data, 26)?;
    let bpp = read_u16(data, 28)?;
    let compression = read_u32(data, 30)?;
    let color_count = read_u32(data, 46).unwrap_or(0) as usize;

    if dib_size < 40 {
        return Err(format!("BM2 DIB header is too short: {dib_size}"));
    }
    let dib_end = 14_usize
        .checked_add(dib_size)
        .ok_or_else(|| "BM2 DIB range overflow".to_string())?;
    if dib_end > data.len() || data_offset < dib_end || data_offset > data.len() {
        return Err("BM2 DIB/pixel offsets are inconsistent with the file".to_string());
    }

    debug_assert_eq!(planes, 1);
    let width_abs = allocation.width;
    let height_abs = allocation.height;
    let top_down = height < 0;
    if compression == 0 && (bpp == 24 || bpp == 32) {
        return decode_bm2_truecolor_uncompressed(
            data,
            data_offset,
            width_abs,
            height_abs,
            top_down,
            (bpp / 8) as usize,
            transparency,
        );
    }

    let palette_start = 14_usize
        .checked_add(dib_size)
        .ok_or_else(|| "BM2 palette offset overflow".to_string())?;
    let palette_len = if color_count == 0 { 256 } else { color_count };
    if palette_len > 256 {
        return Err(format!(
            "BM2 palette has {palette_len} colors; maximum is 256"
        ));
    }
    let mut palette = vec![[0_u8; 4]; 256];
    let palette_end = palette_start
        .checked_add(
            palette_len
                .checked_mul(4)
                .ok_or_else(|| "BM2 palette size overflow".to_string())?,
        )
        .ok_or_else(|| "BM2 palette range overflow".to_string())?;
    if palette_end > data_offset || palette_end > data.len() {
        return Err("BM2 palette overlaps or extends past pixel data".to_string());
    }

    for (i, color) in palette.iter_mut().enumerate().take(palette_len) {
        let offset = palette_start + i * 4;
        if offset + 3 >= data.len() {
            return Err("palette extends past end of file".to_string());
        }
        let b = data[offset];
        let g = data[offset + 1];
        let r = data[offset + 2];
        let a = match transparency {
            Transparency::Black if r == 0 && g == 0 && b == 0 => 0,
            _ => 255,
        };
        *color = [r, g, b, a];
    }

    if compression == 0 {
        return decode_bm2_uncompressed(
            data,
            data_offset,
            width_abs,
            height_abs,
            top_down,
            &palette,
        );
    }

    let mut rgba = vec![0_u8; allocation.rgba_bytes];
    let mut x = 0_usize;
    let mut y = if top_down { 0 } else { height_abs - 1 };
    let mut i = data_offset;
    let mut finished = false;

    while i + 1 < data.len() {
        let count = data[i];
        let value = data[i + 1];
        i += 2;

        if count > 0 {
            for _ in 0..count {
                put_pixel(
                    &mut rgba,
                    width_abs,
                    height_abs,
                    x,
                    y,
                    palette[value as usize],
                )?;
                x += 1;
            }
            continue;
        }

        match value {
            0 => {
                x = 0;
                if top_down {
                    y += 1;
                } else if y == 0 {
                    finished = true;
                    break;
                } else {
                    y -= 1;
                }
            }
            1 => {
                finished = true;
                break;
            }
            2 => {
                if i + 1 >= data.len() {
                    return Err("truncated RLE delta".to_string());
                }
                x += data[i] as usize;
                if top_down {
                    y += data[i + 1] as usize;
                } else {
                    y = y.saturating_sub(data[i + 1] as usize);
                }
                i += 2;
            }
            absolute_count => {
                let absolute_count = absolute_count as usize;
                if i + absolute_count > data.len() {
                    return Err("truncated RLE absolute run".to_string());
                }
                for index in &data[i..i + absolute_count] {
                    put_pixel(
                        &mut rgba,
                        width_abs,
                        height_abs,
                        x,
                        y,
                        palette[*index as usize],
                    )?;
                    x += 1;
                }
                i += absolute_count;
                if i & 1 == 1 {
                    i += 1;
                }
            }
        }
    }

    if !finished {
        return Err("truncated BM2 RLE stream without an end marker".to_string());
    }

    Ok(DecodedImage {
        width: width_abs,
        height: height_abs,
        rgba,
    })
}

fn decode_bm2_truecolor_uncompressed(
    data: &[u8],
    data_offset: usize,
    width: usize,
    height: usize,
    top_down: bool,
    bytes_per_pixel: usize,
    transparency: Transparency,
) -> Result<DecodedImage, String> {
    let row_bytes = width
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| "truecolor BM2 row size overflow".to_string())?;
    let row_stride = row_bytes
        .checked_add(3)
        .map(|value| value / 4 * 4)
        .ok_or_else(|| "truecolor BM2 row stride overflow".to_string())?;
    let required = row_stride
        .checked_mul(height)
        .and_then(|bytes| data_offset.checked_add(bytes))
        .ok_or_else(|| "truecolor BM2 pixel range overflow".to_string())?;
    if required > data.len() {
        return Err("uncompressed truecolor BM2 pixel data extends past end of file".to_string());
    }

    let rgba_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "truecolor BM2 output size overflow".to_string())?;
    let mut rgba = vec![0_u8; rgba_bytes];
    for file_y in 0..height {
        let y = if top_down {
            file_y
        } else {
            height - 1 - file_y
        };
        let row_offset = data_offset + file_y * row_stride;
        for x in 0..width {
            let pixel_offset = row_offset + x * bytes_per_pixel;
            let b = data[pixel_offset];
            let g = data[pixel_offset + 1];
            let r = data[pixel_offset + 2];
            let a = match transparency {
                Transparency::Black if r == 0 && g == 0 && b == 0 => 0,
                _ => 255,
            };
            put_pixel(&mut rgba, width, height, x, y, [r, g, b, a])?;
        }
    }

    Ok(DecodedImage {
        width,
        height,
        rgba,
    })
}

fn decode_bm2_uncompressed(
    data: &[u8],
    data_offset: usize,
    width: usize,
    height: usize,
    top_down: bool,
    palette: &[[u8; 4]],
) -> Result<DecodedImage, String> {
    let row_stride = width
        .checked_add(3)
        .map(|value| value / 4 * 4)
        .ok_or_else(|| "BM2 row stride overflow".to_string())?;
    let required = row_stride
        .checked_mul(height)
        .and_then(|bytes| data_offset.checked_add(bytes))
        .ok_or_else(|| "BM2 pixel range overflow".to_string())?;
    if required > data.len() {
        return Err("uncompressed BM2 pixel data extends past end of file".to_string());
    }

    let rgba_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "BM2 output size overflow".to_string())?;
    let mut rgba = vec![0_u8; rgba_bytes];
    for file_y in 0..height {
        let y = if top_down {
            file_y
        } else {
            height - 1 - file_y
        };
        let row_offset = data_offset + file_y * row_stride;
        for x in 0..width {
            let palette_index = data[row_offset + x] as usize;
            put_pixel(&mut rgba, width, height, x, y, palette[palette_index])?;
        }
    }

    Ok(DecodedImage {
        width,
        height,
        rgba,
    })
}

fn put_pixel(
    rgba: &mut [u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    color: [u8; 4],
) -> Result<(), String> {
    if x >= width || y >= height {
        return Err(format!(
            "RLE pixel out of bounds: {x},{y} in {width}x{height}"
        ));
    }
    let offset = (y * width + x) * 4;
    rgba[offset..offset + 4].copy_from_slice(&color);
    Ok(())
}
