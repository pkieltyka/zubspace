import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { repoRoot } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { authoredMapGeometry } from "./authored_map_geometry.mjs";

// Explicit scenario geometry only. Never applied to shipped settings or an
// ordinary browser lane: each caller must declare and validate its map patch.
export function withFixtureSpawnRegion(settings, { x, y, radius }) {
  if (![x, y, radius].every(Number.isInteger) || x < 1 || x > 1023 || y < 1 || y > 1023 ||
      radius < 0 || radius > 511 || x - radius < 0 || x + radius > 1023 ||
      y - radius < 0 || y + radius > 1023) throw new Error("invalid declared fixture spawn region");
  const text = Buffer.isBuffer(settings) ? settings.toString("utf8") : settings;
  const header = /^\[Spawn\][ \t]*\r?$/m.exec(text);
  if (!header) throw new Error("fixture source has no authored Spawn section");
  const start = header.index + header[0].length, next = /^\[/m.exec(text.slice(start));
  const end = next ? start + next.index : text.length;
  const keys = new Set();
  const section = text.slice(start, end).replace(/^(Team[0-3]-(X|Y|Radius))=-?\d+([ \t]*\r?)$/gm, (_line, key, field, trailing) => {
    if (keys.has(key)) throw new Error("duplicate authored fixture spawn field");
    keys.add(key);
    const value = field === "Radius" ? radius : field === "X" ? x : y;
    return `${key}=${field !== "Radius" && value >= 512 ? value - 1024 : value}${trailing}`;
  });
  if (keys.size !== 12) throw new Error("fixture must replace exactly twelve authored spawn fields");
  return Buffer.from(text.slice(0, start) + section + text.slice(end));
}

// Keep scenery and every balance field byte-identical. The special-weapon
// oracle needs an untouched stationary target beneath one particular door;
// that is declared scenario geometry, not a shipping spawn policy.
export function createSpawnRegionFixture({ zoneId, spawnRegion }) {
  if (!["chaos-zone", "extreme"].includes(zoneId)) throw new Error("unsupported fixture zone");
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-spawn-scenario-"));
  try {
    const zone = path.join(root, zoneId);
    fs.cpSync(path.join(repoRoot, "assets/zones", zoneId), zone, { recursive: true });
    const manifestPath = path.join(zone, "zone.bundle");
    const manifest = fs.readFileSync(manifestPath, "utf8");
    const settingsPath = path.join(zone, "arena.conf");
    const original = fs.readFileSync(settingsPath);
    const settings = withFixtureSpawnRegion(original, spawnRegion);
    const mapName = manifest.match(/^map=(.+)$/m)?.[1]?.trim();
    if (!mapName || path.basename(mapName) !== mapName || mapName.includes("\\")) throw new Error("invalid fixture map");
    const map = fs.readFileSync(path.join(zone, mapName));
    const geometry = authoredMapGeometry(map);
    if (!geometry.authoredHullIsTraversable(spawnRegion.x * 16 + 8, spawnRegion.y * 16 + 8,
      spawnRegion.radius * 16 + 20)) throw new Error("declared scenario spawn lacks conservative map clearance");
    const hash = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
    fs.writeFileSync(settingsPath, settings);
    fs.writeFileSync(manifestPath, manifest.replace(/^settings_sha256=.+$/m, `settings_sha256=${hash(settings)}`));
    return { root, zoneId, spawnRegion: { ...spawnRegion },
      description: "Unmodified map and balance; twelve explicit scenario Spawn fields only",
      originalSettingsSha256: hash(original), settingsSha256: hash(settings), mapSha256: hash(map) };
  } catch (error) { fs.rmSync(root, { recursive: true, force: true }); throw error; }
}
