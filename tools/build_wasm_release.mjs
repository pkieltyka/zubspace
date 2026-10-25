#!/usr/bin/env node

import crypto from "node:crypto";
import { writeZoneAssetManifest } from "./zone_asset_manifest.mjs";
import { clientBuildInfo, stampClientBuildInfo, sourceRevision } from "./client_build_info.mjs";
import fs from "node:fs";
import { verifyMacroquadPairing } from "./macroquad_pairing.mjs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const distRoot = path.join(repoRoot, "dist");
verifyMacroquadPairing(repoRoot);
const releaseWasm = path.join(
  repoRoot,
  "target",
  "wasm32-unknown-unknown",
  "release",
  "zubspace.wasm",
);
const distWasm = path.join(distRoot, "zubspace.wasm");
const jsSource = path.join(repoRoot, "js");
const jsDist = path.join(distRoot, "js");

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    stdio: "inherit",
    shell: process.platform === "win32",
  });
  if (result.status !== 0) {
    process.exit(result.status || 1);
  }
}

function shortHash(filename) {
  return crypto
    .createHash("sha256")
    .update(fs.readFileSync(filename))
    .digest("hex")
    .slice(0, 16);
}

const sourceCommit = sourceRevision(repoRoot);
run("cargo", [
  "build",
  "--locked",
  "--release",
  "--target",
  "wasm32-unknown-unknown",
  "--bin",
  "zubspace",
]);

fs.rmSync(distRoot, { force: true, recursive: true });
fs.mkdirSync(distRoot, { recursive: true });
fs.copyFileSync(releaseWasm, distWasm);
writeZoneAssetManifest(repoRoot, path.join(distRoot, "zone-assets.json"));
fs.cpSync(jsSource, jsDist, { recursive: true });
// Ship the license and asset notes with the code.
for (const filename of ["LICENSE", "ASSETS.md"]) {
  fs.copyFileSync(path.join(repoRoot, filename), path.join(distRoot, filename));
}
fs.mkdirSync(path.join(distRoot, "docs"), { recursive: true });
for (const filename of ["browser-runtime-provenance.md", "chaos_zone_settings.md"]) {
  fs.copyFileSync(path.join(repoRoot, "docs", filename), path.join(distRoot, "docs", filename));
}


// [REVIEW C7] Zone assets are runtime-fetched by the browser: graphics and
// gameplay sound are embedded in the wasm, but `.lvl` maps, `.lvz` archives, and
// zone settings load over HTTP at startup (src/assets/load.rs). A bundle
// without them used to 404 in the container and the client silently fell
// back to the generated skirmish arena while the server simulated the real
// zone map — a guaranteed client/server mismatch. Copy every runtime asset
// the client can request, preserving relative paths, and fail the build if
// the canonical map is missing.
const RUNTIME_ASSET_EXTENSIONS = new Set([
  ".lvl",
  ".lvz",
  ".cfg",
  ".conf",
  ".ini",
  ".set",
]);

function walkFiles(dir, visit) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      walkFiles(full, visit);
    } else if (entry.isFile()) {
      visit(full);
    }
  }
}

let copiedRuntimeAssets = 0;
for (const root of ["assets/zones", "levels"]) {
  const sourceRoot = path.join(repoRoot, root);
  if (!fs.existsSync(sourceRoot)) {
    continue;
  }
  walkFiles(sourceRoot, (sourcePath) => {
    const ext = path.extname(sourcePath).toLowerCase();
    if (!RUNTIME_ASSET_EXTENSIONS.has(ext)) {
      return;
    }
    const relative = path.relative(repoRoot, sourcePath);
    const target = path.join(distRoot, relative);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.copyFileSync(sourcePath, target);
    copiedRuntimeAssets += 1;
  });
}
const requiredAsset = path.join(
  distRoot,
  "assets",
  "zones",
  "chaos-zone",
  "bdhadar.lvl",
);
if (!fs.existsSync(requiredAsset)) {
  console.error(
    "release bundle is missing runtime zone assets (expected assets/zones/chaos-zone/bdhadar.lvl); refusing to ship a bundle that would fall back to the skirmish arena",
  );
  process.exit(1);
}
console.log(`Copied ${copiedRuntimeAssets} runtime zone assets into dist/`);

const version = [
  sourceCommit.slice(0, 12),
  shortHash(distWasm),
  shortHash(path.join(jsDist, "zubspace_menu.js")),
  shortHash(path.join(jsDist, "zubspace_menu.css")),
  shortHash(path.join(jsDist, "zubspace_ships.png")),
  shortHash(path.join(jsDist, "zubspace_icons.png")),
  shortHash(path.join(jsDist, "mq_js_bundle.js")),
  shortHash(path.join(jsDist, "zubspace_runtime_contract.js")),
  shortHash(path.join(jsDist, "zubspace_rtc_config.js")),
  shortHash(path.join(jsDist, "zubspace_zone_catalog.js")),
  shortHash(path.join(jsDist, "zubspace_zone_loading.js")),
  shortHash(path.join(jsDist, "zubspace_zone_cache.js")),
  shortHash(path.join(distRoot, "zone-assets.json")),
  shortHash(path.join(jsDist, "zubspace_player_telemetry.js")),
  shortHash(path.join(jsDist, "zubspace_client_debug.js")),
  shortHash(path.join(jsDist, "zubspace_wasm_identity.js")),
  shortHash(path.join(jsDist, "zubspace_wasm_identity_worker.js")),
].join("-");

let index = fs.readFileSync(path.join(repoRoot, "index.html"), "utf8");
index = index
  .replace('const buildVersion = "dev";', `const buildVersion = "${version}";`)
  .replace(
    'const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";',
    'const wasmPath = "zubspace.wasm";',
  )
  .replace('const zoneManifestPath = "target/zubspace-zone-assets.json";', 'const zoneManifestPath = "zone-assets.json";')
  .replace(
    "js/zubspace_zone_catalog.js?v=dev",
    `js/zubspace_zone_catalog.js?v=${version}`,
  );

for (const asset of ["zubspace_zone_cache.js", "zubspace_zone_loading.js", "zubspace_menu.js", "zubspace_menu.css", "zubspace_player_telemetry.js", "zubspace_client_debug.js", "zubspace_runtime_contract.js"]) {
  index = index.replace(`js/${asset}?v=dev`, `js/${asset}?v=${version}`);
}
const menuBuild = clientBuildInfo(repoRoot);
if (menuBuild.commit !== sourceCommit.slice(0, 8)) throw new Error("Source revision changed during the release build");
index = stampClientBuildInfo(index, menuBuild);
fs.writeFileSync(path.join(distRoot, "index.html"), index);
fs.writeFileSync(
  path.join(distRoot, "BUILD_INFO.json"),
  `${JSON.stringify(
    {
      version,
      appVersion: menuBuild.appVersion,
      git: sourceCommit,
      wasm: "zubspace.wasm",
      javascript: fs.readdirSync(jsDist).filter(name => name.endsWith(".js"))
        .sort().map(name => `js/${name}`),
    },
    null,
    2,
  )}\n`,
);

console.log(`Built browser release in ${distRoot}`);
console.log(`Version ${version}`);
