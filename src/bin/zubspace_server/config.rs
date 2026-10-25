use std::net::{Ipv4Addr, SocketAddrV4};
use std::{env, fs};

use webrtc::peer_connection::RTCIceServer;

use super::limits::MAX_PLAYER_CLIENTS;
use super::zone::CHAOS_ZONE_ID;

const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:9001";
pub(crate) const DEFAULT_SERVER_ZONE: &str = CHAOS_ZONE_ID;
const DEFAULT_MAX_PLAYERS: usize = super::limits::MAX_ARENA_PLAYERS;
const DEFAULT_BOT_COUNT: usize = 0;
/// [REVIEW H10] The compiled fallback matches the shipped
/// `config/zubspace_server.toml` (pinned by
/// `shipped_config_matches_compiled_defaults`), so a bare
/// `zubspace_server` run behaves like the configured local/Docker
/// deployments instead of silently running a different cadence.
pub(crate) const DEFAULT_TICK_RATE_HZ: u16 = 60;
/// [REVIEW M17] Sanity ceiling for the simulation rate; values past this
/// are configuration mistakes (a 65535 Hz tick is a ~15 µs interval).
const MAX_TICK_RATE_HZ: u16 = 240;
const DEFAULT_SIGNALING_OFFER_RATE_PER_MINUTE: usize = 240;
const DEFAULT_SIGNALING_MAX_OFFER_BYTES: usize = 128 * 1024;
/// Lowest supported IPv4 path MTU. This matches Tailscale's common interface
/// MTU and leaves the rtc-dtls 1200-byte handshake default below the path cap.
const MIN_RTC_UDP_PATH_MTU: u16 = 1280;
/// Reserve IPv4 (20), UDP (8), DTLS 1.2 AES-GCM (37), plus 15 bytes of
/// operational margin before selecting the complete SCTP packet ceiling.
const RTC_UDP_OUTER_OVERHEAD_RESERVE: u32 = 80;
// Preserve the existing explicit path-MTU ceiling; without an override the
// upstream 0.21 stack uses its conservative 1191-byte IPv6/TURN budget.
const MAX_CONFIGURED_SCTP_PACKET_SIZE: u32 = 1228;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RtcUdpPortRange {
    pub(crate) min: u16,
    pub(crate) max: u16,
}

impl RtcUdpPortRange {
    pub(crate) fn capacity(self) -> usize {
        usize::from(self.max) - usize::from(self.min) + 1
    }

    pub(crate) fn ports(self) -> impl Iterator<Item = u16> {
        self.min..=self.max
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RtcIceTransportPolicy {
    #[default]
    All,
    Relay,
}

impl RtcIceTransportPolicy {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Relay => "relay",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServerLogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Off,
}

impl ServerLogLevel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
            Self::Off => "off",
        }
    }
}

impl Default for ServerLogLevel {
    fn default() -> Self {
        Self::Info
    }
}

/// [REVIEW M17] Where the effective bind address came from, surfaced in
/// the startup log so a silently-overriding positional argument (or env
/// var) is visible to the operator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum BindSource {
    #[default]
    Default,
    ConfigFile,
    Env,
    CliFlag,
    Positional,
}

impl BindSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::ConfigFile => "config-file",
            Self::Env => "env",
            Self::CliFlag => "--bind",
            Self::Positional => "positional-arg",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerConfig {
    pub(crate) listen_addr: String,
    pub(crate) listen_addr_source: BindSource,
    pub(crate) default_zone: String,
    pub(crate) zone_bundle_root: Option<String>,
    pub(crate) allow_zone_overrides: bool,
    pub(crate) max_players: usize,
    pub(crate) bot_count: usize,
    /// Product spawn policy, independent of displayed bounty or prize weights.
    pub(crate) maxed_spawn_loadout: bool,
    /// Validated zero-based ship indices; external settings use ship numbers 1–8.
    pub(crate) bot_ship_types: Vec<u8>,
    pub(crate) tick_rate_hz: u16,
    pub(crate) log_level: ServerLogLevel,
    /// Per-target prefix overrides; log_level remains the fallback.
    pub(crate) log_filter: String,
    pub(crate) log_file: Option<String>,
    pub(crate) log_json: bool,
    pub(crate) metrics_listen: Option<std::net::SocketAddr>,
    pub(crate) connection_quality: crate::connection_quality::ConnectionQualityPolicy,
    pub(crate) sim_observe: bool,
    /// Product gate for flag and powerball objectives. False for this
    /// iteration: the authoritative objective profile stays disabled even for a
    /// zone that authors an `[Objective]` section, so the shipped game runs no
    /// flag or powerball game. QA fixtures and the future feature opt in
    /// explicitly.
    pub(crate) enable_objectives: bool,
    /// Release policy: distinct server-assigned teams; no client team changes.
    /// Team-mode protocol fixtures opt out explicitly; release binaries expose no switch.
    pub(crate) free_for_all: bool,
    /// Trusted operator policy for the server-directed observer timeline.
    /// Browser URLs and combat-view receipts cannot select or widen it.
    pub(crate) observer_network_profile: crate::combat_timeline::ObserverNetworkProfile,
    /// Enables the bounded QA trace HTTP upload/retrieval surface. Trace
    /// collection still requires browser `trace=1`; the default exposes no
    /// diagnostic mutation endpoint.
    pub(crate) qa_trace_upload: bool,
    /// Opt-in bounded client presentation and server near-hull diagnostics;
    /// never gameplay input.
    pub(crate) client_debug_trace: bool,
    pub(crate) movement_snapback: bool,
    /// Debug-build-only one-shot protocol fault used by the real-browser
    /// correction loss/recovery gate. Release binaries never read its env.
    pub(crate) movement_snapback_test_force_once: bool,
    pub(crate) simulated_tx_loss: crate::simulated_loss::SimulatedTxLossSettings,
    /// QA-only loss can begin after the reliable join fence and complete
    /// map/LVZ transfer are acknowledged, so a gameplay-loss replay exercises
    /// live traffic instead of spending minutes recovering bootstrap chunks.
    pub(crate) simulated_tx_loss_after_bootstrap: bool,
    /// Optional concrete local IPv4 interface used for server ICE sockets.
    /// `None` preserves the legacy wildcard bind used by localhost, container,
    /// and STUN/TURN deployments.
    pub(crate) rtc_udp_bind_ip: Option<Ipv4Addr>,
    /// Optional end-to-end IPv4/UDP path MTU. The server converts this into a
    /// per-SCTP-packet ceiling while preserving full data-channel message size.
    pub(crate) rtc_udp_path_mtu: Option<u16>,
    pub(crate) rtc_udp_port_range: Option<RtcUdpPortRange>,
    pub(crate) rtc_external_ips: Vec<String>,
    pub(crate) rtc_ice_urls: Vec<String>,
    pub(crate) rtc_ice_username: String,
    pub(crate) rtc_ice_credential: String,
    pub(crate) rtc_ice_transport_policy: RtcIceTransportPolicy,
    /// When realtime event messages also go on the game channel.
    pub(crate) realtime_event_game_copy: crate::rtc::RealtimeEventGameCopy,
    /// SCTP SACKs made due by a second packet wait up to 20 ms for outgoing
    /// DATA to carry them, instead of a datagram of their own.
    pub(crate) rtc_defer_sack_to_data: bool,
    /// Per-client snapshot and realtime event budget (token bucket rate and
    /// burst), in KiB/s.
    pub(crate) send_budget_kib_per_second: u32,
    /// How long a WebRTC driver waits after a write wake before flushing, so
    /// one tick's messages on several channels share datagrams. 0 flushes at
    /// once.
    pub(crate) rtc_write_coalesce_micros: u32,
    pub(crate) signaling_allowed_origins: Vec<String>,
    pub(crate) signaling_auth_token: Option<String>,
    pub(crate) signaling_require_https: bool,
    pub(crate) signaling_trust_proxy_headers: bool,
    pub(crate) signaling_offer_rate_per_minute: usize,
    pub(crate) signaling_max_offer_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: DEFAULT_LISTEN_ADDR.to_string(),
            listen_addr_source: BindSource::Default,
            default_zone: DEFAULT_SERVER_ZONE.to_string(),
            zone_bundle_root: None,
            allow_zone_overrides: false,
            max_players: DEFAULT_MAX_PLAYERS,
            bot_count: DEFAULT_BOT_COUNT,
            maxed_spawn_loadout: true,
            bot_ship_types: vec![0],
            tick_rate_hz: DEFAULT_TICK_RATE_HZ,
            log_level: ServerLogLevel::default(),
            log_filter: String::new(),
            log_file: None,
            log_json: false,
            metrics_listen: None,
            connection_quality: crate::connection_quality::ConnectionQualityPolicy::default(),
            sim_observe: false,
            enable_objectives: false,
            free_for_all: true,
            observer_network_profile: crate::combat_timeline::ObserverNetworkProfile::AutomaticWan,
            qa_trace_upload: false,
            client_debug_trace: false,
            movement_snapback: true,
            movement_snapback_test_force_once: false,
            simulated_tx_loss: crate::simulated_loss::SimulatedTxLossSettings::default(),
            simulated_tx_loss_after_bootstrap: false,
            rtc_udp_bind_ip: None,
            rtc_udp_path_mtu: None,
            rtc_udp_port_range: None,
            rtc_external_ips: Vec::new(),
            rtc_ice_urls: Vec::new(),
            rtc_ice_username: String::new(),
            rtc_ice_credential: String::new(),
            rtc_ice_transport_policy: RtcIceTransportPolicy::All,
            realtime_event_game_copy: crate::rtc::RealtimeEventGameCopy::Always,
            rtc_defer_sack_to_data: true,
            send_budget_kib_per_second: crate::state::DEFAULT_SEND_BUDGET_KIB_PER_SECOND,
            rtc_write_coalesce_micros: 0,
            signaling_allowed_origins: vec!["*".to_string()],
            signaling_auth_token: None,
            signaling_require_https: false,
            signaling_trust_proxy_headers: false,
            signaling_offer_rate_per_minute: DEFAULT_SIGNALING_OFFER_RATE_PER_MINUTE,
            signaling_max_offer_bytes: DEFAULT_SIGNALING_MAX_OFFER_BYTES,
        }
    }
}

