import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

// Replay only the actual pre-seam coordinator. These geometry/ordering units
// do not simulate physics, deliver browser input, or establish door recovery.
const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
const start = source.indexOf("function overlapsDoorWithReferenceGameHull("),
  end = source.indexOf("  let priorSeed =", start);
assert(start >= 0 && end > start);
const setupSource = source.slice(start, end) + "return { doorApproach, stagedDistance };\n}";

async function stage(fromX, missPrecise = false) {
  const calls = [];
  let metrics = { playerX: fromX, playerY: 8360, playerVelocityX: 0, playerVelocityY: 0 };
  const actor = { metricsState: async () => ({ uiMetrics: metrics }) };
  const drive = async (_actor, target, _budget, _settle, tolerance) => {
      calls.push("drive");
      if (calls.length === 1) {
        // A valid coarse arrival can occur anywhere in its acceptance disk.
        // The former7590±20 disk includes7570: a live door hull overlap.
        for (let angle = 0; angle < 2 * Math.PI; angle += Math.PI / 16) {
          const point = { positionX: target.x + Math.cos(angle) * tolerance,
            positionY: target.y + Math.sin(angle) * tolerance };
          assert.equal(ctx.overlapsDoorWithReferenceGameHull(point, 472, 522), false,
            "the entire coarse arrival disk must stay outside the live door hull");
        }
        assert.equal(Math.sign(target.x - 7560), Math.sign(fromX - 7560),
          "coarse setup must approach from the pilot's current side");
      } else {
        assert.equal(calls.at(-2), "brake", "brake before entering the narrow staging pocket");
      }
      metrics = { ...metrics, playerX: target.x + (missPrecise && calls.length > 1 ? 40 : 0), playerY: target.y };
      return { reached: true };
    };
  const ctx = vm.createContext({
    prepareStaleDoorRouteOrigin: async () => ({}),
    driveClientToWorldTarget: drive,
    navigateAuthoredTarget: async (actor, target, _server, options) => {
      assert.equal(options.timeoutMs, 90_000);
      assert.equal(options.maximumDesiredSpeed, 100);
      assert.equal(options.desiredSpeedScale, 0.5);
      return drive(actor, target, options.timeoutMs, true, options.arrivalRadiusPx);
    },
    brakeClientVelocity: async () => { calls.push("brake"); return { braked: true }; },
    orientClientForVelocity: async () => { calls.push("orient"); return { oriented: true }; },
  });
  vm.runInContext(setupSource, ctx);
  const result = await ctx.runStaleDoorReadinessGate({ actor, server: { logs: [] }, playerId: 1 });
  return { result, calls };
}

test("both door approaches stop safely before precise hull staging", async () => {
  for (const x of [7500, 8200]) {
    const { result, calls } = await stage(x);
    assert.equal(result.stagedDistance, 0);
    assert.deepEqual(calls, ["drive", "brake", "drive", "brake", "orient"]);
  }
});

test("a failed precise arrival still fails the original eight-pixel precondition", async () => {
  await assert.rejects(stage(8200, true), /could not stage safely/);
});
