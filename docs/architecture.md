# Zubspace architecture

Zubspace is SubSpace / Subspace Continuum rebuilt for the browser. A Rust game
client compiled to WebAssembly runs in the page, and a Rust game server owns
the multiplayer state. They talk over WebRTC DataChannels after one HTTP
signaling request. This page shows the pieces, how data moves between them,
and where each part lives; [Further reading](#further-reading) goes deeper.

## Design goals and boundaries

- Make local input immediate through prediction, while requiring the server
  to validate movement and own combat, progression and lifecycle changes.
- Share deterministic rules between runtimes so fixes to movement or weapon
  physics do not need separate client and server implementations.
- Handle reordering and ordinary packet loss with input history, acknowledged
  snapshot bases, event deduplication and exact application receipts.
- Spend bandwidth on current nearby state. Bound queues and recovery work,
  replace stale unsent snapshots, and preserve one-shot gameplay facts.
- Support useful play beyond a LAN through a server-controlled presentation
  delay and measured connection-quality policy. A smooth view does not let a
  client choose its collision rewind or override authoritative state.

One process currently hosts one active zone. The configured player limit is an
admission cap, not a claim of demonstrated combat capacity. Test connection
capacity, moving clients and combat separately for the deployment being used.
The browser's stored numeric identity is not an authenticated account;
signaling admission controls and gameplay validation serve different purposes.

## The pieces

**Game client (Rust to WASM).** One Cargo package, `zubspace`, builds two
binaries. The client binary, also `zubspace`, has its crate root in
`src/main.rs` and compiles to `zubspace.wasm` for `wasm32-unknown-unknown`. It
uses a Macroquad fork pinned in `Cargo.toml` (`github.com/pkieltyka/macroquad`
rev `25b88535a93d0422c7b9ee9a86dba4c026541612`); `js/mq_js_bundle.js` is the
matching JavaScript loader, and `tools/macroquad_pairing.mjs` checks the pair.
Art and sounds from `assets/graphics/` and `assets/sound/` are compiled into the
WASM; zone files are fetched at runtime ([Zone assets](#zone-assets)). A native
build of the same binary runs an unnetworked local game with client-side bots.

**Page shell (`index.html`, `js/`).** `index.html` holds the canvas, the main
menu and the bootstrap script. It loads the `js/` helpers (zone catalog, zone
loading and cache, menu, RTC configuration, telemetry), the Macroquad loader,
then the WASM. It owns the browser side of networking: the `RTCPeerConnection`,
its three DataChannels, and the `zub_*` imports the WASM calls to connect, send
and receive. The repo-root page loads the debug WASM from
`target/wasm32-unknown-unknown/debug/`; `tools/build_wasm_release.mjs` writes
the release bundle to `dist/` with those paths rewritten.

**Game server (`src/bin/zubspace_server.rs`, `src/bin/zubspace_server/`).** The
second binary, native only. One TCP listener serves signaling and a small
public HTTP API, WebRTC sessions run on a tokio runtime, and the main thread
drives a fixed-rate tick. Settings come from `config/zubspace_server.toml`,
CLI flags and `ZUBSPACE_SERVER_*` environment variables.

**Shared code.** There is no library target. The client's crate root declares
every module under `src/`; the server pulls in the files it shares with
`#[path = "../..."]` attributes in `src/bin/zubspace_server.rs`. Both binaries
compile the movement and gameplay rules (`src/gameplay/player_movement.rs`,
`src/gameplay/shared_rules.rs`), the authority helpers (`src/authority*.rs`,
`src/authority/`, `src/authority_projectiles/`), the combat view schedule
(`src/combat_timeline.rs`), the Zubspace wire formats both sides speak (for
example `src/protocol/snapshot.rs`, `realtime_event.rs`,
`movement_correction.rs`), and Continuum helpers (`src/continuum*.rs`,
`src/zone_wire.rs`, `src/lvz/archive.rs`, `src/bm2.rs`). The server loads maps
and arena settings with its own code (`zone.rs`, `arena_settings.rs`).

**Vendored WebRTC (`vendor/`).** `rtc-0.21.1`, `rtc-sctp-0.21.1` and
`webrtc-0.21.1` replace the crates.io releases through `[patch.crates-io]`.
Each `ZUBSPACE-PATCH.md` lists the local changes: applying a remotely opened
channel's ordering and lifetime to its SCTP stream, SACKs riding in outgoing
DATA packets, and opt-in write coalescing. [Vendoring](vendoring.md) maps the
patches, upstream fixes and the separate library/integration checks.

**Join notifier (`notifier/`).** A separate TypeScript service that polls
`/v1/players` and `/v1/zones` and posts human joins and leaves to Slack or
ntfy (template config: `notifier/etc/notifier.sample.json`).

**Deployment.** `Dockerfile.server` builds the server image; `Dockerfile.client`
builds `dist/` into an nginx image whose `docker/nginx.conf.template` also
proxies signaling and the public API to the server. `docker-compose.yml` runs
both locally. `deploy/` is production: prod and dev on one VPS as rootless
Podman Compose projects behind Cloudflare tunnels, with game UDP going straight
to the host. `scripts/deploy.sh` and `scripts/remote.sh` drive it.

## How a session flows

```text
  Browser tab                                  Host
 +------------------------------------+       +----------------------------------------+
 | index.html + js/                   | HTTP  | static server                          |
 |   menu, zone prefetch, IndexedDB   |<----->|   local: tools/dev_static_server.mjs   |
 |   cache, RTC bridge (zub_* imports)|       |   prod:  nginx (Dockerfile.client)     |
 |                                    |       |   index.html, js/, *.wasm, assets/zones|
 | zubspace.wasm (src/main.rs)        |       |   proxies /v1/rtc/offer, /v1/zones, .. |
 |   input -> 60 Hz prediction -> send|       +-------------------+--------------------+
 |   snapshots/events -> interpolate  |                           | SDP offer -> answer
 |   -> draw                          |       +-------------------v--------------------+
 |                                    |  UDP  | zubspace_server                        |
 |   RTCPeerConnection                |<=====>|   HTTP listener (127.0.0.1:9001 local) |
 |     zubspace-game                  |       |   WebRTC sessions (tokio + vendor/)    |
 |     zubspace-events                |       |   ServerState <- tick loop (60 Hz)     |
 |     zubspace-reliable              |       |     positions, bots, projectiles,      |
 +------------------------------------+       |     damage, AOI snapshots, events      |
                                              +----------------------------------------+
```

1. **Load.** A static server delivers the page: `tools/dev_static_server.mjs`
   on port 7000 locally, the nginx container in production. Both forward a
   fixed allowlist of `/v1/*` paths to the game server, so the page needs no
   URL parameters.
2. **Menu.** The menu lists zones from `/v1/zones`, checks `/v1/players` before
   joining, and prefetches the selected zone's files and the WASM. On a browser's
   first Play/Enter submission, the keyboard guide opens before admission and
   signaling. “OK, I’m ready” continues the join; the acknowledgement is saved
   in localStorage once gameplay is reached. Dismissing it returns to the menu.
   Failed joins retain the acknowledgement only on the current page. If storage is unavailable, the
   acknowledgement lasts for the current page. Explicit name/auto-join URLs
   retain their direct-entry behavior.
3. **Signaling.** The page creates an `RTCPeerConnection` with three
   DataChannels, gathers ICE candidates, and POSTs the SDP offer as JSON to
   `/v1/rtc/offer`; the HTTP response carries the answer. All game traffic
   then uses the DataChannels. There is no fallback transport.
4. **Join.** On the reliable channel the client logs in and enters the arena.
   The server sends arena settings, advertises the zone's files with SHA-256
   digests, sends the ones the client lacks, and runs the map security check.
5. **Play.** The client predicts its own ship and sends positions and input
   records; the server validates them on its tick and returns snapshots and
   events.

The page opens the channels in `index.html`; the server checks their labels and
attributes in `src/bin/zubspace_server/rtc.rs`.

| Label | Delivery | Server to client | Client to server |
| --- | --- | --- | --- |
| `zubspace-game` | unordered, `maxPacketLifeTime` 80 ms | `0x70` snapshots, connection-quality probes, realtime event copies when `realtime_event_game_copy` asks | positions (`0x03`), input records (`0x62`), quality echoes |
| `zubspace-events` | unordered, reliable | realtime events (`0x7d`, batched `0x84`): shots, hits, projectile state | copies of positions that carry a weapon |
| `zubspace-reliable` | ordered, reliable | login, settings, zone files, chat, roster, ship and frequency changes, deaths, score, lifecycle, movement corrections | everything else, plus a third copy of weapon-carrying positions |

### Server tick

`main` in `src/bin/zubspace_server.rs` accepts HTTP connections (one thread
each), reads stdin admin commands (`status`, `players`, `reset`, `shutdown`),
and calls `server_tick::run_server_tick` every `1 / tick_rate` seconds, 60 Hz
by default. Game state is one `ServerState` behind `Arc<Mutex<...>>`. Inbound
DataChannel messages go through a bounded handler pool in `rtc.rs` to
`connection.rs`. Control packets such as chat apply on arrival, and input
records are stored on arrival. Positions are queued and applied by the next
tick in logical input order, since the channels can reorder them. A tick:

1. resends reliable packets whose acknowledgements are overdue;
2. updates each client's quality probes, send budget, door phase, energy
   recharge and short-lived weapon state;
3. applies queued positions: checks them against the shared movement core
   (`sim_observation.rs`), accepts or rejects weapon fire, and opens a
   movement correction for a client that has drifted from the simulation;
4. steps server bots and applies pending portal, brick and prize changes;
5. steps projectiles and turns hits into damage, deaths and score
   (`server_damage.rs`);
6. emits a snapshot to each joined client (`snapshot.rs`).

Snapshots are per recipient. `visibility.rs` sorts other actors by distance:
near ones (about 2,048 px) get field-masked deltas every tick up to 60 Hz, mid
ones (about 2,560 px) get radar positions at 30 Hz, farther ones are left out.
Each frame is a delta against the last snapshot the client acknowledged. A
per-client token bucket (`send_budget_kib_per_second`) caps the bytes; when it
runs low, distant ships hold their last acknowledged state. Per-hit damage
rides in the snapshot; deaths and score go on the reliable channel.

### Client frame

Each animation frame, `src/main.rs` captures keyboard edges, polls the RTC
bridge and applies server facts before any local gameplay (so a correction
lands first), advances doors and bricks on the server clock, runs whole 1/60 s
gameplay steps (`GAMEPLAY_STEP_SECONDS`, at most eight catch-up steps) that
predict the local ship with the shared movement core, sends the position on the
last step, then updates remote presentation and draws.

**Prediction and reconciliation.** The client sends each predicted position
with a rolling record of the inputs behind it. The server replays those inputs
and publishes positions that pass admission, while a separate drift observer
checks the accepted motion over time. Missing history, incompatible predicted
weapon effects or sustained disagreement can require a correction; the client
pauses realtime gameplay, reports the exact state it is correcting, and resumes
when the server confirms the rebased state. Energy, damage, deaths, prizes and
score always come from the server. Remote ships are drawn a server-chosen delay
behind real time (`observer_network_profile`, 200 ms ceiling) with Hermite
interpolation (`src/network/entities.rs`); other players' shots are drawn from
realtime events (`src/remote_weapon/`).

## Authority model

The server owns state; the client predicts and presents. The server decides
accepted positions, energy, bounty, weapons and cooldowns, projectiles,
collision, damage, death, respawn, score, prizes, bots and who sees whom. A
human's shot is tested against targets where that shooter's client was told to
draw them (`src/combat_timeline.rs`), and the server decides the hit. The
client owns feel and presentation: its own motion between corrections, its own
shots' visuals, interpolation, effects, sound and the HUD. The behavioural
reference for what a mechanic should do is the reference game client.
Two differences are deliberate. Units: orientation is in turns on `0.0..1.0` and timestamps are
Continuum 100 Hz ticks, so ported code converts units. Authority: where the
reference decides something locally, the Zubspace server decides it.

## Zone assets

A zone is a directory `assets/zones/<id>/` with a `.lvl` map, `.lvz` archives,
`arena.conf`, `news.txt` and a `zone.bundle` manifest pinning each file by
SHA-256. `chaos-zone` (the default) and `extreme` ship.

- **Server.** Both shipped zones are compiled in with `include_bytes!`
  (`src/bin/zubspace_server/zone/assets.rs`); `zone_bundle_root` adds disk
  bundles checked against their `zone.bundle`. One zone is active per process,
  and a login asking for another switches it only while nobody is joined.
- **Client build.** `build.rs` scans `assets/zones/` and generates the path
  lists `src/constants.rs` includes, and embeds the build commit.
  `tools/zone_asset_manifest.mjs` writes `target/zubspace-zone-assets.json`
  (`dist/zone-assets.json` in release) with every file's SHA-256.
- **Runtime.** The page fetches the selected zone's files over HTTP, six at a
  time, and answers the WASM's file loads from that pool or from the IndexedDB
  cache (`js/zubspace_zone_cache.js`). After login the client keeps only bytes
  whose digest matches the server's advertisement, tells the server which
  files it holds, and receives the rest on the reliable channel. Verified files
  are then cached.

## Source map

Server paths are relative to `src/bin/zubspace_server/`.

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | Client crate root and frame loop |
| `src/game.rs`, `src/game/` | `Game`: input actions, local simulation, applying server facts (`network/`), drawing (`render/`), audio, the client lifecycle sequencer |
| `src/network/`, `src/transport.rs` | Client view of the world (session polling, remote players and snapshots, interpolation); WASM side of the RTC bridge |
| `src/protocol.rs`, `src/protocol/` | Packet formats, Continuum-style and Zubspace-specific; client session state (`session/`) |
| `src/remote_weapon/` | Other players' weapons, drawn from server events |
| `src/assets.rs`, `src/assets/`, `src/map.rs`, `src/lvz/`, `src/bm2.rs`, `src/settings/` | Embedded art and sound; maps, LVZ archives, BM2 bitmaps, arena settings |
| `src/ui.rs`, `src/chat.rs`, `src/radar.rs`, `src/scoreboard.rs`, `src/input.rs` | HUD, chat, radar, scoreboard, keyboard |
| `src/gameplay/` | Engine-independent rules: movement, input records, weapons, spawn |
| `src/authority*.rs`, `src/authority/`, `src/authority_projectiles/`, `src/combat_timeline.rs` | Weapon acceptance, energy, status, prize rolls, projectile records; observer render schedule used for hit geometry |
| `src/bin/zubspace_server.rs`, `config.rs` | Server entry point and accept/tick loop; TOML, CLI and environment configuration |
| `connection.rs`, `http.rs`, `public_api.rs` | HTTP routes and client packet dispatch |
| `rtc.rs`, `rtc/`, `reliable.rs`, `session.rs` | WebRTC sessions and lane routing, reliable delivery, zone file and news transfer |
| `arena.rs`, `arena/`, `identity.rs`, `roster.rs` | Login, join, leave, zone selection, positions, weapons, portals, names, roster |
| `state.rs`, `state/`, `server_tick.rs`, `server_tick/`, `sim_observation.rs` | `ServerState`, the tick, server bots, movement validation |
| `server_damage.rs`, `server_explosions.rs`, `threat_presentation.rs` | Damage, death, score, explosion geometry, harm lead |
| `snapshot.rs`, `visibility.rs` | AOI tiers and per-recipient snapshots |
| `zone.rs`, `zone/`, `arena_settings.rs`, `arena_settings/` | Zone catalog, maps, settings profiles |
| `objectives.rs`, `objectives/` | Bricks; flags and powerballs stay off unless `enable_objectives` is set and the zone authors them |
| `doors.rs`, `hidden_prizes.rs`, `player_events.rs`, `spawn_idle.rs` | Doors, prizes, chat and death events, inactive-spawn timeout |
| `connection_quality.rs`, `connection_quality/` | Probe and echo measurement, forced spectator |
| `observability.rs`, `qa_trace.rs`, `client_debug.rs`, `security.rs` | Prometheus metrics, QA and debug traces, reject counters |

## Build and run locally

```sh
make dev                    # debug server + debug WASM client
make restart-local          # stop, rebuild, start server and static server
make restart-local-release  # same, serving the optimized dist/ bundle
make stop-server
make test-quick             # fast native and Node checks, no browsers
```

The client is at `http://127.0.0.1:7000/`, the game server at
`127.0.0.1:9001`, with logs in `/tmp/zubspace-server.log` and
`/tmp/zubspace-static.log`. The debug WASM is several times slower per frame
than release, so measure frame rate with `make restart-local-release`.

## Tests and tooling

Rust tests sit beside their modules and in `src/tests/` (client,
`make test-unit`) and `src/bin/zubspace_server/tests/` (server,
`make test-integration`).
`tests/*.test.mjs` are Node tests for the page shell and harnesses.
`tests/e2e/` holds real Chrome and Firefox scenarios with real keyboard input
over WebRTC to a local server, each behind a `make test-e2e-*` target. `tools/`
has the dev static server and proxy, release bundler, zone manifest, smoke and
capacity runners; `scripts/` has deploy and remote operations, Cloudflare
config and acceptance runners. `AGENTS.md` lists every target.

## Further reading

[Physics and presentation](physics.md) · [Networking](networking.md) ·
[Vendoring](vendoring.md) · [Authoritative server](authoritative_server.md) ·
[Protocol compatibility](protocol_compatibility.md) · [Gameplay](gameplay.md) ·
[Deployment](deployment.md) and the host runbook
[`deploy/README.md`](../deploy/README.md) ·
[Public server API](public-server-api.md) · [Observability](observability.md) ·
[Connection quality](connection-quality.md) ·
[Zone file cache](zone-file-cache.md) · [E2E diagnostics](e2e-diagnostics.md)
