# Isolate combat from a slow client's presentation readiness

Status: implemented and locally validated. The original investigation and design
are retained below; completion details and evidence are in the final section.

## Goal

A client with a slow or stalled connection must not pause unrelated projectile
flight or independent fights in the same arena. Preserve server authority and
our rule that a remote threat must be presented before it can harm its victim.
Keep ordering where transactions interact, rather than requiring every fight
to wait for the oldest pending hit anywhere in the arena.

This plan does not change ship prediction, weapon tuning, WebRTC reliability,
connection-quality thresholds, or production configuration.

## Findings and evidence

The local investigation confirmed a combat scheduling dependency, not a shared
network-send wait:

- Each RTC session has bounded queues and its own outbound dispatcher. Game,
  event and reliable lanes have independent asynchronous writers. Stalled
  sends do not synchronously wait inside the world tick. Reliable overflow
  terminates that session, and replaceable snapshots coalesce.
- In `ProjectileRegistry::step_with_damage_geometry_and_harmful_eligibility_transactional`,
  pending hits form one global FIFO. `take_while(harmful_eligibility)` selects
  only its ready prefix. An unready older hit makes `new_hit_budget` zero.
- The subsequent projectile loop checks that budget before stepping physics.
  Consequently, even a distant free-flight projectile owned by another player
  is marked `capacity_paused` and does not move.
- Damage preparation/application has additional dependencies. A prepared bomb
  transaction waits for all of its current human victims to become ready;
  freezing that shared explosion's membership and geometry is intentional.
- CPU, the ingress executor and world locks remain shared. Transport queue
  bounds alone do not guarantee isolation under overload or abusive traffic.

A native characterization used the actual server projectile registry: after
creating one unready collision, it added a distant projectile from a different
owner. The unrelated projectile stayed at **x=6000**, while an identical control
without the pending hit advanced to **x=6001** on the same step. This demonstrates
the code path; it does not identify the cause of a particular production incident.

Nine selected existing native transport/readiness tests also passed. That shows
those specific protections still work, not that combat isolation passes. The
characterization itself asserted the defective behavior; the permanent regression
must assert the opposite and fail before the fix.

Local retained investigation artifacts (gitignored and machine-local):

- `.debug/slow-client-isolation/REPORT.md`
- `.debug/slow-client-isolation/projectile_probe.rs`
- `.debug/slow-client-isolation/projectile-probe.log`
- `.debug/slow-client-isolation/result.json` and individual test logs
- `.debug/slow-client-isolation/source-sha256.txt`

The investigation used a dirty shared checkout and temporary test-only
instrumentation. Its recorded HEAD is context, not a clean release provenance
claim. Port the reproduction into tracked tests so future verification does not
depend on those local artifacts.

### Deadline qualification

Source-life readiness has a 50-Continuum-tick (500 ms) hard opening **after first
emission**. A source not yet emitted has no such contract/deadline. Exact
projectile readiness also has its own emission/lead requirement. Therefore,
500 ms is not a universal bound on this arena-wide stall. Preserve those
distinctions when measuring and designing recovery; do not turn missing
presentation into permission for invisible damage.

## Implementation sequence

### 1. Make the defect and transaction dependencies explicit

Add a native regression in `src/authority_projectiles/tests.rs` with independent
owners and spatially separated fights. An unready A-to-B collision must coexist
with C's bullet continuing to move and a ready C-to-D hit progressing. Run it on
the existing code first and retain the failing output.

Review `src/bin/zubspace_server/server_tick.rs`, `server_damage.rs` and
`threat_presentation.rs` alongside the registry. Identify every mutable read/write
that makes two transactions dependent:

- Exact victim runtime/life, energy, shield/EMP and prize/loadout changes.
- Linked-shot consumption, projectile/group lifetime and terminal state.
- Shared explosion membership, self-damage and secondary fragments.
- Death, score/bounty and lifecycle transitions.
- Damage/prize RNG advancement and any other shared mutable ledger.

The existing global-FIFO tests protect real behavior. Do not simply replace
`take_while` with `filter` or disable the readiness barrier.

### 2. Separate physics progress from damage commit eligibility

Allow unrelated free flight to advance even when older damage is deferred.
Freeze only the affected projectile/group at its real collision, with immutable
collision-time geometry, identity and expiry semantics. Use explicit bounded
storage and work budgets; do not let this change create an unbounded backlog or
skip collisions when capacity is exhausted.

Separate the physics-work budget from damage retry/commit capacity. Retain fair,
bounded service across owners, and ensure retrying a blocked transaction does
not consume every opportunity for unrelated new work.

This is an intermediate improvement, not completion: unrelated healthy damage
must progress too, not merely animate while waiting for the same global queue.

### 3. Schedule only genuinely dependent damage in order

