# AGENTS

This file records the local workflow for Codex-style agents working in this repo.

## Build And Test

- Batch related fixes and review code as work progresses. Use `make test-quick`
  for the usual fast native/static check (no browsers or WASM rebuild), and
  smaller focused tests when investigating a specific change. Do not repeatedly
  run browser E2E between edits. Reserve broad E2E for roughly daily checkpoints
  or release validation, plus a narrowly justified browser diagnostic when a
  defect cannot be resolved through code/native evidence. Run full applicable
  acceptance after the final accumulated-code review, before release.

- Native checks: `cargo fmt --check && cargo test -p zubspace`
- Vendored RTC behavior: `make test-vendor-rtc` runs the library suite, including
  actual remote-open reply ordering and timed expiry on SCTP wire DATA. It is
  included in `test-quick`, `test-local-all` and focused acceptance lane 62.
- Fast native/server build: `make build`
- Server-only build: `make build-server`
- Debug WASM client build: `make build-client`
- Local browser-development build: `make dev`
- Release WASM client build: `make build-client-release`
- Every debug build (native client, server and WASM client): `make build-all`
- Docker images/stack: `make docker-build`, `make docker-up`, `make docker-down`
  (see `docs/deployment.md`)
  Image builds receive the full checked-out revision through
  `ZUBSPACE_BUILD_COMMIT`, retain it as an OCI label, and render its first eight
  characters in the client footer. Raw Docker/Compose builds from archives
  must supply a full lowercase commit explicitly; container builds reject an
  unknown revision. Do not copy Git metadata into the build context.
  `make check-container-bundles` checks both built images' bundled documents,
  source labels and client footer/BUILD_INFO without starting game listeners.

## Test Taxonomy

- Unit tests: `make test-unit` runs native game binary/unit coverage without browser processes.
- Server integration tests: `make test-integration` runs the `zubspace_server` binary test suite for server state, packets, authority, AOI, and lifecycle.
- Release server integration tests: `make test-integration-release` repeats the
  server suite with debug assertions disabled; required for the frozen gate.
  Special-weapon E2E can target the built release server with
  `ZUBSPACE_SERVER_QA_TRACE_UPLOAD=1 ZUBSPACE_E2E_SERVER_PROFILE=release node tests/e2e/special_weapons.mjs`.
  The explicit local QA option is required for complete release trace evidence.
- Browser smoke tests: `make test-smoke` is for shallow startup/connection/rendering checks only; do not treat smoke output as combat correctness proof.
- Compose RTC smoke: `make test-compose-rtc` builds and starts the actual Linux host-network images, opens the bundled app in Chrome through nginx, checks all three RTC channels plus join/snapshot ACK/cleanup status, and tears the stack down.
  To leave local play services running, provide all three test-only listener
  overrides: `ZUBSPACE_COMPOSE_CLIENT_PORT=18000`,
  `ZUBSPACE_COMPOSE_SERVER_PORT=18001`, and
  `ZUBSPACE_COMPOSE_UDP_PORT_RANGE=18002-18501` (500 UDP ports, covering the configured player limit).
  The smoke overlays only listeners and the matching healthcheck; shipped
  Compose defaults, images, host networking and acceptance assertions remain.
- Browser E2E tests: `make test-e2e-combat`, `make test-e2e-snapshot-loss`,
  `make test-e2e-spawn-idle` (real two-minute inactive spawn, active-life exemption and immediate return),
  `make test-e2e-thor-proximity` (stock Chaos Thor sensor and real offset primary damage),
  `make test-e2e-input-recovery`, `make test-e2e-firefox-input-recovery`,
  `make test-e2e-movement-correction`,
  `make test-e2e-semantic-input`,
  `make test-e2e-baseline-receipt`, `make test-e2e-warp-relocation`,
  `make test-e2e-wormhole-authority`,
  `make test-e2e-portal-phase2`, `make test-e2e-special-weapons`,
  `make test-e2e-stale-door`, `make test-e2e-clock-stall`,
  `make test-e2e-chat-send-failure`,
  `make test-e2e-dynamic-zones`,
  `make test-e2e-active-projectile-recovery`,
  `make test-e2e-single-bullet`, `make test-e2e-aimed-bullet`, and
  `make test-e2e-bomb` run real browsers, real keyboard input, and the real
  local WebRTC game server.
- Immediate gameplay matrices: `make test-e2e-immediate-bot-combat` runs the
  clean Chrome/Firefox x Chaos/Extreme combat matrix, while
  `make test-e2e-immediate-mild` repeats it under the seeded supported-mild
  delay/loss/duplication/reordering profile.
- Keyboard impairment matrix: `make test-e2e-immediate-keyboard-mild` runs the
  Chrome/Firefox x Chaos/Extreme keyboard/lifecycle cases with the seeded mild
  profile and requires realized network effects and drained timers for both
  the measured pilot and its lifecycle attacker.
- Fast local non-E2E gate: `make test-local-all`
- Frozen acceptance matrix: `make test-frozen-matrix` runs the canonical 15-step matrix through
  `scripts/frozen_matrix.sh`, and `make test-frozen-matrix-list` prints it. The runner prints one status line
  per step, stops at the first failure, refuses a dirty tree, and records the exact head plus per-step logs
  and `result.json` in one evidence directory.
- Focused acceptance lanes: `make test-focused-acceptance` runs `scripts/focused_acceptance.sh`, the
  authority/recovery/special-weapon lanes the canonical 15-step matrix does not cover, with the same
  dirty-tree refusal, head pin and per-lane logs plus `result.json`; `test-focused-acceptance-list`
  prints them. Together the two runners are the layered acceptance surface.
  Both runners verify the pinned HEAD and clean tree around each gate and
  before final reporting; a source change stops the run even with `--keep-going`.
  Results distinguish raw gate exits from provenance failures and report actual
  attempted counts. The focused list appends current release, UI, bot, brick,
  scoring and lifecycle scenarios after its original 24 lanes.
  `node --test tests/acceptance_runner.test.mjs` exercises these failure paths
  in temporary Git repositories without browsers; it is part of `test-quick`.
