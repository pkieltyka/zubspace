#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";
// Necessary topology/measurement fragment, NOT the full mixed-WAN gameplay
// acceptance contract. Real WASM, real keys, real SCTP/DTLS/UDP; no packet hooks.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { startRtcUdpImpairment } from "../../tools/rtc_udp_impairment.mjs";
import { freePort, sleep, startRealServer, startStaticServer, stopProcess,
  waitForServerListening } from "../../tools/browser_firefox_rtc_smoke.mjs";
import { assertDataChannelDeliveryAudit, assertSecurityRejectAudit, openChromeClient,
  startChrome, waitForChrome, waitUntil } from "./combat_replay.mjs";
import { collectArtifactProvenance, assertArtifactProvenanceMatches } from "./artifact_provenance.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import { artifactFileManifest, assertRetainedArtifactDirectoryAvailable,
  validateArtifactPathReferences } from "./artifact_paths.mjs";
import { mixedUdpProfiles } from "./udp_path_profiles.mjs";
import { observeProbeTiming } from "./udp_probe_timing.mjs";
import { browserConsoleEvidence } from "../../tools/browser_console_evidence.mjs";

const runStartedAt = new Date().toISOString();
const e2eScript = "tests/e2e/udp_path_probe.mjs";
const profileKind = process.env.ZUBSPACE_UDP_PROFILE || "fifo-wan";
const profiles = mixedUdpProfiles(profileKind);
const qualityOnly = process.env.ZUBSPACE_UDP_PROBE_QUALITY_ONLY === "1";
const artifactDir = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR ||
  path.resolve(`.debug/udp-path-probe-${Date.now()}-${process.pid}`);
assertRetainedArtifactDirectoryAvailable(artifactDir);
fs.mkdirSync(artifactDir, { recursive: true });
const initial = collectArtifactProvenance({ runStartedAt, e2eScript });
const serverPort = await freePort();
const metricsPort = await freePort();
const staticServer = await startStaticServer();
const server = startRealServer(serverPort, { zone: "chaos-zone", botCount: 0,
  env: { ZUBSPACE_SERVER_SIM_OBSERVE: "1", ZUBSPACE_SERVER_RTC_UDP_BIND_IP: "127.0.0.1",
    ZUBSPACE_SERVER_METRICS_LISTEN: `127.0.0.1:${metricsPort}` } });
// Ordinary default Auto, never a fixed-profile override. Only the fixture's
// owned ICE listener is loopback-pinned; no host/deployment settings change.
const browsers = [], actors = [], urls = [];
let bridge = null, failure = null;
const result = { ok: false, diagnosticOnly: true, contract: qualityOnly
  ? "udp-probe-quality-measurement-only" : "udp-route-and-measurement-fragment",
  profileKind, runStartedAt, phases: [], genuineInputs: [], firstShots: [], controlCompletion: [], bulkControl: null };
function inputs() {
  return server.logs.join("").split("\n").filter(line => line.includes("human authority input trace"))
    .map(line => Object.fromEntries([...line.matchAll(/(\w+)=(-?[\d.]+)/g)]
      .map(([, key, value]) => [key, Number(value)])));
}
function spawnedProjectiles() {
  return server.logs.join("").split("\n").filter(line => line.includes("server projectile spawned"))
    .map(line => Object.fromEntries([...line.matchAll(/(\w+)=(-?[\d.]+)/g)]
      .map(([, key, value]) => [key, Number(value)])));
}

async function sendChatToBoth(sender, marker) {
  await sender.press("Enter", 40); await sender.typeChatText(marker);
  const sentAt = Date.now(); await sender.press("Enter", 40);
  await waitUntil(`reliable chat ${marker} reaches both perspectives`, async () => {
    const deliveries = await Promise.all(actors.map(actor => actor.evaluate(`(() => {
      const messages = window.__zubspaceUiMetrics?.localGameplayState?.chatMessages || [];
      return messages.filter(text => text.endsWith(': ' + ${JSON.stringify(marker)})).length;
    })()`)));
    return deliveries.length === 2 && deliveries.every(count => count === 1);
  }, 8000);
  return Date.now() - sentAt;
}


