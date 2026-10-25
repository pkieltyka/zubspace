import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { captureBrowserArtifactState } from "./qa_trace_capture.mjs";
import { reportBrowserE2eFailure, saveBrowserE2eArtifacts } from "./combat_replay.mjs";
import { assertQaServerBinaryProvenance, assertQaWasmProvenance, validateQaTraceArtifacts,
  validateQaTraceChildResults, writeQaTraceArtifact, writeQaTraceFailureResult } from "./qa_trace_artifacts.mjs";
import { artifactFileManifest, validateArtifactPathReferences } from "./artifact_paths.mjs";

const provenance = { gitHead: "a".repeat(40), workingTreeDiffSha256: "b".repeat(64),
  serverBinary: { sha256: "c".repeat(64), contentSha256: "1".repeat(64), bytes: 42 },
  wasm: { sha256: "d".repeat(64), contentSha256: "2".repeat(64), bytes: 24 } };

test("failure footer remains discoverable after Error.stack has been cached", (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-error-footer-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const error = new Error("retained motion invariant failed");
  const originalStack = error.stack;
  const artifacts = {
    absoluteArtifactDir: directory, urls: [], extraLogPaths: [],
    serverLogPath: "server.log", staticLogPath: "static.json",
    serverEventsPath: "server-events.json", serverDamageEventsPath: "damage.json",
    provenancePath: "provenance.json", clientUrlsPath: "client-urls.txt",
  };
  const emitted = [];
  reportBrowserE2eFailure(error, artifacts, (line) => emitted.push(line));
  assert.equal(error.stack, originalStack);
  assert.equal(error.message, "retained motion invariant failed");
  assert.equal(fs.readFileSync(path.join(directory, "failure.txt"), "utf8"), `${originalStack}\n`);
  assert.equal(emitted.length, 1);
  assert.ok(emitted[0].split("\n").includes(`artifacts: ${directory}`));
  assert.ok(emitted[0].includes(`server log: ${path.join(directory, "server.log")}`));
});

test("common default-provenance capture keeps the parent log-draining loop responsive", async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-qa-default-provenance-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  let heartbeats = 0;
  const timer = setInterval(() => { heartbeats += 1; }, 10);
  try {
    const retained = await saveBrowserE2eArtifacts({
      artifactDir: path.join(directory, "capture"), server: { logs: [] },
      clients: [{ label: "actor", client: {
        freezeQaTrace: async () => null,
        pageState: async () => ({ qaTraceState: { enabled: false, ready: false } }),
      } }],
    });
    assert.ok(heartbeats > 0, "identity collection blocked the parent runner's log readers");
    assert.match(retained.provenance.workingTreeDiffSha256, /^[0-9a-f]{64}$/);
    assert.equal(retained.qaTraceCaptureAudit[0].status, "disabled");
  } finally {
    clearInterval(timer);
  }
});

test("common failure writer retains an unavailable actor instead of silently omitting it", async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-qa-unavailable-test-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const artifactDir = path.join(directory, "failure");
  const retained = await saveBrowserE2eArtifacts({ artifactDir, server: { logs: [] },
    clients: [{ label: "actor", client: null }], provenance });
  assert.equal(retained.qaTraceCaptureAudit.length, 1);
  assert.equal(retained.qaTraceCaptureAudit[0].status, "failed");
  assert.equal(retained.qaTraceCaptureAudit[0].qaTracePath, null);
  const state = JSON.parse(fs.readFileSync(path.join(artifactDir, "actor-state.json"), "utf8"));
  assert.match(state.qaTraceCapture.error, /missing QA trace capture adapter/);
  assert.equal(state.qaTraceBundle, null);
  const resources = JSON.parse(fs.readFileSync(path.join(artifactDir, retained.serverProcessPath), "utf8"));
  assert.equal(resources.available, false, "an exited/missing server cannot report zero resource use");
  assert.match(resources.error, /PID unavailable/);
});

async function fixture(t, bundleId = "e".repeat(32)) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-qa-artifact-test-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const bundle = { schema: "zubspace-qa-trace-v1", version: 1, bundleId,
    provenance: { wasm: { kind: "loaded-wasm-sha256", status: "ready",
      sha256: provenance.wasm.contentSha256, bytes: 24, processedBytes: 24,
      maxBytes: 128 * 1024 * 1024, maxChunkBytes: 256 * 1024, workerTerminated: true,
      elapsedMs: 3, maximumRelayMs: 0.1, workerCpuMs: 1, reason: null },
    serverBinary: { kind: "running-executable-sha256", status: "ready",
      sha256: provenance.serverBinary.contentSha256, bytes: provenance.serverBinary.bytes } },
    limits: { maxBytes: 240 * 1024 }, truncated: true };
  const state = await captureBrowserArtifactState({ freezeQaTrace: async () => bundle,
    pageState: async () => ({ qaTraceState: { enabled: true, ready: true, bundleId: bundle.bundleId } }) });
  return { directory, state };
}

