SERVER_CONFIG ?= config/zubspace_server.toml
ZUBSPACE_BUILD_COMMIT ?= $(shell git rev-parse HEAD 2>/dev/null)
export ZUBSPACE_BUILD_COMMIT
STATIC_PORT ?= 7000
STATIC_ROOT ?= .
SERVER_LOG ?= /tmp/zubspace-server.log
STATIC_LOG ?= /tmp/zubspace-static.log
PLAYTEST_CONFIG ?= etc/server/playtest.toml
DEBUG_DIR ?= .debug
PROM_COMPOSE = docker compose --project-name zubspace-prom --file etc/prom/compose.yml
ARENA ?= chaos-zone
# Human-readable endpoint plus its query-string encoded form. Keep these in
# sync when overriding the default local signaling address.
RTC_OFFER_ENDPOINT ?= http://127.0.0.1:9001/v1/rtc/offer
RTC_OFFER_URL ?= http%3A%2F%2F127.0.0.1%3A9001%2Fv1%2Frtc%2Foffer
WASM_TARGET ?= wasm32-unknown-unknown
# Keep real-time browser development responsive while retaining dev symbols,
# debug assertions, and overflow checks. Use 0 for unoptimized source stepping.
WASM_DEV_OPT_LEVEL ?= 1
RETAINED_GATE_ROOT ?= /tmp/zubspace-retained-external-gates

# WASM linker resolution. Primary path is `.cargo/config.toml`, which
# tells cargo to use the `rust-lld` binary in PATH — works out of the
# box for stock `rustup` users. On environments where rust-lld isn't
# at the standard location (e.g. Nix's slim runtime rustc), auto-detect
# either the bundled driver or LLVM's equivalent `wasm-ld`, then fall
# through to a `find /nix/store` lookup so
# `make build-client` Just Works without manual override. Setting
# `WASM_LINKER=...` explicitly takes precedence.
WASM_LINKER ?= $(shell command -v rust-lld 2>/dev/null || command -v wasm-ld 2>/dev/null || ls -1 $$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld 2>/dev/null || find /nix/store -maxdepth 7 -type f \( -name rust-lld -o -name wasm-ld \) 2>/dev/null | head -1)

.PHONY: \
	build build-all build-client build-client-release build-server dev release \
	check-release-bundle \
	test test-unit test-integration test-smoke test-local-all \
	test-compose-rtc \
	test-multiplayer-correctness test-rtc-config test-snapshot-parser \
	run-server run-client-server start restart stop start-local restart-local stop-server \
	echo-client-link \
	smoke-real-server smoke-firefox-rtc smoke-mixed-rtc \
	test-e2e-combat test-e2e-snapshot-loss test-e2e-input-recovery test-e2e-firefox-input-recovery test-e2e-movement-correction test-e2e-semantic-input test-e2e-baseline-receipt test-e2e-prize-baseline-delay test-e2e-initial-file-request-retry test-e2e-warp-relocation test-e2e-wormhole-authority test-e2e-chaos-route-replan test-e2e-prizes-phase1 test-e2e-negative-energy test-e2e-stale-door test-e2e-clock-stall test-e2e-single-bullet test-e2e-manual-bullet test-e2e-bots test-e2e-bots-firefox test-e2e-active-projectile-recovery \
	test-e2e-portal-phase2 \
	test-e2e-special-weapons test-e2e-special-weapons-chaos test-e2e-special-weapons-reorder test-e2e-special-weapons-extreme test-e2e-special-weapons-firefox test-e2e-special-weapons-firefox-extreme \
	test-e2e-chat-send-failure \
	test-e2e-objective-authority \
	test-e2e-dynamic-zones \
	test-e2e-immediate-quality test-e2e-immediate-startup-lifecycle \
	test-e2e-immediate-keyboard-chords test-e2e-immediate-keyboard-mild test-e2e-immediate-energy-recovery \
	test-e2e-immediate-natural-prizes test-e2e-immediate-bot-combat test-e2e-aged-server-authority \
	test-e2e-immediate-mild test-e2e-immediate-impairment \
	test-e2e-aimed-bullet test-e2e-mixed-single-bullet test-e2e-bomb \
	test-e2e-flag-carrier-ride test-e2e-flag-carry-badge \
	test-webrtc-capacity test-webrtc-channel-contract \
	test-retained-mixed-smoke test-retained-webrtc-channel-contract \
	test-retained-webrtc-capacity test-retained-compose-rtc \
	test-retained-external-gates run-webrtc-channel-contract \
	docker-build docker-build-server docker-build-client docker-up docker-down

# Fast native Rust build for the local game server.
build: build-server

# Local browser-development build: native server plus debug WASM client.
dev: build-server build-client

# Build every local development artifact: the native game client, the native
# server, and the debug WASM game client served by index.html.
build-all: build-client
	cargo build --bins

build-client:
	node tools/zone_asset_manifest.mjs
	node tools/macroquad_pairing.mjs
	@linker="$(strip $(WASM_LINKER))"; \
	export CARGO_PROFILE_DEV_OPT_LEVEL="$(WASM_DEV_OPT_LEVEL)"; \
	export CARGO_PROFILE_DEV_DEBUG_ASSERTIONS=true CARGO_PROFILE_DEV_OVERFLOW_CHECKS=true; \
	if [ -z "$$linker" ]; then \
		cargo build --target $(WASM_TARGET) --bin zubspace; \
	else \
		RUSTFLAGS="-C linker=$$linker -C linker-flavor=wasm-ld -C link-arg=--allow-undefined" cargo build --target $(WASM_TARGET) --bin zubspace; \
	fi

# Production WASM bundle. Runs the optimised cargo release build and
# packages the artifacts under `dist/` (index.html with rewritten
# wasm path, the .wasm blob, the JS shim, BUILD_INFO.json). Consumed
# by Dockerfile.client and any static-host deployment.
build-client-release:
	@linker="$(strip $(WASM_LINKER))"; \
	if [ -z "$$linker" ]; then \
		node tools/build_wasm_release.mjs; \
	else \
		RUSTFLAGS="-C linker=$$linker -C linker-flavor=wasm-ld -C link-arg=--allow-undefined" node tools/build_wasm_release.mjs; \
	fi

# [REVIEW C7] Build the release bundle, then serve dist/ and assert the
# runtime-critical files (index, wasm, JS shim, a known zone .lvl) all
# answer HTTP 200 — the regression guard for shipping a bundle without
# the runtime-fetched zone assets.
check-release-bundle: build-client-release
	node tools/release_bundle_check.mjs

build-server:
	cargo build -p zubspace --bin zubspace_server

# `make release` produces everything needed for production: a release
# server binary plus the `dist/` bundle for the static client. Both
# Dockerfiles run the equivalent of this internally.
release: build-client-release
	cargo build --locked --release -p zubspace --bin zubspace_server

.PHONY: check-container-bundles
check-container-bundles:
	node tools/container_bundle_check.mjs

test:
	cargo test

# Test taxonomy:
# - test-unit: native game binary unit coverage without browser processes.
# - test-integration: server binary integration/state-machine coverage.
# - test-rtc-config: fast browser/capacity RTC configuration and timing checks.
# - test-smoke: shallow browser/server startup and RTC visibility checks.
# - test-compose-rtc: real Linux Compose images, nginx ingress, and Chrome RTC.
# - test-local-all: fast local non-E2E gate before browser combat work.
test-unit:
	cargo test -p zubspace --bin zubspace

test-integration:
	cargo test -p zubspace --bin zubspace_server

.PHONY: test-vendor-rtc
test-vendor-rtc:
	CARGO_TARGET_DIR="$(CURDIR)/target" cargo test --locked --manifest-path vendor/rtc-0.21.1/Cargo.toml --config 'patch.crates-io.rtc-sctp.path="vendor/rtc-sctp-0.21.1"' --lib

.PHONY: test-integration-release
test-integration-release:
	cargo test -p zubspace --release --bin zubspace_server

test-smoke: smoke-real-server smoke-firefox-rtc

test-compose-rtc:
	node tools/compose_rtc_smoke.mjs

# The canonical acceptance-matrix runner. `test-frozen-matrix-list` prints the numbered step list; a full run
# refuses a dirty tree and writes per-step logs plus result.json under /tmp/zubspace-frozen-matrix-<utc>-<head>.
.PHONY: test-frozen-matrix test-frozen-matrix-list

test-frozen-matrix:
	scripts/frozen_matrix.sh

test-frozen-matrix-list:
	scripts/frozen_matrix.sh --list