- Fast browser/capacity RTC configuration gate: `make test-rtc-config`
- Focused multiplayer correctness gate: `make test-multiplayer-correctness` runs server post-respawn authority regressions, the one-tap bullet E2E, and the full combat E2E.
- Random-spawn setup fragment: `make test-e2e-spawn-rendezvous` joins two stock
  Chaos clients, flies genuine-key hull-clear routes, holds an actual ship-change
  receipt, checks stable camera/hidden provisional ship, then releases the exact
  receipt and renews the encounter. This is not combat/death/Firefox proof.
  `rendezvousPlayers` is explicit setup; `waitForPlayersReady` remains an
  observation so it cannot undo a scenario's deliberately separated positions.
- Real WebRTC capacity gate: `make test-webrtc-capacity` runs 200 concurrent release-server handshakes/sessions by default and stays separate from synthetic simulation coverage.
- History lookup measurement: `make bench-history-lookup` runs the optimized
  production projection resolver over dense 200-actor/200-observer histories.
  It reports exact/interpolated/held/missing query cost and allocated history
  storage; it is not a live-server tick or WebRTC capacity acceptance gate.
- Client frame cadence measurement: `make bench-frame-cadence`
  (`tests/e2e/frame_cadence_probe.mjs`) counts a real client's
  `requestAnimationFrame` callbacks over two five-second windows, collects Chrome
  long tasks for the page, and prints one JSON line. `PROBE_ARENA` selects the
  zone and `PROBE_PEER=1` joins a second client and times a DevTools round trip
  with it. It needs the local static server and a game server for that arena
  already running, and it is a measurement, not an acceptance gate.
- Zone render-cost measurements: `make bench-lvz-scan` reports shipped-zone
  LVZ object counts and per-frame object-visit cost, and `make bench-lvz-merge`
  (`lvz_merge_cost_tests` in `src/lvz/render.rs`) times merging each zone's shipped
  archives into a `LvzRenderCache` with the image sections stripped, which separates
  the per-object work from texture upload. Both are ignored tests over the shipped
  assets; they are measurements, not acceptance gates.
- Client CPU profile measurement: `make bench-client-cpu`
  (`tests/e2e/cpu_profile_probe.mjs`) attaches to a fresh client tab over CDP,
  starts the sampling profiler before the page boots, and prints the hottest
  self-time frames as Rust symbols, because the WASM name section survives the
  build. `PROBE_ARENA`, `PROBE_SECONDS`, `PROBE_SAMPLE_US` and
  `PROBE_START_DELAY_MS` control the run - the last one profiles a known phase
  (for example `PROBE_START_DELAY_MS=8000 PROBE_SECONDS=6` covers a boot stall
  that begins a few seconds after load, without the idle frames around it). It
  needs the same local servers as the cadence probe and is a measurement, not an
  acceptance gate.
- Concurrent moving browsers: `make test-e2e-moving-peers` runs eight real
  Chrome/WASM clients on isolated Xvfb displays, with genuine keyboard thrust
  over WebRTC. It requires complete startup readiness, an overlapping ten-second
  admitted-motion window, key release and session/queue cleanup. The normal
  measurement uses INFO logging with authoritative input traces; set
  `ZUBSPACE_SMOKE_LOG_LEVEL=debug` for the separately retained verbose stress
  profile. `ZUBSPACE_E2E_MOVING_PEERS` sets the explicit peer count (2–200).
  `make test-e2e-moving-peers-gpu` runs the same contract on a local desktop
  X/Xwayland display using ordinary Chrome graphics. It asserts a real adapter
  and hardware WebGL/compositing status for every peer, failing on a GPU-less
  or desktop-less host instead of silently becoming a software run. Both rigs
  are acceptance-capable and retain `graphicsRig` display/renderer evidence;
  `graphicsEvidence` includes per-peer browser version and feature status.
  A software-raster floor failure is `rig-bound-ceiling-unclassified`, with
  `renewCommand` naming the native GPU lane; a native-adapter failure is
  `product-motion-floor-failure`. The old Vulkan/Xvfb mode remains an explicit legacy comparison:
  hardware WebGL identity alone does not establish hardware presentation or
  rule out costly CPU readback. Rig choice alone does not demote a passing run. Its historical verdicts remain intact. The native background lane
  respects the compositor's existing frame cap (15 fps on this host); no
  clean 60 fps window is assumed. See [`docs/e2e-diagnostics.md`](docs/e2e-diagnostics.md).
  This movement-only case is not combat, Firefox, or 200-peer capacity proof.
  The opt-in server-timing/compositor profiles
  (`ZUBSPACE_SERVER_POSITION_TIMING=1`, `ZUBSPACE_E2E_MOVING_PROFILE_LAST=1`,
  `ZUBSPACE_E2E_MOVING_COMPOSITOR_TRACE=1`) are diagnostic-only, never
  acceptance; `ZUBSPACE_E2E_MOVING_ATTRIBUTION=1` additionally installs the
  frame/long-task/starvation witness and the client's LVZ merge counters in every
  peer page, which occupies the thread it measures, so it also marks the run
  diagnostic-only. See [`docs/e2e-diagnostics.md`](docs/e2e-diagnostics.md) for the
  witness semantics.
