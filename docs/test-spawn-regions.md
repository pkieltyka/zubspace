# Explicit spawn regions for focused browser tests

Shipped zones choose spawns from their `[Spawn]` section: Chaos leaves it
empty, so the server uses its central random area, and Extreme authors a
small region at the map centre (see [Gameplay](gameplay.md#spawning)). A
focused browser scenario can instead run against a copy of a shipped zone
whose spawn region is pinned to a declared tile. The browser still joins
normally; the server chooses and admits the spawn through its ordinary
lifecycle.

```sh
scripts/e2e-hyprland-tools/run -- node tools/e2e_spawn_region.mjs \
  --zone chaos-zone --x 512 --y 512 --radius 2 \
  -- make test-e2e-input-recovery
```

Build the server and WASM client first (`make dev`); the wrapper does not.

## Options

| Option | Meaning |
| --- | --- |
| `--zone` | `chaos-zone` or `extreme`. Must match the single zone the scenario uses. |
| `--x`, `--y` | Region centre in map tiles (1–1023), not pixels. The tile centre is `(x * 16 + 8, y * 16 + 8)`. |
| `--radius` | Radius in tiles (0–511), and the whole square must stay inside the map. `0` always spawns on the centre tile. |
| `--out` | Evidence directory. Defaults to `.debug/spawn-region-<timestamp>-<pid>/`; it must not exist yet. |

Everything after `--` is the command to run.

## What the wrapper does

1. Copies `assets/zones/<zone>/` to a temporary directory.
2. Sets all twelve `Team0..3-X/Y/Radius` fields of `[Spawn]` to the declared
   region (values of 512 or more are written in Continuum's negative form) and
   updates `settings_sha256` in `zone.bundle`. The map, LVZ files and every
   balance setting are unchanged.
3. Refuses the region unless the centre is clear of solid tiles for a radius of
   `radius * 16 + 20` pixels.
4. Runs the command with `ZUBSPACE_SERVER_ZONE_ROOT` pointing at the copy,
   `ZUBSPACE_SERVER_ZONE` and `ZUBSPACE_E2E_ARENA` set to the zone, and
   `ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES=1`. It refuses to start if
   `ZUBSPACE_SERVER_ZONE_ROOT` is already set.
5. Writes `fixture.json` (command, source `HEAD`, tracked-file status,
   settings and map hashes) and `zone-root/` to the evidence directory, then
   `result.json` with the exit status. Results are marked `diagnosticOnly`.

With a configured region the server picks the region by
`frequency % configured regions`; the wrapper sets all four alike, so every
player uses it. For a nonzero radius it tries up to 100 random tiles within
`radius` of the centre that fit the ship's hull, skipping wormholes and
hostile bricks, then falls back to the centre tile if that fits; otherwise
the spawn is deferred (`server_random_spawn_coordinates_from_settings` in
`src/bin/zubspace_server/zone.rs`).

## Limits

- A pinned region says nothing about the default random-spawn policy.
- Do not wrap multi-zone suites or scenarios that build their own temporary
  zone; those declare their own geometry.
- `tests/e2e/spawn_region_runner.test.mjs` (part of `make test-quick`) covers
  argument parsing, the child environment, exit-status retention and refusal
  of existing output or an existing zone fixture. The field rewrite and
  clearance check live in `tests/e2e/fixture_spawn_region.mjs`.
