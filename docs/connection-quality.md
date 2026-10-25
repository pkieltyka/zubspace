# Connection quality

The server measures every player's link itself, with challenge/echo probes on
the unreliable game channel. Nothing the client reports (pings, traces, input
gaps) feeds the measurement. The readings are used for three things:

- **Forced spectator.** A player whose link stays bad for 10 seconds is moved to
  spectator for one minute and can return once the link is healthy again.
- **Presentation delay.** Under the default `observer_network_profile = "auto"`,
  the delay at which each player sees remote ships is chosen from their probe
  round-trip times ([Networking](networking.md#remote-presentation)).
- **Event copies.** With `realtime_event_game_copy = "on-loss"`, realtime events
  are copied onto the game channel only while a player's probes show loss
  ([Networking](networking.md#realtime-events)).

The monitor is in `src/bin/zubspace_server/connection_quality.rs`, the hold
registry in `connection_quality/holds.rs`, the presentation policy in
`connection_quality/presentation.rs` and the wire format in
`src/protocol/connection_quality.rs`.

## Probes

A probe (`0x81`) and its echo (`0x82`) are both 30 bytes:

```text
type:u8 | version:u8 = 1 | server_session_id:u64 | arena_stream_id:u64 | sequence:u32 | nonce:u64
```

- The server sends one probe every 500 ms to each joined, snapshot-ready
  player, with at most 8 pending at once. The nonce is random and nonzero.
- The client echoes a probe only if its session and arena stream match the
  client's current join. The echo is produced when the game polls its inbound
  packets, so the measured round trip includes the browser's processing time.
- Both directions use only the game channel (unordered, 80 ms lifetime). A probe
  or echo is never moved to the reliable channel, which would hide loss behind
  retransmission.
- The probe's send time is taken by the channel writer immediately before the
  DataChannel send, after any server-side queueing. The echo is timestamped in
  the game channel's receive callback, before the application queue, and
  accepted only if session, arena stream, sequence and nonce all match a
  pending probe and it arrived within 3 s.

So the measurement excludes the server's own queueing in both directions but
includes the network path both ways and the browser's processing. A missing echo is
round-trip probe loss: it cannot say which direction lost the packet, and it is
not the raw UDP loss rate.

## Measurement window

- A probe's outcome (echo or loss) is settled 3 s after it was sent, so slow
  successes and losses are counted at the same age.
- The window keeps outcomes from the last 30 s, at most 64.
- A reading needs at least 20 outcomes before it can judge the link. It reports
  the average RTT of answered probes and the fraction of probes not answered.

The window is discarded, without touching any hold already in force, when the
server cannot trust its own timing or the transport is known to be down:

- the server's polling of the session stalls for 1 s or more;
- the server is overloaded: the average tick work reaches four tick budgets
  (at least 50 ms);
- a queued probe has not been sent within 1 s;
- the WebRTC peer leaves the `connected` state (for example a closed tab
  waiting for ICE to time out), until it is connected again;
- the page is hidden (another tab, minimized), until it is visible again.

**Hidden pages.** A hidden browser page runs no frames, so it answers no
probes, and a few seconds of that silence would read as heavy loss. The page
therefore sends a reliable `0x85` notice (19 bytes: kind, version 1, server
session, arena stream, hidden flag) whenever it hides or shows. While hidden,
the server sends no probes and judges nothing; when it shows again, measurement
starts afresh. The notice never lifts a hold, grants play or adds a sample. A
hidden page sends no positions either: one that sends more than 60 while
claiming to be hidden (beyond those in flight when it hid) loses the pause,
and that session's later hidden notices are ignored. Browsers that never send
the notice are measured as before.

A new join or arena change starts a fresh window.

## Forced spectator

A player in a ship is judged **degraded** when a reading with at least 20
outcomes shows an average RTT of at least `quality_max_rtt_ms` (600 ms) or a
loss of at least `quality_max_loss_per_mille` (150, that is 15%). After 10
seconds of continuous degradation, inside the server tick:

1. The server moves the player to spectator through its ordinary roster
   transaction, marked as forced. It works while the player is dead or low on
   energy, and ends the old life cleanly.
2. It records a **hold** for the player's login identity, ending 60 s later,
   and sends the player a reliable `0x83` Force directive. Everyone receives
   the roster change.
3. The client shows "You've been moved to spectator mode for one minute" with
   the remaining time.

The `0x83` directive (44 bytes) carries the kind (Force, Hold or Clear), server
session, arena stream, player id, predecessor and successor life, hold id,
remaining milliseconds and, for a denial, the exact ship request it refuses.

**Returning.** The player returns by choosing a ship as usual. The server allows
it only when the hold has expired **and** the current session shows sustained
health: at least 20 outcomes with an average RTT below
`quality_recovery_rtt_ms` (400 ms) and loss below
`quality_recovery_loss_per_mille` (100, that is 10%), continuously for 10 s,
re-checked at the moment of the request.

- If not, the request is refused with a Hold directive naming that request, and
  the player stays a spectator with the remaining time shown. When the hold has
  already expired and only the health check failed, the client reconnects once
  to start a fresh measurement on a new connection and stays a spectator. A
  fresh connection needs about 22.5 s of probes before it can show sustained
  health (20 outcomes, then 10 s), so the client counts down 24 s from the new
  connection before asking the player to pick a ship again, and explains any
  refusal in the meantime; it gives up after 75 s.
- If so, the server removes the hold, sends a Clear directive and then the
  ordinary ship change. The new ship then goes through the normal baseline and
  receipt ([Networking](networking.md#reliable-messages-and-lifecycle-receipts)).
  Because a held spectator has no loadout, the client may submit the new
  ship's neutral baseline before its loadout arrives, but no input or weapons
  until the server's receipt.
- A successful return re-arms the policy: a later bad period can force the
  player out again.

**Holds survive reconnects.** Holds are keyed by the login identity, not the
connection, so closing the tab, rejoining or changing arena does not reset or
shorten them. A held identity joins as a spectator with a Hold directive in its
join bootstrap, and held spectators get a Hold refresh at most once per second.
Retries never extend the deadline.

**Limits.** The registry holds at most 4,096 identities (each at most 128
bytes), and a hold is only removed by a healthy return. Login admission counts
every connected identity without a hold as a possible future hold, so the
server can always record one when it needs to; once the registry is full, new
identities are refused at login rather than an unserved hold being evicted.
Hold ids are nonzero and never reused. Identities are not authenticated, so a
player can evade a hold by changing identity; this is a fairness mechanism, not
an account system.

A separate spectator move for players idling at spawn uses its own `0x8d`
packet and is unrelated to connection quality.

## Presentation delay

Under `observer_network_profile = "auto"` the server sets each player's remote
presentation delay from the 95th-percentile probe RTT:

| Half the p95 RTT plus 30 ms | Delay |
| --- | --- |
| up to 70 ms | 70 ms |
| up to 100 ms | 100 ms |
| up to 150 ms | 150 ms |
| above | 200 ms |

Loss of at least 2% raises the delay to at least 100 ms, and at least 6% to at
least 150 ms. A connection starts at 200 ms. An increase applies after the new
value has held for 0.5 s and a decrease after 10 s, except that the first
settling from the starting 200 ms also takes 0.5 s. Nothing changes while the
server is overloaded.

## Event copies

With `realtime_event_game_copy = "on-loss"`, a player gets game-channel event
copies unless their current reading has at least 20 outcomes and no loss at
all. A new connection therefore gets copies until its window fills, roughly the
first 13 seconds.

## Configuration

The four thresholds are in the `[server]` section of
`config/zubspace_server.toml`; the environment variables are
`ZUBSPACE_SERVER_QUALITY_` plus the upper-cased suffix (for example
`ZUBSPACE_SERVER_QUALITY_MAX_RTT_MS`).

| Key | Default | Allowed |
| --- | --- | --- |
| `quality_max_rtt_ms` | 600 | 100–2500 |
| `quality_max_loss_per_mille` | 150 | 2–1000 |
| `quality_recovery_rtt_ms` | 400 | positive, below `quality_max_rtt_ms` |
| `quality_recovery_loss_per_mille` | 100 | positive, below `quality_max_loss_per_mille` |

The probe interval, 30 s window, 20-outcome minimum, 10 s sustain times and
60 s hold are fixed.

Our prod and dev servers set `quality_max_rtt_ms = 400` and
`quality_recovery_rtt_ms = 300` (`deploy/<env>/zubspace_server.toml`).

## Observability

- **`/v1/status` → `connection_quality`**: current sessions, measured sessions,
  probes sent, echoes received, local cancellations, outcomes in windows, the
  worst session average RTT and loss, sessions currently degraded, sessions
  paused because their page is hidden (`page_hidden_sessions`), retained holds,
  held sessions, the three decision counters and the active thresholds.
- **Prometheus**: the same counts (not the thresholds) as
  `zubspace_connection_quality_*`. The per-session values are gauges over
  current sessions and drop when sessions leave; `forced_total`,
  `return_denied_total` and `healthy_return_total` are process-lifetime
  counters. No metric has player or session labels.
- **Logs**: one INFO `server connection quality window` line per session every
  10 s (samples, average RTT, loss, host health), with a DEBUG probe audit
  beside it. Decisions are logged as `connection quality decision` with
  `decision` set to `forced_spectator` (WARN), `return_denied` or
  `return_accepted` (INFO), or `hold_allocation_failed` (ERROR). Each
  visibility change is an INFO `page visibility changed` line (`hidden`,
  `measurement` paused or resumed); a voided hidden claim is a WARN
  `page reported hidden while sending gameplay; measurement resumed`.

See [Observability](observability.md) for the listener and log setup.

## Tests

Native tests in the files above cover the window, policy, holds and directive
ordering. The browser gates are `make test-e2e-connection-quality` (probe
measurement under an impaired profile), `make test-e2e-connection-quality-enforcement`
(sustained latency) and `make test-e2e-connection-quality-loss` (probe loss);
the last two run the full forced move, early denial, reconnect, hold and
healthy return. See [`AGENTS.md`](../AGENTS.md).