test("running binary identity binds content bytes, not the later deployment path or a build label", async (t) => {
  const { state } = await fixture(t);
  assertQaServerBinaryProvenance(state.qaTraceBundle, provenance);
  const original = state.qaTraceBundle.provenance.serverBinary;
  for (const changed of [null, { ...original, status: "unavailable" },
    { ...original, sha256: provenance.serverBinary.sha256 },
    { ...original, bytes: original.bytes + 1 }]) {
    assert.throws(() => assertQaServerBinaryProvenance({ provenance: { serverBinary: changed } }, provenance));
  }
});

test("loaded-WASM identity rejects a build label, partial bytes, changed content or live worker", async (t) => {
  const { state } = await fixture(t);
  assertQaWasmProvenance(state.qaTraceBundle, provenance);
  const original = state.qaTraceBundle.provenance.wasm;
  for (const changed of [null, { ...original, status: "pending" },
    { ...original, sha256: provenance.wasm.sha256 }, { ...original, processedBytes: 1 },
    { ...original, workerTerminated: false }, { ...original, workerCpuMs: null }]) {
    assert.throws(() => assertQaWasmProvenance({ provenance: { wasm: changed } }, provenance));
  }
});

test("custom and common successes must retain the exact declared bundle and provenance", async (t) => {
  const { directory, state } = await fixture(t);
  const audit = writeQaTraceArtifact({ directory, label: "cycle-01", state, serverLogs: [], provenance });
  const resultPath = path.join(directory, "result.json");
  for (const document of [
    { ok: true, provenance, qaTraceCaptureAudit: [audit] },
    { ok: true, provenance, successArtifacts: { qaTraceCaptureAudit: [audit] } },
  ]) assert.equal(validateQaTraceArtifacts(document, resultPath).captured, 1);
  const file = path.join(directory, audit.qaTracePath);
  const original = JSON.parse(fs.readFileSync(file));
  for (const changed of [
    { ...original, bundleId: "f".repeat(32) },
    { ...original, browser: { ...original.browser, version: 2 } },
    { ...original, provenance: { ...original.provenance, wasmSha256: "wrong-build" } },
  ]) {
    fs.writeFileSync(file, JSON.stringify(changed));
    assert.throws(() => validateQaTraceArtifacts({ ok: true, provenance, qaTraceCaptureAudit: [audit] }, resultPath));
  }
  fs.unlinkSync(file);
  assert.throws(() => validateQaTraceArtifacts({ ok: true, provenance, qaTraceCaptureAudit: [audit] }, resultPath));
});

test("failed captures remain explicit and cannot publish an older bundle as fresh", async (t) => {
  const { directory, state } = await fixture(t);
  state.qaTraceCapture.status = "failed";
  state.qaTraceCapture.error = "browser closed";
  const audit = writeQaTraceArtifact({ directory, label: "browser", state, serverLogs: [], provenance });
  assert.equal(audit.qaTracePath, null);
  assert.deepEqual(fs.readdirSync(directory), []);
  assert.throws(() => validateQaTraceArtifacts({ ok: true, provenance, qaTraceCaptureAudit: [audit] }, path.join(directory, "result.json")), /incomplete/);
  assert.equal(validateQaTraceArtifacts({ ok: false, provenance, qaTraceCaptureAudit: [audit] }, path.join(directory, "result.json")), null);
});

test("trace paths cannot escape the artifact directory or overwrite another capture", async (t) => {
  const args = { ...await fixture(t), label: "browser", serverLogs: [], provenance };
  const audit = writeQaTraceArtifact(args);
  assert.throws(() => writeQaTraceArtifact(args), /EEXIST/);
  assert.throws(() => writeQaTraceArtifact({ ...args, label: "../outside" }), /label/);
  for (const qaTracePath of ["../outside.json", "/tmp/outside.json", null]) {
    assert.throws(() => validateQaTraceArtifacts({ ok: true, provenance,
      qaTraceCaptureAudit: [{ ...audit, qaTracePath }] }, path.join(args.directory, "result.json")), /path/i);
  }
});

test("server evidence is a chronological bounded tail with disclosed omissions", async (t) => {
  const args = { ...await fixture(t), label: "browser", provenance };
  const serverLogs = Array.from({ length: 800 }, (_, tick) => `zubspace_event ${JSON.stringify({
    type: "server_collision_witness", tick, witness: "x".repeat(500),
  })}\n`);
  const audit = writeQaTraceArtifact({ ...args, serverLogs });
  const bundle = JSON.parse(fs.readFileSync(path.join(args.directory, audit.qaTracePath)));
  assert.ok(bundle.server.events.length < 512);
  assert.equal(bundle.server.events.at(-1).event.tick, 799);
  assert.equal(bundle.server.truncation.omittedEvents, 800 - bundle.server.events.length);
  assert.equal(bundle.server.truncation.bytes, Buffer.byteLength(JSON.stringify(bundle.server.events)));
  assert.ok(bundle.server.truncation.bytes <= 128 * 1024);
});