Replace the arena-wide readiness dependency with stable ordering for conflicting
transactions. A deferred transaction must reserve all affected victim lives and
shared effects; independent transactions may proceed without overtaking an older
transaction that changes their inputs. Preserve atomic shared explosions.

Resolve the RNG dependency explicitly before enabling independent commits. Audit
whether the current global stream can be safely reserved at a defined immutable
stage, or whether a separately reviewed deterministic ownership scheme is needed.
Keep replay determinism and the intended damage/prize distribution. Do not draw
random outcomes again on retry or quietly change which dependent hit consumes
which draw. Document the chosen rule and update tests that intentionally encode
the superseded global scheduling policy.

Define cleanup and timeout behavior for missing presentation: disconnect,
spectator, death, arena change and numeric ID reuse must release only the correct
runtime's work. A stalled client's own pending work must remain bounded without
blocking unrelated owners. Do not make a global lock span a transport wait.

### 4. Add narrowly scoped diagnostics

Retain enough evidence to distinguish presentation deferral from real capacity
pressure: pending age, blocked owner/victim/group, reason, and independent work
serviced while a transaction waits. Aggregate counters should have bounded
cardinality; exact identities belong in bounded DEBUG/QA traces, not Prometheus
labels. Reuse existing queue, readiness and tick metrics where sufficient.

## Native regression coverage

Use small deterministic tests to establish:

1. One blocked hit does not freeze a distant owner's free flight or ready hit.
2. Ready work progresses across repeated ticks while the blocked hit stays pending.
3. Same-victim hits preserve energy, lethal-hit, score, prize and lifecycle order.
4. Linked bullets still damage once; a wall collision does not consume siblings.
5. A shared explosion keeps its original victim set and geometry and applies once.
6. Disconnect/life replacement cancels stale work without releasing unrelated work
   incorrectly. Releasing readiness commits the blocked encounter once.
7. Per-owner/global limits and retry budgets stay bounded and fair; a blocked
   owner cannot occupy all useful service indefinitely.
8. Original weapon expiry, Repel lifetime extensions, frozen flight and shrapnel
   behavior remain correct. Retries do not invent motion or extra lifetime.
9. Deterministic RNG/ledger behavior follows the explicitly chosen scheduling rule.

Start with the new regressions and affected registry/server-tick tests. Run
`make test-quick` after the implementation batch. Do not run the full browser
matrix between edits.

## Narrow local E2E regression

Implemented target: **`make test-e2e-slow-client-isolation`**.
Implemented script: `tests/e2e/slow_client_isolation.mjs`.
The lane is also appended to focused acceptance without renumbering existing lanes.

Use four real Chrome clients and the real WebRTC server in one Chaos arena:
A fires at B, while C and D fight at a separate, hull-clear location. Disable
bots for this test. Use existing deterministic spawn/setup support and genuine
keyboard flight/fire; do not inject gameplay packets, damage or client positions.

### Realize the correct impairment

A generic browser S2C-drop hook is insufficient by itself: the server may already
have recorded successful emission, letting the normal lead/deadline open harm.
First trace the authoritative readiness boundary and identify all event/snapshot
paths that can satisfy it.

If ordinary network impairment cannot reliably hold that boundary, use a narrowly
scoped, debug-only server QA latch for B's exact source/projectile presentation.
Hold the actual outbound presentation work before the normal emission fact is
recorded; do not falsify readiness timestamps or receipts. Cover alternate
presentation paths so a snapshot cannot silently bypass the hold. Keep quality
probes, admission and unrelated clients flowing. The latch must be opt-in,
bounded, absent from release behavior and automatically released during cleanup.
This proves stalled-presentation isolation, not general Internet packet-loss
coverage. A small real-UDP impairment variant can supplement it afterwards.

### Scenario and assertions

1. Establish a clean control shot in the healthy pair, retaining server collision
   and damage records plus both clients' presentation evidence.
2. Arm B's presentation hold. Fire A's real shot and require an exact pending
   collision, an explicit readiness-deferral reason and a stable victim life.
   A merely slow-looking browser is not proof that the defect was exercised.
3. While that same transaction remains unready, fire C's shot at D. Require
   authoritative forward projectile motion, drawn projectile evidence and exact
   once-only damage on the healthy victim **before B's hold is released**.
4. Fix a useful measurement window before implementation: target two seconds of
   realized hold with a few measured healthy shots. Require healthy collision to
   commit within two configured server steps after eligibility, plus an explicit
   harness observation allowance; do not loosen the deadline after a failed run.
   Use server ticks for authority assertions and wall time separately. Continuum
   ticks are 100 Hz; do not convert them using the 60 Hz simulation loop rate.
5. Prove B took no premature damage. Release the hold through the normal writer
   path and require valid presentation/readiness followed by one terminal result,
   without duplicate damage or a burst of stale replayed hits.
