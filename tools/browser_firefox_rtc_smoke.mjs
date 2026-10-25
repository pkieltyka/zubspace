#!/usr/bin/env node
import { fetchGameStatusResponse } from "./game_status.mjs";
import { serverLogField } from "./server_log_fields.mjs";

import fs from "node:fs";
import { hudLineCountMatchesMode, smokeLiveStateExpression } from "./smoke_ui_contract.mjs";
import { clientBuildInfo, stampClientBuildInfo } from "./client_build_info.mjs";
import { commandWithCpuSet } from "./cpu_affinity.mjs";
import http from "node:http";
import { createGameHttpProxy } from "./game_http_proxy.mjs";
import net from "node:net";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  assertRtcPressureClean,
  assertRtcServerLogClean,
} from "./rtc_gate_audit.mjs";
import { assertCleanGateEnvironment, assertMachineHeadroom, browserLaunchEnvironment, killProcessGroup, registerGateProcess, rememberOwnedXDisplays, finishOwnedXDisplayCleanup } from "./e2e_gate_hygiene.mjs";
import { parseRealtimeWeaponEvent } from "./realtime_event_evidence.mjs";
import { assertRendererConsoleClean, openFirefoxConsole, browserConsoleEvidence } from "./browser_console_evidence.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
export function serverBinaryForProfile(profile = "debug") {
  if (!["debug", "release"].includes(profile)) {
    throw new Error(`unsupported E2E server profile: ${profile}`);
  }
  return `target/${profile}/zubspace_server`;
}
const geckodriverBin = process.env.GECKODRIVER_BIN || "geckodriver";
const firefoxBin = process.env.FIREFOX_BIN || "firefox";

// `ZUBSPACE_FIREFOX_BROWSER_ENV` reaches the browser process only, as geckodriver's
// `moz:firefoxOptions.env`. It exists because some Firefox diagnostics cannot be set in the
// harness or geckodriver environment at all: `MOZ_PROFILER_SHUTDOWN` in geckodriver's own
// environment makes its binary sniff fail every session with "binary is not a Firefox
// executable", so a startup-profiling capture has to be delivered to the browser alone.
// Accepts a JSON object or a `;`/newline-separated KEY=VALUE list.
function firefoxBrowserEnv() {
  const raw = process.env.ZUBSPACE_FIREFOX_BROWSER_ENV?.trim();
  if (!raw) return null;
  let parsed = null;
  if (raw.startsWith("{")) {
    try { parsed = JSON.parse(raw); } catch {
      throw new Error("ZUBSPACE_FIREFOX_BROWSER_ENV is not valid JSON");
    }
  } else {
    parsed = {};
    for (const pair of raw.split(/[;\n]/)) {
      if (!pair.trim()) continue;
      const index = pair.indexOf("=");
      if (index <= 0) throw new Error(`ZUBSPACE_FIREFOX_BROWSER_ENV entry is not KEY=VALUE: ${pair}`);
      parsed[pair.slice(0, index).trim()] = pair.slice(index + 1).trim();
    }
  }
  for (const [key, value] of Object.entries(parsed)) {
    if (typeof value !== "string") {
      throw new Error(`ZUBSPACE_FIREFOX_BROWSER_ENV[${key}] must be a string`);
    }
  }
  return parsed;
}
const firefoxHeadless = process.env.ZUBSPACE_FIREFOX_HEADLESS !== "0";
// Window class carried by every headed test browser (Chrome and Firefox).
// scripts/e2e-hyprland-tools matches exactly this class to park test windows
// on a background Hyprland workspace without touching the user's own
// browser windows. Keep the default in sync with those scripts.
export const e2eWindowClass = process.env.ZUBSPACE_E2E_WINDOW_CLASS || "zubspace-e2e";
const smokeTickRate = process.env.ZUBSPACE_SMOKE_TICK_RATE || "60";
const smokeBotCount = process.env.ZUBSPACE_SMOKE_BOT_COUNT || "";
const smokeLogLevel = process.env.ZUBSPACE_SMOKE_LOG_LEVEL || "debug";
const sampleMs = Number(process.env.ZUBSPACE_SMOKE_POSITION_SAMPLE_MS || "3000");
const configuredWebdriverTimeoutMs = Number(
  process.env.ZUBSPACE_FIREFOX_WEBDRIVER_TIMEOUT_MS || "30000",
);
const webdriverTimeoutMs = Number.isFinite(configuredWebdriverTimeoutMs) &&
    configuredWebdriverTimeoutMs > 0
  ? Math.round(configuredWebdriverTimeoutMs)
  : 30_000;

const KEY_ARROW_UP = "\uE013";
const KEY_ARROW_DOWN = "\uE015";
const KEY_ARROW_LEFT = "\uE012";
const KEY_ARROW_RIGHT = "\uE014";
const KEY_SHIFT = "\uE008";
const KEY_CONTROL = "\uE009";

const WEBDRIVER_KEY = {
  Escape: "\uE00C",
  ArrowUp: KEY_ARROW_UP,
  ArrowDown: KEY_ARROW_DOWN,
  ArrowLeft: KEY_ARROW_LEFT,
  ArrowRight: KEY_ARROW_RIGHT,
  Shift: KEY_SHIFT,
  Control: KEY_CONTROL,
  Space: " ",
};

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function mimeType(filename) {
  if (filename.endsWith(".html")) return "text/html; charset=utf-8";
  if (filename.endsWith(".js")) return "text/javascript; charset=utf-8";
  if (filename.endsWith(".wasm")) return "application/wasm";
  if (filename.endsWith(".css")) return "text/css; charset=utf-8";
  if (filename.endsWith(".png")) return "image/png";
  if (filename.endsWith(".wav") || filename.endsWith(".wa2")) return "audio/wav";
  return "application/octet-stream";
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const port = server.address().port;
      server.close(() => resolve(port));
    });
  });
}

// `ZUBSPACE_E2E_STATIC_ROOT` redirects the *application* bundle the browsers load, which is
// how the moving-peer cadence numbers can be taken against the shipped `dist/` client instead
// of the debug WASM that the repo-root `index.html` points at (the debug profile is built with
// opt-level=1 plus debug and overflow checks, so it is several times more expensive per frame).
// Harness fixtures under `/tests/**` still resolve against the repository, because they are not
// part of the bundle and `dist/` deliberately does not contain them. Both roots are reported by
// `/v1/status` so an artifact can never be read back without knowing which client it drove.
function staticServeRoots() {
  const override = process.env.ZUBSPACE_E2E_STATIC_ROOT?.trim();
  if (!override) return { app: repoRoot, fixture: repoRoot };
  const app = path.resolve(repoRoot, override);
  if (!fs.existsSync(path.join(app, "index.html"))) {
    throw new Error(`ZUBSPACE_E2E_STATIC_ROOT=${app} has no index.html; run \`make build-client-release\` first`);
  }
  return { app, fixture: repoRoot };
}

function startStaticServer({ host = "127.0.0.1", rtcOfferUrl = null } = {}) {
  const roots = staticServeRoots();
  const gameProxy = rtcOfferUrl ? createGameHttpProxy(rtcOfferUrl) : null;
  return new Promise((resolve, reject) => {
    const startedAtMs = Date.now();
    const server = http.createServer((request, response) => {
      if (gameProxy?.(request, response)) return;
      const url = new URL(request.url || "/", "http://127.0.0.1");
      if (url.pathname === "/v1/status" || url.pathname === "/client-status") {
        const body = JSON.stringify({
          ok: true,
          service: "zubspace-client",
          uptime_seconds: Math.floor((Date.now() - startedAtMs) / 1000),
          started_at_ms: startedAtMs,
          root: roots.app,
          fixtureRoot: roots.fixture,
        });
        response.writeHead(200, {
          "content-type": "application/json; charset=utf-8",
          "content-length": Buffer.byteLength(body),
          "cache-control": "no-store",
          "access-control-allow-origin": "*",
        });
        response.end(request.method === "HEAD" ? undefined : body);
        return;
      }
      const relative = decodeURIComponent(url.pathname === "/" ? "/index.html" : url.pathname);
      const normalized = path.normalize(relative).replace(/^(\.\.[/\\])+/, "");
      // `normalized` still carries its leading separator, so classify on the stripped form.
      const fixtureRelative = normalized.replace(/^[/\\]+/, "");
      const isHarnessFixture = fixtureRelative === "tests" || fixtureRelative.startsWith("tests" + path.sep);
      const root = isHarnessFixture ? roots.fixture : roots.app;
      const filename = path.join(root, normalized);
      if (!filename.startsWith(root + path.sep) || !fs.existsSync(filename) || !fs.statSync(filename).isFile()) {
        response.writeHead(404);
        response.end("not found");
        return;
      }
      response.writeHead(200, {
        "content-type": mimeType(filename),
        "cache-control": "no-store",
      });
      if (root === repoRoot && filename === path.join(repoRoot, "index.html")) {
        response.end(stampClientBuildInfo(fs.readFileSync(filename, "utf8"), clientBuildInfo(repoRoot)));
      } else {
        fs.createReadStream(filename).pipe(response);
      }
    });
    server.once("error", reject);
    server.listen(0, host, () => {
      resolve({ server, host, port: server.address().port, appRoot: roots.app, fixtureRoot: roots.fixture });
    });
  });
}

