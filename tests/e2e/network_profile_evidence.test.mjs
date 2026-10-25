import assert from "node:assert/strict";
import test from "node:test";

import { assertNetworkProfileEvidence } from "./network_profile_evidence.mjs";

function completeEvidence() {
  return {
    configured: true,
    profile: { name: "mild", seed: 77, oneWayDelayMs: 30, jitterMs: 10 },
    s2c: {
      seen: 100,
      delayed: 100,
      released: 99,
      snapshotDropped: 1,
      duplicateSelected: 1,
      duplicated: 1,
      reorderSelected: 1,
      reordered: 1,
      delay: { samples: 100, minMs: 20, maxMs: 40, averageMs: 30 },
      pending: 0,
      expired: 0,
    },
    c2s: {
      seen: 100,
      delayed: 100,
      released: 99,
      inputDropped: 1,
      duplicateSelected: 1,
      duplicated: 1,
      reorderSelected: 1,
      reordered: 1,
      delay: { samples: 100, minMs: 20, maxMs: 40, averageMs: 30 },
      pending: 0,
      expired: 0,
    },
  };
}

test("network profile evidence requires every configured effect to be realized", () => {
  const evidence = completeEvidence();
  assert.equal(assertNetworkProfileEvidence(evidence, {
    expectedName: "mild",
    expectedSeed: 77,
  }), evidence);

  evidence.c2s.reordered = 0;
  assert.throws(
    () => assertNetworkProfileEvidence(evidence, {
      expectedName: "mild",
      expectedSeed: 77,
      label: "Firefox mild profile",
    }),
    /Firefox mild profile lacked realized evidence.*c2s\.reordered/,
  );
});

test("network profile evidence rejects wrong scope, delay, pending work, and expiry", () => {
  const evidence = completeEvidence();
  evidence.profile.seed = 78;
  // A zero release is the historical defect the minimum exists to catch: the old `Date.now()` measurement could
  // report one against a 20 ms floor.
  evidence.s2c.delay.minMs = 0;
  evidence.s2c.pending = 1;
  evidence.c2s.expired = 1;
  assert.throws(
    () => assertNetworkProfileEvidence(evidence, {
      expectedName: "mild",
      expectedSeed: 77,
    }),
    (error) => [
      "profile-seed",
      "s2c.realized-delay",
      "drain-or-expiry",
    ].every((item) => error.message.includes(item)),
  );
});

test("network profile evidence rejects a realized delay whose mean misses the authored floor", () => {
  // The mean is what proves the profile was applied; a tail reading cannot condemn it and a low mean cannot pass.
  const evidence = completeEvidence();
  evidence.s2c.delay.averageMs = 19;
  assert.throws(
    () => assertNetworkProfileEvidence(evidence, { expectedName: "mild", expectedSeed: 77 }),
    (error) => error.message.includes("s2c.realized-delay"),
  );
});

test("network profile evidence admits the browser timer's early-fire tail when the mean meets the floor", () => {
  // The frozen matrix's Firefox mild cell: min 18 ms against a 20 ms floor over 28,154 releases, with the
  // mean at 32.88 ms. The minimum is an extreme-order statistic over that many samples, so it is the timer, not the
  // profile; the mean is the signal.
  const evidence = completeEvidence();
  evidence.s2c.delay = { samples: 28_154, minMs: 18, maxMs: 101, averageMs: 32.876_642_750_586_065 };
  evidence.c2s.delay = { samples: 26_719, minMs: 19, maxMs: 90, averageMs: 32.873_872_525_169_354 };
  assert.equal(assertNetworkProfileEvidence(evidence, {
    expectedName: "mild",
    expectedSeed: 77,
  }), evidence);
});

test("network profile evidence admits a minimum just under the delay floor", () => {
  const evidence = completeEvidence();
  evidence.s2c.delay.minMs = 19;
  evidence.c2s.delay.minMs = 19;
  assert.equal(assertNetworkProfileEvidence(evidence, {
    expectedName: "mild",
    expectedSeed: 77,
  }), evidence);
});
