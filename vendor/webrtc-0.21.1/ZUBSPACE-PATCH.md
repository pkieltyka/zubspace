# Zubspace webrtc 0.21.1 provenance

Source: published crates.io `webrtc` 0.21.1, upstream revision recorded in
`.cargo_vcs_info.json`. Vendoring includes library source, normalized/original
manifests, licenses and README. Example, benchmark and integration targets are
omitted except the explicitly retained WebRTC integration targets below.
Normalized manifest target sections are trimmed; upstream dependencies and
features are unchanged. Library tests are retained in full.

`src/lib.rs` re-exports `rtc::sctp::set_defer_sack_to_data` so the
Zubspace server can opt its associations into the vendored rtc-sctp
`AckMode::DeferToData` without a direct dependency on rtc-sctp.

**Write coalescing, opt-in.** Each data channel's `send` wakes the
driver, which flushes the SCTP core at once. When an application writes to
several channels at the same instant from separate tasks (Zubspace's snapshot,
event batch and acknowledgements each tick), the first wake usually flushes
alone and the rest follow in their own datagrams. `set_write_coalescing(wait)`
(re-exported from `src/lib.rs`) makes a driver woken by `WriteNotify` sleep
`wait` before its next flush, so those writes share SCTP packets. Runtime timers
round the wait up (one millisecond on tokio). The default of zero keeps the
upstream behaviour.

Local source changes preserve verbatim wildcard UDP/TCP binding with duplicate
removal and late DNS resolution, and include packet/segment lengths in failed
UDP/GSO write diagnostics. The wildcard unit regression covers both families.

The old awaited DataChannel queue send is retired. Upstream 0.21.1 retains
queued events and stops draining SCTP while the consumer is stalled, without
blocking the driver loop that services ICE and transport timers. We retain the
upstream `data_channel_slow_consumer`, `data_channel_close_wakes_send`, and
`data_channel_send_backpressure` integration targets and shared test helpers
unchanged to verify delivery, bounded buffering, timer liveness and teardown.

`ZUBSPACE.patch` records all local source changes against the published crate.
Apply with `patch --fuzz=0 -p1 < ZUBSPACE.patch` in an unmodified crate directory.
Manifest target trimming is separate from this source patch.

Source patches are rebased against published 0.21.1; all new upstream library tests are retained.
