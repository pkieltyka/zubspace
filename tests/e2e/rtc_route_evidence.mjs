import assert from "node:assert/strict";

// When transport.selectedCandidatePairId is unavailable, require
// one unambiguous nominated/succeeded pair, never an arbitrary
// successful check. An explicit selected id always takes precedence.
export function assertSelectedRtcRoute(stats, expectedPort) {
  assert.equal(stats?.connectionCount, 1, "one pinned RTC association required");
  assert.equal(stats.connections?.length, 1, "one current RTC connection required");
  const connection = stats.connections[0];
  assert.equal(connection.connectionState, "connected");
  const selectedIds = [...new Set(connection.transport
    .map(entry => entry.selectedCandidatePairId).filter(Boolean))];
  assert.ok(selectedIds.length <= 1, "ambiguous selected RTC pairs");
  const selectedId = selectedIds[0];
  const pairs = connection.candidatePairs.filter(pair => pair.state === "succeeded"
    && (selectedId ? pair.id === selectedId : pair.nominated === true));
  assert.equal(pairs.length, 1, "one selected or uniquely nominated successful ICE pair required");
  const remotes = connection.candidates.filter(candidate => candidate.id === pairs[0].remoteCandidateId);
  assert.equal(remotes.length, 1, "selected ICE pair must resolve to exactly one remote candidate");
  assert.equal(remotes[0].address, "127.0.0.1");
  assert.equal(remotes[0].port, expectedPort, "selected route must use this proxy's exact front socket");
  return { source: selectedId ? "transport-selected" : "unique-nominated", pair: pairs[0], remote: remotes[0] };
}

// Consent checks may temporarily report an already selected, connected pair as
// in-progress. Observe a bounded settled sample rather than weakening the
// successful-pair or exact proxy-socket requirement.
export async function waitForSelectedRtcRoute(readStats, expectedPort, {
  timeoutMs = 5_000, pollMs = 100,
} = {}) {
  assert.ok(timeoutMs > 0 && timeoutMs <= 5_000 && pollMs > 0);
  const deadline = Date.now() + timeoutMs;
  let lastError;
  const observations = [];
  do {
    const stats = await readStats();
    try {
      return { stats, selected: assertSelectedRtcRoute(stats, expectedPort), observations };
    } catch (error) {
      lastError = error;
      observations.push({ connectionCount: stats?.connectionCount,
        connections: stats?.connections?.map(c => ({ state: c.connectionState,
          transport: c.transport, candidatePairs: c.candidatePairs })) });
    }
    if (Date.now() >= deadline) break;
    await new Promise(resolve => setTimeout(resolve, Math.min(pollMs, deadline - Date.now())));
  } while (Date.now() < deadline);
  lastError.message += `; selected route did not settle within ${timeoutMs}ms`;
  throw lastError;
}
