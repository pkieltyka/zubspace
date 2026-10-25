# Zubspace rtc-sctp 0.21.1 provenance

Source: published crates.io `rtc-sctp` 0.21.1, upstream revision recorded in
`.cargo_vcs_info.json`. Vendoring includes library source, normalized/original
manifests, licenses and README. Example, benchmark and integration targets are
omitted except the explicitly retained WebRTC integration targets below.
Normalized manifest target sections are trimmed; upstream dependencies and
features are unchanged. Library tests are retained in full.

No 0.20.5-era runtime patch remains. Upstream 0.21.1 counts the complete padded DATA
chunk before bundling, replacing our 0.20.5 wire-size fix. It also fixes fast
retransmit accounting and rounds fragmentation payloads to four-byte alignment.
The default SCTP budget is now 1191 bytes (1188 after alignment), safe for an
IPv6/TURN minimum-MTU path. Explicit server path-MTU settings preserve the
previous configured ceiling: 1200 for a 1280-byte IPv4 path, capped at 1228.
All upstream library tests remain available, including packet-budget tests.

**Local runtime patch: SACKs ride with outgoing DATA.** Upstream
always sends a SACK in its own packet, so every SACK costs a whole datagram
(about 70 bytes of DTLS/UDP framing on a WebRTC path) even when DATA leaves in
the same poll. When new DATA is gathered and a SACK is pending, whether due now
or still waiting on the 200 ms delayed-ack timer, the SACK is now written first
in the first DATA packet (`take_pending_sack_for`), the ack timer is stopped and
the ack state returns to idle. RFC 4960 section 6 allows control chunks ahead of
DATA in one packet, and sending a delayed SACK early only shortens its delay. If
the SACK and the first DATA chunk would not fit one packet, the SACK goes alone
first, as before. Retransmissions, FORWARD-TSN and standalone SACKs are
unchanged. Two library tests cover bundling and the oversized fallback.

**Local runtime patch: `AckMode::DeferToData`, opt-in.** Bundling
alone only catches SACKs that are pending while DATA happens to leave. Under
the normal rule a second packet makes the SACK due at once, so it still went
alone when no DATA was queued in that poll. In the new mode the second packet
instead keeps the SACK pending with the ack timer shortened to at most 20 ms
(`DEFERRED_ACK_INTERVAL`, `TimerTable::start_or_shorten`), so the next DATA
carries it or the timer sends it alone. Losses, duplicates, the I-bit and the
other modes are unchanged. RFC 9260 section 6.2 asks for a SACK at least every
second packet as a SHOULD; the mode is meant for a peer that sends DATA every
few milliseconds, as a Zubspace server does every 16.7 ms. It is off by default,
so upstream behaviour and every upstream test are unchanged.
`set_defer_sack_to_data(true)` (re-exported by the vendored `webrtc`) selects it
for associations created afterwards. `test_deferred_sack_rides_with_the_next_data_or_its_short_timer`
covers both outcomes. All 189 library tests pass.

`ZUBSPACE.patch` records all local source changes against the published crate.
Apply with `patch --fuzz=0 -p1 < ZUBSPACE.patch` in an unmodified crate directory.
Manifest target trimming is separate from this source patch.

Local diagnostic coverage: `src/endpoint/wan_probe_measurement.rs` is an ignored
measurement using the existing native association-pair fixtures. It reports
80ms timed-channel probe delivery at captured application payload sizes over
virtual0/180/240ms RTT paths, with/without reliable warmup and controlled
datagram loss. It changes no library runtime behavior and is not browser,
DTLS/UDP or gameplay acceptance. Run explicitly with the library test filter
`measure_timed_game_probe_delivery_over_virtual_link -- --ignored --nocapture`.

Source patches are rebased against published 0.21.1; all new upstream library tests are retained.
