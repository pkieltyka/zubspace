use flate2::read::ZlibDecoder;
use std::io::Read;
use std::time::Duration;

use crate::CONTINUUM_ARENA_SETTINGS_SIZE;

// Materialize the fixed polynomial once at compile time. Building its eight
// bit steps for every asset byte stalled ordinary debug-WASM file admission.
const REFLECTED_CRC32_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut index = 0;
    while index < table.len() {
        table[index] = reflected_crc32_table_entry(index as u32);
        index += 1;
    }
    table
};

#[allow(dead_code)]
pub(crate) fn continuum_settings_checksum(key: u32, data: &[u8]) -> Result<u32, String> {
    if data.len() < CONTINUUM_ARENA_SETTINGS_SIZE {
        return Err(format!(
            "arena settings packet too short: {} < {}",
            data.len(),
            CONTINUUM_ARENA_SETTINGS_SIZE
        ));
    }

    Ok(data[..CONTINUUM_ARENA_SETTINGS_SIZE]
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) ^ key)
        .fold(0_u32, u32::wrapping_add))
}

#[allow(dead_code)]
pub(crate) fn continuum_map_crc32(tiles: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;

    for tile in tiles
        .iter()
        .copied()
        .filter(|tile| *tile != 0 && (*tile < 0xa1 || *tile == 0xab))
    {
        crc = (crc >> 8) ^ REFLECTED_CRC32_TABLE[((crc ^ tile as u32) & 0xff) as usize];
    }

    crc
}

#[allow(dead_code)]
pub(crate) fn continuum_file_crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;

    for byte in data {
        crc = (crc >> 8) ^ REFLECTED_CRC32_TABLE[((crc ^ *byte as u32) & 0xff) as usize];
    }

    !crc
}

#[allow(dead_code)]
pub(crate) fn weapon_checksum(data: &[u8]) -> u8 {
    data.iter().fold(0, |checksum, byte| checksum ^ *byte)
}

#[allow(dead_code)] // client-bin only
pub(crate) fn bitfield(value: u32, shift: u32, width: u32) -> u32 {
    (value >> shift) & ((1 << width) - 1)
}

#[allow(dead_code)] // client-bin only
pub(crate) fn ticks_to_seconds(ticks: u16) -> f32 {
    ticks as f32 / 100.0
}

pub(crate) fn ticks_to_seconds_u32(ticks: u32) -> f32 {
    ticks as f32 / 100.0
}

#[allow(dead_code)] // client-bin only
pub(crate) fn ticks_to_seconds_i16(ticks: i16) -> f32 {
    ticks.max(0) as f32 / 100.0
}

#[allow(dead_code)] // client-bin only
pub(crate) fn ticks_to_seconds_i32(ticks: i32) -> f32 {
    ticks.max(0) as f32 / 100.0
}

/// Continuum stores many wire-format values as `value * 1000` so they can
/// be carried in integer fields with three decimal places of precision —
/// damage, percent multipliers, and so on. Convert the stored
/// representation back to an `f32` ratio/value, clamping negatives to 0.
#[allow(dead_code)]
pub(crate) fn continuum_thousandths_to_f32(stored: i32) -> f32 {
    stored.max(0) as f32 / 1000.0
}

/// Inverse of `ticks_to_seconds_*`: `seconds * 100` rounded to the nearest
/// Continuum tick. Negative durations clamp to 0.
#[allow(dead_code)]
pub(crate) fn seconds_to_ticks_u32(seconds: f32) -> u32 {
    (seconds.max(0.0) * 100.0).round() as u32
}

#[allow(dead_code)]
pub(crate) fn seconds_to_ticks_i32(seconds: f32) -> i32 {
    (seconds * 100.0).round() as i32
}

#[allow(dead_code)] // client-bin only
pub(crate) fn continuum_bounce_factor(raw_factor: i16) -> f32 {
    16.0 / raw_factor.max(1) as f32
}

#[allow(dead_code)]
pub(crate) fn continuum_radius_pixels(radius: u8) -> f32 {
    // ASSS clientset.c and SubspaceServer ClientSettings.cs copy Radius
    // directly into the eight-bit settings field: it is already in pixels.
    // The reference game divides this value by 16 only because its world is tile-scaled.
    if radius == 0 { 14.0 } else { radius as f32 }
}

