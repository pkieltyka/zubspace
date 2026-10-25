#!/usr/bin/env node
import { fetchGameStatusResponse } from "./game_status.mjs";

import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";

import "../js/zubspace_rtc_config.js";
import {
  fetchJsonWithDeadline,
  fetchTextWithDeadline,
  openCdpSession,
  runCommandWithDeadline,
  terminateChildWithDeadline,
} from "./webrtc_capacity_runner.mjs";
import {
  assertRtcPressureClean,
  assertRtcServerLogClean,
} from "./rtc_gate_audit.mjs";
import { snapshotAckWithinHistory } from "./server_events.mjs";
import { browserLaunchEnvironment } from "./e2e_gate_hygiene.mjs";
import { composeRtcTopology, composeServerMaxPlayers } from "./compose_rtc_topology.mjs";
import { sourceRevision } from "./client_build_info.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const maxPlayers = composeServerMaxPlayers(fs.readFileSync(path.join(repoRoot, "config/zubspace_server.toml"), "utf8"));
const topology = composeRtcTopology(process.env, { maxPlayers });
const { clientPort, serverPort } = topology;
const clientUrl = `http://127.0.0.1:${clientPort}/index.html?name=composesmoke&arena=chaos-zone&packetlog=1&metrics=1`;
const offerUrl = `http://127.0.0.1:${clientPort}/v1/rtc/offer`;
const statusUrl = `http://127.0.0.1:${clientPort}/v1/status`;
const project = `zubspace-compose-rtc-${process.pid}`;
const chromeCandidates = process.env.CHROME_BIN
  ? [process.env.CHROME_BIN]
  : ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"];
const composeEnvironment = {
  ...process.env,
  COMPOSE_ANSI: "never",
  ZUBSPACE_CLIENT_PORT: String(clientPort),
  ZUBSPACE_BUILD_COMMIT: sourceRevision(repoRoot, { ...process.env, ZUBSPACE_REQUIRE_BUILD_REVISION: "1" }),
};
const httpTimeoutMs = 10_000;
const cdpTimeoutMs = 10_000;
const dockerTimeoutMs = environmentTimeout(
  "ZUBSPACE_COMPOSE_COMMAND_TIMEOUT_MS",
  10 * 60_000,
);
const dockerDiagnosticTimeoutMs = environmentTimeout(
  "ZUBSPACE_COMPOSE_DIAGNOSTIC_TIMEOUT_MS",
  15_000,
);
const dockerCleanupTimeoutMs = environmentTimeout(
  "ZUBSPACE_COMPOSE_CLEANUP_TIMEOUT_MS",
  30_000,
);

function environmentTimeout(name, fallback) {
  const raw = process.env[name];
  const value = raw === undefined || raw === "" ? fallback : Number(raw);
  if (!Number.isSafeInteger(value) || value < 100) {
    throw new Error(`${name} must be an integer of at least 100ms, got ${JSON.stringify(raw)}`);
  }
  return value;
}

async function waitUntil(label, predicate, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    const remainingMs = Math.max(1, deadline - Date.now());
    // HTTP/CDP discovery uses the production signaling deadline factory,
    // whose minimum budget is one second. Preserve the last real health
    // failure rather than replacing it with a configuration error on the
    // final sub-second poll. Never extend the original overall deadline.
    if (remainingMs < 1_000) {
      await sleep(remainingMs);
      break;
    }
    const controller = new AbortController();
    let timer;
    try {
      const value = await Promise.race([
        predicate({ timeoutMs: remainingMs, signal: controller.signal }),
        new Promise((_, reject) => {
          timer = setTimeout(
            () => reject(new Error(`${label} predicate exceeded its ${remainingMs}ms budget`)),
            remainingMs,
          );
        }),
      ]);
      if (value) return value;
    } catch (error) {
      lastError = error;
    } finally {
      if (timer !== undefined) clearTimeout(timer);
      controller.abort();
    }
    const retryDelayMs = Math.min(100, Math.max(0, deadline - Date.now()));
    if (retryDelayMs > 0) await sleep(retryDelayMs);
  }
  throw new Error(
    `timed out waiting for ${label}${lastError ? `: ${lastError.message}` : ""}`,
  );
}

