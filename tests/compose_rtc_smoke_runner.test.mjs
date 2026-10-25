import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";
import { composeRtcTopology, composeServerMaxPlayers } from "../tools/compose_rtc_topology.mjs";

import "../js/zubspace_rtc_config.js";

async function actualComposeFunction(name, nextName, context) {
  const source = await readFile(new URL("../tools/compose_rtc_smoke.mjs", import.meta.url), "utf8");
  const declaration = source.indexOf(`function ${name}(`);
  const next = source.indexOf(`function ${nextName}(`, declaration + 1);
  assert.ok(declaration >= 0 && next > declaration);
  const body = source.slice(declaration, next).replace(/async\s*$/, "");
  return vm.runInNewContext(`(${name === "waitUntil" ? "async " : ""}${body})`, context);
}

test("Compose idle accepts the Part A counts and rejects any remaining membership or transport work", async () => {
  const pressure = [];
  const idle = await actualComposeFunction("assertIdleStatus", "startChrome", {
    assertRtcPressureClean: (...args) => pressure.push(args),
  });
  const empty = { total: 0, joinedPlayers: 0, botPlayers: 0, clients: 0, rtc_sessions: 0,
    rtc_udp_ports_in_use: 0, rtc_udp_ports_degraded: 0, rtc_outbound_queue_packets: 0,
    rtc_outbound_queue_bytes: 0, rtc_outbound_reliable_overflows: 0, rtc_channel_protocol_violations: 0 };
  assert.doesNotThrow(() => idle(empty, "initial"));
  assert.equal(pressure[0][2].requireIngressDrained, true);
  for (const name of Object.keys(empty)) {
    assert.throws(() => idle({ ...empty, [name]: 1 }, "initial"), /was not clean/, name);
  }
});

test("Compose polling retains the health failure when its final budget is below the signaling factory minimum", async () => {
  let now = 0;
  const budgets = [];
  const wait = await actualComposeFunction("waitUntil", "assertTcpPortFree", {
    Date: { now: () => now }, AbortController, Promise, Math,
    setTimeout: () => 1, clearTimeout: () => {}, sleep: async ms => { now += ms; },
  });
  await assert.rejects(wait("health", async ({ timeoutMs }) => {
    budgets.push(timeoutMs);
    if (timeoutMs < 1000) globalThis.zubspaceCreateRtcSignalingDeadline({ timeoutMs });
    throw new Error("initial Compose status was not clean");
  }, 2000), /initial Compose status was not clean/);
  assert.equal(now, 2000, "the original overall deadline is unchanged");
  assert.ok(budgets.every(value => value >= 1000));
});

test("shipped Compose and Docker UDP ranges cover the actual server player limit", async () => {
  const config = await readFile(new URL("../config/zubspace_server.toml", import.meta.url), "utf8");
  const compose = await readFile(new URL("../docker-compose.yml", import.meta.url), "utf8");
  const dockerfile = await readFile(new URL("../Dockerfile.server", import.meta.url), "utf8");
  const maxPlayers = Number(config.match(/^max_players\s*=\s*(\d+)\s*$/m)?.[1]);
  assert.equal(maxPlayers, 500, "retain the configured 500-player arena limit");
  const range = compose.match(/ZUBSPACE_SERVER_UDP_PORT_RANGE:\s*"(\d+)-(\d+)"/);
  assert.ok(range, "Compose must publish an explicit UDP range");
  assert.ok(Number(range[2]) - Number(range[1]) + 1 >= maxPlayers,
    "one-port-per-peer Compose range must cover max_players");
  assert.match(dockerfile, new RegExp(`^EXPOSE 9001/tcp ${range[1]}-${range[2]}/udp$`, "m"));
  assert.equal(composeRtcTopology({}).udpPortRange, `${range[1]}-${range[2]}`);
});

test("Compose smoke keeps default listeners and supports a complete isolated topology", () => {
  assert.deepEqual(composeRtcTopology({}), { clientPort: 7000, serverPort: 9001, udpPortRange: "9002-9501", override: null });
  const result = composeRtcTopology({ ZUBSPACE_COMPOSE_CLIENT_PORT: "18000", ZUBSPACE_COMPOSE_SERVER_PORT: "18001",
    ZUBSPACE_COMPOSE_UDP_PORT_RANGE: "18002-18501" });
  assert.equal(result.clientPort, 18000);
  assert.equal(result.serverPort, 18001);
  const server = result.override.services["zubspace-server"], client = result.override.services["zubspace-client"];
  assert.equal(server.environment.ZUBSPACE_SERVER_UDP_PORT_RANGE, "18002-18501");
  assert.equal(server.environment.ZUBSPACE_SERVER_BIND, client.environment.ZUBSPACE_SERVER_UPSTREAM);
  assert.equal(server.healthcheck.test, "curl --fail --silent --show-error http://127.0.0.1:18001/v1/status");
  assert.equal(client.environment.ZUBSPACE_CLIENT_LISTEN_PORT, "18000");
  // Overrides may change listeners, never networking mode, image or capacity.
  assert.deepEqual(Object.keys(server).sort(), ["environment", "healthcheck"]);
  assert.deepEqual(Object.keys(client), ["environment"]);
});