- Real DataChannel contract gate: `make test-webrtc-channel-contract` rejects mismatched DCEP reliability in Chrome and accepts the exact Chrome/Firefox three-channel contract.
- Bot authority gates: `make test-e2e-bots` runs headed Chrome bots against the
  server bot-authority checks; `make test-e2e-bots-firefox` is the Xvfb Firefox
  variant. `ZUBSPACE_E2E_JOIN_MIN_UPTIME_SECONDS=120` selects the aged-join variant.
- Aged-server cross-arena authority gate: `make test-e2e-aged-server-authority`
  runs the real server in Chaos until it has at least 350 seconds of wall
  uptime, then Chrome and Xvfb Firefox join Extreme with two bots and complete a
  four-minute genuine-key motion window plus the exact-life combat,
  stationary-fixture, facing, and fresh-counterfire audits on that aged join and
  arena switch. 350 seconds exceeds the 327.68-second u16 position-timestamp
  half-range, so a freshly booted server cannot exercise this admission path.
- Reference Extreme bot-authority gate: `make test-e2e-bots-reference-extreme`
  retains the Chrome/Firefox authored starting-loadout direct-bomb cases.
  Ordinary bot-authority and aged-server lanes explicitly require maximum
  entry and exact proximity bombs in both zones. Each lane checks its actual
  server loadout policy and records the fixture; an aged lane cannot select
  reference entry. This reference gate is focused acceptance lane 60.
- Immediate quality aggregate: `make test-e2e-immediate-quality` runs the QA-trace,
  startup-lifecycle, keyboard-chords, energy-recovery, natural-prizes, bot-combat,
  mild, and impairment matrices in sequence. Individual components are
  `test-e2e-immediate-startup-lifecycle`, `test-e2e-immediate-keyboard-chords`,
  `test-e2e-immediate-keyboard-high-refresh`, `test-e2e-immediate-energy-recovery`,
  `test-e2e-immediate-natural-prizes`, and `test-e2e-immediate-impairment`.
- Single-lane special-weapon runs: `make test-e2e-special-weapons-chaos`,
  `-extreme`, `-firefox`, `-firefox-extreme`, and `-reorder` each run one
  `tests/e2e/special_weapons.mjs` profile lane with retained success artifacts.
- Additional headed E2E lanes: `make test-e2e-negative-energy` (Extreme
  negative-energy combat replay), `make test-e2e-prizes-phase1` (prize phase-1
  combat replay), `make test-e2e-objective-authority` (objective/authority
  harness), and `make test-e2e-ordinary-url` (Chrome/Firefox ordinary client URL).
- Retained evidence wrappers: `make test-retained-mixed-smoke`,
  `test-retained-webrtc-channel-contract`, `test-retained-webrtc-capacity`, and
  `test-retained-compose-rtc` re-run the corresponding gate with retained
  provenance/artifact evidence; `test-retained-external-gates` runs all four.
- Node harness unit checks: `make test-snapshot-parser` runs the browser JSON
  snapshot parser and evidence-parsing node tests (a prerequisite of most E2E
  targets); `make test-wasm-identity` checks WASM identity binding.
- Release bundle check: `make check-release-bundle` builds the release client and
  validates the bundle with `tools/release_bundle_check.mjs`. If cached release
  build scripts reference GC'd nix-store glibc, the check fails on stale build
  scripts, not the bundle: clear `target/*/release/build/**/build-script-build*`
  and rebuild.

- RTC cleanup evidence must include `rtc_ingress_execution` and empty packet
  queues; see [`docs/e2e-diagnostics.md`](docs/e2e-diagnostics.md) for the
  handler-pool and witness semantics.

## Local Services

- Stop the local game server and static client server: `make stop-server`
- Build the Rust server and debug WASM client: `make dev`
- Build the debug WASM client served by `index.html`: `make build-client`
- Run the WebRTC game server in the foreground: `make run-server`
- Run the static client server in the foreground: `make run-client-server`
- Build the client/server and start both detached with logs: `make start-local`
- Stop stale services, rebuild, start both detached, and print the URL: `make restart-local`
- Print the local RTC client URL: `make echo-client-link`
- Play the optimized release bundle locally (serves `dist/` instead of the debug
  WASM that `index.html` loads): `make start-local-release`, or
  `make restart-local-release`
- Build the release browser bundle only: `make build-client-release`

Preferred restart loop after server/client changes:

```sh
make restart-local
```

Use `make restart-local` instead of ad hoc process cleanup when a client or server change needs to be rebuilt and tried locally. It already runs `make stop-server`, rebuilds the WASM client and Rust server, starts both detached services, and prints the client URL.

Frame-rate work must not measure `make start-local` alone: the repo-root
`index.html` loads the debug WASM client, which `make build-client` builds at
`opt-level=1` with `CARGO_PROFILE_DEV_DEBUG_ASSERTIONS=true` and
`CARGO_PROFILE_DEV_OVERFLOW_CHECKS=true`, so it is several times more expensive
per frame than the shipped build. Use `make restart-local-release` for the
shipped-code comparison and keep the debug pair for tracing.

For browser playtesting, run the two foreground services in separate shells or background them with logs:

```sh
setsid make run-server > /tmp/zubspace-server.log 2>&1 &
setsid make run-client-server > /tmp/zubspace-static.log 2>&1 &
```

The default local client URL is:

```text
http://127.0.0.1:7000/
```

## Deployment

One VPS, `zubspace-prod` (HostHatch New York, 85.155.184.54), runs two
independent environments as rootless Podman Compose projects owned by
`peter`: **prod** at <https://zubspace.gg> and **dev** at
<https://dev.zubspace.gg>. The game server's public API (`/v1/status`,
`/v1/players`, `/v1/ping`, `/v1/zones`, `/v1/zones/{id}[/players|/status]`) is also at `server.zubspace.gg` and
`dev-server.zubspace.gg`. Both environments run 15 bots (ships 1,4,5,7). Prod logs INFO
to the journal and has no client diagnostics; dev logs DEBUG to a file,
accepts client presentation traces and is capped at one vCPU. Browsers reach
nginx through Cloudflare and a Cloudflare tunnel per environment; gameplay
UDP goes straight to the host. The port table, layout, disk limits and
Cloudflare settings are in [`deploy/README.md`](deploy/README.md).