#[allow(dead_code)]
pub(crate) fn sign_extend_i16(value: u16, bits: u8) -> i16 {
    let shift = 16 - bits;
    ((value << shift) as i16) >> shift
}

#[allow(dead_code)]
pub(crate) fn decompress_zlib_payload(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = ZlibDecoder::new(data);
    let mut decoded = Vec::new();
    decoder
        .read_to_end(&mut decoded)
        .map_err(|err| format!("failed to decompress zlib payload: {err}"))?;
    Ok(decoded)
}

#[allow(dead_code)]
pub(crate) fn decompress_zlib_payload_bounded(
    data: &[u8],
    max_decoded_bytes: usize,
) -> Result<Vec<u8>, String> {
    let decoder = ZlibDecoder::new(data);
    let mut decoded = Vec::new();
    decoder
        .take(max_decoded_bytes.saturating_add(1) as u64)
        .read_to_end(&mut decoded)
        .map_err(|err| format!("failed to decompress zlib payload: {err}"))?;
    if decoded.len() > max_decoded_bytes {
        return Err(format!(
            "decompressed zlib payload exceeds {max_decoded_bytes} bytes"
        ));
    }
    Ok(decoded)
}

pub(crate) const CONTINUUM_TICK_MASK: u32 = 0x7fff_ffff;
pub(crate) const CONTINUUM_TICK_MODULUS: u128 = 1_u128 << 31;

/// Continuum tick diff with proper 31-bit wraparound. Both `a` and `b` are
/// masked into the 31-bit timestamp space before subtraction so the result
/// stays correct across the wrap point.
#[allow(dead_code)]
pub(crate) fn continuum_tick_diff(a: u32, b: u32) -> i32 {
    ((((a & CONTINUUM_TICK_MASK) << 1).wrapping_sub((b & CONTINUUM_TICK_MASK) << 1)) as i32) >> 1
}

/// Add a duration to a Continuum tick while staying in its 31-bit timestamp
/// space.
#[allow(dead_code)]
pub(crate) fn continuum_tick_add(tick: u32, delta: u32) -> u32 {
    tick.wrapping_add(delta) & CONTINUUM_TICK_MASK
}

/// Subtract a duration from a Continuum tick while staying in its 31-bit
/// timestamp space.
#[allow(dead_code)]
pub(crate) fn continuum_tick_sub(tick: u32, delta: u32) -> u32 {
    tick.wrapping_sub(delta) & CONTINUUM_TICK_MASK
}

/// Return elapsed ticks when `now` is at or after `anchor`. A slightly-ahead
/// anchor is treated as clock skew, not almost a full 31-bit cycle.
#[allow(dead_code)]
pub(crate) fn continuum_tick_elapsed(now: u32, anchor: u32) -> u32 {
    continuum_tick_diff(now, anchor).max(0) as u32
}

/// Whether a wrap-safe Continuum deadline has been reached.
#[allow(dead_code)]
pub(crate) fn continuum_tick_reached(now: u32, deadline: u32) -> bool {
    continuum_tick_diff(now, deadline) >= 0
}

/// Convert a process-monotonic elapsed duration into the 31-bit Continuum
/// tick space. Compute centiseconds in a wide integer before reducing the
/// value: narrowing or converting through `f64` first would eventually
/// saturate/freeze the gameplay clock.
#[allow(dead_code)]
pub(crate) fn continuum_tick_from_elapsed(elapsed: Duration) -> u32 {
    let ticks =
        u128::from(elapsed.as_secs()) * 100 + u128::from(elapsed.subsec_nanos() / 10_000_000);
    (ticks % CONTINUUM_TICK_MODULUS) as u32
}

const fn reflected_crc32_table_entry(index: u32) -> u32 {
    let mut value = index;
    let mut bit = 0;
    while bit < 8 {
        if value & 1 == 0 {
            value >>= 1;
        } else {
            value = (value >> 1) ^ 0xedb8_8320;
        }
        bit += 1;
    }
    value
}

#[cfg(test)]
mod tick_tests {
    use super::*;

