import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";
import assert from "node:assert/strict";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
function recorder(enabled = true) {
  const start = html.indexOf("          function createQaFrameDiagnostics(");
  const end = html.indexOf("          function createQaStallObserver(", start);
  assert.ok(start >= 0 && end > start);
  const context = vm.createContext({ enabled });
  vm.runInContext(html.slice(start, end), context);
  return vm.runInContext("createQaFrameDiagnostics({enabled})", context);
}
const frame = (at, extra = {}) => ({ at, wallAt: at + 10000, scope: "session:arena",
  ready: true, visible: true, diagnostic: false, ordinal: at / 16,
  serverTick: at / 10, snapshotId: 10, presentationTick: at / 10 - 4,
  lifeSeq: 1, inputBits: 1, ...extra });

test("later stalls survive first capture and live-ring eviction with before/after evidence", () => {
  const r = recorder();
  for (let at = 0; at <= 3200; at += 16) r.observe(frame(at));
  r.observe(frame(3700));
  for (let at = 3716; at <= 6000; at += 16) r.observe(frame(at));
  r.observe(frame(6600));
  for (let at = 6616; at < 40000; at += 16) r.observe(frame(at));
  const s = r.snapshot();
  assert.equal(s.total, 2);
  assert.equal(s.incidents[0].durationMs, 500);
  assert.ok(s.incidents[1].durationMs >= 600);
  for (const i of s.incidents) {
    assert.equal(i.before.length, 24);
    assert.equal(i.after.length, 12);
    assert.equal(i.afterComplete, true);
    assert.ok(i.before.at(-1)[0] < i.at && i.after[0][0] > i.at);
  }
  assert.ok(s.recent[0][0] > 20000);
  const saved = JSON.stringify(s);
  r.observe(frame(41000));
  assert.equal(JSON.stringify(s), saved, "export is independent of later updates");
});
test("bounded history preserves the first and latest incidents and discloses eviction", () => {
  const r = recorder();
  r.observe(frame(0));
  for (let i = 1; i <= 100; i++) r.observe(frame(i * 500));
  for (let i = 0; i < 100; i++) r.work("longtask", i, 60, false);
  const s = r.snapshot();
  assert.equal(s.total, 100);
  assert.equal(s.incidents.length, 8);
  assert.equal(s.incidents[0].serial, 1);
  assert.equal(s.incidents.at(-1).serial, 100);
  assert.equal(s.evicted, 92);
  assert.equal(s.work.length, 64);
  assert.equal(s.workEvicted, 36);
  assert.ok(Buffer.byteLength(JSON.stringify(s)) < 64 * 1024);
});
test("visibility, scope, diagnostic and disabled intervals cannot invent incidents", () => {
  for (const excluded of [{visible:false}, {ready:false}, {diagnostic:true}]) {
    const r = recorder();
    r.observe(frame(0)); r.observe(frame(500, excluded)); r.observe(frame(1000));
    assert.equal(r.snapshot().total, 0);
    r.resetTiming(); r.observe(frame(2000, {scope:"new"}));
    assert.equal(r.snapshot().recent.length, 1);
  }
  const r = recorder(false);
  r.observe(frame(0)); r.observe(frame(500)); r.work("longtask", 0, 500, false);
  assert.equal(r.snapshot().total, 0);
  assert.equal(r.snapshot().work.length, 0);
});
test("smaller hitches and global work eviction cannot erase severe-stall attribution", () => {
  const r = recorder();
  r.observe(frame(0)); r.observe(frame(80));
  r.observe(frame(780));
  r.work("animation-callback", 80, 700, false);
  for (let i = 1; i <= 100; i++) {
    r.observe(frame(780 + i * 90));
    r.work("long-animation-frame", 780 + i * 90, 50, false);
  }
  const s = r.snapshot();
  const worst = s.incidents.find(i => i.durationMs === 700);
  assert.ok(worst);
  assert.equal(s.incidents[0].serial, 1);
  assert.equal(s.incidents.at(-1).serial, 102);
  assert.ok(worst.work.some(w => w[0] === "animation-callback" && w[2] === 700));
  assert.ok(!s.work.some(w => w[0] === "animation-callback"));
});
test("timing attribution retains scalar facts only and reports unavailable APIs", () => {
  const r = recorder();
  r.support("longtask", "unavailable");
  r.work("animation-callback", 100, 500, true);
  r.work("long-animation-frame", 90, 550, false, [300, 140, undefined]);
  const s = r.snapshot();
  assert.equal(s.support.longtask, "unavailable");
  assert.deepEqual(Array.from(s.work[0]), ["animation-callback",100,500,true]);
  assert.equal(s.work[1].at(-1), null);
});

