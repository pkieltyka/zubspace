import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { assertCompleteQaTraceCaptures, captureBrowserArtifactState,
  prepareBrowserArtifactState, withPreparedBrowserArtifactStates } from "./qa_trace_capture.mjs";
import { installTraceUploadFault } from "./qa_trace_drill.mjs";

const bundle = () => ({ schema: "zubspace-qa-trace-v1", version: 1,
  bundleId: "a".repeat(32), limits: { maxBytes: 240 * 1024 } });
const state = () => ({ qaTraceState: { enabled: true, ready: true, bundleId: "a".repeat(32) } });

test("cold captures bind encounter setup before later driver mutations", async () => {
  const client = { rendezvousTrace: [{ status: "complete", plan: { target: { x: 100, y: 200 } } }],
    freezeQaTrace: async () => bundle(), pageState: async () => state() };
  const capture = await prepareBrowserArtifactState(client);
  client.rendezvousTrace[0].status = "changed";
  client.rendezvousTrace[0].plan.target.x = 999;
  const result = await capture.read();
  assert.equal(result.rendezvousTrace[0].status, "complete");
  assert.deepEqual(result.rendezvousTrace[0].plan.target, { x: 100, y: 200 });
});

test("cold artifacts retain the immutable first stall after live rings have advanced", async () => {
  const incident = { phase: "captured", bundle: bundle() };
  let reads = 0;
  const result = await captureBrowserArtifactState({
    freezeQaTrace: async () => bundle(),
    pageState: async () => ({ ...state(), qaTraceState: {
      ...state().qaTraceState, firstStall: { phase: "captured" },
    } }),
    evaluate: async expression => {
      assert.match(expression, /__zubspaceFirstStallCapture/);
      reads++; return incident;
    },
  });
  assert.equal(reads, 1);
  assert.deepEqual(result.firstStallCapture, incident);
  assert.equal(result.qaTraceCapture.status, "captured");
});

test("an advertised first stall without its saved bundle cannot pass capture", async () => {
  const result = await captureBrowserArtifactState({
    freezeQaTrace: async () => bundle(),
    pageState: async () => ({ ...state(), qaTraceState: {
      ...state().qaTraceState, firstStall: { phase: "captured" },
    } }), evaluate: async () => null,
  });
  assert.equal(result.qaTraceCapture.status, "failed");
  assert.match(result.error, /first-stall artifact/);
});

test("every actor freezes before sequential export can advance live histories", async () => {
  const events = [], histories = [[1], [2]];
  const clients = histories.map((history, i) => ({client: {
    freezeQaTrace: async () => {events.push(`marker-${i}`); return bundle();},
    prepareArtifactState: async () => {
      events.push(`freeze-${i}`); const frozen = [...history];
      return {read: async () => {events.push(`read-${i}`); histories[1].push(3); return {...state(), history:frozen};},
        release: async () => events.push(`release-${i}`)};
    },
  }}));
  await withPreparedBrowserArtifactStates(clients, async captures => {
    assert(events.includes("freeze-0") && events.includes("freeze-1"));
    assert.deepEqual((await captures[0].read()).history, [1]);
    assert.deepEqual((await captures[1].read()).history, [2]);
  });
  assert.equal(events.filter(e=>e.startsWith("release-")).length,2);
});

test("writer failure releases every unread snapshot and preserves cleanup errors", async () => {
  const released = [];
  const clients = [0,1].map(i=>({client:{freezeQaTrace:async()=>bundle(),
    prepareArtifactState:async()=>({read:async()=>state(),release:async()=>{
      released.push(i); if(i===1)throw new Error("cleanup failure");
    }})}}));
  await assert.rejects(withPreparedBrowserArtifactStates(clients,async()=>{throw new Error("disk failure");}),error=>{
    assert(error instanceof AggregateError);
    assert.deepEqual(error.errors.map(e=>e.message),["disk failure","cleanup failure"]);return true;
  });
  assert.deepEqual(released,[0,1]);
});

test("malformed prepared adapters are released and cannot become fresh captures",async()=>{
  let releases=0;
  const prepared=await prepareBrowserArtifactState({freezeQaTrace:async()=>bundle(),
    prepareArtifactState:async()=>({release:async()=>{releases++;}})});
  const captured=await prepared.read();
  assert.equal(captured.qaTraceCapture.status,"failed");
  assert.match(captured.error,/invalid prepared/);assert.equal(releases,1);
  await assert.rejects(prepared.read(),/consumed/);
});

