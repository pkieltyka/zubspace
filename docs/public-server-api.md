# Public server API

The game server's HTTP listener handles WebRTC signaling, discovery and
diagnostics. Gameplay itself never uses HTTP; it runs on DataChannels (see
[networking](networking.md)). Locally the listener is
`http://127.0.0.1:9001`. Behind the client image's nginx the `/v1` routes are
served on the site's origin, and the read-only ones are also available on
the API host names `server.zubspace.gg` and `dev-server.zubspace.gg` (see
[deployment](deployment.md#nginx-routes) and
[`deploy/README.md`](../deploy/README.md)).

One server process runs one active zone. The catalog can hold other zones,
which report `status: "standby"` and zero players.

## Read-only routes

| GET route | Response |
| --- | --- |
| `/v1/status` | Process health, build commit, uptime, player and zone totals, transport diagnostics |
| `/v1/players` | `{total_players, players: [player, ...]}` across all zones |
| `/v1/zones` | `{total_players, zones: [summary, ...]}` |
| `/v1/zones/{id}` | One zone summary |
| `/v1/zones/{id}/players` | `{total_players, players: [player, ...]}` for that zone |
| `/v1/zones/{id}/status` | Zone simulation and snapshot diagnostics |
| `/v1/ping` | `200`, plain-text `.` |
| `/`, `/index.html` | Plain-text `Welcome to the Zubspace game server.` |

Conventions:

- Responses carry `Cache-Control: no-store` and
  `Access-Control-Allow-Origin: *`. `OPTIONS` returns `204` with the CORS
  preflight headers.
- Other methods return `405 {"ok":false,"error":"method_not_allowed"}` from the
  server; nginx answers them with `403` before they reach it.
- Unknown paths return `404 {"ok":false,"error":"not_found"}`. A well-formed
  but unknown zone id returns `404 {"ok":false,"error":"unknown_zone"}`.
- `{id}` must be the exact catalog id: 1–15 lowercase letters, digits or
  hyphens, beginning and ending with a letter or digit. Display-name aliases
  are not accepted: use `/v1/zones/extreme`, not the menu alias
  `extreme-games`. A trailing slash or extra path segment is a `404`.
- The zone is selected by the path only; query strings are ignored.
- Clients should ignore fields they do not recognise.

## Players

A player entry is `{"id", "name", "bot", "zone"}`, in that order: the numeric
player id, the display name, `true` for a server-owned bot, and the zone id.
Rosters serialize `total_players` before `players`, list entries by player id,
and `players` always has `total_players` entries.

Counts include every bot and every joined human, including spectators and
players who are dead or respawning. Browser transports that have not joined
are excluded. Names, ids and the bot flag are the only player data exposed;
sessions, addresses, tokens and game state are not.

## Zone summaries

A summary has these fields, in this order:

| Field | Meaning |
| --- | --- |
| `id` | Catalog id |
| `display_name` | Name shown in the menu |
| `status` | `open` for the active zone, otherwise `standby` |
| `default` | `true` for the active zone, which an unnamed login joins |
| `listed` | `true` for `chaos-zone`, the zone the menu lists; others are reachable by link |
| `total_players` | Joined humans plus bots; 0 for a standby zone |
| `max_players` | Configured `max_players`. Admission counts humans plus bots against the smaller of this and 500 |
| `map` | Map file name |
| `maxed_spawn_loadout` | Whether ships spawn with full stats and weapons |
| `free_for_all` | Whether the zone runs free-for-all |

The menu uses `/v1/zones` to resolve the selected zone (falling back to its
embedded catalog if the request fails), `/v1/players` to check a name before
joining, and `/v1/zones/{id}` to check the zone's capacity. These checks are
advisory; the server decides admission when the player logs in.

## `/v1/status`

The response starts with `ok`, `service` (`"zubspace-server"`), `version`,
`uptime`, `zones`, `zones_open` and `total_players`, in that order, followed by
the process-level diagnostics:

- `version`: the full Git commit the binary was built from (see
  [build identity](deployment.md#build-identity)).
- `uptime`: whole seconds since the process started.
- `zones`, `zones_open`: catalog size and number of open zones.
- `total_players`: joined humans plus bots across all zones.
- `tick_rate`, `observer_network_profile`, and `client_debug_trace` (whether
  [client presentation traces](observability.md#client-presentation-traces)
  are enabled).
- `clients`: registered application sessions, joined or not.
- `rtc_sessions`: browser transports. This is what the deploy scripts check
  before restarting prod.
- RTC capacity: `rtc_capacity`, `rtc_effective_capacity`,
  `rtc_udp_port_capacity`, `rtc_udp_ports_in_use`, `rtc_udp_ports_degraded`,
  `rtc_udp_degraded_ports` (see
  [UDP ports](deployment.md#udp-ports-and-capacity)).
- Transport queues and counters (`rtc_inbound_*`, `rtc_outbound_*`,
  `rtc_channel_protocol_violations`, `rtc_ingress_execution` and the other
  `rtc_*` fields).
- `connection_quality`: the summary described in
  [connection quality](connection-quality.md).

## `/v1/zones/{id}/status`

For the active zone the response holds `ok`, `zone`, `status`, `total` (the
zone's `total_players`) and `arena_tick`, plus the simulation diagnostics:
`snapshot_acked_clients`, the objective counters, `tick_timing` (tick interval,
jitter and work summaries over the server's lifetime), threat and projectile
presentation counters, pending-hit and damage counters, movement correction
and snapback counters, Repel and Warp delivery queues, position-input and
realtime-event queues, `presentation_geometry` and `reliable_gameplay`. A
standby zone returns only `ok`, `zone`, `status: "standby"` and `total: 0`.

## Signaling

`POST /v1/rtc/offer` takes a complete (non-trickle) offer as JSON,
`{"type": "offer", "sdp": "..."}`, with `Content-Type: application/json`, and
returns the answer in the same form. The browser posts to this path on its own
origin unless the page URL names another offer URL with `rtc=`.

| Status | `error` | Cause |
| --- | --- | --- |
| `400` | `invalid_webrtc_offer` | The body is not a valid offer |
| `401` | `invalid_admission_token` | `signaling_auth_token` is set and the bearer token is missing or wrong |
| `403` | `origin_not_allowed` | `Origin` is not in `signaling_allowed_origins` |
| `403` | `https_required` | `signaling_require_https` is set and the proxy did not report HTTPS |
| `413` | `offer_too_large` | Body larger than `signaling_max_offer_bytes` |
| `415` | `content_type_must_be_json` | Wrong content type |
| `429` | `offer_rate_limited` | Over `signaling_offer_rate_per_minute` for this client address |
| `503` | `rtc_capacity_unavailable`, `rtc_service_unavailable` | No usable UDP port or a temporary ICE/UDP failure; sent with `Retry-After: 1` |
| `500` | `internal_server_error` | Unexpected failure; details are in the server log |

Error bodies are JSON with `"ok": false` and the `error` code; most also carry
a human-readable `message`, and `413` reports `max_bytes`. CORS
for this route follows the signaling origin policy rather than `*`. The
[deployment guide](deployment.md#signaling-security) explains the security
settings.

## Diagnostic routes

These are opt-in and return `404` unless enabled in the server configuration:

| Route | Enabled by | Purpose |
| --- | --- | --- |
| `GET`, `POST /v1/debug/client-trace` | `client_debug_trace` | Policy and uploads for [client presentation traces](observability.md#client-presentation-traces); requires the browser's live session credentials |
| `POST /v1/qa/trace` | `qa_trace_upload` | QA trace uploads from test browsers |
| `GET /v1/qa/trace/{bundle}` | `qa_trace_upload` | Retrieval of an uploaded trace, from loopback only |

`GET /rtc` and `GET /rtc/signaling` return a static JSON description of the
transport (channel labels and queue limits). nginx does not proxy them.

## Examples

```sh
curl --fail http://127.0.0.1:9001/v1/status
curl --fail http://127.0.0.1:9001/v1/zones
curl --fail http://127.0.0.1:9001/v1/zones/chaos-zone/players
curl --fail https://server.zubspace.gg/v1/players
```

A script watching for activity can poll `/v1/players` every few seconds and
look at the entries with `"bot": false`; the [join notifier](../notifier/README.md)
works this way.