Everything runs from this checkout over SSH:

```sh
make deploy-dev                 # build HEAD on the host, restart dev, join it with headless Chrome
make deploy-dev REF=<commit>    # any commit
make promote-prod               # the exact commit dev serves, to prod
make deploy-prod REF=<commit>   # or any commit straight to prod
make rollback-prod              # prod's previous recorded deploy (also rollback-dev)
make remote-status              # commit, total players, sessions, containers, disk for both
make remote-history             # recorded deploys
make remote-logs-dev FOLLOW=0   # server log tail; SERVICE=client|cloudflared|prometheus
make remote-restart-dev         # restart without redeploying; SERVICE=<one>
make probe-prod                 # join check alone
make deploy-notifier            # the Slack join/leave notifier (notifier/), its own project;
                                # NOTIFIER_CONFIG=notifier/etc/notifier.json installs its real config
```

`FORCE=1` lets prod restart with players online and `PROBE=0` skips the
post-deploy join check. The scripts are
[`scripts/deploy.sh`](scripts/deploy.sh) and
[`scripts/remote.sh`](scripts/remote.sh).

Rules for agents:

- Deploy only when the owner asks, to the environment they name. Normal
  order: `make deploy-dev`, check the probe result, then `make promote-prod`,
  so prod receives a commit dev has already served.
- A deploy ships `git archive` of a commit, never the working tree. Another
  agent's uncommitted work is neither deployed nor touched. If the change you
  mean to ship is uncommitted, say so; never commit someone else's work to
  get it deployed.
- If a commit fails to build on the host, the environment keeps running its
  previous build. Deploy an earlier commit that builds (`REF=`) and report the
  failure; do not patch files on the host.
- Prod refuses to restart while players are connected. Use `FORCE=1` only on
  the owner's say-so.
- Configuration lives in Git: `deploy/compose.yml`, `deploy/<env>/` (server
  TOML, `compose.env` ports and CPU limits, `prometheus.yml`) and
  `deploy/host/`. Edit, commit, deploy; the next deploy overwrites anything
  edited on the host. Host-level changes belong in `deploy/host/setup.sh`,
  which installs its sibling files, so copy the directory and run it there
  (`scp -r deploy/host root@zubspace-prod:/root/zubspace-host`, then
  `ssh root@zubspace-prod /root/zubspace-host/setup.sh`). Cloudflare
  (DNS, tunnel ingress, zone settings, redirect and rate limit) is
  `deploy/cloudflare/zubspace.gg.json`: run `node scripts/cloudflare.mjs plan`,
  then `apply`, never dashboard-only edits.
- Secrets: the Cloudflare API token is `.secrets/cloudflare-token` (git- and
  docker-ignored); tunnel tokens exist only in `~/zubspace/<env>/cloudflared.env`
  on the host; the notifier's real config, which holds its Slack webhook,
  exists only as the git-ignored local `notifier/etc/notifier.json` and the
  host's `~/zubspace/notifier/notifier.json` (commit only
  `notifier/etc/notifier.sample.json`). Never print, copy or commit them, and pass the API token
  through a file or process substitution rather than argv.
- `ssh` needs the network and the owner's SSH config, so run it outside a
  command sandbox. Use `peter@zubspace-prod`; `root@` is only for setup and
  packages.
- Ask before rebooting the host, changing DNS, Cloudflare security settings
  or the firewall, or deleting anything on the host the scripts did not
  create.
- After a deploy, report the probe's JSON: build label, join time, players,
  bots and RTC sessions.

## Browser Smoke Tests

- Mixed Chrome/Firefox RTC smoke: `make smoke-mixed-rtc`
- Firefox RTC smoke: `make smoke-firefox-rtc`
- Real server browser smoke: `make smoke-real-server`
- It is okay to clean up local dev server/static-server processes and browser instances left behind by smoke tests without asking first.

## Browser E2E Tests

- Scenario-oriented browser tests live under `tests/e2e/`.
- Generic service/browser helpers may remain under `tools/` when they are reused by smoke tests and E2E tests.
- Rebuild the WASM client before launching any browser lane: `make dev` or `make build-client`.
  `make build` is server-only, so a lane started behind it drives whatever
  `target/wasm32-unknown-unknown/debug/zubspace.wasm` the previous build left. That is not cosmetic:
  `build.rs` regenerates the embedded `assets/zones/**` level/LVZ/settings manifests when Cargo
  re-runs it, so a stale client keeps asking for files that no longer exist, and
  `smoke-real-server`'s clean-console assertion then fails on 404s for assets that were deleted from
  the repository while the server itself behaved correctly.
- Default combat replay: `make test-e2e-combat`
- Full-energy ship/spectator menu gate followed by combat replay: `make test-e2e-ship-energy`
- Snapshot loss/recovery replay with verified realized drops and snapshot-id gaps: `make test-e2e-snapshot-loss`
- Headed C2S rolling-input omission/reorder recovery: `make test-e2e-input-recovery`
- Headed Firefox C2S omission/reorder and snap-back false-positive recovery:
  `make test-e2e-firefox-input-recovery`
- Headed Chrome lost-offer movement-correction application/receipt recovery:
  `make test-e2e-movement-correction`
- Headed one-shot semantic input and Rocket recovery: `make test-e2e-semantic-input`
- Headed lifecycle transport-ACK versus application-receipt barrier:
  `make test-e2e-baseline-receipt`
