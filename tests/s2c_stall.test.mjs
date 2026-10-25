import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
function section(first, next) {
  const start = html.indexOf(first), end = html.indexOf(next, start + first.length);
  assert(start >= 0 && end > start);
  return html.slice(start, end);
}
function fixture(configured = true) {
  const received = [], logs = [];
  const state = { s2cStallEnabled: false, s2cStallKeepQualityProbes: false,
    s2cStallQualityProbesPassed: 0, s2cStallPacketsSeen: 0,
    s2cStallPacketsBuffered: 0, s2cStallPacketsReleased: 0, s2cStallBuffer: [] };
  const window = { __zubspaceNetworkStats: {}, zubspaceExpandRealtimeEventBatch: () => null };
  const context = vm.createContext({ state, window, s2cStallConfigured: configured,
    maybeCaptureClockBootstrap() {}, maybeImpairActiveProjectileState: () => false,
    maybeImpairPrizeEffectBoundary: () => false, maybeImpairLifecycleReceipt: () => false,
    maybeImpairRepelReceipt: () => false, maybeImpairReliableDeathDelivery: () => false,
    maybeImpairMovementCorrectionOffer: () => false, maybeImpairMovementCorrectionAcceptance: () => false,
    maybeImpairRepelDelivery: () => false, maybeStallClockFields: packet => ({ packet }),
    packetLog: (...args) => logs.push(args), enqueueRtcReceivedPacket: entry => received.push(entry) });
  const probeHelper = html.includes("function isS2cStallQualityProbe(")
    ? section("function isS2cStallQualityProbe(", "function deliverRtcInbound(") : "";
  vm.runInContext(probeHelper + section("function deliverRtcInbound(",
    "// Test-only end-of-measurement barrier."), context);
  vm.runInContext(section("window.__zubspaceSetS2cStallForTest =",
    "window.__zubspaceActiveProjectileImpairmentStateForTest ="), context);
  return { state, window, received, logs, deliver: context.deliverRtcInbound };
}
function probe() {
  const packet = new Uint8Array(30);
  packet.set([0x81, 1]);
  const view = new DataView(packet.buffer);
  view.setBigUint64(2, 11n, true); view.setBigUint64(10, 22n, true);
  view.setUint32(18, 0xffffffff, true); view.setBigUint64(22, 33n, true);
  return packet;
}

test("unarmed URLs cannot enable the S2C fault seam or alter delivery", () => {
  const f = fixture(false), packet = probe();
  assert.equal(f.window.__zubspaceSetS2cStallForTest(true, { keepQualityProbes: true }), false);
  f.deliver("zubspace-game", packet);
  assert.equal(f.received.length, 1); assert.equal(f.state.s2cStallBuffer.length, 0);
});

test("the default armed stall still buffers all traffic and releases each packet once", () => {
  const f = fixture(), packet = probe();
  assert.equal(f.window.__zubspaceSetS2cStallForTest(true), true);
  f.deliver("zubspace-game", packet); f.deliver("zubspace-reliable", Uint8Array.of(0, 3, 1));
  assert.equal(f.received.length, 0); assert.equal(f.state.s2cStallBuffer.length, 2);
  assert.equal(f.window.__zubspaceSetS2cStallForTest(false), false);
  assert.equal(f.received.length, 2); assert.equal(f.state.s2cStallPacketsReleased, 2);
  f.window.__zubspaceSetS2cStallForTest(false); assert.equal(f.received.length, 2);
});

test("door-only mode passes genuine probe framing while all gameplay remains stalled", () => {
  const f = fixture(), packet = probe();
  assert.equal(f.window.__zubspaceSetS2cStallForTest(true, { keepQualityProbes: true }), true);
  f.deliver("zubspace-game", packet);
  assert.equal(f.received.length, 1); assert.equal(f.received[0].packet, packet);
  for (const bad of [Uint8Array.of(0x70), packet.slice(0, 29),
    packet.map((v, i) => i === 1 ? 0 : v),
    packet.map((v, i) => i >= 2 && i < 10 ? 0 : v),
    packet.map((v, i) => i >= 10 && i < 18 ? 0 : v),
    packet.map((v, i) => i >= 22 ? 0 : v)]) f.deliver("zubspace-game", bad);
  f.deliver("zubspace-reliable", packet);
  assert.equal(f.received.length, 1);
  const before = f.window.__zubspaceS2cStallStateForTest();
  assert.equal(before.keepQualityProbes, true); assert.equal(before.qualityProbesPassed, 1);
  assert.equal(before.queued, 7); assert.equal(before.buffered, 7);
  f.window.__zubspaceSetS2cStallForTest(false);
  assert.equal(f.received.length, 8); assert.equal(f.window.__zubspaceS2cStallStateForTest().queued, 0);
  assert.equal(f.window.__zubspaceS2cStallStateForTest().released, 7);
  assert.ok(f.logs.some(entry => entry[2].s2cStall === "quality-passthrough"));
});
