import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { observeProbeTiming } from "./udp_probe_timing.mjs";

test("passive probe attribution counts only native sends that actually return", () => {
  let now = 10, sendCount = 0, fail = false;
  const listeners = new Map();
  const channel = { label: "zubspace-game", bufferedAmount: 5,
    addEventListener: (event, callback) => listeners.set(event, callback),
    send(bytes) { sendCount++; if (fail) throw new Error("native send failed"); assert.equal(bytes.byteLength, 30); return "native-return"; } };
  const context = vm.createContext({ window: { RTCPeerConnection: class { createDataChannel() { return channel; } } },
    ArrayBuffer, Uint8Array, DataView, performance: { now: () => now }, requestAnimationFrame() {} });
  vm.runInContext(`(${observeProbeTiming.toString()})()`, context);
  const peer = new context.window.RTCPeerConnection();
  const actual = peer.createDataChannel("zubspace-game");
  const packet = (type, sequence = 1) => {
    const bytes = new Uint8Array(30); bytes[0] = type; bytes[1] = 1;
    const view = new DataView(bytes.buffer); view.setBigUint64(2, 7n, true);
    view.setBigUint64(10, 9n, true); view.setUint32(18, sequence, true); return bytes;
  };
  listeners.get("message")({ data: packet(0x81).buffer });
  const row = context.window.__udpProbeTiming.rows[0];
  assert.equal(sendCount, 0, "observing cannot emit an echo");
  now = 13; fail = true;
  assert.throws(() => actual.send(packet(0x82)), /native send failed/);
  assert.equal(row.echoedAt, null); assert.equal(row.echoCount, 0);
  assert.equal(row.sendError, "native send failed");
  fail = false; now = 15;
  assert.equal(actual.send(packet(0x82)), "native-return");
  assert.equal(row.echoedAt, 15); assert.equal(row.echoCount, 1);
  assert.equal(row.bufferedBytes, 5);
  actual.send(packet(0x82, 2));
  assert.equal(row.echoCount, 1, "an unrelated probe identity cannot satisfy this row");
  listeners.get("message")({ data: packet(0x81).buffer });
  assert.equal(row.receiveCount, 2);
  assert.equal(context.window.__udpProbeTiming.rows.length, 1);
});
