import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";
import { execFileSync } from "node:child_process";

test("browser dev builds optimize without disabling safety checks or changing native/release profiles", () => {
  const dryRun = (target, ...args) => execFileSync("make", ["--no-print-directory", "-n", target,
    "WASM_LINKER=/test/wasm-ld", ...args], { encoding: "utf8" });
  const normal = dryRun("build-client");
  assert.match(normal, /CARGO_PROFILE_DEV_OPT_LEVEL="1"/);
  assert.match(normal, /CARGO_PROFILE_DEV_DEBUG_ASSERTIONS=true CARGO_PROFILE_DEV_OVERFLOW_CHECKS=true/);
  assert.match(normal, /cargo build --target wasm32-unknown-unknown --bin zubspace/);
  assert.doesNotMatch(normal, /--release/);
  assert.match(dryRun("build-client", "WASM_DEV_OPT_LEVEL=0"), /CARGO_PROFILE_DEV_OPT_LEVEL="0"/);
  assert.doesNotMatch(dryRun("build-server"), /CARGO_PROFILE_DEV_/);
  assert.doesNotMatch(dryRun("build-client-release"), /CARGO_PROFILE_DEV_/);
});

const context = vm.createContext({ URL, URLSearchParams });
vm.runInContext(fs.readFileSync("js/zubspace_rtc_config.js", "utf8"), context);
const rtcConfiguration = context.zubspaceRtcConfiguration;
const signalingHeaders = context.zubspaceSignalingHeaders;
const rtcIceGatheringTimeoutMs = context.zubspaceRtcIceGatheringTimeoutMs;
const rtcSignalingTimeoutMs = context.zubspaceRtcSignalingTimeoutMs;
const waitForIceGatheringComplete = context.zubspaceWaitForIceGatheringComplete;
const createRtcSignalingDeadline = context.zubspaceCreateRtcSignalingDeadline;
const createRtcSignalingAttemptRegistry =
  context.zubspaceCreateRtcSignalingAttemptRegistry;
const createRtcLifecycleController =
  context.zubspaceCreateRtcLifecycleController;
const rtcTransportIsTerminal = context.zubspaceRtcTransportIsTerminal;
const networkProfile = context.zubspaceNetworkProfile;
const networkProfileDecision = context.zubspaceNetworkProfileDecision;
const orderedNetworkProfileSchedule = context.zubspaceOrderedNetworkProfileSchedule;
const iceUrlFixtures = JSON.parse(
  fs.readFileSync("tests/fixtures/rtc_ice_urls.json", "utf8"),
);
const browserEntry = fs.readFileSync("index.html", "utf8");

test("actual browser trace recorder preserves separate runtime and prepared weapon clocks", () => {
  const start = browserEntry.indexOf("                const traceEntry = {");
  const end = browserEntry.indexOf("\n                };", start);
  assert.ok(start >= 0 && end > start);
  const record = metrics => vm.runInNewContext(
    `${browserEntry.slice(start, end + "\n                };".length)}; JSON.parse(JSON.stringify(traceEntry))`,
    { metrics, window: { __zubspaceMetricsPublishCount: 17 },
      actorTrace: [], rawActorStream: null, actorStreamError: null,
      performance: { now: () => 123 }, Date: { now: () => 1000 } },
  );
  const runtime = [{ playerId: 768, eventId: 4, x: 150 }];
  const prepared = [{ playerId: 768, eventId: 3, x: 110 }];
  const entry = record({ remoteProjectileSources: runtime,
    presentedRemoteProjectileSources: prepared, presentedRemoteProjectileSourcesOverflow: 2 });
  assert.deepEqual(JSON.parse(JSON.stringify(entry.remoteProjectileSources)), runtime);
  assert.equal(JSON.stringify(entry.presentedRemoteProjectileSources), JSON.stringify(prepared));
  assert.equal(entry.presentedRemoteProjectileSourcesOverflow, 2);
  assert.equal(entry.renderOrdinal, 17);
  assert.deepEqual(JSON.parse(JSON.stringify(record({ presentedRemoteProjectileSources: [] }).presentedRemoteProjectileSources)), []);
  assert.equal(record({}).presentedRemoteProjectileSources, null,
    "missing payload instrumentation cannot turn into an empty successful draw set");
});

