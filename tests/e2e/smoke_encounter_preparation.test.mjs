import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { prepareSmokeWeaponEncounter } from "./smoke_encounter_preparation.mjs";

test("a stopped early arrival holds station during its peer's twelve-second separation flight", async () => {
  // Execute the actual helper against independent inertial poses and a
  // virtual-clock keyboard actuator. The prior helper left the first ship
  // coasting3px/s for12s and then failed its unchanged16px final guard.
  const source = fs.readFileSync("tests/e2e/smoke_encounter_preparation.mjs", "utf8")
    .replace("export async function", "async function");
  let now = 0, done = false, evidence, error;
  const timers = [], drives = [];
  const sleep = ms => new Promise(resolve => timers.push({ at: now + ms, resolve }));
  const metrics = [0, 1].map(id => ({ selfPlayerId: id, playerLifeSeq: 1,
    playerX: 8200, playerY: 8200, playerVelocityX: 0, playerVelocityY: id ? 0 : 3,
    inSafeZone: false, localGameplayState: { authorityScopeServerSessionId: "12", authorityScopeArenaStreamId: "1" } }));
  const clients = metrics.map((m, index) => ({ index, uiMetrics: async () => structuredClone(m) }));
  const helper = vm.runInNewContext(`${source};prepareSmokeWeaponEncounter`, {
    Date: { now: () => now }, setTimeout: (resolve, ms) => timers.push({ at: now + ms, resolve }),
  });
  const navigation = {
    exactLiveGameplayScope: m => m.playerLifeSeq === 1,
    rendezvousPlayers: async () => ({ plan: { target: { x: 8200, y: 8200 } } }),
    driveClientToWorldTarget: async (client, target, timeout) => {
      const duration = client.index ? 12_000 : 200;
      assert.ok(duration <= timeout, "the actuator must respect the real remaining leg budget");
      drives.push({ index: client.index, startedAt: now, target: { ...target } });
      await sleep(duration);
      Object.assign(metrics[client.index], { playerX: target.x, playerY: target.y,
        playerVelocityX: 0, playerVelocityY: 0 });
      return { reached: true };
    },
    orientClientForProtocolDirection: async () => ({ oriented: true }),
  };
  helper(...clients, {}, { navigation }).then(value => { evidence = value; done = true; },
    failure => { error = failure; done = true; });
  for (let iterations = 0; !done && iterations < 10_000; iterations++) {
    await new Promise(resolve => setImmediate(resolve));
    if (!timers.length) continue;
    const next = Math.min(...timers.map(timer => timer.at));
    const seconds = (next - now) / 1000;
    for (const m of metrics) { m.playerX += m.playerVelocityX * seconds; m.playerY += m.playerVelocityY * seconds; }
    now = next;
    for (let index = timers.length - 1; index >= 0; index--) {
      if (timers[index].at <= now) timers.splice(index, 1)[0].resolve();
    }
  }
  assert.equal(done, true, "joint arrival must finish within bounded virtual time");
  assert.ifError(error);
  assert.equal(evidence.status, "complete");
  assert.ok(drives.some(drive => drive.index === 0), "the coasting peer requires a genuine corrective driver call");
  assert.ok(now < 180_000);
  assert.ok(evidence.separationPx >= 224 && evidence.separationPx <= 288);
});

