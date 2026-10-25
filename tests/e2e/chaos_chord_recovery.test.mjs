import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { loadRendezvousMap } from "./player_rendezvous.mjs";
import { bestBoostHeading } from "./authored_map_geometry.mjs";

test("captured respawn lacks a runway and conservative route origin; central fallback has a runway", () => {
  const { geometry } = loadRendezvousMap("chaos-zone", ["assets/zones"]);
  const heading = (x, y) => bestBoostHeading({
    hullIsTraversable: (a, b) => geometry.authoredHullIsTraversable(a, b),
    safetyDistance: () => 0, x, y, travelPx: 512, clearancePx: 0,
  });
  assert.equal(heading(6049.21, 8769.04), null);
  assert.equal(geometry.authoredHullIsTraversable(6049.21, 8769.04, 20), false);
  assert.equal(geometry.authoredHullIsTraversable(8200, 8200, 20), true);
  assert.equal(heading(8200, 8200).runwayPx, 512);
});

async function replay({ failedWarp = false, failedAim = false, badFinalHeading = false } = {}) {
  const source = fs.readFileSync(new URL("./keyboard_chord_matrix.mjs", import.meta.url), "utf8");
  const start = source.indexOf("async function prepareChaosChordAttempt("), end = source.indexOf("async function settleLifecycleClient", start);
  assert(start >= 0 && end > start);
  const calls = [], server = {};
  const m = { selfPlayerId: 0, playerLifeSeq: 2, playerX: 6049.21, playerY: 8769.04,
    playerVelocityX: 0, playerVelocityY: 0, playerOrientation: 0.25,
    playerAlive: true, serverPlayerAlive: true };
  const actor = { metricsState: async () => ({ uiMetrics: structuredClone(m) }), releaseKeys: async () => calls.push("release") };
  const ctx = vm.createContext({ Math, Number, Date, Error, console: { log() {} },
    authoredRouteGeometry: { authoredHullIsTraversable: () => true },
    distanceToAuthoredSafety: () => 0,
    brakeVelocityDisposition: () => ({ braked: true }),
    brakeClientVelocity: async () => ({ braked: true, speed: 0 }),
    runwayAlongHeading: options => options.x === 8200 && !badFinalHeading ? { runwayPx: 512 } : null,
    bestBoostHeading: options => options.x === 8200 ? { dx: 1, dy: 0, runwayPx: 512 } : null,
    compactMetrics: value => value,
    prepareKeyboardLifecycleOrigin: async options => {
      calls.push("origin"); assert.equal(options.playerId, 0); assert.equal(options.initialLifeSeq, 2);
      assert.equal(options.server, server);
      if (failedWarp) throw new Error("keyboard origin Warp rejected or changed scope");
      m.playerX = 7272; m.playerY = 8824;
      return { warps: [{ outcome: 1 }], final: { lifeSeq: 2, energy: 1700, maxEnergy: 1700 } };
    },
    navigateAuthoredTarget: async (_actor, target, receivedServer, options) => {
      calls.push("navigate"); assert.deepEqual({ ...target }, { x: 8200, y: 8200 });
      assert.equal(receivedServer, server); assert(options.timeoutMs <= 90_000);
      m.playerX = m.playerY = 8200;
      return { keysOnly: true, status: "complete" };
    },
    orientClientForProtocolDirection: async () => ({ oriented: !failedAim }),
    waitUntil: async (_label, predicate) => assert.equal(await predicate(), true),
  });
  vm.runInContext(source.slice(start, end), ctx);
  return { stage: await ctx.prepareChaosChordAttempt(actor, { afterburner: true, server }), calls };
}

test("actual Chaos coordinator recovers a route origin then flies before certifying the measured runway", async () => {
  const { stage, calls } = await replay();
  assert.deepEqual(calls, ["release", "origin", "navigate"]);
  assert.equal(stage.originRecovery.final.lifeSeq, 2);
  assert.equal(stage.runwayRoute.keysOnly, true);
  assert.equal(stage.runway.runwayPx, 512);
});

test("rejected recovery, failed aim and an uncertified final heading remain setup failures", async () => {
  await assert.rejects(replay({ failedWarp: true }), /Warp rejected/);
  await assert.rejects(replay({ failedAim: true }), /would not turn/);
  await assert.rejects(replay({ badFinalHeading: true }), /actual settled heading/);
});
