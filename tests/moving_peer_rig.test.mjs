// The eight-peer motion gate may run on either raster rig, and both are acceptance-capable. That is a
// decision, not an accident, so it is pinned here: the lane must name the rig it measured, must not
// demote a rig to diagnostic-only because of its backend, and must keep proving the adapter is real.
// Without these, the easy regression is a run that quietly becomes "the slow rig" and a pass that
// means nothing because the rig, not the game, set the frame period.
import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

const lane = fs.readFileSync(new URL("../tests/e2e/moving_peer_load.mjs", import.meta.url), "utf8");
const makefile = fs.readFileSync(new URL("../Makefile", import.meta.url), "utf8");

test("the lane records the rig it measured, in the artifact and on the console", () => {
  assert.match(lane, /const graphicsRig = \(\) => \(\{/, "the rig summary is derived, not hand-typed");
  assert.match(lane, /graphicsRig: graphicsRig\(\),/, "result.json carries the rig");
  assert.match(lane, /console\.log\(JSON\.stringify\(\{ ok: !diagnosticOnly, diagnosticOnly, count,\s*\n?\s*graphicsRig: graphicsRig\(\)/,
    "the printed line carries it too, so a terminal read cannot lose it");
  // The requested backend is only an intention; the renderer string is the witness.
  assert.match(lane, /renderers: \[\.\.\.new Set\(Object\.values\(graphicsEvidence\.peers\)/);
});

test("the graphics backend alone does not make a run diagnostic-only", () => {
  const assignment = lane.match(/const diagnosticOnly = ([^;]*);/s);
  assert.ok(assignment, "the diagnostic-only rule is one readable expression");
  assert.doesNotMatch(assignment[1], /graphicsBackend/,
    "a rig choice is not a measurement modifier; it is recorded in graphicsRig instead");
  // Everything that genuinely changes what is measured stays demoting.
  for (const modifier of ["captureDiagnostic", "profiling", "attribution", "serverCpuSet",
    "browserCpuSet", "ZUBSPACE_SERVER_POSITION_TIMING", "staticBundleOverride"]) {
    assert.ok(assignment[1].includes(modifier), `${modifier} must stay diagnostic-only`);
  }
});

test("a claimed GPU rig still has to prove the adapter is real", () => {
  assert.match(lane, /graphicsBackend !== "swiftshader"/, "every GPU path is checked, not trusted");
  assert.match(lane, /graphicsBackend === "native"/, "native desktop graphics has its own verification");
  assert.match(lane, /features\.gpu_compositing !== "enabled"/);
  assert.match(lane, /features\.webgl !== "enabled"/);
  assert.match(lane, /AMD\|NVIDIA\|Intel/, "a real vendor is required");
  assert.match(lane, /swiftshader\|llvmpipe\|lavapipe\|software/,
    "a software renderer answering on a claimed GPU rig is a failure, not a pass");
});

test("the GPU acceptance lane is a named make target on the same contract", () => {
  const target = makefile.match(/test-e2e-moving-peers-gpu: ([^\n]*)\n\t([^\n]*)/);
  assert.ok(target, "the GPU rig has its own target");
  assert.match(target[1], /build-client build-server/, "it builds both artifacts like the software lane");
  assert.match(target[2], /ZUBSPACE_E2E_MOVING_GRAPHICS_BACKEND=native/);
  assert.match(target[2], /node tests\/e2e\/moving_peer_load\.mjs/, "it runs the same lane, not a copy");
});

test("a software-raster floor failure is classified as rig-bound instead of asserted as a product stall", () => {
  // The lane must carry the rig's authority, not just the rig's name: the same gap number means
  // different things on the two rigs, and the artifact is where a reader finds that out.
  assert.match(lane, /gapCeilingIsRigBound: graphicsBackend === "swiftshader"/);
  assert.match(lane, /"rig-bound-ceiling-unclassified"/);
  assert.match(lane, /"product-motion-floor-failure"/);
  assert.match(lane, /renewCommand/, "a rig-bound failure names the renewal that would classify it");
  assert.match(lane, /motion-floor-met/, "and a clean run says so in the same field");

  // The classification has to exist on the path that needs it. The first version computed it after the
  // `!evidence.ok` throw, so the only run that could have used it - the 12-tick software-raster breach -
  // produced a thrown blob in a log and no artifact at all.
  const contractAt = lane.indexOf("const motionContract = {");
  const throwAt = lane.indexOf("if (!evidence.ok)");
  assert.ok(contractAt > 0 && throwAt > contractAt,
    "the verdict is composed before the failure throw, not after it");
  assert.match(lane, /failure: "admission-floor"/, "a failing run writes its own artifact");
  assert.match(lane, /provenance: failureProvenance/,
    "with provenance, so the failed verdict can be rechecked against the tree that produced it");
  assert.match(lane, /worstGapTicks/, "and the worst gap is stated next to its own classification");
});