impl ServerConfig {
    pub(crate) fn from_env_args<I>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = String>,
    {
        let args = args.into_iter().collect::<Vec<_>>();
        let config_path = config_path_from_args(&args)?.or_else(|| {
            env::var("ZUBSPACE_SERVER_CONFIG")
                .ok()
                .filter(|path| !path.is_empty())
        });
        let mut config = Self::default();
        if let Some(path) = config_path {
            config.apply_file(&path)?;
        }
        config.apply_env()?;

        let mut args = args.into_iter();
        let _program = args.next();
        let mut positional_bind_seen = false;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--config" => {
                    let _ = next_arg_value(&mut args, "--config")?;
                }
                "--bind" => {
                    config.listen_addr = next_arg_value(&mut args, "--bind")?;
                    config.listen_addr_source = BindSource::CliFlag;
                }
                "--zone" => config.default_zone = next_arg_value(&mut args, "--zone")?,
                "--zone-root" => {
                    config.zone_bundle_root =
                        normalize_optional_path(next_arg_value(&mut args, "--zone-root")?)
                }
                "--allow-zone-overrides" => config.allow_zone_overrides = true,
                "--max-players" => {
                    config.max_players =
                        parse_positive_usize(&next_arg_value(&mut args, "--max-players")?)?
                }
                "--maxed-spawn-loadout" => {
                    config.maxed_spawn_loadout =
                        parse_env_bool(&next_arg_value(&mut args, "--maxed-spawn-loadout")?)?;
                }
                "--bot-count" => {
                    config.bot_count = parse_usize(&next_arg_value(&mut args, "--bot-count")?)?
                }
                "--bot-ship-types" => {
                    config.bot_ship_types =
                        parse_bot_ship_types(&next_arg_value(&mut args, "--bot-ship-types")?)?
                }
                "--tick-rate" => {
                    config.tick_rate_hz =
                        parse_positive_u16(&next_arg_value(&mut args, "--tick-rate")?)?
                }
                "--log-level" => {
                    config.log_level = parse_log_level(&next_arg_value(&mut args, "--log-level")?)?
                }
                "--log-filter" => {
                    config.log_filter =
                        parse_log_filter(&next_arg_value(&mut args, "--log-filter")?)?
                }
                "--log-file" => {
                    config.log_file =
                        normalize_optional_log_file(next_arg_value(&mut args, "--log-file")?)
                }
                "--log-format" => {
                    config.log_json = parse_log_format(&next_arg_value(&mut args, "--log-format")?)?
                }
                "--metrics-listen" => {
                    config.metrics_listen =
                        parse_metrics_listen(&next_arg_value(&mut args, "--metrics-listen")?)?
                }
                "--sim-observe" => config.sim_observe = true,
                "--enable-objectives" => config.enable_objectives = true,
                "--observer-network-profile" => {
                    config.observer_network_profile = parse_observer_network_profile(
                        &next_arg_value(&mut args, "--observer-network-profile")?,
                    )?
                }
                "--qa-trace-upload" => config.qa_trace_upload = true,
                "--client-debug-trace" => config.client_debug_trace = true,
                "--movement-snapback" => config.movement_snapback = true,
                "--no-movement-snapback" => config.movement_snapback = false,
                "--tx-loss-pct" => {
                    let probability =
                        parse_loss_probability(&next_arg_value(&mut args, "--tx-loss-pct")?)?;
                    config.simulated_tx_loss = config
                        .simulated_tx_loss
                        .with_probability_millionths(probability);
                }
                "--tx-loss-seed" => {
                    let seed = parse_u64(&next_arg_value(&mut args, "--tx-loss-seed")?)?;
                    config.simulated_tx_loss = config.simulated_tx_loss.with_seed(Some(seed));
                }
                "--rtc-udp-port-range" => {
                    config.rtc_udp_port_range = parse_optional_udp_port_range(&next_arg_value(
                        &mut args,
                        "--rtc-udp-port-range",
                    )?)?
                }
                "--rtc-udp-bind-ip" => {
                    config.rtc_udp_bind_ip = parse_optional_rtc_udp_bind_ip(&next_arg_value(
                        &mut args,
                        "--rtc-udp-bind-ip",
                    )?)?
                }
                "--rtc-udp-path-mtu" => {
                    config.rtc_udp_path_mtu = parse_optional_rtc_udp_path_mtu(&next_arg_value(
                        &mut args,
                        "--rtc-udp-path-mtu",
                    )?)?
                }
                "--rtc-external-ips" => {
                    config.rtc_external_ips =
                        parse_csv(&next_arg_value(&mut args, "--rtc-external-ips")?)
                }
                "--rtc-ice-urls" => {
                    config.rtc_ice_urls = parse_csv(&next_arg_value(&mut args, "--rtc-ice-urls")?)
                }
                "--rtc-ice-username" => {
                    config.rtc_ice_username = next_arg_value(&mut args, "--rtc-ice-username")?
                }
                "--rtc-ice-credential" => {
                    config.rtc_ice_credential = next_arg_value(&mut args, "--rtc-ice-credential")?
                }
                "--rtc-ice-policy" => {
                    config.rtc_ice_transport_policy =
                        parse_ice_transport_policy(&next_arg_value(&mut args, "--rtc-ice-policy")?)?
                }
                "--realtime-event-game-copy" => {
                    config.realtime_event_game_copy = crate::rtc::RealtimeEventGameCopy::parse(
                        &next_arg_value(&mut args, "--realtime-event-game-copy")?,
                    )?
                }
                "--signaling-allowed-origins" => {
                    config.signaling_allowed_origins =
                        parse_csv(&next_arg_value(&mut args, "--signaling-allowed-origins")?)
                }
                "--signaling-auth-token" => {
                    config.signaling_auth_token = normalize_optional_secret(next_arg_value(
                        &mut args,
                        "--signaling-auth-token",
                    )?)
                }
                "--signaling-require-https" => config.signaling_require_https = true,
                "--signaling-trust-proxy-headers" => config.signaling_trust_proxy_headers = true,
                "--signaling-offer-rate" => {
                    config.signaling_offer_rate_per_minute =
                        parse_positive_usize(&next_arg_value(&mut args, "--signaling-offer-rate")?)?
                }
                "--signaling-max-offer-bytes" => {
                    config.signaling_max_offer_bytes = parse_positive_usize(&next_arg_value(
                        &mut args,
                        "--signaling-max-offer-bytes",
                    )?)?
                }
                "--help" | "-h" => return Err(server_usage()),
                value if value.starts_with("--") => {
                    return Err(format!(
                        "unknown server option: {value}\n{}",
                        server_usage()
                    ));
                }
                value if !positional_bind_seen => {
                    config.listen_addr = value.to_string();
                    config.listen_addr_source = BindSource::Positional;
                    positional_bind_seen = true;
                }
                value => {
                    return Err(format!(
                        "unexpected server argument: {value}\n{}",
                        server_usage()
                    ));
                }
            }
        }

        config.validate()?;
        Ok(config)
    }

    pub(crate) fn apply_file(&mut self, path: &str) -> Result<(), String> {
        let text = fs::read_to_string(path)
            .map_err(|err| format!("failed to read server config {path:?}: {err}"))?;
        self.apply_toml_like_config(&text, path)
    }

    fn apply_toml_like_config(&mut self, text: &str, source: &str) -> Result<(), String> {
        let mut section = String::new();
        for (line_index, raw_line) in text.lines().enumerate() {
            let line_number = line_index + 1;
            let line = strip_toml_comment(raw_line).trim().to_string();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_ascii_lowercase();
                if section != "server" {
                    return Err(format!(
                        "{source}:{line_number}: unsupported config section [{section}]"
                    ));
                }
                continue;
            }
            if !section.is_empty() && section != "server" {
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("{source}:{line_number}: expected key = value"));
            };
            let key = key.trim().replace('-', "_").to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "quality_max_rtt_ms"
                | "quality_max_loss_per_mille"
                | "quality_recovery_rtt_ms"
                | "quality_recovery_loss_per_mille" => {
                    self.connection_quality
                        .set(&key, &parse_toml_integer(value, source, line_number)?)?;
                }
                "bind" | "listen_addr" => {
                    self.listen_addr = parse_toml_string(value, source, line_number)?;
                    self.listen_addr_source = BindSource::ConfigFile;
                }
                "zone" | "default_zone" => {
                    self.default_zone = parse_toml_string(value, source, line_number)?
                }
                "zone_bundle_root" | "zone_root" => {
                    self.zone_bundle_root =
                        normalize_optional_path(parse_toml_string(value, source, line_number)?)
                }
                "allow_zone_overrides" => {
                    self.allow_zone_overrides = parse_toml_bool(value, source, line_number)?
                }
                "max_players" => {
                    self.max_players =
                        parse_positive_usize(&parse_toml_integer(value, source, line_number)?)?
                }
                "maxed_spawn_loadout" => {
                    self.maxed_spawn_loadout = parse_toml_bool(value, source, line_number)?;
                }
                "bot_count" => {
                    self.bot_count = parse_usize(&parse_toml_integer(value, source, line_number)?)?
                }
                "bot_ship_types" => {
                    self.bot_ship_types =
                        parse_bot_ship_types(&parse_toml_string(value, source, line_number)?)?
                }
                "tick_rate" | "tick_rate_hz" => {
                    self.tick_rate_hz =
                        parse_positive_u16(&parse_toml_integer(value, source, line_number)?)?
                }
                "log_level" | "log" => {
                    self.log_level =
                        parse_log_level(&parse_toml_string(value, source, line_number)?)?
                }
                "log_file" | "log_path" | "log_output_file" => {
                    self.log_file =
                        normalize_optional_log_file(parse_toml_string(value, source, line_number)?)
                }
                "log_filter" => {
                    self.log_filter =
                        parse_log_filter(&parse_toml_string(value, source, line_number)?)?
                }
                "log_format" => {
                    self.log_json =
                        parse_log_format(&parse_toml_string(value, source, line_number)?)?
                }
                "metrics_listen" => {
                    self.metrics_listen =
                        parse_metrics_listen(&parse_toml_string(value, source, line_number)?)?
                }
                "sim_observe" => self.sim_observe = parse_toml_bool(value, source, line_number)?,
                "enable_objectives" => {
                    self.enable_objectives = parse_toml_bool(value, source, line_number)?
                }
                "observer_network_profile" => {
                    self.observer_network_profile = parse_observer_network_profile(
                        &parse_toml_string(value, source, line_number)?,
                    )
                    .map_err(|err| format!("{source}:{line_number}: {err}"))?
                }
                "qa_trace_upload" => {
                    self.qa_trace_upload = parse_toml_bool(value, source, line_number)?
                }
                "client_debug_trace" => {
                    self.client_debug_trace = parse_toml_bool(value, source, line_number)?
                }
                "movement_snapback" => {
                    self.movement_snapback = parse_toml_bool(value, source, line_number)?
                }
                "tx_loss_pct" | "simulated_tx_loss_pct" => {
                    let probability = parse_loss_probability(value).map_err(|err| {
                        format!("{source}:{line_number}: invalid tx loss probability: {err}")
                    })?;
                    self.simulated_tx_loss = self
                        .simulated_tx_loss
                        .with_probability_millionths(probability);
                }
                "tx_loss_seed" | "simulated_tx_loss_seed" => {
                    let seed = parse_u64(&parse_toml_integer(value, source, line_number)?)?;
                    self.simulated_tx_loss = self.simulated_tx_loss.with_seed(Some(seed));
                }
                "rtc_udp_port_range" | "rtc_udp_ports" => {
                    self.rtc_udp_port_range = parse_optional_udp_port_range(&parse_toml_string(
                        value,
                        source,
                        line_number,
                    )?)?
                }
                "rtc_udp_bind_ip" => {
                    self.rtc_udp_bind_ip = parse_optional_rtc_udp_bind_ip(&parse_toml_string(
                        value,
                        source,
                        line_number,
                    )?)?
                }
                "rtc_udp_path_mtu" => {
                    self.rtc_udp_path_mtu = parse_optional_rtc_udp_path_mtu(&parse_toml_integer(
                        value,
                        source,
                        line_number,
                    )?)?
                }
                "rtc_external_ips" => {
                    self.rtc_external_ips =
                        parse_csv(&parse_toml_string(value, source, line_number)?)
                }
                "rtc_ice_urls" => {
                    self.rtc_ice_urls = parse_csv(&parse_toml_string(value, source, line_number)?)
                }
                "rtc_ice_username" => {
                    self.rtc_ice_username = parse_toml_string(value, source, line_number)?
                }
                "rtc_ice_credential" => {
                    self.rtc_ice_credential = parse_toml_string(value, source, line_number)?
                }
                "rtc_ice_policy" => {
                    self.rtc_ice_transport_policy =
                        parse_ice_transport_policy(&parse_toml_string(value, source, line_number)?)?
                }
                "rtc_defer_sack_to_data" => {
                    self.rtc_defer_sack_to_data = parse_toml_bool(value, source, line_number)?
                }
                "rtc_write_coalesce_micros" => {
                    self.rtc_write_coalesce_micros =
                        parse_write_coalescing(&parse_toml_integer(value, source, line_number)?)
                            .map_err(|err| format!("{source}:{line_number}: {err}"))?
                }
                "send_budget_kib_per_second" => {
                    self.send_budget_kib_per_second =
                        parse_send_budget(&parse_toml_integer(value, source, line_number)?)
                            .map_err(|err| format!("{source}:{line_number}: {err}"))?
                }
                "realtime_event_game_copy" => {
                    self.realtime_event_game_copy = crate::rtc::RealtimeEventGameCopy::parse(
                        &parse_toml_string(value, source, line_number)?,
                    )
                    .map_err(|err| format!("{source}:{line_number}: {err}"))?
                }
                "signaling_allowed_origins" => {
                    self.signaling_allowed_origins =
                        parse_csv(&parse_toml_string(value, source, line_number)?)
                }
                "signaling_auth_token" => {
                    self.signaling_auth_token =
                        normalize_optional_secret(parse_toml_string(value, source, line_number)?)
                }
                "signaling_require_https" => {
                    self.signaling_require_https = parse_toml_bool(value, source, line_number)?
                }
                "signaling_trust_proxy_headers" => {
                    self.signaling_trust_proxy_headers =
                        parse_toml_bool(value, source, line_number)?
                }
                "signaling_offer_rate_per_minute" | "signaling_offer_rate" => {
                    self.signaling_offer_rate_per_minute =
                        parse_positive_usize(&parse_toml_integer(value, source, line_number)?)?
                }
                "signaling_max_offer_bytes" => {
                    self.signaling_max_offer_bytes =
                        parse_positive_usize(&parse_toml_integer(value, source, line_number)?)?
                }
                _ => {
                    return Err(format!(
                        "{source}:{line_number}: unsupported server config key {key:?}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn apply_env(&mut self) -> Result<(), String> {
        for (name, key) in [
            ("ZUBSPACE_SERVER_QUALITY_MAX_RTT_MS", "quality_max_rtt_ms"),
            (
                "ZUBSPACE_SERVER_QUALITY_MAX_LOSS_PER_MILLE",
                "quality_max_loss_per_mille",
            ),
            (
                "ZUBSPACE_SERVER_QUALITY_RECOVERY_RTT_MS",
                "quality_recovery_rtt_ms",
            ),
            (
                "ZUBSPACE_SERVER_QUALITY_RECOVERY_LOSS_PER_MILLE",
                "quality_recovery_loss_per_mille",
            ),
        ] {
            if let Ok(value) = env::var(name) {
                self.connection_quality.set(key, &value)?;
            }
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_BIND") {
            self.listen_addr = value;
            self.listen_addr_source = BindSource::Env;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_ZONE") {
            self.default_zone = value;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_ZONE_ROOT") {
            self.zone_bundle_root = normalize_optional_path(value);
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES") {
            self.allow_zone_overrides = parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_MAX_PLAYERS") {
            self.max_players = parse_positive_usize(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT") {
            self.maxed_spawn_loadout = parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_BOT_COUNT") {
            self.bot_count = parse_usize(&value)?;
        }
        if let Ok(value) =
            env::var("ZUBSPACE_SERVER_BOT_SHIP_TYPES").or_else(|_| env::var("BOT_SHIP_TYPES"))
        {
            self.bot_ship_types = parse_bot_ship_types(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_TICK_RATE") {
            self.tick_rate_hz = parse_positive_u16(&value)?;
        }
        if let Ok(value) =
            env::var("ZUBSPACE_SERVER_LOG_LEVEL").or_else(|_| env::var("ZUBSPACE_SERVER_LOG"))
        {
            self.log_level = parse_log_level(&value)?;
        }
        if let Ok(value) =
            env::var("ZUBSPACE_SERVER_LOG_FILE").or_else(|_| env::var("ZUBSPACE_SERVER_LOG_PATH"))
        {
            self.log_file = normalize_optional_log_file(value);
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_LOG_FORMAT") {
            self.log_json = parse_log_format(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_LOG_FILTER") {
            self.log_filter = parse_log_filter(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_METRICS_LISTEN") {
            self.metrics_listen = parse_metrics_listen(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_SIM_OBSERVE") {
            self.sim_observe = parse_env_bool(&value)?;
        }
        // Historical recovery/prize-sharing QA needs the retained team engine.
        // This operator-only fixture is compiled out of the release server.
        #[cfg(debug_assertions)]
        if let Ok(value) = env::var("ZUBSPACE_SERVER_TEST_TEAM_MODE") {
            self.free_for_all = !parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_ENABLE_OBJECTIVES") {
            self.enable_objectives = parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_OBSERVER_NETWORK_PROFILE") {
            self.observer_network_profile = parse_observer_network_profile(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_QA_TRACE_UPLOAD") {
            self.qa_trace_upload = parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_CLIENT_DEBUG_TRACE") {
            self.client_debug_trace = parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_MOVEMENT_SNAPBACK") {
            self.movement_snapback = parse_env_bool(&value)?;
        }
        #[cfg(debug_assertions)]
        if let Ok(value) = env::var("ZUBSPACE_SERVER_TEST_FORCE_MOVEMENT_SNAPBACK_ONCE") {
            self.movement_snapback_test_force_once = parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_TX_LOSS_PCT") {
            let probability = parse_loss_probability(&value)?;
            self.simulated_tx_loss = self
                .simulated_tx_loss
                .with_probability_millionths(probability);
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_TX_LOSS_SEED") {
            self.simulated_tx_loss = self.simulated_tx_loss.with_seed(Some(parse_u64(&value)?));
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_TX_LOSS_AFTER_BOOTSTRAP") {
            self.simulated_tx_loss_after_bootstrap = parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_UDP_PORT_RANGE") {
            self.rtc_udp_port_range = parse_optional_udp_port_range(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_RTC_UDP_BIND_IP") {
            self.rtc_udp_bind_ip = parse_optional_rtc_udp_bind_ip(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_RTC_UDP_PATH_MTU") {
            self.rtc_udp_path_mtu = parse_optional_rtc_udp_path_mtu(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_EXTERNAL_IP") {
            self.rtc_external_ips = parse_csv(&value);
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_RTC_ICE_URLS") {
            self.rtc_ice_urls = parse_csv(&value);
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_RTC_ICE_USERNAME") {
            self.rtc_ice_username = value;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_RTC_ICE_CREDENTIAL") {
            self.rtc_ice_credential = value;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_RTC_ICE_POLICY") {
            self.rtc_ice_transport_policy = parse_ice_transport_policy(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_RTC_WRITE_COALESCE_MICROS") {
            self.rtc_write_coalesce_micros = parse_write_coalescing(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_SEND_BUDGET_KIB_PER_SECOND") {
            self.send_budget_kib_per_second = parse_send_budget(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_RTC_DEFER_SACK_TO_DATA") {
            self.rtc_defer_sack_to_data = parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_REALTIME_EVENT_GAME_COPY") {
            self.realtime_event_game_copy = crate::rtc::RealtimeEventGameCopy::parse(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_SIGNALING_ALLOWED_ORIGINS") {
            self.signaling_allowed_origins = parse_csv(&value);
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_SIGNALING_AUTH_TOKEN") {
            self.signaling_auth_token = normalize_optional_secret(value);
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_SIGNALING_REQUIRE_HTTPS") {
            self.signaling_require_https = parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_SIGNALING_TRUST_PROXY_HEADERS") {
            self.signaling_trust_proxy_headers = parse_env_bool(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_SIGNALING_OFFER_RATE") {
            self.signaling_offer_rate_per_minute = parse_positive_usize(&value)?;
        }
        if let Ok(value) = env::var("ZUBSPACE_SERVER_SIGNALING_MAX_OFFER_BYTES") {
            self.signaling_max_offer_bytes = parse_positive_usize(&value)?;
        }
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        self.connection_quality.validate()?;
        if self.listen_addr.trim().is_empty() {
            return Err("server bind address cannot be empty".to_string());
        }
        if self.max_players == 0 {
            return Err("max players must be greater than zero".to_string());
        }
        if self.max_players > MAX_PLAYER_CLIENTS {
            return Err(format!(
                "max players cannot exceed the {MAX_PLAYER_CLIENTS} unique player client IDs, got {}",
                self.max_players
            ));
        }
        if self.tick_rate_hz == 0 || self.tick_rate_hz > MAX_TICK_RATE_HZ {
            return Err(format!(
                "tick rate must be between 1 and {MAX_TICK_RATE_HZ} Hz, got {}",
                self.tick_rate_hz
            ));
        }
        if let Some(range) = self.rtc_udp_port_range {
            if range.capacity() < self.max_players {
                return Err(format!(
                    "RTC UDP port range {}-{} provides {} one-port-per-peer sockets, but max_players is {}; widen ZUBSPACE_SERVER_UDP_PORT_RANGE/rtc_udp_port_range or lower max_players",
                    range.min,
                    range.max,
                    range.capacity(),
                    self.max_players
                ));
            }
        }
        if let Some(address) = self.rtc_udp_bind_ip {
            validate_local_rtc_udp_bind_ip(address)?;
        }
        if !self.rtc_external_ips.is_empty() {
            return Err(
                "rtc_external_ips/ZUBSPACE_SERVER_EXTERNAL_IP is unsupported: webrtc-rs 0.21 does not apply the configured host-candidate remapping; configure a supported stun: or turn: URL instead and verify the resulting network path"
                    .to_string(),
            );
        }
        let mut requests_turn = false;
        for url in &self.rtc_ice_urls {
            requests_turn |= validate_canonical_ice_url(url)?;
        }
        if requests_turn
            && (self.rtc_ice_username.trim().is_empty()
                || self.rtc_ice_credential.trim().is_empty())
        {
            return Err(
                "TURN ICE URLs require both rtc_ice_username and rtc_ice_credential".to_string(),
            );
        }
        let parsed_ice_urls = RTCIceServer {
            urls: self.rtc_ice_urls.clone(),
            username: self.rtc_ice_username.clone(),
            credential: self.rtc_ice_credential.clone(),
        }
        .urls()
        .map_err(|err| format!("invalid RTC ICE URL configuration: {err}"))?;
        let mut has_turn = false;
        for url in parsed_ice_urls {
            match url.scheme.to_string().as_str() {
                "stun" => {}
                "turn" => {
                    has_turn = true;
                    if url.proto.to_string() != "udp" {
                        return Err(format!(
                            "unsupported RTC ICE URL {url}: webrtc-rs 0.21 server gathering supports TURN over UDP only"
                        ));
                    }
                }
                "stuns" | "turns" => {
                    return Err(format!(
                        "unsupported RTC ICE URL {url}: webrtc-rs 0.21 server gathering does not support secure STUN/TURN URLs"
                    ));
                }
                scheme => {
                    return Err(format!(
                        "unsupported RTC ICE URL scheme {scheme:?}; expected stun: or turn:"
                    ));
                }
            }
        }
        if self.rtc_ice_transport_policy == RtcIceTransportPolicy::Relay && !has_turn {
            return Err("rtc_ice_policy=relay requires at least one turn: URL".to_string());
        }
        if self.signaling_require_https && !self.signaling_trust_proxy_headers {
            return Err(
                "signaling_require_https=true requires signaling_trust_proxy_headers=true: zubspace accepts plaintext HTTP and can verify TLS only from a trusted reverse proxy's X-Forwarded-Proto header"
                    .to_string(),
            );
        }
        if self.signaling_allowed_origins.is_empty() {
            return Err(
                "signaling_allowed_origins must contain '*' or at least one exact HTTP(S) origin"
                    .to_string(),
            );
        }
        if self.signaling_allowed_origins.len() > 1
            && self
                .signaling_allowed_origins
                .iter()
                .any(|origin| origin == "*")
        {
            return Err(
                "signaling_allowed_origins '*' cannot be combined with exact origins".to_string(),
            );
        }
        for origin in &self.signaling_allowed_origins {
            if origin != "*" {
                validate_canonical_signaling_origin(origin)?;
            }
        }
        if self.signaling_offer_rate_per_minute == 0 {
            return Err("signaling offer rate must be greater than zero".to_string());
        }
        if self.signaling_max_offer_bytes == 0 || self.signaling_max_offer_bytes > 1024 * 1024 {
            return Err(format!(
                "signaling max offer bytes must be between 1 and 1048576, got {}",
                self.signaling_max_offer_bytes
            ));
        }
        Ok(())
    }

    pub(crate) fn rtc_udp_bind_address(&self, port: u16) -> SocketAddrV4 {
        SocketAddrV4::new(self.rtc_udp_bind_ip.unwrap_or(Ipv4Addr::UNSPECIFIED), port)
    }

    /// Maximum complete SCTP packet before DTLS. rtc-sctp fragments larger
    /// messages at this boundary, so message/channel capacity is unchanged.
    pub(crate) fn rtc_sctp_max_packet_size(&self) -> Option<u32> {
        self.rtc_udp_path_mtu.map(|path_mtu| {
            (u32::from(path_mtu) - RTC_UDP_OUTER_OVERHEAD_RESERVE)
                .min(MAX_CONFIGURED_SCTP_PACKET_SIZE)
        })
    }

    /// Validate the selected zone against the exact, descriptor-stable catalog
    /// that will be installed in server state. Catalog construction is kept out
    /// of argument parsing so disk bundles are never scanned twice.
    pub(crate) fn validate_zone_ids<'a>(
        &self,
        zone_ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), String> {
        let configured_zones = zone_ids.into_iter().map(str::to_string).collect::<Vec<_>>();
        if configured_zones
            .iter()
            .any(|zone| zone == &self.default_zone)
        {
            return Ok(());
        }
        Err(format!(
            "unsupported server zone {:?}; validated server zones: {}",
            self.default_zone,
            configured_zones.join(", ")
        ))
    }
}

fn validate_canonical_ice_url(value: &str) -> Result<bool, String> {
    const TURN_QUERY: &str = "?transport=udp";

    let (is_turn, authority) = if let Some(authority) = value.strip_prefix("stun:") {
        if authority.contains('?') {
            return Err(format!(
                "unsupported RTC ICE URL {value:?}; the canonical STUN form is stun:host:port with no query"
            ));
        }
        (false, authority)
    } else if let Some(rest) = value.strip_prefix("turn:") {
        let Some(authority) = rest.strip_suffix(TURN_QUERY) else {
            return Err(format!(
                "unsupported RTC ICE URL {value:?}; the canonical TURN form is turn:host:port?transport=udp"
            ));
        };
        (true, authority)
    } else {
        return Err(format!(
            "unsupported RTC ICE URL {value:?}; expected canonical lowercase stun:host:port or turn:host:port?transport=udp"
        ));
    };

    let (host, port_text) = split_canonical_ice_authority(authority).map_err(|reason| {
        format!("ICE URL must use canonical host:port authority in {value:?}: {reason}")
    })?;
    validate_canonical_ice_host(host)
        .map_err(|reason| format!("ICE URL host is not canonical in {value:?}: {reason}"))?;

    if port_text.is_empty() || !port_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "ICE URL port must be a canonical decimal integer from 1 through 65535 in {value:?}"
        ));
    }
    let port = port_text
        .parse::<u16>()
        .map_err(|_| format!("ICE URL port must be between 1 and 65535 in {value:?}"))?;
    if port == 0 || port_text != port.to_string() {
        return Err(format!(
            "ICE URL port must be a canonical decimal integer from 1 through 65535 in {value:?}"
        ));
    }

    Ok(is_turn)
}

fn split_canonical_ice_authority(authority: &str) -> Result<(&str, &str), &'static str> {
    if authority.starts_with('[') {
        let close = authority
            .find(']')
            .ok_or("bracketed IPv6 host is missing its closing bracket")?;
        let host = &authority[..=close];
        let remainder = &authority[close + 1..];
        let port = remainder
            .strip_prefix(':')
            .ok_or("bracketed IPv6 host must be followed by :port")?;
        if port.contains(':') || port.contains(']') {
            return Err("unexpected characters follow the bracketed IPv6 host");
        }
        return Ok((host, port));
    }

    let (host, port) = authority
        .rsplit_once(':')
        .ok_or("host must be followed by an explicit :port")?;
    if host.is_empty() || host.contains(':') {
        return Err("IPv6 hosts must use canonical square brackets");
    }
    Ok((host, port))
}

fn validate_canonical_ice_host(host: &str) -> Result<(), &'static str> {
    if host.starts_with('[') || host.ends_with(']') {
        return Err("IPv6 ICE literals are unsupported while server peer sockets bind only IPv4");
    }

    if let Ok(address) = host.parse::<Ipv4Addr>() {
        if host != address.to_string() {
            return Err("IPv4 hosts must use canonical dotted decimal form");
        }
        return Ok(());
    }
    if looks_like_noncanonical_ipv4(host) {
        return Err("numeric IPv4 hosts must use four canonical dotted-decimal octets");
    }

    if host.is_empty() || host.len() > 253 {
        return Err("DNS hosts must contain 1 through 253 ASCII characters");
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err("DNS labels must contain 1 through 63 ASCII characters");
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("DNS labels cannot start or end with a hyphen");
        }
        if !label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err("DNS hosts permit only lowercase ASCII letters, digits, and hyphens");
        }
    }
    Ok(())
}

fn looks_like_noncanonical_ipv4(host: &str) -> bool {
    host.split('.').all(|piece| {
        if let Some(hex) = piece
            .strip_prefix("0x")
            .or_else(|| piece.strip_prefix("0X"))
        {
            !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        } else {
            !piece.is_empty() && piece.bytes().all(|byte| byte.is_ascii_digit())
        }
    })
}

fn validate_canonical_signaling_origin(origin: &str) -> Result<(), String> {
    let parsed = url::Url::parse(origin)
        .map_err(|err| format!("invalid signaling allowed origin {origin:?}: {err}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "signaling allowed origin {origin:?} must use http:// or https://"
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!(
            "signaling allowed origin {origin:?} cannot contain credentials"
        ));
    }
    if parsed.port() == Some(0) {
        return Err(format!(
            "signaling allowed origin {origin:?} cannot use port 0"
        ));
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!(
            "signaling allowed origin {origin:?} cannot contain a path, query, or fragment"
        ));
    }

    let canonical = parsed.origin().ascii_serialization();
    if canonical == "null" || origin != canonical {
        return Err(format!(
            "signaling allowed origin {origin:?} is not canonical; use {canonical:?}"
        ));
    }
    Ok(())
}

fn next_arg_value(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{option} requires a value\n{}", server_usage()))
}

fn config_path_from_args(args: &[String]) -> Result<Option<String>, String> {
    let mut iter = args.iter().skip(1);
    let mut path: Option<String> = None;
    while let Some(arg) = iter.next() {
        if arg == "--config" {
            let value = iter
                .next()
                .ok_or_else(|| format!("--config requires a value\n{}", server_usage()))?
                .to_string();
            // [REVIEW M17] A second --config used to be silently discarded
            // (first one won); that hides an operator mistake. Error out.
            if path.is_some() {
                return Err(format!(
                    "--config supplied more than once\n{}",
                    server_usage()
                ));
            }
            path = Some(value);
        }
    }
    Ok(path)
}

fn parse_usize(value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("expected unsigned integer, got {value:?}"))
}

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = parse_usize(value)?;
    if parsed == 0 {
        return Err("value must be greater than zero".to_string());
    }
    Ok(parsed)
}

fn parse_positive_u16(value: &str) -> Result<u16, String> {
    let parsed: u16 = value
        .parse()
        .map_err(|_| format!("expected u16 integer, got {value:?}"))?;
    if parsed == 0 {
        return Err("value must be greater than zero".to_string());
    }
    Ok(parsed)
}

pub(crate) fn parse_optional_rtc_udp_bind_ip(value: &str) -> Result<Option<Ipv4Addr>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    if value.trim() != value {
        return Err(format!(
            "RTC UDP bind IP must be a canonical dotted-decimal IPv4 address without surrounding whitespace, got {value:?}"
        ));
    }
    let address = value.parse::<Ipv4Addr>().map_err(|_| {
        format!("RTC UDP bind IP must be a canonical dotted-decimal IPv4 address, got {value:?}")
    })?;
    if address.to_string() != value {
        return Err(format!(
            "RTC UDP bind IP must be canonical dotted-decimal IPv4, got {value:?}"
        ));
    }
    if address.is_unspecified() {
        return Err(
            "explicit RTC UDP bind IP cannot be the unspecified address 0.0.0.0; leave it empty for legacy wildcard binding"
                .to_string(),
        );
    }
    if address.is_multicast() || address == Ipv4Addr::BROADCAST {
        return Err(format!(
            "RTC UDP bind IP must be a unicast address assigned to a local interface, got {address}"
        ));
    }
    Ok(Some(address))
}

fn parse_optional_rtc_udp_path_mtu(value: &str) -> Result<Option<u16>, String> {
    if value.is_empty() || value == "0" {
        return Ok(None);
    }
    if value.trim() != value
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || value.starts_with('0')
    {
        return Err(format!(
            "RTC UDP path MTU must be a canonical decimal integer from {MIN_RTC_UDP_PATH_MTU} through 65535, or 0/unset for the rtc-sctp default, got {value:?}"
        ));
    }
    let mtu = value.parse::<u16>().map_err(|_| {
        format!(
            "RTC UDP path MTU must be a canonical decimal integer from {MIN_RTC_UDP_PATH_MTU} through 65535, got {value:?}"
        )
    })?;
    if mtu < MIN_RTC_UDP_PATH_MTU {
        return Err(format!(
            "RTC UDP path MTU must be at least {MIN_RTC_UDP_PATH_MTU} bytes so DTLS handshake datagrams fit safely, got {mtu}"
        ));
    }
    Ok(Some(mtu))
}

fn validate_local_rtc_udp_bind_ip(address: Ipv4Addr) -> Result<(), String> {
    let assigned = local_ipv4_interface_addresses()?;
    if assigned.contains(&address) {
        return Ok(());
    }
    Err(format!(
        "RTC UDP bind IP {address} is not assigned to a local IPv4 interface; configure an address present in this process/network namespace"
    ))
}

#[cfg(unix)]
fn local_ipv4_interface_addresses() -> Result<Vec<Ipv4Addr>, String> {
    struct IfAddrs(*mut libc::ifaddrs);

    impl Drop for IfAddrs {
        fn drop(&mut self) {
            // SAFETY: `getifaddrs` initialized this list and ownership stays
            // with this guard until exactly one `freeifaddrs` call.
            unsafe { libc::freeifaddrs(self.0) };
        }
    }

    let mut head = std::ptr::null_mut();
    // SAFETY: `head` is a valid out pointer. On success it owns the linked
    // list until the guard above releases it.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(format!(
            "failed to enumerate local IPv4 interfaces for RTC UDP bind validation: {}",
            std::io::Error::last_os_error()
        ));
    }
    let guard = IfAddrs(head);
    let mut addresses = Vec::new();
    let mut current = guard.0;
    while !current.is_null() {
        // SAFETY: every node is owned by `guard` for the duration of this
        // traversal; null socket-address pointers are explicitly skipped.
        let interface = unsafe { &*current };
        if !interface.ifa_addr.is_null()
            && unsafe { (*interface.ifa_addr).sa_family as i32 } == libc::AF_INET
        {
            // SAFETY: AF_INET guarantees `ifa_addr` points to `sockaddr_in`.
            let socket = unsafe { &*(interface.ifa_addr.cast::<libc::sockaddr_in>()) };
            addresses.push(Ipv4Addr::from(u32::from_be(socket.sin_addr.s_addr)));
        }
        current = interface.ifa_next;
    }
    Ok(addresses)
}

