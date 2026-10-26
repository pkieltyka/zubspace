# Server authority

Zubspace runs one authoritative Rust game server per zone
(`src/bin/zubspace_server/`). The browser client predicts the player's own ship
so controls feel immediate, and presents remote actors and weapons from server
snapshots and events. The server owns the game state.

This document covers who owns what, how the tick runs, and how each gameplay
system is validated. Related documents:

- [`architecture.md`](architecture.md): overall system shape.
- [`networking.md`](networking.md): channels, snapshots and events on the wire.
- [`physics.md`](physics.md): shared rules, clocks and the three presentation paths.
- [`protocol_compatibility.md`](protocol_compatibility.md): Continuum packets
  versus Zubspace extensions.
- [`gameplay.md`](gameplay.md): zones, spawn and idle policy, bot tactics.

Named constants can be found by searching the source for them.

## 1. Who owns what

| State or decision | Owner |
| --- | --- |
| Own ship's feel and drawn position | Client prediction, using shared movement code |
| Published ship position | Server: client reports are replayed and accepted only if exact |
| Energy, recharge, status drains | Server; client predicts and is corrected |
| Weapon acceptance: inventory, levels, cooldowns, energy, mines, safe zones | Server |
| Projectiles, collision, damage, death, score, bounty, assists | Server |
| Prize spawns, claims, effects, inventory | Server |
| Rocket, Repel, Warp, Portal, Brick, Decoy, Burst, Thor | Server |
| Spawn location, respawn timing | Server |
| Who sees whom (AOI), snapshot contents | Server |
| Remote interpolation, projectile visuals, HUD, audio | Client presentation |
| Multiplayer bots | Server |
| Flags and powerballs | Server; off by default (section 12) |

Movement is **validated relay**. The client sends its exact ship state with
the inputs that produced it. The server replays those inputs through the same
movement code from the last state it accepted, and accepts the report only if
the replay reproduces it exactly. Accepted positions are relayed to other
players.

Ordinary admitted movement stays on the client's prediction lineage. An honest
client can still need recovery after missing input history, a rejected weapon
prediction or an environmental transition. Its position is rebased at explicit
boundaries: spawning, ship changes, Warp or Portal relocation, a hostile Repel,
or a server movement correction.

Clients never report hits. The server switches Continuum damage reports off at
join and ignores them. Instead, each input carries a combat-view receipt saying
which remote view the player was looking at. The server uses it to judge hits
against what the shooter saw (section 7.2).

## 2. Server process and clock

### 2.1 Process shape

One process hosts one active zone; other catalog zones report `standby`. All
game state is one `ServerState` (`state.rs`) behind `Arc<Mutex<…>>`.

The main thread (`src/bin/zubspace_server.rs`) accepts HTTP signalling, reads
admin commands from stdin, and calls `server_tick::run_server_tick` at
`tick_rate_hz`. That rate defaults to 60 and can go up to 240 (`tick_rate`,
`ZUBSPACE_SERVER_TICK_RATE`, `--tick-rate`). A slow tick skips its missed
deadlines instead of running catch-up ticks.

WebRTC threads parse packets and mostly enqueue them for the tick. Positions go
to bounded per-player queues (`PositionInputQueues`). Gameplay requests go to
the reliable gameplay FIFO (section 3.3). Input records are merged into the
player's input history on arrival, because they are evidence for later
positions.

Humans use player ids below `0x300` and bots use ids from `0x300` up.
`max_players` (default 500) counts both. The design target is about 200 active
players per arena.

### 2.2 The gameplay clock

Gameplay time is the Continuum tick: 100 per second, 31 bits wide, from a
process-monotonic clock (`wire::current_tick`). Each server tick samples it
once and passes it to everything the tick does. Wall-clock `Instant`s appear
only at transport and operations edges such as pacing, resends and rate
limits.

Tick arithmetic goes through the wrap-safe helpers in `src/continuum.rs`
(`continuum_tick_diff` and friends). Signed comparison is unambiguous only
within 2^30 ticks (about 124 days), so long-lived state is kept bounded, and
optional deadlines are `Option<u32>` because tick 0 is a real value.

### 2.3 One server tick

`run_server_tick_at_tick` (`server_tick.rs`) runs these steps in order:

1. **Reliable resends.** Resend due reliable packets.
2. **Per-player pre-pass.** Connection quality, send pacing and doors. Then
   `advance_server_gameplay_energy` applies afterburner drain, recharge and
   status drains exactly once, from the last accepted state.
3. **Positions.** Admit queued positions (section 4), including their weapons,
   lifecycle baselines and correction applications. Then apply the spawn-idle
   policy.
