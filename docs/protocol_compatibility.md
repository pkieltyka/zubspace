# Protocol Compatibility

Zubspace speaks a protocol descended from Subspace Continuum's. Every message
starts with a one-byte type (core transport messages start with `0x00` and a
second type byte), integers are little-endian, and many messages keep their
Continuum ids and byte layouts. Maps, LVZ archives, bitmaps, sounds and arena
settings use the original file formats unchanged.

A Continuum client or zone server cannot talk to Zubspace, though. Browsers
cannot open UDP sockets, so the transport is three WebRTC DataChannels, and
Continuum's encryption handshake is replaced by WebRTC's DTLS. The server
(`src/bin/zubspace_server*`) is Zubspace's own authoritative server, not ASSS
or subgame. Several Continuum packets carry Zubspace extensions, and live game
state travels in Zubspace-only packets: per-recipient snapshots (`0x70`) and
authenticated realtime events (`0x7d`). Original layouts are kept where they
still fit, so the original assets load unchanged and the reference game
client (see [AGENTS.md](../AGENTS.md)) stays a useful oracle.

This page is the wire reference. Behaviour is described in
[architecture.md](architecture.md), [networking.md](networking.md) (channels,
snapshot delivery, AOI, events), [authoritative_server.md](authoritative_server.md),
[connection-quality.md](connection-quality.md) (probes),
[zone-file-cache.md](zone-file-cache.md) and [observability.md](observability.md)
(player report).

## Summary

| Area | Follows Continuum | Zubspace-local |
| --- | --- | --- |
| Assets | `.lvl`, `.lvz`, `.bm2`, `.wa2` | Bounded decode budgets (`src/zone_wire.rs`) |
| Arena settings | 1,428-byte settings packet (`0x0f`) and its checksum | Authored as `arena.conf` text, compiled by the server |
| Encodings | 100 Hz ticks, pixel coordinates, 40 direction bins, `weapon_data` | Raw f32 movement state next to the integer fields |
| Transport framing | Reliable, ACK, chunk and cluster core packets; 520-byte limit | Three DataChannels; `0x7e` file fragments |
| Login and arena entry | `0x24` login, `0x0a`, `0x01` arena login, roster, settings, map info | Identity in the login, revision byte, `0x72` bootstrap, authenticated map info |
| Player state | Client position `0x03` 32-byte prefix | Mandatory position tail and `0x62` input history; `0x70` snapshots instead of position relays |
| Combat | `weapon_data`, death and prize packet prefixes | `0x7d` events, server-owned damage and deaths, receipts |

## What follows Continuum

### Assets

- `.lvl` maps: an optional embedded `BM` bitmap tileset, then 4-byte tile
  records (x in bits 0-11, y in bits 12-23, tile id in bits 24-31) on a
  1024 x 1024 tile map.
- `.lvz` archives: `CONT` sections holding zlib-compressed files and object
  sections tagged `CLV1` or `CLV2` (`src/lvz/archive.rs`).
- `.bm2` bitmaps (`src/bm2.rs`) and `.wa2` sounds (`src/assets/load/sounds.rs`).

Shipped zones live under `assets/zones/<zone>/`. The size and count limits in
`src/zone_wire.rs` are Zubspace hardening, not part of any format.

### Arena settings

A zone authors its settings as `arena.conf`, an ASSS-style `[Section]
Key=Value` file (see [chaos_zone_settings.md](chaos_zone_settings.md)). The
server compiles it into Continuum's 1,428-byte arena-settings packet (a 4-byte
header, eight 144-byte ship blocks, then the arena-wide sections)
(`src/bin/zubspace_server/arena_settings.rs`, `src/settings.rs`), and sends it
as `0x0f` at join; the browser applies it. The browser also loads `arena.conf`
at startup for its local tuning.

### Checksums

