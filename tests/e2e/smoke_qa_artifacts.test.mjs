import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { assertSmokeQaCaptures, createSmokeQaRecorder } from "./smoke_qa_artifacts.mjs";

test("real-server smoke WebGL probe cannot claim the game canvas before runtime startup", () => {
  const source = fs.readFileSync(new URL("../../tools/browser_real_server_smoke.mjs", import.meta.url), "utf8");
  const expression = source.match(/^\s+webgl: (.+),$/m)?.[1];
  assert.ok(expression, "exercise the actual pageState WebGL expression");
  const canvas = { getContext: () => assert.fail("a diagnostic must not create a canvas context") };
  const probe = (globals) => vm.runInNewContext(expression, globals);
  assert.equal(probe({ canvas }), false, "bundle not yet loaded");
  assert.equal(probe({ canvas, gl: undefined }), false, "runtime not initialized");
  assert.equal(probe({ canvas, gl: null }), false, "context creation failed");
  assert.equal(probe({ canvas, gl: { canvas } }), true, "existing game context");
  assert.equal(probe({ canvas, gl: { canvas: {} } }), false, "unrelated context");
});

test("ordinary smoke performs no QA work unless the parent requests retention", async () => {
  const recorder = createSmokeQaRecorder({});
  assert.equal(await recorder.capture("ordinary", { pageState: () => assert.fail("unexpected read") }), null);
  assert.deepEqual(recorder.audit(), []);
});

test("cache predecessor and successor captures survive a later unavailable page", async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-smoke-qa-test-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const recorder = createSmokeQaRecorder({ directory, label: "temporary-zone", provenance: {}, serverLogs: [] });
  const actor = (id) => ({
    events: [],
    freezeQaTrace: async () => ({ schema: "zubspace-qa-trace-v1", version: 1,
      bundleId: id.repeat(32), limits: { maxBytes: 240 * 1024 } }),
    pageState: async () => ({ qaTraceState: { enabled: true, ready: true, bundleId: id.repeat(32) } }),
  });
  await recorder.capture("cache-predecessor", actor("a"));
  await recorder.capture("alpha", actor("b"));
  await assert.rejects(recorder.capture("beta", null), /incomplete/);
  const index = JSON.parse(fs.readFileSync(path.join(directory, "temporary-zone-qa-index.json")));
  assert.deepEqual(index.qaTraceCaptureAudit.map((entry) => entry.status), ["captured", "captured", "failed"]);
  assert.notEqual(index.qaTraceCaptureAudit[0].bundleId, index.qaTraceCaptureAudit[1].bundleId);
  for (const entry of index.qaTraceCaptureAudit.slice(0, 2)) assert.ok(fs.existsSync(path.join(directory, entry.qaTracePath)));
  await recorder.capture("failure-beta", null, { required: false });
  assert.equal(recorder.audit().length, 4);
  const missing = JSON.parse(fs.readFileSync(path.join(directory, "temporary-zone-failure-beta-4-console.json")));
  assert.equal(missing.available, false);
  assert.equal(missing.complete, false);
});

test("dynamic zone acceptance requires both players, the predecessor, and default Chaos", () => {
  const roles = ["alpha-final", "beta-final", "cache-predecessor", "default-chaos"];
  const captures = roles.map((role, index) => ({ label: `zone-${role}-${index + 1}`,
    status: "captured", enabled: true, ready: true, bundleId: String(index).repeat(32),
    bytes: 100, error: null }));
  const args = { label: "zone", captures, cacheRejoin: true, discovery: true };
  assert.doesNotThrow(() => assertSmokeQaCaptures(args));
  assert.throws(() => assertSmokeQaCaptures({ ...args, captures: captures.slice(1) }), /missing/);
  assert.throws(() => assertSmokeQaCaptures({ ...args, captures: [captures[0], ...captures.slice(0, 3)] }), /reuse/);
  assert.throws(() => assertSmokeQaCaptures({ ...args,
    captures: captures.map((entry) => ({ ...entry, label: entry.label.replace("default-chaos", "unknown") })) }), /default-chaos/);
});

test("cold smoke capture retains bounded console errors before joined QA is available", async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-smoke-qa-test-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const recorder = createSmokeQaRecorder({ directory, label: "startup", provenance: {}, serverLogs: [] });
  const events = Array.from({ length: 300 }, (_, index) => ({
    type: "error", text: `${index}: ${index === 0 ? "RuntimeError: unreachable " : ""}${"x".repeat(2200)}`,
  }));
  await recorder.capture("failed", {
    events,
    pageState: async () => ({ uiMetrics: null }),
  }, { required: false });
  const retained = JSON.parse(fs.readFileSync(path.join(directory, "startup-failed-1-console.json")));
  assert.equal(retained.available, true);
  assert.equal(retained.complete, true);
  assert.equal(retained.total, 300);
  assert.equal(retained.errors, 300);
  assert.equal(retained.wasmPanics, 1, "the evicted trap remains counted");
  assert.equal(retained.omitted, 44);
  assert.equal(retained.events.length, 256);
  assert.ok(retained.events[0].text.startsWith("44: "));
  assert.ok(retained.events.every((entry) => entry.text.length <= 2048 && entry.textTruncated));
  assert.ok(events[0].text.length > 2200, "capture cannot mutate the live event buffer");
  assert.equal(recorder.audit()[0].status, "failed");
});

for (const [kind, event, error] of [
  ["panic", { type: "exception", text: "RuntimeError: unreachable at key_up" }, /WASM panic/],
  ["rejected-draw", { type: "warning", text: "Entire call rejected: too many vertices" }, /rejected a draw/],
]) {
  test(`required smoke capture cannot pass with a retained ${kind}`, async t => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-smoke-qa-test-"));
    t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
    const recorder = createSmokeQaRecorder({ directory, label: "verdict", provenance: {}, serverLogs: [] });
    const actor = { events: [event],
      freezeQaTrace: async () => ({ schema: "zubspace-qa-trace-v1", version: 1,
        bundleId: "a".repeat(32), limits: { maxBytes: 240 * 1024 } }),
      pageState: async () => ({ qaTraceState: { enabled: true, ready: true, bundleId: "a".repeat(32) } }),
    };
    await assert.rejects(recorder.capture("alpha", actor), error);
    assert.ok(fs.existsSync(path.join(directory, "verdict-alpha-1-console.json")), "failed evidence is preserved");
  });
}
