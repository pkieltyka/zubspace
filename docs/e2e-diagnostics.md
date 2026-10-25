# E2E diagnostics reference

How to read the opt-in diagnostic profiles and witnesses that browser E2E
gates and measurement probes produce. A run with any of these enabled is
diagnostic evidence, never an acceptance result. The harness itself is
described in [`tests/e2e/README.md`](../tests/e2e/README.md).

## Projectile velocity units

Convert velocities before comparing them with world positions or browser
motion metrics. The fields deliberately use different units:

| Source | Unit |
| --- | --- |
| Arena `BulletSpeed`, `BombSpeed`, `BurstSpeed` | Tenths of a pixel per second |
| `server_projectile_spawned` `velocity_x` / `velocity_y` (also QA trace `velocityX` / `velocityY`) | Tenths of a pixel per second, including inherited ship velocity |
| `server_repel_entity_state` `velocity_x` / `velocity_y` | Pixels per second |
| Browser ship and remote-actor motion metrics | Pixels per second |

`arena_projectile_initialization_tuning`
(`src/bin/zubspace_server/arena_settings.rs`) divides the settings by ten. The
projectile registry integrates a precise pixel-per-second velocity and falls
back to the wire value divided by ten only when none is recorded. For
example, Chaos `BulletSpeed=2000` is 200 px/s, and a spawn log vector of
`(-1334, -4099)` is about 431 px/s. `tests/e2e/bot_combat_evidence.test.mjs`
(in `make test-quick`) checks that the browser pilots lead targets with world
speed rather than wire velocity.

## Frame cadence and main-thread tasks (`trace=1`)

With `trace=1`, `index.html` installs a QA recorder; without it,
`window.__zubspaceFrameDiagnostics()` reports `enabled: false` with empty
accumulators. The snapshot contains:

- **`cadence`**: every animation callback, split into `publishing` (the
  callback published a game frame) and `other`. Each group has **duration**
  (how long the callback ran) and **lateness** (how long the browser waited to
  start it), with sums, maxima and buckets at
  1/2/5/10/20/34/50/80/120/200/400 ms (`bucketBoundsMs`). Values are
  cumulative for the page's life; compute a window mean by subtracting sums,
  never by subtracting means. `cadence.bySource` splits `other` by the
  callback's `Function.name`, capped at 24 labels. A callback that published is
  always counted as `publishing`, whatever its name.
- **Incidents**: frame gaps of 80 ms or more, with the surrounding samples and
  overlapping work entries (work of 50 ms or more).
- **`tasks` and `slowTasks`**: from `installQaTaskWitness`, which wraps the
  loader's XHR, audio decode and WASM seams before the first asset request,
  because some join-path work never runs inside an animation callback. Each
  kind has `count`, `sumMs`, `maxMs` and `maxAtMs`; spans at or above
  `taskFloorMs` (25 ms) are kept in `slowTasks`, so a reader can find what
  covered a frame gap.

