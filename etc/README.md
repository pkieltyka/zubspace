# Local observability configuration

- `server/playtest.toml`: local game configuration with structured JSON logs,
  private Prometheus metrics, and 15 bots using ships 1,4,5,8. Gameplay defaults
  to DEBUG, while `rtc*`/`webrtc*` modules stay at INFO. Edit `log_level` and
  `log_filter` to change the mix.
  It also enables compact client presentation windows; set
  `ZUBSPACE_SERVER_CLIENT_DEBUG_TRACE=false` for the baseline without extra presentation tracing.
- `prom/prometheus.yml`: Zubspace-only scrape target and metric retention.
- `prom/compose.yml`: Linux host-network collector, UI bound to loopback.
- `logs/server.example.jsonl`: **illustrative synthetic examples**, not captured
  gameplay or test evidence. DEBUG windows can have additional fields.

From the repository root:

```sh
make start-prom-server       # http://127.0.0.1:9090
make stop-prom-server        # preserves recorded metrics
```

Start a new recorded playtest (rebuilds and **restarts** the local game/client):

```sh
make restart-local-recording
# Or use the optimized browser bundle for performance comparisons:
make restart-local-release-recording
# Lower-overhead INFO baseline:
ZUBSPACE_SERVER_LOG_LEVEL=info ZUBSPACE_SERVER_CLIENT_DEBUG_TRACE=false make restart-local-recording
# Full transport DEBUG only when investigating transport:
ZUBSPACE_SERVER_LOG_FILTER='' make restart-local-recording
```

The commands use `server/playtest.toml`; override it with
`PLAYTEST_CONFIG=/absolute/path/to/your.toml`. Prometheus's target must match that
config's `metrics_listen`; the default is `127.0.0.1:9100`. Recording enforces
JSON in a session-specific file. Other server environment/CLI
precedence is unchanged. Existing `make restart-local` keeps its usual defaults.

Recordings live in `.debug/<UTC timestamp>-<unique suffix>/`, excluded from Git
and Docker contexts. Set `DEBUG_DIR=/path/to/recordings` to select another root.
`server.log` is NDJSON, `server-console.log` contains stderr/startup errors,
`client.log` contains uploaded client presentation windows, `static.log` contains
client HTTP logs, and `session.json` identifies the launch. Client batches arrive
about every 20 seconds after joining; these are opt-in render-intent diagnostics.
Local logs are kept until you remove them; Prometheus retention is separate.

See [observability documentation](../docs/observability.md) for retention,
session manifests, PromQL examples and measurement limits. No global host
exporter, Docker daemon metrics or remote service is configured here.
