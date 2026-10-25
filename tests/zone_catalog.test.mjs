import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

await import("../js/zubspace_zone_catalog.js");

const limits = globalThis.zubspaceZoneCatalogLimits;
const embedded = globalThis.zubspaceEmbeddedZoneCatalog;
const readCatalog = globalThis.zubspaceReadZoneCatalogResponse;
const resolveCatalog = globalThis.zubspaceResolveZoneCatalogResponse;
const resolveArena = globalThis.zubspaceResolveArenaSelection;
const encoder = new TextEncoder();
const identityCorpus = JSON.parse(
  fs.readFileSync(
    new URL("./fixtures/zone_identity_corpus.json", import.meta.url),
    "utf8",
  ),
);

function descriptor(index = 0, overrides = {}) {
  return {
    id: `zone-${index}`,
    display_name: `Display ${index}`,
    map: `map${index}.lvl`,
    status: "open", default: false, listed: false, total_players: 0,
    max_players: 500, free_for_all: true, maxed_spawn_loadout: true,
    ...overrides,
  };
}

function catalog(zones) {
  const live = zones.map(zone => ({ status: "open", default: false, listed: false, total_players: 0,
    max_players: 500, free_for_all: true, maxed_spawn_loadout: true, ...zone }));
  return JSON.stringify({ total_players: live.reduce((sum, zone) => sum + zone.total_players, 0), zones: live });
}

function fakeResponse(parts, options = {}) {
  const chunks = parts.map((part) =>
    typeof part === "string" ? encoder.encode(part) : part,
  );
  let index = 0;
  const audit = { cancelled: 0, released: 0, reads: 0 };
  const reader = {
    async read() {
      audit.reads += 1;
      if (options.errorAt === index) {
        throw options.readError || new Error("injected reader error");
      }
      if (index >= chunks.length) {
        return { done: true, value: undefined };
      }
      return { done: false, value: chunks[index++] };
    },
    async cancel() {
      audit.cancelled += 1;
      if (options.cancelError) {
        throw new Error("injected cancel error");
      }
    },
    releaseLock() {
      audit.released += 1;
    },
  };
  return {
    ok: options.ok ?? true,
    headers: {
      get(name) {
        if (name.toLowerCase() !== "content-length") return null;
        return options.contentLength ?? null;
      },
    },
    body: { getReader: () => reader },
    audit,
  };
}

test("zone catalog streams the exact encoded cap and rejects cap plus one before JSON parse", async () => {
  const zones = Array.from({ length: limits.maximumZones }, (_, index) =>
    descriptor(index),
  );
  const base = catalog(zones);
  const padding = limits.maximumEncodedBytes - encoder.encode(base).byteLength;
  assert.ok(padding > 0, "derived escape-aware cap must contain 64 valid descriptors");

  const exact = fakeResponse([base, " ".repeat(padding)]);
  const accepted = await readCatalog(exact);
  assert.equal(accepted.length, 64);
  assert.deepEqual(exact.audit, { cancelled: 0, released: 1, reads: 3 });

  const over = fakeResponse([base, " ".repeat(padding), "x"]);
  await assert.rejects(readCatalog(over), /encoded limit/);
  assert.equal(over.audit.cancelled, 1);
  assert.equal(over.audit.released, 0);
});

test("zone catalog accepts 64 descriptors and atomically rejects 65", async () => {
  const sixtyFour = Array.from({ length: 64 }, (_, index) => descriptor(index));
  assert.equal((await readCatalog(fakeResponse([catalog(sixtyFour)]))).length, 64);
  await assert.rejects(
    readCatalog(fakeResponse([catalog([...sixtyFour, descriptor(64)])])),
    /at most 64 zones/,
  );
});

test("zone catalog cancels non-2xx and oversized or malformed Content-Length before reading", async () => {
  const nonOk = fakeResponse([catalog([descriptor(1)])], { ok: false });
  const fallback = await resolveCatalog(nonOk);
  assert.equal(fallback.source, "embedded");
  assert.deepEqual(nonOk.audit, { cancelled: 1, released: 0, reads: 0 });

  for (const contentLength of [
    String(limits.maximumEncodedBytes + 1),
    "not-a-number",
    "12, 12",
    "-1",
    "9007199254740992",
  ]) {
    const response = fakeResponse([catalog([descriptor(1)])], { contentLength });
    await assert.rejects(readCatalog(response));
    assert.deepEqual(
      response.audit,
      { cancelled: 1, released: 0, reads: 0 },
      contentLength,
    );
  }
});

