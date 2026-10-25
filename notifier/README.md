# zubspace-notifier

A small long-running service that polls a Zubspace game server's public
player list (`/v1/players`) and sends a notification when a human player joins
or leaves a zone:

```text
Zub has joined Chaos Zone
Zub has left Chaos Zone after 12m 34s
```

Bots are never announced. Players already online when the notifier starts are
the baseline and are not announced, so a restart never re-sends everyone; their
leave has no time, because their join was not seen. A stay is measured between
polls, so it is accurate to one poll interval, and moving to another zone
starts a new one.
Changes seen in the same poll go out as one notification, one line each. If the
player list cannot be read (for example while a deploy restarts the server),
the last roster is kept and nothing is sent until the list is readable again.
Zone names come from `/v1/zones` on the same server.

## Configuration

The real config is `etc/notifier.json`. It holds the Slack webhook, so it is
git-ignored and docker-ignored and never committed; copy
[`etc/notifier.sample.json`](etc/notifier.sample.json), which mirrors ours
without secrets, and fill it in:

```sh
cp etc/notifier.sample.json etc/notifier.json && chmod 600 etc/notifier.json
```

| Key | Default | Meaning |
|---|---|---|
| `endpoint` | `https://server.zubspace.gg/v1/players` | Player list to poll. |
| `poll_seconds` | `5` | Time between polls. Values under 1 are clamped to 1. |
| `ignore_names` | `[]` | Regular expressions; matching names are never announced. Ours skips the deploy probe's `probe<N>` joins. |
| `provider` | (required) | `"ntfy"` or `"slack"`. Only the selected provider's section is read. |
| `ntfy.server` | `https://ntfy.sh` | ntfy server, including any path for a self-hosted one. |
| `ntfy.topic` | (required for ntfy) | Topic to publish to. Anyone who knows a topic on ntfy.sh can read it, so use an unguessable name. |
| `slack.webhook_url` | (required for slack) | Slack incoming webhook URL. |

The secret can also come from the environment: `NOTIFIER_NTFY_TOPIC` and
`NOTIFIER_SLACK_WEBHOOK_URL` override the config. Unknown keys are rejected.

## Run

Node 26 runs the TypeScript sources directly; pnpm is only needed for the type
check and tests.

```sh
cd notifier
pnpm install
pnpm check && pnpm test
pnpm start                                   # node src/main.ts --config etc/notifier.json
node src/main.ts --config /path/to/other.json
```

In Docker (the image build runs the type check and tests). The image holds no
config, so mount the real one:

```sh
docker build -t zubspace-notifier notifier/
docker run -d --restart unless-stopped --name zubspace-notifier \
  -v "$PWD/notifier/etc/notifier.json:/app/etc/notifier.json:ro" zubspace-notifier
```

On the production host it runs as the `zubspace-notifier` Compose project;
see [`deploy/README.md`](../deploy/README.md#notifier) and `make deploy-notifier`.

The service logs one line per event to stdout and stops cleanly on SIGINT or
SIGTERM.