4. **Bots and relays.** Step the bots (section 11) and relay accepted
   positions.
5. **Requests.** Run the reliable gameplay FIFO, objectives, brick-overlap
   corrections, and queued Portal placements and Warps.
6. **World timers.** Expire bricks and step hidden prizes.
7. **Projectiles.**
   - Cancel the weapons of owners who are now in safe zones.
   - Step Decoys and projectiles.
   - Freeze new collisions.
   - Commit prepared damage in order, stopping at the first capacity deferral.
8. **Snapshots.** Build and send them (section 5).
9. **Diagnostics.** Every 600 server ticks, log security and pacing
   summaries. Record the tick's wall time for load shedding.

The main loop then broadcasts brick and hidden-prize packets reliably and
resends again.

## 3. Identity, lives and lifecycle

### 3.1 Identities

`src/authority_identity.rs` defines the identities that keep old work away
from new state:

- **`PlayerSessionIdentity`** pairs `player_id` with a `u64` session id the
  server never reuses. A per-connection ingress gate checks it, so an old
  connection's packets or cleanup cannot touch a new player with the same
  numeric id.
- **`LifeIdentity`** pairs `player_id` with `life_seq`, a `u32` that advances
  on death, ship change and frequency change, skipping zero.
- **`PlayerRuntimeIdentity`** combines both.

Queued positions, collision victims, damage hits and outbound damage events
record their identity and recheck it when applied.

Projectiles belong to the weapon, not the owner's life. A dead player's shots
keep flying and still credit the shooter. A ship change, frequency change or
leave removes the owner's projectiles and Decoys, announced with an
`OwnerWeaponsCancelled` event.

### 3.2 Lifecycle barriers

Whenever the client's own state has to be replaced, the same pattern applies:

1. The client pauses realtime input.
2. It sends a baseline or request reliably.
3. It resumes only on the server's exact **application receipt**.

A transport ACK never releases gameplay. Receipts name the session, player,
life and sequence they answer, so stale or duplicate receipts do nothing.

| Transition | Client sends | Server answers |
| --- | --- | --- |
| Join, respawn, ship change | Neutral baseline position | `0x75` baseline accepted, carrying the server-chosen spawn |
| Positive Warp prize | Position with a `0x76` marker | `0x77` Warp accepted |
| Movement correction (server proposes `0x7b`) | `0x63` apply or cancel | `0x7c` applied or cancelled |
| Hostile Repel | Position with a `0x79` marker | `0x7a` Repel applied |
| Movement-changing prize | `0x64` effect applied | `0x76` applied, rebased or cancelled |
| Portal placement | `0x65` request | `0x66` terminal |
| Manual Warp (Insert) | `0x6a` request | `0x6b` terminal |

The client acts on receipts as soon as they arrive, even ahead of a gap in the
reliable stream, because each can only retire its own transition.

### 3.3 Reliable gameplay FIFO

Reliable requests that change gameplay are queued per player and applied in
the tick, in arrival order. These are ship and frequency changes, leave, prize
claims, bricks, turret attach/detach, objective actions and correction
applications (`PendingReliableGameplayMutationKind`).

The queue holds 64 entries per player and 49,152 in total. Each tick does at
most 256 items, rotating fairly between players. A packet is ACKed only once
its request is finished or queued, so a full queue makes the client retry.
Positions are admitted first, so requests see the latest accepted position.

## 4. Input and movement

### 4.1 Client stepping

The client turns render time into fixed 1/60 s logical steps
(`FixedGameplayClock`, `src/fixed_step.rs`). It runs at most 8 catch-up steps
per frame (`MAX_GAMEPLAY_CATCH_UP_STEPS`) and discards any time beyond that.
Render schedules within this budget use the same logical movement steps;
a long stall does not simulate all the missed wall time on return.

Held keys apply to every step in the frame. One-shot actions such as weapons
run on the frame's last step, whose position is the one sent.
`Game::local_render_prediction` advances a throwaway copy of the ship by the
leftover fraction of a step, for drawing only.

### 4.2 Input records

Each logical step produces one input record. Records travel in `0x62` packets:

```text
type:u8 = 0x62
base_seq:u16      sequence of the first record
count:u8          1..=32
[bits:u16, direction:u8, door_seed:u8, combat_view_receipt:14 bytes] × count
```

`direction` is the heading in Continuum's 40 bins (0..39). `door_seed` is the
door state used for that step (section 10.2).

Input bits are defined in `src/gameplay/player_input.rs`; the server rejects
any other bit:

| Bit | Meaning |
| --- | --- |
| 0 | Thrust |
| 1 | Reverse |
| 2 | Rotate left |
| 3 | Rotate right |
| 4 | Afterburner held |
| 5 | Rocket activation pulse (one record only) |
| 6 | Safe-zone weapon stop |

Weapons are not input bits; they ride in the position's weapon field.

Records share the unordered game channel with positions, so every packet
repeats a rolling window of recent records (`ClientInputRing`). Each snapshot
carries an `InputApplied` watermark. The client drops records the server
already has, but always resends at least 4.

The server merges records into a 64-sequence history and rejects any attempt to
change a record it has already seen (`sequence_rewrite`).

### 4.3 Positions

The client sends one position per rendered frame, for the frame's last step.
It embeds the current `0x62` window in the position message. SCTP and DTLS may
bundle or fragment messages, so message counts are not UDP datagram counts.
A position carries:

- the Continuum fields (integer pixels, velocity, direction, energy, bounty and
  weapon);
- a `0x78` block with the raw `f32` bits of x, y, both velocities and
  orientation (`ClientPositionCanonicalState`).

The raw values must round to the integer fields. The client timestamp must be
zero; the server stamps accepted state with its own tick.

### 4.4 Shared movement code

Prediction and server replay call the same code in
`src/gameplay/player_movement.rs` (`step_player_movement`,
`integrate_player_collision`, wormhole gravity). It follows the reference game's
per-frame order:

1. Integrate and collide the previous velocity.
2. Apply gravity, rotation, thrust, afterburner, the speed cap and energy for
   the next frame.
3. Run the weapon phase, so bomb recoil takes effect on the following step.

Each side passes in tuning, prize upgrades, EMP, Rocket, Repel, doors and
bricks.

Projectile launch state uses the same integer protocol encoding on both sides
(`initialize_projectiles_for_protocol_direction`). As a result, the native
server and the WASM client start each shot from identical values.

### 4.5 Admission

`accept_position_relay_with_spawn_deferral` (`arena/positions.rs`) checks each
queued position. Any failure rejects the whole position, weapon included,
before anything changes:

1. **Shape.** The packet parses, the raw state matches the integer fields, and
   the position is in the world.
2. **Sequence.** It belongs to the current life and is newer than the last
   accepted sequence. Every input record since then is present, and the
   direction history is reachable at the ship's rotation speed.
3. **Time budget.** The client cannot claim more 60 Hz steps than server time
   allows, beyond one catch-up batch (`MovementTimeBudget`).
4. **Envelope.** Energy, speed and hull position are within limits; the hull
   is not inside a wall, door or enemy brick.
5. **Exact replay.** The server replays every input since the last accepted
   state (`CanonicalPositionWitness`). The replay includes walls, each
   record's door phase, enemy bricks, wormholes and the fired weapon's recoil.
   It must reproduce all five `f32` values bit for bit. A mismatch is rejected
   as `canonical_state_mismatch`, or starts a correction (section 4.6).
6. **Acceleration and path.** Acceleration, distance and the swept path stay
   within limits.

Only then are the weapon, energy and new state committed. The accepted state
becomes the next replay anchor, and the position is relayed. A burst limiter
(3 relays per 50 ms) may skip the public relay; it never affects acceptance.

Each player's queue holds 64 positions. A new movement-only position replaces
older movement-only ones; weapon-bearing positions and baselines are kept. A
position missing input records waits for them. Once a missing record is more
than 32 sequences old it can no longer arrive, and the server starts a
correction.

### 4.6 Corrections and the drift backstop

To move a client off its own lineage, the server proposes an exact state in a
reliable `0x7b`. The reasons are:

- sustained drift;
- a client-predicted weapon the server rejected, so its recoil was wrong;
- an unrecoverable input gap;
- an overlap with a new enemy brick;
- an unacknowledged Repel.

The client pauses and answers `0x63`, applying the correction or cancelling it
if a newer boundary such as a Warp came first. The server replays to the
client's application point, commits, and answers `0x7c`. Only then does the
client install the state.

As a backstop, `sim_observation.rs` runs a second, private simulation of each
ship. Error beyond the half-pixel wire rounding accumulates per axis, leaking by
0.96 per sample. Past 32 px (`MOVEMENT_DRIFT_SNAP_PX`) the server proposes a
sustained-drift correction.

| Setting | TOML / environment / CLI | Default |
| --- | --- | --- |
| Snap-back | `movement_snapback` / `ZUBSPACE_SERVER_MOVEMENT_SNAPBACK` / `--[no-]movement-snapback` | on |
| Simulation logging | `sim_observe` / `ZUBSPACE_SERVER_SIM_OBSERVE` / `--sim-observe` | off |

