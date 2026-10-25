#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn zub_client_debug_frame_requested() -> i32;
    fn zub_client_debug_frame(ptr: *const f64, count: usize) -> i32;
    fn zub_client_debug_muzzle_requested() -> i32;
    fn zub_client_debug_muzzle(ptr: *const f64, count: usize) -> i32;
    fn zub_ui_metrics_requested() -> i32;
    fn zub_debug_mode() -> i32;
    fn zub_frequency_changes_allowed() -> i32;
    fn zub_show_controls();
    fn zub_toggle_fullscreen() -> i32;
    fn zub_remote_sprite_trace_requested() -> i32;
    fn zub_ui_metrics(ptr: *const u8, len: usize) -> i32;
    fn zub_lifecycle_state(ptr: *const u8, len: usize) -> i32;
    fn zub_remote_sprite_frame(ptr: *const u8, len: usize) -> i32;
    fn zub_damage_event(ptr: *const u8, len: usize) -> i32;
    fn zub_weapon_admission_event(ptr: *const u8, len: usize) -> i32;
    fn zub_qa_trace_context(ptr: *const u8, len: usize) -> i32;
    fn zub_local_minutes_since_midnight() -> i32;
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn client_debug_frame_requested() -> bool {
    unsafe { zub_client_debug_frame_requested() != 0 }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_client_debug_frame(frame: &[f64; 32]) {
    unsafe {
        let _ = zub_client_debug_frame(frame.as_ptr(), frame.len());
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn client_debug_muzzle_requested() -> bool {
    unsafe { zub_client_debug_muzzle_requested() != 0 }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_client_debug_muzzle(row: &[f64; 32]) -> bool {
    unsafe { zub_client_debug_muzzle(row.as_ptr(), row.len()) != 0 }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn ui_metrics_requested() -> bool {
    unsafe { zub_ui_metrics_requested() != 0 }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn ui_metrics_requested() -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn remote_sprite_trace_requested() -> bool {
    unsafe { zub_remote_sprite_trace_requested() != 0 }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn remote_sprite_trace_requested() -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_ui_metrics(metrics: &str) {
    unsafe {
        let _ = zub_ui_metrics(metrics.as_ptr(), metrics.len());
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn publish_ui_metrics(_metrics: &str) {}

/// Publish the small always-on lifecycle state. Unlike `publish_ui_metrics` this is called by the caller
/// only when the value changed, so an ordinary frame pays nothing for it.
#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_lifecycle_state(state: &str) {
    unsafe {
        let _ = zub_lifecycle_state(state.as_ptr(), state.len());
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn publish_lifecycle_state(_state: &str) {}

#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_remote_sprite_frame(frame: &str) {
    unsafe {
        let _ = zub_remote_sprite_frame(frame.as_ptr(), frame.len());
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn publish_remote_sprite_frame(_frame: &str) {}

#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_damage_event(event: &str) {
    unsafe {
        let _ = zub_damage_event(event.as_ptr(), event.len());
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn publish_damage_event(_event: &str) {}

#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_weapon_admission_event(event: &str) {
    unsafe {
        let _ = zub_weapon_admission_event(event.as_ptr(), event.len());
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn publish_weapon_admission_event(_event: &str) {}

#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_qa_trace_context(context: &str) {
    unsafe {
        let _ = zub_qa_trace_context(context.as_ptr(), context.len());
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn publish_qa_trace_context(_context: &str) {}

#[cfg(target_arch = "wasm32")]
pub(crate) fn local_minutes_since_midnight() -> Option<u16> {
    let minutes = unsafe { zub_local_minutes_since_midnight() };
    (0..(24 * 60)).contains(&minutes).then_some(minutes as u16)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn local_minutes_since_midnight() -> Option<u16> {
    None
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_player_frame(corrections: u32, ready: bool) {
    unsafe extern "C" {
        fn zub_player_frame(corrections: u32, ready: i32);
    }
    unsafe {
        zub_player_frame(corrections, i32::from(ready));
    }
}

/// The page sends its own visibility notices (`0x85`) when it hides or shows,
/// because no frame runs while it is hidden. It needs the current scope;
/// zeros clear it. Published only when it changes.
#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_connection_scope(server_session_id: u64, arena_stream_id: u64) {
    thread_local! {
        static PUBLISHED: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
    }
    let scope = (server_session_id, arena_stream_id);
    if PUBLISHED.with(|published| published.replace(scope)) == scope {
        return;
    }
    unsafe extern "C" {
        fn zub_connection_scope(ptr: *const u8, len: usize);
    }
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&server_session_id.to_le_bytes());
    bytes[8..].copy_from_slice(&arena_stream_id.to_le_bytes());
    unsafe {
        zub_connection_scope(bytes.as_ptr(), bytes.len());
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn publish_player_report(server_session_id: u64, arena_stream_id: u64, life_seq: u32) {
    unsafe extern "C" {
        fn zub_player_report(ptr: *const u8, len: usize);
    }
    // Version 2 appends the game channel's peak send queue and dropped
    // sends; the page fills both after the eight version 1 counters.
    let mut packet = [0_u8; 62];
    packet[0] = 0x80;
    packet[1] = 2;
    packet[2..10].copy_from_slice(&server_session_id.to_le_bytes());
    packet[10..18].copy_from_slice(&arena_stream_id.to_le_bytes());
    packet[18..22].copy_from_slice(&life_seq.to_le_bytes());
    unsafe {
        zub_player_report(packet.as_ptr(), packet.len());
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn debug_mode() -> bool {
    unsafe { zub_debug_mode() == 1 }
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn debug_mode() -> bool {
    false
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn show_controls() {
    unsafe {
        zub_show_controls();
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn show_controls() {}

#[cfg(target_arch = "wasm32")]
pub(crate) fn toggle_fullscreen() -> bool {
    unsafe { zub_toggle_fullscreen() == 1 }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn toggle_fullscreen() -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn frequency_changes_allowed() -> bool {
    unsafe { zub_frequency_changes_allowed() == 1 }
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn frequency_changes_allowed() -> bool {
    false
}

/// Selected WebRTC route RTT; no fabricated zero while disconnected/unmeasured.
#[cfg(target_arch = "wasm32")]
pub(crate) fn connection_ping_ms() -> Option<u32> {
    unsafe extern "C" {
        fn zub_connection_ping_ms() -> i32;
    }
    u32::try_from(unsafe { zub_connection_ping_ms() }).ok()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn connection_ping_ms() -> Option<u32> {
    None
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn connection_ping_stats() -> Option<[u32; 5]> {
    unsafe extern "C" {
        fn zub_connection_ping_stats(out: *mut u32) -> i32;
    }
    let mut stats = [0; 5];
    (unsafe { zub_connection_ping_stats(stats.as_mut_ptr()) } == 1).then_some(stats)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn connection_ping_stats() -> Option<[u32; 5]> {
    None
}
