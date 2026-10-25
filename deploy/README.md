# Production host runbook

One HostHatch VPS (`zubspace-prod`, 85.155.184.54, New York; Ubuntu 26.04,
3 vCPU Ryzen 9 9950X, 12 GB) runs two independent environments. Each is a
rootless Podman Compose project owned by `peter`, defined by
[`compose.yml`](compose.yml) plus that environment's directory here.

| | prod | dev |
| --- | --- | --- |
| Site | <https://zubspace.gg> (`www.` redirects here) | <https://dev.zubspace.gg> |
| Cloudflare tunnel | `zubspace-prod` | `zubspace-dev` |
| nginx (client image) | 127.0.0.1:7000 | 127.0.0.1:7100 |
| Game server HTTP | 127.0.0.1:9001 | 127.0.0.1:9301 |
| Game server metrics | 127.0.0.1:9100 | 127.0.0.1:9300 |
| WebRTC UDP (public) | 9402-9901 | 9902-10401 |
| Prometheus | 127.0.0.1:9090 | 127.0.0.1:9091 |
| Players / bots | 500 / 15 (ships 1,4,5,7) | 500 / 15 (ships 1,4,5,7) |
| Server logs | INFO, JSON, journal; per-shot, per-hit and per-acknowledgement lines off | DEBUG, JSON, `~/zubspace/dev/logs/server.log` |
| Client presentation traces | off | on (`logs/client.log`) |
| CPU | all 3 vCPU, full weight | at most 1 vCPU, quarter weight |
| Search engines | indexed | `X-Robots-Tag: noindex` on the site and API |

