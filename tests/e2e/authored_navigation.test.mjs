import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

// Test the actual coordinator against a virtual driver, not browser delivery or
// physics. Geometric obstacle/endpoint proof lives in player_rendezvous.test.
async function replay({ lifeChanges = false, earlyClaim = false, slow = false, cruise = false } = {}) {
  let now = 0, claimed = false;
  const m = { playerX: 100, playerY: 200, playerVelocityX: 0, playerVelocityY: 0,
    selfPlayerId: 1, playerLifeSeq: 7, live: true,
    localGameplayState: { authorityScopeServerSessionId: "9", authorityScopeArenaStreamId: "2", brickTiles: [] } };
  const client = { metricsState: async () => ({ uiMetrics: structuredClone(m) }), evaluate: async () => "chaos-zone" };
  const legs = [];
  const source = fs.readFileSync(new URL("./authored_navigation.mjs", import.meta.url), "utf8");
  const ctx = vm.createContext({ Date: { now: () => now }, Math, Number, JSON, Error, process: { env: {} },
    path: { join: () => "/map" }, repoRoot: "/repo", exactLiveGameplayScope: m => m.live,
    waitUntil: async (_label, predicate) => { assert(await predicate()); },
    loadRendezvousMap: () => ({ geometry: {}, provenance: { mapSha256: "fixture" } }),
    planAuthoredApproach: () => ({ routes: [{ drivePoints: [{ x: 200, y: 200 }, { x: 300, y: 200 }] }] }),
    driveClientToWorldTarget: async (_client, point, timeout, settle, tolerance, complete, maximumDesiredSpeed, desiredSpeedScale) => {
      assert.equal(settle, true); assert.equal(tolerance, 3); assert(timeout <= 10_000);
      assert.equal(maximumDesiredSpeed, cruise ? 100 : 42);
      assert.equal(desiredSpeedScale, cruise ? 0.5 : 0.25);
      legs.push(point); now += slow ? 11_000 : 250;
      if (lifeChanges) m.playerLifeSeq++;
      if (earlyClaim) claimed = true;
      const completed = await complete();
      if (!completed) { m.playerX = point.x; m.playerY = point.y; }
      return { reached: true, completed };
    },
  });
  vm.runInContext(source.slice(source.indexOf("export async function")).replace("export ", ""), ctx);
  let result, failure;
  try {
    result = await ctx.navigateAuthoredTarget(client, { x: 300, y: 200 }, null,
      { timeoutMs: 10_000, complete: earlyClaim ? () => claimed : null,
        ...(cruise ? { maximumDesiredSpeed: 100, desiredSpeedScale: 0.5 } : {}) });
  } catch (error) { failure = error; }
  return { result, failure, client, legs };
}

test("authored navigation drives every planned bend and retains exact-scope evidence", async () => {
  const { result, failure, client, legs } = await replay();
  assert.ifError(failure); assert.equal(result.status, "complete"); assert.equal(legs.length, 2);
  assert.equal(result.reached, true); assert.equal(result.completed, false);
  assert.equal(client.rendezvousTrace[0], result);
});

test("a long setup route can use rendezvous cruise without changing its leg deadlines", async () => {
  const { result, failure, legs } = await replay({ cruise: true });
  assert.ifError(failure); assert.equal(result.status, "complete"); assert.equal(legs.length, 2);
});

test("an authoritative prize claim ends travel without inventing endpoint arrival", async () => {
  const { result, failure, legs } = await replay({ earlyClaim: true });
  assert.ifError(failure); assert.equal(legs.length, 1);
  assert.equal(result.completed, true); assert.equal(result.reached, false);
});

test("authored navigation cannot continue across a life change", async () => {
  const { failure, result, client } = await replay({ lifeChanges: true });
  assert(failure); assert.match(failure.message, /lifecycle boundary/);
  assert.equal(result, undefined); assert.equal(client.rendezvousTrace[0].status, "failed");
});

test("a slow terminal driver cannot certify success after the setup deadline", async () => {
  const { failure, result } = await replay({ earlyClaim: true, slow: true });
  assert(failure); assert.match(failure.message, /setup budget/); assert.equal(result, undefined);
});
