# Zubspace rtc 0.21.1 provenance

Source: published crates.io `rtc` 0.21.1, upstream revision recorded in
`.cargo_vcs_info.json`. Vendoring includes library source, normalized/original
manifests, licenses and README. Example, benchmark and integration targets are
omitted except the explicitly retained WebRTC integration targets below.
Normalized manifest target sections are trimmed; upstream dependencies and
features are unchanged. Library tests are retained in full.

The local DCEP adapter preserves remote channel ordering and the u16 lifetime or
retransmit limit, rejecting out-of-range values instead of truncating them.
Unit tests cover all six channel types and the u16 boundary.

The receiving adapter also applies an accepted remote OPEN to the actual SCTP
reply stream. Upstream kept the negotiated public metadata but left remotely
opened reply streams at ordered/reliable defaults. A configure-only internal
command now precedes ACK/application replies; it never sends a second OPEN,
never marks the public in-band channel out-of-band, and is not reapplied for
duplicate or rejected OPENs. Native real-association wire coverage checks all
three Zubspace channel contracts: ordered reliable control, unordered reliable
events and unordered 80 ms timed game data, including actual queued-data expiry
and exactly one ACK. The original behavioral RED is retained in the project
debug evidence. All 286 RTC library tests pass after this change.

The old `set_sctp_max_packet_size` plumbing is retired in favor of upstream
`SettingEngineBuilder::with_sctp_mtu`. Both configure the complete outbound
SCTP DATA-packet budget independently of message size; upstream additionally
rounds payloads down to four-byte alignment. Server configuration validates
its input before calling this API. The retained fragmentation/bundling test
exercises a 64 KiB message plus small messages against the 1200-byte ceiling.
Upstream endpoint-configuration tests cover forwarding the builder setting.
Control packets do not consult this DATA budget (an upstream limitation).

`ZUBSPACE.patch` records all local source changes against the published crate.
Apply with `patch --fuzz=0 -p1 < ZUBSPACE.patch` in an unmodified crate directory.
Manifest target trimming is separate from this source patch.

Source patches are rebased against published 0.21.1; all new upstream library tests are retained.