function startRealServer(
  port,
  {
    zone = null,
    zoneRoot = null,
    botCount = smokeBotCount,
    observerNetworkProfile = null,
    profile = "debug",
    cpuSet = null,
    enableObjectives = false,
    env = {},
  } = {},
) {
  const serverBin = serverBinaryForProfile(profile);
  const binary = path.join(repoRoot, serverBin);
  if (!fs.existsSync(binary)) {
    throw new Error(`missing ${serverBin}; run cargo build --bin zubspace_server first`);
  }
  const logs = [];
  const args = [`127.0.0.1:${port}`, "--tick-rate", smokeTickRate, "--log-level", smokeLogLevel];
  if (zone) {
    args.push("--zone", zone);
  }
  if (zoneRoot) {
    args.push("--zone-root", zoneRoot);
  }
  if (botCount) {
    args.push("--bot-count", String(botCount));
  }
  if (observerNetworkProfile) {
    args.push("--observer-network-profile", observerNetworkProfile);
  }
  if (enableObjectives) {
    // Flags and powerballs are deferred; only the QA fixtures that exercise the
    // retained objective machinery opt in.
    args.push("--enable-objectives");
  }
  const launch = commandWithCpuSet(binary, args, cpuSet);
  const child = spawn(launch.command, launch.args, {
    cwd: repoRoot,
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, ...env },
  });
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => logs.push(chunk));
  child.stderr.on("data", (chunk) => logs.push(chunk));
  registerGateProcess(child);
  return { child, logs,
    zoneRoot: zoneRoot ?? env.ZUBSPACE_SERVER_ZONE_ROOT ?? process.env.ZUBSPACE_SERVER_ZONE_ROOT ?? null };
}

async function waitForServerListening(port, child, logs) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(`zubspace_server exited before listening:\n${logs.join("")}`);
    }
    if (logs.join("").includes(`rtc=http://127.0.0.1:${port}/v1/rtc/offer`) ||
        logs.join("").split("\n").slice(0, -1).some(line => serverLogField(line, "rtc") === `http://127.0.0.1:${port}/v1/rtc/offer`)) {
      return;
    }
    await sleep(100);
  }
  throw new Error(`timed out waiting for zubspace_server on 127.0.0.1:${port}`);
}

function startGeckodriver(port, label) {
  const logs = [];
  // Detached so the lane's cleanup can kill geckodriver *and* the Firefox it
  // spawned inside the same process group.
  // The default BiDi listener is 9222 even when the WebDriver HTTP ports
  // differ. Let the OS bind a distinct port atomically for each owned browser.
  const child = spawn(geckodriverBin, ["--host", "127.0.0.1", "--port", String(port), "--websocket-port", "0"], {
    cwd: repoRoot,
    // Firefox inherits geckodriver's environment, so the short browser temp root has
    // to be set here for the browser that geckodriver later spawns.
    env: browserLaunchEnvironment(process.env, `${label} geckodriver/Firefox`),
    stdio: ["ignore", "pipe", "pipe"],
    detached: true,
  });
  registerGateProcess(child);
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => logs.push(chunk));
  child.stderr.on("data", (chunk) => logs.push(chunk));
  return { child, logs, label, port };
}

async function webdriver(
  driver,
  method,
  pathname,
  body = undefined,
  { timeoutMs = webdriverTimeoutMs } = {},
) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(`http://127.0.0.1:${driver.port}${pathname}`, {
      method,
      headers: body === undefined ? undefined : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: controller.signal,
    });
    const text = await response.text();
    const parsed = text ? JSON.parse(text) : {};
    if (!response.ok) {
      throw new Error(`${driver.label} WebDriver ${method} ${pathname} failed ${response.status}: ${text}`);
    }
    return parsed.value;
  } catch (error) {
    if (controller.signal.aborted) {
      throw new Error(
        `${driver.label} WebDriver ${method} ${pathname} timed out after ${timeoutMs}ms`,
        { cause: error },
      );
    }
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

async function waitForGeckodriver(driver) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (driver.child.exitCode !== null || driver.child.signalCode !== null) {
      throw new Error(`${driver.label} geckodriver exited before ready:\n${driver.logs.join("")}`);
    }
    try {
      await webdriver(driver, "GET", "/status", undefined, {
        timeoutMs: Math.min(1_000, webdriverTimeoutMs),
      });
      return;
    } catch {
      await sleep(100);
    }
  }
  throw new Error(`${driver.label} timed out waiting for geckodriver:\n${driver.logs.join("")}`);
}

async function createSession(driver) {
  const value = await webdriver(driver, "POST", "/session", {
    capabilities: {
      alwaysMatch: {
        browserName: "firefox",
        webSocketUrl: true,
        acceptInsecureCerts: true,
        "moz:firefoxOptions": {
          binary: firefoxBin,
          ...(firefoxBrowserEnv() ? { env: firefoxBrowserEnv() } : {}),
          // Headed windows carry the shared test window class (see
          // e2eWindowClass); Firefox applies --class only under X11/XWayland.
          args: firefoxHeadless ? ["-headless"] : ["--class", e2eWindowClass],
          prefs: {
            // Firefox's Talos ASAP mode, matching Chrome's uncapped scheduler.
            // A requested setting is not proof: the gate measures actual draws.
            ...(process.env.ZUBSPACE_E2E_HIGH_REFRESH === "1"
              ? {
                "layout.frame_rate": 0,
                // The uncapped draw-order proof requires distinct timestamps.
                // Firefox's rounded web clock can report equal readings for
                // adjacent draws. Request precision in this disposable profile;
                // ordinary Firefox gameplay lanes retain browser defaults.
                "privacy.reduceTimerPrecision": false,
              } : {}),
            // Most gameplay suites are not audio-policy tests and retain the
            // historical deterministic autoplay allowance. The audiovisual
            // gate opts into Firefox's ordinary audible-media block so the
            // same trusted keyboard gesture a player uses must unlock the
            // game's AudioContexts.
            "media.autoplay.default":
              process.env.ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE === "1" ? 1 : 0,
            "media.peerconnection.enabled": true,
            "dom.webaudio.enabled": true,
          },
        },
      },
    },
  });
  driver.sessionId = value.sessionId;
  driver.browserConsole = await openFirefoxConsole(value.capabilities?.webSocketUrl);
}

async function deleteSession(driver) {
  driver.browserConsole?.close();
  if (!driver.sessionId) return;
  try {
    await webdriver(driver, "DELETE", `/session/${driver.sessionId}`);
  } catch {
    // Firefox may already be gone during cleanup.
  }
  driver.sessionId = null;
}

async function cycleFirefoxWindowFocus(driver, holdMs = 200) {
  const originalHandle = await webdriver(
    driver,
    "GET",
    `/session/${driver.sessionId}/window`,
  );
  let awayHandle = null;
  try {
    const created = await webdriver(
      driver,
      "POST",
      `/session/${driver.sessionId}/window/new`,
      { type: "tab" },
    );
    if (typeof created?.handle !== "string" || created.handle.length === 0) {
      throw new Error("Firefox new-window response omitted its handle");
    }
    awayHandle = created.handle;
    await webdriver(driver, "POST", `/session/${driver.sessionId}/window`, {
      handle: awayHandle,
    });
    await sleep(holdMs);
  } finally {
    if (awayHandle != null) {
      try {
        await webdriver(driver, "POST", `/session/${driver.sessionId}/window`, {
          handle: awayHandle,
        });
        await webdriver(driver, "DELETE", `/session/${driver.sessionId}/window`);
      } catch {
        // The temporary tab may already have closed during failure cleanup.
      }
    }
    await webdriver(driver, "POST", `/session/${driver.sessionId}/window`, {
      handle: originalHandle,
    });
    await execute(driver, "document.querySelector('#glcanvas')?.focus(); return true;");
  }
  return { originalHandle, awayHandle, holdMs };
}

