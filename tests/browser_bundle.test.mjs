import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

// Exercise the pinned fork runtime so regeneration cannot silently regress asset copies.
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const bundlePath = path.join(repoRoot, "js", "mq_js_bundle.js");
const bundle = fs.readFileSync(bundlePath, "utf8");

// Minified and on one line, so the only safe extraction is brace counting from the function head.
function bundleFunction(name) {
  const head = bundle.indexOf(`${name}:function(`);
  assert.ok(head >= 0, `${name} must exist in ${path.relative(repoRoot, bundlePath)}`);
  const bodyStart = bundle.indexOf("{", head);
  let depth = 0;
  for (let index = bodyStart; index < bundle.length; index += 1) {
    if (bundle[index] === "{") depth += 1;
    else if (bundle[index] === "}" && --depth === 0) {
      const params = bundle.slice(bundle.indexOf("(", head) + 1, bundle.indexOf(")", head));
      return {
        params: params.split(",").map((param) => param.trim()),
        body: bundle.slice(bodyStart + 1, index),
      };
    }
  }
  throw new Error(`unbalanced braces reading ${name}`);
}

function takeBufferHarness({ memoryBytes = 4_096 } = {}) {
  const fn = bundleFunction("fs_take_buffer");
  // The minified body reads `wasm_memory`, `FS` and `console` as bundle globals, so the harness
  // wraps it in a scope that supplies those names rather than passing them as call arguments.
  const invoke = new Function("wasm_memory", "FS", "console", ...fn.params, fn.body);
  const memory = new ArrayBuffer(memoryBytes);
  const view = new Uint8Array(memory);
  // The bundle stores loaded files as plain properties, so the harness must too.
  const loaded = {};
  const assertCalls = [];
  return {
    view,
    loaded,
    assertCalls,
    call(id, target, length) {
      invoke(
        { buffer: memory },
        { loaded_files: loaded },
        { assert: (condition, ...rest) => assertCalls.push({ condition, rest }) },
        id,
        target,
        length,
      );
    },
  };
}

function bundleFunctionArgumentOrder() {
  // fs_take_buffer(id, ptr, len) in the vendored bundle's own parameter naming; pinning the order
  // keeps the harness honest if the signature ever changes.
  const fn = bundleFunction("fs_take_buffer");
  assert.equal(fn.params.length, 3, `expected (id, ptr, len), got ${fn.params.join(",")}`);
  assert.match(fn.params[1], /.*/, "the destination pointer parameter must exist");
  assert.match(fn.params[2], /.*/, "the destination length parameter must exist");
}

test("the bundle copies a loaded asset into module memory with one typed-array copy", () => {
  // The client boots by pulling ~12MB of shipped assets through this one function, and upstream
  // miniquad does that byte-by-byte in JavaScript. Measured on the Chaos-scale wormhole fixture,
  // that loop is the single synchronous main-thread task that freezes a Firefox join for 583ms and
  // leaves Chrome with 106-199ms page-wide gaps in the same phase.
  bundleFunctionArgumentOrder();
  assert.match(
    bundle,
    /zubspace:fs-take-buffer-memcpy/,
    "the bundle must carry the marker for the patched asset copy, or a regeneration has restored the byte loop",
  );
  const fn = bundleFunction("fs_take_buffer");
  assert.doesNotMatch(fn.body, /for\s*\(/, `the asset copy may not loop over bytes: ${fn.body}`);
  assert.match(fn.body, /\.set\(/, "the asset copy must be a typed-array set()");
});

test("the patched asset copy moves every byte and drops the cached file", () => {
  const harness = takeBufferHarness();
  const asset = Uint8Array.from({ length: 1_000 }, (_, index) => (index * 7) & 0xff);
  harness.loaded[7] = asset;
  harness.call(7, 128, asset.length);
  assert.deepEqual(Array.from(harness.view.subarray(128, 128 + asset.length)), Array.from(asset),
    "every byte of the asset must land at the destination the module asked for");
  assert.equal("7" in harness.loaded, false, "the cached file must be released after the copy");
  assert.deepEqual(harness.assertCalls.map((call) => call.condition), [true],
    "a fitting asset must not trip the size assertion");
});

test("an oversized asset cannot write past the buffer the module reserved", () => {
  const harness = takeBufferHarness();
  const asset = Uint8Array.from({ length: 64 }, (_, index) => index + 1);
  harness.loaded[9] = asset;
  const guardStart = 16 + 32;
  harness.view.fill(0xa5, guardStart, guardStart + 32);
  harness.call(9, 16, 32);
  assert.deepEqual(Array.from(harness.view.subarray(16, 16 + 32)), Array.from(asset.subarray(0, 32)),
    "the reserved window must still be filled from the asset");
  assert.deepEqual(Array.from(harness.view.subarray(guardStart, guardStart + 32)),
    new Array(32).fill(0xa5),
    "an over-long asset may not overwrite memory the module never reserved for it");
  assert.equal(harness.assertCalls.at(-1)?.condition, false,
    "the size mismatch must still be reported");
});
