# Gameplay

Zubspace plays like SubSpace / Subspace Continuum: top-down dogfighting in
large tile maps, eight ships, guns and bombs, special items and randomly
spawning prizes ("greens"). Play is free-for-all: each player and bot has
its own frequency, so everyone is hostile to everyone.

The game server owns all game state, including movement admission, weapons,
damage, prizes, scores, spawning and bots. The browser predicts the local
ship and draws what the server reports. [Server authority](authoritative_server.md)
describes how that works; this page describes the game itself.

Behaviour follows the reference game client wherever Zubspace has not chosen
otherwise, converted to this codebase's units.

## Zones

| Zone id | Name | Map | Menu |
| --- | --- | --- | --- |
| `chaos-zone` | Chaos Zone | `bdhadar.lvl` + 14 LVZ archives | Listed; the default |
| `extreme` | Extreme | `doomsy8.lvl` + 8 LVZ archives | Unlisted; open with `?zone=extreme` or `?arena=extreme` |

Each zone lives in `assets/zones/<id>/`: a `zone.bundle` manifest (id, display
name, map, settings, news and LVZ files with SHA-256 pins), `arena.conf`, the
map, its LVZ archives and `news.txt`.

- Both zones are embedded in the server binary. A server started with
  `zone_bundle_root` also loads bundle directories from disk; a bundle replaces
  the embedded zone of the same id only with `allow_zone_overrides = true` and
  `override=<id>` in its manifest. Extra directories add zones (at most 64).
  The shipped configurations load `assets/zones` this way.
- A server process runs one zone at a time. A join for a different zone
  switches the zone only while no human is in the game; otherwise it is
  refused with `ZUBSPACE_ARENA_BUSY`.
- The browser downloads and caches the zone files; see
  [Zone file cache](zone-file-cache.md).
- Chaos settings provenance and Zubspace's changes are in
  [Chaos Zone settings](chaos_zone_settings.md). Extreme's `arena.conf` names
  its public source in its header.
- `assets/zones/zone.dat` is the original Continuum zone directory listing,
  kept for reference. Nothing loads it at runtime.

## Ships

| Key (in the Esc menu) | Ship |
| --- | --- |
| 1 | Warbird |
| 2 | Javelin |
| 3 | Spider |
| 4 | Leviathan |
| 5 | Terrier |
| 6 | Weasel |
| 7 | Lancaster |
| 8 | Shark |

Ship stats, weapon levels, costs and timers come from the zone's
`arena.conf`, which the server turns into the Continuum settings packet that
both sides use. Changing ship or entering spectator mode needs full energy.

**Starting loadout.** With `maxed_spawn_loadout = true` (the default and the
shipped setting), every life starts with maximum energy, recharge, thrust,
speed and rotation, maximum gun and bomb levels, shrapnel, bouncing bullets,
multifire and proximity capability, full item counts, and every status
ability the ship allows (toggled off). Bounty starts at the ship's
`InitialBounty`: 30 in Chaos, 3 in Extreme. With the setting off, players
start from their ship's `Initial*` values plus `InitialBounty` random prizes,
as in Continuum.

**Free-for-all.** Players and bots get distinct frequencies. Requests to
change frequency, including the `=N` chat command, are refused.

## Controls

Two layouts are live at the same time: arrow keys with Ctrl/X, Z/Tab and Q,
or WASD with `,` `.` `/`. Mapping is in `src/input.rs`
(`semantic_action_for_press`, `number_gameplay_action`) and
`src/gameplay/player_input.rs`.

### Flying