Applied snap-backs are counted in `movement_snapbacks_total`
(`/v1/zones/<zone>/status` and Prometheus) and logged as
`movement_snapback_committed`.

### 4.7 Rocket

Input bit 5 is a one-record activation pulse, not an "active" flag. On a new
pulse the server checks the Rocket inventory and the ship's `RocketTime`,
spends a Rocket, and derives the active interval from the pulse's sequence, so
holding the bit or delaying a packet cannot extend it. A private `RocketState`
in each self snapshot repairs lost predictions without ever lengthening one.

## 5. Visibility (AOI) and snapshots

`visibility.rs` indexes players in 1,024 px cells. For each recipient, every
other ship is near, mid or far:

| Tier | Radius | Contents | Rate |
| --- | --- | --- | --- |
| Near | 2,048 px | Full ship state | up to 60 Hz |
| Mid | 2,560 px | Position and life | 30 Hz |
| Far | beyond | nothing | none |

A ship is demoted only 256 px past a radius, and a ship closing in is
classified by up to 0.2 s of predicted travel. Bots are scanned separately
with the same rules. Under load, mid entries are shed first.

Snapshots are delta-encoded per recipient against its last acknowledged frame,
with 32 frames of history. The recipient's own entry is always present.
Recipient-private state is rebuilt into every frame, so a lost frame is
repaired by the next:

- `LifecycleState`: life, alive, respawn tick and Repel delivery;
- `RocketState`;
- `PrizeState`, or a compact reference to an earlier one;
- the `ObserverTimeline` directive;
- `InputApplied`.

Projectiles are not snapshot entities. Fire is announced by `0x7d` realtime
events, and clients simulate the visuals.

## 6. Weapons, energy and specials

### 6.1 Energy

Energy is server-owned (`src/authority/energy.rs`, `status.rs`): recharge and
drains once per tick, weapon cost on acceptance, and damage. The client
predicts it, adopts a lower server value immediately, and blends toward a
higher one.

### 6.2 Fire

A weapon rides on a position and is accepted only with it. The server previews
the weapon before the replay, because recoil changes the replayed velocity,
and commits it after the movement checks pass.

The core check (`src/authority/weapons.rs`) rejects shots that are:

- blocked by arena rules;
- a repeated Repel, Decoy, Burst or Thor;
- not in inventory, or above the ship's gun or bomb level;
- fired from a safe tile;
- unaffordable in energy;
- still on cooldown.

The server wrapper (`arena/positions/weapons.rs`) adds mine limits,
proximity-bomb safety, flag-carrier upgrades and the post-Warp lock. A shot
without a valid combat-view receipt is blocked.

Proximity-bomb safety (BombSafety) refuses a fired, not dropped, proximity
bomb when a hostile ship lies within `(ProximityDistance + level - 1) * 16`
pixels. The server judges it against the frame the shooter's combat-view
receipt names, the remote ship positions that browser had applied, measured
from the shooter's reported, rounded position. The browser keeps the remote
positions of each applied snapshot and judges the same frame and rounded
position before predicting the bomb and its recoil, so the two agree at the
boundary instead of differing by the presentation delay. Without a decision
or retained frame, the server falls back to the latest positions.

### 6.3 Specials

**Decoys** are server entities (`ServerDecoy`). Each copies its owner's
frequency and ship, bounces off walls and enemy bricks, and expires after
`DecoyAliveTime`. They share the projectile caps; at a cap the oldest Decoy
is removed.

**Repel** is resolved on the server when the attacker's Repel is accepted:

- **Projectiles and Decoys.** Hostile ones in range get the repel velocity and
  a fresh lifetime, and mines become moving bombs
  (`ProjectileRegistry::apply_repel`).
- **Ships.** Each hostile ship in range that is outside safety gets a private
  event, holding the new velocity and its duration in 60 Hz steps. A target
  queues up to 16. The event rides the target's snapshots until the client
  marks a position with `0x79`, naming the input after which it applied the
  push. The server replays to that input, applies the same velocity, and
  answers `0x7a`.

Warps wait until every pending Repel on the player has been applied.

**Rocket** is described in section 4.7, **Burst** and **Thor** in section 7.1,
and **Bricks, Portals and Warps** in section 10.3.

## 7. Projectiles, collision and damage

### 7.1 Projectiles

Accepted shots become `ProjectileRecord`s in `ServerState.active_projectiles`
(`src/authority_projectiles/`). This covers bullets, bombs, mines, Burst
pellets, Thor and shrapnel.

