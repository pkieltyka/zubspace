# Chaos Zone settings provenance

`assets/zones/chaos-zone/arena.conf` is the active Chaos Zone configuration.
The server parses it with the generic arena-settings loader
(`src/bin/zubspace_server/arena_settings/profile.rs`) into the 1,428-byte
Continuum arena settings packet that it uses itself and sends to clients. A
test-only packet builder (`arena_settings/chaos.rs`) must stay byte-identical
to that parse. This note records where the values come from; it is not a
second configuration.

## Sources

- **Per-ship values**: the Chaos Zone settings page,
  <http://chaos.svssubspace.com/?page=zonesettings>.
  It lists per-ship stats only, not a full arena configuration.
- **Everything else** (initial values, upgrade steps, weapon costs, speeds,
  damage and lifetimes, bomb and mine delays, prize timing and weights, timed
  prize durations, consumable limits, flag and soccer fields): the SVS
  reference settings distributed with ASSS (`dist/conf/svs`) and Subspace
  Infinity (`svsSettings.cfg`). Chaos is described publicly as an SVS-style
  zone, but no exact live Chaos arena config or settings packet was available.
- **Map**: `bdhadar.lvl`, plus the LVZ archives listed in
  `assets/zones/chaos-zone/zone.bundle`.

Doors, security/checksum fields and any live Chaos-only overrides beyond these
sources are hand-authored.

## Ship stats from the Chaos page

| Ship | Trait | Rotation | Thrust | Speed | Recharge | Energy | Fire delay | Multifire delay |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Warbird | Agile | 290 | 19 | 3250 | 1150 | 1700 | 24 | 50 |
| Javelin | Speed | 230 | 17 | 3750 | 1150 | 1700 | 24 | 50 |
| Spider | Cloak | 230 | 18 | 3250 | 1150 | 1700 | 24 | 50 |
| Leviathan | Level 3 bombs | 230 | 18 | 3250 | 1150 | 1700 | 24 | 50 |
| Terrier | Double guns | 230 | 18 | 3250 | 1150 | 1700 | 25 | 50 |
| Weasel | EMP bombs | 230 | 18 | 3250 | 1150 | 1700 | 24 | 50 |
| Lancaster | Bouncing bombs | 230 | 18 | 3250 | 1150 | 1700 | 24 | 50 |
| Shark | Fast firing | 260 | 18 | 3250 | 1150 | 1700 | 16 | 40 |

These are the `Maximum*` values in `arena.conf`. Status energy costs are the
same for every ship: X-Radar 250, Stealth 333, Antiwarp 100; Cloak 100 on the
ships that have it. Maximum bomb level is 2 (Leviathan 3) and maximum
shrapnel is 8 (Weasel 0). Weasel sees mines on radar and Shark sees bombs.

The client converts these Continuum units the way the reference game does
(`src/settings/config.rs`): rotation `/ 400` turns per second, thrust `* 10`
pixels per second squared, speed `/ 10` pixels per second.

## Zubspace changes to the reference

These are deliberate and commented in `arena.conf`:

- `InitialBounty=30` for every ship (lower than the reference).
- Shark has cloak from spawn (`CloakStatus=2`) with Spider's
  `CloakEnergy=100`.
- `WarpRadiusLimit=176` gives one central spawn and random-warp area for every
  ship. In free-for-all play the server ignores the radar-mode corners that
  `RadarMode=4` would select; `RadarMode` stays 4 in the packet for radar
  presentation.
- `BrickSpan=10` is the ASSS default, used because the reference profile omits
  it.

One more choice is server policy rather than an `arena.conf` value: with
`maxed_spawn_loadout` enabled (the default), every life starts with maximum
energy, recharge, thrust, speed and rotation, maximum gun and bomb levels,
shrapnel, bouncing bullets, multifire and proximity capability, full
consumables, and every status capability the ship allows, with the status
toggles off. Bounty still comes from `InitialBounty`. Setting `maxed_spawn_loadout = false` restores the
reference behaviour of starting values plus `InitialBounty` prize rolls.
