#[allow(dead_code)]
pub(crate) trait PacketTransport {
    fn send_packet(&mut self, packet: &[u8]) -> Result<(), String>;
    fn recv_packet(&mut self) -> Result<Option<Vec<u8>>, String>;

    fn last_received_lane(&self) -> PacketTransportLane {
        PacketTransportLane::Unknown
    }
}

#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PacketTransportLane {
    #[default]
    Unknown,
    Game,
    Event,
    Reliable,
}

#[cfg(any(test, target_arch = "wasm32"))]
fn browser_rtc_packet_limit(lane: PacketTransportLane) -> usize {
    match lane {
        // WebRTC messages are not legacy UDP/reliable datagrams. This bound
        // matches the vendored SCTP default advertised as max-message-size
        // (64 KiB) and admits dense full snapshots plus private baselines.
        PacketTransportLane::Game => 64 * 1024,
        PacketTransportLane::Event => crate::realtime_event::MAX_REALTIME_EVENT_PACKET_LEN,
        PacketTransportLane::Reliable | PacketTransportLane::Unknown => {
            crate::ReliableSequencer::MAX_PACKET_SIZE
        }
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn take_browser_rtc_packet(
    len: usize,
    lane: PacketTransportLane,
    read: impl FnOnce(&mut [u8]) -> Result<usize, String>,
    discard: impl FnOnce() -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    let limit = browser_rtc_packet_limit(lane);
    if len > limit {
        // Do not allocate a buffer using a rejected length or leave the
        // rejected message blocking valid messages on the other RTC lanes.
        discard()?;
        return Err(format!("browser RTC packet is too large: {len} > {limit}"));
    }
    let mut packet = vec![0; len];
    let copied = read(&mut packet)?;
    if copied != len {
        return Err(format!(
            "browser RTC packet length changed while receiving: expected {len}, got {copied}"
        ));
    }
    Ok(packet)
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn zub_rtc_url_len() -> i32;
    fn zub_rtc_url(ptr: *mut u8, len: i32) -> i32;
    fn zub_login_name_len() -> i32;
    fn zub_login_name(ptr: *mut u8, len: i32) -> i32;
    fn zub_login_password_len() -> i32;
    fn zub_login_password(ptr: *mut u8, len: i32) -> i32;
    fn zub_player_identity_len() -> i32;
    fn zub_player_identity(ptr: *mut u8, len: i32) -> i32;
    fn zub_arena_name_len() -> i32;
    fn zub_arena_name(ptr: *mut u8, len: i32) -> i32;
    fn zub_initial_ship() -> i32;
    fn zub_rtc_connect(ptr: *const u8, len: i32) -> i32;
    fn zub_rtc_status() -> i32;
    fn zub_rtc_terminal() -> i32;
    fn zub_rtc_send(ptr: *const u8, len: i32) -> i32;
    fn zub_rtc_recv_len() -> i32;
    fn zub_rtc_recv_channel() -> i32;
    fn zub_rtc_discard_recv() -> i32;
    fn zub_rtc_recv(ptr: *mut u8, len: i32) -> i32;
    fn zub_disconnect_requested() -> i32;
    fn zub_disconnect_sent();
    fn zub_reset_player_identity(ptr: *const u8, len: i32) -> i32;
    fn zub_request_main_menu() -> i32;
    fn zub_request_join_retry(code: i32) -> i32;
    fn zub_network_dropped_realtime_packets() -> u32;
    fn zub_network_gameplay_clock_stalled() -> i32;
    fn zub_map_cache_len(identity_ptr: *const u8, identity_len: i32) -> i32;
    fn zub_map_cache_read(
        identity_ptr: *const u8,
        identity_len: i32,
        data_ptr: *mut u8,
        data_len: i32,
    ) -> i32;
    fn zub_map_cache_write(
        identity_ptr: *const u8,
        identity_len: i32,
        data_ptr: *const u8,
        data_len: i32,
    ) -> i32;
    fn zub_map_cache_remove(identity_ptr: *const u8, identity_len: i32);
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BrowserRtcStatus {
    Idle,
    Connecting,
    Open,
    Closed,
    Error,
    Unknown(i32),
}

#[cfg(target_arch = "wasm32")]
impl BrowserRtcStatus {
    fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Idle,
            1 => Self::Connecting,
            2 => Self::Open,
            3 => Self::Closed,
            4 => Self::Error,
            code => Self::Unknown(code),
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) struct BrowserRtcTransport {
    last_received_lane: PacketTransportLane,
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) struct BrowserNetworkConfig {
    pub(crate) rtc_url: Option<String>,
    pub(crate) login_name: String,
    pub(crate) password: String,
    pub(crate) player_identity: String,
    pub(crate) arena_name: String,
    pub(crate) initial_ship: u8,
}

#[cfg(any(test, target_arch = "wasm32"))]
impl BrowserNetworkConfig {
    const DEFAULT_LOGIN_NAME: &'static str = "zubspace";
    const DEFAULT_PLAYER_IDENTITY: &'static str = "000000000001";

    pub(crate) fn from_values(
        rtc_url: impl Into<String>,
        login_name: impl Into<String>,
        password: impl Into<String>,
        player_identity: impl Into<String>,
        arena_name: impl Into<String>,
    ) -> Self {
        let rtc_url = rtc_url.into();
        let login_name = login_name.into();
        let player_identity = player_identity.into();

        Self {
            rtc_url: (!rtc_url.is_empty()).then_some(rtc_url),
            login_name: if login_name.is_empty() {
                Self::DEFAULT_LOGIN_NAME.to_string()
            } else {
                login_name
            },
            password: password.into(),
            player_identity: if valid_browser_player_identity(&player_identity) {
                player_identity
            } else {
                Self::DEFAULT_PLAYER_IDENTITY.to_string()
            },
            arena_name: arena_name.into(),
            initial_ship: 0,
        }
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn valid_browser_player_identity(identity: &str) -> bool {
    identity.len() == 12 && identity.as_bytes().iter().all(u8::is_ascii_digit)
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn browser_player_identity_parts(identity: &str) -> Option<(u32, u32)> {
    if !valid_browser_player_identity(identity) {
        return None;
    }
    let value = identity.parse::<u64>().ok()?;
    Some((value as u32, (value >> 32) as u32))
}

#[cfg(any(test, target_arch = "wasm32"))]
#[cfg(target_arch = "wasm32")]
pub(crate) fn reset_browser_player_identity(reason: &str) -> Result<(), String> {
    let len = checked_js_len(reason.len())?;
    let result = unsafe { zub_reset_player_identity(reason.as_ptr(), len) };
    if result < 0 {
        return Err(format!("failed to reset browser player identity: {result}"));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn request_browser_main_menu() -> Result<(), String> {
    if unsafe { zub_request_main_menu() } < 0 {
        return Err("failed to return to the main menu".to_string());
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn request_browser_join_retry(code: i32) -> Result<(), String> {
    if unsafe { zub_request_join_retry(code) } < 0 {
        return Err("failed to return to the arena join form".to_owned());
    }
    Ok(())
}

/// The transport status code the page reports, without needing a transport object.
///
/// A client whose peer connection has died still reports `zoneFilesReady: true` and a stale-but-plausible server tick, so a
/// dead socket looks like a readiness bug unless the code the page itself reports is in the same artifact as those flags.
/// This is that code, published in `uiMetrics` for exactly that reason.
#[cfg(target_arch = "wasm32")]
pub(crate) fn browser_rtc_status_code() -> i32 {
    unsafe { zub_rtc_status() }
}

#[cfg(target_arch = "wasm32")]
/// True once the transport reached open and is now gone, which is the precondition for a rejoin and nothing else.
///
/// The status code cannot answer this. The page reports 3 both when ICE disconnects with a grace window still running -
/// where a rejoin would abort a recoverable session - and when the peer connection is closed for good, and it reports 4
/// for a signaling failure that never reached open, where the existing connect failure UI is already the right answer.
/// So the page answers the conjunction directly instead of leaving every caller to reconstruct it from two fields.
pub(crate) fn browser_rtc_transport_is_terminal() -> bool {
    unsafe { zub_rtc_terminal() != 0 }
}

/// Native builds have no browser transport, so a session can never be terminal.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn browser_rtc_transport_is_terminal() -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
impl BrowserRtcTransport {
    pub(crate) fn connect(url: &str) -> Result<Self, String> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(
                "browser RTC transport requires ?rtc=http(s)://.../v1/rtc/offer".to_string(),
            );
        }
        let len = checked_js_len(url.len())?;
        let result = unsafe { zub_rtc_connect(url.as_ptr(), len) };
        if result < 0 {
            return Err(format!("failed to open browser RTC transport: {result}"));
        }

        Ok(Self {
            last_received_lane: PacketTransportLane::Unknown,
        })
    }

    pub(crate) fn status(&self) -> BrowserRtcStatus {
        BrowserRtcStatus::from_code(unsafe { zub_rtc_status() })
    }

    pub(crate) fn transport_name(&self) -> &'static str {
        "rtc"
    }
}

#[cfg(target_arch = "wasm32")]
impl PacketTransport for BrowserRtcTransport {
    fn send_packet(&mut self, packet: &[u8]) -> Result<(), String> {
        let len = checked_js_len(packet.len())?;
        match unsafe { zub_rtc_send(packet.as_ptr(), len) } {
            0 => Ok(()),
            -1 => Err("browser RTC transport is not connected".to_string()),
            -2 => Err(format!(
                "browser RTC transport is not open: {:?}",
                self.status()
            )),
            code => Err(format!("failed to send browser RTC packet: {code}")),
        }
    }

    fn recv_packet(&mut self) -> Result<Option<Vec<u8>>, String> {
        let len = unsafe { zub_rtc_recv_len() };
        if len < 0 {
            return Err(format!("failed to read browser RTC packet length: {len}"));
        }
        if len == 0 {
            self.last_received_lane = PacketTransportLane::Unknown;
            return Ok(None);
        }

        self.last_received_lane = match unsafe { zub_rtc_recv_channel() } {
            1 => PacketTransportLane::Game,
            2 => PacketTransportLane::Event,
            3 => PacketTransportLane::Reliable,
            _ => PacketTransportLane::Unknown,
        };

        let packet = take_browser_rtc_packet(
            len as usize,
            self.last_received_lane,
            |packet| {
                let copied =
                    unsafe { zub_rtc_recv(packet.as_mut_ptr(), checked_js_len(packet.len())?) };
                if copied < 0 {
                    Err(format!("failed to receive browser RTC packet: {copied}"))
                } else {
                    Ok(copied as usize)
                }
            },
            || {
                if unsafe { zub_rtc_discard_recv() } < 0 {
                    Err("failed to discard rejected browser RTC packet".to_string())
                } else {
                    Ok(())
                }
            },
        )?;

        Ok(Some(packet))
    }

    fn last_received_lane(&self) -> PacketTransportLane {
        self.last_received_lane
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn browser_disconnect_requested() -> bool {
    unsafe { zub_disconnect_requested() != 0 }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn mark_browser_disconnect_sent() {
    unsafe { zub_disconnect_sent() };
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn browser_dropped_realtime_packets() -> u32 {
    unsafe { zub_network_dropped_realtime_packets() }
}

/// Browser-E2E-only selective clock seam. The DataChannel continues to
/// deliver and parse snapshots normally; only local gameplay readiness stops
/// observing their advancing clock while this normal-off flag is set.
#[cfg(target_arch = "wasm32")]
pub(crate) fn browser_gameplay_clock_stalled() -> bool {
    unsafe { zub_network_gameplay_clock_stalled() != 0 }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn load_browser_cached_map(identity: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let identity_len = checked_js_len(identity.len()).ok()?;
    let len = unsafe { zub_map_cache_len(identity.as_ptr(), identity_len) };
    if len <= 0 || len as usize > max_bytes {
        if len != 0 {
            unsafe { zub_map_cache_remove(identity.as_ptr(), identity_len) };
        }
        return None;
    }
    let mut bytes = vec![0; len as usize];
    let copied = unsafe {
        zub_map_cache_read(
            identity.as_ptr(),
            identity_len,
            bytes.as_mut_ptr(),
            checked_js_len(bytes.len()).ok()?,
        )
    };
    if copied as usize != bytes.len() {
        unsafe { zub_map_cache_remove(identity.as_ptr(), identity_len) };
        return None;
    }
    Some(bytes)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn store_browser_cached_map(identity: &str, bytes: &[u8]) {
    let (Ok(identity_len), Ok(data_len)) =
        (checked_js_len(identity.len()), checked_js_len(bytes.len()))
    else {
        return;
    };
    let _ =
        unsafe { zub_map_cache_write(identity.as_ptr(), identity_len, bytes.as_ptr(), data_len) };
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn remove_browser_cached_map(identity: &str) {
    let Ok(identity_len) = checked_js_len(identity.len()) else {
        return;
    };
    unsafe { zub_map_cache_remove(identity.as_ptr(), identity_len) };
}

#[cfg(target_arch = "wasm32")]
fn checked_js_len(len: usize) -> Result<i32, String> {
    i32::try_from(len).map_err(|_| format!("buffer is too large for browser import: {len}"))
}

#[cfg(target_arch = "wasm32")]
fn configured_browser_string(
    label: &str,
    len_fn: unsafe extern "C" fn() -> i32,
    copy_fn: unsafe extern "C" fn(*mut u8, i32) -> i32,
) -> Result<String, String> {
    let len = unsafe { len_fn() };
    if len < 0 {
        return Err(format!("failed to read browser {label} length: {len}"));
    }
    if len == 0 {
        return Ok(String::new());
    }

    let mut bytes = vec![0; len as usize];
    let copied = unsafe { copy_fn(bytes.as_mut_ptr(), checked_js_len(bytes.len())?) };
    if copied < 0 {
        return Err(format!("failed to read browser {label}: {copied}"));
    }
    if copied as usize != bytes.len() {
        return Err(format!(
            "browser {label} length changed while reading: expected {}, got {copied}",
            bytes.len()
        ));
    }

    String::from_utf8(bytes).map_err(|err| format!("browser {label} is not valid UTF-8: {err}"))
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn configured_browser_network_config() -> Result<BrowserNetworkConfig, String> {
    let rtc_url = configured_browser_string("RTC URL", zub_rtc_url_len, zub_rtc_url)?;
    let login_name = configured_browser_string("login name", zub_login_name_len, zub_login_name)?;
    let password =
        configured_browser_string("login password", zub_login_password_len, zub_login_password)?;
    let player_identity = configured_browser_string(
        "player identity",
        zub_player_identity_len,
        zub_player_identity,
    )?;
    let arena_name = configured_browser_string("arena name", zub_arena_name_len, zub_arena_name)?;

    let mut config = BrowserNetworkConfig::from_values(
        rtc_url,
        login_name,
        password,
        player_identity,
        arena_name,
    );
    config.initial_ship = u8::try_from(unsafe { zub_initial_ship() })
        .ok()
        .filter(|ship| *ship < 8)
        .unwrap_or(0);
    Ok(config)
}

#[cfg(test)]
mod rtc_receive_tests {
    use super::*;
    use crate::{
        SNAPSHOT_FLAG_FULL, SnapshotEntity, SnapshotEntityState, SnapshotEvent, SnapshotFrame,
        SnapshotHeader, SnapshotLifecycleState, SnapshotPrizeState,
    };

    #[test]
    fn dense_game_snapshots_fit_the_browser_receive_contract() {
        for count in [15, 200, 255] {
            let frame = SnapshotFrame {
                header: SnapshotHeader {
                    flags: SNAPSHOT_FLAG_FULL,
                    snapshot_id: 1,
                    ..Default::default()
                },
                near_entities: (0..count)
                    .map(|id| {
                        SnapshotEntity::near_full(
                            id,
                            SnapshotEntityState {
                                life_seq: 1,
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
                mid_entities: vec![],
                events: vec![
                    SnapshotEvent::PrizeState(SnapshotPrizeState {
                        life_seq: 1,
                        revision: 1,
                        max_energy: 1700,
                        energy: Some(1700),
                        ..Default::default()
                    }),
                    SnapshotEvent::LifecycleState(SnapshotLifecycleState {
                        life_seq: 1,
                        alive: true,
                        ..Default::default()
                    }),
                ],
            };
            let bytes = frame.encode();
            assert!(bytes.len() > crate::ReliableSequencer::MAX_PACKET_SIZE);
            assert!(
                bytes.len() <= browser_rtc_packet_limit(PacketTransportLane::Game),
                "{count} actors need {} bytes",
                bytes.len()
            );
            assert_eq!(
                SnapshotFrame::parse(&bytes).unwrap().near_entities.len(),
                count as usize
            );
        }
    }
}

#[cfg(test)]
mod rtc_receive_queue_tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque};

    #[test]
    fn lane_boundaries_preserve_legacy_limit_and_admit_maximum_event() {
        assert_eq!(browser_rtc_packet_limit(PacketTransportLane::Reliable), 520);
        assert_eq!(browser_rtc_packet_limit(PacketTransportLane::Unknown), 520);
        assert!(crate::realtime_event::MAX_REALTIME_EVENT_PACKET_LEN > 520);
        assert!(
            crate::realtime_event::MAX_REALTIME_EVENT_PACKET_LEN
                < browser_rtc_packet_limit(PacketTransportLane::Game)
        );
        for lane in [
            PacketTransportLane::Game,
            PacketTransportLane::Event,
            PacketTransportLane::Reliable,
            PacketTransportLane::Unknown,
        ] {
            let limit = browser_rtc_packet_limit(lane);
            assert!(
                take_browser_rtc_packet(
                    limit,
                    lane,
                    |out| Ok(out.len()),
                    || panic!("legal message discarded")
                )
                .is_ok()
            );
            assert!(
                take_browser_rtc_packet(
                    limit + 1,
                    lane,
                    |_| panic!("rejected message allocated/read"),
                    || Ok(())
                )
                .is_err()
            );
        }
    }

    #[test]
    fn rejected_message_is_consumed_before_the_next_valid_lane_message() {
        let queue = RefCell::new(VecDeque::from([
            vec![0; browser_rtc_packet_limit(PacketTransportLane::Game) + 1],
            vec![0, 4, 1, 0, 0, 0],
        ]));
        let rejected_len = queue.borrow().front().unwrap().len();
        assert!(
            take_browser_rtc_packet(
                rejected_len,
                PacketTransportLane::Game,
                |_| panic!("oversized input must not allocate/copy"),
                || {
                    queue.borrow_mut().pop_front();
                    Ok(())
                }
            )
            .is_err()
        );
        assert_eq!(queue.borrow().len(), 1);
        let packet = take_browser_rtc_packet(
            6,
            PacketTransportLane::Reliable,
            |out| {
                let next = queue.borrow_mut().pop_front().unwrap();
                out.copy_from_slice(&next);
                Ok(next.len())
            },
            || panic!("valid successor discarded"),
        )
        .unwrap();
        assert_eq!(packet, [0, 4, 1, 0, 0, 0]);
        assert!(queue.borrow().is_empty());
    }
}
