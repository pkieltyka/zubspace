#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { pathToFileURL } from "node:url";
import {
  createSession, deleteSession, execute, firefoxArtifactClient, firefoxKeyDown, firefoxKeyUp,
  freePort, freezeQaTrace, openClient, pageState, screenshot, sleep,
  startGeckodriver, startRealServer, startStaticServer, stopProcess,
  waitForGeckodriver, waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";
import {
  assertDataChannelDeliveryAudit, assertSecurityRejectAudit, openChromeClient,
  saveBrowserE2eArtifacts, startChrome, waitForChrome, waitUntil,
} from "./combat_replay.mjs";
import { assertArtifactProvenanceMatches, collectArtifactProvenance } from "./artifact_provenance.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { artifactFileManifest, validateArtifactPathReferences } from "./artifact_paths.mjs";
import { assertQaTraceCoverage } from "./qa_trace_coverage.mjs";
import { assertCompleteQaTraceCaptures, captureBrowserArtifactState } from "./qa_trace_capture.mjs";
import { assertQaServerBinaryProvenance, assertQaWasmProvenance,
  validateQaTraceChildResults } from "./qa_trace_artifacts.mjs";

const privateSentinel = "qa-unrelated-private-sentinel";

// Only the diagnostic endpoint is impaired. The wrapper never reads or exports
// private headers and cannot touch signaling, DataChannels, or gameplay state.
export function installTraceUploadFault(endpoint, mode) {
  if (window.__qaDrillRestoreFetch) throw new Error("trace fault already installed");
  const original = window.fetch;
  const evidence = { mode, calls: 0, status: null };
  window.__qaDrillUploadFault = evidence;
  window.__qaDrillRestoreFetch = () => {
    window.fetch = original;
    delete window.__qaDrillRestoreFetch;
  };
  window.fetch = async function(input, init) {
    const url = typeof input === "string" ? input : input.url;
    if (url !== endpoint) return original.call(window, input, init);
    evidence.calls += 1;
    if (mode === "dropped") throw new TypeError("qa_drill_dropped_upload");
    const options = mode === "malformed" ? { ...init, body: "{" } : init;
    const response = await original.call(window, input, options);
    evidence.status = response.status;
    return response;
  };
}

export function assertJoinedTrace(joined, bundleId, provenance) {
  assert.equal(joined.schema, "zubspace-qa-trace-joined-v1");
  assert.equal(joined.version, 1);
  assert.equal(joined.bundleId, bundleId);
  assert.equal(joined.browser.bundleId, bundleId);
  const serverBinary = assertQaServerBinaryProvenance(joined.browser, provenance);
  const wasm = assertQaWasmProvenance(joined.browser, provenance);
  assert.deepEqual(joined.provenance?.serverBinary, serverBinary);
  for (const key of ["serverSessionId", "arenaStreamId", "playerId"]) {
    assert.equal(joined.browser.scope[key], joined.scope[key]);
  }
  const context = joined.server.events.find((event) => event.type === "qa_trace_context_issued");
  if (context) assert.equal(context.bundleId, bundleId);
  else assert.ok(joined.server.truncated && joined.server.overwrittenEvents > 0,
    "a bounded ring must disclose eviction of its initial context event");
  assert.ok(joined.server.retainedEvents <= joined.server.limits.maxEvents);
  assert.ok(joined.server.retainedBytes <= joined.server.limits.maxBytes);
  assert.ok(Buffer.byteLength(JSON.stringify(joined.browser)) <= 240 * 1024);
  assert.ok(joined.browser.rings.inputs.length > 0);
  assert.ok(joined.browser.provenance.clientBuildVersion);
  assert.ok(Number.isInteger(joined.browser.provenance.mapChecksum));
  assert.ok(Number.isInteger(joined.browser.provenance.arenaSettingsChecksum));
  assert.equal(typeof joined.browser.truncated, "boolean");
  assert.ok(joined.browser.overwritten && joined.browser.limits);
  const forbidden = /sdp|icecandidate|credential|authorization|uploadtoken|authtoken|chat|voice|audiopayload|peeraddress/i;
  function visit(value) {
    if (Array.isArray(value)) return value.forEach(visit);
    if (value && typeof value === "object") {
      for (const [key, entry] of Object.entries(value)) {
        assert.ok(!forbidden.test(key), `forbidden QA field: ${key}`);
        visit(entry);
      }
    }
  }
  visit(joined);
  assert.ok(!JSON.stringify(joined).includes(privateSentinel));
  assert.deepEqual(Object.keys(joined.browser.location).sort(), ["origin", "path"]);
  return { bundleId, serverBinary, wasm, serverEvents: joined.server.retainedEvents,
    browserBytes: Buffer.byteLength(JSON.stringify(joined.browser)),
    redacted: true, correlated: true };
}

function inputs(server) {
  return server.logs.join("").split("\n")
    .filter((line) => line.includes("human authority input trace"))
    .map((line) => Object.fromEntries([...line.matchAll(/(\w+)=(-?[\d.]+)/g)]
      .map((match) => [match[1], Number(match[2])])));
}

async function proveGenuineRotation(actor, server, label) {
  await waitUntil(`${label}: initial accepted input`, async () => inputs(server).length > 0);
  const before = inputs(server).at(-1);
  const offset = inputs(server).length;
  await actor.keyDown("ArrowLeft");
  try {
    await waitUntil(`${label}: genuine rotation accepted`, async () => inputs(server).slice(offset)
      .some((input) => input.input_bits !== 0 && input.input_direction !== before.input_direction));
    await sleep(200);
  } finally {
    await actor.keyUp("ArrowLeft");
  }
  const releaseOffset = inputs(server).length;
  await waitUntil(`${label}: release accepted`, async () => inputs(server).slice(releaseOffset)
    .some((input) => input.input_bits === 0));
  const after = inputs(server).at(-1);
  const accepted = inputs(server).slice(offset);
  assert.ok(accepted.every((input) => input.player_id === before.player_id && input.life_seq === before.life_seq));
  assert.equal(after.input_bits, 0);
  return { label, before, after, acceptedSamples: accepted.length,
    rotated: true, sameLife: true, releaseNeutral: true };
}

async function main() {
  const browser = process.env.ZUBSPACE_E2E_BROWSER || "chrome";
  assert.ok(["chrome", "firefox"].includes(browser));
  const httpHost = process.env.ZUBSPACE_E2E_HTTP_HOST || "127.0.0.1";
  assert.ok(httpHost === "127.0.0.1" || Object.values(os.networkInterfaces()).flat()
    .some(address => address?.family === "IPv4" && address.address === httpHost),
  "QA HTTP host must be an address of this test machine");
  process.env.ZUBSPACE_SERVER_QA_TRACE_UPLOAD = "1";
  process.env.ZUBSPACE_SERVER_SIM_OBSERVE = "1";
  const runStartedAt = new Date().toISOString();
  const e2eScript = "tests/e2e/qa_trace_drill.mjs";
  const provenance = collectArtifactProvenance({ runStartedAt, e2eScript });
  const artifactDir = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
    `/tmp/zubspace-qa-trace-${browser}-${Date.now()}-${process.pid}`;
  const serverPort = await freePort();
  const staticServer = await startStaticServer({ host: httpHost });
  const server = startRealServer(serverPort, { zone: "chaos-zone", botCount: 0 });
  const browserProcess = browser === "chrome" ? startChrome(await freePort(), "qa-trace")
    : startGeckodriver(await freePort(), "qa-trace");
  const rtc = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
  const url = `http://${httpHost}:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtc)}&arena=chaos-zone&name=qa${browser}&unrelated=${privateSentinel}`;
  let actor = null;
  const evidence = { browser, controls: [], uploads: [], clean: null, forcedFailure: null };
  const open = async (trace, automaticUpload = false) => {
    const clientUrl = `${url}${trace ? "&trace=1&debug=31337" : ""}${automaticUpload ? "&qa_upload=1" : ""}`;
    if (browser === "chrome") return openChromeClient(browserProcess, clientUrl);
    await createSession(browserProcess);
    await openClient(browserProcess, clientUrl);
    return {
      ...firefoxArtifactClient(browserProcess),
      evaluate: (expression) => execute(browserProcess, `return (${expression});`),
      keyDown: (key) => firefoxKeyDown(browserProcess, key),
      keyUp: (key) => firefoxKeyUp(browserProcess, key),
      freezeQaTrace: (options) => freezeQaTrace(browserProcess, options),
      pageState: () => pageState(browserProcess), screenshot: () => screenshot(browserProcess),
      close: () => deleteSession(browserProcess),
    };
  };
  const disconnect = async () => {
    await actor.evaluate("(window.__zubspaceSendDisconnectForTest(), true)");
    await waitUntil("trace drill disconnect and queue cleanup", async () => {
      const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
      return status.clients === 0 && status.joinedPlayers === 0 && status.rtc_outbound_queue_packets === 0;
    });
    await actor.close();
    actor = null;
    let cleanup = null;
    await waitUntil("trace drill RTC teardown completes", async () => {
      cleanup = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
      return cleanup.clients === 0 && cleanup.rtc_sessions === 0 &&
        cleanup.rtc_outbound_queue_packets === 0 && cleanup.rtc_outbound_queue_bytes === 0 &&
        Object.values(cleanup.rtc_inbound_queue_packets || {}).every((count) => count === 0);
    });
    return cleanup;
  };
  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    if (browser === "chrome") await waitForChrome(browserProcess);
    else await waitForGeckodriver(browserProcess);
    actor = await open(false);
    evidence.controls.push(await proveGenuineRotation(actor, server, "disabled"));
    const disabled = await captureBrowserArtifactState(actor);
    assert.equal(disabled.qaTraceCapture.status, "disabled");
    assertCompleteQaTraceCaptures([disabled.qaTraceCapture]);
    evidence.disabledIdentityResources = await actor.evaluate(`performance.getEntriesByType("resource")
      .filter(entry => new URL(entry.name).pathname.includes("zubspace_wasm_identity"))
      .map(entry => new URL(entry.name).pathname)`);
    assert.deepEqual(evidence.disabledIdentityResources, [], "ordinary play must not load identity code/workers");
    evidence.disabled = disabled.qaTraceCapture;
    assert.equal((await actor.evaluate("window.__zubspaceFirstStallCapture()")).phase, "disabled");
    await disconnect();
    evidence.automaticUploads = [];
    for (const mode of ["clean", "dropped"]) {
      const offset = inputs(server).length;
      actor = await open(true, true);
      await waitUntil(`automatic ${mode}: joined and accepting genuine input`, async () =>
        inputs(server).length > offset && await actor.evaluate(
          "window.__zubspaceQaTraceStateForTest?.().ready === true && window.__zubspaceUiMetrics?.playerAlive === true"));
      evidence.controls.push(await proveGenuineRotation(actor, server, `automatic-${mode}-before`));
      const context = await actor.evaluate("window.__zubspaceQaTraceStateForTest()");
      assert.equal(context.firstStall.phase, "armed", "preserve unexpected earlier stalls");
      await actor.evaluate(`(${installTraceUploadFault.toString()})(${JSON.stringify(context.uploadEndpoint)}, ${JSON.stringify(mode)})`);
      const induced = await actor.evaluate(`(() => {
        const start = performance.now();
        while (performance.now() - start < 500) {}
        return { start, end: performance.now() };
      })()`);
      await waitUntil(`automatic ${mode}: upload settled`, async () => actor.evaluate(
        `["uploaded", "failed"].includes(window.__zubspaceQaTraceStateForTest().firstStall.upload)`));
      const saved = await actor.evaluate("window.__zubspaceFirstStallCapture()");
      const fault = await actor.evaluate("window.__qaDrillUploadFault");
      await actor.evaluate("(window.__qaDrillRestoreFetch(), true)");
      assert.equal(saved.phase, "captured");
      assert.equal(saved.upload, mode === "clean" ? "uploaded" : "failed");
      assert.equal(fault.calls, 1);
      assert.equal(saved.bundle.bundleId, context.bundleId);
      assert.ok(saved.captureCostMs < 80);
      const incident = saved.bundle.marker.incident;
      assert.ok(incident.previous.at <= induced.start && incident.current.at >= induced.end);
      assert.ok(incident.durationMs >= 500);
      assertQaServerBinaryProvenance(saved.bundle, provenance);
      assertQaWasmProvenance(saved.bundle, provenance);
      const response = await fetch(`http://127.0.0.1:${serverPort}/v1/qa/trace/${context.bundleId}`);
      assert.equal(response.status, mode === "clean" ? 200 : 404);
      let joined = null;
      if (response.ok) {
        joined = await response.json();
        assertJoinedTrace(joined, context.bundleId, provenance);
        assert.deepEqual(joined.browser, saved.bundle);
      }
      evidence.controls.push(await proveGenuineRotation(actor, server, `automatic-${mode}-after`));
      const retained = await saveBrowserE2eArtifacts({
        artifactDir: `${artifactDir}-automatic-${mode}`, server, staticServer,
        urls: [`${url}&trace=1&qa_upload=1`], clients: [{ label: "actor", client: actor }],
        extraLogs: { "first-stall": JSON.stringify(saved) }, provenance,
      });
      assertCompleteQaTraceCaptures(retained.qaTraceCaptureAudit);
      // Each session owns its own relative manifest. Link its complete result
      // with an exact external hash instead of nesting colliding filenames.
      const childResultPath = path.resolve(`${artifactDir}-automatic-${mode}`, "result.json");
      const childResult = { ok: true, browser, transport: "rtc", mode, induced,
        saved, fault, joined, provenance, successArtifacts: retained };
      fs.writeFileSync(childResultPath, `${JSON.stringify(childResult, null, 2)}\n`);
      validateArtifactPathReferences(childResult, childResultPath);
      evidence.automaticUploads.push({ mode, induced, saved, fault, joined,
        resultPath: childResultPath });
      await disconnect();
    }
    const newSessionInputOffset = inputs(server).length;
    actor = await open(true);
    await waitUntil("trace context and gameplay ready", async () =>
      actor.evaluate("window.__zubspaceQaTraceStateForTest?.().ready === true && window.__zubspaceUiMetrics?.playerAlive === true"));
    await waitUntil("fresh trace-enabled session accepted input", async () =>
      inputs(server).length > newSessionInputOffset);
    evidence.controls.push(await proveGenuineRotation(actor, server, "enabled"));
    // Pause only the browser main thread; gameplay still uses genuine keys
    // and the ordinary RTC connection. This diagnoses capture, not lag recovery
    // acceptance or the cause of an unprompted manual freeze.
    const beforeStall = await actor.evaluate("window.__zubspaceQaTraceStateForTest().firstStall");
    assert.equal(beforeStall.phase, "armed", "an earlier natural stall must be investigated, not overwritten");
    const induced = await actor.evaluate(`(() => {
      const start = performance.now();
      while (performance.now() - start < 500) {}
      return { start, end: performance.now() };
    })()`);
    await waitUntil("first stall captured automatically", async () =>
      actor.evaluate("window.__zubspaceQaTraceStateForTest().firstStall.phase === 'captured'"));
    const firstStall = await actor.evaluate("window.__zubspaceFirstStallCapture()");
    const incident = firstStall.bundle.marker.incident;
    assert.ok(incident.durationMs >= 500);
    assert.ok(incident.previous.at <= induced.start && incident.current.at >= induced.end);
    assert.equal(firstStall.upload, "not-requested");
    assert.ok(firstStall.captureCostMs < 80, `capture itself caused a frame-scale pause: ${firstStall.captureCostMs} ms`);
    assert.ok(Buffer.byteLength(JSON.stringify(firstStall.bundle)) <= 240 * 1024);
    assert.ok(firstStall.bundle.rings.inputs.length > 0, "pre-stall genuine input evidence retained");
    assertQaServerBinaryProvenance(firstStall.bundle, provenance);
    assertQaWasmProvenance(firstStall.bundle, provenance);
    evidence.firstStall = { induced, ...firstStall };
    evidence.controls.push(await proveGenuineRotation(actor, server, "after-automatic-stall-capture"));
    // Later incidents must remain discoverable even though the original full
    // first-stall bundle is immutable. Run these inside actual animation
    // callbacks so the independent callback-duration witness must see them.
    const repeatedPauses = [];
    evidence.repeatedPauses = repeatedPauses;
    for (const duration of [350, 700]) {
      await actor.evaluate(`(() => {
        window.__qaDrillPause = null;
        requestAnimationFrame(() => {
        const start = performance.now();
        while (performance.now() - start < ${duration}) {}
        window.__qaDrillPause = { start, end: performance.now() };
        });
        return true;
      })()`);
      await waitUntil("induced animation callback completed", async () => actor.evaluate(
        "Number.isFinite(window.__qaDrillPause?.end)"));
      const pause = await actor.evaluate("window.__qaDrillPause");
      await waitUntil("later incident retains post-stall context", async () => actor.evaluate(
        `window.__zubspaceFrameDiagnostics().incidents.some(i => i.at >= ${pause.end} && i.afterComplete)`));
      // The heartbeat witness is a bounded ~6-second ring of independent stamps, so read it now,
      // while it still spans this gap. The incident itself is retained in every later snapshot;
      // only the live ring ages out, which is exactly what `covered: false` reports.
      const ringWhileCovered = await actor.evaluate("window.__zubspaceFrameDiagnostics()");
      repeatedPauses.push({ ...pause, ringWhileCovered });
      evidence.controls.push(await proveGenuineRotation(actor, server, `after-${duration}ms-stall`));
    }
    // Wait for actual live-ring eviction, not an arbitrary time assumption.
    await waitUntil("live history advances past preserved incident", async () => actor.evaluate(
      `(window.__zubspaceMetricsTrace?.[0]?.at || 0) > ${incident.current.wallAt}`), 40_000);
    assert.deepEqual(await actor.evaluate("window.__zubspaceFirstStallCapture()"), firstStall,
      "live history eviction or later updates rewrote the preserved incident");
    const repeated = await actor.evaluate("window.__zubspaceFrameDiagnostics()");
    for (const pause of repeatedPauses) {
      const matching = repeated.incidents.find(i => i.at >= pause.end && i.at - i.durationMs <= pause.start);
      assert.ok(matching, "later induced stall missing after live history eviction");
      assert.ok(matching.before.length > 0 && matching.afterComplete);
      assert.ok(matching.work.some(w => w[0] === "animation-callback" && w[1] <= pause.start &&
        w[1] + w[2] >= pause.end), "animation callback timing did not attribute the induced pause");
      // The independent witness must keep stamping through a main-thread-only stall — that
      // is exactly what lets a future natural pause be told apart from a whole-process freeze,
      // where both threads gap together. Assert the other thread's stamps span the induced gap.
      const covered = pause.ringWhileCovered.incidents.find(
        i => i.at >= pause.end && i.at - i.durationMs <= pause.start);
      assert.ok(covered, "induced stall missing from the snapshot taken while its ring still covered it");
      const heartbeat = covered.heartbeat;
      const pauseMs = pause.end - pause.start;
      assert.ok(heartbeat?.covered === true,
        `worker-heartbeat ring did not span the induced ${Math.round(pauseMs)}ms stall: ` +
        `${JSON.stringify(heartbeat)}`);
      assert.ok(heartbeat.ticks >= 3 && heartbeat.spreadMs >= 0.5 * pauseMs,
        `worker stamps stopped advancing across a blocked main thread: ` +
        `${JSON.stringify({ heartbeat, pauseMs, gapMs: matching.durationMs })}`);
    }
    evidence.repeatedStalls = { induced: repeatedPauses, diagnostics: repeated };
    const traceState = await actor.evaluate("window.__zubspaceQaTraceStateForTest()");
    evidence.secureContext = await actor.evaluate("window.isSecureContext");
    if (httpHost !== "127.0.0.1") assert.equal(evidence.secureContext, false);
    const retrieve = async () => {
      const response = await fetch(`http://127.0.0.1:${serverPort}/v1/qa/trace/${traceState.bundleId}`);
      assert.equal(response.status, 200);
      return response.json();
    };
    const upload = async (mode, expectedStatus) => {
      await actor.evaluate(`(${installTraceUploadFault.toString()})(${JSON.stringify(traceState.uploadEndpoint)}, ${JSON.stringify(mode)})`);
      let joined = null;
      let rejected = false;
      let errorMessage = null;
      try { joined = await actor.freezeQaTrace({ upload: true }); }
      catch (error) { rejected = true; errorMessage = String(error.message).slice(0, 512); }
      finally { await actor.evaluate("(window.__qaDrillRestoreFetch(), true)"); }
      const fault = await actor.evaluate("window.__qaDrillUploadFault");
      evidence.uploads.push({ ...fault, rejected, errorMessage });
      assert.equal(fault.calls, 1);
      assert.equal(fault.status, expectedStatus, JSON.stringify(evidence.uploads.at(-1)));
      assert.equal(rejected, expectedStatus !== 200);
      if (joined) {
        assertJoinedTrace(joined, traceState.bundleId, provenance);
        assert.deepEqual(await retrieve(), joined);
      }
      return joined;
    };
    evidence.clean = await upload("clean", 200);
    assert.deepEqual(evidence.clean.browser.marker.frameDiagnostics.incidents, repeated.incidents,
      "manual mark lost earlier incidents under the export byte budget");
    for (const [mode, status] of [["dropped", null], ["malformed", 400]]) {
      await upload(mode, status);
      assert.deepEqual(await retrieve(), evidence.clean, "failed upload must not replace retained evidence");
      evidence.controls.push(await proveGenuineRotation(actor, server, mode));
    }
    // Deliberately fail an oracle, not gameplay. Exercise the same cold
    // failure-artifact writer used by combat tests before recovering normally.
    try { assert.fail("qa_drill_deliberate_invariant_failure"); }
    catch (error) {
      const retained = await saveBrowserE2eArtifacts({
        artifactDir: `${artifactDir}-expected-failure`, server, staticServer,
        urls: [`${url}&trace=1`], clients: [{ label: "actor", client: actor }],
        extraLogs: { invariant: error.message }, provenance,
      });
      assertCompleteQaTraceCaptures(retained.qaTraceCaptureAudit);
      const resultPath = path.join(`${artifactDir}-expected-failure`, "result.json");
      const failureResult = { ok: false, expectedInvariantFailure: true,
        failure: error.message, provenance, successArtifacts: retained };
      fs.writeFileSync(resultPath, `${JSON.stringify(failureResult, null, 2)}\n`);
      validateArtifactPathReferences(failureResult, resultPath);
      evidence.forcedFailure = { message: error.message, captureAudit: retained.qaTraceCaptureAudit };
    }
    evidence.forcedFailure.joined = await upload("forced-invariant", 200);
    evidence.controls.push(await proveGenuineRotation(actor, server, "after-invariant-capture"));
    await upload("quota-third", 200);
    const lastRetained = await upload("quota-fourth", 200);
    await upload("rate-limited", 429);
    assert.deepEqual(await retrieve(), lastRetained);
    evidence.controls.push(await proveGenuineRotation(actor, server, "rate-limited"));
    const state = await actor.pageState();
    const channels = state.networkLog.filter((entry) => entry.message === "rtc data channel open")
      .map((entry) => entry.detail?.label).sort();
    assert.deepEqual(channels, ["zubspace-events", "zubspace-game", "zubspace-reliable"]);
    const current = await collectArtifactProvenanceAsync({ runStartedAt, e2eScript });
    assertArtifactProvenanceMatches(provenance, current);
    const successArtifacts = await saveBrowserE2eArtifacts({ artifactDir, server, staticServer,
      urls: [`${url}&trace=1`], clients: [{ label: "actor", client: actor }],
      extraLogs: { "trace-drill": JSON.stringify(evidence), browser: browserProcess.logs }, provenance: current });
    assertCompleteQaTraceCaptures(successArtifacts.qaTraceCaptureAudit);
    // The drill's disconnect closes the session and waits for full teardown,
    // so it must finish while the artifact writer can still read the page.
    // The delivery audit then verifies the queues drained post-disconnect.
    let cleanup = null;
    const deliveryAudit = await assertDataChannelDeliveryAudit({ server, serverPort, browserLogs: [browserProcess.logs],
      disconnect: async () => { cleanup = await disconnect(); } });
    const securityAudit = assertSecurityRejectAudit(server);
    const childResults = evidence.automaticUploads.map(entry => entry.resultPath);
    const retainedExternalPaths = childResults.map(resultPath => {
      const file = artifactFileManifest(path.dirname(resultPath), { exclude: [] })
        .find(entry => entry.path === "result.json");
      assert.ok(file, "automatic session result is retained");
      return { ...file, path: resultPath };
    });
    const result = { ok: true, browser, arena: "chaos-zone", transport: "rtc", channels,
      extraLogPaths: childResults, retainedExternalPaths,
      controls: evidence.controls, uploads: evidence.uploads, disabled: evidence.disabled,
      disabledIdentityResources: evidence.disabledIdentityResources,
      secureContext: evidence.secureContext,
      firstStall: evidence.firstStall,
      repeatedStalls: evidence.repeatedStalls,
      automaticUploads: evidence.automaticUploads,
      clean: assertJoinedTrace(evidence.clean, traceState.bundleId, current),
      forcedFailure: { message: evidence.forcedFailure.message,
        ...assertJoinedTrace(evidence.forcedFailure.joined, traceState.bundleId, current) },
      deliveryAudit, securityAudit, cleanup, provenance: current, successArtifacts };
    const resultPath = path.join(artifactDir, "result.json");
    // Before it is written: "the run passed" and "every required capture was taken" are different statements, and
    // a missing case must fail here rather than shrink the artifact quietly.
    assertQaTraceCoverage(result);
    result.qaTraceChildResults = validateQaTraceChildResults(result, resultPath);
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
    validateArtifactPathReferences(result, resultPath);
    console.log(JSON.stringify({ ok: true, browser, resultPath, uploads: result.uploads }));
  } catch (error) {
    evidence.firstStallAtFailure = await actor?.evaluate(
      "window.__zubspaceFirstStallCapture?.() || null").catch(() => null);
    await actor?.evaluate("(window.__qaDrillRestoreFetch?.(), true)").catch(() => {});
    const retained = await saveBrowserE2eArtifacts({ artifactDir: `${artifactDir}-failure`, server, staticServer,
      urls: [url], clients: [{ label: "actor", client: actor }],
      extraLogs: { failure: error.stack, "trace-drill": JSON.stringify(evidence), browser: browserProcess.logs }, provenance });
    const resultPath = path.join(`${artifactDir}-failure`, "result.json");
    const failureResult = { ok: false, failure: error.message, provenance, successArtifacts: retained };
    fs.writeFileSync(resultPath, `${JSON.stringify(failureResult, null, 2)}\n`);
    validateArtifactPathReferences(failureResult, resultPath);
    throw new Error(`${error.message}; retained at ${artifactDir}-failure`, { cause: error });
  } finally {
    await actor?.close().catch(() => {});
    await stopProcess(browserProcess.child).catch(() => {});
    await stopProcess(server.child).catch(() => {});
    staticServer.server.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => { console.error(error); process.exitCode = 1; });
}
