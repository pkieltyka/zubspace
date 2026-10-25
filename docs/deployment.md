# Deployment

Zubspace ships as two container images: the authoritative game server and a
static client served by nginx. A browser loads the client and talks HTTP to
nginx for files, signaling and status. After one SDP exchange, all gameplay
travels over three WebRTC DataChannels on UDP directly between the browser and
the game server.

This page covers the images, the local Compose stack, build identity, server
configuration and what a self-hosted public deployment has to provide. The
production setup (one VPS running prod and dev as rootless Podman Compose
projects behind Cloudflare tunnels) is described in
[`deploy/README.md`](../deploy/README.md).

## Topology

```text
Browser --HTTPS--> TLS terminator --HTTP--> nginx (client image)
   |                                          | static bundle
   |                                          +-- /v1/* --> game server HTTP (private, default :9001)
   |
   +-- WebRTC (ICE/DTLS/SCTP) over UDP --> game server, one UDP port per peer
```

nginx is signaling and status infrastructure, not a gameplay relay. The
`zubspace-game`, `zubspace-events` and `zubspace-reliable` channels (see
[networking](networking.md)) flow over whichever ICE candidate pair the browser
and server select. The HTTP routes are listed in the
[public server API](public-server-api.md).

## Images

### Server (`Dockerfile.server`)

- Builds `zubspace_server` with `cargo build --locked --release` in
  `rust:1.97.1-trixie`, then strips it.
- Runtime is `debian:13.6-slim` with `ca-certificates` and `curl` (used by the
  Compose health check), running as user `zubspace` (uid 10001) in `/app`.
- Contents: `/usr/local/bin/zubspace_server`, `/app/config`, the zone bundles
  in `/app/assets/zones`, and `LICENSE`, `ASSETS.md` plus the
  provenance documents under `/app/docs/`.
- Entrypoint `zubspace_server`, default arguments
  `--config /app/config/container.toml`. That file is
  `config/zubspace_server.toml` with `bind = "0.0.0.0:9001"` and
  `log_file = ""` (logs to stdout). Pass `--config <path>` to use a mounted
  TOML instead, as production does.
