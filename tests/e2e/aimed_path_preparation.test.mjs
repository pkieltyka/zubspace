import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { loadRendezvousMap, planPlayerRendezvous } from "./player_rendezvous.mjs";
import { movingHullPathIsClear } from "./bot_combat_evidence.mjs";
import { arrivalControlDistance, brakeVelocityDisposition, routeHeadingErrorTurns } from "./combat_replay.mjs";

const source = fs.readFileSync(new URL("./aimed_bullet_damage.mjs", import.meta.url), "utf8");
const start = source.indexOf("async function placeVictimOnGunLine("), end = source.indexOf("\n}\n", start) + 2;
const signed = (from, to) => ((to - from + 0.5) % 1 + 1) % 1 - 0.5;

test("both aimed rendezvous calls certify the complete firing line on the captured WAN terrain", async () => {
  const { geometry } = loadRendezvousMap("chaos-zone", [path.join(process.cwd(), "assets/zones")]);
  const origins = [{ x: 8264.17, y: 8005.6 }, { x: 8261.43, y: 8006.49 }];
  const lineClear = (source, separation) => movingHullPathIsClear({
    x: source.x, y: source.y, velocityX: 0, velocityY: separation, seconds: 1,
    hullIsOpen: (x, y) => geometry.authoredHullIsTraversable(x, y, 20),
  });
  // The actual previous48px collocation stage contains a wall before the140px
  // target station, independently reproducing the captured browser rejection.
  const previous = planPlayerRendezvous(geometry, origins, { hullRadiusPx: 20, stageRadiusPx: 48 });
  assert.equal(lineClear(previous.target, 140), false);
  const calls = source.match(/await rendezvousPlayers\(alpha, beta, server[^;]*;/g);
  assert.equal(calls?.length, 2, "initial and post-ship-change preparations both matter");
  for (const targetSeparation of [48, 140]) {
    const options = [];
    await vm.runInNewContext(`(async () => { ${calls.join("\n")} })()`, {
      alpha: {}, beta: {}, server: {}, targetSeparation,
      rendezvousPlayers: async (_a, _b, _s, option = {}) => options.push(option),
    });
    for (const option of options) {
      const plan = planPlayerRendezvous(geometry, origins, { hullRadiusPx: 20, ...option });
      // Include the genuine rendezvous receipt's16px arrival error in either
      // axis, not just the ideal tile centre used by the planner.
      for (const x of [-16, 0, 16]) for (const y of [-16, 0, 16]) {
        const station = { x: plan.target.x + x, y: plan.target.y + y };
        assert.equal(lineClear(station, targetSeparation), true);
        assert.equal(lineClear(station, -targetSeparation), true);
      }
    }
  }
});

async function replay({ wrongView = false, retireLife = false } = {}) {
  const sourcePose = { selfPlayerId: 1, playerLifeSeq: 2, playerX: 8617.15, playerY: 8147.29,
    playerVelocityX: 2.56, playerVelocityY: -0.21, playerOrientation: 0.50458,
    remotePositionX: 8524.9, remotePositionY: 8286.68 };
  const targetPose = { selfPlayerId: 0, playerLifeSeq: 2, playerX: 8524.9, playerY: 8286.68,
    playerVelocityX: -2.1, playerVelocityY: 0.5, playerOrientation: 0.00292 };
  const driven = [];
  const shooter = { metricsState: async () => ({ uiMetrics: { ...sourcePose } }) };
  const victim = { metricsState: async () => ({ uiMetrics: { ...targetPose } }),
    hold: async () => {} };
  const controller = vm.runInNewContext(`(${source.slice(start, end)})`, {
    Date, Math, Number, JSON, path, process, repoRoot: process.cwd(), arena: "chaos-zone",
    targetSeparation: 140, loadRendezvousMap, movingHullPathIsClear,
    signedOrientationDelta: signed, compactMetrics: m => m, sleep: async () => {},
    rotateToOrientation: async () => {}, verticalKeyMap: async () => ({ increaseY: "ArrowDown", decreaseY: "ArrowUp" }),
    driveClientToWorldTarget: async (client, target, timeoutMs, settle, tolerance, complete) => {
      assert.equal(client, victim); assert(settle); assert.equal(tolerance, 1);
      assert(timeoutMs > 0 && timeoutMs <= 20_000);
      driven.push({ ...target });
      Object.assign(targetPose, { playerX: target.x, playerY: target.y, playerVelocityX: 0, playerVelocityY: 0 });
      Object.assign(sourcePose, { remotePositionX: target.x + (wrongView ? 30 : 0), remotePositionY: target.y });
      if (retireLife) targetPose.playerLifeSeq++;
      const ready = await complete();
      assert.equal(ready, true);
      return { reached: true, completed: true };
    },
  });
  const placement = await controller({ shooter, victim, shooterOrientation: 0.5, targetSign: 1, label: "captured lateral drift" });
  return { driven, placement };
}

test("captured horizontal drift requires two-axis gun-line preparation", async () => {
  const { driven, placement } = await replay();
  assert.deepEqual(driven, [{ x: 8617.15, y: 8287.29 }]);
  assert.equal(placement.dx, 0); assert(Math.abs(placement.dy - 140) < 1e-9);
});

test("displaced observer presentation and a new life cannot satisfy the unchanged gun-line predicate", async () => {
  await assert.rejects(replay({ wrongView: true }), /could not place separated target on gun line/);
  await assert.rejects(replay({ retireLife: true }), /lifecycle boundary/);
});

test("a vertically settled gun line cannot accept the captured horizontal coast", async () => {
  // Focused64's counterfire has both ships alive, but source/target X speeds
  // of13.23/19.78px/s. A Y-only readiness check can accept that as stopped.
  const poses = [
    { selfPlayerId: 0, playerLifeSeq: 2, playerX: 8960, playerY: 6957,
      playerVelocityX: -13.23, playerVelocityY: 1.54, playerOrientation: 0,
      remotePositionX: 8960, remotePositionY: 6909 },
    { selfPlayerId: 1, playerLifeSeq: 2, playerX: 8960, playerY: 6909,
      playerVelocityX: -19.78, playerVelocityY: 2.46, playerOrientation: 0 },
  ];
  const clients = poses.map(pose => ({ metricsState: async () => ({ uiMetrics: { ...pose } }) }));
  const controller = vm.runInNewContext(`(${source.slice(start, end)})`, {
    Date, Math, Number, JSON, path, process, repoRoot: process.cwd(), arena: "chaos-zone",
    targetSeparation: 48, loadRendezvousMap, movingHullPathIsClear,
    signedOrientationDelta: signed, compactMetrics: m => m, sleep: async () => {},
    rotateToOrientation: async () => {},
    driveClientToWorldTarget: async () => { throw new Error("horizontal coast requires genuine station keeping"); },
  });
  await assert.rejects(controller({ shooter: clients[0], victim: clients[1],
    shooterOrientation: 0, targetSign: -1, label: "captured counterfire coast" }),
  /horizontal coast requires genuine station keeping/);
});

async function replayInertialCounterfire({ separation, quantized, readMs, dropSourceInputs = false, nearStationSpeed = null }) {
  // Execute both the actual coordinator and its actual steering loop. The
  // independent actuator integrates180px/s² thrust and0.575turn/s rotation
  // from authored Chaos Terrier tuning, preserving the captured lateral coast.
  // This is a harness control test, not a substitute for RTC/held-hit evidence.
  const driverSource = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
  const driverStart = driverSource.indexOf("async function driveClientToWorldTarget(");
  const driverEnd = driverSource.indexOf("\nasync function ", driverStart + 1);
  let now = 0, finished = false, result, error;
  const timers = [], keys = [null, null], pulses = [0, 0];
  const sleep = ms => new Promise(resolve => timers.push({ at: now + ms, resolve }));
  const poses = [
    { selfPlayerId: 0, playerLifeSeq: 2, playerX: 8960.09, playerY: 6956.96,
      playerVelocityX: -13.23, playerVelocityY: 1.54, playerOrientation: 0.00625 },
    { selfPlayerId: 1, playerLifeSeq: 2, playerX: 8969.78, playerY: 6908.13,
      playerVelocityX: -19.78, playerVelocityY: 2.46, playerOrientation: 0.23333 },
  ];
  if (nearStationSpeed !== null) {
    // The WAN capture repeatedly returned reached at3.335px/s while its
    // coordinator still required2.7px/s. Start inside both1px stations.
    Object.assign(poses[0], {playerVelocityX:-nearStationSpeed,playerVelocityY:0});
    Object.assign(poses[1], {playerX:poses[0].playerX+0.5,playerY:poses[0].playerY-separation,
      playerVelocityX:-nearStationSpeed,playerVelocityY:0});
  }
  const clients = poses.map((pose, index) => ({
    metricsState: async () => {
      await sleep(readMs);
      return { uiMetrics: { ...pose,
        remotePositionX: poses[1-index].playerX, remotePositionY: poses[1-index].playerY } };
    },
    pulseOnFreshInputStep: async (key, ms) => {
      assert.equal(keys[index], null, "one real-key pilot per actor");
      await sleep(1000 / 60);
      keys[index] = key; pulses[index]++;
      await sleep(quantized ? Math.ceil(ms / (1000 / 60)) * (1000 / 60) : ms);
      keys[index] = null;
      return true;
    },
  }));
  const environment = {
    Date: { now: () => now }, Math, Number, JSON, path, process: { env: {} },
    repoRoot: process.cwd(), arena: "chaos-zone", targetSeparation: separation,
    loadRendezvousMap, movingHullPathIsClear, signedOrientationDelta: signed,
    signedSoakOrientationDelta: signed, compactMetrics: m => m, sleep,
    arrivalControlDistance, brakeVelocityDisposition, routeHeadingErrorTurns,
    pulseClientOnFreshInputStep: (client, key, ms) => client.pulseOnFreshInputStep(key, ms),
    // Heading acquisition has separate canonical-lattice native tests. Give
    // this coordinator an aim delay so residual inertia still matters.
    rotateToOrientation: async (client, target) => {
      await sleep(250);
      poses[clients.indexOf(client)].playerOrientation = target;
    },
  };
  const context = vm.createContext(environment);
  vm.runInContext(driverSource.slice(driverStart, driverEnd), context);
  const coordinator = vm.runInContext(`(${source.slice(start, end)})`, context);
  coordinator({ shooter: clients[0], victim: clients[1], shooterOrientation: 0,
    targetSign: -1, label: "captured inertial counterfire" }).then(
    value => { result = value; finished = true; }, failure => { error = failure; finished = true; });
  for (let iteration = 0; !finished && iteration < 20_000; iteration++) {
    await new Promise(resolve => setImmediate(resolve));
    if (!timers.length) continue;
    const next = Math.min(...timers.map(timer => timer.at));
    for (let remaining = next - now; remaining > 1e-9;) {
      const ms = Math.min(1000 / 60, remaining), dt = ms / 1000;
      poses.forEach((pose, index) => {
        const angle = pose.playerOrientation * Math.PI * 2, key = keys[index];
        if ((key === "w" || key === "s") && !(dropSourceInputs && index === 0)) {
          const acceleration = key === "w" ? 180 : -180;
          pose.playerVelocityX += Math.sin(angle) * acceleration * dt;
          pose.playerVelocityY -= Math.cos(angle) * acceleration * dt;
        }
        pose.playerOrientation = (pose.playerOrientation +
          (key === "d" ? .575 : key === "a" ? -.575 : 0) * dt + 1) % 1;
        const speed = Math.hypot(pose.playerVelocityX, pose.playerVelocityY);
        if (speed > 325) { pose.playerVelocityX *= 325/speed; pose.playerVelocityY *= 325/speed; }
        pose.playerX += pose.playerVelocityX * dt; pose.playerY += pose.playerVelocityY * dt;
      });
      now += ms; remaining -= ms;
    }
    now = next;
    for (let index = timers.length - 1; index >= 0; index--) {
      if (timers[index].at <= now) timers.splice(index, 1)[0].resolve();
    }
  }
  assert.equal(finished, true, "the actual coordinator must terminate within bounded virtual time");
  assert.deepEqual(keys, [null, null], "all key-driving work must drain before returning or failing");
  if (dropSourceInputs) {
    assert.equal(result, undefined, "a coasting source cannot be certified from target placement alone");
    assert.match(error?.message || "", /did not settle|could not place|preparation corridor/);
    return;
  }
  assert.ifError(error);
  assert.ok(result.elapsedMs < 20_000);
  assert.ok(result.shooterSpeed <= 2.7 && result.victimSpeed <= 2.7);
  assert.ok(Math.abs(result.dx) <= 11 && Math.abs(result.watcherDx) <= 17);
  assert.ok(Math.abs(Math.abs(result.dy)-separation) <= 12);
  assert.ok(pulses.every(count => count > 0), "both drifting actors need real steering impulses");
  return { elapsedMs: result.elapsedMs, pulses, routes: result.routes };
}

test("both captured coasting pilots settle within20s under the actual gun-line controller", async () => {
  for (const separation of [48, 140]) for (const quantized of [false, true]) for (const readMs of [5, 20]) {
    await replayInertialCounterfire({ separation, quantized, readMs });
  }
});

test("an unresponsive coasting source fails closed and drains both key pilots", async () => {
  await replayInertialCounterfire({ separation: 48, quantized: true, readMs: 20, dropSourceInputs: true });
});

test("captured WAN rest-band speeds cannot become zero-pulse reached loops", async () => {
  for (const nearStationSpeed of [3.3354160160315836, 2.7060118255469616]) {
    const result=await replayInertialCounterfire({separation:140,quantized:true,readMs:20,nearStationSpeed});
    const returns=result.routes.filter(route=>Number.isFinite(route.speed));
    assert(returns.length>0);
    assert(returns.every(route=>route.speed<=2.7),
      "the route must satisfy its caller's unchanged rest threshold before reporting reached");
  }
});
