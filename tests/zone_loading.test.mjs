import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import { zoneAssetManifest } from "../tools/zone_asset_manifest.mjs";
await import("../js/zubspace_zone_loading.js");
const { create, readBounded } = globalThis.ZubspaceZoneLoading;
const tick = () => new Promise(resolve => setImmediate(resolve));
const data = value => new Response(value);
const manifestResponse = zones => data(JSON.stringify({ version: 1, zones }));

test("generated manifest covers shipped files and prioritizes maps then settings", () => {
  const manifest = zoneAssetManifest(process.cwd());
  for (const [zone, paths] of Object.entries(manifest.zones)) {
    assert.ok(paths.length > 0);
    assert.equal(new Set(paths).size, paths.length);
    for (const name of paths) {
      assert.ok(name.startsWith(`assets/zones/${zone}/`));
      assert.ok(fs.statSync(name).isFile());
    }
    const lastMap = paths.findLastIndex(p => p.endsWith(".lvl"));
    const firstArchive = paths.findIndex(p => p.endsWith(".lvz"));
    assert.ok(lastMap >= 0 && firstArchive > lastMap);
    assert.ok(paths.slice(0, firstArchive).some(p => p.endsWith("arena.conf")));
  }
  assert.ok(manifest.zones["chaos-zone"].every(p => !p.includes("/extreme/")));
});

test("selected zone requests overlap with six slots and never fetch another zone", async () => {
  const paths = Array.from({ length: 15 }, (_, i) => `assets/zones/chaos-zone/file${i}.lvz`);
  const started = [];
  const pending = [];
  const loader = create({ fetch: async url => {
    if (url === "manifest") return manifestResponse({ "chaos-zone": paths, extreme: ["assets/zones/extreme/other.lvl"] });
    started.push(url);
    return new Promise(resolve => pending.push(() => resolve(data(new Uint8Array([1, 2])))));
  } });
  const ready = loader.prepare("chaos-zone", "manifest");
  await tick();
  assert.equal(started.length, 6);
  assert.equal(loader.stats.active, 6);
  while (loader.stats.completed < paths.length) {
    pending.splice(0).forEach(resolve => resolve());
    await tick();
  }
  assert.equal((await ready).length, paths.length);
  assert.equal(loader.stats.peak, 6);
  assert.deepEqual(started, paths);
  await loader.prepare("chaos-zone", "manifest");
  assert.equal(started.length, 15, "menu and repeated consumers share requests");
});

test("WASM filesystem consumes the in-flight HTTP bytes once with exact callbacks", async () => {
  const path = "assets/zones/chaos-zone/a#b.lvz";
  let calls = 0;
  let release;
  const loader = create({ fetch: async url => {
    if (url === "manifest") return manifestResponse({ "chaos-zone": [path] });
    assert.equal(url, path.replace("#", "%23")); calls++;
    return new Promise(resolve => { release = () => resolve(data(new Uint8Array([9, 8, 7]))); });
  } });
  const prepared = loader.prepare("chaos-zone", "manifest");
  await tick();
  const memory = { buffer: new ArrayBuffer(1024) };
  const encoded = new TextEncoder().encode(path.replace("#", "%23"));
  new Uint8Array(memory.buffer).set(encoded);
  const files = { unique_id: 42, loaded_files: {} };
  const loaded = [];
  const env = { fs_load_file: () => { throw new Error("unexpected duplicate XHR"); } };
  loader.installFileLoader(env, files, () => ({ file_loaded: id => loaded.push(id) }), () => memory);
  assert.equal(env.fs_load_file(0, encoded.length), 42);
  assert.deepEqual(loaded, []);
  release(); await prepared; await tick();
  assert.deepEqual(loaded, [42]);
  assert.deepEqual(Array.from(files.loaded_files[42]), [9, 8, 7]);
  assert.equal(calls, 1);
});

test("unknown zones fetch no shipped files and rejected prefetch reports a normal file failure", async () => {
  const calls = [];
  const loader = create({ fetch: async url => {
    calls.push(url);
    if (url === "manifest") return manifestResponse({ "chaos-zone": [] });
    return new Response("missing", { status: 404 });
  } });
  await loader.prepare("disk-only", "manifest");
  assert.deepEqual(calls, ["manifest"]);
  await loader.prepare("chaos-zone", "manifest");
  const encoded = new TextEncoder().encode("assets/zones/chaos-zone/missing.lvz");
  const memory = { buffer: encoded.buffer };
  const files = { unique_id: 1, loaded_files: {} };
  const loaded = [];
  const env = { fs_load_file: () => 99 };
  loader.installFileLoader(env, files, () => ({ file_loaded: id => loaded.push(id) }), () => memory);
  assert.equal(env.fs_load_file(0, encoded.length), 1);
  await tick(); await tick();
  assert.deepEqual(loaded, [1]); assert.equal(files.loaded_files[1], null);
});

