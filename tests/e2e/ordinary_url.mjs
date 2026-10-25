#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";

import fs from "node:fs";
import { assertRendererConsoleClean, browserConsoleEvidence } from "../../tools/browser_console_evidence.mjs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  createSession, deleteSession, execute, firefoxKeyDown, firefoxKeyUp,
  freePort, freezeQaTrace, openClient, pageState, screenshot, sleep, startGeckodriver, firefoxArtifactClient,
  startRealServer, startStaticServer, stopProcess, waitForGeckodriver,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  assertDataChannelDeliveryAudit, assertSecurityRejectAudit, openChromeClient,
  saveBrowserE2eArtifacts, startChrome, waitForChrome, waitUntil,
} from "./combat_replay.mjs";
import {
  assertArtifactProvenanceMatches, collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { validateArtifactPathReferences } from "./artifact_paths.mjs";
import { openCdpSession } from "../../tools/webrtc_capacity_runner.mjs";

// This observes the real WASM request gates; it does not replace them, turn on
// diagnostics, or read/mutate gameplay. In ordinary mode both functions return
// before touching the metrics throttle. Keep the evidence independent of the
// very HUD/sprite payload whose absence this test must prove.
export function installOrdinaryUrlCapture(durationMs) {
  const query = new URLSearchParams(window.location.search);
  if (["trace", "metrics", "packetlog"].some((key) => query.has(key))) {
    throw new Error("ordinary URL must omit every diagnostic option");
  }
  const gates = window.importObject?.env;
  if (typeof gates?.zub_ui_metrics_requested !== "function" ||
      typeof gates?.zub_remote_sprite_trace_requested !== "function") {
    throw new Error("ordinary capture requires the real loaded WASM request gates");
  }
  if (window.__zubspaceOrdinaryCapture) throw new Error("capture already installed");
  const capture = {
    durationMs, timeOrigin: performance.timeOrigin, startedAt: performance.now(), endedAt: null,
    frames: 0, gateCalls: 0, hudRequests: 0, spriteRequests: 0,
    unexpectedPayloadFrames: 0, maximumFrameGapMs: 0, frameGapCount: 0,
    maximumObserverCostMs: 0, observerCostMs: 0, failures: [],
    longTaskSupport: false, longTasks: [], resourceTimings: [],
  };
  window.__zubspaceOrdinaryCapture = capture;
  let longTaskObserver = null;
  if (typeof PerformanceObserver !== "undefined" &&
      PerformanceObserver.supportedEntryTypes?.includes("longtask")) {
    capture.longTaskSupport = true;
    longTaskObserver = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        if (capture.longTasks.length < 32) {
          capture.longTasks.push({ startTime: entry.startTime, duration: entry.duration, name: entry.name });
        }
      }
    });
    longTaskObserver.observe({ type: "longtask" });
  }
  // The opening and closing partial intervals are visible time too. Starting
  // at the first callback or returning before measuring the final one would
  // silently exempt a stall at either edge of the capture.
  let previous = capture.startedAt;
  function sample() {
    const start = performance.now();
    capture.frames += 1;
    const hud = gates.zub_ui_metrics_requested();
    const sprite = gates.zub_remote_sprite_trace_requested();
    capture.gateCalls += 1;
    capture.hudRequests += hud !== 0 ? 1 : 0;
    capture.spriteRequests += sprite !== 0 ? 1 : 0;
    const payloads = ["__zubspaceUiMetrics", "__zubspaceMetricsTrace",
      "__zubspaceRemoteSpriteTrace", "__zubspaceMetricsPublishCount"]
      .filter((key) => window[key] != null);
    if (payloads.length) capture.unexpectedPayloadFrames += 1;
    const gapMs = start - previous;
    capture.maximumFrameGapMs = Math.max(capture.maximumFrameGapMs, gapMs);
    if (gapMs >= 80) capture.frameGapCount += 1;
    if ((hud !== 0 || sprite !== 0 || payloads.length || gapMs >= 80) &&
        capture.failures.length < 16) {
      capture.failures.push({ at: start, hud, sprite, payloads, gapMs });
    }
    previous = start;
    const cost = performance.now() - start;
    capture.observerCostMs += cost;
    capture.maximumObserverCostMs = Math.max(capture.maximumObserverCostMs, cost);
    if (start - capture.startedAt >= durationMs) {
      capture.endedAt = start;
      longTaskObserver?.disconnect();
      capture.resourceTimings = performance.getEntriesByType("resource")
        .filter((entry) => entry.responseEnd >= capture.startedAt)
        .slice(-32).map((entry) => ({
          path: new URL(entry.name).pathname, startTime: entry.startTime,
          responseEnd: entry.responseEnd, duration: entry.duration,
        }));
      return;
    }
    requestAnimationFrame(sample);
  }
  requestAnimationFrame(sample);
}

