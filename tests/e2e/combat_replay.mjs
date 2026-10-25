#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";

import fs from "node:fs";
import { createSpawnRegionFixture } from "./fixture_spawn_region.mjs";
import { serverLogField, serverLogMessage } from "../../tools/server_log_fields.mjs";
import { stageAoiWatcher, aoiOutboundProgress } from "./aoi_outbound_staging.mjs";
import { writeLogChunksAsync } from "../../tools/log_chunks.mjs";
import { commandWithCpuSet } from "../../tools/cpu_affinity.mjs";
import { chromeGraphicsArguments } from "../../tools/chrome_graphics.mjs";
import {
  assertCleanGateEnvironment,
  assertDevToolsPortFree,
  assertMachineHeadroom,
  loadMeasurementNote,
  registerGateProcess,
  rememberOwnedXDisplays,
  browserLaunchEnvironment,
} from "../../tools/e2e_gate_hygiene.mjs";
import { serverTimingEvidence } from "./server_timing_evidence.mjs";
import { captureServerLaunchEvidence, captureServerProcessEvidence } from "./server_process_evidence.mjs";
import { remoteDisplayStepEvidence } from "./remote_motion_cadence.mjs";
import { uninterruptedMotionStage } from "./motion_staging.mjs";
import path from "node:path";
import { spawn } from "node:child_process";
import { usableXDisplay } from "../../tools/x_display.mjs";
import { pathToFileURL } from "node:url";

import {
  e2eWindowClass,
  freePort,
  repoRoot,
  sleep,
  startRealServer,
  startStaticServer,
  stopProcess,
  summarizePacketLog,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  parseServerDamageEvents,
  parseServerDeathEvents,
  parseServerProjectileEvents,
  parseServerRespawnEvents,
  parseZubspaceEvents,
} from "../../tools/server_events.mjs";
import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { c2sRecoveryLogSummaryAsync } from "./c2s_recovery_analysis_async.mjs";
import { recoveryMembershipReady } from "./recovery_membership.mjs";
import {
  assertRtcPressureClean,
  assertRtcPressureSummaryClean,
  rtcPressureSummary,
} from "../../tools/rtc_gate_audit.mjs";
import {
  artifactFileManifest,
  assertRetainedArtifactDirectoryAvailable,
  createAtomicArtifactDirectory,
  validateArtifactPathReferences,
} from "./artifact_paths.mjs";
import {
  frameGapDelta,
  longTaskWitnessInstallScript,
  longTaskWitnessReadScript,
  longTaskWitnessTopScript,
} from "./long_task_witness.mjs";
import { createChaosRouteReplanZone, createWormholeE2eZone } from "./wormhole_fixture.mjs";
import { driveRouteWithReplans } from "./chaos_route_replanner.mjs";
import { chaosSafeRouteToAuthoredSafety } from "./chaos_safe_route.mjs";
import { loadRendezvousMap, planPlayerRendezvous } from "./player_rendezvous.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { prepareKeyboardLifecycleOrigin } from "./keyboard_lifecycle_origin.mjs";
import { planTimedPrizePreparation } from "./timed_prize_preparation.mjs";
import { planNaturalPrizeNavigation } from "./natural_prize_navigation.mjs";
import { authoredMapGeometry, authoredSafetyMargin, deepestStagingPoint } from "./authored_map_geometry.mjs";
import { withPreparedBrowserArtifactStates } from "./qa_trace_capture.mjs";
import { prepareCdpJson } from "./cdp_json_capture.mjs";
import { writeQaTraceArtifact } from "./qa_trace_artifacts.mjs";
import { browserConsoleEvidence, isRejectedDraw } from "../../tools/browser_console_evidence.mjs";

const chromeBin = process.env.CHROME_BIN || "google-chrome";
const applicationRoot = path.resolve(repoRoot, process.env.ZUBSPACE_E2E_STATIC_ROOT || ".");
const wasmPath = process.env.ZUBSPACE_E2E_STATIC_ROOT
  ? path.relative(repoRoot, path.join(applicationRoot, "zubspace.wasm"))
  : "target/wasm32-unknown-unknown/debug/zubspace.wasm";
const clientEntry = path.relative(repoRoot, path.join(applicationRoot, "index.html"));
const buildProvenanceOptions = {
  wasm: wasmPath,
  clientEntry,
  serverBinary: `target/${process.env.ZUBSPACE_E2E_SERVER_PROFILE || "debug"}/zubspace_server`,
};
const defaultArena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const defaultCycleCount = Number(process.env.ZUBSPACE_E2E_CYCLES || "2");
const simulatedTxLossPct = Number(process.env.ZUBSPACE_SERVER_TX_LOSS_PCT || "0");
const simulatedTxLossEnabled = simulatedTxLossPct > 0;
const simulatedTxLossSeed = process.env.ZUBSPACE_SERVER_TX_LOSS_SEED == null
  ? null
  : Number(process.env.ZUBSPACE_SERVER_TX_LOSS_SEED);
const shipChangeEnergyGate = process.env.ZUBSPACE_E2E_SHIP_ENERGY === "1";
const snapshotLossGate = process.env.ZUBSPACE_E2E_SNAPSHOT_LOSS === "1";
if (snapshotLossGate && process.env.ZUBSPACE_SERVER_TX_LOSS_AFTER_BOOTSTRAP == null) {
  process.env.ZUBSPACE_SERVER_TX_LOSS_AFTER_BOOTSTRAP = "1";
}
const movementDeviationGate = process.env.ZUBSPACE_E2E_MOVEMENT_DEVIATION_GATE === "1";
const c2sInputRecoveryGate = process.env.ZUBSPACE_E2E_C2S_INPUT_RECOVERY === "1";
const movementCorrectionGate = process.env.ZUBSPACE_E2E_MOVEMENT_CORRECTION === "1";
const semanticInputGate = process.env.ZUBSPACE_E2E_SEMANTIC_INPUT === "1";
const staleDoorGate = process.env.ZUBSPACE_E2E_STALE_DOOR === "1";
const selectiveClockStallGate = process.env.ZUBSPACE_E2E_CLOCK_STALL === "1";
const baselineReceiptGate = process.env.ZUBSPACE_E2E_BASELINE_RECEIPT === "1";
const warpRelocationGate = process.env.ZUBSPACE_E2E_WARP_RELOCATION === "1";
const prizePhaseOneGate = process.env.ZUBSPACE_E2E_PRIZES_PHASE1 === "1";
const negativeEnergyGate = process.env.ZUBSPACE_E2E_NEGATIVE_ENERGY === "1";
const negativeEnergyLoadout = process.env.ZUBSPACE_E2E_NEGATIVE_ENERGY_LOADOUT || "maximum";
const portalPhaseTwoGate = process.env.ZUBSPACE_E2E_PORTAL_PHASE2 === "1";
const wormholeAuthorityGate = process.env.ZUBSPACE_E2E_WORMHOLE_AUTHORITY === "1";
const chaosRouteReplanGate = process.env.ZUBSPACE_E2E_CHAOS_ROUTE_REPLAN === "1";
const prizePhaseOneDamageSeed = 50;
const movementPrizeIds = new Set([1, 3, 11, 12]);
const chaosWarbirdMovementPrizeValues = new Map([
  [1, { base: 98.4, cap: 115.0 }],
  [3, { base: 0.625, cap: 0.725 }],
  [11, { base: 170.0, cap: 190.0 }],
  [12, { base: 300.0, cap: 325.0 }],
]);
const headlessChrome = process.env.ZUBSPACE_E2E_HEADLESS !== "0";
const successArtifactDir = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
  (process.env.ZUBSPACE_E2E_ARTIFACT_ROOT
    ? path.join(process.env.ZUBSPACE_E2E_ARTIFACT_ROOT, `zubspace-combat_replay-${Date.now()}-${process.pid}`)
    : "");
assertRetainedArtifactDirectoryAvailable(successArtifactDir);
const CLIENT_DEATH_REQUEST = 0x05;
const GAME_CHANNEL_LABEL = "zubspace-game";
const EVENT_CHANNEL_LABEL = "zubspace-events";
const RELIABLE_CHANNEL_LABEL = "zubspace-reliable";
// src/protocol/client_input_record.rs: four-byte packet header followed by
// four input bytes and the fourteen-byte CombatViewReceiptV1 for each entry.
const CLIENT_INPUT_RECORD_HEADER_LEN = 4;
const CLIENT_INPUT_RECORD_ENTRY_LEN = 18;
const instrumentationReadWindows = [];

function logicalInputLifecyclePaused(metrics) {
  const local = metrics?.localGameplayState || {};
  return metrics?.playerAlive !== true ||
    metrics?.serverPlayerAlive !== true ||
    local.authorityScopeAlive !== true ||
    local.authorityPhase !== "Live";
}

function exactLiveGameplayScope(metrics) {
  const local = metrics?.localGameplayState || {};
  return (
    metrics?.playerAlive === true &&
    metrics.serverPlayerAlive === true &&
    (metrics.playerRespawnTimer ?? 0) <= 0 &&
    Number.isInteger(metrics.playerLifeSeq) &&
    local.authorityPhase === "Live" &&
    local.authorityTransitionKind == null &&
    local.authorityApplicationBarrierKind == null &&
    local.authorityApplicationBarrierCount === 0 &&
    local.authorityScopeAlive === true &&
    local.authorityScopeLifeSeq === metrics.playerLifeSeq &&
    local.pendingShipResetPosition === false &&
    local.pendingShipResetDeliveryId == null &&
    local.pendingShipResetLifeSeq == null &&
    local.pendingShipResetSequence == null &&
    local.pendingFrequencyBaselineDeliveryId == null &&
    local.networkPrizeBaselinePending === false &&
    local.zoneMapReady === true &&
    local.doorClockFresh === true
  );
}

let warnedAboutMissingDisplay = false;

function startChrome(port, label, { highRefresh = process.env.ZUBSPACE_E2E_HIGH_REFRESH === "1", display = null, cpuSet = null, graphicsBackend = "swiftshader", allowBackgroundThrottling = false } = {}) {
  const userDataDir = `/tmp/zubspace-combat-e2e-${label}-${port}-${process.pid}`;
  // A leaked browser or X server from an interrupted lane is what turns a timing
  // gate into a false regression, and a port answered by somebody else's DevTools
  // silently drives the wrong instance. Refuse both before spawning.
  assertCleanGateEnvironment(`${label} Chrome`, {});
  assertDevToolsPortFree(port);
  const args = [
    ...chromeGraphicsArguments(graphicsBackend),
    "--password-store=basic",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-search-engine-choice-screen",
    "--disable-extensions",
    ...(allowBackgroundThrottling ? [] : [
      "--disable-background-timer-throttling",
      "--disable-backgrounding-occluded-windows",
      "--disable-renderer-backgrounding",
    ]),
    "--remote-debugging-address=127.0.0.1",
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${userDataDir}`,
    "about:blank",
  ];
  if (process.env.ZUBSPACE_E2E_REQUIRE_AUDIO_GESTURE !== "1") {
    args.splice(args.length - 3, 0, "--autoplay-policy=no-user-gesture-required");
  }
  if (highRefresh) {
    // Browser scheduler stress only; actual game draw cadence is measured by
    // the keyboard gate. This flag alone is not high-refresh evidence.
    args.unshift("--disable-frame-rate-limit");
  }
  if (headlessChrome) {
    args.unshift("--headless=new");
  } else {
    // Headed windows carry the shared test window class so desktop tooling
    // (scripts/e2e-hyprland-tools) can park them on a background workspace
    // without touching the user's own Chrome windows.
    args.unshift("--ozone-platform=x11", `--class=${e2eWindowClass}`);
  }
  const launch = commandWithCpuSet(chromeBin, args, cpuSet);
  let launchCommand = launch.command;
  let launchArgs = launch.args;
  const launchEnv = browserLaunchEnvironment(
    display == null ? process.env : { ...process.env, DISPLAY: display },
    `${label} Chrome`,
  );
  if (!headlessChrome && display == null && !usableXDisplay(process.env.DISPLAY)) {
    // Headed Chrome with no reachable display dies before it opens a window, and every gate that reaches this
    // line reported it as a DevTools failure. Provision a display here rather than in whichever target was
    // most recently hit: Xvfb is not on PATH in this environment, so it arrives the same way the Firefox arms
    // arrive, through nix-shell. xvfb-run sets DISPLAY for the browser it launches.
    if (!warnedAboutMissingDisplay) {
      warnedAboutMissingDisplay = true;
      console.log(`No reachable X display (DISPLAY=${JSON.stringify(process.env.DISPLAY)}`
        + `); running headed Chrome under xvfb-run for this lane and any later lane`);
    }
    const inner = [launchCommand, ...launchArgs]
      .map((arg) => `'${String(arg).replaceAll("'", `'\\''`)}'`)
      .join(" ");
    launchCommand = "nix-shell";
    launchArgs = ["-p", "xvfb-run", "--run", `xvfb-run -a -s "-screen 0 1440x1000x24" ${inner}`];
  }
  const child = spawn(launchCommand, launchArgs, {
    env: launchEnv,
    // Own process group, so the gate's exit/signal cleanup takes the browser and
    // every renderer/GPU child with it instead of orphaning them.
    detached: true,
  });
  registerGateProcess(child, { userDataDir });
  const logs = [];
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => logs.push(chunk));
  child.stderr.on("data", (chunk) => logs.push(chunk));
  return { child, userDataDir, port, label, logs };
}

function chromeExitDetail(chrome) {
  // Chrome exits instantly on an unusable display - ozone prints `Missing X server or $DISPLAY`
  // and dies - which otherwise surfaces as a bare "exited before DevTools" and sends the reader
  // into the artifact directory looking for chrome.log. Carry the child's own words.
  const lines = (chrome.logs ?? []).join("").split("\n").filter((line) => line.trim().length > 0);
  if (lines.length === 0) return "";
  return `: ${lines.slice(-3).join(" | ").slice(-300)}`;
}

async function waitForChrome(chrome) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (chrome.child.exitCode !== null || chrome.child.signalCode !== null) {
      throw new Error(`${chrome.label} Chrome exited before DevTools became available` +
        `${chromeExitDetail(chrome)}`);
    }
    try {
      const response = await fetch(`http://127.0.0.1:${chrome.port}/json/version`);
      if (response.ok) {
        // Page.close can end the launcher before stopProcess runs. Capture
        // its display ownership while DevTools proves the browser is live.
        rememberOwnedXDisplays(chrome.child);
        return;
      }
    } catch {
      // Chrome is still starting.
    }
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${chrome.label} Chrome DevTools${chromeExitDetail(chrome)}`);
}

async function openChromeClient(chrome, pageUrl, { beforeReady = null, initScript = null, waitForReveal = true, targetId = null } = {}) {
  const deadline = Date.now() + 10_000;
  let target = null;
  let lastError = null;
  while (Date.now() < deadline) {
    if (chrome.child.exitCode !== null || chrome.child.signalCode !== null) {
      throw new Error(`${chrome.label} Chrome exited before a DevTools target could be opened` +
        `${chromeExitDetail(chrome)}`);
    }
    try {
      const response = await fetch(
        targetId
          ? `http://127.0.0.1:${chrome.port}/json/list`
          : `http://127.0.0.1:${chrome.port}/json/new?${encodeURIComponent("about:blank")}`,
        targetId ? {} : { method: "PUT" },
      );
      if (!response.ok) {
        throw new Error(`DevTools target request returned HTTP ${response.status}`);
      }
      const payload = await response.json();
      const candidate = targetId ? payload.find(entry => entry.id === targetId) : payload;
      if (!candidate?.webSocketDebuggerUrl) {
        throw new Error("DevTools target response omitted webSocketDebuggerUrl");
      }
      target = candidate;
      break;
    } catch (error) {
      lastError = error;
    }
    await sleep(100);
  }
  if (!target) {
    const suffix = lastError ? `; last error: ${lastError.message}` : "";
    throw new Error(`timed out opening ${chrome.label} Chrome DevTools target${suffix}`);
  }
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  let nextId = 1;
  const pending = new Map();
  const events = [];

  ws.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) {
      const { resolve, reject, timeout } = pending.get(message.id);
      pending.delete(message.id);
      clearTimeout(timeout);
      if (message.error) reject(new Error(JSON.stringify(message.error)));
      else resolve(message.result || {});
      return;
    }
    if (message.method === "Runtime.consoleAPICalled") {
      const type = message.params.type;
      const text = (message.params.args || [])
        .map((arg) => arg.value ?? arg.description ?? "")
        .join(" ");
      if (["error", "warning", "assert"].includes(type) || text === "fix") {
        events.push({ type, text });
      }
    } else if (message.method === "Runtime.exceptionThrown") {
      events.push({
        type: "exception",
        text: message.params.exceptionDetails?.exception?.description ||
          message.params.exceptionDetails?.text || "exception",
      });
    } else if (message.method === "Page.javascriptDialogOpening") {
      events.push({ type: "dialog", text: message.params.message });
    } else if (message.method === "Log.entryAdded") {
      const entry = message.params.entry;
      if (["error", "warning"].includes(entry.level)) {
        events.push({ type: entry.level, text: entry.text });
      }
    }
  };

  ws.onclose = (event) => {
    for (const request of pending.values()) {
      clearTimeout(request.timeout);
      request.reject(new Error(
        `${chrome.label} DevTools WebSocket closed (code=${event.code}, reason=${event.reason || "none"}, chromeExit=${chrome.child.exitCode}, chromeSignal=${chrome.child.signalCode})`,
      ));
    }
    pending.clear();
  };

  const send = (method, params = {}, timeoutMs = 5_000) => {
    const id = nextId++;
    ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        pending.delete(id);
        const state = starvationState(chrome.label);
        state.timedOut += 1;
        state.lastMethod = method;
        // The stall is measured from the last command the page did answer.
        if (state.starvedSince === 0) {
          state.starvedSince = state.lastSuccessAt;
        }
        reject(new Error(`${chrome.label} DevTools command timed out: ${method}`));
      }, timeoutMs);
      pending.set(id, {
        resolve: (value) => {
          const state = starvationState(chrome.label);
          state.lastSuccessAt = Date.now();
          if (state.timedOut > 0 && state.longTasks === null) {
            // The page answers again, so read the witness it has been collecting. The sentinel is set
            // before the await, which is what stops the witness read itself from re-triggering this.
            state.longTasks = [];
            // A long task is only reported once it completes, and the observer callback runs after the
            // page yields - which can be later than the first command that answers again. Read a beat
            // later so the entry is there.
            setTimeout(() => {
              void send("Runtime.evaluate", { expression: LONGTASK_READ, returnByValue: true })
                .then((response) => {
                  state.longTasks = JSON.parse(response?.result?.value ?? "[]");
                })
                .catch(() => {});
            }, 500);
          }
          resolve(value);
        },
        reject,
        timeout,
      });
    });
  };

  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
    setTimeout(() => reject(new Error(`${chrome.label} DevTools WebSocket timeout`)), 5_000);
  });

  await send("Runtime.enable");
  await send("Page.enable");
  await send("Log.enable");
  await send("Emulation.setDeviceMetricsOverride", {
    width: 1280,
    height: 720,
    deviceScaleFactor: 1,
    mobile: false,
  });
  if (initScript) await send("Page.addScriptToEvaluateOnNewDocument", { source: initScript });
  await send("Page.navigate", { url: pageUrl });
  // Installed before the readiness waits so a boot-phase stall is already recorded by the time a gate
  // needs to report it.
  await send("Runtime.evaluate", { expression: LONGTASK_OBSERVE, returnByValue: true }).catch(() => {});
  await send("Page.bringToFront").catch(() => {});
  if (beforeReady) await beforeReady({ send, evaluate: expression => evaluateChrome(send, expression) });
  await waitUntil(`${chrome.label} WASM page`, async () => {
    const state = await evaluateChrome(send, `(() => ({
      canvas: !!document.querySelector('canvas'),
      hasWasm: performance.getEntriesByType('resource').some((entry) => /zubspace\\.wasm/.test(entry.name)),
    }))()`);
    return state.canvas && state.hasWasm;
  }, 20_000);
  await focusCanvas(send);
  try {
    await waitUntil(`${chrome.label} RTC channels`, () => evaluateChrome(send, `(() => {
      const log = window.__zubspaceNetworkLog || [];
      const labels = log.filter(e => e.message === 'rtc data channel open').map(e => e.detail?.label);
      return log.some(e => e.message === 'rtc answer accepted') &&
        ['zubspace-game', 'zubspace-events', 'zubspace-reliable'].every(label => labels.includes(label));
    })()`), 20_000);
  } catch (error) {
    const diagnostic = await evaluateChrome(send, `({
      networkLog: (window.__zubspaceNetworkLog || []).slice(-40),
      lifecycle: window.__zubspaceLifecycle || null,
      readyState: document.readyState,
      contextLost: window.gl?.isContextLost?.() ?? null,
      loadingText: document.querySelector('#loading-overlay')?.textContent?.slice(-600) ?? null
    })`).catch(() => null);
    const bootEvents = events.slice(-12).map(({ type, text }) => ({ type, text: String(text).slice(0, 1000) }));
    throw new Error(`${error.message}; RTC startup: ${JSON.stringify({ ...diagnostic, bootEvents })}`, { cause: error });
  }
  // A baseline/file fault fixture must inspect the pending state while the
  // product splash remains visible. Ordinary clients still require reveal.
  if (waitForReveal) await waitUntil(`${chrome.label} loading overlay`, async () => {
    return evaluateChrome(
      send,
      `(() => {
        const overlay = document.querySelector('#loading-overlay');
        return !overlay || overlay.classList.contains('hidden');
      })()`,
    );
  }, 20_000);
  await focusCanvas(send);

  return {
    label: chrome.label,
    events,
    inputTrace: [],
    async evaluate(expression) {
      return evaluateChrome(send, expression);
    },
    async evaluateAwait(expression) {
      return evaluateChromeAwait(send, expression);
    },
    async setCpuThrottlingRate(rate) {
      await send("Emulation.setCPUThrottlingRate", { rate });
    },
    async setViewport(width, height) {
      await send("Emulation.setDeviceMetricsOverride", {
        width, height, deviceScaleFactor: 1, mobile: false,
      });
    },
    async focus() {
      await focusCanvas(send);
    },
    acquireInputFocusLease() {
      return acquireChromeInputFocusLease(send);
    },
    async press(key, holdMs = 80, focusBefore = true) {
      if (focusBefore) await focusCanvas(send);
      const event = keyEvent(key);
      await dispatchChromeKey(send, "keyDown", event);
      await recordInputTrace(send, chrome.label, "keyDown", event);
      if (holdMs > 0) await sleep(holdMs);
      await dispatchChromeKey(send, "keyUp", event).catch(() => {});
      await recordInputTrace(send, chrome.label, "keyUp", event);
    },
    async keyDown(key) {
      await focusCanvas(send);
      const event = keyEvent(key);
      await dispatchChromeKey(send, "keyDown", event);
      await recordInputTrace(send, chrome.label, "keyDown", event);
    },
    async keyBatch(changes) {
      await focusCanvas(send);
      // Send genuine CDP events back-to-back. Do not insert Runtime tracing or
      // focus round trips between keys; the page's physical trace is the witness.
      await Promise.all(changes.map(({ type, key }) =>
        dispatchChromeKey(send, type, keyEvent(key))));
    },
    async keyDownRepeat(key) {
      await focusCanvas(send);
      const event = keyEvent(key);
      await dispatchChromeKey(send, "keyDown", { ...event, autoRepeat: true });
      await recordInputTrace(send, chrome.label, "keyDownRepeat", event);
    },
    async keyUp(key) {
      const event = keyEvent(key);
      await dispatchChromeKey(send, "keyUp", event).catch(() => {});
      await recordInputTrace(send, chrome.label, "keyUp", event);
    },
    async insertText(text) {
      await send("Input.insertText", { text });
    },
    async typeChatText(text) {
      await focusCanvas(send);
      for (const character of text) {
        const event = keyEvent(character);
        await send("Input.dispatchKeyEvent", {
          type: "char",
          key: event.key,
          code: event.code,
          text: event.key,
          unmodifiedText: event.key,
          windowsVirtualKeyCode: event.keyCode,
          nativeVirtualKeyCode: event.keyCode,
        });
        await recordInputTrace(send, chrome.label, "char", event);
        await sleep(20);
      }
    },
    async pressWithRepeats(key, holdMs = 650, repeatEveryMs = 60) {
      await focusCanvas(send);
      const event = keyEvent(key);
      const traceEvents = [];
      const releaseAt = Date.now() + holdMs;
      await dispatchChromeKey(send, "keyDown", event);
      traceEvents.push({ phase: "keyDown", at: Date.now() });
      while (Date.now() + repeatEveryMs < releaseAt) {
        await sleep(Math.max(0, Math.min(repeatEveryMs, releaseAt - Date.now())));
        if (Date.now() >= releaseAt) break;
        await dispatchChromeKey(send, "keyDown", { ...event, autoRepeat: true });
        traceEvents.push({ phase: "keyDownRepeat", at: Date.now() });
      }
      const remaining = releaseAt - Date.now();
      if (remaining > 0) await sleep(remaining);
      await dispatchChromeKey(send, "keyUp", event).catch(() => {});
      traceEvents.push({ phase: "keyUp", at: Date.now() });
      // Trace serialization can take tens of milliseconds on a debug WASM
      // page. Record after release with the original event timestamps so
      // instrumentation cannot lengthen the physical hold or create another
      // authored fire opportunity.
      for (const traceEvent of traceEvents) {
        await recordInputTrace(
          send,
          chrome.label,
          traceEvent.phase,
          event,
          traceEvent.at,
        );
      }
    },
    async hold(key, holdMs = 500) {
      await focusCanvas(send);
      const event = keyEvent(key);
      await dispatchChromeKey(send, "keyDown", event);
      await recordInputTrace(send, chrome.label, "keyDown", event);
      await sleep(holdMs);
      await dispatchChromeKey(send, "keyUp", event).catch(() => {});
      await recordInputTrace(send, chrome.label, "keyUp", event);
    },
    async pulseOnFreshInputStep(key, holdMs = 20, { requireAppliedStep = false } = {}) {
      await focusCanvas(send);
      const initial = await chromeMetricsState(send);
      const initialSequence = initial.uiMetrics?.clientInputSeq;
      if (!Number.isInteger(initialSequence)) {
        throw new Error(`${chrome.label} has no logical input sequence for pulse alignment`);
      }
      if (logicalInputLifecyclePaused(initial.uiMetrics)) return false;
      const deadline = Date.now() + 1_000;
      let currentSequence = initialSequence;
      let current = initial;
      while (Date.now() < deadline && currentSequence === initialSequence) {
        await sleep(1);
        current = await chromeMetricsState(send);
        currentSequence = current.uiMetrics?.clientInputSeq;
      }
      if (!Number.isInteger(currentSequence) || currentSequence === initialSequence) {
        // The sequencer intentionally freezes logical input while a death,
        // respawn, or other lifecycle boundary is unresolved. A bot can kill
        // the pilot after the navigation controller's preceding live sample,
        // so classify that exact race as a paused control step and let the
        // caller resume after the authoritative successor life is live. A
        // live scope that stops advancing remains a hard product failure.
        if (logicalInputLifecyclePaused(current.uiMetrics)) return false;
        const local = current.uiMetrics?.localGameplayState || {};
        throw new Error(`${chrome.label} logical input did not advance for pulse alignment: ${JSON.stringify({
          clientInputSeq: currentSequence,
          playerAlive: current.uiMetrics?.playerAlive,
          serverPlayerAlive: current.uiMetrics?.serverPlayerAlive,
          playerLifeSeq: current.uiMetrics?.playerLifeSeq,
          authorityPhase: local.authorityPhase,
          authorityScopeAlive: local.authorityScopeAlive,
          authorityScopeLifeSeq: local.authorityScopeLifeSeq,
        })}`);
      }

      // Record only after key-up. Capturing the full evidence row between the
      // edges can itself hold a nominal 20 ms pulse for several extra render
      // frames, making a genuine low-speed braking input physically unable to
      // stop instead of reverse. Both edges remain real CDP keyboard events;
      // their exact dispatch times are retained separately from the later
      // evidence sample.
      const event = keyEvent(key);
      const movementBit = {
        w: 1, ArrowUp: 1, s: 2, ArrowDown: 2,
        a: 4, ArrowLeft: 4, d: 8, ArrowRight: 8,
      }[key];
      if (requireAppliedStep && movementBit === undefined) {
        throw new Error(`cannot witness a movement step for ${key}`);
      }
      await dispatchChromeKey(send, "keyDown", event);
      const keyDownAt = Date.now();
      let delivered = true;
      try {
        if (holdMs > 0) await sleep(holdMs);
        if (requireAppliedStep) {
          // A sub-frame fine tap aligned after a fresh frame can otherwise
          // disappear before the next input step. Observe the game's last
          // consumed PlayerInput, not just the browser's held-key state.
          const appliedDeadline = Date.now() + 1_000;
          while (true) {
            const observed = await chromeMetricsState(send);
            if (logicalInputLifecyclePaused(observed.uiMetrics)) {
              delivered = false;
              break;
            }
            const observedSequence = observed.uiMetrics?.clientInputSeq;
            const bits = observed.uiMetrics?.localGameplayState?.inputBits;
            if (Number.isInteger(observedSequence) && observedSequence !== currentSequence
                && Number.isInteger(bits) && (bits & movementBit) !== 0) break;
            if (Date.now() >= appliedDeadline) {
              throw new Error(`${chrome.label} fine movement pulse reached no logical input step`);
            }
            await sleep(1);
          }
        }
      } finally {
        await dispatchChromeKey(send, "keyUp", event).catch(() => {});
        const keyUpAt = Date.now();
        await recordInputTrace(send, chrome.label, "keyDown", event, keyDownAt);
        await recordInputTrace(send, chrome.label, "keyUp", event, keyUpAt);
      }
      return delivered;
    },
    async holdKeys(keys, holdMs = 500) {
      await focusCanvas(send);
      const events = keys.map(keyEvent);
      // Submit chord edges together. Sequential CDP round trips can straddle
      // a render boundary, turning intended F3+Z into two unrelated semantic
      // batches even though a human pressed them together.
      await Promise.all(events.map((event) => dispatchChromeKey(send, "keyDown", event)));
      for (const event of events) {
        await recordInputTrace(send, chrome.label, "keyDown", event);
      }
      await sleep(holdMs);
      const releaseEvents = [...events].reverse();
      await Promise.all(releaseEvents.map((event) =>
        dispatchChromeKey(send, "keyUp", event).catch(() => {})
      ));
      for (const event of releaseEvents) {
        await recordInputTrace(send, chrome.label, "keyUp", event);
      }
    },
    async holdThenChord(firstKey, secondKey, firstOnlyMs = 180, chordMs = 500) {
      await focusCanvas(send);
      const first = keyEvent(firstKey);
      const second = keyEvent(secondKey);
      await dispatchChromeKey(send, "keyDown", first);
      await recordInputTrace(send, chrome.label, "keyDown", first);
      await sleep(firstOnlyMs);
      await dispatchChromeKey(send, "keyDown", second);
      await recordInputTrace(send, chrome.label, "keyDown", second);
      await sleep(chordMs);
      await dispatchChromeKey(send, "keyUp", second).catch(() => {});
      await recordInputTrace(send, chrome.label, "keyUp", second);
      await dispatchChromeKey(send, "keyUp", first).catch(() => {});
      await recordInputTrace(send, chrome.label, "keyUp", first);
    },
    async holdKeysOnFreshInputStep(keys, holdMs = 500) {
      await focusCanvas(send);
      const initial = await chromeMetricsState(send);
      const initialSequence = initial.uiMetrics?.clientInputSeq;
      if (!Number.isInteger(initialSequence)) {
        throw new Error(`${chrome.label} has no logical input sequence for chord alignment`);
      }
      const deadline = Date.now() + 1_000;
      let currentSequence = initialSequence;
      while (Date.now() < deadline && currentSequence === initialSequence) {
        await sleep(1);
        currentSequence = (await chromeMetricsState(send)).uiMetrics?.clientInputSeq;
      }
      if (!Number.isInteger(currentSequence) || currentSequence === initialSequence) {
        throw new Error(`${chrome.label} logical input did not advance for chord alignment`);
      }
      // A CDP command has no multi-key batch form. Enqueue the requested
      // commands in their declared order without awaiting either round trip.
      // WebSocket command order keeps F3 ahead of Z, while eliminating the
      // 10-20 ms response gap that can let Macroquad sample F3 alone. Await
      // all acknowledgements only after both genuine DevTools edges are on
      // the wire.
      const events = keys.map(keyEvent);
      const keyDownRequests = [];
      for (const event of events) {
        keyDownRequests.push(dispatchChromeKey(send, "keyDown", event));
      }
      await Promise.all(keyDownRequests);
      for (const event of events) {
        await recordInputTrace(send, chrome.label, "keyDown", event);
      }
      await sleep(holdMs);
      const releaseEvents = [...events].reverse();
      await Promise.all(releaseEvents.map((event) =>
        dispatchChromeKey(send, "keyUp", event).catch(() => {})
      ));
      for (const event of releaseEvents) {
        await recordInputTrace(send, chrome.label, "keyUp", event);
      }
    },
    async chord(modifier, key, modifierHoldMs = 35) {
      await focusCanvas(send);
      const modifierEvent = keyEvent(modifier);
      const keyPressEvent = keyEvent(key);
      await dispatchChromeKey(send, "keyDown", modifierEvent);
      await recordInputTrace(send, chrome.label, "keyDown", modifierEvent);
      await dispatchChromeKey(send, "keyDown", keyPressEvent);
      await recordInputTrace(send, chrome.label, "keyDown", keyPressEvent);
      await dispatchChromeKey(send, "keyUp", keyPressEvent).catch(() => {});
      await recordInputTrace(send, chrome.label, "keyUp", keyPressEvent);
      if (modifierHoldMs > 0) await sleep(modifierHoldMs);
      await dispatchChromeKey(send, "keyUp", modifierEvent).catch(() => {});
      await recordInputTrace(send, chrome.label, "keyUp", modifierEvent);
    },
    async holdUntil(key, description, predicate, timeoutMs = 30_000) {
      await focusCanvas(send);
      const event = keyEvent(key);
      await dispatchChromeKey(send, "keyDown", event);
      await recordInputTrace(send, chrome.label, "keyDown", event);
      try {
        // Long movement polls must not clone the bounded 6000-entry packet
        // history every 100 ms. That multi-megabyte CDP serialization stalls
        // the very browser cadence this replay is measuring.
        await waitUntil(description, async () => predicate(await this.metricsState()), timeoutMs);
      } finally {
        await dispatchChromeKey(send, "keyUp", event).catch(() => {});
        await recordInputTrace(send, chrome.label, "keyUp", event);
      }
      return this.pageState();
    },
    async enterShip() {
      // Gameplay is intentionally admitted once the authoritative map is
      // published. Optional LVZ/audio downloads may continue behind reliable
      // traffic (and can take materially longer under the seeded S2C loss
      // gate), so requiring every zone file here tests asset completion rather
      // than the ship-selection authority boundary.
      await waitUntil(`${chrome.label} zone map ready for ship selection`, async () => {
        // Readiness is now readable on the shipped configuration. The game publishes a change-only
        // lifecycle channel that is not gated behind trace or metrics, because before it there was no
        // gameplay state at all on an unflagged page and a driver sitting on this wait could only
        // discover readiness by timing out. The full uiMetrics block stays opt-in.
        const state = await this.metricsState();
        if (state.lifecycle?.zoneMapReady === true) return true;
        return (state.uiMetrics?.localGameplayState || {}).zoneMapReady === true;
      }, 30_000);
      await waitUntil(`${chrome.label} ordinary initial ship join`, async () => {
        // This helper observes assignment, not controls readiness. Baseline
        // fault scenarios must be able to reach their deliberately unready
        // window. Normal scenarios separately wait for gameplay readiness.
        const state = await this.metricsState();
        if (state.lifecycle?.arenaAssigned === true) return true;
        const metrics = state.uiMetrics || {};
        // Joining already requests the selected/default ship. Esc/2 while
        // admission is pending can open a menu whose number is discarded by
        // the unready-frame policy, then swallow a later gameplay test edge.
        // Observe the normal join without changing its ship or keyboard layer.
        return metrics.playerLifeSeq != null && metrics.serverPlayerEnergy != null;
      }, 60_000);
    },
    async firePrimary() {
      await this.press("Control", 30);
      await sleep(250);
    },
    async fireBomb() {
      const before = await this.combatState();
      const beforeBombs = before.uiMetrics?.localBombCount ?? 0;
      await this.press("Tab", 250);
      await sleep(120);
      const afterTab = await this.combatState();
      const tabCreatedBomb =
        (afterTab.uiMetrics?.localBombCount ?? 0) > beforeBombs ||
        (afterTab.uiMetrics?.bombCooldown ?? 0) > 0;
      if (!tabCreatedBomb) {
        await this.press("z", 250);
      }
      await sleep(400);
    },
    async driveForTrace() {
      // This short genuine-key probe needs only current poses. Exporting
      // all diagnostic rings here can consume the input recovery window and
      // manufacture a correction immediately before ordinary movement.
      const attempts = [];
      for (let attempt = 0; attempt < 2; attempt += 1) {
        const before = await this.metricsState();
        await this.hold("ArrowUp", 220);
        await sleep(250);
        const after = await this.metricsState();
        const dx = (after.uiMetrics?.playerX ?? 0) - (before.uiMetrics?.playerX ?? 0);
        const dy = (after.uiMetrics?.playerY ?? 0) - (before.uiMetrics?.playerY ?? 0);
        const moved = Math.hypot(dx, dy);
        attempts.push({ before: before.uiMetrics, after: after.uiMetrics, moved });
        if (moved > 0.5 ||
            Math.hypot(after.uiMetrics?.playerVelocityX ?? 0,
              after.uiMetrics?.playerVelocityY ?? 0) > 0.5) {
          return { before: before.uiMetrics, after: after.uiMetrics, moved, attempts: attempt + 1 };
        }
        // A lifecycle boundary intentionally latches any key that was held
        // across respawn. `hold` has now published a real key-up; retry once
        // with a fresh key edge and still require physical movement.
        await sleep(120);
      }
      throw new Error(`${chrome.label} keyboard movement did not affect local ship: ${JSON.stringify({ attempts })}`);
    },
    async driveRemoteSmoothness(watcher, label, lossRecovery = false) {
      const [watcherBefore, actorBefore] = await Promise.all([
        watcher.pageState(),
        lossRecovery ? this.metricsState() : Promise.resolve(null),
      ]);
      await waitForCombatViewWeaponReady(
        watcher,
        `${label} watcher after retained-state capture`,
      );
      const traceStart = { at: Date.now(), index: watcherBefore.metricsTrace?.length ?? 0 };
      const watchedRemote = lossRecovery
        ? (watcherBefore.uiMetrics?.remotePlayers || []).find(
            (player) => player.id === actorBefore?.uiMetrics?.selfPlayerId,
          )
        : null;
      const watchedActor = watchedRemote
        ? { playerId: watchedRemote.id, lifeSeq: watchedRemote.lifeSeq }
        : null;
      await focusCanvas(send);
      const up = keyEvent("ArrowUp");
      const left = keyEvent("ArrowLeft");
      await dispatchChromeKey(send, "keyDown", up);
      await recordInputTrace(send, chrome.label, "keyDown", up);
      await sleep(160);
      await dispatchChromeKey(send, "keyDown", left);
      await recordInputTrace(send, chrome.label, "keyDown", left);
      await sleep(900);
      await dispatchChromeKey(send, "keyUp", left).catch(() => {});
      await recordInputTrace(send, chrome.label, "keyUp", left);
      await dispatchChromeKey(send, "keyUp", up).catch(() => {});
      await recordInputTrace(send, chrome.label, "keyUp", up);
      await sleep(350);
      const watcherAfter = await watcher.pageState();
      const assertion = lossRecovery
        ? assertRemoteMotionRecoversUnderLoss
        : assertRemoteMotionSmooth;
      return assertion(`${label} remote motion`, watcherAfter, traceStart, watchedActor);
    },
    async driveRemoteRotationSmoothness(watcher, label, lossRecovery = false) {
      await waitUntil(`${chrome.label} rotation controls recover from combat EMP`, async () => {
        const metrics = (await this.metricsState()).uiMetrics || {};
        return (
          metrics.playerAlive === true &&
          Number.isFinite(metrics.localGameplayState?.engineShutdownTimer) &&
          metrics.localGameplayState.engineShutdownTimer <= 0 &&
          metrics.localGameplayState.doorClockFresh === true &&
          metrics.localGameplayState.pendingShipResetDeliveryId == null &&
          metrics.localGameplayState.pendingFrequencyBaselineDeliveryId == null
        );
      }, 10_000);
      const [watcherBefore, actorBefore] = await Promise.all([
        watcher.pageState(),
        this.metricsState(),
      ]);
      await waitForCombatViewWeaponReady(
        watcher,
        `${label} watcher after retained-state capture`,
      );
      const traceStart = { at: Date.now(), index: watcherBefore.metricsTrace?.length ?? 0 };
      const watchedRemote = (watcherBefore.uiMetrics?.remotePlayers || []).find(
        (player) => player.id === actorBefore?.uiMetrics?.selfPlayerId,
      );
      const watchedActor = watchedRemote
        ? { playerId: watchedRemote.id, lifeSeq: watchedRemote.lifeSeq }
        : null;
      if (
        !watchedActor ||
        !Number.isFinite(watchedRemote.displayOrientation) ||
        !Number.isInteger(watcherBefore.uiMetrics?.authoritativeSnapshotId)
      ) {
        throw new Error(`${label} rotation proof is missing its exact starting actor: ${JSON.stringify({
          actor: actorBefore.uiMetrics,
          watcher: watcherBefore.uiMetrics,
        })}`);
      }
      await focusCanvas(send);
      const left = keyEvent("ArrowLeft");
      let priorSnapshotId = watcherBefore.uiMetrics.authoritativeSnapshotId;
      let priorOrientation = watchedRemote.displayOrientation;
      let observedRotation = 0;
      for (let pulse = 0; pulse < 24 && observedRotation < 0.08; pulse += 1) {
        // Short same-direction pulses cannot alias a full Chaos-ship
        // revolution when snapshot publication is delayed. Wait for a real
        // forward actor snapshot between pulses so the proof observes the
        // authoritative path rather than merely a local key duration.
        await dispatchChromeKey(send, "keyDown", left);
        await recordInputTrace(send, chrome.label, "keyDown", left);
        await sleep(180);
        await dispatchChromeKey(send, "keyUp", left).catch(() => {});
        await recordInputTrace(send, chrome.label, "keyUp", left);
        let observed = null;
        await waitUntil(`${label} rotation pulse reaches the watcher`, async () => {
          const state = await watcher.metricsState();
          const metrics = state.uiMetrics || {};
          const actor = (metrics.remotePlayers || []).find(
            (candidate) =>
              candidate.id === watchedActor.playerId &&
              candidate.lifeSeq === watchedActor.lifeSeq,
          );
          const advance = snapshotIdForwardDistance(
            metrics.authoritativeSnapshotId,
            priorSnapshotId,
          );
          if (actor && advance != null && advance > 0 && advance <= 0x7fff) {
            observed = { actor, snapshotId: metrics.authoritativeSnapshotId };
            return true;
          }
          return false;
        }, 5_000);
        observedRotation += orientationDelta(priorOrientation, observed.actor.displayOrientation);
        priorOrientation = observed.actor.displayOrientation;
        priorSnapshotId = observed.snapshotId;
      }
      if (observedRotation < 0.08) {
        throw new Error(`${label} authoritative rotation pulses did not turn the watched actor: ${JSON.stringify({
          watchedActor,
          observedRotation,
          priorSnapshotId,
          priorOrientation,
        })}`);
      }
      // Let the remote presentation consume the authoritative tail after the
      // key release; this wait does not add more rotation input.
      await sleep(500);
      const watcherAfter = await watcher.pageState();
      const assertion = lossRecovery
        ? assertRemoteRotationRecoversUnderLoss
        : assertRemoteRotationSmooth;
      return assertion(`${label} remote rotation`, watcherAfter, traceStart, watchedActor);
    },
    async pageState() {
      return chromePageState(send);
    },
    async artifactState() {
      return chromePageState(send, {cold: true});
    },
    async prepareArtifactState() {
      return chromePageState(send, {cold: true, prepare: true});
    },
    async keyState() {
      // Keyboard hold/repeat checks need only the current UI projection and
      // bridge latches. Pulling packet, sprite, and metrics histories inside a
      // feedback loop can itself stall the debug browser being measured.
      return evaluateChrome(
        send,
        `(() => ({
          uiMetrics: window.__zubspaceUiMetrics || null,
          gameKeyBridge: window.__zubspaceGameKeyStateForTest?.() || null,
        }))()`,
      );
    },
    async cycleFocus(holdMs = 200) {
      const response = await fetch(
        `http://127.0.0.1:${chrome.port}/json/new?${encodeURIComponent("about:blank")}`,
        { method: "PUT" },
      );
      if (!response.ok) {
        throw new Error(`DevTools focus-away target returned HTTP ${response.status}`);
      }
      const away = await response.json();
      if (typeof away?.id !== "string" || away.id.length === 0) {
        throw new Error("DevTools focus-away target omitted its id");
      }
      const activate = async (targetId) => {
        const activated = await fetch(
          `http://127.0.0.1:${chrome.port}/json/activate/${encodeURIComponent(targetId)}`,
        );
        if (!activated.ok) {
          throw new Error(`DevTools target activation returned HTTP ${activated.status}`);
        }
      };
      try {
        await activate(away.id);
        await sleep(holdMs);
      } finally {
        await activate(target.id).catch(() => {});
        await send("Page.bringToFront").catch(() => {});
        await fetch(
          `http://127.0.0.1:${chrome.port}/json/close/${encodeURIComponent(away.id)}`,
        ).catch(() => {});
        await focusCanvas(send).catch(() => {});
      }
      return { originalTargetId: target.id, awayTargetId: away.id, holdMs };
    },
    async freezeQaTrace(options = {}) {
      return evaluateChromeAwait(
        send,
        `window.__zubspaceQaTraceStateForTest?.().enabled === false ? null :
          window.__zubspaceMarkQaIssue({ postTailMs: 0, download: false, upload: false, ...${JSON.stringify(options)} })`,
      );
    },
    async metricsState() {
      return chromeMetricsState(send);
    },
    async combatState() {
      return chromeCombatState(send);
    },
    async lifecycleState() {
      return chromeLifecycleState(send);
    },
    async clickAt(x, y) {
      await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
      await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, x, y });
      await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, x, y });
    },
    async screenshot() {
      const result = await send("Page.captureScreenshot", { format: "png", fromSurface: true });
      return result.data;
    },
    async close() {
      // The last page can terminate Chrome and its launcher before the caller
      // reaches stopProcess. Refresh ownership at that final live boundary;
      // readiness alone cannot account for a display created later in the run.
      rememberOwnedXDisplays(chrome.child);
      await send("Page.close").catch(() => {});
      ws.close();
    },
  };
}

const chromeInputFocusLeases = new WeakMap();

function acquireChromeInputFocusLease(send) {
  chromeInputFocusLeases.set(send, (chromeInputFocusLeases.get(send) || 0) + 1);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    const count = chromeInputFocusLeases.get(send) - 1;
    if (count === 0) chromeInputFocusLeases.delete(send);
    else chromeInputFocusLeases.set(send, count);
  };
}

async function focusCanvas(send) {
  // Real CDP key delivery does not need OS activation. Joint controllers must
  // not steal each other's window focus and trigger the game's legitimate
  // blur key release midway through another pilot's held braking input.
  if (!chromeInputFocusLeases.has(send)) await send("Page.bringToFront").catch(() => {});
  await evaluateChrome(send, "document.querySelector('canvas')?.focus(); true").catch(() => {});
}

function keyEvent(key) {
  if (key === "Space" || key === " ") return { key: " ", code: "Space", keyCode: 32 };
  if (key === "Control") return { key: "Control", code: "ControlLeft", keyCode: 17 };
  if (key === "ArrowUp") return { key: "ArrowUp", code: "ArrowUp", keyCode: 38 };
  if (key === "ArrowDown") return { key: "ArrowDown", code: "ArrowDown", keyCode: 40 };
  if (key === "ArrowLeft") return { key: "ArrowLeft", code: "ArrowLeft", keyCode: 37 };
  if (key === "ArrowRight") return { key: "ArrowRight", code: "ArrowRight", keyCode: 39 };
  if (key === "Tab") return { key: "Tab", code: "Tab", keyCode: 9 };
  if (key === "Escape") return { key: "Escape", code: "Escape", keyCode: 27 };
  if (key === "Enter") return { key: "Enter", code: "Enter", keyCode: 13 };
  if (key === "`") return { key: "`", code: "Backquote", keyCode: 192 };
  if (key === "-") return { key: "-", code: "Minus", keyCode: 189 };
  if (key === "=") return { key: "=", code: "Equal", keyCode: 187 };
  if (key === "Shift") return { key: "Shift", code: "ShiftLeft", keyCode: 16 };
  if (key === "Delete") return { key: "Delete", code: "Delete", keyCode: 46 };
  if (key === "Home") return { key: "Home", code: "Home", keyCode: 36 };
  if (key === "End") return { key: "End", code: "End", keyCode: 35 };
  if (key === "Insert") return { key: "Insert", code: "Insert", keyCode: 45 };
  if (/^F(?:2|3|4|5|6|8|9|10|11)$/.test(key)) {
    const number = Number(key.slice(1));
    return { key, code: key, keyCode: 111 + number };
  }
  if (/^[0-9]$/.test(key)) return { key, code: `Digit${key}`, keyCode: key.charCodeAt(0) };
  if (/^[a-z]$/i.test(key)) {
    const upper = key.toUpperCase();
    return { key, code: `Key${upper}`, keyCode: upper.charCodeAt(0) };
  }
  throw new Error(`unsupported key ${key}`);
}

// CDP does not infer KeyboardEvent modifier flags from earlier keyDown RPCs.
// Keep physical modifier ownership per CDP client, updating synchronously so
// back-to-back/parallel chord commands retain their declared wire order.
const chromeModifierKeys = new WeakMap();
const chromeModifierBits = new Map([
  ["AltLeft", 1], ["AltRight", 1],
  ["ControlLeft", 2], ["ControlRight", 2],
  ["MetaLeft", 4], ["MetaRight", 4],
  ["ShiftLeft", 8], ["ShiftRight", 8],
]);

async function dispatchChromeKey(send, type, event) {
  let held = chromeModifierKeys.get(send);
  if (!held) { held = new Set(); chromeModifierKeys.set(send, held); }
  if (chromeModifierBits.has(event.code)) {
    if (type === "keyUp") held.delete(event.code);
    else if (type === "keyDown") held.add(event.code);
  }
  const modifiers = [...held].reduce((bits, code) => bits | chromeModifierBits.get(code), 0);
  const params = {
    type,
    modifiers,
    key: event.key,
    code: event.code,
    windowsVirtualKeyCode: event.keyCode,
    nativeVirtualKeyCode: event.keyCode,
  };
  if (event.autoRepeat) {
    params.autoRepeat = true;
  }
  if (type === "keyDown" && event.key.length === 1) {
    params.text = event.key;
    params.unmodifiedText = event.key;
  }
  await send("Input.dispatchKeyEvent", params);
}

async function recordInputTrace(send, label, phase, event, eventAt = null) {
  const payload = JSON.stringify({
    label,
    phase,
    key: event.key,
    code: event.code,
    eventAt,
  });
  await evaluateChrome(
    send,
    `(() => {
      if (new URLSearchParams(window.location.search).get("trace") !== "1") {
        return true;
      }
      const { eventAt, ...event } = ${payload};
      const metrics = window.__zubspaceUiMetrics || {};
      const remotePlayers = Array.isArray(metrics.remotePlayers)
        ? metrics.remotePlayers.map((player) => ({
            id: player.id,
            energy: player.energy,
            serverEnergy: player.serverEnergy,
            displayX: player.displayX,
            displayY: player.displayY,
            positionX: player.positionX,
            positionY: player.positionY,
            displayOrientation: player.displayOrientation,
          }))
        : [];
      window.__zubspaceInputTrace ||= [];
      window.__zubspaceInputTrace.push({
        at: eventAt ?? Date.now(),
        ...(eventAt == null ? {} : { sampledAt: Date.now() }),
        ...event,
        selfPlayerId: metrics.selfPlayerId,
        clientInputSeq: metrics.clientInputSeq,
        latestServerTick: metrics.latestServerTick,
        latestSnapshotId: metrics.latestSnapshotId,
        authoritativeSnapshotId: metrics.authoritativeSnapshotId,
        authoritativeSnapshotActors: metrics.authoritativeSnapshotActors || [],
        authoritativeSnapshotActorTrace: metrics.authoritativeSnapshotActorTrace || [],
        authoritativeSnapshotActorStream: metrics.authoritativeSnapshotActorStream || null,
        remoteActorPresentation: metrics.remoteActorPresentation || [],
        latestSnapshotServerTick: metrics.latestSnapshotServerTick,
        lastAckedSnapshotId: metrics.lastAckedSnapshotId,
        lastAckedSnapshotServerTick: metrics.lastAckedSnapshotServerTick,
        pendingWeapon: metrics.pendingWeapon,
        playerEnergy: metrics.playerEnergy,
        playerPredictedEnergy: metrics.playerPredictedEnergy,
        localShipLabelEnergy: metrics.localShipLabelEnergy,
        serverPlayerEnergy: metrics.serverPlayerEnergy,
        playerAlive: metrics.playerAlive,
        serverPlayerAlive: metrics.serverPlayerAlive,
        playerRespawnTimer: metrics.playerRespawnTimer,
        playerRespawnSeq: metrics.playerRespawnSeq,
        playerX: metrics.playerX,
        playerY: metrics.playerY,
        playerVelocityX: metrics.playerVelocityX,
        playerVelocityY: metrics.playerVelocityY,
        playerOrientation: metrics.playerOrientation,
        remoteVisiblePlayers: metrics.remoteVisiblePlayers,
        remotePlayers,
        lastNetworkDamageServerSeq: metrics.lastNetworkDamageServerSeq,
        lastNetworkDamageServerTick: metrics.lastNetworkDamageServerTick,
        lastSelfDamageServerSeq: metrics.lastSelfDamageServerSeq,
        lastSelfDamageServerTick: metrics.lastSelfDamageServerTick,
      });
      if (window.__zubspaceInputTrace.length > 500) {
        window.__zubspaceInputTrace.splice(0, window.__zubspaceInputTrace.length - 500);
      }
      return true;
    })()`,
  ).catch(() => {});
}

async function evaluateChrome(send, expression) {
  const result = await send("Runtime.evaluate", {
    returnByValue: true,
    expression,
  });
  return result.result.value;
}

async function evaluateChromeAwait(send, expression) {
  const result = await send("Runtime.evaluate", {
    returnByValue: true,
    awaitPromise: true,
    expression,
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.text || "browser evaluation failed");
  }
  return result.result.value;
}

async function chromePageState(send, {cold = false, prepare = false} = {}) {
  if (prepare && !cold) throw new Error("only cold artifact state may be prepared");
  // Returning the bounded packet/metrics traces can serialize many megabytes
  // through CDP. Mark that explicitly test-owned pause so the local snapshot
  // cadence audit does not mistake its own full-state capture for an RTC or
  // application delivery stall. The raw window remains in every artifact and
  // only gaps which cross a completed capture are excluded.
  const driverReadWindow = { startedAt: Date.now(), endedAt: null };
  instrumentationReadWindows.push(driverReadWindow);
  if (instrumentationReadWindows.length > 2048) instrumentationReadWindows.shift();
  const instrumentationReadStartedAt = await evaluateChrome(
    send,
    `(() => {
      const startedAt = ${JSON.stringify(driverReadWindow.startedAt)};
      window.__zubspaceInstrumentationReadWindows ||= [];
      window.__zubspaceInstrumentationReadWindows.push({ startedAt, endedAt: null });
      if (window.__zubspaceInstrumentationReadWindows.length > 1024) {
        window.__zubspaceInstrumentationReadWindows.shift();
      }
      return startedAt;
    })()`,
  );
  let endPromise = null;
  const endReadWindow = () => endPromise ||= evaluateChrome(send, `(() => {
    const entry = (window.__zubspaceInstrumentationReadWindows || [])
      .findLast(entry => entry.startedAt === ${JSON.stringify(instrumentationReadStartedAt)});
    const endedAt = Date.now();
    if (entry) entry.endedAt = endedAt;
    return endedAt;
  })()`);
  let transfer;
  try {
    transfer = await (cold ? prepareCdpJson : async (send, expression) => ({
      read: () => evaluateChrome(send, expression), release: async () => {},
    }))(
      send,
      `(() => {
      const rawUiMetrics = window.__zubspaceUiMetrics || null;
      const actorTrace = Array.isArray(rawUiMetrics?.authoritativeSnapshotActorTrace)
        ? rawUiMetrics.authoritativeSnapshotActorTrace.slice(-1)
        : [];
      const rawActorStream = rawUiMetrics?.authoritativeSnapshotActorStream;
      const uiMetrics = rawUiMetrics ? {
        ...rawUiMetrics,
        authoritativeSnapshotActorTrace: actorTrace,
        authoritativeSnapshotActorStream: rawActorStream && typeof rawActorStream === "object"
          ? { ...rawActorStream, frames: actorTrace }
          : rawActorStream,
      } : null;
      return ({
      networkLog: window.__zubspaceNetworkLog || [],
      networkStats: window.__zubspaceNetworkStats || {},
      packetLogFirst: window.__zubspacePacketLogFirst || [],
      packetLog: window.__zubspacePacketLog || [],
      uiMetrics,
      metricsTrace: window.__zubspaceMetricsTrace || [],
      remoteSpriteTrace: window.__zubspaceRemoteSpriteTrace || [],
      remoteLifeTrace: window.__zubspaceRemoteLifeTrace || [],
      audioUnlock: window.__zubspaceAudioUnlockState || null,
      gameKeyBridge: window.__zubspaceGameKeyStateForTest?.() || null,
      botAudioProbe: window.__zubspaceBotAudioProbe || null,
      prizeStateTrace: window.__zubspacePrizeStateTrace || [],
      metricsPublishCount: window.__zubspaceMetricsPublishCount || 0,
      damageTrace: window.__zubspaceDamageTrace || [],
      damagePresentationTrace: window.__zubspaceDamagePresentationTrace || [],
      deathTrace: window.__zubspaceDeathTrace || [],
      inputTrace: window.__zubspaceInputTrace || [],
      weaponAdmissionTrace: window.__zubspaceWeaponAdmissionTrace || [],
      weaponAdmissionTraceTotal: window.__zubspaceWeaponAdmissionTraceTotal || 0,
      weaponAdmissionTraceDropped: window.__zubspaceWeaponAdmissionTraceDropped || 0,
      instrumentationReadWindows: window.__zubspaceInstrumentationReadWindows || [],
      qaTraceState: window.__zubspaceQaTraceStateForTest?.() || null,
      qaTraceBundle: window.__zubspaceLastQaTraceBundle || null,
      currentUrl: window.location.href,
      });
      })()`,
    );
  } catch (error) {
    try { await endReadWindow(); }
    catch (cleanupError) { throw new AggregateError([error, cleanupError],
      `state preparation failed: ${error.message}; read-window cleanup failed: ${cleanupError.message}`); }
    finally { driverReadWindow.endedAt = Date.now(); }
    throw error;
  }
  const finish = async () => {
    let state, readFailure = null, instrumentationReadEndedAt;
    try {
      state = await transfer.read();
    } catch (error) { readFailure = error; throw error; }
    finally {
      try { instrumentationReadEndedAt = await endReadWindow(); }
      catch (cleanupError) {
        if (readFailure) throw new AggregateError([readFailure, cleanupError],
          `state read failed: ${readFailure.message}; read-window cleanup failed: ${cleanupError.message}`);
        throw cleanupError;
      } finally { driverReadWindow.endedAt = Date.now(); }
    }
    const capturedWindow = (state.instrumentationReadWindows || [])
      .findLast((entry) => entry.startedAt === instrumentationReadStartedAt);
    if (capturedWindow) capturedWindow.endedAt = instrumentationReadEndedAt;
    // Include read windows from every browser sharing the driver/host.
    state.instrumentationReadWindows = instrumentationReadWindows.map((entry) => ({ ...entry }));
    return state;
  };
  if (prepare) return {read: finish, release: async () => {
    let releaseFailure = null;
    try { await transfer.release(); }
    catch (error) { releaseFailure = error; }
    try { await endReadWindow(); }
    catch (windowError) {
      if (releaseFailure) throw new AggregateError([releaseFailure, windowError],
        `snapshot release failed: ${releaseFailure.message}; read-window cleanup failed: ${windowError.message}`);
      throw windowError;
    } finally { driverReadWindow.endedAt ??= Date.now(); }
    if (releaseFailure) throw releaseFailure;
  }};
  return finish();
}

async function chromeMetricsState(send) {
  // Round34's authoritative membership stream makes even the otherwise-small
  // metrics poll serialize a bounded actor history. Mark these test-driver
  // reads too; repeated waitUntil polling must not become evidence of a
  // transport cadence stall caused by the test itself.
  const driverReadWindow = { startedAt: Date.now(), endedAt: null };
  instrumentationReadWindows.push(driverReadWindow);
  if (instrumentationReadWindows.length > 2048) instrumentationReadWindows.shift();
  try {
    return await evaluateChrome(
      send,
      `(() => {
        const rawUiMetrics = window.__zubspaceUiMetrics || null;
        const actorTrace = Array.isArray(rawUiMetrics?.authoritativeSnapshotActorTrace)
          ? rawUiMetrics.authoritativeSnapshotActorTrace.slice(-1)
          : [];
        const rawActorStream = rawUiMetrics?.authoritativeSnapshotActorStream;
        return {
          uiMetrics: rawUiMetrics ? {
          ...rawUiMetrics,
          authoritativeSnapshotActorTrace: actorTrace,
          authoritativeSnapshotActorStream: rawActorStream && typeof rawActorStream === "object"
            ? { ...rawActorStream, frames: actorTrace }
            : rawActorStream,
          } : null,
          networkStats: window.__zubspaceNetworkStats || {},
          lifecycle: window.__zubspaceLifecycle || null,
          damageTrace: (window.__zubspaceDamageTrace || []).slice(-256),
          damagePresentationTrace:
            (window.__zubspaceDamagePresentationTrace || []).slice(-64),
          deathTrace: (window.__zubspaceDeathTrace || []).slice(-64),
        };
      })()`,
    );
  } finally {
    driverReadWindow.endedAt = Date.now();
  }
}

async function chromeCombatState(send) {
  // Combat polling deliberately copies only the live authority boundary and
  // bounded event tails. Even the normal metrics payload contains layout,
  // roster, lifecycle-ledger, and zone state that can pause debug WASM long
  // enough to manufacture a presentation hold when sampled every attempt.
  return evaluateChrome(
    send,
    `(() => {
      const metrics = window.__zubspaceUiMetrics || {};
      return {
        uiMetrics: {
          selfPlayerId: metrics.selfPlayerId,
          playerLifeSeq: metrics.playerLifeSeq,
          clientInputSeq: metrics.clientInputSeq,
          selfWins: metrics.selfWins,
          selfLosses: metrics.selfLosses,
          playerRespawnSeq: metrics.playerRespawnSeq,
          playerRespawnTimer: metrics.playerRespawnTimer,
          playerEnergy: metrics.playerEnergy,
          playerPredictedEnergy: metrics.playerPredictedEnergy,
          playerMaxEnergy: metrics.playerMaxEnergy,
          serverPlayerEnergy: metrics.serverPlayerEnergy,
          playerAlive: metrics.playerAlive,
          serverPlayerAlive: metrics.serverPlayerAlive,
          playerX: metrics.playerX,
          playerY: metrics.playerY,
          playerVelocityX: metrics.playerVelocityX,
          playerVelocityY: metrics.playerVelocityY,
          playerOrientation: metrics.playerOrientation,
          remoteVisiblePlayers: metrics.remoteVisiblePlayers,
          remotePositionX: metrics.remotePositionX,
          remotePositionY: metrics.remotePositionY,
          remoteEnergy: metrics.remoteEnergy,
          remoteBombSpawns: metrics.remoteBombSpawns,
          localBombCount: metrics.localBombCount,
          bombCooldown: metrics.bombCooldown,
          pendingWeapon: metrics.pendingWeapon,
          lastNetworkDamageServerSeq: metrics.lastNetworkDamageServerSeq,
          lastNetworkDamageServerTick: metrics.lastNetworkDamageServerTick,
          lastNetworkDamageEnergy: metrics.lastNetworkDamageEnergy,
          lastNetworkDamageAmount: metrics.lastNetworkDamageAmount,
          lastNetworkDamageWeapon: metrics.lastNetworkDamageWeapon,
          combatViewTimeline: metrics.combatViewTimeline || null,
        },
        damageTrace: (window.__zubspaceDamageTrace || []).slice(-256),
        deathTrace: (window.__zubspaceDeathTrace || []).slice(-64),
      };
    })()`,
  );
}

async function chromeLifecycleState(send) {
  // Death assertions often begin after the damage delivery proof has
  // completed. Preserve a compact view of the recent lifecycle samples so a
  // genuine two-second respawn cannot disappear between those two checks.
  // Mapping in the page keeps the large actor/presentation fields out of the
  // DevTools response and avoids stalling debug WASM.
  return evaluateChrome(
    send,
    `(() => {
      const metrics = window.__zubspaceUiMetrics || {};
      const compact = (sample) => ({
        at: sample.at,
        selfPlayerId: sample.selfPlayerId,
        selfWins: sample.selfWins,
        selfLosses: sample.selfLosses,
        playerRespawnSeq: sample.playerRespawnSeq,
        playerRespawnTimer: sample.playerRespawnTimer,
        playerEnergy: sample.playerEnergy,
        playerMaxEnergy: sample.playerMaxEnergy,
        playerAlive: sample.playerAlive,
        serverPlayerAlive: sample.serverPlayerAlive,
        playerX: sample.playerX,
        playerY: sample.playerY,
        playerVelocityX: sample.playerVelocityX,
        playerVelocityY: sample.playerVelocityY,
        remoteProjectileSpawns: sample.remoteProjectileSpawns,
        remoteBulletSpawns: sample.remoteBulletSpawns,
        remoteBombSpawns: sample.remoteBombSpawns,
        authorityPhase: sample.localGameplayState?.authorityPhase,
        authorityInputBits: sample.localGameplayState?.inputBits,
        remoteVisiblePlayers: sample.remoteVisiblePlayers,
        remotePlayers: Array.isArray(sample.remotePlayers)
          ? sample.remotePlayers.map((actor) => ({
              id: actor.id,
              lifeSeq: actor.lifeSeq,
              displayX: actor.displayX,
              displayY: actor.displayY,
            }))
          : [],
      });
      return {
        uiMetrics: compact(metrics),
        lifecycleTrace: (window.__zubspaceMetricsTrace || [])
          .slice(-384)
          .map(compact),
        deathTrace: (window.__zubspaceDeathTrace || []).slice(-64),
      };
    })()`,
  );
}

// A DevTools command that times out is not a slow assertion; it means the page's main thread never
// reached the command inside the command budget. A readiness timeout that hides that behind a deadline
// reads like a gameplay regression, so every timed-out command is recorded per page and reported in the
// failure message. The long-task witness is read only after the page answers again, because reading it
// while the page is blocked would just add another five-second timeout to the failure path.
// The frame-gap sampler rides along only on the wormhole-authority lane, which is the one that
// performs a genuine-key arena relocation and therefore the one that can straddle an arena switch.
// Every other lane keeps the task-only witness, because an extra animation callback on a page whose
// input latency is under test would be part of what that lane measures. The starvation probe rides
// along on the same lane for the same reason: it is the only main-thread attribution Firefox offers,
// and it costs a live task loop on the page.
const LONGTASK_OBSERVE = longTaskWitnessInstallScript({
  frameSampler: wormholeAuthorityGate,
  starvationProbe: wormholeAuthorityGate,
});
const LONGTASK_READ = longTaskWitnessTopScript(4);

const starvationLedger = new Map();
function starvationState(label) {
  let state = starvationLedger.get(label);
  if (!state) {
    state = { timedOut: 0, lastMethod: "none", starvedSince: 0, lastSuccessAt: Date.now(), longTasks: null };
    starvationLedger.set(label, state);
  }
  return state;
}

export function formatStarvationNote(
  now = Date.now(),
  entries = [...starvationLedger.entries()],
) {
  const starved = entries.filter(([, state]) => state.timedOut > 0);
  if (starved.length === 0) {
    return "";
  }
  const pages = starved.map(([label, state]) => {
    // The stall length is the load-bearing number and never depends on the witness being readable.
    const unresponsive = state.starvedSince > 0 ? now - state.starvedSince : 0;
    // Long tasks are reported by Chrome only for the page's own work; a script the driver ran synchronously
    // is not attributed, so an empty witness means "no organic long task around it", not "nothing happened".
    const witness = state.longTasks && state.longTasks.length > 0
      ? `, worst organic long task ${state.longTasks[0].duration}ms at t=${state.longTasks[0].start}ms`
      : "";
    return `${label}: ${state.timedOut} timed out DevTools command(s) (last ${state.lastMethod}), `
      + `unresponsive ${unresponsive}ms${witness}`;
  });
  return `; page main-thread starvation -> ${pages.join("; ")}`;
}

export function resetStarvationLedger() {
  starvationLedger.clear();
}

export function starvationLedgerSnapshot() {
  return Object.fromEntries(
    [...starvationLedger.entries()].map(([label, state]) => [label, { ...state }]),
  );
}

async function waitUntil(description, predicate, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      if (await predicate()) return;
    } catch (error) {
      lastError = error;
    }
    await sleep(100);
  }
  const suffix = lastError ? `; last error: ${lastError.message}` : "";
  // These budgets are wall-clock. A machine that saturated mid-run produces this
  // failure on code that is fine, so name the load instead of leaving the run to
  // read as a gameplay regression.
  throw new Error(
    `timed out waiting for ${description}${suffix}${formatStarvationNote()}${loadMeasurementNote()}`,
  );
}

export async function waitForCombatViewWeaponReady(client, label, timeoutMs = 10_000) {
  await waitUntil(`${label} combat view ready`, async () => {
    const state = await client.metricsState();
    return state.uiMetrics?.combatViewTimeline?.weaponReady === true;
  }, timeoutMs);
}

function mergedPacketLog(state) {
  const bySequence = new Map();
  for (const entry of [...(state.packetLogFirst || []), ...(state.packetLog || [])]) {
    const key = entry.seq ?? `${entry.direction}:${entry.at}:${JSON.stringify(entry.bytes || [])}`;
    bySequence.set(key, entry);
  }
  return [...bySequence.values()].sort((left, right) => (left.seq || 0) - (right.seq || 0));
}

function snapshotIdForwardDistance(candidate, previous) {
  if (!Number.isInteger(candidate) || !Number.isInteger(previous)) return null;
  if (candidate === previous) return 0;
  if (candidate === 0 || previous === 0) return null;
  return candidate > previous
    ? candidate - previous
    : (0xffff - previous) + candidate;
}

function snapshotIdIsSameOrForward(candidate, previous) {
  const distance = snapshotIdForwardDistance(candidate, previous);
  return distance === 0 || (distance != null && distance <= 0x7fff);
}

function snapshotPacketLogWindows(state, startedAt) {
  const first = [...(state.packetLogFirst || [])]
    .filter((entry) => (entry.at ?? 0) >= startedAt)
    .sort((left, right) => (left.seq || 0) - (right.seq || 0));
  const latest = [...(state.packetLog || [])]
    .filter((entry) => (entry.at ?? 0) >= startedAt)
    .sort((left, right) => (left.seq || 0) - (right.seq || 0));
  if (first.length === 0) return latest.length === 0 ? [] : [latest];
  if (latest.length === 0) return [first];
  if ((latest[0].seq ?? 0) <= (first.at(-1).seq ?? 0) + 1) {
    const merged = new Map([...first, ...latest].map((entry) => [entry.seq, entry]));
    return [[...merged.values()].sort((left, right) => (left.seq || 0) - (right.seq || 0))];
  }
  return [first, latest];
}

function receivedSnapshotDiscontinuities(state, startedAt) {
  const gaps = [];
  let receivedSnapshots = 0;
  for (const entries of snapshotPacketLogWindows(state, startedAt)) {
    let previousId = null;
    for (const entry of entries) {
      if (
        entry.direction !== "received" ||
        !Array.isArray(entry.bytes) ||
        entry.bytes.length < 13 ||
        entry.bytes[0] !== 0x70
      ) {
        continue;
      }
      const snapshotId = entry.bytes[2] | (entry.bytes[3] << 8);
      receivedSnapshots += 1;
      if (previousId == null) {
        previousId = snapshotId;
        continue;
      }
      const distance = snapshotIdForwardDistance(snapshotId, previousId);
      if (distance == null || distance === 0 || distance > 0x7fff) {
        continue;
      }
      if (distance > 1) {
        gaps.push({
          at: entry.at ?? null,
          fromSnapshotId: previousId,
          toSnapshotId: snapshotId,
          missingSnapshotIds: distance - 1,
        });
      }
      previousId = snapshotId;
    }
  }
  return {
    receivedSnapshots,
    gapCount: gaps.length,
    missingSnapshotIds: gaps.reduce((total, gap) => total + gap.missingSnapshotIds, 0),
    firstGap: gaps[0] || null,
    lastGap: gaps.at(-1) || null,
  };
}

function simulatedLossSummary(server) {
  let packetsDropped = 0;
  let bytesDropped = 0;
  let summaryCount = 0;
  for (const rawLine of server.logs.join("").split("\n")) {
    const line = rawLine.replace(/\x1b\[[0-9;]*m/g, "");
    if (!line.includes("server simulated tx-loss summary")) continue;
    const packets = line.match(/packets_dropped[=:]\s*(\d+)/);
    const bytes = line.match(/bytes_dropped[=:]\s*(\d+)/);
    if (!packets) continue;
    summaryCount += 1;
    packetsDropped += Number(packets[1]);
    bytesDropped += Number(bytes?.[1] || 0);
  }
  return { summaryCount, packetsDropped, bytesDropped };
}

function serverLogSince(server, offset = 0) {
  return server.logs.join("").slice(offset).replace(/\x1b\[[0-9;]*m/g, "");
}

function parseNumericField(line, name) {
  const value = serverLogField(line, name);
  if (value !== undefined) return typeof value === "number" && Number.isFinite(value) ? value : null;
  line = serverLogMessage(line);
  const match = line.match(new RegExp(`\\b${name}=([-+0-9.eE]+)`));
  return match ? Number(match[1]) : null;
}

export function parseCanonicalDoorRelocations(log) {
  if (typeof log !== "string") {
    throw new Error("canonical door relocation log must be a string");
  }
  const relocations = [];
  for (const rawLine of log.split("\n")) {
    const line = rawLine.replace(/\x1b\[[0-9;]*m/g, "");
    if (!line.includes("canonical door-close relocation observed")) continue;
    const relocation = {
      playerId: parseNumericField(line, "player_id"),
      lifeSeq: parseNumericField(line, "life_seq"),
      clientSequence: parseNumericField(line, "client_sequence"),
      serverTick: parseNumericField(line, "server_tick"),
      x: parseNumericField(line, "x"),
      y: parseNumericField(line, "y"),
    };
    if (
      !Number.isInteger(relocation.playerId) ||
      relocation.playerId < 0 || relocation.playerId > 0xffff ||
      !Number.isInteger(relocation.lifeSeq) ||
      relocation.lifeSeq < 1 || relocation.lifeSeq > 0xffff_ffff ||
      !Number.isInteger(relocation.clientSequence) ||
      relocation.clientSequence < 0 || relocation.clientSequence > 0xffff ||
      !Number.isInteger(relocation.serverTick) ||
      relocation.serverTick < 0 || relocation.serverTick > 0xffff_ffff ||
      !Number.isFinite(relocation.x) ||
      !Number.isFinite(relocation.y)
    ) {
      throw new Error(`malformed canonical door relocation evidence: ${line}`);
    }
    relocations.push({ ...relocation, line });
  }
  return relocations;
}

export function canonicalDoorRelocationsForActor(log, watchedActor) {
  if (
    !watchedActor ||
    !Number.isInteger(watchedActor.playerId) ||
    !Number.isInteger(watchedActor.lifeSeq) ||
    watchedActor.lifeSeq < 1
  ) {
    throw new Error(`malformed canonical door watched actor: ${JSON.stringify(watchedActor)}`);
  }
  return parseCanonicalDoorRelocations(log).filter((relocation) =>
    relocation.playerId === watchedActor.playerId &&
    relocation.lifeSeq === watchedActor.lifeSeq
  );
}

function readPacketLogLeU32(bytes, offset) {
  if (!Array.isArray(bytes) || offset < 0 || offset + 4 > bytes.length) return null;
  return (
    bytes[offset] |
    (bytes[offset + 1] << 8) |
    (bytes[offset + 2] << 16) |
    (bytes[offset + 3] << 24)
  ) >>> 0;
}

function readPacketLogLeF32(bytes, offset) {
  if (!Array.isArray(bytes) || offset < 0 || offset + 4 > bytes.length) return null;
  const view = new DataView(Uint8Array.from(bytes.slice(offset, offset + 4)).buffer);
  return view.getFloat32(0, true);
}

function clientPositionWarpMarker(bytes) {
  if (!Array.isArray(bytes)) return null;
  const offset = bytes[0] === 0x00 && bytes[1] === 0x03 ? 6 : 0;
  if (bytes[offset] !== 0x03 || bytes.length < offset + 60) return null;
  let markerOffset = offset + 55;
  if (bytes[markerOffset] === 0x62) {
    const count = bytes[markerOffset + 3];
    if (!Number.isInteger(count)) return null;
    markerOffset +=
      CLIENT_INPUT_RECORD_HEADER_LEN + count * CLIENT_INPUT_RECORD_ENTRY_LEN;
  }
  if (bytes[markerOffset] !== 0x76 || markerOffset + 5 !== bytes.length) return null;
  const items = readPacketLogLeU32(bytes, offset + 28);
  if (items == null || (items >>> 16) !== 0x5a5a) return null;
  return {
    collectibleId: readPacketLogLeU32(bytes, markerOffset + 1),
    positionSequence: items & 0xffff,
  };
}

function warpRelocationAcceptance(bytes) {
  if (
    !Array.isArray(bytes) ||
    bytes.length !== 17 ||
    bytes[0] !== 0x00 ||
    bytes[1] !== 0x03 ||
    bytes[6] !== 0x77
  ) return null;
  return {
    collectibleId: readPacketLogLeU32(bytes, 7),
    lifeSeq: readPacketLogLeU32(bytes, 11),
    positionSequence: bytes[15] | (bytes[16] << 8),
  };
}

function parseBooleanField(line, name) {
  const value = serverLogField(line, name);
  if (value !== undefined) return value === true;
  line = serverLogMessage(line);
  const match = line.match(new RegExp(`\\b${name}=(true|false)`));
  return match ? match[1] === "true" : false;
}

function parseTextField(line, name) {
  const value = serverLogField(line, name);
  if (value !== undefined) return typeof value === "string" ? value : null;
  line = serverLogMessage(line);
  const match = line.match(new RegExp(`\\b${name}=(?:"([^"]*)"|([^ ]+))`));
  return match ? (match[1] ?? match[2]) : null;
}

function percentile(sorted, ratio) {
  if (sorted.length === 0) return null;
  return sorted[Math.max(0, Math.ceil(sorted.length * ratio) - 1)];
}

function simObservationSummary(server, offset = 0, playerId = null) {
  const samples = [];
  const log = serverLogSince(server, offset);
  const alignmentCounts = {
    stable: 0,
    transition: 0,
    fixture: 0,
    missing: 0,
    invalid: 0,
    unclassified: 0,
  };
  for (const line of log.split("\n")) {
    if (!line.includes("sim observation sample")) continue;
    const samplePlayerId = parseNumericField(line, "player_id");
    if (playerId != null && samplePlayerId !== playerId) continue;
    const parsedAlignment = parseTextField(line, "input_alignment");
    const alignment = Object.hasOwn(alignmentCounts, parsedAlignment)
      ? parsedAlignment
      : "unclassified";
    alignmentCounts[alignment] += 1;
    const deviation = parseNumericField(line, "deviation");
    const inputBits = parseNumericField(line, "input_bits") ?? 0;
    if (!Number.isFinite(deviation)) continue;
    samples.push({
      deviation,
      alignment,
      bounced: parseBooleanField(line, "bounced"),
      repelActive: parseBooleanField(line, "repel_active"),
      active:
        (inputBits & 0x3f) !== 0 ||
        parseBooleanField(line, "repel_active") ||
        parseBooleanField(line, "flagger_active") ||
        parseBooleanField(line, "has_attached_riders"),
    });
  }

  const summarize = (values) => {
    const sorted = values.map((sample) => sample.deviation).sort((a, b) => a - b);
    const underTwo = sorted.filter((value) => value < 2).length;
    return {
      samples: sorted.length,
      underTwoPixels: underTwo,
      underTwoRatio: sorted.length > 0 ? underTwo / sorted.length : null,
      p50Pixels: percentile(sorted, 0.5),
      p95Pixels: percentile(sorted, 0.95),
      maxPixels: sorted.at(-1) ?? null,
    };
  };

  return {
    all: summarize(samples),
    activeInput: summarize(samples.filter((sample) => sample.active)),
    stableActiveInput: summarize(
      samples.filter((sample) => sample.active && sample.alignment === "stable"),
    ),
    transitionDiagnostic: summarize(
      samples.filter((sample) => sample.alignment === "transition"),
    ),
    alignmentCounts,
    missingInputSamples: alignmentCounts.missing,
    invalidInputSamples: alignmentCounts.invalid,
    unclassifiedSamples: alignmentCounts.unclassified,
    bouncedSamples: samples.filter((sample) => sample.bounced).length,
    repelActiveSamples: samples.filter((sample) => sample.repelActive).length,
    // This diagnostic means the observer could not form a comparison at all;
    // keep it separate from an emitted sample explicitly classified missing.
    // The recovery soak reports both so deferred/unobserved input cannot be
    // mistaken for a malformed comparison that happened to score well.
    missingInputDiagnostics: log
      .split("\n")
      .filter((line) =>
        line.includes("sim observation input missing") &&
        (playerId == null || parseNumericField(line, "player_id") === playerId)
      ).length,
  };
}

function simReplayStepSummary(server, offset = 0, playerId = null) {
  const histogram = {};
  for (const line of serverLogSince(server, offset).split("\n")) {
    if (!line.includes("sim observation sample")) continue;
    if (playerId != null && parseNumericField(line, "player_id") !== playerId) continue;
    const steps = parseNumericField(line, "position_sequence_steps");
    if (!Number.isInteger(steps) || steps < 1) continue;
    histogram[steps] = (histogram[steps] || 0) + 1;
  }
  const values = Object.entries(histogram).map(([steps, samples]) => ({
    steps: Number(steps),
    samples,
  })).sort((left, right) => left.steps - right.steps);
  return {
    histogram: values,
    samples: values.reduce((total, entry) => total + entry.samples, 0),
    multiStepSamples: values
      .filter((entry) => entry.steps > 1)
      .reduce((total, entry) => total + entry.samples, 0),
    maxSteps: values.at(-1)?.steps ?? null,
  };
}

function securityRejectCount(server, offset, subsystem, reason) {
  return serverLogSince(server, offset)
    .split("\n")
    .filter((line) =>
      line.includes("security reject") &&
      parseTextField(line, "category") === subsystem &&
      parseTextField(line, "reason") === reason
    ).length;
}

function securityRejectCategorySummary(server, offset, category) {
  const reasons = {};
  for (const line of serverLogSince(server, offset).split("\n")) {
    if (!line.includes("security reject")) continue;
    if (parseTextField(line, "category") !== category) continue;
    const reason = parseTextField(line, "reason") || "<missing-reason>";
    reasons[reason] = (reasons[reason] || 0) + 1;
  }
  return {
    total: Object.values(reasons).reduce((sum, count) => sum + count, 0),
    reasons,
  };
}

function parseStructuredSecurityRejects(server, offset = 0) {
  const records = [];
  for (const line of serverLogSince(server, offset).split("\n")) {
    if (!line.includes("server security reject") || line.includes("security reject summary")) {
      continue;
    }
    records.push({
      playerId: parseNumericField(line, "player_id"),
      category: parseTextField(line, "category") || "<missing-category>",
      reason: parseTextField(line, "reason") || "<missing-reason>",
      line,
    });
  }
  return records;
}

function assertSecurityRejectAudit(server, offset = 0, whitelist = []) {
  const records = parseStructuredSecurityRejects(server, offset);
  const allowed = whitelist.map((entry) => ({
    category: entry.category,
    reason: entry.reason,
    playerId: entry.playerId ?? null,
    count: entry.count,
    matched: 0,
  }));
  const unknown = [];
  for (const record of records) {
    const match = allowed.find((entry) =>
      entry.category === record.category &&
      entry.reason === record.reason &&
      (entry.playerId == null || entry.playerId === record.playerId)
    );
    if (!match) unknown.push(record);
    else match.matched += 1;
  }
  const countMismatches = allowed.filter((entry) =>
    !Number.isInteger(entry.count) || entry.count < 0 || entry.matched !== entry.count
  );
  if (unknown.length > 0 || countMismatches.length > 0) {
    throw new Error(`fail-closed server security rejection audit failed: ${JSON.stringify({
      unknown,
      countMismatches,
      whitelist: allowed,
    })}`);
  }
  const byCategoryReason = {};
  for (const record of records) {
    const key = `${record.category}/${record.reason}`;
    byCategoryReason[key] = (byCategoryReason[key] || 0) + 1;
  }
  return {
    total: records.length,
    byCategoryReason,
    whitelist: allowed.map(({ matched, ...entry }) => ({ ...entry, matched })),
  };
}

function driverDataChannelFullSummary(server, browserLogs = []) {
  const serverLog = serverLogSince(server, 0);
  const labelById = new Map();
  for (const line of serverLog.split("\n")) {
    if (!line.includes("server rtc data channel announced")) continue;
    const id = parseNumericField(line, "channel_id");
    const label = parseTextField(line, "label");
    if (Number.isInteger(id) && label) labelById.set(id, label);
  }
  const counts = { game: 0, event: 0, reliable: 0, unknown: 0, total: 0 };
  const lines = [serverLog, ...browserLogs.map((logs) =>
    Array.isArray(logs) ? logs.join("") : String(logs ?? "")
  )].join("\n").split("\n");
  for (const line of lines) {
    const match = line.match(/Failed to send DataChannelMessage to data channel (\d+): Full/);
    if (!match) continue;
    const label = labelById.get(Number(match[1]));
    const key = label === GAME_CHANNEL_LABEL
      ? "game"
      : label === EVENT_CHANNEL_LABEL
        ? "event"
        : label === RELIABLE_CHANNEL_LABEL
          ? "reliable"
          : "unknown";
    counts[key] += 1;
    counts.total += 1;
  }
  return counts;
}

function repelDeliveryQueueSummary(status) {
  if (!status || typeof status !== "object" || Array.isArray(status)) {
    throw new Error("browser RTC status is missing Repel delivery metrics");
  }
  const summary = {
    queuePackets: status.repel_queue_packets,
    maxTargetDepth: status.repel_queue_max_target_depth,
    awaitingApplication: status.repel_queue_awaiting_application,
    awaitingReceiptAck: status.repel_queue_awaiting_receipt_ack,
    overflows: status.repel_queue_overflows,
  };
  for (const [key, value] of Object.entries(summary)) {
    if (!Number.isFinite(value) || !Number.isInteger(value) || value < 0) {
      throw new Error(`browser RTC Repel delivery metric ${key} is malformed`);
    }
  }
  return summary;
}

function movementSnapbackSummary(status) {
  const total = status?.movement_snapbacks_total;
  const awaitingApplication = status?.movement_corrections_awaiting_application;
  const awaitingReceiptAck = status?.movement_corrections_awaiting_receipt_ack;
  if (
    !Number.isFinite(total) || !Number.isInteger(total) || total < 0 ||
    !Number.isFinite(awaitingApplication) || !Number.isInteger(awaitingApplication) ||
    awaitingApplication < 0 ||
    !Number.isFinite(awaitingReceiptAck) || !Number.isInteger(awaitingReceiptAck) ||
    awaitingReceiptAck < 0
  ) {
    throw new Error("browser RTC status movement_snapbacks_total is malformed");
  }
  return { total, awaitingApplication, awaitingReceiptAck };
}

function dataChannelDrainDisposition(status) {
  const pressure = rtcPressureSummary(status);
  const repelDelivery = repelDeliveryQueueSummary(status);
  const movementSnapbacks = movementSnapbackSummary(status);
  const blockers = {
    ingressExecution: pressure.ingressExecution,
    applicationQueuePackets: pressure.applicationQueuePackets,
    outboundQueuePackets: pressure.outboundQueuePackets,
    outboundQueueBytes: pressure.outboundQueueBytes,
    realtimeEventQueueCurrent: pressure.realtimeEventQueueCurrent,
    projectilePendingHitCurrent: pressure.projectilePendingHitCurrent,
    projectilePendingHitRetainedBytes: pressure.projectilePendingHitRetainedBytes,
    projectilePendingDamageTransactions: pressure.projectilePendingDamageTransactions,
    projectilePendingDamagePayloadBytes: pressure.projectilePendingDamagePayloadBytes,
    repelDelivery,
    movementSnapbacks,
  };
  const drained = ["game", "event", "reliable"].every((channel) =>
    pressure.applicationQueuePackets[channel] === 0
  ) && pressure.outboundQueuePackets === 0 && pressure.outboundQueueBytes === 0 &&
    pressure.ingressExecution.in_flight === 0 && pressure.ingressExecution.waiting === 0 &&
    pressure.realtimeEventQueueCurrent === 0 &&
    pressure.projectilePendingHitCurrent === 0 &&
    pressure.projectilePendingHitRetainedBytes === 0 &&
    pressure.projectilePendingDamageTransactions === 0 &&
    pressure.projectilePendingDamagePayloadBytes === 0 &&
    repelDelivery.queuePackets === 0 &&
    repelDelivery.awaitingApplication === 0 &&
    repelDelivery.awaitingReceiptAck === 0 &&
    movementSnapbacks.awaitingApplication === 0 &&
    movementSnapbacks.awaitingReceiptAck === 0;
  return { drained, blockers, pressure, repelDelivery, movementSnapbacks };
}

// `rtc_ingress_execution.in_flight/waiting` is documented (docs/e2e-diagnostics.md) as disconnect
// cleanup evidence: a live client keeps a packet turn in flight roughly continuously, so asserting
// it before the client leaves measures luck, not cleanup. Every caller had been doing exactly that,
// which is what made `test-e2e-immediate-natural-prizes` fail with `in_flight: 1` and every queue
// empty while its browser was still streaming inputs. The audit therefore owns the disconnect and
// waits for the server to observe it before requiring the drain.
async function assertDataChannelDeliveryAudit({
  server,
  serverPort,
  browserLogs = [],
  disconnect,
}) {
  if (typeof disconnect !== "function") {
    throw new Error("RTC delivery audit requires a disconnect callback; its ingress-drain " +
      "requirement is disconnect cleanup evidence and cannot be asserted against a live client");
  }
  await disconnect();
  let status = null;
  await waitUntil("RTC session closes before its cleanup audit", async () => {
    const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
    if (!response.ok) throw new Error(`RTC status audit failed with HTTP ${response.status}`);
    status = await response.json();
    return status.rtc_sessions === 0;
  }, 10_000);
  let lastDrain = null;
  const drainSamples = [];
  try {
    await waitUntil("RTC application ingress queues drain", async () => {
      const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
      if (!response.ok) throw new Error(`RTC status audit failed with HTTP ${response.status}`);
      status = await response.json();
      lastDrain = dataChannelDrainDisposition(status);
      drainSamples.push({
        at: Date.now(),
        inboundPackets: lastDrain.pressure.inboundPackets,
        applicationQueuePackets: lastDrain.pressure.applicationQueuePackets,
      });
      if (drainSamples.length > 64) drainSamples.shift();
      return lastDrain.drained;
    }, 5_000);
  } catch (error) {
    throw new Error(`${error.message}: ${JSON.stringify({
      blockers: lastDrain?.blockers || { status },
      drainSamples,
    })}`);
  }
  const driverFull = driverDataChannelFullSummary(server, browserLogs);
  if (driverFull.total !== 0 || driverFull.unknown !== 0) {
    throw new Error(`RTC DataChannel delivery audit failed: ${JSON.stringify({
      driverFull,
    })}`);
  }
  const pressure = assertRtcPressureClean(status, "browser E2E final", {requireIngressDrained: true});
  const { repelDelivery, movementSnapbacks } = lastDrain;
  if (repelDelivery.overflows !== 0 || repelDelivery.queuePackets !== 0 ||
      repelDelivery.awaitingApplication !== 0 || repelDelivery.awaitingReceiptAck !== 0) {
    throw new Error(`Repel delivery queue did not drain cleanly: ${JSON.stringify(repelDelivery)}`);
  }
  if (
    movementSnapbacks.awaitingApplication !== 0 ||
    movementSnapbacks.awaitingReceiptAck !== 0
  ) {
    throw new Error(`movement correction transaction did not drain cleanly: ${JSON.stringify(movementSnapbacks)}`);
  }
  const audit = {
    driverFull,
    repelDelivery,
    movementSnapbacks,
    serverTiming: serverTimingEvidence(status),
    presentationGeometry: status.presentation_geometry,
    ...pressure,
  };
  assertBrowserDataChannelDeliveryAudit(audit);
  return audit;
}

function assertBrowserDataChannelDeliveryAudit(audit) {
  if (!audit || typeof audit !== "object" || Array.isArray(audit)) {
    throw new Error("browser RTC delivery audit must be an object");
  }
  const driverFull = audit.driverFull;
  if (!driverFull || typeof driverFull !== "object" || Array.isArray(driverFull)) {
    throw new Error("browser RTC delivery audit is missing driverFull");
  }
  const expectedFullKeys = ["event", "game", "reliable", "total", "unknown"];
  if (JSON.stringify(Object.keys(driverFull).sort()) !== JSON.stringify(expectedFullKeys)) {
    throw new Error("browser RTC delivery audit driverFull keys are malformed");
  }
  for (const key of expectedFullKeys) {
    if (!Number.isFinite(driverFull[key]) || !Number.isInteger(driverFull[key]) || driverFull[key] !== 0) {
      throw new Error(`browser RTC delivery audit driverFull.${key} must be zero`);
    }
  }
  const repelDelivery = audit.repelDelivery;
  if (!repelDelivery || typeof repelDelivery !== "object" || Array.isArray(repelDelivery) ||
      JSON.stringify(Object.keys(repelDelivery).sort()) !==
        JSON.stringify([
          "awaitingApplication",
          "awaitingReceiptAck",
          "maxTargetDepth",
          "overflows",
          "queuePackets",
        ])) {
    throw new Error("browser RTC delivery audit repelDelivery keys are malformed");
  }
  for (const [key, value] of Object.entries(repelDelivery)) {
    if (!Number.isFinite(value) || !Number.isInteger(value) || value < 0 ||
        ((key === "overflows" || key === "queuePackets" ||
          key === "awaitingApplication" || key === "awaitingReceiptAck") && value !== 0)) {
      throw new Error(`browser RTC delivery audit repelDelivery.${key} is not clean`);
    }
  }
  const movementSnapbacks = audit.movementSnapbacks;
  if (!movementSnapbacks || typeof movementSnapbacks !== "object" ||
      Array.isArray(movementSnapbacks) ||
      JSON.stringify(Object.keys(movementSnapbacks)) !==
        JSON.stringify(["total", "awaitingApplication", "awaitingReceiptAck"]) ||
      !Number.isFinite(movementSnapbacks.total) ||
      !Number.isInteger(movementSnapbacks.total) || movementSnapbacks.total < 0 ||
      !Number.isInteger(movementSnapbacks.awaitingApplication) ||
      movementSnapbacks.awaitingApplication !== 0 ||
      !Number.isInteger(movementSnapbacks.awaitingReceiptAck) ||
      movementSnapbacks.awaitingReceiptAck !== 0) {
    throw new Error("browser RTC delivery audit movementSnapbacks is malformed");
  }
  const {
    driverFull: _driverFull,
    repelDelivery: _repelDelivery,
    movementSnapbacks: _movementSnapbacks,
    serverTiming,
    presentationGeometry: _presentationGeometry,
    ...pressure
  } = audit;
  assertRtcPressureSummaryClean(pressure, "browser E2E retained result");
  serverTimingEvidence({ tick_timing: serverTiming });
  return true;
}

function positionInputWitnessRejectSummary(server, offset) {
  const reasons = [
    "missing_input_history",
    "missing_input_record",
    "direction_out_of_range",
    "input_direction_mismatch",
    "invalid_direction_history",
    "embedded_tail_sequence_mismatch",
    "embedded_tail_direction_mismatch",
  ];
  return Object.fromEntries(
    reasons.map((reason) => [reason, securityRejectCount(server, offset, "position", reason)]),
  );
}

function positionAdmissionRejectSummary(server, offset) {
  const reasons = [
    "blocked_position_center",
    "blocked_position_hull",
    "blocked_position_path",
    "impossible_acceleration",
    "impossible_velocity",
    "teleport",
  ];
  return Object.fromEntries(
    reasons.map((reason) => [reason, securityRejectCount(server, offset, "position", reason)]),
  );
}

function c2sInputStats(state) {
  const stats = state.networkStats || {};
  return {
    seen: Number(stats.c2sInputPacketsSeen || 0),
    dropped: Number(stats.c2sInputPacketsDropped || 0),
    delayed: Number(stats.c2sInputPacketsDelayed || 0),
    released: Number(stats.c2sInputPacketsReleased || 0),
    duplicated: Number(stats.c2sInputPacketsDuplicated || 0),
    expired: Number(stats.c2sInputPacketsExpired || 0),
  };
}

function subtractC2sInputStats(after, before) {
  return Object.fromEntries(
    Object.keys(after).map((key) => [key, after[key] - before[key]]),
  );
}

function c2sImpairmentPacketEvidence(state, startedAt) {
  const entries = mergedPacketLog(state).filter((entry) => (entry.at ?? 0) >= startedAt);
  const dropped = entries.filter((entry) => entry.direction === "dropped");
  const delayed = entries.filter((entry) => entry.direction === "delayed");
  const released = entries.filter((entry) =>
    entry.direction === "sent" &&
    entry.detail?.c2sInputImpairment === "delayed-release"
  );
  const duplicated = entries.filter((entry) =>
    entry.direction === "sent" &&
    entry.detail?.c2sInputImpairment === "duplicate-release"
  );
  const overtaken = delayed.filter((held) => {
    const release = released.find((entry) =>
      entry.detail?.ordinal === held.detail?.ordinal && entry.seq > held.seq
    );
    return release != null && entries.some((entry) =>
      entry.direction === "sent" &&
      entry.seq > held.seq &&
      entry.seq < release.seq &&
      entry.detail?.c2sInputImpairment !== "delayed-release"
    );
  });
  return {
    seed: dropped[0]?.detail?.seed ?? delayed[0]?.detail?.seed ?? duplicated[0]?.detail?.seed ?? null,
    dropped: dropped.length,
    delayed: delayed.length,
    released: released.length,
    duplicated: duplicated.length,
    overtaken: overtaken.length,
    droppedTailSequences: dropped.map((entry) => entry.detail?.tailSequence).slice(0, 32),
    delayedTailSequences: delayed.map((entry) => entry.detail?.tailSequence).slice(0, 32),
    duplicatedTailSequences: duplicated.map((entry) => entry.detail?.tailSequence).slice(0, 32),
  };
}

function inputHistoryOffset(bytes) {
  if (!Array.isArray(bytes)) return null;
  if (bytes[0] === 0x62) return 0;
  if (bytes[0] === 0x03 && bytes.length > 55 && bytes[55] === 0x62) return 55;
  return null;
}

function inputPulseSequences(bytes) {
  const offset = inputHistoryOffset(bytes);
  if (offset == null || bytes.length < offset + 4) return [];
  const base = bytes[offset + 1] | (bytes[offset + 2] << 8);
  const count = bytes[offset + 3];
  const recordEnd =
    offset + CLIENT_INPUT_RECORD_HEADER_LEN + count * CLIENT_INPUT_RECORD_ENTRY_LEN;
  if (count === 0 || bytes.length < recordEnd) return [];
  const sequences = [];
  for (let index = 0; index < count; index += 1) {
    const entry =
      offset + CLIENT_INPUT_RECORD_HEADER_LEN + index * CLIENT_INPUT_RECORD_ENTRY_LEN;
    const bits = bytes[entry] | (bytes[entry + 1] << 8);
    if ((bits & (1 << 5)) !== 0) sequences.push((base + index) & 0xffff);
  }
  return sequences;
}

function inputTailSequence(bytes) {
  const offset = inputHistoryOffset(bytes);
  if (offset == null || bytes.length < offset + 4) return null;
  const base = bytes[offset + 1] | (bytes[offset + 2] << 8);
  const count = bytes[offset + 3];
  const recordEnd =
    offset + CLIENT_INPUT_RECORD_HEADER_LEN + count * CLIENT_INPUT_RECORD_ENTRY_LEN;
  if (count === 0 || bytes.length < recordEnd) return null;
  return (base + count - 1) & 0xffff;
}

function targetedRocketPacketEvidence(state, startedAt, targetSequence) {
  const entries = mergedPacketLog(state).filter((entry) => (entry.at ?? 0) >= startedAt);
  const carryingTarget = entries.filter((entry) =>
    inputPulseSequences(entry.bytes).includes(targetSequence)
  );
  const delayed = carryingTarget.filter((entry) =>
    entry.direction === "delayed" && entry.detail?.rocketActivationImpairment === true
  );
  const released = carryingTarget.filter((entry) =>
    entry.direction === "sent" &&
    entry.detail?.rocketActivationImpairment === "delayed-release"
  );
  const release = released[0] || null;
  const sentBeforeRelease = carryingTarget.filter((entry) =>
    entry.direction === "sent" && (release == null || entry.seq < release.seq)
  );
  const suppressedBeforeRelease = carryingTarget.filter((entry) =>
    entry.direction === "dropped" &&
    entry.detail?.reason === "target-duplicate-before-release" &&
    (release == null || entry.seq < release.seq)
  );
  const overtaking = release == null ? [] : entries.filter((entry) =>
    entry.direction === "sent" &&
    entry.seq > (delayed[0]?.seq ?? release.seq) &&
    entry.seq < release.seq &&
    !inputPulseSequences(entry.bytes).includes(targetSequence)
  );
  return {
    delayed: delayed.length,
    released: released.length,
    suppressedBeforeRelease: suppressedBeforeRelease.length,
    sentBeforeRelease: sentBeforeRelease.length,
    overtaken: overtaking.length,
    releaseChannel: release?.detail?.channel ?? null,
    carryingChannelsBeforeRelease: [...new Set(
      sentBeforeRelease.map((entry) => entry.detail?.channel).filter(Boolean),
    )],
  };
}

function rocketThenBombChannelEvidence(state, startedAt, activationSequence) {
  const entries = mergedPacketLog(state).filter((entry) => (entry.at ?? 0) >= startedAt);
  const bombPositions = entries.filter((entry) => {
    if ((entry.at ?? 0) < startedAt || entry.direction !== "sent") return false;
    const bytes = entry.bytes || [];
    if (bytes[0] !== 0x03 || bytes.length < 22) return false;
    const weapon = bytes[20] | (bytes[21] << 8);
    return [3, 4].includes(weapon & 0x1f);
  });
  const weaponSequence = bombPositions.length > 0
    ? inputTailSequence(bombPositions[0].bytes || [])
    : null;
  const activationTailPackets = entries.filter((entry) =>
    entry.direction === "sent" &&
    inputTailSequence(entry.bytes || []) === activationSequence &&
    inputPulseSequences(entry.bytes || []).includes(activationSequence)
  );
  const lastWeaponCopyLogSeq = bombPositions.reduce(
    (latest, entry) => Math.max(latest, entry.seq),
    -1,
  );
  const firstPulseLogSeq = activationTailPackets.reduce(
    (earliest, entry) => Math.min(earliest, entry.seq),
    Number.POSITIVE_INFINITY,
  );
  return {
    copies: bombPositions.length,
    channels: [...new Set(
      bombPositions.map((entry) => entry.detail?.channel).filter(Boolean),
    )].sort(),
    weaponSequence,
    activationSequence,
    activationIsNextSequence:
      weaponSequence != null && ((weaponSequence + 1) & 0xffff) === activationSequence,
    pulseOnWeaponCopies: bombPositions.filter((entry) =>
      inputPulseSequences(entry.bytes || []).includes(activationSequence)
    ).length,
    activationTailPackets: activationTailPackets.length,
    lastWeaponCopyLogSeq,
    firstPulseLogSeq: Number.isFinite(firstPulseLogSeq) ? firstPulseLogSeq : null,
    pulseAfterWeaponCopies:
      Number.isFinite(firstPulseLogSeq) && firstPulseLogSeq > lastWeaponCopyLogSeq,
  };
}

export function parseHiddenPrizeSpawns(server, offset = 0) {
  const prizes = [];
  for (const line of serverLogSince(server, offset).split("\n")) {
    if (!line.includes("server hidden prize spawned")) continue;
    const x = parseNumericField(line, "x");
    const y = parseNumericField(line, "y");
    const collectibleId = parseNumericField(line, "collectible_id");
    const prizeId = parseNumericField(line, "prize_id");
    const expiresAtTick = parseNumericField(line, "expires_at_tick");
    if ([x, y, collectibleId, prizeId, expiresAtTick].every(Number.isFinite)) {
      prizes.push({ x, y, collectibleId, prizeId, expiresAtTick });
    }
  }
  return prizes;
}

export function parsePrizeClaims(server, offset = 0) {
  const claims = [];
  for (const line of serverLogSince(server, offset).split("\n")) {
    if (!line.includes("server prize claimed")) continue;
    const playerId = parseNumericField(line, "player_id");
    const x = parseNumericField(line, "x");
    const y = parseNumericField(line, "y");
    const collectibleId = parseNumericField(line, "collectible_id");
    const prizeId = parseNumericField(line, "prize_id");
    const movementTuningBefore = parseNumericField(line, "movement_tuning_before");
    const movementTuningAfter = parseNumericField(line, "movement_tuning_after");
    const engineShutdownUntilTick = parseNumericField(line, "engine_shutdown_until_tick");
    const authoritativeRecipientCount = parseNumericField(
      line,
      "authoritative_recipient_count",
    );
    if ([playerId, x, y, collectibleId, prizeId].every(Number.isFinite)) {
      claims.push({
        collectibleId,
        playerId,
        x,
        y,
        prizeId,
        movementTuningBefore,
        movementTuningAfter,
        engineShutdownUntilTick,
        authoritativeRecipientCount,
      });
    }
  }
  return claims;
}

function parseDamagePrizeLosses(server, offset = 0) {
  const losses = [];
  for (const line of serverLogSince(server, offset).split("\n")) {
    if (!line.includes("server damage prize lost")) continue;
    const playerId = parseNumericField(line, "player_id");
    const damageSeq = parseNumericField(line, "damage_seq");
    const prizeId = parseNumericField(line, "prize_id");
    const revision = parseNumericField(line, "revision");
    if ([playerId, damageSeq, prizeId, revision].every(Number.isFinite)) {
      losses.push({ playerId, damageSeq, prizeId, revision });
    }
  }
  return losses;
}

async function runPortalPhaseTwoGate({ actor, server, playerId, logOffset }) {
  // Chaos InitialBounty can legitimately award Portal inventory before the
  // hidden stream starts. Spend that authored state through the same real
  // keyboard/RTC path so the later natural Portal pickup has an observable
  // 0 -> 1 acquisition instead of merely clamping at Warbird PortalMax.
  const setupBefore = await actor.pageState();
  const setupPlacements = [];
  while ((await actor.metricsState()).uiMetrics?.localGameplayState?.portals > 0) {
    const beforeSetup = await actor.pageState();
    const portalsBefore = beforeSetup.uiMetrics.localGameplayState.portals;
    await actor.chord("Shift", "Insert", 55);
    await waitUntil("initial-bounty Portal setup spend converges", async () => {
      const local = (await actor.metricsState()).uiMetrics?.localGameplayState || {};
      return local.pendingPortalPlacements === 0 && local.portals === portalsBefore - 1;
    }, 5_000);
    setupPlacements.push({ portalsBefore, portalsAfter: portalsBefore - 1 });
    if (setupPlacements.length > 3) {
      throw new Error(`Chaos Warbird exceeded authored PortalMax during setup: ${JSON.stringify(setupPlacements)}`);
    }
  }

  // The revised 0 key uses any live endpoint instead of replacing it. Clear
  // the setup endpoint through genuine recall before testing fresh placement.
  if ((await actor.metricsState()).uiMetrics?.localGameplayState?.portalActive) {
    await actor.hold("Insert", 55);
    await waitUntil("setup endpoint is consumed before testing 0 placement", async () => {
      const local = (await actor.metricsState()).uiMetrics?.localGameplayState || {};
      return local.pendingPortalWarp === false && local.portalActive === false && local.authorityPhase === "Live";
    }, 6_000);
  }

  const arena = await actor.evaluate("window.__zubspaceArena");
  const loadedMap = loadRendezvousMap(arena, [
    ...((server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT) ?
      [server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT] : []),
    path.join(repoRoot, "assets/zones"),
  ]);
  let target = null;
  let prizeNavigation = null;
  await waitUntil("natural Chaos Portal prize spawn", async () => {
    const state = await actor.metricsState();
    const metrics = state.uiMetrics;
    if (!exactLiveGameplayScope(metrics)) return false;
    if (metrics.localGameplayState?.brickTilesOverflow) throw new Error("incomplete Portal pickup brick geometry");
    const origin = { x: metrics.playerX, y: metrics.playerY };
    const claimed = new Set(parsePrizeClaims(server).map(p => p.collectibleId));
    const candidates = parseHiddenPrizeSpawns(server, logOffset)
      .filter((spawn) => spawn.prizeId === 28)
      .filter(spawn => !claimed.has(spawn.collectibleId))
      .sort((left, right) =>
        Math.hypot(left.x - origin.x, left.y - origin.y) - Math.hypot(right.x - origin.x, right.y - origin.y)
      );
    for (const spawn of candidates) {
      try {
        const navigation = planNaturalPrizeNavigation(loadedMap.geometry, origin, spawn,
          metrics.latestSnapshotServerTick, metrics.localGameplayState?.brickTiles || []);
        if (navigation && navigation.requiredTtlTicks <= 1500) {
          target = spawn; prizeNavigation = navigation; return true;
        }
      } catch (error) {
        if (!/no hull-clear authored approach route|origin lacks conservative hull clearance/.test(error.message)) throw error;
      }
    }
    return false;
  }, 15_000);

  let claim = null;
  const route = await navigateAuthoredTarget(actor, target, server, {
    timeoutMs: 15_000, arrivalRadiusPx: 16, maximumDesiredSpeed: 80,
    complete: async () => {
      claim = parsePrizeClaims(server, logOffset).find((candidate) =>
        candidate.playerId === playerId &&
        candidate.collectibleId === target.collectibleId &&
        candidate.prizeId === 28
      ) || null;
      return claim;
    },
  });
  route.prizeNavigation = prizeNavigation;
  if ((!route.reached && !route.completed) || !claim) {
    throw new Error(`real-key route did not collect the natural Portal prize: ${JSON.stringify({
      target,
      route,
      claims: parsePrizeClaims(server, logOffset),
    })}`);
  }
  await waitUntil("natural Portal acquisition reaches private client inventory", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return metrics.localGameplayState?.portals === 1;
  }, 5_000);

  const before = await actor.pageState();
  const beforeLocal = before.uiMetrics?.localGameplayState || {};
  const beforeCounts = before.uiMetrics?.semanticInputCounts || {};
  const placementStartedAt = Date.now();
  const placementLogOffset = server.logs.join("").length;
  await actor.hold("0", 55);

  let terminal = null;
  await waitUntil("0 Portal spend reaches exact server terminal and client convergence", async () => {
    const events = parseZubspaceEvents(server.logs.join("").slice(placementLogOffset))
      .map(({ event }) => event)
      .filter((event) =>
        event.type === "server_portal_placement_terminal" &&
        event.player_id === playerId
      );
    terminal = events.at(-1) || null;
    const metrics = (await actor.metricsState()).uiMetrics || {};
    const local = metrics.localGameplayState || {};
    return (
      terminal?.outcome === 1 &&
      terminal?.portals_remaining === 0 &&
      semanticCounterDelta(beforeCounts, metrics.semanticInputCounts, "portal") === 1 &&
      local.pendingPortalPlacements === 0 &&
      local.portals === 0 &&
      local.prizeAuthorityRevision === terminal.prize_state_revision
    );
  }, 6_000);

  const after = await actor.pageState();
  const wire = mergedPacketLog(after)
    .filter((entry) => (entry.at ?? 0) >= placementStartedAt)
    .filter((entry) => Array.isArray(entry.bytes)
      && entry.bytes.length >= 7
      && entry.bytes[0] === 0x00
      && entry.bytes[1] === 0x03);
  const request = wire.find((entry) => entry.direction === "sent" && entry.bytes[6] === 0x65);
  const receipt = wire.find((entry) => entry.direction === "received" && entry.bytes[6] === 0x66);
  const requestActionId = request && request.bytes.length === 36
    ? readPacketLogLeU32(request.bytes, 30)
    : null;
  const requestPositionSequence = request && request.bytes.length === 36
    ? request.bytes[34] | (request.bytes[35] << 8)
    : null;
  const receiptActionId = receipt && receipt.bytes.length === 58
    ? readPacketLogLeU32(receipt.bytes, 30)
    : null;
  const receiptPositionSequence = receipt && receipt.bytes.length === 58
    ? receipt.bytes[34] | (receipt.bytes[35] << 8)
    : null;
  const receiptOutcome = receipt?.bytes?.[41] ?? null;
  if (
    !request || !receipt ||
    request.detail?.channel !== RELIABLE_CHANNEL_LABEL ||
    receipt.detail?.channel !== RELIABLE_CHANNEL_LABEL ||
    requestActionId !== receiptActionId ||
    requestActionId !== terminal.action_id ||
    requestPositionSequence !== receiptPositionSequence ||
    requestPositionSequence !== terminal.position_sequence ||
    receiptOutcome !== 1 ||
    !(request.seq < receipt.seq) ||
    semanticCounterDelta(beforeCounts, after.uiMetrics?.semanticInputCounts, "portal") !== 1
  ) {
    throw new Error(`Portal RTC transaction evidence diverged: ${JSON.stringify({
      target,
      claim,
      before: before.uiMetrics,
      after: after.uiMetrics,
      terminal,
      request,
      receipt,
      requestActionId,
      requestPositionSequence,
      receiptActionId,
      receiptPositionSequence,
      receiptOutcome,
    })}`);
  }

  const placedLocal = after.uiMetrics?.localGameplayState || {};
  if (
    placedLocal.portalActive !== true ||
    Math.abs((placedLocal.portalX ?? Number.NaN) - terminal.position_x) > 0.01 ||
    Math.abs((placedLocal.portalY ?? Number.NaN) - terminal.position_y) > 0.01 ||
    !(placedLocal.portalTtl > 0)
  ) {
    throw new Error(`authoritative Portal entity did not converge on the browser: ${JSON.stringify({
      terminal,
      placedLocal,
    })}`);
  }

  // Leave the endpoint using ordinary controls, then recall it with the bare
  // Insert semantic edge. The reliable terminal, not local prediction,
  // chooses and consumes the endpoint and rebases exact motion state.
  await actor.hold("a", 260);
  await actor.hold("w", 900);
  let recallBefore = null;
  await waitUntil("real movement leaves the authoritative Portal endpoint", async () => {
    recallBefore = await actor.pageState();
    const metrics = recallBefore.uiMetrics || {};
    return Math.hypot(
      (metrics.playerX ?? terminal.position_x) - terminal.position_x,
      (metrics.playerY ?? terminal.position_y) - terminal.position_y,
    ) > 48;
  }, 5_000);
  // Shift+0 must choose a random destination without consuming the endpoint.
  await waitUntil("full energy before forced random warp", async () => {
    const m = (await actor.metricsState()).uiMetrics;
    return m.playerEnergy >= m.playerMaxEnergy && m.serverPlayerEnergy >= m.playerMaxEnergy;
  }, 12_000);
  const randomOffset = server.logs.join("").length;
  const randomBefore = await actor.pageState();
  await actor.chord("Shift", "0", 55);
  let randomTerminal = null;
  await waitUntil("Shift+0 bypasses and preserves the live portal", async () => {
    randomTerminal = parseZubspaceEvents(server.logs.join("").slice(randomOffset))
      .map(({ event }) => event).find(event => event.type === "server_portal_warp_terminal" && event.player_id === playerId) || null;
    const m = (await actor.metricsState()).uiMetrics;
    const l = m.localGameplayState;
    return randomTerminal?.outcome === 1 && randomTerminal.used_portal === false &&
      !l.pendingPortalWarp && l.portalActive &&
      l.portalX === after.uiMetrics.localGameplayState.portalX && l.portalY === after.uiMetrics.localGameplayState.portalY &&
      // Ordinary movement can collect MultiPrize between placement and this
      // Warp. Preserve the inventory at the actual action boundary; an older
      // zero baseline would reject that award and could accept a real spend.
      l.portals === randomBefore.uiMetrics.localGameplayState.portals &&
      Math.hypot(m.playerX - randomTerminal.position_x, m.playerY - randomTerminal.position_y) <= 16;
  }, 6_000);
  await waitUntil("random warp settle completes before portal recall", async () =>
    (await actor.metricsState()).uiMetrics.localGameplayState.fakeAntiwarpTimer <= 0, 5_000);
  recallBefore = await actor.pageState();
  const randomRequest = mergedPacketLog(recallBefore).filter(entry =>
    entry.direction === "sent" && entry.bytes?.length === 37 && entry.bytes[6] === 0x6a && entry.bytes[7] === 2 && entry.bytes[36] === 1).at(-1);
  if (!randomRequest || randomRequest.detail?.channel !== RELIABLE_CHANNEL_LABEL) throw new Error("forced random warp lacks mode-specific reliable request evidence");
  const recallCounts = recallBefore.uiMetrics?.semanticInputCounts || {};
  const recallStartedAt = Date.now();
  const recallLogOffset = server.logs.join("").length;
  await actor.hold("0", 55);

  let recallTerminal = null;
  await waitUntil("0 reaches authoritative Portal recall terminal", async () => {
    recallTerminal = parseZubspaceEvents(server.logs.join("").slice(recallLogOffset))
      .map(({ event }) => event)
      .filter((event) =>
        event.type === "server_portal_warp_terminal" && event.player_id === playerId
      )
      .at(-1) || null;
    const metrics = (await actor.metricsState()).uiMetrics || {};
    const local = metrics.localGameplayState || {};
    return (
      recallTerminal?.outcome === 1 &&
      recallTerminal?.used_portal === true &&
      semanticCounterDelta(recallCounts, metrics.semanticInputCounts, "warp") === 1 &&
      local.pendingPortalWarp === false &&
      local.portalActive === false &&
      Math.hypot(
        (metrics.playerX ?? Number.NaN) - recallTerminal.position_x,
        (metrics.playerY ?? Number.NaN) - recallTerminal.position_y,
      ) <= 16
    );
  }, 6_000);

  const recallAfter = await actor.pageState();
  const recallWire = mergedPacketLog(recallAfter)
    .filter((entry) => (entry.at ?? 0) >= recallStartedAt)
    .filter((entry) => Array.isArray(entry.bytes)
      && entry.bytes.length >= 7
      && entry.bytes[0] === 0x00
      && entry.bytes[1] === 0x03);
  const recallRequest = recallWire.find((entry) =>
    entry.direction === "sent" && entry.bytes[6] === 0x6a
  );
  const recallReceipt = recallWire.find((entry) =>
    entry.direction === "received" && entry.bytes[6] === 0x6b
  );
  const recallRequestActionId = recallRequest?.bytes?.length === 37
    ? readPacketLogLeU32(recallRequest.bytes, 30)
    : null;
  const recallReceiptActionId = recallReceipt?.bytes?.length === 64
    ? readPacketLogLeU32(recallReceipt.bytes, 30)
    : null;
  const recallRequestSequence = recallRequest?.bytes?.length === 37
    ? recallRequest.bytes[34] | (recallRequest.bytes[35] << 8)
    : null;
  const recallReceiptSequence = recallReceipt?.bytes?.length === 64
    ? recallReceipt.bytes[34] | (recallReceipt.bytes[35] << 8)
    : null;
  const receiptState = recallReceipt?.bytes?.length === 64
    ? {
        positionX: readPacketLogLeF32(recallReceipt.bytes, 40),
        positionY: readPacketLogLeF32(recallReceipt.bytes, 44),
        velocityX: readPacketLogLeF32(recallReceipt.bytes, 48),
        velocityY: readPacketLogLeF32(recallReceipt.bytes, 52),
        orientation: readPacketLogLeF32(recallReceipt.bytes, 56),
        energy: recallReceipt.bytes[60] | (recallReceipt.bytes[61] << 8),
        usedPortal: recallReceipt.bytes[62] === 1,
        outcome: recallReceipt.bytes[63],
      }
    : null;
  // Random Warp deliberately drains energy, so ordinary recharge resumes
  // after recall. Verify exact application on the retained first-frame trace,
  // not a later pageState sample whose energy has already regenerated.
  const exactRecallApplicationFrame = (recallAfter.metricsTrace || []).find(frame =>
    frame.at >= (recallReceipt?.at ?? Infinity) &&
    frame.playerEnergy === receiptState?.energy &&
    Math.hypot(frame.playerX - receiptState.positionX, frame.playerY - receiptState.positionY) <= 0.02);
  if (
    !recallRequest || !recallReceipt ||
    recallRequest.bytes[7] !== 2 || recallRequest.bytes[36] !== 0 ||
    recallRequest.detail?.channel !== RELIABLE_CHANNEL_LABEL ||
    recallReceipt.detail?.channel !== RELIABLE_CHANNEL_LABEL ||
    recallRequestActionId !== recallReceiptActionId ||
    recallRequestActionId !== recallTerminal.action_id ||
    recallRequestSequence !== recallReceiptSequence ||
    recallRequestSequence !== recallTerminal.position_sequence ||
    receiptState?.usedPortal !== true ||
    receiptState?.outcome !== 1 ||
    Math.abs(receiptState.positionX - terminal.position_x) > 0.01 ||
    Math.abs(receiptState.positionY - terminal.position_y) > 0.01 ||
    Math.abs(receiptState.velocityX - recallAfter.uiMetrics.playerVelocityX) > 3 ||
    Math.abs(receiptState.velocityY - recallAfter.uiMetrics.playerVelocityY) > 3 ||
    Math.abs(receiptState.orientation - recallAfter.uiMetrics.playerOrientation) > 0.01 ||
    receiptState.energy !== recallTerminal.energy ||
    !exactRecallApplicationFrame ||
    !(recallRequest.seq < recallReceipt.seq)
  ) {
    throw new Error(`Portal recall RTC transaction evidence diverged: ${JSON.stringify({
      recallBefore: recallBefore.uiMetrics,
      recallAfter: recallAfter.uiMetrics,
      recallTerminal,
      recallRequest,
      recallReceipt,
      receiptState,
    })}`);
  }
  return {
    controls: ["w", "a", "d", "Shift+Insert (legacy setup)", "0 (place/use)", "Shift+0 (random)"],
    randomWarp: { terminal: randomTerminal, requestMode: randomRequest.bytes[36],
      portalPreserved: true, energyBefore: randomBefore.uiMetrics.playerEnergy },
    authoredChaosPortalWeight: 60,
    authoredWarbirdPortalMax: 3,
    naturalPrize: { target, claim, route },
    initialBountySetup: {
      portals: setupBefore.uiMetrics?.localGameplayState?.portals,
      placements: setupPlacements,
    },
    before: {
      portals: beforeLocal.portals,
      revision: beforeLocal.prizeAuthorityRevision,
      semanticPortal: beforeCounts.portal,
    },
    terminal,
    after: {
      portals: after.uiMetrics.localGameplayState.portals,
      pending: after.uiMetrics.localGameplayState.pendingPortalPlacements,
      revision: after.uiMetrics.localGameplayState.prizeAuthorityRevision,
      semanticPortal: after.uiMetrics.semanticInputCounts.portal,
      portalActive: after.uiMetrics.localGameplayState.portalActive,
      portalX: after.uiMetrics.localGameplayState.portalX,
      portalY: after.uiMetrics.localGameplayState.portalY,
      portalTtl: after.uiMetrics.localGameplayState.portalTtl,
    },
    rtc: {
      requestReliableId: readPacketLogLeU32(request.bytes, 2),
      receiptReliableId: readPacketLogLeU32(receipt.bytes, 2),
      requestChannel: request.detail.channel,
      receiptChannel: receipt.detail.channel,
      actionId: requestActionId,
      positionSequence: requestPositionSequence,
      requestPacketLength: request.bytes.length,
      receiptPacketLength: receipt.bytes.length,
    },
    recall: {
      before: {
        playerX: recallBefore.uiMetrics.playerX,
        playerY: recallBefore.uiMetrics.playerY,
        velocityX: recallBefore.uiMetrics.playerVelocityX,
        velocityY: recallBefore.uiMetrics.playerVelocityY,
        portalX: recallBefore.uiMetrics.localGameplayState.portalX,
        portalY: recallBefore.uiMetrics.localGameplayState.portalY,
      },
      terminal: recallTerminal,
      exactApplicationEnergy: exactRecallApplicationFrame.playerEnergy,
      after: {
        playerX: recallAfter.uiMetrics.playerX,
        playerY: recallAfter.uiMetrics.playerY,
        velocityX: recallAfter.uiMetrics.playerVelocityX,
        velocityY: recallAfter.uiMetrics.playerVelocityY,
        energy: recallAfter.uiMetrics.playerEnergy,
        portalActive: recallAfter.uiMetrics.localGameplayState.portalActive,
        pendingPortalWarp: recallAfter.uiMetrics.localGameplayState.pendingPortalWarp,
      },
      rtc: {
        requestReliableId: readPacketLogLeU32(recallRequest.bytes, 2),
        receiptReliableId: readPacketLogLeU32(recallReceipt.bytes, 2),
        requestChannel: recallRequest.detail.channel,
        receiptChannel: recallReceipt.detail.channel,
        actionId: recallRequestActionId,
        positionSequence: recallRequestSequence,
        requestPacketLength: recallRequest.bytes.length,
        receiptPacketLength: recallReceipt.bytes.length,
        state: receiptState,
      },
    },
  };
}

async function runPrizePhaseOneTimedGate({
  actor,
  peer,
  server,
  playerId,
  peerPlayerId,
  logOffset,
}) {
  const [actorInitial, peerInitial] = await Promise.all([actor.pageState(), peer.pageState()]);
  const actorFrequency = actorInitial.uiMetrics?.playerFrequency;
  const peerFrequency = peerInitial.uiMetrics?.playerFrequency;
  if (![actorFrequency, peerFrequency].every(Number.isInteger)) {
    throw new Error("phase-one team setup is missing authoritative frequencies");
  }
  const actorCombatShip = peerInitial.uiMetrics?.remoteActorPresentation
    ?.find((entry) => entry?.id === playerId)?.ship;
  const peerCombatShip = actorInitial.uiMetrics?.remoteActorPresentation
    ?.find((entry) => entry?.id === peerPlayerId)?.ship;
  if (![actorCombatShip, peerCombatShip]
      .every((ship) => Number.isInteger(ship) && ship >= 0 && ship < 8)) {
    throw new Error(`phase-one setup is missing authoritative combat ships: ${JSON.stringify({
      playerId,
      peerPlayerId,
      actorCombatShip,
      peerCombatShip,
    })}`);
  }
  const teamAlignment = actorFrequency === peerFrequency
    ? { alreadyAligned: true, frequency: actorFrequency }
    : await runFrequencyCutoverGate({
        actor: peer,
        server,
        targetFrequencyOverride: actorFrequency,
      });
  await waitUntil("phase-one natural Shield spawn", async () =>
    parseHiddenPrizeSpawns(server, logOffset).some((prize) => prize.prizeId === 18), 15_000);
  // All long random-spawn travel belongs before pickup/effect timing. Select
  // a live exact prize whose unchanged TTL covers both real-key routes, then
  // hold both pilots just outside it without collecting Shield/Shutdown early.
  const arena = await actor.evaluate("window.__zubspaceArena");
  const loadedMap = loadRendezvousMap(arena, [
    ...((server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT) ?
      [server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT] : []),
    path.join(repoRoot, "assets/zones"),
  ]);
  const prepareTimedPrize = async (prizeId, shooterStandoffPx = null) => {
    let selected = null;
    await waitUntil(`live hull-clear phase-one prize ${prizeId} preparation`, async () => {
      const states = await Promise.all([actor.metricsState(), peer.metricsState()]);
      const metrics = states.map(state => state.uiMetrics);
      if (!metrics.every(exactLiveGameplayScope)) return false;
      if (metrics.some(m => m.localGameplayState?.brickTilesOverflow))
        throw new Error("incomplete brick geometry for phase-one prize setup");
      const origins = metrics.map(m => ({ x: m.playerX, y: m.playerY }));
      const currentTick = metrics[0].latestSnapshotServerTick;
      const claimedIds = new Set(parsePrizeClaims(server).map(candidate => candidate.collectibleId));
      const live = parseHiddenPrizeSpawns(server, logOffset).filter(spawn => {
        const remaining = (spawn.expiresAtTick - currentTick) & 0x7fff_ffff;
        return !claimedIds.has(spawn.collectibleId) && remaining > 0 && remaining < 0x4000_0000;
      });
      const avoidTiles = live.filter(spawn => spawn.prizeId === 18 || spawn.prizeId === -14)
        .map(spawn => ({ x: Math.floor(spawn.x / 16), y: Math.floor(spawn.y / 16) }));
      const blockedTiles = metrics.flatMap(m => m.localGameplayState?.brickTiles || []);
      const candidates = live.filter(spawn => spawn.prizeId === prizeId).sort((a, b) =>
        Math.hypot(a.x - origins[0].x, a.y - origins[0].y) - Math.hypot(b.x - origins[0].x, b.y - origins[0].y));
      for (const spawn of candidates) {
        let preparation;
        try {
          preparation = planTimedPrizePreparation(loadedMap.geometry, origins, spawn, currentTick,
            { blockedTiles, avoidTiles, shooterStandoffPx });
        } catch (error) {
          if (!/no hull-clear authored approach route|no conservative route|no conservative timed-prize shooter standoff|origin lacks conservative hull clearance|not a clear tile-centred stage/.test(error.message)) throw error;
          continue;
        }
        if (preparation) { selected = { spawn, preparation, map: loadedMap.provenance }; return true; }
      }
      return false;
    }, 15_000);
    selected.encounter = await rendezvousPlayers(actor, peer, server, {
      setupBudgetMs: selected.preparation.setupBudgetMs, stageRadiusPx: 20,
      meetingPoint: selected.preparation.plan.target, avoidTiles: selected.preparation.avoidTiles,
    });
    const prepared = (await actor.metricsState()).uiMetrics;
    const remaining = (selected.spawn.expiresAtTick - prepared.latestSnapshotServerTick) & 0x7fff_ffff;
    if (!prepared.localGameplayState?.activePrizeCollectibleIds?.includes(selected.spawn.collectibleId) ||
        !(remaining > 0 && remaining < 0x4000_0000) ||
        parsePrizeClaims(server).some(candidate => candidate.collectibleId === selected.spawn.collectibleId))
      throw new Error(`phase-one prize no longer live and unclaimed after preparation: ${JSON.stringify(selected)}`);
    selected.remainingTicksAfterSetup = remaining;
    return selected;
  };
  const shieldPreparation = await prepareTimedPrize(18);
  let claim = null;
  const routes = [await navigateAuthoredTarget(actor, shieldPreparation.spawn, server, {
    timeoutMs: 40_000, arrivalRadiusPx: 16,
    avoidTiles: shieldPreparation.preparation.pickupAvoidTiles,
    complete: async () => {
      claim = parsePrizeClaims(server, logOffset).find(candidate => candidate.playerId === playerId &&
        candidate.collectibleId === shieldPreparation.spawn.collectibleId && candidate.prizeId === 18) || null;
      return claim;
    },
  })];
  if (!claim) {
    throw new Error(`phase-one natural Shield was not claimed: ${JSON.stringify({
      routes,
      spawns: parseHiddenPrizeSpawns(server, logOffset),
      claims: parsePrizeClaims(server, logOffset),
    })}`);
  }
  const shieldSpawn = parseHiddenPrizeSpawns(server, logOffset)
    .find((spawn) => spawn.collectibleId === claim.collectibleId);
  if (
    !shieldSpawn ||
    shieldSpawn.prizeId !== 18 ||
    shieldSpawn.x !== claim.x ||
    shieldSpawn.y !== claim.y
  ) {
    throw new Error(`phase-one Shield claim changed collectible identity: ${JSON.stringify({
      shieldSpawn,
      claim,
    })}`);
  }
  const collectorBrake = await brakeClientVelocity(actor, 8_000);
  if (!collectorBrake.braked) {
    throw new Error(`phase-one Shield collector could not stop after pickup: ${JSON.stringify(collectorBrake)}`);
  }

  let activated = null;
  await waitUntil("private Shield activation reconciles", async () => {
    const state = await actor.pageState();
    const timer = state.uiMetrics?.localGameplayState?.shieldTimer;
    const revision = state.uiMetrics?.localGameplayState?.prizeAuthorityRevision;
    if (Number.isFinite(timer) && timer > 0 && Number.isFinite(revision)) {
      activated = {
        timer,
        revision,
        lifeSeq: state.uiMetrics.localGameplayState.prizeAuthorityLifeSeq,
        snapshotId: state.uiMetrics.latestSnapshotId,
      };
      return true;
    }
    return false;
  }, 5_000);

  let shared = null;
  await waitUntil("teammate Shield persists through private PrizeState", async () => {
    const state = await peer.pageState();
    const local = state.uiMetrics?.localGameplayState;
    if (
      (local?.shieldTimer ?? 0) > 0 &&
      Number.isFinite(local?.prizeAuthorityRevision) &&
      claim.authoritativeRecipientCount >= 2
    ) {
      shared = {
        timer: local.shieldTimer,
        lifeSeq: local.prizeAuthorityLifeSeq,
        revision: local.prizeAuthorityRevision,
        snapshotId: state.uiMetrics.latestSnapshotId,
        authoritativeRecipientCount: claim.authoritativeRecipientCount,
      };
      return true;
    }
    return false;
  }, 5_000);

  let sharedReconciled = null;
  await waitUntil("teammate Shield survives a later private reconciliation", async () => {
    const state = await peer.pageState();
    const local = state.uiMetrics?.localGameplayState;
    const snapshotId = state.uiMetrics?.latestSnapshotId;
    const snapshotDelta = Number.isFinite(snapshotId)
      ? (snapshotId - shared.snapshotId) & 0xffff
      : 0;
    const revisionDelta = Number.isFinite(local?.prizeAuthorityRevision)
      ? (local.prizeAuthorityRevision - shared.revision) >>> 0
      : 0x8000_0000;
    if (
      snapshotDelta > 0 && snapshotDelta < 0x8000 &&
      revisionDelta < 0x8000_0000 &&
      local?.prizeAuthorityLifeSeq === shared.lifeSeq &&
      (local?.shieldTimer ?? 0) > 0
    ) {
      sharedReconciled = {
        timer: local.shieldTimer,
        lifeSeq: local.prizeAuthorityLifeSeq,
        revision: local.prizeAuthorityRevision,
        revisionDelta,
        snapshotId,
        snapshotDelta,
      };
      return true;
    }
    return false;
  }, 5_000);

  const opposingFrequency = actorFrequency === 2 ? 3 : 2;
  const opposingCutover = await runFrequencyCutoverGate({
    actor: peer,
    server,
    targetFrequencyOverride: opposingFrequency,
  });
  const actorAtShield = await actor.pageState();
  const shieldRoute = await navigateAuthoredTarget(peer, {
    x: actorAtShield.uiMetrics?.playerX, y: actorAtShield.uiMetrics?.playerY,
  }, server, { timeoutMs: 20_000, arrivalRadiusPx: 28,
    maximumDesiredSpeed: 100, desiredSpeedScale: 0.5,
    avoidTiles: shieldPreparation.preparation.pickupAvoidTiles });
  const shieldShooterBrake = await brakeClientVelocity(peer, 8_000);
  const [shieldShooterReady, shieldVictimReady] = await Promise.all([
    peer.pageState(),
    actor.pageState(),
  ]);
  const shieldAim = await orientClientForVelocity(
    peer,
    shieldVictimReady.uiMetrics.playerX - shieldShooterReady.uiMetrics.playerX,
    shieldVictimReady.uiMetrics.playerY - shieldShooterReady.uiMetrics.playerY,
    4_000,
  );
  if (!shieldRoute.reached || !shieldShooterBrake.braked || !shieldAim.oriented) {
    throw new Error(`phase-one Shield damage pair did not settle and aim: ${JSON.stringify({
      shieldRoute,
      shieldShooterBrake,
      shieldAim,
    })}`);
  }
  const shieldDamageStart = parseServerDamageEvents(server.logs).length;
  const shieldedDamage = await fireUntilDamage({
    label: "phase-one shielded teammate counterfire",
    shooter: peer,
    victim: actor,
    server,
    shooterId: peerPlayerId,
    victimId: playerId,
    sinceCount: shieldDamageStart,
    requireLethal: false,
    timeoutMs: 12_000,
    singlePressMs: 220,
  });
  const shieldFraction = Math.fround(
    Math.fround(shieldedDamage.event.shieldRemainingTicks) /
    Math.fround(shieldedDamage.event.shieldLifetimeTicks),
  );
  const shieldProtectedDamage = Math.trunc(Math.fround(
    Math.fround(shieldedDamage.event.unprotectedDamage) * shieldFraction,
  ));
  const expectedShieldedDamage =
    shieldedDamage.event.unprotectedDamage - shieldProtectedDamage;
  if (
    !(shieldedDamage.event.shieldRemainingTicks > 0) ||
    !(shieldedDamage.event.shieldLifetimeTicks > 0) ||
    shieldedDamage.event.damage !== expectedShieldedDamage
  ) {
    throw new Error(`Shield did not apply the exact authoritative remaining-fraction damage: ${JSON.stringify({
      shieldRoute,
      damage: shieldedDamage.event,
      shieldFraction,
      shieldProtectedDamage,
      expectedShieldedDamage,
    })}`);
  }

  const [beforeReset, peerBeforeReset] = await Promise.all([
    actor.pageState(),
    peer.pageState(),
  ]);
  const oldLife = beforeReset.uiMetrics?.playerLifeSeq;
  const actorPresentation = peerBeforeReset.uiMetrics?.remoteActorPresentation
    ?.find((entry) => entry?.id === playerId);
  const oldShip = actorPresentation?.ship;
  if (!Number.isInteger(oldShip) || oldShip < 0 || oldShip >= 8) {
    throw new Error(`ship reset cannot resolve the authoritative current ship: ${JSON.stringify({
      playerId,
      actorPresentation,
    })}`);
  }
  // Select a genuinely different ordinary ship through the real number-key
  // path. The initial server roster currently enters ship 1 directly, so a
  // fixed `1` key would be a same-ship no-op and would not exercise lifecycle
  // replacement at all.
  const replacementShip = oldShip === 0 ? 1 : 0;
  await actor.press("Escape", 60);
  await actor.press(String(replacementShip + 1), 60);
  let reset = null;
  await waitUntil("ship reset publishes replacement prize life", async () => {
    const state = await actor.pageState();
    const local = state.uiMetrics?.localGameplayState;
    const life = state.uiMetrics?.playerLifeSeq;
    if (Number.isFinite(life) && life !== oldLife && local?.prizeAuthorityLifeSeq === life &&
        local?.shieldTimer === 0 && local?.superTimer === 0 && local?.engineShutdownTimer === 0) {
      reset = {
        lifeSeq: life,
        prizeLifeSeq: local.prizeAuthorityLifeSeq,
        revision: local.prizeAuthorityRevision,
        shieldTimer: local.shieldTimer,
        superTimer: local.superTimer,
        engineShutdownTimer: local.engineShutdownTimer,
      };
      return true;
    }
    return false;
  }, 8_000);

  // Ship replacement starts at the arena's actual randomized team spawn.
  // Recover a live encounter before Rocket/Repel timing or inventory begins;
  // an old peer pose is not evidence that Repel can reach this new life.
  const rocketEncounter = await rendezvousPlayers(actor, peer, server);
  const rocketBefore = await actor.pageState();
  await actor.press("F3", 60);
  let rocket = null;
  await waitUntil("phase-one Rocket regression activates", async () => {
    const state = await actor.pageState();
    if ((state.uiMetrics?.playerRockets ?? 0) === (rocketBefore.uiMetrics?.playerRockets ?? 0) - 1 &&
        (state.uiMetrics?.playerRocketTimer ?? 0) > 0 &&
        state.uiMetrics?.pendingRocketActivationSequence == null) {
      rocket = {
        before: rocketBefore.uiMetrics.playerRockets,
        after: state.uiMetrics.playerRockets,
        timer: state.uiMetrics.playerRocketTimer,
        lifeSeq: state.uiMetrics.lastSelfServerRocketLifeSeq,
      };
      return true;
    }
    return false;
  }, 5_000);

  const peerBefore = await peer.pageState();
  await actor.press("q", 80);
  let repel = null;
  await waitUntil("phase-one Repel regression applies privately", async () => {
    const state = await peer.pageState();
    const beforeApplied = peerBefore.uiMetrics?.localGameplayState?.repelDeliveryFramesApplied ?? 0;
    const afterApplied = state.uiMetrics?.localGameplayState?.repelDeliveryFramesApplied ?? 0;
    if (afterApplied > beforeApplied) {
      repel = {
        beforeApplied,
        afterApplied,
        timer: state.uiMetrics.localGameplayState.repelTimer,
        applicationRejects: state.uiMetrics.localGameplayState.repelDeliveryApplicationRejects,
      };
      return true;
    }
    return false;
  }, 5_000);

  const beforeCombatRestore = await actor.pageState();
  const restoreFromLife = beforeCombatRestore.uiMetrics?.playerLifeSeq;
  await actor.press("Escape", 60);
  await actor.press(String(oldShip + 1), 60);
  let combatRestore = null;
  await waitUntil("phase-one replay restores the combat ship baseline", async () => {
    const state = await actor.pageState();
    const life = state.uiMetrics?.playerLifeSeq;
    const local = state.uiMetrics?.localGameplayState;
    if (Number.isFinite(life) && life !== restoreFromLife &&
        local?.prizeAuthorityLifeSeq === life &&
        local?.rocketTimer === 0 && local?.repelTimer === 0) {
      combatRestore = {
        lifeSeq: life,
        prizeLifeSeq: local.prizeAuthorityLifeSeq,
        revision: local.prizeAuthorityRevision,
      };
      return true;
    }
    return false;
  }, 8_000);
  const peerBeforeEmpShip = await peer.pageState();
  const peerEmpOldLife = peerBeforeEmpShip.uiMetrics?.playerLifeSeq;
  await peer.press("Escape", 60);
  await peer.press("6", 60);
  await waitUntil("EMP shooter ship reset publishes private prize life", async () => {
    const state = await peer.pageState();
    const life = state.uiMetrics?.playerLifeSeq;
    return Number.isFinite(life) && life !== peerEmpOldLife &&
      state.uiMetrics?.localGameplayState?.prizeAuthorityLifeSeq === life;
  }, 8_000);
  // Both replacement lives must meet through ordinary keys first. The larger
  // clear stage certifies the existing +96px BombSafety standoff and its line
  // of fire without changing the original timed EMP assertions below.
  const empEncounter = await rendezvousPlayers(actor, peer, server, { stageRadiusPx: 192 });
  const actorBeforeEmp = await actor.pageState();
  const empStandOff = {
    x: actorBeforeEmp.uiMetrics?.playerX + 96,
    y: actorBeforeEmp.uiMetrics?.playerY,
  };
  const empRoute = await driveClientToWorldTarget(
    peer,
    empStandOff,
    20_000,
    false,
    16,
  );
  const empShooterBrake = await brakeClientVelocity(peer, 8_000);
  const [empShooterReady, empVictimReady] = await Promise.all([
    peer.pageState(),
    actor.pageState(),
  ]);
  const empAim = await orientClientForVelocity(
    peer,
    empVictimReady.uiMetrics.playerX - empShooterReady.uiMetrics.playerX,
    empVictimReady.uiMetrics.playerY - empShooterReady.uiMetrics.playerY,
    4_000,
  );
  if (!empRoute.reached) {
    throw new Error(`phase-one EMP pair did not reach its BombSafety standoff: ${JSON.stringify({
      empStandOff,
      empRoute,
      empShooterBrake,
      empAim,
    })}`);
  }
  const empDamageStart = parseServerDamageEvents(server.logs).length;
  const empDamage = await fireUntilDamage({
    label: "phase-one EMP recharge suppression",
    shooter: peer,
    victim: actor,
    server,
    shooterId: peerPlayerId,
    victimId: playerId,
    sinceCount: empDamageStart,
    requireLethal: false,
    timeoutMs: 15_000,
    forceBomb: true,
  });
  if (!(empDamage.event.empUntilTick > 0)) {
    throw new Error(`EMP bomb did not publish an authoritative recharge-suppression deadline: ${JSON.stringify(empDamage.event)}`);
  }
  let empActivated = null;
  await waitUntil("EMP victim reconciles recharge suppression without engine Shutdown", async () => {
    const state = await actor.metricsState();
    const local = state.uiMetrics?.localGameplayState;
    const remaining = (empDamage.event.empUntilTick - state.uiMetrics?.latestSnapshotServerTick) & 0x7fff_ffff;
    if (local?.engineShutdownTimer === 0 && remaining > 0 && remaining < 0x4000_0000 &&
        state.uiMetrics?.lastSelfDamageServerSeq === empDamage.event.damageSeq) {
      empActivated = {
        timer: local.engineShutdownTimer,
        snapshotTick: state.uiMetrics?.latestSnapshotServerTick,
        x: state.uiMetrics?.playerX,
        y: state.uiMetrics?.playerY,
        energy: state.uiMetrics?.playerEnergy,
      };
      return true;
    }
    return false;
  }, 5_000);
  // Keep this natural damage-prize-loss fixture still while its below-cap
  // energy is frozen. The all-ship weapon-trait lane separately requires
  // genuine thrust/turn/fire during EMP, which does not shut engines down.
  await sleep(350);
  const empIdle = await actor.metricsState();
  const idleMovementDistance = Math.hypot(
    (empIdle.uiMetrics?.playerX ?? 0) - empActivated.x,
    (empIdle.uiMetrics?.playerY ?? 0) - empActivated.y,
  );
  const idleSpeed = Math.hypot(
    empIdle.uiMetrics?.playerVelocityX ?? 0,
    empIdle.uiMetrics?.playerVelocityY ?? 0,
  );
  const idleEnergyDelta = Math.abs(
    (empIdle.uiMetrics?.playerEnergy ?? 0) - empActivated.energy,
  );
  if (
    empIdle.uiMetrics?.localGameplayState?.engineShutdownTimer !== 0 ||
    !(((empDamage.event.empUntilTick - empIdle.uiMetrics?.latestSnapshotServerTick) & 0x7fff_ffff) < 0x4000_0000) ||
    idleEnergyDelta > 2
  ) {
    throw new Error(`EMP victim gained engine Shutdown or recharged before its deadline: ${JSON.stringify({
      empActivated,
      idle: empIdle.uiMetrics,
      idleMovementDistance,
      idleSpeed,
      idleEnergyDelta,
    })}`);
  }
  // Damage-prize loss is eligible only when the victim was already below
  // maximum energy before the hit. The EMP bomb above establishes both the
  // exact authoritative aim line and the victim's frozen below-cap energy.
  // Fire the ordinary primary while that same EMP deadline remains active:
  // routing after expiry would let recharge erase eligibility, while applying
  // another harness-side state change would stop being a natural combat proof.
  // Shooter self-splash correctly does not apply EMP, so its ordinary recoil
  // can coexist with the shot and the primary cannot extend the victim's EMP.
  let damageLossProbe = null;
  let damageLossAim = null;
  let damageLossShooterRoute = null;
  let damageLossShooterBrake = null;
  if (parseDamagePrizeLosses(server, logOffset).length === 0) {
    const [damageLossShooter, damageLossVictim] = await Promise.all([
      peer.metricsState(),
      actor.metricsState(),
    ]);
    const damageLossSnapshotTick = damageLossVictim.uiMetrics?.latestSnapshotServerTick;
    const damageLossTicksRemaining = Number.isInteger(damageLossSnapshotTick)
      ? (empDamage.event.empUntilTick - damageLossSnapshotTick) & 0x7fff_ffff
      : 0;
    const damageLossVictimEnergy = damageLossVictim.uiMetrics?.serverPlayerEnergy;
    if (
      damageLossVictim.uiMetrics?.localGameplayState?.engineShutdownTimer !== 0 ||
      !(damageLossTicksRemaining > 0 && damageLossTicksRemaining < 0x4000_0000) ||
      !(damageLossVictimEnergy < damageLossVictim.uiMetrics?.playerMaxEnergy)
    ) {
      throw new Error(`phase-one natural damage-loss probe lost its EMP eligibility boundary: ${JSON.stringify({
        damageLossSnapshotTick,
        damageLossTicksRemaining,
        damageLossVictimEnergy,
        maxEnergy: damageLossVictim.uiMetrics?.playerMaxEnergy,
        shooter: damageLossShooter.uiMetrics,
        victim: damageLossVictim.uiMetrics,
      })}`);
    }
    // The EMP hit already proved this live firing corridor. Brake its real
    // recoil and aim from the current pose; an extra128px standoff trip is
    // unrelated to prize-loss eligibility and can outlast the unchanged EMP.
    damageLossShooterRoute = { required: false, reason: "reuse the live EMP firing corridor" };
    const liveDamageLossBrake = await brakeClientVelocity(peer, 4_000);
    if (!liveDamageLossBrake.braked) {
      throw new Error(`phase-one natural damage-loss shooter did not brake authentic EMP recoil: ${JSON.stringify({
        empAim,
        damageLossShooter: damageLossShooter.uiMetrics,
        damageLossVictim: damageLossVictim.uiMetrics,
        liveDamageLossBrake,
      })}`);
    }
    damageLossShooterBrake = {
      preservedEmpRecoil: true,
      speedBefore: Math.hypot(
        damageLossShooter.uiMetrics?.playerVelocityX ?? 0,
        damageLossShooter.uiMetrics?.playerVelocityY ?? 0,
      ),
      liveBrake: liveDamageLossBrake,
    };
    const [brakedDamageLossShooter, brakedDamageLossVictim] = await Promise.all([
      peer.metricsState(),
      actor.metricsState(),
    ]);
    const brakedDamageLossVictimEnergy = brakedDamageLossVictim.uiMetrics?.serverPlayerEnergy;
    if (
      !(brakedDamageLossVictimEnergy < brakedDamageLossVictim.uiMetrics?.playerMaxEnergy) ||
      !(brakedDamageLossVictimEnergy >= 400)
    ) {
      throw new Error(`phase-one natural damage-loss brake lost its exact nonlethal eligibility: ${JSON.stringify({
        damageLossVictimEnergy,
        brakedDamageLossVictim: brakedDamageLossVictim.uiMetrics,
        liveDamageLossBrake,
      })}`);
    }
    const liveDamageLossAim = await orientClientForProtocolDirection(
      peer,
      brakedDamageLossVictim.uiMetrics.playerX - brakedDamageLossShooter.uiMetrics.playerX,
      brakedDamageLossVictim.uiMetrics.playerY - brakedDamageLossShooter.uiMetrics.playerY,
      4_000,
    );
    if (!liveDamageLossAim.oriented) {
      throw new Error(`phase-one natural damage-loss shooter did not preserve its live aim line: ${JSON.stringify({
        empAim,
        damageLossShooter: damageLossShooter.uiMetrics,
        damageLossVictim: damageLossVictim.uiMetrics,
        liveDamageLossAim,
      })}`);
    }
    const shotBoundary = (await actor.metricsState()).uiMetrics || {};
    const shotBoundaryRemaining = (empDamage.event.empUntilTick - shotBoundary.latestSnapshotServerTick) & 0x7fff_ffff;
    if (!(shotBoundaryRemaining > 0 && shotBoundaryRemaining < 0x4000_0000))
      throw new Error("phase-one natural damage-loss aim outlasted the authoritative EMP deadline");
    const requireEmpProbeTime = event => {
      const remaining = (empDamage.event.empUntilTick - event.serverTick) & 0x7fff_ffff;
      if (!Number.isInteger(event.serverTick) || !(remaining > 0 && remaining < 0x4000_0000))
        throw new Error("phase-one natural damage-loss hit outlasted the authoritative EMP deadline");
    };
    damageLossAim = {
      shotBoundaryTick: shotBoundary.latestSnapshotServerTick,
      shotBoundaryRemaining,
      preservedEmpRecoil: true,
      empAim,
      liveAim: liveDamageLossAim,
      snapshotTick: damageLossSnapshotTick,
      ticksRemaining: damageLossTicksRemaining,
      victimEnergy: damageLossVictimEnergy,
      victimEnergyAfterBrake: brakedDamageLossVictimEnergy,
      victimMaxEnergy: damageLossVictim.uiMetrics?.playerMaxEnergy,
    };
    damageLossProbe = await fireUntilDamage({
      label: "phase-one natural damage-prize loss",
      shooter: peer,
      victim: actor,
      server,
      shooterId: peerPlayerId,
      victimId: playerId,
      sinceCount: empDamage.nextSinceCount,
      requireLethal: false,
      timeoutMs: 3_000,
      singlePressMs: 220,
    });
    requireEmpProbeTime(damageLossProbe.event);
    // DamageFactor is a canonical chance, not a guaranteed first eligible
    // outcome. If that draw was None, take one more ordinary primary only
    // while the frozen authoritative energy proves it cannot be lethal.
    if (parseDamagePrizeLosses(server, logOffset).length === 0) {
      const afterFirstProbe = await actor.metricsState();
      if ((afterFirstProbe.uiMetrics?.playerEnergy ?? 0) >= 400) {
        damageLossProbe = await fireUntilDamage({
          label: "phase-one natural damage-prize loss retry",
          shooter: peer,
          victim: actor,
          server,
          shooterId: peerPlayerId,
          victimId: playerId,
          sinceCount: damageLossProbe.nextSinceCount,
          requireLethal: false,
          timeoutMs: 3_000,
          singlePressMs: 220,
        });
        requireEmpProbeTime(damageLossProbe.event);
      }
    }
  }
  let capturedDamageLoss = null;
  await waitUntil("phase-one damage loss reaches its exact private browser state", async () => {
    const losses = parseDamagePrizeLosses(server, logOffset);
    for (const loss of losses) {
      const client = loss.playerId === playerId
        ? actor
        : loss.playerId === peerPlayerId ? peer : null;
      if (!client) continue;
      const state = await client.metricsState();
      const local = state.uiMetrics?.localGameplayState;
      if (
        local?.damagePrizeLossSeq === loss.damageSeq &&
        local?.damagePrizeLossId === loss.prizeId &&
        (local?.damagePrizeNotificationCount ?? 0) > 0 &&
        typeof local?.lastDamagePrizeNotification === "string" &&
        local.lastDamagePrizeNotification.startsWith("DAMAGE:") &&
        Number.isFinite(local?.prizeAuthorityRevision) &&
        ((local.prizeAuthorityRevision - loss.revision) >>> 0) < 0x8000_0000
      ) {
        capturedDamageLoss = {
          ...loss,
          browser: loss.playerId === playerId ? "actor" : "peer",
          browserLifeSeq: local.prizeAuthorityLifeSeq,
          browserRevision: local.prizeAuthorityRevision,
          browserRevisionDelta: (local.prizeAuthorityRevision - loss.revision) >>> 0,
          snapshotId: state.uiMetrics?.latestSnapshotId,
          notificationCount: local.damagePrizeNotificationCount,
          notification: local.lastDamagePrizeNotification,
        };
        return true;
      }
    }
    return false;
  }, 8_000);
  let damageLossDuplicateReconciliation = null;
  const lossClient = capturedDamageLoss.browser === "actor" ? actor : peer;
  await waitUntil("duplicate private damage-loss marker does not notify twice", async () => {
    const state = await lossClient.metricsState();
    const local = state.uiMetrics?.localGameplayState;
    const snapshotId = state.uiMetrics?.latestSnapshotId;
    const snapshotDelta = Number.isInteger(snapshotId) && Number.isInteger(capturedDamageLoss.snapshotId)
      ? (snapshotId - capturedDamageLoss.snapshotId) & 0xffff
      : 0;
    if (
      snapshotDelta > 0 && snapshotDelta < 0x8000 &&
      local?.damagePrizeLossSeq === capturedDamageLoss.damageSeq &&
      local?.damagePrizeLossId === capturedDamageLoss.prizeId &&
      local?.damagePrizeNotificationCount === capturedDamageLoss.notificationCount &&
      local?.lastDamagePrizeNotification === capturedDamageLoss.notification
    ) {
      damageLossDuplicateReconciliation = {
        snapshotId,
        snapshotDelta,
        notificationCount: local.damagePrizeNotificationCount,
        notification: local.lastDamagePrizeNotification,
      };
      return true;
    }
    return false;
  }, 5_000);
  let empExpired = null;
  await waitUntil("EMP recharge suppression reaches the exact authoritative deadline", async () => {
    const state = await actor.metricsState();
    const local = state.uiMetrics?.localGameplayState;
    const snapshotTick = state.uiMetrics?.latestSnapshotServerTick;
    const elapsed = Number.isInteger(snapshotTick)
      ? (snapshotTick - empDamage.event.empUntilTick) & 0x7fff_ffff
      : 0x7fff_ffff;
    if (local?.engineShutdownTimer === 0 && elapsed < 0x4000_0000) {
      empExpired = {
        timer: local.engineShutdownTimer,
        snapshotTick,
        authoritativeDeadline: empDamage.event.empUntilTick,
        tickPastDeadline: elapsed,
        energy: state.uiMetrics?.playerEnergy,
        maxEnergy: state.uiMetrics?.playerMaxEnergy,
      };
      return true;
    }
    return false;
  }, 12_000);

  const empRechargeThreshold = Math.max((empExpired.energy ?? 0) + 100, 800);
  await waitUntil("EMP-only victim naturally recharges before the union replay", async () => {
    const state = await actor.pageState();
    return (state.uiMetrics?.playerEnergy ?? 0) >= empRechargeThreshold;
  }, 10_000);

  const peerBeforeUnionRestore = await peer.pageState();
  const peerUnionOldLife = peerBeforeUnionRestore.uiMetrics?.playerLifeSeq;
  await peer.press("Escape", 60);
  await peer.press("2", 60);
  let unionShooterCentering = null;
  await waitUntil("union shooter restores a replacement ordinary lifecycle", async () => {
    const state = await peer.pageState();
    const life = state.uiMetrics?.playerLifeSeq;
    const local = state.uiMetrics?.localGameplayState;
    if (
      Number.isFinite(life) && life !== peerUnionOldLife &&
      local?.prizeAuthorityLifeSeq === life &&
      local?.engineShutdownTimer === 0
    ) {
      unionShooterCentering = {
        lifeSeq: life,
        prizeLifeSeq: local.prizeAuthorityLifeSeq,
        revision: local.prizeAuthorityRevision,
        x: state.uiMetrics.playerX,
        y: state.uiMetrics.playerY,
      };
      return true;
    }
    return false;
  }, 8_000);
  await peer.press("Escape", 60);
  await peer.press("6", 60);
  let unionShooterRestore = null;
  await waitUntil("union shooter restores the replacement EMP lifecycle", async () => {
    const state = await peer.pageState();
    const life = state.uiMetrics?.playerLifeSeq;
    const local = state.uiMetrics?.localGameplayState;
    if (
      Number.isFinite(life) && life !== unionShooterCentering.lifeSeq &&
      local?.prizeAuthorityLifeSeq === life &&
      local?.engineShutdownTimer === 0
    ) {
      unionShooterRestore = {
        lifeSeq: life,
        prizeLifeSeq: local.prizeAuthorityLifeSeq,
        revision: local.prizeAuthorityRevision,
        x: state.uiMetrics.playerX,
        y: state.uiMetrics.playerY,
      };
      return true;
    }
    return false;
  }, 8_000);

  const shutdownPreparation = await prepareTimedPrize(-14, 160);
  const shutdownSpawn = shutdownPreparation.spawn;
  // Stage the EMP shooter beside the exact future pickup before the claim.
  // Shutdown suppresses thrust but intentionally preserves momentum, so a
  // post-claim slow chase can consume the authored seven-second deadline
  // while the victim coasts. This route is ordinary keyboard movement and
  // remains independent of the application ACK/terminal impairment below.
  // The prepared160px full-hull firing line stays outside Chaos's authored
  // 80px BombSafety radius; a traversable detour alone would not prove aim.
  const unionShooterStandOff = shutdownPreparation.preparation.shooterStandoff;
  const unionShooterRoute = await navigateAuthoredTarget(peer, unionShooterStandOff, server, {
    timeoutMs: 20_000, arrivalRadiusPx: 16,
    maximumDesiredSpeed: 100, desiredSpeedScale: 0.5,
    avoidTiles: shutdownPreparation.preparation.avoidTiles,
  });
  const unionPeerBrake = await brakeClientVelocity(peer, 8_000);
  const unionShooterStaged = await peer.pageState();
  const unionPreAim = await orientClientForVelocity(
    peer,
    shutdownSpawn.x - unionShooterStaged.uiMetrics.playerX,
    shutdownSpawn.y - unionShooterStaged.uiMetrics.playerY,
    4_000,
  );
  if (!unionShooterRoute.reached || !unionPeerBrake.braked || !unionPreAim.oriented) {
    throw new Error(`phase-one union EMP shooter did not stage at the exact future pickup: ${JSON.stringify({
      shutdownSpawn,
      unionShooterStandOff,
      unionShooterRoute,
      unionPeerBrake,
      unionPreAim,
    })}`);
  }
  const unionShooterBeforeClaim = await peer.pageState();
  const impairmentArmed = await actor.evaluate(
    "window.__zubspaceArmPrizeEffectImpairmentForTest?.() ?? false",
  );
  if (!impairmentArmed) {
    throw new Error("phase-one Shutdown application impairment could not be armed");
  }

  let shutdownClaim = null;
  const shutdownRoute = await navigateAuthoredTarget(actor, shutdownSpawn, server, {
    timeoutMs: 15_000, arrivalRadiusPx: 16,
    avoidTiles: shutdownPreparation.preparation.pickupAvoidTiles,
    complete: async () => {
      shutdownClaim = parsePrizeClaims(server, logOffset).find(candidate =>
        candidate.playerId === playerId && candidate.collectibleId === shutdownSpawn.collectibleId && candidate.prizeId === -14
      ) || null;
      return shutdownClaim;
    },
  });
  if (
    !shutdownClaim ||
    shutdownClaim.x !== shutdownSpawn.x ||
    shutdownClaim.y !== shutdownSpawn.y ||
    !(shutdownClaim.engineShutdownUntilTick > 0)
  ) {
    throw new Error(`phase-one Engine Shutdown claim changed identity or lacked its deadline: ${JSON.stringify({
      shutdownSpawn,
      shutdownClaim,
      shutdownRoute,
    })}`);
  }
  // From the authored Shutdown claim until the union hit, keep every sample
  // on the bounded hot metrics projection. A full page capture serializes the
  // retained packet/trace rings and can itself consume a material fraction of
  // the seven-second gameplay deadline this scenario is measuring.
  const beforeDelayedAckTail = await actor.metricsState();
  await actor.holdKeys(["ArrowUp", "ArrowRight"], 650);
  await actor.holdKeys(["ArrowUp", "ArrowLeft"], 650);
  await waitUntil("held Shutdown pickup ACK is retransmitted before release", async () => {
    const impairment = await actor.evaluate(
      "window.__zubspacePrizeEffectImpairmentStateForTest?.() ?? null",
    );
    return (impairment?.ackSuppressed ?? 0) > 0;
  }, 5_000);
  // This real-key brake still runs before the held ACK is applied. It both
  // keeps the staged EMP geometry intact and proves the authoritative maximum
  // boundary advances through the final old-semantics rotation/thrust tails.
  const delayedAckBrake = await brakeClientVelocity(actor, 3_500);
  if (!delayedAckBrake.braked) {
    throw new Error(`delayed Shutdown ACK could not brake under old semantics: ${JSON.stringify(delayedAckBrake)}`);
  }
  const afterDelayedAckTail = await actor.metricsState();
  const delayedAckImpairment = await actor.evaluate(
    "window.__zubspacePrizeEffectImpairmentStateForTest?.() ?? null",
  );
  const delayedAckTailDelta = Number.isInteger(beforeDelayedAckTail.uiMetrics?.clientInputSeq) &&
    Number.isInteger(afterDelayedAckTail.uiMetrics?.clientInputSeq)
      ? (afterDelayedAckTail.uiMetrics.clientInputSeq - beforeDelayedAckTail.uiMetrics.clientInputSeq) & 0xffff
      : 0;
  if (
    delayedAckImpairment?.ackBuffered !== 1 ||
    delayedAckImpairment?.ackReleased !== 0 ||
    delayedAckImpairment?.ackSuppressed < 1 ||
    delayedAckTailDelta < 30 ||
    afterDelayedAckTail.uiMetrics?.localGameplayState?.pendingPrizeEffectApplications !== 0
  ) {
    throw new Error(`delayed Shutdown ACK did not retain many real rotating/thrusting old-semantics tails: ${JSON.stringify({
      before: beforeDelayedAckTail.uiMetrics,
      after: afterDelayedAckTail.uiMetrics,
      delayedAckTailDelta,
      impairment: delayedAckImpairment,
    })}`);
  }
  const c2sTailArmed = await actor.evaluate(
    "window.__zubspaceArmPrizeEffectC2sTailForTest?.() ?? false",
  );
  if (!c2sTailArmed) {
    throw new Error("phase-one final C2S Shutdown tail impairment could not be armed");
  }
  let c2sTailBeforeMarker = null;
  await waitUntil("final C2S position with embedded input tail is held", async () => {
    const impairment = await actor.evaluate(
      "window.__zubspacePrizeEffectImpairmentStateForTest?.() ?? null",
    );
    if (
      impairment?.c2sTailPositionBuffered === 1 &&
      impairment?.c2sTailSuppressed >= 1 &&
      impairment?.c2sApplicationSeen === 0
    ) {
      c2sTailBeforeMarker = impairment;
      return true;
    }
    return false;
  }, 1_500);
  const releasedAck = await actor.evaluate(
    "window.__zubspaceReleasePrizeEffectAckForTest?.() ?? false",
  );
  if (!releasedAck) {
    throw new Error("held Shutdown pickup ACK could not be released");
  }
  let shutdownActivated = null;
  await waitUntil("server-registry Engine Shutdown reaches private authority", async () => {
    const state = await actor.metricsState();
    const local = state.uiMetrics?.localGameplayState;
    if ((local?.engineShutdownTimer ?? 0) > 0) {
      shutdownActivated = {
        timer: local.engineShutdownTimer,
        lifeSeq: local.prizeAuthorityLifeSeq,
        revision: local.prizeAuthorityRevision,
        snapshotTick: state.uiMetrics.latestSnapshotServerTick,
        x: state.uiMetrics.playerX,
        y: state.uiMetrics.playerY,
        velocityX: state.uiMetrics.playerVelocityX,
        velocityY: state.uiMetrics.playerVelocityY,
        energy: state.uiMetrics.playerEnergy,
      };
      return true;
    }
    return false;
  }, 5_000);
  let c2sMarkerOvertookTail = null;
  await waitUntil("Shutdown application marker overtakes the held final C2S position", async () => {
    const [state, impairment] = await Promise.all([
      actor.metricsState(),
      actor.evaluate("window.__zubspacePrizeEffectImpairmentStateForTest?.() ?? null"),
    ]);
    const local = state.uiMetrics?.localGameplayState;
    const predecessor = Number.isInteger(impairment?.c2sApplicationSequence)
      ? (impairment.c2sApplicationSequence - 1) & 0xffff
      : null;
    // Positions carry every simulation sequence and embed the corresponding
    // rolling input record, so this one product-emitted datagram must be the
    // exact predecessor. A standalone 0x62 is only catch-up/timer-gap
    // redundancy and is therefore optional; when naturally present, keep it
    // bounded by the same rolling-input window.
    const positionTailDistance = Number.isInteger(impairment?.c2sApplicationSequence) &&
      Number.isInteger(impairment?.c2sTailPositionSequence)
      ? (impairment.c2sApplicationSequence - impairment.c2sTailPositionSequence) & 0xffff
      : null;
    const inputTailDistance = Number.isInteger(impairment?.c2sApplicationSequence) &&
      Number.isInteger(impairment?.c2sTailInputSequence)
      ? (impairment.c2sApplicationSequence - impairment.c2sTailInputSequence) & 0xffff
      : null;
    const inputRingLen = state.uiMetrics?.localGameplayState?.inputRingLen;
    const optionalInputTailValid = impairment?.c2sTailInputBuffered !== 1 || (
      Number.isInteger(inputRingLen) &&
      inputTailDistance >= 1 &&
      inputTailDistance <= inputRingLen
    );
    if (
      impairment?.c2sApplicationSeen >= 1 &&
      impairment?.c2sTailPositionBuffered === 1 &&
      impairment?.terminalSeen === 0 &&
      local?.pendingPrizeEffectApplications === 1 &&
      positionTailDistance === 1 &&
      optionalInputTailValid
    ) {
      c2sMarkerOvertookTail = {
        ...impairment,
        predecessor,
        positionTailDistance,
        inputTailDistance,
        inputRingLen,
        clientInputSeq: state.uiMetrics?.clientInputSeq,
      };
      return true;
    }
    return false;
  }, 2_000);
  const releasedC2sTail = await actor.evaluate(
    "window.__zubspaceReleasePrizeEffectC2sTailForTest?.() ?? false",
  );
  if (!releasedC2sTail) {
    throw new Error(`held final C2S Shutdown tail could not be released: ${JSON.stringify(c2sMarkerOvertookTail)}`);
  }
  let heldTerminal = null;
  await waitUntil("Shutdown application terminal is held after exact client application", async () => {
    const [state, impairment] = await Promise.all([
      actor.metricsState(),
      actor.evaluate("window.__zubspacePrizeEffectImpairmentStateForTest?.() ?? null"),
    ]);
    const local = state.uiMetrics?.localGameplayState;
    if (
      impairment?.terminalBuffered === 1 &&
      impairment?.terminalReleased === 0 &&
      local?.pendingPrizeEffectApplications === 1 &&
      Number.isInteger(local?.pendingPrizeEffectApplicationSequence)
    ) {
      heldTerminal = {
        impairment,
        clientInputSeq: state.uiMetrics?.clientInputSeq,
        pendingSequence: local.pendingPrizeEffectApplicationSequence,
        energy: state.uiMetrics?.playerEnergy,
        x: state.uiMetrics?.playerX,
        y: state.uiMetrics?.playerY,
        velocityX: state.uiMetrics?.playerVelocityX,
        velocityY: state.uiMetrics?.playerVelocityY,
      };
      return true;
    }
    return false;
  }, 5_000);
  // The victim remains frozen at this exact application boundary while a
  // terminal retry is suppressed. Use that interval to stage the EMP shooter
  // with ordinary keys instead of serially spending the authored Shutdown
  // lifetime after the retry proof has completed.
  const terminalUnionStandOff = { x: heldTerminal.x + 180, y: heldTerminal.y };
  const [terminalBlockedProof, terminalShooterStage] = await Promise.all([
    (async () => {
      await actor.holdKeys(["ArrowUp", "ArrowRight"], 600);
      await waitUntil("held Shutdown application terminal is retransmitted before release", async () => {
        const impairment = await actor.evaluate(
          "window.__zubspacePrizeEffectImpairmentStateForTest?.() ?? null",
        );
        return (impairment?.terminalSuppressed ?? 0) > 0;
      }, 5_000);
      return {
        state: await actor.metricsState(),
        impairment: await actor.evaluate(
          "window.__zubspacePrizeEffectImpairmentStateForTest?.() ?? null",
        ),
      };
    })(),
    (async () => {
      const route = await driveClientToWorldTarget(
        peer,
        terminalUnionStandOff,
        5_000,
        false,
        14,
        null,
        140,
      );
      const brake = await brakeClientVelocity(peer, 3_000);
      const aim = await orientClientForVelocity(peer, -180, 0, 2_000);
      return { route, brake, aim };
    })(),
  ]);
  const terminalBlocked = terminalBlockedProof.state;
  const terminalRetryImpairment = terminalBlockedProof.impairment;
  const terminalShooterDistance = Math.hypot(
    terminalShooterStage.aim.metrics.playerX - heldTerminal.x,
    terminalShooterStage.aim.metrics.playerY - heldTerminal.y,
  );
  const terminalBlockedDistance = Math.hypot(
    (terminalBlocked.uiMetrics?.playerX ?? 0) - heldTerminal.x,
    (terminalBlocked.uiMetrics?.playerY ?? 0) - heldTerminal.y,
  );
  const terminalBlockedSpeedIncrease = Math.hypot(
    terminalBlocked.uiMetrics?.playerVelocityX ?? 0,
    terminalBlocked.uiMetrics?.playerVelocityY ?? 0,
  ) - Math.hypot(heldTerminal.velocityX, heldTerminal.velocityY);
  const terminalBlockedEnergyDelta = Math.abs(
    (terminalBlocked.uiMetrics?.playerEnergy ?? 0) - heldTerminal.energy,
  );
  if (
    terminalBlocked.uiMetrics?.localGameplayState?.pendingPrizeEffectApplications !== 1 ||
    terminalRetryImpairment?.terminalSuppressed < 1 ||
    terminalBlockedDistance > 1 ||
    terminalBlockedSpeedIncrease > 1 ||
    terminalBlockedEnergyDelta > 2
  ) {
    throw new Error(`held Shutdown terminal prematurely restored gameplay: ${JSON.stringify({
      heldTerminal,
      blocked: terminalBlocked.uiMetrics,
      terminalBlockedDistance,
      terminalBlockedSpeedIncrease,
      terminalBlockedEnergyDelta,
      terminalRetryImpairment,
      terminalUnionStandOff,
      terminalShooterStage,
    })}`);
  }
  if (
    !terminalShooterStage.brake.braked ||
    !terminalShooterStage.aim.oriented ||
    !(terminalShooterDistance > 96 && terminalShooterDistance < 220)
  ) {
    throw new Error(`phase-one EMP shooter did not stage during the held terminal: ${JSON.stringify({
      terminalUnionStandOff,
      terminalShooterStage,
      terminalShooterDistance,
    })}`);
  }
  const releasedTerminal = await actor.evaluate(
    "window.__zubspaceReleasePrizeEffectTerminalForTest?.() ?? false",
  );
  if (!releasedTerminal) {
    throw new Error("held Shutdown application terminal could not be released");
  }
  let shutdownApplicationBoundary = null;
  await waitUntil("exact Shutdown application receipt releases gameplay", async () => {
    const state = await actor.metricsState();
    const metrics = state.uiMetrics || {};
    const local = metrics.localGameplayState || {};
    if (
      local.pendingPrizeEffectApplications === 0 &&
      (local.prizeEffectApplicationReceiptsMatched ?? 0) > 0 &&
      Number.isInteger(local.pendingPrizeEffectApplicationSequence) === false
    ) {
      shutdownApplicationBoundary = {
        clientInputSeq: metrics.clientInputSeq,
        pending: local.pendingPrizeEffectApplications,
        pendingSequence: local.pendingPrizeEffectApplicationSequence,
        receiptsSeen: local.prizeEffectApplicationReceiptsSeen,
        receiptsMatched: local.prizeEffectApplicationReceiptsMatched,
        receiptsUnmatched: local.prizeEffectApplicationReceiptsUnmatched,
        snapshotTick: metrics.latestSnapshotServerTick,
        delayedAckTailDelta,
        delayedAckBrake,
        delayedAckImpairment,
        c2sTailBeforeMarker,
        c2sMarkerOvertookTail,
        heldTerminal,
        terminalBlockedDistance,
        terminalBlockedSpeedIncrease,
        terminalBlockedEnergyDelta,
        terminalRetryImpairment,
        terminalImpairment: await actor.evaluate(
          "window.__zubspacePrizeEffectImpairmentStateForTest?.() ?? null",
        ),
      };
      return true;
    }
    return false;
  }, 5_000);
  // The application-receipt impairment intentionally keeps this phase open
  // through a reliable retry while Shutdown preserves the victim's existing
  // momentum. Re-acquire its live position with ordinary keyboard movement;
  // the earlier fixed pickup coordinate is no longer a sound firing fixture.
  // No browser gameplay hook changes position or velocity here.
  let unionShooterReady = null;
  let unionVictimReady = null;
  let unionPreFireDistance = Number.POSITIVE_INFINITY;
  const unionShooterRestages = [];
  for (let attempt = 0; attempt < 1; attempt += 1) {
    [unionShooterReady, unionVictimReady] = await Promise.all([
      peer.metricsState(),
      actor.metricsState(),
    ]);
    unionPreFireDistance = Math.hypot(
      unionVictimReady.uiMetrics.playerX - unionShooterReady.uiMetrics.playerX,
      unionVictimReady.uiMetrics.playerY - unionShooterReady.uiMetrics.playerY,
    );
    if (unionPreFireDistance > 96 && unionPreFireDistance < 220) {
      unionShooterRestages.push({
        attempt,
        alreadyInRange: true,
        unionPreFireDistance,
      });
      break;
    }
    const liveStandOff = {
      // Stay just inside the proven bomb trajectory window. This minimizes
      // route distance without entering the exact 80px BombSafety exclusion.
      x: unionVictimReady.uiMetrics.playerX + 190,
      y: unionVictimReady.uiMetrics.playerY,
    };
    const route = await driveClientToWorldTarget(
      peer,
      liveStandOff,
      2_500,
      false,
      16,
      null,
      160,
    );
    const brake = await brakeClientVelocity(peer, 1_500);
    [unionShooterReady, unionVictimReady] = await Promise.all([
      peer.metricsState(),
      actor.metricsState(),
    ]);
    unionPreFireDistance = Math.hypot(
      unionVictimReady.uiMetrics.playerX - unionShooterReady.uiMetrics.playerX,
      unionVictimReady.uiMetrics.playerY - unionShooterReady.uiMetrics.playerY,
    );
    unionShooterRestages.push({ attempt, liveStandOff, route, brake, unionPreFireDistance });
    if (route.reached && brake.braked && unionPreFireDistance > 96 && unionPreFireDistance < 220) {
      break;
    }
  }
  if (
    !(unionPreFireDistance > 96 && unionPreFireDistance < 220)
  ) {
    throw new Error(`phase-one Shutdown+EMP shooter could not re-stage on the live victim: ${JSON.stringify({
      unionPreFireDistance,
      shooter: unionShooterReady.uiMetrics,
      victim: unionVictimReady.uiMetrics,
      unionShooterStandOff,
      unionShooterRoute,
      unionPeerBrake,
      unionShooterRestages,
    })}`);
  }
  const unionAim = await orientClientForVelocity(
    peer,
    unionVictimReady.uiMetrics.playerX - unionShooterReady.uiMetrics.playerX,
    unionVictimReady.uiMetrics.playerY - unionShooterReady.uiMetrics.playerY,
    4_000,
  );
  if (!unionAim.oriented) {
    throw new Error(`phase-one Shutdown+EMP shooter did not aim: ${JSON.stringify(unionAim)}`);
  }
  const unionDamageStart = parseServerDamageEvents(server.logs).length;
  const unionDamage = await fireUntilDamage({
    label: "phase-one Engine Shutdown and EMP union",
    shooter: peer,
    victim: actor,
    server,
    shooterId: peerPlayerId,
    victimId: playerId,
    sinceCount: unionDamageStart,
    requireLethal: false,
    timeoutMs: 12_000,
    forceBomb: true,
  });
  const shutdownAfterEmpTicks =
    (shutdownClaim.engineShutdownUntilTick - unionDamage.event.empUntilTick) & 0x7fff_ffff;
  if (
    !(unionDamage.event.empUntilTick > 0) ||
    !(shutdownAfterEmpTicks > 0 && shutdownAfterEmpTicks < 0x4000_0000)
  ) {
    throw new Error(`phase-one union fixture did not retain Shutdown beyond EMP: ${JSON.stringify({
      shutdownClaim,
      damage: unionDamage.event,
      shutdownAfterEmpTicks,
    })}`);
  }
  // This union is measured against two exact server deadlines. Full pageState
  // captures serialize tens of megabytes of retained history and can consume
  // the remaining authored window; use the bounded current metrics projection
  // throughout the time-critical segment.
  let unionActivated = null;
  await waitUntil("Shutdown+EMP union reaches private authority", async () => {
    const state = await actor.metricsState();
    const local = state.uiMetrics?.localGameplayState;
    if (
      (local?.engineShutdownTimer ?? 0) > 0 &&
      state.uiMetrics?.lastSelfDamageServerSeq === unionDamage.event.damageSeq &&
      Math.abs(
        (state.uiMetrics?.playerEnergy ?? 0) - unionDamage.event.energyAfter,
      ) <= 2
    ) {
      unionActivated = {
        timer: local.engineShutdownTimer,
        snapshotTick: state.uiMetrics.latestSnapshotServerTick,
        x: state.uiMetrics.playerX,
        y: state.uiMetrics.playerY,
        velocityX: state.uiMetrics.playerVelocityX,
        velocityY: state.uiMetrics.playerVelocityY,
        energy: state.uiMetrics.playerEnergy,
      };
      return true;
    }
    return false;
  }, 5_000);
  await actor.hold("ArrowUp", 350);
  const unionBlocked = await actor.metricsState();
  const unionBlockedMovementDistance = Math.hypot(
    (unionBlocked.uiMetrics?.playerX ?? 0) - unionActivated.x,
    (unionBlocked.uiMetrics?.playerY ?? 0) - unionActivated.y,
  );
  const unionBlockedSpeed = Math.hypot(
    unionBlocked.uiMetrics?.playerVelocityX ?? 0,
    unionBlocked.uiMetrics?.playerVelocityY ?? 0,
  );
  const unionActivatedSpeed = Math.hypot(
    unionActivated.velocityX,
    unionActivated.velocityY,
  );
  const unionBlockedSpeedIncrease = unionBlockedSpeed - unionActivatedSpeed;
  const unionBlockedEnergyDelta = Math.abs(
    (unionBlocked.uiMetrics?.playerEnergy ?? 0) - unionActivated.energy,
  );
  if (
    !(unionBlocked.uiMetrics?.localGameplayState?.engineShutdownTimer > 0) ||
    unionBlockedSpeedIncrease > 1 ||
    unionBlockedEnergyDelta > 2
  ) {
    throw new Error(`Shutdown+EMP union accepted thrust/recharge while both sources were active: ${JSON.stringify({
      unionActivated,
      blocked: unionBlocked.uiMetrics,
      unionBlockedMovementDistance,
      unionActivatedSpeed,
      unionBlockedSpeed,
      unionBlockedSpeedIncrease,
      unionBlockedEnergyDelta,
    })}`);
  }

  let afterEmpStillShutdown = null;
  await waitUntil("prize Shutdown remains active after EMP expires", async () => {
    const state = await actor.metricsState();
    const snapshotTick = state.uiMetrics?.latestSnapshotServerTick;
    const local = state.uiMetrics?.localGameplayState;
    const pastEmp = Number.isInteger(snapshotTick)
      ? (snapshotTick - unionDamage.event.empUntilTick) & 0x7fff_ffff
      : 0x7fff_ffff;
    const beforeShutdown = Number.isInteger(snapshotTick)
      ? (shutdownClaim.engineShutdownUntilTick - snapshotTick) & 0x7fff_ffff
      : 0x7fff_ffff;
    if (
      pastEmp < 0x4000_0000 &&
      beforeShutdown > 0 && beforeShutdown < 0x4000_0000 &&
      (local?.engineShutdownTimer ?? 0) > 0
    ) {
      afterEmpStillShutdown = {
        snapshotTick,
        timer: local.engineShutdownTimer,
        pastEmp,
        beforeShutdown,
        x: state.uiMetrics.playerX,
        y: state.uiMetrics.playerY,
        velocityX: state.uiMetrics.playerVelocityX,
        velocityY: state.uiMetrics.playerVelocityY,
        energy: state.uiMetrics.playerEnergy,
      };
      return true;
    }
    return false;
  }, 8_000);
  await actor.hold("ArrowUp", 350);
  const afterEmpBlocked = await actor.metricsState();
  const afterEmpBlockedMovementDistance = Math.hypot(
    (afterEmpBlocked.uiMetrics?.playerX ?? 0) - afterEmpStillShutdown.x,
    (afterEmpBlocked.uiMetrics?.playerY ?? 0) - afterEmpStillShutdown.y,
  );
  const afterEmpBlockedSpeed = Math.hypot(
    afterEmpBlocked.uiMetrics?.playerVelocityX ?? 0,
    afterEmpBlocked.uiMetrics?.playerVelocityY ?? 0,
  );
  const afterEmpActiveSpeed = Math.hypot(
    afterEmpStillShutdown.velocityX,
    afterEmpStillShutdown.velocityY,
  );
  const afterEmpBlockedSpeedIncrease = afterEmpBlockedSpeed - afterEmpActiveSpeed;
  const afterEmpBlockedEnergyDelta = Math.abs(
    (afterEmpBlocked.uiMetrics?.playerEnergy ?? 0) - afterEmpStillShutdown.energy,
  );
  const afterEmpBlockedSnapshotTick = afterEmpBlocked.uiMetrics?.latestSnapshotServerTick;
  const afterEmpBeforeShutdown = Number.isInteger(afterEmpBlockedSnapshotTick)
    ? (shutdownClaim.engineShutdownUntilTick - afterEmpBlockedSnapshotTick) & 0x7fff_ffff
    : 0x7fff_ffff;
  if (
    !(afterEmpBeforeShutdown > 0 && afterEmpBeforeShutdown < 0x4000_0000) ||
    afterEmpBlockedSpeedIncrease > 1 ||
    // Recharge is gated on EMP alone: the reference game's `ShipController:244-259` skips the `energy += recharge/10*dt`
    // branch only while `emped_time > 0`, and the EngineShutdown *prize* touches `shutdown_end_tick` and nothing
    // else. So with the EMP expired and the prize still running, thrust must stay blocked (above) and the energy
    // must actually RISE. This used to assert a frozen energy, which re-encoded the union behaviour the reference
    // does not have - the server was fixed to match the reference (`e112c5c`) and this lane then failed
    // (observed: `beforeEnergy 1034`, delta well over the old 2-energy tolerance).
    afterEmpBlockedEnergyDelta <= 2
  ) {
    throw new Error(`prize Shutdown accepted thrust, or did not recharge, after EMP expired first: ${JSON.stringify({
      afterEmpStillShutdown,
      blocked: afterEmpBlocked.uiMetrics,
      afterEmpBlockedMovementDistance,
      afterEmpActiveSpeed,
      afterEmpBlockedSpeed,
      afterEmpBlockedSpeedIncrease,
      afterEmpBlockedEnergyDelta,
      afterEmpBlockedSnapshotTick,
      afterEmpBeforeShutdown,
    })}`);
  }
  let unionExpired = null;
  await waitUntil("Shutdown+EMP union expires only at the later prize deadline", async () => {
    const state = await actor.metricsState();
    const snapshotTick = state.uiMetrics?.latestSnapshotServerTick;
    const pastShutdown = Number.isInteger(snapshotTick)
      ? (snapshotTick - shutdownClaim.engineShutdownUntilTick) & 0x7fff_ffff
      : 0x7fff_ffff;
    if (
      state.uiMetrics?.localGameplayState?.engineShutdownTimer === 0 &&
      pastShutdown < 0x4000_0000
    ) {
      unionExpired = {
        snapshotTick,
        deadline: shutdownClaim.engineShutdownUntilTick,
        pastShutdown,
        timer: state.uiMetrics.localGameplayState.engineShutdownTimer,
      };
      return true;
    }
    return false;
  }, 25_000);
  const replaceLifeViaShipKey = async (client, targetShip, previousLife, label) => {
    await client.press("Escape", 60);
    await client.press(String(targetShip + 1), 60);
    let replacement = null;
    await waitUntil(label, async () => {
      const state = await client.pageState();
      const life = state.uiMetrics?.playerLifeSeq;
      const local = state.uiMetrics?.localGameplayState;
      if (Number.isFinite(life) && life !== previousLife && local?.prizeAuthorityLifeSeq === life) {
        replacement = {
          lifeSeq: life,
          prizeLifeSeq: local.prizeAuthorityLifeSeq,
          revision: local.prizeAuthorityRevision,
          x: state.uiMetrics.playerX,
          y: state.uiMetrics.playerY,
        };
        return true;
      }
      return false;
    }, 8_000);
    return replacement;
  };
  const actorBeforeRestore = await actor.pageState();
  const actorIntermediateShip = actorCombatShip === 0 ? 1 : 0;
  const actorIntermediateRestore = await replaceLifeViaShipKey(
    actor,
    actorIntermediateShip,
    actorBeforeRestore.uiMetrics?.playerLifeSeq,
    "union victim enters an ordinary intermediate lifecycle",
  );
  const actorFinalRestore = await replaceLifeViaShipKey(
    actor,
    actorCombatShip,
    actorIntermediateRestore.lifeSeq,
    "union victim restores the combat lifecycle",
  );
  const peerBeforeFinalRestore = await peer.pageState();
  const peerFinalRestore = await replaceLifeViaShipKey(
    peer,
    peerCombatShip,
    peerBeforeFinalRestore.uiMetrics?.playerLifeSeq,
    "union shooter restores the combat lifecycle",
  );
  await rendezvousPlayers(actor, peer, server);
  await waitForPlayersReady(actor, peer);
  const postUnionRestore = {
    actorIntermediate: actorIntermediateRestore,
    actor: actorFinalRestore,
    peer: peerFinalRestore,
  };
  const claimsDuringGate = parsePrizeClaims(server, logOffset)
    .filter((candidate) => candidate.playerId === playerId);
  return {
    claim,
    shieldSpawn,
    shieldPreparation,
    claimsDuringGate,
    routes,
    collectorBrake,
    teamAlignment,
    shared,
    sharedReconciled,
    opposingCutover,
    shieldRoute,
    shieldShooterBrake,
    shieldAim,
    shieldedDamage: {
      ...shieldedDamage.event,
      shieldFraction,
      shieldProtectedDamage,
      expectedShieldedDamage,
    },
    activated,
    reset,
    rocketEncounter,
    rocket,
    repel,
    combatRestore,
    emp: {
      encounter: empEncounter,
      route: empRoute,
      shooterBrake: empShooterBrake,
      aim: empAim,
      damage: empDamage.event,
      activated: empActivated,
      idleMovementDistance,
      idleSpeed,
      idleEnergyDelta,
      expired: empExpired,
    },
    shutdownEmpUnion: {
      preparation: shutdownPreparation,
      shutdownSpawn,
      shutdownClaim,
      shutdownRoute,
      shutdownActivated,
      shutdownApplicationBoundary,
      shooterCentering: unionShooterCentering,
      shooterRestore: unionShooterRestore,
      shooterRoute: unionShooterRoute,
      shooterBeforeClaim: unionShooterBeforeClaim.uiMetrics,
      shooterBrake: unionPeerBrake,
      preAim: unionPreAim,
      delayedAckBrake,
      terminalUnionStandOff,
      terminalShooterStage,
      terminalShooterDistance,
      liveShooterRestages: unionShooterRestages,
      preFireDistance: unionPreFireDistance,
      aim: unionAim,
      damage: unionDamage.event,
      shutdownAfterEmpTicks,
      activated: unionActivated,
      blockedMovementDistance: unionBlockedMovementDistance,
      activatedSpeed: unionActivatedSpeed,
      blockedSpeed: unionBlockedSpeed,
      blockedSpeedIncrease: unionBlockedSpeedIncrease,
      blockedEnergyDelta: unionBlockedEnergyDelta,
      afterEmpStillShutdown,
      afterEmpBlockedMovementDistance,
      afterEmpActiveSpeed,
      afterEmpBlockedSpeed,
      afterEmpBlockedSpeedIncrease,
      afterEmpBlockedEnergyDelta,
      afterEmpBlockedSnapshotTick,
      afterEmpBeforeShutdown,
      expired: unionExpired,
      postUnionRestore,
    },
    damageLoss: {
      ...capturedDamageLoss,
      duplicateReconciliation: damageLossDuplicateReconciliation,
      shooterRoute: damageLossShooterRoute,
      shooterBrake: damageLossShooterBrake,
      aim: damageLossAim,
      probe: damageLossProbe?.event || null,
    },
  };
}

function signedSoakOrientationDelta(current, target) {
  return ((target - current + 0.5) % 1 + 1) % 1 - 0.5;
}

async function pulseClientOnFreshInputStep(client, key, holdMs, options) {
  if (typeof client.pulseOnFreshInputStep === "function") {
    // Returned, not dropped. The browser-side helper answers `false` when the logical input sequencer is
    // lifecycle-paused (death, respawn, or an unresolved authority boundary) and the pulse therefore reached no
    // ship. Discarding that answer is what let a stalled route report 1054 thrusts it never delivered.
    return await client.pulseOnFreshInputStep(key, holdMs, options);
  }
  await client.press(key, holdMs);
  return true;
}

function arrivalControlDistance(distance, settle, arrivalTolerance) {
  return Math.max(0, distance - (settle ? arrivalTolerance : 4));
}

function authoredDoorClosedForCurrentAndNext(metrics, tileId) {
  const local = metrics?.localGameplayState || {};
  const doorIndex = tileId - 162;
  if (
    local.doorClockFresh !== true ||
    !Number.isInteger(local.doorSeed) ||
    !Number.isInteger(local.doorSeedNext) ||
    doorIndex < 0 ||
    doorIndex > 7
  ) {
    return false;
  }
  const mask = 1 << doorIndex;
  return (local.doorSeed & mask) !== 0 && (local.doorSeedNext & mask) !== 0;
}

function assertExactChaosWarbirdMovementPrizeSequence(weakeningClaim, restoringClaim) {
  const protocolId = Math.abs(weakeningClaim.prizeId);
  const expected = chaosWarbirdMovementPrizeValues.get(protocolId);
  if (!expected || restoringClaim.prizeId !== protocolId) {
    throw new Error(`natural movement sequence used an unexpected prize pair: ${JSON.stringify({
      weakeningClaim,
      restoringClaim,
    })}`);
  }
  const tolerance = protocolId === 3 ? 0.000_01 : 0.01;
  const checks = [
    ["negative before", weakeningClaim.movementTuningBefore, expected.cap],
    ["negative after", weakeningClaim.movementTuningAfter, expected.base],
    ["positive before", restoringClaim.movementTuningBefore, expected.base],
    ["positive after", restoringClaim.movementTuningAfter, expected.cap],
  ];
  for (const [label, actual, exact] of checks) {
    if (!Number.isFinite(actual) || Math.abs(actual - exact) > tolerance) {
      throw new Error(`Chaos Warbird ${label} was ${actual}, expected ${exact}: ${JSON.stringify({
        protocolId,
        weakeningClaim,
        restoringClaim,
      })}`);
    }
  }
  return { protocolId, ...expected, tolerance };
}

// Magnitude of the pilot's heading error against the bearing to the target, in turns.
//
// The engine's orientation is `0 = up (-y)` increasing clockwise
// (`orientation_to_heading` -> `(sin, -cos)`), so the bearing to a target is
// `atan2(dx, -dy)`. The x-right/CCW `atan2(dy, dx)` form this file
// used earlier sits a quarter turn from the engine convention and printed a
// ~92-degree "heading error" whenever the pilot was in fact aligned on a +x
// leg: the retained 641 px Chaos leg reported `attemptHeadingErrorMeanTurns
// 0.257` while the pilot's real error was under 3 degrees. That was the one
// diagnostic the route-leg item asked the next occurrence to be read with, so
// it has to use the engine's convention.
//
// Magnitude only: the sign depends on the engine's turn direction and this must
// not be read as a direction. The driver itself aims at its acceleration vector
// rather than the target bearing, so this measures "is the nose on the target",
// not "what turn did the controller command".
export function routeHeadingErrorTurns(metrics, target) {
  const dx = target.x - metrics.playerX;
  const dy = target.y - metrics.playerY;
  if (!(Math.hypot(dx, dy) > 1)) return 0;
  const bearing = ((Math.atan2(dx, -dy) / (Math.PI * 2)) + 1) % 1;
  const orientation = metrics.playerOrientation ?? 0;
  return Math.abs((((bearing - orientation) % 1) + 1.5) % 1 - 0.5);
}

async function driveClientToWorldTarget(
  client,
  target,
  timeoutMs = 12_000,
  settle = false,
  arrivalTolerance = 6,
  completion = null,
  maximumDesiredSpeed = 42,
  desiredSpeedScale = 0.25,
  progressTimeoutMs = 5_000,
  maximumArrivalSpeed = 3.5,
) {
  const startedAt = Date.now();
  // A diagnostic affordance, not a behaviour change. The route-attempt shape instrument can only report when an
  // attempt actually expires, and on a healthy tree none of them do - so it has never fired in a real lane and the
  // mechanism behind the historic zero-progress timeout is still unobserved. This lets an operator shorten one
  // attempt deliberately to read the shape of a leg that is not closing, instead of guessing from one old failure.
  // A forced expiry is not the same event as a natural 33 s one; read it as a specimen, not as a gate result.
  const attemptBudgetMs = Number(process.env.ZUBSPACE_E2E_ROUTE_ATTEMPT_MS || timeoutMs);
  let bestDistance = Number.POSITIVE_INFINITY;
  let progressDistance = Number.POSITIVE_INFINITY;
  let bestNearTargetSpeed = Number.POSITIVE_INFINITY;
  let bestSettlingVelocityError = Number.POSITIVE_INFINITY;
  let lastProgressAt = startedAt;
  let pulses = 0;
  // A route can be physically blocked while every counter looks healthy: the
  // retained Chrome/Chaos mild route held thrust (`inputBits: 1`), reported
  // velocity (-1.5, 6.5) px/s and `selfCorrectionCount: 0` - no authority
  // snap-back - while creeping about 1 px per 12 render frames for 410 pulses
  // against blocking terrain. Because that creep still beat the 3 px progress
  // floor every <15 s, it never tripped the no-progress watchdog and burned the
  // whole 33 s budget as a plain `route timeout`, which the caller treated as
  // an unre-planable fault. Track thrust-only displacement so a blocked lane is
  // reported as what it is, quickly.
  let lastAction = "none";
  let thrustAnchorAt = 0;
  let thrustAnchorX = 0;
  // Accumulators for the blocked-route report. The check itself is displacement-only, so on its own it cannot
  // tell a ship held still from a ship travelling back and forth inside three pixels - and the first
  // reproduction reported a non-zero speed while reporting almost no displacement, which is exactly the case
  // it cannot classify. These separate "held still" (near-zero speed every sample) from "cancelling" (real
  // speed, real path length, net displacement near zero).
  let thrustPrevX = 0;
  let thrustPrevY = 0;
  let thrustPathPx = 0;
  let thrustMaxSpeed = 0;
  let thrustStillSamples = 0;
  let thrustSamples = 0;
  // Whole-attempt shape, independent of the per-thrust-window accumulators. A leg that reported 640 px
  // remaining at the start and 645 px at the timeout had made no net progress for the entire budget while
  // carrying velocity - which is either a pilot oscillating over a long span or one pinned and pushed. Path
  // length separates them without a second run: large path against near-zero net is oscillation, near-zero
  // path is held still.
  let attemptStartX = null;
  let attemptStartY = null;
  let attemptPrevX = null;
  let attemptPrevY = null;
  let attemptPathPx = 0;
  let attemptMaxSpeed = 0;
  let attemptSamples = 0;
  // Why does it reverse? Path length against net displacement says the pilot travels and gets nowhere; it cannot
  // say whether the pilot is fighting its own corrections or the terrain. These read the control loop instead of
  // its aftermath: the radial velocity component towards the target and how often its sign flips (a reversal is a
  // correction that overshot), how much of the window was spent closing versus opening, the magnitude of the
  // heading error - magnitude only, because the sign depends on the engine's turn convention and this must not be
  // read as a direction - and which action the driver last issued.
  let attemptReversals = 0;
  let attemptClosingSamples = 0;
  let attemptOpeningSamples = 0;
  let attemptHeadingErrorMaxTurns = 0;
  let attemptHeadingErrorSumTurns = 0;
  let attemptRadialSign = 0;
  const attemptActionSamples = {};
  const attemptTrace = [];
  let attemptTraceAt = 0;
  const attemptShapeReport = (m) => ({
    attemptPathPx: Number(attemptPathPx.toFixed(1)),
    attemptNetDisplacementPx: attemptStartX === null || m === undefined ? null
      : Number(Math.hypot((m.playerX ?? 0) - attemptStartX, (m.playerY ?? 0) - attemptStartY).toFixed(1)),
    attemptMaxSpeed: Number(attemptMaxSpeed.toFixed(2)),
    bestSettlingVelocityError: Number.isFinite(bestSettlingVelocityError)
      ? Number(bestSettlingVelocityError.toFixed(3)) : null,
    attemptSamples,
    attemptReversals,
    attemptClosingSamples,
    attemptOpeningSamples,
    attemptHeadingErrorMaxTurns: Number(attemptHeadingErrorMaxTurns.toFixed(3)),
    attemptHeadingErrorMeanTurns: attemptSamples > 0
      ? Number((attemptHeadingErrorSumTurns / attemptSamples).toFixed(3)) : null,
    attemptActionSamples,
    // The client's own account of itself, not just the shape it flew. The retained Chaos leg 7 froze its
    // own position while reporting a live 62.2 px/s closing speed, with `inputBits 0`, 1174 successful pulses and
    // `serverPlayerAlive null` - the signature of a *paused or lifecycle-pending* client, which is a different defect
    // from a physics freeze, and the shape alone cannot tell them apart. Carrying the whole local state costs nothing
    // that matters on a rare failure and decides the question on the next one.
    attemptLocalGameplayState: m?.localGameplayState ?? null,
    attemptSelfCorrectionCount: m?.selfCorrectionCount ?? null,
    attemptServerPlayerAlive: m?.serverPlayerAlive ?? null,
    attemptTrace,
  });
  let thrustAnchorY = 0;
  // A pulse the browser-side helper refused reached no ship, so it must not be counted, and the loop must wait
  // for the lifecycle to resolve instead of spinning. See `pulseClientOnFreshInputStep`. The retained 641 px
  // Chaos leg is what this exists for: `attemptActionSamples {thrust: 1054}` against `attemptPathPx 38.8` in an
  // authored corridor that the harness's own decoder reports as 82-of-82 samples open.
  let undeliveredSince = 0;
  // How long a persistent refusal may last before the attempt names the condition instead of burning the budget.
  // A legitimate respawn resolves in about two seconds; overridable so the failure branch can be exercised
  // without a five-second test.
  const undeliveredBudgetMs = Number(process.env.ZUBSPACE_E2E_ROUTE_UNDELIVERED_MS || 5_000);
  const noteUndeliveredPulse = async () => {
    lastAction = "undelivered";
    if (undeliveredSince === 0) undeliveredSince = Date.now();
    if (Date.now() - undeliveredSince <= undeliveredBudgetMs) {
      await sleep(50);
      return null;
    }
    const stalled = (await client.metricsState()).uiMetrics || {};
    const local = stalled.localGameplayState || {};
    return {
      reached: false,
      reason: "input not admitted",
      ...attemptShapeReport(stalled),
      bestDistance,
      pulses,
      target,
      undeliveredMs: Date.now() - undeliveredSince,
      playerAlive: stalled.playerAlive ?? null,
      serverPlayerAlive: stalled.serverPlayerAlive ?? null,
      playerRespawnTimer: stalled.playerRespawnTimer ?? null,
      playerLifeSeq: stalled.playerLifeSeq ?? null,
      authorityPhase: local.authorityPhase ?? null,
      authorityScopeAlive: local.authorityScopeAlive ?? null,
      authorityTransitionKind: local.authorityTransitionKind ?? null,
    };
  };

  while (Date.now() - startedAt < attemptBudgetMs) {
    const completed = completion ? await completion() : null;
    if (completed) {
      return { reached: true, completed, pulses, target };
    }
    const state = await client.metricsState();
    const metrics = state.uiMetrics || {};
    const values = [
      metrics.playerX,
      metrics.playerY,
      metrics.playerVelocityX,
      metrics.playerVelocityY,
      metrics.playerOrientation,
    ];
    if (!values.every(Number.isFinite)) {
      return { reached: false, reason: "non-finite movement metrics", target, metrics };
    }

    const dx = target.x - metrics.playerX;
    const dy = target.y - metrics.playerY;
    const distance = Math.hypot(dx, dy);
    const speed = Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY);
    attemptSamples += 1;
    if (attemptStartX === null) {
      attemptStartX = metrics.playerX;
      attemptStartY = metrics.playerY;
    } else {
      attemptPathPx += Math.hypot(metrics.playerX - attemptPrevX, metrics.playerY - attemptPrevY);
    }
    attemptPrevX = metrics.playerX;
    attemptPrevY = metrics.playerY;
    if (speed > attemptMaxSpeed) attemptMaxSpeed = speed;
    const radial = distance > 1
      ? ((dx * (metrics.playerVelocityX ?? 0)) + (dy * (metrics.playerVelocityY ?? 0))) / distance : 0;
    // Deadband at 2 px/s: a ship that is merely wobbling at walking pace is not closing or opening on the target,
    // and counting that as a reversal would report control chatter that is not happening.
    if (radial > 2) attemptClosingSamples += 1;
    else if (radial < -2) attemptOpeningSamples += 1;
    if (radial > 2 && attemptRadialSign < 0) attemptReversals += 1;
    else if (radial < -2 && attemptRadialSign > 0) attemptReversals += 1;
    if (radial > 2 || radial < -2) attemptRadialSign = Math.sign(radial);
    const headingErrorTurns = routeHeadingErrorTurns(metrics, target);
    attemptHeadingErrorSumTurns += headingErrorTurns;
    if (headingErrorTurns > attemptHeadingErrorMaxTurns) attemptHeadingErrorMaxTurns = headingErrorTurns;
    attemptActionSamples[lastAction] = (attemptActionSamples[lastAction] ?? 0) + 1;
    const traceNow = Date.now();
    if (traceNow - attemptTraceAt >= 1000) {
      attemptTraceAt = traceNow;
      attemptTrace.push({
        ms: traceNow - startedAt, distancePx: Number(distance.toFixed(1)),
        radialPxPerS: Number(radial.toFixed(1)), speed: Number(speed.toFixed(1)),
        headingErrorTurns: Number(headingErrorTurns.toFixed(3)), action: lastAction,
      });
    }
    const distanceAfterOverlap = arrivalControlDistance(
      distance,
      settle,
      arrivalTolerance,
    );
    // Keep ordinary collectors deliberately slow while allowing focused
    // moving-target fixtures to opt into the full physical braking curve.
    // The square-root taper still brakes inside the arrival radius, and every
    // caller continues to use genuine keyboard steering/thrust.
    const desiredSpeed = Math.min(
      maximumDesiredSpeed,
      Math.sqrt(2 * 180 * distanceAfterOverlap) * desiredSpeedScale,
    );
    const desiredVelocityX = distance > 0.01 ? dx / distance * desiredSpeed : 0;
    const desiredVelocityY = distance > 0.01 ? dy / distance * desiredSpeed : 0;
    const accelX = desiredVelocityX - metrics.playerVelocityX;
    const accelY = desiredVelocityY - metrics.playerVelocityY;
    const accelMagnitude = Math.hypot(accelX, accelY);
    // A fast pilot can first move away while real-key thrust brakes its
    // inherited velocity. Convergence toward the requested velocity is real
    // progress even before distance beats its initial best. Constant speed/
    // position, repeated oscillation and the original leg deadline still fail.
    const velocityProgress = settle && distance > 48 &&
      accelMagnitude + 1 < bestSettlingVelocityError;
    if (settle && distance > 48) {
      bestSettlingVelocityError = Math.min(bestSettlingVelocityError, accelMagnitude);
    }
    const physicallyStopped = brakeVelocityDisposition(metrics, maximumArrivalSpeed).braked;
    if (distance <= arrivalTolerance && (!settle || physicallyStopped)) {
      return { reached: true, distance, speed, pulses, target };
    }
    bestDistance = Math.min(bestDistance, distance);
    if (distance + 3 < progressDistance) {
      progressDistance = distance;
      lastProgressAt = Date.now();
    } else if (velocityProgress) {
      // After braking away from the target, closing from the new turning
      // point must count too; requiring a return past the original closest
      // point first would classify several seconds of genuine approach as
      // stationary. Only a new best velocity convergence reseats this mark.
      progressDistance = distance;
      lastProgressAt = Date.now();
    } else if (settle && distance <= 48 && speed + 1 < bestNearTargetSpeed) {
      // Once the ship has reached the target neighborhood, shedding velocity
      // is real route progress too. Counting only the lowest distance makes a
      // deliberate braking pass look stuck as soon as it crosses the target.
      bestNearTargetSpeed = speed;
      lastProgressAt = Date.now();
    } else if (
      Date.now() - lastProgressAt > progressTimeoutMs &&
      // A settling controller may legitimately hold its position, so for a settle leg a stalled *distance* alone is not
      // a defect - but reported speed with no approach is: at 62 px/s a simulating ship closes 868 px in 14 s. The
      // retained Chaos leg 7 held `distancePx 868.1` and `speed 62.2` unchanged for its whole 59 s budget,
      // and because a settle leg skips the blocked-route watchdog below, nothing named the shape before the budget
      // ran out. Name it instead.
      (!settle || speed > 20)
    ) {
      return {
        reached: false,
        reason: settle ? "reported speed without progress" : "no route progress",
        // Whole-attempt shape and reversal instrumentation. See the declaration comments.
        ...attemptShapeReport(metrics),
        distance,
        bestDistance,
        bestNearTargetSpeed,
        speed,
        pulses,
        target,
      };
    }
    // Thrust without displacement is a blocked lane, not a slow one. Measure it
    // only while the previous action was genuine thrust: a ship turning in
    // place or braking is legitimately stationary and must not trip this.
    if (!settle && lastAction === "thrust") {
      if (thrustAnchorAt !== 0) {
        thrustSamples += 1;
        thrustPathPx += Math.hypot(metrics.playerX - thrustPrevX, metrics.playerY - thrustPrevY);
        if (speed > thrustMaxSpeed) thrustMaxSpeed = speed;
        if (speed < 0.5) thrustStillSamples += 1;
      }
      thrustPrevX = metrics.playerX;
      thrustPrevY = metrics.playerY;
      if (thrustAnchorAt === 0) {
        thrustAnchorAt = Date.now();
        thrustAnchorX = metrics.playerX;
        thrustAnchorY = metrics.playerY;
      } else if (Date.now() - thrustAnchorAt >= 2_500) {
        // Second, independent source, already carried by the same poll. metricsState() returns the page's
        // per-input trace, so one snapshot answers the question the displacement numbers cannot: was the
        // pilot's position constant in every source, or only in the one this loop reads? Constant there too
        // means the ship really was held still and the lane is genuinely blocked. Moving there while the
        // position fields above sit still means this loop's snapshot is stale, which would make every route
        // check built on it capable of misfiring. Input sequence and server tick are included so a stalled
        // pipeline is distinguishable from a moving ship that merely was not closing on the target.
        // Filter on sampledAt when present: entries may be recorded with a caller-supplied eventAt, so the at
        // field is not guaranteed to be the harness wall clock that thrustAnchorAt is. traceSamplesTotal
        // distinguishes the two kinds of zero - nothing admitted in the window, versus no entries to compare.
        const traceClock = (entry) => Number(entry.sampledAt ?? entry.at);
        const traceWindow = (state.inputTrace || []).filter((entry) => traceClock(entry) >= thrustAnchorAt);
        const tracePositions = traceWindow
          .map((entry) => ({ x: Number(entry.playerX), y: Number(entry.playerY) }))
          .filter((p) => Number.isFinite(p.x) && Number.isFinite(p.y));
        let tracePositionSpreadPx = null;
        if (tracePositions.length >= 2) {
          let spread = 0;
          for (const point of tracePositions) {
            spread = Math.max(spread, Math.hypot(point.x - tracePositions[0].x, point.y - tracePositions[0].y));
          }
          tracePositionSpreadPx = Number(spread.toFixed(2));
        }
        const traceSeqs = traceWindow.map((entry) => Number(entry.clientInputSeq)).filter(Number.isFinite);
        const traceTicks = traceWindow.map((entry) => Number(entry.latestServerTick)).filter(Number.isFinite);
        const thrustWindowMs = Date.now() - thrustAnchorAt;
        const thrustDisplacement = Math.hypot(
          metrics.playerX - thrustAnchorX,
          metrics.playerY - thrustAnchorY,
        );
        if (thrustDisplacement < 3) {
          return {
            reached: false,
            reason: "route blocked",
            distance,
            bestDistance,
            speed,
            pulses,
            thrustWindowMs,
            // Read these together. thrustNetDisplacementPx near zero with thrustPathPx near zero and
            // thrustStillSamples close to thrustSamples means the ship was pinned. Net displacement near zero
            // with a real thrustPathPx means it was moving and being sent back - which is what a repeatedly
            // stamped velocity cancellation looks like - and net zero with a real thrustMaxSpeed but a small
            // path length means the samples straddled a stop.
            thrustPathPx: Number(thrustPathPx.toFixed(1)),
            thrustNetDisplacementPx: Number(thrustDisplacement.toFixed(1)),
            thrustMaxSpeed: Number(thrustMaxSpeed.toFixed(2)),
            thrustStillSamples,
            thrustSamples,
            // Read tracePositionSpreadPx against thrustPathPx. Both near zero: the pilot was genuinely held
            // still and the route is really blocked. Spread well above zero while thrustPathPx is zero: the
            // position fields this loop polls went stale and the block report is a harness artefact.
            // What the game believed was held, at the blocked sample and across the window. The bits live on
            // localRenderPrediction (src/game/render/hud/metrics.rs:71), not on localGameplayState. Trace
            // records carry their own admitted inputBits, so counting the ones with thrust, reverse, rotate or
            // afterburner set separates a pilot that was still being driven from one that was coasting. The
            // constants are src/gameplay/player_input.rs:44-48: thrust 1, reverse 2, rotate left 4, rotate
            // right 8, afterburner held 16; 0x3f keeps the held-action bits out of the movement bits.
            inputBitsAtBlock: metrics.localRenderPrediction?.inputBits ?? null,
            traceEntriesWithMovementBits: traceWindow.filter(
              (entry) => Number.isFinite(Number(entry.inputBits)) && (Number(entry.inputBits) & 0x3f) !== 0,
            ).length,
            traceMovementBitsSamples: traceWindow.filter((entry) => Number.isFinite(Number(entry.inputBits))).length,
            traceSamplesTotal: (state.inputTrace || []).length,
            traceSamplesInWindow: traceWindow.length,
            tracePositionSpreadPx,
            traceInputSeqFrom: traceSeqs.length > 0 ? traceSeqs[0] : null,
            traceInputSeqTo: traceSeqs.length > 0 ? traceSeqs[traceSeqs.length - 1] : null,
            traceServerTickFrom: traceTicks.length > 0 ? traceTicks[0] : null,
            traceServerTickTo: traceTicks.length > 0 ? traceTicks[traceTicks.length - 1] : null,
            target,
          };
        }
        thrustAnchorAt = Date.now();
        thrustAnchorX = metrics.playerX;
        thrustAnchorY = metrics.playerY;
        thrustPathPx = 0;
        thrustMaxSpeed = 0;
        thrustStillSamples = 0;
        thrustSamples = 0;
      }
    } else {
      thrustAnchorAt = 0;
    }

    // Feedback-control a desired velocity rather than holding thrust on one
    // initial bearing. The acceleration request naturally points against the
    // current velocity while braking, so overshoot causes a normal keyboard
    // turn-and-thrust recovery instead of an unbounded drift.
    // Once a settling controller enters its accepted arrival circle, request
    // zero velocity instead of continuing to accelerate toward the exact
    // center. The prior nonzero request could pass within two pixels at low
    // speed, accelerate back out, and repeat until timeout despite owning a
    // perfectly valid stopped position for the fixture.
    // A ship already near the desired speed while receding from the target cannot
    // be braked by a nose it has not turned yet: aiming at the instantaneous
    // acceleration error spends the whole budget rotating toward a braking vector
    // while the ship coasts away at unchanged speed (retained Chaos route: 20 s,
    // five thrust pulses, closest approach 396 px). Outside the arrival circle and
    // not settling, point the nose at the target itself.
    const receding = !settle && distance > arrivalTolerance &&
      metrics.playerVelocityX * desiredVelocityX + metrics.playerVelocityY * desiredVelocityY < 0;
    const aimX = receding ? desiredVelocityX : accelX;
    const aimY = receding ? desiredVelocityY : accelY;
    // A stopped pilot just outside acceptance still needs fine approach.
    // Otherwise the tapered request can stay below the acceleration dead zone
    // forever (retained Extreme staging: 7.785 px away, 7.75 px acceptance).
    const stoppedOutsideArrival = physicallyStopped && distance > arrivalTolerance;
    // A caller can require a tighter rest window than the ordinary3.5px/s
    // route. Keep braking inside its arrival circle instead of reporting
    // reached or idling forever in the gap between the two thresholds.
    const arrivedButMoving = settle && distance <= arrivalTolerance && !physicallyStopped;
    if (accelMagnitude < 3 && !stoppedOutsideArrival && !arrivedButMoving) {
      lastAction = "idle";
      await sleep(35);
      continue;
    }

    const desiredOrientation = (
      Math.atan2(aimX, -aimY) / (Math.PI * 2) + 1
    ) % 1;
    const forwardTurn = signedSoakOrientationDelta(metrics.playerOrientation, desiredOrientation);
    const reverseTurn = signedSoakOrientationDelta(metrics.playerOrientation, (desiredOrientation + 0.5) % 1);
    // A settling route may brake with the real reverse-thrust key. Turning
    // half a revolution before every deceleration lets a100px/s pilot coast
    // through the4px waypoint repeatedly. Forward and reverse have the same
    // authored acceleration; choose the shorter turn to the requested impulse.
    // Nonsettling collectors retain their forward-only approach policy.
    const thrustKey = settle && Math.abs(reverseTurn) < Math.abs(forwardTurn) ? "s" : "w";
    const turn = thrustKey === "s" ? reverseTurn : forwardTurn;
    if (Math.abs(turn) > 0.015) {
      // Chaos Warbird rotation is roughly 0.625 turns/s. Scale the real-key
      // pulse to that physical rate so the feedback controller can turn and
      // brake a ship that leaves the first green with residual velocity;
      // the old 100ms/turn multiplier under-rotated by about 16x and could
      // time out while drifting away from a nearby second green.
      lastAction = "turn";
      const turnDelivered = await pulseClientOnFreshInputStep(
        client,
        turn > 0 ? "d" : "a",
        Math.min(200, Math.max(24, Math.abs(turn) * 1_400)),
      );
      if (turnDelivered === false) {
        const stalled = await noteUndeliveredPulse();
        if (stalled) return stalled;
      } else {
        undeliveredSince = 0;
      }
      continue;
    }

    lastAction = "thrust";
    // Size the thrust pulse to the acceleration it has to supply, the same principle the braking path was
    // fixed with in `5581029`: a fixed 22/42 ms pulse adds ~7.3/15 px/s, so near cruise it drives the
    // velocity past the desired value and the next pulses spend themselves cancelling it. Measured thrust is
    // ~367 px/s2, so changing the speed by `accelMagnitude` needs about `accelMagnitude * 2.7` ms; the20ms
    // cruise floor avoids missed samples and the22/42ms cap keeps a distant target accelerating
    // promptly. This is the control-law change the route-leg item asked for, validated against the recorded
    // oscillation (path 1932.9 px vs net 61.6 px).
    // A settling endpoint needs smaller real-key taps than cruise. Retained
    // Firefox/Extreme20ms minima covered one or two60Hz inputs (5.5/11px/s),
    // repeatedly accelerating past the unchanged3.5px/s rest threshold. A4ms
    // fine tap may land on zero or one frame. The stricter gun-line pilot
    // waits for a consumed input step before releasing such a fine tap.
    // Arrival, rest, identity and time budgets remain unchanged.
    const thrustPulseMs = Math.min(
      distance < 48 ? 22 : 42,
      Math.max(settle && distance < 48 ? 4 : 20, accelMagnitude * 2.7),
    );
    const thrustDelivered = await pulseClientOnFreshInputStep(client, thrustKey, thrustPulseMs, {
      requireAppliedStep: settle && distance < 48 && maximumArrivalSpeed < 3.5
        && thrustPulseMs < 1_000 / 60,
    });
    if (thrustDelivered === false) {
      const stalled = await noteUndeliveredPulse();
      if (stalled) return stalled;
      continue;
    }
    undeliveredSince = 0;
    pulses += 1;
  }

  const final = await client.metricsState();
  const finalMetrics = final.uiMetrics || {};
  return {
    reached: false,
    reason: "route timeout",
    // Whole-attempt shape and reversal instrumentation. final.uiMetrics rather than a loop binding: `metrics`
    // is gone by the time the budget runs out, which is the mistake that made this branch throw for four gates.
    ...attemptShapeReport(finalMetrics),
    bestDistance,
    pulses,
    target,
    // The blocked branch has always carried the pilot's own sources; this branch carried a speed and nothing
    // else, so a timeout whose trace shows thrust and no displacement could not be told from one where the
    // input never reached the ship. Carry the same evidence here: what the client believed it held, whether the
    // authority was snapping it back, and whether its own admitted-input trace recorded any movement.
    inputBitsAtTimeout: finalMetrics.localRenderPrediction?.inputBits ?? null,
    selfCorrectionCountAtTimeout: finalMetrics.selfCorrectionCount ?? null,
    lastSelfCorrectionReasonAtTimeout: finalMetrics.lastSelfCorrectionReason ?? null,
    playerAliveAtTimeout: finalMetrics.playerAlive ?? null,
    serverPlayerAliveAtTimeout: finalMetrics.serverPlayerAlive ?? null,
    playerRespawnTimerAtTimeout: finalMetrics.playerRespawnTimer ?? null,
    playerEnergyAtTimeout: finalMetrics.playerEnergy ?? null,
    serverPlayerEnergyAtTimeout: finalMetrics.serverPlayerEnergy ?? null,
    chatActiveAtTimeout: finalMetrics.chatActive ?? null,
    traceSamplesTotal: (final.inputTrace || []).length,
    traceTail: (final.inputTrace || []).slice(-4).map((entry) => ({
      at: Number(entry.sampledAt ?? entry.at),
      playerX: Number(entry.playerX),
      playerY: Number(entry.playerY),
      inputBits: Number(entry.inputBits),
      clientInputSeq: Number(entry.clientInputSeq),
      latestServerTick: Number(entry.latestServerTick),
    })),
    final: finalMetrics,
  };
}

// Reads the shared frame witness without ever becoming the reason a lane fails. The wormhole lane
// is where the arena-switch hitch is being *reproduced*, so an unreadable witness is recorded as a
// reason and the switch still reports its authority evidence.
async function readFrameWitnessWindow(frameWitness) {
  if (!frameWitness) return { ok: false, reason: "no frame witness supplied" };
  try {
    const read = await frameWitness.read();
    if (!read) return { ok: false, reason: "empty frame witness read" };
    if (!read.frameSampler) return { ok: false, reason: "frame witness installed without the sampler" };
    return { ok: true, read };
  } catch (error) {
    return { ok: false, reason: `frame witness read failed: ${String(error?.message ?? error)}` };
  }
}

// Reads the client's own account of what zone volume it has merged and what that merging cost.
// The gate needs both: an arena switch that installs nothing would otherwise report a perfectly
// clean frame window, which is the mistake this fixture was widened to avoid.
async function readZoneMergeState(actor) {
  const local = (await actor.metricsState())?.uiMetrics?.localGameplayState || {};
  return {
    zoneFilesReady: local.zoneFilesReady ?? null,
    pendingZoneDownloads: local.pendingZoneDownloads ?? null,
    lvzMergeCount: local.lvzMergeCount ?? null,
    lvzMergeTotalMs: local.lvzMergeTotalMs ?? null,
    lvzMergeMaxMs: local.lvzMergeMaxMs ?? null,
    zoneLvzObjects: local.zoneLvzObjects ?? null,
    zoneLvzTextures: local.zoneLvzTextures ?? null,
  };
}

async function runWormholeAuthorityGate({ actor, server, playerId, fixture, frameWitness = null }) {
  const before = (await actor.metricsState()).uiMetrics || {};
  const beforeEnergy = before.lastSelfServerEnergy ?? before.playerEnergy;
  if (
    !Number.isFinite(beforeEnergy) ||
    !Number.isFinite(before.playerMaxEnergy) ||
    beforeEnergy < before.playerMaxEnergy - 1 ||
    before.playerAlive !== true ||
    before.serverPlayerAlive !== true
  ) {
    throw new Error(`wormhole fixture did not begin in a full live life: ${JSON.stringify(before)}`);
  }
  const relocationEvents = () => parseZubspaceEvents(server.logs)
    .map((entry) => entry?.event)
    .filter((event) =>
      event?.type === "server_wormhole_relocation" && event.player_id === playerId
    );
  const priorEvents = relocationEvents().length;
  let observedRelocationEvents = 0;
  let routeDriveCount = 0;
  let relocationAppliedMetrics = null;
  // The window opens before the genuine-key approach, so it covers the arrival of the incoming
  // arena's file batches and not only the instant the terminal commits.
  const witnessBefore = await readFrameWitnessWindow(frameWitness);
  const mergeBefore = await readZoneMergeState(actor);
  let replannedFrom = null;
  const routeRun = await driveRouteWithReplans({
    // This active leg heads through the actual map wormhole. The server event, not a guessed distance or timeout,
    // invalidates it; the replacement route is intentionally away from the portal so it can finish during the
    // wormhole cooldown.
    initialDrivePoints: [fixture.target],
    maxReplans: 1,
    currentPosition: async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return { x: metrics.playerX, y: metrics.playerY };
    },
    planFrom: (from) => {
      if (Math.hypot(from.x - fixture.spawn.x, from.y - fixture.spawn.y) > 3) {
        throw new Error(`wormhole route replan did not start at the authoritative landing: ${JSON.stringify(from)}`);
      }
      replannedFrom = { ...from };
      return { drivePoints: [{ x: from.x - 500, y: from.y - 600 }] };
    },
    driveLeg: async (waypoint) => {
      const isInitialPortalLeg = routeDriveCount++ === 0;
      const result = await driveClientToWorldTarget(
        actor,
        waypoint,
        35_000,
        false,
        // The post-relocation waypoint is a short genuine-key recovery leg. Allow a 48px arrival radius so this
        // proof certifies material movement and route recovery without making a small open-arena stop the test's
        // bottleneck.
        isInitialPortalLeg ? 4 : 48,
        async () => {
          const matches = relocationEvents().slice(priorEvents);
          if (matches.length > observedRelocationEvents) {
            observedRelocationEvents = matches.length;
            await waitUntil("client applies the wormhole landing before route recovery", async () => {
              const metrics = (await actor.metricsState()).uiMetrics || {};
              const energy = metrics.lastSelfServerEnergy ?? metrics.playerEnergy;
              const atSpawn = Number.isFinite(metrics.playerX) && Number.isFinite(metrics.playerY) &&
                Math.hypot(metrics.playerX - fixture.spawn.x, metrics.playerY - fixture.spawn.y) <= 3;
              if (atSpawn && Number.isFinite(energy) && energy < beforeEnergy * 0.5) {
                relocationAppliedMetrics = metrics;
                return true;
              }
              return false;
            }, 5_000);
            throw new Error("server committed a wormhole relocation during the active route leg");
          }
          return null;
        },
        220,
        0.75,
        8_000,
      );
      if (result?.reached !== true) {
        throw new Error(`wormhole route waypoint did not complete: ${JSON.stringify(result)}`);
      }
      return result;
    },
  });
  const invalidatedLeg = routeRun.legs.find((leg) => leg.replanned === true);
  const replacementLeg = routeRun.legs.find((leg) => leg.reached === true);
  const route = {
    reached: routeRun.replans === 1 && replacementLeg?.reached === true,
    pulses: replacementLeg?.pulses ?? 0,
    replanCount: routeRun.replans,
    replannedFrom,
    invalidatedLeg: invalidatedLeg ?? null,
    replacementLeg: replacementLeg ?? null,
  };
  if (!route.reached || route.pulses < 1) {
    throw new Error(`genuine-key wormhole route did not relocate: ${JSON.stringify(route)}`);
  }
  if (
    route.replanCount !== 1 ||
    !route.invalidatedLeg?.reason?.includes("wormhole relocation during the active route leg") ||
    !route.replacementLeg?.reached ||
    Math.hypot(route.replannedFrom.x - fixture.spawn.x, route.replannedFrom.y - fixture.spawn.y) > 3
  ) {
    throw new Error(`wormhole relocation did not produce exactly one successful live-position replan: ` +
      JSON.stringify(route));
  }

  let relocation = null;
  await waitUntil("server commits one exact wormhole terminal", async () => {
    const matches = relocationEvents().slice(priorEvents);
    if (matches.length === 1) {
      relocation = matches[0];
      return true;
    }
    if (matches.length > 1) {
      throw new Error(`wormhole terminal repeated: ${JSON.stringify(matches)}`);
    }
    return false;
  }, 5_000);

  const after = (await actor.metricsState()).uiMetrics || {};
  if (relocationAppliedMetrics === null) {
    throw new Error("wormhole route recovered without retaining the pre-recovery client landing state");
  }
  // Both halves of the convergence, because the defect was in the *prediction*: a ship the server relocated to 20%
  // of its energy kept predicting its pre-warp value (measured: server 330, client 911.25), and the
  // earlier `lastSelfServerEnergy ?? playerEnergy` fallback reads whichever field happens to exist, so it can pass
  // while the prediction is still wrong. Both must be finite and below half. They are not asserted equal: the
  // baseline is event-driven and the prediction recharges between events, so they legitimately drift apart.
  const selfEnergy = relocationAppliedMetrics.lastSelfServerEnergy;
  const predictedEnergy = relocationAppliedMetrics.playerEnergy;
  if (
    relocation.position_x !== fixture.spawn.x ||
    relocation.position_y !== fixture.spawn.y ||
    relocation.energy !== Math.floor(beforeEnergy * 0.2) ||
    !Number.isFinite(predictedEnergy) ||
    // The baseline is legitimately absent here - this lane reports `lastSelfServerEnergy: null` - so it is checked
    // only when present. The prediction is the field the defect lived in, so it is always required and always
    // checked.
    (Number.isFinite(selfEnergy) && selfEnergy >= beforeEnergy * 0.5) ||
    predictedEnergy >= beforeEnergy * 0.5 ||
    Math.hypot(relocationAppliedMetrics.playerX - fixture.spawn.x,
      relocationAppliedMetrics.playerY - fixture.spawn.y) > 3
  ) {
    throw new Error(`wormhole authority did not converge atomically: ${JSON.stringify({
      beforeEnergy,
      selfEnergy,
      predictedEnergy,
      relocation,
      relocationAppliedMetrics,
      after,
      fixture: fixture.evidence,
    })}`);
  }
  // The window closes after a settle, because the incoming arena's batches - when a relocation
  // carries any - land over the seconds after the terminal commits.
  let switchFrameWitness = null;
  let switchMerge = null;
  await sleep(3_000);
  let witnessAfter = null;
  if (frameWitness) {
    witnessAfter = await readFrameWitnessWindow(frameWitness);
    switchFrameWitness = witnessBefore.ok && witnessAfter.ok
      ? frameGapDelta(witnessBefore.read, witnessAfter.read)
      : {
        installed: false,
        reason: `before=${witnessBefore.reason ?? "ok"} after=${witnessAfter.reason ?? "ok"}`,
      };
  }
  const mergeAfter = await readZoneMergeState(actor);
  switchMerge = {
    // What the pilot paid to get its Chaos-scale file set: the counters are cumulative for the
    // page's life, and `mergeBefore` was read once the arena had already published, so everything
    // they report was charged to the join, behind the readiness hold - not to this relocation.
    joinInstalls: mergeBefore.lvzMergeCount ?? null,
    joinMergeTotalMs: mergeBefore.lvzMergeTotalMs ?? null,
    joinMergeMaxMs: mergeBefore.lvzMergeMaxMs ?? null,
    relocationInstalls: (mergeAfter.lvzMergeCount ?? 0) - (mergeBefore.lvzMergeCount ?? 0),
    relocationMergeTotalMs:
      Math.round(((mergeAfter.lvzMergeTotalMs ?? 0) - (mergeBefore.lvzMergeTotalMs ?? 0)) * 10) / 10,
    objectsBefore: mergeBefore.zoneLvzObjects ?? null,
    objectsAfter: mergeAfter.zoneLvzObjects ?? null,
    texturesAfter: mergeAfter.zoneLvzTextures ?? null,
    fixtureChaosArchives: fixture?.evidence?.chaosArchiveCount ?? null,
    zoneFilesReady: mergeAfter.zoneFilesReady ?? null,
  };
  // A window drawn over an empty render cache proves nothing about a Chaos-scale arena. The fixture
  // authors the shipped Chaos archive set so the pilot's frames walk 28,922 objects, and the
  // client's own counters are the only evidence that the volume is really there.
  if (!Number.isFinite(switchMerge.objectsAfter) || switchMerge.objectsAfter < 20_000) {
    throw new Error(
      `wormhole arena is not Chaos-scale, so its frame window proves nothing: ` +
        JSON.stringify({ switchMerge, mergeBefore, mergeAfter }),
    );
  }
  if (!(switchMerge.joinInstalls >= (fixture?.evidence?.chaosArchiveCount ?? 0)) ||
      !(switchMerge.joinMergeTotalMs > 0)) {
    throw new Error(
      `the pilot never installed and accounted the fixture's Chaos-scale archive set: ` +
        JSON.stringify({ switchMerge, mergeBefore, mergeAfter }),
    );
  }
  // The page recorder's long-callback ring, carried on the window's own witness read. It holds the
  // zone-merge counters stamped on both sides of every callback at or above 50 ms, so a reported
  // freeze becomes either "this frame merged N archives, +M ms" or "nothing merged and it still cost
  // this much". It arrives on the read rather than a separate evaluate because the WebDriver arm has
  // no generic page-evaluate on its actor - a first attempt through `actor.page.evaluate` failed
  // closed with `ok: false` on Firefox and produced no evidence at all.
  const switchLongFrames = witnessAfter?.ok
    ? {
      ok: true,
      longFrames: witnessAfter.read?.pageLongFrames ?? [],
      tasks: witnessAfter.read?.pageTasks ?? {},
      slowTasks: witnessAfter.read?.pageSlowTasks ?? [],
    }
    : { ok: false, longFrames: [], tasks: {}, slowTasks: [], reason: "no witness read to carry the ring" };
  return {
    fixture: fixture.evidence,
    route,
    beforeEnergy,
    selfEnergy,
    predictedEnergy,
    relocation,
    relocationApplied: compactMetrics(relocationAppliedMetrics),
    switchFrameWitness,
    switchMerge,
    switchLongFrames,
    final: compactMetrics(after),
  };
}

export async function runChaosRouteReplanGate({ actor, server, playerId, fixture }) {
  const before = (await actor.metricsState()).uiMetrics || {};
  const beforeEnergy = before.lastSelfServerEnergy ?? before.playerEnergy;
  if (
    fixture?.kind !== "chaosRouteReplan" ||
    !Number.isFinite(beforeEnergy) ||
    before.playerAlive !== true ||
    before.serverPlayerAlive !== true
  ) {
    throw new Error(`Chaos route fixture did not begin in a full live life: ${JSON.stringify({
      before: compactMetrics(before), fixture: fixture?.evidence,
    })}`);
  }
  const atStart = { x: before.playerX, y: before.playerY };
  if (Math.hypot(atStart.x - fixture.spawn.x, atStart.y - fixture.spawn.y) > 3) {
    throw new Error(`Chaos-derived route fixture did not spawn at its canonical center: ${JSON.stringify({
      atStart, expected: fixture.spawn,
    })}`);
  }

  const originalPlan = chaosSafeRouteToAuthoredSafety(fixture.originalMap, { from: atStart });
  const first = atStart;
  const second = originalPlan.drivePoints[0];
  const target = { x: fixture.wormholeTarget.x, y: fixture.wormholeTarget.y };
  const segmentDistance = Math.hypot(second.x - first.x, second.y - first.y);
  const targetProjection = Math.max(0, Math.min(1,
    ((target.x - first.x) * (second.x - first.x) +
      (target.y - first.y) * (second.y - first.y)) / (segmentDistance * segmentDistance)));
  const targetDistance = Math.hypot(
    target.x - (first.x + targetProjection * (second.x - first.x)),
    target.y - (first.y + targetProjection * (second.y - first.y)),
  );
  const startDistance = Math.hypot(first.x - target.x, first.y - target.y);
  if (startDistance <= fixture.wormholeGravityRadiusPx ||
      targetDistance >= fixture.wormholeGravityRadiusPx) {
    throw new Error(`the first authored Chaos leg no longer enters the deterministic wormhole field: ${JSON.stringify({
      first, second, target, startDistance, targetDistance, targetProjection,
      wormholeGravityRadiusPx: fixture.wormholeGravityRadiusPx, fixture: fixture.evidence,
    })}`);
  }

  // The first authored route leg begins outside the Chaos gravity field and enters it before its waypoint;
  // after relocation the route loop must discard that leg and recompute every remaining point from the landing.
  const initialDrivePoints = originalPlan.drivePoints;
  const relocationEvents = () => parseZubspaceEvents(server.logs)
    .map((entry) => entry?.event)
    .filter((event) =>
      event?.type === "server_wormhole_relocation" && event.player_id === playerId
    );
  const priorEvents = relocationEvents().length;
  let observedRelocationEvents = 0;
  let relocationAppliedMetrics = null;
  let replannedFrom = null;
  let replanEvidence = null;
  let routeDriveCount = 0;
  const routeRun = await driveRouteWithReplans({
    initialDrivePoints,
    maxReplans: 1,
    currentPosition: async () => {
      const metrics = (await actor.metricsState()).uiMetrics || {};
      return { x: metrics.playerX, y: metrics.playerY };
    },
    planFrom: (from) => {
      if (Math.hypot(from.x - fixture.spawn.x, from.y - fixture.spawn.y) > 3) {
        throw new Error(`Chaos safe-route replan did not start at the authoritative landing: ${JSON.stringify(from)}`);
      }
      replannedFrom = { ...from };
      const plan = chaosSafeRouteToAuthoredSafety(fixture.map, {
        from,
        // The dynamic fixture keeps Extreme's portable profile but overrides wormhole gravity to Chaos' 1500
        // and uses the production safe-route clearance around the additional test portal.
        wormholeAvoidRadiusPx: fixture.wormholeAvoidRadiusPx,
      });
      const geometry = authoredMapGeometry(fixture.map, {
        wormholeAvoidRadiusPx: fixture.wormholeAvoidRadiusPx,
      });
      const isSafe = (x, y) => geometry.tileAt(x, y) === 171;
      const deepest = deepestStagingPoint({
        isSafe,
        isHullClear: geometry.authoredTerrainHullIsTraversable,
        from: plan.stage,
      });
      if (!deepest || deepest.safetyMarginPx < 16 || deepest.hullMarginPx < 16) {
        throw new Error(`Chaos route fixture has no authored safety staging point with both margins: ${JSON.stringify({
          plan: { stage: plan.stage, stageTile: plan.stageTile }, deepest, fixture: fixture.evidence,
        })}`);
      }
      const drivePoints = [...plan.drivePoints];
      const finalPoint = drivePoints.at(-1);
      if (Math.hypot(finalPoint.x - deepest.x, finalPoint.y - deepest.y) > 4) {
        drivePoints.push({ x: deepest.x, y: deepest.y });
      }
      replanEvidence = {
        stage: plan.stage,
        stageTile: plan.stageTile,
        hops: plan.hops,
        routePoints: plan.drivePoints.length,
        finalPoint: { x: deepest.x, y: deepest.y },
        expectedSafetyMarginPx: deepest.safetyMarginPx,
        expectedHullMarginPx: deepest.hullMarginPx,
      };
      return { drivePoints };
    },
    driveLeg: async (waypoint, index) => {
      const isInitialPortalLeg = routeDriveCount++ === 0;
      const metrics = (await actor.metricsState()).uiMetrics || {};
      const remaining = Math.hypot(waypoint.x - metrics.playerX, waypoint.y - metrics.playerY);
      const result = await driveClientToWorldTarget(
        actor,
        waypoint,
        Math.max(60_000, Math.round(remaining / 42 * 1_000) + 20_000),
        true,
        4,
        async () => {
          const matches = relocationEvents().slice(priorEvents);
          if (matches.length > 1) {
            throw new Error(`Chaos route fixture relocated more than once: ${JSON.stringify(matches)}`);
          }
          if (matches.length > observedRelocationEvents) {
            observedRelocationEvents = matches.length;
            await waitUntil("client applies the Chaos wormhole landing before replanning", async () => {
              const live = (await actor.metricsState()).uiMetrics || {};
              const energy = live.lastSelfServerEnergy ?? live.playerEnergy;
              const atSpawn = Number.isFinite(live.playerX) && Number.isFinite(live.playerY) &&
                Math.hypot(live.playerX - fixture.spawn.x, live.playerY - fixture.spawn.y) <= 3;
              if (atSpawn && Number.isFinite(energy) && energy < beforeEnergy * 0.5) {
                relocationAppliedMetrics = live;
                return true;
              }
              return false;
            }, 8_000);
            throw new Error("server committed a wormhole relocation during the Chaos safe-route leg");
          }
          return null;
        },
        90,
        0.35,
        8_000,
      );
      if (result?.reached !== true) {
        throw new Error(`Chaos authored safe-route leg ${index} did not reach ${JSON.stringify(waypoint)}: ` +
          JSON.stringify(result));
      }
      return {
        waypoint,
        reached: true,
        pulses: result.pulses,
        distance: Math.round(result.distance ?? 0),
      };
    },
  });

  const relocations = relocationEvents().slice(priorEvents);
  const invalidatedLeg = routeRun.legs.find((leg) => leg.replanned === true);
  const after = (await actor.metricsState()).uiMetrics || {};
  const geometry = authoredMapGeometry(fixture.map, {
    wormholeAvoidRadiusPx: fixture.wormholeAvoidRadiusPx,
  });
  const safetyMarginPx = authoredSafetyMargin((x, y) => geometry.tileAt(x, y) === 171)(
    after.playerX, after.playerY,
  );
  const hullMarginPx = authoredSafetyMargin(geometry.authoredTerrainHullIsTraversable)(
    after.playerX, after.playerY,
  );
  if (
    routeRun.replans !== 1 || relocations.length !== 1 || relocationAppliedMetrics === null ||
    invalidatedLeg?.reached !== false || !invalidatedLeg.reason?.includes("wormhole relocation") ||
    Math.hypot(replannedFrom?.x - fixture.spawn.x, replannedFrom?.y - fixture.spawn.y) > 3 ||
    relocations[0].position_x !== fixture.spawn.x || relocations[0].position_y !== fixture.spawn.y ||
    after.inSafeZone !== true || geometry.tileAt(after.playerX, after.playerY) !== 171 ||
    safetyMarginPx === null || safetyMarginPx < 16 || hullMarginPx === null || hullMarginPx < 16
  ) {
    throw new Error(`Chaos safe-route wormhole recovery did not reach authored safety with one replan: ` +
      JSON.stringify({
        routeRun: {
          replans: routeRun.replans,
          legs: routeRun.legs.length,
          failedLeg: routeRun.legs.at(-1),
          invalidatedLeg,
        },
        relocations,
        replannedFrom,
        replanEvidence,
        safetyMarginPx,
        hullMarginPx,
        final: compactMetrics(after),
        fixture: fixture.evidence,
      }));
  }
  const appliedEnergy = relocationAppliedMetrics.lastSelfServerEnergy ?? relocationAppliedMetrics.playerEnergy;
  if (!Number.isFinite(appliedEnergy) || appliedEnergy >= beforeEnergy * 0.5) {
    throw new Error(`Chaos wormhole landing energy did not converge before recovery: ${JSON.stringify({
      beforeEnergy, appliedEnergy, relocationAppliedMetrics: compactMetrics(relocationAppliedMetrics),
    })}`);
  }

  return {
    fixture: fixture.evidence,
    initial: {
      position: atStart,
      unmodifiedRouteHops: originalPlan.hops,
      unmodifiedDrivePoints: originalPlan.drivePoints.length,
      portalCrossingLeg: { from: first, to: second },
    },
    relocation: relocations[0],
    relocationApplied: compactMetrics(relocationAppliedMetrics),
    replan: {
      count: routeRun.replans,
      from: replannedFrom,
      invalidatedLeg,
      ...replanEvidence,
      completedLegs: routeRun.legs.filter((leg) => leg.reached === true).length,
    },
    authoredSafety: {
      x: after.playerX,
      y: after.playerY,
      inSafeZone: after.inSafeZone,
      safetyMarginPx,
      hullMarginPx,
      requiredMarginPx: 16,
    },
    final: compactMetrics(after),
  };
}

async function observeReleasedOrientationStep(client, initialSequence, label, timeoutMs) {
  if (!Number.isInteger(initialSequence)) {
    throw new Error(`${label} has no logical input sequence at the released-key boundary`);
  }
  let settled = null;
  await waitUntil(`${label} reaches a neutral logical input step`, async () => {
    const state = await client.metricsState();
    const sequence = state.uiMetrics?.clientInputSeq;
    const delta = Number.isInteger(sequence)
      ? (sequence - initialSequence) & 0xffff
      : 0;
    if (delta > 0 && delta < 0x8000) {
      settled = state.uiMetrics || {};
      return true;
    }
    return false;
  }, timeoutMs);
  return settled;
}

async function orientClientForVelocity(client, velocityX, velocityY, timeoutMs = 3_000) {
  const desiredOrientation = (
    Math.atan2(velocityX, -velocityY) / (Math.PI * 2) + 1
  ) % 1;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const metrics = (await client.metricsState()).uiMetrics || {};
    const turn = signedSoakOrientationDelta(metrics.playerOrientation, desiredOrientation);
    if (Math.abs(turn) <= 0.015) {
      const settled = await observeReleasedOrientationStep(
        client,
        metrics.clientInputSeq,
        "continuous aim",
        Math.min(1_000, Math.max(1, deadline - Date.now())),
      );
      const settledTurn = signedSoakOrientationDelta(
        settled.playerOrientation,
        desiredOrientation,
      );
      if (Math.abs(settledTurn) <= 0.015) {
        return { oriented: true, desiredOrientation, metrics: settled };
      }
      continue;
    }
    await pulseClientOnFreshInputStep(
      client,
      turn > 0 ? "d" : "a",
      Math.min(200, Math.max(24, Math.abs(turn) * 1_400)),
    );
  }
  return { oriented: false, desiredOrientation, state: await client.metricsState() };
}

function exactProtocolAimDirection(metrics) {
  const direction = metrics?.playerProtocolDirection;
  if (!Number.isInteger(direction) || direction < 0 || direction >= 40) {
    throw new Error(`protocol-bin aim requires an exact native protocol direction: ${JSON.stringify({
      direction, displayOrientation: metrics?.playerOrientation,
    })}`);
  }
  return direction;
}

async function orientClientForProtocolDirection(client, velocityX, velocityY, timeoutMs = 3_000) {
  const exactOrientation = (
    Math.atan2(velocityX, -velocityY) / (Math.PI * 2) + 1
  ) % 1;
  const desiredDirection = Math.round(exactOrientation * 40) % 40;
  const desiredOrientation = (desiredDirection + 0.5) / 40;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const metrics = (await client.metricsState()).uiMetrics || {};
    const normalizedOrientation = ((metrics.playerOrientation % 1) + 1) % 1;
    // The display rounds orientation to five decimals. Reconstructing a bin
    // from it can certify 0 when the actual f32/wire state is already in 1
    // (retained 0.02500071 -> display 0.025 -> JS positive-wrap floor 0).
    // Keep display angles for steering and native bins for exact admission.
    const direction = exactProtocolAimDirection(metrics);
    if (direction === desiredDirection) {
      // The aim deadline bounds the turning. Once the ship is already inside the
      // wanted bin, the released-step observation still needs at least one real
      // metrics round trip, which Firefox over WebDriver costs tens of milliseconds
      // and a turn loop can spend the whole deadline before reaching this branch.
      // Handing the check `max(1, remaining)` made it abort instantly: the retained
      // Firefox/Chaos timeout proves the client never stalled (worst frame gap 41 ms
      // across the last 1200 frames, and `clientInputSeq` advanced on every one of
      // them - the sequence increments unconditionally in
      // `src/game/network/browser.rs`), so the failure was the budget, not the
      // publication. Overrunning the aim deadline here is bounded to one check and can
      // only make the verdict stricter: a ship that drifts out of the bin while
      // released is reported as not oriented.
      const releasedStepBudgetMs = Math.min(1_000, Math.max(400, deadline - Date.now()));
      const settled = await observeReleasedOrientationStep(
        client,
        metrics.clientInputSeq,
        "protocol-bin aim",
        releasedStepBudgetMs,
      );
      const settledDirection = exactProtocolAimDirection(settled);
      if (settledDirection === desiredDirection) {
        return {
          oriented: true,
          exactOrientation,
          desiredDirection,
          desiredOrientation,
          direction: settledDirection,
          metrics: settled,
        };
      }
      continue;
    }
    const turn = signedSoakOrientationDelta(normalizedOrientation, desiredOrientation);
    await pulseClientOnFreshInputStep(
      client,
      turn > 0 ? "d" : "a",
      Math.min(200, Math.max(24, Math.abs(turn) * 1_400)),
    );
  }
  return {
    oriented: false,
    exactOrientation,
    desiredDirection,
    desiredOrientation,
    state: await client.metricsState(),
  };
}

function brakeOrientationDisposition(metrics, velocityX, velocityY) {
  if (
    !Number.isFinite(metrics?.playerOrientation) ||
    !Number.isFinite(velocityX) || !Number.isFinite(velocityY) ||
    Math.hypot(velocityX, velocityY) <= Number.EPSILON
  ) {
    return { oriented: false, reason: "non-finite braking orientation" };
  }
  const exactOrientation = (
    Math.atan2(velocityX, -velocityY) / (Math.PI * 2) + 1
  ) % 1;
  const desiredDirection = Math.round(exactOrientation * 40) % 40;
  const desiredOrientation = (desiredDirection + 0.5) / 40;
  const orientation = ((metrics.playerOrientation % 1) + 1) % 1;
  const direction = Math.floor(orientation * 40) % 40;
  const exactTurn = signedSoakOrientationDelta(orientation, exactOrientation);
  const protocolTurn = signedSoakOrientationDelta(orientation, desiredOrientation);
  // Braking is a physical opposite-thrust operation, not a projectile wire
  // direction assertion. A minimum genuine key pulse can skip over the
  // nearest protocol bin while landing within the normal continuous heading
  // tolerance (or vice versa at the 39 -> 0 wrap). Either state supplies the
  // intended opposing thrust without weakening exact-bin weapon tests.
  return {
    oriented: direction === desiredDirection || Math.abs(exactTurn) <= 0.015,
    exactOrientation,
    desiredDirection,
    desiredOrientation,
    orientation,
    direction,
    exactTurn,
    protocolTurn,
  };
}

async function orientClientForBraking(client, velocityX, velocityY, timeoutMs = 3_000) {
  const deadline = Date.now() + timeoutMs;
  let disposition = null;
  while (Date.now() < deadline) {
    const metrics = (await client.metricsState()).uiMetrics || {};
    disposition = brakeOrientationDisposition(metrics, velocityX, velocityY);
    if (disposition.oriented || disposition.reason) {
      return { ...disposition, metrics };
    }
    await pulseClientOnFreshInputStep(
      client,
      disposition.protocolTurn > 0 ? "d" : "a",
      Math.min(200, Math.max(24, Math.abs(disposition.protocolTurn) * 1_400)),
    );
  }
  return {
    ...(disposition || {}),
    oriented: false,
    state: await client.metricsState(),
  };
}

// Chaos applies roughly 2.83 px/s of thrust per 60 Hz frame. A browser key
// pulse can therefore cross zero by one frame and settle just above 3 px/s;
// requiring less than that physical input quantum makes the real-key
// controller oscillate forever between two equally valid samples.
function brakeVelocityDisposition(metrics, maximumSpeed = 3.5) {
  const velocityX = metrics?.playerVelocityX;
  const velocityY = metrics?.playerVelocityY;
  const speed = Math.hypot(velocityX, velocityY);
  if (![velocityX, velocityY, speed].every(Number.isFinite)) {
    return { braked: false, reason: "non-finite velocity", metrics };
  }
  return speed <= maximumSpeed
    ? { braked: true, speed, metrics }
    : { braked: false, reason: "still moving", speed, metrics };
}

async function brakeClientVelocity(client, timeoutMs = 4_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const metrics = (await client.metricsState()).uiMetrics || {};
    const disposition = brakeVelocityDisposition(metrics);
    if (disposition.braked || disposition.reason === "non-finite velocity") {
      return disposition;
    }
    const { playerVelocityX: velocityX, playerVelocityY: velocityY } = metrics;
    const { speed } = disposition;
    const oriented = await orientClientForBraking(
      client,
      -velocityX,
      -velocityY,
      1_500,
    );
    if (!oriented.oriented) return { braked: false, reason: "brake turn timeout", oriented };
    // Size the pulse to null the velocity, not to overshoot it. Measured on the Firefox/Extreme
    // leg: retrograde thrust removes about 22 px/s per 60 ms pulse, which is ~367 px/s^2, so the correct hold
    // is roughly speed * 2.7 ms. The previous sizing, speed / 180 * 1000 = speed * 5.6 ms, is about twice as
    // long as nulling needs, and its 20 ms floor means the smallest possible pulse removes ~7.3 px/s. Below
    // about 7 px/s that guarantees the pulse carries the ship through zero and out the other side at roughly
    // half the pulse's delta-v - a quantization limit cycle sitting at ~4.6 px/s, which is exactly the speed
    // these lanes were reporting as a pilot that would not settle (4.46, 4.78 px/s, held constant for tens of
    // seconds with inputBits 0 between pulses). The rest definition, 3.5 px/s, is unchanged: the controller
    // now lands inside it instead of straddling it.
    const holdMs = Math.min(60, Math.max(20, speed * 2.7));
    // Measured, not inferred: a brake that times out reported braked:false with the pilot essentially constant
    // for tens of seconds, which could be a pulse that is too small to move the speed, a pulse that overshoots
    // through zero into the opposite direction, or an orientation that converged to the wrong side. Each
    // possibility needs a different repair and the previous paragraph in this comment guessed wrong twice, so
    // every pulse now leaves its size, the speed it was issued against, and the facing it was issued from, and
    // the next iteration prints the speed that resulted.
    console.log(`brake pulse: speed ${speed.toFixed(3)} px/s, hold ${holdMs.toFixed(0)} ms, `
      + `orientation ${Number(oriented.orientation ?? 0).toFixed(4)} targeting `
      + `${Number(oriented.exactOrientation ?? 0).toFixed(4)} (bin ${oriented.desiredDirection ?? "?"}), `
      + `velocity ${Number(velocityX ?? 0).toFixed(3)},${Number(velocityY ?? 0).toFixed(3)}`);
    await pulseClientOnFreshInputStep(client, "w", holdMs);
  }
  // The last genuine thrust pulse can cross the threshold exactly as its
  // timeout expires. Classify the terminal sample instead of returning a
  // stale timeout verdict for a ship that is already stopped.
  const state = await client.metricsState();
  const terminal = brakeVelocityDisposition(state.uiMetrics || {});
  return terminal.braked
    ? terminal
    : { braked: false, reason: "brake timeout", state };
}

function exactFrozenGameplayState(metrics) {
  return {
    clientInputSeq: metrics?.clientInputSeq,
    pendingWeapon: metrics?.pendingWeapon,
    semanticInputCounts: metrics?.semanticInputCounts,
    localGameplayState: metrics?.localGameplayState,
  };
}

function frozenLocalExecutionState(metrics) {
  const local = metrics?.localGameplayState || {};
  return {
    clientInputSeq: metrics?.clientInputSeq,
    pendingWeapon: metrics?.pendingWeapon,
    semanticInputCounts: metrics?.semanticInputCounts,
    inputRingLen: local.inputRingLen,
    positionX: local.positionX,
    positionY: local.positionY,
    velocityX: local.velocityX,
    velocityY: local.velocityY,
    orientation: local.orientation,
    energy: local.energy,
    bulletCooldown: local.bulletCooldown,
    burstCooldown: local.burstCooldown,
    bombCooldown: local.bombCooldown,
    repelCooldown: local.repelCooldown,
    wormholeCooldown: local.wormholeCooldown,
    // Repeated authoritative RocketState snapshots deliberately self-heal an
    // inactive Rocket timer to its canonical zero.  A negative local timer is
    // already expired, so that normalization is not gameplay-clock progress.
    rocketTimer: Math.max(0, local.rocketTimer),
    rocketCooldown: local.rocketCooldown,
    inventory: {
      repels: local.repels,
      bursts: local.bursts,
      bricks: local.bricks,
      decoys: local.decoys,
      thors: local.thors,
      rockets: local.rockets,
      portals: local.portals,
    },
    localActions: {
      bullets: local.bullets,
      bombs: local.bombs,
      mines: local.mines,
      shrapnel: local.shrapnel,
      thorsActive: local.thorsActive,
      bricksActive: local.bricksActive,
      decoysActive: local.decoysActive,
      repelsActive: local.repelsActive,
      warpsActive: local.warpsActive,
      portalActive: local.portalActive,
    },
  };
}

async function runSelectiveClockStallGate({ actor, peer, server, playerId }) {
  const logOffset = server.logs.join("").length;
  const enabled = await actor.evaluate(
    "window.__zubspaceSetClockStallForTest?.(true) === true",
  );
  if (!enabled) throw new Error("selective clock stall could not be armed");
  const stallStartedAt = Date.now();
  await waitUntil("selective clock stall makes gameplay freshness false", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return metrics.localGameplayState?.doorClockFresh === false;
  }, 4_000);

  const frozenStart = await actor.pageState();
  const frozenBaseline = frozenLocalExecutionState(frozenStart.uiMetrics);
  const startPosition = {
    x: frozenStart.uiMetrics?.playerX,
    y: frozenStart.uiMetrics?.playerY,
  };
  const snapshotStart = frozenStart.uiMetrics?.latestSnapshotId;
  const ackStart = frozenStart.uiMetrics?.lastAckedSnapshotId;
  const remoteStart = {
    x: frozenStart.uiMetrics?.remoteDisplayX,
    y: frozenStart.uiMetrics?.remoteDisplayY,
  };
  // Finish the peer's real-key motion before holding the actor key. Each
  // client owns a separate headed Chrome window, and bringing the peer to the
  // front after the actor key-down correctly fires the actor page's blur
  // handler, which releases all tracked gameplay keys.
  const peerMetrics = (await peer.metricsState()).uiMetrics || {};
  const peerTarget = {
    x: Math.min(15_800, Math.max(600, peerMetrics.playerX + 240)),
    y: peerMetrics.playerY,
  };
  const peerMotion = await driveClientToWorldTarget(peer, peerTarget, 12_000, false);
  const heldRecovery = actor.holdUntil(
    "w",
    "held movement resumes after selective clock recovery",
    (state) => {
      const metrics = state.uiMetrics || {};
      return (
        metrics.localGameplayState?.doorClockFresh === true &&
        metrics.clientInputSeq !== frozenBaseline.clientInputSeq &&
        Math.hypot(metrics.playerX - startPosition.x, metrics.playerY - startPosition.y) > 12
      );
    },
    25_000,
  );
  await sleep(100);
  await actor.holdKeys(["F3", "Control", "z"], 70);
  if (Date.now() - stallStartedAt < 2_200) {
    await sleep(2_200 - (Date.now() - stallStartedAt));
  }
  const frozenEnd = await actor.pageState();
  const frozenFinal = frozenLocalExecutionState(frozenEnd.uiMetrics);
  const seamBeforeRelease = await actor.evaluate(
    "window.__zubspaceClockStallStateForTest?.()",
  );
  const snapshotDistance = snapshotIdForwardDistance(
    frozenEnd.uiMetrics?.latestSnapshotId,
    snapshotStart,
  );
  const ackDistance = snapshotIdForwardDistance(
    frozenEnd.uiMetrics?.lastAckedSnapshotId,
    ackStart,
  );
  const remoteDistance = Math.hypot(
    frozenEnd.uiMetrics?.remoteDisplayX - remoteStart.x,
    frozenEnd.uiMetrics?.remoteDisplayY - remoteStart.y,
  );
  if (
    JSON.stringify(frozenFinal) !== JSON.stringify(frozenBaseline) ||
    frozenEnd.uiMetrics?.localGameplayState?.doorClockFresh !== false ||
    !(snapshotDistance > 0) ||
    !(ackDistance > 0) ||
    !(remoteDistance > 2) ||
    !seamBeforeRelease?.enabled ||
    seamBeforeRelease.delivered !== seamBeforeRelease.seen ||
    seamBeforeRelease.fieldsStalled < 20 ||
    seamBeforeRelease.snapshotsDelivered < 20 ||
    (frozenEnd.metricsPublishCount ?? 0) <= (frozenStart.metricsPublishCount ?? 0)
  ) {
    await actor.evaluate("window.__zubspaceSetClockStallForTest?.(false)");
    throw new Error(`selective clock stall contract failed: ${JSON.stringify({
      frozenBaseline,
      frozenFinal,
      snapshotStart,
      snapshotEnd: frozenEnd.uiMetrics?.latestSnapshotId,
      snapshotDistance,
      ackStart,
      ackEnd: frozenEnd.uiMetrics?.lastAckedSnapshotId,
      ackDistance,
      remoteStart,
      remoteEnd: {
        x: frozenEnd.uiMetrics?.remoteDisplayX,
        y: frozenEnd.uiMetrics?.remoteDisplayY,
      },
      remoteDistance,
      seamBeforeRelease,
      peerMotion,
    })}`);
  }

  const disabled = await actor.evaluate(
    "window.__zubspaceSetClockStallForTest?.(false) === false",
  );
  if (!disabled) throw new Error("selective clock stall could not be released");
  const recoveredMovement = await heldRecovery;
  await sleep(500);
  // A late same-generation settings/level application deliberately replays
  // SECURITY and closes the door clock until the next forward snapshot.  Do
  // not sample the short fail-closed rebind window as a recovery failure;
  // require the authoritative forward sample to reopen it within the normal
  // local-RTC deadline.
  await waitUntil("selective clock recovery remains gameplay-ready", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return metrics.localGameplayState?.doorClockFresh === true;
  }, 4_000);
  const recovered = await actor.pageState();
  const semanticBefore = frozenBaseline.semanticInputCounts || {};
  const semanticAfter = recovered.uiMetrics?.semanticInputCounts || {};
  const rejects = positionAdmissionRejectSummary(server, logOffset);
  const inputWitnessRejects = positionInputWitnessRejectSummary(server, logOffset);
  if (
    recovered.uiMetrics?.localGameplayState?.doorClockFresh !== true ||
    recovered.uiMetrics?.clientInputSeq === frozenBaseline.clientInputSeq ||
    semanticAfter.primary !== semanticBefore.primary ||
    semanticAfter.repel !== semanticBefore.repel ||
    semanticAfter.rocket !== semanticBefore.rocket ||
    Object.values(rejects).some((count) => count !== 0) ||
    Object.values(inputWitnessRejects).some((count) => count !== 0)
  ) {
    throw new Error(`selective clock recovery failed: ${JSON.stringify({
      frozenBaseline,
      recovered: frozenLocalExecutionState(recovered.uiMetrics),
      recoveredMovement: compactMetrics(recoveredMovement.uiMetrics),
      rejects,
      inputWitnessRejects,
    })}`);
  }
  const securityAudit = assertSecurityRejectAudit(server, logOffset, []);
  return {
    stallDurationMs: Date.now() - stallStartedAt,
    frozenBaseline,
    frozenFinal,
    snapshotDistance,
    ackDistance,
    remoteDistance,
    seamBeforeRelease,
    peerMotion,
    recovered: frozenLocalExecutionState(recovered.uiMetrics),
    rejects,
    inputWitnessRejects,
    securityAudit,
    playerId,
  };
}

// A restage can begin inside a naturally open door. The static route planner
// deliberately treats every door as solid and requires a20px hull margin.
// Reuse the existing bounded, genuine-key Warp preparation when that margin
// is absent; it restores full energy and preserves the exact actor life/scope.
async function prepareStaleDoorRouteOrigin({ actor, server, playerId }) {
  const arena = await actor.evaluate("window.__zubspaceArena");
  const { geometry } = loadRendezvousMap(arena, [
    ...((server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT) ?
      [server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT] : []),
    path.join(repoRoot, "assets/zones"),
  ]);
  const metrics = (await actor.metricsState()).uiMetrics || {};
  return prepareKeyboardLifecycleOrigin({ actor, server, playerId,
    initialLifeSeq: metrics.playerLifeSeq, geometry, deadline: Date.now() + 90_000 });
}

function overlapsDoorWithReferenceGameHull(metrics, tileX, tileY) {
  const state = metrics?.localGameplayState || metrics || {};
  if (!Number.isFinite(state.positionX) || !Number.isFinite(state.positionY)) return false;
  // The reference game's newly-closed-door path uses the live ship center (unlike the
  // pixel-truncated wall sweep), with the player hull as the first operand.
  // BoxBoxIntersect is intentionally asymmetric at exact contact: the hull's
  // maximum edge includes the tile minimum, while its minimum edge excludes
  // the tile maximum.
  const centerX = state.positionX;
  const centerY = state.positionY;
  const radius = 14;
  const tileMinX = tileX * 16;
  const tileMinY = tileY * 16;
  const tileMaxX = tileMinX + 16;
  const tileMaxY = tileMinY + 16;
  return (
    centerX + radius >= tileMinX && centerX - radius < tileMaxX &&
    centerY + radius >= tileMinY && centerY - radius < tileMaxY
  );
}

async function runStaleDoorReadinessGate({
  actor,
  peer,
  server,
  playerId,
  setupAttempt = 0,
  setupEvidence = [],
}) {
  const originRecovery = await prepareStaleDoorRouteOrigin({ actor, server, playerId });
  const doorTile = { tileX: 472, tileY: 522, tileId: 162, bit: 0x01 };
  const doorCenter = { x: 7_560, y: 8_360 };
  // Stop well outside the live door before the precise approach. The former
  // 7590±20px coarse disk included7570, inside the door hull; a legitimate
  // close then relocated the pilot before the stale-clock seam even existed.
  // This coarse pocket is not the later entry-pulse origin: that remains27px
  // from the center, with the original measured hull/arrival assertions.
  const initialMetrics = (await actor.metricsState()).uiMetrics || {};
  let doorApproach = {
    x: doorCenter.x + (initialMetrics.playerX < doorCenter.x ? -60 : 60),
    y: doorCenter.y,
  };
  // Random stock spawns can be more than1300px from this fixed map door.
  // Follow the shared full-hull route planner with the rendezvous driver's
  // normal100px/s cruise request. This bounded setup trip is separate from
  // the unchanged precise approach and the later measured recovery deadlines.
  const approach = await navigateAuthoredTarget(actor, doorApproach, server, {
    timeoutMs: 90_000, arrivalRadiusPx: 12,
    maximumDesiredSpeed: 100, desiredSpeedScale: 0.5,
  });
  if (!approach.reached) {
    throw new Error(`stale-door actor could not reach approach: ${JSON.stringify(approach)}`);
  }
  const coarseBrake = await brakeClientVelocity(actor);
  if (!coarseBrake.braked) {
    throw new Error(`stale-door actor could not brake outside approach: ${JSON.stringify(coarseBrake)}`);
  }
  // The coarse route above only establishes the correct door neighborhood.
  // Stage the stopped hull three-to-seven pixels outside the exact asymmetric
  // overlap boundary before waiting for an open phase. Starting anywhere in the old
  // 20px arrival circle made the fixed 225ms entry pulse carry enough residual
  // velocity to coast completely through the door during the one-second
  // stale-clock deadline, so the fixture confused ordinary movement with a
  // door-authority failure.
  const coarseMetrics = (await actor.metricsState()).uiMetrics || {};
  doorApproach = {
    x: doorCenter.x + (coarseMetrics.playerX < doorCenter.x ? -27 : 27),
    y: doorCenter.y,
  };
  const preciseApproach = await driveClientToWorldTarget(
    actor,
    doorApproach,
    12_000,
    false,
    2,
    null,
    12,
  );
  // The generic route helper can report "no progress" after coming to rest
  // inside this fixture's deliberately wider measured outside band. The
  // post-brake hull/distance checks below, rather than its generic two-pixel
  // target verdict, own this setup precondition.
  const braked = await brakeClientVelocity(actor);
  const entryDirectionX = doorApproach.x < doorCenter.x ? 1 : -1;
  const aligned = await orientClientForVelocity(actor, entryDirectionX, 0);
  const stagedMetrics = (await actor.metricsState()).uiMetrics || {};
  const stagedDistance = Math.hypot(
    stagedMetrics.playerX - doorApproach.x,
    stagedMetrics.playerY - doorApproach.y,
  );
  if (
    !braked.braked || !aligned.oriented ||
    !Number.isFinite(stagedDistance) || stagedDistance > 8
  ) {
    throw new Error(`stale-door actor could not stage safely: ${JSON.stringify({
      doorApproach,
      stagedDistance,
      stagedOverlap: overlapsDoorWithReferenceGameHull(
        stagedMetrics,
        doorTile.tileX,
        doorTile.tileY,
      ),
      stagedPosition: { x: stagedMetrics.playerX, y: stagedMetrics.playerY },
      preciseApproach,
      braked,
      aligned,
    })}`);
  }

  // The finite braking floor can carry the hull just into an open door
  // while the final alignment runs. Treat that as the same pre-seam setup
  // drift handled below: its phase loop must observe a clear outside hull,
  // or use the existing three-attempt restaging limit before any stall is
  // armed. Failing here instead never exercised the recovery contract.
  let priorSeed = (await actor.metricsState()).uiMetrics?.localGameplayState?.doorSeed;
  let entered = null;
  let setupRestartBeforeStall = null;
  const openPhaseTrace = [];
  const stageCorrections = [];
  const openDeadline = Date.now() + 75_000;
  while (Date.now() < openDeadline && !entered?.reached) {
    // Real keyboard feedback keeps the ship immediately outside the door
    // while phases advance. Once a newly observed phase opens tile 162, enter
    // it before stalling inbound packets; this avoids spending the one-second
    // freshness allowance on navigation setup.
    await sleep(20);
    const metrics = (await actor.metricsState()).uiMetrics || {};
    const local = metrics.localGameplayState || {};
    priorSeed = local.doorSeed;
    const lastPhase = openPhaseTrace.at(-1);
    if (
      lastPhase?.doorSeed !== local.doorSeed ||
      lastPhase?.doorSeedNext !== local.doorSeedNext ||
      lastPhase?.doorSeedAfterNext !== local.doorSeedAfterNext ||
      lastPhase?.fresh !== local.doorClockFresh
    ) {
      openPhaseTrace.push({
        at: Date.now(),
        serverTick: metrics.latestSnapshotServerTick,
        doorSeed: local.doorSeed,
        doorSeedNext: local.doorSeedNext,
        doorSeedAfterNext: local.doorSeedAfterNext,
        fresh: local.doorClockFresh,
        position: { x: metrics.playerX, y: metrics.playerY },
      });
    }
    const stageDistance = Math.hypot(
      metrics.playerX - doorApproach.x,
      metrics.playerY - doorApproach.y,
    );
    const stageOverlap = overlapsDoorWithReferenceGameHull(
      metrics,
      doorTile.tileX,
      doorTile.tileY,
    );
    if (stageDistance > 256 || stageOverlap) {
      // The physical braking floor can also carry the stopped hull just over
      // the open boundary between feedback samples. If that phase then closes,
      // the server correctly relocates to spawn before the fault seam exists.
      // This is setup evidence, not the stale-door terminal under test: restart
      // the bounded physical-key approach instead of asking the short local
      // correction controller to route hundreds of pixels from spawn.
      setupRestartBeforeStall = {
        reason: stageDistance > 256
          ? "relocated before stale-door seam"
          : "drifted into open door before stale-door seam",
        stageDistance,
        stageOverlap,
        doorSeed: local.doorSeed,
        fresh: local.doorClockFresh,
        position: { x: metrics.playerX, y: metrics.playerY },
      };
      break;
    }
    if (stageDistance > 8) {
      // The reference game's braking floor can leave a real ship coasting at a small
      // non-zero velocity. A long wait for three consecutive authored open
      // phases therefore needs the same closed-loop keyboard correction a
      // human uses; otherwise the fixture can drift tens of pixels away and
      // never exercise the door at all.
      const correctionRoute = await driveClientToWorldTarget(
        actor,
        doorApproach,
        8_000,
        false,
        2,
        null,
        12,
      );
      const correctionBrake = await brakeClientVelocity(actor);
      const correctionAligned = await orientClientForVelocity(actor, entryDirectionX, 0);
      const corrected = (await actor.metricsState()).uiMetrics || {};
      const correctedDistance = Math.hypot(
        corrected.playerX - doorApproach.x,
        corrected.playerY - doorApproach.y,
      );
      const correction = {
        priorDistance: stageDistance,
        correctedDistance,
        route: correctionRoute,
        brake: correctionBrake,
        aligned: correctionAligned,
        position: { x: corrected.playerX, y: corrected.playerY },
      };
      stageCorrections.push(correction);
      if (
        !correctionBrake.braked || !correctionAligned.oriented || correctedDistance > 8 ||
        overlapsDoorWithReferenceGameHull(corrected, doorTile.tileX, doorTile.tileY)
      ) {
        throw new Error(`stale-door feedback could not preserve the outside stage: ${JSON.stringify({
          doorApproach,
          correction,
          openPhaseTrace,
        })}`);
      }
      continue;
    }
    // Chaos advances doors every authored 1.2 seconds. The browser can drain a
    // small already-delivered RTC queue after the transport seam is armed, so
    // current+next alone does not cover the measured roughly two-second stale
    // transition when sampled near a phase boundary. Require exactly one more
    // authenticated successor, leaving at least 2.4 seconds without reviving
    // the old four-phase setup wait.
    const currentAndTwoSuccessorsOpen =
      (local.doorSeed & doorTile.bit) === 0 &&
      (local.doorSeedNext & doorTile.bit) === 0 &&
      (local.doorSeedAfterNext & doorTile.bit) === 0;
    if (
      local.doorClockFresh === true && currentAndTwoSuccessorsOpen
    ) {
      // From the exact narrow outside stage, a 40ms acceleration pulse enters
      // by ordinary unpowered coast but remains inside the 44px-wide reference-game
      // hull interval throughout the measured stale-clock transition.
      await actor.hold("w", 40);
      const entryDeadline = Date.now() + 2_200;
      while (Date.now() < entryDeadline) {
        const entryMetrics = (await actor.metricsState()).uiMetrics || {};
        if (overlapsDoorWithReferenceGameHull(entryMetrics, doorTile.tileX, doorTile.tileY)) {
          entered = { reached: true, overlap: true, entryMetrics };
          break;
        }
        // A close can win the few milliseconds between the metrics sample and
        // overlap observation. Production then correctly relocates to spawn,
        // but the fault seam has not been armed yet. Detect that bounded setup
        // race and restage rather than waiting out the remaining open deadline
        // from a point hundreds of pixels away.
        if (
          Math.hypot(
            entryMetrics.playerX - doorApproach.x,
            entryMetrics.playerY - doorApproach.y,
          ) > 256
        ) {
          setupRestartBeforeStall = {
            reason: "door closed between entry sample and overlap",
            doorSeed: entryMetrics.localGameplayState?.doorSeed,
            fresh: entryMetrics.localGameplayState?.doorClockFresh,
            position: { x: entryMetrics.playerX, y: entryMetrics.playerY },
          };
          break;
        }
        if (
          entryMetrics.localGameplayState?.doorClockFresh !== true ||
          (entryMetrics.localGameplayState?.doorSeed & doorTile.bit) !== 0
        ) {
          entered = { reached: false, reason: "entry phase budget expired", entryMetrics };
          break;
        }
        await sleep(15);
      }
    }
    if (setupRestartBeforeStall) break;
    await sleep(20);
  }
  if (setupRestartBeforeStall && setupAttempt < 2) {
    await sleep(300);
    return runStaleDoorReadinessGate({
      actor,
      peer,
      server,
      playerId,
      setupAttempt: setupAttempt + 1,
      setupEvidence: [...setupEvidence, setupRestartBeforeStall],
    });
  }
  if (setupRestartBeforeStall) {
    throw new Error(`stale-door setup restarted three bounded times before the fault seam: ${JSON.stringify([
      ...setupEvidence,
      setupRestartBeforeStall,
    ])}`);
  }
  if (
    !entered?.reached ||
    !overlapsDoorWithReferenceGameHull(entered.entryMetrics, doorTile.tileX, doorTile.tileY)
  ) {
    throw new Error(`actor did not enter door 162 during a fresh open phase: ${JSON.stringify({
      priorSeed,
      entered,
      stageCorrections,
      openPhaseTrace,
    })}`);
  }

  const intendedLifeSeq = entered.entryMetrics?.playerLifeSeq;
  if (!Number.isInteger(intendedLifeSeq) || intendedLifeSeq < 1) {
    throw new Error(`stale-door actor has no authoritative intended life: ${JSON.stringify({
      playerId,
      intendedLifeSeq,
      entryMetrics: entered.entryMetrics,
    })}`);
  }
  const intendedActor = { playerId, lifeSeq: intendedLifeSeq };
  const logOffset = server.logs.join("").length;
  const enabled = await actor.evaluate(
    "window.__zubspaceSetS2cStallForTest?.(true, { keepQualityProbes: true }) === true",
  );
  if (!enabled) {
    throw new Error("S2C stale-door stall could not be armed");
  }
  const stallStartedAt = Date.now();
  let lostOverlapBeforeStale = null;
  await waitUntil("door clock becomes stale and closes local gameplay gate", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    const fresh = metrics.localGameplayState?.doorClockFresh;
    const closedBeforeStale =
      fresh !== false && (metrics.localGameplayState?.doorSeed & doorTile.bit) !== 0;
    const overlap = overlapsDoorWithReferenceGameHull(metrics, doorTile.tileX, doorTile.tileY);
    if (closedBeforeStale || !overlap) {
      lostOverlapBeforeStale = {
        setupAttempt,
        elapsedMs: Date.now() - stallStartedAt,
        fresh,
        closedBeforeStale,
        overlap,
        doorSeed: metrics.localGameplayState?.doorSeed,
        doorSeedNext: metrics.localGameplayState?.doorSeedNext,
        position: { x: metrics.playerX, y: metrics.playerY },
        velocity: {
          x: metrics.playerVelocityX,
          y: metrics.playerVelocityY,
        },
      };
      return true;
    }
    return fresh === false;
  }, 3_000);
  if (lostOverlapBeforeStale != null) {
    await actor.evaluate("window.__zubspaceSetS2cStallForTest?.(false)");
    if (setupAttempt >= 2) {
      throw new Error(`stale-door setup lost the door overlap on three bounded attempts: ${JSON.stringify([
        ...setupEvidence,
        lostOverlapBeforeStale,
      ])}`);
    }
    await sleep(300);
    return runStaleDoorReadinessGate({
      actor,
      peer,
      server,
      playerId,
      setupAttempt: setupAttempt + 1,
      setupEvidence: [...setupEvidence, lostOverlapBeforeStale],
    });
  }
  const frozenStart = await actor.pageState();
  const frozenBaseline = exactFrozenGameplayState(frozenStart.uiMetrics);
  const startPosition = {
    x: frozenStart.uiMetrics?.playerX,
    y: frozenStart.uiMetrics?.playerY,
  };
  if (
    !overlapsDoorWithReferenceGameHull(
      frozenStart.uiMetrics,
      doorTile.tileX,
      doorTile.tileY,
    ) ||
    (frozenBaseline.localGameplayState?.doorSeed & doorTile.bit) !== 0 ||
    frozenBaseline.localGameplayState?.doorClockFresh !== false
  ) {
    throw new Error(`stale actor was not frozen inside door 162: ${JSON.stringify({
      startPosition,
      frozenBaseline,
    })}`);
  }

  // Move the peer before holding the actor key. Bringing the peer's separate
  // headed Chrome window forward after actor key-down would correctly blur the
  // actor page and release its tracked gameplay keys.
  const peerMotion = await peer.holdKeys(["w", "a"], 2_000);

  const actorRelocations = () => canonicalDoorRelocationsForActor(
    serverLogSince(server, logOffset),
    intendedActor,
  );
  const actorRelocationCount = () => actorRelocations().length;

  // Keep a real held movement sample across recovery. Require motion away from
  // the canonical zero-velocity spawn bundle after the server observes that
  // relocation, so the relocation itself cannot satisfy the held-key proof.
  // The gameplay edges are pressed only after the stale gate is visibly closed
  // and therefore must be discarded rather than replayed later.
  const heldRecovery = actor.holdUntil(
    "w",
    "held thrust resumes only after stale-door recovery",
    (state) => {
      const metrics = state.uiMetrics || {};
      return (
        metrics.localGameplayState?.doorClockFresh === true &&
        metrics.clientInputSeq !== frozenBaseline.clientInputSeq &&
        actorRelocationCount() === 1 &&
        Math.hypot(metrics.playerX - 8_200, metrics.playerY - 8_200) > 12 &&
        Math.hypot(metrics.playerVelocityX, metrics.playerVelocityY) > 5
      );
    },
    20_000,
  );
  await sleep(100);
  await actor.holdKeys(["F3", "Control", "z"], 60);

  const minimumStallMs = 10_000;
  const remaining = minimumStallMs - (Date.now() - stallStartedAt);
  if (remaining > 0) await sleep(remaining);
  const frozenEnd = await actor.pageState();
  const frozenFinal = exactFrozenGameplayState(frozenEnd.uiMetrics);
  const stallBeforeRelease = await actor.evaluate(
    "window.__zubspaceS2cStallStateForTest?.()",
  );
  if (
    JSON.stringify(frozenFinal) !== JSON.stringify(frozenBaseline) ||
    !overlapsDoorWithReferenceGameHull(
      frozenEnd.uiMetrics,
      doorTile.tileX,
      doorTile.tileY,
    ) ||
    (frozenFinal.localGameplayState?.doorSeed & doorTile.bit) !== 0 ||
    !stallBeforeRelease?.enabled ||
    stallBeforeRelease.keepQualityProbes !== true ||
    !(stallBeforeRelease.qualityProbesPassed >= 1) ||
    stallBeforeRelease.buffered < 100 ||
    stallBeforeRelease.queued !== stallBeforeRelease.buffered ||
    (frozenEnd.metricsPublishCount ?? 0) <= (frozenStart.metricsPublishCount ?? 0)
  ) {
    throw new Error(`stale-door local freeze changed gameplay state: ${JSON.stringify({
      frozenBaseline,
      frozenFinal,
      stallBeforeRelease,
      metricsPublishStart: frozenStart.metricsPublishCount,
      metricsPublishEnd: frozenEnd.metricsPublishCount,
    })}`);
  }

  const recoveryStartedAt = Date.now();
  const disabled = await actor.evaluate(
    "window.__zubspaceSetS2cStallForTest?.(false) === false",
  );
  if (!disabled) {
    throw new Error("S2C stale-door stall could not be released");
  }
  const recoveredMovement = await heldRecovery;
  await waitUntil("server accepts exactly one canonical door relocation", async () => {
    return actorRelocationCount() === 1;
  }, 8_000);
  await sleep(750);
  const recovered = await actor.pageState();
  const recoveryTrace = (recovered.metricsTrace || []).filter(
    (metrics) => Number.isFinite(metrics.at) && metrics.at >= recoveryStartedAt - 80,
  );
  const canonicalSpawnSample = recoveryTrace.find((metrics) =>
    Math.hypot(metrics.playerX - 8_200, metrics.playerY - 8_200) <= 24
  );
  const stallAfterRelease = await actor.evaluate(
    "window.__zubspaceS2cStallStateForTest?.()",
  );
  const relocationCount = actorRelocationCount();
  const canonicalRelocations = actorRelocations();
  const rejects = positionAdmissionRejectSummary(server, logOffset);
  const inputWitnessRejects = positionInputWitnessRejectSummary(server, logOffset);
  const semanticAfter = recovered.uiMetrics?.semanticInputCounts || {};
  const semanticBefore = frozenBaseline.semanticInputCounts || {};
  const relocatedDistance = Math.hypot(
    recovered.uiMetrics?.playerX - startPosition.x,
    recovered.uiMetrics?.playerY - startPosition.y,
  );
  if (
    stallAfterRelease?.enabled ||
    stallAfterRelease?.released !== stallAfterRelease?.buffered ||
    stallAfterRelease?.released < stallBeforeRelease.buffered ||
    stallAfterRelease?.queued !== 0 ||
    relocationCount !== 1 ||
    !canonicalSpawnSample ||
    relocatedDistance < 40 ||
    recovered.uiMetrics?.localGameplayState?.doorClockFresh !== true ||
    recovered.uiMetrics?.clientInputSeq === frozenBaseline.clientInputSeq ||
    semanticAfter.primary !== semanticBefore.primary ||
    semanticAfter.rocket !== semanticBefore.rocket ||
    recovered.uiMetrics?.playerRockets !== frozenStart.uiMetrics?.playerRockets ||
    Object.values(rejects).some((count) => count !== 0) ||
    Object.values(inputWitnessRejects).some((count) => count !== 0)
  ) {
    throw new Error(`stale-door recovery failed: ${JSON.stringify({
      frozenBaseline,
      recovered: exactFrozenGameplayState(recovered.uiMetrics),
      recoveredMovement: compactMetrics(recoveredMovement.uiMetrics),
      stallBeforeRelease,
      stallAfterRelease,
      relocationCount,
      intendedActor,
      canonicalRelocations,
      canonicalSpawnSample,
      relocatedDistance,
      rejects,
      inputWitnessRejects,
    })}`);
  }

  const securityAudit = assertSecurityRejectAudit(server, logOffset, []);
  return {
    controls: ["w", "F3", "Control", "z"],
    doorTile: { ...doorTile, ...doorCenter },
    originRecovery,
    approach,
    stageCorrections,
    openPhaseTrace,
    stallDurationMs: Date.now() - stallStartedAt,
    stallBeforeRelease,
    stallAfterRelease,
    frozenBaseline,
    frozenFinal,
    relocationCount,
    intendedActor,
    canonicalRelocations,
    canonicalSpawnSample,
    relocatedDistance,
    recovered: exactFrozenGameplayState(recovered.uiMetrics),
    rejects,
    inputWitnessRejects,
    securityAudit,
    playerId,
  };
}

async function runShipChangeEnergyGate(actor, peer) {
  const before = await actor.pageState();
  const life = before.uiMetrics.playerLifeSeq;
  const playerId = before.uiMetrics.selfPlayerId;
  const observer = await peer.pageState();
  const oldShip = observer.uiMetrics.remoteActorPresentation.find((entry) => entry.id === playerId)?.ship;
  if (!Number.isInteger(oldShip) || oldShip < 0 || oldShip > 7) {
    throw new Error("ship energy gate cannot identify the current playable ship");
  }
  const targetShip = oldShip === 0 ? 1 : 0;
  // Spend the same energy pool used for damage through genuine weapon input.
  await actor.press("Control", 1200);
  const depleted = await actor.pageState();
  if (!(depleted.uiMetrics.playerEnergy < depleted.uiMetrics.playerMaxEnergy - 100)) {
    throw new Error(`ship energy gate did not realize an energy deficit: ${JSON.stringify(compactMetrics(depleted.uiMetrics))}`);
  }
  for (const key of [String(targetShip + 1), "F11"]) {
    await actor.press("Escape", 30);
    await actor.press(key, 30);
    const blocked = await actor.pageState();
    if (blocked.uiMetrics.playerLifeSeq !== life || blocked.uiMetrics.playerAlive !== true ||
        blocked.uiMetrics.localGameplayState?.authorityPhase !== "Live") {
      throw new Error(`damaged ship lifecycle key ${key} was not blocked: ${JSON.stringify(compactMetrics(blocked.uiMetrics))}`);
    }
  }
  await waitUntil("ship fully recharges before changing type", async () => {
    const state = await actor.pageState();
    return state.uiMetrics.playerEnergy >= state.uiMetrics.playerMaxEnergy &&
      state.uiMetrics.localGameplayState?.lastSelfServerEnergy >= state.uiMetrics.playerMaxEnergy;
  }, 20_000);
  await actor.press("Escape", 60);
  await actor.press(String(targetShip + 1), 60);
  await waitUntil("full-energy ship change completes its authority receipt", async () => {
    const state = await actor.pageState();
    return state.uiMetrics.playerLifeSeq !== life &&
      state.uiMetrics.localGameplayState?.authorityPhase === "Live";
  }, 12_000);
  const changed = await actor.pageState();
  // Restore the combat ship through an independently eligible full-energy menu.
  await waitUntil("replacement ship is full before restoring combat profile", async () => {
    const state = await actor.pageState();
    return state.uiMetrics.playerEnergy >= state.uiMetrics.playerMaxEnergy &&
      state.uiMetrics.localGameplayState?.lastSelfServerEnergy >= state.uiMetrics.playerMaxEnergy;
  }, 20_000);
  await actor.press("Escape", 60);
  await actor.press(String(oldShip + 1), 60);
  await waitUntil("combat profile restored after full-energy gate", async () => {
    const state = await actor.pageState();
    return state.uiMetrics.playerLifeSeq !== changed.uiMetrics.playerLifeSeq &&
      state.uiMetrics.localGameplayState?.authorityPhase === "Live";
  }, 12_000);
  return { depletedEnergy: depleted.uiMetrics.playerEnergy, maxEnergy: depleted.uiMetrics.playerMaxEnergy,
    oldLife: life, changedLife: changed.uiMetrics.playerLifeSeq, blockedShipAndSpectator: true };
}

async function runMovementDeviationSoak({ actor, server, playerId, logOffset }) {
  await waitUntil("server-owned hidden prizes spawn for movement soak", async () => {
    return parseHiddenPrizeSpawns(server).length > 0;
  }, 12_000);

  const routes = [];
  // Selecting ship 1 through the ordinary number-key UI guarantees a real
  // lifecycle reset to zero velocity before navigating to a natural green.
  await actor.press("Escape", 60);
  await actor.press("1", 60);
  await sleep(700);

  const playerClaims = () => parsePrizeClaims(server, logOffset)
    .filter((claim) => claim.playerId === playerId);
  const routeToNaturalPrize = async (description, predicate) => {
    const routeDeadline = Date.now() + 35_000;
    while (Date.now() < routeDeadline) {
      const accepted = playerClaims().find(predicate);
      if (accepted) return accepted;

      const actorState = await actor.pageState();
      const actorX = actorState.uiMetrics?.playerX;
      const actorY = actorState.uiMetrics?.playerY;
      if (![actorX, actorY].every(Number.isFinite)) {
        throw new Error(`movement soak lost live ship metrics: ${JSON.stringify(actorState.uiMetrics)}`);
      }

      const claimedPositions = new Set(
        parsePrizeClaims(server).map((claim) => `${claim.x},${claim.y}`),
      );
      const attemptedPositions = new Set(
        routes.map((route) => `${route.target.x},${route.target.y}`),
      );
      const candidates = parseHiddenPrizeSpawns(server)
        .filter(predicate)
        .filter((prize) => !claimedPositions.has(`${prize.x},${prize.y}`))
        .filter((prize) => !attemptedPositions.has(`${prize.x},${prize.y}`))
        .sort((left, right) =>
          Math.hypot(left.x - actorX, left.y - actorY) -
          Math.hypot(right.x - actorX, right.y - actorY)
        );
      if (candidates.length === 0) {
        await sleep(250);
        continue;
      }

      const route = await driveClientToWorldTarget(actor, candidates[0], 15_000);
      routes.push(route);
      await sleep(250);
    }
    throw new Error(`headed movement soak reached no ${description}: ${JSON.stringify({
      routes,
      claims: playerClaims(),
      final: (await actor.pageState()).uiMetrics,
    })}`);
  };

  // Chaos Zone's initial bounty legitimately starts movement stats at their
  // cap. Collect a natural negative movement green first, then its positive
  // counterpart, so the positive protocol 3/11/12 prize has an observable
  // authoritative tuning delta without mutating browser or server state.
  const weakeningClaim = await routeToNaturalPrize(
    "negative movement prize",
    (prize) => prize.prizeId < 0 && movementPrizeIds.has(Math.abs(prize.prizeId)),
  );
  const restoringPrizeId = -weakeningClaim.prizeId;
  const restoringClaim = await routeToNaturalPrize(
    `positive movement prize ${restoringPrizeId}`,
    (prize) => prize.prizeId === restoringPrizeId,
  );
  const exactWarbirdValues = assertExactChaosWarbirdMovementPrizeSequence(
    weakeningClaim,
    restoringClaim,
  );

  await sleep(500);
  const claims = playerClaims()
    .filter((claim) => movementPrizeIds.has(Math.abs(claim.prizeId)));
  const weakeningChangedTuning =
    Number.isFinite(weakeningClaim.movementTuningBefore) &&
    Number.isFinite(weakeningClaim.movementTuningAfter) &&
    weakeningClaim.movementTuningAfter < weakeningClaim.movementTuningBefore;
  const restoringChangedTuning =
    Number.isFinite(restoringClaim.movementTuningBefore) &&
    Number.isFinite(restoringClaim.movementTuningAfter) &&
    restoringClaim.movementTuningAfter > restoringClaim.movementTuningBefore;
  if (!weakeningChangedTuning || !restoringChangedTuning) {
    throw new Error(`natural movement prizes changed no authoritative server tuning: ${JSON.stringify(claims)}`);
  }

  const qwasdLogOffset = server.logs.join("").length;
  // Exercise the player-facing QWASD controls after the natural prize is
  // applied. Curved forward arcs cover thrust + rotation while reverse and
  // repel exercise the remaining special-state adapters.
  for (let cycle = 0; cycle < 4; cycle += 1) {
    await actor.holdKeys(["w", "a"], 500);
    await actor.holdKeys(["w", "d"], 500);
    await actor.hold("s", 300);
    await actor.press("q", 80);
  }
  await sleep(500);

  const deviation = simObservationSummary(server, qwasdLogOffset, playerId);
  const globalDeviation = simObservationSummary(server, qwasdLogOffset);
  if (
    deviation.stableActiveInput.samples < 100 ||
    deviation.stableActiveInput.underTwoRatio == null ||
    deviation.stableActiveInput.underTwoRatio < 0.95 ||
    deviation.transitionDiagnostic.samples < 4 ||
    deviation.missingInputSamples !== 0 ||
    deviation.invalidInputSamples !== 0 ||
    deviation.missingInputDiagnostics !== 0 ||
    deviation.unclassifiedSamples !== 0
  ) {
    throw new Error(`movement deviation gate missed the 95%-under-2px target: ${JSON.stringify(deviation)}`);
  }

  const securityAudit = assertSecurityRejectAudit(server, qwasdLogOffset, []);
  return {
    controls: ["q", "w", "a", "s", "d"],
    naturalPrizeClaims: claims,
    authoritativeTuningDecrease: weakeningClaim,
    authoritativeTuningChange: restoringClaim,
    exactWarbirdValues,
    routes,
    deviation,
    securityAudit,
    scoredWindow: {
      logOffset: qwasdLogOffset,
      playerId,
      player: deviation,
      global: globalDeviation,
    },
    observerLogOffset: qwasdLogOffset,
  };
}

async function runNegativeEnergyReconciliationGate({ actor, server, serverPort, playerId, logOffset }) {
  const statusResponse = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`, {
    signal: AbortSignal.timeout(10_000),
  });
  if (!statusResponse.ok) throw new Error(`negative Energy fixture status HTTP ${statusResponse.status}`);
  const status = await statusResponse.json();
  const maximumEntry = negativeEnergyLoadout === "maximum";
  if (status.maxed_spawn_loadout !== maximumEntry || status.zone !== "extreme") {
    throw new Error(`negative Energy loadout/zone mismatch: ${JSON.stringify({
      profile: negativeEnergyLoadout, observedMaxedSpawnLoadout: status.maxed_spawn_loadout,
      observedZone: status.zone,
    })}`);
  }
  // Exact authored Extreme Warbird values: InitialEnergy=1650,
  // MaximumEnergy=2800, UpgradeEnergy=400. Keep the reference upgrade case
  // and separately exercise the production maximum-entry cap.
  const expectedInitialCapacity = maximumEntry ? 2800 : 1650;
  const expectedChargedCapacity = maximumEntry ? 2800 : 2050;
  const fixture = { profile: negativeEnergyLoadout, maxedSpawnLoadout: maximumEntry,
    observedMaxedSpawnLoadout: status.maxed_spawn_loadout, arenaSettings: "unmodified",
    expectedInitialCapacity, expectedChargedCapacity };
  const routes = [];
  const claims = () => parsePrizeClaims(server, logOffset)
    .filter((claim) => claim.playerId === playerId);
  const routeExact = async (prizeId, label) => {
    let target = null;
    await waitUntil(`${label} natural hidden identity`, async () => {
      const claimed = new Set(parsePrizeClaims(server).map((claim) => claim.collectibleId));
      const attempted = new Set(routes.map((route) => route.target.collectibleId));
      const state = await actor.metricsState();
      const x = state.uiMetrics?.playerX;
      const y = state.uiMetrics?.playerY;
      target = parseHiddenPrizeSpawns(server, logOffset)
        .filter((spawn) => spawn.prizeId === prizeId)
        .filter((spawn) => !claimed.has(spawn.collectibleId))
        .filter((spawn) => !attempted.has(spawn.collectibleId))
        .sort((left, right) =>
          Math.hypot(left.x - x, left.y - y) - Math.hypot(right.x - x, right.y - y)
        )[0] || null;
      return target != null;
    }, 30_000);
    let claim = null;
    const route = await driveClientToWorldTarget(
      actor,
      target,
      20_000,
      false,
      8,
      async () => {
        claim = claims().find((candidate) =>
          candidate.collectibleId === target.collectibleId &&
          candidate.prizeId === prizeId
        ) || null;
        return claim;
      },
      240,
    );
    routes.push(route);
    if (!route.reached || !claim) {
      throw new Error(`${label} did not preserve its natural identity: ${JSON.stringify({
        target,
        route,
        claims: claims(),
      })}`);
    }
    return { target, claim };
  };

  const initial = await actor.metricsState();
  if (initial.uiMetrics?.playerShip !== 0 || initial.uiMetrics?.selfRosterShip !== 0 ||
      initial.uiMetrics?.playerMaxEnergy !== expectedInitialCapacity) {
    throw new Error(`negative Energy entry differs from its declared Warbird fixture: ${JSON.stringify({
      fixture, playerShip: initial.uiMetrics?.playerShip, selfRosterShip: initial.uiMetrics?.selfRosterShip,
      maxEnergy: initial.uiMetrics?.playerMaxEnergy,
    })}`);
  }
  const positiveEnergy = await routeExact(2, "positive Energy");
  const fullCharge = await routeExact(13, "Full Charge");
  const beforeNegative = await actor.metricsState();
  if (!(
    beforeNegative.uiMetrics?.playerEnergy >= beforeNegative.uiMetrics?.playerMaxEnergy - 1 &&
    beforeNegative.uiMetrics?.playerMaxEnergy === expectedChargedCapacity
  )) {
    throw new Error(`positive Energy then Full Charge did not establish the over-cap precursor: ${JSON.stringify({
      fixture,
      initial: initial.uiMetrics,
      beforeNegative: beforeNegative.uiMetrics,
      positiveEnergy,
      fullCharge,
    })}`);
  }
  const negativeRouteStartedAt = Date.now();
  const negativeEnergy = await routeExact(-2, "negative Energy");
  let overCap = null;
  await waitUntil("negative Energy preserves authoritative transient over-cap reconciliation", async () => {
    // Filter the passive wire/prize traces in-page and return only the exact
    // identity plus compact metrics. Shipping the full bounded packet log
    // over DevTools inside a 100 ms poll can itself stall a headed renderer;
    // it is not gameplay evidence and does not strengthen this assertion.
    const observed = await actor.evaluate(`(() => {
      const bySequence = new Map();
      for (const entry of [...(window.__zubspacePacketLogFirst || []), ...(window.__zubspacePacketLog || [])]) {
        const key = entry.seq ?? \`${"${entry.direction}:${entry.at}:${JSON.stringify(entry.bytes || [])}"}\`;
        bySequence.set(key, entry);
      }
      const ack = [...bySequence.values()].reverse().find((entry) => {
        const bytes = entry.bytes || [];
        const collectibleId = bytes.length === 50 && bytes[0] === 0 && bytes[1] === 3
          ? (bytes[7] | (bytes[8] << 8) | (bytes[9] << 16) | (bytes[10] << 24)) >>> 0
          : null;
        return entry.direction === "received" && bytes[6] === 0x61 &&
          collectibleId === ${negativeEnergy.target.collectibleId};
      }) || null;
      const traced = [...(window.__zubspacePrizeStateTrace || [])].reverse().find((entry) =>
        entry.at >= ${negativeRouteStartedAt} && entry.energy > entry.maxEnergy &&
        entry.serverEnergy > entry.maxEnergy && entry.predictedEnergy > entry.maxEnergy &&
        entry.visibleEnergy > entry.maxEnergy
      ) || null;
      const metrics = window.__zubspaceUiMetrics || {};
      return {
        ack: ack ? { bytes: ack.bytes || [], seq: ack.seq, at: ack.at } : null,
        traced,
        metrics: {
          playerEnergy: metrics.playerEnergy,
          playerPredictedEnergy: metrics.playerPredictedEnergy,
          serverPlayerEnergy: metrics.serverPlayerEnergy,
          visibleEnergyDigits: metrics.visibleEnergyDigits,
          playerMaxEnergy: metrics.playerMaxEnergy,
          playerLifeSeq: metrics.playerLifeSeq,
          prizeAuthorityRevision: metrics.localGameplayState?.prizeAuthorityRevision,
        },
      };
    })()`);
    const metrics = observed.metrics || {};
    const ack = observed.ack;
    const ackBytes = ack?.bytes || [];
    const ackPrizeRaw = ackBytes.length === 50
      ? ackBytes[15] | (ackBytes[16] << 8)
      : null;
    const ackPrizeId = ackPrizeRaw == null
      ? null
      : ackPrizeRaw >= 0x8000 ? ackPrizeRaw - 0x1_0000 : ackPrizeRaw;
    const ackEnergy = ackBytes.length === 50
      ? ackBytes[17] | (ackBytes[18] << 8)
      : null;
    const ackLifeSeq = ackBytes.length === 50
      ? (ackBytes[19] | (ackBytes[20] << 8) | (ackBytes[21] << 16) | (ackBytes[22] << 24)) >>> 0
      : null;
    const ackRevision = ackBytes.length === 50
      ? (ackBytes[23] | (ackBytes[24] << 8) | (ackBytes[25] << 16) | (ackBytes[26] << 24)) >>> 0
      : null;
    const live = {
      energy: metrics.playerEnergy,
      predictedEnergy: metrics.playerPredictedEnergy,
      serverEnergy: metrics.serverPlayerEnergy,
      visibleEnergy: metrics.visibleEnergyDigits,
      maxEnergy: metrics.playerMaxEnergy,
      lifeSeq: metrics.playerLifeSeq,
      revision: metrics.prizeAuthorityRevision,
    };
    const traced = observed.traced;
    const candidate = traced || (
      ackPrizeId === -2 &&
      ackEnergy > metrics.playerMaxEnergy &&
      ackLifeSeq === metrics.playerLifeSeq &&
      ackRevision === metrics.prizeAuthorityRevision
        ? {
            energy: ackEnergy,
            predictedEnergy: ackEnergy,
            serverEnergy: ackEnergy,
            visibleEnergy: ackEnergy,
            maxEnergy: metrics.playerMaxEnergy,
            lifeSeq: ackLifeSeq,
            revision: ackRevision,
          }
        : live
    );
    if (candidate.energy > candidate.maxEnergy &&
        candidate.maxEnergy === expectedChargedCapacity - 400) {
      overCap = {
        ...candidate,
        source: traced ? "prize-state-trace" : "recipient-private-pickup-ack",
        acknowledgement: ack ? {
          collectibleId: negativeEnergy.target.collectibleId,
          prizeId: ackPrizeId,
          authoritativeEnergy: ackEnergy,
          lifeSeq: ackLifeSeq,
          revision: ackRevision,
        } : null,
        reconciled: {
          energy: metrics.playerEnergy,
          predictedEnergy: metrics.playerPredictedEnergy,
          serverEnergy: metrics.serverPlayerEnergy,
          visibleEnergy: metrics.visibleEnergyDigits,
          maxEnergy: metrics.playerMaxEnergy,
          lifeSeq: metrics.playerLifeSeq,
          revision: metrics.prizeAuthorityRevision,
        },
      };
      return true;
    }
    return false;
  }, 5_000);
  const projectileStart = parseServerProjectileEvents(server.logs).filter((event) =>
    event.type === "server_projectile_spawned" && event.ownerId === playerId
  ).length;
  await actor.press("Control", 80);
  let naturalSpend = null;
  await waitUntil("over-cap energy converges consistently through realtime weapon spend", async () => {
    const state = await actor.metricsState();
    const metrics = state.uiMetrics || {};
    const projectileCount = parseServerProjectileEvents(server.logs).filter((event) =>
      event.type === "server_projectile_spawned" && event.ownerId === playerId
    ).length;
    // This gate proves the four energy owners reconcile through an ordinary
    // real-key weapon spend. A short Control tap creates one primary-fire edge;
    // exact hit cardinality belongs to the dedicated single-bullet gates,
    // while this prize gate requires its authoritative spawn and owner
    // convergence without conflating projectile travel with prize authority.
    if (
      projectileCount > projectileStart &&
      metrics.playerEnergy < overCap.energy &&
      metrics.serverPlayerEnergy <= overCap.serverEnergy &&
      metrics.playerEnergy <= overCap.energy &&
      metrics.playerPredictedEnergy <= overCap.energy &&
      metrics.visibleEnergyDigits <= overCap.energy
    ) {
      naturalSpend = {
        energy: metrics.playerEnergy,
        predictedEnergy: metrics.playerPredictedEnergy,
        serverEnergy: metrics.serverPlayerEnergy,
        visibleEnergy: metrics.visibleEnergyDigits,
        maxEnergy: metrics.playerMaxEnergy,
        authoritativeProjectileDelta: projectileCount - projectileStart,
      };
      return true;
    }
    return false;
  }, 5_000);
  const securityAudit = assertSecurityRejectAudit(server, logOffset, []);
  return {
    seed: 39_064_502,
    fixture,
    initial: {
      energy: initial.uiMetrics?.playerEnergy,
      maxEnergy: initial.uiMetrics?.playerMaxEnergy,
    },
    positiveEnergy,
    fullCharge,
    negativeEnergy,
    overCap,
    naturalSpend,
    routes,
    securityAudit,
  };
}

async function runWarpRelocationGate({ actor, peer, server, serverPort, playerId, prizeStreamLogOffset }) {
  const startedAt = Date.now();
  const logOffset = server.logs.join("").length;
  if (!Number.isSafeInteger(prizeStreamLogOffset) || prizeStreamLogOffset < 0 || prizeStreamLogOffset > logOffset)
    throw new Error("invalid pre-join Warp prize stream boundary");
  const prizeScope = m => JSON.stringify([m.localGameplayState?.authorityScopeServerSessionId,
    m.localGameplayState?.authorityScopeArenaStreamId, m.selfPlayerId, m.playerLifeSeq]);
  const arena = await actor.evaluate("window.__zubspaceArena");
  const loadedMap = loadRendezvousMap(arena, [
    ...((server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT) ?
      [server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT] : []),
    path.join(repoRoot, "assets/zones"),
  ]);
  // Let the natural Warp collectible cross the ordered reliable stream before
  // intentionally holding Repel A's receipt. Otherwise the held wrapper would
  // correctly head-of-line block the later prize spawn from reaching the
  // browser. The stream starts at join, before random-spawn encounter travel.
  // Choose an unclaimed live prize with enough unchanged TTL for actual routes;
  // a late log boundary must not discard a still-live earlier seeded Warp.
  let prizePreparation = null;
  await waitUntil("live hull-clear natural positive Warp prize preparation", async () => {
    const metrics = (await Promise.all([actor.metricsState(), peer.metricsState()])).map(s => s.uiMetrics);
    if (!metrics.every(exactLiveGameplayScope)) return false;
    if (metrics.some(m => m.localGameplayState?.brickTilesOverflow))
      throw new Error("incomplete brick geometry for Warp prize preparation");
    const origins = metrics.map(m => ({ x: m.playerX, y: m.playerY }));
    const currentTick = metrics[0].latestSnapshotServerTick;
    const claimed = new Set(parsePrizeClaims(server).map(p => p.collectibleId));
    const live = parseHiddenPrizeSpawns(server, prizeStreamLogOffset).filter(p => {
      const remaining = (p.expiresAtTick - currentTick) & 0x7fff_ffff;
      return p.prizeId === 7 && !claimed.has(p.collectibleId) && remaining > 0 && remaining < 0x4000_0000;
    });
    const avoidTiles = live.map(p => ({ x: Math.floor(p.x / 16), y: Math.floor(p.y / 16) }));
    const blockedTiles = metrics.flatMap(m => m.localGameplayState?.brickTiles || []);
    for (const spawn of live) {
      let preparation;
      try {
        preparation = planTimedPrizePreparation(loadedMap.geometry, origins, spawn, currentTick,
          { blockedTiles, avoidTiles, repelPickup: true });
      } catch (error) {
        if (!/no hull-clear authored approach route|no conservative route|no conservative timed-prize Repel launch|origin lacks conservative hull clearance|not a clear tile-centred stage/.test(error.message)) throw error;
        continue;
      }
      if (preparation) { prizePreparation = { spawn, preparation, map: loadedMap.provenance,
        scopes: metrics.map(prizeScope) }; return true; }
    }
    return false;
  }, 12_000);
  const target = prizePreparation.spawn;
  const preparationDeadline = Date.now() + prizePreparation.preparation.setupBudgetMs;
  prizePreparation.encounter = await rendezvousPlayers(actor, peer, server, {
    setupBudgetMs: prizePreparation.preparation.setupBudgetMs, stageRadiusPx: 20,
    meetingPoint: prizePreparation.preparation.plan.target, avoidTiles: prizePreparation.preparation.avoidTiles,
  });
  const focusReleases = [actor, peer].map(client => client.acquireInputFocusLease?.()).filter(Boolean);
  try {
    const stage = async (client, destination) => {
      const remaining = preparationDeadline - Date.now();
      if (remaining <= 0) throw new Error("Warp Repel staging exceeded its qualified setup budget");
      return navigateAuthoredTarget(client, destination, server, {
        timeoutMs: remaining, arrivalRadiusPx: 0, avoidTiles: prizePreparation.preparation.avoidTiles,
      });
    };
    const launch = prizePreparation.preparation.repelLaunch;
    prizePreparation.repelStaging = {
      peer: await stage(peer, launch.peer),
      actor: await stage(actor, launch.actor),
    };
  } finally { for (const release of focusReleases.reverse()) release(); }
  await waitForPlayersReady(actor, peer, server);
  const beforeRepel = await actor.pageState();
  const prepared = beforeRepel.uiMetrics || {};
  const preparedPeer = (await peer.metricsState()).uiMetrics || {};
  if (![prepared, preparedPeer].every((m, i) => exactLiveGameplayScope(m) && prizeScope(m) === prizePreparation.scopes[i]) ||
      prepared.selfPlayerId !== playerId)
    throw new Error("Warp prize preparation crossed a player lifecycle boundary");
  const remaining = (target.expiresAtTick - prepared.latestSnapshotServerTick) & 0x7fff_ffff;
  if (!prepared.localGameplayState?.activePrizeCollectibleIds?.includes(target.collectibleId) ||
      !(remaining > 0 && remaining < 0x4000_0000) ||
      parsePrizeClaims(server).some(p => p.collectibleId === target.collectibleId))
    throw new Error(`Warp prize no longer live and unclaimed after preparation: ${JSON.stringify(prizePreparation)}`);
  const dx = prepared.playerX - preparedPeer.playerX, dy = prepared.playerY - preparedPeer.playerY;
  const tx = target.x - prepared.playerX, ty = target.y - prepared.playerY;
  const separation = Math.hypot(dx, dy);
  const missDistance = Math.abs(dx * ty - dy * tx) / separation;
  if (!(separation >= 64 && separation <= 140 && dx * tx + dy * ty > 0 && missDistance <= 12))
    throw new Error(`Warp Repel launch does not cross its pickup: ${JSON.stringify({ separation, missDistance, prepared, preparedPeer })}`);
  prizePreparation.repelStaging.launch = { separation, missDistance };
  prizePreparation.remainingTicksAfterSetup = remaining;
  const beforeConsumed = prepared.localGameplayState?.consumedRepelDeliveryGenerations ?? 0;
  await sleep(500);
  await peer.press("q", 80);
  let heldRepel = null;
  await waitUntil("first hostile Repel is admitted while its receipt is held", async () => {
    // This interval contains500px/s real flight. Serializing the entire
    // packet/history trace here stalls Chrome; only current metrics and the
    // impairment counters are required to establish the held application.
    const state = await actor.metricsState();
    const local = state.uiMetrics?.localGameplayState;
    const impairment = await actor.evaluate(
      "window.__zubspaceRepelDeliveryImpairmentStateForTest?.()",
    );
    if (
      !Number.isInteger(local?.pendingRepelApplicationGeneration) ||
      impairment?.receiptDropped !== 1 ||
      impairment?.receiptBuffered !== 1 ||
      impairment?.receiptReleased !== 0
    ) {
      return false;
    }
    heldRepel = {
      beforeConsumed,
      pendingGeneration: local.pendingRepelApplicationGeneration,
      pendingSnapshotId: local.pendingRepelApplicationSnapshotId,
      pendingSequence: local.pendingRepelApplicationSequence,
      deferredWarpAcks: local.deferredWarpPickupAcks,
      impairment,
    };
    return true;
  }, 8_000);

  let claim = null;
  const pickupDeadline = Date.now() + 20_000;
  const pickupClaim = () => parsePrizeClaims(server, logOffset).find((entry) =>
    entry.playerId === playerId && entry.collectibleId === target.collectibleId && entry.prizeId === 7
  ) || null;
  claim = pickupClaim();
  // The genuine hostile impulse may have already crossed the collectible.
  // Require its exact native claim, rather than piloting back to a consumed
  // prize or rejecting a legitimate post-contact hull beside terrain.
  const route = claim ? { reached: false, completed: true, keysOnly: true, contact: "native Repel flight" }
    : await navigateAuthoredTarget(actor, target, server, {
    timeoutMs: 20_000, arrivalRadiusPx: 16,
    avoidTiles: prizePreparation.preparation.pickupAvoidTiles,
    complete: async () => {
      claim = pickupClaim();
      return claim;
    },
  });
  if ((!route.reached && !route.completed) || !claim) {
    throw new Error(`real-key route did not collect the natural Warp prize: ${JSON.stringify({
      target,
      route,
      claims: parsePrizeClaims(server, logOffset),
    })}`);
  }
  // A real Repel can collect the prize at500px/s before keyboard navigation
  // begins. Brake that impulse with real controls inside the same20s pickup
  // budget so the later shooter approaches a stopped, exact-life actor.
  route.braking = await brakeClientVelocity(actor, Math.min(8_000, Math.max(0, pickupDeadline - Date.now())));
  if (!route.braking.braked || Date.now() > pickupDeadline)
    throw new Error("Warp pickup did not finish real-key braking within its original budget");

  let warpHeldStatus = null;
  await waitUntil("Warp ACK is held behind the first Repel receipt", async () => {
    const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
    if (!response.ok) return false;
    const status = await response.json();
    const repel = repelDeliveryQueueSummary(status);
    if (
      repel.queuePackets < 1 ||
      repel.awaitingReceiptAck < 1 ||
      status.warp_pickup_acks_deferred < 1
    ) {
      return false;
    }
    warpHeldStatus = {
      repel,
      warpPickupAcksDeferred: status.warp_pickup_acks_deferred,
    };
    return true;
  }, 5_000);

  // Bring the shooter back into exact Repel range without touching browser
  // or server gameplay state, then queue B behind admitted/unreceipted A.
  const actorBeforeSecondRepel = await actor.metricsState();
  const peerRoute = await navigateAuthoredTarget(
    peer,
    {
      x: actorBeforeSecondRepel.uiMetrics?.playerX,
      y: actorBeforeSecondRepel.uiMetrics?.playerY,
    },
    server,
    {
      // A's full225-tick impulse can leave the actor over1,200px away.
      // Use full-hull setup flight at normal ship speed, rather than the
      //42px/s prize collector, inside the same20s second-Repel approach bound.
      // Leave braking margin: full gain repeatedly overshoots the captured
      // waypoints despite a clear path and genuine admitted controls.
      timeoutMs: 20_000, arrivalRadiusPx: 64,
      maximumDesiredSpeed: 325, desiredSpeedScale: 0.5,
    },
  );
  if (!peerRoute.reached) {
    throw new Error(`peer could not enter range for queued Repel B: ${JSON.stringify(peerRoute)}`);
  }
  await peer.press("q", 80);

  let twoRepelsHeldStatus = null;
  await waitUntil("Repel B queues behind A while Warp remains held", async () => {
    const response = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`);
    if (!response.ok) return false;
    const status = await response.json();
    const repel = repelDeliveryQueueSummary(status);
    if (
      repel.queuePackets < 2 ||
      repel.awaitingReceiptAck < 1 ||
      repel.awaitingApplication < 1 ||
      status.warp_pickup_acks_deferred < 1
    ) {
      return false;
    }
    twoRepelsHeldStatus = {
      repel,
      warpPickupAcksDeferred: status.warp_pickup_acks_deferred,
    };
    return true;
  }, 8_000);

  const receiptReleased = await actor.evaluate(
    "window.__zubspaceReleaseRepelReceiptImpairmentForTest?.() === true",
  );
  if (!receiptReleased) {
    throw new Error("could not release the held Repel A application receipt");
  }

  let repelBeforeWarp = null;
  await waitUntil("A then B retire before the held Warp ACK is released", async () => {
    const [state, response] = await Promise.all([
      actor.metricsState(),
      fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`),
    ]);
    if (!response.ok) return false;
    const status = await response.json();
    const repel = repelDeliveryQueueSummary(status);
    const local = state.uiMetrics?.localGameplayState;
    if (
      (local?.consumedRepelDeliveryGenerations ?? 0) < beforeConsumed + 2 ||
      local?.pendingRepelApplicationGeneration != null ||
      repel.queuePackets !== 0 ||
      repel.awaitingApplication !== 0 ||
      repel.awaitingReceiptAck !== 0 ||
      status.warp_pickup_acks_deferred !== 0
    ) {
      return false;
    }
    repelBeforeWarp = {
      heldRepel,
      warpHeldStatus,
      twoRepelsHeldStatus,
      peerRoute,
      afterConsumed: local.consumedRepelDeliveryGenerations,
      pendingGeneration: local.pendingRepelApplicationGeneration,
      pendingSnapshotId: local.pendingRepelApplicationSnapshotId,
      pendingSequence: local.pendingRepelApplicationSequence,
      deferredWarpAcks: local.deferredWarpPickupAcks,
      finalServerQueue: repel,
      finalWarpPickupAcksDeferred: status.warp_pickup_acks_deferred,
      receiptImpairment: await actor.evaluate(
        "window.__zubspaceRepelDeliveryImpairmentStateForTest?.()",
      ),
    };
    return true;
  }, 10_000);

  let relocation = null;
  await waitUntil("explicit Warp marker commits authoritative relocation", async () => {
    relocation = serverLogSince(server, logOffset)
      .split("\n")
      .filter((line) => line.includes("authoritative Warp prize relocation observed"))
      .map((line) => ({
        collectibleId: parseNumericField(line, "collectible_id"),
        lifeSeq: parseNumericField(line, "life_seq"),
        positionSequence: parseNumericField(line, "client_sequence"),
        x: parseNumericField(line, "x"),
        y: parseNumericField(line, "y"),
      }))
      .find((entry) => entry.collectibleId === claim.collectibleId) || null;
    return relocation != null;
  }, 8_000);

  let evidence = null;
  await waitUntil("Warp application receipt returns exact marker identity", async () => {
    const state = await actor.pageState();
    const packets = mergedPacketLog(state)
      .filter((entry) => (entry.at ?? 0) >= startedAt && Array.isArray(entry.bytes));
    const markers = packets
      .map((entry) => ({ entry, marker: clientPositionWarpMarker(entry.bytes) }))
      .filter(({ entry, marker }) =>
        entry.direction === "sent" && marker?.collectibleId === claim.collectibleId
      );
    const receipts = packets
      .map((entry) => ({ entry, receipt: warpRelocationAcceptance(entry.bytes) }))
      .filter(({ entry, receipt }) =>
        entry.direction === "received" && receipt?.collectibleId === claim.collectibleId
      );
    const exactMarker = markers.find(({ marker }) =>
      marker.positionSequence === relocation.positionSequence
    );
    const exactReceipt = receipts.find(({ receipt }) =>
      receipt.lifeSeq === relocation.lifeSeq &&
      receipt.positionSequence === relocation.positionSequence
    );
    if (!exactMarker || !exactReceipt || exactMarker.entry.seq >= exactReceipt.entry.seq) {
      return false;
    }
    evidence = {
      markerCopies: markers.length,
      markerPacketSequence: exactMarker.entry.seq,
      receiptPacketSequence: exactReceipt.entry.seq,
      marker: exactMarker.marker,
      receipt: exactReceipt.receipt,
    };
    return true;
  }, 8_000);

  await sleep(500);
  const final = await actor.pageState();
  const markersAfterReceipt = mergedPacketLog(final)
    .filter((entry) =>
      entry.direction === "sent" &&
      entry.seq > evidence.receiptPacketSequence &&
      clientPositionWarpMarker(entry.bytes)?.collectibleId === claim.collectibleId
    );
  const relocationDistance = Math.hypot(
    (final.uiMetrics?.playerX ?? relocation.x) - relocation.x,
    (final.uiMetrics?.playerY ?? relocation.y) - relocation.y,
  );
  const boundaryPositionLoss = {
    warp: final.networkStats?.warpBoundaryPositionsDropped ?? 0,
    repel: final.networkStats?.repelBoundaryPositionsDropped ?? 0,
  };
  if (
    boundaryPositionLoss.warp !== 1 ||
    boundaryPositionLoss.repel !== 1 ||
    markersAfterReceipt.length !== 0 ||
    !(relocationDistance < 256)
  ) {
    throw new Error(`Warp marker did not retire exactly once at canonical relocation: ${JSON.stringify({
      evidence,
      boundaryPositionLoss,
      markersAfterReceipt,
      relocation,
      final: final.uiMetrics,
      relocationDistance,
    })}`);
  }
  return {
    repelBeforeWarp,
    prizePreparation,
    target,
    claim,
    route,
    relocation,
    relocationDistance,
    evidence,
    boundaryPositionLoss,
    markersAfterReceipt: markersAfterReceipt.length,
    securityAudit: assertSecurityRejectAudit(server, logOffset, []),
  };
}

// Pure reduction of one immutable scored log window. The live runner uses
// the same function in a worker so repeated parsing cannot block server-pipe
// draining. No sample, floor, rejection category or replay rule is filtered.
function c2sRecoveryLogSummary(server, logOffset, playerId, participantIds) {
  const playerDeviation = simObservationSummary(server, logOffset, playerId);
  const globalDeviation = simObservationSummary(server, logOffset);
  const participantDeviations = Object.fromEntries(
    participantIds.map((participantId) => [
      String(participantId),
      simObservationSummary(server, logOffset, participantId),
    ]),
  );
  const replaySteps = simReplayStepSummary(server, logOffset, playerId);
  const missingInputRejects = securityRejectCount(
    server,
    logOffset,
    "position",
    "missing_input_history",
  );
  const inputWitnessRejects = positionInputWitnessRejectSummary(server, logOffset);
  const positionAdmissionRejects = positionAdmissionRejectSummary(server, logOffset);
  const inputRecoveryTimeouts = serverLogSince(server, logOffset)
    .split("\n")
    .filter((line) => line.includes("position input recovery timed out")).length;
  const snapbackRejects = securityRejectCount(server, logOffset, "movement", "snapback");
  return { playerDeviation, globalDeviation, participantDeviations, replaySteps, missingInputRejects, inputWitnessRejects, positionAdmissionRejects, inputRecoveryTimeouts, snapbackRejects };
}

async function runC2sInputRecoverySoak({
  actor,
  peer,
  server,
  serverPort,
  playerId,
  participantIds,
}) {
  await waitForLivePlayerScopes(actor, peer);
  const initialStates = await Promise.all([actor.metricsState(), peer.metricsState()]);
  const expectedMembership = initialStates.map(state => ({
    id: state.uiMetrics.selfPlayerId, lifeSeq: state.uiMetrics.playerLifeSeq,
  }));
  const snapbacksBefore = movementSnapbackSummary(
    await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json(),
  );
  // Put both real-key clients at the same known non-safe Chaos fixture used by
  // the retained successful Repel trace.
  // Starting the long independent steering loops first left hostile-Repel
  // coverage to a scheduler race: on a slower debug browser the target could
  // leave the authored 160 px square before the shooter's first Q edge reached
  // authority. The route can enter the documented combat-view hold between
  // staging and Q; a one-shot edge correctly fails closed in that state and
  // must not be replayed against a later drawn view, so readiness and range are
  // revalidated together immediately before the genuine Q edge below.
  // Random lifecycle origins need full-map approach flight before the original
  // short, exact Repel staging window. Setup is outside measured impairment.
  // The2x CPU profile is already active during setup. Keep100px/s cruise
  // but use the ordinary gentler braking taper: the faster taper oscillated
  // across a3px waypoint without satisfying the unchanged3.5px/s rest bound.
  const fixtureApproaches = await Promise.all([
    navigateAuthoredTarget(actor, { x: 8064, y: 8225 }, server, {
      timeoutMs: 90_000, arrivalRadiusPx: 12, maximumDesiredSpeed: 100, desiredSpeedScale: 0.25,
    }),
    navigateAuthoredTarget(peer, { x: 8138, y: 8190 }, server, {
      timeoutMs: 90_000, arrivalRadiusPx: 12, maximumDesiredSpeed: 100, desiredSpeedScale: 0.25,
    }),
  ]);
  const [actorStage, peerStage] = await Promise.all([
    driveClientToWorldTarget(actor, { x: 8064, y: 8225 }, 12_000, true, 24),
    driveClientToWorldTarget(peer, { x: 8138, y: 8190 }, 12_000, true, 24),
  ]);
  if (!actorStage.reached || !peerStage.reached) {
    throw new Error(`C2S Repel fixture staging failed: ${JSON.stringify({
      actorStage,
      peerStage,
    })}`);
  }
  const [actorFixture, peerFixture] = await Promise.all([
    actor.metricsState(),
    peer.metricsState(),
  ]);
  const actorX = actorFixture.uiMetrics?.playerX;
  const actorY = actorFixture.uiMetrics?.playerY;
  const peerX = peerFixture.uiMetrics?.playerX;
  const peerY = peerFixture.uiMetrics?.playerY;
  const fixtureSeparation = Math.hypot(
    actorX - peerX,
    actorY - peerY,
  );
  if (
    ![actorX, actorY, peerX, peerY].every(Number.isFinite) ||
    actorFixture.uiMetrics?.inSafeZone !== false ||
    peerFixture.uiMetrics?.inSafeZone !== false ||
    !(fixtureSeparation < 120)
  ) {
    throw new Error(`C2S Repel fixture was not authoritative and in range: ${JSON.stringify({
      actor: actorFixture.uiMetrics,
      peer: peerFixture.uiMetrics,
      fixtureSeparation,
    })}`);
  }
  let fixtureMembership = null;
  await waitUntil("both recovery pilots have exact-life authoritative membership after approach", async () => {
    const states = await Promise.all([actor.metricsState(), peer.metricsState()]);
    if (!states.every(state => exactLiveGameplayScope(state.uiMetrics)) ||
        !recoveryMembershipReady(states, expectedMembership)) return false;
    fixtureMembership = states.map(state => ({
      playerId: state.uiMetrics.selfPlayerId, lifeSeq: state.uiMetrics.playerLifeSeq,
      latestSnapshotId: state.uiMetrics.latestSnapshotId,
      actors: state.uiMetrics.authoritativeSnapshotActors,
    }));
    return true;
  }, 30_000);
  const repelFixture = {
    membership: fixtureMembership,
    approaches: fixtureApproaches,
    actor: {
      x: actorX,
      y: actorY,
      inSafeZone: actorFixture.uiMetrics.inSafeZone,
    },
    peer: {
      x: peerX,
      y: peerY,
      inSafeZone: peerFixture.uiMetrics.inSafeZone,
    },
    separation: fixtureSeparation,
  };
  const impairmentEnabled = await actor.evaluate(
    "window.__zubspaceSetC2sInputImpairmentForTest?.(true) === true",
  );
  if (!impairmentEnabled) {
    throw new Error("C2S input impairment was not armed at the WebRTC bridge");
  }
  const start = await actor.metricsState();
  const startedAt = Date.now();
  const beforeStats = c2sInputStats(start);
  const logOffset = server.logs.join("").length;
  const startPosition = {
    x: start.uiMetrics?.playerX,
    y: start.uiMetrics?.playerY,
  };
  const readyActorX = start.uiMetrics?.playerX;
  const readyActorY = start.uiMetrics?.playerY;
  const readyPeer = await peer.metricsState();
  const readyPeerX = readyPeer.uiMetrics?.playerX;
  const readyPeerY = readyPeer.uiMetrics?.playerY;
  const readySeparation = Math.hypot(
    readyActorX - readyPeerX,
    readyActorY - readyPeerY,
  );
  if (
    ![readyActorX, readyActorY, readyPeerX, readyPeerY].every(Number.isFinite) ||
    start.uiMetrics?.inSafeZone !== false ||
    readyPeer.uiMetrics?.inSafeZone !== false ||
    !(readySeparation < 120)
  ) {
    throw new Error(`C2S Repel fixture was not ready at the input boundary: ${JSON.stringify({
      actor: start.uiMetrics,
      peer: readyPeer.uiMetrics,
      readySeparation,
    })}`);
  }
  let maxActorDistance = 0;
  const observeActorDistance = async () => {
    // Steering feedback needs only current metrics. A full Firefox pageState
    // includes bounded packet/sprite/history logs that reach many megabytes
    // during this debug-WASM soak; repeatedly serializing them stalls the
    // browser clock and creates a test-induced door-history discontinuity.
    const state = await actor.metricsState();
    maxActorDistance = Math.max(
      maxActorDistance,
      Math.hypot(
        (state.uiMetrics?.playerX ?? startPosition.x) - startPosition.x,
        (state.uiMetrics?.playerY ?? startPosition.y) - startPosition.y,
      ),
    );
  };

  // Establish the hostile Repel while the two authoritative witnesses are
  // definitely in range, then prove the deliberately dropped private frame
  // and receipt both recover before independent movement can separate them.
  // Q remains genuine DevTools keyboard input and its rolling C2S action is
  // already passing through the seeded omission/reorder seam.
  let readyInputBoundary = null;
  await waitUntil("seeded C2S Repel actor has a ready in-range drawn view", async () => {
    const state = await actor.metricsState();
    const metrics = state.uiMetrics || {};
    const remote = (metrics.remotePlayers || []).find((candidate) =>
      participantIds.includes(candidate.id) && candidate.id !== playerId
    );
    const separation = Math.hypot(
      metrics.playerX - remote?.positionX,
      metrics.playerY - remote?.positionY,
    );
    if (
      metrics.combatViewTimeline?.weaponReady !== true ||
      metrics.playerAlive !== true ||
      metrics.serverPlayerAlive !== true ||
      metrics.inSafeZone !== false ||
      !Number.isInteger(remote?.lifeSeq) ||
      ![
        metrics.playerX,
        metrics.playerY,
        remote.positionX,
        remote.positionY,
        separation,
      ].every(Number.isFinite) ||
      !(separation < 120)
    ) {
      return false;
    }
    readyInputBoundary = {
      clientInputSeq: metrics.clientInputSeq,
      serverTick: metrics.latestServerTick,
      actor: { x: metrics.playerX, y: metrics.playerY },
      remote: {
        id: remote.id,
        lifeSeq: remote.lifeSeq,
        x: remote.positionX,
        y: remote.positionY,
      },
      separation,
      combatViewTimeline: metrics.combatViewTimeline,
    };
    return true;
  }, 10_000);
  await actor.press("q", 80);
  let initialRepelProof = null;
  await waitUntil("seeded hostile Repel delivery and receipt recovery", async () => {
    const [delivery, peerMetrics] = await Promise.all([
      peer.evaluate("window.__zubspaceRepelDeliveryImpairmentStateForTest?.()"),
      peer.metricsState(),
    ]);
    const local = peerMetrics.uiMetrics?.localGameplayState;
    if (
      delivery?.dropped !== 1 ||
      delivery?.recovered !== 1 ||
      delivery?.receiptDropped !== 1 ||
      delivery?.receiptSeen < 2 ||
      local?.consumedRepelDeliveryGenerations < 1 ||
      local?.pendingRepelApplicationGeneration != null
    ) {
      return false;
    }
    initialRepelProof = {
      delivery,
      consumedGenerations: local.consumedRepelDeliveryGenerations,
      framesSeen: local.repelDeliveryFramesSeen,
      framesApplied: local.repelDeliveryFramesApplied,
      pendingGeneration: local.pendingRepelApplicationGeneration,
    };
    return true;
  }, 8_000);

  // Real Chrome keyboard events drive ordinary QWASD movement while the
  // WebRTC bridge loses and reorders only outbound input datagrams. Q also
  // exercises the authoritative hostile-repel event/rebase path on the peer.
  await Promise.all([
    (async () => {
      // Keep the scored window long enough to retain at least 100
      // stable-active samples per real participant while exact raw-f32 replay
      // repairs deliberately omitted and reordered input datagrams.
      for (let cycle = 0; cycle < 10; cycle += 1) {
        await actor.holdKeys(["w", "a"], 500);
        await actor.holdKeys(["w", "d"], 500);
        await actor.hold("s", 300);
        await actor.press("q", 80);
        await observeActorDistance();
      }
    })(),
    (async () => {
      // Exercise the peer independently without letting a strong impaired
      // actor result mask a weak client clock. Turn right, apply real reverse
      // thrust against the outward Q velocity, then restore the heading. The
      // target still reaches the nearby Chaos geometry before braking, so the
      // scored path covers repel delivery, a high-speed wall bounce, and
      // post-collision recovery rather than steering around the collision.
      // The ordinary peer publishes at the normal 10 Hz position cadence,
      // unlike the impaired actor whose recovered position can replay many
      // logical inputs. Keep this branch long enough to earn the same
      // per-participant 100-sample floor on both Chrome and Firefox.
      for (let cycle = 0; cycle < 16; cycle += 1) {
        await peer.hold("d", 400);
        await peer.hold("s", 300);
        await peer.hold("a", 400);
      }
    })(),
  ]);
  const impairmentDisabled = await actor.evaluate(
    "window.__zubspaceSetC2sInputImpairmentForTest?.(false) === false",
  );
  if (!impairmentDisabled) {
    throw new Error("C2S input impairment could not be disabled for tail recovery");
  }
  await sleep(750);

  // Keep the complete packet evidence, but use the existing compressed
  // export where available. Raw CDP object transfer of all rings can block
  // the driver long enough to exhaust the live rolling input window.
  const final = await (actor.artifactState?.() ?? actor.pageState());
  const peerFinal = await (peer.artifactState?.() ?? peer.pageState());
  const repelDeliveryImpairment = await peer.evaluate(
    "window.__zubspaceRepelDeliveryImpairmentStateForTest?.()",
  );
  const impairment = subtractC2sInputStats(c2sInputStats(final), beforeStats);
  const packetEvidence = c2sImpairmentPacketEvidence(final, startedAt);
  const {
    playerDeviation, globalDeviation, participantDeviations, replaySteps,
    missingInputRejects, inputWitnessRejects, positionAdmissionRejects,
    inputRecoveryTimeouts, snapbackRejects,
  } = await c2sRecoveryLogSummaryAsync({
    logText: serverLogSince(server, logOffset), playerId, participantIds,
  });
  const failedParticipant = Object.entries(participantDeviations).find(([, summary]) =>
    summary.stableActiveInput.samples < 100 ||
    summary.stableActiveInput.underTwoRatio == null ||
    summary.stableActiveInput.underTwoRatio < 0.95 ||
    summary.missingInputSamples !== 0 ||
    summary.invalidInputSamples !== 0 ||
    summary.missingInputDiagnostics !== 0 ||
    summary.unclassifiedSamples !== 0
  );
  const snapbacksAfter = movementSnapbackSummary(
    await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json(),
  );
  const distance = Math.hypot(
    (final.uiMetrics?.playerX ?? startPosition.x) - startPosition.x,
    (final.uiMetrics?.playerY ?? startPosition.y) - startPosition.y,
  );
  const peerRepelState = peerFinal.uiMetrics?.localGameplayState;

  if (
    impairment.seen < 100 ||
    impairment.dropped < 1 ||
    impairment.delayed < 1 ||
    impairment.released < 1 ||
    impairment.duplicated < 1 ||
    impairment.expired !== 0 ||
    packetEvidence.dropped < 1 ||
    packetEvidence.delayed < 1 ||
    packetEvidence.released < 1 ||
    packetEvidence.duplicated < 1 ||
    packetEvidence.overtaken < 1 ||
    replaySteps.multiStepSamples < 1 ||
    replaySteps.maxSteps < 2 ||
    playerDeviation.missingInputSamples !== 0 ||
    playerDeviation.invalidInputSamples !== 0 ||
    playerDeviation.missingInputDiagnostics !== 0 ||
    playerDeviation.unclassifiedSamples !== 0 ||
    failedParticipant != null ||
    globalDeviation.stableActiveInput.samples < 200 ||
    globalDeviation.stableActiveInput.underTwoRatio == null ||
    globalDeviation.stableActiveInput.underTwoRatio < 0.95 ||
    globalDeviation.missingInputSamples !== 0 ||
    globalDeviation.invalidInputSamples !== 0 ||
    globalDeviation.missingInputDiagnostics !== 0 ||
    globalDeviation.unclassifiedSamples !== 0 ||
    globalDeviation.bouncedSamples < 1 ||
    globalDeviation.repelActiveSamples < 1 ||
    Object.values(inputWitnessRejects).some((count) => count !== 0) ||
    Object.values(positionAdmissionRejects).some((count) => count !== 0) ||
    inputRecoveryTimeouts !== 0 ||
    snapbackRejects !== 0 ||
    snapbacksBefore.total !== 0 ||
    snapbacksAfter.total !== 0 ||
    snapbacksAfter.total !== snapbacksBefore.total ||
    !repelDeliveryImpairment?.configured ||
    repelDeliveryImpairment.dropped !== 1 ||
    repelDeliveryImpairment.recovered !== 1 ||
    !repelDeliveryImpairment.receiptConfigured ||
    repelDeliveryImpairment.receiptDropped !== 1 ||
    repelDeliveryImpairment.receiptSeen < 2 ||
    peerRepelState?.pendingRepelApplicationGeneration != null ||
    // The fault seam drops the first private frame before Rust receives it.
    // The scored keyboard window can legitimately outlive RepelTime and
    // activate more than one generation, so prove exact per-generation
    // application and account for every Rust-visible retry instead of
    // assuming the whole soak contains exactly one logical Repel.
    peerRepelState?.consumedRepelDeliveryGenerations < 1 ||
    peerRepelState?.repelDeliveryFramesApplied !==
      peerRepelState?.consumedRepelDeliveryGenerations ||
    peerRepelState?.repelDeliveryFramesSeen !==
      peerRepelState?.repelDeliveryFramesApplied +
        peerRepelState?.repelDeliveryPendingRetries +
        peerRepelState?.repelDeliveryConsumedRetries +
        peerRepelState?.repelDeliveryLifeRejects +
        peerRepelState?.repelDeliveryTimingRejects +
        peerRepelState?.repelDeliveryApplicationRejects ||
    peerRepelState?.repelDeliveryLifeRejects !== 0 ||
    peerRepelState?.repelDeliveryTimingRejects !== 0 ||
    peerRepelState?.repelDeliveryApplicationRejects !== 0 ||
    // Duplicate application markers now prompt an idempotent replay of the
    // original reliable receipt. The deliberately dropped first copy must be
    // recovered while the server-accepted logical suffix is still live, so
    // the browser never has to expire provisional authority locally.
    peerRepelState?.repelDeliveryExpiredApplicationCancellations !== 0 ||
    peerRepelState?.repelDeliveryExpiredReceiptsRecovered !== 0 ||
    peerRepelState?.expiredRepelApplications !== 0 ||
    // A queued private retry is no longer mandatory after application. The
    // server advances to reliable receipt recovery at that boundary, proved
    // by the dropped/seen receipt copies and immutable accepted endpoint.
    !Number.isInteger(peerRepelState?.repelDeliveryLastAcceptedEndSequence) ||
    !Number.isInteger(peerRepelState?.repelDeliveryLastAcceptedSteps) ||
    peerRepelState.repelDeliveryLastAcceptedSteps <= 0 ||
    peerRepelState.repelDeliveryLastAcceptedSteps >
      peerRepelState.repelDeliveryLastOfferedSteps ||
    !(maxActorDistance > 0.5)
  ) {
    throw new Error(`C2S input recovery gate failed: ${JSON.stringify({
      impairment,
      packetEvidence,
      replaySteps,
      playerDeviation,
      participantDeviations,
      failedParticipant,
      globalDeviation,
      missingInputRejects,
      inputWitnessRejects,
      positionAdmissionRejects,
      inputRecoveryTimeouts,
      snapbackRejects,
      snapbacksBefore,
      snapbacksAfter,
      repelFixture,
      readyInputBoundary,
      initialRepelProof,
      repelDeliveryImpairment,
      peerRepelApplication: peerRepelState,
      startPosition,
      finalPosition: {
        x: final.uiMetrics?.playerX,
        y: final.uiMetrics?.playerY,
      },
      distance,
      maxActorDistance,
    })}`);
  }

  const securityAudit = assertSecurityRejectAudit(server, logOffset, []);
  return {
    controls: ["q", "w", "a", "s", "d"],
    impairment,
    packetEvidence,
    replaySteps,
    missingInputRejects,
    inputWitnessRejects,
    positionAdmissionRejects,
    inputRecoveryTimeouts,
    snapbackRejects,
    movementSnapbacks: {
      before: snapbacksBefore.total,
      after: snapbacksAfter.total,
      delta: snapbacksAfter.total - snapbacksBefore.total,
    },
    securityAudit,
    repelFixture,
    readyInputBoundary,
    initialRepelProof,
    repelDeliveryImpairment,
    peerRepelApplication: {
      pendingGeneration:
        peerFinal.uiMetrics?.localGameplayState?.pendingRepelApplicationGeneration,
      consumedGenerations:
        peerRepelState?.consumedRepelDeliveryGenerations,
      deferredWarpAcks:
        peerRepelState?.deferredWarpPickupAcks,
      framesSeen: peerRepelState?.repelDeliveryFramesSeen,
      framesApplied: peerRepelState?.repelDeliveryFramesApplied,
      lifeRejects: peerRepelState?.repelDeliveryLifeRejects,
      timingRejects: peerRepelState?.repelDeliveryTimingRejects,
      applicationRejects: peerRepelState?.repelDeliveryApplicationRejects,
      expiredApplicationCancellations:
        peerRepelState?.repelDeliveryExpiredApplicationCancellations,
      expiredReceiptsRecovered:
        peerRepelState?.repelDeliveryExpiredReceiptsRecovered,
      expiredApplications: peerRepelState?.expiredRepelApplications,
      pendingRetries: peerRepelState?.repelDeliveryPendingRetries,
      consumedRetries: peerRepelState?.repelDeliveryConsumedRetries,
      lastSnapshotTick: peerRepelState?.repelDeliveryLastSnapshotTick,
      lastExpiryTick: peerRepelState?.repelDeliveryLastExpiryTick,
      lastObservedTick: peerRepelState?.repelDeliveryLastObservedTick,
      lastOfferedSteps: peerRepelState?.repelDeliveryLastOfferedSteps,
      lastAcceptedEndSequence:
        peerRepelState?.repelDeliveryLastAcceptedEndSequence,
      lastAcceptedSteps: peerRepelState?.repelDeliveryLastAcceptedSteps,
    },
    tailRecoveryWithoutImpairment: impairmentDisabled,
    movementDistance: distance,
    maxMovementDistance: maxActorDistance,
    scoredWindow: {
      logOffset,
      playerId,
      player: playerDeviation,
      participants: participantDeviations,
      global: globalDeviation,
    },
  };
}

function assertSnapshotLossEvidence({ server, startedAt, clients }) {
  const realizedLoss = simulatedLossSummary(server);
  if (realizedLoss.packetsDropped < 1) {
    throw new Error(`snapshot loss gate observed no realized server drops: ${JSON.stringify(realizedLoss)}`);
  }

  const clientEvidence = clients.map(({ label, start, final }) => {
    const discontinuities = receivedSnapshotDiscontinuities(final, startedAt);
    if (discontinuities.gapCount < 1) {
      throw new Error(`${label} observed no received snapshot-id discontinuity: ${JSON.stringify({
        realizedLoss,
        discontinuities,
        metrics: compactMetrics(final.uiMetrics),
      })}`);
    }
    const firstGapId = discontinuities.firstGap.toSnapshotId;
    const latestSnapshotId = final.uiMetrics?.latestSnapshotId;
    const lastAckedSnapshotId = final.uiMetrics?.lastAckedSnapshotId;
    if (
      !snapshotIdIsSameOrForward(latestSnapshotId, firstGapId) ||
      !snapshotIdIsSameOrForward(lastAckedSnapshotId, firstGapId) ||
      !snapshotIdIsSameOrForward(latestSnapshotId, start.uiMetrics?.latestSnapshotId) ||
      (final.uiMetrics?.remoteVisiblePlayers ?? 0) < 1
    ) {
      throw new Error(`${label} did not recover and acknowledge snapshots after loss: ${JSON.stringify({
        discontinuities,
        start: compactMetrics(start.uiMetrics),
        final: compactMetrics(final.uiMetrics),
      })}`);
    }
    return {
      ...discontinuities,
      startSnapshotId: start.uiMetrics?.latestSnapshotId ?? null,
      recoveredSnapshotId: latestSnapshotId,
      recoveredAckedSnapshotId: lastAckedSnapshotId,
    };
  });

  return { realizedLoss, clients: clientEvidence };
}

async function assertAoiLeaveReenterUnderLoss({ actor, watcher, server, actorId }) {
  const [actorBefore, watcherBefore] = await Promise.all([
    actor.pageState(),
    watcher.pageState(),
  ]);
  const startSnapshotId = watcherBefore.uiMetrics?.authoritativeSnapshotId;
  const actorX = actorBefore.uiMetrics?.playerX;
  const actorY = actorBefore.uiMetrics?.playerY;
  const actorOrientation = actorBefore.uiMetrics?.playerOrientation;
  let watcherX = watcherBefore.uiMetrics?.playerX;
  let watcherY = watcherBefore.uiMetrics?.playerY;
  const watchedMembership = (watcherBefore.uiMetrics?.authoritativeSnapshotActors || [])
    .find((player) => player.id === actorId);
  let watchedActor = watchedMembership && Number.isInteger(watchedMembership.lifeSeq)
    ? { playerId: actorId, lifeSeq: watchedMembership.lifeSeq }
    : null;
  const latestAuthoritativeRespawn = parseServerRespawnEvents(server.logs)
    .filter((event) => event.playerId === actorId)
    .at(-1);
  const expectedActorLifeSeq = latestAuthoritativeRespawn?.lifeSeq ?? watchedActor?.lifeSeq;
  if (
    !Number.isFinite(startSnapshotId) ||
    !Number.isFinite(actorX) ||
    !Number.isFinite(actorY) ||
    !Number.isFinite(actorOrientation) ||
    !Number.isFinite(watcherX) ||
    !Number.isFinite(watcherY) ||
    !Number.isInteger(expectedActorLifeSeq) ||
    expectedActorLifeSeq <= 0
  ) {
    throw new Error(`lossy AOI lifecycle missing initial remote actor: ${JSON.stringify({
      actorId,
      actor: actorBefore.uiMetrics,
      watcher: watcherBefore.uiMetrics,
    })}`);
  }

  // Keep the actor in a live ship and drive it through both AOI boundaries
  // using the same real keyboard movement a player uses. Reverse thrust
  // brings it back and forward thrust brakes it near the watcher, avoiding
  // spectator/roster transitions that would test a different reliable path.
  const distanceFromWatcher = (metrics) => Math.hypot(
    (metrics?.playerX ?? actorX) - watcherX,
    (metrics?.playerY ?? actorY) - watcherY,
  );
  // Chaos Zone's spawn has a long open corridor at this slight east/south
  // heading. Keep the tolerance narrow: headings a few hundredths north of
  // it encounter the nearby wall before crossing the mid-AOI radius. In
  // particular, the next discrete browser heading at 0.30 enters that wall
  // and cannot retrace to the watcher, so bind the proof to the 0.27-0.29
  // corridor rather than accepting both direction bins.
  const eastOrientation = 0.28;
  const corridorOrientationTolerance = 0.015;
  const protocolDirectionForOrientation = (orientation) =>
    Math.floor(Math.fround(((orientation % 1) + 1) % 1) * 40) % 40;
  const eastProtocolDirection = protocolDirectionForOrientation(eastOrientation);
  const orientationDistance = (orientation) => {
    if (!Number.isFinite(orientation)) return Number.POSITIVE_INFINITY;
    return Math.abs(((orientation - eastOrientation + 0.5) % 1 + 1) % 1 - 0.5);
  };
  const alignClient = async (client, targetOrientation, initialState, label) => {
    const targetDirection = protocolDirectionForOrientation(targetOrientation);
    let state = initialState;
    for (let attempt = 0; attempt < 40; attempt += 1) {
      const orientation = state.uiMetrics?.playerOrientation;
      const currentDirection = Number.isInteger(state.uiMetrics?.playerProtocolDirection)
        ? state.uiMetrics.playerProtocolDirection
        : protocolDirectionForOrientation(orientation);
      if (currentDirection === targetDirection) return state;
      let signedDirectionSteps = (targetDirection - currentDirection + 40) % 40;
      if (signedDirectionSteps >= 20) signedDirectionSteps -= 40;
      // A 60ms Chaos turn can cross two protocol bins and oscillate around an
      // adjacent target forever. Use an ordinary shorter key pulse for the
      // final two bins, then observe one released render frame before reading
      // the exact direction used by the input encoder.
      await client.press(
        signedDirectionSteps >= 0 ? "ArrowRight" : "ArrowLeft",
        Math.abs(signedDirectionSteps) <= 2 ? 24 : 60,
      );
      await sleep(30);
      state = await client.metricsState();
    }
    throw new Error(`${label} could not align to the open corridor: ${JSON.stringify({
      targetOrientation,
      targetDirection,
      metrics: state.uiMetrics,
    })}`);
  };

  // The corrected single-pair reference-game collision path reaches the east
  // corridor wall roughly 2,600 px from the common spawn, while AOI
  // hysteresis releases an existing mid-tier actor only beyond ~2,816 px.
  // Move the watcher a bounded distance west first, using ordinary keyboard
  // movement, so the actor can cross the real hysteresis boundary without
  // asking the fixture to tunnel through that wall.
  const watcherStaging = await stageAoiWatcher({
    initial: (await watcher.metricsState()).uiMetrics,
    alignWest: (metrics) => alignClient(watcher, 0.75, { uiMetrics: metrics }, "lossy AOI watcher"),
    holdUntil: (predicate) => watcher.holdUntil(
      "ArrowUp",
      "lossy AOI watcher creates bounded westward separation",
      predicate,
      5_000,
    ),
    brake: () => brakeClientVelocity(watcher, 8_000),
  });
  watcherX = watcherStaging.metrics.playerX;
  watcherY = watcherStaging.metrics.playerY;

  const turnedState = await alignClient(actor, eastOrientation, actorBefore, "live AOI actor");
  if (
    turnedState.uiMetrics?.playerProtocolDirection !== eastProtocolDirection ||
    orientationDistance(turnedState.uiMetrics?.playerOrientation) > 0.025
  ) {
    throw new Error(`live AOI actor could not align with the east corridor: ${JSON.stringify({
      actorOrientation,
      eastOrientation,
      eastProtocolDirection,
      turned: turnedState.uiMetrics,
    })}`);
  }
  // The page state captured before turning can still be the last applied
  // snapshot for the preceding life after the combat cycles. Bind the AOI
  // proof to the exact current generation only after a strictly-forward
  // authoritative snapshot exposes that actor generation. From this point on
  // a different life under the reusable player id is not the watched actor.
  await waitUntil("lossy AOI watcher confirms the current actor generation", async () => {
    const metrics = (await watcher.metricsState()).uiMetrics || {};
    const snapshotAdvance = snapshotIdForwardDistance(
      metrics.authoritativeSnapshotId,
      startSnapshotId,
    );
    const currentMembership = (metrics.authoritativeSnapshotActors || [])
      .find((player) => player.id === actorId);
    if (
      snapshotAdvance != null &&
      snapshotAdvance > 0 &&
      snapshotAdvance <= 0x7fff &&
      currentMembership &&
      Number.isInteger(currentMembership.lifeSeq) &&
      currentMembership.lifeSeq === expectedActorLifeSeq
    ) {
      watchedActor = { playerId: actorId, lifeSeq: currentMembership.lifeSeq };
      return true;
    }
    return false;
  }, 12_000);
  const isWatchedActor = (player) =>
    player.id === watchedActor.playerId && player.lifeSeq === watchedActor.lifeSeq;
  const outboundTraceStart = {
    at: Date.now(),
    index: watcherBefore.metricsTrace?.length ?? 0,
  };
  // Presentation smoothness belongs to the 2,048px Near tier. A headed debug
  // browser can travel roughly another 800px while the hold predicate and
  // reverse-thrust brake complete, so begin braking with explicit headroom.
  // The later leg still crosses the 2,560px Mid boundary and proves removal.
  const [outboundActorStart, outboundWatcherStart] = await Promise.all([
    actor.metricsState(),
    watcher.metricsState(),
  ]);
  const outboundProgress = aoiOutboundProgress(
    outboundActorStart.uiMetrics,
    outboundWatcherStart.uiMetrics,
  );
  watcherX = outboundProgress.watcher.x;
  watcherY = outboundProgress.watcher.y;
  const outboundNearBrakeStartDistance = outboundProgress.brakeStartDistance;
  const outboundCruiseState = await actor.holdUntil(
    "ArrowUp",
    "live AOI actor makes visible outbound progress",
    (state) => outboundProgress.reached(state.uiMetrics),
    15_000,
  );
  const outboundVelocity = {
    x: outboundCruiseState.uiMetrics?.playerVelocityX,
    y: outboundCruiseState.uiMetrics?.playerVelocityY,
  };
  if (!Number.isFinite(outboundVelocity.x) || !Number.isFinite(outboundVelocity.y)) {
    throw new Error(`outbound AOI actor has no finite velocity: ${JSON.stringify(outboundCruiseState.uiMetrics)}`);
  }
  const outboundBrake = await brakeClientVelocity(actor, 8_000);
  if (!outboundBrake.braked) {
    throw new Error(`outbound AOI actor could not stop before the far crossing: ${JSON.stringify({
      outboundVelocity,
      outboundBrake,
    })}`);
  }
  const outboundStoppedDistance = distanceFromWatcher(outboundBrake.metrics);
  if (outboundStoppedDistance >= 2_048) {
    throw new Error(`outbound AOI actor did not stop inside the Near presentation tier: ${JSON.stringify({
      outboundNearBrakeStartDistance,
      outboundStoppedDistance,
      outboundVelocity,
      outboundBrake,
      watcher: { x: watcherX, y: watcherY },
    })}`);
  }
  await alignClient(
    actor,
    eastOrientation,
    { uiMetrics: outboundBrake.metrics },
    "outbound AOI actor",
  );
  await waitUntil("lossy AOI outbound display converges before leave", async () => {
    const metrics = (await watcher.metricsState()).uiMetrics || {};
    const actorRemote = (metrics.remotePlayers || []).find(isWatchedActor);
    return (
      actorRemote &&
      Number.isFinite(actorRemote.positionX) &&
      Number.isFinite(actorRemote.positionY) &&
      Number.isFinite(actorRemote.displayX) &&
      Number.isFinite(actorRemote.displayY) &&
      Math.hypot(
        actorRemote.displayX - actorRemote.positionX,
        actorRemote.displayY - actorRemote.positionY,
      ) <= 32
    );
  }, 12_000);
  const watcherOutboundState = await watcher.pageState();
  const outboundMotion = assertRemoteMotionRecoversUnderLoss(
    "lossy AOI outbound",
    watcherOutboundState,
    outboundTraceStart,
    watchedActor,
  );

  await actor.holdUntil(
    "ArrowUp",
    "live AOI actor moves beyond the mid radius",
    (state) => state.uiMetrics?.playerAlive === true && distanceFromWatcher(state.uiMetrics) > 2_800,
    20_000,
  );
  let hidden = null;
  await waitUntil("lossy AOI actor leaves watcher snapshots", async () => {
    const [actorState, watcherState] = await Promise.all([
      actor.metricsState(),
      watcher.metricsState(),
    ]);
    const metrics = watcherState.uiMetrics || {};
    const snapshotAdvance = snapshotIdForwardDistance(
      metrics.authoritativeSnapshotId,
      startSnapshotId,
    );
    const actorRemote = (metrics.remotePlayers || []).find(isWatchedActor);
    const actorMembership = (metrics.authoritativeSnapshotActors || []).find(isWatchedActor);
    if (
      snapshotAdvance != null &&
      snapshotAdvance > 0 &&
      actorState.uiMetrics?.playerAlive === true &&
      distanceFromWatcher(actorState.uiMetrics) > 2_560 &&
      actorMembership == null
    ) {
      hidden = {
        snapshotId: metrics.authoritativeSnapshotId,
        snapshotAdvance,
        actorX: actorState.uiMetrics?.playerX ?? null,
        actorY: actorState.uiMetrics?.playerY ?? null,
        renderProxyRetained: actorRemote != null,
      };
      return true;
    }
    return false;
  }, 15_000);
  const returnedState = await actor.holdUntil(
    "ArrowDown",
    "live AOI actor returns inside the near radius",
    (state) => state.uiMetrics?.playerAlive === true && distanceFromWatcher(state.uiMetrics) < 1_000,
    25_000,
  );
  const brakeStartVelocity = {
    x: returnedState.uiMetrics?.playerVelocityX,
    y: returnedState.uiMetrics?.playerVelocityY,
  };
  if (!Number.isFinite(brakeStartVelocity.x) || !Number.isFinite(brakeStartVelocity.y)) {
    throw new Error(`returning AOI actor has no finite velocity: ${JSON.stringify(returnedState.uiMetrics)}`);
  }
  const brakeStartSpeed = Math.hypot(brakeStartVelocity.x, brakeStartVelocity.y);
  let brakeReleaseState = returnedState;
  if (brakeStartSpeed >= 20) {
    // A near-zero speed exists for only a few simulation frames and can fall
    // between 100 ms browser polls. Once the velocity projection reverses it
    // remains observable, so release thrust on that persistent transition.
    brakeReleaseState = await actor.holdUntil(
      "ArrowUp",
      "returning AOI actor reverses velocity while braking",
      (state) => {
        const velocityX = state.uiMetrics?.playerVelocityX;
        const velocityY = state.uiMetrics?.playerVelocityY;
        return (
          state.uiMetrics?.playerAlive === true &&
          Number.isFinite(velocityX) &&
          Number.isFinite(velocityY) &&
          velocityX * brakeStartVelocity.x + velocityY * brakeStartVelocity.y <= 0
        );
      },
      5_000,
    );
  }
  let reentered = null;
  await waitUntil("lossy AOI actor re-enters watcher snapshots", async () => {
    const state = await watcher.metricsState();
    const metrics = state.uiMetrics || {};
    const actorRemote = (metrics.remotePlayers || []).find(isWatchedActor);
    const actorMembership = (metrics.authoritativeSnapshotActors || []).find(isWatchedActor);
    const snapshotAdvance = snapshotIdForwardDistance(
      metrics.authoritativeSnapshotId,
      hidden.snapshotId,
    );
    if (
      snapshotAdvance != null &&
      snapshotAdvance > 0 &&
      actorMembership &&
      actorRemote &&
      Number.isFinite(actorRemote.positionX) &&
      Number.isFinite(actorRemote.positionY) &&
      Number.isFinite(actorRemote.displayX) &&
      Number.isFinite(actorRemote.displayY)
    ) {
      reentered = {
        snapshotId: metrics.authoritativeSnapshotId,
        snapshotAdvance,
        positionX: actorRemote.positionX,
        positionY: actorRemote.positionY,
      };
      return true;
    }
    return false;
  }, 25_000);

  return {
    actorId,
    actorLifeSeq: watchedActor.lifeSeq,
    awayKey: "ArrowUp",
    returnKey: "ArrowDown",
    eastOrientation,
    initialOrientation: actorOrientation,
    outboundOrientation: turnedState.uiMetrics?.playerOrientation ?? null,
    outboundMotion,
    outboundStaging: {
      watcher: {
        origin: watcherStaging.origin,
        legs: watcherStaging.legs,
        start: outboundProgress.watcher,
      },
      actorStart: outboundProgress.actorStart,
      initialDistance: outboundProgress.initialDistance,
      brakeStartDistance: outboundNearBrakeStartDistance,
      minimumActorTravel: outboundProgress.minimumActorTravel,
      cruise: outboundProgress.measure(outboundCruiseState.uiMetrics),
      stoppedDistance: outboundStoppedDistance,
    },
    brake: {
      startSpeed: brakeStartSpeed,
      startVelocity: brakeStartVelocity,
      releaseVelocity: {
        x: brakeReleaseState.uiMetrics?.playerVelocityX ?? null,
        y: brakeReleaseState.uiMetrics?.playerVelocityY ?? null,
      },
    },
    startSnapshotId,
    hidden,
    reentered,
  };
}

function summarizeClient(state) {
  return summarizePacketLog(mergedPacketLog(state));
}

function compactSummary(summary) {
  return {
    selfId: summary.selfId,
    rosters: summary.rosters,
    rosterNames: summary.rosterNames,
    receivedDeaths: (summary.receivedDeaths || []).slice(-8),
    sentPositionCount: summary.sentPositionCount,
    receivedPositionCount: summary.receivedPositionCount,
    receivedTimestampGaps: summary.receivedTimestampGaps,
    sentWeaponCount: (summary.sentPositions || []).filter((entry) => entry.weapon).length,
    receivedWeaponCount: (summary.receivedPositionUpdates || []).filter((entry) => entry.weapon).length,
    receivedReliablePayloadTypes: (summary.receivedReliablePayloadTypes || []).slice(-32),
    sentReliablePayloadTypes: (summary.sentReliablePayloadTypes || []).slice(-32),
  };
}

function appliedSnapshotGapSummary(state) {
  const sampleTimes = [];
  let previousId = null;
  for (const metrics of state.metricsTrace || []) {
    const snapshotId = metrics.latestSnapshotId;
    if (snapshotId == null || snapshotId === previousId) continue;
    previousId = snapshotId;
    sampleTimes.push(metrics.at);
  }
  const completedInstrumentationReads = [
    ...(state.instrumentationReadWindows || []),
    ...instrumentationReadWindows,
  ].filter(
    (window) =>
      Number.isFinite(window?.startedAt) &&
      Number.isFinite(window?.endedAt) &&
      window.endedAt >= window.startedAt,
  );
  const values = sampleTimes
    .slice(1)
    .flatMap((at, index) => {
      const previousAt = sampleTimes[index];
      const crossesInstrumentationRead = completedInstrumentationReads.some(
        (window) => previousAt <= window.startedAt && at >= window.endedAt,
      );
      return crossesInstrumentationRead ? [] : [at - previousAt];
    })
    .filter((gap) => Number.isFinite(gap) && gap >= 0)
    .sort((a, b) => a - b);
  const percentile = (fraction) =>
    values[Math.min(values.length - 1, Math.floor(values.length * fraction))] ?? null;
  return {
    count: values.length,
    p50Ms: percentile(0.50),
    p95Ms: percentile(0.95),
    maxMs: values.at(-1) ?? null,
  };
}

function assertSnapshotCadence(label, summary, state, lossRecovery = false) {
  let gaps = summary.receivedTimestampGaps || {};
  if (
    !Number.isFinite(gaps.p50Ms) ||
    !Number.isFinite(gaps.p95Ms) ||
    (gaps.count ?? 0) < 5
  ) {
    // The packet logger retains the first and latest bounded windows. Once
    // exact-base decoding landed, a gap between those windows intentionally
    // cannot be synthesized, and a small startup-only sample is not
    // representative. Fall back to the client's applied-snapshot trace, which
    // is the stronger measure of successful reconstruction cadence.
    gaps = appliedSnapshotGapSummary(state);
  }
  if (!Number.isFinite(gaps.p50Ms) || !Number.isFinite(gaps.p95Ms)) {
    throw new Error(`${label} did not capture remote snapshot timestamp gaps: ${JSON.stringify(summary)}`);
  }
  // The no-loss replay keeps the strict 60 Hz local-RTC contract. The seeded
  // 20% loss replay must contain discontinuities by design, so retain the
  // median bound while allowing a bounded multi-frame recovery tail.
  // Debug authority replays retain the exact quantization-cell branch proof
  // and verbose RTC logs, so the debug-only tail can reach a bounded 250 ms
  // even though the median must remain within three frames. Release transport
  // throughput is covered separately by the retained capacity/Compose gates;
  // the seeded loss gate keeps its tighter scenario-specific recovery tail.
  const p95LimitMs = lossRecovery ? 150 : 250;
  if (gaps.p50Ms > 45 || gaps.p95Ms > p95LimitMs) {
    throw new Error(`${label} remote snapshot cadence is too coarse for local RTC play: ${JSON.stringify(gaps)}`);
  }
}

function assertNoClientDeathRequests(label, summary) {
  if ((summary.sentReliablePayloadTypes || []).includes(CLIENT_DEATH_REQUEST)) {
    throw new Error(`${label} sent legacy client death request: ${JSON.stringify(compactSummary(summary))}`);
  }
}

function assertStructuredServerLifecycle({ server, cycles, counterDamage }) {
  const damageEvents = parseServerDamageEvents(server.logs);
  const deathEvents = parseServerDeathEvents(server.logs);
  const respawnEvents = parseServerRespawnEvents(server.logs);
  const projectileEvents = parseServerProjectileEvents(server.logs);
  const projectileSpawns = new Map(
    projectileEvents
      .filter((event) => event.type === "server_projectile_spawned")
      .map((event) => [event.projectileId, event]),
  );
  const projectileHits = new Map(
    projectileEvents
      .filter((event) => event.type === "server_projectile_hit")
      .map((event) => [event.projectileId, event]),
  );

  for (const damage of damageEvents) {
    const spawn = projectileSpawns.get(damage.projectileId);
    const hit = projectileHits.get(damage.projectileId);
    if (!spawn || !hit) {
      throw new Error(`structured server projectile lifecycle missing for damage: ${JSON.stringify({
        damage,
        spawn,
        hit,
        projectileEvents: projectileEvents.slice(-40),
      })}`);
    }
    if (
      spawn.ownerId !== damage.shooterId ||
      spawn.ownerLifeSeq !== damage.shooterLifeSeq ||
      hit.ownerId !== damage.shooterId ||
      hit.ownerLifeSeq !== damage.shooterLifeSeq ||
      (
        damage.kind !== "splash" &&
        (hit.victimId !== damage.victimId || hit.victimLifeSeq !== damage.victimLifeSeq)
      )
    ) {
      throw new Error(`structured projectile lifecycle disagrees with damage: ${JSON.stringify({
        damage,
        spawn,
        hit,
      })}`);
    }
  }

  for (const cycle of cycles) {
    const death = deathEvents.find((event) => event.damageSeq === cycle.kill.damageSeq);
    if (!death) {
      throw new Error(`missing structured death event for cycle ${cycle.cycle}: ${JSON.stringify({
        kill: cycle.kill,
        deathEvents,
      })}`);
    }
    if (
      death.projectileId !== cycle.kill.projectileId ||
      death.killerId !== cycle.kill.shooterId ||
      death.victimId !== cycle.kill.victimId ||
      death.victimLifeSeq !== cycle.kill.victimLifeSeq ||
      death.victimNextLifeSeq !== cycle.postRespawnDamage.victimLifeSeq ||
      death.energyAfter !== 0 ||
      death.respawnDelayMs < 1800
    ) {
      throw new Error(`structured death event disagrees with cycle ${cycle.cycle}: ${JSON.stringify({
        kill: cycle.kill,
        postRespawnDamage: cycle.postRespawnDamage,
        death,
      })}`);
    }

    const respawn = respawnEvents.find((event) =>
      event.playerId === cycle.kill.victimId &&
      event.lifeSeq === cycle.postRespawnDamage.victimLifeSeq
    );
    if (!respawn) {
      throw new Error(`missing structured respawn event for cycle ${cycle.cycle}: ${JSON.stringify({
        postRespawnDamage: cycle.postRespawnDamage,
        respawnEvents,
      })}`);
    }
    if (
      respawn.energy !== 1700 ||
      respawn.maxEnergy !== 1700 ||
      respawn.inputBits !== 0 ||
      respawn.inputDirection !== 0 ||
      respawn.inputHistoryLen !== 1 ||
      respawn.serverTick > cycle.postRespawnDamage.serverTick
    ) {
      throw new Error(`structured respawn event disagrees with cycle ${cycle.cycle}: ${JSON.stringify({
        postRespawnDamage: cycle.postRespawnDamage,
        respawn,
      })}`);
    }
  }

  if (counterDamage?.event) {
    const counterSpawn = projectileSpawns.get(counterDamage.event.projectileId);
    const counterHit = projectileHits.get(counterDamage.event.projectileId);
    if (!counterSpawn || !counterHit) {
      throw new Error(`missing structured counterfire projectile lifecycle: ${JSON.stringify({
        counterDamage: counterDamage.event,
        counterSpawn,
        counterHit,
      })}`);
    }
  }

  return {
    damageEvents: damageEvents.length,
    deathEvents: deathEvents.length,
    respawnEvents: respawnEvents.length,
    projectileSpawns: [...projectileSpawns.values()].length,
    projectileHits: [...projectileHits.values()].length,
  };
}

function compactMetrics(metrics) {
  if (!metrics) return null;
  return {
    selfPlayerId: metrics.selfPlayerId,
    playerShip: metrics.playerShip,
    selfRosterShip: metrics.selfRosterShip,
    playerFrequency: metrics.playerFrequency,
    selfWins: metrics.selfWins,
    selfLosses: metrics.selfLosses,
    clientInputSeq: metrics.clientInputSeq,
    latestServerTick: metrics.latestServerTick,
    latestSnapshotId: metrics.latestSnapshotId,
    authoritativeSnapshotId: metrics.authoritativeSnapshotId,
    authoritativeSnapshotActors: metrics.authoritativeSnapshotActors,
    authoritativeSnapshotActorTrace: metrics.authoritativeSnapshotActorTrace,
    authoritativeSnapshotActorStream: metrics.authoritativeSnapshotActorStream,
    remoteActorPresentation: metrics.remoteActorPresentation,
    latestSnapshotServerTick: metrics.latestSnapshotServerTick,
    latestSnapshotBaseId: metrics.latestSnapshotBaseId,
    lastAckedSnapshotId: metrics.lastAckedSnapshotId,
    lastAckedSnapshotServerTick: metrics.lastAckedSnapshotServerTick,
    selfCorrectionCount: metrics.selfCorrectionCount,
    lastSelfCorrectionReason: metrics.lastSelfCorrectionReason,
    remoteSnapCount: metrics.remoteSnapCount,
    lastRemoteSnapReason: metrics.lastRemoteSnapReason,
    remoteExtrapolatedFrames: metrics.remoteExtrapolatedFrames,
    playerEnergy: metrics.playerEnergy,
    playerPredictedEnergy: metrics.playerPredictedEnergy,
    localShipLabelEnergy: metrics.localShipLabelEnergy,
    visibleEnergyDigits: metrics.visibleEnergyDigits,
    playerMaxEnergy: metrics.playerMaxEnergy,
    serverPlayerEnergy: metrics.serverPlayerEnergy,
    playerAlive: metrics.playerAlive,
    serverPlayerAlive: metrics.serverPlayerAlive,
    playerRespawnTimer: metrics.playerRespawnTimer,
    playerRespawnSeq: metrics.playerRespawnSeq,
    playerX: metrics.playerX,
    playerY: metrics.playerY,
    playerVelocityX: metrics.playerVelocityX,
    playerVelocityY: metrics.playerVelocityY,
    playerOrientation: metrics.playerOrientation,
    repelTimer: metrics.localGameplayState?.repelTimer,
    repelSequenceStart: metrics.localGameplayState?.repelSequenceStart,
    repelSequenceEnd: metrics.localGameplayState?.repelSequenceEnd,
    pendingWeapon: metrics.pendingWeapon,
    remoteVisiblePlayers: metrics.remoteVisiblePlayers,
    remoteEnergy: metrics.remoteEnergy,
    remotePlayers: metrics.remotePlayers,
    remoteProjectileSpawns: metrics.remoteProjectileSpawns,
    remoteBombSpawns: metrics.remoteBombSpawns,
    remoteBulletSpawns: metrics.remoteBulletSpawns,
    remoteRepelSpawns: metrics.remoteRepelSpawns,
    lastNetworkDamageSeq: metrics.lastNetworkDamageSeq,
    lastNetworkDamageServerSeq: metrics.lastNetworkDamageServerSeq,
    lastNetworkDamageServerTick: metrics.lastNetworkDamageServerTick,
    lastNetworkDamageVictimLifeSeq: metrics.lastNetworkDamageVictimLifeSeq,
    lastNetworkDamageShooterLifeSeq: metrics.lastNetworkDamageShooterLifeSeq,
    lastNetworkDamagePlayerId: metrics.lastNetworkDamagePlayerId,
    lastNetworkDamageAttackerId: metrics.lastNetworkDamageAttackerId,
    lastNetworkDamageEnergyBefore: metrics.lastNetworkDamageEnergyBefore,
    lastNetworkDamageEnergy: metrics.lastNetworkDamageEnergy,
    lastNetworkDamageAmount: metrics.lastNetworkDamageAmount,
    lastNetworkDamageWeapon: metrics.lastNetworkDamageWeapon,
    lastNetworkDamageForSelf: metrics.lastNetworkDamageForSelf,
    lastSelfDamageSeq: metrics.lastSelfDamageSeq,
    lastSelfDamageServerSeq: metrics.lastSelfDamageServerSeq,
    lastSelfDamageServerTick: metrics.lastSelfDamageServerTick,
    lastSelfDamageVictimLifeSeq: metrics.lastSelfDamageVictimLifeSeq,
    lastSelfDamageShooterLifeSeq: metrics.lastSelfDamageShooterLifeSeq,
    lastSelfDamageAttackerId: metrics.lastSelfDamageAttackerId,
    lastSelfDamageEnergyBefore: metrics.lastSelfDamageEnergyBefore,
    lastSelfDamageEnergy: metrics.lastSelfDamageEnergy,
    lastSelfDamageAmount: metrics.lastSelfDamageAmount,
    lastSelfDamageWeapon: metrics.lastSelfDamageWeapon,
  };
}

function compactInputTrace(trace) {
  const entries = trace || [];
  return {
    count: entries.length,
    tail: entries.slice(-8).map((entry) => ({
      at: entry.at,
      label: entry.label,
      phase: entry.phase,
      code: entry.code,
      selfPlayerId: entry.selfPlayerId,
      clientInputSeq: entry.clientInputSeq,
      latestServerTick: entry.latestServerTick,
      latestSnapshotServerTick: entry.latestSnapshotServerTick,
      lastAckedSnapshotServerTick: entry.lastAckedSnapshotServerTick,
      pendingWeapon: entry.pendingWeapon,
      playerX: entry.playerX,
      playerY: entry.playerY,
      playerVelocityX: entry.playerVelocityX,
      playerVelocityY: entry.playerVelocityY,
      playerOrientation: entry.playerOrientation,
      playerEnergy: entry.playerEnergy,
      serverPlayerEnergy: entry.serverPlayerEnergy,
      remoteVisiblePlayers: entry.remoteVisiblePlayers,
      remotePlayers: (entry.remotePlayers || []).slice(0, 2).map((player) => ({
        id: player.id,
        energy: player.energy,
        serverEnergy: player.serverEnergy,
        displayX: player.displayX,
        displayY: player.displayY,
        positionX: player.positionX,
        positionY: player.positionY,
      })),
      lastNetworkDamageServerSeq: entry.lastNetworkDamageServerSeq,
      lastSelfDamageServerSeq: entry.lastSelfDamageServerSeq,
    })),
  };
}

function artifactLabel(label) {
  return String(label || "client").replace(/[^a-z0-9_-]+/gi, "-").toLowerCase();
}

async function saveBrowserE2eArtifacts({
  artifactDir,
  server,
  staticServer,
  urls = [],
  clients = [],
  extraLogs = {},
  extraScreenshots = {},
  provenance = null,
}) {
  return withPreparedBrowserArtifactStates(clients, async (captures) => {
    const atomicDirectory = createAtomicArtifactDirectory(artifactDir);
    const workingDirectory = atomicDirectory.temporary;
    const serverLogPath = path.join(workingDirectory, "server.log");
    const staticLogPath = path.join(workingDirectory, "static-server.json");
    const clientUrlsPath = path.join(workingDirectory, "client-urls.txt");
    const serverEventsPath = path.join(workingDirectory, "server-events.json");
    const serverDamageEventsPath = path.join(workingDirectory, "server-damage-events.json");
    const serverCollisionWitnessesPath = path.join(
      workingDirectory,
      "server-collision-witnesses.json",
    );
    const provenancePath = path.join(workingDirectory, "provenance.json");
    const artifactProvenance = provenance || await collectArtifactProvenanceAsync({
      ...buildProvenanceOptions,
      e2eScript: process.argv[1]
        ? path.relative(repoRoot, path.resolve(process.argv[1]))
        : "tests/e2e/combat_replay.mjs",
    });
    // Persist identity before any potentially failing diagnostic serialization.
    fs.writeFileSync(provenancePath, `${JSON.stringify(artifactProvenance, null, 2)}\n`);
    const capturedServerLogs = server.logs.slice();
    await writeLogChunksAsync(serverLogPath, capturedServerLogs);
    fs.writeFileSync(path.join(workingDirectory, "server-process.json"),
      JSON.stringify({
        ...captureServerProcessEvidence(server.child?.pid),
        launch: captureServerLaunchEvidence(server.child),
      }, null, 2));
    fs.writeFileSync(staticLogPath, JSON.stringify({
      port: staticServer?.port ?? null,
      url: staticServer?.port == null
        ? null
        : `http://${staticServer.host || "127.0.0.1"}:${staticServer.port}/index.html`,
      root: applicationRoot,
    }, null, 2));
    fs.writeFileSync(clientUrlsPath, urls.filter(Boolean).join("\n"));
    fs.writeFileSync(
      serverEventsPath,
      JSON.stringify(parseZubspaceEvents(capturedServerLogs), null, 2),
    );
    fs.writeFileSync(
      serverDamageEventsPath,
      JSON.stringify({
        damage: parseServerDamageEvents(capturedServerLogs),
        deaths: parseServerDeathEvents(capturedServerLogs),
        respawns: parseServerRespawnEvents(capturedServerLogs),
        projectiles: parseServerProjectileEvents(capturedServerLogs),
      }, null, 2),
    );
    fs.writeFileSync(
      serverCollisionWitnessesPath,
      JSON.stringify(
        parseZubspaceEvents(capturedServerLogs).filter(
          (entry) => entry?.event?.type === "server_collision_witness",
        ),
        null,
        2,
      ),
    );
    fs.writeFileSync(provenancePath, `${JSON.stringify(artifactProvenance, null, 2)}\n`);

    const extraLogPaths = [];
    for (const [name, content] of Object.entries(extraLogs)) {
      const filename = `${artifactLabel(name)}.log`;
      const logPath = path.join(workingDirectory, filename);
      await writeLogChunksAsync(logPath, content);
      extraLogPaths.push(filename);
    }

    const extraScreenshotPaths = [];
    for (const [name, base64] of Object.entries(extraScreenshots)) {
      const filename = `${artifactLabel(name)}.png`;
      fs.writeFileSync(path.join(workingDirectory, filename), Buffer.from(base64, "base64"), { flag: "wx" });
      extraScreenshotPaths.push(filename);
    }
    const qaTraceCaptureAudit = [];
    const browserConsoleAudits = [];
    const consoleFinalizers = [];
    for (const [index, { label, client }] of clients.entries()) {
      const prefix = artifactLabel(label);
      const consoleFile = `${prefix}-console.json`;
      const saveConsole = () => {
        const evidence = browserConsoleEvidence(client);
        fs.writeFileSync(path.join(workingDirectory, consoleFile),
          `${JSON.stringify(evidence, null, 2)}\n`);
        const { events: _events, ...audit } = evidence;
        return { label: prefix, ...audit };
      };
      extraLogPaths.push(consoleFile);
      consoleFinalizers.push(saveConsole);
      browserConsoleAudits.push(saveConsole());
      const pageState = await captures[index].read();
      qaTraceCaptureAudit.push(writeQaTraceArtifact({ directory: workingDirectory, label: prefix,
        state: pageState, serverLogs: server.logs, provenance: artifactProvenance }));
      fs.writeFileSync(
        path.join(workingDirectory, `${prefix}-state.json`),
        JSON.stringify(pageState, null, 2),
      );
      if (typeof client?.screenshot === "function") {
        const shot = await client.screenshot().catch(() => null);
        if (shot) {
          fs.writeFileSync(path.join(workingDirectory, `${prefix}.png`), Buffer.from(shot, "base64"));
        }
      }
    }
    // A peer may warn while another peer's large capture is being exported.
    consoleFinalizers.forEach((saveConsole, index) => {
      browserConsoleAudits[index] = saveConsole();
    });

    const retained = {
      artifactDir: ".",
      serverLogPath: "server.log",
      serverProcessPath: "server-process.json",
      staticLogPath: "static-server.json",
      clientUrlsPath: "client-urls.txt",
      serverEventsPath: "server-events.json",
      serverDamageEventsPath: "server-damage-events.json",
      serverCollisionWitnessesPath: "server-collision-witnesses.json",
      provenancePath: "provenance.json",
      provenance: artifactProvenance,
      extraLogPaths,
      extraScreenshotPaths,
      qaTraceCaptureAudit,
      browserConsoleAudits,
      urls: urls.filter(Boolean),
      retainedExternalPaths: [],
      artifactFiles: artifactFileManifest(workingDirectory),
    };
    atomicDirectory.publish();
    Object.defineProperty(retained, "absoluteArtifactDir", {
      enumerable: false,
      value: path.resolve(artifactDir),
    });
    return retained;
  });
}

function browserE2eFailureFooter(artifacts) {
  const resolveArtifact = (artifactPath) => path.resolve(
    artifacts.absoluteArtifactDir,
    artifactPath,
  );
  const urlLines = artifacts.urls.length > 0
    ? artifacts.urls.map((url) => `  ${url}`).join("\n")
    : "  <not initialized>";
  const extra = artifacts.extraLogPaths.length > 0
    ? `\nextra logs:\n${artifacts.extraLogPaths.map((logPath) => `  ${resolveArtifact(logPath)}`).join("\n")}`
    : "";
  return [
    `artifacts: ${artifacts.absoluteArtifactDir}`,
    `server log: ${resolveArtifact(artifacts.serverLogPath)}`,
    `static log: ${resolveArtifact(artifacts.staticLogPath)}`,
    `server events: ${resolveArtifact(artifacts.serverEventsPath)}`,
    `server damage events: ${resolveArtifact(artifacts.serverDamageEventsPath)}`,
    `provenance: ${resolveArtifact(artifacts.provenancePath)}`,
    `client urls: ${resolveArtifact(artifacts.clientUrlsPath)}`,
    "client url values:",
    urlLines,
    extra,
  ].filter(Boolean).join("\n");
}

export function reportBrowserE2eFailure(error, artifacts, report = console.error) {
  fs.writeFileSync(
    path.join(artifacts.absoluteArtifactDir, "failure.txt"),
    `${error.stack || error.message}\n`,
  );
  // Error.stack may already be cached. Changing error.message cannot reliably
  // publish the artifact directory, and duplicating server logs into it adds
  // no evidence: the complete log is already retained in the bundle.
  report(browserE2eFailureFooter(artifacts));
}

function assertClockTrace(label, state) {
  const metrics = state.uiMetrics || {};
  const required = [
    "clientInputSeq",
    "latestServerTick",
    "latestSnapshotId",
    "latestSnapshotServerTick",
    "latestSnapshotBaseId",
    "lastAckedSnapshotId",
    "lastAckedSnapshotServerTick",
    "remoteSnapCount",
    "lastRemoteSnapReason",
  ];
  const missing = required.filter((field) => !Number.isFinite(metrics[field]));
  if (missing.length > 0) {
    throw new Error(`${label} missing server-clock/logical-counter trace fields: ${JSON.stringify({
      missing,
      metrics: compactMetrics(metrics),
      traceTail: (state.metricsTrace || []).slice(-20),
    })}`);
  }
  if (metrics.latestSnapshotServerTick > metrics.latestServerTick) {
    throw new Error(`${label} snapshot tick advanced beyond observed server clock: ${JSON.stringify(compactMetrics(metrics))}`);
  }
  if (metrics.lastAckedSnapshotServerTick > metrics.latestSnapshotServerTick) {
    throw new Error(`${label} acked snapshot tick advanced beyond latest snapshot: ${JSON.stringify(compactMetrics(metrics))}`);
  }
}

function assertNoRemoteSnapDuring(label, samples) {
  const snapCounts = samples
    .map((entry) => entry.remoteSnapCount)
    .filter(Number.isFinite);
  if (snapCounts.length < 2) return;
  const min = Math.min(...snapCounts);
  const max = Math.max(...snapCounts);
  if (max > min) {
    throw new Error(`${label} snapped remote interpolation during active motion: ${JSON.stringify({
      before: min,
      after: max,
      samples: samples.slice(-60),
    })}`);
  }
}

// Loss recovery can legitimately make an AOI proxy absent for one or more
// applied snapshots. Its first position after that absence must use the
// explicit Initial snap path because there is no render position to
// interpolate from. Audit each continuously-visible segment independently so
// the absence does not hide a snap within a live segment, and so the required
// reinitialization is not misreported as interpolation snapback.
function assertNoRemoteSnapDuringContinuousVisibility(label, trace, watchedActor) {
  if (
    !watchedActor ||
    !Number.isInteger(watchedActor.playerId) ||
    watchedActor.playerId < 0 ||
    watchedActor.playerId > 0xffff ||
    !Number.isInteger(watchedActor.lifeSeq) ||
    watchedActor.lifeSeq <= 0
  ) {
    throw new Error(`${label} has no exact watched actor identity: ${JSON.stringify(watchedActor)}`);
  }

  let lastAppliedSnapshotId = null;
  let lastActorStreamEpoch = null;
  let lastActorStreamSerial = null;
  let lastActorStreamOverflow = null;
  let watchedPresentInAppliedSnapshot = false;
  let segmentStart = null;
  let previousPresentation = null;
  for (const sample of trace) {
    if (
      !sample ||
      typeof sample !== "object" ||
      !Number.isInteger(sample.authoritativeSnapshotId) ||
      sample.authoritativeSnapshotId <= 0 ||
      sample.authoritativeSnapshotId > 0xffff ||
      !Array.isArray(sample.authoritativeSnapshotActors) ||
      !Array.isArray(sample.authoritativeSnapshotActorTrace) ||
      sample.authoritativeSnapshotActorTrace.length === 0 ||
      !sample.authoritativeSnapshotActorStream ||
      typeof sample.authoritativeSnapshotActorStream !== "object" ||
      !Number.isInteger(sample.authoritativeSnapshotActorStream.epoch) ||
      sample.authoritativeSnapshotActorStream.epoch <= 0 ||
      sample.authoritativeSnapshotActorStream.epoch > 0xffff_ffff ||
      !Number.isInteger(sample.authoritativeSnapshotActorStream.latestSerial) ||
      sample.authoritativeSnapshotActorStream.latestSerial < 0 ||
      sample.authoritativeSnapshotActorStream.latestSerial > 0xffff_ffff ||
      !Number.isInteger(sample.authoritativeSnapshotActorStream.overflow) ||
      sample.authoritativeSnapshotActorStream.overflow < 0 ||
      sample.authoritativeSnapshotActorStream.overflow > 0xffff_ffff ||
      sample.authoritativeSnapshotActorStream.error != null ||
      !Array.isArray(sample.remoteActorPresentation)
    ) {
      throw new Error(`${label} has malformed actor-continuity evidence: ${JSON.stringify(sample)}`);
    }

    const latestMembership = sample.authoritativeSnapshotActorTrace.findLast(
      (membership) => membership?.latestSnapshotId === sample.authoritativeSnapshotId,
    );
    if (
      !latestMembership ||
      latestMembership.epoch !== sample.authoritativeSnapshotActorStream.epoch ||
      latestMembership.serial !== sample.authoritativeSnapshotActorStream.latestSerial ||
      !Array.isArray(latestMembership.actors) ||
      JSON.stringify(latestMembership.actors) !== JSON.stringify(
        sample.authoritativeSnapshotActors.map(({ latestSnapshotId: _ignored, ...actor }) => actor),
      )
    ) {
      throw new Error(`${label} latest actor trace diverges from authoritative membership: ${JSON.stringify({
        sample,
        latestMembership,
      })}`);
    }
    const membershipIds = new Set();
    for (const actor of sample.authoritativeSnapshotActors) {
      if (
        !actor ||
        typeof actor !== "object" ||
        actor.latestSnapshotId !== sample.authoritativeSnapshotId ||
        !Number.isInteger(actor.id) ||
        actor.id < 0 ||
        actor.id > 0xffff ||
        !Number.isInteger(actor.lifeSeq) ||
        actor.lifeSeq <= 0
      ) {
        throw new Error(`${label} has malformed authoritative actor evidence: ${JSON.stringify({
          sample,
          actor,
        })}`);
      }
      if (membershipIds.has(actor.id)) {
        throw new Error(`${label} has duplicate authoritative actor membership: ${JSON.stringify({
          sample,
          actor,
        })}`);
      }
      membershipIds.add(actor.id);
    }
    const presentationIds = new Set();
    for (const actor of sample.remoteActorPresentation) {
      if (
        !actor ||
        typeof actor !== "object" ||
        !Number.isInteger(actor.id) ||
        actor.id < 0 ||
        actor.id > 0xffff ||
        !Number.isInteger(actor.lifeSeq) ||
        actor.lifeSeq <= 0 ||
        !Number.isInteger(actor.snapCount) ||
        actor.snapCount < 0 ||
        !Number.isInteger(actor.initialCount) ||
        actor.initialCount < 0 ||
        !Number.isInteger(actor.lastSnapReason) ||
        actor.lastSnapReason < 0
      ) {
        throw new Error(`${label} has malformed actor presentation evidence: ${JSON.stringify({
          sample,
          actor,
        })}`);
      }
      if (presentationIds.has(actor.id)) {
        throw new Error(`${label} has duplicate actor presentation evidence: ${JSON.stringify({
          sample,
          actor,
        })}`);
      }
      presentationIds.add(actor.id);
    }

    // A browser render frame can drain several snapshot packets. Walk the
    // bounded production trace rather than sampling only the latest one, or
    // an omission and reentry in the same render frame can be hidden.
    for (const membership of sample.authoritativeSnapshotActorTrace) {
      if (
        !membership ||
        typeof membership !== "object" ||
        !Number.isInteger(membership.epoch) ||
        membership.epoch <= 0 ||
        !Number.isInteger(membership.serial) ||
        membership.serial < 0 ||
        !Number.isInteger(membership.latestSnapshotId) ||
        membership.latestSnapshotId <= 0 ||
        membership.latestSnapshotId > 0xffff ||
        !Array.isArray(membership.actors)
      ) {
        throw new Error(`${label} has malformed authoritative actor trace: ${JSON.stringify({
          sample,
          membership,
        })}`);
      }
      if (membership.epoch !== sample.authoritativeSnapshotActorStream.epoch) {
        throw new Error(`${label} mixed authoritative actor stream epochs: ${JSON.stringify({
          sample,
          membership,
        })}`);
      }
      if (lastActorStreamEpoch !== membership.epoch) {
        // `index.html` owns the epoch cursor from connection time and marks
        // any missed serial in `error`.  A scenario assertion can begin from
        // a later, already-evicted slice of that continuously observed stream;
        // its first retained serial is the evidence boundary, not a hidden
        // gap.  From this boundary onward both serial and overflow must remain
        // contiguous/monotonic below.
        lastActorStreamEpoch = membership.epoch;
        lastActorStreamSerial = null;
        lastActorStreamOverflow = sample.authoritativeSnapshotActorStream.overflow;
        lastAppliedSnapshotId = null;
        watchedPresentInAppliedSnapshot = false;
        segmentStart = null;
        previousPresentation = null;
      }
      if (sample.authoritativeSnapshotActorStream.overflow < lastActorStreamOverflow) {
        throw new Error(`${label} authoritative actor overflow counter rewound: ${JSON.stringify({
          previous: lastActorStreamOverflow,
          sample,
        })}`);
      }
      lastActorStreamOverflow = sample.authoritativeSnapshotActorStream.overflow;
      if (lastActorStreamSerial != null) {
        const serialDistance = (membership.serial - lastActorStreamSerial) >>> 0;
        if (serialDistance === 0) continue;
        if (serialDistance !== 1) {
          throw new Error(`${label} authoritative actor stream serial gap: ${JSON.stringify({
            previous: lastActorStreamSerial,
            membership,
          })}`);
        }
      }
      lastActorStreamSerial = membership.serial;
      const tracedIds = new Set();
      for (const actor of membership.actors) {
        if (
          !actor ||
          typeof actor !== "object" ||
          !Number.isInteger(actor.id) ||
          actor.id < 0 ||
          actor.id > 0xffff ||
          !Number.isInteger(actor.lifeSeq) ||
          actor.lifeSeq <= 0 ||
          tracedIds.has(actor.id)
        ) {
          throw new Error(`${label} has malformed authoritative actor trace membership: ${JSON.stringify({
            sample,
            membership,
            actor,
          })}`);
        }
        tracedIds.add(actor.id);
      }
      const forwardDistance = lastAppliedSnapshotId == null
        ? null
        : snapshotIdForwardDistance(membership.latestSnapshotId, lastAppliedSnapshotId);
      const strictlyForward = lastAppliedSnapshotId == null || (
        forwardDistance != null &&
        forwardDistance > 0 &&
        forwardDistance <= 0x7fff
      );
      if (!strictlyForward) continue;
      lastAppliedSnapshotId = membership.latestSnapshotId;
      watchedPresentInAppliedSnapshot = membership.actors.some(
        (actor) =>
          actor.id === watchedActor.playerId && actor.lifeSeq === watchedActor.lifeSeq,
      );
      if (!watchedPresentInAppliedSnapshot) {
        segmentStart = null;
        previousPresentation = null;
      }
    }

    // Only a newly applied raw wire-membership list can prove absence. Cloak,
    // xradar, attachment/render filtering, aggregate visibility, proxy TTL,
    // and repeated/backward snapshot samples cannot sever continuity.
    if (!watchedPresentInAppliedSnapshot) {
      continue;
    }
    const matchingPresentation = sample.remoteActorPresentation.filter(
      (actor) =>
        actor.id === watchedActor.playerId && actor.lifeSeq === watchedActor.lifeSeq,
    );
    if (matchingPresentation.length !== 1) {
      throw new Error(`${label} is missing exact watched-actor presentation evidence: ${JSON.stringify({
        watchedActor,
        sample,
      })}`);
    }
    const presentation = matchingPresentation[0];
    if (segmentStart == null) {
      segmentStart = sample;
      previousPresentation = presentation;
      continue;
    }
    if (
      presentation.snapCount !== previousPresentation.snapCount ||
      presentation.initialCount !== previousPresentation.initialCount
    ) {
      throw new Error(`${label} snapped remote interpolation during continuous active motion: ${JSON.stringify({
        watchedActor,
        before: previousPresentation,
        after: presentation,
        segmentStart,
        current: sample,
      })}`);
    }
    previousPresentation = presentation;
  }
}

function compactMovementTrace(trace) {
  return {
    before: compactMetrics(trace.before),
    after: compactMetrics(trace.after),
    moved: trace.moved,
  };
}

function orientationDelta(a, b) {
  if (!Number.isFinite(a) || !Number.isFinite(b)) return 0;
  return Math.abs(((b - a + 0.5) % 1 + 1) % 1 - 0.5);
}

function signedOrientationDelta(a, b) {
  if (!Number.isFinite(a) || !Number.isFinite(b)) return 0;
  return ((b - a + 0.5) % 1 + 1) % 1 - 0.5;
}

function metricTraceWindow(state, traceStart = 0) {
  const trace = state.metricsTrace || [];
  if (traceStart && typeof traceStart === "object") {
    if (Number.isFinite(traceStart.at)) {
      const byTime = trace.filter((entry) =>
        Number.isFinite(entry.at) && entry.at >= traceStart.at - 80
      );
      if (byTime.length > 0) return byTime;
    }
    if (Number.isInteger(traceStart.index)) {
      return trace.slice(traceStart.index);
    }
  }
  return trace.slice(traceStart);
}

function assertRemoteMotionSmooth(label, state, traceStart = 0) {
  const samples = metricTraceWindow(state, traceStart)
    .filter((entry) =>
      Number.isFinite(entry.remoteDisplayX) &&
      Number.isFinite(entry.remoteDisplayY) &&
      (entry.remoteVisiblePlayers ?? 0) >= 1
    );
  if (samples.length < 12) {
    throw new Error(`${label} did not capture enough remote render samples: ${JSON.stringify({
      samples: samples.length,
      uiMetrics: state.uiMetrics,
      traceTail: (state.metricsTrace || []).slice(-20),
    })}`);
  }

  let maxDisplayDelta = 0;
  const displayStepFailures = [];
  let maxDisplayJerk = 0;
  let maxPositionDelta = 0;
  let maxShortGapPositionDelta = 0;
  let maxPositionSpeed = 0;
  let maxOrientationDelta = 0;
  let minExtrapolatedFrames = null;
  let maxExtrapolatedFrames = null;
  let worstDisplay = null;
  let worstJerk = null;
  let previousDisplayStep = null;
  for (let i = 1; i < samples.length; i += 1) {
    const prev = samples[i - 1];
    const cur = samples[i];
    const displayStep = {
      x: cur.remoteDisplayX - prev.remoteDisplayX,
      y: cur.remoteDisplayY - prev.remoteDisplayY,
    };
    const displayDelta = Math.hypot(displayStep.x, displayStep.y);
    const cadenceSample = (entry) => ({
      at: entry.at, monotonicMs: entry.monotonicMs,
      displayX: entry.remoteDisplayX, displayY: entry.remoteDisplayY,
      velocityX: entry.remotePlayers?.[0]?.velocityX,
      velocityY: entry.remotePlayers?.[0]?.velocityY,
    });
    const stepEvidence = remoteDisplayStepEvidence(cadenceSample(prev), cadenceSample(cur));
    if (!stepEvidence.ok) displayStepFailures.push({ index: i, ...stepEvidence });
    if (previousDisplayStep != null) {
      const displayJerk = Math.hypot(
        displayStep.x - previousDisplayStep.x,
        displayStep.y - previousDisplayStep.y,
      );
      if (displayJerk > maxDisplayJerk) {
        maxDisplayJerk = displayJerk;
        worstJerk = { prev, cur, displayStep, previousDisplayStep, displayJerk };
      }
    }
    previousDisplayStep = displayStep;
    const positionDelta =
      Number.isFinite(cur.remotePositionX) && Number.isFinite(prev.remotePositionX)
        ? Math.hypot(cur.remotePositionX - prev.remotePositionX, cur.remotePositionY - prev.remotePositionY)
        : 0;
    const prevPositionTick = prev.remotePlayers?.[0]?.lastServerPositionTick;
    const curPositionTick = cur.remotePlayers?.[0]?.lastServerPositionTick;
    if (Number.isFinite(prevPositionTick) && Number.isFinite(curPositionTick)) {
      const positionTickDelta = (curPositionTick - prevPositionTick + 0x1_0000) % 0x1_0000;
      if (positionTickDelta > 0 && positionTickDelta < 0x8000) {
        maxPositionSpeed = Math.max(
          maxPositionSpeed,
          positionDelta / (positionTickDelta / 100),
        );
        if (positionTickDelta <= 3) {
          maxShortGapPositionDelta = Math.max(maxShortGapPositionDelta, positionDelta);
        }
      }
    }
    const orientDelta = orientationDelta(prev.remoteDisplayOrientation, cur.remoteDisplayOrientation);
    if (displayDelta > maxDisplayDelta) {
      maxDisplayDelta = displayDelta;
      worstDisplay = { prev, cur, displayDelta, positionDelta, orientDelta };
    }
    maxPositionDelta = Math.max(maxPositionDelta, positionDelta);
    maxOrientationDelta = Math.max(maxOrientationDelta, orientDelta);
    if (Number.isFinite(cur.remoteExtrapolatedFrames)) {
      minExtrapolatedFrames =
        minExtrapolatedFrames == null ? cur.remoteExtrapolatedFrames : Math.min(minExtrapolatedFrames, cur.remoteExtrapolatedFrames);
      maxExtrapolatedFrames =
        maxExtrapolatedFrames == null ? cur.remoteExtrapolatedFrames : Math.max(maxExtrapolatedFrames, cur.remoteExtrapolatedFrames);
    }
  }
  assertNoRemoteSnapDuring(label, samples);

  if (
    minExtrapolatedFrames != null &&
    maxExtrapolatedFrames != null &&
    maxExtrapolatedFrames > minExtrapolatedFrames
  ) {
    throw new Error(`${label} used remote extrapolation during active motion: ${JSON.stringify({
      before: minExtrapolatedFrames,
      after: maxExtrapolatedFrames,
      uiMetrics: state.uiMetrics,
      samples: samples.slice(-40),
    })}`);
  }

  // Keep the existing ordinary-frame, jerk and authority limits. Larger
  // displacement must be explained by measured render time and velocity,
  // within quantization error; long render stalls are failures in their own
  // right, not an allowance for arbitrarily large displacement.
  if (
    displayStepFailures.length > 0 ||
    maxDisplayJerk > 6 ||
    maxShortGapPositionDelta > 12 ||
    maxPositionSpeed > 700 ||
    maxOrientationDelta > 0.05
  ) {
    throw new Error(`${label} had a visible remote render jump: ${JSON.stringify({
      maxDisplayDelta,
      displayStepFailures: displayStepFailures.slice(0, 16),
      maxDisplayJerk,
      maxPositionDelta,
      maxShortGapPositionDelta,
      maxPositionSpeed,
      maxOrientationDelta,
      worstDisplay,
      worstJerk,
      uiMetrics: state.uiMetrics,
      samples: samples.slice(-40),
    })}`);
  }
  return {
    samples: samples.length,
    maxDisplayDelta,
    maxDisplayJerk,
    maxPositionDelta,
    maxShortGapPositionDelta,
    maxPositionSpeed,
    maxOrientationDelta,
    remoteExtrapolatedFrames: maxExtrapolatedFrames ?? null,
  };
}

function assertRemoteMotionRecoversUnderLoss(label, state, traceStart = 0, watchedActor) {
  const trace = metricTraceWindow(state, traceStart);
  assertNoRemoteSnapDuringContinuousVisibility(label, trace, watchedActor);
  const samples = trace.flatMap((entry) => {
    const actor = entry.remotePlayers.find((candidate) =>
      candidate.id === watchedActor.playerId && candidate.lifeSeq === watchedActor.lifeSeq
    );
    if (
      !actor ||
      !Number.isFinite(actor.displayX) ||
      !Number.isFinite(actor.displayY) ||
      !Number.isFinite(actor.positionX) ||
      !Number.isFinite(actor.positionY)
    ) return [];
    return [{
      ...entry,
      remoteDisplayX: actor.displayX,
      remoteDisplayY: actor.displayY,
      remotePositionX: actor.positionX,
      remotePositionY: actor.positionY,
      watchedActor: actor,
    }];
  });
  if (samples.length < 12) {
    throw new Error(`${label} did not capture enough lossy remote render samples: ${JSON.stringify({
      samples: samples.length,
      uiMetrics: state.uiMetrics,
      traceTail: (state.metricsTrace || []).slice(-20),
    })}`);
  }

  let displayPath = 0;
  let authoritativePath = 0;
  let maxDisplayDelta = 0;
  let minExtrapolatedFrames = null;
  let maxExtrapolatedFrames = null;
  for (let index = 1; index < samples.length; index += 1) {
    const previous = samples[index - 1];
    const current = samples[index];
    const displayDelta = Math.hypot(
      current.remoteDisplayX - previous.remoteDisplayX,
      current.remoteDisplayY - previous.remoteDisplayY,
    );
    displayPath += displayDelta;
    maxDisplayDelta = Math.max(maxDisplayDelta, displayDelta);
    authoritativePath += Math.hypot(
      current.remotePositionX - previous.remotePositionX,
      current.remotePositionY - previous.remotePositionY,
    );
    if (Number.isFinite(current.remoteExtrapolatedFrames)) {
      minExtrapolatedFrames = minExtrapolatedFrames == null
        ? current.remoteExtrapolatedFrames
        : Math.min(minExtrapolatedFrames, current.remoteExtrapolatedFrames);
      maxExtrapolatedFrames = maxExtrapolatedFrames == null
        ? current.remoteExtrapolatedFrames
        : Math.max(maxExtrapolatedFrames, current.remoteExtrapolatedFrames);
    }
  }
  const first = samples[0];
  const last = samples.at(-1);
  const snapshotAdvance = snapshotIdForwardDistance(
    last.latestSnapshotId,
    first.latestSnapshotId,
  );
  const finalTargetError = Math.hypot(
    last.remoteDisplayX - last.remotePositionX,
    last.remoteDisplayY - last.remotePositionY,
  );
  if (
    displayPath < 3 ||
    authoritativePath < 3 ||
    snapshotAdvance == null ||
    snapshotAdvance < 1 ||
    snapshotAdvance > 0x7fff ||
    finalTargetError > 32 ||
    (
      minExtrapolatedFrames != null &&
      maxExtrapolatedFrames != null &&
      maxExtrapolatedFrames > minExtrapolatedFrames
    )
  ) {
    throw new Error(`${label} did not make bounded progress and recover under loss: ${JSON.stringify({
      samples: samples.length,
      displayPath,
      authoritativePath,
      maxDisplayDelta,
      snapshotAdvance,
      finalTargetError,
      minExtrapolatedFrames,
      maxExtrapolatedFrames,
      uiMetrics: state.uiMetrics,
      traceTail: samples.slice(-40),
    })}`);
  }

  return {
    mode: "loss-recovery",
    samples: samples.length,
    displayPath,
    authoritativePath,
    maxDisplayDelta,
    snapshotAdvance,
    finalTargetError,
    remoteExtrapolatedFrames: maxExtrapolatedFrames ?? null,
  };
}

function assertRemoteRotationSmooth(label, state, traceStart = 0) {
  const samples = metricTraceWindow(state, traceStart)
    .filter((entry) =>
      Number.isFinite(entry.remoteDisplayOrientation) &&
      (entry.remoteVisiblePlayers ?? 0) >= 1
    );
  if (samples.length < 24) {
    throw new Error(`${label} did not capture enough remote rotation samples: ${JSON.stringify({
      samples: samples.length,
      uiMetrics: state.uiMetrics,
      traceTail: (state.metricsTrace || []).slice(-20),
    })}`);
  }

  let totalAbsDelta = 0;
  let maxOrientationDelta = 0;
  let previousMovementSign = 0;
  let signFlips = 0;
  let worst = null;
  for (let i = 1; i < samples.length; i += 1) {
    const prev = samples[i - 1];
    const cur = samples[i];
    const signed = signedOrientationDelta(prev.remoteDisplayOrientation, cur.remoteDisplayOrientation);
    const delta = Math.abs(signed);
    totalAbsDelta += delta;
    if (delta > maxOrientationDelta) {
      maxOrientationDelta = delta;
      worst = { prev, cur, signedDelta: signed, delta };
    }
    if (delta > 0.002) {
      const sign = Math.sign(signed);
      if (previousMovementSign !== 0 && sign !== previousMovementSign) {
        signFlips += 1;
      }
      previousMovementSign = sign;
    }
  }
  assertNoRemoteSnapDuring(label, samples);

  // Three protocol heading bins are enough to prove a visible ordered turn;
  // the server may coalesce later same-direction records while the exact
  // authority replay catches up. Keep the jump and reversal bounds strict.
  // Exact decimal protocol/display bins are not exactly representable in
  // binary64 (the retained 0.04 step becomes 0.040000000000000036). Keep the
  // authored jump bound unchanged while excluding representation noise.
  const orientationJumpLimit = 0.04;
  const orientationComparisonEpsilon = 1e-12;
  if (
    totalAbsDelta < 0.06 ||
    maxOrientationDelta > orientationJumpLimit + orientationComparisonEpsilon ||
    signFlips > 2
  ) {
    throw new Error(`${label} had visible remote rotation jitter: ${JSON.stringify({
      totalAbsDelta,
      maxOrientationDelta,
      signFlips,
      worst,
      uiMetrics: state.uiMetrics,
      samples: samples.slice(-60),
    })}`);
  }
  return {
    samples: samples.length,
    totalAbsDelta,
    maxOrientationDelta,
    signFlips,
  };
}

function assertRemoteRotationRecoversUnderLoss(label, state, traceStart = 0, watchedActor) {
  const trace = metricTraceWindow(state, traceStart);
  assertNoRemoteSnapDuringContinuousVisibility(label, trace, watchedActor);
  const samples = trace.flatMap((entry) => {
    const actor = entry.remotePlayers.find((candidate) =>
      candidate.id === watchedActor.playerId && candidate.lifeSeq === watchedActor.lifeSeq
    );
    if (!actor || !Number.isFinite(actor.displayOrientation)) return [];
    return [{
      ...entry,
      remoteDisplayOrientation: actor.displayOrientation,
      watchedActor: actor,
    }];
  });
  if (samples.length < 24) {
    throw new Error(`${label} did not capture enough lossy rotation samples: ${JSON.stringify({
      samples: samples.length,
      uiMetrics: state.uiMetrics,
      traceTail: (state.metricsTrace || []).slice(-20),
    })}`);
  }

  let totalAbsDelta = 0;
  let maxOrientationDelta = 0;
  for (let index = 1; index < samples.length; index += 1) {
    const delta = orientationDelta(
      samples[index - 1].remoteDisplayOrientation,
      samples[index].remoteDisplayOrientation,
    );
    totalAbsDelta += delta;
    maxOrientationDelta = Math.max(maxOrientationDelta, delta);
  }
  const snapshotAdvance = snapshotIdForwardDistance(
    samples.at(-1).latestSnapshotId,
    samples[0].latestSnapshotId,
  );
  if (
    totalAbsDelta < 0.08 ||
    snapshotAdvance == null ||
    snapshotAdvance < 1 ||
    snapshotAdvance > 0x7fff
  ) {
    throw new Error(`${label} did not rotate while snapshots recovered under loss: ${JSON.stringify({
      samples: samples.length,
      totalAbsDelta,
      maxOrientationDelta,
      snapshotAdvance,
      uiMetrics: state.uiMetrics,
      traceTail: samples.slice(-60),
    })}`);
  }
  return {
    mode: "loss-recovery",
    samples: samples.length,
    totalAbsDelta,
    maxOrientationDelta,
    snapshotAdvance,
  };
}

function matchingDamageEvents(server, sinceCount, shooterId, victimId) {
  return parseServerDamageEvents(server.logs)
    .slice(sinceCount)
    .filter((event) => event.shooterId === shooterId && event.victimId === victimId);
}

function bulletSelfDamageEvents(server, sinceCount) {
  return parseServerDamageEvents(server.logs)
    .slice(sinceCount)
    .filter((event) =>
      event.shooterId === event.victimId &&
      [1, 2, 7, 15].includes(event.weapon & 0x001f)
    );
}

function assertNoBulletSelfDamage(server, sinceCount, label) {
  const events = bulletSelfDamageEvents(server, sinceCount);
  if (events.length > 0) {
    throw new Error(`${label} produced bullet self-damage: ${JSON.stringify({
      events,
      serverDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
    })}`);
  }
}

function assertServerDamageAccounting(event, label) {
  const expectedEnergyAfter = Math.max(0, event.energyBefore - event.damage);
  if (event.energyAfter !== expectedEnergyAfter) {
    throw new Error(`${label} server damage accounting mismatch: ${JSON.stringify({
      event,
      expectedEnergyAfter,
    })}`);
  }
}

function deathMatches(death, killerId, killedId) {
  return death.killerId === killerId && death.killedId === killedId;
}

function matchingReliableDeathEvents(
  deaths,
  { reliableId, killerId, killedId, observedAfter = 0 },
) {
  return deaths.filter((event) =>
    Number.isFinite(event.at) &&
    event.at >= observedAfter &&
    event.reliableId === reliableId &&
    deathMatches(event, killerId, killedId)
  );
}

function selfDeathEvents(state, playerId) {
  const summary = summarizeClient(state);
  return [
    ...summary.receivedDeaths,
    ...(state.deathTrace || []),
  ].filter((death) => deathMatches(death, playerId, playerId));
}

function damageTraceMatches(entry, event) {
  return (
    entry &&
    entry.victimId === event.victimId &&
    entry.attackerId === event.shooterId &&
    (event.damageSeq == null || entry.damageSeq === event.damageSeq) &&
    (event.projectileId == null || entry.projectileId === event.projectileId) &&
    (event.serverTick == null || entry.serverTick === event.serverTick) &&
    (event.victimLifeSeq == null || entry.victimLifeSeq === event.victimLifeSeq) &&
    (event.shooterLifeSeq == null || entry.shooterLifeSeq === event.shooterLifeSeq) &&
    (entry.weapon & 0x001f) === (event.weapon & 0x001f) &&
    entry.damage === event.damage &&
    (entry.energyBefore == null || Math.abs(entry.energyBefore - event.energyBefore) <= 1) &&
    Math.abs(entry.energyAfter - event.energyAfter) <= 1
  );
}

function damageTraceMatchCount(state, event, sinceIndex = 0) {
  return (state.damageTrace || [])
    .slice(sinceIndex)
    .filter((entry) => damageTraceMatches(entry, event)).length;
}

async function waitForLivePlayerScopes(alpha, beta) {
  await waitUntil("both players have distinct exact live scopes before encounter setup", async () => {
    const states = await Promise.all([alpha.metricsState(), beta.metricsState()]);
    const metrics = states.map(state => state.uiMetrics);
    return metrics.every(exactLiveGameplayScope) &&
      metrics[0].selfPlayerId !== metrics[1].selfPlayerId;
  }, snapshotLossGate ? 120_000 : 30_000);
}

let rendezvousTail = Promise.resolve();

// Explicit spatial setup only. Never call this implicitly in a visibility
// assertion: some scenarios deliberately park a third observer off the lane.
async function rendezvousPlayers(alpha, beta, server = null, {
  setupBudgetMs = 180_000, stageRadiusPx = 48, meetingPoint = null, avoidTiles = [], recoverOrigins = false,
} = {}) {
  if (!Number.isFinite(setupBudgetMs) || setupBudgetMs <= 0 || setupBudgetMs > 180_000) {
    throw new Error("invalid bounded rendezvous setup budget");
  }
  if (!Number.isFinite(stageRadiusPx) || stageRadiusPx < 20 || stageRadiusPx > 768) {
    throw new Error("invalid rendezvous stage radius");
  }
  if (typeof recoverOrigins !== "boolean") throw new Error("invalid rendezvous origin-recovery policy");
  const previous = rendezvousTail;
  let unlock;
  rendezvousTail = new Promise(resolve => { unlock = resolve; });
  await previous;
  if (!Array.isArray(avoidTiles) || !avoidTiles.every(tile => Number.isInteger(tile?.x) &&
      Number.isInteger(tile?.y) && tile.x >= 0 && tile.x < 1024 && tile.y >= 0 && tile.y < 1024))
    throw new Error("invalid rendezvous avoidance tiles");
  const evidence = { startedAt: Date.now(), status: "pending", keysOnly: true, setupBudgetMs, stageRadiusPx,
    meetingPoint, avoidTiles };
  const focusReleases = [];
  for (const client of [alpha, beta]) (client.rendezvousTrace ||= []).push(evidence);
  const scope = metrics => {
    const local = metrics.localGameplayState || {};
    return JSON.stringify([local.authorityScopeServerSessionId, local.authorityScopeArenaStreamId,
      metrics.selfPlayerId, metrics.playerLifeSeq]);
  };
  const read = async () => (await Promise.all([alpha.metricsState(), beta.metricsState()])).map(state => state.uiMetrics);
  try {
    await waitForLivePlayerScopes(alpha, beta);
    const deadline = Date.now() + setupBudgetMs;
    const initial = await read();
    evidence.scopes = initial.map(scope);
    const leasedActors = [];
    for (const [index, client] of [alpha, beta].entries()) {
      if (typeof client.acquireInputFocusLease === "function") {
        focusReleases.push(client.acquireInputFocusLease());
        leasedActors.push(index);
      }
    }
    evidence.inputFocusIsolation = { leasedActors,
      policy: "suppress automation window activation during joint real-key navigation; canvas and game blur semantics unchanged" };
    const releases = await Promise.allSettled([alpha, beta].map(client =>
      typeof client.releaseKeys === "function" ? client.releaseKeys() : undefined));
    const releaseErrors = releases.filter(result => result.status === "rejected").map(result => result.reason);
    if (releaseErrors.length) throw new AggregateError(releaseErrors, "rendezvous inherited-key release failed");
    // Both pilots are participating in setup. Give each a genuine facing
    // action before one waits for the other's long route; an untouched fresh
    // spawn correctly becomes a spectator after two minutes. Fresh neutral
    // spawns face north, so east proves real controls without spending energy.
    const preflight = await Promise.allSettled([alpha, beta].map(client =>
      orientClientForProtocolDirection(client, 1, 0, 4_000)));
    evidence.preflightFailures = preflight.flatMap((result, actorIndex) => result.status === "rejected"
      ? [{ actorIndex, name: result.reason?.name, message: result.reason?.message,
        stack: result.reason?.stack }] : []);
    const preflightErrors = preflight.filter(result => result.status === "rejected").map(result => result.reason);
    if (preflightErrors.length) throw new AggregateError(preflightErrors, "rendezvous preflight failed");
    evidence.preflight = preflight.map(result => ({ oriented: result.value.oriented,
      direction: result.value.direction, desiredDirection: result.value.desiredDirection }));
    if (!evidence.preflight.every(aim => aim.oriented)) throw new Error("rendezvous preflight did not turn both pilots");
    // A preceding gameplay check may leave a pilot coasting beside a wall.
    // Certify a stopped origin BEFORE planning with the wider route margin;
    // steering from a moving origin can otherwise invalidate that certificate.
    // Fresh stationary spawns take the existing brake helper's no-input path.
    const stops = await Promise.allSettled([alpha, beta].map(client => {
      const remaining = deadline - Date.now();
      if (remaining <= 0) throw new Error("rendezvous exceeded its setup budget before initial braking");
      return brakeClientVelocity(client, Math.min(8_000, remaining));
    }));
    evidence.initialStops = stops.map(result => result.status === "fulfilled"
      ? { braked: result.value.braked, speed: result.value.speed, reason: result.value.reason ?? null }
      : { braked: false, error: String(result.reason) });
    const stopErrors = stops.filter(result => result.status === "rejected").map(result => result.reason);
    if (stopErrors.length) throw new AggregateError(stopErrors, "rendezvous initial braking failed");
    if (!evidence.initialStops.every(stop => stop.braked)) throw new Error("rendezvous initial stop failed");
    if (Date.now() >= deadline) throw new Error("rendezvous exceeded its setup budget during initial braking");
    let before = await read();
    if (!before.every(metrics => brakeVelocityDisposition(metrics).braked)) {
      throw new Error("rendezvous initial stop did not retain rest before route certification");
    }
    evidence.origins = before.map(metrics => ({ x: metrics.playerX, y: metrics.playerY,
      velocityX: metrics.playerVelocityX, velocityY: metrics.playerVelocityY }));
    if (!before.every((metrics, index) => exactLiveGameplayScope(metrics) && scope(metrics) === evidence.scopes[index])) {
      throw new Error("rendezvous preflight crossed a lifecycle boundary");
    }
    const arenas = await Promise.all([alpha, beta].map(client => client.evaluate("window.__zubspaceArena")));
    if (arenas[0] !== arenas[1]) throw new Error("rendezvous players are in different arenas");
    const loaded = loadRendezvousMap(arenas[0], [
      ...((server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT) ?
        [server?.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT] : []),
      path.join(repoRoot, "assets/zones"),
    ]);
    evidence.map = loaded.provenance;
    if (recoverOrigins) {
      // Only explicit setup callers may request ordinary-key origin recovery.
      // Preserve their exact lives/scopes and the original total deadline;
      // observation-only callers and deliberately parked peers never Warp.
      evidence.originsBeforeRecovery = evidence.origins;
      evidence.originRecoveries = [];
      for (const [index, client] of [alpha, beta].entries()) {
        evidence.originRecoveries.push(await prepareKeyboardLifecycleOrigin({
          actor: client, server, playerId: before[index].selfPlayerId,
          initialLifeSeq: before[index].playerLifeSeq, geometry: loaded.geometry, deadline,
        }));
      }
      before = await read();
      if (Date.now() >= deadline || !before.every((metrics, index) =>
        exactLiveGameplayScope(metrics) && scope(metrics) === evidence.scopes[index]))
        throw new Error("rendezvous origin preparation exceeded its deadline or exact scope");
      evidence.origins = before.map(metrics => ({ x: metrics.playerX, y: metrics.playerY,
        velocityX: metrics.playerVelocityX, velocityY: metrics.playerVelocityY }));
    }
    const origins = before.map(metrics => ({ x: metrics.playerX, y: metrics.playerY }));
    const blockedTiles = avoidTiles.concat(before.flatMap(metrics => {
      if (metrics.localGameplayState?.brickTilesOverflow) throw new Error("incomplete brick geometry for rendezvous");
      return metrics.localGameplayState?.brickTiles || [];
    }));
    // A server-chosen tile has one complete neighbouring tile of clearance.
    // A 20px square retains headroom beyond the ordinary 14px hull without
    // wrongly demanding a fourth tile at the 24px grid boundary.
    evidence.plan = planPlayerRendezvous(loaded.geometry, origins, { blockedTiles, hullRadiusPx: 20, stageRadiusPx, meetingPoint });
    evidence.legs = [[], []];
    evidence.stationKeeping = [[], []];
    let jointlyArrived = false;
    let routeFailure = null;
    const observeJointArrival = async () => {
      if (routeFailure) throw routeFailure;
      if (jointlyArrived) return true;
      const states = await read();
      if (!states.every((metrics, index) => exactLiveGameplayScope(metrics) && scope(metrics) === evidence.scopes[index])) {
        throw new Error("rendezvous station keeping crossed a lifecycle boundary");
      }
      jointlyArrived = states.every(metrics => brakeVelocityDisposition(metrics).braked && !metrics.inSafeZone &&
        Math.hypot(metrics.playerX - evidence.plan.target.x, metrics.playerY - evidence.plan.target.y) <= 4);
      if (jointlyArrived) {
        evidence.jointArrivalAt = Date.now();
        // Retain the actual joint arrival before subsequent genuine aim turns
        // allow the admitted residual velocity to coast away from the target.
        evidence.jointArrival = states.map(metrics => ({ scope: scope(metrics),
          x: metrics.playerX, y: metrics.playerY,
          velocityX: metrics.playerVelocityX, velocityY: metrics.playerVelocityY }));
      }
      return jointlyArrived;
    };
    const routes = await Promise.allSettled([alpha, beta].map(async (client, index) => {
      try {
        const legs = evidence.legs[index];
        for (const target of evidence.plan.routes[index].drivePoints) {
          // Each bend is a real stopped pose. Long straight legs may cruise;
          // the full hull margin, not a centre-line test, certifies the route.
          const remaining = deadline - Date.now();
          if (remaining <= 0) throw new Error("rendezvous exceeded its setup budget");
          const current = (await client.metricsState()).uiMetrics;
          if (!exactLiveGameplayScope(current) || scope(current) !== evidence.scopes[index]) {
            throw new Error("rendezvous cannot continue across a changed lifecycle");
          }
          const result = await driveClientToWorldTarget(client, target,
            Math.min(30_000, remaining), true, 4, null, 100, 0.5, 8_000);
          legs.push(result);
          if (!result.reached) throw new Error(`rendezvous leg did not settle: ${JSON.stringify(result)}`);
        }
        // Rest admits a small physical input quantum of residual velocity; it
        // does not mean anchored. The early c70e33b arrival coasted ~44px while
        // its peer finished. Keep it near the shared stage with the same real
        // keys until BOTH poses are ready, within the original overall budget.
        while (!await observeJointArrival()) {
          const remaining = deadline - Date.now();
          if (remaining <= 0) throw new Error("rendezvous exceeded its setup budget while holding station");
          const current = (await client.metricsState()).uiMetrics;
          if (Math.hypot(current.playerX - evidence.plan.target.x, current.playerY - evidence.plan.target.y) > 4 ||
              !brakeVelocityDisposition(current).braked) {
            const result = await driveClientToWorldTarget(client, evidence.plan.target,
              Math.min(30_000, remaining), true, 4, () => jointlyArrived, 100, 0.5, 8_000);
            evidence.stationKeeping[index].push({ at: Date.now(), ...result });
            if (!result.reached) throw new Error(`rendezvous station keeping did not settle: ${JSON.stringify(result)}`);
          } else {
            await sleep(35);
          }
        }
        return legs;
      } catch (error) {
        routeFailure ||= error;
        throw error;
      }
    }));
    const routeErrors = routes.filter(result => result.status === "rejected").map(result => result.reason);
    if (routeErrors.length) throw new AggregateError(routeErrors,
      `rendezvous failed: ${routeErrors.map(error => error.message).join("; ")}`);
    const positioned = await read();
    const aims = await Promise.allSettled([alpha, beta].map((client, index) => {
      const self = positioned[index], peer = positioned[1 - index];
      const dx = peer.playerX - self.playerX, dy = peer.playerY - self.playerY;
      const remaining = deadline - Date.now();
      if (remaining <= 0) throw new Error("rendezvous exceeded its setup budget before aiming");
      return orientClientForProtocolDirection(client, dx || dy ? dx : 0, dx || dy ? dy : -1, Math.min(4_000, remaining));
    }));
    const aimErrors = aims.filter(result => result.status === "rejected").map(result => result.reason);
    if (aimErrors.length) throw new AggregateError(aimErrors,
      `rendezvous aiming failed: ${aimErrors.map(error => error.message).join("; ")}`);
    evidence.aim = aims.map(result => result.value);
    if (!evidence.aim.every(aim => aim.oriented)) throw new Error("rendezvous did not establish genuine firing headings");
    const after = await read();
    evidence.final = after.map(metrics => ({ scope: scope(metrics), x: metrics.playerX, y: metrics.playerY,
      velocityX: metrics.playerVelocityX, velocityY: metrics.playerVelocityY,
      orientation: metrics.playerOrientation, inputSequence: metrics.clientInputSeq }));
    evidence.separationPx = Math.hypot(after[0].playerX - after[1].playerX, after[0].playerY - after[1].playerY);
    if (!after.every((metrics, index) => exactLiveGameplayScope(metrics) && scope(metrics) === evidence.scopes[index] &&
        brakeVelocityDisposition(metrics).braked && !metrics.inSafeZone) || evidence.separationPx > 24 || Date.now() > deadline) {
      throw new Error(`rendezvous did not preserve live stopped overlapping hulls: ${JSON.stringify(evidence.final)}`);
    }
    evidence.status = "complete";
    return evidence;
  } catch (error) {
    evidence.status = "failed";
    evidence.error = String(error.message);
    throw error;
  } finally {
    evidence.finishedAt = Date.now();
    for (const release of focusReleases.reverse()) release();
    unlock();
  }
}

async function waitForPlayersReady(alpha, beta, server = null) {
  try {
    await waitUntil("both players assigned ids, exact live scopes, and visible", async () => {
      // Read only the current HUD metrics while the initial reliable map/LVZ
      // transfer is still draining. Polling the multi-megabyte packet and
      // history traces here can monopolize Chrome's main thread long enough
      // to fill the application-reliable reorder queue, thereby manufacturing
      // the startup failure this readiness check is meant to observe.
      const [alphaState, betaState] = await Promise.all([
        alpha.metricsState(),
        beta.metricsState(),
      ]);
      const alphaId = alphaState.uiMetrics?.selfPlayerId;
      const betaId = betaState.uiMetrics?.selfPlayerId;
      return (
        alphaId != null &&
        betaId != null &&
        alphaId !== betaId &&
        exactLiveGameplayScope(alphaState.uiMetrics) &&
        exactLiveGameplayScope(betaState.uiMetrics) &&
        (alphaState.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1 &&
        (betaState.uiMetrics?.remoteVisiblePlayers ?? 0) >= 1
      );
    // Sustained seeded loss is active during the reliable map/LVZ bootstrap.
    // Its bounded ordered queue deliberately recovers gaps in three-second
    // resend waves, so the readiness deadline must cover that real recovery
    // instead of assuming a lossless bootstrap.
    }, snapshotLossGate ? 120_000 : 30_000);
  } catch (error) {
    const [alphaState, betaState] = await Promise.all([alpha.pageState(), beta.pageState()]);
    throw new Error(`${error.message}: ${JSON.stringify({
      alpha: {
        metrics: alphaState.uiMetrics,
        summary: compactSummary(summarizeClient(alphaState)),
      },
      beta: {
        metrics: betaState.uiMetrics,
        summary: compactSummary(summarizeClient(betaState)),
      },
      serverLifecycleDiagnostics: server?.logs?.join("").split("\n")
        .filter((line) => /ship|life|position|security_reject|rejecting/i.test(line))
        .slice(0, 240),
    })}`);
  }
}

function semanticCounterDelta(before, after, key) {
  return (after?.[key] ?? 0) - (before?.[key] ?? 0);
}

async function runFrequencyCutoverGate({
  actor,
  server,
  requireActiveRocket = false,
  delayApplicationReceipt = false,
  targetFrequencyOverride = null,
}) {
  await waitUntil("frequency cutover starts on a fresh authoritative clock", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return (
      metrics.playerAlive === true &&
      metrics.chatActive === false &&
      metrics.localGameplayState?.doorClockFresh === true &&
      metrics.localGameplayState?.pendingShipResetDeliveryId == null &&
      metrics.localGameplayState?.pendingFrequencyBaselineDeliveryId == null
    );
  }, 15_000);
  const startedAt = Date.now();
  const startedLogOffset = server.logs.join("").length;
  if (delayApplicationReceipt) {
    const armed = await actor.evaluate(
      "window.__zubspaceArmLifecycleReceiptImpairmentForTest?.() === true",
    );
    if (!armed) throw new Error("lifecycle baseline application-receipt seam could not arm");
  }
  const before = await actor.pageState();
  const beforeMetrics = before.uiMetrics || {};
  if (requireActiveRocket && !(beforeMetrics.playerRocketTimer > 0)) {
    throw new Error(`frequency cutover must begin during active RocketTime: ${JSON.stringify(beforeMetrics)}`);
  }
  const rocketCountBefore = beforeMetrics.playerRockets;
  const rocketSemanticCountBefore = beforeMetrics.semanticInputCounts?.rocket;
  const snapshotBefore = beforeMetrics.latestSnapshotId;
  const targetFrequency = Number.isInteger(targetFrequencyOverride)
    ? targetFrequencyOverride
    : beforeMetrics.playerFrequency === 2 ? 3 : 2;
  if (targetFrequency === beforeMetrics.playerFrequency) {
    throw new Error(`frequency cutover target is already active: ${targetFrequency}`);
  }

  // Drive the ordinary player-facing chat command. This must traverse the
  // reliable request, private canonical ACK, roster update, exact reliable
  // baseline, its transport ACK, and the exact application-level acceptance
  // receipt before realtime input resumes.
  await actor.press("Enter", 80);
  await waitUntil("frequency cutover opens chat input", async () =>
    (await actor.metricsState()).uiMetrics?.chatActive === true, 2_000);
  await actor.typeChatText(`=${targetFrequency}`);
  // Keep the submit edge down across several logical frames. A busy headed
  // Chrome can otherwise receive key-down and key-up within one render turn,
  // leaving the command text open without ever emitting the reliable request.
  await actor.press("Enter", 120);
  // Observe the actual user-facing input state rather than guessing from a
  // packet-log publication race. A closed input proves the sampled Enter edge
  // submitted the command; injecting another Enter here would reopen empty
  // chat and corrupt the next genuine-keyboard cutover.
  await waitUntil("frequency cutover submits and closes chat input", async () =>
    (await actor.metricsState()).uiMetrics?.chatActive === false, 2_000);

  let rosterState = null;
  await waitUntil("frequency cutover roster update", async () => {
    const state = await actor.metricsState();
    return state.uiMetrics?.playerFrequency === targetFrequency;
  }, 15_000);
  // Long semantic runs retain intentionally rich traces. Poll only the small
  // metrics surface above so CDP serialization cannot starve the render/input
  // loop, then capture the complete state exactly once at the proven boundary.
  rosterState = await actor.pageState();
  const readySequence = rosterState.uiMetrics?.clientInputSeq;
  let applicationReceiptRace = null;
  if (delayApplicationReceipt) {
    await waitUntil("server application receipt is held after baseline transport ACK", async () => {
      const state = await actor.evaluate(
        "window.__zubspaceLifecycleReceiptImpairmentStateForTest?.() || null",
      );
      return state?.seen >= 1 && state?.buffered === 1 && state?.released === 0;
    }, 6_000);
    const blockedStart = await actor.pageState();
    const blockedSequence = blockedStart.uiMetrics?.clientInputSeq;
    const blockedPosition = {
      x: blockedStart.uiMetrics?.playerX,
      y: blockedStart.uiMetrics?.playerY,
    };
    const heldMovement = actor.holdUntil(
      "w",
      "held movement resumes immediately after baseline application receipt",
      (state) => {
        const metrics = state.uiMetrics || {};
        return (
          Number.isInteger(metrics.clientInputSeq) &&
          Number.isInteger(blockedSequence) &&
          ((metrics.clientInputSeq - blockedSequence) & 0xffff) >= 3 &&
          Math.hypot(
            (metrics.playerX ?? blockedPosition.x) - blockedPosition.x,
            (metrics.playerY ?? blockedPosition.y) - blockedPosition.y,
          ) > 8
        );
      },
      10_000,
    );
    await sleep(750);
    const blockedEnd = await actor.pageState();
    const impairmentBeforeRelease = await actor.evaluate(
      "window.__zubspaceLifecycleReceiptImpairmentStateForTest?.() || null",
    );
    const wireBeforeRelease = mergedPacketLog(blockedEnd)
      .filter((entry) => (entry.at ?? 0) >= startedAt && Array.isArray(entry.bytes));
    const baselinePacket = wireBeforeRelease.find((entry) =>
      entry.direction === "sent" &&
      entry.bytes.length >= 7 &&
      entry.bytes[0] === 0x00 &&
      entry.bytes[1] === 0x03 &&
      entry.bytes[6] === 0x03
    );
    const baselineReliableId = baselinePacket
      ? readPacketLogLeU32(baselinePacket.bytes, 2)
      : null;
    const transportAck = wireBeforeRelease.find((entry) =>
      entry.direction === "received" &&
      entry.bytes.length >= 6 &&
      entry.bytes[0] === 0x00 &&
      entry.bytes[1] === 0x04 &&
      readPacketLogLeU32(entry.bytes, 2) === baselineReliableId
    );
    const leakedRealtime = wireBeforeRelease.filter((entry) =>
      entry.direction === "sent" &&
      baselinePacket &&
      entry.seq > baselinePacket.seq &&
      (entry.bytes[0] === 0x03 || entry.bytes[0] === 0x62)
    );
    if (
      !baselinePacket ||
      !transportAck ||
      impairmentBeforeRelease?.released !== 0 ||
      impairmentBeforeRelease?.buffered !== 1 ||
      blockedEnd.uiMetrics?.clientInputSeq !== blockedSequence ||
      leakedRealtime.length !== 0
    ) {
      await actor.evaluate("window.__zubspaceReleaseLifecycleReceiptImpairmentForTest?.()");
      await heldMovement.catch(() => {});
      throw new Error(`transport ACK opened or leaked through the lifecycle barrier: ${JSON.stringify({
        blockedSequence,
        finalBlockedSequence: blockedEnd.uiMetrics?.clientInputSeq,
        impairmentBeforeRelease,
        baselinePacket,
        transportAck,
        leakedRealtime,
      })}`);
    }
    const released = await actor.evaluate(
      "window.__zubspaceReleaseLifecycleReceiptImpairmentForTest?.() === true",
    );
    if (!released) throw new Error("held lifecycle application receipt could not be released");
    const recovered = await heldMovement;
    applicationReceiptRace = {
      blockedDurationMs: 750,
      blockedSequence,
      baselineReliableId,
      transportAckSequence: transportAck.seq,
      leakedRealtimePackets: leakedRealtime.length,
      impairmentBeforeRelease,
      recoveredSequence: recovered.uiMetrics?.clientInputSeq,
      recoveredDistance: Math.hypot(
        (recovered.uiMetrics?.playerX ?? blockedPosition.x) - blockedPosition.x,
        (recovered.uiMetrics?.playerY ?? blockedPosition.y) - blockedPosition.y,
      ),
    };
  } else {
    await waitUntil("frequency cutover lifecycle barrier releases realtime", async () => {
      const state = await actor.metricsState();
      const sequence = state.uiMetrics?.clientInputSeq;
      return Number.isInteger(sequence)
        && Number.isInteger(readySequence)
        && ((sequence - readySequence) & 0xffff) >= 3;
    }, 6_000);
  }

  const after = await actor.pageState();
  const reliable = mergedPacketLog(after)
    .filter((entry) => (entry.at ?? 0) >= startedAt)
    .filter((entry) => Array.isArray(entry.bytes)
      && entry.bytes.length >= 7
      && entry.bytes[0] === 0x00
      && entry.bytes[1] === 0x03)
    .map((entry) => ({
      seq: entry.seq,
      at: entry.at,
      direction: entry.direction,
      reliableId: (entry.bytes[2]
        | (entry.bytes[3] << 8)
        | (entry.bytes[4] << 16)
        | (entry.bytes[5] << 24)) >>> 0,
      payloadType: entry.bytes[6],
      lifeSeq: entry.bytes[6] === 0x74 && entry.bytes.length >= 15
        ? (entry.bytes[11]
          | (entry.bytes[12] << 8)
          | (entry.bytes[13] << 16)
          | (entry.bytes[14] << 24)) >>> 0
        : null,
    }));
  const request = reliable.find((entry) => entry.direction === "sent" && entry.payloadType === 0x0f);
  const cutover = reliable.find((entry) => entry.direction === "received" && entry.payloadType === 0x74);
  const roster = reliable.find((entry) => entry.direction === "received" && entry.payloadType === 0x1d);
  const baseline = reliable.find((entry) => entry.direction === "sent"
    && entry.payloadType === 0x03
    && request
    && entry.seq > request.seq);
  const acceptance = reliable.find((entry) => entry.direction === "received"
    && entry.payloadType === 0x75
    && baseline
    && entry.seq > baseline.seq);
  if (!request || !cutover || !roster || !baseline || !acceptance
      || !(request.seq < cutover.seq && cutover.seq < roster.seq
        && roster.seq < baseline.seq && baseline.seq < acceptance.seq)) {
    throw new Error(`frequency cutover wire order/readiness missing: ${JSON.stringify({
      targetFrequency,
      readySequence,
      finalSequence: after.uiMetrics?.clientInputSeq,
      reliable,
    })}`);
  }
  let inactiveRocketState = null;
  if (requireActiveRocket) {
    await waitUntil("frequency cutover inactive new-life Rocket snapshot", async () => {
      const state = await actor.pageState();
      const metrics = state.uiMetrics || {};
      const snapshotAdvanced = Number.isInteger(snapshotBefore)
        && Number.isInteger(metrics.latestSnapshotId)
        && metrics.latestSnapshotId !== snapshotBefore;
      if (
        snapshotAdvanced &&
        metrics.lastSelfServerRocketLifeSeq === cutover.lifeSeq &&
        metrics.lastSelfServerRocketActivationSequence == null &&
        metrics.lastSelfServerRocketUntilTick == null &&
        metrics.pendingRocketActivationSequence == null &&
        metrics.playerRocketTimer === 0 &&
        metrics.playerRockets === rocketCountBefore &&
        metrics.semanticInputCounts?.rocket === rocketSemanticCountBefore
      ) {
        inactiveRocketState = state;
        return true;
      }
      return false;
    }, 6_000);
    // Neutral realtime inputs have crossed the lifecycle barrier. Without a
    // new F3 edge, the new life must remain inactive and spend nothing else.
    await sleep(350);
    const stableRocket = await actor.pageState();
    if (
      stableRocket.uiMetrics?.playerRocketTimer !== 0 ||
      stableRocket.uiMetrics?.playerRockets !== rocketCountBefore ||
      stableRocket.uiMetrics?.semanticInputCounts?.rocket !== rocketSemanticCountBefore
    ) {
      throw new Error(`frequency cutover reactivated Rocket without F3: ${JSON.stringify({
        before: beforeMetrics,
        inactive: inactiveRocketState?.uiMetrics,
        stable: stableRocket.uiMetrics,
        cutover,
      })}`);
    }
  }
  const displacement = Math.hypot(
    (after.uiMetrics?.playerX ?? 0) - (beforeMetrics.playerX ?? 0),
    (after.uiMetrics?.playerY ?? 0) - (beforeMetrics.playerY ?? 0),
  );
  if (!(displacement < 2_000)) {
    throw new Error(`frequency cutover reset/teleported physical state: ${JSON.stringify({
      before: beforeMetrics,
      after: after.uiMetrics,
      displacement,
    })}`);
  }
  return {
    fromFrequency: beforeMetrics.playerFrequency,
    toFrequency: targetFrequency,
    request,
    cutover,
    roster,
    baseline,
    acceptance,
    readySequence,
    finalSequence: after.uiMetrics?.clientInputSeq,
    displacement,
    activeRocketCutover: requireActiveRocket,
    rocketCountBefore,
    rocketCountAfter: inactiveRocketState?.uiMetrics?.playerRockets,
    rocketTimerBefore: beforeMetrics.playerRocketTimer,
    rocketTimerAfter: inactiveRocketState?.uiMetrics?.playerRocketTimer,
    rocketLifeAfter: inactiveRocketState?.uiMetrics?.lastSelfServerRocketLifeSeq,
    rocketActivationAfter: inactiveRocketState?.uiMetrics?.lastSelfServerRocketActivationSequence,
    rocketUntilAfter: inactiveRocketState?.uiMetrics?.lastSelfServerRocketUntilTick,
    applicationReceiptRace,
    securityAudit: assertSecurityRejectAudit(server, startedLogOffset, []),
  };
}

async function runSemanticInputGate({ actor, server, playerId }) {
  const startedLogOffset = server.logs.join("").length;
  const initial = await actor.metricsState();
  const initialMetrics = initial.uiMetrics || {};
  const initialPeerDistance = Math.hypot(
    (initialMetrics.playerX ?? 0) - (initialMetrics.remotePositionX ?? initialMetrics.playerX ?? 0),
    (initialMetrics.playerY ?? 0) - (initialMetrics.remotePositionY ?? initialMetrics.playerY ?? 0),
  );
  if (initialMetrics.inSafeZone || initialPeerDistance <= 140) {
    // The default spawn's north edge is only about 55px away. Align with the
    // same open east/south corridor used by the AOI replay before creating the
    // separation required by BombSafety.
    const eastOrientation = 0.27;
    let aligned = initial;
    for (let attempt = 0; attempt < 16; attempt += 1) {
      const orientation = aligned.uiMetrics?.playerOrientation;
      if (Math.abs(signedOrientationDelta(orientation, eastOrientation)) <= 0.025) break;
      const turn = signedOrientationDelta(orientation, eastOrientation) >= 0
        ? "ArrowRight"
        : "ArrowLeft";
      await actor.press(turn, 60);
      aligned = await actor.metricsState();
    }
    if (Math.abs(signedOrientationDelta(aligned.uiMetrics?.playerOrientation, eastOrientation)) > 0.035) {
      throw new Error(`semantic actor could not align with clear corridor: ${JSON.stringify({
        initial: initialMetrics,
        aligned: aligned.uiMetrics,
      })}`);
    }
    await actor.holdUntil(
      "ArrowUp",
      "semantic actor leaves safe zone and clears bomb safety",
      (state) => {
        const metrics = state.uiMetrics || {};
        const peerDistance = Math.hypot(
          (metrics.playerX ?? 0) - (metrics.remotePositionX ?? metrics.playerX ?? 0),
          (metrics.playerY ?? 0) - (metrics.remotePositionY ?? metrics.playerY ?? 0),
        );
        return metrics.inSafeZone === false && peerDistance > 140;
      },
      10_000,
    );
    await sleep(250);
  }
  // The ordering contract starts with both actions available. A ship-change
  // boundary can preserve a short pre-existing BombFireDelay; pressing during
  // that cooldown would prove only that both actions were correctly refused.
  await waitUntil("Rocket and bomb are available for the same weapon-phase chord", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return (
      (metrics.playerRockets ?? 0) >= 3 &&
      (metrics.playerRocketTimer ?? 0) === 0 &&
      (metrics.bombCooldown ?? 0) === 0
    );
  }, 4_000);
  const beforeState = await actor.metricsState();
  const before = beforeState.uiMetrics || {};
  const beforeCounts = before.semanticInputCounts || {};
  if (!(before.playerRockets >= 3)) {
    throw new Error(`semantic input gate requires three InitialBounty Rockets: ${JSON.stringify(before)}`);
  }
  const bombCountBefore = before.localBombCount ?? 0;
  const chordStartedAt = Date.now();

  // Concurrent DevTools keydowns plus a short held frame model a real chord.
  // The reference game orders F3 before Z. The resulting weapon-bearing position is
  // copied across all three channels on pulse-free input N. The ordered Rocket
  // action starts movement and emits its pulse on N+1; server receipt need not
  // place the independent Rocket and bomb admissions on one tick. This is an
  // action-order and cross-channel proof, not the loss proof.
  await actor.holdKeysOnFreshInputStep(["F3", "z"], 100);
  try {
    await waitUntil("weapon-phase Rocket and bomb activate once", async () => {
      const state = await actor.metricsState();
      const metrics = state.uiMetrics || {};
      const local = metrics.localGameplayState || {};
      return (
        semanticCounterDelta(beforeCounts, metrics.semanticInputCounts, "rocket") === 1 &&
        metrics.playerRockets === before.playerRockets - 1 &&
        metrics.playerRocketTimer > 0 &&
        metrics.pendingRocketActivationSequence == null &&
        metrics.lastSelfServerRocketActivationSequence != null &&
        ((metrics.localBombCount ?? 0) > bombCountBefore || metrics.bombCooldown > 0) &&
        local.pendingCombatViewActions === 0 &&
        local.combatViewActionsDropped === 0
      );
    }, 4_000);
  } catch (error) {
    const failed = await actor.pageState();
    const serverEvents = parseZubspaceEvents(server.logs.join("").slice(startedLogOffset))
      .map(({ event }) => event)
      .slice(-30);
    throw new Error(`${error.message}: ${JSON.stringify({
      before,
      failed: failed.uiMetrics,
      inputTrace: (failed.inputTrace || []).slice(-12),
      serverEvents,
    })}`);
  }
  const rocketStarted = await actor.metricsState();

  // Prove that the authorized timer drives ordinary keyboard movement, then
  // later reaches zero without a second F3 press.
  // Curve off the bomb's straight launch line before it arms. Continuing to
  // thrust east lets RocketTime catch the actor's own bomb and can naturally
  // remove prize 27 through the unrelated damage-prize path, exhausting the
  // inventory this gate intentionally spends across three Rocket edges.
  await actor.hold("ArrowRight", 650);
  await actor.hold("ArrowUp", 750);
  await sleep(200);
  const rocketMoving = await actor.metricsState();
  const movementDistance = Math.hypot(
    (rocketMoving.uiMetrics?.playerX ?? 0) - (rocketStarted.uiMetrics?.playerX ?? 0),
    (rocketMoving.uiMetrics?.playerY ?? 0) - (rocketStarted.uiMetrics?.playerY ?? 0),
  );
  if (
    !(movementDistance > 10) ||
    !(rocketMoving.uiMetrics?.playerRocketTimer > 0) ||
    !(rocketMoving.uiMetrics.playerRocketTimer < rocketStarted.uiMetrics.playerRocketTimer)
  ) {
    throw new Error(`Rocket timer did not drive a timed keyboard run: ${JSON.stringify({
      movementDistance,
      started: rocketStarted.uiMetrics,
      moving: rocketMoving.uiMetrics,
    })}`);
  }

  // Exercise the lifecycle boundary while RocketTime is still active. This
  // exact ordering is the regression: after the cutover the first new-life
  // private RocketState must be inactive and realtime input must stay live.
  const frequencyCutover = await runFrequencyCutoverGate({
    actor,
    server,
    requireActiveRocket: true,
  });

  // Distinct F3-only loss/reorder proof. The URL-gated bridge captures the
  // next tail pulse before any channel fan-out, delays exactly one recovery
  // packet, and suppresses every other game/event/reliable copy containing
  // that sequence until release.
  const targetedStartedAt = Date.now();
  const targetedArmed = await actor.evaluate(
    "window.__zubspaceArmRocketActivationImpairmentForTest?.() === true",
  );
  if (!targetedArmed) {
    throw new Error("semantic Rocket gate could not arm targeted activation impairment");
  }
  await actor.press("F3", 50);
  await waitUntil("targeted Rocket pulse ages 20-30 logical steps before release", async () => {
    const impairment = await actor.evaluate(
      "window.__zubspaceRocketActivationImpairmentStateForTest?.() || null",
    );
    if (
      impairment?.targetSequence == null ||
      impairment?.delayedTailSequence == null ||
      impairment?.delayed !== 1 ||
      impairment?.released !== 0
    ) {
      return false;
    }
    const elapsedSteps =
      ((impairment.delayedTailSequence - impairment.targetSequence) & 0xffff) + 1;
    return elapsedSteps >= 25;
  }, 3_000);
  const beforeTargetRelease = await actor.metricsState();
  const beforeTargetReleaseState = await actor.evaluate(
    "window.__zubspaceRocketActivationImpairmentStateForTest?.() || null",
  );
  const delayedLogicalSteps =
    ((beforeTargetReleaseState.delayedTailSequence - beforeTargetReleaseState.targetSequence)
      & 0xffff) + 1;
  const activationsBeforeRelease = parseZubspaceEvents(
    server.logs.join("").slice(startedLogOffset),
  ).filter(({ event }) =>
    event.type === "server_rocket_activated" && event.player_id === playerId
  );
  if (
    beforeTargetReleaseState?.delayed !== 1 ||
    beforeTargetReleaseState?.released !== 0 ||
    delayedLogicalSteps < 20 ||
    delayedLogicalSteps > 30 ||
    activationsBeforeRelease.length !== 1
  ) {
    throw new Error(`targeted Rocket reached authority before delayed release: ${JSON.stringify({
      impairment: beforeTargetReleaseState,
      metrics: beforeTargetRelease.uiMetrics,
      activationsBeforeRelease: activationsBeforeRelease.map(({ event }) => event),
    })}`);
  }
  const targetedReleaseRequested = await actor.evaluate(
    "window.__zubspaceReleaseRocketActivationImpairmentForTest?.() === true",
  );
  if (!targetedReleaseRequested) {
    throw new Error(`targeted Rocket recovery packet could not be released: ${JSON.stringify({
      impairment: beforeTargetReleaseState,
    })}`);
  }

  await waitUntil("targeted Rocket recovery packet authorizes exactly once", async () => {
    const state = await actor.metricsState();
    const metrics = state.uiMetrics || {};
    const impairment = await actor.evaluate(
      "window.__zubspaceRocketActivationImpairmentStateForTest?.() || null",
    );
    const activations = parseZubspaceEvents(
      server.logs.join("").slice(startedLogOffset),
    ).filter(({ event }) =>
      event.type === "server_rocket_activated" && event.player_id === playerId
    );
    return (
      impairment?.released === 1 &&
      activations.length === 2 &&
      semanticCounterDelta(beforeCounts, metrics.semanticInputCounts, "rocket") === 2 &&
      metrics.pendingRocketActivationSequence == null &&
      metrics.lastSelfServerRocketActivationSequence === impairment.targetSequence &&
      metrics.playerRockets === before.playerRockets - 2 &&
      metrics.playerRocketTimer > 0
    );
  }, 5_000);
  const targetedStarted = await actor.pageState();
  const targetedImpairment = await actor.evaluate(
    "window.__zubspaceRocketActivationImpairmentStateForTest?.() || null",
  );
  const targetedEvidence = targetedRocketPacketEvidence(
    targetedStarted,
    targetedStartedAt,
    targetedImpairment.targetSequence,
  );
  const timerBeforeRelease = beforeTargetRelease.uiMetrics?.playerRocketTimer;
  const timerAfterReconciliation = targetedStarted.uiMetrics?.playerRocketTimer;
  const timerIncrease = timerAfterReconciliation - timerBeforeRelease;
  const timerIncreaseTolerance = 0.04;
  const targetedActivationAfterRelease = parseZubspaceEvents(
    server.logs.join("").slice(startedLogOffset),
  ).map(({ event }) => event).find((event) =>
    event.type === "server_rocket_activated" &&
    event.player_id === playerId &&
    event.input_sequence === targetedImpairment.targetSequence
  );
  if (
    targetedImpairment.matched < 2 ||
    targetedImpairment.suppressed < 1 ||
    targetedImpairment.delayed !== 1 ||
    targetedImpairment.released !== 1 ||
    targetedImpairment.expired !== 0 ||
    targetedEvidence.delayed !== 1 ||
    targetedEvidence.released !== 1 ||
    targetedEvidence.suppressedBeforeRelease < 1 ||
    targetedEvidence.sentBeforeRelease !== 0 ||
    targetedEvidence.carryingChannelsBeforeRelease.length !== 0 ||
    !Number.isFinite(timerBeforeRelease) ||
    !Number.isFinite(timerAfterReconciliation) ||
    timerIncrease > timerIncreaseTolerance ||
    targetedActivationAfterRelease?.elapsed_logical_steps !== delayedLogicalSteps ||
    !(targetedActivationAfterRelease?.remaining_ticks < 1_000)
  ) {
    throw new Error(`targeted Rocket all-copy loss/recovery proof failed: ${JSON.stringify({
      targetedImpairment,
      targetedEvidence,
      delayedLogicalSteps,
      timerBeforeRelease,
      timerAfterReconciliation,
      timerIncrease,
      targetedActivationAfterRelease,
    })}`);
  }
  await actor.hold("ArrowUp", 700);
  const targetedMoving = await actor.metricsState();
  const targetedMovementDistance = Math.hypot(
    (targetedMoving.uiMetrics?.playerX ?? 0) - (targetedStarted.uiMetrics?.playerX ?? 0),
    (targetedMoving.uiMetrics?.playerY ?? 0) - (targetedStarted.uiMetrics?.playerY ?? 0),
  );
  if (
    !(targetedMovementDistance > 5) ||
    !(targetedMoving.uiMetrics?.playerRocketTimer > 0) ||
    !(targetedMoving.uiMetrics.playerRocketTimer < targetedStarted.uiMetrics.playerRocketTimer) ||
    targetedMoving.uiMetrics?.playerRockets !== before.playerRockets - 2
  ) {
    throw new Error(`targeted Rocket timer/movement/inventory did not converge: ${JSON.stringify({
      targetedMovementDistance,
      started: targetedStarted.uiMetrics,
      moving: targetedMoving.uiMetrics,
    })}`);
  }

  // The delayed activation's adjusted wall deadline can precede the exact
  // canonical logical interval when headed Chrome produces fewer than 60
  // movement records per wall second. Wait for both authorities: pressing
  // after only the wall timer reaches zero would still be inside the prior
  // 599-record logical interval and the server must quarantine that pulse.
  // Press a real F3 at the first logically inactive opportunity and require
  // one new authorization/spend.
  await waitUntil("delayed Rocket reaches logical expiry", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    return (
      metrics.playerRocketTimer === 0 &&
      metrics.pendingRocketActivationSequence == null &&
      metrics.playerRockets === before.playerRockets - 2
    );
  }, 12_000);
  await waitUntil("delayed Rocket reaches exact logical expiry", async () => {
    const metrics = (await actor.metricsState()).uiMetrics || {};
    if (!Number.isInteger(metrics.clientInputSeq)) {
      return false;
    }
    const offset =
      (metrics.clientInputSeq - targetedImpairment.targetSequence) & 0xffff;
    return offset >= 599 && offset < 0x8000;
  }, 3_000);
  const beforeExpiryRetrigger = await actor.metricsState();
  const expiryRetriggerStartedAt = Date.now();
  await actor.press("F3", 50);
  await waitUntil("exact post-expiry F3 authorizes and spends once", async () => {
    const state = await actor.metricsState();
    const metrics = state.uiMetrics || {};
    const activations = parseZubspaceEvents(
      server.logs.join("").slice(startedLogOffset),
    ).filter(({ event }) =>
      event.type === "server_rocket_activated" && event.player_id === playerId
    );
    return (
      activations.length === 3 &&
      semanticCounterDelta(beforeCounts, metrics.semanticInputCounts, "rocket") === 3 &&
      metrics.pendingRocketActivationSequence == null &&
      metrics.playerRockets === before.playerRockets - 3 &&
      metrics.playerRocketTimer > 0
    );
  }, 5_000);
  const expiryRetriggerStarted = await actor.metricsState();
  const expiryRetriggerActivations = parseZubspaceEvents(
    server.logs.join("").slice(startedLogOffset),
  ).map(({ event }) => event).filter((event) =>
    event.type === "server_rocket_activated" && event.player_id === playerId
  );
  const expiryRetriggerActivation = expiryRetriggerActivations[2];
  const expiryRetriggerServerTail = server.logs.join("").slice(startedLogOffset);
  if (
    expiryRetriggerActivation?.input_sequence === targetedImpairment.targetSequence ||
    expiryRetriggerActivation?.elapsed_logical_steps !== 1 ||
    expiryRetriggerActivation?.remaining_ticks !== 999 ||
    expiryRetriggerActivation?.rockets_remaining !== before.playerRockets - 3 ||
    expiryRetriggerServerTail.includes("rocket_overlap_outside_interval")
  ) {
    throw new Error(`exact post-expiry Rocket retrigger diverged: ${JSON.stringify({
      before: beforeExpiryRetrigger.uiMetrics,
      after: expiryRetriggerStarted.uiMetrics,
      activation: expiryRetriggerActivation,
      expiryRetriggerStartedAt,
    })}`);
  }

  for (const key of ["Control", "q", "F4", "F5", "F6"]) {
    await actor.press(key, 0);
  }
  for (const key of ["Delete", "Home", "End", "Insert"]) {
    await actor.chord("Shift", key, 35);
  }

  const expectedKeys = [
    "primary",
    "repel",
    "rocket",
    "brick",
    "decoy",
    "thor",
    "burst",
    "cloak",
    "antiwarp",
    "portal",
  ];
  await waitUntil("all semantic short taps dispatch exactly once", async () => {
    const state = await actor.metricsState();
    const counts = state.uiMetrics?.semanticInputCounts || {};
    return expectedKeys.every((key) =>
      semanticCounterDelta(beforeCounts, counts, key) === (key === "rocket" ? 3 : 1)
    );
  }, 4_000);
  const afterTaps = await actor.metricsState();
  await sleep(500);
  const stable = await actor.metricsState();
  const deltas = Object.fromEntries(expectedKeys.map((key) => [
    key,
    semanticCounterDelta(beforeCounts, stable.uiMetrics?.semanticInputCounts, key),
  ]));
  if (!expectedKeys.every((key) => deltas[key] === (key === "rocket" ? 3 : 1))) {
    throw new Error(`semantic taps repeated or disappeared after drain: ${JSON.stringify({
      before: beforeCounts,
      after: afterTaps.uiMetrics?.semanticInputCounts,
      stable: stable.uiMetrics?.semanticInputCounts,
      deltas,
    })}`);
  }

  await waitUntil("RocketTime expires without another activation", async () => {
    const state = await actor.metricsState();
    return state.uiMetrics?.playerRocketTimer === 0;
  }, 12_000);
  const expired = await actor.pageState();
  const serverEvents = parseZubspaceEvents(server.logs.join("").slice(startedLogOffset))
    .map(({ event }) => event);
  const rocketActivations = serverEvents.filter(
    (event) => event.type === "server_rocket_activated" && event.player_id === playerId,
  );
  if (rocketActivations.length !== 3) {
    throw new Error(`expected three distinct server Rocket activations: ${JSON.stringify(rocketActivations)}`);
  }
  const [chordActivation, targetedActivation, retriggerActivation] = rocketActivations;
  const rocketBombOrder = rocketThenBombChannelEvidence(
    expired,
    chordStartedAt,
    chordActivation.input_sequence,
  );
  const bombSpawns = serverEvents.filter(
    (event) =>
      event.type === "server_projectile_spawned" &&
      event.owner_id === playerId &&
      [3, 4].includes(event.weapon & 0x1f),
  );
  if (bombSpawns.length < 1) {
    throw new Error(`weapon-phase F3+Z produced no authoritative bomb: ${JSON.stringify({
      chordActivation,
      serverEvents: serverEvents.slice(-30),
    })}`);
  }
  if (
    rocketBombOrder.copies < 3 ||
    !["zubspace-events", "zubspace-game", "zubspace-reliable"].every((channel) =>
      rocketBombOrder.channels.includes(channel)
    ) ||
    !rocketBombOrder.activationIsNextSequence ||
    rocketBombOrder.pulseOnWeaponCopies !== 0 ||
    rocketBombOrder.activationTailPackets < 1 ||
    !rocketBombOrder.pulseAfterWeaponCopies
  ) {
    throw new Error(`F3+Z did not preserve Rocket-then-bomb action order and N+1 Rocket onset: ${JSON.stringify({
      chordActivation,
      rocketBombOrder,
    })}`);
  }
  if (
    targetedActivation.input_sequence !== targetedImpairment.targetSequence ||
    targetedActivation.elapsed_logical_steps !== delayedLogicalSteps ||
    targetedActivation.rockets_remaining !== before.playerRockets - 2 ||
    retriggerActivation.rockets_remaining !== expired.uiMetrics?.playerRockets ||
    expired.uiMetrics?.playerRockets !== before.playerRockets - 3
  ) {
    throw new Error(`Rocket server/client inventory diverged: ${JSON.stringify({
      before: before.playerRockets,
      chordActivation,
      targetedActivation,
      retriggerActivation,
      targetedImpairment,
      expired: expired.uiMetrics,
    })}`);
  }

  const serverTail = server.logs.join("").slice(startedLogOffset);
  const forbidden = [
    "rocket_not_in_inventory",
    "rocket_bomb_cooldown",
    "rocket_logical_timer_extension",
    "missing_input_history",
    "movement snap-back applied",
  ].filter((needle) => serverTail.includes(needle));
  if (forbidden.length > 0) {
    throw new Error(`semantic/Rocket run produced forbidden server diagnostics: ${JSON.stringify({ forbidden })}`);
  }
  const combatActions = {
    pending: expired.uiMetrics?.localGameplayState?.pendingCombatViewActions,
    dropped: expired.uiMetrics?.localGameplayState?.combatViewActionsDropped,
  };
  if (combatActions.pending !== 0 || combatActions.dropped !== 0) {
    throw new Error(`semantic combat-view action queue did not drain cleanly: ${JSON.stringify({
      combatActions,
      localGameplayState: expired.uiMetrics?.localGameplayState,
    })}`);
  }

  const securityAudit = assertSecurityRejectAudit(server, startedLogOffset, []);
  return {
    deltas,
    initialRockets: before.playerRockets,
    rocketsRemaining: expired.uiMetrics?.playerRockets,
    chordActivation,
    targetedActivation,
    retriggerActivation,
    rocketBombOrder,
    bombSpawns,
    targetedImpairment,
    targetedEvidence,
    delayedLogicalSteps,
    timerBeforeRelease,
    timerAfterReconciliation,
    timerIncrease,
    beforeExpiryRetrigger: beforeExpiryRetrigger.uiMetrics,
    expiryRetriggerStarted: expiryRetriggerStarted.uiMetrics,
    movementDistance,
    targetedMovementDistance,
    rocketTimerStarted: rocketStarted.uiMetrics?.playerRocketTimer,
    rocketTimerAfterMovement: rocketMoving.uiMetrics?.playerRocketTimer,
    rocketTimerExpired: expired.uiMetrics?.playerRocketTimer,
    frequencyCutover,
    combatActions,
    forbiddenDiagnostics: forbidden,
    securityAudit,
  };
}

async function waitForBrowserDamageTrace(client, event, label, sinceIndex = 0) {
  await waitUntil(`${label} browser damage trace`, async () => {
    const state = await client.combatState();
    return damageTraceMatchCount(state, event, sinceIndex) >= 1;
  }, 8_000);
}

async function assertSingleBrowserDamageTrace(client, event, label, sinceIndex = 0) {
  await waitForBrowserDamageTrace(client, event, label, sinceIndex);
  await sleep(500);
  const state = await client.combatState();
  const matches = damageTraceMatchCount(state, event, sinceIndex);
  if (matches !== 1) {
    throw new Error(`${label} expected exactly one browser damage trace for one server hit: ${JSON.stringify({
      matches,
      event,
      damageTrace: (state.damageTrace || []).slice(sinceIndex),
      uiMetrics: state.uiMetrics,
    })}`);
  }
}

async function waitForDeathPackets({
  shooter,
  victim,
  shooterId,
  victimId,
  expectedWins,
  expectedLosses,
  previousRespawnSeq = -1,
}) {
  let evidence = null;
  await waitUntil(`death packet ${shooterId}->${victimId}`, async () => {
    const [shooterState, victimState] = await Promise.all([
      shooter.lifecycleState(),
      victim.lifecycleState(),
    ]);
    const shooterDeaths = (shooterState.deathTrace || [])
      .filter((death) => deathMatches(death, shooterId, victimId));
    const victimDeaths = (victimState.deathTrace || [])
      .filter((death) => deathMatches(death, shooterId, victimId));
    const shooterSawDeath = shooterDeaths.length > 0;
    const victimSawDeath = victimDeaths.length > 0;
    const victimRespawnSample = [
      ...(victimState.lifecycleTrace || []),
      victimState.uiMetrics || {},
    ].find((metrics) =>
      (metrics.playerRespawnSeq ?? 0) > previousRespawnSeq &&
      (metrics.playerRespawnTimer ?? 0) > 0.5 &&
      (metrics.playerRespawnTimer ?? 0) <= 2.2 &&
      (metrics.playerEnergy ?? 1) <= 1 &&
      metrics.playerAlive === false &&
      metrics.serverPlayerAlive === false
    );
    const victimRespawning = victimRespawnSample != null;
    const passed =
      shooterSawDeath &&
      victimSawDeath &&
      (shooterState.uiMetrics?.selfWins ?? 0) >= expectedWins &&
      (victimState.uiMetrics?.selfLosses ?? 0) >= expectedLosses &&
      victimRespawning;
    if (passed) {
      evidence = {
        shooterSawDeath,
        victimSawDeath,
        shooterWins: shooterState.uiMetrics?.selfWins ?? null,
        victimLosses: victimState.uiMetrics?.selfLosses ?? null,
        victimRespawnTimer: victimRespawnSample?.playerRespawnTimer ?? null,
        victimRespawnSeq: victimRespawnSample?.playerRespawnSeq ?? null,
        victimEnergy: victimRespawnSample?.playerEnergy ?? null,
        victimAlive: victimRespawnSample?.playerAlive ?? null,
        victimServerAlive: victimRespawnSample?.serverPlayerAlive ?? null,
        victimRespawnObservedAt: victimRespawnSample?.at ?? null,
        shooterDeaths,
        victimDeaths,
      };
    }
    return passed;
  }, 8_000);
  return evidence;
}

async function assertDeadInputSuppressed({ cycle, victim, watcher, server, victimId, previousRespawnSeq }) {
  // `firePrimary` intentionally holds the key for 700 ms and then waits
  // another 250 ms before inspecting server damage. Requiring a full second
  // to remain in the two-second respawn left only a few milliseconds for the
  // DevTools sample. Half a second still leaves ample time for the short
  // dead-input probe below while avoiding a scheduler-dependent race.
  const minimumRespawnWindowSeconds = 0.5;
  await waitUntil(`cycle ${cycle} victim still in respawn delay before dead-input check`, async () => {
    const state = await victim.lifecycleState();
    const metrics = state.uiMetrics || {};
    return (
      (metrics.playerRespawnSeq ?? 0) > previousRespawnSeq &&
      (metrics.playerRespawnTimer ?? 0) > minimumRespawnWindowSeconds &&
      (metrics.playerEnergy ?? 1) <= 1 &&
      metrics.playerAlive === false &&
      metrics.serverPlayerAlive === false
    );
  }, 4_000);

  const [victimBefore, watcherBefore] = await Promise.all([
    victim.lifecycleState(),
    watcher.lifecycleState(),
  ]);
  const beforeMetrics = victimBefore.uiMetrics || {};
  const watcherBeforeMetrics = watcherBefore.uiMetrics || {};
  const serverDamageCount = parseServerDamageEvents(server.logs).length;
  const beforeRemoteProjectiles = watcherBeforeMetrics.remoteProjectileSpawns ?? 0;
  const beforeRemoteBullets = watcherBeforeMetrics.remoteBulletSpawns ?? 0;
  const beforeRemoteBombs = watcherBeforeMetrics.remoteBombSpawns ?? 0;
  const beforeX = beforeMetrics.playerX;
  const beforeY = beforeMetrics.playerY;

  const probeStartedAt = Date.now();
  await victim.press("Control", 30);
  await victim.press("z", 30);
  await victim.hold("ArrowUp", 50);
  await sleep(80);

  const [victimAfter, watcherAfter] = await Promise.all([
    victim.lifecycleState(),
    watcher.lifecycleState(),
  ]);
  const afterMetrics = victimAfter.uiMetrics || {};
  const watcherAfterMetrics = watcherAfter.uiMetrics || {};
  const stillDead =
    (afterMetrics.playerRespawnSeq ?? 0) > previousRespawnSeq &&
    ((afterMetrics.playerRespawnTimer ?? 0) > 0 || (afterMetrics.playerEnergy ?? 1) <= 1) &&
    afterMetrics.playerAlive === false &&
    afterMetrics.serverPlayerAlive === false;
  const cleanRespawnAfterDeadInput =
    (afterMetrics.playerRespawnSeq ?? 0) > previousRespawnSeq &&
    afterMetrics.playerRespawnTimer === 0 &&
    afterMetrics.playerAlive === true &&
    afterMetrics.serverPlayerAlive === true &&
    (afterMetrics.playerEnergy ?? 0) >= (afterMetrics.playerMaxEnergy ?? 1700) - 1;
  const respawnBoundaryPendingAfterDeadInput =
    (afterMetrics.playerRespawnSeq ?? 0) > previousRespawnSeq &&
    afterMetrics.playerRespawnTimer === 0 &&
    afterMetrics.playerAlive === true &&
    afterMetrics.serverPlayerAlive === false &&
    ["AwaitingDurableSubmission", "AwaitingApplicationReceipt"]
      .includes(afterMetrics.authorityPhase) &&
    afterMetrics.authorityInputBits === 0 &&
    (afterMetrics.playerEnergy ?? 0) >= (afterMetrics.playerMaxEnergy ?? 1700) - 1;
  if (!stillDead && !cleanRespawnAfterDeadInput && !respawnBoundaryPendingAfterDeadInput) {
    throw new Error(`cycle ${cycle} victim left dead state inconsistently during dead-input assertion: ${JSON.stringify({
      before: beforeMetrics,
      after: afterMetrics,
    })}`);
  }

  // A throttled/scheduler-stalled browser can legitimately finish its respawn
  // during this real-key probe. Measure only trace samples whose authoritative
  // life state is still dead; post-respawn thrust is ordinary live input and
  // must not be misclassified as movement while dead.
  const deadProbeSamples = [beforeMetrics, ...(victimAfter.metricsTrace || [])]
    .filter((metrics) =>
      metrics === beforeMetrics ||
      (
        Number.isFinite(metrics.at) &&
        metrics.at >= probeStartedAt - 40 &&
        metrics.playerAlive === false &&
        metrics.serverPlayerAlive === false
      )
    );
  const moveDistance = deadProbeSamples.reduce((maxDistance, metrics) => {
    if (
      !Number.isFinite(beforeX) ||
      !Number.isFinite(beforeY) ||
      !Number.isFinite(metrics.playerX) ||
      !Number.isFinite(metrics.playerY)
    ) {
      return maxDistance;
    }
    return Math.max(
      maxDistance,
      Math.hypot(metrics.playerX - beforeX, metrics.playerY - beforeY),
    );
  }, 0);
  const velocity = deadProbeSamples.reduce(
    (maxVelocity, metrics) => Math.max(
      maxVelocity,
      Math.hypot(metrics.playerVelocityX ?? 0, metrics.playerVelocityY ?? 0),
    ),
    0,
  );
  const damageFromDeadVictim = parseServerDamageEvents(server.logs)
    .slice(serverDamageCount)
    .filter((event) => event.shooterId === victimId);
  const remoteProjectileDelta =
    (watcherAfterMetrics.remoteProjectileSpawns ?? 0) - beforeRemoteProjectiles;
  const remoteBulletDelta = (watcherAfterMetrics.remoteBulletSpawns ?? 0) - beforeRemoteBullets;
  const remoteBombDelta = (watcherAfterMetrics.remoteBombSpawns ?? 0) - beforeRemoteBombs;

  if (
    moveDistance > 1.0 ||
    velocity > 1.0 ||
    damageFromDeadVictim.length > 0 ||
    remoteProjectileDelta > 0 ||
    remoteBulletDelta > 0 ||
    remoteBombDelta > 0
  ) {
    throw new Error(`cycle ${cycle} dead player input was not suppressed: ${JSON.stringify({
      moveDistance,
      velocity,
      damageFromDeadVictim,
      remoteProjectileDelta,
      remoteBulletDelta,
      remoteBombDelta,
      victimBefore: beforeMetrics,
      victimAfter: afterMetrics,
      watcherBefore: watcherBeforeMetrics,
      watcherAfter: watcherAfterMetrics,
    })}`);
  }

  return {
    probeStartedAt,
    victimRespawnTimerBefore: beforeMetrics.playerRespawnTimer ?? null,
    victimRespawnTimerAfter: afterMetrics.playerRespawnTimer ?? null,
    moveDistance,
    velocity,
    remoteProjectileDelta,
    remoteBulletDelta,
    remoteBombDelta,
    respawnedDuringProbe: cleanRespawnAfterDeadInput,
    respawnBoundaryPendingDuringProbe: respawnBoundaryPendingAfterDeadInput,
  };
}

async function waitForRespawnCycle(victim, previousRespawnSeq) {
  let sawRespawnDelay = false;
  const respawnComplete = (metrics) =>
    (metrics.playerRespawnSeq ?? 0) > previousRespawnSeq &&
    (metrics.playerRespawnTimer ?? 0) <= 0.05 &&
    metrics.playerAlive === true &&
    metrics.serverPlayerAlive === true &&
    Number.isFinite(metrics.playerEnergy) &&
    Number.isFinite(metrics.playerMaxEnergy) &&
    metrics.playerEnergy >= metrics.playerMaxEnergy - 1;

  await waitUntil("victim enters respawn delay", async () => {
    const state = await victim.lifecycleState();
    const metrics = state.uiMetrics || {};
    sawRespawnDelay = [
      ...(state.lifecycleTrace || []),
      metrics,
    ].some((sample) =>
      (sample.playerRespawnSeq ?? 0) > previousRespawnSeq &&
      (sample.playerRespawnTimer ?? 0) > 0.5 &&
      (sample.playerEnergy ?? 1) <= 1 &&
      sample.playerAlive === false &&
      sample.serverPlayerAlive === false
    );
    return sawRespawnDelay;
  }, 8_000);
  await waitUntil("victim finishes respawn", async () => {
    const state = await victim.lifecycleState();
    const metrics = state.uiMetrics || {};
    return sawRespawnDelay && [
      ...(state.lifecycleTrace || []),
      metrics,
    ].some(respawnComplete);
  }, 12_000);
}

async function assertRemoteDeathRespawnVisibility({
  cycle,
  watcher,
  traceStartedAt,
  victimId,
  predecessorLifeSeq,
  beforeReappearance = null,
}) {
  if (!Number.isInteger(victimId) || victimId < 0 || victimId > 0xffff) {
    throw new Error(`cycle ${cycle} remote visibility requires a valid victim id, got ${victimId}`);
  }
  if (!Number.isInteger(predecessorLifeSeq) ||
      predecessorLifeSeq <= 0 || predecessorLifeSeq > 0xffff_ffff) {
    throw new Error(
      `cycle ${cycle} remote visibility requires a nonzero predecessor life, got ${predecessorLifeSeq}`,
    );
  }
  const expectedLifeSeq = predecessorLifeSeq === 0xffff_ffff
    ? 1
    : predecessorLifeSeq + 1;
  const currentTrace = (state) => (state.lifecycleTrace || [])
    .filter((entry) => Number.isFinite(entry.at) && entry.at >= traceStartedAt);
  let hiddenSample = null;
  await waitUntil(`cycle ${cycle} remote victim disappears after death`, async () => {
    const state = await watcher.lifecycleState();
    hiddenSample = currentTrace(state)
      .find((entry) =>
        !(entry.remotePlayers || []).some((actor) => actor.id === victimId)
      );
    return hiddenSample != null;
  }, 6_000);

  // The private respawn receipt must still arrive within its original
  // deadline. A valid random destination can be outside this observer's AOI;
  // key-driven encounter setup is separate from the relisting deadline.
  const setupStartedAt = Date.now();
  const rendezvous = beforeReappearance ? await beforeReappearance() : null;
  const setupElapsedMs = Date.now() - setupStartedAt;
  let reappearedSample = null;
  await waitUntil(`cycle ${cycle} remote victim reappears after respawn`, async () => {
    const state = await watcher.lifecycleState();
    reappearedSample = currentTrace(state)
      .filter((entry) => entry.at > hiddenSample.at)
      .find((entry) => (entry.remotePlayers || []).some((actor) =>
        actor.id === victimId &&
        actor.lifeSeq === expectedLifeSeq &&
        Number.isFinite(actor.displayX) &&
        Number.isFinite(actor.displayY)
      ));
    return reappearedSample != null;
  }, 12_000);
  const reappearedActor = reappearedSample?.remotePlayers?.find((actor) =>
    actor.id === victimId && actor.lifeSeq === expectedLifeSeq
  );

  return {
    rendezvous,
    setupElapsedMs,
    reappearanceTimeoutMs: 12_000,
    hiddenAt: hiddenSample?.at ?? null,
    reappearedAt: reappearedSample?.at ?? null,
    reappearedLifeSeq: reappearedActor?.lifeSeq ?? null,
    reappearedPosition: reappearedActor
      ? {
          x: reappearedActor.displayX,
          y: reappearedActor.displayY,
        }
      : null,
  };
}

async function fireUntilDamage({
  label,
  shooter,
  victim,
  server,
  shooterId,
  victimId,
  sinceCount,
  requireLethal,
  timeoutMs = 45_000,
  singlePressMs = null,
  forceBomb = false,
  allowDelayedLifeSeqCatchup = false,
  onServerDamageEvent = null,
}) {
  const deadline = Date.now() + timeoutMs;
  let attempts = 0;
  while (Date.now() < deadline) {
    // This hot loop needs only the live combat boundary and bounded
    // damage/death tails. Pulling the complete packet, sprite, membership, or
    // even layout/lifecycle metrics before every physical-key attempt can
    // pause debug WASM long enough to create the very presentation hold under
    // test.
    const [shooterBefore, victimBefore] = await Promise.all([
      shooter.combatState(),
      victim.combatState(),
    ]);
    // A projectile fired by the previous attempt can land after that
    // attempt's short post-key wait. Consume that in-flight result before
    // issuing another shot, so its already-delivered browser trace cannot
    // fall outside the next attempt's trace window.
    const priorEvent = matchingDamageEvents(server, sinceCount, shooterId, victimId)
      .find((entry) => (requireLethal ? entry.lethal : true));
    if (priorEvent) {
      assertNoBulletSelfDamage(server, sinceCount, label);
      assertServerDamageAccounting(priorEvent, label);
      const callbackResult = onServerDamageEvent
        ? await onServerDamageEvent(priorEvent)
        : null;
      await Promise.all([
        assertSingleBrowserDamageTrace(victim, priorEvent, `${label} victim`),
        assertSingleBrowserDamageTrace(shooter, priorEvent, `${label} shooter`),
      ]);
      return {
        event: priorEvent,
        callbackResult,
        attempts,
        nextSinceCount: parseServerDamageEvents(server.logs).length,
      };
    }
    const shooterMetrics = shooterBefore.uiMetrics || {};
    if ((shooterMetrics.playerRespawnTimer ?? 0) > 0 || (shooterMetrics.playerEnergy ?? 0) <= 0) {
      await sleep(500);
      continue;
    }
    if (shooterMetrics.combatViewTimeline?.weaponReady !== true) {
      // Full retained-state captures are intentional scheduler stalls. The
      // product contract rejects a weapon edge while its drawn cursor is
      // behind, so wait for the explicit server-directed recovery boundary
      // before issuing the next genuine keyboard edge. Failure to recover is
      // still a hard product failure; no shot or assertion is synthesized.
      await waitUntil(`${label} shooter combat view recovers after instrumentation`, async () => {
        const state = await shooter.combatState();
        const metrics = state.uiMetrics || {};
        return (
          (metrics.playerRespawnTimer ?? 0) > 0 ||
          (metrics.playerEnergy ?? 0) <= 0 ||
          metrics.combatViewTimeline?.weaponReady === true
        );
      }, Math.min(5_000, Math.max(1, deadline - Date.now())));
      continue;
    }
    attempts += 1;
    const shooterRespawnSeqBefore = shooterMetrics.playerRespawnSeq ?? 0;
    const shooterDamageTraceStart = shooterBefore.damageTrace?.length ?? 0;
    const victimDamageTraceStart = victimBefore.damageTrace?.length ?? 0;
    if (forceBomb) {
      await shooter.fireBomb();
    } else if (singlePressMs != null) {
      await shooter.press("Control", singlePressMs);
      await sleep(250);
    } else if (attempts % 7 === 0) {
      await shooter.fireBomb();
    } else {
      await shooter.firePrimary();
    }
    const events = matchingDamageEvents(server, sinceCount, shooterId, victimId);
    assertNoBulletSelfDamage(server, sinceCount, label);
    if (singlePressMs != null && !forceBomb) {
      const shooterAfter = await shooter.combatState();
      const afterMetrics = shooterAfter.uiMetrics || {};
      const selfDeaths = selfDeathEvents(shooterAfter, shooterId);
      if (
        afterMetrics.playerAlive === false ||
        afterMetrics.serverPlayerAlive === false ||
        (
          !allowDelayedLifeSeqCatchup &&
          (afterMetrics.playerRespawnSeq ?? 0) > shooterRespawnSeqBefore
        ) ||
        selfDeaths.length > 0
      ) {
        throw new Error(`${label} shooter died or entered respawn while firing one bullet: ${JSON.stringify({
          shooterId,
          shooterBefore: shooterMetrics,
          shooterAfter: afterMetrics,
          selfDeaths,
          serverDamageEvents: parseServerDamageEvents(server.logs).slice(sinceCount),
        })}`);
      }
    }
    const event = events.find((entry) => (requireLethal ? entry.lethal : true));
    if (event) {
      assertServerDamageAccounting(event, label);
      const callbackResult = onServerDamageEvent
        ? await onServerDamageEvent(event)
        : null;
      // Server log collection and browser packet delivery are independent.
      // If either trace raced ahead of the log observer, match this event by
      // its protocol identity across the complete trace rather than excluding
      // the sample captured just before this shot.
      const victimSinceIndex = damageTraceMatchCount(victimBefore, event) > 0
        ? 0
        : victimDamageTraceStart;
      const shooterSinceIndex = damageTraceMatchCount(shooterBefore, event) > 0
        ? 0
        : shooterDamageTraceStart;
      await Promise.all([
        assertSingleBrowserDamageTrace(victim, event, `${label} victim`, victimSinceIndex),
        assertSingleBrowserDamageTrace(shooter, event, `${label} shooter`, shooterSinceIndex),
      ]);
      return {
        event,
        callbackResult,
        attempts,
        nextSinceCount: parseServerDamageEvents(server.logs).length,
      };
    }
  }

  const [shooterState, victimState] = await Promise.all([shooter.pageState(), victim.pageState()]);
  assertNoBulletSelfDamage(server, sinceCount, label);
  throw new Error(`${label} did not produce ${requireLethal ? "lethal " : ""}server damage: ${JSON.stringify({
    shooterId,
    victimId,
    sinceCount,
    serverDamageEvents: parseServerDamageEvents(server.logs).slice(-20),
    shooterMetrics: shooterState.uiMetrics,
    victimMetrics: victimState.uiMetrics,
    shooterSummary: summarizeClient(shooterState),
    victimSummary: summarizeClient(victimState),
  })}`);
}

async function runKillRespawnDamageCycle({
  cycle,
  shooter,
  victim,
  server,
  shooterId,
  victimId,
  sinceCount,
}) {
  const initialRendezvous = await rendezvousPlayers(shooter, victim, server);
  const [beforeVictim, beforeShooter] = await Promise.all([
    victim.pageState(),
    shooter.pageState(),
  ]);
  const previousRespawnSeq = beforeVictim.uiMetrics?.playerRespawnSeq ?? 0;
  const shooterTraceStartedAt = Date.now();
  let deadInput = null;
  const kill = await fireUntilDamage({
    label: `cycle ${cycle} kill`,
    shooter,
    victim,
    server,
    shooterId,
    victimId,
    sinceCount,
    requireLethal: true,
    onServerDamageEvent: async () => {
      deadInput = await assertDeadInputSuppressed({
        cycle,
        victim,
        watcher: shooter,
        server,
        victimId,
        previousRespawnSeq,
      });
    },
  });
  const deathEvidence = await waitForDeathPackets({
    shooter,
    victim,
    shooterId,
    victimId,
    expectedWins: cycle,
    expectedLosses: cycle,
    previousRespawnSeq,
  });
  const remoteVisibility = await assertRemoteDeathRespawnVisibility({
    cycle,
    watcher: shooter,
    traceStartedAt: shooterTraceStartedAt,
    victimId,
    predecessorLifeSeq: kill.event.victimLifeSeq,
    beforeReappearance: async () => {
      await waitForRespawnCycle(victim, previousRespawnSeq);
      return rendezvousPlayers(shooter, victim, server);
    },
  });

  const damage = await fireUntilDamage({
    label: `cycle ${cycle} post-respawn damage`,
    shooter,
    victim,
    server,
    shooterId,
    victimId,
    sinceCount: kill.nextSinceCount,
    requireLethal: false,
  });
  if (
    !Number.isInteger(kill.event.victimLifeSeq) ||
    !Number.isInteger(damage.event.victimLifeSeq) ||
    damage.event.victimLifeSeq <= kill.event.victimLifeSeq
  ) {
    throw new Error(`cycle ${cycle} post-respawn damage did not advance victim life_seq: ${JSON.stringify({
      kill: kill.event,
      postRespawnDamage: damage.event,
    })}`);
  }

  return {
    cycle,
    initialRendezvous,
    kill: kill.event,
    death: deathEvidence,
    deadInput,
    remoteVisibility,
    postRespawnDamage: damage.event,
    killAttempts: kill.attempts,
    postRespawnDamageAttempts: damage.attempts,
    nextSinceCount: damage.nextSinceCount,
  };
}

async function runLossySnapshotDamageCycle({
  cycle,
  shooter,
  victim,
  server,
  shooterId,
  victimId,
  sinceCount,
}) {
  const initialRendezvous = await rendezvousPlayers(shooter, victim, server);
  const [beforeVictim, beforeShooter] = await Promise.all([
    victim.pageState(),
    shooter.pageState(),
  ]);
  const previousRespawnSeq = beforeVictim.uiMetrics?.playerRespawnSeq ?? 0;
  const shooterTraceStartedAt = Date.now();
  const victimTraceStartedAt = shooterTraceStartedAt;
  let deadInput = null;
  const kill = await fireUntilDamage({
    label: `cycle ${cycle} lossy snapshot kill`,
    shooter,
    victim,
    server,
    shooterId,
    victimId,
    sinceCount,
    requireLethal: true,
    onServerDamageEvent: async () => {
      deadInput = await assertDeadInputSuppressed({
        cycle,
        victim,
        watcher: shooter,
        server,
        victimId,
        previousRespawnSeq,
      });
    },
  });

  // The rolling lifecycle rings cover only a few seconds. Capture the
  // snapshot-driven death and exact remote disappearance before navigating
  // to a new random spawn; a valid route can take longer than either ring.
  let victimLifecycleState = null;
  await waitUntil(`cycle ${cycle} forced reliable death-copy loss`, async () => {
    victimLifecycleState = await victim.pageState();
    return (victimLifecycleState.networkStats?.deathDeliveryPacketsDropped ?? 0) >= cycle;
  }, 12_000);
  const deathDeliveryPacketsDropped =
    victimLifecycleState.networkStats?.deathDeliveryPacketsDropped ?? 0;
  if (deathDeliveryPacketsDropped < cycle) {
    throw new Error(`cycle ${cycle} did not force the first reliable death copy to be lost: ${JSON.stringify({
      deathDeliveryPacketsDropped,
      networkStats: victimLifecycleState.networkStats,
    })}`);
  }
  const lifecycleSample = (victimLifecycleState.metricsTrace || [])
    .find((metrics) =>
      Number.isFinite(metrics.at) &&
      metrics.at >= victimTraceStartedAt &&
      (metrics.playerRespawnSeq ?? 0) > previousRespawnSeq &&
      (metrics.playerRespawnTimer ?? 0) > 0 &&
      metrics.playerAlive === false &&
      metrics.serverPlayerAlive === false
    );
  if (!lifecycleSample) {
    throw new Error(`cycle ${cycle} did not retain snapshot lifecycle death evidence`);
  }
  const deathReliableId =
    victimLifecycleState.networkStats?.deathDeliveryLastDroppedReliableId;
  if (!Number.isInteger(deathReliableId)) {
    throw new Error(`cycle ${cycle} did not retain the impaired reliable death id`);
  }
  let respawnRendezvous = null;
  const remoteVisibility = await assertRemoteDeathRespawnVisibility({
    cycle,
    watcher: shooter,
    traceStartedAt: shooterTraceStartedAt,
    victimId,
    predecessorLifeSeq: kill.event.victimLifeSeq,
    beforeReappearance: async () => {
      await waitForRespawnCycle(victim, previousRespawnSeq);
      respawnRendezvous = await rendezvousPlayers(shooter, victim, server);
      return respawnRendezvous;
    },
  });

  let recovery = null;
  await waitUntil(`cycle ${cycle} lossy snapshot respawn recovery`, async () => {
    const respawn = parseServerRespawnEvents(server.logs).find((event) =>
      event.playerId === victimId && event.lifeSeq > kill.event.victimLifeSeq
    );
    if (!respawn) return false;
    const [shooterState, victimState] = await Promise.all([
      shooter.pageState(),
      victim.pageState(),
    ]);
    const victimMetrics = victimState.uiMetrics || {};
    const remoteVictim = (shooterState.uiMetrics?.remotePlayers || [])
      .find((player) => player.id === victimId);
    const recovered =
      (victimMetrics.latestSnapshotServerTick ?? 0) >= respawn.serverTick &&
      victimMetrics.playerAlive === true &&
      victimMetrics.serverPlayerAlive === true &&
      Number.isFinite(victimMetrics.playerEnergy) &&
      Number.isFinite(victimMetrics.playerMaxEnergy) &&
      victimMetrics.playerEnergy >= victimMetrics.playerMaxEnergy - 1 &&
      remoteVictim != null &&
      remoteVictim.serverEnergy >= victimMetrics.playerMaxEnergy - 1;
    if (recovered) {
      recovery = {
        respawn,
        victimSnapshotId: victimMetrics.latestSnapshotId ?? null,
        victimSnapshotBaseId: victimMetrics.latestSnapshotBaseId ?? null,
        victimAckedSnapshotId: victimMetrics.lastAckedSnapshotId ?? null,
        remoteVictimEnergy: remoteVictim.serverEnergy,
      };
    }
    return recovered;
  }, 12_000);
  // Refresh the bounded reliable-event tail after recovery, so an eventual
  // retry is still compared with the earlier dead-input probe. The private
  // lifecycle sample above is retained independently of trace eviction.
  victimLifecycleState = await victim.pageState();
  const victimReliableDeaths = matchingReliableDeathEvents(
    victimLifecycleState.deathTrace || [],
    {
      reliableId: deathReliableId,
      killerId: shooterId,
      killedId: victimId,
      observedAfter: victimTraceStartedAt,
    },
  );
  const firstReliableDeathAt = victimReliableDeaths
    .map((event) => event.at)
    .filter(Number.isFinite)
    .sort((left, right) => left - right)[0] ?? null;
  const processedBeforeReliableResend =
    Number.isFinite(deadInput?.probeStartedAt) &&
    (firstReliableDeathAt == null || deadInput.probeStartedAt < firstReliableDeathAt);
  const deathEvidence = {
    delivery: "snapshot-lifecycle",
    victimSawDeath: true,
    victimRespawnSeq: lifecycleSample.playerRespawnSeq ?? null,
    victimRespawnTimer: lifecycleSample.playerRespawnTimer ?? null,
    victimEnergy: lifecycleSample.playerEnergy ?? null,
    victimAlive: lifecycleSample.playerAlive ?? null,
    victimServerAlive: lifecycleSample.serverPlayerAlive ?? null,
    lifecycleObservedAt: lifecycleSample.at ?? null,
    deathReliableId,
    firstReliableDeathAt,
    victimReliableDeaths,
  };
  if (!processedBeforeReliableResend) {
    throw new Error(`cycle ${cycle} did not process snapshot lifecycle before reliable death resend: ${JSON.stringify({
      deadInput,
      deathEvidence,
      deathDeliveryPacketsDropped,
    })}`);
  }

  const damage = await fireUntilDamage({
    label: `cycle ${cycle} lossy post-respawn damage`,
    shooter,
    victim,
    server,
    shooterId,
    victimId,
    sinceCount: kill.nextSinceCount,
    requireLethal: false,
  });
  if (
    !Number.isInteger(kill.event.victimLifeSeq) ||
    !Number.isInteger(damage.event.victimLifeSeq) ||
    damage.event.victimLifeSeq <= kill.event.victimLifeSeq
  ) {
    throw new Error(`cycle ${cycle} lossy recovery did not advance victim life_seq: ${JSON.stringify({
      kill: kill.event,
      postRespawnDamage: damage.event,
    })}`);
  }

  return {
    cycle,
    initialRendezvous,
    respawnRendezvous,
    kill: kill.event,
    death: deathEvidence,
    deadInput,
    remoteVisibility,
    lifecycleDelivery: {
      deathDeliveryPacketsDropped,
      processedBeforeReliableResend,
    },
    lossRecovery: recovery,
    postRespawnDamage: damage.event,
    killAttempts: kill.attempts,
    postRespawnDamageAttempts: damage.attempts,
    nextSinceCount: damage.nextSinceCount,
  };
}

async function runMovementCorrectionRecoveryGate({
  actor,
  server,
  serverPort,
  playerId,
  logOffset,
}) {
  // First establish the actual lost offer. The optional transport fixture
  // holds only its retry recovery until the real moving phase releases it.
  // Use the original20s offer budget for waiting AND movement, rather than
  // adding a new deadline before the existing barrier observation.
  const offerPhaseDeadline = Date.now() + 20_000;
  await waitUntil("first correction offer is lost before old-lineage movement", async () => {
    const m = await actor.metricsState();
    return m.networkStats?.movementCorrectionOffersDropped === 1 &&
      m.networkStats.movementCorrectionOffersRecovered === 0 &&
      m.uiMetrics?.localGameplayState?.pendingMovementCorrectionGeneration == null;
  }, Math.max(1, offerPhaseDeadline - Date.now()));
  const delayedMotionStart = await actor.metricsState();
  await actor.holdKeys(["ArrowUp", "ArrowRight"], 1_400);
  const delayedMotionEnd = await actor.metricsState();
  const preBarrierMovementDistance = Math.hypot(
    (delayedMotionEnd.uiMetrics?.playerX ?? 0) -
      (delayedMotionStart.uiMetrics?.playerX ?? 0),
    (delayedMotionEnd.uiMetrics?.playerY ?? 0) -
      (delayedMotionStart.uiMetrics?.playerY ?? 0),
  );
  if (!(preBarrierMovementDistance > 2)) {
    throw new Error(`movement correction delay did not retain moving old-lineage evidence: ${JSON.stringify({
      preBarrierMovementDistance,
      delayedMotionStart: delayedMotionStart.uiMetrics,
      delayedMotionEnd: delayedMotionEnd.uiMetrics,
    })}`);
  }

  if (!await actor.evaluate("window.__zubspaceReleaseMovementCorrectionOfferForTest?.() === true")) {
    throw new Error("movement correction offer recovery phase was not configured");
  }
  let barrier = null;
  await waitUntil("lost correction offer reaches the application barrier and loses its first terminal", async () => {
    barrier = await actor.metricsState();
    const stats = barrier.networkStats || {};
    const local = barrier.uiMetrics?.localGameplayState || {};
    return (
      stats.movementCorrectionOffersSeen >= 2 &&
      stats.movementCorrectionOffersDropped === 1 &&
      stats.movementCorrectionOffersRecovered === 1 &&
      stats.movementCorrectionApplicationsSent >= 1 &&
      stats.movementCorrectionAcceptancesSeen >= 1 &&
      stats.movementCorrectionAcceptancesDropped === 1 &&
      stats.movementCorrectionAcceptancesRecovered === 0 &&
      local.pendingMovementCorrectionGeneration != null &&
      local.pendingMovementCorrectionApplicationSequence != null &&
      local.pendingMovementCorrectionDeliveryId != null &&
      local.pendingMovementCorrectionActions === 0
    );
  }, Math.max(1, offerPhaseDeadline - Date.now()));
  if (Date.now() > offerPhaseDeadline) throw new Error("correction offer exceeded its original20s observation budget");

  const primaryBefore = barrier.uiMetrics?.semanticInputCounts?.primary ?? 0;
  const projectileStart = parseServerProjectileEvents(server.logs).filter((event) =>
    event.type === "server_projectile_spawned" && event.ownerId === playerId
  ).length;
  await actor.press("Control", 220);
  let heldEdge = null;
  await waitUntil("fire edge is retained while the correction terminal is missing", async () => {
    heldEdge = await actor.metricsState();
    const local = heldEdge.uiMetrics?.localGameplayState || {};
    return (
      local.pendingMovementCorrectionGeneration != null &&
      local.pendingMovementCorrectionActions === 1 &&
      (heldEdge.uiMetrics?.semanticInputCounts?.primary ?? 0) === primaryBefore &&
      parseServerProjectileEvents(server.logs).filter((event) =>
        event.type === "server_projectile_spawned" && event.ownerId === playerId
      ).length === projectileStart
    );
  }, 2_000);

  if (!await actor.evaluate("window.__zubspaceReleaseMovementCorrectionAcceptanceForTest?.() === true")) {
    throw new Error("movement correction terminal recovery phase was not configured");
  }
  let recovered = null;
  await waitUntil("lost movement correction offer recovers through its application receipt", async () => {
    recovered = await actor.metricsState();
    const stats = recovered.networkStats || {};
    const local = recovered.uiMetrics?.localGameplayState || {};
    return (
      stats.movementCorrectionOffersSeen >= 2 &&
      stats.movementCorrectionOffersDropped === 1 &&
      stats.movementCorrectionOffersRecovered === 1 &&
      stats.movementCorrectionApplicationsSent >= 1 &&
      stats.movementCorrectionAcceptancesSeen >= 2 &&
      stats.movementCorrectionAcceptancesDropped === 1 &&
      stats.movementCorrectionAcceptancesRecovered === 1 &&
      local.movementCorrectionReceiptsSeen >= 1 &&
      local.movementCorrectionReceiptsMatched >= 1 &&
      local.movementCorrectionReceiptsUnmatched === 0 &&
      local.pendingMovementCorrectionGeneration == null &&
      local.pendingMovementCorrectionApplicationSequence == null &&
      local.pendingMovementCorrectionDeliveryId == null &&
      local.pendingMovementCorrectionActions === 0 &&
      local.movementCorrectionActionsDropped === 0 &&
      (recovered.uiMetrics?.semanticInputCounts?.primary ?? 0) === primaryBefore + 1 &&
      recovered.uiMetrics?.selfCorrectionCount >= 1 &&
      recovered.uiMetrics?.lastSelfCorrectionReason === 2
    );
  }, 20_000);

  const correctionLines = serverLogSince(server, logOffset).split("\n");
  const proposal = correctionLines.find((line) =>
    line.includes("movement snapback proposed") &&
    parseNumericField(line, "player_id") === playerId
  );
  const committed = correctionLines.find((line) =>
    line.includes("movement snapback application committed") &&
    parseNumericField(line, "player_id") === playerId
  );
  const detectedSequence = parseNumericField(committed || "", "detected_sequence");
  const applicationSequence = parseNumericField(committed || "", "application_sequence");
  const correctionGeneration = parseNumericField(committed || "", "correction_generation");
  const delayedSteps =
    Number.isInteger(detectedSequence) && Number.isInteger(applicationSequence)
      ? (applicationSequence - detectedSequence) & 0xffff
      : null;
  const structuredCommits = parseZubspaceEvents(serverLogSince(server, logOffset))
    .map(({ event }) => event)
    .filter(
      (event) => event.type === "movement_snapback_committed" && event.player_id === playerId,
    );
  const structuredCommit = structuredCommits[0] || null;
  if (
    !proposal ||
    !committed ||
    !(delayedSteps > 1 && delayedSteps < 0x8000) ||
    structuredCommits.length !== 1 ||
    structuredCommit.life_seq !== recovered.uiMetrics?.playerLifeSeq ||
    structuredCommit.correction_generation !== correctionGeneration ||
    structuredCommit.detected_sequence !== detectedSequence ||
    structuredCommit.application_sequence !== applicationSequence
  ) {
    throw new Error(`movement correction did not commit after a real multi-step offer delay: ${JSON.stringify({
      proposal,
      committed,
      structuredCommits,
      detectedSequence,
      applicationSequence,
      delayedSteps,
      networkStats: recovered?.networkStats,
      localGameplayState: recovered?.uiMetrics?.localGameplayState,
    })}`);
  }

  const postReceiptLogOffset = server.logs.join("").length;
  const before = await actor.metricsState();
  await actor.holdKeys(["ArrowUp", "ArrowLeft"], 1_800);
  await sleep(500);
  const afterMovement = await actor.metricsState();
  const sequenceDistance =
    ((afterMovement.uiMetrics?.clientInputSeq ?? 0) -
      (before.uiMetrics?.clientInputSeq ?? 0)) & 0xffff;
  const movementDistance = Math.hypot(
    (afterMovement.uiMetrics?.playerX ?? 0) - (before.uiMetrics?.playerX ?? 0),
    (afterMovement.uiMetrics?.playerY ?? 0) - (before.uiMetrics?.playerY ?? 0),
  );

  await waitUntil("barrier-retained fire reaches server authority exactly once", async () => {
    return parseServerProjectileEvents(server.logs).filter((event) =>
      event.type === "server_projectile_spawned" && event.ownerId === playerId
    ).length === projectileStart + 1;
  }, 8_000);
  await sleep(500);

  const deviation = simObservationSummary(server, postReceiptLogOffset, playerId);
  const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
  if (
    sequenceDistance < 60 || sequenceDistance >= 0x8000 ||
    movementDistance <= 2 ||
    deviation.stableActiveInput.samples < 20 ||
    deviation.stableActiveInput.underTwoRatio == null ||
    deviation.stableActiveInput.underTwoRatio < 0.95 ||
    deviation.missingInputSamples !== 0 ||
    deviation.invalidInputSamples !== 0 ||
    deviation.missingInputDiagnostics !== 0 ||
    deviation.unclassifiedSamples !== 0 ||
    !Number.isInteger(status.movement_snapbacks_total) ||
    status.movement_snapbacks_total < 1
  ) {
    throw new Error(`movement correction did not resume exact movement and weapon authority: ${JSON.stringify({
      sequenceDistance,
      movementDistance,
      deviation,
      status,
      before: before.uiMetrics,
      afterMovement: afterMovement.uiMetrics,
    })}`);
  }

  return {
    offerReliableId: recovered.networkStats.movementCorrectionOfferReliableId,
    offersSeen: recovered.networkStats.movementCorrectionOffersSeen,
    offersDropped: recovered.networkStats.movementCorrectionOffersDropped,
    offersRecovered: recovered.networkStats.movementCorrectionOffersRecovered,
    applicationsSent: recovered.networkStats.movementCorrectionApplicationsSent,
    acceptancesSeen: recovered.networkStats.movementCorrectionAcceptancesSeen,
    acceptancesDropped: recovered.networkStats.movementCorrectionAcceptancesDropped,
    acceptancesRecovered: recovered.networkStats.movementCorrectionAcceptancesRecovered,
    offerRetriesHeldForScenario: recovered.networkStats.movementCorrectionOffersHeld || 0,
    terminalRetriesHeldForScenario: recovered.networkStats.movementCorrectionAcceptancesHeld || 0,
    transportRecoveryPhasesControlled: true,
    receiptsSeen: recovered.uiMetrics.localGameplayState.movementCorrectionReceiptsSeen,
    receiptsMatched: recovered.uiMetrics.localGameplayState.movementCorrectionReceiptsMatched,
    detectedSequence,
    applicationSequence,
    delayedSteps,
    structuredCommit,
    preBarrierMovementDistance,
    barrierWeapon: {
      primaryBefore,
      retainedActions: heldEdge.uiMetrics.localGameplayState.pendingMovementCorrectionActions,
      authoritativeProjectileDelta: 1,
    },
    sequenceDistance,
    movementDistance,
    authoritativeProjectileSpawned: true,
    deviation,
    serverSnapbacksTotal: status.movement_snapbacks_total,
  };
}

async function main() {
  assertMachineHeadroom("combat gate", {});
  const runStartedAt = new Date().toISOString();
  const arena = defaultArena;
  const cycleCount = defaultCycleCount;
  const wormholeFixture = wormholeAuthorityGate ? createWormholeE2eZone() : null;
  const chaosRouteFixture = chaosRouteReplanGate ? createChaosRouteReplanZone() : null;
  const scenarioFixture = wormholeFixture || chaosRouteFixture;
  if (scenarioFixture) {
    if (arena !== scenarioFixture.arena) {
      scenarioFixture.cleanup();
      throw new Error(
        `${wormholeAuthorityGate ? "ZUBSPACE_E2E_WORMHOLE_AUTHORITY" : "ZUBSPACE_E2E_CHAOS_ROUTE_REPLAN"}` +
          `=1 requires ZUBSPACE_E2E_ARENA=${scenarioFixture.arena}`,
      );
    }
    process.env.ZUBSPACE_SERVER_ZONE_ROOT = scenarioFixture.root;
    process.env.ZUBSPACE_SERVER_ZONE = scenarioFixture.arena;
  }
  if (!Number.isInteger(cycleCount) || cycleCount < 1) {
    throw new Error(`invalid ZUBSPACE_E2E_CYCLES=${process.env.ZUBSPACE_E2E_CYCLES || ""}`);
  }
  if (snapshotLossGate && !simulatedTxLossEnabled) {
    throw new Error("ZUBSPACE_E2E_SNAPSHOT_LOSS=1 requires positive ZUBSPACE_SERVER_TX_LOSS_PCT");
  }
  if (snapshotLossGate && (!Number.isSafeInteger(simulatedTxLossSeed) || simulatedTxLossSeed < 0)) {
    throw new Error("ZUBSPACE_E2E_SNAPSHOT_LOSS=1 requires integer ZUBSPACE_SERVER_TX_LOSS_SEED");
  }
  if (c2sInputRecoveryGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_C2S_INPUT_RECOVERY=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (movementCorrectionGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_MOVEMENT_CORRECTION=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (semanticInputGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_SEMANTIC_INPUT=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (staleDoorGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_STALE_DOOR=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (selectiveClockStallGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_CLOCK_STALL=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (baselineReceiptGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_BASELINE_RECEIPT=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (warpRelocationGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_WARP_RELOCATION=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (chaosRouteReplanGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_CHAOS_ROUTE_REPLAN=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (prizePhaseOneGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_PRIZES_PHASE1=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (prizePhaseOneGate && arena !== "chaos-zone") {
    throw new Error("ZUBSPACE_E2E_PRIZES_PHASE1=1 requires ZUBSPACE_E2E_ARENA=chaos-zone");
  }
  if (negativeEnergyGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_NEGATIVE_ENERGY=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (portalPhaseTwoGate && headlessChrome) {
    throw new Error("ZUBSPACE_E2E_PORTAL_PHASE2=1 requires headed Chrome (ZUBSPACE_E2E_HEADLESS=0)");
  }
  if (portalPhaseTwoGate && arena !== "chaos-zone") {
    throw new Error("ZUBSPACE_E2E_PORTAL_PHASE2=1 requires ZUBSPACE_E2E_ARENA=chaos-zone");
  }
  if (negativeEnergyGate && arena !== "extreme") {
    throw new Error("ZUBSPACE_E2E_NEGATIVE_ENERGY=1 requires ZUBSPACE_E2E_ARENA=extreme");
  }
  if (negativeEnergyGate && !["maximum", "reference"].includes(negativeEnergyLoadout)) {
    throw new Error("negative Energy requires a maximum or reference loadout");
  }
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const provenanceOptions = {
    ...buildProvenanceOptions,
    runStartedAt,
    additionalFiles: ["tests/e2e/recovery_membership.mjs", ...(scenarioFixture ? [
      "tests/e2e/wormhole_fixture.mjs",
      "tests/e2e/chaos_route_replanner.mjs",
      "tests/e2e/chaos_safe_route.mjs",
      "tests/e2e/authored_map_geometry.mjs",
    ] : [])],
  };
  const initialProvenance = collectArtifactProvenance(provenanceOptions);

  // These retained lanes exercise the legacy team/cutover authority engine,
  // explicitly separate from default FFA product acceptance (menu_shortcuts).
  if (semanticInputGate || baselineReceiptGate || prizePhaseOneGate) {
    process.env.ZUBSPACE_SERVER_TEST_TEAM_MODE = "1";
  }
  if (movementDeviationGate || warpRelocationGate || prizePhaseOneGate || negativeEnergyGate || portalPhaseTwoGate) {
    process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED = negativeEnergyGate
      ? "39064502"
      : portalPhaseTwoGate ? "22"
      : warpRelocationGate ? "412" : prizePhaseOneGate ? "708182" : "15";
  }
  if (prizePhaseOneGate) {
    // The first seeded chance draw misses every exact integer denominator
    // reachable from Chaos's authentic EMP self-splash damage range. The next
    // draw selects a loss for the following real 400-damage primary. This
    // preserves production hit/RNG order while making the headed proof stable
    // across natural EMP explosion geometry.
    process.env.ZUBSPACE_DAMAGE_PRIZE_RNG_SEED = String(prizePhaseOneDamageSeed);
    // Product entry bounty is 30, below the authored PrizeShareLimit=100.
    // Shared-prize admission therefore needs no limit override.
    // Glue/EngineShutdown has zero hidden weight in every checked-in arena.
    // Add one phase-one-only hidden-stream weight so normal registry claims
    // and private reconciliation can prove the overlapping Shutdown+EMP path.
    process.env.ZUBSPACE_SERVER_TEST_HIDDEN_ENGINE_SHUTDOWN_WEIGHT = "40";
    // Retain the exact seeded pickup identity while the replay first proves
    // EMP-only expiry; selection/effect semantics remain production paths.
    process.env.ZUBSPACE_SERVER_TEST_HIDDEN_PRIZE_TTL_TICKS = "30000";
  }
  if (
    movementDeviationGate ||
    c2sInputRecoveryGate ||
    movementCorrectionGate ||
    wormholeAuthorityGate ||
    chaosRouteReplanGate
  ) {
    process.env.ZUBSPACE_SERVER_SIM_OBSERVE = "1";
  }
  if (movementCorrectionGate) {
    process.env.ZUBSPACE_SERVER_TEST_FORCE_MOVEMENT_SNAPBACK_ONCE = "1";
  }
  const serverPort = await freePort();
  const alphaPort = await freePort();
  const betaPort = await freePort();
  const staticServer = await startStaticServer();
  // These three compatibility lanes explicitly retain the legacy team engine.
  // RadarMode4 corner components have no common conservative encounter route.
  // Declare their spatial setup independently of production FFA spawn coverage;
  // nonzero radius still lets the real server choose each initial destination.
  const teamSpawnFixture = (semanticInputGate || baselineReceiptGate || prizePhaseOneGate) ? createSpawnRegionFixture({
    zoneId: arena, spawnRegion: { x: 512, y: 512, radius: 2 },
  }) : null;
  // This receipt-ordering test needs the seeded, naturally spawned Warp during
  // its unchanged 45s lifetime. Production random spawns can require a 4,009px
  // detour, before any receipt assertion starts. Declare only this mechanic's
  // pickup-clear origin; retain FFA, real server selection, and the original
  // prize. A nearby randomized peer can otherwise collect it before setup.
  const warpSpawnFixture = warpRelocationGate ? createSpawnRegionFixture({
    zoneId: arena, spawnRegion: { x: 512, y: 512, radius: 0 },
  }) : null;
  // Portal's inventory/placement/recall proof has a15s natural pickup bound;
  // declare a pickup-clear origin separately from ordinary random-spawn QA.
  const portalSpawnFixture = portalPhaseTwoGate ? createSpawnRegionFixture({
    zoneId: arena, spawnRegion: { x: 512, y: 512, radius: 0 },
  }) : null;
  // The snapshot-loss AOI proof deliberately uses the authored central
  // west/east corridor. A random spawn can put terrain inside both measured
  // routes; declare its origin without changing the map or AOI thresholds.
  const aoiSpawnFixture = snapshotLossGate ? createSpawnRegionFixture({
    zoneId: arena, spawnRegion: { x: 512, y: 512, radius: 0 },
  }) : null;
  // This door-clock scenario asserts the canonical centre relocation below.
  // Declare that exact origin instead of inheriting the production random
  // spawn policy; ordinary join/gameplay lanes independently cover that policy.
  // Map, door phases, balance, and every measured recovery assertion stay stock.
  const staleDoorSpawnFixture = staleDoorGate ? createSpawnRegionFixture({
    zoneId: arena, spawnRegion: { x: 512, y: 512, radius: 0 },
  }) : null;
  const declaredSpawnFixture = teamSpawnFixture || warpSpawnFixture || portalSpawnFixture || aoiSpawnFixture || staleDoorSpawnFixture;
  const teamFixtureLogs = () => {
    if (!declaredSpawnFixture) return {};
    const prefix = staleDoorSpawnFixture ? "stale-door" : aoiSpawnFixture ? "snapshot-loss" : portalSpawnFixture ? "portal-phase-two" : warpSpawnFixture ? "warp-relocation" : prizePhaseOneGate ? "phase-one" : "legacy-team";
    return {
      [`${prefix}-spawn-fixture`]: JSON.stringify(declaredSpawnFixture),
      [`${prefix}-scenario-settings`]: fs.readFileSync(path.join(declaredSpawnFixture.root, arena, "arena.conf"), "utf8"),
      [`${prefix}-zone-bundle`]: fs.readFileSync(path.join(declaredSpawnFixture.root, arena, "zone.bundle"), "utf8"),
    };
  };
  const server = startRealServer(serverPort, {
    ...(wormholeAuthorityGate || chaosRouteReplanGate ? { botCount: 0 } : {}),
    ...(declaredSpawnFixture ? { zoneRoot: declaredSpawnFixture.root } : {}),
    env: {
      ...(negativeEnergyGate ? {
        ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: String(negativeEnergyLoadout === "maximum"),
      } : {}),
      ...(declaredSpawnFixture ? { ZUBSPACE_SERVER_ALLOW_ZONE_OVERRIDES: "1" } : {}),
    },
  });
  const alphaChrome = startChrome(alphaPort, "alpha");
  const betaChrome = startChrome(betaPort, "beta");
  let alpha = null;
  let beta = null;
  let alphaUrl = null;
  let betaUrl = null;

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    if (
      (c2sInputRecoveryGate || movementCorrectionGate) &&
      !server.logs.join("").includes("movement_snapback=true")
    ) {
      throw new Error("movement recovery server did not start with the shipped snap-back default enabled");
    }
    await Promise.all([waitForChrome(alphaChrome), waitForChrome(betaChrome)]);
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const pageBase = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&password=secret&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1`;
    alphaUrl = `${pageBase}&name=e2ealpha${c2sInputRecoveryGate || semanticInputGate ? "&c2s_input_impairment=armed" : ""}${movementCorrectionGate ? "&movement_correction_impairment=phased" : ""}${semanticInputGate ? "&rocket_activation_impairment=armed" : ""}${staleDoorGate ? "&s2c_stall=armed" : ""}${selectiveClockStallGate ? "&clock_stall=armed" : ""}${baselineReceiptGate ? "&lifecycle_receipt_impairment=armed" : ""}${warpRelocationGate ? "&repel_receipt_impairment=hold&boundary_position_impairment=armed" : ""}${prizePhaseOneGate ? "&prize_effect_impairment=hold" : ""}`;
    betaUrl = `${pageBase}&name=e2ebeta${snapshotLossGate ? "&death_delivery_impairment=armed" : ""}${c2sInputRecoveryGate ? "&repel_delivery_impairment=armed&repel_receipt_impairment=armed" : ""}`;
    // Start both network clients early so server-timed prize/objective streams
    // retain their normal chronology, while giving beta a deterministic join
    // lead and serializing the actual ship-selection lifecycle below.
    // Initial neutral input can trigger the forced proposal while RTC boot
    // waits for gameplay readiness, before openChromeClient returns. Capture
    // before either connection starts so proposal and terminal share a scope.
    const movementCorrectionLogOffset = server.logs.join("").length;
    // The phase-one hidden stream starts as pilots join. Random-spawn setup
    // may outlast its first wave, but live seeded prizes remain legitimate
    // candidates; the later exact-ID/expiry guards own pickup eligibility.
    const prizePhaseOneStreamLogOffset = server.logs.join("").length;
    const warpPrizeStreamLogOffset = server.logs.join("").length;
    const portalPrizeStreamLogOffset = server.logs.join("").length;
    const betaOpen = openChromeClient(betaChrome, betaUrl);
    await sleep(150);
    const alphaOpen = openChromeClient(alphaChrome, alphaUrl);
    [beta, alpha] = await Promise.all([betaOpen, alphaOpen]);
    await beta.enterShip();
    await alpha.enterShip();
    if (movementCorrectionGate || warpRelocationGate || portalPhaseTwoGate || chaosRouteReplanGate) {
      // This local transport-recovery scenario owns the pending correction
      // before any spatial flight. A rendezvous here can consume its fault.
      // Warp owns a prize-specific rendezvous before holding Repel; a generic
      // encounter first wastes the live collectible's unchanged lifetime.
      // Portal also owns its bounded natural pickup before placement/recall.
      // Chaos route recovery owns an explicitly declared canonical fixture origin.
      await waitForLivePlayerScopes(alpha, beta);
    } else {
      await rendezvousPlayers(alpha, beta, server);
      await waitForPlayersReady(alpha, beta, server);
    }

    const shipChangeEnergy = shipChangeEnergyGate ? await runShipChangeEnergyGate(alpha, beta) : null;
    if (shipChangeEnergyGate) await rendezvousPlayers(alpha, beta, server);
    if (!movementCorrectionGate && !warpRelocationGate) await waitForPlayersReady(alpha, beta, server);
    const [alphaStart, betaStart] = await Promise.all([alpha.pageState(), beta.pageState()]);
    const snapshotLossObservationStartedAt = Date.now();
    // Roster/player packets describe every member and packet-log summaries do
    // not own the browser's current authority identity. Bind scenario actions
    // to the exact Game-applied self scope instead of whichever roster member
    // happened to be parsed last.
    const alphaId = alphaStart.uiMetrics?.selfPlayerId;
    const betaId = betaStart.uiMetrics?.selfPlayerId;
    if (alphaId == null || betaId == null || alphaId === betaId) {
      throw new Error(`missing distinct player ids: ${JSON.stringify({ alphaId, betaId })}`);
    }

    const prizePhaseOneLogOffset = prizePhaseOneGate
      ? prizePhaseOneStreamLogOffset
      : server.logs.join("").length;
    const prizePhaseOneTimed = prizePhaseOneGate
      ? await runPrizePhaseOneTimedGate({
          actor: alpha,
          peer: beta,
          server,
          playerId: alphaId,
          peerPlayerId: betaId,
          logOffset: prizePhaseOneLogOffset,
        })
      : null;
    if (prizePhaseOneGate) {
      await waitForPlayersReady(alpha, beta);
    }

    const negativeEnergy = negativeEnergyGate
      ? await runNegativeEnergyReconciliationGate({
          actor: alpha,
          server,
          serverPort,
          playerId: alphaId,
          logOffset: prizePhaseOneLogOffset,
        })
      : null;

    const portalPhaseTwo = portalPhaseTwoGate
      ? await runPortalPhaseTwoGate({
          actor: alpha,
          server,
          playerId: alphaId,
          logOffset: portalPrizeStreamLogOffset,
        })
      : null;

    const baselineReceipt = baselineReceiptGate
      ? await runFrequencyCutoverGate({
          actor: alpha,
          server,
          delayApplicationReceipt: true,
        })
      : null;

    const warpRelocation = warpRelocationGate
      ? await runWarpRelocationGate({
          actor: alpha,
          peer: beta,
          server,
          serverPort,
          playerId: alphaId,
          prizeStreamLogOffset: warpPrizeStreamLogOffset,
        })
      : null;

    const wormholeAuthority = wormholeAuthorityGate
      ? await runWormholeAuthorityGate({
          actor: alpha,
          server,
          playerId: alphaId,
          fixture: wormholeFixture,
          // The arena-switch merge is a render-path cost, and a motionless ship hitching for a tenth
          // of a second is invisible to every existing witness here: authority, energy and position
          // all survive it. The gap sampler is bracketed around the relocation so the window is the
          // switch itself, not the whole lane, and the result is reported rather than asserted -
          // reproducing the hitch is the step that has to precede choosing a fix for it.
          frameWitness: {
            read: async () => JSON.parse(await alpha.evaluate(longTaskWitnessReadScript())),
          },
        })
      : null;

    const chaosRouteReplan = chaosRouteReplanGate
      ? await runChaosRouteReplanGate({
          actor: alpha,
          server,
          playerId: alphaId,
          fixture: chaosRouteFixture,
        })
      : null;

    const staleDoor = staleDoorGate
      ? await runStaleDoorReadinessGate({
          actor: alpha,
          peer: beta,
          server,
          playerId: alphaId,
        })
      : null;

    const selectiveClockStall = selectiveClockStallGate
      ? await runSelectiveClockStallGate({
          actor: alpha,
          peer: beta,
          server,
          playerId: alphaId,
        })
      : null;

    const semanticInput = semanticInputGate
      ? await runSemanticInputGate({ actor: alpha, server, playerId: alphaId })
      : null;
    const movementCorrection = movementCorrectionGate
      ? await runMovementCorrectionRecoveryGate({
          actor: alpha,
          server,
          serverPort,
          playerId: alphaId,
          logOffset: movementCorrectionLogOffset,
        })
      : null;
    if (movementCorrectionGate) {
      await rendezvousPlayers(alpha, beta, server);
      await waitForPlayersReady(alpha, beta, server);
    }
    const sinceStart = parseServerDamageEvents(server.logs).length;
    const cycles = [];
    let sinceCount = sinceStart;
    const combatPreludeEnabled =
      !c2sInputRecoveryGate &&
      !movementCorrectionGate &&
      !semanticInputGate &&
      !staleDoorGate &&
      !selectiveClockStallGate &&
      !baselineReceiptGate &&
      !warpRelocationGate &&
      !wormholeAuthorityGate &&
      !chaosRouteReplanGate &&
      !prizePhaseOneGate &&
      !negativeEnergyGate &&
      !portalPhaseTwoGate;
    const replayCycleCount = combatPreludeEnabled ? cycleCount : 0;
    for (let cycle = 1; cycle <= replayCycleCount; cycle += 1) {
      // The entry check cannot see load that arrives after the lane starts, and a
      // busy machine fails these timing assertions on correct code. Refusing per
      // cycle reads as an invalid run rather than a combat regression.
      assertMachineHeadroom(`combat gate cycle ${cycle}`, {});
      const runCycle = snapshotLossGate
        ? runLossySnapshotDamageCycle
        : runKillRespawnDamageCycle;
      const result = await runCycle({
        cycle,
        shooter: alpha,
        victim: beta,
        server,
        shooterId: alphaId,
        victimId: betaId,
        sinceCount,
      });
      cycles.push(result);
      sinceCount = result.nextSinceCount;
    }

    const counterDamage = combatPreludeEnabled
      ? await fireUntilDamage({
          label: "counter damage beta to alpha",
          shooter: beta,
          victim: alpha,
          server,
          shooterId: betaId,
          victimId: alphaId,
          sinceCount,
          requireLethal: false,
          timeoutMs: 20_000,
          singlePressMs: 220,
          // Under seeded outbound loss, an already-completed respawn's local
          // life-sequence update can arrive during this key press. The alive
          // checks, self-death trace, and server damage event still reject an
          // actual shooter death during the one-bullet assertion.
          allowDelayedLifeSeqCatchup: snapshotLossGate,
        })
      : null;
    if (counterDamage) {
      sinceCount = counterDamage.nextSinceCount;
    }

    let prizePhaseOneDamageLoss = prizePhaseOneTimed?.damageLoss || null;
    if (prizePhaseOneGate && !prizePhaseOneDamageLoss) {
      await waitUntil("server damage-prize loss reaches the private browser state", async () => {
        const losses = parseDamagePrizeLosses(server, prizePhaseOneLogOffset);
        for (const loss of losses) {
          const client = loss.playerId === alphaId ? alpha : loss.playerId === betaId ? beta : null;
          if (!client) continue;
          const state = await client.pageState();
          const local = state.uiMetrics?.localGameplayState;
          const browserState = [
            {
              lifeSeq: local?.prizeAuthorityLifeSeq,
              revision: local?.prizeAuthorityRevision,
              damagePrizeLossSeq: local?.damagePrizeLossSeq,
              damagePrizeLossId: local?.damagePrizeLossId,
            },
            ...(state.prizeStateTrace || []),
          ].find((candidate) =>
            candidate.damagePrizeLossSeq === loss.damageSeq &&
            candidate.damagePrizeLossId === loss.prizeId &&
            Number.isFinite(candidate.revision) &&
            ((candidate.revision - loss.revision) >>> 0) < 0x8000_0000
          );
          if (browserState) {
            prizePhaseOneDamageLoss = {
              ...loss,
              browser: loss.playerId === alphaId ? "alpha" : "beta",
              browserLifeSeq: browserState.lifeSeq,
              browserRevision: browserState.revision,
              browserRevisionDelta: (browserState.revision - loss.revision) >>> 0,
            };
            return true;
          }
        }
        return false;
      }, 8_000);
    }

    let c2sInputRecovery = null;
    if (c2sInputRecoveryGate) {
      const cpuThrottleRate = 2;
      await Promise.all([
        alpha.setCpuThrottlingRate(cpuThrottleRate),
        beta.setCpuThrottlingRate(cpuThrottleRate),
      ]);
      try {
        c2sInputRecovery = await runC2sInputRecoverySoak({
          actor: alpha,
          peer: beta,
          server,
          serverPort,
          playerId: alphaId,
          participantIds: [alphaId, betaId],
        });
        c2sInputRecovery.cpuThrottling = {
          rate: cpuThrottleRate,
          participants: [alphaId, betaId],
        };
      } finally {
        await Promise.all([
          alpha.setCpuThrottlingRate(1),
          beta.setCpuThrottlingRate(1),
        ]);
      }
    }

    const movementObservationLogOffset = server.logs.join("").length;
    const movementDeviation = movementDeviationGate
      ? await runMovementDeviationSoak({
          actor: alpha,
          server,
          playerId: alphaId,
          logOffset: movementObservationLogOffset,
        })
      : null;
    if (movementDeviationGate) {
      // Restore the normal combat profile through the same player-facing key;
      // the focused movement gate is complete after this lifecycle reset.
      await alpha.press("Escape", 60);
      await alpha.press("2", 60);
      await sleep(700);
      await rendezvousPlayers(alpha, beta, server);
      await waitForPlayersReady(alpha, beta);
    }

    const aoiLifecycle = snapshotLossGate
      ? await assertAoiLeaveReenterUnderLoss({
          actor: beta,
          watcher: alpha,
          server,
          actorId: betaId,
        })
      : null;
    if (shipChangeEnergyGate && arena === "chaos-zone") {
      const origins = await Promise.all([alpha, beta].map(async client => {
        const { uiMetrics: metrics } = await client.metricsState();
        return { x: metrics.playerX, y: metrics.playerY };
      }));
      const geometry = authoredMapGeometry(fs.readFileSync(
        path.join(repoRoot, "assets/zones/chaos-zone/bdhadar.lvl")));
      const target = uninterruptedMotionStage(geometry, origins);
      const routes = await Promise.all([alpha, beta].map(client =>
        driveClientToWorldTarget(client, target, 20_000, true, 6, null, 100)));
      if (!routes.every(route => route.reached)) {
        throw new Error(`ship-energy motion staging failed: ${JSON.stringify({ target, routes })}`);
      }
      shipChangeEnergy.motionStaging = { origins, target, routes };
      await waitForPlayersReady(alpha, beta, server);
    }
    const movement = snapshotLossGate || movementCorrectionGate || semanticInputGate || staleDoorGate || selectiveClockStallGate || baselineReceiptGate || warpRelocationGate || wormholeAuthorityGate || chaosRouteReplanGate || prizePhaseOneGate || negativeEnergyGate || portalPhaseTwoGate
      ? []
      : await Promise.all([alpha.driveForTrace(), beta.driveForTrace()]);
    const remoteMotion = snapshotLossGate ? [aoiLifecycle.outboundMotion] : [];
    const remoteRotation = [];
    if (!snapshotLossGate && !movementDeviationGate && !c2sInputRecoveryGate && !movementCorrectionGate && !semanticInputGate && !staleDoorGate && !selectiveClockStallGate && !baselineReceiptGate && !warpRelocationGate && !wormholeAuthorityGate && !chaosRouteReplanGate && !prizePhaseOneGate && !negativeEnergyGate && !portalPhaseTwoGate) {
      remoteMotion.push(await alpha.driveRemoteSmoothness(beta, "alpha watched by beta"));
      remoteMotion.push(await beta.driveRemoteSmoothness(alpha, "beta watched by alpha"));
      remoteRotation.push(await alpha.driveRemoteRotationSmoothness(beta, "alpha watched by beta"));
      remoteRotation.push(await beta.driveRemoteRotationSmoothness(alpha, "beta watched by alpha"));
    }
    await sleep(1000);
    const [alphaFinal, betaFinal] = await Promise.all([alpha, beta].map(client =>
      c2sInputRecoveryGate ? client.artifactState() : client.pageState()));
    const seriousEvents = [...alpha.events, ...beta.events].filter(
      (event) => event.type === "error" || event.type === "exception" || isRejectedDraw(event),
    );
    if (seriousEvents.length > 0) {
      throw new Error(`browser console errors: ${JSON.stringify(seriousEvents)}`);
    }
    const alphaSummary = summarizeClient(alphaFinal);
    const betaSummary = summarizeClient(betaFinal);
    if (!movementDeviationGate && !c2sInputRecoveryGate && !movementCorrectionGate && !semanticInputGate && !staleDoorGate && !selectiveClockStallGate && !baselineReceiptGate && !warpRelocationGate && !wormholeAuthorityGate && !chaosRouteReplanGate && !prizePhaseOneGate && !negativeEnergyGate && !portalPhaseTwoGate) {
      // Per-input debug observation intentionally adds synchronous logging
      // overhead; cadence is covered by the default and loss-specific modes.
      assertSnapshotCadence("alpha", alphaSummary, alphaFinal, snapshotLossGate);
      assertSnapshotCadence("beta", betaSummary, betaFinal, snapshotLossGate);
    }
    assertNoClientDeathRequests("alpha", alphaSummary);
    assertNoClientDeathRequests("beta", betaSummary);
    assertClockTrace("alpha", alphaFinal);
    assertClockTrace("beta", betaFinal);
    const snapshotLossEvidence = snapshotLossGate
      ? assertSnapshotLossEvidence({
          server,
          startedAt: snapshotLossObservationStartedAt,
          clients: [
            { label: "alpha", start: alphaStart, final: alphaFinal },
            { label: "beta", start: betaStart, final: betaFinal },
          ],
        })
      : null;
    // Parse every structured category/reason dynamically. Normal and induced
    // transport-loss gates intentionally whitelist nothing: any new position,
    // weapon, lifecycle, frequency, admission, witness, or security reject is
    // therefore a hard failure instead of disappearing outside a static list.
    const securityAudit = assertSecurityRejectAudit(server, 0, []);
    const dataChannelDeliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [alphaChrome.logs, betaChrome.logs],
      // A scenario that already ended one client's session (death-delivery impairment gates) must
      // not fail the cleanup wait: the audit itself confirms the session actually closed.
      disconnect: async () => {
        await Promise.all([
          alpha.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
          beta.evaluate("window.__zubspaceSendDisconnectForTest?.(); true").catch(() => {}),
        ]);
      },
    });
    const serverLifecycle = assertStructuredServerLifecycle({
      server,
      cycles,
      counterDamage,
    });
    const finalProvenance = await collectArtifactProvenanceAsync(provenanceOptions);
    assertArtifactProvenanceMatches(initialProvenance, finalProvenance);
    const successArtifacts = successArtifactDir
      ? await saveBrowserE2eArtifacts({
          artifactDir: successArtifactDir,
          server,
          staticServer,
          urls: [alphaUrl, betaUrl],
          clients: [
            { label: "alpha", client: alpha },
            { label: "beta", client: beta },
          ],
          extraLogs: {
            "alpha-chrome": alphaChrome.logs,
            "beta-chrome": betaChrome.logs,
            ...teamFixtureLogs(),
          },
          provenance: finalProvenance,
        })
      : null;
    const result = {
      ok: true,
      transport: "rtc",
      browserPair: "chrome-chrome",
      arena,
      cycleCount,
      combatPreludeEnabled,
      legacyTeamSpawnFixture: teamSpawnFixture,
      warpRelocationSpawnFixture: warpSpawnFixture,
      portalPhaseTwoSpawnFixture: portalSpawnFixture,
      snapshotLossSpawnFixture: aoiSpawnFixture,
      staleDoorSpawnFixture,
      simulatedTxLossEnabled,
      simulatedTxLossPct,
      simulatedTxLossSeed,
      snapshotLossGate,
      snapshotLossEvidence,
      securityAudit,
      dataChannelDeliveryAudit,
      provenance: finalProvenance,
      aoiLifecycle,
      alphaId,
      betaId,
      cycles,
      counterDamage: counterDamage
        ? {
            damage: counterDamage.event,
            attempts: counterDamage.attempts,
          }
        : null,
      movement: snapshotLossGate || movementCorrectionGate || semanticInputGate || staleDoorGate || selectiveClockStallGate || baselineReceiptGate || warpRelocationGate || wormholeAuthorityGate || chaosRouteReplanGate || prizePhaseOneGate || negativeEnergyGate || portalPhaseTwoGate
        ? null
        : {
            alpha: compactMovementTrace(movement[0]),
            beta: compactMovementTrace(movement[1]),
          },
      movementDeviation,
      c2sInputRecovery,
      movementCorrection,
      semanticInput,
      staleDoor,
      selectiveClockStall,
      baselineReceipt,
      warpRelocation,
      wormholeAuthority,
      chaosRouteReplan,
      negativeEnergy,
      portalPhaseTwo,
      prizePhaseOne: prizePhaseOneGate ? {
        damagePrizeRngTestOverride: {
          seed: prizePhaseOneDamageSeed,
          empSelfSplashMissDamageRange: [1, 730],
          followingPrimaryDamage: 400,
          selectedPrizeId: 22,
          reason: "preserve authentic EMP self-splash RNG order, then select the following real Chaos primary",
        },
        prizeShareEligibility: {
          checkedInLimit: 100,
          startingBounty: 30,
          overrideApplied: false,
        },
        hiddenEngineShutdownWeightTestOverride: {
          checkedInValue: 0,
          overrideValue: 40,
          reason: "all checked-in arenas assign Glue/EngineShutdown hidden weight zero",
        },
        hiddenPrizeTtlTestOverride: {
          checkedInRangeTicks: [4000, 8000],
          overrideTicks: 30000,
          reason: "retain the exact seeded Shutdown identity through the preceding EMP-only expiry proof",
        },
        timed: prizePhaseOneTimed,
        spawnFixture: teamSpawnFixture,
        damageLoss: prizePhaseOneDamageLoss,
      } : null,
      remoteMotion,
      remoteRotation,
      serverLifecycle,
      successArtifacts,
      shipChangeEnergy,
      alpha: {
        uiMetrics: compactMetrics(alphaFinal.uiMetrics),
        inputTrace: compactInputTrace(alphaFinal.inputTrace),
        summary: compactSummary(alphaSummary),
      },
      beta: {
        uiMetrics: compactMetrics(betaFinal.uiMetrics),
        inputTrace: compactInputTrace(betaFinal.inputTrace),
        summary: compactSummary(betaSummary),
      },
      serverDamageEvents: parseServerDamageEvents(server.logs).slice(-30),
    };
    if (successArtifactDir) {
      const resultPath = path.join(successArtifactDir, "result.json");
      fs.writeFileSync(
        resultPath,
        `${JSON.stringify(result, null, 2)}\n`,
      );
      validateArtifactPathReferences(result, resultPath);
    }
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    const artifactDir = path.join("/tmp", `zubspace-combat-e2e-${Date.now()}-${process.pid}`);
    const artifacts = await saveBrowserE2eArtifacts({
      artifactDir,
      server,
      staticServer,
      urls: [alphaUrl, betaUrl],
      clients: [
        { label: "alpha", client: alpha },
        { label: "beta", client: beta },
      ],
      extraLogs: {
        "alpha-chrome": alphaChrome.logs,
        "beta-chrome": betaChrome.logs,
        ...teamFixtureLogs(),
      },
    });
    reportBrowserE2eFailure(error, artifacts);
    throw error;
  } finally {
    await alpha?.close().catch(() => {});
    await beta?.close().catch(() => {});
    await Promise.all([
      stopProcess(alphaChrome.child),
      stopProcess(betaChrome.child),
      stopProcess(server.child),
    ]).catch(() => {});
        // maxRetries is not cosmetic. rmSync throws ENOTEMPTY when a just-stopped Chrome is still flushing its
    // profile directory, and a lane that launched the browser under a provisioned display (xvfb-run wraps the
    // process, so the group teardown lands a beat differently) hit exactly that. The directory
    // is this lane's own throwaway profile, so retrying the delete is the fix; failing the gate on it is not.
    ;
    fs.rmSync(betaChrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
    wormholeFixture?.cleanup();
    chaosRouteFixture?.cleanup();
    if (teamSpawnFixture) fs.rmSync(teamSpawnFixture.root, { recursive: true, force: true });
    if (warpSpawnFixture) fs.rmSync(warpSpawnFixture.root, { recursive: true, force: true });
    if (portalSpawnFixture) fs.rmSync(portalSpawnFixture.root, { recursive: true, force: true });
    if (aoiSpawnFixture) fs.rmSync(aoiSpawnFixture.root, { recursive: true, force: true });
    if (staleDoorSpawnFixture) fs.rmSync(staleDoorSpawnFixture.root, { recursive: true, force: true });
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
  chromePageState,
  damageTraceMatches,
  damageTraceMatchCount,
  dispatchChromeKey,
  evaluateChrome,
  fireUntilDamage,
  focusCanvas,
  acquireChromeInputFocusLease,
  assertServerDamageAccounting,
  keyEvent,
  main,
  matchingDamageEvents,
  mergedPacketLog,
  openChromeClient,
  parseServerDamageEvents,
  parseServerProjectileEvents,
  runKillRespawnDamageCycle,
  startChrome,
  compactMetrics,
  compactMovementTrace,
  assertRemoteMotionSmooth,
  assertRemoteMotionRecoversUnderLoss,
  assertNoRemoteSnapDuringContinuousVisibility,
  assertRemoteRotationSmooth,
  assertRemoteRotationRecoversUnderLoss,
  appliedSnapshotGapSummary,
  compactSummary,
  summarizeClient,
  waitForBrowserDamageTrace,
  assertSingleBrowserDamageTrace,
  assertSnapshotLossEvidence,
  browserE2eFailureFooter,
  receivedSnapshotDiscontinuities,
  waitForChrome,
  waitForDeathPackets,
  waitForPlayersReady,
  waitForLivePlayerScopes,
  exactLiveGameplayScope,
  rendezvousPlayers,
  waitForRespawnCycle,
  waitUntil,
  saveBrowserE2eArtifacts,
  parseStructuredSecurityRejects,
  assertSecurityRejectAudit,
  driverDataChannelFullSummary,
  driveClientToWorldTarget,
  assertDataChannelDeliveryAudit,
  assertBrowserDataChannelDeliveryAudit,
  repelDeliveryQueueSummary,
  brakeClientVelocity,
  brakeOrientationDisposition,
  brakeVelocityDisposition,
  arrivalControlDistance,
  authoredDoorClosedForCurrentAndNext,
  orientClientForProtocolDirection,
  movementSnapbackSummary,
  logicalInputLifecyclePaused,
  dataChannelDrainDisposition,
  matchingReliableDeathEvents,
  simObservationSummary,
  simReplayStepSummary,
  simulatedLossSummary,
  c2sRecoveryLogSummary,
  c2sInputStats,
  subtractC2sInputStats,
  c2sImpairmentPacketEvidence,
  runC2sInputRecoverySoak,
  runWormholeAuthorityGate,
  clientPositionWarpMarker,
  inputPulseSequences,
  inputTailSequence,
};
