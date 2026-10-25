#!/usr/bin/env bash
# Build one commit on the production host and (re)start one environment there.
#
#   scripts/deploy.sh <prod|dev> [git-ref]     # default ref: HEAD
#   scripts/deploy.sh promote                  # prod := the commit dev serves
#   scripts/deploy.sh rollback <prod|dev>      # that environment's previous deploy
#
# Ships `git archive` of the commit to the host (committed files only and no
# .git, so local work in progress never reaches it), builds both images there
# with rootless Podman unless that commit's images already exist, installs
# deploy/compose.yml plus deploy/<env>/ from the same commit into
# ~/zubspace/<env>/ and restarts that environment's Compose project. Prod is not
# restarted while players are connected unless ZUBSPACE_DEPLOY_FORCE=1. Each
# successful deploy is appended to ~/zubspace/<env>/deploys.log, which rollback
# reads; rolling back twice returns to where you started. The newest three
# builds are kept on the host.
#
#   ZUBSPACE_DEPLOY_HOST   ssh destination (default peter@zubspace-prod)
#   ZUBSPACE_DEPLOY_FORCE  1 restarts prod even with players online
set -euo pipefail

usage() {
  echo "usage: scripts/deploy.sh <prod|dev> [git-ref] | promote | rollback <prod|dev>" >&2
  exit 2
}
host=${ZUBSPACE_DEPLOY_HOST:-peter@zubspace-prod}
remote() { ssh "$host" "$@"; }
# The full commit an environment's nginx is serving right now.
serving() {
  remote "port=\$(sed -n 's/^ZUBSPACE_CLIENT_PORT=//p' ~/zubspace/$1/compose.env) \
    && curl -fsS --max-time 3 http://127.0.0.1:\$port/BUILD_INFO.json | jq -er .git"
}
cd "$(git rev-parse --show-toplevel)"

mode=deploy
case "${1:-}" in
  promote)
    mode=promote env=prod
    ref=$(serving dev) || { echo "deploy: cannot read the commit dev serves" >&2; exit 1; }
    ;;
  rollback)
    mode=rollback env=${2:-}
    [[ "$env" =~ ^(prod|dev)$ ]] || usage
    current=$(serving "$env" || true)
    ref=$(remote "tac ~/zubspace/$env/deploys.log 2>/dev/null || true" | awk -v cur="$current" '$2 != cur { print $2; exit }')
    [ -n "$ref" ] || { echo "deploy: no earlier $env deploy is recorded; use scripts/deploy.sh $env <commit>" >&2; exit 1; }
    ;;
  *) env=${1:-} ref=${2:-HEAD} ;;
esac
case "$env" in
  prod) url=https://zubspace.gg ;;
  dev) url=https://dev.zubspace.gg ;;
  *) usage ;;
esac
commit=$(git rev-parse --verify "$ref^{commit}")
tag=${commit:0:12}
dir=zubspace/$env
src=zubspace/src/$tag

setting() { git show "$commit:deploy/$env/compose.env" | sed -n "s/^$1=//p"; }

echo "deploy: $mode $env -> $(git log -1 --format='%h %s' "$commit")"
current=$(serving "$env" 2>/dev/null || true)
if [ "$current" = "$commit" ]; then
  echo "deploy: $env already serves $tag"
elif [ -n "$current" ] && git cat-file -e "$current^{commit}" 2>/dev/null; then
  read -r behind ahead < <(git rev-list --left-right --count "$current...$commit")
  echo "deploy: $env serves ${current:0:12}; $tag adds $ahead commit(s) and drops $behind"
fi
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo "deploy: note: uncommitted changes in this checkout are not part of $tag"
fi

http_port=$(setting ZUBSPACE_SERVER_HTTP_PORT)
client_port=$(setting ZUBSPACE_CLIENT_PORT)
# The running server's status JSON. Builds before the /v1 prefix answer only
# /status, and a deploy or rollback may meet one of them.
server_status="{ curl -fsS --max-time 3 http://127.0.0.1:$http_port/v1/status \
  || curl -fsS --max-time 3 http://127.0.0.1:$http_port/status; } 2>/dev/null"
