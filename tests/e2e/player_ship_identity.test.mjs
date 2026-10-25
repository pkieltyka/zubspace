import assert from "node:assert/strict";
import test from "node:test";
import { hasPlayerShip, playerShipReady, selectPlayerShip } from "./player_ship_identity.mjs";

const ready = (ship = 0, life = 1) => ({
  playerShip: ship, selfRosterShip: ship, playerLifeSeq: life,
  playerAlive: true, serverPlayerAlive: true, playerEnergy: 2800, playerMaxEnergy: 2800,
  localGameplayState: { zoneMapReady: true, zoneFilesReady: true, doorClockFresh: true,
    authorityReadinessSecurity: true, authorityPhase: "Live", pendingShipResetPosition: false,
    pendingShipResetDeliveryId: null, authorityTransitionKind: null },
});

test("maximum-entry Warbird is not Lancaster; both explicit witnesses must agree", () => {
  assert.equal(hasPlayerShip(ready(), 6), false);
  assert.equal(hasPlayerShip(ready(6), 6), true);
  for (const metrics of [{ playerShip: 6 }, { selfRosterShip: 6 },
    { playerShip: 6, selfRosterShip: 0 }, { playerShip: "6", selfRosterShip: 6 }]) {
    assert.equal(hasPlayerShip(metrics, 6), false);
  }
  assert.equal(hasPlayerShip(ready(8), 8), false);
});

test("selection readiness requires live applied map/security and no pending transition", () => {
  assert.equal(playerShipReady(ready()), true);
  for (const field of ["zoneMapReady", "zoneFilesReady", "doorClockFresh", "authorityReadinessSecurity"]) {
    const metrics = ready(); metrics.localGameplayState[field] = false;
    assert.equal(playerShipReady(metrics), false, field);
  }
  for (const patch of [{ authorityPhase: "AwaitingDurableSubmission" },
    { pendingShipResetPosition: true }, { pendingShipResetDeliveryId: 4 },
    { authorityTransitionKind: "Ship" }]) {
    assert.equal(playerShipReady({ ...ready(), localGameplayState: { ...ready().localGameplayState, ...patch } }), false);
  }
  assert.equal(playerShipReady({ ...ready(), serverPlayerAlive: false }), false);
});

test("one genuine selection waits for matching roster, exact successor life and applied baseline", async () => {
  const samples = [ready(), ready(6, 1), { ...ready(6, 2), selfRosterShip: 0 },
    { ...ready(6, 2), localGameplayState: { ...ready().localGameplayState, pendingShipResetDeliveryId: 9 } },
    ready(6, 3), ready(6, 2)];
  const keys = []; let reads = 0;
  const result = await selectPlayerShip({ ship: 6, label: "Lancaster", fromShip: 0,
    read: async () => { reads += 1; return samples.shift(); },
    press: async (...args) => keys.push(args),
    waitUntil: async (label, predicate) => {
      for (let i = 0; i < 10; i++) if (await predicate()) return;
      assert.fail(label);
    },
  });
  assert.equal(reads, 6);
  assert.deepEqual(keys, [["Escape", 200], ["7", 200]]);
  assert.equal(result.fromLifeSeq, 1); assert.equal(result.lifeSeq, 2);
  assert.equal(result.ship, 6); assert.equal(result.rosterShip, 6);
  assert.equal(result.maxEnergy, 2800); assert.equal(result.selectionRequested, true);
});

test("an already-confirmed ship requires no lifecycle request", async () => {
  const result = await selectPlayerShip({ ship: 0, label: "Warbird", read: async () => ready(),
    press: async () => assert.fail("same ship must not request another life"),
    waitUntil: async (label, predicate) => assert.equal(await predicate(), true, label),
  });
  assert.equal(result.selectionRequested, false); assert.equal(result.lifeSeq, 1);
});
