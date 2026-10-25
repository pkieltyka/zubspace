#!/usr/bin/env node
import fs from "node:fs";
import { verifyMacroquadPairing } from "./macroquad_pairing.mjs";
import path from "node:path";
import crypto from "node:crypto";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { sourceRevision, clientBuildInfo } from "./client_build_info.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const revision = sourceRevision(root, { ...process.env, ZUBSPACE_REQUIRE_BUILD_REVISION: "1" });
const build = clientBuildInfo(root);
verifyMacroquadPairing(root);
const sha = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
function docker(args) {
  const result = spawnSync("docker", args, { encoding: "utf8", maxBuffer: 8 * 1024 * 1024, timeout: 60_000 });
  if (result.status !== 0 || result.error) throw new Error(`docker ${args.slice(0, 2).join(" ")} failed: ${result.error || result.stderr}`);
  return result.stdout;
}
const files = ["LICENSE", "ASSETS.md", "docs/browser-runtime-provenance.md", "docs/chaos_zone_settings.md"];
const expected = new Map(files.map(file => [file, sha(fs.readFileSync(path.join(root, file)))]));
const results = [];
for (const [image, prefix] of [["zubspace/server:dev", "/app"], ["zubspace/client:dev", "/usr/share/nginx/html"]]) {
  const [inspection] = JSON.parse(docker(["image", "inspect", image]));
  if (inspection.Config?.Labels?.["org.opencontainers.image.revision"] !== revision)
    throw new Error(`${image} source-revision label does not match ${revision}`);
  // These containers only read files: no game process or network listeners.
  const output = docker(["run", "--rm", "--network", "none", "--entrypoint", "sha256sum", image,
    ...files.map(file => `${prefix}/${file}`)]);
  const actual = new Map(output.trim().split("\n").map(line => {
    const match = /^([0-9a-f]{64})  (.+)$/.exec(line);
    if (!match) throw new Error(`invalid image checksum output: ${image}`);
    return [match[2], match[1]];
  }));
  for (const [file, checksum] of expected)
    if (actual.get(`${prefix}/${file}`) !== checksum) throw new Error(`${image} missing or changed file: ${file}`);
  if (image === "zubspace/client:dev") {
    for (const file of ["js/mq_js_bundle.js", "js/mq_js_bundle.provenance.json", "js/zubspace_runtime_contract.js"]) {
      const output = docker(["run", "--rm", "--network", "none", "--entrypoint", "sha256sum", image, `${prefix}/${file}`]);
      if (output.split(" ")[0] !== sha(fs.readFileSync(path.join(root, file))))
        throw new Error(`client image has a stale runtime: ${file}`);
    }
    const info = JSON.parse(docker(["run", "--rm", "--network", "none", "--entrypoint", "cat", image, `${prefix}/BUILD_INFO.json`]));
    const html = docker(["run", "--rm", "--network", "none", "--entrypoint", "cat", image, `${prefix}/index.html`]);
    if (info.git !== revision || info.appVersion !== build.appVersion ||
        !html.includes(`id="menu-build">v${build.appVersion} · ${revision.slice(0, 8)}`))
      throw new Error("client image BUILD_INFO/footer do not match its source revision");
  }
  results.push({ image, imageId: inspection.Id, sourceRevision: revision, filesVerified: expected.size });
}
console.log(JSON.stringify({ ok: true, images: results }, null, 2));