try {
  await waitForServerListening(serverPort, server.child, server.logs);
  const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
  assert.equal(status.observer_network_profile, "auto");
  result.observerProfile = status.observer_network_profile;
  bridge = await startRtcUdpImpairment({ upstream: `http://127.0.0.1:${serverPort}/v1/rtc/offer`,
    origin: `http://127.0.0.1:${staticServer.port}`,
    profiles });
  for (const label of ["udp-fast", "udp-slow"]) {
    const browser = startChrome(await freePort(), label); browsers.push(browser);
    await waitForChrome(browser);
    const url = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(bridge.url)}&arena=chaos-zone&name=${label.replaceAll("-", "")}&metrics=1`;
    urls.push(url);
    const actor = await openChromeClient(browser, url, { initScript: `(${observeProbeTiming.toString()})()` }); actors.push(actor);
    await actor.enterShip();
    const readyWaitStarted = Date.now();
    if (actors.length === 2 && !qualityOnly) {
      // The older fragment sent chat only after both downloads finished. It
      // could not establish control completion alongside active file traffic.
      const before = await actor.evaluate("window.__zubspaceUiMetrics?.localGameplayState || null");
      assert.ok(before?.pendingFileRequests > 0 && before.zoneFilesReady === false,
        "bulk/control measurement requires a genuinely unfinished slow-peer transfer");
      const fast = actors[0];
      const fastId = result.firstShots[0].player_id;
      const inputOffset = inputs().length;
      const motionStarted = Date.now();
      await fast.keyDown("ArrowUp");
      try {
        await waitUntil("fast-peer motion reaches authority during slow-peer bulk transfer", () =>
          inputs().slice(inputOffset).some(input => input.player_id === fastId && (input.input_bits & 1) !== 0), 5000);
      } finally { await fast.keyUp("ArrowUp"); }
      const motionAdmittedMs = Date.now() - motionStarted;
      const deliveredToBothMs = await sendChatToBoth(fast, `udpbulk${process.pid}`);
      const after = await actor.evaluate("window.__zubspaceUiMetrics?.localGameplayState || null");
      result.bulkControl = { pendingFilesBefore: before.pendingFileRequests,
        chunksBefore: before.fileChunksAccepted, pendingFilesAfter: after?.pendingFileRequests,
        chunksAfter: after?.fileChunksAccepted, motionAdmittedMs, deliveredToBothMs };
      assert.ok(result.bulkControl.chunksAfter > result.bulkControl.chunksBefore,
        "bulk file delivery must progress while movement and reliable control complete");
    }
    let lastReadyLog = 0;
    let readiness = null;
    await waitUntil(`${label} canonical gameplay step ready`, async () => {
      readiness = await actor.evaluate(`(() => {
        const m = window.__zubspaceUiMetrics?.localGameplayState || {};
        return { ready: m.gameplayStepReady === true, phase: m.authorityPhase,
          playerId: window.__zubspaceUiMetrics?.selfPlayerId,
          lifeSeq: window.__zubspaceUiMetrics?.playerLifeSeq,
          mapReady: m.zoneMapReady, filesReady: m.zoneFilesReady,
          pendingFiles: m.pendingFileRequests, chunks: m.fileChunksAccepted };
      })()`);
      if (!readiness.ready && Date.now() - lastReadyLog > 10000) {
        lastReadyLog = Date.now(); console.log(JSON.stringify({ stage: "startup-wait", label,
          elapsedMs: Date.now() - readyWaitStarted, readiness }));
      }
      return readiness.ready;
    }, 75000); // Diagnostic budget for the full 2 MB zone over impaired SCTP;
    // retain actual startup duration, never treat this as a startup SLA pass.
    result.genuineInputs.push({ label, startupReadyWaitMs: Date.now() - readyWaitStarted, readiness });
    assert.ok(Number.isInteger(readiness.playerId), "canonical readiness must identify the exact pilot");
    assert.ok(Number.isInteger(readiness.lifeSeq), "canonical readiness must identify the exact life");
    console.log(JSON.stringify({ stage: "client-ready", label, bridge: bridge.snapshot().errors }));
    await actor.evaluate(`(() => {
      const sample = { startedAt: performance.now(), frames: 0, maxGapMs: 0, endedAt: null };
      window.__udpCadence = sample; let previous = performance.now();
      function frame(now) { sample.frames++; sample.maxGapMs = Math.max(sample.maxGapMs, now - previous);
        previous = now; if (now - sample.startedAt < 25000) requestAnimationFrame(frame); else sample.endedAt = now; }
      requestAnimationFrame(frame); return true;
    })()`);
    const inputOffset = inputs().length;
    await actor.keyDown("ArrowUp"); await sleep(qualityOnly ? 100 : 450); await actor.keyUp("ArrowUp");
    await waitUntil(`${label} genuine thrust reaches authority`, () =>
      inputs().slice(inputOffset).some(input => input.player_id === readiness.playerId && (input.input_bits & 1) !== 0), 5000);
    if (qualityOnly) {
      await actor.press("ArrowDown", 100);
      result.genuineInputs.push({ label, records: inputs().slice(inputOffset) });
      continue;
    }
    await actor.press("Control", 350);
    await waitUntil(`${label} initial weapon edge reaches authority`, () =>
      inputs().slice(inputOffset).some(input => input.player_id === readiness.playerId && input.reported_weapon !== 0), 5000);
    const firstShot = inputs().slice(inputOffset).find(input =>
      input.player_id === readiness.playerId && input.reported_weapon !== 0);
    assert.equal(firstShot.life_seq, readiness.lifeSeq, "the first edge must stay in its admitted life");
    let committed = null;
    await waitUntil(`${label} first reported logical weapon edge owns an authoritative projectile`, () => {
      committed = spawnedProjectiles().find(projectile => projectile.owner_id === readiness.playerId &&
        projectile.owner_life_seq === readiness.lifeSeq && projectile.owner_input_sequence === firstShot.input_sequence);
      return committed !== undefined;
    }, 5000);
    // A mirrored copy may commit this edge before the position copy arrives;
    // that later duplicate truthfully reports accepted_weapon=0. Correlate the
    // actual logical owner/life/input commitment, not one transport copy's status.
    result.firstShots.push({ label, ...firstShot, committed });
    assert.equal(committed.weapon, firstShot.reported_weapon);
    result.genuineInputs.push({ label, records: inputs().slice(inputOffset) });
  }
  if (qualityOnly) {
    // No rendezvous, combat or acceptance claim. Keep the original UDP profile
    // throughout the complete quality window; capture both foreground pages.
    for (let sample = 0; sample <= 16; sample++) {
      const pages = await Promise.all(actors.map(actor => actor.evaluateAwait(`(async () => {
        const m = window.__zubspaceUiMetrics || {}, local = m.localGameplayState || {};
        return { rtc: await window.__zubspaceRtcStatsForTest(),
          backlog: window.__zubspaceRtcReceiveBacklog(), probeTiming: window.__udpProbeTiming,
          ui: { selfPlayerId: m.selfPlayerId, playerLifeSeq: m.playerLifeSeq,
            phase: local.authorityPhase, rtcRejoinCount: local.rtcRejoinCount,
            holdRemainingMs: local.connectionQualityHoldRemainingMs,
            combatViewTimeline: m.combatViewTimeline },
          network: (window.__zubspaceNetworkLog || []).slice(-30) };
      })()`)));
      const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
      result.phases.push({ label: `quality-${sample}`, at: new Date().toISOString(),
        bridge: bridge.snapshot(), pages, status });
      console.log(JSON.stringify({ stage: "quality-sample", sample, quality: status.connection_quality }));
      if (sample < 16) await sleep(5000);
    }
    fs.writeFileSync(path.join(artifactDir, "quality-final.prom"),
      await (await fetch(`http://127.0.0.1:${metricsPort}/metrics`)).text());
  } else {
  for (let index = 0; index < actors.length; index++) {
    const marker = `udpcheck${process.pid}${index}`;
    const deliveredToBothMs = await sendChatToBoth(actors[index], marker);
    result.controlCompletion.push({ sender: index, marker, deliveredToBothMs });
  }
  await sleep(4000);
  const snapshot = async label => {
    const pages = [];
    for (const actor of actors) pages.push(await actor.evaluateAwait(`(async () => ({
      rtc: await window.__zubspaceRtcStatsForTest(), backlog: window.__zubspaceRtcReceiveBacklog(),
      cadence: window.__udpCadence, ui: window.__zubspaceUiMetrics || null,
      probeTiming: window.__udpProbeTiming,
      network: (window.__zubspaceNetworkLog || []).slice(-30)
    }))()`));
    const status = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
    const metricReply = await fetch(`http://127.0.0.1:${metricsPort}/metrics`);
    assert.equal(metricReply.status, 200);
    const metrics = await metricReply.text();
    assert.ok(metrics.includes("zubspace_observer_auto_targets"));
    fs.writeFileSync(path.join(artifactDir, `${label}.prom`), metrics);
    result.phases.push({ label, at: new Date().toISOString(), bridge: bridge.snapshot(), pages, status });
  };
  await snapshot("impaired");
  const impaired = result.phases.at(-1);
  assert.equal(impaired.bridge.sessions.length, 2);
  assert.equal(impaired.pages[0].ui?.combatViewTimeline?.targetDelayTicks, 7,
    "the impaired peer must not raise the fast peer's independently measured target");
  const slowTarget = impaired.pages[1].ui?.combatViewTimeline?.targetDelayTicks;
  assert.ok(slowTarget >= 15 && slowTarget <= 20, "the impaired peer must apply a measured WAN target within its cap");
  for (let index = 0; index < 2; index++) {
    const session = impaired.bridge.sessions[index];
    const connection = impaired.pages[index].rtc.connections[0];
    assert.equal(impaired.pages[index].rtc.connectionCount, 1, "one pinned association per exact pilot is required");
    assert.equal(connection.connectionState, "connected");
    const selectedId = connection.transport.find(entry => entry.selectedCandidatePairId)?.selectedCandidatePairId;
    const pair = connection.candidatePairs.find(entry => selectedId ? entry.id === selectedId : entry.nominated && entry.state === "succeeded");
    assert.ok(pair, "selected/nominated ICE pair required");
    const remote = connection.candidates.find(entry => entry.id === pair.remoteCandidateId);
    assert.equal(remote?.port, session.frontPort, "selected route must terminate at this association's front socket");
    assert.equal(remote?.address, "127.0.0.1");
    assert.equal(session.transport.unexpectedSources, 0);
    for (const lane of [session.transport.c2s, session.transport.s2c]) {
      assert.ok(lane.applicationForwarded > 0, "real encrypted application traffic must cross each path");
      assert.equal(lane.overflowDrops, 0); assert.equal(lane.sendErrors, 0);
    }
  }
  const slow = impaired.bridge.sessions[1].transport;
  assert.ok(slow.c2s.maximumQueueAgeMs >= 55 && slow.s2c.maximumQueueAgeMs >= 100,
    "authored asymmetric delay must be realized");
  assert.ok(slow.c2s.applicationDrops + slow.s2c.applicationDrops > 0,
    "loss must affect encrypted application datagrams, not just STUN");
  const recovery = profileKind === "fifo-wan" ? { preserveOrder: true } : {};
  bridge.setProfiles(1, recovery, recovery);
  await Promise.all(actors.map(async actor => {
    await actor.keyDown("ArrowRight"); await sleep(400); await actor.keyUp("ArrowRight");
    await actor.press("Control", 300);
  }));
  const recoveryStarted = Date.now();
  await waitUntil("both clients apply the recovered 70 ms view without a hold", async () => {
    const views = await Promise.all(actors.map(actor => actor.evaluate(`window.__zubspaceUiMetrics?.combatViewTimeline || null`)));
    result.lastRecoveryViews = views;
    return views.every(view => view?.targetDelayTicks === 7 && view.delayTicks === 7 &&
      view.presentationRenderTick === view.scheduledRenderTick && view.weaponReady === true);
  }, 60000);
  result.appliedRecoveryMs = Date.now() - recoveryStarted;
  await snapshot("recovery");
  }
  result.securityAudit = assertSecurityRejectAudit(server);
  result.deliveryAudit = await assertDataChannelDeliveryAudit({ server, serverPort,
    browserLogs: browsers.flatMap(browser => browser.logs), disconnect: async () => {
      await Promise.all(actors.map(actor => actor.evaluate("(window.__zubspaceSendDisconnectForTest(), true)")));
    } });
  await bridge.close(); result.bridgeCleanup = bridge.snapshot();
  for (const session of result.bridgeCleanup.sessions) {
    assert.equal(session.transport.closed, true); assert.equal(session.transport.pendingBytes, 0);
    for (const lane of [session.transport.c2s, session.transport.s2c]) {
      assert.equal(lane.queuedPackets, 0); assert.equal(lane.queuedBytes, 0); assert.equal(lane.timerActive, false);
    }
  }
  result.provenance = await collectArtifactProvenanceAsync({ runStartedAt, e2eScript });
  assertArtifactProvenanceMatches(initial, result.provenance);
  result.ok = true;
} catch (error) {
  failure = error; result.failure = error.stack;
  result.provenance = initial;
  result.bridgeAtFailure = bridge?.snapshot();
  for (let index = 0; index < actors.length; index++) {
    try { fs.writeFileSync(path.join(artifactDir, `failure-${index}.json`), JSON.stringify({
      keys: await actors[index].keyState(), stats: await actors[index].evaluateAwait("window.__zubspaceRtcStatsForTest()"),
      network: await actors[index].evaluate("(window.__zubspaceNetworkLog || []).slice(-60)"),
      probeTiming: await actors[index].evaluate("window.__udpProbeTiming"),
      events: actors[index].events.slice(-40),
    }, null, 2) + "\n"); } catch {}
    try { fs.writeFileSync(path.join(artifactDir, `failure-${index}.png`), Buffer.from(await actors[index].screenshot(), "base64")); } catch {}
  }
  try { fs.writeFileSync(path.join(artifactDir, "failure.prom"), await
    (await fetch(`http://127.0.0.1:${metricsPort}/metrics`)).text());
    result.statusAtFailure = await (await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`)).json();
  } catch {}
} finally {
  for (let index = 0; index < actors.length; index++) {
    fs.writeFileSync(path.join(artifactDir, `browser-${index}-console.json`),
      JSON.stringify(browserConsoleEvidence(actors[index]), null, 2) + "\n");
  }
  for (const actor of actors) await actor.close().catch(() => {});
  await bridge?.close();
  result.bridgeCleanup ??= bridge?.snapshot();
  await Promise.all([...browsers.map(browser => stopProcess(browser.child)), stopProcess(server.child)]);
  await new Promise(resolve => staticServer.server.close(resolve));
  fs.writeFileSync(path.join(artifactDir, "server.log"), server.logs.join(""));
  for (let index = 0; index < browsers.length; index++) {
    fs.writeFileSync(path.join(artifactDir, `browser-${index}.log`), browsers[index].logs.join(""));
    if (browsers[index].userDataDir) fs.rmSync(browsers[index].userDataDir,
      { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
  }
  result.artifactFiles = artifactFileManifest(artifactDir);
  const resultPath = path.join(artifactDir, "result.json");
  fs.writeFileSync(resultPath, JSON.stringify(result, null, 2) + "\n");
  validateArtifactPathReferences(result, resultPath);
  console.log(JSON.stringify({ ok: result.ok, diagnosticOnly: true, artifactDir, failure: failure?.message }));
}
if (failure) process.exitCode = 1;
