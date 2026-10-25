import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
const source = html.slice(html.indexOf("        const trackedGameKeys ="), html.indexOf("        function queryParam("));
assert.ok(source.includes("function releaseTrackedGameKeys("));

function bridge({ held = "ArrowUp" } = {}) {
  const windowListeners = new Map(), documentListeners = new Map(), microtasks = [], releases = [], wasmCalls = [];
  let wasmBorrowed = false;
  const canvas = { focus() {}, onkeydown(event) { releases.push(`down:${event.code}`); }, onkeyup(event) {
    assert.equal(wasmBorrowed, false, "key_up must not re-enter an active WASM frame borrow");
    releases.push(event.code);
  } };
  const context = {
    canvas,
    window: { addEventListener(type, callback) { windowListeners.set(type, callback); } },
    document: { hidden: true, addEventListener(type, callback) { documentListeners.set(type, callback); } },
    gameKeyCodes: new Set(["ArrowUp", "Slash"]), browserHandledGameKeyCodes: new Set(["ArrowUp"]),
    wasm_exports: {
      key_down: (...args) => wasmCalls.push(["key_down", ...args]),
      key_up: (...args) => wasmCalls.push(["key_up", ...args]),
    },
    HTMLInputElement: class {}, HTMLTextAreaElement: class {}, HTMLSelectElement: class {}, HTMLButtonElement: class {},
    KeyboardEvent: class { constructor(type, properties) { Object.assign(this, { type }, properties); } },
    queueMicrotask(callback) { microtasks.push(callback); },
  };
  vm.runInNewContext(source, context);
  windowListeners.get("keydown")({ code: held, target: canvas, preventDefault() {} });
  return {
    releases, microtasks, wasmCalls, canvas, listeners: windowListeners,
    snapshot: () => context.window.__zubspaceGameKeyStateForTest(),
    fire(type) { (windowListeners.get(type) || documentListeners.get(type))(); },
    borrowed(value) { wasmBorrowed = value; },
    flush() { while (microtasks.length) microtasks.shift()(); },
  };
}

for (const [event, reason] of [
  ["zubspace-controls-open", "controls-guide"], ["blur", "window-blur"],
  ["visibilitychange", "document-hidden"], ["pagehide", "pagehide"], ["freeze", "document-freeze"],
]) {
  test(`${event} releases held keys after the active frame returns`, () => {
    const input = bridge();
    input.borrowed(true);
    assert.doesNotThrow(() => input.fire(event));
    assert.equal(input.releases.length, 0);
    assert.equal(input.microtasks.length, 1);
    input.borrowed(false);
    input.flush();
    assert.deepEqual(input.releases, ["ArrowUp"]);
    assert.equal(input.snapshot().trackedCodes.length, 0);
    assert.equal(input.snapshot().forcedReleaseBatches, 1);
    assert.equal(input.snapshot().lastForcedReleaseReason, reason);
  });
}

test("overlapping lifecycle notifications do not replay a forced key release", () => {
  const input = bridge();
  input.borrowed(true);
  input.fire("blur"); input.fire("visibilitychange");
  input.borrowed(false); input.flush();
  assert.deepEqual(input.releases, ["ArrowUp"]);
  assert.equal(input.snapshot().forcedReleaseBatches, 1);
});

test("Slash reaches the game as miniquad's Slash code, and its keypress still types", () => {
  const input = bridge({ held: "Slash" });
  let prevented = false;
  // Off the canvas, the bridge forwards other keys to the bundle's handlers.
  const event = (code, properties = {}) => ({ code, target: {}, preventDefault() { prevented = true; }, ...properties });
  input.listeners.get("keydown")(event("Slash", { shiftKey: true, repeat: true }));
  input.listeners.get("keyup")(event("Slash"));
  assert.deepEqual(input.wasmCalls, [["key_down", 47, 0, false], ["key_down", 47, 1, true], ["key_up", 47, 0]]);
  assert.deepEqual(input.releases, [], "the bundle's own handlers never see Slash");
  assert.equal(prevented, false, "keydown stays uncancelled so chat receives the keypress");
  input.listeners.get("keypress")(event("Slash"));
  assert.equal(prevented, true, "the keypress default (Firefox Quick Find) is cancelled");
});

test("a Slash held across focus loss is released as miniquad's Slash code", () => {
  const input = bridge({ held: "Slash" });
  input.fire("blur");
  input.flush();
  assert.deepEqual(input.wasmCalls, [["key_down", 47, 0, false], ["key_up", 47, 0]]);
  assert.deepEqual(input.releases, []);
});
