// The whole-attempt shape report is only read when a route actually times out, and the first version of it threw
// ReferenceError: metrics is not defined on that path - invisible through four green gates, because a green gate
// never walks the failure branch. These cases exist so the instrument is exercised on the only path it occupies,
// rather than being trusted because it parsed.
import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import vm from "node:vm";
import { driveClientToWorldTarget, routeHeadingErrorTurns, arrivalControlDistance,
  brakeVelocityDisposition } from "./combat_replay.mjs";

// Run the actual steering loop against an independent, virtual-time inertial
// actuator. This tests the harness, not shipping physics or network admission.
// Values come from the retained d536e96 Chaos failure: 190px/s² thrust,
// roughly0.625 turns/s, and an already coasting pilot facing east on a south leg.
async function replaySettlingApproach({ readMs = 35, rotate = 0, extreme = false, quantized = false, receding = false, warpApproach = false } = {}) {
  const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const start = source.indexOf("async function driveClientToWorldTarget("),
    end = source.indexOf("\nasync function ", start + 1);
  let now = 0;
  const capturedWarp = warpApproach ? JSON.parse(fs.readFileSync(new URL("../fixtures/warp_second_repel_8137398.json", import.meta.url))) : null;
  const m = warpApproach ? { ...capturedWarp.origin } : receding
    ? { playerX: 8482.66, playerY: 8168.24, playerVelocityX: 302.66,
      playerVelocityY: 10.13, playerOrientation: (0.325 + rotate) % 1 }
    : extreme
    ? { playerX: 7539.77, playerY: 8625.63, playerVelocityX: -0.88,
      playerVelocityY: 0.07, playerOrientation: (0.21 + rotate) % 1 }
    : { playerX: 8072, playerY: 4008.3, playerVelocityX: 0,
      playerVelocityY: 70.2, playerOrientation: (0.25 + rotate) % 1 };
  const acceleration = extreme ? 330 : 190;
  const rotation = extreme ? 0.9 : 0.625;
  const advance = (ms, key) => {
    for (let remaining = ms; remaining > 1e-9;) {
      const dt = Math.min(1000 / 60, remaining) / 1000;
      const angle = m.playerOrientation * 2 * Math.PI;
      if (key === "w" || key === "s") {
        const thrust = (key === "w" ? acceleration : -acceleration) * dt;
        m.playerVelocityX += Math.sin(angle) * thrust;
        m.playerVelocityY -= Math.cos(angle) * thrust;
      }
      m.playerOrientation = (m.playerOrientation + (key === "d" ? rotation : key === "a" ? -rotation : 0) * dt + 1) % 1;
      m.playerX += m.playerVelocityX * dt; m.playerY += m.playerVelocityY * dt;
      now += dt * 1000; remaining -= dt * 1000;
    }
  };
  const client = {
    metricsState: async () => { advance(readMs); return { uiMetrics: { ...m } }; },
    pulseOnFreshInputStep: async (key, ms) => {
      advance(1000 / 60);
      // A conservative ceiling actuator: a requested20ms pulse can cover
      // two60Hz inputs, while a4ms fine pulse covers one. The real retained
      // Firefox trace contains both one- and two-input minimum pulses.
      advance(quantized ? Math.ceil(ms / (1000 / 60)) * (1000 / 60) : ms, key);
      return true;
    },
  };
  const ctx = vm.createContext({ Date: { now: () => now }, process: { env: {} },
    sleep: async ms => advance(ms), routeHeadingErrorTurns, arrivalControlDistance, brakeVelocityDisposition,
    signedSoakOrientationDelta: (current, target) => ((target - current + 0.5) % 1 + 1) % 1 - 0.5,
    pulseClientOnFreshInputStep: (actor, key, ms) => actor.pulseOnFreshInputStep(key, ms),
  });
  vm.runInContext(source.slice(start, end), ctx);
  if (warpApproach) {
    // Execute the actual second-Repel coordinator to obtain its current
    // policy; a hard-coded corrected policy would hide a caller regression.
    const gate = source.indexOf("async function runWarpRelocationGate(");
    const policyStart = source.indexOf("  const actorBeforeSecondRepel =", gate);
    const policyEnd = source.indexOf('  await peer.press("q", 80);', policyStart);
    const policyContext = vm.createContext({ actor: { metricsState: async () => ({ uiMetrics: {
      playerX: capturedWarp.plan.destination.x, playerY: capturedWarp.plan.destination.y,
    } }) }, peer: {}, server: {}, navigateAuthoredTarget: async (_peer, _target, _server, options) => ({ reached: true, ...options }) });
    const policy = await vm.runInContext(`(async()=>{${source.slice(policyStart, policyEnd)};return peerRoute;})()`, policyContext);
    const legs = [];
    for (const target of capturedWarp.plan.routes[0].drivePoints) {
      const remaining = policy.timeoutMs - now;
      if (remaining <= 0) break;
      legs.push(await ctx.driveClientToWorldTarget(client, target, remaining, true, 3,
        async () => { advance(readMs); return false; }, policy.maximumDesiredSpeed, policy.desiredSpeedScale));
      if (!legs.at(-1).reached) break;
    }
    return { reached: legs.length === 2 && legs.every(leg => leg.reached) && now <= policy.timeoutMs,
      elapsedMs: now, legs };
  }
  return ctx.driveClientToWorldTarget(client,
    receding ? { x: 8216, y: 8184 } : extreme ? { x: 8184, y: 8376 } : { x: 8072, y: 4760 },
    30_000, true, extreme ? 3 : 4,
    receding ? async () => { advance(readMs); return false; } : null,
    100, 0.5, extreme ? 5_000 : 8_000);
}