async function assertTcpPortFree(port) {
  const probe = net.createServer();
  await new Promise((resolve, reject) => {
    probe.once("error", (error) =>
      reject(new Error(`TCP port ${port} is already in use: ${error.message}`)),
    );
    probe.listen(port, "127.0.0.1", resolve);
  });
  await new Promise((resolve) => probe.close(resolve));
}

async function freeTcpPort() {
  const probe = net.createServer();
  await new Promise((resolve, reject) => {
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", resolve);
  });
  const { port } = probe.address();
  await new Promise((resolve) => probe.close(resolve));
  return port;
}

async function compose(
  args,
  { check = true, timeoutMs = dockerTimeoutMs } = {},
) {
  let result;
  try {
    result = await runCommandWithDeadline(
      "docker",
      ["compose", "--project-name", project, ...args],
      {
        cwd: repoRoot,
        env: composeEnvironment,
        timeoutMs,
        termGraceMs: 1_500,
        killGraceMs: 1_500,
        label: `docker compose ${args.join(" ")}`,
      },
    );
  } catch (error) {
    result = error.commandResult || {
      error,
      status: null,
      stderr: error.message,
      stdout: "",
      timedOut: false,
    };
  }
  if (check && (result.error || result.status !== 0)) {
    const output = `${result.stdout || ""}${result.stderr || ""}`;
    throw new Error(
      `docker compose ${args.join(" ")} failed${result.timedOut ? " after its deadline" : ""}${result.error ? `: ${result.error.message}` : ""}\n${output.slice(-16_000)}`,
    );
  }
  return result;
}

async function status(timeoutMs = httpTimeoutMs) {
  return fetchJsonWithDeadline(statusUrl, {
    fetchImpl: fetchGameStatusResponse,
    timeoutMs: Math.min(httpTimeoutMs, timeoutMs),
    label: "Compose status endpoint",
    request: { cache: "no-store" },
  });
}

function assertIdleStatus(value, phase) {
  assertRtcPressureClean(value, phase, {requireIngressDrained: true});
  const nonzero = [
    "clients",
    "total",
    "joinedPlayers",
    "botPlayers",
    "rtc_sessions",
    "rtc_udp_ports_in_use",
    "rtc_outbound_queue_packets",
    "rtc_outbound_queue_bytes",
    "rtc_outbound_reliable_overflows",
    "rtc_channel_protocol_violations",
  ].filter((name) => value[name] !== 0);
  if (nonzero.length > 0 || value.rtc_udp_ports_degraded !== 0) {
    throw new Error(`${phase} Compose status was not clean: ${JSON.stringify(value)}`);
  }
}

async function startChrome(debugPort) {
  const userDataDir = path.join(os.tmpdir(), `zubspace-compose-rtc-${process.pid}`);
  fs.rmSync(userDataDir, { force: true, recursive: true });
  for (const binary of chromeCandidates) {
    let probe;
    try {
      probe = await runCommandWithDeadline(binary, ["--version"], {
        timeoutMs: httpTimeoutMs,
        termGraceMs: 500,
        killGraceMs: 500,
        label: `${binary} --version`,
      });
    } catch {
      continue;
    }
    if (probe.error || probe.status !== 0) continue;
    const logs = [];
    const child = spawn(binary, [
      "--headless=new",
      "--disable-gpu",
      "--no-sandbox",
      "--disable-dev-shm-usage",
      "--no-first-run",
      "--password-store=basic",
      "--remote-debugging-address=127.0.0.1",
      `--remote-debugging-port=${debugPort}`,
      `--user-data-dir=${userDataDir}`,
      "about:blank",
    ], {
      env: browserLaunchEnvironment(process.env, "compose smoke Chrome"),
    });
    child.stdout.on("data", (data) => logs.push(data.toString()));
    child.stderr.on("data", (data) => logs.push(data.toString()));
    child.on("error", (error) => logs.push(`Chrome process error: ${error.message}\n`));
    return { binary, child, logs, userDataDir };
  }
  throw new Error(`no Chrome/Chromium executable found (${chromeCandidates.join(", ")})`);
}

