import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import crypto from "node:crypto";
import { Readable } from "node:stream";
import { Worker as ThreadWorker } from "node:worker_threads";
import test from "node:test";

const workerUrl = new URL("../js/zubspace_wasm_identity_worker.js", import.meta.url).href;
const source = fs.readFileSync(new URL("../js/zubspace_wasm_identity.js", import.meta.url), "utf8");
const workerAdapter = new URL(`data:text/javascript,${encodeURIComponent(`
  import { parentPort, workerData } from "node:worker_threads";
  globalThis.self = { postMessage: data => parentPort.postMessage(data) };
  await import(workerData.url);
  parentPort.on("message", data => self.onmessage({ data }));
`)}`);

class WorkerBridge {
  constructor(url) {
    this.child = new ThreadWorker(workerAdapter, { workerData: { url } });
    this.child.on("message", data => this.onmessage?.({ data }));
    this.child.on("error", () => this.onerror?.({ preventDefault() {} }));
  }
  postMessage(data, transfer) { this.child.postMessage(data, transfer); }
  terminate() { void this.child.terminate(); }
}

function capture(options, Worker = WorkerBridge) {
  const context = vm.createContext({ Worker, TransformStream, Response,
    Uint8Array, performance, setTimeout, clearTimeout });
  vm.runInContext(source, context);
  return context.ZubspaceWasmIdentity.createCapture(workerUrl, options);
}

function responseChunks(bytes, chunkSize = 65536, headers) {
  let offset = 0;
  return new Response(new ReadableStream({
    pull(controller) {
      if (offset === bytes.length) return controller.close();
      const end = Math.min(bytes.length, offset + chunkSize);
      controller.enqueue(bytes.subarray(offset, end));
      offset = end;
    },
  }), { headers });
}