test("capture is fresh, explicit, bounded, and followed by one full state read", async () => {
  const calls = [];
  const result = await captureBrowserArtifactState({
    freezeQaTrace: async () => { calls.push("freeze"); return bundle(); },
    pageState: async () => { calls.push("state"); return state(); },
  });
  assert.deepEqual(calls, ["freeze", "state"]);
  assert.equal(result.qaTraceCapture.status, "captured");
  assert.deepEqual(result.qaTraceBundle, bundle());
  assert.equal(assertCompleteQaTraceCaptures([result.qaTraceCapture]).captured, 1);
});

test("cold adapter failures cannot fall back to a weaker ordinary state read", async () => {
  const result = await captureBrowserArtifactState({freezeQaTrace: async () => bundle(),
    artifactState: async () => { throw new Error("cold transfer failed"); },
    pageState: async () => { throw new Error("ordinary state must not be called"); }});
  assert.equal(result.error, "cold transfer failed");
  assert.equal(result.qaTraceCapture.status, "failed");
});

test("failed or absent adapters cannot borrow an older frozen bundle", async () => {
  for (const adapter of [undefined, async () => { throw new Error("marker failed"); }]) {
    const result = await captureBrowserArtifactState({ freezeQaTrace: adapter,
      pageState: async () => ({ ...state(), qaTraceBundle: bundle() }) });
    assert.equal(result.qaTraceCapture.status, "failed");
    assert.throws(() => assertCompleteQaTraceCaptures([result.qaTraceCapture]), /incomplete/);
    assert.deepEqual(result.qaTraceBundle, bundle(), "old evidence remains available for forensics only");
  }
});

test("unready, unavailable, malformed, mismatched, and oversized captures fail acceptance", async () => {
  for (const [frozen, page] of [
    [null, state()],
    [undefined, state()],
    [bundle(), { qaTraceState: { enabled: true, ready: false } }],
    [bundle(), {}],
    [{ ...bundle(), version: 2 }, state()],
    [{ ...bundle(), bundleId: "b".repeat(32) }, state()],
    [{ ...bundle(), excess: "x".repeat(240 * 1024) }, state()],
  ]) {
    const result = await captureBrowserArtifactState({
      freezeQaTrace: async () => frozen, pageState: async () => page,
    });
    assert.equal(result.qaTraceCapture.status, "failed");
    assert.throws(() => assertCompleteQaTraceCaptures([result.qaTraceCapture]), /incomplete/);
  }
  assert.throws(() => assertCompleteQaTraceCaptures([]), /missing/);
});

test("state read failures remain retained failures even when the marker succeeded", async () => {
  for (const pageState of [() => { throw new Error("closed page"); }, async () => null]) {
    const result = await captureBrowserArtifactState({ freezeQaTrace: async () => bundle(), pageState });
    assert.equal(result.qaTraceCapture.status, "failed");
    assert.ok(result.error);
    assert.deepEqual(result.qaTraceBundle, bundle());
  }
});

test("explicitly disabled ordinary play is accepted without producing a trace", async () => {
  const result = await captureBrowserArtifactState({ freezeQaTrace: async () => null,
    pageState: async () => ({ qaTraceState: { enabled: false, ready: false } }) });
  assert.equal(result.qaTraceBundle, null);
  assert.equal(assertCompleteQaTraceCaptures([result.qaTraceCapture]).disabled, 1);
});

test("trace drill faults affect only the exact HTTP diagnostic endpoint and restore fetch", async () => {
  for (const mode of ["dropped", "malformed", "clean"]) {
    const received = [];
    const window = {};
    const original = async function(url, options) {
      assert.equal(this, window, "native fetch must retain its Window receiver for unbound calls");
      received.push({ url, options });
      return { status: options?.body === "{" ? 400 : 200 };
    };
    window.fetch = original;
    const endpoint = "http://localhost:9001/v1/qa/trace";
    vm.runInNewContext(`(${installTraceUploadFault.toString()})(endpoint, mode)`, { window, endpoint, mode });
    const options = { method: "POST", headers: { private: "never-export" }, body: "{}" };
    const wrappedFetch = window.fetch;
    await wrappedFetch("http://localhost:9001/v1/rtc/offer", options);
    assert.equal(received[0].options, options, "signaling is passed through without alteration");
    assert.equal(window.__qaDrillUploadFault.calls, 0);
    if (mode === "dropped") {
      await assert.rejects(window.fetch(endpoint, options), /qa_drill_dropped_upload/);
      assert.equal(received.length, 1);
    } else {
      await window.fetch(endpoint, options);
      assert.equal(received[1].options.body, mode === "malformed" ? "{" : "{}");
      assert.equal(received[1].options.headers, options.headers);
    }
    assert.equal(window.__qaDrillUploadFault.calls, 1);
    assert.ok(!JSON.stringify(window.__qaDrillUploadFault).includes("never-export"));
    window.__qaDrillRestoreFetch();
    assert.equal(window.fetch, original);
  }
});
