import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";
import assert from "node:assert/strict";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
function observer(freeze, options = {}) {
  const start = html.indexOf("          function createQaStallObserver(");
  const end = html.indexOf("          function downloadQaTrace(", start);
  assert.ok(start >= 0 && end > start, "production stall observer exists");
  const context = vm.createContext({ freeze, options, performance });
  vm.runInContext(html.slice(start, end), context);
  return vm.runInContext("createQaStallObserver(freeze, options)", context);
}
const frame = (at, extra = {}) => ({ at, wallAt: 10000 + at,
  scope: "server:arena:pilot", lifeSeq: 1, ready: true, visible: true,
  arenaLive: true, diagnostic: false, ...extra });

function encode(bundle, maximum) {
  const start = html.indexOf("          function encodeQaStallBundle(");
  const end = html.indexOf("          function downloadQaTrace(", start);
  const context = vm.createContext({ bundle, maximum, TextEncoder });
  vm.runInContext(html.slice(start, end), context);
  return vm.runInContext("encodeQaStallBundle(bundle, maximum)", context);
}
test("saved bytes are independent of later mutable damage witnesses", () => {
  const bundle = { rings: { damage: [{ energy: 100 }] } };
  const saved = encode(bundle, 1024);
  bundle.rings.damage[0].energy = 900;
  bundle.rings.damage.length = 0;
  assert.equal(JSON.parse(saved).rings.damage[0].energy, 100);
});
test("immutable capture enforces the UTF-8 byte limit, including exact boundary", () => {
  const bundle = { label: "🚀é" };
  const size = Buffer.byteLength(JSON.stringify(bundle));
  assert.equal(encode(bundle, size), JSON.stringify(bundle));
  assert.throws(() => encode(bundle, size - 1), /byte budget/);
});

test("first 80ms gameplay gap freezes once with both original endpoints", () => {
  const calls = [];
  const o = observer(marker => { calls.push(marker); return { saved: true }; });
  o.observe(frame(0)); o.observe(frame(79)); o.observe(frame(159));
  o.observe(frame(900));
  assert.equal(calls.length, 1);
  assert.equal(calls[0].previous.at, 79);
  assert.equal(calls[0].current.at, 159);
  assert.equal(calls[0].durationMs, 80);
  assert.equal(o.snapshot().phase, "captured");
});
test("hidden, unready and explicit diagnostic intervals reset the baseline", () => {
  for (const excluded of [{ visible: false }, { ready: false }, { diagnostic: true },
    { arenaLive: false }, { arenaLive: undefined }]) {
    let captures = 0;
    const o = observer(() => { captures++; });
    o.observe(frame(0)); o.observe(frame(500, excluded)); o.observe(frame(1000));
    o.observe(frame(1016));
    assert.equal(captures, 0);
  }
});
test("zone loading gaps are not gameplay stalls and the live arena resumes monitoring", () => {
  // The shipped client merges each arriving authored archive on the render path
  // while gameplay holds until the whole zone publishes, so the load phase
  // legitimately contains frame-scale gaps - a measured 159.4 ms Chaos merge
  // hitch was being frozen as the first *gameplay* frame gap. Such an interval
  // must neither consume the single capture nor leave the observer blind: the
  // reset also drops the interval that spans the boundary, and monitoring
  // resumes on the first frame inside the arena.
  const calls = [];
  const o = observer((marker) => { calls.push(marker); return { saved: true }; });
  o.observe(frame(0, { arenaLive: false }));
  o.observe(frame(160, { arenaLive: false }));
  o.observe(frame(1_000));
  assert.equal(calls.length, 0, "a merge hitch and the boundary interval are not stalls");
  o.observe(frame(1_080));
  assert.equal(calls.length, 1);
  assert.equal(calls[0].previous.at, 1_000);
  assert.equal(calls[0].current.at, 1_080);
  assert.equal(calls[0].durationMs, 80);
  assert.equal(o.snapshot().phase, "captured");
});
test("disabled tracing cannot invoke capture", () => {
  const o = observer(() => { throw Error("must not run"); }, { enabled: false });
  o.observe(frame(0)); o.observe(frame(500));
  assert.equal(o.snapshot().phase, "disabled");
});
test("scope changes and explicit visibility resets do not invent stalls", () => {
  let captures = 0;
  const o = observer(() => { captures++; });
  o.observe(frame(0)); o.observe(frame(500, { scope: "new" }));
  o.resetTiming(); o.observe(frame(1000, { scope: "new" }));
  assert.equal(captures, 0);
});
test("guard precedes capture so reentrancy and failure cannot recurse", () => {
  let count = 0, o;
  o = observer(() => { count++; o.observe(frame(1000)); throw Error("capture failed"); });
  o.observe(frame(0)); assert.doesNotThrow(() => o.observe(frame(500)));
  o.observe(frame(1500));
  assert.equal(count, 1);
  assert.equal(o.snapshot().phase, "failed");
  assert.equal(o.snapshot().error, "capture failed");
});
test("capture retains original scope across later joins and rejects malformed times", () => {
  let scope;
  const o = observer(marker => { scope = marker.current.scope; });
  o.observe(frame(NaN)); o.observe(frame(0)); o.observe(frame(-1));
  o.observe(frame(100)); o.observe(frame(600));
  o.observe(frame(1200, { scope: "new" }));
  assert.equal(scope, "server:arena:pilot");
  assert.equal(o.snapshot().phase, "captured");
});
