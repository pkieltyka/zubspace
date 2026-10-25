import assert from "node:assert/strict";
import test from "node:test";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { assertArtifactProvenanceMatches, collectArtifactProvenance } from "./artifact_provenance.mjs";

test("live provenance collection yields the log-draining event loop and preserves exact identity", async () => {
  const options = { e2eScript: "tests/e2e/artifact_provenance_async.test.mjs" };
  const expected = collectArtifactProvenance(options);
  let settled = false;
  let heartbeats = 0;
  const timer = setInterval(() => { if (!settled) heartbeats += 1; }, 10);
  try {
    const actual = await collectArtifactProvenanceAsync(options);
    settled = true;
    assert.ok(heartbeats > 0, "collection must not starve the parent runner's log readers");
    assertArtifactProvenanceMatches(expected, actual);
  } finally {
    clearInterval(timer);
  }
});

test("worker-side identity errors reject instead of returning incomplete evidence", async () => {
  await assert.rejects(collectArtifactProvenanceAsync({ e2eScript: "tests/e2e/nonexistent-provenance-input.mjs" }));
});