test("failure publication preserves unavailable and captured clients with an exact file manifest", async (t) => {
  const { directory, state } = await fixture(t);
  writeQaTraceFailureResult({ directory, provenance, captures: [
    { label: "ready", state, serverLogs: [] },
    { label: "closed", state: await captureBrowserArtifactState(null), serverLogs: [] },
  ] });
  const resultPath = path.join(directory, "result.json");
  const document = JSON.parse(fs.readFileSync(resultPath));
  assert.equal(document.ok, false);
  assert.equal(document.qaTraceCaptureAudit[0].status, "captured");
  assert.equal(document.qaTraceCaptureAudit[1].status, "failed");
  assert.ok(validateArtifactPathReferences(document, resultPath));
});

test("browser successes cannot omit the audit and disabled captures cannot claim a file", async (t) => {
  const { directory } = await fixture(t);
  const resultPath = path.join(directory, "result.json");
  assert.throws(() => validateQaTraceArtifacts({ ok: true, browser: "firefox" }, resultPath), /missing/);
  const state = await captureBrowserArtifactState({ freezeQaTrace: async () => null,
    pageState: async () => ({ qaTraceState: { enabled: false, ready: false } }) });
  const capture = writeQaTraceArtifact({ directory, label: "ordinary", state, provenance });
  assert.equal(validateQaTraceArtifacts({ ok: true, qaTraceCaptureAudit: [capture] }, resultPath).disabled, 1);
  assert.throws(() => validateQaTraceArtifacts({ ok: true,
    qaTraceCaptureAudit: [{ ...capture, qaTracePath: "old-qa-trace.json" }] }, resultPath), /disabled/);
});

test("QA trace parents validate each retained automatic-upload session manifest and provenance", async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-qa-child-results-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const parentDirectory = path.join(directory, "parent");
  fs.mkdirSync(parentDirectory);
  const uploads = [];
  const resultPaths = [];
  const retainedExternalPaths = [];

  for (const mode of ["clean", "dropped"]) {
    const childDirectory = path.join(directory, mode);
    fs.mkdirSync(childDirectory);
    const { state } = await fixture(t, mode === "clean" ? "a".repeat(32) : "b".repeat(32));
    const capture = writeQaTraceArtifact({ directory: childDirectory, label: "actor", state,
      serverLogs: [], provenance });
    const child = { ok: true, mode, saved: { bundle: state.qaTraceBundle }, provenance,
      successArtifacts: { qaTraceCaptureAudit: [capture], artifactFiles: artifactFileManifest(childDirectory) } };
    const resultPath = path.join(childDirectory, "result.json");
    fs.writeFileSync(resultPath, `${JSON.stringify(child, null, 2)}\n`);
    const manifestEntry = artifactFileManifest(childDirectory, { exclude: [] })
      .find((entry) => entry.path === "result.json");
    assert.ok(manifestEntry, "child result is included in the parent external-file manifest");
    resultPaths.push(resultPath);
    retainedExternalPaths.push({ ...manifestEntry, path: resultPath });
    uploads.push({ mode, resultPath, saved: child.saved });
  }

  const parent = { ok: true, provenance, automaticUploads: uploads, extraLogPaths: resultPaths,
    retainedExternalPaths, clean: { bundleId: "c".repeat(32) }, forcedFailure: { bundleId: "c".repeat(32) },
    disabled: { bundleId: null, bytes: 0 },
    successArtifacts: { qaTraceCaptureAudit: [{ bundleId: "c".repeat(32), bytes: 1 }], artifactFiles: [] } };
  const parentResultPath = path.join(parentDirectory, "result.json");
  const audit = validateQaTraceChildResults(parent, parentResultPath);
  assert.deepEqual(audit.map((entry) => entry.mode), ["clean", "dropped"]);
  assert.ok(audit.every((entry) => entry.provenanceMatchesParent && entry.captures === 1));

  const child = JSON.parse(fs.readFileSync(resultPaths[1], "utf8"));
  child.provenance = { ...child.provenance, gitHead: "f".repeat(40) };
  fs.writeFileSync(resultPaths[1], `${JSON.stringify(child, null, 2)}\n`);
  const changedManifest = artifactFileManifest(path.dirname(resultPaths[1]), { exclude: [] })
    .find((entry) => entry.path === "result.json");
  retainedExternalPaths[1] = { ...changedManifest, path: resultPaths[1] };
  assert.throws(() => validateQaTraceChildResults(parent, parentResultPath), /gitHead|provenance/);
});