# The focused stress/recovery acceptance runner. `test-focused-acceptance-list` prints its lanes; a run
# refuses a dirty tree and writes per-lane logs plus result.json. It adds the authority/recovery/special-weapon
# scenarios the canonical 15-step frozen matrix does not cover.
.PHONY: test-focused-acceptance test-focused-acceptance-list

test-focused-acceptance:
	scripts/focused_acceptance.sh

test-focused-acceptance-list:
	scripts/focused_acceptance.sh --list

# Fast day-to-day checks: no browsers, WASM build, or dev-service restart.
.PHONY: test-quick
test-quick:
	node --test tests/e2e/bot_facing_encounter.test.mjs tests/e2e/spawn_region_runner.test.mjs tests/e2e/setup_recovery.test.mjs tests/s2c_stall.test.mjs tests/e2e/emp_damage_loss_preparation.test.mjs tests/e2e/bot_feel_door_boundary.test.mjs tests/e2e/counterfire_life.test.mjs tests/e2e/cutover_wire_observation.test.mjs
	$(MAKE) --no-print-directory test-vendor-rtc
	node tools/macroquad_pairing.mjs
	node --test tests/macroquad_pairing.test.mjs tests/browser_bundle.test.mjs tests/wasm_identity.test.mjs tests/e2e/chrome_key_modifiers.test.mjs tests/browser_console_evidence.test.mjs tests/e2e/browser_console_artifacts.test.mjs tests/e2e/firefox_artifact_client.test.mjs tests/e2e/keyboard_trace_reader.test.mjs tests/log_chunks.test.mjs tests/owner_projectile_trace.test.mjs tests/e2e/owner_bomb_handoff_evidence.test.mjs
	cargo fmt --check
	node --test tests/game_status.test.mjs tests/client_build_info.test.mjs tests/smoke_ui_contract.test.mjs tests/e2e/player_ship_identity.test.mjs tests/e2e/bot_authority_loadout.test.mjs tests/e2e/baseline_wait_packets.test.mjs tests/e2e/gate_hygiene.test.mjs tests/e2e/bot_combat_evidence.test.mjs tests/e2e/bot_pilot_safety.test.mjs tests/e2e/moving_bomb_pilot.test.mjs tests/e2e/protocol_aim_direction.test.mjs
	node --test tests/acceptance_runner.test.mjs tests/rtc_config.test.mjs tests/game_http_proxy.test.mjs tests/local_recording.test.mjs tests/client_debug.test.mjs tests/e2e/bot_facing.test.mjs tests/e2e/bot_detector_evidence.test.mjs tests/e2e/bot_weapon_energy_evidence.test.mjs tests/e2e/bot_projectile_presentation_evidence.test.mjs tests/e2e/held_projectile_evidence.test.mjs tests/e2e/shrapnel_reference.test.mjs
	node --test tests/zone_cache.test.mjs tests/zone_loading.test.mjs tests/menu_admission.test.mjs tests/game_key_bridge.test.mjs tests/fullscreen.test.mjs tests/e2e/motion_staging.test.mjs tests/e2e/stale_door_staging.test.mjs tests/e2e/keyboard_lifecycle_route.test.mjs tests/e2e/keyboard_origin_recovery.test.mjs tests/e2e/chaos_chord_recovery.test.mjs tests/e2e/player_rendezvous.test.mjs tests/e2e/rendezvous_staging.test.mjs tests/e2e/smoke_encounter_preparation.test.mjs tests/e2e/chrome_input_focus.test.mjs tests/e2e/fine_input_step.test.mjs tests/e2e/rtc_route_evidence.test.mjs tests/e2e/snapshot_loss_lifecycle.test.mjs tests/e2e/recovery_membership.test.mjs tests/e2e/movement_trace_read.test.mjs tests/e2e/movement_correction_staging.test.mjs tests/e2e/authored_navigation.test.mjs tests/e2e/route_attempt_shape.test.mjs tests/e2e/natural_prize_navigation.test.mjs tests/e2e/antiwarp_scope_preparation.test.mjs tests/e2e/ship_abilities_preparation.test.mjs tests/e2e/timed_prize_preparation.test.mjs tests/e2e/portal_inventory_baseline.test.mjs tests/e2e/special_weapon_repel_staging.test.mjs tests/e2e/aimed_pilot_orientation.test.mjs tests/e2e/aimed_path_preparation.test.mjs tests/e2e/bomb_pilot_orientation.test.mjs tests/e2e/bomb_path_preparation.test.mjs tests/e2e/menu_flight_evidence.test.mjs tests/e2e/timed_prize_visual_preparation.test.mjs tests/e2e/bot_encounter_staging.test.mjs tests/e2e/extreme_safety_return.test.mjs tests/e2e/weapon_lifecycle_fixture.test.mjs tests/e2e/brick_weapon_stage.test.mjs tests/e2e/qa_trace_capture.test.mjs tests/e2e/weapon_birth_view_evidence.test.mjs tests/e2e/spectator_bot_audio.test.mjs tests/e2e/client_debug_overhead_probe.test.mjs
	cargo test -p zubspace --quiet

.PHONY: start-prom-server stop-prom-server check-prom-config test-local-observability restart-local-recording restart-local-release-recording

# A separate project/volume: stopping the collector never stops the game or
# deletes metric history. Linux host networking reaches a loopback-only server.
check-prom-config:
	$(PROM_COMPOSE) run --rm -T --no-deps --entrypoint /bin/promtool prometheus check config /etc/prometheus/prometheus.yml

start-prom-server: check-prom-config
	$(PROM_COMPOSE) up -d prometheus
	@curl --fail --silent --show-error --retry 10 --retry-connrefused --retry-delay 1 --max-time 2 http://127.0.0.1:9090/-/ready
	@test "$$(docker inspect --format '{{.State.Running}}' zubspace-prom_1)" = true
	@echo "Zubspace Prometheus: http://127.0.0.1:9090 (scrapes 127.0.0.1:9100)"

stop-prom-server:
	$(PROM_COMPOSE) down

test-local-observability: build-server
	node --test tests/local_recording.test.mjs
	cargo test -p zubspace --bin zubspace_server log_filter --quiet
	cargo test -p zubspace --bin zubspace_server client_debug --quiet
	cargo test -p zubspace --bin zubspace_server local_playtest_config --quiet
	$(MAKE) --no-print-directory check-prom-config
	node tools/local_observability_check.mjs

restart-local-recording: start-prom-server
	node tools/local_recording.mjs --config "$(PLAYTEST_CONFIG)" --directory "$(DEBUG_DIR)"

restart-local-release-recording: start-prom-server
	node tools/local_recording.mjs --config "$(PLAYTEST_CONFIG)" --directory "$(DEBUG_DIR)" --release

test-local-all: test-rtc-config test-vendor-rtc
	cargo fmt --check
	cargo test -p zubspace --quiet
	$(MAKE) --no-print-directory dev

test-multiplayer-correctness: dev
	cargo test -p zubspace --bin zubspace_server respawn --quiet
	$(MAKE) --no-print-directory test-e2e-single-bullet
	$(MAKE) --no-print-directory test-e2e-combat

run-server: build-server
	target/debug/zubspace_server --config $(SERVER_CONFIG) --log-file /tmp/zubspace-server.log

run-client-server:
	RTC_OFFER_ENDPOINT="$(RTC_OFFER_ENDPOINT)" node tools/dev_static_server.mjs $(STATIC_PORT) $(STATIC_ROOT)

start-local: dev
	@setsid target/debug/zubspace_server --config $(SERVER_CONFIG) > $(SERVER_LOG) 2>&1 &
	@RTC_OFFER_ENDPOINT="$(RTC_OFFER_ENDPOINT)" setsid node tools/dev_static_server.mjs $(STATIC_PORT) $(STATIC_ROOT) > $(STATIC_LOG) 2>&1 &
	@echo "server log: $(SERVER_LOG)"
	@echo "static log: $(STATIC_LOG)"
	@$(MAKE) --no-print-directory echo-client-link

restart-local:
	@$(MAKE) --no-print-directory stop-server
	@$(MAKE) --no-print-directory start-local

# Play the optimized release bundle locally. This builds `dist/` and serves that
# bundle instead of the debug WASM (`opt-level=1` with debug assertions and
# overflow checks) that the repo-root `index.html` loads, so the same game server
# can be compared against the shipped client build.
start-local-release: build-client-release build-server
	@setsid target/debug/zubspace_server --config $(SERVER_CONFIG) > $(SERVER_LOG) 2>&1 &
	@RTC_OFFER_ENDPOINT="$(RTC_OFFER_ENDPOINT)" setsid node tools/dev_static_server.mjs $(STATIC_PORT) dist > $(STATIC_LOG) 2>&1 &
	@echo "server log: $(SERVER_LOG)"
	@echo "static log: $(STATIC_LOG)"
	@$(MAKE) --no-print-directory echo-client-link