6. Disconnect all peers and require drained fixtures, empty packet queues and
   ingress cleanup. Abort/teardown must release the latch even on failure.

Before the fix, this scenario must fail specifically because independent
projectile/damage progress stops behind the verified pending transaction. A setup,
geometry or impairment failure is not the expected red result. After the fix,
the identical scenario must pass with unchanged gameplay requirements.

Reuse `focused_browser_evidence.mjs` and existing projectile-presentation evidence
helpers. Retain exact source/build provenance, server DEBUG logs, selected-route
and realized-impairment evidence, collision/transaction IDs, life identities,
both perspectives' presentation traces, timing summaries and failure screenshots.
Use the repository's Hyprland background-workspace wrapper for headed browsers.

Once implemented, local usage should be:

```sh
make dev
scripts/e2e-hyprland-tools/run -- make test-e2e-slow-client-isolation
```

Start with this one Chrome/Chaos lane. Renew affected shared-explosion,
linked-shot, recovery and capacity coverage after code review; broaden browser or
WAN coverage only where the change warrants it. Reserve the full applicable
acceptance suite for the final release checkpoint.

## Completion criteria

- Native regressions and the narrow real-browser lane demonstrate independent
  flight **and damage** while another victim remains unready.
- Presentation fairness, interacting-hit ordering, shared explosions, bounded
  capacity and lifecycle cleanup remain intact.
- All scheduling/RNG behavior changes are documented and deliberately tested.
- Existing failures and pre-fix evidence remain available; no test requirement is
  weakened to obtain a pass.
- Report remaining limits honestly. This work does not establish immunity to
  host saturation, denial of service or every possible WAN condition.

## Implemented design and validation

The registry now separates new-collision discovery from damage retry capacity.
It retains per-owner bounds, adds a 256-per-primary-victim-life cap, and rolls
back the complete projectile step (including a wall-despawn mark) when that
victim has no space. Existing expiry and frozen-flight/Repel semantics remain.

The server freezes up to 256 *new* transactions per step without repeatedly
spending that budget on the already-prepared prefix. It then scans the bounded
collision ledger in stable order. Deferred transactions reserve their shooter
and all primary/splash victims; these conflicts propagate transitively. A ready,
independent transaction can commit even when an older one waits. Commit attempts
remain bounded at 256 per step. Unknown, unprepared splash membership cannot be
overtaken. Atomic damage and delivery reservations remain in place.

RNG policy is deliberately **actual commit order**, under the existing world
transaction gate. An independent ready encounter can consume a draw before an
older waiting encounter. Random distribution and stream implementation do not
change; retries do not reserve or advance draws. A native control/retry comparison
checks resulting seeds, victim energies and prize revisions. This avoids treating
an arena-wide RNG stream as a reason to block all independent fights.

Three bounded-cardinality counters expose readiness waits, actor-conflict waits
and independent commits. Exact collision and commit ticks are distinguished in
committed-hit logs. The real-browser seam retains queued presentation envelopes
before ordinary emission and is absent from release builds.

Verified coverage includes the new free-flight, independent damage, full-prefix
starvation, per-victim wall rollback, release/idempotency and commit-order RNG
regressions, plus the existing damage, linked-volley, explosion, lifecycle,
capacity and transport tests. `make test-quick` and release server integration
are the non-browser gates; complete current counts are retained in their logs.

The narrow E2E passed with two healthy shots while another exact collision waited
for roughly 3.5 seconds. Healthy shooter and victim both submitted moving draws
and received exact damage. The released victim also drew its held projectile
before damage. The old scheduler was tested in an isolated checkout with only
the same debug hold/test instrumentation and a commit-tick log field added: its
control shot and hold setup passed, then independent damage timed out. Retained
pre-fix data includes four frozen state facts and 319 frozen draw frames in the
healthy victim's view, so the finding does not rely on a timeout alone.

Retained local evidence is under
`.debug/slow-client-isolation/implementation/`: `native-red.log`,
`native-green.log`, `quick.log`, `release-tests.log`, the pre-fix
`browser-baseline-failure/`, initial successful `browser-third/`, and final
renewal `browser-final/`. Earlier browser harness failures remain alongside them.
The first browser attempt incorrectly used the compact metrics reader for the
render ring; the second queried process instead of zone diagnostic counters.
Those readers and static-server cleanup were corrected without weakening damage,
rendering or scheduling assertions. Build/source provenance and RTC cleanup are
retained by the standard focused-evidence wrapper.

No production deployment is part of this change. Shared actors/explosions still
retain their necessary ordering. Genuine global capacity exhaustion, shared CPU
and atomic fan-delivery admission remain shared resources; this does not promise
that all overload or adversarial traffic is isolated.
