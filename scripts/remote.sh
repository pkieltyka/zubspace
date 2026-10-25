#!/usr/bin/env bash
# Inspect and operate the environments on the production host.
#
#   scripts/remote.sh status                    # both environments and the host
#   scripts/remote.sh history [prod|dev]        # recorded deploys, newest last
#   scripts/remote.sh logs <prod|dev> [service] # server (default), client, cloudflared, prometheus; or logs notifier
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
set -euo pipefail

usage() {
  sed -n '4,9s/^#   //p' "$0" >&2
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
