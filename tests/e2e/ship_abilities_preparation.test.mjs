import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";
import { loadRendezvousMap, planPlayerRendezvous } from "./player_rendezvous.mjs";

const repoRoot = path.resolve(".");
const capture = JSON.parse(fs.readFileSync("tests/fixtures/ship_abilities_pickup_8137398.json"));
const settings = fs.readFileSync("assets/zones/chaos-zone/arena.conf", "utf8");
// Same authored two-player virtual region as hidden_prizes.rs. Expand by
// both pickup hulls and the retained post-heading arrival tolerance.
const section = settings.match(/^\[Prize\]([\s\S]*?)(?=^\[|$(?![\s\S]))/m)[1];
const value = name => Number(section.match(new RegExp(`^${name}=(\\d+)$`, "m"))[1]);
const extent = value("MinimumVirtual") + value("UpgradeVirtual") * 2;
const offset = (1024 - extent) / 2;
const field = { min: (offset + 1 + 0.5) * 16 - 14 - 8 - 16,
  max: (offset + extent - 2 + 0.5) * 16 + 14 + 8 + 16 };
const overlaps = p => p.x >= field.min && p.x <= field.max && p.y >= field.min && p.y <= field.max;

test("the retained successor-life rendezvous crosses the live prize field and has a real MultiPrize notification", () => {
  assert.equal(capture.observedShip, 1);
  assert.equal(capture.observedLifeSeq, 2);
  assert.equal(capture.observedPickupCount, 1);
  assert.equal(capture.observedNotification, "MultiPrize!");
  let crossed = false;
  for (const route of capture.plan.routes) {
    let previous = route.from;
    for (const point of route.drivePoints) {
      const samples = Math.max(1, Math.ceil(Math.hypot(point.x - previous.x, point.y - previous.y)));
      for (let index = 0; index <= samples; index++) {
        if (overlaps({ x: previous.x + (point.x - previous.x) * index / samples,
          y: previous.y + (point.y - previous.y) * index / samples })) crossed = true;
      }
      previous = point;
    }
  }
  assert.equal(crossed, true);
});

test("the actual starting-loadout audit remains outside every possible two-player prize pickup", async t => {
  const source = fs.readFileSync("tests/e2e/ship_abilities.mjs", "utf8");
  const start = source.indexOf("async function main() {") + "async function main() {".length;
  const end = source.indexOf("  const browsers =", start);
  assert.ok(start > 0 && end > start);
  const fixtures = [];
  t.after(() => { for (const fixture of fixtures) fs.rmSync(fixture.root, { recursive: true, force: true }); });
  const startup = await vm.runInNewContext(`(async()=>{${source.slice(start, end)};return {server};})()`, {
    process: { env: {} }, repoRoot, appRoot: repoRoot, path,
    freePort: async () => 10000, startStaticServer: async () => ({ port: 10001 }),
    createFocusedBrowserEvidence: () => ({}),
    createSpawnRegionFixture: options => { const fixture = createSpawnRegionFixture(options); fixtures.push(fixture); return fixture; },
    startRealServer: (port, options) => ({ port, options }),
  });
  const fixture = fixtures[0];
  const loaded = loadRendezvousMap("chaos-zone", [startup.server.options.zoneRoot ?? path.join(repoRoot, "assets/zones")]);
  const origins = fixture ? [0, 1].map(() => ({ x: fixture.spawnRegion.x * 16 + 8, y: fixture.spawnRegion.y * 16 + 8 }))
    : capture.plan.routes.map(route => route.from);
  const plan = planPlayerRendezvous(loaded.geometry, origins, { hullRadiusPx: 20, stageRadiusPx: fixture ? 20 : 48 });
  assert.ok(origins.every(origin => !overlaps(origin)) && !overlaps(plan.target) &&
    plan.routes.every(route => route.drivePoints.every(point => !overlaps(point))),
  "starting-loadout setup must not cross the field in which the captured ambient pickup occurred");
  assert.equal(fixtures.length, 1);
  assert.equal(startup.server.options.zoneRoot, fixture.root);
  assert.equal(startup.server.options.env.ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES, "1");
  assert.equal(startup.server.options.env.ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT, undefined,
    "retain the ordinary product maximum-loadout policy");
  assert.ok(plan.routes.every(route => route.drivePoints.length === 0), "every ship life starts already at the same clear station");
  const original = loadRendezvousMap("chaos-zone", [path.join(repoRoot, "assets/zones")]);
  assert.equal(loaded.provenance.mapSha256, original.provenance.mapSha256);
  const withoutSpawn = text => text.replace(/^Team[0-3]-(X|Y|Radius)=.*$/gm, "<scenario Spawn>");
  assert.equal(withoutSpawn(fs.readFileSync(path.join(loaded.provenance.directory, "arena.conf"), "utf8")), withoutSpawn(settings));
});