test("worker SHA-256 matches known answers and independent crypto across padding/chunk boundaries", async () => {
  for (const [size, chunkSize] of [[3, 1], [55, 7], [56, 7], [57, 7], [63, 13], [64, 64],
    [65, 17], [129, 1], [1000000, 65536], [300001, 300001]]) {
    const bytes = new Uint8Array(size).fill(97);
    if (size === 3) bytes.set([97, 98, 99]);
    const identity = capture();
    const output = await identity.wrapResponse(responseChunks(bytes, chunkSize)).arrayBuffer();
    assert.deepEqual(new Uint8Array(output), bytes);
    const result = identity.snapshot();
    assert.equal(result.status, "ready", JSON.stringify(result));
    assert.equal(result.bytes, size);
    assert.equal(result.sha256, crypto.createHash("sha256").update(bytes).digest("hex"));
    if (size === 3) assert.equal(result.sha256,
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    if (size === 1000000) assert.equal(result.sha256,
      "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    assert.equal(result.workerTerminated, true);
    assert.ok(result.elapsedMs >= result.maximumRelayMs);
  }
});

test("hash is single-pass and a later response cannot replace the loaded identity", async () => {
  const identity = capture();
  await identity.wrapResponse(new Response("abc")).arrayBuffer();
  const before = identity.snapshot();
  const replacement = new Response("different deployed bytes");
  assert.equal(identity.wrapResponse(replacement), replacement);
  assert.equal(await replacement.text(), "different deployed bytes");
  assert.deepEqual(identity.snapshot(), before);
  before.sha256 = "forged";
  assert.notEqual(identity.snapshot().sha256, "forged");
});

test("borrowed subarray storage is never detached or hashed outside its exact bounds", async () => {
  const backing = Buffer.alloc(1024, 0xa5);
  backing.set([97, 98, 99], 100);
  const identity = capture();
  const body = new ReadableStream({ start(controller) {
    controller.enqueue(backing.subarray(100, 103));
    controller.close();
  } });
  assert.equal(await identity.wrapResponse(new Response(body)).text(), "abc");
  assert.equal(backing.length, 1024);
  assert.equal(backing[99], 0xa5);
  assert.equal(backing[103], 0xa5);
  assert.equal(identity.snapshot().bytes, 3);
  assert.equal(identity.snapshot().sha256, crypto.createHash("sha256").update("abc").digest("hex"));
});

test("missing worker, errors, mismatched replies and timeout preserve every compiler byte", async () => {
  class Missing { constructor() { throw new Error("unavailable"); } }
  class ErrorReply {
    postMessage() { queueMicrotask(() => this.onmessage({ data: { type: "error" } })); }
    terminate() {}
  }
  class WrongReply extends ErrorReply {
    postMessage() { queueMicrotask(() => this.onmessage({ data: { type: "ack", bytes: -1 } })); }
  }
  class NeverReplies { postMessage() {} terminate() {} }
  for (const Worker of [Missing, ErrorReply, WrongReply, NeverReplies]) {
    const identity = capture({ timeoutMs: 25 }, Worker);
    assert.equal(await identity.wrapResponse(new Response("abc")).text(), "abc");
    assert.equal(identity.snapshot().status, "unavailable");
    assert.equal(identity.snapshot().sha256, null);
    assert.equal(identity.snapshot().workerTerminated, true);
  }
});

test("empty and oversized bodies cannot claim identity or break normal loading", async () => {
  for (const response of [new Response(""), new Response("abc", { headers: { "content-length": "2000" } }),
    responseChunks(new Uint8Array(40).fill(97), 4)]) {
    const identity = capture({ maxBytes: 16 });
    const expected = await response.clone().text();
    assert.equal(await identity.wrapResponse(response).text(), expected);
    assert.equal(identity.snapshot().status, "unavailable");
    assert.equal(identity.snapshot().sha256, null);
    assert.equal(identity.snapshot().workerTerminated, true);
  }
});

test("streaming diagnostic uses one bounded worker request at a time", async () => {
  let pending = 0, maximumPending = 0, maximumBytes = 0, processed = 0;
  class MeasuredWorker {
    postMessage(data) {
      pending += 1;
      maximumPending = Math.max(maximumPending, pending);
      maximumBytes = Math.max(maximumBytes, data.buffer?.byteLength ?? 0);
      processed += data.buffer?.byteLength ?? 0;
      queueMicrotask(() => {
        pending -= 1;
        this.onmessage({ data: data.type === "finish"
          ? { type: "digest", bytes: processed, sha256: "a".repeat(64), cpuMs: 0 }
          : { type: "ack", bytes: processed } });
      });
    }
    terminate() {}
  }
  const identity = capture({}, MeasuredWorker);
  const bytes = new Uint8Array(1024 * 1024);
  await identity.wrapResponse(responseChunks(bytes, bytes.length)).arrayBuffer();
  assert.equal(identity.snapshot().status, "ready");
  assert.equal(maximumPending, 1);
  assert.equal(maximumBytes, 256 * 1024);
  assert.equal(processed, bytes.length);
});

test("actual WASM stream retains exact content identity without loading a second deployment response",
  { skip: !process.env.ZUBSPACE_TEST_WASM_PATH }, async () => {
  const file = process.env.ZUBSPACE_TEST_WASM_PATH;
  const identity = capture();
  const expected = crypto.createHash("sha256");
  let bytes = 0;
  const response = identity.wrapResponse(new Response(Readable.toWeb(fs.createReadStream(file))));
  for await (const chunk of response.body) { expected.update(chunk); bytes += chunk.length; }
  assert.equal(identity.snapshot().status, "ready", JSON.stringify(identity.snapshot()));
  assert.equal(identity.snapshot().bytes, bytes);
  assert.equal(identity.snapshot().sha256, expected.digest("hex"));
  assert.equal(identity.snapshot().workerTerminated, true);
});

test("the actual Macroquad client has no unresolved wasm-bindgen glue imports",
  { skip: !process.env.ZUBSPACE_TEST_WASM_PATH }, async () => {
  const module = await WebAssembly.compile(fs.readFileSync(process.env.ZUBSPACE_TEST_WASM_PATH));
  const imports = WebAssembly.Module.imports(module);
  assert.deepEqual(imports.filter(entry => entry.module.startsWith("__wbindgen")), [],
    "Macroquad loads WASM directly; logging must use real runtime imports");
  for (const name of ["webgl_selection_version", "init_webgl", "audio_source_state"])
    assert.ok(imports.some(entry => entry.module === "env" && entry.name === name), name);
});

test("real loader preserves streaming and fallback compilation with optional response preparation", async () => {
  const bundle = fs.readFileSync("js/mq_js_bundle.js", "utf8");
  const start = bundle.indexOf("function load(");
  const end = bundle.indexOf('(function(){"use strict";const c=window.AudioContext', start);
  assert.ok(start > 0 && end > start);
  const loader = bundle.slice(start, end);
  for (const streaming of [false, true]) {
    for (const mode of ["ordinary", "traced", "throwing", "ordinary-prefetched", "traced-prefetched"]) {
      const bytes = Uint8Array.from([0, 97, 115, 109, 1, 0, 0, 0]);
      const identity = capture();
      let fetches = 0, prepares = 0, completed;
      const finished = new Promise(resolve => { completed = resolve; });
      const compile = async input => {
        assert.deepEqual(new Uint8Array(input), bytes);
        return {};
      };
      const context = vm.createContext({ fetch: async () => { fetches++; return new Response(bytes); },
        plugins: [], register_plugins() {}, add_missing_functions_stabs() {}, init_plugins() {},
        version: 1, console, Response, Uint8Array, TextDecoder, AbortController, setTimeout, clearTimeout, WebAssembly: {
          compileStreaming: streaming ? async response => compile(await (await response).arrayBuffer()) : undefined,
          compile, instantiate: async () => ({ exports: { memory: {}, crate_version: () => 1, main: completed } }),
        }, importObject: {} });
      vm.runInContext(loader, context);
      const prefetched = mode.endsWith("-prefetched");
      const ordinary = mode.startsWith("ordinary");
      vm.runInContext(fs.readFileSync("js/zubspace_zone_loading.js", "utf8"), context);
      context.zoneWarmup = context.ZubspaceZoneLoading.create();
      if (prefetched) await context.zoneWarmup.warmWasm("unchanged-url");
      const prepare = ordinary ? undefined : response => {
        prepares++;
        if (mode === "throwing") throw new Error("diagnostic disabled");
        return identity.wrapResponse(response);
      };
      if (prefetched) context.zoneWarmup.startRuntime(context.load, "unchanged-url", prepare);
      else context.load("unchanged-url", prepare);
      await finished;
      assert.equal(fetches, 1);
      assert.equal(prepares, ordinary ? 0 : 1);
      if (mode.startsWith("traced")) assert.equal(identity.snapshot().status, "ready");
    }
  }
});

test("optional identity script cannot block startup or replace a load after its deadline", () => {
  const entry = fs.readFileSync("index.html", "utf8");
  const start = entry.indexOf("function loadGameRuntime() {");
  const end = entry.indexOf("discoverArenaChoices().finally", start);
  const runtimeLoader = entry.slice(start, end);
  for (const mode of ["ordinary", "traced", "script-error", "factory-error", "timeout"]) {
    const scripts = [], loads = [];
    let deadline = null, factoryCalls = 0;
    const hook = response => response;
    const context = vm.createContext({ URLSearchParams,
      assetVersion: "test", wasmPath: "actual.wasm", wasmIdentityCapture: null,
      importObject: {env:{}}, FS: {}, wasm_exports: {}, wasm_memory: {},
      installZubspaceAudioUnlockBridge() {},
      add_missing_functions_stabs: null,
      installZubspaceRtcBridge() {}, installZubspaceLoadingBridge() {},
      document: { createElement: () => ({}), body: { appendChild: script => scripts.push(script) } },
      load: (...args) => loads.push(args),
      window: { location: { search: mode === "ordinary" ? "" : "?trace=1" },
        setTimeout: (callback, ms) => { assert.equal(ms, 5000); deadline = callback; return 1; },
        clearTimeout: () => { deadline = null; },
        ZubspaceWasmIdentity: { createCapture: () => {
          factoryCalls++;
          if (mode === "factory-error") throw new Error("unavailable");
          return { wrapResponse: hook };
        } },
      },
    });
    vm.runInContext(fs.readFileSync("js/zubspace_zone_loading.js", "utf8"), context);
    context.zoneWarmup = context.ZubspaceZoneLoading.create({host:{fetch:()=>{throw Error("unexpected request");}}});
    vm.runInContext(runtimeLoader, context);
    context.loadGameRuntime();
    scripts[0].onload();
    assert.equal(typeof context.importObject.env.fs_load_file, "function");
    if (mode === "ordinary") {
      assert.equal(scripts.length, 1);
      assert.equal(factoryCalls, 0);
    } else {
      assert.equal(scripts.length, 2);
      assert.equal(loads.length, 0);
      if (mode === "timeout") { deadline(); scripts[1].onload(); }
      else if (mode === "script-error") scripts[1].onerror();
      else scripts[1].onload();
      scripts[1].onerror();
    }
    assert.equal(loads.length, 1);
    assert.equal(loads[0][0], "actual.wasm?v=test");
    assert.equal(loads[0][1], mode === "traced" ? hook : undefined);
    if (mode === "timeout") assert.equal(factoryCalls, 0);
    assert.equal(deadline, null);
  }
});

// The metrics payload is one positional `format!` in Rust, and the fields a stalled browser run is diagnosed from exist only in
// the wasm build: the readiness chain, the two halves of the gameplay-step predicate, the transport verdict that separates a dead
// socket from a readiness bug, and the zone-file request state that closed the stranding defect. A native test cannot observe any
// of them - the native payload variant genuinely omits those keys - so the only honest check is against the shipped binary.
test("the built client publishes the readiness, transport and zone-file metric keys", () => {
  const wasmUrl = new URL("../target/wasm32-unknown-unknown/debug/zubspace.wasm", import.meta.url);
  const bytes = fs.readFileSync(wasmUrl);
  for (const key of [
    "zoneFilesReady",
    "gameplayStepReady",
    "gameplayScopeReady",
    "stepAuthorityReady",
    "rebaseSuppressed",
    "rtcTransportStatus",
    "rtcTransportTerminal",
    "rtcRejoinCount",
    "fileRequestInFlight",
    "fileRequestReissues",
    "fileRequestFramesSinceSend",
  ]) {
    assert.ok(
      bytes.includes(Buffer.from(key)),
      `${key} must be present in the built client, because a stalled run is attributed from these fields`,
    );
  }
});