#[cfg(not(unix))]
fn local_ipv4_interface_addresses() -> Result<Vec<Ipv4Addr>, String> {
    Err(
        "explicit RTC UDP bind IP validation is not supported on this platform; leave rtc_udp_bind_ip empty and use STUN/TURN"
            .to_string(),
    )
}

fn parse_optional_udp_port_range(value: &str) -> Result<Option<RtcUdpPortRange>, String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("ephemeral") {
        return Ok(None);
    }
    let Some((min, max)) = value.split_once('-') else {
        return Err(format!(
            "invalid RTC UDP port range {value:?}; expected MIN-MAX or ephemeral"
        ));
    };
    let min = min
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("invalid RTC UDP minimum port in {value:?}"))?;
    let max = max
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("invalid RTC UDP maximum port in {value:?}"))?;
    if min == 0 || max == 0 {
        return Err("RTC UDP port range cannot contain port 0".to_string());
    }
    if min > max {
        return Err(format!(
            "invalid RTC UDP port range {value:?}; minimum exceeds maximum"
        ));
    }
    Ok(Some(RtcUdpPortRange { min, max }))
}

pub(crate) fn parse_bot_ship_types(value: &str) -> Result<Vec<u8>, String> {
    let mut ships = Vec::new();
    for item in value.split(',') {
        let ship = item.trim().parse::<u8>().map_err(|_| {
            format!("bot ship types must be a non-empty comma-separated list of ship numbers 1–8, got {value:?}")
        })?;
        if !(1..=8).contains(&ship) {
            return Err(format!("bot ship number must be 1–8, got {ship}"));
        }
        if ships.contains(&(ship - 1)) {
            return Err(format!(
                "duplicate bot ship number {ship}; list each ship once"
            ));
        }
        ships.push(ship - 1);
    }
    Ok(ships)
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_ice_transport_policy(value: &str) -> Result<RtcIceTransportPolicy, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "all" => Ok(RtcIceTransportPolicy::All),
        "relay" => Ok(RtcIceTransportPolicy::Relay),
        other => Err(format!(
            "expected RTC ICE transport policy all|relay, got {other:?}"
        )),
    }
}

