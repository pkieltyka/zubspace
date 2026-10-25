import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";

export const forkRevision = "25b88535a93d0422c7b9ee9a86dba4c026541612";
export const forkRepository = "https://github.com/pkieltyka/macroquad";
export const runtimeSha256 = "3a262ff4f74cc051b49d4b487ea136154fbd5c8e71a8053ff33785bc7de74d3e";
export function verifyMacroquadPairing(root, assetRoot = root) {
  const read = (base, name) => fs.readFileSync(path.join(base, name), "utf8");
  const lock = read(root, "Cargo.lock");
  const packages = lock.split("[[package]]").slice(1).map((block) =>
    Object.fromEntries([...block.matchAll(/^(name|version|source) = "([^"]+)"/gm)].map((match) => [match[1], match[2]])));
  const source = `git+${forkRepository}?rev=${forkRevision}#${forkRevision}`;
  for (const [name, version, expectedSource] of [
    ["macroquad", "0.5.1", source], ["macroquad_macro", "0.1.8", source],
    ["miniquad", "0.4.11", source], ["quad-snd", "0.2.8", "registry+https://github.com/rust-lang/crates.io-index"],
  ]) {
    const matches = packages.filter((pkg) => pkg.name === name);
    if (matches.length !== 1 || matches[0].version !== version || matches[0].source !== expectedSource)
      throw new Error(`Macroquad pairing: ${name} must resolve once to ${version} from ${expectedSource}`);
  }
  const manifest = read(root, "Cargo.toml");
  const pins = [...manifest.matchAll(/^macroquad = \{([^\n]+)\}/gm)];
  if (pins.length !== 2 || pins.some((pin) => !pin[1].includes(`git = "${forkRepository}"`) || !pin[1].includes(`rev = "${forkRevision}"`)))
    throw new Error("Macroquad pairing: both target dependencies must pin the reviewed revision");
  const bundle = fs.readFileSync(path.join(assetRoot, "js/mq_js_bundle.js"));
  const metadata = JSON.parse(read(assetRoot, "js/mq_js_bundle.provenance.json"));
  if (bundle.length !== 43504 || crypto.createHash("sha256").update(bundle).digest("hex") !== runtimeSha256 || metadata.bundleSha256 !== runtimeSha256
      || metadata.pairing.repository !== forkRepository || metadata.pairing.tag !== "zubspace-v0.5.1"
      || metadata.abi.webglSelection !== 2 || metadata.abi.miniquad !== 2)
    throw new Error("Macroquad pairing: JavaScript runtime/provenance differs from zubspace-v0.5.1");
  return { revision: forkRevision, runtimeSha256, bytes: bundle.length };
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log(JSON.stringify(verifyMacroquadPairing(path.resolve(path.dirname(fileURLToPath(import.meta.url)), ".."))));
}