restart-local-release:
	@$(MAKE) --no-print-directory stop-server
	@$(MAKE) --no-print-directory start-local-release

# Short aliases used by the README quickstart.
start: start-local

restart: restart-local

stop: stop-server

stop-server:
	-@pkill -TERM -f '^([^ ]*/)?zubspace_server( |$$)' 2>/dev/null || true
	-@pkill -TERM -f '^([^ ]*/)?node ([^ ]*/)?dev_static_server[.]mjs( |$$)' 2>/dev/null || true
	-@sleep 0.2
	-@pkill -KILL -f '^([^ ]*/)?zubspace_server( |$$)' 2>/dev/null || true
	-@pkill -KILL -f '^([^ ]*/)?node ([^ ]*/)?dev_static_server[.]mjs( |$$)' 2>/dev/null || true

echo-client-link:
	@echo
	@echo "Open zubspace in your browser:"
	@echo "  http://127.0.0.1:$(STATIC_PORT)/"
	@echo
	@echo "Local game connection (no URL parameters required):"
	@echo "  WebRTC offer: $(RTC_OFFER_ENDPOINT)"
	@echo "  Arena:        $(ARENA)"
	@echo

smoke-real-server:
	node tools/browser_real_server_smoke.mjs

smoke-firefox-rtc:
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 xvfb-run -a -s "-screen 0 1440x1000x24" node tools/browser_firefox_rtc_smoke.mjs'

smoke-mixed-rtc:
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 xvfb-run -a -s "-screen 0 1440x1000x24" node tools/browser_mixed_rtc_smoke.mjs'

test-rtc-config:
	node --test tests/rtc_udp_impairment.test.mjs
	node --test tests/rtc_send_pressure.test.mjs
	node --test tests/game_status.test.mjs tests/client_build_info.test.mjs tests/smoke_ui_contract.test.mjs tests/e2e/player_ship_identity.test.mjs tests/e2e/bot_authority_loadout.test.mjs
	node --test tests/game_http_proxy.test.mjs
	node --test tests/player_telemetry.test.mjs tests/server_log_fields.test.mjs
	node --test tests/cpu_affinity.test.mjs
	node --test tests/chrome_graphics.test.mjs
	node --test tests/moving_peer_rig.test.mjs
	node --test tests/browser_bundle.test.mjs
	node --test tests/capture_responsiveness.test.mjs
	node --test tests/cdp_json_capture.test.mjs
	node --test tests/gzip_json_chunks.test.mjs tests/gzip_json_decode.test.mjs
	node --test tests/qa_trace_budget.test.mjs
	node --test tests/qa_stall_capture.test.mjs
	node --test tests/qa_frame_diagnostics.test.mjs
	node --test tests/qa_mark_download.test.mjs
	node --test tests/log_chunks.test.mjs
	node --test tests/webrtc_capacity_expression.test.mjs
	node --test tests/zone_cache.test.mjs tests/zone_loading.test.mjs tests/menu.test.mjs tests/rtc_config.test.mjs tests/realtime_event_evidence.test.mjs tests/zone_catalog.test.mjs tests/webrtc_capacity_config.test.mjs tests/webrtc_capacity_runner.test.mjs tests/compose_rtc_smoke_runner.test.mjs tests/rtc_gate_audit.test.mjs tests/browser_firefox_rtc_smoke_runner.test.mjs

.PHONY: bench-history-lookup
bench-history-lookup:
	cargo test -p zubspace --release --bin zubspace_server history_lookup_capacity_benchmark -- --ignored --nocapture --test-threads=1

.PHONY: bench-lvz-scan
bench-lvz-scan:
	cargo test -p zubspace --release --bin zubspace lvz_scan_cost -- --ignored --nocapture --test-threads=1

.PHONY: bench-lvz-merge
bench-lvz-merge:
	cargo test -p zubspace --release --bin zubspace lvz_merge_cost -- --ignored --nocapture --test-threads=1

.PHONY: bench-frame-cadence
bench-frame-cadence:
	node tests/e2e/frame_cadence_probe.mjs

bench-capture-overhead:
	PROBE_ARENA=$(or $(PROBE_ARENA),chaos-zone) node tests/e2e/capture_overhead_probe.mjs

.PHONY: bench-capture-overhead

.PHONY: bench-client-cpu
bench-client-cpu:
	node tests/e2e/cpu_profile_probe.mjs

# Diagnostic capture only: records a real owner bomb handoff, including a
# discontinuity if present. Its successful capture is not a gameplay PASS.
.PHONY: test-e2e-rocket-audio-hud
test-e2e-rocket-audio-hud: build-client build-server
	node tests/e2e/rocket_audio_hud.mjs

.PHONY: probe-owner-bomb-handoff
probe-owner-bomb-handoff: build-client build-server
	node tests/e2e/owner_bomb_handoff_probe.mjs

.PHONY: test-e2e-owner-bomb-handoff
# Exact last-predicted/first-confirmed draw plus the complete bounded transition.
test-e2e-owner-bomb-handoff: build-client build-server
	ZUBSPACE_E2E_REQUIRE_OWNER_BOMB_CONTINUITY=1 node tests/e2e/owner_bomb_handoff_probe.mjs

test-snapshot-parser: test-rtc-config build-server build-client
	node --test tests/e2e/browser_json_snapshot.test.mjs tests/e2e/baseline_wait_packets.test.mjs
	node --test tests/e2e/tab_return_visibility.test.mjs tests/e2e/firefox_key_encoding.test.mjs
	node --test tests/e2e/udp_probe_timing.test.mjs tests/e2e/bot_motion_boundaries.test.mjs tests/e2e/bot_respawn_aoi.test.mjs tests/e2e/bot_spawn_fixture.test.mjs
	node --test tests/e2e/antiwarp_scope_preparation.test.mjs tests/e2e/ship_abilities_preparation.test.mjs
	node --test tests/e2e/player_rendezvous.test.mjs tests/e2e/rendezvous_staging.test.mjs tests/e2e/smoke_encounter_preparation.test.mjs tests/e2e/timed_prize_preparation.test.mjs tests/e2e/portal_inventory_baseline.test.mjs tests/e2e/special_weapon_repel_staging.test.mjs tests/e2e/aimed_pilot_orientation.test.mjs tests/e2e/aimed_path_preparation.test.mjs tests/e2e/bomb_pilot_orientation.test.mjs tests/e2e/bomb_path_preparation.test.mjs tests/e2e/menu_flight_evidence.test.mjs tests/e2e/timed_prize_visual_preparation.test.mjs
	node --test tests/bullet_feedback.test.mjs
	node --test tests/e2e/remote_motion_cadence.test.mjs tests/e2e/server_process_evidence.test.mjs tests/e2e/artifact_paths.test.mjs
	node --test tests/e2e/gate_hygiene.test.mjs
	node --test tests/e2e/starvation_witness.test.mjs tests/e2e/long_task_witness.test.mjs
	node --test tests/e2e/moving_peer_evidence.test.mjs tests/e2e/moving_peer_readiness.test.mjs
	node --test tests/e2e/counterfire_life.test.mjs
	node --test tests/e2e/bot_facing_encounter.test.mjs
	node --test tests/e2e/moving_bomb_coverage.test.mjs
	node --test tests/e2e/bot_facing.test.mjs tests/e2e/bot_detector_evidence.test.mjs tests/e2e/bot_weapon_energy_evidence.test.mjs tests/e2e/bot_projectile_presentation_evidence.test.mjs tests/e2e/held_projectile_evidence.test.mjs
	node --test tests/e2e/firefox_artifact_client.test.mjs tests/browser_console_evidence.test.mjs tests/game_key_bridge.test.mjs tests/e2e/browser_console_artifacts.test.mjs
	node --test tests/e2e/keyboard_chords.test.mjs tests/e2e/keyboard_trace_reader.test.mjs tests/e2e/keyboard_network_profile.test.mjs tests/e2e/input_latency_evidence.test.mjs tests/e2e/keyboard_lifecycle_route.test.mjs tests/e2e/keyboard_origin_recovery.test.mjs tests/e2e/chaos_chord_recovery.test.mjs tests/e2e/held_key_batches.test.mjs tests/e2e/server_timing_evidence.test.mjs tests/e2e/bot_pilot_safety.test.mjs tests/e2e/moving_bomb_pilot.test.mjs tests/e2e/protocol_aim_direction.test.mjs
	# The route driver's failure-path instrument. It sat unwired and therefore never ran in a gate, which is the
	# same blind spot that let its first version throw on the branch it exists to report.
	node --test tests/e2e/route_attempt_shape.test.mjs tests/e2e/stale_door_staging.test.mjs
	# The Chaos wormhole gravity field. Its absence from the planner is what made a safe-zone route leg fly the
	# pilot into the wormhole and fail lane 19 of the focused acceptance surface, twice in a row.
	node --test tests/e2e/chaos_wormhole_avoidance.test.mjs
	# The relocation recovery branch must plan from the live post-relocation position and stay bounded.
	node --test tests/e2e/chaos_route_replanner.test.mjs
	# The Chaos-derived fixture puts a real portal on the original authored safe-route leg and certifies its replacement geometry.
	node --test tests/e2e/chaos_route_replan_fixture.test.mjs
	# The shared runway probe the Chaos/Extreme chord staging selects a heading with.
	node --test tests/e2e/runway_heading.test.mjs tests/e2e/aoi_outbound_staging.test.mjs
	node --test tests/wasm_identity.test.mjs tests/e2e/ordinary_url.test.mjs tests/e2e/qa_trace_capture.test.mjs tests/e2e/qa_trace_artifacts.test.mjs tests/e2e/qa_trace_coverage.test.mjs tests/e2e/smoke_qa_artifacts.test.mjs tests/e2e/focused_browser_evidence.test.mjs tests/e2e/artifact_provenance_async.test.mjs
	node --test --test-concurrency=1 tests/e2e/snapshot_parser.test.mjs tests/e2e/client_input_packet_parser.test.mjs tests/e2e/sim_observation_summary.test.mjs tests/e2e/recovery_membership.test.mjs tests/e2e/c2s_recovery_analysis_async.test.mjs tests/e2e/security_reject_audit.test.mjs tests/e2e/artifact_provenance.test.mjs tests/e2e/retained_gate_evidence.test.mjs tests/e2e/dynamic_zone_weapon_schedule.test.mjs tests/e2e/bot_combat_evidence.test.mjs tests/e2e/remote_motion_soak.test.mjs tests/e2e/network_profile_evidence.test.mjs tests/e2e/server_observer_profile.test.mjs tests/e2e/energy_recovery_evidence.test.mjs

