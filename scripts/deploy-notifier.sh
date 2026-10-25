#!/usr/bin/env bash
# Build the player notifier from one commit on the production host and
# (re)start it there.
#
#   scripts/deploy-notifier.sh [git-ref]     # default ref: HEAD
#
# Ships `git archive` of the commit's notifier/ directory, builds
# zubspace/notifier:<tag> with rootless Podman (the image build runs the
# notifier's type check and tests) unless it exists, installs
# deploy/notifier/compose.yml from the same commit into ~/zubspace/notifier/
# and restarts the zubspace-notifier Compose project. The notifier does not
# touch prod or dev and announces nobody already online when it starts, so it
# can be redeployed at any time. Each deploy is appended to
# ~/zubspace/notifier/deploys.log; the newest three images are kept.
#
# The real config (with the Slack webhook) is never in Git or the image; the
# host keeps it in ~/zubspace/notifier/notifier.json. NOTIFIER_CONFIG=<file>
# (usually notifier/etc/notifier.json, git-ignored) validates that file here
# and installs it there first; without it the host keeps the config it has.
#
#   ZUBSPACE_DEPLOY_HOST   ssh destination (default peter@zubspace-prod)
#   NOTIFIER_CONFIG        local config to install on the host
set -euo pipefail

host=${ZUBSPACE_DEPLOY_HOST:-peter@zubspace-prod}
remote() { ssh "$host" "$@"; }
cd "$(git rev-parse --show-toplevel)"

commit=$(git rev-parse --verify "${1:-HEAD}^{commit}")
tag=${commit:0:12}
dir=zubspace/notifier
container=zubspace-notifier-notifier-1

echo "deploy-notifier: $(git log -1 --format='%h %s' "$commit")"
config=${NOTIFIER_CONFIG:-}
if [ -n "$config" ]; then
  # The host passes no environment, so the file alone must load, secret included.
  node --input-type=module -e '
    const { loadConfig } = await import(process.argv[1]);
    await loadConfig(process.argv[2], {}).catch((error) => {
      console.error(`deploy-notifier: ${error.message}`);
      process.exit(1);
    });' "$PWD/notifier/src/config.ts" "$config"
  echo "deploy-notifier: installing $config as ~/$dir/notifier.json on $host"
  # Mode 600, owned by the container's `node` user (uid 1000 inside).
  remote "mkdir -p ~/$dir && umask 077 && cat > ~/$dir/notifier.json.new \
    && podman unshare chown 1000:1000 ~/$dir/notifier.json.new \
    && mv -f ~/$dir/notifier.json.new ~/$dir/notifier.json" < "$config"
fi
remote "test -s ~/$dir/notifier.json" || {
  echo "deploy-notifier: ~/$dir/notifier.json is missing on $host;" \
    "install one with NOTIFIER_CONFIG=notifier/etc/notifier.json (make deploy-notifier NOTIFIER_CONFIG=...)" >&2
  exit 1
}

if remote "podman image exists localhost/zubspace/notifier:$tag"; then
  echo "deploy-notifier: image for $tag already on $host"
else
  echo "deploy-notifier: building zubspace/notifier:$tag on $host"
  git archive --format=tar "$commit:notifier" \
    | remote "rm -rf ~/$dir/build && mkdir -p ~/$dir/build && tar -xf - -C ~/$dir/build \
      && podman build -t zubspace/notifier:$tag ~/$dir/build; status=\$?; rm -rf ~/$dir/build; exit \$status"
fi

echo "deploy-notifier: starting zubspace-notifier at $tag"
started=$(remote "date -u +%FT%TZ")
git archive --format=tar "$commit:deploy/notifier" compose.yml | remote "tar -xf - -C ~/$dir"
remote "cd ~/$dir \
  && printf 'NOTIFIER_TAG=%s\nNOTIFIER_COMMIT=%s\n' $tag $commit > release.env \
  && PODMAN_COMPOSE_WARNING_LOGS=false podman compose --project-name zubspace-notifier --env-file release.env \
       up -d --remove-orphans --force-recreate"

# A started notifier has read its config and the player list once.
echo "deploy-notifier: waiting for the first poll"
for _ in $(seq 30); do
  remote "podman logs --since $started $container 2>&1 | grep -q 'notifier: baseline'" && break
  sleep 1
done
if ! remote "podman logs --since $started $container 2>&1 | grep -q 'notifier: baseline'"; then
  echo "deploy-notifier: the notifier has not polled; its log:" >&2
  remote "podman logs --since $started $container 2>&1 | tail -n 20" >&2
  exit 1
fi
remote "podman logs --since $started $container 2>&1 | grep '^notifier: '"

entry="$(date -u +%FT%TZ) $commit deploy $(id -un)@$(hostname -s)"
remote "echo '$entry' >> ~/$dir/deploys.log \
  && tail -n 1000 ~/$dir/deploys.log > ~/$dir/deploys.log.tmp && mv ~/$dir/deploys.log.tmp ~/$dir/deploys.log"

# Keep the newest three images, and any image a container still uses.
remote "podman images --format '{{.CreatedAt}} {{.Tag}}' localhost/zubspace/notifier | sort -r \
  | awk 'NR > 3 { print \$NF }' \
  | xargs -r -I{} podman rmi localhost/zubspace/notifier:{} >/dev/null 2>&1 || true"

echo "deploy-notifier: zubspace-notifier runs $commit on $host"
