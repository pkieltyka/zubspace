import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { referenceDirectionsForSeed } from "./shrapnel_reference.mjs";
test("browser staging oracle matches every retained random reference-game fragment direction", () => {
  const rows = fs.readFileSync(new URL("../fixtures/reference_game_shrapnel_directions.csv", import.meta.url), "utf8")
    .split("\n").filter(line => line && !line.startsWith("#")).map(line => line.split(",").map(Number));
  let checked = 0;
  for (const [random, seed, count, index, x, y] of rows) {
    if (!random) continue;
    const actual = referenceDirectionsForSeed(seed, count)[index];
    assert.ok(Math.abs(actual.x-x) < 0.000002 && Math.abs(actual.y-y) < 0.000002,
      `reference vector mismatch seed=${seed} count=${count} index=${index}`);
    checked++;
  }
  assert.ok(checked >= 180);
});
