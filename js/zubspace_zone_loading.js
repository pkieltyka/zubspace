(function (root) {
  "use strict";
  const fileLimit = 1024 * 1024 - 17;
  const zoneLimit = 32 * 1024 * 1024;
  // Debug WASM retains symbols and is currently about 81 MiB. Match the
  // existing identity collector's cap rather than defeating its prefetch.
  const wasmLimit = 128 * 1024 * 1024;
  const safePath = /^assets\/zones\/[a-z0-9-]+\/(?:[A-Za-z0-9_#@.-]+\/)*[A-Za-z0-9_#@.-]+\.(?:lvl|lvz|cfg|conf|ini|set)$/i;

  async function readBounded(response, limit) {
    if (!response.ok) throw new Error(`asset HTTP ${response.status}`);
    const length = response.headers.get("content-length");
    if (length !== null && (!/^\d+$/.test(length) || Number(length) > limit)) {
      await response.body?.cancel();
      throw new Error("asset exceeds byte limit");
    }
    const reader = response.body.getReader();
    const chunks = [];
    let total = 0;
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        total += value.byteLength;
        if (total > limit || chunks.length >= Math.max(4096, Math.ceil(limit / 1024))) throw new Error("asset exceeds byte/chunk limit");
        chunks.push(value);
      }
    } catch (error) {
      await reader.cancel().catch(() => {});
      throw error;
    } finally { reader.releaseLock(); }
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return bytes;
  }

  function create(options = {}) {
    const host = options.host || root;
    const fetchFile = options.fetch || host.fetch.bind(host);
    const requests = new Map();
    const completedFiles = new Map();
    const zones = new Set();
    const queue = [];
    const stats = { cacheHits: 0, started: 0, completed: 0, failures: 0, bytes: 0, active: 0, peak: 0 };
    let manifest = null;
    let retained = 0;
    let wasm = null;

    async function fetchBytes(url, limit) {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 20000);
      try { return await readBounded(await fetchFile(url, { signal: controller.signal }), limit); }
      finally { clearTimeout(timeout); }
    }
    function pump() {
      while (stats.active < 6 && queue.length) {
        const job = queue.shift();
        stats.active += 1;
        stats.started += 1;
        stats.peak = Math.max(stats.peak, stats.active);
        (async()=>{
          const value=manifest?await manifest.catch(()=>null):null;
          await options.cache?.ready;
          const digest=value?.sha256?.[job.path];
          const parts=job.path.split("/");
          const identity=/^[a-f0-9]{64}$/.test(digest||"")?`v2.${parts[2]}.${digest}.${parts.at(-1)}`:null;
          const hit=identity&&options.cache?.read(identity);
          if(hit){stats.cacheHits++;return hit;}
          return fetchBytes(job.path.replaceAll("#", "%23"), fileLimit);
        })().then(bytes => {
          if (retained + bytes.byteLength > zoneLimit) throw new Error("startup assets exceed zone byte limit");
          retained += bytes.byteLength;
          stats.bytes += bytes.byteLength;
          stats.completed += 1;
          completedFiles.set(job.path,bytes);
          job.resolve(bytes);
        }).catch(error => { stats.failures += 1; job.reject(error); })
          .finally(() => { stats.active -= 1; pump(); });
      }
    }
    function request(path) {
      if (!safePath.test(path) || path.split("/").some(part => part === ".." || part === ".")) {
        return Promise.reject(new Error("invalid zone asset path"));
      }
      if (!requests.has(path)) {
        if (requests.size >= 128) return Promise.reject(new Error("too many startup assets"));
        const pending = new Promise((resolve, reject) => queue.push({ path, resolve, reject }));
        // A speculative failure is consumed even if WASM never needs that file.
        pending.catch(() => {});
        requests.set(path, pending);
        pump();
      }
      return requests.get(path);
    }
    async function prepare(zone, manifestUrl) {
      zones.add(zone);
      if (!manifest) {
        manifest = fetchBytes(manifestUrl, 1024 * 1024).then(bytes => {
          const value = JSON.parse(new TextDecoder().decode(bytes));
          if (![1,2].includes(value.version) || !value.zones || typeof value.zones !== "object") throw new Error("invalid zone manifest");
          return value;
        });
      }
      const value = await manifest;
      const paths = Object.hasOwn(value.zones, zone) ? value.zones[zone] : [];
      if (!Array.isArray(paths) || paths.length > 128 || new Set(paths).size !== paths.length ||
          paths.some(path => typeof path !== "string" || !safePath.test(path) || !path.startsWith(`assets/zones/${zone}/`))) {
        throw new Error("invalid selected-zone manifest");
      }
      // Generation orders map and settings first. All requests share one pool.
      return Promise.allSettled(paths.map(request));
    }
    function installFileLoader(env, files, exports, memory) {
      const original = env.fs_load_file;
      env.fs_load_file = (ptr, len) => {
        const encoded = new TextDecoder().decode(new Uint8Array(memory().buffer, ptr, len));
        let path;
        try { path = decodeURIComponent(encoded); } catch { return original(ptr, len); }
        const zone = path.split("/")[2];
        if (!zones.has(zone) || !safePath.test(path)) return original(ptr, len);
        const id = files.unique_id++;
        request(path).then(bytes => { files.loaded_files[id] = bytes; }, () => { files.loaded_files[id] = null; })
          .then(() => exports().file_loaded(id));
        return id;
      };
    }
    function warmWasm(url) {
      if (!wasm || wasm.url !== url) {
        wasm = { url, bytes: fetchBytes(url, wasmLimit).catch(() => null) };
      }
      return wasm.bytes;
    }
    function startRuntime(loader, url, prepareResponse) {
      if (wasm?.url !== url) return loader(url, prepareResponse);
      const original = host.fetch;
      const prefetched = wasm;
      wasm = null; // Release the warm byte buffer after the compiler takes it.
      // The pinned runtime calls fetch synchronously in load(). Intercept only
      // that call; restore immediately, retaining its ordinary import checks.
      host.fetch = function (input, options) {
        if (input === url && !options) return prefetched.bytes.then(bytes => bytes
          ? new Response(bytes, { headers: { "content-type": "application/wasm" } })
          : original.call(host, input, options));
        return original.call(host, input, options);
      };
      try { return loader(url, prepareResponse); }
      finally { host.fetch = original; }
    }
    function provisionalBytes(identity) {
      const parts=root.ZubspaceZoneCache?.identityParts(identity);
      return parts?completedFiles.get(`assets/zones/${parts.zone}/${parts.filename}`)||null:null;
    }
    return { prepare, warmWasm, startRuntime, installFileLoader, provisionalBytes, stats };
  }
  root.ZubspaceZoneLoading = { create, readBounded, limits: Object.freeze({fileBytes:fileLimit, zoneBytes:zoneLimit, wasmBytes:wasmLimit, concurrency:6}) };
})(globalThis);
