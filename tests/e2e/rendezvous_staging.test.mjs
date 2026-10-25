import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

// Exercise the real setup coordinator with virtual time. The movement driver
// supplies recorded stopped-but-drifting arrivals; this tests orchestration,
// not ship physics or genuine browser keyboard delivery.
async function replay({ emptyFirstRoute = false, changeLifeAt = null, setupBudgetMs = 180_000, stageRadiusPx = 48, meetingPoint = null, avoidTiles = [], finalAimMs = 50, inheritedMotion = false, brakeFails = false, brakeLifeChange = false, recoverOrigins = false, originNeedsRecovery = false, originRecoveryRejected = false } = {}) {
  let now = 0, done = false, result, failure, activeFocusLeases = 0;
  const timers = [], drives = [[], []], plannerOptions = [], brakeCalls = [], originCalls = [];
  const sleep = ms => new Promise(resolve => timers.push({ at: now + ms, resolve }));
  const metrics = [0, 1].map(index => ({ selfPlayerId: index, playerLifeSeq: 1,
    playerX: emptyFirstRoute && index === 0 ? 500 : index * 1000, playerY: 1000,
    playerVelocityX: 0, playerVelocityY: inheritedMotion && index === 0 ? -15.89 : 0, inSafeZone: false, live: true,
    localGameplayState: { authorityScopeServerSessionId: "123", authorityScopeArenaStreamId: "1" } }));
  const clients = metrics.map((m, index) => ({ index,
    acquireInputFocusLease: () => { activeFocusLeases++; return () => { activeFocusLeases--; }; },
    metricsState: async () => ({ uiMetrics: structuredClone(m) }), evaluate: async () => "chaos-zone" }));
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const start = source.indexOf("let rendezvousTail ="), end = source.indexOf("async function waitForPlayersReady(", start);
  assert(start >= 0 && end > start);
  const ctx = vm.createContext({ Date: { now: () => now }, Promise, AggregateError, Math,
    path: { join: () => "/zone" }, repoRoot: "/repo", process: { env: {} }, sleep,
    waitForLivePlayerScopes: async () => {}, exactLiveGameplayScope: m => m.live,
    loadRendezvousMap: () => ({ geometry: {}, provenance: {} }),
    prepareKeyboardLifecycleOrigin: async options => {
      const index = options.actor.index;
      assert.equal(options.playerId, index); assert.equal(options.initialLifeSeq, 1);
      assert.ok(options.deadline > now && options.deadline <= 180_000);
      originCalls.push({ index, deadline: options.deadline });
      if (originRecoveryRejected) throw new Error("ordinary origin Warp was rejected");
      await sleep(20);
      if (index === 0 && originNeedsRecovery) metrics[0].playerX = 64;
      metrics[index].playerVelocityX = metrics[index].playerVelocityY = 0;
      return { warps: index === 0 && originNeedsRecovery ? [{ outcome: 1, life_seq: 1 }] : [] };
    },
    planPlayerRendezvous: (_geometry, _origins, options) => {
      if (inheritedMotion && metrics.some(m => Math.hypot(m.playerVelocityX, m.playerVelocityY) > 3.5))
        throw new Error("route certification observed inherited motion");
      if (originNeedsRecovery && _origins.some(point => point.x < 50))
        throw new Error("rendezvous origin lacks conservative hull clearance");
      plannerOptions.push(options);
      return { target: { x: 500, y: 1000 }, routes: clients.map((_, i) => ({
        drivePoints: emptyFirstRoute && i === 0 ? [] : [{ x: 500, y: 1000 }],
      })) };
    },
    brakeClientVelocity: async (client, timeout) => {
      assert(timeout > 0 && timeout <= 8000);
      brakeCalls.push({ index: client.index, timeout, at: now });
      await sleep(150);
      if (brakeLifeChange) metrics[0].playerLifeSeq = 2;
      if (!brakeFails) {
        metrics[client.index].playerVelocityX = 0;
        metrics[client.index].playerVelocityY = 0;
      }
      return { braked: !brakeFails, reason: brakeFails ? "still moving" : null,
        speed: Math.hypot(metrics[client.index].playerVelocityX, metrics[client.index].playerVelocityY),
        metrics: structuredClone(metrics[client.index]) };
    },
    brakeVelocityDisposition: m => ({ braked: Math.hypot(m.playerVelocityX, m.playerVelocityY) <= 3.5 }),
    orientClientForProtocolDirection: async () => {
      await sleep(drives.some(legs => legs.length > 0) ? finalAimMs : 50);
      return { oriented: true, direction: 10, desiredDirection: 10 };
    },
    driveClientToWorldTarget: async (client, target, timeout, _settle, _tolerance, completion) => {
      if (await completion?.()) return { reached: true, completed: true };
      const index = client.index, first = drives[index].length === 0;
      drives[index].push({ at: now, target, timeout });
      const duration = first && !(emptyFirstRoute && index === 0) ? (index === 0 ? 24_000 : 5_000) : 250;
      assert(duration <= timeout, "simulated travel must obey the actual leg budget");
      await sleep(duration);
      metrics[index].playerX = target.x; metrics[index].playerY = target.y;
      // The shorter c70e33b route finished at 2.316px/s and drifted ~44px
      // while its peer completed. Both values meet the unchanged rest limit.
      metrics[index].playerVelocityY = index === 0 ? -0.69 : -2.22;
      return { reached: true, distance: 0, speed: Math.abs(metrics[index].playerVelocityY), target };
    },
  });
  vm.runInContext(source.slice(start, end), ctx);
  ctx.rendezvousPlayers(...clients, null, { setupBudgetMs, stageRadiusPx, meetingPoint, avoidTiles, recoverOrigins }).then(value => { result = value; done = true; }, error => { failure = error; done = true; });
  for (let step = 0; !done && step < 20_000; step++) {
    await new Promise(resolve => setImmediate(resolve));
    if (!timers.length) continue;
    const next = Math.min(...timers.map(timer => timer.at)), dt = (next - now) / 1000;
    for (const m of metrics) { m.playerX += m.playerVelocityX * dt; m.playerY += m.playerVelocityY * dt; }
    now = next;
    if (changeLifeAt !== null && now >= changeLifeAt) metrics[0].playerLifeSeq = 2;
    for (let i = timers.length - 1; i >= 0; i--) if (timers[i].at <= now) timers.splice(i, 1)[0].resolve();
  }
  assert(done, "setup must complete or fail within virtual-time bounds");
  assert.equal(activeFocusLeases, 0, "joint navigation restores focus policy on success and failure");
  return { result, failure, drives, now, plannerOptions, brakeCalls, originCalls };
}