    #[test]
    fn crc_lookup_preserves_every_byte_and_continuum_map_filter() {
        // Independent bit-at-a-time reference, including the map's distinct
        // tile filter and lack of a final complement.
        fn reference(bytes: impl Iterator<Item = u8>) -> u32 {
            let mut crc = u32::MAX;
            for byte in bytes {
                crc ^= byte as u32;
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
                }
            }
            crc
        }
        for byte in 0..=u8::MAX {
            assert_eq!(
                continuum_file_crc32(&[byte]),
                !reference([byte].into_iter())
            );
        }
        let bytes = (0..=u8::MAX).cycle().take(4097).collect::<Vec<_>>();
        assert_eq!(
            continuum_file_crc32(&bytes),
            !reference(bytes.iter().copied())
        );
        assert_eq!(
            continuum_map_crc32(&bytes),
            reference(
                bytes
                    .iter()
                    .copied()
                    .filter(|tile| { *tile != 0 && (*tile < 0xa1 || *tile == 0xab) })
            )
        );
        assert_eq!(continuum_file_crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn continuum_tick_helpers_cross_the_31_bit_wrap() {
        let before_wrap = 0x7fff_fffe;
        let after_wrap = continuum_tick_add(before_wrap, 3);

        assert_eq!(after_wrap, 1);
        assert_eq!(continuum_tick_diff(after_wrap, before_wrap), 3);
        assert_eq!(continuum_tick_elapsed(after_wrap, before_wrap), 3);
        assert_eq!(continuum_tick_sub(after_wrap, 3), before_wrap);
        assert!(continuum_tick_reached(after_wrap, before_wrap));
        assert!(!continuum_tick_reached(before_wrap, after_wrap));
    }

    #[test]
    fn continuum_elapsed_rejects_a_slightly_ahead_anchor() {
        assert_eq!(continuum_tick_diff(100, 101), -1);
        assert_eq!(continuum_tick_elapsed(100, 101), 0);
    }

    #[test]
    fn elapsed_duration_clock_wraps_without_saturating() {
        for total_ticks in [
            0_u64,
            1,
            (1_u64 << 31) - 1,
            1_u64 << 31,
            (1_u64 << 32) + 17,
            3 * (1_u64 << 31) + 123,
        ] {
            let elapsed = Duration::from_millis(total_ticks * 10);
            assert_eq!(
                continuum_tick_from_elapsed(elapsed),
                (u128::from(total_ticks) % CONTINUUM_TICK_MODULUS) as u32
            );
        }
    }

    #[test]
    fn gameplay_sources_do_not_reintroduce_masked_tick_subtraction() {
        let audited_sources = [
            ("authority/weapons.rs", include_str!("authority/weapons.rs")),
            (
                "authority_projectiles/registry.rs",
                include_str!("authority_projectiles/registry.rs"),
            ),
            (
                "server/arena/positions.rs",
                include_str!("bin/zubspace_server/arena/positions.rs"),
            ),
            (
                "server/arena/positions/weapons.rs",
                include_str!("bin/zubspace_server/arena/positions/weapons.rs"),
            ),
            (
                "server/hidden_prizes.rs",
                include_str!("bin/zubspace_server/hidden_prizes.rs"),
            ),
            (
                "server/objectives/bricks.rs",
                include_str!("bin/zubspace_server/objectives/bricks.rs"),
            ),
            (
                "server/objectives/powerballs.rs",
                include_str!("bin/zubspace_server/objectives/powerballs.rs"),
            ),
            (
                "server/server_damage.rs",
                include_str!("bin/zubspace_server/server_damage.rs"),
            ),
            (
                "server/server_tick.rs",
                include_str!("bin/zubspace_server/server_tick.rs"),
            ),
            (
                "server/state/client_runtime.rs",
                include_str!("bin/zubspace_server/state/client_runtime.rs"),
            ),
            (
                "server/state/projectiles.rs",
                include_str!("bin/zubspace_server/state/projectiles.rs"),
            ),
        ];

        for (name, source) in audited_sources {
            let code_only = source
                .lines()
                .map(|line| line.split("//").next().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                !(code_only.contains("wrapping_sub") && code_only.contains("0x7fff_ffff")),
                "{name} must use the canonical Continuum tick helpers"
            );
        }
    }
}