| Action | Keys |
| --- | --- |
| Thrust / reverse | Up or W / Down or S (held) |
| Turn | Left or A / Right or D (held) |
| Afterburner | Hold Shift while thrusting or reversing |
| Guns | Ctrl, X or `,`; hold to keep firing. X and `,` also fire while Shift is held |
| Bomb | Tab, Z or `.` |
| Mine | Shift+Tab, Shift+Z, Shift+`.`, or M |
| Repel | Q, `` ` ``, `/`, or Shift+Ctrl |
| Burst | 1 or Shift+Delete |
| Rocket | 2 or F3 |
| Decoy | 3 or F5 |
| Brick | 4 or F4 (not in a safe zone) |
| Thor | 5 or F6 |
| Stealth | 6 or Home |
| Cloak | 7 or Shift+Home |
| X-Radar | 8 or End |
| Antiwarp | 9 or Shift+End |
| Multifire on/off | `-` or Delete |
| Place portal, or warp to it | 0 |
| Place portal | Shift+Insert |
| Warp | Insert: to your portal if one is placed, otherwise home (needs full energy) |
| Random warp | Shift+0 |
| Attach / detach | F7 |
| Player list | Hold Space; Up/Down scroll. Flying and firing pause while it is open |
| Full map | Hold Alt |
| Chat | Enter |
| Menu | Esc |

Each key press is recorded with the Shift and Ctrl state at the moment it went
down, so chords such as Shift+Delete work however quickly they are released.
Key repeat does not count as a new press.

### Esc menu

| Key | Action |
| --- | --- |
| Esc | Resume |
| 1–8 | Change ship |
| S | Spectate |
| Q | Leave the zone |
| K | Controls dialog |
| F | Fullscreen |
| Enter | Close the menu and open chat |

### Spectating

| Action | Keys |
| --- | --- |
| Pan the camera | Arrows or WASD; hold Shift to pan faster. Panning stops following |
| Follow next player | Ctrl, X or `,` |
| Follow previous player | Tab, Z or `.` |
| X-Radar | 8 or End |
| Play again | Esc, then 1–8 |

The first follow press picks the nearest player; later presses step through
players in id order.

### Chat

Chat lines are at most 96 characters. Up/Down recall the last 16 lines you
sent; Tab completes a name after `/p`.

| Input | Effect |
| --- | --- |
| `text` | Public message |
| `'text`, `//text`, `/t text`, `/team text` | Team message |
| `/p name text` (also `/pm`, `/msg`, `/private`) | Private message, by name, name prefix or player id |
| `/s N text`, `/sound N text` | Message with chat sound N (0–255) |
| `?ping`, `?lag` | Show local RTC connection diagnostics |
| `?status` | Show recharge, thruster, speed and rotation upgrade percentages and shrapnel |
| `?sound N`, `?chat N`, `?voice N`, `?music N` | Volume 0–10 (`?sound=N` also works) |

The server only relays chat; it has no `?` or `*` commands.

### Connection diagnostics

The top-right HUD shows the selected WebRTC route's round-trip time beneath
energy, in milliseconds, sampled once a second. Missing, disconnected or stale
measurements display `-- ms`; local offline play has no ping readout. The
readout turns yellow from 150 ms and red from 300 ms. This is
transport RTT, not a measurement of server processing or presentation delay.
`?ping` reports current, average, low and high RTT over up to 60 recent samples.
`?lag` adds mean successive RTT variation. Packet loss is explicitly unavailable:
the server's probe-loss readings are not sent to the client by this protocol.
Both commands reply locally in chat without broadcasting the request.

### Developer keys