// The publish loop is wrapped by the page's own requestAnimationFrame shim: enter before the
// callback body, observe() inside it, leave after. `at` is the monotonic publish stamp and
// [start, start+duration] is when the callback actually ran, which is the pair no incident-ring
// field can express - the ring only keeps work at or above 50ms.
function publishingCallback(r, { start, duration, at }) {
  const token = r.enterCallback(start);
  r.observe(frame(at));
  r.leaveCallback(token, start + duration);
}

test("cadence prices a slow frame's two halves and counts frames the incident ring drops", () => {
  const r = recorder();
  publishingCallback(r, { start: 1000, duration: 8, at: 1008 });
  // 30ms of scheduler wait before this one started, then only 12ms of work inside it.
  publishingCallback(r, { start: 1038, duration: 12, at: 1050 });
  // And one callback that ran long once it had started.
  publishingCallback(r, { start: 1053, duration: 40, at: 1093 });
  const { cadence } = r.snapshot();
  assert.equal(cadence.publishing.callbacks, 3, "a 40ms median frame must still be accounted");
  assert.equal(cadence.publishing.publishes, 3);
  assert.equal(cadence.publishing.durationMaxMs, 40, "the work half");
  assert.equal(cadence.publishing.latenessMaxMs, 30, "the scheduler half, kept separate from it");
  assert.equal(cadence.publishing.durationMeanMs, 20);
  // The sums ride along: a reader that wants the mean over a window has to subtract two
  // snapshots, and a mean cannot be subtracted.
  assert.equal(cadence.publishing.durationSumMs, 60);
  assert.equal(cadence.publishing.latenessSumMs, 33);
  assert.equal(cadence.publishing.latenessMeanMs, 11);
  assert.equal(cadence.other.callbacks, 0);
  // The snapshot is a JSON round-trip out of another realm, so compare a local copy.
  assert.deepEqual([...cadence.bucketBoundsMs], [1, 2, 5, 10, 20, 34, 50, 80, 120, 200, 400]);
  assert.equal(cadence.publishing.durationBuckets.reduce((a, b) => a + b, 0), 3);
  assert.equal(cadence.publishing.latenessBuckets.reduce((a, b) => a + b, 0), 3);
  // The same page records no incident work at all, because none of this reached 50ms: cadence is
  // the only witness for the common case. That is why this field exists.
  assert.equal(r.snapshot().incidents.length, 0);
});

test("cadence groups the publish loop apart from every other callback in the page", () => {
  const r = recorder();
  publishingCallback(r, { start: 500, duration: 6, at: 506 });
  // An instrument's own animation callback, sandwiched in the same frame, must not move the
  // game loop's numbers and must not erase the wait the game loop felt.
  const other = r.enterCallback(510);
  r.leaveCallback(other, 511);
  publishingCallback(r, { start: 540, duration: 7, at: 547 });
  const { cadence } = r.snapshot();
  assert.equal(cadence.other.callbacks, 1);
  assert.equal(cadence.other.publishes, 0, "a callback that published nothing is not the game loop");
  assert.equal(cadence.publishing.callbacks, 2);
  assert.equal(cadence.publishing.latenessMaxMs, 34, "measured publishing-callback to "
    + "publishing-callback, so the 511ms instrument end is not the reference point");
});

test("cadence resets with the timing base and ignores a disabled recorder", () => {
  const r = recorder();
  publishingCallback(r, { start: 100, duration: 4, at: 104 });
  assert.equal(r.snapshot().cadence.publishing.callbacks, 1);
  r.resetTiming();
  const cleared = r.snapshot().cadence;
  assert.equal(cleared.publishing.callbacks, 0);
  assert.equal(cleared.publishing.durationMaxMs, 0);
  assert.equal(cleared.publishing.latenessMaxMs, 0);
  assert.equal(recorder(false).enterCallback(1), null, "a disabled recorder allocates nothing");
  // A clock that moved backwards is not a measurement.
  const guarded = recorder();
  const token = guarded.enterCallback(200);
  guarded.leaveCallback(token, 150);
  assert.equal(guarded.snapshot().cadence.other.callbacks, 0);
});