async function stopChild(child) {
  const stopped = await terminateChildWithDeadline(child);
  if (!stopped) throw new Error("Chrome did not exit after SIGTERM/SIGKILL deadlines");
}

async function chromeJson(debugPort, pathname, options = {}, timeoutMs = httpTimeoutMs) {
  return fetchJsonWithDeadline(`http://127.0.0.1:${debugPort}${pathname}`, {
    timeoutMs: Math.min(httpTimeoutMs, timeoutMs),
    label: `Chrome ${pathname}`,
    request: options,
  });
}

async function openChromePage(debugPort, url, chrome) {
  await waitUntil("Chrome debugging endpoint", async ({ timeoutMs }) => {
    if (chrome.child.exitCode !== null) {
      throw new Error(`Chrome exited early\n${chrome.logs.join("").slice(-8_000)}`);
    }
    return chromeJson(debugPort, "/json/version", {}, timeoutMs);
  });
  const target = await chromeJson(
    debugPort,
    `/json/new?${encodeURIComponent(url)}`,
    { method: "PUT" },
    httpTimeoutMs,
  );
  if (!target.webSocketDebuggerUrl) {
    throw new Error(`Chrome did not expose the new page: ${JSON.stringify(target)}`);
  }
  return connectCdp(target.webSocketDebuggerUrl);
}

async function connectCdp(webSocketDebuggerUrl) {
  const exceptions = [];
  const session = await openCdpSession(webSocketDebuggerUrl, {
    timeoutMs: cdpTimeoutMs,
    onEvent(message) {
      if (message.method === "Runtime.exceptionThrown") {
        exceptions.push(message.params.exceptionDetails);
      }
    },
  });
  await session.send("Runtime.enable");
  await session.send("Page.enable");
  return {
    exceptions,
    send: session.send,
    async evaluate(expression) {
      const response = await session.send("Runtime.evaluate", {
        expression,
        awaitPromise: true,
        returnByValue: true,
      });
      if (response.exceptionDetails) {
        throw new Error(
          response.exceptionDetails.exception?.description || response.exceptionDetails.text,
        );
      }
      return response.result.value;
    },
    close() {
      session.close();
    },
  };
}

const pageStateExpression = `(() => {
  const networkLog = window.__zubspaceNetworkLog || [];
  const channelLabels = networkLog
    .filter((entry) => entry.message === "rtc data channel open")
    .map((entry) => entry.detail?.label);
  return {
    readyState: document.readyState,
    hasCanvas: !!document.querySelector("canvas"),
    hasWasm: performance.getEntriesByType("resource").some((entry) => /zubspace\\.wasm/.test(entry.name)),
    rtcStatus: window.__zubspaceRtcStatusForTest?.(),
    rtcError: window.__zubspaceRtcLastErrorForTest?.(),
    offerUrl: networkLog.find((entry) => entry.message === "connecting")?.detail?.offerUrl,
    acceptedAnswer: networkLog.some((entry) => entry.message === "rtc answer accepted"),
    channelLabels,
    metrics: window.__zubspaceUiMetrics || null,
    networkLog,
  };
})()`;

