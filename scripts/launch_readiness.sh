#!/usr/bin/env bash
# Supplement the canonical frozen and focused acceptance surfaces. Deliberately
# sequential: timing-sensitive browser lanes must not compete with compilation.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 2
STEPS=(
  'node --test --test-concurrency=1 tests/*.test.mjs tests/e2e/*.test.mjs'
  'make test-integration-release'
  'make check-release-bundle'
  'scripts/frozen_matrix.sh --keep-going'
  'scripts/focused_acceptance.sh --keep-going'
  'make test-e2e-ship-energy'
  'make test-e2e-menu-shortcuts'
  'make test-e2e-main-menu'
  'make test-e2e-timed-prize-visuals'
  'make test-e2e-clean-url'
  'make test-e2e-connection-quality'
  'make test-e2e-connection-quality-enforcement'
  'make test-e2e-connection-quality-loss'
  'make test-e2e-bot-concealment'
  'make test-e2e-antiwarp-scope'
  'make test-e2e-multifire-bindings'
  'make test-e2e-status-energy'
  'make test-e2e-bot-ship-variety'
  'make test-e2e-ship-abilities'
  'make test-e2e-decoy-presentation'
  'make test-e2e-enemy-energy-labels'
  'make test-e2e-spectator'
  'make test-e2e-spectator-firefox'
  'make test-e2e-observability'
  'ZUBSPACE_SERVER_LOG_FORMAT=json ZUBSPACE_SMOKE_LOG_LEVEL=debug make test-e2e-combat'
  'make test-e2e-prize-baseline-delay'
  'make test-e2e-initial-file-request-retry'
  'make test-e2e-chaos-route-replan'
  'make test-e2e-flag-carrier-ride'
  'make test-e2e-flag-carry-badge'
  'make test-e2e-moving-peers-gpu'
  'make smoke-real-server'
  'make smoke-firefox-rtc'
  'ZUBSPACE_SERVER_QA_TRACE_UPLOAD=1 ZUBSPACE_E2E_SERVER_PROFILE=release node tests/e2e/special_weapons.mjs'
)
if [[ ${1:-} == --list ]]; then printf '%s\n' "${STEPS[@]}"; exit 0; fi
FROM=${1:-1}
[[ $FROM =~ ^[0-9]+$ ]] && ((FROM >= 1 && FROM <= ${#STEPS[@]})) || exit 2
HEAD=$(git rev-parse HEAD)
if [[ -n $(git status --porcelain) ]]; then echo 'launch readiness requires a clean committed tree' >&2; exit 3; fi
DIR=${ZUBSPACE_LAUNCH_READINESS_DIR:-/tmp/zubspace-launch-readiness-$(date -u +%Y%m%dT%H%M%SZ)-${HEAD:0:8}}
mkdir -p "$DIR" || exit 2
export ZUBSPACE_FROZEN_MATRIX_DIR="$DIR/frozen"
export ZUBSPACE_FOCUSED_ACCEPTANCE_DIR="$DIR/focused"
export RETAINED_GATE_ROOT="$DIR/retained"
export ZUBSPACE_E2E_ARTIFACT_ROOT="$DIR/browser"
printf 'launch readiness head=%s evidence=%s\n' "$HEAD" "$DIR"
printf '{"head":"%s","steps":[\n' "$HEAD" > "$DIR/result.json"
FAILED=0
SEP=''
for i in "${!STEPS[@]}"; do
  step=$((i + 1)); ((step < FROM)) && continue
  if [[ $(git rev-parse HEAD) != "$HEAD" || -n $(git status --porcelain) ]]; then echo 'source changed during acceptance; stopping' >&2; FAILED=1; break; fi
  command=${STEPS[$i]}
  log="$DIR/step-$(printf '%02d' "$step").log"
  started=$(date +%s)
  # Wrap the entire runner so even retained smokes nested in the frozen matrix
  # inherit the silent-workspace rule. Nested enable/disable is idempotent.
  scripts/e2e-hyprland-tools/run -- bash -c "$command" > "$log" 2>&1
  code=$?
  elapsed=$(( $(date +%s) - started ))
  ((code != 0)) && FAILED=1
  printf 'step %02d exit=%d %ds %s log=%s\n' "$step" "$code" "$elapsed" "$command" "$log"
  # Commands are constants above, so JSON escaping is unnecessary here.
  printf '%s{"step":%d,"command":"%s","exit":%d,"seconds":%d,"log":"step-%02d.log"}' "$SEP" "$step" "$command" "$code" "$elapsed" "$step" >> "$DIR/result.json"
  SEP=','
done
if [[ $(git rev-parse HEAD) != "$HEAD" || -n $(git status --porcelain) ]]; then FAILED=1; echo 'source changed before final acceptance snapshot' >&2; fi
printf '],"failed":%d}\n' "$FAILED" >> "$DIR/result.json"
exit "$FAILED"
