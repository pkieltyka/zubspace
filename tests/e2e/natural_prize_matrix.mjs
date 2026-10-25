#!/usr/bin/env node
import { fetchGameStatusResponse } from "../../tools/game_status.mjs";

import fs from "node:fs";
import path from "node:path";

import {
  assertDataChannelDeliveryAudit,
  assertSecurityRejectAudit,
  brakeClientVelocity,
  compactMetrics,
  openChromeClient,
  simulatedLossSummary,
  startChrome,
  waitForChrome,
  waitUntil,
} from "./combat_replay.mjs";
import {
  assertArtifactProvenanceMatches,
  collectArtifactProvenance,
} from "./artifact_provenance.mjs";
import { collectArtifactProvenanceAsync } from "./artifact_provenance_async.mjs";
import {
  artifactFileManifest,
  createAtomicArtifactDirectory,
  validateArtifactPathReferences,
} from "./artifact_paths.mjs";
import { assertCompleteQaTraceCaptures, captureBrowserArtifactState } from "./qa_trace_capture.mjs";
import { writeQaTraceArtifact, writeQaTraceFailureResult } from "./qa_trace_artifacts.mjs";
import { assertRendererConsoleClean, browserConsoleEvidence } from "../../tools/browser_console_evidence.mjs";
import { navigateAuthoredTarget } from "./authored_navigation.mjs";
import { loadRendezvousMap } from "./player_rendezvous.mjs";
import { NATURAL_PRIZE_ARRIVAL_RADIUS_PX, naturalPrizePreclaimStage, planNaturalPrizeNavigation } from "./natural_prize_navigation.mjs";
import {
  createSession,
  deleteSession,
  execute,
  freezeQaTrace,
  firefoxArtifactClient,
  freePort,
  holdFirefoxKeys,
  metricsState as firefoxMetricsState,
  openClient,
  pageState,
  repoRoot,
  screenshot,
  sleep,
  startGeckodriver,
  startRealServer,
  startStaticServer,
  stopProcess,
  waitForGeckodriver,
  waitForServerListening,
} from "../../tools/browser_firefox_rtc_smoke.mjs";

const browser = process.env.ZUBSPACE_E2E_BROWSER || "chrome";
const arena = process.env.ZUBSPACE_E2E_ARENA || "chaos-zone";
const seed = Number(process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED ||
  (arena === "chaos-zone" ? "377" : "244"));
const simulatedTxLossPct = Number(process.env.ZUBSPACE_SERVER_TX_LOSS_PCT || "0");
const simulatedTxLossSeed = process.env.ZUBSPACE_SERVER_TX_LOSS_SEED == null
  ? null
  : Number(process.env.ZUBSPACE_SERVER_TX_LOSS_SEED);
process.env.ZUBSPACE_HIDDEN_PRIZE_RNG_SEED = String(seed);
process.env.ZUBSPACE_SERVER_SIM_OBSERVE ||= "1";
const runStartedAt = new Date().toISOString();
const e2eScript = "tests/e2e/natural_prize_matrix.mjs";
const wasmPath = "target/wasm32-unknown-unknown/debug/zubspace.wasm";
// Acquisition needs room for a real zero-to-one grant. Production keeps its
// maximum-entry policy; this isolated fixture uses authored reference starts.
const fixture = {
  maxedSpawnLoadout: false,
  arenaSettings: process.env.ZUBSPACE_SERVER_ZONE_ROOT ? "explicit disk-catalog fixture" : "unmodified",
  zoneRoot: process.env.ZUBSPACE_SERVER_ZONE_ROOT || null,
};
const target = process.env.ZUBSPACE_E2E_SUCCESS_ARTIFACT_DIR || path.join(
  "/tmp",
  `zubspace-natural-prize-${browser}-${arena}-${Date.now()}-${process.pid}`,
);

if (!new Set(["chrome", "firefox"]).has(browser)) {
  throw new Error("ZUBSPACE_E2E_BROWSER must be chrome or firefox");
}
if (!new Set(["chaos-zone", "extreme"]).has(arena)) {
  throw new Error("natural prize matrix is scoped to chaos-zone and extreme");
}
if (process.env.ZUBSPACE_SMOKE_BOT_COUNT !== "0") {
  throw new Error("natural prize matrix requires ZUBSPACE_SMOKE_BOT_COUNT=0");
}

function numericField(line, field) {
  const match = line.match(new RegExp(`(?:^|\\s)${field}=(-?\\d+(?:\\.\\d+)?)`));
  return match ? Number(match[1]) : null;
}