// The recorder slice also defines the task witness, so one load exposes both.
function loaded() {
  const start = html.indexOf("          function createQaFrameDiagnostics(");
  const end = html.indexOf("          function createQaStallObserver(", start);
  assert.ok(start >= 0 && end > start);
  const context = vm.createContext({});
  vm.runInContext(html.slice(start, end), context);
  return vm.runInContext("({ createQaFrameDiagnostics, installQaTaskWitness })", context);
}

test("task accounting groups by kind and reports count, sum and worst", () => {
  const { createQaFrameDiagnostics } = loaded();
  const diagnostics = createQaFrameDiagnostics({ enabled: true });
  diagnostics.task("xhr-readystatechange", 100, 40);
  diagnostics.task("xhr-readystatechange", 200, 8);
  diagnostics.task("decodeAudioData", 300, 26);
  const snapshot = diagnostics.snapshot();
  assert.equal(snapshot.tasks["xhr-readystatechange"].count, 2);
  assert.equal(snapshot.tasks["xhr-readystatechange"].sumMs, 48);
  assert.equal(snapshot.tasks["xhr-readystatechange"].maxMs, 40);
  assert.equal(snapshot.tasks["xhr-readystatechange"].maxAtMs, 100);
  assert.equal(snapshot.taskFloorMs, 25, "the floor is published, not assumed by a harness");
});

test("a slow task keeps its span so a frame gap can ask what covered it", () => {
  const { createQaFrameDiagnostics } = loaded();
  const diagnostics = createQaFrameDiagnostics({ enabled: true });
  diagnostics.task("xhr-readystatechange", 1000, 60);
  diagnostics.task("xhr-readystatechange", 2000, 30);
  diagnostics.task("decodeAudioData", 3000, 24);
  // The recorder lives in its own vm realm, so compare through JSON rather than deep-equal objects
  // whose prototypes came from a different context.
  const plain = (value) => JSON.parse(JSON.stringify(value));
  assert.deepEqual(plain(diagnostics.tasksOverlapping(1010, 1050)),
    [{ kind: "xhr-readystatechange", atMs: 1000, durationMs: 60 }]);
  assert.deepEqual(plain(diagnostics.tasksOverlapping(1070, 1900)), [], "a gap with no task over it is empty");
  assert.deepEqual(plain(diagnostics.tasksOverlapping(2010, 2400)),
    [{ kind: "xhr-readystatechange", atMs: 2000, durationMs: 30 }]);
  assert.equal(diagnostics.snapshot().slowTasks.length, 2, "the under-floor task keeps no span");
});

test("the task witness times the asset loader's XHR handler including the work it causes", () => {
  const { createQaFrameDiagnostics, installQaTaskWitness } = loaded();
  const diagnostics = createQaFrameDiagnostics({ enabled: true });
  let nowMs = 0;
  class XMLHttpRequest {
    constructor() { this.readyState = 0; this.status = 0; }
    open() { this.readyState = 1; }
    send() {
      this.readyState = 4; this.status = 200;
      if (typeof this.onreadystatechange === "function") this.onreadystatechange();
    }
  }
  const globals = { window: { XMLHttpRequest }, now: () => nowMs };
  installQaTaskWitness(diagnostics, globals);
  const request = new XMLHttpRequest();
  request.open("GET", "chaos-zone.lvz");
  // What the real handler does: hand the bytes to the module and let it merge them there.
  request.onreadystatechange = () => { nowMs += 42; };
  request.send();
  const snapshot = diagnostics.snapshot();
  assert.equal(snapshot.tasks["xhr-readystatechange"].count, 1);
  assert.equal(snapshot.tasks["xhr-readystatechange"].sumMs, 42, "the handler's own work is timed");
  assert.equal(snapshot.slowTasks[0].kind, "xhr-readystatechange");
});

test("the task witness times module instantiation and its continuation without double wrapping", async () => {
  const { createQaFrameDiagnostics, installQaTaskWitness } = loaded();
  const diagnostics = createQaFrameDiagnostics({ enabled: true });
  let nowMs = 0;
  let calls = 0;
  const globals = {
    now: () => nowMs,
    window: { WebAssembly: { instantiate: async () => { calls += 1; nowMs += 20; return { instance: {} }; } } },
  };
  installQaTaskWitness(diagnostics, globals);
  installQaTaskWitness(diagnostics, globals);
  await globals.window.WebAssembly.instantiate(new ArrayBuffer(8));
  const snapshot = diagnostics.snapshot();
  assert.equal(snapshot.tasks["wasm-instantiate"].count, 1, "the synchronous call is timed once");
  assert.equal(snapshot.tasks["wasm-instantiate-ready"].count, 1, "the continuation is timed");
  assert.equal(calls, 1, "double installation does not double-wrap the original call");
});