- **Movement.** Projectiles move in 10 ms substeps, X then Y, probing walls at
  most 8 px apart (`step_terrain`, `src/gameplay/weapon_motion.rs`). A long
  server step cannot carry a shot through a one-tile wall.
- **Thor** ignores walls, doors and bricks but still hits ships.
- **Burst** pellets ignore ships until they touch a wall.
- **Lifetimes** are arena settings, measured in server ticks.

### 7.2 Hit validation: the combat-view timeline

Players aim at remote ships drawn slightly in the past. Zubspace makes that
delay a server-owned timeline, so the server can judge hits against what the
shooter saw without letting the client pick a rewind.

**The directive.** The server sends each observer an
`ObserverTimelineDirectiveV1` (23 bytes, `src/combat_timeline.rs`) in its
snapshots:

- It sets a generation, a target delay and a ceiling.
- The delay changes by at most one tick per 10 ticks.
- A new generation can be issued at most every 100 ticks.

**The receipt.** The client's `ClientObserverCombatTimeline` draws remote ships
at that delay and adds a 14-byte `CombatViewReceiptV1` to each input record. It
names the life, input sequence, timeline generation, newest applied snapshot
and a hold flag, but never a time, delay or target.

| Profile | Target delay (ticks) | Ceiling (ticks) |
| --- | ---: | ---: |
| `clean-lan` | 7 | 9 |
| `supported-mild-wan` | 10 | 14 |
| `supported-slow-wan` | 15 | 20 |
| `recovery-stress` | 20 | 28 |
| `auto` (default) | 20 | 20 |

`auto` adapts each observer among 7, 10, 15 and 20 ticks from measured round
trip and loss ([`connection-quality.md`](connection-quality.md)). Select a
profile with `observer_network_profile`,
`ZUBSPACE_SERVER_OBSERVER_NETWORK_PROFILE` or `--observer-network-profile`.

**At collision time.** A human's projectile records the shooter's input
sequence and timeline generation. At each collision check, the server rebuilds
where the shooter's client drew each target:

- the render tick comes from the recorded schedule;
- target motion comes from `ActorMotionHistory` (65 samples per segment);
- only the snapshots actually sent to the shooter are used.

That projection is the target's anchor for direct hits, proximity and splash.
It must lie within 12 px of the target's real motion; otherwise the target is
unavailable to that shot. There is no fallback to the current position.

History never crosses a discontinuity such as a spawn, death, ship change,
Warp, Repel, correction or door relocation; each starts a new
`MotionSegmentIdentity`. Bot shots use the target's exact server motion, and a
player's own bomb splash uses the player's accepted hull. Withholding receipts
can only block a player's own new shots.

### 7.3 Fair warning before harm

`ThreatPresentationContract` (`threat_presentation.rs`) makes a threat visible
before it can hurt a human:

- **Attacker.** A non-self hit on a human needs the attacker's current life to
  have been sent to that human. Harm opens 50 ticks after that first send.
- **Shot.** The shot's own event or projectile state must have been sent at
  least 28 ticks before the hit.
- **Bots.** A bot does not fire at a human until its life has opened this way.

Readiness depends only on the server's successful sends, never on client
acknowledgements, so a client cannot extend its own immunity.

### 7.4 Damage transactions

A detected collision is frozen as an immutable record of the impact, victims,
anchors and damage. Pending hits have a stable collision-order ledger, bounded
at 128 per owner, 256 per primary victim life and 3,072 in total. New collision
discovery and damage commit attempts have separate 256-per-step budgets.
Unrelated free flight continues while an older collision awaits presentation.

Transactions sharing a shooter or any primary/splash victim retain their order;
blocked dependencies propagate to later transactions involving those players.
Independent transactions may commit while an older encounter waits for
presentation or delivery capacity. A shared explosion remains atomic with its
collision-time victim membership and geometry. Life/session validation and
cleanup still own cancellation; missing presentation never grants harm.

Mutable energy, lethality, prize loss and score derive at commit under the arena
transaction gate. The shared damage/prize RNG stream advances only on successful
commit, in actual commit order. Relative to the former arena-wide FIFO, an
independent encounter can therefore receive an earlier random draw. The stream
and distribution are unchanged; retries do not reserve draws or advance it.
Replay determinism includes scheduling/readiness inputs, not just collision order.

`damage_readiness_waits_total` and `damage_conflict_waits_total` count scheduling
attempts deferred for each reason. `damage_independent_commits_total` counts
successful commits while an older independent transaction is blocked. They are
available in zone status and as `zubspace_`-prefixed Prometheus counters, without
player labels. Shared CPU/locks and genuine global capacity limits remain shared
resources; this is combat scheduling isolation, not overload immunity.