function forwardContinuumTicks(fromTick, toTick) {
  if (!Number.isInteger(fromTick) || !Number.isInteger(toTick)) return null;
  const distance = (toTick - fromTick) & 0x7fffffff;
  return distance === 0 || distance >= 0x40000000 ? null : distance;
}

function hiddenBrickSpawns(server) {
  return server.logs.join("").split("\n")
    .filter((line) => line.includes("server hidden prize spawned"))
    .map((line) => ({
      x: numericField(line, "x"),
      y: numericField(line, "y"),
      collectibleId: numericField(line, "collectible_id"),
      prizeId: numericField(line, "prize_id"),
      expiresAtTick: numericField(line, "expires_at_tick"),
    }))
    .filter((entry) => entry.prizeId === 26 &&
      [entry.x, entry.y, entry.collectibleId, entry.expiresAtTick].every(Number.isFinite));
}

function matchingClaims(server, playerId, collectibleId) {
  return server.logs.join("").split("\n")
    .filter((line) => line.includes("server prize claimed"))
    .map((line) => ({
      playerId: numericField(line, "player_id"),
      collectibleId: numericField(line, "collectible_id"),
      x: numericField(line, "x"),
      y: numericField(line, "y"),
      prizeId: numericField(line, "prize_id"),
      authoritativeRecipientCount: numericField(line, "authoritative_recipient_count"),
    }))
    .filter((entry) => entry.playerId === playerId && entry.collectibleId === collectibleId);
}

function u16(bytes, offset) {
  return (bytes[offset] | (bytes[offset + 1] << 8)) >>> 0;
}

function u32(bytes, offset) {
  return (
    bytes[offset] |
    (bytes[offset + 1] << 8) |
    (bytes[offset + 2] << 16) |
    (bytes[offset + 3] << 24)
  ) >>> 0;
}

function logicalPrizePackets(state, collectibleId) {
  const bySequence = new Map();
  for (const entry of [...(state.packetLogFirst || []), ...(state.packetLog || [])]) {
    const bytes = entry.bytes || [];
    const key = entry.seq ?? `${entry.direction}:${entry.at}:${JSON.stringify(bytes)}`;
    bySequence.set(key, entry);
  }
  const requests = [];
  const acknowledgements = [];
  const seenRequestIds = new Set();
  const seenAckIds = new Set();
  for (const entry of bySequence.values()) {
    const bytes = entry.bytes || [];
    if (entry.direction === "sent" && bytes[0] === 0x07 && bytes.length === 11 &&
      u32(bytes, 1) === collectibleId) {
      requests.push({
        reliableId: null,
        collectibleId: u32(bytes, 1),
        sequence: entry.seq ?? null,
        at: entry.at ?? null,
        bytes,
      });
      continue;
    }
    if (bytes[0] !== 0 || bytes[1] !== 3 || bytes.length < 11) continue;
    const reliableId = u32(bytes, 2);
    if (entry.direction === "sent" && bytes[6] === 0x07 && bytes.length === 17 &&
      u32(bytes, 7) === collectibleId && !seenRequestIds.has(reliableId)) {
      seenRequestIds.add(reliableId);
      requests.push({
        reliableId,
        collectibleId: u32(bytes, 7),
        sequence: entry.seq ?? null,
        at: entry.at ?? null,
        bytes,
      });
    }
    if (entry.direction === "received" && bytes[6] === 0x61 && bytes.length === 50 &&
      u32(bytes, 7) === collectibleId && !seenAckIds.has(reliableId)) {
      seenAckIds.add(reliableId);
      const rawPrizeId = u16(bytes, 15);
      acknowledgements.push({
        reliableId,
        collectibleId: u32(bytes, 7),
        sequence: entry.seq ?? null,
        at: entry.at ?? null,
        x: u16(bytes, 11),
        y: u16(bytes, 13),
        prizeId: rawPrizeId >= 0x8000 ? rawPrizeId - 0x1_0000 : rawPrizeId,
        lifeSeq: u32(bytes, 19),
        revision: u32(bytes, 23),
        bytes,
      });
    }
  }
  return { requests, acknowledgements };
}

function prizeRequestPayload(request) {
  // Extreme uses a tracked-reliable wrapper while Chaos sends the scoped
  // overlap request directly on the gameplay lane. Compare the complete
  // immutable payload in either case, excluding only the reliable wrapper ID
  // that is expected to change across retries.
  return request.reliableId == null ? request.bytes : request.bytes.slice(6);
}

function prizeRequestEvidenceKey(request) {
  return `${request.sequence}:${request.at}:${JSON.stringify(request.bytes)}`;
}

