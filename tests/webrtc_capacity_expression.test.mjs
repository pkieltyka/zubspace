import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { loadExpression } from "../tools/webrtc_capacity_load.mjs";
import { MAX_PLAYER_CLIENTS } from "../tools/webrtc_capacity_config.mjs";

test("capacity browser setup is importable without starting services", () => {
  const expression = loadExpression("http://127.0.0.1:9001/v1/rtc/offer", {
    count: 192, batchSize: 24, holdMs: 1000,
  });
  assert.match(expression, /const count = 192;/);
  assert.match(expression, /const batchSize = 24;/);
  assert.match(expression, /const holdMs = 1000;/);
  assert.match(expression, /snapshots: \[\.\.\.snapshotCounts\]/);
  assert.doesNotThrow(() => new vm.Script(expression));
});

test("capacity setup rejects unbounded populations, batches, waits and URLs", () => {
  for (const options of [{count: 0}, {count: MAX_PLAYER_CLIENTS + 1}, {count: 1.5},
    {batchSize: 0}, {batchSize: MAX_PLAYER_CLIENTS + 1}, {batchSize: 2.5},
    {holdMs: 0}, {holdMs: 600001}, {holdMs: NaN}]) {
    assert.throws(() => loadExpression("http://127.0.0.1:9001/v1/rtc/offer", options));
  }
  assert.throws(() => loadExpression("file:///tmp/offer"));
});
