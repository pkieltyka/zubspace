import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { repoRoot } from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  CHAOS_WORMHOLE_AVOID_RADIUS_PX,
  CHAOS_WORMHOLE_FIELD_AVOID_PX,
  CHAOS_WORMHOLE_GRAVITY,
} from "./chaos_safe_route.mjs";

const TILE_SIZE = 16;
const SPAWN_TILE = 512;
const WORMHOLE_ANCHOR_X = 600;
const WORMHOLE_ANCHOR_Y = 512;
const WORMHOLE_TILE_ID = 220;
const CHAOS_ROUTE_WORMHOLE_ANCHOR = { x: 596, y: 506 };
const CHAOS_ROUTE_SPAWN = { x: 8200, y: 8200 };
const CHAOS_ROUTE_REPLAN_ARENA = "chaos-replan";

// The pilot's arena has to carry the archive volume the frame-cost questions are about, so the
// client's render cache holds what a real Chaos join builds instead of the fixture's own single
// object. `make bench-lvz-merge` measures this shipped set at `merge_ms=82 slowest_merge_ms=60`
// across 14 archives and 28,922 objects, and the client's own counters now report what the same
// set costs in a live browser. They are attached as extra `lvz=` manifest entries only: the map
// stays tile-only, so the spawn position and every existing authority, energy, and position
// assertion are unaffected. Note this widens the arena the pilot already lives in - the wormhole
// terminal relocates within one arena, so no archive installs during the relocation itself.
const CHAOS_ARCHIVE_SOURCE = "assets/zones/chaos-zone";

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

