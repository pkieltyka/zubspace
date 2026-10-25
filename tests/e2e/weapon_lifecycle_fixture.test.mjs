import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { repoRoot } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { createWeaponLifecycleFixture } from "./weapon_lifecycle_fixture.mjs";

const sha = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
const spawnFields = text => text.replace(/^Team[0-3]-(?:X|Y|Radius)=.*$/gm, "<declared spawn field>");

test("the all-eight weapon lane declares the canonical origin required by its original EMP stage", t => {
  const source = fs.readFileSync(new URL("./ship_weapon_traits.mjs", import.meta.url), "utf8");
  const start = source.indexOf("  const fixture = createWeaponLifecycleFixture(");
  const end = source.indexOf("\n  const port =", start);
  assert(start > 0 && end > start);
  const fixture = vm.runInNewContext(`(() => { ${source.slice(start, end)} return fixture; })()`,
    { createWeaponLifecycleFixture });
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  assert.deepEqual(fixture.spawnRegion, { x: 512, y: 512, radius: 0 });
  const former = createWeaponLifecycleFixture({ spawnRegion: { x: 512, y: 512, radius: 2 },
    clearBounds: { minX: 488, maxX: 544, minY: 488, maxY: 544 }, solidRows: [505, 523] });
  t.after(() => fs.rmSync(former.root, { recursive: true, force: true }));
  assert.equal(fixture.fixtureMapSha256, former.fixtureMapSha256);
  const settings = fs.readFileSync(path.join(fixture.root, "chaos-zone/arena.conf"), "utf8");
  const original = fs.readFileSync(path.join(repoRoot, "assets/zones/chaos-zone/arena.conf"), "utf8");
  assert.equal(spawnFields(settings), spawnFields(original));
  for (let team = 0; team < 4; team++) {
    assert.match(settings, new RegExp(`^Team${team}-X=-512$`, "m"));
    assert.match(settings, new RegExp(`^Team${team}-Y=-512$`, "m"));
    assert.match(settings, new RegExp(`^Team${team}-Radius=0$`, "m"));
  }
  // The existing driver allows3px arrival error. Even its nearest corner
  // remains outside the unchanged72px prerequisite from the declared origin.
  assert(Math.hypot(8252 - 3 - 8200, 8136 + 3 - 8200) > 72);
  assert(Math.hypot(8252.31 - 8216, 8136.32 - 8168) < 72,
    "the retained random origin genuinely violates the same prerequisite");
});

test("map-only fixtures preserve both shipped settings profiles byte for byte", t => {
  for (const zoneId of ["chaos-zone", "extreme"]) {
    const fixture = createWeaponLifecycleFixture({ zoneId });
    t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
    const original = fs.readFileSync(path.join(repoRoot, "assets/zones", zoneId, "arena.conf"));
    assert.deepEqual(fs.readFileSync(path.join(fixture.root, zoneId, "arena.conf")), original);
    assert.equal(fixture.spawnRegion, null);
    assert.equal(fixture.settingsSha256, sha(original));
  }
});

test("explicit fixture regions change only twelve Spawn fields and bind the resulting manifest", t => {
  for (const zoneId of ["chaos-zone", "extreme"]) {
    const spawnRegion = { x: 512, y: 512, radius: 2 };
    const fixture = createWeaponLifecycleFixture({ zoneId, spawnRegion });
    t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
    const directory = path.join(fixture.root, zoneId);
    const original = fs.readFileSync(path.join(repoRoot, "assets/zones", zoneId, "arena.conf"), "utf8");
    const settings = fs.readFileSync(path.join(directory, "arena.conf"), "utf8");
    assert.equal(spawnFields(settings), spawnFields(original));
    for (let i = 0; i < 4; i++) {
      assert(settings.includes(`Team${i}-X=-512\n`)); assert(settings.includes(`Team${i}-Y=-512\n`));
      assert(settings.includes(`Team${i}-Radius=2\n`));
    }
    assert.deepEqual(fixture.spawnRegion, spawnRegion);
    const manifest = fs.readFileSync(path.join(directory, "zone.bundle"), "utf8");
    assert.equal(manifest.match(/^settings_sha256=(.+)$/m)[1], sha(settings));
    assert.equal(manifest.match(/^map_sha256=(.+)$/m)[1], fixture.fixtureMapSha256);
    assert.equal(fixture.originalSettingsSha256, sha(original));
    assert.equal(fixture.settingsSha256, sha(settings));
  }
});

