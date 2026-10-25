import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";

test("keyboard matrix rejects unsupported network profiles before starting services", () => {
  const result = spawnSync(process.execPath, ["tests/e2e/keyboard_chord_matrix.mjs"], {
    env: { ...process.env, ZUBSPACE_E2E_NETWORK_PROFILE: "unverified-profile" },
    encoding: "utf8", timeout: 10_000,
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /unsupported browser impairment profile/);
});

test("keyboard matrix rejects invalid seeds before starting services", () => {
  for (const seed of ["0", "-1", "1.5", "4294967296", "invalid"]) {
    const result = spawnSync(process.execPath, ["tests/e2e/keyboard_chord_matrix.mjs"], {
      env: { ...process.env, ZUBSPACE_E2E_NETWORK_PROFILE: "mild", ZUBSPACE_E2E_NETWORK_PROFILE_SEED: seed },
      encoding: "utf8", timeout: 10_000,
    });
    assert.equal(result.status, 1, seed);
    assert.match(result.stderr, /keyboard network profile seed must be a nonzero u32/);
  }
});

test("measurement barrier preserves delayed delivery and does not hide expiry", async () => {
  const { readFileSync } = await import("node:fs");
  const { runInNewContext } = await import("node:vm");
  const source = readFileSync("index.html", "utf8");
  const inbound = source.slice(source.indexOf("          let networkProfileDraining ="),
    source.indexOf("          const RTC_DISCONNECT_GRACE_MS"));
  const outbound = source.slice(source.indexOf("          function maybeScheduleNetworkProfileSend("),
    source.indexOf("          window.__zubspaceNetworkProfileStateForTest"));
  const fixture = () => {
    const state = new Proxy({ connectionAttempt: 1, peerConnection: {} }, { get: (obj, key) => obj[key] ?? 0 });
    const timers = [], delivered = [], window = {};
    window.setTimeout = callback => timers.push(callback);
    window.zubspaceNetworkProfileDecision = () => ({ delayMs: 20 });
    window.zubspaceOrderedNetworkProfileSchedule = (now, delay) => ({ dueAt: now + delay, delayMs: delay });
    const context = { state, window, networkParams: new URLSearchParams(), deterministicNetworkProfile: { name: "mild", seed: 1 },
      incrementNetworkProfileCounter: key => state[key]++, setNetworkProfileCounter: (key, value) => state[key] = value,
      performance: { now: () => 100 }, packetLog() {}, recordNetworkProfileRelease() {},
      clientInputPacketOffset: () => null, deliverRtcInbound: (_label, packet) => delivered.push(packet[0]) };
    runInNewContext(inbound + outbound + "\nwindow.inbound = maybeScheduleNetworkProfileInbound; window.outbound = maybeScheduleNetworkProfileSend;", context);
    return { state, timers, delivered, window };
  };
  for (const direction of ["inbound", "outbound"]) {
    const f = fixture();
    const channel = { label: "zubspace-reliable", readyState: "open", send: packet => f.delivered.push(packet[0]) };
    const send = value => direction === "inbound"
      ? f.window.inbound(channel.label, Uint8Array.of(value))
      : f.window.outbound(Uint8Array.of(value), channel);
    assert.equal(send(1), true);
    f.window.__zubspaceEndNetworkProfileMeasurementForTest();
    assert.equal(send(2), true);
    assert.deepEqual(f.delivered, [], "barrier must not release an existing delayed packet early");
    f.timers.shift()();
    assert.deepEqual(f.delivered, [1, 2], "post-window reliable traffic must follow the delayed packet");
    assert.equal(f.state.networkProfileS2cExpired + f.state.networkProfileC2sExpired, 0);
  }
  const f = fixture();
  f.window.inbound("zubspace-reliable", Uint8Array.of(1));
  f.window.__zubspaceEndNetworkProfileMeasurementForTest();
  f.window.inbound("zubspace-reliable", Uint8Array.of(2));
  f.state.connectionAttempt++;
  f.timers.shift()();
  assert.deepEqual(f.delivered, []);
  assert.equal(f.state.networkProfileS2cExpired, 1, "retired session must still fail the no-expiry contract");
});