test("WASM warming supplies the runtime and diagnostics without another download or a lingering fetch override", async () => {
  const calls = [];
  const host = { fetch: async url => { calls.push(url); return data(new Uint8Array([0, 97, 115, 109])); } };
  const original = host.fetch;
  const loader = create({ host });
  await loader.warmWasm("game.wasm?v=1");
  let passed;
  const diagnostics = response => { passed = response; return response; };
  const runtime = (url, wrap) => host.fetch(url).then(wrap).then(r => r.arrayBuffer());
  const bytes = await loader.startRuntime(runtime, "game.wasm?v=1", diagnostics);
  assert.deepEqual(Array.from(new Uint8Array(bytes)), [0, 97, 115, 109]);
  assert.equal(passed.headers.get("content-type"), "application/wasm");
  assert.deepEqual(calls, ["game.wasm?v=1"]);
  assert.equal(host.fetch, original);
  assert.throws(() => loader.startRuntime(() => { throw Error("startup"); }, "game.wasm?v=1"), /startup/);
  assert.equal(host.fetch, original);
});

test("a failed optional WASM prefetch falls back to the ordinary runtime request", async () => {
  let count = 0;
  const host = { fetch: async () => { if (++count === 1) throw Error("network"); return data("wasm"); } };
  const loader = create({ host });
  assert.equal(await loader.warmWasm("game.wasm"), null);
  assert.equal(await loader.startRuntime(url => host.fetch(url).then(r => r.text()), "game.wasm"), "wasm");
  assert.equal(count, 2);
});

test("bounded response reader rejects oversized advertised and streamed bodies", async () => {
  await assert.rejects(readBounded(new Response("data", { headers: { "content-length": "5" } }), 4), /byte limit/);
  await assert.rejects(readBounded(data(new Uint8Array(5)), 4), /byte\/chunk limit/);
  assert.deepEqual(Array.from(await readBounded(data(new Uint8Array([1, 2, 3, 4])), 4)), [1, 2, 3, 4]);
});

test("manifest failures cannot inject cross-zone, traversal or duplicate downloads", async () => {
  for (const paths of [
    ["assets/zones/extreme/other.lvl"],
    ["assets/zones/chaos-zone/../bad.lvl"],
    ["assets/zones/chaos-zone/a.lvl", "assets/zones/chaos-zone/a.lvl"],
  ]) {
    const calls = [];
    const loader = create({ fetch: async url => { calls.push(url); return manifestResponse({ "chaos-zone": paths }); } });
    // Traversal is rejected by request(), and is never fetched, just like an
    // ordinary failed speculative file. Cross-zone/duplicates reject manifest.
    const result = await loader.prepare("chaos-zone", "manifest").catch(() => null);
    if (result) assert.ok(result.every(item => item.status === "rejected"));
    assert.deepEqual(calls, ["manifest"]);
  }
});


test("actual page warms before waiting for the player and packages both startup resources", () => {
  const html = fs.readFileSync("index.html", "utf8");
  const begin = html.indexOf("discoverArenaChoices().finally(() => {");
  const region = html.slice(begin);
  assert.ok(region.indexOf("void warmSelectedZone();") < region.indexOf("resolvePlayerIdentity().then"));
  assert.ok(html.indexOf("void zoneWarmup.warmWasm(") < begin);
  assert.ok(html.includes("zoneWarmup.installFileLoader(importObject.env, FS, () => wasm_exports, () => wasm_memory)"));
  assert.ok(html.includes("zoneWarmup.startRuntime(load,"));
  const release = fs.readFileSync("tools/build_wasm_release.mjs", "utf8");
  assert.ok(release.includes('writeZoneAssetManifest(repoRoot, path.join(distRoot, "zone-assets.json"))'));
  assert.ok(release.includes('const zoneManifestPath = "zone-assets.json";'));
  assert.ok(release.includes('"zubspace_zone_loading.js"'));
  const check = fs.readFileSync("tools/release_bundle_check.mjs", "utf8");
  assert.ok(check.includes('"/zone-assets.json"'));
  assert.ok(check.includes('"/js/zubspace_zone_loading.js"'));
});


test("WASM warm limit accommodates the actual symbol-bearing development client", () => {
  const wasm = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
  if (fs.existsSync(wasm)) assert.ok(fs.statSync(wasm).size <= globalThis.ZubspaceZoneLoading.limits.wasmBytes);
});