test("zone catalog Content-Length is an early ceiling while streamed bytes remain authoritative", async () => {
  const zones = Array.from({ length: limits.maximumZones }, (_, index) =>
    descriptor(index),
  );
  const base = catalog(zones);
  const padding = limits.maximumEncodedBytes - encoder.encode(base).byteLength;
  const exact = fakeResponse([base, " ".repeat(padding)], {
    contentLength: String(limits.maximumEncodedBytes),
  });
  assert.equal((await readCatalog(exact)).length, 64);

  const lyingSmallerButBounded = fakeResponse([catalog([descriptor(1)])], {
    contentLength: "1",
  });
  assert.equal((await readCatalog(lyingSmallerButBounded)).length, 1);

  const lyingLargerButBounded = fakeResponse([catalog([descriptor(1)])], {
    contentLength: String(limits.maximumEncodedBytes),
  });
  assert.equal((await readCatalog(lyingLargerButBounded)).length, 1);

  const lyingSmallerOverStreamCap = fakeResponse(
    [base, " ".repeat(padding), "x"],
    { contentLength: "1" },
  );
  await assert.rejects(readCatalog(lyingSmallerOverStreamCap), /encoded limit/);
  assert.equal(lyingSmallerOverStreamCap.audit.cancelled, 1);
});

test("zone catalog validates chunked boundaries without response.json", async () => {
  const dotted = descriptor(1, { map: "map.v1.lvl" });
  const body = encoder.encode(catalog([dotted]));
  const response = fakeResponse([
    body.subarray(0, 1),
    body.subarray(1, 17),
    body.subarray(17, body.length - 1),
    body.subarray(body.length - 1),
  ]);
  assert.deepEqual(
    JSON.parse(JSON.stringify(await readCatalog(response))),
    [dotted],
  );
  assert.equal(response.audit.released, 1);
});

test("zone catalog rejects duplicate normalized ids and labels", async () => {
  await assert.rejects(
    readCatalog(fakeResponse([catalog([descriptor(1), descriptor(1)])])),
    /duplicate normalized identity/,
  );
  await assert.rejects(
    readCatalog(
      fakeResponse([
        catalog([
          descriptor(1, { display_name: "Alias-One" }),
          descriptor(2, { display_name: "Alias One" }),
        ]),
      ]),
    ),
    /duplicate normalized identity/,
  );
});

test("shared server/browser display-name corpus has identical acceptance boundaries", async () => {
  assert.deepEqual(
    JSON.parse(
      JSON.stringify(
        await readCatalog(
          fakeResponse([catalog(identityCorpus.accepted_descriptors)]),
        ),
      ),
    ),
    JSON.parse(catalog(identityCorpus.accepted_descriptors)).zones,
  );

  for (const [index, display_name] of identityCorpus.rejected_display_names.entries()) {
    await assert.rejects(
      readCatalog(
        fakeResponse([
          catalog([descriptor(index + 20, { display_name })]),
        ]),
      ),
      undefined,
      `browser accepted server-invalid display_name ${JSON.stringify(display_name)}`,
    );
  }

  for (const collision of identityCorpus.collision_catalogs) {
    await assert.rejects(
      readCatalog(fakeResponse([catalog(collision)])),
      /duplicate normalized identity/,
      `browser accepted server-invalid collision ${JSON.stringify(collision)}`,
    );
  }
});

test("zone catalog rejects malformed envelopes, descriptor aliases, types, and strings", async () => {
  const malformed = [
    null,
    [],
    { total_players: "0", zones: [] },
    { ok: true, total_players: 1, zones: [] },
    { ok: true, zones: {} },
    { ok: true, zones: [null] },

    { ok: true, zones: [descriptor(1, { id: 7 })] },
    { ok: true, zones: [descriptor(1, { id: "Upper" })] },
    { ok: true, zones: [descriptor(1, { id: "a".repeat(16) })] },
    { ok: true, zones: [descriptor(1, { display_name: "" })] },
    { ok: true, zones: [descriptor(1, { display_name: "x\ny" })] },
    { ok: true, zones: [descriptor(1, { display_name: "Zøne" })] },
    { ok: true, zones: [descriptor(1, { display_name: "x".repeat(33) })] },
    { ok: true, zones: [descriptor(1, { map: "../map.lvl" })] },
    { ok: true, zones: [descriptor(1, { map: "bad\\map.lvl" })] },
    { ok: true, zones: [descriptor(1, { map: "https://x/lvl" })] },
    { ok: true, zones: [descriptor(1, { map: "map.png" })] },
    { ok: true, zones: [descriptor(1, { map: "x".repeat(12) + ".lvl" })] },
  ];
  for (const value of malformed) {
    await assert.rejects(
      readCatalog(fakeResponse([JSON.stringify(value && !Array.isArray(value) ? { total_players: 0, ...value } : value)])),
      undefined,
      JSON.stringify(value),
    );
  }
});

