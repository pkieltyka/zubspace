# Vendored dependencies

Zubspace pins its browser renderer to a Git revision and patches its native
WebRTC stack through local Cargo paths. `Cargo.toml` chooses the sources and
`Cargo.lock` records the resolved graph. These are separate dependency chains.

## Renderer and browser runtime

Both targets use the same Macroquad fork revision. Its Miniquad dependency is
vendored inside that fork, not under this repository's `vendor/`. The WASM
target enables audio; the native target disables default features.

Rust and `js/mq_js_bundle.js` must be upgraded together, with the matching
provenance JSON and runtime-contract checks. The exact revision, runtime hash,
ABI and antialiasing choice are in
[Browser runtime provenance](browser-runtime-provenance.md).
`node tools/macroquad_pairing.mjs` checks their agreement without a browser.

## WebRTC stack

`[patch.crates-io]` in the root manifest replaces these published 0.21.1
packages:

| Package | Role | Local runtime changes |
| --- | --- | --- |
| [`rtc`](../vendor/rtc-0.21.1/ZUBSPACE-PATCH.md) | WebRTC protocol core and DataChannel negotiation | Preserve remote DCEP ordering/reliability and apply it to the actual SCTP reply stream before ACK/application replies |
| [`rtc-sctp`](../vendor/rtc-sctp-0.21.1/ZUBSPACE-PATCH.md) | SCTP association, fragmentation, acknowledgements and partial reliability | Bundle pending SACKs with DATA; optional short deferral of a second-packet SACK |
| [`webrtc`](../vendor/webrtc-0.21.1/ZUBSPACE-PATCH.md) | Async driver and socket integration used by the server | Export the SACK option, optional write coalescing, wildcard-bind handling and richer UDP write errors |

Each directory retains `.cargo_vcs_info.json`, its upstream manifests and
licenses, a `ZUBSPACE-PATCH.md` rationale, and a `ZUBSPACE.patch` source diff
against the published crate. Example and benchmark targets are trimmed; the
library tests and three selected `webrtc` backpressure/close integration tests
remain: `data_channel_slow_consumer`, `data_channel_close_wakes_send` and
`data_channel_send_backpressure`. Manifest target trimming is separate from
the source patch.

### Why the patches matter

The three DataChannel contracts must apply in both directions. Keeping the
browser's public DCEP metadata while replying on an ordered, reliable SCTP
stream would defeat the timed game channel. Native wire tests check reply
ordering, queued DATA expiry and the ACK, in addition to channel metadata.

SACK bundling and write coalescing save datagrams rather than changing game
semantics. A SACK can share an outgoing DATA packet when it fits. With
`rtc_defer_sack_to_data`, the second-packet ACK waits at most 20 ms for DATA;
loss/duplicate handling still uses the immediate paths. With
`rtc_write_coalesce_micros`, the driver briefly waits before flushing writes
from several channels. Both changes remain bounded, and the channels still
share transport congestion control. Server configuration and deployed values
are described in [Networking](networking.md#configuration).

Upstream 0.21.1 already supplies the earlier DATA-padding/packet-budget fixes,
the SCTP MTU builder setting and nonblocking slow-consumer handling. Those
retired patches should not be reapplied during upgrades. The per-crate notes
explain what was replaced upstream and what remains local.

The default SCTP DATA-packet budget is 1,191 bytes, with payload alignment
handled by the library. `rtc_udp_path_mtu` describes a measured IPv4 path MTU;
the server derives its SCTP budget after transport overhead. This is distinct
from a DataChannel message-size limit: larger messages can be fragmented.

## Updating the stack

1. Obtain the published crate for each new version and compare it with the
   current per-crate patch notes and source diff. Keep upstream changes that
   supersede our fixes; port the remaining behavioral changes deliberately.
2. Preserve upstream library tests and the retained `webrtc` integration
   targets. Reconcile normalized/original manifests and target trimming as
   well as Rust source; `ZUBSPACE.patch` alone does not describe that trimming.
3. Update the root dependency version and all three patch paths together,
   refresh `Cargo.lock`, and inspect the resolved packages for unintended
   registry copies of the patched crates.
4. Regenerate each source diff against its new published base and update its
   rationale and provenance. Check that it applies cleanly, without fuzz:

   ```sh
   patch --dry-run --fuzz=0 -p1 < /path/to/ZUBSPACE.patch
   ```

   Run that command inside an unmodified upstream crate directory, not the
   already patched vendor directory.
5. Run the relevant library and integration checks below. Rebuild client and
   server before the real Chrome/Firefox channel-contract and gameplay checks.
   Confirm reply-stream reliability and timed expiry on actual DATA, not just
   negotiation fields or a successful handshake.

## Checks

`make test-vendor-rtc` runs the vendored **rtc library suite**, including its
real SCTP-association wire regressions. It does not run every test in all three
packages. It is included in `make test-quick`.

For an explicit stack upgrade, run the other suites from the repository root:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo test --locked \
  --manifest-path vendor/rtc-sctp-0.21.1/Cargo.toml --lib

CARGO_TARGET_DIR="$PWD/target" cargo test --locked \
  --manifest-path vendor/webrtc-0.21.1/Cargo.toml \
  --config 'patch.crates-io.rtc.path="vendor/rtc-0.21.1"' \
  --config 'patch.crates-io.rtc-sctp.path="vendor/rtc-sctp-0.21.1"'
```

The standalone manifest commands need the local patch overrides shown above
where applicable; they do not automatically inherit the root package's patch
table. Browser channel-contract, Compose and capacity checks are listed in
[the E2E guide](../tests/e2e/README.md). A capacity handshake test does not
establish full combat performance at the same player count.
