# shellcheck shell=bash
# Shared helpers for scripts/e2e-hyprland-tools. Source this file; do not run it.
#
# Every headed browser the zubspace test launchers open carries the window
# class $ZUBSPACE_E2E_WINDOW_CLASS: tests/e2e/combat_replay.mjs passes
# --class to Chrome and tools/browser_firefox_rtc_smoke.mjs passes it to
# Firefox. These tools register Hyprland runtime window rules that match ONLY
# that class, so the user's own Chrome and Firefox windows are never touched.

ZUBSPACE_E2E_WINDOW_CLASS="${ZUBSPACE_E2E_WINDOW_CLASS:-zubspace-e2e}"
ZUBSPACE_E2E_HYPR_WORKSPACE="${ZUBSPACE_E2E_HYPR_WORKSPACE:-99}"
export ZUBSPACE_E2E_WINDOW_CLASS ZUBSPACE_E2E_HYPR_WORKSPACE

tool_name=$(basename "$0")

log() { printf 'e2e-hyprland-tools/%s: %s\n' "$tool_name" "$*" >&2; }
die() { log "$@"; exit 1; }

case "$ZUBSPACE_E2E_WINDOW_CLASS" in
  ""|*[!A-Za-z0-9_-]*)
    die "ZUBSPACE_E2E_WINDOW_CLASS must match [A-Za-z0-9_-]+ (got '$ZUBSPACE_E2E_WINDOW_CLASS')" ;;
esac
case "$ZUBSPACE_E2E_HYPR_WORKSPACE" in
  ""|*[!0-9]*)
    die "ZUBSPACE_E2E_HYPR_WORKSPACE must be a workspace number (got '$ZUBSPACE_E2E_HYPR_WORKSPACE')" ;;
esac

hypr_session_present() { [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]; }

hypr_reachable() {
  command -v hyprctl >/dev/null 2>&1 && hyprctl version >/dev/null 2>&1
}

# Returns 0 when the tool should proceed and 3 when there is no Hyprland
# session (callers treat that as a no-op). Exits 2 when Hyprland is running
# but hyprctl cannot reach it, which usually means a sandbox is blocking the
# Hyprland unix socket.
hypr_check() {
  if ! hypr_session_present; then
    log "no Hyprland session (HYPRLAND_INSTANCE_SIGNATURE unset); nothing to do"
    return 3
  fi
  if ! hypr_reachable; then
    log "Hyprland is running but hyprctl cannot reach it (\"Couldn't open a socket\")."
    log "Rerun outside any sandbox that blocks unix sockets; hyprctl needs the Hyprland socket."
    exit 2
  fi
}

# Run a Lua chunk inside Hyprland and print what it returns. The chunk sees
# the locals CLASS and WORKSPACE. `hyprctl repl` prints return values;
# `hyprctl eval` only prints "ok".
hypr_lua() {
  hyprctl repl "local CLASS = \"$ZUBSPACE_E2E_WINDOW_CLASS\" local WORKSPACE = \"$ZUBSPACE_E2E_HYPR_WORKSPACE\" $1"
}

# Number of mapped windows carrying the test class.
hypr_test_window_count() {
  hypr_lua 'local n = 0 for _, w in ipairs(hl.get_windows()) do if w.class == CLASS then n = n + 1 end end return tostring(n)'
}
