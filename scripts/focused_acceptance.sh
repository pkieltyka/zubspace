#!/usr/bin/env bash
# The focused stress/recovery acceptance runner.
#
# The canonical 15-step matrix (scripts/frozen_matrix.sh) covers the native/config gates and the
# broad Chrome/Firefox x Chaos/Extreme clean and impairment matrices. It does not cover the focused
# stress/recovery, authority and special-weapon scenarios. This runner adds exactly those lanes,
# run the same way as the frozen matrix: it refuses a dirty tree, pins the head, keeps a per-lane
# log, and writes a result.json.
#
# Usage:
#   scripts/focused_acceptance.sh                 # every lane, stop at the first failure
#   scripts/focused_acceptance.sh --list          # print the lanes and run nothing
#   scripts/focused_acceptance.sh --from 4 --to 9 # run a range (inclusive)
#   scripts/focused_acceptance.sh --keep-going    # run later lanes after one fails; still exits non-zero
#
# Flags and powerballs are not enabled in the shipped zones, so their lanes (flag-carry-badge,
# flag-carrier-ride) are not here.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 2
source "$REPO_ROOT/scripts/acceptance_tree_guard.sh" || exit 2

# Lane commands, in order.
LANES=(
  "make test-e2e-snapshot-loss"
  "make test-e2e-input-recovery"
  "make test-e2e-firefox-input-recovery"
  "make test-e2e-semantic-input"
  "make test-e2e-baseline-receipt"
  "make test-e2e-warp-relocation"
  "make test-e2e-wormhole-authority"
  "make test-e2e-portal-phase2"
  "make test-e2e-prizes-phase1"
  "make test-e2e-negative-energy"
  "make test-e2e-objective-authority"
  "make test-e2e-clock-stall"
  "make test-e2e-active-projectile-recovery"
  "make test-e2e-chat-send-failure"
  "make test-e2e-dynamic-zones"
  "make test-e2e-special-weapons"
  "make test-e2e-bots"
  "make test-e2e-bots-firefox"
  "make test-e2e-immediate-keyboard-high-refresh"
  "make test-e2e-mixed-single-bullet"
  "make test-e2e-manual-bullet"
  "make test-e2e-aimed-bullet"
  "make test-e2e-bomb"
  "make test-e2e-transport-recovery"
  # Release/native and product/lifecycle coverage.
  "make test-integration-release"
  "make check-release-bundle"
  "make test-e2e-prize-baseline-delay"
  "make test-e2e-initial-file-request-retry"
  "make test-e2e-chaos-route-replan"
  "make test-e2e-main-menu"
  "make test-e2e-menu-shortcuts"
  "make test-e2e-spectator"
  "make test-e2e-spectator-firefox"
  "make test-e2e-held-primary-fire"
  "make test-e2e-observability"
  "make test-e2e-timed-prize-visuals"
  "make test-e2e-clean-url"
  "make test-e2e-connection-quality"
  "make test-e2e-bot-concealment"
  "make test-e2e-bot-detector-exhaustion"
  "make test-e2e-antiwarp-scope"
  "make test-e2e-multifire-bindings"
  "make test-e2e-status-energy"
  "make test-e2e-bot-ship-variety"
  "make test-e2e-ship-abilities"
  "make test-e2e-ship-weapon-traits"
  "make test-e2e-decoy-presentation"
  "make test-e2e-brick-warp"
  "make test-e2e-brick-weapons"
  "make test-e2e-shrapnel-damage"
  "make test-e2e-enemy-energy-labels"
  "make test-e2e-connection-quality-enforcement"
  "make test-e2e-connection-quality-loss"
  "make test-e2e-weapon-safety-lifecycle"
  "make test-e2e-weapon-cutover"
  "make test-e2e-expanded-scoring"
  "make test-e2e-bot-projectile-presentation"
  "make test-e2e-maxed-respawn"
  "node --test tests/nginx_qa_proxy.test.mjs"
  # Preserve reference Extreme direct-bomb coverage alongside maximum-entry
  # production proximity-bomb authority.
  "make test-e2e-bots-reference-extreme"
  # Actual autonomous bot audio while following; human audio is a separate lane.
  "make test-e2e-spectator-bot-audio"
  # DCEP metadata alone cannot prove the actual SCTP reply wire policy.
  "make test-vendor-rtc"
  # Same exact linked-shot/damage oracle, with independently impaired UDP paths.
  "make test-e2e-aimed-bullet-wan"
  # Actual human presentation holds, stationary drawn flight, and terminal tails.
  "make test-e2e-held-projectile"
  # Verified binary storage and ordinary HTTP/F5/clean-browser reuse over RTC.
  "make test-browser-zone-cache test-e2e-zone-cache-join"
  # Live duplicate rejection, bounded retry, and abrupt browser crash/rejoin.
  "make test-e2e-identity-rejoin"
  # Wall-clock spawn grace is separate from the connection-quality policy.
  "make test-e2e-spawn-idle"
  # Reference Thor sensor, stock power and real primary damage outside the hull.
  "make test-e2e-thor-proximity"
  # Random lifecycle positions need real-key encounter setup and held-view proof.
  "make test-e2e-spawn-rendezvous"
)

