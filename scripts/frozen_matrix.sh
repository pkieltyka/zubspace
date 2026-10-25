#!/usr/bin/env bash
# The canonical acceptance-matrix runner: the native and configuration gates plus the broad
# Chrome/Firefox x Chaos/Extreme clean and impairment matrices, run in order on one unchanged tree.
# scripts/focused_acceptance.sh adds the focused authority/recovery lanes this list does not cover.
#
# Usage:
#   scripts/frozen_matrix.sh                 # run every step, stop at the first failure
#   scripts/frozen_matrix.sh --list          # print the numbered list, run nothing
#   scripts/frozen_matrix.sh --from 6 --to 9 # run a range (inclusive)
#   scripts/frozen_matrix.sh --keep-going    # run later steps after one fails; still exits non-zero
#
# Output: one status line per step on stdout, a per-step log under the evidence directory, and a result.json
# recording the exact tree, so a green tail cannot be attached to a tree that did not produce it.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 2
source "$REPO_ROOT/scripts/acceptance_tree_guard.sh" || exit 2

# Steps, in order.
STEPS=(
  "cargo fmt --check"
  "cargo test -p zubspace"
  "cargo check --all-targets"
  "make test-snapshot-parser"
  "make test-wasm-identity"
  "make test-rtc-config"
  "make test-multiplayer-correctness"
  "make test-e2e-ordinary-url"
  "make test-e2e-movement-correction"
  "make test-e2e-stale-door"
  "make test-e2e-moving-peers"
  "make test-e2e-immediate-keyboard-mild"
  "make test-e2e-immediate-quality"
  "make test-retained-external-gates"
  "make test-e2e-aged-server-authority"
)

FROM=1
TO=${#STEPS[@]}
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

acceptance_validate_range "$FROM" "$TO" "${#STEPS[@]}" || exit 2

if [[ $LIST_ONLY -eq 1 ]]; then
  for i in "${!STEPS[@]}"; do
    printf '%2d  %s\n' "$((i + 1))" "${STEPS[$i]}"
  done
  exit 0
fi

HEAD="$(git rev-parse HEAD)" || exit 2
SHORT="${HEAD:0:7}"
# An uncommitted tree cannot be rebuilt from the recorded head, so the result would describe nothing.
if ! acceptance_assert_unchanged_tree "$HEAD"; then
  echo "frozen matrix refuses to run on a dirty tree; commit or stash first, or a renewal describes state that" >&2
  echo "cannot be reconstructed. Reported dirty paths:" >&2
  git status --porcelain | head -20 >&2
  exit 3
fi

DIR="${ZUBSPACE_FROZEN_MATRIX_DIR:-/tmp/zubspace-frozen-matrix-$(date -u +%Y%m%dT%H%M%SZ)-${SHORT}}"
mkdir -p "$DIR" || exit 2

# Retained gates refuse to overwrite an existing artifact directory, and the Makefile's RETAINED_GATE_ROOT default is
# a fixed /tmp path, so a second run would fail on the previous run's evidence. Scope the root to this run's evidence
# directory instead, next to the per-step logs that describe it.
export RETAINED_GATE_ROOT="${ZUBSPACE_FROZEN_MATRIX_RETAINED_ROOT:-$DIR/retained}"

echo "frozen matrix: head=$SHORT steps=$((TO - FROM + 1)) of ${#STEPS[@]} evidence=$DIR"

RESULT_STEPS=""
FAILED=0
SOURCE_UNCHANGED=true
RUN_COUNT=0
for i in "${!STEPS[@]}"; do
  step=$((i + 1))
  if [[ $step -lt $FROM || $step -gt $TO ]]; then continue; fi
  name="${STEPS[$i]}"
  log="$DIR/step-$(printf '%02d' "$step")-$(echo "$name" | tr -cs '[:alnum:]-' '-' | sed 's/-$//').log"
  started=$(date +%s)
  gate_code=null
  lane_source_unchanged=true
  if ! acceptance_assert_unchanged_tree "$HEAD" > "$log" 2>&1; then
    lane_source_unchanged=false
    SOURCE_UNCHANGED=false
    code=4
  else
    # Route headed browsers to the silent workspace when Hyprland is running, same as any other lane. Only
    # browser steps: registering the window rule around a native step toggles a rule for a step that launches
    # nothing, which is noise in the log and nothing else.
    if [[ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" && -x "$REPO_ROOT/scripts/e2e-hyprland-tools/run" && ( "$name" == *e2e* || "$name" == *test-retained-external-gates* ) ]]; then
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
  printf 'step %02d: %-45s %s exit=%d %ds log=%s\n' \
    "$step" "$name" "$status" "$code" "$elapsed" "$(basename "$log")"
  if [[ -z "$RESULT_STEPS" ]]; then RESULT_STEPS=""; fi
  RESULT_STEPS="${RESULT_STEPS}${RESULT_STEPS:+,}{\"step\":$step,\"name\":\"$name\",\"exit\":$code,\"seconds\":$elapsed,\"gateExit\":$gate_code,\"sourceUnchanged\":$lane_source_unchanged}"
  if [[ "$SOURCE_UNCHANGED" != true || ( $code -ne 0 && $KEEP_GOING -eq 0 ) ]]; then
    echo "frozen matrix stopped at step $step ($name); evidence=$DIR"
    break
  fi
done

if ! acceptance_assert_unchanged_tree "$HEAD" >> "$DIR/tree-guard.log" 2>&1; then
  SOURCE_UNCHANGED=false
  FAILED=1
fi

cat > "$DIR/result.json" <<JSON
{"kind":"frozen-matrix","head":"$HEAD","runSteps":$RUN_COUNT,"requestedSteps":$((${TO} - ${FROM} + 1)),"sourceUnchanged":$SOURCE_UNCHANGED,"totalSteps":${#STEPS[@]},"from":$FROM,"to":$TO,"failed":$FAILED,"evidence":"$DIR","steps":[$RESULT_STEPS]}
JSON

if [[ $FAILED -ne 0 ]]; then
  echo "frozen matrix: RED (evidence=$DIR)"
  exit 1
fi
echo "frozen matrix: GREEN head=$SHORT (evidence=$DIR)"