test("fixture regions reject unsafe, noninteger and out-of-patch spawn envelopes", () => {
  for (const spawnRegion of [{ x: 512, y: 507, radius: 0 }, { x: 512, y: 512, radius: 6 },
    { x: 496, y: 512, radius: 0 }, { x: 512, y: 512, radius: -1 },
    { x: 512.5, y: 512, radius: 0 }, { x: 512, y: 512, radius: 512 }]) {
    assert.throws(() => createWeaponLifecycleFixture({ spawnRegion }), /spawn region.*clearance/);
  }
  assert.throws(() => createWeaponLifecycleFixture({ solidRows: [512], spawnRegion: { x: 512, y: 512, radius: 0 } }), /spawn region.*clearance/);
});

test("declared custom fixture spawns preserve CRLF and all non-Spawn bytes", async () => {
  const { withFixtureSpawnRegion } = await import("./fixture_spawn_region.mjs");
  for (const zoneId of ["chaos-zone", "extreme"]) {
    const original = fs.readFileSync(path.join(repoRoot, "assets/zones", zoneId, "arena.conf"), "utf8").replace(/\r?\n/g, "\r\n");
    const modified = withFixtureSpawnRegion(original, { x: 512, y: 513, radius: 0 }).toString("utf8");
    assert.equal(spawnFields(modified), spawnFields(original));
    assert.equal((modified.match(/\r\n/g) || []).length, (original.match(/\r\n/g) || []).length);
    for (let i = 0; i < 4; i++) {
      assert(modified.includes(`Team${i}-X=-512\r\n`));
      assert(modified.includes(`Team${i}-Y=-511\r\n`));
      assert(modified.includes(`Team${i}-Radius=0\r\n`));
    }
  }
});

test("custom fixture region rewriting rejects ambiguous or incomplete authority settings", async () => {
  const { withFixtureSpawnRegion } = await import("./fixture_spawn_region.mjs");
  const original = fs.readFileSync(path.join(repoRoot, "assets/zones/chaos-zone/arena.conf"), "utf8");
  const region = { x: 512, y: 512, radius: 0 };
  assert.throws(() => withFixtureSpawnRegion(original.replace(/^Team3-Radius=.*\n/m, ""), region), /exactly twelve/);
  assert.throws(() => withFixtureSpawnRegion(original.replace(/^Team3-Radius=.*$/m, "Team0-X=0"), region), /duplicate/);
  assert.throws(() => withFixtureSpawnRegion(original.replace("[Spawn]", "[Other]"), region), /no authored Spawn/);
  for (const bad of [{ x: NaN, y: 512, radius: 0 }, { x: 10, y: 512, radius: 11 },
    { x: 1023, y: 512, radius: 2 }, { x: 512, y: 512, radius: -1 }]) {
    assert.throws(() => withFixtureSpawnRegion(original, bad), /invalid declared/);
  }
});

test("the stationary door oracle declares only Spawn fields and preserves the actual Chaos map", async t => {
  const { createSpawnRegionFixture } = await import("./fixture_spawn_region.mjs");
  const fixture = createSpawnRegionFixture({ zoneId: "chaos-zone", spawnRegion: { x: 512, y: 512, radius: 0 } });
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  const source = path.join(repoRoot, "assets/zones/chaos-zone");
  const copy = path.join(fixture.root, "chaos-zone");
  const original = fs.readFileSync(path.join(source, "arena.conf"), "utf8");
  const settings = fs.readFileSync(path.join(copy, "arena.conf"), "utf8");
  assert.equal(spawnFields(settings), spawnFields(original));
  for (const name of fs.readdirSync(source)) {
    if (["arena.conf", "zone.bundle"].includes(name)) continue;
    assert.deepEqual(fs.readFileSync(path.join(copy, name)), fs.readFileSync(path.join(source, name)));
  }
  assert.equal(fs.readFileSync(path.join(copy, "zone.bundle"), "utf8"),
    fs.readFileSync(path.join(source, "zone.bundle"), "utf8").replace(/^settings_sha256=.+$/m, `settings_sha256=${sha(settings)}`));
  assert.equal(fixture.settingsSha256, sha(settings));
  assert.equal(fixture.originalSettingsSha256, sha(original));
  assert.throws(() => createSpawnRegionFixture({ zoneId: "chaos-zone", spawnRegion: { x: 512, y: 512, radius: 511 } }), /clearance/);
});
