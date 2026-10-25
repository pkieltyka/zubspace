#!/usr/bin/env bash
# Inspect and operate the environments on the production host.
#
#   scripts/remote.sh status                    # both environments and the host
#   scripts/remote.sh history [prod|dev]        # recorded deploys, newest last
#   scripts/remote.sh logs <prod|dev> [service] # server (default), client, cloudflared, prometheus; or logs notifier
#   scripts/remote.sh sessions <prod|dev>       # one line per player session, then totals
#   scripts/remote.sh restart <prod|dev> [service]
#   scripts/remote.sh shell                     # interactive shell in ~/zubspace
#   scripts/remote.sh prometheus                # forward both Prometheus UIs
#
# Deploying, promoting and rolling back are scripts/deploy.sh.
#
#   ZUBSPACE_DEPLOY_HOST   ssh destination (default peter@zubspace-prod)
#   ZUBSPACE_DEPLOY_FORCE  1 restarts prod even with players online
#   ZUBSPACE_LOG_FOLLOW    0 prints the log tail and exits (default 1 follows)
#   ZUBSPACE_LOG_LINES     lines of history before following (default 200)
#   ZUBSPACE_SESSION_HOURS hours of sessions to summarize (default 24)
set -euo pipefail

usage() {
  sed -n '4,10s/^#   //p' "$0" >&2
  exit 2
}
host=${ZUBSPACE_DEPLOY_HOST:-peter@zubspace-prod}
remote() { ssh "$host" "$@"; }
need_env() { [[ "${1:-}" =~ ^(prod|dev)$ ]] || usage; }
cd "$(git rev-parse --show-toplevel)"
# Commit subject when the commit exists in this checkout.
subject() { git log -1 --format=%s "$1" 2>/dev/null || true; }

command=${1:-}
case "$command" in
  status)
    # One `|`-separated line per environment, then one for the host.
    remote bash -s <<'EOF' | while IFS='|' read -r kind env commit game containers deployed; do
for env in prod dev; do
  d=~/zubspace/$env
  [ -f "$d/compose.env" ] || continue
  client=$(sed -n 's/^ZUBSPACE_CLIENT_PORT=//p' "$d/compose.env")
  http=$(sed -n 's/^ZUBSPACE_SERVER_HTTP_PORT=//p' "$d/compose.env")
  commit=$(curl -fsS --max-time 3 "127.0.0.1:$client/BUILD_INFO.json" | jq -er .git) || commit="not serving"
  # Builds before the /v1 prefix answer only /status.
  game=$({ curl -fsS --max-time 3 "127.0.0.1:$http/v1/status" || curl -fsS --max-time 3 "127.0.0.1:$http/status"; } 2>/dev/null | jq -r '"total \(.total_players // .total // .num_players // ((.players // 0) + (.bots // 0)))  sessions \(.rtc_sessions)  up \((.uptime // .uptime_seconds) / 3600 | floor)h\((.uptime // .uptime_seconds) % 3600 / 60 | floor)m"') \
    || game="game server not answering"
  containers=$(podman ps -a --filter "label=com.docker.compose.project=zubspace-$env" \
      --format '{{index .Labels "com.docker.compose.service"}} {{.Status}}' \
    | sed -E 's/ Up .*\((.*)\)$/ \1/; s/ Up .*/ up/; s/ (Exited|Created).*/ DOWN/' | sort | paste -sd, - | sed 's/,/, /g')
  deployed=$(tail -n 1 "$d/deploys.log" 2>/dev/null | awk '{ print $1 " (" $3 ")" }')
  echo "env|$env|$commit|$game|$containers|${deployed:-unrecorded}"
done
if [ -f ~/zubspace/notifier/release.env ]; then
  commit=$(sed -n 's/^NOTIFIER_COMMIT=//p' ~/zubspace/notifier/release.env)
  state=$(podman ps -a --filter label=com.docker.compose.project=zubspace-notifier --format '{{.Status}}' \
    | sed -E 's/^Up .*/up/; s/^(Exited|Created).*/DOWN/' | head -n 1)
  deployed=$(tail -n 1 ~/zubspace/notifier/deploys.log 2>/dev/null | awk '{ print $1 }')
  echo "notifier|notifier|$commit|${state:-no container}||${deployed:-unrecorded}"
