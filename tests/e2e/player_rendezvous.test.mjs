import test from "node:test";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { authoredMapGeometry } from "./authored_map_geometry.mjs";
import { loadRendezvousMap, planPlayerRendezvous } from "./player_rendezvous.mjs";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";

const tileRecords = rows => Buffer.concat(rows.map(([x, y, id]) => {
  const record = Buffer.alloc(4); record.writeUInt32LE((x | y << 12 | id << 24) >>> 0); return record;
}));
const verify = (geometry, plan, blocked = []) => {
  assert.equal(geometry.tileAt(plan.target.x, plan.target.y), 0);
  for (const route of plan.routes) {
    let prior = route.from;
    for (const point of route.drivePoints) {
      assert(Math.hypot(point.x - prior.x, point.y - prior.y) <= plan.maxLegPx + 1e-6);
      const steps = Math.max(1, Math.ceil(Math.hypot(point.x - prior.x, point.y - prior.y)));
      for (let i = 0; i <= steps; i++) {
        const x = prior.x + (point.x - prior.x) * i / steps, y = prior.y + (point.y - prior.y) * i / steps;
        assert(geometry.authoredHullIsTraversable(x, y, plan.hullRadiusPx), `blocked segment ${JSON.stringify({ prior, point, x, y })}`);
        assert(!blocked.some(p => Math.abs(p.x * 16 + 8 - x) <= plan.hullRadiusPx + 8 &&
          Math.abs(p.y * 16 + 8 - y) <= plan.hullRadiusPx + 8));
      }
      prior = point;
    }
    assert.deepEqual(prior, plan.target);
  }
};

test("retained legacy-team corner entries need a declared connected encounter fixture", () => {
  const root = path.resolve(new URL("../../assets/zones", import.meta.url).pathname);
  const stock = loadRendezvousMap("chaos-zone", [root]);
  const origins = [{ x: 14184, y: 2600 }, { x: 2632, y: 3432 }];
  for (const point of origins) assert(stock.geometry.authoredHullIsTraversable(point.x, point.y, 20));
  assert.throws(() => planPlayerRendezvous(stock.geometry, origins, { hullRadiusPx: 20, stageRadiusPx: 48 }),
    /no common conservative rendezvous route/, "the original gate cannot fly between these components");
  const fixture = createSpawnRegionFixture({ zoneId: "chaos-zone", spawnRegion: { x: 512, y: 512, radius: 2 } });
  try {
    const configured = loadRendezvousMap("chaos-zone", [fixture.root]);
    assert.equal(configured.provenance.mapSha256, stock.provenance.mapSha256);
    const original = fs.readFileSync(path.join(stock.provenance.directory, "arena.conf"), "utf8").split("\n");
    const changed = fs.readFileSync(path.join(configured.provenance.directory, "arena.conf"), "utf8").split("\n");
    assert.equal(changed.length, original.length);
    const differences = changed.flatMap((line, i) => line !== original[i] ? [line] : []);
    assert.equal(differences.length, 12);
    assert(differences.every(line => /^Team[0-3]-(?:X|Y|Radius)=/.test(line)), "no balance or authority field changes");
    const declared = [{ x: 510 * 16 + 8, y: 510 * 16 + 8 }, { x: 514 * 16 + 8, y: 514 * 16 + 8 }];
    const plan = planPlayerRendezvous(configured.geometry, declared, { hullRadiusPx: 20, stageRadiusPx: 48 });
    verify(configured.geometry, plan);
    assert.equal(plan.hullRadiusPx, 20);
    assert.equal(plan.stageRadiusPx, 48);
    assert.equal(stock.provenance.settingsSha256, fixture.originalSettingsSha256);
    assert.equal(configured.provenance.settingsSha256, fixture.settingsSha256);
  } finally {
    fs.rmSync(fixture.root, { recursive: true, force: true });
  }
});

test("separated pilots get capped hull-clear routes to one firing stage", () => {
  const geometry = authoredMapGeometry(Buffer.alloc(0));
  const plan = planPlayerRendezvous(geometry, [{ x: 8200, y: 8200 }, { x: 6200, y: 7800 }], { maxLegPx: 400 });
  verify(geometry, plan);
  const distance = plan.routes.reduce((sum, route) => sum + route.distancePx, 0);
  assert(distance > 2000);
  assert(plan.routes.every(route => route.distancePx < distance * 0.7), "both pilots share the real travel");
});

test("the route turns around a wall and never simplifies across its corner", () => {
  const rows = [];
  for (let y = 490; y <= 530; y++) rows.push([512, y, 1]);
  const geometry = authoredMapGeometry(tileRecords(rows));
  const plan = planPlayerRendezvous(geometry, [{ x: 8040, y: 8200 }, { x: 8360, y: 8200 }]);
  assert(plan.routes.reduce((sum, route) => sum + route.distancePx, 0) > 700);
  verify(geometry, plan);
});

