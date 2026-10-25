import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

// Native ordering/eligibility checks over the captured EMP follow-up poses.
// These virtual actors do not establish browser firing or damage proof.
const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
const start = source.indexOf("  let damageLossProbe = null;"),
  end = source.indexOf("\n  let capturedDamageLoss", start);
assert(start >= 0 && end > start);

async function replay({ braked = true, expiresBeforeShot = false, lateHit = false } = {}) {
  const calls = [];
  const victim = { playerX: 8200, playerY: 8350.02, playerVelocityX: 0, playerVelocityY: 3.17,
    serverPlayerEnergy: 964, playerEnergy: 964, playerMaxEnergy: 1700, latestSnapshotServerTick: 5904,
    localGameplayState: { engineShutdownTimer: 0 } };
  const shooter = { playerX: 8326.88, playerY: 8303.57, playerVelocityX: 35.99, playerVelocityY: -12.62 };
  const actor = { metricsState: async () => ({ uiMetrics: structuredClone(victim) }) },
    peer = { metricsState: async () => ({ uiMetrics: structuredClone(shooter) }) };
  let fired = false;
  const context = vm.createContext({ actor, peer, server: {}, playerId: 0, peerPlayerId: 1, logOffset: 0,
    empAim: { oriented: true }, empDamage: { event: { empUntilTick: 6172 }, nextSinceCount: 3 },
    parseDamagePrizeLosses: () => fired ? [{ damageSeq: 4 }] : [],
    driveClientToWorldTarget: () => { throw new Error("unnecessary standoff travel consumed the timed EMP"); },
    brakeClientVelocity: async (client, budget) => {
      assert.equal(client, peer); assert.equal(budget, 4_000); calls.push("brake");
      shooter.playerVelocityX = shooter.playerVelocityY = 0;
      victim.latestSnapshotServerTick += 20;
      return { braked, speed: 0 };
    },
    orientClientForProtocolDirection: async (client, dx, dy, budget) => {
      assert.equal(client, peer); assert.equal(budget, 4_000);
      assert.equal(dx, victim.playerX - shooter.playerX); assert.equal(dy, victim.playerY - shooter.playerY);
      calls.push("aim"); victim.latestSnapshotServerTick = expiresBeforeShot ? 6172 : 5939;
      return { oriented: true };
    },
    fireUntilDamage: async options => {
      assert.equal(options.shooter, peer); assert.equal(options.victim, actor);
      assert.equal(options.requireLethal, false); assert.equal(options.timeoutMs, 3_000);
      assert.equal(options.sinceCount, 3); calls.push("fire"); fired = true;
      return { event: { serverTick: lateHit ? 6172 : 5964 }, nextSinceCount: 4 };
    } });
  const result = await vm.runInContext(`(async()=>{${source.slice(start, end)}
    return { damageLossProbe, damageLossAim, damageLossShooterRoute };})()`, context);
  return { calls, result };
}

test("captured recoil is braked before a live aimed shot without an unrelated standoff trip", async () => {
  const { calls, result } = await replay();
  assert.deepEqual(calls, ["brake", "aim", "fire"]);
  assert.equal(result.damageLossShooterRoute.required, false);
  assert.equal(result.damageLossAim.shotBoundaryRemaining, 233);
  assert.equal(result.damageLossProbe.event.serverTick, 5964);
});
test("failed genuine braking cannot become a passing prize-loss probe", async () => {
  await assert.rejects(replay({ braked: false }), /did not brake authentic EMP recoil/);
});
test("expiry before the follow-up shot cannot be hidden by below-cap energy", async () => {
  await assert.rejects(replay({ expiresBeforeShot: true }), /aim outlasted the authoritative EMP deadline/);
});
test("an authoritative hit after expiry fails even if the client still observed an eligible tick", async () => {
  await assert.rejects(replay({ lateHit: true }), /hit outlasted the authoritative EMP deadline/);
});