The game server uses one UDP port per browser session and refuses to start
unless its range is at least `max_players` ports wide
([deployment](../docs/deployment.md#udp-ports-and-capacity)).
The host firewall opens both ranges from [`host/setup.sh`](host/setup.sh);
changing a range means editing that script and running it again, since a
deploy does not touch the firewall. `max_players` is an admission cap, not a
measured performance limit.

Browsers reach each site through Cloudflare and that environment's tunnel;
nginx serves the static client and proxies `/v1/rtc/offer`, the public
`/v1/status`, `/v1/players`, `/v1/ping`, `/v1/zones` and scoped
`/v1/zones/{id}[/players|/status]` routes, and the diagnostic upload paths to
the game server. Gameplay then flows over UDP straight to the host's public IP,
which the server binds (`rtc_udp_bind_ip`) and places in its WebRTC answer, so
no DNS name is involved.

`server.zubspace.gg` and `dev-server.zubspace.gg` expose only the game server's
public read-only API (`/v1/status`, `/v1/players`, `/v1/ping`, `/v1/zones`
and `/v1/zones/{id}[/players|/status]`;
[`docs/public-server-api.md`](../docs/public-server-api.md)). They are proxied
names on the same tunnels: an explicit path rule sends these reads to that
environment's nginx, and anything else gets a 404. SSH uses the IP (the
`zubspace-prod` alias).

## Deploy

```sh
make deploy-dev                 # HEAD to dev, then a headless Chrome join check
make deploy-dev REF=<commit>    # any commit
make promote-prod               # the exact commit dev serves, to prod
make deploy-prod REF=<commit>   # any commit straight to prod
make rollback-prod              # prod's previous recorded deploy (also rollback-dev)
make probe-dev                  # the join check alone (also probe-prod)
```

`FORCE=1` lets prod restart with players connected; `PROBE=0` skips the join
check, which plays for 15 seconds as `probe<N>` and prints the build label,
join time and server counts.

[`scripts/deploy.sh`](../scripts/deploy.sh) ships `git archive` of the commit
(only committed files, so local work in progress never reaches the host),
builds `zubspace/server:<tag>` and `zubspace/client:<tag>` on the host with
Podman unless they exist, installs `compose.yml` and `deploy/<env>/` **from
that same commit**, restarts the project and waits until nginx serves the
commit's `BUILD_INFO.json` and the game server's `/v1/status` reports the same
`version`. A first build takes several minutes; redeploying
an already built commit (dev to prod, or a rollback within the last three
builds) only restarts containers. Prod refuses to restart while any browser
session is connected unless `ZUBSPACE_DEPLOY_FORCE=1` (`FORCE=1`). A commit
that fails to build leaves the environment on its previous build.

Each successful deploy appends `time commit mode user@host` to
`~/zubspace/<env>/deploys.log`. Rollback picks the newest entry that differs
from the commit being served, so a second rollback undoes the first.

Config changes therefore go through Git: edit `deploy/<env>/...`, commit, deploy.
The only host-only files are `~/zubspace/<env>/cloudflared.env` (the tunnel
token, mode 600), the generated `release.env` and `deploys.log`.

## Operate

```sh
make remote-status                       # per environment: commit, players, sessions, uptime, containers, last deploy; host disk, memory, load, journal
make remote-history                      # deploys.log of both environments
make remote-logs-prod                    # follow the server log (dev: logs/server.log)
make remote-logs-dev SERVICE=cloudflared FOLLOW=0 TAIL=50
make remote-restart-prod SERVICE=server  # without SERVICE: every service
make remote-shell                        # ssh into ~/zubspace
```

`remote-restart-prod` refuses, like a deploy, while browser sessions are
connected unless `FORCE=1`.

By hand on the host:

```sh
cd ~/zubspace/prod                      # or ~/zubspace/dev
podman compose -p zubspace-prod ps
podman logs -f zubspace-prod-server-1
tail -f ~/zubspace/dev/logs/server.log | jq -c 'select(.level != "DEBUG")'
```

Prometheus listens on loopback only. `make remote-prometheus` forwards it to
local ports that stay clear of the development Prometheus on 9090:
prod <http://127.0.0.1:19090>, dev <http://127.0.0.1:19091>.

Jobs: `zubspace` (game server), `cloudflared`, `node` (host CPU, memory, disk,
network; one exporter run by prod), `prometheus`. Useful game queries are in
[`docs/observability.md`](../docs/observability.md). The client's debug UI is a
URL switch on either site: `?debug=31337`.

## Notifier

The player notifier ([`notifier/`](../notifier/README.md)) posts "<name> has
joined Chaos Zone" and "<name> has left Chaos Zone after 12m 34s" to Slack for
human players. It polls `https://server.zubspace.gg/v1/players` every 2 seconds and
runs as its own Compose project, `zubspace-notifier` in `~/zubspace/notifier/`,
separate from prod and dev:

```sh
make deploy-notifier                     # HEAD (or REF=<commit>): build on the host, restart, wait for its first poll
make deploy-notifier NOTIFIER_CONFIG=notifier/etc/notifier.json   # also install that config on the host first
make remote-logs-notifier FOLLOW=0       # its log: start-up, baseline, every notification
```

Game deploys never restart it, and redeploying it is always safe: players
already online when it starts are not announced. Its real config, which
holds the Slack incoming-webhook URL, is never in Git or the image: locally it
is the git-ignored `notifier/etc/notifier.json` (shape in
`notifier/etc/notifier.sample.json`), and on the host
`~/zubspace/notifier/notifier.json` (mode 600, owned by the container's `node`
user), mounted read-only. `NOTIFIER_CONFIG=` validates the local file before
replacing the host's; to change the webhook or the poll interval, edit the
local file and deploy with it.

## Disk limits

The host must not fill up while nobody is watching:

- **Container logs** use the journald driver. [`host/journald.conf`](host/journald.conf)
  caps the whole journal at 20 GB, keeps 20 GB of disk free, and drops entries
  older than 60 days.
- **Dev's file logs** (`server.log`, `client.log`) rotate at 500 MB, ten
  compressed generations, checked hourly ([`host/logrotate.conf`](host/logrotate.conf)).
- **Prometheus** keeps 30 days or 5 GB (prod) and 14 days or 5 GB (dev).
- **Builds**: each deploy keeps the three newest image builds and source trees
  and prunes dangling layers.

## Host setup

[`host/setup.sh`](host/setup.sh) is the idempotent root setup: Podman, the log
limits, lingering for `peter` (containers with `restart: always` come back after
a reboot through `podman-restart.service`), key-only SSH and the firewall
(22/tcp plus both UDP ranges; no web ports, since the tunnels connect outbound).
It installs the config files next to it, so copy the directory over and run it
there:

```sh
scp -r deploy/host root@zubspace-prod:/root/zubspace-host
ssh root@zubspace-prod /root/zubspace-host/setup.sh
```

## Cloudflare

Zone `zubspace.gg` (Free plan). Its desired state is in Git:
[`cloudflare/zubspace.gg.json`](cloudflare/zubspace.gg.json), applied with
[`scripts/cloudflare.mjs`](../scripts/cloudflare.mjs):

```sh
node scripts/cloudflare.mjs plan     # differences only; exits 2 when there are some
node scripts/cloudflare.mjs apply    # make Cloudflare match the file
```

Edit the file, run `plan`, then `apply` and commit, the same way as host
config. The script manages these:

- **Zone settings:** SSL Full (strict), Always Use HTTPS, minimum TLS 1.2,
  Browser Cache TTL "Respect Existing Headers", caching level Standard, email
  obfuscation and Rocket Loader off.
- **Tunnel ingress:** each tunnel sends its site hostname to nginx, and on its
  server hostname only `/v1/status`, `/v1/players`, `/v1/ping`, `/v1/zones`
  and `/v1/zones/{id}[/players|/status]`; anything else gets a 404. Ports come
  from `deploy/<env>/compose.env`.
- **DNS:** `zubspace.gg` and `server` go to the `zubspace-prod` tunnel, `dev`
  and `dev-server` to `zubspace-dev`, and `www` to the apex. All are proxied.
  Two DNS-only records verify the domain for Google Search Console (a TXT on
  the apex) and Bing Webmaster Tools (a CNAME); both services recheck them, so
  keep them.
- **Rulesets:** the 301 redirect from `www.zubspace.gg`, a rate limit of 60
  `/v1/rtc/offer` requests per 10 seconds per IP, and a cache rule that never
  caches `/v1/*`. The game server also sends `Cache-Control: no-store` on every
  response, so the API is never cached by Cloudflare or browsers.

The script reports DNS records that are not in the file and never deletes
them. Tunnel tokens are not in the file: each lives only in
`~/zubspace/<env>/cloudflared.env` on the host. Creating a new tunnel is a
dashboard step, and the script then manages its ingress.

Never put a challenge (Under Attack, Bot Fight Mode) in front of `/v1/rtc/offer`,
`/v1/status`, `/v1/players` or `/v1/zones`: a `fetch` cannot solve it, so joins
would fail. The API
token for automation is `.secrets/cloudflare-token` in the repo checkout
(git- and docker-ignored).
