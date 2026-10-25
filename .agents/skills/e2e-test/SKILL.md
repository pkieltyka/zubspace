---
name: e2e-test
description: Use when working on zubspace browser E2E tests, multiplayer RTC QA, combat replay scenarios, or validating real server/client gameplay with Chrome or Firefox keyboard input.
---

# Zubspace E2E Test

Use this skill when the task involves browser E2E coverage, multiplayer
sync QA, WebRTC/DataChannel gameplay validation, or combat replay tests.

## Workflow

1. Read `AGENTS.md` and [`docs/e2e-diagnostics.md`](../../../docs/e2e-diagnostics.md).
2. Build the current server and WASM client before running browser E2E:
   `make build-client && make build-server`.
3. Under Hyprland, run browser gates through
   `scripts/e2e-hyprland-tools/run -- <command>` (or `enable` once and
   `disable` when done) so test windows open on silent workspace 99; see
   the Test Runner section in `AGENTS.md`.
4. Prefer `make test-e2e-combat` for the core kill -> respawn -> damage
   regression path. Use `ZUBSPACE_E2E_ARENA` and `ZUBSPACE_E2E_CYCLES`
   when the scenario needs a non-default arena or repeat count.
5. Keep scenario-oriented scripts in `tests/e2e/`. Keep reusable
   service/browser helpers in `tools/` when smoke tests also use them.
6. Do not use raw gameplay packet hooks in browser E2E. Drive real
   keyboard input through DevTools/WebDriver and normal browser
   networking. Raw hooks belong in protocol/authority tests.
7. For combat tests, verify both players' perspectives plus server
   authority: movement, bullets, bombs, health/energy, death packet,
   score update, respawn timer, post-respawn damage, and a repeated
   cycle.
8. On failure, preserve compact artifacts: server log, browser packet
   logs, damage traces, UI metrics, and screenshots.

## Cleanup

It is allowed to stop stale local zubspace servers, static servers, and
browser instances left behind by E2E runs without asking first.