export function ordinaryUrlEvidence(capture) {
  return {
    ...capture,
    ok: capture?.endedAt != null && capture.endedAt - capture.startedAt >= capture.durationMs &&
      capture.frames >= capture.durationMs / 50 && capture.gateCalls === capture.frames &&
      capture.hudRequests === 0 && capture.spriteRequests === 0 &&
      capture.unexpectedPayloadFrames === 0 && capture.frameGapCount === 0,
  };
}

function acceptedInputs(server) {
  return server.logs.join("").split("\n")
    .filter((line) => line.includes("human authority input trace"))
    .map((line) => Object.fromEntries([...line.matchAll(/(\w+)=(-?[\d.]+)/g)]
      .map((match) => [match[1], Number(match[2])])));
}

async function main() {
  const browser = process.env.ZUBSPACE_E2E_BROWSER || "chrome";
  const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
  const profiling = process.env.ZUBSPACE_E2E_ORDINARY_PROFILE === "1";
  if (profiling && browser !== "chrome") throw new Error("CPU profiling requires Chrome DevTools");
  if (!["chrome", "firefox"].includes(browser) || !["chaos-zone", "extreme"].includes(arena)) {
    throw new Error("ordinary URL gate requires Chrome/Firefox and Chaos/Extreme");
  }
  process.env.ZUBSPACE_SERVER_SIM_OBSERVE = "1";
  const runStartedAt = new Date().toISOString();
  const e2eScript = "tests/e2e/ordinary_url.mjs";
  const initialProvenance = collectArtifactProvenance({ runStartedAt, e2eScript });
  const artifactDir = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
    `/tmp/zubspace-ordinary-${browser}-${arena}-${Date.now()}-${process.pid}`;
  const serverPort = await freePort();
  const staticServer = await startStaticServer();
  const server = startRealServer(serverPort, { zone: arena, botCount: 2 });
  const browserProcess = browser === "chrome"
    ? startChrome(await freePort(), "ordinary-url")
    : startGeckodriver(await freePort(), "ordinary-url");
  const rtc = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
  const url = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtc)}&arena=${arena}&name=url${browser}`;
  let actor = null;
  let capture = null;
  let profiler = null;
  let cpuProfile = null;
  let profileStartBefore = null;
  let profileStartAfter = null;
  const finishProfile = async () => {
    if (!profiler) return;
    try {
      cpuProfile = {
        ...(await profiler.send("Profiler.stop")).profile,
        browserStartBefore: profileStartBefore, browserStartAfter: profileStartAfter,
      };
    } finally {
      profiler.close();
      profiler = null;
    }
  };
  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    if (browser === "chrome") {
      await waitForChrome(browserProcess);
      actor = await openChromeClient(browserProcess, url);
    } else {
      await waitForGeckodriver(browserProcess);
      await createSession(browserProcess);
      await openClient(browserProcess, url);
      actor = {
        ...firefoxArtifactClient(browserProcess),
        evaluate: (expression) => execute(browserProcess, `return (${expression});`),
        keyDown: (key) => firefoxKeyDown(browserProcess, key),
        keyUp: (key) => firefoxKeyUp(browserProcess, key),
        pageState: () => pageState(browserProcess),
        freezeQaTrace: (options) => freezeQaTrace(browserProcess, options),
        screenshot: () => screenshot(browserProcess),
      };
    }
    if (profiling) {
      const targets = await (await fetch(`http://127.0.0.1:${browserProcess.port}/json/list`)).json();
      const target = targets.find((entry) => entry.type === "page" && entry.url === url);
      if (!target?.webSocketDebuggerUrl) throw new Error("ordinary profiling target is missing");
      profiler = await openCdpSession(target.webSocketDebuggerUrl, { timeoutMs: 30000 });
      await profiler.send("Profiler.enable");
      await profiler.send("Profiler.setSamplingInterval", { interval: 1000 });
      profileStartBefore = await actor.evaluate("performance.now()");
      await profiler.send("Profiler.start");
      profileStartAfter = await actor.evaluate("performance.now()");
    }
    await waitUntil("ordinary player has accepted authority", async () => acceptedInputs(server).length > 0);
    // Observe from the first accepted gameplay life, including genuine input
    // delivery. Starting after the thrust gesture could miss a fast client's
    // remaining synchronous bootstrap work. Key commands are permitted here;
    // no browser history/metrics polling is used during capture.
    await actor.evaluate(`(${installOrdinaryUrlCapture.toString()})(60000)`);
    const before = acceptedInputs(server).at(-1);
    const inputOffset = acceptedInputs(server).length;
    await actor.keyDown("ArrowUp");
    try {
      await waitUntil("genuine thrust reaches server", async () =>
        acceptedInputs(server).slice(inputOffset).some((input) => (input.input_bits & 1) !== 0));
      await sleep(450);
    } finally {
      await actor.keyUp("ArrowUp");
    }
    const releaseOffset = acceptedInputs(server).length;
    await waitUntil("released thrust reaches server", async () =>
      acceptedInputs(server).slice(releaseOffset).some((input) => input.input_bits === 0));
    const after = acceptedInputs(server).at(-1);
    const displacement = Math.hypot(after.x - before.x, after.y - before.y);
    if (!(displacement > 0.5) || before.player_id !== after.player_id || before.life_seq !== after.life_seq) {
      throw new Error(`ordinary genuine input did not move one exact life: ${JSON.stringify({ before, after, displacement })}`);
    }
    // No browser polling during the cadence interval. Its independent bounded
    // rAF observer freezes before the single read, without enabling trace=1.
    await sleep(30_000);
    console.log(JSON.stringify({ stage: "ordinary_capture", browser, arena, remainingMs: 30000 }));
    await sleep(30_100);
    capture = ordinaryUrlEvidence(await actor.evaluate("window.__zubspaceOrdinaryCapture"));
    await finishProfile();
    if (!capture.ok) throw new Error(`ordinary diagnostics/cadence failed: ${JSON.stringify(capture)}`);
    const graphics = await actor.evaluate(`(() => {
      if (typeof gl === 'undefined' || !gl) return null;
      const debug = gl.getExtension('WEBGL_debug_renderer_info');
      return { version: gl.getParameter(gl.VERSION), attributes: gl.getContextAttributes(),
        renderer: debug ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER),
        webglSelectionVersion: importObject.env.webgl_selection_version(),
        audioSourceStateAvailable: typeof importObject.env.audio_source_state === 'function' };
    })()`);
    if (!graphics?.attributes?.antialias || graphics.webglSelectionVersion !== 2 || !graphics.audioSourceStateAvailable)
      throw new Error(`fork runtime/antialias contract unavailable on test host: ${JSON.stringify(graphics)}`);
    const finalState = await actor.pageState();
    const channels = finalState.networkLog.filter((entry) => entry.message === "rtc data channel open")
      .map((entry) => entry.detail?.label);
    if (!["zubspace-game", "zubspace-events", "zubspace-reliable"].every((label) => channels.includes(label))) {
      throw new Error(`ordinary URL lacks normal RTC channels: ${JSON.stringify(channels)}`);
    }
    const dataChannelDeliveryAudit = await assertDataChannelDeliveryAudit({
      server, serverPort, browserLogs: [browserProcess.logs],
      disconnect: async () => {
        await actor.evaluate("(window.__zubspaceSendDisconnectForTest(), true)");
      },
    });
    let cleanup;
    await waitUntil("ordinary RTC session and queues clean up", async () => {
      cleanup = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
      return cleanup.clients === 0 && cleanup.rtc_sessions === 0 &&
        cleanup.rtc_outbound_queue_packets === 0 && cleanup.rtc_outbound_queue_bytes === 0 &&
        Object.values(cleanup.rtc_inbound_queue_packets || {}).every((count) => count === 0);
    }, 10000);
    const securityAudit = assertSecurityRejectAudit(server);
    const provenance = await collectArtifactProvenanceAsync({ runStartedAt, e2eScript });
    assertArtifactProvenanceMatches(initialProvenance, provenance);
    const successArtifacts = await saveBrowserE2eArtifacts({
      artifactDir, server, staticServer, urls: [url],
      clients: [{ label: "ordinary", client: actor }],
      extraLogs: {
        browser: browserProcess.logs,
        ...(cpuProfile ? { "cpu-profile": JSON.stringify(cpuProfile) } : {}),
      }, provenance,
    });
    const result = {
      ok: !profiling, diagnosticOnly: profiling, browser, arena, transport: "rtc", url, capture,
      genuineInput: { before, after, displacement }, channels, graphics,
      dataChannelDeliveryAudit, securityAudit, cleanup, provenance, successArtifacts,
    };
    assertRendererConsoleClean(browserConsoleEvidence(actor));
    const resultPath = path.join(artifactDir, "result.json");
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
    validateArtifactPathReferences(result, resultPath);
    console.log(JSON.stringify({ ok: !profiling, diagnosticOnly: profiling, browser, arena, capture, resultPath }));
  } catch (error) {
    await finishProfile().catch(() => {});
    const failureDir = `${artifactDir}-failure`;
    await saveBrowserE2eArtifacts({
      artifactDir: failureDir, server, staticServer, urls: [url],
      clients: [{ label: "ordinary", client: actor }],
      extraLogs: {
        browser: browserProcess.logs, failure: error.stack, capture: JSON.stringify(capture),
        ...(cpuProfile ? { "cpu-profile": JSON.stringify(cpuProfile) } : {}),
      },
    });
    throw new Error(`${error.message}; retained at ${failureDir}`, { cause: error });
  } finally {
    profiler?.close();
    if (browser === "chrome") await actor?.close().catch(() => {});
    else await deleteSession(browserProcess);
    await Promise.all([stopProcess(browserProcess.child), stopProcess(server.child)]);
    staticServer.server.close();
    if (browserProcess.userDataDir) fs.rmSync(browserProcess.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => { console.error(error.stack); process.exitCode = 1; });
}
