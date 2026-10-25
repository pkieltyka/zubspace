# Physics and client presentation

The client and server compile the same movement, weapon-launch and terrain
rules from this repository. They have different runtime adapters: the client
predicts and draws, while the server admits inputs and resolves the game.
There is no separate physics crate or independently maintained server copy.

## Where the rules live

The client declares modules from `src/main.rs`; the server includes shared
files through `#[path]` declarations in `src/bin/zubspace_server.rs`.

| Shared source | Responsibility |
| --- | --- |
| `src/gameplay/player_movement.rs` | Thrust, rotation, speed limits, movement modifiers, ship collision and wormhole gravity |
| `src/gameplay/shared_rules.rs` | Weapon launch layout, inherited velocity, recoil, shrapnel generation, door-overlap rules and shared timing constants |
| `src/gameplay/weapon_motion.rs` | Swept projectile terrain motion and bounce axis order |
| `src/continuum_position.rs`, `src/continuum_weapon.rs` | Protocol rounding, direction bins, weapon encoding and contact geometry |
| `src/authority/`, `src/authority_damage.rs`, `src/authority_projectiles/` | Energy, weapon eligibility, projectile state and authoritative collision/damage helpers |
| `src/combat_timeline.rs` | The observer schedule and receipts used to relate a rendered view to server collision history |

The movement core accepts plain state, tuning and sampled environment facts.
The adapters supply map solidity, door phase, frequency-aware bricks, prizes
and temporary effects. Sharing the integrator does not remove the need to
keep those inputs consistent. Server adapters live in
`src/bin/zubspace_server/sim_observation.rs` and `arena/positions/`; client
adapters live in `src/gameplay/` and `src/game/simulation/`.

## Units and clocks

| Quantity | Representation |
| --- | --- |
| World position and simulation velocity | Pixels and pixels per second; map tiles are 16 pixels |
| Orientation | Turns in `0.0..1.0`; the weapon protocol quantizes directions into 40 bins |
| Encoded projectile velocity | Tenths of a pixel per second; convert before comparing with simulated motion |
| Human movement inputs | Fixed 1/60-second logical steps with wrapping input sequence numbers |
| Server loop | 60 Hz by default, configured separately with `tick_rate` |
| Continuum time | 100 ticks per second; full gameplay time is a wrapping 31-bit value, and position packets use a truncated u16 timestamp |
| Browser render time | Animation frames, with a fractional movement preview for drawing |

Input sequences, snapshot ids, protocol ticks and render ordinals are distinct
identities. Use the wrap-safe helpers and clock conversions in the code rather
than subtracting different clocks or treating every tick as 1/60 second.

## Ship movement and tuning

The client accumulates render time in `FixedGameplayClock`
(`src/fixed_step.rs`). It runs complete 60 Hz movement steps, then draws a
throwaway fractional preview through `Game::local_render_prediction`. The
preview does not create an input record or replace canonical state.

Movement follows the reference game's order: integrate the previous velocity
and resolve collision, then apply gravity, rotation, thrust and speed limits
for the next step. The weapon phase follows movement, so bomb recoil affects
subsequent motion. Afterburner depends on thrust input and available energy;
Rocket, EMP, Shutdown, Repel and zone settings supply their own modifiers.

Ship tuning comes from each zone's `arena.conf`, encoded by the server's
`arena_settings/` code and decoded by the client. Spawn loadout and prize state
determine which upgrades are active. Maximum starting upgrades do not give
every ship the same underlying thrust, turn rate, energy or weapons.

The server replays the submitted input history and checks the client's exact
five-component movement state, sequence and time budget before publishing an
accepted position. Invalid movement is rejected; lifecycle changes and
correction receipts rebase prediction. The detailed admission rules are in
[Server authority](authoritative_server.md#4-input-and-movement).

## Weapon motion and collision

Shared launch rules derive barrel layout, ship-velocity inheritance and
weapon velocity from the accepted firing pose and ship settings. The client
predicts local weapon visuals; the server checks cooldowns, inventory, energy
and safe-zone rules before committing the shot.

Both runtimes use the shared terrain step: 10 ms projectile substeps, X before
Y, with swept probes no more than 8 pixels apart. The runtime supplies terrain
solidity and handles weapon-specific bounce, proximity and terminal behavior.
Ship contacts, explosion damage and deaths are resolved by the server.
Contact boxes and pixel rounding are part of the rules; render smoothing must
not change them.

Remote weapon state is anchored to server time and the immutable shot identity,
including its source session, life and shot group. Delayed events do not give
a weapon a new lifetime. Expiry, cancellation, Repel mutations and terminal
events constrain what the client may still show. Owner death alone does not
expire a weapon. See `src/game/simulation/remote.rs`,
`src/remote_weapon/` and `src/authority_projectiles/`.

## Three presentation paths

- **Own ship:** immediate prediction from local inputs, with the fractional
  drawing preview and server-owned rebases described above.
- **Remote ships:** interpolation at the server-issued observer render tick,
  holding an available pose when history cannot bracket it. They do not use
  open-ended position extrapolation.
- **Weapons:** local simulation from their authoritative source and state,
  using shared terrain rules. A fresh remote shot can begin on its source
  actor's presentation cursor and converge to current weapon time before its
  harm deadline. This visual convergence neither adds lifetime nor delays
  server damage (`src/remote_weapon/view_clock.rs`).

The server verifies combat-view receipts and resolves collision against its
retained presentation history. A client does not choose an arbitrary rewind
time or report its own hits. [Networking](networking.md) describes the wire
schedule and [Server authority](authoritative_server.md#7-projectiles-collision-and-damage)
describes collision and threat-presentation rules.

## Recovery and verification

At most eight movement steps run after a slow render; excess elapsed time is
discarded. Closed readiness barriers discard accumulated gameplay time too.
This bounds recovery work and packet bursts; it does not promise uninterrupted
simulation during a suspended tab. Fresh snapshots and exact application
receipts restore the client to the server's current state.

Native tests cover shared rules, protocol rounding, replay and authoritative
damage. Retained contact and shrapnel fixtures pin reference behavior.
`make test-quick` is the routine check; focused real-browser combat and recovery
scenarios test how the runtimes fit together. Test commands and their scope
are documented in [the E2E guide](../tests/e2e/README.md).