test("non-publishing callbacks are grouped by the function that ran, capped, and reset with the window", () => {
  const { createQaFrameDiagnostics } = loaded();
  const diagnostics = createQaFrameDiagnostics({ enabled: true });
  const run = (label, at, durationMs) => {
    const token = diagnostics.enterCallback(at, label);
    diagnostics.leaveCallback(token, at + durationMs);
  };
  run("animation", 0, 20);
  run("animation", 100, 150);
  run("harness-sampler", 300, 5);
  const sources = diagnostics.snapshot().cadence.bySource;
  assert.equal(sources.animation.callbacks, 2);
  assert.equal(sources.animation.durationSumMs, 170);
  assert.equal(sources.animation.durationMaxMs, 150);
  assert.equal(sources.animation.maxAtMs, 100, "the worst call is locatable in time");
  assert.equal(sources["harness-sampler"].callbacks, 1);
  // A callback that published a frame is the game loop whatever its function is named, so it joins
  // `publishing` instead of growing the source map with its own name.
  const publishingToken = diagnostics.enterCallback(400, "animation");
  diagnostics.observe({ at: 410, wallAt: 10410, scope: "session:arena", ready: true, visible: true,
    diagnostic: false, ordinal: 30, serverTick: 4, snapshotId: 9, presentationTick: 1, lifeSeq: 1,
    inputBits: 1 });
  diagnostics.leaveCallback(publishingToken, 460);
  const after = diagnostics.snapshot().cadence.bySource;
  assert.equal(after.publishing.callbacks, 1, "a publishing callback is grouped as the game loop");
  assert.equal(after.animation.callbacks, 2, "and does not also count under its own name");
  diagnostics.resetTiming();
  assert.deepEqual(JSON.parse(JSON.stringify(diagnostics.snapshot().cadence.bySource)), {},
    "a new window starts clean");
});

test("the source map cannot grow without bound", () => {
  const { createQaFrameDiagnostics } = loaded();
  const diagnostics = createQaFrameDiagnostics({ enabled: true });
  for (let index = 0; index < 200; index += 1) {
    const token = diagnostics.enterCallback(index * 100, "label-" + index);
    diagnostics.leaveCallback(token, index * 100 + 10);
  }
  assert.ok(Object.keys(diagnostics.snapshot().cadence.bySource).length <= 24,
    "a page that builds a closure per frame cannot grow the map without bound");
});

test("resetTiming clears task accounting so window maxima exist, not just the cadence", () => {
  const { createQaFrameDiagnostics } = loaded();
  const diagnostics = createQaFrameDiagnostics({ enabled: true });
  diagnostics.task("xhr-readystatechange", 100, 40);
  diagnostics.task("decodeAudioData", 200, 30);
  const before = diagnostics.snapshot();
  assert.equal(before.tasks["xhr-readystatechange"].count, 1);
  assert.equal(before.slowTasks.length, 2);
  diagnostics.resetTiming();
  const after = diagnostics.snapshot();
  assert.deepEqual(JSON.parse(JSON.stringify(after.tasks)), {},
    "a window that inherits page-life task totals would report a cost that happened in an earlier window");
  assert.deepEqual(JSON.parse(JSON.stringify(after.slowTasks)), []);
  // And the window fills again from zero, so a max read after a reset is a window max.
  diagnostics.task("xhr-readystatechange", 5000, 60);
  assert.equal(diagnostics.snapshot().tasks["xhr-readystatechange"].maxMs, 60);
  assert.equal(diagnostics.snapshot().tasks["xhr-readystatechange"].count, 1);
});