test("captured second-Repel cruise settles both waypoints within its original20s budget", async () => {
  for (const readMs of [1, 3, 5, 10, 20]) {
    for (const quantized of [false, true]) {
      const result = await replaySettlingApproach({ warpApproach: true, readMs, quantized });
      assert(result.reached, JSON.stringify({ readMs, quantized, result }));
      assert(result.elapsedMs <= 20_000);
      assert(result.legs.every(leg => leg.distance <= 3 && leg.speed <= 3.5));
    }
  }
});

test("a fast settling route brakes with reverse thrust instead of repeatedly overshooting during half-turns", async () => {
  for (const readMs of [20, 35, 60]) {
    for (const rotate of [0, 0.125, 0.25, 0.375]) {
      const result = await replaySettlingApproach({ readMs, rotate });
      assert(result.reached, `read ${readMs}ms, facing ${rotate}: ${JSON.stringify(result)}`);
      assert(result.distance <= 4 && result.speed <= 3.5, "retain exact arrival and rest criteria");
    }
  }
});

test("the retained Extreme waypoint settles under continuous and quantized keyboard pulses", async () => {
  for (const quantized of [false, true]) {
    for (const readMs of [20, 35, 60, 95]) {
      for (const rotate of [0, 0.125, 0.25, 0.375]) {
        const result = await replaySettlingApproach({ extreme: true, quantized, readMs, rotate });
        assert(result.reached, `quantized${quantized}, read${readMs}ms, facing${rotate}: ${JSON.stringify(result)}`);
        assert(result.distance <= 3 && result.speed <= 3.5,
          "retain the original3px waypoint and3.5px/s stopped-speed contract");
      }
    }
  }
});

test("a genuinely braking receding pilot can turn and return before the original leg deadline", async () => {
  // Captured ef5e269 late recovery starts with302.8px/s away from safety.
  // The authored navigator samples exact scope as well as the driver state;
  // include that second read rather than assuming cost-free completion polls.
  for (const readMs of [20, 35]) {
    for (const quantized of [false, true]) {
      for (const rotate of [0, .125, .25, .375]) {
        const result = await replaySettlingApproach({ extreme: true, receding: true, readMs, quantized, rotate });
        assert(result.reached, JSON.stringify({ readMs, quantized, rotate, result }));
        assert(result.distance <= 3 && result.speed <= 3.5);
      }
    }
  }
});

test("slow instrumentation cannot misclassify measured braking and inbound motion as a frozen far route", async () => {
  // At slower artificial read rates some cases legitimately exhaust the
  // unchanged30sec budget or fail fine settling. Do not call them passes;
  // this regression asserts only the false far-route watchdog diagnosis.
  for (const readMs of [60, 95]) {
    for (const quantized of [false, true]) {
      for (const rotate of [0, .125, .25, .375]) {
        const result = await replaySettlingApproach({ extreme: true, receding: true, readMs, quantized, rotate });
        assert(!(result.reason === "reported speed without progress" && result.distance > 48),
          JSON.stringify({ readMs, quantized, rotate, result }));
      }
    }
  }
});