test("authored doors are blocked regardless of their animation phase", () => {
  const geometry = authoredMapGeometry(tileRecords([[512, 512, 169]]));
  const plan = planPlayerRendezvous(geometry, [{ x: 8104, y: 8200 }, { x: 8296, y: 8200 }]);
  verify(geometry, plan);
});

test("temporary brick hulls are avoided too", () => {
  const geometry = authoredMapGeometry(Buffer.alloc(0));
  const blocked = Array.from({ length: 16 }, (_, i) => ({ x: 512, y: 505 + i }));
  const plan = planPlayerRendezvous(geometry, [{ x: 8104, y: 8200 }, { x: 8296, y: 8200 }], { blockedTiles: blocked });
  verify(geometry, plan, blocked);
});

test("safety can be flown through but cannot become the firing stage", () => {
  const rows = [];
  for (let x = 508; x <= 516; x++) for (let y = 508; y <= 516; y++) rows.push([x, y, 171]);
  const geometry = authoredMapGeometry(tileRecords(rows));
  const plan = planPlayerRendezvous(geometry, [{ x: 8200, y: 8200 }, { x: 8408, y: 8200 }]);
  assert(Math.hypot(plan.target.x - 8200, plan.target.y - 8200) > 100);
  verify(geometry, plan);
});

test("unreachable peers and invalid origins fail instead of borrowing a blocked route", () => {
  const rows = Array.from({ length: 1024 }, (_, y) => [512, y, 1]);
  const geometry = authoredMapGeometry(tileRecords(rows));
  assert.throws(() => planPlayerRendezvous(geometry, [{ x: 8104, y: 8200 }, { x: 8296, y: 8200 }]), /no common/);
  assert.throws(() => planPlayerRendezvous(geometry, [{ x: 8200, y: 8200 }, { x: 8296, y: 8200 }]), /origin lacks/);
  assert.throws(() => planPlayerRendezvous(geometry, [{ x: NaN, y: 8200 }, { x: 8296, y: 8200 }]), /finite/);
  assert.throws(() => planPlayerRendezvous(geometry, [{ x: 8104, y: 8200 }, { x: 8296, y: 8200 }], { hullRadiusPx: 1 }), /clearance/);
});

test("shipped Chaos routes several real central spawn shapes without crossing expanded objects or gravity", () => {
  const root = path.resolve(new URL("../../assets/zones", import.meta.url).pathname);
  const loaded = loadRendezvousMap("chaos-zone", [root]);
  assert(loaded.provenance.wormholeAvoidRadiusPx > 1200);
  // Actual receipt positions retained by the 83cafc8 initial-join gate.
  for (const peer of [{ x: 6920, y: 8136 }, { x: 8392, y: 7800 }, { x: 8552, y: 6472 }]) {
    const plan = planPlayerRendezvous(loaded.geometry, [{ x: 8200, y: 8200 }, peer]);
    verify(loaded.geometry, plan);
  }
});

test("shipped Extreme staging leaves its safe central region without changing assets", () => {
  const root = path.resolve(new URL("../../assets/zones", import.meta.url).pathname);
  const loaded = loadRendezvousMap("extreme", [root]);
  const plan = planPlayerRendezvous(loaded.geometry, [{ x: 8200, y: 8200 }, { x: 8176, y: 8192 }]);
  verify(loaded.geometry, plan);
});

test("the retained Chaos staircase route avoids one stop per diagonal tile", () => {
  const root = path.resolve(new URL("../../assets/zones", import.meta.url).pathname);
  const loaded = loadRendezvousMap("chaos-zone", [root]);
  const plan = planPlayerRendezvous(loaded.geometry,
    [{ x: 6248, y: 9720 }, { x: 7896.46, y: 7437.76 }], { hullRadiusPx: 20 });
  verify(loaded.geometry, plan);
  const legs = plan.routes.reduce((sum, route) => sum + route.drivePoints.length, 0);
  assert(legs <= 20, `the old route took 57 stopped legs; got ${legs}`);
});

test("the keyboard lifecycle maze pair uses full-map detours to one open encounter", () => {
  const root = path.resolve(new URL("../../assets/zones", import.meta.url).pathname);
  const loaded = loadRendezvousMap("chaos-zone", [root]);
  // The bounded local counterfire search returns null at these actual11ae624
  // poses; both are legal floor, but a connected route leaves its search box.
  const plan = planPlayerRendezvous(loaded.geometry,
    [{ x: 6888, y: 8792 }, { x: 7318.3, y: 9348.09 }], { hullRadiusPx: 20 });
  verify(loaded.geometry, plan);
  assert(plan.routes.every(route => route.distancePx > 2000), "retain the actual long maze detours");
  assert(plan.routes.every(route => route.drivePoints.length <= 10));
});

