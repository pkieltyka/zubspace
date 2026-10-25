import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { assertFocusedConnectionContinuity, createFocusedBrowserEvidence } from "./focused_browser_evidence.mjs";
import { validateArtifactPathReferences } from "./artifact_paths.mjs";
import { validateQaTraceArtifacts } from "./qa_trace_artifacts.mjs";

const connected = () => [
  { message: "rtc connection state", detail: { state: "connecting" } },
  { message: "rtc connection state", detail: { state: "connected" } },
  ...["zubspace-game", "zubspace-events", "zubspace-reliable"].map((label) => ({
    message: "rtc data channel open", detail: { label },
  })),
];

function fixture(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-focused-evidence-test-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const artifactDir = path.join(directory, "success");
  const calls = [];
  const identities = [];
  const provenance = { formatVersion: 7, gitHead: "a".repeat(40),
    workingTreeDiffSha256: "b".repeat(64), runStartedAt: "test-start",
    serverBinary: { sha256: "c".repeat(64), contentSha256: "1".repeat(64), bytes: 42 },
    wasm: { sha256: "d".repeat(64), contentSha256: "2".repeat(64), bytes: 24 } };
  const bundle = { schema: "zubspace-qa-trace-v1", version: 1, bundleId: "e".repeat(32),
    limits: { maxBytes: 240 * 1024 }, provenance: {
      serverBinary: { kind: "running-executable-sha256", status: "ready",
        sha256: provenance.serverBinary.contentSha256, bytes: 42 },
      wasm: { kind: "loaded-wasm-sha256", status: "ready", sha256: provenance.wasm.contentSha256,
        bytes: 24, processedBytes: 24, maxBytes: 128 * 1024 * 1024, maxChunkBytes: 256 * 1024,
        workerTerminated: true, elapsedMs: 2, maximumRelayMs: 0.1, workerCpuMs: 1, reason: null },
    } };
  const actor = { freezeQaTrace: async () => bundle,
    pageState: async () => ({ networkLog: connected(), qaTraceState: { enabled: true, ready: true, bundleId: bundle.bundleId } }),
    screenshot: async () => Buffer.from("test screenshot").toString("base64") };
  const ordinary = { freezeQaTrace: async () => null,
    pageState: async () => ({ networkLog: connected(), qaTraceState: { enabled: false, ready: false } }) };
  const capture = { server: { logs: [] }, serverPort: 1234, staticServer: { port: 5678 },
    urls: ["http://localhost/alpha", "http://localhost/beta"],
    clients: [{ label: "alpha", client: actor }, { label: "beta", client: ordinary }],
    extraLogs: { "alpha-browser": ["browser log\n"], "beta-browser": [] },
    // The drain the delivery audit asserts is disconnect-cleanup evidence, so the capture has to hand
    // over the sessions that produce the traffic.
    disconnect: async () => { calls.push("disconnect"); } };
  const operations = {
    collectArtifactProvenance: (options) => { identities.push(options); calls.push("provenance"); return structuredClone(provenance); },
    collectArtifactProvenanceAsync: async (options) => operations.collectArtifactProvenance(options),
    assertDataChannelDeliveryAudit: async (args) => {
      // Ordered deliberately: the real audit ends the sessions first and only then reads the queues, so
      // recording the disconnect before this call is what pins the hand-over.
      await args.disconnect();
      calls.push("delivery");
      assert.equal(args.server, capture.server);
      assert.equal(args.serverPort, capture.serverPort);
      assert.deepEqual(args.browserLogs, Object.values(capture.extraLogs));
      return { testedDelivery: true };
    },
    assertSecurityRejectAudit: (server) => {
      calls.push("security"); assert.equal(server, capture.server); return { total: 0 };
    },
  };
  const create = (identityOptions = {}) => createFocusedBrowserEvidence({
    e2eScript: "tests/e2e/example.mjs", artifactDir, ...identityOptions,
  }, operations);
  return { artifactDir, calls, identities, provenance, capture, operations, create };
}

test("an explicit phase directory overrides a populated environment artifact root", (t) => {
  const f = fixture(t);
  const parent = path.dirname(f.artifactDir);
  fs.writeFileSync(path.join(parent, "previous-phase.txt"), "retained evidence");
  const previous = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR;
  process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR = parent;
  t.after(() => {
    if (previous === undefined) delete process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR;
    else process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR = previous;
  });
  assert.doesNotThrow(() => f.create());
});