test("zone catalog cancels after reader, byte-type, UTF-8, and JSON failures", async () => {
  const readerError = fakeResponse(["{"], { errorAt: 1 });
  await assert.rejects(readCatalog(readerError), /injected reader error/);
  assert.equal(readerError.audit.cancelled, 1);

  const timerAbort = fakeResponse(["{"], {
    errorAt: 1,
    readError: new DOMException("signal timed out", "AbortError"),
  });
  await assert.rejects(readCatalog(timerAbort), { name: "AbortError" });
  assert.equal(timerAbort.audit.cancelled, 1);

  const wrongType = fakeResponse(["{}"]);
  wrongType.body.getReader = () => ({
    read: async () => ({ done: false, value: "not bytes" }),
    cancel: async () => {
      wrongType.audit.cancelled += 1;
    },
  });
  await assert.rejects(readCatalog(wrongType), /non-byte chunk/);
  assert.equal(wrongType.audit.cancelled, 1);

  const invalidUtf8 = fakeResponse([new Uint8Array([0xc3, 0x28])]);
  await assert.rejects(readCatalog(invalidUtf8));
  // The byte stream reached EOF; it is released even though decoding failed.
  assert.equal(invalidUtf8.audit.released, 1);

  const invalidJson = fakeResponse(["{"]);
  await assert.rejects(readCatalog(invalidJson));
  assert.equal(invalidJson.audit.released, 1);
});