function prizePacketCandidates(state) {
  const bySequence = new Map();
  for (const entry of [...(state.packetLogFirst || []), ...(state.packetLog || [])]) {
    const bytes = entry.bytes || [];
    const key = entry.seq ?? `${entry.direction}:${entry.at}:${JSON.stringify(bytes)}`;
    bySequence.set(key, entry);
  }
  return [...bySequence.values()]
    .filter((entry) => {
      const bytes = entry.bytes || [];
      return (bytes[0] === 0x07 && bytes.length === 11) ||
        (bytes[0] === 0 && bytes[1] === 3 && (bytes[6] === 0x07 || bytes[6] === 0x61));
    })
    .map((entry) => ({
      direction: entry.direction,
      seq: entry.seq,
      length: (entry.bytes || []).length,
      reliableId: entry.bytes?.[0] === 0 ? u32(entry.bytes, 2) : null,
      payloadType: entry.bytes?.[0] === 0 ? entry.bytes?.[6] : entry.bytes?.[0],
      collectibleId: entry.bytes?.[0] === 0 ? u32(entry.bytes, 7) : u32(entry.bytes, 1),
    }));
}

function authorityReady(state) {
  const metrics = state?.uiMetrics || {};
  const local = metrics.localGameplayState || {};
  return metrics.playerAlive === true && metrics.serverPlayerAlive === true &&
    Number.isInteger(metrics.selfPlayerId) && Number.isInteger(metrics.playerLifeSeq) &&
    // Ordinary gameplay is intentionally admitted at authoritative map
    // publication. The seeded post-bootstrap-loss replay additionally waits
    // for full file publication so its first loss sample belongs to live
    // gameplay rather than the bounded map/LVZ transfer.
    local.zoneMapReady === true &&
    (simulatedTxLossPct === 0 || local.zoneFilesReady === true) &&
    local.pendingShipResetPosition === false &&
    (local.lifecycleBaselineReceiptsMatched ?? 0) >= 1 &&
    (local.lifecycleBaselineReceiptsUnmatched ?? 0) === 0;
}

async function fetchStatus(port) {
  const response = await fetchGameStatusResponse(`http://127.0.0.1:${port}/v1/status`);
  if (!response.ok) throw new Error(`status failed with HTTP ${response.status}`);
  return response.json();
}

async function waitForCleanup(port) {
  let status = null;
  await waitUntil("natural prize session cleanup", async () => {
    status = await fetchStatus(port);
    return status.clients === 0 && status.joinedPlayers === 0 && status.rtc_sessions === 0 &&
      status.rtc_outbound_queue_packets === 0 && status.rtc_outbound_queue_bytes === 0 &&
      Object.values(status.rtc_inbound_queue_packets || {}).every((count) => count === 0);
  }, 8_000);
  return status;
}

function compactPrizeState(state) {
  const metrics = state?.uiMetrics || {};
  const local = metrics.localGameplayState || {};
  return {
    metrics: compactMetrics(metrics),
    readiness: {
      zoneFilesReady: local.zoneFilesReady,
      zoneMapReady: local.zoneMapReady,
      pendingShipResetPosition: local.pendingShipResetPosition,
      pendingShipResetDeliveryId: local.pendingShipResetDeliveryId,
      lifecycleBaselineReceiptsSeen: local.lifecycleBaselineReceiptsSeen,
      lifecycleBaselineReceiptsMatched: local.lifecycleBaselineReceiptsMatched,
      lifecycleBaselineReceiptsUnmatched: local.lifecycleBaselineReceiptsUnmatched,
      reliableNextProcessId: local.reliableNextProcessId,
      reliableQueuedPackets: local.reliableQueuedPackets,
      pendingFileRequests: local.pendingFileRequests,
      pendingZoneDownloads: local.pendingZoneDownloads,
      pendingZonePublication: local.pendingZonePublication,
    },
    prize: {
      bricks: local.bricks,
      lifeSeq: local.prizeAuthorityLifeSeq,
      revision: local.prizeAuthorityRevision,
      activePrizeCount: local.activePrizeCount,
      activePrizeCollectibleIds: local.activePrizeCollectibleIds,
      playerX: metrics.playerX,
      playerY: metrics.playerY,
      notificationCount: local.prizePickupNotificationCount,
      lastNotification: local.lastPrizePickupNotification,
      soundPlayCount: local.prizePickupSoundPlayCount,
    },
    packetLogTail: (state?.packetLog || []).slice(-48),
    prizeStateTrace: (state?.prizeStateTrace || []).slice(-24),
    networkStats: state?.networkStats || {},
  };
}

