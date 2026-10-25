#!/usr/bin/env node

// [REVIEW C7] Release-bundle smoke check: serve `dist/` with the same
// static server used in development and assert every runtime-critical
// file answers HTTP 200 — index, wasm, the JS bundle, and a known zone
// map (the asset class that used to be missing entirely and silently
// degraded the client to the skirmish arena). Run via
// `make check-release-bundle`.

import fs from "node:fs";
import { verifyMacroquadPairing } from "./macroquad_pairing.mjs";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const distRoot = path.join(repoRoot, "dist");
verifyMacroquadPairing(repoRoot);
const port = Number(process.env.ZUBSPACE_BUNDLE_CHECK_PORT || "7177");

if (!fs.existsSync(path.join(distRoot, "index.html"))) {
  console.error("dist/ has no index.html — run `node tools/build_wasm_release.mjs` first");
  process.exit(1);
}

verifyMacroquadPairing(repoRoot, distRoot);

const releaseIndex = fs.readFileSync(path.join(distRoot, "index.html"), "utf8");
if (!releaseIndex.includes('const defaultRtcOfferPath = "/v1/rtc/offer";')) {
  console.error(
    "dist/index.html does not default signaling to the bundled same-origin /v1/rtc/offer proxy",
  );
  process.exit(1);
}

const buildInfo = JSON.parse(fs.readFileSync(path.join(distRoot, "BUILD_INFO.json"), "utf8"));
const shippedJavascript = fs.readdirSync(path.join(distRoot, "js"))
  .filter(name => name.endsWith(".js")).sort().map(name => `js/${name}`);
if (JSON.stringify(buildInfo.javascript) !== JSON.stringify(shippedJavascript)) {
  throw new Error("BUILD_INFO.json JavaScript inventory does not match the shipped files");
}
if (!releaseIndex.includes(`id="menu-build">v${buildInfo.appVersion} · ${buildInfo.git.slice(0, 8)}`)) {
  throw new Error("release menu version/commit does not match BUILD_INFO.json");
}

const required = [
  "/index.html",
  "/LICENSE",
  "/ASSETS.md",
  "/docs/browser-runtime-provenance.md",
  "/zubspace.wasm",
  "/js/mq_js_bundle.js",
  "/js/mq_js_bundle.provenance.json",
  "/js/zubspace_runtime_contract.js",
  "/js/zubspace_rtc_config.js",
  "/js/zubspace_zone_catalog.js",
  "/js/zubspace_zone_loading.js",
  "/js/zubspace_zone_cache.js",
  "/zone-assets.json",
  "/js/zubspace_menu.js",
  "/js/zubspace_player_telemetry.js",
  "/js/zubspace_client_debug.js",
  "/js/zubspace_menu.css",
  "/js/zubspace_ships.png",
  "/js/zubspace_icons.png",
  "/js/zubspace_share.png",
  "/assets/zones/chaos-zone/bdhadar.lvl",
];

const server = spawn(
  process.execPath,
  [path.join(repoRoot, "tools", "dev_static_server.mjs"), String(port), distRoot],
  { stdio: ["ignore", "pipe", "pipe"] },
);

function stopServer() {
  if (!server.killed) {
    server.kill("SIGTERM");
  }
}

async function waitForServer() {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/index.html`);
      if (response.ok) {
        return;
      }
    } catch {
      // not listening yet
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`static server did not start on port ${port}`);
}

try {
  await waitForServer();
  const failures = [];
  for (const route of required) {
    const response = await fetch(`http://127.0.0.1:${port}${route}`);
    if (response.status !== 200) {
      failures.push(`${route} -> HTTP ${response.status}`);
      continue;
    }
    const bytes = (await response.arrayBuffer()).byteLength;
    if (bytes === 0) {
      failures.push(`${route} -> empty body`);
      continue;
    }
    console.log(`ok: ${route} (${bytes} bytes)`);
  }
  if (failures.length > 0) {
    console.error(`release bundle check FAILED:\n  ${failures.join("\n  ")}`);
    process.exitCode = 1;
  } else {
    console.log("release bundle check passed");
  }
} catch (error) {
  console.error(`release bundle check errored: ${error?.message || error}`);
  process.exitCode = 1;
} finally {
  stopServer();
}
