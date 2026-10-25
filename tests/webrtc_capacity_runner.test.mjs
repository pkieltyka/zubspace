import assert from "node:assert/strict";
import test from "node:test";

import "../js/zubspace_rtc_config.js";
import {
  capacityCdpRpcTimeoutMs,
  capacitySignalingExchange,
  evaluateCdpExpression,
  fetchJsonWithDeadline,
  runWithCleanup,
} from "../tools/webrtc_capacity_runner.mjs";

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

  fireActive() {
    const active = this.handles.filter((handle) => !handle.cleared);
    assert.equal(active.length, 1, "expected exactly one owned deadline");
    active[0].callback();
    return active[0];
  }
}

function createPeer({ stalledAnswer = false, onAnswer = () => {} } = {}) {
  return {
    localDescription: { type: "offer", sdp: "v=0" },
    closeCalls: 0,
    closed: false,
    setRemoteDescription() {
      onAnswer();
      return stalledAnswer ? new Promise(() => {}) : Promise.resolve();
    },
    close() {
      this.closeCalls += 1;
      this.closed = true;
    },
  };
}

function cleanupFixture(peer) {
  const resources = {
    browserProcesses: 1,
    serverProcesses: 1,
    peers: new Set(peer ? [peer] : []),
    cleanupCalls: 0,
  };
  return {
    resources,
    async cleanup() {
      resources.cleanupCalls += 1;
      for (const activePeer of resources.peers) {
        if (!activePeer.closed) activePeer.close();
      }
      resources.peers.clear();
      resources.browserProcesses = 0;
      resources.serverProcesses = 0;
    },
  };
}

function assertClean(resources) {
  assert.equal(resources.cleanupCalls, 1);
  assert.equal(resources.browserProcesses, 0);
  assert.equal(resources.serverProcesses, 0);
  assert.equal(resources.peers.size, 0);
}

function productionDeadlineFactory(timers) {
  return (options) =>
    globalThis.zubspaceCreateRtcSignalingDeadline({ ...options, timers });
}

test("capacity runner aborts a stalled signaling fetch and cleans every resource", async () => {
  const timers = new FakeTimers();
  const peer = createPeer();
  const fixture = cleanupFixture(peer);
  let started;
  const stageStarted = new Promise((resolve) => {
    started = resolve;
  });
  let signal;
  const operation = runWithCleanup(
    () =>
      capacitySignalingExchange({
        peer,
        offerUrl: "https://play.example.test/v1/rtc/offer",
        signalingToken: "",
        timeoutMs: 1000,
        createDeadline: productionDeadlineFactory(timers),
        fetchImpl: (_url, request) => {
          signal = request.signal;
          started();
          return new Promise(() => {});
        },
      }),
    fixture.cleanup,
  );
  await stageStarted;
  assert.equal(timers.fireActive().delay, 1000);
  await assert.rejects(operation, /signaling did not complete within 1000ms/);
  assert.equal(signal.aborted, true);
  assert.equal(peer.closeCalls, 1);
  assertClean(fixture.resources);
});

test("capacity runner bounds a stalled signaling response body and closes its peer", async () => {
  const timers = new FakeTimers();
  const peer = createPeer();
  const fixture = cleanupFixture(peer);
  let started;
  const stageStarted = new Promise((resolve) => {
    started = resolve;
  });
  const operation = runWithCleanup(
    () =>
      capacitySignalingExchange({
        peer,
        offerUrl: "https://play.example.test/v1/rtc/offer",
        timeoutMs: 1000,
        createDeadline: productionDeadlineFactory(timers),
        fetchImpl: async () => ({
          ok: true,
          json() {
            started();
            return new Promise(() => {});
          },
        }),
      }),
    fixture.cleanup,
  );
  await stageStarted;
  timers.fireActive();
  await assert.rejects(operation, /signaling did not complete within 1000ms/);
  assert.equal(peer.closeCalls, 1);
  assertClean(fixture.resources);
});

