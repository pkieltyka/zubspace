import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { parseZubspaceEvents } from "../../tools/server_events.mjs";
import { assertArtifactProvenanceMatches } from "./artifact_provenance.mjs";
import { artifactFileManifest, validateArtifactPathReferences } from "./artifact_paths.mjs";
import { assertQaTraceCoverage } from "./qa_trace_coverage.mjs";
import { assertCompleteQaTraceCaptures } from "./qa_trace_capture.mjs";

const correlatedTypes = new Set([
  "server_collision_witness", "server_damage_applied", "server_player_death",
  "server_player_respawn", "server_projectile_spawned", "server_projectile_hit",
]);

function compactProvenance(provenance) {
  return {
    formatVersion: provenance?.formatVersion,
    gitHead: provenance?.gitHead,
    workingTreeDiffSha256: provenance?.workingTreeDiffSha256,
    clientBuildSha256: provenance?.clientBuildSha256,
    serverBinarySha256: provenance?.serverBinary?.sha256 ?? null,
    wasmSha256: provenance?.wasm?.sha256 ?? null,
    clientEntrySha256: provenance?.clientEntry?.sha256 ?? null,
    e2eScriptSha256: provenance?.e2eScript?.sha256 ?? null,
  };
}

export function assertQaServerBinaryProvenance(bundle, provenance) {
  const identity = bundle?.provenance?.serverBinary;
  assert.equal(identity?.kind, "running-executable-sha256", "missing running-server identity");
  assert.equal(identity.status, "ready", "running-server identity unavailable");
  assert.match(identity.sha256, /^[0-9a-f]{64}$/);
  // fileEvidence.sha256 also binds pathname, mode, and filesystem type. The
  // running process reports the digest of its bytes, not that framed identity.
  assert.equal(identity.sha256, provenance?.serverBinary?.contentSha256,
    "QA bundle came from a different server executable");
  assert.equal(identity.bytes, provenance?.serverBinary?.bytes);
  assert.ok(Number.isSafeInteger(identity.bytes) && identity.bytes > 0);
  return identity;
}

export function assertQaWasmProvenance(bundle, provenance) {
  const identity = bundle?.provenance?.wasm;
  assert.equal(identity?.kind, "loaded-wasm-sha256", "missing loaded-WASM identity");
  assert.equal(identity.status, "ready", "loaded-WASM identity unavailable");
  assert.match(identity.sha256, /^[0-9a-f]{64}$/);
  assert.equal(identity.sha256, provenance?.wasm?.contentSha256,
    "QA bundle came from different loaded WASM bytes");
  assert.equal(identity.bytes, provenance?.wasm?.bytes);
  assert.ok(Number.isSafeInteger(identity.bytes) && identity.bytes > 0);
  assert.equal(identity.processedBytes, identity.bytes);
  assert.equal(identity.maxBytes, 128 * 1024 * 1024);
  assert.ok(identity.bytes <= identity.maxBytes);
  assert.equal(identity.maxChunkBytes, 256 * 1024);
  assert.equal(identity.workerTerminated, true);
  for (const field of ["elapsedMs", "maximumRelayMs", "workerCpuMs"]) {
    assert.ok(Number.isFinite(identity[field]) && identity[field] >= 0, `missing ${field}`);
  }
  assert.equal(identity.reason, null);
  return identity;
}