function stubClient(shape, extra = {}) {
  let n = 0;
  let samples = 0;
  const metrics = () => {
    // The phase must come from the sample counter, not the shared one: pulses bump it too, so a phase derived
    // from `n` advances two counts per iteration, never changes parity, and the "oscillating" pilot turns out to
    // have been standing still. That is what the first draft of this file did, and it reported path 0 truthfully.
    samples += 1;
    n += 1;
    const sign = samples % 2 === 0 ? -1 : 1;
    const offset = shape === "oscillating" ? sign * 3 : 0;
    return {
      uiMetrics: {
        playerX: 5000 + offset, playerY: 6000 + (shape === "oscillating" ? sign * 3 : 0),
        playerVelocityX: shape === "oscillating" ? sign * 28 : 0,
        playerVelocityY: shape === "oscillating" ? sign * 28 : 0,
        playerOrientation: 0, playerAlive: true, inSafeZone: false, clientInputSeq: n,
        ...extra,
      },
    };
  };
  return {
    metricsState: async () => metrics(),
    pulseOnFreshInputStep: async () => { n += 1; },
    press: async () => { n += 1; },
    evaluate: async () => true,
  };
}

test("a timed-out route reports path length against net displacement instead of crashing the failure", async () => {
  // Target 640 px away, budget 120 ms, progress watchdog 8 s: the only reachable exit is the timeout return,
  // which is the branch that used to throw.
  const client = stubClient("oscillating");
  const route = await driveClientToWorldTarget(client, { x: 5640, y: 6000 }, 120, true, 12, null, 90, 0.35, 8_000);
  assert.equal(route.reached, false);
  assert.equal(route.reason, "route timeout", `expected the timeout branch, got ${JSON.stringify(route)}`);
  assert.ok(route.attemptSamples >= 2, `expected several samples, got ${route.attemptSamples}`);
  assert.ok(route.attemptMaxSpeed > 20, `an oscillating pilot must report real speed, got ${route.attemptMaxSpeed}`);
  // The classification this whole instrument exists to make: travelling without getting anywhere.
  assert.ok(route.attemptPathPx > route.attemptNetDisplacementPx,
    `path ${route.attemptPathPx} should exceed net ${route.attemptNetDisplacementPx} for an oscillating pilot`);
});

test("a held-still pilot reports near-zero path length, which is the shape that distinguishes it", async () => {
  const client = stubClient("still");
  const route = await driveClientToWorldTarget(client, { x: 5640, y: 6000 }, 120, true, 12, null, 90, 0.35, 8_000);
  assert.equal(route.reason, "route timeout");
  assert.equal(route.attemptMaxSpeed, 0);
  assert.equal(route.attemptPathPx, 0);
  assert.equal(route.attemptNetDisplacementPx, 0);
});

function marchingClient({ stepPx, speedPxPerS }) {
  // A pilot that closes on the target steadily but never gets there inside the budget: the control loop is doing
  // exactly what the route wants, and the report must say closing with no reversals rather than chatter.
  let samples = 0;
  return {
    async metricsState() {
      // The delay is load-bearing. A stub that resolves instantly lets the driver's budget loop run ~10^6 times in
      // 400 ms, and the first draft of this fixture then "marched" four million pixels, crossed the target, and
      // was counted as opening on it - a truthful report about a pilot that never existed. Real sampling costs a
      // CDP round trip, so the fixture must cost time too.
      await new Promise((resolve) => setTimeout(resolve, 1));
      samples += 1;
      return {
        uiMetrics: {
          playerX: 5000 + stepPx * samples, playerY: 6000,
          playerVelocityX: speedPxPerS, playerVelocityY: 0,
          // East is 0.25 turns in the engine's convention (0 = up, increasing clockwise).
          playerOrientation: 0.25, playerAlive: true, inSafeZone: false, clientInputSeq: samples,
        },
      };
    },
    async pulseOnFreshInputStep() {},
    async press() {},
  };
}

test("heading error uses the engine's 0 = up orientation instead of an x-right bearing", () => {
  const east = (orientation) => routeHeadingErrorTurns(
    { playerX: 0, playerY: 0, playerOrientation: orientation }, { x: 100, y: 0 });
  // Due east is a quarter turn in the engine's orientation space, so a ship pointing up is a quarter turn off.
  // The previous `atan2(dy, dx)` form treated orientation 0 as east and reported zero here.
  assert.equal(east(0), 0.25);
  assert.equal(east(0.25), 0);
  assert.equal(routeHeadingErrorTurns(
    { playerX: 0, playerY: 0, playerOrientation: 0 }, { x: 0, y: -100 }), 0);
  assert.equal(routeHeadingErrorTurns(
    { playerX: 0, playerY: 0, playerOrientation: 0.5 }, { x: 0, y: 100 }), 0);
  // The retained 641 px Chaos leg, read with the corrected convention: the pilot was aligned to under 3 degrees
  // while the payload reported `attemptHeadingErrorMeanTurns 0.257` (roughly 92 degrees).
  const retained = routeHeadingErrorTurns(
    { playerX: 8210, playerY: 7838, playerOrientation: 0.26 }, { x: 8840, y: 7848 });
  assert.ok(retained < 0.02, `the retained leg was aligned, got ${retained}`);
});

