import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

// The genuine-key driver only needs the current pose. A full diagnostic
// export here can stall the runner/page and exhaust an unrelated input tail.
const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
const start = source.indexOf("async driveForTrace() {"), end = source.indexOf("async driveRemoteSmoothness(", start);
assert(start > 0 && end > start);
const method = vm.runInNewContext(`({${source.slice(start, end)}})`, {
  sleep: async () => {}, chrome: { label: "test-pilot" },
}).driveForTrace;

async function run(poses) {
  let reads = 0, holds = 0;
  const client = { metricsState: async () => {
    assert(reads < poses.length, "the unchanged bounded attempts must not keep polling");
    return { uiMetrics: poses[reads++] };
  }, pageState: async () => { throw Error("full diagnostic export in a current-pose motion probe"); },
  hold: async (key, ms) => { assert.equal(key, "ArrowUp"); assert.equal(ms, 220); holds++; } };
  const result = await method.call(client);
  return { result, reads, holds };
}
const pose = (x, vx = 0) => ({ playerX: x, playerY: 0, playerVelocityX: vx, playerVelocityY: 0 });

test("the real motion probe reads compact poses while keeping its genuine key and displacement proof", async () => {
  const { result, reads, holds } = await run([pose(10), pose(13)]);
  assert.equal(result.moved, 3); assert.equal(result.attempts, 1);
  assert.equal(reads, 2); assert.equal(holds, 1);
});

test("a latched first edge gets the original one retry and must then move", async () => {
  const { result, reads, holds } = await run([pose(10), pose(10), pose(10), pose(11)]);
  assert.equal(result.moved, 1); assert.equal(result.attempts, 2);
  assert.equal(reads, 4); assert.equal(holds, 2);
});

test("two stationary attempts still fail", async () => {
  await assert.rejects(run([pose(10), pose(10), pose(10), pose(10)]), /keyboard movement did not affect local ship/);
});

test("the original nonzero velocity branch is retained", async () => {
  const { result } = await run([pose(10), pose(10, 1)]);
  assert.equal(result.attempts, 1); assert.equal(result.moved, 0);
});
