import fs from "node:fs";
import crypto from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

const extensions = new Set([".lvl", ".lvz", ".cfg", ".conf", ".ini", ".set"]);
export function zoneAssetManifest(root) {
  const zones = {};
  const base = path.join(root, "assets/zones");
  function visit(dir, files) {
    for (const item of fs.readdirSync(dir, { withFileTypes: true })) {
      const filename = path.join(dir, item.name);
      if (item.isDirectory()) visit(filename, files);
      else if (item.isFile() && extensions.has(path.extname(filename).toLowerCase())) {
        files.push(path.relative(root, filename).split(path.sep).join("/"));
      }
    }
  }
  for (const item of fs.readdirSync(base, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (!item.isDirectory() || !/^[a-z0-9](?:[a-z0-9-]{0,13}[a-z0-9])?$/.test(item.name)) continue;
    const files = [];
    visit(path.join(base, item.name), files);
    const priority = filename => filename.endsWith(".lvl") ? 0 : filename.endsWith(".lvz") ? 2 : 1;
    files.sort((a, b) => priority(a) - priority(b) || (a < b ? -1 : a > b ? 1 : 0));
    zones[item.name] = files;
  }
  const sha256 = Object.fromEntries(Object.values(zones).flat().map(filename =>
    [filename, crypto.createHash("sha256").update(fs.readFileSync(path.join(root, filename))).digest("hex")]));
  return { version: 2, zones, sha256 };
}
export function writeZoneAssetManifest(root, output) {
  const bytes = `${JSON.stringify(zoneAssetManifest(root))}\n`;
  fs.mkdirSync(path.dirname(output), { recursive: true });
  // Atomic replacement allows debug and release builds to finish concurrently.
  const temporary = `${output}.${process.pid}.tmp`;
  fs.writeFileSync(temporary, bytes);
  fs.renameSync(temporary, output);
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  writeZoneAssetManifest(root, path.join(root, "target/zubspace-zone-assets.json"));
}