The damage rules (`src/authority_damage.rs`, `src/combat.rs`,
`server_damage.rs`):

- explosion damage falls off with distance;
- Shield scales damage by its remaining fraction;
- an enemy EMP bomb freezes recharge, but a player's own splash never EMPs
  them;
- a lethal self-bomb leaves the shooter at 1 energy.

Each hit produces a WATCH_DAMAGE event for the victim, the shooter and nearby
players. Clients use it only for effects. Energy comes from snapshots, and
death from the lifecycle path.

## 8. Death, respawn, ships and spectating

### 8.1 Death and score

The kill pipeline (`build_kill_pipeline` and `build_bot_kill_pipeline`):

- awards the killer a win;
- unless it was a self-kill, gives the killer kill points equal to the
  victim's bounty;
- gives the killer a bounty increase if the shot came from its current life;
- gives assists to players who damaged the victim within the last 500 ticks.

The victim's life then advances. The server sends a reliable `PLAYER_DEATH`
(`0x06`) with the killer, the death-green id, the next life and the respawn
tick. Every snapshot also repeats the private `LifecycleState`, so a delayed
reliable death cannot keep the victim alive locally. Whichever arrives first
applies the death, and it is applied once.

### 8.2 Respawn

The respawn delay is `EnterDelay` plus 1.25 s, and at least 2 s. When it
expires:

1. The client sends a neutral baseline reliably: zero velocity, the server's
   energy and bounty, at the map's fixed spawn anchor. This proves only that
   the client is ready.
2. The server picks the actual spawn
   (`server_random_spawn_coordinates_from_settings`), avoiding enemy bricks.
3. The server returns that spawn in the `0x75` receipt.

If no spawn is free, the baseline waits and is retried.

### 8.3 Ship and frequency changes

A ship change (`0x18`) needs full energy, otherwise the server rejects it
(`0x7f`). On success the server advances the life and answers `0x1d` with the
new `life_seq`. The client then sends a neutral baseline and waits for `0x75`,
as after a respawn. The client abandons a transition after 15 s.

Zubspace is free-for-all: every player and bot has its own frequency, equal to
its player id, and frequency requests are rejected. A debug-only team mode
(`ZUBSPACE_SERVER_TEST_TEAM_MODE`) keeps team rules for tests, such as prize
sharing, turret attach and frequency changes. In that mode a frequency change
is acknowledged with `0x74`, carrying an exact baseline that the client echoes
back before `0x75`.

### 8.4 Spectating

Ship 8 is the spectator. Switching to it is a ship change, so it needs full
energy. The server can also force a player to spectate for a bad connection
([`connection-quality.md`](connection-quality.md)) or for idling at spawn
([`gameplay.md`](gameplay.md)).

Spectators send no positions. A scoped spectator-view request moves only their
AOI centre (`player_events/spectate.rs`). Spectators see every frequency.

## 9. Prizes

### 9.1 Spawns and claims

**Hidden greens** (`hidden_prizes.rs`):

- Every `PrizeDelay` the server draws `PrizeHideCount` candidates from its own
  RNG.
- At most `PrizeFactor × players / 1000` greens are live, never more than 256.
- Players means live humans in ships plus bots, and nothing spawns without a
  human.
- Skipped candidates still consume their random draws, so the sequence is
  deterministic.

**Death greens** drop where a moving ship died. They use a separate RNG stream
and no negative prizes.

Every green gets a nonzero collectible id that is never reused, even across
arena resets.

**Claims.** A claim (`0x07`) runs in the reliable gameplay FIFO. The server
reads only the collectible id and checks overlap against the ship's last
accepted position. One transaction removes the green, applies the effect,
records a receipt, acknowledges privately (`0x61`) and broadcasts. The winner
can retry safely; everyone else is rejected.

### 9.2 Effects

Effects are shared rules (`src/authority_prizes.rs`,
`src/gameplay/shared_rules.rs`). Each context draws from its own list:

| Context | Excluded prizes |
| --- | --- |
| Initial bounty, replayed with a fixed seed at every spawn | Warp, Full Charge, Engine Shutdown, Super, Shield, Brick |
| Multiprize sub-prizes | Warp, Engine Shutdown, Super, Shield, Multiprize, Brick |
| Hidden and death greens | none |
| Damage loss (victim ship's `DamageFactor` chance) | Warp, Full Charge, Engine Shutdown, Super, Shield, Multiprize |

With `maxed_spawn_loadout` on (the default), each spawn then raises energy,
movement stats, weapons and statuses to the ship's maximums. See
[`gameplay.md`](gameplay.md).

Damage loss runs inside the damage transaction. It removes one prize from the
victim of a non-lethal hit, but only when the victim has bounty and less than
full energy.

A damage loss that changes rotation, thrust, speed or recharge applies on the
server at once, while the victim's browser keeps its previous values until the
private `PrizeState` reaches it. For input up to one measured round trip plus
200 ms past the last input the server had received (at most one second of
input, same life), a movement replay or turn history that fails at the new
values is retried once at the replaced values (`DamageMovementGrace`).

How the main effects apply:

- **Movement upgrades** step the live `f32` value and clamp it to the ship's
  limits.
- **Stealth, cloak, X-radar and antiwarp** are server-owned capabilities.
  Their toggles are client input, but the server strips unowned toggles and
  drains energy for active ones.
- **Super** draws a duration below `SuperTime`.
- **Shield** lasts `ShieldsTime`.
- **Engine Shutdown** lasts `EngineShutdownTime`, tripled for the negative
  prize.

Prizes that change movement (recharge, rotation, thrust, speed, Engine
Shutdown and positive Multiprize) need an exact switch-over, or the replay
could not know which inputs used the old values. The client pauses, applies the effect,
and reports the first input that used it (`0x64`). The server installs the
effect at that input and answers `0x76`.

The client predicts pickups for feel. The canonical state is the private
`PrizeState` in every self snapshot. It holds bounty, maximum energy, movement
values, inventory and timed-effect deadlines, keyed by life and revision.

## 10. Map features

### 10.1 Safe zones

Safety comes from the server's map: tile 171 at the accepted position. The
client's safety bit is ignored. Inside a safe zone:

- weapons are rejected;
- bricks cannot be dropped;
- hostile Repel skips the ship.

Entering safety cancels the owner's live projectiles and Decoys
(`server_tick/safety.rs`). A weapon key pressed inside safety stops the ship,
recorded as input bit 6.

### 10.2 Doors

Dynamic doors use the reference game's door RNG with a fixed seed. One arena timestamp,
sent in the security challenge, anchors it (`doors.rs`). The client advances
doors from its estimate of the server tick, and may run at most 100 ticks past
the newest one it has (`DOOR_CLOCK_MAX_EXTRAPOLATION_TICKS`).

Each input record names the door seed it used. The server accepts only phases
the schedule could have reached by then, and replays collision with that
phase. When a door closes on a ship, the relocation is accepted only if the
replay reproduces it, rebuilding up to 4,096 intermediate phases.

### 10.3 Bricks, Warp and Portal

**Bricks** (`objectives/bricks.rs`) spend server inventory and are placed from
the server's own position and heading.

- A drop is refused in safety, without inventory, during the post-Warp lock or
  during cooldown.
- Bricks last `BrickTime` and are replayed to late joiners.
- They block other frequencies' ships, Decoys and projectiles; Thor passes
  through.
- A ship trapped by a new enemy brick is moved by a correction.

**Positive Warp prize.** A claim authorises one relocation; up to 64 can be
outstanding. The client relocates and marks its positions with `0x76` until the
server's `0x77`. Ordinary positions cannot use an authorisation.

**Portal placement** (Shift+Insert, `0x65`) names the client's last position.
The server waits up to 12 ticks for that position, then checks life,
inventory, settle time and enemy antiwarp. It stores one Portal per player,
expiring after `WarpPointDelay`, and answers `0x66` with the remaining count.

**Manual Warp** (Insert, `0x6a`, answered by `0x6b`) waits the same way.

- **With a Portal:** the ship moves to it and keeps velocity and energy.
- **Without one:** the ship needs full energy and a ready bomb cooldown. It
  stops at the spawn point, or at a random clear spot with Shift+0, and drops
  to 1 energy if it moved.

Afterwards, bombs, mines, Decoys, Burst, Thor, Rocket and bricks are locked
for 50 ticks.

## 11. Bots

Multiplayer bots are server actors (`state/bots.rs`, AI in
`server_tick/bots.rs`). Each has a synthetic session, an id from `0x300` up,
its own frequency and an advancing `life_seq`.

`bot_count` (default 0) sets the number of bots, and `bot_ship_types` lists
ship numbers assigned round-robin.

Bots share player authority for everything that affects others: energy,
projectiles, the damage FIFO and kill pipeline, respawn, prize effects, score,
realtime events and motion history. Their shots at humans follow the
fair-warning rule (section 7.3).

Bots differ from players in three ways:

- they steer with their own kinematics, sharing only the collision solver;
- AI skill tiers adjust aim, target reaction and fire cadence (2×/3×/4× the
  authored delay for Hard/Medium/Easy), without changing ship stats;
- they collect prizes by overlap rather than by claim.

Tactics are in [`gameplay.md`](gameplay.md). `src/bot.rs` is for offline play
only.

## 12. Objectives

Flags and powerballs have a server engine (`objectives/`). It runs only when
`enable_objectives` is set (`--enable-objectives` or
`ZUBSPACE_SERVER_ENABLE_OBJECTIVES`) **and** the zone's `arena.conf` has an
`[Objective]` section. Neither shipped zone (`chaos-zone`, `extreme`) has one.

When enabled:

- objective actions are reliable requests bound to the session, life and
  accepted position;
- they run in the reliable gameplay FIFO;
- their state reaches clients as versioned reliable snapshots;
- clients render it without simulating flags or balls locally.

## 13. Client reconciliation

| Client component | Owns |
| --- | --- |
| `ClientAuthoritySequencer` (`src/game/lifecycle_sequencer.rs`) | Session, stream, player and life scope; lifecycle phases; application barriers and their deadlines |
| `ClientObserverCombatTimeline` (`src/combat_timeline.rs`) | Remote render delay from the directive; the receipt for each input |
| `RemoteWeaponPresentationQueue` (`src/game.rs`) | Remote weapon events waiting for their source ship to appear |
| `Game` | The predicted own ship |
| `NetworkWorld` (`src/network/`) | Roster, snapshot entities, interpolation |

Prediction runs only while `ClientAuthorityView::gameplay_scope` is present:
the sequencer is `Live` and no application barrier is pending. Facts from all
three channels pass through one admission plan per poll. Facts from an older
session, stream or life are inert, and a rejected snapshot is ignored
entirely.

The client changes its own canonical ship only from:

- its self snapshot entry: energy, and position while respawning;
- `PrizeState`: same or newer revision, current life only;
- receipts and terminals: `0x75`, `0x77`, `0x7c`, the frequency baseline, and
  the Portal and Warp terminals.

Remote ships are drawn at the observer timeline's render tick with Hermite
interpolation, holding the last sample rather than extrapolating. Without a
directive, the client falls back to an adaptive 60–280 ms buffer.

## 14. Reference implementations

The reference game's client is the behavioural reference: what a mechanic
should do, plus its frame order,
rounding and weapon rules.

The Zubspace server is the state authority. Where the reference game decides something
locally, such as hits, weapon removal or effect expiry, Zubspace decides it on
the server. A client-side shortcut is a bug, not a port.

Units (orientation in turns from 0 to 1), transport and owner decisions such
as free-for-all differ on purpose. Port reference code by converting units, and
read tuning from each zone's `arena.conf`. [`../AGENTS.md`](../AGENTS.md) maps
subjects to the reference game's files.

## 15. Source map

Paths without a directory are under `src/bin/zubspace_server/`.

| Area | Files |
| --- | --- |
| Entry and tick | `src/bin/zubspace_server.rs`, `server_tick.rs`, `server_tick/` |
| State and identity | `state.rs`, `state/`, `src/authority_identity.rs` |
| Ingress | `connection.rs`, `rtc.rs`, `reliable.rs` |
| Admission, weapons, lifecycle | `arena/positions.rs`, `arena/positions/`, `arena/roster_changes.rs`, `arena/lifecycle.rs` |
| Replay and drift | `sim_observation.rs`, `state/movement_clock.rs` |
| Shared rules | `src/gameplay/player_movement.rs`, `src/gameplay/shared_rules.rs`, `src/gameplay/weapon_motion.rs` |
| Energy, status, fire | `src/authority/` |
| Projectiles and damage | `src/authority_projectiles/`, `src/authority_damage.rs`, `server_damage.rs`, `server_explosions.rs` |
| Combat view | `src/combat_timeline.rs`, `threat_presentation.rs` |
| Prizes | `hidden_prizes.rs`, `player_events/prizes.rs`, `src/authority_prizes.rs` |
| Doors, bricks, objectives | `doors.rs`, `objectives/` |
| AOI and snapshots | `visibility.rs`, `snapshot.rs`, `src/protocol/snapshot.rs` |
| Bots | `state/bots.rs`, `server_tick/bots.rs` |
| Configuration | `config.rs`, `config/zubspace_server.toml` |
| Client authority | `src/game/lifecycle_sequencer.rs`, `src/game/network.rs`, `src/game/network/` |