.PHONY: test-wasm-identity
test-wasm-identity: build-client
	ZUBSPACE_TEST_WASM_PATH=target/wasm32-unknown-unknown/debug/zubspace.wasm node --test tests/wasm_identity.test.mjs

test-e2e-combat: test-snapshot-parser
	node tests/e2e/combat_replay.mjs

.PHONY: test-e2e-ship-energy
test-e2e-ship-energy: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_SHIP_ENERGY=1 node tests/e2e/combat_replay.mjs

test-e2e-snapshot-loss: test-snapshot-parser
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-snapshot-loss-$$(date +%s%N) ZUBSPACE_E2E_SNAPSHOT_LOSS=1 ZUBSPACE_SERVER_TX_LOSS_PCT=20 ZUBSPACE_SERVER_TX_LOSS_SEED=141421 node tests/e2e/combat_replay.mjs

test-e2e-input-recovery: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-input-recovery-$$(date +%s%N) ZUBSPACE_E2E_C2S_INPUT_RECOVERY=1 ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs

test-e2e-firefox-input-recovery: test-snapshot-parser build-server build-client
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_FIREFOX_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-firefox-input-recovery-$$(date +%s%N) xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/firefox_input_recovery.mjs'

test-e2e-movement-correction: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_MOVEMENT_CORRECTION=1 ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs

test-e2e-semantic-input: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_SEMANTIC_INPUT=1 ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs

test-e2e-baseline-receipt: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_BASELINE_RECEIPT=1 ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs

test-e2e-prize-baseline-delay: test-snapshot-parser build-server build-client
	ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_PRIZE_BASELINE_DELAY_SNAPSHOTS=1800 ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/prize_baseline_delay.mjs

test-e2e-initial-file-request-retry: test-snapshot-parser build-server build-client
	ZUBSPACE_SERVER_TEST_DROP_INITIAL_MAP_FILE_REQUESTS=6 ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-initial-file-request-retry-$$(date +%s%N) ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/initial_file_request_retry.mjs

test-e2e-warp-relocation: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_WARP_RELOCATION=1 ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs

test-e2e-wormhole-authority: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-wormhole-authority-$$(date +%s%N) ZUBSPACE_E2E_WORMHOLE_AUTHORITY=1 ZUBSPACE_E2E_ARENA=wormhole-e2e ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_WORMHOLE_AUTHORITY=1 ZUBSPACE_E2E_FIREFOX_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-wormhole-authority-firefox-$$(date +%s%N) xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/firefox_input_recovery.mjs'

test-e2e-chaos-route-replan: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-chaos-route-replan-$$(date +%s%N) ZUBSPACE_E2E_CHAOS_ROUTE_REPLAN=1 ZUBSPACE_E2E_ARENA=chaos-replan ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_CHAOS_ROUTE_REPLAN=1 ZUBSPACE_E2E_FIREFOX_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-chaos-route-replan-firefox-$$(date +%s%N) xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/firefox_input_recovery.mjs'

test-e2e-prizes-phase1: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_PRIZES_PHASE1=1 ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs

.PHONY: test-e2e-firefox-safety-return
test-e2e-firefox-safety-return: build-server build-client
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_SMOKE_BOT_COUNT=2 ZUBSPACE_E2E_ARENA=extreme ZUBSPACE_E2E_JOIN_MIN_UPTIME_SECONDS=0 ZUBSPACE_E2E_SPAWN_LOADOUT=maximum ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ=2,3 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,3 xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/firefox_safety_return.mjs'

test-e2e-negative-energy: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-negative-energy-maximum-$$(date +%s%N) ZUBSPACE_E2E_NEGATIVE_ENERGY=1 ZUBSPACE_E2E_NEGATIVE_ENERGY_LOADOUT=maximum ZUBSPACE_E2E_ARENA=extreme ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-negative-energy-reference-$$(date +%s%N) ZUBSPACE_E2E_NEGATIVE_ENERGY=1 ZUBSPACE_E2E_NEGATIVE_ENERGY_LOADOUT=reference ZUBSPACE_E2E_ARENA=extreme ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs

test-e2e-portal-phase2: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_PORTAL_PHASE2=1 ZUBSPACE_E2E_ARENA=chaos-zone ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs

test-e2e-special-weapons: test-e2e-special-weapons-chaos test-e2e-special-weapons-reorder test-e2e-special-weapons-extreme test-e2e-special-weapons-firefox test-e2e-special-weapons-firefox-extreme

test-e2e-special-weapons-chaos: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-special-weapons-chrome-chaos-$$(date +%s%N) ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_ARENA=chaos-zone ZUBSPACE_E2E_BROWSER_PAIR=chrome-chrome ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/special_weapons.mjs

test-e2e-special-weapons-reorder: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-special-weapons-chrome-chaos-reorder-$$(date +%s%N) ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_REALTIME_REPEL_REORDER=1 ZUBSPACE_E2E_ARENA=chaos-zone ZUBSPACE_E2E_BROWSER_PAIR=chrome-chrome ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/special_weapons.mjs

test-e2e-special-weapons-extreme: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-special-weapons-chrome-extreme-$$(date +%s%N) ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_ARENA=extreme ZUBSPACE_E2E_BROWSER_PAIR=chrome-chrome ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/special_weapons.mjs

test-e2e-special-weapons-firefox: test-snapshot-parser build-server build-client
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-special-weapons-firefox-chaos-$$(date +%s%N) ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_ARENA=chaos-zone ZUBSPACE_E2E_BROWSER_PAIR=chrome-firefox xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/special_weapons.mjs'

test-e2e-special-weapons-firefox-extreme: test-snapshot-parser build-server build-client
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-special-weapons-firefox-extreme-$$(date +%s%N) ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_ARENA=extreme ZUBSPACE_E2E_BROWSER_PAIR=chrome-firefox xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/special_weapons.mjs'

test-e2e-stale-door: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_STALE_DOOR=1 ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs

test-e2e-clock-stall: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_CLOCK_STALL=1 ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/combat_replay.mjs

test-e2e-chat-send-failure: test-snapshot-parser build-server build-client
	node tests/e2e/chat_send_failure.mjs

.PHONY: test-e2e-transport-recovery
test-e2e-transport-recovery: test-snapshot-parser build-server build-client
	node tests/e2e/transport_recovery.mjs