`src/continuum.rs` holds the one implementation both binaries use: the arena
settings checksum (sum of the packet's u32 words XOR a key), the map tile CRC,
the reflected CRC32 of files and the XOR checksum byte of a client position.
The server's `0x18` security challenge (17 bytes: prize seed, door seed,
timestamp, checksum key) asks for these, and the client answers with `0x1a`.
The challenge's door seed and timestamp also anchor the door timing that
client and server simulate.

### Units and encodings

- Time is Continuum's 100 Hz tick, masked to 31 bits; comparisons wrap
  (`continuum_tick_diff`). The server counts ticks from process start.
- Positions are u16 pixels (16-pixel tiles), velocities are i16 in tenths of
  a pixel per second, and direction is a byte from 0 to 39. Zubspace's own
  orientation is a fraction of a turn from 0 to 1 (`src/continuum_position.rs`
  converts).
- `weapon_data` is the Continuum u16 bitfield: type in bits 0-4, level minus
  one in bits 5-6, bouncing shrapnel in bit 7, shrapnel level in bits 8-9,
  shrapnel count in bits 10-14 and the alternate (mine) flag in bit 15
  (`src/continuum_weapon.rs`).
- Fixed-width strings are NUL-padded.

### Core transport packets

Core packets start with `0x00`: reliable message `00 03 | id:u32 | payload`,
ACK `00 04 | id:u32`, time sync request and response `00 05` and `00 06`,
disconnect `00 07` (accepted only on the reliable lane), small chunk body and
tail `00 08` and `00 09`, and cluster `00 0e | (len:u8 | packet)*`, which is
processed recursively. The client also parses huge chunks (`00 0a` to
`00 0c`); the server never sends them.

Reliable framing follows the reference game (`src/protocol/reliable.rs` in the client,
`src/bin/zubspace_server/reliable.rs` in the server): 520-byte packets, a
6-byte reliable header, ACK clusters of at most 74 and a 256-entry inbound
out-of-order queue. The differences change no packet shape:

- Reliable ids are compared with a wrap-safe half-window; the reference game's
  `id < next` breaks at the `u32::MAX` wrap.
- The client never drops ACKs, where the reference game discards those past 256.
- A new out-of-order message that does not fit the inbound queue is not
  ACKed, so the peer resends it after the gap drains.
- The client resends after 300 ticks; the server resends after 3 seconds and
  holds up to 4,096 unacknowledged messages per client.
