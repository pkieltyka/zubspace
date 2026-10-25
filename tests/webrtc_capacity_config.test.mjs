import assert from "node:assert/strict";
import test from "node:test";

import {
  MAX_PLAYER_CLIENTS,
  assertRtcUdpPortOwnership,
  capacityBrowserUrl,
  capacityExpectedCandidatePaths,
  maxPlayerClientsFromRustLimits,
  parseCapacityClientCount,
  parseExpectedCandidatePaths,
} from "../tools/webrtc_capacity_config.mjs";

test("capacity runner follows the authoritative Rust client-id ceiling", () => {
  assert.equal(MAX_PLAYER_CLIENTS, 768);
  assert.equal(parseCapacityClientCount("1"), 1);
  assert.equal(parseCapacityClientCount("768"), 768);
  assert.throws(() => parseCapacityClientCount("769"), /1\.\.768/);
  assert.throws(() => parseCapacityClientCount("0"), /1\.\.768/);
  assert.throws(() => parseCapacityClientCount("1.5"), /1\.\.768/);
});

test("Rust limit reader supports the authoritative hexadecimal boundary", () => {
  assert.equal(
    maxPlayerClientsFromRustLimits(
      "pub(crate) const SERVER_BOT_ID_BASE: u16 = 0x0300;",
    ),
    768,
  );
  assert.throws(() => maxPlayerClientsFromRustLimits("const OTHER: u16 = 4;"), /could not read/);
});

test("candidate-path expectations accept only WebRTC candidate types", () => {
  assert.deepEqual(parseExpectedCandidatePaths("host->host,relay->relay,host->host"), [
    "host->host",
    "relay->relay",
  ]);
  assert.deepEqual(parseExpectedCandidatePaths("", "srflx->host"), ["srflx->host"]);
  assert.throws(() => parseExpectedCandidatePaths("udp->host"), /invalid ICE candidate path/);
  assert.throws(() => parseExpectedCandidatePaths("host->relay->host"), /invalid ICE candidate path/);
  assert.deepEqual(capacityExpectedCandidatePaths(""), ["host->prflx"]);
  assert.deepEqual(capacityExpectedCandidatePaths("", { icePolicy: "relay" }), [
    "relay->relay",
  ]);
  assert.throws(
    () => capacityExpectedCandidatePaths("", { remoteMode: true }),
    /requires ZUBSPACE_RTC_LOAD_EXPECT_CANDIDATE_PATHS/,
  );
});

test("external capacity mode requires an allowed browser origin", () => {
  assert.equal(capacityBrowserUrl(""), "about:blank");
  assert.equal(
    capacityBrowserUrl(" https://play.example.test/capacity-probe.html ", {
      remoteMode: true,
    }),
    "https://play.example.test/capacity-probe.html",
  );
  assert.throws(
    () => capacityBrowserUrl("", { remoteMode: true }),
    /requires ZUBSPACE_RTC_LOAD_BROWSER_URL/,
  );
});

test("capacity status enforces one fixed UDP port per active RTC peer", () => {
  assert.equal(
    assertRtcUdpPortOwnership({ rtc_udp_ports_in_use: 0 }, 0, "before load")
      .rtc_udp_ports_in_use,
    0,
  );
  assert.equal(
    assertRtcUdpPortOwnership({ rtc_udp_ports_in_use: 8 }, 8, "during steady load")
      .rtc_udp_ports_in_use,
    8,
  );
  assert.equal(
    assertRtcUdpPortOwnership({ rtc_udp_ports_in_use: 0 }, 0, "after cleanup")
      .rtc_udp_ports_in_use,
    0,
  );
});

test("capacity status reports actionable fixed-port ownership failures", () => {
  assert.throws(
    () =>
      assertRtcUdpPortOwnership(
        { rtc_udp_ports_in_use: 7, rtc_sessions: 8 },
        8,
        "during steady load",
      ),
    /during steady load: expected rtc_udp_ports_in_use=8 .*got 7/,
  );
  assert.throws(
    () => assertRtcUdpPortOwnership({}, 0, "before load"),
    /before load: expected rtc_udp_ports_in_use=0.*non-negative integer.*one exclusive fixed UDP port per RTC peer/,
  );
  assert.throws(
    () => assertRtcUdpPortOwnership({ rtc_udp_ports_in_use: -1 }, 0, "after cleanup"),
    /after cleanup: expected rtc_udp_ports_in_use=0/,
  );
});