test("an early stopped arrival holds station instead of drifting away during the longer route", async () => {
  const { result, failure, drives, now } = await replay();
  assert.ifError(failure);
  assert.equal(result.status, "complete"); assert(result.separationPx <= 24);
  assert(drives[1].length > 1, "the early arrival needs real corrective driver calls");
  assert(now < 180_000, "station keeping shares the original setup budget");
});

test("a pilot already at the stage participates in the same joint arrival barrier", async () => {
  const { result, failure } = await replay({ emptyFirstRoute: true });
  assert.ifError(failure); assert.equal(result.status, "complete"); assert(result.separationPx <= 24);
});

test("a life change during travel cannot be hidden by station keeping", async () => {
  const { failure, result } = await replay({ changeLifeAt: 4_000 });
  assert(failure, "setup must reject the changed exact life");
  assert.equal(result, undefined);
  assert.match(failure.message, /lifecycle|rendezvous/);
});

test("a transport recovery caller can shorten setup without extending its total deadline", async () => {
  const { failure, result } = await replay({ setupBudgetMs: 10_000 });
  assert(failure, "the simulated24s route must not pass a10s remaining budget");
  assert.equal(result, undefined);
});

test("a larger coasting stage retains the real-key joint barrier and original setup bound", async () => {
  const { result, failure, plannerOptions, now } = await replay({ stageRadiusPx: 384 });
  assert.ifError(failure);
  assert.equal(plannerOptions.length, 1);
  assert.equal(plannerOptions[0].stageRadiusPx, 384);
  assert.equal(plannerOptions[0].hullRadiusPx, 20);
  assert.equal(result.stageRadiusPx, 384);
  assert(result.separationPx <= 24);
  assert(now < 180_000);
});