FROM=1
TO=${#LANES[@]}
KEEP_GOING=0
LIST_ONLY=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --list) LIST_ONLY=1; shift ;;
    --keep-going) KEEP_GOING=1; shift ;;
    --from) [[ $# -ge 2 ]] || { echo "--from requires a value" >&2; exit 2; }; FROM="$2"; shift 2 ;;
    --to) [[ $# -ge 2 ]] || { echo "--to requires a value" >&2; exit 2; }; TO="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

acceptance_validate_range "$FROM" "$TO" "${#LANES[@]}" || exit 2

if [[ $LIST_ONLY -eq 1 ]]; then
  for i in "${!LANES[@]}"; do
    printf '%2d  %s\n' "$((i + 1))" "${LANES[$i]}"
  done
  exit 0
fi

HEAD="$(git rev-parse HEAD)" || exit 2
SHORT="${HEAD:0:7}"
if ! acceptance_assert_unchanged_tree "$HEAD"; then
  echo "focused acceptance refuses a dirty tree; commit or stash first, or the result would describe" >&2
  echo "state nobody can rebuild. Reported dirty paths:" >&2
  git status --porcelain | head -20 >&2
  exit 3
fi

DIR="${ZUBSPACE_FOCUSED_ACCEPTANCE_DIR:-/tmp/zubspace-focused-acceptance-$(date -u +%Y%m%dT%H%M%SZ)-${SHORT}}"
mkdir -p "$DIR" || exit 2

echo "focused acceptance: head=$SHORT lanes=$((TO - FROM + 1)) of ${#LANES[@]} evidence=$DIR"

RESULT_LANES=""
FAILED=0
SOURCE_UNCHANGED=true
RUN_COUNT=0
for i in "${!LANES[@]}"; do
  lane=$((i + 1))
  if [[ $lane -lt $FROM || $lane -gt $TO ]]; then continue; fi
  name="${LANES[$i]}"
  log="$DIR/lane-$(printf '%02d' "$lane")-$(echo "$name" | tr -cs '[:alnum:]-' '-').log"
  started=$(date +%s)
  gate_code=null
  lane_source_unchanged=true
  if ! acceptance_assert_unchanged_tree "$HEAD" > "$log" 2>&1; then
    lane_source_unchanged=false
    SOURCE_UNCHANGED=false
    code=4
  else
    # Route headed browsers to the silent workspace when Hyprland is running, same as any other lane.
    if [[ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" && -x "$REPO_ROOT/scripts/e2e-hyprland-tools/run" ]]; then
      "$REPO_ROOT/scripts/e2e-hyprland-tools/run" -- bash -c "$name" > "$log" 2>&1
      code=$?
    else
      bash -c "$name" > "$log" 2>&1
      code=$?
    fi
    gate_code=$code
    if ! acceptance_assert_unchanged_tree "$HEAD" >> "$log" 2>&1; then
      lane_source_unchanged=false
      SOURCE_UNCHANGED=false
      code=4
    fi
  fi
  RUN_COUNT=$((RUN_COUNT + 1))
  elapsed=$(( $(date +%s) - started ))
  status=pass
  if [[ $code -ne 0 ]]; then status=fail; FAILED=1; fi
  printf 'lane %02d: %-48s %s exit=%d %ds log=%s\n' \
    "$lane" "$name" "$status" "$code" "$elapsed" "$(basename "$log")"
  RESULT_LANES="${RESULT_LANES}${RESULT_LANES:+,}{\"lane\":$lane,\"gate\":\"$name\",\"exit\":$code,\"seconds\":$elapsed,\"gateExit\":$gate_code,\"sourceUnchanged\":$lane_source_unchanged}"
  if [[ "$SOURCE_UNCHANGED" != true || ( $code -ne 0 && $KEEP_GOING -eq 0 ) ]]; then
    echo "focused acceptance stopped at lane $lane ($name); evidence=$DIR"
    break
  fi
done

if ! acceptance_assert_unchanged_tree "$HEAD" >> "$DIR/tree-guard.log" 2>&1; then
  SOURCE_UNCHANGED=false
  FAILED=1
fi

cat > "$DIR/result.json" <<JSON
{"kind":"focused-acceptance","head":"$HEAD","runLanes":$RUN_COUNT,"requestedLanes":$((${TO} - ${FROM} + 1)),"sourceUnchanged":$SOURCE_UNCHANGED,"totalLanes":${#LANES[@]},"from":$FROM,"to":$TO,"failed":$FAILED,"evidence":"$DIR","lanes":[$RESULT_LANES]}
JSON

if [[ $FAILED -ne 0 ]]; then
  echo "focused acceptance: RED (evidence=$DIR)"
  exit 1
fi
echo "focused acceptance: GREEN head=$SHORT (evidence=$DIR)"