test-e2e-objective-authority: build-client build-server
	node tests/e2e/objective_authority.mjs

test-e2e-dynamic-zones: build-client build-server
	node tests/e2e/dynamic_zones.mjs

.PHONY: test-e2e-moving-peers
test-e2e-moving-peers: build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=$${ZUBSPACE_SMOKE_LOG_LEVEL:-info} ZUBSPACE_E2E_HEADLESS=0 nix-shell -p xorg-server --run 'node tests/e2e/moving_peer_load.mjs'

# The same eight-peer/10-tick contract with ordinary browser graphics on a
# desktop display. Assert real hardware WebGL AND hardware compositing for
# every peer. Native GPU runs need a reachable local X/Xwayland desktop;
# the portable software lane keeps its isolated Xvfb screens.
.PHONY: test-e2e-moving-peers-gpu
test-e2e-moving-peers-gpu: build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=$${ZUBSPACE_SMOKE_LOG_LEVEL:-info} ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_MOVING_GRAPHICS_BACKEND=native node tests/e2e/moving_peer_load.mjs

test-e2e-single-bullet:
	node tests/e2e/single_bullet_damage.mjs

test-e2e-bots: build-client build-server
	# Retain two moving bots with ordinary physics, recharge and damage.
	# Mute weapons only on collision lives1,2 for human-owned lethal proofs.
	# Fresh hostile successors still must prove lethal counterfire and firing
	# facing in the unchanged script; no stationary fixture is selected.
	ZUBSPACE_E2E_BOT_SPAWN_FIXTURE=central ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,2 ZUBSPACE_SMOKE_BOT_COUNT=2 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 node tests/e2e/bot_authority.mjs

test-e2e-bots-firefox: build-client build-server
	# Match Chrome: moving collision lives, followed by real hostile counterfire.
	ZUBSPACE_E2E_BOT_SPAWN_FIXTURE=central ZUBSPACE_SMOKE_BOT_COUNT=2 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,2 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/bot_authority_firefox.mjs'

test-e2e-active-projectile-recovery: build-client build-server
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-active-projectile-recovery-$$(date +%s%N) ZUBSPACE_E2E_ACTIVE_PROJECTILE_IMPAIRMENT=1 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=chaos-zone node tests/e2e/bot_authority.mjs

# Immediate browser-gameplay quality gate.
# Every scenario drives genuine DevTools/WebDriver keys over the normal WebRTC
# channels and retains a uniquely named, exact-tree artifact by default.
test-e2e-immediate-quality: test-e2e-qa-trace test-e2e-immediate-startup-lifecycle test-e2e-immediate-keyboard-chords test-e2e-immediate-energy-recovery test-e2e-immediate-natural-prizes test-e2e-immediate-bot-combat test-e2e-immediate-mild test-e2e-immediate-impairment

test-e2e-immediate-startup-lifecycle: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=chrome ZUBSPACE_E2E_ARENA=chaos-zone ZUBSPACE_E2E_CYCLES=10 node tests/e2e/startup_lifecycle_matrix.mjs
	ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=chrome ZUBSPACE_E2E_ARENA=extreme ZUBSPACE_E2E_CYCLES=10 node tests/e2e/startup_lifecycle_matrix.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=firefox ZUBSPACE_E2E_ARENA=chaos-zone ZUBSPACE_E2E_CYCLES=10 xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/startup_lifecycle_matrix.mjs'
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=firefox ZUBSPACE_E2E_ARENA=extreme ZUBSPACE_E2E_CYCLES=10 xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/startup_lifecycle_matrix.mjs'

test-e2e-immediate-keyboard-mild:
	ZUBSPACE_E2E_NETWORK_PROFILE=mild ZUBSPACE_E2E_NETWORK_PROFILE_SEED=271828 $(MAKE) test-e2e-immediate-keyboard-chords

.PHONY: test-e2e-immediate-keyboard-high-refresh
test-e2e-immediate-keyboard-high-refresh:
	ZUBSPACE_E2E_HIGH_REFRESH=1 $(MAKE) test-e2e-immediate-keyboard-chords

test-e2e-immediate-keyboard-chords: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=chrome ZUBSPACE_E2E_ARENA=chaos-zone node tests/e2e/keyboard_chord_matrix.mjs
	ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=chrome ZUBSPACE_E2E_ARENA=extreme node tests/e2e/keyboard_chord_matrix.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=firefox ZUBSPACE_E2E_ARENA=chaos-zone xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/keyboard_chord_matrix.mjs'
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=firefox ZUBSPACE_E2E_ARENA=extreme xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/keyboard_chord_matrix.mjs'

test-e2e-immediate-energy-recovery: test-snapshot-parser build-client build-server
	ZUBSPACE_E2E_BROWSER=chrome node tests/e2e/energy_recovery.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_BROWSER=firefox xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/energy_recovery.mjs'

test-e2e-immediate-natural-prizes: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=chrome ZUBSPACE_E2E_ARENA=chaos-zone node tests/e2e/natural_prize_matrix.mjs
	ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=chrome ZUBSPACE_E2E_ARENA=extreme node tests/e2e/natural_prize_matrix.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=firefox ZUBSPACE_E2E_ARENA=chaos-zone xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/natural_prize_matrix.mjs'
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_E2E_BROWSER=firefox ZUBSPACE_E2E_ARENA=extreme xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/natural_prize_matrix.mjs'

test-e2e-immediate-bot-combat: test-snapshot-parser build-client build-server
	# Isolate Chrome Chaos against one authoritative bot so the repeated human
	# bullet/bomb lifecycle cannot split its damage across two regenerating bots.
	# Life 2 keeps production movement/collision/recharge but cannot counterfire;
	# the surrounding lives retain ordinary hostility and reciprocal combat.
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-immediate-bots-chrome-chaos-$$(date +%s%N) ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=2 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=chaos-zone node tests/e2e/bot_authority.mjs
	# Isolate every combat leg against one authoritative bot so unrelated
	# bot-v-bot crossfire cannot steal an exact human-owned lifecycle proof.
	# Extreme lives 1 and 3 retain ordinary movement/collision/recharge but
	# cannot counterfire while the browser proves the initial and repeated
	# human-owned terminals. A later hostile successor independently proves
	# ordinary bot projectiles, presentation, damage, and lethal counterfire.
	# The standalone two-bot authority targets retain multi-bot presentation
	# and crossfire coverage independently of this attribution matrix.
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-immediate-bots-chrome-extreme-$$(date +%s%N) ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ=2,3 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,3 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=extreme node tests/e2e/bot_authority.mjs
	# Firefox Chaos lives 2 and 3 are passive exact collision oracles for the
	# initial and repeated bombs; the repeated lethal retires life 3 before a
	# later successor resumes hostile fire. This keeps human-owned geometry and
	# bot lethality independent instead of racing them on one exact life.
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-immediate-bots-firefox-chaos-$$(date +%s%N) ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=2 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=2,3 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=chaos-zone xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/bot_authority_firefox.mjs'
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-immediate-bots-firefox-extreme-$$(date +%s%N) ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ=2,3 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,3 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=extreme xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/bot_authority_firefox.mjs'

# Preserve authored direct bombs independently of production maximum entry.
.PHONY: test-e2e-bots-reference-extreme
test-e2e-bots-reference-extreme: test-snapshot-parser build-client build-server
	ZUBSPACE_E2E_SPAWN_LOADOUT=reference ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-reference-bots-chrome-extreme-$$(date +%s%N) ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ=2,3 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,3 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=extreme node tests/e2e/bot_authority.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_SPAWN_LOADOUT=reference ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-reference-bots-firefox-extreme-$$(date +%s%N) ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ=2,3 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,3 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=extreme xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/bot_authority_firefox.mjs'

# Aged-server cross-arena authority lane. The real server runs Chaos until it
# has >=350 seconds of wall uptime, which is past the 327.68-second u16 position
# timestamp half-range that a freshly booted server cannot reach. Both browsers
# then join Extreme with two bots, hold a four-minute genuine-key motion window,
# and complete the exact-life combat, stationary-fixture, facing, and fresh
# counterfire audits on that aged join and arena switch. Success artifact
# directories are created atomically by the runner and must never be pre-created.
test-e2e-aged-server-authority: test-snapshot-parser build-client build-server
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-aged-server-chrome-extreme-$$(date +%s%N) ZUBSPACE_E2E_JOIN_MIN_UPTIME_SECONDS=350 ZUBSPACE_E2E_INITIAL_SERVER_ZONE=chaos-zone ZUBSPACE_E2E_MOTION_SOAK_MS=240000 ZUBSPACE_E2E_REQUIRE_FRESH_COUNTERFIRE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ=2,3 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,3 ZUBSPACE_SMOKE_BOT_COUNT=2 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=extreme node tests/e2e/bot_authority.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-aged-server-firefox-extreme-$$(date +%s%N) ZUBSPACE_E2E_JOIN_MIN_UPTIME_SECONDS=350 ZUBSPACE_E2E_INITIAL_SERVER_ZONE=chaos-zone ZUBSPACE_E2E_MOTION_SOAK_MS=240000 ZUBSPACE_E2E_REQUIRE_FRESH_COUNTERFIRE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ=2,3 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,3 ZUBSPACE_SMOKE_BOT_COUNT=2 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=extreme xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/bot_authority_firefox.mjs'