test("focused success retains both actors, explicit trace modes, audits, and a verified manifest", async (t) => {
  const f = fixture(t);
  const evidence = f.create();
  assert.deepEqual(f.calls, ["provenance"], "initial identity precedes service startup");
  const result = await evidence.succeed({ ok: true, transport: "rtc", events: [{ damage: 123 }] }, f.capture);
  const resultPath = path.join(f.artifactDir, "result.json");
  assert.deepEqual(JSON.parse(fs.readFileSync(resultPath)), result);
  assert.deepEqual(result.events, [{ damage: 123 }]);
  assert.deepEqual(result.dataChannelDeliveryAudit, { testedDelivery: true });
  assert.deepEqual(result.securityAudit, { total: 0 });
  // Disconnect-cleanup evidence closes the gate: capture and continuity read the live pages, then the
  // sessions end, then the drain is asserted.
  assert.deepEqual(f.calls, ["provenance", "security", "provenance", "provenance", "disconnect", "delivery", "provenance", "security"]);
  assert.deepEqual(validateQaTraceArtifacts(result, resultPath), { ok: true, captured: 1, disabled: 1 });
  assert.ok(validateArtifactPathReferences(result, resultPath));
  assert.ok(result.successArtifacts.artifactFiles.some((file) => file.path === "alpha.png"));
  assert.ok(result.successArtifacts.artifactFiles.some((file) => file.path === "beta-state.json"));
});

test("focused evidence binds the explicitly served release bundle at every identity capture", async (t) => {
  const f = fixture(t);
  const identities = { wasm: "dist/zubspace.wasm", clientEntry: "dist/index.html", serverBinary: "target/debug/zubspace_server" };
  await f.create(identities).succeed({ ok: true }, f.capture);
  assert.ok(f.identities.length >= 3);
  for (const captured of f.identities) {
    for (const [key, value] of Object.entries(identities)) assert.equal(captured[key], value);
    assert.equal(captured.e2eScript, "tests/e2e/example.mjs");
  }
});

test("channel and security failures cannot publish success", async (t) => {
  for (const operation of ["assertDataChannelDeliveryAudit", "assertSecurityRejectAudit"]) {
    const f = fixture(t);
    f.operations[operation] = () => { throw new Error(`rejected ${operation}`); };
    await assert.rejects(f.create().succeed({ ok: true }, f.capture), /rejected/);
    assert.equal(fs.existsSync(path.join(f.artifactDir, "result.json")), false);
  }
});

test("finalization cannot turn a failed scenario or empty client set into success", async (t) => {
  const f = fixture(t);
  const evidence = f.create();
  await assert.rejects(evidence.succeed({ ok: false }, f.capture), /independently pass/);
  assert.deepEqual(f.calls, ["provenance"]);
  await assert.rejects(evidence.succeed({ ok: true }, { ...f.capture, clients: [] }), /requires client connection/);
  assert.equal(fs.existsSync(path.join(f.artifactDir, "result.json")), false);
});

test("a capture that cannot hand over its sessions cannot publish success", async (t) => {
  // The drain the delivery audit requires is disconnect-cleanup evidence: asserting it against a client
  // that is still streaming measures luck. A focused gate that cannot end its own sessions therefore has
  // to be refused before anything is published, not quietly audited while still connected.
  const f = fixture(t);
  await assert.rejects(
    f.create().succeed({ ok: true }, { ...f.capture, disconnect: undefined }),
    /hand its own browser sessions/,
  );
  assert.equal(fs.existsSync(path.join(f.artifactDir, "result.json")), false);
  assert.deepEqual(f.calls, ["provenance"]);
});

test("source drift before or during cold capture rejects success", async (t) => {
  for (const captureDrift of [false, true]) {
    const f = fixture(t);
    const evidence = f.create();
    if (captureDrift) f.capture.clients[0].client.freezeQaTrace = async () => {
      f.provenance.gitHead = "f".repeat(40); return null;
    };
    else f.provenance.gitHead = "f".repeat(40);
    await assert.rejects(evidence.succeed({ ok: true }, f.capture), /provenance drifted/);
    assert.equal(fs.existsSync(path.join(f.artifactDir, "result.json")), false);
  }
});

test("capture and disconnect native tail is retained and hashed without replacing the initial log", async (t) => {
  const f = fixture(t);
  f.capture.server.logs.push("before capture\n");
  const originalFreeze = f.capture.clients[0].client.freezeQaTrace;
  f.capture.clients[0].client.freezeQaTrace = async () => {
    f.capture.server.logs.push("during QA capture\n");
    return originalFreeze();
  };
  f.capture.disconnect = async () => { f.calls.push("disconnect"); f.capture.server.logs.push("disconnect cleanup\n"); };
  const result = await f.create().succeed({ ok: true }, f.capture);
  assert.equal(fs.readFileSync(path.join(f.artifactDir, "server.log"), "utf8"), "before capture\nduring QA capture\n");
  assert.equal(fs.readFileSync(path.join(f.artifactDir, "server-capture-and-cleanup.log"), "utf8"), "during QA capture\ndisconnect cleanup\n");
  assert.deepEqual(result.captureLogScope, { startChunk: 1, endChunk: 3,
    phase: "artifact-capture-start-through-final-disconnect-audit", overlapsInitialServerLog: true });
  assert.ok(result.successArtifacts.extraLogPaths.includes("server-capture-and-cleanup.log"));
  assert.ok(result.successArtifacts.artifactFiles.some(row => row.path === "server-capture-and-cleanup.log" && /^[a-f0-9]{64}$/.test(row.sha256)));
  assert.ok(validateArtifactPathReferences(result, path.join(f.artifactDir, "result.json")));
  assert.equal(f.calls.filter(call => call === "security").length, 2);
});