- Map and LVZ transfers use `0x7e` fragments instead of the small chunk stream
  (see [Zone file transfer](#zone-file-transfer)).

## What is Zubspace-local

### Transport

The browser opens three DataChannels (`index.html`; the server checks them in
`src/bin/zubspace_server/rtc.rs`):

| Label | Delivery | Carries |
| --- | --- | --- |
| `zubspace-game` | unordered, `maxPacketLifeTime` 80 ms | C2S `0x03`, `0x62`, `0x82`; S2C `0x70`, `0x81` and, by policy, copies of `0x7d`/`0x84` |
| `zubspace-events` | unordered, reliable | S2C `0x7d` and `0x84` |
| `zubspace-reliable` | ordered, reliable | Everything else, mostly wrapped in Continuum reliable messages |

A client position that carries a weapon is mirrored on all three channels.
Whether events are also copied to the game channel is the server's
`realtime_event_game_copy` setting (`always`, `on-loss` or `never`);
[networking.md](networking.md) explains it. The reliable lane keeps the
Continuum framing even though the DataChannel is already reliable, because the
reliable ids identify exact deliveries: the lifecycle receipt `0x75`, for
example, names the reliable id of the message it answers.

### Identity and scope

- **Player identity.** The browser keeps a random 12-digit id in
  `localStorage` and sends it in the `0x24` login: the low 32 bits in the
  machine-id field (offset 66) and the high bits in the remote-IP field
  (offset 89). There are no accounts; the password field is ignored. The
  server rejects a malformed id or one already connected.
- **`server_session_id` (u64)** is chosen when the server process starts.
- **`arena_stream_id` (u64)** is allocated for each arena join and never
  reused. It arrives in `0x72` and is repeated in every snapshot header, event
  and scoped private packet; the client drops anything from another join.
- **`life_seq` (u32)** counts an actor's lives. It wraps from `u32::MAX` to 1;
  zero means absent.
- **Source session id (u64)** is the server's never-reused per-connection id.
  Realtime events use it to name a weapon's owner across numeric player-id
  reuse.
- **`event_id` (u32)** and **`snapshot_id` (u16, never 0)** are allocated per
  recipient.

Time sync keeps its Continuum shape: the client's request carries a nonce in
the old timestamp slot, and the 26-byte response adds `server_session_id`
and `arena_stream_id`. The browser counts a sample only when both match its
current join.

### Client position (`0x03`)

```text
off  field                  notes
0    type = 0x03
1    direction:u8           0..39
2    timestamp:u32          must be 0; the server rejects any other value
6    velocity_x:i16
8    y:u16
10   checksum:u8            Continuum XOR checksum, over the whole packet
11   togglables:u8          status bits
12   x:u16
14   velocity_y:i16
16   bounty:u16
18   energy:u16
20   weapon:u16             weapon_data
22   energy:u16             Continuum extra data
24   repel_snapshot_id:u16  Continuum s2c latency; set only with a 0x79 marker
26   0x5250:u16             Continuum flag timer; set only with a 0x79 marker
28   items:u32              0x5a5a0000 | position_sequence:u16
32   0x78 | 1 | 20 | x_bits:u32 | y_bits:u32 | vx_bits:u32 | vy_bits:u32 | orientation_bits:u32
55   optional 0x62 input record (same layout as the standalone packet)
..   optional 0x79 | 3 | snapshot_id:u16 | application_sequence:u16 | delivery_generation:u32
..   optional 0x76 | collectible_id:u32
```

The `0x78` block is the exact f32 state the browser simulated; it must be
finite and must round to the integer fields. The embedded `0x62` record lets
the server validate the position together with the inputs that produced it,
even though the game channel is unordered; its last record must match the
position's sequence and direction. `0x79` marks the input after which the
client applied a Repel and is answered by `0x7a`. `0x76` marks a positive Warp
prize relocation and is answered by `0x77`. Nothing may follow the last marker.
The parser is `parse_client_position_snapshot` in `src/continuum_position.rs`.

### Input record (`0x62`)

```text
0x62 | base_seq:u16 | count:u8 (1..=32)
     | count x ( bits:u16 | direction:u8 | door_seed:u8 | receipt:14 bytes )
receipt = version:u8 | flags:u8 | observer_life_seq:u32 | input_sequence:u16
        | timeline_generation:u32 | latest_game_applied_snapshot_id:u16
```

Each record is one fixed 1/60-second gameplay step. Input bits 0-4 are thrust,
reverse, rotate left, rotate right and afterburner; bit 5 is a one-record
Rocket activation pulse and bit 6 a safe-zone weapon stop. Firing travels in
the position's `weapon` field, not here. A record repeats in later packets
until the server reports it applied (snapshot event `0x06`), with at least the
last four always resent (`src/protocol/client_input_record.rs`).

### Snapshots (`0x70`)

Snapshots replace Continuum's position relays (`0x05`, `0x28`, `0x39`,
`0x3a`); the client still parses those, but the server never sends them. The
codec in `src/protocol/snapshot.rs` is compiled into both binaries.
[networking.md](networking.md) covers rates, AOI tiers and delta bases.

```text
0x70 | flags:u8 | snapshot_id:u16 | base_id:u16 | tick:u32 | arena_stream_id:u64
     | near_count:u8 | mid_count:u8 | event_count:u8          (21-byte header)
     | near entries | mid entries | events
entry = entity_id:u16 | field_mask:u16 | the fields the mask selects, in bit order
event = event_type:u8 | body_len:u8 | body
```

Header flags: `0x01` marks a full frame (its `base_id` is ignored); `0x02`,
`0x04` and `0x08` are mandatory format revisions (sample time, motion segment,
relocation generation) that are always set; `0x10` selects compact sample
times and `0x20` compact private state. A frame with any other bit, or
without the mandatory three, is rejected.

```text
mask    field                 width
0x0001  x                     u16, or i8 offset from the base with 0x4000
0x0002  y                     u16, or i8 offset from the base with 0x4000
0x0004  velocity_x            i16
0x0008  velocity_y            i16
0x0010  direction             u8
0x0020  energy                u16
0x0040  weapon                u16
0x0080  status                u8
0x0100  bounty                u16
0x0200  ping                  u8
0x0400  flag timer            u16
0x0800  life_seq              u32
0x1000  sample time           tick:u32 | third:u8; in compact frames one byte,
                              ticks_behind * 3 + third, or 0xff then the 5 bytes
0x2000  motion generations    motion_segment:u32 | relocation_generation:u32
0x4000  positions are offsets from the acknowledged base (delta frames only)
```

Mid-tier entries carry only x, y and `life_seq`.

| Event | Body | Content |
| --- | --- | --- |
| `0x01` | 33 | WATCH_DAMAGE (also sent as a `0x7d` kind 2 event) |
| `0x02` | 16 | Private Rocket state |
| `0x03` | 42, or 13 in compact frames without a Repel delivery | Private lifecycle state: alive, respawn deadline, Repel delivery |
| `0x04` | 63 | Private prize and loadout state |
| `0x05` | 23 | Observer timeline directive |
| `0x06` | 6 | `life_seq:u32 \| through_sequence:u16`: inputs applied so far |
| `0x07` | 4 | `snapshot_id:u16 \| energy:u16`: reuse of an earlier prize body (compact frames only) |

The client acknowledges with `0x71 | snapshot_id:u16` inside a reliable
message, every fifth snapshot and at once for a frame that delivers a Repel. A
delta whose base it no longer holds is dropped, and the client asks for a full
frame with the one-byte `0x73`.

### Realtime events (`0x7d`) and batches (`0x84`)

Events are one-shot facts kept out of the snapshot state
(`src/protocol/realtime_event.rs`):

```text
0x7d | version=2 | kind:u8 | server_session_id:u64 | arena_stream_id:u64
     | event_id:u32 | server_tick:u32                         (27-byte header)
```

| Kind | Size | Fact |
| --- | --- | --- |
| 1 | 74 | Weapon fired: source player, session and life, `shot_group_id:u64`, raw f32 position, velocity and orientation, frequency, ship, `weapon_data` |
| 2 | 72 | WATCH_DAMAGE: victim and shooter (player, session, life), weapon, damage, energy before and after, projectile id, damage sequence, lethal |
| 3 | 28 + 45 per state (1-16) | Projectile and Decoy states after a Repel; entity kind 1 projectile, 2 Decoy, bit `0x80` held |
| 4 | 122, or 124 | One projectile or Decoy state. Flags: `0x01` expiry, `0x02` proximity sensor, `0x04` terminal, `0x08` appended source input sequence (the 124-byte form), `0x10` held in place |
| 5 | 52 | Owner weapon cancellation: silently removes one source session's weapons and Decoys through a given shot group, optionally bounded by an owner input sequence |

A departure's kind 5 travels inside an ordered reliable message so it reaches
observers before the player leaves; every other event goes on the events
channel. The client admits an event id once within a 4,096-id window and
drops events from another join.

When the server has two or more events for one recipient in a tick, it sends
them as one `0x84` batch of at most 1,100 bytes:

```text
0x84 | version=1 | count:u8 (2..=255) | server_session_id:u64 | arena_stream_id:u64
     | count x ( len:u16 | the 0x7d packet without its 16 id bytes )
```

`zubspaceExpandRealtimeEventBatch` in `js/zubspace_rtc_config.js` rebuilds
the exact `0x7d` packets before WASM sees them.
`tests/fixtures/realtime_event_batch.json` is shared by the Rust and
JavaScript tests.

### Zone file transfer

1. At join the server sends `0x29` in its authenticated form:
   `0x29 | "ZMAP" | version=1 | zone_id[16] | map filename[16] | map CRC32:u32
   | compressed size:u32 | map SHA-256[32] | file_count:u16` (the map plus its
   LVZ archives), then one `filename[16] | CRC32:u32 | size:u32 |
   SHA-256[32]` entry per LVZ.
2. The client requests each file with the reference game's indexed form `0x0c |
   index:u16` (index 0 is the map), or reports that it already holds the file
   with `0x7c`: version 1 (50 bytes) for the map, version 2 (84 bytes) for an
   LVZ, the latter only once the server has sent the `ZCAP` trailer. See
   [zone-file-cache.md](zone-file-cache.md).
3. The server sends the file as a Continuum `0x2a` packet (zlib-compressed
   `.lvl`, or the LVZ archive as is), split into `0x7e` fragments:
   `0x7e | version=1 | file_index:u16 | total_len:u32 | offset:u32 | data`.
   Each fragment fits one reliable message (at most 502 data bytes), and up
   to 64 are in flight. Unlike Continuum's small chunk stream, other reliable
   messages can travel between fragments.
4. The client checks the CRC32 and SHA-256 before it uses a file. Once the map
   is resolved, sent or held, the server sends the `0x18` security challenge.

`0x0d` (news request) is answered with Continuum `0x10` carrying `news.txt`.

### Lifecycle and authority transactions

Wherever a client action crosses the server's authority, Zubspace adds
request, receipt and terminal packets; [authoritative_server.md](authoritative_server.md)
describes them.

| Action | Packets |
| --- | --- |
| New life | Opened by S2C `0x72` (join), `0x1d` (ship change from C2S `0x18`; `0x7f` refuses), `0x74` (frequency change from C2S `0x0f`) or `0x06` (death, with the respawn deadline); the client then sends a reliable baseline `0x03`, and S2C `0x75` closes it |
| Movement correction | S2C `0x7b` proposal, C2S `0x63` application, S2C `0x7c` terminal |
| Prize pickup | C2S `0x07`; S2C `0x61` to the collector, `0x08` to the others |
| Shutdown prize | C2S `0x64` application; S2C `0x76` terminal |
| Positive Warp prize | `0x76` position marker; S2C `0x77` |
| Hostile Repel | `0x79` position marker; S2C `0x7a` |
| Portal placement | C2S `0x65`; S2C `0x66` |
| Manual Warp | C2S `0x6a`; S2C `0x6b` |
| Flags and balls | C2S `0x67`; S2C `0x68`, `0x69` |

Most of these carry the session, arena stream, player and life they belong to,
and the client ignores any that do not match its current join.

## Packet reference

Sizes are total bytes, including the type byte. "Ext" marks a Continuum
packet that Zubspace extends, by appending fields or by reusing a slot it does
not need, such as a client timestamp; "ZS" marks a Zubspace-only packet. Paths
are under `src/`; `server/` means `src/bin/zubspace_server/`.

### Client to server

| Id | Origin | Size | Purpose | Source |
| --- | --- | --- | --- | --- |
| `00 xx` | Continuum | | Core: reliable, ACK, time sync, disconnect, small chunk, cluster | `protocol/reliable.rs`, `protocol/login.rs` |
| `0x01` | Ext | 28 | Arena login; Continuum's 27 bytes plus the client protocol revision | `protocol/login.rs` |
| `0x02` | Continuum | 1 | Leave arena | `protocol/client_requests.rs` |
| `0x03` | Ext | 55+ | Position: the 32-byte Continuum form plus a mandatory tail | `protocol/client_position.rs`, `continuum_position.rs` |
| `0x05` | Continuum | 5 | Death report; the server rejects it | `server/connection.rs` |
| `0x06` | Continuum | variable | Chat | `protocol/chat_packets.rs` |
| `0x07` | Ext | 11 | Take prize; the timestamp slot holds the server-issued collectible id | `protocol/client_requests.rs` |
| `0x08` | Ext | 3, 34, 36 | Spectate, or a version 1 or 2 scoped camera view | `protocol/client_requests.rs`, `spectator_view.rs` |
| `0x0c` | Continuum | 3 | File request by index (0 is the map) | `protocol/files.rs` |
| `0x0d` | Continuum | 1 | News request | `protocol/client_requests.rs` |
| `0x0f` | Ext | variable | Frequency change; adds `position_sequence_base:u16` and a complete pre-change `0x03` position | `protocol/client_requests.rs` |
| `0x10`, `0x14` | Continuum | 3, 1 | Attach, drop attached | `protocol/client_requests.rs` |
| `0x13`, `0x15`, `0x1f`-`0x21` | Continuum | | Flag and ball requests; accepted, but the browser sends `0x67` | `server/objectives/` |
| `0x18` | Ext | 2, 4 | Ship change; the 4-byte form adds `position_sequence_base:u16` | `protocol/client_requests.rs` |
| `0x19` | Continuum | 97 | Set banner | `protocol/client_requests.rs` |
| `0x1a` | Continuum | 40, 46 | Security response | `protocol/security.rs` |
| `0x1c` | Continuum | 5 | Drop brick | `protocol/client_requests.rs` |
| `0x24` | Ext | 165 | Continuum login carrying the player identity | `protocol/login.rs`, `server/identity.rs` |
| `0x62` | ZS | 4 + 18 per input | Input record | `protocol/client_input_record.rs` |
| `0x63` | ZS | 25 | Movement correction application, version 2 | `protocol/movement_correction.rs` |
| `0x64` | ZS | 28 | Shutdown application, version 1 | `protocol/prize_application.rs` |
| `0x65` | ZS | 30 | Portal placement request, version 1 | `protocol/portal_placement.rs` |
| `0x67` | ZS | 45 | Objective action, version 1 | `protocol/objectives.rs` |
| `0x6a` | ZS | 31 | Manual Warp request, version 2 | `protocol/portal_warp.rs` |
| `0x71` | ZS | 3 | Snapshot ACK | `protocol/snapshot.rs` |
| `0x73` | ZS | 1, 18 | Full snapshot request; with mode `1`, projectile refresh after a renderer pause | `protocol/snapshot.rs` |
| `0x7c` | ZS | 50, 84 | Map already cached (v1), LVZ already held (v2) | `protocol/files.rs`, `zone_wire.rs` |
| `0x80` | ZS | 54, 62 | Player telemetry report, version 1 or 2 | `browser_metrics.rs`, `server/observability.rs` |
| `0x82` | ZS | 30 | Connection-quality echo | `protocol/connection_quality.rs` |
| `0x85` | ZS | 19 | Page visibility notice, version 1 (sent by the page itself) | `js/zubspace_player_telemetry.js`, `server/connection_quality.rs` |

### Server to client

| Id | Origin | Size | Purpose | Source |
| --- | --- | --- | --- | --- |
| `00 xx` | Continuum | | Core: reliable, ACK, small chunk, cluster | `server/reliable.rs` |
| `00 06` | Ext | 26 | Time sync response; Continuum's 10 bytes plus `server_session_id:u64 \| arena_stream_id:u64` | `protocol/login.rs`, `server/security.rs` |
| `0x01` | Continuum | 3 | Player id | `server/roster.rs` |
| `0x02` | Continuum | 1 | Join game | `server/arena/lifecycle.rs` |
| `0x03` | Ext | 64 (+20) | Player entering; squad is `zubspace`, and nonzero assists append a `0x09` in the same payload | `server/roster.rs` |
| `0x04` | Continuum | 3 | Player leaving | `server/roster.rs` |
| `0x06` | Ext | 22 | Player death; Continuum's 10 bytes plus `green_collectible_id:u32 \| life_seq:u32 \| respawn_at_tick:u32` | `server/server_damage.rs` |
| `0x07` | Continuum | variable | Chat | `server/player_events/chat.rs` |
| `0x08` | Ext | 13 | Player prize; the timestamp slot holds the collectible id | `server/player_events/prizes.rs` |
| `0x09` | Ext | 20 | Score update; Continuum's 15 bytes plus version `1` and `assists:u32` | `server/roster.rs` |
| `0x0a` | Ext | 26 | Password response (20 bytes) plus the trailer `ZCAP 01 01` | `server/session.rs` |
| `0x0b`, `0x0e`, `0x12`-`0x16`, `0x21`, `0x2e` | Continuum | | Objectives: soccer goal, turret links, flag position, claim, victory and drop, bricks (17 bytes, or 1 to clear), powerball | `server/objectives/packets.rs` |
| `0x0f` | Continuum | 1,428 | Arena settings | `server/arena_settings.rs` |
| `0x10` | Continuum | 17 + data | File transfer (news) | `server/zone.rs` |
| `0x18` | Continuum | 17 | Security challenge | `server/security.rs` |
| `0x1c` | Continuum | 2 | Spectate (extra position data on or off) | `server/player_events/spectate.rs` |
| `0x1d` | Ext | 10 | Ship and frequency change; Continuum's 6 bytes plus `life_seq:u32` | `server/roster.rs` |
| `0x1e`, `0x1f` | Continuum | | Personal and player banner | `server/roster.rs` |
| `0x29` | Ext | 80 + 56 per LVZ | Map information in the authenticated `ZMAP` form | `server/zone.rs` |
| `0x2a` | Continuum | 17 + data | Compressed map or LVZ (filename left empty), inside `0x7e` | `server/zone.rs` |
| `0x2f` | Continuum | variable | Arena directory; active zone first | `server/roster.rs` |
| `0x32` | Continuum | 5 | Set coordinates | `server/arena/positions.rs` |
| `0x33` | Continuum | text | Login failure, with a `ZUBSPACE_*` code | `server/session.rs` |
| `0x37` | Continuum | 2 | Toggle damage reporting; always off | `server/player_events/combat.rs` |
| `0x60` | ZS | 13 | Hidden prize spawn | `protocol/prizes.rs` |
| `0x61` | ZS | 44 | Prize pickup ACK to the collector | `protocol/prizes.rs` |
| `0x66` | ZS | 52 | Portal placement terminal, version 2 | `protocol/portal_placement.rs` |
| `0x68` | ZS | 49 | Objective state, version 2 | `protocol/objectives.rs` |
| `0x69` | ZS | 25 | Objective score, version 1 | `protocol/objectives.rs` |
| `0x6b` | ZS | 58 | Manual Warp terminal, version 1 | `protocol/portal_warp.rs` |
| `0x6c` | ZS | 108 | QA trace context, version 2; sent by debug builds and when QA trace upload or client debug tracing is configured | `protocol/qa_trace.rs` |
| `0x70` | ZS | 21 + body | Snapshot | `protocol/snapshot.rs`, `server/snapshot.rs` |
| `0x72` | ZS | 34 | Clock and sequence bootstrap, version 3 | `clock_bootstrap.rs` |
| `0x74` | ZS | 46 | Frequency change ACK with canonical state | `protocol/players/frequency.rs` |
| `0x75` | ZS | 20 | Lifecycle baseline receipt, version 3 | `protocol/lifecycle_receipts.rs` |
| `0x76` | ZS | 31 | Shutdown application terminal | `protocol/prize_application.rs` |
| `0x77` | ZS | 11 | Positive Warp receipt | `protocol/lifecycle_receipts.rs` |
| `0x7a` | ZS | 26 | Repel application receipt, version 3 | `protocol/lifecycle_receipts.rs` |
| `0x7b` | ZS | 47, 55 | Movement correction proposal, version 2, or 3 for the Repel-window reason | `protocol/movement_correction.rs` |
| `0x7c` | ZS | 49 | Movement correction terminal, version 3 | `protocol/movement_correction.rs` |
| `0x7d` | ZS | 52-748 | Realtime event | `protocol/realtime_event.rs` |
| `0x7e` | ZS | 12 + data | File transfer fragment | `file_transfer_chunk.rs` |
| `0x7f` | ZS | 27 | Ship change refused, version 1 | `protocol/lifecycle_receipts.rs` |
| `0x81` | ZS | 30 | Connection-quality probe | `protocol/connection_quality.rs` |
| `0x83` | ZS | 44 | Connection-quality directive | `protocol/connection_quality.rs` |
| `0x84` | ZS | up to 1,100 | Realtime event batch | `protocol/realtime_event.rs` |
| `0x8d` | ZS | 28 | Spawn-idle spectator transition, version 1 | `protocol/forced_spectator.rs` |

The client parses further Continuum server packets (positions, KOTH, UFO,
turf flags, LVZ toggles, redirect, select box and others) that the Zubspace
server never sends. As in Continuum, ids are per direction: `0x7c`, for
example, is a cached-map check from the client and a movement-correction
terminal from the server. `0x76` and `0x79` also appear as markers inside a
client position.

## Protocol revisions and older clients

The browser and server are built from one commit and deployed together (see
[deployment.md](deployment.md)). There is no general version negotiation. One
mechanism lets a newer server keep serving a browser tab still running an
older bundle: the client protocol revision, the byte after the 27-byte arena
login (`ArenaLoginRequest::PROTOCOL_REVISION`, currently 3). The server stores
it per client (`ClientHandle::protocol_revision`) and treats a 27-byte login
as revision 0. A server that predates the byte ignores it.

| Revision | The server may send |
| --- | --- |
| 0 | Snapshots with 5-byte sample times and full private state |
| 1 | Compact one-byte sample times (header flag `0x10`) |
| 2 | Compact private state: 13-byte lifecycle bodies and prize references (flag `0x20`, event `0x07`) |
| 3 | Position offsets in delta frames (mask bit `0x4000`) |

The constants are `CLIENT_PROTOCOL_*` in `src/protocol/snapshot.rs`; the
server checks them in `src/bin/zubspace_server/snapshot.rs`. Other packets
cover older peers individually:

- The server accepts the version 1 player report (`0x80`) and the version 1
  scoped spectate view (`0x08`).
- The client parses the Continuum forms of `00 06`, `0x09`, `0x1d` and
  `0x29`, version 2 lifecycle receipts (16 bytes, no spawn point), 57- and
  61-byte prize bodies and the 11-byte WATCH_DAMAGE body.
- Without the `ZCAP` trailer, the client requests every LVZ with `0x0c` and
  sends no version 2 `0x7c`. Older clients ignore the trailer.

Beyond these, an old tab and a newer server are not expected to work
together: the decoders reject unknown versions, flags, kinds and lengths
rather than guess.

### Changing the protocol

- Gate a new server form that older clients would reject on a new revision:
  raise `PROTOCOL_REVISION`, add a `CLIENT_PROTOCOL_*` constant and check it
  on the server. When changing a client packet, keep accepting the previous
  form on the server for tabs that are still open.
- Give new formats an explicit version byte or flag, not a sentinel value.
  Fixed-width decoders reject truncation and trailing bytes.
- Keep a Continuum packet's original fields in place and append extensions;
  check it against the reference game before calling it Continuum-compatible.
- Update the shared codec, round-trip tests and this page in the same change.