# Repeat the complete four-leg gameplay/combat acceptance under the supported
# mild Internet envelope. The browser impairment is deterministic and must
# realize delay, loss, duplication, and reordering in both directions; the
# server independently selects its trusted supported-mild observer policy.
# Extreme lives 1 and 3 isolate the initial and repeated human-owned combat
# terminals so bot counterfire cannot race the real-key safe-edge route under
# delay. Lives 2 and 4+ remain ordinarily hostile, so both browsers still
# prove reciprocal bot lethality independently. A complete real-key bullet
# burst may retire life 3 and move the repeated bomb to life 4 without
# weakening either exact-life proof.
test-e2e-immediate-mild: test-snapshot-parser build-client build-server
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-immediate-mild-chrome-chaos-$$(date +%s%N) ZUBSPACE_E2E_NETWORK_PROFILE=mild ZUBSPACE_E2E_NETWORK_PROFILE_SEED=271828 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=2 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=chaos-zone node tests/e2e/bot_authority.mjs
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-immediate-mild-chrome-extreme-$$(date +%s%N) ZUBSPACE_E2E_NETWORK_PROFILE=mild ZUBSPACE_E2E_NETWORK_PROFILE_SEED=271828 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ=2,3 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,3 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=extreme node tests/e2e/bot_authority.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-immediate-mild-firefox-chaos-$$(date +%s%N) ZUBSPACE_E2E_NETWORK_PROFILE=mild ZUBSPACE_E2E_NETWORK_PROFILE_SEED=271828 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=2 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=2,3 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=chaos-zone xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/bot_authority_firefox.mjs'
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR=/tmp/zubspace-immediate-mild-firefox-extreme-$$(date +%s%N) ZUBSPACE_E2E_NETWORK_PROFILE=mild ZUBSPACE_E2E_NETWORK_PROFILE_SEED=271828 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_AFTER_HUMAN_DAMAGE=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_FROM_LIFE_SEQ=1 ZUBSPACE_SERVER_TEST_STATIONARY_BOT_STATIC_LIFE_SEQ=2,3 ZUBSPACE_SERVER_TEST_PASSIVE_BOT_LIFE_SEQ=1,3 ZUBSPACE_SMOKE_BOT_COUNT=1 ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE=1 ZUBSPACE_E2E_ARENA=extreme xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/bot_authority_firefox.mjs'

# Seeded, bounded transport adversity. The combat replay proves realized S2C
# loss and recovery; Chrome+Firefox input recovery prove C2S omission,
# delay/overtake and explicit duplicate idempotence; natural prize collection
# proves exact application convergence and no duplicate claim under S2C loss.
test-e2e-immediate-impairment: test-snapshot-parser build-client build-server test-e2e-snapshot-loss test-e2e-input-recovery test-e2e-firefox-input-recovery
	node tests/e2e/retained_gate_evidence.mjs --gate webrtc-channel-contract --out /tmp/zubspace-immediate-channel-contract-$$(date +%s%N) --input tools/browser_rtc_smoke.mjs --input tools/browser_firefox_rtc_smoke.mjs --input tools/rtc_gate_audit.mjs -- make --no-print-directory run-webrtc-channel-contract
	ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_SERVER_TX_LOSS_PCT=12 ZUBSPACE_SERVER_TX_LOSS_SEED=314159 ZUBSPACE_SERVER_TX_LOSS_AFTER_BOOTSTRAP=1 ZUBSPACE_E2E_BROWSER=chrome ZUBSPACE_E2E_ARENA=chaos-zone node tests/e2e/natural_prize_matrix.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_SMOKE_BOT_COUNT=0 ZUBSPACE_SERVER_TX_LOSS_PCT=12 ZUBSPACE_SERVER_TX_LOSS_SEED=314159 ZUBSPACE_SERVER_TX_LOSS_AFTER_BOOTSTRAP=1 ZUBSPACE_E2E_BROWSER=firefox ZUBSPACE_E2E_ARENA=extreme xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/natural_prize_matrix.mjs'

test-e2e-manual-bullet:
	node tests/e2e/manual_bullet_damage.mjs

.PHONY: test-e2e-ordinary-url test-e2e-qa-trace
test-e2e-qa-trace: test-snapshot-parser build-client build-server
	$(MAKE) test-e2e-qa-trace-chrome

# The Chrome arm is headed, and until now it assumed somebody was logged in and reachable: with no usable
# display it died as `qa-trace Chrome exited before DevTools became available: Missing X server`, which
# failed the whole immediate-quality aggregate because qa-trace is one of its prerequisites. The Firefox
# arm beside it has always provisioned its own Xvfb, so a gate whose browser is only there to drive a real
# page should not depend on the caller's session.
#
# Testing whether DISPLAY is *set* is not enough and was the first attempt: a sandboxed shell here reported
# DISPLAY=:0 while the X socket was unreachable, so Chrome still died. The probe therefore connects to the
# display's socket, which is what "usable" means for a headed browser.
.PHONY: test-e2e-qa-trace-chrome
test-e2e-qa-trace-chrome:
	@if node -e 'const net=require("net");const d=(process.env.DISPLAY||"");const n=d.indexOf(":")>=0?Number(d.split(":")[1].split(".")[0]):0;const s=net.createConnection({path:"/tmp/.X11-unix/X"+(Number.isFinite(n)?n:0)});s.setTimeout(1500);s.on("timeout",()=>process.exit(1));s.on("error",()=>process.exit(1));s.on("connect",()=>{s.destroy();process.exit(0);});' 2>/dev/null; then \
	  ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_BROWSER=chrome node tests/e2e/qa_trace_drill.mjs; \
	else \
	  echo "qa-trace: no usable X display, running the headed Chrome arm under Xvfb"; \
	  nix-shell -p xvfb-run --run 'xvfb-run -a -s "-screen 0 1440x1000x24" env ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_BROWSER=chrome node tests/e2e/qa_trace_drill.mjs'; \
	fi
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_BROWSER=firefox xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/qa_trace_drill.mjs'

test-e2e-ordinary-url: test-snapshot-parser build-client build-server
	ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_BROWSER=chrome ZUBSPACE_E2E_ARENA=chaos-zone node tests/e2e/ordinary_url.mjs
	ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_BROWSER=chrome ZUBSPACE_E2E_ARENA=extreme node tests/e2e/ordinary_url.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_BROWSER=firefox ZUBSPACE_E2E_ARENA=chaos-zone xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/ordinary_url.mjs'
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_BROWSER=firefox ZUBSPACE_E2E_ARENA=extreme xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/ordinary_url.mjs'

test-e2e-aimed-bullet:
	node tests/e2e/aimed_bullet_damage.mjs

.PHONY: test-e2e-aimed-bullet-wan
test-e2e-aimed-bullet-wan:
	ZUBSPACE_E2E_AIMED_BULLET_UDP_PROFILE=fifo-wan node tests/e2e/aimed_bullet_damage.mjs

.PHONY: test-e2e-held-projectile
test-e2e-held-projectile:
	ZUBSPACE_E2E_ARENA=chaos-zone ZUBSPACE_E2E_AIMED_BULLET_HELD_CONTACT=1 node tests/e2e/aimed_bullet_damage.mjs

test-e2e-mixed-single-bullet:
	nix-shell -p geckodriver --run 'node tests/e2e/mixed_single_bullet_damage.mjs'

test-e2e-bomb:
	node tests/e2e/bomb_damage.mjs

# The authored 8192 zone is pixel-bound under software GL, and how fast it renders depends on how many
# pixels its window occupies. A dedicated Xvfb display fixes the viewport so the cadence - and with it
# DevTools command latency - cannot depend on what else is on the user's desktop. The "~1.7 fps while the
# page was healthy" figure that used to be quoted here came from dividing metric publications by wall time
# across a boot-phase main-thread stall; see docs/e2e-diagnostics.md.
test-e2e-flag-carrier-ride:
	nix-shell -p xvfb-run --run 'xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/flag_carrier_ride.mjs'