- Headed explicit Warp collectible/life/position receipt path:
  `make test-e2e-warp-relocation`
- Headed Chrome/Firefox genuine-key deterministic map-wormhole relocation,
  energy, and security path: `make test-e2e-wormhole-authority`
- Headed Portal inventory placement request/terminal path:
  `make test-e2e-portal-phase2`
- Headed Chrome/Firefox Chaos special authority plus authored-zero Extreme path:
  `make test-e2e-special-weapons`
- Headed stale-door clock freeze/catch-up relocation: `make test-e2e-stale-door`
- Headed Chrome/Firefox QA marker, correlated upload/retrieval, diagnostic-fault
  isolation, and genuine-key continuity: `make test-e2e-qa-trace`. Its Chrome arm is headed, and
  it provisions its own display when the caller has no usable one: the recipe connects to
  `/tmp/.X11-unix/X<n>` for the current `$DISPLAY` and, when that fails, reruns the same headed Chrome
  under `xvfb-run` at 1440x1000x24. Testing that `$DISPLAY` is merely *set* is not sufficient - a
  sandboxed shell reported `DISPLAY=:0` while the socket was unreachable, and Chrome died with
  `Missing X server` anyway, which also failed `make test-e2e-immediate-quality` because qa-trace is one
  of its prerequisites. Under a real session the arm runs on the user's display unchanged.
- Separated aimed bullet replay: `make test-e2e-aimed-bullet`
- Held-projectile rendering: `make test-e2e-held-projectile` runs three genuine
  point-blank human-victim linked shots at a distinct fixed 48 px separation.
  It requires a real held wire fact, multiple stationary drawn frames with
  continuing TTL age, exact damage/terminal correlation, both perspectives and
  a complete post-terminal absence tail. It is focused lane 64. The normal
  separated-shot lane keeps its original minimum 80 px geometry requirement.
- Mixed fast/slow real-UDP aimed-bullet replay: `make test-e2e-aimed-bullet-wan`
  reuses the same linked-shot, exact damage, both-view and counterfire assertions
  with the ordinary Auto policy and isolated FIFO WAN fixture. It additionally
  requires selected-route proof, realized encrypted application loss and drained
  fixture sockets/queues. It is focused acceptance lane 63; the earlier 62 lanes
  retain their ordering. The ordinary aimed-bullet lane remains unchanged.
- Normal-URL manual bullet replay: `make test-e2e-manual-bullet`
- Focused post-respawn bomb damage replay: `make test-e2e-bomb`
- Recoverable chat-send/browser-client ownership replay:
  `make test-e2e-chat-send-failure`
- Mixed Chrome/Firefox single-bullet counterfire replay:
  `make test-e2e-mixed-single-bullet`
- Active-projectile game-copy omission plus delayed event-lane recovery:
  `make test-e2e-active-projectile-recovery`
- Disk Chaos/Extreme plus no-rebuild temporary-zone replay:
  `make test-e2e-dynamic-zones`
- Default E2E arena: `ZUBSPACE_E2E_ARENA=chaos-zone`
- Default E2E repeat count: `ZUBSPACE_E2E_CYCLES=2`
- Retained E2E artifacts must include exact-tree provenance and per-channel
  DataChannel delivery/security audits. Recheck a result with
  `node tests/e2e/verify_artifact_provenance.mjs /path/to/result.json`.
- For E2E test work, follow `.agents/skills/e2e-test/SKILL.md`.

## Test Runner

Under Hyprland (`HYPRLAND_INSTANCE_SIGNATURE` set), launch every headed
browser you start — E2E gates, smokes, playtests, ad-hoc debug windows — on
the silent background workspace 99 so the user's focus and workspace never
move. Use the helpers in
[`scripts/e2e-hyprland-tools/`](scripts/e2e-hyprland-tools/README.md):

```sh
scripts/e2e-hyprland-tools/run -- make test-e2e-combat   # one gate
scripts/e2e-hyprland-tools/enable                         # or: several gates ...
make test-e2e-single-bullet
scripts/e2e-hyprland-tools/status                         # rule state + open test windows
scripts/e2e-hyprland-tools/disable                        # ... then switch the rule off
```

- `run` registers the rule, runs the command, and disables the rule again
  (`--keep` leaves it on). `enable`/`disable` are idempotent. `close`
  terminates leftover test browsers after a crash. `show`/`back` move the
  user's view to workspace 99 and back; use them only when the user asks to
  look.
