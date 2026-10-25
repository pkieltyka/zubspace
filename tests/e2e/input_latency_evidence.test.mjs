import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import fs from "node:fs";
import { INPUT_LATENCY_CAPTURE, assertNextDrawPrediction, assertDrawCadence, assertHeldDrawCadence } from "./input_latency_evidence.mjs";

test("retained Firefox consecutive draws with equal clock readings remain insufficient timing proof", () => {
  const capture = JSON.parse(fs.readFileSync("tests/fixtures/firefox_high_refresh_clock_8137398.json"));
  const press = capture.inputs[0];
  assert.equal(capture.frames.length, 23);
  assert.ok(capture.frames.every((frame, index) => frame.renderOrdinal === 552 + index));
  const duplicate = capture.frames.filter(frame => frame.monotonicMs === 7468);
  assert.deepEqual(duplicate.map(frame => frame.renderOrdinal), [567, 568]);
  assert.throws(() => assertHeldDrawCadence(capture, press, 552), /invalid clock order/);
});

test("only the disposable Firefox high-refresh profile requests a precise measurement clock", async () => {
  const source = fs.readFileSync("tools/browser_firefox_rtc_smoke.mjs", "utf8");
  const start = source.indexOf("async function createSession(driver)");
  const end = source.indexOf("\nasync function deleteSession(", start);
  assert.ok(start >= 0 && end > start);
  for (const highRefresh of [undefined, "0", "1"]) {
    let request;
    const create = vm.runInNewContext(`${source.slice(start, end)};createSession`, {
      webdriver: async (_driver, method, pathname, body) => {
        assert.equal(method, "POST"); assert.equal(pathname, "/session");
        request = body;
        return { sessionId: "native-profile-control", capabilities: {} };
      },
      process: { env: { ZUBSPACE_E2E_HIGH_REFRESH: highRefresh } },
      firefoxBin: "firefox", firefoxBrowserEnv: () => null,
      firefoxHeadless: false, e2eWindowClass: "zubspace-e2e",
      openFirefoxConsole: async () => null,
    });
    const driver = {};
    await create(driver);
    const prefs = request.capabilities.alwaysMatch["moz:firefoxOptions"].prefs;
    assert.equal(driver.sessionId, "native-profile-control");
    assert.equal(prefs["layout.frame_rate"], highRefresh === "1" ? 0 : undefined);
    assert.equal(prefs["privacy.reduceTimerPrecision"], highRefresh === "1" ? false : undefined);
  }
});

test("held-key cadence separates retained pre-press controller time from physical input", () => {
  const frames = Array.from({ length: 24 }, (_, i) => ({ renderOrdinal: 647 + i,
    monotonicMs: 9941 + i * 9 + (i >= 3 ? 66 : 0) }));
  const press = { serial: 9, kind: "keydown", code: "ArrowUp", playerLifeSeq: 1,
    renderOrdinal: 651, monotonicMs: frames[4].monotonicMs + 2 };
  const release = { ...press, serial: 10, kind: "keyup", renderOrdinal: 669,
    monotonicMs: frames[22].monotonicMs + 2 };
  assert.throws(() => assertDrawCadence(frames, 647, 90), /not realized/);
  const proof = assertHeldDrawCadence({ frames, inputs: [press, release] }, press, 647);
  assert.ok(proof.measuredHz > 110);
  assert.equal(proof.captureWindow[3].monotonicMs - proof.captureWindow[2].monotonicMs, 75);
  const slow = frames.map(frame => ({ ...frame,
    monotonicMs: frame.monotonicMs + (frame.renderOrdinal >= 660 ? 100 : 0) }));
  assert.throws(() => assertHeldDrawCadence({ frames: slow, inputs: [press, { ...release, monotonicMs: release.monotonicMs + 100 }] }, press, 647), /not realized/);
  assert.throws(() => assertHeldDrawCadence({ frames, inputs: [press] }, press, 647), /physical release/);
  assert.throws(() => assertHeldDrawCadence({ frames, inputs: [press, { ...release,
    monotonicMs: release.monotonicMs + 100 }] }, press, 647), /physical hold cadence not realized/);
});

test("high-refresh evidence measures all actual consecutive draw intervals", () => {
  const frames = Array.from({ length: 100 }, (_, i) => ({ renderOrdinal: i, monotonicMs: i * 1000 / 144 }));
  const proof = assertDrawCadence(frames, 0, 90);
  assert.ok(Math.abs(proof.measuredHz - 144) < 1e-8);
  assert.equal(proof.intervals.length, 99);
  for (const bad of [
    frames.slice(1), frames.filter((_, i) => i !== 40), frames.slice(0, 10),
    frames.map(frame => ({ ...frame, monotonicMs: frame.renderOrdinal * 1000 / 60 })),
    frames.map(frame => ({ ...frame, monotonicMs: 0 })),
  ]) assert.throws(() => assertDrawCadence(bad, 0, 90));
  const stalled = frames.map(frame => ({ ...frame,
    monotonicMs: frame.monotonicMs + (frame.renderOrdinal >= 50 ? 100 : 0) }));
  const withStall = assertDrawCadence(stalled, 0, 90);
  assert.ok(withStall.maxMs > 100, "do not discard inconvenient frame gaps");
  assert.ok(withStall.measuredHz < proof.measuredHz);
});

function withDraw(value) {
  for (const record of [...value.inputs, ...value.frames]) {
    record.localRenderPrediction = { orientation: record.playerOrientation,
      velocityX: record.playerVelocityX, velocityY: record.playerVelocityY,
      inputBits: record.localInputBits, partialSeconds: 0 };
  }
  return value;
}

