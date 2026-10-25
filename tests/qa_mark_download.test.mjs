import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";
import assert from "node:assert/strict";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
const start = html.indexOf("          async function markQaIssue(");
const end = html.indexOf("          const qaStallObserver =", start);
assert.ok(start >= 0 && end > start);
function fixture(mode) {
  const downloads = [];
  const browser = { bundleId: "fresh", marker: { serial: 3 } };
  const context = vm.createContext({ performance, URLSearchParams, setTimeout,
    qaDiagnosticDepth: 0, qaTraceMarkSerial: 0,
    qaStallObserver: { resetTiming() {} }, qaFrameDiagnostics: { resetTiming() {} },
    QA_TRACE_DEFAULT_POST_TAIL_MS: 0, qaTraceMark: null, qaTraceId: null,
    qaTracePrivateContext: { bundleId: "fresh", uploadToken: "not-exported" },
    qaTraceUploadEndpoint: "/trace", canvas: { focus() {} },
    window: { location: { search: "?qa_upload=1" }, __zubspaceLastQaTraceBundle: {bundleId:"old"} },
    buildQaTraceBundle() { if(mode === "capture-failed") throw Error("capture-failed"); return browser; },
    downloadQaTrace(value) { downloads.push(value); },
    async fetch() {
      if(mode === "dropped") throw Error("dropped");
      return { ok: mode === "clean", status: 429,
        async json() { return mode === "clean" ? {browser} : {ok:false,error:"rate-limited"}; } };
    },
  });
  vm.runInContext(html.slice(start, end), context);
  return { downloads, browser, context, run: options => context.markQaIssue({postTailMs:0,...options}) };
}
for(const mode of ["dropped", "rate-limited"]) {
  test(`${mode} upload preserves fresh local download and reports failure`, async () => {
    const f = fixture(mode);
    await assert.rejects(f.run(), /local browser capture downloaded/);
    assert.deepEqual(f.downloads, [f.browser]);
    assert.equal(f.context.qaDiagnosticDepth, 0);
    const noDownload = fixture(mode);
    await assert.rejects(noDownload.run({download:false}));
    assert.equal(noDownload.downloads.length, 0);
  });
}
test("capture failure never exports an older incident", async () => {
  const f = fixture("capture-failed");
  await assert.rejects(f.run());
  assert.equal(f.downloads.length, 0);
});
test("successful upload downloads joined evidence; local export needs no upload", async () => {
  const f = fixture("clean"); await f.run();
  assert.deepEqual(f.downloads, [{browser:f.browser}]);
  const local = fixture("dropped"); await local.run({upload:false});
  assert.deepEqual(local.downloads, [local.browser]);
});
