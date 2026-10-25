import assert from "node:assert/strict";
import test from "node:test";
import { acquireChromeInputFocusLease, focusCanvas } from "./combat_replay.mjs";

test("joint navigation retains real canvas focus without activating competing windows", async () => {
  const calls = [];
  const send = async (method, params) => { calls.push({ method, params }); return { result: { value: true } }; };
  await focusCanvas(send);
  assert.deepEqual(calls.map(row => row.method), ["Page.bringToFront", "Runtime.evaluate"]);
  calls.length = 0;
  const release = acquireChromeInputFocusLease(send);
  try {
    await focusCanvas(send);
    assert.deepEqual(calls.map(row => row.method), ["Runtime.evaluate"]);
    assert.match(calls[0].params.expression, /canvas.*focus\(\)/);
  } finally { release(); }
  calls.length = 0;
  await focusCanvas(send);
  assert.deepEqual(calls.map(row => row.method), ["Page.bringToFront", "Runtime.evaluate"]);
});

test("focus leases are per-client, nest and restore normal behavior exactly once", async () => {
  const calls = [];
  const a = async method => { calls.push(`a:${method}`); return { result: { value: true } }; };
  const b = async method => { calls.push(`b:${method}`); return { result: { value: true } }; };
  const outer = acquireChromeInputFocusLease(a), inner = acquireChromeInputFocusLease(a);
  inner(); inner();
  await focusCanvas(a); await focusCanvas(b);
  assert.deepEqual(calls, ["a:Runtime.evaluate", "b:Page.bringToFront", "b:Runtime.evaluate"]);
  outer(); outer(); calls.length = 0;
  await focusCanvas(a);
  assert.deepEqual(calls, ["a:Page.bringToFront", "a:Runtime.evaluate"]);
});
