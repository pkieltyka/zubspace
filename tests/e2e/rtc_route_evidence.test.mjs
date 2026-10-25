import assert from "node:assert/strict";
import test from "node:test";
import { assertSelectedRtcRoute, waitForSelectedRtcRoute } from "./rtc_route_evidence.mjs";

function fixture(selected = true) {
  return { connectionCount: 1, connections: [{ connectionState: "connected",
    transport: selected ? [{ selectedCandidatePairId: "pair" }] : [],
    candidatePairs: [{ id: "pair", state: "succeeded", nominated: true, remoteCandidateId: "remote" }],
    candidates: [{ id: "remote", address: "127.0.0.1", port: 19001 }],
  }] };
}

test("an explicit selected pair resolves the exact UDP front socket", () => {
  assert.equal(assertSelectedRtcRoute(fixture(), 19001).source, "transport-selected");
});

test("an omitted transport selector requires a uniquely nominated successful pair", () => {
  assert.equal(assertSelectedRtcRoute(fixture(false), 19001).source, "unique-nominated");
});

test("an explicit missing selected pair cannot fall back to another nominated pair", () => {
  const stats = fixture();
  stats.connections[0].transport[0].selectedCandidatePairId = "missing";
  assert.throws(() => assertSelectedRtcRoute(stats, 19001), /one selected or uniquely nominated/);
});

test("an ambiguous, failed or unnominated fallback never proves the selected route", () => {
  const ambiguous = fixture(false);
  ambiguous.connections[0].candidatePairs.push({ ...ambiguous.connections[0].candidatePairs[0], id: "other" });
  assert.throws(() => assertSelectedRtcRoute(ambiguous, 19001), /one selected or uniquely nominated/);
  for (const patch of [{ state: "failed" }, { nominated: false }]) {
    const stats = fixture(false);
    Object.assign(stats.connections[0].candidatePairs[0], patch);
    assert.throws(() => assertSelectedRtcRoute(stats, 19001), /one selected or uniquely nominated/);
  }
});

test("a wrong remote socket, disconnected peer or second association is rejected", () => {
  for (const mutate of [
    stats => { stats.connections[0].candidates[0].address = "192.0.2.1"; },
    stats => { stats.connections[0].candidates[0].port++; },
    stats => { stats.connections[0].connectionState = "disconnected"; },
    stats => { stats.connectionCount++; },
  ]) {
    const stats = fixture(); mutate(stats);
    assert.throws(() => assertSelectedRtcRoute(stats, 19001));
  }
});

test("a transient consent check must settle into the same strict selected-route proof", async () => {
  let reads = 0;
  const result = await waitForSelectedRtcRoute(async () => {
    const stats = fixture();
    if (++reads < 3) stats.connections[0].candidatePairs[0].state = "in-progress";
    return stats;
  }, 19001, { timeoutMs: 100, pollMs: 1 });
  assert.equal(reads, 3);
  assert.equal(result.observations.length, 2);
  assert.equal(result.selected.source, "transport-selected");
  assert.equal(result.selected.pair.state, "succeeded");
});

test("settling cannot authorize a wrong proxy socket or a persistently pending pair", async () => {
  for (const mutate of [
    stats => { stats.connections[0].candidates[0].port++; },
    stats => { stats.connections[0].candidatePairs[0].state = "in-progress"; },
  ]) {
    await assert.rejects(waitForSelectedRtcRoute(async () => {
      const stats = fixture(); mutate(stats); return stats;
    }, 19001, { timeoutMs: 10, pollMs: 1 }), /did not settle/);
  }
});