test("the reversal counters separate closing, opening and overshoot instead of reporting only distance", async () => {
  const oscillating = await driveClientToWorldTarget(stubClient("oscillating"), { x: 5640, y: 6000 },
    400, true, 12, null, 90, 0.35, 8_000);
  assert.equal(oscillating.reason, "route timeout");
  assert.ok(oscillating.attemptReversals >= 2,
    `a pilot that alternates radial direction must report reversals, got ${oscillating.attemptReversals}`);
  assert.ok(oscillating.attemptClosingSamples > 0 && oscillating.attemptOpeningSamples > 0,
    JSON.stringify({ c: oscillating.attemptClosingSamples, o: oscillating.attemptOpeningSamples }));
  assert.ok(Array.isArray(oscillating.attemptTrace) && oscillating.attemptTrace.length >= 1);
  assert.ok(oscillating.attemptTrace.every((row) => Number.isFinite(row.radialPxPerS)
    && Number.isFinite(row.headingErrorTurns) && typeof row.action === "string"),
    "every trace row must carry the four control-loop fields, not just a position");

  const closing = await driveClientToWorldTarget(marchingClient({ stepPx: 0.5, speedPxPerS: 30 }),
    { x: 9000, y: 6000 }, 400, true, 12, null, 90, 0.35, 8_000);
  assert.equal(closing.reason, "route timeout");
  assert.equal(closing.attemptReversals, 0, "a pilot that only closes must not be reported as fighting itself");
  assert.ok(closing.attemptOpeningSamples === 0 && closing.attemptClosingSamples >= 2,
    JSON.stringify({ c: closing.attemptClosingSamples, o: closing.attemptOpeningSamples }));
  // Target is due east and the stub's orientation is 0.25 turns, so the heading error must read as zero rather
  // than as an artefact of the turn convention.
  assert.equal(closing.attemptHeadingErrorMaxTurns, 0);

  const still = await driveClientToWorldTarget(stubClient("still"), { x: 5640, y: 6000 },
    120, true, 12, null, 90, 0.35, 8_000);
  assert.equal(still.attemptReversals, 0);
  assert.equal(still.attemptClosingSamples, 0);
  assert.equal(still.attemptOpeningSamples, 0);
});

function pausedInputClient({ refuseFor }) {
  // A pilot whose logical input sequencer is lifecycle-paused: the browser-side helper refuses the pulse, so
  // nothing reaches a ship. This is the shape the retained 641 px Chaos leg reported as 1054 thrusts against
  // 38.8 px of path in an authored corridor the map decoder reads as completely open.
  let samples = 0;
  let attempts = 0;
  return {
    pulseAttempts: () => attempts,
    async metricsState() {
      await new Promise((resolve) => setTimeout(resolve, 1));
      samples += 1;
      return {
        uiMetrics: {
          playerX: 8200, playerY: 7848,
          playerVelocityX: 0, playerVelocityY: 0,
          playerOrientation: 0.25, playerAlive: true, inSafeZone: true,
          playerRespawnTimer: 0, playerLifeSeq: 3,
          clientInputSeq: samples,
          localGameplayState: { authorityPhase: "Live", authorityScopeAlive: true },
        },
        inputTrace: [],
      };
    },
    async pulseOnFreshInputStep() {
      attempts += 1;
      return attempts > refuseFor;
    },
    async press() {},
  };
}

test("a refused pulse is never counted as a thrust, and a persistent refusal names the condition", async () => {
  const previous = process.env.ZUBSPACE_E2E_ROUTE_UNDELIVERED_MS;
  process.env.ZUBSPACE_E2E_ROUTE_UNDELIVERED_MS = "30";
  try {
    const client = pausedInputClient({ refuseFor: Number.POSITIVE_INFINITY });
    const route = await driveClientToWorldTarget(
      client, { x: 8840, y: 7848 }, 4_000, true, 12, null, 90, 0.35, 8_000);
    assert.equal(route.reached, false);
    assert.equal(route.reason, "input not admitted",
      `a lifecycle-paused pilot must name the condition, got ${JSON.stringify(route.reason)}`);
    assert.equal(route.pulses, 0, "a refused pulse reached no ship and must not be counted");
    assert.ok(route.undeliveredMs >= 30, `expected an undelivered window, got ${route.undeliveredMs}`);
    assert.ok(client.pulseAttempts() >= 2, "the loop must keep trying while the lifecycle resolves");

    const recovering = pausedInputClient({ refuseFor: 2 });
    const resumed = await driveClientToWorldTarget(
      recovering, { x: 8840, y: 7848 }, 400, true, 12, null, 90, 0.35, 8_000);
    assert.equal(resumed.pulses, recovering.pulseAttempts() - 2,
      "only the pulses the ship actually accepted may be counted");
  } finally {
    if (previous === undefined) delete process.env.ZUBSPACE_E2E_ROUTE_UNDELIVERED_MS;
    else process.env.ZUBSPACE_E2E_ROUTE_UNDELIVERED_MS = previous;
  }
});