test("Compose smoke rejects partial, conflicting or capacity-reducing listener overrides", () => {
  const valid = { ZUBSPACE_COMPOSE_CLIENT_PORT: "18000", ZUBSPACE_COMPOSE_SERVER_PORT: "18001",
    ZUBSPACE_COMPOSE_UDP_PORT_RANGE: "18002-18501" };
  assert.throws(() => composeRtcTopology({ ZUBSPACE_COMPOSE_CLIENT_PORT: "18000" }), /require all/);
  for (const value of ["", "0", "01", "65536", "-1", "1.5", "foo"])
    assert.throws(() => composeRtcTopology({ ...valid, ZUBSPACE_COMPOSE_CLIENT_PORT: value }), /must be a port/);
  assert.throws(() => composeRtcTopology({ ...valid, ZUBSPACE_COMPOSE_SERVER_PORT: "18000" }), /must differ/);
  for (const range of ["18002-18201", "18002-18500", "18501-18002", "65337-65536", "bad"])
    assert.throws(() => composeRtcTopology({ ...valid, ZUBSPACE_COMPOSE_UDP_PORT_RANGE: range }), /range|500 UDP ports|must be a port/);
});

test("Compose capacity follows the checked-in server table and fails closed", () => {
  assert.equal(composeServerMaxPlayers("[server]\nmax_players = 500 # players\n[other]\nmax_players = 1\n"), 500);
  assert.equal(composeServerMaxPlayers("[server]\nmax_players = 5_00\n"), 500);
  for (const config of ["", "[other]\nmax_players=500", "[server]\n", "[server]\nmax_players=500\nmax_players=200",
    "[server]\nmax_players=500\n[server]\n", "[server]\nmax_players=0", "[server]\nmax_players=769",
    '[server]\nmax_players="500"', "[server]\nmax_players=500.5", "[server]\nmax_players=invalid"])
    assert.throws(() => composeServerMaxPlayers(config), /max_players/);
  const listeners = { ZUBSPACE_COMPOSE_CLIENT_PORT: "18000", ZUBSPACE_COMPOSE_SERVER_PORT: "18001",
    ZUBSPACE_COMPOSE_UDP_PORT_RANGE: "18002-18501" };
  assert.throws(() => composeRtcTopology(listeners, { maxPlayers: 501 }), /at least 501 UDP ports/);
  assert.throws(() => composeRtcTopology({}, { maxPlayers: 501 }), /at least 501 UDP ports/);
  assert.equal(composeRtcTopology({ ...listeners, ZUBSPACE_COMPOSE_UDP_PORT_RANGE: "18002-18601" },
    { maxPlayers: 600 }).udpPortRange, "18002-18601");
  for (const maxPlayers of [0, 769, 1.5, NaN, "500"])
    assert.throws(() => composeRtcTopology({}, { maxPlayers }), /max_players/);
});

import {
  evaluateCdpExpression,
  fetchJsonWithDeadline,
  openCdpSession,
  runCommandWithDeadline,
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
    active[0].cleared = true;
    active[0].callback();
  }
}

function productionDeadlineFactory(timers) {
  return (options) =>
    globalThis.zubspaceCreateRtcSignalingDeadline({ ...options, timers });
}

function cleanupFixture() {
  const resources = {
    children: new Set(),
    sockets: new Set(),
    containers: new Set(["compose-server", "compose-nginx"]),
    cleanupCalls: 0,
  };
  return {
    resources,
    async cleanup() {
      resources.cleanupCalls += 1;
      for (const child of resources.children) {
        if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
      }
      resources.children.clear();
      for (const socket of resources.sockets) socket.close();
      resources.sockets.clear();
      resources.containers.clear();
    },
  };
}

function assertClean(resources) {
  assert.equal(resources.cleanupCalls, 1);
  assert.equal(resources.children.size, 0, "no child process may survive cleanup");
  assert.equal(resources.sockets.size, 0, "no debugger socket may survive cleanup");
  assert.equal(resources.containers.size, 0, "no Compose container may survive cleanup");
}

test("Compose Dockerfiles copy every patched vendored crate", async () => {
  const cargoToml = await readFile(new URL("../Cargo.toml", import.meta.url), "utf8");
  const patchSection = cargoToml.match(/\[patch\.crates-io\]([\s\S]*?)(?:\n\[|$)/)?.[1];
  assert.ok(patchSection, "Cargo.toml must retain an explicit crates.io patch section");
  const patchPaths = [...patchSection.matchAll(/\bpath\s*=\s*"([^"]+)"/g)].map(
    (match) => match[1],
  );
  assert.ok(patchPaths.length > 0, "the patched vendor path inventory must not be empty");

  for (const dockerfileName of ["Dockerfile.server", "Dockerfile.client"]) {
    const dockerfile = await readFile(new URL(`../${dockerfileName}`, import.meta.url), "utf8");
    const dockerfileLines = new Set(dockerfile.split(/\r?\n/));
    for (const patchPath of patchPaths) {
      assert.ok(
        dockerfileLines.has(`COPY ${patchPath} ./${patchPath}`),
        `${dockerfileName} must copy patched crate ${patchPath}`,
      );
    }
  }
});

