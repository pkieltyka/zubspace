#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
import fs from "node:fs";
import { createDesktopXDisplay } from "../../tools/e2e_desktop_display.mjs";
import { spawn } from "node:child_process";
import { freePort, sleep, startRealServer, startStaticServer, stopProcess,
  waitForServerListening, serverBinaryForProfile } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { startChrome, waitForChrome, openChromeClient, waitUntil,
  assertDataChannelDeliveryAudit, assertSecurityRejectAudit,
  saveBrowserE2eArtifacts } from "./combat_replay.mjs";
import { collectArtifactProvenance, assertArtifactProvenanceMatches } from "./artifact_provenance.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { movingPeerWindowEvidence, createAdmittedRecordReader } from "./moving_peer_evidence.mjs";
import { openCdpSession } from "../../tools/webrtc_capacity_runner.mjs";
import { lifecycleIsReady } from "./lifecycle_readiness.mjs";
import { loadExpression } from "../../tools/webrtc_capacity_load.mjs";
import { measureCaptureResponsiveness } from "./capture_responsiveness.mjs";
import { longTaskWitnessInstallScript, longTaskWitnessReadScript, frameGapDelta,
  attributeGapCovers, STARVATION_FLOOR_MS } from "./long_task_witness.mjs";

async function isolatedDisplay() {
  const child = spawn(process.env.ZUBSPACE_XVFB_BIN || "Xvfb",
    ["-displayfd", "3", "-screen", "0", "1024x768x24", "-nolisten", "tcp"],
    { stdio: ["ignore", "pipe", "pipe", "pipe"] });
  const logs = [];
  child.stderr.on("data", (chunk) => logs.push(String(chunk)));
  try {
    const display = await new Promise((resolve, reject) => {
      let response = "";
      const timer = setTimeout(() => reject(new Error("Xvfb display allocation timed out")), 10000);
      const fail = (error) => { clearTimeout(timer); reject(error); };
      child.once("error", fail);
      child.once("exit", (code) => fail(new Error(`Xvfb exited: ${code}`)));
      child.stdio[3].on("data", (chunk) => {
        response += chunk;
        if (/^\d+\n$/.test(response)) { clearTimeout(timer); resolve(`:${response.trim()}`); }
      });
    });
    return { child, display, logs };
  } catch (error) { await stopProcess(child); throw error; }
}