async function execute(driver, script, args = []) {
  return webdriver(driver, "POST", `/session/${driver.sessionId}/execute/sync`, {
    script,
    args,
  });
}

async function executeAsync(driver, script, args = []) {
  return webdriver(driver, "POST", `/session/${driver.sessionId}/execute/async`, {
    script,
    args,
  });
}

// Artifact-time only. Await the real marker through WebDriver's async-script
// callback; do not add freezing or historical serialization to hot metrics.
async function freezeQaTrace(driver, options = {}) {
  const result = await executeAsync(driver, `
    const done = arguments[arguments.length - 1];
    Promise.resolve().then(() => {
      if (window.__zubspaceQaTraceStateForTest?.().enabled === false) return null;
      if (typeof window.__zubspaceMarkQaIssue !== 'function') {
        throw new Error('QA trace marker is unavailable');
      }
      return window.__zubspaceMarkQaIssue(arguments[0]);
    }).then(
      value => done({ ok: true, value }),
      error => done({ ok: false, error: String(error?.message || error) })
    );
  `, [{ postTailMs: 0, download: false, upload: false, ...options }]);
  if (result?.ok !== true) {
    throw new Error(result?.error || "Firefox QA trace capture returned no result");
  }
  return result.value;
}

async function screenshot(driver) {
  return webdriver(driver, "GET", `/session/${driver.sessionId}/screenshot`);
}

async function navigate(driver, url) {
  await webdriver(driver, "POST", `/session/${driver.sessionId}/url`, { url });
}

async function findElement(driver, selector) {
  return webdriver(driver, "POST", `/session/${driver.sessionId}/element`, {
    using: "css selector",
    value: selector,
  });
}

function elementId(element) {
  return element["element-6066-11e4-a52e-4f735466cecf"];
}

async function clickElement(driver, element) {
  await webdriver(driver, "POST", `/session/${driver.sessionId}/element/${elementId(element)}/click`, {});
}

async function performKeyActions(driver, actions, release = true) {
  await webdriver(driver, "POST", `/session/${driver.sessionId}/actions`, {
    actions: [
      {
        type: "key",
        id: "keyboard",
        actions,
      },
    ],
  });
  if (release) {
    await webdriver(driver, "DELETE", `/session/${driver.sessionId}/actions`);
  }
}

async function firefoxKeyDown(driver, key) {
  const value = WEBDRIVER_KEY[key] || key;
  await execute(driver, "document.querySelector('#glcanvas')?.focus(); return true;");
  await performKeyActions(driver, [{ type: "keyDown", value }], false);
}

async function firefoxKeyUp(driver, key) {
  const value = WEBDRIVER_KEY[key] || key;
  await performKeyActions(driver, [{ type: "keyUp", value }], false);
}

async function firefoxKeyBatch(driver, changes) {
  await execute(driver, "document.querySelector('#glcanvas')?.focus(); return true;");
  // One WebDriver tick, with a stable source per physical key. Releases must
  // use this same API/source; DELETE /actions remains emergency cleanup.
  await webdriver(driver, "POST", `/session/${driver.sessionId}/actions`, {
    actions: changes.map(({ type, key }) => ({
      type: "key", id: `batch-${key}`,
      actions: [{ type, value: WEBDRIVER_KEY[key] || key }],
    })),
  });
}

async function releaseFirefoxKeys(driver) {
  await webdriver(driver, "DELETE", `/session/${driver.sessionId}/actions`);
}

async function holdFirefoxKeys(driver, keys, durationMs) {
  const values = keys.map((key) => WEBDRIVER_KEY[key] || key);
  await execute(driver, "document.querySelector('#glcanvas')?.focus(); return true;");
  await performKeyActions(driver, [
    ...values.map((value) => ({ type: "keyDown", value })),
    { type: "pause", duration: Math.max(0, Math.round(durationMs)) },
    ...values.toReversed().map((value) => ({ type: "keyUp", value })),
  ]);
}

async function holdThenChordFirefoxKeys(
  driver,
  firstKey,
  secondKey,
  firstOnlyMs = 180,
  chordMs = 500,
  releaseOrder = [secondKey, firstKey],
) {
  const first = WEBDRIVER_KEY[firstKey] || firstKey;
  const second = WEBDRIVER_KEY[secondKey] || secondKey;
  const releases = releaseOrder.map((key) => WEBDRIVER_KEY[key] || key);
  await execute(driver, "document.querySelector('#glcanvas')?.focus(); return true;");
  await performKeyActions(driver, [
    { type: "keyDown", value: first },
    { type: "pause", duration: Math.max(0, Math.round(firstOnlyMs)) },
    { type: "keyDown", value: second },
    { type: "pause", duration: Math.max(0, Math.round(chordMs)) },
    ...releases.map((value) => ({ type: "keyUp", value })),
  // An empty release order intentionally leaves the keys held for a later
  // genuine keyUp. DELETE /actions would release them before that observation.
  ], releases.length > 0);
}