// Cold boundary only. All writers use this same compact server/browser join.
// A failed fresh marker is evidence, but an older bundle cannot satisfy it.
export function writeQaTraceArtifact({ directory, label, state, serverLogs = [], provenance }) {
  assert.match(label, /^[a-z0-9_-]+$/, "QA artifact label must be a safe unique filename prefix");
  const capture = state?.qaTraceCapture ?? { status: "failed", enabled: null, ready: null,
    bundleId: null, bytes: 0, error: "browser was unavailable before artifact capture" };
  const audit = { label, ...capture, qaTracePath: null };
  if (capture.status !== "captured") return audit;
  assertCompleteQaTraceCaptures([capture]);
  const candidates = parseZubspaceEvents(serverLogs)
    .filter((entry) => correlatedTypes.has(entry?.event?.type));
  const events = [];
  let bytes = 2;
  for (let index = candidates.length - 1; index >= 0 && events.length < 512; index -= 1) {
    const nextBytes = Buffer.byteLength(JSON.stringify(candidates[index])) + (events.length ? 1 : 0);
    if (bytes + nextBytes > 128 * 1024) break;
    events.push(candidates[index]);
    bytes += nextBytes;
  }
  events.reverse();
  audit.qaTracePath = `${label}-qa-trace.json`;
  fs.writeFileSync(path.join(directory, audit.qaTracePath), `${JSON.stringify({
    schema: "zubspace-qa-trace-correlated-v1", version: 1, bundleId: capture.bundleId,
    provenance: { ...compactProvenance(provenance), capturedAt: provenance?.capturedAt,
      runStartedAt: provenance?.runStartedAt },
    server: { events, truncation: { omittedEvents: candidates.length - events.length,
      bytes, maxEvents: 512, maxBytes: 128 * 1024 } },
    browser: state.qaTraceBundle,
  }, null, 2)}\n`, { flag: "wx" });
  return audit;
}

// The file manifest proves bytes have not changed. This additionally proves
// those bytes are the declared fresh capture for this exact build and session.
export function validateQaTraceArtifacts(document, resultPath) {
  if (document.ok !== true) return null;
  const captures = document.successArtifacts?.qaTraceCaptureAudit ?? document.qaTraceCaptureAudit;
  if (captures === undefined && !document.successArtifacts && !document.browser) return null;
  const audit = assertCompleteQaTraceCaptures(captures);
  const expected = compactProvenance(document.provenance ?? document.successArtifacts?.provenance);
  const seen = new Set();
  for (const capture of captures) {
    if (capture.status === "disabled") {
      assert.equal(capture.qaTracePath, null, "disabled capture must not declare a trace path");
      continue;
    }
    assert.match(capture.qaTracePath ?? "", /^[a-z0-9_-]+-qa-trace\.json$/, "invalid QA trace path");
    assert.ok(!seen.has(capture.qaTracePath), "duplicate QA trace path");
    seen.add(capture.qaTracePath);
    const file = path.join(path.dirname(path.resolve(resultPath)), capture.qaTracePath);
    const stat = fs.lstatSync(file);
    assert.ok(stat.isFile() && !stat.isSymbolicLink(), "QA trace path must be a regular file");
    const bundle = JSON.parse(fs.readFileSync(file, "utf8"));
    assert.equal(bundle.schema, "zubspace-qa-trace-correlated-v1");
    assert.equal(bundle.version, 1);
    assert.equal(bundle.bundleId, capture.bundleId);
    assert.equal(bundle.browser?.schema, "zubspace-qa-trace-v1");
    assert.equal(bundle.browser.version, 1);
    assert.equal(bundle.browser.bundleId, capture.bundleId);
    assertQaServerBinaryProvenance(bundle.browser,
      document.provenance ?? document.successArtifacts?.provenance);
    assertQaWasmProvenance(bundle.browser,
      document.provenance ?? document.successArtifacts?.provenance);
    assert.equal(bundle.browser.limits?.maxBytes, 240 * 1024);
    assert.equal(Buffer.byteLength(JSON.stringify(bundle.browser)), capture.bytes);
    assert.ok(Array.isArray(bundle.server?.events) && bundle.server.events.length <= 512);
    assert.ok(Buffer.byteLength(JSON.stringify(bundle.server.events)) <= 128 * 1024);
    for (const [key, value] of Object.entries(expected)) assert.deepEqual(bundle.provenance?.[key], value);
  }
  return audit;
}

function resolveResultReference(parentResultPath, reference) {
  return path.resolve(path.dirname(path.resolve(parentResultPath)), reference);
}

