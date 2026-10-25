import assert from "node:assert/strict";
import test from "node:test";

import { predictionTracksAuthoritativeSnapshot } from "./energy_recovery_evidence.mjs";

const rechargePerSecond = 115;
const predictionTolerance = 25;
const frameQuantization = rechargePerSecond / 60 + 2;

test("energy prediction permits either ordering within one quantized frame", () => {
  assert.equal(predictionTracksAuthoritativeSnapshot({
    predicted: 295.97,
    server: 298,
    predictionTolerance,
  }, rechargePerSecond), true);
  assert.equal(predictionTracksAuthoritativeSnapshot({
    predicted: 298 + predictionTolerance + frameQuantization,
    server: 298,
    predictionTolerance,
  }, rechargePerSecond), true);
});

test("energy prediction rejects lag or lead beyond the explicit contract", () => {
  assert.equal(predictionTracksAuthoritativeSnapshot({
    predicted: 298 - frameQuantization - 0.01,
    server: 298,
    predictionTolerance,
  }, rechargePerSecond), false);
  assert.equal(predictionTracksAuthoritativeSnapshot({
    predicted: 298 + predictionTolerance + frameQuantization + 0.01,
    server: 298,
    predictionTolerance,
  }, rechargePerSecond), false);
  assert.equal(predictionTracksAuthoritativeSnapshot({
    predicted: Number.NaN,
    server: 298,
    predictionTolerance,
  }, rechargePerSecond), false);
});