- How it works: the repo launchers tag every headed test browser with the
  window class `zubspace-e2e` (`tests/e2e/combat_replay.mjs` passes
  `--class` to Chrome, `tools/browser_firefox_rtc_smoke.mjs` to Firefox;
  `ZUBSPACE_E2E_WINDOW_CLASS` overrides it). The tools register one Hyprland
  runtime window rule matching exactly that class
  (`workspace = "99 silent"`; `ZUBSPACE_E2E_HYPR_WORKSPACE` overrides the
  number) with `focus_on_activate = false` and `suppress_event = "activate"`,
  because the E2E helpers call CDP `Page.bringToFront`, an activation request
  that would otherwise switch the user to workspace 99 whenever
  `misc:focus_on_activate` is on (it is on in the user's config). The rule
  also floats test windows at 1440x1000, the screen size of the Xvfb gates:
  a tiled window alone on workspace 99 fills the whole monitor, and
  swiftshader then renders several times more pixels, which dropped the
  Chrome ordinary-URL cadence gate to ~34 fps and failed it. Never install
  rules matching `google-chrome` or `firefox` yourself: those capture the
  user's own browser windows.
- Ad-hoc browsers you launch by hand must pass the same class:
  `google-chrome --class=zubspace-e2e --ozone-platform=x11 --user-data-dir=…`
  or `MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 firefox --class zubspace-e2e …`.
  Firefox applies `--class` only through XWayland (native Wayland keeps the
  app id `firefox`); `run` exports those two variables, and the Firefox
  Makefile gates already set them under Xvfb, where windows never touch the
  display anyway.
- The tools talk to the Hyprland unix socket. In a sandbox that blocks unix
  sockets `hyprctl` fails with "Couldn't open a socket": run the tools (or
  the wrapped gate) outside the sandbox. Without Hyprland the tools are
  no-ops and `run` simply executes its command.
- Verified on Hyprland 0.56: a Chrome window hidden on
  workspace 99 keeps `requestAnimationFrame` at 60fps (302 frames per 5s,
  worst gap 17ms), CDP keyboard injection needs no OS focus, and
  `make test-e2e-ordinary-url`'s Chrome/Chaos cell passes through `run` with
  the same frame profile as a headless run (3598 frames, 67.9ms maximum
  gap). Workspace 99 is created lazily by the rule and disappears when its
  last window closes.
- Hyprland >= 0.55 gotchas: the config is Lua (`hyprctl repl`/`eval`;
  `hl.window_rule({...})` returns a handle with `:set_enabled(bool)`), old
  shell-style dispatcher syntax and `hyprctl keyword` are rejected, a rule
  stays registered even after its Lua handle is garbage collected (the tools
  keep handles in `_G.__zubspace_e2e`), and there is no supported way to move
  an already-mapped unfocused window via dispatch — the rule must be
  registered before the browser launches.

## Reference Game Client

The behavioural reference for game-client semantics is the reference game client, checked out locally at
`/home/peter/Dev/other/reference-game` (sources under `src/`). Consult it whenever a question is "what is this mechanic
supposed to do" rather than "what does our code do" - several gameplay defects were only correctly specified after reading it.

Where to look, by subject:

- `src/WeaponManager` - weapon generation and every weapon's own lifetime. `GetWeaponTotalAliveTime` (~`:1056`)
  gives per-type alive time (bullet, bomb, dropped-mine, decoy), `:938` stamps `end_tick` at generation, `:877-887` is the
  burst activation that fans `BurstShrapnel` independent bullets evenly around a full turn, and `:868-875` is linked-weapon
  teardown. Weapon lifetime belongs to the weapon, never to its owner.
- `src/ShipController` - ship motion and input application. Afterburner (`:71-75`, `:121`, `:207`) substitutes the
  ship's `MaximumThrust` for normal thrust, costs `AfterburnerEnergy / 10 * dt`, and is gated on actually thrusting forward or
  backward. Ship damage/damage-level math (`:2022`) also lives here.
- `src/ArenaSettings` - the tunables: per-ship `BurstSpeed` / `BurstShrapnel` / `BurstMax` / `InitialBurst` /
  `MaximumThrust` / `AfterburnerEnergy`, and arena-wide `BulletAliveTime` / `BombAliveTime` / `DecoyAliveTime` /
  `MineAliveTime` / `BurstDamageLevel`. Timeouts in this codebase are arena settings, not constants to hard-code.
- `src/InputState` - the reference input action set. Our current movement-record bits are defined in
  `src/gameplay/player_input.rs`: `BIT_THRUST=1`, `BIT_REVERSE=2`, `BIT_ROTATE_LEFT=4`,
  `BIT_ROTATE_RIGHT=8`, `BIT_AFTERBURNER_HELD=16`, `BIT_ROCKET_ACTIVATION_PULSE=32`,
  `BIT_SAFE_ZONE_WEAPON_STOP=64`. Firing is carried separately in the weapon payload;
  do not encode it as movement bit 4 (that requests a left turn).

Explicit owner decisions take precedence over the reference game's behavior. Implement and
test those intentional differences as specified; consult the reference game for mechanics
the owner has not overridden.

Two differences that are ours by design, so do not "fix" them toward the reference:

- **Units.** The reference uses 40000-unit circles, radians helpers and its own tick macros. This repo's orientation is
  **turns on 0.0..1.0** (`src/gameplay/player_movement.rs:66`) and its timestamps are u16 position ticks. Translating
  reference code means converting units, not copying constants.
- **Authority.** The reference client is the authority for *behaviour*; our server is the authority for *state*. Where the
  reference resolves something locally (predicted motion, local weapon removal, cosmetic cleanup), the equivalent decision in
  this repo belongs on the server, and a client-side shortening of it is a bug rather than a port.


- [`docs/`](docs/README.md) describes the system as it is implemented. Update the
  relevant doc when behaviour changes, and don't add dated review, status or
  evidence notes there.
- Active zone assets live under `assets/zones/**`; do not restore the removed `levels/*` folder.
- The local multiplayer transport target is WebRTC DataChannels with HTTP used only for signaling/status support.

- `make test-e2e-menu-shortcuts` verifies gameplay Q, menu weapon/movement isolation, leave-to-Join, roster cleanup, and manual rejoin in Chrome.
- `make test-e2e-arena-kill-notifications` verifies a genuine-key kill reaches an
  uninvolved active player and spectator exactly once, including respawn recovery.
- `make test-e2e-expanded-scoring` verifies exact server bounty points, kills,
  deaths, average bounty and rating on both held-Space tables after genuine
  kills/respawns, then reconnects both identities and checks a fresh late join.
  It retains separate live/reconnect evidence with independent session/drain
  audits under `ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR` (a temporary root by default).

- `make test-e2e-main-menu` checks the public menu, live population, desktop/narrow-screen layout, editable saved names, Chaos default, hidden Extreme URL, and starfield teardown with a real Chrome/WebRTC join.

- `make test-e2e-spectator` and `make test-e2e-spectator-firefox` verify free camera panning, human/bot roster following, observer input isolation, and Esc + 1/8 returning to play.
- `make test-e2e-held-primary-fire` verifies held-Space/Up/Ctrl isolation in
  ordinary two-human combat, then releases Space while Ctrl remains physically
  down and requires repeated authoritative damage in both clients. Releasing
  Ctrl must stop weapon emission. This complements the tap/auto-repeat and
  kill/respawn lanes; it does not replace them. The spectator and held-primary
  scripts support `ZUBSPACE_E2E_STATIC_ROOT=dist` with matching bundle provenance.

- Production observability: `make test-observability` checks bounded reporting
  and histogram units; `make test-e2e-observability` checks ordinary browser
  reporting, JSON logs and the separate Prometheus listener. Operator reference:
  [`docs/observability.md`](docs/observability.md).
- Full prelaunch renewal: `scripts/launch_readiness.sh --list` enumerates the
  canonical/focused runners plus supplementary active and retained-feature QA.
  `scripts/launch_readiness.sh` requires a clean tree, runs sequentially, retains
  all failures, and writes a head-pinned result. An optional numeric argument
  resumes at that step into a fresh evidence directory. Ignored benchmarks stay
  measurements; remote deployment happens only on the owner's request (see
  Deployment above).