test("dynamic-zone lookup binds exact bytes and refuses manifest traversal or stale content", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zub-rendezvous-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const directory = path.join(root, "different-directory"); fs.mkdirSync(directory);
  const map = tileRecords([[512, 512, 1]]), settings = Buffer.from("[Warbird]\nGravity=2500\n");
  const sha = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
  const manifest = `version=1\nid=custom-zone\nmap=test.lvl\nmap_sha256=${sha(map)}\nsettings=arena.conf\nsettings_sha256=${sha(settings)}\n`;
  fs.writeFileSync(path.join(directory, "zone.bundle"), manifest);
  fs.writeFileSync(path.join(directory, "test.lvl"), map); fs.writeFileSync(path.join(directory, "arena.conf"), settings);
  const loaded = loadRendezvousMap("custom-zone", [root]);
  assert.equal(loaded.provenance.mapSha256, sha(map));
  assert.equal(loaded.geometry.tileAt(8200, 8200), 1);
  assert.equal(loaded.provenance.wormholeAvoidRadiusPx, Math.sqrt(2500 * 1000) + 32);
  fs.writeFileSync(path.join(directory, "test.lvl"), Buffer.alloc(0));
  assert.throws(() => loadRendezvousMap("custom-zone", [root]), /checksum mismatch/);
  fs.writeFileSync(path.join(directory, "zone.bundle"), manifest.replace("map=test.lvl", "map=../escape.lvl"));
  assert.throws(() => loadRendezvousMap("custom-zone", [root]), /invalid map/);
  assert.throws(() => loadRendezvousMap("../bad", [root]), /invalid.*identity/);
  assert.throws(() => loadRendezvousMap("missing", [root]), /no authored/);
});

test("an authored approach keeps the fixed prize/witness destination and routes around terrain", async () => {
  const { planAuthoredApproach } = await import("./player_rendezvous.mjs");
  const rows = Array.from({ length: 41 }, (_, i) => [512, 490 + i, 1]);
  const geometry = authoredMapGeometry(tileRecords(rows));
  const destination = { x: 8360, y: 8200 };
  const plan = planAuthoredApproach(geometry, { x: 8040, y: 8200 }, destination, { arrivalRadiusPx: 0 });
  verify(geometry, plan);
  assert.deepEqual(plan.target, destination);
  assert.equal(plan.routes.length, 1);
  assert(plan.routes[0].distancePx > 700);
});

test("stationary witness approaches leave the witness untouched and refuse blocked destinations", async () => {
  const { planAuthoredApproach } = await import("./player_rendezvous.mjs");
  const geometry = authoredMapGeometry(tileRecords([[512, 512, 1]]));
  const origin = { x: 8104, y: 8200 }, destination = { x: 8360, y: 8200 };
  const copy = structuredClone(destination);
  const plan = planAuthoredApproach(geometry, origin, destination, { arrivalRadiusPx: 48 });
  verify(geometry, plan);
  assert.deepEqual(destination, copy);
  assert(Math.hypot(plan.target.x-destination.x, plan.target.y-destination.y) <= 48);
  assert.throws(() => planAuthoredApproach(geometry, origin, { x: 8200, y: 8200 }, { arrivalRadiusPx: 0 }), /no hull-clear/);
  assert.throws(() => planAuthoredApproach(geometry, origin, destination, { arrivalRadiusPx: 1000 }), /invalid/);
});

test("the retained post-death bot Repel origin has a hull-clear approach without changing its spawn", async () => {
  const { planAuthoredApproach } = await import("./player_rendezvous.mjs");
  const loaded = loadRendezvousMap("chaos-zone", [path.resolve(new URL("../../assets/zones", import.meta.url).pathname)]);
  const origin = { x: 8312, y: 10120 }, target = { x: 8243, y: 8946 };
  assert(Math.hypot(target.x - origin.x, target.y - origin.y) > 1100);
  const plan = planAuthoredApproach(loaded.geometry, origin, target, { arrivalRadiusPx: 96 });
  verify(loaded.geometry, plan);
  assert.deepEqual(plan.destination, target);
  assert(Math.hypot(plan.target.x - target.x, plan.target.y - target.y) < 96);
  assert(plan.routes[0].distancePx > 1000);
});