`debug=31337` in the page URL adds a text HUD and energy labels on other
ships, and makes F9 toggle the developer overlay, which includes the network
status panel and shading of hidden solid tiles (see
[Chaos hidden solid tiles](#chaos-hidden-solid-tiles)). Without it, F9 does
nothing.

## Arena score leader

The currently playing pilot with the highest total score (kill points plus flag
points) has the original gold rotating Subspace shield to the left of their ship
name. Server bots participate too, and the leader sees it on their own ship.
Spectators are excluded; tied scores go to the lowest player ID. Death and
respawn retain eligibility, while leaving or switching to spectator promotes
the next eligible player. Selection uses the full authoritative arena roster,
including players outside the camera's area of interest. The badge follows
normal name visibility, so it does not reveal a hidden cloaked opponent.
The ten 16×16 frames in `ssshield.bm2` repeat once per second.

## Weapons and items

| Weapon | Notes |
| --- | --- |
| Guns | Up to `MaxGuns` levels. Terrier fires two barrels. Multifire adds angled side shots. Bouncing bullets rebound off walls |
| Bombs | Up to `MaxBombs` levels. Proximity bombs also detonate near a target. Exploding bombs release `Shrapnel` fragments. Weasel fires EMP bombs, which pause the victim's recharge, and Lancaster bombs bounce off walls (`BombBounceCount`) |
| Mines | Bombs laid stationary, up to `MaxMines` per ship |
| Thor | A bomb that flies through walls |
| Burst | Bullets fanned in a full circle that arm when they first hit a wall |
| Decoy | A copy of your ship that drifts with your velocity and mirrors your turns |
| Repel | Pushes nearby enemy ships and weapons away |
| Rocket | A timed speed boost (`RocketThrust`, `RocketSpeed`, `RocketTime`) |
| Brick | A temporary wall (`BrickTime`) that its owner can fly through |
| Portal / Warp | Place a portal, then warp to it; without a portal, Warp returns home |

The server checks inventory, energy and cooldowns for every shot, simulates
every projectile, and applies damage, shrapnel and expiry. Lifetimes come from
the zone settings (`BulletAliveTime`, `BombAliveTime`, `MineAliveTime`,
`DecoyAliveTime` and so on), not from constants. An owner's death does not
remove their weapons; entering a safe zone, changing ship or spectating
removes that player's projectiles, mines and decoys still in play.

**Status abilities.** Stealth hides you from enemy radar; Cloak hides your ship
from enemy view; X-Radar shows cloaked and stealthed enemies; Antiwarp stops
enemies near you from warping or placing portals. Each drains energy per
second while on and switches off when energy cannot cover the cost.

**Safe zones** (tile 171) block firing and brick placement.

## Prizes

The server spawns greens while at least one human is in the game. Every
`PrizeDelay` it places up to `PrizeHideCount` new prizes on empty tiles in a
central area that grows with the player count, chosen by the `[PrizeWeight]`
table, with a `PrizeNegativeFactor` chance of a negative prize. Each lasts a
random time between `PrizeMinExist` and `PrizeMaxExist`, and the number on the
map is capped by `PrizeFactor` and the player count. A ship that dies while
moving may drop a green that lasts `DeathPrizeTime`.

Prize types: Recharge, Energy, Rotation, Stealth, Cloak, X-Radar, Warp, Guns,
Bombs, Bouncing Bullets, Thruster, Top Speed, Full Charge, Engine Shutdown,
Multifire, Proximity, Super, Shields, Shrapnel, Antiwarp, Repel, Burst, Decoy,
Thor, Multiprize, Brick, Rocket and Portal. Super lasts a random time up to
`SuperTime`; Shields last `ShieldsTime`.

## Spawning

The server chooses every spawn point:

- If the zone's `[Spawn]` section configures regions, a player spawns in region
  `frequency % configured regions`, at a random tile within its radius.
  Extreme configures one region at the map centre with a radius of 2 tiles.
- Otherwise (Chaos), players spawn at random in a central square whose size
  grows with the population, capped by `WarpRadiusLimit` (176 tiles in Chaos,
  which the formula always reaches). The radar-mode corners Continuum uses for
  team play are ignored in free-for-all.
- A candidate tile must fit the ship's hull clear of walls, wormholes and
  hostile bricks. After 100 failed tries the server tries the fallback point;
  if that is blocked, the spawn waits for the next attempt.
- Random warps (Shift+0) use the central square in every zone, ignoring
  `[Spawn]` regions.

After a death the server respawns the player after `EnterDelay` (hundredths
of a second) plus 1.25 s, but never sooner than 2 s; with `EnterDelay=0` the
wait is 2 s. That gives 2 s in Chaos and 3.25 s in Extreme
(`src/respawn.rs`).

Focused browser tests can pin the spawn region; see
[Test spawn regions](test-spawn-regions.md).

## Idle players move to spectator

A fresh life that is never flown is moved to spectator mode so idle players do
not sit in the arena as free kills. This is not a general AFK timeout
(`src/bin/zubspace_server/spawn_idle.rs`).

- The clock starts with the life's first ordinary input. Movement keys, a
  change of heading or an accepted shot mark the life as active, and an active
  life is never moved, however long it later sits still. Drifting, status
  toggles, holding Shift alone and routine updates do not count.
- After more than 120 seconds without activity the server makes the player a
  spectator and sends a private notice (packet `0x8d`). The client shows
  "Moved to spectator: no movement or firing after spawning." The player can
  pick a ship again at once.
- Inactive time carries across respawns, so being killed repeatedly does not
  reset it. It restarts after an active life, a new zone or connection, or a
  ship chosen from the menu. Time spent loading does not count.
- The server logs `spawn_idle_spectator`. Bots are exempt.

## Bots

Bots are server-side players with ordinary roster entries, lives, energy,
bounty and scores. They fight, die, respawn and collect prizes, and their
weapons go through the same projectile and damage code as players'.

**Configuration** (server config file, environment or command line):

| Setting | Environment | Default | Shipped prod/dev |
| --- | --- | --- | --- |
| `bot_count` | `ZUBSPACE_SERVER_BOT_COUNT` | 0 | 15 |
| `bot_ship_types` | `ZUBSPACE_SERVER_BOT_SHIP_TYPES` | `"1"` | `"1,4,5,7"` (Warbird, Leviathan, Terrier, Lancaster) |

Ships are assigned round-robin from `bot_ship_types`. Names come from a pool of
40, with `Bot N` as a fallback. About half the bots (chosen at random) have
X-Radar on.

A fifteen-bot roster has **3 Hard, 5 Medium and 7 Easy** pilots. Tiers are
shuffled independently of names, ship types and X-Radar, and persist through
respawns. Other roster sizes use `ceil(count / 5)` Hard and approximately
`count / 3` Medium (capped by the remaining slots), with the rest Easy. One bot
retains the existing Hard behavior.

| AI tier | Fire delay × ship setting | New-target reaction | Moving-target lead | Maximum aim drift |
| --- | --- | --- | --- | --- |
| Hard | 2× (previous behavior) | Immediate | Full intercept | None |
| Medium | 3× | 200 ms | 85% | ±3° |
| Easy | 4× | 400 ms | 65% | ±7° |

Aim drift changes smoothly over four seconds, staggered by pilot and life.
The bot steers toward its imperfect aim before firing; shots still leave its
actual facing and use ordinary shared weapon physics. All tiers have identical
ship stats, damage and energy rules for the same ship/loadout. Difficulty does
not weaken movement, collision handling or bomb self-preservation. These are
initial tuning values for playtesting, not a claim of equal win rates.

**Behaviour** (`src/bin/zubspace_server/server_tick/bots.rs`). Each tick a bot
picks one goal, in order:

1. Leave a safe zone it is in.
2. Head for a flag or ball, when objectives are enabled.
3. Chase its assigned human. Each live human is assigned at most one bot, so
   players are not mobbed. Bots approach a pilot in a safe zone but do not fire
   at them, and they cannot see a target that is both cloaked and stealthed
   unless they have X-Radar.
4. Pick up a nearby prize (within 640 px) when below 65% energy.
5. Fight another bot, if humans are present but it has none assigned.
6. Otherwise, hold near the map centre.

Bots fly with their ship's real turning, thrust and speed, collide with walls,
doors and bricks, and aim with the tier's lead and accuracy. They use their
tier's fire delay, never use multifire, and hold fire while no human is in the game.
After two consecutive gun volleys a bot fires a bomb instead when a two-second
forecast shows the explosion reaching the target without catching the bot,
and it keeps at least a quarter of its maximum energy after paying for it
(`server_tick/bots/tactics.rs`).

Bots respawn at a random point 96–512 px from the clear tile nearest the map
centre, never in a safe zone.

The native (non-browser) client build runs an offline sandbox with four local
bots. The browser client never simulates bots itself.

## Flags and soccer

The server has flag and powerball code, but no shipped zone uses it: neither
`arena.conf` has an `[Objective]` section and `enable_objectives` is `false`
in every shipped configuration. Flag and goal tiles in the maps and the
`[Flag]` and `[Soccer]` settings are inert. Browser tests that need flags use
the `flag-test` fixture zone in `tests/fixtures/zones/`.

## Audio

Sound effects are the original Continuum `.wa2` files (plain RIFF/WAVE) in
`assets/sound/`, compiled into the WASM client (`src/assets/load/sounds.rs`).
Your own sounds are not positional. Other players' and bots' sounds are: volume falls off linearly with distance from your ship (or the
spectator camera) and is silent beyond half the larger screen dimension. There
is no stereo panning (`src/spatial_audio.rs`).

| Sound files | Played for |
| --- | --- |
| `gun1`–`gun4` | Gun fire, by gun level |
| `bomb1`–`bomb4`, `ebomb1`–`ebomb4` | Bomb fire by level; EMP ships use `ebomb*`, also for mines |
| `mine1`–`mine4` | Mines, by level |
| `repel`, `decoy`, `burst`, `thor` | Those items |
| `rocket1`, `rocket2` | Rocket ignition, then a looping sustain until it ends |
| `thrust`, `rev` | Forward and reverse thrust, replayed on start, direction change and afterburner |
| `multion`, `multioff` | Multifire toggle |
| `stealth`, `cloak`, `xradar`, `antiwarp`, `off` | Status abilities on; `off` for any of them off |
| `prize` | Prize pickup |
| `explode0` | Bullet impact |
| `explode1` | Ship explosion |
| `explode2`, `ebombex` | Bomb or EMP bomb explosion |
| `bounce` | Bouncing projectiles and ships hitting walls at speed |
| `warppnt`, `warp` | Portal placement; warps |
| `wall` | Brick placement |
| `alarm` | Every 1.2 s while energy is below 25% |
| `bong1`–`bong26` | Chat sounds 1–26; LVZ `bong<N>` files override or extend them (N up to 255) |
| `victory`, `victoryl` | Chat sounds 102 (once) and 100/101 (start and stop a loop) |
| `goal` | Chat sound 104 |

`flag`, `ballbnc`, `catch` and `throw` play only in the offline sandbox.
`hum.wa2` and `arena-enter.wav` are not used, and entering a zone is silent.

## Chaos hidden solid tiles

Chaos's map uses solid tile ids that have no base-tile art, so they collide
like walls but are invisible. This is intended map geometry and matches
the reference game: base tiles are drawn only up to id 190, while 192–240 (except the
wormhole, 220) and 242–252 are solid; 241 is not solid, and bricks use 250
(`is_solid_tile` and `should_draw_base_tile` in `src/tile.rs`; the server
mirrors the solidity rule). A test,
`chaos_zone_contains_solid_tiles_without_base_art`, counts 486 cells of tile
242, 14,230 of 243 and 1,200 of 244 in `bdhadar.lvl`.

To tell this geometry from a collision bug, open the developer overlay
(`debug=31337`, then F9). It shades solid tiles that have no art in red.
