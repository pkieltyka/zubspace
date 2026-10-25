import test from "node:test";
import assert from "node:assert/strict";
import { serverTimingEvidence } from "./server_timing_evidence.mjs";

function fixture() {
  const histogram = () => ({ samples: 100, saturated: false, min_us: 0, max_us: 100,
    p50: { lower_us: 40, upper_us: 47 }, p95: { lower_us: 88, upper_us: 95 },
    p99: { lower_us: 96, upper_us: 100 } });
  return { tick_timing: { scope: "server-lifetime", interval: histogram(), absolute_jitter: histogram(), work: histogram() } };
}

test("timing evidence preserves bounded quantiles of equal tick populations", () => {
  const f = fixture();
  assert.deepEqual(serverTimingEvidence(f), f.tick_timing);
});

test("a tick boundary without its work sample is rejected, in either direction", () => {
  for (const mutate of [
    t => { t.work.samples += 1; },
    t => { t.work.samples -= 1; },
  ]) {
    const f = fixture(); mutate(f.tick_timing);
    assert.throws(() => serverTimingEvidence(f), /do not match tick boundaries/);
  }
});

test("missing, empty, saturated, inconsistent and fabricated timing evidence fails", () => {
  assert.throws(() => serverTimingEvidence({}));
  for (const mutate of [
    t => { t.work.samples = 0; },
    t => { t.work.saturated = true; },
    t => { t.interval.samples += 1; },
    t => { t.work.samples += 2; },
    t => { t.work.p95.lower_us = 12; },
    t => { t.work.p99.upper_us = 1000; },
    t => { t.work.p50.upper_us = NaN; },
    t => { delete t.work.max_us; },
  ]) {
    const f = fixture(); mutate(f.tick_timing);
    assert.throws(() => serverTimingEvidence(f));
  }
});