fn parse_u64(value: &str) -> Result<u64, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("expected u64 integer, got {value:?}"))
}

/// Preserve the existing loss-env convention: values in 0..=1 are
/// probabilities, while values above 1 are percentages. Store the result as
/// integer millionths so configuration stays exactly comparable and avoids
/// floating-point state in `ServerConfig`.
fn parse_loss_probability(value: &str) -> Result<u32, String> {
    let value = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("expected number, got {value:?}"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!(
            "expected a non-negative finite number, got {value}"
        ));
    }
    let probability = if value > 1.0 { value / 100.0 } else { value };
    Ok((probability.clamp(0.0, 1.0) * 1_000_000.0).round() as u32)
}

fn server_usage() -> String {
    "usage: zubspace_server [bind] [--config PATH] [--bind ADDR] [--zone ID] [--zone-root DIR] [--allow-zone-overrides] [--max-players N] [--bot-count N] [--bot-ship-types CSV_1_TO_8] [--maxed-spawn-loadout true|false] [--tick-rate HZ] [--log-level error|warn|info|debug|trace|off] [--log-filter TARGET=LEVEL,...] [--log-file PATH] [--log-format text|json] [--metrics-listen IP:PORT] [--sim-observe] [--observer-network-profile auto|clean-lan|supported-mild-wan|supported-slow-wan|recovery-stress] [--qa-trace-upload] [--client-debug-trace] [--movement-snapback|--no-movement-snapback] [--tx-loss-pct PCT] [--tx-loss-seed N] [--rtc-udp-bind-ip IPV4] [--rtc-udp-path-mtu BYTES] [--rtc-udp-port-range MIN-MAX|ephemeral] [--rtc-external-ips CSV (unsupported)] [--rtc-ice-urls CSV] [--rtc-ice-username USER] [--rtc-ice-credential SECRET] [--rtc-ice-policy all|relay] [--signaling-allowed-origins CSV] [--signaling-auth-token SECRET] [--signaling-require-https] [--signaling-trust-proxy-headers] [--signaling-offer-rate N] [--signaling-max-offer-bytes N]".to_string()
}

