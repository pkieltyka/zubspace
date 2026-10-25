import assert from "node:assert/strict";

// Actual visibilitychange events from a genuine tab switch. This validates
// the observation; it never changes document visibility or browser scheduling.
export function tabReturnVisibilityEvidence(events, requiredHiddenMs) {
  assert.ok(Array.isArray(events) && events.length >= 3, "missing actual tab visibility transitions");
  assert.ok(Number.isFinite(requiredHiddenMs) && requiredHiddenMs > 0);
  for (let index = 0; index < events.length; index++) {
    const event = events[index];
    assert.ok(Number.isFinite(event.at) && typeof event.hidden === "boolean");
    if (index) assert.ok(event.at >= events[index - 1].at, "visibility clock regressed");
  }
  assert.equal(events[0].hidden, false, "test must start in the visible game tab");
  const hiddenIndex = events.findIndex(event => event.hidden);
  assert.ok(hiddenIndex > 0, "game tab never actually became hidden");
  const visible = events.slice(hiddenIndex + 1).find(event => !event.hidden);
  assert.ok(visible, "game tab did not become visible again");
  const hiddenMs = visible.at - events[hiddenIndex].at;
  assert.ok(hiddenMs >= requiredHiddenMs, `hidden interval ${hiddenMs}ms is shorter than ${requiredHiddenMs}ms`);
  assert.equal(events.at(-1).hidden, false);
  return { hiddenMs, hiddenAt: events[hiddenIndex].at, visibleAt: visible.at };
}