if ! git show "$commit:deploy/$env/zubspace_server.toml" | grep -qx "bind = \"127.0.0.1:$http_port\""; then
  echo "deploy: deploy/$env/zubspace_server.toml bind does not match ZUBSPACE_SERVER_HTTP_PORT=$http_port" >&2
  exit 1
fi
remote "test -s ~/$dir/cloudflared.env" || {
  echo "deploy: ~/$dir/cloudflared.env (TUNNEL_TOKEN) is missing on $host" >&2
  exit 1
}

if remote "podman image exists localhost/zubspace/server:$tag && podman image exists localhost/zubspace/client:$tag"; then
  echo "deploy: images for $tag already on $host"
else
  echo "deploy: uploading $commit"
  git archive --format=tar "$commit" | gzip -1 \
    | remote "rm -rf ~/$src && mkdir -p ~/$src && tar -xzf - -C ~/$src"
  for image in server client; do
    echo "deploy: building zubspace/$image:$tag on $host"
    remote "cd ~/$src && podman build --build-arg ZUBSPACE_BUILD_COMMIT=$commit \
      -f Dockerfile.$image -t zubspace/$image:$tag ."
  done
fi

if [ "$env" = prod ] && [ "${ZUBSPACE_DEPLOY_FORCE:-0}" != 1 ]; then
  sessions=$(remote "$server_status | jq -r '.rtc_sessions // 0'" || echo 0)
  if [ "${sessions:-0}" -gt 0 ]; then
    echo "deploy: $sessions player session(s) online in prod; $tag is built but not started." >&2
    echo "deploy: rerun with ZUBSPACE_DEPLOY_FORCE=1 (make ... FORCE=1) to restart anyway." >&2
    exit 3
  fi
fi

echo "deploy: starting zubspace-$env at $tag"
git archive --format=tar "$commit:deploy" compose.yml | remote "mkdir -p ~/$dir/logs && tar -xf - -C ~/$dir"
git archive --format=tar "$commit:deploy/$env" | remote "tar -xf - -C ~/$dir"
remote "cd ~/$dir \
  && printf 'ZUBSPACE_TAG=%s\nZUBSPACE_COMMIT=%s\n' $tag $commit > release.env \
  && podman unshare chown 10001:10001 logs \
  && podman unshare chmod 755 logs \
  && PODMAN_COMPOSE_WARNING_LOGS=false podman compose --project-name zubspace-$env --env-file compose.env --env-file release.env \
       up -d --remove-orphans"

# nginx must serve the commit's client and the game server must report the same
# commit in its status `version` (builds older than that field report none).
live() {
  remote "curl -fsS --max-time 3 http://127.0.0.1:$client_port/BUILD_INFO.json | jq -er .git \
    && $server_status | jq -r '.version // \"none\"'" 2>/dev/null \
    | paste -sd' ' -
}
echo "deploy: waiting for $tag to serve"
for _ in $(seq 60); do
  case "$(live || true)" in "$commit $commit" | "$commit none") break ;; esac
  sleep 2
done
case "$(live || true)" in
  "$commit $commit" | "$commit none") ;;
  *)
    echo "deploy: zubspace-$env is not serving $commit (client server: $(live || true)); see: ssh $host 'cd ~/$dir && podman compose -p zubspace-$env ps'" >&2
    exit 1
    ;;
esac
entry="$(date -u +%FT%TZ) $commit $mode $(id -un)@$(hostname -s)"
remote "echo '$entry' >> ~/$dir/deploys.log \
  && tail -n 1000 ~/$dir/deploys.log > ~/$dir/deploys.log.tmp && mv ~/$dir/deploys.log.tmp ~/$dir/deploys.log"

# Keep the newest three builds, and any image a container still uses.
remote "cd ~/zubspace/src && ls -1t | tail -n +4 | xargs -r rm -rf
  for image in server client; do
    podman images --format '{{.CreatedAt}} {{.Tag}}' localhost/zubspace/\$image | sort -r \
      | awk 'NR > 3 { print \$NF }' \
      | xargs -r -I{} podman rmi localhost/zubspace/\$image:{} >/dev/null 2>&1 || true
  done
  podman image prune -f >/dev/null"

echo "deploy: zubspace-$env serves $commit on $host"
curl -fsS --max-time 10 "$url/BUILD_INFO.json" | jq -c '{git, version}' || echo "deploy: $url is not reachable through Cloudflare yet" >&2
