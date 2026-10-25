import assert from "node:assert/strict";
import test from "node:test";
import { tabReturnVisibilityEvidence } from "./tab_return_visibility.mjs";

test("tab-return QA requires real hidden and visible transitions over its full interval", () => {
  const events = [{ at: 100, hidden: false }, { at: 200, hidden: true }, { at: 125200, hidden: false }];
  assert.equal(tabReturnVisibilityEvidence(events, 125000).hiddenMs, 125000);
  for (const wrong of [[], events.slice(0, 2),
    events.map(event => ({ ...event, hidden: false })),
    [{ at: 100, hidden: true }, ...events.slice(1)],
    [...events.slice(0, 2), { at: 125199, hidden: false }],
    [...events, { at: 125199, hidden: true }],
    [...events.slice(0, 2), { at: NaN, hidden: false }]]) {
    assert.throws(() => tabReturnVisibilityEvidence(wrong, 125000));
  }
});