- `make test-e2e-timed-prize-visuals` collects genuine-key natural Shield and
  Super pickups with the current Chaos prize weights and retains before/active
  screenshots. It uses normal FFA policy and authoritative private effect state.
- Debug-only `ZUBSPACE_SERVER_TEST_TEAM_MODE=1` preserves historical team/cutover
  fixtures in semantic-input, baseline-receipt and phase-one prize QA. Release
  servers ignore it. `/v1/zones/{id}.free_for_all` identifies the zone policy; these
  fixtures do not prove default FFA restrictions. The menu-shortcuts gate does.

- `make test-e2e-clean-url` drives a fresh ordinary Chrome URL with no query
  parameters through name entry, genuine flight input, Esc-Q leave, rejoin and
  remembered-name refresh. It uses the same configured HTTP proxy as the local
  development server; all gameplay still travels over the three RTC channels.

- `make test-e2e-connection-quality` measures scoped, unreliable server probe/echo
  traffic in two ordinary Chrome clients, including realized slow-profile delay.
  This measurement lane does not prove forced-spectator cooldown enforcement.
- `make test-e2e-bot-concealment` verifies genuine stealth/cloak/X-radar keys,
  human world/radar visibility, bot concealment/reacquisition and unchanged
  per-human bot assignment using real Chrome clients and authoritative target
  traces. It uses unchanged Chaos settings with only the central 3x3 map tiles
  made safe for stationary actors. Weapons stay enabled and actor life sequences
  must remain unchanged; this visibility lane is not combat proof.
- `make test-e2e-bot-detector-exhaustion` repeats that genuine-key detector
  contract at a declared energy boundary: zero recharge/prize weights and
  zero Shark concealment costs keep both human targets hidden until the
  Warbird detector exhausts at its unchanged authored 25 energy/second. Both
  profiles retain maximum spawn capabilities and require matching server AI
  detector state, acquisition, actual pursuit and client snapshot bits. The
  exhaustion profile measures the drain and requires loss of hidden-target
  acquisition without losing capability or resetting the bot's life.
- `make test-e2e-timed-prize-visuals` collects real seeded Chaos Shield and Super
  greens using keyboard flight, checks authoritative pickup/active receipts and
  retains before-pickup/active screenshots. It uses debug server logging to locate
  natural prize spawns; the measured player's UI remains in ordinary mode.
- `make test-e2e-antiwarp-scope` collects a natural Chaos Antiwarp green, uses
  genuine keys to block/release nearby enemy portal placement, recall and random
  warp, waits real twenty-second portal expiries, replaces the expired endpoint
  with zero, rejects empty-inventory zero fallback and random-warps with Shift-zero
  without an endpoint. It requires unchanged life/RTC scope. Native tests cover exact
  range, hostility/lifecycle exclusions and server rejection without spending.
- `make test-e2e-multifire-bindings` uses a declared temporary Chaos prize fixture
  (zero starting bounty, only positive MultiFire greens) to verify acquisition,
  minus/Delete toggles, menu/chat isolation, and one/three/one bullet groups on
  the server and authenticated observer. Shipped Chaos prize weights stay intact.
- `make test-e2e-status-energy` isolates exhaustion in a declared zero-recharge,
  no-prize Chaos fixture with owned status capabilities. It keeps actual status
  costs, uses genuine keys for independent/combined activation, measures server
  drain, and requires prediction/opponent deactivation without losing capability,
  life or RTC identity. It does not measure ordinary Chaos recharge or EMP damage.
- `make test-e2e-bot-ship-variety` runs ordinary shipped Chaos with 15 moving,
  firing server bots across Warbird/Javelin/Leviathan/Terrier/Lancaster. It
  checks all five types draw/fire/deal damage, distinct FFA frequencies and
  unique named roster entries, actual respawn, and held-Space roster scrolling.
  It also runs the companion three-bot Warbird/Terrier energy lane: exact
  per-activation debits, double-barrel grouping, cooldown and a continuous
  same-life window of at least eight shots over 300 ticks. Overlapping QA
  captures respect the existing four-upload sliding quota; bounded trace gaps
  cannot count as continuous evidence. An explicit artifact directory receives
  `variety/` and `energy/` results. `make test-e2e-bot-weapon-energy` runs only
  that companion lane. Autonomous AI selects bullets; bomb/mines/Super and
  rejection boundaries remain native authority tests, not browser bot claims.
