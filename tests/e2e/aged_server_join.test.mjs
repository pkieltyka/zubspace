import test from "node:test";
import assert from "node:assert/strict";
import { agedServerJoinOptions } from "./aged_server_join.mjs";

test("ordinary joins preserve immediate same-zone startup", () => {
  assert.deepEqual(agedServerJoinOptions({}, "extreme"), {
    minimumUptimeSeconds: 0, initialZone: "extreme",
  });
});
test("aged joins independently select initial and requested zones", () => {
  assert.deepEqual(agedServerJoinOptions({
    ZUBSPACE_E2E_JOIN_MIN_UPTIME_SECONDS: "350",
    ZUBSPACE_E2E_INITIAL_SERVER_ZONE: "chaos-zone",
  }, "extreme"), { minimumUptimeSeconds: 350, initialZone: "chaos-zone" });
});
test("invalid aging requirements fail instead of silently skipping coverage", () => {
  for (const value of ["-1", "NaN", "1.5", "901", "Infinity"]) {
    assert.throws(() => agedServerJoinOptions({
      ZUBSPACE_E2E_JOIN_MIN_UPTIME_SECONDS: value,
    }, "extreme"));
  }
});