test("capture cursor excludes controller annotations interleaved with physical events", () => {
  const capture = vm.runInNewContext(INPUT_LATENCY_CAPTURE, { window: {
    __zubspaceInputTrace: [{ serial: 6, kind: "keyup" }, { phase: "keyUp" }],
    __zubspaceMetricsTrace: [],
  } });
  assert.equal(capture.inputs.length, 1);
  assert.equal(capture.inputs.at(-1).serial, 6);
});

function fixture() {
  return { inputs: [{ serial: 1, kind: "keydown", code: "ArrowLeft", renderOrdinal: 10,
    monotonicMs: 100, playerLifeSeq: 3, clientInputSeq: 65535, playerOrientation: 0.5 }],
  frames: [{ renderOrdinal: 11, monotonicMs: 109, playerLifeSeq: 3, playerAlive: true,
    serverPlayerAlive: true, clientInputSeq: 0, localInputBits: 4, playerOrientation: 0.49 }] };
}

test("next-draw prediction measures a genuine event/frame pair across input wrap", () => {
  const proof = assertNextDrawPrediction(withDraw(fixture()), 0, ["ArrowLeft"]);
  assert.equal(proof[0].latencyMs, 9);
  assert.equal(proof[0].sequenceAdvance, 1);
});

test("retained Chrome zero-step frame fails despite rotation on the following draw", () => {
  // Compact fields from input-latency/chrome-chaos-failure.
  // The old trace omitted frame life; pin it to isolate the timing violation.
  const evidence = { inputs: [{ serial: 1, kind: "keydown", code: "ArrowLeft",
    renderOrdinal: 99, monotonicMs: 6629.1, playerLifeSeq: 1,
    clientInputSeq: 17307, playerOrientation: 0 }], frames: [
    { renderOrdinal: 100, monotonicMs: 6632.4, playerLifeSeq: 1,
      playerAlive: true, serverPlayerAlive: true, clientInputSeq: 17307,
      localInputBits: 0, playerOrientation: 0 },
    { renderOrdinal: 101, monotonicMs: 6648.8, playerLifeSeq: 1,
      playerAlive: true, serverPlayerAlive: true, clientInputSeq: 17308,
      localInputBits: 4, playerOrientation: 0.99 },
  ] };
  assert.throws(() => assertNextDrawPrediction(withDraw(evidence), 0, ["ArrowLeft"]),
    /did not consume the held key/);
});

test("next-draw proof does not accept a later successful frame or a key latch without prediction", () => {
  for (const mutate of [
    x => { x.frames[0].renderOrdinal = 12; },
    x => { x.frames[0].localInputBits = 0; },
    x => { x.frames[0].clientInputSeq = 65535; },
    x => { x.frames[0].playerOrientation = 0.5; },
    x => { x.frames[0].playerOrientation = 0.51; },
    x => { x.frames[0].playerLifeSeq = 4; },
    x => { x.frames[0].playerAlive = false; },
    x => { x.frames[0].monotonicMs = 99; },
    x => { x.inputs = []; },
  ]) {
    const value = fixture();
    mutate(value);
    assert.throws(() => assertNextDrawPrediction(withDraw(value), 0, ["ArrowLeft"]));
  }
});

test("actual next-draw fractional prediction needs no fabricated canonical step", () => {
  const value = withDraw(fixture());
  value.frames[0].clientInputSeq = 65535;
  value.frames[0].localRenderPrediction.partialSeconds = 0.0042;
  assert.equal(assertNextDrawPrediction(value, 0, ["ArrowLeft"])[0].sequenceAdvance, 0);
  value.frames[0].localRenderPrediction.orientation = 0.5;
  assert.throws(() => assertNextDrawPrediction(value, 0, ["ArrowLeft"]), /requested rotation/);
  value.frames[0].localRenderPrediction.partialSeconds = 0.1;
  assert.throws(() => assertNextDrawPrediction(value, 0, ["ArrowLeft"]), /unbounded/);
});

function thrustFixture(speed) {
  return withDraw({
    inputs: [{ serial: 1, kind: "keydown", code: "ArrowUp", renderOrdinal: 10,
      monotonicMs: 100, playerLifeSeq: 1, clientInputSeq: 200,
      playerVelocityX: 0, playerVelocityY: -speed }],
    frames: [{ renderOrdinal: 11, monotonicMs: 108, playerLifeSeq: 1, playerAlive: true,
      serverPlayerAlive: true, clientInputSeq: 201, localInputBits: 1,
      playerVelocityX: 0, playerVelocityY: -speed }],
  });
}

test("terminal-speed thrust needs no velocity change on the next draw", () => {
  // Extreme keyboard pilot: pinned at 345 px/s with the thrust bit consumed, so
  // the clamped prediction is identical one partial step later.
  const proof = assertNextDrawPrediction(thrustFixture(345), 0, ["ArrowUp"]);
  assert.equal(proof[0].saturatedByClamp, true);
  assert.equal(proof[0].latencyMs, 8);
});

test("a stalled prediction below the run's top speed still fails", () => {
  const value = thrustFixture(345);
  const faster = { renderOrdinal: 12, monotonicMs: 116, playerLifeSeq: 1,
    playerAlive: true, serverPlayerAlive: true, clientInputSeq: 202, localInputBits: 1,
    playerVelocityX: 0, playerVelocityY: -400 };
  faster.localRenderPrediction = { orientation: 0, velocityX: 0, velocityY: -400,
    inputBits: 1, partialSeconds: 0 };
  value.frames.push(faster);
  assert.throws(() => assertNextDrawPrediction(value, 0, ["ArrowUp"]),
    /did not predict thrust/);
});
