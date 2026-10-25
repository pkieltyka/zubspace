pub(crate) fn read_u32_le(payload: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        payload.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

pub(crate) fn read_capped_c_string(bytes: &[u8], max_len: usize) -> String {
    let max_len = bytes.len().min(max_len);
    let end = bytes[..max_len]
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(max_len);
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

pub(crate) fn copy_fixed(target: &mut [u8], source: &[u8]) {
    let len = target.len().min(source.len());
    target[..len].copy_from_slice(&source[..len]);
}

pub(crate) fn put_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn put_i32(data: &mut [u8], offset: usize, value: i32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn put_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn put_i16(data: &mut [u8], offset: usize, value: i16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn current_tick() -> u32 {
    // Use a process-monotonic start instant (not wall clock) so the tick
    // doesn't jump if the system clock is adjusted, and route through the
    // canonical integer-duration conversion so the 31-bit mask matches the
    // client. Wide integer centiseconds avoid the `f64 -> u32` saturation
    // that used to freeze the clock after roughly 497 days.
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let elapsed = START.get_or_init(std::time::Instant::now).elapsed();
    crate::continuum::continuum_tick_from_elapsed(elapsed)
}

pub(crate) fn server_session_id() -> u64 {
    // Session identity is metadata for bootstrap/reconnect detection, not a
    // gameplay clock. Gameplay timing always uses `current_tick()`.
    static SERVER_SESSION_ID: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SERVER_SESSION_ID.get_or_init(|| {
        let wall_clock_nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);
        wall_clock_nonce ^ ((std::process::id() as u64) << 32)
    })
}

// Continuum CRC32 lives in `src/continuum.rs` — single source of truth so
// the server-computed map / file checksums can never drift from the
// client's checks.
pub(crate) use crate::continuum::continuum_file_crc32;