test("zone catalog fallback is complete and survives cancel failure", async () => {
  const malformed = fakeResponse(["{"], { cancelError: true, errorAt: 1 });
  const fallback = await resolveCatalog(malformed);
  assert.equal(fallback.source, "embedded");
  assert.strictEqual(fallback.zones, embedded);
  const missingBody = await resolveCatalog({ ok: true, body: null });
  assert.equal(missingBody.source, "embedded");
  assert.strictEqual(missingBody.zones, embedded);
  assert.deepEqual(
    JSON.parse(JSON.stringify(fallback.zones)),
    [
      { id: "chaos-zone", display_name: "Chaos Zone", map: "bdhadar.lvl" },
      { id: "extreme", display_name: "Extreme", map: "doomsy8.lvl" },
    ],
  );

  const html = fs.readFileSync("index.html", "utf8");
  const start = html.indexOf("async function fetchArenaChoices");
  const end = html.indexOf("function readStoredArena", start);
  const source = html.slice(start, end);
  assert.doesNotMatch(source, /response\.json\s*\(/);
  assert.ok(
    source.indexOf("zubspaceResolveZoneCatalogResponse(response)") <
      source.indexOf("installArenaCatalog(catalog.zones)"),
    "complete validation must precede every registration",
  );
  assert.doesNotMatch(source, /registerArenaChoice\(zone\.id/);
  assert.match(source, /__zubspaceZoneCatalogSource = "embedded"/);

  const resolveStart = html.indexOf("function resolveArenaChoice");
  const resolveEnd = html.indexOf("function randomPlayerId", resolveStart);
  const resolveSource = html.slice(resolveStart, resolveEnd);
  assert.doesNotMatch(resolveSource, /registerArenaChoice/);
  assert.match(resolveSource, /zubspaceResolveArenaSelection/);
});

test("arena resolution treats the URL as an initial catalog hint with explicit precedence", () => {
  const zones = [...embedded, descriptor(9, {
    id: "test-zone",
    display_name: "Test Zone",
    map: "test.lvl",
  })];

  assert.equal(
    resolveArena(zones, { queryArena: "Extreme", storedArena: "chaos-zone" }),
    "extreme",
    "a valid catalog query hint wins for initial selection",
  );
  assert.equal(
    resolveArena(zones, { queryArena: "unknown-zone", storedArena: "extreme" }),
    "extreme",
    "an unknown query cannot bypass a valid stored catalog selection",
  );
  assert.equal(
    resolveArena(zones, { queryArena: "unknown-zone", storedArena: "also-unknown" }),
    "chaos-zone",
    "unknown hints fail closed to the defined catalog fallback",
  );
  assert.equal(
    resolveArena(zones.slice(1), {
      queryArena: "unknown-zone",
      storedArena: "also-unknown",
    }),
    "extreme",
    "a catalog without the preferred fallback selects its first advertised zone",
  );
  assert.throws(() => resolveArena([], {}), /nonempty validated catalog/);
});

test("outgoing arena configuration always follows the current resolved DOM selection", () => {
  const html = fs.readFileSync("index.html", "utf8");
  const start = html.indexOf("function configuredParamBytes");
  const end = html.indexOf("function configuredPlayerIdentityBytes", start);
  const source = html.slice(start, end);
  assert.match(
    source,
    /if \(name === "arena"\)\s*\{\s*return encoder\.encode\(window\.__zubspaceArena \|\| "chaos-zone"\)/,
  );
  assert.doesNotMatch(source, /name === "arena"\s*&&/);
  assert.doesNotMatch(source, /params\.get\("arena"\)/);

  // A reconnect on the same page must keep the user's latest catalog choice,
  // irrespective of the URL hint that initialized the page.
  const rawQueryHint = "chaos-zone";
  let currentArena = resolveArena(embedded, { queryArena: rawQueryHint });
  assert.equal(currentArena, "chaos-zone");
  currentArena = "extreme";
  const outgoingArena = () => currentArena || "chaos-zone";
  assert.equal(outgoingArena(), "extreme");
  assert.equal(outgoingArena(), "extreme", "reconnect cannot resurrect the raw URL hint");
});

test("discovery E2E readiness never acquires the game canvas WebGL context", () => {
  const source = fs.readFileSync("tools/browser_real_server_smoke.mjs", "utf8");
  const helperStart = source.indexOf("async runtimeBootstrapState()");
  const expressionStart = source.indexOf("expression: `", helperStart) +
    "expression: `".length;
  const expressionEnd = source.indexOf("`", expressionStart);
  assert.ok(
    helperStart >= 0 && expressionEnd > expressionStart,
    "context-neutral bootstrap expression must exist",
  );
  assert.doesNotThrow(
    () => new Function(`return ${source.slice(expressionStart, expressionEnd)};`),
    "context-neutral bootstrap expression must parse before Chrome",
  );
  const start = source.indexOf(
    'await waitUntil("both discovery-selected clients to start WASM"',
  );
  const end = source.indexOf(
    'await waitUntil("both browser clients to join the real server"',
    start,
  );
  assert.ok(start >= 0 && end > start, "discovery readiness block must exist");
  const readiness = source.slice(start, end);
  assert.match(readiness, /runtimeBootstrapState\(\)/);
  assert.doesNotMatch(readiness, /\.pageState\(\)/);
  assert.doesNotMatch(readiness, /getContext\s*\(/);
});

test("Part A validates live counts and ignores bounded additive metadata", async () => {
  const first = descriptor(1, {total_players: 3, listed: true, default: true});
  const second = descriptor(2, {status: "standby"});
  const payload = {total_players:3,zones:[{...first,future:"ignored"},second],future:true};
  const result = await resolveCatalog(fakeResponse([JSON.stringify(payload)]));
  assert.equal(result.source,"server");assert.equal(result.total,3);
  assert.deepEqual(result.zones,[first,second]);
  for (const overrides of [{total_players:-1},{total_players:1.5},{total_players:"3"},{max_players:0},{max_players:501},
    {status:"standby",total_players:1},{status:null},{listed:"true"},{default:0},{free_for_all:null},{maxed_spawn_loadout:0}]) {
    const invalid = {...payload,zones:[{...first,...overrides},second]};
    await assert.rejects(readCatalog(fakeResponse([JSON.stringify(invalid)])),undefined,JSON.stringify(overrides));
  }
  await assert.rejects(readCatalog(fakeResponse([JSON.stringify({...payload,total_players:4})])),/total disagrees/);
  await assert.rejects(readCatalog(fakeResponse([JSON.stringify({ok:true,zones:[{id:"old",display_name:"Old",map:"old.lvl"}]})])),/schema/);
  const fallback=await resolveCatalog(fakeResponse(["{broken"]));
  assert.equal(fallback.total,null,"fallback has identities, never invented live counts");
});

test("startup discovery shares one in-flight bounded request with the population poll", async () => {
  const vm = await import("node:vm");
  const html = fs.readFileSync("index.html", "utf8");
  const source = html.slice(html.indexOf("        let arenaDiscoveryInFlight"), html.indexOf("        function readStoredArena"));
  let fetches=0,finish;
  const installed=[];
  const window={zubspaceEmbeddedZoneCatalog:embedded,zubspaceResolveZoneCatalogResponse:resolveCatalog,setTimeout,clearTimeout};
  const scope=vm.createContext({window,AbortController,gameServerUrl:()=>"https://server.example/v1/zones",
    installArenaCatalog:zones=>installed.push(zones),fetch:()=>{fetches++;return new Promise(resolve=>{finish=resolve;});}});
  vm.runInContext(source,scope);
  const first=scope.discoverArenaChoices(),second=scope.discoverArenaChoices();
  assert.equal(first,second);assert.equal(fetches,1);
  finish(fakeResponse([catalog([descriptor(1,{total_players:3})])]));
  const result=await first;
  assert.equal(result.total,3);assert.equal(installed.length,1);assert.equal(window.__zubspaceZoneCatalogSource,"server");
  const refresh=scope.discoverArenaChoices();assert.equal(fetches,2);
  finish(fakeResponse(["broken"]));assert.equal(await refresh,undefined);
  assert.equal(installed.length,1,"a failed refresh never installs partial or fabricated live metadata");
});