async function replay({ farOriginRetained = false, lifeChange = false, drift = false,
  failedLeg = false, failedAim = false, unsafe = false } = {}) {
  const calls = [];
  const metrics = [0, 1].map(id => ({ selfPlayerId: id, playerLifeSeq: 2,
    playerX: id ? 7364 : 9373, playerY: id ? 8484 : 7760,
    playerVelocityX: 0, playerVelocityY: 0, inSafeZone: false,
    localGameplayState: { authorityScopeServerSessionId: "123", authorityScopeArenaStreamId: "1" } }));
  const clients = metrics.map((m, index) => ({ index, uiMetrics: async () => structuredClone(m) }));
  const server = { zoneRoot: "/actual-temporary-zone-root" };
  const navigation = {
    exactLiveGameplayScope: m => m?.localGameplayState?.authorityScopeServerSessionId === "123",
    rendezvousPlayers: async (alpha, beta, actualServer, options) => {
      assert.equal(alpha, clients[0]); assert.equal(beta, clients[1]); assert.equal(actualServer, server);
      calls.push({ kind: "certified-stage", ...options });
      if (!farOriginRetained) for (const m of metrics) { m.playerX = 8200; m.playerY = 8200; }
      return { status: "complete", plan: { target: { x: 8200, y: 8200 } } };
    },
    driveClientToWorldTarget: async (client, target, timeout, settle, tolerance, complete, maxSpeed, taper) => {
      calls.push({ kind: "real-key-leg", target, timeout, settle, tolerance, complete, maxSpeed, taper });
      assert.ok(clients.includes(client));
      metrics[client.index].playerX = target.x; metrics[client.index].playerY = target.y;
      if (lifeChange) metrics[1].playerLifeSeq++;
      if (drift) metrics[0].playerVelocityY = 4;
      if (unsafe) metrics[1].inSafeZone = true;
      return { reached: !failedLeg };
    },
    orientClientForProtocolDirection: async (client, x, y, timeout) => {
      calls.push({ kind: "parallel-heading", index: client.index, x, y, timeout });
      return { oriented: !failedAim };
    },
  };
  let evidence, error;
  try { evidence = await prepareSmokeWeaponEncounter(...clients, server, { navigation }); }
  catch (failure) { error = failure; }
  return { calls, clients, evidence, error };
}

test("random near-tier boundary origins need a real stopped encounter, not a rendered recovery bullet", async () => {
  const { evidence, error, clients, calls } = await replay();
  assert.ifError(error);
  assert.equal(evidence.status, "complete");
  assert.equal(evidence.separationPx, 256);
  assert.equal(clients[0].smokeEncounterTrace[0], evidence);
  assert.equal(clients[1].smokeEncounterTrace[0], evidence);
  assert.equal(calls[0].stageRadiusPx, 384);
  assert.equal(calls[0].recoverOrigins, true);
  assert.deepEqual(calls[1].target, { x: 8456, y: 8200 });
  assert.equal(calls[1].settle, true);
  assert.equal(calls[1].tolerance, 4);
  assert.ok(calls[1].timeout > 0 && calls[1].timeout <= 30_000);
  assert.ok(calls.slice(2).every(call => call.x === 0 && call.y === -1));
  // Setup cannot manufacture weapon facts or replace the delivery oracle.
  assert.deepEqual(calls.map(call => call.kind),
    ["certified-stage", "real-key-leg", "parallel-heading", "parallel-heading"]);
});

for (const option of ["farOriginRetained", "lifeChange", "drift", "failedLeg", "failedAim", "unsafe"]) {
  test(`smoke setup refuses ${option} and retains its failed preparation`, async () => {
    const { evidence, error, clients } = await replay({ [option]: true });
    assert.equal(evidence, undefined);
    assert.ok(error);
    if (option === "drift") assert.ok(error.errors?.some(failure => /stopped live receipt/.test(failure.message)),
      "reject the actual false driver-stop receipt, not a mock's actor restriction");
    const retained = clients[0].smokeEncounterTrace[0];
    assert.equal(retained.status, "failed");
    assert.ok(retained.finishedAt >= retained.startedAt);
    assert.ok(retained.error.length > 0);
  });
}

test("the smoke retains cold setup evidence and stages every original-carrier and mine proof", () => {
  const source = fs.readFileSync("tools/browser_real_server_smoke.mjs", "utf8");
  assert.ok(source.includes("smokeEncounterTrace: this.smokeEncounterTrace || []"));
  assert.ok(source.includes("rendezvousTrace: this.rendezvousTrace || []"));
  for (const pair of ["alpha, beta", "target, source", "returningClient, peerClient"]) {
    assert.ok(source.includes(`await prepareSmokeWeaponEncounter(${pair}, { ...server, zoneRoot: smokeZoneRoot })`));
  }
});
