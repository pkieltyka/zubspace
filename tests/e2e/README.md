# Zubspace browser E2E tests

Scenarios in this folder run the real WASM client in real Chrome or Firefox
against the real game server over WebRTC, and drive it with real keyboard
input. Each one checks what the player sees together with what the server
decided. [`AGENTS.md`](../../AGENTS.md) lists every `make` target; this file
explains how the harness works. Diagnostic profiles and measurement witnesses
are in [`docs/e2e-diagnostics.md`](../../docs/e2e-diagnostics.md).

## Rules for scenarios

- Drive the same UI and keys a player uses: CDP `Input` events in Chrome,
  WebDriver in Firefox (through geckodriver). Never inject gameplay packets or
  write game state from a browser test; raw packet hooks belong in native
  protocol and authority tests.
- Assert on server logs and events, browser traces (`damageTrace` entries carry
  the server's damage, projectile and life identities), UI metrics and
  screenshots. UI metrics are live state and may already show recharge after a
  hit; no scenario relies on a damage-hold timer.
- Fault injection goes only through explicit seams that leave gameplay state
  alone:
  - server environment variables named `ZUBSPACE_SERVER_TEST_*`, such as
    `ZUBSPACE_SERVER_TEST_DROP_INITIAL_MAP_FILE_REQUESTS` and
    `ZUBSPACE_SERVER_TEST_PRIZE_BASELINE_DELAY_SNAPSHOTS`. Most are compiled
    out of release builds (`#[cfg(debug_assertions)]`); the prize-share and
    hidden-prize seams instead also require `ZUBSPACE_E2E_PRIZES_PHASE1=1`;
  - server-side seeded send loss (`ZUBSPACE_SERVER_TX_LOSS_PCT`,
    `ZUBSPACE_SERVER_TX_LOSS_SEED`);
  - URL parameters read by the browser's RTC bridge in `index.html`
    (`c2s_input_impairment`, `s2c_stall`, `clock_stall`,
    `chat_send_failure`, and others) that drop, delay or fail whole
    DataChannel messages. Ordinary URLs never enter these paths.
- When Chaos or Extreme settings change, update the expectations in the
  scenarios that read them.

## Running

Build the server and the debug WASM client first (`make dev`). Targets that
depend on `test-snapshot-parser` build both themselves; the single, manual,
aimed and mixed bullet lanes, `test-e2e-bomb` and the flag lanes do not.

On Hyprland, wrap headed runs so the windows open on silent workspace 99:

```sh
scripts/e2e-hyprland-tools/run -- make test-e2e-combat
```

Firefox lanes run through `nix-shell -p geckodriver`, most of them on a
private 1440x1000 Xvfb screen. Chrome runs headless unless `ZUBSPACE_E2E_HEADLESS=0`;
most focused lanes set that.

## Layout

| Path | Contents |
| --- | --- |
| `combat_replay.mjs` | Chrome/CDP driver, the default kill/respawn/damage replay, and many focused scenarios selected by `ZUBSPACE_E2E_<SCENARIO>=1` (input recovery, semantic input, baseline receipt, stale door, clock stall, warp relocation, Portal, snapshot loss and more). Other Chrome scenarios import its helpers. |
| `<scenario>.mjs` | One script per other scenario, for example `special_weapons.mjs`, `bot_authority.mjs`, `ordinary_url.mjs`. Firefox drivers include `bot_authority_firefox.mjs` and `firefox_input_recovery.mjs`. |
| `*.test.mjs` | Node unit tests for harness logic (parsers, staging, oracles). They run without browsers in `make test-quick` and `make test-snapshot-parser`. |
| `verify_artifact_provenance.mjs` | Re-verifies a retained result. |
| `retained_gate_evidence.mjs` | Wraps external RTC gates with provenance (see below). |
| `fixtures/` | Static fixture data. Fixture zones such as `flag-test` are under `tests/fixtures/zones/`. |
| `../../tools/` | Helpers shared with the smoke tests: service and browser startup (`browser_firefox_rtc_smoke.mjs`), stale-process checks (`e2e_gate_hygiene.mjs`), server status (`game_status.mjs`), log parsing (`server_events.mjs`), RTC audits (`rtc_gate_audit.mjs`), console capture (`browser_console_evidence.mjs`) and spawn-region fixtures (`e2e_spawn_region.mjs`). |

## Common environment variables

| Variable | Default | Effect |
| --- | --- | --- |
| `ZUBSPACE_E2E_ARENA` | `chaos-zone` | Zone to join. |
| `ZUBSPACE_E2E_CYCLES` | `2` | Kill, respawn and damage cycles in the combat replay. |
| `ZUBSPACE_E2E_HEADLESS` | headless | `0` runs Chrome headed. |
| `ZUBSPACE_E2E_BROWSER` | `chrome` | `firefox` for scripts that support both. |
| `CHROME_BIN` | `google-chrome` | Chrome executable. |
| `ZUBSPACE_E2E_STATIC_ROOT` | repository root | Serve another bundle, such as `dist` from `make build-client-release`. |
| `ZUBSPACE_E2E_SERVER_PROFILE` | `debug` | `release` runs `target/release/zubspace_server`. |
| `ZUBSPACE_SMOKE_BOT_COUNT` | server default | Server bot count for the run. |
| `ZUBSPACE_SMOKE_LOG_LEVEL` | `debug` | Server log level for the run. |
| `ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR` | none | Retain success artifacts here. It must not exist yet; use one per lane. |
| `ZUBSPACE_E2E_ARTIFACT_ROOT` | `/tmp` | Parent directory for generated artifact directories. |
| `ZUBSPACE_E2E_WINDOW_CLASS` | `zubspace-e2e` | Window class given to headed test browsers. |

## Artifacts and provenance

Retained results bind the exact source tree and build hashes, server and
browser logs, per-channel DataChannel delivery and security audits, and a file
manifest with sizes and SHA-256 hashes. Writers stage each directory and
publish it with an atomic rename. Recheck a result later with:

```sh
node tests/e2e/verify_artifact_provenance.mjs /path/to/result.json
```

The verifier rejects missing or changed files, paths outside the retained
root, symlinks and stale references to another run.

Browser console output is captured for the whole session (CDP in Chrome,
WebDriver BiDi `log.entryAdded` in Firefox) and written as
`<label>-console.json`. Counters cover every event; only a bounded tail is
kept (256 events, 2,048 characters each). A captured
`Entire call rejected` draw warning fails a result that requires a clean
console.

The expensive external RTC gates have provenance-retaining wrappers:

```sh
make test-retained-mixed-smoke
make test-retained-webrtc-channel-contract
make test-retained-webrtc-capacity
make test-retained-compose-rtc
```

They write under `RETAINED_GATE_ROOT` (default
`/tmp/zubspace-retained-external-gates`). Each `result.json` records the
source, build and entry-script inputs, raw logs, parsed gate counters and exit
status; the Compose wrapper also records both image ids. A wrapped gate fails
on any application backpressure, reliable overflow, structured security reject
or DataChannel `Full` record, or if its expected JSON output is missing or
malformed.

## QA trace capture

With `trace=1`, the client records a QA trace. The common artifact writer
freezes the page once and reads the full state once, recording the outcome in
`qaTraceCaptureAudit` (captured, disabled or failed); the verifier rejects a
retained success whose capture is missing or failed. Correlated server-event
tails are capped at 512 events or 128 KiB and report what they omit. Ordinary
URLs never collect a trace.

Bundles identify the build:

- `provenance.wasm` is the SHA-256 of the exact WASM bytes the loader
  compiled, hashed in a worker while streaming. On error, timeout or oversize
  input the identity is reported unavailable and loading continues normally.
  Ordinary play loads neither helper nor worker.
- `provenance.serverBinary` is the SHA-256 and size of the running server
  executable, captured before the server listens.
- The received map and settings packet carry CRC32 fingerprints.

These identify builds; they do not attest a client or grant authority.
`make test-wasm-identity` builds the client and checks the WASM hashing
against Node's crypto, including the real file; the faster
`tests/wasm_identity.test.mjs` run in `make test-quick` skips the real-file
case unless `ZUBSPACE_TEST_WASM_PATH` is set.

`make test-e2e-qa-trace` exercises the Chrome and Firefox marker and export
path, authenticated upload and host-local retrieval, a deliberately failed
invariant (retained with `ok: false`), and disabled, dropped, malformed and
rate-limited diagnostics, while genuine keyboard input still reaches the
server. Set `ZUBSPACE_E2E_HTTP_HOST` to an IPv4 address of the test machine to
load the page from a non-secure HTTP origin.

## Scenario notes

### Combat

- `test-e2e-combat`: two Chrome clients; movement, bullets, death, score,
  respawn, dead-input suppression, post-respawn damage and remote-motion
  smoothness, from both views and the server.
- `test-e2e-single-bullet`: one Ctrl tap maps to exactly one server damage
  event and one browser damage trace per client, with exact arithmetic and HUD
  clamping. The tap is 30 ms by default
  (`ZUBSPACE_E2E_SINGLE_BULLET_PRESS_MS`).
- `test-e2e-manual-bullet`: the normal page URL without `trace` or
  `packetlog` (it adds `metrics=1` for HUD reads). After full recharge, one hit
  must take the victim's server and HUD energy from 1700 to 1300; full-recharge
  shots repeat in both directions.
- `test-e2e-aimed-bullet`: ships fly apart, line up with keyboard rotation and
  hit each other from a distance, in both directions.
- `test-e2e-mixed-single-bullet`: the same one-tap exchange between Chrome and
  Firefox, rejecting any self-damage.
- `test-e2e-bomb`: keyboard bomb fire, server bomb damage, remote bomb
  rendering and self-splash separation after respawn.

The manual and mixed lanes give the two clients distinct names, because the
server rejects duplicate names.

### Network recovery

- `test-e2e-snapshot-loss`: 20% seeded server send loss. Requires realized
  drops, snapshot-id gaps, recovery and acknowledgement, bounded remote motion,
  exact damage, and a keyboard-driven area-of-interest leave and re-entry while
  loss is active.
- `test-e2e-input-recovery`: the browser bridge drops and delays outbound
  input datagrams while both pages are CPU-throttled. The server rebuilds
  missing inputs from the overlapping rolling input tails (32 records per tail)
  and never snaps back an honest client. The same lane drops the first Repel
  lifecycle record and the first `0x7a` receipt and requires exactly-once
  recovery.
- `test-e2e-baseline-receipt`: holds the server's `0x75` application receipt
  after the baseline transport ACK and proves no realtime input escapes until
  it arrives, then that held keys move immediately.
- `test-e2e-stale-door`: stalls inbound DataChannel delivery until the door
  clock is stale. The local simulation must freeze, keys pressed while stale
  are discarded, held keys resume, and a door that closed meanwhile causes
  exactly one server relocation.
- `test-e2e-clock-stall`: keeps delivering every packet but withholds the
  network clock from gameplay readiness; local execution freezes while
  snapshots, ACKs and remote presentation continue.
- `test-e2e-chat-send-failure`: a failed reliable chat send must not take the
  client out of the game or stop input, positions and snapshot polling.
- `test-e2e-initial-file-request-retry`: the debug server ignores six initial
  map-file requests; the same RTC session must keep retrying until the map
  arrives and the zone installs.
- `test-e2e-prize-baseline-delay`: the debug server withholds the first
  private prize state. No gameplay packet may leave before it arrives; the
  long-wait notice shows, and play resumes afterwards.

### Weapons and items

- `test-e2e-semantic-input`: short taps of Ctrl, Q, F4–F6 and
  Shift+Delete/Home/End/Insert each dispatch exactly once with the modifiers
  held at press time; concurrent F3+Z and a delayed Rocket activation keep
  Rocket inventory and timing in step with the server.
- `test-e2e-special-weapons`: Decoy, Thor, Burst, Rocket and Repel in Chaos
  (Chrome and Firefox), a variant that delivers the public Repel event before
  older Decoy and Thor carriers, and Extreme with the maximum spawn loadout
  turned off, so the Warbird starts with none of these items and each key
  must change nothing.
- `test-e2e-portal-phase2`: Portal placement with Shift+Insert, a natural
  Portal pickup, and recall with Insert, all confirmed by the server.
- `test-e2e-warp-relocation`: a naturally spawned Warp prize relocates the
  player once, identified by collectible, life and position sequence.
- `test-e2e-wormhole-authority`: a server wormhole relocation interrupts a
  keyboard route in Chrome and Firefox; each must replan exactly once.
- `test-e2e-chaos-route-replan`: the same replan on a fixture with a portal
  added to the Chaos map, ending in an authored safe zone.

### Presentation and cadence

- `test-e2e-ordinary-url`: Chrome and Firefox in Chaos and Extreme with all
  diagnostic URL options omitted. Genuine thrust must reach the server, no
  HUD or sprite diagnostic payload may be requested, and frame gaps over the
  capture window are recorded.
- `test-e2e-immediate-keyboard-high-refresh`: the keyboard chord matrix with
  uncapped frame scheduling (Chrome `--disable-frame-rate-limit`, Firefox
  `layout.frame_rate=0`). This is scheduler stress, not real high-refresh
  display QA.
- Motion soak: `ZUBSPACE_E2E_MOTION_SOAK_MS=240000 make test-e2e-bots` (or
  `test-e2e-bots-firefox`) records 240–300 seconds of uninterrupted remote
  motion inside the page before the bot combat scenario.
- `test-e2e-flag-carrier-ride` and `test-e2e-flag-carry-badge`: the generated
  `flag-test` zone (`tests/fixtures/zones/flag-test/arena.conf` over the Chaos
  map, no LVZ). The first checks a carried flag drawn on a moving carrier; the
  second checks the HUD carry badge pixels before, during and after a carry.
  These prove presentation only.

## Independent combat under a stalled recipient

Run `make dev`, then
`scripts/e2e-hyprland-tools/run -- make test-e2e-slow-client-isolation`.
This four-Chrome lane keeps an exact A-to-B collision pending by holding B's
projectile presentation in the real server queue. C-to-D shots must still move,
be drawn and apply once within the normal server scheduling bound. Releasing B
must finish its original collision once, followed by full session/queue cleanup.
Only scenario Spawn coordinates change; map and Chaos weapon tuning stay intact.
The server seam is debug-only and automatically bounded. Details are in
[`E2E diagnostics`](../../docs/e2e-diagnostics.md#slow-client-combat-isolation).
