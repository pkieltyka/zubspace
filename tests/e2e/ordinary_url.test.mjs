import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { installOrdinaryUrlCapture, ordinaryUrlEvidence } from "./ordinary_url.mjs";

function fixture({ search = "?arena=chaos-zone", hud = () => 0, sprite = () => 0 } = {}) {
  let now = 0;
  let callback;
  const env = { zub_ui_metrics_requested: hud, zub_remote_sprite_trace_requested: sprite };
  const context = vm.createContext({
    window: { location: { search }, importObject: { env } },
    URL, URLSearchParams,
    performance: { now: () => now, getEntriesByType: () => [] },
    requestAnimationFrame: (next) => { callback = next; },
  });
  return {
    context, env,
    install: () => vm.runInContext(`(${installOrdinaryUrlCapture.toString()})(1000)`, context),
    frame(at) { now = at; callback(); },
    finish() { for (let at = 10; at <= 1000; at += 10) this.frame(at); },
    get capture() { return context.window.__zubspaceOrdinaryCapture; },
  };
}

test("ordinary capture observes unchanged request gates without enabling diagnostic payloads", () => {
  const state = fixture();
  const hud = state.env.zub_ui_metrics_requested;
  const sprite = state.env.zub_remote_sprite_trace_requested;
  state.install();
  state.finish();
  assert.equal(ordinaryUrlEvidence(state.capture).ok, true);
  assert.equal(state.capture.gateCalls, state.capture.frames);
  assert.equal(state.env.zub_ui_metrics_requested, hud);
  assert.equal(state.env.zub_remote_sprite_trace_requested, sprite);
  assert.equal(state.context.window.__zubspaceUiMetrics, undefined);
  assert.equal(state.context.window.__zubspaceRemoteSpriteTrace, undefined);
});

test("ordinary capture rejects either enabled path, retained diagnostics, and material frame gaps", () => {
  for (const options of [{ hud: () => 1 }, { sprite: () => 1 }]) {
    const state = fixture(options);
    state.install();
    state.finish();
    assert.equal(ordinaryUrlEvidence(state.capture).ok, false);
    assert.equal(state.capture.failures.length, 16, "failure evidence is bounded");
  }
  const payload = fixture();
  payload.install();
  payload.context.window.__zubspaceUiMetrics = {};
  payload.finish();
  assert.equal(ordinaryUrlEvidence(payload.capture).ok, false);
  const gap = fixture();
  gap.install();
  gap.frame(10);
  for (let at = 100; at <= 1000; at += 10) gap.frame(at);
  assert.equal(gap.capture.frameGapCount, 1);
  assert.equal(ordinaryUrlEvidence(gap.capture).ok, false);
  assert.equal(ordinaryUrlEvidence({ ...gap.capture, endedAt: null }).ok, false);
});

test("ordinary capture fails closed for diagnostic URLs or missing real WASM gates", () => {
  for (const option of ["trace=1", "metrics=1", "packetlog=1", "trace=0"]) {
    assert.throws(() => fixture({ search: `?${option}` }).install(), /omit every diagnostic/);
  }
  const state = fixture();
  delete state.env.zub_ui_metrics_requested;
  assert.throws(() => state.install(), /real loaded WASM/);
});

test("ordinary capture includes stalls before its first frame and at its closing frame", () => {
  const first = fixture();
  first.install();
  for (let at = 90; at <= 1000; at += 10) first.frame(at);
  assert.equal(first.capture.maximumFrameGapMs, 90);
  assert.equal(ordinaryUrlEvidence(first.capture).ok, false);

  const last = fixture();
  last.install();
  for (let at = 10; at <= 910; at += 10) last.frame(at);
  last.frame(1000);
  assert.equal(last.capture.maximumFrameGapMs, 90);
  assert.equal(ordinaryUrlEvidence(last.capture).ok, false);
});