test("Compose smoke bounds a stalled status request and leaves no resources", async () => {
  const timers = new FakeTimers();
  const fixture = cleanupFixture();
  let started;
  const requestStarted = new Promise((resolve) => {
    started = resolve;
  });
  let signal;
  const operation = runWithCleanup(
    () =>
      fetchJsonWithDeadline("http://127.0.0.1:7000/v1/status", {
        timeoutMs: 1000,
        createDeadline: productionDeadlineFactory(timers),
        label: "Compose status endpoint",
        fetchImpl: (_url, request) => {
          signal = request.signal;
          started();
          return new Promise(() => {});
        },
      }),
    fixture.cleanup,
  );
  await requestStarted;
  timers.fireActive();
  await assert.rejects(operation, /did not complete within 1000ms/);
  assert.equal(signal.aborted, true);
  assertClean(fixture.resources);
});

test("Compose smoke bounds a stalled Chrome JSON body and leaves no resources", async () => {
  const timers = new FakeTimers();
  const fixture = cleanupFixture();
  let started;
  const bodyStarted = new Promise((resolve) => {
    started = resolve;
  });
  const operation = runWithCleanup(
    () =>
      fetchJsonWithDeadline("http://127.0.0.1:9222/json/version", {
        timeoutMs: 1000,
        createDeadline: productionDeadlineFactory(timers),
        label: "Chrome /json/version",
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
  await bodyStarted;
  timers.fireActive();
  await assert.rejects(operation, /did not complete within 1000ms/);
  assertClean(fixture.resources);
});

test("Compose smoke bounds a stalled CDP WebSocket open and closes its socket", async () => {
  const timers = new FakeTimers();
  const fixture = cleanupFixture();
  class StalledWebSocket {
    constructor() {
      this.closed = false;
      fixture.resources.sockets.add(this);
    }

    close() {
      this.closed = true;
      fixture.resources.sockets.delete(this);
    }
  }

  const operation = runWithCleanup(
    () =>
      openCdpSession("ws://127.0.0.1/devtools/page/stalled-open", {
        timeoutMs: 1000,
        WebSocketImpl: StalledWebSocket,
        timers,
      }),
    fixture.cleanup,
  );
  timers.fireActive();
  await assert.rejects(operation, /CDP WebSocket did not open within 1000ms/);
  assertClean(fixture.resources);
});

test("Compose smoke bounds a stalled CDP RPC and closes its socket", async () => {
  const timers = new FakeTimers();
  const fixture = cleanupFixture();
  let started;
  const rpcStarted = new Promise((resolve) => {
    started = resolve;
  });
  class StalledRpcWebSocket {
    constructor() {
      this.closed = false;
      fixture.resources.sockets.add(this);
      queueMicrotask(() => this.onopen?.());
    }

    send(raw) {
      const message = JSON.parse(raw);
      if (message.method === "Runtime.enable") {
        queueMicrotask(() =>
          this.onmessage?.({ data: JSON.stringify({ id: message.id, result: {} }) }),
        );
      } else {
        started();
      }
    }

    close() {
      this.closed = true;
      fixture.resources.sockets.delete(this);
    }
  }

  const operation = runWithCleanup(
    () =>
      evaluateCdpExpression("ws://127.0.0.1/devtools/page/stalled-rpc", "42", {
        timeoutMs: 1000,
        WebSocketImpl: StalledRpcWebSocket,
        timers,
      }),
    fixture.cleanup,
  );
  await rpcStarted;
  timers.fireActive();
  await assert.rejects(operation, /CDP Runtime\.evaluate did not complete within 1000ms/);
  assertClean(fixture.resources);
});

test("Compose smoke kills a stalled Docker command and preserves diagnostics", async () => {
  const fixture = cleanupFixture();
  let child;
  const operation = runWithCleanup(
    async () => {
      try {
        await runCommandWithDeadline(
          process.execPath,
          [
            "-e",
            "process.on('SIGTERM', () => {}); process.stdout.write('before-hang'); setInterval(() => {}, 1000)",
          ],
          {
            timeoutMs: 250,
            termGraceMs: 100,
            killGraceMs: 500,
            label: "stalled docker compose command",
            spawnImpl(...args) {
              child = spawn(...args);
              fixture.resources.children.add(child);
              child.once("close", () => fixture.resources.children.delete(child));
              return child;
            },
          },
        );
        assert.fail("stalled command unexpectedly completed");
      } catch (error) {
        assert.match(error.message, /did not exit within 250ms/);
        assert.match(error.commandResult.stdout, /before-hang/);
        assert.deepEqual(error.commandResult.killSignals, ["SIGTERM", "SIGKILL"]);
        assert.equal(child.signalCode, "SIGKILL");
        assert.equal(fixture.resources.children.size, 0, "runner must reap its timed-out child");
        throw error;
      }
    },
    fixture.cleanup,
  );
  await assert.rejects(operation, /stalled docker compose command/);
  assertClean(fixture.resources);
});
