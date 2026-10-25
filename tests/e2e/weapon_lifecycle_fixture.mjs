import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { withFixtureSpawnRegion } from "./fixture_spawn_region.mjs";
import { repoRoot } from "../../tools/browser_firefox_rtc_smoke.mjs";

const hash = bytes => crypto.createHash("sha256").update(bytes).digest("hex");

export function createWeaponLifecycleFixture({ zoneId = "chaos-zone", clearBounds = { minX: 496, maxX: 536, minY: 496, maxY: 528 }, solidRows = [], spawnRegion = null } = {}) {
  if (!["chaos-zone", "extreme"].includes(zoneId)) throw new Error("unsupported weapon fixture zone");
  if (spawnRegion) {
    const { x, y, radius } = spawnRegion;
    if (![x, y, radius].every(Number.isInteger) || x < 1 || x > 1023 || y < 1 || y > 1023 || radius < 0 || radius > 511 ||
        x - radius - 1 < clearBounds.minX || x + radius + 1 > clearBounds.maxX ||
        y - radius - 1 < clearBounds.minY || y + radius + 1 > clearBounds.maxY ||
        solidRows.some(row => row >= y - radius - 1 && row <= y + radius + 1) ||
        (x + radius + 1 >= 500 && x - radius - 1 <= 524 && y + radius + 1 >= 506 && y - radius - 1 <= 507)) {
      throw new Error("fixture spawn region must have full hull clearance inside the declared empty patch");
    }
  }
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-weapon-safety-"));
  const zone = path.join(root, zoneId);
  fs.cpSync(path.join(repoRoot, "assets/zones", zoneId), zone, { recursive: true });
  const manifestPath = path.join(zone, "zone.bundle");
  const manifest = fs.readFileSync(manifestPath, "utf8");
  const mapPath = path.join(zone, manifest.match(/^map=(.+)$/m)[1]);
  const original = fs.readFileSync(mapPath);
  const offset = original.subarray(0, 2).toString() === "BM" ? original.readUInt32LE(2) : 0;
  const records = [];
  // Remove existing records only in the declared arena patch, including any
  // special tile origins. Preserve the rest of the shipped map and its art.
  for (let cursor = offset; cursor + 4 <= original.length; cursor += 4) {
    const word = original.readUInt32LE(cursor), x = word & 4095, y = (word >>> 12) & 4095;
    if (x < clearBounds.minX || x > clearBounds.maxX || y < clearBounds.minY || y > clearBounds.maxY) records.push(original.subarray(cursor, cursor + 4));
  }
  for (let y = 506; y <= 507; y++) for (let x = 500; x <= 524; x++) {
    const record = Buffer.alloc(4);
    record.writeUInt32LE((171 * 2 ** 24 + y * 4096 + x) >>> 0);
    records.push(record);
  }
  for (const y of solidRows) for (let x = clearBounds.minX; x <= clearBounds.maxX; x++) {
    const record = Buffer.alloc(4);
    record.writeUInt32LE((2 ** 24 + y * 4096 + x) >>> 0);
    records.push(record);
  }
  const map = Buffer.concat([original.subarray(0, offset), ...records]);
  fs.writeFileSync(mapPath, map);
  const settingsPath = path.join(zone, "arena.conf");
  const originalSettings = fs.readFileSync(settingsPath);
  let settings = originalSettings;
  if (spawnRegion) {
    settings = withFixtureSpawnRegion(originalSettings, spawnRegion);
    fs.writeFileSync(settingsPath, settings);
  }
  fs.writeFileSync(manifestPath, manifest.replace(/^map_sha256=.+$/m, `map_sha256=${hash(map)}`)
    .replace(/^settings_sha256=.+$/m, `settings_sha256=${hash(settings)}`));
  return { root, description: `${zoneId === "chaos-zone" ? "Chaos" : "Extreme"} ${spawnRegion ? "map and explicit fixture spawn region" : "map only"}: clear x${clearBounds.minX}..${clearBounds.maxX}/y${clearBounds.minY}..${clearBounds.maxY}; safety x500..524/y506..507; ${solidRows.length ? `solid rows ${solidRows.join(",")}; ` : ""}unchanged weapon, damage, energy, timer and ship settings`,
    originalMapSha256: hash(original), fixtureMapSha256: hash(map),
    spawnRegion: spawnRegion ? { ...spawnRegion } : null,
    originalSettingsSha256: hash(originalSettings), settingsSha256: hash(settings) };
}