Kinds ending in `-resolved`, `-rejected` or `-waited` measure the time from a
call to its asynchronous result, which happens off the main thread. Only the
other kinds (a callback's own execution time) are main-thread cost.

`trace=1` collects without showing anything. The visible QA marker and export
controls also need `debug=31337`; automated collectors call
`window.__zubspaceMarkQaIssue` directly.

## Measuring frame rate directly

Do not derive a frame rate by dividing HUD metric publications by elapsed
time; publications are not a frame clock. Use a direct witness:

- `make bench-frame-cadence` (`tests/e2e/frame_cadence_probe.mjs`) counts
  `requestAnimationFrame` callbacks over two five-second windows and collects
  Chrome long tasks, reading them twice about 25 seconds apart: a duration that
  grows is a task still running, one that does not is a completed stall.
  `PROBE_ARENA` selects the zone and `PROBE_PEER=1` adds a second client. It
  needs the local static server and a game server already running.
- `make bench-client-cpu` (`tests/e2e/cpu_profile_probe.mjs`) samples a fresh
  tab's CPU profile; see `AGENTS.md` for its options.

Chrome reports long tasks only for the page's own work; a script the driver
ran through `Runtime.evaluate` does not appear.

## Readiness timeouts name a blocked main thread

`tests/e2e/combat_replay.mjs` counts, per page label, every DevTools command
that exceeds its budget (5 seconds by default), and `waitUntil` appends that
ledger to its timeout message:

```text
timed out waiting for carrier entered the game; page main-thread starvation -> carrier: 3 timed out
DevTools command(s) (last Runtime.evaluate), unresponsive 8004ms, worst organic long task 730ms at t=4903ms
```

`unresponsive` is measured from the last command the page answered, so it is
the stall length even when no long-task witness is readable. The witness
(`PerformanceObserver` for `longtask`, installed right after `Page.navigate`)
is read only after the page answers again, after a 500 ms delay, because
Chrome reports a long task only once it ends. An empty witness beside a large
`unresponsive` figure means no organic long task, not that nothing happened.
A clean run adds no note. `tests/e2e/starvation_witness.test.mjs` covers this.

## Foreign X displays are named, not refused

`tools/e2e_gate_hygiene.mjs` refuses leftovers of an interrupted lane: an
orphaned test browser, an orphaned `Xvfb`, a stale `/tmp/.X<N>-lock`. A live
`Xvfb` that is not in the harness's process tree, not adopted by init, not
under an `xvfb-run` wrapper (including Nix's `.xvfb-run` shape) and not serving
the current `$DISPLAY` is reported through `foreignXDisplays` instead, and
`loadMeasurementNote` adds it to the machine-load note in failure output:

```text
; machine load 1m=4.17 (a saturated machine invalidates this measurement); foreign X display live during
this run -> :78 pid 1589221 (not refused: it is not ours)
```

`tests/e2e/gate_hygiene.test.mjs` covers both wrapper shapes.

## Moving peers

`tests/e2e/moving_peer_load.mjs` runs several headed Chrome clients that fly
with genuine keys at the same time. Both make targets require every peer to
be ready, an overlapping ten-second admitted-motion window with no gap above
10 server ticks, key release, and transport, security and queue cleanup.

| Target | Graphics | Display |
| --- | --- | --- |
| `make test-e2e-moving-peers` | `swiftshader` (software ANGLE) | One isolated Xvfb screen per peer |
| `make test-e2e-moving-peers-gpu` | `native` (Chrome's normal GPU selection) | The local desktop X/Xwayland display |

The native lane fails, rather than falling back to software, unless every
peer reports an AMD, NVIDIA or Intel WebGL renderer and CDP
`SystemInfo.getInfo` shows hardware WebGL and GPU compositing enabled. When
the user's Xwayland exposes only an `X<n>_` socket, the harness addresses it
through a temporary symlink that it removes on exit
(`tools/e2e_desktop_display.mjs`). Under Hyprland, the
`scripts/e2e-hyprland-tools` rule enables `render_unfocused` for test windows
only, so the lane runs at the compositor's unfocused frame cap (configured by
the user, not the harness).

`ZUBSPACE_E2E_MOVING_GRAPHICS_BACKEND=vulkan` selects a legacy Xvfb Vulkan
comparison. Its renderer string does not prove hardware presentation; that
path can still read frames back on the CPU.

Results record:

- `graphicsRig`: requested backend, display path and the renderer strings
  actually seen.
- `graphicsEvidence`: each peer's browser version and GPU feature status.
- `motionContract.classification`: `motion-floor-met`;
  `rig-bound-ceiling-unclassified` for a gap failure on the software rig, with
  `renewCommand` naming the GPU lane; or `product-motion-floor-failure` for a
  gap failure on a hardware rig. Rig choice alone never demotes a passing run.
- `diagnosticOnly`: true when any profiling, attribution, capture diagnostic,
  CPU pinning, `ZUBSPACE_SERVER_POSITION_TIMING=1` or
  `ZUBSPACE_E2E_STATIC_ROOT` override is active. Such a run reports
  `ok: false`.

| Variable | Effect |
| --- | --- |
| `ZUBSPACE_E2E_MOVING_PEERS` | Peer count, 2–200 (default 8) |
| `ZUBSPACE_E2E_BACKGROUND_RTC_PEERS` | Extra stationary protocol-only RTC peers (total at most 200) |
| `ZUBSPACE_SMOKE_LOG_LEVEL` | Server log level; the make targets use `info`, `debug` is the verbose stress profile |
| `ZUBSPACE_E2E_MOVING_PROFILE_LAST`, `ZUBSPACE_E2E_MOVING_COMPOSITOR_TRACE` | Profile the last peer's browser |
| `ZUBSPACE_E2E_MOVING_ATTRIBUTION` | Page attribution (below) |
| `ZUBSPACE_E2E_MOVING_SERVER_CPUSET`, `ZUBSPACE_E2E_MOVING_BROWSER_CPUSET` | Pin server or browsers to CPUs |

### Page attribution (`ZUBSPACE_E2E_MOVING_ATTRIBUTION=1`)

Installs the shared page witness (`tests/e2e/long_task_witness.mjs`) in every
peer. Its probes occupy the thread they measure, so the run is diagnostic. It
separates a peer whose thread was busy from one that received no frame, and
from one that was installing a zone archive. `attributionEvidence` has one
entry per peer:

- `frameCost`: the distribution of every animation-callback interval in the
  window (count, median, p95, maximum, and counts at 34/50/66/80 ms), not only
  intervals past the gap floor.
- `witnessAtWindowStart` / `witnessAtWindowEnd`: the witness's own state at
  both ends, including when each producer started
  (`frameSamplerStartedAtMs`, `starvationProbeStartedAtMs`). In Chrome the
  frame sampler is attached at peer readiness and covers only the time after
  it.
- `starvationWindowMaximumMs`: the longest wait seen by a probe that re-posts a
  zero-delay timer to itself.
- Each attributed frame gap (80 ms floor) carries `gapMs`, the probe's
  observations and an `attribution`:
  - `main-thread-busy`: the probe saw a wait of at least 34 ms, or ticked so
    sparsely that it was itself stuck behind a long task.
  - `frame-not-delivered`: the probe ticked densely; the thread was free and
    the browser produced no frame.
  - `timer-starved-while-rendering`, `probe-silent`, `probe-unsupported`: the
    probe cannot attribute the gap.
- `merge`: the client's LVZ merge counters read at both ends (`installCount`,
  `mergeTotalMs`, `mergeMaxMs`, `objects`, `textures`, `zoneFilesReady`), with
  `installGrowthInWindow` and `mergeMsGrowthInWindow` showing whether an
  archive was installed during live play.
- An entry with `unavailable` or `readError` means that peer produced no
  attribution, not that nothing stalled.

### Server timing (`ZUBSPACE_SERVER_POSITION_TIMING=1`)

Adds INFO records to the server log. All durations are elapsed wall time,
including scheduling, measured inside the server; none measure UDP arrival or
affect gameplay, readiness, deadlines or retransmission.

- **Position packets**: a queue-entry witness when the DataChannel callback
  enqueues, then runtime lookup and decode, joined-lock wait, handling and
  total time after the handler pops the queue.
- **Other packets slower than 5 ms**: packet type and subtype, lookup,
  joined-lock and handling times, plus arena-gate and session-ingress lookup
  and wait, identity check and dispatch. At most 16 records are written per
  one-second window; the suppressed count is carried into the next record. No
  payload bytes are logged.
- **Bootstrap reliable progress**: up to four records per session (reserved,
  sink complete, first exact ACK, all exact ACKs) with frame counts and
  elapsed time. Duplicate or stale ACKs add none.

## RTC cleanup evidence (`rtc_ingress_execution`)

`/v1/status` reports `rtc_ingress_execution`: `limit` and `workers`
describe the bounded handler pool, `in_flight` counts reserved, submitted or
running turns, and `waiting` counts callers waiting for a slot. Registration
work shares this pool with packet turns. Live traffic may have turns in
flight, and idle workers remain while the server listens. Disconnect cleanup
evidence requires `in_flight` and `waiting` to be zero as well as empty packet
queues.

## Server status in E2E evidence

`tools/game_status.mjs` builds a QA-only view from `/v1/status`, `/v1/zones`
and `/v1/players` (read concurrently) and then the active zone's `/status`,
under one five-second deadline. `qaStatusSource` records the paths.
`joinedPlayers` and `botPlayers` are derived from the combined roster,
`snapshot_acked_clients` comes from zone status, and the process `total` stays
separate from `zoneTotal`. The reads can observe different moments during a
join or leave, so the view assumes no cross-request consistency. Missing
scopes, unknown schemas and missing diagnostics fail closed. The endpoints are
documented in [Public server API](public-server-api.md).

## Free-for-all and the team fixture

Servers are free-for-all: humans and bots get distinct frequencies, frequency
change requests are refused, and `/v1/zones` reports `free_for_all`.

Debug server builds accept `ZUBSPACE_SERVER_TEST_TEAM_MODE=1`, which restores
team play. `combat_replay.mjs` sets it for its semantic-input,
baseline-receipt and `ZUBSPACE_E2E_PRIZES_PHASE1` lanes, and
`rendezvous_focus_probe.mjs` sets it too. Release builds compile it out. Results from those lanes say nothing about free-for-all
rules. The client enables the `=N` frequency command only when the zone
catalog reports `free_for_all: false`; a missing catalog fails closed.

## Debug and trace URL options

| Option | Effect |
| --- | --- |
| `debug=31337` | On-screen text HUD, energy labels on other ships, the F9 developer overlay, the status strip and the QA marker/export controls (with `trace=1`) |
| `trace=1` | QA trace and frame/task recorders, without visible controls |
| `metrics=1` | Low-rate HUD metrics for test assertions (implied by `trace=1`) |
| `packetlog=1` | Browser packet log |

Ordinary URLs set none of these; `make test-e2e-ordinary-url` checks that the
client then requests no diagnostic payloads.

## QA trace uploads

The local static server (`tools/dev_static_server.mjs`), the ordinary-URL
browser helper and the release nginx frontend forward exactly
`POST /v1/qa/trace` to the game server, through `tools/game_http_proxy.mjs`
locally. The game server owns authentication, the origin check, the 256 KiB
size cap and the upload rate limit. Uploads are accepted only when the server
runs with `ZUBSPACE_SERVER_QA_TRACE_UPLOAD=1`; otherwise the endpoint answers
404, which is different from a missing proxy route. Retrieval
(`GET /v1/qa/trace/<id>`) is served only on loopback by the game server and
must not be proxied. `make test-nginx-qa-proxy` checks the release nginx
template in a temporary local container.

## Graphics backend for keyboard lanes

`tests/e2e/keyboard_chord_matrix.mjs` runs Chrome with software ANGLE
(`swiftshader`) by default and Chrome's normal renderer selection (`native`)
when `ZUBSPACE_E2E_HIGH_REFRESH=1`, because uncapped software rendering
blocks inside GL buffer uploads. Set
`ZUBSPACE_E2E_KEYBOARD_GRAPHICS_BACKEND=swiftshader` to reproduce the
uncapped software stress case.

## Firefox-only environment

`ZUBSPACE_FIREFOX_BROWSER_ENV` passes environment variables to the Firefox
process only (geckodriver's `moz:firefoxOptions.env`), as a JSON object or a
`;`- or newline-separated `KEY=VALUE` list. Use it for browser diagnostics
such as `MOZ_PROFILER_*`, which break geckodriver when set in its own
environment.
