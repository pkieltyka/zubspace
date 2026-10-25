import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { loadRendezvousMap, planAuthoredApproach } from "./player_rendezvous.mjs";

const map = loadRendezvousMap("chaos-zone", [path.resolve("assets/zones")]);
const settings = fs.readFileSync(path.join(map.provenance.directory, "arena.conf"), "utf8");
const setting = name => Number(settings.match(new RegExp(`^${name}=(\\d+)$`, "m"))?.[1]);
// The reference game's WeaponManager::SimulateHit: (ProximityDistance+rawLevel+3)*18-14,
// then overlap with the victim hull. Chaos's Warbird hull is14px, cancelling
// that subtraction. This independent reference calculation explains the RED.
const thorSensor = (setting("ProximityDistance") + 0 + 3) * 18;
const squareDistance = (a, b) => Math.max(Math.abs(a.x - b.x), Math.abs(a.y - b.y));

test("retained special Thor armed and receded before the real Q", () => {
  const target = { x: 8200, y: 8200 }, launch = { x: 8200, y: 8306 };
  assert.equal(thorSensor, 108);
  assert(squareDistance(launch, target) < thorSensor);
  // Captured native spawn velocity1902/618 is in tenths of a pixel per sec.
  const next = { x: Math.round(launch.x + 190.2 * .02), y: Math.round(launch.y + 61.8 * .02) };
  assert(squareDistance(next, target) <= thorSensor);
  assert(squareDistance(next, target) > squareDistance(launch, target),
    "the reference sensor explodes as the acquired target recedes");
});

test("actual post-Burst setup leaves both hostile ships outside Thor and inside Repel", async () => {
  const source = fs.readFileSync(new URL("./special_weapons.mjs", import.meta.url), "utf8");
  const start = source.indexOf("  const thorRepelOrigin ="), end = source.indexOf("  // The Burst wall/damage leg", start);
  assert(start > source.indexOf('activeBurstPresentation != null'));
  assert(end > start);
  const peer = { metricsState: async () => ({ uiMetrics: { playerX: 8200, playerY: 8200 } }) };
  const actor = {}, observer = {}, calls = [];
  const ctx = vm.createContext({ actor, observer, peer,
    settleSpecialWeaponLane: async (client, target, options) => {
      const origin = client === actor ? { x: 8200, y: 8306 } : { x: 8264, y: 8424 };
      const route = planAuthoredApproach(map.geometry, origin, target, { arrivalRadiusPx: 0 });
      assert.equal(route.routes[0].drivePoints.length, 1);
      assert.equal(options.routeTimeoutMs, 15_000);
      calls.push(client); return { reached: true, route };
    },
  });
  const stages = await vm.runInContext(`(async()=>{${source.slice(start, end)};return thorRepelStage;})()`, ctx);
  assert.deepEqual(calls, [observer, actor]);
  for (const hostile of [{ x: 8200, y: 8200 }, stages.observer]) {
    assert(squareDistance(stages.actor, hostile) > thorSensor + 20);
    const atQ = { x: stages.actor.x + 190.2 * .083, y: stages.actor.y + 61.8 * .083 };
    assert(squareDistance(atQ, hostile) > thorSensor);
    assert(squareDistance(atQ, { x: 8200, y: 8200 }) < setting("RepelDistance"));
  }
});