- `make test-e2e-ship-abilities` selects all eight ships through the Escape menu
  in actual shipped Chaos, checks bounty-30 capability ownership/inactive starts,
  positive/unavailable keys and opponent status agreement, and measures normal
  recharge sustaining status costs. Ambient prize pickup is forbidden; explicit
  ship changes advance life while RTC remains unchanged.
- `make test-e2e-enemy-energy-labels` uses a declared no-recharge/no-prize/no-LVZ
  presentation fixture and a stationary passive bot. Genuine shots create low
  energy; retained screenshot pixels must preserve own yellow/red labels, hide
  enemy human/bot labels normally and show them in debug, including free/follow
  spectator views. Damage remains enabled; this is not a combat acceptance lane.
- `make test-e2e-connection-quality-enforcement` and
  `make test-e2e-connection-quality-loss` exercise the real sustained-latency
  and round-trip probe-loss policies respectively, including a healthy peer,
  brief spike, forced spectator, early denial, camera/follow, new RTC transport
  with the same browser identity, the full 60-second hold and healthy return.
  They change only DataChannel impairment and use genuine keyboard controls.

- `make test-e2e-decoy-presentation` checks stationary overlap, inherited moving
  velocity, mirrored owner turns, local-label absence and remote ship/name/radar
  presentation with genuine Chrome input in unmodified Chaos.

- `make test-e2e-brick-warp` groups cardinal brick geometry, friendly hull
  passage, hostile overlap relocation, occupied-center rejection, late-join
  replay, full authored 120-second expiry and four central random warps using
  genuine Chrome keys. Its declared map-only Chaos fixture preserves shipped
  weapon/settings/art. Projectile-versus-brick collision is a separate gate.

- `make test-e2e-brick-weapons` verifies hostile bullet reflection/no hit,
  a friendly linked Terrier hit and 20-pixel cardinal barrel spacing, actual
  Thor flight through the wall, and a bound wall-explosion fragment fan on
  three Chrome clients. It uses the same map-only fixture as brick/warp and
  DEBUG server lifecycle evidence. It does not prove fragment damage.

- `make test-e2e-shrapnel-damage` uses real Warbird/Leviathan mines and three
  human clients, preserving Chaos weapon/settings/art. Its declared larger
  map-only clear patch permits staging beyond each bomb's blast radius on a
  reference-predicted fragment ray. It requires level-two/three fragment
  damage after the inactive window, exact receipts in all views and correlated
  server explosion QA. The staging oracle is checked against retained reference-game
  vectors in `test-quick`; no gameplay packet or state injection is used.

- `make test-e2e-ship-weapon-traits` groups all-eight Chaos bomb/fragment
  source/count/power checks, owner/witness artwork captures, Lancaster's
  north-wall bounce/south-wall explosion and a real Weasel EMP victim. EMP
  must pause recharge while preserving rotation, thrust and accepted firing,
  then allow recharge to resume. Its map-only fixture adds two repeatable
  solid rows without changing weapon/settings/art.

- `make test-nginx-qa-proxy` is an opt-in local Docker integration check of
  the release nginx template: exact authenticated QA POST forwarding, denied
  retrieval/wrong methods and the 256-KiB upload cap. It creates and removes
  only its own temporary host-network container; it uses the cached release
  nginx base image and does not restart the development services.

- `make test-e2e-weapon-safety-lifecycle` uses genuine Chrome keys and an explicit
  map-only Chaos fixture to verify spectator projectile recovery, silent mine/
  Decoy cancellation on safety entry, valid post-egress launches, preservation
  across owner death/respawn, cancellation across same-session source lives,
  and real older-life mine damage against a hostile human and the successor owner.

- `make test-e2e-weapon-cutover` verifies genuine-key ship/spectator transitions
  cancel that session's mine/Decoy copies on both clients, preserve an unrelated
  player's mine, deliver exact source cutoffs and accept a later valid launch.
  It also uses Esc-Q and the normal Join form to verify reliable departure
  cutoff-before-player-left ordering, old-session removal and a fresh-session
  launch, retaining the departing owner's pre-leave trace and final live peers.

- `make test-e2e-bot-projectile-presentation` verifies ordinary 15-bot ships
  1,4,5,8 combat in Chrome player/free-spectator views. It correlates exact
  server collision witnesses with drawn projectiles/victims, linked-shot
  retirement and complete half-second render tails after terminal delivery.

- `make test-e2e-maxed-respawn` runs two genuine-key human death/respawn cycles
  with unchanged Extreme settings and a declared map-only clear combat patch.
  It uses the shared combat assertions for dead-input suppression, death/score,
  remote visibility and post-respawn damage, and requires maxed energy 2800.

- `make test-e2e-spectator-bot-audio` verifies camera-local running WebAudio
  while a genuine-key spectator follows a moving, autonomously firing bot;
  one human supplies movement and no weapon input, isolating the source.
- `make bench-client-debug-overhead` requires a prebuilt matching release
  server and `dist/` client. It alternates actual server client-debug policy
  OFF/ON/ON/OFF on ordinary URLs, retaining rAF/CDP task cost, exact source/
  binary provenance, uploaded NDJSON, phase metrics and RTC cleanup. It is a
  hardware diagnostic measurement, not an acceptance gate or latency verdict.

- `make test-browser-zone-cache` exercises actual binary IndexedDB storage,
  quota recovery and eviction without RTC or gameplay claims.
- `make test-e2e-zone-cache-join` verifies selected-zone HTTP reuse, genuine F5
  and clean Chrome profile restart against the real RTC server.
- `make test-e2e-identity-rejoin` verifies live duplicate identity rejection,
  bounded retry exhaustion without incumbent eviction, then abrupt owned-browser
  SIGKILL and immediate profile restart with same-connection automatic recovery.