fn normalize_optional_log_file(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_optional_path(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_optional_secret(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_log_level(value: &str) -> Result<ServerLogLevel, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "error" => Ok(ServerLogLevel::Error),
        "warn" | "warning" => Ok(ServerLogLevel::Warn),
        "info" => Ok(ServerLogLevel::Info),
        "debug" => Ok(ServerLogLevel::Debug),
        "trace" => Ok(ServerLogLevel::Trace),
        "off" | "none" => Ok(ServerLogLevel::Off),
        other => Err(format!(
            "expected log level error|warn|info|debug|trace|off, got {other:?}"
        )),
    }
}

fn parse_log_filter(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        return Ok(String::new());
    }
    value
        .split(',')
        .map(|directive| {
            let (target, level) = directive.split_once('=').ok_or_else(|| {
                "log_filter requires comma-separated target=level overrides; use log_level for the default".to_string()
            })?;
            let target = target.trim();
            if target.is_empty()
                || !target.chars().all(|c| c.is_ascii_alphanumeric() || "_:-.".contains(c))
            {
                return Err(format!("invalid log_filter target prefix {target:?}"));
            }
            let level = parse_log_level(level)
                .map_err(|error| format!("invalid log_filter for {target:?}: {error}"))?;
            Ok(format!("{target}={}", level.as_str()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|directives| directives.join(","))
}

/// 0-5000 microseconds: beyond that the added delay rivals a 60 Hz tick.
fn parse_write_coalescing(value: &str) -> Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|micros| *micros <= 5_000)
        .ok_or_else(|| format!("write coalescing must be 0..=5000 microseconds, got {value:?}"))
}

/// 16-4096 KiB/s: below that a single dense frame cannot fit, above it the
/// budget no longer bounds anything.
fn parse_send_budget(value: &str) -> Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|kib| (16..=4096).contains(kib))
        .ok_or_else(|| format!("send budget must be 16..=4096 KiB/s, got {value:?}"))
}

