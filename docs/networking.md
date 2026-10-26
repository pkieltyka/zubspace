# Networking

Zubspace runs multiplayer over WebRTC DataChannels between the browser and
`zubspace_server`. HTTP serves the page and assets, exchanges the WebRTC offer
and answer, and provides the API and optional diagnostic uploads. Live gameplay
uses the DataChannels; there is no WebSocket, WebTransport or raw UDP fallback.
Packets on the DataChannels are Subspace Continuum shapes where that is useful
(login, reliable wrappers, roster, chat, settings, map transfer) plus Zubspace
extensions for everything the authoritative server adds: input records,
snapshots, one-shot events and lifecycle receipts.

The overall picture:

- The browser runs its own ship at fixed 60 Hz steps and publishes the last
  step's position, rather than every catch-up step. Each position embeds the
  recent input records the server has not yet
  applied, so a lost datagram is repaired by the next one.
- The server simulates the arena (60 Hz by default) and owns all state. On its
  snapshot cadence it builds each player a personal view: the actors near them,
  delta-encoded against a frame the player is known to hold.
- Shots, hits and projectile states travel as separate, self-contained
  realtime events, batched per tick.
- Login, roster, chat, score, settings, zone files and the life/ship lifecycle
  use the Continuum reliable layer on an ordered channel.

What the server decides and how the client reconciles with it is in
[Server authority](authoritative_server.md); which formats follow Continuum is
in [Protocol compatibility](protocol_compatibility.md). This page covers the
transport and the wire.

## Connection flow

1. **Offer.** The browser creates one `RTCPeerConnection` with three
   DataChannels, waits for ICE gathering to finish (signaling is non-trickle)
   and POSTs the SDP offer as JSON to `/v1/rtc/offer`. The endpoint defaults
   to the page's own origin; the `rtc` query parameter overrides it. ICE
   gathering is bounded by `ice_gather_timeout_ms` (default 15 s) and the
   offer/answer exchange by `signaling_timeout_ms` (default 20 s); both accept
   1–60 s. A Bearer token (`rtc_token`) is only ever sent to the page's own
   origin.