- `EXPOSE 9001/tcp 9002-9501/udp` is documentation only. The bundled config
  uses ephemeral UDP ports; a container deployment sets a fixed range (see
  [UDP ports](#udp-ports-and-capacity)).

### Client (`Dockerfile.client`)

- The builder (Rust image plus Node 26 and `lld`) runs
  `tools/build_wasm_release.mjs`, which builds the release WASM and writes
  `dist/`: a stamped `index.html`, `zubspace.wasm`, `js/`, `zone-assets.json`,
  the runtime zone files from `assets/zones/` (`.lvl`, `.lvz` and settings
  files), `BUILD_INFO.json`, and `LICENSE`, `ASSETS.md` and the provenance documents.
- Runtime is `nginx:1.31.3-alpine3.24` serving `dist/` from
  `/usr/share/nginx/html`. The base image's entrypoint renders
  `docker/nginx.conf.template` with `envsubst` at start.

| Variable | Default | Meaning |
| --- | --- | --- |
| `ZUBSPACE_SERVER_UPSTREAM` | `zubspace-server:9001` (also a build arg) | Game server HTTP address nginx proxies to |
| `ZUBSPACE_CLIENT_LISTEN_ADDRESS` | `0.0.0.0` | nginx listen address |
| `ZUBSPACE_CLIENT_LISTEN_PORT` | `80` | nginx listen port |
| `ZUBSPACE_ROBOTS_TAG` | empty | `X-Robots-Tag` value on every response; empty sends no header |

### nginx routes

The site server block (`default_server`):

| Path | Behaviour |
| --- | --- |
| `/`, `/index.html` | `Cache-Control: no-store`, so a reload picks up a new build |
| `*.wasm`, `*.js` | Cached 30 days, `immutable`; the page requests them with `?v=<build version>` |
| Everything else in the bundle | `Cache-Control: no-cache` (revalidate); zone files and `BUILD_INFO.json` keep their names across builds |
| `/healthz` | `200 ok`, the Compose health check |
| `/client-status` | Static JSON identifying the client service |
| `/v1/status`, `/v1/zones`, `/v1/zones/{id}[/players\|/status]` | Proxied, GET and OPTIONS |
| `/v1/players`, `/v1/ping` | Proxied, GET |
| `/v1/rtc/offer` | Proxied with 30-second proxy timeouts |
| `/v1/qa/trace` | Proxied, POST only, 256 KiB body limit |
| `/v1/debug/client-trace` | Proxied, GET and POST, 192 KiB body limit, 5-second timeouts |

Disallowed methods on these locations get `403` from nginx. A second server
block answers host names matching `^(?:[a-z0-9-]+-)?server\.` (for example
`server.zubspace.gg`): it proxies only the read-only `/v1/status`,
`/v1/players`, `/v1/ping` and `/v1/zones[...]` routes and returns `404` for
everything else.

Forwarded headers are rewritten, never passed through from arbitrary callers:

- `X-Real-IP` and `X-Forwarded-For` are set to nginx's view of the client
  address. When the TCP peer is loopback (`127.0.0.1` or `::1`), nginx takes
  that address from `CF-Connecting-IP`, so a same-host tunnel or proxy still
  reports the real browser.
- `X-Forwarded-Proto` keeps an incoming value only from a loopback peer;
  otherwise it is nginx's own scheme.

The template therefore works behind a TLS terminator on the same host
(cloudflared, or a local reverse proxy). A TLS terminator on another host needs
a changed template that trusts that host explicitly.

## Build identity

Every build carries the full Git commit it was built from, in
`ZUBSPACE_BUILD_COMMIT`: a 40- or 64-character lowercase hexadecimal hash.

- The Makefile sets it to `git rev-parse HEAD` and exports it.
  `make docker-build` passes it as a build arg, and
  [`scripts/deploy.sh`](../scripts/deploy.sh) passes the commit it deploys.
- Container builds set `ZUBSPACE_REQUIRE_BUILD_REVISION=1`, and
  `.dockerignore` keeps `.git` out of the build context, so an image build
  without the arg fails. For a raw Compose build, supply it yourself:
  `ZUBSPACE_BUILD_COMMIT=$(git rev-parse HEAD) docker compose build`. A build
  from a source archive must supply the archive's original commit.
- Outside a container, `build.rs` falls back to `git rev-parse HEAD`. When both
  a supplied value and a checkout exist, the client build requires them to
  match.

Where it shows up:

| Place | Value |
| --- | --- |
| Image label `org.opencontainers.image.revision` | Full commit |
| Server `GET /v1/status` field `version` | Full commit, fixed at build time |
| Client `BUILD_INFO.json` field `git` | Full commit |
| Client `BUILD_INFO.json` field `version` | First 12 characters of the commit plus short hashes of the WASM and JS assets; used as the `?v=` cache key |
| Menu footer | `v<Cargo package version> · <first 8 characters>` |

The commit names `HEAD`. Uncommitted changes in a working tree are built into
an image but not reflected in its identity; `scripts/deploy.sh` avoids this by
building from `git archive` of the commit. `make check-container-bundles`
checks both built images' bundled documents, revision labels and the client's
footer and `BUILD_INFO.json`.

## Local Compose stack

```sh
make docker-build   # both images, tagged zubspace/server:dev and zubspace/client:dev
make docker-up      # docker compose up --build
# open http://localhost:7000/
make docker-down
```

`docker-compose.yml` needs Docker Engine on Linux. Both services use
`network_mode: host`:

- **zubspace-server** listens for HTTP on `127.0.0.1:9001`, uses the UDP range
  `9002-9501` and starts `ZUBSPACE_SERVER_BOT_COUNT` bots (default 0). It
  allows any signaling origin, requires no token and no HTTPS, and trusts the
  forwarded headers set by the local nginx. Its health check is
  `curl http://127.0.0.1:9001/v1/status`.
- **zubspace-client** starts once the server is healthy and listens on
  `127.0.0.1:${ZUBSPACE_CLIENT_PORT:-7000}`, proxying to `127.0.0.1:9001`.

HTTP is loopback-only, but the RTC sockets bind `0.0.0.0` and are reachable on
every host interface. Under host networking, Compose `ports:` rules do not
apply; restrict UDP `9002-9501` with the host firewall if the machine is on an
untrusted network.

Host networking is required for same-host play: the server must reach the
browser's host or mDNS candidate, and the browser the server's host candidate.
On a bridge network with only the UDP range published, signaling succeeds but
the DataChannels never open. Docker Desktop's host networking is not
equivalent.

`make test-compose-rtc` builds and starts both images, joins through nginx in
headless Chrome, checks all three channels, the gameplay join and snapshot
acknowledgements, and the status cleanup, then tears the stack down. To run it
beside a running local stack, set all three of `ZUBSPACE_COMPOSE_CLIENT_PORT`,
`ZUBSPACE_COMPOSE_SERVER_PORT` and `ZUBSPACE_COMPOSE_UDP_PORT_RANGE`.

## Server configuration

Settings are applied in this order, later sources winning:

1. compiled defaults;
2. a TOML file from `--config <path>` or `ZUBSPACE_SERVER_CONFIG`;
3. `ZUBSPACE_SERVER_*` environment variables;
4. command-line flags.

[`config/zubspace_server.toml`](../config/zubspace_server.toml) is the
commented reference for every key, and `deploy/<env>/zubspace_server.toml` are
complete production examples. `zubspace_server --help` prints the flags.
Boolean environment values accept only `true`, `false`, `1` or `0`; anything
else fails startup. Startup validates the whole configuration, loads the zone
catalog, checks every fixed UDP port, and only then binds the HTTP listener.

Settings that matter for a deployment:

| TOML key | Environment | Flag | Default | Meaning |
| --- | --- | --- | --- | --- |
| `bind` | `ZUBSPACE_SERVER_BIND` | `--bind` | `127.0.0.1:9001` | HTTP listener (signaling, status, API) |
| `zone` | `ZUBSPACE_SERVER_ZONE` | `--zone` | `chaos-zone` | The active zone |
| `zone_bundle_root` | `ZUBSPACE_SERVER_ZONE_ROOT` | `--zone-root` | empty (embedded zones only) | Directory of disk zone bundles |
| `allow_zone_overrides` | `ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES` | `--allow-zone-overrides` | `false` | Let disk bundles replace embedded zones |
| `max_players` | `ZUBSPACE_SERVER_MAX_PLAYERS` | `--max-players` | `500` | RTC session capacity (at most 768); arena admission counts humans plus bots against the smaller of this and 500 |
| `bot_count` | `ZUBSPACE_SERVER_BOT_COUNT` | `--bot-count` | `0` | Server-owned bots, simulated inside the server tick |
| `tick_rate` | `ZUBSPACE_SERVER_TICK_RATE` | `--tick-rate` | `60` | Simulation rate in Hz (1–240) |
| `rtc_udp_port_range` | `ZUBSPACE_SERVER_UDP_PORT_RANGE` | `--rtc-udp-port-range` | `ephemeral` | `MIN-MAX` fixed range, or `ephemeral` |
| `rtc_udp_bind_ip` | `ZUBSPACE_SERVER_RTC_UDP_BIND_IP` | `--rtc-udp-bind-ip` | empty (`0.0.0.0`) | IPv4 address the RTC sockets bind and advertise |
| `rtc_udp_path_mtu` | `ZUBSPACE_SERVER_RTC_UDP_PATH_MTU` | `--rtc-udp-path-mtu` | `0` | Measured IPv4 path MTU, 1280–65535; 0 keeps the stack default |
| `rtc_ice_urls`, `rtc_ice_username`, `rtc_ice_credential`, `rtc_ice_policy` | `ZUBSPACE_SERVER_RTC_ICE_*` | `--rtc-ice-*` | empty, `all` | Server-side STUN/TURN |
| `signaling_*` | `ZUBSPACE_SERVER_SIGNALING_*` | `--signaling-*` | see below | Signaling security |
| `log_*`, `metrics_listen`, `client_debug_trace` | `ZUBSPACE_SERVER_LOG_*`, `ZUBSPACE_SERVER_METRICS_LISTEN`, `ZUBSPACE_SERVER_CLIENT_DEBUG_TRACE` | `--log-*`, `--metrics-listen`, `--client-debug-trace` | text logs, no metrics | See [observability](observability.md) |

The network-efficiency settings `send_budget_kib_per_second`,
`realtime_event_game_copy`, `rtc_write_coalesce_micros` and
`rtc_defer_sack_to_data` are described in [networking](networking.md); the
connection-quality thresholds `quality_*` in
[connection quality](connection-quality.md).

### Zone bundles

The server builds its zone catalog once at startup from the embedded Chaos
Zone and Extreme plus any bundles under `zone_bundle_root`. A relative root
resolves from the working directory (`/app` in the image, which carries
`assets/zones`). A configured root that is missing, a symlink or invalid fails
startup; it never silently falls back to the embedded zones. Adding or
replacing a bundle takes a server restart, not a rebuild of the server or
client.

Each immediate subdirectory is one bundle, named after its `id`, holding a
`zone.bundle` manifest:

```text
version=1
id=chaos-zone
display_name=Chaos Zone
override=chaos-zone
map=bdhadar.lvl
map_sha256=<64 hex>
settings=arena.conf
settings_sha256=<64 hex>
news=news.txt
news_sha256=<64 hex>
lvz=aiq.lvz,<64 hex>
```

- `id` is 1–15 lowercase letters, digits or hyphens, not starting or ending
  with a hyphen. Ids and display names must stay unique across the catalog
  after lowercasing and removing punctuation.
- Every listed file is pinned by its SHA-256. A digest mismatch, malformed
  manifest, symlink, nested directory or invalid map, LVZ or settings file
  rejects the whole catalog. A few extra regular files (a README, say) are
  ignored.
- Bounds: at most 64 zones, 64 LVZ files per zone, 1,048,559 bytes per map or
  LVZ file, 256 KiB for settings or news, and 32 MiB of transferable files per
  zone.
- A new zone omits `override`. Replacing an embedded zone needs both
  `override=<same id>` in the manifest and `allow_zone_overrides = true`. The
  checked-in configs enable overrides and ship same-id bundles for both
  embedded zones.
- Disk bundles are supported on Linux, Android, macOS, iOS and the BSDs. On
  other targets a configured root fails startup.

To serve a different catalog from the image, mount it read-only and point the
server at it:

```sh
docker run --rm --network host \
  -v /srv/zubspace/zones:/srv/zones:ro \
  -e ZUBSPACE_SERVER_ZONE_ROOT=/srv/zones \
  -e ZUBSPACE_SERVER_BIND=127.0.0.1:9001 \
  -e ZUBSPACE_SERVER_UDP_PORT_RANGE=9002-9501 \
  zubspace/server:dev --config /app/config/container.toml --zone my-zone
```

Bundle contents are trusted operator configuration; protect the directory with
ordinary file ownership.

## UDP ports and capacity

The vendored webrtc-rs 0.21 stack gives each peer connection its own UDP
socket, so the server holds one port per admitted RTC session.

- **`ephemeral`** (the bundled default) lets the OS choose ports. It suits
  local runs but cannot be forwarded or firewalled statically.
- **`MIN-MAX`** must provide at least `max_players` ports; startup fails
  otherwise. Startup also binds every port in the range once and exits naming
  the first unavailable port.

A port that fails to bind at admission, or is not released in time after a
session closes, is quarantined. A background worker keeps retrying it with
backoff, and the port returns to service once it binds again. A session's port
is reserved before ICE gathering starts, so half-open offers count against
capacity, and offers beyond the usable capacity get `503` with
`Retry-After: 1`. `/v1/status` reports `rtc_capacity`,
`rtc_effective_capacity`, `rtc_udp_port_capacity`, `rtc_udp_ports_in_use`,
`rtc_udp_ports_degraded` and `rtc_udp_degraded_ports`.

The Compose stack uses `9002-9501`; production uses `9402-9901` (prod) and
`9902-10401` (dev), each 500 ports for `max_players = 500`. Keep the range, the
firewall and any port forwarding in step.

## Network reachability

What the server advertises in its answer depends on `rtc_udp_bind_ip`:

- **Empty** binds `0.0.0.0` and advertises a `0.0.0.0` host candidate. This
  works only when the browser runs on the same machine.
- **An IPv4 address** binds and advertises exactly that address. It must be a
  canonical dotted-decimal unicast address assigned to an interface in the
  server's network namespace (a host-network container sees the host's
  interfaces). This is independent of the HTTP `bind`.

`rtc_external_ips` (`ZUBSPACE_SERVER_EXTERNAL_IP`) is not supported: a
nonempty value fails startup, because the stack does not apply that
host-candidate remapping to the answer.

**Public IPv4 on the interface** is the simplest public topology and the one
production uses: set `rtc_udp_bind_ip` to the public address, open the UDP
range in the firewall, and configure no ICE servers. The browser's connectivity
checks reach the server's host candidate, and the server learns the browser's
NAT mapping as a peer-reflexive candidate. Because only the client side is
behind a NAT, this works for any client NAT type, including symmetric NAT, as
long as outbound UDP is allowed. A host that maps its public IP onto a private
interface (common on large cloud providers) does not fit this topology.

**STUN and TURN** can be configured on both sides when the server is not
directly reachable; production uses neither, so verify the selected candidate
path ([below](#verifying-a-deployments-network-path)) before relying on them.
Both sides accept only a canonical subset: `stun:host:port` with no query, or
`turn:host:port?transport=udp` with a username and credential. Schemes and
hosts must be lowercase, hosts a DNS name or dotted-decimal IPv4 (no IPv6
literals; server sockets are IPv4 only), and ports explicit. `stuns:`,
`turns:` and TURN over TCP are rejected, and `relay` policy requires a TURN
URL. There is no fallback for networks that block outbound UDP.

```toml
rtc_ice_urls = "stun:stun.example.net:3478,turn:turn.example.net:3478?transport=udp"
rtc_ice_username = "user"
rtc_ice_credential = "secret"
rtc_ice_policy = "all"   # or "relay"
```

The browser takes its ICE settings from the page URL: repeatable `ice=` URLs,
`ice_username`, `ice_credential` and `ice_policy`. TURN credentials in a page
URL are visible to anyone with the link; hand out short-lived ones.

`rtc_udp_path_mtu` limits SCTP packets for constrained paths such as
Tailscale. With an explicit MTU the complete SCTP packet is capped at
`min(MTU − 80, 1228)` bytes (1200 at 1280); 0 or unset uses the stack's
1191-byte default. Larger messages are fragmented, so message limits are
unchanged.

## Signaling security

The HTTP listener speaks plaintext HTTP and is meant to sit behind a trusted
proxy on a private interface. A public deployment should:

1. **Keep `bind` private.** Only the proxy may reach it. With
   `signaling_trust_proxy_headers = true` the server believes `X-Real-IP`,
   `X-Forwarded-For` and `X-Forwarded-Proto`; a reachable listener would let
   callers forge them.
2. **Terminate TLS at the proxy** and set `signaling_require_https = true`. The
   server then rejects offers without `X-Forwarded-Proto: https` (`403
   https_required`). This setting requires
   `signaling_trust_proxy_headers = true`.
3. **Set `signaling_allowed_origins`** to exact origins such as
   `https://play.example.net` (scheme and host only, no path or credentials).
   `*` allows any origin and cannot be combined with exact origins.
4. **Rate-limit at the edge.** The server also enforces a fixed one-minute
   window per client address, `signaling_offer_rate_per_minute` (environment
   `ZUBSPACE_SERVER_SIGNALING_OFFER_RATE`, flag `--signaling-offer-rate`,
   default 240).
5. **Optionally require a token.** `signaling_auth_token` makes the server
   demand `Authorization: Bearer <token>`, compared in constant time against
   that one static value. It gates RTC admission only. The browser sends a
   token from `window.__zubspaceRtcToken` or the `rtc_token` URL parameter, and
   only to an offer endpoint on the page's own origin. For per-player
   credentials, put a proxy in front that validates them and substitutes the
   static backend token; never ship that token in the page.

Built-in bounds: 8 KiB request headers, 10-second socket read and write
timeouts, `signaling_max_offer_bytes` (default 128 KiB, at most 1 MiB), a
10-second server ICE-gathering limit and a 20-second limit for the DataChannels
to open after the answer. The browser gives the whole exchange 20 seconds and
its own ICE gathering 15 seconds; the page URL parameters
`signaling_timeout_ms` and `ice_gather_timeout_ms` (1000–60000) change them.
Error responses are listed in the
[public server API](public-server-api.md#signaling).

Production's settings, from `deploy/prod/zubspace_server.toml`:

```toml
bind = "127.0.0.1:9001"
signaling_allowed_origins = "https://zubspace.gg"
signaling_require_https = true
signaling_trust_proxy_headers = true
signaling_offer_rate_per_minute = 600
```

By default the browser sends its offer to `/v1/rtc/offer` on the page's own
origin. The `rtc=<absolute offer URL>` parameter points it at another server
for unauthenticated development; with a token present, a cross-origin offer
URL is refused before any peer connection is created.

## Verifying a deployment's network path

`make test-webrtc-capacity` builds the release server and opens 200 real
`RTCPeerConnection`s from headless Chrome on the local machine (set
`ZUBSPACE_RTC_LOAD_CLIENTS` to change the count). It checks that every peer
opens all three channels, joins, exchanges snapshots and acknowledgements, and
that sessions and ports return to zero afterwards. Locally it requires every
peer to select a `host->prflx` path. This is a socket and concurrency check,
not evidence about NATs.

To check a real network path, run `node tools/webrtc_capacity_load.mjs` from a
client network against an idle deployed server:

- `ZUBSPACE_RTC_LOAD_OFFER_URL`, `ZUBSPACE_RTC_LOAD_STATUS_URL` and
  `ZUBSPACE_RTC_LOAD_BROWSER_URL` (a benign page on an allowed origin) select
  the server; `ZUBSPACE_RTC_LOAD_SIGNALING_TOKEN` supplies a token.
- `ZUBSPACE_RTC_LOAD_EXPECT_CANDIDATE_PATHS` is required in this mode: a
  comma-separated list of acceptable `local->remote` paths using `host`,
  `srflx`, `prflx` or `relay`.
- `ZUBSPACE_RTC_LOAD_ICE_URLS`, `_ICE_USERNAME`, `_ICE_CREDENTIAL` and
  `_ICE_POLICY` configure the test browser only; configure the server's ICE
  settings separately.

The remote server needs a fixed UDP range large enough for the run; the runner
checks one in-use port per peer during load and none afterwards.

## Limits

Zubspace keeps all game state in memory, so a restart disconnects everyone and
loses scores. One process runs one active zone, with no replication across
processes. DNS, certificates, edge authentication and DDoS protection belong to
the deployment.