fn parse_observer_network_profile(
    value: &str,
) -> Result<crate::combat_timeline::ObserverNetworkProfile, String> {
    use crate::combat_timeline::ObserverNetworkProfile;

    match value {
        "auto" => Ok(ObserverNetworkProfile::AutomaticWan),
        "clean-lan" => Ok(ObserverNetworkProfile::CleanLan),
        "supported-mild-wan" => Ok(ObserverNetworkProfile::SupportedMildWan),
        "supported-slow-wan" => Ok(ObserverNetworkProfile::SupportedSlowWan),
        "recovery-stress" => Ok(ObserverNetworkProfile::RecoveryStress),
        other => Err(format!(
            "expected observer network profile auto|clean-lan|supported-mild-wan|supported-slow-wan|recovery-stress, got {other:?}"
        )),
    }
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in line.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' && in_string {
            escaped = true;
            continue;
        }
        if byte == b'"' {
            in_string = !in_string;
            continue;
        }
        if byte == b'#' && !in_string {
            return &line[..index];
        }
    }
    line
}

fn parse_toml_string(value: &str, source: &str, line_number: usize) -> Result<String, String> {
    if !(value.starts_with('"') && value.ends_with('"') && value.len() >= 2) {
        return Err(format!("{source}:{line_number}: expected quoted string"));
    }
    let mut parsed = String::new();
    let mut chars = value[1..value.len() - 1].chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            parsed.push(ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            return Err(format!(
                "{source}:{line_number}: unterminated string escape"
            ));
        };
        match escaped {
            '"' => parsed.push('"'),
            '\\' => parsed.push('\\'),
            'n' => parsed.push('\n'),
            'r' => parsed.push('\r'),
            't' => parsed.push('\t'),
            other => {
                return Err(format!(
                    "{source}:{line_number}: unsupported string escape \\{other}"
                ));
            }
        }
    }
    Ok(parsed)
}