async function waitUntil(description, predicate, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${description}`);
}

async function openClient(driver, url) {
  await navigate(driver, url);
  await waitUntil(`${driver.label} canvas`, async () => {
    return execute(
      driver,
      `return !!document.querySelector('#glcanvas') &&
        performance.getEntriesByType('resource').some((entry) => /zubspace\\.wasm/.test(entry.name));`,
      );
  }, 60_000);
  await execute(driver, "document.querySelector('#glcanvas')?.focus(); return true;");
  await waitUntil(`${driver.label} RTC channels`, async () => {
    const state = await execute(driver, `return ${smokeLiveStateExpression};`);
    const messages = state.networkLog.map((entry) => entry.message);
    const labels = state.networkLog
      .filter((entry) => entry.message === "rtc data channel open")
      .map((entry) => entry.detail?.label);
    return (
      messages.includes("rtc answer accepted") &&
      labels.includes("zubspace-game") &&
      labels.includes("zubspace-events") &&
      labels.includes("zubspace-reliable")
    );
  }, 60_000);
  await waitUntil(`${driver.label} loading overlay`, async () => {
    return execute(
      driver,
      `const overlay = document.querySelector('#loading-overlay');
       return !overlay || overlay.classList.contains('hidden');`,
    );
  }, 60_000);
  const canvas = await findElement(driver, "#glcanvas");
  await clickElement(driver, canvas);
  await execute(driver, "document.querySelector('#glcanvas')?.focus(); return true;");
  await installFirefoxAudioProbe(driver);
}

export function firefoxArtifactClient(driver, {
  executeScript = execute, readPageState = pageState, freezeTrace = freezeQaTrace,
} = {}) {
  const browserConsole = driver.browserConsole;
  return {
    events: browserConsole?.events,
    consoleEvidence: () => browserConsole?.snapshot() || browserConsoleEvidence(null),
    evaluate: (expression) => executeScript(driver, `return (${expression});`),
    pageState: () => readPageState(driver),
    freezeQaTrace: (options) => freezeTrace(driver, options),
  };
}

async function pageState(driver) {
  return execute(
    driver,
    `return {
      networkLog: window.__zubspaceNetworkLog || [],
      packetLogFirst: window.__zubspacePacketLogFirst || [],
      packetLog: window.__zubspacePacketLog || [],
      uiMetrics: window.__zubspaceUiMetrics || null,
      metricsTrace: window.__zubspaceMetricsTrace || [],
      remoteSpriteTrace: window.__zubspaceRemoteSpriteTrace || [],
      remoteLifeTrace: window.__zubspaceRemoteLifeTrace || [],
      prizeStateTrace: window.__zubspacePrizeStateTrace || [],
      damageTrace: window.__zubspaceDamageTrace || [],
      damagePresentationTrace: window.__zubspaceDamagePresentationTrace || [],
      inputTrace: window.__zubspaceInputTrace || [],
      weaponAdmissionTrace: window.__zubspaceWeaponAdmissionTrace || [],
      weaponAdmissionTraceTotal: window.__zubspaceWeaponAdmissionTraceTotal || 0,
      weaponAdmissionTraceDropped: window.__zubspaceWeaponAdmissionTraceDropped || 0,
      audioUnlock: window.__zubspaceAudioUnlockState || null,
      gameKeyBridge: window.__zubspaceGameKeyStateForTest?.() || null,
      qaTraceState: window.__zubspaceQaTraceStateForTest?.() || null,
      qaTraceBundle: window.__zubspaceLastQaTraceBundle || null,
      firefoxAudioProbe: window.__zubFirefoxAudioProbe || null,
      networkStats: window.__zubspaceNetworkStats || {},
      currentUrl: window.location.href,
    };`,
  );
}

async function metricsState(driver) {
  // Firefox WebDriver serializes the result of every script command. Combat
  // feedback loops must not repeatedly copy the bounded packet, sprite, and
  // damage histories merely to read current steering/lifecycle state.
  return execute(
    driver,
    `const rawUiMetrics = window.__zubspaceUiMetrics || null;
     const actorTrace = Array.isArray(rawUiMetrics?.authoritativeSnapshotActorTrace)
       ? rawUiMetrics.authoritativeSnapshotActorTrace.slice(-1)
       : [];
     const rawActorStream = rawUiMetrics?.authoritativeSnapshotActorStream;
     return {
       uiMetrics: rawUiMetrics ? {
         ...rawUiMetrics,
         authoritativeSnapshotActorTrace: actorTrace,
         authoritativeSnapshotActorStream: rawActorStream && typeof rawActorStream === 'object'
           ? { ...rawActorStream, frames: actorTrace }
           : rawActorStream,
       } : null,
       networkStats: window.__zubspaceNetworkStats || {},
       damageTrace: (window.__zubspaceDamageTrace || []).slice(-256),
       damagePresentationTrace:
         (window.__zubspaceDamagePresentationTrace || []).slice(-64),
       deathTrace: (window.__zubspaceDeathTrace || []).slice(-64),
     };`,
  );
}

async function installFirefoxAudioProbe(driver) {
  return execute(
    driver,
    `if (!window.__zubFirefoxAudioProbeInstalled) {
      window.__zubFirefoxAudioProbeInstalled = true;
      window.__zubFirefoxAudioProbe = {
        available: !!(window.AudioContext || window.webkitAudioContext),
        starts: 0,
        runningStarts: 0,
        sourceConnections: 0,
        durations: [],
        explicitDecoded: 0,
        explicitStarted: 0,
        errors: [],
      };
      const proto = window.AudioBufferSourceNode && window.AudioBufferSourceNode.prototype;
      if (proto && !proto.__zubFirefoxStartPatched) {
        const realStart = proto.start;
        const realConnect = proto.connect;
        proto.start = function(...args) {
          const probe = window.__zubFirefoxAudioProbe;
          probe.starts += 1;
          if (this.context && this.context.state === "running") probe.runningStarts += 1;
          probe.durations.push(this.buffer ? this.buffer.duration : null);
          try {
            return realStart.apply(this, args);
          } catch (error) {
            probe.errors.push(String(error));
            throw error;
          }
        };
        proto.connect = function(...args) {
          window.__zubFirefoxAudioProbe.sourceConnections += 1;
          return realConnect.apply(this, args);
        };
        proto.__zubFirefoxStartPatched = true;
      }
    }
    return window.__zubFirefoxAudioProbe;`,
  );
}

async function probeFirefoxAudioPlayback(driver) {
  return executeAsync(
    driver,
    `const done = arguments[arguments.length - 1];
     (async () => {
       const probe = window.__zubFirefoxAudioProbe || {
         available: !!(window.AudioContext || window.webkitAudioContext),
         starts: 0,
         runningStarts: 0,
         sourceConnections: 0,
         durations: [],
         explicitDecoded: 0,
         explicitStarted: 0,
         errors: [],
       };
       window.__zubFirefoxAudioProbe = probe;
       try {
         const AudioContextCtor = window.AudioContext || window.webkitAudioContext;
         if (!AudioContextCtor) throw new Error("AudioContext unavailable");
         const context = new AudioContextCtor();
         if (context.state === "suspended" && context.resume) {
           await context.resume();
         }
         const response = await fetch("/assets/sound/bomb1.wa2", { cache: "no-store" });
         if (!response.ok) throw new Error("sound fetch failed: " + response.status);
         const bytes = await response.arrayBuffer();
         const decoded = await context.decodeAudioData(bytes.slice(0));
         probe.explicitDecoded += 1;
         const source = context.createBufferSource();
         source.buffer = decoded;
         source.connect(context.destination);
         source.start(0);
         probe.explicitStarted += 1;
         done(probe);
       } catch (error) {
         probe.errors.push(String(error && error.message ? error.message : error));
         done(probe);
       }
     })();`,
  );
}

function assertFirefoxAudio(name, state) {
  const unlock = state.audioUnlock;
  const probe = state.firefoxAudioProbe;
  if (
    !unlock?.installed ||
    !unlock.gestureSeen ||
    unlock.trackedContexts < 1 ||
    unlock.resumeFailures > 0
  ) {
    throw new Error(`${name} audio unlock bridge did not track a gesture and game audio context: ${JSON.stringify(unlock)}`);
  }
  if (
    !probe?.available ||
    probe.explicitDecoded < 1 ||
    probe.explicitStarted < 1 ||
    probe.starts < 1 ||
    (probe.errors || []).length > 0
  ) {
    throw new Error(`${name} did not decode/start original Firefox audio: ${JSON.stringify(probe)}`);
  }
}

function assertFirefoxUiMetrics(name, metrics) {
  if (
    !metrics ||
    !metrics.energyInside ||
    metrics.energyCenterDelta > 2 ||
    !metrics.energyDigitsInside ||
    !metrics.energyDigitsRightAligned ||
    !metrics.hudTextInside ||
    metrics.hudRadarOverlap ||
    !metrics.radarBottomRight ||
    !metrics.radarLabelInside ||
    !metrics.rightStatusPanelInside ||
    !metrics.rightStatusPanelClearOfRadar ||
    !metrics.leftItemPanelInside ||
    !metrics.shipLabelInside ||
    !metrics.notificationInside ||
    !metrics.textPositiveScale ||
    metrics.hudTextScale > 1 ||
    metrics.shipLabelScale > 1 ||
    metrics.notificationScale > 1 ||
    !hudLineCountMatchesMode(metrics)
  ) {
    throw new Error(`${name} Firefox UI metrics are not fitted: ${JSON.stringify(metrics)}`);
  }
}

async function enterShip(driver) {
  await execute(driver, "document.querySelector('#glcanvas')?.focus(); return true;");
  // The ordinary join already selects a playable ship. Repeated Esc/2 during
  // asynchronous admission can leak the final 2 into gameplay as Rocket.
  await waitUntil("Firefox ordinary join playable", async () => {
    return execute(driver, "return window.__zubspaceLifecycle?.inGameplay === true;");
  }, 60_000);
}

async function driveAndRelease(driver) {
  await execute(driver, "document.querySelector('#glcanvas')?.focus(); return true;");
  await performKeyActions(driver, [
    { type: "keyDown", value: KEY_ARROW_UP },
    { type: "keyDown", value: KEY_ARROW_RIGHT },
    { type: "pause", duration: 700 },
    { type: "keyUp", value: KEY_ARROW_RIGHT },
    { type: "keyUp", value: KEY_ARROW_UP },
  ]);
  return { releasedAt: await execute(driver, "return Date.now();") };
}

function packetsFromLog(log, direction) {
  return log
    .filter((entry) => entry.direction === direction)
    .map((entry) => {
      const frame = Buffer.from(entry.bytes || []);
      frame.zubspaceDetail = {
        ...(entry.detail && typeof entry.detail === "object" ? entry.detail : {}),
        at: entry.at || 0,
      };
      return frame;
    });
}

function parseReceivedPositionUpdates(packet) {
  const at = packet.zubspaceDetail?.at ?? null;
  const realtimeWeapon = parseRealtimeWeaponEvent(packet);
  if (realtimeWeapon) {
    return [{
      ...realtimeWeapon,
      timestamp: realtimeWeapon.serverTick & 0xffff,
      timestampMask: 0xffff,
      at,
    }];
  }
  if (packet[0] === 0x05 && packet.length >= 23) {
    return [
      {
        source: packet.readUInt16LE(8),
        timestamp: packet.readUInt16LE(2),
        timestampMask: 0xffff,
        x: packet.readUInt16LE(4),
        y: packet.readUInt16LE(15),
        velocityX: packet.readInt16LE(10),
        velocityY: packet.readInt16LE(6),
        weapon: packet.readUInt16LE(19),
        at,
      },
    ];
  }
  if (packet[0] === 0x39 && packet.length >= 11 && (packet.length - 1) % 10 === 0) {
    return parseBatchedPositionUpdates(packet, 10, false, at);
  }
  if (packet[0] === 0x3a && packet.length >= 12 && (packet.length - 1) % 11 === 0) {
    return parseBatchedPositionUpdates(packet, 11, true, at);
  }
  return [];
}

function parseBatchedPositionUpdates(packet, entryLen, includesTogglables, at) {
  const updates = [];
  for (let offset = 1; offset < packet.length; offset += entryLen) {
    let source;
    let cursor;
    if (includesTogglables) {
      source = packet.readUInt16LE(offset) & 0x03ff;
      cursor = offset + 2;
    } else {
      source = packet[offset];
      cursor = offset + 1;
    }
    const packedHeading = packet.readUInt16LE(cursor);
    const packedPos = packet.readUInt32LE(cursor + 2);
    const packedVelocity = packet.readUInt16LE(cursor + 6);
    const velocityY = signExtend(packedVelocity & 0x3fff, 14);
    const multiplier = packet.readInt8(cursor + 8);
    const velocityX = (((packedVelocity >> 14) + multiplier * 4) * 16) + (packedPos >>> 28);
    updates.push({
      source,
      timestamp: packedHeading & 0x03ff,
      timestampMask: 0x03ff,
      x: packedPos & 0x3fff,
      y: (packedPos >>> 14) & 0x3fff,
      velocityX,
      velocityY,
      weapon: 0,
      at,
    });
  }
  return updates;
}

function signExtend(value, bits) {
  const sign = 1 << (bits - 1);
  return (value & sign) ? value - (1 << bits) : value;
}

function fixedCString(buffer) {
  const zero = buffer.indexOf(0);
  return buffer.subarray(0, zero < 0 ? buffer.length : zero).toString("utf8");
}

function isReliable(packet) {
  return packet.length >= 6 && packet[0] === 0x00 && packet[1] === 0x03;
}

function reliablePayload(packet) {
  return isReliable(packet) ? packet.subarray(6) : null;
}

function reliableMessageId(packet) {
  return isReliable(packet) ? packet.readUInt32LE(2) : null;
}

function logicalReliablePayloads(packets) {
  const payloads = [];
  const smallChunks = [];
  const seenReliableIds = new Set();
  for (const packet of packets) {
    const payload = reliablePayload(packet);
    if (!payload) continue;
    const reliableId = reliableMessageId(packet);
    if (seenReliableIds.has(reliableId)) continue;
    seenReliableIds.add(reliableId);
    if (payload.length >= 2 && payload[0] === 0x00 && payload[1] === 0x08) {
      smallChunks.push(payload.subarray(2));
      continue;
    }
    if (payload.length >= 2 && payload[0] === 0x00 && payload[1] === 0x09) {
      smallChunks.push(payload.subarray(2));
      payloads.push(Buffer.concat(smallChunks));
      smallChunks.length = 0;
      continue;
    }
    payloads.push(payload);
  }
  return payloads;
}

function parseRoster(payload) {
  if (!payload || payload[0] !== 0x03 || payload.length < 64) return null;
  return {
    id: payload.readUInt16LE(51),
    name: fixedCString(payload.subarray(3, 23)),
    ship: payload[1],
    frequency: payload.readUInt16LE(53),
    wins: payload.readUInt16LE(55),
    losses: payload.readUInt16LE(57),
  };
}

function parseWatchDamage(payload) {
  if (!payload || payload[0] !== 0x38 || payload.length < 7 || (payload.length - 7) % 9 !== 0) {
    return null;
  }
  const entries = [];
  for (let offset = 7; offset < payload.length; offset += 9) {
    entries.push({
      attackerId: payload.readUInt16LE(offset),
      weapon: payload.readUInt16LE(offset + 2),
      energy: payload.readUInt16LE(offset + 4),
      damage: payload.readUInt16LE(offset + 6),
      timestampOffset: payload[offset + 8],
    });
  }
  return {
    playerId: payload.readUInt16LE(1),
    timestamp: payload.readUInt32LE(3),
    entries,
  };
}

function parseClientDamageReport(payload) {
  if (!payload || payload[0] !== 0x32 || payload.length < 14 || (payload.length - 5) % 9 !== 0) {
    return null;
  }
  const entries = [];
  for (let offset = 5; offset < payload.length; offset += 9) {
    entries.push({
      shooterId: payload.readUInt16LE(offset),
      weapon: payload.readUInt16LE(offset + 2),
      energy: payload.readUInt16LE(offset + 4),
      damage: payload.readUInt16LE(offset + 6),
      timestampOffset: payload[offset + 8],
    });
  }
  return {
    timestamp: payload.readUInt32LE(1),
    entries,
  };
}

function skipSnapshotEntity(packet, offset) {
  const parsed = readSnapshotEntity(packet, offset);
  return parsed?.next ?? null;
}

function readSnapshotEntity(packet, offset) {
  if (!packet || offset + 4 > packet.length) return null;
  const entity = {
    source: packet.readUInt16LE(offset),
    fieldMask: packet.readUInt16LE(offset + 2),
  };
  const fieldMask = entity.fieldMask;
  if ((fieldMask & ~0x7fff) !== 0) return null;
  let pos = offset + 4;
  const readField = (bit, size, name, reader) => {
    if ((fieldMask & bit) === 0) return true;
    if (pos + size > packet.length) return false;
    entity[name] = reader(pos);
    pos += size;
    return true;
  };
  // Bit 14: x and y are one-byte offsets from the acknowledged base.
  if ((fieldMask & (1 << 14)) !== 0) {
    if (!readField(1 << 0, 1, "xOffset", (at) => packet.readInt8(at))) return null;
    if (!readField(1 << 1, 1, "yOffset", (at) => packet.readInt8(at))) return null;
  } else {
    if (!readField(1 << 0, 2, "x", (at) => packet.readUInt16LE(at))) return null;
    if (!readField(1 << 1, 2, "y", (at) => packet.readUInt16LE(at))) return null;
  }
  if (!readField(1 << 2, 2, "velocityX", (at) => packet.readInt16LE(at))) return null;
  if (!readField(1 << 3, 2, "velocityY", (at) => packet.readInt16LE(at))) return null;
  if (!readField(1 << 4, 1, "direction", (at) => packet[at])) return null;
  if (!readField(1 << 5, 2, "energy", (at) => packet.readUInt16LE(at))) return null;
  if (!readField(1 << 6, 2, "weapon", (at) => packet.readUInt16LE(at))) return null;
  if (!readField(1 << 7, 1, "status", (at) => packet[at])) return null;
  if (!readField(1 << 8, 2, "bounty", (at) => packet.readUInt16LE(at))) return null;
  if (!readField(1 << 9, 1, "ping", (at) => packet[at])) return null;
  if (!readField(1 << 10, 2, "flagTimer", (at) => packet.readUInt16LE(at))) return null;
  if (!readField(1 << 11, 4, "lifeSeq", (at) => packet.readUInt32LE(at))) return null;
  if ((packet[1] & 0x10) !== 0 && (fieldMask & (1 << 12)) !== 0) {
    // Compact frames: ticks behind the frame tick * 3 + third, or 0xff and
    // the five-byte form (src/protocol/snapshot.rs).
    if (pos + 1 > packet.length) return null;
    const byte = packet[pos];
    pos += 1;
    if (byte !== 0xff) {
      const frameTick = packet.readUInt32LE(6) & 0x7fffffff;
      entity.sampleTime = { tick: (frameTick - Math.floor(byte / 3)) & 0x7fffffff, third: byte % 3 };
    } else if (!readField(1 << 12, 5, "sampleTime", (at) => ({ tick: packet.readUInt32LE(at), third: packet[at + 4] }))) return null;
  } else if (!readField(1 << 12, 5, "sampleTime", (at) => ({ tick: packet.readUInt32LE(at), third: packet[at + 4] }))) return null;
  if (!readField(1 << 13, 4, "motionSegment", (at) => packet.readUInt32LE(at))) return null;
  if ((packet[1] & 0x08) !== 0 &&
      !readField(1 << 13, 4, "relocationGeneration", (at) => packet.readUInt32LE(at))) return null;
  if (entity.sampleTime && (entity.sampleTime.third > 2 || entity.sampleTime.tick > 0x7fffffff)) return null;
  return { entity, next: pos };
}

// Merge an entry over its base, turning one-byte position offsets (mask bit
// 14) back into absolute positions as the Rust decoder does.
function withResolvedOffsets(base, entity) {
  const { xOffset, yOffset, ...fields } = entity;
  const state = { ...base, ...fields };
  if (xOffset !== undefined) state.x = (base.x ?? 0) + xOffset;
  if (yOffset !== undefined) state.y = (base.y ?? 0) + yOffset;
  return state;
}

function parseSnapshotPositionUpdates(packet, historyBySnapshot) {
  if (!packet || packet[0] !== 0x70 || packet.length < 21) return [];
  const flags = packet[1];
  // Diagnostic readers retain support for historical pre-source-time bundles;
  // the live Rust codec requires source-time, motion and relocation revisions.
  if ((flags & ~0x3f) !== 0) return [];
  const snapshotId = packet.readUInt16LE(2);
  const baseId = packet.readUInt16LE(4);
  const timestamp = packet.readUInt32LE(6) & 0xffff;
  const nearCount = packet[18];
  const midCount = packet[19];
  const isFull = (flags & 0x01) !== 0;
  const baseEntities = isFull ? new Map() : historyBySnapshot.get(baseId);
  if (!baseEntities) return [];
  const frameEntities = new Map();
  let pos = 21;
  const updates = [];
  for (let index = 0; index < nearCount; index += 1) {
    const parsed = readSnapshotEntity(packet, pos);
    if (!parsed) return updates;
    const entity = parsed.entity;
    pos = parsed.next;
    const state = withResolvedOffsets(baseEntities.get(entity.source) || {}, entity);
    if ((flags & 0x02) !== 0 && !state.sampleTime) return [];
    if ((flags & 0x04) !== 0 && !Number.isInteger(state.motionSegment)) return [];
    if ((flags & 0x08) !== 0 && !Number.isInteger(state.relocationGeneration)) return [];
    frameEntities.set(entity.source, state);
    if (Number.isFinite(state.x) && Number.isFinite(state.y)) {
      updates.push({
        source: entity.source,
        timestamp,
        timestampMask: 0xffff,
        x: state.x,
        y: state.y,
        velocityX: state.velocityX ?? 0,
        velocityY: state.velocityY ?? 0,
        weapon: entity.weapon ?? 0,
        at: packet.zubspaceDetail?.at ?? null,
      });
    }
  }
  for (let index = 0; index < midCount; index += 1) {
    const parsed = readSnapshotEntity(packet, pos);
    if (!parsed) return updates;
    const entity = parsed.entity;
    pos = parsed.next;
    const baseState = baseEntities.get(entity.source) || {};
    const position = withResolvedOffsets(baseState, entity);
    // Match the shared Rust codec and server history: mid-tier bases retain
    // radar position only. Near-only fields must return to defaults if this
    // mid snapshot is ACKed and the entity later returns near.
    const state = {
      source: entity.source,
      fieldMask: entity.fieldMask,
      x: position.x,
      y: position.y,
      velocityX: 0,
      velocityY: 0,
      direction: 0,
      energy: 0,
      weapon: 0,
      status: 0,
      bounty: 0,
      ping: 0,
      flagTimer: 0,
    };
    frameEntities.set(entity.source, state);
  }
  historyBySnapshot.set(snapshotId, frameEntities);
  while (historyBySnapshot.size > 32) {
    const removableId = [...historyBySnapshot.keys()].find(
      (id) => isFull || id !== baseId,
    );
    if (removableId == null) break;
    historyBySnapshot.delete(removableId);
  }
  return updates;
}

function parseSnapshotWatchDamageReports(packet) {
  if (packet?.[0] === 0x7d && packet.length === 72 && packet[2] === 2) {
    return [{
      playerId: packet.readUInt16LE(27),
      timestamp: packet.readUInt32LE(23),
      entries: [{
        attackerId: packet.readUInt16LE(41),
        weapon: packet.readUInt16LE(55),
        damage: packet.readUInt16LE(57),
        energy: packet.readUInt16LE(61),
        timestampOffset: 0,
        eventId: packet.readUInt32LE(19),
      }],
    }];
  }
  if (!packet || packet[0] !== 0x70 || packet.length < 21) return [];
  const timestamp = packet.readUInt32LE(6);
  const nearCount = packet[18];
  const midCount = packet[19];
  const eventCount = packet[20];
  let pos = 21;
  for (let index = 0; index < nearCount + midCount; index += 1) {
    pos = skipSnapshotEntity(packet, pos);
    if (pos === null) return [];
  }
  const reports = [];
  for (let index = 0; index < eventCount; index += 1) {
    if (pos + 2 > packet.length) return reports;
    const eventType = packet[pos];
    const bodyLen = packet[pos + 1];
    const bodyStart = pos + 2;
    const bodyEnd = bodyStart + bodyLen;
    if (bodyEnd > packet.length) return reports;
    if (eventType === 0x01 && bodyLen >= 11) {
      reports.push({
        playerId: packet.readUInt16LE(bodyStart),
        timestamp,
        entries: [
          {
            attackerId: packet.readUInt16LE(bodyStart + 2),
            weapon: packet.readUInt16LE(bodyStart + 4),
            damage: packet.readUInt16LE(bodyStart + 6),
            energy: packet.readUInt16LE(bodyStart + 8),
            timestampOffset: 0,
          },
        ],
      });
    }
    pos = bodyEnd;
  }
  return reports;
}

function parsePlayerDeath(payload) {
  if (!payload || payload[0] !== 0x06 || payload.length < 10) {
    return null;
  }
  return {
    killerId: payload.readUInt16LE(2),
    killedId: payload.readUInt16LE(4),
    bounty: payload.readUInt16LE(6),
    flagTransfer: payload.readUInt16LE(8),
  };
}

function parseScoreUpdate(payload) {
  if (!payload || payload[0] !== 0x09 ||
      !(payload.length === 15 || (payload.length === 20 && payload[15] === 1))) {
    return null;
  }
  return {
    assists: payload.length === 20 ? payload.readUInt32LE(16) : 0,
    playerId: payload.readUInt16LE(1),
    killPoints: payload.readUInt32LE(3),
    flagPoints: payload.readUInt32LE(7),
    wins: payload.readUInt16LE(11),
    losses: payload.readUInt16LE(13),
  };
}

function summarizePacketLog(packetLog) {
  const sentPackets = packetsFromLog(packetLog, "sent");
  const receivedPackets = packetsFromLog(packetLog, "received");
  const receivedPayloads = [
    ...logicalReliablePayloads(receivedPackets),
    ...receivedPackets.filter((packet) => packet.length > 0 && packet[0] !== 0x00 && packet[0] !== 0x70),
  ];
  const sentPayloads = [
    ...logicalReliablePayloads(sentPackets),
    ...sentPackets.filter((packet) => packet.length > 0 && packet[0] !== 0x00 && packet[0] !== 0x70),
  ];
  const receivedDamageReports = [
    ...receivedPayloads.map(parseWatchDamage).filter(Boolean),
    ...receivedPackets.flatMap(parseSnapshotWatchDamageReports),
  ];
  const snapshotCarry = new Map();
  const receivedSnapshotPositionUpdates = receivedPackets.flatMap((packet) =>
    parseSnapshotPositionUpdates(packet, snapshotCarry),
  );
  const rosters = receivedPayloads.map(parseRoster).filter(Boolean);
  const playerIds = receivedPayloads
    .filter((payload) => payload[0] === 0x01 && payload.length === 3)
    .map((payload) => payload.readUInt16LE(1));
  const sentPositions = sentPackets
    .filter((packet) => packet[0] === 0x03 && packet.length >= 22)
    .map((packet) => ({
      at: packet.zubspaceDetail?.at ?? null,
      timestamp: packet.readUInt32LE(2),
      x: packet.readUInt16LE(12),
      y: packet.readUInt16LE(8),
      velocityX: packet.readInt16LE(6),
      velocityY: packet.readInt16LE(14),
      weapon: packet.readUInt16LE(20),
      channel: packet.zubspaceDetail?.channel ?? null,
    }));
  const receivedPositionUpdates = [
    ...receivedPackets.flatMap(parseReceivedPositionUpdates),
    ...receivedSnapshotPositionUpdates,
  ];
  const sentPositionWeapons = sentPackets
    .filter((packet) => packet[0] === 0x03 && packet.length >= 22)
    .map((packet) => ({
      at: packet.zubspaceDetail?.at ?? null,
      timestamp: packet.readUInt32LE(2),
      weapon: packet.readUInt16LE(20),
      x: packet.readUInt16LE(12),
      y: packet.readUInt16LE(8),
      channel: packet.zubspaceDetail?.channel ?? null,
    }));
  const receivedPositionWeapons = receivedPackets
    .filter((packet) => packet[0] === 0x05 && packet.length >= 23)
    .map((packet) => ({
      at: packet.zubspaceDetail?.at ?? null,
      source: packet.readUInt16LE(8),
      timestamp: packet.readUInt16LE(2),
      weapon: packet.readUInt16LE(19),
      x: packet.readUInt16LE(4),
      y: packet.readUInt16LE(15),
      channel: packet.zubspaceDetail?.channel ?? null,
    }));
  return {
    selfId: playerIds.at(-1) ?? null,
    rosters,
    rosterNames: rosters.map((roster) => roster.name),
    receivedPacketHeads: receivedPackets.slice(0, 40).map((packet) => ({
      first: packet[0] ?? null,
      second: packet[1] ?? null,
      length: packet.length,
      channel: packet.zubspaceDetail?.channel ?? null,
    })),
    receivedReliablePayloadTypes: receivedPayloads.map((payload) => payload[0]),
    sentReliablePayloadTypes: sentPayloads.map((payload) => payload[0]),
    receivedDamageReports,
    sentDamageReports: sentPayloads.map(parseClientDamageReport).filter(Boolean),
    receivedDeaths: receivedPayloads.map(parsePlayerDeath).filter(Boolean),
    receivedScoreUpdates: receivedPayloads.map(parseScoreUpdate).filter(Boolean),
    sentPositions,
    receivedPositionUpdates,
    receivedPositionSources: receivedPositionUpdates.map((update) => update.source),
    sentPositionWeapons,
    receivedPositionWeapons,
    sentPositionCount: sentPositions.length,
    receivedPositionCount: receivedPositionUpdates.length,
    receivedTimestampGaps: positionTimestampGapSummary(receivedPositionUpdates),
  };
}

function timelineRate(items) {
  const timestamps = items
    .map((item) => item.at)
    .filter((at) => typeof at === "number" && at > 0)
    .sort((a, b) => a - b);
  if (timestamps.length < 2) {
    return 0;
  }
  const elapsedSeconds = Math.max(
    (timestamps[timestamps.length - 1] - timestamps[0]) / 1000,
    0.001,
  );
  return Number((timestamps.length / elapsedSeconds).toFixed(1));
}

function positionTimestampGapSummary(updates) {
  const lastBySource = new Map();
  const gapsMs = [];
  for (const update of updates) {
    const last = lastBySource.get(update.source);
    if (last && last.timestampMask === update.timestampMask) {
      const mask = update.timestampMask;
      const delta = (update.timestamp - last.timestamp) & mask;
      const halfRange = (mask + 1) / 2;
      if (delta > 0 && delta < halfRange && delta <= 25) {
        gapsMs.push(delta * 10);
      }
    }
    lastBySource.set(update.source, {
      ...update,
      timestamp: update.timestamp & update.timestampMask,
    });
  }
  gapsMs.sort((a, b) => a - b);
  return {
    count: gapsMs.length,
    p50Ms: percentile(gapsMs, 0.5),
    p95Ms: percentile(gapsMs, 0.95),
    maxMs: gapsMs.at(-1) ?? null,
  };
}

function percentile(sortedValues, ratio) {
  if (sortedValues.length === 0) return null;
  const index = Math.min(
    sortedValues.length - 1,
    Math.max(0, Math.round((sortedValues.length - 1) * ratio)),
  );
  return sortedValues[index];
}

function assertCoastingPositionStream(label, positions, releaseAt) {
  const afterRelease = positions.filter(
    (position) =>
      position.weapon === 0 &&
      typeof position.at === "number" &&
      position.at >= releaseAt + 250,
  );
  if (afterRelease.length < 5) {
    throw new Error(`${label} did not keep sending positions after key release`);
  }
  const movingSamples = afterRelease.filter(
    (position) => position.velocityX !== 0 || position.velocityY !== 0,
  );
  const uniquePositions = new Set(afterRelease.map((position) => `${position.x},${position.y}`));
  if (movingSamples.length < 3 || uniquePositions.size < 2) {
    throw new Error(`${label} post-release positions did not show coasting motion`);
  }
}

function assertCoastingPositionRelay(label, updates, releaseAt) {
  const afterRelease = updates.filter(
    (position) =>
      typeof position.at === "number" &&
      position.at >= releaseAt + 250,
  );
  if (afterRelease.length < 5) {
    const observedTimes = updates
      .map((position) => position.at)
      .filter((at) => typeof at === "number")
      .sort((a, b) => a - b);
    throw new Error(
      `${label} did not receive positions after remote key release ` +
        `(after=${afterRelease.length}, total=${updates.length}, ` +
        `cutoff=${releaseAt + 250}, first=${observedTimes[0] ?? "none"}, ` +
        `last=${observedTimes.at(-1) ?? "none"})`,
    );
  }
  const movingSamples = afterRelease.filter(
    (position) => position.velocityX !== 0 || position.velocityY !== 0,
  );
  const uniquePositions = new Set(afterRelease.map((position) => `${position.x},${position.y}`));
  if (movingSamples.length < 3 || uniquePositions.size < 2) {
    throw new Error(`${label} post-release remote positions did not show coasting motion`);
  }
}

function remoteTracePositions(metricsTrace, after = 0) {
  return metricsTrace
    .filter((entry) => entry.at >= after)
    .flatMap((entry) =>
      (entry.remotePlayers || [])
        .filter(
          (player) =>
            Number.isFinite(player.positionX) && Number.isFinite(player.positionY),
        )
        .map((player) => ({ at: entry.at, x: player.positionX, y: player.positionY })),
    );
}

function assertRemoteCoastingTrace(label, metricsTrace, releaseAt) {
  const samples = remoteTracePositions(metricsTrace, releaseAt + 250);
  if (samples.length < 5) {
    const traceTimes = metricsTrace
      .map((entry) => entry.at)
      .filter((at) => Number.isFinite(at));
    const nearbyTrace = metricsTrace
      .filter((entry) => entry.at >= releaseAt - 500)
      .slice(-24)
      .map((entry) => ({
        at: entry.at,
        selfPlayerId: entry.selfPlayerId,
        playerAlive: entry.playerAlive,
        serverPlayerAlive: entry.serverPlayerAlive,
        snapshotId: entry.authoritativeSnapshotId,
        actorIds: (entry.authoritativeSnapshotActors || []).map((actor) => actor.id ?? actor.playerId),
        remotePlayers: (entry.remotePlayers || []).map((player) => ({
          playerId: player.id ?? player.playerId,
          x: player.positionX,
          y: player.positionY,
          alive: player.alive,
        })),
      }));
    throw new Error(
      `${label} did not apply remote positions after key release: ${JSON.stringify({
        releaseAt,
        cutoff: releaseAt + 250,
        samples: samples.length,
        traceEntries: metricsTrace.length,
        firstTraceAt: traceTimes[0] ?? null,
        lastTraceAt: traceTimes.at(-1) ?? null,
        nearbyTrace,
      })}`,
    );
  }
  const uniquePositions = new Set(samples.map((position) => `${position.x},${position.y}`));
  if (uniquePositions.size < 2) {
    throw new Error(`${label} post-release remote positions did not show coasting motion`);
  }
}

async function stopProcess(child) {
  if (!child) return;
  const displays = rememberOwnedXDisplays(child);
  if (child.exitCode !== null || child.signalCode !== null) {
    await finishOwnedXDisplayCleanup(displays);
    return;
  }
  const exited = new Promise((resolve) => child.once("exit", resolve));
  // Process-group signals: a browser launched for a lane leads its own group, and
  // killing only the leader orphans every renderer it spawned.
  killProcessGroup(child, "SIGTERM");
  await Promise.race([exited, sleep(2_000)]);
  if (child.exitCode === null && child.signalCode === null) {
    killProcessGroup(child, "SIGKILL");
    await Promise.race([exited, sleep(1_000)]);
  }
  await finishOwnedXDisplayCleanup(displays);
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run cargo build --target wasm32-unknown-unknown --bin zubspace first`);
  }

  // A leftover browser or X server from an interrupted lane is what makes a smoke
  // fail for the wrong reason.
  assertCleanGateEnvironment("Firefox RTC smoke", {});
  assertMachineHeadroom("Firefox RTC smoke", {});

  const serverPort = await freePort();
  const alphaDriverPort = await freePort();
  const betaDriverPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort);
  const alpha = startGeckodriver(alphaDriverPort, "alpha-firefox");
  const beta = startGeckodriver(betaDriverPort, "beta-firefox");
  let sampled = null;
  let coastingClients = null;
  let encounterSetup = null;

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    await Promise.all([waitForGeckodriver(alpha), waitForGeckodriver(beta)]);
    await Promise.all([createSession(alpha), createSession(beta)]);

    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&password=secret&arena=chaos-zone&packetlog=1&trace=1`;
    try {
      await Promise.all([
        openClient(alpha, `${pageBase}&name=ffalpha`),
        openClient(beta, `${pageBase}&name=ffbeta`),
      ]);
    } catch (error) {
      const states = await Promise.allSettled([pageState(alpha), pageState(beta)]);
      const stateValue = (entry) => entry.status === "fulfilled"
        ? entry.value
        : { pageStateError: entry.reason?.message || String(entry.reason) };
      throw new Error(`${error.message}; RTC startup diagnostics: ${JSON.stringify({
        alpha: stateValue(states[0]),
        beta: stateValue(states[1]),
        serverLog: server.logs.join(""),
        alphaDriverLog: alpha.logs.join(""),
        betaDriverLog: beta.logs.join(""),
      })}`);
    }

    await Promise.all([enterShip(alpha), enterShip(beta)]);
    // Random entry can place the pilots in different AOI tiers. Establish
    // one genuine near-view encounter before measuring remote coasting.
    // Import after this module initializes: the shared helper also imports
    // the generic Firefox transport functions defined here.
    const { rendezvousPlayers } = await import("../tests/e2e/combat_replay.mjs");
    coastingClients = [alpha, beta].map(driver => ({
      evaluate: expression => execute(driver, `return (${expression});`),
      metricsState: () => metricsState(driver),
      hold: (key, ms) => holdFirefoxKeys(driver, [key], ms),
      holdKeys: (keys, ms) => holdFirefoxKeys(driver, keys, ms),
      press: (key, ms = 60) => holdFirefoxKeys(driver, [key], ms),
    }));
    encounterSetup = await rendezvousPlayers(...coastingClients, server, { stageRadiusPx: 384 });
    await Promise.all([probeFirefoxAudioPlayback(alpha), probeFirefoxAudioPlayback(beta)]);
    await sleep(1000);
    const [alphaDrive, betaDrive] = await Promise.all([driveAndRelease(alpha), driveAndRelease(beta)]);
    // Packet-log timestamps come from the browser that received each frame.
    // Capture a post-release cutoff in that same clock domain instead of
    // comparing timestamps across two Firefox processes (or Firefox and Node).
    const [alphaRelayCutoff, betaRelayCutoff] = await Promise.all([
      execute(alpha, "return Date.now();"),
      execute(beta, "return Date.now();"),
    ]);
    const sampleStart = Date.now();
    await sleep(Number.isFinite(sampleMs) ? Math.max(sampleMs, 0) : 3000);

    const [alphaState, betaState] = await Promise.all([pageState(alpha), pageState(beta)]);
    const alphaSummary = summarizePacketLog(alphaState.packetLog);
    const betaSummary = summarizePacketLog(betaState.packetLog);
    // Retain the actual assertion inputs on failure. A later browser read can
    // describe a different state after a collision, lifecycle edge or pause.
    sampled = { alphaState, betaState, alphaSummary, betaSummary,
      alphaDrive, betaDrive, alphaRelayCutoff, betaRelayCutoff,
      sampleStart, sampledAt: Date.now(), encounterSetup };

    assertCoastingPositionStream("alpha firefox local", alphaSummary.sentPositions, alphaDrive.releasedAt);
    assertCoastingPositionStream("beta firefox local", betaSummary.sentPositions, betaDrive.releasedAt);
    assertRemoteCoastingTrace("alpha firefox receiving beta", alphaState.metricsTrace, alphaRelayCutoff);
    assertRemoteCoastingTrace("beta firefox receiving alpha", betaState.metricsTrace, betaRelayCutoff);
    assertFirefoxUiMetrics("alpha", alphaState.uiMetrics);
    assertFirefoxUiMetrics("beta", betaState.uiMetrics);
    assertFirefoxAudio("alpha", alphaState);
    assertFirefoxAudio("beta", betaState);

    let pressureStatus = null;
    await waitUntil("Firefox RTC ingress queues drain", async () => {
      const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
      if (!response.ok) return null;
      const status = await response.json();
      try {
        const audit = assertRtcPressureClean(status, "Firefox smoke final");
        pressureStatus = status;
        return audit;
      } catch {
        return null;
      }
    }, 5_000);
    const pressureAudit = assertRtcPressureClean(pressureStatus, "Firefox smoke final");
    const serverLogAudit = assertRtcServerLogClean(server.logs, "Firefox smoke final");
    const rendererConsole = {
      alpha: assertRendererConsoleClean(alpha.browserConsole?.snapshot() || browserConsoleEvidence(null)),
      beta: assertRendererConsoleClean(beta.browserConsole?.snapshot() || browserConsoleEvidence(null)),
    };

    const elapsedMs = Date.now() - sampleStart;
    console.log(JSON.stringify({
      ok: true,
      transport: "rtc",
      browser: "firefox",
      arena: "chaos-zone",
      tickRate: Number(smokeTickRate),
      sampleElapsedMs: elapsedMs,
      encounterSetup,
      pressureAudit,
      serverLogAudit,
      rendererConsole,
      alpha: {
        sentPositions: alphaSummary.sentPositionCount,
        appliedRemotePositionSamples: remoteTracePositions(alphaState.metricsTrace).length,
        diagnosticParsedPositions: alphaSummary.receivedPositionCount,
        sentPerSecond: timelineRate(alphaSummary.sentPositions),
        appliedRemoteSamplesPerSecond: timelineRate(remoteTracePositions(alphaState.metricsTrace)),
        uiMetrics: alphaState.uiMetrics,
        audioUnlock: alphaState.audioUnlock,
        firefoxAudioProbe: alphaState.firefoxAudioProbe,
      },
      beta: {
        sentPositions: betaSummary.sentPositionCount,
        appliedRemotePositionSamples: remoteTracePositions(betaState.metricsTrace).length,
        diagnosticParsedPositions: betaSummary.receivedPositionCount,
        sentPerSecond: timelineRate(betaSummary.sentPositions),
        appliedRemoteSamplesPerSecond: timelineRate(remoteTracePositions(betaState.metricsTrace)),
        uiMetrics: betaState.uiMetrics,
        audioUnlock: betaState.audioUnlock,
        firefoxAudioProbe: betaState.firefoxAudioProbe,
      },
    }, null, 2));
  } catch (error) {
    const artifactDir = path.join(process.env.ZUBSPACE_E2E_ARTIFACT_ROOT || "/tmp",
      `firefox-smoke-failure-${Date.now()}-${process.pid}`);
    try {
      fs.mkdirSync(path.dirname(artifactDir), { recursive: true });
      fs.mkdirSync(artifactDir);
      const write = (name, value) => fs.writeFileSync(path.join(artifactDir, name),
        `${JSON.stringify(value, null, 2)}\n`);
      write("failure.json", { error: error.stack || String(error),
        measurementCaptured: sampled !== null });
      if (sampled) write("measurement.json", sampled);
      if (coastingClients) write("setup-navigation.json", coastingClients.map(client => client.rendezvousTrace || []));
      write("alpha-console.json", alpha.browserConsole?.snapshot() || browserConsoleEvidence(null));
      write("beta-console.json", beta.browserConsole?.snapshot() || browserConsoleEvidence(null));
      fs.writeFileSync(path.join(artifactDir, "server.log"), server.logs.join(""));
      fs.writeFileSync(path.join(artifactDir, "alpha-geckodriver.log"), alpha.logs.join(""));
      fs.writeFileSync(path.join(artifactDir, "beta-geckodriver.log"), beta.logs.join(""));
    } catch (captureError) {
      throw new Error(`${error.message}; Firefox smoke capture failed: ${captureError.message}`,
        { cause: error });
    }
    throw new Error(`${error.message}; Firefox smoke artifacts: ${artifactDir}`, { cause: error });
  } finally {
    await Promise.all([deleteSession(alpha), deleteSession(beta)]).catch(() => {});
    await Promise.all([stopProcess(alpha.child), stopProcess(beta.child), stopProcess(server.child)]).catch(() => {});
    staticServer.server.close();
  }
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;

if (isMain) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exit(1);
  });
}

export {
  assertCoastingPositionRelay,
  assertCoastingPositionStream,
  createSession,
  cycleFirefoxWindowFocus,
  deleteSession,
  driveAndRelease,
  enterShip,
  execute,
  executeAsync,
  freezeQaTrace,
  freePort,
  firefoxKeyDown,
  firefoxKeyUp,
  firefoxKeyBatch,
  holdFirefoxKeys,
  holdThenChordFirefoxKeys,
  installFirefoxAudioProbe,
  openClient,
  metricsState,
  pageState,
  probeFirefoxAudioPlayback,
  releaseFirefoxKeys,
  parseSnapshotPositionUpdates,
  repoRoot,
  sampleMs,
  screenshot,
  sleep,
  startGeckodriver,
  startRealServer,
  startStaticServer,
  stopProcess,
  summarizePacketLog,
  timelineRate,
  waitForGeckodriver,
  waitForServerListening,
  waitUntil,
  webdriver,
};
