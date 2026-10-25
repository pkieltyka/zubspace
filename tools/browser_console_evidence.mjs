import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";

const MAX_EVENTS = 256;
const MAX_TEXT = 2048;
const MAX_REJECTION_EXAMPLES = 8;

export function isRejectedDraw(event) {
  return /\bEntire call rejected\b/i.test(String(event?.text || ""));
}

export function isWasmPanic(event) {
  return /\bRuntimeError:\s*(?:unreachable|memory access out of bounds|out of bounds|uninitialized element|indirect call|call_indirect|divide by zero|integer overflow)|\bpanicked at\b/i
    .test(String(event?.text || ""));
}

function compactEvent(event) {
  const text = String(event?.text || "");
  return { type: String(event?.type || "unknown").slice(0, 32),
    text: text.slice(0, MAX_TEXT), textTruncated: text.length > MAX_TEXT };
}

// Count every event before bounded retention. A rejection cannot disappear
// behind a later flood of harmless warnings or text truncation.
export function createConsoleRecorder(source) {
  const events = [];
  const rejectionExamples = [];
  const wasmPanicExamples = [];
  let total = 0, rejectedDraws = 0, wasmPanics = 0, errors = 0, warnings = 0;
  return {
    events,
    record(event) {
      const entry = compactEvent(event);
      total++;
      if (["warning", "warn"].includes(entry.type)) warnings++;
      if (["error", "exception", "assert"].includes(entry.type)) errors++;
      if (isRejectedDraw(event)) {
        rejectedDraws++;
        if (rejectionExamples.length < MAX_REJECTION_EXAMPLES) rejectionExamples.push(entry);
      }
      if (isWasmPanic(event)) {
        wasmPanics++;
        if (wasmPanicExamples.length < MAX_REJECTION_EXAMPLES) wasmPanicExamples.push(entry);
      }
      if (events.length === MAX_EVENTS) events.shift();
      events.push(entry);
    },
    snapshot({ complete = true, failure = null } = {}) {
      return { available: true, source, complete, failure, total, warnings, errors,
        rejectedDraws, wasmPanics, omitted: total - events.length,
        limits: { events: MAX_EVENTS, text: MAX_TEXT, rejectionExamples: MAX_REJECTION_EXAMPLES },
        rejectionExamples: rejectionExamples.map(entry => ({ ...entry })),
        wasmPanicExamples: wasmPanicExamples.map(entry => ({ ...entry })),
        events: events.map(entry => ({ ...entry })) };
    },
  };
}

export function browserConsoleEvidence(client) {
  if (typeof client?.consoleEvidence === "function") return client.consoleEvidence();
  if (!Array.isArray(client?.events)) {
    return { available: false, source: null, complete: false,
      failure: "browser console collector unavailable", total: null, rejectedDraws: null, events: [] };
  }
  const recorder = createConsoleRecorder("CDP");
  for (const event of client.events) recorder.record(event);
  return recorder.snapshot();
}

export function assertRendererConsoleClean(evidence) {
  assert.equal(evidence?.available, true, "browser console evidence is missing");
  assert.equal(evidence.complete, true, `browser console capture incomplete: ${evidence.failure}`);
  assert.equal(evidence.rejectedDraws, 0,
    `renderer rejected a draw: ${JSON.stringify(evidence.rejectionExamples)}`);
  const retainedPanics = (evidence.events || []).filter(isWasmPanic);
  // Older captures did not count traps. Inspect their retained rows without
  // rewriting the original evidence. New counters survive bounded eviction.
  const panics = evidence.wasmPanics ?? retainedPanics.length;
  assert.ok(Number.isSafeInteger(panics) && panics >= retainedPanics.length,
    "invalid WASM panic counter");
  assert.equal(panics, 0,
    `WASM panic in browser console: ${JSON.stringify(evidence.wasmPanicExamples || retainedPanics)}`);
  return evidence;
}