async function installPrizeAudioProbe(actor) {
  return actor.evaluate(`(() => {
    if (window.__zubspacePrizeAudioProbe?.installed) {
      return window.__zubspacePrizeAudioProbe;
    }
    const probe = window.__zubspacePrizeAudioProbe = {
      installed: true,
      starts: 0,
      runningStarts: 0,
      errors: [],
    };
    const proto = window.AudioBufferSourceNode?.prototype;
    if (!proto?.start) {
      probe.errors.push("AudioBufferSourceNode.start unavailable");
      return probe;
    }
    const originalStart = proto.start;
    proto.start = function(...args) {
      probe.starts += 1;
      if (this.context?.state === "running") probe.runningStarts += 1;
      try {
        return originalStart.apply(this, args);
      } catch (error) {
        probe.errors.push(String(error));
        throw error;
      }
    };
    return probe;
  })()`);
}

async function prizeAudioProbe(actor) {
  return actor.evaluate(`(() => ({
    ...(window.__zubspacePrizeAudioProbe || {}),
    unlock: window.__zubspaceAudioUnlockState || null,
  }))()`);
}

async function main() {
  if (!fs.existsSync(path.join(repoRoot, wasmPath))) {
    throw new Error(`missing ${wasmPath}; run make build-client first`);
  }
  const initialProvenance = collectArtifactProvenance({ runStartedAt, e2eScript });
  const artifacts = createAtomicArtifactDirectory(target);
  const staticServer = await startStaticServer();
  const serverPort = await freePort();
  const server = startRealServer(serverPort, { zone: arena,
    env: { ZUBSPACE_SERVER_MAXED_SPAWN_LOADOUT: "false" },
  });
  let chrome = null;
  let chromeClient = null;
  let firefox = null;
  let actor = null;
  let state = null;
  let lastPrizePacketEvidence = [];
  let targetBaselineEvidence = null;
  async function disconnectPrizeClient() {
    if (browser === "chrome") {
      await chromeClient.evaluate("window.__zubspaceSendDisconnectForTest?.(); true");
    } else {
      await execute(firefox, "window.__zubspaceSendDisconnectForTest?.(); return true;");
    }
  }

  try {
    await waitForServerListening(serverPort, server.child, server.logs);
    const statusResponse = await fetchGameStatusResponse(`http://127.0.0.1:${serverPort}/v1/status`, {
      signal: AbortSignal.timeout(10_000),
    });
    if (!statusResponse.ok) throw new Error(`natural prize fixture status returned HTTP ${statusResponse.status}`);
    const fixtureStatus = await statusResponse.json();
    if (fixtureStatus.maxed_spawn_loadout !== false || fixtureStatus.zone !== arena || fixtureStatus.botPlayers !== 0) {
      throw new Error(`natural acquisition fixture requires reference loadouts in its requested empty arena: ${JSON.stringify(fixtureStatus)}`);
    }
    const rtcUrl = `http://127.0.0.1:${serverPort}/v1/rtc/offer`;
    const url = `http://127.0.0.1:${staticServer.port}/index.html?rtc=${encodeURIComponent(rtcUrl)}&arena=${encodeURIComponent(arena)}&packetlog=1&trace=1&name=prize${browser}`;
    if (browser === "chrome") {
      chrome = startChrome(await freePort(), `natural-prize-${arena}`);
      await waitForChrome(chrome);
      chromeClient = await openChromeClient(chrome, url);
      actor = chromeClient;
    } else {
      firefox = startGeckodriver(await freePort(), `natural-prize-${arena}-firefox`);
      await waitForGeckodriver(firefox);
      await createSession(firefox);
      await openClient(firefox, url);
      actor = {
        ...firefoxArtifactClient(firefox),
        // Steering needs current metrics, not the multi-megabyte diagnostic
        // histories in pageState. Full snapshots belong at evidence boundaries.
        metricsState: () => firefoxMetricsState(firefox),
        pageState: () => pageState(firefox),
        freezeQaTrace: () => freezeQaTrace(firefox),
        evaluate: (source) => execute(firefox, `return (${source});`),
        press: (key, durationMs) => holdFirefoxKeys(firefox, [key], durationMs),
        hold: (key, durationMs) => holdFirefoxKeys(firefox, [key], durationMs),
      };
    }
    await waitUntil(`${browser} ${arena} initial authority`, async () => {
      state = await actor.metricsState();
      return authorityReady(state);
    }, 70_000);
    // A nearby first-wave collectible can complete before the server's
    // periodic loss summary. Start the prize transaction only after this
    // exact post-bootstrap connection has actually dropped traffic, so a
    // green impaired replay cannot be a zero-loss fast path.
    if (simulatedTxLossPct > 0) {
      await waitUntil(`${browser} ${arena} realized post-bootstrap loss`, () =>
        simulatedLossSummary(server).packetsDropped > 0, 20_000);
    }
    const playerId = state.uiMetrics.selfPlayerId;
    const before = await actor.pageState();
    const beforeLocal = before.uiMetrics.localGameplayState;
    if (beforeLocal.bricks !== 0 || !Number.isInteger(beforeLocal.prizeAuthorityRevision)) {
      throw new Error(`natural Brick precondition was not authoritative zero: ${JSON.stringify(compactPrizeState(before))}`);
    }

    let targetPrize = null;
    let pickupBefore = null;
    let targetNavigation = null;
    let approach = null;
    const loadedMap = loadRendezvousMap(arena, [
      ...((server.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT)
        ? [server.zoneRoot || process.env.ZUBSPACE_SERVER_ZONE_ROOT] : []),
      path.join(repoRoot, "assets/zones"),
    ]);
    await installPrizeAudioProbe(actor);
    await waitUntil(`${arena} natural Brick spawn and presentation`, async () => {
      state = await actor.metricsState();
      const ids = state.uiMetrics?.localGameplayState?.activePrizeCollectibleIds || [];
      const x = state.uiMetrics?.playerX;
      const y = state.uiMetrics?.playerY;
      const currentTick = state.uiMetrics?.latestServerTick;
      const activeSpawns = server.logs.join("").split("\n")
        .filter((line) => line.includes("server hidden prize spawned"))
        .map((line) => ({
          x: numericField(line, "x"),
          y: numericField(line, "y"),
          collectibleId: numericField(line, "collectible_id"),
          prizeId: numericField(line, "prize_id"),
          expiresAtTick: numericField(line, "expires_at_tick"),
        }))
        .filter((spawn) => ids.includes(spawn.collectibleId) &&
          (forwardContinuumTicks(currentTick, spawn.expiresAtTick) ?? 0) > 0);
      const candidates = hiddenBrickSpawns(server)
        .filter((spawn) => ids.includes(spawn.collectibleId))
        // An isolated green lets the replay attribute the exact inventory and
        // revision delta to one overlap even when the seeded wave places
        // another natural prize in the same 16-pixel neighborhood.
        .filter((spawn) => activeSpawns.every((other) =>
          other.collectibleId === spawn.collectibleId ||
          Math.hypot(other.x - spawn.x, other.y - spawn.y) >= 48
        ))
        .sort((left, right) =>
          Math.hypot(left.x - x, left.y - y) - Math.hypot(right.x - x, right.y - y)
        );
      const local = state.uiMetrics.localGameplayState;
      if (local.brickTilesOverflow) throw new Error("incomplete brick geometry for natural prize approach");
      for (const candidate of candidates) {
        // A natural prize may be in another disconnected region. Only choose
        // a target whose real hull route fits its unchanged remaining TTL.
        let navigation;
        try {
          navigation = planNaturalPrizeNavigation(loadedMap.geometry, { x, y }, candidate,
            currentTick, local.brickTiles || []);
        } catch (error) {
          if (error.message !== "no hull-clear authored approach route") throw error;
          continue;
        }
        if (!navigation) continue;
        targetPrize = candidate;
        targetNavigation = { ...navigation, map: loadedMap.provenance };
        break;
      }
      return targetPrize != null;
    }, arena === "extreme" ? 70_000 : 45_000);

    // Random spawn travel is setup. Finish it before taking the exact
    // target-specific inventory/audio baseline, since unrelated natural prizes
    // can legitimately be collected along a long route. The final claim keeps
    // its original30sec bound and must still prove a zero-to-one Brick grant.
    const preclaimStage = naturalPrizePreclaimStage(loadedMap.geometry, targetNavigation,
      state.uiMetrics.localGameplayState.brickTiles || []);
    if (preclaimStage) {
      approach = await navigateAuthoredTarget(actor, preclaimStage, server, {
        timeoutMs: targetNavigation.setupBudgetMs, arrivalRadiusPx: 0,
        maximumDesiredSpeed: 100, desiredSpeedScale: 0.5,
      });
    }
    pickupBefore = await actor.pageState();
    targetBaselineEvidence = { targetPrize, targetNavigation, approach, pickupBefore: compactPrizeState(pickupBefore) };
    if (pickupBefore.uiMetrics.localGameplayState.bricks !== 0 ||
        !pickupBefore.uiMetrics.localGameplayState.activePrizeCollectibleIds.includes(targetPrize.collectibleId) ||
        matchingClaims(server, playerId, targetPrize.collectibleId).length !== 0) {
      throw new Error(`natural Brick pre-claim setup lost its exact zero-to-one precondition: ${JSON.stringify({ targetPrize, approach, state: compactPrizeState(pickupBefore) })}`);
    }
    let claim = null;
    const route = await navigateAuthoredTarget(actor, targetPrize, server, {
      timeoutMs: 30_000, arrivalRadiusPx: NATURAL_PRIZE_ARRIVAL_RADIUS_PX,
      maximumDesiredSpeed: 100, desiredSpeedScale: 0.5,
      complete: async () => {
        claim = matchingClaims(server, playerId, targetPrize.collectibleId)[0] || null;
        return claim;
      },
    });
    if (!route.completed || !claim) {
      throw new Error(`real-key route did not produce the exact server claim: ${JSON.stringify({ targetPrize, route })}`);
    }
    // Stop residual ship inertia as soon as the exact target is claimed. The
    // deterministic wave can place unrelated prizes along the route; those
    // are legitimate gameplay, but they must not be allowed to race a later
    // target-specific reconciliation sample.
    const brake = await brakeClientVelocity(actor, 4_000);
    let targetConvergence = null;
    await waitUntil(`${browser} ${arena} private prize convergence`, async () => {
      state = await actor.pageState();
      const local = state.uiMetrics?.localGameplayState || {};
      const pickupBeforeLocal = pickupBefore.uiMetrics.localGameplayState;
      const packets = logicalPrizePackets(state, targetPrize.collectibleId);
      lastPrizePacketEvidence = prizePacketCandidates(state);
      const acknowledgement = packets.acknowledgements.at(-1);
      if (packets.requests.length < 1 || acknowledgement == null) return false;
      const candidates = [
        local,
        ...(state.prizeStateTrace || []).map((entry) => ({
          prizeAuthorityLifeSeq: entry.lifeSeq,
          prizeAuthorityRevision: entry.revision,
          bricks: entry.bricks,
          prizePickupNotificationCount: entry.notificationCount,
          lastPrizePickupNotification: entry.lastNotification,
          prizePickupSoundPlayCount: entry.soundPlayCount,
          activePrizeCollectibleIds: entry.activePrizeCollectibleIds,
        })),
      ];
      targetConvergence = candidates.find((candidate) => {
        const revisionAdvance =
          (candidate.prizeAuthorityRevision - pickupBeforeLocal.prizeAuthorityRevision) >>> 0;
        return candidate.prizeAuthorityLifeSeq === acknowledgement.lifeSeq &&
          candidate.prizeAuthorityRevision === acknowledgement.revision &&
          candidate.bricks === pickupBeforeLocal.bricks + 1 &&
        revisionAdvance > 0 && revisionAdvance < 0x8000_0000 &&
          candidate.prizePickupNotificationCount ===
          pickupBeforeLocal.prizePickupNotificationCount + 1 &&
          candidate.lastPrizePickupNotification === "Brick increased." &&
          candidate.prizePickupSoundPlayCount === pickupBeforeLocal.prizePickupSoundPlayCount + 1 &&
          !candidate.activePrizeCollectibleIds.includes(targetPrize.collectibleId);
      }) || null;
      return targetConvergence != null;
    }, 8_000);
    // Bind the target-specific checks to the immutable page state that
    // satisfied convergence. A slow browser can legitimately overlap a
    // different natural prize before another page read; that later authority
    // revision must not replace the exact acknowledgement under test.
    const converged = state;
    const packets = logicalPrizePackets(converged, targetPrize.collectibleId);
    const acknowledgement = packets.acknowledgements[0];
    const afterLocal = targetConvergence;
    const audioEvidence = await prizeAudioProbe(actor);
    const expectedReliableRequest = arena === "extreme";
    const acknowledgementsMatch = packets.acknowledgements.every((candidate) =>
      candidate.collectibleId === acknowledgement.collectibleId &&
      candidate.x === acknowledgement.x && candidate.y === acknowledgement.y &&
      candidate.prizeId === acknowledgement.prizeId &&
      candidate.lifeSeq === acknowledgement.lifeSeq &&
      candidate.revision === acknowledgement.revision
    );
    if (
      acknowledgement.prizeId !== 26 || acknowledgement.x !== targetPrize.x ||
      acknowledgement.y !== targetPrize.y ||
      acknowledgement.lifeSeq !== converged.uiMetrics.playerLifeSeq ||
      acknowledgement.revision !== afterLocal.prizeAuthorityRevision ||
      claim.x !== targetPrize.x || claim.y !== targetPrize.y || claim.prizeId !== 26 ||
      packets.requests.some((request) => (request.reliableId != null) !== expectedReliableRequest) ||
      !acknowledgementsMatch
    ) {
      throw new Error(`private acknowledgement did not match the exact natural claim: ${JSON.stringify({ targetPrize, claim, acknowledgement, state: compactPrizeState(converged) })}`);
    }

    await sleep(1_500);
    const reconciled = await actor.pageState();
    const reconciledLocal = reconciled.uiMetrics.localGameplayState;
    const duplicateClaims = matchingClaims(server, playerId, targetPrize.collectibleId);
    const reconciledPackets = logicalPrizePackets(reconciled, targetPrize.collectibleId);
    // Receiving a reliable ACK in the browser worker does not mean the game
    // loop has applied it before the next disposable-game send. Bind retry
    // retirement to the `converged` state above, where the exact target was
    // removed and its feedback/revision applied, rather than comparing raw
    // packet-log sequence numbers across those two lanes.
    const convergedRequestKeys = new Set(packets.requests.map(prizeRequestEvidenceKey));
    const postConvergenceRequests = reconciledPackets.requests.filter((request) =>
      !convergedRequestKeys.has(prizeRequestEvidenceKey(request))
    );
    // A lost application terminal can legitimately make the client submit the
    // same immutable pickup request in a fresh tracked-reliable wrapper.  The
    // first six bytes are that wrapper's type/id; compare the scoped pickup
    // payload so a changed collectible/life/revision still fails closed while
    // transport retries remain valid evidence of the recovery contract.
    const requestPayload = JSON.stringify(prizeRequestPayload(packets.requests[0]));
    const targetRequestCopiesMatch = reconciledPackets.requests.every((request) =>
      JSON.stringify(prizeRequestPayload(request)) === requestPayload
    );
    const acknowledgementCopiesMatch = reconciledPackets.acknowledgements.every((candidate) =>
      candidate.collectibleId === acknowledgement.collectibleId &&
      candidate.x === acknowledgement.x && candidate.y === acknowledgement.y &&
      candidate.prizeId === acknowledgement.prizeId &&
      candidate.lifeSeq === acknowledgement.lifeSeq &&
      candidate.revision === acknowledgement.revision
    );
    const laterFeedbackCount =
      reconciledLocal.prizePickupNotificationCount - afterLocal.prizePickupNotificationCount;
    const laterSoundCount =
      reconciledLocal.prizePickupSoundPlayCount - afterLocal.prizePickupSoundPlayCount;
    const targetFeedbackRepeated =
      laterFeedbackCount > 0 &&
      reconciledLocal.lastPrizePickupNotification === "Brick increased.";
    if (
      duplicateClaims.length !== 1 ||
      reconciledPackets.requests.length < packets.requests.length ||
      postConvergenceRequests.length !== 0 ||
      !targetRequestCopiesMatch ||
      !acknowledgementCopiesMatch ||
      reconciledLocal.bricks !== afterLocal.bricks ||
      laterFeedbackCount < 0 || laterSoundCount < 0 || targetFeedbackRepeated ||
      reconciledLocal.activePrizeCollectibleIds.includes(targetPrize.collectibleId)
    ) {
      throw new Error(`prize retry/reconciliation duplicated or resurrected the collectible: ${JSON.stringify({ duplicateClaims, reconciledPackets, postConvergenceRequests, targetRequestCopiesMatch, acknowledgementCopiesMatch, laterFeedbackCount, laterSoundCount, targetFeedbackRepeated, state: compactPrizeState(reconciled) })}`);
    }
    if (
      audioEvidence.installed !== true ||
      audioEvidence.starts < 1 ||
      audioEvidence.runningStarts < 1 ||
      (audioEvidence.errors || []).length > 0 ||
      audioEvidence.unlock?.gestureSeen !== true ||
      (audioEvidence.unlock?.resumeFailures ?? 0) !== 0
    ) {
      throw new Error(`trusted-input prize audio did not start in a running context: ${JSON.stringify(audioEvidence)}`);
    }

    const securityAudit = assertSecurityRejectAudit(server);
    const lossEvidence = simulatedLossSummary(server);
    if (simulatedTxLossPct > 0 && lossEvidence.packetsDropped < 1) {
      throw new Error(`seeded natural-prize replay observed no realized server loss: ${JSON.stringify({
        simulatedTxLossPct,
        simulatedTxLossSeed,
        lossEvidence,
      })}`);
    }
    const deliveryAudit = await assertDataChannelDeliveryAudit({
      server,
      serverPort,
      browserLogs: [browser === "chrome" ? chrome.logs : firefox.logs],
      disconnect: disconnectPrizeClient,
    });
    const screenshotBytes = browser === "chrome"
      ? Buffer.from(await chromeClient.screenshot(), "base64")
      : Buffer.from(await screenshot(firefox), "base64");
    fs.writeFileSync(path.join(artifacts.temporary, "browser.png"), screenshotBytes);
    await sleep(750);
    const cleanupStatus = await waitForCleanup(serverPort);
    const artifactState = await captureBrowserArtifactState(actor);
    assertCompleteQaTraceCaptures([artifactState.qaTraceCapture]);
    const provenance = await collectArtifactProvenanceAsync({ runStartedAt, e2eScript });
    assertArtifactProvenanceMatches(initialProvenance, provenance);
    const browserConsole = browserConsoleEvidence(actor);
    fs.writeFileSync(path.join(artifacts.temporary, "browser-console.json"), `${JSON.stringify(browserConsole, null, 2)}\n`);
    assertRendererConsoleClean(browserConsole);
    const { events: _consoleEvents, ...browserConsoleSummary } = browserConsole;
    fs.writeFileSync(path.join(artifacts.temporary, "server.log"), server.logs.join(""));
    fs.writeFileSync(path.join(artifacts.temporary, "browser-state.json"), `${JSON.stringify(reconciled, null, 2)}\n`);
    const qaTraceCaptureAudit = [writeQaTraceArtifact({ directory: artifacts.temporary,
      label: "browser", state: artifactState, serverLogs: server.logs, provenance })];
    const result = {
      ok: true,
      fixture: { ...fixture, observedMaxedSpawnLoadout: fixtureStatus.maxed_spawn_loadout },
      browser,
      arena,
      seed,
      playerId,
      targetPrize,
      targetNavigation,
      approach,
      route,
      brake,
      claim,
      request: reconciledPackets.requests[0],
      requestCopies: reconciledPackets.requests.length,
      preApplicationRequestCopies: packets.requests.length,
      postApplicationRequestCopies: postConvergenceRequests.length,
      acknowledgement,
      acknowledgementCopies: reconciledPackets.acknowledgements.length,
      feedback: {
        notification: afterLocal.lastPrizePickupNotification,
        notificationCount: afterLocal.prizePickupNotificationCount,
        soundPlayCount: afterLocal.prizePickupSoundPlayCount,
        laterDistinctNotificationCount: laterFeedbackCount,
        laterDistinctSoundCount: laterSoundCount,
        audioEvidence,
      },
      before: compactPrizeState(before),
      pickupBefore: compactPrizeState(pickupBefore),
      after: compactPrizeState(reconciled),
      impairment: {
        simulatedTxLossPct,
        simulatedTxLossSeed,
        realizedLoss: lossEvidence,
      },
      securityAudit,
      deliveryAudit,
      cleanupStatus,
      provenance,
      qaTraceCaptureAudit,
      browserConsoleAudits: [{ label: "browser", ...browserConsoleSummary }],
      artifactFiles: [],
    };
    const resultPath = path.join(artifacts.temporary, "result.json");
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
    result.artifactFiles = artifactFileManifest(artifacts.temporary, { exclude: ["result.json"] });
    fs.writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
    validateArtifactPathReferences(result, resultPath);
    artifacts.publish();
    console.log(JSON.stringify({ ok: true, browser, arena, seed, targetPrize, claim, acknowledgement }, null, 2));
  } catch (error) {
    artifacts.cleanup();
    const failure = `${target}-failure`;
    fs.mkdirSync(failure, { recursive: false });
    fs.writeFileSync(path.join(failure, "server.log"), server.logs.join(""));
    fs.writeFileSync(path.join(failure, "browser-state.json"), `${JSON.stringify(state ? compactPrizeState(state) : null, null, 2)}\n`);
    fs.writeFileSync(path.join(failure, "prize-packets.json"), `${JSON.stringify(lastPrizePacketEvidence, null, 2)}\n`);
    fs.writeFileSync(path.join(failure, "target-baseline.json"), `${JSON.stringify(targetBaselineEvidence, null, 2)}\n`);
    fs.writeFileSync(path.join(failure, "setup-navigation.json"), `${JSON.stringify(actor?.rendezvousTrace || [], null, 2)}\n`);
    fs.writeFileSync(path.join(failure, "error.txt"), `${error.stack || error.message}\n`);
    fs.writeFileSync(path.join(failure, "browser-console.json"), `${JSON.stringify(browserConsoleEvidence(actor), null, 2)}\n`);
    writeQaTraceFailureResult({ directory: failure, provenance: initialProvenance,
      captures: [{ label: "browser", state: await captureBrowserArtifactState(actor), serverLogs: server.logs }] });
    error.stack = `${error.stack || error.message}\nretained natural-prize failure: ${failure}`;
    throw error;
  } finally {
    await chromeClient?.close().catch(() => {});
    await deleteSession(firefox).catch(() => {});
    await Promise.all([
      stopProcess(chrome?.child),
      stopProcess(firefox?.child),
      stopProcess(server.child),
    ]).catch(() => {});
    if (chrome?.userDataDir) fs.rmSync(chrome.userDataDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 150 });
    staticServer.server.close();
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
