import assert from "node:assert/strict";
import test from "node:test";
import "../js/zubspace_client_debug.js";

function fixture({ muzzleWidth = 32 } = {}) {
  let time = 0, visible = true, active = true, fail = false;
  const sent = [], intervals = new Set();
  const timers = { setTimeout, clearTimeout,
    setInterval(fn) { intervals.add(fn); return fn; }, clearInterval(fn) { intervals.delete(fn); } };
  const context = { bundleId: "a".repeat(32), uploadToken: "b".repeat(64), serverSessionId: "18446744073709551614", arenaStreamId: "9", playerId: 7 };
  const fetcher = async (url, options = {}) => {
    sent.push({ url, ...options });
    if (url.endsWith("/v1/status")) return { ok: true, json: async () => ({ client_debug_trace: true }) };
    if (options.method === "POST") {
      if (fail) throw new Error("unavailable");
      return { ok: true, body: { cancel: async () => {} } };
    }
    return { ok: true, json: async () => ({ enabled: true, schemaVersion: 1, frameWidth: 34, muzzleWidth, intervalMs: 20000, maxFrames: 1200, maxBytes: 196608 }) };
  };
  const tracer = ZubspaceClientDebug.create({ now: () => time, visible: () => visible, active: () => active, fetch: fetcher, build: () => "dev", timers });
  const row = new Float64Array(32).fill(1);
  return { tracer, row, context, sent, intervals, start: () => tracer.setContext(context, "http://localhost:7000/v1/qa/trace"),
    advance: delta => time += delta, visible: value => visible = value, active: value => active = value, fail: value => fail = value };
}

test("client debug stays inert without server policy and sends compact authenticated numeric frames", async () => {
  const f = fixture();
  f.tracer.frame(f.row); assert.equal(f.tracer.snapshot().bufferedFrames, 0);
  await f.start(); assert.equal(f.tracer.requested(), true); assert.equal(f.intervals.size, 1);
  f.tracer.frame(f.row); f.advance(16.667); f.tracer.frame(f.row); f.tracer.event(1);
  assert.equal(await f.tracer.flush(), true);
  const post = f.sent.find(request => request.method === "POST"), body = JSON.parse(post.body);
  assert.equal(post.url, "http://localhost:7000/v1/debug/client-trace");
  assert.equal(post.headers["X-Zubspace-QA-Token"], f.context.uploadToken);
  assert.ok(!post.body.includes(f.context.uploadToken));
  assert.equal(body.scope.serverSessionId, f.context.serverSessionId);
  assert.equal(body.frames.length, 2); assert.equal(body.frames[0].length, 34);
  assert.equal(body.frames[1][1], 16.667); assert.deepEqual(body.events, [[1, 16.667]]);
  f.tracer.clear(); assert.equal(f.intervals.size, 0);
});

test("ring and encoded payload stay bounded and disclose discarded frames", async () => {
  const f = fixture(); await f.start(); f.row.fill(999999999999.123);
  for (let i = 0; i < 4000; i++) { f.advance(16); f.tracer.frame(f.row); }
  assert.equal(f.tracer.snapshot().bufferedFrames, 1200);
  await f.tracer.flush();
  const body = f.sent.find(request => request.method === "POST").body, report = JSON.parse(body);
  assert.ok(Buffer.byteLength(body) <= 196608); assert.ok(report.frames.length < 1200);
  assert.equal(report.frames.length + report.droppedFrames, 4000);
  f.tracer.clear();
});

test("hidden/disconnected time and stale session data do not leak into a successor", async () => {
  const f = fixture(); await f.start(); f.tracer.frame(f.row); f.visible(false);
  await f.tracer.flush(); assert.equal(f.sent.filter(request => request.method === "POST").length, 0);
  f.visible(true); f.advance(50000); f.tracer.frame(f.row);
  await f.tracer.setContext({ ...f.context, bundleId: "c".repeat(32), arenaStreamId: "10" }, "http://localhost:7000/v1/rtc/offer");
  assert.equal(f.tracer.snapshot().bufferedFrames, 0); assert.equal(f.intervals.size, 1);
  f.tracer.frame(f.row); await f.tracer.flush();
  const report = JSON.parse(f.sent.find(request => request.method === "POST").body);
  assert.equal(report.scope.arenaStreamId, "10"); assert.equal(report.sequence, 1); assert.equal(report.frames[0][1], 0);
  f.active(false); assert.equal(f.tracer.requested(), false); f.tracer.clear();
});

test("failed uploads drop one frozen batch, keep bounded future collection and never retry it", async () => {
  const f = fixture(); await f.start(); f.fail(true); f.tracer.frame(f.row);
  assert.equal(await f.tracer.flush(), false); assert.equal(f.tracer.snapshot().failures, 1);
  assert.equal(f.tracer.snapshot().pending, false); assert.equal(f.tracer.snapshot().bufferedFrames, 0);
  f.fail(false); f.advance(20_000); f.tracer.frame(f.row); await f.tracer.flush();
  assert.deepEqual(f.sent.filter(request => request.method === "POST").map(request => JSON.parse(request.body).sequence), [1, 2]);
  f.tracer.clear();
});