test("capacity runner bounds stalled remote-answer application and closes its peer", async () => {
  const timers = new FakeTimers();
  let started;
  const stageStarted = new Promise((resolve) => {
    started = resolve;
  });
  const peer = createPeer({ stalledAnswer: true, onAnswer: started });
  const fixture = cleanupFixture(peer);
  const operation = runWithCleanup(
    () =>
      capacitySignalingExchange({
        peer,
        offerUrl: "https://play.example.test/v1/rtc/offer",
        timeoutMs: 1000,
        createDeadline: productionDeadlineFactory(timers),
        fetchImpl: async () => ({
          ok: true,
          json: async () => ({ type: "answer", sdp: "v=0" }),
        }),
      }),
    fixture.cleanup,
  );
  await stageStarted;
  timers.fireActive();
  await assert.rejects(operation, /signaling did not complete within 1000ms/);
  assert.equal(peer.closeCalls, 1);
  assertClean(fixture.resources);
});

test("capacity runner aborts a stalled status request and runs process cleanup", async () => {
  const timers = new FakeTimers();
  const peer = createPeer();
  const fixture = cleanupFixture(peer);
  let started;
  const stageStarted = new Promise((resolve) => {
    started = resolve;
  });
  let signal;
  const operation = runWithCleanup(
    () =>
      fetchJsonWithDeadline("https://play.example.test/v1/status", {
        timeoutMs: 1000,
        createDeadline: productionDeadlineFactory(timers),
        label: "server status endpoint",
        fetchImpl: (_url, request) => {
          signal = request.signal;
          started();
          return new Promise(() => {});
        },
      }),
    fixture.cleanup,
  );
  await stageStarted;
  timers.fireActive();
  await assert.rejects(operation, /signaling did not complete within 1000ms/);
  assert.equal(signal.aborted, true);
  assertClean(fixture.resources);
});

test("capacity runner rejects a stalled CDP RPC and closes socket, peers, and processes", async () => {
  const timers = new FakeTimers();
  let evaluationStarted;
  const stageStarted = new Promise((resolve) => {
    evaluationStarted = resolve;
  });
  const sockets = [];
  class FakeWebSocket {
    constructor() {
      this.closed = false;
      this.closeCalls = 0;
      sockets.push(this);
      queueMicrotask(() => this.onopen?.());
    }

    send(raw) {
      const message = JSON.parse(raw);
      if (message.method === "Runtime.enable") {
        queueMicrotask(() =>
          this.onmessage?.({ data: JSON.stringify({ id: message.id, result: {} }) }),
        );
      } else {
        evaluationStarted();
      }
    }

    close() {
      this.closeCalls += 1;
      this.closed = true;
    }
  }

  const peer = createPeer();
  const fixture = cleanupFixture(peer);
  const operation = runWithCleanup(
    () =>
      evaluateCdpExpression("ws://127.0.0.1/devtools/page/stalled", "42", {
        timeoutMs: 1000,
        WebSocketImpl: FakeWebSocket,
        timers,
      }),
    fixture.cleanup,
  );
  await stageStarted;
  assert.equal(timers.fireActive().delay, 1000);
  await assert.rejects(operation, /CDP Runtime\.evaluate did not complete within 1000ms/);
  assert.equal(sockets.length, 1);
  assert.equal(sockets[0].closed, true);
  assert.equal(sockets[0].closeCalls, 1);
  assertClean(fixture.resources);
});

test("capacity CDP timeout covers signaling batches and hold time unless explicitly set", () => {
  assert.equal(
    capacityCdpRpcTimeoutMs({
      signalingTimeoutMs: 20_000,
      holdMs: 10_000,
      clientCount: 200,
      batchSize: 200,
    }),
    90_000,
  );
  assert.equal(
    capacityCdpRpcTimeoutMs({
      raw: "45000",
      signalingTimeoutMs: 20_000,
      holdMs: 10_000,
      clientCount: 200,
      batchSize: 200,
    }),
    45_000,
  );
  assert.throws(
    () =>
      capacityCdpRpcTimeoutMs({
        raw: "999",
        signalingTimeoutMs: 20_000,
        holdMs: 10_000,
        clientCount: 1,
        batchSize: 1,
      }),
    /1000\.\.86400000/,
  );
});