# The HUD carry badge is asserted here from the client's own pixels: the own-carry slot must light on a
# real capture and return to its pre-capture bytes when the authored 3600-tick goal clears the carry. The
# hold is the authored 36 s, so this lane is slower than the ride gate.
test-e2e-flag-carry-badge:
	nix-shell -p xvfb-run --run 'xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/flag_carry_badge.mjs'

# Real concurrent WebRTC handshake/session/load gate. Defaults to the full
# supported 200-peer capacity; override ZUBSPACE_RTC_LOAD_CLIENTS for a quick
# local iteration without changing the production claim.
test-webrtc-capacity: test-rtc-config
	cargo build --release -p zubspace --bin zubspace_server
	node tools/webrtc_capacity_load.mjs

# Real Chrome DCEP mismatch rejection plus the valid Chrome and Firefox
# three-channel contracts. The Firefox leg also exercises normal gameplay over
# the exact timed-unreliable game channel.
test-webrtc-channel-contract: build-client build-server
	$(MAKE) --no-print-directory run-webrtc-channel-contract

run-webrtc-channel-contract:
	node tools/browser_rtc_smoke.mjs
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 xvfb-run -a -s "-screen 0 1440x1000x24" node tools/browser_firefox_rtc_smoke.mjs'

# Provenance-bound versions of the four expensive external RTC gates. Each
# result retains command stdout/stderr, exact source/build/script hashes, and
# (for Compose) the content ids of both tested images.
test-retained-mixed-smoke: dev
	node tests/e2e/retained_gate_evidence.mjs --gate mixed-smoke --out $(RETAINED_GATE_ROOT)/mixed-smoke --input tools/browser_mixed_rtc_smoke.mjs --input tools/browser_firefox_rtc_smoke.mjs --input tools/server_events.mjs --input tools/rtc_gate_audit.mjs -- $(MAKE) --no-print-directory smoke-mixed-rtc

test-retained-webrtc-channel-contract: build-client build-server
	node tests/e2e/retained_gate_evidence.mjs --gate webrtc-channel-contract --out $(RETAINED_GATE_ROOT)/webrtc-channel-contract --input tools/browser_rtc_smoke.mjs --input tools/browser_firefox_rtc_smoke.mjs --input tools/rtc_gate_audit.mjs -- $(MAKE) --no-print-directory run-webrtc-channel-contract

test-retained-webrtc-capacity: test-rtc-config
	cargo build --release -p zubspace --bin zubspace_server
	node tests/e2e/retained_gate_evidence.mjs --gate webrtc-capacity-200 --out $(RETAINED_GATE_ROOT)/webrtc-capacity-200 --server-binary target/release/zubspace_server --input tools/webrtc_capacity_load.mjs --input tools/webrtc_capacity_config.mjs --input tools/webrtc_capacity_runner.mjs --input tools/rtc_gate_audit.mjs -- node tools/webrtc_capacity_load.mjs

test-retained-compose-rtc:
	node tests/e2e/retained_gate_evidence.mjs --gate compose-rtc --out $(RETAINED_GATE_ROOT)/compose-rtc --input config/zubspace_server.toml --input tools/compose_rtc_smoke.mjs --input tools/compose_rtc_topology.mjs --input tools/rtc_gate_audit.mjs --input docker-compose.yml --input Dockerfile.server --input Dockerfile.client --docker-image zubspace/server:dev --docker-image zubspace/client:dev -- node tools/compose_rtc_smoke.mjs

test-retained-external-gates: test-retained-mixed-smoke test-retained-webrtc-channel-contract test-retained-webrtc-capacity test-retained-compose-rtc

# -----------------------------------------------------------------------------
# Docker / docker-compose. See docs/deployment.md for production guidance.
# -----------------------------------------------------------------------------

docker-build: docker-build-server docker-build-client

docker-build-server:
	docker build --build-arg "ZUBSPACE_BUILD_COMMIT=$$ZUBSPACE_BUILD_COMMIT" -f Dockerfile.server -t zubspace/server:dev .

docker-build-client:
	docker build --build-arg "ZUBSPACE_BUILD_COMMIT=$$ZUBSPACE_BUILD_COMMIT" -f Dockerfile.client -t zubspace/client:dev .

docker-up:
	docker compose up --build

docker-down:
	docker compose down

# Production host (zubspace-prod) runs prod (https://zubspace.gg) and dev
# (https://dev.zubspace.gg); deploy/README.md is the runbook.
#   make deploy-dev [REF=<commit>]   build REF (default HEAD) there, restart dev, probe it
#   make promote-prod                deploy the commit dev serves to prod
#   make rollback-prod               prod's previous recorded deploy (also rollback-dev)
#   make remote-status               commits, containers, players and disk
# FORCE=1 lets prod restart with players online; PROBE=0 skips the join probe.
REF ?= HEAD
FORCE ?= 0
PROBE ?= 1
SERVICE ?=
FOLLOW ?= 1
TAIL ?= 200
DEPLOY = ZUBSPACE_DEPLOY_FORCE=$(FORCE) scripts/deploy.sh
REMOTE = ZUBSPACE_DEPLOY_FORCE=$(FORCE) ZUBSPACE_LOG_FOLLOW=$(FOLLOW) ZUBSPACE_LOG_LINES=$(TAIL) scripts/remote.sh
AFTER_DEPLOY = $(if $(filter 1,$(PROBE)),$(MAKE) --no-print-directory probe-$(1))
.PHONY: deploy-dev deploy-prod promote-prod rollback-dev rollback-prod probe-dev probe-prod \
	deploy-notifier remote-status remote-history remote-logs-dev remote-logs-prod remote-logs-notifier \
	remote-restart-dev remote-restart-prod remote-shell remote-prometheus
deploy-dev deploy-prod: deploy-%:
	$(DEPLOY) $* "$(REF)"
	$(call AFTER_DEPLOY,$*)

promote-prod:
	$(DEPLOY) promote
	$(call AFTER_DEPLOY,prod)

rollback-dev rollback-prod: rollback-%:
	$(DEPLOY) rollback $*
	$(call AFTER_DEPLOY,$*)

# Headless Chrome joins the live site over Cloudflare and WebRTC (no window).
probe-dev:
	node tools/remote_join_probe.mjs https://dev.zubspace.gg/

probe-prod:
	node tools/remote_join_probe.mjs https://zubspace.gg/

# The player join/leave notifier (notifier/), its own project on the host.
# NOTIFIER_CONFIG=notifier/etc/notifier.json (git-ignored) also installs that
# real config there; without it the host keeps its own.
NOTIFIER_CONFIG ?=
deploy-notifier:
	NOTIFIER_CONFIG="$(NOTIFIER_CONFIG)" scripts/deploy-notifier.sh "$(REF)"

remote-status:
	$(REMOTE) status

remote-history:
	$(REMOTE) history

# SERVICE=client|cloudflared|prometheus (default server); FOLLOW=0 prints and exits.
remote-logs-dev remote-logs-prod remote-logs-notifier: remote-logs-%:
	$(REMOTE) logs $* $(SERVICE)

# Restarts every service, or only SERVICE, without redeploying.
remote-restart-dev remote-restart-prod: remote-restart-%:
	$(REMOTE) restart $* $(SERVICE)

remote-shell:
	$(REMOTE) shell

remote-prometheus:
	$(REMOTE) prometheus

.PHONY: test-e2e-weapon-birth-view
test-e2e-weapon-birth-view: build-client build-server
	node --test tests/e2e/weapon_birth_view_evidence.test.mjs
	ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/weapon_birth_view.mjs

.PHONY: test-e2e-spawn-rendezvous
test-e2e-spawn-rendezvous: build-client build-server
	node --test tests/e2e/player_rendezvous.test.mjs tests/e2e/rendezvous_staging.test.mjs tests/e2e/smoke_encounter_preparation.test.mjs
	ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/spawn_rendezvous.mjs

.PHONY: test-e2e-menu-shortcuts
test-e2e-menu-shortcuts: test-snapshot-parser build-server build-client
	node tests/e2e/menu_shortcuts.mjs

.PHONY: test-e2e-fullscreen test-e2e-fullscreen-firefox
test-e2e-fullscreen: build-client build-server
	ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_BROWSER=chrome node tests/e2e/fullscreen.mjs

test-e2e-fullscreen-firefox: build-client build-server
	ZUBSPACE_E2E_BROWSER=firefox ZUBSPACE_FIREFOX_HEADLESS=0 nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/fullscreen.mjs'

