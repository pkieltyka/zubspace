import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";
import { loadRendezvousMap } from "./player_rendezvous.mjs";
import { planNaturalPrizeNavigation } from "./natural_prize_navigation.mjs";

const shipping = path.resolve("assets/zones");
const capture = JSON.parse(fs.readFileSync("tests/fixtures/antiwarp_pickup_expiry_8137398.json"));

test("the retained natural Antiwarp target expires before its actual random-spawn detour finishes", () => {
  const { geometry } = loadRendezvousMap("chaos-zone", [shipping]);
  assert.equal(capture.routeReached, true);
  assert.equal(capture.routeCompleted, false);
  assert.ok(capture.actualRouteDurationMs > capture.ttlTicks * 10);
  assert.ok(capture.finalServerTick > capture.prize.expiresAtTick);
  // Even selection at the first possible spawn tick cannot satisfy the
  // existing terrain/TTL-qualified preparation budget. No extra TTL is added.
  assert.equal(planNaturalPrizeNavigation(geometry, capture.origin, capture.prize, capture.spawnTick), null);
});

test("the actual Antiwarp lane declares only its timed-pickup origin and preserves Chaos gameplay tuning", async t => {
  const source = fs.readFileSync("tests/e2e/antiwarp_scope.mjs", "utf8");
  const start = source.indexOf("async function main() {") + "async function main() {".length;
  const end = source.indexOf("  const browsers =", start);
  assert.ok(start > 0 && end > start);
  const fixtures = [];
  t.after(() => { for (const fixture of fixtures) fs.rmSync(fixture.root, { recursive: true, force: true }); });
  const startup = await vm.runInNewContext(`(async()=>{${source.slice(start, end)};return {fixture,server};})()`, {
    process: { env: {} },
    freePort: async () => 10000,
    startStaticServer: async () => ({ port: 10001 }),
    createFocusedBrowserEvidence: () => ({}),
    createSpawnRegionFixture: options => { const fixture = createSpawnRegionFixture(options); fixtures.push(fixture); return fixture; },
    startRealServer: (port, options) => ({ port, options }),
  });
  const startupMap = loadRendezvousMap("chaos-zone", [startup.server.options.zoneRoot ?? shipping]);
  const startupOrigin = fixtures[0] ? { x: fixtures[0].spawnRegion.x * 16 + 8,
    y: fixtures[0].spawnRegion.y * 16 + 8 } : capture.origin;
  assert.ok(planNaturalPrizeNavigation(startupMap.geometry, startupOrigin, capture.prize, capture.spawnTick),
    "the actual scenario origin must permit the original pickup before its unchanged expiry");
  assert.equal(fixtures.length, 1, "this timed acquisition lane needs an explicit private origin");
  const fixture = fixtures[0];
  assert.deepEqual(fixture.spawnRegion, { x: 512, y: 512, radius: 0 });
  assert.equal(startup.server.options.zoneRoot, fixture.root);
  assert.equal(startup.server.options.botCount, 0);
  assert.equal(startup.server.options.env.ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES, "1");
  assert.equal(startup.server.options.env.ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT, "false");
  const declared = loadRendezvousMap("chaos-zone", [fixture.root]);
  const original = loadRendezvousMap("chaos-zone", [shipping]);
  assert.equal(declared.provenance.mapSha256, original.provenance.mapSha256);
  const withoutSpawn = text => text.replace(/^Team[0-3]-(X|Y|Radius)=.*$/gm, "<explicit scenario Spawn>");
  assert.equal(withoutSpawn(fs.readFileSync(path.join(declared.provenance.directory, "arena.conf"), "utf8")),
    withoutSpawn(fs.readFileSync(path.join(original.provenance.directory, "arena.conf"), "utf8")));
  const prize = structuredClone(capture.prize);
  const route = planNaturalPrizeNavigation(declared.geometry, { x: 8200, y: 8200 }, prize, capture.spawnTick);
  assert.ok(route);
  assert.equal(route.plan.routes[0].drivePoints.length, 1);
  assert.ok(route.requiredTtlTicks <= capture.ttlTicks);
  assert.deepEqual(prize, capture.prize, "neither prize identity nor its real lifetime changes");
  // The co-located witness must not collect the green while only turning.
  assert.ok(Math.abs(prize.x - 8200) >= 14 + 8);
});