test("a long callback carries the zone-merge counters on both sides of itself", () => {
  const { createQaFrameDiagnostics } = loaded();
  const diagnostics = createQaFrameDiagnostics({ enabled: true });
  assert.equal(diagnostics.snapshot().longFrameFloorMs, 50);
  // A frame that merged three archives and paid 101ms for them.
  diagnostics.longFrame(1000, 140,
    { lvzMergeCount: 11, lvzMergeTotalMs: 14.0, lvzMergeMaxMs: 6.0, zoneLvzObjects: 20000, zoneLvzTextures: 120 },
    { lvzMergeCount: 14, lvzMergeTotalMs: 115.0, lvzMergeMaxMs: 101.0, zoneLvzObjects: 28922, zoneLvzTextures: 151 },
    false);
  // A frame that cost the same and merged nothing.
  diagnostics.longFrame(5000, 130,
    { lvzMergeCount: 14, lvzMergeTotalMs: 115.0, lvzMergeMaxMs: 101.0, zoneLvzObjects: 28922, zoneLvzTextures: 151 },
    { lvzMergeCount: 14, lvzMergeTotalMs: 115.0, lvzMergeMaxMs: 101.0, zoneLvzObjects: 28922, zoneLvzTextures: 151 },
    false);
  const frames = diagnostics.snapshot().longFrames;
  assert.equal(frames.length, 2);
  assert.equal(frames[0].mergeCountGrowth, 3, "the frame reports the archives it merged");
  assert.equal(frames[0].mergeMsGrowth, 101, "and the merge milliseconds it paid");
  assert.equal(frames[0].objectsAfter, 28922);
  assert.equal(frames[1].mergeCountGrowth, 0, "a frame that merged nothing says so, which is an answer too");
  assert.equal(frames[1].mergeMsGrowth, 0);
  // Under the floor there is nothing to attribute, and the ring is bounded and keeps the first frames,
  // because the expensive frames are the boot frames and evicting them would evict the evidence.
  diagnostics.longFrame(9000, 49, null, null, false);
  assert.equal(diagnostics.snapshot().longFrames.length, 2);
  for (let index = 0; index < 40; index += 1) diagnostics.longFrame(10000 + index, 60, null, null, false);
  const capped = diagnostics.snapshot().longFrames;
  assert.equal(capped.length, 16);
  assert.equal(capped[0].atMs, 1000, "the earliest long frames survive; later ones are dropped");
  assert.equal(capped[15].atMs, 10013, "and the ring stops taking new ones instead of evicting old");
});

test("the page reads the zone-merge counters from the payload that actually carries them", async () => {
  const { readFile } = await import("node:fs/promises");
  const page = await readFile(new URL("../index.html", import.meta.url), "utf8");
  const helper = /const zoneAccounting = \(\) => \{[\s\S]*?\n            \};/.exec(page)?.[0] ?? "";
  assert.match(helper, /localGameplayState/,
    "lvzMergeCount lives under localGameplayState; reading the payload root yields a null-filled ring");
  assert.match(helper, /Number\.isFinite\(state\.lvzMergeCount\)/,
    "and the helper refuses a payload that does not carry the counters instead of reporting zeros");
});

