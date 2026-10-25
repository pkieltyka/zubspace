import test from "node:test";
import assert from "node:assert/strict";
import { dispatchChromeKey, keyEvent } from "./combat_replay.mjs";

function capture() {
  const calls = [];
  const send = async (method, params) => { assert.equal(method, "Input.dispatchKeyEvent"); calls.push(params); };
  return { calls, send, key: (type, name) => dispatchChromeKey(send, type, keyEvent(name)) };
}

test("real CDP chord events carry modifiers through non-modifier release", async () => {
  const c = capture();
  for (const [type, key] of [["keyDown", "Control"], ["keyUp", "Control"],
    ["keyDown", "Shift"], ["keyDown", "Delete"], ["keyUp", "Delete"], ["keyUp", "Shift"]]) await c.key(type, key);
  assert.deepEqual(c.calls.map(event => event.modifiers), [2, 0, 8, 8, 8, 0]);
  assert.deepEqual(c.calls.map(event => event.code), ["ControlLeft", "ControlLeft", "ShiftLeft", "Delete", "Delete", "ShiftLeft"]);
});

test("parallel chord RPCs retain declared physical modifier transitions", async () => {
  const c = capture();
  await Promise.all([c.key("keyDown", "Shift"), c.key("keyDown", "Control"), c.key("keyDown", "x")]);
  await Promise.all([c.key("keyUp", "x"), c.key("keyUp", "Control"), c.key("keyUp", "Shift")]);
  assert.deepEqual(c.calls.map(event => event.modifiers), [8, 10, 10, 10, 8, 0]);
});

test("modifier ownership is client-local and retains the other physical side", async () => {
  const a = capture(), b = capture();
  await a.key("keyDown", "Shift");
  await b.key("keyDown", "x");
  assert.equal(b.calls[0].modifiers, 0);
  const right = { key: "Shift", code: "ShiftRight", keyCode: 16 };
  await dispatchChromeKey(a.send, "keyDown", right);
  await a.key("keyUp", "Shift");
  await a.key("keyDown", "x");
  assert.equal(a.calls.at(-1).modifiers, 8);
  await dispatchChromeKey(a.send, "keyUp", right);
  await a.key("keyUp", "x");
  assert.equal(a.calls.at(-1).modifiers, 0);
});
