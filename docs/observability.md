# Observability

The game server reports on itself in three ways:

- **Structured logs** from `tracing`, as text or one JSON object per line.
- **Prometheus metrics** on a separate, private HTTP listener.
- **Player experience reports**: every browser sends a small summary of its
  frame times and network round trip every five seconds, which the server
  folds into the metrics.

An opt-in fourth source, **client presentation traces**, uploads sampled
render-state frames from each browser for debugging presentation problems.

Everything here is diagnostic. Reports from browsers are untrusted and never
change movement, combat, energy, pacing or authority, and no metric carries a
player, session or name label. The production environments' logging, metrics
ports and Prometheus instances are described in
[`deploy/README.md`](../deploy/README.md).

## Server logs

| TOML key | Environment | Flag | Default | Meaning |
| --- | --- | --- | --- | --- |
| `log_level` | `ZUBSPACE_SERVER_LOG_LEVEL` | `--log-level` | `info` | Fallback level: `error`, `warn`, `info`, `debug`, `trace` or `off` |
| `log_filter` | `ZUBSPACE_SERVER_LOG_FILTER` | `--log-filter` | empty | Comma-separated `target-prefix=level` overrides |
| `log_format` | `ZUBSPACE_SERVER_LOG_FORMAT` | `--log-format` | `text` | `text` or `json` |
| `log_file` | `ZUBSPACE_SERVER_LOG_FILE` | `--log-file` | empty (stdout) | Append to this file instead |

Filter targets are literal prefixes of `tracing` targets, and the most
specific match wins, so `rtc=info` also covers `rtc_sctp`, `rtc_dtls` and
`rtc_ice`. An override can be more or less verbose than `log_level`, and
`target=off` silences a target. A malformed filter fails startup. For example,
`log_level = "debug"` with `log_filter = "rtc=info,webrtc=info"` keeps
gameplay and session detail while holding the WebRTC stack at INFO; this is
the dev and local playtest setting.

A JSON line has `timestamp` (UTC RFC 3339 with sub-millisecond precision),
`level`, `target` and `fields` (which include `message` and, for many events,
an `event` name). The startup event `zubspace server started` records the
effective configuration.

Lines are written by a dedicated thread, never by the code that emits them, so
the tick never waits on log I/O. If the output stalls and the 16,384-line
queue fills, new lines are dropped and counted in
`zubspace_log_lines_dropped_total`. A process killed mid-write can lose the
lines still queued.

### Per-event targets

The highest-volume lines, one per shot, projectile, hit, damage or
acknowledgement, log under their own targets, each a child of the module that
emits it ([`log_targets.rs`](../src/bin/zubspace_server/log_targets.rs)):

- `zubspace_server::server_tick::combat`
- `zubspace_server::server_damage::combat`
- `zubspace_server::arena::positions::combat`
- `zubspace_server::arena::positions::weapons::combat`
- `zubspace_server::reliable::acks`
- `zubspace_server::rtc::acks`

A filter on the parent module still matches them. Prod logs the combat targets
at INFO and sets the two acknowledgement targets to `warn`. It also enables
DEBUG for a few low-volume modules: `observability` (browser experience
reports), `arena::positions` (projectile spawns and snap-back replay detail),
`server_damage` (deaths with killer and weapon), `player_events::prizes` and
`connection_quality` (probe audits). Dev logs everything at DEBUG. A server
test reads both deployed configs to keep this split.

### Notable events

Entries containing a space are log messages; the others are values of the
`event` field.