test("a stale policy response cannot enable diagnostics after session cleanup", async () => {
  let answer;
  const tracer = ZubspaceClientDebug.create({ now: () => 0, visible: () => true, active: () => true, build: () => "dev",
    fetch: () => new Promise(resolve => { answer = resolve; }) });
  const start = tracer.setContext({ bundleId: "a", uploadToken: "b" }, "http://localhost:7000/v1/rtc/offer");
  tracer.clear(); answer({ ok: true, json: async () => ({ enabled: true, schemaVersion: 1, frameWidth: 34, intervalMs: 20000, maxFrames: 1200, maxBytes: 196608 }) });
  await start; assert.equal(tracer.requested(), false);
});

test("a browser failure event can upload even when no rendered frame follows it", async () => {
  const f=fixture(); await f.start(); f.tracer.event(3); await f.tracer.flush();
  const report=JSON.parse(f.sent.find(request=>request.method==="POST").body);
  assert.deepEqual(report.frames,[]); assert.deepEqual(report.events,[[3,0]]);
  f.tracer.clear();
});

test("a disabled advertised collector is never probed or enabled", async () => {
  const urls=[];
  const tracer=ZubspaceClientDebug.create({ now:()=>0, visible:()=>true, active:()=>true, build:()=>"dev",
    fetch:async url=>{urls.push(url);return {ok:true,json:async()=>({client_debug_trace:false})};} });
  await tracer.setContext({bundleId:"a",uploadToken:"b"},"http://localhost:7000/v1/rtc/offer");
  assert.deepEqual(urls,["http://localhost:7000/v1/status"]);assert.equal(tracer.requested(),false);tracer.clear();
});

test("muzzle evidence preserves exact headings and split identities, deduplicates and obeys policy", async () => {
  const old = fixture({ muzzleWidth: null }); await old.start();
  assert.equal(old.tracer.muzzleRequested(), false);
  assert.equal(old.tracer.muzzle(old.row), false); old.tracer.clear();
  const f = fixture(); await f.start();
  f.row[2] = 0xffffffff; f.row[3] = 0xfffffffe;
  f.row[17] = Math.fround(0.02499999); f.row[20] = Math.fround(0.5499999);
  const expected = [...f.row];
  assert.equal(f.tracer.muzzle(f.row), true);
  assert.equal(f.tracer.muzzleRequested(), false);
  f.advance(100);
  assert.equal(f.tracer.muzzle(f.row), false, "same owner/life/shot/child is not sampled twice in one window");
  f.row.fill(0);
  await f.tracer.flush();
  const report = JSON.parse(f.sent.find(r => r.method === "POST").body);
  assert.deepEqual(report.events, [[5, 0, ...expected]]);
  assert.deepEqual(report.frames, []);
  f.tracer.clear(); assert.equal(f.tracer.muzzleRequested(), false);
});

test("muzzle ring discloses evictions without consuming the browser failure budget", async () => {
  const f = fixture(); await f.start();
  for (let i = 0; i < 200; i++) {
    f.advance(100); f.row[4] = i;
    assert.equal(f.tracer.muzzle(f.row), true);
    f.tracer.event(1);
  }
  assert.equal(f.tracer.snapshot().bufferedMuzzles, 16);
  assert.equal(f.tracer.snapshot().droppedMuzzles, 184);
  await f.tracer.flush();
  const request = f.sent.find(r => r.method === "POST"), report = JSON.parse(request.body);
  assert.ok(Buffer.byteLength(request.body) <= 196608);
  assert.equal(report.events.length, 49);
  assert.equal(report.events.filter(r => r[0] === 1).length, 32);
  assert.equal(report.events.filter(r => r[0] === 5).length, 16);
  assert.deepEqual(report.events.at(-1), [6, 20000, 184]);
  assert.equal(f.tracer.snapshot().bufferedMuzzles, 0); f.tracer.clear();
});

test("malformed and hidden muzzle samples do not enter a later upload", async () => {
  const f = fixture(); await f.start();
  for (const bad of [new Float64Array(31), new Float64Array(32).fill(NaN), new Float64Array(32).fill(Infinity), new Float64Array(32).fill(1e13)]) {
    assert.equal(f.tracer.muzzle(bad), false);
  }
  assert.equal(f.tracer.muzzle(f.row), true);
  f.visible(false); await f.tracer.flush(); f.visible(true);
  assert.equal(f.tracer.snapshot().bufferedMuzzles, 0);
  assert.equal(f.sent.filter(r => r.method === "POST").length, 0); f.tracer.clear();
});
