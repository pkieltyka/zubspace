#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import { validateArtifactPathReferences } from "./artifact_paths.mjs";
import { validateQaTraceArtifacts, validateQaTraceChildResults } from "./qa_trace_artifacts.mjs";

const artifactPath = process.argv[2];
if (!artifactPath) {
  throw new Error("usage: node tests/e2e/verify_artifact_provenance.mjs <provenance.json|result.json>");
}
const document = JSON.parse(fs.readFileSync(path.resolve(artifactPath), "utf8"));
const artifactPathAudit = path.basename(artifactPath) === "result.json"
  ? validateArtifactPathReferences(document, artifactPath)
  : null;
// Failure artifacts may truthfully report that the page never became ready.
// A retained success from the common browser writer may not hide that gap.
const qaTraceAudit = validateQaTraceArtifacts(document, artifactPath);
const qaTraceChildAudit = validateQaTraceChildResults(document, artifactPath);
const expected = document.provenance || document.successArtifacts?.provenance || document;
const current = collectArtifactProvenance({
  e2eScript: expected.e2eScript?.path || "tests/e2e/combat_replay.mjs",
  serverBinary: expected.serverBinary?.path || "target/debug/zubspace_server",
  wasm: expected.wasm?.path || "target/wasm32-unknown-unknown/debug/zubspace.wasm",
  clientEntry: expected.clientEntry?.path || "index.html",
  additionalFiles: (expected.additionalFiles || []).map((entry) => entry.path),
  dockerImages: (expected.dockerImages || []).map((entry) => entry.name),
  runStartedAt: expected.runStartedAt || null,
});
assertArtifactProvenanceMatches(expected, current);
console.log(JSON.stringify({ ok: true, artifactPathAudit, qaTraceAudit, qaTraceChildAudit, provenance: current }, null, 2));
