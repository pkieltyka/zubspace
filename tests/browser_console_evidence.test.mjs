import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import { createConsoleRecorder, browserConsoleEvidence, assertRendererConsoleClean,
  openFirefoxConsole } from "../tools/browser_console_evidence.mjs";

test("the retained menu key-up WASM borrow panics reject a successful renderer verdict", () => {
  const captured = JSON.parse(fs.readFileSync(new URL("./fixtures/menu_keyup_panic_42b85eb.json", import.meta.url)));
  assert.equal(captured.console.errors, 3);
  assert.throws(() => assertRendererConsoleClean(captured.console), /WASM panic/);
});

test("a WASM panic remains fatal after bounded event eviction and text truncation", () => {
  const recorder = createConsoleRecorder("fixture");
  recorder.record({ type: "exception", text: "x".repeat(2200) + " RuntimeError: memory access out of bounds" });
  for (let n = 0; n < 300; n++) recorder.record({ type: "warning", text: `harmless ${n}` });
  const evidence = recorder.snapshot();
  assert.equal(evidence.wasmPanics, 1);
  assert.equal(evidence.wasmPanicExamples[0].textTruncated, true);
  assert.equal(evidence.events.some(e => e.type === "exception"), false);
  assert.throws(() => assertRendererConsoleClean(evidence), /WASM panic/);
});

test("a rejected draw survives warning floods and text truncation", () => {
  const recorder = createConsoleRecorder("fixture");
  recorder.record({ type: "warning", text: "x".repeat(2200) + "; Entire call rejected" });
  for (let n = 0; n < 300; n++) recorder.record({ type: "warning", text: `harmless ${n}` });
  const evidence = recorder.snapshot();
  assert.equal(evidence.total, 301);
  assert.equal(evidence.events.length, 256);
  assert.equal(evidence.omitted, 45);
  assert.equal(evidence.rejectedDraws, 1);
  assert.equal(evidence.rejectionExamples[0].textTruncated, true);
  assert.throws(() => assertRendererConsoleClean(evidence), /renderer rejected a draw/);
  evidence.events[0].text = "mutated";
  assert.notEqual(recorder.snapshot().events[0].text, "mutated");
});

test("a missing or disconnected console collector is not a clean rendering verdict", () => {
  assert.throws(() => assertRendererConsoleClean(browserConsoleEvidence({})), /missing/);
  assert.throws(() => assertRendererConsoleClean({ available: true, complete: false }), /incomplete/);
  const evidence = browserConsoleEvidence({ events: [{ type: "warning", text: "ordinary warning" }] });
  assert.equal(evidence.warnings, 1);
  assert.equal(evidence.rejectedDraws, 0);
  assertRendererConsoleClean(evidence);
});

class Socket {
  static last;
  constructor() { Socket.last = this; queueMicrotask(() => this.onopen()); }
  send(raw) {
    const request = JSON.parse(raw);
    assert.deepEqual(request, { id: 1, method: "session.subscribe", params: { events: ["log.entryAdded"] } });
    queueMicrotask(() => this.emit({ type: "success", id: 1, result: { subscription: "logs" } }));
  }
  emit(message) { this.onmessage({ data: JSON.stringify(message) }); }
  close() { this.closed = true; this.onclose?.(); }
}

test("Firefox subscribes before use and retains real BiDi warning messages", async () => {
  const client = await openFirefoxConsole("ws://127.0.0.1:1234/session/test", { WebSocketImpl: Socket });
  const socket = Socket.last;
  socket.emit({ type: "event", method: "log.entryAdded", params: { level: "warn", text: "geometry: Entire call rejected" } });
  assert.equal(client.snapshot().rejectedDraws, 1);
  assert.equal(client.events[0].type, "warning");
  assert.throws(() => assertRendererConsoleClean(client.snapshot()), /rejected a draw/);
  client.close();
  assert.equal(client.snapshot().complete, true);
});

test("Firefox subscription failure, timeout and unexpected closure fail closed", async () => {
  class Rejected extends Socket {
    send() { queueMicrotask(() => this.emit({ type: "error", id: 1, error: "unknown command", message: "no logs" })); }
  }
  await assert.rejects(openFirefoxConsole("ws://localhost:1234/session/test", { WebSocketImpl: Rejected }), /no logs/);
  assert.equal(Socket.last.closed, true);
  class Silent extends Socket { send() {} }
  await assert.rejects(openFirefoxConsole("ws://localhost:1234/session/test", { WebSocketImpl: Silent, timeoutMs: 10 }), /timed out/);
  assert.equal(Socket.last.closed, true);
  const client = await openFirefoxConsole("ws://localhost:1234/session/test", { WebSocketImpl: Socket });
  Socket.last.onclose();
  assert.throws(() => assertRendererConsoleClean(client.snapshot()), /closed unexpectedly/);
  client.close();
  await assert.rejects(openFirefoxConsole("ws://untrusted.example/session/test", { WebSocketImpl: Socket }), /local WebDriver/);
});