test("a settle leg reporting speed without approaching is named instead of burning its budget", async () => {
  // The retained Chaos leg 7: distance and speed unchanged for the whole budget, on a line the map reports
  // as clear, while the client's input sequence kept advancing. A settle leg skips the blocked-route watchdog, so this
  // is the only branch that can name the shape - without it the leg spends 60 s and reports a bare "route timeout".
  let samples = 0;
  const frozen = {
    async metricsState() {
      await new Promise((resolve) => setTimeout(resolve, 1));
      samples += 1;
      return {
        uiMetrics: {
          playerX: 1564, playerY: 9715.87, playerVelocityX: 5.43, playerVelocityY: 61.94,
          playerOrientation: 0.25, playerAlive: true, inSafeZone: false, clientInputSeq: samples,
        },
      };
    },
    async pulseOnFreshInputStep() { samples += 1; return true; },
    async press() {},
  };
  const route = await driveClientToWorldTarget(frozen, { x: 1560, y: 10584 }, 6_000, true, 4, null, 42, 0.25, 200);
  assert.equal(route.reached, false);
  assert.equal(route.reason, "reported speed without progress",
    `expected the speed-without-progress branch, got ${JSON.stringify(route.reason)}`);
  assert.ok(route.attemptSamples < 3_000,
    `must name the shape instead of spending the budget: ${route.attemptSamples} samples`);
  assert.equal(route.attemptNetDisplacementPx, 0);
  assert.ok(route.attemptMaxSpeed > 60, "the fixture must really report a moving ship");
});

test("a stalled leg's failure carries the client's own state, which separates paused from blocked", async () => {
  // The retained Chaos leg 7 reported a live 62.2 px/s closing speed with a frozen position, `inputBits 0`,
  // 1174 successful pulses and `serverPlayerAlive null`. Those last three say the client was paused or
  // lifecycle-pending rather than failing at physics, and the shape report alone could not tell the two apart - which
  // is the whole reason this state is now carried.
  const client = stubClient("still", {
    inputBits: 0,
    serverPlayerAlive: null,
    selfCorrectionCount: 0,
    localGameplayState: {
      gameStepSuppressed: true,
      rebaseSuppressed: true,
      stepAuthorityReady: false,
      doorClockFresh: false,
      authorityReadinessSecurity: true,
      networkSecurityGeneration: { serverSessionId: "71", arenaEpoch: 12 },
      networkSecurityChallengeGeneration: { serverSessionId: "71", arenaEpoch: 11 },
      networkSecurityGenerationMatchesAuthorityScope: true,
      networkSecurityChallengeGenerationMatchesCurrent: false,
    },
  });
  const route = await driveClientToWorldTarget(client, { x: 5640, y: 6000 }, 120, true, 12, null, 90, 0.35, 8_000);
  assert.equal(route.reached, false);
  assert.equal(
    route.attemptLocalGameplayState?.gameStepSuppressed,
    true,
    `the failure must carry the client's own state, got ${JSON.stringify(route.attemptLocalGameplayState)}`,
  );
  assert.equal(route.attemptServerPlayerAlive, null, "an unknown server life state must survive into the report");
  assert.equal(route.attemptSelfCorrectionCount, 0, "and so must the correction count");
  assert.equal(route.attemptLocalGameplayState?.doorClockFresh, false);
  assert.equal(route.attemptLocalGameplayState?.authorityReadinessSecurity, true);
  assert.deepEqual(route.attemptLocalGameplayState?.networkSecurityGeneration, {
    serverSessionId: "71",
    arenaEpoch: 12,
  });
  assert.deepEqual(route.attemptLocalGameplayState?.networkSecurityChallengeGeneration, {
    serverSessionId: "71",
    arenaEpoch: 11,
  });
  assert.equal(route.attemptLocalGameplayState?.networkSecurityGenerationMatchesAuthorityScope, true);
  assert.equal(route.attemptLocalGameplayState?.networkSecurityChallengeGenerationMatchesCurrent, false);
});
