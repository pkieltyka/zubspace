import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { verifyMacroquadPairing, forkRevision } from "../tools/macroquad_pairing.mjs";
const { verifyImports } = createRequire(import.meta.url)("../js/zubspace_runtime_contract.js");

test("the locked Rust fork, its vendored Miniquad and shipped runtime form the reviewed pair", () => {
  assert.equal(verifyMacroquadPairing(process.cwd()).revision, forkRevision);
});

test("pairing rejects a stale runtime, stale target pin, and registry Miniquad", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-runtime-pair-"));
  try {
    fs.mkdirSync(path.join(root, "js"));
    const files = ["Cargo.toml", "Cargo.lock", "js/mq_js_bundle.js", "js/mq_js_bundle.provenance.json"];
    for (const file of files) fs.copyFileSync(file, path.join(root, file));
    for (const [file, mutate] of [
      ["js/mq_js_bundle.js", (text) => `${text}\n`],
      ["Cargo.toml", (text) => text.replace(forkRevision, "0".repeat(40))],
      ["Cargo.lock", (text) => text.replace(/(name = "miniquad"[\s\S]*?source = ")[^"]+/, '$1registry+https://github.com/rust-lang/crates.io-index')],
    ]) {
      const target = path.join(root, file), original = fs.readFileSync(target, "utf8");
      fs.writeFileSync(target, mutate(original));
      assert.throws(() => verifyMacroquadPairing(root), /Macroquad pairing/);
      fs.writeFileSync(target, original);
    }
  } finally { fs.rmSync(root, { recursive: true, force: true }); }
});

// Actual WASM import section: env.fn : () -> (). No fallback functions are installed.
const importedModule = new WebAssembly.Module(Uint8Array.from([
  0,97,115,109,1,0,0,0, 1,4,1,96,0,0, 2,10,1,3,101,110,118,2,102,110,0,0,
]));
test("startup rejects missing imports before instantiation and preserves real implementations", () => {
  const imports = { env: {} };
  assert.throws(() => verifyImports(importedModule, imports), /env.fn/);
  assert.deepEqual(imports, { env: {} });
  imports.env.fn = 1;
  assert.throws(() => verifyImports(importedModule, imports), /env.fn/);
  let calls = 0;
  imports.env.fn = () => { calls++; };
  verifyImports(importedModule, imports);
  new WebAssembly.Instance(importedModule, imports);
  imports.env.fn();
  assert.equal(calls, 1);
});
