import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

// Run the actual callers with virtual keyboard navigation. This verifies only
// setup retry classification; real movement/combat remains a browser contract.
for (const [file, name] of [
  ["bot_authority.mjs", "returnExtremeChromePilotToSafety"],
  ["bot_authority_firefox.mjs", "returnExtremeFirefoxPilotToSafety"],
]) {
  async function replay({ error = null, retire = null, alwaysRetire = false } = {}) {
    const source = fs.readFileSync(new URL(file, import.meta.url), "utf8");
    const start = source.indexOf(`async function ${name}(`);
    assert(start >= 0);
    const fn = source.slice(start, source.indexOf("\n}\n", start) + 3);
    const state = { playerLifeSeq: 2, playerAlive: true, serverPlayerAlive: true,
      inSafeZone: false, playerVelocityX: 20, playerVelocityY: 0 };
    let calls = 0, fineCalls = 0, waits = 0;
    const actor = { metricsState: async () => ({ uiMetrics: structuredClone(state) }) };
    const context = vm.createContext({
      Math, Number, Error, extremeRecoveryMaximumSpeed: 8,
      positionIsOnAuthoredSafeTile: () => true,
      waitUntil: async (_label, predicate, timeout) => {
        waits++; assert.equal(timeout, 15_000);
        // A genuine successor baseline arrives before a new route is allowed.
        state.playerAlive = state.serverPlayerAlive = true;
        assert(await predicate());
      },
      navigateAuthoredTarget: async (_actor, _center, _server, options) => {
        calls++; assert.equal(options.timeoutMs, 90_000);
        assert.equal(options.maximumDesiredSpeed, 100);
        assert.equal(options.desiredSpeedScale, name.includes('Firefox') ? 0.25 : 0.5);
        if (calls === 1 || alwaysRetire) {
          if (retire === "life") state.playerLifeSeq++;
          if (retire === "death") state.playerAlive = state.serverPlayerAlive = false;
          if (error) throw new Error(error);
        }
        return { reached: true };
      },
      driveClientToWorldTarget: async (_actor, _center, timeout, brake, radius) => {
        fineCalls++; assert.equal(timeout, 20_000); assert.equal(brake, true);
        assert.equal(radius, 12); state.inSafeZone = true;
        return { reached: true };
      },
    });
    vm.runInContext(fn + `\nthis.run = ${name};`, context);
    let result, failure;
    try { result = await context.run(actor); } catch (e) { failure = e; }
    return { result, failure, calls, fineCalls, waits };
  }

  for (const retire of ["life", "death"]) {
    test(`${name} reacquires a genuinely ${retire}-retired route`, async () => {
      const r = await replay({ error: "authored navigation crossed a lifecycle boundary", retire });
      assert.ifError(r.failure); assert.equal(r.result, true);
      assert.equal(r.calls, 2); assert.equal(r.fineCalls, 1); assert.equal(r.waits, 2);
    });
  }
  test(`${name} does not swallow a lifecycle error on an unchanged live scope`, async () => {
    const r = await replay({ error: "authored navigation crossed a lifecycle boundary" });
    assert.match(r.failure.message, /lifecycle boundary/); assert.equal(r.calls, 1);
    assert.equal(r.fineCalls, 0);
  });
  test(`${name} preserves unrelated navigation failures even after a death`, async () => {
    const r = await replay({ error: "genuine authored navigation did not reach its planned leg", retire: "death" });
    assert.match(r.failure.message, /planned leg/); assert.equal(r.calls, 1);
  });
  test(`${name} cannot expand its original three attempts`, async () => {
    const r = await replay({ error: "authored navigation crossed a lifecycle boundary", retire: "life", alwaysRetire: true });
    assert.ifError(r.failure); assert.equal(r.result, false); assert.equal(r.calls, 3);
    assert.equal(r.fineCalls, 0);
  });
}