test("a malformed coasting-stage request fails before navigation", async () => {
  for (const stageRadiusPx of [NaN, 0, 19, 769]) {
    const { failure, plannerOptions } = await replay({ stageRadiusPx });
    assert.match(failure.message, /invalid rendezvous stage radius/);
    assert.equal(plannerOptions.length, 0);
  }
});

test("requested timed-prize stage and avoidance tiles reach the genuine coordinator's planner", async () => {
  const meetingPoint = { x: 500, y: 1000 }, avoidTiles = [{ x: 512, y: 512 }];
  const { result, failure, plannerOptions } = await replay({ meetingPoint, avoidTiles, stageRadiusPx: 20 });
  assert.ifError(failure);
  assert.equal(result.status, "complete");
  assert.deepEqual(plannerOptions[0].meetingPoint, meetingPoint);
  assert.deepEqual([...plannerOptions[0].blockedTiles], avoidTiles);
  assert(result.separationPx <= 24);
});

test("malformed avoidance geometry fails before any setup flight", async () => {
  for (const avoidTiles of [null, [{ x: -1, y: 5 }], [{ x: 5, y: 1024 }], [{ x: 1.5, y: 5 }]]) {
    const { failure, plannerOptions } = await replay({ avoidTiles });
    assert.match(failure.message, /invalid rendezvous avoidance tiles/);
    assert.equal(plannerOptions.length, 0);
  }
});
test("joint-arrival evidence retains the real barrier pose before residual drift during aiming", async () => {
  const { result, failure } = await replay({ finalAimMs: 2500 });
  assert.ifError(failure);
  const distance = pose => Math.hypot(pose.x - result.plan.target.x, pose.y - result.plan.target.y);
  assert.equal(result.jointArrival.length, 2);
  assert(result.jointArrival.every(pose => distance(pose) <= 4
    && Math.hypot(pose.velocityX, pose.velocityY) <= 3.5));
  assert(result.final.some(pose => distance(pose) > 4), "later aim drift is a separate observation");
  assert(result.separationPx <= 24);
});

test("inherited motion stops before conservative route certification", async () => {
  const { result, failure, plannerOptions, brakeCalls } = await replay({ inheritedMotion: true });
  assert.ifError(failure);
  assert.equal(result.status, "complete");
  assert.equal(brakeCalls.length, 2, "both pilots need a verified stopped origin");
  assert.equal(plannerOptions[0].hullRadiusPx, 20, "do not lower the route margin");
});

test("a failed initial stop cannot be accepted by the route planner", async () => {
  const { failure, plannerOptions } = await replay({ inheritedMotion: true, brakeFails: true });
  assert.match(failure?.message || "", /stop|brak/i);
  assert.equal(plannerOptions.length, 0);
});

test("initial braking cannot hide a lifecycle change", async () => {
  const { failure, plannerOptions } = await replay({ inheritedMotion: true, brakeLifeChange: true });
  assert.match(failure?.message || "", /lifecycle|scope/i);
  assert.equal(plannerOptions.length, 0);
});


test("only an explicitly requested setup may recover narrow route origins with ordinary controls", async () => {
  const refused = await replay({ originNeedsRecovery: true });
  assert.match(refused.failure?.message ?? "", /origin lacks conservative hull clearance/);
  assert.equal(refused.originCalls.length, 0);
  const allowed = await replay({ originNeedsRecovery: true, recoverOrigins: true });
  assert.ifError(allowed.failure); assert.equal(allowed.result.status, "complete");
  assert.deepEqual(allowed.originCalls.map(call => call.index), [0, 1]);
  assert.equal(allowed.result.originRecoveries[0].warps[0].life_seq, 1);
  assert.equal(allowed.result.scopes[0], '["123","1",0,1]');
  assert.equal(allowed.result.originsBeforeRecovery[0].x, 0);
  assert.equal(allowed.result.origins[0].x, 64);
});

test("rejected opt-in origin preparation fails before route planning", async () => {
  const result = await replay({ recoverOrigins: true, originRecoveryRejected: true });
  assert.match(result.failure?.message ?? "", /ordinary origin Warp was rejected/);
  assert.equal(result.plannerOptions.length, 0);
});