fn parse_toml_integer(value: &str, source: &str, line_number: usize) -> Result<String, String> {
    let digits = value.replace('_', "");
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("{source}:{line_number}: expected unsigned integer"));
    }
    Ok(digits)
}

fn parse_toml_bool(value: &str, source: &str, line_number: usize) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{source}:{line_number}: expected boolean")),
    }
}

pub(crate) fn parse_env_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(format!(
            "expected environment boolean true|false|1|0, got {value:?}"
        )),
    }
}

fn parse_log_format(value: &str) -> Result<bool, String> {
    match value.trim() {
        "json" => Ok(true),
        "text" => Ok(false),
        _ => Err("log_format must be text or json".into()),
    }
}
fn parse_metrics_listen(value: &str) -> Result<Option<std::net::SocketAddr>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    value.parse().map(Some).map_err(|_| {
        "metrics_listen must be an IP:port socket address (or empty to disable)".into()
    })
}

#[cfg(test)]
mod connection_quality_config_tests {
    use super::*;
    #[test]
    fn quality_limits_parse_and_require_recovery_hysteresis() {
        let mut config = ServerConfig::default();
        config.apply_toml_like_config("[server]\nquality_max_rtt_ms=800\nquality_recovery_rtt_ms=500\nquality_max_loss_per_mille=200\nquality_recovery_loss_per_mille=80", "quality-test").unwrap();
        config.validate().unwrap();
        assert_eq!(config.connection_quality.max_rtt_ms, 800);
        assert_eq!(config.connection_quality.max_loss_per_mille, 200);
        assert_eq!(config.connection_quality.recovery_rtt_ms, 500);
        assert_eq!(config.connection_quality.recovery_loss_per_mille, 80);
        for setting in [
            "quality_max_rtt_ms=99",
            "quality_max_rtt_ms=3000",
            "quality_recovery_rtt_ms=600",
            "quality_recovery_rtt_ms=0",
            "quality_max_loss_per_mille=1001",
            "quality_max_loss_per_mille=0",
            "quality_recovery_loss_per_mille=150",
            "quality_recovery_loss_per_mille=0",
            "quality_max_rtt_ms=-1",
            "quality_max_rtt_ms=65536",
        ] {
            let mut invalid = ServerConfig::default();
            assert!(
                invalid
                    .apply_toml_like_config(setting, "quality-test")
                    .and_then(|_| invalid.validate())
                    .is_err(),
                "accepted invalid {setting}"
            );
        }
    }
}
