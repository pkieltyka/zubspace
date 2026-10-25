# Zubspace docs

These docs describe Zubspace as it is implemented. Start with the architecture
overview; the rest go deeper into one area each.

| Doc | What it covers |
| --- | --- |
| [Architecture](architecture.md) | The pieces, how data flows between them, and where things live in the source |
| [Physics and presentation](physics.md) | Shared rules, units, prediction, interpolation, weapon motion and recovery |
| [Gameplay](gameplay.md) | Zones, ships, controls, weapons and prizes, spawning, bots, audio |
| [Server authority](authoritative_server.md) | What the server decides, what the client predicts, and how they reconcile |
| [Networking](networking.md) | Connection flow, DataChannels, input records, snapshots, events, bandwidth |
| [Vendored dependencies](vendoring.md) | Renderer/runtime pairing, WebRTC patches, upgrade workflow and stack tests |
| [Connection quality](connection-quality.md) | How the server measures each player's link and when it moves a player to spectator |
| [Protocol compatibility](protocol_compatibility.md) | Which formats follow Subspace Continuum and which are Zubspace's own |
| [Public server API](public-server-api.md) | The HTTP endpoints for status, players, zones and signaling |
| [Zone file cache](zone-file-cache.md) | How the browser fetches, verifies and reuses zone files |
| [Deployment](deployment.md) | The container images, configuration and self-hosting |
| [Observability](observability.md) | Server logs, Prometheus metrics and player experience reports |
| [E2E diagnostics](e2e-diagnostics.md) | Diagnostic profiles and evidence used by the browser test suites |
| [Test spawn regions](test-spawn-regions.md) | Fixed spawn regions for focused browser tests |

Provenance records that ship inside the release images:

- [Chaos zone settings](chaos_zone_settings.md): where the Chaos arena settings come from
- [Browser runtime provenance](browser-runtime-provenance.md): the pinned Macroquad fork and its JS bundle

Elsewhere in the repo: [our servers](../deploy/README.md),
[local recording configuration](../etc/README.md),
[the browser test harness](../tests/e2e/README.md),
[the join notifier](../notifier/README.md) and
[game assets](../ASSETS.md).