function createWormholeE2eZone() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-wormhole-zone-"));
  const arena = "wormhole-e2e";
  const directory = path.join(root, arena);
  fs.mkdirSync(directory, { recursive: true });

  // A valid tile-only LVL: the empty arena selects the ordinary neutral
  // center spawn and this one 5x5 animated tile begins 1,400 px to its right,
  // beyond the strongest authored 1,000 px gravity radius during bootstrap.
  // Both the browser and server expand the same anchor record.
  const packed = (
    WORMHOLE_ANCHOR_X |
    (WORMHOLE_ANCHOR_Y << 12) |
    (WORMHOLE_TILE_ID << 24)
  ) >>> 0;
  const map = Buffer.alloc(4);
  map.writeUInt32LE(packed, 0);
  // Dynamic zone settings must be the complete portable Continuum profile;
  // the bundled Chaos file is intentionally completed by its embedded-only
  // compatibility builder and cannot be renamed into a dynamic identity.
  const settings = fs.readFileSync(
    path.join(repoRoot, "assets/zones/extreme/arena.conf"),
  );
  const news = Buffer.from("Deterministic browser wormhole authority fixture\n");
  const chaosArchives = fs
    .readdirSync(path.join(repoRoot, CHAOS_ARCHIVE_SOURCE))
    .filter((name) => name.endsWith(".lvz"))
    .sort()
    .map((name) => {
      const bytes = fs.readFileSync(path.join(repoRoot, CHAOS_ARCHIVE_SOURCE, name));
      fs.writeFileSync(path.join(directory, name), bytes);
      return { name, entries: [`lvz=${name},${sha256(bytes)}`], bytes: bytes.length };
    });
  const mapFilename = "wormhole.lvl";
  fs.writeFileSync(path.join(directory, mapFilename), map);
  fs.writeFileSync(path.join(directory, "arena.conf"), settings);
  fs.writeFileSync(path.join(directory, "news.txt"), news);
  const manifest = Buffer.from([
    "version=1",
    `id=${arena}`,
    "display_name=Wormhole E2E",
    `map=${mapFilename}`,
    `map_sha256=${sha256(map)}`,
    "settings=arena.conf",
    `settings_sha256=${sha256(settings)}`,
    "news=news.txt",
    `news_sha256=${sha256(news)}`,
    ...chaosArchives.flatMap((archive) => archive.entries),
    "",
  ].join("\n"));
  fs.writeFileSync(path.join(directory, "zone.bundle"), manifest);

  return {
    arena,
    root,
    spawn: {
      x: (SPAWN_TILE + 0.5) * TILE_SIZE,
      y: (SPAWN_TILE + 0.5) * TILE_SIZE,
    },
    target: {
      x: (WORMHOLE_ANCHOR_X + 0.5) * TILE_SIZE,
      y: (WORMHOLE_ANCHOR_Y + 0.5) * TILE_SIZE,
    },
    evidence: {
      arena,
      mapFilename,
      mapSha256: sha256(map),
      settingsSha256: sha256(settings),
      newsSha256: sha256(news),
      manifestSha256: sha256(manifest),
      spawnTile: SPAWN_TILE,
      wormholeAnchor: [WORMHOLE_ANCHOR_X, WORMHOLE_ANCHOR_Y],
      chaosArchiveCount: chaosArchives.length,
      chaosArchiveBytes: chaosArchives.reduce((total, archive) => total + archive.bytes, 0),
    },
    cleanup() {
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

function createChaosRouteReplanZone() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-chaos-route-zone-"));
  const arena = CHAOS_ROUTE_REPLAN_ARENA;
  const directory = path.join(root, arena);
  fs.mkdirSync(directory, { recursive: true });

  // Keep the shipped Chaos tile grid and its authored safe region. Add one test-only wormhole on the known
  // eastbound safe-route corridor; the browser first plans against the unmodified Chaos grid, then the real
  // server relocates it to the canonical center spawn. The replacement route uses the modified grid and avoids
  // the new portal. Extreme settings are the complete portable profile required by dynamic zone bundles; adjust
  // their wormhole gravity and top speed to Chaos' shipped values so the fixture exercises its real field radius.
  const originalMap = fs.readFileSync(path.join(repoRoot, CHAOS_ARCHIVE_SOURCE, "bdhadar.lvl"));
  const portalRecord = Buffer.alloc(4);
  portalRecord.writeUInt32LE((CHAOS_ROUTE_WORMHOLE_ANCHOR.x |
    (CHAOS_ROUTE_WORMHOLE_ANCHOR.y << 12) | (WORMHOLE_TILE_ID << 24)) >>> 0);
  const map = Buffer.concat([originalMap, portalRecord]);
  const extremeSettings = fs.readFileSync(path.join(repoRoot, "assets/zones/extreme/arena.conf"), "utf8");
  const gravityOverrides = (extremeSettings.match(/^Gravity=1000$/gm) || []).length;
  const gravityTopSpeedOverrides = (extremeSettings.match(/^GravityTopSpeed=250$/gm) || []).length;
  if (gravityOverrides === 0 || gravityTopSpeedOverrides === 0) {
    throw new Error("portable Extreme settings no longer expose the gravity fields needed by the Chaos fixture");
  }
  let spawnOverrideCount = 0;
  const settings = Buffer.from(extremeSettings
    .replace(/^Gravity=1000$/gm, `Gravity=${CHAOS_WORMHOLE_GRAVITY}`)
    .replace(/^GravityTopSpeed=250$/gm, "GravityTopSpeed=100")
    // This route-replan fixture requires its existing exact origin/landing.
    // Declare that private spatial precondition rather than inherit the
    // shipping profile's random radius. Signed-10-bit -512 means tile512.
    .replace(/^Team([0-3])-(X|Y|Radius)=-?\d+$/gm, (_line, team, field) => {
      spawnOverrideCount++;
      return `Team${team}-${field}=${field === "Radius" ? 0 : -512}`;
    }));
  if (spawnOverrideCount !== 12) throw new Error("Chaos route fixture requires all twelve explicit spawn fields");
  const news = Buffer.from("Chaos-derived safe-route relocation fixture\n");
  const chaosArchives = fs
    .readdirSync(path.join(repoRoot, CHAOS_ARCHIVE_SOURCE))
    .filter((name) => name.endsWith(".lvz"))
    .sort()
    .map((name) => {
      const bytes = fs.readFileSync(path.join(repoRoot, CHAOS_ARCHIVE_SOURCE, name));
      fs.writeFileSync(path.join(directory, name), bytes);
      return { name, entries: [`lvz=${name},${sha256(bytes)}`], bytes: bytes.length };
    });
  const mapFilename = "chaos-rp.lvl";
  fs.writeFileSync(path.join(directory, mapFilename), map);
  fs.writeFileSync(path.join(directory, "arena.conf"), settings);
  fs.writeFileSync(path.join(directory, "news.txt"), news);
  const manifest = Buffer.from([
    "version=1",
    `id=${arena}`,
    "display_name=Chaos Safe Route Replan E2E",
    `map=${mapFilename}`,
    `map_sha256=${sha256(map)}`,
    "settings=arena.conf",
    `settings_sha256=${sha256(settings)}`,
    "news=news.txt",
    `news_sha256=${sha256(news)}`,
    ...chaosArchives.flatMap((archive) => archive.entries),
    "",
  ].join("\n"));
  fs.writeFileSync(path.join(directory, "zone.bundle"), manifest);

  return {
    kind: "chaosRouteReplan",
    arena,
    root,
    originalMap,
    map,
    settings,
    spawn: CHAOS_ROUTE_SPAWN,
    wormholeAnchor: CHAOS_ROUTE_WORMHOLE_ANCHOR,
    wormholeTarget: {
      x: (CHAOS_ROUTE_WORMHOLE_ANCHOR.x + 2.5) * 16,
      y: (CHAOS_ROUTE_WORMHOLE_ANCHOR.y + 2.5) * 16,
    },
    wormholeGravityRadiusPx: CHAOS_WORMHOLE_AVOID_RADIUS_PX,
    wormholeAvoidRadiusPx: CHAOS_WORMHOLE_FIELD_AVOID_PX,
    evidence: {
      arena,
      mapFilename,
      sourceMap: "assets/zones/chaos-zone/bdhadar.lvl",
      sourceMapSha256: sha256(originalMap),
      mapSha256: sha256(map),
      settingsSha256: sha256(settings),
      newsSha256: sha256(news),
      manifestSha256: sha256(manifest),
      spawn: CHAOS_ROUTE_SPAWN,
      wormholeAnchor: CHAOS_ROUTE_WORMHOLE_ANCHOR,
      settingsProfile: "portable Extreme with Chaos wormhole gravity/top speed and declared canonical spawn",
      declaredSpawnRegion: { tileX: 512, tileY: 512, radius: 0 },
      overriddenSpawnSettingCount: spawnOverrideCount,
      wormholeGravity: CHAOS_WORMHOLE_GRAVITY,
      wormholeGravityRadiusPx: CHAOS_WORMHOLE_AVOID_RADIUS_PX,
      wormholeAvoidRadiusPx: CHAOS_WORMHOLE_FIELD_AVOID_PX,
      overriddenGravitySettingCount: gravityOverrides,
      overriddenGravityTopSpeedSettingCount: gravityTopSpeedOverrides,
      chaosArchiveCount: chaosArchives.length,
      chaosArchiveBytes: chaosArchives.reduce((total, archive) => total + archive.bytes, 0),
    },
    cleanup() {
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

export { createChaosRouteReplanZone, createWormholeE2eZone };