2. **Answer.** The server checks origin, token, rate and size limits (see
   [Configuration](#configuration)), reserves a UDP port, gathers its own
   candidates (up to 10 s) and returns the answer. If the answer cannot be
   made, the response is `400 invalid_webrtc_offer`,
   `503 rtc_capacity_unavailable`,
   `503 rtc_service_unavailable` (both 503s carry `Retry-After: 1`) or
   `500 internal_server_error`.
3. **Channels.** The server accepts exactly the three channels below, with
   exactly those labels and reliability settings, announced in-band with an
   empty subprotocol. Anything else (a fourth channel, a duplicate, a
   negotiated channel, altered ordering or lifetime) is a protocol violation
   and closes the peer. A peer whose channels have not all opened within 20 s
   of the answer is dropped.
4. **Registration.** When all three channels are open the server allocates a
   player id and starts the session's writer.
5. **Login.** The client sends a Continuum password login (`0x24`) carrying the
   player name and the browser's stored numeric identity. The identity is not
   authenticated; the server only rejects invalid names and identities and
   those already in use.
   On acceptance (`0x0a`) the client sends the arena login (`0x01`), which has
   one extra byte at offset 27: the newest **client protocol revision** the
   build understands (currently 3). A login without it counts as revision 0.
   The server only uses compact snapshot forms the client has declared (see
   [Compact forms](#compact-forms)).
6. **Join bootstrap.** The server queues one reliable batch: the `0x72` clock
   bootstrap (server session id, a new `arena_stream_id` for this join, the
   current server tick, sequence bases, tick and snapshot rates), player id,
   join, roster, arena settings, the `0x29` map advertisement (`ZMAP`: zone id,
   each map/LVZ file's name, size, CRC and SHA-256), spawn coordinates and the
   arena directory.
7. **Snapshot fence.** Snapshots start only after the client has acknowledged
   every reliable frame of that bootstrap, so no realtime state can overtake
   it.
8. **Zone files.** The client reuses verified cached files (`0x7c`) or requests
   them one at a time (`0x0c` → `0x2a`, each at most 1 MiB). Local gameplay
   stays closed until the map is installed and the map security challenge has
   seeded the door clock. Details: [Zone file cache](zone-file-cache.md).

Snapshots, realtime events and quality probes carry the `arena_stream_id` from
step 6 (events and probes also carry the `server_session_id`), and the client
drops any that do not match its current join, so nothing from a previous
connection, arena or server process can be applied.

When an established peer reports `disconnected`, the browser waits up to 5 s for
it to recover before closing all channels. Server-side sessions move through
Handshaking → Registered → Closing → Closed; `Closed` is published only after
the player is removed and the outbound queue is closed.

## DataChannels

| Label | Settings | Client → server | Server → client |
| --- | --- | --- | --- |
| `zubspace-game` | unordered, `maxPacketLifeTime` 80 ms | positions (`0x03`), standalone input records (`0x62`), quality echoes (`0x82`) | snapshots (`0x70`), quality probes (`0x81`), optional copies of realtime events |
| `zubspace-events` | unordered, reliable | copies of weapon-bearing positions | realtime events (`0x7d`, batched as `0x84`) |
| `zubspace-reliable` | ordered, reliable | everything else, including snapshot ACKs and lifecycle messages | everything else |

The events channel has neither a lifetime nor a retransmit limit, so it is
fully reliable but not ordered: a lost event is retransmitted by SCTP without
holding up later ones.

The game channel's 80 ms lifetime limits stale DATA retransmission; it is not
an 80 ms RTT requirement or the connection-quality disconnect threshold.
Later snapshots and repeated input history repair missing replaceable state.
Weapon submissions have reliable copies because losing a one-shot action has
different consequences from missing an older position.

All three channels share an SCTP association over DTLS and the selected ICE
path, normally UDP. Their separate ordering and writer queues avoid
application-level lane blocking, but do not give them separate congestion
control or unlimited bandwidth. Connection-quality policy and the observer
delay are described in [Connection quality](connection-quality.md).

Routing rules worth knowing:

- A position that fires a weapon is sent with the same bytes on all three
  channels. The server consumes each weapon sequence at most once. Its
  replay window follows the admitted position stream, so after any length of
  time without firing the next shot still counts as new.
- The browser drops an ordinary (non-weapon) position or input record rather
  than queue it when the game channel already holds more than 2 KiB; the next
  position carries the same input history.
- Quality echoes never fall back to the reliable channel, so loss stays
  visible to the measurement ([Connection quality](connection-quality.md)).
- Client messages larger than 2 KiB are a protocol violation and disconnect the
  session.

## Clocks and identities

- **Server tick.** The simulation loop runs at `tick_rate` (default 60 Hz,
  range 1–240).
- **Protocol time.** Timestamps on the wire are Continuum ticks: 100 Hz
  (10 ms), 31-bit, from a process-monotonic clock. Snapshot headers and
  realtime events carry this tick.
- **Client steps.** The client simulates in fixed 1/60 s steps (at most 8
  catch-up steps per frame). Each step has a u16 input sequence.
- **Scopes.** `server_session_id` (per server process), `arena_stream_id` (per
  join) and `life_seq` (per ship life) bind every authority-bearing packet.
  Reusable numbers such as player ids and snapshot ids are never trusted on
  their own.

## Client → server: positions and input records

Each step that publishes a position sends one `0x03` packet:

| Part | Size | Content |
| --- | --- | --- |
| Continuum position prefix | 32 B | direction, position, velocity, status, bounty, energy, weapon, plus the Continuum extra fields |
| Raw state (`0x78`) | 23 B | the exact f32 x, y, vx, vy and orientation that the integer fields were derived from |
| Input history (`0x62`) | 4 B + 18 B per input | `base_seq:u16`, `count:u8`, then per input `bits:u16`, `direction:u8`, `door_seed:u8` and a 14-byte combat-view receipt |
| Optional markers | | `0x79` Repel application, `0x76` Warp application |

The combat-view receipt names the newest snapshot the client applied and the
server's presentation schedule it was drawing with; the server uses it to judge
hits against what the shooter saw, and also as a delta-base acknowledgement
(below). Input bits are defined in `src/gameplay/player_input.rs`.

The input history is a rolling window of up to 32 records. Every snapshot to a
player carries an `InputApplied` event: the last input of the current life the
server has applied with no gap. The client sends only the inputs after it, but
never fewer than 4. Under loss the applied point stops at the first gap, so the
window grows back towards 32 on its own. When a slow frame runs several steps
at once, only the last step publishes a position (its history covers the
others); a standalone `0x62` is sent only for a step that publishes no
position.

The server keeps 64 recent input records per player reconstructed from
overlapping packets, waits up to 12 ticks for a missing record, and replays
every record exactly before accepting a position. Admission, movement
validation and correction are described in
[Server authority](authoritative_server.md).

## Server → client: snapshots

Each tick the server builds one `0x70` frame per joined player
(`src/bin/zubspace_server/snapshot.rs`; wire codec shared with the client in
`src/protocol/snapshot.rs`):

```text
type:u8 = 0x70
flags:u8
snapshot_id:u16        per recipient, never 0
base_id:u16            acknowledged frame this delta is relative to
tick:u32               protocol tick (100 Hz, 31-bit)
arena_stream_id:u64
near_count:u8, mid_count:u8, event_count:u8
near_entities[], mid_entities[], events[]
```

| Flag | Meaning |
| --- | --- |
| `0x01` | FULL: no base, every field present |
| `0x02`, `0x04`, `0x08` | mandatory revisions (source sample time, motion segment, relocation generation); always set |
| `0x10` | one-byte sample times (client revision ≥ 1) |
| `0x20` | compact private state (client revision ≥ 2) |

An entity is `id:u16`, a 16-bit field mask and the selected fields. Near
entities carry position, velocity, direction, energy, status, bounty, ping,
flag timer, life, sample time and motion segment. Mid entities carry position
and life only. The weapon field is always zero in production; shots travel as
realtime events.

Snapshot events are `type:u8 | len:u8 | body`, so unknown types can be skipped:

| Type | Event | Notes |
| --- | --- | --- |
| `0x01` | WatchDamage | public hit report; also sent as a realtime event |
| `0x02` | RocketState | private, every frame |
| `0x03` | LifecycleState | private: life, alive, respawn deadline, pending Repel delivery |
| `0x04` | PrizeState | private: bounty, max energy, upgrades, inventory, timed effects, energy |
| `0x05` | ObserverTimeline | the server's presentation schedule for this viewer |
| `0x06` | InputApplied | contiguous applied input, for trimming the client's window |
| `0x07` | PrizeReference | compact stand-in for an unchanged PrizeState |

The private state repeats every frame, so a lost frame or a delayed reliable
packet is repaired by the next frame instead of a resend.

### Delta bases and acknowledgements

Server and client each keep the last 32 frames. The server encodes against the
newest frame the client is known to hold, and sends FULL when it has none.

- The client acknowledges every 5th frame (about 12 Hz) with a reliable `0x71`,
  and immediately for a frame carrying a Repel delivery. These ACKs also give
  the server its RTT samples.
- The combat-view receipt in every input record also advances the base: the
  client keeps every frame it decoded as a base, so the server can delta
  against a frame about one round trip old rather than the last `0x71`.
- If a delta arrives whose base the client no longer has, the client drops its
  entity state (keeping its self-contained events), does not acknowledge it,
  and sends a reliable `0x73` at most every 600 ms. The server clears the base
  and the next frame is FULL.

### Area of interest

`src/bin/zubspace_server/visibility.rs`:

- **Near** (within 2,048 px): full state in every frame, at the tick rate up to
  60 Hz.
- **Mid** (within 2,560 px): position and life every other frame (30 Hz).
- **Far:** nothing.
- A demoted actor must move 256 px past the radius before dropping a tier, and
  an approaching actor is projected 0.2 s ahead (at most 1,024 px) so it
  appears before it crosses the edge.
- The client keeps a remote ship for 12 snapshot ids after it last appeared, so
  one lost frame or a near-to-mid change does not make it blink.

### Send budget and holds

Snapshots and realtime events share a per-player token bucket of
`send_budget_kib_per_second` (default 64 KiB/s, bursting the same amount).
Each tick the queued events are paid for first; events that do not fit wait,
in order, for the next tick. Then the snapshot:

1. If the bucket holds the frame plus two ticks of refill, it goes out whole.
2. Otherwise the server shrinks it towards one tick's refill by **holding**
   some actors at their state in the base frame, so they cost only their id and
   an empty mask. Mid actors are held first, then near actors farthest from the
   player. An actor is held at most 2 frames in a row, and births, deaths and
   warps are never held.
3. Only if the held frame still does not fit is the frame skipped.

Separately, when the tick's work averages 60% of the tick budget the mid tier is
dropped (still refreshed every 6th frame), and at 80% with four consecutive
overruns whole frames are skipped.

### Compact forms

Each depends on the client's declared protocol revision:

- **One-byte sample times** (revision 1): an actor's sample time is encoded as
  `ticks_behind × 3 + third` relative to the header tick, for samples up to 84
  ticks behind; `0xff` escapes to the full 5-byte form. Saves 4 B per moving
  actor per frame.
- **Compact private state** (revision 2): a LifecycleState without a pending
  Repel delivery is 13 bytes instead of 42. A PrizeState equal to the one in the
  acknowledged base (apart from energy) is sent as a 4-byte reference to the
  frame that carried it plus the current energy, and the client decoder expands
  it back. The full body is resent at least every 60 frames.
- **Position offsets** (revision 3): in delta frames, an actor's x and y may be
  signed bytes relative to its state in the base (entity field bit 14) instead
  of absolute u16s.

### Remote presentation

Remote ships are drawn behind real time, never extrapolated. Once the server's
ObserverTimeline schedule is installed, the client renders at the server's
chosen render tick (server tick minus a presentation delay) by interpolating
between the exact sample times in the snapshots. Before that, it falls back to
an adaptive Hermite interpolator with a 60–280 ms buffer. Under the default
`observer_network_profile = "auto"`, the server picks a delay of 70–200 ms for
each viewer from its probe measurements
([Connection quality](connection-quality.md)).

This differs from predicting the local ship and simulating weapon visuals.
[Physics and presentation](physics.md#three-presentation-paths) explains the
three paths and how fresh weapon draws converge without changing lifetime or
the server's damage clock.

## Realtime events

One-shot facts travel outside the snapshot so they cannot be overwritten by
newer state (`src/protocol/realtime_event.rs`). A `0x7d` event has a 27-byte
header:

```text
type:u8 = 0x7d | version:u8 = 2 | kind:u8 |
server_session_id:u64 | arena_stream_id:u64 | event_id:u32 | server_tick:u32
```

| Kind | Payload |
| --- | --- |
| 1 Weapon | source player/session/life, shot group, exact f32 pose and velocity, frequency, ship, weapon word |
| 2 WatchDamage | victim and shooter identities, weapon, damage, energy before and after, projectile id, damage sequence, lethal flag |
| 3 Repel mutations | new state for up to 16 projectiles or decoys pushed by a Repel |
| 4 Projectile state | the weapon source plus projectile id, child index, current position and velocity, expiry, burst state, bounces left, optional proximity-sensor target (proximity bombs and Thors) |
| 5 Owner weapons cancelled | cancels a player's mines/decoys up to a shot group |

Every payload is self-contained: it can be drawn without consulting the
current roster, so it stays correct after the shooter dies, leaves or has its
player id reused.

Events are addressed per recipient, filtered by the recipient's area of
interest, with that recipient's own nonzero event ids. Up to 3,072 events can
wait per recipient. The client authenticates the session and arena stream,
then deduplicates within a 4,096-id window, so the same event arriving on two
channels, late or out of order, applies once.

Both ends validate a projectile state the same way
(`RealtimeProjectileStateEvent::invalid_field` names the failing check). The
server checks each one before sending: a state that would fail is withheld with
a WARN, and it still opens the recipient's presentation clock as if it had been
sent, so a pending hit is never held back. On the game and events channels the
client skips a `0x7d` it cannot decode and counts it (`realtime_event_rejects`,
with a WARN at the first and each power of two), so one bad event cannot fail
the frame's poll and delay the packets behind it. The reliable channel still
fails closed.

**Batching (`0x84`).** A recipient's events from one tick leave as one message:

```text
type:u8 = 0x84 | version:u8 = 1 | count:u8 | server_session_id:u64 | arena_stream_id:u64
then per event: len:u16 | the 0x7d packet without its 16 id bytes
```

A batch is at most 1,100 bytes (below one SCTP packet's payload, so it never
spans datagrams) and 255 events; a run of one event goes out as a plain `0x7d`.
The browser rebuilds the exact `0x7d` packets in JavaScript before the game
sees them.

**Game-channel copy.** Events always go on the reliable events channel. A
second copy on the game channel arrives about one round trip sooner when the
datagram carrying the first is lost, at the cost of doubling event bytes.
`realtime_event_game_copy` selects `always` (the compiled default), `on-loss`
(only while the player's quality probes show loss in their 30 s window or have
fewer than 20 samples) or `never`.

## Reliable messages and lifecycle receipts

The ordered channel carries the Continuum reliable layer: `00 03` wrappers with
a 32-bit id, `00 04` ACKs, small and huge chunks, and clusters, with 520-byte
packets. SCTP already delivers that channel reliably, but the layer is kept
because it is the Continuum format and because its ACKs are what the server
and client wait on. A frame is resent after 3 s without an ACK. A receiver
only acknowledges a frame it has room to keep in its reorder queue, so a full
queue causes a resend rather than a gap.

Changes that rebase a ship's movement (ship change, respawn, frequency change,
Warp, Repel, server corrections and some prize effects) use application
barriers: the client sends an exact request, baseline or marker, and the server
answers with a receipt naming the session, life and input sequence it applied
to. The client treats the change as done only on that exact receipt; a
transport ACK is never enough. Receipts that retire one exact pending request
are dispatched as soon as they arrive, even past an unrelated gap in the
reliable sequence.

| Client sends | Server answers |
| --- | --- |
| neutral baseline position after a ship change or respawn (reliable `0x03`) | `0x75` baseline accepted, with the commit tick |
| frequency change (`0x0f`) with a baseline | `0x74` cutover ACK with the server's exact baseline, then `0x75` |
| ship change (`0x18`) | roster change, or `0x7f` rejected |
| position with a `0x76` Warp marker | `0x77` warp accepted |
| position with a `0x79` Repel marker | `0x7a` Repel application accepted |
| `0x63` correction applied, after a `0x7b` correction proposal | `0x7c` correction terminal |
| `0x64` prize effect applied, when a `0x61` prize ACK asks for it | `0x76` prize effect terminal |
| `0x65` Portal placement / `0x6a` Portal warp | `0x66` / `0x6b` terminal |

The client keeps at most 256 tracked reliable requests outstanding; a request
that cannot be tracked waits instead of being sent untracked. What each
transition means for gameplay is in [Server authority](authoritative_server.md).

Other Zubspace packets on the reliable channel: from the server, `0x60` hidden
prize spawn, `0x6c` QA trace context, `0x83` connection-quality directive and
`0x8d` spawn-idle spectator; from the client, the `0x80` player experience
report ([Observability](observability.md)) and the `0x85` page visibility
notice ([Connection quality](connection-quality.md#measurement-window)).

## Server RTC internals

All of this lives in `src/bin/zubspace_server/rtc.rs` and `rtc/writer.rs`.

- **Ports.** Each peer gets its own UDP socket: an ephemeral port by default,
  or one from `rtc_udp_port_range`, which must hold at least `max_players`
  ports. Once `max_players` sessions are open or handshaking (fewer if ports
  have failed to bind), new offers get `503 rtc_capacity_unavailable` before
  any ICE work.
- **Inbound.** Each channel feeds a 4,096-packet, 256 KiB application queue. A
  full queue disconnects the session rather than silently losing reliable
  input. Quality echoes are timestamped on the channel callback, before this
  queue.
- **Outbound.** Each session has one mailbox per channel, sharing a 4,096-packet,
  4 MiB limit. The snapshot slot holds only the newest unsent snapshot, so a
  stalled channel sends current state rather than a backlog. Everything else is
  a FIFO that is never dropped; overflowing it disconnects the session. A
  waiting snapshot gets a turn after at most 8 FIFO packets. Each channel's
  SCTP send buffer is 256 KiB, and each mailbox waits for its own channel's
  capacity. A blocked application send on the ordered channel therefore does
  not occupy the game channel's writer; transport congestion remains shared.

`/v1/status` and the Prometheus listener expose queue depths, coalescing,
overflows, protocol violations and UDP datagram counts
([Observability](observability.md)).

## Vendored WebRTC and SCTP

Zubspace builds against vendored copies of the `webrtc`, `rtc` and `rtc-sctp`
0.21.1 crates under `vendor/`. Each has a `ZUBSPACE.patch` against the
published crate and a `ZUBSPACE-PATCH.md` explaining it.
[Vendoring](vendoring.md) describes the upgrade procedure and the separate
test commands. `make test-vendor-rtc` runs the `rtc` library suite, including
real SCTP reply-stream reliability and expiry checks.

- **`rtc`**: the DataChannel open handshake (DCEP) keeps the remote channel's
  ordering and lifetime or retransmit limit, and applies an accepted remote
  open to the actual SCTP reply stream. Without this, a browser-opened channel
  would be answered with ordered, reliable defaults and the server could not
  check the channel contract. The SCTP packet size is set through upstream's
  `with_sctp_mtu`.
- **`rtc-sctp`**: a pending SACK rides in the next outgoing DATA packet instead
  of its own datagram. An opt-in `AckMode::DeferToData` lets a SACK made due by
  a second packet wait up to 20 ms for that DATA (`rtc_defer_sack_to_data`).
  The default SCTP packet budget is 1,191 bytes, safe for an IPv6/TURN
  minimum-MTU path.
- **`webrtc`**: opt-in write coalescing (`set_write_coalescing`): after a write
  wakes the driver it waits briefly before flushing, so one tick's snapshot,
  event batch and ACKs on different channels share datagrams
  (`rtc_write_coalesce_micros`). It also keeps wildcard UDP binding and adds
  packet sizes to UDP write errors.

## Configuration

Server keys live in `config/zubspace_server.toml`, parsed by
`src/bin/zubspace_server/config.rs`. Each can also be set in the environment as
`ZUBSPACE_SERVER_` plus the upper-cased key, except `rtc_udp_port_range`
(`ZUBSPACE_SERVER_UDP_PORT_RANGE`), `rtc_external_ips`
(`ZUBSPACE_SERVER_EXTERNAL_IP`) and `signaling_offer_rate_per_minute`
(`ZUBSPACE_SERVER_SIGNALING_OFFER_RATE`); several also have command-line flags.
Deployment values are in `deploy/<env>/zubspace_server.toml` and
[Deployment](deployment.md).

| Key | Default | Meaning |
| --- | --- | --- |
| `tick_rate` | 60 | simulation rate, 1–240 Hz |
| `max_players` | 500 | arena limit, at most 768 (the player id range below bot ids) |
| `send_budget_kib_per_second` | 64 | per-player snapshot and event budget, 16–4096 |
| `realtime_event_game_copy` | `always` | `always`, `on-loss` or `never` |
| `rtc_defer_sack_to_data` | true | SACKs wait up to 20 ms for outgoing DATA |
| `rtc_write_coalesce_micros` | 0 | driver wait before flushing writes, 0–5000 µs |
| `rtc_udp_port_range` | `ephemeral` | `min-max` UDP range for peers |
| `rtc_udp_bind_ip` | wildcard | one local IPv4 address for peer sockets and host candidates |
| `rtc_udp_path_mtu` | 0 (unset) | measured IPv4 path MTU (≥ 1280); sets the SCTP packet size to MTU − 80, at most 1,228 |
| `rtc_ice_urls`, `rtc_ice_username`, `rtc_ice_credential`, `rtc_ice_policy` | empty, `all` | server STUN/TURN; `relay` requires TURN |
| `rtc_external_ips` | empty | rejected at startup: webrtc-rs does not apply it |
| `signaling_allowed_origins`, `signaling_auth_token`, `signaling_require_https`, `signaling_trust_proxy_headers` | `*`, none, false, false | signaling admission; HTTPS enforcement needs trusted proxy headers |
| `signaling_offer_rate_per_minute`, `signaling_max_offer_bytes` | 240, 131072 | per-client offer rate and offer size |
| `observer_network_profile` | `auto` | remote presentation delay policy |
| `quality_*` | | connection-quality thresholds ([Connection quality](connection-quality.md)) |
| `tx_loss_pct` | 0 | test-only seeded loss of outbound snapshots and reliable frames |

Browser query parameters: `rtc` (offer URL), `rtc_token`, `ice`,
`ice_username`, `ice_credential`, `ice_policy`, `ice_gather_timeout_ms` and
`signaling_timeout_ms`. ICE URLs on both sides must be canonical
`stun:host:port` or `turn:host:port?transport=udp` with an IPv4 address or
lowercase DNS name.

Both public environments run with `realtime_event_game_copy = "on-loss"`,
`send_budget_kib_per_second = 96` and `rtc_write_coalesce_micros = 500`.

## Bandwidth

These changes reduce bytes and datagrams per player without lowering the
configured simulation and snapshot cadence or changing prediction and server
authority.

The goal is fresher state with less framing and retransmission work, while
keeping one-shot facts and authoritative application receipts intact. The
optimizations act at different layers; a smaller application message, fewer
DataChannel sends and fewer UDP datagrams are separate measurements. The main
changes are:

- **Trimmed input window.** Positions carry only inputs the server has not
  applied (at least 4) instead of always 32. A position went from about 635 B to
  about 130 B, and the browser's game-channel send queue peak fell from about
  940 B to under 200 B, well inside the 2 KiB drop threshold.
- **Event batching and the loss-only game copy.** About three events per
  message, and no second copy on a clean link.
- **SACKs with DATA, deferred SACKs and write coalescing.** Fewer, fuller
  datagrams in both directions (fewer server datagrams also means fewer
  browser SACKs).
- **Delta bases from input receipts.** Bases about one round trip old, so
  smaller deltas.
- **Compact sample times, private state and position offsets.** 4 B per moving
  actor for sample times; 29 B per frame for the short lifecycle form and 59 B
  for a prize reference; up to 4 B per moving actor for position offsets.
- **Holds instead of skipped frames.** Under a tight budget, distant actors
  update less often instead of whole frames disappearing.

Measured on the dev server with 48 bots crowding one player (the dense case),
per player, bytes above UDP (including DTLS and SCTP framing):

| | Before | After |
| --- | --- | --- |
| Upload | 47.2 KB/s | 15.0 KB/s (−68%) |
| Download | 76.4 KB/s | 49.0 KB/s (−36%) |
| Server datagrams sent | 160/s | 78/s (−51%) |
| Snapshots skipped by the budget | 29 per 2 min | 0 |

"After" uses the public environments' settings above. Crowding varies between
runs, so treat individual figures as indicative. Write coalescing added about
0.2 ms to the snapshot ACK round trip. Upload is now mostly framing: roughly
120 datagrams/s at about 70 B of DTLS/SCTP overhead each.

To measure a deployment, `node tools/remote_session_probe.mjs <site-url>
[seconds]` plays a scripted session in headless Chrome and prints the
browser's WebRTC byte, datagram and per-channel message counts. Server-side
counterparts (`zubspace_rtc_udp_datagrams_sent_total`,
`zubspace_snapshot_delta_bytes`, `zubspace_snapshot_send_skipped_total`,
`zubspace_snapshot_entities_held_total`,
`zubspace_realtime_event_game_copies_total`) are listed in
[Observability](observability.md).

## Tests

Gates are listed in [`AGENTS.md`](../AGENTS.md). The ones aimed at this layer:
`make test-rtc-config` (browser and server RTC configuration, timeouts),
`make test-webrtc-channel-contract` (the three-channel contract in Chrome and
Firefox), `make test-webrtc-capacity` (many real concurrent peers),
`make test-vendor-rtc`, `make test-e2e-snapshot-loss` (seeded snapshot loss and
recovery) and `make test-e2e-input-recovery` (input omission and reordering).