| Event or message | Level | What it reports |
| --- | --- | --- |
| `server_tick_health` | INFO | Every 600 ticks (10 s at 60 Hz): session count, tick work and interval jitter averages, overrun and snapback counts |
| `server_tick_overrun_phases` | WARN | A tick over its budget, split into admission, bots, world and publication, snapshots and finish time; at most one per second |
| `server connection quality window` | INFO | Per session: probe samples, average RTT and round-trip probe loss (see [connection quality](connection-quality.md)) |
| `player_experience_window` | DEBUG | Each accepted browser report; prod enables this one target in `log_filter` |
| `session_summary` | INFO | Per session at removal: duration, ship, kills and deaths, browser frame rate, slow and worst frames, corrections, ICE and probe RTT, probes and echoes, snap-backs, rejected positions, weapons fired and rejected, and how often the page was hidden (`page_hidden_periods`) |
| `page visibility changed` | INFO | The page hid or showed, pausing or resuming connection-quality measurement (see [connection quality](connection-quality.md#measurement-window)) |
| `server weapon rejected` | INFO | A refused weapon request; `block_cause` names a block that applied before the fire rules (`bomb_safety`, `mine_limit`, `objective_ball`, `objective_lock`, `warp_bomb_lock`, or, when the shot had no verified combat view, the input's frozen `combat_view_*` receipt rejection such as `combat_view_application_too_far_behind`, else a `projectile_timeline_*` reason), else `none` |
| `canonical_mismatch_detail` | WARN | Detail on a rejected movement replay, at most one per life per ten seconds |
| `server zone_file_manifest_resolved` | INFO | Per join: zone files held and sent, bytes and elapsed time (see [zone file cache](zone-file-cache.md)) |
| `client_debug_window` | INFO | Each accepted client presentation upload |

```sh
jq -c 'select(.level == "ERROR" or .level == "WARN")' server.log
jq -c 'select(.fields.event == "server_tick_health")' server.log
jq -c 'select(.fields.player_id == 12)' server.log
```

## Prometheus metrics

Metrics are off until `metrics_listen` (`ZUBSPACE_SERVER_METRICS_LISTEN`,
`--metrics-listen`) names an `IP:port`. A bind failure fails startup. The
listener serves only `GET /metrics` in the Prometheus text format, with no
authentication and no CORS; bind it to a private address. Production uses
`127.0.0.1:9100` (prod) and `127.0.0.1:9300` (dev).

Counters and histograms cover the life of the process; gauges are current
values. Histograms use fixed power-of-two buckets: upper bounds of
2<sup>k</sup>−1 microseconds for k = 4..24 (15 µs to about 16.8 s) for the
`_seconds` histograms, and the same numbers in bytes for the `_bytes` ones.
Labeled metrics use only fixed label values.

### Process and tick

| Metric | Type | Meaning |
| --- | --- | --- |
| `zubspace_uptime_seconds` | gauge | Process uptime |
| `zubspace_sessions` | gauge | Current application sessions, joined or not |
| `zubspace_players` | gauge | Joined humans (including spectators) plus bots |
| `zubspace_sessions_started_total`, `zubspace_sessions_ended_total` | counter | Registered and retired sessions |
| `zubspace_tick_budget_seconds` | gauge | `1 / tick_rate` |
| `zubspace_tick_work_seconds` | histogram | Work per tick, including state-lock waits, excluding broadcasts after the tick |
| `zubspace_tick_work_ewma_seconds` | gauge | Smoothed tick work; the load-shedding input |
| `zubspace_tick_interval_seconds` | histogram | Time between tick starts |
| `zubspace_tick_absolute_jitter_seconds` | histogram | Deviation from the configured tick interval |
| `zubspace_tick_overruns_total` | counter | Ticks whose work met or exceeded the budget |
| `zubspace_tick_admission_seconds`, `zubspace_tick_bots_seconds`, `zubspace_tick_world_and_publish_seconds`, `zubspace_tick_snapshots_seconds`, `zubspace_tick_finish_seconds` | histogram | Exclusive wall time of each tick phase, including scheduling and lock waits |
| `zubspace_tick_phase_logs_suppressed_total` | counter | `server_tick_overrun_phases` lines suppressed by the once-per-second limit |
| `zubspace_log_lines_dropped_total` | counter | Log lines dropped because the writer queue was full |

### Snapshots and events

| Metric | Type | Meaning |
| --- | --- | --- |
| `zubspace_snapshot_ack_round_trip_seconds` | histogram | Snapshot sent to its acknowledgement received; includes network, queues and browser scheduling |
| `zubspace_snapshot_full_bytes`, `zubspace_snapshot_delta_bytes` | histogram | Size of each full or delta snapshot sent |
| `zubspace_snapshot_send_skipped_total` | counter | Snapshots skipped because the recipient's send budget was spent |
| `zubspace_snapshots_with_holds_total`, `zubspace_snapshot_entities_held_total` | counter | Snapshots that held some actors at their acknowledged state to fit the budget, and how many actors were held |
| `zubspace_realtime_event_send_deferred_total` | counter | Realtime events postponed to a later tick by the send budget |
| `zubspace_realtime_events_sent_total{kind}` | counter | Realtime events queued, by `kind`: `weapon`, `watch_damage`, `repel_mutations`, `projectile_state`, `owner_weapons_cancelled` |
| `zubspace_realtime_event_game_copies_total{decision}` | counter | Event messages also copied to the game channel (`sent`) or not copied because the link showed no loss (`skipped`) |
| `zubspace_movement_snapbacks_total` | counter | Server-issued corrections for sustained movement drift |
| `zubspace_observer_auto_targets{target_ticks}` | gauge | Joined sessions on the automatic presentation profile, by current target delay (`7`, `10`, `15`, `20` ticks) |

The send budget, holds and game-channel copies are explained in
[networking](networking.md).

### RTC transport

`channel` is `game`, `events` or `reliable`.

| Metric | Type | Meaning |
| --- | --- | --- |
| `zubspace_rtc_outbound_queue_packets`, `zubspace_rtc_outbound_queue_bytes` | gauge | Queued outbound packets and bytes |
| `zubspace_rtc_reliable_overflows_total` | counter | Reliable queue overflows; each forces a disconnect |
| `zubspace_rtc_realtime_coalesced_total` | counter | Stale realtime packets replaced before sending |
| `zubspace_rtc_protocol_violations_total` | counter | DataChannel protocol violations |
| `zubspace_rtc_inbound_queue_packets{channel}` | gauge | Received packets waiting to be processed |
| `zubspace_rtc_inbound_packets_total{channel}`, `zubspace_rtc_inbound_bytes_total{channel}` | counter | Received packets and application bytes processed |
| `zubspace_rtc_outbound_packets_total{channel}`, `zubspace_rtc_outbound_bytes_total{channel}` | counter | Application packets and bytes accepted by each channel, including event copies |
| `zubspace_rtc_send_capacity_waits_total{channel}`, `zubspace_rtc_send_capacity_wait_seconds_total{channel}` | counter | Sends that waited for channel buffer space, and the total wait |
| `zubspace_rtc_sent_queue_age_max_seconds{channel}` | gauge | Longest time a sent packet spent queued |
| `zubspace_rtc_outstanding_bytes_peak{channel}` | gauge | Highest sampled per-channel outstanding bytes at dispatch |
| `zubspace_rtc_lane_queue_packets{channel}`, `zubspace_rtc_lane_queue_bytes{channel}`, `zubspace_rtc_lane_oldest_age_seconds{channel}` | gauge | Current per-session send queues, summed across sessions, and the oldest queued packet's age |
| `zubspace_rtc_udp_datagrams_sent_total`, `zubspace_rtc_udp_datagrams_received_total` | counter | UDP datagrams on each session's selected candidate pair, excluding STUN |
| `zubspace_rtc_udp_bytes_sent_total`, `zubspace_rtc_udp_bytes_received_total` | counter | Bytes above UDP on those pairs, including DTLS and SCTP framing |

The UDP counters come from each peer connection's own statistics, polled every
five seconds and once at close. Compare them with the per-channel packet
counters to see how many DataChannel messages share a datagram.

### Connection quality

Gauges `zubspace_connection_quality_sessions`, `_measured_sessions`,
`_dispatch_started`, `_echoes_received`, `_local_dispatch_cancelled`,
`_samples`, `_max_session_average_rtt_ms`,
`_max_session_round_trip_probe_loss`, `_sustained_degraded_sessions`,
`_retained_holds` and `_held_sessions`; counters
`zubspace_connection_quality_forced_total`, `_return_denied_total` and
`_healthy_return_total`. Their meaning is in
[connection quality](connection-quality.md).

### Zone files

`zubspace_zone_files_sent_total`, `zubspace_zone_bytes_sent_total`,
`zubspace_zone_files_held_total` and `zubspace_zone_bytes_held_total`
(counters) count zone files transferred and zone files a browser already held;
see [zone file cache](zone-file-cache.md#server-side-counting).

### Browser reports

| Metric | Type | Meaning |
| --- | --- | --- |
| `zubspace_browser_reports_total` | counter | Accepted reports; check this before reading the others |
| `zubspace_browser_frames_total`, `zubspace_browser_slow_frames_total` | counter | Visible frame intervals reported, and those over 50 ms |
| `zubspace_browser_unready_frames_total` | counter | Frames without gameplay authority, including spectating and transitions |
| `zubspace_browser_corrections_total` | counter | Local self-corrections reported by browsers |
| `zubspace_browser_frame_window_mean_seconds`, `zubspace_browser_frame_window_max_seconds` | histogram | Mean and worst frame interval of each report window (one sample per window, not per frame) |
| `zubspace_browser_ice_round_trip_seconds` | histogram | RTT of the selected ICE candidate pair from `getStats()`; absent when unavailable, never zero |
| `zubspace_browser_game_buffered_peak_bytes` | histogram | Largest game-channel `bufferedAmount` before a send in each window |
| `zubspace_browser_game_sends_dropped_total` | counter | Game-channel sends the page dropped because they would have pushed its queue past 2 KiB |

### Client trace counters

Counters `zubspace_client_debug_reports_total`, `_frames_total`,
`_dropped_frames_total`, `_rejections_total`, `_log_failures_total`,
`_browser_errors_total`, `_context_losses_total` and
`_context_restorations_total`, and the gauge
`zubspace_client_debug_follow_camera_error_pixels_max` (largest reported
distance between the camera and a followed player's drawn position, capped at
one million pixels). See [below](#client-presentation-traces).

### Example queries

```promql
# p95 snapshot acknowledgement round trip over five minutes
histogram_quantile(0.95, sum by (le) (rate(zubspace_snapshot_ack_round_trip_seconds_bucket[5m])))

# Share of reported frames slower than 50 ms
sum(rate(zubspace_browser_slow_frames_total[5m])) / sum(rate(zubspace_browser_frames_total[5m]))

# Tick work as a fraction of the budget
zubspace_tick_work_ewma_seconds / zubspace_tick_budget_seconds

# Sustained reliable overflows mean a stalled or underprovisioned client path
increase(zubspace_rtc_reliable_overflows_total[10m])
```

Low ICE RTT with slow acknowledgements or frames points at browser scheduling
or queueing; high tick work and jitter together with growing queues point at
the server or host. Neither measures input-to-display latency.

## Player experience reports

Each browser adds one small callback to its render loop. Every five seconds it
sends one 62-byte report (version 2; the server still accepts the 54-byte
version 1) on the reliable DataChannel: frame count, slow frames, mean and
maximum frame interval, unready frames, self-corrections, the selected ICE
pair's RTT from `getStats()`, and the game channel's peak send queue and
dropped sends. Hidden-page time is excluded, windows longer than 30 seconds
are discarded, and reports are dropped rather than queued when the channel is
backed up. Short sessions and the final partial window may never be reported.

The server accepts a report only from a joined session with a valid security
check, in the current server session, arena and life, with exact framing and
plausible values, and at most one per session every two seconds.

## Client presentation traces

When `client_debug_trace` is on (`ZUBSPACE_SERVER_CLIENT_DEBUG_TRACE`,
`--client-debug-trace`; off by default, on in dev and the local playtest
profile), every joined browser uploads sampled frames of its render state
about every 20 seconds. Ordinary URLs work; the game's debug UI is not
involved.

- After joining, the browser checks `client_debug_trace` in `/v1/status`, then
  fetches the policy from `GET /v1/debug/client-trace` using its session's
  diagnostic credentials. Uploads are `POST` to the same path. Both return
  `404` when the feature is off. If the policy check fails, or an upload gets
  `403` or `404`, collection stays off for the rest of that join.
- The browser keeps a ring of 1,200 frames and up to 32 failure events, sends
  one request at a time with a five-second deadline, and drops the oldest
  frames to stay under 192 KiB. `droppedFrames` reports the loss. Failed
  uploads are not retried.
- The server checks the schema, the session scope and an increasing sequence
  number, accepts at most one batch per session every ten seconds, and tracks
  16 sessions at a time.

Accepted batches are appended as NDJSON to `client.log` in the directory of
`log_file`, or `.debug/client.log` when logging to stdout. Each record has
`timestamp`, `level` (`DEBUG`), `event` (`client_presentation_window`),
`schemaVersion`, `bundleId`, `serverSessionId`, `arenaStreamId`, `playerId`,
`serverBinarySha256` and the browser's `report`. A write failure returns `503`
and counts in `zubspace_client_debug_log_failures_total`.

A frame row has 34 numbers; positions are world pixels:

| Index | Contents |
| --- | --- |
| 0–1 | Browser time in ms, and the interval since the previous frame (capped at 30 s) |
| 2 | Fixed-step remainder |
| 3–6 | Rendered local position x/y, then canonical ship x/y |
| 7–9 | Followed player id (−1 for none) and its drawn x/y |
| 10–15 | Presentation render tick, estimated server tick, local life, correction count, authority-ready flag, spectator flag (−1 when unknown) |
| 16–24 | One visible local bomb: kind (1), owner, life, prediction id as two u32 halves, child index, x/y, animation time |
| 25–33 | One visible remote bomb (kind 3) or bullet (kind 2), with the same fields and the server shot-group id |

Kind −1 means no sample. Events are `[kind, browser ms]` with kind 1 for an
exception, 2 an unhandled rejection, 3 WebGL context loss and 4 restoration.
When the policy advertises `muzzleWidth`, the browser also records up to 16
kind-5 muzzle samples per batch (at most ten a second) pairing a fresh remote
bullet with its owner's drawn pose, plus a kind-6 count of samples evicted.
The producers are `src/game/render/client_debug.rs` and
`js/zubspace_client_debug.js`.

The same setting makes the server log sampled `server_projectile_near_hull`
records from its projectile collision at INFO (as `zubspace_event` lines, at
most 64 per 100 ticks).

These rows describe what the renderer intended to draw at the start of a
frame, not what reached the screen, and sampling has a cost of its own.

## Local recording

Tracked local configuration lives in [`etc/`](../etc/README.md).
`etc/server/playtest.toml` runs 15 bots (ships 1, 4, 5, 8) with JSON logs,
gameplay DEBUG and WebRTC INFO, metrics on `127.0.0.1:9100` and client
presentation traces on.

```sh
make start-prom-server                  # Prometheus on http://127.0.0.1:9090
make restart-local-recording            # rebuild and restart the game and client, recording
make restart-local-release-recording    # the same with the optimized client
make stop-prom-server                   # keeps the stored metrics
```

The collector (`etc/prom/compose.yml`, container `zubspace-prom_1`) uses host
networking, scrapes `127.0.0.1:9100` every five seconds and keeps seven days
or 1 GB in the `zubspace-prom_data` volume. Starting it does not start the game
or enable metrics on a server that is already running.

Each recording gets its own `.debug/<UTC timestamp>-<suffix>/` directory
(`DEBUG_DIR` changes the root, `PLAYTEST_CONFIG` the server config):

- `server.log`: the server's NDJSON log;
- `server-console.log`: anything written to stdout or stderr outside the
  logger, such as startup errors or panics;
- `client.log`: client presentation traces;
- `static.log`: the local static server's output;
- `session.json`: launch time, configuration hash, server and WASM hashes,
  source revision and working-tree status.

Recording forces JSON logging into `server.log`; other environment overrides
still apply, for example
`ZUBSPACE_SERVER_LOG_LEVEL=info ZUBSPACE_SERVER_CLIENT_DEBUG_TRACE=false`
for a lighter baseline or `ZUBSPACE_SERVER_LOG_FILTER=''` for full transport
detail. Local logs are kept until you delete them.

## Tests

`make test-observability` covers report parsing, log fields and metric
exposition. `make test-e2e-observability` joins ordinary browser clients and
checks the reports, JSON logs and the metrics listener end to end.
`make test-local-observability` checks the playtest config, the Prometheus
configuration and a real server scraped by a temporary collector.