test("source drift during the final disconnect audit cannot publish success", async (t) => {
  const f = fixture(t), original = f.operations.assertDataChannelDeliveryAudit;
  f.operations.assertDataChannelDeliveryAudit = async args => {
    const result = await original(args);
    f.provenance.gitHead = "f".repeat(40);
    return result;
  };
  await assert.rejects(f.create().succeed({ ok: true }, f.capture), /provenance drifted/);
  assert.equal(fs.existsSync(path.join(f.artifactDir, "result.json")), false);
});

test("a security rejection arising during capture cannot be hidden by the early audit", async (t) => {
  const f = fixture(t);
  let audits = 0;
  f.operations.assertSecurityRejectAudit = () => {
    if (++audits === 2) throw new Error("late security rejection");
    return { total: 0 };
  };
  await assert.rejects(f.create().succeed({ ok: true }, f.capture), /late security rejection/);
  assert.equal(audits, 2);
  assert.equal(fs.existsSync(path.join(f.artifactDir, "result.json")), false);
});

test("an unavailable second actor cannot be hidden by a valid first capture", async (t) => {
  const f = fixture(t);
  f.capture.clients[1].client = null;
  await assert.rejects(f.create().succeed({ ok: true }, f.capture), /missing its connection log/);
  assert.equal(fs.existsSync(path.join(f.artifactDir, "result.json")), false);
});

test("clean focused acceptance rejects transient disconnect/reconnect and missing channel evidence", () => {
  assert.equal(assertFocusedConnectionContinuity([{ label: "actor", networkLog: connected() }]).length, 1);
  for (const state of ["disconnected", "failed", "closed", "unknown"]) {
    const networkLog = [...connected(), { message: "rtc connection state", detail: { state } },
      { message: "rtc connection state", detail: { state: "connected" } }];
    assert.throws(() => assertFocusedConnectionContinuity([{ label: "actor", networkLog }]), /lost its RTC connection/);
  }
  assert.throws(() => assertFocusedConnectionContinuity([{ label: "actor", networkLog: connected().slice(0, -1) }]), /missing zubspace-reliable/);
  assert.throws(() => assertFocusedConnectionContinuity([{ label: "actor", networkLog: [] }]), /not connected/);
});

test("failure retains its cause, both actor dispositions, initial identity, and all logs in its directory", async (t) => {
  const f = fixture(t);
  const evidence = f.create();
  f.capture.clients[1].client = null;
  const artifacts = await evidence.fail(new Error("original gameplay assertion"), f.capture);
  const resultPath = path.join(`${f.artifactDir}-failure`, "result.json");
  const result = JSON.parse(fs.readFileSync(resultPath));
  assert.equal(result.ok, false);
  assert.match(result.error, /original gameplay assertion/);
  assert.deepEqual(result.initialProvenance, f.provenance);
  assert.deepEqual(result.failureArtifacts.qaTraceCaptureAudit.map((entry) => entry.status), ["captured", "failed"]);
  assert.ok(validateArtifactPathReferences(result, resultPath));
  assert.equal(artifacts.absoluteArtifactDir, `${f.artifactDir}-failure`);
  assert.match(fs.readFileSync(path.join(artifacts.absoluteArtifactDir, "failure.log"), "utf8"), /original gameplay assertion/);
  await assert.rejects(evidence.fail(new Error("second failure"), f.capture), /already exists/);
  assert.match(JSON.parse(fs.readFileSync(resultPath)).error, /original gameplay assertion/);
});

test("artifact root retains default browser evidence outside ephemeral Nix TMPDIR", async t => {
  const f = fixture(t);
  const previousRoot = process.env.ZUBSPACE_E2E_ARTIFACT_ROOT;
  const previousSuccess = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR;
  process.env.ZUBSPACE_E2E_ARTIFACT_ROOT = path.dirname(f.artifactDir);
  delete process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR;
  try {
    const evidence = createFocusedBrowserEvidence({ e2eScript: "tests/e2e/nix-retention.mjs" }, f.operations);
    await evidence.succeed({ ok: true }, f.capture);
    const retained = fs.readdirSync(path.dirname(f.artifactDir)).find(name => name.startsWith("zubspace-nix-retention-"));
    assert.ok(retained, "browser evidence must use the configured persistent root");
    assert.equal(JSON.parse(fs.readFileSync(path.join(path.dirname(f.artifactDir), retained, "result.json"))).ok, true);
  } finally {
    if (previousRoot === undefined) delete process.env.ZUBSPACE_E2E_ARTIFACT_ROOT;
    else process.env.ZUBSPACE_E2E_ARTIFACT_ROOT = previousRoot;
    if (previousSuccess === undefined) delete process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR;
    else process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR = previousSuccess;
  }
});