test("the retained off-screen bot trace can stage a visible encounter through the unchanged map", async () => {
  const { planAuthoredApproach } = await import("./player_rendezvous.mjs");
  const loaded = loadRendezvousMap("chaos-zone", [path.resolve(new URL("../../assets/zones", import.meta.url).pathname)]);
  const origin = { x: 6680, y: 7128 }, target = { x: 7199, y: 8040 };
  const plan = planAuthoredApproach(loaded.geometry, origin, target, { arrivalRadiusPx: 96 });
  verify(loaded.geometry, plan);
  assert.deepEqual(plan.destination, target);
  assert(Math.hypot(plan.target.x - target.x, plan.target.y - target.y) < 96);
  assert(plan.routes[0].distancePx > 900);
});

test("the recorded Firefox mid-tier pair reaches one open coasting area through full-hull routes", () => {
  const loaded = loadRendezvousMap("chaos-zone", [path.resolve(new URL("../../assets/zones", import.meta.url).pathname)]);
  const origins = [{ x: 7960, y: 6728 }, { x: 7752, y: 8856 }];
  assert(Math.hypot(origins[0].x - origins[1].x, origins[0].y - origins[1].y) > 2048);
  const plan = planPlayerRendezvous(loaded.geometry, origins, { hullRadiusPx: 20, stageRadiusPx: 384 });
  verify(loaded.geometry, plan);
  assert.equal(plan.stageRadiusPx, 384);
  assert(loaded.geometry.authoredHullIsTraversable(plan.target.x, plan.target.y, 384));
  assert(plan.routes.every(route => route.drivePoints.length > 0 && route.distancePx > 500),
    "both actual distant entries require genuine flight before coasting");
});

test("random Chaos entries can meet at a clear EMP stage with the original 96px standoff", () => {
  const loaded = loadRendezvousMap("chaos-zone", [path.resolve(new URL("../../assets/zones", import.meta.url).pathname)]);
  const origins = [{ x: 7960, y: 6728 }, { x: 7752, y: 8856 }];
  const plan = planPlayerRendezvous(loaded.geometry, origins, { hullRadiusPx: 20, stageRadiusPx: 192 });
  verify(loaded.geometry, plan);
  // Include both original4px arrival allowances and the actual14px ship hull
  // throughout the short lateral leg, not just a centre-line endpoint check.
  for (let offset = -4; offset <= 100; offset += 2) {
    assert(loaded.geometry.authoredHullIsTraversable(plan.target.x + offset, plan.target.y - 4, 18));
    assert(loaded.geometry.authoredHullIsTraversable(plan.target.x + offset, plan.target.y + 4, 18));
    assert.equal(loaded.geometry.tileAt(plan.target.x + offset, plan.target.y), 0);
  }
  assert(plan.routes.every(route => route.distancePx > 500));
});

test("a requested pre-claim meeting point is reached by both full-hull routes", () => {
  const geometry = authoredMapGeometry(Buffer.alloc(0));
  const origins = [{ x: 8104, y: 8200 }, { x: 8296, y: 8200 }];
  const meetingPoint = { x: 8248, y: 8312 };
  const blockedTiles = [{ x: 512, y: 515 }];
  const plan = planPlayerRendezvous(geometry, origins, { meetingPoint, blockedTiles });
  assert.deepEqual(plan.target, meetingPoint, "timed setup may not substitute an arbitrary midpoint");
  verify(geometry, plan, blockedTiles);
});

test("a blocked, non-grid or non-finite requested meeting point cannot silently fall back", () => {
  const geometry = authoredMapGeometry(tileRecords([[512, 512, 1]]));
  const origins = [{ x: 8104, y: 8200 }, { x: 8296, y: 8200 }];
  for (const meetingPoint of [{ x: 8200, y: 8200 }, { x: 8248.5, y: 8312 }, { x: NaN, y: 8312 }]) {
    assert.throws(() => planPlayerRendezvous(geometry, origins, { meetingPoint }), /meeting point|rendezvous stage/);
  }
});

test("retained keyboard coasting pose has a legal hull but needs a different 20px route origin", () => {
  const root = path.resolve(new URL("../../assets/zones", import.meta.url).pathname);
  const { geometry } = loadRendezvousMap("chaos-zone", [root]);
  const start = { x: 7784, y: 8959.97 }, stopped = { x: 7784, y: 8984 };
  assert.equal(geometry.authoredHullIsTraversable(start.x, start.y, 14), true);
  assert.equal(geometry.authoredHullIsTraversable(start.x, start.y, 20), false);
  for (let y = start.y; y <= stopped.y; y += 0.25)
    assert.equal(geometry.authoredHullIsTraversable(start.x, y, 14), true);
  assert.equal(geometry.authoredHullIsTraversable(stopped.x, stopped.y, 20), true);
  const other = { x: 7272, y: 8824 };
  assert.throws(() => planPlayerRendezvous(geometry, [start, other], { hullRadiusPx: 20 }), /origin lacks/);
  const plan = planPlayerRendezvous(geometry, [stopped, other], { hullRadiusPx: 20 });
  verify(geometry, plan);
});