async function main() {
  if (process.env.ZUBSPACE_E2E_HEADLESS !== "0") throw new Error("headed Chrome is required");
  const count = Number(process.env.ZUBSPACE_E2E_MOVING_PEERS || 8);
  if (!Number.isSafeInteger(count) || count < 2 || count > 200) throw new Error("moving peers must be 2..200");
  const backgroundCount = Number(process.env.ZUBSPACE_E2E_BACKGROUND_RTC_PEERS || 0);
  if (!Number.isSafeInteger(backgroundCount) || backgroundCount < 0 || count + backgroundCount > 200) {
    throw new Error("moving plus background RTC population must not exceed 200");
  }
  const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
  const serverProfile = process.env.ZUBSPACE_E2E_SERVER_PROFILE || "debug";
  const serverBinary = serverBinaryForProfile(serverProfile);
  const serverCpuSet = process.env.ZUBSPACE_E2E_MOVING_SERVER_CPUSET || null;
  const browserCpuSet = process.env.ZUBSPACE_E2E_MOVING_BROWSER_CPUSET || null;
  const graphicsBackend = process.env.ZUBSPACE_E2E_MOVING_GRAPHICS_BACKEND || "swiftshader";
  if (!["swiftshader", "vulkan", "native"].includes(graphicsBackend)) throw new Error("unsupported moving graphics backend");
  const graphicsEvidence = {requested: graphicsBackend, peers: {}};
  const observedCpuSets = {server: null, browsers: {}};
  const processCpuSet = (pid) => {
    const value = fs.readFileSync(`/proc/${pid}/status`, "utf8")
      .match(/^Cpus_allowed_list:\s*(.+)$/m)?.[1];
    if (!value) throw new Error(`missing CPU affinity evidence for process ${pid}`);
    return value.trim();
  };
  process.env.ZUBSPACE_SERVER_SIM_OBSERVE = "1";
  const options = { e2eScript: "tests/e2e/moving_peer_load.mjs", runStartedAt: new Date().toISOString(),
    serverBinary, additionalFiles: ["tests/e2e/capacity_host.html"] };
  const initial = collectArtifactProvenance(options);
  const artifactDir = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
    `/tmp/zubspace-moving-peers-${Date.now()}-${process.pid}`;
  const port = await freePort(), staticServer = await startStaticServer();
  const server = startRealServer(port, { zone: arena, botCount: 0, profile: serverProfile, cpuSet: serverCpuSet });
  const reader = createAdmittedRecordReader();
  const admittedRecords = () => reader.read(server.logs);
  const peers = [];
  let desktopDisplay = null;
  const movingDisplay = () => desktopDisplay
    ? { child: null, display: desktopDisplay.display, logs: [] }
    : isolatedDisplay();
  let background = null;
  const backgroundEvidence = { count: backgroundCount, kind: "protocol-only stationary background; not genuine-key pilots" };
  const executionEvidence = {before: null, after: null, failure: null};
  const backgroundEvaluate = async (expression) => {
    const result = await background.cdp.send("Runtime.evaluate", {expression, awaitPromise: true, returnByValue: true});
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  const status = async () => (await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`)).json();
  let evidence = null;
  let windowTraces = null;
  let profiler = null, cpuProfile = null, compositorTrace = null, profileClocks = null;
  const captureDiagnostic = process.env.ZUBSPACE_E2E_MOVING_CAPTURE_DIAGNOSTIC === "1";
  let captureResponsiveness = null;
  const compositorProfiling = process.env.ZUBSPACE_E2E_MOVING_COMPOSITOR_TRACE === "1";
  const profiling = compositorProfiling || process.env.ZUBSPACE_E2E_MOVING_PROFILE_LAST === "1";
  // Which client build the browsers actually loaded is part of the measurement, and an
  // overridden bundle is never the repo-default moving-peer acceptance run: `dist/` is the
  // optimized release client and the default lane serves the debug WASM, whose per-frame cost
  // is several times higher. Recording the root keeps a build-fidelity comparison readable and
  // stops it being mistaken for the acceptance gate.
  const staticBundleOverride = process.env.ZUBSPACE_E2E_STATIC_ROOT?.trim() || null;
  // Attribution profile (`ZUBSPACE_E2E_MOVING_ATTRIBUTION=1`): put the shared frame/long-task/
  // starvation witness in every peer page and read it back around the measured window, so a peer
  // whose server tick gap trips this gate can be told apart from a peer whose main thread was simply
  // busy, and from a peer that was installing a zone archive at that moment. The probe occupies the
  // thread it measures, so a run with it on is a diagnostic and never an acceptance measurement.
  const attribution = process.env.ZUBSPACE_E2E_MOVING_ATTRIBUTION === "1";
  const attributionEvidence = {};
  const readPeerMerge = async (peer) => {
    // The counters live in the client's own metrics payload, which the peer helper already reads;
    // there is no `window.localGameplayState` global to evaluate against.
    const local = (await peer.client.metricsState())?.uiMetrics?.localGameplayState || {};
    return {
      installCount: local.lvzMergeCount ?? null,
      mergeTotalMs: local.lvzMergeTotalMs ?? null,
      mergeMaxMs: local.lvzMergeMaxMs ?? null,
      objects: local.zoneLvzObjects ?? null,
      textures: local.zoneLvzTextures ?? null,
      zoneFilesReady: local.zoneFilesReady ?? null,
    };
  };
  const witnessSummary = (read) => read && {
    nowMs: read.now, startedAtMs: read.startedAt, frames: read.frames,
    maximumGapMs: read.maximumGapMs, gapCount: read.gapCount, gapFloorMs: read.gapFloorMs,
    intervalEvictions: read.intervalEvictions, starvationTicks: read.starvationTicks,
    starvationMaximumMs: read.starvationMaximumMs, starvationSupport: read.starvationSupport,
    longTaskSupport: read.longTaskSupport, frameSampler: read.frameSampler,
    // When each producer was attached: a sampler attached after page load cannot speak for the
    // interval before it, and this is the only field that says so.
    frameSamplerStartedAtMs: read.frameSamplerStartedAt ?? null,
    starvationProbeStartedAtMs: read.starvationProbeStartedAt ?? null,
  };
  // A windowed read resets the page's accumulators at the window edge. Sums and bucket counts
  // difference across a window, maxima do not: with page-life accumulators the window maxima are
  // structurally unavailable and a reader sees `maxDurationMsInWindow: null` next to a 96 ms
  // starvation, which reads like an instrument gap rather than a limit of differencing. Only the
  // attribution lane resets, because a page-life observer still wants what it clears.
  const readPeerCadence = async (peer, { reset = false } = {}) => {
    if (reset) {
      await peer.client.evaluate('(() => (typeof window.__zubspaceQaResetFrameDiagnostics === "function"'
        + ' ? window.__zubspaceQaResetFrameDiagnostics() : false))()');
    }
    // The client's own per-callback accounting. Its incident ring only keeps work at or above
    // 50ms, so a 40ms median frame - which is what eight concurrent peers measure - leaves it
    // empty; the cadence accumulators are the only in-page witness for that case, and they are
    // what separates "the browser started the callback late" from "the callback ran long".
    // Project inside the page: the full snapshot carries the recent-sample, incident and work
    // rings, and serializing all of that through Runtime.evaluate comes back as nothing at all,
    // which a reader would otherwise file as "no cadence happened".
    const value = await peer.client.evaluate(`(() => {
      const diagnostics = window.__zubspaceFrameDiagnostics;
      if (typeof diagnostics !== "function") { return null; }
      const snapshot = diagnostics();
      if (!snapshot || !snapshot.cadence) { return null; }
      return { publishing: snapshot.cadence.publishing, other: snapshot.cadence.other,
        bucketBoundsMs: snapshot.cadence.bucketBoundsMs, incidents: snapshot.total ?? 0,
        // The two projections a gap can be joined against. bySource names which callback function
        // ran, and work plus slowTasks carry spans, so a gap either has a cover or provably does not;
        // without them a stall can only be timed, never attributed. (No backticks in this comment:
        // it lives inside a template literal that the page evaluates.)
        bySource: snapshot.cadence.bySource ?? null, tasks: snapshot.tasks ?? null,
        taskFloorMs: snapshot.taskFloorMs ?? null,
        slowTasks: (snapshot.slowTasks ?? []).slice(-24), work: (snapshot.work ?? []).slice(-16),
        workFields: snapshot.workFields ?? null,
        enabled: snapshot.enabled === true };
    })()`);
    return value && value.enabled ? {
      publishing: value.publishing, other: value.other,
      bucketBoundsMs: value.bucketBoundsMs, incidents: value.incidents,
      // Carried through the outer projection as well. Building the returned object from scratch is
      // where a witness silently becomes unavailable: the inner page projection gathered bySource,
      // tasks and the span rings, and a run then reported `windowTasks: null` for every peer, which a
      // reader would file as "no tasks ran" rather than "the lane dropped the fields it read".
      bySource: value.bySource, tasks: value.tasks, taskFloorMs: value.taskFloorMs,
      slowTasks: value.slowTasks, work: value.work, workFields: value.workFields,
    } : null;
  };
  const cadenceDelta = (before, after) => {
    if (!before || !after) {
      return {
        available: false,
        reason: "no cadence snapshot was readable on this peer (trace=1 installs the accounting)",
      };
    }
    const round = (value) => Math.round(value * 100) / 100;
    const group = (key) => {
      const b = before[key] || {}, a = after[key] || {};
      const callbacks = (a.callbacks || 0) - (b.callbacks || 0);
      const durationSum = (a.durationSumMs || 0) - (b.durationSumMs || 0);
      const latenessSum = (a.latenessSumMs || 0) - (b.latenessSumMs || 0);
      return {
        callbacks,
        durationMeanMs: callbacks ? round(durationSum / callbacks) : null,
        durationSumMs: round(durationSum),
        latenessMeanMs: callbacks ? round(latenessSum / callbacks) : null,
        latenessSumMs: round(latenessSum),
        // A max only speaks for the window when the window set it; otherwise the page-life max
        // came from before the window and the window's own max is merely no larger.
        maxDurationMsInWindow: (a.durationMaxMs || 0) > (b.durationMaxMs || 0) ? a.durationMaxMs : null,
        maxLatenessMsInWindow: (a.latenessMaxMs || 0) > (b.latenessMaxMs || 0) ? a.latenessMaxMs : null,
        durationBucketsInWindow: (a.durationBuckets || []).map((count, index) =>
          count - ((b.durationBuckets || [])[index] || 0)),
        latenessBucketsInWindow: (a.latenessBuckets || []).map((count, index) =>
          count - ((b.latenessBuckets || [])[index] || 0)),
      };
    };
    return {
      available: true,
      bucketBoundsMs: after.bucketBoundsMs,
      publishing: group("publishing"),
      other: group("other"),
      incidentsObservedInWindow: (after.incidents ?? 0) - (before.incidents ?? 0),
    };
  };
  const readPeerAttribution = async (peer, phase) => {
    const witness = JSON.parse(await peer.client.evaluate(longTaskWitnessReadScript({ longTaskLimit: 4 })));
    const merge = await readPeerMerge(peer);
    const cadence = await readPeerCadence(peer, { reset: phase === "window-start" });
    if (phase === "window-start") return { witness, merge, cadence };
    const delta = frameGapDelta(peer.attributionBefore.witness, witness);
    const intervalsInWindow = (witness.intervals || []).filter((entry) =>
      entry.at > peer.attributionBefore.witness.now && entry.at <= witness.now).length;
    return {
      windowFromMs: peer.attributionBefore.witness.now,
      windowToMs: witness.now,
      frameGapCount: delta.gapCount,
      largestFrameGapMs: delta.largestGapMs ?? null,
      frameCost: delta.frameCost,
      // The witness reports its own bookkeeping at both ends: if `frames` did not advance or
      // `startedAt` sits inside the window, the instrument - not the page - is what to believe.
      witnessAtWindowStart: witnessSummary(peer.attributionBefore.witness),
      witnessAtWindowEnd: witnessSummary(witness),
      frameSamplesInWindow: intervalsInWindow,
      starvationWindowMaximumMs: delta.starvation?.windowMaximumMs ?? null,
      starvationWindowBusySamples: delta.starvation?.windowBusySamples ?? null,
      starvationWindowTimerWaitMs: delta.starvation?.windowTimerWaitMaximumMs ?? null,
      starvationWindowUnknownSamples: delta.starvation?.windowUnknownSamples ?? null,
      // The raw entries travel with the counts. A maximum alone is not re-derivable: this milestone
      // turned on whether a wait's `framesAdvanced` was 1 or 2, and an artifact that kept only the
      // maximum could not be re-read after the rule changed.
      starvationWindowEntries: (delta.starvation?.entries ?? []).slice(0, 4),
      starvationPageWideMaximumMs: delta.starvation?.pageWideMaximumMs ?? null,
      gapsAttributed: attributeGapCovers((delta.gapsAttributed || []).map((gap) => ({
        gapMs: gap.gapMs, at: gap.at, attribution: gap.attribution,
        probeStarvedMs: gap.starvation?.maximumMs ?? null, longTaskMs: gap.longTask?.duration ?? null,
      })), cadence),
      // With the window-edge reset installed, every aggregate below describes this window, not the
      // page. Saying so keeps a reader from comparing it to a page-life number elsewhere.
      cadenceScope: "window",
      windowWorstCallbackMs: cadence && cadence.bySource
        ? Object.fromEntries(Object.entries(cadence.bySource)
            .map(([key, value]) => [key, Math.round(value.durationMaxMs ?? 0)]))
        : null,
      windowTasks: cadence && cadence.tasks
        ? Object.fromEntries(Object.entries(cadence.tasks)
            .map(([key, value]) => [key, { count: value.count, sumMs: Math.round(value.sumMs ?? 0),
              maxMs: Math.round(value.maxMs ?? 0) }]))
        : null,
      merge: { ...merge,
        installGrowthInWindow: merge.installCount - peer.attributionBefore.merge.installCount,
        mergeMsGrowthInWindow: Math.round(((merge.mergeTotalMs ?? 0) - (peer.attributionBefore.merge.mergeTotalMs ?? 0)) * 10) / 10 },
      mergeAtWindowStart: peer.attributionBefore.merge,
      cadenceInWindow: cadenceDelta(peer.attributionBefore.cadence, cadence),
      cadencePageLife: cadence ? { publishing: cadence.publishing, other: cadence.other } : null,
      longTasks: witness.longTasks.slice(-4),
      starvationFloorMs: STARVATION_FLOOR_MS,
      phase,
    };
  };
  const collectPeerAttribution = async () => {
    for (const peer of peers) {
      if (!peer.client) continue;
      if (!peer.attributionBefore) {
        // A missing before-read makes window attribution impossible; say so in the evidence instead
        // of dropping the peer, so an empty map can never be read as "no stalls happened".
        attributionEvidence[peer.label] = { unavailable: "window-start read failed" };
        continue;
      }
      try {
        attributionEvidence[peer.label] = await readPeerAttribution(peer, "window-end");
      } catch (readError) {
        attributionEvidence[peer.label] = { readError: String(readError) };
      }
    }
    return attributionEvidence;
  };
  // Rig choice alone never demotes an unprofiled run. The native desktop
  // lane additionally verifies hardware compositing; the historical Vulkan
  // variant records its Xvfb presentation path. Measurement modifiers stay
  // diagnostic-only and never count as acceptance.
  const diagnosticOnly = captureDiagnostic || profiling || attribution ||
    serverCpuSet != null || browserCpuSet != null ||
    process.env.ZUBSPACE_SERVER_POSITION_TIMING === "1" || staticBundleOverride != null;
  const graphicsRig = () => ({
    requested: graphicsBackend,
    display: desktopDisplay?.evidence || { kind: "isolated-xvfb", serverProcessOwned: true },
    softwareRaster: graphicsBackend === "swiftshader",
    // Preserve the software-raster ceiling distinction. Native acceptance
    // additionally verifies compositing per peer; renderer identity alone was
    // insufficient for the old Vulkan readback rig.
    gapCeilingIsRigBound: graphicsBackend === "swiftshader",
    // The renderer string is the witness that the pages really got the backend the run claims; the
    // requested name alone is only an intention.
    renderers: [...new Set(Object.values(graphicsEvidence.peers)
      .map((peer) => peer && peer.renderer).filter(Boolean))],
  });
  let tracingComplete = null, completeTracing = null;
  const finishProfile = async () => {
    if (!profiler) return;
    try {
      cpuProfile = (await profiler.send("Profiler.stop")).profile;
      if (tracingComplete) {
        await profiler.send("Tracing.end");
        let timer;
        const terminal = await Promise.race([tracingComplete,
          new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("compositor trace completion timed out")), 10000); })
        ]).finally(() => clearTimeout(timer));
        const parts = [];
        let bytes = 0;
        try {
          for (;;) {
            const chunk = await profiler.send("IO.read", { handle: terminal.stream, size: 65536 });
            const data = chunk.base64Encoded ? Buffer.from(chunk.data, "base64").toString("utf8") : chunk.data;
            bytes += Buffer.byteLength(data);
            if (bytes > 64 * 1024 * 1024) throw new Error("compositor trace exceeds 64 MiB bound");
            parts.push(data);
            if (chunk.eof) break;
          }
          compositorTrace = parts.join("");
          if (terminal.dataLossOccurred) throw new Error("compositor trace reports data loss");
        } finally { await profiler.send("IO.close", { handle: terminal.stream }); }
      }
    }
    finally { profiler.close(); profiler = null; }
  };
  const profileLogs = () => ({
    "ingress-execution": JSON.stringify(executionEvidence),
    "graphics-backend": JSON.stringify(graphicsEvidence),
    ...(captureResponsiveness ? {"capture-responsiveness": JSON.stringify(captureResponsiveness)} : {}),
    ...(cpuProfile ? { "last-peer-cpu-profile": JSON.stringify(cpuProfile) } : {}),
    ...(compositorTrace ? { "last-peer-compositor-trace": compositorTrace } : {}),
    ...(profileClocks ? { "profile-clocks": JSON.stringify(profileClocks) } : {}),
  });
  const captureWindowTraces = async () => {
    if (windowTraces) return;
    // Freeze all peers' bounded diagnostic tails before sequential artifact
    // hashing/screenshots can evict the measured send/render window.
    windowTraces = Object.fromEntries(await Promise.all(peers.filter((p) => p.client).map(async (p) => [
      `${p.label}-window`, JSON.stringify(await p.client.evaluate(`({
        at: Date.now(), packets: window.__zubspacePacketLog || [],
        frames: (window.__zubspaceMetricsTrace || []).map(m => ({
          at: m.at, monotonicMs: m.monotonicMs, renderOrdinal: m.renderOrdinal,
          sequence: m.clientInputSeq, life: m.playerLifeSeq, input: m.localInputBits
        }))
      })`)),
    ])));
  };
  try {
    await waitForServerListening(port, server.child, server.logs);
    if (serverCpuSet) observedCpuSets.server = processCpuSet(server.child.pid);
    if (graphicsBackend === "native") desktopDisplay = await createDesktopXDisplay();
    for (let i = 0; i < count; i++) {
      const display = await movingDisplay();
      const peer = { label: `moving-${i}`, display, chrome: null, client: null };
      peers.push(peer);
      peer.chrome = startChrome(await freePort(), peer.label, { display: display.display, cpuSet: browserCpuSet, graphicsBackend });
      await waitForChrome(peer.chrome);
      if (browserCpuSet) observedCpuSets.browsers[peer.label] = processCpuSet(peer.chrome.child.pid);
      peer.url = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(`http://127.0.0.1:${port}/v1/rtc/offer`)}&arena=${arena}&name=moving${i}&trace=1&packetlog=1&packetlog_bytes=128`;
      peer.client = await openChromeClient(peer.chrome, peer.url);
      await waitUntil(`${peer.label} owns live authority`, async () => {
        const state = await peer.client.pageState();
        const metrics = state.uiMetrics;
        if (!lifecycleIsReady(state)) return false;
        peer.playerId = metrics.selfPlayerId;
        return Number.isInteger(peer.playerId);
      });
      console.log(JSON.stringify({ stage: "joined", peer: i, playerId: peer.playerId }));
      graphicsEvidence.peers[peer.label] = await peer.client.evaluateAwait(`(() => {
        const canvas=document.querySelector('#glcanvas');
        const gl=canvas && (canvas.getContext('webgl') || canvas.getContext('webgl2'));
        if(!gl)throw new Error('game WebGL context unavailable');
        const ext=gl.getExtension('WEBGL_debug_renderer_info');
        if(!ext)throw new Error('game renderer identity unavailable');
        return {vendor:gl.getParameter(ext.UNMASKED_VENDOR_WEBGL),renderer:gl.getParameter(ext.UNMASKED_RENDERER_WEBGL)};
      })()`);
      if (graphicsBackend === "native") {
        const version = await (await fetch(`http://127.0.0.1:${peer.chrome.port}/json/version`)).json();
        const browserCdp = await openCdpSession(version.webSocketDebuggerUrl, { timeoutMs: 5_000 });
        try {
          const info = await browserCdp.send("SystemInfo.getInfo");
          const features = info.gpu?.featureStatus || {};
          Object.assign(graphicsEvidence.peers[peer.label], {
            browserVersion: version.Browser, featureStatus: features,
            compositor: { glImplementationParts: info.gpu?.auxAttributes?.glImplementationParts,
              skiaBackendType: info.gpu?.auxAttributes?.skiaBackendType },
          });
          if (features.gpu_compositing !== "enabled" || features.webgl !== "enabled") {
            throw new Error(`native GPU lane requires hardware WebGL and compositing: ${JSON.stringify(features)}`);
          }
        } finally { browserCdp.close(); }
      }
      if (attribution) {
        // Installed per peer right after readiness, so the ring covers the whole live window and the
        // before-read at window start only has to delta the movement interval itself.
                // Attribution runs lower the gap ring's floor to 40ms. The eight-peer contract turns on
        // single-digit tick gaps and the rig's own frame period is 29-43 ms, so an 80 ms floor keeps a
        // real 60-90 ms live-arena freeze out of the ring entirely: the window then reports no gap and
        // the freeze is invisible to attribution while the starvation probe still counts it. Only the
        // diagnostic lane lowers it, because a lower floor fills the ring faster.
        await peer.client.evaluate(longTaskWitnessInstallScript(
          attribution ? { longTask: true, starvationProbe: true, gapFloorMs: 40 }
            : { longTask: true, starvationProbe: true }));
      }
      if (graphicsBackend !== "swiftshader" && (!/AMD|NVIDIA|Intel/.test(graphicsEvidence.peers[peer.label].renderer) ||
          /swiftshader|llvmpipe|lavapipe|software/i.test(graphicsEvidence.peers[peer.label].renderer))) {
        throw new Error("hardware diagnostic did not obtain a hardware game renderer");
      }
    }
    if ((await status()).clients !== count) throw new Error("server population differs from requested workload");
    if (backgroundCount) {
      background = {display: await movingDisplay()};
      background.chrome = startChrome(await freePort(), "background-rtc", {display: background.display.display, cpuSet: browserCpuSet, graphicsBackend});
      await waitForChrome(background.chrome);
      if (browserCpuSet) observedCpuSets.browsers.background = processCpuSet(background.chrome.child.pid);
      const url = `http://127.0.0.1:${staticServer.port}/tests/e2e/capacity_host.html`;
      const target = await (await fetch(`http://127.0.0.1:${background.chrome.port}/json/new?${encodeURIComponent(url)}`, {method: "PUT"})).json();
      background.cdp = await openCdpSession(target.webSocketDebuggerUrl, {timeoutMs: 60000});
      await waitUntil("background capacity origin", () => backgroundEvaluate(`location.href === ${JSON.stringify(url)} && document.readyState === 'complete'`));
      backgroundEvidence.setup = await backgroundEvaluate(loadExpression(`http://127.0.0.1:${port}/v1/rtc/offer`, {
        count: backgroundCount, batchSize: 24, holdMs: 1000,
      }));
      if (backgroundEvidence.setup.connected !== backgroundCount) throw new Error("background population incomplete");
      await waitUntil("combined RTC population", async () => {
        const current = await status();
        return current.clients === count + backgroundCount && current.rtc_sessions === count + backgroundCount;
      });
      await waitUntil("every background peer receives initial FULL/delta stream", async () => {
        const receipts = await backgroundEvaluate("window.__zubspaceCapacityReceiptCounts()");
        backgroundEvidence.readiness = receipts;
        return receipts.openPeers === backgroundCount &&
          receipts.snapshots.length === backgroundCount && receipts.deltas.length === backgroundCount &&
          receipts.snapshots.every((n) => n > 0) && receipts.deltas.every((n) => n > 0);
      });
      backgroundEvidence.before = await backgroundEvaluate("window.__zubspaceCapacityReceiptCounts()");
    }
    if (profiling) {
      const peer = peers.at(-1);
      const targets = await (await fetch(`http://127.0.0.1:${peer.chrome.port}/json/list`)).json();
      const target = targets.find((t) => t.type === "page" && t.url === peer.url);
      if (!target?.webSocketDebuggerUrl) throw new Error("profile target unavailable");
      if (compositorProfiling) tracingComplete = new Promise((resolve) => { completeTracing = resolve; });
      profiler = await openCdpSession(target.webSocketDebuggerUrl, { timeoutMs: 30000,
        onEvent: (event) => { if (event.method === "Tracing.tracingComplete") completeTracing?.(event.params); } });
      if (compositorProfiling) await profiler.send("Tracing.start", {
        categories: "devtools.timeline,blink.user_timing,cc,viz,gpu,renderer.scheduler",
        options: "record-until-full", transferMode: "ReturnAsStream",
      });
      await profiler.send("Profiler.enable");
      await profiler.send("Profiler.setSamplingInterval", { interval: 1000 });
      profileClocks = { before: await peer.client.evaluate("({at:Date.now(),mono:performance.now(),origin:performance.timeOrigin})") };
      await profiler.send("Profiler.start");
      profileClocks.after = await peer.client.evaluate("(console.timeStamp('moving-load-start'),{at:Date.now(),mono:performance.now(),origin:performance.timeOrigin})");
    }
    const offset = admittedRecords().length;
    executionEvidence.before = (await status()).rtc_ingress_execution;
    const execution = executionEvidence.before;
    if (!execution || !Number.isSafeInteger(execution.limit) || execution.limit <= 0 ||
        !Number.isSafeInteger(execution.in_flight) || execution.in_flight < 0 ||
        !Number.isSafeInteger(execution.waiting) || execution.waiting < 0 ||
        execution.workers !== execution.limit || execution.in_flight > execution.limit) {
      throw new Error("bounded ingress worker status is missing or invalid");
    }
    if (attribution) {
      await Promise.all(peers.map(async (peer) => {
        peer.attributionBefore = await readPeerAttribution(peer, "window-start")
          .then((read) => { if (!read.witness) throw new Error("frame witness absent in the peer page"); return read; })
          .catch(() => null);
      }));
    }
    await Promise.all(peers.map((peer) => peer.client.keyDown("ArrowUp")));
    await waitUntil("all peers admit held thrust", async () => peers.every((peer) =>
      admittedRecords().slice(offset).some((r) => r.player_id === peer.playerId && (r.input_bits & 1))));
    const startTick = admittedRecords().at(-1).server_tick;
    const endTick = (startTick + 1000) % 0x80000000;
    backgroundEvidence.movementWindow = {startTick, endTick};
    console.log(JSON.stringify({stage: "movement-window-start", startTick, endTick,
      at: new Date().toISOString(), monotonicNs: process.hrtime.bigint().toString()}));
    await sleep(10200);
    console.log(JSON.stringify({stage: "movement-window-end", startTick, endTick,
      at: new Date().toISOString(), monotonicNs: process.hrtime.bigint().toString()}));
    if (attribution) await collectPeerAttribution();
    await finishProfile();
    executionEvidence.after = (await status()).rtc_ingress_execution;
    evidence = movingPeerWindowEvidence({ records: admittedRecords(),
      playerIds: peers.map((peer) => peer.playerId), startTick, endTick,
      maximumGapTicks: 10, minimumDistance: 10 });
    console.log(JSON.stringify({stage: "movement-measured", ok: evidence.ok,
      peers: evidence.peers.map(({playerId, maximumGap, failures}) =>
        ({playerId, maximumGap, failures}))}));
    await captureWindowTraces();
    if (captureDiagnostic) {
      const peer = peers[0];
      const targets = await (await fetch(`http://127.0.0.1:${peer.chrome.port}/json/list`)).json();
      const target = targets.find(t => t.type === "page" && t.url === peer.url);
      if (!target?.webSocketDebuggerUrl) throw new Error("capture diagnostic target unavailable");
      const fresh = await openCdpSession(target.webSocketDebuggerUrl, {timeoutMs: 5000});
      const expression = "({at:Date.now(),sequence:window.__zubspaceUiMetrics?.clientInputSeq})";
      try {
        captureResponsiveness = await measureCaptureResponsiveness({
          existing: () => peer.client.evaluate(expression),
          fresh: async () => {
            const response = await fresh.send("Runtime.evaluate", {expression, returnByValue:true});
            if (response.exceptionDetails) throw new Error("fresh evaluation exception");
            return response.result?.value;
          },
          freeze: async () => {
            const bundle = await peer.client.freezeQaTrace();
            return {schema:bundle?.schema, bytes:Buffer.byteLength(JSON.stringify(bundle ?? null))};
          },
        });
      } finally { fresh.close(); }
    }
    if (backgroundCount) {
      backgroundEvidence.after = await backgroundEvaluate("window.__zubspaceCapacityReceiptCounts()");
      const {before, after} = backgroundEvidence;
      if (before.openPeers !== backgroundCount || after.openPeers !== backgroundCount ||
          after.snapshots.length !== backgroundCount || after.deltas.length !== backgroundCount ||
          !after.snapshots.every((n, i) => n > before.snapshots[i]) ||
          !after.deltas.every((n, i) => n > before.deltas[i])) {
        throw new Error("background RTC receipts did not continue throughout moving window");
      }
    }
    // One verdict per rig, with the rig's authority attached to it, rather than a single number whose
    // meaning depends on which browser flag happened to pick the rasterizer. Computed BEFORE the failure
    // throw: the classification exists for the failing run, and a policy that only reaches result.json on
    // success is invisible in exactly the case where someone needs it - the first version did that, and a
    // 12-tick software-raster breach produced a thrown blob with no rig classification at all.
    const peerVerdicts = evidence?.peers ?? (Array.isArray(evidence) ? evidence : []);
    const admissionGapFailures = peerVerdicts
      .filter((peer) => (peer?.failures || []).includes("admission-gap"))
      .map((peer) => peer?.playerId ?? null);
    const rigBoundCeiling = graphicsBackend === "swiftshader";
    const motionContract = {
      rig: graphicsBackend,
      gapCeilingIsRigBound: rigBoundCeiling,
      maximumGapTicks: evidence?.maximumGapTicks ?? null,
      admissionGapFailures,
      worstGapTicks: peerVerdicts.length ? Math.max(...peerVerdicts.map((peer) => peer?.maximumGap ?? 0)) : null,
      classification: admissionGapFailures.length
        ? rigBoundCeiling ? "rig-bound-ceiling-unclassified" : "product-motion-floor-failure"
        : "motion-floor-met",
      renewCommand: admissionGapFailures.length && rigBoundCeiling
        ? "make test-e2e-moving-peers-gpu" : null,
    };
    if (!evidence.ok) {
      // A failing movement run is the run that most needs evidence, so it gets its own artifact with
      // provenance rather than only a thrown blob in a log.
      const failureDir = `${artifactDir}-motion-failure`;
      fs.mkdirSync(failureDir, { recursive: true });
      const failureProvenance = await collectArtifactProvenanceAsync(options);
      fs.writeFileSync(`${failureDir}/result.json`, JSON.stringify({
        ok: false, failure: "admission-floor", motionContract, graphicsRig: graphicsRig(),
        evidence, staticBundle: { requested: staticBundleOverride, appRoot: staticServer.appRoot,
          fixtureRoot: staticServer.fixtureRoot }, provenance: failureProvenance,
      }, null, 2));
      throw new Error(`concurrent movement failed: ${JSON.stringify(motionContract)}; `
        + `full verdicts and provenance in ${failureDir}/result.json; raw evidence ${JSON.stringify(evidence)}`);
    }

    const releaseOffset = admittedRecords().length;
    await Promise.all(peers.map((peer) => peer.client.keyUp("ArrowUp")));
    await waitUntil("every peer admits key release", async () => peers.every((peer) =>
      admittedRecords().slice(releaseOffset).some((r) => r.player_id === peer.playerId && r.input_bits === 0)));
    const measuredStatus = await status();
    if (measuredStatus.clients !== count + backgroundCount || measuredStatus.rtc_sessions !== count + backgroundCount) {
      throw new Error("combined population changed during movement");
    }
    const disconnectRtcPopulation = async () => {
      await Promise.all(peers.map((peer) => peer.client.evaluate("(window.__zubspaceSendDisconnectForTest(), true)")));
      if (backgroundCount) await backgroundEvaluate("(window.__zubspaceCapacityPeers.forEach(peer => peer.close()), true)");
    };
    const dataChannelDeliveryAudit = await assertDataChannelDeliveryAudit({ server, serverPort: port,
      browserLogs: peers.map((peer) => peer.chrome.logs), disconnect: disconnectRtcPopulation });
    let cleanup;
    await waitUntil("all RTC peers and queues drain", async () => {
      cleanup = await status();
      return cleanup.clients === 0 && cleanup.rtc_sessions === 0 &&
        cleanup.rtc_ingress_execution?.in_flight === 0 &&
        cleanup.rtc_ingress_execution?.waiting === 0 &&
        cleanup.rtc_outbound_queue_packets === 0 && cleanup.rtc_outbound_queue_bytes === 0 &&
        Object.values(cleanup.rtc_inbound_queue_packets || {}).every((n) => n === 0);
    }, 10000);
    const securityAudit = assertSecurityRejectAudit(server);
    const provenance = await collectArtifactProvenanceAsync(options);
    assertArtifactProvenanceMatches(initial, provenance);
    const successArtifacts = await saveBrowserE2eArtifacts({ artifactDir, server, staticServer,
      urls: peers.map((p) => p.url), clients: peers,
      extraLogs: { ...Object.fromEntries(peers.map((p) => [p.label, p.chrome.logs])), ...windowTraces,
        background: JSON.stringify(backgroundEvidence),
        "background-browser": background?.chrome?.logs || [],
        ...profileLogs() }, provenance });
    fs.writeFileSync(`${artifactDir}/result.json`, JSON.stringify({ ok: !diagnosticOnly, diagnosticOnly,
      scope: "genuine-key concurrent browser movement; not combat or 200-peer acceptance unless count=200",
      count, arena, serverProfile, serverLogLevel: process.env.ZUBSPACE_SMOKE_LOG_LEVEL || "debug",
      staticBundle: { requested: staticBundleOverride, appRoot: staticServer.appRoot,
        fixtureRoot: staticServer.fixtureRoot },
      backgroundEvidence, totalRtcPopulation: count + backgroundCount,
      requestedCpuSets: {server: serverCpuSet, browsers: browserCpuSet},
      observedCpuSets,
      graphicsEvidence,
      graphicsRig: graphicsRig(),
      motionContract,
      attributionProfile: attribution,
      attributionEvidence,
      displays: peers.map((p) => p.display.display),
      evidence, recordReader: reader.stats, measuredStatus, cleanup, securityAudit, dataChannelDeliveryAudit,
      provenance, successArtifacts }, null, 2));
    console.log(JSON.stringify({ ok: !diagnosticOnly, diagnosticOnly, count,
      graphicsRig: graphicsRig(), motionContract, resultPath: `${artifactDir}/result.json` }));
  } catch (error) {
    // Preserve startup backlog too: readiness can fail before the measured
    // window exists. Bound this diagnostic read without replacing the failure.
    try {
      const response = await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`, {
        signal: AbortSignal.timeout(3000),
      });
      if (!response.ok) throw new Error(`status HTTP ${response.status}`);
      const current = await response.json();
      executionEvidence.failure = {at: new Date().toISOString(),
        execution: current.rtc_ingress_execution,
        inboundQueuePackets: current.rtc_inbound_queue_packets,
        clients: current.clients, rtcSessions: current.rtc_sessions};
    } catch (captureError) {
      executionEvidence.failure = {captureError: String(captureError)};
    }
    await finishProfile().catch(() => {});
    if (attribution) await collectPeerAttribution().catch(() => {});
    await captureWindowTraces().catch((captureError) => {
      windowTraces = { "window-capture-failure": captureError.stack };
    });
    await saveBrowserE2eArtifacts({ artifactDir: `${artifactDir}-failure`, server, staticServer,
      provenance: await collectArtifactProvenanceAsync(options),
      urls: peers.map((p) => p.url), clients: peers.filter((p) => p.client),
      extraLogs: { failure: error.stack, evidence: JSON.stringify(evidence), recordReader: JSON.stringify(reader.stats),
        attribution: JSON.stringify(attributionEvidence),
        "cpu-sets": JSON.stringify({requested: {server: serverCpuSet, browsers: browserCpuSet}, observed: observedCpuSets}),
        background: JSON.stringify(backgroundEvidence),
        "background-browser": background?.chrome?.logs || [],
        ...Object.fromEntries(peers.map((p) => [p.label, p.chrome?.logs || p.display.logs])), ...windowTraces,
        ...profileLogs() } });
    console.error(`artifacts: ${artifactDir}-failure`);
    throw error;
  } finally {
    if (background) {
      background.cdp?.close();
      if (background.chrome) await stopProcess(background.chrome.child);
      await stopProcess(background.display.child);
    }
    for (const peer of peers) {
      await peer.client?.keyUp("ArrowUp").catch(() => {});
      await peer.client?.close().catch(() => {});
      if (peer.chrome) await stopProcess(peer.chrome.child);
      await stopProcess(peer.display.child);
    }
    desktopDisplay?.close();
    await stopProcess(server.child);
    staticServer.server.close();
  }
}
main().catch((error) => { console.error(error.stack); process.exitCode = 1; });
