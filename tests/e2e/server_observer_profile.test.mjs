import assert from "node:assert/strict";
import test from "node:test";

import {
  assertServerObserverProfile,
  serverObserverProfileForNetworkProfile,
} from "./server_observer_profile.mjs";

test("trusted server profiles match each supported browser impairment envelope", () => {
  assert.equal(serverObserverProfileForNetworkProfile(""), "clean-lan");
  assert.equal(serverObserverProfileForNetworkProfile("mild"), "supported-mild-wan");
  assert.equal(serverObserverProfileForNetworkProfile("slow"), "supported-slow-wan");
  assert.throws(
    () => serverObserverProfileForNetworkProfile("recovery-stress"),
    /unsupported browser impairment profile/,
  );
});

test("artifact evidence requires the effective server-owned profile", () => {
  assert.deepEqual(
    assertServerObserverProfile(
      { observer_network_profile: "supported-slow-wan" },
      "supported-slow-wan",
    ),
    {
      authority: "trusted-server-config",
      expectedProfile: "supported-slow-wan",
      actualProfile: "supported-slow-wan",
    },
  );
  assert.throws(
    () => assertServerObserverProfile(
      { observer_network_profile: "clean-lan" },
      "supported-slow-wan",
    ),
    /server observer profile mismatch/,
  );
});