.PHONY: test-e2e-arena-admission
test-e2e-arena-admission: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/arena_admission.mjs

.PHONY: test-e2e-entry-reveal
test-e2e-entry-reveal: test-snapshot-parser build-server build-client
	ZUBSPACE_E2E_ADMISSION_ENTRY_ONLY=1 ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/arena_admission.mjs

.PHONY: test-e2e-main-menu
test-e2e-main-menu: test-rtc-config build-client build-server
	node tests/e2e/main_menu.mjs

.PHONY: test-e2e-spectator test-e2e-spectator-firefox
test-e2e-spectator: test-snapshot-parser build-client build-server
	node tests/e2e/spectator.mjs

.PHONY: test-e2e-held-primary-fire
test-e2e-held-primary-fire: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/held_primary_fire.mjs

.PHONY: test-e2e-spectator-bot-audio bench-client-debug-overhead
test-e2e-spectator-bot-audio: test-snapshot-parser build-client build-server
	node tests/e2e/spectator_bot_audio.mjs

bench-client-debug-overhead:
	ZUBSPACE_E2E_STATIC_ROOT=dist ZUBSPACE_SMOKE_LOG_LEVEL=info node tests/e2e/client_debug_overhead_probe.mjs

test-e2e-spectator-firefox: test-snapshot-parser build-client build-server
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_SPECTATOR_BROWSER=firefox xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/spectator.mjs'

.PHONY: test-observability test-e2e-observability
test-observability:
	node --test tests/player_telemetry.test.mjs tests/server_log_fields.test.mjs
	cargo test -p zubspace --bin zubspace_server observability

.PHONY: test-e2e-gun-bomb-boundary
test-e2e-gun-bomb-boundary: build-client build-server
	nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 ZUBSPACE_FIREFOX_HEADLESS=0 xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/gun_bomb_boundary.mjs'

.PHONY: test-e2e-keyboard-origin-recovery
test-e2e-keyboard-origin-recovery: build-client build-server
	node tests/e2e/keyboard_origin_recovery.mjs

test-e2e-observability: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info node tests/e2e/observability.mjs

.PHONY: test-e2e-timed-prize-visuals
test-e2e-timed-prize-visuals: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/timed_prize_visuals.mjs

.PHONY: test-e2e-clean-url
test-e2e-clean-url: test-snapshot-parser build-client build-server
	node tests/e2e/clean_url.mjs

.PHONY: test-e2e-connection-quality
test-e2e-connection-quality: test-snapshot-parser build-client build-server
	ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/connection_quality.mjs

.PHONY: test-e2e-bot-concealment
test-e2e-bot-concealment: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/bot_concealment.mjs

.PHONY: test-e2e-bot-detector-exhaustion
test-e2e-bot-detector-exhaustion: test-snapshot-parser build-client build-server
	ZUBSPACE_E2E_BOT_DETECTOR_EXHAUSTION=1 ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/bot_concealment.mjs

.PHONY: test-e2e-antiwarp-scope
test-e2e-antiwarp-scope: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/antiwarp_scope.mjs

.PHONY: test-e2e-multifire-bindings
test-e2e-multifire-bindings: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/multifire_bindings.mjs

.PHONY: test-e2e-status-energy
test-e2e-status-energy: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/status_energy.mjs

.PHONY: test-e2e-bot-ship-variety
test-e2e-bot-ship-variety: test-snapshot-parser build-client build-server
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR="$${ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR:+$${ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR}/variety}" ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/bot_ship_variety.mjs
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR="$${ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR:+$${ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR}/energy}" ZUBSPACE_E2E_BOT_WEAPON_ENERGY=1 ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/bot_ship_variety.mjs

.PHONY: test-e2e-bot-weapon-energy
test-e2e-bot-weapon-energy: test-snapshot-parser build-client build-server
	ZUBSPACE_E2E_BOT_SPAWN_FIXTURE=central ZUBSPACE_E2E_BOT_WEAPON_ENERGY=1 ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/bot_ship_variety.mjs

.PHONY: test-e2e-ship-abilities
test-e2e-ship-abilities: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/ship_abilities.mjs

.PHONY: test-e2e-ship-weapon-traits
test-e2e-ship-weapon-traits: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/ship_weapon_traits.mjs

.PHONY: test-e2e-decoy-presentation
test-e2e-decoy-presentation: test-snapshot-parser build-client build-server
	node tests/e2e/decoy_presentation.mjs

.PHONY: test-e2e-brick-warp
test-e2e-brick-warp: test-snapshot-parser build-client build-server
	node --test tests/e2e/weapon_lifecycle_fixture.test.mjs
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/brick_warp.mjs

.PHONY: test-e2e-brick-weapons
test-e2e-brick-weapons: test-snapshot-parser build-client build-server
	node --test tests/e2e/weapon_lifecycle_fixture.test.mjs tests/e2e/brick_weapon_stage.test.mjs
	ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/brick_weapons.mjs

.PHONY: test-e2e-shrapnel-damage
test-e2e-shrapnel-damage: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/shrapnel_damage.mjs

.PHONY: test-nginx-qa-proxy
test-nginx-qa-proxy:
	node --test tests/nginx_qa_proxy.test.mjs

.PHONY: test-e2e-enemy-energy-labels
test-e2e-enemy-energy-labels: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/enemy_energy_labels.mjs

.PHONY: test-e2e-connection-quality-enforcement test-e2e-connection-quality-loss
test-e2e-connection-quality-enforcement: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/connection_quality_enforcement.mjs

test-e2e-connection-quality-loss: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_QUALITY_PROFILE=degraded-loss node tests/e2e/connection_quality_enforcement.mjs

# Real tab switches with normal scheduling, separate from impaired-path QA.
# Keep these serial even when the caller requests parallel make jobs.
.PHONY: test-e2e-tab-return test-e2e-tab-return-chrome test-e2e-tab-return-firefox
test-e2e-tab-return:
	$(MAKE) test-e2e-tab-return-firefox
	$(MAKE) test-e2e-tab-return-chrome

test-e2e-tab-return-chrome: build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 ZUBSPACE_E2E_BROWSER=chrome node tests/e2e/tab_return_recovery.mjs

test-e2e-tab-return-firefox: build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_FIREFOX_HEADLESS=0 ZUBSPACE_E2E_BROWSER=firefox nix-shell -p geckodriver xvfb-run --run 'MOZ_ENABLE_WAYLAND=0 GDK_BACKEND=x11 xvfb-run -a -s "-screen 0 1440x1000x24" node tests/e2e/tab_return_recovery.mjs'

.PHONY: test-e2e-weapon-safety-lifecycle
test-e2e-weapon-safety-lifecycle: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/weapon_safety_lifecycle.mjs

.PHONY: test-e2e-weapon-cutover
test-e2e-weapon-cutover: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/weapon_cutover.mjs

.PHONY: test-e2e-expanded-scoring
test-e2e-expanded-scoring: test-snapshot-parser build-client build-server
	ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR="$${ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR:-$$(mktemp -d /tmp/zubspace-expanded-scoring-XXXXXX)/evidence}" ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/expanded_scoring.mjs

.PHONY: test-e2e-arena-kill-notifications
test-e2e-arena-kill-notifications: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/arena_kill_notifications.mjs

.PHONY: test-e2e-bot-projectile-presentation
test-e2e-bot-projectile-presentation: test-snapshot-parser
	ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/bot_projectile_presentation.mjs

.PHONY: test-e2e-maxed-respawn
test-e2e-maxed-respawn: test-snapshot-parser
	ZUBSPACE_SMOKE_LOG_LEVEL=debug ZUBSPACE_E2E_ARENA=extreme ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/maxed_respawn.mjs

# Narrow browser storage component; no gameplay/performance acceptance claim.
.PHONY: test-browser-zone-cache
test-browser-zone-cache:
	ZUBSPACE_E2E_HEADLESS=1 node tests/e2e/zone_cache_storage.mjs

.PHONY: test-e2e-zone-cache-join
test-e2e-zone-cache-join: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/zone_cache_join.mjs

.PHONY: test-e2e-identity-rejoin
test-e2e-identity-rejoin: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/identity_rejoin.mjs

.PHONY: test-e2e-spawn-idle
test-e2e-spawn-idle: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/spawn_idle.mjs

.PHONY: test-e2e-thor-proximity
test-e2e-thor-proximity: test-snapshot-parser build-client build-server
	ZUBSPACE_SMOKE_LOG_LEVEL=info ZUBSPACE_E2E_HEADLESS=0 node tests/e2e/thor_proximity.mjs