// A successful QA drill is a parent result plus independently retained session
// results. The parent's file hash proves which child bytes it names; validate
// each child's own manifest, capture files, and exact build provenance too.
export function validateQaTraceChildResults(document, resultPath) {
  if (!Array.isArray(document?.automaticUploads)) return null;

  assertQaTraceCoverage(document);
  validateArtifactPathReferences(document, resultPath);
  const parentProvenance = document.provenance ?? document.successArtifacts?.provenance;
  assert.ok(parentProvenance, "QA trace parent is missing provenance");

  const expectedPaths = document.automaticUploads.map((entry) =>
    resolveResultReference(resultPath, entry.resultPath));
  assert.equal(new Set(expectedPaths).size, expectedPaths.length,
    "automatic uploads must retain distinct session results");
  const extraResultPaths = (document.extraLogPaths ?? [])
    .filter((entry) => path.basename(entry) === "result.json")
    .map((entry) => resolveResultReference(resultPath, entry));
  const declaredResultPaths = (document.retainedExternalPaths ?? [])
    .filter((entry) => path.basename(entry.path) === "result.json")
    .map((entry) => path.resolve(entry.path));
  const sorted = (values) => [...values].sort();
  assert.deepEqual(sorted(extraResultPaths), sorted(expectedPaths),
    "QA trace parent extraLogPaths must name every automatic-upload session result exactly once");
  assert.deepEqual(sorted(declaredResultPaths), sorted(expectedPaths),
    "QA trace parent must hash-bind every automatic-upload session result exactly once");

  return document.automaticUploads.map((upload, index) => {
    const childPath = expectedPaths[index];
    const child = JSON.parse(fs.readFileSync(childPath, "utf8"));
    assert.equal(child.ok, true, `${upload.mode} automatic-upload session result must pass`);
    assert.equal(child.mode, upload.mode, `${upload.mode} child result must retain its delivery mode`);
    assertArtifactProvenanceMatches(parentProvenance,
      child.provenance ?? child.successArtifacts?.provenance);
    const artifactPathAudit = validateArtifactPathReferences(child, childPath);
    const qaTraceAudit = validateQaTraceArtifacts(child, childPath);
    assert.ok(qaTraceAudit?.captured > 0,
      `${upload.mode} session result must retain its own fresh QA trace`);

    const uploadedBundle = upload.saved?.bundle;
    const childBundle = child.saved?.bundle;
    assert.ok(uploadedBundle?.bundleId, `${upload.mode} parent entry must name its uploaded bundle`);
    assert.equal(childBundle?.bundleId, uploadedBundle.bundleId,
      `${upload.mode} child result must retain the uploaded bundle`);
    assert.ok(child.successArtifacts?.qaTraceCaptureAudit?.some((capture) =>
      capture.status === "captured" && capture.bundleId === uploadedBundle.bundleId),
    `${upload.mode} child capture audit must match the uploaded bundle`);

    return { mode: upload.mode, resultPath: childPath, retainedFiles: artifactPathAudit.files,
      captures: qaTraceAudit.captured, provenanceMatchesParent: true };
  });
}

export function writeQaTraceFailureResult({ directory, provenance, captures }) {
  const qaTraceCaptureAudit = captures.map(({ label, state, serverLogs }) => {
    assert.match(label, /^[a-z0-9_-]+$/, "invalid QA artifact label");
    fs.writeFileSync(path.join(directory, `${label}-capture-state.json`), `${JSON.stringify(state, null, 2)}\n`);
    if (serverLogs) fs.writeFileSync(path.join(directory, `${label}-server.log`), serverLogs.join(""));
    return writeQaTraceArtifact({ directory, provenance, label, state, serverLogs });
  });
  fs.writeFileSync(path.join(directory, "result.json"), `${JSON.stringify({
    ok: false, provenance, qaTraceCaptureAudit, artifactFiles: artifactFileManifest(directory),
  }, null, 2)}\n`);
}