test("the task witness times the doors that are not the animation callback", () => {
  const { createQaFrameDiagnostics, installQaTaskWitness } = loaded();
  const diagnostics = createQaFrameDiagnostics({ enabled: true });
  let nowMs = 0;
  const timers = [];
  class EventTarget {
    constructor() { this.registered = []; }
    addEventListener(type, handler, options) { this.registered.push({ type, handler, options }); }
    removeEventListener(type, handler) {
      const index = this.registered.findIndex((e) => e.type === type && e.handler === handler);
      if (index >= 0) this.registered.splice(index, 1);
    }
    dispatch(type) { for (const entry of [...this.registered]) if (entry.type === type) entry.handler({ type }); }
  }
  const window = {
    EventTarget,
    setTimeout: (fn) => { timers.push(fn); return timers.length; },
    setInterval: (fn) => { timers.push(fn); return -timers.length; },
  };
  installQaTaskWitness(diagnostics, { window, now: () => nowMs });

  // The shape the join freeze has: a callback that holds the thread for over half a second, arriving
  // through a door the animation-callback ring never sees.
  window.setTimeout(() => { nowMs += 568; }, 0);
  timers.shift()();
  window.setInterval(() => { nowMs += 30; }, 16);
  timers.shift()();

  const target = new EventTarget();
  const first = () => { nowMs += 90; };
  const second = () => { nowMs += 5; };
  target.addEventListener("keydown", first);
  target.addEventListener("keydown", second); // a different function on the same type must still fire
  target.addEventListener("keydown", first);   // the same function again is discarded, as browsers do
  target.dispatch("keydown");
  target.removeEventListener("keydown", first);
  target.dispatch("keydown");
  target.addEventListener("keyup", () => { nowMs += 7; }, { once: true });

  const snapshot = diagnostics.snapshot();
  assert.equal(snapshot.tasks.timer.count, 1);
  assert.equal(snapshot.tasks.timer.maxMs, 568, "a timer that holds the thread is timed as its own cost");
  assert.equal(snapshot.tasks.interval.count, 1);
  assert.equal(snapshot.tasks["listener-keydown"].count, 3, "2 listeners fire, then only the survivor");
  assert.equal(snapshot.tasks["listener-keydown"].sumMs, 100);
  assert.equal(target.registered.filter((entry) => entry.type === "keydown").length, 1,
    "removal found what was registered, and the duplicate never landed");
  assert.equal(snapshot.tasks["listener-self-retiring"].count, 1,
    "a once/signal registration is passed through unwrapped, and says so");
  const keyup = target.registered.find((entry) => entry.type === "keyup");
  assert.equal(typeof keyup.handler, "function");
  assert.notEqual(keyup.handler.__qaTaskWitness, true, "passed through untouched, not wrapped and left stale");
  const slow = snapshot.slowTasks.map((entry) => `${entry.kind}:${entry.durationMs}`);
  // Compared as JSON: the recorder runs in a vm context, and a cross-realm container compares oddly
  // against a literal even when every element matches, which is a test-harness fact rather than a
  // finding about the instrument.
  assert.equal(JSON.stringify(slow), JSON.stringify(["timer:568", "interval:30", "listener-keydown:90"]),
    "in the order they ran, each priced on its own execution time");
});

test("the task witness prices the GL surface and says whether it got on it", () => {
  const { createQaFrameDiagnostics, installQaTaskWitness } = loaded();
  const diagnostics = createQaFrameDiagnostics({ enabled: true });
  let nowMs = 0;
  class WebGLRenderingContext {
    compileShader() { nowMs += 240; }   // deferred translation and compile, on the calling thread
    linkProgram() { nowMs += 180; }
    texImage2D() { nowMs += 60; }
    drawArrays() { nowMs += 0.4; }
  }
  class HTMLCanvasElement { }
  HTMLCanvasElement.prototype.getContext = function (kind) {
    return kind === "webgl" ? new WebGLRenderingContext() : null;
  };
  installQaTaskWitness(diagnostics, { window: { HTMLCanvasElement }, now: () => nowMs });

  // A context taken the way the loader takes one, AFTER the witness is installed.
  const context = Object.getPrototypeOf(new HTMLCanvasElement()).getContext.call({ }, "webgl");
  context.compileShader();
  context.linkProgram();
  context.texImage2D();
  for (let index = 0; index < 100; index += 1) context.drawArrays();

  const { tasks, slowTasks } = diagnostics.snapshot();
  assert.equal(tasks["gl-surface-wrapped"].count, 1,
    "the artifact records that the witness was on the surface at all");
  assert.equal(tasks["gl-compileShader"].maxMs, 240);
  assert.equal(tasks["gl-linkProgram"].maxMs, 180);
  assert.equal(tasks["gl-texImage2D"].sumMs, 60);
  assert.equal(tasks["gl-drawArrays-calls"].count, 100);
  assert.equal(Math.round(tasks["gl-drawArrays-calls"].sumMs), 40,
    "draws are priced as a sum; no single draw is the stall");
  assert.deepEqual(JSON.parse(JSON.stringify(slowTasks.map((entry) => entry.kind))),
    ["gl-compileShader", "gl-linkProgram", "gl-texImage2D"]);

  // A second getContext on the same context must not double-wrap it.
  context.compileShader();
  assert.equal(diagnostics.snapshot().tasks["gl-surface-wrapped"].count, 1);
  assert.equal(diagnostics.snapshot().tasks["gl-compileShader"].count, 2);
});