fi
disk=$(df -h --output=used,size,pcent / | tail -n 1 | awk '{ print $1 " of " $2 " (" $3 ")" }')
mem=$(free -h | awk '/^Mem:/ { print $3 " of " $2 }')
journal=$(du -sh /var/log/journal 2>/dev/null | cut -f1)
echo "host|$(hostname)|$disk|$mem|$(cut -d' ' -f1-3 /proc/loadavg)|${journal:-?}"
EOF
      if [ "$kind" = env ]; then
        printf '%s  %s  %s\n' "$env" "${commit:0:12}" "$(subject "$commit")"
        printf '      %s\n      %s\n      deployed %s\n' "$game" "$containers" "$deployed"
      elif [ "$kind" = notifier ]; then
        printf 'notifier  %s  %s\n      notifier %s\n      deployed %s\n' "${commit:0:12}" "$(subject "$commit")" "$game" "$deployed"
      else
        printf '%s  disk %s  memory %s  load %s  journal %s\n' "$env" "$commit" "$game" "$containers" "$deployed"
      fi
    done
    ;;
  history)
    envs=${2:-prod dev}
    for env in $envs; do
      need_env "$env"
      echo "$env:"
      remote "cat ~/zubspace/$env/deploys.log 2>/dev/null || true" | while read -r when commit mode who; do
        printf '  %s  %s  %-8s %s  %s\n' "$when" "${commit:0:12}" "$mode" "$who" "$(subject "$commit")"
      done
    done
    ;;
  logs)
    [[ "${2:-}" =~ ^(prod|dev|notifier)$ ]] || usage
    env=$2 service=${3:-server}
    # The notifier project has a single service.
    if [ "$env" = notifier ]; then service=notifier; fi
    lines=${ZUBSPACE_LOG_LINES:-200}
    follow_file='' follow_journal=''
    [ "${ZUBSPACE_LOG_FOLLOW:-1}" = 1 ] && follow_file=-F follow_journal=-f
    # A server with log_file set (dev) writes there instead of the journal.
    if [ "$service" = server ] && remote "test -s ~/zubspace/$env/logs/server.log"; then
      remote "tail -n $lines $follow_file ~/zubspace/$env/logs/server.log"
    else
      remote "podman logs --tail $lines $follow_journal zubspace-$env-$service-1"
    fi
    ;;
  sessions)
    need_env "${2:-}"
    env=$2
    since=$(date -u -d "-${ZUBSPACE_SESSION_HOURS:-24} hours" +%Y-%m-%dT%H:%M:%SZ)
    # Each removed player session logs one `session_summary` line
    # (docs/observability.md). Dev logs to a rotated file, prod to the journal.
    if remote "test -s ~/zubspace/$env/logs/server.log"; then
      remote "cat ~/zubspace/$env/logs/server.log.1 ~/zubspace/$env/logs/server.log 2>/dev/null \
        | grep -F '\"session_summary\"' || true"
    else
      remote "podman logs --since $since zubspace-$env-server-1 2>&1 | grep -F '\"session_summary\"' || true"
    fi | jq -rs --arg since "$since" '
      def n(f): (f // 0) * 10 | round / 10;
      # Server probe RTT, else the browser ICE RTT; null when never measured
      # (a browser can report 0 before its first ICE measurement).
      def rtt: [.probe_rtt_ms_mean, .ice_rtt_ms_mean] | map(select(. != null and . > 0)) | first;
      def band(lo; hi): [.[] | rtt as $r | select($r != null and $r >= lo and $r < hi)] | length;
      [.[] | select(.timestamp >= $since) | .fields + {at: .timestamp[5:16]}
        | select(.name | test("^probe[0-9]+$") | not)] as $s
      | "\($s | length) player sessions since \($since) (deploy probes excluded)",
        ($s[] | "\(.at)  \(.name)  \(.duration_s)s  \(if .ship >= 8 then "spectator" else "ship \(.ship + 1)" end)  K/D \(.kills)/\(.deaths)  \(n(.fps_mean)) fps  slow \(.slow_frames)  corrections \(.corrections)  rtt \(rtt // "-" | if type == "number" then "\(round) ms" else . end)  snap-backs \(.snapbacks)  shots \(.weapons_fired) fired, \(.weapons_rejected) refused (\(.weapons_rejected_combat_view) unverified view)"),
        "rtt: \($s | band(0; 80)) under 80 ms, \($s | band(80; 150)) 80-149, \($s | band(150; 300)) 150-299, \($s | band(300; 1e9)) 300+, \([$s[] | select(rtt == null)] | length) unmeasured",
        "under 55 fps: \([$s[] | select(.fps_mean != null and .fps_mean < 55)] | length); with snap-backs: \([$s[] | select(.snapbacks > 0)] | length)",
        "shots: \([$s[].weapons_fired] | add // 0) fired, \([$s[].weapons_rejected] | add // 0) refused, \([$s[].weapons_rejected_combat_view] | add // 0) of them for an unverified view"'
    ;;
  restart)
    need_env "${2:-}"
    env=$2 service=${3:-}
    if [ "$env" = prod ] && [ "${ZUBSPACE_DEPLOY_FORCE:-0}" != 1 ]; then
      sessions=$(remote "port=\$(sed -n 's/^ZUBSPACE_SERVER_HTTP_PORT=//p' ~/zubspace/prod/compose.env) \
        && { curl -fsS --max-time 3 http://127.0.0.1:\$port/v1/status || curl -fsS --max-time 3 http://127.0.0.1:\$port/status; } 2>/dev/null \
        | jq -r '.rtc_sessions // 0'" || echo 0)
      if [ "${sessions:-0}" -gt 0 ]; then
        echo "remote: $sessions player session(s) online in prod; rerun with ZUBSPACE_DEPLOY_FORCE=1 (make ... FORCE=1)." >&2
        exit 3
      fi
    fi
    remote "cd ~/zubspace/$env && PODMAN_COMPOSE_WARNING_LOGS=false podman compose --project-name zubspace-$env \
      --env-file compose.env --env-file release.env restart $service"
    echo "remote: restarted zubspace-$env ${service:-(all services)}; check with make remote-status or make probe-$env"
    ;;
  shell)
    exec ssh -t "$host" 'cd ~/zubspace && exec "$SHELL" -l'
    ;;
  prometheus)
    # Local 9090 belongs to the development Prometheus (etc/prom).
    echo "remote: prod http://127.0.0.1:19090  dev http://127.0.0.1:19091  (Ctrl-C to stop)"
    exec ssh -N -L 19090:127.0.0.1:9090 -L 19091:127.0.0.1:9091 "$host"
    ;;
  *) usage ;;
esac