export function validateBrowserConsoleArtifacts(document, resultPath) {
  if (document.ok !== true) return null;
  const audits = document.successArtifacts?.browserConsoleAudits ?? document.browserConsoleAudits;
  if (audits === undefined) return { available: false, reason: "legacy capture has no console audit" };
  assert.ok(Array.isArray(audits));
  let unavailable = 0;
  const labels = new Set();
  for (const audit of audits) {
    assert.match(audit.label, /^[a-z0-9_-]+$/);
    assert.ok(!labels.has(audit.label), "duplicate browser console label");
    labels.add(audit.label);
    const filename = path.join(path.dirname(path.resolve(resultPath)), `${audit.label}-console.json`);
    const stat = fs.lstatSync(filename);
    assert.ok(stat.isFile() && !stat.isSymbolicLink(), "browser console must be a regular file");
    const retained = JSON.parse(fs.readFileSync(filename, "utf8"));
    const { events, ...retainedSummary } = retained;
    const { label: _label, ...summary } = audit;
    assert.deepEqual(summary, retainedSummary, "browser console summary disagrees with retained events");
    assert.ok(Array.isArray(events) && events.length <= MAX_EVENTS);
    if (!retained.available) { unavailable++; continue; }
    assert.ok(Number.isSafeInteger(retained.total) && retained.total >= events.length);
    assert.equal(retained.omitted, retained.total - events.length);
    assert.ok(Number.isSafeInteger(retained.rejectedDraws) && retained.rejectedDraws >= 0 &&
      retained.rejectedDraws <= retained.total);
    assertRendererConsoleClean(retained);
  }
  return { available: audits.length > 0 && unavailable === 0,
    captures: audits.length, unavailable };
}

// W3C WebDriver BiDi: opt into webSocketUrl at new-session time, subscribe to
// log.entryAdded before navigation, then passively observe the owned browser.
// https://www.w3.org/TR/webdriver-bidi/#event-log-entryAdded
export async function openFirefoxConsole(url, {
  WebSocketImpl = globalThis.WebSocket, timeoutMs = 10000,
} = {}) {
  assert.match(String(url), /^ws:\/\/(?:localhost|127\.0\.0\.1|\[::1\])(?::\d+)?\//,
    "Firefox console requires its local WebDriver WebSocket URL");
  assert.ok(Number.isSafeInteger(timeoutMs) && timeoutMs > 0);
  const recorder = createConsoleRecorder("WebDriver BiDi");
  const socket = new WebSocketImpl(url);
  let subscribed = false, intentionalClose = false, failure = null;
  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("Firefox console subscription timed out")), timeoutMs);
      const fail = message => {
        failure = message;
        clearTimeout(timer);
        if (!subscribed) reject(new Error(message));
      };
      socket.onopen = () => {
        try { socket.send(JSON.stringify({ id: 1,
          method: "session.subscribe", params: { events: ["log.entryAdded"] } })); }
        catch (error) { fail(`Firefox console subscription send failed: ${error.message}`); }
      };
      socket.onmessage = event => {
        let message;
        try { message = JSON.parse(event.data); }
        catch { fail("Firefox console received malformed JSON"); return; }
        if (message.id === 1) {
          if (message.type === "error" || message.error) {
            fail(`Firefox console subscription rejected: ${message.message || message.error}`);
          } else if (message.type === "success" && typeof message.result?.subscription === "string") {
            subscribed = true; clearTimeout(timer); resolve();
          } else fail("Firefox console subscription acknowledgement invalid");
        } else if (message.method === "log.entryAdded") {
          const entry = message.params || {};
          if (["warn", "error"].includes(entry.level) || isRejectedDraw(entry)) {
            recorder.record({ type: entry.level === "warn" ? "warning" : entry.level,
              text: entry.text ?? (entry.args || []).map(arg => arg.value ?? "").join(" ") });
          }
        }
      };
      socket.onerror = () => fail("Firefox console WebSocket failed");
      socket.onclose = () => { if (!intentionalClose) fail("Firefox console WebSocket closed unexpectedly"); };
    });
  } catch (error) {
    intentionalClose = true;
    socket.close();
    throw error;
  }
  return {
    events: recorder.events,
    snapshot: () => recorder.snapshot({ complete: subscribed && failure === null, failure }),
    close() { intentionalClose = true; socket.close(); },
  };
}