test("published cadence means carry microsecond precision, so a trace can be compared to itself", () => {
  const r = recorder();
  // Callbacks whose mean cannot be represented in a binary double.
  publishingCallback(r, { start: 1000, duration: 22 / 3, at: 1000 + 22 / 3 });
  publishingCallback(r, { start: 1040, duration: 26 / 3, at: 1040 + 26 / 3 });
  publishingCallback(r, { start: 1080, duration: 9.1, at: 1080 + 9.1 });
  const { cadence } = r.snapshot();
  const group = cadence.publishing;
  assert.ok(group.callbacks === 3);
  const digits = (value) => String(value).split(".")[1]?.length ?? 0;
  assert.ok(digits(group.durationMeanMs) <= 3,
    `durationMeanMs must not carry double noise, got ${group.durationMeanMs}`);
  assert.ok(digits(group.latenessMeanMs) <= 3,
    `latenessMeanMs must not carry double noise, got ${group.latenessMeanMs}`);
  // The sums stay fractional on purpose. What is pinned is the published precision, not a claim about
  // how the two copies of a trace are produced: the drill compares a saved browser bundle against the
  // copy retrieved from the server, and the origin of their one-ulp disagreement was not established,
  // so this test guards the observable property rather than a story about the cause.
});

test("the page reads inputBits only from localRenderPrediction, where the client publishes it", () => {
  // One documented source, not a dead-field repair. Both metric blocks carry inputBits - local_render_prediction
  // at src/game/render/hud/metrics.rs:71 and local_gameplay_state at metrics.rs:477 - so reads from either have
  // returned real values, and a 400-file sample of retained QA traces confirms it (161,024 entries with the key,
  // zero nulls, values 0/1/4). The guard exists because the two blocks are assembled by different format strings:
  // whichever block a consumer reads is the one that can silently stop carrying the field, so every consumer
  // reads the block the payload documents first.
  const wrong = [];
  html.split("\n").forEach((line, index) => {
    if (line.includes("localGameplayState?.inputBits")) wrong.push(index + 1);
  });
  assert.deepEqual(wrong, [], `inputBits read from an object that never carries it, at lines ${wrong.join(", ")}`);
  const correct = html.split("\n").filter((line) => line.includes("localRenderPrediction?.inputBits")).length;
  assert.ok(correct >= 2, `expected the HUD payload and the QA trace entry both to read it, found ${correct}`);
});

test("the always-on lifecycle channel publishes readiness without building the metrics block", async () => {
  // Requirement: an automated driver must be able to order on readiness on the shipped configuration,
  // where nobody asked for diagnostics. Constraint (tests/rtc_config.test.mjs): ordinary play must not
  // build the full presentation payload, which walks remote state and measures text every requested frame.
  // The reconciliation is a separate channel that carries a couple of scalars and publishes on change.
  const html = fs.readFileSync("index.html", "utf8");
  const metrics = fs.readFileSync("src/game/render/hud/metrics.rs", "utf8");

  const importStart = html.indexOf("importObject.env.zub_lifecycle_state =");
  assert.notEqual(importStart, -1, "index.html must define the lifecycle import");
  const importBody = html.slice(importStart, html.indexOf("};", importStart));
  assert.match(importBody, /window\.__zubspaceLifecycle/, "the lifecycle channel must reach the page");
  assert.doesNotMatch(importBody, /traceEnabled|metricsEnabled/,
    "the lifecycle channel must not be gated behind diagnostics; an unflagged page has to expose readiness");

  // The full block must stay opt-in, and the lifecycle publish must sit ahead of that gate.
  assert.match(metrics, /if !ui_metrics_requested\(\) \{\s*return;/,
    "the full metrics block must still be gated on an explicit request");
  const lifecycleAt = metrics.indexOf("PUBLISHED_LIFECYCLE_STATE.with");
  const gateAt = metrics.indexOf("if !ui_metrics_requested()");
  assert.notEqual(lifecycleAt, -1, "metrics.rs must publish lifecycle state");
  assert.ok(lifecycleAt < gateAt, "lifecycle must be published before the metrics block is skipped");
  assert.match(metrics, /thread_local! \{\s*static PUBLISHED_LIFECYCLE_STATE/,
    "the last published value must be remembered so a steady frame publishes nothing");

  // inGameplay must be the harness's own condition, not a weaker stand-in for it: a life sequence assigned
  // and a server-published energy received. If either source drifts, the channel would report gameplay
  // that the harness would not accept, and a driver ordering on it would act one frame early.
  assert.match(metrics, /authority_scope\.scope\.life_seq\.is_some\(\) && self\.last_self_server_energy\.is_some\(\)/,
    "inGameplay must be exactly the condition the browser harness tests");
  assert.ok(metrics.includes("inGameplay\\\""),
    "the published state must carry the gameplay flag in the JSON it formats");
});
