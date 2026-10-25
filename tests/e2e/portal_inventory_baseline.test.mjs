import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const fixture = JSON.parse(fs.readFileSync(new URL("../fixtures/portal_inventory_baseline_8137398.json", import.meta.url)));
const source = fs.readFileSync(new URL("./combat_replay.mjs", import.meta.url), "utf8");
const start = source.indexOf('await waitUntil("Shift+0 bypasses and preserves the live portal", async () => {');
const body = source.indexOf("async () => {", start) + "async () => {".length;
const end = source.indexOf("\n  }, 6_000);", body);
assert.ok(start >= 0 && end > body);

function capturedInventory(frame) {
  const packet = Buffer.from(frame.bytes);
  assert.equal(packet[0], 0x70);
  let offset = 21;
  const sizes = [2, 2, 2, 2, 1, 2, 2, 1, 2, 1, 2, 4, 5, 4];
  for (let index = 0; index < packet[18] + packet[19]; index++) {
    const mask = packet.readUInt16LE(offset + 2);
    offset += 4 + sizes.reduce((sum, size, bit) => sum + ((mask & (1 << bit)) ? size : 0), 0);
    if ((packet[1] & 8) && (mask & (1 << 13))) offset += 4;
  }
  for (let index = 0; index < packet[20]; index++) {
    const kind = packet[offset], length = packet[offset + 1];
    offset += 2;
    if (kind === 4) return { revision: packet.readUInt32LE(offset + 4), portals: packet[offset + 38] };
    offset += length;
  }
  throw new Error("captured private inventory missing");
}

async function convergence({ inventoryDelta = 0, endpointDelta = 0, usedPortal = false, pending = false } = {}) {
  const old = capturedInventory(fixture.afterPlacement), before = capturedInventory(fixture.beforeRandomWarp);
  const endpoint = { portalX: fixture.placement.position_x, portalY: fixture.placement.position_y };
  const local = { ...endpoint, portals: before.portals + inventoryDelta, portalActive: true, pendingPortalWarp: pending };
  local.portalY += endpointDelta;
  const ctx = vm.createContext({ Math, server: { logs: ["captured terminal"] }, randomOffset: 0, playerId: fixture.randomWarp.player_id,
    randomTerminal: null, parseZubspaceEvents: () => [{ event: { ...fixture.randomWarp, used_portal: usedPortal } }],
    actor: { metricsState: async () => ({ uiMetrics: { playerX: fixture.randomWarp.position_x,
      playerY: fixture.randomWarp.position_y, localGameplayState: local } }) },
    after: { uiMetrics: { localGameplayState: { ...endpoint, portals: old.portals } } },
    randomBefore: { uiMetrics: { localGameplayState: { ...endpoint, portals: before.portals } } },
  });
  return vm.runInContext(`(async()=>{${source.slice(body, end)}})()`, ctx);
}

test("captured natural MultiPrize changes inventory before the random Warp", () => {
  assert.deepEqual(capturedInventory(fixture.afterPlacement), { revision: 37, portals: 0 });
  assert.deepEqual(capturedInventory(fixture.beforeRandomWarp), { revision: 38, portals: 1 });
  assert.equal(fixture.randomWarp.used_portal, false);
  assert.ok(fixture.beforeRandomWarp.at < 1000532283250, "inventory arrives before the captured Shift+0");
});

test("actual random-Warp predicate preserves the immediate pre-action inventory after a natural award", async () => {
  assert.equal(await convergence(), true);
});

for (const [name, options] of Object.entries({ spend: { inventoryDelta: -1 }, inventedAward: { inventoryDelta: 1 },
  endpointChange: { endpointDelta: 1 }, consumedPortal: { usedPortal: true }, pendingTerminal: { pending: true } })) {
  test(`random-Warp predicate still rejects ${name}`, async () => assert.equal(await convergence(options), false));
}