async function main() {
  let chrome = null;
  let page = null;
  let pageState = null;
  let during = null;
  let after = null;
  let testError = null;
  let composeMayOwnResources = false;
  let overrideDirectory = null;

  try {
    if (process.platform !== "linux") {
      throw new Error("the checked-in Compose topology requires Docker Engine host networking on Linux");
    }
    await assertTcpPortFree(clientPort);
    await assertTcpPortFree(serverPort);
    if (topology.override) {
      overrideDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "zubspace-compose-ports-"));
      const overrideFile = path.join(overrideDirectory, "ports.json");
      fs.writeFileSync(overrideFile, JSON.stringify(topology.override));
      composeEnvironment.COMPOSE_FILE = [path.join(repoRoot, "docker-compose.yml"), overrideFile].join(path.delimiter);
    }
    // Keep progress text distinct from the one JSON verdict consumed by the
    // retained gate's strict result-count and pressure/security audits.
    process.stdout.write(`Compose topology: client=${clientPort} server=${serverPort} udp=${topology.udpPortRange} isolated=${!!topology.override}\n`);
    await compose(["config", "--quiet"]);
    process.stdout.write("Building and starting the real Compose images...\n");
    composeMayOwnResources = true;
    await compose(["up", "--build", "--detach", "--remove-orphans"]);
    const before = await waitUntil("Compose nginx and server health", async ({ timeoutMs }) => {
      const value = await status(timeoutMs);
      assertIdleStatus(value, "initial");
      return value.rtc_effective_capacity === maxPlayers ? value : null;
    }, 90_000);
    const pressureBefore = assertRtcPressureClean(before, "Compose initial");

    // A caller cannot inject its own proxy identity through nginx: the proxy
    // overwrites forwarding headers and the direct server listener is loopback-only.
    const spoofAddress = "198.51.100.77";
    const spoofResponse = await fetchTextWithDeadline(offerUrl, {
      timeoutMs: httpTimeoutMs,
      label: "nginx signaling identity probe",
      request: {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-Real-IP": spoofAddress,
          "X-Forwarded-For": spoofAddress,
          "X-Forwarded-Proto": "https",
        },
        body: "{}",
      },
    });
    if (spoofResponse.status !== 400) {
      throw new Error(`nginx signaling probe returned HTTP ${spoofResponse.status}`);
    }
    const serverLogs = (await compose(["logs", "--no-color", "zubspace-server"])).stdout || "";
    if (serverLogs.includes(spoofAddress)) {
      throw new Error(`nginx exposed caller-supplied proxy identity to the server: ${spoofAddress}`);
    }

    const debugPort = await freeTcpPort();
    chrome = await startChrome(debugPort);
    page = await openChromePage(debugPort, clientUrl, chrome);
    pageState = await waitUntil("Compose WebRTC channels and gameplay snapshot ACK", async () => {
      const value = await page.evaluate(pageStateExpression);
      if (value.rtcStatus === 4) {
        throw new Error(`browser RTC failed: ${value.rtcError}; ${JSON.stringify(value.networkLog)}`);
      }
      const channels = new Set(value.channelLabels);
      const metrics = value.metrics;
      return value.readyState === "complete" &&
        value.hasCanvas &&
        value.hasWasm &&
        value.rtcStatus === 2 &&
        value.offerUrl === offerUrl &&
        value.acceptedAnswer &&
        ["zubspace-game", "zubspace-events", "zubspace-reliable"].every((label) => channels.has(label)) &&
        Number.isFinite(metrics?.selfPlayerId) &&
        snapshotAckWithinHistory(
          metrics?.latestSnapshotId,
          metrics?.lastAckedSnapshotId,
        )
        ? value
        : null;
    }, 45_000);
    during = await waitUntil("joined Compose gameplay status", async ({ timeoutMs }) => {
      const value = await status(timeoutMs);
      if (!(value.clients === 1 &&
        value.joinedPlayers === 1 &&
        value.rtc_sessions === 1 &&
        value.rtc_udp_ports_in_use === 1 &&
        value.snapshot_acked_clients === 1)) return null;
      try {
        assertRtcPressureClean(value, "Compose joined gameplay");
        return value;
      } catch {
        return null;
      }
    }, 15_000);
    const pressureDuring = assertRtcPressureClean(during, "Compose joined gameplay");
    if (
      during.rtc_udp_ports_degraded !== 0 ||
      during.rtc_outbound_reliable_overflows !== 0 ||
      during.rtc_channel_protocol_violations !== 0 ||
      page.exceptions.length !== 0
    ) {
      throw new Error(
        `Compose RTC showed degradation, protocol failures, or browser exceptions: ${JSON.stringify({ during, exceptions: page.exceptions })}`,
      );
    }

    await page.send("Page.navigate", { url: "about:blank" });
    after = await waitUntil("Compose peer cleanup", async ({ timeoutMs }) => {
      const value = await status(timeoutMs);
      try {
        assertIdleStatus(value, "post-browser");
        return value;
      } catch {
        return null;
      }
    }, 20_000);
    const pressureAfter = assertRtcPressureClean(after, "Compose post-browser", {requireIngressDrained: true});
    const finalComposeLogs = await compose(["logs", "--no-color"]);
    const retainedComposeLogs = `${finalComposeLogs.stdout || ""}${finalComposeLogs.stderr || ""}`;
    const serverLogAudit = assertRtcServerLogClean(retainedComposeLogs, "Compose final", {
      requireRuntimeEvidence: true,
    });
    process.stdout.write(retainedComposeLogs);
    process.stdout.write(
      `${JSON.stringify({
        ok: true,
        browser: chrome.binary,
        topology: { clientPort, serverPort, udpPortRange: topology.udpPortRange, maxPlayers },
        clientUrl,
        offerUrl: pageState.offerUrl,
        channels: [...new Set(pageState.channelLabels)].sort(),
        snapshot: {
          latest: pageState.metrics.latestSnapshotId,
          acked: pageState.metrics.lastAckedSnapshotId,
        },
        pressureAudit: {
          before: pressureBefore,
          during: pressureDuring,
          after: pressureAfter,
        },
        serverLogAudit,
        status: {
          before: {
            rtcSessions: before.rtc_sessions,
            clients: before.clients,
            udpPortsInUse: before.rtc_udp_ports_in_use,
          },
          during: {
            rtcSessions: during.rtc_sessions,
            clients: during.clients,
            players: during.joinedPlayers,
            udpPortsInUse: during.rtc_udp_ports_in_use,
            snapshotAckedClients: during.snapshot_acked_clients,
          },
          after: {
            rtcSessions: after.rtc_sessions,
            clients: after.clients,
            udpPortsInUse: after.rtc_udp_ports_in_use,
          },
        },
      }, null, 2)}\n`,
    );
  } catch (error) {
    testError = error;
    if (composeMayOwnResources) {
      const diagnostics = await compose(["ps"], {
        check: false,
        timeoutMs: dockerDiagnosticTimeoutMs,
      });
      const logs = await compose(["logs", "--no-color", "--tail", "200"], {
        check: false,
        timeoutMs: dockerDiagnosticTimeoutMs,
      });
      process.stderr.write(`${diagnostics.stdout || ""}${diagnostics.stderr || ""}`);
      process.stderr.write(`${logs.stdout || ""}${logs.stderr || ""}`);
    }
  } finally {
    const cleanupErrors = [];
    try {
      page?.close();
    } catch (error) {
      cleanupErrors.push(error);
    }
    try {
      await stopChild(chrome?.child);
    } catch (error) {
      cleanupErrors.push(error);
    }
    try {
      if (chrome?.userDataDir) fs.rmSync(chrome.userDataDir, { force: true, recursive: true });
    } catch (error) {
      cleanupErrors.push(error);
    }
    if (composeMayOwnResources) {
      let down = await compose(["down", "--remove-orphans"], {
        check: false,
        timeoutMs: dockerCleanupTimeoutMs,
      });
      if (down.error || down.status !== 0) {
        cleanupErrors.push(
          new Error(`docker compose down failed\n${down.stdout || ""}${down.stderr || ""}`),
        );
        await compose(["kill"], {
          check: false,
          timeoutMs: dockerCleanupTimeoutMs,
        });
        down = await compose(["down", "--remove-orphans"], {
          check: false,
          timeoutMs: dockerCleanupTimeoutMs,
        });
        if (down.error || down.status !== 0) {
          cleanupErrors.push(
            new Error(`docker compose fallback cleanup failed\n${down.stdout || ""}${down.stderr || ""}`),
          );
        }
      }
    }
    if (overrideDirectory && cleanupErrors.length === 0) {
      try { fs.rmSync(overrideDirectory, { recursive: true, force: true }); }
      catch (error) { cleanupErrors.push(error); }
    } else if (overrideDirectory) {
      process.stderr.write(`retained Compose port override after cleanup failure: ${overrideDirectory}\n`);
    }
    if (cleanupErrors.length > 0) {
      const cleanupError = new AggregateError(cleanupErrors, "Compose smoke cleanup failed");
      if (!testError) testError = cleanupError;
      else process.stderr.write(`${cleanupError.stack || cleanupError.message}\n`);
    }
  }
  if (testError) throw testError;
}

await main();