test("ordinary browser play does not build discarded QA presentation diagnostics", () => {
  assert.match(
    browserEntry,
    /const metricsEnabled = traceEnabled \|\|\s*new URLSearchParams\(window\.location\.search\)\.get\("metrics"\) === "1";/,
  );
  assert.match(
    browserEntry,
    /importObject\.env\.zub_ui_metrics_requested = \(\) => \{\s*if \(!metricsEnabled\) \{\s*return 0;/,
  );
  assert.match(
    browserEntry,
    /importObject\.env\.zub_remote_sprite_trace_requested = \(\) =>\s*traceEnabled \? 1 : 0;/,
  );
  const networkRenderer = fs.readFileSync("src/game/render/ships/network.rs", "utf8");
  assert.match(
    networkRenderer,
    /let trace_requested = remote_sprite_trace_requested\(\);/,
  );
  assert.doesNotMatch(
    networkRenderer,
    /let trace_requested = ui_metrics_requested\(\);/,
  );
});

test("frame projection follows deferred Game application while physical edges retain the preceding draw", () => {
  const main = fs.readFileSync("src/main.rs", "utf8");
  const inputSource = main.indexOf("let mut input_events = FrameInputEvents::new();");
  const capture = main.indexOf("render_input.capture_frame(&mut input_events);");
  const receipt = main.indexOf("game.prepare_pending_semantic_weapon_receipt(&render_input, 0);");
  const poll = main.indexOf("game.poll_browser_network_before_gameplay();");
  const liveApplication = main.indexOf("game.update(");
  const unreadyApplication = main.indexOf("game.update_while_gameplay_unready(");
  const projection = main.indexOf("game.update_remote_presentation(");
  const draw = main.indexOf("game.draw(gameplay_clock.remainder_seconds() as f32, &render_input);");
  assert.ok(inputSource >= 0 && inputSource < capture);
  assert.equal(main.match(/render_input\.capture_frame\(/g)?.length, 1);
  assert.ok(capture >= 0 && capture < receipt && receipt < poll);
  assert.ok(poll < liveApplication && poll < unreadyApplication);
  assert.ok(liveApplication < projection && unreadyApplication < projection,
    "both frame branches must apply deferred snapshots before projecting their render clock");
  assert.ok(draw >= 0 && projection < draw, "project the fully applied frame immediately before drawing");
  assert.equal(main.match(/game\.update_remote_presentation\(/g)?.length, 1);
});

test("browser RTC defaults to the same-host candidate policy", () => {
  assert.deepEqual(JSON.parse(JSON.stringify(rtcConfiguration(""))), {
    iceServers: [],
    iceTransportPolicy: "all",
  });
});

test("deterministic network profiles pin the Phase 3 mild and slow envelopes", () => {
  assert.deepEqual(
    JSON.parse(JSON.stringify(networkProfile("?network_profile=mild&network_profile_seed=7"))),
    {
      name: "mild",
      seed: 7,
      oneWayDelayMs: 30,
      jitterMs: 10,
      s2cSnapshotOmissionMillionths: 10_000,
      c2sInputOmissionMillionths: 5_000,
      realtimeDuplicateMillionths: 5_000,
      realtimeReorderMillionths: 5_000,
    },
  );
  assert.deepEqual(
    JSON.parse(JSON.stringify(networkProfile("?network_profile=slow&network_profile_seed=9"))),
    {
      name: "slow",
      seed: 9,
      oneWayDelayMs: 75,
      jitterMs: 25,
      s2cSnapshotOmissionMillionths: 30_000,
      c2sInputOmissionMillionths: 10_000,
      realtimeDuplicateMillionths: 10_000,
      realtimeReorderMillionths: 10_000,
    },
  );
  assert.equal(networkProfile(""), null);
  assert.throws(() => networkProfile("?network_profile=unknown"), /unsupported/);
  assert.throws(
    () => networkProfile("?network_profile=mild&network_profile_seed=0"),
    /nonzero u32/,
  );
});

test("quality impairment profiles delay real traffic and drop only unreliable probes", () => {
  const latency = networkProfile("?network_profile=degraded-latency&network_profile_seed=7");
  const loss = networkProfile("?network_profile=degraded-loss&network_profile_seed=7");
  for (const direction of ["s2c", "c2s"]) {
    let drops = 0;
    for (let ordinal = 1; ordinal <= 2000; ordinal++) {
      const delayed = networkProfileDecision(latency, direction, ordinal, { quality: true, realtime: true });
      assert.ok(delayed.delayMs >= 330 && delayed.delayMs <= 370);
      assert.equal(delayed.dropped, false);
      const missing = networkProfileDecision(loss, direction, ordinal, { quality: true, realtime: true });
      assert.deepEqual(missing, networkProfileDecision(loss, direction, ordinal, { quality: true, realtime: true }));
      drops += Number(missing.dropped);
      assert.equal(networkProfileDecision(loss, direction, ordinal, { quality: true, reliable: true }).dropped, false);
    }
    assert.ok(drops > 300 && drops < 500, `${direction}: ${drops}/2000`);
  }
});

test("network profile decisions are repeatable bounded and realize every configured effect", () => {
  for (const name of ["mild", "slow"]) {
    const profile = networkProfile(`?network_profile=${name}&network_profile_seed=271828`);
    const counts = {
      s2cDrops: 0,
      c2sDrops: 0,
      s2cDuplicates: 0,
      c2sDuplicates: 0,
      s2cReorders: 0,
      c2sReorders: 0,
    };
    for (let ordinal = 1; ordinal <= 20_000; ordinal += 1) {
      const s2c = networkProfileDecision(profile, "s2c", ordinal, {
        snapshot: true,
        realtime: true,
      });
      const repeated = networkProfileDecision(profile, "s2c", ordinal, {
        snapshot: true,
        realtime: true,
      });
      assert.deepEqual(s2c, repeated);
      assert.ok(s2c.delayMs >= profile.oneWayDelayMs - profile.jitterMs);
      assert.ok(
        s2c.delayMs <= profile.oneWayDelayMs * 2 + profile.jitterMs * 2,
      );
      counts.s2cDrops += Number(s2c.dropped);
      counts.s2cDuplicates += Number(s2c.duplicated);
      counts.s2cReorders += Number(s2c.reordered);

      const c2s = networkProfileDecision(profile, "c2s", ordinal, {
        input: true,
        realtime: true,
      });
      counts.c2sDrops += Number(c2s.dropped);
      counts.c2sDuplicates += Number(c2s.duplicated);
      counts.c2sReorders += Number(c2s.reordered);
      const reliable = networkProfileDecision(profile, "s2c", ordinal, {
        snapshot: false,
        realtime: false,
        reliable: true,
      });
      assert.equal(reliable.dropped, false);
      assert.equal(reliable.duplicated, false);
      assert.equal(reliable.reordered, false);
    }
    for (const [effect, count] of Object.entries(counts)) {
      assert.ok(count > 0, `${name} did not realize ${effect}`);
    }
  }
});

test("ordered profile scheduling derives due time and delay from one clock sample", () => {
  assert.deepEqual(
    JSON.parse(JSON.stringify(orderedNetworkProfileSchedule(1_000, 20, 0))),
    { dueAt: 1_020, delayMs: 20 },
  );
  assert.deepEqual(
    JSON.parse(JSON.stringify(orderedNetworkProfileSchedule(1_000, 20, 1_025))),
    { dueAt: 1_026, delayMs: 26 },
  );
  assert.throws(
    () => orderedNetworkProfileSchedule(Number.NaN, 20, 0),
    /finite clock values/,
  );
});

test("browser RTC accepts STUN and authenticated TURN shapes", () => {
  const params = new URLSearchParams({
    ice_username: "pilot",
    ice_credential: "secret",
    ice_policy: "relay",
  });
  params.append("ice", "stun:stun.example.test:3478");
  params.append("ice", "turn:turn.example.test:3478?transport=udp");
  params.set("ice_gather_timeout_ms", "45000");
  assert.deepEqual(JSON.parse(JSON.stringify(rtcConfiguration(`?${params}`))), {
    iceServers: [
      {
        urls: [
          "stun:stun.example.test:3478",
          "turn:turn.example.test:3478?transport=udp",
        ],
        username: "pilot",
        credential: "secret",
      },
    ],
    iceTransportPolicy: "relay",
  });
  assert.equal(
    rtcIceGatheringTimeoutMs(params.get("ice_gather_timeout_ms")),
    45000,
  );
});

test("non-trickle ICE gathering timeout is realistic and bounded", () => {
  assert.equal(rtcIceGatheringTimeoutMs(null), 15000);
  assert.equal(rtcIceGatheringTimeoutMs("1000"), 1000);
  assert.equal(rtcIceGatheringTimeoutMs("60000"), 60000);
  assert.throws(() => rtcIceGatheringTimeoutMs("999"), /1000\.\.60000/);
  assert.throws(() => rtcIceGatheringTimeoutMs("60001"), /1000\.\.60000/);
  assert.throws(() => rtcIceGatheringTimeoutMs("slow"), /1000\.\.60000/);
});

test("overall signaling timeout is configurable and bounded", () => {
  assert.equal(rtcSignalingTimeoutMs(null), 20000);
  assert.equal(rtcSignalingTimeoutMs("1000"), 1000);
  assert.equal(rtcSignalingTimeoutMs("60000"), 60000);
  assert.throws(() => rtcSignalingTimeoutMs("999"), /1000\.\.60000/);
  assert.throws(() => rtcSignalingTimeoutMs("60001"), /1000\.\.60000/);
});

class FakeTimers {
  handles = [];

  setTimeout(callback, delay) {
    const handle = { callback, cleared: false, delay };
    this.handles.push(handle);
    return handle;
  }

  clearTimeout(handle) {
    handle.cleared = true;
  }

  fire(handle) {
    if (!handle.cleared) handle.callback();
  }
}

class FakePeer {
  constructor() {
    this.closeCalls = 0;
    this.iceGatheringState = "gathering";
    this.listeners = new Set();
  }

  addEventListener(name, listener) {
    assert.equal(name, "icegatheringstatechange");
    this.listeners.add(listener);
  }

  removeEventListener(name, listener) {
    assert.equal(name, "icegatheringstatechange");
    this.listeners.delete(listener);
  }

  completeGathering() {
    this.iceGatheringState = "complete";
    for (const listener of [...this.listeners]) listener();
  }

  close() {
    this.closeCalls += 1;
  }
}

test("delayed non-trickle gathering waits for the final STUN/TURN candidates", async () => {
  const peer = new FakePeer();
  const timers = new FakeTimers();
  const gathered = waitForIceGatheringComplete(peer, 45000, timers);

  assert.equal(timers.handles.length, 1);
  assert.equal(timers.handles[0].delay, 45000);
  assert.equal(peer.listeners.size, 1);
  peer.completeGathering();
  await gathered;

  assert.equal(timers.handles[0].cleared, true);
  assert.equal(peer.listeners.size, 0);
  assert.equal(peer.closeCalls, 0);
});

test("non-trickle gathering timeout rejects incomplete SDP and cleans up", async () => {
  const peer = new FakePeer();
  const timers = new FakeTimers();
  const gathered = waitForIceGatheringComplete(peer, 15000, timers);
  const rejected = assert.rejects(
    gathered,
    /non-trickle ICE gathering did not complete within 15000ms.*STUN\/TURN.*ice_gather_timeout_ms/,
  );

  timers.fire(timers.handles[0]);
  await rejected;

  assert.equal(timers.handles[0].cleared, true);
  assert.equal(peer.listeners.size, 0);
  assert.equal(
    peer.closeCalls,
    0,
    "the lifecycle owner must close only after preserving the explicit error",
  );
});

class FakeAbortController {
  constructor() {
    this.signal = {
      aborted: false,
      reason: null,
      listeners: new Set(),
      addEventListener(name, listener) {
        assert.equal(name, "abort");
        this.listeners.add(listener);
      },
      removeEventListener(name, listener) {
        assert.equal(name, "abort");
        this.listeners.delete(listener);
      },
    };
  }

  abort(reason) {
    this.signal.aborted = true;
    this.signal.reason = reason;
    for (const listener of [...this.signal.listeners]) listener();
  }
}

test("deferred signaling deadline owns cancellation before its response timer starts", async () => {
  const timers = new FakeTimers();
  const deadline = createRtcSignalingDeadline({
    timeoutMs: 5000,
    timers,
    AbortController: FakeAbortController,
    startImmediately: false,
  });
  assert.equal(deadline.isStarted(), false);
  assert.equal(timers.handles.length, 0);

  const waiting = assert.rejects(
    deadline.wait(new Promise(() => {})),
    /superseded/,
  );
  assert.equal(deadline.cancel(true), true);
  await waiting;
  assert.equal(deadline.signal.aborted, true);
  assert.equal(deadline.signal.listeners.size, 0);
  assert.equal(deadline.start(), false);
  assert.equal(timers.handles.length, 0);
});

test("browser connect owns and races every pre-signaling peer operation", () => {
  const html = fs.readFileSync("index.html", "utf8");
  const connectStart = html.indexOf("async function connectRtc");
  const connectEnd = html.indexOf("function sendNetworkPacket", connectStart);
  assert.notEqual(connectStart, -1);
  assert.notEqual(connectEnd, -1);
  const connectSource = html.slice(connectStart, connectEnd);
  const attachAt = connectSource.indexOf("rtcSignalingAttempts.attach(peer, signalingDeadline)");
  const offerAt = connectSource.indexOf("peer.createOffer()");
  assert.ok(attachAt >= 0 && attachAt < offerAt);
  assert.match(
    connectSource,
    /await signalingDeadline\.wait\(peer\.createOffer\(\)\)/,
  );
  assert.match(
    connectSource,
    /await signalingDeadline\.wait\(peer\.setLocalDescription\(offer\)\)/,
  );
  assert.match(
    connectSource,
    /zubspaceWaitForIceGatheringComplete\([\s\S]*?signalingDeadline\.signal,[\s\S]*?\),\s*\);/,
  );
  assert.ok(
    connectSource.indexOf("signalingDeadline.start()") >
      connectSource.indexOf("zubspaceWaitForIceGatheringComplete"),
  );
});

test("overall signaling deadline aborts a pending fetch with an explicit error", async () => {
  const timers = new FakeTimers();
  const timeouts = [];
  const deadline = createRtcSignalingDeadline({
    timeoutMs: 4000,
    timers,
    AbortController: FakeAbortController,
    onTimeout: (error) => timeouts.push(error.message),
  });
  const waiting = assert.rejects(
    deadline.wait(new Promise(() => {})),
    /signaling did not complete within 4000ms.*proxy health/,
  );

  timers.fire(timers.handles[0]);
  await waiting;
  assert.equal(deadline.signal.aborted, true);
  assert.deepEqual(timeouts, [
    "WebRTC signaling did not complete within 4000ms; check proxy health and retry",
  ]);
});

test("overall signaling deadline covers a pending response body", async () => {
  const timers = new FakeTimers();
  const deadline = createRtcSignalingDeadline({
    timeoutMs: 3000,
    timers,
    AbortController: FakeAbortController,
  });
  const response = await deadline.wait(
    Promise.resolve({ json: () => new Promise(() => {}) }),
  );
  const waitingBody = assert.rejects(
    deadline.wait(response.json()),
    /signaling did not complete within 3000ms/,
  );

  timers.fire(timers.handles[0]);
  await waitingBody;
  assert.equal(deadline.signal.aborted, true);
});

test("overall signaling deadline covers pending remote answer application", async () => {
  const timers = new FakeTimers();
  const deadline = createRtcSignalingDeadline({
    timeoutMs: 2500,
    timers,
    AbortController: FakeAbortController,
  });
  const applying = assert.rejects(
    deadline.wait(new Promise(() => {})),
    /signaling did not complete within 2500ms/,
  );

  timers.fire(timers.handles[0]);
  await applying;
  assert.equal(deadline.signal.aborted, true);
});

test("a cancelled old signaling timer cannot affect a replacement attempt", () => {
  const timers = new FakeTimers();
  let current = true;
  let timeoutCalls = 0;
  const deadline = createRtcSignalingDeadline({
    timeoutMs: 5000,
    timers,
    AbortController: FakeAbortController,
    isCurrent: () => current,
    onTimeout: () => {
      timeoutCalls += 1;
    },
  });
  const staleCallback = timers.handles[0].callback;
  assert.equal(deadline.cancel(), true);
  current = false;

  staleCallback();
  assert.equal(timeoutCalls, 0);
  assert.equal(deadline.signal.aborted, false);
});

class FakeEventTarget {
  constructor() {
    this.listeners = new Map();
  }

  addEventListener(name, listener) {
    const listeners = this.listeners.get(name) || new Set();
    listeners.add(listener);
    this.listeners.set(name, listeners);
  }

  removeEventListener(name, listener) {
    this.listeners.get(name)?.delete(listener);
  }

  dispatch(name, event = {}) {
    for (const listener of [...(this.listeners.get(name) || [])]) {
      listener(event);
    }
  }

  listenerCount() {
    return [...this.listeners.values()].reduce(
      (count, listeners) => count + listeners.size,
      0,
    );
  }
}

class FakeLifecyclePeer extends FakeEventTarget {
  constructor() {
    super();
    this.connectionState = "connecting";
    this.iceConnectionState = "checking";
    this.iceGatheringState = "gathering";
    this.closeCalls = 0;
  }

  setConnectionState(value) {
    this.connectionState = value;
    this.dispatch("connectionstatechange");
  }

  close() {
    this.closeCalls += 1;
    this.connectionState = "closed";
    this.dispatch("connectionstatechange");
  }
}

class FakeLifecycleChannel extends FakeEventTarget {
  constructor(label) {
    super();
    this.label = label;
    this.readyState = "connecting";
    this.ordered = label === "zubspace-reliable";
    this.maxPacketLifeTime = label === "zubspace-game" ? 80 : null;
    this.maxRetransmits = null;
    this.closeCalls = 0;
  }

  open() {
    this.readyState = "open";
    this.dispatch("open");
  }

  close() {
    this.closeCalls += 1;
    this.readyState = "closed";
    this.dispatch("close");
  }
}

function lifecycleFixture(options = {}) {
  const state = {
    peerConnection: null,
    gameChannel: null,
    eventChannel: null,
    reliableChannel: null,
    status: 1,
    lastClosedPeerConnectionState: null,
    lastErrorMessage: null,
  };
  const timers = new FakeTimers();
  const logs = [];
  const controller = createRtcLifecycleController({
    state,
    disconnectGraceMs: 5000,
    timers,
    log: (...entry) => logs.push(entry),
    ...options,
  });
  const peer = new FakeLifecyclePeer();
  const channels = {
    gameChannel: new FakeLifecycleChannel("zubspace-game"),
    eventChannel: new FakeLifecycleChannel("zubspace-events"),
    reliableChannel: new FakeLifecycleChannel("zubspace-reliable"),
  };
  controller.activate(peer, channels);
  return { state, timers, logs, controller, peer, channels };
}

function openLifecycleFixture(fixture) {
  fixture.peer.setConnectionState("connected");
  for (const channel of Object.values(fixture.channels)) channel.open();
  assert.equal(fixture.state.status, 2);
}

function assertLifecycleClosed(fixture, status, errorPattern) {
  assert.equal(fixture.state.status, status);
  assert.match(fixture.state.lastErrorMessage, errorPattern);
  assert.equal(fixture.state.peerConnection, null);
  assert.equal(fixture.state.gameChannel, null);
  assert.equal(fixture.state.eventChannel, null);
  assert.equal(fixture.state.reliableChannel, null);
  assert.equal(fixture.peer.closeCalls, 1);
  assert.equal(fixture.peer.listenerCount(), 0);
  for (const channel of Object.values(fixture.channels)) {
    assert.equal(channel.closeCalls, 1);
    assert.equal(channel.listenerCount(), 0);
  }
}

for (const stalledPhase of [
  "offer",
  "local-description",
  "gather",
  "fetch",
  "body",
  "answer",
]) {
  test(`superseding a stalled signaling ${stalledPhase} releases only the old attempt`, async () => {
    const state = {
      peerConnection: null,
      gameChannel: null,
      eventChannel: null,
      reliableChannel: null,
      status: 1,
      lastClosedPeerConnectionState: null,
      lastErrorMessage: null,
    };
    const timers = new FakeTimers();
    const registry = createRtcSignalingAttemptRegistry();
    const controller = createRtcLifecycleController({
      state,
      disconnectGraceMs: 5000,
      timers,
      onClose: (peer) => registry.cancel(peer, true),
    });
    const oldPeer = new FakeLifecyclePeer();
    const oldChannels = {
      gameChannel: new FakeLifecycleChannel("zubspace-game"),
      eventChannel: new FakeLifecycleChannel("zubspace-events"),
      reliableChannel: new FakeLifecycleChannel("zubspace-reliable"),
    };
    controller.activate(oldPeer, oldChannels);
    const oldDeadline = registry.attach(
      oldPeer,
      createRtcSignalingDeadline({
        timeoutMs: 5000,
        timers,
        AbortController: FakeAbortController,
        startImmediately: false,
      }),
    );
    let oldSettled = false;
    const never = () => new Promise(() => {});
    let resolvePhaseReached;
    const phaseReached = new Promise((resolve) => {
      resolvePhaseReached = resolve;
    });
    const stallAtPhase = () => {
      resolvePhaseReached();
      return never();
    };
    const oldCoroutine = (async () => {
      try {
        if (stalledPhase === "offer") {
          await oldDeadline.wait(stallAtPhase());
        } else {
          await oldDeadline.wait(Promise.resolve({ type: "offer" }));
          if (stalledPhase === "local-description") {
            await oldDeadline.wait(stallAtPhase());
          } else {
            await oldDeadline.wait(Promise.resolve());
            if (stalledPhase === "gather") {
              const gathering = waitForIceGatheringComplete(
                oldPeer,
                15000,
                timers,
                oldDeadline.signal,
              );
              resolvePhaseReached();
              await oldDeadline.wait(
                gathering,
              );
            } else {
              assert.equal(oldDeadline.start(), true);
              if (stalledPhase === "fetch") {
                await oldDeadline.wait(stallAtPhase());
              } else {
                const response = await oldDeadline.wait(
                  Promise.resolve({ json: stallAtPhase }),
                );
                if (stalledPhase === "body") {
                  await oldDeadline.wait(response.json());
                } else {
                  await oldDeadline.wait(Promise.resolve({ type: "answer" }));
                  await oldDeadline.wait(stallAtPhase());
                }
              }
            }
          }
        }
        assert.fail("superseded signaling coroutine must not remain pending");
      } catch (error) {
        assert.match(error.message, /superseded/);
      } finally {
        registry.cancel(oldPeer, false);
        oldSettled = true;
      }
    })();
    await phaseReached;
    const oldTimerHandles = [...timers.handles];

    const replacementPeer = new FakeLifecyclePeer();
    const replacementChannels = {
      gameChannel: new FakeLifecycleChannel("zubspace-game"),
      eventChannel: new FakeLifecycleChannel("zubspace-events"),
      reliableChannel: new FakeLifecycleChannel("zubspace-reliable"),
    };
    controller.activate(replacementPeer, replacementChannels);
    const replacementDeadline = registry.attach(
      replacementPeer,
      createRtcSignalingDeadline({
        timeoutMs: 5000,
        timers,
        AbortController: FakeAbortController,
        startImmediately: false,
      }),
    );
    await oldCoroutine;

    assert.equal(oldSettled, true);
    assert.equal(oldDeadline.signal.aborted, true);
    assert.equal(oldDeadline.signal.listeners.size, 0);
    for (const timer of oldTimerHandles) assert.equal(timer.cleared, true);
    assert.equal(registry.has(oldPeer), false);
    assert.equal(oldPeer.closeCalls, 1);
    assert.equal(oldPeer.listenerCount(), 0);
    for (const channel of Object.values(oldChannels)) {
      assert.equal(channel.closeCalls, 1);
      assert.equal(channel.listenerCount(), 0);
    }
    assert.equal(state.peerConnection, replacementPeer);
    assert.equal(replacementPeer.closeCalls, 0);
    assert.ok(replacementPeer.listenerCount() > 0);
    assert.equal(registry.has(replacementPeer), true);
    assert.equal(replacementDeadline.signal.aborted, false);
    assert.equal(replacementDeadline.signal.listeners.size, 0);
    assert.equal(replacementDeadline.isStarted(), false);

    controller.terminatePeer(replacementPeer, 3, "test cleanup");
    assert.equal(registry.size(), 0);
  });
}

test("RTC disconnect grace preserves a peer that reconnects", () => {
  const fixture = lifecycleFixture();
  openLifecycleFixture(fixture);

  fixture.peer.setConnectionState("disconnected");
  assert.equal(fixture.state.status, 3);
  assert.match(fixture.state.lastErrorMessage, /waiting up to 5000ms/);
  assert.equal(fixture.timers.handles.length, 1);
  assert.equal(fixture.timers.handles[0].delay, 5000);

  fixture.peer.setConnectionState("connected");
  assert.equal(fixture.timers.handles[0].cleared, true);
  assert.equal(fixture.state.status, 2);
  assert.equal(fixture.state.lastErrorMessage, null);
  assert.equal(fixture.state.peerConnection, fixture.peer);
  assert.equal(fixture.peer.closeCalls, 0);
});

test("ICE timeout remains visible when peer close dispatches synchronously", async () => {
  const fixture = lifecycleFixture();
  const gathering = waitForIceGatheringComplete(
    fixture.peer,
    15000,
    fixture.timers,
  );
  const handled = gathering.catch((error) => {
    fixture.controller.terminatePeer(fixture.peer, 4, error.message);
  });

  fixture.timers.fire(fixture.timers.handles[0]);
  await handled;

  assertLifecycleClosed(
    fixture,
    4,
    /non-trickle ICE gathering did not complete within 15000ms/,
  );
  assert.equal(
    fixture.logs.some((entry) =>
      String(entry[2]?.reason).includes("non-trickle ICE gathering"),
    ),
    true,
  );
});

test("RTC disconnect grace expiry closes channels, peer, timers, and listeners", () => {
  const fixture = lifecycleFixture();
  openLifecycleFixture(fixture);
  fixture.peer.setConnectionState("disconnected");
  fixture.timers.fire(fixture.timers.handles[0]);

  assertLifecycleClosed(fixture, 4, /did not recover within 5000ms/);
});

test("terminal RTC peer failure performs guarded transport cleanup", () => {
  const fixture = lifecycleFixture();
  openLifecycleFixture(fixture);
  fixture.peer.setConnectionState("disconnected");
  const disconnectTimer = fixture.timers.handles[0];
  fixture.peer.setConnectionState("failed");

  assert.equal(disconnectTimer.cleared, true);
  assertLifecycleClosed(fixture, 4, /peer connection failed/);
});

test("RTC error logs retain cloneable failure details for cold QA snapshots", () => {
  const fixture = lifecycleFixture();
  openLifecycleFixture(fixture);
  const error = Object.defineProperties(new Error("SCTP abort"), {
    errorDetail: { value: "sctp-failure" },
    sctpCauseCode: { value: 12 },
  });
  const event = { type: "error", error, uncloneable() {} };
  assert.throws(() => structuredClone(event), /clone/i);
  fixture.channels.eventChannel.dispatch("error", event);
  const logs = structuredClone(fixture.logs);
  const detail = logs.find(entry => entry[1] === "rtc data channel error")[2];
  assert.equal(detail.label, "zubspace-events");
  assert.equal(detail.eventType, "error");
  assert.equal(detail.error.message, "SCTP abort");
  assert.equal(detail.error.errorDetail, "sctp-failure");
  assert.equal(detail.error.sctpCauseCode, 12);
  assertLifecycleClosed(fixture, 4, /zubspace-events failed/);
});

test("terminal RTC channel error and close perform transport cleanup", () => {
  const failed = lifecycleFixture();
  openLifecycleFixture(failed);
  failed.channels.eventChannel.dispatch("error", { message: "boom" });
  assertLifecycleClosed(failed, 4, /zubspace-events failed/);

  const closed = lifecycleFixture();
  openLifecycleFixture(closed);
  closed.channels.gameChannel.dispatch("close");
  assertLifecycleClosed(closed, 3, /zubspace-game closed/);
});

test("the open instant is recorded once and survives a later terminal close", () => {
  const fixture = lifecycleFixture();
  assert.equal(fixture.state.openNotifiedAt, undefined);
  assert.equal(rtcTransportIsTerminal(fixture.state), false);

  openLifecycleFixture(fixture);
  assert.equal(typeof fixture.state.openNotifiedAt, "number");
  const openedAt = fixture.state.openNotifiedAt;

  // The terminal predicate is false while the session is live, even though a
  // stale close code alone would be ambiguous.
  assert.equal(rtcTransportIsTerminal(fixture.state), false);

  // A channel close is terminal once the transport has opened, and the open
  // instant is preserved so a reader can date the transition.
  fixture.channels.gameChannel.dispatch("close");
  assert.equal(fixture.state.status, 3);
  assert.equal(fixture.state.openNotifiedAt, openedAt);
  assert.equal(rtcTransportIsTerminal(fixture.state), true);
});

test("a connection that never opened is not a recovery candidate", () => {
  const fixture = lifecycleFixture();
  // Failed before any channel opened: the connect-failure UI is the right
  // answer, and a rejoin must not fire for a transport that never worked.
  fixture.peer.setConnectionState("failed");
  assert.equal(fixture.state.status, 4);
  assert.equal(
    rtcTransportIsTerminal(fixture.state),
    false,
    "a signaling failure that never reached open must not look terminal",
  );

  // The same guard holds for a page whose state predates this field: an absent
  // openNotifiedAt must read as not-opened, not as a completed open.
  assert.equal(rtcTransportIsTerminal({ status: 3 }), false);
  assert.equal(rtcTransportIsTerminal({ status: 4 }), false);
  assert.equal(rtcTransportIsTerminal(null), false);
});

test("the page seeds the open instant and wires the shared terminal predicate", () => {
  // The wasm import must consume the one shared predicate rather than a second
  // copy, or the page and its tests can disagree about what terminal means.
  assert.match(
    browserEntry,
    /openNotifiedAt:\s*null,/,
    "the RTC state must seed openNotifiedAt so an absent value cannot read as opened",
  );
  assert.match(
    browserEntry,
    /importObject\.env\.zub_rtc_terminal\s*=\s*\(\)\s*=>\s*\n\s*window\.zubspaceRtcTransportIsTerminal\(state\)/,
    "zub_rtc_terminal must route through the shared predicate",
  );
  assert.doesNotMatch(
    browserEntry,
    /importObject\.env\.zub_rtc_terminal[\s\S]{0,160}state\.status === 3/,
    "zub_rtc_terminal must not re-derive the condition inline",
  );
});

test("stale callbacks from a replaced RTC peer cannot clear the replacement", () => {
  const old = lifecycleFixture();
  openLifecycleFixture(old);
  const staleFailure = [...old.peer.listeners.get("connectionstatechange")][0];
  const staleClose = [...old.channels.reliableChannel.listeners.get("close")][0];

  const replacementPeer = new FakeLifecyclePeer();
  const replacementChannels = {
    gameChannel: new FakeLifecycleChannel("zubspace-game"),
    eventChannel: new FakeLifecycleChannel("zubspace-events"),
    reliableChannel: new FakeLifecycleChannel("zubspace-reliable"),
  };
  old.controller.activate(replacementPeer, replacementChannels);
  replacementPeer.setConnectionState("connected");
  for (const channel of Object.values(replacementChannels)) channel.open();
  assert.equal(old.state.status, 2);

  old.peer.connectionState = "failed";
  staleFailure();
  staleClose();

  assert.equal(old.state.status, 2);
  assert.equal(old.state.peerConnection, replacementPeer);
  assert.equal(replacementPeer.closeCalls, 0);
  assert.equal(old.state.lastErrorMessage, null);
});

test("browser RTC accepts every shared canonical ICE host fixture", () => {
  for (const value of iceUrlFixtures.accepted) {
    const params = new URLSearchParams({
      ice: value,
      ice_username: "pilot",
      ice_credential: "secret",
    });
    assert.deepEqual(
      JSON.parse(JSON.stringify(rtcConfiguration(`?${params}`))),
      {
        iceServers: [
          {
            urls: [value],
            username: "pilot",
            credential: "secret",
          },
        ],
        iceTransportPolicy: "all",
      },
      value,
    );
  }
});

test("browser RTC rejects unusable relay shapes", () => {
  assert.throws(
    () =>
      rtcConfiguration(
        "?ice=turn%3Aturn.example.test%3A3478%3Ftransport%3Dudp",
      ),
    /require ice_username and ice_credential/,
  );
  assert.throws(() => rtcConfiguration("?ice_policy=relay"), /requires a TURN ICE URL/);
  assert.throws(
    () => rtcConfiguration("?ice=stuns%3Astun.example.test%3A5349"),
    /must be canonical/,
  );
  assert.throws(
    () =>
      rtcConfiguration(
        "?ice=turn%3Aturn.example.test%3A3478%3Ftransport%3Dtcp&ice_username=pilot&ice_credential=secret",
      ),
    /must be canonical/,
  );
});

test("browser RTC rejects every shared non-canonical ICE host fixture", () => {
  for (const value of iceUrlFixtures.rejected) {
    const params = new URLSearchParams({ ice: value });
    assert.throws(() => rtcConfiguration(`?${params}`), /canonical/, value);
  }
});

test("browser signaling sends admission tokens only to the exact page origin", () => {
  assert.deepEqual(JSON.parse(JSON.stringify(signalingHeaders(""))), {
    "Content-Type": "application/json",
  });
  assert.deepEqual(
    JSON.parse(
      JSON.stringify(
        signalingHeaders(
          "?rtc_token=admit-me",
          undefined,
          "https://play.example.test/v1/rtc/offer",
          "https://play.example.test",
        ),
      ),
    ),
    {
      "Content-Type": "application/json",
      Authorization: "Bearer admit-me",
    },
  );
  assert.deepEqual(
    JSON.parse(
      JSON.stringify(
        signalingHeaders(
          "?rtc_token=query-token",
          "bootstrap-token",
          "/v1/rtc/offer",
          "https://play.example.test",
        ),
      ),
    ),
    {
      "Content-Type": "application/json",
      Authorization: "Bearer bootstrap-token",
    },
  );
});

test("arbitrary rtc overrides cannot receive an injected browser token", () => {
  const search = "?rtc=https%3A%2F%2Fattacker.example%2Fcollect";
  const offerUrl = new URLSearchParams(search).get("rtc");
  assert.throws(
    () =>
      signalingHeaders(
        search,
        "injected-bootstrap-secret",
        offerUrl,
        "https://play.example.test",
      ),
    /refusing to send.*Bearer token.*cross-origin.*attacker\.example.*must use the page origin https:\/\/play\.example\.test.*omit the token/,
  );

  assert.deepEqual(
    JSON.parse(
      JSON.stringify(
        signalingHeaders(search, undefined, offerUrl, "https://play.example.test"),
      ),
    ),
    { "Content-Type": "application/json" },
    "an explicit cross-origin rtc override remains available without credentials",
  );
  assert.throws(
    () =>
      signalingHeaders(
        `${search}&rtc_token=query-secret`,
        undefined,
        offerUrl,
        "https://play.example.test",
      ),
    /authenticated signaling must use the page origin/,
  );
});

test("requested disconnect cleanup is informational while unexpected RTC failure remains an error", () => {
  for (const source of ["channel", "peer"]) {
    const fixture = lifecycleFixture();
    openLifecycleFixture(fixture);
    fixture.state.disconnectRequested = true;
    if (source === "channel") fixture.channels.eventChannel.dispatch("error", { type: "error", error: new Error("SCTP abort") });
    else fixture.peer.setConnectionState("failed");
    assertLifecycleClosed(fixture, 3, /disconnect requested/);
    assert.equal(fixture.logs.filter(entry => entry[0] === "error").length, 0);
    assert.ok(fixture.logs.some(entry => entry[1] === "rtc transport closed"));
  }
  const unexpected = lifecycleFixture();
  openLifecycleFixture(unexpected);
  unexpected.channels.eventChannel.dispatch("error", { type: "error" });
  assertLifecycleClosed(unexpected, 4, /failed/);
  assert.ok(unexpected.logs.some(entry => entry[0] === "error"));
});

test("bounded RTC receive queue preserves mixed-lane FIFO and both capacity edges atomically", () => {
  let now = 100;
  const make = context.zubspaceCreateRtcReceiveQueue;
  const queue = make({ maxPackets: 2, maxBytes: 5, now: () => now });
  const first = { packet: new Uint8Array([1, 2]), channel: "zubspace-reliable" };
  const second = { packet: new Uint8Array([3, 4, 5]), channel: "zubspace-game" };
  queue.push(first); now = 110; queue.push(second); now = 120;
  assert.equal(queue.length, 2); assert.equal(queue.bytes, 5); assert.equal(queue.oldestAgeMs, 20);
  assert.throws(() => queue.push({ packet: new Uint8Array([6]) }), /exceeds capacity/);
  assert.equal(queue.front(), first); assert.equal(queue.shift(), first);
  assert.equal(queue.bytes, 3); assert.equal(queue.oldestAgeMs, 10);
  assert.throws(() => queue.push({ packet: new Uint8Array(3) }), /exceeds capacity/);
  assert.equal(queue.length, 1); assert.equal(queue.bytes, 3);
  queue.push(first);
  assert.equal(queue.shift(), second); assert.equal(queue.shift(), first);
  assert.equal(queue.shift(), undefined); assert.equal(queue.bytes, 0); assert.equal(queue.oldestAgeMs, 0);
  assert.throws(() => queue.push({ packet: new Uint8Array(0) }), /binary packet/);
  assert.throws(() => queue.push({}), /binary packet/);
  for (const maxPackets of [0, -1, Infinity, 1.5]) assert.throws(() => make({ maxPackets }), /positive safe integers/);
});

test("RTC cursor compaction keeps every fact and clear releases retained storage", () => {
  const queue = context.zubspaceCreateRtcReceiveQueue({ maxPackets: 4096, maxBytes: 4096, now: () => 0 });
  const entries = Array.from({ length: 2048 }, (_, i) => ({ packet: new Uint8Array([i % 256]), ordinal: i }));
  for (const entry of entries) queue.push(entry);
  for (let i = 0; i < 1536; i++) assert.equal(queue.shift(), entries[i]);
  for (let i = 0; i < 1024; i++) queue.push(entries[i]);
  for (let i = 1536; i < 2048; i++) assert.equal(queue.shift(), entries[i]);
  for (let i = 0; i < 1024; i++) assert.equal(queue.shift(), entries[i]);
  assert.equal(queue.length, 0); assert.equal(queue.bytes, 0);
  queue.push(entries[0]); queue.clear();
  assert.equal(queue.front(), undefined); assert.equal(queue.bytes, 0);
  queue.push(entries[1]); assert.equal(queue.shift(), entries[1]);
});

test("RTC receive overflow closes only its owning transport and clears the backlog", () => {
  const healthy = lifecycleFixture();
  openLifecycleFixture(healthy);
  const queue = context.zubspaceCreateRtcReceiveQueue({ maxPackets: 1, maxBytes: 10, now: () => 0 });
  const fixture = lifecycleFixture({
    onMessage(channel, event) { queue.push({ packet: new Uint8Array(event.data), channel: channel.label }); },
    onClose() { queue.clear(); },
  });
  openLifecycleFixture(fixture);
  fixture.channels.gameChannel.dispatch("message", { data: new Uint8Array([1]).buffer });
  assert.equal(queue.length, 1);
  fixture.channels.eventChannel.dispatch("message", { data: new Uint8Array([2]).buffer });
  assertLifecycleClosed(fixture, 4, /receive backlog exceeds capacity/);
  assert.equal(healthy.state.status, 2);
  assert.equal(healthy.peer.closeCalls, 0);
  healthy.controller.closeActive();
  assert.equal(queue.length, 0); assert.equal(queue.bytes, 0);
  assert.match(browserEntry, /queue: window\.zubspaceCreateRtcReceiveQueue\(\)/);
  assert.match(browserEntry, /rtcLifecycle\.closeActive\(4, error\?\.message/);
  assert.doesNotMatch(browserEntry, /state\.queue\[0\]|state\.queue = \[\]/);
});

test("batched realtime events expand to the exact packets the server batched", () => {
  // Arrays from the page's VM context compare by value only after copying.
  const expand = (packet) => {
    const events = context.zubspaceExpandRealtimeEventBatch(packet);
    return events && Array.from(events, (event) => Buffer.from(event).toString("hex"));
  };
  const fixture = JSON.parse(fs.readFileSync("tests/fixtures/realtime_event_batch.json", "utf8"));
  const bytes = (hex) => Uint8Array.from(Buffer.from(hex, "hex"));
  const batch = bytes(fixture.batch);
  assert.deepEqual(expand(batch), fixture.events);
  // Anything else passes through untouched.
  assert.equal(expand(bytes(fixture.events[0])), null);
  assert.equal(expand(new Uint8Array([0x70, 1, 2])), null);
  // Malformed batches yield nothing rather than a partial or shifted event.
  assert.deepEqual(expand(batch.subarray(0, batch.length - 1)), []);
  assert.deepEqual(expand(Uint8Array.from([...batch, 0])), []);
  const version = batch.slice();
  version[1] = 2;
  assert.deepEqual(expand(version), []);
  const single = batch.slice();
  single[2] = 1;
  assert.deepEqual(expand(single), []);
  const innerType = batch.slice();
  innerType[21] = 0x70;
  assert.deepEqual(expand(innerType), []);
});
