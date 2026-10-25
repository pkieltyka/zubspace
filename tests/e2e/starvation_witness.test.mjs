// Contract tests for the readiness-timeout diagnostic. A gate that cannot talk to a page is almost always
// looking at a blocked main thread, and the message has to say so - or say nothing at all when nothing
// starved, because a permanent note on healthy runs teaches authors to ignore it.

import assert from "node:assert/strict";
import test from "node:test";

import {
  formatStarvationNote,
  resetStarvationLedger,
  starvationLedgerSnapshot,
} from "./combat_replay.mjs";

const state = (overrides) => ({
  timedOut: 1,
  lastMethod: "Runtime.evaluate",
  starvedSince: 1_000,
  lastSuccessAt: 1_000,
  longTasks: null,
  ...overrides,
});

test("a clean run adds no starvation note", () => {
  resetStarvationLedger();
  assert.equal(formatStarvationNote(2_000), "");
  assert.deepEqual(starvationLedgerSnapshot(), {});
  // A page that answered every command must not be reported even though it is tracked.
  assert.equal(formatStarvationNote(2_000, [["carrier", state({ timedOut: 0 })]]), "");
});

test("the note names the page, the command, and the measured stall", () => {
  const note = formatStarvationNote(9_000, [
    ["carrier", state({ timedOut: 3, starvedSince: 4_000 })],
    ["observer", state({ timedOut: 0 })],
  ]);
  assert.match(note, /carrier: 3 timed out DevTools command\(s\) \(last Runtime\.evaluate\)/);
  // starvedSince holds the last answered command, so the stall is 9_000 - 4_000.
  assert.match(note, /unresponsive 5000ms/);
  assert.ok(!note.includes("observer"), "a page with no timeout must not appear");
});

test("an organic long task is quoted alongside the stall", () => {
  const note = formatStarvationNote(9_000, [
    [
      "carrier",
      state({
        timedOut: 2,
        starvedSince: 8_000,
        longTasks: [
          { duration: 1916, start: 8705 },
          { duration: 728, start: 4905 },
        ],
      }),
    ],
  ]);
  assert.match(note, /unresponsive 1000ms, worst organic long task 1916ms at t=8705ms/);
});

test("the stall is still reported with no witness at all", () => {
  // Chrome does not attribute a script the driver ran synchronously, so an empty or unreadable witness is
  // expected for some real stalls and must never swallow the measurement.
  const empty = formatStarvationNote(9_000, [["carrier", state({ starvedSince: 8_000, longTasks: [] })]]);
  const unreadable = formatStarvationNote(9_000, [["carrier", state({ starvedSince: 8_000 })]]);
  for (const note of [empty, unreadable]) {
    assert.match(note, /1 timed out DevTools command/);
    assert.match(note, /unresponsive 1000ms/);
    assert.ok(!note.includes("long task"), "an absent witness must not be guessed");
  }
});

test("several starved pages are all listed", () => {
  const note = formatStarvationNote(9_000, [
    ["carrier", state({ starvedSince: 8_000 })],
    ["observer", state({ timedOut: 5, starvedSince: 6_000 })],
  ]);
  assert.match(note, /carrier: /);
  assert.match(note, /observer: 5 timed out/);
  assert.match(note, /page main-thread starvation ->/);
});
