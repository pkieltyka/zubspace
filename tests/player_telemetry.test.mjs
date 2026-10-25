import assert from "node:assert/strict";
import test from "node:test";
import "../js/zubspace_player_telemetry.js";

function fixture() {
  let time = 0, visible = true;
  const packets = [];
  const peer = { connectionState: "connected", getStats: async () => new Map([
    ["transport", { type: "transport", selectedCandidatePairId: "selected" }],
    ["selected", { id: "selected", type: "candidate-pair", currentRoundTripTime: 0.042 }],
    ["old", { id: "old", type: "candidate-pair", nominated: true, state: "succeeded", currentRoundTripTime: 9 }],
  ]) };
  const telemetry = ZubspacePlayerTelemetry.create({ now: () => time, visible: () => visible, peer: () => peer, send: p => packets.push(p) });
  const template = new Uint8Array(54); template[0] = 0x80; template[1] = 1;
  return { telemetry, template, packets, peer, advance: ms => time += ms, visible: value => visible = value };
}

test("visible frame stalls and selected-pair RTT survive the bounded report", async () => {
  const f = fixture();
  f.telemetry.frame(10, true);
  for (let i = 0; i < 100; i++) { f.advance(16); f.telemetry.frame(10, true); }
  f.advance(100); f.telemetry.frame(12, false);
  await f.telemetry.report(f.template);
  const view = new DataView(f.packets[0].buffer);
  assert.equal(view.getUint32(22, true), 101);
  assert.equal(view.getUint32(26, true), 1);
  assert.equal(view.getUint32(30, true), 1_700_000);
  assert.equal(view.getUint32(34, true), 100_000);
  assert.equal(view.getUint32(38, true), 42_000);
  assert.equal(view.getUint32(42, true), 2);
  assert.equal(view.getUint32(46, true), 1);
});

test("hidden time is excluded and missing ICE statistics stay absent", async () => {
  const f = fixture();
  f.telemetry.frame(0, true); f.advance(3000); f.visible(false); f.telemetry.frame(0, true);
  await f.telemetry.report(f.template); assert.equal(f.packets.length, 0);
  f.visible(true); f.telemetry.frame(0, true);
  f.advance(1500); f.telemetry.frame(0, true);
  f.peer.getStats = async () => { throw new Error("stats unavailable"); };
  await f.telemetry.report(f.template);
  const view = new DataView(f.packets[0].buffer);
  assert.equal(view.getUint32(30, true), 1_500_000);
  assert.equal(view.getUint32(38, true), 0xffffffff);
});

test("a peer replaced while getStats is pending cannot emit an old report", async () => {
  let resolve;
  const f = fixture(); f.peer.getStats = () => new Promise(done => resolve = done);
  f.telemetry.frame(0, true); f.advance(1500); f.telemetry.frame(0, true);
  const pending = f.telemetry.report(f.template);
  f.peer.connectionState = "closed"; resolve(new Map()); await pending;
  assert.equal(f.packets.length, 0);
});

test("ambiguous nominated pairs cannot masquerade as a selected ICE RTT", async () => {
  const f = fixture();
  f.peer.getStats = async () => new Map([
    ["a", { type: "candidate-pair", nominated: true, state: "succeeded", currentRoundTripTime: 0.02 }],
    ["b", { type: "candidate-pair", nominated: true, state: "succeeded", currentRoundTripTime: 0.09 }],
  ]);
  f.telemetry.frame(0, true); f.advance(1500); f.telemetry.frame(0, true);
  await f.telemetry.report(f.template);
  assert.equal(new DataView(f.packets[0].buffer).getUint32(38, true), 0xffffffff);
});

test("version 2 reports carry the game channel's queue peak and this window's dropped sends", async () => {
  let sending = { bufferedPeak: 0, dropped: 7 };
  const f = fixture();
  const packets = [];
  let time = 0;
  const telemetry = ZubspacePlayerTelemetry.create({
    now: () => time, visible: () => true, peer: () => f.peer, send: p => packets.push(p),
    transport: () => sending,
  });
  const template = new Uint8Array(62); template[0] = 0x80; template[1] = 2;
  const window = async (peak, dropped) => {
    sending = { bufferedPeak: peak, dropped };
    telemetry.frame(0, true); time += 1500; telemetry.frame(0, true);
    await telemetry.report(template);
    const view = new DataView(packets.at(-1).buffer);
    return [view.getUint32(22, true), view.getUint32(54, true), view.getUint32(58, true)];
  };
  // The first window has no earlier count, so drops before it are not claimed.
  assert.deepEqual(await window(2_400, 7), [1, 2_400, 0]);
  assert.deepEqual(await window(900, 12), [1, 900, 5]);
  assert.deepEqual(await window(64 * 1024 * 1024, 12), [1, 16 * 1024 * 1024, 0]);
});

test("a template of an unknown version or length is never filled", async () => {
  const f = fixture();
  f.telemetry.frame(0, true); f.advance(1500); f.telemetry.frame(0, true);
  const wrong = new Uint8Array(62); wrong[0] = 0x80; wrong[1] = 1;
  await f.telemetry.report(wrong);
  assert.equal(f.packets.length, 0);
});
